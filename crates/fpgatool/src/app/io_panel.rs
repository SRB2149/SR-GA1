//! The IO controller inspector: per-pad names and loop-around board wiring.
//!
//! Two things live here that are not configuration. A **pad name** is the
//! design's own name for the signal on a pin — a display alias, so the pad keeps
//! the reserved identity the fabric gave it and stimulus keys, trace ids and the
//! bitstream are untouched. A **loop** is a physical wire on the board from a
//! chip output back to a chip input, which costs no configuration bits but is
//! the only way a signal can travel leftward or return to the cell that produced
//! it; see `docs/fabric-notes.md`.

use super::inspector::trace_button;
use super::{colors, App};
use eframe::egui;
use egui::Color32;
use fpga_core::config::IoPad;

/// Key for the per-pad name edit buffers. One shared buffer would fight itself
/// across four lanes on screen at once.
pub fn buf_key(pad: IoPad) -> String {
    format!("{}:{}:{}", if pad.input_side { "in" } else { "out" }, pad.row, pad.lane)
}

pub fn show(app: &mut App, ui: &mut egui::Ui, input_side: bool, row: usize) {
    ui.heading(if input_side {
        format!("input controller — row {}", row)
    } else {
        format!("output controller — row {}", row)
    });
    ui.label("IO controllers hold no configuration bits.");
    ui.separator();

    for lane in (0..app.fabric.horz_lanes).rev() {
        let pad = IoPad { input_side, row, lane };
        pad_row(app, ui, pad);
    }

    ui.separator();
    loop_section(app, ui, input_side, row);

    if !app.fabric.ddio.is_empty() {
        ui.separator();
        for d in &app.fabric.ddio {
            ui.label(
                egui::RichText::new(format!(
                    "{} is forced 0 while {} = 1 (pad direction)",
                    d.input, d.dir
                ))
                .small()
                .color(Color32::from_gray(120)),
            );
        }
    }
}

/// One lane: its name, its live value, its trace toggle, and an editable name.
fn pad_row(app: &mut App, ui: &mut egui::Ui, pad: IoPad) {
    let reserved = pad.reserved_name(&app.fabric);
    let Some(reserved) = reserved else {
        ui.horizontal(|ui| {
            ui.monospace(format!("{}:", pad.lane));
            ui.colored_label(colors::DIM, "— unused");
        });
        return;
    };
    let constant = pad.is_constant(&app.fabric);
    let shown = app
        .file
        .design
        .effective_io_name(&app.fabric, pad)
        .unwrap_or_else(|| reserved.clone());
    let named = app.file.design.io_name(pad).is_some();
    let driver = if pad.input_side {
        app.file.design.loopback_driver(pad)
    } else {
        None
    };

    ui.horizontal(|ui| {
        ui.monospace(format!("{}:", pad.lane));
        if constant {
            // Tied to a constant by the fabric: no design signal, nothing to
            // name and nothing to drive.
            ui.colored_label(colors::DIM, &shown);
            return;
        }

        let key = buf_key(pad);
        if !app.io_name_bufs.contains_key(&key) {
            app.io_name_bufs.insert(key.clone(), shown.clone());
        }
        let mut commit: Option<String> = None;
        if let Some(buf) = app.io_name_bufs.get_mut(&key) {
            let resp = ui.add(
                egui::TextEdit::singleline(buf)
                    .desired_width(130.0)
                    .text_color(colors::name_color(&shown, None)),
            );
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                commit = Some(buf.to_string());
            }
        }
        if let Some(name) = commit {
            app.try_rename_io(pad, &name);
        }

        if let Some(s) = app.sim.as_ref().and_then(|s| s.settled.as_ref()) {
            let v = if pad.input_side {
                s.horz_in(0, pad.row, pad.lane)
            } else {
                s.horz_edge(pad.row, pad.lane)
            };
            ui.label(
                egui::RichText::new(if v { "1" } else { "0" })
                    .color(colors::value_fill(v))
                    .monospace(),
            );
        }

        let id = format!("{}:{}", if pad.input_side { "in" } else { "out" }, reserved);
        trace_button(app, ui, id, "trace");
    });

    // The pad's own identity, and what it is doing, under the field.
    ui.horizontal(|ui| {
        ui.add_space(22.0);
        let mut note = format!("pin {}", reserved);
        if let Some(from) = driver {
            let from_name = app
                .file
                .design
                .effective_io_name(&app.fabric, from)
                .unwrap_or_default();
            note.push_str(&format!(" — driven by {} through board wiring", from_name));
        }
        ui.label(egui::RichText::new(note).small().color(Color32::from_gray(120)));
        if named {
            ui.label(
                egui::RichText::new("named")
                    .color(Color32::from_rgb(255, 220, 120))
                    .small(),
            );
            if ui.small_button("revert").clicked() {
                app.apply("revert pad name", move |d, _| d.revert_io_name(pad));
                app.io_name_bufs.remove(&buf_key(pad));
            }
        }
    });
    if let Some(err) = app.io_rename_err.clone() {
        if app.io_rename_pad == Some(pad) {
            ui.colored_label(Color32::LIGHT_RED, err);
        }
    }
}

