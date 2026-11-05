//! The editor UI (egui): tools, canvas interaction, property panels, menus.

use crate::brush::{self, BrushKind, BrushSettings, StrokeState};
use crate::doc::*;
use crate::geom::{simplify, Pt};
use crate::npr::{self, NprJob, NprWorker};
use crate::render::Renderer;
use crate::vector::{self, BoolOp};
use crate::{demo, io, shapes, styles};
use eframe::egui::{self, Color32, Modifiers, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use std::path::PathBuf;
use std::sync::Arc;
use tiny_skia::Pixmap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Node,
    Perspective,
    Pen,
    Rect,
    Ellipse,
    Polygon,
    Freehand,
    Text,
    Brush,
}

impl Tool {
    const ALL: [Tool; 10] = [
        Tool::Select,
        Tool::Node,
        Tool::Perspective,
        Tool::Pen,
        Tool::Rect,
        Tool::Ellipse,
        Tool::Polygon,
        Tool::Freehand,
        Tool::Text,
        Tool::Brush,
    ];
    fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select  (V)",
            Tool::Node => "Edit nodes  (A)",
            Tool::Pen => "Pen  (P)",
            Tool::Perspective => "Perspective  (Q)",
            Tool::Rect => "Rectangle  (R)",
            Tool::Ellipse => "Ellipse  (E)",
            Tool::Polygon => "Star / Polygon  (S)",
            Tool::Freehand => "Freehand shape  (F)",
            Tool::Text => "Text  (T)",
            Tool::Brush => "Brush  (B)",
        }
    }
    fn key(self) -> egui::Key {
        match self {
            Tool::Select => egui::Key::V,
            Tool::Node => egui::Key::A,
            Tool::Pen => egui::Key::P,
            Tool::Perspective => egui::Key::Q,
            Tool::Rect => egui::Key::R,
            Tool::Ellipse => egui::Key::E,
            Tool::Polygon => egui::Key::S,
            Tool::Freehand => egui::Key::F,
            Tool::Text => egui::Key::T,
            Tool::Brush => egui::Key::B,
        }
    }
    fn hint(self) -> &'static str {
        match self {
            Tool::Select => "Click to select, Shift+click or drag a box to select several. Handles scale, the round handle rotates. Double-click to edit the shape (or the text).",
            Tool::Node => "Drag nodes and handles (Alt breaks a smooth handle). Click a segment to add a node, double-click a node for smooth/corner, Del removes it.",
            Tool::Pen => "Click for corners, click+drag for curves. Click the first node to close, Enter to finish an open path, Backspace undoes a node, Esc cancels.",
            Tool::Perspective => "Drag any corner of the selected object to put it in perspective — paint follows along.",
            Tool::Rect => "Drag to draw a rectangle (Shift = square). Click for a default size.",
            Tool::Ellipse => "Drag to draw an ellipse (Shift = circle).",
            Tool::Polygon => "Drag to draw a star or polygon. Sides / star depth in the tool panel.",
            Tool::Freehand => "Draw a closed freehand shape — it becomes a smooth vector curve.",
            Tool::Text => "Click or drag a box to place text, then edit it in the properties panel.",
            Tool::Brush => "Paint on the selected object (clipped to its shape) or on a new paint layer. Alt+click picks a colour.",
        }
    }
}

enum Drag {
    None,
    Move { start: Pt, items: Vec<(u64, [Pt; 4])> },
    Marquee { start: Pt, cur: Pt, additive: bool },
    Node { id: u64, sp: usize, ni: usize, offset: Pt },
    Handle { id: u64, sp: usize, ni: usize, out: bool },
    PenHandle,
    Scale { id: u64, handle: usize, anchor: Pt, h0: Pt, orig: [Pt; 4], angle: f32 },
    Rotate { id: u64, center: Pt, a0: f32, orig: [Pt; 4] },
    Corner { id: u64, idx: usize },
    Create { start: Pt, cur: Pt },
    Freehand { pts: Vec<Pt> },
    Paint { id: u64, last: (f32, f32), state: StrokeState },
}

enum PropAction {
    ConvertToPath,
    EditNodes,
    ResetPerspective,
    FlipH,
    FlipV,
    ClearPaint,
    CopyStyle,
    PasteStyle,
    Front,
    Back,
}

fn handle_pos(q: &[Pt; 4], i: usize) -> Pt {
    match i {
        0 => q[0],
        1 => q[0].lerp(q[1], 0.5),
        2 => q[1],
        3 => q[1].lerp(q[2], 0.5),
        4 => q[2],
        5 => q[2].lerp(q[3], 0.5),
        6 => q[3],
        _ => q[3].lerp(q[0], 0.5),
    }
}

/// Canvas point -> (paint-layer pixel, paint pixels per canvas pixel).
fn doc_to_paint(obj: &Object, p: Pt) -> Option<((f32, f32), f32)> {
    let paint = obj.paint.as_ref()?;
    let inv = obj.homography().inverse()?;
    let (lx, ly) = inv.apply(p.x as f64, p.y as f64)?;
    let (lx2, ly2) = inv.apply(p.x as f64 + 1.0, p.y as f64)?;
    let (lx3, ly3) = inv.apply(p.x as f64, p.y as f64 + 1.0)?;
    let kx = paint.width() as f64 / obj.size[0].max(1.0) as f64;
    let ky = paint.height() as f64 / obj.size[1].max(1.0) as f64;
    let sx = ((lx2 - lx) * kx).hypot((ly2 - ly) * ky);
    let sy = ((lx3 - lx) * kx).hypot((ly3 - ly) * ky);
    Some((((lx * kx) as f32, (ly * ky) as f32), ((sx + sy) * 0.5) as f32))
}

fn rgba_label(ui: &mut egui::Ui, label: &str, c: &mut Rgba) {
    ui.horizontal(|ui| {
        ui.color_edit_button_srgba_unmultiplied(c);
        ui.label(label);
    });
}

/// A path being drawn with the pen tool (node coordinates are canvas pixels).
struct PenDraft {
    nodes: Vec<PathNode>,
    closing: bool,
}

pub struct JabroApp {
    doc: Document,
    /// Primary selection (shown in the properties panel).
    selected: Option<u64>,
    /// Additional selected objects (Shift+click / marquee).
    multi: Vec<u64>,
    /// Selected node for the node tool: (object, sub-path, node).
    node_sel: Option<(u64, usize, usize)>,
    pen: Option<PenDraft>,
    stroke_width: f32,
    /// Put the keyboard focus into the text field of the properties panel next frame.
    focus_text: bool,
    tool: Tool,
    color_a: Rgba,
    color_b: Rgba,
    brush: BrushSettings,
    poly_sides: u32,
    poly_inner: f32,
    text_font: FontChoice,
    renderer: Renderer,
    raw: Option<Pixmap>,
    texture: Option<egui::TextureHandle>,
    zoom: f32,
    pan: Vec2,
    fit_pending: bool,
    canvas_rect: Rect,
    drag: Drag,
    pressing: bool,
    last_drag_pos: Option<Pt>,
    snapshot: Option<Document>,
    undo: Vec<Document>,
    redo: Vec<Document>,
    dirty: bool,
    gen: u64,
    npr_preview: bool,
    npr_pending: bool,
    npr_busy: bool,
    worker: NprWorker,
    status: String,
    path: Option<PathBuf>,
    prop_edit_open: bool,
    cursor_doc: Option<Pt>,
    pressure: Option<f32>,
    style_clipboard: Option<(FillStyle, StrokeStyle, Effects)>,
    show_about: bool,
    new_w: u32,
    new_h: u32,
}

