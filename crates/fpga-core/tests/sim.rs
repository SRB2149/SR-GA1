//! Simulator tests: combinational routing and logic, registers with derived
//! clocks, synchronous reset, tick-0 edge seeding, and combinational loop
//! detection through the DDIO direction feedback.

use fpga_core::config::Design;
use fpga_core::fabric::Fabric;
use fpga_core::sim::{SimError, SimState, Stimulus};

fn fabric() -> Fabric {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fabric.toml");
    Fabric::load_file(std::path::Path::new(path)).expect("fabric.toml must load")
}

#[test]
fn passthrough_carries_stimulus_to_output() {
    let f = fabric();
    let mut d = Design::new(&f);
    for col in 0..f.columns {
        d.set_clb(&f, col, 0, "minor_horz_sel", 0b01).unwrap();
    }
    let mut stim = Stimulus::default();
    stim.set("input_0", vec![false, true]);

    let (mut state, s0) = SimState::new(&f, &d, &stim).unwrap();
    assert_eq!(s0.horz_edge(0, 0), false);
    let s1 = state.step(&f, &d, &stim).unwrap();
    assert_eq!(s1.horz_edge(0, 0), true);
    let s2 = state.step(&f, &d, &stim).unwrap();
    assert_eq!(s2.horz_edge(0, 0), false);
}

#[test]
fn and3_combinational_logic() {
    let f = fabric();
    let mut d = Design::new(&f);
    // CLB(0,0) computes AND3(input_0, input_1, input_2) (all defaults) and
    // drives it on lane 0; the rest of row 0 passes it to output_0.
    for col in 1..f.columns {
        d.set_clb(&f, col, 0, "minor_horz_sel", 0b01).unwrap();
    }
    let mut stim = Stimulus::default();
    stim.set("input_0", vec![false, true, true, true]);
    stim.set("input_1", vec![true, true, false, true]);
    stim.set("input_2", vec![true, true, true, true]);

    let (mut state, s0) = SimState::new(&f, &d, &stim).unwrap();
    let mut got = vec![s0.horz_edge(0, 0)];
    for _ in 1..4 {
        got.push(state.step(&f, &d, &stim).unwrap().horz_edge(0, 0));
    }
    assert_eq!(got, vec![false, true, false, true]);
}

/// Column 0's clock is derived from input_0: CLB(0,0)'s op (OR3 of input_0
/// with two constant-0 inputs) snakes up the vertical ring to the tap, where
/// CSB0 selects it. CLB(0,0)'s own FF captures op on each rising edge and is
/// observed on output lane 3.
fn clocked_design(f: &Fabric) -> Design {
    let mut d = Design::new(f);
    d.set_clb(f, 0, 0, "operation_select", 1).unwrap(); // OR3 -> op = input_0
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
    stim.set("input_3", vec![true]); // FF enable at (0,0)

    let (mut state, s0) = SimState::new(&f, &d, &stim).unwrap();
    assert_eq!(s0.clocks[0], false);
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
        assert_eq!(state.step(&f, &d, &stim).unwrap().horz_edge(0, 3), false);
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
    assert_eq!(state.ff_value(0, 0), true, "captured before reset");

    // Assert reset while the clock is high: tick 2 falls (no edge), so the
    // reset must wait for tick 3's rising edge.
    state.reset = true;
    state.step(&f, &d, &stim).unwrap();
    assert_eq!(state.ff_value(0, 0), true, "reset is synchronous, not immediate");
    state.step(&f, &d, &stim).unwrap();
    assert_eq!(state.ff_value(0, 0), false, "reset value loaded on rising edge");

    // Configured reset value 1 shows up in the tick-0 state.
    d.set_clb(&f, 0, 0, "op_ff_reset_val", 1).unwrap();
    let (state, _) = SimState::new(&f, &d, &stim).unwrap();
    assert_eq!(state.ff_value(0, 0), true);
}

#[test]
fn no_spurious_edge_on_tick_zero() {
    let f = fabric();
    let d = clocked_design(&f);
    let mut stim = Stimulus::default();
    stim.set("input_0", vec![true]); // clock starts and stays at 1
    stim.set("input_3", vec![true]);

    let (mut state, s0) = SimState::new(&f, &d, &stim).unwrap();
    assert_eq!(s0.clocks[0], true);
    for _ in 1..5 {
        state.step(&f, &d, &stim).unwrap();
        assert_eq!(state.ff_value(0, 0), false, "a constant-1 clock never rises");
    }
}

