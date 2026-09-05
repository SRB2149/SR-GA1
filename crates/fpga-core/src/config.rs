//! Design-side configuration state: per-cell config field values and the
//! user's pinned block names. GUI-free; the naming engine and bitstream codec
//! read from here.

use crate::fabric::{Fabric, Field, FieldSlice};
use std::collections::{HashMap, HashSet};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockId {
    Clb { col: usize, row: usize },
    Csb { col: usize },
}

/// The default (auto-generated) name for a block, before pinning.
pub fn default_name(fabric: &Fabric, block: BlockId) -> String {
    match block {
        BlockId::Clb { col, row } => fabric.clb_name(col, row),
        BlockId::Csb { col } => fabric.csb_name(col),
    }
}

/// Field values for one cell, indexed like the fabric's field list for that
/// cell kind. All zero by default, matching the fabric description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellConfig {
    values: Vec<u64>,
}

impl CellConfig {
    fn new(fields: &[Field]) -> Self {
        CellConfig {
            values: vec![0; fields.len()],
        }
    }

    pub fn get(&self, field: usize) -> u64 {
        self.values[field]
    }

    /// Store a value masked to `width` bits; returns what was stored.
    pub fn set(&mut self, field: usize, width: usize, value: u64) -> u64 {
        let mask = if width >= 64 { u64::MAX } else { (1u64 << width) - 1 };
        let v = value & mask;
        self.values[field] = v;
        v
    }