impl JabroApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let ctx = cc.egui_ctx.clone();
        Self::with_worker(NprWorker::new(move || ctx.request_repaint()))
    }

    fn with_worker(worker: NprWorker) -> Self {
        let doc = demo::demo_document();
        let (new_w, new_h) = (doc.width, doc.height);
        Self {
            doc,
            selected: None,
            multi: Vec::new(),
            node_sel: None,
            pen: None,
            stroke_width: 4.0,
            focus_text: false,
            tool: Tool::Select,
            color_a: [220, 60, 50, 255],
            color_b: [40, 90, 200, 255],
            brush: BrushSettings::default(),
            poly_sides: 5,
            poly_inner: 0.5,
            text_font: FontChoice::Black,
            renderer: Renderer::default(),
            raw: None,
            texture: None,
            zoom: 1.0,
            pan: Vec2::ZERO,
            fit_pending: true,
            canvas_rect: Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0)),
            drag: Drag::None,
            pressing: false,
            last_drag_pos: None,
            snapshot: None,
            undo: Vec::new(),
            redo: Vec::new(),
            dirty: true,
            gen: 0,
            npr_preview: true,
            npr_pending: false,
            npr_busy: false,
            worker,
            status: "Welcome to Jabro Draw — this demo scene is fully editable.".into(),
            path: None,
            prop_edit_open: false,
            cursor_doc: None,
            pressure: None,
            style_clipboard: None,
            show_about: false,
            new_w,
            new_h,
        }
    }

    // ---------------------------------------------------------------- history

    fn push_undo(&mut self, d: Document) {
        self.undo.push(d);
        if self.undo.len() > 80 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn checkpoint(&mut self) {
        let d = self.doc.clone();
        self.push_undo(d);
    }

    fn commit_snapshot(&mut self) {
        if let Some(s) = self.snapshot.take() {
            self.push_undo(s);
        }
    }

    fn after_doc_swap(&mut self) {
        self.dirty = true;
        if let Some(id) = self.selected {
            if self.doc.find(id).is_none() {
                self.selected = None;
            }
        }
        let doc = &self.doc;
        self.multi.retain(|id| doc.find(*id).is_some());
        self.validate_node_sel();
    }

    // ---------------------------------------------------------------- selection

    /// All selected object ids, bottom-most first.
    fn selection(&self) -> Vec<u64> {
        self.doc
            .objects
            .iter()
            .filter(|o| Some(o.id) == self.selected || self.multi.contains(&o.id))
            .map(|o| o.id)
            .collect()
    }

    fn is_selected(&self, id: u64) -> bool {
        self.selected == Some(id) || self.multi.contains(&id)
    }

    fn select_only(&mut self, id: Option<u64>) {
        if self.selected != id {
            self.node_sel = None;
        }
        self.selected = id;
        self.multi.clear();
    }

    fn toggle_in_selection(&mut self, id: u64) {
        if self.selected == Some(id) {
            self.selected = self.multi.pop();
        } else if let Some(pos) = self.multi.iter().position(|m| *m == id) {
            self.multi.remove(pos);
        } else {
            if let Some(old) = self.selected {
                self.multi.push(old);
            }
            self.selected = Some(id);
        }
        self.node_sel = None;
    }

    fn validate_node_sel(&mut self) {
        if let Some((id, sp, ni)) = self.node_sel {
            let ok = self.selected == Some(id)
                && matches!(self.doc.find(id).map(|o| &o.kind),
                    Some(ShapeKind::Path { subpaths }) if subpaths.get(sp).is_some_and(|s| ni < s.nodes.len()));
            if !ok {
                self.node_sel = None;
            }
        }
    }

    // ---------------------------------------------------------------- paths

    fn boolean(&mut self, op: BoolOp) {
        let ids = self.selection();
        let mut used = Vec::new();
        let mut outlines = Vec::new();
        for id in &ids {
            if let Some(c) = self.doc.find(*id).and_then(vector::outline_doc) {
                used.push(*id);
                outlines.push(c);
            }
        }
        if outlines.len() < 2 {
            self.status = "Select at least two vector objects (Shift+click or drag a box).".into();
            return;
        }
        let shapes = vector::boolean(&outlines, op);
        let Some((subpaths, quad, size)) = vector::polygons_to_object_parts(&shapes) else {
            self.status = format!("{} left nothing — objects unchanged.", op.label());
            return;
        };
        self.checkpoint();
        let base = self.doc.find(used[0]).expect("base").clone();
        let index = self.doc.index_of(used[0]).unwrap_or(0);
        let id = self.doc.alloc_id();
        let mut obj = Object::new(id, ShapeKind::Path { subpaths }, 0.0, 0.0, size[0], size[1]);
        obj.quad = quad;
        obj.name = format!("{} {}", op.label(), id);
        obj.fill = base.fill.clone();
        obj.stroke = base.stroke.clone();
        obj.effects = base.effects.clone();
        obj.opacity = base.opacity;
        self.doc.objects.retain(|o| !used.contains(&o.id));
        let index = index.min(self.doc.objects.len());
        self.doc.objects.insert(index, obj);
        self.select_only(Some(id));
        self.dirty = true;
        self.status = format!("{}: {} objects combined into one editable path.", op.label(), used.len());
    }

    fn convert_selected_to_path(&mut self) {
        let ids = self.selection();
        let convertible = ids
            .iter()
            .filter(|id| {
                self.doc
                    .find(**id)
                    .is_some_and(|o| !matches!(o.kind, ShapeKind::Path { .. } | ShapeKind::Bitmap))
            })
            .count();
        if convertible == 0 {
            self.status = "Nothing to convert (already paths, or paint layers).".into();
            return;
        }
        self.checkpoint();
        for id in ids {
            if let Some(o) = self.doc.find_mut(id) {
                vector::convert_to_path(o);
            }
        }
        self.dirty = true;
        self.status = format!("Converted {convertible} object(s) to editable paths.");
    }

    fn finish_pen(&mut self, closed: bool) {
        let Some(draft) = self.pen.take() else { return };
        if draft.nodes.len() < 2 {
            return;
        }
        let sp = SubPath {
            nodes: draft.nodes,
            closed,
        };
        let Some(b) = vector::build(std::slice::from_ref(&sp), 1.0, 1.0).and_then(|p| p.compute_tight_bounds())
        else {
            return;
        };
        let (w, h) = (b.width().max(1.0), b.height().max(1.0));
        let (x0, y0) = (b.left() - (w - b.width()) * 0.5, b.top() - (h - b.height()) * 0.5);
        let norm = |q: [f32; 2]| [(q[0] - x0) / w, (q[1] - y0) / h];
        let nodes = sp
            .nodes
            .iter()
            .map(|n| PathNode {
                p: norm(n.p),
                h_in: norm(n.h_in),
                h_out: norm(n.h_out),
                smooth: n.smooth,
            })
            .collect();
        if self.snapshot.is_some() {
            self.commit_snapshot();
        } else {
            self.checkpoint();
        }
        let id = self.doc.alloc_id();
        let kind = ShapeKind::Path {
            subpaths: vec![SubPath { nodes, closed }],
        };
        let mut obj = Object::new(id, kind, x0, y0, w, h);
        if closed {
            obj.fill = FillStyle {
                kind: FillKind::Solid,
                a: self.color_a,
                b: self.color_b,
                ..Default::default()
            };
        } else {
            obj.fill.kind = FillKind::None;
            obj.stroke = StrokeStyle {
                enabled: true,
                color: self.color_a,
                width: self.stroke_width,
            };
        }
        self.doc.objects.push(obj);
        self.select_only(Some(id));
        self.dirty = true;
        self.status = if closed { "Closed path created." } else { "Open path created (stroke only)." }.into();
    }

    /// Node / handle positions of a path object in screen space.
    fn norm_to_screen(&self, obj: &Object, q: [f32; 2]) -> Option<Pos2> {
        let hm = obj.homography();
        let p = hm.apply_pt(Pt::new(q[0] * obj.size[0], q[1] * obj.size[1]))?;
        Some(self.to_screen(p))
    }

    fn doc_to_norm(obj: &Object, p: Pt) -> Option<[f32; 2]> {
        let l = obj.homography().inverse()?.apply_pt(p)?;
        Some([l.x / obj.size[0].max(1e-3), l.y / obj.size[1].max(1e-3)])
    }

    /// Find a node or handle of the selected path under the cursor.
    fn pick_node(&self, sp_screen: Pos2) -> Option<Drag> {
        let id = self.selected?;
        let obj = self.doc.find(id)?;
        let ShapeKind::Path { subpaths } = &obj.kind else { return None };
        if let Some((sid, s, n)) = self.node_sel {
            if sid == id {
                if let Some(sub) = subpaths.get(s) {
                    let len = sub.nodes.len();
                    // the selected node's handles plus the facing handles of its neighbours
                    let mut cands = vec![(n, true), (n, false)];
                    if len > 1 && (sub.closed || n > 0) {
                        cands.push(((n + len - 1) % len, true));
                    }
                    if len > 1 && (sub.closed || n + 1 < len) {
                        cands.push(((n + 1) % len, false));
                    }
                    for (ni, out) in cands {
                        let Some(nd) = sub.nodes.get(ni) else { continue };
                        let h = if out { nd.h_out } else { nd.h_in };
                        if h != nd.p && self.norm_to_screen(obj, h)?.distance(sp_screen) < 7.0 {
                            return Some(Drag::Handle { id, sp: s, ni, out });
                        }
                    }
                }
            }
        }
        for (s, sub) in subpaths.iter().enumerate() {
            for (n, nd) in sub.nodes.iter().enumerate() {
                if self.norm_to_screen(obj, nd.p)?.distance(sp_screen) < 7.0 {
                    return Some(Drag::Node { id, sp: s, ni: n, offset: Pt::default() });
                }
            }
        }
        None
    }

    /// Nearest point on a segment of the selected path: (sub-path, segment index, t).
    fn pick_segment(&self, sp_screen: Pos2) -> Option<(usize, usize, f32)> {
        let obj = self.doc.find(self.selected?)?;
        let ShapeKind::Path { subpaths } = &obj.kind else { return None };
        let mut best: Option<(f32, usize, usize, f32)> = None;
        for (s, sub) in subpaths.iter().enumerate() {
            let n = sub.nodes.len();
            let segs = if sub.closed { n } else { n.saturating_sub(1) };
            for i in 0..segs {
                let (a, b) = (sub.nodes[i], sub.nodes[(i + 1) % n]);
                let pts = [a.p, a.h_out, b.h_in, b.p].map(|q| self.norm_to_screen(obj, q));
                let [Some(p0), Some(c1), Some(c2), Some(p3)] = pts else { continue };
                let (p0, c1, c2, p3) = (
                    Pt::new(p0.x, p0.y),
                    Pt::new(c1.x, c1.y),
                    Pt::new(c2.x, c2.y),
                    Pt::new(p3.x, p3.y),
                );
                for k in 1..40 {
                    let t = k as f32 / 40.0;
                    let q = vector::cubic(p0, c1, c2, p3, t);
                    let d = q.dist(Pt::new(sp_screen.x, sp_screen.y));
                    if d < 6.0 && best.is_none_or(|b| d < b.0) {
                        best = Some((d, s, i, t));
                    }
                }
            }
        }
        best.map(|(_, s, i, t)| (s, i, t))
    }

    fn delete_node(&mut self) {
        let Some((id, s, n)) = self.node_sel else { return };
        self.checkpoint();
        let mut remove_obj = false;
        if let Some(o) = self.doc.find_mut(id) {
            if let ShapeKind::Path { subpaths } = &mut o.kind {
                if let Some(sub) = subpaths.get_mut(s) {
                    sub.nodes.remove(n.min(sub.nodes.len().saturating_sub(1)));
                    if sub.nodes.len() < 2 {
                        subpaths.remove(s);
                    }
                }
                remove_obj = subpaths.is_empty();
            }
            if !remove_obj {
                vector::normalize_bounds(o);
                o.touch();
            }
        }
        if remove_obj {
            self.doc.objects.retain(|o| o.id != id);
            self.select_only(None);
        }
        self.node_sel = None;
        self.dirty = true;
    }

    fn undo(&mut self) {
        if let Some(d) = self.undo.pop() {
            let cur = std::mem::replace(&mut self.doc, d);
            self.redo.push(cur);
            self.after_doc_swap();
            self.status = "Undo".into();
        }
    }

    fn redo(&mut self) {
        if let Some(d) = self.redo.pop() {
            let cur = std::mem::replace(&mut self.doc, d);
            self.undo.push(cur);
            self.after_doc_swap();
            self.status = "Redo".into();
        }
    }

    // ---------------------------------------------------------------- files

    fn replace_doc(&mut self, doc: Document) {
        self.checkpoint();
        self.doc = doc;
        self.renderer.clear();
        self.selected = None;
        self.new_w = self.doc.width;
        self.new_h = self.doc.height;
        self.fit_pending = true;
        self.dirty = true;
    }

    fn new_doc(&mut self) {
        let mut d = Document::new(self.new_w.max(16), self.new_h.max(16));
        d.npr = self.doc.npr.clone();
        d.npr.style = NprStyle::None;
        self.replace_doc(d);
        self.path = None;
        self.status = "New document".into();
    }

    fn open(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Jabro Draw", &["jabro"])
            .pick_file()
        else {
            return;
        };
        match io::load(&path) {
            Ok(d) => {
                self.replace_doc(d);
                self.status = format!("Opened {}", path.display());
                self.path = Some(path);
            }
            Err(e) => self.status = format!("Could not open: {e}"),
        }
    }

    fn save(&mut self, save_as: bool) {
        let path = match (&self.path, save_as) {
            (Some(p), false) => p.clone(),
            _ => {
                let Some(p) = rfd::FileDialog::new()
                    .add_filter("Jabro Draw", &["jabro"])
                    .set_file_name("drawing.jabro")
                    .save_file()
                else {
                    return;
                };
                p
            }
        };
        match io::save(&self.doc, &path) {
            Ok(()) => {
                self.status = format!("Saved {}", path.display());
                self.path = Some(path);
            }
            Err(e) => self.status = format!("Could not save: {e}"),
        }
    }

    fn export_png(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PNG image", &["png"])
            .set_file_name("jabro.png")
            .save_file()
        else {
            return;
        };
        let raw = self.renderer.render(&self.doc);
        let out = npr::apply(&raw, &self.doc.npr);
        match io::export_png(&out, &path) {
            Ok(()) => self.status = format!("Exported {}", path.display()),
            Err(e) => self.status = format!("Export failed: {e}"),
        }
    }

    // ---------------------------------------------------------------- view

    fn origin(&self) -> Pos2 {
        let r = self.canvas_rect;
        let size = Vec2::new(self.doc.width as f32, self.doc.height as f32) * self.zoom;
        r.center() + self.pan - size * 0.5
    }

    fn to_screen(&self, p: Pt) -> Pos2 {
        let o = self.origin();
        Pos2::new(o.x + p.x * self.zoom, o.y + p.y * self.zoom)
    }

    fn to_doc(&self, p: Pos2) -> Pt {
        let o = self.origin();
        Pt::new((p.x - o.x) / self.zoom, (p.y - o.y) / self.zoom)
    }

    fn fit(&mut self) {
        let r = self.canvas_rect;
        let z = ((r.width() - 40.0) / self.doc.width as f32)
            .min((r.height() - 40.0) / self.doc.height as f32);
        self.zoom = z.clamp(0.05, 8.0);
        self.pan = Vec2::ZERO;
    }

    fn zoom_at(&mut self, screen: Pos2, factor: f32) {
        let d = self.to_doc(screen);
        self.zoom = (self.zoom * factor).clamp(0.05, 32.0);
        let size = Vec2::new(self.doc.width as f32, self.doc.height as f32) * self.zoom;
        let o_new = screen - Vec2::new(d.x, d.y) * self.zoom;
        self.pan = (o_new - self.canvas_rect.center()) + size * 0.5;
    }

    // ---------------------------------------------------------------- rendering

    fn upload(&mut self, ctx: &egui::Context, pm: &Pixmap) {
        let img = egui::ColorImage::from_rgba_premultiplied(
            [pm.width() as usize, pm.height() as usize],
            pm.data(),
        );
        match &mut self.texture {
            Some(t) => t.set(img, egui::TextureOptions::LINEAR),
            None => self.texture = Some(ctx.load_texture("canvas", img, egui::TextureOptions::LINEAR)),
        }
    }

    fn refresh(&mut self, ctx: &egui::Context) {
        if self.dirty {
            let pm = self.renderer.render(&self.doc);
            self.gen += 1;
            self.upload(ctx, &pm);
            self.raw = Some(pm);
            self.dirty = false;
            self.npr_pending = self.npr_preview && self.doc.npr.style != NprStyle::None;
            if !self.npr_pending {
                self.npr_busy = false;
            }
        }
        let pointer_down = ctx.input(|i| i.pointer.any_down());
        if self.npr_pending && !pointer_down {
            if let Some(raw) = &self.raw {
                self.worker.submit(NprJob {
                    gen: self.gen,
                    img: raw.clone(),
                    settings: self.doc.npr.clone(),
                });
                self.npr_busy = true;
            }
            self.npr_pending = false;
        }
    }

    fn poll_npr(&mut self, ctx: &egui::Context) {
        while let Ok(res) = self.worker.rx.try_recv() {
            if res.gen == self.gen && self.npr_preview && self.doc.npr.style != NprStyle::None {
                self.upload(ctx, &res.img);
                self.npr_busy = false;
            }
        }
    }

    // ---------------------------------------------------------------- picking

    fn hit_test(&self, p: Pt) -> Option<u64> {
        self.doc
            .objects
            .iter()
            .rev()
            .filter(|o| o.visible)
            .find(|o| {
                let (lo, hi) = o.bounds();
                p.x >= lo.x && p.y >= lo.y && p.x <= hi.x && p.y <= hi.y && self.renderer.hit(o, p)
            })
            .map(|o| o.id)
    }

    fn pick_color(&self, p: Pt) -> Option<Rgba> {
        let pm = self.raw.as_ref()?;
        if p.x < 0.0 || p.y < 0.0 {
            return None;
        }
        let c = pm.pixel(p.x as u32, p.y as u32)?.demultiply();
        Some([c.red(), c.green(), c.blue(), 255])
    }

    fn rotate_handle(&self, obj: &Object) -> Pos2 {
        let top = obj.quad[0].lerp(obj.quad[1], 0.5);
        let n = (top - obj.center()).normalized();
        let s = self.to_screen(top);
        s + Vec2::new(n.x, n.y) * 28.0
    }

    // ---------------------------------------------------------------- tools

    /// "Edit this object's shape": text goes to its text field, paths go to the node tool,
    /// other shapes are turned into an editable path first (one undo step).
    fn edit_shape(&mut self, id: u64) {
        let Some(kind) = self.doc.find(id).map(|o| o.kind.clone()) else { return };
        self.select_only(Some(id));
        match kind {
            ShapeKind::Text { .. } => {
                self.focus_text = true;
                self.status = "Type the new text in the properties panel; the font is right below it.".into();
            }
            ShapeKind::Bitmap => {
                self.status = "Paint layers have no outline — reshape them with the Brush (eraser) instead.".into();
            }
            ShapeKind::Path { .. } => {
                self.set_tool(Tool::Node);
                self.status = "Drag nodes and handles. Click a segment to add a node.".into();
            }
            _ => {
                self.checkpoint();
                if let Some(o) = self.doc.find_mut(id) {
                    vector::convert_to_path(o);
                }
                self.dirty = true;
                self.set_tool(Tool::Node);
                self.status = format!("{} turned into an editable path — drag the nodes.", kind.label());
            }
        }
    }

    /// Font from the tool panel: applies to the selected text object as well as to new text.
    fn set_font(&mut self, f: FontChoice) {
        self.text_font = f;
        let Some(id) = self.selected else { return };
        if !matches!(self.doc.find(id).map(|o| &o.kind), Some(ShapeKind::Text { .. })) {
            return;
        }
        self.checkpoint();
        if let Some(o) = self.doc.find_mut(id) {
            if let ShapeKind::Text { font, .. } = &mut o.kind {
                *font = f;
            }
            o.fit_text_width();
            o.touch();
        }
        self.dirty = true;
    }

    fn set_tool(&mut self, t: Tool) {
        if self.tool == Tool::Pen && t != Tool::Pen && self.pen.is_some() {
            self.finish_pen(false);
        }
        self.tool = t;
    }

    fn new_paint_layer(&mut self) -> u64 {
        let id = self.doc.alloc_id();
        let (w, h) = (self.doc.width as f32, self.doc.height as f32);
        let mut obj = Object::new(id, ShapeKind::Bitmap, 0.0, 0.0, w, h);
        obj.fill.kind = FillKind::None;
        obj.paint = Pixmap::new(w as u32, h as u32).map(Arc::new);
        self.doc.objects.push(obj);
        self.selected = Some(id);
        id
    }

    fn brush_radius_base(&self) -> f32 {
        let p = if self.brush.pressure { self.pressure.unwrap_or(1.0) } else { 1.0 };
        (self.brush.size * 0.5 * (0.15 + 0.85 * p)).max(0.5)
    }

    fn on_press(&mut self, p: Pt, sp: Pos2, mods: Modifiers) {
        self.last_drag_pos = Some(p);
        self.snapshot = Some(self.doc.clone());
        match self.tool {
            Tool::Select | Tool::Perspective => self.press_select(p, sp, mods),
            Tool::Node => self.press_node(p, sp),
            Tool::Pen => self.press_pen(p, sp),
            Tool::Rect | Tool::Ellipse | Tool::Polygon | Tool::Text => {
                self.drag = Drag::Create { start: p, cur: p }
            }
            Tool::Freehand => self.drag = Drag::Freehand { pts: vec![p] },
            Tool::Brush => self.press_brush(p, mods),
        }
    }

    fn press_pen(&mut self, p: Pt, sp: Pos2) {
        let ends = self.pen.as_ref().and_then(|d| {
            let n = d.nodes.len();
            (n >= 2).then(|| {
                let s = |q: [f32; 2]| self.to_screen(Pt::new(q[0], q[1]));
                (s(d.nodes[0].p), s(d.nodes[n - 1].p))
            })
        });
        if let Some((first, last)) = ends {
            if first.distance(sp) < 9.0 {
                if let Some(d) = &mut self.pen {
                    d.closing = true;
                }
                self.drag = Drag::PenHandle;
                return;
            }
            if last.distance(sp) < 6.0 {
                self.finish_pen(false);
                return;
            }
        }
        if let Some(draft) = &mut self.pen {
            draft.nodes.push(PathNode::corner([p.x, p.y]));
        } else {
            self.pen = Some(PenDraft {
                nodes: vec![PathNode::corner([p.x, p.y])],
                closing: false,
            });
        }
        self.drag = Drag::PenHandle;
    }

    fn press_node(&mut self, p: Pt, sp: Pos2) {
        if let Some(d) = self.pick_node(sp) {
            match d {
                Drag::Node { id, sp: s, ni, .. } => {
                    self.node_sel = Some((id, s, ni));
                    let offset = self
                        .doc
                        .find(id)
                        .and_then(|o| match &o.kind {
                            ShapeKind::Path { subpaths } => {
                                let q = subpaths[s].nodes[ni].p;
                                o.homography().apply_pt(Pt::new(q[0] * o.size[0], q[1] * o.size[1]))
                            }
                            _ => None,
                        })
                        .map(|np| np - p)
                        .unwrap_or_default();
                    self.drag = Drag::Node { id, sp: s, ni, offset };
                }
                other => self.drag = other,
            }
            return;
        }
        if let (Some(id), Some((s, i, t))) = (self.selected, self.pick_segment(sp)) {
            self.commit_snapshot();
            if let Some(o) = self.doc.find_mut(id) {
                if let ShapeKind::Path { subpaths } = &mut o.kind {
                    let ni = vector::split_segment(&mut subpaths[s], i, t);
                    self.node_sel = Some((id, s, ni));
                    self.drag = Drag::Node { id, sp: s, ni, offset: Pt::default() };
                }
                o.touch();
            }
            self.dirty = true;
            self.status = "Node added".into();
            return;
        }
        let hit = self.hit_test(p);
        self.select_only(hit);
        self.drag = Drag::None;
        let Some(id) = hit else { return };
        match self.doc.find(id).map(|o| &o.kind) {
            Some(ShapeKind::Text { .. }) => {
                self.status = "Text keeps its letters editable — use Convert to path in the properties panel to reshape the outlines.".into();
            }
            Some(ShapeKind::Bitmap) => self.status = "Paint layers have no nodes.".into(),
            Some(ShapeKind::Path { .. }) => {
                self.status = "Drag a node or handle; click a segment to add a node.".into()
            }
            // rectangles, ellipses, stars, freehand: make them editable on the spot
            Some(_) => self.edit_shape(id),
            None => {}
        }
    }

    fn press_select(&mut self, p: Pt, sp: Pos2, mods: Modifiers) {
        if let (Some(id), true) = (self.selected, self.multi.is_empty()) {
            if let Some(obj) = self.doc.find(id) {
                if self.tool == Tool::Perspective {
                    for i in 0..4 {
                        if self.to_screen(obj.quad[i]).distance(sp) < 11.0 {
                            self.drag = Drag::Corner { id, idx: i };
                            return;
                        }
                    }
                } else {
                    if self.rotate_handle(obj).distance(sp) < 10.0 {
                        let c = obj.center();
                        self.drag = Drag::Rotate { id, center: c, a0: (p - c).angle(), orig: obj.quad };
                        return;
                    }
                    for h in 0..8 {
                        let hp = handle_pos(&obj.quad, h);
                        if self.to_screen(hp).distance(sp) < 9.0 {
                            self.drag = Drag::Scale {
                                id,
                                handle: h,
                                anchor: handle_pos(&obj.quad, (h + 4) % 8),
                                h0: hp,
                                orig: obj.quad,
                                angle: obj.rotation(),
                            };
                            return;
                        }
                    }
                }
            }
        }
        match self.hit_test(p) {
            Some(id) => {
                if mods.shift {
                    self.toggle_in_selection(id);
                } else if !self.is_selected(id) {
                    self.select_only(Some(id));
                } else {
                    // clicking a member of the selection makes it the primary one
                    if self.selected != Some(id) {
                        if let Some(old) = self.selected {
                            self.multi.push(old);
                        }
                        self.multi.retain(|m| *m != id);
                        self.selected = Some(id);
                    }
                }
                let items = self
                    .selection()
                    .into_iter()
                    .filter_map(|i| self.doc.find(i).map(|o| (i, o.quad)))
                    .collect();
                self.drag = Drag::Move { start: p, items };
            }
            None => {
                if !mods.shift {
                    self.select_only(None);
                }
                self.drag = Drag::Marquee {
                    start: p,
                    cur: p,
                    additive: mods.shift,
                };
            }
        }
    }

    fn press_brush(&mut self, p: Pt, mods: Modifiers) {
        if mods.alt {
            if let Some(c) = self.pick_color(p) {
                self.color_a = c;
                self.status = format!("Picked colour #{:02X}{:02X}{:02X}", c[0], c[1], c[2]);
            }
            self.drag = Drag::None;
            return;
        }
        let target = self
            .selected
            .filter(|id| self.doc.find(*id).map_or(false, |o| o.visible));
        let id = match target {
            Some(id) => id,
            None => self.new_paint_layer(),
        };
        self.commit_snapshot();
        let base_r = self.brush_radius_base();
        let mut settings = self.brush.clone();
        settings.color = self.color_a;
        let Some(obj) = self.doc.find_mut(id) else { return };
        if obj.paint.is_none() {
            let (w, h) = obj.raster_size();
            obj.paint = Pixmap::new(w, h).map(Arc::new);
        }
        let Some((pc, scale)) = doc_to_paint(obj, p) else { return };
        let mut state = StrokeState::new();
        if let Some(layer) = obj.paint.as_mut() {
            brush::segment(Arc::make_mut(layer), pc, pc, base_r * scale, &settings, &mut state);
        }
        obj.touch();
        self.dirty = true;
        self.drag = Drag::Paint { id, last: pc, state };
    }

    fn on_drag(&mut self, p: Pt, mods: Modifiers) {
        if self.last_drag_pos == Some(p) {
            return;
        }
        self.last_drag_pos = Some(p);
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        self.drag = match drag {
            Drag::Move { start, items } => {
                self.commit_snapshot();
                let mut d = p - start;
                if mods.shift {
                    if d.x.abs() > d.y.abs() {
                        d.y = 0.0
                    } else {
                        d.x = 0.0
                    }
                }
                for (id, orig) in &items {
                    if let Some(o) = self.doc.find_mut(*id) {
                        o.quad = orig.map(|c| c + d);
                    }
                }
                self.dirty = true;
                Drag::Move { start, items }
            }
            Drag::Marquee { start, additive, .. } => Drag::Marquee { start, cur: p, additive },
            Drag::Node { id, sp, ni, offset } => {
                self.commit_snapshot();
                if let Some(o) = self.doc.find_mut(id) {
                    if let Some(q) = Self::doc_to_norm(o, p + offset) {
                        if let ShapeKind::Path { subpaths } = &mut o.kind {
                            if let Some(nd) = subpaths.get_mut(sp).and_then(|s| s.nodes.get_mut(ni)) {
                                let d = [q[0] - nd.p[0], q[1] - nd.p[1]];
                                for v in [&mut nd.p, &mut nd.h_in, &mut nd.h_out] {
                                    v[0] += d[0];
                                    v[1] += d[1];
                                }
                            }
                        }
                        vector::normalize_bounds(o);
                        o.touch();
                    }
                }
                self.dirty = true;
                Drag::Node { id, sp, ni, offset }
            }
            Drag::Handle { id, sp, ni, out } => {
                self.commit_snapshot();
                if let Some(o) = self.doc.find_mut(id) {
                    let (w, h) = (o.size[0], o.size[1]);
                    if let Some(q) = Self::doc_to_norm(o, p) {
                        if let ShapeKind::Path { subpaths } = &mut o.kind {
                            if let Some(nd) = subpaths.get_mut(sp).and_then(|s| s.nodes.get_mut(ni)) {
                                if mods.alt {
                                    nd.smooth = false;
                                }
                                let (moved, other) = if out { (&mut nd.h_out, &mut nd.h_in) } else { (&mut nd.h_in, &mut nd.h_out) };
                                *moved = q;
                                if nd.smooth {
                                    // keep the opposite handle aligned, with its own length (in pixels)
                                    let c = Pt::new(nd.p[0] * w, nd.p[1] * h);
                                    let m = Pt::new(q[0] * w, q[1] * h);
                                    let len = Pt::new(other[0] * w, other[1] * h).dist(c);
                                    let dir = (c - m).normalized();
                                    let o2 = c + dir * len;
                                    *other = [o2.x / w, o2.y / h];
                                }
                            }
                        }
                        vector::normalize_bounds(o);
                        o.touch();
                    }
                }
                self.dirty = true;
                Drag::Handle { id, sp, ni, out }
            }
            Drag::PenHandle => {
                let zoom = self.zoom;
                if let Some(draft) = &mut self.pen {
                    let idx = if draft.closing { 0 } else { draft.nodes.len() - 1 };
                    let nd = &mut draft.nodes[idx];
                    let c = Pt::new(nd.p[0], nd.p[1]);
                    if c.dist(p) * zoom > 3.0 {
                        let mirror = c - (p - c);
                        if draft.closing {
                            // dragging on the first node shapes the closing curve
                            nd.h_in = [mirror.x, mirror.y];
                            nd.h_out = [p.x, p.y];
                        } else {
                            nd.h_out = [p.x, p.y];
                            nd.h_in = [mirror.x, mirror.y];
                        }
                        nd.smooth = true;
                    }
                }
                Drag::PenHandle
            }
            Drag::Scale { id, handle, anchor, h0, orig, angle } => {
                self.commit_snapshot();
                let ra = |v: Pt| v.rotate(-angle);
                let h0l = ra(h0 - anchor);
                let ml = ra(p - anchor);
                let affects_x = handle != 1 && handle != 5;
                let affects_y = handle != 3 && handle != 7;
                let mut sx = if affects_x && h0l.x.abs() > 1e-3 { ml.x / h0l.x } else { 1.0 };
                let mut sy = if affects_y && h0l.y.abs() > 1e-3 { ml.y / h0l.y } else { 1.0 };
                if mods.shift && affects_x && affects_y {
                    let s = if sx.abs() > sy.abs() { sx } else { sy };
                    sx = s;
                    sy = s;
                }
                let fix = |s: f32| if s.abs() < 0.01 { 0.01f32.copysign(s) } else { s };
                let (sx, sy) = (fix(sx), fix(sy));
                if let Some(o) = self.doc.find_mut(id) {
                    o.quad = orig.map(|c| {
                        let l = ra(c - anchor);
                        anchor + Pt::new(l.x * sx, l.y * sy).rotate(angle)
                    });
                    o.update_size_from_quad();
                }
                self.dirty = true;
                Drag::Scale { id, handle, anchor, h0, orig, angle }
            }
            Drag::Rotate { id, center, a0, orig } => {
                self.commit_snapshot();
                let mut da = (p - center).angle() - a0;
                if mods.shift {
                    let step = 15f32.to_radians();
                    da = (da / step).round() * step;
                }
                if let Some(o) = self.doc.find_mut(id) {
                    o.quad = orig.map(|c| center + (c - center).rotate(da));
                }
                self.dirty = true;
                Drag::Rotate { id, center, a0, orig }
            }
            Drag::Corner { id, idx } => {
                self.commit_snapshot();
                if let Some(o) = self.doc.find_mut(id) {
                    o.quad[idx] = p;
                }
                self.dirty = true;
                Drag::Corner { id, idx }
            }
            Drag::Create { start, .. } => Drag::Create { start, cur: p },
            Drag::Freehand { mut pts } => {
                if pts.last().map_or(true, |l| l.dist(p) * self.zoom >= 2.0) {
                    pts.push(p);
                }
                Drag::Freehand { pts }
            }
            Drag::Paint { id, last, mut state } => {
                let base_r = self.brush_radius_base();
                let mut settings = self.brush.clone();
                settings.color = self.color_a;
                let mut new_last = last;
                if let Some(obj) = self.doc.find_mut(id) {
                    if let Some((pc, scale)) = doc_to_paint(obj, p) {
                        if let Some(layer) = obj.paint.as_mut() {
                            brush::segment(Arc::make_mut(layer), last, pc, base_r * scale, &settings, &mut state);
                        }
                        obj.touch();
                        new_last = pc;
                    }
                }
                self.dirty = true;
                Drag::Paint { id, last: new_last, state }
            }
            Drag::None => Drag::None,
        };
    }

    fn on_release(&mut self, mods: Modifiers) {
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        match drag {
            Drag::Create { start, cur } => self.finish_create(start, cur, mods),
            Drag::Freehand { pts } => self.finish_freehand(pts),
            Drag::PenHandle => {
                if self.pen.as_ref().is_some_and(|d| d.closing) {
                    self.finish_pen(true);
                }
            }
            Drag::Marquee { start, cur, additive } => {
                if start.dist(cur) * self.zoom > 3.0 {
                    let (lo, hi) = (
                        Pt::new(start.x.min(cur.x), start.y.min(cur.y)),
                        Pt::new(start.x.max(cur.x), start.y.max(cur.y)),
                    );
                    if !additive {
                        self.select_only(None);
                    }
                    let hits: Vec<u64> = self
                        .doc
                        .objects
                        .iter()
                        .filter(|o| o.visible)
                        .filter(|o| {
                            let (a, b) = o.bounds();
                            a.x <= hi.x && b.x >= lo.x && a.y <= hi.y && b.y >= lo.y
                        })
                        .map(|o| o.id)
                        .collect();
                    for id in hits {
                        if !self.is_selected(id) {
                            self.toggle_in_selection(id);
                        }
                    }
                    let n = self.selection().len();
                    self.status = format!("{n} object(s) selected");
                }
            }
            _ => {}
        }
        self.snapshot = None;
        self.last_drag_pos = None;
    }

    fn finish_create(&mut self, start: Pt, cur: Pt, mods: Modifiers) {
        let mut w = (cur.x - start.x).abs();
        let mut h = (cur.y - start.y).abs();
        let mut x = start.x.min(cur.x);
        let mut y = start.y.min(cur.y);
        let clicked = w < 4.0 && h < 4.0;
        if mods.shift && !clicked {
            let s = w.max(h);
            w = s;
            h = s;
            x = if cur.x < start.x { start.x - s } else { start.x };
            y = if cur.y < start.y { start.y - s } else { start.y };
        }
        let kind = match self.tool {
            Tool::Rect => ShapeKind::Rect { corner: 0.0 },
            Tool::Ellipse => ShapeKind::Ellipse,
            Tool::Polygon => ShapeKind::Polygon {
                sides: self.poly_sides,
                inner: self.poly_inner,
            },
            Tool::Text => ShapeKind::Text {
                text: "Jabro".into(),
                font: self.text_font,
            },
            _ => return,
        };
        if clicked {
            let (dw, dh) = match self.tool {
                Tool::Text => {
                    let asp = shapes::text_aspect("Jabro", self.text_font).unwrap_or(3.0);
                    (100.0 * asp, 100.0)
                }
                Tool::Polygon => (200.0, 200.0),
                _ => (220.0, 160.0),
            };
            x = start.x - dw / 2.0;
            y = start.y - dh / 2.0;
            w = dw;
            h = dh;
        }
        self.commit_snapshot();
        let id = self.doc.alloc_id();
        let mut obj = Object::new(id, kind, x, y, w, h);
        obj.fill = FillStyle {
            kind: FillKind::Solid,
            a: self.color_a,
            b: self.color_b,
            ..Default::default()
        };
        self.doc.objects.push(obj);
        self.selected = Some(id);
        self.dirty = true;
        self.status = "Created — try a style preset in the properties panel.".into();
    }

    fn finish_freehand(&mut self, pts: Vec<Pt>) {
        if pts.len() < 3 {
            return;
        }
        let pts = simplify(&pts, 1.2);
        if pts.len() < 3 {
            return;
        }
        let (mut lo, mut hi) = (pts[0], pts[0]);
        for p in &pts {
            lo = Pt::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = Pt::new(hi.x.max(p.x), hi.y.max(p.y));
        }
        let (w, h) = (hi.x - lo.x, hi.y - lo.y);
        if w < 3.0 || h < 3.0 {
            return;
        }
        let norm: Vec<[f32; 2]> = pts.iter().map(|p| [(p.x - lo.x) / w, (p.y - lo.y) / h]).collect();
        self.commit_snapshot();
        let id = self.doc.alloc_id();
        let mut obj = Object::new(id, ShapeKind::Freehand { points: norm }, lo.x, lo.y, w, h);
        // freehand shapes become editable Bézier paths right away
        vector::convert_to_path(&mut obj);
        vector::normalize_bounds(&mut obj);
        obj.fill = FillStyle {
            kind: FillKind::Solid,
            a: self.color_a,
            b: self.color_b,
            ..Default::default()
        };
        self.doc.objects.push(obj);
        self.selected = Some(id);
        self.dirty = true;
    }

    // ---------------------------------------------------------------- object ops

    fn delete_selected(&mut self) {
        let ids = self.selection();
        if ids.is_empty() {
            return;
        }
        self.checkpoint();
        self.doc.objects.retain(|o| !ids.contains(&o.id));
        self.select_only(None);
        self.dirty = true;
    }

    fn duplicate_selected(&mut self) {
        let ids = self.selection();
        if ids.is_empty() {
            return;
        }
        self.checkpoint();
        let primary = self.selected;
        let mut new_primary = None;
        let mut new_ids = Vec::new();
        for old in ids {
            let Some(i) = self.doc.index_of(old) else { continue };
            let id = self.doc.alloc_id();
            let mut o = self.doc.objects[i].clone();
            o.id = id;
            o.name = format!("{} copy", o.name);
            o.quad = o.quad.map(|p| p + Pt::new(20.0, 20.0));
            o.touch();
            self.doc.objects.insert(i + 1, o);
            if Some(old) == primary {
                new_primary = Some(id);
            } else {
                new_ids.push(id);
            }
        }
        self.select_only(new_primary);
        self.multi = new_ids;
        self.dirty = true;
    }

    fn move_selected(&mut self, delta: isize) {
        if let Some(i) = self.selected.and_then(|id| self.doc.index_of(id)) {
            let j = (i as isize + delta).clamp(0, self.doc.objects.len() as isize - 1) as usize;
            if i != j {
                self.checkpoint();
                let o = self.doc.objects.remove(i);
                self.doc.objects.insert(j, o);
                self.dirty = true;
            }
        }
    }

    fn apply_to_selected(&mut self, f: impl FnOnce(&mut Object)) {
        if let Some(id) = self.selected {
            if self.doc.find(id).is_some() {
                self.checkpoint();
                if let Some(o) = self.doc.find_mut(id) {
                    f(o);
                    o.touch();
                }
                self.dirty = true;
            }
        }
    }

    // ---------------------------------------------------------------- input

    fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::Key;
        let cmd = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, k));
        if cmd(Key::S) {
            self.save(false);
        }
        if cmd(Key::O) {
            self.open();
        }
        if cmd(Key::E) {
            self.export_png();
        }
        if ctx.wants_keyboard_input() {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)) || cmd(Key::Y) {
            self.redo();
        }
        if cmd(Key::Z) {
            self.undo();
        }
        if cmd(Key::D) {
            self.duplicate_selected();
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::C)) {
            self.convert_selected_to_path();
        }
        let (del, backspace, esc, enter, zero, mods) = ctx.input(|i| {
            (
                i.key_pressed(Key::Delete),
                i.key_pressed(Key::Backspace),
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Num0),
                i.modifiers,
            )
        });
        if self.pen.is_some() {
            if enter {
                self.finish_pen(false);
            }
            if esc {
                self.pen = None;
                self.status = "Path cancelled".into();
            }
            if backspace || del {
                if let Some(d) = &mut self.pen {
                    d.nodes.pop();
                    if d.nodes.is_empty() {
                        self.pen = None;
                    }
                }
            }
        } else {
            if del || backspace {
                if self.tool == Tool::Node && self.node_sel.is_some() {
                    self.delete_node();
                } else {
                    self.delete_selected();
                }
            }
            if esc {
                if self.node_sel.is_some() {
                    self.node_sel = None;
                } else {
                    self.select_only(None);
                }
            }
        }
        if zero {
            self.fit();
        }
        if mods.is_none() {
            for t in Tool::ALL {
                if ctx.input(|i| i.key_pressed(t.key())) {
                    self.set_tool(t);
                }
            }
        }
    }

    fn read_pressure(&mut self, ctx: &egui::Context) {
        let f = ctx.input(|i| {
            i.events.iter().rev().find_map(|e| match e {
                egui::Event::Touch { force, .. } => *force,
                _ => None,
            })
        });
        if let Some(f) = f {
            self.pressure = Some(f.clamp(0.02, 1.0));
        }
    }

    // ---------------------------------------------------------------- panels

    fn menu_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::menu::bar(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New").clicked() {
                    self.new_doc();
                    ui.close_menu();
                }
                if ui.button("Open…  (Ctrl+O)").clicked() {
                    ui.close_menu();
                    self.open();
                }
                if ui.button("Save  (Ctrl+S)").clicked() {
                    ui.close_menu();
                    self.save(false);
                }
                if ui.button("Save as…").clicked() {
                    ui.close_menu();
                    self.save(true);
                }
                ui.separator();
                if ui.button("Export PNG…  (Ctrl+E)").clicked() {
                    ui.close_menu();
                    self.export_png();
                }
                ui.separator();
                if ui.button("Load demo scene").clicked() {
                    self.replace_doc(demo::demo_document());
                    ui.close_menu();
                }
                if ui.button("Quit").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("Undo  (Ctrl+Z)")).clicked() {
                    self.undo();
                    ui.close_menu();
                }
                if ui.add_enabled(!self.redo.is_empty(), egui::Button::new("Redo  (Ctrl+Y)")).clicked() {
                    self.redo();
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Duplicate  (Ctrl+D)").clicked() {
                    self.duplicate_selected();
                    ui.close_menu();
                }
                if ui.button("Delete  (Del)").clicked() {
                    self.delete_selected();
                    ui.close_menu();
                }
            });
            ui.menu_button("Object", |ui| {
                if ui.button("Bring to front").clicked() {
                    self.move_selected(isize::MAX / 2);
                    ui.close_menu();
                }
                if ui.button("Bring forward").clicked() {
                    self.move_selected(1);
                    ui.close_menu();
                }
                if ui.button("Send backward").clicked() {
                    self.move_selected(-1);
                    ui.close_menu();
                }
                if ui.button("Send to back").clicked() {
                    self.move_selected(-(isize::MAX / 2));
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Reset perspective").clicked() {
                    self.apply_to_selected(|o| o.reset_perspective());
                    ui.close_menu();
                }
                if ui.button("New paint layer").clicked() {
                    self.checkpoint();
                    self.new_paint_layer();
                    self.dirty = true;
                    ui.close_menu();
                }
            });
            ui.menu_button("Path", |ui| {
                let n = self.selection().len();
                for op in BoolOp::ALL {
                    if ui
                        .add_enabled(n >= 2, egui::Button::new(op.label()))
                        .on_hover_text(op.hint())
                        .clicked()
                    {
                        self.boolean(op);
                        ui.close_menu();
                    }
                }
                ui.separator();
                if ui.add_enabled(n >= 1, egui::Button::new("Convert to path  (Ctrl+Shift+C)")).clicked() {
                    self.convert_selected_to_path();
                    ui.close_menu();
                }
            });
            ui.menu_button("View", |ui| {
                if ui.button("Fit to window  (0)").clicked() {
                    self.fit();
                    ui.close_menu();
                }
                if ui.button("Actual pixels (100%)").clicked() {
                    self.zoom = 1.0;
                    self.pan = Vec2::ZERO;
                    ui.close_menu();
                }
                ui.checkbox(&mut self.npr_preview, "Show NPR look");
            });
            ui.menu_button("Help", |ui| {
                if ui.button("About Jabro Draw").clicked() {
                    self.show_about = true;
                    ui.close_menu();
                }
            });
        });
    }

    fn look_bar(&mut self, ui: &mut egui::Ui) {
        let before = self.doc.npr.clone();
        let preview_before = self.npr_preview;
        ui.horizontal(|ui| {
            ui.strong("NPR look");
            egui::ComboBox::from_id_salt("npr_style")
                .selected_text(self.doc.npr.style.label())
                .show_ui(ui, |ui| {
                    for s in NprStyle::ALL {
                        ui.selectable_value(&mut self.doc.npr.style, s, s.label());
                    }
                });
            ui.add(egui::Slider::new(&mut self.doc.npr.strength, 0.0..=1.0).text("Strength"));
            ui.add(egui::Slider::new(&mut self.doc.npr.brush, 0.0..=1.0).text("Brush size"));
            ui.add(egui::Slider::new(&mut self.doc.npr.paper, 0.0..=1.0).text("Paper"));
            if ui.button("Vary").on_hover_text("New random seed for the strokes").clicked() {
                self.doc.npr.seed = self.doc.npr.seed.wrapping_add(1);
            }
            ui.checkbox(&mut self.npr_preview, "Preview");
            if self.npr_busy {
                ui.spinner();
                ui.label("painting…");
            }
        });
        if before != self.doc.npr || preview_before != self.npr_preview {
            self.dirty = true;
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(self.tool.hint());
            ui.separator();
            ui.label(format!("Zoom {:.0}%", self.zoom * 100.0));
            if let Some(p) = self.cursor_doc {
                ui.label(format!("x {:.0}  y {:.0}", p.x, p.y));
            }
            ui.separator();
            ui.label(self.status.as_str());
        });
    }

    fn tool_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.strong("Tools");
        for t in Tool::ALL {
            let r = ui
                .add_sized([ui.available_width(), 24.0], egui::SelectableLabel::new(self.tool == t, t.label()))
                .on_hover_text(t.hint());
            if r.clicked() {
                self.set_tool(t);
            }
        }
        ui.separator();
        ui.strong("Colours");
        ui.horizontal(|ui| {
            ui.color_edit_button_srgba_unmultiplied(&mut self.color_a);
            ui.color_edit_button_srgba_unmultiplied(&mut self.color_b);
            if ui.small_button("⇄").on_hover_text("Swap").clicked() {
                std::mem::swap(&mut self.color_a, &mut self.color_b);
            }
        });
        ui.separator();
        match self.tool {
            Tool::Pen => {
                ui.strong("Pen");
                ui.add(egui::Slider::new(&mut self.stroke_width, 0.5..=40.0).text("Line width"));
                ui.small("Closed paths are filled with the first colour; open paths become strokes of this width.");
                if self.pen.is_some() {
                    ui.horizontal(|ui| {
                        if ui.button("Finish open").clicked() {
                            self.finish_pen(false);
                        }
                        if ui.button("Close path").clicked() {
                            self.finish_pen(true);
                        }
                    });
                }
            }
            Tool::Node => {
                ui.strong("Edit nodes");
                let sel = self.node_sel;
                if let Some((id, s, n)) = sel {
                    let mut toggle = false;
                    ui.horizontal(|ui| {
                        if ui.button("Smooth / corner").clicked() {
                            toggle = true;
                        }
                        if ui.button("Delete node").clicked() {
                            self.delete_node();
                        }
                    });
                    if toggle {
                        self.checkpoint();
                        if let Some(o) = self.doc.find_mut(id) {
                            let (w, h) = (o.size[0], o.size[1]);
                            if let ShapeKind::Path { subpaths } = &mut o.kind {
                                if let Some(sub) = subpaths.get_mut(s) {
                                    if n < sub.nodes.len() {
                                        vector::toggle_smooth(sub, n, w, h);
                                    }
                                }
                            }
                            vector::normalize_bounds(o);
                            o.touch();
                        }
                        self.dirty = true;
                    }
                }
                ui.small("Shapes and text can be turned into paths with Path › Convert to path.");
            }
            Tool::Polygon => {
                ui.strong("Star / polygon");
                ui.add(egui::Slider::new(&mut self.poly_sides, 3..=16).text("Sides"));
                ui.add(egui::Slider::new(&mut self.poly_inner, 0.15..=1.0).text("Star depth"));
            }
            Tool::Text => {
                ui.strong("Text");
                let selected_font = self
                    .selected
                    .and_then(|id| self.doc.find(id))
                    .and_then(|o| match &o.kind {
                        ShapeKind::Text { font, .. } => Some(*font),
                        _ => None,
                    });
                let current = selected_font.unwrap_or(self.text_font);
                let mut f = current;
                ui.horizontal(|ui| {
                    ui.label("Font");
                    egui::ComboBox::from_id_salt("tool_font")
                        .selected_text(f.label())
                        .show_ui(ui, |ui| {
                            for c in FontChoice::ALL {
                                ui.selectable_value(&mut f, c, c.label());
                            }
                        });
                });
                if f != current {
                    self.set_font(f);
                }
                ui.small(if selected_font.is_some() {
                    "Changes the selected text (and the next text you create)."
                } else {
                    "Font for new text. Select a text object to change its font."
                });
            }
            Tool::Brush => {
                ui.strong("Brush");
                for k in BrushKind::ALL {
                    ui.radio_value(&mut self.brush.kind, k, k.label());
                }
                ui.add(egui::Slider::new(&mut self.brush.size, 1.0..=300.0).logarithmic(true).text("Size"));
                ui.add(egui::Slider::new(&mut self.brush.hardness, 0.0..=1.0).text("Hardness"));
                ui.add(egui::Slider::new(&mut self.brush.flow, 0.02..=1.0).text("Flow"));
                ui.add(egui::Slider::new(&mut self.brush.spacing, 0.02..=1.0).text("Spacing"));
                ui.checkbox(&mut self.brush.pressure, "Pen pressure");
                if ui.button("New paint layer").clicked() {
                    self.checkpoint();
                    self.new_paint_layer();
                    self.dirty = true;
                }
                ui.small("Paints into the selected object (clipped to its shape). With nothing selected a new paint layer is created. Alt+click = colour picker.");
            }
            _ => {
                ui.small("Tip: select an object and pick a style preset on the right, then paint on it with the Brush and bend it with the Perspective tool.");
            }
        }
    }

    fn objects_panel(&mut self, ui: &mut egui::Ui) {
        ui.strong("Objects");
        enum A {
            Vis(u64),
            Sel(u64, bool),
        }
        let mut action = None;
        egui::ScrollArea::vertical()
            .id_salt("objlist")
            .max_height(190.0)
            .show(ui, |ui| {
                for obj in self.doc.objects.iter().rev() {
                    ui.horizontal(|ui| {
                        let mut vis = obj.visible;
                        if ui.checkbox(&mut vis, "").on_hover_text("Visible").changed() {
                            action = Some(A::Vis(obj.id));
                        }
                        let sel = self.selected == Some(obj.id) || self.multi.contains(&obj.id);
                        let r = ui
                            .selectable_label(sel, obj.name.as_str())
                            .on_hover_text("Shift+click to add to the selection");
                        if r.clicked() {
                            action = Some(A::Sel(obj.id, ui.input(|i| i.modifiers.shift)));
                        }
                    });
                }
            });
        match action {
            Some(A::Vis(id)) => {
                self.checkpoint();
                if let Some(o) = self.doc.find_mut(id) {
                    o.visible = !o.visible;
                }
                self.dirty = true;
            }
            Some(A::Sel(id, true)) => self.toggle_in_selection(id),
            Some(A::Sel(id, false)) => self.select_only(Some(id)),
            None => {}
        }
        ui.horizontal(|ui| {
            let has = self.selected.is_some();
            if ui.add_enabled(has, egui::Button::new("Up")).clicked() {
                self.move_selected(1);
            }
            if ui.add_enabled(has, egui::Button::new("Down")).clicked() {
                self.move_selected(-1);
            }
            if ui.add_enabled(has, egui::Button::new("Duplicate")).clicked() {
                self.duplicate_selected();
            }
            if ui.add_enabled(has, egui::Button::new("Delete")).clicked() {
                self.delete_selected();
            }
        });
    }

    fn right_panel(&mut self, ui: &mut egui::Ui) {
        self.objects_panel(ui);
        ui.separator();
        let count = self.selection().len();
        if count >= 2 {
            ui.strong(format!("Combine {count} objects"));
            let mut op_clicked = None;
            ui.horizontal_wrapped(|ui| {
                for op in BoolOp::ALL {
                    if ui.button(op.label()).on_hover_text(op.hint()).clicked() {
                        op_clicked = Some(op);
                    }
                }
            });
            if let Some(op) = op_clicked {
                self.boolean(op);
            }
            ui.small("The result is an editable path that keeps the style of the bottom object.");
            ui.separator();
        }
        let before = self.doc.clone();
        let mut actions: Vec<PropAction> = Vec::new();
        let mut resize: Option<(u32, u32)> = None;
        let has_clip = self.style_clipboard.is_some();
        let selected = self.selected;
        let focus_text = std::mem::take(&mut self.focus_text);
        let mut edit_nodes = false;
        egui::ScrollArea::vertical().id_salt("props").show(ui, |ui| {
            match selected.and_then(|id| self.doc.find_mut(id)) {
                Some(obj) => object_props(ui, obj, &mut actions, has_clip, focus_text),
                None => {
                    doc_props(ui, &mut self.doc, &mut self.new_w, &mut self.new_h, &mut resize);
                }
            }
        });

        for a in actions {
            match a {
                PropAction::ConvertToPath => {
                    if let Some(o) = selected.and_then(|id| self.doc.find_mut(id)) {
                        vector::convert_to_path(o);
                    }
                }
                PropAction::EditNodes => edit_nodes = true,
                PropAction::ResetPerspective => {
                    if let Some(o) = selected.and_then(|id| self.doc.find_mut(id)) {
                        o.reset_perspective();
                    }
                }
                PropAction::FlipH => {
                    if let Some(o) = selected.and_then(|id| self.doc.find_mut(id)) {
                        let q = o.quad;
                        o.quad = [q[1], q[0], q[3], q[2]];
                    }
                }
                PropAction::FlipV => {
                    if let Some(o) = selected.and_then(|id| self.doc.find_mut(id)) {
                        let q = o.quad;
                        o.quad = [q[3], q[2], q[1], q[0]];
                    }
                }
                PropAction::ClearPaint => {
                    if let Some(o) = selected.and_then(|id| self.doc.find_mut(id)) {
                        o.paint = None;
                    }
                }
                PropAction::CopyStyle => {
                    if let Some(o) = selected.and_then(|id| self.doc.find(id)) {
                        self.style_clipboard = Some((o.fill.clone(), o.stroke.clone(), o.effects.clone()));
                        self.status = "Style copied".into();
                    }
                }
                PropAction::PasteStyle => {
                    if let (Some(o), Some((f, s, e))) =
                        (selected.and_then(|id| self.doc.find_mut(id)), self.style_clipboard.clone())
                    {
                        o.fill = f;
                        o.stroke = s;
                        o.effects = e;
                    }
                }
                PropAction::Front => {
                    if let Some(i) = selected.and_then(|id| self.doc.index_of(id)) {
                        let o = self.doc.objects.remove(i);
                        self.doc.objects.push(o);
                    }
                }
                PropAction::Back => {
                    if let Some(i) = selected.and_then(|id| self.doc.index_of(id)) {
                        let o = self.doc.objects.remove(i);
                        self.doc.objects.insert(0, o);
                    }
                }
            }
        }
        if let Some((w, h)) = resize {
            self.doc.width = w.clamp(16, 8192);
            self.doc.height = h.clamp(16, 8192);
            self.fit_pending = true;
        }

        if !self.doc.same_content(&before) {
            for o in &mut self.doc.objects {
                if let Some(b) = before.objects.iter().find(|b| b.id == o.id) {
                    if !b.same_content(o) {
                        o.touch();
                    }
                }
            }
            self.dirty = true;
            if !self.prop_edit_open {
                self.push_undo(before);
                self.prop_edit_open = true;
            }
        }
        let busy = ui.input(|i| i.pointer.any_down()) || ui.ctx().memory(|m| m.focused().is_some());
        if !busy {
            self.prop_edit_open = false;
        }
        // runs after change detection because it records its own undo step
        if let (true, Some(id)) = (edit_nodes, selected) {
            self.edit_shape(id);
        }
    }

    // ---------------------------------------------------------------- canvas

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        self.canvas_rect = rect;
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        let ctx = ui.ctx().clone();
        if self.fit_pending {
            self.fit();
            self.fit_pending = false;
        }
        let typing = ctx.wants_keyboard_input();
        let (scroll, zoom_delta, mods, latest, pressed, down, released, space, middle, delta) = ctx.input(|i| {
            (
                i.smooth_scroll_delta,
                i.zoom_delta(),
                i.modifiers,
                i.pointer.latest_pos(),
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.key_down(egui::Key::Space),
                i.pointer.middle_down(),
                i.pointer.delta(),
            )
        });
        let space = space && !typing;
        if resp.hovered() {
            if zoom_delta != 1.0 {
                if let Some(p) = latest {
                    self.zoom_at(p, zoom_delta);
                }
            } else if scroll.y != 0.0 {
                if let Some(p) = latest {
                    self.zoom_at(p, (scroll.y * 0.0018).exp());
                }
            }
        }
        if (middle || (space && down)) && (resp.hovered() || resp.dragged()) {
            self.pan += delta;
        }

        self.cursor_doc = latest.filter(|p| rect.contains(*p)).map(|p| self.to_doc(p));
        if pressed && resp.hovered() && !space {
            if let Some(p) = latest {
                self.pressing = true;
                let d = self.to_doc(p);
                self.on_press(d, p, mods);
            }
        }
        if self.pressing && down && !pressed {
            if let Some(p) = latest {
                let d = self.to_doc(p);
                self.on_drag(d, mods);
            }
        }
        if self.pressing && (released || !down) {
            self.on_release(mods);
            self.pressing = false;
        }
        let dbl = ctx.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary));
        if dbl && resp.hovered() && self.tool == Tool::Select {
            if let Some(id) = latest.map(|p| self.to_doc(p)).and_then(|d| self.hit_test(d)) {
                self.edit_shape(id);
            }
        } else if dbl && resp.hovered() && self.tool == Tool::Node {
            if let Some(Drag::Node { id, sp, ni, .. }) = latest.and_then(|p| self.pick_node(p)) {
                self.checkpoint();
                if let Some(o) = self.doc.find_mut(id) {
                    let (w, h) = (o.size[0], o.size[1]);
                    if let ShapeKind::Path { subpaths } = &mut o.kind {
                        vector::toggle_smooth(&mut subpaths[sp], ni, w, h);
                    }
                    vector::normalize_bounds(o);
                    o.touch();
                }
                self.node_sel = Some((id, sp, ni));
                self.dirty = true;
            }
        }

        self.refresh(&ctx);

        let painter = ui.painter_at(rect);
        let doc_rect = Rect::from_min_size(
            self.origin(),
            Vec2::new(self.doc.width as f32, self.doc.height as f32) * self.zoom,
        );
        painter.rect_filled(doc_rect.translate(Vec2::splat(5.0)), 0.0, Color32::from_black_alpha(110));
        if let Some(tex) = &self.texture {
            painter.image(
                tex.id(),
                doc_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        self.draw_overlays(&painter);

        if self.tool == Tool::Brush {
            if let Some(p) = latest.filter(|p| rect.contains(*p)) {
                let r = self.brush.size * 0.5 * self.zoom;
                painter.circle_stroke(p, r, Stroke::new(1.0_f32, Color32::from_black_alpha(160)));
                painter.circle_stroke(p, r + 1.0, Stroke::new(1.0_f32, Color32::from_white_alpha(160)));
            }
        }
    }

    fn quad_screen(&self, q: &[Pt; 4]) -> Vec<Pos2> {
        q.iter().map(|&p| self.to_screen(p)).collect()
    }

    fn draw_overlays(&self, painter: &egui::Painter) {
        let sel_col = Color32::from_rgb(0, 170, 255);
        if matches!(self.tool, Tool::Select | Tool::Perspective) && !self.pressing {
            if let Some(p) = self.cursor_doc {
                if let Some(id) = self.hit_test(p) {
                    if Some(id) != self.selected {
                        if let Some(o) = self.doc.find(id) {
                            painter.add(Shape::closed_line(
                                self.quad_screen(&o.quad),
                                Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(0, 170, 255, 120)),
                            ));
                        }
                    }
                }
            }
        }
        for id in &self.multi {
            if let Some(o) = self.doc.find(*id) {
                painter.add(Shape::closed_line(self.quad_screen(&o.quad), Stroke::new(1.5_f32, sel_col)));
            }
        }
        if self.tool == Tool::Node {
            self.draw_node_overlay(painter);
        }
        let single = self.multi.is_empty();
        if let Some(obj) = self
            .selected
            .and_then(|id| self.doc.find(id))
            .filter(|_| self.tool != Tool::Node)
        {
            let pts = self.quad_screen(&obj.quad);
            painter.add(Shape::closed_line(pts.clone(), Stroke::new(1.5_f32, sel_col)));
            match self.tool {
                Tool::Perspective if single => {
                    for p in &pts {
                        painter.circle_filled(*p, 6.5, Color32::from_rgb(255, 150, 30));
                        painter.circle_stroke(*p, 6.5, Stroke::new(1.0_f32, Color32::BLACK));
                    }
                }
                Tool::Select if single => {
                    let rh = self.rotate_handle(obj);
                    let top = self.to_screen(obj.quad[0].lerp(obj.quad[1], 0.5));
                    painter.line_segment([top, rh], Stroke::new(1.0_f32, sel_col));
                    painter.circle_filled(rh, 5.5, Color32::WHITE);
                    painter.circle_stroke(rh, 5.5, Stroke::new(1.0_f32, sel_col));
                    for h in 0..8 {
                        let c = self.to_screen(handle_pos(&obj.quad, h));
                        let r = Rect::from_center_size(c, Vec2::splat(8.0));
                        painter.rect_filled(r, 0.0, Color32::WHITE);
                        painter.add(Shape::closed_line(
                            vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()],
                            Stroke::new(1.0_f32, sel_col),
                        ));
                    }
                }
                _ => {}
            }
        }
        let preview = Stroke::new(1.5_f32, Color32::from_rgb(255, 120, 0));
        match &self.drag {
            Drag::Create { start, cur } => {
                let (a, b) = (self.to_screen(*start), self.to_screen(*cur));
                let r = Rect::from_two_pos(a, b);
                match self.tool {
                    Tool::Ellipse | Tool::Polygon => {
                        let c = r.center();
                        let pts: Vec<Pos2> = (0..64)
                            .map(|i| {
                                let t = i as f32 / 64.0 * std::f32::consts::TAU;
                                Pos2::new(c.x + t.cos() * r.width() * 0.5, c.y + t.sin() * r.height() * 0.5)
                            })
                            .collect();
                        painter.add(Shape::closed_line(pts, preview));
                    }
                    _ => {
                        painter.add(Shape::closed_line(
                            vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()],
                            preview,
                        ));
                    }
                }
            }
            Drag::Freehand { pts } => {
                let s: Vec<Pos2> = pts.iter().map(|&p| self.to_screen(p)).collect();
                painter.add(Shape::line(s, preview));
            }
            Drag::Marquee { start, cur, .. } => {
                let r = Rect::from_two_pos(self.to_screen(*start), self.to_screen(*cur));
                painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(0, 170, 255, 28));
                painter.add(Shape::closed_line(
                    vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()],
                    Stroke::new(1.0_f32, sel_col),
                ));
            }
            _ => {}
        }
        self.draw_pen_overlay(painter);
    }

    fn path_polylines_screen(&self, obj: &Object) -> Vec<Vec<Pos2>> {
        let Some(path) = shapes::build_path(&obj.kind, obj.size[0], obj.size[1]) else {
            return Vec::new();
        };
        let hm = obj.homography();
        vector::flatten(&path, 0.5 / self.zoom.max(0.05))
            .into_iter()
            .map(|c| c.into_iter().filter_map(|p| hm.apply_pt(p)).map(|p| self.to_screen(p)).collect())
            .collect()
    }

    fn draw_node_overlay(&self, painter: &egui::Painter) {
        let col = Color32::from_rgb(0, 170, 255);
        let Some(obj) = self.selected.and_then(|id| self.doc.find(id)) else { return };
        for line in self.path_polylines_screen(obj) {
            painter.add(Shape::line(line, Stroke::new(1.5_f32, col)));
        }
        let ShapeKind::Path { subpaths } = &obj.kind else {
            painter.add(Shape::closed_line(self.quad_screen(&obj.quad), Stroke::new(1.0_f32, col)));
            return;
        };
        let sel = self.node_sel.filter(|(id, _, _)| *id == obj.id);
        let handle_col = Color32::from_rgb(255, 120, 0);
        for (s, sub) in subpaths.iter().enumerate() {
            let n = sub.nodes.len();
            for (i, nd) in sub.nodes.iter().enumerate() {
                let Some(c) = self.norm_to_screen(obj, nd.p) else { continue };
                let mut handles = Vec::new();
                if let Some((_, ss, ni)) = sel {
                    if ss == s {
                        if ni == i {
                            handles.extend([nd.h_in, nd.h_out]);
                        } else if (ni + 1) % n == i && (sub.closed || i > 0) {
                            handles.push(nd.h_in);
                        } else if (i + 1) % n == ni && (sub.closed || ni > 0) {
                            handles.push(nd.h_out);
                        }
                    }
                }
                for hq in handles {
                    if hq != nd.p {
                        if let Some(hp) = self.norm_to_screen(obj, hq) {
                            painter.line_segment([c, hp], Stroke::new(1.0_f32, handle_col));
                            painter.circle_filled(hp, 4.0, handle_col);
                        }
                    }
                }
                let is_sel = sel.is_some_and(|(_, ss, ni)| ss == s && ni == i);
                let fill = if is_sel { col } else { Color32::WHITE };
                if nd.smooth {
                    painter.circle_filled(c, 4.5, fill);
                    painter.circle_stroke(c, 4.5, Stroke::new(1.0_f32, col));
                } else {
                    let r = Rect::from_center_size(c, Vec2::splat(8.0));
                    painter.rect_filled(r, 0.0, fill);
                    painter.add(Shape::closed_line(
                        vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()],
                        Stroke::new(1.0_f32, col),
                    ));
                }
            }
        }
    }

    fn draw_pen_overlay(&self, painter: &egui::Painter) {
        let Some(d) = &self.pen else { return };
        let col = Color32::from_rgb(255, 120, 0);
        let pt = |q: [f32; 2]| Pt::new(q[0], q[1]);
        let seg = |a: &PathNode, b: &PathNode| -> Vec<Pos2> {
            (0..=32)
                .map(|k| vector::cubic(pt(a.p), pt(a.h_out), pt(b.h_in), pt(b.p), k as f32 / 32.0))
                .map(|p| self.to_screen(p))
                .collect()
        };
        for w in d.nodes.windows(2) {
            painter.add(Shape::line(seg(&w[0], &w[1]), Stroke::new(2.0_f32, col)));
        }
        if d.closing && d.nodes.len() >= 2 {
            painter.add(Shape::line(seg(&d.nodes[d.nodes.len() - 1], &d.nodes[0]), Stroke::new(2.0_f32, col)));
        } else if !self.pressing {
            if let (Some(last), Some(c)) = (d.nodes.last(), self.cursor_doc) {
                let ghost = PathNode::corner([c.x, c.y]);
                painter.add(Shape::line(seg(last, &ghost), Stroke::new(1.0_f32, col.gamma_multiply(0.6))));
            }
        }
        let active = if d.closing { 0 } else { d.nodes.len().saturating_sub(1) };
        for (i, nd) in d.nodes.iter().enumerate() {
            let c = self.to_screen(pt(nd.p));
            if i == active {
                for hq in [nd.h_in, nd.h_out] {
                    if hq != nd.p {
                        let hp = self.to_screen(pt(hq));
                        painter.line_segment([c, hp], Stroke::new(1.0_f32, col));
                        painter.circle_filled(hp, 3.5, col);
                    }
                }
            }
            let size = if i == 0 && d.nodes.len() >= 2 { 10.0 } else { 7.0 };
            let r = Rect::from_center_size(c, Vec2::splat(size));
            painter.rect_filled(r, 0.0, if i == 0 { col } else { Color32::WHITE });
            painter.add(Shape::closed_line(
                vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()],
                Stroke::new(1.0_f32, col),
            ));
        }
    }
}

