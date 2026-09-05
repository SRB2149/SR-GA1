//! Inspectors: CLB (schematic-style input muxes, operation, FF, output
//! muxes), CSB (clock source), IO controllers, and the config bit inspector.

use super::{colors, App};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use fpga_core::bitstream;
use fpga_core::config::BlockId;
use fpga_core::fabric::{BusOut, Source};
use fpga_core::naming::NetOrigin;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        match app.selected {
            Some(BlockId::Clb { col, row }) => clb_inspector(app, ui, col, row),
            Some(BlockId::Csb { col }) => csb_inspector(app, ui, col),
            None => {
                if let Some((input_side, row)) = app.io_selected {
                    io_inspector(app, ui, input_side, row);
                }
            }
        }
    });
}

// ---------------------------------------------------------------------------

fn name_header(app: &mut App, ui: &mut egui::Ui, b: BlockId) {
    ui.horizontal(|ui| {
        ui.label("name:");
        let resp = ui.add(egui::TextEdit::singleline(&mut app.rename_buf).desired_width(150.0));
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let name = app.rename_buf.clone();
            app.try_rename(b, &name);
        }
        if app.is_pinned(b) {
            ui.label(egui::RichText::new("pinned").color(Color32::from_rgb(255, 220, 120)).small());
            if ui.small_button("revert to default").clicked() {
                app.apply("revert name", move |d, _| d.revert_name(b));
                app.rename_buf = app.block_label(b);
            }
        }
    });
    if let Some(err) = app.rename_err.clone() {
        ui.colored_label(Color32::LIGHT_RED, err);
    }
}

fn source_label(app: &App, col: usize, row: usize, src: Source) -> String {
    let this = app.block_label(BlockId::Clb { col, row });
    match src {
        Source::Op => format!("op ({}{})", this, app.fabric.naming.suffix_op),
        Source::Reg => format!("reg ({}{})", this, app.fabric.naming.suffix_reg),
        Source::Carry => format!("carry ({}{})", this, app.fabric.naming.suffix_carry),
        Source::Const(false) => format!("constant 0 ({})", app.fabric.naming.constant_zero),
        Source::Const(true) => format!("constant 1 ({})", app.fabric.naming.constant_one),
        Source::HorzIn(k) => format!("pass h{} ({})", k, app.net_label(app.nets.horz_in(col, row, k))),
        Source::VertIn(k) => format!("pass v{} ({})", k, app.net_label(app.nets.vert_in(col, row, k))),
        Source::CarryIn => format!("carry-in ({})", app.net_label(app.nets.carry_in(col, row))),
    }
}

fn trace_button(app: &mut App, ui: &mut egui::Ui, id: String, label: &str) {
    let on = app.is_traced(&id);
    if ui
        .selectable_label(on, format!("{} {}", if on { "◉" } else { "○" }, label))
        .on_hover_text("flag for the waveform panel and VCD export")
        .clicked()
    {
        app.toggle_trace(id);
    }
}

// ---------------------------------------------------------------------------

