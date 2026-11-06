//! More NPR looks, the print and craft styles: linocut, risograph, pop-art halftone, engraving,
//! gouache, pastel, stained glass, sumi-e ink wash, paper cut-out, chalkboard and blueprint.
//!
//! Each takes the finished picture (`Img`, composited over white) and the look settings:
//! `brush` sets the size of the marks, `paper` how much the paper (or board, or glass) shows,
//! and `seed` varies the marks. The helpers they share with the painterly looks live in `npr`.

use crate::doc::NprSettings;
use crate::npr::{dist3, kuwahara, lic, paper_grain, saturate, sobel, tangent_field, Img};
use crate::raster::gaussian;
use crate::textures::{fbm, rand01, smoothstep, value_noise, Rng};
use std::f32::consts::TAU;

#[inline]
fn frac(v: f32) -> f32 {
    v - v.floor()
}

#[inline]
fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

#[inline]
fn lum3(c: [f32; 3]) -> f32 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}

/// Ink `col` printed with coverage `a` onto `base` (it multiplies, like real ink).
#[inline]
fn print(base: &mut [f32; 3], col: [f32; 3], a: f32) {
    for c in 0..3 {
        base[c] *= 1.0 - a * (1.0 - col[c]);
    }
}

/// Paper of colour `col` with its grain showing by `amount`.
#[inline]
fn paper_at(col: [f32; 3], grain: f32, amount: f32) -> [f32; 3] {
    let f = 1.0 - amount * (1.0 - grain);
    [col[0] * f, col[1] * f, col[2] * f]
}

/// The picture sampled through a wobbly displacement: hand-cut, hand-drawn edges.
fn wobble(img: &Img, amp: f32, scale: f32, seed: u32) -> Img {
    if amp < 0.15 {
        return img.clone();
    }
    let (w, h) = (img.w, img.h);
    let mut out = img.clone();
    for y in 0..h {
        for x in 0..w {
            let (xf, yf) = (x as f32, y as f32);
            let dx = (fbm(xf / scale, yf / scale, seed, 3) - 0.5) * 2.0 * amp;
            let dy = (fbm(xf / scale, yf / scale, seed + 7, 3) - 0.5) * 2.0 * amp;
            let c = img.sample(xf + dx, yf + dy);
            for k in 0..3 {
                out.p[k][y * w + x] = c[k];
            }
        }
    }
    out
}

/// Streaks that follow the forms of `lum`, `len` pixels long: about 0..1, 0.5 on average.
fn streaks(lum: &[f32], w: usize, h: usize, sigma: f32, len: usize, seed: u32) -> Vec<f32> {
    let (tx, ty) = tangent_field(lum, w, h, sigma, seed);
    let noise: Vec<f32> = (0..w * h).map(|i| rand01((i % w) as i32, (i / w) as i32, seed + 17)).collect();
    let len = len.max(1);
    let s = lic(&noise, &tx, &ty, w, h, len);
    let gain = ((2 * len + 1) as f32).sqrt() * 0.9;
    s.iter().map(|v| 0.5 + (v - 0.5) * gain).collect()
}

/// Edge strength 0..1 of a tone map.
fn edges(tone: &[f32], w: usize, h: usize, gain: f32) -> Vec<f32> {
    let (gx, gy) = sobel(tone, w, h);
    gx.iter().zip(&gy).map(|(a, b)| ((a * a + b * b).sqrt() * gain).min(1.0)).collect()
}

/// Thin ink outlines (difference of Gaussians): 0 = none, 1 = full ink.
fn outlines(lum: &[f32], w: usize, h: usize, sigma: f32, strength: f32) -> Vec<f32> {
    let mut g1 = lum.to_vec();
    gaussian(&mut g1, w, h, sigma, true);
    let mut g2 = lum.to_vec();
    gaussian(&mut g2, w, h, sigma * 1.6, true);
    g1.iter()
        .zip(&g2)
        .map(|(a, b)| {
            let d = a - 0.985 * b;
            if d >= 0.0 { 0.0 } else { (-(strength * d)).tanh().clamp(0.0, 1.0) }
        })
        .collect()
}

