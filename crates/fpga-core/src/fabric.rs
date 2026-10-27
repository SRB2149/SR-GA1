//! Loader and validator for `fabric.toml`, the declarative fabric
//! description. The application never parses SystemVerilog: everything it
//! knows about the fabric — dimensions, config fields, mux legality tables,
//! bitstream ordering, naming — comes through [`Fabric::load_str`] or
//! [`Fabric::load_file`]. All load failures are reported as [`FabricError`]
//! with a line number where one is known; nothing here panics on bad input.

use serde::Deserialize;
use std::fmt;
use std::ops::Range;
use std::path::Path;
use toml::Spanned;

// ---------------------------------------------------------------------------
// Errors

#[derive(Debug, Clone)]
pub struct FabricError {
    pub message: String,
    pub line: Option<usize>,
}

impl fmt::Display for FabricError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "fabric.toml:{}: {}", line, self.message),
            None => write!(f, "fabric.toml: {}", self.message),
        }
    }
}

impl std::error::Error for FabricError {}

fn line_at(src: &str, offset: usize) -> usize {
    src[..offset.min(src.len())].bytes().filter(|&b| b == b'\n').count() + 1
}

// ---------------------------------------------------------------------------
// Validated model

/// A source a mux can select. Bus lanes are pass-throughs (the incoming net's
/// name continues onto the driven segment); `Op`/`Reg`/`Carry` introduce a new
/// net named after the CLB; constants carry the reserved constant names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    HorzIn(usize),
    VertIn(usize),
    Const(bool),
    Op,
    Reg,
    Carry,
    /// The dedicated carry chain input, carrying the neighbouring cell's
    /// carry output (see [`CarrySpec`]).
    CarryIn,
}

impl Source {
    pub fn is_pass_through(self) -> bool {
        matches!(self, Source::HorzIn(_) | Source::VertIn(_) | Source::CarryIn)
    }

    pub fn is_local_output(self) -> bool {
        matches!(self, Source::Op | Source::Reg | Source::Carry)
    }
}

/// How carry outputs chain between cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CarryChain {
    /// One chain per column, from row 0 upward, with no wrap at the top.
    ColumnUp,
}

#[derive(Debug, Clone)]
pub struct CarrySpec {
    pub chain: CarryChain,
    /// What the first cell of each chain sees on its carry input.
    pub edge: bool,
}

/// An outgoing bus lane driven by an output mux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusOut {
    Horz(usize),
    Vert(usize),
}

/// One named config field. `offset` is the shift-chain index of its LSB
/// within the owning cell.
#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub width: usize,
    pub offset: usize,
}

/// A mux select: a whole field, or one bit of it (`bit` set, width 1).
/// `field` indexes into the owning cell's field list.
#[derive(Debug, Clone, Copy)]
pub struct FieldSlice {
    pub field: usize,
    pub bit: Option<usize>,
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

/// The 3-input lookup table. `order` lists input mux indices most-significant
/// first, so the LUT output is bit `index` of the `field` value, where `index`
/// is built from the input values in that order.
#[derive(Debug, Clone)]
pub struct LutSpec {
    /// Index into `clb_fields` of the truth-table field.
    pub field: usize,
    /// Input mux indices, most-significant index bit first.
    pub order: Vec<usize>,
    /// Display names for well-known values, in file order.
    pub presets: Vec<(String, u64)>,
}

#[derive(Debug, Clone)]
pub struct FfSpec {
    /// Write-enable source (a bus lane or constant, never a local output).
    pub enable: Source,
    /// Index into `clb_fields` of the 1-bit synchronous reset value.
    pub reset_value_field: usize,
}

#[derive(Debug, Clone)]
pub struct CsbClock {
    pub select: FieldSlice,
    /// Vertical ring lane per select code.
    pub ring_lanes: Vec<usize>,
    /// Index into `csb_fields` of the 1-bit couple-to-previous override.
    pub couple_field: usize,
}

/// A dual-direction pad, as indices into the pad lists: `input` into
/// `io_inputs`, `output` and `dir` into `io_outputs`.
#[derive(Debug, Clone, Copy)]
pub struct Ddio {
    pub input: usize,
    pub output: usize,
    pub dir: usize,
}

/// Something the IO controller can put onto a left-edge lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoSource {
    /// An input-side pad (index into `io_inputs`).
    InPad(usize),
    /// The value the IO controller drives onto an output-side pad (index into
    /// `io_outputs`) — an on-chip loop from the right edge back to the left.
    OutPad(usize),
}

/// What enters one left-edge lane.
#[derive(Debug, Clone, Copy)]
pub enum LeftLane {
    /// An input mux over `io_mux_sources`, selected by an IO config field.
    Mux(FieldSlice),
    Fixed(IoSource),
}

