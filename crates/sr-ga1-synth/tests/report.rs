//! Tests for the synthesis report.
//!
//! The report is the tool's only account of what it did, so two properties are
//! worth pinning: every utilisation figure must be measured rather than
//! guessed (and so must never exceed capacity or contradict a sibling figure),
//! and the text must stay a report. Earlier versions mixed in advice about how
//! to write the source — "a chain runs up a single column, at most 4 cells", "a
//! board wire is the only way to move a signal leftward" — which reads as
//! guidance about the design rather than a statement about the bitstream in
//! hand. `no_design_stage_advice` keeps that out.

use sr_ga1_synth::flow::{self, Effort, Options, Outcome};
use sr_ga1_synth::place::Target;
use sr_ga1_synth::progress::Progress;
use sr_ga1_synth::report;
use sr_ga1_synth::yosys::Yosys;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn design(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("designs").join(name)
}

fn have_yosys() -> bool {
    Yosys::discover(None).is_ok()
}

fn synthesise(source: &str, top: &str, tag: &str, constraints: Option<&str>) -> Outcome {
    let options = Options {
        sources: vec![design(source)],
        top: top.to_string(),
        fabric: repo_root().join("fabric.toml"),
        effort: Effort::Medium,
        time_budget: Duration::from_secs(120),
        seed: 0,
        check_only: false,
        keep_intermediates: false,
        workdir: std::env::temp_dir().join(format!("sr-ga1-synth-report-{}", tag)),
        verbose: false,
        yosys: None,
        constraints: constraints.map(design),
    };
    let mut progress = Progress::new(true);
    flow::run(&options, &mut progress)
        .unwrap_or_else(|e| panic!("{} did not synthesise: {}", source, e))
}

fn rendered(outcome: &Outcome) -> String {
    let target = Target::with_carry(
        &outcome.fabric,
        &outcome.library,
        outcome.carry.as_ref(),
        &outcome.constraints,
    );
    report::render(outcome, &target)
}

/// No figure may exceed its capacity, and the parts must add up to the whole.
#[test]
fn utilisation_figures_are_consistent() {
    if !have_yosys() {
        eprintln!("skipping: Yosys not found");
        return;
    }
    let outcome = synthesise("count4.sv", "count4", "usage", Some("loopback.toml"));
    let u = report::usage(&outcome);

    for (label, used, total) in [
        ("CLBs", u.clbs_used, u.clbs),
        ("registers", u.registers, u.clbs),
        ("carry cells", u.carry_cells, u.clbs),
        ("clocks", u.clocks, u.csbs),
        ("horizontal segments", u.horz_used, u.horz),
        ("vertical segments", u.vert_used, u.vert),
        ("input pads", u.in_pads_used, u.in_pads),
        ("output pads", u.out_pads_used, u.out_pads),
        ("loop wires", u.loops_used, u.loops_pool),
    ] {
        assert!(used <= total, "{}: {} used exceeds capacity {}", label, used, total);
        assert!(total > 0, "{}: capacity should not be zero on this fabric", label);
    }
    assert_eq!(u.clbs_used, u.logic + u.buffers, "CLB total must be logic plus buffers");

    // The per-lane breakdowns must sum to the totals they are a breakdown of,
    // or the grid and the table would be telling different stories.
    assert_eq!(
        u.horz_by_lane.values().sum::<usize>(),
        u.horz_used,
        "the per-row grid must account for every horizontal segment"
    );
    assert_eq!(
        u.vert_by_lane.values().sum::<usize>(),
        u.vert_used,
        "the per-column grid must account for every vertical segment"
    );

    // count4 is the design that needs board wiring, so it exercises the pads
    // that pin assignment never sees.
    assert!(u.loops_used > 0, "count4 should need loop-around wiring");
    assert!(u.registers > 0 && u.carry_cells > 0, "count4 has registers and a carry chain");
}

/// Every resource line carries an absolute figure *and* a percentage.
#[test]
fn every_resource_reports_absolute_and_percentage() {
    if !have_yosys() {
        eprintln!("skipping: Yosys not found");
        return;
    }
    let outcome = synthesise("count4.sv", "count4", "pct", Some("loopback.toml"));
    let text = rendered(&outcome);
    let table: Vec<&str> = text
        .lines()
        .skip_while(|l| !l.starts_with("utilisation"))
        .take_while(|l| !l.starts_with("cell types"))
        .collect();

    for resource in [
        "CLBs",
        "mapped logic",
        "routing buffers",
        "registers",
        "carry chain cells",
        "clock sources (CSB)",
        "horizontal segments",
        "vertical segments",
        "input pads",
        "output pads",
        "loop-around wires",
    ] {
        let line = table
            .iter()
            .find(|l| l.trim_start().starts_with(resource))
            .unwrap_or_else(|| panic!("no utilisation line for {}: {:?}", resource, table));
        assert!(line.contains('%'), "{} should carry a percentage: {}", resource, line);
        let figures: Vec<&str> =
            line.split_whitespace().filter(|w| w.parse::<usize>().is_ok()).collect();
        assert!(
            figures.len() >= 2,
            "{} should carry a used and a total figure: {}",
            resource,
            line
        );
    }
}