fn clb_inspector(app: &mut App, ui: &mut egui::Ui, col: usize, row: usize) {
    let b = BlockId::Clb { col, row };
    ui.heading(app.block_label(b));
    name_header(app, ui, b);
    ui.separator();

    // Routed inputs, indexed list, highest lane first; clock shown apart.
    ui.label(egui::RichText::new("inputs (horizontal lanes)").strong());
    for lane in (0..app.fabric.horz_lanes).rev() {
        let origin = app.nets.horz_in(col, row, lane);
        let name = app.net_label(origin);
        let color = if matches!(origin, NetOrigin::FloatingLoop | NetOrigin::Const(_)) {
            colors::DIM
        } else {
            colors::name_color(&name, None)
        };
        ui.horizontal(|ui| {
            ui.monospace(format!("{}:", lane));
            ui.colored_label(color, name);
            if let Some(s) = app.sim.as_ref().and_then(|s| s.settled.as_ref()) {
                let v = s.horz_in(col, row, lane);
                ui.label(egui::RichText::new(if v { "1" } else { "0" }).color(colors::value_fill(v)).monospace());
            }
        });
    }
    ui.horizontal(|ui| {
        ui.colored_label(colors::CLOCK, "clk:");
        ui.colored_label(colors::CLOCK, app.clock_label(col));
        ui.label(egui::RichText::new("(dedicated network)").small().color(Color32::from_gray(120)));
    });
    ui.horizontal(|ui| {
        ui.colored_label(colors::CARRY, "carry-in:");
        ui.colored_label(colors::CARRY, app.net_label(app.nets.carry_in(col, row)));
        if let Some(s) = app.sim.as_ref().and_then(|s| s.settled.as_ref()) {
            let v = s.clb_carry_in(col, row);
            ui.label(egui::RichText::new(if v { "1" } else { "0" }).color(colors::value_fill(v)).monospace());
        }
        ui.label(egui::RichText::new("(dedicated chain)").small().color(Color32::from_gray(120)));
    });
    ui.separator();

    // Input mux schematic: four wires, three muxes straddling them.
    input_mux_schematic(app, ui, col, row);
    ui.separator();

    // Operation.
    ui.label(egui::RichText::new("operation").strong());
    let code = app.file.design.clb(col, row).slice(&app.fabric.op_select) as usize;
    let mut chosen = code;
    egui::ComboBox::from_id_salt(("op", col, row))
        .selected_text(&app.fabric.operations[code].name)
        .show_ui(ui, |ui| {
            for (i, op) in app.fabric.operations.iter().enumerate() {
                ui.selectable_value(&mut chosen, i, &op.name);
            }
        });
    if chosen != code {
        let slice = app.fabric.op_select;
        app.set_slice_clb(col, row, slice, chosen as u64, "change operation");
    }
    let table = &app.fabric.operations[app.file.design.clb(col, row).slice(&app.fabric.op_select) as usize].table;
    egui::CollapsingHeader::new("truth table")
        .id_salt(("tt", col, row))
        .show(ui, |ui| {
            ui.monospace("c b a | out  carry");
            for i in 0..table.len() {
                let (a, b_, c) = (i & 1, (i >> 1) & 1, (i >> 2) & 1);
                ui.monospace(format!(
                    "{} {} {} |  {}     {}",
                    c,
                    b_,
                    a,
                    table[i] as u8,
                    app.fabric.carry_table[i] as u8
                ));
            }
        });

    // Flip-flop.
    ui.separator();
    ui.label(egui::RichText::new("flip-flop").strong());
    ui.label(format!(
        "D is hard-wired to op; write-enable is h3 ({})",
        app.net_label(app.nets.horz_in(col, row, 3.min(app.fabric.horz_lanes - 1)))
    ));
    let rst_field = app.fabric.ff.reset_value_field;
    let mut rst = app.file.design.clb(col, row).get(rst_field) != 0;
    if ui.checkbox(&mut rst, "reset value 1 (synchronous, global reset)").changed() {
        let width = app.fabric.clb_fields[rst_field].width;
        app.apply("change FF reset value", move |d, _| {
            d.clb_mut(col, row).set(rst_field, width, rst as u64);
        });
    }
    if let Some(s) = app.sim.as_ref().and_then(|s| s.settled.as_ref()) {
        let v = s.clb_ff(col, row);
        ui.horizontal(|ui| {
            ui.label("current value:");
            ui.label(egui::RichText::new(if v { "1" } else { "0" }).color(colors::value_fill(v)).monospace());
        });
    }
    if let Some(s) = app.sim.as_ref().and_then(|s| s.settled.as_ref()) {
        let v = s.clb_carry(col, row);
        ui.horizontal(|ui| {
            ui.colored_label(colors::CARRY, "carry-out:");
            ui.label(egui::RichText::new(if v { "1" } else { "0" }).color(colors::value_fill(v)).monospace());
        });
    }
    ui.label(
        egui::RichText::new("carry-out leaves on the dedicated chain to the cell above; it is not routable onto the buses. Select carry-in on input c and XOR3 to make this cell a full adder.")
            .small()
            .color(Color32::from_gray(120)),
    );

    // Output muxes.
    ui.separator();
    ui.label(egui::RichText::new("output muxes").strong());
    let muxes: Vec<(usize, BusOut)> = app.fabric.output_muxes.iter().enumerate().map(|(i, m)| (i, m.drives)).collect();
    for (mi, drives) in muxes {
        let mux = app.fabric.output_muxes[mi].clone();
        let target = match drives {
            BusOut::Horz(l) => format!("h_out {}", l),
            BusOut::Vert(l) => format!("v_out {}", l),
        };
        let sel = app.file.design.clb(col, row).slice(&mux.select) as usize;
        let mut chosen = sel;
        ui.horizontal(|ui| {
            ui.monospace(format!("{:8}", target));
            egui::ComboBox::from_id_salt(("omux", col, row, mi))
                .width(230.0)
                .selected_text(source_label(app, col, row, mux.sources[sel]))
                .show_ui(ui, |ui| {
                    for (i, &src) in mux.sources.iter().enumerate() {
                        ui.selectable_value(&mut chosen, i, format!("{}: {}", i, source_label(app, col, row, src)));
                    }
                });
        });
        if chosen != sel {
            app.set_slice_clb(col, row, mux.select, chosen as u64, &format!("route {}", target));
        }
    }

    // Tracing.
    ui.separator();
    ui.label(egui::RichText::new("trace").strong());
    ui.horizontal(|ui| {
        trace_button(app, ui, format!("op:{}:{}", col, row), "op");
        trace_button(app, ui, format!("ff:{}:{}", col, row), "reg");
        trace_button(app, ui, format!("carry:{}:{}", col, row), "carry");
    });

    config_bits(app, ui, b);
}

