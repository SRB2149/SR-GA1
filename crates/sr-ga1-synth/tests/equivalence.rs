//! Equivalence checking against the source design, under Verilator.
//!
//! These are the tests that would catch a wrong bitstream. Everything else
//! checks that the flow is self-consistent; this checks that what it produces
//! actually computes the design. The configuration is decoded back out of the
//! bitstream first, so the encoding is covered too.
//!
//! They need both Yosys and Verilator, and skip with a message when either is
//! missing rather than failing.

use sr_ga1_synth::equiv::Verilator;
use sr_ga1_synth::equivrun;
use sr_ga1_synth::flow::{self, Effort, Options, Outcome};
use sr_ga1_synth::progress::Progress;
use sr_ga1_synth::yosys::Yosys;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn design(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("designs").join(name)
}

fn tools() -> Option<Verilator> {
    if Yosys::discover(None).is_err() {
        eprintln!("skipping: Yosys is not installed");
        return None;
    }
    match Verilator::discover(None) {
        Ok(tool) => Some(tool),
        Err(_) => {
            eprintln!("skipping: Verilator is not installed");
            None
        }
    }
}

fn options(source: &str, top: &str, tag: &str, constraints: Option<&str>) -> Options {
    Options {
        sources: vec![design(source)],
        top: top.to_string(),
        fabric: repo_root().join("fabric.toml"),
        effort: Effort::Medium,
        time_budget: Duration::from_secs(40),
        seed: 0,
        check_only: false,
        keep_intermediates: false,
        workdir: std::env::temp_dir().join(format!("sr-ga1-equiv-{}", tag)),
        verbose: false,
        yosys: None,
        constraints: constraints.map(design),
    }
}

/// Synthesise, then prove the result computes the design.
fn check(source: &str, top: &str, tag: &str, constraints: Option<&str>, cycles: usize) -> Outcome {
    let Some(verilator) = tools() else { return dummy() };
    let opts = options(source, top, tag, constraints);
    let mut progress = Progress::new(true);
    let outcome = flow::run(&opts, &mut progress)
        .unwrap_or_else(|e| panic!("{} did not synthesise: {}", source, e));

    let result = equivrun::check(&outcome, &opts.sources, &opts.workdir, cycles, &verilator)
        .unwrap_or_else(|e| panic!("{} is not equivalent to its source:\n{}", source, e));
    assert_eq!(result.cycles, cycles);
    assert!(!result.compared.is_empty(), "the check must compare at least one output");
    outcome
}

/// Used only when the toolchain is absent and the test has already reported a
/// skip; nothing downstream inspects it.
fn dummy() -> Outcome {
    // Reaching here means `tools()` returned None and the caller returns early
    // in practice. Constructing a real Outcome is not possible without running
    // the flow, so this marks the path as unreachable.
    unreachable!("equivalence tests return before this when the toolchain is missing")
}

#[test]
fn combinational_logic_matches_its_source() {
    if tools().is_none() {
        return;
    }
    check("and2.sv", "and2", "and2", None, 400);
    check("inv1.sv", "inv1", "inv1", None, 400);
}

/// A register exercises the derived column clock and the lane-3 enable, neither
/// of which any purely combinational design touches.
#[test]
fn a_register_matches_its_source() {
    if tools().is_none() {
        return;
    }
    check("dff1.sv", "dff1", "dff1", None, 800);
}

#[test]
fn a_shift_register_matches_its_source() {
    if tools().is_none() {
        return;
    }
    check("shift4.sv", "shift4", "shift4", None, 800);
}

/// The adder is the one design whose result comes off the dedicated carry
/// chain rather than the operation core alone.
#[test]
fn a_carry_chain_adder_matches_its_source() {
    if tools().is_none() {
        return;
    }
    let outcome = check("add4.sv", "add4", "add4", None, 800);
    assert_eq!(outcome.packed.chains.len(), 1, "the adder should use the carry chain");
}

/// The counter needs everything at once: the carry chain, a derived clock,
/// lane-3 enables, and feedback through board wiring. If the loop-around
/// modelling were wrong, this is where it would show.
#[test]
fn a_counter_through_loop_around_wiring_matches_its_source() {
    if tools().is_none() {
        return;
    }
    let outcome =
        check("count4.sv", "count4", "count4", Some("loopback.toml"), 800);
    assert_eq!(outcome.packed.register_count(), 4);
    assert!(
        !sr_ga1_synth::report::loopback_wiring(&outcome).is_empty(),
        "this design is only buildable through the board wiring, so it must use some"
    );
}

/// Two clock domains: the columns are partitioned between them and each
/// register has to be clocked by its own domain. Both clocks are driven from the
/// same testbench clock, so this verifies each domain's logic rather than
/// cross-domain timing - which vector comparison cannot establish anyway.
#[test]
fn two_clock_domains_match_their_source() {
    if tools().is_none() {
        return;
    }
    let outcome = check("twoclk.sv", "twoclk", "twoclk", None, 600);
    assert_eq!(outcome.packed.clocks.len(), 2);
}

#[test]
fn a_self_dependent_register_through_board_wiring_matches_its_source() {
    if tools().is_none() {
        return;
    }
    check("toggle.sv", "toggle", "toggle", Some("loopback.toml"), 800);
}
