//! The cell library, derived from `fabric.toml`.
//!
//! The fabric has no LUT: it has eight fixed functions over three selected
//! inputs, so technology mapping is standard-cell mapping. This module turns
//! the operation tables in the fabric description into two things:
//!
//! 1. a genlib ABC can map against, one gate per distinct *function*; and
//! 2. for each gate, every *physical* way to build it — which operation code,
//!    which physical input each gate pin sits on, and which physical inputs
//!    must be tied to a constant.
//!
//! The second list is what makes Phase 6 able to re-map a cell whose
//! constant turned out to be unroutable instead of failing the whole run.
//! Degrading on physical `c` is cheap (its mux reaches two major lanes),
//! degrading on `a` is expensive (its mux sees only the two minor lanes,
//! which carry no constant outside row 3), so implementations are ordered by
//! what their constants actually cost.
//!
//! Nothing here is hard-coded per operation: a fabric edit that changes,
//! adds or removes an operation regenerates the library.

use crate::fabric::{Fabric, Source};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// What one physical CLB input carries in a particular implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PhysIn {
    /// Gate pin `k` of the mapped cell.
    Pin(usize),
    /// Tied to a constant, which the router must deliver to this input.
    Const(bool),
    /// The operation ignores this input: any legal source will do, and no
    /// constant needs routing.
    DontCare,
}

/// One physical realisation of a gate: an operation code plus an assignment
/// for each physical input, in the fabric's input mux order (`a`, `b`, `c`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellImpl {
    pub op_code: u64,
    pub op_name: String,
    pub inputs: Vec<PhysIn>,
}

impl CellImpl {
    /// Physical inputs that must be driven with a constant.
    pub fn constants(&self) -> impl Iterator<Item = (usize, bool)> + '_ {
        self.inputs.iter().enumerate().filter_map(|(i, p)| match p {
            PhysIn::Const(v) => Some((i, *v)),
            _ => None,
        })
    }

    /// Physical input carrying a given gate pin.
    pub fn input_of_pin(&self, pin: usize) -> Option<usize> {
        self.inputs.iter().position(|p| *p == PhysIn::Pin(pin))
    }

    /// Whether a gate pin arrives on this physical input.
    pub fn takes_pin_on(&self, phys_input: usize) -> bool {
        matches!(self.inputs.get(phys_input), Some(PhysIn::Pin(_)))
    }
}

/// One distinct logic function, as ABC sees it.
#[derive(Debug, Clone)]
pub struct LogicCell {
    pub name: String,
    pub arity: usize,
    /// `table[i]` is the result when pin `k` takes bit `k` of `i`.
    pub table: Vec<bool>,
    /// Pin permutations that leave the function unchanged — the freedom the
    /// packer has when matching pins to the restricted input windows.
    pub symmetries: Vec<Vec<usize>>,
    /// Physical realisations, cheapest constants first.
    pub impls: Vec<CellImpl>,
}

impl LogicCell {
    /// Implementations that accept a live pin on `phys_input`, cheapest
    /// constants first.
    ///
    /// Which form to build a cell in is a routing-time question, not a fixed
    /// ranking: the best buffer is the one whose live pin already sits on the
    /// lane the signal arrived on. A chip input entering row 0 on lane 0 can
    /// only reach input `a`, and `OR3(a, 0, 0)` buffers it without the signal
    /// changing lane at all — whereas the globally cheapest form would move
    /// it onto a major lane first.
    pub fn impls_for_input(&self, phys_input: usize) -> Vec<&CellImpl> {
        self.impls.iter().filter(|i| i.takes_pin_on(phys_input)).collect()
    }

    /// Genlib formula for this function, in `*` `+` `!` only.
    pub fn expression(&self) -> String {
        sum_of_products(&self.table, self.arity)
    }