/// Column 3 as a 3-bit ripple adder: each cell takes its third operand from
/// the carry chain and computes the sum bit with XOR3, so carry-out follows
/// automatically. Operands arrive on lanes 0/1, sums leave on lane 0.
#[test]
fn carry_chain_makes_a_ripple_adder() {
    let f = fabric();
    let mut d = Design::new(&f);
    let xor3 = f.operation_by_name("XOR3").expect("XOR3") as u64;
    for row in 0..3 {
        for col in 0..3 {
            d.set_clb(&f, col, row, "minor_horz_sel", 0b11).unwrap();
        }
        d.set_clb(&f, 3, row, "input_mux_c_sel", 3).unwrap();
        d.set_clb(&f, 3, row, "operation_select", xor3).unwrap();
        for col in 4..f.columns {
            d.set_clb(&f, col, row, "minor_horz_sel", 0b01).unwrap();
        }
    }

    let a_pins = ["input_0", "input_4", "input_8"];
    let b_pins = ["input_1", "input_5", "input_9"];
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
            assert_eq!(s.clb_carry_in(3, 0), false, "chain starts at 0");
            assert_eq!(s.clb_carry_in(3, 1), s.clb_carry(3, 0), "chain runs upward");
        }
    }
}

/// Vertical lanes 0 and 2 carry the combinational `op` output, so routing a
/// CLB's own op around the ring and back into its input c (source 2 = v0)
/// closes a combinational loop. With an inverting op it cannot settle.
/// The lane swap alternates 2 -> 0 -> 2 -> 0 over the four rows.
fn ring_loop_design(f: &Fabric) -> Design {
    let mut d = Design::new(f);
    d.set_clb(f, 0, 0, "input_mux_c_sel", 2).unwrap(); // input c <- v_in[0]
    d.set_clb(f, 0, 0, "operation_select", f.operation_by_name("XNOR3").unwrap() as u64)
        .unwrap(); // op = !c with a = b = 0
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

/// The only combinational feedback in this fabric runs through the DDIO
/// direction gate: route ddio_in_0 through logic to ddio_dir_0, which in turn
/// forces ddio_in_0 low — with the stimulus high, the loop can never settle.
fn ddio_loop_design(f: &Fabric) -> Design {
    let mut d = Design::new(f);
    d.set_clb(f, 0, 2, "operation_select", 1).unwrap(); // OR3 -> op = ddio_in_0
    d.set_clb(f, 0, 3, "minor_horz_sel", 0b01).unwrap(); // lane 0 <- fixed_zero
    d.set_clb(f, 0, 3, "major_horz2_sel", 2).unwrap(); // bridge v_in:2 (ddio) onto lane 2
    d.set_clb(f, 0, 3, "operation_select", 4).unwrap(); // NOR3 -> h_out1 = 0
    d.set_clb(f, 1, 3, "operation_select", 2).unwrap(); // XOR3(0, 0, ddio) = ddio -> lane 0
    for col in 2..f.columns {
        d.set_clb(f, col, 3, "minor_horz_sel", 0b01).unwrap(); // pass lane 0 to ddio_dir_0
    }
    d
}

#[test]
fn ddio_feedback_loop_is_detected() {
    let f = fabric();
    let d = ddio_loop_design(&f);
    let mut stim = Stimulus::default();
    stim.set("ddio_in_0", vec![true]);
    match SimState::new(&f, &d, &stim) {
        Err(SimError::CombinationalLoop { blocks }) => {
            assert!(!blocks.is_empty(), "loop report should name blocks");
        }
        Ok(_) => panic!("oscillating DDIO feedback must be reported as a loop"),
    }
}

#[test]
fn ddio_feedback_settles_when_stimulus_low() {
    let f = fabric();
    let d = ddio_loop_design(&f);
    let mut stim = Stimulus::default();
    stim.set("ddio_in_0", vec![false]);
    let (_, s0) = SimState::new(&f, &d, &stim).unwrap();
    assert_eq!(s0.horz_edge(3, 0), false, "ddio_dir_0 settles low");
    assert_eq!(s0.horz_in(0, 2, 2), false);
}
