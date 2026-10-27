//! Simulator tests: combinational routing and LUT logic, registers with
//! derived clocks, synchronous reset, tick-0 edge seeding, the IO controller's
//! muxes and on-chip loops, and combinational loop detection through the
//! vertical ring and DDIO read-back.

use fpga_core::config::Design;
use fpga_core::fabric::Fabric;
use fpga_core::sim::{SimError, SimState, Stimulus};

fn fabric() -> Fabric {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fabric.toml");
    Fabric::load_file(std::path::Path::new(path)).expect("fabric.toml must load")
}

fn lut(f: &Fabric, name: &str) -> u64 {
    f.preset_value(name).expect(name)
}

fn out_pad(f: &Fabric, name: &str) -> usize {
    f.output_pad(name).expect(name)
}

#[test]
fn passthrough_carries_stimulus_to_output() {
    let f = fabric();
    let mut d = Design::new(&f);
    for col in 0..f.columns {
        d.set_clb(&f, col, 0, "minor_horz_sel", 0b01).unwrap();
    }
    // The all-zero IO configuration feeds row 0 lane 0 from input_0 and
    // drives output_0 from row 0 lane 0.
    let mut stim = Stimulus::default();
    stim.set("input_0", vec![false, true]);

    let o0 = out_pad(&f, "output_0");
    let (mut state, s0) = SimState::new(&f, &d, &stim).unwrap();
    assert!(!s0.pad_out(o0));
    let s1 = state.step(&f, &d, &stim).unwrap();
    assert!(s1.pad_out(o0));
    assert!(s1.horz_edge(0, 0));
    let s2 = state.step(&f, &d, &stim).unwrap();
    assert!(!s2.pad_out(o0));
}

#[test]
fn and3_lut_logic() {
    let f = fabric();
    let mut d = Design::new(&f);
    // CLB(0,0) computes AND3(input_0, input_1, input_2): a reads lane 0
    // (input_0), b reads lane 1 (input mux set to input_1), c reads lane 2
    // (fixed input_2). The rest of row 0 passes it to output_0.
    d.set_io(&f, "input_mux_sel0b", 1).unwrap();
    d.set_clb(&f, 0, 0, "lut", lut(&f, "AND3")).unwrap();
    for col in 1..f.columns {
        d.set_clb(&f, col, 0, "minor_horz_sel", 0b01).unwrap();
    }
    let mut stim = Stimulus::default();
    stim.set("input_0", vec![false, true, true, true]);
    stim.set("input_1", vec![true, true, false, true]);
    stim.set("input_2", vec![true, true, true, true]);

    let o0 = out_pad(&f, "output_0");
    let (mut state, s0) = SimState::new(&f, &d, &stim).unwrap();
    let mut got = vec![s0.pad_out(o0)];
    for _ in 1..4 {
        got.push(state.step(&f, &d, &stim).unwrap().pad_out(o0));
    }
    assert_eq!(got, vec![false, true, false, true]);
}

#[test]
fn lut_index_puts_input_a_first() {
    // MUX2 = c ? a : b is not symmetric, so it catches a reversed index.
    let f = fabric();
    let mut d = Design::new(&f);
    d.set_io(&f, "input_mux_sel0b", 1).unwrap();
    d.set_clb(&f, 0, 0, "lut", lut(&f, "MUX2")).unwrap();
    for (a, b, c) in [(true, false, true), (false, true, true), (true, false, false), (false, true, false)] {
        let mut stim = Stimulus::default();
        stim.set("input_0", vec![a]);
        stim.set("input_1", vec![b]);
        stim.set("input_2", vec![c]);
        let (_, s) = SimState::new(&f, &d, &stim).unwrap();
        assert_eq!(s.clb_op(0, 0), if c { a } else { b }, "a={a} b={b} c={c}");
    }
}

/// Column 0's clock is derived from input_0: CLB(0,0)'s op (BUF_A of
/// input_0) snakes up the vertical ring to the tap, where CSB0 selects it.
/// CLB(0,0)'s own FF captures op on each rising edge and is observed on
/// right-edge row 0 lane 3.
fn clocked_design(f: &Fabric) -> Design {
    let mut d = Design::new(f);
    d.set_clb(f, 0, 0, "lut", lut(f, "BUF_A")).unwrap();
    d.set_clb(f, 0, 1, "minor_vert_sel", 0b0100).unwrap();
    d.set_clb(f, 0, 2, "minor_vert_sel", 0b0001).unwrap();
    d.set_clb(f, 0, 3, "minor_vert_sel", 0b0100).unwrap();
    d.set_csb(f, 0, "bus_addr", 2).unwrap();
    d.set_clb(f, 0, 0, "major_horz3_sel", 5).unwrap(); // reg -> lane 3
    for col in 1..f.columns {
        d.set_clb(f, col, 0, "major_horz3_sel", 3).unwrap(); // pass lane 3
    }
    d
}