    /// Whether the function is monotone in a pin, which sets its genlib phase.
    fn pin_phase(&self, pin: usize) -> &'static str {
        let bit = 1 << pin;
        let mut rising = false;
        let mut falling = false;
        for i in 0..self.table.len() {
            if i & bit != 0 {
                continue;
            }
            match (self.table[i], self.table[i | bit]) {
                (false, true) => rising = true,
                (true, false) => falling = true,
                _ => {}
            }
        }
        match (rising, falling) {
            (true, false) => "NONINV",
            (false, true) => "INV",
            _ => "UNKNOWN",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CellLibrary {
    pub cells: Vec<LogicCell>,
    /// Number of physical inputs the operation core takes.
    pub phys_inputs: usize,
    /// Names of the physical inputs, in mux order.
    pub phys_names: Vec<String>,
}

impl CellLibrary {
    pub fn cell(&self, name: &str) -> Option<&LogicCell> {
        self.cells.iter().find(|c| c.name == name)
    }

    /// Build the library by cofactoring every operation against every way of
    /// tying its inputs to constants.
    pub fn derive(fabric: &Fabric) -> CellLibrary {
        let phys = fabric.input_muxes.len();
        let candidates = constant_lanes_per_input(fabric);

        // Keyed by (arity, truth table) so every way of building the same
        // function collapses into one gate with several implementations.
        let mut found: BTreeMap<(usize, Vec<bool>), Vec<CellImpl>> = BTreeMap::new();

        // Each physical input is either tied to a constant or driven by one of
        // the gate's pins — and **two physical inputs may share a pin**. That
        // last case is not a curiosity: `MUX2(p, p, -)` is a buffer needing no
        // constant at all, and it is the only way to buffer a signal in a
        // column or row where no constant can be reached. Enumerating only
        // distinct pins would miss it.
        const TIED_ZERO: usize = 0;
        const TIED_ONE: usize = 1;
        let first_pin = 2usize;
        let options = first_pin + phys;

        for op in &fabric.operations {
            for pattern in 0..options.pow(phys as u32) {
                let mut assign = Vec::with_capacity(phys);
                let mut p = pattern;
                for _ in 0..phys {
                    assign.push(p % options);
                    p /= options;
                }
                // Renumber the pins by first appearance, so the same wiring is
                // only ever discovered once.
                let mut pin_order: Vec<usize> = Vec::new();
                for &a in &assign {
                    if a >= first_pin && !pin_order.contains(&a) {
                        pin_order.push(a);
                    }
                }
                if pin_order.is_empty() {
                    continue; // every input tied: a constant, not a cell
                }
                let pin_of = |a: usize| pin_order.iter().position(|&p| p == a);

                // Cofactor: evaluate the operation with the ties applied and
                // the shared pins driven together.
                let arity = pin_order.len();
                let mut table = vec![false; 1 << arity];
                for (minterm, slot) in table.iter_mut().enumerate() {
                    let mut index = 0usize;
                    for (i, &a) in assign.iter().enumerate() {
                        let value = match a {
                            TIED_ZERO => false,
                            TIED_ONE => true,
                            _ => {
                                let pin = pin_of(a).unwrap_or(0);
                                minterm >> pin & 1 != 0
                            }
                        };
                        if value {
                            index |= 1 << i;
                        }
                    }
                    *slot = op.table[index];
                }

                // Pins the cofactor ignores cost nothing to drive.
                let used: Vec<bool> =
                    (0..arity).map(|pin| depends_on(&table, arity, pin)).collect();
                if !used.iter().any(|&u| u) {
                    continue; // a constant: no cell needed
                }
                // Dropping a pin the function ignores is only sound when one
                // physical input drives it. If two share it, they still have to
                // carry the *same* value — `XOR3(a, b, b)` is `a`, but only
                // while b and c are tied together, which "don't care" does not
                // say. Such a form needs a net routed to two inputs for no
                // benefit, so discard it: the useful shared-pin case, where the
                // shared pin is the signal itself, is unaffected.
                let sound = (0..arity).all(|pin| {
                    used[pin]
                        || assign.iter().filter(|&&a| pin_of(a) == Some(pin)).count() == 1
                });
                if !sound {
                    continue;
                }
                let (table, arity, kept) = restrict(&table, arity, &used);

                let mut inputs = Vec::with_capacity(phys);
                for &a in &assign {
                    inputs.push(match a {
                        TIED_ZERO => PhysIn::Const(false),
                        TIED_ONE => PhysIn::Const(true),
                        _ => {
                            let pin = pin_of(a).unwrap_or(0);
                            match kept.iter().position(|&k| k == pin) {
                                Some(new_pin) => PhysIn::Pin(new_pin),
                                None => PhysIn::DontCare,
                            }
                        }
                    });
                }
                let cell_impl =
                    CellImpl { op_code: op.code, op_name: op.name.clone(), inputs };
                let entry = found.entry((arity, table)).or_default();
                if !entry.contains(&cell_impl) {
                    entry.push(cell_impl);
                }
            }
        }

        let mut cells: Vec<LogicCell> = found
            .into_iter()
            .map(|((arity, table), mut impls)| {
                impls.sort_by_key(|i| (impl_cost(i, &candidates), i.op_code, i.inputs.clone()));
                LogicCell {
                    name: cell_name(fabric, arity, &table),
                    arity,
                    symmetries: symmetries(&table, arity),
                    table,
                    impls,
                }
            })
            .collect();
        cells.sort_by(|a, b| (a.arity, &a.name).cmp(&(b.arity, &b.name)));

        CellLibrary {
            cells,
            phys_inputs: phys,
            phys_names: fabric.input_muxes.iter().map(|m| m.name.clone()).collect(),
        }
    }

    /// Render `fabric.genlib`. Every logic gate costs exactly one CLB, so
    /// ABC minimising area is ABC minimising CLB count.
    pub fn genlib(&self, fabric: &Fabric) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# Generated from {} by sr-ga1-synth.", fabric.path.display());
        let _ = writeln!(out, "# Fabric \"{}\": one gate = one CLB, so area is CLB count.", fabric.name);
        let _ = writeln!(out, "# Do not edit; regenerate with --keep-intermediates.");
        let _ = writeln!(out);
        let _ = writeln!(out, "GATE zero  0 O=CONST0;");
        let _ = writeln!(out, "GATE one   0 O=CONST1;");
        let _ = writeln!(out);
        for cell in &self.cells {
            let _ = writeln!(out, "GATE {:<10} 1 O={};", cell.name, cell.expression());
            for pin in 0..cell.arity {
                let _ = writeln!(
                    out,
                    "  PIN {} {} 1 999 1 0 1 0",
                    gate_pin_name(pin),
                    cell.pin_phase(pin)
                );
            }
        }
        out
    }

    /// A human-readable dump of the physical realisations, for the report
    /// and `--keep-intermediates`.
    pub fn implementations_report(&self) -> String {
        let mut out = String::new();
        for cell in &self.cells {
            let _ = writeln!(out, "{} ({} inputs)  O={}", cell.name, cell.arity, cell.expression());
            for imp in &cell.impls {
                let assigns: Vec<String> = imp
                    .inputs
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let phys = self.phys_names.get(i).map(|s| s.as_str()).unwrap_or("?");
                        match p {
                            PhysIn::Pin(k) => format!("{}={}", phys, gate_pin_name(*k)),
                            PhysIn::Const(v) => format!("{}={}", phys, u8::from(*v)),
                            PhysIn::DontCare => format!("{}=-", phys),
                        }
                    })
                    .collect();
                let _ = writeln!(out, "    {:<6} {}", imp.op_name, assigns.join(" "));
            }
        }
        out
    }
}

