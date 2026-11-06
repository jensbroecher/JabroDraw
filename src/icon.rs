//! The Jabro Draw icon, drawn with Jabro Draw itself: a rounded gradient tile with a beveled,
//! 3D-extruded "J". The J is our own outline (no font involved). `--make-icon <dir>` renders it
//! at every Windows icon size and writes `icon.ico` (used by build.rs for the .exe) and PNGs.

use crate::doc::*;
use crate::export::{self, Options};
use crate::geom::Pt;
use std::path::Path;
use tiny_skia::Pixmap;

/// Outline of the "J" on the 256 x 256 icon canvas, clockwise.
fn j_outline() -> Vec<[f64; 2]> {
    let mut pts: Vec<[f64; 2]> = vec![[100.0, 40.0], [196.0, 40.0], [196.0, 80.0], [174.0, 80.0], [174.0, 146.0]];
    let arc = |pts: &mut Vec<[f64; 2]>, c: (f64, f64), r: f64, a0: f64, a1: f64| {
        let n = 24;
        for i in 0..=n {
            let t = a0 + (a1 - a0) * i as f64 / n as f64;
            pts.push([c.0 + r * t.cos(), c.1 + r * t.sin()]);
        }
    };
    use std::f64::consts::PI;
    // outer curve of the hook, the round end, then the inner curve back up
    arc(&mut pts, (112.0, 146.0), 62.0, 0.0, PI);
    arc(&mut pts, (73.0, 146.0), 23.0, PI, 2.0 * PI);
    arc(&mut pts, (112.0, 146.0), 16.0, PI, 0.0);
    pts.extend([[128.0, 80.0], [100.0, 80.0]]);
    pts
}

pub fn icon_document() -> Document {
    let mut d = Document::new(256, 256);
    d.background = [0, 0, 0, 0];
    // the tile
    let id = d.alloc_id();
    let mut tile = Object::new(id, ShapeKind::Rect { corner: 0.44 }, 10.0, 10.0, 236.0, 236.0);
    tile.fill = FillStyle {
        kind: FillKind::Linear,
        a: [92, 46, 255, 255],
        b: [255, 120, 60, 255],
        stops: vec![(0.0, [92, 46, 255, 255]), (0.55, [200, 60, 220, 255]), (1.0, [255, 140, 60, 255])],
        points: Some(([0.0, 0.0], [1.0, 1.0])),
        ..Default::default()
    };
    tile.stroke.enabled = false;
    tile.effects.bevel = Bevel { enabled: true, width: 14.0, depth: 0.5, softness: 3.0, intensity: 0.55, ..Default::default() };
    tile.name = "Tile".into();
    d.objects.push(tile);
    // the J
    let (subpaths, quad, size) = crate::vector::polygons_to_object_parts(&vec![vec![j_outline()]]).expect("J outline");
    let id = d.alloc_id();
    let mut j = Object::new(id, ShapeKind::Path { subpaths }, 0.0, 0.0, size[0], size[1]);
    j.quad = quad;
    j.update_size_from_quad();
    j.fill = FillStyle {
        kind: FillKind::Linear,
        a: [255, 255, 255, 255],
        b: [255, 232, 200, 255],
        points: Some(([0.0, 0.0], [0.0, 1.0])),
        ..Default::default()
    };
    j.stroke.enabled = false;
    j.effects.bevel = Bevel { enabled: true, width: 7.0, depth: 0.9, softness: 1.5, intensity: 0.7, ..Default::default() };
    j.effects.extrude = Extrude {
        enabled: true,
        depth: 14.0,
        angle: 300.0,
        auto_color: false,
        color: [70, 30, 150, 255],
        shade: 0.6,
        ..Default::default()
    };
    j.effects.shadow = Shadow { enabled: true, color: [40, 0, 60, 255], dx: 6.0, dy: 9.0, blur: 10.0, opacity: 0.45 };
    j.name = "J".into();
    // a little to the left so the 3D side fits inside the tile
    j.translate(Pt::new(-10.0, -4.0));
    d.objects.push(j);
    d
}

