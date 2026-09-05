//! Naming engine tests: pass-through chains (horizontal, vertical snake,
//! ring), the v->h bridge, floating loops, CSB couple chains and cycles, and
//! pinned-name behaviour.

use fpga_core::config::{BlockId, Design};
use fpga_core::fabric::Fabric;
use fpga_core::naming::{resolve, Namer, NetOrigin, OutputKind};

fn fabric() -> Fabric {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fabric.toml");
    Fabric::load_file(std::path::Path::new(path)).expect("fabric.toml must load")
}

#[test]
fn default_configuration_names() {
    let f = fabric();
    let d = Design::new(&f);
    let nets = resolve(&f, &d);
    let namer = Namer::new(&f, &d);

    // Left edge: reserved IO names straight from the input controller.
    assert_eq!(namer.net_name(nets.horz_in(0, 0, 0)), "input_0");
    assert_eq!(namer.net_name(nets.horz_in(0, 2, 3)), "ddio_in_1");
    assert_eq!(namer.net_name(nets.horz_in(0, 3, 1)), "fixed_one");

    // One column in: minor lanes carry the previous CLB's op, major lanes the
    // default constant zero.
    assert_eq!(namer.net_name(nets.horz_in(1, 0, 0)), "CLB0_0_op");
    assert_eq!(namer.net_name(nets.horz_in(1, 0, 1)), "CLB0_0_op");
    assert_eq!(namer.net_name(nets.horz_in(1, 0, 2)), "fixed_zero");
    assert_eq!(namer.net_name(nets.horz_in(1, 0, 3)), "fixed_zero");

    // Vertical defaults: lanes 0/2 carry op, lanes 1/3 carry reg, from the
    // row below (ring: row 0's inputs come from the top row).
    assert_eq!(namer.net_name(nets.vert_in(2, 1, 0)), "CLB2_0_op");
    assert_eq!(namer.net_name(nets.vert_in(2, 1, 3)), "CLB2_0_reg");
    assert_eq!(namer.net_name(nets.vert_in(2, 0, 0)), "CLB2_3_op");

    // Default clock: bus_addr 0 -> ring lane 0 at the tap -> top row's op.
    for col in 0..f.columns {
        assert_eq!(namer.clock_name(&nets.clocks[col]), format!("CLB{}_3_op", col));
        assert_eq!(nets.clocks[col].source_csb, col);
    }
}

#[test]
fn horizontal_passthrough_chain() {
    let f = fabric();
    let mut d = Design::new(&f);
    // Row 0: every CLB from column 1 on passes lane 0 through, so CLB0_0's op
    // (driven at column 1's input by default) spans the rest of the row.
    for col in 1..f.columns {
        d.set_clb(&f, col, 0, "minor_horz_sel", 0b01).unwrap();
    }
    let nets = resolve(&f, &d);
    let namer = Namer::new(&f, &d);
    for col in 1..f.columns {
        assert_eq!(namer.net_name(nets.horz_in(col, 0, 0)), "CLB0_0_op");
    }
    assert_eq!(namer.net_name(nets.horz_edge(0, 0)), "CLB0_0_op");

    // Passing through at column 0 as well makes the chip input span instead.
    d.set_clb(&f, 0, 0, "minor_horz_sel", 0b01).unwrap();
    let nets = resolve(&f, &d);
    let namer = Namer::new(&f, &d);
    assert_eq!(namer.net_name(nets.horz_edge(0, 0)), "input_0");
}

#[test]
fn major_mux_bridges_vertical_to_horizontal() {
    let f = fabric();
    let mut d = Design::new(&f);
    // CLB(2,1) drives h_out:2 from v_in:2 (code 2). By default that vertical
    // segment carries CLB(2,0)'s op.
    d.set_clb(&f, 2, 1, "major_horz2_sel", 2).unwrap();
    let nets = resolve(&f, &d);
    let namer = Namer::new(&f, &d);
    assert_eq!(namer.net_name(nets.horz_in(3, 1, 2)), "CLB2_0_op");
}

