//! Yosys and ABC, driven as a subprocess.
//!
//! The frontend is not reimplemented here: Yosys parses and elaborates the
//! SystemVerilog and ABC maps it against `fabric.genlib`. This module finds
//! the tools, generates the script, runs it, and hands back the two netlists
//! the rest of the flow reads — the elaborated one for subset checking and
//! the mapped one for packing.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The Yosys release this flow is developed and tested against. A different
/// build is allowed but warned about, since script syntax does drift.
pub const TESTED_VERSION: &str = "0.69+117";

#[derive(Debug, Clone)]
pub struct ToolError {
    pub message: String,
    /// The tool's own output, when it produced any.
    pub log: Option<String>,
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(log) = &self.log {
            let tail: Vec<&str> = log.lines().rev().take(25).collect();
            if !tail.is_empty() {
                write!(f, "\n\nlast output from the tool:")?;
                for line in tail.into_iter().rev() {
                    write!(f, "\n  {}", line)?;
                }
            }
        }
        Ok(())
    }
}

impl std::error::Error for ToolError {}

fn missing_yosys() -> ToolError {
    ToolError {
        message: format!(
            "Yosys was not found, and this tool cannot elaborate SystemVerilog without it.\n\
             \n\
             Install one of:\n  \
             - OSS CAD Suite (recommended, bundles Yosys, ABC and Verilator):\n    \
             https://github.com/YosysHQ/oss-cad-suite-build/releases — extract and add its \
             bin directory to PATH\n  \
             - Debian/Ubuntu: apt install yosys\n  \
             - macOS: brew install yosys\n\
             \n\
             Then put yosys on PATH, set the YOSYS environment variable, or pass \
             --yosys <path>.\n\
             Tested against Yosys {}.",
            TESTED_VERSION
        ),
        log: None,
    }
}

#[derive(Debug, Clone)]
pub struct Yosys {
    pub exe: PathBuf,
    pub version: String,
    /// Directories the child process needs on PATH. An OSS CAD Suite install
    /// keeps its shared libraries in a sibling `lib`, and yosys will not
    /// start without it.
    library_path: Vec<PathBuf>,
}

impl Yosys {
    /// Find Yosys: an explicit path, then `$YOSYS`, then `PATH`, then the
    /// usual install locations.
    pub fn discover(explicit: Option<&Path>) -> Result<Yosys, ToolError> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(p) = explicit {
            candidates.push(p.to_path_buf());
        }
        if let Some(env) = std::env::var_os("YOSYS") {
            candidates.push(PathBuf::from(env));
        }
        candidates.push(PathBuf::from("yosys"));
        for root in ["C:/oss-cad-suite", "/opt/oss-cad-suite", "/usr/local/oss-cad-suite"] {
            candidates.push(Path::new(root).join("bin").join(exe_name("yosys")));
        }
        candidates.push(PathBuf::from("/usr/bin/yosys"));
        candidates.push(PathBuf::from("/usr/local/bin/yosys"));