/// What an input mux can select in order to see a constant: either a
/// constant-capable horizontal lane, or a constant wired straight into the
/// mux. Empty means no upstream CLB can hand this input a constant at all.
fn constant_lanes_per_input(fabric: &Fabric) -> Vec<Vec<usize>> {
    let const_lanes = fabric.const_capable_horz_lanes();
    fabric
        .input_muxes
        .iter()
        .map(|mux| {
            mux.sources
                .iter()
                .filter_map(|s| match s {
                    Source::HorzIn(l) if const_lanes.contains(l) => Some(*l),
                    _ => None,
                })
                .collect()
        })
        .collect()
}

/// An input that can reach no constant-capable lane is only usable in a row
/// whose IO map supplies the constant at the edge, so it must never be the
/// first choice. Well clear of any real lane count.
const UNREACHABLE_CONSTANT: u32 = 8;

/// What an implementation's constants really cost: the number of distinct
/// horizontal lanes that have to carry a constant for it.
///
/// This is not the number of tied inputs. The input mux windows overlap, so
/// several tied inputs can share one lane — `b` sees `{h1, h2}` and `c` sees
/// `{h2, h3, v0, carry}`, so tying both to 0 costs the *single* lane `h2`,
/// not two. Since a lane carries one value, inputs tied to 0 and inputs tied
/// to 1 must land on different lanes.
///
/// Arity is three, so this brute-forces the assignment rather than pretending
/// to be a set cover solver.
fn impl_cost(imp: &CellImpl, candidates: &[Vec<usize>]) -> u32 {
    let mut wanted: Vec<(usize, bool)> = imp.constants().collect();
    // Inputs no upstream CLB can reach are charged individually and dropped
    // from the lane assignment.
    let mut penalty = 0;
    wanted.retain(|&(input, _)| {
        let reachable = candidates.get(input).is_some_and(|l| !l.is_empty());
        if !reachable {
            penalty += UNREACHABLE_CONSTANT;
        }
        reachable
    });
    penalty + minimum_constant_lanes(&wanted, candidates)
}

