//! The end-to-end check: a counter produced by `sr-ga1-synth` must actually
//! count when this crate simulates it.
//!
//! `tests/data/count4.json` is a real synthesiser output for
//! `crates/sr-ga1-synth/tests/designs/count4.sv` — `count <= count + 1` — using
//! the carry chain, a derived column clock, lane-3 enables, and four
//! loop-around board wires. Before board wiring was modelled here, this design
//! loaded and simulated as a counter stuck at zero, because each `count` bit's
//! feedback arrived as stimulus rather than from the output driving it.
//!
//! Regenerate with:
//!
//! ```text
//! cargo run -p sr-ga1-synth -- crates/sr-ga1-synth/tests/designs/count4.sv \
//!     --top count4 --constraints crates/sr-ga1-synth/tests/designs/loopback.toml \
//!     -o crates/fpga-core/tests/data/count4.json --seed 0
//! ```

use fpga_core::config::IoPad;
use fpga_core::designfile::load_design;
use fpga_core::drc::{self, Severity};
use fpga_core::fabric::Fabric;
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
fn the_synthesised_design_carries_its_names_and_wiring() {
    let (f, file) = counter();

    // Four loops, one per count bit: each bit feeds its own adder cell, which is
    // the whole reason this design needs board wiring.
    assert_eq!(file.design.loopback().len(), 4, "one loop per count bit");
    for link in file.design.loopback() {
        assert!(!link.from.input_side, "a loop starts at an output");
        assert!(link.to.input_side, "and ends at an input");
    }

    // And the design's own names, so the canvas reads `clk` rather than a pin id.
    let named: Vec<String> = file
        .design
        .io_names()
        .map(|(pad, name)| {
            format!("{} on {}", name, pad.reserved_name(&f).unwrap_or_default())
        })
        .collect();
    assert!(
        named.iter().any(|n| n.starts_with("clk on ")),
        "the clock should be named: {:?}",
        named
    );
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
    // square wave. Everything else is left alone: the count bits arrive through
    // the board wiring, not from stimulus.
    let clock_pad = file
        .design
        .io_names()
        .find(|(_, name)| *name == "clk")
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

    let (mut state, _) =
        SimState::new(&f, &file.design, &stimulus).expect("the design must settle");

    let read = |settled: &fpga_core::sim::Settled| -> u32 {
        bits.iter().enumerate().fold(0u32, |acc, (index, pad)| {
            acc | u32::from(settled.horz_edge(pad.row, pad.lane)) << index
        })
    };

    // Release reset, then step and watch it advance. One tick of the stimulus
    // pattern is half a clock period, so two ticks make one rising edge.
    state.reset = false;
    let mut seen: Vec<u32> = Vec::new();
    for _ in 0..24 {
        let settled = state.step(&f, &file.design, &stimulus).expect("must keep settling");
        seen.push(read(&settled));
    }

    // Take the value after each rising edge and check it increments, wrapping at
    // four bits.
    let mut counts: Vec<u32> = Vec::new();
    for (index, value) in seen.iter().enumerate() {
        if index % 2 == 1 {
            counts.push(*value);
        }
    }
    assert!(counts.len() >= 8, "not enough samples: {:?}", counts);
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

/// Board wiring must not *add* a cycle. The flip-flops break every path that
/// runs through a loop, so wiring the counter up is not what makes it cyclic.
///
/// This deliberately compares against the same design with the wiring removed
/// rather than asserting DRC is silent: this configuration is reported as cyclic
/// either way, which is a pre-existing disagreement between this crate's DRC and
/// the synthesiser's own loop checker, not something the wiring causes.
#[test]
fn board_wiring_adds_no_combinational_loop() {
    let (f, file) = counter();
    let mut without = file.design.clone();
    for link in file.design.loopback().to_vec() {
        without.clear_loopback(link.to);
    }

    let cycles = |design: &fpga_core::config::Design| -> usize {
        drc::check(&f, design)
            .iter()
            .filter(|i| i.severity == Severity::Error && i.message.contains("combinational loop"))
            .count()
    };
    assert_eq!(
        cycles(&file.design),
        cycles(&without),
        "adding the board wiring must not introduce a cycle: every path through a loop          crosses a flip-flop"
    );
}
