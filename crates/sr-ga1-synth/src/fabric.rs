//! Loader and validator for `fabric.toml`.
//!
//! The synthesiser reads the same fabric description as the visual
//! programmer, so a fabric edit retargets both tools. Nothing about the grid
//! is hard-coded here: dimensions, config field layout, mux legality, the
//! LUT, the IO controller and the scan chain order all come from the file.
//! Every load failure carries a line number where one is recoverable.

use serde::Deserialize;
use std::collections::BTreeMap;
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

/// The CLB's lookup table.
#[derive(Debug, Clone)]
pub struct LutSpec {
    /// The truth-table field, as a whole-field select.
    pub select: FieldSlice,
    /// Input mux indices, most significant index bit first: the LUT output
    /// for input values `v` is bit `sum(v[order[k]] << (n-1-k))` of the field.
    pub order: Vec<usize>,
    /// Display names for well-known values, in file order.
    pub presets: Vec<(String, u64)>,
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

/// A dual-direction pad, as indices into the pad lists: `input` into
/// `io_inputs`, `output` and `dir` into `io_outputs`. While `dir` is 1 the
/// fabric side of `input` reads `output` back.
#[derive(Debug, Clone, Copy)]
pub struct Ddio {
    pub input: usize,
    pub output: usize,
    pub dir: usize,
}

/// Something the IO controller can put onto a left-edge lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IoSource {
    /// An input-side pad (index into `io_inputs`).
    InPad(usize),
    /// The value driven onto an output-side pad (index into `io_outputs`),
    /// read back on-chip.
    OutPad(usize),
}

/// What enters one left-edge lane.
#[derive(Debug, Clone, Copy)]
pub enum LeftLane {
    /// An input mux over `io_mux_sources`.
    Mux(FieldSlice),
    Fixed(IoSource),
}

/// What drives one output-side pad from the right edge.
#[derive(Debug, Clone, Copy)]
pub enum PadDrive {
    /// An output mux over every right-edge lane; code `row * horz_lanes + lane`.
    Mux(FieldSlice),
    Fixed { row: usize, lane: usize },
}

