//! Tick-based simulator.
//!
//! Each tick executes in this exact order (see `docs/simulation.md`):
//! 1. apply the stimulus values for this tick;
//! 2. settle all combinational logic to a fixed point, using the register
//!    values held from the end of the previous tick;
//! 3. resolve every CSB's clock from that settled state;
//! 4. compare each column's clock against its sample from the previous tick —
//!    a 0->1 transition is a rising edge;
//! 5. commit all register updates simultaneously for every rising column.
//!
//! No edges fire on tick 0: the previous-clock samples are seeded with the
//! tick-0 resolved values. Registers start holding their configured reset
//! values. A pass-through ring with no driver settles at 0. Combinational
//! loops that cannot settle (only reachable through the DDIO direction
//! feedback in this fabric) are detected and reported with the blocks
//! involved, never hung on.

use crate::config::{BlockId, Design};
use crate::fabric::{BusOut, CarryChain, Fabric, Source};
use std::collections::HashMap;

/// Per-input stimulus patterns, keyed by the fabric's reserved input names.
/// A pattern repeats once exhausted; a missing pattern reads as constant 0.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stimulus {
    patterns: HashMap<String, Vec<bool>>,
}

impl Stimulus {
    pub fn set(&mut self, name: &str, pattern: Vec<bool>) {
        if pattern.is_empty() {
            self.patterns.remove(name);
        } else {
            self.patterns.insert(name.to_string(), pattern);
        }
    }

    pub fn pattern(&self, name: &str) -> Option<&[bool]> {
        self.patterns.get(name).map(Vec::as_slice)
    }

    pub fn value_at(&self, name: &str, tick: u64) -> bool {
        match self.patterns.get(name) {
            Some(p) if !p.is_empty() => p[(tick % p.len() as u64) as usize],
            _ => false,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &[bool])> {
        self.patterns.iter().map(|(n, p)| (n.as_str(), p.as_slice()))
    }
}

#[derive(Debug, Clone)]
pub enum SimError {
    /// Combinational logic did not reach a fixed point; `blocks` are the CLBs
    /// whose outputs were still changing.
    CombinationalLoop { blocks: Vec<BlockId> },
}

impl std::fmt::Display for SimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SimError::CombinationalLoop { blocks } => {
                write!(f, "combinational loop: logic never settles ({} blocks involved)", blocks.len())
            }
        }
    }
}

impl std::error::Error for SimError {}

/// The settled combinational state of one tick, before register commit.
#[derive(Debug, Clone)]
pub struct Settled {
    pub tick: u64,
    columns: usize,
    rows: usize,
    horz_lanes: usize,
    vert_lanes: usize,
    horz: Vec<bool>,
    vert: Vec<bool>,
    op: Vec<bool>,
    carry: Vec<bool>,
    ff: Vec<bool>,
    carry_edge: bool,
    /// Resolved clock value per column.
    pub clocks: Vec<bool>,
}

impl Settled {
    fn hidx(&self, row: usize, lane: usize, pos: usize) -> usize {
        (row * self.horz_lanes + lane) * (self.columns + 1) + pos
    }

    fn vidx(&self, col: usize, lane: usize, pos: usize) -> usize {
        (col * self.vert_lanes + lane) * self.rows + pos
    }

    fn cidx(&self, col: usize, row: usize) -> usize {
        row * self.columns + col
    }

    /// Value on the horizontal segment entering CLB `(col, row)` on `lane`.
    pub fn horz_in(&self, col: usize, row: usize, lane: usize) -> bool {
        self.horz[self.hidx(row, lane, col)]
    }

    /// Value on the segment entering the output controller (`row`, `lane`) —
    /// i.e. the chip output mapped there.
    pub fn horz_edge(&self, row: usize, lane: usize) -> bool {
        self.horz[self.hidx(row, lane, self.columns)]
    }

    pub fn vert_in(&self, col: usize, row: usize, lane: usize) -> bool {
        self.vert[self.vidx(col, lane, row)]
    }

    pub fn ring_tap(&self, col: usize, lane: usize) -> bool {
        self.vert_in(col, 0, lane)
    }

    pub fn clb_op(&self, col: usize, row: usize) -> bool {
        self.op[self.cidx(col, row)]
    }

    pub fn clb_carry(&self, col: usize, row: usize) -> bool {
        self.carry[self.cidx(col, row)]
    }

    /// The value arriving on this cell's dedicated carry-chain input.
    pub fn clb_carry_in(&self, col: usize, row: usize) -> bool {
        if row == 0 {
            self.carry_edge
        } else {
            self.clb_carry(col, row - 1)
        }
    }

    /// The register value held during this tick (pre-commit).
    pub fn clb_ff(&self, col: usize, row: usize) -> bool {
        self.ff[self.cidx(col, row)]
    }
}

