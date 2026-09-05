//! Wire identity colors: each distinct net name hashes to a stable hue, and
//! bracket-indexed names (`data[3]`) share their base name's hue with
//! brightness scaled across the bus's actual index range, MSB brightest.

use eframe::egui::Color32;

/// Split `data[3]` into ("data", Some(3)); plain names have no index.
pub fn bus_parts(name: &str) -> (&str, Option<usize>) {
    if let Some(open) = name.rfind('[') {
        if let Some(idx) = name[open + 1..].strip_suffix(']').and_then(|s| s.parse().ok()) {
            return (&name[..open], Some(idx));
        }
    }
    (name, None)
}

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn hsv(h: f32, s: f32, v: f32) -> Color32 {
    let h = h.rem_euclid(360.0) / 60.0;
    let i = h.floor();
    let f = h - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    let (r, g, b) = match i as i32 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

/// The identity color for a net name. `bus_max` is the highest index seen on
/// the same base name this frame (for brightness scaling); pass `None` for
/// non-bus names. Saturation and the brightness floor are chosen so the color
/// reads as a thin edge against both the black and green value fills.
pub fn name_color(name: &str, bus_max: Option<usize>) -> Color32 {
    let (base, idx) = bus_parts(name);
    let hue = (fnv1a(base) % 3600) as f32 / 10.0;
    match (idx, bus_max) {
        (Some(i), Some(max)) if max > 0 => {
            let t = (i.min(max)) as f32 / max as f32; // LSB dark .. MSB bright
            hsv(hue, 0.75, 0.5 + 0.5 * t)
        }
        _ => hsv(hue, 0.72, 0.88),
    }
}

/// Fill colors for simulated values (the "pipe" interior).
pub fn value_fill(v: bool) -> Color32 {
    if v {
        Color32::from_rgb(0, 200, 60)
    } else {
        Color32::from_rgb(12, 12, 12)
    }
}

pub const DIM: Color32 = Color32::from_gray(95);
pub const CLOCK: Color32 = Color32::from_rgb(255, 160, 40);
pub const CARRY: Color32 = Color32::from_rgb(190, 130, 255);
