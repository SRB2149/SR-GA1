//! The end-to-end check: a counter produced by `sr-ga1-synth` must actually
//! count when this crate simulates it.
//!
//! `tests/data/count4.json` is a real synthesiser output for
//! `crates/sr-ga1-synth/tests/designs/count4.sv` — `count <= count + 1` — using
//! the carry chain, a derived column clock, lane-3 enables, and the IO
//! controller's on-chip loops to bring each `count` bit back to its adder cell.
//! No board wiring is involved.
//!
//! Regenerate with:
//!
//! ```text
//! cargo run -p sr-ga1-synth -- crates/sr-ga1-synth/tests/designs/count4.sv \
//!     --top count4 -o crates/fpga-core/tests/data/count4.json --seed 0
//! ```

use fpga_core::config::IoPad;
use fpga_core::designfile::load_design;
use fpga_core::drc::{self, Severity};
use fpga_core::fabric::{Fabric, IoSource};
use fpga_core::sim::{SimState, Stimulus};

fn fabric() -> Fabric {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fabric.toml");
    Fabric::load_file(std::path::Path::new(path)).expect("fabric.toml must load")
}

fn counter() -> (Fabric, fpga_core::designfile::DesignFile) {
    let f = fabric();
    let src = include_str!("data/count4.json");
    let (file, warnings) = load_design(&f, src).expect("the synthesised design must load");
    assert!(warnings.is_empty(), "loaded with warnings: {:?}", warnings);
    (f, file)
}

#[test]
fn the_synthesised_design_carries_its_names_and_loops() {
    let (f, file) = counter();

    // Each count bit feeds its own adder cell, which is why the design needs
    // loops at all; they are on-chip, so no board wiring.
    assert!(file.design.loopback().is_empty(), "no board wiring needed");
    let on_chip = (0..f.rows)
        .flat_map(|row| (0..f.horz_lanes).map(move |lane| (row, lane)))
        .filter(|&(row, lane)| matches!(file.design.left_source(&f, row, lane), IoSource::OutPad(_)))
        .count();
    assert!(on_chip >= 1, "the count bits return through the IO controller");

    // And the design's own names, so the canvas reads `clk` rather than a pin id.
    let named: Vec<String> = file
        .design
        .io_names()
        .map(|(pad, name)| format!("{} on {}", name, pad.reserved_name(&f).unwrap_or_default()))
        .collect();
    assert!(named.iter().any(|n| n.starts_with("clk on ")), "the clock should be named: {:?}", named);
    assert!(
        named.iter().filter(|n| n.starts_with("count[")).count() >= 4,
        "every count bit should be named: {:?}",
        named
    );
}

/// The one that matters. Toggle the clock pin and watch the count advance.
#[test]
fn the_synthesised_counter_counts() {
    let (f, file) = counter();

    // Find the clock pin by the name the synthesiser gave it, then drive it as a
    // square wave. Everything else is left alone: the count bits come back
    // through the IO controller, not from stimulus.
    let clock_pad = file
        .design
        .io_names()
        .find(|(pad, name)| pad.input_side && *name == "clk")
        .map(|(pad, _)| pad)
        .expect("the design names its clock");
    let clock_pin = clock_pad.reserved_name(&f).expect("the clock pad exists");

    let mut stimulus = Stimulus::default();
    stimulus.set(&clock_pin, vec![false, true]);

    // Where each count bit leaves the chip, in bit order.
    let mut bits: Vec<IoPad> = Vec::new();
    for index in 0..4 {
        let wanted = format!("count[{}]", index);
        let pad = file
            .design
            .io_names()
            .find(|(pad, name)| !pad.input_side && *name == wanted)
            .map(|(pad, _)| pad)
            .unwrap_or_else(|| panic!("{} should leave on an output pad", wanted));
        bits.push(pad);
    }

    let (mut state, _) = SimState::new(&f, &file.design, &stimulus).expect("the design must settle");

    let read = |settled: &fpga_core::sim::Settled| -> u32 {
        bits.iter()
            .enumerate()
            .fold(0u32, |acc, (index, pad)| acc | u32::from(settled.pad(*pad)) << index)
    };

    // One tick of the stimulus pattern is half a clock period, so two ticks
    // make one rising edge.
    let mut seen: Vec<u32> = Vec::new();
    for _ in 0..40 {
        let settled = state.step(&f, &file.design, &stimulus).expect("must keep settling");
        seen.push(read(&settled));
    }

    // Take the value after each rising edge and check it increments, wrapping at
    // four bits.
    let counts: Vec<u32> = seen.iter().skip(1).step_by(2).copied().collect();
    assert!(counts.len() >= 16, "not enough samples: {:?}", counts);
    assert!(
        counts.windows(2).any(|w| w[1] != w[0]),
        "the count never changed, so the feedback is not reaching the adder: {:?}",
        counts
    );
    for w in counts.windows(2) {
        assert_eq!(
            w[1],
            (w[0] + 1) % 16,
            "expected the count to increment by one each rising edge: {:?}",
            counts
        );
    }
}

/// Every feedback path in a counter crosses a flip-flop, and the synthesiser
/// leaves no don't-care input reading a lane that could close a ring, so DRC
/// finds no combinational loop at all.
#[test]
fn the_counter_has_no_combinational_loop() {
    let (f, file) = counter();
    let loops: Vec<String> = drc::check(&f, &file.design)
        .into_iter()
        .filter(|i| i.severity == Severity::Error)
        .map(|i| i.message)
        .collect();
    assert!(loops.is_empty(), "{:?}", loops);
}
