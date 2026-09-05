//! Net naming engine. Resolves the true origin of the signal on every bus
//! segment and every column clock, given the fabric description and a
//! design's configuration.
//!
//! The central rule: a routing mux that selects an incoming bus lane is a
//! pass-through — the net keeps its origin across the segment boundary. Only
//! a mux selecting a local CLB output (`op`, `reg`, `carry`) introduces a new
//! net, named after that CLB. Pass-through chains can cycle (the vertical
//! buses are closed rings), so resolution detects cycles; a ring fed only by
//! its own pass-throughs has no origin and resolves to [`NetOrigin::FloatingLoop`].
//!
//! Origins are structural (block coordinates, not strings), so a resolved
//! [`Netlist`] stays valid across renames: display names are produced by
//! [`Namer`] at render time, which applies pinned names automatically.

use crate::config::{default_name, BlockId, Design};
use crate::fabric::{BusOut, CarryChain, Fabric, Source};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputKind {
    Op,
    Reg,
    Carry,
}

/// Where the signal on a segment really comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetOrigin {
    /// Driven by the input controller at the left edge (reserved name).
    Io { row: usize, lane: usize },
    /// A constant selected by an output mux (or a row-3 IO constant, which
    /// shares the reserved constant names via `Io`).
    Const(bool),
    /// A CLB output — a genuinely new net.
    ClbOutput { col: usize, row: usize, kind: OutputKind },
    /// A pass-through cycle with no real driver (e.g. a vertical ring where
    /// every hop passes through). Displays as the unconnected marker.
    FloatingLoop,
}

/// One column's resolved clock. `origin` is `None` when the CSB couple chain
/// selects in a full circle with no real source (a DRC error). `source_csb`
/// is the CSB whose bus selection sourced the clock — the end of the couple
/// chain — used for the fallback name; it equals the column itself when the
/// chain is circular or uncoupled.
#[derive(Debug, Clone)]
pub struct ClockNet {
    pub origin: Option<NetOrigin>,
    pub source_csb: usize,
}

/// The resolved origin of every bus segment and clock in the fabric.
///
/// Horizontal segments are indexed by the column position they enter:
/// position `c` is the segment entering CLB `(c, row)`, and position
/// `columns` is the segment entering the output controller. Vertical
/// segments are a ring: position `r` is the segment entering row `r`, and
/// position 0 — driven by the top row — is also the CSB tap point.
pub struct Netlist {
    columns: usize,
    rows: usize,
    horz_lanes: usize,
    vert_lanes: usize,
    horz: Vec<NetOrigin>,
    vert: Vec<NetOrigin>,
    /// Carry-chain input origin per cell, row-major.
    carry: Vec<NetOrigin>,
    pub clocks: Vec<ClockNet>,
}

impl Netlist {
    fn hidx(&self, row: usize, lane: usize, pos: usize) -> usize {
        (row * self.horz_lanes + lane) * (self.columns + 1) + pos
    }

    fn vidx(&self, col: usize, lane: usize, pos: usize) -> usize {
        (col * self.vert_lanes + lane) * self.rows + pos
    }

    /// The horizontal segment entering CLB `(col, row)` on `lane`.
    pub fn horz_in(&self, col: usize, row: usize, lane: usize) -> NetOrigin {
        self.horz[self.hidx(row, lane, col)]
    }

    /// The horizontal segment entering the output controller on `row`/`lane`.
    pub fn horz_edge(&self, row: usize, lane: usize) -> NetOrigin {
        self.horz[self.hidx(row, lane, self.columns)]
    }

    /// The vertical segment entering CLB `(col, row)` on `lane`.
    pub fn vert_in(&self, col: usize, row: usize, lane: usize) -> NetOrigin {
        self.vert[self.vidx(col, lane, row)]
    }

    /// The vertical ring value at the loop point — what the column's CSB can
    /// tap on `lane`. Identical to `vert_in(col, 0, lane)`.
    pub fn ring_tap(&self, col: usize, lane: usize) -> NetOrigin {
        self.vert_in(col, 0, lane)
    }

    /// What arrives on CLB `(col, row)`'s dedicated carry-chain input: the
    /// neighbouring cell's carry output, or the chain's edge constant.
    pub fn carry_in(&self, col: usize, row: usize) -> NetOrigin {
        self.carry[row * self.columns + col]
    }

    /// The CLB inspector's input list: the horizontal lanes entering the
    /// block, lane 0 first. (The column clock is not in this list — it
    /// arrives on the dedicated network; see [`Netlist::clocks`].)
    pub fn clb_inputs(&self, col: usize, row: usize) -> Vec<NetOrigin> {
        (0..self.horz_lanes).map(|lane| self.horz_in(col, row, lane)).collect()
    }
}

