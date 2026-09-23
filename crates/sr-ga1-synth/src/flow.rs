//! The flow: elaborate, map, pack, then search for a placement that routes.
//!
//! The outer loop is the point of this module. A single annealing run costs
//! milliseconds on a 28-CLB fabric, so the tool does not place once and hope;
//! it places from many seeds, routes each, and keeps the best result until
//! either the design fits or the time budget is spent. Failure feeds back:
//! congestion from a failed routing becomes placement cost on the next
//! attempt, rather than being thrown away.

use crate::backend::{self, Wiring};
use crate::carry::CarryPlan;
use crate::constraints::Constraints;
use crate::design::Design;
use crate::fabric::Fabric;
use crate::genlib::CellLibrary;
use crate::loops;
use crate::netlist::Netlist;
use crate::pack::{self, PackedDesign};
use crate::place::{Placement, Placer, Target};
use crate::progress::Progress;
use crate::route::{Router, Routing};
use crate::rrg::Rrg;
use crate::subset;
use crate::yosys::{self, FrontendRequest, Yosys};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effort {
    Low,
    Medium,
    High,
}

impl Effort {
    /// Annealing sweeps per attempt, and how many placement attempts to try
    /// before giving up early.
    fn schedule(self) -> (usize, usize) {
        match self {
            Effort::Low => (20, 4),
            Effort::Medium => (60, 32),
            Effort::High => (140, 400),
        }
    }
    fn route_iterations(self) -> usize {
        // A routing iteration over a few hundred nodes is microseconds, so
        // there is no reason to be stingy: letting the negotiation run longer is
        // much cheaper than throwing the placement away.
        match self {
            Effort::Low => 25,
            Effort::Medium => 60,
            Effort::High => 120,
        }
    }
}

