//! Chip pad names and loop-around board wiring.
//!
//! A pad name is a display alias: an *input* pad is where a net begins, so
//! naming it renames that net, while naming an *output* pad only labels the pad,
//! because the net arriving there is named after whatever drives it. Neither
//! touches stimulus keys, trace ids or the bitstream.
//!
//! A loop is a wire from a chip output back to a chip input. It costs no
//! configuration, it is the only way a signal can return to the cell that
//! produced it, and — being a wire — it breaks no combinational path, so a cycle
//! closed through one oscillates exactly as one inside the fabric would.

use fpga_core::config::{BlockId, Design, IoPad};
use fpga_core::designfile::{load_design, save_design, DesignFile};
use fpga_core::drc::{self, Severity};
use fpga_core::fabric::Fabric;
use fpga_core::naming::{resolve, Namer};
use fpga_core::sim::{SimError, SimState, Stimulus};

fn fabric() -> Fabric {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fabric.toml");
    Fabric::load_file(std::path::Path::new(path)).expect("fabric.toml must load")
}

/// Pass row 0 lane 0 straight through the fabric to the right edge: every CLB in
/// the row continues lane 0 rather than driving its own result onto it.
fn passthrough_row0(fabric: &Fabric) -> Design {
    let mut design = Design::new(fabric);
    for col in 0..fabric.columns {
        design
            .set_clb(fabric, col, 0, "minor_horz_sel", 0b01)
            .expect("lane 0 passes through");
    }
    design
}

#[test]
fn naming_an_input_pad_renames_its_net() {
    let f = fabric();
    let mut design = passthrough_row0(&f);
    let pad = IoPad::input(0, 0);

    let before = {
        let nets = resolve(&f, &design);
        Namer::new(&f, &design).net_name(nets.horz_edge(0, 0))
    };
    assert_eq!(before, "input_0", "the reserved name before renaming");

    design.rename_io(&f, pad, "clk").expect("input_0 may be named");
    let nets = resolve(&f, &design);
    let namer = Namer::new(&f, &design);
    assert_eq!(
        namer.net_name(nets.horz_edge(0, 0)),
        "clk",
        "the net originates at the pad, so every segment carrying it follows"
    );
    assert_eq!(namer.net_name(nets.horz_in(3, 0, 0)), "clk");

    design.revert_io_name(pad);
    let nets = resolve(&f, &design);
    assert_eq!(Namer::new(&f, &design).net_name(nets.horz_edge(0, 0)), "input_0");
}

#[test]
fn naming_an_output_pad_does_not_rename_any_net() {
    let f = fabric();
    let mut design = passthrough_row0(&f);
    design
        .rename_io(&f, IoPad::output(0, 0), "result")
        .expect("output_0 may be named");

    let nets = resolve(&f, &design);
    // The net arriving at the right edge still comes from the input pad.
    assert_eq!(Namer::new(&f, &design).net_name(nets.horz_edge(0, 0)), "input_0");
    assert_eq!(
        design.effective_io_name(&f, IoPad::output(0, 0)).as_deref(),
        Some("result"),
        "the pad itself displays the design's name"
    );
}

/// A block and a pad may share a name, because a block is not a net: its nets
/// are the suffixed forms. A cell called `count[0]` and the pad `count[0]` leaves
/// on are the same signal seen at two places, and the synthesiser names both —
/// rejecting that would throw the names away on load.
#[test]
fn a_block_and_a_pad_may_carry_the_same_name() {
    let f = fabric();
    let mut design = Design::new(&f);

    design.rename(&f, BlockId::Clb { col: 0, row: 0 }, "count[0]").expect("block");
    design
        .rename_io(&f, IoPad::output(0, 0), "count[0]")
        .expect("the pad the cell's result leaves on may share its name");

    // And in the other order.
    design.rename_io(&f, IoPad::output(0, 1), "q").expect("pad");
    design.rename(&f, BlockId::Clb { col: 1, row: 0 }, "q").expect("block");
}

