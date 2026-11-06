//! Export: PNG / JPEG / WebP at 1–8× resolution, layered PSD, and SVG (real vectors where SVG
//! can express the object, embedded pictures where it can't).

use crate::canvas;
use crate::doc::*;
use crate::geom::Pt;
use crate::npr;
use crate::render::Renderer;
use crate::shapes;
use crate::vector;
use base64::Engine;
use std::fmt::Write as _;
use std::path::Path;
use tiny_skia::{PathSegment, Pixmap};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Webp,
    Svg,
    Psd,
}

impl Format {
    pub const ALL: [Format; 5] = [Format::Png, Format::Jpeg, Format::Webp, Format::Svg, Format::Psd];
    pub fn label(self) -> &'static str {
        match self {
            Format::Png => "PNG — lossless, transparency",
            Format::Jpeg => "JPEG — small photos, no transparency",
            Format::Webp => "WebP — lossless, transparency",
            Format::Svg => "SVG — vectors (effects as pictures)",
            Format::Psd => "PSD — one layer per object",
        }
    }
    pub fn ext(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpg",
            Format::Webp => "webp",
            Format::Svg => "svg",
            Format::Psd => "psd",
        }
    }
    pub fn raster(self) -> bool {
        matches!(self, Format::Png | Format::Jpeg | Format::Webp)
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    pub format: Format,
    /// Resolution multiplier (1 = canvas size).
    pub scale: f32,
    pub transparent: bool,
    /// Apply the NPR look (raster formats).
    pub npr: bool,
    pub selection_only: bool,
    pub quality: u8,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            format: Format::Png,
            scale: 1.0,
            transparent: false,
            npr: true,
            selection_only: false,
            quality: 90,
        }
    }
}

/// Everything that is measured in pixels grows with the export resolution.
pub fn scale_object(o: &mut Object, k: f32) {
    o.quad = o.quad.map(|c| c * k);
    o.size = [(o.size[0] * k).round().max(1.0), (o.size[1] * k).round().max(1.0)];
    o.stroke.width *= k;
    o.effects.bevel.width *= k;
    o.effects.bevel.softness *= k;
    o.effects.shadow.dx *= k;
    o.effects.shadow.dy *= k;
    o.effects.shadow.blur *= k;
    o.effects.glow.size *= k;
    o.fill.tex_scale *= k;
    o.transparency.tex_scale *= k;
    o.touch();
}

/// The document as it will be exported: optionally only `ids` (cropped to them), scaled by `k`.
pub fn prepare(doc: &Document, ids: Option<&[u64]>, k: f32) -> Result<Document, String> {
    let mut d = doc.clone();
    d.guides.clear();
    if let Some(ids) = ids {
        d.objects.retain(|o| ids.contains(&o.id));
        let (lo, hi) = canvas::content_bounds(&d).ok_or("nothing selected to export")?;
        d.width = ((hi.x - lo.x).ceil() as u32).max(1);
        d.height = ((hi.y - lo.y).ceil() as u32).max(1);
        for o in &mut d.objects {
            o.quad = o.quad.map(|c| c - lo);
        }
    }
    if (k - 1.0).abs() > 1e-3 {
        d.width = ((d.width as f32 * k).round() as u32).max(1);
        d.height = ((d.height as f32 * k).round() as u32).max(1);
        for o in &mut d.objects {
            scale_object(o, k);
        }
    }
    if d.width > 20_000 || d.height > 20_000 || d.width as u64 * d.height as u64 > 200_000_000 {
        return Err(format!("{} × {} px is too large — pick a smaller scale", d.width, d.height));
    }
    Ok(d)
}

/// Finished raster picture.
pub fn render(doc: &Document, ids: Option<&[u64]>, opts: &Options) -> Result<Pixmap, String> {
    let d = prepare(doc, ids, opts.scale)?;
    let mut r = Renderer::default();
    let pm = r.render_opts(&d, !opts.transparent);
    if !(opts.npr && d.any_look()) {
        return Ok(pm);
    }
    let runs = r.render_runs(&d);
    let mut out = npr::compose(&runs, d.width, d.height, d.background, opts.scale);
    if opts.transparent {
        // the look is painted on white paper; keep the original transparency
        for (o, src) in out.data_mut().chunks_exact_mut(4).zip(pm.data().chunks_exact(4)) {
            let a = src[3] as f32 / 255.0;
            for c in 0..3 {
                o[c] = (o[c] as f32 * a + 0.5) as u8;
            }
            o[3] = src[3];
        }
    }
    Ok(out)
}

