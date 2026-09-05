//! Simulation panel: run controls, stimulus editor, waveform panel and VCD
//! export.

use super::{App, SimRuntime};
use eframe::egui;
use fpga_core::config::BlockId;
use fpga_core::sim::{SimError, SimState};
use fpga_core::trace::Tracer;

fn describe_error(app: &App, e: &SimError) -> String {
    match e {
        SimError::CombinationalLoop { blocks } => {
            let names: Vec<String> = blocks.iter().take(8).map(|&b| app.block_label(b)).collect();
            format!(
                "combinational loop — logic never settles (through: {}{})",
                names.join(", "),
                if blocks.len() > 8 { ", …" } else { "" }
            )
        }
    }
}

pub fn start_sim(app: &mut App) {
    match SimState::new(&app.fabric, &app.file.design, &app.file.stimulus) {
        Ok((state, settled)) => {
            let (mut tracer, warnings) = Tracer::new(&app.fabric, &app.file.traces);
            tracer.sample(&settled);
            app.sim = Some(SimRuntime {
                state,
                settled: Some(settled),
                tracer,
                trace_start: 0,
                running: false,
                tps: 10.0,
                accum: 0.0,
                error: None,
            });
            for w in warnings {
                app.toast(w);
            }
        }
        Err(e) => {
            let msg = describe_error(app, &e);
            app.toast(format!("cannot start simulation: {}", msg));
        }
    }
}

pub fn rebuild_tracer(app: &mut App) {
    if app.sim.is_none() {
        return;
    }
    let (mut tracer, warnings) = Tracer::new(&app.fabric, &app.file.traces);
    if let Some(sim) = &mut app.sim {
        if let Some(s) = &sim.settled {
            tracer.sample(s);
        }
        sim.trace_start = sim.state.tick;
        sim.tracer = tracer;
    }
    for w in warnings {
        app.toast(w);
    }
}

fn step_once(app: &mut App) {
    let Some(sim) = &mut app.sim else { return };
    match sim.state.step(&app.fabric, &app.file.design, &app.file.stimulus) {
        Ok(s) => {
            sim.tracer.sample(&s);
            sim.settled = Some(s);
            sim.error = None;
        }
        Err(e) => {
            sim.running = false;
            let msg = describe_error_owned(&app.fabric, &app.file.design, &e);
            if let Some(sim) = &mut app.sim {
                sim.error = Some(msg);
            }
        }
    }
}

fn describe_error_owned(fabric: &fpga_core::fabric::Fabric, design: &fpga_core::config::Design, e: &SimError) -> String {
    match e {
        SimError::CombinationalLoop { blocks } => {
            let namer = fpga_core::naming::Namer::new(fabric, design);
            let names: Vec<String> = blocks.iter().take(8).map(|&b| namer.block_name(b)).collect();
            format!(
                "combinational loop — logic never settles (through: {}{})",
                names.join(", "),
                if blocks.len() > 8 { ", …" } else { "" }
            )
        }
    }
}

/// Advance a running simulation by wall-clock time. Called every frame.
pub fn advance_sim(app: &mut App, ctx: &egui::Context) {
    let (running, tps) = match &app.sim {
        Some(s) if s.running && s.error.is_none() => (true, s.tps),
        _ => return,
    };
    let _ = running;
    let dt = ctx.input(|i| i.stable_dt).min(0.25);
    let steps = {
        let sim = app.sim.as_mut().expect("checked above");
        sim.accum += dt * tps;
        let steps = sim.accum.floor() as u64;
        sim.accum -= steps as f32;
        steps.min(5000)
    };
    for _ in 0..steps {
        step_once(app);
        if app.sim.as_ref().is_some_and(|s| !s.running) {
            break;
        }
    }
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        match &mut app.sim {
            None => {
                if ui.button("▶ start simulation").clicked() {
                    start_sim(app);
                }
                ui.label(egui::RichText::new("registers start at their configured reset values").small().color(egui::Color32::from_gray(120)));
            }
            Some(sim) => {
                let run_label = if sim.running { "⏸ pause" } else { "▶ run" };
                if ui.button(run_label).clicked() {
                    sim.running = !sim.running;
                    sim.accum = 0.0;
                }
                let mut do_step = false;
                let mut do_reset = false;
                let mut do_stop = false;
                if ui.button("step").clicked() {
                    do_step = true;
                }
                if ui.button("⟲ to tick 0").clicked() {
                    do_reset = true;
                }
                if ui.button("✖ stop").clicked() {
                    do_stop = true;
                }
                ui.label(format!("tick {}", sim.state.tick));
                ui.separator();
                ui.checkbox(&mut sim.state.reset, "assert reset")
                    .on_hover_text("synchronous: lands on each column's next rising clock edge");
                ui.separator();
                ui.label("ticks/s:");
                ui.add(egui::Slider::new(&mut sim.tps, 0.5..=2000.0).logarithmic(true));
                if let Some(err) = sim.error.clone() {
                    ui.colored_label(egui::Color32::LIGHT_RED, err);
                }
                if do_step {
                    step_once(app);
                } else if do_reset {
                    start_sim(app);
                } else if do_stop {
                    app.sim = None;
                }
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let can_export = app.sim.as_ref().is_some_and(|s| !s.tracer.is_empty() && !s.tracer.history.is_empty());
            if ui.add_enabled(can_export, egui::Button::new("export VCD…")).clicked() {
                export_vcd(app);
            }
            let n = app.file.traces.len();
            ui.label(egui::RichText::new(format!("{} signal(s) flagged", n)).small().color(egui::Color32::from_gray(120)));
        });
    });

    egui::CollapsingHeader::new("stimulus").show(ui, |ui| {
        ui.label(
            egui::RichText::new("patterns repeat when exhausted: \"0\" is constant 0, \"01\" is a clock, longer strings are serial data")
                .small()
                .color(egui::Color32::from_gray(120)),
        );
        // Unique stimulus-driven input names, in fabric order.
        let mut names: Vec<String> = Vec::new();
        for row in &app.fabric.io_inputs {
            for name in row {
                if *name != app.fabric.naming.constant_zero
                    && *name != app.fabric.naming.constant_one
                    && !names.contains(name)
                {
                    names.push(name.clone());
                }
            }
        }
        for name in names {
            let buf = app
                .stim_bufs
                .entry(name.clone())
                .or_insert_with(|| match app.file.stimulus.pattern(&name) {
                    Some(p) => p.iter().map(|&b| if b { '1' } else { '0' }).collect(),
                    None => "0".to_string(),
                });
            let mut text = buf.clone();
            ui.horizontal(|ui| {
                ui.monospace(format!("{:10}", name));
                let valid = text.chars().all(|c| c == '0' || c == '1');
                let edit = egui::TextEdit::singleline(&mut text)
                    .desired_width(220.0)
                    .text_color(if valid { egui::Color32::from_gray(220) } else { egui::Color32::LIGHT_RED });
                if ui.add(edit).changed() {
                    *buf = text.clone();
                    if valid {
                        let pattern: Vec<bool> = text.chars().map(|c| c == '1').collect();
                        app.file.stimulus.set(&name, pattern);
                        app.dirty = true;
                    }
                }
                if let Some(sim) = &app.sim {
                    let v = app.file.stimulus.value_at(&name, sim.state.tick);
                    ui.label(
                        egui::RichText::new(if v { "1" } else { "0" })
                            .color(super::colors::value_fill(v))
                            .monospace(),
                    );
                }
            });
        }
    });

    if app.show_waves {
        waveform_panel(app, ui);
    }
}

