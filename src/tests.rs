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
