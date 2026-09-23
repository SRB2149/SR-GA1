//! Equivalence checking under Verilator.
//!
//! The bitstream is emitted, then **decoded back** and turned into a flat
//! SystemVerilog model of the configured fabric. That model is simulated
//! side-by-side with the original design over random vectors, and any
//! disagreement is a hard failure naming the cycle, the vector and the signal.
//!
//! Decoding rather than re-using the internal configuration is deliberate: it
//! means the check covers the bitstream encoding, the mux semantics, the
//! operation tables, the flip-flop's fused data path, the lane-3 enables and the
//! derived column clocks — the whole chain from netlist to bits, not just the
//! parts above it.
//!
//! The generated model is a plain netlist rather than a fabric with muxes,
//! because the configuration is known: every mux collapses to the one source it
//! selected. It is also known to be acyclic, since `loops.rs` refuses anything
//! else, which is what keeps Verilator from tripping over circular logic.

use crate::design::Design;
use crate::fabric::{BusOut, Fabric, FieldSlice, Source};
use std::fmt;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct EquivError {
    pub message: String,
    pub log: Option<String>,
}

impl fmt::Display for EquivError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(log) = &self.log {
            let tail: Vec<&str> = log.lines().rev().take(30).collect();
            for line in tail.into_iter().rev() {
                write!(f, "\n  {}", line)?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for EquivError {}

/// What a completed check found.
#[derive(Debug, Clone)]
pub struct EquivResult {
    pub cycles: usize,
    pub compared: Vec<String>,
    /// Inputs that were driven, excluding the clock and reset.
    pub driven: Vec<String>,
}

// ---------------------------------------------------------------------------
// Tool discovery

/// Verilator, and the compiler it needs for the model it writes.
#[derive(Debug, Clone)]
pub struct Verilator {
    pub exe: PathBuf,
    pub version: String,
    /// `VERILATOR_ROOT`, needed when calling the binary directly.
    root: Option<PathBuf>,
}

fn missing() -> EquivError {
    EquivError {
        message: "Verilator was not found, so the synthesised configuration cannot be checked \
                  against the source.\n\n\
                  Install one of:\n  \
                  - OSS CAD Suite (bundles Verilator, Yosys and ABC):\n    \
                  https://github.com/YosysHQ/oss-cad-suite-build/releases\n  \
                  - Debian/Ubuntu: apt install verilator\n  \
                  - macOS: brew install verilator\n\n\
                  Then put it on PATH, set VERILATOR, or pass --verilator <path>. \
                  Re-run without --equiv to skip the check."
            .to_string(),
        log: None,
    }
}

impl Verilator {
    pub fn discover(explicit: Option<&Path>) -> Result<Verilator, EquivError> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(p) = explicit {
            candidates.push(p.to_path_buf());
        }
        if let Some(env) = std::env::var_os("VERILATOR") {
            candidates.push(PathBuf::from(env));
        }
        // The plain `verilator` launcher is a Perl script, and on Windows the
        // Perl on PATH often lacks the modules it wants. `verilator_bin` is the
        // real executable and needs only VERILATOR_ROOT, so it is tried first.
        for root in ["C:/oss-cad-suite", "/opt/oss-cad-suite", "/usr/local/oss-cad-suite"] {
            candidates.push(Path::new(root).join("bin").join(exe_name("verilator_bin")));
        }
        candidates.push(PathBuf::from(exe_name("verilator_bin")));
        candidates.push(PathBuf::from("verilator"));
        candidates.push(PathBuf::from("/usr/bin/verilator"));

        for candidate in candidates {
            let root = share_root(&candidate);
            if let Some(version) = probe(&candidate, root.as_deref()) {
                return Ok(Verilator { exe: candidate, version, root });
            }
        }
        Err(missing())
    }

    pub fn command(&self) -> Command {
        let mut cmd = Command::new(&self.exe);
        if let Some(root) = &self.root {
            cmd.env("VERILATOR_ROOT", root);
        }
        cmd
    }
}

fn exe_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{}.exe", base)
    } else {
        base.to_string()
    }
}

/// `<root>/bin/verilator_bin` keeps its runtime headers in `<root>/share/verilator`.
///
/// The path is normalised to forward slashes: Verilator passes `VERILATOR_ROOT`
/// straight into the makefile it generates, and `make` eats backslashes, which
/// turns the include path into nonsense.
fn share_root(exe: &Path) -> Option<PathBuf> {
    let bin = exe.parent()?;
    if bin.file_name()? != "bin" {
        return None;
    }
    let candidate = bin.parent()?.join("share").join("verilator");
    if !candidate.is_dir() {
        return None;
    }
    Some(PathBuf::from(candidate.to_string_lossy().replace('\\', "/")))
}

fn probe(exe: &Path, root: Option<&Path>) -> Option<String> {
    let mut cmd = Command::new(exe);
    if let Some(root) = root {
        cmd.env("VERILATOR_ROOT", root);
    }
    let output = cmd.arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_string())
}

