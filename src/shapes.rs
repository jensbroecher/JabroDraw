//! Vector geometry for each shape kind, built in the object's local space (0..w, 0..h).

use crate::doc::{FontChoice, ShapeKind, TextAlign, TextLayout, TextPath};
use crate::fonts;
use ab_glyph::{Font, OutlineCurve};
use tiny_skia::{Path, PathBuilder, Rect};

pub fn build_path(kind: &ShapeKind, w: f32, h: f32) -> Option<Path> {
    match kind {
        ShapeKind::Rect { corner } => {
            let r = corner.clamp(0.0, 1.0) * w.min(h) * 0.5;
            if r < 0.5 {
                Some(PathBuilder::from_rect(Rect::from_xywh(0.0, 0.0, w, h)?))
            } else {
                rounded_rect(w, h, r)
            }
        }
        ShapeKind::Ellipse => PathBuilder::from_oval(Rect::from_xywh(0.0, 0.0, w, h)?),
        ShapeKind::Polygon { sides, inner } => polygon(*sides, *inner, w, h),
        ShapeKind::Freehand { points } => smooth_closed(points, w, h),
        ShapeKind::Text { text, font, layout } => text_path(text, *font, layout, w, h),
        ShapeKind::Bitmap => None,
        ShapeKind::Path { subpaths } => crate::vector::build(subpaths, w, h),
    }
}

fn rounded_rect(w: f32, h: f32, r: f32) -> Option<Path> {
    let k = 0.5523 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(r, 0.0);
    pb.line_to(w - r, 0.0);
    pb.cubic_to(w - r + k, 0.0, w, r - k, w, r);
    pb.line_to(w, h - r);
    pb.cubic_to(w, h - r + k, w - r + k, h, w - r, h);
    pb.line_to(r, h);
    pb.cubic_to(r - k, h, 0.0, h - r + k, 0.0, h - r);
    pb.line_to(0.0, r);
    pb.cubic_to(0.0, r - k, r - k, 0.0, r, 0.0);
    pb.close();
    pb.finish()
}