/// What drives one output-side pad from the right edge.
#[derive(Debug, Clone, Copy)]
pub enum PadDrive {
    /// An output mux over every right-edge lane; select code
    /// `row * horz_lanes + lane`.
    Mux(FieldSlice),
    Fixed { row: usize, lane: usize },
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
    pub name: String,
    pub columns: usize,
    pub rows: usize,
    pub horz_lanes: usize,
    pub vert_lanes: usize,
    pub vertical_ring: bool,
    pub clb_fields: Vec<Field>,
    pub input_muxes: Vec<InputMux>,
    pub lut: LutSpec,
    /// Indexed like the LUT (see [`LutSpec::order`]).
    pub carry_table: Vec<bool>,
    pub carry: CarrySpec,
    pub ff: FfSpec,
    pub output_muxes: Vec<OutputMux>,
    pub csb_fields: Vec<Field>,
    pub csb_clock: CsbClock,
    pub io_fields: Vec<Field>,
    /// Input-side pad names: the chip inputs, then the fabric side of each DDIO.
    pub io_inputs: Vec<String>,
    /// Output-side pad names: the chip outputs, then each DDIO's out and dir.
    pub io_outputs: Vec<String>,
    pub ddio: Vec<Ddio>,
    /// Sources of every left-edge input mux, in select-code order.
    pub io_mux_sources: Vec<IoSource>,
    /// `io_left[row][lane]`.
    pub io_left: Vec<Vec<LeftLane>>,
    /// Per output-side pad, indexed like `io_outputs`.
    pub io_drive: Vec<PadDrive>,
    pub chain: Vec<ChainGroup>,
    /// The bit sent ahead of the configuration, if the chip expects one.
    pub start_bit: Option<bool>,
    pub naming: Naming,
}

impl Fabric {
    pub fn load_file(path: &Path) -> Result<Fabric, FabricError> {
        let src = std::fs::read_to_string(path).map_err(|e| FabricError {
            message: format!("cannot read {}: {}", path.display(), e),
            line: None,
        })?;
        Self::load_str(&src)
    }

    pub fn load_str(src: &str) -> Result<Fabric, FabricError> {
        let raw: RawFile = toml::from_str(src).map_err(|e| FabricError {
            message: e.message().to_string(),
            line: e.span().map(|s| line_at(src, s.start)),
        })?;
        build(src, raw)
    }

    pub fn clb_bits(&self) -> usize {
        self.clb_fields.iter().map(|f| f.width).sum()
    }

    pub fn csb_bits(&self) -> usize {
        self.csb_fields.iter().map(|f| f.width).sum()
    }

    pub fn io_bits(&self) -> usize {
        self.io_fields.iter().map(|f| f.width).sum()
    }

    /// Configuration bits in the chain (excluding any start bit).
    pub fn total_bits(&self) -> usize {
        self.columns * self.rows * self.clb_bits() + self.io_bits() + self.columns * self.csb_bits()
    }

    /// Bits in a complete transmission: the start bit, if any, plus the
    /// configuration.
    pub fn stream_bits(&self) -> usize {
        self.total_bits() + self.start_bit.is_some() as usize
    }

    pub fn clb_field(&self, name: &str) -> Option<&Field> {
        self.clb_fields.iter().find(|f| f.name == name)
    }

    pub fn csb_field(&self, name: &str) -> Option<&Field> {
        self.csb_fields.iter().find(|f| f.name == name)
    }

    pub fn io_field(&self, name: &str) -> Option<&Field> {
        self.io_fields.iter().find(|f| f.name == name)
    }

    /// The truth-table index for these input mux outputs (listed in
    /// `input_muxes` order, input "a" first).
    pub fn lut_index(&self, inputs: &[bool]) -> usize {
        self.lut.order.iter().fold(0, |acc, &m| (acc << 1) | inputs[m] as usize)
    }

    /// Evaluate a LUT value on the input mux outputs (input "a" first).
    /// `None` if the input count doesn't match the fabric.
    pub fn eval_lut(&self, lut: u64, inputs: &[bool]) -> Option<bool> {
        if inputs.len() != self.input_muxes.len() {
            return None;
        }
        Some((lut >> self.lut_index(inputs)) & 1 != 0)
    }

    pub fn eval_carry(&self, inputs: &[bool]) -> Option<bool> {
        if inputs.len() != self.input_muxes.len() {
            return None;
        }
        Some(self.carry_table[self.lut_index(inputs)])
    }

    /// The first preset whose value matches `lut`, if any.
    pub fn preset_name(&self, lut: u64) -> Option<&str> {
        self.lut.presets.iter().find(|(_, v)| *v == lut).map(|(n, _)| n.as_str())
    }

    pub fn preset_value(&self, name: &str) -> Option<u64> {
        self.lut.presets.iter().find(|(n, _)| n == name).map(|(_, v)| *v)
    }

    /// The right-edge lane an output mux select code picks.
    pub fn drive_lane(&self, code: usize) -> (usize, usize) {
        (code / self.horz_lanes, code % self.horz_lanes)
    }

    pub fn input_pad(&self, name: &str) -> Option<usize> {
        self.io_inputs.iter().position(|n| n == name)
    }