/// Resolve the whole fabric. Cheap enough to rerun on every config change.
pub fn resolve(fabric: &Fabric, design: &Design) -> Netlist {
    let mut r = Resolver::new(fabric, design);
    for row in 0..fabric.rows {
        for lane in 0..fabric.horz_lanes {
            for pos in 0..=fabric.columns {
                r.horz_origin(row, lane, pos);
            }
        }
    }
    for col in 0..fabric.columns {
        for lane in 0..fabric.vert_lanes {
            for pos in 0..fabric.rows {
                r.vert_origin(col, lane, pos);
            }
        }
    }
    let mut carry = Vec::with_capacity(fabric.columns * fabric.rows);
    for row in 0..fabric.rows {
        for col in 0..fabric.columns {
            carry.push(r.carry_in_origin(col, row));
        }
    }
    let clocks = r.resolve_clocks();
    let horz = r.horz.into_iter().map(Slot::finish).collect();
    let vert = r.vert.into_iter().map(Slot::finish).collect();
    Netlist {
        columns: fabric.columns,
        rows: fabric.rows,
        horz_lanes: fabric.horz_lanes,
        vert_lanes: fabric.vert_lanes,
        horz,
        vert,
        carry,
        clocks,
    }
}

#[derive(Clone, Copy)]
enum Slot {
    Todo,
    Doing,
    Done(NetOrigin),
}

impl Slot {
    fn finish(self) -> NetOrigin {
        match self {
            Slot::Done(o) => o,
            // Unreachable after a full resolve pass; be safe rather than panic.
            _ => NetOrigin::FloatingLoop,
        }
    }
}

struct Resolver<'a> {
    fabric: &'a Fabric,
    design: &'a Design,
    /// Output mux index per outgoing horizontal / vertical lane (validation
    /// guarantees exactly one each).
    hmux: Vec<usize>,
    vmux: Vec<usize>,
    horz: Vec<Slot>,
    vert: Vec<Slot>,
}

impl<'a> Resolver<'a> {
    fn new(fabric: &'a Fabric, design: &'a Design) -> Self {
        let mut hmux = vec![0; fabric.horz_lanes];
        let mut vmux = vec![0; fabric.vert_lanes];
        for (i, m) in fabric.output_muxes.iter().enumerate() {
            match m.drives {
                BusOut::Horz(lane) => hmux[lane] = i,
                BusOut::Vert(lane) => vmux[lane] = i,
            }
        }
        Resolver {
            fabric,
            design,
            hmux,
            vmux,
            horz: vec![Slot::Todo; fabric.rows * fabric.horz_lanes * (fabric.columns + 1)],
            vert: vec![Slot::Todo; fabric.columns * fabric.vert_lanes * fabric.rows],
        }
    }

    fn hidx(&self, row: usize, lane: usize, pos: usize) -> usize {
        (row * self.fabric.horz_lanes + lane) * (self.fabric.columns + 1) + pos
    }

    fn vidx(&self, col: usize, lane: usize, pos: usize) -> usize {
        (col * self.fabric.vert_lanes + lane) * self.fabric.rows + pos
    }

    fn horz_origin(&mut self, row: usize, lane: usize, pos: usize) -> NetOrigin {
        let i = self.hidx(row, lane, pos);
        match self.horz[i] {
            Slot::Done(o) => return o,
            // Re-entering a segment mid-resolution means a pure pass-through
            // cycle; the in-progress computation will settle to the same value.
            Slot::Doing => return NetOrigin::FloatingLoop,
            Slot::Todo => {}
        }
        self.horz[i] = Slot::Doing;
        let origin = if pos == 0 {
            NetOrigin::Io { row, lane }
        } else {
            let col = pos - 1;
            let mux = self.hmux[lane];
            let sel = self.design.clb(col, row).slice(&self.fabric.output_muxes[mux].select) as usize;
            let src = self.fabric.output_muxes[mux].sources[sel];
            self.source_origin(col, row, src)
        };
        self.horz[i] = Slot::Done(origin);
        origin
    }