/// Every IO port must be named, with its pad, its physical position and the
/// CLB that produces or consumes it.
#[test]
fn every_io_pin_reports_its_pad_and_placement() {
    if !have_yosys() {
        eprintln!("skipping: Yosys not found");
        return;
    }
    let outcome = synthesise("count4.sv", "count4", "pins", Some("loopback.toml"));
    let text = rendered(&outcome);
    let section: Vec<&str> = text
        .lines()
        .skip_while(|l| !l.starts_with("IO pin assignment"))
        .take_while(|l| !l.starts_with("loop-around wiring"))
        .collect();

    for port in outcome.packed.inputs.iter().chain(outcome.packed.outputs.iter()) {
        let line = section
            .iter()
            .find(|l| l.split_whitespace().nth(1) == Some(port.name.as_str()))
            .unwrap_or_else(|| panic!("{} is missing from the IO section: {:?}", port.name, section));
        // The reset is the one pin with no pad: it is fanned out in hardware.
        if outcome.packed.reset == Some(port.net) {
            assert!(line.contains("reset"), "the reset pin should say so: {}", line);
            continue;
        }
        assert!(
            line.contains("row ") && line.contains("lane "),
            "{} should report a physical position: {}",
            port.name,
            line
        );
        assert!(
            line.contains("CLB(") || line.contains("CSB"),
            "{} should report what produces or consumes it: {}",
            port.name,
            line
        );
    }

    // And the inventory of what is left, so the cost of the next pin is visible.
    assert!(
        section.iter().any(|l| l.contains("unassigned input pads")),
        "the free input pads should be listed"
    );
    assert!(
        section.iter().any(|l| l.contains("unassigned output pads")),
        "the free output pads should be listed"
    );
}

/// The loop-around section must name both ends *and* their positions, since
/// someone has to solder the wire.
#[test]
fn loop_wiring_reports_both_ends_with_positions() {
    if !have_yosys() {
        eprintln!("skipping: Yosys not found");
        return;
    }
    let outcome = synthesise("count4.sv", "count4", "loops", Some("loopback.toml"));
    let text = rendered(&outcome);
    for link in report::loopback_wiring(&outcome) {
        let line = text
            .lines()
            .find(|l| l.contains("wire ") && l.contains(&link.from) && l.contains(&link.to))
            .unwrap_or_else(|| panic!("{} -> {} is missing from the report", link.from, link.to));
        assert_eq!(
            line.matches("row ").count(),
            2,
            "both ends of the wire need a position: {}",
            line
        );
    }
}

/// The report states results. Advice about how to write the source belongs in
/// the docs, not in an account of a finished bitstream.
#[test]
fn no_design_stage_advice() {
    if !have_yosys() {
        eprintln!("skipping: Yosys not found");
        return;
    }
    // Both shapes: one that uses the carry chain and board wiring, and a purely
    // combinational one that exercises every "none" branch.
    for (source, top, tag, constraints) in [
        ("count4.sv", "count4", "advice-a", Some("loopback.toml")),
        ("inv1.sv", "inv1", "advice-b", None),
    ] {
        let outcome = synthesise(source, top, tag, constraints);
        let text = rendered(&outcome).to_lowercase();
        for phrase in [
            // Design guidance that earlier versions printed.
            "a chain runs up a single column",
            "cannot extend",
            "a wider addition",
            "is the only way to move",
            "the only way to move a signal leftward",
            // Pointers to other tools, which are not results either.
            "the visual programmer",
            "docs/",
            // Generic advice-giving.
            "consider ",
            "you should",
            "you can ",
            "try ",
            "instead of",
        ] {
            assert!(
                !text.contains(phrase),
                "{}: the report should not contain design-stage advice {:?}",
                source,
                phrase
            );
        }
    }
}

/// A section heading's underline must match it, which is easy to break by
/// editing a heading that now carries figures in it.
#[test]
fn section_underlines_match_their_headings() {
    if !have_yosys() {
        eprintln!("skipping: Yosys not found");
        return;
    }
    let outcome = synthesise("count4.sv", "count4", "rules", Some("loopback.toml"));
    let text = rendered(&outcome);
    let lines: Vec<&str> = text.lines().collect();
    let mut checked = 0;
    for pair in lines.windows(2) {
        let (heading, rule) = (pair[0], pair[1]);
        if rule.len() < 4 || !rule.chars().all(|c| c == '-') {
            continue;
        }
        assert_eq!(
            rule.chars().count(),
            heading.chars().count(),
            "underline does not match heading {:?}",
            heading
        );
        checked += 1;
    }
    assert!(checked >= 8, "expected to check several headings, saw {}", checked);
}