// -------------------------------------------------------------------- property panels

fn object_props(
    ui: &mut egui::Ui,
    obj: &mut Object,
    actions: &mut Vec<PropAction>,
    has_clip: bool,
    focus_text: bool,
) {
    ui.horizontal(|ui| {
        ui.strong(obj.kind.label());
        ui.text_edit_singleline(&mut obj.name);
    });
    ui.add(egui::Slider::new(&mut obj.opacity, 0.0..=1.0).text("Opacity"));

    let mut refit_text = false;
    egui::CollapsingHeader::new("Shape").default_open(true).show(ui, |ui| {
        match &mut obj.kind {
            ShapeKind::Rect { corner } => {
                ui.add(egui::Slider::new(corner, 0.0..=1.0).text("Corner radius"));
            }
            ShapeKind::Polygon { sides, inner } => {
                ui.add(egui::Slider::new(sides, 3..=24).text("Sides"));
                ui.add(egui::Slider::new(inner, 0.1..=1.0).text("Star depth"));
            }
            ShapeKind::Ellipse => {
                ui.label("Ellipse — use the handles to resize, or Edit nodes to reshape it freely.");
            }
            ShapeKind::Freehand { .. } => {
                ui.label("Freehand shape.");
            }
            ShapeKind::Text { text, font } => {
                ui.label("Text");
                let r = ui.add(
                    egui::TextEdit::multiline(text)
                        .id(egui::Id::new("jabro_text_field"))
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                if focus_text {
                    r.request_focus();
                }
                if r.changed() {
                    refit_text = true;
                }
                let before = *font;
                ui.horizontal(|ui| {
                    ui.label("Font");
                    egui::ComboBox::from_id_salt("obj_font")
                        .selected_text(font.label())
                        .show_ui(ui, |ui| {
                            for f in FontChoice::ALL {
                                ui.selectable_value(font, f, f.label());
                            }
                        });
                });
                if *font != before {
                    refit_text = true;
                }
            }
            ShapeKind::Path { subpaths } => {
                let nodes: usize = subpaths.iter().map(|s| s.nodes.len()).sum();
                ui.label(format!("Path with {} nodes in {} sub-path(s).", nodes, subpaths.len()));
                let mut all_closed = subpaths.iter().all(|s| s.closed);
                if ui.checkbox(&mut all_closed, "Closed").changed() {
                    for s in subpaths.iter_mut() {
                        s.closed = all_closed;
                    }
                }
            }
            ShapeKind::Bitmap => {
                ui.label("Paint layer — its shape is whatever you paint on it.");
            }
        }
        ui.horizontal_wrapped(|ui| match obj.kind {
            ShapeKind::Bitmap => {}
            ShapeKind::Text { .. } => {
                if ui
                    .button("Convert to path")
                    .on_hover_text("Turns the letters into editable outlines. Afterwards the text can no longer be retyped.")
                    .clicked()
                {
                    actions.push(PropAction::ConvertToPath);
                }
            }
            _ => {
                if ui
                    .button("Edit nodes")
                    .on_hover_text("Reshape the outline with the node tool (A). Shapes are turned into an editable path first.")
                    .clicked()
                {
                    actions.push(PropAction::EditNodes);
                }
            }
        });
    });
    if refit_text {
        obj.fit_text_width();
    }

    ui.add_space(4.0);
    ui.strong("Style presets");
    ui.horizontal_wrapped(|ui| {
        for (name, f) in styles::PRESETS {
            if ui.button(*name).clicked() {
                f(obj);
            }
        }
    });
    ui.horizontal(|ui| {
        if ui.button("Copy style").clicked() {
            actions.push(PropAction::CopyStyle);
        }
        if ui.add_enabled(has_clip, egui::Button::new("Paste style")).clicked() {
            actions.push(PropAction::PasteStyle);
        }
    });

    if obj.kind != ShapeKind::Bitmap {
        egui::CollapsingHeader::new("Fill").default_open(true).show(ui, |ui| {
            egui::ComboBox::from_id_salt("fill_kind")
                .selected_text(obj.fill.kind.label())
                .show_ui(ui, |ui| {
                    for k in FillKind::ALL {
                        ui.selectable_value(&mut obj.fill.kind, k, k.label());
                    }
                });
            match obj.fill.kind {
                FillKind::None => {}
                FillKind::Solid => rgba_label(ui, "Colour", &mut obj.fill.a),
                FillKind::Linear | FillKind::Radial => {
                    rgba_label(ui, "Start colour", &mut obj.fill.a);
                    rgba_label(ui, "End colour", &mut obj.fill.b);
                    if obj.fill.kind == FillKind::Linear {
                        ui.add(egui::Slider::new(&mut obj.fill.angle, 0.0..=360.0).text("Angle"));
                    }
                }
                FillKind::Texture => {
                    egui::ComboBox::from_id_salt("tex_kind")
                        .selected_text(obj.fill.texture.label())
                        .show_ui(ui, |ui| {
                            for t in TextureKind::ALL {
                                ui.selectable_value(&mut obj.fill.texture, t, t.label());
                            }
                        });
                    rgba_label(ui, "Dark", &mut obj.fill.a);
                    rgba_label(ui, "Light", &mut obj.fill.b);
                    ui.add(egui::Slider::new(&mut obj.fill.tex_scale, 0.2..=5.0).logarithmic(true).text("Scale"));
                }
            }
        });
        egui::CollapsingHeader::new("Outline").default_open(false).show(ui, |ui| {
            ui.checkbox(&mut obj.stroke.enabled, "Enabled");
            rgba_label(ui, "Colour", &mut obj.stroke.color);
            ui.add(egui::Slider::new(&mut obj.stroke.width, 0.5..=40.0).text("Width"));
        });
    }

    egui::CollapsingHeader::new("Bevel & material").default_open(true).show(ui, |ui| {
        let b = &mut obj.effects.bevel;
        ui.checkbox(&mut b.enabled, "Enabled");
        egui::ComboBox::from_id_salt("bevel_profile")
            .selected_text(b.profile.label())
            .show_ui(ui, |ui| {
                for p in BevelProfile::ALL {
                    ui.selectable_value(&mut b.profile, p, p.label());
                }
            });
        if b.profile != BevelProfile::Pillow {
            ui.add(egui::Slider::new(&mut b.width, 1.0..=80.0).text("Width"));
        }
        ui.add(egui::Slider::new(&mut b.depth, 0.05..=3.0).text("Depth"));
        ui.add(egui::Slider::new(&mut b.softness, 0.0..=8.0).text("Softness"));
        ui.add(egui::Slider::new(&mut b.light_angle, 0.0..=360.0).text("Light angle"));
        ui.add(egui::Slider::new(&mut b.light_elevation, 5.0..=85.0).text("Light height"));
        ui.add(egui::Slider::new(&mut b.intensity, 0.0..=1.5).text("Shading"));
        rgba_label(ui, "Highlight", &mut b.highlight);
        rgba_label(ui, "Shadow", &mut b.shadow);
        ui.add(egui::Slider::new(&mut b.gloss, 0.0..=1.0).text("Gloss"));
        ui.add(egui::Slider::new(&mut b.shininess, 2.0..=120.0).logarithmic(true).text("Shininess"));
        ui.add(egui::Slider::new(&mut b.reflection, 0.0..=1.0).text("Chrome reflection"));
    });

    egui::CollapsingHeader::new("Drop shadow").default_open(false).show(ui, |ui| {
        let s = &mut obj.effects.shadow;
        ui.checkbox(&mut s.enabled, "Enabled");
        rgba_label(ui, "Colour", &mut s.color);
        ui.add(egui::Slider::new(&mut s.dx, -60.0..=60.0).text("Offset X"));
        ui.add(egui::Slider::new(&mut s.dy, -60.0..=60.0).text("Offset Y"));
        ui.add(egui::Slider::new(&mut s.blur, 0.0..=60.0).text("Blur"));
        ui.add(egui::Slider::new(&mut s.opacity, 0.0..=1.0).text("Opacity"));
    });

    egui::CollapsingHeader::new("Outer glow").default_open(false).show(ui, |ui| {
        let g = &mut obj.effects.glow;
        ui.checkbox(&mut g.enabled, "Enabled");
        rgba_label(ui, "Colour", &mut g.color);
        ui.add(egui::Slider::new(&mut g.size, 1.0..=80.0).text("Size"));
        ui.add(egui::Slider::new(&mut g.opacity, 0.0..=1.0).text("Opacity"));
    });

    egui::CollapsingHeader::new("Transform & paint").default_open(false).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Reset perspective").clicked() {
                actions.push(PropAction::ResetPerspective);
            }
            if ui.button("Flip H").clicked() {
                actions.push(PropAction::FlipH);
            }
            if ui.button("Flip V").clicked() {
                actions.push(PropAction::FlipV);
            }
            if ui.button("To front").clicked() {
                actions.push(PropAction::Front);
            }
            if ui.button("To back").clicked() {
                actions.push(PropAction::Back);
            }
        });
        match &obj.paint {
            Some(p) => {
                ui.label(format!("Paint layer: {} × {} px", p.width(), p.height()));
                if ui.button("Clear paint").clicked() {
                    actions.push(PropAction::ClearPaint);
                }
            }
            None => {
                ui.label("No paint yet — use the Brush tool on this object.");
            }
        }
    });
}