/// What a pad name must still avoid: another pad, a reserved name, and any net a
/// block implies.
#[test]
fn a_pad_name_must_not_collide_with_another_net() {
    let f = fabric();
    let mut design = Design::new(&f);

    // Another pad's name.
    design.rename_io(&f, IoPad::input(0, 0), "mine").expect("first pad");
    let err = design.rename_io(&f, IoPad::input(0, 1), "mine").unwrap_err();
    assert!(err.to_string().contains("already in use"), "{}", err);

    // A reserved pad name belonging to a different pad.
    let err = design.rename_io(&f, IoPad::input(0, 1), "input_4").unwrap_err();
    assert!(err.to_string().contains("already in use"), "{}", err);

    // A net a block implies. CLB0_0's combinational output is CLB0_0_op.
    let derived = format!("CLB0_0{}", f.naming.suffix_op);
    let err = design.rename_io(&f, IoPad::input(0, 1), &derived).unwrap_err();
    assert!(err.to_string().contains("already in use"), "{}", err);

    // Renaming a pad to what it already displays is not a collision.
    design.rename_io(&f, IoPad::input(0, 2), "input_2").expect("idempotent rename");
}

/// The mirror: a block cannot take a name whose derived net would collide with a
/// pad's net.
#[test]
fn a_blocks_derived_nets_must_not_collide_with_a_pad() {
    let f = fabric();
    let mut design = Design::new(&f);
    // A pad named exactly what CLB0_0's output net would be called if the block
    // were renamed to "sum".
    let pad_name = format!("sum{}", f.naming.suffix_op);
    design.rename_io(&f, IoPad::input(0, 0), &pad_name).expect("pad");

    let err = design.rename(&f, BlockId::Clb { col: 0, row: 0 }, "sum").unwrap_err();
    assert!(
        err.to_string().contains("collide"),
        "the block's output net would clash with the pad: {}",
        err
    );
}

#[test]
fn a_constant_pad_cannot_be_named_or_driven() {
    let f = fabric();
    let mut design = Design::new(&f);
    // Row 3's lanes are tied to constants by the input controller.
    let constant = (0..f.horz_lanes)
        .map(|lane| IoPad::input(3, lane))
        .find(|pad| pad.is_constant(&f))
        .expect("row 3 supplies constants");

    assert!(design.rename_io(&f, constant, "nope").is_err());
    assert!(design.set_loopback(&f, IoPad::output(0, 0), constant).is_err());
}

#[test]
fn a_loop_runs_from_an_output_to_an_input() {
    let f = fabric();
    let mut design = Design::new(&f);
    let out = IoPad::output(0, 0);
    let inp = IoPad::input(1, 0);

    // The wrong way round is refused.
    assert!(design.set_loopback(&f, inp, out).is_err());

    design.set_loopback(&f, out, inp).expect("output to input");
    assert_eq!(design.loopback_driver(inp), Some(out));
    assert_eq!(design.loopback_targets(out), vec![inp]);

    // One output may fan out to several inputs.
    let second = IoPad::input(2, 0);
    design.set_loopback(&f, out, second).expect("fan out");
    assert_eq!(design.loopback_targets(out).len(), 2);

    // An input has exactly one driver, so a second wire into it replaces the
    // first rather than adding to it.
    let other = IoPad::output(0, 1);
    design.set_loopback(&f, other, inp).expect("replace the driver");
    assert_eq!(design.loopback_driver(inp), Some(other));
    assert_eq!(design.loopback().len(), 2);

    design.clear_loopback(inp);
    assert_eq!(design.loopback_driver(inp), None);
}

#[test]
fn the_simulator_carries_a_value_round_a_loop() {
    let f = fabric();
    let mut design = passthrough_row0(&f);
    // Row 1 also passes lane 0 through, so whatever arrives at its left edge
    // reaches its right edge.
    for col in 0..f.columns {
        design.set_clb(&f, col, 1, "minor_horz_sel", 0b01).expect("pass through");
    }
    // Wire row 0's output back into row 1's input.
    design
        .set_loopback(&f, IoPad::output(0, 0), IoPad::input(1, 0))
        .expect("loop");

    let mut stimulus = Stimulus::default();
    stimulus.set("input_0", vec![true]);
    let (_state, settled) =
        SimState::new(&f, &design, &stimulus).expect("a feed-forward loop settles");

    assert!(settled.horz_edge(0, 0), "row 0 carries the stimulus out");
    assert!(
        settled.horz_in(0, 1, 0),
        "the board wire brings it back into row 1, so row 1's left edge is high"
    );
    assert!(settled.horz_edge(1, 0), "and row 1 passes it through to its own output");
}

