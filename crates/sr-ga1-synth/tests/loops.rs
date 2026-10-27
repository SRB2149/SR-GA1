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

fn pad_in(f: &Fabric, name: &str) -> Node {
    Node::InPad(f.input_pad(name).expect(name))
}

fn pad_out(f: &Fabric, name: &str) -> Node {
    Node::OutPad(f.output_pad(name).expect(name))
}

/// Every CLB in row 0 buffers its input `a`, which reads lane 0.
fn buffered_row0(f: &Fabric) -> Design {
    let mut config = Design::new(f);
    let buf_a = f.lut_value(|v| v[0]);
    for col in 0..f.columns {
        config.set_clb(f, col, 0, "lut", buf_a).expect("lut");
    }
    config
}

/// With a wire from a chip output back to the chip input feeding the same row,
/// row 0 closes a combinational loop: every CLB drives its own `_op` onto minor
/// lane 0, its LUT copies input `a`, and input `a` reads lane 0.
#[test]
fn a_combinational_loop_through_board_wiring_is_caught() {
    let f = fabric();
    let config = buffered_row0(&f);
    // The all-zero IO configuration: output_0 reads row 0 lane 0, and row 0
    // lane 0 reads input_0.
    let (out, back) = (pad_out(&f, "output_0"), pad_in(&f, "input_0"));

    let cycles = loops::find_cycles(&f, &config, &[(out, back)]);
    assert!(
        !cycles.is_empty(),
        "wiring output_0 back to input_0 closes a combinational path through row 0,          and the checker must refuse it"
    );
    // The cycle must actually run through the two pad nodes.
    let touches_pads = cycles
        .iter()
        .any(|c| c.nodes.contains(&out) && c.nodes.contains(&back));
    assert!(touches_pads, "the reported cycle should name the board wire: {}", cycles[0]);
}

/// The same loop closed by the IO controller instead of the board.
#[test]
fn a_combinational_loop_through_an_on_chip_loop_is_caught() {
    let f = fabric();
    let mut config = buffered_row0(&f);
    let output_0 = f.io_mux_sources.iter().position(|s| {
        *s == sr_ga1_synth::fabric::IoSource::OutPad(f.output_pad("output_0").unwrap())
    });
    config.set_io(&f, "input_mux_sel0a", output_0.expect("output_0 is readable") as u64).unwrap();
    let cycles = loops::find_cycles(&f, &config, &[]);
    assert!(!cycles.is_empty(), "row 0 feeds itself through output_0");
    assert!(cycles.iter().any(|c| c.nodes.contains(&pad_out(&f, "output_0"))));
}

/// A LUT that ignores the input the loop returns on cannot oscillate through
/// it, so it is not a cycle.
#[test]
fn an_ignored_input_closes_no_loop() {
    let f = fabric();
    let mut config = Design::new(&f);
    // Input c reads lane 2, which the loop never reaches (it returns on
    // lanes 0 and 1); a and b read the loop but the LUT ignores them.
    let buf_c = f.lut_value(|v| v[2]);
    for col in 0..f.columns {
        config.set_clb(&f, col, 0, "lut", buf_c).expect("lut");
    }
    let cycles =
        loops::find_cycles(&f, &config, &[(pad_out(&f, "output_0"), pad_in(&f, "input_0"))]);
    assert!(cycles.is_empty(), "the looped lanes are read by nothing: {}", cycles.first().map(|c| c.to_string()).unwrap_or_default());
}

/// A register in the path is what makes a legal feedback loop legal: the
/// flip-flop is the one edge the combinational graph leaves out.
#[test]
fn a_registered_path_through_board_wiring_is_not_a_loop() {
    let f = fabric();
    let mut config = buffered_row0(&f);

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
    // The rest of row 0 passes lane 2 along, and output_2 reads it.
    let pass = f
        .output_mux(sr_ga1_synth::fabric::BusOut::Horz(2))
        .and_then(|mux| mux.sources.iter().position(|s| *s == sr_ga1_synth::fabric::Source::HorzIn(2)))
        .expect("lane 2 passes through");
    for col in 1..f.columns {
        config.set_clb(&f, col, 0, &f.clb_fields[major].name, pass as u64).expect("pass");
    }
    config.set_io(&f, "output_mux_sel2", 2).expect("output_2 reads row 0 lane 2");

    let cycles = loops::find_cycles(
        &f,
        &config,
        &[(pad_out(&f, "output_2"), pad_in(&f, "input_0"))],
    );
    assert!(
        cycles.is_empty(),
        "a path that crosses the flip-flop is sequential, not combinational; found {}",
        cycles.first().map(|c| c.to_string()).unwrap_or_default()
    );
}
