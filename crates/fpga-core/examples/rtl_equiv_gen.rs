//! RTL equivalence harness generator.
//!
//! Emits self-checking SystemVerilog testbenches that program the SR_GA1 RTL
//! with a bitstream exported by this tool, drive random stimulus, and compare
//! the chip outputs against this simulator's results tick by tick. See
//! `docs/rtl-equivalence.md` for how the comparison is made sound (X-masking,
//! register cones, glitch-safe clocking) and how to run the testbenches.
//!
//!   cargo run -p fpga-core --example rtl_equiv_gen -- \
//!       [--fabric fabric.toml] [--out tb/equiv] [--comb 4] [--reg 2] \
//!       [--ticks 40] [--seed 1]

use fpga_core::bitstream;
use fpga_core::config::Design;
use fpga_core::drc;
use fpga_core::fabric::{BusOut, Fabric, Source};
use fpga_core::naming::{resolve, NetOrigin};
use fpga_core::sim::{SimState, Stimulus};
use std::collections::HashSet;
use std::path::PathBuf;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn bits(&mut self, width: usize) -> u64 {
        self.next() & ((1u64 << width) - 1)
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |flag: &str, default: &str| -> String {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
            .unwrap_or_else(|| default.to_string())
    };
    let fabric_path = get("--fabric", "fabric.toml");
    let out_dir = PathBuf::from(get("--out", "tb/equiv"));
    let n_comb: usize = get("--comb", "4").parse().unwrap_or(4);
    let n_reg: usize = get("--reg", "2").parse().unwrap_or(2);
    let ticks: usize = get("--ticks", "40").parse().unwrap_or(40);
    let seed: u64 = get("--seed", "1").parse().unwrap_or(1);

    let fabric = match Fabric::load_file(std::path::Path::new(&fabric_path)) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    };
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("error: cannot create {}: {}", out_dir.display(), e);
        std::process::exit(1);
    }

    let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(1));
    let mut names = Vec::new();
    for i in 0..n_comb {
        let name = format!("equiv_comb{}", i);
        emit_case(&fabric, &out_dir, &name, &mut rng, ticks, false);
        names.push(name);
    }
    for i in 0..n_reg {
        let name = format!("equiv_reg{}", i);
        emit_case(&fabric, &out_dir, &name, &mut rng, ticks, true);
        names.push(name);
    }

    // A ModelSim/Questa batch script covering every generated case.
    let hdl = [
        "hdl/regs/shift_reg_no_reset.sv",
        "hdl/mux/mux.sv",
        "hdl/clb/clb.sv",
        "hdl/grid/clb_grid.sv",
        "hdl/io/io_controller.sv",
        "hdl/clock/clock_sel.sv",
        "hdl/clock/clock_bank.sv",
        "hdl/top/sr-ga1.sv",
    ];
    let mut do_script = String::from("vlib work\n");
    do_script.push_str(&format!("vlog -sv {}\n", hdl.join(" ")));
    let out = out_dir.display().to_string().replace('\\', "/");
    for n in &names {
        do_script.push_str(&format!("vlog -sv {}/{}_tb.sv\n", out, n));
    }
    for n in &names {
        do_script.push_str(&format!("vsim -c work.{}_tb -do {}/{}.do\n", n, out, n));
    }
    do_script.push_str("quit -f\n");
    let do_path = out_dir.join("run_all.do");
    std::fs::write(&do_path, do_script).expect("write run_all.do");
    println!("wrote {} testbench(es) and {}", names.len(), do_path.display());
}

fn random_design(fabric: &Fabric, rng: &mut Rng) -> Design {
    let mut d = Design::new(fabric);
    for row in 0..fabric.rows {
        for col in 0..fabric.columns {
            for (i, f) in fabric.clb_fields.iter().enumerate() {
                let v = rng.bits(f.width);
                d.clb_mut(col, row).set(i, f.width, v);
            }
        }
    }
    for col in 0..fabric.columns {
        for (i, f) in fabric.csb_fields.iter().enumerate() {
            // Bias coupling low so most columns tap their own ring.
            let v = if f.name == "couple_to_previous" && !rng.chance(20) { 0 } else { rng.bits(f.width) };
            d.csb_mut(col).set(i, f.width, v);
        }
    }
    d
}

