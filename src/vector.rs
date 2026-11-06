//! Bézier paths: building, converting, flattening, node editing helpers and
//! boolean operations (union / subtract / intersect / exclude) via i_overlay.

use crate::doc::{Object, PathNode, ShapeKind, SubPath};
use crate::geom::{simplify, Pt};
use crate::shapes;
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;
use std::sync::Arc;
use tiny_skia::{FilterQuality, Path, PathBuilder, PathSegment, Pixmap, PixmapPaint, Transform};

pub type Contours = Vec<Vec<[f64; 2]>>;
pub type Shapes = Vec<Vec<Vec<[f64; 2]>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoolOp {
    Union,
    Subtract,
    Intersect,
    Exclude,
}

impl BoolOp {
    pub const ALL: [BoolOp; 4] = [BoolOp::Union, BoolOp::Subtract, BoolOp::Intersect, BoolOp::Exclude];
    pub fn label(self) -> &'static str {
        match self {
            BoolOp::Union => "Union",
            BoolOp::Subtract => "Subtract",
            BoolOp::Intersect => "Intersect",
            BoolOp::Exclude => "Exclude",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            BoolOp::Union => "Merge all selected shapes into one",
            BoolOp::Subtract => "Cut the upper shapes out of the bottom one",
            BoolOp::Intersect => "Keep only the area all shapes share",
            BoolOp::Exclude => "Keep the areas covered an odd number of times",
        }
    }
}

