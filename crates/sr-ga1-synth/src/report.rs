//! The synthesis report.
//!
//! On a 28-CLB fabric the interesting question is almost never "did it
//! work" but "what did it cost, and what is nearly full". So the report
//! leads with utilisation against capacity and lane occupancy per row and
//! column, and says explicitly how each register's enable was satisfied —
//! that being the constraint most likely to decide whether the next change
//! still fits.

use crate::backend;
use crate::designjson::LoopbackLink;
use crate::flow::Outcome;
use crate::pack::{Enable, PackedDesign};
use crate::place::Target;
use crate::rrg::Node;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Longest chain of operation cores a signal passes through. There is no
/// timing model, so this is a structural depth, not a delay.
pub fn logic_depth(packed: &PackedDesign) -> usize {
    let mut driver: BTreeMap<u32, usize> = BTreeMap::new();
    for (index, cell) in packed.cells.iter().enumerate() {
        if let Some(net) = cell.op {
            driver.insert(net, index);
        }
    }
    let mut depth: Vec<Option<usize>> = vec![None; packed.cells.len()];

    // Memoised depth-first walk. A register output is a boundary: its depth
    // restarts, because the flip-flop breaks the combinational path.
    fn walk(
        cell: usize,
        packed: &PackedDesign,
        driver: &BTreeMap<u32, usize>,
        depth: &mut Vec<Option<usize>>,
        visiting: &mut Vec<bool>,
    ) -> usize {
        if let Some(d) = depth[cell] {
            return d;
        }
        if visiting[cell] {
            return 0; // guards against a cycle the loop checker will reject
        }
        visiting[cell] = true;
        let mut deepest = 0;
        for pin in &packed.cells[cell].pins {
            if let crate::pack::Signal::Net(net) = pin {
                if let Some(&upstream) = driver.get(net) {
                    // A registered source starts a new path.
                    let registered = packed.cells[upstream]
                        .reg
                        .as_ref()
                        .is_some_and(|r| r.q == *net);
                    if !registered {
                        deepest = deepest.max(walk(upstream, packed, driver, depth, visiting));
                    }
                }
            }
        }
        visiting[cell] = false;
        let result = deepest + 1;
        depth[cell] = Some(result);
        result
    }

    let mut visiting = vec![false; packed.cells.len()];
    (0..packed.cells.len())
        .map(|c| walk(c, packed, &driver, &mut depth, &mut visiting))
        .max()
        .unwrap_or(0)
}

/// The board wiring this configuration depends on.
///
/// Read back off the finished routing rather than decided in advance: the pool
/// in the constraints file says which pads *may* be wired, and the router picks
/// which pairs it actually needs.
pub fn loopback_wiring(outcome: &Outcome) -> Vec<LoopbackLink> {
    let pad_name = |node: crate::rrg::Node| -> Option<String> {
        match node {
            crate::rrg::Node::HSeg { row, col, lane } if col == outcome.fabric.columns => outcome
                .fabric
                .io_outputs
                .get(row)
                .and_then(|lanes| lanes.get(lane))
                .cloned()
                .flatten(),
            crate::rrg::Node::HSeg { row, col: 0, lane } => {
                outcome.fabric.io_inputs.get(row).and_then(|lanes| lanes.get(lane)).cloned()
            }
            _ => None,
        }
    };
    let mut out = Vec::new();
    for (source, sink, net) in outcome.routing.loopbacks(&outcome.rrg) {
        let (Some(from), Some(to)) = (pad_name(source), pad_name(sink)) else { continue };
        out.push(LoopbackLink {
            from,
            to,
            net: outcome
                .wiring
                .problem
                .nets
                .get(net)
                .map(|n| n.name.clone())
                .unwrap_or_default(),
        });
    }
    out.sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
    out.dedup();
    out
}

