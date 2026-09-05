//! Headless CLI: `fpgatool build`, `check`, `import`, `diff`. All failure
//! paths print a message and return a non-zero exit code; nothing panics on
//! malformed input.

use fpga_core::bitstream;
use fpga_core::config::BlockId;
use fpga_core::designfile::{load_design, save_design, DesignFile};
use fpga_core::drc::{self, Severity};
use fpga_core::fabric::Fabric;
use std::path::{Path, PathBuf};

pub const USAGE: &str = "\
fpgatool — SR-GA1 fabric configuration tool

USAGE:
  fpgatool                          launch the GUI
  fpgatool <design.json>            launch the GUI with a design loaded
  fpgatool build <design.json> -o <bits.txt> [--raw] [--wrap N] [--fabric <fabric.toml>]
  fpgatool check <design.json> [--fabric <fabric.toml>]
  fpgatool import <bits.txt> -o <design.json> [--name <name>] [--fabric <fabric.toml>]
  fpgatool diff <a.json> <b.json> [--fabric <fabric.toml>]

  build    export the design's bitstream (exit 1 on any error)
  check    run design rule checks; exit 1 if any DRC error is found
  import   reconstruct a design file from a bitstream
  diff     report blocks whose configuration differs; exit 1 when they differ

The fabric description defaults to ./fabric.toml.";

struct Args {
    positional: Vec<String>,
    output: Option<PathBuf>,
    fabric: PathBuf,
    raw: bool,
    wrap: Option<usize>,
    name: Option<String>,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut out = Args {
        positional: Vec::new(),
        output: None,
        fabric: PathBuf::from("fabric.toml"),
        raw: false,
        wrap: None,
        name: None,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" | "--output" => {
                out.output = Some(PathBuf::from(
                    it.next().ok_or_else(|| format!("{} needs a path", a))?,
                ))
            }
            "--fabric" => out.fabric = PathBuf::from(it.next().ok_or("--fabric needs a path")?),
            "--raw" => out.raw = true,
            "--wrap" => {
                let n = it.next().ok_or("--wrap needs a number")?;
                out.wrap = Some(n.parse().map_err(|_| format!("bad --wrap value \"{}\"", n))?);
            }
            "--name" => out.name = Some(it.next().ok_or("--name needs a value")?.clone()),
            other if other.starts_with('-') => return Err(format!("unknown option \"{}\"", other)),
            other => out.positional.push(other.to_string()),
        }
    }
    Ok(out)
}

fn load_fabric(path: &Path) -> Result<Fabric, String> {
    Fabric::load_file(path).map_err(|e| e.to_string())
}

fn read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {}", path, e))
}

fn load(fabric: &Fabric, path: &str) -> Result<DesignFile, String> {
    let (file, warnings) = load_design(fabric, &read(path)?).map_err(|e| format!("{}: {}", path, e))?;
    for w in warnings {
        eprintln!("warning: {}: {}", path, w);
    }
    Ok(file)
}

/// Run a CLI subcommand; returns the process exit code.
pub fn run(cmd: &str, rest: &[String]) -> i32 {
    match run_inner(cmd, rest) {
        Ok(code) => code,
        Err(msg) => {
            eprintln!("error: {}", msg);
            1
        }
    }
}

