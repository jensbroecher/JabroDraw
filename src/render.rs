//! Object render pipeline with caching:
//!   local raster  (shape fill + paint layer + outline + bevel/material)   — cached by content revision
//!   -> canvas layer (projective warp + drop shadow + glow)                — cached by placement
//!   -> composite onto the document.

use crate::doc::{Document, FillKind, FillStyle, Object, ShapeKind};
use crate::effects;
use crate::geom::{point_in_poly, Pt};
use crate::raster;
use crate::shapes;
use crate::textures;
use std::collections::{HashMap, HashSet};
use tiny_skia::{
    BlendMode, Color, FillRule, FilterQuality, GradientStop, LineCap, LineJoin, LinearGradient, Mask,
    Paint, Pattern, Pixmap, PixmapPaint, Point, RadialGradient, SpreadMode, Stroke, Transform,
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

fn fill_shader_paint<'a>(fill: &FillStyle, w: f32, h: f32, pad: f32, tex: Option<&'a Pixmap>) -> Option<Paint<'a>> {
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
            let t = tex?;
            paint.shader = Pattern::new(
                t.as_ref(),
                SpreadMode::Repeat,
                FilterQuality::Nearest,
                1.0,
                Transform::identity(),
            );
        }
    }
    Some(paint)
}

/// Render an object's content in its own local space (plus padding).
pub fn render_local(obj: &Object, light: [f32; 3]) -> (Pixmap, f32) {
    let (w, h) = obj.raster_size();
    let (wf, hf) = (w as f32, h as f32);
    let stroke_extra = if obj.stroke.enabled && obj.kind != ShapeKind::Bitmap {
        obj.stroke.width * 0.5
    } else {
        0.0
    };
    let pad = (stroke_extra + 3.0).ceil();
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
        let tex = if obj.fill.kind == FillKind::Texture {
            textures::generate(
                obj.fill.texture,
                pw,
                ph,
                obj.fill.tex_scale,
                obj.fill.a,
                obj.fill.b,
                (obj.id as u32).wrapping_mul(7919),
            )
        } else {
            None
        };
        if let Some(paint) = fill_shader_paint(&obj.fill, wf, hf, pad, tex.as_ref()) {
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

    // 3. outline
    if let (Some(path), true) = (&path, obj.stroke.enabled && obj.stroke.width > 0.0) {
        let mut paint = Paint {
            anti_alias: true,
            ..Default::default()
        };
        paint.set_color(color(obj.stroke.color));
        let stroke = Stroke {
            width: obj.stroke.width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Default::default()
        };
        pm.stroke_path(path, &paint, &stroke, Transform::identity(), None);
    }

    // 4. bevel / material
    if obj.effects.bevel.enabled {
        let coverage = match (&path, obj.fill.kind) {
            (Some(path), k) if k != FillKind::None => shape_coverage(path, obj, pw, ph, &pm),
            _ => None,
        };
        effects::apply_bevel(&mut pm, &obj.effects.bevel, light, coverage.as_deref());
    }
    (pm, pad)
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
    if obj.stroke.enabled && obj.stroke.width > 0.0 {
        let stroke = Stroke {
            width: obj.stroke.width,
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
        let light_key = if obj.effects.bevel.enabled {
            (light[1].atan2(light[0]).to_degrees() * 2.0).round() as i32
        } else {
            0
        };
        let stale = match self.cache.get(&obj.id) {
            Some(e) => e.rev != obj.rev || e.light_key != light_key,
            None => true,
        };
        if stale {
            let (local, pad) = render_local(obj, light);
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
        let mut out = Pixmap::new(doc.width.max(1), doc.height.max(1)).expect("canvas");
        let bg = doc.background;
        out.fill(Color::from_rgba8(bg[0], bg[1], bg[2], 255));
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