fn doc_props(
    ui: &mut egui::Ui,
    doc: &mut Document,
    new_w: &mut u32,
    new_h: &mut u32,
    resize: &mut Option<(u32, u32)>,
) {
    ui.strong("Document");
    rgba_label(ui, "Background", &mut doc.background);
    ui.horizontal(|ui| {
        ui.add(egui::DragValue::new(new_w).range(16..=8192).suffix(" px"));
        ui.label("×");
        ui.add(egui::DragValue::new(new_h).range(16..=8192).suffix(" px"));
        if ui.button("Resize canvas").clicked() {
            *resize = Some((*new_w, *new_h));
        }
    });
    ui.add_space(8.0);
    ui.label(format!("{} objects. Select one to edit its fill, bevel, shadow and paint.", doc.objects.len()));
    ui.add_space(8.0);
    ui.small(
        "Workflow: draw a shape → apply a style preset → paint on it with the Brush (paint stays \
         clipped to the shape) → put it in perspective with the Perspective tool → choose an NPR look \
         at the top to render the whole picture as watercolor, oil, pencil…",
    );
}

impl eframe::App for JabroApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.shortcuts(ctx);
        self.read_pressure(ctx);
        egui::TopBottomPanel::top("menu").show(ctx, |ui| self.menu_bar(ui, ctx));
        egui::TopBottomPanel::top("look").show(ctx, |ui| self.look_bar(ui));
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| self.status_bar(ui));
        egui::SidePanel::left("tools")
            .resizable(false)
            .exact_width(190.0)
            .show(ctx, |ui| self.tool_panel(ui));
        egui::SidePanel::right("props")
            .default_width(310.0)
            .show(ctx, |ui| self.right_panel(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(Color32::from_gray(52)))
            .show(ctx, |ui| self.canvas(ui));
        self.poll_npr(ctx);
        if self.show_about {
            let mut open = true;
            egui::Window::new("About Jabro Draw")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.heading("Jabro Draw 0.1");
                    ui.label("A hybrid vector + paint editor: shapes you can paint on, bevel, texture, bend into perspective and render with NPR looks.");
                    ui.label("Inspired by the classic VectorBit idea. Written in Rust with egui + tiny-skia.");
                });
            self.show_about = open;
        }
    }
}

