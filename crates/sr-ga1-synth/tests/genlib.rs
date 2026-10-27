//! The cell library is derived, not written down, so these tests check the
//! derivation against what the fabric actually does: every implementation is
//! re-simulated through the LUT value it carries, in the fabric's index order.

use sr_ga1_synth::fabric::Fabric;
use sr_ga1_synth::genlib::{CellLibrary, PhysIn};
use std::path::{Path, PathBuf};

fn fabric() -> Fabric {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("fabric.toml");
    Fabric::load_file(&path).expect("the repository fabric.toml must load")
}

/// Run an implementation through its LUT value and check it computes the
/// cell's function for every input combination, whatever the don't-care
/// inputs carry.
fn implementations_are_faithful(f: &Fabric, lib: &CellLibrary) {
    for cell in &lib.cells {
        for imp in &cell.impls {
            for physical in 0..1usize << lib.phys_inputs {
                let values: Vec<bool> = (0..lib.phys_inputs).map(|i| physical >> i & 1 != 0).collect();
                let mut minterm = 0usize;
                for (i, phys) in imp.inputs.iter().enumerate() {
                    if let PhysIn::Pin(k) = phys {
                        if values[i] {
                            minterm |= 1 << k;
                        }
                    }
                }
                assert_eq!(
                    f.eval_lut(imp.lut, &values),
                    cell.table[minterm],
                    "{} via LUT 0x{:02X} disagrees with its function at inputs {:?}",
                    cell.name,
                    imp.lut,
                    values
                );
            }
        }
    }
}

#[test]
fn every_implementation_computes_its_cell() {
    let f = fabric();
    let lib = CellLibrary::derive(&f);
    implementations_are_faithful(&f, &lib);
}

#[test]
fn the_library_covers_the_cells_the_flow_needs() {
    let f = fabric();
    let lib = CellLibrary::derive(&f);
    for name in [
        "AND3", "OR3", "XOR3", "NAND3", "NOR3", "XNOR3", "AO21", "MUX2", "AND2", "OR2", "NAND2",
        "NOR2", "XOR2", "XNOR2", "INV", "BUF",
    ] {
        assert!(lib.cell(name).is_some(), "the derived library is missing {}", name);
    }
}

#[test]
fn cell_names_describe_the_function_they_name() {
    let f = fabric();
    let lib = CellLibrary::derive(&f);
    // `table[i]` is the result at minterm i, pin k taken from bit k.
    let expected: &[(&str, &[bool])] = &[
        ("INV", &[true, false]),
        ("BUF", &[false, true]),
        ("AND2", &[false, false, false, true]),
        ("NAND2", &[true, true, true, false]),
        ("OR2", &[false, true, true, true]),
        ("NOR2", &[true, false, false, false]),
        ("XOR2", &[false, true, true, false]),
        ("XNOR2", &[true, false, false, true]),
        ("AND3", &[false, false, false, false, false, false, false, true]),
        ("OR3", &[false, true, true, true, true, true, true, true]),
        ("AO21", &[false, false, false, true, true, true, true, true]),
        ("MUX2", &[false, false, true, true, false, true, false, true]),
    ];
    for (name, table) in expected {
        let cell = lib.cell(name).unwrap_or_else(|| panic!("{} missing", name));
        assert_eq!(cell.table, *table, "{} is attached to the wrong function", name);
    }
}


/// Every pin placement exists, so a pin can always be moved to whichever input
/// can see the lane its signal arrives on.
#[test]
fn every_pin_placement_is_available() {
    let f = fabric();
    let lib = CellLibrary::derive(&f);
    for cell in &lib.cells {
        let expected = match cell.arity {
            1 => 3,
            2 => 6,
            _ => 6,
        };
        assert_eq!(cell.impls.len(), expected, "{} has {} placements", cell.name, cell.impls.len());
        for imp in &cell.impls {
            let pins = imp.inputs.iter().filter(|p| matches!(p, PhysIn::Pin(_))).count();
            assert_eq!(pins, cell.arity, "{}: one physical input per pin, none shared", cell.name);
        }
    }
    // Every 3-input function that uses all three inputs, plus the 2- and
    // 1-input ones that use all of theirs.
    assert_eq!(lib.cells.iter().filter(|c| c.arity == 3).count(), 218);
    assert_eq!(lib.cells.iter().filter(|c| c.arity == 2).count(), 10);
    assert_eq!(lib.cells.iter().filter(|c| c.arity == 1).count(), 2);
}