fn run_inner(cmd: &str, rest: &[String]) -> Result<i32, String> {
    let args = parse_args(rest)?;
    match cmd {
        "build" => {
            let [design_path] = args.positional.as_slice() else {
                return Err("build takes exactly one design file".to_string());
            };
            let fabric = load_fabric(&args.fabric)?;
            let file = load(&fabric, design_path)?;
            let out_path = args.output.ok_or("build needs -o <bits.txt>")?;
            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let text = bitstream::format_text(&fabric, &file.design, &file.name, &timestamp, args.raw, args.wrap);
            std::fs::write(&out_path, text).map_err(|e| format!("cannot write {}: {}", out_path.display(), e))?;
            println!(
                "wrote {} bits to {}",
                fabric.total_bits(),
                out_path.display()
            );
            Ok(0)
        }
        "check" => {
            let [design_path] = args.positional.as_slice() else {
                return Err("check takes exactly one design file".to_string());
            };
            let fabric = load_fabric(&args.fabric)?;
            let file = load(&fabric, design_path)?;
            let items = drc::check(&fabric, &file.design);
            let mut errors = 0;
            for item in &items {
                let tag = match item.severity {
                    Severity::Error => {
                        errors += 1;
                        "ERROR"
                    }
                    Severity::Warning => "warn ",
                    Severity::Info => "info ",
                };
                println!("{}  {}", tag, item.message);
            }
            println!(
                "{} error(s), {} warning(s), {} notice(s)",
                errors,
                items.iter().filter(|i| i.severity == Severity::Warning).count(),
                items.iter().filter(|i| i.severity == Severity::Info).count()
            );
            Ok(if errors > 0 { 1 } else { 0 })
        }
        "import" => {
            let [bits_path] = args.positional.as_slice() else {
                return Err("import takes exactly one bitstream file".to_string());
            };
            let fabric = load_fabric(&args.fabric)?;
            let bits = bitstream::parse_text(&read(bits_path)?).map_err(|e| format!("{}: {}", bits_path, e))?;
            let design = bitstream::import_bits(&fabric, &bits).map_err(|e| format!("{}: {}", bits_path, e))?;
            let out_path = args.output.ok_or("import needs -o <design.json>")?;
            let mut file = DesignFile::new(&fabric, args.name.as_deref().unwrap_or("imported"));
            file.design = design;
            std::fs::write(&out_path, save_design(&fabric, &file))
                .map_err(|e| format!("cannot write {}: {}", out_path.display(), e))?;
            println!("imported {} bits into {}", bits.len(), out_path.display());
            Ok(0)
        }
        "diff" => {
            let [a_path, b_path] = args.positional.as_slice() else {
                return Err("diff takes exactly two design files".to_string());
            };
            let fabric = load_fabric(&args.fabric)?;
            let a = load(&fabric, a_path)?;
            let b = load(&fabric, b_path)?;
            let mut differences = 0;
            for block in a.design.blocks() {
                let mut lines = Vec::new();
                match block {
                    BlockId::Clb { col, row } => {
                        for f in &fabric.clb_fields {
                            let va = a.design.get_clb(&fabric, col, row, &f.name).unwrap_or(0);
                            let vb = b.design.get_clb(&fabric, col, row, &f.name).unwrap_or(0);
                            if va != vb {
                                lines.push(format!("  {}: {} -> {}", f.name, va, vb));
                            }
                        }
                    }
                    BlockId::Csb { col } => {
                        for f in &fabric.csb_fields {
                            let va = a.design.get_csb(&fabric, col, &f.name).unwrap_or(0);
                            let vb = b.design.get_csb(&fabric, col, &f.name).unwrap_or(0);
                            if va != vb {
                                lines.push(format!("  {}: {} -> {}", f.name, va, vb));
                            }
                        }
                    }
                }
                let pa = a.design.pinned_name(block);
                let pb = b.design.pinned_name(block);
                if pa != pb {
                    lines.push(format!(
                        "  name: {} -> {}",
                        pa.unwrap_or("(default)"),
                        pb.unwrap_or("(default)")
                    ));
                }
                if !lines.is_empty() {
                    differences += 1;
                    println!("{}", fpga_core::config::default_name(&fabric, block));
                    for l in lines {
                        println!("{}", l);
                    }
                }
            }
            if differences == 0 {
                println!("designs are identical");
                Ok(0)
            } else {
                println!("{} block(s) differ", differences);
                Ok(1)
            }
        }
        other => Err(format!("unknown command \"{}\"\n\n{}", other, USAGE)),
    }
}