/// The schematic: 4 input wires with the 3 muxes straddling adjacent pairs,
/// as wired in the RTL. Clicking a mux's candidate dot picks that source.
fn input_mux_schematic(app: &mut App, ui: &mut egui::Ui, col: usize, row: usize) {
    ui.label(egui::RichText::new("input muxes").strong());

    // One rail per source any input mux can select: the horizontal lanes it
    // straddles, then the vertical lanes and the carry chain that input c
    // reaches. Highest lane on top, matching the fabric view.
    let mut rails: Vec<Source> = Vec::new();
    for m in &app.fabric.input_muxes {
        for &s in &m.sources {
            if !rails.contains(&s) {
                rails.push(s);
            }
        }
    }
    let order = |s: &Source| match *s {
        Source::HorzIn(l) => (0usize, app.fabric.horz_lanes - 1 - l),
        Source::VertIn(l) => (1, app.fabric.vert_lanes - 1 - l),
        Source::CarryIn => (2, 0),
        _ => (3, 0),
    };
    rails.sort_by_key(order);

    let rail_origin = |s: Source| match s {
        Source::HorzIn(l) => app.nets.horz_in(col, row, l),
        Source::VertIn(l) => app.nets.vert_in(col, row, l),
        Source::CarryIn => app.nets.carry_in(col, row),
        Source::Const(b) => NetOrigin::Const(b),
        _ => NetOrigin::FloatingLoop,
    };
    let rail_tag = |s: Source| match s {
        Source::HorzIn(l) => format!("h{}", l),
        Source::VertIn(l) => format!("v{}", l),
        Source::CarryIn => "cin".to_string(),
        _ => "k".to_string(),
    };

    let width = ui.available_width().max(260.0);
    let spacing = 24.0;
    let height = rails.len() as f32 * spacing + 24.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter_at(rect);
    let font = FontId::proportional(11.0);
    let rail_y = |i: usize| rect.min.y + 14.0 + i as f32 * spacing;

    for (i, &s) in rails.iter().enumerate() {
        let origin = rail_origin(s);
        let name = app.net_label(origin);
        let color = if matches!(origin, NetOrigin::FloatingLoop | NetOrigin::Const(_)) {
            colors::DIM
        } else {
            colors::name_color(&name, None)
        };
        let y = rail_y(i);
        // The dedicated carry chain is not a bus: draw it dashed.
        let a = Pos2::new(rect.min.x + 4.0, y);
        let b = Pos2::new(rect.max.x - 116.0, y);
        if s == Source::CarryIn {
            painter.extend(egui::Shape::dashed_line(&[a, b], Stroke::new(2.0_f32, colors::CARRY), 5.0, 3.0));
        } else {
            painter.line_segment([a, b], Stroke::new(2.0_f32, color));
        }
        painter.text(
            Pos2::new(rect.min.x + 6.0, y - 3.0),
            Align2::LEFT_BOTTOM,
            format!("{}: {}", rail_tag(s), name),
            font.clone(),
            if s == Source::CarryIn { colors::CARRY } else { color },
        );
    }

    let mut clicks: Vec<(usize, u64)> = Vec::new();
    for (k, m) in app.fabric.input_muxes.iter().enumerate() {
        let sel = app.file.design.clb(col, row).slice(&m.select);
        let mux_x = rect.max.x - 108.0 + k as f32 * 34.0;
        // The straddled rails.
        let ys: Vec<f32> = m
            .sources
            .iter()
            .map(|s| rail_y(rails.iter().position(|r| r == s).unwrap_or(0)))
            .collect();
        let top = ys.iter().cloned().fold(f32::MAX, f32::min) - 8.0;
        let bot = ys.iter().cloned().fold(f32::MIN, f32::max) + 8.0;
        let body = Rect::from_min_max(Pos2::new(mux_x, top), Pos2::new(mux_x + 22.0, bot));
        painter.rect_filled(body, 4.0, Color32::from_gray(48));
        painter.rect_stroke(body, 4.0, Stroke::new(1.0_f32, Color32::from_gray(110)));
        painter.text(body.center_bottom() + Vec2::new(0.0, 12.0), Align2::CENTER_CENTER, &m.name, font.clone(), Color32::from_gray(200));
        for (i, y) in ys.iter().enumerate() {
            let dot = Pos2::new(mux_x + 11.0, *y);
            let active = sel == i as u64;
            let dot_rect = Rect::from_center_size(dot, Vec2::splat(14.0));
            let resp = ui.interact(dot_rect, ui.id().with(("imux", col, row, k, i)), Sense::click());
            painter.circle_filled(
                dot,
                if active { 5.0 } else { 3.5 },
                if active {
                    Color32::from_rgb(90, 170, 255)
                } else if resp.hovered() {
                    Color32::from_gray(180)
                } else {
                    Color32::from_gray(110)
                },
            );
            if resp.clicked() && !active {
                clicks.push((k, i as u64));
            }
        }
    }
    for (k, val) in clicks {
        let m = app.fabric.input_muxes[k].clone();
        app.set_slice_clb(col, row, m.select, val, &format!("input mux {}", m.name));
    }
}

