//! Packing: the mapped netlist becomes a list of CLBs.
//!
//! Three fabric facts drive everything here.
//!
//! * **The flip-flop's data input is hard-wired to its own CLB's operation
//!   result.** There is no independent register data path, so a register can
//!   only ever live on the cell that computes its value. Fusing a DFF into
//!   its driving cell is therefore not an optimisation, it is the only legal
//!   placement — and since `_op` and `_reg` are separately routable, it
//!   costs nothing even when the driver has other consumers. A DFF fed by a
//!   chip input, by another register, or by a cell that already carries one
//!   needs a buffer CLB of its own.
//! * **Horizontal lane 3 is the register's write enable.** An unconditional
//!   register needs a constant 1 there; an enabled one needs its enable net
//!   routed there. That is recorded as a routing requirement on the cell,
//!   because it is frequently the constraint that decides whether a design
//!   fits.
//! * **Reset is global, synchronous, and wins over the enable**, loading the
//!   constant in config bit 19.

use crate::carry::{CarryPlan, CARRY_CELL};
use crate::genlib::CellLibrary;
use crate::naming::{NameAllocator, NetNames};
use crate::netlist::{Bit, Module, SrcLoc};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Debug, Clone)]
pub struct PackError {
    pub loc: Option<SrcLoc>,
    pub message: String,
}

impl fmt::Display for PackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.loc {
            Some(loc) => write!(f, "{}: {}", loc, self.message),
            None => write!(f, "{}", self.message),
        }
    }
}

impl std::error::Error for PackError {}

/// What a cell pin or a register output is connected to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Signal {
    Net(u32),
    Const(bool),
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Signal::Net(n) => write!(f, "net {}", n),
            Signal::Const(v) => write!(f, "constant {}", u8::from(*v)),
        }
    }
}

impl Signal {
    fn from_bit(bit: Bit) -> Option<Signal> {
        match bit {
            Bit::Net(n) => Some(Signal::Net(n)),
            Bit::Zero => Some(Signal::Const(false)),
            Bit::One => Some(Signal::Const(true)),
            _ => None,
        }
    }
}

/// How a packed register's write enable is satisfied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Enable {
    /// Capture on every clock edge: lane 3 must carry a constant 1 at this
    /// CLB's position. Free in row 3, otherwise driven by an upstream CLB.
    Always,
    /// Lane 3 must carry this net at this CLB's position.
    Net(u32),
}

#[derive(Debug, Clone)]
pub struct Register {
    /// The net the register drives, read from `_reg`.
    pub q: u32,
    pub enable: Enable,
    /// Config bit 19: the value a global reset loads.
    pub reset_value: bool,
    /// Clock net, traced back to a chip input in Phase 5.
    pub clock: u32,
    /// The global reset net, if this register uses one.
    pub reset: Option<u32>,
}

/// Where a cell sits in a ripple adder.
///
/// The chain is hardware, not routing: a cell's carry output goes only to the
/// cell directly above it, so a chain pins its members to consecutive
/// ascending rows of one column with the least significant bit at the bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarryLink {
    pub chain: usize,
    /// 0 is the least significant bit, which sits in the bottom row.
    pub index: usize,
    /// For the least significant bit only: an external carry-in, which has to
    /// arrive on a horizontal lane because row 0's chain input is tied off.
    /// `None` means the tied-off value is what the adder wants.
    pub carry_in: Option<Signal>,
}

/// One CLB's worth of work: a logic function, optionally with its register.
#[derive(Debug, Clone)]
pub struct PackedCell {
    pub name: String,
    /// Cell name in the derived library, e.g. `AO21`. For a carry cell this
    /// is the operation that produces the sum bit.
    pub gate: String,
    /// Pin k of the gate, in genlib pin order. A carry cell has two: the
    /// addends, with the third input coming from the chain.
    pub pins: Vec<Signal>,
    /// The net `_op` drives, if anything reads it.
    pub op: Option<u32>,
    pub reg: Option<Register>,
    pub src: Option<SrcLoc>,
    /// Set when the packer created this cell rather than the designer.
    pub inserted_buffer: bool,
    /// Set when this cell is one bit of a ripple adder.
    pub carry: Option<CarryLink>,
}

