//! Loop checking, including through board wiring.
//!
//! A loop-around wire is combinational: it breaks no path. So a cycle closed
//! through one oscillates in silicon exactly as a cycle inside the fabric
//! would, and the checker has to see it. Forgetting that would be the easiest
//! way to ship a configuration that does not work, which is why it has its own
//! test rather than relying on the end-to-end runs.

use sr_ga1_synth::design::Design;
use sr_ga1_synth::fabric::Fabric;
use sr_ga1_synth::loops;
use sr_ga1_synth::rrg::Node;
use std::path::{Path, PathBuf};

fn fabric() -> Fabric {
    let path: PathBuf =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("fabric.toml");
    Fabric::load_file(&path).expect("the repository fabric.toml must load")
}

/// The all-default configuration must be acyclic, or every empty region of
/// every design would be a loop.
#[test]
fn the_default_configuration_is_loop_free() {
    let f = fabric();
    let config = Design::new(&f);
    let cycles = loops::find_cycles(&f, &config, &[]);
    assert!(
        cycles.is_empty(),
        "the all-zero configuration should be acyclic; found {}",
        cycles[0]
    );
}

/// With a wire from a chip output back to the chip input feeding the same row,
/// the default configuration *does* close a combinational loop: every CLB drives
/// its own `_op` onto minor lane 0, and input `a` reads it back.
#[test]
fn a_combinational_loop_through_board_wiring_is_caught() {
    let f = fabric();
    let config = Design::new(&f);
    let out = Node::HSeg { row: 0, col: f.columns, lane: 0 };
    let back = Node::HSeg { row: 0, col: 0, lane: 0 };

    let cycles = loops::find_cycles(&f, &config, &[(out, back)]);
    assert!(
        !cycles.is_empty(),
        "wiring output_0 back to input_0 closes a combinational path through row 0, \
         and the checker must refuse it"
    );
    // The cycle must actually run through the two pad nodes.
    let touches_pads = cycles
        .iter()
        .any(|c| c.nodes.contains(&out) && c.nodes.contains(&back));
    assert!(touches_pads, "the reported cycle should name the board wire: {}", cycles[0]);
}

/// A register in the path is what makes a legal feedback loop legal: the
/// flip-flop is the one edge the combinational graph leaves out.
#[test]
fn a_registered_path_through_board_wiring_is_not_a_loop() {
    let f = fabric();
    let mut config = Design::new(&f);

    // CLB(0,0) drives its registered output onto major lane 2 instead of its
    // combinational one, so the path out to the pad crosses the flip-flop.
    let major = f
        .clb_fields
        .iter()
        .position(|field| field.name == "major_horz2_sel")
        .expect("the fabric has a major lane 2 select");
    let reg_code = f
        .output_mux(sr_ga1_synth::fabric::BusOut::Horz(2))
        .and_then(|mux| {
            mux.sources.iter().position(|s| *s == sr_ga1_synth::fabric::Source::Reg)
        })
        .expect("major lane 2 can carry the registered output");
    config
        .set_clb(&f, 0, 0, &f.clb_fields[major].name, reg_code as u64)
        .expect("setting a mux select");

    // Break the minor-lane path that the previous test relies on, so the only
    // route from this cell to the right edge is the registered one.
    for col in 0..f.columns {
        config
            .set_clb(&f, col, 0, "minor_horz_sel", 0b01)
            .expect("lane 0 passes through instead of taking _op");
    }

    let out = Node::HSeg { row: 0, col: f.columns, lane: 2 };
    let back = Node::HSeg { row: 0, col: 0, lane: 0 };
    let cycles = loops::find_cycles(&f, &config, &[(out, back)]);
    assert!(
        cycles.is_empty(),
        "a path that crosses the flip-flop is sequential, not combinational; found {}",
        cycles.first().map(|c| c.to_string()).unwrap_or_default()
    );
}
