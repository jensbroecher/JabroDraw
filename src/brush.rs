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
    /// Wet-edged washes that settle into the paper.
    Watercolor,
    /// Image hose: sprays leaves, grass, stars, hearts, flowers or dots.
    Scatter,
    /// Paints the object's transparency mask: hides what is under the brush.
    Hide,
    /// Paints the mask back: reveals hidden parts again.
    Reveal,
    /// Click to hide a connected area of similar colour (pictures / paint layers).
    MagicWand,
    /// Click an object in a picture: AI finds its outline (keep it, hide it or copy it out).
    AiSelect,
}

impl BrushKind {
    pub const ALL: [BrushKind; 11] = [
        BrushKind::Round,
        BrushKind::Airbrush,
        BrushKind::Chalk,
        BrushKind::Smudge,
        BrushKind::Eraser,
        BrushKind::Watercolor,
        BrushKind::Scatter,
        BrushKind::Hide,
        BrushKind::Reveal,
        BrushKind::MagicWand,
        BrushKind::AiSelect,
    ];
    pub fn label(self) -> &'static str {
        match self {
            BrushKind::Round => "Round",
            BrushKind::Airbrush => "Airbrush",
            BrushKind::Chalk => "Chalk",
            BrushKind::Smudge => "Smudge",
            BrushKind::Eraser => "Eraser (deletes paint)",
            BrushKind::Watercolor => "Wet watercolor",
            BrushKind::Scatter => "Scatter (leaves, stars…)",
            BrushKind::Hide => "Transparency: hide",
            BrushKind::Reveal => "Transparency: reveal",
            BrushKind::MagicWand => "Magic wand (click to hide an area)",
            BrushKind::AiSelect => "AI object select (click an object)",
        }
    }
    /// Works on the transparency mask instead of the paint.
    pub fn is_mask(self) -> bool {
        matches!(self, BrushKind::Hide | BrushKind::Reveal)
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
    /// Scatter brush: what it sprays and how much the colour varies (0..1).
    pub scatter: ScatterShape,
    pub jitter: f32,
    /// Paint mirrored / turned copies of every stroke.
    pub symmetry: Symmetry,
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
            scatter: ScatterShape::Leaves,
            jitter: 0.6,
            symmetry: Symmetry::Off,
        }
    }
}

/// What the scatter brush sprays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScatterShape {
    Leaves,
    Grass,
    Stars,
    Hearts,
    Flowers,
    Dots,
}

impl ScatterShape {
    pub const ALL: [ScatterShape; 6] = [
        ScatterShape::Leaves,
        ScatterShape::Grass,
        ScatterShape::Stars,
        ScatterShape::Hearts,
        ScatterShape::Flowers,
        ScatterShape::Dots,
    ];
    pub fn label(self) -> &'static str {
        match self {
            ScatterShape::Leaves => "Leaves",
            ScatterShape::Grass => "Grass",
            ScatterShape::Stars => "Stars",
            ScatterShape::Hearts => "Hearts",
            ScatterShape::Flowers => "Flowers",
            ScatterShape::Dots => "Confetti dots",
        }
    }
}

/// A watercolor stroke in progress: the layer as it was, plus how wet each pixel is.
struct Wet {
    base: Vec<u8>,
    mask: Vec<f32>,
    lo: (i32, i32),
    hi: (i32, i32),
}

pub struct StrokeState {
    carry: f32,
    last_stamp: Option<(f32, f32)>,
    seed: u32,
    wet: Option<Wet>,
}

impl StrokeState {
    pub fn new() -> Self {
        Self {
            carry: 0.0,
            last_stamp: None,
            seed: 0x9e37_79b9,
            wet: None,
        }
    }

