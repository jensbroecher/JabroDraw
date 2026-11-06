//! Non-destructive object effects: bevel / material lighting, environment reflection,
//! point / spot / distant lights, transparency, paint outlines, drop shadow and outer glow.

use crate::doc::{
    Adjust, Bevel, BevelProfile, Effects, Light, LightKind, Lighting, StrokePos, StrokeStyle, Transparency,
    TransparencyKind,
};
use crate::geom::Pt;
use crate::raster::{alpha_of, gaussian};
use tiny_skia::Pixmap;

/// Light direction expressed in the object's local frame (so rotated / mirrored objects
/// stay lit from the same side of the canvas).
pub fn local_light(b: &Bevel, ex: Pt, ey: Pt) -> [f32; 3] {
    let az = b.light_angle.to_radians();
    let el = b.light_elevation.to_radians().clamp(0.05, 1.55);
    let l2 = Pt::new(az.cos(), -az.sin());
    let lx = l2.dot(ex);
    let ly = l2.dot(ey);
    let len = (lx * lx + ly * ly).sqrt().max(1e-6);
    [lx / len * el.cos(), ly / len * el.cos(), el.sin()]
}

fn dt1d(f: &[f64], d: &mut [f64], v: &mut [usize], z: &mut [f64]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let sect = |q: usize, p: usize| -> f64 {
        let (qf, pf) = (q as f64, p as f64);
        ((f[q] + qf * qf) - (f[p] + pf * pf)) / (2.0 * qf - 2.0 * pf)
    };
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..n {
        let mut s = sect(q, v[k]);
        while s <= z[k] {
            k -= 1;
            s = sect(q, v[k]);
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    k = 0;
    for (q, dq) in d.iter_mut().enumerate().take(n) {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let dist = q as f64 - v[k] as f64;
        *dq = dist * dist + f[v[k]];
    }
}

/// Exact euclidean distance (in pixels) from every inside pixel to the shape edge.
pub fn edt_inside(alpha: &[f32], w: usize, h: usize) -> Vec<f32> {
    let inf = ((w + h) * (w + h)) as f64 + 1.0;
    let mut f: Vec<f64> = alpha
        .iter()
        .map(|&a| if a >= 0.5 { inf } else { 0.0 })
        .collect();
    let maxn = w.max(h);
    let mut line = vec![0f64; maxn];
    let mut d = vec![0f64; maxn];
    let mut v = vec![0usize; maxn];
    let mut z = vec![0f64; maxn + 1];
    for x in 0..w {
        for y in 0..h {
            line[y] = f[y * w + x];
        }
        dt1d(&line[..h], &mut d[..h], &mut v, &mut z);
        for y in 0..h {
            f[y * w + x] = d[y];
        }
    }
    for y in 0..h {
        line[..w].copy_from_slice(&f[y * w..(y + 1) * w]);
        dt1d(&line[..w], &mut d[..w], &mut v, &mut z);
        f[y * w..(y + 1) * w].copy_from_slice(&d[..w]);
    }
    f.iter().map(|&v| v.sqrt() as f32).collect()
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Procedural "chrome" environment: sky above the horizon, dark ground below.
fn chrome_env(rx: f32, ry: f32) -> [f32; 3] {
    let u = -ry * 0.9 + 0.08 + rx * 0.05;
    if u >= 0.0 {
        let t = (u / 0.9).clamp(0.0, 1.0).sqrt();
        let horizon_glow = (1.0 - (u / 0.06).clamp(0.0, 1.0)) * 0.15;
        let c = mix3([0.98, 0.98, 1.0], [0.18, 0.36, 0.72], t);
        [
            (c[0] + horizon_glow).min(1.0),
            (c[1] + horizon_glow).min(1.0),
            (c[2] + horizon_glow).min(1.0),
        ]
    } else {
        let t = (-u / 0.9).clamp(0.0, 1.0).sqrt();
        mix3([0.42, 0.33, 0.24], [0.08, 0.06, 0.05], t)
    }
}

fn rgb(c: [u8; 4]) -> [f32; 3] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0]
}

/// Bevel + material shading applied in place to a premultiplied local raster.
/// Returns the height map so object lights can shade the same relief.
pub fn apply_bevel(pm: &mut Pixmap, b: &Bevel, light: [f32; 3], coverage: Option<&[f32]>) -> Option<Vec<f32>> {
    let w = pm.width() as usize;
    let h = pm.height() as usize;
    let n = w * h;
    let alpha = alpha_of(pm);
    if !alpha.iter().any(|&a| a > 0.0) {
        return None;
    }
    // The bevel follows the shape coverage (so translucent fills still bevel as a whole),
    // or the pixel alpha for pure paint objects.
    let cov: &[f32] = match coverage {
        Some(c) if c.len() == n => c,
        _ => &alpha,
    };
    let dist = edt_inside(cov, w, h);
    let mut width = b.width.max(0.5);
    if b.profile == BevelProfile::Pillow {
        width = dist.iter().cloned().fold(1.0, f32::max);
    }
    let mut height = vec![0f32; n];
    for i in 0..n {
        if dist[i] <= 0.0 {
            continue;
        }
        let d = (dist[i] - 1.0 + cov[i].min(1.0)).max(0.0);
        let t = (d / width).clamp(0.0, 1.0);
        let p = match b.profile {
            BevelProfile::Round | BevelProfile::Pillow => (1.0 - (1.0 - t) * (1.0 - t)).sqrt(),
            BevelProfile::Chisel => t,
            BevelProfile::Cove => 1.0 - (1.0 - t * t).max(0.0).sqrt(),
            BevelProfile::Ridge => 1.0 - (2.0 * t - 1.0).abs(),
        };
        height[i] = p * width * b.depth;
    }
    gaussian(&mut height, w, h, b.softness.max(0.6), true);

    let (lx, ly, lz) = (light[0], light[1], light[2]);
    let flat = lz;
    let hl = {
        let (x, y, z) = (lx, ly, lz + 1.0);
        let l = (x * x + y * y + z * z).sqrt();
        [x / l, y / l, z / l]
    };
    let shin = b.shininess.max(1.0);
    let flat_spec = hl[2].powf(shin);
    let hi_c = rgb(b.highlight);
    let sh_c = rgb(b.shadow);
    let data = pm.data_mut();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let a = alpha[i];
            if a <= 0.0 {
                continue;
            }
            let hx = (height[y * w + (x + 1).min(w - 1)] - height[y * w + x.saturating_sub(1)]) * 0.5;
            let hy = (height[(y + 1).min(h - 1) * w + x] - height[y.saturating_sub(1) * w + x]) * 0.5;
            let (mut nx, mut ny, mut nz) = (-hx, -hy, 1.0f32);
            let nl = (nx * nx + ny * ny + nz * nz).sqrt();
            nx /= nl;
            ny /= nl;
            nz /= nl;
            let ndl = nx * lx + ny * ly + nz * lz;
            let s = ndl - flat;
            let s = if s > 0.0 {
                s / (1.0 - flat).max(0.05)
            } else {
                s / (1.0 + flat).max(0.05)
            };
            let amt = (s.abs() * b.intensity).min(1.0);
            let p = i * 4;
            let mut c = [
                (data[p] as f32 / 255.0 / a).min(1.0),
                (data[p + 1] as f32 / 255.0 / a).min(1.0),
                (data[p + 2] as f32 / 255.0 / a).min(1.0),
            ];
            c = mix3(c, if s > 0.0 { hi_c } else { sh_c }, amt);
            if b.reflection > 0.0 {
                let rx = 2.0 * nz * nx;
                // Fake curvature across flat faces so they mirror a horizon, like real chrome.
                let ry = 2.0 * nz * ny + (y as f32 / h as f32 - 0.5) * 1.2;
                let env = chrome_env(rx, ry);
                let tint = [
                    env[0] * (0.3 + 0.7 * (c[0] * 1.4).min(1.0)),
                    env[1] * (0.3 + 0.7 * (c[1] * 1.4).min(1.0)),
                    env[2] * (0.3 + 0.7 * (c[2] * 1.4).min(1.0)),
                ];
                c = mix3(c, tint, b.reflection.clamp(0.0, 1.0));
            }
            if b.gloss > 0.0 {
                let ndh = (nx * hl[0] + ny * hl[1] + nz * hl[2]).max(0.0);
                let sp = ((ndh.powf(shin) - flat_spec) / (1.0 - flat_spec).max(1e-3)).max(0.0) * b.gloss;
                for v in c.iter_mut() {
                    *v += (1.0 - *v) * sp.min(1.0);
                }
            }
            for k in 0..3 {
                data[p + k] = (c[k].clamp(0.0, 1.0) * a * 255.0 + 0.5) as u8;
            }
        }
    }
    Some(height)
}