/// The registered template from the simulator tests: column 0's clock is
/// input_0, buffered through CLB(0,0)'s op and snaked up the vertical ring;
/// rows 0-2 of column 0 hold randomized logic whose registers are observed on
/// lane 3 of their rows at the right edge.
fn registered_design(fabric: &Fabric, rng: &mut Rng) -> Design {
    let mut d = Design::new(fabric);
    let set = |d: &mut Design, col: usize, row: usize, field: &str, v: u64| {
        d.set_clb(fabric, col, row, field, v).expect("template field");
    };
    set(&mut d, 0, 0, "operation_select", 1); // OR3 of (input_0, 0-able, 0-able)
    set(&mut d, 0, 1, "minor_vert_sel", 0b0100);
    set(&mut d, 0, 2, "minor_vert_sel", 0b0001);
    set(&mut d, 0, 3, "minor_vert_sel", 0b0100);
    d.set_csb(fabric, 0, "bus_addr", 2).expect("bus_addr");
    // Observe rows 0..3 registers of column 0 on lane 3 across each row.
    for row in 0..fabric.rows.min(3) {
        set(&mut d, 0, row, "major_horz3_sel", 5);
        for col in 1..fabric.columns {
            set(&mut d, col, row, "major_horz3_sel", 3);
        }
    }
    // Randomize the clocked data logic in column 0 (not the clock buffer's
    // input muxes, which must keep reading input_0 on mux a).
    for row in 1..fabric.rows.min(3) {
        set(&mut d, 0, row, "input_mux_a_sel", rng.bits(1));
        set(&mut d, 0, row, "input_mux_b_sel", rng.bits(1));
        set(&mut d, 0, row, "input_mux_c_sel", rng.bits(2));
        set(&mut d, 0, row, "operation_select", rng.bits(3));
        set(&mut d, 0, row, "op_ff_reset_val", rng.bits(1));
    }
    set(&mut d, 0, 0, "op_ff_reset_val", rng.bits(1));
    d
}

/// Which output bits may be compared. Walks each output's value cone: a
/// register outside the allowed set, or a floating loop, makes the bit
/// unpredictable in RTL (X) or model-divergent, so it is masked out.
fn output_mask(fabric: &Fabric, design: &Design, allow_reg_col0: bool) -> u16 {
    let nets = resolve(fabric, design);
    let mut mask = 0u16;
    for (row, lanes) in fabric.io_outputs.iter().enumerate() {
        for (lane, name) in lanes.iter().enumerate() {
            let Some(name) = name else { continue };
            let Some(bit) = out_bit(name) else { continue };
            if cone_is_comparable(fabric, design, &nets, row, lane, allow_reg_col0) {
                mask |= 1 << bit;
            }
        }
    }
    mask
}

