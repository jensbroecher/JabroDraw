//! NPR (non-photorealistic rendering) "looks" applied to the finished picture:
//! watercolor, oil, impressionist strokes, pointillism, pencil, colored pencil, comic ink.
//! Runs on a background worker thread so the UI stays responsive.

use crate::doc::{NprSettings, NprStyle, Rgba};
use crate::raster::gaussian;
use crate::textures::{fbm, rand01, smoothstep, value_noise, Rng};
use std::sync::mpsc::{channel, Receiver, Sender};
use tiny_skia::{Color, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

#[derive(Clone)]
pub struct Img {
    pub w: usize,
    pub h: usize,
    pub p: [Vec<f32>; 3],
}

impl Img {
    pub fn from_pixmap(pm: &Pixmap) -> Img {
        let w = pm.width() as usize;
        let h = pm.height() as usize;
        let d = pm.data();
        let n = w * h;
        let mut p = [vec![0.0; n], vec![0.0; n], vec![0.0; n]];
        for i in 0..n {
            let a = d[i * 4 + 3] as f32 / 255.0;
            for (c, plane) in p.iter_mut().enumerate() {
                // composite over white paper
                plane[i] = d[i * 4 + c] as f32 / 255.0 + (1.0 - a);
            }
        }
        Img { w, h, p }
    }

    pub fn to_pixmap(&self) -> Pixmap {
        let mut pm = Pixmap::new(self.w as u32, self.h as u32).expect("size");
        let d = pm.data_mut();
        for i in 0..self.w * self.h {
            for c in 0..3 {
                d[i * 4 + c] = (self.p[c][i].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
            d[i * 4 + 3] = 255;
        }
        pm
    }

    pub fn lum(&self) -> Vec<f32> {
        (0..self.w * self.h)
            .map(|i| 0.299 * self.p[0][i] + 0.587 * self.p[1][i] + 0.114 * self.p[2][i])
            .collect()
    }

    pub fn blurred(&self, sigma: f32) -> Img {
        let mut o = self.clone();
        for plane in o.p.iter_mut() {
            gaussian(plane, self.w, self.h, sigma, true);
        }
        o
    }

    #[inline]
    pub fn at(&self, x: usize, y: usize) -> [f32; 3] {
        let i = y.min(self.h - 1) * self.w + x.min(self.w - 1);
        [self.p[0][i], self.p[1][i], self.p[2][i]]
    }

    pub fn sample(&self, x: f32, y: f32) -> [f32; 3] {
        let x = x.clamp(0.0, self.w as f32 - 1.001);
        let y = y.clamp(0.0, self.h as f32 - 1.001);
        let (x0, y0) = (x as usize, y as usize);
        let (tx, ty) = (x - x0 as f32, y - y0 as f32);
        let a = self.at(x0, y0);
        let b = self.at(x0 + 1, y0);
        let c = self.at(x0, y0 + 1);
        let d = self.at(x0 + 1, y0 + 1);
        let mut o = [0.0; 3];
        for k in 0..3 {
            o[k] = (a[k] * (1.0 - tx) + b[k] * tx) * (1.0 - ty) + (c[k] * (1.0 - tx) + d[k] * tx) * ty;
        }
        o
    }
}

fn dist3(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn saturate(img: &mut Img, k: f32) {
    for i in 0..img.w * img.h {
        let l = 0.299 * img.p[0][i] + 0.587 * img.p[1][i] + 0.114 * img.p[2][i];
        for c in 0..3 {
            img.p[c][i] = (l + (img.p[c][i] - l) * k).clamp(0.0, 1.0);
        }
    }
}

fn paper_grain(w: usize, h: usize, seed: u32, k: f32) -> Vec<f32> {
    let mut g = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let (xf, yf) = (x as f32, y as f32);
            g.push(0.6 * fbm(xf / (2.2 * k), yf / (2.2 * k), seed, 2) + 0.4 * fbm(xf / (12.0 * k), yf / (12.0 * k), seed + 1, 3));
        }
    }
    g
}

fn integral(src: &[f32], w: usize, h: usize) -> Vec<f64> {
    let s = w + 1;
    let mut ii = vec![0f64; s * (h + 1)];
    for y in 0..h {
        let mut row = 0f64;
        for x in 0..w {
            row += src[y * w + x] as f64;
            ii[(y + 1) * s + x + 1] = ii[y * s + x + 1] + row;
        }
    }
    ii
}

#[inline]
fn rsum(ii: &[f64], w: usize, x0: usize, y0: usize, x1: usize, y1: usize) -> f64 {
    let s = w + 1;
    ii[y1 * s + x1] - ii[y0 * s + x1] - ii[y1 * s + x0] + ii[y0 * s + x0]
}

/// Kuwahara filter (O(1) per pixel via integral images) — flattens areas while keeping edges.
pub fn kuwahara(img: &Img, r: usize) -> Img {
    let (w, h) = (img.w, img.h);
    let lum = img.lum();
    let lum2: Vec<f32> = lum.iter().map(|v| v * v).collect();
    let il = integral(&lum, w, h);
    let il2 = integral(&lum2, w, h);
    let ic = [
        integral(&img.p[0], w, h),
        integral(&img.p[1], w, h),
        integral(&img.p[2], w, h),
    ];
    let mut out = img.clone();
    for y in 0..h {
        for x in 0..w {
            let xa = x.saturating_sub(r);
            let xb = (x + r + 1).min(w);
            let ya = y.saturating_sub(r);
            let yb = (y + r + 1).min(h);
            let quads = [
                (xa, x + 1, ya, y + 1),
                (x, xb, ya, y + 1),
                (xa, x + 1, y, yb),
                (x, xb, y, yb),
            ];
            let mut best = f64::MAX;
            let mut bi = 0;
            for (qi, &(x0, x1, y0, y1)) in quads.iter().enumerate() {
                let n = ((x1 - x0) * (y1 - y0)) as f64;
                let m = rsum(&il, w, x0, y0, x1, y1) / n;
                let v = rsum(&il2, w, x0, y0, x1, y1) / n - m * m;
                if v < best {
                    best = v;
                    bi = qi;
                }
            }
            let (x0, x1, y0, y1) = quads[bi];
            let n = ((x1 - x0) * (y1 - y0)) as f64;
            for c in 0..3 {
                out.p[c][y * w + x] = (rsum(&ic[c], w, x0, y0, x1, y1) / n) as f32;
            }
        }
    }
    out
}

fn sobel(l: &[f32], w: usize, h: usize) -> (Vec<f32>, Vec<f32>) {
    let mut gx = vec![0.0; w * h];
    let mut gy = vec![0.0; w * h];
    let at = |x: isize, y: isize| -> f32 {
        let xc = x.clamp(0, w as isize - 1) as usize;
        let yc = y.clamp(0, h as isize - 1) as usize;
        l[yc * w + xc]
    };
    for y in 0..h as isize {
        for x in 0..w as isize {
            let i = y as usize * w + x as usize;
            gx[i] = (at(x + 1, y - 1) + 2.0 * at(x + 1, y) + at(x + 1, y + 1))
                - (at(x - 1, y - 1) + 2.0 * at(x - 1, y) + at(x - 1, y + 1));
            gy[i] = (at(x - 1, y + 1) + 2.0 * at(x, y + 1) + at(x + 1, y + 1))
                - (at(x - 1, y - 1) + 2.0 * at(x, y - 1) + at(x + 1, y - 1));
        }
    }
    (gx, gy)
}

/// Smoothed edge-tangent field from the structure tensor (falls back to a noise flow).
fn tangent_field(l: &[f32], w: usize, h: usize, sigma: f32, seed: u32) -> (Vec<f32>, Vec<f32>) {
    let (gx, gy) = sobel(l, w, h);
    let mut e: Vec<f32> = gx.iter().map(|v| v * v).collect();
    let mut f: Vec<f32> = gx.iter().zip(&gy).map(|(a, b)| a * b).collect();
    let mut g: Vec<f32> = gy.iter().map(|v| v * v).collect();
    gaussian(&mut e, w, h, sigma, true);
    gaussian(&mut f, w, h, sigma, true);
    gaussian(&mut g, w, h, sigma, true);
    let mut tx = vec![0.0; w * h];
    let mut ty = vec![0.0; w * h];
    for i in 0..w * h {
        let strength = ((e[i] - g[i]).powi(2) + 4.0 * f[i] * f[i]).sqrt();
        let (x, y) = ((i % w) as f32, (i / w) as f32);
        let flow = fbm(x / 160.0, y / 160.0, seed + 31, 2) * std::f32::consts::TAU * 1.5;
        let a = 0.5 * (2.0 * f[i]).atan2(e[i] - g[i]);
        let k = (strength * 20.0).min(1.0);
        let (ax, ay) = (-a.sin(), a.cos());
        let (bx, by) = (flow.cos(), flow.sin());
        let (bx, by) = if ax * bx + ay * by < 0.0 { (-bx, -by) } else { (bx, by) };
        let (vx, vy) = (ax * k + bx * (1.0 - k), ay * k + by * (1.0 - k));
        let len = (vx * vx + vy * vy).sqrt().max(1e-6);
        tx[i] = vx / len;
        ty[i] = vy / len;
    }
    (tx, ty)
}

/// Line integral convolution of white noise along the tangent field: brush-stroke streaks.
fn lic(noise: &[f32], tx: &[f32], ty: &[f32], w: usize, h: usize, len: usize) -> Vec<f32> {
    let mut out = vec![0.0; w * h];
    for py in 0..h {
        for px in 0..w {
            let i0 = py * w + px;
            let mut sum = noise[i0];
            let mut count = 1.0;
            for dir in [1.0f32, -1.0] {
                let (mut x, mut y) = (px as f32 + 0.5, py as f32 + 0.5);
                let (mut pdx, mut pdy) = (tx[i0] * dir, ty[i0] * dir);
                for _ in 0..len {
                    x += pdx;
                    y += pdy;
                    if x < 0.0 || y < 0.0 || x >= w as f32 || y >= h as f32 {
                        break;
                    }
                    let i = y as usize * w + x as usize;
                    let (mut dx, mut dy) = (tx[i], ty[i]);
                    if dx * pdx + dy * pdy < 0.0 {
                        dx = -dx;
                        dy = -dy;
                    }
                    sum += noise[i];
                    count += 1.0;
                    pdx = dx;
                    pdy = dy;
                }
            }
            out[i0] = sum / count;
        }
    }
    out
}

fn watercolor(img: &Img, s: &NprSettings) -> Img {
    let k = s.px_scale.max(0.1);
    let (w, h) = (img.w, img.h);
    let seed = s.seed;
    // 1. wobbly, hand-made boundaries
    let amp = (2.0 + 4.0 * s.brush) * k;
    let mut wob = img.clone();
    for y in 0..h {
        for x in 0..w {
            let (xf, yf) = (x as f32, y as f32);
            let dx = (fbm(xf / (38.0 * k), yf / (38.0 * k), seed, 3) - 0.5) * 2.0 * amp;
            let dy = (fbm(xf / (38.0 * k), yf / (38.0 * k), seed + 7, 3) - 0.5) * 2.0 * amp;
            let c = img.sample(xf + dx, yf + dy);
            for k in 0..3 {
                wob.p[k][y * w + x] = c[k];
            }
        }
    }
    // 2. flatten into washes
    let r = ((2.0 + s.brush * 6.0) * k).max(1.0) as usize;
    let abs = kuwahara(&wob, r).blurred(k);
    // 3. pigment pooling at wash edges
    let mut l = abs.lum();
    gaussian(&mut l, w, h, 1.2 * k, true);
    let (gx, gy) = sobel(&l, w, h);
    let grain = paper_grain(w, h, seed + 11, k);
    let mut out = abs.clone();
    for i in 0..w * h {
        let (x, y) = ((i % w) as f32, (i / w) as f32);
        let edge = ((gx[i] * gx[i] + gy[i] * gy[i]).sqrt() * 1.6).min(1.0);
        let turb = fbm(x / (70.0 * k), y / (70.0 * k), seed + 3, 4) - 0.5;
        let gr = grain[i] - 0.5;
        let density = (1.0 + 0.9 * edge + 0.45 * turb + 0.6 * gr * s.paper).max(0.3);
        for c in 0..3 {
            let v = abs.p[c][i] * 0.9 + 0.1; // transparent pigment, paper shows through
            let v = v - (v - v * v) * (density - 1.0);
            out.p[c][i] = (v * (1.0 - s.paper * 0.06 * (1.0 - grain[i]))).clamp(0.0, 1.0);
        }
    }
    out
}

fn oil(img: &Img, s: &NprSettings) -> Img {
    let k = s.px_scale.max(0.1);
    let (w, h) = (img.w, img.h);
    let r = ((2.0 + s.brush * 7.0) * k).max(1.0) as usize;
    let mut kw = kuwahara(img, r);
    kw = kuwahara(&kw, (r / 2).max(1));
    saturate(&mut kw, 1.15);
    let lum = kw.lum();
    let (tx, ty) = tangent_field(&lum, w, h, (3.0 + s.brush * 3.0) * k, s.seed);
    let noise: Vec<f32> = (0..w * h)
        .map(|i| rand01((i % w) as i32, (i / w) as i32, s.seed))
        .collect();
    let len = ((4.0 + s.brush * 10.0) * k) as usize;
    let streak = lic(&noise, &tx, &ty, w, h, len);
    let gain = ((2 * len + 1) as f32).sqrt() * 0.6;
    let height: Vec<f32> = streak
        .iter()
        .zip(&lum)
        .map(|(s, l)| 0.5 + (s - 0.5) * gain + l * 0.3)
        .collect();
    let mut out = kw.clone();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let hx = height[y * w + (x + 1).min(w - 1)] - height[y * w + x.saturating_sub(1)];
            let hy = height[(y + 1).min(h - 1) * w + x] - height[y.saturating_sub(1) * w + x];
            let shade = (hx + hy) * 0.707 * 0.9;
            let spec = (shade - 0.14).max(0.0) * 0.5;
            let weave = 0.5 + 0.25 * ((x as f32 * 1.3 / k).sin() + (y as f32 * 1.3 / k).sin());
            for c in 0..3 {
                let v = kw.p[c][i] * (1.0 + shade) + spec;
                out.p[c][i] = (v * (1.0 - s.paper * 0.12 * weave)).clamp(0.0, 1.0);
            }
        }
    }
    out
}

