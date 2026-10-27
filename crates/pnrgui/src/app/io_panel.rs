//! The IO controller inspector, opened from either edge tile of a row.
//!
//! The left side shows what enters each of the row's lanes. Lanes 0/1 are
//! input muxes over the chip inputs, the DDIO pads and `output_0..3` (an
//! on-chip loop from the right edge); lanes 2/3 are fixed pins. The right side
//! shows which output-side pads read the row's lanes, with a lane picker for
//! every muxed pad. Both sides edit the IO controller's configuration bits.
//!
//! Two things live here that are not configuration. A **pad name** is the
//! design's own name for the signal on a pin — a display alias, so the pad keeps
//! the reserved identity the fabric gave it and stimulus keys, trace ids and the
//! bitstream are untouched. A **board loop** is a physical wire from a chip
//! output back to a chip input, for loops beyond what the controller can close
//! on-chip; see `docs/loop-around.md`.

use super::inspector::{config_bits, trace_button};
use super::{colors, App};
use eframe::egui;
use egui::Color32;
use fpga_core::config::{BlockId, IoPad};
use fpga_core::fabric::{IoSource, LeftLane, PadDrive};

/// Key for the per-pad name edit buffers. One shared buffer would fight itself
/// across several pads on screen at once.
pub fn buf_key(pad: IoPad) -> String {
    format!("{}:{}", if pad.input_side { "in" } else { "out" }, pad.index)
}

fn note(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(egui::RichText::new(text.into()).small().color(Color32::from_gray(120)));
}

pub fn show(app: &mut App, ui: &mut egui::Ui, input_side: bool, row: usize) {
    let name = app.block_label(BlockId::Io);
    ui.heading(if input_side {
        format!("{} — row {} inputs", name, row)
    } else {
        format!("{} — row {} outputs", name, row)
    });
    ui.separator();
    if input_side {
        left_side(app, ui, row);
    } else {
        right_side(app, ui, row);
    }

    if !app.fabric.ddio.is_empty() {
        ui.separator();
        for d in app.fabric.ddio.clone() {
            note(
                ui,
                format!(
                    "{} reads back {} while {} = 1 (output mode), else the pin",
                    app.fabric.io_inputs[d.input], app.fabric.io_outputs[d.output], app.fabric.io_outputs[d.dir]
                ),
            );
        }
    }
    ui.separator();
    config_bits(app, ui, BlockId::Io);
}

// ---------------------------------------------------------------------------
// Left edge: what enters each lane.

fn left_side(app: &mut App, ui: &mut egui::Ui, row: usize) {
    ui.label(egui::RichText::new("lanes entering the fabric").strong());
    for lane in (0..app.fabric.horz_lanes).rev() {
        let current = app.file.design.left_source(&app.fabric, row, lane);
        ui.horizontal(|ui| {
            ui.monospace(format!("{}:", lane));
            match app.fabric.io_left[row][lane] {
                LeftLane::Mux(sel) => {
                    let code = app.file.design.io().slice(&sel) as usize;
                    let mut chosen = code;
                    egui::ComboBox::from_id_salt(("iomux", row, lane))
                        .width(220.0)
                        .selected_text(app.io_source_label(current))
                        .show_ui(ui, |ui| {
                            for (i, &src) in app.fabric.io_mux_sources.iter().enumerate() {
                                ui.selectable_value(&mut chosen, i, format!("{:2}: {}", i, app.io_source_label(src)));
                            }
                        });
                    if chosen != code {
                        app.set_slice_io(sel, chosen as u64, &format!("row {} lane {} input", row, lane));
                    }
                }
                LeftLane::Fixed(_) => {
                    ui.label(app.io_source_label(current));
                    ui.label(egui::RichText::new("fixed").small().color(Color32::from_gray(120)));
                }
            }
            if let Some(s) = app.sim.as_ref().and_then(|s| s.settled.as_ref()) {
                let v = s.horz_in(0, row, lane);
                ui.label(egui::RichText::new(if v { "1" } else { "0" }).color(colors::value_fill(v)).monospace());
            }
        });
    }

    // The pads this row reads, for naming, tracing and board wiring.
    let mut pads: Vec<usize> = Vec::new();
    for lane in (0..app.fabric.horz_lanes).rev() {
        if let IoSource::InPad(i) = app.file.design.left_source(&app.fabric, row, lane) {
            if !pads.contains(&i) {
                pads.push(i);
            }
        }
    }
    ui.separator();
    ui.label(egui::RichText::new("pins read by this row").strong());
    for i in pads {
        pad_row(app, ui, IoPad::input(i));
        board_loop_picker(app, ui, IoPad::input(i));
    }
    note(ui, "a pin wired back from an output on the board takes no stimulus");
}

/// Choose the output the board wires to an input pin.
fn board_loop_picker(app: &mut App, ui: &mut egui::Ui, pad: IoPad) {
    let outputs: Vec<(IoPad, String)> = (0..app.fabric.io_outputs.len())
        .map(IoPad::output)
        .filter_map(|p| Some((p, app.file.design.effective_io_name(&app.fabric, p)?)))
        .collect();
    let current = app.file.design.loopback_driver(pad);
    let label = match current {
        Some(from) => format!("board wire from {}", app.file.design.effective_io_name(&app.fabric, from).unwrap_or_default()),
        None => "no board wire".to_string(),
    };
    let mut choice: Option<Option<IoPad>> = None;
    ui.horizontal(|ui| {
        ui.add_space(22.0);
        egui::ComboBox::from_id_salt(("boardloop", pad.index))
            .selected_text(label)
            .width(200.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(current.is_none(), "no board wire").clicked() {
                    choice = Some(None);
                }
                for (out_pad, name) in &outputs {
                    if ui.selectable_label(current == Some(*out_pad), format!("board wire from {}", name)).clicked() {
                        choice = Some(Some(*out_pad));
                    }
                }
            });
    });
    match choice {
        Some(Some(from)) => app.set_loopback(from, pad),
        Some(None) => app.apply("remove loop", move |d, _| d.clear_loopback(pad)),
        None => {}
    }
}

