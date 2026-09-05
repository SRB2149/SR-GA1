//! Design file save/load tests: full round trip, fabric-mismatch handling,
//! and tolerance of unknown fields with warnings.

use fpga_core::bitstream::export_bits;
use fpga_core::config::BlockId;
use fpga_core::designfile::{load_design, save_design, DesignFile};
use fpga_core::fabric::Fabric;

fn fabric() -> Fabric {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fabric.toml");
    Fabric::load_file(std::path::Path::new(path)).expect("fabric.toml must load")
}

fn sample(f: &Fabric) -> DesignFile {
    let mut file = DesignFile::new(f, "counter");
    file.design.set_clb(f, 2, 1, "operation_select", 6).unwrap();
    file.design.set_clb(f, 0, 3, "minor_horz_sel", 0b11).unwrap();
    file.design.set_csb(f, 4, "couple_to_previous", 1).unwrap();
    file.design.rename(f, BlockId::Clb { col: 2, row: 1 }, "adder").unwrap();
    file.stimulus.set("input_0", vec![false, true]);
    file.stimulus.set("input_3", vec![true]);
    file.tick = 17;
    file.traces = vec!["out:0:0".to_string()];
    file
}

#[test]
fn save_load_round_trip() {
    let f = fabric();
    let file = sample(&f);
    let json = save_design(&f, &file);
    let (loaded, warnings) = load_design(&f, &json).unwrap();
    assert!(warnings.is_empty(), "unexpected warnings: {:?}", warnings);
    assert_eq!(loaded.name, "counter");
    assert_eq!(loaded.tick, 17);
    assert_eq!(loaded.traces, file.traces);
    assert_eq!(export_bits(&f, &loaded.design), export_bits(&f, &file.design));
    assert_eq!(loaded.design.pinned_name(BlockId::Clb { col: 2, row: 1 }), Some("adder"));
    assert_eq!(loaded.stimulus.pattern("input_0"), Some(&[false, true][..]));
    assert_eq!(loaded.stimulus.pattern("input_3"), Some(&[true][..]));
}

#[test]
fn sparse_file_omits_defaults() {
    let f = fabric();
    let json = save_design(&f, &DesignFile::new(&f, "empty"));
    assert!(!json.contains("\"0,0\""), "default blocks must not be serialized");
    let (loaded, warnings) = load_design(&f, &json).unwrap();
    assert!(warnings.is_empty());
    assert!(export_bits(&f, &loaded.design).iter().all(|&b| !b));
}

#[test]
fn unknown_field_warns_but_loads() {
    let f = fabric();
    let json = save_design(&f, &sample(&f)).replace("\"operation_select\"", "\"operation_selekt\"");
    let (loaded, warnings) = load_design(&f, &json).unwrap();
    assert!(
        warnings.iter().any(|w| w.contains("operation_selekt")),
        "warnings: {:?}",
        warnings
    );
    assert_eq!(loaded.design.get_clb(&f, 2, 1, "operation_select").unwrap(), 0);
}

#[test]
fn grid_mismatch_is_an_error() {
    let f = fabric();
    let json = save_design(&f, &sample(&f)).replace("\"columns\": 7", "\"columns\": 12");
    let err = load_design(&f, &json).unwrap_err();
    assert!(err.message.contains("12x4"), "got: {}", err.message);
}

#[test]
fn other_fabric_differences_warn() {
    let f = fabric();
    let json = save_design(&f, &sample(&f)).replace("\"clb_bits\": 20", "\"clb_bits\": 23");
    let (_, warnings) = load_design(&f, &json).unwrap();
    assert!(
        warnings.iter().any(|w| w.contains("check the result carefully")),
        "warnings: {:?}",
        warnings
    );
}

#[test]
fn unsupported_version_is_an_error() {
    let f = fabric();
    let json = save_design(&f, &sample(&f)).replace("\"version\": 1", "\"version\": 99");
    let err = load_design(&f, &json).unwrap_err();
    assert!(err.message.contains("99"), "got: {}", err.message);
}

#[test]
fn malformed_json_is_an_error_not_a_panic() {
    let f = fabric();
    assert!(load_design(&f, "{ not json").is_err());
    assert!(load_design(&f, "").is_err());
}
