//! The configuration a synthesis run produces: one field vector per CLB and
//! CSB, plus the block names carried through to the design file.
//!
//! This is the handover point between the backend and the two emitters. It
//! knows nothing about nets or cells — everything above it has been resolved
//! into configuration field values by the time a `Design` exists.

use crate::fabric::Fabric;
use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BlockId {
    Clb { col: usize, row: usize },
    Csb { col: usize },
}

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlockId::Clb { col, row } => write!(f, "CLB({}, {})", col, row),
            BlockId::Csb { col } => write!(f, "CSB{}", col),
        }
    }
}

impl BlockId {
    /// Key used in the design file's `pinned` map.
    pub fn json_key(&self) -> String {
        match self {
            BlockId::Clb { col, row } => format!("clb:{},{}", col, row),
            BlockId::Csb { col } => format!("csb:{}", col),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfigError {
    pub message: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ConfigError {}

/// One cell's configuration: a value per declared field, all defaulting to 0.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BlockConfig {
    values: Vec<u64>,
}

impl BlockConfig {
    fn new(fields: usize) -> Self {
        BlockConfig { values: vec![0; fields] }
    }
    pub fn get(&self, field: usize) -> u64 {
        self.values.get(field).copied().unwrap_or(0)
    }
    /// Stores `value` truncated to the field width, returning what was kept.
    pub fn set(&mut self, field: usize, width: usize, value: u64) -> u64 {
        let mask = if width >= 64 { u64::MAX } else { (1u64 << width) - 1 };
        let stored = value & mask;
        if let Some(slot) = self.values.get_mut(field) {
            *slot = stored;
        }
        stored
    }
    pub fn is_default(&self) -> bool {
        self.values.iter().all(|&v| v == 0)
    }
}

#[derive(Debug, Clone)]
pub struct Design {
    columns: usize,
    rows: usize,
    clbs: Vec<BlockConfig>,
    csbs: Vec<BlockConfig>,
    names: BTreeMap<BlockId, String>,
    /// Which bits of each CLB field were written deliberately, as opposed to
    /// left at their default. Indexed `[clb][field]`, so two routing
    /// decisions sharing one field (lane 0 and lane 1 both live in
    /// `minor_horz_sel`) do not look like a conflict.
    written: Vec<Vec<u64>>,
}

impl Design {
    pub fn new(fabric: &Fabric) -> Self {
        Design {
            columns: fabric.columns,
            rows: fabric.rows,
            clbs: vec![BlockConfig::new(fabric.clb_fields.len()); fabric.clb_count()],
            csbs: vec![BlockConfig::new(fabric.csb_fields.len()); fabric.columns],
            names: BTreeMap::new(),
            written: vec![vec![0; fabric.clb_fields.len()]; fabric.clb_count()],
        }
    }

    fn clb_index(&self, col: usize, row: usize) -> usize {
        row * self.columns + col
    }

    pub fn clb(&self, col: usize, row: usize) -> &BlockConfig {
        &self.clbs[self.clb_index(col, row)]
    }
    pub fn clb_mut(&mut self, col: usize, row: usize) -> &mut BlockConfig {
        let i = self.clb_index(col, row);
        &mut self.clbs[i]
    }
    pub fn csb(&self, col: usize) -> &BlockConfig {
        &self.csbs[col]
    }
    pub fn csb_mut(&mut self, col: usize) -> &mut BlockConfig {
        &mut self.csbs[col]
    }

    /// Set a CLB field by name. Fails rather than silently dropping a value
    /// that does not fit, since a truncated mux select is a miscompile.
    pub fn set_clb(
        &mut self,
        fabric: &Fabric,
        col: usize,
        row: usize,
        field: &str,
        value: u64,
    ) -> Result<(), ConfigError> {
        if col >= fabric.columns || row >= fabric.rows {
            return Err(ConfigError {
                message: format!(
                    "CLB ({}, {}) is outside the {}x{} grid",
                    col, row, fabric.columns, fabric.rows
                ),
            });
        }
        let Some(idx) = fabric.clb_field(field) else {
            return Err(ConfigError {
                message: format!("the CLB has no configuration field named \"{}\"", field),
            });
        };
        let spec = &fabric.clb_fields[idx];
        if value > spec.max() {
            return Err(ConfigError {
                message: format!(
                    "CLB ({}, {}) field \"{}\" cannot hold {}; it is {} bits wide",
                    col, row, field, value, spec.width
                ),
            });
        }
        self.clb_mut(col, row).set(idx, spec.width, value);
        Ok(())
    }

    pub fn set_csb(
        &mut self,
        fabric: &Fabric,
        col: usize,
        field: &str,
        value: u64,
    ) -> Result<(), ConfigError> {
        if col >= fabric.columns {
            return Err(ConfigError {
                message: format!("CSB {} is outside the {}-column grid", col, fabric.columns),
            });
        }
        let Some(idx) = fabric.csb_field(field) else {
            return Err(ConfigError {
                message: format!("the CSB has no configuration field named \"{}\"", field),
            });
        };
        let spec = &fabric.csb_fields[idx];
        if value > spec.max() {
            return Err(ConfigError {
                message: format!(
                    "CSB {} field \"{}\" cannot hold {}; it is {} bits wide",
                    col, field, value, spec.width
                ),
            });
        }
        self.csb_mut(col).set(idx, spec.width, value);
        Ok(())
    }

    /// Write a mux select, which may be one bit of a shared field —
    /// `minor_horz_sel[0]` drives lane 0 and `[1]` drives lane 1, so two
    /// independent routing decisions live in one field.
    ///
    /// Returns the value already there if it disagrees, which is how the
    /// emitter detects two routes claiming the same mux.
    pub fn apply_slice(
        &mut self,
        fabric: &Fabric,
        col: usize,
        row: usize,
        slice: crate::fabric::FieldSlice,
        value: u64,
    ) -> Result<(), u64> {
        let spec = &fabric.clb_fields[slice.field];
        let index = self.clb_index(col, row);
        let current = self.clb(col, row).get(slice.field);
        let (mask, shifted) = match slice.bit {
            Some(bit) => (1u64 << bit, (value & 1) << bit),
            None => (spec.max(), value & spec.max()),
        };
        let already = self.written[index][slice.field];
        if already & mask != 0 && current & mask != shifted {
            return Err((current & mask) >> slice.bit.unwrap_or(0));
        }
        let updated = (current & !mask) | shifted;
        self.clb_mut(col, row).set(slice.field, spec.width, updated);
        self.written[index][slice.field] |= mask;
        Ok(())
    }

    /// Whether a slice has been deliberately written, as opposed to left at
    /// its default.
    pub fn slice_written(&self, col: usize, row: usize, slice: crate::fabric::FieldSlice) -> bool {
        let mask = match slice.bit {
            Some(bit) => 1u64 << bit,
            None => u64::MAX,
        };
        self.written[self.clb_index(col, row)][slice.field] & mask != 0
    }

    pub fn get_clb(&self, fabric: &Fabric, col: usize, row: usize, field: &str) -> Option<u64> {
        fabric.clb_field(field).map(|i| self.clb(col, row).get(i))
    }
    pub fn get_csb(&self, fabric: &Fabric, col: usize, field: &str) -> Option<u64> {
        fabric.csb_field(field).map(|i| self.csb(col).get(i))
    }

    /// Give a block a name. Names are emitted pinned, so the GUI keeps them.
    pub fn name(&mut self, block: BlockId, name: impl Into<String>) {
        self.names.insert(block, name.into());
    }
    pub fn names(&self) -> impl Iterator<Item = (&BlockId, &String)> {
        self.names.iter()
    }
    pub fn name_of(&self, block: BlockId) -> Option<&str> {
        self.names.get(&block).map(|s| s.as_str())
    }

    pub fn columns(&self) -> usize {
        self.columns
    }
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// CLBs holding a non-default configuration — the utilisation figure the
    /// report quotes, and what the GUI counts as "used".
    pub fn used_clbs(&self) -> usize {
        self.clbs.iter().filter(|c| !c.is_default()).count()
    }
}
