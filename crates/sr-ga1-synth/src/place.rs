//! Pin assignment and placement, solved together.
//!
//! The IO positions are hard-wired — a given chip input arrives on exactly
//! one row and one lane — so which pin a signal uses decides which row its
//! consumers want to be in. That makes pin choice a variable in the same
//! optimisation as placement rather than a formality, and the cost function
//! below prices them against each other.
//!
//! The cost model is shaped by one dominant fact: **horizontal lanes flow
//! left to right and never turn.** A signal that has to reach a different
//! row, or move leftward, must pass through a CLB's operation core onto a
//! vertical ring, which costs a whole CLB. So a connection is nearly free if
//! its driver sits in the same row and to the left of its consumer, and
//! expensive otherwise. Everything else — lane-3 enables wanting row 3,
//! constants wanting the major lanes, congestion per row — is a smaller
//! correction on top.
//!
//! Annealing runs are milliseconds at these sizes, so the search is many
//! short runs from different seeds rather than one long one.

use crate::carry::CarryPlan;
use crate::constraints::{Constraints, LoopbackPool};
use crate::genlib::{CellLibrary, PhysIn};
use crate::pack::{Enable, PackedDesign, Signal};
use crate::rrg::Node;
use crate::fabric::{Fabric, Source};
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// Shared empty constraints, so `Target::new` needs no caller-owned value.
static EMPTY_CONSTRAINTS: OnceLock<Constraints> = OnceLock::new();

/// Deterministic generator, so a seed always reproduces a run. Nothing here
/// needs statistical quality, only repeatability.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        // Avoid the zero state, which would stick.
        Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1)
    }
    fn next_u64(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    pub fn below(&mut self, limit: usize) -> usize {
        if limit == 0 {
            0
        } else {
            (self.next_u64() % limit as u64) as usize
        }
    }
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Where one packed cell sits and how it is built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    pub col: usize,
    pub row: usize,
    /// Index into the library cell's implementation list.
    pub imp: usize,
    /// Index into its symmetry group: which gate pin lands on which input.
    pub perm: usize,
}

/// What one physical CLB input must be given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operand {
    /// Carries a design signal.
    Signal(Signal),
    /// Must be tied to a constant.
    Const(bool),
    /// Comes from the dedicated carry chain, not from routing at all.
    CarryIn,
    /// The operation ignores it; any legal mux code will do.
    DontCare,
}

#[derive(Debug, Clone)]
pub struct Placement {
    /// One entry per cell of the packed design, in the same order.
    pub cells: Vec<Placed>,
    /// Design input net -> chip input pad name.
    pub input_pins: BTreeMap<u32, String>,
    /// Design output net -> chip output pad name.
    pub output_pins: BTreeMap<u32, String>,
    /// Column whose CSB sources each clock net.
    pub clock_columns: BTreeMap<u32, usize>,
}

impl Placement {
    pub fn cell_at(&self, col: usize, row: usize) -> Option<usize> {
        self.cells.iter().position(|p| p.col == col && p.row == row)
    }

    /// Which clock reaches each column.
    ///
    /// A CSB either taps its own column's ring or couples to the previous
    /// column, and column 0 couples from the last one, so the coupling chain is
    /// a ring. Each uncoupled CSB therefore owns a contiguous cyclic block of
    /// columns: the one it sits in, plus every column after it until the next
    /// source. That block is the clock domain, and a register only ever sees the
    /// clock of the column it sits in.
    pub fn clock_domains(&self, columns: usize) -> Vec<Option<u32>> {
        let mut source_at: Vec<Option<u32>> = vec![None; columns];
        for (clock, &column) in &self.clock_columns {
            if column < columns {
                source_at[column] = Some(*clock);
            }
        }
        let mut out = vec![None; columns];
        // Walk twice round the ring, so a column before the first source picks
        // up the last one.
        let mut current = None;
        for step in 0..columns * 2 {
            let column = step % columns;
            if let Some(clock) = source_at[column] {
                current = Some(clock);
            }
            out[column] = current;
        }
        out
    }

    /// The clock a register in this column would be driven by.
    pub fn clock_of_column(&self, columns: usize, col: usize) -> Option<u32> {
        self.clock_domains(columns).get(col).copied().flatten()
    }