    fn rand(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        (self.seed >> 8) as f32 / 16_777_216.0
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
    let spacing_k = match s.kind {
        BrushKind::Scatter => s.spacing.max(0.45),
        _ => s.spacing,
    };
    let spacing = (radius * 2.0 * spacing_k).max(0.5);
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let dist = (dx * dx + dy * dy).sqrt();
    let dir = dy.atan2(dx);
    let mut t = if st.last_stamp.is_none() { 0.0 } else { st.carry };
    while t <= dist {
        let f = if dist > 0.0 { t / dist } else { 0.0 };
        let p = (a.0 + dx * f, a.1 + dy * f);
        match s.kind {
            BrushKind::Scatter => scatter_stamp(pm, p, radius, s, st, dir),
            BrushKind::Watercolor => wet_stamp(pm, p, radius, s, st),
            _ => stamp(pm, p, radius, s, st.last_stamp),
        }
        st.last_stamp = Some(p);
        t += spacing;
    }
    st.carry = t - dist;
    if s.kind == BrushKind::Watercolor {
        render_wet(pm, s, st, radius);
    }
}

fn shape_path(shape: ScatterShape) -> Option<tiny_skia::Path> {
    use tiny_skia::PathBuilder;
    let mut pb = PathBuilder::new();
    match shape {
        ScatterShape::Leaves => {
            pb.move_to(-1.0, 0.0);
            pb.quad_to(0.0, -0.62, 1.0, 0.0);
            pb.quad_to(0.0, 0.62, -1.0, 0.0);
            pb.close();
        }
        ScatterShape::Grass => {
            // a blade growing from the stamp point (pointing along +x before rotation)
            pb.move_to(0.0, -0.13);
            pb.quad_to(1.2, -0.22, 2.3, 0.08);
            pb.quad_to(1.1, 0.06, 0.0, 0.13);
            pb.close();
        }
        ScatterShape::Stars => {
            for i in 0..10 {
                let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
                let r = if i % 2 == 0 { 1.0 } else { 0.45 };
                if i == 0 {
                    pb.move_to(r * a.cos(), r * a.sin());
                } else {
                    pb.line_to(r * a.cos(), r * a.sin());
                }
            }
            pb.close();
        }
        ScatterShape::Hearts => {
            pb.move_to(0.0, 0.9);
            pb.cubic_to(-1.2, 0.1, -0.9, -0.9, 0.0, -0.35);
            pb.cubic_to(0.9, -0.9, 1.2, 0.1, 0.0, 0.9);
            pb.close();
        }
        ScatterShape::Flowers => {
            for i in 0..5 {
                let a = i as f32 * std::f32::consts::TAU / 5.0;
                pb.push_circle(0.55 * a.cos(), 0.55 * a.sin(), 0.42);
            }
        }
        ScatterShape::Dots => pb.push_circle(0.0, 0.0, 0.7),
    }
    pb.finish()
}

/// Image hose: sprays little shapes with random size, angle, offset and colour variation.
fn scatter_stamp(pm: &mut Pixmap, c: (f32, f32), r: f32, s: &BrushSettings, st: &mut StrokeState, dir: f32) {
    let scale = r * (0.5 + 0.6 * st.rand());
    let side = (st.rand() - 0.5) * r * 1.1;
    let (px, py) = (c.0 - dir.sin() * side, c.1 + dir.cos() * side);
    let rot = match s.scatter {
        ScatterShape::Grass => -std::f32::consts::FRAC_PI_2 + (st.rand() - 0.5) * 0.8,
        ScatterShape::Leaves => dir + (st.rand() - 0.5) * 2.2,
        _ => st.rand() * std::f32::consts::TAU,
    };
    // colour variation around the brush colour
    let j = s.jitter;
    let col = s.color.map(|v| v as f32 / 255.0);
    let (h, sat, l) = crate::effects::rgb_to_hsl(col[0], col[1], col[2]);
    let hue = h + (st.rand() - 0.5) * 70.0 * j;
    let light = (l + (st.rand() - 0.5) * 0.35 * j).clamp(0.05, 0.95);
    let (r_, g_, b_) = crate::effects::hsl_to_rgb(hue, sat, light);
    let Some(path) = shape_path(s.scatter) else { return };
    let ts = tiny_skia::Transform::from_translate(px, py)
        .pre_concat(tiny_skia::Transform::from_rotate(rot.to_degrees()))
        .pre_scale(scale, scale);
    let mut paint = tiny_skia::Paint::default();
    let alpha = (s.flow * col[3]).clamp(0.0, 1.0);
    paint.set_color(tiny_skia::Color::from_rgba(r_, g_, b_, alpha).unwrap_or(tiny_skia::Color::BLACK));
    pm.fill_path(&path, &paint, tiny_skia::FillRule::Winding, ts, None);
    if s.scatter == ScatterShape::Leaves {
        // a darker midrib
        let mut pb = tiny_skia::PathBuilder::new();
        pb.move_to(-0.85, 0.0);
        pb.line_to(0.85, 0.0);
        if let Some(rib) = pb.finish() {
            let (r2, g2, b2) = crate::effects::hsl_to_rgb(hue, sat, (light * 0.6).max(0.02));
            paint.set_color(tiny_skia::Color::from_rgba(r2, g2, b2, alpha).unwrap_or(tiny_skia::Color::BLACK));
            let stroke = tiny_skia::Stroke { width: 0.07, ..Default::default() };
            pm.stroke_path(&rib, &paint, &stroke, ts, None);
        }
    }
}

/// Watercolor: only records how wet the paper is; `render_wet` turns that into pigment.
fn wet_stamp(pm: &Pixmap, c: (f32, f32), r: f32, s: &BrushSettings, st: &mut StrokeState) {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let wet = st.wet.get_or_insert_with(|| Wet {
        base: pm.data().to_vec(),
        mask: vec![0.0; (w * h) as usize],
        lo: (i32::MAX, i32::MAX),
        hi: (i32::MIN, i32::MIN),
    });
    let r = r.max(1.0);
    let (x0, x1) = (((c.0 - r).floor() as i32).max(0), ((c.0 + r).ceil() as i32).min(w - 1));
    let (y0, y1) = (((c.1 - r).floor() as i32).max(0), ((c.1 + r).ceil() as i32).min(h - 1));
    for y in y0..=y1 {
        for x in x0..=x1 {
            let d = ((x as f32 + 0.5 - c.0).powi(2) + (y as f32 + 0.5 - c.1).powi(2)).sqrt() / r;
            if d >= 1.0 {
                continue;
            }
            // ragged, irregular wet edge: the reach of the water varies along the stroke
            let wobble = value_noise(x as f32 * 0.09, y as f32 * 0.09, 41) * 0.6
                + value_noise(x as f32 * 0.3, y as f32 * 0.3, 43) * 0.4;
            let edge_at = 1.0 - 0.22 * wobble;
            let v = (1.0 - smoothstep(0.78 * edge_at, edge_at, d)) * s.flow;
            let i = (y * w + x) as usize;
            wet.mask[i] = wet.mask[i].max(v);
        }
    }
    wet.lo = (wet.lo.0.min(x0), wet.lo.1.min(y0));
    wet.hi = (wet.hi.0.max(x1), wet.hi.1.max(y1));
}

/// Pigment pools at the edges of the wet area and settles into the paper grain.
/// Only the part touched since the last call is recomputed, so long strokes stay fast.
fn render_wet(pm: &mut Pixmap, s: &BrushSettings, st: &mut StrokeState, r: f32) {
    let Some(wet) = st.wet.as_mut() else { return };
    if wet.hi.0 < wet.lo.0 {
        return;
    }
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let sigma = r * 0.22 + 1.0;
    let reach = (sigma * 3.0).ceil() as i32 + 1;
    // pixels whose result can change, and the larger area the blur has to read for them
    let (ox0, oy0) = ((wet.lo.0 - reach).max(0), (wet.lo.1 - reach).max(0));
    let (ox1, oy1) = ((wet.hi.0 + reach).min(w - 1), (wet.hi.1 + reach).min(h - 1));
    let (x0, y0) = ((ox0 - reach).max(0), (oy0 - reach).max(0));
    let (x1, y1) = ((ox1 + reach).min(w - 1), (oy1 + reach).min(h - 1));
    wet.lo = (i32::MAX, i32::MAX);
    wet.hi = (i32::MIN, i32::MIN);
    let (rw, rh) = ((x1 - x0 + 1) as usize, (y1 - y0 + 1) as usize);
    let mut region = vec![0f32; rw * rh];
    for y in 0..rh {
        let row = (y0 as usize + y) * w as usize + x0 as usize;
        region[y * rw..(y + 1) * rw].copy_from_slice(&wet.mask[row..row + rw]);
    }
    let mut soft = region.clone();
    crate::raster::gaussian(&mut soft, rw, rh, sigma, false);
    let col = s.color.map(|v| v as f32 / 255.0);
    let data = pm.data_mut();
    for gy in oy0..=oy1 {
        for gx in ox0..=ox1 {
            let (x, y) = ((gx - x0) as usize, (gy - y0) as usize);
            let c = region[y * rw + x];
            let i = (gy as usize * w as usize + gx as usize) * 4;
            if c <= 0.0 {
                data[i..i + 4].copy_from_slice(&wet.base[i..i + 4]);
                continue;
            }
            let edge = (c - soft[y * rw + x]).max(0.0);
            // pigment granulation: soft blotches plus a little fine tooth
            let grain = value_noise(gx as f32 * 0.11, gy as f32 * 0.11, 97) * 0.65
                + value_noise(gx as f32 * 0.45, gy as f32 * 0.45, 98) * 0.35
                - 0.5;
            let d = ((0.32 * c + 1.8 * edge) * (1.0 + 0.7 * grain)).clamp(0.0, 0.95) * col[3];
            let base = &wet.base[i..i + 4];
            let ba = base[3] as f32 / 255.0;
            let a = (d * 255.0 + base[3] as f32 * (1.0 - d)).round().min(255.0);
            for k in 0..3 {
                // glazing: a wash over earlier paint filters it like pigment does (multiply),
                // over bare paper it is just its own colour
                let under = if ba > 0.0 { base[k] as f32 / base[3] as f32 } else { 1.0 };
                let tint = col[k] * (1.0 - ba + ba * under);
                data[i + k] = (tint * 255.0 * d + base[k] as f32 * (1.0 - d)).round().min(a) as u8;
            }
            data[i + 3] = a as u8;
        }
    }
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

/// Paint several copies of every stroke at once, mirrored or turned around the layer's centre.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Symmetry {
    #[default]
    Off,
    /// Left and right mirror each other.
    Mirror,
    /// Mirrored left / right and top / bottom.
    Quad,
    /// The stroke turned `n` times around the centre.
    Radial(u8),
    /// Turned `n` times, each copy also mirrored (a kaleidoscope / mandala).
    Kaleido(u8),
}

impl Symmetry {
    pub fn label(self) -> String {
        match self {
            Symmetry::Off => "Off".into(),
            Symmetry::Mirror => "Mirror".into(),
            Symmetry::Quad => "4-way mirror".into(),
            Symmetry::Radial(n) => format!("Radial × {n}"),
            Symmetry::Kaleido(n) => format!("Kaleidoscope × {n}"),
        }
    }