// ---------------------------------------------------------------------------
// The generated fabric model

/// Read a mux select out of a configuration.
fn select(config: &Design, col: usize, row: usize, slice: FieldSlice) -> u64 {
    let raw = config.clb(col, row).get(slice.field);
    match slice.bit {
        Some(bit) => raw >> bit & 1,
        None => raw,
    }
}

fn hseg(row: usize, col: usize, lane: usize) -> String {
    format!("h_r{}_c{}_l{}", row, col, lane)
}

fn vseg(col: usize, row: usize, lane: usize) -> String {
    format!("v_c{}_r{}_l{}", col, row, lane)
}

/// The SystemVerilog expression for whatever a mux source carries.
fn source_expr(fabric: &Fabric, col: usize, row: usize, source: Source) -> String {
    match source {
        Source::HorzIn(lane) => hseg(row, col, lane),
        Source::VertIn(lane) => vseg(col, row, lane),
        Source::Const(v) => format!("1'b{}", u8::from(v)),
        Source::Op => format!("op_c{}_r{}", col, row),
        Source::Reg => format!("reg_c{}_r{}", col, row),
        Source::Carry => format!("cy_c{}_r{}", col, row),
        Source::CarryIn => {
            if row == 0 {
                format!("1'b{}", u8::from(fabric.carry.edge))
            } else {
                format!("cy_c{}_r{}", col, row - 1)
            }
        }
        Source::VRing(lane) => vseg(col, 0, lane),
    }
}

