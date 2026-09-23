//! The generated equivalence testbench.
//!
//! Drives the original design and the fabric model from the same vectors and
//! compares them after every clock edge. Vectors come from an LFSR rather than
//! `$random`, so a failure is reproducible and the same run always tests the
//! same thing.
//!
//! Ports are handled whole, not bit by bit. The packed design names every bit
//! separately (`a[0]`, `a[1]`, ...) because each one lands on its own pad, but
//! the reference module declares `a` as a vector and has to be connected as
//! one — wiring only the first bit compiles cleanly and quietly compares
//! nonsense.
//!
//! The comparison happens a moment *after* the rising edge rather than on it.
//! The fabric's column clocks are derived combinationally from a chip input, so
//! both sides see the same edge, but sampling exactly on it would race the
//! non-blocking updates.

use crate::designjson::LoopbackLink;
use crate::fabric::Fabric;
use crate::flow::Outcome;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// One port of the design, and where each of its bits sits on the chip.
#[derive(Debug, Clone)]
pub struct PortSignal {
    /// Port name as the design declares it.
    pub port: String,
    /// Declared width, from the highest bit index in use.
    pub width: usize,
    /// `(bit index, pad name)`, for the bits that reached a pad.
    pub bits: Vec<(usize, String)>,
}

impl PortSignal {
    fn declaration(&self, prefix: &str) -> String {
        if self.width == 1 {
            format!("    logic {}{};", prefix, self.port)
        } else {
            format!("    logic [{}:0] {}{};", self.width - 1, prefix, self.port)
        }
    }
    /// How to refer to one bit of it.
    fn bit(&self, prefix: &str, index: usize) -> String {
        if self.width == 1 {
            format!("{}{}", prefix, self.port)
        } else {
            format!("{}{}[{}]", prefix, self.port, index)
        }
    }
}

/// How the design's ports map onto chip pads.
pub struct PortMap {
    pub inputs: Vec<PortSignal>,
    pub outputs: Vec<PortSignal>,
    /// Clock ports and the pads they enter on. Every domain is driven from the
    /// same testbench clock: both the reference and the model see identical
    /// waveforms, so they must agree regardless of rate, and comparing them on
    /// one common edge keeps the sampling unambiguous. It does mean the check
    /// verifies each domain's logic rather than cross-domain timing, which
    /// vector comparison cannot establish in any case.
    pub clocks: Vec<(String, String)>,
    pub reset: Option<String>,
}

/// Gather the bits of each port back into a whole port.
fn group(bits: &[(&str, usize, String)]) -> Vec<PortSignal> {
    let mut by_port: BTreeMap<&str, Vec<(usize, String)>> = BTreeMap::new();
    for (port, index, pad) in bits {
        by_port.entry(port).or_default().push((*index, pad.clone()));
    }
    by_port
        .into_iter()
        .map(|(port, mut bits)| {
            bits.sort();
            let width = bits.iter().map(|(index, _)| index + 1).max().unwrap_or(1);
            PortSignal { port: port.to_string(), width, bits }
        })
        .collect()
}

impl PortMap {
    pub fn of(outcome: &Outcome) -> PortMap {
        let packed = &outcome.packed;
        let mut input_bits: Vec<(&str, usize, String)> = Vec::new();
        let mut clocks: Vec<(String, String)> = Vec::new();
        let mut reset = None;
        for port in &packed.inputs {
            if packed.reset == Some(port.net) {
                reset = Some(port.port.clone());
                continue;
            }
            let Some(pad) = outcome.placement.input_pins.get(&port.net) else { continue };
            if packed.clocks.contains(&port.net) {
                if !clocks.iter().any(|(p, _)| *p == port.port) {
                    clocks.push((port.port.clone(), pad.clone()));
                }
            } else {
                input_bits.push((port.port.as_str(), port.index, pad.clone()));
            }
        }
        let mut output_bits: Vec<(&str, usize, String)> = Vec::new();
        for port in &packed.outputs {
            if let Some(pad) = outcome.placement.output_pins.get(&port.net) {
                output_bits.push((port.port.as_str(), port.index, pad.clone()));
            }
        }
        PortMap {
            inputs: group(&input_bits),
            outputs: group(&output_bits),
            clocks,
            reset,
        }
    }

