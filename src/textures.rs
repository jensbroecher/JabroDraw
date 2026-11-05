//! Procedural textures and noise — no bitmap assets needed.

use crate::doc::{Rgba, TextureKind};
use tiny_skia::Pixmap;

#[inline]
pub fn hash2(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0xcb1a_b31f).wrapping_add(0x9e37_79b9);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    h
}

#[inline]
pub fn rand01(x: i32, y: i32, seed: u32) -> f32 {
    (hash2(x, y, seed) & 0x00ff_ffff) as f32 / 16_777_216.0
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[inline]
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Smooth value noise in 0..1.
pub fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let xi = x.floor();
    let yi = y.floor();
    let (xf, yf) = (x - xi, y - yi);
    let (x0, y0) = (xi as i32, yi as i32);
    let u = xf * xf * (3.0 - 2.0 * xf);
    let v = yf * yf * (3.0 - 2.0 * yf);
    let a = rand01(x0, y0, seed);
    let b = rand01(x0 + 1, y0, seed);
    let c = rand01(x0, y0 + 1, seed);
    let d = rand01(x0 + 1, y0 + 1, seed);
    lerp(lerp(a, b, u), lerp(c, d, u), v)
}

/// Fractal Brownian motion, 0..1.
pub fn fbm(x: f32, y: f32, seed: u32, octaves: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut total = 0.0;
    let mut f = 1.0;
    for i in 0..octaves {
        sum += amp * value_noise(x * f, y * f, seed.wrapping_add(i * 1013));
        total += amp;
        amp *= 0.5;
        f *= 2.03;
    }
    sum / total
}

/// Worley / cellular noise: (distance to nearest, distance to second nearest, cell id hash).
pub fn worley(x: f32, y: f32, seed: u32) -> (f32, f32, u32) {
    let xi = x.floor() as i32;
    let yi = y.floor() as i32;
    let mut f1 = f32::MAX;
    let mut f2 = f32::MAX;
    let mut id = 0;
    for dy in -1..=1 {
        for dx in -1..=1 {
            let cx = xi + dx;
            let cy = yi + dy;
            let px = cx as f32 + rand01(cx, cy, seed);
            let py = cy as f32 + rand01(cx, cy, seed ^ 0x55aa);
            let d = ((px - x).powi(2) + (py - y).powi(2)).sqrt();
            if d < f1 {
                f2 = f1;
                f1 = d;
                id = hash2(cx, cy, seed);
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    (f1, f2, id)
}

/// Scalar texture value 0..1 at texture-space coordinates.
pub fn sample(kind: TextureKind, x: f32, y: f32, seed: u32) -> f32 {
    match kind {
        TextureKind::Clouds => fbm(x / 70.0, y / 70.0, seed, 5),
        TextureKind::Wood => {
            let warp = fbm(x / 140.0, y / 50.0, seed, 3) * 5.0;
            let rings = (y / 16.0 + warp).fract();
            let ring = smoothstep(0.0, 0.7, rings) * (1.0 - smoothstep(0.8, 1.0, rings));
            let grain = value_noise(x / 45.0, y / 1.3, seed + 7);
            (0.7 * ring + 0.3 * grain).clamp(0.0, 1.0)
        }
        TextureKind::Marble => {
            let t = fbm(x / 90.0, y / 90.0, seed, 5) * 9.0;
            let v = ((x / 45.0 + y / 90.0 + t).sin()).abs();
            v.powf(0.35)
        }
        TextureKind::Canvas => {
            let wx = (x * 1.25).sin();
            let wy = (y * 1.25).sin();
            let over = if ((x / 2.5) as i32 + (y / 2.5) as i32) % 2 == 0 { wx } else { wy };
            (0.55 + 0.25 * over + 0.2 * (value_noise(x / 2.0, y / 2.0, seed) - 0.5)).clamp(0.0, 1.0)
        }
        TextureKind::Paper => {
            (0.75 + 0.45 * (fbm(x / 2.5, y / 2.5, seed, 3) - 0.5)
                + 0.25 * (fbm(x / 25.0, y / 25.0, seed + 3, 3) - 0.5))
                .clamp(0.0, 1.0)
        }
        TextureKind::BrushedMetal => {
            let a = value_noise(x / 240.0, y / 0.9, seed);
            let b = value_noise(x / 35.0, y / 0.6, seed + 1);
            (0.35 + 0.4 * a + 0.25 * b).clamp(0.0, 1.0)
        }
        TextureKind::Cells => {
            let (f1, f2, id) = worley(x / 38.0, y / 38.0, seed);
            let border = smoothstep(0.0, 0.12, f2 - f1);
            let tone = 0.55 + 0.35 * ((id & 0xff) as f32 / 255.0);
            let speck = fbm(x / 4.0, y / 4.0, seed + 9, 2);
            (border * (tone * 0.8 + speck * 0.2)).clamp(0.0, 1.0)
        }
        // pictures are sampled by the renderer itself
        TextureKind::Image => 0.5,
    }
}

/// Generate a premultiplied texture between colours `a` (value 0) and `b` (value 1).
pub fn generate(
    kind: TextureKind,
    w: u32,
    h: u32,
    scale: f32,
    a: Rgba,
    b: Rgba,
    seed: u32,
) -> Option<Pixmap> {
    let mut pm = Pixmap::new(w.max(1), h.max(1))?;
    let s = 1.0 / scale.max(0.05);
    let ca = a.map(|v| v as f32 / 255.0);
    let cb = b.map(|v| v as f32 / 255.0);
    let wd = w as usize;
    let data = pm.data_mut();
    for y in 0..h as usize {
        for x in 0..wd {
            let t = sample(kind, x as f32 * s, y as f32 * s, seed);
            let i = (y * wd + x) * 4;
            let al = lerp(ca[3], cb[3], t);
            for c in 0..3 {
                data[i + c] = (lerp(ca[c], cb[c], t) * al * 255.0 + 0.5) as u8;
            }
            data[i + 3] = (al * 255.0 + 0.5) as u8;
        }
    }
    Some(pm)
}

/// Tiny deterministic RNG.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next_u32(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 32) as u32
    }
    pub fn f(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / 16_777_216.0
    }
}
