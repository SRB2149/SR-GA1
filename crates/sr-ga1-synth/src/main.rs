//! `sr-ga1-synth` — compile SystemVerilog onto the SR-GA1 fabric.

use clap::Parser;
use sr_ga1_synth::bitstream;
use sr_ga1_synth::designjson::{save_design, DesignFile};
use sr_ga1_synth::flow::{self, Effort, Options};
use sr_ga1_synth::place::Target;
use sr_ga1_synth::progress::Progress;
use sr_ga1_synth::report;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "sr-ga1-synth",
    about = "Compile small SystemVerilog designs onto the SR-GA1 FPGA fabric",
    long_about = None,
)]
struct Cli {
    /// SystemVerilog source files.
    #[arg(required = true, value_name = "FILE")]
    sources: Vec<PathBuf>,

    /// Top module name.
    #[arg(long, value_name = "MODULE")]
    top: String,

    /// Fabric description shared with the visual programmer.
    #[arg(long, default_value = "fabric.toml", value_name = "FILE")]
    fabric: PathBuf,

    /// Design file for the visual programmer.
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    output: Option<PathBuf>,

    /// Configuration bitstream.
    #[arg(long, value_name = "FILE")]
    bitstream: Option<PathBuf>,

    /// Synthesis report.
    #[arg(long, value_name = "FILE")]
    report: Option<PathBuf>,

    /// Elaborate and map only, reporting cell count against capacity.
    #[arg(long)]
    check: bool,

    /// How hard to search for a placement that routes.
    #[arg(long, default_value = "high", value_name = "LEVEL")]
    effort: String,

    /// Seconds the outer search loop may spend.
    #[arg(long = "time-budget", default_value = "60", value_name = "SECONDS")]
    time_budget: f64,

    /// Seed, for reproducible runs.
    #[arg(long, default_value = "0", value_name = "N")]
    seed: u64,

    /// Pin locks, DDIO declarations, instance placement and clock assignment.
    #[arg(long, value_name = "FILE")]
    constraints: Option<PathBuf>,

    /// Keep the Yosys netlists, genlib and intermediate dumps.
    #[arg(long = "keep-intermediates")]
    keep_intermediates: bool,

    /// Bitstream with no comment header.
    #[arg(long)]
    raw: bool,

    /// Directory for intermediates.
    #[arg(long = "work-dir", default_value = "synth-work", value_name = "DIR")]
    work_dir: PathBuf,

    /// Path to the Yosys executable, if it is not on PATH.
    #[arg(long, value_name = "PATH")]
    yosys: Option<PathBuf>,

    /// Check the emitted bitstream against the source under Verilator.
    #[arg(long)]
    equiv: bool,

    /// Cycles of random vectors the equivalence check runs.
    #[arg(long = "equiv-cycles", default_value = "2000", value_name = "N")]
    equiv_cycles: usize,

    /// Path to Verilator, if it is not on PATH.
    #[arg(long, value_name = "PATH")]
    verilator: Option<PathBuf>,

    /// Per-stage detail and timings.
    #[arg(short = 'v', long)]
    verbose: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let effort: Effort = match cli.effort.parse() {
        Ok(e) => e,
        Err(message) => {
            eprintln!("sr-ga1-synth: {}", message);
            return ExitCode::FAILURE;
        }
    };
    if cli.time_budget <= 0.0 || !cli.time_budget.is_finite() {
        eprintln!("sr-ga1-synth: --time-budget must be a positive number of seconds");
        return ExitCode::FAILURE;
    }
    let options = Options {
        sources: cli.sources.clone(),
        top: cli.top.clone(),
        fabric: cli.fabric.clone(),
        effort,
        time_budget: Duration::from_secs_f64(cli.time_budget),
        seed: cli.seed,
        check_only: cli.check,
        keep_intermediates: cli.keep_intermediates,
        workdir: cli.work_dir.clone(),
        verbose: cli.verbose,
        yosys: cli.yosys.clone(),
        constraints: cli.constraints.clone(),
    };

    let mut progress = Progress::new(false);