fn out_bit(name: &str) -> Option<usize> {
    if let Some(n) = name.strip_prefix("output_") {
        return n.parse().ok();
    }
    if let Some(n) = name.strip_prefix("ddio_out_") {
        return n.parse::<usize>().ok().map(|k| 10 + k);
    }
    if let Some(n) = name.strip_prefix("ddio_dir_") {
        return n.parse::<usize>().ok().map(|k| 12 + k);
    }
    None
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Seg {
    H(usize, usize, usize), // row, lane, pos
    V(usize, usize, usize), // col, lane, pos
}

fn cone_is_comparable(
    fabric: &Fabric,
    design: &Design,
    nets: &fpga_core::naming::Netlist,
    row: usize,
    lane: usize,
    allow_reg_col0: bool,
) -> bool {
    let mut visited: HashSet<Seg> = HashSet::new();
    let mut stack = vec![Seg::H(row, lane, fabric.columns)];
    let hmux = |lane: usize| fabric.output_muxes.iter().find(|m| m.drives == BusOut::Horz(lane)).unwrap();
    let vmux = |lane: usize| fabric.output_muxes.iter().find(|m| m.drives == BusOut::Vert(lane)).unwrap();
    while let Some(seg) = stack.pop() {
        if !visited.insert(seg) {
            continue;
        }
        // A floating pass-through ring is X in RTL, 0 in this model.
        let floating = match seg {
            Seg::H(r, l, p) if p < fabric.columns => nets.horz_in(p, r, l) == NetOrigin::FloatingLoop,
            Seg::H(r, l, _) => nets.horz_edge(r, l) == NetOrigin::FloatingLoop,
            Seg::V(c, l, p) => nets.vert_in(c, p, l) == NetOrigin::FloatingLoop,
        };
        if floating {
            return false;
        }
        let push_src = |stack: &mut Vec<Seg>, src: Source, col: usize, cell_row: usize| -> bool {
            match src {
                Source::Const(_) => true,
                Source::HorzIn(k) => {
                    stack.push(Seg::H(cell_row, k, col));
                    true
                }
                Source::VertIn(k) => {
                    stack.push(Seg::V(col, k, cell_row));
                    true
                }
                Source::Reg => allow_reg_col0 && col == 0,
                Source::Op | Source::Carry | Source::CarryIn => {
                    // A carry input is the cell below's carry, so walk down
                    // the chain collecting every stage's bus dependencies.
                    let mut r = cell_row;
                    if src == Source::CarryIn {
                        if r == 0 {
                            return true; // chain edge constant
                        }
                        r -= 1;
                    }
                    loop {
                        let mut chained = false;
                        for m in &fabric.input_muxes {
                            let sel = design.clb(col, r).slice(&m.select) as usize;
                            match m.sources[sel] {
                                Source::HorzIn(k) => stack.push(Seg::H(r, k, col)),
                                Source::VertIn(k) => stack.push(Seg::V(col, k, r)),
                                Source::CarryIn => chained = r > 0,
                                _ => {}
                            }
                        }
                        if !chained {
                            break;
                        }
                        r -= 1;
                    }
                    true
                }
            }
        };
        let ok = match seg {
            Seg::H(r, l, 0) => {
                // Left edge; a gated DDIO input depends on its direction pin.
                let name = &fabric.io_inputs[r][l];
                if let Some(dd) = fabric.ddio.iter().find(|d| d.input == *name) {
                    if let Some((dr, dl)) = fabric.io_outputs.iter().enumerate().find_map(|(rr, ls)| {
                        ls.iter().position(|n| n.as_deref() == Some(dd.dir.as_str())).map(|ll| (rr, ll))
                    }) {
                        stack.push(Seg::H(dr, dl, fabric.columns));
                    }
                }
                true
            }
            Seg::H(r, l, p) => {
                let col = p - 1;
                let m = hmux(l);
                let sel = design.clb(col, r).slice(&m.select) as usize;
                push_src(&mut stack, m.sources[sel], col, r)
            }
            Seg::V(c, l, p) => {
                let driver_row = (p + fabric.rows - 1) % fabric.rows;
                let m = vmux(l);
                let sel = design.clb(c, driver_row).slice(&m.select) as usize;
                push_src(&mut stack, m.sources[sel], c, driver_row)
            }
        };
        if !ok {
            return false;
        }
    }
    // If registers are allowed, the column's clock must be the glitch-safe
    // template clock, which the caller guarantees by construction.
    true
}

fn stimulus_inputs(fabric: &Fabric) -> Vec<String> {
    let mut names = Vec::new();
    for row in &fabric.io_inputs {
        for name in row {
            if *name != fabric.naming.constant_zero && *name != fabric.naming.constant_one && !names.contains(name) {
                names.push(name.clone());
            }
        }
    }
    names
}

fn stim_bit(name: &str) -> Option<usize> {
    if let Some(n) = name.strip_prefix("input_") {
        return n.parse().ok();
    }
    if let Some(n) = name.strip_prefix("ddio_in_") {
        return n.parse::<usize>().ok().map(|k| 10 + k);
    }
    None
}

fn emit_case(fabric: &Fabric, out_dir: &std::path::Path, name: &str, rng: &mut Rng, ticks: usize, registered: bool) {
    let inputs = stimulus_inputs(fabric);
    'attempt: for attempt in 0..500 {
        let design = if registered { registered_design(fabric, rng) } else { random_design(fabric, rng) };
        // Reject any structural combinational cycle. A non-inverting loop
        // settles here but can still spin forever in delta-cycle event
        // simulation, so these configurations are not comparable. Since
        // input mux c can read vertical lane 0 (a combinational lane), the
        // vertical ring is a loop source as well as the DDIO gate.
        if !drc::combinational_loops(fabric, &design).is_empty() {
            continue 'attempt;
        }
        // Random stimulus. The registered class is two-phase: input_0 (the
        // clock) toggles alone on odd ticks; data changes only on even ticks.
        let mut vectors: Vec<u16> = Vec::with_capacity(ticks);
        let mut data: u16 = (rng.next() & 0xFFF) as u16;
        for t in 0..ticks {
            if registered {
                if t % 2 == 0 {
                    data = (rng.next() & 0xFFE) as u16; // input_0 low
                    vectors.push(data);
                } else {
                    vectors.push(data | 1); // only input_0 rises
                }
            } else {
                vectors.push((rng.next() & 0xFFF) as u16);
            }
        }
        let mut stim = Stimulus::default();
        for input in &inputs {
            let Some(bit) = stim_bit(input) else { continue };
            let pattern: Vec<bool> = vectors.iter().map(|v| (v >> bit) & 1 != 0).collect();
            stim.set(input, pattern);
        }

        let mask = output_mask(fabric, &design, registered);
        if mask == 0 || (registered && mask & 0b1000 == 0) {
            continue 'attempt; // nothing comparable — re-roll
        }

        // Reference run: expected[t] is the post-commit settled output state.
        let Ok((mut state, s0)) = SimState::new(fabric, &design, &stim) else {
            continue 'attempt; // combinational loop — re-roll
        };
        let outputs_of = |s: &fpga_core::sim::Settled| -> u16 {
            let mut v = 0u16;
            for (row, lanes) in fabric.io_outputs.iter().enumerate() {
                for (lane, name) in lanes.iter().enumerate() {
                    if let Some(bit) = name.as_deref().and_then(out_bit) {
                        if s.horz_edge(row, lane) {
                            v |= 1 << bit;
                        }
                    }
                }
            }
            v
        };
        let mut expected = vec![outputs_of(&s0)];
        let mut ok = true;
        for _ in 1..ticks {
            if state.step(fabric, &design, &stim).is_err() {
                ok = false;
                break;
            }
            match state.view(fabric, &design, &stim) {
                Ok(v) => expected.push(outputs_of(&v)),
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue 'attempt;
        }

        // Quiescent ring values (all inputs 0, registers at reset): assigned
        // procedurally after `release`, because a released variable keeps the
        // forced value until its driver next changes — a ring segment that
        // settles to a constant would otherwise stay stuck at the forced 0.
        let quiet_rings: Vec<u8> = match SimState::new(fabric, &design, &Stimulus::default()) {
            Ok((_, s)) => (0..fabric.columns)
                .map(|col| {
                    (0..fabric.vert_lanes).fold(0u8, |acc, lane| {
                        acc | ((s.ring_tap(col, lane) as u8) << lane)
                    })
                })
                .collect(),
            Err(_) => vec![0; fabric.columns],
        };

        let bits = bitstream::export_bits(fabric, &design);
        let tb = render_tb(name, &bits, &vectors, &expected, mask, registered);
        let path = out_dir.join(format!("{}_tb.sv", name));
        std::fs::write(&path, tb).expect("write testbench");
        std::fs::write(out_dir.join(format!("{}.do", name)), render_do(name, bits.len(), &quiet_rings))
            .expect("write do script");
        println!(
            "{}: attempt {}, mask {:014b}, {} ticks -> {}",
            name,
            attempt + 1,
            mask,
            ticks,
            path.display()
        );
        return;
    }
    eprintln!("{}: gave up after 500 attempts", name);
}

/// The per-case simulator control script. Pure pass-through vertical rings
/// are copy cycles: in zero-delay event simulation, unequal values rotate
/// around them forever and hit the delta iteration limit — transiently
/// during programming for almost any bitstream. Every ring closes through
/// the top-level vertical_buses nets, so the script freezes those at 0 for
/// exactly the programming phase (2 ns per bit), then releases them and
/// deposits the quiescent settled values (all inputs 0, registers at reset).
/// A deposit yields to the next real driver change, so nothing stays stale;
/// this is done at the tool level because an SV-side force/release on these
/// port-collapsed variables permanently detaches them from their drivers in
/// ModelSim.
fn render_do(name: &str, n_bits: usize, quiet_rings: &[u8]) -> String {
    // The cut point is each row-0 CLB's vert_bus_in port: ModelSim refuses
    // to force elements of the top-level unpacked vertical_buses array, but
    // the packed per-instance port takes a force, and every ring cycle runs
    // through it.
    let port = |c: usize| format!("{{/{}_tb/dut/clb_grid_u/row[0]/col[{}]/clb_u/vert_bus_in}}", name, c);
    let mut s = String::new();
    for c in 0..quiet_rings.len() {
        s.push_str(&format!("force -freeze {} 2#0000\n", port(c)));
    }
    s.push_str(&format!("run {}ns\n", 2 * n_bits));
    for (c, v) in quiet_rings.iter().enumerate() {
        s.push_str(&format!("noforce {}\n", port(c)));
        s.push_str(&format!("force -deposit {} 2#{:04b}\n", port(c), v));
    }
    s.push_str("run -all\nquit -f\n");
    s
}

#[allow(clippy::too_many_arguments)]
fn render_tb(
    name: &str,
    bits: &[bool],
    vectors: &[u16],
    expected: &[u16],
    mask: u16,
    registered: bool,
) -> String {
    let n = bits.len();
    let bitstr: String = bits.iter().map(|&b| if b { '1' } else { '0' }).collect();
    let mut s = String::new();
    s.push_str(&format!(
        "// Generated by rtl_equiv_gen — do not edit. Class: {}.\n`timescale 1ns/1ps\n\nmodule {}_tb;\n",
        if registered { "registered" } else { "combinational" },
        name
    ));
    s.push_str(
        "    logic       shift_clk = 0, shift_data_in = 0, reset = 1;\n\
         \x20   logic [9:0] inputs = '0;\n\
         \x20   logic [1:0] ddio_in = '0;\n\
         \x20   wire  [9:0] outputs;\n\
         \x20   wire  [1:0] ddio_dir, ddio_out;\n\
         \x20   wire        shift_data_out;\n\n\
         \x20   SR_GA1 dut (.*);\n\n",
    );
    s.push_str(&format!("    localparam int TICKS = {};\n", vectors.len()));
    s.push_str(&format!("    localparam logic [0:{}] BITS = {}'b{};\n", n - 1, n, bitstr));
    s.push_str(&format!("    localparam logic [13:0] MASK = 14'b{:014b};\n", mask));
    s.push_str("    logic [11:0] stim     [0:TICKS-1];\n    logic [13:0] expected [0:TICKS-1];\n");
    s.push_str("    integer errors = 0;\n\n    initial begin\n");
    for (t, v) in vectors.iter().enumerate() {
        s.push_str(&format!("        stim[{}] = 12'b{:012b};\n", t, v));
    }
    for (t, v) in expected.iter().enumerate() {
        s.push_str(&format!("        expected[{}] = 14'b{:014b};\n", t, v));
    }
    s.push_str(&format!(
        "\n        // Program the scan chain (first bit transmitted lands deepest).\n\
         \x20       // The companion .do script freezes the ring nets during this\n\
         \x20       // phase and deposits their settled values afterwards.\n\
         \x20       for (int i = 0; i < {}; i++) begin\n\
         \x20           shift_data_in = BITS[i];\n\
         \x20           #1 shift_clk = 1; #1 shift_clk = 0;\n\
         \x20       end\n\
         \x20       shift_data_in = 0;\n\
         \x20       // Reset training: give every input-derived clock edges while\n\
         \x20       // reset is held, so reachable registers load their reset values.\n\
         \x20       reset = 1;\n\
         \x20       repeat (3) begin\n\
         \x20           {{ddio_in, inputs}} = 12'h000; #5;\n\
         \x20           {{ddio_in, inputs}} = 12'hFFF; #5;\n\
         \x20       end\n\
         \x20       {{ddio_in, inputs}} = stim[0]; #5;\n\
         \x20       reset = 0; #1;\n\
         \x20       check(0);\n\
         \x20       for (int t = 1; t < TICKS; t++) begin\n\
         \x20           {{ddio_in, inputs}} = stim[t];\n\
         \x20           #5;\n\
         \x20           check(t);\n\
         \x20       end\n\
         \x20       if (errors == 0) $display(\"PASS {}\");\n\
         \x20       else $display(\"FAIL {}: %0d mismatch(es)\", errors);\n\
         \x20       $finish;\n\
         \x20   end\n\n",
        n, name, name
    ));
    s.push_str(
        "    task check(input int t);\n\
         \x20       logic [13:0] got;\n\
         \x20       got = {ddio_dir, ddio_out, outputs};\n\
         \x20       for (int b = 0; b < 14; b++) begin\n\
         \x20           if (MASK[b] && (got[b] !== expected[t][b])) begin\n\
         \x20               errors++;\n\
         \x20               $display(\"tick %0d bit %0d: got %b expected %b\", t, b, got[b], expected[t][b]);\n\
         \x20           end\n\
         \x20       end\n\
         \x20   endtask\n\nendmodule\n",
    );
    s
}