/// Emit a flat SystemVerilog model of the configured fabric.
///
/// Ports are named after the chip pads, so the testbench can wire them up by
/// name and a loop-around wire is one `assign` in the testbench.
pub fn fabric_model(fabric: &Fabric, config: &Design, module: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "// Generated by sr-ga1-synth from the emitted bitstream.");
    let _ = writeln!(out, "// Every mux is collapsed to the source its configuration selects.");
    let _ = writeln!(out, "`default_nettype none");
    let _ = writeln!(out);

    // ---- ports -------------------------------------------------------------
    let inputs: Vec<&String> = fabric
        .io_inputs
        .iter()
        .flatten()
        .filter(|n| **n != fabric.naming.constant_zero && **n != fabric.naming.constant_one)
        .collect();
    let outputs: Vec<&String> = fabric.io_outputs.iter().flatten().flatten().collect();

    let _ = writeln!(out, "module {} (", module);
    let _ = writeln!(out, "    input  wire reset,");
    for name in &inputs {
        let _ = writeln!(out, "    input  wire {},", name);
    }
    for (index, name) in outputs.iter().enumerate() {
        let comma = if index + 1 == outputs.len() { "" } else { "," };
        let _ = writeln!(out, "    output wire {}{}", name, comma);
    }
    let _ = writeln!(out, ");");
    let _ = writeln!(out);

    // ---- truth tables ------------------------------------------------------
    // A literal cannot be indexed in SystemVerilog, so each operation's table
    // becomes a named constant and the cell indexes that.
    let _ = writeln!(out, "    // Operation truth tables, indexed by {{c, b, a}}.");
    for op in &fabric.operations {
        let _ = writeln!(
            out,
            "    localparam logic [{}:0] TBL_{} = {};",
            op.table.len() - 1,
            op.name.to_uppercase(),
            table_literal(&op.table)
        );
    }
    let _ = writeln!(
        out,
        "    localparam logic [{}:0] TBL_CARRY = {};",
        fabric.carry_table.len() - 1,
        table_literal(&fabric.carry_table)
    );
    let _ = writeln!(out);

    // ---- wires -------------------------------------------------------------
    for row in 0..fabric.rows {
        for col in 0..=fabric.columns {
            for lane in 0..fabric.horz_lanes {
                let _ = writeln!(out, "    wire {};", hseg(row, col, lane));
            }
        }
    }
    for col in 0..fabric.columns {
        for row in 0..fabric.rows {
            for lane in 0..fabric.vert_lanes {
                let _ = writeln!(out, "    wire {};", vseg(col, row, lane));
            }
        }
    }
    for col in 0..fabric.columns {
        for row in 0..fabric.rows {
            let _ = writeln!(out, "    wire op_c{}_r{};", col, row);
            let _ = writeln!(out, "    reg  reg_c{}_r{};", col, row);
            let _ = writeln!(out, "    wire cy_c{}_r{};", col, row);
        }
        let _ = writeln!(out, "    wire clk_c{};", col);
    }
    let _ = writeln!(out);

    // ---- the left edge -----------------------------------------------------
    let _ = writeln!(out, "    // Input controller: what enters each row at column 0.");
    for (row, lanes) in fabric.io_inputs.iter().enumerate() {
        for (lane, name) in lanes.iter().enumerate() {
            let value = if *name == fabric.naming.constant_zero {
                "1'b0".to_string()
            } else if *name == fabric.naming.constant_one {
                "1'b1".to_string()
            } else {
                name.clone()
            };
            let _ = writeln!(out, "    assign {} = {};", hseg(row, 0, lane), value);
        }
    }
    let _ = writeln!(out);

    // ---- each CLB ----------------------------------------------------------
    for col in 0..fabric.columns {
        for row in 0..fabric.rows {
            let _ = writeln!(out, "    // CLB({}, {})", col, row);

            // Operation inputs, each resolved to the single selected source.
            let mut input_names = Vec::new();
            for (index, mux) in fabric.input_muxes.iter().enumerate() {
                let code = select(config, col, row, mux.select) as usize;
                let source = mux.sources.get(code).copied().unwrap_or(Source::Const(false));
                let name = format!("in{}_c{}_r{}", index, col, row);
                let _ = writeln!(
                    out,
                    "    wire {} = {};",
                    name,
                    source_expr(fabric, col, row, source)
                );
                input_names.push(name);
            }
            let index_expr = {
                let bits: Vec<String> = input_names.iter().rev().cloned().collect();
                format!("{{{}}}", bits.join(", "))
            };

            // The operation and the carry, straight from the fabric's tables.
            let op_code = select(config, col, row, fabric.op_select);
            let op = fabric
                .operations
                .iter()
                .find(|o| o.code == op_code)
                .or_else(|| fabric.operations.first());
            let name = op.map(|o| o.name.to_uppercase()).unwrap_or_else(|| "AND3".to_string());
            let _ = writeln!(
                out,
                "    assign op_c{}_r{} = TBL_{}[{}];",
                col, row, name, index_expr
            );
            let _ = writeln!(
                out,
                "    assign cy_c{}_r{} = TBL_CARRY[{}];",
                col, row, index_expr
            );

            // The flip-flop: data hard-wired to this cell's own result, enable
            // taken from the lane the fabric nominates, synchronous reset that
            // wins over the enable.
            let enable = source_expr(fabric, col, row, fabric.ff.enable);
            let reset_value = config.clb(col, row).get(fabric.ff.reset_value_field);
            let _ = writeln!(out, "    always_ff @(posedge clk_c{}) begin", col);
            let _ = writeln!(out, "        if (reset) reg_c{}_r{} <= 1'b{};", col, row, reset_value);
            let _ = writeln!(
                out,
                "        else if ({}) reg_c{}_r{} <= op_c{}_r{};",
                enable, col, row, col, row
            );
            let _ = writeln!(out, "    end");

            // Outgoing lanes.
            for mux in &fabric.output_muxes {
                let code = select(config, col, row, mux.select) as usize;
                let source = mux.sources.get(code).copied().unwrap_or(Source::Const(false));
                let target = match mux.drives {
                    BusOut::Horz(lane) => hseg(row, col + 1, lane),
                    BusOut::Vert(lane) => vseg(col, (row + 1) % fabric.rows, lane),
                };
                let _ = writeln!(
                    out,
                    "    assign {} = {};",
                    target,
                    source_expr(fabric, col, row, source)
                );
            }
            let _ = writeln!(out);
        }
    }

    // ---- the clock network -------------------------------------------------
    let _ = writeln!(out, "    // Clock selectors: each taps its column's ring, or couples.");
    let couple_field = fabric.csb_clock.couple_field;
    for col in 0..fabric.columns {
        let coupled = config.csb(col).get(couple_field) != 0;
        if coupled {
            let previous = (col + fabric.columns - 1) % fabric.columns;
            let _ = writeln!(out, "    assign clk_c{} = clk_c{};", col, previous);
        } else {
            let code = {
                let slice = fabric.csb_clock.select;
                let raw = config.csb(col).get(slice.field);
                match slice.bit {
                    Some(bit) => raw >> bit & 1,
                    None => raw,
                }
            } as usize;
            let lane = fabric.csb_clock.ring_lanes.get(code).copied().unwrap_or(0);
            let _ = writeln!(out, "    assign clk_c{} = {};", col, vseg(col, 0, lane));
        }
    }
    let _ = writeln!(out);

    // ---- the right edge ----------------------------------------------------
    let _ = writeln!(out, "    // Output controller: what each row presents at the right edge.");
    for (row, lanes) in fabric.io_outputs.iter().enumerate() {
        for (lane, name) in lanes.iter().enumerate() {
            if let Some(name) = name {
                let _ = writeln!(out, "    assign {} = {};", name, hseg(row, fabric.columns, lane));
            }
        }
    }

    let _ = writeln!(out, "endmodule");
    let _ = writeln!(out, "`default_nettype wire");
    out
}

fn table_literal(table: &[bool]) -> String {
    let mut bits = 0u32;
    for (index, &value) in table.iter().enumerate() {
        if value {
            bits |= 1 << index;
        }
    }
    format!("{}'h{:02x}", table.len(), bits)
}