/// Black ink cut from a block: dark areas stay inked, mid-tones become gouged lines, light
/// areas are cut away. A second flat colour sits under the black, like a reduction print.
pub fn linocut(img: &Img, s: &NprSettings) -> Img {
    let k = s.px_scale.max(0.1);
    let (w, h) = (img.w, img.h);
    let r = ((1.5 + s.brush * 4.0) * k).max(1.0) as usize;
    let flat = kuwahara(&wobble(img, (1.0 + 2.0 * s.brush) * k, 30.0 * k, s.seed), r);
    let mut tone = flat.lum();
    gaussian(&mut tone, w, h, 0.8 * k, true);
    let carve = streaks(&tone, w, h, (2.5 + 2.0 * s.brush) * k, ((5.0 + 12.0 * s.brush) * k) as usize, s.seed);
    let edge = edges(&tone, w, h, 2.2);
    let grain = paper_grain(w, h, s.seed + 23, k);
    let ink = [0.09, 0.07, 0.08];
    let mut out = img.clone();
    for i in 0..w * h {
        let dark = 1.0 - tone[i];
        let mut a = smoothstep(-0.06, 0.06, (dark - 0.15) * 1.3 - carve[i]);
        a = a.max(edge[i] * smoothstep(0.05, 0.3, dark));
        let c = flat.at(i % w, i / w);
        let mx = c[0].max(c[1]).max(c[2]).max(1e-3);
        let sat = (mx - c[0].min(c[1]).min(c[2])) / mx;
        let l = lum3(c).max(0.05);
        let block = c.map(|v| ((v / l * 0.55).clamp(0.0, 1.0) * 3.0).round() / 3.0);
        let skip = smoothstep(0.62, 0.9, grain[i]) * s.paper;
        let bk = smoothstep(0.1, 0.32, dark) * smoothstep(0.08, 0.25, sat) * (1.0 - 0.35 * skip);
        let a = a * (1.0 - 0.45 * skip);
        let mut px = paper_at([0.96, 0.93, 0.85], grain[i], 0.08 * s.paper);
        print(&mut px, block, bk);
        print(&mut px, ink, a);
        for ch in 0..3 {
            out.p[ch][i] = px[ch].clamp(0.0, 1.0);
        }
    }
    out
}

/// Three grainy ink layers (blue, fluorescent pink, yellow) printed slightly out of register.
pub fn risograph(img: &Img, s: &NprSettings) -> Img {
    let k = s.px_scale.max(0.1);
    let (w, h) = (img.w, img.h);
    let flat = kuwahara(img, ((1.0 + 2.0 * s.brush) * k).max(1.0) as usize).blurred(0.6 * k);
    let inks = [[0.0, 0.47, 0.75], [1.0, 0.28, 0.69], [1.0, 0.91, 0.08]];
    let shift = (0.8 + 3.0 * s.brush) * k;
    let grain_size = (2.0 + 2.0 * s.brush) * k;
    let grain = paper_grain(w, h, s.seed + 5, k);
    let offs: Vec<(f32, f32)> = (0..3)
        .map(|j| {
            let a = rand01(j, 9, s.seed) * TAU;
            (a.cos() * shift, a.sin() * shift)
        })
        .collect();
    let mut out = img.clone();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let (xf, yf) = (x as f32, y as f32);
            let mut px = paper_at([0.97, 0.95, 0.90], grain[i], 0.07 * s.paper);
            for (j, ink) in inks.iter().enumerate() {
                let c = flat.sample(xf - offs[j].0, yf - offs[j].1);
                let cov = smoothstep(0.03, 0.97, (1.0 - c[j]).max(0.0).powf(0.85));
                let sj = s.seed + 31 + j as u32 * 7;
                let g = 0.6 * value_noise(xf / grain_size, yf / grain_size, sj)
                    + 0.25 * value_noise(xf / (grain_size * 2.3), yf / (grain_size * 2.3), sj + 1)
                    + 0.15 * rand01(x as i32, y as i32, sj + 2);
                let dot = smoothstep(-0.12, 0.12, cov * 1.2 - g);
                let roll = 0.88 + 0.24 * fbm(xf / (140.0 * k), yf / (140.0 * k), s.seed + 40 + j as u32, 2);
                print(&mut px, *ink, (dot * roll).min(1.0));
            }
            for ch in 0..3 {
                out.p[ch][i] = px[ch].clamp(0.0, 1.0);
            }
        }
    }
    out
}

