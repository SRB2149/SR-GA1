//! The constraints file.
//!
//! It carries pin locks, DDIO declarations, instance placement, clock
//! assignment, and the pads that are physically wired output-back-to-input on
//! the board, which the router may then use as ordinary routing resources.
//! Board wiring is not a property of the chip, which is why it lives here
//! rather than in `fabric.toml` — and it is rarely needed, because the IO
//! controller can read `output_0..3` back onto the left edge on-chip without
//! any wiring at all. A pool adds to that, it does not replace it.
//!
//! Everything here is validated against the fabric when the file is
//! read, and against the design once it has been elaborated — a constraint that
//! names a signal the design does not have is an error, not a shrug. A
//! constraint that makes fitting impossible is reported as such rather than
//! surfacing later as a mysterious routing failure.

use crate::fabric::Fabric;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use toml::Spanned;

#[derive(Debug, Clone)]
pub struct ConstraintError {
    pub path: PathBuf,
    pub line: Option<usize>,
    pub message: String,
}

impl fmt::Display for ConstraintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "{}:{}: {}", self.path.display(), line, self.message),
            None => write!(f, "{}: {}", self.path.display(), self.message),
        }
    }
}

impl std::error::Error for ConstraintError {}

fn line_at(src: &str, offset: usize) -> usize {
    src[..offset.min(src.len())].bytes().filter(|&b| b == b'\n').count() + 1
}

/// A chip pad, resolved to its side and index in the fabric's pad list for
/// that side. Which lane it reaches is IO controller configuration, chosen by
/// the router.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pad {
    pub name: String,
    /// True for an input-side pad (`io_inputs`), false for an output-side one.
    pub input: bool,
    pub index: usize,
}

/// Pads available for loop-around wiring.
///
/// The tool pairs them itself and reports the wiring that has to exist, so an
/// output may feed several inputs (one pin can fan out) while each input has
/// exactly one driver.
#[derive(Debug, Clone, Default)]
pub struct LoopbackPool {
    pub outputs: Vec<Pad>,
    pub inputs: Vec<Pad>,
}

impl LoopbackPool {
    pub fn is_empty(&self) -> bool {
        self.outputs.is_empty() || self.inputs.is_empty()
    }
    /// Input pads the board drives, which a design input therefore cannot use.
    pub fn reserved_inputs(&self) -> Vec<&str> {
        self.inputs.iter().map(|p| p.name.as_str()).collect()
    }
}

/// One DDIO pad bound to three design nets.
///
/// DDIO is never inferred from SystemVerilog: the pad drives `out` when `dir` is
/// high, and `in` then reads that output back, which is a board-level decision.
/// An unconstrained DDIO pad has its direction tied to 0, so it stays an input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdioBinding {
    /// Index into the fabric's DDIO list.
    pub pin: usize,
    /// Design net that reads the pad.
    pub input: String,
    /// Design net that drives it.
    pub output: String,
    /// Design net that selects the direction.
    pub dir: String,
    /// The three pads, resolved.
    pub in_pad: Pad,
    pub out_pad: Pad,
    pub dir_pad: Pad,
}

/// A cell pinned to a position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementLock {
    /// Cell or signal name, as it appears in the design.
    pub name: String,
    pub col: usize,
    pub row: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Constraints {
    pub loopback: LoopbackPool,
    /// Design signal name -> chip pad name.
    pub pins: BTreeMap<String, Pad>,
    pub ddio: Vec<DdioBinding>,
    pub placement: Vec<PlacementLock>,
    /// Clock net name -> column whose CSB sources it.
    pub clocks: BTreeMap<String, usize>,
}

impl Constraints {
    /// Pads a DDIO binding claims, which are otherwise excluded from use.
    pub fn ddio_pads(&self) -> Vec<&str> {
        self.ddio
            .iter()
            .flat_map(|d| [d.in_pad.name.as_str(), d.out_pad.name.as_str(), d.dir_pad.name.as_str()])
            .collect()
    }

    /// The pad a design signal is locked to, if any.
    pub fn pin_for(&self, signal: &str) -> Option<&Pad> {
        self.pins.get(signal)
    }

    /// Whether anything at all was constrained, for the report.
    pub fn is_empty(&self) -> bool {
        self.loopback.is_empty()
            && self.pins.is_empty()
            && self.ddio.is_empty()
            && self.placement.is_empty()
            && self.clocks.is_empty()
    }
}

impl Constraints {
    pub fn load_file(path: &Path, fabric: &Fabric) -> Result<Constraints, ConstraintError> {
        let src = std::fs::read_to_string(path).map_err(|e| ConstraintError {
            path: path.to_path_buf(),
            line: None,
            message: format!("cannot read the constraints file: {}", e),
        })?;
        Constraints::load_str(&src, path, fabric)
    }