    /// What each physical input of a placed cell must carry.
    pub fn operands(
        &self,
        design: &PackedDesign,
        target: &Target,
        cell: usize,
    ) -> Vec<Operand> {
        let library = target.library;
        let placed = &self.cells[cell];
        let packed = &design.cells[cell];

        // An adder bit does not come from the library: its third input is the
        // carry chain, which is not routable, and the addends are commutative
        // so the only choice is which one takes which input.
        if let (Some(link), Some(plan)) = (&packed.carry, target.carry) {
            let mut out = vec![Operand::DontCare; library.phys_inputs];
            let swap = placed.perm % 2 == 1;
            for (slot, &input) in plan.addend_inputs.iter().enumerate() {
                let pin = if swap { 1 - slot } else { slot };
                out[input] =
                    packed.pins.get(pin).copied().map(Operand::Signal).unwrap_or(Operand::DontCare);
            }
            out[plan.carry_input] = match link.carry_in {
                // The least significant bit may need a carry-in the tied-off
                // chain input cannot supply, and then it has to be routed.
                Some(signal) if link.index == 0 => Operand::Signal(signal),
                _ => Operand::CarryIn,
            };
            return out;
        }

        let Some(gate) = library.cell(&packed.gate) else {
            return vec![Operand::DontCare; library.phys_inputs];
        };
        let imp = &gate.impls[placed.imp.min(gate.impls.len() - 1)];
        let perm = &gate.symmetries[placed.perm.min(gate.symmetries.len() - 1)];
        imp.inputs
            .iter()
            .map(|phys| match phys {
                // Pin k of the implementation carries gate pin perm[k]; the
                // permutation is a symmetry of the function, so the result is
                // unchanged.
                PhysIn::Pin(k) => {
                    let pin = perm.get(*k).copied().unwrap_or(*k);
                    packed
                        .pins
                        .get(pin)
                        .copied()
                        .map(Operand::Signal)
                        .unwrap_or(Operand::DontCare)
                }
                PhysIn::Const(v) => Operand::Const(*v),
                PhysIn::DontCare => Operand::DontCare,
            })
            .collect()
    }
}

/// Everything placement needs to know about the target that is not in the
/// packed design: where pads are, which lanes carry constants, and which
/// input mux can see which lane.
pub struct Target<'a> {
    pub fabric: &'a Fabric,
    pub library: &'a CellLibrary,
    /// How to build an adder bit, when the fabric has a usable carry chain.
    pub carry: Option<&'a CarryPlan>,
    /// Everything the user pinned down: pin locks, DDIO bindings, instance
    /// placement, clock assignment and the loop-around pool.
    pub constraints: &'a Constraints,
    /// Input pad name -> (row, lane).
    pub input_pads: Vec<(String, usize, usize)>,
    /// Output pad name -> (row, lane).
    pub output_pads: Vec<(String, usize, usize)>,
}

impl<'a> Target<'a> {
    pub fn new(fabric: &'a Fabric, library: &'a CellLibrary) -> Target<'a> {
        Target::with_carry(
            fabric,
            library,
            None,
            EMPTY_CONSTRAINTS.get_or_init(Constraints::default),
        )
    }

    pub fn with_carry(
        fabric: &'a Fabric,
        library: &'a CellLibrary,
        carry: Option<&'a CarryPlan>,
        constraints: &'a Constraints,
    ) -> Target<'a> {
        // DDIO pads are never assigned automatically. They are only used when
        // a design declares them in the constraints file, naming the input,
        // output and direction nets — and `ddio_dir` in particular steers a
        // pad rather than carrying a design signal, so handing an ordinary
        // output to it would silently reconfigure a pin.
        let ddio: Vec<&str> = fabric
            .ddio
            .iter()
            .flat_map(|d| [d.input.as_str(), d.output.as_str(), d.dir.as_str()])
            .collect();

        // An input pad in the loop-around pool is driven by a board wire, so a
        // design input cannot also use it. Output pads are different: a loop
        // only taps one, so it may still carry a design output.
        let looped_in = constraints.loopback.reserved_inputs();
        // A pad the user locked, or one a DDIO declaration claims, is assigned
        // by that constraint rather than chosen here.
        let locked: Vec<&str> = constraints.pins.values().map(|p| p.name.as_str()).collect();
        let ddio_bound = constraints.ddio_pads();
        let unavailable = |name: &str| {
            ddio.contains(&name)
                || looped_in.contains(&name)
                || locked.contains(&name)
                || ddio_bound.contains(&name)
        };

        let mut input_pads = Vec::new();
        for (row, lanes) in fabric.io_inputs.iter().enumerate() {
            for (lane, name) in lanes.iter().enumerate() {
                // The reserved constant names are not assignable pins.
                if *name == fabric.naming.constant_zero || *name == fabric.naming.constant_one {
                    continue;
                }
                if unavailable(name) {
                    continue;
                }
                input_pads.push((name.clone(), row, lane));
            }
        }
        let mut output_pads = Vec::new();
        for (row, lanes) in fabric.io_outputs.iter().enumerate() {
            for (lane, name) in lanes.iter().enumerate() {
                if let Some(name) = name {
                    if unavailable(name) {
                        continue;
                    }
                    output_pads.push((name.clone(), row, lane));
                }
            }
        }
        Target { fabric, library, carry, constraints, input_pads, output_pads }
    }

    /// Board wiring available as routing.
    pub fn loopback(&self) -> &LoopbackPool {
        &self.constraints.loopback
    }

    /// The pad a design signal must use, from a pin lock or a DDIO binding.
    pub fn forced_pad(&self, signal: &str) -> Option<&crate::constraints::Pad> {
        if let Some(pad) = self.constraints.pin_for(signal) {
            return Some(pad);
        }
        self.constraints.ddio.iter().find_map(|d| {
            if d.input == signal {
                Some(&d.in_pad)
            } else if d.output == signal {
                Some(&d.out_pad)
            } else if d.dir == signal {
                Some(&d.dir_pad)
            } else {
                None
            }
        })
    }

    /// Where a pad physically is.
    ///
    /// Resolved against the fabric, **not** against the assignable lists: a pad
    /// that a pin lock, a DDIO binding or the loop-around pool has claimed is
    /// deliberately absent from those lists, but it still has a position and the
    /// signal on it still has to be routed there.
    pub fn input_pad(&self, name: &str) -> Option<(usize, usize)> {
        for (row, lanes) in self.fabric.io_inputs.iter().enumerate() {
            for (lane, pad) in lanes.iter().enumerate() {
                if pad == name {
                    return Some((row, lane));
                }
            }
        }
        None
    }
    pub fn output_pad(&self, name: &str) -> Option<(usize, usize)> {
        for (row, lanes) in self.fabric.io_outputs.iter().enumerate() {
            for (lane, pad) in lanes.iter().enumerate() {
                if pad.as_deref() == Some(name) {
                    return Some((row, lane));
                }
            }
        }
        None
    }

    /// Horizontal lanes a given input mux can select.
    fn lanes_of_input(&self, mux: usize) -> Vec<usize> {
        self.fabric.input_muxes[mux]
            .sources
            .iter()
            .filter_map(|s| match s {
                Source::HorzIn(l) => Some(*l),
                _ => None,
            })
            .collect()
    }

    /// Whether a constant can be delivered to this input without spending a
    /// CLB on manufacturing one. See `Fabric::constant_reaches_input`.
    pub fn input_reaches_constant(&self, mux: usize, col: usize, row: usize, value: bool) -> bool {
        self.fabric.constant_reaches_input(mux, col, row, value)
    }
}

