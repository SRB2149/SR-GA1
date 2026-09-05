//! Tests for the fabric.toml loader against the real description at the repo
//! root, including the 12x4 / 16x4 scaling requirement and error reporting.

use fpga_core::fabric::{ChainCells, ChainOrder, Fabric, Source};

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
    assert_eq!((f.columns, f.rows), (7, 4));
    assert_eq!((f.horz_lanes, f.vert_lanes), (4, 4));
    assert!(f.vertical_ring);
    assert_eq!(f.clb_bits(), 20);
    assert_eq!(f.csb_bits(), 3);
    assert_eq!(f.total_bits(), 581);
}

#[test]
fn scales_by_editing_columns_only() {
    for (columns, total) in [(12, 996), (16, 1328)] {
        let s = src().replace("columns = 7", &format!("columns = {columns}"));
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
    assert_eq!(field("operation_select").offset, 4);
    assert_eq!(field("minor_horz_sel").offset, 7);
    assert_eq!(field("minor_vert_sel").offset, 9);
    assert_eq!(field("major_horz2_sel").offset, 13);
    assert_eq!(field("major_horz3_sel").offset, 16);
    assert_eq!(field("op_ff_reset_val").offset, 19);
    assert_eq!(f.csb_field("bus_addr").expect("bus_addr").offset, 0);
    assert_eq!(f.csb_field("couple_to_previous").expect("couple").offset, 2);
}

#[test]
fn operation_semantics() {
    let f = load();
    let code = |name: &str| f.operation_by_name(name).expect(name);
    assert_eq!(code("AND3"), 0);
    assert_eq!(code("AO21"), 6);
    for i in 0..8usize {
        let (a, b, c) = (i & 1 != 0, i & 2 != 0, i & 4 != 0);
        let inputs = [a, b, c];
        assert_eq!(f.eval_operation(code("AND3"), &inputs), Some(a && b && c));
        assert_eq!(f.eval_operation(code("OR3"), &inputs), Some(a || b || c));
        assert_eq!(f.eval_operation(code("XOR3"), &inputs), Some(a ^ b ^ c));
        assert_eq!(f.eval_operation(code("NAND3"), &inputs), Some(!(a && b && c)));
        assert_eq!(f.eval_operation(code("NOR3"), &inputs), Some(!(a || b || c)));
        assert_eq!(f.eval_operation(code("XNOR3"), &inputs), Some(!(a ^ b ^ c)));
        assert_eq!(f.eval_operation(code("AO21"), &inputs), Some((a && b) || c));
        assert_eq!(f.eval_operation(code("MUX2"), &inputs), Some(if c { a } else { b }));
        let carry = (a as u8 + b as u8 + c as u8) >= 2;
        assert_eq!(f.eval_carry(&inputs), Some(carry));
    }
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
    assert_eq!(f.chain.len(), 2);
    assert_eq!(f.chain[0].cells, ChainCells::Clb);
    assert_eq!(f.chain[0].order, ChainOrder::RowMajor);
    assert_eq!(f.chain[1].cells, ChainCells::Csb);
    assert_eq!(f.chain[1].order, ChainOrder::ColumnAscending);
    assert_eq!(f.clb_name(0, 3), "CLB0_3");
    assert_eq!(f.csb_name(6), "CSB6");
    assert_eq!(f.io_inputs[3][1], "fixed_one");
    assert_eq!(f.io_outputs[3][2], None);
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
fn rejects_missing_operation_code() {
    let s = src().replace(
        r#"{ code = 7, name = "MUX2",  table = [0,0,1,1,0,1,0,1] },"#,
        "",
    );
    expect_error(&s, "no operation defined for code 7");
}

#[test]
fn rejects_bad_select_reference() {
    let s = src().replace(
        r#"select = "input_mux_a_sel""#,
        r#"select = "input_mux_a_zel""#,
    );
    expect_error(&s, "input_mux_a_zel");
}
