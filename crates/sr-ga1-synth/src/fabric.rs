//! Loader and validator for `fabric.toml`.
//!
//! The synthesiser reads the same fabric description as the visual
//! programmer, so a fabric edit retargets both tools. Nothing about the grid
//! is hard-coded here: dimensions, config field layout, mux legality, the
//! operation library, IO maps and the scan chain order all come from the
//! file. Every load failure carries a line number where one is recoverable.

use serde::Deserialize;
use std::fmt;
use std::path::{Path, PathBuf};
use toml::Spanned;

// ---------------------------------------------------------------------------
// Errors

#[derive(Debug, Clone)]
pub struct FabricError {
    pub path: PathBuf,
    pub line: Option<usize>,
    pub message: String,
}

impl fmt::Display for FabricError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "{}:{}: {}", self.path.display(), line, self.message),
            None => write!(f, "{}: {}", self.path.display(), self.message),
        }
    }
}

impl std::error::Error for FabricError {}

fn line_at(src: &str, offset: usize) -> usize {
    src[..offset.min(src.len())].bytes().filter(|&b| b == b'\n').count() + 1
}

// ---------------------------------------------------------------------------
// Validated model

/// Anything a mux can select. Bus lanes and `CarryIn` are pass-throughs — the
/// incoming net continues onto the driven segment. `Op`/`Reg`/`Carry`
/// introduce a net local to the CLB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    HorzIn(usize),
    VertIn(usize),
    Const(bool),
    Op,
    Reg,
    Carry,
    CarryIn,
    /// CSB only: the column's vertical ring at the loop tap.
    VRing(usize),
}

impl Source {
    pub fn is_pass_through(self) -> bool {
        matches!(self, Source::HorzIn(_) | Source::VertIn(_) | Source::CarryIn)
    }
    pub fn is_local_output(self) -> bool {
        matches!(self, Source::Op | Source::Reg | Source::Carry)
    }
}

/// A lane driven by an output mux.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BusOut {
    Horz(usize),
    Vert(usize),
}

/// One configuration field, at a fixed position in the cell's shift register.
#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub width: usize,
    /// Chain index of the field's LSB.
    pub offset: usize,
}

impl Field {
    pub fn max(&self) -> u64 {
        if self.width >= 64 {
            u64::MAX
        } else {
            (1u64 << self.width) - 1
        }
    }
}

/// A whole field, or one bit of one, used as a mux select.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldSlice {
    pub field: usize,
    pub bit: Option<usize>,
    /// Number of select bits this slice provides.
    pub width: usize,
}

#[derive(Debug, Clone)]
pub struct InputMux {
    pub name: String,
    pub select: FieldSlice,
    pub sources: Vec<Source>,
}

#[derive(Debug, Clone)]
pub struct OutputMux {
    pub drives: BusOut,
    pub select: FieldSlice,
    pub sources: Vec<Source>,
}