/// Add, change or remove the board wiring for this controller's lanes.
fn loop_section(app: &mut App, ui: &mut egui::Ui, input_side: bool, row: usize) {
    ui.label(egui::RichText::new("loop-around wiring").strong());
    ui.label(
        egui::RichText::new(
            "a physical wire on the board from a chip output back to a chip input. It costs \
             no configuration, and it is the only way a signal can travel leftward or return \
             to the cell that produced it.",
        )
        .small()
        .color(Color32::from_gray(120)),
    );

    if input_side {
        // Choose the output driving each input lane on this row.
        let outputs = output_pads(app);
        for lane in (0..app.fabric.horz_lanes).rev() {
            let pad = IoPad::input(row, lane);
            if pad.reserved_name(&app.fabric).is_none() || pad.is_constant(&app.fabric) {
                continue;
            }
            let current = app.file.design.loopback_driver(pad);
            let label = match current {
                Some(from) => app
                    .file
                    .design
                    .effective_io_name(&app.fabric, from)
                    .unwrap_or_else(|| "?".to_string()),
                None => "— not looped".to_string(),
            };
            let mut choice: Option<Option<IoPad>> = None;
            ui.horizontal(|ui| {
                ui.monospace(format!("{}:", lane));
                egui::ComboBox::from_id_salt(("loopsrc", row, lane))
                    .selected_text(label)
                    .width(190.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(current.is_none(), "— not looped").clicked() {
                            choice = Some(None);
                        }
                        for (out_pad, name) in &outputs {
                            if ui
                                .selectable_label(current == Some(*out_pad), name)
                                .clicked()
                            {
                                choice = Some(Some(*out_pad));
                            }
                        }
                    });
            });
            match choice {
                Some(Some(from)) => app.set_loopback(from, pad),
                Some(None) => {
                    app.apply("remove loop", move |d, _| d.clear_loopback(pad));
                }
                None => {}
            }
        }
        ui.label(
            egui::RichText::new(
                "a looped input is driven by the board, so it takes no stimulus",
            )
            .small()
            .color(Color32::from_gray(120)),
        );
    } else {
        // Outputs are read-only here: a loop is owned by the input it drives.
        let mut any = false;
        for lane in (0..app.fabric.horz_lanes).rev() {
            let pad = IoPad::output(row, lane);
            let targets = app.file.design.loopback_targets(pad);
            if targets.is_empty() {
                continue;
            }
            any = true;
            let names: Vec<String> = targets
                .iter()
                .map(|t| {
                    app.file
                        .design
                        .effective_io_name(&app.fabric, *t)
                        .unwrap_or_else(|| "?".to_string())
                })
                .collect();
            ui.horizontal(|ui| {
                ui.monospace(format!("{}:", lane));
                ui.label(format!("feeds {}", names.join(", ")));
            });
        }
        if !any {
            ui.label(
                egui::RichText::new("none of these outputs is looped back")
                    .small()
                    .color(Color32::from_gray(120)),
            );
        }
        ui.label(
            egui::RichText::new("add a loop from the input it drives")
                .small()
                .color(Color32::from_gray(120)),
        );
    }
}

/// Every output pad that could drive a loop, with its display name.
fn output_pads(app: &App) -> Vec<(IoPad, String)> {
    let mut out = Vec::new();
    for row in 0..app.fabric.rows {
        for lane in 0..app.fabric.horz_lanes {
            let pad = IoPad::output(row, lane);
            if let Some(name) = app.file.design.effective_io_name(&app.fabric, pad) {
                out.push((pad, name));
            }
        }
    }
    out
}
