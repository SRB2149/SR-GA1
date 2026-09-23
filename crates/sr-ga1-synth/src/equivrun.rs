//! Running the equivalence check.
//!
//! Writes the fabric model and the testbench, then builds and runs them under
//! Verilator. A mismatch is a hard failure carrying the cycle, the vector and
//! the signal, straight from the testbench's own output.

use crate::bitstream;
use crate::equiv::{EquivError, EquivResult, Verilator};
use crate::equivtb::{testbench, PortMap};
use crate::flow::Outcome;
use crate::report;
use std::path::Path;

/// The model module's name, and the files written into the work directory.
const MODEL_MODULE: &str = "srga1_configured";
const MODEL_FILE: &str = "srga1_configured.sv";
const TB_FILE: &str = "equiv_tb.sv";

/// Check the emitted bitstream against the original design.
///
/// The configuration is **decoded from the bitstream** rather than taken from
/// memory, so the check covers the encoding as well as everything above it. If
/// the two disagree anywhere, that is a failure — there is no tolerance here.
pub fn check(
    outcome: &Outcome,
    sources: &[std::path::PathBuf],
    workdir: &Path,
    cycles: usize,
    verilator: &Verilator,
) -> Result<EquivResult, EquivError> {
    let fabric = &outcome.fabric;

    // Round-trip through the bitstream, so the model is built from the bits
    // that will actually be shifted into the chip.
    let bits = bitstream::export_bits(fabric, &outcome.config);
    let decoded = bitstream::import_bits(fabric, &bits).map_err(|e| EquivError {
        message: format!("the emitted bitstream does not decode: {}", e),
        log: None,
    })?;
    if bitstream::export_bits(fabric, &decoded) != bits {
        return Err(EquivError {
            message: "the bitstream does not survive a round trip, so the configuration the \
                      chip would receive is not the one that was placed"
                .to_string(),
            log: None,
        });
    }

    let map = PortMap::of(outcome);
    if map.outputs.is_empty() {
        return Err(EquivError {
            message: "this design drives no chip output, so there is nothing to compare"
                .to_string(),
            log: None,
        });
    }

    let loopbacks = report::loopback_wiring(outcome);
    let model = crate::equiv::fabric_model(fabric, &decoded, MODEL_MODULE);
    let bench = testbench(outcome, &map, MODEL_MODULE, cycles, &loopbacks);

    let write = |name: &str, text: &str| -> Result<(), EquivError> {
        let path = workdir.join(name);
        std::fs::write(&path, text).map_err(|e| EquivError {
            message: format!("cannot write {}: {}", path.display(), e),
            log: None,
        })
    };
    write(MODEL_FILE, &model)?;
    write(TB_FILE, &bench)?;

    // Verilator needs the design sources too, by absolute path.
    let mut command = verilator.command();
    command
        .current_dir(workdir)
        .arg("--binary")
        .arg("--timing")
        .arg("--quiet")
        .arg("-Wno-fatal")
        .arg("--top-module")
        .arg("equiv_tb")
        .arg("-Mdir")
        .arg("obj_equiv")
        .arg("-o")
        .arg("equiv_sim");
    for source in sources {
        command.arg(absolute(source));
    }
    command.arg(MODEL_FILE).arg(TB_FILE);

    let built = command.output().map_err(|e| EquivError {
        message: format!("could not start {}: {}", verilator.exe.display(), e),
        log: None,
    })?;
    let build_log = format!(
        "{}{}",
        String::from_utf8_lossy(&built.stdout),
        String::from_utf8_lossy(&built.stderr)
    );
    if !built.status.success() {
        return Err(EquivError {
            message: format!(
                "Verilator could not build the comparison ({}). The model and testbench are \
                 in {} if they need looking at.",
                status(&built.status),
                workdir.display()
            ),
            log: Some(build_log),
        });
    }

    // Run it.
    let binary = workdir.join("obj_equiv").join(exe_name("equiv_sim"));
    let run = std::process::Command::new(&binary).current_dir(workdir).output().map_err(|e| {
        EquivError {
            message: format!("could not run {}: {}", binary.display(), e),
            log: Some(build_log.clone()),
        }
    })?;
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );

    if log.contains("EQUIV OK") {
        return Ok(EquivResult {
            cycles,
            compared: map.compared(),
            driven: map.driven(),
        });
    }

    let mismatches: Vec<&str> = log.lines().filter(|l| l.contains("MISMATCH")).collect();
    let message = if mismatches.is_empty() {
        format!(
            "the equivalence check did not complete. The model and testbench are in {}.",
            workdir.display()
        )
    } else {
        format!(
            "the synthesised configuration does not match the source design ({} mismatch(es) \
             reported). This is a bug in the flow, not in the design.",
            mismatches.len()
        )
    };
    Err(EquivError { message, log: Some(log) })
}

fn status(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit status {}", code),
        None => "terminated by a signal".to_string(),
    }
}

fn exe_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{}.exe", base)
    } else {
        base.to_string()
    }
}

fn absolute(path: &Path) -> std::path::PathBuf {
    std::fs::canonicalize(path)
        .map(|p| std::path::PathBuf::from(p.to_string_lossy().replace("\\\\?\\", "")))
        .unwrap_or_else(|_| path.to_path_buf())
}
