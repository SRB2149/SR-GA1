//! The dedicated carry chain.
//!
//! Addition is the one thing on this fabric that is not ordinary logic. Every
//! CLB computes the full-adder carry of its three selected inputs
//! unconditionally and hands it to the cell directly above on a dedicated
//! wire, so a ripple adder is one CLB per bit: set the LUT to `a ^ b ^ c`,
//! take `c` from `carry_in`, and the sum appears on `_op` while the carry
//! climbs the column by itself.
//!
//! Three hard limits come with it, and all three are reported rather than
//! worked around:
//!
//! * **A chain lives in one column and runs upward**, LSB at the bottom, so
//!   it is at most `rows` cells long — four on this fabric.
//! * **Row 0's carry input is tied off**, and the chain does not wrap. An
//!   adder with a real carry-in must take it on a horizontal lane instead,
//!   which the LSB cell can do because the same input mux reaches both.
//! * **The top cell's carry-out goes nowhere by itself.** Carry is on no
//!   output mux, so a carry that is needed outside the chain — a carry-out
//!   port, or the next segment of an adder wider than a column — is taken by
//!   a *tap*: one more cell directly above, whose input `c` reads `carry_in`
//!   and whose LUT copies it to `_op`. A wider adder is several segments, each
//!   taking the previous tap's net as its carry-in on a lane.
//!
//! Nothing here is hard-coded: the LUT value and the mux codes are derived
//! from the fabric's own tables, so a fabric whose carry works differently is
//! detected instead of mis-targeted.

use crate::fabric::{Fabric, Source};
use std::fmt;

/// The name the Yosys arithmetic map gives each adder bit.
pub const CARRY_CELL: &str = "SRGA1_CARRY";

/// The packed cell that taps a chain's carry onto an ordinary net.
pub const CARRY_TAP: &str = "CARRY_TAP";

#[derive(Debug, Clone)]
pub struct CarryError {
    pub message: String,
}

impl fmt::Display for CarryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CarryError {}

/// How to build one bit of a ripple adder on this fabric.
#[derive(Debug, Clone)]
pub struct CarryPlan {
    /// LUT value whose result is the three-input XOR.
    pub lut: u64,
    /// LUT value that copies the carry input, for a tap.
    pub tap_lut: u64,
    /// The library gate computing it, which carry cells are packed as.
    pub gate: String,
    /// Physical input that can read the carry chain, and the select code.
    pub carry_input: usize,
    pub carry_code: u64,
    /// The other two physical inputs, which take the addends.
    pub addend_inputs: Vec<usize>,
    /// Longest chain the fabric allows.
    pub max_length: usize,
    /// Row the least significant cell must occupy.
    pub lsb_row: usize,
    /// Value the chain's first cell receives, which is tied off in hardware.
    pub edge: bool,
}

