//! The cell library is derived, not written down, so these tests check the
//! derivation against what the fabric actually does: every implementation is
//! re-simulated through the operation truth tables it came from.

use sr_ga1_synth::fabric::Fabric;
use sr_ga1_synth::genlib::{CellLibrary, PhysIn};
use std::path::{Path, PathBuf};

fn fabric() -> Fabric {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("fabric.toml");
    Fabric::load_file(&path).expect("the repository fabric.toml must load")
}

/// Run an implementation through the fabric's own operation table and check
/// it computes the cell's function for every input combination.
fn implementations_are_faithful(f: &Fabric, lib: &CellLibrary) {
    for cell in &lib.cells {
        for imp in &cell.impls {
            let op = f
                .operations
                .iter()
                .find(|o| o.code == imp.op_code)
                .unwrap_or_else(|| panic!("{} uses operation code {}", cell.name, imp.op_code));
            // A don't-care physical input must genuinely not matter, so try
            // it both ways.
            let dont_cares: Vec<usize> = imp
                .inputs
                .iter()
                .enumerate()
                .filter(|(_, p)| **p == PhysIn::DontCare)
                .map(|(i, _)| i)
                .collect();
            for fill in 0..1usize << dont_cares.len() {
                for minterm in 0..1usize << cell.arity {
                    let mut index = 0usize;
                    for (i, phys) in imp.inputs.iter().enumerate() {
                        let value = match phys {
                            PhysIn::Pin(k) => minterm >> k & 1 != 0,
                            PhysIn::Const(v) => *v,
                            PhysIn::DontCare => {
                                let slot = dont_cares.iter().position(|&d| d == i).unwrap_or(0);
                                fill >> slot & 1 != 0
                            }
                        };
                        if value {
                            index |= 1 << i;
                        }
                    }
                    assert_eq!(
                        op.table[index], cell.table[minterm],
                        "{} via {} disagrees with the fabric at minterm {}",
                        cell.name, imp.op_name, minterm
                    );
                }
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

#[test]
fn degraded_cells_prefer_the_cheap_inputs() {
    let f = fabric();
    let lib = CellLibrary::derive(&f);
    // Physical input order is a, b, c; a sees only the minor lanes, which
    // carry no constant outside row 3, so it must never be the first choice.
    for name in ["AND2", "OR2", "NAND2", "NOR2", "XOR2", "XNOR2", "INV", "BUF"] {
        let cell = lib.cell(name).expect("cell present");
        let first = cell.impls.first().expect("at least one implementation");
        assert!(
            !matches!(first.inputs[0], PhysIn::Const(_)),
            "{} would tie input a, which is expensive outside row 3: {:?}",
            name,
            first
        );
    }
}

#[test]
fn an_unroutable_constant_has_somewhere_to_fall_back_to() {
    let f = fabric();
    let lib = CellLibrary::derive(&f);
    // Phase 6 re-maps a cell when its constant proves unroutable, so the
    // common degraded cells must offer more than one physical form.
    for name in ["INV", "BUF", "AND2", "OR2"] {
        let cell = lib.cell(name).expect("cell present");
        assert!(
            cell.impls.len() > 1,
            "{} has only one implementation, so a re-map has nowhere to go",
            name
        );
    }
}

/// Constants are counted by the lanes they need, not the inputs they tie.
/// Input `b` sees {h1, h2} and input `c` sees {h2, h3, v0, carry}, so tying
/// both to 0 costs the single lane h2 — the same as tying one of them.
#[test]
fn tying_two_inputs_to_one_lane_costs_no_more_than_tying_one() {
    let f = fabric();
    let lib = CellLibrary::derive(&f);
    let buf = lib.cell("BUF").expect("BUF");

    let position = |op: &str, inputs: &[PhysIn]| {
        buf.impls
            .iter()
            .position(|i| i.op_name == op && i.inputs == inputs)
            .unwrap_or_else(|| panic!("{} {:?} is not an implementation of BUF", op, inputs))
    };
    // OR3(a, 0, 0) ties b and c, but both read the one constant lane.
    let or3 = position("OR3", &[PhysIn::Pin(0), PhysIn::Const(false), PhysIn::Const(false)]);
    // AO21(-, 0, a) ties only b.
    let ao21 = position("AO21", &[PhysIn::DontCare, PhysIn::Const(false), PhysIn::Pin(0)]);
    assert!(
        or3 < ao21,
        "OR3(a,0,0) needs one constant lane and keeps the signal on a minor lane, so it \
         should not rank below a form that moves the signal onto a major lane"
    );

    // Anything tying input a must rank last: no upstream CLB can put a
    // constant on a minor lane.
    let ties_a = |i: &&sr_ga1_synth::genlib::CellImpl| matches!(i.inputs[0], PhysIn::Const(_));
    let first_a = buf.impls.iter().position(|i| ties_a(&i)).expect("some form ties a");
    assert!(
        first_a > or3 && first_a > ao21,
        "forms tying input a must rank below every form that does not"
    );
}

#[test]
fn the_shared_constant_lane_is_one_a_major_mux_can_drive() {
    let f = fabric();
    // The lane that serves both b and c must be one whose driving mux can
    // produce constants, or the saving is imaginary.
    let const_lanes = f.const_capable_horz_lanes();
    let b = f.input_mux("b").expect("input b");
    let c = f.input_mux("c").expect("input c");
    let shared: Vec<usize> = const_lanes
        .iter()
        .copied()
        .filter(|lane| {
            let sees = |m: &sr_ga1_synth::fabric::InputMux| {
                m.sources.contains(&sr_ga1_synth::fabric::Source::HorzIn(*lane))
            };
            sees(b) && sees(c)
        })
        .collect();
    assert_eq!(shared, vec![2], "b and c overlap on exactly one constant-capable lane");
}

#[test]
fn buffer_forms_can_be_selected_by_arrival_lane() {
    let f = fabric();
    let lib = CellLibrary::derive(&f);
    let buf = lib.cell("BUF").expect("BUF");

    // A chip input entering on a minor lane can only reach input a, so the
    // router needs at least one form whose live pin sits there.
    let on_a = buf.impls_for_input(0);
    assert!(!on_a.is_empty(), "no buffer accepts its signal on input a");
    assert!(on_a.iter().all(|i| i.takes_pin_on(0)));
    assert!(
        on_a.iter().any(|i| i.op_name == "OR3" && i.inputs[1] == PhysIn::Const(false)),
        "OR3(a, 0, 0) should be available for a signal arriving on a minor lane"
    );

    // And a signal arriving on a major lane or the vertical bus reaches c.
    let on_c = buf.impls_for_input(2);
    assert!(!on_c.is_empty(), "no buffer accepts its signal on input c");
    assert!(on_c.iter().all(|i| i.takes_pin_on(2)));

    // The filter must preserve the cheapest-first order of `impls`.
    let indices: Vec<usize> = on_a
        .iter()
        .map(|sel| buf.impls.iter().position(|i| i == *sel).expect("selected form is a member"))
        .collect();
    assert!(indices.windows(2).all(|w| w[0] < w[1]), "selection must stay cheapest-first");
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
