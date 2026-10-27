//! The cell library, derived from `fabric.toml`.
//!
//! The CLB computes its result with a lookup table over three selected
//! inputs, so every function of up to three inputs costs exactly one CLB. This
//! module turns that into two things:
//!
//! 1. a genlib ABC can map against, one gate per distinct *function* that
//!    depends on all of its pins; and
//! 2. for each gate, every *physical* way to build it — which physical input
//!    each gate pin sits on, and the LUT value that computes the function from
//!    there.
//!
//! The second list is what lets placement move a pin to whichever input mux
//! can actually see the lane its signal arrives on. Input `a` reads only the
//! two minor lanes, `b` straddles a minor and a major lane, and `c` reaches the
//! major lanes, the vertical ring and the carry chain, so where a pin sits
//! decides what it can be fed by. An input a function does not use is simply
//! a don't-care: a LUT ignores it, so nothing has to be tied off.
//!
//! The LUT's bit order comes from the fabric (`[clb.lut] index`), so nothing
//! here assumes which input is the most significant.

use crate::fabric::Fabric;
use std::fmt::Write as _;

/// What one physical CLB input carries in a particular implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PhysIn {
    /// Gate pin `k` of the mapped cell.
    Pin(usize),
    /// The LUT ignores this input: any legal source will do.
    DontCare,
}

/// One physical realisation of a gate: where each pin sits, in the fabric's
/// input mux order (`a`, `b`, `c`), and the LUT value that computes the gate's
/// function from those positions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellImpl {
    pub lut: u64,
    pub inputs: Vec<PhysIn>,
}

impl CellImpl {
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
    /// Physical realisations: every placement of the pins on distinct inputs.
    pub impls: Vec<CellImpl>,
}

impl LogicCell {
    /// Implementations that accept a live pin on `phys_input`.
    ///
    /// The best buffer is the one whose live pin already sits on the input
    /// the signal arrived at, so this is a routing-time question rather than a
    /// fixed ranking.
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
    /// Number of physical inputs the LUT takes.
    pub phys_inputs: usize,
    /// Names of the physical inputs, in mux order.
    pub phys_names: Vec<String>,
}

impl CellLibrary {
    pub fn cell(&self, name: &str) -> Option<&LogicCell> {
        self.cells.iter().find(|c| c.name == name)
    }

    /// The single-input identity, which the router and packer build buffers
    /// from.
    pub fn buffer(&self) -> Option<&LogicCell> {
        self.cells.iter().find(|c| c.arity == 1 && c.table == [false, true])
    }

    /// Build the library: every function of 1..=inputs pins that depends on
    /// all of them, with every placement of its pins on distinct inputs.
    pub fn derive(fabric: &Fabric) -> CellLibrary {
        let phys = fabric.input_muxes.len();
        let mut cells = Vec::new();
        for arity in 1..=phys {
            let placements = injections(arity, phys);
            for bits in 0u64..1 << (1 << arity) {
                let table: Vec<bool> = (0..1 << arity).map(|i| bits >> i & 1 != 0).collect();
                if !(0..arity).all(|pin| depends_on(&table, arity, pin)) {
                    continue; // a smaller function, or a constant
                }
                let impls = placements
                    .iter()
                    .map(|placement| {
                        let mut inputs = vec![PhysIn::DontCare; phys];
                        for (pin, &input) in placement.iter().enumerate() {
                            inputs[input] = PhysIn::Pin(pin);
                        }
                        let lut = fabric.lut_value(|values| {
                            let minterm = placement
                                .iter()
                                .enumerate()
                                .fold(0usize, |acc, (pin, &input)| acc | usize::from(values[input]) << pin);
                            table[minterm]
                        });
                        CellImpl { lut, inputs }
                    })
                    .collect();
                cells.push(LogicCell {
                    name: cell_name(fabric, arity, &table),
                    arity,
                    symmetries: symmetries(&table, arity),
                    table,
                    impls,
                });
            }
        }
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

    /// A human-readable dump of the physical realisations, for
    /// `--keep-intermediates`.
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
                            PhysIn::DontCare => format!("{}=-", phys),
                        }
                    })
                    .collect();
                let _ = writeln!(out, "    LUT 0x{:02X}  {}", imp.lut, assigns.join(" "));
            }
        }
        out
    }
}

/// Every way to put `pins` gate pins on distinct physical inputs, as the
/// input each pin lands on.
fn injections(pins: usize, inputs: usize) -> Vec<Vec<usize>> {
    fn extend(current: &mut Vec<usize>, pins: usize, inputs: usize, out: &mut Vec<Vec<usize>>) {
        if current.len() == pins {
            out.push(current.clone());
            return;
        }
        for input in 0..inputs {
            if !current.contains(&input) {
                current.push(input);
                extend(current, pins, inputs, out);
                current.pop();
            }
        }
    }
    let mut out = Vec::new();
    extend(&mut Vec::new(), pins, inputs, &mut out);
    out
}

/// Does the function actually depend on this pin?
fn depends_on(table: &[bool], arity: usize, pin: usize) -> bool {
    let bit = 1 << pin;
    (0..1 << arity).any(|i| i & bit == 0 && table[i] != table[i | bit])
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
/// the genlib read like a cell library rather than a hash dump. A full-width
/// function takes the fabric's preset name for the LUT value that computes it
/// with pin k on input k, when there is one.
fn cell_name(fabric: &Fabric, arity: usize, table: &[bool]) -> String {
    if arity == fabric.input_muxes.len() {
        let lut = fabric.lut_value(|values| {
            let minterm = (0..arity).fold(0usize, |acc, pin| acc | usize::from(values[pin]) << pin);
            table[minterm]
        });
        if let Some(name) = fabric.preset_name(lut) {
            return name.to_string();
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
        .unwrap_or_else(|| format!("LOGIC{}_{:02X}", arity, bits))
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
