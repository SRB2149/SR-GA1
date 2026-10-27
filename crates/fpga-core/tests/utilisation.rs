//! Utilisation: what a configuration really uses, not what the all-zero
//! default happens to drive.

use fpga_core::config::{BlockId, Design};
use fpga_core::designfile::load_design;
use fpga_core::fabric::Fabric;
use fpga_core::utilisation::utilisation;

fn fabric() -> Fabric {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fabric.toml");
    Fabric::load_file(std::path::Path::new(path)).expect("fabric.toml must load")
}

#[test]
fn the_empty_design_uses_nothing() {
    let f = fabric();
    let u = utilisation(&f, &Design::new(&f));
    for (label, count) in [
        ("CLBs", u.clbs),
        ("registers", u.registers),
        ("CSBs", u.csbs),
        ("horizontal", u.horz_segments),
        ("vertical", u.vert_segments),
        ("input pads", u.input_pads),
        ("output pads", u.output_pads),
        ("input muxes", u.input_muxes),
        ("on-chip loops", u.on_chip_loops),
    ] {
        assert_eq!(count.used, 0, "{} should be unused in the empty design", label);
        assert!(count.total > 0, "{} has capacity", label);
    }
    assert_eq!(u.clbs.total, 32);
    assert_eq!(u.horz_segments.total, 4 * 4 * 9);
    assert_eq!(u.vert_segments.total, 8 * 4 * 4);
    assert_eq!(u.input_muxes.total, 8);
}

#[test]
fn a_pass_through_uses_its_lane_end_to_end() {
    let f = fabric();
    let mut d = Design::new(&f);
    for col in 0..f.columns {
        d.set_clb(&f, col, 0, "minor_horz_sel", 0b01).unwrap();
    }
    let u = utilisation(&f, &d);
    // input_0 enters row 0 lane 0 and leaves on output_0, which reads it.
    for pos in 0..=f.columns {
        assert!(u.horz(0, 0, pos), "row 0 lane 0 segment {} carries input_0", pos);
    }
    assert_eq!(u.horz_by_lane(0, 0).used, f.columns + 1);
    assert_eq!(u.input_pads.used, 1);
    assert!(u.output_pads.used >= 1);
    // The pass-through CLBs are configured, so they count as used blocks.
    assert_eq!(u.clbs.used, f.columns);
    assert!(u.block(BlockId::Clb { col: 3, row: 0 }));
    assert!(!u.block(BlockId::Clb { col: 3, row: 1 }));
}

#[test]
fn the_synthesised_counter_reports_its_resources() {
    let f = fabric();
    let (file, _) = load_design(&f, include_str!("data/count4.json")).expect("loads");
    let u = utilisation(&f, &file.design);
    assert_eq!(u.registers.used, 4);
    assert!(u.clbs.used >= 4);
    assert!(u.csbs.used >= 1, "the counter's clock comes through a CSB");
    assert_eq!(u.on_chip_loops.used, 4, "one loop per count bit");
    assert!(u.output_pads.used >= 4);
    assert!(u.carry_links.used >= 3, "a 4-bit chain has three internal carries");
    assert!(u.vert_segments.used >= 1, "the clock rides the vertical ring");
}

#[test]
fn named_clbs_leave_out_routing_pass_throughs() {
    let f = fabric();
    let (file, _) = load_design(&f, include_str!("data/count4.json")).expect("loads");
    let u = utilisation(&f, &file.design);
    // count[0..3] and buf_clk are named; the rest only pass lanes through.
    assert_eq!(u.named_clbs.used, 5);
    assert!(u.clbs.used > u.named_clbs.used);
    assert!(u.block_counted(BlockId::Clb { col: 2, row: 0 }, true));
    assert!(u.block(BlockId::Clb { col: 0, row: 0 }));
    assert!(!u.block_counted(BlockId::Clb { col: 0, row: 0 }, true));
}
