//! The routing graph is the piece everything else trusts, so these check it
//! against the architectural facts directly, not against itself.

use sr_ga1_synth::fabric::Fabric;
use sr_ga1_synth::rrg::{EdgeKind, Node, Rrg};
use std::path::{Path, PathBuf};

fn fabric() -> Fabric {
    let path: PathBuf =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("fabric.toml");
    Fabric::load_file(&path).expect("the repository fabric.toml must load")
}

/// Nodes reachable in one hop.
fn successors(g: &Rrg, from: Node) -> Vec<Node> {
    let id = g.id(from).unwrap_or_else(|| panic!("{} is not a node", from));
    let mut out: Vec<Node> = g.out[id].iter().map(|e| g.node(e.to)).collect();
    out.sort();
    out.dedup();
    out
}

fn reaches(g: &Rrg, from: Node, to: Node) -> bool {
    successors(g, from).contains(&to)
}

#[test]
fn op_and_reg_reach_exactly_the_documented_lanes() {
    let f = fabric();
    let g = Rrg::build(&f);
    let (col, row) = (2, 1);

    // _op: horizontal 0..3 and vertical 0, 2.
    for lane in 0..4 {
        assert!(
            reaches(&g, Node::Op { col, row }, Node::HSeg { row, col: col + 1, lane }),
            "_op should reach horizontal lane {}",
            lane
        );
    }
    for lane in [0, 2] {
        assert!(
            reaches(&g, Node::Op { col, row }, Node::VSeg { col, row: row + 1, lane }),
            "_op should reach vertical lane {}",
            lane
        );
    }
    for lane in [1, 3] {
        assert!(
            !reaches(&g, Node::Op { col, row }, Node::VSeg { col, row: row + 1, lane }),
            "vertical lane {} carries _reg, not _op",
            lane
        );
    }

    // _reg: vertical 1, 3 and the two major horizontal lanes only.
    for lane in [1, 3] {
        assert!(reaches(&g, Node::Reg { col, row }, Node::VSeg { col, row: row + 1, lane }));
    }
    for lane in [0, 2] {
        assert!(!reaches(&g, Node::Reg { col, row }, Node::VSeg { col, row: row + 1, lane }));
    }
    for lane in [2, 3] {
        assert!(reaches(&g, Node::Reg { col, row }, Node::HSeg { row, col: col + 1, lane }));
    }
    for lane in [0, 1] {
        assert!(
            !reaches(&g, Node::Reg { col, row }, Node::HSeg { row, col: col + 1, lane }),
            "minor horizontal lane {} carries only _op or a pass-through",
            lane
        );
    }
}

#[test]
fn carry_reaches_nothing_but_the_cell_above() {
    let f = fabric();
    let g = Rrg::build(&f);
    let (col, row) = (3, 1);
    let successors = successors(&g, Node::Carry { col, row });
    assert_eq!(
        successors,
        vec![Node::In { col, row: row + 1, mux: 2 }],
        "carry must leave only on the dedicated chain, into input c above"
    );
}

#[test]
fn row_zero_has_no_carry_in() {
    let f = fabric();
    let g = Rrg::build(&f);
    // Nothing feeds input c of row 0 by the carry chain.
    let target = Node::In { col: 1, row: 0, mux: 2 };
    for id in 0..g.len() {
        for edge in &g.out[id] {
            if g.node(edge.to) == target {
                assert_ne!(
                    edge.kind,
                    EdgeKind::CarryChain,
                    "row 0's carry input is tied off, but {} feeds it",
                    g.node(id)
                );
            }
        }
    }
}

#[test]
fn the_vertical_ring_closes_and_swaps_lanes() {
    let f = fabric();
    let g = Rrg::build(&f);
    let col = 4;
    let top = f.rows - 1;

    // The top row's vertical outputs land on row 0 — the ring closure, and
    // the same node the CSB taps.
    assert!(
        reaches(&g, Node::Op { col, row: top }, Node::VSeg { col, row: 0, lane: 0 }),
        "the ring must close from the top row back into row 0"
    );
    assert_eq!(
        Rrg::clock_taps(&f, col)[0].1,
        Node::VSeg { col, row: 0, lane: 0 },
        "the CSB taps the ring at its loop point"
    );

    // Pass-through swaps 0<->2 and 1<->3 at every hop.
    for (from, to) in [(0usize, 2usize), (2, 0), (1, 3), (3, 1)] {
        assert!(
            reaches(&g, Node::VSeg { col, row: 1, lane: from }, Node::VSeg { col, row: 2, lane: to }),
            "vertical lane {} should continue on lane {}",
            from,
            to
        );
        assert!(
            !reaches(&g, Node::VSeg { col, row: 1, lane: from }, Node::VSeg { col, row: 2, lane: from }),
            "vertical lane {} must not continue on its own lane",
            from
        );
    }
}

