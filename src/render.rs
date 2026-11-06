//! Object render pipeline with caching:
//!   local raster  (fill + paint layer + outline + bevel/material + lights + transparency)
//!                                                                         — cached by content revision
//!   -> canvas layer (projective warp + drop shadow + glow)                — cached by placement
//!   -> composite onto the document.

use crate::doc::{
    DashKind, Document, FillKind, FillStyle, LightKind, Object, StrokePos, StrokeStyle, TextureKind,
};
use crate::effects;
use crate::geom::{point_in_poly, Pt};
use crate::raster;
use crate::shapes;
use crate::textures;
use std::collections::{HashMap, HashSet};
use tiny_skia::{
    BlendMode, Color, FillRule, FilterQuality, GradientStop, LineCap, LineJoin, LinearGradient, Mask,
    Paint, Pattern, Pixmap, PixmapPaint, Point, RadialGradient, SpreadMode, Stroke, StrokeDash, Transform,
};

struct Entry {
    rev: u64,
    light_key: i32,
    local: Pixmap,
    pad: f32,
    layer_key: Option<[u32; 10]>,
    layer: Option<(Pixmap, i32, i32)>,
}

#[derive(Default)]
pub struct Renderer {
    cache: HashMap<u64, Entry>,
}

fn color(c: [u8; 4]) -> Color {
    Color::from_rgba8(c[0], c[1], c[2], c[3])
}

fn fill_shader_paint<'a>(
    fill: &FillStyle,
    w: f32,
    h: f32,
    pad: f32,
    tex: Option<(&'a Pixmap, Transform)>,
) -> Option<Paint<'a>> {
    let mut paint = Paint {
        anti_alias: true,
        ..Default::default()
    };
    let c = Point::from_xy(pad + w * 0.5, pad + h * 0.5);
    match fill.kind {
        FillKind::None => return None,
        FillKind::Solid => paint.set_color(color(fill.a)),
        FillKind::Linear => {
            let a = fill.angle.to_radians();
            let (dx, dy) = (a.cos(), a.sin());
            let half = 0.5 * ((w * dx).abs() + (h * dy).abs()).max(1.0);
            let s = Point::from_xy(c.x - dx * half, c.y - dy * half);
            let e = Point::from_xy(c.x + dx * half, c.y + dy * half);
            paint.shader = LinearGradient::new(
                s,
                e,
                vec![GradientStop::new(0.0, color(fill.a)), GradientStop::new(1.0, color(fill.b))],
                SpreadMode::Pad,
                Transform::identity(),
            )?;
        }
        FillKind::Radial => {
            let r = 0.5 * (w * w + h * h).sqrt();
            paint.shader = RadialGradient::new(
                c,
                c,
                r.max(1.0),
                vec![GradientStop::new(0.0, color(fill.a)), GradientStop::new(1.0, color(fill.b))],
                SpreadMode::Pad,
                Transform::identity(),
            )?;
        }
        FillKind::Texture => {
            let (t, ts) = tex?;
            paint.shader = Pattern::new(t.as_ref(), SpreadMode::Repeat, FilterQuality::Bilinear, 1.0, ts);
        }
    }
    Some(paint)
}

fn outline_stroke(s: &StrokeStyle, width: f32) -> Stroke {
    Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        dash: match s.dash {
            DashKind::Solid => None,
            DashKind::Dashed => StrokeDash::new(vec![s.width * 3.0, s.width * 2.0], 0.0),
            DashKind::Dotted => StrokeDash::new(vec![0.01, s.width * 2.2], 0.0),
        },
        ..Default::default()
    }
}