impl CarryPlan {
    /// Work out how the fabric's carry chain can be used, or say why it
    /// cannot be.
    pub fn derive(fabric: &Fabric) -> Result<CarryPlan, CarryError> {
        let inputs = fabric.input_muxes.len();
        if inputs != 3 {
            return Err(CarryError {
                message: format!(
                    "the carry chain expects a three-input LUT; this fabric has {}",
                    inputs
                ),
            });
        }

        // The carry output must be the full-adder carry, or a chain of these
        // cells does not add. Majority is symmetric, so the table's index
        // order does not matter here.
        let majority: Vec<bool> = (0..8)
            .map(|i| (i & 1) + (i >> 1 & 1) + (i >> 2 & 1) >= 2)
            .collect();
        if fabric.carry_table != majority {
            return Err(CarryError {
                message: "the fabric's carry output is not the full-adder carry of its three \
                          inputs, so a ripple adder cannot be built from it"
                    .to_string(),
            });
        }

        // The sum bit is the three-input XOR, which the LUT can always hold.
        let lut = fabric.lut_value(|v| v[0] ^ v[1] ^ v[2]);
        let library = crate::genlib::CellLibrary::derive(fabric);
        let gate = library
            .cells
            .iter()
            .find(|c| c.arity == 3 && c.impls.iter().any(|i| i.lut == lut))
            .map(|c| c.name.clone())
            .ok_or_else(|| CarryError {
                message: "the cell library has no three-input XOR".to_string(),
            })?;

        // Which input reads the chain.
        let (carry_input, carry_code) = fabric
            .input_muxes
            .iter()
            .enumerate()
            .find_map(|(index, mux)| {
                mux.sources
                    .iter()
                    .position(|s| *s == Source::CarryIn)
                    .map(|code| (index, code as u64))
            })
            .ok_or_else(|| CarryError {
                message: "no operation input can read the carry chain".to_string(),
            })?;

        let addend_inputs: Vec<usize> = (0..inputs).filter(|&i| i != carry_input).collect();

        let tap_lut = fabric.lut_value(|v| v[carry_input]);
        Ok(CarryPlan {
            lut,
            tap_lut,
            gate,
            carry_input,
            carry_code,
            addend_inputs,
            max_length: fabric.rows,
            lsb_row: if fabric.carry.edge { fabric.rows - 1 } else { 0 },
            edge: fabric.carry.edge,
        })
    }

    /// The Yosys techmap file that turns `$alu` into a chain of these cells.
    ///
    /// `alumacc` runs first, so addition, subtraction and comparison all
    /// arrive as `$alu` and one mapping covers them. `X` and `CO` are left as
    /// ordinary logic for ABC to map — only the sum and the chain itself come
    /// from the dedicated hardware.
    pub fn arith_map(&self) -> String {
        format!(
            r#"// Generated by sr-ga1-synth. Maps Yosys' $alu onto the SR-GA1 carry chain.
//
// One {cell} per bit: S = A ^ B ^ CI in the LUT ({op}), and CO on
// the dedicated chain to the cell above. The chain is at most {max} cells
// long and its final carry-out is unreachable, both of which the packer
// checks after mapping.

(* blackbox *)
module {cell} (A, B, CI, S, CO);
    input  A, B, CI;
    output S, CO;
endmodule

module \$alu (A, B, CI, BI, X, Y, CO);
    parameter A_SIGNED = 0;
    parameter B_SIGNED = 0;
    parameter A_WIDTH = 1;
    parameter B_WIDTH = 1;
    parameter Y_WIDTH = 1;

    input  [A_WIDTH-1:0] A;
    input  [B_WIDTH-1:0] B;
    input  CI, BI;
    output [Y_WIDTH-1:0] X, Y, CO;

    wire [Y_WIDTH-1:0] a, b;

    // Sign- or zero-extend both operands to the result width.
    \$pos #(
        .A_SIGNED(A_SIGNED),
        .A_WIDTH(A_WIDTH),
        .Y_WIDTH(Y_WIDTH)
    ) extend_a (.A(A), .Y(a));

    \$pos #(
        .A_SIGNED(B_SIGNED),
        .A_WIDTH(B_WIDTH),
        .Y_WIDTH(Y_WIDTH)
    ) extend_b (.A(B), .Y(b));

    // BI asks for a subtraction, which is addition of the inverted operand.
    wire [Y_WIDTH-1:0] bb = BI ? ~b : b;

    wire [Y_WIDTH:0] chain;
    assign chain[0] = CI;

    genvar i;
    generate
        for (i = 0; i < Y_WIDTH; i = i + 1) begin : bits
            {cell} adder (
                .A(a[i]),
                .B(bb[i]),
                .CI(chain[i]),
                .S(Y[i]),
                .CO(chain[i + 1])
            );
            assign CO[i] = chain[i + 1];
            assign X[i] = a[i] ^ bb[i];
        end
    endgenerate
endmodule
"#,
            cell = CARRY_CELL,
            op = self.gate,
            max = self.max_length
        )
    }
}
