//! End-to-end synthesis tests.
//!
//! These drive the library rather than the binary, so a failure points at a
//! stage instead of at a process exit code. They need Yosys; when it is
//! absent they skip with a message rather than failing, so the rest of the
//! suite still runs on a machine without the toolchain.

use sr_ga1_synth::bitstream;
use sr_ga1_synth::flow::{self, Effort, Options, Outcome};
use sr_ga1_synth::loops;
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

/// Yosys is required for anything past the source scan.
fn have_yosys() -> bool {
    Yosys::discover(None).is_ok()
}

fn options(source: &str, top: &str, tag: &str, seed: u64) -> Options {
    Options {
        sources: vec![design(source)],
        top: top.to_string(),
        fabric: repo_root().join("fabric.toml"),
        effort: Effort::Medium,
        // A ceiling, not a target: these designs normally fit on the first
        // attempt in well under a second. Generous enough that a busy machine
        // running several test binaries at once does not fail spuriously.
        time_budget: Duration::from_secs(120),
        seed,
        check_only: false,
        keep_intermediates: false,
        workdir: std::env::temp_dir().join(format!("sr-ga1-synth-test-{}", tag)),
        verbose: false,
        yosys: None,
        constraints: None,
    }
}

/// Same, with the board's loop-around wiring declared.
fn options_with_loopback(source: &str, top: &str, tag: &str, seed: u64) -> Options {
    Options { constraints: Some(design("loopback.toml")), ..options(source, top, tag, seed) }
}

fn fabric_of(_source: &str) -> sr_ga1_synth::fabric::Fabric {
    sr_ga1_synth::fabric::Fabric::load_file(&repo_root().join("fabric.toml"))
        .expect("the repository fabric.toml must load")
}

fn synthesise(source: &str, top: &str, tag: &str, seed: u64) -> Outcome {
    let mut progress = Progress::new(true);
    flow::run(&options(source, top, tag, seed), &mut progress)
        .unwrap_or_else(|e| panic!("{} did not synthesise: {}", source, e))
}

/// Every emitted configuration must be combinationally acyclic and must fit.
fn check_wellformed(outcome: &Outcome, source: &str) {
    let used: Vec<(sr_ga1_synth::rrg::Node, sr_ga1_synth::rrg::Node)> = outcome
        .routing
        .loopbacks(&outcome.rrg)
        .into_iter()
        .map(|(a, b, _)| (a, b))
        .collect();
    let cycles = loops::find_cycles(&outcome.fabric, &outcome.config, &used);
    assert!(
        cycles.is_empty(),
        "{} produced a configuration with {} combinational loop(s); the first is {}",
        source,
        cycles.len(),
        cycles[0]
    );

    let used = outcome.packed.clb_count() + outcome.routing.buffers.len();
    assert!(
        used <= outcome.fabric.clb_count(),
        "{} used {} CLBs of {}",
        source,
        used,
        outcome.fabric.clb_count()
    );
    assert!(outcome.routing.is_complete(), "{} left connections unrouted", source);

    // The bitstream must survive a round trip exactly.
    let bits = bitstream::export_bits(&outcome.fabric, &outcome.config);
    assert_eq!(bits.len(), outcome.fabric.stream_bits());
    let decoded = bitstream::import_bits(&outcome.fabric, &bits)
        .unwrap_or_else(|e| panic!("{}: emitted bitstream did not decode: {}", source, e));
    assert_eq!(
        bitstream::export_bits(&outcome.fabric, &decoded),
        bits,
        "{}: bitstream round trip changed the configuration",
        source
    );
}

#[test]
fn an_inverter_fits_in_one_clb() {
    if !have_yosys() {
        eprintln!("skipping: Yosys is not installed");
        return;
    }
    let outcome = synthesise("inv1.sv", "inv1", "inv1", 0);
    check_wellformed(&outcome, "inv1.sv");
    assert_eq!(outcome.packed.clb_count(), 1, "an inverter is one cell");
    assert_eq!(outcome.routing.buffers.len(), 0, "and needs no buffering");
}