/// Comic-print pop art: every area becomes one of a few printer's inks, flat or as a field of
/// dots on white, with black outlines.
pub fn pop_art(img: &Img, s: &NprSettings) -> Img {
    let k = s.px_scale.max(0.1);
    let (w, h) = (img.w, img.h);
    let mut flat = kuwahara(img, ((2.0 + 3.0 * s.brush) * k).max(1.0) as usize).blurred(0.8 * k);
    saturate(&mut flat, 1.5);
    let lum = img.lum();
    let line = outlines(&lum, w, h, (1.0 + 1.2 * s.brush) * k, 60.0);
    let cell = (4.0 + 7.0 * s.brush) * k;
    let px_cell = 1.0 / cell;
    const INKS: [[f32; 3]; 6] =
        [[0.93, 0.12, 0.15], [0.12, 0.35, 0.82], [1.0, 0.85, 0.0], [0.1, 0.62, 0.3], [1.0, 0.52, 0.08], [0.08, 0.08, 0.1]];
    // what each ink looks like flat and as 30% / 60% dots, to pick the nearest for an area
    let mut cands: Vec<([f32; 3], usize, f32)> = vec![([1.0, 1.0, 1.0], 0, 0.0)];
    for (j, ink) in INKS.iter().enumerate() {
        for cov in [0.3f32, 0.6, 1.0] {
            cands.push((mix3([1.0; 3], *ink, cov), j, cov));
        }
    }
    let norm = |c: [f32; 3]| {
        let mx = c[0].max(c[1]).max(c[2]).max(1e-3);
        [c[0] / mx, c[1] / mx, c[2] / mx]
    };
    let cands: Vec<([f32; 3], f32, usize, f32)> = cands.iter().map(|(c, j, cov)| (norm(*c), lum3(*c), *j, *cov)).collect();
    let grain = paper_grain(w, h, s.seed + 9, k);
    let (sn, cs) = 45.0f32.to_radians().sin_cos();
    let mut out = img.clone();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let (xf, yf) = (x as f32, y as f32);
            let c = flat.at(x, y);
            let (cn, cl) = (norm(c), lum3(c));
            let mut best = (f32::MAX, 0usize);
            for (ci, (n, l, _, _)) in cands.iter().enumerate() {
                let d = dist3(cn, *n) * 0.7 + (cl - l).abs() * 0.6;
                if d < best.0 {
                    best = (d, ci);
                }
            }
            let (_, _, j, cov) = cands[best.1];
            let mut px = paper_at([1.0, 0.99, 0.96], grain[i], 0.1 * s.paper);
            if cov >= 0.99 {
                print(&mut px, INKS[j], 1.0);
            } else if cov > 0.0 {
                let u = (xf * cs + yf * sn) * px_cell;
                let v = (-xf * sn + yf * cs) * px_cell;
                let (du, dv) = (frac(u) - 0.5, frac(v) - 0.5);
                let rr = (du * du + dv * dv).sqrt();
                let rad = (cov / std::f32::consts::PI).sqrt();
                let a = smoothstep(-0.5 * px_cell, 0.5 * px_cell, rad - rr);
                print(&mut px, INKS[j], a);
            }
            print(&mut px, [0.05, 0.05, 0.06], line[i]);
            for ch in 0..3 {
                out.p[ch][i] = px[ch].clamp(0.0, 1.0);
            }
        }
    }
    out
}

