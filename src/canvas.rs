//! Canvas sizes: templates, aspect ratios and the visual extent of the content.

use crate::doc::{Document, Object};
use crate::geom::Pt;
use serde::{Deserialize, Serialize};

pub struct CanvasPreset {
    pub group: &'static str,
    pub name: &'static str,
    pub w: u32,
    pub h: u32,
}

const fn p(group: &'static str, name: &'static str, w: u32, h: u32) -> CanvasPreset {
    CanvasPreset { group, name, w, h }
}

/// Common canvas templates. Print sizes are at 300 dpi.
pub const PRESETS: &[CanvasPreset] = &[
    p("Screen", "Full HD 16:9", 1920, 1080),
    p("Screen", "HD 16:9", 1280, 720),
    p("Screen", "QHD 16:9", 2560, 1440),
    p("Screen", "4K UHD 16:9", 3840, 2160),
    p("Screen", "Ultrawide 21:9", 2560, 1080),
    p("Screen", "Classic 4:3", 1600, 1200),
    p("Screen", "Widescreen 16:10", 1920, 1200),
    p("Social & web", "Square post 1:1", 1080, 1080),
    p("Social & web", "Large square 1:1", 2048, 2048),
    p("Social & web", "Portrait post 4:5", 1080, 1350),
    p("Social & web", "Story / reel 9:16", 1080, 1920),
    p("Social & web", "Video thumbnail 16:9", 1280, 720),
    p("Social & web", "Link preview 1.91:1", 1200, 630),
    p("Social & web", "Header banner 3:1", 1500, 500),
    p("Social & web", "Icon 1:1", 512, 512),
    p("Photo", "Photo 3:2", 1800, 1200),
    p("Photo", "Photo 4:3", 2000, 1500),
    p("Photo", "Photo 2:3 portrait", 1200, 1800),
    p("Photo", "Panorama 3:1", 3000, 1000),
    p("Print 300 dpi", "A4 portrait", 2480, 3508),
    p("Print 300 dpi", "A4 landscape", 3508, 2480),
    p("Print 300 dpi", "A5 portrait", 1748, 2480),
    p("Print 300 dpi", "A3 portrait", 3508, 4961),
    p("Print 300 dpi", "US Letter portrait", 2550, 3300),
    p("Print 300 dpi", "US Letter landscape", 3300, 2550),
    p("Print 300 dpi", "Postcard 6×4 in", 1800, 1200),
    p("Print 300 dpi", "Business card 3.5×2 in", 1050, 600),
];

pub const MAX_SIZE: u32 = 8192;
pub const MIN_SIZE: u32 = 16;

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// "16:9", "4:5", "1:1"… or a decimal ratio like "1.91:1" when the numbers get unwieldy.
pub fn aspect_label(w: u32, h: u32) -> String {
    if w == 0 || h == 0 {
        return "—".into();
    }
    let g = gcd(w, h);
    let (a, b) = (w / g, h / g);
    if a <= 32 && b <= 32 {
        return format!("{a}:{b}");
    }
    let known = [
        (16.0, 9.0),
        (16.0, 10.0),
        (21.0, 9.0),
        (4.0, 3.0),
        (3.0, 2.0),
        (5.0, 4.0),
        (9.0, 16.0),
        (3.0, 4.0),
        (2.0, 3.0),
    ];
    let r = w as f32 / h as f32;
    for (x, y) in known {
        // marketing ratios are approximate (2560×1080 is sold as 21:9)
        if (r / (x / y) - 1.0).abs() < 0.02 {
            return format!("{x}:{y}");
        }
    }
    if r >= 1.0 {
        format!("{:.2}:1", r)
    } else {
        format!("1:{:.2}", 1.0 / r)
    }
}

/// Everything an object puts on the canvas: its outline plus shadow, glow and outer stroke.
pub fn visual_bounds(o: &Object) -> (Pt, Pt) {
    let (mut lo, mut hi) = o.bounds();
    let fx = &o.effects;
    let mut grow = o.stroke.outer_extent();
    if fx.glow.enabled {
        grow = grow.max(fx.glow.size * 1.2);
    }
    lo = lo - Pt::new(grow, grow);
    hi = hi + Pt::new(grow, grow);
    if fx.shadow.enabled {
        let b = fx.shadow.blur * 1.2;
        lo = Pt::new(lo.x.min(lo.x + fx.shadow.dx - b), lo.y.min(lo.y + fx.shadow.dy - b));
        hi = Pt::new(hi.x.max(hi.x + fx.shadow.dx + b), hi.y.max(hi.y + fx.shadow.dy + b));
    }
    (lo, hi)
}

/// Bounds of all visible objects (None for an empty document).
pub fn content_bounds(doc: &Document) -> Option<(Pt, Pt)> {
    doc.objects
        .iter()
        .filter(|o| o.visible)
        .map(visual_bounds)
        .reduce(|(a, b), (c, d)| (Pt::new(a.x.min(c.x), a.y.min(c.y)), Pt::new(b.x.max(d.x), b.y.max(d.y))))
}

/// How the canvas must change so it contains `lo..hi` (plus `margin`): new size and the
/// offset to move all objects by (positive when the canvas grows to the left / top).
/// None if everything already fits.
pub fn grow_to_fit(w: u32, h: u32, lo: Pt, hi: Pt, margin: f32) -> Option<(u32, u32, Pt)> {
    let left = (margin - lo.x).max(0.0).ceil();
    let top = (margin - lo.y).max(0.0).ceil();
    let right = (hi.x + margin - w as f32).max(0.0).ceil();
    let bottom = (hi.y + margin - h as f32).max(0.0).ceil();
    if left + top + right + bottom <= 0.0 {
        return None;
    }
    let nw = ((w as f32 + left + right) as u32).clamp(MIN_SIZE, MAX_SIZE);
    let nh = ((h as f32 + top + bottom) as u32).clamp(MIN_SIZE, MAX_SIZE);
    Some((nw, nh, Pt::new(left, top)))
}

/// Per-user preferences, remembered between sessions.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Extend the canvas when an object is dropped past its edge.
    pub auto_grow: bool,
}

fn settings_path() -> Option<std::path::PathBuf> {
    let base = std::env::var("APPDATA").or_else(|_| std::env::var("HOME")).ok()?;
    Some(std::path::PathBuf::from(base).join("JabroDraw").join("settings.json"))
}

impl Settings {
    pub fn load() -> Settings {
        settings_path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(p) = settings_path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(json) = serde_json::to_vec_pretty(self) {
                let _ = std::fs::write(p, json);
            }
        }
    }
}