fn polygon(sides: u32, inner: f32, w: f32, h: f32) -> Option<Path> {
    let n = sides.clamp(3, 64) as usize;
    let star = inner < 0.999;
    let count = if star { n * 2 } else { n };
    let mut pb = PathBuilder::new();
    for i in 0..count {
        let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::TAU / count as f32;
        let r = if star && i % 2 == 1 { inner.clamp(0.05, 1.0) } else { 1.0 };
        let x = (0.5 + 0.5 * r * a.cos()) * w;
        let y = (0.5 + 0.5 * r * a.sin()) * h;
        if i == 0 {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
    pb.close();
    pb.finish()
}

/// Closed Catmull-Rom spline through the points.
fn smooth_closed(points: &[[f32; 2]], w: f32, h: f32) -> Option<Path> {
    let n = points.len();
    if n < 3 {
        return None;
    }
    let p = |i: usize| -> (f32, f32) {
        let q = points[i % n];
        (q[0] * w, q[1] * h)
    };
    let mut pb = PathBuilder::new();
    let (sx, sy) = p(0);
    pb.move_to(sx, sy);
    for i in 0..n {
        let p0 = p(i + n - 1);
        let p1 = p(i);
        let p2 = p(i + 1);
        let p3 = p(i + 2);
        let c1 = (p1.0 + (p2.0 - p0.0) / 6.0, p1.1 + (p2.1 - p0.1) / 6.0);
        let c2 = (p2.0 - (p3.0 - p1.0) / 6.0, p2.1 - (p3.1 - p1.1) / 6.0);
        pb.cubic_to(c1.0, c1.1, c2.0, c2.1, p2.0, p2.1);
    }
    pb.close();
    pb.finish()
}

enum Seg {
    Move(f32, f32),
    Line(f32, f32),
    Quad(f32, f32, f32, f32),
    Cubic(f32, f32, f32, f32, f32, f32),
    Close,
}

struct TextOutline {
    segs: Vec<Seg>,
    min: (f32, f32),
    max: (f32, f32),
    unit: f32,
}

/// Extent of laid-out text in its layout units (font units, or curve units on a path).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextBounds {
    pub min: [f32; 2],
    pub max: [f32; 2],
    /// Size of one layout unit in em (font units and curve units differ).
    pub unit: f32,
}

/// A curve prepared for walking along it by arc length.
struct Walk {
    pts: Vec<(f32, f32)>,
    cum: Vec<f32>,
    total: f32,
    closed: bool,
}

impl Walk {
    fn new(tp: &TextPath) -> Option<Walk> {
        let mut pts: Vec<(f32, f32)> = tp.pts.iter().map(|p| (p[0], p[1])).collect();
        if tp.flip {
            pts.reverse();
        }
        if tp.closed && pts.len() > 2 {
            pts.push(pts[0]);
        }
        if pts.len() < 2 {
            return None;
        }
        let mut cum = vec![0.0f32];
        for w in pts.windows(2) {
            let d = ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt();
            let last = cum[cum.len() - 1];
            cum.push(last + d);
        }
        let total = cum[cum.len() - 1];
        (total > 1e-3).then_some(Walk { pts, cum, total, closed: tp.closed })
    }

    /// Point at arc length `s` (wraps on closed curves, runs straight on past open ends).
    fn at(&self, s: f32) -> (f32, f32) {
        let s = if self.closed { s.rem_euclid(self.total) } else { s };
        let n = self.pts.len();
        let (i, s) = if s <= 0.0 {
            (0, s)
        } else if s >= self.total {
            (n - 2, s - self.cum[n - 2])
        } else {
            let i = self.cum.partition_point(|&c| c <= s).clamp(1, n - 1) - 1;
            (i, s - self.cum[i])
        };
        let (a, b) = (self.pts[i], self.pts[i + 1]);
        let len = (self.cum[i + 1] - self.cum[i]).max(1e-6);
        (a.0 + (b.0 - a.0) * s / len, a.1 + (b.1 - a.1) * s / len)
    }
}

/// Where one glyph goes: its centre on the baseline, the baseline direction and "up".
#[derive(Clone, Copy)]
struct Frame {
    p: (f32, f32),
    t: (f32, f32),
    up: (f32, f32),
    scale: f32,
}

impl Frame {
    /// Map a font-space point (relative to the glyph's centre, y up) into layout space.
    fn map(&self, dx: f32, gy: f32) -> (f32, f32) {
        let (x, y) = (dx * self.scale, gy * self.scale);
        (self.p.0 + self.t.0 * x + self.up.0 * y, self.p.1 + self.t.1 * x + self.up.1 * y)
    }
}

fn text_outline(text: &str, font: FontChoice, layout: &TextLayout) -> Option<TextOutline> {
    let f = fonts::get(font)?;
    let upm = f.units_per_em().unwrap_or(1000.0);
    let ascent = f.ascent_unscaled();
    let descent = f.descent_unscaled();
    let lh = (ascent - descent + f.line_gap_unscaled() + layout.leading * upm).max(upm * 0.2);
    let spacing = layout.spacing * upm;
    let walk = layout.path.as_ref().and_then(Walk::new);
    // text on a curve is a single line
    let joined: String;
    let lines: Vec<&str> = if walk.is_some() {
        joined = text.split('\n').collect::<Vec<_>>().join(" ");
        vec![joined.as_str()]
    } else {
        text.split('\n').collect()
    };
    // glyphs with their pen position along each line
    let mut glyphs: Vec<Vec<(ab_glyph::GlyphId, f32, f32)>> = Vec::with_capacity(lines.len());
    let mut widths = Vec::with_capacity(lines.len());
    for line in &lines {
        let mut pen = 0.0;
        let mut prev = None;
        let mut row = Vec::new();
        for ch in line.chars() {
            let gid = f.glyph_id(ch);
            if let Some(p) = prev {
                pen += f.kern_unscaled(p, gid);
            }
            let adv = f.h_advance_unscaled(gid);
            row.push((gid, pen, adv));
            pen += adv + spacing;
            prev = Some(gid);
        }
        widths.push(if row.is_empty() { 0.0 } else { pen - spacing });
        glyphs.push(row);
    }
    let maxw = widths.iter().cloned().fold(0.0, f32::max);
    let shift = |w: f32| match layout.align {
        TextAlign::Left => 0.0,
        TextAlign::Center => (maxw - w) * 0.5,
        TextAlign::Right => maxw - w,
    };
    let arc = layout.arc.clamp(-360.0, 360.0).to_radians();
    let radius = (arc.abs() > 0.01 && maxw > 0.0).then(|| maxw / arc);
    let frame = |li: usize, cx: f32| -> Frame {
        if let (Some(w), Some(tp)) = (walk.as_ref(), layout.path.as_ref()) {
            let scale = tp.em.max(0.01) / upm;
            let len = widths[0] * scale;
            let s0 = tp.start.clamp(0.0, 1.0) * w.total
                - match layout.align {
                    TextAlign::Left => 0.0,
                    TextAlign::Center => len * 0.5,
                    TextAlign::Right => len,
                };
            let s = s0 + cx * scale;
            // average the direction over about a letter, so corners turn smoothly
            let hw = (upm * 0.25 * scale).max(0.5);
            let (a, b) = (w.at(s - hw), w.at(s + hw));
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let l = (dx * dx + dy * dy).sqrt().max(1e-6);
            let t = (dx / l, dy / l);
            let up = (t.1, -t.0);
            let p = w.at(s);
            let lift = tp.offset * tp.em;
            return Frame { p: (p.0 + up.0 * lift, p.1 + up.1 * lift), t, up, scale };
        }
        let base = li as f32 * lh + ascent;
        match radius {
            Some(r) => {
                // inner lines of an arch get a smaller circle; never let the radius flip over
                let rl = r - li as f32 * lh;
                let rl = if rl.signum() != r.signum() || rl.abs() < lh * 0.5 { lh * 0.5 * r.signum() } else { rl };
                let th = (cx - maxw * 0.5) / rl;
                let (s, c) = th.sin_cos();
                let centre = (maxw * 0.5, ascent + r);
                Frame { p: (centre.0 + rl * s, centre.1 - rl * c), t: (c, s), up: (s, -c), scale: 1.0 }
            }
            None => Frame { p: (cx, base), t: (1.0, 0.0), up: (0.0, -1.0), scale: 1.0 },
        }
    };
    let mut segs = Vec::new();
    let mut min = (f32::MAX, f32::MAX);
    let mut max = (f32::MIN, f32::MIN);
    let mut track = |p: (f32, f32)| {
        min = (min.0.min(p.0), min.1.min(p.1));
        max = (max.0.max(p.0), max.1.max(p.1));
    };
    for (li, row) in glyphs.iter().enumerate() {
        let off = if walk.is_some() { 0.0 } else { shift(widths[li]) };
        for &(gid, pen, adv) in row {
            let Some(ol) = f.outline(gid) else { continue };
            let fr = frame(li, off + pen + adv * 0.5);
            let tx = |pt: ab_glyph::Point| fr.map(pt.x - adv * 0.5, pt.y);
            let mut last: Option<(f32, f32)> = None;
            for c in &ol.curves {
                let (start, seg, end) = match *c {
                    OutlineCurve::Line(a, b) => {
                        let (a, b) = (tx(a), tx(b));
                        track(b);
                        (a, Seg::Line(b.0, b.1), b)
                    }
                    OutlineCurve::Quad(a, b, c2) => {
                        let (a, b, c2) = (tx(a), tx(b), tx(c2));
                        track(b);
                        track(c2);
                        (a, Seg::Quad(b.0, b.1, c2.0, c2.1), c2)
                    }
                    OutlineCurve::Cubic(a, b, c2, d) => {
                        let (a, b, c2, d) = (tx(a), tx(b), tx(c2), tx(d));
                        track(b);
                        track(c2);
                        track(d);
                        (a, Seg::Cubic(b.0, b.1, c2.0, c2.1, d.0, d.1), d)
                    }
                };
                let jump = match last {
                    None => true,
                    Some(l) => (l.0 - start.0).abs() > 1e-3 || (l.1 - start.1).abs() > 1e-3,
                };
                if jump {
                    if last.is_some() {
                        segs.push(Seg::Close);
                    }
                    segs.push(Seg::Move(start.0, start.1));
                    track(start);
                }
                segs.push(seg);
                last = Some(end);
            }
            if last.is_some() {
                segs.push(Seg::Close);
            }
        }
    }
    if segs.is_empty() || max.0 <= min.0 || max.1 <= min.1 {
        return None;
    }
    let unit = match (walk.is_some(), layout.path.as_ref()) {
        (true, Some(tp)) => 1.0 / tp.em.max(0.01),
        _ => 1.0 / upm,
    };
    Some(TextOutline { segs, min, max, unit })
}

/// Text outlines stretched to fill the local box (w x h).
pub fn text_path(text: &str, font: FontChoice, layout: &TextLayout, w: f32, h: f32) -> Option<Path> {
    let t = text_outline(text, font, layout)?;
    let sx = w / (t.max.0 - t.min.0);
    let sy = h / (t.max.1 - t.min.1);
    let m = |x: f32, y: f32| ((x - t.min.0) * sx, (y - t.min.1) * sy);
    let mut pb = PathBuilder::new();
    for s in &t.segs {
        match *s {
            Seg::Move(x, y) => {
                let p = m(x, y);
                pb.move_to(p.0, p.1)
            }
            Seg::Line(x, y) => {
                let p = m(x, y);
                pb.line_to(p.0, p.1)
            }
            Seg::Quad(x1, y1, x, y) => {
                let (a, b) = (m(x1, y1), m(x, y));
                pb.quad_to(a.0, a.1, b.0, b.1)
            }
            Seg::Cubic(x1, y1, x2, y2, x, y) => {
                let (a, b, c) = (m(x1, y1), m(x2, y2), m(x, y));
                pb.cubic_to(a.0, a.1, b.0, b.1, c.0, c.1)
            }
            Seg::Close => pb.close(),
        }
    }
    pb.finish()
}

/// Natural width / height ratio of a piece of text.
pub fn text_aspect(text: &str, font: FontChoice, layout: &TextLayout) -> Option<f32> {
    let t = text_outline(text, font, layout)?;
    Some((t.max.0 - t.min.0) / (t.max.1 - t.min.1))
}

/// Extent of the laid-out text in layout units (see `Object::relayout_text`).
pub fn text_bounds(text: &str, font: FontChoice, layout: &TextLayout) -> Option<TextBounds> {
    let t = text_outline(text, font, layout)?;
    Some(TextBounds { min: [t.min.0, t.min.1], max: [t.max.0, t.max.1], unit: t.unit })
}