#[test]
fn vertical_snake_and_ring_tap() {
    let f = fabric();
    let mut d = Design::new(&f);
    // CLB(5,0) introduces its op on v_out:0 (default). Rows 1-3 pass it
    // through; the snake swaps lanes 0<->2 at every hop, so it travels
    // lane 0 -> 2 -> 0 -> 2 and reaches the ring tap on lane 2.
    d.set_clb(&f, 5, 1, "minor_vert_sel", 0b0100).unwrap();
    d.set_clb(&f, 5, 2, "minor_vert_sel", 0b0001).unwrap();
    d.set_clb(&f, 5, 3, "minor_vert_sel", 0b0100).unwrap();
    d.set_csb(&f, 5, "bus_addr", 2).unwrap();
    let nets = resolve(&f, &d);
    let namer = Namer::new(&f, &d);

    assert_eq!(namer.net_name(nets.vert_in(5, 1, 0)), "CLB5_0_op");
    assert_eq!(namer.net_name(nets.vert_in(5, 2, 2)), "CLB5_0_op");
    assert_eq!(namer.net_name(nets.vert_in(5, 3, 0)), "CLB5_0_op");
    assert_eq!(namer.net_name(nets.ring_tap(5, 2)), "CLB5_0_op");
    assert_eq!(namer.clock_name(&nets.clocks[5]), "CLB5_0_op");
}

#[test]
fn floating_vertical_ring() {
    let f = fabric();
    let mut d = Design::new(&f);
    // Every row of column 4 passes all four vertical lanes through: two
    // closed pass-through cycles with no driver.
    for row in 0..f.rows {
        d.set_clb(&f, 4, row, "minor_vert_sel", 0b1111).unwrap();
    }
    let nets = resolve(&f, &d);
    let namer = Namer::new(&f, &d);
    for row in 0..f.rows {
        for lane in 0..f.vert_lanes {
            assert_eq!(nets.vert_in(4, row, lane), NetOrigin::FloatingLoop);
        }
    }
    assert_eq!(namer.net_name(nets.vert_in(4, 0, 0)), "<unconnected>");
    // The CSB selects the floating ring: fall back to its own clock name.
    assert_eq!(namer.clock_name(&nets.clocks[4]), "CSB4_clock");
}

#[test]
fn csb_couple_chain_propagates_transitively() {
    let f = fabric();
    let mut d = Design::new(&f);
    d.set_csb(&f, 6, "couple_to_previous", 1).unwrap();
    d.set_csb(&f, 5, "couple_to_previous", 1).unwrap();
    let nets = resolve(&f, &d);
    let namer = Namer::new(&f, &d);
    // CSB6 -> CSB5 -> CSB4, which selects its own column's ring lane 0.
    assert_eq!(nets.clocks[6].source_csb, 4);
    assert_eq!(namer.clock_name(&nets.clocks[6]), "CLB4_3_op");
    assert_eq!(namer.clock_name(&nets.clocks[5]), "CLB4_3_op");
}

#[test]
fn csb_full_circle_terminates() {
    let f = fabric();
    let mut d = Design::new(&f);
    for col in 0..f.columns {
        d.set_csb(&f, col, "couple_to_previous", 1).unwrap();
    }
    let nets = resolve(&f, &d);
    let namer = Namer::new(&f, &d);
    for col in 0..f.columns {
        assert!(nets.clocks[col].origin.is_none(), "column {} should have no source", col);
        assert_eq!(namer.clock_name(&nets.clocks[col]), format!("CSB{}_clock", col));
    }
}