#[derive(Debug, Clone)]
pub struct Operation {
    pub code: u64,
    pub name: String,
    /// 2^inputs entries; entry i is the result for input k taken from bit k of i.
    pub table: Vec<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CarryChain {
    ColumnUp,
}

#[derive(Debug, Clone)]
pub struct CarrySpec {
    pub chain: CarryChain,
    /// Value entering the first cell of each chain.
    pub edge: bool,
}

#[derive(Debug, Clone)]
pub struct FfSpec {
    pub data: Source,
    pub enable: Source,
    pub reset_value_field: usize,
}

#[derive(Debug, Clone)]
pub struct CsbClock {
    pub select: FieldSlice,
    /// Ring lane selected by each select code.
    pub ring_lanes: Vec<usize>,
    pub couple_field: usize,
}

#[derive(Debug, Clone)]
pub struct Ddio {
    pub input: String,
    pub output: String,
    pub dir: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainCells {
    Clb,
    Csb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainOrder {
    RowMajor,
    ColumnAscending,
}

#[derive(Debug, Clone)]
pub struct ChainGroup {
    pub cells: ChainCells,
    pub order: ChainOrder,
}

#[derive(Debug, Clone)]
pub struct Naming {
    pub clb: String,
    pub csb: String,
    pub suffix_op: String,
    pub suffix_reg: String,
    pub suffix_carry: String,
    pub constant_zero: String,
    pub constant_one: String,
    pub clock_fallback: String,
    pub unconnected: String,
}

#[derive(Debug, Clone)]
pub struct Fabric {
    pub path: PathBuf,
    pub name: String,
    pub columns: usize,
    pub rows: usize,
    pub horz_lanes: usize,
    pub vert_lanes: usize,
    pub vertical_ring: bool,

    pub clb_fields: Vec<Field>,
    pub input_muxes: Vec<InputMux>,
    pub op_select: FieldSlice,
    pub operations: Vec<Operation>,
    pub carry_table: Vec<bool>,
    pub carry: CarrySpec,
    pub ff: FfSpec,
    pub output_muxes: Vec<OutputMux>,

    pub csb_fields: Vec<Field>,
    pub csb_clock: CsbClock,

    /// `io_inputs[row][lane]`: reserved net entering at the left edge.
    pub io_inputs: Vec<Vec<String>>,
    /// `io_outputs[row][lane]`: chip output read at the right edge, if any.
    pub io_outputs: Vec<Vec<Option<String>>>,
    pub ddio: Vec<Ddio>,
    pub chain: Vec<ChainGroup>,
    pub naming: Naming,
}

impl Fabric {
    pub fn clb_bits(&self) -> usize {
        self.clb_fields.iter().map(|f| f.width).sum()
    }
    pub fn csb_bits(&self) -> usize {
        self.csb_fields.iter().map(|f| f.width).sum()
    }
    pub fn total_bits(&self) -> usize {
        self.columns * self.rows * self.clb_bits() + self.columns * self.csb_bits()
    }
    pub fn clb_count(&self) -> usize {
        self.columns * self.rows
    }

    pub fn clb_field(&self, name: &str) -> Option<usize> {
        self.clb_fields.iter().position(|f| f.name == name)
    }
    pub fn csb_field(&self, name: &str) -> Option<usize> {
        self.csb_fields.iter().position(|f| f.name == name)
    }
    pub fn input_mux(&self, name: &str) -> Option<&InputMux> {
        self.input_muxes.iter().find(|m| m.name == name)
    }
    pub fn output_mux(&self, drives: BusOut) -> Option<&OutputMux> {
        self.output_muxes.iter().find(|m| m.drives == drives)
    }
    pub fn operation(&self, name: &str) -> Option<&Operation> {
        self.operations.iter().find(|o| o.name == name)
    }

    /// Select code that makes `mux` take `source`, if it can.
    pub fn code_for(mux: &OutputMux, source: Source) -> Option<u64> {
        mux.sources.iter().position(|&s| s == source).map(|i| i as u64)
    }

    /// The lane an incoming vertical signal continues on when a CLB passes it
    /// through — read from the mux table rather than assumed, since this
    /// fabric swaps lane pairs at every hop.
    pub fn vert_pass_target(&self, lane_in: usize) -> Option<usize> {
        self.output_muxes.iter().find_map(|m| match m.drives {
            BusOut::Vert(out) if m.sources.contains(&Source::VertIn(lane_in)) => Some(out),
            _ => None,
        })
    }

    /// Vertical lanes that can carry a given local output.
    pub fn vert_lanes_for(&self, src: Source) -> Vec<usize> {
        self.output_muxes
            .iter()
            .filter_map(|m| match m.drives {
                BusOut::Vert(l) if m.sources.contains(&src) => Some(l),
                _ => None,
            })
            .collect()
    }

    /// Horizontal lanes that can carry a given local output.
    pub fn horz_lanes_for(&self, src: Source) -> Vec<usize> {
        self.output_muxes
            .iter()
            .filter_map(|m| match m.drives {
                BusOut::Horz(l) if m.sources.contains(&src) => Some(l),
                _ => None,
            })
            .collect()
    }

    /// Horizontal lanes whose driving mux can produce either constant — the
    /// cheap constant sources for the cells to the right.
    pub fn const_capable_horz_lanes(&self) -> Vec<usize> {
        self.output_muxes
            .iter()
            .filter_map(|m| match m.drives {
                BusOut::Horz(l)
                    if m.sources.contains(&Source::Const(false))
                        && m.sources.contains(&Source::Const(true)) =>
                {
                    Some(l)
                }
                _ => None,
            })
            .collect()
    }

    /// Whether a registered value can ever get back to an operation input.
    ///
    /// This decides whether the fabric can hold state that depends on itself.
    /// The flip-flop's data input is its own cell's operation result, so a
    /// register whose next value depends on its current value needs `_reg` to
    /// reach that same cell's inputs. The only path back to a cell is its
    /// column's vertical ring, so the question reduces to whether any operation
    /// input can read a vertical lane that carries `_reg`.
    ///
    /// On the 7x4 SR-GA1 the answer is no: lanes 0 and 2 carry `_op`, lanes 1
    /// and 3 carry `_reg`, the pass-through swap keeps those two pairs separate,
    /// and input `c` reads lane 0. Counters, accumulators and LFSRs are
    /// therefore unbuildable, and `sr-ga1-synth` says so instead of searching
    /// for a placement that cannot exist.
    pub fn register_feedback_possible(&self) -> bool {
        let registered = self.vert_lanes_for(Source::Reg);
        self.input_muxes.iter().any(|mux| {
            mux.sources.iter().any(|source| match source {
                Source::VertIn(lane) => registered.contains(lane),
                _ => false,
            })
        })
    }

    /// Whether a constant can be delivered to one of a CLB's operation inputs
    /// without spending a CLB on manufacturing it.
    ///
    /// Column matters, and getting this wrong is a subtle way to produce
    /// unroutable placements. A constant on a major lane comes from the major
    /// mux of the CLB to the *left*, so at column 0 no such CLB exists and the
    /// only constants available are the ones the IO map brings in at the edge.
    /// The minor lanes never carry a constant at all, since their muxes emit
    /// only `op` or a pass-through of their own lane.
    pub fn constant_reaches_input(
        &self,
        mux: usize,
        col: usize,
        row: usize,
        value: bool,
    ) -> bool {
        let Some(mux) = self.input_muxes.get(mux) else { return false };
        let const_lanes = self.const_capable_horz_lanes();
        mux.sources.iter().any(|source| match source {
            // Wired straight into the mux, if a fabric ever does that.
            Source::Const(v) => *v == value,
            Source::HorzIn(lane) => {
                // Supplied at the left edge and passed along the row.
                if self.io_constant(row, *lane) == Some(value) {
                    return true;
                }
                // Or driven by an upstream CLB's major mux.
                const_lanes.contains(lane) && col > 0
            }
            _ => false,
        })
    }

    /// Constant value entering at the left edge of a row, if that lane is
    /// tied to one of the reserved constant nets.
    pub fn io_constant(&self, row: usize, lane: usize) -> Option<bool> {
        let name = self.io_inputs.get(row)?.get(lane)?;
        if *name == self.naming.constant_zero {
            Some(false)
        } else if *name == self.naming.constant_one {
            Some(true)
        } else {
            None
        }
    }

    pub fn load_file(path: &Path) -> Result<Fabric, FabricError> {
        let src = std::fs::read_to_string(path).map_err(|e| FabricError {
            path: path.to_path_buf(),
            line: None,
            message: format!("cannot read fabric description: {}", e),
        })?;
        Fabric::load_str(&src, path)
    }

    pub fn load_str(src: &str, path: &Path) -> Result<Fabric, FabricError> {
        let raw: RawFabric = toml::from_str(src).map_err(|e| FabricError {
            path: path.to_path_buf(),
            line: e.span().map(|s| line_at(src, s.start)),
            message: e.message().to_string(),
        })?;
        Builder { src, path }.build(raw)
    }
}

// ---------------------------------------------------------------------------
// Raw schema, as written in the file

#[derive(Deserialize)]
struct RawFabric {
    schema: u32,
    fabric: RawGrid,
    buses: RawBuses,
    clb: RawClb,
    csb: RawCsb,
    io: RawIo,
    bitstream: RawBitstream,
    naming: RawNaming,
}

#[derive(Deserialize)]
struct RawGrid {
    name: String,
    columns: Spanned<usize>,
    rows: Spanned<usize>,
}

#[derive(Deserialize)]
struct RawBuses {
    horizontal: Spanned<usize>,
    vertical: Spanned<usize>,
    #[serde(default)]
    vertical_ring: bool,
}

#[derive(Deserialize)]
struct RawField {
    name: Spanned<String>,
    width: Spanned<usize>,
}

#[derive(Deserialize)]
struct RawClb {
    fields: Vec<RawField>,
    input_muxes: Vec<RawInputMux>,
    carry: RawCarry,
    operation: RawOperation,
    ff: RawFf,
    output_muxes: Vec<RawOutputMux>,
}

#[derive(Deserialize)]
struct RawInputMux {
    name: String,
    select: Spanned<String>,
    sources: Vec<Spanned<String>>,
}

#[derive(Deserialize)]
struct RawOutputMux {
    drives: Spanned<String>,
    select: Spanned<String>,
    sources: Vec<Spanned<String>>,
}

#[derive(Deserialize)]
struct RawCarry {
    chain: Spanned<String>,
    edge: Spanned<u8>,
}

#[derive(Deserialize)]
struct RawOp {
    code: Spanned<u64>,
    name: String,
    table: Spanned<Vec<u8>>,
}

#[derive(Deserialize)]
struct RawOperation {
    select: Spanned<String>,
    ops: Vec<RawOp>,
    carry_table: Spanned<Vec<u8>>,
}

#[derive(Deserialize)]
struct RawFf {
    data: Spanned<String>,
    enable: Spanned<String>,
    edge: Spanned<String>,
    reset: Spanned<String>,
    reset_value_field: Spanned<String>,
}

#[derive(Deserialize)]
struct RawCsb {
    fields: Vec<RawField>,
    clock: RawCsbClock,
}

#[derive(Deserialize)]
struct RawCsbClock {
    select: Spanned<String>,
    sources: Vec<Spanned<String>>,
    couple_field: Spanned<String>,
}

#[derive(Deserialize)]
struct RawDdio {
    #[serde(rename = "in")]
    input: String,
    out: String,
    dir: String,
}

#[derive(Deserialize)]
struct RawIo {
    inputs: Spanned<Vec<Vec<String>>>,
    outputs: Spanned<Vec<Vec<String>>>,
    #[serde(default)]
    ddio: Vec<Spanned<RawDdio>>,
}

#[derive(Deserialize)]
struct RawChainGroup {
    cells: Spanned<String>,
    order: Spanned<String>,
}

#[derive(Deserialize)]
struct RawBitstream {
    chain: Vec<RawChainGroup>,
}

#[derive(Deserialize)]
struct RawNaming {
    clb: String,
    csb: String,
    suffix_op: String,
    suffix_reg: String,
    suffix_carry: String,
    constant_zero: String,
    constant_one: String,
    clock_fallback: String,
    unconnected: String,
}

// ---------------------------------------------------------------------------
// Validation

struct Builder<'a> {
    src: &'a str,
    path: &'a Path,
}

impl<'a> Builder<'a> {
    fn err<T>(&self, span: std::ops::Range<usize>, message: String) -> Result<T, FabricError> {
        Err(FabricError {
            path: self.path.to_path_buf(),
            line: Some(line_at(self.src, span.start)),
            message,
        })
    }