/// The icon at `size` x `size` pixels (re-rendered at that size, so small icons stay crisp).
pub fn render(size: u32) -> Result<Pixmap, String> {
    let opts = Options { scale: size as f32 / 256.0, transparent: true, npr: false, ..Default::default() };
    export::render(&icon_document(), None, &opts)
}

/// Windows icon sizes, largest first.
pub const SIZES: [u32; 8] = [256, 128, 64, 48, 40, 32, 24, 16];

fn straight(pm: &Pixmap) -> image::RgbaImage {
    image::RgbaImage::from_raw(pm.width(), pm.height(), export::straight_rgba(pm)).expect("size")
}

/// An .ico with every size: PNG for 256 px, classic 32-bit bitmaps for the rest (understood by
/// every part of Windows).
pub fn ico_bytes(images: &[(u32, Pixmap)]) -> Result<Vec<u8>, String> {
    let mut blobs: Vec<Vec<u8>> = Vec::new();
    for (s, pm) in images {
        let img = straight(pm);
        if *s >= 256 {
            let mut png = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| e.to_string())?;
            blobs.push(png);
        } else {
            let (w, h) = (*s, *s);
            let mut b = Vec::new();
            let mask_row = (w as usize).div_ceil(32) * 4;
            b.extend(40u32.to_le_bytes());
            b.extend((w as i32).to_le_bytes());
            b.extend((2 * h as i32).to_le_bytes()); // colour + mask
            b.extend(1u16.to_le_bytes());
            b.extend(32u16.to_le_bytes());
            b.extend(0u32.to_le_bytes());
            b.extend(((w * h * 4) as u32 + (mask_row * h as usize) as u32).to_le_bytes());
            b.extend([0u8; 16]);
            for y in (0..h).rev() {
                for x in 0..w {
                    let p = img.get_pixel(x, y);
                    b.extend([p[2], p[1], p[0], p[3]]);
                }
            }
            // AND mask: set where fully transparent
            for y in (0..h).rev() {
                let mut row = vec![0u8; mask_row];
                for x in 0..w {
                    if img.get_pixel(x, y)[3] == 0 {
                        row[x as usize / 8] |= 0x80 >> (x % 8);
                    }
                }
                b.extend(row);
            }
            blobs.push(b);
        }
    }
    let mut out = Vec::new();
    out.extend(0u16.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend((images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for ((s, _), blob) in images.iter().zip(&blobs) {
        let dim = if *s >= 256 { 0 } else { *s as u8 };
        out.extend([dim, dim, 0, 0]);
        out.extend(1u16.to_le_bytes());
        out.extend(32u16.to_le_bytes());
        out.extend((blob.len() as u32).to_le_bytes());
        out.extend(offset.to_le_bytes());
        offset += blob.len() as u32;
    }
    for blob in blobs {
        out.extend(blob);
    }
    Ok(out)
}

/// `--make-icon <dir>`: writes icon.ico, icon-256.png and icon-sheet.png (all sizes side by side).
pub fn make_icon(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let images: Vec<(u32, Pixmap)> = SIZES.iter().map(|&s| render(s).map(|p| (s, p))).collect::<Result<_, _>>()?;
    std::fs::write(dir.join("icon.ico"), ico_bytes(&images)?).map_err(|e| e.to_string())?;
    straight(&images[0].1).save(dir.join("icon-256.png")).map_err(|e| e.to_string())?;
    // preview sheet on light and dark backgrounds
    let total: u32 = SIZES.iter().sum::<u32>() + 10 * SIZES.len() as u32 + 10;
    let mut sheet = Pixmap::new(total, 2 * 276).ok_or("sheet")?;
    sheet.fill(tiny_skia::Color::from_rgba8(240, 240, 240, 255));
    let dark = tiny_skia::Rect::from_xywh(0.0, 276.0, total as f32, 276.0).ok_or("rect")?;
    let mut p = tiny_skia::Paint::default();
    p.set_color_rgba8(32, 32, 36, 255);
    sheet.fill_rect(dark, &p, tiny_skia::Transform::identity(), None);
    let mut x = 10;
    for (s, img) in &images {
        for y in [10, 286] {
            sheet.draw_pixmap(x, y, img.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
        }
        x += *s as i32 + 10;
    }
    sheet.save_png(dir.join("icon-sheet.png")).map_err(|e| e.to_string())
}
