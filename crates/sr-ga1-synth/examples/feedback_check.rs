//! Can a register's output ever reach an input of a cell that could recompute
//! it? Sequential logic with state needs Q -> next-state logic -> D, and on
//! this fabric D is hard-wired to the cell's own LUT result, so the question
//! is whether `_reg` can get back to the inputs of its own CLB — inside the
//! fabric alone, and with the IO controller reading chip outputs back.
//!
//! This walks the real routing graph rather than reasoning about the tables by
//! hand.

use sr_ga1_synth::fabric::Fabric;
use sr_ga1_synth::rrg::{EdgeKind, Node, Rrg};
use std::collections::VecDeque;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "fabric.toml".into());
    let fabric = Fabric::load_file(std::path::Path::new(&path)).expect("fabric loads");
    let graph = Rrg::build(&fabric);

    // Reachability ignoring capacity: if a path does not exist here, no
    // placement or routing effort can invent one.
    let reachable = |start: Node, through_io: bool| -> Vec<Node> {
        let Some(start) = graph.id(start) else { return Vec::new() };
        let mut seen = vec![false; graph.len()];
        let mut queue = VecDeque::new();
        seen[start] = true;
        queue.push_back(start);
        let mut out = Vec::new();
        while let Some(node) = queue.pop_front() {
            for edge in &graph.out[node] {
                // The flip-flop is the boundary we are asking about, so do not
                // cross another one.
                if edge.kind == EdgeKind::Register {
                    continue;
                }
                if !through_io && edge.kind == EdgeKind::OnChipLoop {
                    continue;
                }
                if !seen[edge.to] {
                    seen[edge.to] = true;
                    out.push(graph.node(edge.to));
                    queue.push_back(edge.to);
                }
            }
        }
        out
    };

    println!("fabric {} ({}x{})", fabric.name, fabric.columns, fabric.rows);
    println!();

    for (through_io, label) in [(false, "inside the fabric"), (true, "with the IO controller's on-chip loops")] {
        let mut cells = 0;
        for col in 0..fabric.columns {
            for row in 0..fabric.rows {
                let own = reachable(Node::Reg { col, row }, through_io)
                    .iter()
                    .any(|n| matches!(n, Node::In { col: c, row: r, .. } if *c == col && *r == row));
                if own {
                    cells += 1;
                }
            }
        }
        println!(
            "{}: {} of {} registers can reach an input of their own CLB",
            label,
            cells,
            fabric.columns * fabric.rows
        );
    }

    // Which vertical lanes each output can occupy, and which the logic reads.
    println!();
    println!("vertical lanes carrying _op:  {:?}", fabric.vert_lanes_for(sr_ga1_synth::fabric::Source::Op));
    println!("vertical lanes carrying _reg: {:?}", fabric.vert_lanes_for(sr_ga1_synth::fabric::Source::Reg));
    let c = fabric.input_mux("c").expect("input c");
    println!("input c reads: {:?}", c.sources);
}