#[test]
fn carry_chain_names_follow_the_column_up() {
    let f = fabric();
    let mut d = Design::new(&f);
    // Column 3 takes carry-in on input c all the way up.
    for row in 0..f.rows {
        d.set_clb(&f, 3, row, "input_mux_c_sel", 3).unwrap();
    }
    let nets = resolve(&f, &d);
    let namer = Namer::new(&f, &d);

    // Row 0 sees the chain's edge constant; every other row sees the carry
    // output of the cell below it.
    assert_eq!(nets.carry_in(3, 0), NetOrigin::Const(false));
    assert_eq!(namer.net_name(nets.carry_in(3, 0)), "fixed_zero");
    for row in 1..f.rows {
        assert_eq!(
            nets.carry_in(3, row),
            NetOrigin::ClbOutput { col: 3, row: row - 1, kind: OutputKind::Carry }
        );
        assert_eq!(namer.net_name(nets.carry_in(3, row)), format!("CLB3_{}_carry", row - 1));
    }
    // Chains are per column and never wrap, so the top row's carry goes
    // nowhere and a neighbouring column is unaffected.
    assert_eq!(nets.carry_in(4, 0), NetOrigin::Const(false));

    // A pinned name propagates onto the carry net like any other output.
    d.rename(&f, BlockId::Clb { col: 3, row: 1 }, "adder_bit1").unwrap();
    let namer = Namer::new(&f, &d);
    assert_eq!(namer.net_name(nets.carry_in(3, 2)), "adder_bit1_carry");
}

#[test]
fn pinned_names_propagate_and_revert() {
    let f = fabric();
    let mut d = Design::new(&f);
    let block = BlockId::Clb { col: 0, row: 0 };
    d.rename(&f, block, "alu").unwrap();
    let nets = resolve(&f, &d);
    let namer = Namer::new(&f, &d);
    assert!(namer.is_pinned(block));
    assert_eq!(namer.block_name(block), "alu");
    assert_eq!(namer.net_name(nets.horz_in(1, 0, 0)), "alu_op");
    assert_eq!(
        nets.horz_in(1, 0, 0),
        NetOrigin::ClbOutput { col: 0, row: 0, kind: OutputKind::Op }
    );

    d.revert_name(block);
    let namer = Namer::new(&f, &d);
    assert!(!namer.is_pinned(block));
    assert_eq!(namer.net_name(nets.horz_in(1, 0, 0)), "CLB0_0_op");
}

#[test]
fn rename_collisions_are_rejected() {
    let f = fabric();
    let mut d = Design::new(&f);
    let a = BlockId::Clb { col: 0, row: 0 };
    let b = BlockId::Clb { col: 1, row: 1 };

    assert!(d.rename(&f, a, "").is_err(), "empty");
    assert!(d.rename(&f, a, "input_0").is_err(), "reserved IO name");
    assert!(d.rename(&f, a, "fixed_zero").is_err(), "reserved constant");
    assert!(d.rename(&f, a, "CSB2_clock").is_err(), "clock fallback name");
    assert!(d.rename(&f, a, "CLB0_1").is_err(), "another block's default name");
    assert!(d.rename(&f, a, "CLB0_1_op").is_err(), "another block's derived net name");

    d.rename(&f, a, "alu").unwrap();
    assert!(d.rename(&f, b, "alu").is_err(), "already pinned elsewhere");
    assert!(d.rename(&f, b, "alu_reg").is_err(), "derived from a pinned name");

    // Derived collision in the other direction: pinning "x_op" first makes a
    // later "x" illegal because x + suffix_op would collide.
    d.rename(&f, BlockId::Clb { col: 3, row: 3 }, "x_op").unwrap();
    assert!(d.rename(&f, b, "x").is_err(), "own derived net would collide");

    // CSBs share the same namespace.
    assert!(d.rename(&f, BlockId::Csb { col: 0 }, "alu_carry").is_err());
    d.rename(&f, BlockId::Csb { col: 0 }, "main_clk_sel").unwrap();

    // A block may re-pin to its own default name.
    assert!(d.rename(&f, a, "CLB0_0").is_ok());
}