/// Copperplate engraving: sets of parallel lines that swell with the shadows and bulge over
/// the forms, crossing in the darkest parts.
pub fn engraving(img: &Img, s: &NprSettings) -> Img {
    let k = s.px_scale.max(0.1);
    let (w, h) = (img.w, img.h);
    let lum = img.lum();
    let mut tone = lum.clone();
    gaussian(&mut tone, w, h, 1.0 * k, true);
    let mut relief = lum.clone();
    gaussian(&mut relief, w, h, (5.0 + 8.0 * s.brush) * k, true);
    let spacing = (3.0 + 4.5 * s.brush) * k;
    let swell = spacing * 2.5;
    let sets = [(0.30f32, 0.10f32), (-0.95, 0.42), (1.25, 0.7)];
    let edge = edges(&tone, w, h, 1.6);
    let grain = paper_grain(w, h, s.seed + 13, k);
    let ink = [0.12, 0.09, 0.08];
    let mut out = img.clone();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let (xf, yf) = (x as f32, y as f32);
            let dark = 1.0 - tone[i];
            let wob = (fbm(xf / (60.0 * k), yf / (60.0 * k), s.seed + 2, 2) - 0.5) * 1.5 * k;
            let mut a: f32 = 0.0;
            for (j, (ang, t0)) in sets.iter().enumerate() {
                // hairlines where it is light, swelling into the shadows
                let amount = smoothstep(*t0, 1.0, dark);
                if amount <= 0.0 {
                    continue;
                }
                let (sn, cs) = ang.sin_cos();
                let u = xf * cs + yf * sn + relief[i] * swell + wob;
                let f = frac(u / spacing + j as f32 * 0.37);
                let half = 0.07 + 0.38 * amount;
                let line = smoothstep(-0.5 / spacing, 0.5 / spacing, half - (f - 0.5).abs());
                a = a + line - a * line;
            }
            a = a.max(edge[i] * 0.85).max(smoothstep(0.9, 0.98, dark));
            let a = a * (1.0 - s.paper * 0.3 * smoothstep(0.7, 0.95, grain[i]));
            // a hand-coloured print: a pale wash of the picture's colours under the lines
            let c = img.at(x, y);
            let mx = c[0].max(c[1]).max(c[2]).max(1e-3);
            let sat = (mx - c[0].min(c[1]).min(c[2])) / mx;
            let wash = mix3([1.0; 3], c.map(|v| v / mx), 0.35 * smoothstep(0.15, 0.5, sat));
            let mut px = paper_at([0.95, 0.93, 0.87], grain[i], 0.08 * s.paper);
            for ch in 0..3 {
                px[ch] *= wash[ch];
            }
            print(&mut px, ink, a);
            for ch in 0..3 {
                out.p[ch][i] = px[ch].clamp(0.0, 1.0);
            }
        }
    }
    out
}

/// Opaque, matte poster paint: flat patches with a ridge at their edges and dry streaks.
pub fn gouache(img: &Img, s: &NprSettings) -> Img {
    let k = s.px_scale.max(0.1);
    let (w, h) = (img.w, img.h);
    let r = ((3.0 + 8.0 * s.brush) * k).max(1.0) as usize;
    let mut flat = kuwahara(&wobble(img, 0.8 * k, 24.0 * k, s.seed), r);
    flat = kuwahara(&flat, (r / 2).max(1));
    saturate(&mut flat, 1.12);
    let lum = flat.lum();
    let levels = 7.0;
    let st = streaks(&lum, w, h, (3.0 + 3.0 * s.brush) * k, ((6.0 + 10.0 * s.brush) * k) as usize, s.seed);
    let mut q: Vec<f32> = lum.iter().map(|l| (l * levels).round() / levels).collect();
    gaussian(&mut q, w, h, 1.0 * k, true);
    let mut out = img.clone();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let (xf, yf) = (x as f32, y as f32);
            let l = lum[i].max(0.001);
            let f = ((l * levels).round() / levels).max(0.08) / l;
            let hx = q[y * w + (x + 1).min(w - 1)] - q[y * w + x.saturating_sub(1)];
            let hy = q[(y + 1).min(h - 1) * w + x] - q[y.saturating_sub(1) * w + x];
            let ridge = (hx + hy) * 0.35;
            let weave = 0.5 + 0.25 * ((xf * 1.3 / k).sin() + (yf * 1.3 / k).sin());
            let shade = 1.0 + 0.1 * (st[i] - 0.5) + ridge;
            for c in 0..3 {
                let v = flat.p[c][i] * f * shade;
                let v = v * 0.9 + 0.06;
                out.p[c][i] = (v * (1.0 - s.paper * 0.08 * weave)).clamp(0.0, 0.97);
            }
        }
    }
    out
}