#[test]
fn a_two_input_gate_fits_in_one_clb() {
    if !have_yosys() {
        eprintln!("skipping: Yosys is not installed");
        return;
    }
    let outcome = synthesise("and2.sv", "and2", "and2", 0);
    check_wellformed(&outcome, "and2.sv");
    assert_eq!(outcome.packed.clb_count(), 1);
}

/// The first design that exercises the clock network: a register forces a
/// clock buffer onto a vertical ring, a CSB selection, and a lane-3 enable.
#[test]
fn a_register_brings_up_the_clock_network() {
    if !have_yosys() {
        eprintln!("skipping: Yosys is not installed");
        return;
    }
    let outcome = synthesise("dff1.sv", "dff1", "dff1", 0);
    check_wellformed(&outcome, "dff1.sv");
    assert_eq!(outcome.packed.register_count(), 1);

    // Exactly one CSB must be uncoupled, or the coupling chain has no source.
    let fabric = &outcome.fabric;
    let couple = &fabric.csb_fields[fabric.csb_clock.couple_field].name;
    let uncoupled: Vec<usize> = (0..fabric.columns)
        .filter(|&col| outcome.config.get_csb(fabric, col, couple) == Some(0))
        .collect();
    assert_eq!(
        uncoupled.len(),
        1,
        "exactly one CSB should source the clock; {:?} are uncoupled",
        uncoupled
    );

    // That CSB's column must have the clock on the ring lane it taps.
    let sourcing = uncoupled[0];
    assert!(
        outcome.placement.clock_columns.values().any(|&c| c == sourcing),
        "the uncoupled CSB should be the one the clock was routed to"
    );

    // Reset takes no pad: it is a dedicated pin.
    if let Some(reset) = outcome.packed.reset {
        assert!(
            !outcome.placement.input_pins.contains_key(&reset),
            "the global reset must not consume a routable input pad"
        );
    }
}

/// Reset is dedicated, so a register without one still works and still must
/// not route anything for it.
#[test]
fn the_same_seed_gives_the_same_bitstream() {
    if !have_yosys() {
        eprintln!("skipping: Yosys is not installed");
        return;
    }
    let first = synthesise("dff1.sv", "dff1", "det-a", 7);
    let second = synthesise("dff1.sv", "dff1", "det-b", 7);
    let a = bitstream::format_text(&first.fabric, &first.config, "dff1", "fixed", true);
    let b = bitstream::format_text(&second.fabric, &second.config, "dff1", "fixed", true);
    assert_eq!(a, b, "the same input and seed must produce an identical bitstream");
}

#[test]
fn a_different_seed_is_still_a_valid_configuration() {
    if !have_yosys() {
        eprintln!("skipping: Yosys is not installed");
        return;
    }
    for seed in [1u64, 12345] {
        let outcome = synthesise("dff1.sv", "dff1", &format!("seed-{}", seed), seed);
        check_wellformed(&outcome, "dff1.sv");
    }
}

/// DDIO pads are never assigned automatically: `ddio_dir` steers a pin rather
/// than carrying a design signal.
#[test]
fn automatic_pin_assignment_avoids_ddio_pads() {
    if !have_yosys() {
        eprintln!("skipping: Yosys is not installed");
        return;
    }
    let outcome = synthesise("dff1.sv", "dff1", "ddio", 0);
    let reserved: Vec<&str> = outcome.fabric.ddio_pad_names();
    for pad in outcome
        .placement
        .input_pins
        .values()
        .chain(outcome.placement.output_pins.values())
    {
        assert!(
            !reserved.contains(&pad.as_str()),
            "{} is a DDIO pad and must not be assigned automatically",
            pad
        );
    }
}

