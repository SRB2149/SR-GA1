//! The main fabric view: the whole FPGA on one pan/zoom canvas — CLB grid,
//! CSBs with their chain, IO controllers, labelled bus wires, and the
//! dedicated clock network as a visually distinct layer.

use super::{colors, App};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use fpga_core::config::BlockId;
use fpga_core::fabric::Fabric;
use fpga_core::naming::NetOrigin;
use std::collections::HashMap;

pub const TILE_W: f32 = 150.0;
pub const TILE_H: f32 = 100.0;
const GAP_X: f32 = 96.0;
const GAP_Y: f32 = 64.0;
const IO_W: f32 = 84.0;
const CSB_H: f32 = 44.0;
const CSB_GAP: f32 = 112.0;
/// Where the vertical lanes stop below row 0: the selected lane alone carries
/// on down to the tap channel, so the tap never crosses the other lanes.
const STUB_ABOVE_CSB: f32 = 46.0;
/// The horizontal run from the selected ring lane to its CSB.
const TAP_ABOVE_CSB: f32 = 18.0;
/// The clock's run from the CSB's top corner out to the column's clock line.
const CLOCK_EXIT_ABOVE_CSB: f32 = 8.0;
const MARGIN: f32 = 40.0;

fn col_x(c: usize) -> f32 {
    MARGIN + IO_W + GAP_X + c as f32 * (TILE_W + GAP_X)
}

fn row_y(fabric: &Fabric, r: usize) -> f32 {
    // Row 0 at the bottom.
    MARGIN + (fabric.rows - 1 - r) as f32 * (TILE_H + GAP_Y)
}

fn lane_y(fabric: &Fabric, y0: f32, lane: usize) -> f32 {
    // Bit 0 at the bottom, bit 3 at the top.
    y0 + 22.0 + (fabric.horz_lanes - 1 - lane) as f32 * 19.0
}

fn vlane_x(fabric: &Fabric, x0: f32, lane: usize) -> f32 {
    // Bit 3 on the left, bit 0 on the right.
    x0 + 26.0 + (fabric.vert_lanes - 1 - lane) as f32 * 24.0
}

fn clk_x(x0: f32) -> f32 {
    x0 + TILE_W + GAP_X * 0.68
}

fn csb_y(fabric: &Fabric) -> f32 {
    row_y(fabric, 0) + TILE_H + CSB_GAP
}

fn stub_end_y(fabric: &Fabric) -> f32 {
    csb_y(fabric) - STUB_ABOVE_CSB
}

fn right_io_x(fabric: &Fabric) -> f32 {
    col_x(fabric.columns - 1) + TILE_W + GAP_X
}

/// How far below the CSB row a loop-around wire is routed. Each loop gets its
/// own lane so several can run side by side without overlapping.
const LOOP_LANE_H: f32 = 18.0;
const LOOP_FIRST_LANE: f32 = 40.0;

fn loop_lane_y(fabric: &Fabric, index: usize) -> f32 {
    csb_y(fabric) + CSB_H + LOOP_FIRST_LANE + index as f32 * LOOP_LANE_H
}

/// The extent "zoom to fit" uses. Loop-around wires are routed around the
/// outside of the grid, so the extent has to grow with them or they are drawn
/// off-screen the moment the view is fitted.
pub fn world_rect_with(fabric: &Fabric, loops: usize) -> Rect {
    let bottom = if loops == 0 {
        csb_y(fabric) + CSB_H + MARGIN
    } else {
        loop_lane_y(fabric, loops - 1) + MARGIN
    };
    Rect::from_min_max(
        Pos2::new(0.0, 0.0),
        Pos2::new(right_io_x(fabric) + IO_W + MARGIN, bottom),
    )
}

pub fn block_rect(fabric: &Fabric, b: BlockId) -> Rect {
    match b {
        BlockId::Clb { col, row } => Rect::from_min_size(
            Pos2::new(col_x(col), row_y(fabric, row)),
            Vec2::new(TILE_W, TILE_H),
        ),
        BlockId::Csb { col } => Rect::from_min_size(
            Pos2::new(col_x(col) + TILE_W * 0.12, csb_y(fabric)),
            Vec2::new(TILE_W * 0.76, CSB_H),
        ),
        // The controller spans both edges; its input tiles stand for it.
        BlockId::Io => io_rect(fabric, true, fabric.rows - 1).union(io_rect(fabric, true, 0)),
    }
}