#[test]
fn a_combinational_loop_through_board_wiring_is_detected_not_hung_on() {
    let f = fabric();
    let mut design = Design::new(&f);
    // Every CLB in row 0 drives its own result onto lane 0 (the default), and
    // input a reads lane 0, so tying the row's output back to its input closes a
    // path through four operation cores.
    design
        .set_loopback(&f, IoPad::output(0, 0), IoPad::input(0, 0))
        .expect("loop");
    // NOR3 inverts, so the loop cannot settle.
    for col in 0..f.columns {
        design.set_clb(&f, col, 0, "operation_select", 4).expect("NOR3");
    }

    let stimulus = Stimulus::default();
    match SimState::new(&f, &design, &stimulus) {
        Err(SimError::CombinationalLoop { blocks }) => {
            assert!(!blocks.is_empty(), "the report should name the cells involved");
        }
        Ok(_) => panic!("an inverting loop through board wiring cannot settle"),
    }
}

#[test]
fn drc_reports_a_loop_through_board_wiring() {
    let f = fabric();
    let mut design = Design::new(&f);
    design
        .set_loopback(&f, IoPad::output(0, 0), IoPad::input(0, 0))
        .expect("loop");

    let items = drc::check(&f, &design);
    let loops: Vec<&str> = items
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .map(|i| i.message.as_str())
        .filter(|m| m.contains("combinational loop"))
        .collect();
    assert!(
        !loops.is_empty(),
        "wiring an output back to the input feeding it closes a loop through row 0's logic"
    );
}

#[test]
fn logic_feeding_a_loop_is_not_reported_as_unused() {
    let f = fabric();
    let mut design = passthrough_row0(&f);
    // CLB(0,1) is genuinely configured — an all-default cell is reported as
    // *unconfigured*, which is a different category from dead logic — and drives
    // its result onto lane 0, which the rest of row 1 passes along.
    design.set_clb(&f, 0, 1, "operation_select", 1).expect("OR3");
    for col in 1..f.columns {
        design.set_clb(&f, col, 1, "minor_horz_sel", 0b01).expect("pass through");
    }
    // Row 1's output feeds row 2's input, and row 2 carries it onward: without
    // the liveness edge, row 1's logic looks dead.
    design
        .set_loopback(&f, IoPad::output(1, 0), IoPad::input(2, 0))
        .expect("loop");
    for col in 0..f.columns {
        design.set_clb(&f, col, 2, "minor_horz_sel", 0b01).expect("pass through");
    }

    let items = drc::check(&f, &design);
    let dead: Vec<&str> = items
        .iter()
        .map(|i| i.message.as_str())
        .filter(|m| m.contains("CLB0_1") && m.contains("unused"))
        .collect();
    assert!(dead.is_empty(), "row 1's logic reaches a chip output: {:?}", dead);
}

#[test]
fn the_file_carries_names_and_wiring_and_stays_compatible() {
    let f = fabric();
    let mut file = DesignFile::new(&f, "loops");
    file.design
        .rename_io(&f, IoPad::input(0, 0), "clk")
        .expect("name an input");
    file.design
        .rename_io(&f, IoPad::output(0, 1), "q")
        .expect("name an output");
    file.design
        .set_loopback(&f, IoPad::output(0, 0), IoPad::input(1, 0))
        .expect("loop");

    let text = save_design(&f, &file);
    assert!(text.contains("\"io_names\""), "names should be written");
    assert!(text.contains("\"loopback\""), "wiring should be written");

    let (back, warnings) = load_design(&f, &text).expect("round trip");
    assert!(warnings.is_empty(), "{:?}", warnings);
    assert_eq!(back.design.io_name(IoPad::input(0, 0)), Some("clk"));
    assert_eq!(back.design.io_name(IoPad::output(0, 1)), Some("q"));
    assert_eq!(
        back.design.loopback_driver(IoPad::input(1, 0)),
        Some(IoPad::output(0, 0))
    );

    // A design with neither must be byte-identical to the format before these
    // fields existed, which is what keeps the synthesiser's golden tests honest.
    let plain = DesignFile::new(&f, "plain");
    let text = save_design(&f, &plain);
    assert!(!text.contains("io_names"));
    assert!(!text.contains("loopback"));
}

#[test]
fn a_file_naming_a_pad_this_fabric_lacks_warns_rather_than_fails() {
    let f = fabric();
    let plain = save_design(&f, &DesignFile::new(&f, "x"));
    // Splice in an entry for a pad that does not exist.
    let doctored = plain.replace(
        "  \"view\": null",
        "  \"view\": null,\n  \"io_names\": {\n    \"not_a_pad\": \"whatever\"\n  }",
    );
    let (_file, warnings) = load_design(&f, &doctored).expect("still loads");
    assert!(
        warnings.iter().any(|w| w.contains("not_a_pad")),
        "the unknown pad should be reported: {:?}",
        warnings
    );
}