#[test]
fn register_captures_on_derived_clock_edges() {
    let f = fabric();
    let d = clocked_design(&f);
    let mut stim = Stimulus::default();
    stim.set("input_0", vec![false, true]);
    stim.set("input_3", vec![true]); // FF enable at (0,0): row 0 lane 3 is fixed input_3

    let (mut state, s0) = SimState::new(&f, &d, &stim).unwrap();
    assert!(!s0.clocks[0]);
    // Registers commit at end of tick, so the FF's new value is visible from
    // the tick after each rising edge.
    let mut got = vec![s0.horz_edge(0, 3)];
    for _ in 1..5 {
        got.push(state.step(&f, &d, &stim).unwrap().horz_edge(0, 3));
    }
    assert_eq!(got, vec![false, false, true, true, true]);
}

#[test]
fn ff_enable_gates_capture() {
    let f = fabric();
    let d = clocked_design(&f);
    let mut stim = Stimulus::default();
    stim.set("input_0", vec![false, true]);
    stim.set("input_3", vec![false]); // enable held low: FF never captures

    let (mut state, _) = SimState::new(&f, &d, &stim).unwrap();
    for _ in 1..6 {
        assert!(!state.step(&f, &d, &stim).unwrap().horz_edge(0, 3));
    }
}

#[test]
fn synchronous_reset_takes_effect_on_next_edge() {
    let f = fabric();
    let mut d = clocked_design(&f);
    let mut stim = Stimulus::default();
    stim.set("input_0", vec![false, true]);
    stim.set("input_3", vec![true]);

    let (mut state, _) = SimState::new(&f, &d, &stim).unwrap();
    // Tick 1 rises (input_0 goes 0 -> 1) and the FF captures 1.
    state.step(&f, &d, &stim).unwrap();
    assert!(state.ff_value(0, 0), "captured before reset");

    // Assert reset while the clock is high: tick 2 falls (no edge), so the
    // reset must wait for tick 3's rising edge.
    state.reset = true;
    state.step(&f, &d, &stim).unwrap();
    assert!(state.ff_value(0, 0), "reset is synchronous, not immediate");
    state.step(&f, &d, &stim).unwrap();
    assert!(!state.ff_value(0, 0), "reset value loaded on rising edge");

    // Configured reset value 1 shows up in the tick-0 state.
    d.set_clb(&f, 0, 0, "op_ff_reset_val", 1).unwrap();
    let (state, _) = SimState::new(&f, &d, &stim).unwrap();
    assert!(state.ff_value(0, 0));
}

#[test]
fn no_spurious_edge_on_tick_zero() {
    let f = fabric();
    let d = clocked_design(&f);
    let mut stim = Stimulus::default();
    stim.set("input_0", vec![true]); // clock starts and stays at 1
    stim.set("input_3", vec![true]);

    let (mut state, s0) = SimState::new(&f, &d, &stim).unwrap();
    assert!(s0.clocks[0]);
    for _ in 1..5 {
        state.step(&f, &d, &stim).unwrap();
        assert!(!state.ff_value(0, 0), "a constant-1 clock never rises");
    }
}

