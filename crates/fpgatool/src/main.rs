//! fpgatool — configure, visualise, simulate and export bitstreams for the
//! SR-GA1 fabric. Runs as a GUI with no arguments, or headless via the
//! `build` / `check` / `import` / `diff` subcommands.

mod app;
mod cli;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("build") | Some("check") | Some("import") | Some("diff") => {
            let cmd = args[0].clone();
            std::process::exit(cli::run(&cmd, &args[1..]));
        }
        Some("help") | Some("--help") | Some("-h") => println!("{}", cli::USAGE),
        Some(path) if !path.starts_with('-') => app::run(Some(path.to_string())),
        None => app::run(None),
        Some(other) => {
            eprintln!("unknown argument \"{}\"\n\n{}", other, cli::USAGE);
            std::process::exit(2);
        }
    }
}
