//! Bitmap → vector tracing. Our own implementation (potrace is GPL, so it is not used):
//! colour reduction with k-means, speck removal, stacked colour layers (so neighbouring
//! regions never show hairline gaps), pixel-edge contour tracing, smoothing and
//! simplification. The contours then become smooth, editable Bézier paths.

use crate::doc::Rgba;
use crate::geom::Pt;
use tiny_skia::Pixmap;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TraceSettings {
    /// Number of colours (1 = a single silhouette in the picture's average colour).
    pub colors: usize,
    /// 0 = loose and simple, 1 = follows every bump.
    pub detail: f32,
    /// 0 = keeps corners crisp, 1 = very rounded.
    pub smooth: f32,
    /// Areas smaller than this many (traced) pixels are swallowed by their surroundings.
    pub speckle: usize,
}

impl Default for TraceSettings {
    fn default() -> Self {
        Self { colors: 6, detail: 0.6, smooth: 0.5, speckle: 12 }
    }
}

/// One colour of the result: closed contours (outer edges and holes, opposite winding) in the
/// source picture's pixel coordinates. Layers come bottom first.
pub struct TracedLayer {
    pub color: Rgba,
    pub contours: Vec<Vec<Pt>>,
}

/// Longest side the tracer works at; bigger pictures are sampled down first.
const MAX_SIDE: u32 = 1200;

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

/// Picture pixels as Lab colour (None where mostly transparent), at tracing size.
fn sample_input(pm: &Pixmap) -> (usize, usize, f32, Vec<Option<[f32; 3]>>) {
    let (sw, sh) = (pm.width(), pm.height());
    let k = (MAX_SIDE as f32 / sw.max(sh) as f32).min(1.0);
    let (w, h) = (((sw as f32 * k).round() as usize).max(1), ((sh as f32 * k).round() as usize).max(1));
    let d = pm.data();
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            // box-average the source pixels this one covers (premultiplied, then unpremultiply)
            let (x0, x1) = ((x as f32 / k) as u32, (((x + 1) as f32 / k) as u32).clamp(x as u32 + 1, sw).max(1));
            let (y0, y1) = ((y as f32 / k) as u32, (((y + 1) as f32 / k) as u32).clamp(y as u32 + 1, sh).max(1));
            let mut acc = [0f32; 4];
            let mut n: f32 = 0.0;
            for sy in y0.min(sh - 1)..y1.min(sh) {
                for sx in x0.min(sw - 1)..x1.min(sw) {
                    let i = (sy * sw + sx) as usize * 4;
                    for c in 0..4 {
                        acc[c] += d[i + c] as f32;
                    }
                    n += 1.0;
                }
            }
            let a = acc[3] / n.max(1.0);
            px.push((a >= 128.0).then(|| srgb_to_lab([acc[0] / acc[3] * 255.0, acc[1] / acc[3] * 255.0, acc[2] / acc[3] * 255.0])));
        }
    }
    (w, h, 1.0 / k, px)
}

