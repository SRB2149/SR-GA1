//! Bitstream codec: exports and imports the configuration scan chain.
//!
//! The fabric's `[bitstream]` section declares the physical chain in
//! data-flow order (the first cell is nearest `shift_data_in`; within a cell,
//! field-list order with each field's LSB at the lower chain index). Because
//! first-shifted bits travel deepest, the *transmission* order emitted here
//! is the exact reverse: the last cell's highest bit first, the first cell's
//! bit 0 last.

use crate::config::Design;
use crate::fabric::{ChainCells, ChainOrder, Fabric};
use std::fmt;

#[derive(Debug, Clone)]
pub struct BitstreamError {
    pub message: String,
}

impl fmt::Display for BitstreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for BitstreamError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cell {
    Clb { col: usize, row: usize },
    Csb { col: usize },
}

/// All cells in shift-chain data-flow order.
fn chain_cells(fabric: &Fabric) -> Vec<Cell> {
    let mut cells = Vec::with_capacity(fabric.columns * fabric.rows + fabric.columns);
    for group in &fabric.chain {
        match group.cells {
            ChainCells::Clb => match group.order {
                ChainOrder::RowMajor => {
                    for row in 0..fabric.rows {
                        for col in 0..fabric.columns {
                            cells.push(Cell::Clb { col, row });
                        }
                    }
                }
                ChainOrder::ColumnAscending => {
                    for col in 0..fabric.columns {
                        for row in 0..fabric.rows {
                            cells.push(Cell::Clb { col, row });
                        }
                    }
                }
            },
            ChainCells::Csb => {
                for col in 0..fabric.columns {
                    cells.push(Cell::Csb { col });
                }
            }
        }
    }
    cells
}

/// The exact serial transmission order as (cell, field index, bit) triples —
/// what the config bit inspector shows and both codec directions share.
fn transmission_order(fabric: &Fabric) -> Vec<(Cell, usize, usize)> {
    let mut order = Vec::with_capacity(fabric.total_bits());
    for cell in chain_cells(fabric).iter().rev() {
        let fields = match cell {
            Cell::Clb { .. } => &fabric.clb_fields,
            Cell::Csb { .. } => &fabric.csb_fields,
        };
        for (fi, field) in fields.iter().enumerate().rev() {
            for bit in (0..field.width).rev() {
                order.push((*cell, fi, bit));
            }
        }
    }
    order
}

/// Export the design's configuration bits in transmission order.
pub fn export_bits(fabric: &Fabric, design: &Design) -> Vec<bool> {
    transmission_order(fabric)
        .iter()
        .map(|(cell, fi, bit)| {
            let value = match cell {
                Cell::Clb { col, row } => design.clb(*col, *row).get(*fi),
                Cell::Csb { col } => design.csb(*col).get(*fi),
            };
            (value >> bit) & 1 != 0
        })
        .collect()
}

/// Reconstruct a configuration from bits in transmission order. Pinned names
/// and stimulus are not part of a bitstream; the result carries config only.
pub fn import_bits(fabric: &Fabric, bits: &[bool]) -> Result<Design, BitstreamError> {
    let expected = fabric.total_bits();
    if bits.len() != expected {
        return Err(BitstreamError {
            message: format!("bitstream is {} bits, this fabric needs exactly {}", bits.len(), expected),
        });
    }
    let mut design = Design::new(fabric);
    for ((cell, fi, bit), &value) in transmission_order(fabric).iter().zip(bits) {
        if !value {
            continue;
        }
        match cell {
            Cell::Clb { col, row } => {
                let width = fabric.clb_fields[*fi].width;
                let v = design.clb(*col, *row).get(*fi) | (1 << bit);
                design.clb_mut(*col, *row).set(*fi, width, v);
            }
            Cell::Csb { col } => {
                let width = fabric.csb_fields[*fi].width;
                let v = design.csb(*col).get(*fi) | (1 << bit);
                design.csb_mut(*col).set(*fi, width, v);
            }
        }
    }
    Ok(design)
}

/// Render the export as plain text: optional `# name` / `# timestamp` header
/// lines, then the bits, wrapped at `wrap` bits per line (`None` = one line).
pub fn format_text(
    fabric: &Fabric,
    design: &Design,
    design_name: &str,
    timestamp: &str,
    raw: bool,
    wrap: Option<usize>,
) -> String {
    let bits = export_bits(fabric, design);
    let mut out = String::new();
    if !raw {
        out.push_str(&format!("# {}\n# {}\n", design_name, timestamp));
    }
    let digits: String = bits.iter().map(|&b| if b { '1' } else { '0' }).collect();
    match wrap {
        Some(n) if n > 0 => {
            for chunk in digits.as_bytes().chunks(n) {
                out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
                out.push('\n');
            }
        }
        _ => {
            out.push_str(&digits);
            out.push('\n');
        }
    }
    out
}

/// Parse bitstream text: `#` comment lines and blank lines are ignored, all
/// other characters must be 0 or 1 (whitespace allowed anywhere).
pub fn parse_text(src: &str) -> Result<Vec<bool>, BitstreamError> {
    let mut bits = Vec::new();
    for (lineno, line) in src.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        for ch in line.chars() {
            match ch {
                '0' => bits.push(false),
                '1' => bits.push(true),
                c if c.is_whitespace() => {}
                c => {
                    return Err(BitstreamError {
                        message: format!("line {}: unexpected character '{}' in bitstream", lineno + 1, c),
                    })
                }
            }
        }
    }
    Ok(bits)
}

/// The bit indices (in transmission order) owned by one block — for the
/// config bit inspector. Returns `(stream_index, field_index, bit)` triples.
pub fn block_bits(fabric: &Fabric, block: crate::config::BlockId) -> Vec<(usize, usize, usize)> {
    transmission_order(fabric)
        .iter()
        .enumerate()
        .filter_map(|(i, (cell, fi, bit))| {
            let matches = match (cell, block) {
                (Cell::Clb { col, row }, crate::config::BlockId::Clb { col: c, row: r }) => *col == c && *row == r,
                (Cell::Csb { col }, crate::config::BlockId::Csb { col: c }) => *col == c,
                _ => false,
            };
            matches.then_some((i, *fi, *bit))
        })
        .collect()
}