#[cfg(test)]
mod tests {
    //! Scripted mouse/keyboard sequences against the real editor state (no window needed).
    use super::*;

    fn app_with(objects: Vec<Object>) -> JabroApp {
        let mut app = JabroApp::with_worker(NprWorker::new(|| {}));
        let mut doc = Document::new(800, 600);
        doc.next_id = 100;
        doc.objects = objects;
        app.doc = doc;
        app.canvas_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        app.zoom = 1.0;
        app.pan = Vec2::ZERO;
        app
    }

    fn click(app: &mut JabroApp, p: Pt, mods: Modifiers) {
        let s = app.to_screen(p);
        app.on_press(p, s, mods);
        app.on_release(mods);
    }

    fn drag(app: &mut JabroApp, from: Pt, to: Pt, mods: Modifiers) {
        let s = app.to_screen(from);
        app.on_press(from, s, mods);
        for k in 1..=5 {
            app.on_drag(from.lerp(to, k as f32 / 5.0), mods);
        }
        app.on_release(mods);
    }

    fn rect(id: u64, x: f32, y: f32) -> Object {
        Object::new(id, ShapeKind::Rect { corner: 0.0 }, x, y, 100.0, 100.0)
    }

    fn path_of(o: &Object) -> &Vec<SubPath> {
        match &o.kind {
            ShapeKind::Path { subpaths } => subpaths,
            _ => panic!("not a path"),
        }
    }