fn io_rect(fabric: &Fabric, input_side: bool, row: usize) -> Rect {
    let x = if input_side { MARGIN } else { right_io_x(fabric) };
    Rect::from_min_size(Pos2::new(x, row_y(fabric, row)), Vec2::new(IO_W, TILE_H))
}

/// One wire from the right edge back to the left, as drawn.
struct LoopWire {
    /// Right-edge (row, lane) it leaves from.
    from: (usize, usize),
    /// Left-edge (row, lane) it enters on.
    to: (usize, usize),
    on_chip: bool,
    label: String,
}

/// Every loop to draw: each left-edge lane the IO controller feeds from an
/// output, then each board wire into a pin some lane reads.
fn loop_wires(app: &App) -> Vec<LoopWire> {
    let fabric = &app.fabric;
    let design = &app.file.design;
    let mut wires = Vec::new();
    for row in 0..fabric.rows {
        for lane in 0..fabric.horz_lanes {
            if let fpga_core::fabric::IoSource::OutPad(o) = design.left_source(fabric, row, lane) {
                wires.push(LoopWire {
                    from: design.pad_lane(fabric, o),
                    to: (row, lane),
                    on_chip: true,
                    label: format!("on-chip: {} → row {} lane {}", app.pad_display_name(&fabric.io_outputs[o]), row, lane),
                });
            }
        }
    }
    for link in design.loopback() {
        if link.from.input_side || !link.to.input_side {
            continue;
        }
        let from_name = design.effective_io_name(fabric, link.from).unwrap_or_default();
        let to_name = design.effective_io_name(fabric, link.to).unwrap_or_default();
        for to in design.lanes_reading(fabric, fpga_core::fabric::IoSource::InPad(link.to.index)) {
            wires.push(LoopWire {
                from: design.pad_lane(fabric, link.from.index),
                to,
                on_chip: false,
                label: format!("board: {} → {}", from_name, to_name),
            });
        }
    }
    wires
}

