//! Design-side configuration state: per-cell config field values and the
//! user's pinned block names. GUI-free; the naming engine and bitstream codec
//! read from here.

use crate::fabric::{Fabric, Field, FieldSlice};
use std::collections::{BTreeMap, HashMap, HashSet};
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

/// One chip pad, by side and position. Structural like every other index in
/// this crate, so a pad survives being renamed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IoPad {
    /// True for the left-edge input controller, false for the right-edge output.
    pub input_side: bool,
    pub row: usize,
    pub lane: usize,
}

impl IoPad {
    pub fn input(row: usize, lane: usize) -> IoPad {
        IoPad { input_side: true, row, lane }
    }
    pub fn output(row: usize, lane: usize) -> IoPad {
        IoPad { input_side: false, row, lane }
    }

    /// The reserved name the fabric gives this pad, if it has one.
    pub fn reserved_name(self, fabric: &Fabric) -> Option<String> {
        if self.input_side {
            fabric.io_inputs.get(self.row)?.get(self.lane).cloned()
        } else {
            fabric.io_outputs.get(self.row)?.get(self.lane)?.clone()
        }
    }

    /// Whether the fabric ties this pad to a constant, in which case it carries
    /// no design signal and cannot be renamed or looped.
    pub fn is_constant(self, fabric: &Fabric) -> bool {
        match self.reserved_name(fabric) {
            Some(name) => {
                name == fabric.naming.constant_zero || name == fabric.naming.constant_one
            }
            None => false,
        }
    }
}

/// A board wire from a chip output pad back to a chip input pad.
///
/// This is wiring outside the chip, so it costs no configuration bits and
/// appears in no bitstream. It matters here because it is the only way a signal
/// can travel leftward or return to the cell that produced it — see
/// `docs/fabric-notes.md` — and because it is combinational, so a cycle closed
/// through one oscillates exactly as a cycle inside the fabric would.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Loopback {
    /// Output pad the wire starts at.
    pub from: IoPad,
    /// Input pad it returns to. One input has one driver, so this is unique.
    pub to: IoPad,
}

/// A full fabric configuration: one [`CellConfig`] per CLB and CSB, the pinned
/// block names, the design's own names for the chip pads, and the board's
/// loop-around wiring. Grid indexing matches the fabric: row 0 at the bottom,
/// column 0 at the left.
#[derive(Debug, Clone)]
pub struct Design {
    pub columns: usize,
    pub rows: usize,
    clbs: Vec<CellConfig>,
    csbs: Vec<CellConfig>,
    pinned: HashMap<BlockId, String>,
    /// Display names for chip pads. A pad keeps its reserved identity; this is
    /// what the design calls the signal on it.
    io_names: BTreeMap<IoPad, String>,
    /// Board wiring, at most one entry per input pad.
    loopback: Vec<Loopback>,
}

