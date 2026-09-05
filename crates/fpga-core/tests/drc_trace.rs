//! DRC and trace/VCD tests.

use fpga_core::config::Design;
use fpga_core::drc::{check, Severity};
use fpga_core::fabric::Fabric;
use fpga_core::sim::{SimState, Stimulus};
use fpga_core::trace::Tracer;

fn fabric() -> Fabric {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fabric.toml");
    Fabric::load_file(std::path::Path::new(path)).expect("fabric.toml must load")
}

/// Same design as the simulator tests: column 0 clocked from input_0, with
/// CLB(0,0)'s register routed to output lane 3.
fn clocked_design(f: &Fabric) -> Design {
    let mut d = Design::new(f);
    d.set_clb(f, 0, 0, "operation_select", 1).unwrap();
    d.set_clb(f, 0, 1, "minor_vert_sel", 0b0100).unwrap();
    d.set_clb(f, 0, 2, "minor_vert_sel", 0b0001).unwrap();
    d.set_clb(f, 0, 3, "minor_vert_sel", 0b0100).unwrap();
    d.set_csb(f, 0, "bus_addr", 2).unwrap();
    d.set_clb(f, 0, 0, "major_horz3_sel", 5).unwrap();
    for col in 1..f.columns {
        d.set_clb(f, col, 0, "major_horz3_sel", 3).unwrap();
    }
    d
}

#[test]
fn empty_design_has_no_errors() {
    let f = fabric();
    let items = check(&f, &Design::new(&f));
    assert!(
        items.iter().all(|i| i.severity != Severity::Error),
        "unexpected errors: {:?}",
        items.iter().filter(|i| i.severity == Severity::Error).map(|i| &i.message).collect::<Vec<_>>()
    );
    // Blocks that don't contribute anywhere are flagged as unused.
    assert!(items.iter().any(|i| i.message.contains("unused")));
}

#[test]
fn csb_full_circle_is_an_error() {
    let f = fabric();
    let mut d = Design::new(&f);
    for col in 0..f.columns {
        d.set_csb(&f, col, "couple_to_previous", 1).unwrap();
    }
    let items = check(&f, &d);
    let errors: Vec<_> = items.iter().filter(|i| i.severity == Severity::Error).collect();
    assert_eq!(errors.len(), f.columns, "one error per circularly-coupled column");
    assert!(errors[0].message.contains("full circle"));
}

#[test]
fn floating_ring_and_clockless_column_warn() {
    let f = fabric();
    let mut d = Design::new(&f);
    for row in 0..f.rows {
        d.set_clb(&f, 4, row, "minor_vert_sel", 0b1111).unwrap();
    }
    let items = check(&f, &d);
    assert!(items
        .iter()
        .any(|i| i.severity == Severity::Warning && i.message.contains("floating pass-through loop")));
    assert!(items
        .iter()
        .any(|i| i.severity == Severity::Warning && i.message.contains("no usable clock")));
}

#[test]
fn derived_clock_is_informational_and_names_the_signal() {
    let f = fabric();
    let d = clocked_design(&f);
    let items = check(&f, &d);
    let info = items
        .iter()
        .find(|i| i.severity == Severity::Info && i.message.contains("derived from fabric signal"))
        .expect("expected a derived-clock notice");
    assert!(info.message.contains("CLB0_0_op"), "got: {}", info.message);
    // Only column 0 has live registers, so only one such notice.
    assert_eq!(
        items.iter().filter(|i| i.message.contains("derived from fabric signal")).count(),
        1
    );
}

#[test]
fn ddio_feedback_is_reported_as_potential_loop() {
    let f = fabric();
    let mut d = Design::new(&f);
    d.set_clb(&f, 0, 2, "operation_select", 1).unwrap();
    d.set_clb(&f, 0, 3, "minor_horz_sel", 0b01).unwrap();
    d.set_clb(&f, 0, 3, "major_horz2_sel", 2).unwrap();
    d.set_clb(&f, 0, 3, "operation_select", 4).unwrap();
    d.set_clb(&f, 1, 3, "operation_select", 2).unwrap();
    for col in 2..f.columns {
        d.set_clb(&f, col, 3, "minor_horz_sel", 0b01).unwrap();
    }
    let items = check(&f, &d);
    assert!(
        items
            .iter()
            .any(|i| i.severity == Severity::Error && i.message.contains("combinational loop")),
        "items: {:?}",
        items.iter().map(|i| &i.message).collect::<Vec<_>>()
    );
}

#[test]
fn vertical_ring_loop_is_reported() {
    let f = fabric();
    let mut d = Design::new(&f);
    // input c <- v_in[0] (a combinational lane), then pass the op back
    // around the ring to close a loop through this cell's own logic.
    d.set_clb(&f, 0, 0, "input_mux_c_sel", 2).unwrap();
    d.set_clb(&f, 0, 1, "minor_vert_sel", 0b0001).unwrap();
    d.set_clb(&f, 0, 2, "minor_vert_sel", 0b0100).unwrap();
    d.set_clb(&f, 0, 3, "minor_vert_sel", 0b0001).unwrap();
    let items = check(&f, &d);
    assert!(
        items
            .iter()
            .any(|i| i.severity == Severity::Error && i.message.contains("combinational loop")),
        "items: {:?}",
        items.iter().map(|i| &i.message).collect::<Vec<_>>()
    );
}

#[test]
fn tracer_records_and_exports_vcd() {
    let f = fabric();
    let d = clocked_design(&f);
    let mut stim = Stimulus::default();
    stim.set("input_0", vec![false, true]);
    stim.set("input_3", vec![true]);

    let ids = vec!["clk:0".to_string(), "ff:0:0".to_string(), "bogus:9".to_string()];
    let (mut tracer, warnings) = Tracer::new(&f, &ids);
    assert_eq!(warnings.len(), 1, "bad id dropped with warning");
    assert_eq!(tracer.ids.len(), 2);

    let (mut state, s0) = SimState::new(&f, &d, &stim).unwrap();
    tracer.sample(&s0);
    for _ in 0..4 {
        let s = state.step(&f, &d, &stim).unwrap();
        tracer.sample(&s);
    }
    assert_eq!(tracer.history.len(), 5);
    // clk follows input_0's [0,1] pattern; ff (pre-commit view) goes high in
    // the tick after the first rising edge.
    let clk: Vec<bool> = tracer.history.iter().map(|r| r[0]).collect();
    let ff: Vec<bool> = tracer.history.iter().map(|r| r[1]).collect();
    assert_eq!(clk, vec![false, true, false, true, false]);
    assert_eq!(ff, vec![false, false, true, true, true]);

    let vcd = tracer.to_vcd(&["CLB0_0_op".to_string(), "CLB0_0_reg".to_string()]);
    assert!(vcd.contains("$var wire 1 ! CLB0_0_op $end"));
    assert!(vcd.contains("$var wire 1 \" CLB0_0_reg $end"));
    assert!(vcd.contains("#0\n$dumpvars"));
    assert!(vcd.contains("#1\n1!"), "clk rises at tick 1:\n{}", vcd);
    assert!(vcd.contains("#2\n"), "changes at tick 2");
}