        let mut first_failure: Option<ToolError> = None;
        for candidate in candidates {
            let library_path = sibling_lib(&candidate);
            match probe(&candidate, &library_path) {
                Ok(version) => return Ok(Yosys { exe: candidate, version, library_path }),
                Err(e) => {
                    // An explicit path that fails is worth reporting as-is;
                    // a bare "yosys" that is simply absent is not.
                    if first_failure.is_none() && e.log.is_some() {
                        first_failure = Some(e);
                    }
                }
            }
        }
        Err(first_failure.unwrap_or_else(missing_yosys))
    }

    /// True when the build differs from the one this flow was tested on.
    pub fn version_warning(&self) -> Option<String> {
        if self.version.contains(TESTED_VERSION) {
            None
        } else {
            Some(format!(
                "using Yosys {} but this flow is tested against {}; if elaboration fails in \
                 an unexpected way, that difference is the first thing to check",
                self.version, TESTED_VERSION
            ))
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.exe);
        if !self.library_path.is_empty() {
            let existing = std::env::var_os("PATH").unwrap_or_default();
            let mut entries: Vec<PathBuf> = self.library_path.clone();
            entries.extend(std::env::split_paths(&existing));
            if let Ok(joined) = std::env::join_paths(entries) {
                cmd.env("PATH", joined);
            }
        }
        cmd
    }

    /// Run a script, returning its log. `workdir` is where the script's
    /// relative paths resolve.
    pub fn run_script(&self, script: &str, workdir: &Path) -> Result<String, ToolError> {
        let script_path = workdir.join("synth.ys");
        std::fs::write(&script_path, script).map_err(|e| ToolError {
            message: format!("cannot write {}: {}", script_path.display(), e),
            log: None,
        })?;
        let output = self
            .command()
            .current_dir(workdir)
            .arg("-q")
            .arg("-l")
            .arg("yosys.log")
            .arg("-s")
            .arg("synth.ys")
            .output()
            .map_err(|e| ToolError {
                message: format!("could not start {}: {}", self.exe.display(), e),
                log: None,
            })?;

        let log = std::fs::read_to_string(workdir.join("yosys.log")).unwrap_or_else(|_| {
            format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        if output.status.success() {
            Ok(log)
        } else {
            Err(ToolError {
                message: format!(
                    "Yosys failed while elaborating the design ({}). The message above the \
                     script trace usually names the file and line.",
                    status_text(&output.status)
                ),
                log: Some(log),
            })
        }
    }
}

fn status_text(status: &std::process::ExitStatus) -> String {
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

/// `<root>/bin/yosys` keeps its shared libraries in `<root>/lib`.
fn sibling_lib(exe: &Path) -> Vec<PathBuf> {
    let Some(bin) = exe.parent() else { return Vec::new() };
    if bin.file_name().is_none_or(|n| n != "bin") {
        return Vec::new();
    }
    let Some(root) = bin.parent() else { return Vec::new() };
    let lib = root.join("lib");
    if lib.is_dir() {
        vec![lib]
    } else {
        Vec::new()
    }
}

fn probe(exe: &Path, library_path: &[PathBuf]) -> Result<String, ToolError> {
    let mut cmd = Command::new(exe);
    if !library_path.is_empty() {
        let existing = std::env::var_os("PATH").unwrap_or_default();
        let mut entries: Vec<PathBuf> = library_path.to_vec();
        entries.extend(std::env::split_paths(&existing));
        if let Ok(joined) = std::env::join_paths(entries) {
            cmd.env("PATH", joined);
        }
    }
    let output = cmd.arg("-V").output().map_err(|_| missing_yosys())?;
    if !output.status.success() {
        return Err(ToolError {
            message: format!("{} did not run successfully", exe.display()),
            log: Some(String::from_utf8_lossy(&output.stderr).into_owned()),
        });
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        return Err(ToolError {
            message: format!("{} printed no version banner", exe.display()),
            log: None,
        });
    }
    Ok(line.trim_start_matches("Yosys ").to_string())
}

// ---------------------------------------------------------------------------
// Script generation

/// Everything the frontend needs to know about one run.
#[derive(Debug, Clone)]
pub struct FrontendRequest {
    pub sources: Vec<PathBuf>,
    pub top: String,
    /// Written by this tool before the run, relative to `workdir`.
    pub genlib: PathBuf,
    pub workdir: PathBuf,
    /// Map adders onto the fabric's carry chain instead of letting them
    /// expand into ordinary gates.
    pub carry_chains: bool,
}

/// Names of the files the script writes, relative to the work directory.
pub const ELABORATED_JSON: &str = "elaborated.json";
pub const MAPPED_JSON: &str = "mapped.json";
pub const ARITH_MAP: &str = "arith_map.v";

/// The flip-flop the fabric has: rising edge, synchronous reset that wins
/// over the enable, active-high enable, either reset value. `x` says the
/// power-on value is undefined, which is true — the configuration shift
/// registers have no reset.
pub const TARGET_FF: &str = "$_SDFFE_PP?P_";

impl FrontendRequest {
    pub fn script(&self) -> String {
        let mut s = String::new();
        s.push_str("# Generated by sr-ga1-synth. Regenerate with --keep-intermediates.\n");
        for source in &self.sources {
            s.push_str(&format!("read_verilog -sv {}\n", quote(source)));
        }
        s.push_str(&format!("hierarchy -top {} -check\n", self.top));
        s.push_str("proc\n");
        s.push_str("flatten\n");
        s.push_str("tribuf -logic\n");
        s.push_str("opt -full\n");
        s.push_str("fsm\n");
        s.push_str("opt -full\n");
        s.push_str("wreduce\n");
        s.push_str("peepopt\n");
        s.push_str("opt -full\n");
        // The subset check reads this: coarse cells intact, so an inferred
        // latch is still a latch and an adder is still an adder.
        s.push_str(&format!("write_json {}\n", ELABORATED_JSON));

        if self.carry_chains {
            // `alumacc` funnels addition, subtraction and comparison into
            // `$alu`, so one map covers all of them. It has to run *after* the
            // elaborated dump, because it rewrites `$mul` into `$macc` and the
            // subset check needs to see a multiply as a multiply.
            //
            // It also has to run before the generic `techmap`: left to itself,
            // techmap expands `$add` through `$alu` into `$fa` and `$lcu` and
            // then into ordinary gates, and the carry chain never gets used.
            s.push_str("alumacc\n");
            s.push_str("opt -full\n");
            s.push_str(&format!("techmap -map {}\n", ARITH_MAP));
            s.push_str("opt_expr\n");
            s.push_str("opt_clean\n");
        }
        s.push_str("techmap\n");
        s.push_str("opt -full\n");
        // dfflegalize goes last, with no `opt` after it: `opt_dff` undoes the
        // legalisation by folding a constant-1 enable away, turning the
        // enabled flop we asked for back into a plain one.
        s.push_str(&format!("dfflegalize -cell {} x\n", TARGET_FF));
        s.push_str(&format!("abc -genlib {}\n", quote(&self.genlib)));
        s.push_str("opt_clean\n");
        s.push_str(&format!("write_json {}\n", MAPPED_JSON));
        s.push_str("stat\n");
        s
    }
}

fn quote(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if text.contains(char::is_whitespace) {
        format!("\"{}\"", text)
    } else {
        text
    }
}
