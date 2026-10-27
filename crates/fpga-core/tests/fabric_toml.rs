//! Tests for the fabric.toml loader against the real description at the repo
//! root, including the 12x4 / 16x4 scaling requirement and error reporting.

use fpga_core::fabric::{ChainCells, ChainOrder, Fabric, IoSource, LeftLane, PadDrive, Source};

fn src() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fabric.toml");
    std::fs::read_to_string(path).expect("fabric.toml at repo root")
}

fn load() -> Fabric {
    Fabric::load_str(&src()).expect("fabric.toml must validate")
}

#[test]
fn dimensions_and_bit_counts() {
    let f = load();
    assert_eq!(f.name, "SR-GA1");
    assert_eq!((f.columns, f.rows), (8, 4));
    assert_eq!((f.horz_lanes, f.vert_lanes), (4, 4));
    assert!(f.vertical_ring);
    assert_eq!(f.clb_bits(), 25);
    assert_eq!(f.io_bits(), 64);
    assert_eq!(f.csb_bits(), 3);
    // 0-799 CLBs, 800-863 IO controller, 864-887 clock bank (hdl/top/sr-ga1.sv).
    assert_eq!(f.total_bits(), 888);
    assert_eq!(f.start_bit, Some(true));
    assert_eq!(f.stream_bits(), 889);
}

#[test]
fn scales_by_editing_columns_only() {
    for (columns, total) in [(12, 1300), (16, 1712)] {
        let s = src().replace("columns = 8", &format!("columns = {columns}"));
        let f = Fabric::load_str(&s).expect("scaled fabric must validate");
        assert_eq!(f.columns, columns);
        assert_eq!(f.total_bits(), total);
    }
}

#[test]
fn field_offsets_match_rtl() {
    let f = load();
    let field = |name: &str| f.clb_field(name).expect(name);
    assert_eq!(field("input_mux_a_sel").offset, 0);
    assert_eq!(field("input_mux_c_sel").offset, 2);
    assert_eq!(field("input_mux_c_sel").width, 2);
    assert_eq!(field("lut").offset, 4);
    assert_eq!(field("lut").width, 8);
    assert_eq!(field("minor_horz_sel").offset, 12);
    assert_eq!(field("minor_vert_sel").offset, 14);
    assert_eq!(field("major_horz2_sel").offset, 18);
    assert_eq!(field("major_horz3_sel").offset, 21);
    assert_eq!(field("op_ff_reset_val").offset, 24);
    assert_eq!(f.csb_field("bus_addr").expect("bus_addr").offset, 0);
    assert_eq!(f.csb_field("couple_to_previous").expect("couple").offset, 2);
    // hdl/io/io_controller.sv register map.
    let io = |name: &str| f.io_field(name).expect(name).offset;
    assert_eq!(io("input_mux_sel0a"), 0);
    assert_eq!(io("input_mux_sel3b"), 28);
    assert_eq!(io("output_mux_sel0"), 32);
    assert_eq!(io("output_mux_sel_ddio0"), 48);
    assert_eq!(io("output_mux_sel_ddio_dir1"), 60);
}

