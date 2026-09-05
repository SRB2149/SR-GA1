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

/// One operation of the fixed core. `table[i]` is the result when input mux
/// k's output equals bit k of `i` (input "a" is bit 0).
#[derive(Debug, Clone)]
pub struct Operation {
    pub name: String,
    pub table: Vec<bool>,
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
    pub name: String,
    pub columns: usize,
    pub rows: usize,
    pub horz_lanes: usize,
    pub vert_lanes: usize,
    pub vertical_ring: bool,
    pub clb_fields: Vec<Field>,
    pub input_muxes: Vec<InputMux>,
    /// The config field slice holding the operation select code.
    pub op_select: FieldSlice,
    /// Indexed by operation select code.
    pub operations: Vec<Operation>,
    pub carry_table: Vec<bool>,
    pub carry: CarrySpec,
    pub ff: FfSpec,
    pub output_muxes: Vec<OutputMux>,
    pub csb_fields: Vec<Field>,
    pub csb_clock: CsbClock,
    /// `io_inputs[row][lane]`: reserved net name entering at the left edge.
    pub io_inputs: Vec<Vec<String>>,
    /// `io_outputs[row][lane]`: chip output read at the right edge, `None` if
    /// the lane is unused there.
    pub io_outputs: Vec<Vec<Option<String>>>,
    pub ddio: Vec<Ddio>,
    pub chain: Vec<ChainGroup>,
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

    pub fn total_bits(&self) -> usize {
        self.columns * self.rows * self.clb_bits() + self.columns * self.csb_bits()
    }

    pub fn clb_field(&self, name: &str) -> Option<&Field> {
        self.clb_fields.iter().find(|f| f.name == name)
    }

    pub fn csb_field(&self, name: &str) -> Option<&Field> {
        self.csb_fields.iter().find(|f| f.name == name)
    }

    pub fn operation_by_name(&self, name: &str) -> Option<usize> {
        self.operations.iter().position(|op| op.name == name)
    }

    /// Evaluate operation `code` on the input mux outputs (input "a" first).
    /// `None` if the code or input count doesn't match the fabric.
    pub fn eval_operation(&self, code: usize, inputs: &[bool]) -> Option<bool> {
        if inputs.len() != self.input_muxes.len() {
            return None;
        }
        self.operations.get(code).map(|op| op.table[table_index(inputs)])
    }

