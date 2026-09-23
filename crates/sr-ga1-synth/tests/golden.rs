//! Format drift guard.
//!
//! The synthesiser and the visual programmer share no code, so the only
//! thing keeping their two file formats identical is this test: take files
//! the GUI exported, read them with this tool, write them back, and require
//! byte-for-byte equality. If either side changes its schema, this fails
//! loudly instead of producing a design file the GUI silently mis-reads.

use sr_ga1_synth::bitstream;
use sr_ga1_synth::designjson::{load_design, save_design};
use sr_ga1_synth::fabric::Fabric;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is <repo>/crates/sr-ga1-synth.
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn fabric() -> Fabric {
    let path = repo_root().join("fabric.toml");
    Fabric::load_file(&path).expect("the repository fabric.toml must load")
}

fn golden(name: &str) -> (PathBuf, String) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("golden").join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read golden file {}: {}", path.display(), e));
    (path, text)
}

#[test]
fn fabric_matches_the_documented_geometry() {
    let f = fabric();
    assert_eq!(f.name, "SR-GA1");
    assert_eq!((f.columns, f.rows), (7, 4));
    assert_eq!(f.clb_bits(), 20);
    assert_eq!(f.csb_bits(), 3);
    assert_eq!(f.total_bits(), 581);
    assert_eq!(f.operations.len(), 8);
    assert_eq!(f.operation("MUX2").map(|o| o.code), Some(7));
}

#[test]
fn design_json_round_trips_byte_for_byte() {
    let f = fabric();
    let (path, text) = golden("adder4.json");
    let (design, file) = load_design(&f, &text, &path).expect("golden design file must load");
    assert_eq!(save_design(&f, &design, &file), text);
}

#[test]
fn bitstream_matches_the_gui_export() {
    let f = fabric();
    let (json_path, json) = golden("adder4.json");
    let (design, file) = load_design(&f, &json, &json_path).expect("golden design file must load");

    let (_, expected) = golden("adder4.txt");
    let timestamp = expected
        .lines()
        .nth(1)
        .and_then(|l| l.strip_prefix("# "))
        .expect("golden bitstream must carry a timestamp comment");
    let emitted = bitstream::format_text(&f, &design, &file.name, timestamp, false);
    assert_eq!(emitted, expected);
}

#[test]
fn raw_mode_emits_bits_only() {
    let f = fabric();
    let (json_path, json) = golden("adder4.json");
    let (design, file) = load_design(&f, &json, &json_path).expect("golden design file must load");

    let raw = bitstream::format_text(&f, &design, &file.name, "unused", true);
    assert!(!raw.starts_with('#'));
    assert_eq!(raw.trim_end().len(), f.total_bits());
}

#[test]
fn bitstream_round_trips_through_the_decoder() {
    let f = fabric();
    let (path, json) = golden("adder4.json");
    let (design, _) = load_design(&f, &json, &path).expect("golden design file must load");

    let bits = bitstream::export_bits(&f, &design);
    assert_eq!(bits.len(), f.total_bits());
    let decoded = bitstream::import_bits(&f, &bits).expect("a freshly exported bitstream decodes");
    assert_eq!(bitstream::export_bits(&f, &decoded), bits);

    // And via the text form, which is what actually reaches the programmer.
    let text = bitstream::format_text(&f, &design, "round trip", "now", false);
    let parsed = bitstream::parse_text(&text).expect("emitted bitstream text must parse");
    assert_eq!(parsed, bits);
}

#[test]
fn a_wrong_length_bitstream_is_refused() {
    let f = fabric();
    let err = bitstream::import_bits(&f, &vec![false; f.total_bits() - 1]).unwrap_err();
    assert!(err.message.contains("581"), "message should quote the expected length: {}", err);
}
