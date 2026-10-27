//! How much of the fabric a configuration actually uses.
//!
//! "Used" means two things at once: the resource carries something that
//! reaches a chip output or a live register's clock (DRC's liveness, which
//! follows only the inputs each LUT reads), and what it carries was put there
//! on purpose. The second rule matters because the all-zero configuration is
//! not empty in hardware — every CLB drives its constant-0 LUT result onto its
//! minor lanes and a constant 0 onto its major lanes, and every output pad
//! reads some lane. So a segment carrying the output of an unconfigured CLB,
//! or a constant 0, does not count; a segment carrying a pad's signal, a
//! configured CLB's output or a constant 1 does.
//!
//! A configured CLB is not necessarily doing logic: a routed design sets the
//! lane muxes of every CLB a signal merely passes through, so `clbs` can read
//! close to full. The synthesiser names the CLBs that hold logic and leaves
//! pass-throughs unnamed, so `named_clbs` counts only configured CLBs with a
//! pinned name.

use crate::config::{BlockId, Design};
use crate::drc::liveness;
use crate::fabric::{Fabric, IoSource, Source};
use crate::naming::{resolve, NetOrigin};

/// Used / total, for one kind of resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Count {
    pub used: usize,
    pub total: usize,
}

impl Count {
    pub fn fraction(self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            self.used as f32 / self.total as f32
        }
    }
}

#[derive(Debug, Clone)]
pub struct Utilisation {
    columns: usize,
    rows: usize,
    horz_lanes: usize,
    vert_lanes: usize,
    horz: Vec<bool>,
    vert: Vec<bool>,
    clb: Vec<bool>,
    clb_named: Vec<bool>,
    csb: Vec<bool>,
    pub clbs: Count,
    /// Configured CLBs that also carry a pinned name.
    pub named_clbs: Count,
    pub registers: Count,
    /// CLBs whose carry input is read by live logic.
    pub carry_links: Count,
    pub csbs: Count,
    pub horz_segments: Count,
    pub vert_segments: Count,
    pub input_pads: Count,
    pub output_pads: Count,
    /// Left-edge input muxes carrying something used.
    pub input_muxes: Count,
    /// Left-edge lanes fed by a chip output read back on-chip.
    pub on_chip_loops: Count,
}

impl Utilisation {
    /// Whether the horizontal segment entering column `pos` (`pos ==
    /// columns` is the right edge) on `row`/`lane` is used.
    pub fn horz(&self, row: usize, lane: usize, pos: usize) -> bool {
        self.horz[(row * self.horz_lanes + lane) * (self.columns + 1) + pos]
    }

    /// Whether the vertical segment entering row `pos` of `col` on `lane` is
    /// used.
    pub fn vert(&self, col: usize, lane: usize, pos: usize) -> bool {
        self.vert[(col * self.vert_lanes + lane) * self.rows + pos]
    }

    pub fn block(&self, block: BlockId) -> bool {
        self.block_counted(block, false)
    }

    /// As `block`, but with `named_only` a CLB counts only when it is named.
    pub fn block_counted(&self, block: BlockId, named_only: bool) -> bool {
        match block {
            BlockId::Clb { col, row } if named_only => self.clb_named[row * self.columns + col],
            BlockId::Clb { col, row } => self.clb[row * self.columns + col],
            BlockId::Csb { col } => self.csb[col],
            BlockId::Io => self.input_muxes.used > 0 || self.output_pads.used > 0,
        }
    }

    /// Used segments per (row, lane), out of `columns + 1`.
    pub fn horz_by_lane(&self, row: usize, lane: usize) -> Count {
        Count {
            used: (0..=self.columns).filter(|&pos| self.horz(row, lane, pos)).count(),
            total: self.columns + 1,
        }
    }

    /// Used segments per (column, lane), out of `rows`.
    pub fn vert_by_lane(&self, col: usize, lane: usize) -> Count {
        Count {
            used: (0..self.rows).filter(|&pos| self.vert(col, lane, pos)).count(),
            total: self.rows,
        }
    }
}

