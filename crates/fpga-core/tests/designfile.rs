//! Design file save/load tests: full round trip, fabric-mismatch and version
//! refusal, and tolerance of unknown fields with warnings.

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
    file.design.set_clb(f, 2, 1, "lut", 0xEA).unwrap();
    file.design.set_clb(f, 0, 3, "minor_horz_sel", 0b11).unwrap();
    file.design.set_csb(f, 4, "couple_to_previous", 1).unwrap();
    file.design.set_io(f, "input_mux_sel1a", 12).unwrap();
    file.design.set_io(f, "output_mux_sel2", 7).unwrap();
    file.design.rename(f, BlockId::Clb { col: 2, row: 1 }, "adder").unwrap();
    file.design.rename(f, BlockId::Io, "pins").unwrap();
    file.stimulus.set("input_0", vec![false, true]);
    file.stimulus.set("input_3", vec![true]);
    file.tick = 17;
    file.traces = vec!["out:output_0".to_string()];
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
    assert_eq!(loaded.design.pinned_name(BlockId::Io), Some("pins"));
    assert_eq!(loaded.design.get_io(&f, "input_mux_sel1a").unwrap(), 12);
    assert!(json.contains("\"io\": {"), "IO config is written:\n{}", json);
    assert_eq!(loaded.stimulus.pattern("input_0"), Some(&[false, true][..]));
    assert_eq!(loaded.stimulus.pattern("input_3"), Some(&[true][..]));
}

#[test]
fn sparse_file_omits_defaults() {
    let f = fabric();
    let json = save_design(&f, &DesignFile::new(&f, "empty"));
    assert!(!json.contains("\"0,0\""), "default blocks must not be serialized");
    assert!(!json.contains("\"io\""), "an all-default IO controller must not be serialized");
    let (loaded, warnings) = load_design(&f, &json).unwrap();
    assert!(warnings.is_empty());
    assert!(export_bits(&f, &loaded.design).iter().skip(1).all(|&b| !b));
}

#[test]
fn unknown_field_warns_but_loads() {
    let f = fabric();
    let json = save_design(&f, &sample(&f)).replace("\"lut\"", "\"lutt\"");
    let (loaded, warnings) = load_design(&f, &json).unwrap();
    assert!(warnings.iter().any(|w| w.contains("lutt")), "warnings: {:?}", warnings);
    assert_eq!(loaded.design.get_clb(&f, 2, 1, "lut").unwrap(), 0);
}

#[test]
fn grid_mismatch_is_an_error() {
    let f = fabric();
    let json = save_design(&f, &sample(&f)).replace("\"columns\": 8", "\"columns\": 12");
    let err = load_design(&f, &json).unwrap_err();
    assert!(err.message.contains("12x4"), "got: {}", err.message);
}

#[test]
fn a_config_layout_mismatch_is_an_error() {
    let f = fabric();
    let json = save_design(&f, &sample(&f)).replace("\"clb_bits\": 25", "\"clb_bits\": 20");
    let err = load_design(&f, &json).unwrap_err();
    assert!(err.message.contains("20/64/3"), "got: {}", err.message);
}

#[test]
fn a_fabric_name_difference_warns() {
    let f = fabric();
    let json = save_design(&f, &sample(&f)).replace("\"name\": \"SR-GA1\"", "\"name\": \"SR-GA0\"");
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
    let json = save_design(&f, &sample(&f)).replace("\"version\": 2", "\"version\": 99");
    let err = load_design(&f, &json).unwrap_err();
    assert!(err.message.contains("99"), "got: {}", err.message);
}

/// A file from before the LUT CLB and the configurable IO controller is
/// refused by its version, before any of its stale fields are looked at.
#[test]
fn a_version_1_file_is_refused() {
    let f = fabric();
    let old = r#"{
  "version": 1,
  "fabric": { "name": "SR-GA1", "columns": 7, "rows": 4, "clb_bits": 20, "csb_bits": 3 },
  "name": "old",
  "clbs": { "0,0": { "operation_select": 2 } }
}"#;
    let err = load_design(&f, old).unwrap_err();
    assert!(err.message.contains("version 1"), "got: {}", err.message);
    assert!(err.message.contains("rebuilt"), "got: {}", err.message);
}

#[test]
fn malformed_json_is_an_error_not_a_panic() {
    let f = fabric();
    assert!(load_design(&f, "{ not json").is_err());
    assert!(load_design(&f, "").is_err());
}
