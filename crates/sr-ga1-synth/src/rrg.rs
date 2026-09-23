//! The routing resource graph.
//!
//! Every node is a place a signal can be, every edge is a mux code that puts
//! it there, and each edge carries the configuration assignment it implies.
//! A routed path is therefore already a bitstream: there is no second
//! translation step that could disagree with the first.
//!
//! The graph is built entirely from `fabric.toml`'s mux tables. In
//! particular the vertical lane swap is never written down here — it falls
//! out of `v_out:0` listing `v_in:2` as its pass-through source.
//!
//! Three things are modelled deliberately rather than as a side effect:
//!
//! * **The vertical buses are closed rings.** `VSeg(col, 0, lane)` is the
//!   segment entering row 0, which is driven by the top row's output muxes —
//!   the same value the column's CSB taps. So the clock tap and the ring
//!   closure are one node, not two.
//! * **Lane 3 is both a wire and a register enable.** `HSeg(row, col, 3)` is
//!   the segment entering `CLB(col, row)`, and whatever occupies it *is*
//!   that CLB's write enable. Capacity one then resolves "route a signal
//!   past" against "hold this register enabled" automatically, instead of
//!   one silently breaking the other.
//! * **A vertical hop costs a CLB.** The only horizontal-to-vertical path is
//!   through a CLB's operation core, so the graph carries an explicit
//!   `In -> Op` edge at every CLB, priced accordingly. The router allocates
//!   its own buffers; without that, most designs on this fabric are simply
//!   unroutable.

use crate::constraints::LoopbackPool;
use crate::fabric::{BusOut, Fabric, FieldSlice, Source};
use std::collections::HashMap;

/// A place a signal can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Node {
    /// Horizontal segment entering `CLB(col, row)`. `col == columns` is the
    /// segment the output controller reads.
    HSeg { row: usize, col: usize, lane: usize },
    /// Vertical segment entering `CLB(col, row)`. Row 0's segment closes the
    /// ring and is what the CSB taps.
    VSeg { col: usize, row: usize, lane: usize },
    /// A CLB's combinational output.
    Op { col: usize, row: usize },
    /// A CLB's registered output.
    Reg { col: usize, row: usize },
    /// A CLB's carry output, which reaches nothing but the cell above.
    Carry { col: usize, row: usize },
    /// A CLB's operation input, by input mux index.
    In { col: usize, row: usize, mux: usize },
    Const(bool),
}

/// A configuration write an edge implies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Assign {
    pub col: usize,
    pub row: usize,
    pub slice: FieldSlice,
    pub value: u64,
}

/// What taking an edge costs beyond the wire itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// A lane continuing through a CLB, or a signal entering one.
    Wire,
    /// A CLB driving its own output onto a lane.
    Drive,
    /// Passing through a CLB's operation core: the only horizontal-to-
    /// vertical path there is, and it spends a CLB.
    Through,
    /// The dedicated carry chain.
    CarryChain,
    /// The flip-flop. Not a combinational edge, so it breaks loops.
    Register,
    /// A board wire from a chip output back to a chip input. Costs no
    /// configuration bits, spends no CLB, and — being a wire — breaks no
    /// combinational path, so a cycle through one is still a cycle.
    Loopback,
}

#[derive(Debug, Clone, Copy)]
pub struct Edge {
    pub to: usize,
    pub kind: EdgeKind,
    pub assign: Option<Assign>,
}

pub struct Rrg {
    pub nodes: Vec<Node>,
    index: HashMap<Node, usize>,
    pub out: Vec<Vec<Edge>>,
    pub columns: usize,
    pub rows: usize,
}

impl Rrg {
    pub fn id(&self, node: Node) -> Option<usize> {
        self.index.get(&node).copied()
    }

