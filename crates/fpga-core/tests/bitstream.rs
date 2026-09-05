//! Bitstream codec tests against known-good positions derived from the RTL
//! unit tests: CSB6's couple bit is transmitted first, CLB(0,0)'s
//! input_mux_a_sel last, each cell's bits in descending chain-index order.

use fpga_core::bitstream::{export_bits, format_text, import_bits, parse_text};
use fpga_core::config::Design;
use fpga_core::fabric::Fabric;

fn fabric() -> Fabric {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fabric.toml");
    Fabric::load_file(std::path::Path::new(path)).expect("fabric.toml must load")
}

#[test]
fn all_zero_design_exports_zeros() {
    let f = fabric();
    let bits = export_bits(&f, &Design::new(&f));
    assert_eq!(bits.len(), 581);
    assert!(bits.iter().all(|&b| !b));
}

#[test]
fn known_bit_positions() {
    let f = fabric();

    // First transmitted bit: CSB6 couple_to_previous (deepest register bit).
    let mut d = Design::new(&f);
    d.set_csb(&f, 6, "couple_to_previous", 1).unwrap();
    let bits = export_bits(&f, &d);
    assert!(bits[0]);
    assert_eq!(bits.iter().filter(|&&b| b).count(), 1);

    // CSB0 occupies the last CSB slot before the CLBs: bits 18..21.
    let mut d = Design::new(&f);
    d.set_csb(&f, 0, "bus_addr", 0b10).unwrap();
    let bits = export_bits(&f, &d);
    assert!(bits[19], "bus_addr bit 1 follows the couple bit");
    assert_eq!(bits.iter().filter(|&&b| b).count(), 1);

    // First CLB in the stream is (col 6, row 3); its first bit is
    // op_ff_reset_val (chain index 19).
    let mut d = Design::new(&f);
    d.set_clb(&f, 6, 3, "op_ff_reset_val", 1).unwrap();
    let bits = export_bits(&f, &d);
    assert!(bits[21]);
    assert_eq!(bits.iter().filter(|&&b| b).count(), 1);

    // Last transmitted bit lands in CLB(0,0)'s input_mux_a_sel.
    let mut d = Design::new(&f);
    d.set_clb(&f, 0, 0, "input_mux_a_sel", 1).unwrap();
    let bits = export_bits(&f, &d);
    assert!(bits[580]);
    assert_eq!(bits.iter().filter(|&&b| b).count(), 1);

    // Multi-bit fields transmit MSB first: operation_select bit 2 of CLB(0,0)
    // is chain index 6, so 13 bits into that cell's 20 (561 + 13).
    let mut d = Design::new(&f);
    d.set_clb(&f, 0, 0, "operation_select", 0b100).unwrap();
    let bits = export_bits(&f, &d);
    assert!(bits[574]);
    assert_eq!(bits.iter().filter(|&&b| b).count(), 1);
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
    for row in 0..f.rows {
        for col in 0..f.columns {
            for i in 0..f.clb_fields.len() {
                let width = f.clb_fields[i].width;
                d.clb_mut(col, row).set(i, width, next());
            }
        }
    }
    for col in 0..f.columns {
        for i in 0..f.csb_fields.len() {
            let width = f.csb_fields[i].width;
            d.csb_mut(col).set(i, width, next());
        }
    }

    let bits = export_bits(&f, &d);
    let imported = import_bits(&f, &bits).unwrap();
    assert_eq!(export_bits(&f, &imported), bits);
    assert_eq!(
        imported.get_clb(&f, 3, 2, "major_horz2_sel").unwrap(),
        d.get_clb(&f, 3, 2, "major_horz2_sel").unwrap()
    );
    assert_eq!(
        imported.get_csb(&f, 5, "bus_addr").unwrap(),
        d.get_csb(&f, 5, "bus_addr").unwrap()
    );
}

#[test]
fn rejects_wrong_length() {
    let f = fabric();
    let err = import_bits(&f, &vec![false; 100]).unwrap_err();
    assert!(err.message.contains("581"), "got: {}", err.message);
}

#[test]
fn text_format_round_trip() {
    let f = fabric();
    let mut d = Design::new(&f);
    d.set_clb(&f, 2, 1, "operation_select", 5).unwrap();
    d.set_csb(&f, 3, "bus_addr", 1).unwrap();

    let text = format_text(&f, &d, "my design", "2026-08-28 12:00:00", false, None);
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("# my design"));
    assert_eq!(lines.next(), Some("# 2026-08-28 12:00:00"));
    let body = lines.next().unwrap();
    assert_eq!(body.len(), 581);
    assert!(lines.next().is_none(), "single line by default");

    let bits = parse_text(&text).unwrap();
    assert_eq!(bits, export_bits(&f, &d));
    let reimported = import_bits(&f, &bits).unwrap();
    assert_eq!(reimported.get_clb(&f, 2, 1, "operation_select").unwrap(), 5);
    assert_eq!(reimported.get_csb(&f, 3, "bus_addr").unwrap(), 1);

    // Raw mode has no header; wrapping splits the body only.
    let raw = format_text(&f, &d, "my design", "now", true, Some(100));
    assert!(!raw.starts_with('#'));
    assert_eq!(raw.lines().count(), 6); // ceil(581 / 100)
    assert_eq!(parse_text(&raw).unwrap(), export_bits(&f, &d));
}

#[test]
fn parse_rejects_garbage() {
    let err = parse_text("0101x01").unwrap_err();
    assert!(err.message.contains('x'), "got: {}", err.message);
}