fn painterly(src: &Img, s: &NprSettings, dots: bool) -> Img {
    let k = s.px_scale.max(0.1);
    let (w, h) = (src.w, src.h);
    let mut canvas = Pixmap::new(w as u32, h as u32).expect("size");
    canvas.fill(Color::from_rgba8(250, 246, 236, 255));
    let big = if dots { 3.0 + s.brush * 9.0 } else { 4.0 + s.brush * 18.0 } * k;
    let radii: Vec<f32> = [big, big * 0.5, big * 0.25]
        .into_iter()
        .filter(|r| *r >= 1.5)
        .collect();
    let mut rng = Rng::new(s.seed as u64 * 7 + 3);
    let max_len = 4 + (s.brush * 12.0) as usize;
    for (li, &r) in radii.iter().enumerate() {
        let reference = src.blurred((r * 0.5).max(0.6));
        let rl = reference.lum();
        let (gx, gy) = sobel(&rl, w, h);
        let cur = Img::from_pixmap(&canvas);
        let grid = r.max(1.0) as usize;
        let threshold = if li == 0 { -1.0 } else { 0.05 + 0.02 * li as f32 };
        let mut strokes: Vec<(Vec<(f32, f32)>, [f32; 3])> = Vec::new();
        for gy0 in (0..h).step_by(grid) {
            for gx0 in (0..w).step_by(grid) {
                let jx = (gx0 as f32 + rng.f() * grid as f32).min(w as f32 - 1.0);
                let jy = (gy0 as f32 + rng.f() * grid as f32).min(h as f32 - 1.0);
                let (xi, yi) = (jx as usize, jy as usize);
                let rc = reference.at(xi, yi);
                if dist3(rc, cur.at(xi, yi)) <= threshold {
                    continue;
                }
                let pts = if dots {
                    vec![(jx, jy)]
                } else {
                    let mut pts = vec![(jx, jy)];
                    let (mut x, mut y) = (jx, jy);
                    let (mut lx, mut ly) = (0.0f32, 0.0f32);
                    for i in 1..max_len {
                        let (xi, yi) = (x as usize, y as usize);
                        if i > 2 {
                            let here = reference.at(xi, yi);
                            if dist3(here, cur.at(xi, yi)) < dist3(here, rc) {
                                break;
                            }
                        }
                        let idx = yi.min(h - 1) * w + xi.min(w - 1);
                        let (g1, g2) = (gx[idx], gy[idx]);
                        let mag = (g1 * g1 + g2 * g2).sqrt();
                        let (mut dx, mut dy) = if mag < 0.02 {
                            let a = fbm(x / (140.0 * k), y / (140.0 * k), s.seed + 5, 2) * std::f32::consts::TAU * 1.5;
                            (a.cos(), a.sin())
                        } else {
                            (-g2 / mag, g1 / mag)
                        };
                        if lx * dx + ly * dy < 0.0 {
                            dx = -dx;
                            dy = -dy;
                        }
                        if i > 1 {
                            dx = 0.6 * dx + 0.4 * lx;
                            dy = 0.6 * dy + 0.4 * ly;
                            let l = (dx * dx + dy * dy).sqrt().max(1e-6);
                            dx /= l;
                            dy /= l;
                        }
                        x += r * dx;
                        y += r * dy;
                        if x < 0.0 || y < 0.0 || x >= w as f32 || y >= h as f32 {
                            break;
                        }
                        if dist3(reference.at(x as usize, y as usize), rc) > 0.22 {
                            break;
                        }
                        pts.push((x, y));
                        lx = dx;
                        ly = dy;
                    }
                    pts
                };
                let jit = if dots { 0.12 } else { 0.05 };
                let mut col = rc;
                let v = (rng.f() - 0.5) * jit;
                for c in col.iter_mut() {
                    *c = (*c + v + (rng.f() - 0.5) * jit * 0.6).clamp(0.0, 1.0);
                }
                if dots {
                    let l = 0.299 * col[0] + 0.587 * col[1] + 0.114 * col[2];
                    for c in col.iter_mut() {
                        *c = (l + (*c - l) * 1.35).clamp(0.0, 1.0);
                    }
                }
                strokes.push((pts, col));
            }
        }
        // shuffle
        for i in (1..strokes.len()).rev() {
            let j = (rng.next_u32() as usize) % (i + 1);
            strokes.swap(i, j);
        }
        let width = if dots { r * 1.1 } else { r * 1.7 };
        let stroke = Stroke {
            width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Default::default()
        };
        for (pts, col) in strokes {
            let mut pb = PathBuilder::new();
            pb.move_to(pts[0].0, pts[0].1);
            if pts.len() == 1 {
                pb.line_to(pts[0].0 + 0.05, pts[0].1);
            } else {
                for p in &pts[1..] {
                    pb.line_to(p.0, p.1);
                }
            }
            if let Some(path) = pb.finish() {
                let mut paint = Paint {
                    anti_alias: true,
                    ..Default::default()
                };
                paint.set_color(Color::from_rgba(col[0], col[1], col[2], 0.93).unwrap_or(Color::BLACK));
                canvas.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
        }
    }
    let mut out = Img::from_pixmap(&canvas);
    if s.paper > 0.0 {
        for y in 0..h {
            for x in 0..w {
                let weave = 0.5 + 0.25 * ((x as f32 * 1.3 / k).sin() + (y as f32 * 1.3 / k).sin());
                let i = y * w + x;
                for c in 0..3 {
                    out.p[c][i] *= 1.0 - s.paper * 0.1 * weave;
                }
            }
        }
    }
    out
}

fn pencil(img: &Img, s: &NprSettings, colored: bool) -> Img {
    let k = s.px_scale.max(0.1);
    let (w, h) = (img.w, img.h);
    let lum = img.lum();
    let mut inv: Vec<f32> = lum.iter().map(|l| 1.0 - l).collect();
    gaussian(&mut inv, w, h, (1.0 + s.brush * 4.0) * k, true);
    let mut tone = lum.clone();
    gaussian(&mut tone, w, h, 2.0 * k, true);
    let mut soft = lum.clone();
    gaussian(&mut soft, w, h, (0.8 + s.brush) * k, true);
    let (ex, ey) = sobel(&soft, w, h);
    let grain = paper_grain(w, h, s.seed + 21, k);
    let mut sat = img.clone();
    saturate(&mut sat, 1.3);
    let mut out = img.clone();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let edge = ((ex[i] * ex[i] + ey[i] * ey[i]).sqrt() * 1.8).min(1.0);
            let sketch = (lum[i] / (1.0 - inv[i]).max(0.02)).min(1.0).powf(2.2) * (1.0 - 0.55 * edge);
            // only real shadows get hatched, light mid-tones stay clean paper
            let dark = smoothstep(0.22, 1.0, 1.0 - tone[i]);
            let (xf, yf) = (x as f32, y as f32);
            let u = (xf + yf) * 0.7071;
            let v = (xf - yf) * 0.7071;
            let h1 = value_noise(v * 0.7 / k, u * 0.035 / k, s.seed + 5);
            let h2 = value_noise(u * 0.7 / k, v * 0.035 / k, s.seed + 9);
            let c1 = smoothstep(-0.08, 0.08, dark * 1.25 - h1);
            let c2 = smoothstep(-0.08, 0.08, dark * 1.25 - 0.45 - h2);
            let g = 0.55 + 0.45 * grain[i];
            if colored {
                let cover = ((0.3 + 0.7 * c1) * g).min(1.0);
                let line = 0.35 + 0.65 * sketch;
                for c in 0..3 {
                    let tint = 1.0 + (sat.p[c][i] - 1.0) * cover * 0.9;
                    out.p[c][i] = (tint * line * (1.0 - 0.25 * c2 * dark)).clamp(0.0, 1.0);
                }
            } else {
                let shade = 1.0 - 0.45 * c1 * dark.sqrt() - 0.3 * c2;
                let val = sketch * shade;
                let dk = (1.0 - val) * g;
                let v = 1.0 - dk;
                out.p[0][i] = v * 0.97;
                out.p[1][i] = v * 0.97;
                out.p[2][i] = v * 0.99;
            }
        }
    }
    out
}