fn trace_display_name(app: &App, id: &str) -> String {
    let parts: Vec<&str> = id.split(':').collect();
    let num = |s: &str| s.parse::<usize>().unwrap_or(0);
    match parts.as_slice() {
        ["in", name] | ["out", name] => name.to_string(),
        ["clk", col] => app.clock_label(num(col)),
        ["ff", col, row] => format!(
            "{}{}",
            app.block_label(BlockId::Clb { col: num(col), row: num(row) }),
            app.fabric.naming.suffix_reg
        ),
        ["op", col, row] => format!(
            "{}{}",
            app.block_label(BlockId::Clb { col: num(col), row: num(row) }),
            app.fabric.naming.suffix_op
        ),
        ["carry", col, row] => format!(
            "{}{}",
            app.block_label(BlockId::Clb { col: num(col), row: num(row) }),
            app.fabric.naming.suffix_carry
        ),
        ["h", row, lane, pos] => {
            let (row, lane, pos) = (num(row), num(lane), num(pos));
            let origin = if pos < app.fabric.columns {
                app.nets.horz_in(pos, row, lane)
            } else {
                app.nets.horz_edge(row, lane)
            };
            app.net_label(origin)
        }
        ["v", col, lane, pos] => app.net_label(app.nets.vert_in(num(col), num(pos), num(lane))),
        _ => id.to_string(),
    }
}

fn waveform_panel(app: &mut App, ui: &mut egui::Ui) {
    let Some(sim) = &app.sim else {
        ui.label("start the simulation to see waveforms");
        return;
    };
    if sim.tracer.is_empty() {
        ui.label("no signals flagged — use the trace buttons in the inspectors");
        return;
    }
    // Collect stepped traces stacked vertically, newest signal on top.
    let n = sim.tracer.ids.len();
    let start = sim.trace_start as f64;
    let mut lines: Vec<(String, Vec<[f64; 2]>)> = Vec::new();
    for (i, id) in sim.tracer.ids.iter().enumerate() {
        let name = trace_display_name(app, id);
        let base = ((n - 1 - i) as f64) * 1.5;
        let mut pts: Vec<[f64; 2]> = Vec::new();
        let mut prev: Option<bool> = None;
        for (t, row) in sim.tracer.history.iter().enumerate() {
            let x = start + t as f64;
            let v = row[i];
            if let Some(p) = prev {
                if p != v {
                    pts.push([x, base + p as u8 as f64]);
                }
            }
            pts.push([x, base + v as u8 as f64]);
            prev = Some(v);
        }
        lines.push((name, pts));
    }
    egui_plot::Plot::new("waves")
        .height(140.0 + 20.0 * n as f32)
        .legend(egui_plot::Legend::default())
        .allow_drag(true)
        .allow_zoom(true)
        .show_y(false)
        .show(ui, |pui| {
            for (name, pts) in lines {
                pui.line(egui_plot::Line::new(egui_plot::PlotPoints::from(pts)).name(name));
            }
        });
}

fn export_vcd(app: &mut App) {
    let Some(sim) = &app.sim else { return };
    let names: Vec<String> = sim.tracer.ids.iter().map(|id| trace_display_name(app, id)).collect();
    let vcd = sim.tracer.to_vcd(&names);
    if let Some(path) = rfd::FileDialog::new()
        .add_filter("VCD", &["vcd"])
        .set_file_name(format!("{}.vcd", app.file.name))
        .save_file()
    {
        match std::fs::write(&path, vcd) {
            Ok(()) => app.toast(format!("wrote {}", path.display())),
            Err(e) => app.toast(format!("cannot write {}: {}", path.display(), e)),
        }
    }
}
