//! Bitstream emitter and decoder.
//!
//! `fabric.toml`'s `[bitstream]` section declares the physical chain in
//! data-flow order: the first cell listed is nearest `shift_data_in`, and
//! within a cell the fields shift in declaration order with each field's LSB
//! at the lower chain index. First-shifted bits travel deepest, so the
//! transmission order emitted here is the exact reverse — the last cell's
//! highest bit first, the first cell's bit 0 last.
//!
//! When the fabric declares a start bit it leads the transmission: shifted in
//! first, it has passed out of the far end of the chain by the time the
//! configuration is in place. Export prepends it; import checks and strips it.
//!
//! This must stay byte-identical to the visual programmer's export; the
//! golden tests in `tests/` are what keeps the two honest.

use crate::design::{BlockId, Design};
use crate::fabric::{ChainCells, ChainOrder, Fabric, Field};
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

/// Every cell in shift-chain data-flow order.
fn chain_cells(fabric: &Fabric) -> Vec<BlockId> {
    let mut cells = Vec::with_capacity(fabric.clb_count() + fabric.columns + 1);
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

fn fields_of(fabric: &Fabric, cell: BlockId) -> &[Field] {
    match cell {
        BlockId::Clb { .. } => &fabric.clb_fields,
        BlockId::Csb { .. } => &fabric.csb_fields,
        BlockId::Io => &fabric.io_fields,
    }
}

/// The serial order of the configuration bits (after any start bit) as
/// (cell, field, bit) triples. Both codec directions walk this one list, so
/// they cannot disagree.
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

/// A complete transmission: the start bit, if any, then the configuration.
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

/// Rebuild a configuration from a complete transmission — the other half of
/// the round-trip test. Names are not part of a bitstream.
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
                "bitstream is {} bits; this fabric needs exactly {}{}",
                bits.len(),
                expected,
                start
            ),
        });
    }
    let config = match fabric.start_bit {
        Some(start) if bits[0] != start => {
            return Err(BitstreamError {
                message: format!("bitstream must begin with the start bit {}", u8::from(start)),
            })
        }
        Some(_) => &bits[1..],
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

/// Render the bitstream file: `# name` / `# timestamp` header lines unless
/// `raw`, then every bit on one line.
pub fn format_text(
    fabric: &Fabric,
    design: &Design,
    design_name: &str,
    timestamp: &str,
    raw: bool,
) -> String {
    let bits = export_bits(fabric, design);
    let mut out = String::with_capacity(bits.len() + 64);
    if !raw {
        out.push_str(&format!("# {}\n# {}\n", design_name, timestamp));
    }
    out.extend(bits.iter().map(|&b| if b { '1' } else { '0' }));
    out.push('\n');
    out
}

/// Parse bitstream text: `#` comments and blank lines are ignored, every
/// other character must be 0 or 1.
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
                        message: format!(
                            "line {}: unexpected character '{}' in bitstream",
                            lineno + 1,
                            c
                        ),
                    })
                }
            }
        }
    }
    Ok(bits)
}