/// A shift register is the first design with register-to-register paths, so
/// every register needs a cell that recomputes its data. Its registers capture
/// unconditionally, which needs a constant 1 on lane 3 from the CLB to the
/// left — so none of them can sit in column 0, whose lane 3 is a chip input.
#[test]
fn unconditional_registers_avoid_column_zero() {
    if !have_yosys() {
        eprintln!("skipping: Yosys is not installed");
        return;
    }
    let outcome = synthesise("shift4.sv", "shift4", "shift4", 0);
    check_wellformed(&outcome, "shift4.sv");
    assert_eq!(outcome.packed.register_count(), 4);
    for (index, cell) in outcome.packed.cells.iter().enumerate() {
        if cell.reg.as_ref().is_some_and(|r| r.enable == sr_ga1_synth::pack::Enable::Always) {
            assert!(
                outcome.placement.cells[index].col > 0,
                "{} captures unconditionally but sits in column 0",
                cell.name
            );
        }
    }
}

/// The adder is the design the carry chain exists for: one CLB per bit, in a
/// single column, least significant at the bottom.
#[test]
fn an_adder_uses_one_carry_chain_up_one_column() {
    if !have_yosys() {
        eprintln!("skipping: Yosys is not installed");
        return;
    }
    let outcome = synthesise("add4.sv", "add4", "add4", 0);
    check_wellformed(&outcome, "add4.sv");

    assert_eq!(outcome.packed.chains.len(), 1, "a 4-bit addition is one chain");
    let chain = &outcome.packed.chains[0];
    assert_eq!(chain.len(), 4, "one cell per bit");
    assert_eq!(
        outcome.packed.clb_count(),
        4,
        "the chain replaces the adder logic entirely; ordinary gates would cost far more"
    );

    let plan = outcome.carry.as_ref().expect("the fabric has a usable carry chain");
    let column = outcome.placement.cells[chain[0]].col;
    for (index, &cell) in chain.iter().enumerate() {
        let placed = &outcome.placement.cells[cell];
        assert_eq!(placed.col, column, "a chain occupies a single column");
        assert_eq!(
            placed.row,
            plan.lsb_row + index,
            "chain cells occupy consecutive ascending rows"
        );
    }
}

/// An addition wider than a column is split into segments. Each segment that
/// hands its carry on ends in a tap cell directly above its top bit, which
/// turns the carry into a net the next segment reads as its carry-in.
#[test]
fn an_addition_wider_than_a_column_is_split_into_tapped_segments() {
    if !have_yosys() {
        eprintln!("skipping: Yosys is not installed");
        return;
    }
    let outcome = synthesise("add5.sv", "add5", "add5", 0);
    check_wellformed(&outcome, "add5.sv");
    let plan = outcome.carry.as_ref().expect("the fabric has a usable carry chain");
    let is_tap = |cell: usize| outcome.packed.cells[cell].carry.as_ref().is_some_and(|l| l.tap);
    let bits: usize = outcome.packed.chains.iter().flatten().filter(|&&c| !is_tap(c)).count();
    assert_eq!(bits, 5, "one cell per bit");
    assert!(outcome.packed.chains.len() >= 2, "five bits cannot share one four-row column");
    for chain in &outcome.packed.chains {
        assert!(chain.len() <= plan.max_length, "a segment plus its tap fits one column");
        // Only the top of a segment can be a tap.
        for &cell in &chain[..chain.len() - 1] {
            assert!(!is_tap(cell), "a tap sits above its segment's top bit");
        }
        // Consecutive rows of one column; only a segment fed by the tied-off
        // edge has to start at the edge row.
        let (column, base) = (outcome.placement.cells[chain[0]].col, outcome.placement.cells[chain[0]].row);
        let routed_in = outcome.packed.cells[chain[0]].carry.as_ref().is_some_and(|l| l.carry_in.is_some());
        if !routed_in {
            assert_eq!(base, plan.lsb_row);
        }
        for (index, &cell) in chain.iter().enumerate() {
            assert_eq!(outcome.placement.cells[cell].col, column);
            assert_eq!(outcome.placement.cells[cell].row, base + index);
        }
    }
    // The final carry is the sum's top bit, so the last segment is tapped too.
    assert!(is_tap(*outcome.packed.chains.last().unwrap().last().unwrap()));
}