fn comic(img: &Img, s: &NprSettings) -> Img {
    let k = s.px_scale.max(0.1);
    let (w, h) = (img.w, img.h);
    let mut kw = kuwahara(img, ((2.0 + s.brush * 4.0) * k) as usize);
    saturate(&mut kw, 1.25);
    let lum = kw.lum();
    let levels = 3.0 + (1.0 - s.brush) * 3.0;
    let sigma = (0.8 + s.brush * 1.5) * k;
    let base = img.lum();
    let mut g1 = base.clone();
    gaussian(&mut g1, w, h, sigma, true);
    let mut g2 = base;
    gaussian(&mut g2, w, h, sigma * 1.6, true);
    let mut out = kw.clone();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let l = lum[i].max(0.001);
            let q = ((l * levels).round() / levels).max(0.06);
            let f = q / l;
            let d = g1[i] - 0.985 * g2[i];
            let ink = if d >= 0.0 { 1.0 } else { (1.0 + (70.0 * d).tanh()).clamp(0.0, 1.0) };
            // halftone dots in the shadows
            let mut tone = 1.0;
            if s.paper > 0.0 && l < 0.55 {
                let (xf, yf) = (x as f32, y as f32);
                let u = (xf + yf) * 0.7071 / (6.0 * k);
                let v = (xf - yf) * 0.7071 / (6.0 * k);
                let du = u - u.floor() - 0.5;
                let dv = v - v.floor() - 0.5;
                let rr = (du * du + dv * dv).sqrt();
                if rr < (0.55 - l) * 0.9 {
                    tone = 1.0 - 0.35 * s.paper;
                }
            }
            for c in 0..3 {
                out.p[c][i] = (kw.p[c][i] * f * ink * tone).clamp(0.0, 1.0);
            }
        }
    }
    out
}