/// Soft chalk pastel on toned paper, with the tooth of the paper showing through, and
/// charcoal along the edges.
pub fn pastel(img: &Img, s: &NprSettings) -> Img {
    let k = s.px_scale.max(0.1);
    let (w, h) = (img.w, img.h);
    let paper = [0.80, 0.76, 0.70];
    let mut soft = img.blurred((1.0 + 2.0 * s.brush) * k);
    saturate(&mut soft, 1.3);
    let lum = soft.lum();
    let st = streaks(&lum, w, h, (4.0 + 4.0 * s.brush) * k, ((4.0 + 8.0 * s.brush) * k) as usize, s.seed);
    let grain = paper_grain(w, h, s.seed + 29, k * (1.0 + s.brush));
    let edge = edges(&lum, w, h, 1.7);
    let mut out = img.clone();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let c = soft.at(x, y);
            let want = smoothstep(0.04, 0.4, dist3(c, paper));
            let tooth = 0.5 + (grain[i] - 0.5) * 1.6 * (0.4 + 0.6 * s.paper) + (st[i] - 0.5) * 1.0;
            let cover = (want * (0.25 + 0.95 * smoothstep(0.15, 0.85, tooth))).min(1.0);
            let chalk = c.map(|v| v * 0.92 + 0.08);
            let mut px = mix3(paper, chalk, cover);
            let ch = edge[i] * (0.5 + 0.5 * grain[i]);
            print(&mut px, [0.12, 0.11, 0.11], ch * 0.75);
            for ch in 0..3 {
                out.p[ch][i] = px[ch].clamp(0.0, 1.0);
            }
        }
    }
    out
}

/// Glass pieces that follow the picture's edges, each lit a little unevenly, set in lead.
pub fn stained_glass(img: &Img, s: &NprSettings) -> Img {
    let k = s.px_scale.max(0.1);
    let (w, h) = (img.w, img.h);
    let cell = ((14.0 + 34.0 * s.brush) * k).max(4.0);
    let flat = kuwahara(img, (2.0 * k).max(1.0) as usize).blurred(k);
    let gw = (w as f32 / cell).ceil() as usize + 2;
    let gh = (h as f32 / cell).ceil() as usize + 2;
    let mut rng = Rng::new(s.seed as u64 * 13 + 1);
    let mut seeds: Vec<(f32, f32, [f32; 3])> = Vec::with_capacity(gw * gh);
    for gy in 0..gh {
        for gx in 0..gw {
            let sx = (gx as f32 - 1.0 + 0.15 + 0.7 * rng.f()) * cell;
            let sy = (gy as f32 - 1.0 + 0.15 + 0.7 * rng.f()) * cell;
            let col = flat.sample(sx, sy);
            seeds.push((sx, sy, col));
        }
    }
    // each pixel joins the nearest piece, where "near" also means "the same colour", so the
    // pieces follow the picture's edges
    let lambda = cell * 1.4;
    let mut id = vec![0u32; w * h];
    for y in 0..h {
        for x in 0..w {
            let (xf, yf) = (x as f32, y as f32);
            let c = flat.at(x, y);
            let (gx0, gy0) = ((xf / cell) as usize + 1, (yf / cell) as usize + 1);
            let mut best = (f32::MAX, 0usize);
            for dy in -1..=1isize {
                for dx in -1..=1isize {
                    let gi = ((gy0 as isize + dy) as usize).min(gh - 1) * gw + ((gx0 as isize + dx) as usize).min(gw - 1);
                    let (sx, sy, sc) = seeds[gi];
                    let d = ((xf - sx).powi(2) + (yf - sy).powi(2)).sqrt() + lambda * dist3(c, sc);
                    if d < best.0 {
                        best = (d, gi);
                    }
                }
            }
            id[y * w + x] = best.1 as u32;
        }
    }
    let mut sum = vec![[0.0f32; 4]; seeds.len()];
    for y in 0..h {
        for x in 0..w {
            let c = flat.at(x, y);
            let s = &mut sum[id[y * w + x] as usize];
            for ch in 0..3 {
                s[ch] += c[ch];
            }
            s[3] += 1.0;
        }
    }
    let mut lead = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if (x + 1 < w && id[i] != id[i + 1]) || (y + 1 < h && id[i] != id[i + w]) {
                lead[i] = 1.0;
            }
        }
    }
    let sigma = (1.0 + 1.6 * s.brush) * k * 0.6;
    gaussian(&mut lead, w, h, sigma, true);
    let norm = sigma * 2.5;
    let mut out = img.clone();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let (xf, yf) = (x as f32, y as f32);
            let (sx, sy, _) = seeds[id[i] as usize];
            let sm = sum[id[i] as usize];
            let n = sm[3].max(1.0);
            let avg = [sm[0] / n, sm[1] / n, sm[2] / n];
            let l = lum3(avg);
            let col = avg.map(|v| (l + (v - l) * 1.35).clamp(0.0, 1.0));
            let grad = (0.5 + 0.5 * ((xf - sx) * -0.6 + (yf - sy) * -0.8) / cell).clamp(0.0, 1.0);
            let streak = fbm(xf / (9.0 * k), yf / (28.0 * k), s.seed + 50, 2) - 0.5;
            let light = 0.78 + 0.35 * grad + 0.3 * streak * s.paper;
            let px = col.map(|v| (v * light).clamp(0.0, 1.0));
            let la = smoothstep(0.25, 0.7, lead[i] * norm);
            let px = mix3(px, [0.11, 0.10, 0.09], la);
            for ch in 0..3 {
                out.p[ch][i] = px[ch];
            }
        }
    }
    out
}

