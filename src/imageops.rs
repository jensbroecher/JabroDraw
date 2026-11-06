//! Imported pictures: loading, transparency masks, background removal (border flood +
//! magic wand) with feathered, halo-free edges.

use crate::raster::gaussian;
use std::collections::VecDeque;
use std::path::Path;
use tiny_skia::Pixmap;

pub const IMAGE_EXTENSIONS: [&str; 26] = [
    "png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff", "tga", "ico", "qoi", "jxl", "avif", "heic", "heif", "hif", "jxr",
    "wdp", "hdp", "dng", "cr2", "cr3", "nef", "arw", "orf", "rw2",
];

pub fn is_image_path(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// JPEG XL, decoded in pure Rust (jxl-oxide).
fn load_jxl(path: &Path) -> Result<image::RgbaImage, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let dec = jxl_oxide::integration::JxlDecoder::new(std::io::BufReader::new(file)).map_err(|e| e.to_string())?;
    let img = image::DynamicImage::from_decoder(dec).map_err(|e| e.to_string())?;
    Ok(img.to_rgba8())
}

/// Pixels of a picture file: the built-in decoders first, then Windows' own codecs for the
/// rest (HEIC, AVIF, camera RAW…).
fn decode(path: &Path) -> Result<image::RgbaImage, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext == "jxl" {
        return load_jxl(path);
    }
    match image::open(path) {
        Ok(img) => Ok(img.to_rgba8()),
        Err(e) => crate::wic::load(path).map_err(|w| if crate::wic::EXTENSIONS.contains(&ext.as_str()) { w } else { e.to_string() }),
    }
}

/// Decode any common image file into a premultiplied pixmap (at most 4096 px per side).
pub fn load_image(path: &Path) -> Result<Pixmap, String> {
    let mut rgba = decode(path)?;
    let (w, h) = rgba.dimensions();
    let max = w.max(h);
    if max > 4096 {
        let k = 4096.0 / max as f32;
        let (nw, nh) = (((w as f32 * k) as u32).max(1), ((h as f32 * k) as u32).max(1));
        rgba = image::imageops::resize(&rgba, nw, nh, image::imageops::FilterType::Triangle);
    }
    let (w, h) = rgba.dimensions();
    let mut pm = Pixmap::new(w, h).ok_or("empty image")?;
    for (d, s) in pm.data_mut().chunks_exact_mut(4).zip(rgba.as_raw().chunks_exact(4)) {
        let a = s[3] as u32;
        d[0] = ((s[0] as u32 * a + 127) / 255) as u8;
        d[1] = ((s[1] as u32 * a + 127) / 255) as u8;
        d[2] = ((s[2] as u32 * a + 127) / 255) as u8;
        d[3] = s[3];
    }
    Ok(pm)
}

/// A fully visible (white) mask.
pub fn new_mask(w: u32, h: u32) -> Option<Pixmap> {
    let mut m = Pixmap::new(w.max(1), h.max(1))?;
    m.fill(tiny_skia::Color::WHITE);
    Some(m)
}

pub fn invert_mask(m: &mut Pixmap) {
    for px in m.data_mut().chunks_exact_mut(4) {
        let v = 255 - px[0];
        px[0] = v;
        px[1] = v;
        px[2] = v;
        px[3] = 255;
    }
}

#[inline]
fn rgb_at(img: &[u8], i: usize) -> ([f32; 3], f32) {
    let a = img[i * 4 + 3] as f32 / 255.0;
    if a <= 0.0 {
        return ([0.0; 3], 0.0);
    }
    (
        [
            img[i * 4] as f32 / 255.0 / a,
            img[i * 4 + 1] as f32 / 255.0 / a,
            img[i * 4 + 2] as f32 / 255.0 / a,
        ],
        a,
    )
}

#[inline]
fn color_dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    (((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)) / 3.0).sqrt()
}

