//! The app icon (the App UI Substrate › The window): a generic one, drawn here in code from the
//! theme's own palette, with no third-party artwork. A dark rounded tile holds a list of three
//! commands, a bullet and a bar each in the series' first colours: what the skeleton's window shows.
//! An app replaces it with its own design, drawn the same way.
//!
//! The window uses [`icon_data`] at run time. An app that wants a favicon or a Windows `.ico`
//! generates them from [`rgba`] and checks them in, with a test that they still match.

use std::sync::Arc;

use {{crate_prefix_ident}}engine::theme::{self as palette, Look, Rgb};

/// The window icon's size; the system scales it for the title bar and the task bar.
pub const WINDOW_SIZE: u32 = 256;
/// The largest icon drawn; a bigger request is clamped.
pub const MAX_SIZE: u32 = 1024;

/// Geometry in unit coordinates (0..1, y down).
const TILE_INSET: f64 = 0.03;
const TILE_RADIUS: f64 = 0.2;
/// Each row: its centre line and where its bar ends, and the series entry it is drawn in.
const ROWS: [(f64, f64, u64); 3] = [(0.33, 0.80, 0), (0.50, 0.66, 1), (0.67, 0.74, 2)];
const BULLET_X: f64 = 0.23;
const BULLET_RADIUS: f64 = 0.055;
const BAR_START: f64 = 0.35;
const BAR_HEIGHT: f64 = 0.11;

/// A colour of the dark look, as 0..1 channels.
fn channels(rgb: Rgb) -> [f64; 3] {
    [rgb.r, rgb.g, rgb.b].map(|c| f64::from(c) / 255.0)
}

/// The colour of one point, or `None` outside the tile.
fn shade(x: f64, y: f64) -> Option<[f64; 3]> {
    if !in_rounded_square(x, y) {
        return None;
    }
    if let Some(rgb) = row(x, y) {
        return Some(channels(rgb));
    }
    let (top, bottom) = (channels(palette::HEADER.of(Look::Dark)), channels(palette::BASE.of(Look::Dark)));
    let t = ((y - TILE_INSET) / (1.0 - 2.0 * TILE_INSET)).clamp(0.0, 1.0);
    Some([0, 1, 2].map(|i| top[i] + (bottom[i] - top[i]) * t))
}

fn in_rounded_square(x: f64, y: f64) -> bool {
    let half = 0.5 - TILE_INSET;
    let qx = (x - 0.5).abs() - (half - TILE_RADIUS);
    let qy = (y - 0.5).abs() - (half - TILE_RADIUS);
    // `sqrt`, not `hypot`: IEEE rounds it exactly everywhere, so every platform draws the same bytes.
    let (ox, oy) = (qx.max(0.0), qy.max(0.0));
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) <= TILE_RADIUS
}

/// The colour of the row under a point, if any: its bullet in `text`, its bar (a capsule) in its
/// series colour.
fn row(x: f64, y: f64) -> Option<Rgb> {
    let r = BAR_HEIGHT / 2.0;
    ROWS.iter().find_map(|&(cy, end, entry)| {
        let (bx, by) = (x - BULLET_X, y - cy);
        if bx * bx + by * by <= BULLET_RADIUS * BULLET_RADIUS {
            return Some(palette::TEXT.of(Look::Dark));
        }
        let dx = x - x.max(BAR_START + r).min(end - r);
        (dx * dx + by * by <= r * r).then(|| palette::SERIES.of(Look::Dark).at(entry))
    })
}

/// The icon as straight-alpha RGBA8, `size` × `size` (clamped to 1..=[`MAX_SIZE`]), each pixel
/// supersampled so the edges are smooth.
pub fn rgba(size: u32) -> Vec<u8> {
    let size = size.clamp(1, MAX_SIZE);
    let samples: u32 = if size <= 64 { 8 } else { 4 };
    let scale = f64::from(size);
    let mut out = Vec::with_capacity((size as usize).saturating_mul(size as usize).saturating_mul(4));
    for py in 0..size {
        for px in 0..size {
            out.extend(pixel(px, py, samples, scale));
        }
    }
    out
}

/// Pixel (`px`, `py`) of an icon `scale` pixels wide, from `samples` × `samples` samples: the mean
/// colour of the samples inside the shape, and its coverage as alpha.
fn pixel(px: u32, py: u32, samples: u32, scale: f64) -> [u8; 4] {
    let n = f64::from(samples * samples);
    let (mut acc, mut alpha) = ([0.0f64; 3], 0.0f64);
    for sy in 0..samples {
        for sx in 0..samples {
            let x = (f64::from(px) + (f64::from(sx) + 0.5) / f64::from(samples)) / scale;
            let y = (f64::from(py) + (f64::from(sy) + 0.5) / f64::from(samples)) / scale;
            if let Some(c) = shade(x, y) {
                for (a, v) in acc.iter_mut().zip(c) {
                    *a += v;
                }
                alpha += 1.0;
            }
        }
    }
    // In 0..=255 once clamped: the cast neither wraps nor truncates a whole part.
    let byte = |v: f64| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    let [r, g, b] = if alpha > 0.0 { acc.map(|v| v / alpha) } else { [0.0; 3] }.map(byte);
    [r, g, b, byte(alpha / n)]
}

/// The window icon, for `ViewportBuilder::with_icon` (the title bar and the task bar).
pub fn icon_data() -> Arc<egui::IconData> {
    Arc::new(egui::IconData { rgba: rgba(WINDOW_SIZE), width: WINDOW_SIZE, height: WINDOW_SIZE })
}

#[cfg(test)]
mod tests;
