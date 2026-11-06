//! Pixel utilities: fast gaussian blur, bilinear sampling, projective warping.

use crate::geom::Homography;
use tiny_skia::Pixmap;

fn box_h(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize, clamp: bool) {
    let norm = 1.0 / (2 * r + 1) as f32;
    let ri = r as isize;
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        let out = &mut dst[y * w..(y + 1) * w];
        let at = |i: isize| -> f32 {
            if i < 0 {
                if clamp {
                    row[0]
                } else {
                    0.0
                }
            } else if i >= w as isize {
                if clamp {
                    row[w - 1]
                } else {
                    0.0
                }
            } else {
                row[i as usize]
            }
        };
        let mut acc = 0.0f32;
        for i in -ri..=ri {
            acc += at(i);
        }
        for (x, o) in out.iter_mut().enumerate() {
            *o = acc * norm;
            let xi = x as isize;
            acc += at(xi + ri + 1) - at(xi - ri);
        }
    }
}

fn box_v(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize, clamp: bool) {
    let norm = 1.0 / (2 * r + 1) as f32;
    let ri = r as isize;
    let mut col = vec![0.0f32; h];
    for x in 0..w {
        for (y, c) in col.iter_mut().enumerate() {
            *c = src[y * w + x];
        }
        let at = |i: isize| -> f32 {
            if i < 0 {
                if clamp {
                    col[0]
                } else {
                    0.0
                }
            } else if i >= h as isize {
                if clamp {
                    col[h - 1]
                } else {
                    0.0
                }
            } else {
                col[i as usize]
            }
        };
        let mut acc = 0.0f32;
        for i in -ri..=ri {
            acc += at(i);
        }
        for y in 0..h {
            dst[y * w + x] = acc * norm;
            let yi = y as isize;
            acc += at(yi + ri + 1) - at(yi - ri);
        }
    }
}

fn box_radii(sigma: f32) -> [usize; 3] {
    let n = 3.0f32;
    let w_ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut wl = w_ideal.floor() as i32;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wl = wl.max(1);
    let wu = wl + 2;
    let m_ideal = (12.0 * sigma * sigma - n * (wl * wl) as f32 - 4.0 * n * wl as f32 - 3.0 * n)
        / (-4.0 * wl as f32 - 4.0);
    let m = m_ideal.round() as i32;
    let mut out = [0usize; 3];
    for (i, o) in out.iter_mut().enumerate() {
        let size = if (i as i32) < m { wl } else { wu };
        *o = ((size - 1) / 2).max(0) as usize;
    }
    out
}

/// Approximate gaussian blur (3 box passes). `clamp` = extend edges, otherwise zero outside.
pub fn gaussian(buf: &mut [f32], w: usize, h: usize, sigma: f32, clamp: bool) {
    if sigma < 0.35 || w == 0 || h == 0 {
        return;
    }
    let mut tmp = vec![0.0f32; buf.len()];
    for r in box_radii(sigma) {
        if r == 0 {
            continue;
        }
        box_h(buf, &mut tmp, w, h, r, clamp);
        box_v(&tmp, buf, w, h, r, clamp);
    }
}

/// Bilinear sample of a premultiplied RGBA8 buffer at continuous pixel coords (pixel centres at i+0.5).
#[inline]
pub fn sample_bilinear(src: &[u8], w: i32, h: i32, x: f32, y: f32) -> [f32; 4] {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = fx.floor() as i32;
    let y0 = fy.floor() as i32;
    let tx = fx - x0 as f32;
    let ty = fy - y0 as f32;
    let mut out = [0.0f32; 4];
    let taps = [
        (x0, y0, (1.0 - tx) * (1.0 - ty)),
        (x0 + 1, y0, tx * (1.0 - ty)),
        (x0, y0 + 1, (1.0 - tx) * ty),
        (x0 + 1, y0 + 1, tx * ty),
    ];
    for (sx, sy, wt) in taps {
        if wt <= 0.0 || sx < 0 || sy < 0 || sx >= w || sy >= h {
            continue;
        }
        let i = ((sy * w + sx) * 4) as usize;
        for c in 0..4 {
            out[c] += src[i + c] as f32 * wt;
        }
    }
    out
}