/// Whether a pin was chosen by the tool or pinned by the user, which the report
/// has to distinguish: a constrained pin is not a result, it is an input.
fn pad_source(outcome: &Outcome, signal: &str) -> &'static str {
    let constraints = &outcome.constraints;
    if constraints.pins.contains_key(signal) {
        return "locked by constraint";
    }
    if constraints
        .ddio
        .iter()
        .any(|d| d.input == signal || d.output == signal || d.dir == signal)
    {
        return "DDIO declaration";
    }
    "automatic"
}

/// Chip pad -> the design signal it carries.
///
/// The GUI's `pinned` map names blocks, not nets, so the reserved pad names are
/// all it would otherwise have for the IO. Carrying the design's own names over
/// means a signal reads as `clk` rather than `input_8` — and the constants and
/// the dedicated reset are deliberately absent, having no design signal.
pub fn io_names(outcome: &Outcome) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    for port in &outcome.packed.inputs {
        if let Some(pad) = outcome.placement.input_pins.get(&port.net) {
            out.insert(pad.clone(), port.name.clone());
        }
    }
    for port in &outcome.packed.outputs {
        if let Some(pad) = outcome.placement.output_pins.get(&port.net) {
            out.insert(pad.clone(), port.name.clone());
        }
    }
    out
}

