//! Design rule checks.
//!
//! Multiple drivers are impossible by construction in this fabric (every
//! outgoing segment is owned by exactly one mux), so there is no check for
//! them. What is checked:
//! - CSB chains that couple in a full circle with no real source (error);
//! - potential combinational loops in the value-dependency graph — through
//!   the DDIO direction feedback, or the vertical ring now that input mux c
//!   can read the combinational lane 0 (error);
//! - columns whose CSB taps a floating pass-through ring — no clock (warning);
//! - vertical rings that are pure pass-through loops with no driver (warning);
//! - used logic fed by a floating loop (warning);
//! - columns with live registers running on a fabric-derived clock — a ripple
//!   clock on real silicon whose skew the idealised simulator will not
//!   reproduce (info, naming the signal);
//! - blocks that neither originate nor repeat any live signal (info).

use crate::config::{BlockId, Design};
use crate::fabric::{BusOut, Fabric, Source};
use crate::naming::{resolve, Namer, NetOrigin};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone)]
pub struct DrcItem {
    pub severity: Severity,
    /// Clicking the item selects this block in the GUI.
    pub block: Option<BlockId>,
    pub message: String,
}

/// Segment id used by the liveness and loop analyses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Seg {
    Horz { row: usize, lane: usize, pos: usize },
    Vert { col: usize, lane: usize, pos: usize },
}

pub fn check(fabric: &Fabric, design: &Design) -> Vec<DrcItem> {
    let mut items = Vec::new();
    let nets = resolve(fabric, design);
    let namer = Namer::new(fabric, design);
    let live = liveness(fabric, design);

    // Clocks.
    for (col, clock) in nets.clocks.iter().enumerate() {
        match clock.origin {
            None => items.push(DrcItem {
                severity: Severity::Error,
                block: Some(BlockId::Csb { col }),
                message: format!(
                    "{}: the CSB couple chain selects in a full circle — no clock source exists for this column",
                    namer.block_name(BlockId::Csb { col })
                ),
            }),
            Some(NetOrigin::FloatingLoop) => items.push(DrcItem {
                severity: Severity::Warning,
                block: Some(BlockId::Csb { col }),
                message: format!(
                    "{}: selects a floating pass-through ring — the column has no usable clock",
                    namer.block_name(BlockId::Csb { col })
                ),
            }),
            Some(origin) => {
                let column_has_live_reg = (0..fabric.rows)
                    .any(|row| live.need_reg[row * fabric.columns + col]);
                if column_has_live_reg {
                    items.push(DrcItem {
                        severity: Severity::Info,
                        block: Some(BlockId::Csb { col }),
                        message: format!(
                            "column {} runs on a clock derived from fabric signal \"{}\" — a ripple clock on real \
                             hardware; the simulator's idealised timing will not reproduce its skew",
                            col,
                            namer.net_name(origin)
                        ),
                    });
                }
            }
        }
    }

    // Floating vertical rings, one item per affected column.
    for col in 0..fabric.columns {
        let lanes: Vec<usize> = (0..fabric.vert_lanes)
            .filter(|&lane| nets.ring_tap(col, lane) == NetOrigin::FloatingLoop)
            .collect();
        if !lanes.is_empty() {
            items.push(DrcItem {
                severity: Severity::Warning,
                block: Some(BlockId::Csb { col }),
                message: format!(
                    "column {}: vertical ring lane(s) {} form a floating pass-through loop with no driver",
                    col,
                    lanes.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(", ")
                ),
            });
        }
    }

    // Used logic fed by a floating loop.
    for row in 0..fabric.rows {
        for col in 0..fabric.columns {
            let ci = row * fabric.columns + col;
            if !(live.need_op[ci] || live.need_carry[ci]) {
                continue;
            }
            for m in &fabric.input_muxes {
                let sel = design.clb(col, row).slice(&m.select) as usize;
                let floating = match m.sources[sel] {
                    Source::HorzIn(k) => nets.horz_in(col, row, k) == NetOrigin::FloatingLoop,
                    Source::VertIn(k) => nets.vert_in(col, row, k) == NetOrigin::FloatingLoop,
                    _ => false,
                };
                if floating {
                    items.push(DrcItem {
                        severity: Severity::Warning,
                        block: Some(BlockId::Clb { col, row }),
                        message: format!(
                            "{}: input \"{}\" is fed by a floating loop and will read constant 0 in simulation",
                            namer.block_name(BlockId::Clb { col, row }),
                            m.name
                        ),
                    });
                }
            }
        }
    }

    // Potential combinational loops (through logic or the DDIO gate).
    for blocks in combinational_loops(fabric, design) {
        let names: Vec<String> = blocks.iter().map(|&b| namer.block_name(b)).collect();
        items.push(DrcItem {
            severity: Severity::Error,
            block: blocks.first().copied(),
            message: format!(
                "potential combinational loop through: {} — simulation will refuse to settle if it oscillates",
                names.join(", ")
            ),
        });
    }

    // Unused blocks. An all-default CLB is "unconfigured" (untouched); a
    // configured CLB that neither originates nor repeats anything live is
    // dead logic.
    for row in 0..fabric.rows {
        for col in 0..fabric.columns {
            let ci = row * fabric.columns + col;
            let all_default = (0..fabric.clb_fields.len()).all(|i| design.clb(col, row).get(i) == 0);
            if all_default {
                items.push(DrcItem {
                    severity: Severity::Info,
                    block: Some(BlockId::Clb { col, row }),
                    message: format!(
                        "{}: unused — every config bit at its default",
                        namer.block_name(BlockId::Clb { col, row })
                    ),
                });
                continue;
            }
            let originates = live.need_op[ci] || live.need_carry[ci] || live.need_reg[ci];
            let repeats = (0..fabric.horz_lanes)
                .any(|lane| live.is_live(Seg::Horz { row, lane, pos: col + 1 }, fabric))
                || (0..fabric.vert_lanes)
                    .any(|lane| live.is_live(Seg::Vert { col, lane, pos: (row + 1) % fabric.rows }, fabric));
            if !originates && !repeats {
                items.push(DrcItem {
                    severity: Severity::Info,
                    block: Some(BlockId::Clb { col, row }),
                    message: format!(
                        "{}: configured but contributes to no chip output or clock",
                        namer.block_name(BlockId::Clb { col, row })
                    ),
                });
            }
        }
    }

    items.sort_by_key(|i| i.severity);
    items
}