/// A carry-in port and a carry-out port: the carry-in arrives on a lane at the
/// least significant bit, and the carry-out comes from a tap.
#[test]
fn an_adder_with_carry_in_and_carry_out_fits() {
    if !have_yosys() {
        eprintln!("skipping: Yosys is not installed");
        return;
    }
    let outcome = synthesise("add4c.sv", "add4c", "add4c", 0);
    check_wellformed(&outcome, "add4c.sv");
    let first = outcome.packed.chains[0][0];
    let link = outcome.packed.cells[first].carry.as_ref().expect("a chain cell");
    assert!(link.carry_in.is_some(), "cin is routed into the least significant bit");
    assert!(outcome.packed.outputs.iter().any(|p| p.name == "cout"), "cout is an output");
}

/// An undeclared identifier is an error, not an implicit wire nothing drives —
/// which would otherwise optimise the whole design away without a word.
#[test]
fn an_undeclared_identifier_is_an_error() {
    if !have_yosys() {
        eprintln!("skipping: Yosys is not installed");
        return;
    }
    let dir = repo_root().join("target").join("synth-tests").join("undeclared");
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let source = dir.join("typo.sv");
    std::fs::write(
        &source,
        "module typo (input logic [3:0] a, input logic c_in, output logic [3:0] y);
    assign y = a + cin;
endmodule
",
    )
    .expect("write source");
    let mut opts = options("add4.sv", "typo", "undeclared", 0);
    opts.sources = vec![source];
    let mut progress = Progress::new(true);
    let text = match flow::run(&opts, &mut progress) {
        Err(e) => format!("{}", e),
        Ok(_) => panic!("`cin` is not declared"),
    };
    assert!(text.contains("cin"), "the error should name the identifier: {}", text);
}

/// A register that depends on its own value needs its output back at its own
/// cell. The vertical ring cannot do it, but the IO controller can: the value
/// leaves on one of the chip outputs it reads back, and re-enters on the left
/// edge — no board wiring at all.
#[test]
fn a_self_dependent_register_loops_back_on_chip() {
    if !have_yosys() {
        eprintln!("skipping: Yosys is not installed");
        return;
    }
    let outcome = synthesise("toggle.sv", "toggle", "toggle", 0);
    check_wellformed(&outcome, "toggle.sv");
    assert_eq!(outcome.packed.register_count(), 1);
    assert!(sr_ga1_synth::report::loopback_wiring(&outcome).is_empty(), "no board wiring");
    let loops = sr_ga1_synth::report::on_chip_loops(&outcome);
    assert!(!loops.is_empty(), "the feedback returns through the IO controller");
    let readable: Vec<String> = outcome
        .fabric
        .loopable_outputs()
        .iter()
        .map(|&o| outcome.fabric.io_outputs[o].clone())
        .collect();
    for (pad, ..) in &loops {
        assert!(readable.contains(pad), "{} cannot be read back", pad);
    }
}

/// The capability that decides the case above, read straight off the fabric.
#[test]
fn register_feedback_is_a_fabric_capability() {
    let f = fabric_of("add4.sv");
    // Inside the fabric the two vertical lane pairs never mix, so a register
    // cannot reach its own cell through the ring...
    let op = f.vert_lanes_for(sr_ga1_synth::fabric::Source::Op);
    let reg = f.vert_lanes_for(sr_ga1_synth::fabric::Source::Reg);
    assert!(op.iter().all(|l| !reg.contains(l)), "_op and _reg lanes are disjoint");
    // ...but the IO controller reads output_0..3 back onto the left edge.
    assert_eq!(f.loopable_outputs().len(), 4);
    assert!(f.register_feedback_possible());
}