/// How much a connection that cannot run straight along a row costs, in the
/// same units as a lane hop. One vertical detour spends a CLB.
const HOP: f64 = 30.0;
/// An unconditional register outside the row whose lane 3 enters as a
/// constant 1 needs an upstream CLB to drive its enable.
const ENABLE_OFF_FREE_ROW: f64 = 12.0;
/// A constant the input mux cannot reach from a major lane.
const UNREACHABLE_CONSTANT: f64 = 40.0;
/// A connection the fabric simply cannot make from where the cell has been
/// put. Large enough to dominate, finite so annealing can still climb out.
const INFEASIBLE: f64 = 400.0;
/// What a loop-around wire costs: no CLB, but a scarce input pad and a physical
/// connection that has to exist. Matches `route::edge_base`.
const LOOPBACK: f64 = 20.0;

pub struct Placer<'a> {
    design: &'a PackedDesign,
    target: &'a Target<'a>,
    /// Lane that gates the flip-flop.
    enable_lane: Option<usize>,
    /// Row whose enable segment enters as a constant 1, if any.
    free_enable_row: Option<usize>,
}

impl<'a> Placer<'a> {
    pub fn new(design: &'a PackedDesign, target: &'a Target<'a>) -> Placer<'a> {
        let enable_lane = match target.fabric.ff.enable {
            Source::HorzIn(lane) => Some(lane),
            _ => None,
        };
        let free_enable_row = enable_lane.and_then(|lane| {
            (0..target.fabric.rows).find(|&row| target.fabric.io_constant(row, lane) == Some(true))
        });
        Placer { design, target, enable_lane, free_enable_row }
    }

    /// Whether a register placed here can be held enabled at all.
    ///
    /// The enable is a segment like any other, so the constant 1 it needs comes
    /// either from the IO map at the left edge or from an upstream CLB's major
    /// mux — and at column 0 there is no upstream CLB. A register in column 0
    /// outside the free-enable row simply cannot capture.
    fn enable_reachable(&self, col: usize, row: usize) -> bool {
        let Some(lane) = self.enable_lane else { return false };
        if self.target.fabric.io_constant(row, lane) == Some(true) {
            return true;
        }
        self.target.fabric.const_capable_horz_lanes().contains(&lane) && col > 0
    }

    /// The row whose registers capture for free, reported so the caller can
    /// explain the placement.
    pub fn free_enable_row(&self) -> Option<usize> {
        self.free_enable_row
    }

    /// Anneal from one seed. Returns the best placement found.
    pub fn anneal(&self, seed: u64, sweeps: usize) -> (Placement, f64) {
        let mut rng = Rng::new(seed);
        let mut state = self.initial(&mut rng);
        let mut cost = self.cost(&state);
        let mut best = state.clone();
        let mut best_cost = cost;

        let cells = self.design.cells.len();
        if cells == 0 {
            return (state, cost);
        }
        let moves_per_sweep = (cells * 8).max(24);
        let mut temperature = cost.max(1.0) / 4.0;

        for _ in 0..sweeps {
            for _ in 0..moves_per_sweep {
                let Some(undo) = self.perturb(&mut state, &mut rng) else { continue };
                let candidate = self.cost(&state);
                let delta = candidate - cost;
                if delta <= 0.0 || rng.unit() < (-delta / temperature).exp() {
                    cost = candidate;
                    if cost < best_cost {
                        best_cost = cost;
                        best = state.clone();
                    }
                } else {
                    undo.apply(&mut state, &self.design.chains);
                }
            }
            temperature *= 0.92;
            if temperature < 1e-3 {
                break;
            }
        }
        (best, best_cost)
    }

    fn initial(&self, rng: &mut Rng) -> Placement {
        let fabric = self.target.fabric;
        let slot_of = |col: usize, row: usize| row * fabric.columns + col;
        let mut taken = vec![false; fabric.columns * fabric.rows];
        let mut cells: Vec<Option<Placed>> = vec![None; self.design.cells.len()];

        // Cells the user pinned go first: they have no freedom at all.
        for cell in 0..self.design.cells.len() {
            if let Some((col, row)) = self.locked_position(cell) {
                taken[slot_of(col, row)] = true;
                cells[cell] = Some(Placed { col, row, imp: 0, perm: 0 });
            }
        }

        // Carry chains next, because they are the only other cells with no
        // freedom of their own: a chain occupies consecutive ascending rows of
        // a single column, starting at the row whose carry input is tied off.
        let lsb_row = self.target.carry.map(|p| p.lsb_row).unwrap_or(0);
        for chain in &self.design.chains {
            // A lock on any member fixes the whole chain's column, since a
            // chain cannot straddle columns.
            let locked_column = chain.iter().find_map(|&cell| {
                self.locked_position(cell).map(|(col, _)| col)
            });
            let start = rng.below(fabric.columns.max(1));
            let column = locked_column.unwrap_or_else(|| {
                (0..fabric.columns)
                    .map(|k| (start + k) % fabric.columns)
                    .find(|&col| {
                        (0..chain.len())
                            .all(|i| lsb_row + i < fabric.rows && !taken[slot_of(col, lsb_row + i)])
                    })
                    .unwrap_or(0)
            });
            for (index, &cell) in chain.iter().enumerate() {
                let row = (lsb_row + index).min(fabric.rows - 1);
                taken[slot_of(column, row)] = true;
                cells[cell] = Some(Placed { col: column, row, imp: 0, perm: 0 });
            }
        }

        let mut free: Vec<(usize, usize)> = (0..fabric.rows)
            .flat_map(|row| (0..fabric.columns).map(move |col| (col, row)))
            .filter(|&(col, row)| !taken[slot_of(col, row)])
            .collect();
        // Shuffle deterministically.
        for i in (1..free.len()).rev() {
            free.swap(i, rng.below(i + 1));
        }

        for (index, packed) in self.design.cells.iter().enumerate() {
            if cells[index].is_some() {
                continue;
            }
            // Prefer the free-enable row for unconditional registers.
            let wants_row = match (&packed.reg, self.free_enable_row) {
                (Some(r), Some(row)) if r.enable == Enable::Always => Some(row),
                _ => None,
            };
            let pick = wants_row
                .and_then(|row| free.iter().position(|&(_, r)| r == row))
                .unwrap_or(0);
            let (col, row) = if free.is_empty() { (0, 0) } else { free.remove(pick) };
            cells[index] = Some(Placed { col, row, imp: 0, perm: 0 });
        }

        let cells: Vec<Placed> = cells
            .into_iter()
            .map(|c| c.unwrap_or(Placed { col: 0, row: 0, imp: 0, perm: 0 }))
            .collect();

        let mut placement = Placement {
            cells,
            input_pins: BTreeMap::new(),
            output_pins: BTreeMap::new(),
            clock_columns: BTreeMap::new(),
        };
        self.assign_pins(&mut placement);
        // Each domain needs its own CSB, so the source columns must be
        // distinct. Locked ones are honoured first, then the rest take the
        // lowest free column.
        let mut taken_columns: Vec<usize> = Vec::new();
        for clock in &self.design.clocks {
            if let Some(column) = self.locked_clock_column(*clock) {
                placement.clock_columns.insert(*clock, column);
                taken_columns.push(column);
            }
        }
        for clock in &self.design.clocks {
            if placement.clock_columns.contains_key(clock) {
                continue;
            }
            let column = (0..fabric.columns)
                .find(|c| !taken_columns.contains(c))
                .unwrap_or(0);
            taken_columns.push(column);
            placement.clock_columns.insert(*clock, column);
        }
        placement
    }

    /// The column the user named as this clock's source, if any.
    fn locked_clock_column(&self, clock: u32) -> Option<usize> {
        let name = self.design.names.get(clock)?;
        self.target.constraints.clocks.get(name).copied()
    }

    /// Whether a net's pad was decided by a constraint rather than chosen.
    fn pin_is_forced(&self, net: u32) -> bool {
        let named = self
            .design
            .inputs
            .iter()
            .chain(&self.design.outputs)
            .find(|p| p.net == net)
            .map(|p| p.name.as_str());
        named.is_some_and(|name| self.target.forced_pad(name).is_some())
    }

    /// The position a cell was pinned to, if the user pinned it.
    fn locked_position(&self, cell: usize) -> Option<(usize, usize)> {
        let name = &self.design.cells[cell].name;
        self.target
            .constraints
            .placement
            .iter()
            .find(|lock| lock.name == *name)
            .map(|lock| (lock.col, lock.row))
    }

    /// Chain this cell belongs to, if any.
    fn chain_of(&self, cell: usize) -> Option<usize> {
        self.design.cells[cell].carry.as_ref().map(|link| link.chain)
    }

    /// Slide a whole chain to another column, keeping each cell's row. Returns
    /// false if that column cannot hold it.
    fn move_chain(&self, state: &mut Placement, chain: usize, column: usize) -> bool {
        let members = &self.design.chains[chain];
        // A pinned member pins the column for all of them.
        if members.iter().any(|&cell| self.locked_position(cell).is_some()) {
            return false;
        }
        for &cell in members {
            let row = state.cells[cell].row;
            if let Some(occupant) = state.cell_at(column, row) {
                if occupant != cell && self.chain_of(occupant) != Some(chain) {
                    return false;
                }
            }
        }
        for &cell in members {
            state.cells[cell].col = column;
        }
        true
    }

    /// Give every design port a pad, preferring one in the row where its
    /// traffic already is. Pads are taken greedily in a fixed order, so the
    /// result is deterministic.
    fn assign_pins(&self, placement: &mut Placement) {
        let mut taken: Vec<String> = Vec::new();
        for port in &self.design.inputs {
            // Reset is a dedicated pin distributed in hardware, so it takes
            // no routable pad and no routing resources at all.
            if self.design.reset == Some(port.net) {
                continue;
            }
            // A pin lock or a DDIO binding decides this one.
            if let Some(pad) = self.target.forced_pad(&port.name) {
                placement.input_pins.insert(port.net, pad.name.clone());
                continue;
            }
            let wanted_row = self
                .design
                .cells
                .iter()
                .enumerate()
                .find(|(_, c)| c.pins.contains(&Signal::Net(port.net)))
                .map(|(i, _)| placement.cells[i].row);
            let pad = self
                .target
                .input_pads
                .iter()
                .filter(|(name, _, _)| !taken.contains(name))
                .min_by_key(|(_, row, lane)| {
                    let row_miss = wanted_row.map(|w| w.abs_diff(*row)).unwrap_or(0);
                    (row_miss, *lane)
                })
                .map(|(name, _, _)| name.clone());
            if let Some(pad) = pad {
                taken.push(pad.clone());
                placement.input_pins.insert(port.net, pad);
            }
        }
        let mut taken_out: Vec<String> = Vec::new();
        for port in &self.design.outputs {
            if let Some(pad) = self.target.forced_pad(&port.name) {
                placement.output_pins.insert(port.net, pad.name.clone());
                continue;
            }
            let driver_row = self
                .design
                .cells
                .iter()
                .enumerate()
                .find(|(_, c)| c.op == Some(port.net) || c.reg.as_ref().is_some_and(|r| r.q == port.net))
                .map(|(i, _)| placement.cells[i].row);
            let pad = self
                .target
                .output_pads
                .iter()
                .filter(|(name, _, _)| !taken_out.contains(name))
                .min_by_key(|(_, row, lane)| {
                    let row_miss = driver_row.map(|w| w.abs_diff(*row)).unwrap_or(0);
                    (row_miss, *lane)
                })
                .map(|(name, _, _)| name.clone());
            if let Some(pad) = pad {
                taken_out.push(pad.clone());
                placement.output_pins.insert(port.net, pad);
            }
        }
    }

    fn perturb(&self, state: &mut Placement, rng: &mut Rng) -> Option<Undo> {
        if state.cells.is_empty() {
            return None;
        }
        let fabric = self.target.fabric;
        match rng.below(14) {
            // Move a cell to a free position. A chain member has no freedom of
            // its own — its row is fixed by its significance — so the whole
            // chain slides sideways together instead.
            0..=4 => {
                let cell = rng.below(state.cells.len());
                if self.locked_position(cell).is_some() {
                    return None;
                }
                if let Some(chain) = self.chain_of(cell) {
                    let before: Vec<Placed> = self.design.chains[chain]
                        .iter()
                        .map(|&c| state.cells[c].clone())
                        .collect();
                    let column = rng.below(fabric.columns);
                    if !self.move_chain(state, chain, column) {
                        return None;
                    }
                    return Some(Undo::Chain(chain, before));
                }
                let col = rng.below(fabric.columns);
                let row = rng.below(fabric.rows);
                if state.cell_at(col, row).is_some() {
                    return None;
                }
                let before = state.cells[cell].clone();
                state.cells[cell].col = col;
                state.cells[cell].row = row;
                Some(Undo::Cell(cell, before))
            }
            // Swap two cells. Chain members are excluded: swapping one out of
            // its row would break the chain's ascending order.
            5..=6 => {
                let a = rng.below(state.cells.len());
                let b = rng.below(state.cells.len());
                if a == b
                    || self.chain_of(a).is_some()
                    || self.chain_of(b).is_some()
                    || self.locked_position(a).is_some()
                    || self.locked_position(b).is_some()
                {
                    return None;
                }
                let before_a = state.cells[a].clone();
                let before_b = state.cells[b].clone();
                let (ca, ra) = (state.cells[a].col, state.cells[a].row);
                state.cells[a].col = state.cells[b].col;
                state.cells[a].row = state.cells[b].row;
                state.cells[b].col = ca;
                state.cells[b].row = ra;
                Some(Undo::Pair(a, before_a, b, before_b))
            }
            // Try another way of building the cell.
            7..=8 => {
                let cell = rng.below(state.cells.len());
                let gate = self.target.library.cell(&self.design.cells[cell].gate)?;
                let before = state.cells[cell].clone();
                state.cells[cell].imp = rng.below(gate.impls.len());
                state.cells[cell].perm = rng.below(gate.symmetries.len());
                Some(Undo::Cell(cell, before))
            }
            // Reassign a chip pin. Pin choice is a real variable here, not a
            // formality: the IO positions are fixed, so which pad a signal
            // takes decides which row its consumers want to sit in. A greedy
            // first pass will happily give a row-2 pad to a signal whose
            // consumer is in row 3 and strand the signal that needed it.
            9..=12 => {
                let inputs = !state.input_pins.is_empty();
                let outputs = !state.output_pins.is_empty();
                if !inputs && !outputs {
                    return None;
                }
                let before = (state.input_pins.clone(), state.output_pins.clone());
                let do_inputs = inputs && (!outputs || rng.below(4) != 0);
                let (pins, pads): (&mut BTreeMap<u32, String>, &[(String, usize, usize)]) =
                    if do_inputs {
                        (&mut state.input_pins, &self.target.input_pads)
                    } else {
                        (&mut state.output_pins, &self.target.output_pads)
                    };
                let nets: Vec<u32> = pins.keys().copied().collect();
                let net = nets[rng.below(nets.len())];
                if self.pin_is_forced(net) {
                    return None;
                }
                let pad = pads[rng.below(pads.len())].0.clone();
                // If another net holds that pad, trade; otherwise just take it.
                let holder = pins.iter().find(|(_, p)| **p == pad).map(|(n, _)| *n);
                let current = pins.get(&net).cloned();
                pins.insert(net, pad);
                match (holder, current) {
                    (Some(other), Some(freed)) if other != net => {
                        pins.insert(other, freed);
                    }
                    _ => {}
                }
                Some(Undo::Pins(before.0, before.1))
            }
            // Move a clock to another column.
            _ => {
                let clocks: Vec<u32> = state.clock_columns.keys().copied().collect();
                if clocks.is_empty() {
                    return None;
                }
                let clock = clocks[rng.below(clocks.len())];
                if self.locked_clock_column(clock).is_some() {
                    return None;
                }
                let column = rng.below(fabric.columns);
                // One CSB sources one domain.
                if state
                    .clock_columns
                    .iter()
                    .any(|(other, &taken)| *other != clock && taken == column)
                {
                    return None;
                }
                let before = state.clock_columns.clone();
                state.clock_columns.insert(clock, column);
                Some(Undo::Clocks(before))
            }
        }
    }

    /// The cost of a placement: how much routing it implies.
    pub fn cost(&self, state: &Placement) -> f64 {
        let mut total = 0.0;

        // Where each net is driven from.
        let mut driver: BTreeMap<u32, (usize, usize)> = BTreeMap::new();
        for (index, cell) in self.design.cells.iter().enumerate() {
            let placed = &state.cells[index];
            if let Some(net) = cell.op {
                driver.insert(net, (placed.col, placed.row));
            }
            if let Some(reg) = &cell.reg {
                driver.insert(reg.q, (placed.col, placed.row));
            }
        }
        // Chip inputs are driven at the left edge of their pad's row.
        let mut pad_row: BTreeMap<u32, usize> = BTreeMap::new();
        for (net, pad) in &state.input_pins {
            if let Some((row, _)) = self.target.input_pad(pad) {
                pad_row.insert(*net, row);
            }
        }

        for (index, cell) in self.design.cells.iter().enumerate() {
            let placed = &state.cells[index];
            let operands = state.operands(self.design, self.target, index);

            for (mux, operand) in operands.iter().enumerate() {
                match operand {
                    Operand::Signal(Signal::Net(net)) => {
                        total += self.connection_cost(
                            *net,
                            placed,
                            &driver,
                            &pad_row,
                            self.input_reaches_major(mux),
                        );
                        // An operand can only arrive on a lane this mux sees;
                        // a mux with a narrow window is harder to feed.
                        let window = self.target.lanes_of_input(mux).len() as f64;
                        total += 1.0 / window.max(1.0);
                    }
                    Operand::Signal(Signal::Const(v)) | Operand::Const(v) => {
                        if self.target.input_reaches_constant(mux, placed.col, placed.row, *v) {
                            total += 0.5;
                        } else {
                            total += UNREACHABLE_CONSTANT;
                        }
                    }
                    Operand::CarryIn | Operand::DontCare => {}
                }
            }

            // Lane 3 enables.
            if let Some(reg) = &cell.reg {
                match &reg.enable {
                    Enable::Always => {
                        if !self.enable_reachable(placed.col, placed.row) {
                            total += INFEASIBLE;
                        } else if Some(placed.row) != self.free_enable_row {
                            total += ENABLE_OFF_FREE_ROW;
                        }
                    }
                    Enable::Net(net) => {
                        // The enable is a major lane, which a vertical bridge
                        // reaches directly.
                        total += self.connection_cost(*net, placed, &driver, &pad_row, true);
                    }
                }
            }
        }

        // Design outputs must leave on their pad's row, at its lane.
        for (net, pad) in &state.output_pins {
            let Some((pad_row, _)) = self.target.output_pad(pad) else { continue };
            let Some(&(_, driver_row)) = driver.get(net) else { continue };
            if pad_row != driver_row {
                total += HOP + pad_row.abs_diff(driver_row) as f64;
            }
        }

        total += self.chain_penalty(state);
        total += self.clock_cost(state, &pad_row);
        total += self.clock_domain_cost(state);

        // Rough congestion: how many cells share each row and column.
        let mut per_row = vec![0.0; self.target.fabric.rows];
        let mut per_col = vec![0.0; self.target.fabric.columns];
        for placed in &state.cells {
            per_row[placed.row] += 1.0;
            per_col[placed.col] += 1.0;
        }
        let lanes = self.target.fabric.horz_lanes as f64;
        for count in per_row {
            if count > lanes {
                total += (count - lanes) * 4.0;
            }
        }
        for count in per_col {
            if count > self.target.fabric.rows as f64 {
                total += 4.0;
            }
        }

        total
    }

    /// What it costs to get a net from where it is driven to a consumer.
    fn connection_cost(
        &self,
        net: u32,
        sink: &Placed,
        driver: &BTreeMap<u32, (usize, usize)>,
        pad_row: &BTreeMap<u32, usize>,
        // Whether the destination can be reached by a signal arriving from
        // another row: such a signal comes back onto the horizontal buses
        // through a major mux, so it lands only on a major lane.
        reaches_major: bool,
    ) -> f64 {
        // Column 0's incoming segments are driven by the IO controller and by
        // nothing else, so a cell there can only be fed by the pad entering on
        // its own row. No amount of routing changes that.
        let unfeedable_at_edge = |source_row: Option<usize>| {
            sink.col == 0 && source_row != Some(sink.row)
        };
        // An input that cannot see a major lane needs a further CLB to move a
        // cross-row signal onto its minor lane.
        let cross_row_extra = if reaches_major { 0.0 } else { HOP };
        // A cross-row signal needs columns to the *left* of its destination to
        // stage through: one CLB to drive it onto a vertical ring, and another
        // to bring it back onto a horizontal lane. A destination hard against
        // the left edge has nowhere to do that, and the only ring it can reach
        // is the one column whose CLBs are also the row's pass-throughs — which
        // is how several nets end up fighting over a single operation core.
        let staging = sink.col as f64;
        let mut staging_extra = (2.0 - staging).max(0.0) * HOP;
        let mut cross_row = HOP + cross_row_extra;
        // A loop-around wire crosses rows and moves leftward for the price of an
        // input pad and no CLB at all, so once the board has some, the placer
        // should stop contorting itself to avoid what is now cheap.
        if !self.target.loopback().is_empty() {
            cross_row = cross_row.min(LOOPBACK);
            staging_extra = staging_extra.min(LOOPBACK);
        }

        if let Some(&(col, row)) = driver.get(&net) {
            if unfeedable_at_edge(None) {
                return INFEASIBLE;
            }
            if row == sink.row && col < sink.col {
                // Straight along the row: the cheap case.
                (sink.col - col) as f64 * 0.25
            } else {
                // Anything else needs a vertical detour through a CLB.
                cross_row + staging_extra + row.abs_diff(sink.row) as f64
            }
        } else if let Some(&row) = pad_row.get(&net) {
            if unfeedable_at_edge(Some(row)) {
                return INFEASIBLE;
            }
            if row == sink.row {
                sink.col as f64 * 0.25
            } else {
                cross_row + staging_extra + row.abs_diff(sink.row) as f64
            }
        } else {
            // Not driven by anything placed: a constant or an unused net.
            0.0
        }
    }

    /// Whether an operand arriving from another row can land on a lane this
    /// input mux can see. Only the major lanes are reachable from the vertical
    /// ring, so an input that sees none of them needs an extra CLB.
    fn input_reaches_major(&self, mux: usize) -> bool {
        let major = self.target.fabric.const_capable_horz_lanes();
        self.target.lanes_of_input(mux).iter().any(|lane| major.contains(lane))
    }

    /// A register must sit in a column its own clock reaches.
    ///
    /// With one clock domain every column qualifies and this is free. With more
    /// than one it is a hard constraint: the CSB coupling chain gives each
    /// domain a contiguous block of columns, and a register placed outside its
    /// block would be clocked by the wrong signal — which no amount of routing
    /// can fix.
    fn clock_domain_cost(&self, state: &Placement) -> f64 {
        if self.design.clocks.len() < 2 {
            return 0.0;
        }
        let columns = self.target.fabric.columns;
        let domains = state.clock_domains(columns);
        let mut total = 0.0;

        // Two domains cannot be sourced from one column.
        let mut sources: Vec<usize> = state.clock_columns.values().copied().collect();
        sources.sort_unstable();
        let distinct = {
            let mut unique = sources.clone();
            unique.dedup();
            unique.len()
        };
        total += (sources.len() - distinct) as f64 * INFEASIBLE;

        for (index, cell) in self.design.cells.iter().enumerate() {
            let Some(reg) = &cell.reg else { continue };
            let column = state.cells[index].col;
            if domains.get(column).copied().flatten() != Some(reg.clock) {
                total += INFEASIBLE;
            }
        }
        total
    }

    /// What each clock's column choice costs.
    ///
    /// A clock has to be driven onto its column's vertical ring, and the only
    /// horizontal-to-vertical path is through a CLB's operation core. So the
    /// chosen column needs a free CLB in the clock's own row that can also
    /// neutralise its idle inputs — which at column 0 it generally cannot,
    /// there being no upstream CLB to supply a constant.
    fn clock_cost(&self, state: &Placement, pad_row: &BTreeMap<u32, usize>) -> f64 {
        let mut total = 0.0;
        for clock in &self.design.clocks {
            let Some(&column) = state.clock_columns.get(clock) else { continue };
            let Some(&row) = pad_row.get(clock) else { continue };
            // The buffer that puts the clock on the ring lands here.
            if state.cell_at(column, row).is_some() {
                total += HOP;
            }
            let can_neutralise = (0..self.target.fabric.input_muxes.len())
                .any(|mux| self.target.input_reaches_constant(mux, column, row, false))
                || (0..self.target.fabric.input_muxes.len())
                    .any(|mux| self.target.input_reaches_constant(mux, column, row, true));
            if !can_neutralise {
                total += INFEASIBLE;
            }
        }
        total
    }

    /// Carry chains have no placement freedom beyond their column, so this is
    /// a safety net rather than a gradient: if the invariant is ever broken,
    /// the cost says so loudly instead of the router failing obscurely.
    fn chain_penalty(&self, state: &Placement) -> f64 {
        let Some(plan) = self.target.carry else { return 0.0 };
        let mut total = 0.0;
        for chain in &self.design.chains {
            let Some(&first) = chain.first() else { continue };
            let column = state.cells[first].col;
            for (index, &cell) in chain.iter().enumerate() {
                let placed = &state.cells[cell];
                if placed.col != column || placed.row != plan.lsb_row + index {
                    total += INFEASIBLE;
                }
            }
        }
        total
    }
}

enum Undo {
    Cell(usize, Placed),
    Pair(usize, Placed, usize, Placed),
    /// A whole carry chain, which only ever moves as a unit.
    Chain(usize, Vec<Placed>),
    Pins(BTreeMap<u32, String>, BTreeMap<u32, String>),
    Clocks(BTreeMap<u32, usize>),
}

impl Undo {
    fn apply(self, state: &mut Placement, chains: &[Vec<usize>]) {
        match self {
            Undo::Cell(index, before) => state.cells[index] = before,
            Undo::Chain(chain, before) => {
                for (&cell, placed) in chains[chain].iter().zip(before) {
                    state.cells[cell] = placed;
                }
            }
            Undo::Pair(a, before_a, b, before_b) => {
                state.cells[a] = before_a;
                state.cells[b] = before_b;
            }
            Undo::Pins(inputs, outputs) => {
                state.input_pins = inputs;
                state.output_pins = outputs;
            }
            Undo::Clocks(before) => state.clock_columns = before,
        }
    }
}

/// Node a design output must be driven onto.
pub fn output_node(fabric: &Fabric, row: usize, lane: usize) -> Node {
    Node::HSeg { row, col: fabric.columns, lane }
}
