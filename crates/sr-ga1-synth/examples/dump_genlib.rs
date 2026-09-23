//! Dump the derived cell library, for eyeballing and for feeding to ABC.
use sr_ga1_synth::{fabric::Fabric, genlib::CellLibrary};

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "fabric.toml".into());
    let f = Fabric::load_file(std::path::Path::new(&path)).expect("fabric loads");
    let lib = CellLibrary::derive(&f);
    match std::env::args().nth(2).as_deref() {
        Some("impls") => print!("{}", lib.implementations_report()),
        _ => print!("{}", lib.genlib(&f)),
    }
}