    fn node_doc(o: &Object, q: [f32; 2]) -> Pt {
        o.homography().apply_pt(Pt::new(q[0] * o.size[0], q[1] * o.size[1])).unwrap()
    }

    #[test]
    fn pen_draws_a_closed_curve_and_undo_removes_it() {
        let mut app = app_with(vec![]);
        app.tool = Tool::Pen;
        let none = Modifiers::NONE;
        click(&mut app, Pt::new(100.0, 100.0), none);
        drag(&mut app, Pt::new(300.0, 100.0), Pt::new(350.0, 160.0), none); // smooth node
        click(&mut app, Pt::new(200.0, 300.0), none);
        click(&mut app, Pt::new(100.0, 100.0), none); // first node closes
        assert!(app.pen.is_none());
        assert_eq!(app.doc.objects.len(), 1);
        let o = &app.doc.objects[0];
        let sp = &path_of(o)[0];
        assert!(sp.closed);
        assert_eq!(sp.nodes.len(), 3);
        assert!(sp.nodes[1].smooth && !sp.nodes[0].smooth);
        let p1 = node_doc(o, sp.nodes[1].p);
        assert!(p1.dist(Pt::new(300.0, 100.0)) < 0.5, "node stays where it was clicked: {p1:?}");
        let h = node_doc(o, sp.nodes[1].h_out);
        assert!(h.dist(Pt::new(350.0, 160.0)) < 0.5, "handle follows the drag: {h:?}");
        assert_eq!(app.selected, Some(o.id));
        app.undo();
        assert!(app.doc.objects.is_empty());
    }