// ---------------------------------------------------------------------------

fn csb_inspector(app: &mut App, ui: &mut egui::Ui, col: usize) {
    let b = BlockId::Csb { col };
    ui.heading(app.block_label(b));
    name_header(app, ui, b);
    ui.separator();

    ui.horizontal(|ui| {
        ui.colored_label(colors::CLOCK, "resolved clock:");
        ui.colored_label(colors::CLOCK, app.clock_label(col));
    });
    if let Some(s) = app.sim.as_ref().and_then(|s| s.settled.as_ref()) {
        let v = s.clocks[col];
        ui.horizontal(|ui| {
            ui.label("current value:");
            ui.label(egui::RichText::new(if v { "1" } else { "0" }).color(colors::value_fill(v)).monospace());
        });
    }
    ui.separator();

    let clock = app.fabric.csb_clock.clone();
    let coupled = app.file.design.csb(col).get(clock.couple_field) != 0;
    let prev = (col + app.fabric.columns - 1) % app.fabric.columns;
    let mut want_coupled = coupled;
    ui.checkbox(
        &mut want_coupled,
        format!("couple to previous ({})", app.block_label(BlockId::Csb { col: prev })),
    );
    if want_coupled != coupled {
        let field = clock.couple_field;
        let width = app.fabric.csb_fields[field].width;
        app.apply("couple CSB", move |d, _| {
            d.csb_mut(col).set(field, width, want_coupled as u64);
        });
    }

    let sel = app.file.design.csb(col).slice(&clock.select) as usize;
    ui.add_enabled_ui(!want_coupled, |ui| {
        ui.label("vertical ring tap (at the loop point):");
        let mut chosen = sel;
        for (i, &lane) in clock.ring_lanes.iter().enumerate() {
            let name = app.net_label(app.nets.ring_tap(col, lane));
            ui.radio_value(&mut chosen, i, format!("lane {} — {}", lane, name));
        }
        if chosen != sel {
            app.set_slice_csb(col, clock.select, chosen as u64, "select clock tap");
        }
    });
    ui.label(
        egui::RichText::new("the clock feeds every CLB in this column on the dedicated network; it is not routable on the fabric")
            .small()
            .color(Color32::from_gray(120)),
    );
    ui.separator();
    trace_button(app, ui, format!("clk:{}", col), "trace this clock");
    config_bits(app, ui, b);
}