    pub fn copies(self) -> usize {
        match self {
            Symmetry::Off => 1,
            Symmetry::Mirror => 2,
            Symmetry::Quad => 4,
            Symmetry::Radial(n) => n.max(2) as usize,
            Symmetry::Kaleido(n) => 2 * n.max(2) as usize,
        }
    }

    /// Every copy of point `p` (the first is `p` itself), around centre `c`.
    pub fn points(self, p: (f32, f32), c: (f32, f32)) -> Vec<(f32, f32)> {
        let (dx, dy) = (p.0 - c.0, p.1 - c.1);
        let turn = |a: f32, x: f32, y: f32| {
            let (s, co) = a.sin_cos();
            (c.0 + x * co - y * s, c.1 + x * s + y * co)
        };
        match self {
            Symmetry::Off => vec![p],
            Symmetry::Mirror => vec![p, (c.0 - dx, p.1)],
            Symmetry::Quad => vec![p, (c.0 - dx, p.1), (p.0, c.1 - dy), (c.0 - dx, c.1 - dy)],
            Symmetry::Radial(n) => {
                let n = n.max(2) as usize;
                (0..n).map(|i| turn(i as f32 * std::f32::consts::TAU / n as f32, dx, dy)).collect()
            }
            Symmetry::Kaleido(n) => {
                let n = n.max(2) as usize;
                (0..n)
                    .flat_map(|i| {
                        let a = i as f32 * std::f32::consts::TAU / n as f32;
                        [turn(a, dx, dy), turn(a, dx, -dy)]
                    })
                    .collect()
            }
        }
    }
}

#[cfg(test)]
mod symmetry_tests {
    use super::*;

    #[test]
    fn copies_land_where_they_should() {
        let c = (50.0, 50.0);
        assert_eq!(Symmetry::Mirror.points((60.0, 30.0), c), vec![(60.0, 30.0), (40.0, 30.0)]);
        let q = Symmetry::Quad.points((60.0, 30.0), c);
        assert!(q.contains(&(40.0, 70.0)) && q.len() == 4);
        let r = Symmetry::Radial(4).points((60.0, 50.0), c);
        assert_eq!(r.len(), 4);
        assert!((r[1].0 - 50.0).abs() < 1e-4 && (r[1].1 - 60.0).abs() < 1e-4, "a quarter turn: {:?}", r[1]);
        let k = Symmetry::Kaleido(3).points((60.0, 45.0), c);
        assert_eq!(k.len(), 6);
        assert!((k[1].1 - 55.0).abs() < 1e-4, "mirrored copy: {:?}", k[1]);
        for s in [Symmetry::Off, Symmetry::Mirror, Symmetry::Quad, Symmetry::Radial(5), Symmetry::Kaleido(6)] {
            assert_eq!(s.points((1.0, 2.0), c).len(), s.copies());
        }
    }
}