/// Vector outline: centred on the edge, or only its inner / outer half (masked).
fn draw_outline(pm: &mut Pixmap, path: &tiny_skia::Path, s: &StrokeStyle) {
    let mut paint = Paint {
        anti_alias: true,
        ..Default::default()
    };
    paint.set_color(color(s.color));
    let (pw, ph) = (pm.width(), pm.height());
    match s.position {
        StrokePos::Center => {
            pm.stroke_path(path, &paint, &outline_stroke(s, s.width), Transform::identity(), None);
        }
        StrokePos::Inside | StrokePos::Outside => {
            let Some(mut mask) = Mask::new(pw, ph) else { return };
            mask.fill_path(path, FillRule::Winding, true, Transform::identity());
            if s.position == StrokePos::Outside {
                mask.invert();
            }
            let st = outline_stroke(s, s.width * 2.0);
            pm.stroke_path(path, &paint, &st, Transform::identity(), Some(&mask));
        }
    }
}

/// Render an object's content in its own local space (plus padding).
pub fn render_local(obj: &Object, light: [f32; 3], ex: Pt, ey: Pt) -> (Pixmap, f32) {
    let (w, h) = obj.raster_size();
    let (wf, hf) = (w as f32, h as f32);
    let pad = (obj.stroke.outer_extent() + 3.0).ceil();
    let pw = w + 2 * pad as u32;
    let ph = h + 2 * pad as u32;
    let mut pm = match Pixmap::new(pw, ph) {
        Some(p) => p,
        None => return (Pixmap::new(1, 1).unwrap(), 0.0),
    };
    let path = shapes::build_path(&obj.kind, wf, hf)
        .and_then(|p| p.transform(Transform::from_translate(pad, pad)));

    // 1. fill
    if let Some(path) = &path {
        let f = &obj.fill;
        let picture = f.image.as_ref().filter(|_| f.texture == TextureKind::Image);
        let generated = if f.kind == FillKind::Texture && f.texture != TextureKind::Image {
            textures::generate(f.texture, pw, ph, f.tex_scale, f.a, f.b, (obj.id as u32).wrapping_mul(7919))
        } else {
            None
        };
        let tex: Option<(&Pixmap, Transform)> = match picture {
            Some(img) => {
                let img: &Pixmap = &img.0;
                let ts = if f.image_stretch {
                    Transform::from_row(wf / img.width() as f32, 0.0, 0.0, hf / img.height() as f32, pad, pad)
                } else {
                    let s = f.tex_scale.max(0.01);
                    Transform::from_row(s, 0.0, 0.0, s, pad, pad)
                };
                Some((img, ts))
            }
            None => generated.as_ref().map(|g| (g, Transform::identity())),
        };
        if let Some(paint) = fill_shader_paint(f, wf, hf, pad, tex) {
            pm.fill_path(path, &paint, FillRule::Winding, Transform::identity(), None);
        }
    }

    // 2. paint layer, clipped to the shape
    if let Some(layer) = &obj.paint {
        let layer: &Pixmap = layer;
        let sx = wf / layer.width() as f32;
        let sy = hf / layer.height() as f32;
        let t = Transform::from_row(sx, 0.0, 0.0, sy, pad, pad);
        let pp = PixmapPaint {
            opacity: 1.0,
            blend_mode: BlendMode::SourceOver,
            quality: FilterQuality::Bilinear,
        };
        match &path {
            Some(path) => {
                if let Some(mut mask) = Mask::new(pw, ph) {
                    mask.fill_path(path, FillRule::Winding, true, Transform::identity());
                    pm.draw_pixmap(0, 0, layer.as_ref(), &pp, t, Some(&mask));
                }
            }
            None => pm.draw_pixmap(0, 0, layer.as_ref(), &pp, t, None),
        }
    }

    // 3. outline + painted transparency mask. A paint layer / picture is masked first so
    //    its outline hugs what is left (sticker look after a background removal); a vector
    //    shape is masked after, so its outline gets hidden along with it.
    let mask: Option<&Pixmap> = obj.mask.as_deref();
    let outline = obj.stroke.enabled && obj.stroke.width > 0.0;
    match &path {
        Some(path) => {
            if outline {
                draw_outline(&mut pm, path, &obj.stroke);
            }
            if let Some(m) = mask {
                apply_mask(&mut pm, m, pad, wf, hf);
            }
        }
        None => {
            if let Some(m) = mask {
                apply_mask(&mut pm, m, pad, wf, hf);
            }
            if outline {
                effects::alpha_outline(&mut pm, &obj.stroke);
            }
        }
    }

    // 4. bevel / material
    let mut heights = None;
    if obj.effects.bevel.enabled {
        let coverage = match (&path, obj.fill.kind) {
            (Some(path), k) if k != FillKind::None && mask.is_none() => shape_coverage(path, obj, pw, ph, &pm),
            _ => None,
        };
        heights = effects::apply_bevel(&mut pm, &obj.effects.bevel, light, coverage.as_deref());
    }

    let frame = effects::LightFrame {
        w: wf,
        h: hf,
        pad,
        ex,
        ey,
    };
    // 5. object lights (they shade the bevel relief too)
    if obj.effects.lighting.active() {
        effects::apply_lights(&mut pm, &obj.effects.lighting, heights.as_deref(), &frame);
    }

    // 6. transparency
    effects::apply_transparency(&mut pm, &obj.transparency, &frame, (obj.id as u32).wrapping_mul(31));
    (pm, pad)
}