/// Column 3 as a 3-bit ripple adder: each cell takes its third operand from
/// the carry chain and computes the sum bit with XOR3, so carry-out follows
/// automatically. Operands arrive on lanes 0/1, sums leave on lane 0.
#[test]
fn carry_chain_makes_a_ripple_adder() {
    let f = fabric();
    let mut d = Design::new(&f);
    let a_pins = ["input_0", "input_4", "input_8"];
    let b_pins = ["input_1", "input_5", "input_9"];
    for row in 0..3 {
        d.set_io(&f, &format!("input_mux_sel{row}a"), f.input_pad(a_pins[row]).unwrap() as u64).unwrap();
        d.set_io(&f, &format!("input_mux_sel{row}b"), f.input_pad(b_pins[row]).unwrap() as u64).unwrap();
        for col in 0..3 {
            d.set_clb(&f, col, row, "minor_horz_sel", 0b11).unwrap();
        }
        d.set_clb(&f, 3, row, "input_mux_c_sel", 3).unwrap();
        d.set_clb(&f, 3, row, "lut", lut(&f, "XOR3")).unwrap();
        for col in 4..f.columns {
            d.set_clb(&f, col, row, "minor_horz_sel", 0b01).unwrap();
        }
    }

    for a in 0..8u32 {
        for b in 0..8u32 {
            let mut stim = Stimulus::default();
            for (i, pin) in a_pins.iter().enumerate() {
                stim.set(pin, vec![(a >> i) & 1 != 0]);
            }
            for (i, pin) in b_pins.iter().enumerate() {
                stim.set(pin, vec![(b >> i) & 1 != 0]);
            }
            let (_, s) = SimState::new(&f, &d, &stim).unwrap();
            let sum = (0..3).fold(0u32, |acc, row| acc | ((s.horz_edge(row, 0) as u32) << row));
            let carry_out = s.clb_carry(3, 2) as u32;
            assert_eq!(sum | (carry_out << 3), a + b, "{} + {}", a, b);
            assert!(!s.clb_carry_in(3, 0), "chain starts at 0");
            assert_eq!(s.clb_carry_in(3, 1), s.clb_carry(3, 0), "chain runs upward");
        }
    }
}

/// A register that depends on its own value, with no board wiring: CLB(0,0)
/// inverts its own registered output, which reaches the right edge on lane 3
/// and returns to its input a through the IO controller (output_0 selects row
/// 0 lane 3; row 0 lane 0's input mux reads output_0). Column 0's clock comes
/// from input_4 through CLB(0,1) and the vertical ring.
#[test]
fn on_chip_loop_makes_a_toggle_flip_flop() {
    let f = fabric();
    let mut d = Design::new(&f);
    d.set_io(&f, "output_mux_sel0", 3).unwrap(); // output_0 <- row 0 lane 3
    d.set_io(&f, "input_mux_sel0a", 12).unwrap(); // row 0 lane 0 <- output_0
    d.set_clb(&f, 0, 0, "lut", lut(&f, "INV_A")).unwrap();
    d.set_clb(&f, 0, 0, "major_horz3_sel", 5).unwrap(); // reg -> lane 3
    for col in 1..f.columns {
        d.set_clb(&f, col, 0, "major_horz3_sel", 3).unwrap();
    }
    d.set_io(&f, "input_mux_sel1a", 4).unwrap(); // row 1 lane 0 <- input_4
    d.set_clb(&f, 0, 1, "lut", lut(&f, "BUF_A")).unwrap(); // op onto v lane 0
    d.set_clb(&f, 0, 2, "minor_vert_sel", 0b0100).unwrap(); // v2 <- v0
    d.set_clb(&f, 0, 3, "minor_vert_sel", 0b0001).unwrap(); // v0 <- v2, to the tap
    d.set_csb(&f, 0, "bus_addr", 0).unwrap();

    let mut stim = Stimulus::default();
    stim.set("input_4", vec![false, true]);
    stim.set("input_3", vec![true]);

    let o0 = out_pad(&f, "output_0");
    let (mut state, s0) = SimState::new(&f, &d, &stim).unwrap();
    let mut got = vec![s0.pad_out(o0)];
    for _ in 1..7 {
        got.push(state.step(&f, &d, &stim).unwrap().pad_out(o0));
    }
    assert_eq!(got, vec![false, false, true, true, false, false, true]);
    assert!(fpga_core::drc::combinational_loops(&f, &d).is_empty(), "the loop is registered");
}

#[test]
fn an_unregistered_on_chip_loop_is_detected() {
    let f = fabric();
    let mut d = Design::new(&f);
    d.set_io(&f, "input_mux_sel0a", 12).unwrap(); // row 0 lane 0 <- output_0
    d.set_clb(&f, 0, 0, "lut", lut(&f, "INV_A")).unwrap();
    for col in 1..f.columns {
        d.set_clb(&f, col, 0, "minor_horz_sel", 0b01).unwrap();
    }
    assert!(matches!(
        SimState::new(&f, &d, &Stimulus::default()),
        Err(SimError::CombinationalLoop { .. })
    ));
    assert!(!fpga_core::drc::combinational_loops(&f, &d).is_empty());
}

