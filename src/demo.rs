//! The start-up showcase scene (also used for headless test renders).

use crate::brush::{self, BrushKind, BrushSettings, StrokeState};
use crate::doc::*;
use crate::geom::Pt;
use crate::npr;
use crate::render::Renderer;
use crate::styles;
use std::sync::Arc;
use tiny_skia::Pixmap;

/// Paint a polyline (in local coordinates of the object) into its paint layer.
fn paint_local(obj: &mut Object, pts: &[(f32, f32)], b: &BrushSettings) {
    let (w, h) = obj.raster_size();
    let layer = obj
        .paint
        .get_or_insert_with(|| Arc::new(Pixmap::new(w, h).expect("paint layer")));
    let pm = Arc::make_mut(layer);
    let mut st = StrokeState::new();
    let r = b.size * 0.5;
    for win in pts.windows(2) {
        brush::segment(pm, win[0], win[1], r, b, &mut st);
    }
    obj.touch();
}

fn wave(x0: f32, x1: f32, y: f32, amp: f32, freq: f32, phase: f32, steps: usize) -> Vec<(f32, f32)> {
    (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            let x = x0 + (x1 - x0) * t;
            (x, y + (t * freq * std::f32::consts::TAU + phase).sin() * amp)
        })
        .collect()
}

