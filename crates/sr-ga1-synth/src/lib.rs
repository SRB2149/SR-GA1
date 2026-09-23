//! SR-GA1 SystemVerilog synthesiser.
//!
//! A standalone command-line flow: elaborate with Yosys, map against the
//! fabric's own cell library with ABC, pack, place, route, verify, and emit
//! a configuration bitstream plus a design file the visual programmer can
//! open with names intact.
//!
//! The tool shares no code with the visual programmer. Both read the same
//! `fabric.toml`, and the golden tests hold the two output formats together.

pub mod backend;
pub mod bitstream;
pub mod carry;
pub mod constraints;
pub mod design;
pub mod designjson;
pub mod equiv;
pub mod equivrun;
pub mod equivtb;
pub mod fabric;
pub mod flow;
pub mod genlib;
pub mod loops;
pub mod naming;
pub mod netlist;
pub mod pack;
pub mod place;
pub mod progress;
pub mod report;
pub mod route;
pub mod rrg;
pub mod subset;
pub mod yosys;