fn srgb_to_lab(c: [f32; 3]) -> [f32; 3] {
    let lin = |v: f32| {
        let v = v / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    let (r, g, b) = (lin(c[0]), lin(c[1]), lin(c[2]));
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
    let f = |t: f32| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn lab_to_srgb(l: [f32; 3]) -> [f32; 3] {
    let fy = (l[0] + 16.0) / 116.0;
    let (fx, fz) = (fy + l[1] / 500.0, fy - l[2] / 200.0);
    let inv = |t: f32| if t.powi(3) > 0.008856 { t.powi(3) } else { (t - 16.0 / 116.0) / 7.787 };
    let (x, y, z) = (inv(fx) * 0.95047, inv(fy), inv(fz) * 1.08883);
    let r = 3.2406 * x - 1.5372 * y - 0.4986 * z;
    let g = -0.9689 * x + 1.8758 * y + 0.0415 * z;
    let b = 0.0557 * x - 0.2040 * y + 1.0570 * z;
    let gam = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        255.0 * if v <= 0.0031308 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
    };
    [gam(r), gam(g), gam(b)]
}

/// Colour centres (in Lab) by weighted k-means over a colour histogram. Each distinct colour
/// counts with the square root of how often it occurs, so a thin red line or a small sun
/// still gets its own colour next to a big sky gradient.
fn palette(px: &[Option<[f32; 3]>], k: usize) -> Vec<[f32; 3]> {
    let mut bins: std::collections::HashMap<(i32, i32, i32), ([f64; 3], f64)> = std::collections::HashMap::new();
    for p in px.iter().flatten() {
        let key = ((p[0] / 2.5).floor() as i32, (p[1] / 2.5).floor() as i32, (p[2] / 2.5).floor() as i32);
        let e = bins.entry(key).or_insert(([0.0; 3], 0.0));
        for c in 0..3 {
            e.0[c] += p[c] as f64;
        }
        e.1 += 1.0;
    }
    let mut pts: Vec<([f32; 3], f32)> = bins
        .into_values()
        .map(|(s, n)| ([(s[0] / n) as f32, (s[1] / n) as f32, (s[2] / n) as f32], (n as f32).sqrt()))
        .collect();
    if pts.is_empty() {
        return Vec::new();
    }
    // deterministic order (HashMap iteration is not)
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let k = k.clamp(1, 16).min(pts.len());
    // start: the heaviest colour, then repeatedly the one worst served so far
    let first = pts.iter().max_by(|a, b| a.1.total_cmp(&b.1)).map(|p| p.0).unwrap();
    let mut centres = vec![first];
    let mut best: Vec<f32> = pts.iter().map(|p| dist2(p.0, first)).collect();
    while centres.len() < k {
        let pick = (0..pts.len()).max_by(|&a, &b| (best[a] * pts[a].1).total_cmp(&(best[b] * pts[b].1))).unwrap();
        if best[pick] <= 0.0 {
            break;
        }
        centres.push(pts[pick].0);
        for (b, p) in best.iter_mut().zip(&pts) {
            *b = b.min(dist2(p.0, pts[pick].0));
        }
    }
    for _ in 0..16 {
        let mut sum = vec![[0f32; 4]; centres.len()];
        for (p, wgt) in &pts {
            let c = nearest(&centres, *p);
            for i in 0..3 {
                sum[c][i] += p[i] * wgt;
            }
            sum[c][3] += wgt;
        }
        for (c, s) in centres.iter_mut().zip(&sum) {
            if s[3] > 0.0 {
                *c = [s[0] / s[3], s[1] / s[3], s[2] / s[3]];
            }
        }
    }
    centres
}

fn nearest(centres: &[[f32; 3]], p: [f32; 3]) -> usize {
    let mut best = (0, f32::MAX);
    for (i, c) in centres.iter().enumerate() {
        let d = dist2(*c, p);
        if d < best.1 {
            best = (i, d);
        }
    }
    best.0
}

/// Swallow connected areas smaller than `min_area` into their most common neighbour.
fn despeckle(labels: &mut [i32], w: usize, h: usize, min_area: usize) {
    if min_area <= 1 {
        return;
    }
    let mut seen = vec![false; labels.len()];
    let mut stack = Vec::new();
    let mut region = Vec::new();
    for start in 0..labels.len() {
        if seen[start] {
            continue;
        }
        let l = labels[start];
        region.clear();
        stack.push(start);
        seen[start] = true;
        let mut neighbours: Vec<(i32, usize)> = Vec::new();
        while let Some(i) = stack.pop() {
            region.push(i);
            let (x, y) = (i % w, i / w);
            let mut visit = |j: usize| {
                if labels[j] == l {
                    if !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                } else if let Some(e) = neighbours.iter_mut().find(|e| e.0 == labels[j]) {
                    e.1 += 1;
                } else {
                    neighbours.push((labels[j], 1));
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }
        if region.len() < min_area {
            if let Some(&(to, _)) = neighbours.iter().max_by_key(|e| e.1) {
                for &i in &region {
                    labels[i] = to;
                }
            }
        }
    }
}

/// Closed loops along the pixel edges of a binary mask: clockwise on screen around filled
/// areas, anticlockwise around holes, so non-zero filling reproduces the mask exactly.
fn trace_loops(mask: &[bool], w: usize, h: usize) -> Vec<Vec<(i32, i32)>> {
    let inside = |x: i64, y: i64| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && mask[y as usize * w + x as usize];
    // outgoing edges per lattice vertex (at most two at a saddle)
    let vw = w + 1;
    let mut out: Vec<[u8; 2]> = vec![[0, 0]; vw * (h + 1)]; // direction + 1 (0 = none)
    let push = |out: &mut Vec<[u8; 2]>, vx: usize, vy: usize, dir: u8| {
        let e = &mut out[vy * vw + vx];
        if e[0] == 0 {
            e[0] = dir + 1;
        } else {
            e[1] = dir + 1;
        }
    };
    // directions: 0 right, 1 down, 2 left, 3 up
    let mut edges = 0usize;
    for y in 0..h {
        for x in 0..w {
            if !mask[y * w + x] {
                continue;
            }
            let (xi, yi) = (x as i64, y as i64);
            if !inside(xi, yi - 1) {
                push(&mut out, x, y, 0);
                edges += 1;
            }
            if !inside(xi + 1, yi) {
                push(&mut out, x + 1, y, 1);
                edges += 1;
            }
            if !inside(xi, yi + 1) {
                push(&mut out, x + 1, y + 1, 2);
                edges += 1;
            }
            if !inside(xi - 1, yi) {
                push(&mut out, x, y + 1, 3);
                edges += 1;
            }
        }
    }
    let step = |d: u8| -> (i32, i32) {
        match d {
            0 => (1, 0),
            1 => (0, 1),
            2 => (-1, 0),
            _ => (0, -1),
        }
    };
    let mut loops = Vec::new();
    let mut walked = 0usize;
    for start in 0..out.len() {
        while out[start][0] != 0 {
            let (sx, sy) = ((start % vw) as i32, (start / vw) as i32);
            let mut pts = Vec::new();
            let (mut x, mut y) = (sx, sy);
            let mut prev: Option<u8> = None;
            loop {
                let idx = y as usize * vw + x as usize;
                let [a, b] = out[idx];
                if a == 0 || walked > edges {
                    break; // cannot happen on a well-formed mask; never loop forever
                }
                // at a saddle the two edges turn right and left: take the right turn, which
                // keeps diagonal neighbours apart
                let take_b = b != 0 && prev.is_some_and(|p| b - 1 == (p + 1) % 4);
                let dir = if take_b {
                    out[idx] = [a, 0];
                    b - 1
                } else {
                    out[idx] = [b, 0];
                    a - 1
                };
                if prev != Some(dir) {
                    pts.push((x, y)); // only corners
                }
                prev = Some(dir);
                let (dx, dy) = step(dir);
                x += dx;
                y += dy;
                walked += 1;
                if (x, y) == (sx, sy) {
                    break;
                }
            }
            if pts.len() >= 3 {
                loops.push(pts);
            }
        }
    }
    loops
}

fn area(c: &[Pt]) -> f32 {
    let n = c.len();
    (0..n).map(|i| c[i].cross(c[(i + 1) % n])).sum::<f32>() * 0.5
}

/// Resample a corner-only pixel loop densely, smooth it, then simplify it.
fn refine(lp: &[(i32, i32)], s: &TraceSettings, scale: f32) -> Option<Vec<Pt>> {
    let n = lp.len();
    // dense points: every pixel step along the loop, so smoothing acts evenly
    let mut dense = Vec::new();
    for i in 0..n {
        let (a, b) = (lp[i], lp[(i + 1) % n]);
        let steps = (a.0 - b.0).abs().max((a.1 - b.1).abs()).max(1);
        for k in 0..steps {
            let t = k as f32 / steps as f32;
            dense.push(Pt::new(a.0 as f32 + (b.0 - a.0) as f32 * t, a.1 as f32 + (b.1 - a.1) as f32 * t));
        }
    }
    let m = dense.len();
    if m < 4 {
        return None;
    }
    // Taubin smoothing (a shrink step then an inflate step), so shapes keep their size;
    // the pixel staircase itself is removed by the simplification below
    let passes = (s.smooth.clamp(0.0, 1.0) * 6.0).round() as usize;
    for _ in 0..passes {
        for k in [0.5f32, -0.53] {
            let prev = dense.clone();
            for i in 0..m {
                let (a, b, c) = (prev[(i + m - 1) % m], prev[i], prev[(i + 1) % m]);
                let lap = Pt::new((a.x + c.x) * 0.5 - b.x, (a.y + c.y) * 0.5 - b.y);
                dense[i] = b + lap * k;
            }
        }
    }
    let tol = 0.6 + (1.0 - s.detail.clamp(0.0, 1.0)) * 2.0;
    let simple = simplify_loop(&dense, tol);
    (simple.len() >= 3).then(|| simple.into_iter().map(|p| p * scale).collect())
}

fn simplify_loop(pts: &[Pt], eps: f32) -> Vec<Pt> {
    let far = (1..pts.len()).max_by(|&a, &b| pts[0].dist(pts[a]).total_cmp(&pts[0].dist(pts[b]))).unwrap_or(0);
    if far == 0 {
        return pts.to_vec();
    }
    let mut out = crate::geom::simplify(&pts[..=far], eps);
    let mut back = pts[far..].to_vec();
    back.push(pts[0]);
    let b = crate::geom::simplify(&back, eps);
    out.extend_from_slice(&b[1..b.len() - 1]);
    out
}

/// Trace a picture into colour layers (bottom first).
pub fn trace(pm: &Pixmap, s: &TraceSettings) -> Vec<TracedLayer> {
    let (w, h, scale, px) = sample_input(pm);
    let centres = palette(&px, s.colors);
    if centres.is_empty() {
        return Vec::new();
    }
    let mut labels: Vec<i32> = px.iter().map(|p| p.map_or(-1, |c| nearest(&centres, c) as i32)).collect();
    despeckle(&mut labels, w, h, s.speckle.max(1));
    // biggest colour at the bottom; every layer also covers the layers above it, so
    // anti-aliased seams between neighbours never let the background through
    let mut count = vec![0usize; centres.len()];
    for &l in &labels {
        if l >= 0 {
            count[l as usize] += 1;
        }
    }
    let mut order: Vec<usize> = (0..centres.len()).filter(|&i| count[i] > 0).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(count[i]));
    let mut rank = vec![usize::MAX; centres.len()];
    for (r, &i) in order.iter().enumerate() {
        rank[i] = r;
    }
    let min_loop = (s.speckle.max(1) as f32).max(2.0);
    let mut layers = Vec::new();
    for (r, &ci) in order.iter().enumerate() {
        let mask: Vec<bool> = labels.iter().map(|&l| l >= 0 && rank[l as usize] >= r).collect();
        let mut contours = Vec::new();
        for lp in trace_loops(&mask, w, h) {
            let raw: Vec<Pt> = lp.iter().map(|p| Pt::new(p.0 as f32, p.1 as f32)).collect();
            if area(&raw).abs() < min_loop {
                continue;
            }
            if let Some(c) = refine(&lp, s, scale) {
                contours.push(c);
            }
        }
        if !contours.is_empty() {
            let c = lab_to_srgb(centres[ci]);
            layers.push(TracedLayer {
                color: [c[0].round() as u8, c[1].round() as u8, c[2].round() as u8, 255],
                contours,
            });
        }
    }
    layers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture() -> Pixmap {
        // white background, red disc with a blue square hole-ish patch inside it
        let mut pm = Pixmap::new(120, 100).unwrap();
        pm.fill(tiny_skia::Color::WHITE);
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(220, 30, 30, 255);
        let disc = tiny_skia::PathBuilder::from_circle(60.0, 50.0, 40.0).unwrap();
        pm.fill_path(&disc, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
        paint.set_color_rgba8(20, 40, 200, 255);
        pm.fill_rect(tiny_skia::Rect::from_xywh(45.0, 35.0, 30.0, 30.0).unwrap(), &paint, tiny_skia::Transform::identity(), None);
        pm
    }

    #[test]
    fn loops_follow_pixel_edges_with_holes_reversed() {
        // a 4x4 ring: outer loop clockwise, hole anticlockwise
        let mut m = vec![true; 16];
        m[5] = false;
        m[6] = false;
        m[9] = false;
        m[10] = false;
        let loops = trace_loops(&m, 4, 4);
        assert_eq!(loops.len(), 2);
        let areas: Vec<f32> = loops
            .iter()
            .map(|l| area(&l.iter().map(|p| Pt::new(p.0 as f32, p.1 as f32)).collect::<Vec<_>>()))
            .collect();
        assert!(areas.contains(&16.0) && areas.contains(&-4.0), "{areas:?}");
    }

    #[test]
    fn diagonal_pixels_trace_as_separate_squares() {
        let m = vec![true, false, false, true];
        let loops = trace_loops(&m, 2, 2);
        assert_eq!(loops.len(), 2);
        assert!(loops.iter().all(|l| l.len() == 4));
    }

    #[test]
    fn three_colours_become_three_stacked_layers() {
        let layers = trace(&picture(), &TraceSettings { colors: 3, ..Default::default() });
        assert_eq!(layers.len(), 3);
        // biggest (white background) at the bottom
        assert!(layers[0].color[1] > 200, "{:?}", layers[0].color);
        let red = layers.iter().find(|l| l.color[0] > 180 && l.color[1] < 80).expect("a red layer");
        let blue = layers.iter().find(|l| l.color[2] > 160 && l.color[0] < 80).expect("a blue layer");
        // the stacked red layer is a solid disc (it also covers the blue square above it)
        assert_eq!(red.contours.len(), 1);
        let a = area(&red.contours[0]).abs();
        assert!((a - std::f32::consts::PI * 1600.0).abs() < 250.0, "disc area {a}");
        let b = area(&blue.contours[0]).abs();
        assert!((b - 900.0).abs() < 120.0, "square area {b}");
        assert!(red.contours[0].len() < 60, "simplified to {} points", red.contours[0].len());
    }

    #[test]
    fn transparent_pixels_stay_empty() {
        let mut pm = Pixmap::new(50, 50).unwrap();
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(0, 0, 0, 255);
        pm.fill_rect(tiny_skia::Rect::from_xywh(10.0, 10.0, 20.0, 20.0).unwrap(), &paint, tiny_skia::Transform::identity(), None);
        let layers = trace(&pm, &TraceSettings { colors: 4, ..Default::default() });
        assert_eq!(layers.len(), 1);
        let a = area(&layers[0].contours[0]).abs();
        assert!((a - 400.0).abs() < 40.0, "{a}");
    }
}
