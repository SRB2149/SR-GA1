//! Bitstream codec tests against known-good positions derived from the RTL:
//! the start bit leads, then CSB7's couple bit (the deepest register bit), the
//! IO controller, and the CLBs down to CLB(0,0)'s input_mux_a_sel last, each
//! cell's bits in descending chain-index order.

use fpga_core::bitstream::{block_bits, export_bits, format_text, import_bits, parse_text};
use fpga_core::config::{BlockId, Design};
use fpga_core::fabric::Fabric;

fn fabric() -> Fabric {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fabric.toml");
    Fabric::load_file(std::path::Path::new(path)).expect("fabric.toml must load")
}

/// The stream positions of every 1 bit after the start bit.
fn ones(bits: &[bool]) -> Vec<usize> {
    bits.iter().enumerate().skip(1).filter(|(_, &b)| b).map(|(i, _)| i).collect()
}

#[test]
fn all_zero_design_exports_only_the_start_bit() {
    let f = fabric();
    let bits = export_bits(&f, &Design::new(&f));
    assert_eq!(bits.len(), 889);
    assert!(bits[0], "the start bit is a 1 and leads the stream");
    assert!(ones(&bits).is_empty());
}

#[test]
fn known_bit_positions() {
    let f = fabric();

    // Right after the start bit: CSB7 couple_to_previous (deepest register bit).
    let mut d = Design::new(&f);
    d.set_csb(&f, 7, "couple_to_previous", 1).unwrap();
    assert_eq!(ones(&export_bits(&f, &d)), vec![1]);

    // CSB0 occupies the last CSB slot: stream bits 22..24.
    let mut d = Design::new(&f);
    d.set_csb(&f, 0, "bus_addr", 0b10).unwrap();
    assert_eq!(ones(&export_bits(&f, &d)), vec![23], "bus_addr bit 1 follows the couple bit");

    // The IO controller sits between the clock bank and the CLBs: its bit 63
    // (output_mux_sel_ddio_dir1[3]) at 25, its bit 0 (input_mux_sel0a[0]) at 88.
    let mut d = Design::new(&f);
    d.set_io(&f, "output_mux_sel_ddio_dir1", 0b1000).unwrap();
    assert_eq!(ones(&export_bits(&f, &d)), vec![25]);
    let mut d = Design::new(&f);
    d.set_io(&f, "input_mux_sel0a", 1).unwrap();
    assert_eq!(ones(&export_bits(&f, &d)), vec![88]);

    // First CLB in the stream is (col 7, row 3); its first bit is
    // op_ff_reset_val (chain index 24).
    let mut d = Design::new(&f);
    d.set_clb(&f, 7, 3, "op_ff_reset_val", 1).unwrap();
    assert_eq!(ones(&export_bits(&f, &d)), vec![89]);

    // Last transmitted bit lands in CLB(0,0)'s input_mux_a_sel.
    let mut d = Design::new(&f);
    d.set_clb(&f, 0, 0, "input_mux_a_sel", 1).unwrap();
    assert_eq!(ones(&export_bits(&f, &d)), vec![888]);

    // Multi-bit fields transmit MSB first: lut bit 7 of CLB(0,0) is chain
    // index 11, so 13 bits into that cell's 25 (864 + 13).
    let mut d = Design::new(&f);
    d.set_clb(&f, 0, 0, "lut", 0x80).unwrap();
    assert_eq!(ones(&export_bits(&f, &d)), vec![877]);
    assert!(block_bits(&f, BlockId::Clb { col: 0, row: 0 }).contains(&(877, 3, 7)));
    assert_eq!(block_bits(&f, BlockId::Io).len(), 64);
}

#[test]
fn round_trips_a_scrambled_design() {
    let f = fabric();
    let mut d = Design::new(&f);
    // Deterministic pseudo-random fill over every field of every cell.
    let mut lcg: u64 = 0x2545F4914F6CDD1D;
    let mut next = move || {
        lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        lcg >> 33
    };
    for block in d.blocks() {
        let fields = match block {
            BlockId::Clb { .. } => &f.clb_fields,
            BlockId::Csb { .. } => &f.csb_fields,
            BlockId::Io => &f.io_fields,
        };
        for (i, field) in fields.iter().enumerate() {
            d.block_mut(block).set(i, field.width, next());
        }
    }

    let bits = export_bits(&f, &d);
    let imported = import_bits(&f, &bits).unwrap();
    assert_eq!(export_bits(&f, &imported), bits);
    assert_eq!(
        imported.get_clb(&f, 3, 2, "major_horz2_sel").unwrap(),
        d.get_clb(&f, 3, 2, "major_horz2_sel").unwrap()
    );
    assert_eq!(imported.get_csb(&f, 5, "bus_addr").unwrap(), d.get_csb(&f, 5, "bus_addr").unwrap());
    assert_eq!(
        imported.get_io(&f, "output_mux_sel2").unwrap(),
        d.get_io(&f, "output_mux_sel2").unwrap()
    );
}

#[test]
fn rejects_wrong_length() {
    let f = fabric();
    let err = import_bits(&f, &[false; 100]).unwrap_err();
    assert!(err.message.contains("889"), "got: {}", err.message);
    assert!(err.message.contains("1 start bit + 888"), "got: {}", err.message);
}

#[test]
fn rejects_a_missing_start_bit() {
    let f = fabric();
    let mut bits = export_bits(&f, &Design::new(&f));
    bits[0] = false;
    let err = import_bits(&f, &bits).unwrap_err();
    assert!(err.message.contains("start bit"), "got: {}", err.message);
}

#[test]
fn text_format_round_trip() {
    let f = fabric();
    let mut d = Design::new(&f);
    d.set_clb(&f, 2, 1, "lut", 0x69).unwrap();
    d.set_csb(&f, 3, "bus_addr", 1).unwrap();
    d.set_io(&f, "input_mux_sel2b", 13).unwrap();

    let text = format_text(&f, &d, "my design", "2026-10-12 12:00:00", false, None);
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("# my design"));
    assert_eq!(lines.next(), Some("# 2026-10-12 12:00:00"));
    let body = lines.next().unwrap();
    assert_eq!(body.len(), 889);
    assert!(body.starts_with('1'));
    assert!(lines.next().is_none(), "single line by default");

    let bits = parse_text(&text).unwrap();
    assert_eq!(bits, export_bits(&f, &d));
    let reimported = import_bits(&f, &bits).unwrap();
    assert_eq!(reimported.get_clb(&f, 2, 1, "lut").unwrap(), 0x69);
    assert_eq!(reimported.get_csb(&f, 3, "bus_addr").unwrap(), 1);
    assert_eq!(reimported.get_io(&f, "input_mux_sel2b").unwrap(), 13);

    // Raw mode has no header; wrapping splits the body only.
    let raw = format_text(&f, &d, "my design", "now", true, Some(100));
    assert!(!raw.starts_with('#'));
    assert_eq!(raw.lines().count(), 9); // ceil(889 / 100)
    assert_eq!(parse_text(&raw).unwrap(), export_bits(&f, &d));
}

#[test]
fn parse_rejects_garbage() {
    let err = parse_text("0101x01").unwrap_err();
    assert!(err.message.contains('x'), "got: {}", err.message);
}