pub fn utilisation(fabric: &Fabric, design: &Design) -> Utilisation {
    let (columns, rows) = (fabric.columns, fabric.rows);
    let (hl, vl) = (fabric.horz_lanes, fabric.vert_lanes);
    let live = liveness(fabric, design);
    let nets = resolve(fabric, design);

    let configured = |col: usize, row: usize| {
        (0..fabric.clb_fields.len()).any(|f| design.clb(col, row).get(f) != 0)
    };
    let meaningful = |origin: NetOrigin| match origin {
        NetOrigin::Io { .. } | NetOrigin::Const(true) => true,
        NetOrigin::ClbOutput { col, row, .. } => configured(col, row),
        NetOrigin::Const(false) | NetOrigin::FloatingLoop => false,
    };

    let mut horz = vec![false; rows * hl * (columns + 1)];
    for row in 0..rows {
        for lane in 0..hl {
            for pos in 0..=columns {
                let i = (row * hl + lane) * (columns + 1) + pos;
                let origin = if pos < columns { nets.horz_in(pos, row, lane) } else { nets.horz_edge(row, lane) };
                horz[i] = live.horz[i] && meaningful(origin);
            }
        }
    }
    let mut vert = vec![false; columns * vl * rows];
    for col in 0..columns {
        for lane in 0..vl {
            for pos in 0..rows {
                let i = (col * vl + lane) * rows + pos;
                vert[i] = live.vert[i] && meaningful(nets.vert_in(col, pos, lane));
            }
        }
    }

    let mut clb = vec![false; columns * rows];
    let mut clb_named = vec![false; columns * rows];
    let mut registers = 0;
    let mut carry_links = 0;
    let mut reg_columns = vec![false; columns];
    for row in 0..rows {
        for col in 0..columns {
            let ci = row * columns + col;
            clb[ci] = configured(col, row);
            clb_named[ci] = clb[ci] && design.pinned_name(BlockId::Clb { col, row }).is_some();
            if clb[ci] && live.need_reg[ci] {
                registers += 1;
                reg_columns[col] = true;
            }
            let reads_carry = fabric.input_muxes.iter().any(|m| {
                let sel = design.clb(col, row).slice(&m.select) as usize;
                m.sources[sel] == Source::CarryIn
            });
            if clb[ci] && reads_carry && row > 0 && (live.need_op[ci] || live.need_carry[ci]) {
                carry_links += 1;
            }
        }
    }

    // A CSB is used when it clocks a live register, either in its own column
    // or by passing its clock along the coupling chain.
    let mut csb = vec![false; columns];
    for (col, _) in reg_columns.iter().enumerate().filter(|(_, &r)| r) {
        let mut cur = col;
        for _ in 0..columns {
            csb[cur] = true;
            if design.csb(cur).get(fabric.csb_clock.couple_field) == 0 {
                break;
            }
            cur = (cur + columns - 1) % columns;
        }
    }

    // The IO controller.
    let mut in_pads = vec![false; fabric.io_inputs.len()];
    let mut muxes_used = 0;
    let mut muxes_total = 0;
    let mut loops = 0;
    let mut loop_lanes = 0;
    let loopable = fabric.io_mux_sources.iter().any(|s| matches!(s, IoSource::OutPad(_)));
    for row in 0..rows {
        for lane in 0..hl {
            let used = horz[(row * hl + lane) * (columns + 1)];
            let muxed = matches!(fabric.io_left[row][lane], crate::fabric::LeftLane::Mux(_));
            if muxed {
                muxes_total += 1;
                if loopable {
                    loop_lanes += 1;
                }
            }
            if !used {
                continue;
            }
            if muxed {
                muxes_used += 1;
            }
            match design.left_source(fabric, row, lane) {
                IoSource::InPad(pad) => in_pads[pad] = true,
                IoSource::OutPad(_) => loops += 1,
            }
        }
    }
    let out_used = (0..fabric.io_outputs.len())
        .filter(|&o| {
            let (row, lane) = design.pad_lane(fabric, o);
            horz[(row * hl + lane) * (columns + 1) + columns]
        })
        .count();

    Utilisation {
        columns,
        rows,
        horz_lanes: hl,
        vert_lanes: vl,
        clbs: Count { used: clb.iter().filter(|&&u| u).count(), total: columns * rows },
        named_clbs: Count { used: clb_named.iter().filter(|&&u| u).count(), total: columns * rows },
        registers: Count { used: registers, total: columns * rows },
        carry_links: Count { used: carry_links, total: columns * rows.saturating_sub(1) },
        csbs: Count { used: csb.iter().filter(|&&u| u).count(), total: columns },
        horz_segments: Count { used: horz.iter().filter(|&&u| u).count(), total: horz.len() },
        vert_segments: Count { used: vert.iter().filter(|&&u| u).count(), total: vert.len() },
        input_pads: Count { used: in_pads.iter().filter(|&&u| u).count(), total: in_pads.len() },
        output_pads: Count { used: out_used, total: fabric.io_outputs.len() },
        input_muxes: Count { used: muxes_used, total: muxes_total },
        on_chip_loops: Count { used: loops, total: loop_lanes },
        horz,
        vert,
        clb,
        clb_named,
        csb,
    }
}