    pub fn load_str(
        src: &str,
        path: &Path,
        fabric: &Fabric,
    ) -> Result<Constraints, ConstraintError> {
        let raw: RawConstraints = toml::from_str(src).map_err(|e| ConstraintError {
            path: path.to_path_buf(),
            line: e.span().map(|s| line_at(src, s.start)),
            message: e.message().to_string(),
        })?;

        let fail = |line: Option<usize>, message: String| ConstraintError {
            path: path.to_path_buf(),
            line,
            message,
        };

        let mut loopback = LoopbackPool::default();
        if let Some(raw_pool) = &raw.loopback {
            let allow_ddio = raw_pool.allow_ddio;
            let ddio: Vec<&str> = fabric.ddio_pad_names();

            for entry in &raw_pool.outputs {
                let name = entry.get_ref();
                let line = Some(line_at(src, entry.span().start));
                let Some(pad) = output_pad(fabric, name) else {
                    return Err(fail(
                        line,
                        format!(
                            "\"{}\" is not a chip output on this fabric. The outputs are: {}",
                            name,
                            list_outputs(fabric).join(", ")
                        ),
                    ));
                };
                if ddio.contains(&name.as_str()) && !allow_ddio {
                    return Err(fail(
                        line,
                        format!(
                            "\"{}\" is a DDIO pad, whose direction is steered by ddio_dir; using \
                             it for loop-around wiring needs allow_ddio = true and a matching \
                             direction constraint.",
                            name
                        ),
                    ));
                }
                if loopback.outputs.iter().any(|p| p.name == *name) {
                    return Err(fail(line, format!("output \"{}\" is listed twice", name)));
                }
                loopback.outputs.push(pad);
            }

            for entry in &raw_pool.inputs {
                let name = entry.get_ref();
                let line = Some(line_at(src, entry.span().start));
                let Some(pad) = input_pad(fabric, name) else {
                    return Err(fail(
                        line,
                        format!(
                            "\"{}\" is not a chip input on this fabric. The inputs are: {}",
                            name,
                            list_inputs(fabric).join(", ")
                        ),
                    ));
                };
                if ddio.contains(&name.as_str()) && !allow_ddio {
                    return Err(fail(
                        line,
                        format!(
                            "\"{}\" is a DDIO pad; using it for loop-around wiring needs \
                             allow_ddio = true and a matching direction constraint.",
                            name
                        ),
                    ));
                }
                if loopback.inputs.iter().any(|p| p.name == *name) {
                    return Err(fail(line, format!("input \"{}\" is listed twice", name)));
                }
                loopback.inputs.push(pad);
            }

            // A pool with only one end is a typo, not a constraint.
            if loopback.outputs.is_empty() != loopback.inputs.is_empty() {
                return Err(fail(
                    find_section(src, "loopback"),
                    "a loop-around pool needs both outputs and inputs; one end on its own \
                     cannot form a loop"
                        .to_string(),
                ));
            }
        }

        // ---- pin locks -----------------------------------------------------
        // A pad may be named for either direction; which one it is decides
        // whether the signal is expected to be an input or an output, and that
        // is cross-checked against the design later.
        let mut pins = BTreeMap::new();
        for (signal, entry) in &raw.pins {
            let pad_name = entry.get_ref();
            let line = Some(line_at(src, entry.span().start));
            let pad = input_pad(fabric, pad_name).or_else(|| output_pad(fabric, pad_name));
            let Some(pad) = pad else {
                return Err(fail(
                    line,
                    format!(
                        "\"{}\" is not a chip pad on this fabric. Inputs: {}. Outputs: {}.",
                        pad_name,
                        list_inputs(fabric).join(", "),
                        list_outputs(fabric).join(", ")
                    ),
                ));
            };
            if let Some(other) = pins.iter().find(|(_, p): &(&String, &Pad)| p.name == pad.name) {
                return Err(fail(
                    line,
                    format!(
                        "pad \"{}\" is locked to both \"{}\" and \"{}\"; one pad carries one signal",
                        pad.name, other.0, signal
                    ),
                ));
            }
            if loopback.inputs.iter().any(|p| p.name == pad.name) {
                return Err(fail(
                    line,
                    format!(
                        "pad \"{}\" is in the loop-around input pool, so it is driven by a board \
                         wire and cannot also carry design signal \"{}\"",
                        pad.name, signal
                    ),
                ));
            }
            pins.insert(signal.clone(), pad);
        }

        // ---- DDIO ----------------------------------------------------------
        let mut ddio = Vec::new();
        for entry in &raw.ddio {
            let decl = entry.get_ref();
            let line = Some(line_at(src, entry.span().start));
            let index = *decl.pin.get_ref();
            let Some(spec) = fabric.ddio.get(index) else {
                return Err(fail(
                    Some(line_at(src, decl.pin.span().start)),
                    format!(
                        "this fabric has {} DDIO pad(s), numbered 0..{}; there is no pad {}",
                        fabric.ddio.len(),
                        fabric.ddio.len().saturating_sub(1),
                        index
                    ),
                ));
            };
            if ddio.iter().any(|d: &DdioBinding| d.pin == index) {
                return Err(fail(line, format!("DDIO pad {} is declared twice", index)));
            }
            let pad = |name: &String, input: bool, index: usize| Pad { name: name.clone(), input, index };
            ddio.push(DdioBinding {
                pin: index,
                input: decl.input.clone(),
                output: decl.out.clone(),
                dir: decl.dir.clone(),
                in_pad: pad(&fabric.io_inputs[spec.input], true, spec.input),
                out_pad: pad(&fabric.io_outputs[spec.output], false, spec.output),
                dir_pad: pad(&fabric.io_outputs[spec.dir], false, spec.dir),
            });
        }

        // ---- instance placement --------------------------------------------
        let mut placement = Vec::new();
        for (name, entry) in &raw.placement {
            let text = entry.get_ref();
            let line = Some(line_at(src, entry.span().start));
            let Some((col, row)) = parse_position(text) else {
                return Err(fail(
                    line,
                    format!(
                        "\"{}\" is not a position; write it as \"col,row\", for example \"4,0\"",
                        text
                    ),
                ));
            };
            if col >= fabric.columns || row >= fabric.rows {
                return Err(fail(
                    line,
                    format!(
                        "CLB ({}, {}) is outside the {}x{} grid",
                        col, row, fabric.columns, fabric.rows
                    ),
                ));
            }
            if let Some(other) = placement.iter().find(|l: &&PlacementLock| l.col == col && l.row == row)
            {
                return Err(fail(
                    line,
                    format!(
                        "CLB ({}, {}) is claimed by both \"{}\" and \"{}\"; one CLB holds one cell",
                        col, row, other.name, name
                    ),
                ));
            }
            placement.push(PlacementLock { name: name.clone(), col, row });
        }

        // ---- clock assignment ----------------------------------------------
        let mut clocks = BTreeMap::new();
        for (net, entry) in &raw.clocks {
            let column = *entry.get_ref();
            let line = Some(line_at(src, entry.span().start));
            if column >= fabric.columns {
                return Err(fail(
                    line,
                    format!(
                        "column {} does not exist; this fabric has columns 0..{}",
                        column,
                        fabric.columns - 1
                    ),
                ));
            }
            if let Some(other) = clocks.iter().find(|(_, c): &(&String, &usize)| **c == column) {
                return Err(fail(
                    line,
                    format!(
                        "column {} is named as the source for both \"{}\" and \"{}\"; one CSB \
                         sources one domain",
                        column, other.0, net
                    ),
                ));
            }
            clocks.insert(net.clone(), column);
        }

        Ok(Constraints { loopback, pins, ddio, placement, clocks })
    }
}

