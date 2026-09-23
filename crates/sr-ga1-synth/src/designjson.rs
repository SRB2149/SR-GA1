//! Design file emitter and reader — the JSON the visual programmer opens.
//!
//! The two tools share no code, so this schema is written out independently
//! and held to the GUI's format by the golden tests. Anything the
//! synthesiser does not produce (view state, trace flags) is emitted in its
//! default form so a round trip through the GUI is lossless.

use crate::design::{BlockId, Design};
use crate::fabric::Fabric;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

pub const FILE_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct DesignFileError {
    pub path: PathBuf,
    pub message: String,
}

impl fmt::Display for DesignFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

impl std::error::Error for DesignFileError {}

/// Stimulus vectors the GUI replays, keyed by chip input net name.
pub type Stimulus = BTreeMap<String, Vec<u8>>;

/// One loop-around board wire the configuration depends on.
///
/// This is an extension to the format the visual programmer writes. It is the
/// last field and is omitted when empty, so a design with no loop-around wiring
/// still round-trips byte-for-byte against the GUI's own exports — which is what
/// the golden tests check. The GUI ignores unknown fields, so it will load a
/// file that has this section, but it neither simulates the wiring nor preserves
/// the field when saving. See `docs/gui-loopback-support.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopbackLink {
    /// Chip output pad the wire starts at.
    pub from: String,
    /// Chip input pad it returns to.
    pub to: String,
    /// Net being carried, for readability.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub net: String,
}

impl fmt::Display for LoopbackLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} -> {}", self.from, self.to)
    }
}