pub fn demo_document() -> Document {
    let mut d = Document::new(1200, 800);
    d.background = [246, 240, 228, 255];

    // Sky
    let id = d.alloc_id();
    let mut sky = Object::new(id, ShapeKind::Rect { corner: 0.0 }, 0.0, 0.0, 1200.0, 800.0);
    sky.name = "Sky".into();
    sky.fill = FillStyle {
        kind: FillKind::Linear,
        a: [110, 168, 232, 255],
        b: [252, 236, 205, 255],
        angle: 90.0,
        ..Default::default()
    };
    d.objects.push(sky);

    // Sun with glow
    let id = d.alloc_id();
    let mut sun = Object::new(id, ShapeKind::Ellipse, 930.0, 70.0, 170.0, 170.0);
    sun.name = "Sun".into();
    sun.fill = FillStyle {
        kind: FillKind::Radial,
        a: [255, 252, 210, 255],
        b: [255, 168, 40, 255],
        ..Default::default()
    };
    sun.effects.glow = Glow {
        enabled: true,
        color: [255, 200, 90, 255],
        size: 40.0,
        opacity: 0.85,
    };
    d.objects.push(sun);

    // Rolling hills (freehand vector, procedural texture, inflated bevel)
    let id = d.alloc_id();
    let hill_pts = vec![
        [0.0, 0.42],
        [0.14, 0.16],
        [0.3, 0.32],
        [0.47, 0.06],
        [0.64, 0.28],
        [0.82, 0.1],
        [1.0, 0.36],
        [1.0, 1.0],
        [0.5, 1.0],
        [0.0, 1.0],
    ];
    let mut hills = Object::new(id, ShapeKind::Freehand { points: hill_pts }, -60.0, 470.0, 1320.0, 380.0);
    hills.name = "Hills".into();
    hills.fill = FillStyle {
        kind: FillKind::Texture,
        a: [52, 118, 48, 255],
        b: [150, 196, 86, 255],
        texture: TextureKind::Clouds,
        tex_scale: 1.4,
        ..Default::default()
    };
    hills.effects.bevel = Bevel {
        enabled: true,
        profile: BevelProfile::Pillow,
        depth: 0.12,
        intensity: 0.6,
        gloss: 0.05,
        ..Default::default()
    };
    crate::vector::convert_to_path(&mut hills);
    d.objects.push(hills);

    // Cloud: boolean union of four ellipses -> one editable path with a soft pillow bevel
    let puffs: Vec<_> = [
        (55.0, 263.0, 120.0, 62.0),
        (110.0, 230.0, 115.0, 88.0),
        (185.0, 246.0, 120.0, 72.0),
        (250.0, 268.0, 95.0, 52.0),
    ]
    .iter()
    .filter_map(|&(x, y, w, h)| crate::vector::outline_doc(&Object::new(0, ShapeKind::Ellipse, x, y, w, h)))
    .collect();
    let shapes = crate::vector::boolean(&puffs, crate::vector::BoolOp::Union);
    if let Some((subpaths, quad, size)) = crate::vector::polygons_to_object_parts(&shapes) {
        let id = d.alloc_id();
        let mut cloud = Object::new(id, ShapeKind::Path { subpaths }, 0.0, 0.0, size[0], size[1]);
        cloud.quad = quad;
        cloud.name = "Cloud (union)".into();
        cloud.fill = FillStyle {
            kind: FillKind::Solid,
            a: [255, 255, 255, 240],
            ..Default::default()
        };
        cloud.effects.bevel = Bevel {
            enabled: true,
            profile: BevelProfile::Pillow,
            depth: 0.2,
            softness: 2.0,
            intensity: 0.45,
            shadow: [90, 120, 170, 255],
            gloss: 0.1,
            ..Default::default()
        };
        cloud.effects.shadow = Shadow {
            enabled: true,
            color: [60, 90, 140, 255],
            dx: 6.0,
            dy: 8.0,
            blur: 14.0,
            opacity: 0.25,
        };
        d.objects.push(cloud);
    }

    // Wooden sign in perspective with chalk doodles painted on it
    let id = d.alloc_id();
    let mut sign = Object::new(id, ShapeKind::Rect { corner: 0.12 }, 0.0, 0.0, 420.0, 230.0);
    sign.name = "Wood sign".into();
    styles::wood(&mut sign);
    sign.quad = [
        Pt::new(90.0, 330.0),
        Pt::new(500.0, 372.0),
        Pt::new(486.0, 560.0),
        Pt::new(70.0, 600.0),
    ];
    sign.update_size_from_quad();
    {
        let (w, h) = (sign.size[0], sign.size[1]);
        let chalk = BrushSettings {
            kind: BrushKind::Chalk,
            size: 9.0,
            hardness: 0.8,
            flow: 1.0,
            spacing: 0.1,
            color: [250, 250, 245, 255],
            pressure: false,
            ..Default::default()
        };
        paint_local(&mut sign, &wave(w * 0.12, w * 0.88, h * 0.38, h * 0.1, 2.0, 0.0, 80), &chalk);
        paint_local(&mut sign, &wave(w * 0.12, w * 0.88, h * 0.62, h * 0.08, 3.0, 1.2, 80), &chalk);
        let circle: Vec<(f32, f32)> = (0..=60)
            .map(|i| {
                let a = i as f32 / 60.0 * std::f32::consts::TAU;
                (w * 0.82 + a.cos() * h * 0.12, h * 0.25 + a.sin() * h * 0.12)
            })
            .collect();
        let yellow = BrushSettings {
            color: [255, 220, 90, 255],
            ..chalk.clone()
        };
        paint_local(&mut sign, &circle, &yellow);
    }
    sign.effects.lighting = Lighting {
        enabled: true,
        ambient: 0.55,
        specular: 0.4,
        lights: vec![Light {
            kind: LightKind::Spot,
            pos: [0.12, 0.05],
            target: [0.55, 0.55],
            height: 0.6,
            cone: 32.0,
            softness: 0.7,
            intensity: 1.3,
            color: [255, 228, 170, 255],
            ..Default::default()
        }],
    };
    d.objects.push(sign);

    // Chrome star
    let id = d.alloc_id();
    let mut star = Object::new(id, ShapeKind::Polygon { sides: 5, inner: 0.46 }, 680.0, 300.0, 230.0, 230.0);
    star.name = "Chrome star".into();
    styles::chrome(&mut star);
    let c = star.center();
    star.quad = star.quad.map(|p| c + (p - c).rotate(-0.12));
    d.objects.push(star);

    // Glass bubble with paint strokes inside (clipped to the shape)
    let id = d.alloc_id();
    let mut bubble = Object::new(id, ShapeKind::Ellipse, 950.0, 330.0, 200.0, 200.0);
    bubble.name = "Glass bubble".into();
    styles::glass(&mut bubble);
    {
        let b = BrushSettings {
            kind: BrushKind::Round,
            size: 26.0,
            hardness: 0.5,
            flow: 0.9,
            spacing: 0.1,
            color: [255, 80, 120, 255],
            pressure: false,
            ..Default::default()
        };
        paint_local(&mut bubble, &wave(-20.0, 220.0, 120.0, 30.0, 1.5, 0.3, 90), &b);
        let b2 = BrushSettings {
            color: [255, 210, 60, 255],
            size: 16.0,
            ..b.clone()
        };
        paint_local(&mut bubble, &wave(-20.0, 220.0, 150.0, 22.0, 2.0, 2.0, 90), &b2);
    }
    d.objects.push(bubble);

    // Embossed paint strokes on a free paint layer
    let id = d.alloc_id();
    let mut strokes = Object::new(id, ShapeKind::Bitmap, 0.0, 0.0, 1200.0, 800.0);
    strokes.name = "Embossed strokes".into();
    strokes.fill.kind = FillKind::None;
    strokes.effects.bevel = Bevel {
        enabled: true,
        profile: BevelProfile::Round,
        width: 7.0,
        depth: 0.9,
        intensity: 0.8,
        gloss: 0.8,
        shininess: 30.0,
        ..Default::default()
    };
    strokes.effects.shadow = Shadow {
        enabled: true,
        dx: 4.0,
        dy: 5.0,
        blur: 6.0,
        opacity: 0.35,
        ..Default::default()
    };
    {
        let b = BrushSettings {
            kind: BrushKind::Round,
            size: 22.0,
            hardness: 0.9,
            flow: 1.0,
            spacing: 0.08,
            color: [235, 70, 50, 255],
            pressure: false,
            ..Default::default()
        };
        paint_local(&mut strokes, &wave(560.0, 1140.0, 660.0, 28.0, 1.5, 0.0, 160), &b);
        let b2 = BrushSettings {
            color: [50, 120, 235, 255],
            size: 16.0,
            ..b.clone()
        };
        paint_local(&mut strokes, &wave(580.0, 1120.0, 720.0, 20.0, 2.2, 1.0, 160), &b2);
    }
    strokes.stroke = StrokeStyle {
        enabled: true,
        color: [255, 255, 255, 255],
        width: 3.0,
        position: StrokePos::Outside,
        ..Default::default()
    };
    d.objects.push(strokes);

    // Title text in gold, slightly tilted
    let id = d.alloc_id();
    let title_kind = ShapeKind::Text {
        text: "Jabro Draw".into(),
        font: FontChoice::Black,
    };
    let mut title = Object::new(id, title_kind, 0.0, 0.0, 600.0, 105.0);
    title.name = "Title".into();
    title.fit_text_width();
    let tw = title.size[0];
    title.quad = Object::rect_quad(560.0 - tw * 0.5, 80.0, tw, 105.0);
    styles::gold(&mut title);
    title.effects.bevel.width = 6.0;
    let c = title.center();
    title.quad = title.quad.map(|p| c + (p - c).rotate(-0.05));
    d.objects.push(title);

    d
}