pub fn show(app: &mut App, ui: &mut egui::Ui, now: f64) {
    let avail = ui.available_rect_before_wrap();
    let response = ui.allocate_rect(avail, Sense::click_and_drag());
    let painter = ui.painter_at(avail);
    painter.rect_filled(avail, 0.0, Color32::from_gray(24));

    let world = world_rect_with(&app.fabric, loop_wires(app).len());
    if app.fit_requested {
        app.fit_requested = false;
        let zoom = (avail.width() / world.width()).min(avail.height() / world.height()) * 0.96;
        app.cam.zoom = zoom.clamp(0.05, 4.0);
        app.cam.pan = (avail.center() - avail.min.to_vec2() - world.center().to_vec2() * app.cam.zoom).to_vec2();
    }
    if let Some(b) = app.center_on.take() {
        let r = block_rect(&app.fabric, b);
        app.cam.pan = (avail.center() - avail.min.to_vec2() - r.center().to_vec2() * app.cam.zoom).to_vec2();
    }

    // Pan and zoom.
    if response.dragged() {
        app.cam.pan += response.drag_delta();
    }
    if let Some(hover) = response.hover_pos() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.1 {
            let factor = (1.0 + scroll * 0.0015).clamp(0.5, 2.0);
            let new_zoom = (app.cam.zoom * factor).clamp(0.05, 4.0);
            let factor = new_zoom / app.cam.zoom;
            let mouse = hover - avail.min.to_vec2();
            app.cam.pan = mouse.to_vec2() - (mouse.to_vec2() - app.cam.pan) * factor;
            app.cam.zoom = new_zoom;
        }
    }

    let zoom = app.cam.zoom;
    let to_screen = |p: Pos2| -> Pos2 { avail.min + app.cam.pan + p.to_vec2() * zoom };
    let detail = zoom > 0.42;
    let seg_labels = detail && app.show_net_names;
    let overlay = app.show_util_overlay;
    let lane_font = FontId::proportional((11.0 * zoom).clamp(8.0, 15.0));
    let block_font = FontId::proportional((14.0 * zoom).clamp(9.0, 20.0));
    let small_font = FontId::proportional((10.5 * zoom).clamp(8.0, 14.0));

    // Bus index ranges for bracketed names (colour brightness scaling).
    let mut bus_max: HashMap<String, usize> = HashMap::new();
    let mut all_origins: Vec<NetOrigin> = Vec::new();
    for row in 0..app.fabric.rows {
        for lane in 0..app.fabric.horz_lanes {
            for col in 0..=app.fabric.columns {
                all_origins.push(if col < app.fabric.columns {
                    app.nets.horz_in(col, row, lane)
                } else {
                    app.nets.horz_edge(row, lane)
                });
            }
        }
    }
    for col in 0..app.fabric.columns {
        for lane in 0..app.fabric.vert_lanes {
            for row in 0..app.fabric.rows {
                all_origins.push(app.nets.vert_in(col, row, lane));
            }
        }
    }
    for &o in &all_origins {
        let name = app.net_label(o);
        let (base, idx) = colors::bus_parts(&name);
        if let Some(i) = idx {
            let e = bus_max.entry(base.to_string()).or_insert(0);
            *e = (*e).max(i);
        }
    }
    let wire_color = |app: &App, o: NetOrigin| -> (Color32, String, bool) {
        let name = app.net_label(o);
        let dim = matches!(o, NetOrigin::FloatingLoop | NetOrigin::Const(_));
        let color = if dim {
            colors::DIM
        } else {
            let (base, idx) = colors::bus_parts(&name);
            colors::name_color(&name, idx.and(bus_max.get(base).copied()))
        };
        (color, name, dim)
    };

    let settled = app.sim.as_ref().and_then(|s| s.settled.as_ref());
    let draw_wire = |painter: &egui::Painter, a: Pos2, b: Pos2, color: Color32, value: Option<bool>, dim: bool| {
        let (a, b) = (to_screen(a), to_screen(b));
        match value {
            Some(v) if !dim => {
                let outer = (4.5 * zoom).clamp(2.0, 6.0);
                let inner = (2.2 * zoom).clamp(1.0, 3.0);
                painter.line_segment([a, b], Stroke::new(outer, color));
                if outer - inner >= 1.0 {
                    painter.line_segment([a, b], Stroke::new(inner, colors::value_fill(v)));
                }
            }
            Some(v) => {
                let w = (3.0 * zoom).clamp(1.5, 4.0);
                painter.line_segment([a, b], Stroke::new(w, colors::value_fill(v)));
            }
            None => {
                let w = if dim { (1.4 * zoom).clamp(0.8, 2.0) } else { (2.2 * zoom).clamp(1.0, 3.0) };
                painter.line_segment([a, b], Stroke::new(w, color));
            }
        }
    };

    // ------------------------------------------------------------------
    // Horizontal buses.
    for row in 0..app.fabric.rows {
        let y0 = row_y(&app.fabric, row);
        for lane in 0..app.fabric.horz_lanes {
            let y = lane_y(&app.fabric, y0, lane);
            for pos in 0..=app.fabric.columns {
                let (x_from, x_to) = if pos == 0 {
                    (MARGIN + IO_W, col_x(0))
                } else if pos < app.fabric.columns {
                    (col_x(pos - 1) + TILE_W, col_x(pos))
                } else {
                    (col_x(pos - 1) + TILE_W, right_io_x(&app.fabric))
                };
                let origin = if pos < app.fabric.columns {
                    app.nets.horz_in(pos, row, lane)
                } else {
                    app.nets.horz_edge(row, lane)
                };
                let (mut color, name, mut dim) = wire_color(app, origin);
                let mut value = settled.map(|s| {
                    if pos < app.fabric.columns {
                        s.horz_in(pos, row, lane)
                    } else {
                        s.horz_edge(row, lane)
                    }
                });
                if overlay {
                    let used = app.util.horz(row, lane, pos);
                    color = if used { colors::UTIL_USED } else { colors::UTIL_FREE };
                    dim = !used;
                    value = None;
                }
                draw_wire(&painter, Pos2::new(x_from, y), Pos2::new(x_to, y), color, value, dim);
                if seg_labels {
                    painter.text(
                        to_screen(Pos2::new((x_from + x_to) * 0.5, y - 7.0)),
                        Align2::CENTER_BOTTOM,
                        name,
                        lane_font.clone(),
                        if dim { colors::DIM } else { color },
                    );
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Vertical buses (closed rings; the stub below row 0 is the CSB tap).
    for col in 0..app.fabric.columns {
        let x0 = col_x(col);
        for lane in 0..app.fabric.vert_lanes {
            let x = vlane_x(&app.fabric, x0, lane);
            for pos in 0..app.fabric.rows {
                let origin = app.nets.vert_in(col, pos, lane);
                let (mut color, name, mut dim) = wire_color(app, origin);
                let mut value = settled.map(|s| s.vert_in(col, pos, lane));
                if overlay {
                    let used = app.util.vert(col, lane, pos);
                    color = if used { colors::UTIL_USED } else { colors::UTIL_FREE };
                    dim = !used;
                    value = None;
                }
                let (y_from, y_to) = if pos == 0 {
                    // The ring below row 0: it stops short of the CSB row,
                    // and only the tapped lane continues down to its CSB.
                    (stub_end_y(&app.fabric), row_y(&app.fabric, 0) + TILE_H)
                } else {
                    (row_y(&app.fabric, pos - 1), row_y(&app.fabric, pos) + TILE_H)
                };
                draw_wire(&painter, Pos2::new(x, y_from), Pos2::new(x, y_to), color, value, dim);
                if seg_labels && pos > 0 {
                    // Staggered per lane so neighbouring labels don't overlap.
                    let stagger = (lane as f32 - 1.5) * 12.0;
                    painter.text(
                        to_screen(Pos2::new(x + 3.0, (y_from + y_to) * 0.5 + stagger)),
                        Align2::LEFT_CENTER,
                        &name,
                        small_font.clone(),
                        if dim { colors::DIM } else { color },
                    );
                }
                if pos == 0 && seg_labels {
                    let stagger = (lane as f32 - 1.5) * 12.0;
                    painter.text(
                        to_screen(Pos2::new(x + 3.0, (y_from + y_to) * 0.5 + stagger)),
                        Align2::LEFT_CENTER,
                        name,
                        small_font.clone(),
                        if dim { colors::DIM } else { color },
                    );
                }
            }
            // Loop glyph above the top row: the bus wraps back to the tap.
            let top = row_y(&app.fabric, app.fabric.rows - 1);
            painter.line_segment(
                [to_screen(Pos2::new(x, top)), to_screen(Pos2::new(x, top - 12.0))],
                Stroke::new((1.2 * zoom).clamp(0.6, 2.0), colors::DIM),
            );
            if detail {
                painter.text(
                    to_screen(Pos2::new(x, top - 14.0)),
                    Align2::CENTER_BOTTOM,
                    "⟲",
                    small_font.clone(),
                    colors::DIM,
                );
            }
        }
    }

    // ------------------------------------------------------------------
    // Dedicated carry chain: one per column, running upward with no wrap.
    // Drawn lit only where the cell above actually selects it, since an
    // unused carry output goes nowhere.
    for col in 0..app.fabric.columns {
        let cx = col_x(col) + TILE_W - 14.0;
        for row in 0..app.fabric.rows {
            let uses_carry = app.fabric.input_muxes.iter().any(|m| {
                let sel = app.file.design.clb(col, row).slice(&m.select) as usize;
                m.sources[sel] == fpga_core::fabric::Source::CarryIn
            });
            let (y_from, y_to) = if row == 0 {
                (row_y(&app.fabric, 0) + TILE_H + 20.0, row_y(&app.fabric, 0) + TILE_H)
            } else {
                (row_y(&app.fabric, row - 1), row_y(&app.fabric, row) + TILE_H)
            };
            let value = settled.map(|s| s.clb_carry_in(col, row));
            let color = if uses_carry { colors::CARRY } else { colors::CARRY.gamma_multiply(0.35) };
            let a = to_screen(Pos2::new(cx, y_from));
            let b = to_screen(Pos2::new(cx, y_to));
            let w = (if uses_carry { 3.0 } else { 1.4 } * zoom).clamp(0.7, 4.0);
            painter.line_segment([a, b], Stroke::new(w, color));
            if let Some(v) = value {
                if uses_carry && w > 2.0 {
                    painter.line_segment([a, b], Stroke::new((w * 0.45).max(1.0), colors::value_fill(v)));
                }
            }
            // Arrowhead into the consuming cell, so the direction is clear.
            if uses_carry && zoom > 0.35 {
                let tip = to_screen(Pos2::new(cx, y_to));
                let s = 4.0 * zoom;
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        tip,
                        tip + Vec2::new(-s, s * 1.6),
                        tip + Vec2::new(s, s * 1.6),
                    ],
                    color,
                    Stroke::NONE,
                ));
            }
            if seg_labels && uses_carry {
                painter.text(
                    to_screen(Pos2::new(cx + 4.0, (y_from + y_to) * 0.5)),
                    Align2::LEFT_CENTER,
                    app.net_label(app.nets.carry_in(col, row)),
                    small_font.clone(),
                    colors::CARRY,
                );
            }
        }
    }

    // ------------------------------------------------------------------
    // Clock network: distinct dashed layer. The CSB sources the column's
    // clock: it leaves the CSB's top corner, runs up the column, and feeds
    // every CLB's clock pin. The ring lane the CSB taps is drawn explicitly.
    for col in 0..app.fabric.columns {
        let x0 = col_x(col);
        let cx = clk_x(x0);
        let csb = block_rect(&app.fabric, BlockId::Csb { col });
        let clock_value = settled.map(|s| s.clocks[col]);
        let stroke = Stroke::new(
            (1.8 * zoom).clamp(0.9, 2.6),
            match clock_value {
                Some(true) => Color32::from_rgb(0, 220, 80),
                Some(false) => colors::CLOCK.gamma_multiply(0.45),
                None => colors::CLOCK,
            },
        );
        let dash = (6.0 * zoom).max(2.0);
        let gap = (4.0 * zoom).max(1.5);
        let top_feed = row_y(&app.fabric, app.fabric.rows - 1) + TILE_H - 12.0;
        let exit_x = csb.max.x - 12.0;
        let exit_y = csb.min.y - CLOCK_EXIT_ABOVE_CSB;
        let arrow = |painter: &egui::Painter, tip: Pos2, dir: Vec2| {
            if zoom > 0.35 {
                let s = 3.5 * zoom;
                let tip = to_screen(tip);
                let back = -dir * s * 1.6;
                let side = Vec2::new(-dir.y, dir.x) * s;
                painter.add(egui::Shape::convex_polygon(
                    vec![tip, tip + back + side, tip + back - side],
                    stroke.color,
                    Stroke::NONE,
                ));
            }
        };
        // Out of the CSB's top corner, across to the column's clock line, and
        // up the column.
        painter.extend(egui::Shape::dashed_line(
            &[
                to_screen(Pos2::new(exit_x, csb.min.y)),
                to_screen(Pos2::new(exit_x, exit_y)),
                to_screen(Pos2::new(cx, exit_y)),
                to_screen(Pos2::new(cx, top_feed)),
            ],
            stroke,
            dash,
            gap,
        ));
        // Feed each CLB's clock pin (right edge, near the bottom), with an
        // arrow up the column below each feed and one into each pin.
        for row in 0..app.fabric.rows {
            let y = row_y(&app.fabric, row) + TILE_H - 12.0;
            painter.extend(egui::Shape::dashed_line(
                &[to_screen(Pos2::new(cx, y)), to_screen(Pos2::new(x0 + TILE_W, y))],
                stroke,
                dash,
                gap,
            ));
            arrow(&painter, Pos2::new(cx, y + 18.0), Vec2::new(0.0, -1.0));
            arrow(&painter, Pos2::new(x0 + TILE_W, y), Vec2::new(-1.0, 0.0));
        }
        // The CSB's input: either the explicit vertical-bus tap, or the chain.
        let coupled = app.file.design.csb(col).get(app.fabric.csb_clock.couple_field) != 0;
        if !coupled {
            let sel = app.file.design.csb(col).slice(&app.fabric.csb_clock.select) as usize;
            let lane = app.fabric.csb_clock.ring_lanes[sel];
            let stub_end = stub_end_y(&app.fabric);
            let tap_y = csb_y(&app.fabric) - TAP_ABOVE_CSB;
            let lx = vlane_x(&app.fabric, x0, lane);
            let tap_stroke = Stroke::new((2.0 * zoom).clamp(1.0, 2.8), colors::CLOCK);
            for (a, b) in [
                (Pos2::new(lx, stub_end), Pos2::new(lx, tap_y)),
                (Pos2::new(lx, tap_y), Pos2::new(csb.center().x, tap_y)),
                (Pos2::new(csb.center().x, tap_y), Pos2::new(csb.center().x, csb.min.y)),
            ] {
                painter.line_segment([to_screen(a), to_screen(b)], tap_stroke);
            }
            painter.circle_filled(to_screen(Pos2::new(lx, stub_end)), (3.0 * zoom).clamp(1.5, 4.0), colors::CLOCK);
        }
        if seg_labels {
            painter.text(
                to_screen(Pos2::new(cx + 4.0, csb.min.y - 26.0)),
                Align2::LEFT_CENTER,
                app.clock_label(col),
                small_font.clone(),
                colors::CLOCK,
            );
        }
    }

    // CSB chain links (col N takes N-1; col 0 wraps from the last).
    let link_y = csb_y(&app.fabric) + CSB_H * 0.5;
    for col in 0..app.fabric.columns {
        let coupled = app.file.design.csb(col).get(app.fabric.csb_clock.couple_field) != 0;
        let stroke = Stroke::new(
            (if coupled { 2.4 } else { 1.2 } * zoom).clamp(0.6, 3.0),
            if coupled { colors::CLOCK } else { colors::DIM },
        );
        if col > 0 {
            let a = block_rect(&app.fabric, BlockId::Csb { col: col - 1 }).right_center();
            let b = block_rect(&app.fabric, BlockId::Csb { col }).left_center();
            painter.line_segment([to_screen(a), to_screen(b)], stroke);
        } else {
            // Wrap link from the last CSB around the bottom to CSB 0.
            let last = block_rect(&app.fabric, BlockId::Csb { col: app.fabric.columns - 1 });
            let first = block_rect(&app.fabric, BlockId::Csb { col: 0 });
            let y = link_y + CSB_H * 0.5 + 16.0;
            let pts = [
                last.right_center(),
                Pos2::new(last.right_center().x + 16.0, link_y),
                Pos2::new(last.right_center().x + 16.0, y),
                Pos2::new(first.left_center().x - 16.0, y),
                Pos2::new(first.left_center().x - 16.0, link_y),
                first.left_center(),
            ];
            for w in pts.windows(2) {
                painter.line_segment([to_screen(w[0]), to_screen(w[1])], stroke);
            }
        }
    }

    // ------------------------------------------------------------------
    // Loops from the right edge back to the left: on-chip through the IO
    // controller (dashed), or board wiring from a chip output to a chip
    // input (solid).
    //
    // Routed around the outside rather than across the grid, one lane per loop,
    // and drawn with the net's own identity colour and live value like any
    // other wire — it carries a real signal. The arrowhead at the input end is
    // the direction cue, matching the carry chain.
    for (index, wire) in loop_wires(app).into_iter().enumerate() {
        let (from_row, from_lane) = wire.from;
        let (to_row, to_lane) = wire.to;
        let out_rect = io_rect(&app.fabric, false, from_row);
        let in_rect = io_rect(&app.fabric, true, to_row);
        let start = Pos2::new(out_rect.max.x - 2.0, lane_y(&app.fabric, out_rect.min.y, from_lane));
        let end = Pos2::new(in_rect.min.x - 2.0, lane_y(&app.fabric, in_rect.min.y, to_lane));
        let y = loop_lane_y(&app.fabric, index);
        let out_x = start.x + 14.0 + index as f32 * 6.0;
        let in_x = end.x - 14.0 - index as f32 * 6.0;

        // The net is the one arriving at the driving output pad.
        let origin = app.nets.horz_edge(from_row, from_lane);
        let (color, _name, dim) = wire_color(app, origin);
        let value = settled.map(|s| s.horz_edge(from_row, from_lane));

        let pts = [
            start,
            Pos2::new(out_x, start.y),
            Pos2::new(out_x, y),
            Pos2::new(in_x, y),
            Pos2::new(in_x, end.y),
            end,
        ];
        for w in pts.windows(2) {
            if wire.on_chip && value.is_none() {
                let w_px = (2.2 * zoom).clamp(1.0, 3.0);
                painter.extend(egui::Shape::dashed_line(
                    &[to_screen(w[0]), to_screen(w[1])],
                    Stroke::new(w_px, color),
                    (7.0 * zoom).max(2.0),
                    (4.0 * zoom).max(1.5),
                ));
            } else {
                draw_wire(&painter, w[0], w[1], color, value, dim);
            }
        }

        // Arrowhead into the input pad.
        if zoom > 0.35 {
            let s = 5.0;
            let tip = end;
            painter.add(egui::Shape::convex_polygon(
                vec![
                    to_screen(tip),
                    to_screen(Pos2::new(tip.x - 1.6 * s, tip.y - s)),
                    to_screen(Pos2::new(tip.x - 1.6 * s, tip.y + s)),
                ],
                color,
                Stroke::NONE,
            ));
        }
        if seg_labels {
            painter.text(
                to_screen(Pos2::new((out_x + in_x) * 0.5, y - 9.0)),
                Align2::CENTER_CENTER,
                wire.label,
                small_font.clone(),
                Color32::from_gray(150),
            );
        }
    }

    // ------------------------------------------------------------------
    // Tiles.
    let mut clicked_block: Option<BlockId> = None;
    let mut clicked_io: Option<(bool, usize)> = None;
    let pointer_world = response
        .interact_pointer_pos()
        .map(|p| ((p - avail.min - app.cam.pan) / zoom).to_pos2());

    for row in 0..app.fabric.rows {
        for col in 0..app.fabric.columns {
            let b = BlockId::Clb { col, row };
            let rect = block_rect(&app.fabric, b);
            let srect = Rect::from_min_max(to_screen(rect.min), to_screen(rect.max));
            let selected = app.selected == Some(b);
            let in_multi = app.multi.contains(&b);
            let fill = if overlay && app.util.block_counted(b, app.util_named_clbs) {
                Color32::from_rgb(78, 62, 30)
            } else {
                Color32::from_gray(40)
            };
            painter.rect_filled(srect, 4.0, fill);
            let outline = if selected {
                Stroke::new(2.5_f32, Color32::from_rgb(90, 170, 255))
            } else if in_multi {
                Stroke::new(2.0_f32, Color32::from_rgb(90, 130, 200))
            } else {
                Stroke::new(1.0_f32, Color32::from_gray(90))
            };
            painter.rect_stroke(srect, 4.0, outline);
            let name = app.block_label(b);
            let name_color = if app.is_pinned(b) { Color32::from_rgb(255, 220, 120) } else { Color32::from_gray(210) };
            painter.text(srect.center() - Vec2::new(0.0, 8.0 * zoom), Align2::CENTER_CENTER, name, block_font.clone(), name_color);
            if zoom > 0.3 {
                let lut = app.file.design.clb(col, row).get(app.fabric.lut.field);
                let label = match app.fabric.preset_name(lut) {
                    Some(name) => name.to_string(),
                    None => format!("LUT 0x{:02X}", lut),
                };
                painter.text(
                    srect.center() + Vec2::new(0.0, 12.0 * zoom),
                    Align2::CENTER_CENTER,
                    label,
                    small_font.clone(),
                    Color32::from_gray(140),
                );
            }
            if let Some(s) = settled {
                let v = s.clb_ff(col, row);
                painter.circle_filled(
                    to_screen(Pos2::new(rect.max.x - 10.0, rect.min.y + 10.0)),
                    (4.0 * zoom).clamp(2.0, 5.0),
                    colors::value_fill(v),
                );
            }
        }
    }
    for col in 0..app.fabric.columns {
        let b = BlockId::Csb { col };
        let rect = block_rect(&app.fabric, b);
        let srect = Rect::from_min_max(to_screen(rect.min), to_screen(rect.max));
        let selected = app.selected == Some(b);
        let fill = if overlay && app.util.block(b) {
            Color32::from_rgb(78, 62, 30)
        } else {
            Color32::from_gray(36)
        };
        painter.rect_filled(srect, 4.0, fill);
        painter.rect_stroke(
            srect,
            4.0,
            if selected {
                Stroke::new(2.5_f32, Color32::from_rgb(90, 170, 255))
            } else {
                Stroke::new(1.0_f32, colors::CLOCK.gamma_multiply(0.6))
            },
        );
        let name_color = if app.is_pinned(b) { Color32::from_rgb(255, 220, 120) } else { Color32::from_gray(200) };
        painter.text(srect.center(), Align2::CENTER_CENTER, app.block_label(b), small_font.clone(), name_color);
    }
    for (input_side, label) in [(true, "inputs"), (false, "outputs")] {
        for row in 0..app.fabric.rows {
            let rect = io_rect(&app.fabric, input_side, row);
            let srect = Rect::from_min_max(to_screen(rect.min), to_screen(rect.max));
            let selected = app.io_selected == Some((input_side, row));
            painter.rect_filled(srect, 4.0, Color32::from_gray(32));
            painter.rect_stroke(
                srect,
                4.0,
                if selected {
                    Stroke::new(2.5_f32, Color32::from_rgb(90, 170, 255))
                } else {
                    Stroke::new(1.0_f32, Color32::from_gray(80))
                },
            );
            painter.text(
                to_screen(Pos2::new(rect.center().x, rect.min.y + 8.0)),
                Align2::CENTER_CENTER,
                label,
                small_font.clone(),
                Color32::from_gray(140),
            );
            if zoom > 0.35 {
                for lane in 0..app.fabric.horz_lanes {
                    // Inputs: the pin (or looped-back output) the lane reads,
                    // blue where an input mux chose it. Outputs: every pin
                    // reading the lane.
                    let (name, color) = if input_side {
                        let src = app.file.design.left_source(&app.fabric, row, lane);
                        let name = app.pad_display_name(app.fabric.io_source_name(src));
                        let muxed = matches!(app.fabric.io_left[row][lane], fpga_core::fabric::LeftLane::Mux(_));
                        let color = if muxed { Color32::from_rgb(140, 190, 255) } else { Color32::from_gray(170) };
                        (name, color)
                    } else {
                        let names: Vec<String> = (0..app.fabric.io_outputs.len())
                            .filter(|&o| app.file.design.pad_lane(&app.fabric, o) == (row, lane))
                            .map(|o| app.pad_display_name(&app.fabric.io_outputs[o]))
                            .collect();
                        if names.is_empty() {
                            ("—".to_string(), colors::DIM)
                        } else {
                            (names.join(","), Color32::from_gray(170))
                        }
                    };
                    let y = lane_y(&app.fabric, rect.min.y, lane);
                    let (x, anchor) = if input_side {
                        (rect.max.x - 4.0, Align2::RIGHT_CENTER)
                    } else {
                        (rect.min.x + 4.0, Align2::LEFT_CENTER)
                    };
                    painter.text(to_screen(Pos2::new(x, y)), anchor, name, small_font.clone(), color);
                }
            }
        }
    }

    // Flash highlight from search.
    if let Some((b, until)) = app.flash {
        if now < until {
            let rect = block_rect(&app.fabric, b);
            let srect = Rect::from_min_max(to_screen(rect.min), to_screen(rect.max)).expand(6.0);
            painter.rect_stroke(srect, 6.0, Stroke::new(3.0_f32, Color32::from_rgb(255, 230, 90)));
            ui.ctx().request_repaint();
        } else {
            app.flash = None;
        }
    }

    // ------------------------------------------------------------------
    // Click handling (after painting so we know nothing else consumed it).
    if response.clicked() {
        if let Some(p) = pointer_world {
            for row in 0..app.fabric.rows {
                for col in 0..app.fabric.columns {
                    if block_rect(&app.fabric, BlockId::Clb { col, row }).contains(p) {
                        clicked_block = Some(BlockId::Clb { col, row });
                    }
                }
            }
            for col in 0..app.fabric.columns {
                if block_rect(&app.fabric, BlockId::Csb { col }).contains(p) {
                    clicked_block = Some(BlockId::Csb { col });
                }
            }
            for input_side in [true, false] {
                for row in 0..app.fabric.rows {
                    if io_rect(&app.fabric, input_side, row).contains(p) {
                        clicked_io = Some((input_side, row));
                    }
                }
            }
            let ctrl = ui.input(|i| i.modifiers.command);
            match (clicked_block, clicked_io) {
                (Some(b), _) if ctrl => {
                    if let Some(i) = app.multi.iter().position(|x| *x == b) {
                        app.multi.remove(i);
                    } else {
                        app.multi.push(b);
                    }
                }
                (Some(b), _) => app.select(Some(b)),
                (None, Some(io)) => {
                    app.select(None);
                    app.io_selected = Some(io);
                }
                (None, None) => app.select(None),
            }
        }
    }
}
