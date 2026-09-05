//! Signal tracing and VCD export.
//!
//! A trace target is any observable wire, identified by a stable string id
//! that is saved with the design:
//!   `in:<name>`      chip input (post-DDIO-gating, as the fabric sees it)
//!   `out:<name>`     chip output
//!   `clk:<col>`      column clock
//!   `ff:<col>:<row>` a CLB's register
//!   `op:<col>:<row>` a CLB's operation result
//!   `carry:<col>:<row>`
//!   `h:<row>:<lane>:<pos>` raw horizontal segment (pos = entering that column)
//!   `v:<col>:<lane>:<pos>` raw vertical segment (pos = entering that row)

use crate::fabric::Fabric;
use crate::sim::Settled;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceTarget {
    Input { row: usize, lane: usize },
    Output { row: usize, lane: usize },
    Clock { col: usize },
    Ff { col: usize, row: usize },
    Op { col: usize, row: usize },
    Carry { col: usize, row: usize },
    Horz { row: usize, lane: usize, pos: usize },
    Vert { col: usize, lane: usize, pos: usize },
}

pub fn parse_target(fabric: &Fabric, id: &str) -> Result<TraceTarget, String> {
    let parts: Vec<&str> = id.split(':').collect();
    let num = |s: &str| s.parse::<usize>().map_err(|_| format!("bad number \"{}\" in trace id \"{}\"", s, id));
    match parts.as_slice() {
        ["in", name] => fabric
            .io_inputs
            .iter()
            .enumerate()
            .find_map(|(row, lanes)| lanes.iter().position(|n| n == name).map(|lane| TraceTarget::Input { row, lane }))
            .ok_or_else(|| format!("\"{}\" is not a chip input", name)),
        ["out", name] => fabric
            .io_outputs
            .iter()
            .enumerate()
            .find_map(|(row, lanes)| {
                lanes
                    .iter()
                    .position(|n| n.as_deref() == Some(*name))
                    .map(|lane| TraceTarget::Output { row, lane })
            })
            .ok_or_else(|| format!("\"{}\" is not a chip output", name)),
        ["clk", col] => in_grid(fabric, TraceTarget::Clock { col: num(col)? }),
        ["ff", col, row] => in_grid(fabric, TraceTarget::Ff { col: num(col)?, row: num(row)? }),
        ["op", col, row] => in_grid(fabric, TraceTarget::Op { col: num(col)?, row: num(row)? }),
        ["carry", col, row] => in_grid(fabric, TraceTarget::Carry { col: num(col)?, row: num(row)? }),
        ["h", row, lane, pos] => in_grid(
            fabric,
            TraceTarget::Horz { row: num(row)?, lane: num(lane)?, pos: num(pos)? },
        ),
        ["v", col, lane, pos] => in_grid(
            fabric,
            TraceTarget::Vert { col: num(col)?, lane: num(lane)?, pos: num(pos)? },
        ),
        _ => Err(format!("unknown trace id \"{}\"", id)),
    }
}

fn in_grid(fabric: &Fabric, t: TraceTarget) -> Result<TraceTarget, String> {
    let ok = match t {
        TraceTarget::Clock { col } => col < fabric.columns,
        TraceTarget::Ff { col, row } | TraceTarget::Op { col, row } | TraceTarget::Carry { col, row } => {
            col < fabric.columns && row < fabric.rows
        }
        TraceTarget::Horz { row, lane, pos } => row < fabric.rows && lane < fabric.horz_lanes && pos <= fabric.columns,
        TraceTarget::Vert { col, lane, pos } => col < fabric.columns && lane < fabric.vert_lanes && pos < fabric.rows,
        _ => true,
    };
    if ok {
        Ok(t)
    } else {
        Err("trace target outside the grid".to_string())
    }
}