/// "mixed": watercolor + ink outlines for the page, a clean title and an oil-painted star.
pub fn mixed_looks(doc: &mut Document) {
    doc.npr.style = NprStyle::Watercolor;
    doc.npr_stack = vec![NprSettings {
        style: NprStyle::ComicInk,
        strength: 0.55,
        brush: 0.2,
        paper: 0.0,
        ..Default::default()
    }];
    for o in &mut doc.objects {
        match o.name.as_str() {
            "Title" => o.npr_mode = NprMode::Off,
            "Chrome star" => {
                o.npr_mode = NprMode::Own;
                o.npr_own = NprSettings {
                    style: NprStyle::OilPaint,
                    brush: 0.5,
                    ..Default::default()
                };
            }
            _ => {}
        }
    }
}

pub fn render_demo_to_png(path: &str, style: &str) -> Result<(), String> {
    let mut doc = demo_document();
    if style == "mixed" {
        mixed_looks(&mut doc);
    } else {
        doc.npr.style = NprStyle::from_name(style).ok_or_else(|| format!("unknown style '{style}'"))?;
    }
    let mut r = Renderer::default();
    let t0 = std::time::Instant::now();
    let pm = r.render(&doc);
    let t1 = t0.elapsed();
    let out = if doc.any_look() {
        let runs = r.render_runs(&doc);
        npr::compose(&runs, doc.width, doc.height, doc.background, 1.0)
    } else {
        pm
    };
    println!("render {:?}, npr {:?}", t1, t0.elapsed() - t1);
    out.save_png(path).map_err(|e| e.to_string())
}