/// Board wiring still works as an extra resource, and anything it uses comes
/// from the pool the constraints offered.
#[test]
fn loop_around_wiring_is_an_extra_resource() {
    if !have_yosys() {
        eprintln!("skipping: Yosys is not installed");
        return;
    }
    let mut progress = Progress::new(true);
    let outcome = flow::run(
        &options_with_loopback("toggle.sv", "toggle", "toggle-loop", 0),
        &mut progress,
    )
    .expect("a toggle flip-flop should fit with a pool declared");
    check_wellformed(&outcome, "toggle.sv");
    assert_eq!(outcome.packed.register_count(), 1);

    let wiring = sr_ga1_synth::report::loopback_wiring(&outcome);
    // Every link must name pads that were actually offered in the pool.
    let pool = &outcome.constraints.loopback;
    for link in &wiring {
        assert!(
            pool.outputs.iter().any(|p| p.name == link.from),
            "{} was not offered as a loop output",
            link.from
        );
        assert!(
            pool.inputs.iter().any(|p| p.name == link.to),
            "{} was not offered as a loop input",
            link.to
        );
    }
}

/// The counter is the design that motivated all of this: four carry-chain bits,
/// each feeding its own next value back — through the IO controller, with no
/// board wiring.
#[test]
fn a_counter_fits_on_chip() {
    if !have_yosys() {
        eprintln!("skipping: Yosys is not installed");
        return;
    }
    let outcome = synthesise("count4.sv", "count4", "count4", 0);
    check_wellformed(&outcome, "count4.sv");

    assert_eq!(outcome.packed.register_count(), 4);
    assert_eq!(outcome.packed.chains.len(), 1, "the increment uses the carry chain");
    assert_eq!(outcome.packed.chains[0].len(), 4);
    assert!(sr_ga1_synth::report::loopback_wiring(&outcome).is_empty(), "no board wiring");
    // One loop per bit, since each count bit feeds its own adder cell.
    let loops = sr_ga1_synth::report::on_chip_loops(&outcome);
    let nets: std::collections::BTreeSet<&str> = loops.iter().map(|(.., net)| net.as_str()).collect();
    assert_eq!(nets.len(), 4, "each count bit needs its own way back: {:?}", loops);
}

/// With a pool declared as well, the counter still fits and board wiring never
/// collides with a design input.
#[test]
fn a_counter_fits_with_a_pool_declared() {
    if !have_yosys() {
        eprintln!("skipping: Yosys is not installed");
        return;
    }
    let mut progress = Progress::new(true);
    let outcome =
        flow::run(&options_with_loopback("count4.sv", "count4", "count4-loop", 0), &mut progress)
            .expect("a 4-bit counter should fit with loop-around wiring available");
    check_wellformed(&outcome, "count4.sv");
    assert_eq!(outcome.packed.register_count(), 4);
    let wiring = sr_ga1_synth::report::loopback_wiring(&outcome);

    // An input pad carrying a board wire must never also be a design input.
    for link in &wiring {
        assert!(
            !outcome.placement.input_pins.values().any(|pad| *pad == link.to),
            "{} is driven by a board wire and cannot also be a design input",
            link.to
        );
    }
}

/// The wiring is physical, so it must not change between identical runs.
#[test]
fn the_chosen_wiring_is_reproducible() {
    if !have_yosys() {
        eprintln!("skipping: Yosys is not installed");
        return;
    }
    let run = |tag: &str| {
        let mut progress = Progress::new(true);
        flow::run(&options_with_loopback("toggle.sv", "toggle", tag, 11), &mut progress)
            .expect("toggle fits with loops")
    };
    let first = run("wire-a");
    let second = run("wire-b");
    assert_eq!(
        sr_ga1_synth::report::loopback_wiring(&first),
        sr_ga1_synth::report::loopback_wiring(&second),
        "the same seed must ask for the same physical connections"
    );
    assert_eq!(
        sr_ga1_synth::report::on_chip_loops(&first),
        sr_ga1_synth::report::on_chip_loops(&second)
    );
    assert_eq!(
        bitstream::format_text(&first.fabric, &first.config, "toggle", "fixed", true),
        bitstream::format_text(&second.fabric, &second.config, "toggle", "fixed", true)
    );
}