    #[test]
    fn pen_enter_finishes_an_open_stroke() {
        let mut app = app_with(vec![]);
        app.tool = Tool::Pen;
        click(&mut app, Pt::new(50.0, 50.0), Modifiers::NONE);
        click(&mut app, Pt::new(250.0, 80.0), Modifiers::NONE);
        app.finish_pen(false); // what Enter does
        let o = &app.doc.objects[0];
        assert!(!path_of(o)[0].closed);
        assert_eq!(o.fill.kind, FillKind::None);
        assert!(o.stroke.enabled);
        app.undo();
        assert!(app.doc.objects.is_empty(), "open path creation is undoable too");
    }

    #[test]
    fn marquee_and_shift_click_build_a_selection_for_booleans() {
        let mut app = app_with(vec![rect(1, 50.0, 50.0), rect(2, 100.0, 80.0), rect(3, 600.0, 400.0)]);
        app.tool = Tool::Select;
        drag(&mut app, Pt::new(10.0, 10.0), Pt::new(300.0, 300.0), Modifiers::NONE);
        let mut sel = app.selection();
        sel.sort();
        assert_eq!(sel, vec![1, 2]);
        click(&mut app, Pt::new(650.0, 450.0), Modifiers::SHIFT);
        assert_eq!(app.selection().len(), 3);
        click(&mut app, Pt::new(650.0, 450.0), Modifiers::SHIFT);
        assert_eq!(app.selection().len(), 2, "shift+click toggles");

        app.boolean(BoolOp::Union);
        assert_eq!(app.doc.objects.len(), 2, "two rects merged, third untouched");
        let merged = app.doc.objects.iter().find(|o| matches!(o.kind, ShapeKind::Path { .. })).unwrap();
        let (lo, hi) = merged.bounds();
        assert!(lo.dist(Pt::new(50.0, 50.0)) < 1.0 && hi.dist(Pt::new(200.0, 180.0)) < 1.0, "{lo:?} {hi:?}");
        app.undo();
        assert_eq!(app.doc.objects.len(), 3);
    }