    pub fn output_pad(&self, name: &str) -> Option<usize> {
        self.io_outputs.iter().position(|n| n == name)
    }

    /// The DDIO whose fabric-side input is this input-side pad.
    pub fn ddio_of_input(&self, pad: usize) -> Option<&Ddio> {
        self.ddio.iter().find(|d| d.input == pad)
    }

    pub fn io_source_name(&self, src: IoSource) -> &str {
        match src {
            IoSource::InPad(i) => &self.io_inputs[i],
            IoSource::OutPad(o) => &self.io_outputs[o],
        }
    }

    pub fn clb_name(&self, col: usize, row: usize) -> String {
        self.naming
            .clb
            .replace("{col}", &col.to_string())
            .replace("{row}", &row.to_string())
    }

    pub fn csb_name(&self, col: usize) -> String {
        self.naming.csb.replace("{col}", &col.to_string())
    }

    pub fn io_name(&self) -> String {
        self.naming.io.clone()
    }
}

// ---------------------------------------------------------------------------
// Raw TOML schema (deserialized 1:1, then validated into the model above)

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    schema: Spanned<u32>,
    fabric: RawFabric,
    buses: RawBuses,
    clb: RawClb,
    csb: RawCsb,
    io: RawIo,
    bitstream: RawBitstream,
    naming: RawNaming,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFabric {
    name: String,
    columns: Spanned<u32>,
    rows: Spanned<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBuses {
    horizontal: Spanned<u32>,
    vertical: Spanned<u32>,
    vertical_ring: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawField {
    name: Spanned<String>,
    width: Spanned<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawClb {
    fields: Vec<RawField>,
    input_muxes: Vec<RawInputMux>,
    lut: RawLut,
    carry: RawCarry,
    ff: RawFf,
    output_muxes: Vec<RawOutputMux>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCarry {
    chain: Spanned<String>,
    edge: Spanned<u8>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInputMux {
    name: String,
    select: Spanned<String>,
    sources: Spanned<Vec<Spanned<String>>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLut {
    field: Spanned<String>,
    index: Spanned<Vec<Spanned<String>>>,
    carry_table: Spanned<Vec<u8>>,
    #[serde(default)]
    presets: Vec<RawPreset>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPreset {
    name: Spanned<String>,
    value: Spanned<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFf {
    data: Spanned<String>,
    enable: Spanned<String>,
    edge: Spanned<String>,
    reset: Spanned<String>,
    reset_value_field: Spanned<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOutputMux {
    drives: Spanned<String>,
    select: Spanned<String>,
    sources: Spanned<Vec<Spanned<String>>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCsb {
    fields: Vec<RawField>,
    clock: RawCsbClock,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCsbClock {
    select: Spanned<String>,
    sources: Spanned<Vec<Spanned<String>>>,
    couple_field: Spanned<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIo {
    fields: Vec<RawField>,
    inputs: Spanned<Vec<Spanned<String>>>,
    outputs: Spanned<Vec<Spanned<String>>>,
    ddio: Vec<RawDdio>,
    input_mux_sources: Spanned<Vec<Spanned<String>>>,
    left: Spanned<Vec<Vec<Spanned<String>>>>,
    drive: Spanned<std::collections::BTreeMap<String, Spanned<String>>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDdio {
    #[serde(rename = "in")]
    input: Spanned<String>,
    out: Spanned<String>,
    dir: Spanned<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBitstream {
    chain: Spanned<Vec<RawChainGroup>>,
    start_bit: Option<Spanned<u8>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawChainGroup {
    cells: Spanned<String>,
    order: Spanned<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNaming {
    clb: Spanned<String>,
    csb: Spanned<String>,
    io: String,
    suffix_op: String,
    suffix_reg: String,
    suffix_carry: String,
    constant_zero: String,
    constant_one: String,
    clock_fallback: Spanned<String>,
    unconnected: String,
}

// ---------------------------------------------------------------------------
// Validation

struct Ctx<'a> {
    src: &'a str,
}

impl<'a> Ctx<'a> {
    fn err(&self, span: Range<usize>, message: String) -> FabricError {
        FabricError {
            message,
            line: Some(line_at(self.src, span.start)),
        }
    }
}

fn build(src: &str, raw: RawFile) -> Result<Fabric, FabricError> {
    let ctx = Ctx { src };

    if *raw.schema.get_ref() != 2 {
        return Err(ctx.err(
            raw.schema.span(),
            format!("unsupported schema version {} (expected 2)", raw.schema.get_ref()),
        ));
    }

    let columns = positive(&ctx, &raw.fabric.columns, "fabric.columns")?;
    let rows = positive(&ctx, &raw.fabric.rows, "fabric.rows")?;
    let horz_lanes = positive(&ctx, &raw.buses.horizontal, "buses.horizontal")?;
    let vert_lanes = positive(&ctx, &raw.buses.vertical, "buses.vertical")?;

    let clb_fields = build_fields(&ctx, "clb", &raw.clb.fields)?;
    let csb_fields = build_fields(&ctx, "csb", &raw.csb.fields)?;

    // Input muxes: bus lanes and constants only — a CLB output feeding its
    // own operation input would be an unconditional combinational loop.
    let mut input_muxes = Vec::new();
    for m in &raw.clb.input_muxes {
        let what = format!("input mux \"{}\"", m.name);
        let select = parse_select(&ctx, &clb_fields, &m.select, &what)?;
        let sources = parse_sources(&ctx, &m.sources, select.width, horz_lanes, vert_lanes, &what)?;
        for (tok, s) in m.sources.get_ref().iter().zip(&sources) {
            if s.is_local_output() {
                return Err(ctx.err(
                    tok.span(),
                    format!("{}: source \"{}\" is a CLB output; input muxes can only select bus lanes or constants", what, tok.get_ref()),
                ));
            }
        }
        input_muxes.push(InputMux {
            name: m.name.clone(),
            select,
            sources,
        });
    }
    if input_muxes.is_empty() {
        return Err(FabricError {
            message: "clb.input_muxes must not be empty".to_string(),
            line: None,
        });
    }
    let table_len = 1usize << input_muxes.len();

    // Lookup table.
    let raw_lut = &raw.clb.lut;
    let lut_field = require_field(&ctx, &clb_fields, &raw_lut.field, table_len, "clb.lut.field")?;
    let mut order = Vec::new();
    for tok in raw_lut.index.get_ref() {
        let m = input_muxes.iter().position(|m| m.name == *tok.get_ref()).ok_or_else(|| {
            ctx.err(tok.span(), format!("clb.lut.index: \"{}\" is not an input mux", tok.get_ref()))
        })?;
        if order.contains(&m) {
            return Err(ctx.err(tok.span(), format!("clb.lut.index: \"{}\" appears twice", tok.get_ref())));
        }
        order.push(m);
    }
    if order.len() != input_muxes.len() {
        return Err(ctx.err(
            raw_lut.index.span(),
            format!("clb.lut.index must list every input mux ({} of them)", input_muxes.len()),
        ));
    }
    let mut presets = Vec::new();
    for p in &raw_lut.presets {
        let value = *p.value.get_ref();
        if table_len < 64 && value >> table_len != 0 {
            return Err(ctx.err(
                p.value.span(),
                format!("clb.lut.presets: \"{}\" = {:#x} does not fit in {} bits", p.name.get_ref(), value, table_len),
            ));
        }
        presets.push((p.name.get_ref().clone(), value));
    }
    let lut = LutSpec { field: lut_field, order, presets };
    let carry_table = parse_table(&ctx, &raw_lut.carry_table, table_len, "clb.lut.carry_table")?;

    // Carry chain.
    let carry = CarrySpec {
        chain: match raw.clb.carry.chain.get_ref().as_str() {
            "column_up" => CarryChain::ColumnUp,
            other => {
                return Err(ctx.err(
                    raw.clb.carry.chain.span(),
                    format!("clb.carry.chain \"{}\" is unsupported (expected \"column_up\")", other),
                ))
            }
        },
        edge: match raw.clb.carry.edge.get_ref() {
            0 => false,
            1 => true,
            other => {
                return Err(ctx.err(
                    raw.clb.carry.edge.span(),
                    format!("clb.carry.edge must be 0 or 1, found {}", other),
                ))
            }
        },
    };

    // Flip-flop.
    if raw.clb.ff.data.get_ref() != "op" {
        return Err(ctx.err(
            raw.clb.ff.data.span(),
            format!("clb.ff.data \"{}\" is unsupported (expected \"op\")", raw.clb.ff.data.get_ref()),
        ));
    }
    if raw.clb.ff.edge.get_ref() != "rising" {
        return Err(ctx.err(
            raw.clb.ff.edge.span(),
            format!("clb.ff.edge \"{}\" is unsupported (expected \"rising\")", raw.clb.ff.edge.get_ref()),
        ));
    }
    if raw.clb.ff.reset.get_ref() != "sync" {
        return Err(ctx.err(
            raw.clb.ff.reset.span(),
            format!("clb.ff.reset \"{}\" is unsupported (expected \"sync\")", raw.clb.ff.reset.get_ref()),
        ));
    }
    let ff_enable = parse_source(raw.clb.ff.enable.get_ref(), horz_lanes, vert_lanes)
        .map_err(|msg| ctx.err(raw.clb.ff.enable.span(), format!("clb.ff.enable: {}", msg)))?;
    if ff_enable.is_local_output() {
        return Err(ctx.err(
            raw.clb.ff.enable.span(),
            "clb.ff.enable must be a bus lane or constant".to_string(),
        ));
    }
    let reset_value_field = require_field(&ctx, &clb_fields, &raw.clb.ff.reset_value_field, 1, "clb.ff.reset_value_field")?;
    let ff = FfSpec {
        enable: ff_enable,
        reset_value_field,
    };

    // Output muxes: every outgoing lane driven by exactly one mux.
    let mut output_muxes = Vec::new();
    let mut horz_driven = vec![false; horz_lanes];
    let mut vert_driven = vec![false; vert_lanes];
    for m in &raw.clb.output_muxes {
        let drives = parse_bus_out(&ctx, &m.drives, horz_lanes, vert_lanes)?;
        let driven = match drives {
            BusOut::Horz(n) => &mut horz_driven[n],
            BusOut::Vert(n) => &mut vert_driven[n],
        };
        if *driven {
            return Err(ctx.err(
                m.drives.span(),
                format!("output mux: \"{}\" is already driven by another mux", m.drives.get_ref()),
            ));
        }
        *driven = true;
        let what = format!("output mux for \"{}\"", m.drives.get_ref());
        let select = parse_select(&ctx, &clb_fields, &m.select, &what)?;
        let sources = parse_sources(&ctx, &m.sources, select.width, horz_lanes, vert_lanes, &what)?;
        output_muxes.push(OutputMux {
            drives,
            select,
            sources,
        });
    }
    for (lane, driven) in horz_driven.iter().enumerate() {
        if !driven {
            return Err(ctx.err(
                raw.buses.horizontal.span(),
                format!("no output mux drives h_out:{} — every outgoing lane needs exactly one", lane),
            ));
        }
    }
    for (lane, driven) in vert_driven.iter().enumerate() {
        if !driven {
            return Err(ctx.err(
                raw.buses.vertical.span(),
                format!("no output mux drives v_out:{} — every outgoing lane needs exactly one", lane),
            ));
        }
    }

    // CSB clock mux.
    let csb_select = parse_select(&ctx, &csb_fields, &raw.csb.clock.select, "csb.clock")?;
    let want = 1usize << csb_select.width;
    let raw_ring = raw.csb.clock.sources.get_ref();
    if raw_ring.len() != want {
        return Err(ctx.err(
            raw.csb.clock.sources.span(),
            format!(
                "csb.clock: {} sources required for {}-bit select \"{}\", found {}",
                want, csb_select.width, raw.csb.clock.select.get_ref(), raw_ring.len()
            ),
        ));
    }
    let mut ring_lanes = Vec::new();
    for tok in raw_ring {
        let lane = tok
            .get_ref()
            .strip_prefix("v_ring:")
            .and_then(|n| n.parse::<usize>().ok())
            .ok_or_else(|| ctx.err(
                tok.span(),
                format!("csb.clock: unknown source token \"{}\" (expected \"v_ring:N\")", tok.get_ref()),
            ))?;
        if lane >= vert_lanes {
            return Err(ctx.err(
                tok.span(),
                format!("csb.clock: vertical ring lane {} out of range (buses.vertical = {})", lane, vert_lanes),
            ));
        }
        ring_lanes.push(lane);
    }
    let couple_field = require_field(&ctx, &csb_fields, &raw.csb.clock.couple_field, 1, "csb.clock.couple_field")?;
    let csb_clock = CsbClock {
        select: csb_select,
        ring_lanes,
        couple_field,
    };

    // IO controller.
    let io_fields = build_fields(&ctx, "io", &raw.io.fields)?;
    let mut all_pads: Vec<String> = Vec::new();
    let mut pad_list = |list: &Spanned<Vec<Spanned<String>>>, what: &str| -> Result<Vec<String>, FabricError> {
        let mut out = Vec::new();
        for tok in list.get_ref() {
            let name = tok.get_ref();
            if name.is_empty() {
                return Err(ctx.err(tok.span(), format!("{}: empty pad name", what)));
            }
            if all_pads.contains(name) {
                return Err(ctx.err(tok.span(), format!("{}: pad \"{}\" is declared twice", what, name)));
            }
            all_pads.push(name.clone());
            out.push(name.clone());
        }
        Ok(out)
    };
    let mut io_inputs = pad_list(&raw.io.inputs, "io.inputs")?;
    let mut io_outputs = pad_list(&raw.io.outputs, "io.outputs")?;
    // DDIO pads join the lists: fabric-side inputs after the chip inputs, and
    // outs then dirs after the chip outputs.
    let mut ddio_names = Vec::new();
    for d in &raw.io.ddio {
        for tok in [&d.input, &d.out, &d.dir] {
            if all_pads.contains(tok.get_ref()) {
                return Err(ctx.err(tok.span(), format!("io.ddio: pad \"{}\" is declared twice", tok.get_ref())));
            }
            all_pads.push(tok.get_ref().clone());
        }
        ddio_names.push((d.input.get_ref().clone(), d.out.get_ref().clone(), d.dir.get_ref().clone()));
    }
    let mut ddio = Vec::new();
    for (i, _, _) in &ddio_names {
        io_inputs.push(i.clone());
    }
    for (_, o, _) in &ddio_names {
        io_outputs.push(o.clone());
    }
    for (_, _, d) in &ddio_names {
        io_outputs.push(d.clone());
    }
    for (i, o, d) in &ddio_names {
        ddio.push(Ddio {
            input: io_inputs.iter().position(|n| n == i).expect("just pushed"),
            output: io_outputs.iter().position(|n| n == o).expect("just pushed"),
            dir: io_outputs.iter().position(|n| n == d).expect("just pushed"),
        });
    }
    let io_source = |tok: &Spanned<String>, what: &str| -> Result<IoSource, FabricError> {
        let name = tok.get_ref();
        if let Some(i) = io_inputs.iter().position(|n| n == name) {
            return Ok(IoSource::InPad(i));
        }
        if let Some(o) = io_outputs.iter().position(|n| n == name) {
            return Ok(IoSource::OutPad(o));
        }
        Err(ctx.err(tok.span(), format!("{}: \"{}\" is not a declared pad", what, name)))
    };
    let mut io_mux_sources = Vec::new();
    for tok in raw.io.input_mux_sources.get_ref() {
        io_mux_sources.push(io_source(tok, "io.input_mux_sources")?);
    }
    let raw_left = raw.io.left.get_ref();
    if raw_left.len() != rows {
        return Err(ctx.err(
            raw.io.left.span(),
            format!("io.left: {} rows required (fabric.rows = {}), found {}", rows, rows, raw_left.len()),
        ));
    }
    let mut io_left = Vec::new();
    for (r, row) in raw_left.iter().enumerate() {
        if row.len() != horz_lanes {
            return Err(ctx.err(
                raw.io.left.span(),
                format!("io.left: row {} has {} lanes, expected {} (buses.horizontal)", r, row.len(), horz_lanes),
            ));
        }
        let mut lanes = Vec::new();
        for tok in row {
            lanes.push(match tok.get_ref().strip_prefix("sel:") {
                Some(field) => {
                    let sel = parse_select(&ctx, &io_fields, &Spanned::new(tok.span(), field.to_string()), "io.left")?;
                    if (1usize << sel.width) != io_mux_sources.len() {
                        return Err(ctx.err(
                            tok.span(),
                            format!(
                                "io.left: a {}-bit select cannot address the {} input mux sources",
                                sel.width,
                                io_mux_sources.len()
                            ),
                        ));
                    }
                    LeftLane::Mux(sel)
                }
                None => LeftLane::Fixed(io_source(tok, "io.left")?),
            });
        }
        io_left.push(lanes);
    }
    let raw_drive = raw.io.drive.get_ref();
    for (pad, tok) in raw_drive {
        if !io_outputs.contains(pad) {
            return Err(ctx.err(tok.span(), format!("io.drive: \"{}\" is not an output-side pad", pad)));
        }
    }
    let mut io_drive = Vec::new();
    for pad in &io_outputs {
        let tok = raw_drive.get(pad).ok_or_else(|| {
            ctx.err(raw.io.drive.span(), format!("io.drive: nothing drives output-side pad \"{}\"", pad))
        })?;
        let text = tok.get_ref();
        io_drive.push(if let Some(field) = text.strip_prefix("sel:") {
            let sel = parse_select(&ctx, &io_fields, &Spanned::new(tok.span(), field.to_string()), "io.drive")?;
            if (1usize << sel.width) != rows * horz_lanes {
                return Err(ctx.err(
                    tok.span(),
                    format!("io.drive: a {}-bit select cannot address the {} right-edge lanes", sel.width, rows * horz_lanes),
                ));
            }
            PadDrive::Mux(sel)
        } else {
            let mut parts = text.strip_prefix("lane:").unwrap_or("").split(':');
            let (Some(Ok(row)), Some(Ok(lane)), None) = (
                parts.next().map(str::parse::<usize>),
                parts.next().map(str::parse::<usize>),
                parts.next(),
            ) else {
                return Err(ctx.err(
                    tok.span(),
                    format!("io.drive: \"{}\" must be \"sel:<field>\" or \"lane:R:L\"", text),
                ));
            };
            if row >= rows || lane >= horz_lanes {
                return Err(ctx.err(tok.span(), format!("io.drive: lane {}:{} is outside the fabric", row, lane)));
            }
            PadDrive::Fixed { row, lane }
        });
    }

    // Bitstream chain: each cell group exactly once.
    let mut chain = Vec::new();
    for g in raw.bitstream.chain.get_ref() {
        let cells = match g.cells.get_ref().as_str() {
            "clb" => ChainCells::Clb,
            "io" => ChainCells::Io,
            "csb" => ChainCells::Csb,
            other => {
                return Err(ctx.err(
                    g.cells.span(),
                    format!("bitstream.chain: unknown cell group \"{}\" (expected \"clb\", \"io\" or \"csb\")", other),
                ))
            }
        };
        let order = match (cells, g.order.get_ref().as_str()) {
            (ChainCells::Clb, "row_major") => ChainOrder::RowMajor,
            (ChainCells::Io, "single") => ChainOrder::Single,
            (ChainCells::Csb, "column_ascending") => ChainOrder::ColumnAscending,
            (_, other) => {
                let want = match cells {
                    ChainCells::Clb => "row_major",
                    ChainCells::Io => "single",
                    ChainCells::Csb => "column_ascending",
                };
                return Err(ctx.err(
                    g.order.span(),
                    format!(
                        "bitstream.chain: unknown {} order \"{}\" (expected \"{}\")",
                        g.cells.get_ref(), other, want
                    ),
                ));
            }
        };
        if chain.iter().any(|c: &ChainGroup| c.cells == cells) {
            return Err(ctx.err(
                g.cells.span(),
                format!("bitstream.chain: cell group \"{}\" appears twice", g.cells.get_ref()),
            ));
        }
        chain.push(ChainGroup { cells, order });
    }
    for (cells, label) in [(ChainCells::Clb, "clb"), (ChainCells::Io, "io"), (ChainCells::Csb, "csb")] {
        if !chain.iter().any(|c| c.cells == cells) {
            return Err(ctx.err(
                raw.bitstream.chain.span(),
                format!("bitstream.chain: cell group \"{}\" is missing", label),
            ));
        }
    }
    let start_bit = match &raw.bitstream.start_bit {
        None => None,
        Some(b) => match b.get_ref() {
            0 => Some(false),
            1 => Some(true),
            other => {
                return Err(ctx.err(b.span(), format!("bitstream.start_bit must be 0 or 1, found {}", other)))
            }
        },
    };

    // Naming templates.
    for (tpl, what, need_row) in [
        (&raw.naming.clb, "naming.clb", true),
        (&raw.naming.csb, "naming.csb", false),
        (&raw.naming.clock_fallback, "naming.clock_fallback", false),
    ] {
        if !tpl.get_ref().contains("{col}") {
            return Err(ctx.err(tpl.span(), format!("{}: template must contain {{col}}", what)));
        }
        if need_row && !tpl.get_ref().contains("{row}") {
            return Err(ctx.err(tpl.span(), format!("{}: template must contain {{row}}", what)));
        }
    }

    Ok(Fabric {
        name: raw.fabric.name,
        columns,
        rows,
        horz_lanes,
        vert_lanes,
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
        chain,
        start_bit,
        naming: Naming {
            clb: raw.naming.clb.into_inner(),
            csb: raw.naming.csb.into_inner(),
            io: raw.naming.io,
            suffix_op: raw.naming.suffix_op,
            suffix_reg: raw.naming.suffix_reg,
            suffix_carry: raw.naming.suffix_carry,
            constant_zero: raw.naming.constant_zero,
            constant_one: raw.naming.constant_one,
            clock_fallback: raw.naming.clock_fallback.into_inner(),
            unconnected: raw.naming.unconnected,
        },
    })
}

fn positive(ctx: &Ctx, value: &Spanned<u32>, what: &str) -> Result<usize, FabricError> {
    let v = *value.get_ref() as usize;
    if v == 0 {
        return Err(ctx.err(value.span(), format!("{} must be at least 1", what)));
    }
    Ok(v)
}

fn build_fields(ctx: &Ctx, cell: &str, raws: &[RawField]) -> Result<Vec<Field>, FabricError> {
    let mut fields: Vec<Field> = Vec::new();
    let mut offset = 0;
    for f in raws {
        let name = f.name.get_ref();
        if name.is_empty() {
            return Err(ctx.err(f.name.span(), format!("{}.fields: empty field name", cell)));
        }
        if fields.iter().any(|g| g.name == *name) {
            return Err(ctx.err(
                f.name.span(),
                format!("{}.fields: duplicate field name \"{}\"", cell, name),
            ));
        }
        let width = *f.width.get_ref() as usize;
        if width == 0 {
            return Err(ctx.err(
                f.width.span(),
                format!("{}.fields: field \"{}\" must be at least 1 bit wide", cell, name),
            ));
        }
        fields.push(Field {
            name: name.clone(),
            width,
            offset,
        });
        offset += width;
    }
    Ok(fields)
}

/// Parse a select reference: a field name, or `name[bit]` for one bit of a
/// multi-bit field.
fn parse_select(
    ctx: &Ctx,
    fields: &[Field],
    select: &Spanned<String>,
    what: &str,
) -> Result<FieldSlice, FabricError> {
    let text = select.get_ref();
    let (name, bit) = match text.find('[') {
        Some(open) => {
            let bit = text[open + 1..]
                .strip_suffix(']')
                .and_then(|n| n.parse::<usize>().ok())
                .ok_or_else(|| ctx.err(
                    select.span(),
                    format!("{}: malformed select \"{}\" (expected \"field\" or \"field[bit]\")", what, text),
                ))?;
            (&text[..open], Some(bit))
        }
        None => (text.as_str(), None),
    };
    let field = fields.iter().position(|f| f.name == name).ok_or_else(|| {
        let known: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        ctx.err(
            select.span(),
            format!("{}: select \"{}\" is not a config field (known fields: {})", what, name, known.join(", ")),
        )
    })?;
    if let Some(b) = bit {
        if b >= fields[field].width {
            return Err(ctx.err(
                select.span(),
                format!("{}: bit {} out of range for {}-bit field \"{}\"", what, b, fields[field].width, name),
            ));
        }
    }
    let width = if bit.is_some() { 1 } else { fields[field].width };
    Ok(FieldSlice { field, bit, width })
}

fn require_field(
    ctx: &Ctx,
    fields: &[Field],
    name: &Spanned<String>,
    width: usize,
    what: &str,
) -> Result<usize, FabricError> {
    let idx = fields
        .iter()
        .position(|f| f.name == *name.get_ref())
        .ok_or_else(|| ctx.err(
            name.span(),
            format!("{}: \"{}\" is not a config field", what, name.get_ref()),
        ))?;
    if fields[idx].width != width {
        return Err(ctx.err(
            name.span(),
            format!("{}: field \"{}\" must be {} bit(s) wide, is {}", what, name.get_ref(), width, fields[idx].width),
        ));
    }
    Ok(idx)
}

fn parse_source(token: &str, horz_lanes: usize, vert_lanes: usize) -> Result<Source, String> {
    if let Some(n) = token.strip_prefix("h_in:") {
        let lane = n.parse::<usize>().map_err(|_| format!("bad lane in \"{}\"", token))?;
        if lane >= horz_lanes {
            return Err(format!("horizontal lane {} out of range (buses.horizontal = {})", lane, horz_lanes));
        }
        return Ok(Source::HorzIn(lane));
    }
    if let Some(n) = token.strip_prefix("v_in:") {
        let lane = n.parse::<usize>().map_err(|_| format!("bad lane in \"{}\"", token))?;
        if lane >= vert_lanes {
            return Err(format!("vertical lane {} out of range (buses.vertical = {})", lane, vert_lanes));
        }
        return Ok(Source::VertIn(lane));
    }
    match token {
        "const:0" => Ok(Source::Const(false)),
        "const:1" => Ok(Source::Const(true)),
        "op" => Ok(Source::Op),
        "reg" => Ok(Source::Reg),
        "carry" => Ok(Source::Carry),
        "carry_in" => Ok(Source::CarryIn),
        _ => Err(format!("unknown source token \"{}\"", token)),
    }
}

fn parse_sources(
    ctx: &Ctx,
    sources: &Spanned<Vec<Spanned<String>>>,
    select_width: usize,
    horz_lanes: usize,
    vert_lanes: usize,
    what: &str,
) -> Result<Vec<Source>, FabricError> {
    let want = 1usize << select_width;
    let raw = sources.get_ref();
    if raw.len() != want {
        return Err(ctx.err(
            sources.span(),
            format!("{}: {} sources required for {}-bit select, found {}", what, want, select_width, raw.len()),
        ));
    }
    raw.iter()
        .map(|tok| {
            parse_source(tok.get_ref(), horz_lanes, vert_lanes)
                .map_err(|msg| ctx.err(tok.span(), format!("{}: {}", what, msg)))
        })
        .collect()
}

fn parse_bus_out(
    ctx: &Ctx,
    drives: &Spanned<String>,
    horz_lanes: usize,
    vert_lanes: usize,
) -> Result<BusOut, FabricError> {
    let text = drives.get_ref();
    if let Some(n) = text.strip_prefix("h_out:") {
        let lane = n.parse::<usize>().ok().filter(|&l| l < horz_lanes).ok_or_else(|| ctx.err(
            drives.span(),
            format!("output mux: bad horizontal lane in \"{}\" (buses.horizontal = {})", text, horz_lanes),
        ))?;
        return Ok(BusOut::Horz(lane));
    }
    if let Some(n) = text.strip_prefix("v_out:") {
        let lane = n.parse::<usize>().ok().filter(|&l| l < vert_lanes).ok_or_else(|| ctx.err(
            drives.span(),
            format!("output mux: bad vertical lane in \"{}\" (buses.vertical = {})", text, vert_lanes),
        ))?;
        return Ok(BusOut::Vert(lane));
    }
    Err(ctx.err(
        drives.span(),
        format!("output mux: unknown target \"{}\" (expected \"h_out:N\" or \"v_out:N\")", text),
    ))
}

fn parse_table(
    ctx: &Ctx,
    table: &Spanned<Vec<u8>>,
    len: usize,
    what: &str,
) -> Result<Vec<bool>, FabricError> {
    let raw = table.get_ref();
    if raw.len() != len {
        return Err(ctx.err(
            table.span(),
            format!("{}: truth table must have {} entries, found {}", what, len, raw.len()),
        ));
    }
    raw.iter()
        .map(|&v| match v {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(ctx.err(
                table.span(),
                format!("{}: truth table entries must be 0 or 1, found {}", what, v),
            )),
        })
        .collect()
}