fn straight_rgba(pm: &Pixmap) -> Vec<u8> {
    let mut v = Vec::with_capacity(pm.data().len());
    for px in pm.data().chunks_exact(4) {
        let a = px[3] as f32 / 255.0;
        let un = |c: u8| if a > 0.0 { (c as f32 / a + 0.5).min(255.0) as u8 } else { 0 };
        v.extend_from_slice(&[un(px[0]), un(px[1]), un(px[2]), px[3]]);
    }
    v
}

fn rgb_over(pm: &Pixmap, bg: Rgba) -> Vec<u8> {
    let mut v = Vec::with_capacity(pm.data().len() / 4 * 3);
    for px in pm.data().chunks_exact(4) {
        let inv = 1.0 - px[3] as f32 / 255.0;
        for c in 0..3 {
            v.push((px[c] as f32 + bg[c] as f32 * inv + 0.5).min(255.0) as u8);
        }
    }
    v
}

pub fn save_raster(pm: &Pixmap, path: &Path, opts: &Options, bg: Rgba) -> Result<(), String> {
    use image::ExtendedColorType;
    let file = || std::fs::File::create(path).map(std::io::BufWriter::new).map_err(|e| e.to_string());
    match opts.format {
        Format::Png => pm.save_png(path).map_err(|e| e.to_string()),
        Format::Jpeg => {
            let bg = if opts.transparent { [255, 255, 255, 255] } else { bg };
            let rgb = rgb_over(pm, bg);
            image::codecs::jpeg::JpegEncoder::new_with_quality(file()?, opts.quality.clamp(1, 100))
                .encode(&rgb, pm.width(), pm.height(), ExtendedColorType::Rgb8)
                .map_err(|e| e.to_string())
        }
        Format::Webp => image::codecs::webp::WebPEncoder::new_lossless(file()?)
            .encode(&straight_rgba(pm), pm.width(), pm.height(), ExtendedColorType::Rgba8)
            .map_err(|e| e.to_string()),
        _ => Err("not a picture format".into()),
    }
}

// -------------------------------------------------------------------------------- PSD

fn be16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_be_bytes());
}
fn be32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_be_bytes());
}

struct PsdLayer {
    name: String,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    rgba: Vec<u8>,
    opacity: u8,
    visible: bool,
}

/// Layered Photoshop file: a background layer plus one layer per object (bottom to top).
pub fn write_psd(doc: &Document, ids: Option<&[u64]>, opts: &Options, path: &Path) -> Result<(), String> {
    let d = prepare(doc, ids, opts.scale)?;
    let (w, h) = (d.width, d.height);
    let mut r = Renderer::default();
    let merged = r.render_opts(&d, !opts.transparent);
    let mut layers = Vec::new();
    if !opts.transparent {
        let mut bg = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            bg.extend_from_slice(&[d.background[0], d.background[1], d.background[2], 255]);
        }
        layers.push(PsdLayer {
            name: "Background".into(),
            x: 0,
            y: 0,
            w,
            h,
            rgba: bg,
            opacity: 255,
            visible: true,
        });
    }
    for o in &d.objects {
        if let Some((pm, x, y)) = r.object_layer(o, w, h) {
            layers.push(PsdLayer {
                name: o.name.clone(),
                x,
                y,
                w: pm.width(),
                h: pm.height(),
                rgba: straight_rgba(&pm),
                opacity: (o.opacity.clamp(0.0, 1.0) * 255.0).round() as u8,
                visible: o.visible,
            });
        }
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"8BPS");
    be16(&mut out, 1);
    out.extend_from_slice(&[0; 6]);
    be16(&mut out, 3);
    be32(&mut out, h);
    be32(&mut out, w);
    be16(&mut out, 8);
    be16(&mut out, 3); // RGB
    be32(&mut out, 0); // colour mode data
    be32(&mut out, 0); // image resources

    let mut info = Vec::new();
    be16(&mut info, layers.len() as u16);
    for l in &layers {
        be32(&mut info, l.y as u32);
        be32(&mut info, l.x as u32);
        be32(&mut info, (l.y + l.h as i32) as u32);
        be32(&mut info, (l.x + l.w as i32) as u32);
        be16(&mut info, 4);
        for id in [-1i16, 0, 1, 2] {
            be16(&mut info, id as u16);
            be32(&mut info, 2 + l.w * l.h);
        }
        info.extend_from_slice(b"8BIMnorm");
        info.extend_from_slice(&[l.opacity, 0, if l.visible { 0 } else { 2 }, 0]);
        let name: Vec<u8> = l.name.chars().map(|c| if c.is_ascii() { c as u8 } else { b'?' }).take(255).collect();
        let padded = (1 + name.len()).div_ceil(4) * 4;
        be32(&mut info, (4 + 4 + padded) as u32);
        be32(&mut info, 0); // layer mask
        be32(&mut info, 0); // blending ranges
        info.push(name.len() as u8);
        info.extend_from_slice(&name);
        info.resize(info.len() + padded - 1 - name.len(), 0);
    }
    for l in &layers {
        for ch in [3usize, 0, 1, 2] {
            be16(&mut info, 0); // raw
            info.extend(l.rgba.iter().skip(ch).step_by(4));
        }
    }
    if info.len() % 2 == 1 {
        info.push(0);
    }
    be32(&mut out, (4 + info.len() + 4) as u32);
    be32(&mut out, info.len() as u32);
    out.extend_from_slice(&info);
    be32(&mut out, 0); // global layer mask
    // merged picture (shown by programs that don't read layers)
    be16(&mut out, 0);
    let flat = rgb_over(&merged, [255, 255, 255, 255]);
    for ch in 0..3 {
        out.extend(flat.iter().skip(ch).step_by(3));
    }
    std::fs::write(path, out).map_err(|e| e.to_string())
}

