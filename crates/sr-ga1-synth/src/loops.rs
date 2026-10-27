//! Combinational loop checking on the emitted configuration.
//!
//! This is not advisory. `vert_in[0]` feeds input mux `c`, and vertical lane 0
//! carries `_op`, so a CLB that drives its own `_op` onto the ring and reads
//! `vert_in[0]` closes a combinational loop through its own LUT. The IO
//! controller adds more ways round: a chip output read back onto the left
//! edge, and a DDIO pad in output mode reading its own output. Roughly nine in
//! ten randomly configured fabrics contain a loop. Nothing this tool emits may.
//!
//! A LUT only passes on the inputs its truth table reads, so an input it
//! ignores — a buffer's two idle inputs, say — is not an edge here. That is
//! what keeps don't-care inputs left at their default select from closing a
//! cycle that no signal could ever travel.
//!
//! The check runs on the *final* configuration rather than on the routing, so
//! it also catches loops created by CLBs nobody routed through — the default
//! all-zero configuration drives `_op` onto the minor lanes and constants onto
//! the major ones, and a future fabric edit could make that default cyclic.
//! Reading the realised mux selects is the only way to be sure.
//!
//! Loop-around board wiring is included as a combinational edge. A wire from a
//! chip output back to a chip input is exactly that — a wire — so it breaks no
//! path, and a cycle closed through one oscillates in silicon just as surely as
//! one closed inside the fabric. Leaving it out of this graph would be the
//! easiest way to ship a design that does not work.

use crate::design::Design;
use crate::fabric::{BusOut, Fabric, FieldSlice, IoSource, Source};
use crate::rrg::Node;
use std::collections::BTreeMap;
use std::fmt;

/// A combinational cycle, as the sequence of nodes it runs through.
#[derive(Debug, Clone)]
pub struct Cycle {
    pub nodes: Vec<Node>,
}

impl fmt::Display for Cycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let path: Vec<String> = self.nodes.iter().map(|n| n.to_string()).collect();
        write!(f, "{} -> {}", path.join(" -> "), self.nodes[0])
    }
}

/// Read a mux select out of a configuration.
fn select(config: &Design, col: usize, row: usize, slice: FieldSlice) -> u64 {
    let raw = config.clb(col, row).get(slice.field);
    match slice.bit {
        Some(bit) => raw >> bit & 1,
        None => raw,
    }
}

/// Whether a LUT value's output changes with input mux `k`.
pub fn lut_depends_on(fabric: &Fabric, lut: u64, k: usize) -> bool {
    let n = fabric.input_muxes.len();
    (0..1usize << n).any(|minterm| {
        let mut values: Vec<bool> = (0..n).map(|b| minterm >> b & 1 != 0).collect();
        let before = fabric.eval_lut(lut, &values);
        values[k] = !values[k];
        before != fabric.eval_lut(lut, &values)
    })
}

/// The constant a right-edge lane is known to carry, found by following
/// pass-throughs back to a constant select or an all-zero LUT.
fn right_edge_constant(fabric: &Fabric, config: &Design, row: usize, lane: usize) -> Option<bool> {
    let (mut col, mut lane) = (fabric.columns, lane);
    while col > 0 {
        let cell = col - 1;
        let mux = fabric.output_mux(BusOut::Horz(lane))?;
        let code = select(config, cell, row, mux.select) as usize;
        match *mux.sources.get(code)? {
            Source::Const(v) => return Some(v),
            Source::Op if config.clb(cell, row).get(fabric.lut.select.field) == 0 => return Some(false),
            Source::HorzIn(k) => {
                lane = k;
                col = cell;
            }
            _ => return None,
        }
    }
    None
}