/// Fewest distinct lanes that can carry every requested constant, where a
/// lane may serve several inputs but only one value.
fn minimum_constant_lanes(wanted: &[(usize, bool)], candidates: &[Vec<usize>]) -> u32 {
    fn search(
        wanted: &[(usize, bool)],
        candidates: &[Vec<usize>],
        chosen: &mut Vec<(usize, bool)>,
        best: &mut u32,
    ) {
        let distinct = |chosen: &Vec<(usize, bool)>| {
            let mut lanes: Vec<usize> = chosen.iter().map(|&(lane, _)| lane).collect();
            lanes.sort_unstable();
            lanes.dedup();
            lanes.len() as u32
        };
        if distinct(chosen) >= *best {
            return; // already no better than what we have
        }
        let Some(&(input, value)) = wanted.first() else {
            *best = distinct(chosen);
            return;
        };
        for &lane in &candidates[input] {
            // A lane already carrying the other value cannot be reused.
            if chosen.iter().any(|&(l, v)| l == lane && v != value) {
                continue;
            }
            chosen.push((lane, value));
            search(&wanted[1..], candidates, chosen, best);
            chosen.pop();
        }
    }

    if wanted.is_empty() {
        return 0;
    }
    let mut best = u32::MAX;
    search(wanted, candidates, &mut Vec::new(), &mut best);
    // Unsatisfiable only if a value clash leaves no assignment, in which case
    // the implementation needs a constant the fabric cannot place.
    if best == u32::MAX {
        UNREACHABLE_CONSTANT * wanted.len() as u32
    } else {
        best
    }
}

/// Does the function actually depend on this pin?
fn depends_on(table: &[bool], arity: usize, pin: usize) -> bool {
    let bit = 1 << pin;
    (0..1 << arity).any(|i| i & bit == 0 && table[i] != table[i | bit])
}

/// Drop the pins the function ignores, renumbering the rest.
fn restrict(table: &[bool], arity: usize, used: &[bool]) -> (Vec<bool>, usize, Vec<usize>) {
    let kept: Vec<usize> = (0..arity).filter(|&p| used[p]).collect();
    if kept.len() == arity {
        return (table.to_vec(), arity, kept);
    }
    let mut out = vec![false; 1 << kept.len()];
    for (minterm, slot) in out.iter_mut().enumerate() {
        let mut index = 0usize;
        for (new_pin, &old_pin) in kept.iter().enumerate() {
            if minterm >> new_pin & 1 != 0 {
                index |= 1 << old_pin;
            }
        }
        *slot = table[index];
    }
    let n = kept.len();
    (out, n, kept)
}

/// Pin permutations that leave the truth table unchanged.
fn symmetries(table: &[bool], arity: usize) -> Vec<Vec<usize>> {
    let mut out = Vec::new();
    for perm in permutations(arity) {
        let ok = (0..1 << arity).all(|i| {
            let mut j = 0usize;
            for (pin, &target) in perm.iter().enumerate() {
                if i >> pin & 1 != 0 {
                    j |= 1 << target;
                }
            }
            table[i] == table[j]
        });
        if ok {
            out.push(perm);
        }
    }
    out
}

fn permutations(n: usize) -> Vec<Vec<usize>> {
    let mut out = Vec::new();
    let mut current: Vec<usize> = (0..n).collect();
    permute(&mut current, 0, &mut out);
    out
}

fn permute(current: &mut Vec<usize>, k: usize, out: &mut Vec<Vec<usize>>) {
    if k == current.len() {
        out.push(current.clone());
        return;
    }
    for i in k..current.len() {
        current.swap(k, i);
        permute(current, k + 1, out);
        current.swap(k, i);
    }
}

pub fn gate_pin_name(pin: usize) -> String {
    // Gate pins are named independently of the physical inputs, since a pin
    // does not always sit on the physical input of the same index.
    let letters = [b'p', b'q', b'r', b's'];
    letters
        .get(pin)
        .map(|&c| (c as char).to_string())
        .unwrap_or_else(|| format!("p{}", pin))
}

