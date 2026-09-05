//! Versioned, human-readable JSON design file: configuration, pinned names,
//! stimulus, tick, trace flags and opaque view state. Loading against a
//! different fabric produces clear warnings (or an error when the grid no
//! longer fits) instead of silently misconfiguring.

use crate::config::{BlockId, Design};
use crate::fabric::Fabric;
use crate::sim::Stimulus;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

pub const FILE_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct FileError {
    pub message: String,
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for FileError {}

/// Everything a saved design carries besides the fabric description itself.
#[derive(Debug, Clone)]
pub struct DesignFile {
    pub name: String,
    pub design: Design,
    pub stimulus: Stimulus,
    pub tick: u64,
    /// Trace flags, as the wire identifiers used by the waveform/VCD layer.
    pub traces: Vec<String>,
    /// Opaque GUI view state (pan/zoom/selection); core never interprets it.
    pub view: Option<serde_json::Value>,
}

impl DesignFile {
    pub fn new(fabric: &Fabric, name: &str) -> Self {
        DesignFile {
            name: name.to_string(),
            design: Design::new(fabric),
            stimulus: Stimulus::default(),
            tick: 0,
            traces: Vec::new(),
            view: None,
        }
    }
}

// ---------------------------------------------------------------------------
// On-disk schema

#[derive(Serialize, Deserialize)]
struct FileSchema {
    version: u32,
    fabric: FabricRef,
    name: String,
    /// Sparse: only non-zero fields, only non-default blocks. Keys "col,row".
    #[serde(default)]
    clbs: BTreeMap<String, BTreeMap<String, u64>>,
    /// Keys "col".
    #[serde(default)]
    csbs: BTreeMap<String, BTreeMap<String, u64>>,
    /// Keys "clb:col,row" / "csb:col".
    #[serde(default)]
    pinned: BTreeMap<String, String>,
    #[serde(default)]
    stimulus: BTreeMap<String, Vec<u8>>,
    #[serde(default)]
    tick: u64,
    #[serde(default)]
    traces: Vec<String>,
    #[serde(default)]
    view: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize, PartialEq)]
struct FabricRef {
    name: String,
    columns: usize,
    rows: usize,
    clb_bits: usize,
    csb_bits: usize,
}

impl FabricRef {
    fn of(fabric: &Fabric) -> Self {
        FabricRef {
            name: fabric.name.clone(),
            columns: fabric.columns,
            rows: fabric.rows,
            clb_bits: fabric.clb_bits(),
            csb_bits: fabric.csb_bits(),
        }
    }
}

pub fn save_design(fabric: &Fabric, file: &DesignFile) -> String {
    let mut clbs = BTreeMap::new();
    for row in 0..fabric.rows {
        for col in 0..fabric.columns {
            let mut fields = BTreeMap::new();
            for (i, field) in fabric.clb_fields.iter().enumerate() {
                let v = file.design.clb(col, row).get(i);
                if v != 0 {
                    fields.insert(field.name.clone(), v);
                }
            }
            if !fields.is_empty() {
                clbs.insert(format!("{},{}", col, row), fields);
            }
        }
    }
    let mut csbs = BTreeMap::new();
    for col in 0..fabric.columns {
        let mut fields = BTreeMap::new();
        for (i, field) in fabric.csb_fields.iter().enumerate() {
            let v = file.design.csb(col).get(i);
            if v != 0 {
                fields.insert(field.name.clone(), v);
            }
        }
        if !fields.is_empty() {
            csbs.insert(col.to_string(), fields);
        }
    }
    let mut pinned = BTreeMap::new();
    for (block, name) in file.design.pinned() {
        let key = match block {
            BlockId::Clb { col, row } => format!("clb:{},{}", col, row),
            BlockId::Csb { col } => format!("csb:{}", col),
        };
        pinned.insert(key, name.to_string());
    }
    let stimulus = file
        .stimulus
        .iter()
        .map(|(n, p)| (n.to_string(), p.iter().map(|&b| b as u8).collect()))
        .collect();
    let schema = FileSchema {
        version: FILE_VERSION,
        fabric: FabricRef::of(fabric),
        name: file.name.clone(),
        clbs,
        csbs,
        pinned,
        stimulus,
        tick: file.tick,
        traces: file.traces.clone(),
        view: file.view.clone(),
    };
    // BTreeMaps and struct order make this deterministic; serialization of
    // plain data cannot fail.
    serde_json::to_string_pretty(&schema).unwrap_or_default()
}