// ---------------------------------------------------------------------------

fn io_inspector(app: &mut App, ui: &mut egui::Ui, input_side: bool, row: usize) {
    ui.heading(if input_side {
        format!("input controller — row {}", row)
    } else {
        format!("output controller — row {}", row)
    });
    ui.label("IO controllers hold no configuration bits.");
    ui.separator();
    for lane in (0..app.fabric.horz_lanes).rev() {
        let name = if input_side {
            Some(app.fabric.io_inputs[row][lane].clone())
        } else {
            app.fabric.io_outputs[row][lane].clone()
        };
        ui.horizontal(|ui| {
            ui.monospace(format!("{}:", lane));
            match name {
                Some(name) => {
                    let constant = name == app.fabric.naming.constant_zero || name == app.fabric.naming.constant_one;
                    ui.colored_label(
                        if constant { colors::DIM } else { colors::name_color(&name, None) },
                        &name,
                    );
                    if let Some(s) = app.sim.as_ref().and_then(|s| s.settled.as_ref()) {
                        let v = if input_side { s.horz_in(0, row, lane) } else { s.horz_edge(row, lane) };
                        ui.label(egui::RichText::new(if v { "1" } else { "0" }).color(colors::value_fill(v)).monospace());
                    }
                    if !constant {
                        let id = format!("{}:{}", if input_side { "in" } else { "out" }, name);
                        trace_button(app, ui, id, "trace");
                    }
                }
                None => {
                    ui.colored_label(colors::DIM, "— unused");
                }
            }
        });
    }
    if !app.fabric.ddio.is_empty() {
        ui.separator();
        for d in &app.fabric.ddio {
            ui.label(
                egui::RichText::new(format!("{} is forced 0 while {} = 1 (pad direction)", d.input, d.dir))
                    .small()
                    .color(Color32::from_gray(120)),
            );
        }
    }
    if input_side {
        ui.separator();
        ui.label(
            egui::RichText::new("drive these pins from the stimulus panel below")
                .small()
                .color(Color32::from_gray(120)),
        );
    }
}

// ---------------------------------------------------------------------------

fn config_bits(app: &App, ui: &mut egui::Ui, b: BlockId) {
    egui::CollapsingHeader::new("config bits (bitstream positions)")
        .id_salt(("bits", b))
        .show(ui, |ui| {
            let fields = match b {
                BlockId::Clb { .. } => &app.fabric.clb_fields,
                BlockId::Csb { .. } => &app.fabric.csb_fields,
            };
            let owned = bitstream::block_bits(&app.fabric, b);
            ui.monospace("stream  field                bit  value");
            for (stream, fi, bit) in owned {
                let value = match b {
                    BlockId::Clb { col, row } => app.file.design.clb(col, row).get(fi),
                    BlockId::Csb { col } => app.file.design.csb(col).get(fi),
                };
                ui.monospace(format!(
                    "{:6}  {:20} {:3}  {}",
                    stream,
                    fields[fi].name,
                    bit,
                    (value >> bit) & 1
                ));
            }
            ui.label(
                egui::RichText::new("stream = position in the exported bitstream (0 is transmitted first)")
                    .small()
                    .color(Color32::from_gray(120)),
            );
        });
}