#[inline]
fn normal_at(height: Option<&[f32]>, w: usize, h: usize, x: usize, y: usize) -> [f32; 3] {
    let Some(hm) = height else { return [0.0, 0.0, 1.0] };
    let hx = (hm[y * w + (x + 1).min(w - 1)] - hm[y * w + x.saturating_sub(1)]) * 0.5;
    let hy = (hm[(y + 1).min(h - 1) * w + x] - hm[y.saturating_sub(1) * w + x]) * 0.5;
    let l = (hx * hx + hy * hy + 1.0).sqrt();
    [-hx / l, -hy / l, 1.0 / l]
}

fn norm3(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    [v[0] / l, v[1] / l, v[2] / l]
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Geometry the object lights need: the local raster layout and its axes on the canvas.
pub struct LightFrame {
    /// Size of the shape inside the padded raster, and the padding.
    pub w: f32,
    pub h: f32,
    pub pad: f32,
    /// Unit vectors of the local x / y axes on the canvas (for distant lights).
    pub ex: Pt,
    pub ey: Pt,
}

/// Point, spot and distant lights on the object's own surface (bevel relief included).
pub fn apply_lights(pm: &mut Pixmap, lighting: &Lighting, height: Option<&[f32]>, f: &LightFrame) {
    let lights: Vec<&Light> = lighting.lights.iter().filter(|l| l.enabled).collect();
    if lights.is_empty() {
        return;
    }
    let (pw, ph) = (pm.width() as usize, pm.height() as usize);
    let scale = (f.w + f.h) * 0.5;
    struct Prepared {
        col: [f32; 3],
        kind: LightKind,
        pos: [f32; 3],
        dir: [f32; 3],
        axis: [f32; 3],
        range: f32,
        cos_in: f32,
        cos_out: f32,
    }
    let prepared: Vec<Prepared> = lights
        .iter()
        .map(|l| {
            let c = rgb(l.color);
            let col = [c[0] * l.intensity, c[1] * l.intensity, c[2] * l.intensity];
            let pos = [f.pad + l.pos[0] * f.w, f.pad + l.pos[1] * f.h, l.height.max(0.01) * scale];
            let target = [f.pad + l.target[0] * f.w, f.pad + l.target[1] * f.h, 0.0];
            let axis = norm3([target[0] - pos[0], target[1] - pos[1], target[2] - pos[2]]);
            let az = l.angle.to_radians();
            let el = l.elevation.to_radians().clamp(0.02, 1.56);
            let l2 = Pt::new(az.cos(), -az.sin());
            let (lx, ly) = (l2.dot(f.ex), l2.dot(f.ey));
            let len = (lx * lx + ly * ly).sqrt().max(1e-6);
            let dir = [lx / len * el.cos(), ly / len * el.cos(), el.sin()];
            let cone = l.cone.clamp(1.0, 89.0).to_radians();
            let outer = (cone * (1.0 + l.softness.clamp(0.0, 1.0))).min(1.55);
            Prepared {
                col,
                kind: l.kind,
                pos,
                dir,
                axis,
                range: (l.range.max(0.01) * scale).max(1.0),
                cos_in: cone.cos(),
                cos_out: outer.cos(),
            }
        })
        .collect();
    let ambient = lighting.ambient.clamp(0.0, 2.0);
    let spec_amt = lighting.specular.clamp(0.0, 2.0);
    let data = pm.data_mut();
    for y in 0..ph {
        for x in 0..pw {
            let p = (y * pw + x) * 4;
            let a = data[p + 3] as f32 / 255.0;
            if a <= 0.0 {
                continue;
            }
            let n = normal_at(height, pw, ph, x, y);
            let mut diff = [ambient; 3];
            let mut spec = [0.0f32; 3];
            for l in &prepared {
                let (ldir, atten) = match l.kind {
                    LightKind::Distant => (l.dir, 1.0),
                    _ => {
                        let v = [l.pos[0] - x as f32, l.pos[1] - y as f32, l.pos[2]];
                        let d = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-3);
                        let ld = [v[0] / d, v[1] / d, v[2] / d];
                        let mut att = 1.0 / (1.0 + (d / l.range).powi(2));
                        if l.kind == LightKind::Spot {
                            let cos_t = -dot3(ld, l.axis);
                            att *= crate::textures::smoothstep(l.cos_out, l.cos_in, cos_t);
                        }
                        (ld, att * 1.6)
                    }
                };
                if atten <= 0.0 {
                    continue;
                }
                let ndl = dot3(n, ldir).max(0.0);
                let hv = norm3([ldir[0], ldir[1], ldir[2] + 1.0]);
                let s = dot3(n, hv).max(0.0).powf(40.0) * spec_amt * atten;
                for k in 0..3 {
                    diff[k] += l.col[k] * ndl * atten;
                    spec[k] += l.col[k] * s;
                }
            }
            for k in 0..3 {
                let c = (data[p + k] as f32 / 255.0 / a).min(1.0);
                let v = (c * diff[k] + spec[k]).clamp(0.0, 1.0);
                data[p + k] = (v * a * 255.0 + 0.5) as u8;
            }
        }
    }
}

fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) * 0.5;
    if max - min < 1e-6 {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h * 60.0, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    if s <= 0.0 {
        return (l, l, l);
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let hk = h.rem_euclid(360.0) / 360.0;
    let f = |t: f32| {
        let t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    (f(hk + 1.0 / 3.0), f(hk), f(hk - 1.0 / 3.0))
}

/// Levels, hue / saturation / lightness and contrast on the object's own pixels.
pub fn apply_adjust(pm: &mut Pixmap, a: &Adjust) {
    let range = (a.white - a.black).max(0.01);
    let inv_gamma = 1.0 / a.gamma.clamp(0.1, 10.0);
    let c = a.contrast.clamp(-1.0, 0.99);
    let contrast = (1.0 + c) / (1.0 - c);
    for px in pm.data_mut().chunks_exact_mut(4) {
        let alpha = px[3] as f32 / 255.0;
        if alpha <= 0.0 {
            continue;
        }
        let mut v = [0.0f32; 3];
        for k in 0..3 {
            let x = (px[k] as f32 / 255.0 / alpha).min(1.0);
            v[k] = ((x - a.black) / range).clamp(0.0, 1.0).powf(inv_gamma);
        }
        if a.hue != 0.0 || a.saturation != 0.0 || a.lightness != 0.0 {
            let (h, s, l) = rgb_to_hsl(v[0], v[1], v[2]);
            let s = (s * (1.0 + a.saturation)).clamp(0.0, 1.0);
            let l = if a.lightness > 0.0 { l + (1.0 - l) * a.lightness } else { l * (1.0 + a.lightness) };
            let (r, g, b) = hsl_to_rgb(h + a.hue, s, l.clamp(0.0, 1.0));
            v = [r, g, b];
        }
        for k in 0..3 {
            let x = ((v[k] - 0.5) * contrast + 0.5).clamp(0.0, 1.0);
            px[k] = (x * alpha * 255.0 + 0.5) as u8;
        }
    }
}

/// Opacity of the object's transparency at a local raster pixel.
pub fn transparency_at(t: &Transparency, x: f32, y: f32, f: &LightFrame, seed: u32) -> f32 {
    let lerp = |k: f32| t.start + (t.end - t.start) * k.clamp(0.0, 1.0);
    let px = Pt::new(x - f.pad, y - f.pad);
    let p0 = Pt::new(t.p0[0] * f.w, t.p0[1] * f.h);
    let p1 = Pt::new(t.p1[0] * f.w, t.p1[1] * f.h);
    match t.kind {
        TransparencyKind::None => 1.0,
        TransparencyKind::Flat => t.start,
        TransparencyKind::Linear => {
            let d = p1 - p0;
            lerp((px - p0).dot(d) / d.dot(d).max(1e-6))
        }
        TransparencyKind::Radial => lerp(px.dist(p0) / p1.dist(p0).max(1e-3)),
        TransparencyKind::Texture => match (&t.texture, &t.image) {
            (crate::doc::TextureKind::Image, Some(img)) => {
                lerp(crate::imageops::luminance_at(&img.0, px.x, px.y, f.w, f.h))
            }
            _ => {
                let s = 1.0 / t.tex_scale.max(0.05);
                lerp(crate::textures::sample(t.texture, x * s, y * s, seed))
            }
        },
    }
    .clamp(0.0, 1.0)
}

/// Multiply the local raster by the object's transparency (it then also thins shadows and glows).
pub fn apply_transparency(pm: &mut Pixmap, t: &Transparency, f: &LightFrame, seed: u32) {
    if t.kind == TransparencyKind::None {
        return;
    }
    let pw = pm.width() as usize;
    let data = pm.data_mut();
    for (i, px) in data.chunks_exact_mut(4).enumerate() {
        if px[3] == 0 {
            continue;
        }
        let k = transparency_at(t, (i % pw) as f32 + 0.5, (i / pw) as f32 + 0.5, f, seed);
        for v in px.iter_mut() {
            *v = (*v as f32 * k + 0.5) as u8;
        }
    }
}

/// Outline around whatever is painted (paint layers have no vector path to stroke).
pub fn alpha_outline(pm: &mut Pixmap, s: &StrokeStyle) {
    let w = pm.width() as usize;
    let h = pm.height() as usize;
    let alpha = alpha_of(pm);
    if !alpha.iter().any(|&a| a > 0.0) {
        return;
    }
    let inv: Vec<f32> = alpha.iter().map(|a| 1.0 - a).collect();
    let d_out = edt_inside(&inv, w, h); // outside pixels: distance to the paint
    let d_in = edt_inside(&alpha, w, h); // inside pixels: distance to the edge
    let (reach_out, reach_in) = match s.position {
        StrokePos::Outside => (s.width, 0.0),
        StrokePos::Inside => (0.0, s.width),
        StrokePos::Center => (s.width * 0.5, s.width * 0.5),
    };
    let col = rgb(s.color);
    let ca = s.color[3] as f32 / 255.0;
    let data = pm.data_mut();
    for i in 0..w * h {
        let p = i * 4;
        let a = alpha[i];
        if d_out[i] > 0.0 {
            // outside the paint: draw underneath anything semi-transparent
            let k = (reach_out + 0.5 - d_out[i]).clamp(0.0, 1.0) * ca;
            if k > 0.0 {
                let under = k * (1.0 - a);
                for c in 0..3 {
                    data[p + c] = (data[p + c] as f32 + col[c] * 255.0 * under).min(255.0) as u8;
                }
                data[p + 3] = ((a + under) * 255.0 + 0.5).min(255.0) as u8;
            }
        } else if d_in[i] > 0.0 && reach_in > 0.0 {
            let k = (reach_in + 0.5 - d_in[i]).clamp(0.0, 1.0) * ca;
            if k > 0.0 {
                for c in 0..3 {
                    data[p + c] = (col[c] * 255.0 * k * a + data[p + c] as f32 * (1.0 - k)) as u8;
                }
            }
        }
    }
}

/// Composite drop shadow and outer glow underneath an already warped layer.
pub fn shadow_glow(layer: &Pixmap, fx: &Effects) -> Pixmap {
    let w = layer.width() as usize;
    let h = layer.height() as usize;
    let n = w * h;
    let alpha = alpha_of(layer);
    let mut out = vec![0f32; n * 4];
    let over = |out: &mut [f32], i: usize, col: [f32; 3], a: f32| {
        let p = i * 4;
        let inv = 1.0 - a;
        for k in 0..3 {
            out[p + k] = col[k] * a + out[p + k] * inv;
        }
        out[p + 3] = a + out[p + 3] * inv;
    };
    if fx.shadow.enabled {
        let mut a = alpha.clone();
        gaussian(&mut a, w, h, fx.shadow.blur * 0.5, false);
        let dx = fx.shadow.dx.round() as i64;
        let dy = fx.shadow.dy.round() as i64;
        let col = rgb(fx.shadow.color);
        let op = fx.shadow.opacity * fx.shadow.color[3] as f32 / 255.0;
        for y in 0..h as i64 {
            let sy = y - dy;
            if sy < 0 || sy >= h as i64 {
                continue;
            }
            for x in 0..w as i64 {
                let sx = x - dx;
                if sx < 0 || sx >= w as i64 {
                    continue;
                }
                let sa = a[(sy as usize) * w + sx as usize] * op;
                if sa > 0.0 {
                    over(&mut out, y as usize * w + x as usize, col, sa.min(1.0));
                }
            }
        }
    }
    if fx.glow.enabled {
        let mut a = alpha.clone();
        gaussian(&mut a, w, h, fx.glow.size * 0.5, false);
        let col = rgb(fx.glow.color);
        let op = fx.glow.opacity * fx.glow.color[3] as f32 / 255.0;
        for (i, &v) in a.iter().enumerate() {
            let ga = (v * 2.2).min(1.0) * op;
            if ga > 0.0 {
                over(&mut out, i, col, ga);
            }
        }
    }
    let src = layer.data();
    let mut pm = Pixmap::new(w as u32, h as u32).expect("layer size");
    let d = pm.data_mut();
    for i in 0..n {
        let p = i * 4;
        let sa = src[p + 3] as f32 / 255.0;
        let inv = 1.0 - sa;
        let a = sa + out[p + 3] * inv;
        let a8 = (a * 255.0 + 0.5).min(255.0) as u8;
        for k in 0..3 {
            let v = src[p + k] as f32 / 255.0 + out[p + k] * inv;
            d[p + k] = ((v * 255.0 + 0.5) as u8).min(a8);
        }
        d[p + 3] = a8;
    }
    pm
}
