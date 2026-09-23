//! From a placement to a configuration.
//!
//! Two halves. First, turning a placement into a routing problem: who drives
//! what, which input pin each operand has to reach, which segments must carry
//! a constant, and where each clock has to land on its column's ring. Second,
//! turning the finished routing back into configuration fields — which is
//! nearly mechanical, because every graph edge already names the write it
//! implies.

use crate::design::{BlockId, Design};
use crate::fabric::{Fabric, Source};
use crate::pack::{Enable, PackedDesign, Signal};
use crate::place::{Operand, Placement, Target};
use crate::route::{Net, NetKind, Problem, Routing, Sink};
use crate::rrg::{Node, Rrg};
use std::collections::BTreeMap;
use std::fmt;

/// Reserved to nothing: no net may use the node.
const RESERVED_UNUSABLE: usize = usize::MAX;

#[derive(Debug, Clone)]
pub struct BackendError {
    pub message: String,
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for BackendError {}

/// The routing problem plus the bookkeeping needed to read its result.
pub struct Wiring {
    pub problem: Problem,
    /// Net index for each design net that had to be routed.
    pub signal_nets: BTreeMap<u32, usize>,
    /// Net index of each constant.
    pub const_nets: [Option<usize>; 2],
    /// Net index of each clock.
    pub clock_nets: BTreeMap<u32, usize>,
}

/// Build the routing problem implied by a placement.
pub fn build_wiring(
    design: &PackedDesign,
    placement: &Placement,
    target: &Target,
    rrg: &Rrg,
) -> Wiring {
    let fabric = target.fabric;
    let library = target.library;
    let mut nets: Vec<Net> = Vec::new();
    let mut signal_nets: BTreeMap<u32, usize> = BTreeMap::new();
    let mut clock_nets: BTreeMap<u32, usize> = BTreeMap::new();
    let mut reserved: Vec<Option<usize>> = vec![None; rrg.len()];
    let mut clb_used = vec![false; fabric.clb_count()];

    let id = |node: Node| rrg.id(node).expect("every fabric node is in the graph");

    // ---- one net per driven design signal ---------------------------------
    // Drivers first, so sinks can be attached to an existing net.
    let mut driver_node: BTreeMap<u32, Node> = BTreeMap::new();
    for (index, cell) in design.cells.iter().enumerate() {
        let placed = &placement.cells[index];
        clb_used[placed.row * fabric.columns + placed.col] = true;
        if let Some(net) = cell.op {
            driver_node.insert(net, Node::Op { col: placed.col, row: placed.row });
        }
        if let Some(reg) = &cell.reg {
            driver_node.insert(reg.q, Node::Reg { col: placed.col, row: placed.row });
        }
    }
    for (net, pad) in &placement.input_pins {
        if let Some((row, lane)) = target.input_pad(pad) {
            driver_node.insert(*net, Node::HSeg { row, col: 0, lane });
        }
    }

    // Clocks are routed to a ring tap rather than to a cell input.
    for clock in &design.clocks {
        let Some(&root) = driver_node.get(clock) else { continue };
        let column = placement.clock_columns.get(clock).copied().unwrap_or(0);
        let options: Vec<usize> =
            Rrg::clock_taps(fabric, column).into_iter().map(|(_, node)| id(node)).collect();
        clock_nets.insert(*clock, nets.len());
        nets.push(Net {
            name: design.names.get(*clock).unwrap_or("clock").to_string(),
            kind: NetKind::Clock,
            roots: vec![id(root)],
            sinks: vec![Sink {
                what: format!("column {} clock ring tap", column),
                options,
            }],
        });
    }

    for (net, root) in &driver_node {
        if clock_nets.contains_key(net) {
            continue;
        }
        signal_nets.insert(*net, nets.len());
        nets.push(Net {
            name: design.names.get(*net).unwrap_or("net").to_string(),
            kind: NetKind::Signal,
            roots: vec![id(*root)],
            sinks: Vec::new(),
        });
    }

    // ---- constants --------------------------------------------------------
    // One net per value, rooted at the constant source and at every segment
    // the IO map already supplies it on. That is what makes row 3's lane-3
    // enables free, and what lets `b` and `c` share one lane.
    let mut const_nets = [None, None];
    for value in [false, true] {
        let mut roots = vec![id(Node::Const(value))];
        for row in 0..fabric.rows {
            for lane in 0..fabric.horz_lanes {
                if fabric.io_constant(row, lane) == Some(value) {
                    roots.push(id(Node::HSeg { row, col: 0, lane }));
                }
            }
        }
        const_nets[usize::from(value)] = Some(nets.len());
        nets.push(Net {
            name: if value {
                fabric.naming.constant_one.clone()
            } else {
                fabric.naming.constant_zero.clone()
            },
            kind: NetKind::Constant(value),
            roots,
            sinks: Vec::new(),
        });
    }

    // ---- sinks ------------------------------------------------------------
    for (index, cell) in design.cells.iter().enumerate() {
        let placed = &placement.cells[index];
        let (col, row) = (placed.col, placed.row);
        let operands = placement.operands(design, target, index);

        for (mux, operand) in operands.iter().enumerate() {
            let pin = Node::In { col, row, mux };
            match operand {
                Operand::Signal(Signal::Net(net)) => {
                    if let Some(&net_id) = signal_nets.get(net) {
                        nets[net_id].sinks.push(Sink {
                            what: format!(
                                "CLB({}, {}) input {} of {}",
                                col, row, library.phys_names[mux], cell.name
                            ),
                            options: vec![id(pin)],
                        });
                        reserved[id(pin)] = Some(net_id);
                    } else {
                        reserved[id(pin)] = Some(RESERVED_UNUSABLE);
                    }
                }
                Operand::Signal(Signal::Const(v)) | Operand::Const(v) => {
                    let net_id = const_nets[usize::from(*v)].expect("constant nets exist");
                    nets[net_id].sinks.push(Sink {
                        what: format!(
                            "CLB({}, {}) input {} of {} tied to {}",
                            col, row, library.phys_names[mux], cell.name, u8::from(*v)
                        ),
                        options: vec![id(pin)],
                    });
                    reserved[id(pin)] = Some(net_id);
                }
                // The carry chain is hardware: nothing is routed to it, and
                // nothing else may use the input that reads it.
                Operand::CarryIn | Operand::DontCare => {
                    reserved[id(pin)] = Some(RESERVED_UNUSABLE)
                }
            }
        }

        // The cell owns its own outputs, so nothing may buffer through it.
        let op_owner = cell.op.and_then(|n| signal_nets.get(&n).copied());
        reserved[id(Node::Op { col, row })] = Some(op_owner.unwrap_or(RESERVED_UNUSABLE));
        let reg_owner = cell.reg.as_ref().and_then(|r| {
            signal_nets.get(&r.q).copied().or_else(|| clock_nets.get(&r.q).copied())
        });
        reserved[id(Node::Reg { col, row })] = Some(reg_owner.unwrap_or(RESERVED_UNUSABLE));

        // The register enable, which is an ordinary segment that happens to
        // gate the flip-flop.
        if let Some(reg) = &cell.reg {
            if let Some(enable_node) = Rrg::enable_node(fabric, col, row) {
                match &reg.enable {
                    Enable::Always => {
                        let net_id = const_nets[1].expect("constant one net exists");
                        nets[net_id].sinks.push(Sink {
                            what: format!(
                                "CLB({}, {}) register enable of {} held at 1",
                                col, row, cell.name
                            ),
                            options: vec![id(enable_node)],
                        });
                    }
                    Enable::Net(net) => {
                        if let Some(&net_id) = signal_nets.get(net) {
                            nets[net_id].sinks.push(Sink {
                                what: format!(
                                    "CLB({}, {}) register enable of {}",
                                    col, row, cell.name
                                ),
                                options: vec![id(enable_node)],
                            });
                        }
                    }
                }
            }
        }
    }

    // ---- design outputs ---------------------------------------------------
    for (net, pad) in &placement.output_pins {
        let Some((row, lane)) = target.output_pad(pad) else { continue };
        let Some(&net_id) = signal_nets.get(net) else { continue };
        nets[net_id].sinks.push(Sink {
            what: format!("chip output {}", pad),
            options: vec![id(Node::HSeg { row, col: fabric.columns, lane })],
        });
    }

    Wiring {
        problem: Problem { nets, reserved, clb_used, const_net: const_nets },
        signal_nets,
        const_nets,
        clock_nets,
    }
}

/// Turn a completed routing into a configuration.
pub fn emit(
    fabric: &Fabric,
    design: &PackedDesign,
    placement: &Placement,
    target: &Target,
    rrg: &Rrg,
    wiring: &Wiring,
    routing: &Routing,
) -> Result<Design, BackendError> {
    let library = target.library;
    let mut config = Design::new(fabric);

    // Block names have to be unique, and so do the net names derived from them:
    // the visual programmer rejects a duplicate, and rejects a name whose
    // `_op`/`_reg`/`_carry` form would collide with an existing one. Several
    // cells legitimately want the same name — three buffers can all be carrying
    // `count[3]` — so they are allocated rather than assigned.
    let mut names = crate::naming::NameAllocator::new();
    for reserved in [&fabric.naming.constant_zero, &fabric.naming.constant_one] {
        names.reserve(reserved);
    }
    for pad in fabric.io_inputs.iter().flatten() {
        names.reserve(pad);
    }
    for pad in fabric.io_outputs.iter().flatten().flatten() {
        names.reserve(pad);
    }
    let allocate = |allocator: &mut crate::naming::NameAllocator, preferred: &str| -> String {
        let name = allocator.unique(preferred);
        // Reserve the nets this block will imply, so a later block cannot take
        // a name that collides with one of them.
        for suffix in [
            &fabric.naming.suffix_op,
            &fabric.naming.suffix_reg,
            &fabric.naming.suffix_carry,
        ] {
            allocator.reserve(&format!("{}{}", name, suffix));
        }
        name
    };

    // ---- placed cells: operation and reset value --------------------------
    for (index, cell) in design.cells.iter().enumerate() {
        let placed = &placement.cells[index];

        // An adder bit's operation and carry select come from the carry plan,
        // not from a library implementation: its third input is the chain.
        let op_code = match (&cell.carry, target.carry) {
            (Some(link), Some(plan)) => {
                // Only the least significant cell may take its carry from a
                // lane, and only when the tied-off edge value is wrong.
                let from_chain = link.index > 0 || link.carry_in.is_none();
                if from_chain {
                    let mux = &fabric.input_muxes[plan.carry_input];
                    config
                        .apply_slice(fabric, placed.col, placed.row, mux.select, plan.carry_code)
                        .map_err(|had| BackendError {
                            message: format!(
                                "CLB({}, {}) must read the carry chain on input {} but that mux                                  is already set to code {}",
                                placed.col, placed.row, plan.carry_input, had
                            ),
                        })?;
                }
                plan.op_code
            }
            _ => {
                let Some(gate) = library.cell(&cell.gate) else {
                    return Err(BackendError {
                        message: format!(
                            "cell {} has gate {} which is not in the library",
                            cell.name, cell.gate
                        ),
                    });
                };
                gate.impls[placed.imp.min(gate.impls.len() - 1)].op_code
            }
        };
        config
            .apply_slice(fabric, placed.col, placed.row, fabric.op_select, op_code)
            .map_err(|had| BackendError {
                message: format!(
                    "CLB({}, {}) is asked to be both operation {} and {}",
                    placed.col, placed.row, had, op_code
                ),
            })?;
        if let Some(reg) = &cell.reg {
            let field = &fabric.clb_fields[fabric.ff.reset_value_field];
            config
                .set_clb(fabric, placed.col, placed.row, &field.name, u64::from(reg.reset_value))
                .map_err(|e| BackendError { message: e.message })?;
        }
        let name = allocate(&mut names, &cell.name);
        config.name(BlockId::Clb { col: placed.col, row: placed.row }, name);
    }

    // ---- buffers the router allocated -------------------------------------
    for buffer in &routing.buffers {
        config
            .apply_slice(fabric, buffer.col, buffer.row, fabric.op_select, buffer.op_code)
            .map_err(|had| BackendError {
                message: format!(
                    "CLB({}, {}) is used as a buffer needing operation {} but already holds {}",
                    buffer.col, buffer.row, buffer.op_code, had
                ),
            })?;
        let preferred = format!("buf_{}", wiring.problem.nets[buffer.net].name);
        let name = allocate(&mut names, &preferred);
        config.name(BlockId::Clb { col: buffer.col, row: buffer.row }, name);
    }

    // ---- every routed edge names its own configuration write --------------
    for (net_id, route) in routing.routes.iter().enumerate() {
        for (_, edge) in &route.edges {
            let Some(assign) = edge.assign else { continue };
            config
                .apply_slice(fabric, assign.col, assign.row, assign.slice, assign.value)
                .map_err(|had| BackendError {
                    message: format!(
                        "net \"{}\" needs CLB({}, {}) mux code {} but {} is already using code {}",
                        wiring.problem.nets[net_id].name,
                        assign.col,
                        assign.row,
                        assign.value,
                        "another net",
                        had
                    ),
                })?;
        }
    }

    // ---- clocks: which ring lane each CSB taps ---------------------------
    // A CSB is named after its clock net, allocated from the same pool.
    let mut clock_names: BTreeMap<u32, String> = BTreeMap::new();
    for clock in &design.clocks {
        let preferred = design.names.get(*clock).unwrap_or("clock").to_string();
        clock_names.insert(*clock, allocate(&mut names, &preferred));
    }
    configure_clocks(fabric, design, placement, rrg, wiring, routing, &mut config, &clock_names)?;

    Ok(config)
}

/// Work out each column's clock source and write the CSB fields.
///
/// A CSB either taps its own column's ring, or couples to the previous
/// column. One uncoupled CSB per clock domain sources it, and the columns
/// after it couple along the chain until the next domain starts. A fully
/// coupled chain has no source at all, so at least one CSB must select a lane.
fn configure_clocks(
    fabric: &Fabric,
    design: &PackedDesign,
    placement: &Placement,
    rrg: &Rrg,
    wiring: &Wiring,
    routing: &Routing,
    config: &mut Design,
    names: &BTreeMap<u32, String>,
) -> Result<(), BackendError> {
    let couple_field = fabric.csb_fields[fabric.csb_clock.couple_field].name.clone();
    let addr_field = fabric.csb_fields[fabric.csb_clock.select.field].name.clone();

    if design.clocks.is_empty() {
        // Nothing is clocked. Leave every CSB at its default rather than
        // building a coupled loop with no source, which is a DRC error.
        return Ok(());
    }

    // Which column sources each clock, and on which ring lane the routing
    // actually landed.
    let mut sources: Vec<(usize, u64, u32)> = Vec::new();
    for clock in &design.clocks {
        let column = placement.clock_columns.get(clock).copied().unwrap_or(0);
        let net_id = match wiring.clock_nets.get(clock) {
            Some(&id) => id,
            None => continue,
        };
        let route = &routing.routes[net_id];
        let taps = Rrg::clock_taps(fabric, column);
        let landed = taps.iter().find(|(_, node)| {
            rrg.id(*node).is_some_and(|id| route.nodes.contains(&id))
        });
        let Some(&(code, _)) = landed else {
            return Err(BackendError {
                message: format!(
                    "clock \"{}\" was not routed onto column {}'s vertical ring, so no CSB can \
                     tap it. Every fabric clock must reach a ring lane at the loop point.",
                    design.names.get(*clock).unwrap_or("<unnamed>"),
                    column
                ),
            });
        };
        sources.push((column, code, *clock));
    }
    if sources.is_empty() {
        return Err(BackendError {
            message: "the design is clocked but no clock reached a vertical ring".to_string(),
        });
    }
    sources.sort_by_key(|&(column, _, _)| column);

    // Each sourcing column selects its lane; every other column couples to
    // the column before it, so it inherits the nearest source to its left.
    for col in 0..fabric.columns {
        match sources.iter().find(|&&(column, _, _)| column == col) {
            Some(&(_, code, _)) => {
                config
                    .set_csb(fabric, col, &addr_field, code)
                    .map_err(|e| BackendError { message: e.message })?;
                config
                    .set_csb(fabric, col, &couple_field, 0)
                    .map_err(|e| BackendError { message: e.message })?;
            }
            None => {
                config
                    .set_csb(fabric, col, &couple_field, 1)
                    .map_err(|e| BackendError { message: e.message })?;
            }
        }
        // Only the sourcing CSB is named. A coupled one carries the same clock,
        // and the visual programmer resolves that by tracing the coupling chain
        // - naming them all would just be the same name several times over,
        // which it rejects as a collision.
        let name = match sources.iter().find(|&&(column, _, _)| column == col) {
            Some(&(_, _, clock)) => match names.get(&clock) {
                Some(name) => name.clone(),
                None => continue,
            },
            None => continue,
        };
        config.name(BlockId::Csb { col }, name);
    }

    Ok(())
}

/// Column-to-clock-domain map, for the report.
pub fn clock_domains(
    fabric: &Fabric,
    _design: &PackedDesign,
    placement: &Placement,
) -> BTreeMap<usize, Option<u32>> {
    placement
        .clock_domains(fabric.columns)
        .into_iter()
        .enumerate()
        .collect()
}

/// Whether the fabric's enable source is a horizontal lane, and which — used
/// by the report to explain how enables were satisfied.
pub fn enable_lane(fabric: &Fabric) -> Option<usize> {
    match fabric.ff.enable {
        Source::HorzIn(lane) => Some(lane),
        _ => None,
    }
}