    if cli.check {
        return match flow::front(&options, &mut progress) {
            Ok(check) => {
                progress.clear();
                let used = check.packed.clb_count();
                let capacity = check.fabric.clb_count();
                println!(
                    "{}: {} CLBs of {} before placement ({} register(s), {} unconditional)",
                    check.packed.top,
                    used,
                    capacity,
                    check.packed.register_count(),
                    check.packed.always_enabled()
                );
                for (gate, count) in check.packed.gate_histogram() {
                    println!("  {:<10} {}", gate, count);
                }
                println!(
                    "  routing buffers and clock buffering are not counted here; the placed \
                     result is usually a little larger."
                );
                for warning in &check.warnings {
                    eprintln!("warning: {}", warning);
                }
                if used > capacity {
                    ExitCode::FAILURE
                } else {
                    ExitCode::SUCCESS
                }
            }
            Err(e) => {
                progress.clear();
                eprintln!("sr-ga1-synth: {}", e);
                ExitCode::FAILURE
            }
        };
    }

    let outcome = match flow::run(&options, &mut progress) {
        Ok(outcome) => outcome,
        Err(e) => {
            progress.clear();
            eprintln!("sr-ga1-synth: {}", e);
            return ExitCode::FAILURE;
        }
    };
    progress.done("done");

    let target = Target::with_carry(
        &outcome.fabric,
        &outcome.library,
        outcome.carry.as_ref(),
        &outcome.constraints,
    );

    // Design file for the GUI.
    if let Some(path) = &cli.output {
        let file = DesignFile {
            name: outcome.packed.top.clone(),
            stimulus: Default::default(),
            tick: 0,
            traces: Vec::new(),
            loopback: report::loopback_wiring(&outcome),
            io_names: report::io_names(&outcome),
        };
        let text = save_design(&outcome.fabric, &outcome.config, &file);
        if let Err(e) = std::fs::write(path, text) {
            eprintln!("sr-ga1-synth: cannot write {}: {}", path.display(), e);
            return ExitCode::FAILURE;
        }
    }

    // Bitstream.
    if let Some(path) = &cli.bitstream {
        let timestamp = timestamp();
        let text = bitstream::format_text(
            &outcome.fabric,
            &outcome.config,
            &outcome.packed.top,
            &timestamp,
            cli.raw,
        );
        if let Err(e) = std::fs::write(path, text) {
            eprintln!("sr-ga1-synth: cannot write {}: {}", path.display(), e);
            return ExitCode::FAILURE;
        }
    }

    // Report.
    let rendered = report::render(&outcome, &target);
    if let Some(path) = &cli.report {
        if let Err(e) = std::fs::write(path, &rendered) {
            eprintln!("sr-ga1-synth: cannot write {}: {}", path.display(), e);
            return ExitCode::FAILURE;
        }
    }
    if cli.verbose {
        print!("{}", rendered);
    }

    // Equivalence check, before anything is reported as good.
    if cli.equiv {
        progress.stage("checking equivalence under Verilator");
        let tool = match sr_ga1_synth::equiv::Verilator::discover(cli.verilator.as_deref()) {
            Ok(tool) => tool,
            Err(e) => {
                progress.clear();
                eprintln!("sr-ga1-synth: {}", e);
                return ExitCode::FAILURE;
            }
        };
        if cli.verbose {
            progress.note(&format!("using {} ({})", tool.exe.display(), tool.version));
        }
        match sr_ga1_synth::equivrun::check(
            &outcome,
            &cli.sources,
            &cli.work_dir,
            cli.equiv_cycles,
            &tool,
        ) {
            Ok(result) => {
                progress.clear();
                println!(
                    "equivalence: {} cycle(s) checked, {} output(s) compared, no mismatches",
                    result.cycles,
                    result.compared.len()
                );
            }
            Err(e) => {
                progress.clear();
                eprintln!("sr-ga1-synth: {}", e);
                return ExitCode::FAILURE;
            }
        }
    }

    println!("{}", report::summary(&outcome));
    for warning in &outcome.warnings {
        eprintln!("warning: {}", warning);
    }
    // The board wiring is not optional: without it the bitstream does not work.
    let wiring = report::loopback_wiring(&outcome);
    if !wiring.is_empty() {
        eprintln!();
        eprintln!("This design depends on loop-around wiring. Connect:");
        for link in &wiring {
            eprintln!("    {} -> {}", link.from, link.to);
        }
        eprintln!("The visual programmer cannot model board wiring, so its simulation of");
        eprintln!("this design will not match the hardware.");
    }
    ExitCode::SUCCESS
}

/// A timestamp in the format the visual programmer writes, without pulling in
/// a date library: seconds since the epoch rendered as UTC.
fn timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = now / 86_400;
    let seconds = now % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        year,
        month,
        day,
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}

/// Howard Hinnant's days-to-civil algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
