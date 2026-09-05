//! Core library for the SR-GA1 FPGA tool: fabric model, naming engine,
//! simulator and bitstream codec. GUI-free so it can drive the CLI and be
//! unit-tested directly.

pub mod bitstream;
pub mod config;
pub mod designfile;
pub mod drc;
pub mod fabric;
pub mod naming;
pub mod sim;
pub mod trace;