/// Multiply the local raster by a grey mask stretched over the object's box.
fn apply_mask(pm: &mut Pixmap, mask: &Pixmap, pad: f32, w: f32, h: f32) {
    let pw = pm.width() as usize;
    let (mw, mh) = (mask.width() as f32, mask.height() as f32);
    let md = mask.data();
    let data = pm.data_mut();
    for (i, px) in data.chunks_exact_mut(4).enumerate() {
        if px[3] == 0 {
            continue;
        }
        let x = (i % pw) as f32 + 0.5 - pad;
        let y = (i / pw) as f32 + 0.5 - pad;
        let u = (x / w.max(1e-3) * mw).clamp(0.5, mw - 0.5);
        let v = (y / h.max(1e-3) * mh).clamp(0.5, mh - 0.5);
        let k = raster::sample_bilinear(md, mask.width() as i32, mask.height() as i32, u, v)[0] / 255.0;
        if k < 0.999 {
            for c in px.iter_mut() {
                *c = (*c as f32 * k + 0.5) as u8;
            }
        }
    }
}

/// Opaque coverage of the shape (+ outline), merged with whatever paint lies on top.
fn shape_coverage(path: &tiny_skia::Path, obj: &Object, pw: u32, ph: u32, pm: &Pixmap) -> Option<Vec<f32>> {
    let mut cov = Pixmap::new(pw, ph)?;
    let mut paint = Paint {
        anti_alias: true,
        ..Default::default()
    };
    paint.set_color(Color::WHITE);
    cov.fill_path(path, &paint, FillRule::Winding, Transform::identity(), None);
    let reach = obj.stroke.outer_extent();
    if reach > 0.0 {
        let stroke = Stroke {
            width: reach * 2.0,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Default::default()
        };
        cov.stroke_path(path, &paint, &stroke, Transform::identity(), None);
    }
    Some(
        cov.data()
            .chunks_exact(4)
            .zip(pm.data().chunks_exact(4))
            .map(|(c, p)| c[3].max(p[3]) as f32 / 255.0)
            .collect(),
    )
}

fn layer_key(obj: &Object, dw: u32, dh: u32) -> [u32; 10] {
    let q = &obj.quad;
    [
        q[0].x.to_bits(),
        q[0].y.to_bits(),
        q[1].x.to_bits(),
        q[1].y.to_bits(),
        q[2].x.to_bits(),
        q[2].y.to_bits(),
        q[3].x.to_bits(),
        q[3].y.to_bits(),
        dw,
        dh,
    ]
}

fn build_layer(local: &Pixmap, pad: f32, obj: &Object, dw: u32, dh: u32) -> Option<(Pixmap, i32, i32)> {
    let fx = &obj.effects;
    let mut margin = 0i32;
    if fx.shadow.enabled {
        let off = fx.shadow.dx.abs().max(fx.shadow.dy.abs());
        margin = margin.max((fx.shadow.blur * 1.5 + off).ceil() as i32 + 2);
    }
    if fx.glow.enabled {
        margin = margin.max((fx.glow.size * 1.6).ceil() as i32 + 2);
    }
    let (warped, x, y) = raster::warp(local, pad, &obj.homography(), dw, dh, margin)?;
    if margin > 0 {
        Some((effects::shadow_glow(&warped, fx), x, y))
    } else {
        Some((warped, x, y))
    }
}