/// Everything the constraints file can pin down, honoured at once.
#[test]
fn constraints_are_honoured() {
    if !have_yosys() {
        eprintln!("skipping: Yosys is not installed");
        return;
    }
    let mut progress = Progress::new(true);
    let mut opts = options("count4.sv", "count4", "locked", 0);
    opts.constraints = Some(design("locked.toml"));
    let outcome =
        flow::run(&opts, &mut progress).expect("the constrained design should still fit");
    check_wellformed(&outcome, "count4.sv");

    let constraints = &outcome.constraints;

    // Pin locks.
    for (signal, pad) in &constraints.pins {
        let net = outcome
            .packed
            .inputs
            .iter()
            .chain(&outcome.packed.outputs)
            .find(|p| p.name == *signal)
            .map(|p| p.net)
            .unwrap_or_else(|| panic!("{} should be a port", signal));
        let assigned = outcome
            .placement
            .input_pins
            .get(&net)
            .or_else(|| outcome.placement.output_pins.get(&net))
            .unwrap_or_else(|| panic!("{} should have a pad", signal));
        assert_eq!(assigned, &pad.name, "{} was not placed on the pad it was locked to", signal);
    }

    // Instance placement, and the carry chain following its pinned member.
    for lock in &constraints.placement {
        let index = outcome
            .packed
            .cells
            .iter()
            .position(|c| c.name == lock.name)
            .unwrap_or_else(|| panic!("{} should be a cell", lock.name));
        let placed = &outcome.placement.cells[index];
        assert_eq!(
            (placed.col, placed.row),
            (lock.col, lock.row),
            "{} was not placed where it was pinned",
            lock.name
        );
    }

    // Clock assignment: the named column sources the domain, and is the one
    // uncoupled CSB.
    let fabric = &outcome.fabric;
    let couple = &fabric.csb_fields[fabric.csb_clock.couple_field].name;
    for (net, column) in &constraints.clocks {
        let clock = outcome
            .packed
            .clocks
            .iter()
            .find(|c| outcome.packed.names.get(**c) == Some(net.as_str()))
            .unwrap_or_else(|| panic!("{} should be a clock", net));
        assert_eq!(
            outcome.placement.clock_columns.get(clock),
            Some(column),
            "{} was not sourced from the column it was assigned to",
            net
        );
        assert_eq!(
            outcome.config.get_csb(fabric, *column, couple),
            Some(0),
            "the sourcing CSB must be uncoupled"
        );
    }
}