/// Ink washes on rice paper: a few tones that bleed at their edges, dry-brush streaks of
/// "flying white", bold strokes on the strong edges and a pale colour wash.
pub fn sumi_e(img: &Img, s: &NprSettings) -> Img {
    let k = s.px_scale.max(0.1);
    let (w, h) = (img.w, img.h);
    let wob = wobble(img, (2.0 + 3.0 * s.brush) * k, 45.0 * k, s.seed);
    let flat = kuwahara(&wob, ((2.0 + 5.0 * s.brush) * k).max(1.0) as usize).blurred(1.5 * k);
    let tone = flat.lum();
    let dark: Vec<f32> = tone.iter().map(|t| 1.0 - t).collect();
    let mut ink = vec![0.0f32; w * h];
    for (t, a) in [(0.27f32, 0.32f32), (0.55, 0.32), (0.78, 0.36)] {
        let mut m: Vec<f32> = dark.iter().map(|d| smoothstep(t - 0.05, t + 0.05, *d)).collect();
        gaussian(&mut m, w, h, (1.5 + 2.5 * s.brush) * k, true);
        for i in 0..w * h {
            ink[i] += m[i] * a;
        }
    }
    let carve = streaks(&tone, w, h, (3.0 + 3.0 * s.brush) * k, ((6.0 + 14.0 * s.brush) * k) as usize, s.seed);
    let edge = edges(&tone, w, h, 1.4);
    let grain = paper_grain(w, h, s.seed + 37, k);
    let mut out = img.clone();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let dry = 1.0 - 0.55 * smoothstep(0.55, 0.95, carve[i]) * smoothstep(0.2, 0.6, ink[i]);
            let mut a = (ink[i] * dry).min(1.0);
            let stroke = smoothstep(0.15, 0.45, edge[i]) * 0.95 * (0.6 + 0.4 * smoothstep(0.2, 0.8, carve[i]));
            a = a.max(stroke);
            let c = flat.at(x, y);
            let mx = c[0].max(c[1]).max(c[2]).max(1e-3);
            let sat = (mx - c[0].min(c[1]).min(c[2])) / mx;
            let wash = mix3([1.0; 3], c.map(|v| v / mx), 0.4 * smoothstep(0.15, 0.5, sat));
            let mut px = paper_at([0.96, 0.94, 0.88], grain[i], 0.1 * s.paper);
            for ch in 0..3 {
                px[ch] *= wash[ch];
            }
            let a = a * (1.0 - s.paper * 0.25 * smoothstep(0.7, 0.95, grain[i]));
            print(&mut px, [0.08, 0.08, 0.10], a);
            for ch in 0..3 {
                out.p[ch][i] = px[ch].clamp(0.0, 1.0);
            }
        }
    }
    out
}