/// Warp a padded local raster onto the canvas through a projective transform.
/// Returns the warped layer and its top-left canvas position. `margin` adds empty space
/// around the result (room for shadows / glows).
pub fn warp(
    local: &Pixmap,
    pad: f32,
    fwd: &Homography,
    dw: u32,
    dh: u32,
    margin: i32,
) -> Option<(Pixmap, i32, i32)> {
    let lw = local.width() as f32;
    let lh = local.height() as f32;
    let corners = [
        (-pad, -pad),
        (lw - pad, -pad),
        (lw - pad, lh - pad),
        (-pad, lh - pad),
    ];
    let (mut minx, mut miny, mut maxx, mut maxy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (x, y) in corners {
        let (cx, cy) = fwd.apply(x as f64, y as f64)?;
        minx = minx.min(cx);
        miny = miny.min(cy);
        maxx = maxx.max(cx);
        maxy = maxy.max(cy);
    }
    if !minx.is_finite() || !maxx.is_finite() || !miny.is_finite() || !maxy.is_finite() {
        return None;
    }
    let m = margin as i64;
    let x0 = ((minx.floor() as i64) - m).max(-m);
    let y0 = ((miny.floor() as i64) - m).max(-m);
    let x1 = ((maxx.ceil() as i64) + m).min(dw as i64 + m);
    let y1 = ((maxy.ceil() as i64) + m).min(dh as i64 + m);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let bw = (x1 - x0) as u32;
    let bh = (y1 - y0) as u32;
    let inv = fwd.inverse()?;
    let mut out = Pixmap::new(bw, bh)?;
    let src = local.data();
    let sw = local.width() as i32;
    let sh = local.height() as i32;
    let od = out.data_mut();
    let im = inv.m;
    let fm = fwd.m;
    let padd = pad as f64;
    for py in 0..bh as usize {
        let cy = (y0 + py as i64) as f64 + 0.5;
        let cx0 = x0 as f64 + 0.5;
        let mut u = im[0] * cx0 + im[1] * cy + im[2];
        let mut v = im[3] * cx0 + im[4] * cy + im[5];
        let mut wz = im[6] * cx0 + im[7] * cy + im[8];
        for px in 0..bw as usize {
            if wz.abs() > 1e-12 {
                let lx = u / wz;
                let ly = v / wz;
                let den = fm[6] * lx + fm[7] * ly + fm[8];
                if den > 0.0 {
                    let sx = (lx + padd) as f32;
                    let sy = (ly + padd) as f32;
                    if sx > -1.0 && sy > -1.0 && sx < lw + 1.0 && sy < lh + 1.0 {
                        let c = sample_bilinear(src, sw, sh, sx, sy);
                        if c[3] > 0.0 {
                            let i = (py * bw as usize + px) * 4;
                            let a = c[3].round().min(255.0);
                            od[i + 3] = a as u8;
                            for k in 0..3 {
                                od[i + k] = c[k].round().min(a) as u8;
                            }
                        }
                    }
                }
            }
            u += im[0];
            v += im[3];
            wz += im[6];
        }
    }
    Some((out, x0 as i32, y0 as i32))
}

/// Like `warp`, but the object is first bent by a mesh (warp tool / liquify). The local
/// raster is cut into small triangles that are each mapped through mesh + perspective.
#[allow(clippy::too_many_arguments)]
pub fn warp_mesh(
    local: &Pixmap,
    pad: f32,
    fwd: &Homography,
    mesh: &crate::doc::Mesh,
    w: f32,
    h: f32,
    dw: u32,
    dh: u32,
    margin: i32,
) -> Option<(Pixmap, i32, i32)> {
    const SUB: usize = 6;
    let (nu, nv) = (mesh.cols * SUB, mesh.rows * SUB);
    let (pu, pv) = (pad / w.max(1.0), pad / h.max(1.0));
    let mut canvas = Vec::with_capacity((nu + 1) * (nv + 1));
    let mut source = Vec::with_capacity((nu + 1) * (nv + 1));
    for j in 0..=nv {
        for i in 0..=nu {
            let u = -pu + (1.0 + 2.0 * pu) * i as f32 / nu as f32;
            let v = -pv + (1.0 + 2.0 * pv) * j as f32 / nv as f32;
            let m = mesh.map(u, v);
            let c = fwd.apply((m[0] * w) as f64, (m[1] * h) as f64)?;
            canvas.push((c.0 as f32, c.1 as f32));
            source.push((u * w + pad, v * h + pad));
        }
    }
    let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in &canvas {
        minx = minx.min(x);
        miny = miny.min(y);
        maxx = maxx.max(x);
        maxy = maxy.max(y);
    }
    if !minx.is_finite() || !maxy.is_finite() {
        return None;
    }
    let m = margin as i64;
    let x0 = ((minx.floor() as i64) - m).max(-m);
    let y0 = ((miny.floor() as i64) - m).max(-m);
    let x1 = ((maxx.ceil() as i64) + m).min(dw as i64 + m);
    let y1 = ((maxy.ceil() as i64) + m).min(dh as i64 + m);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (bw, bh) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let mut out = Pixmap::new(bw as u32, bh as u32)?;
    let (src, sw, sh) = (local.data(), local.width() as i32, local.height() as i32);
    let od = out.data_mut();
    let edge = |a: (f32, f32), b: (f32, f32), p: (f32, f32)| (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0);
    let mut tri = |a: usize, b: usize, c: usize| {
        let (pa, pb, pc) = (canvas[a], canvas[b], canvas[c]);
        let area = edge(pa, pb, pc);
        if area.abs() < 1e-6 {
            return;
        }
        let lx = ((pa.0.min(pb.0).min(pc.0)).floor() as i64 - x0).max(0) as usize;
        let ly = ((pa.1.min(pb.1).min(pc.1)).floor() as i64 - y0).max(0) as usize;
        let hx = (((pa.0.max(pb.0).max(pc.0)).ceil() as i64 - x0).max(0) as usize).min(bw);
        let hy = (((pa.1.max(pb.1).max(pc.1)).ceil() as i64 - y0).max(0) as usize).min(bh);
        let (sa, sb, sc) = (source[a], source[b], source[c]);
        for py in ly..hy {
            for px in lx..hx {
                let p = ((x0 + px as i64) as f32 + 0.5, (y0 + py as i64) as f32 + 0.5);
                let w0 = edge(pb, pc, p) / area;
                let w1 = edge(pc, pa, p) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                    continue;
                }
                let sx = sa.0 * w0 + sb.0 * w1 + sc.0 * w2;
                let sy = sa.1 * w0 + sb.1 * w1 + sc.1 * w2;
                let col = sample_bilinear(src, sw, sh, sx, sy);
                if col[3] > 0.0 {
                    let i = (py * bw + px) * 4;
                    let al = col[3].round().min(255.0);
                    od[i + 3] = al as u8;
                    for k in 0..3 {
                        od[i + k] = col[k].round().min(al) as u8;
                    }
                }
            }
        }
    };
    for j in 0..nv {
        for i in 0..nu {
            let a = j * (nu + 1) + i;
            let (b, c, d) = (a + 1, a + nu + 1, a + nu + 2);
            tri(a, b, d);
            tri(a, d, c);
        }
    }
    Some((out, x0 as i32, y0 as i32))
}

/// Extract the alpha channel as 0..1 floats.
pub fn alpha_of(pm: &Pixmap) -> Vec<f32> {
    pm.data().chunks_exact(4).map(|p| p[3] as f32 / 255.0).collect()
}