impl Renderer {
    pub fn clear(&mut self) {
        self.cache.clear();
    }

    fn ensure(&mut self, obj: &Object, dw: u32, dh: u32) {
        let (ex, ey) = obj.axes();
        let light = effects::local_light(&obj.effects.bevel, ex, ey);
        // Bevel and distant lights are canvas-relative, so rotating the object re-shades it.
        let canvas_lit = obj.effects.bevel.enabled
            || (obj.effects.lighting.active()
                && obj.effects.lighting.lights.iter().any(|l| l.kind == LightKind::Distant));
        let light_key = if canvas_lit {
            let a = (ex.angle().to_degrees() * 2.0).round() as i32;
            let b = (ey.angle().to_degrees() * 2.0).round() as i32;
            a.wrapping_mul(1000).wrapping_add(b)
        } else {
            0
        };
        let stale = match self.cache.get(&obj.id) {
            Some(e) => e.rev != obj.rev || e.light_key != light_key,
            None => true,
        };
        if stale {
            let (local, pad) = render_local(obj, light, ex, ey);
            self.cache.insert(
                obj.id,
                Entry {
                    rev: obj.rev,
                    light_key,
                    local,
                    pad,
                    layer_key: None,
                    layer: None,
                },
            );
        }
        let e = self.cache.get_mut(&obj.id).expect("entry");
        let key = layer_key(obj, dw, dh);
        if e.layer_key != Some(key) {
            e.layer = build_layer(&e.local, e.pad, obj, dw, dh);
            e.layer_key = Some(key);
        }
    }

    pub fn render(&mut self, doc: &Document) -> Pixmap {
        self.render_opts(doc, true)
    }

    /// Render with or without the page background (transparent exports, clipboard images).
    pub fn render_opts(&mut self, doc: &Document, background: bool) -> Pixmap {
        let mut out = Pixmap::new(doc.width.max(1), doc.height.max(1)).expect("canvas");
        if background {
            let bg = doc.background;
            out.fill(Color::from_rgba8(bg[0], bg[1], bg[2], 255));
        }
        let mut live = HashSet::new();
        for obj in &doc.objects {
            live.insert(obj.id);
            if !obj.visible || obj.opacity <= 0.0 {
                continue;
            }
            self.ensure(obj, doc.width, doc.height);
            if let Some(Entry {
                layer: Some((lp, x, y)),
                ..
            }) = self.cache.get(&obj.id)
            {
                let paint = PixmapPaint {
                    opacity: obj.opacity.clamp(0.0, 1.0),
                    blend_mode: BlendMode::SourceOver,
                    quality: FilterQuality::Nearest,
                };
                out.draw_pixmap(*x, *y, lp.as_ref(), &paint, Transform::identity(), None);
            }
        }
        self.cache.retain(|id, _| live.contains(id));
        out
    }

    /// Opacity of the object itself (no shadow / glow) at a canvas point — for picking.
    pub fn alpha_at(&self, obj: &Object, p: Pt) -> Option<f32> {
        let e = self.cache.get(&obj.id)?;
        if e.rev != obj.rev {
            return None;
        }
        let inv = obj.homography().inverse()?;
        let l = inv.apply_pt(p)?;
        let s = raster::sample_bilinear(
            e.local.data(),
            e.local.width() as i32,
            e.local.height() as i32,
            l.x + e.pad,
            l.y + e.pad,
        );
        Some(s[3] / 255.0)
    }

    pub fn hit(&self, obj: &Object, p: Pt) -> bool {
        match self.alpha_at(obj, p) {
            Some(a) => a > 0.08,
            None => point_in_poly(p, &obj.quad),
        }
    }
}