/// Layers of cut coloured paper: flat colours, scissor-wobbly edges, a drop shadow below and
/// to the right of every edge, and paper fibres.
pub fn paper_cut(img: &Img, s: &NprSettings) -> Img {
    let k = s.px_scale.max(0.1);
    let (w, h) = (img.w, img.h);
    let r = ((5.0 + 10.0 * s.brush) * k).max(1.0) as usize;
    let mut flat = kuwahara(&wobble(img, (1.0 + 1.5 * s.brush) * k, 22.0 * k, s.seed), r);
    flat = kuwahara(&flat, (r / 2).max(1)).blurred(1.2 * k);
    saturate(&mut flat, 1.15);
    let lum = flat.lum();
    let levels = 5.0;
    let mut poster = flat.clone();
    for i in 0..w * h {
        let l = lum[i].max(0.001);
        let q = ((l * levels).round() / levels).max(0.06);
        for c in 0..3 {
            let v = (flat.p[c][i] * q / l).clamp(0.0, 1.0);
            let d = ((v - q) * 3.0).round() / 3.0;
            poster.p[c][i] = (q + d).clamp(0.0, 1.0);
        }
    }
    // specks too small to cut out of paper join the colour around them
    let key = |c: [f32; 3]| ((c[0] * 31.0) as u32) << 20 | ((c[1] * 31.0) as u32) << 10 | (c[2] * 31.0) as u32;
    let mut keys: Vec<u32> = (0..w * h).map(|i| key(poster.at(i % w, i / w))).collect();
    for _ in 0..2 {
        keys = majority(&keys, w, h, (2.0 * k).round().max(1.0) as usize);
    }
    for i in 0..w * h {
        let kk = keys[i];
        for (c, shift) in [(0usize, 20u32), (1, 10), (2, 0)] {
            poster.p[c][i] = ((kk >> shift) & 1023) as f32 / 31.0;
        }
    }
    let mut b = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if (x + 1 < w && keys[i] != keys[i + 1]) || (y + 1 < h && keys[i] != keys[i + w]) {
                b[i] = 1.0;
            }
        }
    }
    let off = (2.0 + 3.0 * s.brush) * k;
    let sigma = (1.5 + 2.0 * s.brush) * k;
    let (ox, oy) = ((off * 0.6).round() as usize, off.round() as usize);
    let mut shifted = vec![0.0f32; w * h];
    for y in oy..h {
        for x in ox..w {
            shifted[y * w + x] = b[(y - oy) * w + (x - ox)];
        }
    }
    gaussian(&mut shifted, w, h, sigma, true);
    let mut near = b;
    gaussian(&mut near, w, h, sigma, true);
    let norm = sigma * 2.5;
    let mut out = img.clone();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let (xf, yf) = (x as f32, y as f32);
            let sh = ((shifted[i] - 0.8 * near[i]) * norm).clamp(0.0, 1.0);
            let fibre = 0.5 + 0.5 * fbm(xf / (2.5 * k), yf / (16.0 * k), s.seed + 61, 2);
            for c in 0..3 {
                let v = poster.p[c][i] * (1.0 - 0.4 * sh) * (1.0 - 0.08 * s.paper * (1.0 - fibre));
                out.p[c][i] = v.clamp(0.0, 1.0);
            }
        }
    }
    out
}

/// The most common value in the (2r+1)² window around each pixel.
fn majority(keys: &[u32], w: usize, h: usize, r: usize) -> Vec<u32> {
    let mut out = keys.to_vec();
    let mut counts: Vec<(u32, u32)> = Vec::with_capacity(16);
    for y in 0..h {
        for x in 0..w {
            counts.clear();
            for yy in y.saturating_sub(r)..(y + r + 1).min(h) {
                for xx in x.saturating_sub(r)..(x + r + 1).min(w) {
                    let kk = keys[yy * w + xx];
                    match counts.iter_mut().find(|(k, _)| *k == kk) {
                        Some(e) => e.1 += 1,
                        None => counts.push((kk, 1)),
                    }
                }
            }
            out[y * w + x] = counts.iter().max_by_key(|(_, n)| *n).map_or(keys[y * w + x], |(k, _)| *k);
        }
    }
    out
}