// ---------------------------------------------------------------------------
// Right edge: which pads read each lane.

fn right_side(app: &mut App, ui: &mut egui::Ui, row: usize) {
    ui.label(egui::RichText::new("lanes leaving the fabric").strong());
    for lane in (0..app.fabric.horz_lanes).rev() {
        let readers: Vec<String> = (0..app.fabric.io_outputs.len())
            .filter(|&o| app.file.design.pad_lane(&app.fabric, o) == (row, lane))
            .map(|o| app.pad_display_name(&app.fabric.io_outputs[o]))
            .collect();
        ui.horizontal(|ui| {
            ui.monospace(format!("{}:", lane));
            ui.label(app.net_label(app.nets.horz_edge(row, lane)));
            ui.label(
                egui::RichText::new(if readers.is_empty() {
                    "→ no pad".to_string()
                } else {
                    format!("→ {}", readers.join(", "))
                })
                .color(Color32::from_gray(160)),
            );
            if let Some(s) = app.sim.as_ref().and_then(|s| s.settled.as_ref()) {
                let v = s.horz_edge(row, lane);
                ui.label(egui::RichText::new(if v { "1" } else { "0" }).color(colors::value_fill(v)).monospace());
            }
        });
    }

    // Fixed pads on this row first, then every muxed pad (each can read any
    // row, so they are listed from every row's tile).
    let fixed: Vec<usize> = (0..app.fabric.io_outputs.len())
        .filter(|&o| matches!(app.fabric.io_drive[o], PadDrive::Fixed { row: r, .. } if r == row))
        .collect();
    let muxed: Vec<usize> = (0..app.fabric.io_outputs.len())
        .filter(|&o| matches!(app.fabric.io_drive[o], PadDrive::Mux(_)))
        .collect();
    ui.separator();
    if !fixed.is_empty() {
        ui.label(egui::RichText::new("pins fixed to this row").strong());
        for o in fixed {
            pad_row(app, ui, IoPad::output(o));
        }
        ui.separator();
    }
    ui.label(egui::RichText::new("muxed pins (any lane)").strong());
    for o in muxed {
        pad_row(app, ui, IoPad::output(o));
        let PadDrive::Mux(sel) = app.fabric.io_drive[o] else { continue };
        let code = app.file.design.io().slice(&sel) as usize;
        let mut chosen = code;
        let lane_label = |app: &App, code: usize| {
            let (r, l) = app.fabric.drive_lane(code);
            format!("row {} lane {} ({})", r, l, app.net_label(app.nets.horz_edge(r, l)))
        };
        ui.horizontal(|ui| {
            ui.add_space(22.0);
            ui.label("reads");
            egui::ComboBox::from_id_salt(("omuxpad", o))
                .width(240.0)
                .selected_text(lane_label(app, code))
                .show_ui(ui, |ui| {
                    for c in 0..app.fabric.rows * app.fabric.horz_lanes {
                        ui.selectable_value(&mut chosen, c, format!("{:2}: {}", c, lane_label(app, c)));
                    }
                });
        });
        if chosen != code {
            let name = app.fabric.io_outputs[o].clone();
            app.set_slice_io(sel, chosen as u64, &format!("route {}", name));
        }
    }

    // On-chip and board loops leaving from here, read-only: a loop is owned
    // by the input it drives.
    let mut loops: Vec<String> = Vec::new();
    for r in 0..app.fabric.rows {
        for l in 0..app.fabric.horz_lanes {
            if let IoSource::OutPad(o) = app.file.design.left_source(&app.fabric, r, l) {
                loops.push(format!("{} → row {} lane {} (on-chip)", app.pad_display_name(&app.fabric.io_outputs[o]), r, l));
            }
        }
    }
    for link in app.file.design.loopback() {
        let from = app.file.design.effective_io_name(&app.fabric, link.from).unwrap_or_default();
        let to = app.file.design.effective_io_name(&app.fabric, link.to).unwrap_or_default();
        loops.push(format!("{} → {} (board wire)", from, to));
    }
    ui.separator();
    ui.label(egui::RichText::new("loops back to the left edge").strong());
    if loops.is_empty() {
        note(ui, "none — set a row's lane 0/1 input to output_0..3 to loop back on-chip");
    }
    for l in loops {
        ui.label(l);
    }
}

// ---------------------------------------------------------------------------

/// One pad: an editable name, its live value and its trace toggle.
fn pad_row(app: &mut App, ui: &mut egui::Ui, pad: IoPad) {
    let Some(reserved) = pad.reserved_name(&app.fabric) else { return };
    let shown = app.file.design.effective_io_name(&app.fabric, pad).unwrap_or_else(|| reserved.clone());
    let named = app.file.design.io_name(pad).is_some();

    ui.horizontal(|ui| {
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
            let v = s.pad(pad);
            ui.label(egui::RichText::new(if v { "1" } else { "0" }).color(colors::value_fill(v)).monospace());
        }
        let id = format!("{}:{}", if pad.input_side { "in" } else { "out" }, reserved);
        trace_button(app, ui, id, "trace");
    });
    ui.horizontal(|ui| {
        ui.add_space(22.0);
        note(ui, format!("pin {}", reserved));
        if named {
            ui.label(egui::RichText::new("named").color(Color32::from_rgb(255, 220, 120)).small());
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