#[test]
fn a_buffer_can_take_its_signal_on_any_input() {
    let f = fabric();
    let lib = CellLibrary::derive(&f);
    let buf = lib.buffer().expect("BUF");
    for input in 0..lib.phys_inputs {
        let forms = buf.impls_for_input(input);
        assert_eq!(forms.len(), 1, "one buffer per arrival input");
        // It copies that input whatever the others carry.
        for physical in 0..8usize {
            let values: Vec<bool> = (0..3).map(|i| physical >> i & 1 != 0).collect();
            assert_eq!(f.eval_lut(forms[0].lut, &values), values[input]);
        }
    }
    // With a as the most significant LUT index bit, BUF on a is the BUF_A preset.
    assert_eq!(buf.impls_for_input(0)[0].lut, 0xF0);
}

#[test]
fn the_adder_sum_is_the_fabrics_xor3_preset() {
    let f = fabric();
    let lib = CellLibrary::derive(&f);
    let xor3 = lib.cell("XOR3").expect("XOR3");
    assert!(xor3.impls.iter().all(|i| i.lut == 0x96), "XOR3 is symmetric, so every placement is 0x96");
}

#[test]
fn symmetry_groups_match_commutativity() {
    let f = fabric();
    let lib = CellLibrary::derive(&f);
    assert_eq!(lib.cell("AND3").expect("AND3").symmetries.len(), 6, "AND3 is fully commutative");
    assert_eq!(lib.cell("XOR3").expect("XOR3").symmetries.len(), 6, "XOR3 is fully commutative");
    // AO21 is (p*q)+r: only its two AND inputs may swap.
    assert_eq!(lib.cell("AO21").expect("AO21").symmetries.len(), 2);
    // MUX2 is r ? p : q — no permutation preserves it.
    assert_eq!(lib.cell("MUX2").expect("MUX2").symmetries.len(), 1);
}

#[test]
fn the_genlib_is_well_formed() {
    let f = fabric();
    let lib = CellLibrary::derive(&f);
    let text = lib.genlib(&f);
    assert!(text.contains("GATE zero  0 O=CONST0;"));
    assert!(text.contains("GATE one   0 O=CONST1;"));
    // ABC's formula parser gets nothing but these operators.
    for line in text.lines().filter(|l| l.starts_with("GATE")) {
        let expr = line.split_once("O=").expect("gate has an output").1;
        for ch in expr.chars() {
            assert!(
                ch.is_ascii_alphanumeric() || "*+!();_ ".contains(ch),
                "unexpected character {:?} in genlib expression {}",
                ch,
                expr
            );
        }
    }
    // Every gate must cost exactly one CLB, except the free constants.
    let areas: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("GATE"))
        .filter_map(|l| l.split_whitespace().nth(2))
        .collect();
    assert!(areas.iter().all(|a| *a == "0" || *a == "1"), "areas were {:?}", areas);
}

#[test]
fn expressions_evaluate_to_their_truth_tables() {
    let f = fabric();
    let lib = CellLibrary::derive(&f);
    for cell in &lib.cells {
        let expr = cell.expression();
        for minterm in 0..1usize << cell.arity {
            assert_eq!(
                eval(&expr, minterm),
                cell.table[minterm],
                "{}: O={} disagrees at minterm {}",
                cell.name,
                expr,
                minterm
            );
        }
    }
}

/// Minimal evaluator for the sum-of-products genlib subset: terms joined by
/// `+`, literals joined by `*`, optional leading `!`, pins named p, q, r, s.
fn eval(expr: &str, minterm: usize) -> bool {
    expr.split('+').any(|term| {
        term.split('*').all(|lit| {
            let (negated, name) = match lit.strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, lit),
            };
            let pin = match name.trim() {
                "p" => 0,
                "q" => 1,
                "r" => 2,
                "s" => 3,
                other => panic!("unexpected literal {:?}", other),
            };
            (minterm >> pin & 1 != 0) != negated
        })
    })
}