impl Design {
    pub fn new(fabric: &Fabric) -> Self {
        Design {
            columns: fabric.columns,
            rows: fabric.rows,
            clbs: vec![CellConfig::new(&fabric.clb_fields); fabric.columns * fabric.rows],
            csbs: vec![CellConfig::new(&fabric.csb_fields); fabric.columns],
            pinned: HashMap::new(),
            io_names: BTreeMap::new(),
            loopback: Vec::new(),
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
            // The nets this block would imply must not collide with anything
            // that is already a net, pad names included.
            // Excluding this block: a block re-pinned to its own default name
            // would otherwise collide with the nets it already implies.
            let nets = self.net_names(fabric, None, Some(block));
            for suffix in [
                &fabric.naming.suffix_op,
                &fabric.naming.suffix_reg,
                &fabric.naming.suffix_carry,
            ] {
                let derived = format!("{}{}", name, suffix);
                if used.contains(derived.as_str()) || nets.contains(derived.as_str()) {
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

    // ------------------------------------------------------------------
    // Chip pad names
    //
    // A pad name is a display alias: the pad keeps the reserved identity the
    // fabric gave it, so stimulus keys, trace ids and the bitstream are all
    // untouched and renaming can never invalidate an existing design. The one
    // place it is more than a label is an *input* pad, which is where a net
    // originates — see `naming::Namer::net_name`.

    pub fn io_name(&self, pad: IoPad) -> Option<&str> {
        self.io_names.get(&pad).map(String::as_str)
    }

    pub fn io_names(&self) -> impl Iterator<Item = (IoPad, &str)> {
        self.io_names.iter().map(|(p, n)| (*p, n.as_str()))
    }

    /// The name this pad displays: the design's if it has one, else reserved.
    pub fn effective_io_name(&self, fabric: &Fabric, pad: IoPad) -> Option<String> {
        match self.io_name(pad) {
            Some(name) => Some(name.to_string()),
            None => pad.reserved_name(fabric),
        }
    }

    /// Give a pad the design's own name for the signal on it.
    pub fn rename_io(
        &mut self,
        fabric: &Fabric,
        pad: IoPad,
        name: &str,
    ) -> Result<(), RenameError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(RenameError {
                message: "a pad name cannot be empty".to_string(),
            });
        }
        if pad.reserved_name(fabric).is_none() {
            return Err(RenameError {
                message: "this lane has no pad on it".to_string(),
            });
        }
        if pad.is_constant(fabric) {
            return Err(RenameError {
                message: "this pad is tied to a constant by the fabric and carries no design \
                          signal"
                    .to_string(),
            });
        }
        // Renaming a pad to what it is already called is not a collision.
        let reserved = pad.reserved_name(fabric);
        if reserved.as_deref() != Some(name) && self.net_names(fabric, Some(pad), None).contains(name) {
            return Err(RenameError {
                message: format!("\"{}\" is already in use", name),
            });
        }
        self.io_names.insert(pad, name.to_string());
        Ok(())
    }

    /// Drop a pad's design name, leaving the fabric's reserved one.
    pub fn revert_io_name(&mut self, pad: IoPad) {
        self.io_names.remove(&pad);
    }

    // ------------------------------------------------------------------
    // Loop-around board wiring

    pub fn loopback(&self) -> &[Loopback] {
        &self.loopback
    }

    /// The output pad driving this input pad, if the board wires one to it.
    pub fn loopback_driver(&self, to: IoPad) -> Option<IoPad> {
        self.loopback.iter().find(|l| l.to == to).map(|l| l.from)
    }

    /// The input pads this output pad feeds. One pin can fan out to several.
    pub fn loopback_targets(&self, from: IoPad) -> Vec<IoPad> {
        self.loopback.iter().filter(|l| l.from == from).map(|l| l.to).collect()
    }

    /// Wire an output pad back to an input pad. Replaces any existing wire into
    /// that input, since one input has exactly one driver.
    pub fn set_loopback(
        &mut self,
        fabric: &Fabric,
        from: IoPad,
        to: IoPad,
    ) -> Result<(), RenameError> {
        if from.input_side || !to.input_side {
            return Err(RenameError {
                message: "a loop runs from a chip output to a chip input".to_string(),
            });
        }
        if from.reserved_name(fabric).is_none() {
            return Err(RenameError {
                message: "that output lane has no pad on it".to_string(),
            });
        }
        if to.reserved_name(fabric).is_none() {
            return Err(RenameError {
                message: "that input lane has no pad on it".to_string(),
            });
        }
        if to.is_constant(fabric) {
            return Err(RenameError {
                message: "that input is tied to a constant by the fabric, so nothing can drive it"
                    .to_string(),
            });
        }
        self.loopback.retain(|l| l.to != to);
        self.loopback.push(Loopback { from, to });
        self.loopback.sort_by_key(|l| (l.to, l.from));
        Ok(())
    }

    /// Remove the wire into an input pad, if there is one.
    pub fn clear_loopback(&mut self, to: IoPad) {
        self.loopback.retain(|l| l.to != to);
    }

    /// Every name that a rename of `exclude` must avoid: reserved IO and
    /// constant names, clock fallbacks, the unconnected marker, the design's
    /// own pad names, and all other blocks' default names, pinned names and
    /// derived net names. Default names stay reserved even while a block is
    /// pinned to something else, so a later revert can never collide.
    fn used_names(&self, fabric: &Fabric, exclude: BlockId) -> HashSet<String> {
        // Pad names are excluded here for the reason given on `net_names`: a
        // block and a pad carrying the same signal should be allowed the same
        // name. A block's *derived* nets are checked against pad names
        // separately, in `rename`.
        let mut used = self.reserved_names(fabric);
        for block in self.blocks() {
            if block == exclude {
                continue;
            }
            self.add_block_names(fabric, block, &mut used);
        }
        used
    }

    /// Every name that is a *net* name, which is the set a pad name must avoid.
    ///
    /// A block's own name is deliberately not in here. A block is not a net: its
    /// nets are the suffixed forms, so a cell called `count[0]` and the pad
    /// `count[0]` leaves on are the same signal seen at two places and naming
    /// both of them that is right, not a clash. What a pad must avoid is another
    /// pad, a reserved name, and any derived net name.
    fn net_names(
        &self,
        fabric: &Fabric,
        exclude_pad: Option<IoPad>,
        exclude_block: Option<BlockId>,
    ) -> HashSet<String> {
        let mut used = self.reserved_and_pad_names(fabric, exclude_pad);
        for block in self.blocks() {
            if Some(block) == exclude_block {
                continue;
            }
            for name in self.block_name_forms(fabric, block) {
                for suffix in [
                    &fabric.naming.suffix_op,
                    &fabric.naming.suffix_reg,
                    &fabric.naming.suffix_carry,
                ] {
                    used.insert(format!("{}{}", name, suffix));
                }
            }
        }
        used
    }

    /// A block's default name and its pinned name, if it has one. The default
    /// stays reserved through a rename so a later revert can never collide.
    fn block_name_forms(&self, fabric: &Fabric, block: BlockId) -> Vec<String> {
        let mut names = vec![default_name(fabric, block)];
        if let Some(pinned) = self.pinned_name(block) {
            names.push(pinned.to_string());
        }
        names
    }

    fn add_block_names(&self, fabric: &Fabric, block: BlockId, used: &mut HashSet<String>) {
        for n in self.block_name_forms(fabric, block) {
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

    /// Names the fabric reserves, plus the design's pad names. The reserved
    /// name of a renamed pad stays reserved: a block called `input_0` would be
    /// baffling even once that pad is displaying something else.
    fn reserved_and_pad_names(
        &self,
        fabric: &Fabric,
        exclude_pad: Option<IoPad>,
    ) -> HashSet<String> {
        let mut used = self.reserved_names(fabric);
        for (pad, name) in self.io_names() {
            if Some(pad) == exclude_pad {
                continue;
            }
            used.insert(name.to_string());
        }
        used
    }

    /// Just the names the fabric itself reserves.
    fn reserved_names(&self, fabric: &Fabric) -> HashSet<String> {
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
        used
    }
}

fn field_index(fields: &[Field], name: &str, cell: &str) -> Result<usize, String> {
    fields
        .iter()
        .position(|f| f.name == name)
        .ok_or_else(|| format!("unknown {} field \"{}\"", cell, name))
}
