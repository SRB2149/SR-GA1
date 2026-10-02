//! The synthesis report.
//!
//! On a 28-CLB fabric the interesting question is almost never "did it
//! work" but "what did it cost, and what is nearly full". So the report
//! leads with a utilisation table giving every resource as an absolute figure
//! *and* as a percentage of capacity, then says where each IO pin landed and
//! how each register's enable was satisfied — that being the constraint most
//! likely to decide whether the next change still fits.
//!
//! Everything here reports what the tool *did*. It deliberately carries no
//! advice about how to write the source: a report that mixes results with
//! design suggestions makes it unclear which lines describe the bitstream in
//! hand. Caveats about a figure's meaning stay, because they describe the
//! number being printed.

use crate::backend;
use crate::designjson::LoopbackLink;
use crate::flow::Outcome;
use crate::pack::{Enable, PackedDesign, Signal};
use crate::place::Target;
use crate::rrg::Node;
use std::collections::{BTreeMap, BTreeSet};
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
            if let Signal::Net(net) = pin {
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
    let mut out = Vec::new();
    for (source, sink, net) in outcome.routing.loopbacks(&outcome.rrg) {
        let (Some(from), Some(to)) = (pad_at(outcome, source), pad_at(outcome, sink)) else {
            continue;
        };
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

/// The pad an RRG node *is*, for the two node kinds that sit on the chip edge.
fn pad_at(outcome: &Outcome, node: Node) -> Option<String> {
    match node {
        Node::HSeg { row, col, lane } if col == outcome.fabric.columns => outcome
            .fabric
            .io_outputs
            .get(row)
            .and_then(|lanes| lanes.get(lane))
            .cloned()
            .flatten(),
        Node::HSeg { row, col: 0, lane } => {
            outcome.fabric.io_inputs.get(row).and_then(|lanes| lanes.get(lane)).cloned()
        }
        _ => None,
    }
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

// ---- utilisation -----------------------------------------------------------

/// Every capacity figure the report quotes, gathered once so the summary line
/// and the table cannot disagree.
pub struct Usage {
    pub logic: usize,
    pub buffers: usize,
    pub clbs_used: usize,
    pub clbs: usize,
    pub registers: usize,
    pub carry_cells: usize,
    pub clocks: usize,
    pub csbs: usize,
    pub horz_used: usize,
    pub horz: usize,
    pub vert_used: usize,
    pub vert: usize,
    /// Horizontal segments in use, keyed by (row, lane).
    pub horz_by_lane: BTreeMap<(usize, usize), usize>,
    /// Vertical ring segments in use, keyed by (column, lane).
    pub vert_by_lane: BTreeMap<(usize, usize), usize>,
    pub in_pads_used: usize,
    pub in_pads: usize,
    pub in_constants: usize,
    pub out_pads_used: usize,
    pub out_pads: usize,
    pub loops_used: usize,
    pub loops_pool: usize,
    pub bits: usize,
}

/// Measure the finished configuration against the fabric's capacity.
pub fn usage(outcome: &Outcome) -> Usage {
    let fabric = &outcome.fabric;
    let packed = &outcome.packed;

    // Count distinct occupied segments: a node reached by two routes would be
    // congestion, which the router refuses, so a set is the honest total.
    let mut horz_nodes: BTreeSet<usize> = BTreeSet::new();
    let mut vert_nodes: BTreeSet<usize> = BTreeSet::new();
    let mut horz_by_lane: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    let mut vert_by_lane: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    for route in &outcome.routing.routes {
        for &node in &route.nodes {
            match outcome.rrg.node(node) {
                Node::HSeg { .. } => {
                    horz_nodes.insert(node);
                }
                Node::VSeg { .. } => {
                    vert_nodes.insert(node);
                }
                _ => {}
            }
        }
    }
    for &node in &horz_nodes {
        if let Node::HSeg { row, lane, .. } = outcome.rrg.node(node) {
            *horz_by_lane.entry((row, lane)).or_insert(0) += 1;
        }
    }
    for &node in &vert_nodes {
        if let Node::VSeg { col, lane, .. } = outcome.rrg.node(node) {
            *vert_by_lane.entry((col, lane)).or_insert(0) += 1;
        }
    }

    // Pads. A constant-tied input carries no design signal, so it is capacity
    // the design could never have used; count it separately rather than
    // deflating the percentage with pads that were never available.
    let mut in_pads = 0;
    let mut in_constants = 0;
    for row in 0..fabric.rows {
        for lane in 0..fabric.horz_lanes {
            if fabric.io_constant(row, lane).is_some() {
                in_constants += 1;
            } else if fabric.io_inputs.get(row).and_then(|l| l.get(lane)).is_some() {
                in_pads += 1;
            }
        }
    }
    let out_pads = fabric
        .io_outputs
        .iter()
        .flatten()
        .filter(|name| name.as_deref().is_some_and(|n| !n.is_empty()))
        .count();

    let reset = packed.reset;
    let in_pads_used = outcome
        .placement
        .input_pins
        .iter()
        .filter(|(net, _)| Some(**net) != reset)
        .map(|(_, pad)| pad.clone())
        .collect::<BTreeSet<_>>();
    let out_pads_used = outcome.placement.output_pins.values().cloned().collect::<BTreeSet<_>>();
    let wiring = loopback_wiring(outcome);
    let pool = &outcome.constraints.loopback;

    Usage {
        logic: packed.clb_count(),
        buffers: outcome.routing.buffers.len(),
        clbs_used: packed.clb_count() + outcome.routing.buffers.len(),
        clbs: fabric.clb_count(),
        registers: packed.register_count(),
        carry_cells: packed.chains.iter().map(|c| c.len()).sum(),
        clocks: packed.clocks.len(),
        csbs: fabric.columns,
        horz_used: horz_nodes.len(),
        horz: fabric.rows * fabric.horz_lanes * (fabric.columns + 1),
        vert_used: vert_nodes.len(),
        vert: fabric.columns * fabric.vert_lanes * fabric.rows,
        horz_by_lane,
        vert_by_lane,
        // A loop input pad is board-driven and excluded from pin assignment, so
        // it is a pad in use that `input_pins` does not know about.
        in_pads_used: in_pads_used
            .union(&wiring.iter().map(|l| l.to.clone()).collect())
            .count(),
        in_pads,
        in_constants,
        out_pads_used: out_pads_used
            .union(&wiring.iter().map(|l| l.from.clone()).collect())
            .count(),
        out_pads,
        loops_used: wiring.len(),
        loops_pool: pool.outputs.len().min(pool.inputs.len()),
        bits: fabric.total_bits(),
    }
}

/// A percentage of capacity, with a zero capacity written as `n/a` rather than
/// a division by zero.
pub fn share(used: usize, total: usize) -> String {
    if total == 0 {
        return "n/a".to_string();
    }
    format!("{:.1}%", 100.0 * used as f64 / total as f64)
}

/// `share`, right-aligned for the utilisation table's column.
fn pct(used: usize, total: usize) -> String {
    format!("{:>7}", share(used, total))
}

/// A section heading with an underline that always matches its width.
fn heading(out: &mut String, text: &str) {
    let _ = writeln!(out, "{}", text);
    let _ = writeln!(out, "{}", "-".repeat(text.chars().count()));
}

fn metric(out: &mut String, label: &str, used: usize, total: usize) {
    let _ = writeln!(out, "  {:<22} {:>5} {:>7} {:>8}", label, used, total, pct(used, total));
}

// ---- where a signal is produced and consumed -------------------------------

/// The CLB a chip output's value comes out of, naming which of the cell's two
/// outputs drives it — `_reg` and `_op` leave a CLB on different lanes, so the
/// distinction is what explains the pad it could reach.
fn output_producer(outcome: &Outcome, net: u32) -> String {
    let packed = &outcome.packed;
    for (index, cell) in packed.cells.iter().enumerate() {
        let placed = &outcome.placement.cells[index];
        if cell.reg.as_ref().is_some_and(|r| r.q == net) {
            return format!("CLB({}, {}) _reg", placed.col, placed.row);
        }
        if cell.op == Some(net) {
            return format!("CLB({}, {}) _op", placed.col, placed.row);
        }
    }
    // A port that is driven straight from another port: no cell in between.
    if let Some(port) = packed.inputs.iter().find(|p| p.net == net) {
        return format!("chip input {}", port.name);
    }
    "no driver".to_string()
}

/// Everything that reads a chip input. Answers "the value arrives here — and
/// then what", which is the half of pin assignment a pad name alone omits.
fn input_consumers(outcome: &Outcome, net: u32) -> String {
    let packed = &outcome.packed;
    if packed.reset == Some(net) {
        return "all registers (hardware)".to_string();
    }
    if packed.clocks.contains(&net) {
        let column = outcome.placement.clock_columns.get(&net).copied();
        return match column {
            Some(col) => format!("CSB{} (column {}'s ring)", col, col),
            None => "a CSB".to_string(),
        };
    }
    let mut places: Vec<String> = Vec::new();
    for (index, cell) in packed.cells.iter().enumerate() {
        let reads_pin = cell.pins.iter().any(|p| *p == Signal::Net(net));
        let gates = cell.reg.as_ref().is_some_and(|r| r.enable == Enable::Net(net));
        if reads_pin || gates {
            let placed = &outcome.placement.cells[index];
            let mut place = format!("CLB({}, {})", placed.col, placed.row);
            if gates && !reads_pin {
                place.push_str(" enable");
            }
            places.push(place);
        }
    }
    if places.is_empty() {
        return "nothing".to_string();
    }
    // Long fan-outs are summarised: the full list belongs in the placement map.
    if places.len() > 4 {
        let shown = places[..4].join(", ");
        return format!("{}, +{} more", shown, places.len() - 4);
    }
    places.join(", ")
}

// ---- the report ------------------------------------------------------------

pub fn render(outcome: &Outcome, target: &Target) -> String {
    let fabric = &outcome.fabric;
    let packed = &outcome.packed;
    let u = usage(outcome);
    let mut out = String::new();

    let _ = writeln!(out, "SR-GA1 synthesis report");
    let _ = writeln!(out, "=======================");
    let _ = writeln!(out);
    let _ = writeln!(out, "design            {}", packed.top);
    let _ = writeln!(out, "fabric            {} ({}x{})", fabric.name, fabric.columns, fabric.rows);
    let _ = writeln!(out, "bitstream length  {} bits", u.bits);
    let _ = writeln!(
        out,
        "search            {} attempt(s), attempt {} won, {:.2}s elapsed",
        outcome.attempts,
        outcome.winning_attempt + 1,
        outcome.elapsed.as_secs_f64()
    );
    let _ = writeln!(out, "routing           converged in {} iteration(s)", outcome.routing.iterations);

    // ---- utilisation ------------------------------------------------------
    let _ = writeln!(out);
    heading(&mut out, "utilisation");
    let _ = writeln!(out, "  {:<22} {:>5} {:>7} {:>8}", "resource", "used", "total", "usage");
    metric(&mut out, "CLBs", u.clbs_used, u.clbs);
    metric(&mut out, "  mapped logic", u.logic, u.clbs);
    metric(&mut out, "  routing buffers", u.buffers, u.clbs);
    metric(&mut out, "registers", u.registers, u.clbs);
    metric(&mut out, "carry chain cells", u.carry_cells, u.clbs);
    metric(&mut out, "clock sources (CSB)", u.clocks, u.csbs);
    metric(&mut out, "horizontal segments", u.horz_used, u.horz);
    metric(&mut out, "vertical segments", u.vert_used, u.vert);
    metric(&mut out, "input pads", u.in_pads_used, u.in_pads);
    metric(&mut out, "output pads", u.out_pads_used, u.out_pads);
    if u.loops_pool > 0 {
        metric(&mut out, "loop-around wires", u.loops_used, u.loops_pool);
    }
    let _ = writeln!(
        out,
        "  every CLB carries one flip-flop and can host one carry bit, so those \
         share the CLB total."
    );
    let _ = writeln!(
        out,
        "  {} further input pad(s) are tied to a constant by the fabric and carry no \
         design signal.",
        u.in_constants
    );

    // ---- cell types --------------------------------------------------------
    let _ = writeln!(out);
    heading(&mut out, "cell types");
    for (gate, count) in packed.gate_histogram() {
        let _ = writeln!(
            out,
            "  {:<10} {:>4} {:>8} of mapped logic",
            gate,
            count,
            pct(count, u.logic)
        );
    }
    if u.buffers > 0 {
        let _ = writeln!(
            out,
            "  {} routing buffer(s) carry no mapped cell and are counted separately",
            u.buffers
        );
    }

    // ---- IO pins -----------------------------------------------------------
    let _ = writeln!(out);
    heading(&mut out, "IO pin assignment");
    let _ = writeln!(
        out,
        "  {:<4} {:<16} {:<12} {:<14} {:<28} {}",
        "dir", "signal", "pad", "position", "produced by / consumed by", "chosen"
    );
    for port in &packed.inputs {
        if packed.reset == Some(port.net) {
            let _ = writeln!(
                out,
                "  {:<4} {:<16} {:<12} {:<14} {:<28} {}",
                "in",
                port.name,
                "(reset pin)",
                "dedicated",
                input_consumers(outcome, port.net),
                "hardware"
            );
            continue;
        }
        match outcome.placement.input_pins.get(&port.net) {
            Some(pad) => {
                let position = target
                    .input_pad(pad)
                    .map(|(r, l)| format!("row {} lane {}", r, l))
                    .unwrap_or_else(|| "?".to_string());
                let _ = writeln!(
                    out,
                    "  {:<4} {:<16} {:<12} {:<14} {:<28} {}",
                    "in",
                    port.name,
                    pad,
                    position,
                    input_consumers(outcome, port.net),
                    pad_source(outcome, &port.name)
                );
            }
            None => {
                let _ = writeln!(
                    out,
                    "  {:<4} {:<16} {:<12} {:<14} {:<28} {}",
                    "in", port.name, "(none)", "unassigned", "nothing reads it", "-"
                );
            }
        }
    }
    for port in &packed.outputs {
        match outcome.placement.output_pins.get(&port.net) {
            Some(pad) => {
                let position = target
                    .output_pad(pad)
                    .map(|(r, l)| format!("row {} lane {}", r, l))
                    .unwrap_or_else(|| "?".to_string());
                let _ = writeln!(
                    out,
                    "  {:<4} {:<16} {:<12} {:<14} {:<28} {}",
                    "out",
                    port.name,
                    pad,
                    position,
                    output_producer(outcome, port.net),
                    pad_source(outcome, &port.name)
                );
            }
            None => {
                let _ = writeln!(
                    out,
                    "  {:<4} {:<16} {:<12} {:<14} {:<28} {}",
                    "out", port.name, "(none)", "unassigned", output_producer(outcome, port.net), "-"
                );
            }
        }
    }

    // What is left, so the cost of the next pin is visible.
    let wiring = loopback_wiring(outcome);
    let claimed_in: BTreeSet<&str> = outcome
        .placement
        .input_pins
        .values()
        .map(|s| s.as_str())
        .chain(wiring.iter().map(|l| l.to.as_str()))
        .collect();
    let claimed_out: BTreeSet<&str> = outcome
        .placement
        .output_pins
        .values()
        .map(|s| s.as_str())
        .chain(wiring.iter().map(|l| l.from.as_str()))
        .collect();
    let mut free_in: Vec<String> = Vec::new();
    for row in 0..fabric.rows {
        for lane in 0..fabric.horz_lanes {
            if fabric.io_constant(row, lane).is_some() {
                continue;
            }
            if let Some(name) = fabric.io_inputs.get(row).and_then(|l| l.get(lane)) {
                if !claimed_in.contains(name.as_str()) {
                    free_in.push(format!("{} (row {} lane {})", name, row, lane));
                }
            }
        }
    }
    let mut free_out: Vec<String> = Vec::new();
    for row in 0..fabric.rows {
        for lane in 0..fabric.horz_lanes {
            let Some(Some(name)) = fabric.io_outputs.get(row).map(|l| l.get(lane).cloned().flatten())
            else {
                continue;
            };
            if name.is_empty() || claimed_out.contains(name.as_str()) {
                continue;
            }
            free_out.push(format!("{} (row {} lane {})", name, row, lane));
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  unassigned input pads  ({:>2}): {}",
        free_in.len(),
        wrap(&free_in, 31)
    );
    let _ = writeln!(
        out,
        "  unassigned output pads ({:>2}): {}",
        free_out.len(),
        wrap(&free_out, 31)
    );

    // Pads a route left a signal on without a design output asking for it. The
    // pad still carries that value in silicon, so it is part of where the
    // design ended up.
    let mut incidental: Vec<String> = Vec::new();
    for route in &outcome.routing.routes {
        for &node in &route.nodes {
            if let Node::HSeg { col, .. } = outcome.rrg.node(node) {
                if col != fabric.columns {
                    continue;
                }
                if let Some(name) = pad_at(outcome, outcome.rrg.node(node)) {
                    if !claimed_out.contains(name.as_str()) && !incidental.contains(&name) {
                        incidental.push(name);
                    }
                }
            }
        }
    }
    if !incidental.is_empty() {
        incidental.sort();
        let _ = writeln!(
            out,
            "  output pads carrying a routed signal that is not a design output: {}",
            incidental.join(", ")
        );
    }

    // ---- loop-around wiring ------------------------------------------------
    let _ = writeln!(out);
    heading(&mut out, "loop-around wiring");
    let pool = &outcome.constraints.loopback;
    if pool.is_empty() {
        let _ = writeln!(out, "  no pool declared, so none was available to use");
    } else {
        let _ = writeln!(
            out,
            "  pool: {} output pad(s), {} input pad(s), giving at most {} wire(s)",
            pool.outputs.len(),
            pool.inputs.len(),
            u.loops_pool
        );
        if wiring.is_empty() {
            let _ = writeln!(out, "  none used: this design routes entirely inside the fabric");
        } else {
            let _ = writeln!(out, "  THIS DESIGN REQUIRES THE FOLLOWING PHYSICAL CONNECTIONS:");
            for link in &wiring {
                let from = target
                    .output_pad(&link.from)
                    .map(|(r, l)| format!("row {} lane {}", r, l))
                    .unwrap_or_else(|| "?".to_string());
                let to = target
                    .input_pad(&link.to)
                    .map(|(r, l)| format!("row {} lane {}", r, l))
                    .unwrap_or_else(|| "?".to_string());
                let _ = writeln!(
                    out,
                    "    wire {:<12} ({:<12}) -> {:<12} ({:<12}) carrying {}",
                    link.from, from, link.to, to, link.net
                );
            }
            let spare_out: Vec<String> = pool
                .outputs
                .iter()
                .map(|p| p.name.clone())
                .filter(|name| !wiring.iter().any(|l| l.from == *name))
                .collect();
            let spare_in: Vec<String> = pool
                .inputs
                .iter()
                .map(|p| p.name.clone())
                .filter(|name| !wiring.iter().any(|l| l.to == *name))
                .collect();
            if !spare_out.is_empty() || !spare_in.is_empty() {
                let _ = writeln!(
                    out,
                    "  unused pool pads: {} output(s) [{}], {} input(s) [{}]",
                    spare_out.len(),
                    spare_out.join(", "),
                    spare_in.len(),
                    spare_in.join(", ")
                );
            } else {
                let _ = writeln!(out, "  the whole pool is in use");
            }
            let _ = writeln!(
                out,
                "  these wires must be present on the board for the configuration to work"
            );
        }
    }

    // ---- registers and enables --------------------------------------------
    let _ = writeln!(out);
    heading(&mut out, "registers");
    let _ = writeln!(
        out,
        "  {} of {} CLBs hold a register ({})",
        u.registers,
        u.clbs,
        pct(u.registers, u.clbs).trim()
    );
    let enable_lane = backend::enable_lane(fabric);
    if let Some(lane) = enable_lane.filter(|_| u.registers > 0) {
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
                "  {:<16} CLB({}, {})  reset={}  clock={}  {}",
                cell.name,
                placed.col,
                placed.row,
                u8::from(reg.reset_value),
                packed.names.get(reg.clock).unwrap_or("<unnamed>"),
                how
            );
        }
    }

    // ---- carry chains ------------------------------------------------------
    let _ = writeln!(out);
    heading(&mut out, "carry chains");
    if packed.chains.is_empty() {
        let _ = writeln!(out, "  none used");
    } else {
        let _ = writeln!(
            out,
            "  {} chain(s) holding {} of {} CLBs ({})",
            packed.chains.len(),
            u.carry_cells,
            u.clbs,
            pct(u.carry_cells, u.clbs).trim()
        );
        for (index, chain) in packed.chains.iter().enumerate() {
            let column = chain.first().map(|&c| outcome.placement.cells[c].col);
            let _ = writeln!(
                out,
                "  chain {}: {} of {} rows in column {} ({}), least significant at the bottom",
                index,
                chain.len(),
                fabric.rows,
                column.map(|c| c.to_string()).unwrap_or_else(|| "?".into()),
                pct(chain.len(), fabric.rows).trim()
            );
            for &cell in chain {
                let placed = &outcome.placement.cells[cell];
                let _ = writeln!(out, "    row {} = {}", placed.row, packed.cells[cell].name);
            }
            if chain.len() == fabric.rows {
                let _ = writeln!(out, "    column full; the top cell's carry-out is unreachable");
            }
        }
    }

    // ---- clocks ------------------------------------------------------------
    let _ = writeln!(out);
    heading(&mut out, "clock domains");
    if packed.clocks.is_empty() {
        let _ = writeln!(out, "  none: the design is purely combinational, every CSB left at default");
    } else {
        let _ = writeln!(
            out,
            "  {} of {} CSBs source a clock ({})",
            u.clocks,
            u.csbs,
            pct(u.clocks, u.csbs).trim()
        );
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
                .filter(|c| c.reg.as_ref().is_some_and(|r| r.clock == *clock))
                .count();
            let _ = writeln!(
                out,
                "    {:<16} {} of {} columns [{}], {} register(s)",
                packed.names.get(*clock).unwrap_or("<unnamed>"),
                columns.len(),
                fabric.columns,
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
                "    no clock reaches columns [{}]; nothing sequential is placed there",
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

    // ---- lane occupancy ----------------------------------------------------
    let segments = fabric.columns + 1;
    let _ = writeln!(out);
    heading(
        &mut out,
        &format!(
            "horizontal lane segments in use: {} of {} ({})",
            u.horz_used,
            u.horz,
            pct(u.horz_used, u.horz).trim()
        ),
    );
    let _ = writeln!(
        out,
        "         {}",
        (0..fabric.horz_lanes)
            .map(|lane| format!("lane {:<5}", lane))
            .collect::<Vec<_>>()
            .join("")
            .trim_end()
    );
    for row in 0..fabric.rows {
        let counts: Vec<String> = (0..fabric.horz_lanes)
            .map(|lane| {
                let used = u.horz_by_lane.get(&(row, lane)).copied().unwrap_or(0);
                let share = format!("{:.0}%", 100.0 * used as f64 / segments as f64);
                format!("{:>2}/{} {:>4}", used, segments, share)
            })
            .collect();
        let _ = writeln!(out, "  row {}  {}", row, counts.join(" "));
    }
    let _ = writeln!(out);
    heading(
        &mut out,
        &format!(
            "vertical ring segments in use: {} of {} ({})",
            u.vert_used,
            u.vert,
            pct(u.vert_used, u.vert).trim()
        ),
    );
    let _ = writeln!(
        out,
        "         {}",
        (0..fabric.vert_lanes)
            .map(|lane| format!("lane {:<5}", lane))
            .collect::<Vec<_>>()
            .join("")
            .trim_end()
    );
    for col in 0..fabric.columns {
        let counts: Vec<String> = (0..fabric.vert_lanes)
            .map(|lane| {
                let used = u.vert_by_lane.get(&(col, lane)).copied().unwrap_or(0);
                let share = format!("{:.0}%", 100.0 * used as f64 / fabric.rows as f64);
                format!("{:>2}/{} {:>4}", used, fabric.rows, share)
            })
            .collect();
        let _ = writeln!(out, "  col {}  {}", col, counts.join(" "));
    }

    // ---- depth -------------------------------------------------------------
    let _ = writeln!(out);
    heading(&mut out, "logic depth");
    let _ = writeln!(
        out,
        "  {} operation core(s) on the longest combinational path, counted \
         structurally: there is no timing model",
        logic_depth(packed)
    );

    // ---- DDIO --------------------------------------------------------------
    let _ = writeln!(out);
    heading(&mut out, "DDIO");
    if fabric.ddio.is_empty() {
        let _ = writeln!(out, "  this fabric has no dual-direction pads");
    } else {
        let _ = writeln!(
            out,
            "  {} of {} dual-direction pad(s) bound ({})",
            outcome.constraints.ddio.len(),
            fabric.ddio.len(),
            pct(outcome.constraints.ddio.len(), fabric.ddio.len()).trim()
        );
        for (index, pad) in fabric.ddio.iter().enumerate() {
            match outcome.constraints.ddio.iter().find(|b| b.pin == index) {
                Some(b) => {
                    let _ = writeln!(
                        out,
                        "  pad {} in={} out={} dir={}  <- {} / {} / {}",
                        index, pad.input, pad.output, pad.dir, b.input, b.output, b.dir
                    );
                }
                None => {
                    let _ = writeln!(
                        out,
                        "  pad {} in={} out={} dir={}  unbound; {} is forced 0 while {} is 1",
                        index, pad.input, pad.output, pad.dir, pad.input, pad.dir
                    );
                }
            }
        }
    }

    // ---- constraints -------------------------------------------------------
    let _ = writeln!(out);
    heading(&mut out, "constraints");
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
    heading(
        &mut out,
        &format!(
            "placement: {} of {} CLBs ({})",
            u.clbs_used,
            u.clbs,
            pct(u.clbs_used, u.clbs).trim()
        ),
    );
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
    heading(&mut out, "warnings");
    if outcome.warnings.is_empty() {
        let _ = writeln!(out, "  none");
    } else {
        for warning in &outcome.warnings {
            let _ = writeln!(out, "  {}", warning);
        }
    }

    out
}

/// Comma-separate a list, wrapping onto continuation lines indented by `indent`
/// so a long pad inventory stays readable in a terminal.
fn wrap(items: &[String], indent: usize) -> String {
    if items.is_empty() {
        return "none".to_string();
    }
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for item in items {
        let piece = if line.is_empty() { item.clone() } else { format!(", {}", item) };
        if !line.is_empty() && line.chars().count() + piece.chars().count() + indent > 96 {
            lines.push(line);
            line = item.clone();
        } else {
            line.push_str(&piece);
        }
    }
    lines.push(line);
    lines.join(&format!("\n{}", " ".repeat(indent)))
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
    let u = usage(outcome);
    format!(
        "{}: fitted in {} of {} CLBs ({}) — {} logic + {} buffers, {} register(s), \
         {} input pad(s) of {}, {} output pad(s) of {}, depth {}, {} attempt(s) in {:.2}s",
        outcome.packed.top,
        u.clbs_used,
        u.clbs,
        pct(u.clbs_used, u.clbs).trim(),
        u.logic,
        u.buffers,
        u.registers,
        u.in_pads_used,
        u.in_pads,
        u.out_pads_used,
        u.out_pads,
        logic_depth(&outcome.packed),
        outcome.attempts,
        outcome.elapsed.as_secs_f64()
    )
}