#[test]
fn horizontal_lanes_cannot_reach_a_vertical_lane_without_a_clb() {
    let f = fabric();
    let g = Rrg::build(&f);
    let (col, row) = (2, 2);
    for lane in 0..f.horz_lanes {
        for target in 0..f.vert_lanes {
            assert!(
                !reaches(
                    &g,
                    Node::HSeg { row, col, lane },
                    Node::VSeg { col, row: row + 1, lane: target }
                ),
                "a horizontal lane must go through the operation core to reach a vertical one"
            );
        }
    }
    // And the path that does exist is marked as costing a CLB.
    let id = g.id(Node::In { col, row, mux: 0 }).expect("input node");
    let through = g.out[id]
        .iter()
        .find(|e| g.node(e.to) == Node::Op { col, row })
        .expect("input reaches the operation core");
    assert_eq!(through.kind, EdgeKind::Through);
}

#[test]
fn input_windows_match_the_fabric() {
    let f = fabric();
    let g = Rrg::build(&f);
    let (col, row) = (5, 2);

    // a sees h0 and h1 only.
    for lane in 0..4 {
        let seen = reaches(&g, Node::HSeg { row, col, lane }, Node::In { col, row, mux: 0 });
        assert_eq!(seen, lane < 2, "input a and horizontal lane {}", lane);
    }
    // b sees h1 and h2.
    for lane in 0..4 {
        let seen = reaches(&g, Node::HSeg { row, col, lane }, Node::In { col, row, mux: 1 });
        assert_eq!(seen, lane == 1 || lane == 2, "input b and horizontal lane {}", lane);
    }
    // c sees h2, h3, vertical lane 0 and the carry chain.
    for lane in 0..4 {
        let seen = reaches(&g, Node::HSeg { row, col, lane }, Node::In { col, row, mux: 2 });
        assert_eq!(seen, lane >= 2, "input c and horizontal lane {}", lane);
    }
    assert!(reaches(&g, Node::VSeg { col, row, lane: 0 }, Node::In { col, row, mux: 2 }));
    for lane in 1..4 {
        assert!(!reaches(&g, Node::VSeg { col, row, lane }, Node::In { col, row, mux: 2 }));
    }
}

#[test]
fn the_enable_is_an_ordinary_routing_node() {
    let f = fabric();
    let g = Rrg::build(&f);
    // Lane 3's segment entering a CLB is that CLB's write enable, and it is
    // the same node any signal routed along lane 3 would occupy.
    let enable = Rrg::enable_node(&f, 3, 1).expect("the flip-flop has an enable");
    assert_eq!(enable, Node::HSeg { row: 1, col: 3, lane: 3 });
    assert!(g.id(enable).is_some(), "the enable must be a real routing node");

    // Row 3's lane 3 enters as a constant 1, which is why unconditional
    // registers are cheapest there.
    assert_eq!(f.io_constant(3, 3), Some(true));
    assert_eq!(f.io_constant(3, 1), Some(true));
    assert_eq!(f.io_constant(3, 0), Some(false));
    assert_eq!(f.io_constant(0, 3), None, "row 0's lane 3 is a chip input");
}

#[test]
fn constants_reach_only_the_major_lanes() {
    let f = fabric();
    let g = Rrg::build(&f);
    let (col, row) = (1, 1);
    for lane in 0..f.horz_lanes {
        let seen = reaches(&g, Node::Const(true), Node::HSeg { row, col: col + 1, lane });
        assert_eq!(
            seen,
            lane >= 2,
            "constants come from the major muxes only; lane {} disagrees",
            lane
        );
    }
    assert_eq!(f.const_capable_horz_lanes(), vec![2, 3]);
}

#[test]
fn the_register_edge_is_the_only_non_combinational_one() {
    let f = fabric();
    let g = Rrg::build(&f);
    let id = g.id(Node::Op { col: 0, row: 0 }).expect("op node");
    let to_reg = g.out[id]
        .iter()
        .find(|e| g.node(e.to) == Node::Reg { col: 0, row: 0 })
        .expect("the flip-flop follows the operation result");
    assert_eq!(to_reg.kind, EdgeKind::Register);
    assert!(to_reg.assign.is_none(), "the flip-flop is hard-wired, not configured");
}