pub fn render(outcome: &Outcome, target: &Target) -> String {
    let fabric = &outcome.fabric;
    let packed = &outcome.packed;
    let mut out = String::new();

    let _ = writeln!(out, "SR-GA1 synthesis report");
    let _ = writeln!(out, "=======================");
    let _ = writeln!(out);
    let _ = writeln!(out, "design            {}", packed.top);
    let _ = writeln!(out, "fabric            {} ({}x{})", fabric.name, fabric.columns, fabric.rows);
    let _ = writeln!(out, "bitstream length  {} bits", fabric.total_bits());
    let _ = writeln!(
        out,
        "search            {} attempt(s), attempt {} won, {:.2}s elapsed",
        outcome.attempts,
        outcome.winning_attempt + 1,
        outcome.elapsed.as_secs_f64()
    );
    let _ = writeln!(out, "routing           converged in {} iteration(s)", outcome.routing.iterations);

    // ---- utilisation ------------------------------------------------------
    let logic_cells = packed.clb_count();
    let buffers = outcome.routing.buffers.len();
    let used = logic_cells + buffers;
    let _ = writeln!(out);
    let _ = writeln!(out, "CLB usage");
    let _ = writeln!(out, "---------");
    let _ = writeln!(
        out,
        "  {} of {} ({:.0}%)",
        used,
        fabric.clb_count(),
        100.0 * used as f64 / fabric.clb_count() as f64
    );
    let _ = writeln!(out, "  {} mapped logic, {} routing buffers", logic_cells, buffers);

    let _ = writeln!(out);
    let _ = writeln!(out, "cell types");
    let _ = writeln!(out, "----------");
    for (gate, count) in packed.gate_histogram() {
        let _ = writeln!(out, "  {:<10} {}", gate, count);
    }

    // ---- registers and enables --------------------------------------------
    let _ = writeln!(out);
    let _ = writeln!(out, "registers");
    let _ = writeln!(out, "---------");
    let _ = writeln!(out, "  {} register(s)", packed.register_count());
    let enable_lane = backend::enable_lane(fabric);
    if let Some(lane) = enable_lane {
        let free_row = (0..fabric.rows).find(|&r| fabric.io_constant(r, lane) == Some(true));
        let _ = writeln!(
            out,
            "  lane {} is the write enable; {}",
            lane,
            match free_row {
                Some(row) => format!("row {} receives a constant 1 at the left edge", row),
                None => "no row receives a free constant 1".to_string(),
            }
        );
        for (index, cell) in packed.cells.iter().enumerate() {
            let Some(reg) = &cell.reg else { continue };
            let placed = &outcome.placement.cells[index];
            let how = match &reg.enable {
                Enable::Always if Some(placed.row) == free_row => {
                    "always, free from the row's constant".to_string()
                }
                Enable::Always => "always, driven by an upstream CLB".to_string(),
                Enable::Net(net) => format!(
                    "gated by {}",
                    packed.names.get(*net).unwrap_or("<unnamed>")
                ),
            };
            let _ = writeln!(
                out,
                "  {:<16} CLB({}, {})  reset={}  {}",
                cell.name,
                placed.col,
                placed.row,
                u8::from(reg.reset_value),
                how
            );
        }
    }

    // ---- carry chains ------------------------------------------------------
    let _ = writeln!(out);
    let _ = writeln!(out, "carry chains");
    let _ = writeln!(out, "------------");
    if packed.chains.is_empty() {
        let _ = writeln!(
            out,
            "  none used. A chain runs up a single column, at most {} cells, and the top \
             cell's carry-out is unreachable.",
            fabric.rows
        );
    } else {
        for (index, chain) in packed.chains.iter().enumerate() {
            let column = chain
                .first()
                .map(|&c| outcome.placement.cells[c].row)
                .map(|_| outcome.placement.cells[chain[0]].col);
            let bits: Vec<String> = chain
                .iter()
                .map(|&cell| {
                    let placed = &outcome.placement.cells[cell];
                    format!("row {} = {}", placed.row, packed.cells[cell].name)
                })
                .collect();
            let _ = writeln!(
                out,
                "  chain {}: {} of {} cells in column {}, least significant at the bottom",
                index,
                chain.len(),
                fabric.rows,
                column.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
            );
            for bit in bits {
                let _ = writeln!(out, "    {}", bit);
            }
            if chain.len() == fabric.rows {
                let _ = writeln!(
                    out,
                    "    this chain fills the column; the top cell's carry-out reaches nothing, \
                     so a wider addition cannot extend it"
                );
            }
        }
    }

    // ---- lane utilisation --------------------------------------------------
    let mut horz: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    let mut vert: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    for route in &outcome.routing.routes {
        for &node in &route.nodes {
            match outcome.rrg.node(node) {
                Node::HSeg { row, lane, .. } => *horz.entry((row, lane)).or_insert(0) += 1,
                Node::VSeg { col, lane, .. } => *vert.entry((col, lane)).or_insert(0) += 1,
                _ => {}
            }
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "horizontal lane segments in use (per row)");
    let _ = writeln!(out, "-----------------------------------------");
    let segments = fabric.columns + 1;
    for row in 0..fabric.rows {
        let counts: Vec<String> = (0..fabric.horz_lanes)
            .map(|lane| format!("{:>2}/{}", horz.get(&(row, lane)).copied().unwrap_or(0), segments))
            .collect();
        let _ = writeln!(out, "  row {}  {}", row, counts.join("  "));
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "vertical ring segments in use (per column)");
    let _ = writeln!(out, "------------------------------------------");
    for col in 0..fabric.columns {
        let counts: Vec<String> = (0..fabric.vert_lanes)
            .map(|lane| {
                format!("{:>2}/{}", vert.get(&(col, lane)).copied().unwrap_or(0), fabric.rows)
            })
            .collect();
        let _ = writeln!(out, "  col {}  {}", col, counts.join("  "));
    }

    // ---- depth -------------------------------------------------------------
    let _ = writeln!(out);
    let _ = writeln!(out, "logic depth");
    let _ = writeln!(out, "-----------");
    let _ = writeln!(
        out,
        "  {} operation core(s) on the longest combinational path (no timing model exists)",
        logic_depth(packed)
    );

    // ---- clocks ------------------------------------------------------------
    let _ = writeln!(out);
    let _ = writeln!(out, "clock domains");
    let _ = writeln!(out, "-------------");
    if packed.clocks.is_empty() {
        let _ = writeln!(out, "  none: the design is purely combinational, every CSB left at default");
    } else {
        for clock in &packed.clocks {
            let column = outcome.placement.clock_columns.get(clock).copied().unwrap_or(0);
            let _ = writeln!(
                out,
                "  {:<16} sourced by CSB{} from column {}'s vertical ring",
                packed.names.get(*clock).unwrap_or("<unnamed>"),
                column,
                column
            );
        }
        // The column partition: each uncoupled CSB owns a contiguous block of
        // columns, and a register only ever sees its own column's clock.
        let domains = outcome.placement.clock_domains(fabric.columns);
        let _ = writeln!(out, "  column partition:");
        for clock in &packed.clocks {
            let columns: Vec<String> = domains
                .iter()
                .enumerate()
                .filter(|(_, owner)| **owner == Some(*clock))
                .map(|(col, _)| col.to_string())
                .collect();
            let registers = packed
                .cells
                .iter()
                .enumerate()
                .filter(|(_, c)| c.reg.as_ref().is_some_and(|r| r.clock == *clock))
                .count();
            let _ = writeln!(
                out,
                "    {:<16} columns [{}], {} register(s)",
                packed.names.get(*clock).unwrap_or("<unnamed>"),
                columns.join(", "),
                registers
            );
        }
        let orphans: Vec<usize> = domains
            .iter()
            .enumerate()
            .filter(|(_, owner)| owner.is_none())
            .map(|(col, _)| col)
            .collect();
        if !orphans.is_empty() {
            let _ = writeln!(
                out,
                "    no clock reaches columns [{}]; nothing sequential can be placed there",
                orphans.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", ")
            );
        }
    }
    if let Some(reset) = packed.reset {
        let _ = writeln!(
            out,
            "  reset {} is the global pin, fanned out in hardware and sampled on each \
             column's clock",
            packed.names.get(reset).unwrap_or("<unnamed>")
        );
    }

    // ---- pins --------------------------------------------------------------
    let _ = writeln!(out);
    let _ = writeln!(out, "pin assignment");
    let _ = writeln!(out, "--------------");
    for port in &packed.inputs {
        if packed.reset == Some(port.net) {
            let _ = writeln!(
                out,
                "  in   {:<16} -> dedicated reset pin, fanned out in hardware (no routing)",
                port.name
            );
            continue;
        }
        if let Some(pad) = outcome.placement.input_pins.get(&port.net) {
            let position = target
                .input_pad(pad)
                .map(|(r, l)| format!("row {} lane {}", r, l))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "  in   {:<16} -> {:<12} {}  ({})",
                port.name,
                pad,
                position,
                pad_source(outcome, &port.name)
            );
        }
    }
    for port in &packed.outputs {
        if let Some(pad) = outcome.placement.output_pins.get(&port.net) {
            let position = target
                .output_pad(pad)
                .map(|(r, l)| format!("row {} lane {}", r, l))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "  out  {:<16} -> {:<12} {}  ({})",
                port.name,
                pad,
                position,
                pad_source(outcome, &port.name)
            );
        }
    }

    // ---- loop-around wiring ------------------------------------------------
    let _ = writeln!(out);
    let _ = writeln!(out, "loop-around wiring");
    let _ = writeln!(out, "------------------");
    let pool = &outcome.constraints.loopback;
    if pool.is_empty() {
        let _ = writeln!(
            out,
            "  none declared. A board wire from a chip output back to a chip input is the only              way to move a signal leftward or back into the cell that produced it."
        );
    } else {
        let used = loopback_wiring(outcome);
        let _ = writeln!(
            out,
            "  pool: {} output pad(s), {} input pad(s)",
            pool.outputs.len(),
            pool.inputs.len()
        );
        if used.is_empty() {
            let _ = writeln!(out, "  none needed: this design routes entirely inside the fabric");
        } else {
            let _ = writeln!(out, "  THIS DESIGN REQUIRES THE FOLLOWING PHYSICAL CONNECTIONS:");
            for link in &used {
                let _ = writeln!(
                    out,
                    "    wire {:<12} -> {:<12} carrying {}",
                    link.from, link.to, link.net
                );
            }
            let spare_out: Vec<&str> = pool
                .outputs
                .iter()
                .map(|p| p.name.as_str())
                .filter(|name| !used.iter().any(|l| l.from == *name))
                .collect();
            let spare_in: Vec<&str> = pool
                .inputs
                .iter()
                .map(|p| p.name.as_str())
                .filter(|name| !used.iter().any(|l| l.to == *name))
                .collect();
            if !spare_out.is_empty() || !spare_in.is_empty() {
                let _ = writeln!(
                    out,
                    "  unused pool pads: outputs [{}], inputs [{}]",
                    spare_out.join(", "),
                    spare_in.join(", ")
                );
            }
            let _ = writeln!(
                out,
                "  Note: the visual programmer cannot model board wiring, so its simulation"
            );
            let _ = writeln!(
                out,
                "  of this design will diverge from hardware. See docs/gui-loopback-support.md."
            );
        }
    }

    // ---- DDIO --------------------------------------------------------------
    let _ = writeln!(out);
    let _ = writeln!(out, "DDIO");
    let _ = writeln!(out, "----");
    let _ = writeln!(
        out,
        "  no DDIO pins declared; both pads stay at their default with the input path gated off"
    );

    // ---- constraints -------------------------------------------------------
    let _ = writeln!(out);
    let _ = writeln!(out, "constraints");
    let _ = writeln!(out, "-----------");
    let constraints = &outcome.constraints;
    if constraints.is_empty() {
        let _ = writeln!(out, "  none: everything above was chosen by the tool");
    } else {
        for (signal, pad) in &constraints.pins {
            let _ = writeln!(out, "  pin       {:<16} -> {}", signal, pad.name);
        }
        for binding in &constraints.ddio {
            let _ = writeln!(
                out,
                "  ddio {}    in={} out={} dir={}",
                binding.pin, binding.input, binding.output, binding.dir
            );
        }
        for lock in &constraints.placement {
            let _ = writeln!(out, "  placement {:<16} -> CLB({}, {})", lock.name, lock.col, lock.row);
        }
        for (net, column) in &constraints.clocks {
            let _ = writeln!(out, "  clock     {:<16} -> column {}", net, column);
        }
        if !constraints.loopback.is_empty() {
            let _ = writeln!(
                out,
                "  loopback  pool of {} output(s) and {} input(s)",
                constraints.loopback.outputs.len(),
                constraints.loopback.inputs.len()
            );
        }
    }

    // ---- placement map -----------------------------------------------------
    let _ = writeln!(out);
    let _ = writeln!(out, "placement");
    let _ = writeln!(out, "---------");
    for row in (0..fabric.rows).rev() {
        let cells: Vec<String> = (0..fabric.columns)
            .map(|col| {
                match outcome.placement.cell_at(col, row) {
                    Some(index) => {
                        let name = &packed.cells[index].name;
                        format!("{:<14}", truncate(name, 14))
                    }
                    None => match outcome
                        .routing
                        .buffers
                        .iter()
                        .find(|b| b.col == col && b.row == row)
                    {
                        Some(_) => format!("{:<14}", "(buffer)"),
                        None => format!("{:<14}", "."),
                    },
                }
            })
            .collect();
        let _ = writeln!(out, "  row {} | {}", row, cells.join(" "));
    }

    // ---- warnings ----------------------------------------------------------
    let _ = writeln!(out);
    let _ = writeln!(out, "warnings");
    let _ = writeln!(out, "--------");
    if outcome.warnings.is_empty() {
        let _ = writeln!(out, "  none");
    } else {
        for warning in &outcome.warnings {
            let _ = writeln!(out, "  {}", warning);
        }
    }

    out
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        text.to_string()
    } else {
        text.chars().take(width - 1).chain(std::iter::once('~')).collect()
    }
}

/// Short summary for stdout.
pub fn summary(outcome: &Outcome) -> String {
    let used = outcome.packed.clb_count() + outcome.routing.buffers.len();
    format!(
        "{}: fitted in {} of {} CLBs ({} logic + {} buffers), {} register(s), \
         depth {}, {} attempt(s) in {:.2}s",
        outcome.packed.top,
        used,
        outcome.fabric.clb_count(),
        outcome.packed.clb_count(),
        outcome.routing.buffers.len(),
        outcome.packed.register_count(),
        logic_depth(&outcome.packed),
        outcome.attempts,
        outcome.elapsed.as_secs_f64()
    )
}