/// The RTL indexes the LUT as `lut[{input_a, input_b, input_c}]`: input a is
/// the most significant bit. The presets are the op table hdl/clb's unit test
/// uses, so each must compute the function it is named after.
#[test]
fn lut_semantics() {
    let f = load();
    let preset = |name: &str| f.preset_value(name).expect(name);
    assert_eq!(preset("XOR3"), 0x96);
    assert_eq!(f.preset_name(0xEA), Some("AO21"));
    for i in 0..8usize {
        let (a, b, c) = (i & 4 != 0, i & 2 != 0, i & 1 != 0);
        let inputs = [a, b, c];
        assert_eq!(f.lut_index(&inputs), i);
        let eval = |name: &str| f.eval_lut(preset(name), &inputs);
        assert_eq!(eval("AND3"), Some(a && b && c));
        assert_eq!(eval("OR3"), Some(a || b || c));
        assert_eq!(eval("XOR3"), Some(a ^ b ^ c));
        assert_eq!(eval("NAND3"), Some(!(a && b && c)));
        assert_eq!(eval("NOR3"), Some(!(a || b || c)));
        assert_eq!(eval("XNOR3"), Some(!(a ^ b ^ c)));
        assert_eq!(eval("AO21"), Some((a && b) || c));
        assert_eq!(eval("MUX2"), Some(if c { a } else { b }));
        assert_eq!(eval("BUF_A"), Some(a));
        assert_eq!(eval("INV_C"), Some(!c));
        let carry = (a as u8 + b as u8 + c as u8) >= 2;
        assert_eq!(f.eval_carry(&inputs), Some(carry));
        assert_eq!(eval("MAJ3"), Some(carry));
    }
}

#[test]
fn io_controller_matches_rtl() {
    let f = load();
    let input = |name: &str| IoSource::InPad(f.input_pad(name).expect(name));
    let output = |name: &str| IoSource::OutPad(f.output_pad(name).expect(name));
    // 0-9 chip inputs, 10-11 DDIO, 12-15 the first four chip outputs.
    assert_eq!(f.io_mux_sources.len(), 16);
    assert_eq!(f.io_mux_sources[0], input("input_0"));
    assert_eq!(f.io_mux_sources[10], input("ddio_in_0"));
    assert_eq!(f.io_mux_sources[15], output("output_3"));
    let fixed = |row: usize, lane: usize| match f.io_left[row][lane] {
        LeftLane::Fixed(src) => f.io_source_name(src).to_string(),
        LeftLane::Mux(_) => "mux".to_string(),
    };
    assert_eq!(fixed(0, 0), "mux");
    assert_eq!(fixed(3, 1), "mux");
    assert_eq!(fixed(0, 3), "input_3");
    assert_eq!(fixed(1, 2), "input_6");
    assert_eq!(fixed(2, 3), "ddio_in_1");
    assert_eq!(fixed(3, 2), "input_8");
    let drive = |name: &str| f.io_drive[f.output_pad(name).expect(name)];
    assert!(matches!(drive("output_0"), PadDrive::Mux(_)));
    assert!(matches!(drive("output_4"), PadDrive::Fixed { row: 1, lane: 2 }));
    assert!(matches!(drive("output_9"), PadDrive::Fixed { row: 3, lane: 3 }));
    assert!(matches!(drive("ddio_dir_1"), PadDrive::Mux(_)));
    assert_eq!(f.drive_lane(14), (3, 2));
    let d = f.ddio[1];
    assert_eq!(f.io_inputs[d.input], "ddio_in_1");
    assert_eq!(f.io_outputs[d.output], "ddio_out_1");
    assert_eq!(f.io_outputs[d.dir], "ddio_dir_1");
}

#[test]
fn major_mux_sources_match_rtl() {
    let f = load();
    let major = f
        .output_muxes
        .iter()
        .find(|m| m.drives == fpga_core::fabric::BusOut::Horz(2))
        .expect("h_out:2 mux");
    assert_eq!(
        major.sources,
        vec![
            Source::Const(false),
            Source::Const(true),
            Source::VertIn(2),
            Source::HorzIn(2),
            Source::Op,
            Source::Reg,
            Source::VertIn(3),
            Source::HorzIn(3),
        ]
    );
    let major3 = f
        .output_muxes
        .iter()
        .find(|m| m.drives == fpga_core::fabric::BusOut::Horz(3))
        .expect("h_out:3 mux");
    assert_eq!(
        major3.sources,
        vec![
            Source::Const(false),
            Source::Const(true),
            Source::VertIn(3),
            Source::HorzIn(3),
            Source::Op,
            Source::Reg,
            Source::VertIn(2),
            Source::HorzIn(2),
        ]
    );
    assert_eq!(f.ff.enable, Source::HorzIn(3));

    // Input c reaches the carry chain, which makes the cell a full adder.
    let c = f.input_muxes.iter().find(|m| m.name == "c").expect("input mux c");
    assert_eq!(
        c.sources,
        vec![Source::HorzIn(2), Source::HorzIn(3), Source::VertIn(0), Source::CarryIn]
    );
    assert_eq!(f.carry.chain, fpga_core::fabric::CarryChain::ColumnUp);
    assert!(!f.carry.edge);
}