/// White and pastel chalk on a dark board: lines on the edges, rubbed shading where the
/// picture is light, breaking up on the board's texture.
pub fn chalkboard(img: &Img, s: &NprSettings) -> Img {
    let k = s.px_scale.max(0.1);
    let (w, h) = (img.w, img.h);
    let lum = img.lum();
    let mut soft = lum.clone();
    gaussian(&mut soft, w, h, (0.8 + s.brush) * k, true);
    let edge = edges(&soft, w, h, 2.0);
    let mut tone = lum.clone();
    gaussian(&mut tone, w, h, 2.0 * k, true);
    let grain = paper_grain(w, h, s.seed + 41, k);
    let mut sat = img.clone();
    saturate(&mut sat, 1.2);
    let mut out = img.clone();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let (xf, yf) = (x as f32, y as f32);
            let dust = 0.5 + 0.5 * fbm(xf / (90.0 * k), yf / (60.0 * k), s.seed + 43, 3);
            let d = 0.08 * dust * s.paper;
            let board = [0.09 + d, 0.14 + d, 0.12 + d];
            let light = smoothstep(0.25, 0.8, tone[i]);
            let u = (xf + yf) * 0.7071;
            let v = (xf - yf) * 0.7071;
            let hatch = smoothstep(-0.08, 0.08, light * 1.1 - 0.15 - value_noise(v * 0.25 / k, u * 0.02 / k, s.seed + 5));
            let tooth = 0.35 + 0.65 * grain[i];
            let cover = (edge[i] * 1.5 * tooth).max(light * 0.8 * hatch * tooth).min(1.0);
            // chalk is always light, whatever colour it is
            let c = sat.at(x, y);
            let chalk = mix3([0.96, 0.96, 0.93], c.map(|v| v * 0.4 + 0.6), 0.65);
            let px = mix3(board, chalk, cover);
            for ch in 0..3 {
                out.p[ch][i] = px[ch].clamp(0.0, 1.0);
            }
        }
    }
    out
}

/// A cyanotype: white lines and hatching on Prussian blue, over a faint drafting grid.
pub fn blueprint(img: &Img, s: &NprSettings) -> Img {
    let k = s.px_scale.max(0.1);
    let (w, h) = (img.w, img.h);
    let lum = img.lum();
    let line = outlines(&lum, w, h, (0.7 + 0.8 * s.brush) * k, 140.0);
    let mut tone = lum.clone();
    gaussian(&mut tone, w, h, 1.5 * k, true);
    let grain = paper_grain(w, h, s.seed + 47, k);
    let grid = (24.0 + 16.0 * s.brush) * k;
    let hatch_sp = (4.0 + 3.0 * s.brush) * k;
    let mut out = img.clone();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let (xf, yf) = (x as f32, y as f32);
            let blotch = 0.9 + 0.2 * fbm(xf / (120.0 * k), yf / (120.0 * k), s.seed + 48, 3);
            let bg = paper_at([0.07 * blotch, 0.21 * blotch, 0.47 * blotch], grain[i], 0.1 * s.paper);
            let light = smoothstep(0.55, 1.0, tone[i]) * 0.16;
            let gl = if frac(xf / grid) < 1.0 / grid || frac(yf / grid) < 1.0 / grid { 0.09 } else { 0.0 };
            let u = (xf - yf) * 0.7071;
            let f = frac(u / hatch_sp);
            let half = smoothstep(0.6, 1.0, tone[i]) * 0.3;
            let hatch = smoothstep(-0.5 / hatch_sp, 0.5 / hatch_sp, half - (f - 0.5).abs()) * 0.35;
            let a = line[i].max(light).max(gl).max(hatch) * (0.8 + 0.2 * grain[i]);
            let px = mix3(bg, [0.92, 0.95, 1.0], a);
            for ch in 0..3 {
                out.p[ch][i] = px[ch].clamp(0.0, 1.0);
            }
        }
    }
    out
}