// -------------------------------------------------------------------------------- SVG

fn hex(c: Rgba) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// Can this object be written as plain SVG (no effects SVG can't express)?
fn plain_vector(o: &Object) -> bool {
    let fx = &o.effects;
    o.kind != ShapeKind::Bitmap
        && o.paint.is_none()
        && o.mask.is_none()
        && !fx.bevel.enabled
        && !fx.shadow.enabled
        && !fx.glow.enabled
        && !fx.lighting.active()
        && o.transparency.kind == TransparencyKind::None
        && o.fill.kind != FillKind::Texture
        && (!o.stroke.enabled || o.stroke.position == StrokePos::Center)
        && o.adjust.is_identity()
        && o.mesh.as_ref().is_none_or(|m| m.is_identity())
}

fn path_data(p: &tiny_skia::Path, map: &dyn Fn(f32, f32) -> (f32, f32)) -> String {
    let mut s = String::new();
    for seg in p.segments() {
        match seg {
            PathSegment::MoveTo(a) => {
                let (x, y) = map(a.x, a.y);
                let _ = write!(s, "M{x:.2} {y:.2}");
            }
            PathSegment::LineTo(a) => {
                let (x, y) = map(a.x, a.y);
                let _ = write!(s, "L{x:.2} {y:.2}");
            }
            PathSegment::QuadTo(a, b) => {
                let (ax, ay) = map(a.x, a.y);
                let (bx, by) = map(b.x, b.y);
                let _ = write!(s, "Q{ax:.2} {ay:.2} {bx:.2} {by:.2}");
            }
            PathSegment::CubicTo(a, b, c) => {
                let (ax, ay) = map(a.x, a.y);
                let (bx, by) = map(b.x, b.y);
                let (cx, cy) = map(c.x, c.y);
                let _ = write!(s, "C{ax:.2} {ay:.2} {bx:.2} {by:.2} {cx:.2} {cy:.2}");
            }
            PathSegment::Close => s.push('Z'),
        }
    }
    s
}