/// A constraint that names something the design does not have is an error, not
/// a shrug: silently ignoring it looks exactly like honouring it.
#[test]
fn a_constraint_naming_an_unknown_signal_is_refused() {
    if !have_yosys() {
        eprintln!("skipping: Yosys is not installed");
        return;
    }
    let dir = std::env::temp_dir().join("sr-ga1-bad-constraints");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("bad.toml");
    std::fs::write(&path, "[pins]
not_a_port = \"input_0\"
").expect("write");

    let mut progress = Progress::new(true);
    let mut opts = options("dff1.sv", "dff1", "bad-constraints", 0);
    opts.constraints = Some(path);
    let text = match flow::run(&opts, &mut progress) {
        Err(e) => format!("{}", e),
        Ok(_) => panic!("a pin lock on a signal the design lacks must be refused"),
    };
    assert!(
        text.contains("not_a_port") && text.contains("not a port"),
        "the message should name the signal: {}",
        text
    );
}

/// With more than one clock, the CSB coupling chain partitions the columns and a
/// register has to sit in a column its own clock reaches. Getting this wrong
/// clocks a register from the wrong domain, which no routing can fix.
#[test]
fn clock_domains_partition_the_columns() {
    if !have_yosys() {
        eprintln!("skipping: Yosys is not installed");
        return;
    }
    let outcome = synthesise("twoclk.sv", "twoclk", "twoclk", 0);
    check_wellformed(&outcome, "twoclk.sv");
    assert_eq!(outcome.packed.clocks.len(), 2, "two clocks means two domains");

    let fabric = &outcome.fabric;
    let domains = outcome.placement.clock_domains(fabric.columns);

    // Each domain has exactly one uncoupled CSB sourcing it.
    let couple = &fabric.csb_fields[fabric.csb_clock.couple_field].name;
    let uncoupled: Vec<usize> = (0..fabric.columns)
        .filter(|&col| outcome.config.get_csb(fabric, col, couple) == Some(0))
        .collect();
    assert_eq!(uncoupled.len(), 2, "one source per domain; uncoupled: {:?}", uncoupled);
    for clock in &outcome.packed.clocks {
        let column = outcome.placement.clock_columns[clock];
        assert!(uncoupled.contains(&column), "column {} should source a domain", column);
    }

    // And every register sits in a column its own clock reaches.
    for (index, cell) in outcome.packed.cells.iter().enumerate() {
        let Some(reg) = &cell.reg else { continue };
        let column = outcome.placement.cells[index].col;
        assert_eq!(
            domains[column],
            Some(reg.clock),
            "{} is in column {}, which belongs to another clock domain",
            cell.name,
            column
        );
    }
}

/// Names are emitted pinned, so they have to survive the visual programmer's
/// collision rules: unique block names, and no derived `_op`/`_reg`/`_carry` net
/// colliding with anything either. Several cells legitimately want the same
/// name, so this is not automatic.
#[test]
fn emitted_names_are_unique_and_collision_free() {
    if !have_yosys() {
        eprintln!("skipping: Yosys is not installed");
        return;
    }
    let mut progress = Progress::new(true);
    let mut opts = options("count4.sv", "count4", "names", 0);
    opts.constraints = Some(design("loopback.toml"));
    let outcome = flow::run(&opts, &mut progress).expect("count4 should fit");
    let fabric = &outcome.fabric;

    let mut taken: Vec<String> = Vec::new();
    // The fabric's own reserved names are already in use.
    taken.push(fabric.naming.constant_zero.clone());
    taken.push(fabric.naming.constant_one.clone());
    taken.push(fabric.naming.io.clone());
    taken.extend(fabric.io_inputs.iter().cloned());
    taken.extend(fabric.io_outputs.iter().cloned());

    for (block, name) in outcome.config.names() {
        assert!(
            !taken.contains(name),
            "{} is named \"{}\", which is already in use",
            block,
            name
        );
        taken.push(name.clone());
        // And the nets it implies must not collide either.
        for suffix in [
            &fabric.naming.suffix_op,
            &fabric.naming.suffix_reg,
            &fabric.naming.suffix_carry,
        ] {
            let derived = format!("{}{}", name, suffix);
            assert!(
                !taken.contains(&derived),
                "{} would imply net \"{}\", which collides with an existing name",
                block,
                derived
            );
            taken.push(derived);
        }
    }

    // Every placed cell and every router buffer should be named, so nothing
    // meaningful shows up as a bare grid position.
    let named = outcome.config.names().count();
    let expected = outcome.packed.clb_count() + outcome.routing.buffers.len();
    assert!(
        named >= expected,
        "{} block(s) named but {} CLBs are in use",
        named,
        expected
    );

    // The design's own IO names are carried over, so a pad reads as the signal.
    let io = sr_ga1_synth::report::io_names(&outcome);
    for port in outcome.packed.inputs.iter().chain(&outcome.packed.outputs) {
        if outcome.packed.reset == Some(port.net) {
            continue;
        }
        assert!(
            io.values().any(|name| *name == port.name),
            "{} should appear in the emitted IO names",
            port.name
        );
    }
}

/// `--check` must work without placing or routing, and must be honest about
/// what it is not counting.
#[test]
fn check_mode_reports_cells_without_routing() {
    if !have_yosys() {
        eprintln!("skipping: Yosys is not installed");
        return;
    }
    let mut progress = Progress::new(true);
    let mut opts = options("dff1.sv", "dff1", "check", 0);
    opts.check_only = true;
    let check = flow::front(&opts, &mut progress).expect("check mode should succeed");
    assert_eq!(check.packed.register_count(), 1);
    assert!(check.packed.clb_count() <= check.fabric.clb_count());
}