#[derive(Serialize, Deserialize)]
struct FileSchema {
    version: u32,
    fabric: FabricRef,
    name: String,
    /// Sparse: non-zero fields of non-default blocks only. Keys "col,row".
    #[serde(default)]
    clbs: BTreeMap<String, BTreeMap<String, u64>>,
    /// Keys "col".
    #[serde(default)]
    csbs: BTreeMap<String, BTreeMap<String, u64>>,
    /// Keys "clb:col,row" / "csb:col".
    #[serde(default)]
    pinned: BTreeMap<String, String>,
    #[serde(default)]
    stimulus: Stimulus,
    #[serde(default)]
    tick: u64,
    #[serde(default)]
    traces: Vec<String>,
    #[serde(default)]
    view: Option<serde_json::Value>,
    /// Loop-around board wiring. Last, and omitted when empty, so the format
    /// stays byte-identical to the GUI's for designs that do not use it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    loopback: Vec<LoopbackLink>,
    /// Chip pad -> the design signal on it. The GUI's `pinned` map names blocks
    /// only, so without this the IO shows as `input_8` rather than `clk`.
    /// Omitted when empty, like `loopback`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    io_names: BTreeMap<String, String>,
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

/// Everything the design file carries beyond the configuration itself.
#[derive(Debug, Clone, Default)]
pub struct DesignFile {
    pub name: String,
    pub stimulus: Stimulus,
    pub tick: u64,
    pub traces: Vec<String>,
    /// Board wiring the configuration depends on, if any.
    pub loopback: Vec<LoopbackLink>,
    /// Chip pad -> design signal name, so the IO reads as the designer wrote it.
    pub io_names: BTreeMap<String, String>,
}

/// Render the design file. Deterministic: every map is ordered, so the same
/// configuration always produces the same bytes.
pub fn save_design(fabric: &Fabric, design: &Design, file: &DesignFile) -> String {
    let mut clbs = BTreeMap::new();
    for row in 0..fabric.rows {
        for col in 0..fabric.columns {
            let mut fields = BTreeMap::new();
            for (i, field) in fabric.clb_fields.iter().enumerate() {
                let v = design.clb(col, row).get(i);
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
            let v = design.csb(col).get(i);
            if v != 0 {
                fields.insert(field.name.clone(), v);
            }
        }
        if !fields.is_empty() {
            csbs.insert(col.to_string(), fields);
        }
    }
    let pinned: BTreeMap<String, String> =
        design.names().map(|(b, n)| (b.json_key(), n.clone())).collect();

    let schema = FileSchema {
        version: FILE_VERSION,
        fabric: FabricRef::of(fabric),
        name: file.name.clone(),
        clbs,
        csbs,
        pinned,
        stimulus: file.stimulus.clone(),
        tick: file.tick,
        traces: file.traces.clone(),
        view: None,
        loopback: file.loopback.clone(),
        io_names: file.io_names.clone(),
    };
    // Plain data with ordered maps: serialisation cannot fail and cannot vary.
    serde_json::to_string_pretty(&schema).unwrap_or_default()
}

/// Read a design file back — used by the golden tests and by `--check`
/// against an existing design.
pub fn load_design(
    fabric: &Fabric,
    src: &str,
    path: &Path,
) -> Result<(Design, DesignFile), DesignFileError> {
    let fail = |message: String| DesignFileError { path: path.to_path_buf(), message };
    let schema: FileSchema =
        serde_json::from_str(src).map_err(|e| fail(format!("not a valid design file: {}", e)))?;
    if schema.version != FILE_VERSION {
        return Err(fail(format!(
            "design file version {} is not supported (this build reads version {})",
            schema.version, FILE_VERSION
        )));
    }
    let current = FabricRef::of(fabric);
    if schema.fabric != current {
        return Err(fail(format!(
            "design was saved against fabric \"{}\" ({}x{}, {} CLB bits, {} CSB bits); \
             the loaded fabric is \"{}\" ({}x{}, {} CLB bits, {} CSB bits)",
            schema.fabric.name,
            schema.fabric.columns,
            schema.fabric.rows,
            schema.fabric.clb_bits,
            schema.fabric.csb_bits,
            current.name,
            current.columns,
            current.rows,
            current.clb_bits,
            current.csb_bits
        )));
    }

    let mut design = Design::new(fabric);
    for (key, fields) in &schema.clbs {
        let Some((col, row)) = parse_pair(key) else {
            return Err(fail(format!("CLB entry \"{}\" is not a \"col,row\" key", key)));
        };
        for (name, &value) in fields {
            design
                .set_clb(fabric, col, row, name, value)
                .map_err(|e| fail(e.message))?;
        }
    }
    for (key, fields) in &schema.csbs {
        let Ok(col) = key.parse::<usize>() else {
            return Err(fail(format!("CSB entry \"{}\" is not a column number", key)));
        };
        for (name, &value) in fields {
            design.set_csb(fabric, col, name, value).map_err(|e| fail(e.message))?;
        }
    }
    for (key, name) in &schema.pinned {
        let block = parse_block_key(key)
            .ok_or_else(|| fail(format!("pinned name \"{}\" has an unrecognised key", key)))?;
        let in_grid = match block {
            BlockId::Clb { col, row } => col < fabric.columns && row < fabric.rows,
            BlockId::Csb { col } => col < fabric.columns,
        };
        if !in_grid {
            return Err(fail(format!("pinned name \"{}\" names a block outside the grid", name)));
        }
        design.name(block, name.clone());
    }

    Ok((
        design,
        DesignFile {
            name: schema.name,
            stimulus: schema.stimulus,
            tick: schema.tick,
            traces: schema.traces,
            loopback: schema.loopback,
            io_names: schema.io_names,
        },
    ))
}

fn parse_pair(key: &str) -> Option<(usize, usize)> {
    let (a, b) = key.split_once(',')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

fn parse_block_key(key: &str) -> Option<BlockId> {
    if let Some(rest) = key.strip_prefix("clb:") {
        let (col, row) = parse_pair(rest)?;
        return Some(BlockId::Clb { col, row });
    }
    if let Some(rest) = key.strip_prefix("csb:") {
        return rest.trim().parse().ok().map(|col| BlockId::Csb { col });
    }
    None
}