    fn bare<T>(&self, message: String) -> Result<T, FabricError> {
        Err(FabricError { path: self.path.to_path_buf(), line: None, message })
    }

    fn fields(&self, raw: &[RawField], owner: &str) -> Result<Vec<Field>, FabricError> {
        let mut out: Vec<Field> = Vec::with_capacity(raw.len());
        let mut offset = 0;
        for f in raw {
            let width = *f.width.get_ref();
            if width == 0 || width > 32 {
                return self.err(
                    f.width.span(),
                    format!(
                        "{} field \"{}\" has width {}; expected 1..=32",
                        owner,
                        f.name.get_ref(),
                        width
                    ),
                );
            }
            if out.iter().any(|e| e.name == *f.name.get_ref()) {
                return self.err(
                    f.name.span(),
                    format!("{} field \"{}\" is declared twice", owner, f.name.get_ref()),
                );
            }
            out.push(Field { name: f.name.get_ref().clone(), width, offset });
            offset += width;
        }
        if out.is_empty() {
            return self.bare(format!("{} declares no configuration fields", owner));
        }
        Ok(out)
    }

    /// `field` or `field[bit]`, resolved against a field list.
    fn slice(
        &self,
        spec: &Spanned<String>,
        fields: &[Field],
        owner: &str,
    ) -> Result<FieldSlice, FabricError> {
        let text = spec.get_ref().trim();
        let (name, bit) = match text.split_once('[') {
            Some((n, rest)) => {
                let Some(idx) = rest.strip_suffix(']') else {
                    return self.err(spec.span(), format!("{}: malformed select \"{}\"", owner, text));
                };
                let Ok(b) = idx.trim().parse::<usize>() else {
                    return self
                        .err(spec.span(), format!("{}: malformed bit index in \"{}\"", owner, text));
                };
                (n.trim(), Some(b))
            }
            None => (text, None),
        };
        let Some(field) = fields.iter().position(|f| f.name == name) else {
            return self.err(
                spec.span(),
                format!("{}: no configuration field named \"{}\"", owner, name),
            );
        };
        if let Some(b) = bit {
            if b >= fields[field].width {
                return self.err(
                    spec.span(),
                    format!(
                        "{}: field \"{}\" is {} bits wide; no bit {}",
                        owner, name, fields[field].width, b
                    ),
                );
            }
        }
        let width = if bit.is_some() { 1 } else { fields[field].width };
        Ok(FieldSlice { field, bit, width })
    }