    /// Read a mux select: the whole field, or one bit of it.
    pub fn slice(&self, s: &FieldSlice) -> u64 {
        let v = self.values[s.field];
        match s.bit {
            Some(b) => (v >> b) & 1,
            None => v,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RenameError {
    pub message: String,
}

impl fmt::Display for RenameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for RenameError {}

/// A full fabric configuration: one [`CellConfig`] per CLB and CSB, plus the
/// pinned names. Grid indexing matches the fabric: row 0 at the bottom,
/// column 0 at the left.
#[derive(Debug, Clone)]
pub struct Design {
    pub columns: usize,
    pub rows: usize,
    clbs: Vec<CellConfig>,
    csbs: Vec<CellConfig>,
    pinned: HashMap<BlockId, String>,
}

impl Design {
    pub fn new(fabric: &Fabric) -> Self {
        Design {
            columns: fabric.columns,
            rows: fabric.rows,
            clbs: vec![CellConfig::new(&fabric.clb_fields); fabric.columns * fabric.rows],
            csbs: vec![CellConfig::new(&fabric.csb_fields); fabric.columns],
            pinned: HashMap::new(),
        }
    }

    fn clb_index(&self, col: usize, row: usize) -> usize {
        assert!(
            col < self.columns && row < self.rows,
            "CLB ({}, {}) out of range for {}x{} grid",
            col,
            row,
            self.columns,
            self.rows
        );
        row * self.columns + col
    }

    pub fn clb(&self, col: usize, row: usize) -> &CellConfig {
        &self.clbs[self.clb_index(col, row)]
    }

    pub fn clb_mut(&mut self, col: usize, row: usize) -> &mut CellConfig {
        let i = self.clb_index(col, row);
        &mut self.clbs[i]
    }

    pub fn csb(&self, col: usize) -> &CellConfig {
        assert!(col < self.columns, "CSB {} out of range", col);
        &self.csbs[col]
    }

    pub fn csb_mut(&mut self, col: usize) -> &mut CellConfig {
        assert!(col < self.columns, "CSB {} out of range", col);
        &mut self.csbs[col]
    }

    pub fn get_clb(&self, fabric: &Fabric, col: usize, row: usize, field: &str) -> Result<u64, String> {
        let idx = field_index(&fabric.clb_fields, field, "CLB")?;
        Ok(self.clb(col, row).get(idx))
    }

    pub fn set_clb(
        &mut self,
        fabric: &Fabric,
        col: usize,
        row: usize,
        field: &str,
        value: u64,
    ) -> Result<u64, String> {
        let idx = field_index(&fabric.clb_fields, field, "CLB")?;
        let width = fabric.clb_fields[idx].width;
        Ok(self.clb_mut(col, row).set(idx, width, value))
    }

    pub fn get_csb(&self, fabric: &Fabric, col: usize, field: &str) -> Result<u64, String> {
        let idx = field_index(&fabric.csb_fields, field, "CSB")?;
        Ok(self.csb(col).get(idx))
    }

    pub fn set_csb(&mut self, fabric: &Fabric, col: usize, field: &str, value: u64) -> Result<u64, String> {
        let idx = field_index(&fabric.csb_fields, field, "CSB")?;
        let width = fabric.csb_fields[idx].width;
        Ok(self.csb_mut(col).set(idx, width, value))
    }

    // ------------------------------------------------------------------
    // Pinned names

    pub fn pinned_name(&self, block: BlockId) -> Option<&str> {
        self.pinned.get(&block).map(String::as_str)
    }

    pub fn pinned(&self) -> impl Iterator<Item = (BlockId, &str)> {
        self.pinned.iter().map(|(b, n)| (*b, n.as_str()))
    }

    /// The name the block currently displays: pinned if pinned, else default.
    pub fn effective_name(&self, fabric: &Fabric, block: BlockId) -> String {
        match self.pinned_name(block) {
            Some(n) => n.to_string(),
            None => default_name(fabric, block),
        }
    }

    pub fn blocks(&self) -> Vec<BlockId> {
        let mut out = Vec::with_capacity(self.columns * self.rows + self.columns);
        for row in 0..self.rows {
            for col in 0..self.columns {
                out.push(BlockId::Clb { col, row });
            }
        }
        for col in 0..self.columns {
            out.push(BlockId::Csb { col });
        }
        out
    }

    /// Pin a manual name on a block. Rejects collisions with reserved names,
    /// any block's default or pinned name, any derived net name, and names
    /// whose own derived nets would collide.
    pub fn rename(&mut self, fabric: &Fabric, block: BlockId, name: &str) -> Result<(), RenameError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(RenameError {
                message: "a block name cannot be empty".to_string(),
            });
        }
        let used = self.used_names(fabric, block);
        if used.contains(name) {
            return Err(RenameError {
                message: format!("\"{}\" is already in use", name),
            });
        }
        if matches!(block, BlockId::Clb { .. }) {
            for suffix in [
                &fabric.naming.suffix_op,
                &fabric.naming.suffix_reg,
                &fabric.naming.suffix_carry,
            ] {
                let derived = format!("{}{}", name, suffix);
                if used.contains(derived.as_str()) {
                    return Err(RenameError {
                        message: format!(
                            "\"{}\" would make this block's output net \"{}\" collide with an existing name",
                            name, derived
                        ),
                    });
                }
            }
        }
        self.pinned.insert(block, name.to_string());
        Ok(())
    }

    /// Revert a block to its auto-generated name.
    pub fn revert_name(&mut self, block: BlockId) {
        self.pinned.remove(&block);
    }

    /// Every name that a rename of `exclude` must avoid: reserved IO and
    /// constant names, clock fallbacks, the unconnected marker, and all other
    /// blocks' default names, pinned names and derived net names. Default
    /// names stay reserved even while a block is pinned to something else, so
    /// a later revert can never collide.
    fn used_names(&self, fabric: &Fabric, exclude: BlockId) -> HashSet<String> {
        let mut used = HashSet::new();
        for row in &fabric.io_inputs {
            for name in row {
                used.insert(name.clone());
            }
        }
        for row in &fabric.io_outputs {
            for name in row.iter().flatten() {
                used.insert(name.clone());
            }
        }
        used.insert(fabric.naming.constant_zero.clone());
        used.insert(fabric.naming.constant_one.clone());
        used.insert(fabric.naming.unconnected.clone());
        for col in 0..self.columns {
            used.insert(fabric.naming.clock_fallback.replace("{col}", &col.to_string()));
        }
        for block in self.blocks() {
            if block == exclude {
                continue;
            }
            let mut names = vec![default_name(fabric, block)];
            if let Some(pinned) = self.pinned_name(block) {
                names.push(pinned.to_string());
            }
            for n in names {
                if matches!(block, BlockId::Clb { .. }) {
                    for suffix in [
                        &fabric.naming.suffix_op,
                        &fabric.naming.suffix_reg,
                        &fabric.naming.suffix_carry,
                    ] {
                        used.insert(format!("{}{}", n, suffix));
                    }
                }
                used.insert(n);
            }
        }
        used
    }
}

fn field_index(fields: &[Field], name: &str, cell: &str) -> Result<usize, String> {
    fields
        .iter()
        .position(|f| f.name == name)
        .ok_or_else(|| format!("unknown {} field \"{}\"", cell, name))
}