#[test]
fn chain_and_names() {
    let f = load();
    assert_eq!(f.chain.len(), 3);
    assert_eq!(f.chain[0].cells, ChainCells::Clb);
    assert_eq!(f.chain[0].order, ChainOrder::RowMajor);
    assert_eq!(f.chain[1].cells, ChainCells::Io);
    assert_eq!(f.chain[2].cells, ChainCells::Csb);
    assert_eq!(f.chain[2].order, ChainOrder::ColumnAscending);
    assert_eq!(f.clb_name(0, 3), "CLB0_3");
    assert_eq!(f.csb_name(7), "CSB7");
    assert_eq!(f.io_name(), "IO");
    assert_eq!(f.io_inputs.len(), 12);
    assert_eq!(f.io_outputs.len(), 14);
}

fn expect_error(s: &str, needle: &str) {
    let err = Fabric::load_str(s).expect_err("must not validate");
    assert!(
        err.to_string().contains(needle),
        "error {:?} does not mention {:?}",
        err.to_string(),
        needle
    );
    assert!(err.line.is_some(), "error should carry a line number: {}", err);
}

#[test]
fn rejects_out_of_range_lane() {
    let s = src().replace(
        r#"sources = ["h_in:0", "h_in:1"]"#,
        r#"sources = ["h_in:0", "h_in:9"]"#,
    );
    expect_error(&s, "horizontal lane 9 out of range");
}

#[test]
fn rejects_wrong_source_count() {
    let s = src().replace(
        r#"sources = ["h_in:0", "h_in:1"]"#,
        r#"sources = ["h_in:0", "h_in:1", "h_in:2"]"#,
    );
    expect_error(&s, "2 sources required for 1-bit select");
}

#[test]
fn rejects_duplicate_field() {
    let s = src().replace(
        r#"{ name = "op_ff_reset_val", width = 1 },"#,
        r#"{ name = "input_mux_a_sel", width = 1 },"#,
    );
    expect_error(&s, "duplicate field name");
}

#[test]
fn rejects_unknown_key() {
    let s = format!("{}\nbogus_key = 1\n", src());
    let err = Fabric::load_str(&s).expect_err("must not validate");
    assert!(err.to_string().contains("bogus_key"), "got: {}", err);
}

#[test]
fn rejects_a_lut_field_of_the_wrong_width() {
    let s = src().replace(r#"{ name = "lut", width = 8 },"#, r#"{ name = "lut", width = 7 },"#);
    expect_error(&s, "must be 8 bit(s) wide");
}

#[test]
fn rejects_an_undriven_output_pad() {
    let s = src().replace("output_9 = \"lane:3:3\"", "");
    expect_error(&s, "nothing drives output-side pad \"output_9\"");
}

#[test]
fn rejects_an_unknown_io_source() {
    let s = src().replace(
        r#"["sel:input_mux_sel0a", "sel:input_mux_sel0b", "input_2", "input_3"]"#,
        r#"["sel:input_mux_sel0a", "sel:input_mux_sel0b", "input_2", "input_33"]"#,
    );
    expect_error(&s, "\"input_33\" is not a declared pad");
}

#[test]
fn rejects_bad_select_reference() {
    let s = src().replace(
        r#"select = "input_mux_a_sel""#,
        r#"select = "input_mux_a_zel""#,
    );
    expect_error(&s, "input_mux_a_zel");
}