/// Friendly names for the functions that actually turn up, so the report and
/// the genlib read like a cell library rather than a hash dump.
fn cell_name(fabric: &Fabric, arity: usize, table: &[bool]) -> String {
    if arity == fabric.input_muxes.len() {
        if let Some(op) = fabric.operations.iter().find(|o| o.table == table) {
            return op.name.clone();
        }
    }
    // Bit `i` of `bits` is the result at minterm `i`, so these patterns read
    // least-significant-minterm first: AND2 is true only at minterm 3.
    let bits: u32 = table.iter().enumerate().map(|(i, &v)| u32::from(v) << i).sum();
    let known: &[(usize, u32, &str)] = &[
        (1, 0b01, "INV"),
        (1, 0b10, "BUF"),
        (2, 0b1000, "AND2"),
        (2, 0b0111, "NAND2"),
        (2, 0b1110, "OR2"),
        (2, 0b0001, "NOR2"),
        (2, 0b0110, "XOR2"),
        (2, 0b1001, "XNOR2"),
        (2, 0b0010, "ANDNQ2"),
        (2, 0b0100, "ANDNP2"),
        (2, 0b1011, "ORNQ2"),
        (2, 0b1101, "ORNP2"),
    ];
    known
        .iter()
        .find(|&&(a, t, _)| a == arity && t == bits)
        .map(|&(_, _, n)| n.to_string())
        .unwrap_or_else(|| format!("LOGIC{}_{:X}", arity, bits))
}

/// Minimal-ish sum of products over `*`, `+`, `!`, via prime implicants and a
/// greedy cover. Functions here have at most three inputs, so exactness is
/// not worth the code; readability of the emitted genlib is.
fn sum_of_products(table: &[bool], arity: usize) -> String {
    let minterms: Vec<usize> = (0..1 << arity).filter(|&i| table[i]).collect();
    if minterms.is_empty() {
        return "CONST0".to_string();
    }
    if minterms.len() == 1 << arity {
        return "CONST1".to_string();
    }

    // An implicant is (values, mask): mask bit set = that pin is fixed.
    let mut implicants: Vec<(usize, usize)> =
        minterms.iter().map(|&m| (m, (1 << arity) - 1)).collect();
    let mut primes: Vec<(usize, usize)> = Vec::new();
    loop {
        let mut merged = vec![false; implicants.len()];
        let mut next: Vec<(usize, usize)> = Vec::new();
        for i in 0..implicants.len() {
            for j in i + 1..implicants.len() {
                let (vi, mi) = implicants[i];
                let (vj, mj) = implicants[j];
                if mi != mj {
                    continue;
                }
                let diff = (vi ^ vj) & mi;
                if diff.count_ones() == 1 {
                    merged[i] = true;
                    merged[j] = true;
                    let candidate = (vi & !diff, mi & !diff);
                    if !next.contains(&candidate) {
                        next.push(candidate);
                    }
                }
            }
        }
        for (i, &imp) in implicants.iter().enumerate() {
            if !merged[i] && !primes.contains(&imp) {
                primes.push(imp);
            }
        }
        if next.is_empty() {
            break;
        }
        implicants = next;
    }

    let covers = |imp: (usize, usize), m: usize| (m & imp.1) == (imp.0 & imp.1);
    let mut uncovered: Vec<usize> = minterms.clone();
    let mut chosen: Vec<(usize, usize)> = Vec::new();
    while !uncovered.is_empty() {
        let best = primes
            .iter()
            .filter(|&&p| !chosen.contains(&p))
            .max_by_key(|&&p| {
                (uncovered.iter().filter(|&&m| covers(p, m)).count(), p.1.count_zeros())
            })
            .copied();
        let Some(best) = best else { break };
        uncovered.retain(|&m| !covers(best, m));
        chosen.push(best);
    }
    chosen.sort();

    let terms: Vec<String> = chosen
        .iter()
        .map(|&(values, mask)| {
            let literals: Vec<String> = (0..arity)
                .filter(|&pin| mask >> pin & 1 != 0)
                .map(|pin| {
                    let name = gate_pin_name(pin);
                    if values >> pin & 1 != 0 {
                        name
                    } else {
                        format!("!{}", name)
                    }
                })
                .collect();
            literals.join("*")
        })
        .collect();
    terms.join("+")
}