    /// Panics-free lookup used where the node is known to exist by
    /// construction; returns 0 only if the graph was built inconsistently,
    /// which the self-check below rules out.
    pub fn node(&self, id: usize) -> Node {
        self.nodes[id]
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn build(fabric: &Fabric) -> Rrg {
        Rrg::with_loopback(fabric, &LoopbackPool::default())
    }

    /// Build the graph, adding the board's loop-around wiring.
    ///
    /// An edge is emitted for every pool output paired with every pool input,
    /// and the router chooses which to use: one output pin can legitimately
    /// fan out to several input pins, while each input has exactly one driver,
    /// which the single-occupancy rule already enforces. The pairing that ends
    /// up being used is read back off the finished routing.
    pub fn with_loopback(fabric: &Fabric, pool: &LoopbackPool) -> Rrg {
        let mut nodes = Vec::new();
        let mut index = HashMap::new();
        let add = |node: Node, nodes: &mut Vec<Node>, index: &mut HashMap<Node, usize>| {
            if let std::collections::hash_map::Entry::Vacant(slot) = index.entry(node) {
                slot.insert(nodes.len());
                nodes.push(node);
            }
        };

        for row in 0..fabric.rows {
            for col in 0..=fabric.columns {
                for lane in 0..fabric.horz_lanes {
                    add(Node::HSeg { row, col, lane }, &mut nodes, &mut index);
                }
            }
        }
        for col in 0..fabric.columns {
            for row in 0..fabric.rows {
                for lane in 0..fabric.vert_lanes {
                    add(Node::VSeg { col, row, lane }, &mut nodes, &mut index);
                }
                add(Node::Op { col, row }, &mut nodes, &mut index);
                add(Node::Reg { col, row }, &mut nodes, &mut index);
                add(Node::Carry { col, row }, &mut nodes, &mut index);
                for mux in 0..fabric.input_muxes.len() {
                    add(Node::In { col, row, mux }, &mut nodes, &mut index);
                }
            }
        }
        add(Node::Const(false), &mut nodes, &mut index);
        add(Node::Const(true), &mut nodes, &mut index);

        let mut out: Vec<Vec<Edge>> = vec![Vec::new(); nodes.len()];
        let mut link = |from: Node, to: Node, kind: EdgeKind, assign: Option<Assign>| {
            let (Some(&f), Some(&t)) = (index.get(&from), index.get(&to)) else {
                return;
            };
            out[f].push(Edge { to: t, kind, assign });
        };

        for col in 0..fabric.columns {
            for row in 0..fabric.rows {
                // Output muxes: each source reaches the lane the mux drives.
                for mux in &fabric.output_muxes {
                    let target = match mux.drives {
                        BusOut::Horz(lane) => Node::HSeg { row, col: col + 1, lane },
                        BusOut::Vert(lane) => {
                            // The ring: the top row drives row 0's segment.
                            let next = (row + 1) % fabric.rows;
                            Node::VSeg { col, row: next, lane }
                        }
                    };
                    for (code, source) in mux.sources.iter().enumerate() {
                        let assign = Some(Assign {
                            col,
                            row,
                            slice: mux.select,
                            value: code as u64,
                        });
                        let (from, kind) = match *source {
                            Source::HorzIn(lane) => {
                                (Node::HSeg { row, col, lane }, EdgeKind::Wire)
                            }
                            Source::VertIn(lane) => {
                                (Node::VSeg { col, row, lane }, EdgeKind::Wire)
                            }
                            Source::Const(v) => (Node::Const(v), EdgeKind::Drive),
                            Source::Op => (Node::Op { col, row }, EdgeKind::Drive),
                            Source::Reg => (Node::Reg { col, row }, EdgeKind::Drive),
                            // Carry never reaches a bus, and carry_in is an
                            // input-side source only.
                            Source::Carry | Source::CarryIn | Source::VRing(_) => continue,
                        };
                        link(from, target, kind, assign);
                    }
                }

                // Input muxes: what each operation input can see.
                for (mux_index, mux) in fabric.input_muxes.iter().enumerate() {
                    let target = Node::In { col, row, mux: mux_index };
                    for (code, source) in mux.sources.iter().enumerate() {
                        let assign = Some(Assign {
                            col,
                            row,
                            slice: mux.select,
                            value: code as u64,
                        });
                        let (from, kind) = match *source {
                            Source::HorzIn(lane) => {
                                (Node::HSeg { row, col, lane }, EdgeKind::Wire)
                            }
                            Source::VertIn(lane) => {
                                (Node::VSeg { col, row, lane }, EdgeKind::Wire)
                            }
                            Source::Const(v) => (Node::Const(v), EdgeKind::Wire),
                            Source::CarryIn => {
                                if row == 0 {
                                    continue; // row 0's carry-in is tied off
                                }
                                (Node::Carry { col, row: row - 1 }, EdgeKind::CarryChain)
                            }
                            Source::Op | Source::Reg | Source::Carry | Source::VRing(_) => continue,
                        };
                        link(from, target, kind, assign);
                    }
                }

                // The operation core and the carry adder: combinational, and
                // the only way from a horizontal lane onto a vertical one.
                for mux_index in 0..fabric.input_muxes.len() {
                    link(
                        Node::In { col, row, mux: mux_index },
                        Node::Op { col, row },
                        EdgeKind::Through,
                        None,
                    );
                    link(
                        Node::In { col, row, mux: mux_index },
                        Node::Carry { col, row },
                        EdgeKind::Through,
                        None,
                    );
                }
                // The flip-flop, which is where combinational paths end.
                link(Node::Op { col, row }, Node::Reg { col, row }, EdgeKind::Register, None);
            }
        }

        for source in &pool.outputs {
            for sink in &pool.inputs {
                link(
                    Node::HSeg { row: source.row, col: fabric.columns, lane: source.lane },
                    Node::HSeg { row: sink.row, col: 0, lane: sink.lane },
                    EdgeKind::Loopback,
                    None,
                );
            }
        }

        Rrg { nodes, index, out, columns: fabric.columns, rows: fabric.rows }
    }

    /// The loop-around edges in the graph, as the pad nodes they join.
    pub fn loopback_edges(&self) -> Vec<(Node, Node)> {
        let mut out = Vec::new();
        for id in 0..self.len() {
            for edge in &self.out[id] {
                if edge.kind == EdgeKind::Loopback {
                    out.push((self.node(id), self.node(edge.to)));
                }
            }
        }
        out
    }

    /// The chip input pads, as the nodes they drive.
    pub fn input_pads(fabric: &Fabric) -> Vec<(String, Node)> {
        let mut out = Vec::new();
        for (row, lanes) in fabric.io_inputs.iter().enumerate() {
            for (lane, name) in lanes.iter().enumerate() {
                out.push((name.clone(), Node::HSeg { row, col: 0, lane }));
            }
        }
        out
    }

    /// The chip output pads, as the nodes they read.
    pub fn output_pads(fabric: &Fabric) -> Vec<(String, Node)> {
        let mut out = Vec::new();
        for (row, lanes) in fabric.io_outputs.iter().enumerate() {
            for (lane, name) in lanes.iter().enumerate() {
                if let Some(name) = name {
                    out.push((
                        name.clone(),
                        Node::HSeg { row, col: fabric.columns, lane },
                    ));
                }
            }
        }
        out
    }

    /// Where a column's CSB taps its ring, per select code.
    pub fn clock_taps(fabric: &Fabric, col: usize) -> Vec<(u64, Node)> {
        fabric
            .csb_clock
            .ring_lanes
            .iter()
            .enumerate()
            .map(|(code, &lane)| (code as u64, Node::VSeg { col, row: 0, lane }))
            .collect()
    }

    /// The segment that gates a CLB's register, which is also an ordinary
    /// routing resource. There is only one of these per CLB, by design.
    pub fn enable_node(fabric: &Fabric, col: usize, row: usize) -> Option<Node> {
        match fabric.ff.enable {
            Source::HorzIn(lane) => Some(Node::HSeg { row, col, lane }),
            Source::VertIn(lane) => Some(Node::VSeg { col, row, lane }),
            _ => None,
        }
    }
}

impl std::fmt::Display for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Node::HSeg { row, col, lane } => write!(f, "h[{}] row {} before col {}", lane, row, col),
            Node::VSeg { col, row, lane } => write!(f, "v[{}] col {} before row {}", lane, col, row),
            Node::Op { col, row } => write!(f, "CLB({}, {})._op", col, row),
            Node::Reg { col, row } => write!(f, "CLB({}, {})._reg", col, row),
            Node::Carry { col, row } => write!(f, "CLB({}, {})._carry", col, row),
            Node::In { col, row, mux } => write!(f, "CLB({}, {}) input {}", col, row, mux),
            Node::Const(v) => write!(f, "constant {}", u8::from(*v)),
        }
    }
}