// ---------------------------------------------------------------------------
// Liveness: which segments and cell outputs contribute to a chip output or a
// column clock (demanded transitively from those seeds).

struct Liveness {
    horz: Vec<bool>,
    vert: Vec<bool>,
    need_op: Vec<bool>,
    need_carry: Vec<bool>,
    need_reg: Vec<bool>,
}

impl Liveness {
    fn is_live(&self, seg: Seg, fabric: &Fabric) -> bool {
        match seg {
            Seg::Horz { row, lane, pos } => self.horz[(row * fabric.horz_lanes + lane) * (fabric.columns + 1) + pos],
            Seg::Vert { col, lane, pos } => self.vert[(col * fabric.vert_lanes + lane) * fabric.rows + pos],
        }
    }
}

fn liveness(fabric: &Fabric, design: &Design) -> Liveness {
    let (columns, rows) = (fabric.columns, fabric.rows);
    let (hl, vl) = (fabric.horz_lanes, fabric.vert_lanes);
    let mut live = Liveness {
        horz: vec![false; rows * hl * (columns + 1)],
        vert: vec![false; columns * vl * rows],
        need_op: vec![false; columns * rows],
        need_carry: vec![false; columns * rows],
        need_reg: vec![false; columns * rows],
    };

    #[derive(Clone, Copy)]
    enum Demand {
        Seg(Seg),
        Op(usize, usize),
        Carry(usize, usize),
        Reg(usize, usize),
    }

    let mux_for = |drives: BusOut| {
        fabric
            .output_muxes
            .iter()
            .position(|m| m.drives == drives)
            .expect("validated: every lane driven")
    };
    let hmux: Vec<usize> = (0..hl).map(|lane| mux_for(BusOut::Horz(lane))).collect();
    let vmux: Vec<usize> = (0..vl).map(|lane| mux_for(BusOut::Vert(lane))).collect();

    let mut work: Vec<Demand> = Vec::new();
    for (row, lanes) in fabric.io_outputs.iter().enumerate() {
        for (lane, name) in lanes.iter().enumerate() {
            if name.is_some() {
                work.push(Demand::Seg(Seg::Horz { row, lane, pos: columns }));
            }
        }
    }

    while let Some(d) = work.pop() {
        match d {
            Demand::Seg(seg) => {
                let slot = match seg {
                    Seg::Horz { row, lane, pos } => &mut live.horz[(row * hl + lane) * (columns + 1) + pos],
                    Seg::Vert { col, lane, pos } => &mut live.vert[(col * vl + lane) * rows + pos],
                };
                if *slot {
                    continue;
                }
                *slot = true;
                match seg {
                    Seg::Horz { row, lane, pos: 0 } => {
                        // A gated DDIO input depends on its direction output.
                        let name = &fabric.io_inputs[row][lane];
                        if let Some(d) = fabric.ddio.iter().find(|d| d.input == *name) {
                            if let Some((dr, dl)) = out_pos(fabric, &d.dir) {
                                work.push(Demand::Seg(Seg::Horz { row: dr, lane: dl, pos: columns }));
                            }
                        }
                    }
                    Seg::Horz { row, lane, pos } => {
                        let col = pos - 1;
                        push_source_demand(&mut work, fabric, design, col, row, hmux[lane]);
                    }
                    Seg::Vert { col, lane, pos } => {
                        let driver_row = (pos + rows - 1) % rows;
                        push_source_demand(&mut work, fabric, design, col, driver_row, vmux[lane]);
                    }
                }
            }
            Demand::Op(col, row) => {
                let ci = row * columns + col;
                if live.need_op[ci] {
                    continue;
                }
                live.need_op[ci] = true;
                push_input_demands(&mut work, fabric, design, col, row);
            }
            Demand::Carry(col, row) => {
                let ci = row * columns + col;
                if live.need_carry[ci] {
                    continue;
                }
                live.need_carry[ci] = true;
                push_input_demands(&mut work, fabric, design, col, row);
            }
            Demand::Reg(col, row) => {
                let ci = row * columns + col;
                if live.need_reg[ci] {
                    continue;
                }
                live.need_reg[ci] = true;
                // The FF captures op, gated by the enable source, on the
                // column clock — demand all three.
                work.push(Demand::Op(col, row));
                match fabric.ff.enable {
                    Source::HorzIn(k) => work.push(Demand::Seg(Seg::Horz { row, lane: k, pos: col })),
                    Source::VertIn(k) => work.push(Demand::Seg(Seg::Vert { col, lane: k, pos: row })),
                    Source::CarryIn if row > 0 => work.push(Demand::Carry(col, row - 1)),
                    _ => {}
                }
                demand_clock_tap(&mut work, fabric, design, col);
            }
        }
    }
    return live;

    // -- helpers ---------------------------------------------------------

    fn out_pos(fabric: &Fabric, name: &str) -> Option<(usize, usize)> {
        fabric.io_outputs.iter().enumerate().find_map(|(row, lanes)| {
            lanes
                .iter()
                .position(|n| n.as_deref() == Some(name))
                .map(|lane| (row, lane))
        })
    }

    fn push_source_demand(
        work: &mut Vec<Demand>,
        fabric: &Fabric,
        design: &Design,
        col: usize,
        row: usize,
        mux: usize,
    ) {
        let m = &fabric.output_muxes[mux];
        let sel = design.clb(col, row).slice(&m.select) as usize;
        match m.sources[sel] {
            Source::Op => work.push(Demand::Op(col, row)),
            Source::Carry => work.push(Demand::Carry(col, row)),
            Source::Reg => work.push(Demand::Reg(col, row)),
            Source::Const(_) => {}
            Source::HorzIn(k) => work.push(Demand::Seg(Seg::Horz { row, lane: k, pos: col })),
            Source::VertIn(k) => work.push(Demand::Seg(Seg::Vert { col, lane: k, pos: row })),
            Source::CarryIn => push_carry_demand(work, col, row),
        }
    }

    /// The carry chain runs upward, so a cell's carry input demands the carry
    /// of the cell below; at row 0 it is the chain's edge constant.
    fn push_carry_demand(work: &mut Vec<Demand>, col: usize, row: usize) {
        if row > 0 {
            work.push(Demand::Carry(col, row - 1));
        }
    }

    fn push_input_demands(work: &mut Vec<Demand>, fabric: &Fabric, design: &Design, col: usize, row: usize) {
        for m in &fabric.input_muxes {
            let sel = design.clb(col, row).slice(&m.select) as usize;
            match m.sources[sel] {
                Source::HorzIn(k) => work.push(Demand::Seg(Seg::Horz { row, lane: k, pos: col })),
                Source::VertIn(k) => work.push(Demand::Seg(Seg::Vert { col, lane: k, pos: row })),
                Source::CarryIn => push_carry_demand(work, col, row),
                _ => {}
            }
        }
    }

    fn demand_clock_tap(work: &mut Vec<Demand>, fabric: &Fabric, design: &Design, col: usize) {
        let clock = &fabric.csb_clock;
        let columns = fabric.columns;
        let mut visited = vec![false; columns];
        let mut cur = col;
        loop {
            if visited[cur] {
                return;
            }
            visited[cur] = true;
            if design.csb(cur).get(clock.couple_field) != 0 {
                cur = (cur + columns - 1) % columns;
                continue;
            }
            let sel = design.csb(cur).slice(&clock.select) as usize;
            let lane = clock.ring_lanes[sel];
            work.push(Demand::Seg(Seg::Vert { col: cur, lane, pos: 0 }));
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// Structural loop detection over the value-dependency graph. A cycle that
// passes through CLB logic or a DDIO gate can oscillate; pure pass-through
// cycles are the floating rings reported separately.

/// Every structural combinational cycle in the configuration, each as the
/// list of blocks it runs through. A cycle here does not always oscillate --
/// a non-inverting loop settles -- but it is never intentional, and event
/// simulators can spin on one even where a fixed point exists.
pub fn combinational_loops(fabric: &Fabric, design: &Design) -> Vec<Vec<BlockId>> {
    let (columns, rows) = (fabric.columns, fabric.rows);
    let (hl, vl) = (fabric.horz_lanes, fabric.vert_lanes);
    let n_h = rows * hl * (columns + 1);
    let n_v = columns * vl * rows;
    let idx = |seg: Seg| match seg {
        Seg::Horz { row, lane, pos } => (row * hl + lane) * (columns + 1) + pos,
        Seg::Vert { col, lane, pos } => n_h + (col * vl + lane) * rows + pos,
    };

    // Dependency edges, each annotated with the CLB it passes through (None
    // for plain pass-through / gate edges) and whether it goes through logic
    // or a gate rather than a wire.
    struct Edge {
        to: usize,
        through_logic: bool,
        block: Option<BlockId>,
    }
    let mut edges: Vec<Vec<Edge>> = (0..n_h + n_v).map(|_| Vec::new()).collect();

    let mux_sources = |mux: &crate::fabric::OutputMux, col: usize, row: usize| {
        let sel = design.clb(col, row).slice(&mux.select) as usize;
        mux.sources[sel]
    };
    let add_source_edges = |edges: &mut Vec<Vec<Edge>>, from: usize, src: Source, col: usize, row: usize| match src {
        Source::HorzIn(k) => edges[from].push(Edge {
            to: idx(Seg::Horz { row, lane: k, pos: col }),
            through_logic: false,
            block: Some(BlockId::Clb { col, row }),
        }),
        Source::VertIn(k) => edges[from].push(Edge {
            to: idx(Seg::Vert { col, lane: k, pos: row }),
            through_logic: false,
            block: Some(BlockId::Clb { col, row }),
        }),
        Source::Op | Source::Carry => {
            // The cell's own inputs, plus every cell feeding it down the
            // carry chain (which runs upward, so this always terminates).
            let mut r = row;
            loop {
                let mut chained = false;
                for m in &fabric.input_muxes {
                    let sel = design.clb(col, r).slice(&m.select) as usize;
                    let to = match m.sources[sel] {
                        Source::HorzIn(k) => Some(idx(Seg::Horz { row: r, lane: k, pos: col })),
                        Source::VertIn(k) => Some(idx(Seg::Vert { col, lane: k, pos: r })),
                        Source::CarryIn => {
                            chained = r > 0;
                            None
                        }
                        _ => None,
                    };
                    if let Some(to) = to {
                        edges[from].push(Edge {
                            to,
                            through_logic: true,
                            block: Some(BlockId::Clb { col, row: r }),
                        });
                    }
                }
                if !chained {
                    break;
                }
                r -= 1;
            }
        }
        // Reg is sequential state and Const has no dependency; a carry input
        // is expanded by the Op/Carry arm of whichever cell reads it.
        Source::Reg | Source::Const(_) | Source::CarryIn => {}
    };

    for row in 0..rows {
        for lane in 0..hl {
            // Gated left-edge inputs depend on the DDIO direction output.
            let name = &fabric.io_inputs[row][lane];
            if let Some(d) = fabric.ddio.iter().find(|d| d.input == *name) {
                if let Some((dr, dl)) = fabric.io_outputs.iter().enumerate().find_map(|(r, lanes)| {
                    lanes.iter().position(|n| n.as_deref() == Some(d.dir.as_str())).map(|l| (r, l))
                }) {
                    let from = idx(Seg::Horz { row, lane, pos: 0 });
                    edges[from].push(Edge {
                        to: idx(Seg::Horz { row: dr, lane: dl, pos: columns }),
                        through_logic: true,
                        block: None,
                    });
                }
            }
            for pos in 1..=columns {
                let col = pos - 1;
                let from = idx(Seg::Horz { row, lane, pos });
                let m = fabric.output_muxes.iter().find(|m| m.drives == BusOut::Horz(lane)).unwrap();
                let src = mux_sources(m, col, row);
                add_source_edges(&mut edges, from, src, col, row);
            }
        }
    }
    for col in 0..columns {
        for lane in 0..vl {
            for pos in 0..rows {
                let driver_row = (pos + rows - 1) % rows;
                let from = idx(Seg::Vert { col, lane, pos });
                let m = fabric.output_muxes.iter().find(|m| m.drives == BusOut::Vert(lane)).unwrap();
                let src = mux_sources(m, col, driver_row);
                add_source_edges(&mut edges, from, src, col, driver_row);
            }
        }
    }

    // Iterative DFS; report each cycle containing a through-logic edge once.
    let n = edges.len();
    let mut color = vec![0u8; n]; // 0 white, 1 on stack, 2 done
    let mut loops: Vec<Vec<BlockId>> = Vec::new();
    for start in 0..n {
        if color[start] != 0 {
            continue;
        }
        // Stack of (node, next edge index); path holds the edges taken.
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        color[start] = 1;
        let mut path: Vec<(usize, usize)> = Vec::new(); // (edge owner node, edge index)
        loop {
            let Some(&(node, e)) = stack.last() else { break };
            if e < edges[node].len() {
                stack.last_mut().expect("non-empty").1 += 1;
                let to = edges[node][e].to;
                match color[to] {
                    0 => {
                        color[to] = 1;
                        path.push((node, e));
                        stack.push((to, 0));
                    }
                    1 => {
                        // Back edge: collect the cycle from `to` around to node.
                        let mut cycle_edges: Vec<(usize, usize)> = vec![(node, e)];
                        for &(pn, pe) in path.iter().rev() {
                            cycle_edges.push((pn, pe));
                            if pn == to {
                                break;
                            }
                        }
                        let mut blocks: Vec<BlockId> = Vec::new();
                        let mut has_logic = false;
                        for (pn, pe) in cycle_edges {
                            let edge = &edges[pn][pe];
                            has_logic |= edge.through_logic;
                            if let Some(b) = edge.block {
                                if !blocks.contains(&b) {
                                    blocks.push(b);
                                }
                            }
                        }
                        if has_logic {
                            blocks.sort_by_key(|b| match *b {
                                BlockId::Clb { col, row } => (0, col, row),
                                BlockId::Csb { col } => (1, col, 0),
                            });
                            if !loops.contains(&blocks) {
                                loops.push(blocks);
                            }
                        }
                    }
                    _ => {}
                }
            } else {
                color[node] = 2;
                stack.pop();
                path.pop();
            }
        }
    }
    loops
}