    fn source(
        &self,
        spec: &Spanned<String>,
        horz: usize,
        vert: usize,
        owner: &str,
    ) -> Result<Source, FabricError> {
        let text = spec.get_ref().trim();
        let lane = |t: &str, limit: usize| t.parse::<usize>().ok().filter(|&n| n < limit);
        let parsed = match text.split_once(':') {
            Some(("h_in", n)) => lane(n, horz).map(Source::HorzIn),
            Some(("v_in", n)) => lane(n, vert).map(Source::VertIn),
            Some(("v_ring", n)) => lane(n, vert).map(Source::VRing),
            Some(("const", "0")) => Some(Source::Const(false)),
            Some(("const", "1")) => Some(Source::Const(true)),
            None => match text {
                "op" => Some(Source::Op),
                "reg" => Some(Source::Reg),
                "carry" => Some(Source::Carry),
                "carry_in" => Some(Source::CarryIn),
                _ => None,
            },
            _ => None,
        };
        match parsed {
            Some(s) => Ok(s),
            None => self.err(spec.span(), format!("{}: unknown source token \"{}\"", owner, text)),
        }
    }

    fn bus_out(
        &self,
        spec: &Spanned<String>,
        horz: usize,
        vert: usize,
    ) -> Result<BusOut, FabricError> {
        let text = spec.get_ref().trim();
        let parsed = match text.split_once(':') {
            Some(("h_out", n)) => n.parse::<usize>().ok().filter(|&l| l < horz).map(BusOut::Horz),
            Some(("v_out", n)) => n.parse::<usize>().ok().filter(|&l| l < vert).map(BusOut::Vert),
            _ => None,
        };
        match parsed {
            Some(b) => Ok(b),
            None => self.err(spec.span(), format!("unknown output lane \"{}\"", text)),
        }
    }

