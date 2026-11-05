//! Non-destructive object effects: bevel / material lighting, environment reflection,
//! drop shadow and outer glow.

use crate::doc::{Bevel, BevelProfile, Effects};
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
pub fn apply_bevel(pm: &mut Pixmap, b: &Bevel, light: [f32; 3], coverage: Option<&[f32]>) {
    let w = pm.width() as usize;
    let h = pm.height() as usize;
    let n = w * h;
    let alpha = alpha_of(pm);
    if !alpha.iter().any(|&a| a > 0.0) {
        return;
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