/// Load a design. Returns the file plus human-readable warnings for anything
/// that did not carry over (unknown fields, truncated values, bad pins).
pub fn load_design(fabric: &Fabric, src: &str) -> Result<(DesignFile, Vec<String>), FileError> {
    let schema: FileSchema = serde_json::from_str(src).map_err(|e| FileError {
        message: format!("not a valid design file: {}", e),
    })?;
    if schema.version != FILE_VERSION {
        return Err(FileError {
            message: format!(
                "design file version {} is not supported (this build reads version {})",
                schema.version, FILE_VERSION
            ),
        });
    }
    let mut warnings = Vec::new();
    let current = FabricRef::of(fabric);
    if schema.fabric.columns != current.columns || schema.fabric.rows != current.rows {
        return Err(FileError {
            message: format!(
                "design was built for a {}x{} fabric; the loaded fabric is {}x{}",
                schema.fabric.columns, schema.fabric.rows, current.columns, current.rows
            ),
        });
    }
    if schema.fabric != current {
        warnings.push(format!(
            "design was saved against fabric \"{}\" ({} CLB bits, {} CSB bits); \
             now loading \"{}\" ({} CLB bits, {} CSB bits) — check the result carefully",
            schema.fabric.name,
            schema.fabric.clb_bits,
            schema.fabric.csb_bits,
            current.name,
            current.clb_bits,
            current.csb_bits
        ));
    }

    let mut design = Design::new(fabric);
    for (key, fields) in &schema.clbs {
        let Some((col, row)) = parse_pair(key) else {
            warnings.push(format!("ignoring CLB entry with bad key \"{}\"", key));
            continue;
        };
        if col >= fabric.columns || row >= fabric.rows {
            warnings.push(format!("ignoring CLB ({}, {}): outside the grid", col, row));
            continue;
        }
        for (fname, &value) in fields {
            match design.set_clb(fabric, col, row, fname, value) {
                Ok(stored) if stored != value => warnings.push(format!(
                    "CLB ({}, {}) field \"{}\": value {} truncated to {}",
                    col, row, fname, value, stored
                )),
                Ok(_) => {}
                Err(_) => warnings.push(format!(
                    "CLB ({}, {}): unknown field \"{}\" ignored",
                    col, row, fname
                )),
            }
        }
    }
    for (key, fields) in &schema.csbs {
        let Ok(col) = key.parse::<usize>() else {
            warnings.push(format!("ignoring CSB entry with bad key \"{}\"", key));
            continue;
        };
        if col >= fabric.columns {
            warnings.push(format!("ignoring CSB {}: outside the grid", col));
            continue;
        }
        for (fname, &value) in fields {
            match design.set_csb(fabric, col, fname, value) {
                Ok(stored) if stored != value => warnings.push(format!(
                    "CSB {} field \"{}\": value {} truncated to {}",
                    col, fname, value, stored
                )),
                Ok(_) => {}
                Err(_) => warnings.push(format!("CSB {}: unknown field \"{}\" ignored", col, fname)),
            }
        }
    }
    for (key, name) in &schema.pinned {
        let block = key
            .strip_prefix("clb:")
            .and_then(parse_pair_str)
            .map(|(col, row)| BlockId::Clb { col, row })
            .or_else(|| key.strip_prefix("csb:").and_then(|c| c.parse().ok()).map(|col| BlockId::Csb { col }));
        let Some(block) = block else {
            warnings.push(format!("ignoring pinned name with bad key \"{}\"", key));
            continue;
        };
        let in_grid = match block {
            BlockId::Clb { col, row } => col < fabric.columns && row < fabric.rows,
            BlockId::Csb { col } => col < fabric.columns,
        };
        if !in_grid {
            warnings.push(format!("ignoring pinned name \"{}\": block outside the grid", name));
            continue;
        }
        if let Err(e) = design.rename(fabric, block, name) {
            warnings.push(format!("could not restore pinned name \"{}\": {}", name, e));
        }
    }

    let mut stimulus = Stimulus::default();
    for (name, pattern) in &schema.stimulus {
        stimulus.set(name, pattern.iter().map(|&v| v != 0).collect());
    }

    Ok((
        DesignFile {
            name: schema.name,
            design,
            stimulus,
            tick: schema.tick,
            traces: schema.traces,
            view: schema.view,
        },
        warnings,
    ))
}

fn parse_pair(key: &str) -> Option<(usize, usize)> {
    let (a, b) = key.split_once(',')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

fn parse_pair_str(key: &str) -> Option<(usize, usize)> {
    parse_pair(key)
}