/// Typical background colour: the per-channel median of the image border.
pub fn border_color(img: &Pixmap) -> [f32; 3] {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let d = img.data();
    let mut ch: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut push = |i: usize| {
        let (c, a) = rgb_at(d, i);
        if a > 0.5 {
            for k in 0..3 {
                ch[k].push(c[k]);
            }
        }
    };
    for x in 0..w {
        push(x);
        push((h - 1) * w + x);
    }
    for y in 0..h {
        push(y * w);
        push(y * w + w - 1);
    }
    let mut out = [1.0; 3];
    for k in 0..3 {
        if !ch[k].is_empty() {
            ch[k].sort_by(f32::total_cmp);
            out[k] = ch[k][ch[k].len() / 2];
        }
    }
    out
}

/// Connected region (4-neighbour) starting at `seeds` whose colours are within `tol` (0..1) of
/// `reference`. Already transparent pixels always belong to the region.
pub fn flood_region(img: &Pixmap, seeds: &[(usize, usize)], reference: [f32; 3], tol: f32) -> Vec<bool> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let d = img.data();
    let mut inside = vec![false; w * h];
    let accept = |i: usize| {
        let (c, a) = rgb_at(d, i);
        a < 0.05 || color_dist(c, reference) <= tol
    };
    let mut queue = VecDeque::new();
    for &(x, y) in seeds {
        if x < w && y < h {
            let i = y * w + x;
            if !inside[i] && accept(i) {
                inside[i] = true;
                queue.push_back(i);
            }
        }
    }
    while let Some(i) = queue.pop_front() {
        let (x, y) = (i % w, i / w);
        let mut visit = |j: usize| {
            if !inside[j] && accept(j) {
                inside[j] = true;
                queue.push_back(j);
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
    inside
}

/// Hide `region` in the mask. `feather` (px) softens the cut edge on the kept side, which also
/// swallows the 1-2 px colour fringe photos usually have around the subject.
pub fn hide_region(mask: &mut Pixmap, region: &[bool], feather: f32) {
    let (w, h) = (mask.width() as usize, mask.height() as usize);
    if region.len() != w * h {
        return;
    }
    let mut removed: Vec<f32> = region.iter().map(|&r| if r { 1.0 } else { 0.0 }).collect();
    if feather > 0.0 {
        gaussian(&mut removed, w, h, feather * 0.5, true);
    }
    for (i, px) in mask.data_mut().chunks_exact_mut(4).enumerate() {
        let keep = if region[i] { 0.0 } else { (1.0 - removed[i] * 1.6).clamp(0.0, 1.0) };
        let v = (px[0] as f32 * keep + 0.5) as u8;
        px[0] = v;
        px[1] = v;
        px[2] = v;
        px[3] = 255;
    }
}

/// Automatic background removal: whatever touches the border and looks like the border colour.
pub fn remove_background(img: &Pixmap, mask: &mut Pixmap, tol: f32, feather: f32) -> usize {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let bg = border_color(img);
    let mut seeds = Vec::new();
    for x in 0..w {
        seeds.push((x, 0));
        seeds.push((x, h - 1));
    }
    for y in 0..h {
        seeds.push((0, y));
        seeds.push((w - 1, y));
    }
    let region = flood_region(img, &seeds, bg, tol);
    let count = region.iter().filter(|r| **r).count();
    hide_region(mask, &region, feather);
    count
}

/// Magic wand: hide the connected area of similar colour around one pixel.
pub fn magic_wand(img: &Pixmap, mask: &mut Pixmap, x: usize, y: usize, tol: f32, feather: f32) -> usize {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if x >= w || y >= h {
        return 0;
    }
    let (seed, _) = rgb_at(img.data(), y * w + x);
    let region = flood_region(img, &[(x, y)], seed, tol);
    let count = region.iter().filter(|r| **r).count();
    hide_region(mask, &region, feather);
    count
}

/// Brightness (0..1) of a picture stretched over a w x h box, at local point (x, y).
pub fn luminance_at(img: &Pixmap, x: f32, y: f32, w: f32, h: f32) -> f32 {
    let iw = img.width() as f32;
    let ih = img.height() as f32;
    let u = (x / w.max(1e-3) * iw).clamp(0.5, iw - 0.5);
    let v = (y / h.max(1e-3) * ih).clamp(0.5, ih - 0.5);
    let c = crate::raster::sample_bilinear(img.data(), img.width() as i32, img.height() as i32, u, v);
    // transparent parts of the picture count as dark
    (0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]) / 255.0
}