/// Simulator state carried between ticks.
#[derive(Debug, Clone)]
pub struct SimState {
    pub tick: u64,
    /// Manual reset level: while true, every rising column edge loads the
    /// configured reset values instead of capturing (synchronous reset).
    pub reset: bool,
    columns: usize,
    ff: Vec<bool>,
    prev_clk: Vec<bool>,
}

impl SimState {
    /// Start from the reset state at tick 0. Also settles tick 0 to seed the
    /// previous-clock samples (so no edges fire on tick 0) and returns that
    /// settled view.
    pub fn new(fabric: &Fabric, design: &Design, stimulus: &Stimulus) -> Result<(SimState, Settled), SimError> {
        let ff = reset_values(fabric, design);
        let settled = settle(fabric, design, stimulus, 0, &ff)?;
        let state = SimState {
            tick: 0,
            reset: false,
            columns: fabric.columns,
            ff,
            prev_clk: settled.clocks.clone(),
        };
        Ok((state, settled))
    }

    pub fn ff_value(&self, col: usize, row: usize) -> bool {
        self.ff[row * self.columns + col]
    }

    /// Settle the current tick again without advancing (register values as
    /// committed at the end of the last step).
    pub fn view(&self, fabric: &Fabric, design: &Design, stimulus: &Stimulus) -> Result<Settled, SimError> {
        settle(fabric, design, stimulus, self.tick, &self.ff)
    }

    /// Advance one tick and return its settled (pre-commit) state.
    pub fn step(&mut self, fabric: &Fabric, design: &Design, stimulus: &Stimulus) -> Result<Settled, SimError> {
        self.tick += 1;
        let settled = settle(fabric, design, stimulus, self.tick, &self.ff)?;
        for col in 0..fabric.columns {
            if self.prev_clk[col] || !settled.clocks[col] {
                continue;
            }
            for row in 0..fabric.rows {
                let i = row * fabric.columns + col;
                if self.reset {
                    self.ff[i] = design.clb(col, row).get(fabric.ff.reset_value_field) != 0;
                } else if enable_value(fabric, &settled, col, row) {
                    self.ff[i] = settled.clb_op(col, row);
                }
            }
        }
        self.prev_clk = settled.clocks.clone();
        Ok(settled)
    }
}

fn reset_values(fabric: &Fabric, design: &Design) -> Vec<bool> {
    let mut ff = vec![false; fabric.columns * fabric.rows];
    for row in 0..fabric.rows {
        for col in 0..fabric.columns {
            ff[row * fabric.columns + col] = design.clb(col, row).get(fabric.ff.reset_value_field) != 0;
        }
    }
    ff
}

fn enable_value(fabric: &Fabric, s: &Settled, col: usize, row: usize) -> bool {
    match fabric.ff.enable {
        Source::HorzIn(k) => s.horz_in(col, row, k),
        Source::VertIn(k) => s.vert_in(col, row, k),
        Source::CarryIn => s.clb_carry_in(col, row),
        Source::Const(b) => b,
        // Validation rejects local outputs as FF enables.
        _ => false,
    }
}

/// What drives one left-edge lane.
enum LeftPin {
    Const(bool),
    /// A stimulus input, optionally gated off when the named DDIO direction
    /// output (at the right edge) is 1.
    Stim { name: String, gate: Option<(usize, usize)> },
}