    fn vert_origin(&mut self, col: usize, lane: usize, pos: usize) -> NetOrigin {
        let i = self.vidx(col, lane, pos);
        match self.vert[i] {
            Slot::Done(o) => return o,
            Slot::Doing => return NetOrigin::FloatingLoop,
            Slot::Todo => {}
        }
        self.vert[i] = Slot::Doing;
        let driver_row = (pos + self.fabric.rows - 1) % self.fabric.rows;
        let mux = self.vmux[lane];
        let sel = self.design.clb(col, driver_row).slice(&self.fabric.output_muxes[mux].select) as usize;
        let src = self.fabric.output_muxes[mux].sources[sel];
        let origin = self.source_origin(col, driver_row, src);
        self.vert[i] = Slot::Done(origin);
        origin
    }

    /// The origin of `src` as selected by an output mux in CLB `(col, row)`.
    fn source_origin(&mut self, col: usize, row: usize, src: Source) -> NetOrigin {
        match src {
            Source::Op => NetOrigin::ClbOutput { col, row, kind: OutputKind::Op },
            Source::Reg => NetOrigin::ClbOutput { col, row, kind: OutputKind::Reg },
            Source::Carry => NetOrigin::ClbOutput { col, row, kind: OutputKind::Carry },
            Source::Const(b) => NetOrigin::Const(b),
            Source::HorzIn(k) => self.horz_origin(row, k, col),
            Source::VertIn(k) => self.vert_origin(col, k, row),
            Source::CarryIn => self.carry_in_origin(col, row),
        }
    }

    /// The carry chain is structural — the neighbouring cell's carry output,
    /// or the edge constant — so it needs no cycle handling.
    fn carry_in_origin(&self, col: usize, row: usize) -> NetOrigin {
        match self.fabric.carry.chain {
            CarryChain::ColumnUp => {
                if row == 0 {
                    NetOrigin::Const(self.fabric.carry.edge)
                } else {
                    NetOrigin::ClbOutput { col, row: row - 1, kind: OutputKind::Carry }
                }
            }
        }
    }

    fn resolve_clocks(&mut self) -> Vec<ClockNet> {
        let columns = self.fabric.columns;
        let clock = &self.fabric.csb_clock;
        (0..columns)
            .map(|col| {
                let mut visited = vec![false; columns];
                let mut cur = col;
                loop {
                    if visited[cur] {
                        // Coupled in a full circle: no real source.
                        return ClockNet { origin: None, source_csb: col };
                    }
                    visited[cur] = true;
                    if self.design.csb(cur).get(clock.couple_field) != 0 {
                        cur = (cur + columns - 1) % columns;
                        continue;
                    }
                    let sel = self.design.csb(cur).slice(&clock.select) as usize;
                    let lane = clock.ring_lanes[sel];
                    let origin = self.vert_origin(cur, lane, 0);
                    return ClockNet { origin: Some(origin), source_csb: cur };
                }
            })
            .collect()
    }
}

/// Renders display names for blocks, nets and clocks, applying pinned names.
pub struct Namer<'a> {
    pub fabric: &'a Fabric,
    pub design: &'a Design,
}

impl<'a> Namer<'a> {
    pub fn new(fabric: &'a Fabric, design: &'a Design) -> Self {
        Namer { fabric, design }
    }

    pub fn block_name(&self, block: BlockId) -> String {
        match self.design.pinned_name(block) {
            Some(n) => n.to_string(),
            None => default_name(self.fabric, block),
        }
    }

    pub fn is_pinned(&self, block: BlockId) -> bool {
        self.design.pinned_name(block).is_some()
    }

    pub fn net_name(&self, origin: NetOrigin) -> String {
        match origin {
            NetOrigin::Io { row, lane } => self.fabric.io_inputs[row][lane].clone(),
            NetOrigin::Const(false) => self.fabric.naming.constant_zero.clone(),
            NetOrigin::Const(true) => self.fabric.naming.constant_one.clone(),
            NetOrigin::ClbOutput { col, row, kind } => {
                let suffix = match kind {
                    OutputKind::Op => &self.fabric.naming.suffix_op,
                    OutputKind::Reg => &self.fabric.naming.suffix_reg,
                    OutputKind::Carry => &self.fabric.naming.suffix_carry,
                };
                format!("{}{}", self.block_name(BlockId::Clb { col, row }), suffix)
            }
            NetOrigin::FloatingLoop => self.fabric.naming.unconnected.clone(),
        }
    }

    /// A column clock is named after the net its CSB chain resolves to; a
    /// floating ring or circular couple chain falls back to the fallback
    /// template with the sourcing CSB's column.
    pub fn clock_name(&self, clock: &ClockNet) -> String {
        match clock.origin {
            Some(origin) if origin != NetOrigin::FloatingLoop => self.net_name(origin),
            _ => self
                .fabric
                .naming
                .clock_fallback
                .replace("{col}", &clock.source_csb.to_string()),
        }
    }
}