pub fn target_value(t: TraceTarget, s: &Settled) -> bool {
    match t {
        TraceTarget::Input { row, lane } => s.horz_in(0, row, lane),
        TraceTarget::Output { row, lane } => s.horz_edge(row, lane),
        TraceTarget::Clock { col } => s.clocks[col],
        TraceTarget::Ff { col, row } => s.clb_ff(col, row),
        TraceTarget::Op { col, row } => s.clb_op(col, row),
        TraceTarget::Carry { col, row } => s.clb_carry(col, row),
        TraceTarget::Horz { row, lane, pos } => s.horz_in(pos, row, lane),
        TraceTarget::Vert { col, lane, pos } => s.vert_in(col, pos, lane),
    }
}

/// Records flagged signals over ticks and renders a VCD.
#[derive(Debug, Clone, Default)]
pub struct Tracer {
    pub ids: Vec<String>,
    targets: Vec<TraceTarget>,
    /// `history[tick][signal]`.
    pub history: Vec<Vec<bool>>,
}

impl Tracer {
    /// Build from saved trace ids; unparsable ids are dropped with a warning.
    pub fn new(fabric: &Fabric, ids: &[String]) -> (Tracer, Vec<String>) {
        let mut warnings = Vec::new();
        let mut out = Tracer::default();
        for id in ids {
            match parse_target(fabric, id) {
                Ok(t) => {
                    out.ids.push(id.clone());
                    out.targets.push(t);
                }
                Err(e) => warnings.push(format!("dropping trace \"{}\": {}", id, e)),
            }
        }
        (out, warnings)
    }

    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    /// Record one tick's settled values. Call exactly once per tick, starting
    /// at tick 0.
    pub fn sample(&mut self, s: &Settled) {
        self.history.push(self.targets.iter().map(|&t| target_value(t, s)).collect());
    }

    pub fn clear(&mut self) {
        self.history.clear();
    }

    /// Render the recorded history as a VCD, one timescale unit per tick.
    /// `names` are the display names (resolved net names) per signal, in the
    /// same order as `ids`.
    pub fn to_vcd(&self, names: &[String]) -> String {
        let mut out = String::new();
        out.push_str("$comment SR-GA1 fpgatool trace $end\n");
        out.push_str("$timescale 1 ns $end\n");
        out.push_str("$scope module fabric $end\n");
        let code = |i: usize| {
            // Printable VCD id chars, ! (33) through ~ (126), multi-char as needed.
            let mut n = i;
            let mut s = String::new();
            loop {
                s.push((33 + (n % 94)) as u8 as char);
                n /= 94;
                if n == 0 {
                    break;
                }
                n -= 1;
            }
            s
        };
        for (i, _) in self.targets.iter().enumerate() {
            let name: String = names
                .get(i)
                .map(|n| n.replace(|c: char| c.is_whitespace(), "_"))
                .unwrap_or_else(|| self.ids[i].replace(':', "_"));
            out.push_str(&format!("$var wire 1 {} {} $end\n", code(i), name));
        }
        out.push_str("$upscope $end\n$enddefinitions $end\n");
        let mut last: Vec<Option<bool>> = vec![None; self.targets.len()];
        for (tick, row) in self.history.iter().enumerate() {
            let changes: Vec<(usize, bool)> = row
                .iter()
                .enumerate()
                .filter(|&(i, &v)| last[i] != Some(v))
                .map(|(i, &v)| (i, v))
                .collect();
            if changes.is_empty() {
                continue;
            }
            out.push_str(&format!("#{}\n", tick));
            if tick == 0 {
                out.push_str("$dumpvars\n");
            }
            for (i, v) in changes {
                out.push_str(&format!("{}{}\n", if v { '1' } else { '0' }, code(i)));
                last[i] = Some(v);
            }
            if tick == 0 {
                out.push_str("$end\n");
            }
        }
        // Close the wave at the final tick so viewers show full extent.
        if !self.history.is_empty() {
            out.push_str(&format!("#{}\n", self.history.len() - 1));
        }
        out
    }
}