impl PackedCell {
    /// Every signal this cell reads.
    pub fn inputs(&self) -> impl Iterator<Item = Signal> + '_ {
        self.pins.iter().copied()
    }
    pub fn drives_register(&self) -> bool {
        self.reg.is_some()
    }
    pub fn is_carry(&self) -> bool {
        self.carry.is_some()
    }
}

/// A design input or output bit and the pad it must reach.
#[derive(Debug, Clone)]
pub struct PortBit {
    /// Design-level name, e.g. `count[2]`.
    pub name: String,
    pub net: u32,
    /// Index within the declared port, for pin-lock lookup.
    pub port: String,
    pub index: usize,
}

#[derive(Debug, Clone)]
pub struct PackedDesign {
    pub top: String,
    pub cells: Vec<PackedCell>,
    pub inputs: Vec<PortBit>,
    pub outputs: Vec<PortBit>,
    /// Carry chains, each listing its cells least significant first.
    pub chains: Vec<Vec<usize>>,
    /// Clock nets, in first-seen order; each needs a column and a CSB.
    pub clocks: Vec<u32>,
    pub reset: Option<u32>,
    /// Names for every net, for the report and the design file.
    pub names: NetNames,
    pub warnings: Vec<String>,
}

impl PackedDesign {
    pub fn clb_count(&self) -> usize {
        self.cells.len()
    }
    pub fn register_count(&self) -> usize {
        self.cells.iter().filter(|c| c.drives_register()).count()
    }
    /// Cells whose register captures unconditionally — cheapest in the row
    /// whose lane 3 enters as a constant 1.
    pub fn always_enabled(&self) -> usize {
        self.cells
            .iter()
            .filter(|c| matches!(c.reg.as_ref().map(|r| &r.enable), Some(Enable::Always)))
            .count()
    }

    pub fn gate_histogram(&self) -> BTreeMap<String, usize> {
        let mut out = BTreeMap::new();
        for cell in &self.cells {
            *out.entry(cell.gate.clone()).or_insert(0) += 1;
        }
        out
    }
}

/// Yosys' fine synchronous-reset flops, with or without a clock enable:
/// `$_SDFFE_[clk][rst][val][en]_` and `$_SDFF_[clk][rst][val]_`.
///
/// Both are accepted. `dfflegalize` is asked for the enabled form, but a
/// register with no enable in the source can legitimately arrive as the plain
/// one, and that simply means the enable is always asserted.
fn parse_sync_ff(ty: &str) -> Option<(bool, bool, bool, bool)> {
    if let Some(body) = ty.strip_prefix("$_SDFFE_").and_then(|b| b.strip_suffix('_')) {
        let chars: Vec<char> = body.chars().collect();
        let [clk, rst, val, en] = chars[..] else { return None };
        return Some((clk == 'P', rst == 'P', val == '1', en == 'P'));
    }
    let body = ty.strip_prefix("$_SDFF_")?.strip_suffix('_')?;
    let chars: Vec<char> = body.chars().collect();
    let [clk, rst, val] = chars[..] else { return None };
    Some((clk == 'P', rst == 'P', val == '1', true))
}