pub fn apply(src: &Pixmap, s: &NprSettings) -> Pixmap {
    if s.style == NprStyle::None || s.strength <= 0.0 || src.width() < 4 || src.height() < 4 {
        return src.clone();
    }
    let img = Img::from_pixmap(src);
    let styled = match s.style {
        NprStyle::None => img.clone(),
        NprStyle::Watercolor => watercolor(&img, s),
        NprStyle::OilPaint => oil(&img, s),
        NprStyle::Impressionist => painterly(&img, s, false),
        NprStyle::Pointillism => painterly(&img, s, true),
        NprStyle::Pencil => pencil(&img, s, false),
        NprStyle::ColorPencil => pencil(&img, s, true),
        NprStyle::ComicInk => comic(&img, s),
    };
    let t = s.strength.clamp(0.0, 1.0);
    if t >= 0.999 {
        return styled.to_pixmap();
    }
    let mut mixed = styled;
    for c in 0..3 {
        for (m, o) in mixed.p[c].iter_mut().zip(&img.p[c]) {
            *m = o + (*m - o) * t;
        }
    }
    mixed.to_pixmap()
}

/// Consecutive objects (bottom to top) that are painted with the same looks.
pub struct LookRun {
    /// The run's objects on a transparent, canvas-sized layer.
    pub layer: Pixmap,
    pub looks: Vec<NprSettings>,
    /// The bottom run in the document's look: it also paints the page (paper) itself.
    pub base: bool,
}

