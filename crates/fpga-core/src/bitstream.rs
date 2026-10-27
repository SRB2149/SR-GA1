//! Bitstream codec: exports and imports the configuration scan chain.
//!
//! The fabric's `[bitstream]` section declares the physical chain in
//! data-flow order (the first cell is nearest `shift_data_in`; within a cell,
//! field-list order with each field's LSB at the lower chain index). Because
//! first-shifted bits travel deepest, the *transmission* order emitted here
//! is the exact reverse: the last cell's highest bit first, the first cell's
//! bit 0 last.
//!
//! When the fabric declares a start bit it leads the transmission: it is
//! shifted in first, so by the time the configuration is in place it has
//! passed out of the far end of the chain. Export prepends it and import
//! checks and strips it, so a stream is `stream_bits()` long.

use crate::config::{BlockId, Design};
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

/// All cells in shift-chain data-flow order.
fn chain_cells(fabric: &Fabric) -> Vec<BlockId> {
    let mut cells = Vec::with_capacity(fabric.columns * fabric.rows + fabric.columns + 1);
    for group in &fabric.chain {
        match (group.cells, group.order) {
            (ChainCells::Clb, ChainOrder::ColumnAscending) => {
                for col in 0..fabric.columns {
                    for row in 0..fabric.rows {
                        cells.push(BlockId::Clb { col, row });
                    }
                }
            }
            (ChainCells::Clb, _) => {
                for row in 0..fabric.rows {
                    for col in 0..fabric.columns {
                        cells.push(BlockId::Clb { col, row });
                    }
                }
            }
            (ChainCells::Io, _) => cells.push(BlockId::Io),
            (ChainCells::Csb, _) => {
                for col in 0..fabric.columns {
                    cells.push(BlockId::Csb { col });
                }
            }
        }
    }
    cells
}

fn fields_of(fabric: &Fabric, cell: BlockId) -> &[crate::fabric::Field] {
    match cell {
        BlockId::Clb { .. } => &fabric.clb_fields,
        BlockId::Csb { .. } => &fabric.csb_fields,
        BlockId::Io => &fabric.io_fields,
    }
}

/// The serial transmission order of the configuration bits (after any start
/// bit) as (cell, field index, bit) triples — what the config bit inspector
/// shows and both codec directions share.
fn transmission_order(fabric: &Fabric) -> Vec<(BlockId, usize, usize)> {
    let mut order = Vec::with_capacity(fabric.total_bits());
    for cell in chain_cells(fabric).iter().rev() {
        for (fi, field) in fields_of(fabric, *cell).iter().enumerate().rev() {
            for bit in (0..field.width).rev() {
                order.push((*cell, fi, bit));
            }
        }
    }
    order
}

/// Export a complete transmission: the start bit, if the fabric has one, then
/// the configuration bits.
pub fn export_bits(fabric: &Fabric, design: &Design) -> Vec<bool> {
    fabric
        .start_bit
        .into_iter()
        .chain(
            transmission_order(fabric)
                .iter()
                .map(|(cell, fi, bit)| (design.block(*cell).get(*fi) >> bit) & 1 != 0),
        )
        .collect()
}

/// Reconstruct a configuration from a complete transmission. Pinned names
/// and stimulus are not part of a bitstream; the result carries config only.
pub fn import_bits(fabric: &Fabric, bits: &[bool]) -> Result<Design, BitstreamError> {
    let expected = fabric.stream_bits();
    if bits.len() != expected {
        let start = if fabric.start_bit.is_some() {
            format!(" (1 start bit + {} configuration bits)", fabric.total_bits())
        } else {
            String::new()
        };
        return Err(BitstreamError {
            message: format!(
                "bitstream is {} bits, this fabric needs exactly {}{}",
                bits.len(),
                expected,
                start
            ),
        });
    }
    let config = match fabric.start_bit {
        Some(start) => {
            if bits[0] != start {
                return Err(BitstreamError {
                    message: format!(
                        "bitstream must begin with the start bit {}, found {}",
                        start as u8, bits[0] as u8
                    ),
                });
            }
            &bits[1..]
        }
        None => bits,
    };
    let mut design = Design::new(fabric);
    for ((cell, fi, bit), &value) in transmission_order(fabric).iter().zip(config) {
        if !value {
            continue;
        }
        let width = fields_of(fabric, *cell)[*fi].width;
        let v = design.block(*cell).get(*fi) | (1 << bit);
        design.block_mut(*cell).set(*fi, width, v);
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

/// The bit indices (in the complete transmission, start bit included) owned
/// by one block — for the config bit inspector. Returns
/// `(stream_index, field_index, bit)` triples.
pub fn block_bits(fabric: &Fabric, block: BlockId) -> Vec<(usize, usize, usize)> {
    let offset = fabric.start_bit.is_some() as usize;
    transmission_order(fabric)
        .iter()
        .enumerate()
        .filter_map(|(i, (cell, fi, bit))| (*cell == block).then_some((i + offset, *fi, *bit)))
        .collect()
}