/// Build the realised combinational graph: what each mux is actually selecting.
///
/// The flip-flop is deliberately absent. `_op -> _reg` is the only edge that
/// crosses a clock boundary, so leaving it out is what makes a legitimate
/// registered feedback path legal while a combinational one is not.
fn combinational_edges(
    fabric: &Fabric,
    config: &Design,
    loopbacks: &[(Node, Node)],
) -> BTreeMap<Node, Vec<Node>> {
    let mut edges: BTreeMap<Node, Vec<Node>> = BTreeMap::new();
    let mut add = |from: Node, to: Node| edges.entry(from).or_default().push(to);

    // Board wiring: a chip output feeding a chip input, combinational.
    for &(source, sink) in loopbacks {
        add(source, sink);
    }

    // The IO controller: the right-edge lane each output-side pad reads, and
    // what each left-edge lane reads — a pad, or an output read back on-chip.
    for pad in 0..fabric.io_outputs.len() {
        let (row, lane) = config.pad_lane(fabric, pad);
        add(Node::HSeg { row, col: fabric.columns, lane }, Node::OutPad(pad));
    }
    for row in 0..fabric.rows {
        for lane in 0..fabric.horz_lanes {
            let from = match config.left_source(fabric, row, lane) {
                IoSource::InPad(pad) => Node::InPad(pad),
                IoSource::OutPad(pad) => Node::OutPad(pad),
            };
            add(from, Node::HSeg { row, col: 0, lane });
        }
    }
    // A DDIO pad in output mode reads its own output back, gated by its
    // direction. Only a direction held at a constant 0 rules that out.
    for d in &fabric.ddio {
        let (row, lane) = config.pad_lane(fabric, d.dir);
        if right_edge_constant(fabric, config, row, lane) == Some(false) {
            continue;
        }
        add(Node::OutPad(d.dir), Node::InPad(d.input));
        add(Node::OutPad(d.output), Node::InPad(d.input));
    }

    for col in 0..fabric.columns {
        for row in 0..fabric.rows {
            // Output muxes: exactly one source is live per lane.
            for mux in &fabric.output_muxes {
                let code = select(config, col, row, mux.select) as usize;
                let Some(source) = mux.sources.get(code) else { continue };
                let target = match mux.drives {
                    BusOut::Horz(lane) => Node::HSeg { row, col: col + 1, lane },
                    BusOut::Vert(lane) => {
                        Node::VSeg { col, row: (row + 1) % fabric.rows, lane }
                    }
                };
                let from = match *source {
                    Source::HorzIn(lane) => Node::HSeg { row, col, lane },
                    Source::VertIn(lane) => Node::VSeg { col, row, lane },
                    Source::Op => Node::Op { col, row },
                    // A registered value is not combinationally downstream of
                    // anything in this cycle, so it starts no path.
                    Source::Reg | Source::Const(_) => continue,
                    Source::Carry | Source::CarryIn | Source::VRing(_) => continue,
                };
                add(from, target);
            }

            // Input muxes, then the LUT and the carry adder, both of which
            // are combinational. The LUT passes on only the inputs it reads.
            let lut = config.clb(col, row).get(fabric.lut.select.field);
            for (index, mux) in fabric.input_muxes.iter().enumerate() {
                let code = select(config, col, row, mux.select) as usize;
                let Some(source) = mux.sources.get(code) else { continue };
                let pin = Node::In { col, row, mux: index };
                let from = match *source {
                    Source::HorzIn(lane) => Node::HSeg { row, col, lane },
                    Source::VertIn(lane) => Node::VSeg { col, row, lane },
                    Source::CarryIn => {
                        if row == 0 {
                            continue; // tied off
                        }
                        Node::Carry { col, row: row - 1 }
                    }
                    Source::Const(_) => continue,
                    Source::Op | Source::Reg | Source::Carry | Source::VRing(_) => continue,
                };
                add(from, pin);
                if lut_depends_on(fabric, lut, index) {
                    add(pin, Node::Op { col, row });
                }
                add(pin, Node::Carry { col, row });
            }
        }
    }
    edges
}

/// Every combinational cycle in a configuration. Empty means the design is
/// safe to emit.
pub fn find_cycles(fabric: &Fabric, config: &Design, loopbacks: &[(Node, Node)]) -> Vec<Cycle> {
    let edges = combinational_edges(fabric, config, loopbacks);

    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        New,
        Open,
        Done,
    }
    let nodes: Vec<Node> = {
        let mut all: Vec<Node> = edges.keys().copied().collect();
        for targets in edges.values() {
            all.extend(targets.iter().copied());
        }
        all.sort();
        all.dedup();
        all
    };
    let index: BTreeMap<Node, usize> =
        nodes.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    let mut mark = vec![Mark::New; nodes.len()];
    let mut cycles = Vec::new();

    // Iterative depth-first search, so a large fabric cannot overflow the
    // stack. `stack` holds the current path.
    for start in 0..nodes.len() {
        if mark[start] != Mark::New {
            continue;
        }
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        mark[start] = Mark::Open;
        while !stack.is_empty() {
            let top = stack.len() - 1;
            let (node, next) = stack[top];
            let successors = edges.get(&nodes[node]).map(|v| v.as_slice()).unwrap_or(&[]);
            if next >= successors.len() {
                mark[node] = Mark::Done;
                stack.pop();
                continue;
            }
            let target = successors[next];
            stack[top].1 += 1;
            let Some(&target_index) = index.get(&target) else { continue };
            match mark[target_index] {
                Mark::New => {
                    mark[target_index] = Mark::Open;
                    stack.push((target_index, 0));
                }
                Mark::Open => {
                    // Found a back edge: the path from `target` onwards is a
                    // cycle.
                    let at = stack.iter().position(|&(n, _)| n == target_index).unwrap_or(0);
                    cycles.push(Cycle {
                        nodes: stack[at..].iter().map(|&(n, _)| nodes[n]).collect(),
                    });
                }
                Mark::Done => {}
            }
        }
    }
    cycles
}

/// Vertical rings left in full pass-through with nothing driving them. Not a
/// combinational cycle in the logic sense, but a ring with no source: every
/// lane simply circulates whatever it happened to hold.
pub fn driverless_rings(fabric: &Fabric, config: &Design) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for col in 0..fabric.columns {
        for lane in 0..fabric.vert_lanes {
            // Follow the lane around the ring; if every hop is a
            // pass-through, nothing drives it.
            let mut current = lane;
            let mut all_pass = true;
            for row in 0..fabric.rows {
                let Some(mux) = fabric
                    .output_muxes
                    .iter()
                    .find(|m| m.drives == BusOut::Vert(current))
                else {
                    all_pass = false;
                    break;
                };
                let code = select(config, col, row, mux.select) as usize;
                match mux.sources.get(code) {
                    Some(Source::VertIn(next)) => current = *next,
                    _ => {
                        all_pass = false;
                        break;
                    }
                }
            }
            if all_pass {
                out.push((col, lane));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}
