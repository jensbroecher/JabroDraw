//! Vector geometry for each shape kind, built in the object's local space (0..w, 0..h).

use crate::doc::{FontChoice, ShapeKind};
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
        ShapeKind::Text { text, font } => text_path(text, *font, w, h),
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
}

fn line_advance<F: Font>(f: &F, line: &str) -> f32 {
    let mut pen = 0.0;
    let mut prev = None;
    for ch in line.chars() {
        let g = f.glyph_id(ch);
        if let Some(p) = prev {
            pen += f.kern_unscaled(p, g);
        }
        pen += f.h_advance_unscaled(g);
        prev = Some(g);
    }
    pen
}

fn text_outline(text: &str, font: FontChoice) -> Option<TextOutline> {
    let f = fonts::get(font)?;
    let ascent = f.ascent_unscaled();
    let descent = f.descent_unscaled();
    let lh = ascent - descent + f.line_gap_unscaled();
    let lines: Vec<&str> = text.split('\n').collect();
    let widths: Vec<f32> = lines.iter().map(|l| line_advance(f, l)).collect();
    let maxw = widths.iter().cloned().fold(0.0, f32::max);
    let mut segs = Vec::new();
    let mut min = (f32::MAX, f32::MAX);
    let mut max = (f32::MIN, f32::MIN);
    let mut track = |x: f32, y: f32| {
        min = (min.0.min(x), min.1.min(y));
        max = (max.0.max(x), max.1.max(y));
    };
    for (li, line) in lines.iter().enumerate() {
        let mut pen = (maxw - widths[li]) * 0.5;
        let base = li as f32 * lh + ascent;
        let mut prev = None;
        for ch in line.chars() {
            let gid = f.glyph_id(ch);
            if let Some(p) = prev {
                pen += f.kern_unscaled(p, gid);
            }
            if let Some(ol) = f.outline(gid) {
                let tx = |pt: ab_glyph::Point| (pen + pt.x, base - pt.y);
                let mut last: Option<(f32, f32)> = None;
                for c in &ol.curves {
                    let (start, seg, end) = match *c {
                        OutlineCurve::Line(a, b) => {
                            let (a, b) = (tx(a), tx(b));
                            (a, Seg::Line(b.0, b.1), b)
                        }
                        OutlineCurve::Quad(a, b, c2) => {
                            let (a, b, c2) = (tx(a), tx(b), tx(c2));
                            (a, Seg::Quad(b.0, b.1, c2.0, c2.1), c2)
                        }
                        OutlineCurve::Cubic(a, b, c2, d) => {
                            let (a, b, c2, d) = (tx(a), tx(b), tx(c2), tx(d));
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
                        track(start.0, start.1);
                    }
                    match &seg {
                        Seg::Line(x, y) => track(*x, *y),
                        Seg::Quad(x1, y1, x, y) => {
                            track(*x1, *y1);
                            track(*x, *y)
                        }
                        Seg::Cubic(x1, y1, x2, y2, x, y) => {
                            track(*x1, *y1);
                            track(*x2, *y2);
                            track(*x, *y)
                        }
                        _ => {}
                    }
                    segs.push(seg);
                    last = Some(end);
                }
                if last.is_some() {
                    segs.push(Seg::Close);
                }
            }
            pen += f.h_advance_unscaled(gid);
            prev = Some(gid);
        }
    }
    if segs.is_empty() || max.0 <= min.0 || max.1 <= min.1 {
        return None;
    }
    Some(TextOutline { segs, min, max })
}

/// Text outlines stretched to fill the local box (w x h).
pub fn text_path(text: &str, font: FontChoice, w: f32, h: f32) -> Option<Path> {
    let t = text_outline(text, font)?;
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
pub fn text_aspect(text: &str, font: FontChoice) -> Option<f32> {
    let t = text_outline(text, font)?;
    Some((t.max.0 - t.min.0) / (t.max.1 - t.min.1))
}