/// `"col,row"`.
fn parse_position(text: &str) -> Option<(usize, usize)> {
    let (col, row) = text.split_once(',')?;
    Some((col.trim().parse().ok()?, row.trim().parse().ok()?))
}

/// Rough line of a `[section]` header, for errors about a section as a whole.
fn find_section(src: &str, name: &str) -> Option<usize> {
    let needle = format!("[{}", name);
    src.lines()
        .position(|line| line.trim_start().starts_with(&needle))
        .map(|index| index + 1)
}

pub fn output_pad(fabric: &Fabric, name: &str) -> Option<Pad> {
    fabric
        .output_pad(name)
        .map(|index| Pad { name: name.to_string(), input: false, index })
}

pub fn input_pad(fabric: &Fabric, name: &str) -> Option<Pad> {
    fabric
        .input_pad(name)
        .map(|index| Pad { name: name.to_string(), input: true, index })
}

fn list_outputs(fabric: &Fabric) -> Vec<String> {
    fabric.io_outputs.clone()
}

fn list_inputs(fabric: &Fabric) -> Vec<String> {
    fabric.io_inputs.clone()
}

// ---------------------------------------------------------------------------
// On-disk schema

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConstraints {
    #[serde(default)]
    loopback: Option<RawLoopback>,
    /// Design signal -> pad name.
    #[serde(default)]
    pins: BTreeMap<String, Spanned<String>>,
    #[serde(default)]
    ddio: Vec<Spanned<RawDdio>>,
    /// Cell or signal name -> "col,row".
    #[serde(default)]
    placement: BTreeMap<String, Spanned<String>>,
    /// Clock net -> sourcing column.
    #[serde(default)]
    clocks: BTreeMap<String, Spanned<usize>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDdio {
    /// Index of the DDIO pad in the fabric's list.
    pin: Spanned<usize>,
    #[serde(rename = "in")]
    input: String,
    out: String,
    dir: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLoopback {
    #[serde(default)]
    outputs: Vec<Spanned<String>>,
    #[serde(default)]
    inputs: Vec<Spanned<String>>,
    #[serde(default)]
    allow_ddio: bool,
}