/// A declared loop-around pool becomes graph edges from output pads to input
/// pads, one per combination, carrying no configuration at all.
#[test]
fn loop_around_wiring_becomes_routing_edges() {
    let f = fabric();
    let pool = sr_ga1_synth::constraints::Constraints::load_str(
        "[loopback]
outputs = [\"output_2\", \"output_6\"]
inputs = [\"input_0\"]
",
        Path::new("test.toml"),
        &f,
    )
    .expect("the pool should parse")
    .loopback;

    let plain = Rrg::build(&f);
    assert!(plain.loopback_edges().is_empty(), "no pool means no board wiring");

    let g = Rrg::with_loopback(&f, &pool);
    let edges = g.loopback_edges();
    assert_eq!(edges.len(), 2, "two outputs times one input");
    // output_2 is row 0 lane 2; input_0 is row 0 lane 0.
    assert!(edges.contains(&(
        Node::HSeg { row: 0, col: f.columns, lane: 2 },
        Node::HSeg { row: 0, col: 0, lane: 0 }
    )));
    // output_6 is row 1 lane 2, so this edge also changes row - which is the
    // other thing loop-around wiring buys.
    assert!(edges.contains(&(
        Node::HSeg { row: 1, col: f.columns, lane: 2 },
        Node::HSeg { row: 0, col: 0, lane: 0 }
    )));
}

/// A pool that names a pad the fabric does not have, or only one end, is a
/// mistake worth reporting rather than ignoring.
#[test]
fn a_malformed_loopback_pool_is_rejected() {
    let f = fabric();
    let parse = |text: &str| {
        sr_ga1_synth::constraints::Constraints::load_str(text, Path::new("test.toml"), &f)
    };
    assert!(parse("[loopback]
outputs = [\"nonsense\"]
inputs = [\"input_0\"]
").is_err());
    assert!(parse("[loopback]
outputs = [\"input_0\"]
inputs = [\"input_1\"]
").is_err());
    assert!(parse("[loopback]
outputs = [\"output_2\"]
").is_err(), "one end is not a loop");
    // DDIO pads need an explicit opt-in, since their direction is steered.
    assert!(parse("[loopback]
outputs = [\"ddio_out_0\"]
inputs = [\"input_0\"]
").is_err());
    // Pin locks are validated against the fabric here, and against the design
    // later: a pad that does not exist is caught immediately.
    assert!(parse("[pins]
clk = \"input_0\"
").is_ok());
    assert!(parse("[pins]
clk = \"nonexistent\"
").is_err());
    // Two signals cannot share one pad.
    assert!(parse("[pins]
a = \"input_0\"
b = \"input_0\"
").is_err());
    // Positions must be inside the grid, and written "col,row".
    assert!(parse("[placement]
foo = \"2,1\"
").is_ok());
    assert!(parse("[placement]
foo = \"99,1\"
").is_err());
    assert!(parse("[placement]
foo = \"nonsense\"
").is_err());
    // Two cells cannot share one CLB.
    assert!(parse("[placement]
a = \"2,1\"
b = \"2,1\"
").is_err());
    // A clock must name a column that exists, and one CSB sources one domain.
    assert!(parse("[clocks]
clk = 3
").is_ok());
    assert!(parse("[clocks]
clk = 99
").is_err());
    assert!(parse("[clocks]
a = 3
b = 3
").is_err());
    // A pad cannot be both loop-driven and locked to a design signal.
    assert!(parse(
        "[loopback]
outputs = [\"output_2\"]
inputs = [\"input_0\"]
         [pins]
clk = \"input_0\"
"
    )
    .is_err());
    // A typo'd section name is caught rather than ignored.
    assert!(parse("[pinz]
clk = \"input_0\"
").is_err());
    // DDIO pads are numbered, and the number must exist.
    assert!(parse("[[ddio]]
pin = 9
in = \"a\"
out = \"b\"
dir = \"c\"
").is_err());
}

#[test]
fn pads_sit_at_the_documented_positions() {
    let f = fabric();
    let inputs = Rrg::input_pads(&f);
    let outputs = Rrg::output_pads(&f);
    assert!(inputs.contains(&("input_0".into(), Node::HSeg { row: 0, col: 0, lane: 0 })));
    assert!(inputs.contains(&("input_9".into(), Node::HSeg { row: 2, col: 0, lane: 1 })));
    assert!(inputs.contains(&("ddio_in_0".into(), Node::HSeg { row: 2, col: 0, lane: 2 })));
    assert!(outputs.contains(&("output_0".into(), Node::HSeg { row: 0, col: f.columns, lane: 0 })));
    assert!(outputs.contains(&("ddio_dir_1".into(), Node::HSeg { row: 3, col: f.columns, lane: 1 })));
    // Row 3 lanes 2 and 3 are unused on the output side.
    assert!(!outputs.iter().any(|(_, n)| *n == Node::HSeg { row: 3, col: f.columns, lane: 3 }));
}

#[test]
fn every_edge_names_a_configuration_write_unless_it_is_hard_wired() {
    let f = fabric();
    let g = Rrg::build(&f);
    for id in 0..g.len() {
        for edge in &g.out[id] {
            match edge.kind {
                // Wiring decisions are configured.
                EdgeKind::Wire | EdgeKind::Drive | EdgeKind::CarryChain => assert!(
                    edge.assign.is_some(),
                    "{} -> {} changes routing but writes no configuration",
                    g.node(id),
                    g.node(edge.to)
                ),
                // The operation core, the flip-flop and board wiring are not.
                EdgeKind::Through | EdgeKind::Register | EdgeKind::Loopback => {
                    assert!(edge.assign.is_none())
                }
            }
            if let Some(assign) = edge.assign {
                assert!(assign.col < f.columns && assign.row < f.rows);
                assert!(
                    assign.value < 1 << assign.slice.width,
                    "{} -> {} writes {} into {} select bits",
                    g.node(id),
                    g.node(edge.to),
                    assign.value,
                    assign.slice.width
                );
            }
        }
    }
}