pub fn pack(
    module: &Module,
    library: &CellLibrary,
    carry_plan: Option<&CarryPlan>,
    top: &str,
) -> Result<PackedDesign, PackError> {
    let names = NetNames::collect(module);
    let mut warnings = Vec::new();

    // ---- read the mapped cells -------------------------------------------
    struct RawReg {
        cell: String,
        d: Signal,
        q: u32,
        enable: Enable,
        reset_value: bool,
        clock: u32,
        reset: Option<u32>,
        src: Option<SrcLoc>,
    }

    // One entry per adder bit, before they are threaded into chains.
    struct RawCarry {
        cell: String,
        a: Signal,
        b: Signal,
        ci: Signal,
        s: Option<u32>,
        co: Option<u32>,
        src: Option<SrcLoc>,
    }

    let mut logic: Vec<PackedCell> = Vec::new();
    let mut regs: Vec<RawReg> = Vec::new();
    let mut carries: Vec<RawCarry> = Vec::new();
    let mut allocator = NameAllocator::new();

    for (cell_name, cell) in &module.cells {
        if let Some((clk_pos, rst_pos, reset_value, en_pos)) = parse_sync_ff(&cell.ty) {
            // dfflegalize was asked for exactly one shape; anything else
            // means the design needed something the fabric cannot do.
            if !clk_pos || !rst_pos || !en_pos {
                return Err(PackError {
                    loc: cell.src(),
                    message: format!(
                        "register \"{}\" needs {} polarity control, which the CLB does not have",
                        cell_name,
                        if !clk_pos { "an inverted clock" } else { "an active-low" }
                    ),
                });
            }
            let d = cell.bit("D").and_then(Signal::from_bit).ok_or_else(|| PackError {
                loc: cell.src(),
                message: format!("register \"{}\" has no usable data input", cell_name),
            })?;
            let q = cell.bit("Q").and_then(|b| b.as_net()).ok_or_else(|| PackError {
                loc: cell.src(),
                message: format!("register \"{}\" drives no net", cell_name),
            })?;
            let clock = cell.bit("C").and_then(|b| b.as_net()).ok_or_else(|| PackError {
                loc: cell.src(),
                message: format!(
                    "register \"{}\" has a constant clock; every fabric clock must come from \
                     a chip input routed onto a vertical ring",
                    cell_name
                ),
            })?;
            let enable = match cell.bit("E") {
                Some(Bit::One) | None => Enable::Always,
                Some(Bit::Zero) => {
                    return Err(PackError {
                        loc: cell.src(),
                        message: format!(
                            "register \"{}\" is permanently disabled; it can never capture",
                            cell_name
                        ),
                    })
                }
                Some(Bit::Net(n)) => Enable::Net(n),
                Some(other) => {
                    return Err(PackError {
                        loc: cell.src(),
                        message: format!(
                            "register \"{}\" has an enable of {}, which is not a routable value",
                            cell_name, other
                        ),
                    })
                }
            };
            let reset = match cell.bit("R") {
                Some(Bit::Net(n)) => Some(n),
                _ => None,
            };
            regs.push(RawReg {
                cell: cell_name.clone(),
                d,
                q,
                enable,
                reset_value,
                clock,
                reset,
                src: cell.src(),
            });
            continue;
        }

        if cell.ty.trim_start_matches('\\') == CARRY_CELL {
            let plan = carry_plan.ok_or_else(|| PackError {
                loc: cell.src(),
                message: format!(
                    "the design contains an adder but this fabric has no usable carry chain,                      so cell \"{}\" cannot be built",
                    display(cell_name)
                ),
            })?;
            let _ = plan;
            let read = |port: &str| -> Result<Signal, PackError> {
                cell.bit(port).and_then(Signal::from_bit).ok_or_else(|| PackError {
                    loc: cell.src(),
                    message: format!(
                        "adder cell \"{}\" has no usable {} input",
                        display(cell_name),
                        port
                    ),
                })
            };
            carries.push(RawCarry {
                cell: cell_name.clone(),
                a: read("A")?,
                b: read("B")?,
                ci: read("CI")?,
                s: cell.bit("S").and_then(|b| b.as_net()),
                co: cell.bit("CO").and_then(|b| b.as_net()),
                src: cell.src(),
            });
            continue;
        }

        // Everything else must be a gate from the derived library.
        let Some(gate) = library.cell(&cell.ty) else {
            return Err(PackError {
                loc: cell.src(),
                message: format!(
                    "cell \"{}\" has type {}, which is not in the fabric's cell library; \
                     the mapper should not have produced it",
                    cell_name, cell.ty
                ),
            });
        };
        let mut pins = Vec::with_capacity(gate.arity);
        for pin in 0..gate.arity {
            let port = crate::genlib::gate_pin_name(pin);
            let bit = cell.bit(&port).ok_or_else(|| PackError {
                loc: cell.src(),
                message: format!("cell \"{}\" ({}) has no pin {}", cell_name, cell.ty, port),
            })?;
            let signal = Signal::from_bit(bit).ok_or_else(|| PackError {
                loc: cell.src(),
                message: format!(
                    "cell \"{}\" pin {} is driven by {}, which is not a real value",
                    cell_name, port, bit
                ),
            })?;
            pins.push(signal);
        }
        let out = cell.bit("O").and_then(|b| b.as_net()).ok_or_else(|| PackError {
            loc: cell.src(),
            message: format!("cell \"{}\" ({}) drives no net", cell_name, cell.ty),
        })?;
        logic.push(PackedCell {
            name: String::new(), // assigned once fusion has settled
            gate: cell.ty.clone(),
            pins,
            op: Some(out),
            reg: None,
            src: cell.src(),
            inserted_buffer: false,
            carry: None,
        });
    }

    // ---- fuse registers into their drivers --------------------------------
    let mut driver_of: BTreeMap<u32, usize> = BTreeMap::new();
    // ---- thread the adder bits into chains --------------------------------
    // A cell's carry output feeds exactly one cell above it, so the chains
    // are found by following CO to the CI that reads it. Anything else
    // reading a CO is a design that wants a carry the fabric cannot deliver.
    let mut chains: Vec<Vec<usize>> = Vec::new();
    if !carries.is_empty() {
        let plan = carry_plan.ok_or_else(|| PackError {
            loc: None,
            message: "the design contains adders but this fabric has no usable carry chain"
                .to_string(),
        })?;

        // Who consumes each carry output, by CI.
        let mut consumer: BTreeMap<u32, usize> = BTreeMap::new();
        for (index, cell) in carries.iter().enumerate() {
            if let Signal::Net(net) = cell.ci {
                if let Some(previous) = consumer.insert(net, index) {
                    return Err(PackError {
                        loc: carries[index].src.clone(),
                        message: format!(
                            "adder cells \"{}\" and \"{}\" both read the same carry, but a \
                             carry output reaches only the cell directly above it",
                            display(&carries[previous].cell),
                            display(&carries[index].cell)
                        ),
                    });
                }
            }
        }
        let produces: BTreeMap<u32, usize> = carries
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.co.map(|net| (net, i)))
            .collect();

        // A chain starts at a cell whose carry-in is not another cell's
        // carry-out.
        let mut placed_in_chain = vec![false; carries.len()];
        for (index, cell) in carries.iter().enumerate() {
            let fed_by_chain = match cell.ci {
                Signal::Net(net) => produces.contains_key(&net),
                Signal::Const(_) => false,
            };
            if fed_by_chain || placed_in_chain[index] {
                continue;
            }
            let mut chain = vec![index];
            placed_in_chain[index] = true;
            let mut current = index;
            while let Some(co) = carries[current].co {
                let Some(&next) = consumer.get(&co) else { break };
                if placed_in_chain[next] {
                    break;
                }
                placed_in_chain[next] = true;
                chain.push(next);
                current = next;
            }
            chains.push(chain);
        }

        for chain in &chains {
            if chain.len() > plan.max_length {
                return Err(PackError {
                    loc: carries[chain[0]].src.clone(),
                    message: format!(
                        "this addition is {} bits wide, but a carry chain runs up a single \
                         column and is at most {} cells long. Split the arithmetic into \
                         {}-bit pieces, or build the wider adder in the operation core at \
                         roughly three CLBs per bit.",
                        chain.len(),
                        plan.max_length,
                        plan.max_length
                    ),
                });
            }
            // The carry-out of the top cell reaches nothing, and no cell's
            // carry may be read by ordinary logic.
            for (position, &index) in chain.iter().enumerate() {
                let Some(co) = carries[index].co else { continue };
                let feeds_next = position + 1 < chain.len()
                    && carries[chain[position + 1]].ci == Signal::Net(co);
                let read_elsewhere = logic.iter().any(|c| c.pins.contains(&Signal::Net(co)))
                    || regs.iter().any(|r| r.d == Signal::Net(co))
                    || module.ports.values().any(|p| {
                        p.direction == crate::netlist::Direction::Output
                            && p.bits.iter().any(|b| b.as_net() == Some(co))
                    });
                if read_elsewhere || (!feeds_next && position + 1 < chain.len()) {
                    return Err(PackError {
                        loc: carries[index].src.clone(),
                        message: format!(
                            "the carry out of adder bit {} is read by something other than the \
                             next bit of the same adder. Carry appears on no output mux: the \
                             only place it goes is the cell directly above, so this value \
                             cannot be produced. A carry-out has to be rebuilt in the \
                             operation core.",
                            position
                        ),
                    });
                }
            }
        }

        // Turn each adder bit into a cell. The sum is the operation result,
        // and the third input comes from the chain rather than from routing.
        let mut cell_of: BTreeMap<usize, usize> = BTreeMap::new();
        for (chain_index, chain) in chains.iter().enumerate() {
            for (position, &raw) in chain.iter().enumerate() {
                let source = &carries[raw];
                let carry_in = if position == 0 {
                    match source.ci {
                        // The tied-off edge value is what the chain provides
                        // for free; anything else has to be routed in.
                        Signal::Const(v) if v == plan.edge => None,
                        other => Some(other),
                    }
                } else {
                    None
                };
                cell_of.insert(raw, logic.len());
                logic.push(PackedCell {
                    name: String::new(),
                    gate: plan.op_name.clone(),
                    pins: vec![source.a, source.b],
                    op: source.s,
                    reg: None,
                    src: source.src.clone(),
                    inserted_buffer: false,
                    carry: Some(CarryLink { chain: chain_index, index: position, carry_in }),
                });
            }
        }
        // Rewrite the chains to point at packed cell indices.
        for chain in chains.iter_mut() {
            for entry in chain.iter_mut() {
                *entry = cell_of[entry];
            }
        }
    }

    for (i, cell) in logic.iter().enumerate() {
        if let Some(net) = cell.op {
            driver_of.insert(net, i);
        }
    }

    let buffer_gate = library
        .cells
        .iter()
        .find(|c| c.arity == 1 && c.table == [false, true])
        .ok_or_else(|| PackError {
            loc: None,
            message: "the fabric's cell library has no buffer, so a register fed by a chip \
                      input cannot be built"
                .to_string(),
        })?
        .name
        .clone();

    let mut taken: BTreeSet<usize> = BTreeSet::new();
    let mut buffers: Vec<PackedCell> = Vec::new();
    for reg in &regs {
        let register = Register {
            q: reg.q,
            enable: reg.enable.clone(),
            reset_value: reg.reset_value,
            clock: reg.clock,
            reset: reg.reset,
        };
        let fuse_target = match reg.d {
            Signal::Net(net) => driver_of.get(&net).copied(),
            Signal::Const(_) => None,
        };
        match fuse_target {
            // The driver is free: the register rides along at no cost.
            Some(index) if taken.insert(index) => {
                logic[index].reg = Some(register);
            }
            // The driver already carries a register, or the data comes from
            // a chip input, a constant, or another register's output. Either
            // way this one needs a CLB that recomputes the value.
            _ => {
                buffers.push(PackedCell {
                    name: String::new(),
                    gate: buffer_gate.clone(),
                    pins: vec![reg.d],
                    op: None,
                    reg: Some(register),
                    src: reg.src.clone(),
                    inserted_buffer: true,
                    carry: None,
                });
                let why = match reg.d {
                    Signal::Const(v) => format!("its data is the constant {}", u8::from(v)),
                    Signal::Net(net) if driver_of.contains_key(&net) => {
                        "its driver already carries a register".to_string()
                    }
                    Signal::Net(_) => {
                        "its data comes from a chip input or another register".to_string()
                    }
                };
                // Name the signal, not Yosys' internal cell: "sr[0]" tells
                // the designer where to look, "$auto$ff.cc:337:slice$78" does
                // not.
                let who = names
                    .get(reg.q)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| display(&reg.cell));
                warnings.push(format!(
                    "register \"{}\" costs an extra CLB to buffer because {}",
                    who, why
                ));
            }
        }
    }
    logic.extend(buffers);

    // ---- names -------------------------------------------------------------
    // A CLB is named for what it produces: its registered output if it has
    // one, otherwise its combinational output.
    allocator.reserve("fixed_zero");
    allocator.reserve("fixed_one");
    for cell in logic.iter_mut() {
        let preferred = cell
            .reg
            .as_ref()
            .and_then(|r| names.get(r.q).map(|s| s.to_string()))
            .or_else(|| cell.op.and_then(|n| names.get(n).map(|s| s.to_string())))
            .unwrap_or_else(|| cell.gate.to_lowercase());
        cell.name = allocator.unique(&preferred);
    }

    // ---- clocks and reset --------------------------------------------------
    let mut clocks: Vec<u32> = Vec::new();
    let mut resets: BTreeSet<u32> = BTreeSet::new();
    for cell in &logic {
        if let Some(reg) = &cell.reg {
            if !clocks.contains(&reg.clock) {
                clocks.push(reg.clock);
            }
            if let Some(r) = reg.reset {
                resets.insert(r);
            }
        }
    }
    if resets.len() > 1 {
        let listed: Vec<String> = resets
            .iter()
            .map(|r| names.get(*r).unwrap_or("<unnamed>").to_string())
            .collect();
        return Err(PackError {
            loc: None,
            message: format!(
                "the design has {} reset nets ({}); the chip has one global reset pin",
                resets.len(),
                listed.join(", ")
            ),
        });
    }

    // ---- ports -------------------------------------------------------------
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    for (port_name, port) in &module.ports {
        for (index, bit) in port.bits.iter().enumerate() {
            let Some(net) = bit.as_net() else { continue };
            let name = names
                .get(net)
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("{}[{}]", crate::naming::sanitise(port_name), index));
            let entry =
                PortBit { name, net, port: port_name.trim_start_matches('\\').to_string(), index };
            match port.direction {
                crate::netlist::Direction::Input => inputs.push(entry),
                crate::netlist::Direction::Output => outputs.push(entry),
                crate::netlist::Direction::Inout => {}
            }
        }
    }

    // A clock that is not a chip input has nowhere to come from: the chip's
    // only clock pin is the programming clock.
    for clock in &clocks {
        if !inputs.iter().any(|p| p.net == *clock) {
            let name = names.get(*clock).unwrap_or("<unnamed>");
            return Err(PackError {
                loc: None,
                message: format!(
                    "clock net \"{}\" does not come from a chip input. The fabric has no clock \
                     pin: every column clock is derived from a signal routed onto a vertical \
                     ring, so a clock must originate at an input pad.",
                    name
                ),
            });
        }
    }

    Ok(PackedDesign {
        top: top.to_string(),
        cells: logic,
        chains,
        inputs,
        outputs,
        clocks,
        reset: resets.into_iter().next(),
        names,
        warnings,
    })
}

fn display(name: &str) -> String {
    name.trim_start_matches('\\').to_string()
}