/// Vertical lanes 0 and 2 carry the combinational `op` output, so routing a
/// CLB's own op around the ring and back into its input c (source 2 = v0)
/// closes a combinational loop. With an inverting LUT it cannot settle.
/// The lane swap alternates 2 -> 0 -> 2 -> 0 over the four rows.
fn ring_loop_design(f: &Fabric) -> Design {
    let mut d = Design::new(f);
    d.set_clb(f, 0, 0, "input_mux_c_sel", 2).unwrap(); // input c <- v_in[0]
    d.set_clb(f, 0, 0, "lut", lut(f, "INV_C")).unwrap(); // op = !c
    d.set_clb(f, 0, 1, "minor_vert_sel", 0b0001).unwrap(); // v_out[0] <- v_in[2]
    d.set_clb(f, 0, 2, "minor_vert_sel", 0b0100).unwrap(); // v_out[2] <- v_in[0]
    d.set_clb(f, 0, 3, "minor_vert_sel", 0b0001).unwrap(); // v_out[0] <- v_in[2]
    d
}

#[test]
fn vertical_ring_loop_is_detected_not_hung_on() {
    let f = fabric();
    let d = ring_loop_design(&f);
    match SimState::new(&f, &d, &Stimulus::default()) {
        Err(SimError::CombinationalLoop { blocks }) => {
            assert!(!blocks.is_empty(), "loop report should name blocks");
        }
        Ok(_) => panic!("an inverting ring loop through input c must not settle"),
    }
}

/// DDIO read-back: CLB(0,2) inverts ddio_in_0 (row 2 lane 2) onto row 2 lane
/// 0, and that lane drives both ddio_out_0 and ddio_dir_0. With the pin high
/// the lane is 0, the pad stays an input and all is stable. With the pin low
/// the lane goes 1, the pad turns into an output and reads back its own 1,
/// which drives the lane to 0 again: it can never settle.
fn ddio_loop_design(f: &Fabric) -> Design {
    let mut d = Design::new(f);
    d.set_clb(f, 0, 2, "lut", lut(f, "INV_C")).unwrap(); // c = row 2 lane 2 = ddio_in_0
    for col in 1..f.columns {
        d.set_clb(f, col, 2, "minor_horz_sel", 0b01).unwrap();
    }
    d.set_io(f, "output_mux_sel_ddio0", 8).unwrap(); // row 2 lane 0
    d.set_io(f, "output_mux_sel_ddio_dir0", 8).unwrap();
    d
}

#[test]
fn ddio_read_back_loop_is_detected() {
    let f = fabric();
    let d = ddio_loop_design(&f);
    let mut stim = Stimulus::default();
    stim.set("ddio_in_0", vec![false]);
    match SimState::new(&f, &d, &stim) {
        Err(SimError::CombinationalLoop { blocks }) => {
            assert!(!blocks.is_empty(), "loop report should name blocks");
        }
        Ok(_) => panic!("oscillating DDIO read-back must be reported as a loop"),
    }
}

#[test]
fn ddio_read_back_settles_when_the_pin_is_high() {
    let f = fabric();
    let d = ddio_loop_design(&f);
    let mut stim = Stimulus::default();
    stim.set("ddio_in_0", vec![true]);
    let (_, s0) = SimState::new(&f, &d, &stim).unwrap();
    assert!(!s0.pad_out(out_pad(&f, "ddio_dir_0")), "ddio_dir_0 settles low");
    assert!(s0.horz_in(0, 2, 2), "the pad stays an input and reads the pin");
}

#[test]
fn a_ddio_in_output_mode_reads_back_its_own_output() {
    let f = fabric();
    let mut d = Design::new(&f);
    // ddio_out_0 <- row 0 lane 0 (input_0 passed straight across), dir <- row
    // 0 lane 2 (a constant 1 from CLB(7,0)'s major mux).
    for col in 0..f.columns {
        d.set_clb(&f, col, 0, "minor_horz_sel", 0b01).unwrap();
    }
    d.set_clb(&f, 7, 0, "major_horz2_sel", 1).unwrap();
    d.set_io(&f, "output_mux_sel_ddio0", 0).unwrap();
    d.set_io(&f, "output_mux_sel_ddio_dir0", 2).unwrap();
    let mut stim = Stimulus::default();
    stim.set("input_0", vec![true]);
    stim.set("ddio_in_0", vec![false]);
    let (_, s) = SimState::new(&f, &d, &stim).unwrap();
    let ddio_in_0 = f.input_pad("ddio_in_0").unwrap();
    assert!(s.pad_in(ddio_in_0), "dir = 1: the fabric reads the pad's own output");
    assert!(s.horz_in(0, 2, 2), "row 2 lane 2 is fixed to ddio_in_0");
}
