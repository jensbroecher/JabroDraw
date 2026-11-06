//! Headless checks of the core pipeline.

use crate::brush::{self, BrushSettings, StrokeState};
use crate::doc::*;
use crate::effects::edt_inside;
use crate::geom::{Homography, Pt};
use crate::render::Renderer;
use std::sync::Arc;
use tiny_skia::Pixmap;

#[test]
fn homography_maps_corners_and_inverts() {
    let q = [
        Pt::new(10.0, 20.0),
        Pt::new(300.0, 60.0),
        Pt::new(280.0, 250.0),
        Pt::new(30.0, 200.0),
    ];
    let h = Homography::from_quad(&q, 200.0, 100.0);
    let local = [(0.0, 0.0), (200.0, 0.0), (200.0, 100.0), (0.0, 100.0)];
    for (i, (x, y)) in local.iter().enumerate() {
        let (cx, cy) = h.apply(*x, *y).unwrap();
        assert!((cx - q[i].x as f64).abs() < 1e-3 && (cy - q[i].y as f64).abs() < 1e-3, "corner {i}");
    }
    let inv = h.inverse().unwrap();
    let (cx, cy) = h.apply(57.0, 33.0).unwrap();
    let (lx, ly) = inv.apply(cx, cy).unwrap();
    assert!((lx - 57.0).abs() < 1e-6 && (ly - 33.0).abs() < 1e-6);
}

#[test]
fn distance_transform_of_square() {
    let (w, h) = (11, 11);
    let mut a = vec![0.0f32; w * h];
    for y in 2..9 {
        for x in 2..9 {
            a[y * w + x] = 1.0;
        }
    }
    let d = edt_inside(&a, w, h);
    assert_eq!(d[0], 0.0);
    assert!((d[5 * w + 5] - 4.0).abs() < 1e-4, "centre is 4 px from the outside, got {}", d[5 * w + 5]);
    assert!((d[2 * w + 5] - 1.0).abs() < 1e-4);
}

#[test]
fn paint_is_clipped_to_shape_and_follows_perspective() {
    let mut doc = Document::new(200, 200);
    let id = doc.alloc_id();
    let mut obj = Object::new(id, ShapeKind::Ellipse, 50.0, 50.0, 100.0, 100.0);
    obj.fill.kind = FillKind::None;
    let mut layer = Pixmap::new(100, 100).unwrap();
    // paint a full-width red bar through the middle, including outside the ellipse corners
    let b = BrushSettings {
        size: 30.0,
        hardness: 1.0,
        flow: 1.0,
        color: [255, 0, 0, 255],
        ..Default::default()
    };
    let mut st = StrokeState::new();
    brush::segment(&mut layer, (0.0, 50.0), (100.0, 50.0), 15.0, &b, &mut st);
    obj.paint = Some(Arc::new(layer));
    doc.objects.push(obj);

    let mut r = Renderer::default();
    let out = r.render(&doc);
    let px = |x: u32, y: u32| out.pixel(x, y).unwrap().demultiply();
    assert!(px(100, 100).red() > 240 && px(100, 100).green() < 20, "paint visible in centre");
    assert!(px(51, 51).red() > 240 && px(51, 51).green() > 240, "nothing outside the ellipse (white bg)");

    // Shift the right edge down in perspective: the bar's right end must move down too.
    let o = doc.find_mut(id).unwrap();
    o.quad[1] = Pt::new(150.0, 90.0);
    o.quad[2] = Pt::new(150.0, 190.0);
    let out = r.render(&doc);
    let p = out.pixel(145, 140).unwrap().demultiply();
    assert!(p.red() > 200 && p.green() < 80, "bar followed the perspective warp");
}

