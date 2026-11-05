//! Stamp based brush engine that paints into an object's paint layer.

use crate::doc::Rgba;
use crate::textures::{smoothstep, value_noise};
use tiny_skia::Pixmap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrushKind {
    Round,
    Airbrush,
    Chalk,
    Smudge,
    Eraser,
}

impl BrushKind {
    pub const ALL: [BrushKind; 5] = [
        BrushKind::Round,
        BrushKind::Airbrush,
        BrushKind::Chalk,
        BrushKind::Smudge,
        BrushKind::Eraser,
    ];
    pub fn label(self) -> &'static str {
        match self {
            BrushKind::Round => "Round",
            BrushKind::Airbrush => "Airbrush",
            BrushKind::Chalk => "Chalk",
            BrushKind::Smudge => "Smudge",
            BrushKind::Eraser => "Eraser",
        }
    }
}

#[derive(Clone, Debug)]
pub struct BrushSettings {
    pub kind: BrushKind,
    /// Diameter in canvas pixels.
    pub size: f32,
    pub hardness: f32,
    pub flow: f32,
    /// Stamp spacing as a fraction of the diameter.
    pub spacing: f32,
    pub color: Rgba,
    pub pressure: bool,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self {
            kind: BrushKind::Round,
            size: 24.0,
            hardness: 0.6,
            flow: 0.8,
            spacing: 0.12,
            color: [200, 40, 40, 255],
            pressure: true,
        }
    }
}

pub struct StrokeState {
    carry: f32,
    last_stamp: Option<(f32, f32)>,
}

impl StrokeState {
    pub fn new() -> Self {
        Self {
            carry: 0.0,
            last_stamp: None,
        }
    }
}

/// Paint a straight segment a -> b (paint-layer pixel coordinates).
pub fn segment(
    pm: &mut Pixmap,
    a: (f32, f32),
    b: (f32, f32),
    radius: f32,
    s: &BrushSettings,
    st: &mut StrokeState,
) {
    let spacing = (radius * 2.0 * s.spacing).max(0.5);
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let dist = (dx * dx + dy * dy).sqrt();
    let mut t = if st.last_stamp.is_none() { 0.0 } else { st.carry };
    while t <= dist {
        let f = if dist > 0.0 { t / dist } else { 0.0 };
        let p = (a.0 + dx * f, a.1 + dy * f);
        stamp(pm, p, radius, s, st.last_stamp);
        st.last_stamp = Some(p);
        t += spacing;
    }
    st.carry = t - dist;
}

fn falloff(d: f32, hardness: f32, r: f32) -> f32 {
    let aa = ((1.0 - d) * r).clamp(0.0, 1.0);
    if hardness >= 0.999 {
        return aa;
    }
    let soft = if d <= hardness {
        1.0
    } else {
        1.0 - smoothstep(0.0, 1.0, (d - hardness) / (1.0 - hardness))
    };
    soft * aa
}

fn stamp(pm: &mut Pixmap, c: (f32, f32), r: f32, s: &BrushSettings, prev: Option<(f32, f32)>) {
    let r = r.max(0.5);
    let w = pm.width() as i32;
    let h = pm.height() as i32;
    let x0 = ((c.0 - r - 1.0).floor() as i32).max(0);
    let x1 = ((c.0 + r + 1.0).ceil() as i32).min(w - 1);
    let y0 = ((c.1 - r - 1.0).floor() as i32).max(0);
    let y1 = ((c.1 + r + 1.0).ceil() as i32).min(h - 1);
    if x0 > x1 || y0 > y1 {
        return;
    }

    // Smudge: copy the area around the previous stamp and drag it along.
    let mut smudge: Option<(i32, i32, i32, i32, Vec<[f32; 4]>, (f32, f32))> = None;
    if s.kind == BrushKind::Smudge {
        let Some(p) = prev else { return };
        let delta = (c.0 - p.0, c.1 - p.1);
        let sx0 = ((p.0 - r - 2.0).floor() as i32).max(0);
        let sx1 = ((p.0 + r + 2.0).ceil() as i32).min(w - 1);
        let sy0 = ((p.1 - r - 2.0).floor() as i32).max(0);
        let sy1 = ((p.1 + r + 2.0).ceil() as i32).min(h - 1);
        if sx0 > sx1 || sy0 > sy1 {
            return;
        }
        let data = pm.data();
        let mut buf = Vec::with_capacity(((sx1 - sx0 + 1) * (sy1 - sy0 + 1)) as usize);
        for y in sy0..=sy1 {
            for x in sx0..=sx1 {
                let i = ((y * w + x) * 4) as usize;
                buf.push([
                    data[i] as f32,
                    data[i + 1] as f32,
                    data[i + 2] as f32,
                    data[i + 3] as f32,
                ]);
            }
        }
        smudge = Some((sx0, sy0, sx1, sy1, buf, delta));
    }

    let col = [
        s.color[0] as f32 / 255.0,
        s.color[1] as f32 / 255.0,
        s.color[2] as f32 / 255.0,
    ];
    let ca = s.color[3] as f32 / 255.0;
    let data = pm.data_mut();
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f32 + 0.5 - c.0;
            let dy = y as f32 + 0.5 - c.1;
            let d = (dx * dx + dy * dy).sqrt() / r;
            if d >= 1.0 {
                continue;
            }
            let mut a = match s.kind {
                BrushKind::Airbrush => (1.0 - d * d).powi(2) * s.flow * 0.3,
                _ => falloff(d, s.hardness, r) * s.flow,
            };
            if s.kind == BrushKind::Chalk {
                let g = value_noise(x as f32 * 0.55, y as f32 * 0.55, 77) * 0.6
                    + value_noise(x as f32 * 0.17, y as f32 * 0.17, 78) * 0.4;
                a *= smoothstep(0.38, 0.62, g);
            }
            if a <= 0.0 {
                continue;
            }
            let i = ((y * w + x) * 4) as usize;
            match s.kind {
                BrushKind::Eraser => {
                    for k in 0..4 {
                        data[i + k] = (data[i + k] as f32 * (1.0 - a)).round() as u8;
                    }
                }
                BrushKind::Smudge => {
                    if let Some((sx0, sy0, sx1, sy1, buf, delta)) = &smudge {
                        let sx = (x as f32 - delta.0).round() as i32;
                        let sy = (y as f32 - delta.1).round() as i32;
                        if sx < *sx0 || sy < *sy0 || sx > *sx1 || sy > *sy1 {
                            continue;
                        }
                        let src = buf[((sy - sy0) * (sx1 - sx0 + 1) + (sx - sx0)) as usize];
                        let k = (a * 0.9).min(1.0);
                        for ch in 0..4 {
                            data[i + ch] =
                                (data[i + ch] as f32 + (src[ch] - data[i + ch] as f32) * k).round() as u8;
                        }
                    }
                }
                _ => {
                    let sa = a * ca;
                    let inv = 1.0 - sa;
                    let na = (255.0 * sa + data[i + 3] as f32 * inv).round().min(255.0);
                    for k in 0..3 {
                        let v = col[k] * 255.0 * sa + data[i + k] as f32 * inv;
                        data[i + k] = v.round().min(na) as u8;
                    }
                    data[i + 3] = na as u8;
                }
            }
        }
    }
}