impl std::str::FromStr for Effort {
    type Err = String;
    fn from_str(s: &str) -> Result<Effort, String> {
        match s {
            "low" => Ok(Effort::Low),
            "medium" => Ok(Effort::Medium),
            "high" => Ok(Effort::High),
            other => Err(format!("unknown effort \"{}\"; expected low, medium or high", other)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    pub sources: Vec<PathBuf>,
    pub top: String,
    pub fabric: PathBuf,
    pub effort: Effort,
    pub time_budget: Duration,
    pub seed: u64,
    pub check_only: bool,
    pub keep_intermediates: bool,
    pub workdir: PathBuf,
    pub verbose: bool,
    pub yosys: Option<PathBuf>,
    /// Constraints file, which today carries the board's loop-around wiring.
    pub constraints: Option<PathBuf>,
}

#[derive(Debug)]
pub struct FlowError {
    pub message: String,
    /// Diagnostics that each name a construct and where it came from.
    pub diagnostics: Vec<String>,
}

impl fmt::Display for FlowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        for d in &self.diagnostics {
            write!(f, "\n  {}", d)?;
        }
        Ok(())
    }
}

impl std::error::Error for FlowError {}

impl FlowError {
    fn plain(message: impl Into<String>) -> FlowError {
        FlowError { message: message.into(), diagnostics: Vec::new() }
    }
}

/// What a completed run produces.
pub struct Outcome {
    pub fabric: Fabric,
    pub library: CellLibrary,
    pub carry: Option<CarryPlan>,
    pub constraints: Constraints,
    /// Kept so the report can name the segments a route occupied.
    pub rrg: Rrg,
    pub packed: PackedDesign,
    pub placement: Placement,
    pub routing: Routing,
    pub wiring: Wiring,
    pub config: Design,
    pub attempts: usize,
    pub winning_attempt: usize,
    pub elapsed: Duration,
    pub warnings: Vec<String>,
}

/// Elaboration and mapping only — what `--check` reports.
pub struct CheckOutcome {
    pub fabric: Fabric,
    pub library: CellLibrary,
    pub packed: PackedDesign,
    pub carry: Option<CarryPlan>,
    pub constraints: Constraints,
    pub warnings: Vec<String>,
}

/// Run the frontend: Yosys, the subset check, ABC, and packing.
pub fn front(options: &Options, progress: &mut Progress) -> Result<CheckOutcome, FlowError> {
    let fabric = Fabric::load_file(&options.fabric)
        .map_err(|e| FlowError::plain(format!("{}", e)))?;
    let library = CellLibrary::derive(&fabric);

    let constraints = match &options.constraints {
        Some(path) => Constraints::load_file(path, &fabric)
            .map_err(|e| FlowError::plain(format!("{}", e)))?,
        None => Constraints::default(),
    };

    let mut warnings = Vec::new();

    // The source scan runs before Yosys, so a construct Yosys would silently
    // ignore is reported with its own line rather than vanishing.
    progress.stage("scanning sources");
    let mut diagnostics = Vec::new();
    for source in &options.sources {
        let text = std::fs::read_to_string(source).map_err(|e| {
            FlowError::plain(format!("cannot read {}: {}", source.display(), e))
        })?;
        for d in subset::scan_source(source, &text) {
            diagnostics.push(d.to_string());
        }
    }
    if !diagnostics.is_empty() {
        return Err(FlowError {
            message: format!(
                "{} construct(s) outside the supported subset (see docs/sv-subset.md)",
                diagnostics.len()
            ),
            diagnostics,
        });
    }

    std::fs::create_dir_all(&options.workdir).map_err(|e| {
        FlowError::plain(format!("cannot create {}: {}", options.workdir.display(), e))
    })?;

    let genlib_path = options.workdir.join("fabric.genlib");
    std::fs::write(&genlib_path, library.genlib(&fabric)).map_err(|e| {
        FlowError::plain(format!("cannot write {}: {}", genlib_path.display(), e))
    })?;
    if options.keep_intermediates {
        let _ = std::fs::write(
            options.workdir.join("fabric-implementations.txt"),
            library.implementations_report(),
        );
    }

    // The carry chain is only offered if the fabric actually has a usable
    // one; otherwise adders become ordinary logic and cost far more.
    let carry = CarryPlan::derive(&fabric);
    if let Err(e) = &carry {
        warnings.push(format!("the carry chain is unavailable: {}", e));
    }
    if let Ok(plan) = &carry {
        let path = options.workdir.join(yosys::ARITH_MAP);
        std::fs::write(&path, plan.arith_map()).map_err(|e| {
            FlowError::plain(format!("cannot write {}: {}", path.display(), e))
        })?;
    }

    progress.stage("finding Yosys");
    let tool = Yosys::discover(options.yosys.as_deref())
        .map_err(|e| FlowError::plain(format!("{}", e)))?;
    if let Some(warning) = tool.version_warning() {
        warnings.push(warning);
    }
    if options.verbose {
        progress.note(&format!("using {} ({})", tool.exe.display(), tool.version));
    }

    progress.stage("elaborating and mapping");
    let request = FrontendRequest {
        sources: options.sources.iter().map(|p| absolute(p)).collect(),
        top: options.top.clone(),
        genlib: PathBuf::from("fabric.genlib"),
        workdir: options.workdir.clone(),
        carry_chains: carry.is_ok(),
    };
    let script = request.script();
    tool.run_script(&script, &options.workdir)
        .map_err(|e| FlowError::plain(format!("{}", e)))?;

    // Subset check on the elaborated netlist, which is authoritative: it sees
    // what each construct actually became.
    let elaborated_path = options.workdir.join(yosys::ELABORATED_JSON);
    let elaborated =
        Netlist::load(&elaborated_path).map_err(|e| FlowError::plain(format!("{}", e)))?;
    let top_module = elaborated
        .top(&options.top, &elaborated_path)
        .or_else(|_| {
            elaborated
                .top(&format!("\\{}", options.top), &elaborated_path)
        })
        .map_err(|e| FlowError::plain(format!("{}", e)))?;
    let violations = subset::check_elaborated(top_module);
    if !violations.is_empty() {
        return Err(FlowError {
            message: format!(
                "{} construct(s) the fabric cannot implement (see docs/sv-subset.md)",
                violations.len()
            ),
            diagnostics: violations.iter().map(|d| d.to_string()).collect(),
        });
    }
    let ddio_nets: Vec<String> = fabric
        .ddio
        .iter()
        .flat_map(|d| [d.input.clone(), d.output.clone(), d.dir.clone()])
        .collect();
    let ddio_declared: Vec<String> = constraints
        .ddio
        .iter()
        .flat_map(|d| [d.in_pad.name.clone(), d.out_pad.name.clone(), d.dir_pad.name.clone()])
        .collect();
    for d in subset::check_ddio_references(top_module, &ddio_nets, &ddio_declared) {
        warnings.push(d.to_string());
    }

    progress.stage("packing");
    let mapped_path = options.workdir.join(yosys::MAPPED_JSON);
    let mapped = Netlist::load(&mapped_path).map_err(|e| FlowError::plain(format!("{}", e)))?;
    let mapped_top = mapped
        .top(&options.top, &mapped_path)
        .map_err(|e| FlowError::plain(format!("{}", e)))?;
    let packed = pack::pack(mapped_top, &library, carry.as_ref().ok(), &options.top)
        .map_err(|e| FlowError::plain(format!("{}", e)))?;
    warnings.extend(packed.warnings.clone());

    // Each clock domain needs its own CSB to source it, and there is one CSB
    // per column.
    if packed.clocks.len() > fabric.columns {
        let named: Vec<String> = packed
            .clocks
            .iter()
            .map(|c| packed.names.get(*c).unwrap_or("<unnamed>").to_string())
            .collect();
        return Err(FlowError {
            message: format!(
                "this design has {} clock domains but the fabric has {} columns, and each                  domain needs its own clock selector to source it. Reduce the number of                  distinct clocks, or target a wider fabric.",
                packed.clocks.len(),
                fabric.columns
            ),
            diagnostics: vec![format!("clocks: {}", named.join(", "))],
        });
    }

    validate_constraints(&constraints, &packed, &fabric, options)?;

    // Feedback is a fabric capability, not a placement problem: if a
    // registered value cannot get back to an operation input, no placement can
    // build a design that needs it. Say so now rather than spending the whole
    // time budget failing to route.
    if !fabric.register_feedback_possible() && constraints.loopback.is_empty() {
        if let Some(cycle) = feedback_cycle(&packed) {
            let named: Vec<String> = cycle
                .iter()
                .map(|&cell| packed.cells[cell].name.clone())
                .collect();
            let lines = [
                "this design needs a register to depend on its own value, which the fabric cannot route."
                    .to_string(),
                String::new(),
                "The flip-flop's data input is hard-wired to its own CLB's operation result, so the"
                    .to_string(),
                "only way back to a cell is its column's vertical ring. Vertical lanes".to_string()
                    + &format!(
                        " {:?} carry the",
                        fabric.vert_lanes_for(crate::fabric::Source::Op)
                    ),
                format!(
                    "combinational result and {:?} carry the registered one, the pass-through swap",
                    fabric.vert_lanes_for(crate::fabric::Source::Reg)
                ),
                "keeps those pairs separate, and no operation input reads a registered lane. So a"
                    .to_string(),
                "value can leave a register and travel right, but it can never come back."
                    .to_string(),
                String::new(),
                "Counters, accumulators and LFSRs all need exactly this. Pointing input \"c\" at a"
                    .to_string(),
                "registered vertical lane in fabric.toml, instead of \"v_in:0\", would make them"
                    .to_string(),
                "buildable - and would also remove the combinational loops lane 0 allows today."
                    .to_string(),
                String::new(),
                "Alternatively, wire spare chip outputs back to spare chip inputs on the board and"
                    .to_string(),
                "declare them in a constraints file, and this tool will route the feedback through"
                    .to_string(),
                "them:".to_string(),
                String::new(),
                "    [loopback]".to_string(),
                "    outputs = [\"output_2\", \"output_3\"]".to_string(),
                "    inputs  = [\"input_0\", \"input_1\"]".to_string(),
            ];
            return Err(FlowError {
                message: lines.join("
"),
                diagnostics: vec![format!("the feedback runs through: {}", named.join(" -> "))],
            });
        }
    }

    if packed.clb_count() > fabric.clb_count() {
        return Err(FlowError {
            message: format!(
                "the design needs {} CLBs but the fabric has {}. This is {} too many — the \
                 design has to get smaller; there is no packing that recovers it.",
                packed.clb_count(),
                fabric.clb_count(),
                packed.clb_count() - fabric.clb_count()
            ),
            diagnostics: packed
                .gate_histogram()
                .iter()
                .map(|(gate, count)| format!("{} x {}", count, gate))
                .collect(),
        });
    }

    Ok(CheckOutcome { fabric, library, packed, carry: carry.ok(), constraints, warnings })
}

/// The whole flow, including the place-and-route search.
pub fn run(options: &Options, progress: &mut Progress) -> Result<Outcome, FlowError> {
    let started = Instant::now();
    let CheckOutcome { fabric, library, packed, carry, constraints, mut warnings } =
        front(options, progress)?;

    let rrg = Rrg::with_loopback(&fabric, &constraints.loopback);
    let target =
        Target::with_carry(&fabric, &library, carry.as_ref(), &constraints);
    let placer = Placer::new(&packed, &target);
    let (sweeps, max_attempts) = options.effort.schedule();

    let mut best: Option<(usize, Placement, Wiring, Routing, Design)> = None;
    let mut best_score = usize::MAX;
    let mut attempts = 0usize;
    // The closest attempt that did not fit, kept so the failure report can
    // name the connections that failed rather than just counting them.
    let mut closest: Option<(Wiring, Routing)> = None;
    // Why the best-routed attempts were still unusable, if that is what
    // happened: a routing can succeed and still be impossible to configure.
    let mut last_emit_error: Option<String> = None;
    let mut rejected_for_loops = 0usize;

    for attempt in 0..max_attempts {
        if started.elapsed() >= options.time_budget && best.is_some() {
            break;
        }
        if started.elapsed() >= options.time_budget && attempt > 0 {
            break;
        }
        attempts += 1;
        let seed = options.seed.wrapping_add(attempt as u64 * 0x9E3779B9);
        let (placement, _cost) = placer.anneal(seed, sweeps);

        let wiring = backend::build_wiring(&packed, &placement, &target, &rrg);
        let mut router = Router::new(&rrg, &fabric, &library);
        let routing = router.route(&wiring.problem, options.effort.route_iterations());

        // How far off this attempt is. Unreached sinks and leftover congestion
        // both make a routing unusable, so both count. Scoring on unrouted
        // sinks alone lets a congested attempt with nothing unrouted set the bar
        // at zero, after which no later attempt looks like an improvement and
        // the search quietly stops evaluating anything at all.
        let score = routing.unrouted.len() + routing.congested.len();

        if routing.is_complete() {
            // A complete routing still has to be configurable and loop-free
            // before it counts as a result.
            match backend::emit(&fabric, &packed, &placement, &target, &rrg, &wiring, &routing) {
                Ok(config) => {
                    let used: Vec<(crate::rrg::Node, crate::rrg::Node)> =
                        routing.loopbacks(&rrg).into_iter().map(|(a, b, _)| (a, b)).collect();
                    let cycles = loops::find_cycles(&fabric, &config, &used);
                    if cycles.is_empty() {
                        best_score = 0;
                        best = Some((attempt, placement, wiring, routing, config));
                    } else {
                        rejected_for_loops += 1;
                        if options.verbose {
                            progress.note(&format!(
                                "attempt {} routed but closed {} combinational loop(s);                                  rejecting it",
                                attempt,
                                cycles.len()
                            ));
                        }
                    }
                }
                Err(e) => {
                    if options.verbose {
                        progress
                            .note(&format!("attempt {} could not be emitted: {}", attempt, e));
                    }
                    last_emit_error = Some(format!("{}", e));
                }
            }
        } else if score < best_score {
            best_score = score;
            closest = Some((wiring, routing));
        }

        progress.search(attempt + 1, started.elapsed(), options.time_budget, best_score);
        if best.is_some() {
            // Fitted; nothing is gained by looking further.
            break;
        }
    }

    let Some((winning_attempt, placement, wiring, routing, config)) = best else {
        return Err(failure_report(
            &fabric,
            &packed,
            &rrg,
            attempts,
            best_score,
            closest,
            last_emit_error,
            rejected_for_loops,
        ));
    };

    let rings = loops::driverless_rings(&fabric, &config);
    for (col, lane) in rings {
        warnings.push(format!(
            "column {} vertical lane {} is left in full pass-through, so nothing drives it",
            col, lane
        ));
    }

    Ok(Outcome {
        fabric,
        library,
        carry,
        constraints,
        rrg,
        packed,
        placement,
        routing,
        wiring,
        config,
        attempts,
        winning_attempt,
        elapsed: started.elapsed(),
        warnings,
    })
}

/// "Routing failed" on its own is useless on a fabric this constrained, so
/// say what ran out.
fn failure_report(
    fabric: &Fabric,
    packed: &PackedDesign,
    rrg: &Rrg,
    attempts: usize,
    unrouted: usize,
    closest: Option<(Wiring, Routing)>,
    emit_error: Option<String>,
    rejected_for_loops: usize,
) -> FlowError {
    let mut diagnostics = Vec::new();
    diagnostics.push(format!(
        "{} of {} CLBs used by logic before routing ({} register(s), {} unconditional, which          want the row whose lane {} enters as a constant 1)",
        packed.clb_count(),
        fabric.clb_count(),
        packed.register_count(),
        packed.always_enabled(),
        backend::enable_lane(fabric).map(|l| l.to_string()).unwrap_or_else(|| "?".into())
    ));

    if let Some((_wiring, routing)) = closest {
        diagnostics.push(format!(
            "the closest attempt left {} connection(s) unrouted after {} routing iteration(s):",
            routing.unrouted.len(),
            routing.iterations
        ));
        // Name what failed. On this fabric the reason is nearly always a
        // specific resource, so say which sink and which net wanted it.
        for failure in routing.unrouted.iter().take(12) {
            diagnostics
                .push(format!("    \"{}\" could not reach {}", failure.net_name, failure.what));
        }
        if routing.unrouted.len() > 12 {
            diagnostics.push(format!("    ... and {} more", routing.unrouted.len() - 12));
        }

        if !routing.congested.is_empty() {
            diagnostics.push("segments wanted by more than one net:".to_string());
            let mut listed = routing.congested.clone();
            listed.sort_by_key(|&(_, count)| std::cmp::Reverse(count));
            for (node, count) in listed.iter().take(10) {
                diagnostics.push(format!("    {} wanted by {} nets", node, count));
            }
        }

        // Which lanes filled up, per row and column: the most useful single
        // number when a design nearly fits.
        let mut horz: std::collections::BTreeMap<(usize, usize), usize> = Default::default();
        let mut vert: std::collections::BTreeMap<(usize, usize), usize> = Default::default();
        for route in &routing.routes {
            for &node in &route.nodes {
                match rrg.node(node) {
                    crate::rrg::Node::HSeg { row, lane, .. } => {
                        *horz.entry((row, lane)).or_insert(0) += 1
                    }
                    crate::rrg::Node::VSeg { col, lane, .. } => {
                        *vert.entry((col, lane)).or_insert(0) += 1
                    }
                    _ => {}
                }
            }
        }
        let segments = fabric.columns + 1;
        for row in 0..fabric.rows {
            let counts: Vec<String> = (0..fabric.horz_lanes)
                .map(|lane| {
                    format!("h{}={}/{}", lane, horz.get(&(row, lane)).copied().unwrap_or(0), segments)
                })
                .collect();
            diagnostics.push(format!("    row {} {}", row, counts.join("  ")));
        }
        for col in 0..fabric.columns {
            let counts: Vec<String> = (0..fabric.vert_lanes)
                .map(|lane| {
                    format!("v{}={}/{}", lane, vert.get(&(col, lane)).copied().unwrap_or(0), fabric.rows)
                })
                .collect();
            diagnostics.push(format!("    col {} {}", col, counts.join("  ")));
        }
        diagnostics.push(format!(
            "    {} CLB(s) spent on routing buffers in that attempt",
            routing.buffers.len()
        ));
    } else if unrouted != usize::MAX {
        diagnostics.push(format!("the closest attempt left {} connection(s) unrouted", unrouted));
    }

    if let Some(reason) = emit_error {
        diagnostics.push(format!(
            "some attempts routed but could not be configured: {}",
            reason
        ));
    }
    if rejected_for_loops > 0 {
        diagnostics.push(format!(
            "{} attempt(s) routed but closed a combinational loop and were rejected",
            rejected_for_loops
        ));
    }

    FlowError {
        message: format!("could not fit the design after {} placement attempt(s)", attempts),
        diagnostics,
    }
}

/// Check the constraints against the elaborated design.
///
/// A constraint that names something the design does not have is a mistake worth
/// reporting: silently ignoring it looks exactly like honouring it.
fn validate_constraints(
    constraints: &Constraints,
    packed: &PackedDesign,
    fabric: &Fabric,
    options: &Options,
) -> Result<(), FlowError> {
    let where_from = options
        .constraints
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "the constraints".to_string());
    let fail = |message: String, diagnostics: Vec<String>| FlowError {
        message: format!("{}: {}", where_from, message),
        diagnostics,
    };

    let is_input = |pad: &str| fabric.io_inputs.iter().flatten().any(|n| n == pad);
    let is_output =
        |pad: &str| fabric.io_outputs.iter().flatten().flatten().any(|n| n == pad);
    let input_names: Vec<&str> = packed.inputs.iter().map(|p| p.name.as_str()).collect();
    let output_names: Vec<&str> = packed.outputs.iter().map(|p| p.name.as_str()).collect();

    // ---- pin locks ---------------------------------------------------------
    for (signal, pad) in &constraints.pins {
        let signal_is_input = input_names.contains(&signal.as_str());
        let signal_is_output = output_names.contains(&signal.as_str());
        if !signal_is_input && !signal_is_output {
            return Err(fail(
                format!("\"{}\" is not a port of this design", signal),
                vec![
                    format!("inputs:  {}", input_names.join(", ")),
                    format!("outputs: {}", output_names.join(", ")),
                ],
            ));
        }
        if signal_is_input && !is_input(&pad.name) {
            return Err(fail(
                format!(
                    "\"{}\" is a design input but \"{}\" is a chip output",
                    signal, pad.name
                ),
                Vec::new(),
            ));
        }
        if signal_is_output && !is_output(&pad.name) {
            return Err(fail(
                format!(
                    "\"{}\" is a design output but \"{}\" is a chip input",
                    signal, pad.name
                ),
                Vec::new(),
            ));
        }
        if packed.reset == packed.inputs.iter().find(|p| p.name == *signal).map(|p| p.net) {
            return Err(fail(
                format!(
                    "\"{}\" is the global reset, which is a dedicated pin fanned out in                      hardware; it takes no routable pad and cannot be locked to \"{}\"",
                    signal, pad.name
                ),
                Vec::new(),
            ));
        }
    }

    // ---- DDIO --------------------------------------------------------------
    for binding in &constraints.ddio {
        if !input_names.contains(&binding.input.as_str()) {
            return Err(fail(
                format!(
                    "DDIO pad {} names \"{}\" as its input net, which is not a design input",
                    binding.pin, binding.input
                ),
                vec![format!("inputs: {}", input_names.join(", "))],
            ));
        }
        for (net, what) in [(&binding.output, "output"), (&binding.dir, "direction")] {
            if !output_names.contains(&net.as_str()) {
                return Err(fail(
                    format!(
                        "DDIO pad {} names \"{}\" as its {} net, which is not a design output",
                        binding.pin, net, what
                    ),
                    vec![format!("outputs: {}", output_names.join(", "))],
                ));
            }
        }
    }

    // ---- instance placement ------------------------------------------------
    for lock in &constraints.placement {
        if !packed.cells.iter().any(|c| c.name == lock.name) {
            let mut known: Vec<&str> = packed.cells.iter().map(|c| c.name.as_str()).collect();
            known.sort_unstable();
            return Err(fail(
                format!(
                    "no cell named \"{}\"; cells are named after the signal they produce, as                      shown in the placement section of the report",
                    lock.name
                ),
                vec![format!("cells: {}", known.join(", "))],
            ));
        }
    }

    // A carry chain has no placement freedom beyond its column, so a lock on
    // one of its cells has to agree with the chain's geometry.
    for (index, chain) in packed.chains.iter().enumerate() {
        let lsb_row = 0;
        let mut column: Option<(usize, &str)> = None;
        for (position, &cell) in chain.iter().enumerate() {
            let name = packed.cells[cell].name.as_str();
            let Some(lock) = constraints.placement.iter().find(|l| l.name == name) else {
                continue;
            };
            let wanted = lsb_row + position;
            if lock.row != wanted {
                return Err(fail(
                    format!(
                        "\"{}\" is bit {} of an adder, so the carry chain requires it in row {},                          but it is pinned to row {}",
                        name, position, wanted, lock.row
                    ),
                    vec![
                        "A chain runs up one column with the least significant bit at the bottom;                          its cells cannot be reordered."
                            .to_string(),
                    ],
                ));
            }
            if let Some((other_col, other_name)) = column {
                if other_col != lock.col {
                    return Err(fail(
                        format!(
                            "\"{}\" and \"{}\" are bits of the same adder but are pinned to                              columns {} and {}",
                            other_name, name, other_col, lock.col
                        ),
                        vec!["A carry chain occupies a single column.".to_string()],
                    ));
                }
            }
            column = Some((lock.col, name));
        }
        let _ = index;
    }

    // ---- clock assignment --------------------------------------------------
    for net in constraints.clocks.keys() {
        let known: Vec<String> = packed
            .clocks
            .iter()
            .map(|c| packed.names.get(*c).unwrap_or("<unnamed>").to_string())
            .collect();
        if !known.iter().any(|k| k == net) {
            return Err(fail(
                format!("\"{}\" is not a clock in this design", net),
                vec![format!(
                    "clocks: {}",
                    if known.is_empty() { "none".to_string() } else { known.join(", ") }
                )],
            ));
        }
    }

    Ok(())
}

/// A cycle in the cell dependency graph, following both combinational and
/// registered outputs.
///
/// Any such cycle is unroutable here. A combinational one is illegal outright,
/// and a registered one cannot be built because `_reg` never returns to an
/// operation input. Returns the cells on the cycle, in order.
fn feedback_cycle(packed: &PackedDesign) -> Option<Vec<usize>> {
    // Who drives each net, whether combinationally or through a register.
    let mut driver: std::collections::BTreeMap<u32, usize> = Default::default();
    for (index, cell) in packed.cells.iter().enumerate() {
        if let Some(net) = cell.op {
            driver.insert(net, index);
        }
        if let Some(reg) = &cell.reg {
            driver.insert(reg.q, index);
        }
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        New,
        Open,
        Done,
    }
    let mut mark = vec![Mark::New; packed.cells.len()];
    let mut path: Vec<usize> = Vec::new();

    fn walk(
        cell: usize,
        packed: &PackedDesign,
        driver: &std::collections::BTreeMap<u32, usize>,
        mark: &mut Vec<Mark>,
        path: &mut Vec<usize>,
    ) -> Option<Vec<usize>> {
        mark[cell] = Mark::Open;
        path.push(cell);
        for pin in &packed.cells[cell].pins {
            let crate::pack::Signal::Net(net) = pin else { continue };
            let Some(&upstream) = driver.get(net) else { continue };
            match mark[upstream] {
                Mark::Open => {
                    let at = path.iter().position(|&c| c == upstream).unwrap_or(0);
                    return Some(path[at..].to_vec());
                }
                Mark::New => {
                    if let Some(found) = walk(upstream, packed, driver, mark, path) {
                        return Some(found);
                    }
                }
                Mark::Done => {}
            }
        }
        path.pop();
        mark[cell] = Mark::Done;
        None
    }

    for cell in 0..packed.cells.len() {
        if mark[cell] == Mark::New {
            if let Some(found) = walk(cell, packed, &driver, &mut mark, &mut path) {
                return Some(found);
            }
        }
    }
    None
}

fn absolute(path: &Path) -> PathBuf {
    std::fs::canonicalize(path)
        .map(|p| {
            // Strip the Windows verbatim prefix, which Yosys scripts dislike.
            let text = p.to_string_lossy().replace("\\\\?\\", "");
            PathBuf::from(text)
        })
        .unwrap_or_else(|_| path.to_path_buf())
}