    pub fn eval_carry(&self, inputs: &[bool]) -> Option<bool> {
        if inputs.len() != self.input_muxes.len() {
            return None;
        }
        Some(self.carry_table[table_index(inputs)])
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
}

fn table_index(inputs: &[bool]) -> usize {
    inputs
        .iter()
        .enumerate()
        .fold(0, |acc, (i, &b)| acc | ((b as usize) << i))
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
    operation: RawOperation,
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
struct RawOperation {
    select: Spanned<String>,
    ops: Vec<RawOp>,
    carry_table: Spanned<Vec<u8>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOp {
    code: Spanned<u32>,
    name: Spanned<String>,
    table: Spanned<Vec<u8>>,
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
    inputs: Spanned<Vec<Vec<String>>>,
    outputs: Spanned<Vec<Vec<String>>>,
    ddio: Vec<RawDdio>,
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

    if *raw.schema.get_ref() != 1 {
        return Err(ctx.err(
            raw.schema.span(),
            format!("unsupported schema version {} (expected 1)", raw.schema.get_ref()),
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

    // Operation core.
    let op_select = parse_select(&ctx, &clb_fields, &raw.clb.operation.select, "clb.operation")?;
    let op_count = 1usize << op_select.width;
    let mut operations: Vec<Option<Operation>> = vec![None; op_count];
    for op in &raw.clb.operation.ops {
        let code = *op.code.get_ref() as usize;
        if code >= op_count {
            return Err(ctx.err(
                op.code.span(),
                format!(
                    "operation \"{}\": code {} out of range for {}-bit select (max {})",
                    op.name.get_ref(), code, op_select.width, op_count - 1
                ),
            ));
        }
        if operations[code].is_some() {
            return Err(ctx.err(
                op.code.span(),
                format!("operation \"{}\": code {} is already used", op.name.get_ref(), code),
            ));
        }
        let table = parse_table(&ctx, &op.table, table_len, &format!("operation \"{}\"", op.name.get_ref()))?;
        operations[code] = Some(Operation {
            name: op.name.get_ref().clone(),
            table,
        });
    }
    let operations: Vec<Operation> = operations
        .into_iter()
        .enumerate()
        .map(|(code, op)| {
            op.ok_or_else(|| ctx.err(
                raw.clb.operation.select.span(),
                format!("clb.operation: no operation defined for code {}", code),
            ))
        })
        .collect::<Result<_, _>>()?;
    let carry_table = parse_table(&ctx, &raw.clb.operation.carry_table, table_len, "clb.operation.carry_table")?;

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

    // IO maps.
    let io_inputs = io_grid(&ctx, &raw.io.inputs, rows, horz_lanes, "io.inputs")?;
    for row in &io_inputs {
        for name in row {
            if name.is_empty() {
                return Err(ctx.err(
                    raw.io.inputs.span(),
                    "io.inputs: empty names are not allowed (every lane is driven at the left edge)".to_string(),
                ));
            }
        }
    }
    let io_outputs: Vec<Vec<Option<String>>> = io_grid(&ctx, &raw.io.outputs, rows, horz_lanes, "io.outputs")?
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|name| if name.is_empty() { None } else { Some(name) })
                .collect()
        })
        .collect();

    let mut ddio = Vec::new();
    for d in &raw.io.ddio {
        let input = d.input.get_ref().clone();
        if !io_inputs.iter().flatten().any(|n| *n == input) {
            return Err(ctx.err(
                d.input.span(),
                format!("io.ddio: \"{}\" does not appear in io.inputs", input),
            ));
        }
        let output = d.out.get_ref().clone();
        if !io_outputs.iter().flatten().flatten().any(|n| *n == output) {
            return Err(ctx.err(
                d.out.span(),
                format!("io.ddio: \"{}\" does not appear in io.outputs", output),
            ));
        }
        let dir = d.dir.get_ref().clone();
        if !io_outputs.iter().flatten().flatten().any(|n| *n == dir) {
            return Err(ctx.err(
                d.dir.span(),
                format!("io.ddio: \"{}\" does not appear in io.outputs", dir),
            ));
        }
        ddio.push(Ddio { input, output, dir });
    }

    // Bitstream chain: each cell group exactly once.
    let mut chain = Vec::new();
    for g in raw.bitstream.chain.get_ref() {
        let cells = match g.cells.get_ref().as_str() {
            "clb" => ChainCells::Clb,
            "csb" => ChainCells::Csb,
            other => {
                return Err(ctx.err(
                    g.cells.span(),
                    format!("bitstream.chain: unknown cell group \"{}\" (expected \"clb\" or \"csb\")", other),
                ))
            }
        };
        let order = match (cells, g.order.get_ref().as_str()) {
            (ChainCells::Clb, "row_major") => ChainOrder::RowMajor,
            (ChainCells::Csb, "column_ascending") => ChainOrder::ColumnAscending,
            (ChainCells::Clb, other) => {
                return Err(ctx.err(
                    g.order.span(),
                    format!("bitstream.chain: unknown clb order \"{}\" (expected \"row_major\")", other),
                ))
            }
            (ChainCells::Csb, other) => {
                return Err(ctx.err(
                    g.order.span(),
                    format!("bitstream.chain: unknown csb order \"{}\" (expected \"column_ascending\")", other),
                ))
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
    for (cells, label) in [(ChainCells::Clb, "clb"), (ChainCells::Csb, "csb")] {
        if !chain.iter().any(|c| c.cells == cells) {
            return Err(ctx.err(
                raw.bitstream.chain.span(),
                format!("bitstream.chain: cell group \"{}\" is missing", label),
            ));
        }
    }

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
        chain,
        naming: Naming {
            clb: raw.naming.clb.into_inner(),
            csb: raw.naming.csb.into_inner(),
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

fn io_grid(
    ctx: &Ctx,
    grid: &Spanned<Vec<Vec<String>>>,
    rows: usize,
    lanes: usize,
    what: &str,
) -> Result<Vec<Vec<String>>, FabricError> {
    let raw = grid.get_ref();
    if raw.len() != rows {
        return Err(ctx.err(
            grid.span(),
            format!("{}: {} rows required (fabric.rows = {}), found {}", what, rows, rows, raw.len()),
        ));
    }
    for (i, row) in raw.iter().enumerate() {
        if row.len() != lanes {
            return Err(ctx.err(
                grid.span(),
                format!("{}: row {} has {} lanes, expected {} (buses.horizontal)", what, i, row.len(), lanes),
            ));
        }
    }
    Ok(raw.clone())
}