    /// Signals the check compares, for the report.
    pub fn compared(&self) -> Vec<String> {
        self.outputs
            .iter()
            .flat_map(|signal| {
                signal
                    .bits
                    .iter()
                    .map(move |(index, pad)| format!("{} on {}", signal.bit("", *index), pad))
            })
            .collect()
    }

    /// Signals the check drives, for the report.
    pub fn driven(&self) -> Vec<String> {
        self.inputs
            .iter()
            .flat_map(|signal| {
                signal
                    .bits
                    .iter()
                    .map(move |(index, pad)| format!("{} on {}", signal.bit("", *index), pad))
            })
            .collect()
    }
}

/// Assignable chip input pads, excluding the reserved constants.
fn pads(fabric: &Fabric) -> Vec<String> {
    fabric
        .io_inputs
        .iter()
        .flatten()
        .filter(|n| **n != fabric.naming.constant_zero && **n != fabric.naming.constant_one)
        .cloned()
        .collect()
}

pub fn testbench(
    outcome: &Outcome,
    map: &PortMap,
    model: &str,
    cycles: usize,
    loopbacks: &[LoopbackLink],
) -> String {
    let fabric = &outcome.fabric;
    let top = &outcome.packed.top;
    let input_pads = pads(fabric);
    let output_pads: Vec<String> =
        fabric.io_outputs.iter().flatten().flatten().cloned().collect();

    let mut o = String::new();
    let w = &mut o;
    let _ = writeln!(w, "// Generated by sr-ga1-synth: equivalence check.");
    let _ = writeln!(w, "//");
    let _ = writeln!(w, "// The design as written, and the same design as the bitstream configures");
    let _ = writeln!(w, "// the fabric, driven identically and compared every cycle.");
    let _ = writeln!(w, "module equiv_tb;");
    let _ = writeln!(w, "    logic clk = 1'b0;");
    let _ = writeln!(w, "    logic reset = 1'b1;");
    let _ = writeln!(w, "    int unsigned cycle = 0;");
    let _ = writeln!(w, "    int unsigned mismatches = 0;");
    let _ = writeln!(w, "    logic [31:0] lfsr = 32'h1234_5678;");
    let _ = writeln!(w);

    for signal in &map.inputs {
        let _ = writeln!(w, "{}", signal.declaration(""));
    }
    for signal in &map.outputs {
        let _ = writeln!(w, "{}", signal.declaration("ref_"));
    }
    let _ = writeln!(w);
    for pad in &input_pads {
        let _ = writeln!(w, "    logic pad_{};", pad);
    }
    for pad in &output_pads {
        let _ = writeln!(w, "    logic out_{};", pad);
    }
    let _ = writeln!(w);

    // ---- the reference design ---------------------------------------------
    let _ = writeln!(w, "    {} reference (", top);
    let mut refs: Vec<String> = Vec::new();
    for (clock_port, _) in &map.clocks {
        refs.push(format!(".{}(clk)", clock_port));
    }
    if let Some(reset_port) = &map.reset {
        refs.push(format!(".{}(reset)", reset_port));
    }
    for signal in &map.inputs {
        refs.push(format!(".{}({})", signal.port, signal.port));
    }
    for signal in &map.outputs {
        refs.push(format!(".{}(ref_{})", signal.port, signal.port));
    }
    let _ = writeln!(w, "        {}", refs.join(",\n        "));
    let _ = writeln!(w, "    );");
    let _ = writeln!(w);

    // ---- the configured fabric -------------------------------------------
    let _ = writeln!(w, "    {} configured (", model);
    let mut ports = vec![".reset(reset)".to_string()];
    for pad in &input_pads {
        ports.push(format!(".{}(pad_{})", pad, pad));
    }
    for pad in &output_pads {
        ports.push(format!(".{}(out_{})", pad, pad));
    }
    let _ = writeln!(w, "        {}", ports.join(",\n        "));
    let _ = writeln!(w, "    );");
    let _ = writeln!(w);

    // ---- driving the pads -------------------------------------------------
    let mut driven: Vec<String> = Vec::new();
    for (_, pad) in &map.clocks {
        let _ = writeln!(w, "    assign pad_{} = clk;", pad);
        driven.push(pad.clone());
    }
    for signal in &map.inputs {
        for (index, pad) in &signal.bits {
            let _ = writeln!(w, "    assign pad_{} = {};", pad, signal.bit("", *index));
            driven.push(pad.clone());
        }
    }
    if !loopbacks.is_empty() {
        let _ = writeln!(w);
        let _ = writeln!(w, "    // Loop-around board wiring, which the configuration depends on.");
        for link in loopbacks {
            let _ = writeln!(w, "    assign pad_{} = out_{};", link.to, link.from);
            driven.push(link.to.clone());
        }
    }
    let _ = writeln!(w);
    let _ = writeln!(w, "    // Pads the design does not use are held low.");
    for pad in &input_pads {
        if !driven.contains(pad) {
            let _ = writeln!(w, "    assign pad_{} = 1'b0;", pad);
        }
    }
    let _ = writeln!(w);

    // ---- stimulus ---------------------------------------------------------
    let _ = writeln!(w, "    always #5 clk = ~clk;");
    let _ = writeln!(w);
    let _ = writeln!(w, "    // New vectors on the falling edge, so both sides see them settled");
    let _ = writeln!(w, "    // well before the next rising edge.");
    let _ = writeln!(w, "    always @(negedge clk) begin");
    let _ = writeln!(
        w,
        "        lfsr <= {{lfsr[30:0], lfsr[31] ^ lfsr[21] ^ lfsr[1] ^ lfsr[0]}};"
    );
    let mut tap = 0usize;
    for signal in &map.inputs {
        for (index, _) in &signal.bits {
            let _ = writeln!(w, "        {} <= lfsr[{}];", signal.bit("", *index), tap % 32);
            tap += 1;
        }
    }
    let _ = writeln!(w, "    end");
    let _ = writeln!(w);

    // ---- comparison -------------------------------------------------------
    let _ = writeln!(w, "    initial begin");
    for signal in &map.inputs {
        let _ = writeln!(w, "        {} = '0;", signal.port);
    }
    let _ = writeln!(w, "        repeat (4) @(posedge clk);");
    let _ = writeln!(w, "        @(negedge clk) reset = 1'b0;");
    let _ = writeln!(w);
    let _ = writeln!(w, "        repeat ({}) begin", cycles);
    let _ = writeln!(w, "            @(posedge clk);");
    let _ = writeln!(w, "            #1;");
    let _ = writeln!(w, "            cycle = cycle + 1;");

    let vector_args: Vec<String> = map.inputs.iter().map(|s| s.port.clone()).collect();
    let vector_format: Vec<String> =
        map.inputs.iter().map(|s| format!("{}=%b", s.port)).collect();

    for signal in &map.outputs {
        for (index, pad) in &signal.bits {
            let reference = signal.bit("ref_", *index);
            let _ = writeln!(w, "            if ({} !== out_{}) begin", reference, pad);
            let _ = writeln!(w, "                mismatches = mismatches + 1;");
            let _ = writeln!(w, "                $display(");
            let _ = writeln!(
                w,
                "                    \"MISMATCH cycle=%0d signal={} pad={} expected=%b got=%b\",",
                signal.bit("", *index),
                pad
            );
            let _ = writeln!(w, "                    cycle, {}, out_{});", reference, pad);
            if !vector_args.is_empty() {
                let _ = writeln!(
                    w,
                    "                $display(\"    inputs: {}\", {});",
                    vector_format.join(" "),
                    vector_args.join(", ")
                );
            }
            let _ = writeln!(w, "                if (mismatches >= 8) begin");
            let _ = writeln!(w, "                    $display(\"EQUIV FAILED\");");
            let _ = writeln!(w, "                    $finish;");
            let _ = writeln!(w, "                end");
            let _ = writeln!(w, "            end");
        }
    }
    let _ = writeln!(w, "        end");
    let _ = writeln!(w);
    let _ = writeln!(w, "        if (mismatches == 0) $display(\"EQUIV OK %0d cycles\", cycle);");
    let _ = writeln!(w, "        else $display(\"EQUIV FAILED\");");
    let _ = writeln!(w, "        $finish;");
    let _ = writeln!(w, "    end");
    let _ = writeln!(w, "endmodule");
    o
}