#[test]
fn save_load_roundtrip_keeps_paint() {
    let doc = crate::demo::demo_document();
    let path = std::env::temp_dir().join("jabro_roundtrip_test.jabro");
    crate::io::save(&doc, &path).unwrap();
    let back = crate::io::load(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(back.objects.len(), doc.objects.len());
    for (a, b) in doc.objects.iter().zip(&back.objects) {
        assert_eq!(a.kind, b.kind);
        assert_eq!(a.effects, b.effects);
        assert_eq!(a.paint.as_ref().map(|p| p.width()), b.paint.as_ref().map(|p| p.width()));
    }
    assert!(back.next_id > back.objects.iter().map(|o| o.id).max().unwrap());
}

#[test]
fn every_npr_style_produces_an_image() {
    let mut doc = crate::demo::demo_document();
    doc.width = 240;
    doc.height = 160;
    let mut r = Renderer::default();
    let raw = r.render(&doc);
    for s in NprStyle::ALL {
        doc.npr.style = s;
        let out = crate::npr::apply(&raw, &doc.npr);
        assert_eq!((out.width(), out.height()), (240, 160), "{s:?}");
    }
}

// ------------------------------------------------------------------ paths & booleans

use crate::vector::{self, BoolOp};

fn area(shapes: &vector::Shapes) -> f64 {
    let mut a = 0.0;
    for shape in shapes {
        for c in shape {
            let n = c.len();
            let mut s = 0.0;
            for i in 0..n {
                let (p, q) = (c[i], c[(i + 1) % n]);
                s += p[0] * q[1] - q[0] * p[1];
            }
            a += s * 0.5; // outer and holes have opposite orientation
        }
    }
    a.abs()
}

fn square(id: u64, x: f32, y: f32) -> Object {
    Object::new(id, ShapeKind::Rect { corner: 0.0 }, x, y, 100.0, 100.0)
}

#[test]
fn boolean_ops_have_expected_areas() {
    let a = vector::outline_doc(&square(1, 0.0, 0.0)).unwrap();
    let b = vector::outline_doc(&square(2, 50.0, 0.0)).unwrap();
    let inputs = vec![a, b];
    let check = |op, want: f64| {
        let got = area(&vector::boolean(&inputs, op));
        assert!((got - want).abs() < 1.0, "{op:?}: got {got}, want {want}");
    };
    check(BoolOp::Union, 15000.0);
    check(BoolOp::Subtract, 5000.0);
    check(BoolOp::Intersect, 5000.0);
    check(BoolOp::Exclude, 10000.0);
}

#[test]
fn subtract_makes_a_hole_that_renders() {
    let outer = vector::outline_doc(&Object::new(1, ShapeKind::Ellipse, 20.0, 20.0, 160.0, 160.0)).unwrap();
    let inner = vector::outline_doc(&Object::new(2, ShapeKind::Ellipse, 70.0, 70.0, 60.0, 60.0)).unwrap();
    let shapes = vector::boolean(&[outer, inner], BoolOp::Subtract);
    let (subpaths, quad, size) = vector::polygons_to_object_parts(&shapes).unwrap();
    assert_eq!(subpaths.len(), 2, "outer ring + hole");
    let mut doc = Document::new(200, 200);
    let mut ring = Object::new(9, ShapeKind::Path { subpaths }, 0.0, 0.0, size[0], size[1]);
    ring.quad = quad;
    ring.fill.a = [0, 0, 255, 255];
    doc.objects.push(ring);
    let out = Renderer::default().render(&doc);
    let px = |x, y| out.pixel(x, y).unwrap().demultiply();
    assert!(px(100, 100).red() > 240, "hole shows the white background");
    assert!(px(45, 100).blue() > 240 && px(45, 100).red() < 20, "ring is filled");
}

#[test]
fn convert_to_path_keeps_the_rendering() {
    let mut doc = Document::new(220, 160);
    let mut star = Object::new(1, ShapeKind::Polygon { sides: 5, inner: 0.45 }, 10.0, 10.0, 140.0, 130.0);
    star.quad[1] = Pt::new(200.0, 30.0); // some perspective
    star.fill.a = [200, 30, 30, 255];
    doc.objects.push(star);
    let mut r = Renderer::default();
    let before = r.render(&doc);
    assert!(vector::convert_to_path(&mut doc.objects[0]));
    let after = r.render(&doc);
    let diff = before
        .data()
        .iter()
        .zip(after.data())
        .filter(|(a, b)| (**a as i32 - **b as i32).abs() > 8)
        .count();
    assert!(diff < 40, "{diff} differing channels");
}

#[test]
fn normalize_bounds_keeps_nodes_in_place_under_perspective() {
    let mut o = Object::new(1, ShapeKind::Ellipse, 0.0, 0.0, 100.0, 80.0);
    o.quad = [Pt::new(10.0, 10.0), Pt::new(150.0, 30.0), Pt::new(140.0, 120.0), Pt::new(0.0, 100.0)];
    assert!(vector::convert_to_path(&mut o));
    // drag the first node far outside the old box
    if let ShapeKind::Path { subpaths } = &mut o.kind {
        let n = &mut subpaths[0].nodes[0];
        n.p[1] -= 0.6;
        n.h_in[1] -= 0.6;
        n.h_out[1] -= 0.6;
    }
    let doc_pts = |o: &Object| -> Vec<Pt> {
        let hm = o.homography();
        let ShapeKind::Path { subpaths } = &o.kind else { panic!() };
        subpaths[0]
            .nodes
            .iter()
            .map(|n| hm.apply_pt(Pt::new(n.p[0] * o.size[0], n.p[1] * o.size[1])).unwrap())
            .collect()
    };
    let before = doc_pts(&o);
    assert!(vector::normalize_bounds(&mut o));
    let after = doc_pts(&o);
    for (a, b) in before.iter().zip(&after) {
        assert!(a.dist(*b) < 0.05, "{a:?} moved to {b:?}");
    }
}

#[test]
fn split_segment_keeps_curve_shape() {
    let mut o = Object::new(1, ShapeKind::Ellipse, 0.0, 0.0, 100.0, 100.0);
    vector::convert_to_path(&mut o);
    let ShapeKind::Path { subpaths } = &mut o.kind else { panic!() };
    let sp = &mut subpaths[0];
    let (a, b) = (sp.nodes[0], sp.nodes[1]);
    let pt = |q: [f32; 2]| Pt::new(q[0], q[1]);
    let expect = vector::cubic(pt(a.p), pt(a.h_out), pt(b.h_in), pt(b.p), 0.3);
    let i = vector::split_segment(sp, 0, 0.3);
    assert!(pt(sp.nodes[i].p).dist(expect) < 1e-5);
    assert_eq!(sp.nodes.len(), 5, "4-node circle + 1");
}

// ------------------------------------------------------------------ transparency, lights, outlines

fn render_one(o: Object, w: u32, h: u32) -> tiny_skia::Pixmap {
    let mut doc = Document::new(w, h);
    doc.objects.push(o);
    Renderer::default().render(&doc)
}

fn lum(pm: &tiny_skia::Pixmap, x: u32, y: u32) -> f32 {
    let c = pm.pixel(x, y).unwrap().demultiply();
    (c.red() as f32 + c.green() as f32 + c.blue() as f32) / (3.0 * 255.0)
}

#[test]
fn linear_and_radial_transparency_fade_the_object() {
    let mut o = Object::new(1, ShapeKind::Rect { corner: 0.0 }, 0.0, 0.0, 100.0, 100.0);
    o.fill.a = [0, 0, 0, 255];
    o.transparency.kind = TransparencyKind::Linear; // default: opaque at top, clear at bottom
    let out = render_one(o.clone(), 100, 100);
    assert!(lum(&out, 50, 3) < 0.1, "top is opaque black");
    assert!((lum(&out, 50, 50) - 0.5).abs() < 0.08, "middle is half see-through");
    assert!(lum(&out, 50, 97) > 0.9, "bottom shows the white page");

    o.transparency.kind = TransparencyKind::Radial;
    o.transparency.p0 = [0.5, 0.5];
    o.transparency.p1 = [1.0, 0.5];
    let out = render_one(o, 100, 100);
    assert!(lum(&out, 50, 50) < 0.1, "centre opaque");
    assert!(lum(&out, 98, 50) > 0.9, "rim transparent");
}

#[test]
fn transparency_follows_perspective() {
    let mut o = Object::new(1, ShapeKind::Rect { corner: 0.0 }, 0.0, 0.0, 100.0, 100.0);
    o.fill.a = [0, 0, 0, 255];
    o.transparency.kind = TransparencyKind::Linear;
    o.transparency.p0 = [0.0, 0.5];
    o.transparency.p1 = [1.0, 0.5]; // fades left -> right
    o.quad = Object::rect_quad(0.0, 0.0, 100.0, 100.0).map(|p| Pt::new(p.y, p.x)); // mirrored on the diagonal
    let out = render_one(o, 100, 100);
    assert!(lum(&out, 50, 3) < 0.15 && lum(&out, 50, 96) > 0.85, "the fade now runs top -> bottom");
}

#[test]
fn point_and_spot_lights_brighten_where_they_shine() {
    let mut o = Object::new(1, ShapeKind::Rect { corner: 0.0 }, 0.0, 0.0, 200.0, 200.0);
    o.fill.a = [128, 128, 128, 255];
    o.effects.lighting.enabled = true;
    o.effects.lighting.ambient = 0.2;
    o.effects.lighting.lights.push(Light::new(LightKind::Point, [0.5, 0.5]));
    let out = render_one(o.clone(), 200, 200);
    assert!(lum(&out, 100, 100) > lum(&out, 5, 5) + 0.15, "pool of light under the lamp");

    let mut spot = Light::new(LightKind::Spot, [0.2, 0.2]);
    spot.target = [0.7, 0.7];
    spot.cone = 15.0;
    spot.softness = 0.1;
    o.effects.lighting.lights = vec![spot];
    let out = render_one(o, 200, 200);
    assert!(lum(&out, 140, 140) > 0.3, "inside the cone is lit");
    assert!(lum(&out, 180, 20) < 0.15, "outside the cone stays at ambient");
}

#[test]
fn outline_positions_inside_outside_and_around_paint() {
    let mut o = Object::new(1, ShapeKind::Rect { corner: 0.0 }, 40.0, 40.0, 100.0, 100.0);
    o.fill.a = [0, 0, 255, 255];
    o.stroke = StrokeStyle {
        enabled: true,
        color: [255, 0, 0, 255],
        width: 10.0,
        position: StrokePos::Outside,
        ..Default::default()
    };
    let out = render_one(o.clone(), 180, 180);
    let red = |pm: &tiny_skia::Pixmap, x, y| {
        let c = pm.pixel(x, y).unwrap().demultiply();
        c.red() > 200 && c.blue() < 60
    };
    assert!(red(&out, 35, 90), "outside outline sits outside the edge");
    assert!(!red(&out, 45, 90), "inside stays the fill colour");

    o.stroke.position = StrokePos::Inside;
    let out = render_one(o, 180, 180);
    assert!(red(&out, 45, 90), "inside outline sits inside");
    assert!(!red(&out, 35, 90), "nothing outside");

    // paint layer: a dot gets an outline traced around it
    let mut layer = Object::new(2, ShapeKind::Bitmap, 0.0, 0.0, 100.0, 100.0);
    layer.fill.kind = FillKind::None;
    let mut pm = tiny_skia::Pixmap::new(100, 100).unwrap();
    let b = BrushSettings {
        size: 30.0,
        hardness: 1.0,
        flow: 1.0,
        color: [0, 0, 255, 255],
        ..Default::default()
    };
    brush::segment(&mut pm, (50.0, 50.0), (50.0, 50.0), 15.0, &b, &mut StrokeState::new());
    layer.paint = Some(Arc::new(pm));
    layer.stroke = StrokeStyle {
        enabled: true,
        color: [255, 0, 0, 255],
        width: 6.0,
        position: StrokePos::Outside,
        ..Default::default()
    };
    let out = render_one(layer, 100, 100);
    assert!(red(&out, 50, 32), "ring just outside the painted dot");
    assert!(!red(&out, 50, 50), "dot itself stays blue");
}

// ------------------------------------------------------------------ pictures, masks, background removal

use crate::imageops;

/// White picture with a red disc in the middle (and a second small red square in the corner).
fn test_picture() -> tiny_skia::Pixmap {
    let mut pm = tiny_skia::Pixmap::new(120, 80).unwrap();
    pm.fill(tiny_skia::Color::WHITE);
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(220, 30, 30, 255);
    let disc = tiny_skia::PathBuilder::from_circle(60.0, 40.0, 25.0).unwrap();
    pm.fill_path(&disc, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
    let sq = tiny_skia::PathBuilder::from_rect(tiny_skia::Rect::from_xywh(5.0, 5.0, 10.0, 10.0).unwrap());
    pm.fill_path(&sq, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
    pm
}

fn mask_at(m: &tiny_skia::Pixmap, x: u32, y: u32) -> u8 {
    m.pixel(x, y).unwrap().red()
}

#[test]
fn remove_background_hides_the_border_colour_only() {
    let img = test_picture();
    let mut mask = imageops::new_mask(120, 80).unwrap();
    let n = imageops::remove_background(&img, &mut mask, 0.15, 1.0);
    assert!(n > 5000, "most of the white is gone ({n})");
    assert_eq!(mask_at(&mask, 2, 2), 0, "corner background hidden");
    assert_eq!(mask_at(&mask, 60, 40), 255, "subject kept");
    assert_eq!(mask_at(&mask, 10, 10), 255, "red square kept too");
}

#[test]
fn magic_wand_hides_only_the_clicked_area() {
    let img = test_picture();
    let mut mask = imageops::new_mask(120, 80).unwrap();
    imageops::magic_wand(&img, &mut mask, 10, 10, 0.1, 0.0); // click the small red square
    assert_eq!(mask_at(&mask, 10, 10), 0);
    assert_eq!(mask_at(&mask, 60, 40), 255, "the disc is not connected, so it stays");
    assert_eq!(mask_at(&mask, 100, 70), 255, "white stays");
}

#[test]
fn load_image_reads_a_png() {
    let path = std::env::temp_dir().join("jabro_load_test.png");
    test_picture().save_png(&path).unwrap();
    let img = imageops::load_image(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!((img.width(), img.height()), (120, 80));
    let c = img.pixel(60, 40).unwrap();
    assert!(c.red() > 200 && c.green() < 60);
}

#[test]
fn mask_hides_part_of_an_object() {
    let mut o = Object::new(1, ShapeKind::Rect { corner: 0.0 }, 0.0, 0.0, 100.0, 100.0);
    o.fill.a = [0, 0, 0, 255];
    let mut m = imageops::new_mask(10, 10).unwrap();
    for (i, px) in m.data_mut().chunks_exact_mut(4).enumerate() {
        if i % 10 < 5 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        }
    }
    o.mask = Some(Arc::new(m));
    let out = render_one(o, 100, 100);
    assert!(lum(&out, 20, 50) > 0.95, "left half hidden");
    assert!(lum(&out, 80, 50) < 0.05, "right half visible");
}

#[test]
fn picture_textures_for_fill_and_transparency() {
    // 2x1 picture: red | blue
    let mut pic = tiny_skia::Pixmap::new(2, 1).unwrap();
    pic.data_mut().copy_from_slice(&[255, 0, 0, 255, 0, 0, 255, 255]);
    let pic = ImageData(Arc::new(pic));
    let mut o = Object::new(1, ShapeKind::Rect { corner: 0.0 }, 0.0, 0.0, 100.0, 50.0);
    o.fill = FillStyle {
        kind: FillKind::Texture,
        texture: TextureKind::Image,
        image: Some(pic.clone()),
        image_stretch: true,
        ..Default::default()
    };
    let out = render_one(o.clone(), 100, 50);
    let c = |x| out.pixel(x, 25).unwrap().demultiply();
    assert!(c(15).red() > 200 && c(15).blue() < 60, "left is red");
    assert!(c(85).blue() > 200 && c(85).red() < 60, "right is blue");

    // brightness as opacity: black | white picture, dark = see-through
    let mut bw = tiny_skia::Pixmap::new(2, 1).unwrap();
    bw.data_mut().copy_from_slice(&[0, 0, 0, 255, 255, 255, 255, 255]);
    o.fill = FillStyle {
        kind: FillKind::Solid,
        a: [0, 0, 0, 255],
        ..Default::default()
    };
    o.transparency = Transparency {
        kind: TransparencyKind::Texture,
        texture: TextureKind::Image,
        image: Some(ImageData(Arc::new(bw))),
        start: 0.0,
        end: 1.0,
        ..Default::default()
    };
    let out = render_one(o, 100, 50);
    assert!(lum(&out, 10, 25) > 0.9, "dark part of the picture = transparent");
    assert!(lum(&out, 90, 25) < 0.1, "bright part = opaque");
}

#[test]
fn pictures_and_masks_survive_save_and_load() {
    let mut doc = Document::new(200, 200);
    let mut o = Object::new(1, ShapeKind::Bitmap, 10.0, 10.0, 120.0, 80.0);
    o.paint = Some(Arc::new(test_picture()));
    let mut m = imageops::new_mask(120, 80).unwrap();
    imageops::remove_background(o.paint.as_ref().unwrap(), &mut m, 0.15, 0.0);
    o.mask = Some(Arc::new(m));
    o.fill.image = Some(ImageData(Arc::new(test_picture())));
    doc.objects.push(o);
    let path = std::env::temp_dir().join("jabro_pic_roundtrip.jabro");
    crate::io::save(&doc, &path).unwrap();
    let back = crate::io::load(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    let b = &back.objects[0];
    assert_eq!(mask_at(b.mask.as_ref().unwrap(), 2, 2), 0);
    assert_eq!(b.fill.image.as_ref().unwrap().0.width(), 120);
}

// ------------------------------------------------------------------ export

use crate::export::{self, Format, Options};

fn export_doc() -> Document {
    let mut d = Document::new(200, 100);
    let mut a = Object::new(1, ShapeKind::Rect { corner: 0.2 }, 10.0, 10.0, 80.0, 60.0);
    a.name = "Plain box".into();
    a.fill.a = [200, 30, 30, 255];
    let mut b = Object::new(2, ShapeKind::Ellipse, 110.0, 10.0, 80.0, 80.0);
    b.name = "Shiny ball".into();
    b.effects.bevel.enabled = true;
    d.objects = vec![a, b];
    d
}

#[test]
fn hi_res_export_scales_everything() {
    let mut d = export_doc();
    d.objects[1].effects.shadow.enabled = true;
    let p = export::prepare(&d, None, 2.0).unwrap();
    assert_eq!((p.width, p.height), (400, 200));
    let b = &p.objects[1];
    assert!(b.quad[0].dist(Pt::new(220.0, 20.0)) < 1e-3 && b.size == [160.0, 160.0]);
    assert_eq!(b.effects.bevel.width, d.objects[1].effects.bevel.width * 2.0);
    assert_eq!(b.effects.shadow.blur, d.objects[1].effects.shadow.blur * 2.0);
    // selection only: cropped to the object
    let s = export::prepare(&d, Some(&[1]), 1.0).unwrap();
    assert_eq!((s.width, s.height, s.objects.len()), (80, 60, 1));
}

#[test]
fn jpeg_and_webp_files_decode() {
    let d = export_doc();
    for (fmt, transparent) in [(Format::Jpeg, false), (Format::Webp, true)] {
        let opts = Options { format: fmt, scale: 2.0, transparent, ..Default::default() };
        let path = std::env::temp_dir().join(format!("jabro_export_test.{}", fmt.ext()));
        export::export(&d, None, &opts, &path).unwrap();
        let img = image::open(&path).unwrap().to_rgba8();
        let _ = std::fs::remove_file(&path);
        assert_eq!(img.dimensions(), (400, 200), "{fmt:?}");
        let corner = img.get_pixel(1, 1);
        if transparent {
            assert_eq!(corner[3], 0, "transparent background kept");
        } else {
            assert!(corner[0] > 240, "white page");
        }
        let red = img.get_pixel(100, 80);
        assert!(red[0] > 150 && red[1] < 90, "{fmt:?}: {red:?}");
    }
}

#[test]
fn psd_has_one_named_layer_per_object() {
    let d = export_doc();
    let path = std::env::temp_dir().join("jabro_export_test.psd");
    export::export(&d, None, &Options { format: Format::Psd, ..Default::default() }, &path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    let psd = psd::Psd::from_bytes(&bytes).expect("valid PSD");
    assert_eq!((psd.width(), psd.height()), (200, 100));
    let names: Vec<&str> = psd.layers().iter().map(|l| l.name()).collect();
    // the reader lists top first; the file stores bottom first, like Photoshop
    assert_eq!(names, vec!["Shiny ball", "Plain box", "Background"]);
    let flat = psd.rgba();
    let i = ((40 * 200 + 50) * 4) as usize;
    assert!(flat[i] > 150 && flat[i + 1] < 90, "merged image shows the red box");
}

#[test]
fn svg_keeps_plain_shapes_as_vectors() {
    let mut d = export_doc();
    d.objects[0].quad[1] = Pt::new(95.0, 0.0); // a bit of perspective on the box
    let path = std::env::temp_dir().join("jabro_export_test.svg");
    export::export(&d, None, &Options { format: Format::Svg, ..Default::default() }, &path).unwrap();
    let svg = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert!(svg.starts_with("<svg") && svg.trim_end().ends_with("</svg>"));
    assert_eq!(svg.matches("<path").count(), 1, "plain box as a vector path");
    assert!(svg.contains("fill=\"#c81e1e\""));
    assert_eq!(svg.matches("<image").count(), 1, "beveled ball embedded as a picture");
    assert!(svg.contains("data:image/png;base64,"));

    // a warped box can't be a plain SVG path any more: it goes in as a picture too
    let mut m = Mesh::identity(1, 1);
    m.pts[3] = [1.3, 1.2];
    d.objects[0].mesh = Some(m);
    export::export(&d, None, &Options { format: Format::Svg, ..Default::default() }, &path).unwrap();
    let svg = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(svg.matches("<path").count(), 0);
    assert_eq!(svg.matches("<image").count(), 2);
}

// ------------------------------------------------------------------ per-object looks

fn saturation_at(pm: &tiny_skia::Pixmap, x: u32, y: u32) -> i32 {
    let c = pm.pixel(x, y).unwrap().demultiply();
    let (r, g, b) = (c.red() as i32, c.green() as i32, c.blue() as i32);
    r.max(g).max(b) - r.min(g).min(b)
}

fn two_red_boxes() -> Document {
    let mut d = Document::new(240, 120);
    for (i, x) in [(1, 20.0), (2, 140.0)] {
        let mut o = Object::new(i, ShapeKind::Rect { corner: 0.0 }, x, 20.0, 80.0, 80.0);
        o.fill.a = [220, 30, 30, 255];
        d.objects.push(o);
    }
    d
}

fn compose_doc(d: &Document) -> tiny_skia::Pixmap {
    let mut r = Renderer::default();
    let runs = r.render_runs(d);
    crate::npr::compose(&runs, d.width, d.height, d.background, 1.0)
}

#[test]
fn an_object_can_stay_clean_under_a_document_look() {
    let mut d = two_red_boxes();
    d.npr.style = NprStyle::Pencil; // greyscale look
    d.objects[1].npr_mode = NprMode::Off;
    let out = compose_doc(&d);
    assert!(saturation_at(&out, 60, 60) < 30, "box 1 is pencil-grey");
    assert!(saturation_at(&out, 180, 60) > 150, "box 2 stays red");
}

#[test]
fn an_object_can_have_its_own_look() {
    let mut d = two_red_boxes();
    d.objects[0].npr_mode = NprMode::Own;
    d.objects[0].npr_own = NprSettings { style: NprStyle::Pencil, ..Default::default() };
    assert_eq!(d.looks_for(&d.objects[1]), vec![], "no document look");
    let out = compose_doc(&d);
    assert!(saturation_at(&out, 60, 60) < 30, "box 1 sketched");
    assert!(saturation_at(&out, 180, 60) > 150, "box 2 clean");
    assert!(lum(&out, 120, 110) > 0.95, "page untouched far from the sketch");
}

#[test]
fn looks_stack_on_top_of_each_other() {
    let mut d = two_red_boxes();
    d.npr.style = NprStyle::Watercolor;
    let single = compose_doc(&d);
    d.npr_stack.push(NprSettings { style: NprStyle::ComicInk, ..Default::default() });
    assert_eq!(d.looks().len(), 2);
    let stacked = compose_doc(&d);
    let diff = single.data().iter().zip(stacked.data()).filter(|(a, b)| (**a as i32 - **b as i32).abs() > 20).count();
    assert!(diff > 1000, "the second look changes the picture ({diff})");
}

// ------------------------------------------------------------------ gradients, blend modes, adjustments

#[test]
fn multi_stop_gradient_along_chosen_points() {
    let mut o = Object::new(1, ShapeKind::Rect { corner: 0.0 }, 0.0, 0.0, 200.0, 50.0);
    o.fill = FillStyle {
        kind: FillKind::Linear,
        stops: vec![(0.0, [255, 0, 0, 255]), (0.5, [0, 255, 0, 255]), (1.0, [0, 0, 255, 255])],
        points: Some(([0.0, 0.5], [1.0, 0.5])),
        ..Default::default()
    };
    let out = render_one(o, 200, 50);
    let c = |x| out.pixel(x, 25).unwrap().demultiply();
    assert!(c(5).red() > 230 && c(5).green() < 30, "red at the start");
    assert!(c(100).green() > 230 && c(100).red() < 30, "green in the middle");
    assert!(c(195).blue() > 230 && c(195).green() < 30, "blue at the end");
}

#[test]
fn multiply_blend_darkens_what_is_underneath() {
    let mut d = Document::new(100, 100);
    let mut a = Object::new(1, ShapeKind::Rect { corner: 0.0 }, 0.0, 0.0, 100.0, 100.0);
    a.fill.a = [255, 0, 0, 255];
    let mut b = Object::new(2, ShapeKind::Rect { corner: 0.0 }, 50.0, 0.0, 50.0, 100.0);
    b.fill.a = [0, 0, 255, 255];
    b.blend = Blend::Multiply;
    d.objects = vec![a, b];
    let out = Renderer::default().render(&d);
    let p = out.pixel(75, 50).unwrap().demultiply();
    assert!(p.red() < 20 && p.blue() < 20, "red × blue = black: {p:?}");
    let q = out.pixel(25, 50).unwrap().demultiply();
    assert!(q.red() > 240, "untouched part stays red");
}

#[test]
fn colour_adjustments_shift_hue_saturation_and_levels() {
    let red = |adjust: Adjust| {
        let mut o = Object::new(1, ShapeKind::Rect { corner: 0.0 }, 0.0, 0.0, 40.0, 40.0);
        o.fill.a = [200, 40, 40, 255];
        o.adjust = adjust;
        render_one(o, 40, 40).pixel(20, 20).unwrap().demultiply()
    };
    let green = red(Adjust { hue: 120.0, ..Default::default() });
    assert!(green.green() > 150 && green.red() < 80, "hue +120: red becomes green {green:?}");
    let grey = red(Adjust { saturation: -1.0, ..Default::default() });
    assert!((grey.red() as i32 - grey.blue() as i32).abs() < 4, "no colour left {grey:?}");
    let dark = red(Adjust { black: 0.5, ..Default::default() });
    // black point 0.5: 0.78 -> (0.78 - 0.5) / 0.5 = 0.57 (145); 0.16 -> 0
    assert!(dark.green() == 0 && (dark.red() as i32 - 145).abs() <= 2, "levels crush the shadows {dark:?}");
}

#[test]
fn mesh_warp_bends_the_rendered_object_and_its_hit_area() {
    let mut doc = Document::new(220, 220);
    let id = doc.alloc_id();
    let mut obj = Object::new(id, ShapeKind::Rect { corner: 0.0 }, 50.0, 50.0, 100.0, 100.0);
    obj.fill.a = [255, 0, 0, 255];
    let mut mesh = Mesh::identity(1, 1);
    mesh.pts[3] = [1.4, 1.4]; // pull the bottom-right corner out to (190, 190)
    obj.mesh = Some(mesh);
    doc.objects.push(obj);
    let mut r = Renderer::default();
    let out = r.render(&doc);
    let px = |x: u32, y: u32| out.pixel(x, y).unwrap().demultiply();
    assert!(px(165, 165).red() > 240 && px(165, 165).green() < 20, "stretched corner is filled");
    assert!(px(145, 60).red() > 240 && px(145, 60).green() < 20, "top edge still in place");
    assert!(px(40, 40).green() > 240, "outside stays background");
    assert!(px(185, 70).green() > 240, "right of the slanted edge is background");
    let o = &doc.objects[0];
    assert!(r.hit(o, Pt::new(170.0, 170.0)), "hit test follows the warp");
    assert!(!r.hit(o, Pt::new(185.0, 70.0)));
}

#[test]
fn scatter_brush_sprays_every_shape_with_colour_variation() {
    use crate::brush::{BrushKind, ScatterShape};
    for shape in ScatterShape::ALL {
        let mut layer = Pixmap::new(240, 80).unwrap();
        let b = BrushSettings {
            kind: BrushKind::Scatter,
            size: 30.0,
            flow: 1.0,
            color: [60, 140, 40, 255],
            scatter: shape,
            jitter: 0.8,
            ..Default::default()
        };
        let mut st = StrokeState::new();
        brush::segment(&mut layer, (20.0, 40.0), (220.0, 40.0), 15.0, &b, &mut st);
        let mut colours = std::collections::HashSet::new();
        let mut painted = 0;
        for p in layer.pixels() {
            if p.alpha() == 255 {
                painted += 1;
                colours.insert((p.red() / 8, p.green() / 8, p.blue() / 8));
            }
        }
        assert!(painted > 400, "{shape:?} sprays something ({painted} px)");
        assert!(colours.len() >= 4, "{shape:?} varies its colour ({} shades)", colours.len());
    }
}

fn watercolor_stroke(pieces: usize) -> Pixmap {
    use crate::brush::BrushKind;
    let mut layer = Pixmap::new(220, 100).unwrap();
    let b = BrushSettings {
        kind: BrushKind::Watercolor,
        size: 40.0,
        flow: 0.8,
        color: [30, 60, 200, 255],
        ..Default::default()
    };
    let mut st = StrokeState::new();
    let (a, z) = ((20.0f32, 50.0f32), (200.0f32, 50.0f32));
    for k in 0..pieces {
        let f0 = k as f32 / pieces as f32;
        let f1 = (k + 1) as f32 / pieces as f32;
        let p0 = (a.0 + (z.0 - a.0) * f0, a.1);
        let p1 = (a.0 + (z.0 - a.0) * f1, a.1);
        brush::segment(&mut layer, p0, p1, 20.0, &b, &mut st);
    }
    layer
}

#[test]
fn watercolor_pools_pigment_at_the_edges() {
    let layer = watercolor_stroke(10);
    // average alpha per row over the middle of the stroke
    let row = |y: u32| (70..150).map(|x| layer.pixel(x, y).unwrap().alpha() as f32).sum::<f32>() / 80.0;
    let centre = row(50);
    let rim = (31..=48).map(|y| row(y).max(row(100 - y))).fold(0.0, f32::max);
    assert!(centre > 30.0, "the wash is visible ({centre})");
    assert!(rim > centre * 1.3, "the rim is darker than the middle (rim {rim}, centre {centre})");
    assert!(row(5) < 1.0 && row(95) < 1.0, "nothing outside the stroke");
}

#[test]
fn watercolor_incremental_rendering_matches_a_single_pass() {
    let one = watercolor_stroke(1);
    let many = watercolor_stroke(12);
    let worst = one
        .data()
        .iter()
        .zip(many.data())
        .map(|(a, b)| (*a as i32 - *b as i32).abs())
        .max()
        .unwrap();
    assert!(worst <= 3, "mouse-move by mouse-move painting gives the same result (max diff {worst})");
}

// ------------------------------------------------------------------ text layout & 3D extrude

fn text_object(s: &str) -> Object {
    let kind = ShapeKind::Text { text: s.into(), font: FontChoice::Sans, layout: TextLayout::default() };
    let mut o = Object::new(1, kind, 100.0, 100.0, 300.0, 80.0);
    o.fit_text_width();
    o
}

fn set_layout(o: &mut Object, f: impl FnOnce(&mut TextLayout)) {
    let before = o.text_bounds();
    if let ShapeKind::Text { layout, .. } = &mut o.kind {
        f(layout);
    }
    o.relayout_text(before);
}

#[test]
fn letter_spacing_widens_the_text_and_keeps_letter_size() {
    let mut o = text_object("Hello");
    let (w0, h0, left) = (o.size[0], o.size[1], o.quad[0]);
    set_layout(&mut o, |l| l.spacing = 0.3);
    assert!(o.size[0] > w0 * 1.25, "wider: {} -> {}", w0, o.size[0]);
    assert!((o.size[1] - h0).abs() < 0.5, "same letter height");
    assert!(o.quad[0].dist(left) < 0.5, "stays where it was");
    set_layout(&mut o, |l| l.spacing = 0.0);
    assert!((o.size[0] - w0).abs() < 0.5, "and back");
}

#[test]
fn arc_bends_the_baseline_and_keeps_letter_size() {
    let mut o = text_object("ABCDEFGHIJ");
    let (w0, h0) = (o.size[0], o.size[1]);
    set_layout(&mut o, |l| l.arc = 180.0);
    assert!(o.size[1] > h0 * 1.8, "an arch is much taller: {} -> {}", h0, o.size[1]);
    assert!(o.size[0] < w0, "and narrower");
    // the middle letters sit at the top of the box, the ends lower down
    let mut doc = Document::new(800, 600);
    doc.objects.push(o.clone());
    let out = Renderer::default().render(&doc);
    let (lo, hi) = o.bounds();
    let ink = |x: f32| (lo.y as u32..hi.y as u32).find(|&y| out.pixel(x as u32, y).unwrap().demultiply().red() < 128);
    let mid = ink((lo.x + hi.x) * 0.5).expect("ink in the middle");
    let end = ink(lo.x + (hi.x - lo.x) * 0.06).expect("ink at the left end");
    assert!(end > mid + 40, "ends hang lower (mid {mid}, end {end})");
    set_layout(&mut o, |l| l.arc = 0.0);
    assert!((o.size[1] - h0).abs() < 1.0 && (o.size[0] - w0).abs() < 1.0, "straight again");
}

#[test]
fn line_spacing_and_alignment() {
    let mut o = text_object("Wide line\nab");
    let h0 = o.size[1];
    set_layout(&mut o, |l| l.leading = 0.5);
    assert!(o.size[1] > h0 * 1.15, "more space between lines");
    // left aligned: the short line starts at the left edge of the box
    set_layout(&mut o, |l| l.align = TextAlign::Left);
    let mut doc = Document::new(800, 600);
    doc.objects.push(o.clone());
    let out = Renderer::default().render(&doc);
    let (lo, hi) = o.bounds();
    let row = (lo.y + (hi.y - lo.y) * 0.8) as u32;
    let first = (lo.x as u32..hi.x as u32).find(|&x| out.pixel(x, row).unwrap().demultiply().red() < 128).unwrap();
    assert!((first as f32) < lo.x + (hi.x - lo.x) * 0.1, "second line starts at the left");
}

fn extrude_doc(taper: f32) -> Document {
    let mut doc = Document::new(240, 240);
    let id = doc.alloc_id();
    let mut o = Object::new(id, ShapeKind::Rect { corner: 0.0 }, 60.0, 60.0, 80.0, 80.0);
    o.fill.a = [220, 40, 40, 255];
    o.effects.extrude = Extrude { enabled: true, depth: 30.0, angle: 315.0, taper, ..Default::default() };
    doc.objects.push(o);
    doc
}

#[test]
fn extrude_adds_lit_sides_behind_the_face() {
    let out = Renderer::default().render(&extrude_doc(0.0));
    let px = |x: u32, y: u32| out.pixel(x, y).unwrap().demultiply();
    assert_eq!((px(100, 100).red(), px(100, 100).green()), (220, 40), "the face is untouched");
    let corner = px(152, 152);
    assert!(corner.green() < 100 && corner.red() < 220, "sides fill in behind, darker: {corner:?}");
    assert!(px(50, 50).green() > 240 && px(156, 63).green() > 240, "nothing in front or beside");
    // light from the upper right: the right-hand side is brighter than the bottom
    let (right, bottom) = (px(146, 110), px(110, 146));
    assert!(right.red() > bottom.red() + 30, "right {right:?} vs bottom {bottom:?}");
    // the extruded body is part of the object's own layer, so shadows include it
    let tapered = Renderer::default().render(&extrude_doc(0.9));
    assert!(tapered.pixel(155, 155).unwrap().demultiply().green() > 240, "a vanishing back stays small");
}

// ------------------------------------------------------------------ animation export

fn moving_box_doc() -> Document {
    let mut doc = Document::new(120, 80);
    let id = doc.alloc_id();
    let mut o = Object::new(id, ShapeKind::Rect { corner: 0.0 }, 10.0, 20.0, 30.0, 30.0);
    o.fill.a = [200, 30, 30, 255];
    o.set_key(0);
    o.quad = Object::rect_quad(80.0, 20.0, 30.0, 30.0);
    o.set_key(5);
    doc.objects.push(o);
    doc.anim.frames = 6;
    doc.anim.ease = false;
    doc
}

fn gif_frames(path: &std::path::Path) -> Vec<image::Frame> {
    use image::AnimationDecoder;
    let f = std::io::BufReader::new(std::fs::File::open(path).unwrap());
    image::codecs::gif::GifDecoder::new(f).unwrap().into_frames().collect_frames().unwrap()
}

#[test]
fn animation_exports_every_frame_as_gif_and_png_sequence() {
    use crate::anim::{self, AnimFormat};
    let doc = moving_box_doc();
    let (tx, _rx) = std::sync::mpsc::channel();
    let gif = std::env::temp_dir().join("jabro_anim_test.gif");
    anim::export_animation(&doc, &export::Options::default(), AnimFormat::Gif, &gif, &tx).unwrap();
    let frames = gif_frames(&gif);
    let _ = std::fs::remove_file(&gif);
    assert_eq!(frames.len(), 6);
    let red = |f: &image::Frame, x: u32| f.buffer().get_pixel(x, 35)[1] < 100;
    assert!(red(&frames[0], 20) && !red(&frames[0], 95), "starts on the left");
    assert!(!red(&frames[5], 20) && red(&frames[5], 95), "ends on the right");

    let png = std::env::temp_dir().join("jabro_anim_seq.png");
    anim::export_animation(&doc, &export::Options::default(), AnimFormat::PngSequence, &png, &tx).unwrap();
    for i in 0..6 {
        let p = anim::frame_path(&png, i);
        assert!(p.exists(), "{} written", p.display());
        let _ = std::fs::remove_file(p);
    }
    // the document itself is untouched by exporting
    assert_eq!(doc.objects[0].quad[0], Pt::new(80.0, 20.0));
}

#[test]
fn timelapse_export_holds_the_last_frame() {
    use crate::anim::{self, AnimFormat};
    let mut doc = moving_box_doc();
    let mut shots = Vec::new();
    for f in [0, 2, 5] {
        doc.apply_frame(f);
        let pm = Renderer::default().render(&doc);
        shots.push(Arc::new(anim::snapshot(&pm, 640, doc.background).unwrap()));
    }
    let (tx, _rx) = std::sync::mpsc::channel();
    let gif = std::env::temp_dir().join("jabro_timelapse_test.gif");
    anim::export_timelapse(&shots, 3.0, AnimFormat::Gif, &gif, &tx).unwrap();
    let frames = gif_frames(&gif);
    let _ = std::fs::remove_file(&gif);
    assert_eq!(frames.len(), 3);
    let (n, d) = frames[2].delay().numer_denom_ms();
    assert!(n / d >= 2000, "the finished picture stays a while");
}