/// SVG: plain shapes and text stay vectors; objects with bevels, lights, paint, textures and
/// similar are embedded as pictures at their place (so the file looks the same).
pub fn write_svg(doc: &Document, ids: Option<&[u64]>, opts: &Options, path: &Path) -> Result<(), String> {
    let d = prepare(doc, ids, 1.0)?;
    let (w, h) = (d.width, d.height);
    let k = opts.scale;
    let mut svg = String::new();
    let _ = writeln!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="0 0 {w} {h}">"#,
        (w as f32 * k).round(),
        (h as f32 * k).round()
    );
    let _ = writeln!(svg, "<!-- Made with Jabro Draw -->");
    if !opts.transparent {
        let _ = writeln!(svg, r#"<rect width="{w}" height="{h}" fill="{}"/>"#, hex(d.background));
    }
    let mut r = Renderer::default();
    let mut defs = String::new();
    let mut body = String::new();
    for (n, o) in d.objects.iter().enumerate().filter(|(_, o)| o.visible) {
        let opacity = if o.opacity < 0.999 { format!(r#" opacity="{:.3}""#, o.opacity) } else { String::new() };
        let geometry = shapes::build_path(&o.kind, o.size[0], o.size[1]);
        if let (true, Some(p)) = (plain_vector(o), geometry) {
            let q = &o.quad;
            let affine = (q[0] + q[2] - q[1] - q[3]).len() < 1e-3;
            let (pw, ph) = (o.size[0].max(1e-3), o.size[1].max(1e-3));
            // affine placements keep exact curves via a transform; perspective is flattened
            let (d_attr, transform) = if affine {
                let (ex, ey) = ((q[1] - q[0]) * (1.0 / pw), (q[3] - q[0]) * (1.0 / ph));
                (
                    path_data(&p, &|x, y| (x, y)),
                    format!(
                        r#" transform="matrix({:.6} {:.6} {:.6} {:.6} {:.3} {:.3})""#,
                        ex.x, ex.y, ey.x, ey.y, q[0].x, q[0].y
                    ),
                )
            } else {
                let hm = o.homography();
                let mut s = String::new();
                for c in vector::flatten(&p, 0.25) {
                    for (i, pt) in c.iter().enumerate() {
                        let m = hm.apply_pt(*pt).unwrap_or(*pt);
                        let _ = write!(s, "{}{:.2} {:.2}", if i == 0 { "M" } else { "L" }, m.x, m.y);
                    }
                    s.push('Z');
                }
                (s, String::new())
            };
            // gradient ends, in the path's own coordinates
            let local = |x: f32, y: f32| -> (f32, f32) {
                if affine {
                    (x, y)
                } else {
                    let m = o.homography().apply_pt(Pt::new(x, y)).unwrap_or(Pt::new(x, y));
                    (m.x, m.y)
                }
            };
            let f = &o.fill;
            let fill = match f.kind {
                FillKind::None => "none".to_string(),
                FillKind::Solid => {
                    format!(r#"{}" fill-opacity="{:.3}"#, hex(f.a), f.a[3] as f32 / 255.0)
                }
                FillKind::Linear | FillKind::Radial => {
                    let id = format!("g{n}");
                    let stops = format!(
                        r#"<stop offset="0" stop-color="{}" stop-opacity="{:.3}"/><stop offset="1" stop-color="{}" stop-opacity="{:.3}"/>"#,
                        hex(f.a),
                        f.a[3] as f32 / 255.0,
                        hex(f.b),
                        f.b[3] as f32 / 255.0
                    );
                    let (cx, cy) = (pw * 0.5, ph * 0.5);
                    if f.kind == FillKind::Linear {
                        let a = f.angle.to_radians();
                        let (dx, dy) = (a.cos(), a.sin());
                        let half = 0.5 * ((pw * dx).abs() + (ph * dy).abs()).max(1.0);
                        let (x1, y1) = local(cx - dx * half, cy - dy * half);
                        let (x2, y2) = local(cx + dx * half, cy + dy * half);
                        let _ = writeln!(
                            defs,
                            r#"<linearGradient id="{id}" gradientUnits="userSpaceOnUse" x1="{x1:.2}" y1="{y1:.2}" x2="{x2:.2}" y2="{y2:.2}">{stops}</linearGradient>"#
                        );
                    } else {
                        let (gx, gy) = local(cx, cy);
                        let rr = 0.5 * (pw * pw + ph * ph).sqrt();
                        let _ = writeln!(
                            defs,
                            r#"<radialGradient id="{id}" gradientUnits="userSpaceOnUse" cx="{gx:.2}" cy="{gy:.2}" r="{rr:.2}">{stops}</radialGradient>"#
                        );
                    }
                    format!("url(#{id})")
                }
                FillKind::Texture => unreachable!(),
            };
            let s = &o.stroke;
            let stroke = if s.enabled {
                let dash = match s.dash {
                    DashKind::Solid => String::new(),
                    DashKind::Dashed => format!(r#" stroke-dasharray="{} {}""#, s.width * 3.0, s.width * 2.0),
                    DashKind::Dotted => format!(r#" stroke-dasharray="0.01 {}""#, s.width * 2.2),
                };
                format!(
                    r#" stroke="{}" stroke-opacity="{:.3}" stroke-width="{}" stroke-linejoin="round" stroke-linecap="round"{dash}"#,
                    hex(s.color),
                    s.color[3] as f32 / 255.0,
                    s.width
                )
            } else {
                String::new()
            };
            let _ = writeln!(body, r#"<path d="{d_attr}" fill="{fill}"{stroke}{transform}{opacity}/>"#);
        } else if let Some((pm, x, y)) = r.object_layer(o, w, h) {
            let png = pm.encode_png().map_err(|e| e.to_string())?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(png);
            let _ = writeln!(
                body,
                r#"<image x="{x}" y="{y}" width="{}" height="{}"{opacity} href="data:image/png;base64,{b64}"/>"#,
                pm.width(),
                pm.height()
            );
        }
    }
    if !defs.is_empty() {
        let _ = writeln!(svg, "<defs>\n{defs}</defs>");
    }
    svg.push_str(&body);
    svg.push_str("</svg>\n");
    std::fs::write(path, svg).map_err(|e| e.to_string())
}

/// Export according to `opts` (selection only when `ids` is given).
pub fn export(doc: &Document, ids: Option<&[u64]>, opts: &Options, path: &Path) -> Result<(), String> {
    match opts.format {
        Format::Svg => write_svg(doc, ids, opts, path),
        Format::Psd => write_psd(doc, ids, opts, path),
        _ => {
            let pm = render(doc, ids, opts)?;
            save_raster(&pm, path, opts, doc.background)
        }
    }
}