fn settle(
    fabric: &Fabric,
    design: &Design,
    stimulus: &Stimulus,
    tick: u64,
    ff: &[bool],
) -> Result<Settled, SimError> {
    let (columns, rows) = (fabric.columns, fabric.rows);
    let (hl, vl) = (fabric.horz_lanes, fabric.vert_lanes);
    let hidx = |row: usize, lane: usize, pos: usize| (row * hl + lane) * (columns + 1) + pos;
    let vidx = |col: usize, lane: usize, pos: usize| (col * vl + lane) * rows + pos;

    let mut hmux = vec![0; hl];
    let mut vmux = vec![0; vl];
    for (i, m) in fabric.output_muxes.iter().enumerate() {
        match m.drives {
            BusOut::Horz(lane) => hmux[lane] = i,
            BusOut::Vert(lane) => vmux[lane] = i,
        }
    }

    // Left-edge drivers, with DDIO gating positions resolved to right-edge
    // (row, lane) coordinates.
    let dir_pos = |name: &str| {
        fabric.io_outputs.iter().enumerate().find_map(|(row, lanes)| {
            lanes
                .iter()
                .position(|n| n.as_deref() == Some(name))
                .map(|lane| (row, lane))
        })
    };
    let mut left = Vec::with_capacity(rows * hl);
    for row in 0..rows {
        for lane in 0..hl {
            let name = &fabric.io_inputs[row][lane];
            left.push(if *name == fabric.naming.constant_zero {
                LeftPin::Const(false)
            } else if *name == fabric.naming.constant_one {
                LeftPin::Const(true)
            } else {
                let gate = fabric
                    .ddio
                    .iter()
                    .find(|d| d.input == *name)
                    .and_then(|d| dir_pos(&d.dir));
                LeftPin::Stim { name: name.clone(), gate }
            });
        }
    }

    let mut h = vec![false; rows * hl * (columns + 1)];
    let mut v = vec![false; columns * vl * rows];
    let mut op = vec![false; columns * rows];
    let mut carry = vec![false; columns * rows];

    #[allow(clippy::too_many_arguments)]
    let src_val = |src: Source,
                   col: usize,
                   row: usize,
                   h: &[bool],
                   v: &[bool],
                   op_v: bool,
                   carry_v: bool,
                   ff_v: bool,
                   carry_in_v: bool| match src {
        Source::HorzIn(k) => h[hidx(row, k, col)],
        Source::VertIn(k) => v[vidx(col, k, row)],
        Source::Const(b) => b,
        Source::Op => op_v,
        Source::Reg => ff_v,
        Source::Carry => carry_v,
        Source::CarryIn => carry_in_v,
    };

    let max_sweeps = h.len() + v.len() + 4;
    let mut changed_blocks: Vec<BlockId> = Vec::new();
    for sweep in 0..=max_sweeps {
        let mut changed = false;
        changed_blocks.clear();

        for row in 0..rows {
            for lane in 0..hl {
                let val = match &left[row * hl + lane] {
                    LeftPin::Const(b) => *b,
                    LeftPin::Stim { name, gate } => {
                        let mut x = stimulus.value_at(name, tick);
                        if let Some((dr, dl)) = gate {
                            if h[hidx(*dr, *dl, columns)] {
                                x = false;
                            }
                        }
                        x
                    }
                };
                let i = hidx(row, lane, 0);
                if h[i] != val {
                    h[i] = val;
                    changed = true;
                }
            }
        }

        for col in 0..columns {
            // Rows ascend, and the carry chain runs upward, so a cell's
            // carry input is already final when the cell is evaluated.
            for row in 0..rows {
                let ci = row * columns + col;
                let carry_in_v = match fabric.carry.chain {
                    CarryChain::ColumnUp => {
                        if row == 0 {
                            fabric.carry.edge
                        } else {
                            carry[ci - columns]
                        }
                    }
                };
                let cfg = design.clb(col, row);
                let mut idx = 0usize;
                for (k, m) in fabric.input_muxes.iter().enumerate() {
                    let sel = cfg.slice(&m.select) as usize;
                    let val = src_val(m.sources[sel], col, row, &h, &v, op[ci], carry[ci], ff[ci], carry_in_v);
                    idx |= (val as usize) << k;
                }
                let code = cfg.slice(&fabric.op_select) as usize;
                let o = fabric.operations[code].table[idx];
                let c = fabric.carry_table[idx];
                let mut cell_changed = false;
                if op[ci] != o {
                    op[ci] = o;
                    cell_changed = true;
                }
                if carry[ci] != c {
                    carry[ci] = c;
                    cell_changed = true;
                }
                for lane in 0..hl {
                    let m = &fabric.output_muxes[hmux[lane]];
                    let sel = cfg.slice(&m.select) as usize;
                    let val = src_val(m.sources[sel], col, row, &h, &v, op[ci], carry[ci], ff[ci], carry_in_v);
                    let i = hidx(row, lane, col + 1);
                    if h[i] != val {
                        h[i] = val;
                        cell_changed = true;
                    }
                }
                for lane in 0..vl {
                    let m = &fabric.output_muxes[vmux[lane]];
                    let sel = cfg.slice(&m.select) as usize;
                    let val = src_val(m.sources[sel], col, row, &h, &v, op[ci], carry[ci], ff[ci], carry_in_v);
                    let i = vidx(col, lane, (row + 1) % rows);
                    if v[i] != val {
                        v[i] = val;
                        cell_changed = true;
                    }
                }
                if cell_changed {
                    changed = true;
                    changed_blocks.push(BlockId::Clb { col, row });
                }
            }
        }

        if !changed {
            break;
        }
        if sweep == max_sweeps {
            return Err(SimError::CombinationalLoop { blocks: changed_blocks });
        }
    }

    // Resolve each column's clock from the settled ring values, following
    // couple chains; a full circle has no source and reads 0.
    let clock_spec = &fabric.csb_clock;
    let clocks = (0..columns)
        .map(|col| {
            let mut visited = vec![false; columns];
            let mut cur = col;
            loop {
                if visited[cur] {
                    return false;
                }
                visited[cur] = true;
                if design.csb(cur).get(clock_spec.couple_field) != 0 {
                    cur = (cur + columns - 1) % columns;
                    continue;
                }
                let sel = design.csb(cur).slice(&clock_spec.select) as usize;
                let lane = clock_spec.ring_lanes[sel];
                return v[vidx(cur, lane, 0)];
            }
        })
        .collect();

    Ok(Settled {
        tick,
        columns,
        rows,
        horz_lanes: hl,
        vert_lanes: vl,
        horz: h,
        vert: v,
        op,
        carry,
        ff: ff.to_vec(),
        carry_edge: fabric.carry.edge,
        clocks,
    })
}