/// One way an IO-controller connection can be made: the lane, and the IO
/// configuration write it needs (none for a fixed connection).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoLink {
    pub row: usize,
    pub lane: usize,
    pub write: Option<(FieldSlice, u64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainCells {
    Clb,
    Io,
    Csb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainOrder {
    RowMajor,
    ColumnAscending,
    Single,
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
    pub io: String,
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
    pub lut: LutSpec,
    /// Indexed like the LUT.
    pub carry_table: Vec<bool>,
    pub carry: CarrySpec,
    pub ff: FfSpec,
    pub output_muxes: Vec<OutputMux>,

    pub csb_fields: Vec<Field>,
    pub csb_clock: CsbClock,

    pub io_fields: Vec<Field>,
    /// Input-side pads: the chip inputs, then the fabric side of each DDIO.
    pub io_inputs: Vec<String>,
    /// Output-side pads: the chip outputs, then each DDIO's out and dir.
    pub io_outputs: Vec<String>,
    pub ddio: Vec<Ddio>,
    /// Sources of every left-edge input mux, in select-code order.
    pub io_mux_sources: Vec<IoSource>,
    /// `io_left[row][lane]`.
    pub io_left: Vec<Vec<LeftLane>>,
    /// Per output-side pad.
    pub io_drive: Vec<PadDrive>,
    pub chain: Vec<ChainGroup>,
    /// The bit sent ahead of the configuration, if the chip expects one.
    pub start_bit: Option<bool>,
    pub naming: Naming,
}

impl Fabric {
    pub fn clb_bits(&self) -> usize {
        self.clb_fields.iter().map(|f| f.width).sum()
    }
    pub fn csb_bits(&self) -> usize {
        self.csb_fields.iter().map(|f| f.width).sum()
    }
    pub fn io_bits(&self) -> usize {
        self.io_fields.iter().map(|f| f.width).sum()
    }
    /// Configuration bits in the chain, excluding any start bit.
    pub fn total_bits(&self) -> usize {
        self.columns * self.rows * self.clb_bits() + self.io_bits() + self.columns * self.csb_bits()
    }
    /// A complete transmission: the start bit, if any, plus the configuration.
    pub fn stream_bits(&self) -> usize {
        self.total_bits() + usize::from(self.start_bit.is_some())
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
    pub fn io_field(&self, name: &str) -> Option<usize> {
        self.io_fields.iter().position(|f| f.name == name)
    }
    pub fn input_pad(&self, name: &str) -> Option<usize> {
        self.io_inputs.iter().position(|n| n == name)
    }
    pub fn output_pad(&self, name: &str) -> Option<usize> {
        self.io_outputs.iter().position(|n| n == name)
    }
    /// Whether a name is one of the fabric's pads, on either side.
    pub fn is_pad(&self, name: &str) -> bool {
        self.input_pad(name).is_some() || self.output_pad(name).is_some()
    }
    /// The DDIO a pad belongs to, on whichever side.
    pub fn ddio_pad_names(&self) -> Vec<&str> {
        self.ddio
            .iter()
            .flat_map(|d| {
                [
                    self.io_inputs[d.input].as_str(),
                    self.io_outputs[d.output].as_str(),
                    self.io_outputs[d.dir].as_str(),
                ]
            })
            .collect()
    }
    pub fn ddio_of_input(&self, pad: usize) -> Option<&Ddio> {
        self.ddio.iter().find(|d| d.input == pad)
    }

    /// Every left-edge lane an IO source can be put on, and the write that
    /// does it.
    pub fn lanes_for_source(&self, source: IoSource) -> Vec<IoLink> {
        let mut out = Vec::new();
        for (row, lanes) in self.io_left.iter().enumerate() {
            for (lane, left) in lanes.iter().enumerate() {
                match *left {
                    LeftLane::Fixed(s) if s == source => out.push(IoLink { row, lane, write: None }),
                    LeftLane::Mux(select) => {
                        if let Some(code) = self.io_mux_sources.iter().position(|&s| s == source) {
                            out.push(IoLink { row, lane, write: Some((select, code as u64)) });
                        }
                    }
                    _ => {}
                }
            }
        }
        out
    }

    /// Every right-edge lane that can drive an output-side pad, and the write
    /// that selects it.
    pub fn lanes_for_output(&self, pad: usize) -> Vec<IoLink> {
        match self.io_drive.get(pad) {
            Some(PadDrive::Fixed { row, lane }) => vec![IoLink { row: *row, lane: *lane, write: None }],
            Some(PadDrive::Mux(select)) => (0..self.rows)
                .flat_map(|row| (0..self.horz_lanes).map(move |lane| (row, lane)))
                .map(|(row, lane)| IoLink {
                    row,
                    lane,
                    write: Some((*select, (row * self.horz_lanes + lane) as u64)),
                })
                .collect(),
            None => Vec::new(),
        }
    }

    /// Output-side pads the IO controller can read back onto the left edge.
    pub fn loopable_outputs(&self) -> Vec<usize> {
        let mut out: Vec<usize> = self
            .io_mux_sources
            .iter()
            .filter_map(|s| match s {
                IoSource::OutPad(o) => Some(*o),
                IoSource::InPad(_) => None,
            })
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The truth-table index for these input mux outputs (in `input_muxes`
    /// order).
    pub fn lut_index(&self, inputs: &[bool]) -> usize {
        self.lut
            .order
            .iter()
            .fold(0, |acc, &m| (acc << 1) | usize::from(inputs.get(m).copied().unwrap_or(false)))
    }

    /// The LUT value computing `f`, a function of the input mux outputs in
    /// `input_muxes` order.
    pub fn lut_value(&self, f: impl Fn(&[bool]) -> bool) -> u64 {
        let n = self.input_muxes.len();
        let mut lut = 0u64;
        for minterm in 0..1usize << n {
            let inputs: Vec<bool> = (0..n).map(|k| minterm >> k & 1 != 0).collect();
            if f(&inputs) {
                lut |= 1 << self.lut_index(&inputs);
            }
        }
        lut
    }

    /// Evaluate a LUT value on the input mux outputs.
    pub fn eval_lut(&self, lut: u64, inputs: &[bool]) -> bool {
        lut >> self.lut_index(inputs) & 1 != 0
    }

    pub fn preset_name(&self, lut: u64) -> Option<&str> {
        self.lut.presets.iter().find(|(_, v)| *v == lut).map(|(n, _)| n.as_str())
    }
    pub fn input_mux(&self, name: &str) -> Option<&InputMux> {
        self.input_muxes.iter().find(|m| m.name == name)
    }
    pub fn output_mux(&self, drives: BusOut) -> Option<&OutputMux> {
        self.output_muxes.iter().find(|m| m.drives == drives)
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

    /// Whether a registered value can ever get back to a LUT input.
    ///
    /// This decides whether the fabric can hold state that depends on itself.
    /// The flip-flop's data input is its own cell's LUT result, so a register
    /// whose next value depends on its current value needs `_reg` to reach that
    /// same cell's inputs. Inside the fabric the only path back to a cell is
    /// its column's vertical ring, and on SR-GA1 that never works: lanes 0 and
    /// 2 carry `_op`, lanes 1 and 3 carry `_reg`, the pass-through swap keeps
    /// the pairs apart, and input `c` reads lane 0. The IO controller is what
    /// closes the gap, by reading chip outputs back onto the left edge.
    pub fn register_feedback_possible(&self) -> bool {
        let registered = self.vert_lanes_for(Source::Reg);
        let through_ring = self.input_muxes.iter().any(|mux| {
            mux.sources.iter().any(|source| match source {
                Source::VertIn(lane) => registered.contains(lane),
                _ => false,
            })
        });
        through_ring || !self.loopable_outputs().is_empty()
    }

    /// Whether a constant can be delivered to one of a CLB's inputs without
    /// spending a CLB on manufacturing it.
    ///
    /// Column matters. A constant on a major lane comes from the major mux of
    /// the CLB to the *left*, so at column 0 no such CLB exists and the IO
    /// controller brings no constants in. The minor lanes never carry a
    /// constant at all, since their muxes emit only `op` or a pass-through of
    /// their own lane. On a LUT fabric the only consumer is the register
    /// enable, which wants a constant 1.
    pub fn constant_reaches_input(&self, mux: usize, col: usize, value: bool) -> bool {
        let Some(mux) = self.input_muxes.get(mux) else { return false };
        let const_lanes = self.const_capable_horz_lanes();
        mux.sources.iter().any(|source| match source {
            Source::Const(v) => *v == value,
            Source::HorzIn(lane) => const_lanes.contains(lane) && col > 0,
            _ => false,
        })
    }

    /// Whether a lane entering `col` can carry a constant, which needs an
    /// upstream CLB's major mux.
    pub fn constant_reaches_lane(&self, lane: usize, col: usize) -> bool {
        col > 0 && self.const_capable_horz_lanes().contains(&lane)
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
    lut: RawLut,
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
struct RawPreset {
    name: String,
    value: Spanned<u64>,
}

#[derive(Deserialize)]
struct RawLut {
    field: Spanned<String>,
    index: Vec<Spanned<String>>,
    carry_table: Spanned<Vec<u8>>,
    #[serde(default)]
    presets: Vec<RawPreset>,
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
    fields: Vec<RawField>,
    inputs: Vec<Spanned<String>>,
    outputs: Vec<Spanned<String>>,
    #[serde(default)]
    ddio: Vec<Spanned<RawDdio>>,
    input_mux_sources: Vec<Spanned<String>>,
    left: Spanned<Vec<Vec<Spanned<String>>>>,
    drive: Spanned<BTreeMap<String, Spanned<String>>>,
}

#[derive(Deserialize)]
struct RawChainGroup {
    cells: Spanned<String>,
    order: Spanned<String>,
}

#[derive(Deserialize)]
struct RawBitstream {
    chain: Vec<RawChainGroup>,
    start_bit: Option<Spanned<u8>>,
}

#[derive(Deserialize)]
struct RawNaming {
    clb: String,
    csb: String,
    io: String,
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
        if raw.schema != 2 {
            return self.bare(format!(
                "fabric schema version {} is not supported (this build reads 2)",
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

        let table_len = 1usize << input_muxes.len();
        let lut_select = self.slice(&raw.clb.lut.field, &clb_fields, "LUT")?;
        if lut_select.bit.is_some() || lut_select.width != table_len {
            return self.err(
                raw.clb.lut.field.span(),
                format!(
                    "the LUT field must be a whole {}-bit field for {} inputs",
                    table_len,
                    input_muxes.len()
                ),
            );
        }
        let mut order = Vec::new();
        for name in &raw.clb.lut.index {
            let Some(m) = input_muxes.iter().position(|m| m.name == *name.get_ref()) else {
                return self.err(name.span(), format!("LUT index \"{}\" is not an input mux", name.get_ref()));
            };
            if order.contains(&m) {
                return self.err(name.span(), format!("LUT index \"{}\" appears twice", name.get_ref()));
            }
            order.push(m);
        }
        if order.len() != input_muxes.len() {
            return self.bare(format!("the LUT index must list all {} input muxes", input_muxes.len()));
        }
        let mut presets = Vec::new();
        for p in &raw.clb.lut.presets {
            let value = *p.value.get_ref();
            if table_len < 64 && value >> table_len != 0 {
                return self.err(p.value.span(), format!("LUT preset \"{}\" does not fit the table", p.name));
            }
            presets.push((p.name.clone(), value));
        }
        let lut = LutSpec { select: lut_select, order, presets };
        let carry_raw = raw.clb.lut.carry_table.get_ref();
        if carry_raw.len() != table_len {
            return self.err(
                raw.clb.lut.carry_table.span(),
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

        // ---- the IO controller --------------------------------------------
        let io_fields = self.fields(&raw.io.fields, "IO")?;
        let mut seen: Vec<String> = Vec::new();
        let mut io_inputs: Vec<String> = Vec::new();
        for name in &raw.io.inputs {
            if seen.contains(name.get_ref()) {
                return self.err(name.span(), format!("pad \"{}\" is declared twice", name.get_ref()));
            }
            seen.push(name.get_ref().clone());
            io_inputs.push(name.get_ref().clone());
        }
        let mut io_outputs: Vec<String> = Vec::new();
        for name in &raw.io.outputs {
            if seen.contains(name.get_ref()) {
                return self.err(name.span(), format!("pad \"{}\" is declared twice", name.get_ref()));
            }
            seen.push(name.get_ref().clone());
            io_outputs.push(name.get_ref().clone());
        }
        for d in &raw.io.ddio {
            for net in [&d.get_ref().input, &d.get_ref().out, &d.get_ref().dir] {
                if seen.contains(net) {
                    return self.err(d.span(), format!("pad \"{}\" is declared twice", net));
                }
                seen.push(net.clone());
            }
        }
        // DDIO pads join the lists: fabric-side inputs after the chip inputs,
        // then outs and dirs after the chip outputs.
        io_inputs.extend(raw.io.ddio.iter().map(|d| d.get_ref().input.clone()));
        io_outputs.extend(raw.io.ddio.iter().map(|d| d.get_ref().out.clone()));
        io_outputs.extend(raw.io.ddio.iter().map(|d| d.get_ref().dir.clone()));
        let ddio: Vec<Ddio> = raw
            .io
            .ddio
            .iter()
            .map(|d| {
                let r = d.get_ref();
                Ddio {
                    input: io_inputs.iter().position(|n| *n == r.input).unwrap_or(0),
                    output: io_outputs.iter().position(|n| *n == r.out).unwrap_or(0),
                    dir: io_outputs.iter().position(|n| *n == r.dir).unwrap_or(0),
                }
            })
            .collect();
        let io_source = |name: &Spanned<String>| -> Result<IoSource, FabricError> {
            if let Some(i) = io_inputs.iter().position(|n| n == name.get_ref()) {
                return Ok(IoSource::InPad(i));
            }
            if let Some(o) = io_outputs.iter().position(|n| n == name.get_ref()) {
                return Ok(IoSource::OutPad(o));
            }
            self.err(name.span(), format!("\"{}\" is not a declared pad", name.get_ref()))
        };
        let mut io_mux_sources = Vec::new();
        for name in &raw.io.input_mux_sources {
            io_mux_sources.push(io_source(name)?);
        }
        let left_raw = raw.io.left.get_ref();
        if left_raw.len() != rows {
            return self.err(
                raw.io.left.span(),
                format!("io.left has {} rows; the fabric has {}", left_raw.len(), rows),
            );
        }
        let mut io_left = Vec::new();
        for (r, lanes) in left_raw.iter().enumerate() {
            if lanes.len() != horz {
                return self.err(
                    raw.io.left.span(),
                    format!("io.left row {} lists {} lanes; the fabric has {}", r, lanes.len(), horz),
                );
            }
            let mut row = Vec::new();
            for spec in lanes {
                row.push(match spec.get_ref().strip_prefix("sel:") {
                    Some(field) => {
                        let select =
                            self.slice(&Spanned::new(spec.span(), field.to_string()), &io_fields, "io.left")?;
                        if 1usize << select.width != io_mux_sources.len() {
                            return self.err(
                                spec.span(),
                                format!(
                                    "io.left: {} select bits cannot choose between {} sources",
                                    select.width,
                                    io_mux_sources.len()
                                ),
                            );
                        }
                        LeftLane::Mux(select)
                    }
                    None => LeftLane::Fixed(io_source(spec)?),
                });
            }
            io_left.push(row);
        }
        let drive_raw = raw.io.drive.get_ref();
        for (pad, spec) in drive_raw {
            if !io_outputs.contains(pad) {
                return self.err(spec.span(), format!("io.drive: \"{}\" is not an output-side pad", pad));
            }
        }
        let mut io_drive = Vec::new();
        for pad in &io_outputs {
            let Some(spec) = drive_raw.get(pad) else {
                return self.err(raw.io.drive.span(), format!("io.drive: nothing drives \"{}\"", pad));
            };
            let text = spec.get_ref();
            io_drive.push(if let Some(field) = text.strip_prefix("sel:") {
                let select = self.slice(&Spanned::new(spec.span(), field.to_string()), &io_fields, "io.drive")?;
                if 1usize << select.width != rows * horz {
                    return self.err(
                        spec.span(),
                        format!("io.drive: {} select bits cannot address {} lanes", select.width, rows * horz),
                    );
                }
                PadDrive::Mux(select)
            } else {
                let parts: Vec<&str> = text.split(':').collect();
                let parsed = match parts.as_slice() {
                    ["lane", r, l] => r.parse::<usize>().ok().zip(l.parse::<usize>().ok()),
                    _ => None,
                };
                match parsed {
                    Some((row, lane)) if row < rows && lane < horz => PadDrive::Fixed { row, lane },
                    _ => {
                        return self.err(
                            spec.span(),
                            format!("io.drive: \"{}\" must be \"sel:<field>\" or \"lane:R:L\"", text),
                        )
                    }
                }
            });
        }

        let mut chain_groups: Vec<ChainGroup> = Vec::new();
        for g in &raw.bitstream.chain {
            let cells = match g.cells.get_ref().as_str() {
                "clb" => ChainCells::Clb,
                "io" => ChainCells::Io,
                "csb" => ChainCells::Csb,
                other => {
                    return self
                        .err(g.cells.span(), format!("unknown chain cell kind \"{}\"", other))
                }
            };
            let order = match g.order.get_ref().as_str() {
                "row_major" => ChainOrder::RowMajor,
                "column_ascending" => ChainOrder::ColumnAscending,
                "single" => ChainOrder::Single,
                other => {
                    return self.err(g.order.span(), format!("unknown chain order \"{}\"", other))
                }
            };
            if chain_groups.iter().any(|c| c.cells == cells) {
                return self.err(g.cells.span(), format!("chain group \"{}\" appears twice", g.cells.get_ref()));
            }
            chain_groups.push(ChainGroup { cells, order });
        }
        for cells in [ChainCells::Clb, ChainCells::Io, ChainCells::Csb] {
            if !chain_groups.iter().any(|c| c.cells == cells) {
                return self.bare("the scan chain must cover the CLBs, the IO controller and the CSBs".into());
            }
        }
        let start_bit = match &raw.bitstream.start_bit {
            None => None,
            Some(b) if *b.get_ref() <= 1 => Some(*b.get_ref() == 1),
            Some(b) => return self.err(b.span(), "the start bit must be 0 or 1".into()),
        };

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
            lut,
            carry_table,
            carry,
            ff,
            output_muxes,
            csb_fields,
            csb_clock,
            io_fields,
            io_inputs,
            io_outputs,
            ddio,
            io_mux_sources,
            io_left,
            io_drive,
            chain: chain_groups,
            start_bit,
            naming: Naming {
                clb: raw.naming.clb,
                csb: raw.naming.csb,
                io: raw.naming.io,
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