    #[test]
    fn group_move_moves_every_selected_object() {
        let mut app = app_with(vec![rect(1, 50.0, 50.0), rect(2, 300.0, 50.0)]);
        app.tool = Tool::Select;
        click(&mut app, Pt::new(60.0, 60.0), Modifiers::NONE);
        click(&mut app, Pt::new(310.0, 60.0), Modifiers::SHIFT);
        drag(&mut app, Pt::new(320.0, 70.0), Pt::new(340.0, 110.0), Modifiers::NONE);
        assert!(app.doc.objects[0].quad[0].dist(Pt::new(70.0, 90.0)) < 1e-3);
        assert!(app.doc.objects[1].quad[0].dist(Pt::new(320.0, 90.0)) < 1e-3);
    }

    #[test]
    fn node_tool_drags_one_node_and_inserts_on_segments() {
        let mut app = app_with(vec![rect(1, 100.0, 100.0)]);
        app.selected = Some(1);
        app.convert_selected_to_path();
        app.tool = Tool::Node;
        // drag the top-right corner node up and to the right
        drag(&mut app, Pt::new(200.0, 100.0), Pt::new(240.0, 60.0), Modifiers::NONE);
        let o = &app.doc.objects[0];
        let nodes: Vec<Pt> = path_of(o)[0].nodes.iter().map(|n| node_doc(o, n.p)).collect();
        assert!(nodes.iter().any(|p| p.dist(Pt::new(240.0, 60.0)) < 0.5), "{nodes:?}");
        assert!(nodes.iter().any(|p| p.dist(Pt::new(100.0, 100.0)) < 0.5), "other corners stay");
        assert!(nodes.iter().any(|p| p.dist(Pt::new(100.0, 200.0)) < 0.5));
        let before = path_of(&app.doc.objects[0])[0].nodes.len();
        // click the middle of the bottom edge -> new node
        click(&mut app, Pt::new(150.0, 200.0), Modifiers::NONE);
        let after = path_of(&app.doc.objects[0])[0].nodes.len();
        assert_eq!(after, before + 1);
        // and delete it again
        app.delete_node();
        assert_eq!(path_of(&app.doc.objects[0])[0].nodes.len(), before);
    }

    fn text_obj(id: u64, s: &str) -> Object {
        let kind = ShapeKind::Text {
            text: s.into(),
            font: FontChoice::Sans,
        };
        Object::new(id, kind, 300.0, 100.0, 160.0, 60.0)
    }

    #[test]
    fn edit_shape_makes_rectangles_editable_and_text_retypable() {
        let mut app = app_with(vec![rect(1, 50.0, 50.0), text_obj(2, "Hi")]);
        app.edit_shape(1);
        assert!(matches!(app.doc.objects[0].kind, ShapeKind::Path { .. }));
        assert_eq!(app.tool, Tool::Node);
        app.undo();
        assert!(matches!(app.doc.objects[0].kind, ShapeKind::Rect { .. }), "one undo step");

        app.tool = Tool::Select;
        app.edit_shape(2);
        assert!(app.focus_text, "text goes to its text field");
        assert!(matches!(app.doc.objects[1].kind, ShapeKind::Text { .. }), "text stays text");
    }

    #[test]
    fn font_dropdown_changes_the_selected_text() {
        let mut app = app_with(vec![text_obj(2, "Hello")]);
        app.select_only(Some(2));
        app.set_font(FontChoice::Impact);
        assert!(matches!(&app.doc.objects[0].kind,
            ShapeKind::Text { font: FontChoice::Impact, text } if text == "Hello"));
        assert_eq!(app.text_font, FontChoice::Impact, "also the default for new text");
        app.undo();
        assert!(matches!(&app.doc.objects[0].kind, ShapeKind::Text { font: FontChoice::Sans, .. }));
    }

    #[test]
    fn node_tool_click_converts_an_ellipse_on_the_spot() {
        let mut app = app_with(vec![Object::new(1, ShapeKind::Ellipse, 100.0, 100.0, 200.0, 100.0)]);
        app.tool = Tool::Node;
        click(&mut app, Pt::new(200.0, 150.0), Modifiers::NONE);
        assert!(matches!(&app.doc.objects[0].kind,
            ShapeKind::Path { subpaths } if subpaths[0].nodes.len() == 4));
        assert_eq!(app.selected, Some(1));
    }
}