    fn build(&self, raw: RawFabric) -> Result<Fabric, FabricError> {
        if raw.schema != 1 {
            return self.bare(format!(
                "fabric schema version {} is not supported (this build reads 1)",
                raw.schema
            ));
        }
        let columns = *raw.fabric.columns.get_ref();
        let rows = *raw.fabric.rows.get_ref();
        if columns == 0 {
            return self.err(raw.fabric.columns.span(), "fabric has zero columns".into());
        }
        if rows == 0 {
            return self.err(raw.fabric.rows.span(), "fabric has zero rows".into());
        }
        let horz = *raw.buses.horizontal.get_ref();
        let vert = *raw.buses.vertical.get_ref();
        if horz == 0 {
            return self.err(raw.buses.horizontal.span(), "fabric has no horizontal lanes".into());
        }
        if vert == 0 {
            return self.err(raw.buses.vertical.span(), "fabric has no vertical lanes".into());
        }

        let clb_fields = self.fields(&raw.clb.fields, "CLB")?;
        let csb_fields = self.fields(&raw.csb.fields, "CSB")?;

        let mut input_muxes = Vec::new();
        for m in &raw.clb.input_muxes {
            let owner = format!("input mux \"{}\"", m.name);
            let select = self.slice(&m.select, &clb_fields, &owner)?;
            let mut sources = Vec::new();
            for s in &m.sources {
                sources.push(self.source(s, horz, vert, &owner)?);
            }
            if sources.len() != 1 << select.width {
                return self.err(
                    m.select.span(),
                    format!(
                        "{}: {} select bits cannot choose between {} sources",
                        owner,
                        select.width,
                        sources.len()
                    ),
                );
            }
            input_muxes.push(InputMux { name: m.name.clone(), select, sources });
        }
        if input_muxes.is_empty() {
            return self.bare("the CLB declares no input muxes".into());
        }

        let op_select = self.slice(&raw.clb.operation.select, &clb_fields, "operation")?;
        let table_len = 1usize << input_muxes.len();
        let mut operations: Vec<Operation> = Vec::new();
        for op in &raw.clb.operation.ops {
            let table = op.table.get_ref();
            if table.len() != table_len {
                return self.err(
                    op.table.span(),
                    format!(
                        "operation \"{}\" has {} truth table entries; {} inputs need {}",
                        op.name,
                        table.len(),
                        input_muxes.len(),
                        table_len
                    ),
                );
            }
            let code = *op.code.get_ref();
            if code >= 1 << op_select.width {
                return self.err(
                    op.code.span(),
                    format!(
                        "operation \"{}\" has code {}, out of range for a {}-bit select",
                        op.name, code, op_select.width
                    ),
                );
            }
            if operations.iter().any(|o| o.code == code) {
                return self.err(op.code.span(), format!("operation code {} is used twice", code));
            }
            operations.push(Operation {
                code,
                name: op.name.clone(),
                table: table.iter().map(|&v| v != 0).collect(),
            });
        }
        operations.sort_by_key(|o| o.code);
        let carry_raw = raw.clb.operation.carry_table.get_ref();
        if carry_raw.len() != table_len {
            return self.err(
                raw.clb.operation.carry_table.span(),
                format!("carry table has {} entries; expected {}", carry_raw.len(), table_len),
            );
        }
        let carry_table: Vec<bool> = carry_raw.iter().map(|&v| v != 0).collect();

        let chain = match raw.clb.carry.chain.get_ref().as_str() {
            "column_up" => CarryChain::ColumnUp,
            other => {
                return self.err(
                    raw.clb.carry.chain.span(),
                    format!("unknown carry chain topology \"{}\"", other),
                )
            }
        };
        let carry = CarrySpec { chain, edge: *raw.clb.carry.edge.get_ref() != 0 };

        let ff_data = self.source(&raw.clb.ff.data, horz, vert, "flip-flop data")?;
        if ff_data != Source::Op {
            return self.err(
                raw.clb.ff.data.span(),
                "the flip-flop data input must be the operation result; this synthesiser \
                 cannot target a fabric with an independent register data path"
                    .into(),
            );
        }
        let ff_enable = self.source(&raw.clb.ff.enable, horz, vert, "flip-flop enable")?;
        if raw.clb.ff.edge.get_ref() != "rising" {
            return self.err(
                raw.clb.ff.edge.span(),
                format!(
                    "unsupported clock edge \"{}\"; expected \"rising\"",
                    raw.clb.ff.edge.get_ref()
                ),
            );
        }
        if raw.clb.ff.reset.get_ref() != "sync" {
            return self.err(
                raw.clb.ff.reset.span(),
                format!(
                    "unsupported reset style \"{}\"; expected \"sync\"",
                    raw.clb.ff.reset.get_ref()
                ),
            );
        }
        let reset_slice =
            self.slice(&raw.clb.ff.reset_value_field, &clb_fields, "flip-flop reset value")?;
        let ff = FfSpec { data: ff_data, enable: ff_enable, reset_value_field: reset_slice.field };

        let mut output_muxes: Vec<OutputMux> = Vec::new();
        for m in &raw.clb.output_muxes {
            let drives = self.bus_out(&m.drives, horz, vert)?;
            let owner = format!("output mux for \"{}\"", m.drives.get_ref());
            let select = self.slice(&m.select, &clb_fields, &owner)?;
            let mut sources = Vec::new();
            for s in &m.sources {
                sources.push(self.source(s, horz, vert, &owner)?);
            }
            if sources.len() != 1 << select.width {
                return self.err(
                    m.select.span(),
                    format!(
                        "{}: {} select bits cannot choose between {} sources",
                        owner,
                        select.width,
                        sources.len()
                    ),
                );
            }
            if output_muxes.iter().any(|e| e.drives == drives) {
                return self.err(
                    m.drives.span(),
                    format!("lane \"{}\" is driven by two muxes", m.drives.get_ref()),
                );
            }
            output_muxes.push(OutputMux { drives, select, sources });
        }
        for lane in 0..horz {
            if !output_muxes.iter().any(|m| m.drives == BusOut::Horz(lane)) {
                return self.bare(format!("horizontal lane {} has no output mux", lane));
            }
        }
        for lane in 0..vert {
            if !output_muxes.iter().any(|m| m.drives == BusOut::Vert(lane)) {
                return self.bare(format!("vertical lane {} has no output mux", lane));
            }
        }

        let csb_select = self.slice(&raw.csb.clock.select, &csb_fields, "CSB clock select")?;
        let mut ring_lanes = Vec::new();
        for s in &raw.csb.clock.sources {
            match self.source(s, horz, vert, "CSB clock select")? {
                Source::VRing(l) => ring_lanes.push(l),
                _ => {
                    return self.err(
                        s.span(),
                        format!("CSB clock sources must be v_ring:N; found \"{}\"", s.get_ref()),
                    )
                }
            }
        }
        if ring_lanes.len() != 1 << csb_select.width {
            return self.err(
                raw.csb.clock.select.span(),
                format!(
                    "CSB clock: {} select bits cannot choose between {} ring lanes",
                    csb_select.width,
                    ring_lanes.len()
                ),
            );
        }
        let couple = self.slice(&raw.csb.clock.couple_field, &csb_fields, "CSB couple")?;
        let csb_clock = CsbClock { select: csb_select, ring_lanes, couple_field: couple.field };

        let io_in_raw = raw.io.inputs.get_ref();
        if io_in_raw.len() != rows {
            return self.err(
                raw.io.inputs.span(),
                format!("io.inputs has {} rows; the fabric has {}", io_in_raw.len(), rows),
            );
        }
        for (r, lanes) in io_in_raw.iter().enumerate() {
            if lanes.len() != horz {
                return self.err(
                    raw.io.inputs.span(),
                    format!(
                        "io.inputs row {} lists {} lanes; the fabric has {}",
                        r,
                        lanes.len(),
                        horz
                    ),
                );
            }
        }
        let io_out_raw = raw.io.outputs.get_ref();
        if io_out_raw.len() != rows {
            return self.err(
                raw.io.outputs.span(),
                format!("io.outputs has {} rows; the fabric has {}", io_out_raw.len(), rows),
            );
        }
        for (r, lanes) in io_out_raw.iter().enumerate() {
            if lanes.len() != horz {
                return self.err(
                    raw.io.outputs.span(),
                    format!(
                        "io.outputs row {} lists {} lanes; the fabric has {}",
                        r,
                        lanes.len(),
                        horz
                    ),
                );
            }
        }
        let io_inputs = io_in_raw.clone();
        let io_outputs: Vec<Vec<Option<String>>> = io_out_raw
            .iter()
            .map(|lanes| {
                lanes.iter().map(|n| if n.is_empty() { None } else { Some(n.clone()) }).collect()
            })
            .collect();

        let mut ddio = Vec::new();
        for d in &raw.io.ddio {
            let r = d.get_ref();
            let known = |net: &str| {
                io_inputs.iter().any(|lanes| lanes.iter().any(|n| n == net))
                    || io_outputs
                        .iter()
                        .any(|lanes| lanes.iter().any(|n| n.as_deref() == Some(net)))
            };
            for net in [&r.input, &r.out, &r.dir] {
                if !known(net) {
                    return self
                        .err(d.span(), format!("DDIO net \"{}\" appears in no IO map row", net));
                }
            }
            ddio.push(Ddio { input: r.input.clone(), output: r.out.clone(), dir: r.dir.clone() });
        }

        let mut chain_groups = Vec::new();
        let mut seen_clb = false;
        let mut seen_csb = false;
        for g in &raw.bitstream.chain {
            let cells = match g.cells.get_ref().as_str() {
                "clb" => {
                    seen_clb = true;
                    ChainCells::Clb
                }
                "csb" => {
                    seen_csb = true;
                    ChainCells::Csb
                }
                other => {
                    return self
                        .err(g.cells.span(), format!("unknown chain cell kind \"{}\"", other))
                }
            };
            let order = match g.order.get_ref().as_str() {
                "row_major" => ChainOrder::RowMajor,
                "column_ascending" => ChainOrder::ColumnAscending,
                other => {
                    return self.err(g.order.span(), format!("unknown chain order \"{}\"", other))
                }
            };
            chain_groups.push(ChainGroup { cells, order });
        }
        if !seen_clb || !seen_csb {
            return self.bare("the scan chain must cover both CLBs and CSBs".into());
        }

        Ok(Fabric {
            path: self.path.to_path_buf(),
            name: raw.fabric.name.clone(),
            columns,
            rows,
            horz_lanes: horz,
            vert_lanes: vert,
            vertical_ring: raw.buses.vertical_ring,
            clb_fields,
            input_muxes,
            op_select,
            operations,
            carry_table,
            carry,
            ff,
            output_muxes,
            csb_fields,
            csb_clock,
            io_inputs,
            io_outputs,
            ddio,
            chain: chain_groups,
            naming: Naming {
                clb: raw.naming.clb,
                csb: raw.naming.csb,
                suffix_op: raw.naming.suffix_op,
                suffix_reg: raw.naming.suffix_reg,
                suffix_carry: raw.naming.suffix_carry,
                constant_zero: raw.naming.constant_zero,
                constant_one: raw.naming.constant_one,
                clock_fallback: raw.naming.clock_fallback,
                unconnected: raw.naming.unconnected,
            },
        })
    }
}