fn lerp2(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

fn is_line(a: &PathNode, b: &PathNode) -> bool {
    a.h_out == a.p && b.h_in == b.p
}

/// Build a tiny-skia path from normalised sub-paths, scaled to the local box w x h.
pub fn build(subpaths: &[SubPath], w: f32, h: f32) -> Option<Path> {
    let s = |q: [f32; 2]| (q[0] * w, q[1] * h);
    let seg = |pb: &mut PathBuilder, a: &PathNode, b: &PathNode| {
        let e = s(b.p);
        if is_line(a, b) {
            pb.line_to(e.0, e.1);
        } else {
            let c1 = s(a.h_out);
            let c2 = s(b.h_in);
            pb.cubic_to(c1.0, c1.1, c2.0, c2.1, e.0, e.1);
        }
    };
    let mut pb = PathBuilder::new();
    for sp in subpaths {
        let n = sp.nodes.len();
        if n < 2 {
            continue;
        }
        let st = s(sp.nodes[0].p);
        pb.move_to(st.0, st.1);
        for i in 1..n {
            seg(&mut pb, &sp.nodes[i - 1], &sp.nodes[i]);
        }
        if sp.closed {
            seg(&mut pb, &sp.nodes[n - 1], &sp.nodes[0]);
            pb.close();
        }
    }
    pb.finish()
}

fn smooth_flag(nd: &PathNode) -> bool {
    let a = [nd.h_in[0] - nd.p[0], nd.h_in[1] - nd.p[1]];
    let b = [nd.h_out[0] - nd.p[0], nd.h_out[1] - nd.p[1]];
    let la = (a[0] * a[0] + a[1] * a[1]).sqrt();
    let lb = (b[0] * b[0] + b[1] * b[1]).sqrt();
    if la < 1e-6 || lb < 1e-6 {
        return false;
    }
    let cross = (a[0] * b[1] - a[1] * b[0]) / (la * lb);
    let dot = (a[0] * b[0] + a[1] * b[1]) / (la * lb);
    cross.abs() < 0.02 && dot < 0.0
}

/// Convert a tiny-skia path in local pixel space into normalised editable sub-paths.
pub fn from_skia(path: &Path, w: f32, h: f32) -> Vec<SubPath> {
    let (w, h) = (w.max(1e-3), h.max(1e-3));
    let n = |p: tiny_skia::Point| [p.x / w, p.y / h];
    let mut out: Vec<SubPath> = Vec::new();
    let mut cur: Option<SubPath> = None;
    let finish = |out: &mut Vec<SubPath>, sp: SubPath| {
        if sp.nodes.len() >= 2 {
            out.push(sp);
        }
    };
    for seg in path.segments() {
        match seg {
            PathSegment::MoveTo(p) => {
                if let Some(sp) = cur.take() {
                    finish(&mut out, sp);
                }
                cur = Some(SubPath {
                    nodes: vec![PathNode::corner(n(p))],
                    closed: false,
                });
            }
            PathSegment::LineTo(p) => {
                if let Some(sp) = cur.as_mut() {
                    sp.nodes.push(PathNode::corner(n(p)));
                }
            }
            PathSegment::QuadTo(q, p) => {
                if let Some(sp) = cur.as_mut() {
                    let (qn, pn) = (n(q), n(p));
                    if let Some(last) = sp.nodes.last_mut() {
                        last.h_out = lerp2(last.p, qn, 2.0 / 3.0);
                    }
                    sp.nodes.push(PathNode {
                        p: pn,
                        h_in: lerp2(pn, qn, 2.0 / 3.0),
                        h_out: pn,
                        smooth: false,
                    });
                }
            }
            PathSegment::CubicTo(c1, c2, p) => {
                if let Some(sp) = cur.as_mut() {
                    if let Some(last) = sp.nodes.last_mut() {
                        last.h_out = n(c1);
                    }
                    let pn = n(p);
                    sp.nodes.push(PathNode {
                        p: pn,
                        h_in: n(c2),
                        h_out: pn,
                        smooth: false,
                    });
                }
            }
            PathSegment::Close => {
                if let Some(mut sp) = cur.take() {
                    sp.closed = true;
                    if sp.nodes.len() > 1 {
                        let first = sp.nodes[0].p;
                        let last = sp.nodes[sp.nodes.len() - 1].p;
                        if (first[0] - last[0]).abs() < 1e-4 && (first[1] - last[1]).abs() < 1e-4 {
                            let l = sp.nodes.pop().expect("node");
                            sp.nodes[0].h_in = l.h_in;
                        }
                    }
                    finish(&mut out, sp);
                }
            }
        }
    }
    if let Some(sp) = cur.take() {
        finish(&mut out, sp);
    }
    for sp in &mut out {
        for nd in &mut sp.nodes {
            nd.smooth = smooth_flag(nd);
        }
    }
    out
}

/// Turn any vector object (rect, ellipse, star, freehand, text) into an editable path.
/// Placement and perspective are kept.
pub fn convert_to_path(obj: &mut Object) -> bool {
    if matches!(obj.kind, ShapeKind::Path { .. } | ShapeKind::Bitmap) {
        return false;
    }
    if obj.kind == ShapeKind::Ellipse {
        // the classic 4-node Bézier circle is much nicer to edit than tiny-skia's conic pieces
        let k = 0.5523 * 0.5;
        let node = |p: [f32; 2], d: [f32; 2]| PathNode {
            p,
            h_in: [p[0] - d[0], p[1] - d[1]],
            h_out: [p[0] + d[0], p[1] + d[1]],
            smooth: true,
        };
        let nodes = vec![
            node([1.0, 0.5], [0.0, k]),
            node([0.5, 1.0], [-k, 0.0]),
            node([0.0, 0.5], [0.0, -k]),
            node([0.5, 0.0], [k, 0.0]),
        ];
        obj.kind = ShapeKind::Path {
            subpaths: vec![SubPath { nodes, closed: true }],
        };
        obj.touch();
        return true;
    }
    let (w, h) = (obj.size[0], obj.size[1]);
    let Some(path) = shapes::build_path(&obj.kind, w, h) else {
        return false;
    };
    let subpaths = from_skia(&path, w, h);
    if subpaths.is_empty() {
        return false;
    }
    obj.kind = ShapeKind::Path { subpaths };
    obj.touch();
    true
}

fn curve_steps(pts: &[Pt], tol: f32) -> usize {
    let mut len = 0.0;
    for w in pts.windows(2) {
        len += w[0].dist(w[1]);
    }
    ((len * 0.5 / tol.max(0.01)).sqrt().ceil() as usize).clamp(2, 256)
}

/// Flatten a path into polylines (one per contour).
pub fn flatten(path: &Path, tol: f32) -> Vec<Vec<Pt>> {
    let mut out = Vec::new();
    let mut cur: Vec<Pt> = Vec::new();
    let mut last = Pt::default();
    let mut start = Pt::default();
    for seg in path.segments() {
        match seg {
            PathSegment::MoveTo(p) => {
                if cur.len() >= 2 {
                    out.push(std::mem::take(&mut cur));
                }
                cur.clear();
                last = Pt::new(p.x, p.y);
                start = last;
                cur.push(last);
            }
            PathSegment::LineTo(p) => {
                last = Pt::new(p.x, p.y);
                cur.push(last);
            }
            PathSegment::QuadTo(q, p) => {
                let (q, e) = (Pt::new(q.x, q.y), Pt::new(p.x, p.y));
                let n = curve_steps(&[last, q, e], tol);
                for i in 1..=n {
                    let t = i as f32 / n as f32;
                    let u = 1.0 - t;
                    cur.push(last * (u * u) + q * (2.0 * u * t) + e * (t * t));
                }
                last = e;
            }
            PathSegment::CubicTo(c1, c2, p) => {
                let (c1, c2, e) = (Pt::new(c1.x, c1.y), Pt::new(c2.x, c2.y), Pt::new(p.x, p.y));
                let n = curve_steps(&[last, c1, c2, e], tol);
                for i in 1..=n {
                    cur.push(cubic(last, c1, c2, e, i as f32 / n as f32));
                }
                last = e;
            }
            PathSegment::Close => {
                if cur.len() >= 2 {
                    out.push(std::mem::take(&mut cur));
                }
                last = start;
            }
        }
    }
    if cur.len() >= 2 {
        out.push(cur);
    }
    out
}

pub fn cubic(p0: Pt, c1: Pt, c2: Pt, p3: Pt, t: f32) -> Pt {
    let u = 1.0 - t;
    p0 * (u * u * u) + c1 * (3.0 * u * u * t) + c2 * (3.0 * u * t * t) + p3 * (t * t * t)
}

/// Canvas point -> the object's own (unbent) local pixel coordinates, through its perspective
/// and, if it has one, its warp mesh.
pub fn canvas_to_local(o: &Object, p: Pt) -> Option<Pt> {
    let hm = o.homography();
    let Some(mesh) = o.mesh.as_ref().filter(|m| !m.is_identity()) else {
        return hm.inverse()?.apply_pt(p);
    };
    const SUB: usize = 3;
    let (nu, nv) = (mesh.cols * SUB, mesh.rows * SUB);
    let (w, h) = (o.size[0], o.size[1]);
    let mut verts = Vec::with_capacity((nu + 1) * (nv + 1));
    for j in 0..=nv {
        for i in 0..=nu {
            let (u, v) = (i as f32 / nu as f32, j as f32 / nv as f32);
            let m = mesh.map(u, v);
            verts.push((hm.apply_pt(Pt::new(m[0] * w, m[1] * h))?, Pt::new(u * w, v * h)));
        }
    }
    let edge = |a: Pt, b: Pt, q: Pt| (b.x - a.x) * (q.y - a.y) - (b.y - a.y) * (q.x - a.x);
    for j in 0..nv {
        for i in 0..nu {
            let a = j * (nu + 1) + i;
            for (x, y, z) in [(a, a + 1, a + nu + 2), (a, a + nu + 2, a + nu + 1)] {
                let (pa, pb, pc) = (verts[x].0, verts[y].0, verts[z].0);
                let area = edge(pa, pb, pc);
                if area.abs() < 1e-9 {
                    continue;
                }
                let w0 = edge(pb, pc, p) / area;
                let w1 = edge(pc, pa, p) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 >= -1e-4 && w1 >= -1e-4 && w2 >= -1e-4 {
                    return Some(verts[x].1 * w0 + verts[y].1 * w1 + verts[z].1 * w2);
                }
            }
        }
    }
    None
}

/// The object's outline as closed polygons in canvas coordinates (None for paint layers).
pub fn outline_doc(obj: &Object) -> Option<Contours> {
    if obj.kind == ShapeKind::Bitmap {
        return None;
    }
    let (w, h) = (obj.size[0], obj.size[1]);
    let path = shapes::build_path(&obj.kind, w, h)?;
    let hm = obj.homography();
    let out: Contours = flatten(&path, 0.15)
        .into_iter()
        .filter(|c| c.len() >= 3)
        .map(|c| c.iter().filter_map(|p| hm.apply(p.x as f64, p.y as f64)).map(|(x, y)| [x, y]).collect())
        .collect();
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Boolean combination of outlines, bottom-most first.
pub fn boolean(inputs: &[Contours], op: BoolOp) -> Shapes {
    let Some(first) = inputs.first() else {
        return Vec::new();
    };
    let empty: Contours = Vec::new();
    let mut acc: Shapes = first.overlay(&empty, OverlayRule::Subject, FillRule::NonZero);
    match op {
        BoolOp::Subtract => {
            let mut cutter: Shapes = Vec::new();
            for c in &inputs[1..] {
                cutter = if cutter.is_empty() {
                    c.overlay(&empty, OverlayRule::Subject, FillRule::NonZero)
                } else {
                    cutter.overlay(c, OverlayRule::Union, FillRule::NonZero)
                };
            }
            acc = acc.overlay(&cutter, OverlayRule::Difference, FillRule::NonZero);
        }
        _ => {
            let rule = match op {
                BoolOp::Union => OverlayRule::Union,
                BoolOp::Intersect => OverlayRule::Intersect,
                _ => OverlayRule::Xor,
            };
            for c in &inputs[1..] {
                acc = acc.overlay(c, rule, FillRule::NonZero);
            }
        }
    }
    acc
}

/// Douglas-Peucker for a closed loop: split at the point farthest from the start so neither
/// half has identical end points.
fn simplify_closed(pts: &[Pt], eps: f32) -> Vec<Pt> {
    let far = (1..pts.len())
        .max_by(|&a, &b| pts[0].dist(pts[a]).total_cmp(&pts[0].dist(pts[b])))
        .unwrap_or(0);
    if far == 0 {
        return pts.to_vec();
    }
    let mut out = simplify(&pts[..=far], eps);
    let mut back = pts[far..].to_vec();
    back.push(pts[0]);
    let b = simplify(&back, eps);
    out.extend_from_slice(&b[1..b.len() - 1]);
    out
}

/// Rebuild smooth, editable nodes from polygons: collinear points are dropped and gentle
/// turns become smooth Bézier nodes, sharp ones stay corners.
pub fn polygons_to_object_parts(shapes: &Shapes) -> Option<(Vec<SubPath>, [Pt; 4], [f32; 2])> {
    let mut contours: Vec<Vec<Pt>> = Vec::new();
    for shape in shapes {
        for c in shape {
            let pts: Vec<Pt> = c.iter().map(|p| Pt::new(p[0] as f32, p[1] as f32)).collect();
            if pts.len() < 3 {
                continue;
            }
            let s = simplify_closed(&pts, 0.35);
            if s.len() >= 3 {
                contours.push(s);
            }
        }
    }
    if contours.is_empty() {
        return None;
    }
    let (mut lo, mut hi) = (contours[0][0], contours[0][0]);
    for c in &contours {
        for p in c {
            lo = Pt::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = Pt::new(hi.x.max(p.x), hi.y.max(p.y));
        }
    }
    let (w, h) = ((hi.x - lo.x).max(1.0), (hi.y - lo.y).max(1.0));
    let norm = |p: Pt| [(p.x - lo.x) / w, (p.y - lo.y) / h];
    let subpaths = contours
        .iter()
        .map(|c| {
            let n = c.len();
            let nodes = (0..n)
                .map(|i| {
                    let prev = c[(i + n - 1) % n];
                    let cur = c[i];
                    let next = c[(i + 1) % n];
                    let a = cur - prev;
                    let b = next - cur;
                    let cos = a.dot(b) / (a.len() * b.len()).max(1e-6);
                    if cos > 0.85 {
                        // turn < ~32°: smooth node with Catmull-Rom style handles
                        let t = (next - prev).normalized();
                        PathNode {
                            p: norm(cur),
                            h_in: norm(cur - t * (a.len() / 3.0)),
                            h_out: norm(cur + t * (b.len() / 3.0)),
                            smooth: true,
                        }
                    } else {
                        PathNode::corner(norm(cur))
                    }
                })
                .collect();
            SubPath { nodes, closed: true }
        })
        .collect();
    Some((subpaths, crate::doc::Object::rect_quad(lo.x, lo.y, w, h), [w.round(), h.round()]))
}

/// After node edits, refit the object's local box to the path's tight bounds without moving
/// anything on the canvas (exact even in perspective). Paint is carried along.
pub fn normalize_bounds(obj: &mut Object) -> bool {
    let ShapeKind::Path { subpaths } = &obj.kind else {
        return false;
    };
    let (w, h) = (obj.size[0], obj.size[1]);
    let Some(path) = build(subpaths, w, h) else {
        return false;
    };
    let Some(b) = path.compute_tight_bounds() else {
        return false;
    };
    let (mut x0, mut y0, mut x1, mut y1) = (b.left(), b.top(), b.right(), b.bottom());
    if x1 - x0 < 1.0 {
        let c = (x0 + x1) * 0.5;
        x0 = c - 0.5;
        x1 = c + 0.5;
    }
    if y1 - y0 < 1.0 {
        let c = (y0 + y1) * 0.5;
        y0 = c - 0.5;
        y1 = c + 0.5;
    }
    if x0.abs() < 0.01 && y0.abs() < 0.01 && (x1 - w).abs() < 0.01 && (y1 - h).abs() < 0.01 {
        return false;
    }
    let hm = obj.homography();
    let (Some(a), Some(bq), Some(c), Some(d)) = (
        hm.apply_pt(Pt::new(x0, y0)),
        hm.apply_pt(Pt::new(x1, y0)),
        hm.apply_pt(Pt::new(x1, y1)),
        hm.apply_pt(Pt::new(x0, y1)),
    ) else {
        return false;
    };
    let (nw, nh) = (x1 - x0, y1 - y0);
    let remap = |q: [f32; 2]| [(q[0] * w - x0) / nw, (q[1] * h - y0) / nh];
    if let ShapeKind::Path { subpaths } = &mut obj.kind {
        for sp in subpaths.iter_mut() {
            for nd in sp.nodes.iter_mut() {
                nd.p = remap(nd.p);
                nd.h_in = remap(nd.h_in);
                nd.h_out = remap(nd.h_out);
            }
        }
    }
    if let Some(old) = obj.paint.take() {
        let kx = old.width() as f32 / w.max(1.0);
        let ky = old.height() as f32 / h.max(1.0);
        let npw = (nw * kx).round().clamp(1.0, 4096.0);
        let nph = (nh * ky).round().clamp(1.0, 4096.0);
        if let Some(mut np) = Pixmap::new(npw as u32, nph as u32) {
            let sx = (w / old.width() as f32) * (npw / nw);
            let sy = (h / old.height() as f32) * (nph / nh);
            let t = Transform::from_row(sx, 0.0, 0.0, sy, -x0 * npw / nw, -y0 * nph / nh);
            let pp = PixmapPaint {
                quality: FilterQuality::Nearest,
                ..Default::default()
            };
            let old_ref: &Pixmap = &old;
            np.draw_pixmap(0, 0, old_ref.as_ref(), &pp, t, None);
            obj.paint = Some(Arc::new(np));
        } else {
            obj.paint = Some(old);
        }
    }
    obj.quad = [a, bq, c, d];
    obj.size = [nw.round().clamp(1.0, 4096.0), nh.round().clamp(1.0, 4096.0)];
    obj.touch();
    true
}

/// Split segment `i` (node i -> node i+1, wrapping for closed paths) at parameter t.
/// Returns the index of the new node.
pub fn split_segment(sp: &mut SubPath, i: usize, t: f32) -> usize {
    let n = sp.nodes.len();
    let j = (i + 1) % n;
    let (a, b) = (sp.nodes[i], sp.nodes[j]);
    let new_node = if is_line(&a, &b) {
        PathNode::corner(lerp2(a.p, b.p, t))
    } else {
        let ab = lerp2(a.p, a.h_out, t);
        let bc = lerp2(a.h_out, b.h_in, t);
        let cd = lerp2(b.h_in, b.p, t);
        let abc = lerp2(ab, bc, t);
        let bcd = lerp2(bc, cd, t);
        let m = lerp2(abc, bcd, t);
        sp.nodes[i].h_out = ab;
        sp.nodes[j].h_in = cd;
        PathNode {
            p: m,
            h_in: abc,
            h_out: bcd,
            smooth: true,
        }
    };
    sp.nodes.insert(i + 1, new_node);
    i + 1
}

/// Make a node smooth (handles from its neighbours) or a sharp corner (handles retracted).
pub fn toggle_smooth(sp: &mut SubPath, i: usize, w: f32, h: f32) {
    let n = sp.nodes.len();
    let nd = sp.nodes[i];
    if nd.smooth || nd.h_in != nd.p || nd.h_out != nd.p {
        sp.nodes[i].h_in = nd.p;
        sp.nodes[i].h_out = nd.p;
        sp.nodes[i].smooth = false;
        return;
    }
    let has_prev = sp.closed || i > 0;
    let has_next = sp.closed || i + 1 < n;
    let prev = if has_prev { sp.nodes[(i + n - 1) % n].p } else { nd.p };
    let next = if has_next { sp.nodes[(i + 1) % n].p } else { nd.p };
    // work in pixels so the tangent is not distorted by a non-square box
    let px = |q: [f32; 2]| Pt::new(q[0] * w, q[1] * h);
    let (pp, pc, pn) = (px(prev), px(nd.p), px(next));
    let t = (pn - pp).normalized();
    let hin = pc - t * (pc.dist(pp) / 3.0);
    let hout = pc + t * (pc.dist(pn) / 3.0);
    sp.nodes[i].h_in = [hin.x / w, hin.y / h];
    sp.nodes[i].h_out = [hout.x / w, hout.y / h];
    sp.nodes[i].smooth = true;
}