/// Paint every run with its looks and stack them. Outside a run (plus a little spill, like
/// real paint) the picture underneath stays as it was, so a clean logo can sit on a watercolor
/// and a pencil-sketched object can sit on a clean photo.
pub fn compose(runs: &[LookRun], w: u32, h: u32, bg: Rgba, px_scale: f32) -> Pixmap {
    let mut canvas = Pixmap::new(w.max(1), h.max(1)).expect("canvas");
    canvas.fill(Color::from_rgba8(bg[0], bg[1], bg[2], 255));
    let over = tiny_skia::PixmapPaint::default();
    for run in runs {
        let mut with = canvas.clone();
        with.draw_pixmap(0, 0, run.layer.as_ref(), &over, Transform::identity(), None);
        if run.looks.is_empty() {
            canvas = with;
            continue;
        }
        let mut styled = with.clone();
        for look in &run.looks {
            let mut l = look.clone();
            l.px_scale = px_scale;
            styled = apply(&styled, &l);
        }
        if run.base {
            canvas = styled;
            continue;
        }
        let brush = run.looks.iter().map(|l| l.brush).fold(0.0, f32::max);
        let spread = (2.0 + 6.0 * brush) * px_scale;
        let (wu, hu) = (w as usize, h as usize);
        let alpha = crate::raster::alpha_of(&run.layer);
        let mut m = alpha.clone();
        gaussian(&mut m, wu, hu, spread, false);
        let out = with.data().to_vec();
        let st = styled.data();
        let dst = canvas.data_mut();
        for i in 0..wu * hu {
            let k = (m[i] * 2.5).min(1.0).max(alpha[i]);
            for c in 0..4 {
                let p = i * 4 + c;
                dst[p] = (out[p] as f32 * (1.0 - k) + st[p] as f32 * k + 0.5) as u8;
            }
        }
    }
    canvas
}

pub struct NprJob {
    pub gen: u64,
    pub runs: Vec<LookRun>,
    pub w: u32,
    pub h: u32,
    pub bg: Rgba,
}

pub struct NprResult {
    pub gen: u64,
    pub img: Pixmap,
}

/// Background thread that always works on the newest submitted picture.
pub struct NprWorker {
    tx: Sender<NprJob>,
    pub rx: Receiver<NprResult>,
}

impl NprWorker {
    pub fn new(on_done: impl Fn() + Send + 'static) -> Self {
        let (jtx, jrx) = channel::<NprJob>();
        let (rtx, rrx) = channel::<NprResult>();
        std::thread::spawn(move || {
            while let Ok(mut job) = jrx.recv() {
                while let Ok(newer) = jrx.try_recv() {
                    job = newer;
                }
                let img = compose(&job.runs, job.w, job.h, job.bg, 1.0);
                if rtx.send(NprResult { gen: job.gen, img }).is_err() {
                    break;
                }
                on_done();
            }
        });
        Self { tx: jtx, rx: rrx }
    }

    pub fn submit(&self, job: NprJob) {
        let _ = self.tx.send(job);
    }
}
