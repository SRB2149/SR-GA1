//! Turning SystemVerilog signal names into block names the GUI will show.
//!
//! A CLB that computes `count[2]` should be called that, not `CLB3_1`. Names
//! are emitted pinned so the GUI's auto-naming leaves them alone, and
//! bracket indices survive sanitisation so its bus colouring still works.
//!
//! Collisions are resolved deterministically — the same netlist always
//! produces the same names, which is part of the tool's byte-identical
//! output guarantee.

use crate::netlist::{Bit, Module};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Best available SystemVerilog name for each net bit in a module.
#[derive(Debug, Clone, Default)]
pub struct NetNames {
    by_bit: HashMap<u32, String>,
}

impl NetNames {
    /// Walk the module's `netnames`, preferring names the designer wrote
    /// over the ones Yosys invented, then shorter over longer, then
    /// alphabetical so the result never depends on hash order.
    pub fn collect(module: &Module) -> NetNames {
        let mut candidates: BTreeMap<u32, Vec<(u8, usize, String)>> = BTreeMap::new();

        let mut consider = |name: &str, bits: &[Bit], hidden: bool| {
            let base = sanitise(name);
            if base.is_empty() {
                return;
            }
            let width = bits.len();
            for (index, bit) in bits.iter().enumerate() {
                let Some(net) = bit.as_net() else { continue };
                let text = if width == 1 {
                    base.clone()
                } else {
                    format!("{}[{}]", base, index)
                };
                candidates.entry(net).or_default().push((
                    u8::from(hidden),
                    text.len(),
                    text,
                ));
            }
        };

        // Ports first: a name on the boundary is the most meaningful one.
        for (name, port) in &module.ports {
            consider(name, &port.bits, false);
        }
        for (name, net) in &module.netnames {
            consider(name, &net.bits, net.hide_name != 0);
        }

        let mut by_bit = HashMap::new();
        for (net, mut options) in candidates {
            options.sort();
            if let Some((_, _, name)) = options.into_iter().next() {
                by_bit.insert(net, name);
            }
        }
        NetNames { by_bit }
    }

    pub fn get(&self, net: u32) -> Option<&str> {
        self.by_bit.get(&net).map(|s| s.as_str())
    }

    /// Name for a bit, including constants.
    pub fn of(&self, bit: Bit) -> Option<String> {
        match bit {
            Bit::Net(n) => self.get(n).map(|s| s.to_string()),
            Bit::Zero => Some("fixed_zero".to_string()),
            Bit::One => Some("fixed_one".to_string()),
            _ => None,
        }
    }
}

/// Keep what a SystemVerilog identifier and a bus index need, replace the
/// rest. Yosys' leading backslash and hierarchy dots go; brackets stay.
pub fn sanitise(name: &str) -> String {
    let trimmed = name.trim_start_matches('\\');
    let mut out = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '[' || ch == ']' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    // A name must still look like an identifier once the GUI reads it back.
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

/// Hands out unique names, appending `_2`, `_3`, ... to duplicates in a
/// fixed order.
#[derive(Debug, Default)]
pub struct NameAllocator {
    used: BTreeSet<String>,
}

impl NameAllocator {
    pub fn new() -> NameAllocator {
        NameAllocator::default()
    }

    /// Reserve names that must not be taken, such as the fabric's own
    /// reserved IO and constant names.
    pub fn reserve(&mut self, name: &str) {
        self.used.insert(name.to_string());
    }

    pub fn unique(&mut self, preferred: &str) -> String {
        let base = if preferred.is_empty() { "cell".to_string() } else { sanitise(preferred) };
        if self.used.insert(base.clone()) {
            return base;
        }
        // Keep a bus index at the end: count[2] collides as count[2]_2.
        for n in 2..usize::MAX {
            let candidate = format!("{}_{}", base, n);
            if self.used.insert(candidate.clone()) {
                return candidate;
            }
        }
        base
    }
}
