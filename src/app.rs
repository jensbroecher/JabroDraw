//! The editor UI (egui): tools, canvas interaction, property panels, menus.

use crate::brush::{self, BrushKind, BrushSettings, StrokeState};
use crate::doc::*;
use crate::geom::{simplify, Pt};
use crate::npr::{self, NprJob, NprWorker};
use crate::ai::{self, AiJob, AiModel, AiWorker};
use crate::canvas::{self, Settings};
use crate::render::Renderer;
use crate::vector::{self, BoolOp};
use crate::{demo, imageops, io, shapes, styles};
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
    Transparency,
    Light,
}

impl Tool {
    const ALL: [Tool; 12] = [
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
        Tool::Transparency,
        Tool::Light,
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
            Tool::Transparency => "Transparency  (X)",
            Tool::Light => "Lights  (L)",
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
            Tool::Transparency => egui::Key::X,
            Tool::Light => egui::Key::L,
        }
    }
    fn hint(self) -> &'static str {
        match self {
            Tool::Select => "Click to select, Shift+click or drag a box to select several. Handles scale, the round handle rotates. Double-click to edit the shape (or the text).",
            Tool::Node => "Drag nodes and handles (Alt breaks a smooth handle). Click a segment to add a node, double-click a node for smooth/corner, Del removes it.",
            Tool::Pen => "Click for corners, click+drag for curves. Right-click (or click the first node) closes the shape, Enter finishes an open path, Backspace undoes a node, Esc cancels.",
            Tool::Perspective => "Drag any corner of the selected object to put it in perspective — paint follows along.",
            Tool::Rect => "Drag to draw a rectangle (Shift = square). Click for a default size.",
            Tool::Ellipse => "Drag to draw an ellipse (Shift = circle).",
            Tool::Polygon => "Drag to draw a star or polygon. Sides / star depth in the tool panel.",
            Tool::Freehand => "Draw a closed freehand shape — it becomes a smooth vector curve.",
            Tool::Text => "Click or drag a box to place text, then edit it in the properties panel.",
            Tool::Brush => "Paint on the selected object (clipped to its shape) or on a new paint layer. Alt+click picks a colour.",
            Tool::Transparency => "Drag across an object to fade it out: from opaque where you start to transparent where you let go. Drag the two handles to adjust.",
            Tool::Light => "Click an object to add a light there (type in the tool panel). Drag a lamp to move it, a spotlight's diamond to aim it.",
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
    /// New transparency fade being dragged out from `q0` (normalised local point).
    TransNew { id: u64, q0: [f32; 2] },
    /// Moving the start (0) or end (1) handle of the fade.
    TransHandle { id: u64, which: usize },
    LightPos { id: u64, idx: usize },
    LightTarget { id: u64, idx: usize },
    LightDir { id: u64, idx: usize },
    /// Moving (or creating) ruler guide `idx`.
    Guide { idx: usize },
    Scale { id: u64, handle: usize, anchor: Pt, h0: Pt, orig: [Pt; 4], angle: f32 },
    Rotate { id: u64, center: Pt, a0: f32, orig: [Pt; 4] },
    Corner { id: u64, idx: usize },
    Create { start: Pt, cur: Pt },
    Freehand { pts: Vec<Pt> },
    Paint { id: u64, last: (f32, f32), state: StrokeState, mask: bool },
}

enum PropAction {
    ConvertToPath,
    EditNodes,
    LoadFillImage,
    LoadTransImage,
    RemoveBackground,
    MaskInvert,
    MaskClear,
    ResetPerspective,
    FlipH,
    FlipV,
    ResetBrushWork,
    AiRemoveBackground,
    CopyStyle,
    PasteStyle,
    Front,
    Back,
}

/// Cheap fingerprint of a picture, to recognise our own copy on the system clipboard.
fn image_signature(img: &Pixmap) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ ((img.width() as u64) << 32 | img.height() as u64);
    for (i, b) in img.data().iter().enumerate().step_by(61) {
        h = (h ^ (*b as u64 ^ i as u64)).wrapping_mul(0x100_0000_01b3);
    }
    h
}

fn set_clipboard_image(img: &Pixmap) -> Result<(), String> {
    let mut bytes = Vec::with_capacity(img.data().len());
    for px in img.data().chunks_exact(4) {
        let c = tiny_skia::PremultipliedColorU8::from_rgba(px[0], px[1], px[2], px[3])
            .map(|c| c.demultiply())
            .map_or([0, 0, 0, 0], |c| [c.red(), c.green(), c.blue(), c.alpha()]);
        bytes.extend_from_slice(&c);
    }
    let data = arboard::ImageData {
        width: img.width() as usize,
        height: img.height() as usize,
        bytes: bytes.into(),
    };
    arboard::Clipboard::new()
        .and_then(|mut cb| cb.set_image(data))
        .map_err(|e| e.to_string())
}

/// A picture on the system clipboard (screenshots, images copied in other programs).
fn get_clipboard_image() -> Option<Pixmap> {
    let img = arboard::Clipboard::new().ok()?.get_image().ok()?;
    let (w, h) = (img.width as u32, img.height as u32);
    let mut pm = Pixmap::new(w, h)?;
    for (d, s) in pm.data_mut().chunks_exact_mut(4).zip(img.bytes.chunks_exact(4)) {
        let a = s[3] as u32;
        d[0] = ((s[0] as u32 * a + 127) / 255) as u8;
        d[1] = ((s[1] as u32 * a + 127) / 255) as u8;
        d[2] = ((s[2] as u32 * a + 127) / 255) as u8;
        d[3] = s[3];
    }
    Some(pm)
}

/// Is Ctrl+V held right now? egui reports no paste event when the clipboard holds only a
/// picture, so on Windows the key state is checked directly.
#[cfg(windows)]
fn ctrl_v_down() -> bool {
    #[link(name = "user32")]
    extern "system" {
        fn GetAsyncKeyState(vkey: i32) -> i16;
    }
    // SAFETY: plain Win32 call without pointers.
    unsafe { GetAsyncKeyState(0x11) < 0 && GetAsyncKeyState(0x56) < 0 }
}

#[cfg(not(windows))]
fn ctrl_v_down() -> bool {
    false
}

#[derive(Clone, Copy)]
enum AlignTo {
    Left,
    CenterH,
    Right,
    Top,
    Middle,
    Bottom,
}

/// State of the New document / Canvas size dialog.
struct CanvasDialog {
    new_doc: bool,
    w: u32,
    h: u32,
    lock: bool,
    /// Which part of the picture stays put when resizing: (column, row) 0..2;
    /// (9, 9) means "fit to content".
    anchor: (u8, u8),
}

enum DocAction {
    CanvasDialog,
    FitContent,
    AutoGrow(bool),
}

/// Settings of the properties panel's Picture section.
struct PictureUi {
    tol: f32,
    feather: f32,
    ai_model: AiModel,
    ai_busy: bool,
}

/// Undo all brush work on an object: pictures go back to how they were imported, other
/// objects lose their painting; painted transparency is cleared either way.
fn reset_brush_work(o: &mut Object) {
    o.paint = o.original.as_ref().map(|i| i.0.clone());
    o.mask = None;
    o.touch();
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

/// Move the `moving` ids as one block (keeping their relative order) to before position `gap`
/// of `list` (gap == list.len() means the end).
fn reorder_ids(list: &[u64], moving: &[u64], gap: usize) -> Vec<u64> {
    let gap = gap.min(list.len());
    let before = list[..gap].iter().filter(|id| moving.contains(id)).count();
    let block: Vec<u64> = list.iter().copied().filter(|id| moving.contains(id)).collect();
    let mut rest: Vec<u64> = list.iter().copied().filter(|id| !moving.contains(id)).collect();
    let at = (gap - before).min(rest.len());
    rest.splice(at..at, block);
    rest
}

/// Canvas point -> (paint-layer pixel, paint pixels per canvas pixel).
/// The layer a brush works on: the paint, or the transparency mask.
fn brush_layer(obj: &mut Object, mask: bool) -> Option<&mut Arc<Pixmap>> {
    if mask {
        obj.mask.as_mut()
    } else {
        obj.paint.as_mut()
    }
}

/// Give the object a (fully visible) transparency mask if it has none. Pictures get one at
/// their own resolution so the magic wand works pixel by pixel.
fn ensure_mask(obj: &mut Object) {
    if obj.mask.is_none() {
        let (w, h) = match (&obj.kind, &obj.paint) {
            (ShapeKind::Bitmap, Some(p)) => (p.width(), p.height()),
            _ => obj.raster_size(),
        };
        obj.mask = imageops::new_mask(w, h).map(Arc::new);
    }
}

fn doc_to_paint(obj: &Object, p: Pt) -> Option<((f32, f32), f32)> {
    let paint = obj.paint.as_ref()?;
    doc_to_layer(obj, p, paint.width(), paint.height())
}

fn doc_to_mask(obj: &Object, p: Pt) -> Option<((f32, f32), f32)> {
    let mask = obj.mask.as_ref()?;
    doc_to_layer(obj, p, mask.width(), mask.height())
}

/// Canvas point -> (pixel in a layer of lw x lh stretched over the object, layer pixels per
/// canvas pixel).
fn doc_to_layer(obj: &Object, p: Pt, lw: u32, lh: u32) -> Option<((f32, f32), f32)> {
    let inv = obj.homography().inverse()?;
    let (lx, ly) = inv.apply(p.x as f64, p.y as f64)?;
    let (lx2, ly2) = inv.apply(p.x as f64 + 1.0, p.y as f64)?;
    let (lx3, ly3) = inv.apply(p.x as f64, p.y as f64 + 1.0)?;
    let kx = lw as f64 / obj.size[0].max(1.0) as f64;
    let ky = lh as f64 / obj.size[1].max(1.0) as f64;
    let sx = ((lx2 - lx) * kx).hypot((ly2 - ly) * ky);
    let sy = ((lx3 - lx) * kx).hypot((ly3 - ly) * ky);
    Some((((lx * kx) as f32, (ly * ky) as f32), ((sx + sy) * 0.5) as f32))
}

fn pct_slider(v: &mut f32) -> egui::Slider<'_> {
    egui::Slider::new(v, 0.0..=1.0).custom_formatter(|n, _| format!("{:.0}%", n * 100.0))
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
    /// Objects being dragged in the object list (ids in list order).
    list_drag: Option<Vec<u64>>,
    /// Fade type a Transparency-tool drag creates.
    trans_kind: TransparencyKind,
    /// Light type a Lights-tool click adds, and the light being edited.
    new_light_kind: LightKind,
    light_sel: Option<usize>,
    /// Colour tolerance (0..1) and edge feather (px) for background removal / magic wand.
    bg_tol: f32,
    bg_feather: f32,
    ai: AiWorker,
    ai_model: AiModel,
    /// Object the AI is currently working on.
    ai_busy: Option<u64>,
    /// The document as it was last saved / opened (to detect unsaved changes).
    saved_doc: Document,
    close_dialog: bool,
    allow_close: bool,
    title: String,
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
    style_clipboard: Option<(FillStyle, StrokeStyle, Effects, Transparency)>,
    show_about: bool,
    settings: Settings,
    canvas_dialog: Option<CanvasDialog>,
    /// Objects copied with Ctrl+C / Ctrl+X (pasted back as editable objects).
    clipboard: Vec<Object>,
    paste_count: u32,
    /// Fingerprint of the picture we put on the system clipboard.
    clip_image_sig: Option<u64>,
    /// Use the system clipboard (off in tests so they never touch it).
    system_clipboard: bool,
    ctrl_v_was_down: bool,
    /// Snap lines to show while dragging: (vertical?, position).
    snap_lines: Vec<(bool, f32)>,
}

impl JabroApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let ctx = cc.egui_ctx.clone();
        let ctx2 = ctx.clone();
        let mut app = Self::with_worker(
            NprWorker::new(move || ctx.request_repaint()),
            AiWorker::new(move || ctx2.request_repaint()),
        );
        app.settings = Settings::load();
        app.system_clipboard = true;
        app
    }

    fn with_worker(worker: NprWorker, ai: AiWorker) -> Self {
        let doc = demo::demo_document();

        let saved_doc = doc.clone();
        Self {
            doc,
            selected: None,
            multi: Vec::new(),
            node_sel: None,
            pen: None,
            stroke_width: 4.0,
            focus_text: false,
            list_drag: None,
            trans_kind: TransparencyKind::Linear,
            new_light_kind: LightKind::Point,
            light_sel: None,
            bg_tol: 0.15,
            bg_feather: 1.5,
            ai,
            ai_model: AiModel::Best,
            ai_busy: None,
            saved_doc,
            close_dialog: false,
            allow_close: false,
            title: String::new(),
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
            settings: Settings::default(),
            canvas_dialog: None,
            clipboard: Vec::new(),
            paste_count: 0,
            clip_image_sig: None,
            system_clipboard: false,
            ctrl_v_was_down: false,
            snap_lines: Vec::new(),
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
                ..Default::default()
            };
        }
        self.doc.objects.push(obj);
        self.select_only(Some(id));
        self.dirty = true;
        self.status = if closed { "Closed path created." } else { "Open path created (stroke only)." }.into();
    }

    /// Right-click with the pen: join the last node back to the first and finish the shape.
    fn close_pen_path(&mut self) {
        match self.pen.as_ref().map(|d| d.nodes.len()) {
            Some(n) if n >= 2 => self.finish_pen(true),
            Some(_) => self.status = "Place at least two nodes before closing the path.".into(),
            None => {}
        }
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
        self.saved_doc = doc.clone();
        self.doc = doc;
        self.renderer.clear();
        self.selected = None;
        self.fit_pending = true;
        self.dirty = true;
    }

    fn new_doc(&mut self, w: u32, h: u32) {
        let mut d = Document::new(w, h);
        d.npr = self.doc.npr.clone();
        d.npr.style = NprStyle::None;
        self.replace_doc(d);
        self.path = None;
        self.status = format!("New {w} × {h} document");
    }

    /// Change the canvas size, moving every object by `offset` (used when the canvas grows to
    /// the left / top). The view is adjusted so nothing appears to jump. No undo step here.
    fn resize_canvas(&mut self, w: u32, h: u32, offset: Pt) {
        let w = w.clamp(canvas::MIN_SIZE, canvas::MAX_SIZE);
        let h = h.clamp(canvas::MIN_SIZE, canvas::MAX_SIZE);
        if offset != Pt::default() {
            for o in &mut self.doc.objects {
                o.quad = o.quad.map(|c| c + offset);
            }
        }
        // keep content at the same spot on screen: origin_new = origin_old - offset * zoom
        let o_new = self.origin() - Vec2::new(offset.x, offset.y) * self.zoom;
        self.doc.width = w;
        self.doc.height = h;
        let size = Vec2::new(w as f32, h as f32) * self.zoom;
        self.pan = (o_new - self.canvas_rect.center()) + size * 0.5;
        self.dirty = true;
    }

    /// Canvas just big enough for all objects (with a small margin).
    fn fit_canvas_to_content(&mut self) -> bool {
        let Some((lo, hi)) = canvas::content_bounds(&self.doc) else {
            self.status = "Nothing to fit — the document is empty.".into();
            return false;
        };
        let m = 16.0;
        let w = (hi.x - lo.x + 2.0 * m).ceil() as u32;
        let h = (hi.y - lo.y + 2.0 * m).ceil() as u32;
        self.resize_canvas(w, h, Pt::new(m - lo.x, m - lo.y));
        self.status = format!("Canvas fitted to the content: {} × {} px", self.doc.width, self.doc.height);
        true
    }

    /// Grow the canvas after a drop if something now sticks out (when the setting is on).
    fn auto_grow(&mut self) {
        if !self.settings.auto_grow {
            return;
        }
        let Some((lo, hi)) = canvas::content_bounds(&self.doc) else { return };
        if let Some((w, h, off)) = canvas::grow_to_fit(self.doc.width, self.doc.height, lo, hi, 8.0) {
            self.resize_canvas(w, h, off);
            self.status = format!("Canvas grew to {w} × {h} px to fit everything.");
        }
    }

    fn open_canvas_dialog(&mut self, new_doc: bool) {
        self.canvas_dialog = Some(CanvasDialog {
            new_doc,
            w: self.doc.width,
            h: self.doc.height,
            lock: false,
            anchor: (1, 1),
        });
    }

    fn canvas_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.canvas_dialog.take() else { return };
        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        let title = if d.new_doc { "New document" } else { "Canvas size" };
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Template");
                    let current = canvas::PRESETS
                        .iter()
                        .find(|p| p.w == d.w && p.h == d.h)
                        .map_or("Custom".to_string(), |p| p.name.to_string());
                    egui::ComboBox::from_id_salt("canvas_preset")
                        .selected_text(current)
                        .width(220.0)
                        .height(420.0)
                        .show_ui(ui, |ui| {
                            let mut group = "";
                            for p in canvas::PRESETS {
                                if p.group != group {
                                    group = p.group;
                                    ui.add_space(4.0);
                                    ui.strong(group);
                                }
                                let label = format!("{}   {} × {}", p.name, p.w, p.h);
                                if ui.selectable_label(d.w == p.w && d.h == p.h, label).clicked() {
                                    d.w = p.w;
                                    d.h = p.h;
                                }
                            }
                        });
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let ratio = d.w as f32 / d.h.max(1) as f32;
                    let rw = ui.add(egui::DragValue::new(&mut d.w).range(canvas::MIN_SIZE..=canvas::MAX_SIZE).suffix(" px"));
                    ui.label("×");
                    let rh = ui.add(egui::DragValue::new(&mut d.h).range(canvas::MIN_SIZE..=canvas::MAX_SIZE).suffix(" px"));
                    if d.lock && rw.changed() {
                        d.h = ((d.w as f32 / ratio).round() as u32).clamp(canvas::MIN_SIZE, canvas::MAX_SIZE);
                    } else if d.lock && rh.changed() {
                        d.w = ((d.h as f32 * ratio).round() as u32).clamp(canvas::MIN_SIZE, canvas::MAX_SIZE);
                    }
                    if ui.button("⇄").on_hover_text("Swap portrait / landscape").clicked() {
                        std::mem::swap(&mut d.w, &mut d.h);
                    }
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut d.lock, "Keep aspect ratio");
                    ui.label(format!("Aspect {}", canvas::aspect_label(d.w, d.h)));
                });
                if !d.new_doc {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("Keep the picture at");
                        egui::Grid::new("canvas_anchor").spacing([2.0, 2.0]).show(ui, |ui| {
                            for row in 0..3u8 {
                                for col in 0..3u8 {
                                    let sel = d.anchor == (col, row);
                                    if ui
                                        .add_sized([22.0, 22.0], egui::SelectableLabel::new(sel, if sel { "●" } else { "" }))
                                        .clicked()
                                    {
                                        d.anchor = (col, row);
                                    }
                                }
                                ui.end_row();
                            }
                        });
                    });
                    ui.small("The canvas grows or shrinks away from the chosen spot.");
                    ui.add_space(4.0);
                    if ui
                        .button("Fit canvas to content")
                        .on_hover_text("Make the canvas just big enough for all objects, shadows and glows included")
                        .clicked()
                    {
                        if let Some((lo, hi)) = canvas::content_bounds(&self.doc) {
                            d.w = ((hi.x - lo.x + 32.0).ceil() as u32).clamp(canvas::MIN_SIZE, canvas::MAX_SIZE);
                            d.h = ((hi.y - lo.y + 32.0).ceil() as u32).clamp(canvas::MIN_SIZE, canvas::MAX_SIZE);
                            d.anchor = (9, 9); // "fit" marker
                        }
                    }
                    let mut g = self.settings.auto_grow;
                    if ui
                        .checkbox(&mut g, "Grow the canvas automatically when objects go past the edge")
                        .changed()
                    {
                        self.settings.auto_grow = g;
                        self.settings.save();
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(if d.new_doc { "  Create  " } else { "  Apply  " }).clicked() {
                        apply = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if apply {
            if d.new_doc {
                self.new_doc(d.w, d.h);
            } else if d.w != self.doc.width || d.h != self.doc.height || d.anchor == (9, 9) {
                self.checkpoint();
                if d.anchor == (9, 9) {
                    self.fit_canvas_to_content();
                } else {
                    let dx = (d.w as f32 - self.doc.width as f32) * d.anchor.0 as f32 * 0.5;
                    let dy = (d.h as f32 - self.doc.height as f32) * d.anchor.1 as f32 * 0.5;
                    self.resize_canvas(d.w, d.h, Pt::new(dx.round(), dy.round()));
                    self.status = format!("Canvas is now {} × {} px", self.doc.width, self.doc.height);
                }
                self.fit_pending = true;
            }
        } else if open && !cancel {
            self.canvas_dialog = Some(d);
        }
    }

    fn open(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Jabro Draw", &["jabro"])
            .pick_file()
        {
            self.open_path(path);
        }
    }

    fn open_path(&mut self, path: PathBuf) {
        match io::load(&path) {
            Ok(d) => {
                self.replace_doc(d);
                self.status = format!("Opened {}", path.display());
                self.path = Some(path);
            }
            Err(e) => self.status = format!("Could not open: {e}"),
        }
    }

    /// Returns true when the document was written.
    fn save(&mut self, save_as: bool) -> bool {
        let path = match (&self.path, save_as) {
            (Some(p), false) => p.clone(),
            _ => {
                let Some(p) = rfd::FileDialog::new()
                    .add_filter("Jabro Draw", &["jabro"])
                    .set_file_name("drawing.jabro")
                    .save_file()
                else {
                    return false;
                };
                p
            }
        };
        match io::save(&self.doc, &path) {
            Ok(()) => {
                self.status = format!("Saved {}", path.display());
                self.path = Some(path);
                self.saved_doc = self.doc.clone();
                true
            }
            Err(e) => {
                self.status = format!("Could not save: {e}");
                false
            }
        }
    }

    fn has_unsaved_changes(&self) -> bool {
        self.pen.is_some() || !self.doc.same_content(&self.saved_doc)
    }

    fn doc_name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into())
    }

    /// Window close (X button, Alt+F4 or File › Quit): ask first if there is unsaved work.
    fn handle_close(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close && self.has_unsaved_changes() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_dialog = true;
        }
        if !self.close_dialog {
            return;
        }
        let mut choice = None;
        let modal = egui::Modal::new(egui::Id::new("close_dialog")).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.heading("Save changes?");
            ui.add_space(6.0);
            ui.label(format!("“{}” has changes that are not saved yet.", self.doc_name()));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("  Save  ").clicked() {
                    choice = Some(0);
                }
                if ui.button("Don't save").clicked() {
                    choice = Some(1);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(2);
                }
            });
        });
        if modal.should_close() && choice.is_none() {
            choice = Some(2);
        }
        match choice {
            Some(0) => {
                if self.save(false) {
                    self.close_dialog = false;
                    self.allow_close = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                // save dialog cancelled or failed: keep the question open
            }
            Some(1) => {
                self.close_dialog = false;
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Some(_) => self.close_dialog = false,
            None => {}
        }
    }

    /// "name* — Jabro Draw" while there are unsaved changes.
    fn update_title(&mut self, ctx: &egui::Context) {
        let title = format!(
            "{}{} — Jabro Draw",
            self.doc_name(),
            if self.has_unsaved_changes() { "*" } else { "" }
        );
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
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
        // new shapes and pen nodes start on a snap line too
        let p = if matches!(self.tool, Tool::Rect | Tool::Ellipse | Tool::Polygon | Tool::Text | Tool::Pen)
            && self.snapping(mods)
        {
            let q = self.snap_point(p, &[]);
            self.snap_lines.clear();
            q
        } else {
            p
        };
        match self.tool {
            Tool::Select | Tool::Perspective => self.press_select(p, sp, mods),
            Tool::Node => self.press_node(p, sp),
            Tool::Pen => self.press_pen(p, sp),
            Tool::Transparency => self.press_transparency(p, sp),
            Tool::Light => self.press_light(p, sp),
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

    fn press_transparency(&mut self, p: Pt, sp: Pos2) {
        if let Some(o) = self.selected.and_then(|id| self.doc.find(id)) {
            let t = &o.transparency;
            if matches!(t.kind, TransparencyKind::Linear | TransparencyKind::Radial) {
                for (which, q) in [(1, t.p1), (0, t.p0)] {
                    if self.norm_to_screen(o, q).is_some_and(|s| s.distance(sp) < 10.0) {
                        self.drag = Drag::TransHandle { id: o.id, which };
                        return;
                    }
                }
            }
        }
        let Some(id) = self.hit_test(p).or(self.selected) else {
            self.drag = Drag::None;
            return;
        };
        if self.selected != Some(id) {
            self.select_only(Some(id));
        }
        // nothing changes until the mouse actually moves (a plain click only selects)
        self.drag = match self.doc.find(id).and_then(|o| Self::doc_to_norm(o, p)) {
            Some(q0) => Drag::TransNew { id, q0 },
            None => Drag::None,
        };
    }

    /// Screen position of a distant light's direction handle (the side the light comes from).
    fn distant_handle(&self, o: &Object, l: &Light) -> Pos2 {
        let a = l.angle.to_radians();
        self.to_screen(o.center()) + Vec2::new(a.cos(), -a.sin()) * 70.0
    }

    fn press_light(&mut self, p: Pt, sp: Pos2) {
        if let Some(o) = self.selected.and_then(|id| self.doc.find(id)) {
            let id = o.id;
            for (idx, l) in o.effects.lighting.lights.iter().enumerate() {
                let near = |q: [f32; 2]| self.norm_to_screen(o, q).is_some_and(|s| s.distance(sp) < 11.0);
                let hit = match l.kind {
                    LightKind::Spot if near(l.target) => Some(Drag::LightTarget { id, idx }),
                    LightKind::Point | LightKind::Spot if near(l.pos) => Some(Drag::LightPos { id, idx }),
                    LightKind::Distant if self.distant_handle(o, l).distance(sp) < 11.0 => {
                        Some(Drag::LightDir { id, idx })
                    }
                    _ => None,
                };
                if let Some(d) = hit {
                    self.light_sel = Some(idx);
                    self.drag = d;
                    return;
                }
            }
        }
        // click on an object: put a new light there
        let Some(id) = self.hit_test(p) else {
            self.drag = Drag::None;
            return;
        };
        if self.selected != Some(id) {
            self.select_only(Some(id));
        }
        let Some(q) = self.doc.find(id).and_then(|o| Self::doc_to_norm(o, p)) else { return };
        self.commit_snapshot();
        let kind = self.new_light_kind;
        let mut idx = 0;
        if let Some(o) = self.doc.find_mut(id) {
            let lighting = &mut o.effects.lighting;
            lighting.enabled = true;
            lighting.lights.push(Light::new(kind, q));
            idx = lighting.lights.len() - 1;
            o.touch();
        }
        self.light_sel = Some(idx);
        self.dirty = true;
        self.status = format!("{} added — drag it to move it; its settings are under Lights.", kind.label());
        self.drag = match kind {
            LightKind::Spot => Drag::LightTarget { id, idx },
            LightKind::Point => Drag::LightPos { id, idx },
            LightKind::Distant => Drag::LightDir { id, idx },
        };
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
        if self.tool == Tool::Select {
            if let Some(idx) = self.guide_at(sp) {
                self.drag = Drag::Guide { idx };
                return;
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
        if self.brush.kind == BrushKind::MagicWand {
            self.drag = Drag::None;
            self.magic_wand_at(p);
            return;
        }
        let mask = self.brush.kind.is_mask();
        let target = self
            .selected
            .filter(|id| self.doc.find(*id).map_or(false, |o| o.visible));
        let id = match (target, mask) {
            (Some(id), _) => id,
            // the transparency brushes need an object to work on
            (None, true) => match self.hit_test(p) {
                Some(id) => {
                    self.select_only(Some(id));
                    id
                }
                None => {
                    self.status = "Click on an object to paint its transparency.".into();
                    self.drag = Drag::None;
                    return;
                }
            },
            (None, false) => self.new_paint_layer(),
        };
        self.commit_snapshot();
        let base_r = self.brush_radius_base();
        let settings = self.brush_settings();
        let Some(obj) = self.doc.find_mut(id) else { return };
        if mask {
            ensure_mask(obj);
        } else if obj.paint.is_none() {
            let (w, h) = obj.raster_size();
            obj.paint = Pixmap::new(w, h).map(Arc::new);
        }
        let mapped = if mask { doc_to_mask(obj, p) } else { doc_to_paint(obj, p) };
        let Some((pc, scale)) = mapped else { return };
        let mut state = StrokeState::new();
        if let Some(layer) = brush_layer(obj, mask) {
            brush::segment(Arc::make_mut(layer), pc, pc, base_r * scale, &settings, &mut state);
        }
        obj.touch();
        self.dirty = true;
        self.drag = Drag::Paint { id, last: pc, state, mask };
    }

    /// Brush settings for the next stamp: the transparency brushes paint black / white
    /// into the mask with a plain round tip.
    fn brush_settings(&self) -> BrushSettings {
        let mut s = self.brush.clone();
        s.color = match s.kind {
            BrushKind::Hide => [0, 0, 0, 255],
            BrushKind::Reveal => [255, 255, 255, 255],
            _ => self.color_a,
        };
        if s.kind.is_mask() {
            s.kind = BrushKind::Round;
        }
        s
    }

    /// The picture (or paint layer) the magic wand / background removal works on.
    fn picture_target(&self, p: Option<Pt>) -> Option<u64> {
        let has_paint = |id: u64| self.doc.find(id).is_some_and(|o| o.paint.is_some());
        p.and_then(|p| self.hit_test(p))
            .filter(|id| has_paint(*id))
            .or(self.selected.filter(|id| has_paint(*id)))
    }

    fn magic_wand_at(&mut self, p: Pt) {
        let Some(id) = self.picture_target(Some(p)) else {
            self.status = "The magic wand works on pictures and paint layers — click on one.".into();
            return;
        };
        self.select_only(Some(id));
        let (tol, feather) = (self.bg_tol, self.bg_feather);
        self.checkpoint();
        let mut removed = 0;
        if let Some(o) = self.doc.find_mut(id) {
            ensure_mask(o);
            if let (Some((pc, _)), Some(img)) = (doc_to_paint(o, p), o.paint.clone()) {
                if let Some(mask) = o.mask.as_mut() {
                    let m = Arc::make_mut(mask);
                    if m.width() == img.width() && m.height() == img.height() {
                        removed = imageops::magic_wand(&img, m, pc.0 as usize, pc.1 as usize, tol, feather);
                    }
                }
            }
            o.touch();
        }
        self.dirty = true;
        self.status = if removed > 0 {
            format!("Hid {removed} pixels. Raise Tolerance to take more, or paint them back with Transparency: reveal.")
        } else {
            "Nothing hidden there.".into()
        };
    }

    /// Send the picture to the AI worker; the result arrives in `poll_ai`.
    fn start_ai_removal(&mut self, id: u64) {
        if self.ai_busy.is_some() {
            return;
        }
        let Some(img) = self.doc.find(id).and_then(|o| o.paint.clone()) else {
            self.status = "AI background removal works on pictures — import one first.".into();
            return;
        };
        self.ai.submit(AiJob {
            id,
            model: self.ai_model,
            img,
        });
        self.ai_busy = Some(id);
        self.status = "AI is looking for the subject…".into();
    }

    fn poll_ai(&mut self) {
        while let Ok(ev) = self.ai.rx.try_recv() {
            self.ai_busy = None;
            match ev {
                ai::AiEvent::Mask { id, mask } => {
                    let fits = self.doc.find(id).and_then(|o| o.paint.as_ref()).is_some_and(|p| {
                        p.width() == mask.width() && p.height() == mask.height()
                    });
                    if !fits {
                        continue; // picture deleted or changed meanwhile
                    }
                    self.checkpoint();
                    if let Some(o) = self.doc.find_mut(id) {
                        o.mask = Some(Arc::new(mask));
                        o.touch();
                    }
                    self.dirty = true;
                    self.status =
                        "Background removed. Touch up with the Brush tool's Transparency: hide / reveal.".into();
                }
                ai::AiEvent::Error(e) => self.status = format!("AI background removal failed: {e}"),
            }
        }
    }

    fn remove_background(&mut self, id: u64) {
        let (tol, feather) = (self.bg_tol, self.bg_feather);
        let mut removed = 0;
        if let Some(o) = self.doc.find_mut(id) {
            let Some(img) = o.paint.clone() else { return };
            ensure_mask(o);
            if let Some(mask) = o.mask.as_mut() {
                let m = Arc::make_mut(mask);
                if m.width() == img.width() && m.height() == img.height() {
                    removed = imageops::remove_background(&img, m, tol, feather);
                }
            }
            o.touch();
        }
        self.dirty = true;
        self.status = format!(
            "Background removed ({removed} pixels hidden). Touch up with the Transparency hide / reveal brushes."
        );
    }

    // ---------------------------------------------------------------- snapping & arranging

    fn snapping(&self, mods: Modifiers) -> bool {
        (self.settings.snap || self.settings.grid) && !mods.alt
    }

    /// Lines things can snap to: canvas edges and centre, guides, other objects' edges and
    /// centres (objects in `exclude` are the ones being moved).
    fn snap_targets(&self, exclude: &[u64]) -> (Vec<f32>, Vec<f32>) {
        let (w, h) = (self.doc.width as f32, self.doc.height as f32);
        let mut xs = Vec::new();
        let mut ys = Vec::new();
        if self.settings.snap {
            xs.extend([0.0, w * 0.5, w]);
            ys.extend([0.0, h * 0.5, h]);
            for g in &self.doc.guides {
                if g.vertical {
                    xs.push(g.pos)
                } else {
                    ys.push(g.pos)
                }
            }
            for o in self.doc.objects.iter().filter(|o| o.visible && !exclude.contains(&o.id)) {
                if o.kind == ShapeKind::Bitmap && o.size[0] >= w - 1.0 && o.size[1] >= h - 1.0 {
                    continue; // full-page paint layers would only snap to the page again
                }
                let (lo, hi) = o.bounds();
                xs.extend([lo.x, (lo.x + hi.x) * 0.5, hi.x]);
                ys.extend([lo.y, (lo.y + hi.y) * 0.5, hi.y]);
            }
        }
        (xs, ys)
    }

    fn grid_shift(&self, v: f32, tol: f32) -> Option<f32> {
        if !self.settings.grid {
            return None;
        }
        let g = self.settings.grid_size.max(1.0);
        let s = (v / g).round() * g - v;
        (s.abs() <= tol).then_some(s)
    }

    /// Shift for a box (lo..hi) so one of its edges or its centre lands on a snap line.
    fn snap_box(&mut self, lo: Pt, hi: Pt, exclude: &[u64]) -> Pt {
        let (xs, ys) = self.snap_targets(exclude);
        let tol = 7.0 / self.zoom;
        self.snap_lines.clear();
        let mut adj = Pt::default();
        match canvas::snap_shift(&[lo.x, (lo.x + hi.x) * 0.5, hi.x], &xs, tol) {
            Some((s, t)) => {
                adj.x = s;
                self.snap_lines.push((true, t));
            }
            None => adj.x = self.grid_shift(lo.x, tol).unwrap_or(0.0),
        }
        match canvas::snap_shift(&[lo.y, (lo.y + hi.y) * 0.5, hi.y], &ys, tol) {
            Some((s, t)) => {
                adj.y = s;
                self.snap_lines.push((false, t));
            }
            None => adj.y = self.grid_shift(lo.y, tol).unwrap_or(0.0),
        }
        adj
    }

    fn snap_point(&mut self, p: Pt, exclude: &[u64]) -> Pt {
        let adj = self.snap_box(p, p, exclude);
        p + adj
    }

    /// Align the selection: to each other, or to the canvas when one object is selected.
    fn align_selected(&mut self, how: AlignTo) {
        let ids = self.selection();
        if ids.is_empty() {
            return;
        }
        let bounds: Vec<(u64, Pt, Pt)> = ids
            .iter()
            .filter_map(|id| self.doc.find(*id).map(|o| (*id, o.bounds().0, o.bounds().1)))
            .collect();
        let (rlo, rhi) = if bounds.len() == 1 {
            (Pt::default(), Pt::new(self.doc.width as f32, self.doc.height as f32))
        } else {
            bounds.iter().fold((Pt::new(f32::MAX, f32::MAX), Pt::new(f32::MIN, f32::MIN)), |(a, b), (_, lo, hi)| {
                (Pt::new(a.x.min(lo.x), a.y.min(lo.y)), Pt::new(b.x.max(hi.x), b.y.max(hi.y)))
            })
        };
        self.checkpoint();
        for (id, lo, hi) in bounds {
            let d = match how {
                AlignTo::Left => Pt::new(rlo.x - lo.x, 0.0),
                AlignTo::CenterH => Pt::new((rlo.x + rhi.x - lo.x - hi.x) * 0.5, 0.0),
                AlignTo::Right => Pt::new(rhi.x - hi.x, 0.0),
                AlignTo::Top => Pt::new(0.0, rlo.y - lo.y),
                AlignTo::Middle => Pt::new(0.0, (rlo.y + rhi.y - lo.y - hi.y) * 0.5),
                AlignTo::Bottom => Pt::new(0.0, rhi.y - hi.y),
            };
            if let Some(o) = self.doc.find_mut(id) {
                o.quad = o.quad.map(|c| c + d);
            }
        }
        self.dirty = true;
    }

    /// Equal gaps between three or more selected objects.
    fn distribute_selected(&mut self, horizontal: bool) {
        let mut items: Vec<(u64, Pt, Pt)> = self
            .selection()
            .iter()
            .filter_map(|id| self.doc.find(*id).map(|o| (*id, o.bounds().0, o.bounds().1)))
            .collect();
        if items.len() < 3 {
            self.status = "Select three or more objects to distribute them.".into();
            return;
        }
        let key = |lo: Pt, hi: Pt| if horizontal { lo.x + hi.x } else { lo.y + hi.y };
        items.sort_by(|a, b| key(a.1, a.2).total_cmp(&key(b.1, b.2)));
        let size = |lo: Pt, hi: Pt| if horizontal { hi.x - lo.x } else { hi.y - lo.y };
        let start = if horizontal { items[0].1.x } else { items[0].1.y };
        let end = items.iter().map(|(_, lo, hi)| if horizontal { hi.x.max(lo.x) } else { hi.y.max(lo.y) }).fold(f32::MIN, f32::max);
        let total: f32 = items.iter().map(|(_, lo, hi)| size(*lo, *hi)).sum();
        let gap = (end - start - total) / (items.len() - 1) as f32;
        self.checkpoint();
        let mut cursor = start;
        for (id, lo, hi) in items {
            let from = if horizontal { lo.x } else { lo.y };
            let d = cursor - from;
            if let Some(o) = self.doc.find_mut(id) {
                o.quad = o.quad.map(|c| if horizontal { c + Pt::new(d, 0.0) } else { c + Pt::new(0.0, d) });
            }
            cursor += size(lo, hi) + gap;
        }
        self.dirty = true;
    }

    // ---------------------------------------------------------------- clipboard

    /// Copy the selection: objects go to the internal clipboard (pasted back as editable
    /// objects); other programs get a picture of them on the system clipboard.
    fn copy_selected(&mut self) -> bool {
        let ids = self.selection();
        if ids.is_empty() {
            return false;
        }
        self.clipboard = ids.iter().filter_map(|id| self.doc.find(*id).cloned()).collect();
        self.paste_count = 0;
        if self.system_clipboard {
            if let Some(img) = self.selection_image() {
                self.clip_image_sig = Some(image_signature(&img));
                let _ = set_clipboard_image(&img);
            }
        }
        self.status = format!("Copied {} object(s)", self.clipboard.len());
        true
    }

    fn cut_selected(&mut self) {
        if self.copy_selected() {
            self.delete_selected();
            self.status = format!("Cut {} object(s)", self.clipboard.len());
        }
    }

    /// The selected objects rendered on a transparent background, cropped to their extent.
    fn selection_image(&self) -> Option<Pixmap> {
        let objs: Vec<Object> = self.selection().iter().filter_map(|id| self.doc.find(*id).cloned()).collect();
        let mut tmp = Document::new(1, 1);
        tmp.objects = objs;
        let (lo, hi) = canvas::content_bounds(&tmp)?;
        tmp.width = ((hi.x - lo.x).ceil() as u32 + 2).clamp(1, canvas::MAX_SIZE);
        tmp.height = ((hi.y - lo.y).ceil() as u32 + 2).clamp(1, canvas::MAX_SIZE);
        for o in &mut tmp.objects {
            o.quad = o.quad.map(|c| c - lo + Pt::new(1.0, 1.0));
        }
        Some(Renderer::default().render_opts(&tmp, false))
    }

    /// Paste: text from other programs becomes a text object, pictures become picture objects,
    /// and what was copied here comes back as editable objects.
    fn paste(&mut self, text: Option<String>) {
        if let Some(t) = text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
            self.paste_text(&t);
            return;
        }
        if self.system_clipboard {
            if let Some(img) = get_clipboard_image() {
                if self.clip_image_sig != Some(image_signature(&img)) || self.clipboard.is_empty() {
                    let at = self.view_center();
                    self.add_picture_object(img, "Pasted picture".into(), Some(at));
                    return;
                }
            }
        }
        self.paste_objects();
    }

    fn paste_objects(&mut self) {
        if self.clipboard.is_empty() {
            self.status = "Nothing to paste".into();
            return;
        }
        self.checkpoint();
        self.paste_count += 1;
        let off = Pt::new(20.0, 20.0) * self.paste_count as f32;
        let mut new_ids = Vec::new();
        for src in self.clipboard.clone() {
            let id = self.doc.alloc_id();
            let mut o = src;
            o.id = id;
            o.quad = o.quad.map(|c| c + off);
            o.touch();
            self.doc.objects.push(o);
            new_ids.push(id);
        }
        let primary = new_ids.pop();
        self.select_only(primary);
        self.multi = new_ids;
        self.dirty = true;
        self.status = format!("Pasted {} object(s)", self.selection().len());
    }

    fn paste_text(&mut self, text: &str) {
        let text: String = text.chars().take(400).collect();
        let font = self.text_font;
        let aspect = shapes::text_aspect(&text, font).unwrap_or(3.0);
        let h = (60.0 * text.lines().count().max(1) as f32).min(400.0);
        let w = (h * aspect).min(self.doc.width as f32 * 0.9);
        let c = self.view_center();
        self.checkpoint();
        let id = self.doc.alloc_id();
        let mut o = Object::new(id, ShapeKind::Text { text, font }, c.x - w / 2.0, c.y - h / 2.0, w, h);
        o.fill = FillStyle {
            kind: FillKind::Solid,
            a: self.color_a,
            ..Default::default()
        };
        o.fit_text_width();
        self.doc.objects.push(o);
        self.select_only(Some(id));
        self.dirty = true;
        self.status = "Pasted text as a text object".into();
    }

    /// Canvas point in the middle of the visible area (clamped to the page).
    fn view_center(&self) -> Pt {
        let c = self.to_doc(self.canvas_rect.center());
        Pt::new(
            c.x.clamp(0.0, self.doc.width as f32),
            c.y.clamp(0.0, self.doc.height as f32),
        )
    }

    /// Import a picture as a new object, scaled to fit the page (`at` = where to centre it).
    fn import_image_object(&mut self, path: &std::path::Path, at: Option<Pt>) {
        let img = match imageops::load_image(path) {
            Ok(i) => i,
            Err(e) => {
                self.status = format!("Could not import {}: {e}", path.display());
                return;
            }
        };
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Picture".into());
        self.add_picture_object(img, name, at);
    }

    fn add_picture_object(&mut self, img: Pixmap, name: String, at: Option<Pt>) {
        let (iw, ih) = (img.width() as f32, img.height() as f32);
        let (dw, dh) = (self.doc.width as f32, self.doc.height as f32);
        let k = (0.8 * dw / iw).min(0.8 * dh / ih).min(1.0);
        let (w, h) = ((iw * k).max(1.0), (ih * k).max(1.0));
        let c = at.unwrap_or(Pt::new(dw * 0.5, dh * 0.5));
        self.checkpoint();
        let id = self.doc.alloc_id();
        let mut obj = Object::new(id, ShapeKind::Bitmap, c.x - w * 0.5, c.y - h * 0.5, w, h);
        obj.fill.kind = FillKind::None;
        let img = Arc::new(img);
        obj.original = Some(ImageData(img.clone()));
        obj.paint = Some(img);
        self.status = format!(
            "Added {name} ({}×{} px). Remove its background in the properties panel.",
            iw as u32,
            ih as u32
        );
        obj.name = name;
        self.doc.objects.push(obj);
        self.select_only(Some(id));
        self.set_tool(Tool::Select);
        self.dirty = true;
    }

    fn import_image_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Pictures", &imageops::IMAGE_EXTENSIONS)
            .pick_file()
        {
            self.import_image_object(&path, None);
        }
    }

    fn pick_image_file(&mut self) -> Option<ImageData> {
        let path = rfd::FileDialog::new()
            .add_filter("Pictures", &imageops::IMAGE_EXTENSIONS)
            .pick_file()?;
        match imageops::load_image(&path) {
            Ok(img) => Some(ImageData(Arc::new(img))),
            Err(e) => {
                self.status = format!("Could not load {}: {e}", path.display());
                None
            }
        }
    }

    /// Files dropped onto the window: pictures become objects where they land, .jabro files open.
    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        let at = ctx
            .input(|i| i.pointer.latest_pos())
            .filter(|p| self.canvas_rect.contains(*p))
            .map(|p| self.to_doc(p));
        for f in dropped {
            let Some(path) = f.path else { continue };
            if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("jabro")) {
                self.open_path(path);
            } else if imageops::is_image_path(&path) {
                self.import_image_object(&path, at);
            } else {
                self.status = format!("Can't import {}", path.display());
            }
        }
    }

    fn on_drag(&mut self, p: Pt, mods: Modifiers) {
        if self.last_drag_pos == Some(p) {
            return;
        }
        self.last_drag_pos = Some(p);
        // drawing / reshaping snaps the pointer itself; moving snaps the whole selection below
        let snap_pointer = match &self.drag {
            Drag::Scale { id, .. } | Drag::Corner { id, .. } | Drag::Handle { id, .. } => Some(vec![*id]),
            Drag::Create { .. } => Some(Vec::new()),
            _ => None,
        };
        self.snap_lines.clear();
        let p = match snap_pointer {
            Some(excl) if self.snapping(mods) => self.snap_point(p, &excl),
            _ => p,
        };
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        self.drag = match drag {
            Drag::Guide { idx } => {
                self.commit_snapshot();
                if let Some(g) = self.doc.guides.get_mut(idx) {
                    g.pos = if g.vertical { p.x } else { p.y }.round();
                }
                Drag::Guide { idx }
            }
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
                if self.snapping(mods) {
                    let ids: Vec<u64> = items.iter().map(|(i, _)| *i).collect();
                    let (mut lo, mut hi) = (Pt::new(f32::MAX, f32::MAX), Pt::new(f32::MIN, f32::MIN));
                    for (_, q) in &items {
                        for c in q {
                            lo = Pt::new(lo.x.min(c.x + d.x), lo.y.min(c.y + d.y));
                            hi = Pt::new(hi.x.max(c.x + d.x), hi.y.max(c.y + d.y));
                        }
                    }
                    let adj = self.snap_box(lo, hi, &ids);
                    d = d + adj;
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
            Drag::TransNew { id, q0 } => {
                let start_screen = self.doc.find(id).and_then(|o| self.norm_to_screen(o, q0));
                let moved = start_screen.is_some_and(|s| s.distance(self.to_screen(p)) > 4.0);
                if moved {
                    self.commit_snapshot();
                    let kind = self.trans_kind;
                    if let Some(o) = self.doc.find_mut(id) {
                        if let Some(q) = Self::doc_to_norm(o, p) {
                            let t = &mut o.transparency;
                            if !matches!(t.kind, TransparencyKind::Linear | TransparencyKind::Radial) {
                                t.kind = kind;
                                if t.start <= t.end + 0.05 {
                                    t.start = 1.0;
                                    t.end = 0.0;
                                }
                            }
                            t.p0 = q0;
                            t.p1 = q;
                            o.touch();
                        }
                    }
                    self.dirty = true;
                }
                Drag::TransNew { id, q0 }
            }
            Drag::TransHandle { id, which } => {
                self.commit_snapshot();
                if let Some(o) = self.doc.find_mut(id) {
                    if let Some(q) = Self::doc_to_norm(o, p) {
                        if which == 0 {
                            o.transparency.p0 = q;
                        } else {
                            o.transparency.p1 = q;
                        }
                        o.touch();
                    }
                }
                self.dirty = true;
                Drag::TransHandle { id, which }
            }
            Drag::LightPos { id, idx } | Drag::LightTarget { id, idx } => {
                let target = matches!(drag, Drag::LightTarget { .. });
                self.commit_snapshot();
                if let Some(o) = self.doc.find_mut(id) {
                    if let Some(q) = Self::doc_to_norm(o, p) {
                        if let Some(l) = o.effects.lighting.lights.get_mut(idx) {
                            if target {
                                l.target = q;
                            } else {
                                // a spotlight keeps pointing at the same spot while it moves
                                l.pos = q;
                            }
                        }
                        o.touch();
                    }
                }
                self.dirty = true;
                drag
            }
            Drag::LightDir { id, idx } => {
                self.commit_snapshot();
                if let Some(o) = self.doc.find_mut(id) {
                    let v = p - o.center();
                    if v.len() > 1.0 {
                        if let Some(l) = o.effects.lighting.lights.get_mut(idx) {
                            l.angle = (-v.y).atan2(v.x).to_degrees().rem_euclid(360.0);
                        }
                        o.touch();
                    }
                }
                self.dirty = true;
                Drag::LightDir { id, idx }
            }
            Drag::Node { id, sp, ni, offset } => {
                self.commit_snapshot();
                let target = if self.snapping(mods) { self.snap_point(p + offset, &[id]) } else { p + offset };
                if let Some(o) = self.doc.find_mut(id) {
                    if let Some(q) = Self::doc_to_norm(o, target) {
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
            Drag::Paint { id, last, mut state, mask } => {
                let base_r = self.brush_radius_base();
                let settings = self.brush_settings();
                let mut new_last = last;
                if let Some(obj) = self.doc.find_mut(id) {
                    let mapped = if mask { doc_to_mask(obj, p) } else { doc_to_paint(obj, p) };
                    if let Some((pc, scale)) = mapped {
                        if let Some(layer) = brush_layer(obj, mask) {
                            brush::segment(Arc::make_mut(layer), last, pc, base_r * scale, &settings, &mut state);
                        }
                        obj.touch();
                        new_last = pc;
                    }
                }
                self.dirty = true;
                Drag::Paint { id, last: new_last, state, mask }
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
        if self.settings.auto_grow {
            let sticks_out = canvas::content_bounds(&self.doc).is_some_and(|(lo, hi)| {
                canvas::grow_to_fit(self.doc.width, self.doc.height, lo, hi, 8.0).is_some()
            });
            if sticks_out {
                self.commit_snapshot(); // part of the same undo step as the drop
                self.auto_grow();
            }
        }
        self.snap_lines.clear();
        self.snapshot = None;
        self.last_drag_pos = None;
    }

    const RULER: f32 = 20.0;

    fn ruler_rects(&self) -> Option<(Rect, Rect)> {
        if !self.settings.rulers {
            return None;
        }
        let r = self.canvas_rect;
        Some((
            Rect::from_min_max(r.left_top(), Pos2::new(r.right(), r.top() + Self::RULER)),
            Rect::from_min_max(r.left_top(), Pos2::new(r.left() + Self::RULER, r.bottom())),
        ))
    }

    /// Pressing on a ruler pulls out a new guide (top ruler: horizontal, left: vertical).
    fn press_ruler(&mut self, sp: Pos2) -> bool {
        let Some((top, left)) = self.ruler_rects() else { return false };
        let vertical = if left.contains(sp) && !top.contains(sp) {
            true
        } else if top.contains(sp) {
            false
        } else {
            return false;
        };
        let d = self.to_doc(sp);
        self.checkpoint();
        self.doc.guides.push(Guide {
            vertical,
            pos: if vertical { d.x } else { d.y }.round(),
        });
        self.drag = Drag::Guide { idx: self.doc.guides.len() - 1 };
        self.status = "Drag the guide onto the canvas; drop it back on the ruler to remove it.".into();
        true
    }

    fn guide_at(&self, sp: Pos2) -> Option<usize> {
        let o = self.origin();
        self.doc.guides.iter().position(|g| {
            if g.vertical {
                (o.x + g.pos * self.zoom - sp.x).abs() < 4.0
            } else {
                (o.y + g.pos * self.zoom - sp.y).abs() < 4.0
            }
        })
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

    /// Drop `moving` (ids, in list order) into the object list before display row `gap`
    /// (row 0 is the topmost object). One undo step; nothing happens if the order is unchanged.
    fn reorder_display(&mut self, moving: &[u64], gap: usize) {
        let display: Vec<u64> = self.doc.objects.iter().rev().map(|o| o.id).collect();
        let new_display = reorder_ids(&display, moving, gap);
        if new_display == display {
            return;
        }
        self.checkpoint();
        let mut pool: std::collections::HashMap<u64, Object> =
            std::mem::take(&mut self.doc.objects).into_iter().map(|o| (o.id, o)).collect();
        self.doc.objects = new_display.iter().rev().filter_map(|id| pool.remove(id)).collect();
        self.dirty = true;
        self.status = match moving.len() {
            1 => "Moved in the stacking order".into(),
            n => format!("Moved {n} objects in the stacking order"),
        };
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
        if cmd(Key::I) {
            self.import_image_dialog();
        }
        if cmd(Key::N) {
            self.open_canvas_dialog(true);
        }
        // Ctrl+V with only a picture on the clipboard produces no egui event: watch the keys.
        let v_down = ctrl_v_down() && ctx.input(|i| i.focused);
        let v_pressed = v_down && !self.ctrl_v_was_down;
        self.ctrl_v_was_down = v_down;
        if ctx.wants_keyboard_input() {
            return;
        }
        let clip_events: Vec<egui::Event> = ctx.input(|i| {
            i.events
                .iter()
                .filter(|e| matches!(e, egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_)))
                .cloned()
                .collect()
        });
        let mut pasted = false;
        for e in clip_events {
            match e {
                egui::Event::Copy => {
                    self.copy_selected();
                }
                egui::Event::Cut => self.cut_selected(),
                egui::Event::Paste(text) => {
                    pasted = true;
                    self.paste(Some(text));
                }
                _ => {}
            }
        }
        if v_pressed && !pasted {
            self.paste(None);
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
                if ui.button("New…  (Ctrl+N)").clicked() {
                    self.open_canvas_dialog(true);
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
                if ui.button("Import picture…  (Ctrl+I)").clicked() {
                    ui.close_menu();
                    self.import_image_dialog();
                }
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
                let has_sel = self.selected.is_some();
                if ui.add_enabled(has_sel, egui::Button::new("Cut  (Ctrl+X)")).clicked() {
                    self.cut_selected();
                    ui.close_menu();
                }
                if ui.add_enabled(has_sel, egui::Button::new("Copy  (Ctrl+C)")).clicked() {
                    self.copy_selected();
                    ui.close_menu();
                }
                if ui.button("Paste  (Ctrl+V)").clicked() {
                    let text = if self.system_clipboard {
                        arboard::Clipboard::new().ok().and_then(|mut c| c.get_text().ok())
                    } else {
                        None
                    };
                    // our own copy, a picture, or text from another program
                    let own = self.clip_image_sig.is_some()
                        && get_clipboard_image().is_some_and(|i| Some(image_signature(&i)) == self.clip_image_sig);
                    self.paste(if own { None } else { text });
                    ui.close_menu();
                }
                ui.separator();
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
            ui.menu_button("Canvas", |ui| {
                if ui.button("Canvas size…").clicked() {
                    self.open_canvas_dialog(false);
                    ui.close_menu();
                }
                if ui.button("Fit canvas to content").clicked() {
                    self.checkpoint();
                    if self.fit_canvas_to_content() {
                        self.fit_pending = true;
                    }
                    ui.close_menu();
                }
                let mut g = self.settings.auto_grow;
                if ui.checkbox(&mut g, "Grow automatically past the edge").changed() {
                    self.settings.auto_grow = g;
                    self.settings.save();
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
                ui.separator();
                let before = self.settings.clone();
                ui.checkbox(&mut self.settings.snap, "Snap to objects, canvas and guides")
                    .on_hover_text("Hold Alt while dragging to place freely");
                ui.checkbox(&mut self.settings.grid, "Grid (show and snap)");
                if self.settings.grid {
                    ui.add(egui::Slider::new(&mut self.settings.grid_size, 2.0..=200.0).logarithmic(true).text("Grid size"));
                }
                ui.checkbox(&mut self.settings.rulers, "Rulers")
                    .on_hover_text("Drag from a ruler to make a guide; drop it back to remove it");
                if ui.add_enabled(!self.doc.guides.is_empty(), egui::Button::new("Remove all guides")).clicked() {
                    self.checkpoint();
                    self.doc.guides.clear();
                    ui.close_menu();
                }
                if before.snap != self.settings.snap
                    || before.grid != self.settings.grid
                    || before.grid_size != self.settings.grid_size
                    || before.rulers != self.settings.rulers
                {
                    self.settings.save();
                }
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
            if ui
                .link(format!(
                    "Canvas {} × {} ({})",
                    self.doc.width,
                    self.doc.height,
                    canvas::aspect_label(self.doc.width, self.doc.height)
                ))
                .on_hover_text("Change the canvas size")
                .clicked()
            {
                self.open_canvas_dialog(false);
            }
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
                ui.small("Right-click closes the shape (filled with the first colour). Enter leaves it open as a stroke of this width.");
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
            Tool::Transparency => {
                ui.strong("Transparency");
                ui.label("Dragging creates a:");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.trans_kind, TransparencyKind::Linear, "Linear fade");
                    ui.radio_value(&mut self.trans_kind, TransparencyKind::Radial, "Radial fade");
                });
                let has_trans = self
                    .selected
                    .and_then(|id| self.doc.find(id))
                    .is_some_and(|o| o.transparency.kind != TransparencyKind::None);
                if has_trans && ui.button("Remove transparency").clicked() {
                    self.apply_to_selected(|o| o.transparency.kind = TransparencyKind::None);
                }
                ui.small("Opacity values, flat and texture transparency are in the properties panel.");
            }
            Tool::Light => {
                ui.strong("Lights");
                ui.label("A click on an object adds a:");
                for k in LightKind::ALL {
                    ui.radio_value(&mut self.new_light_kind, k, k.label());
                }
                let n = self
                    .selected
                    .and_then(|id| self.doc.find(id))
                    .map_or(0, |o| o.effects.lighting.lights.len());
                if let Some(idx) = self.light_sel.filter(|i| *i < n) {
                    if ui.button(format!("Remove light {}", idx + 1)).clicked() {
                        self.apply_to_selected(move |o| {
                            o.effects.lighting.lights.remove(idx);
                        });
                        self.light_sel = None;
                    }
                }
                ui.small("Lights belong to their object and move with it. Ambient light, colour and strength are under Lights in the properties panel.");
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
                if self.brush.kind == BrushKind::MagicWand {
                    ui.add(egui::Slider::new(&mut self.bg_tol, 0.01..=0.6).text("Tolerance"));
                    ui.add(egui::Slider::new(&mut self.bg_feather, 0.0..=8.0).text("Soft edge"));
                    ui.small("Click a picture: the connected area of similar colour becomes transparent. Nothing is deleted — paint it back with Transparency: reveal.");
                } else {
                    ui.add(egui::Slider::new(&mut self.brush.size, 1.0..=300.0).logarithmic(true).text("Size"));
                    ui.add(egui::Slider::new(&mut self.brush.hardness, 0.0..=1.0).text("Hardness"));
                    ui.add(egui::Slider::new(&mut self.brush.flow, 0.02..=1.0).text("Flow"));
                    ui.add(egui::Slider::new(&mut self.brush.spacing, 0.02..=1.0).text("Spacing"));
                    ui.checkbox(&mut self.brush.pressure, "Pen pressure");
                }
                let has_work = self
                    .selected
                    .and_then(|id| self.doc.find(id))
                    .is_some_and(|o| o.mask.is_some() || o.paint.is_some() && o.original.as_ref().is_none_or(|orig| !o.paint.as_ref().is_some_and(|p| Arc::ptr_eq(p, &orig.0))));
                if has_work
                    && ui
                        .button("Reset brush work")
                        .on_hover_text("Back to the original: removes painting and painted transparency on the selected object")
                        .clicked()
                {
                    self.apply_to_selected(reset_brush_work);
                    self.status = "Brush work reset — back to the original.".into();
                }
                if self.brush.kind.is_mask() {
                    ui.small("Paints the selected object's transparency: hide makes it see-through, reveal brings it back. Works on pictures, shapes and text. Low flow = partly transparent.");
                } else if self.brush.kind != BrushKind::MagicWand {
                    if ui.button("New paint layer").clicked() {
                        self.checkpoint();
                        self.new_paint_layer();
                        self.dirty = true;
                    }
                    ui.small("Paints into the selected object (clipped to its shape). With nothing selected a new paint layer is created. Alt+click = colour picker.");
                }
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
            DragStart(u64),
        }
        let mut action = None;
        let dragging = self.list_drag.clone();
        // (object id, row rect) top to bottom, as displayed
        let mut rows: Vec<(u64, Rect)> = Vec::new();
        let out = egui::ScrollArea::vertical()
            .id_salt("objlist")
            .max_height(220.0)
            .show(ui, |ui| {
                for obj in self.doc.objects.iter().rev() {
                    let moving = dragging.as_ref().is_some_and(|d| d.contains(&obj.id));
                    let row = ui.horizontal(|ui| {
                        let mut vis = obj.visible;
                        if ui.checkbox(&mut vis, "").on_hover_text("Visible").changed() {
                            action = Some(A::Vis(obj.id));
                        }
                        let sel = self.selected == Some(obj.id) || self.multi.contains(&obj.id);
                        let text = if moving {
                            egui::RichText::new(obj.name.as_str()).weak().italics()
                        } else {
                            egui::RichText::new(obj.name.as_str())
                        };
                        let r = ui
                            .add(egui::SelectableLabel::new(sel, text))
                            .interact(Sense::click_and_drag())
                            .on_hover_text("Drag to change the stacking order · Shift+click to add to the selection");
                        if r.clicked() {
                            action = Some(A::Sel(obj.id, ui.input(|i| i.modifiers.shift)));
                        }
                        if r.drag_started() {
                            action = Some(A::DragStart(obj.id));
                        }
                        if r.hovered() || r.dragged() {
                            ui.ctx().set_cursor_icon(if r.dragged() {
                                egui::CursorIcon::Grabbing
                            } else {
                                egui::CursorIcon::Grab
                            });
                        }
                    });
                    rows.push((obj.id, row.response.rect));
                }
                // scroll the list when dragging close to its top or bottom edge
                if dragging.is_some() {
                    if let Some(p) = ui.ctx().pointer_latest_pos() {
                        let clip = ui.clip_rect();
                        if p.y < clip.top() + 18.0 {
                            ui.scroll_with_delta(Vec2::new(0.0, 6.0));
                        } else if p.y > clip.bottom() - 18.0 {
                            ui.scroll_with_delta(Vec2::new(0.0, -6.0));
                        }
                    }
                }
            });
        let list_rect = out.inner_rect;
        if let Some(A::DragStart(id)) = action {
            // dragging a selected object carries the whole selection along
            let moving: Vec<u64> = if self.is_selected(id) {
                rows.iter().map(|(i, _)| *i).filter(|i| self.is_selected(*i)).collect()
            } else {
                vec![id]
            };
            self.list_drag = Some(moving);
            action = None;
        }
        if let Some(moving) = self.list_drag.clone() {
            let (pos, down) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.primary_down()));
            let gap = pos.map(|p| {
                rows.iter()
                    .position(|(_, r)| p.y < r.center().y)
                    .unwrap_or(rows.len())
            });
            let over_list = pos.is_some_and(|p| list_rect.expand2(Vec2::new(24.0, 30.0)).contains(p));
            if let (Some(g), true) = (gap, over_list) {
                let y = match rows.get(g) {
                    Some((_, r)) => r.top() - 1.0,
                    None => rows.last().map_or(list_rect.top(), |(_, r)| r.bottom() + 1.0),
                };
                let y = y.clamp(list_rect.top(), list_rect.bottom());
                let accent = Color32::from_rgb(0, 170, 255);
                let painter = ui.painter_at(list_rect.expand(3.0));
                painter.line_segment(
                    [Pos2::new(list_rect.left() + 2.0, y), Pos2::new(list_rect.right() - 2.0, y)],
                    Stroke::new(2.5_f32, accent),
                );
                painter.circle_filled(Pos2::new(list_rect.left() + 3.0, y), 3.5, accent);
            }
            if let Some(p) = pos {
                let label = match moving.as_slice() {
                    [one] => self.doc.find(*one).map(|o| o.name.clone()).unwrap_or_default(),
                    many => format!("{} objects", many.len()),
                };
                let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("objlist_ghost"));
                let painter = ui.ctx().layer_painter(layer);
                let galley = painter.layout_no_wrap(label, egui::FontId::proportional(13.0), Color32::WHITE);
                let r = Rect::from_min_size(p + Vec2::new(14.0, 6.0), galley.size()).expand(4.0);
                painter.rect_filled(r, 4.0, Color32::from_rgba_unmultiplied(0, 120, 200, 220));
                painter.galley(r.min + Vec2::splat(4.0), galley, Color32::WHITE);
            }
            if !down {
                if let (Some(g), true) = (gap, over_list) {
                    self.reorder_display(&moving, g);
                }
                self.list_drag = None;
            }
        }
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
            Some(A::DragStart(_)) | None => {}
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
        if count >= 1 {
            ui.strong(if count == 1 { "Arrange (to the canvas)".to_string() } else { format!("Arrange {count} objects") });
            let mut align = None;
            let mut dist = None;
            ui.horizontal_wrapped(|ui| {
                for (label, a, tip) in [
                    ("Left", AlignTo::Left, "Align left edges"),
                    ("Centre", AlignTo::CenterH, "Centre horizontally"),
                    ("Right", AlignTo::Right, "Align right edges"),
                    ("Top", AlignTo::Top, "Align top edges"),
                    ("Middle", AlignTo::Middle, "Centre vertically"),
                    ("Bottom", AlignTo::Bottom, "Align bottom edges"),
                ] {
                    if ui.small_button(label).on_hover_text(tip).clicked() {
                        align = Some(a);
                    }
                }
                if count >= 3 {
                    if ui.small_button("Space ↔").on_hover_text("Equal horizontal gaps").clicked() {
                        dist = Some(true);
                    }
                    if ui.small_button("Space ↕").on_hover_text("Equal vertical gaps").clicked() {
                        dist = Some(false);
                    }
                }
            });
            if let Some(a) = align {
                self.align_selected(a);
            }
            if let Some(h) = dist {
                self.distribute_selected(h);
            }
            ui.separator();
        }
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
        let mut doc_act: Option<DocAction> = None;
        let auto_grow = self.settings.auto_grow;
        let has_clip = self.style_clipboard.is_some();
        let selected = self.selected;
        let focus_text = std::mem::take(&mut self.focus_text);
        let mut edit_nodes = false;
        let mut pic = PictureUi {
            tol: self.bg_tol,
            feather: self.bg_feather,
            ai_model: self.ai_model,
            ai_busy: self.ai_busy.is_some(),
        };
        egui::ScrollArea::vertical().id_salt("props").show(ui, |ui| {
            match selected.and_then(|id| self.doc.find_mut(id)) {
                Some(obj) => object_props(ui, obj, &mut actions, has_clip, focus_text, &mut pic),
                None => {
                    doc_props(ui, &mut self.doc, auto_grow, &mut doc_act);
                }
            }
        });
        (self.bg_tol, self.bg_feather, self.ai_model) = (pic.tol, pic.feather, pic.ai_model);

        for a in actions {
            match a {
                PropAction::ConvertToPath => {
                    if let Some(o) = selected.and_then(|id| self.doc.find_mut(id)) {
                        vector::convert_to_path(o);
                    }
                }
                PropAction::EditNodes => edit_nodes = true,
                PropAction::LoadFillImage => {
                    if let Some(img) = self.pick_image_file() {
                        if let Some(o) = selected.and_then(|id| self.doc.find_mut(id)) {
                            o.fill.kind = FillKind::Texture;
                            o.fill.texture = TextureKind::Image;
                            o.fill.image = Some(img);
                        }
                    }
                }
                PropAction::LoadTransImage => {
                    if let Some(img) = self.pick_image_file() {
                        if let Some(o) = selected.and_then(|id| self.doc.find_mut(id)) {
                            o.transparency.kind = TransparencyKind::Texture;
                            o.transparency.texture = TextureKind::Image;
                            o.transparency.image = Some(img);
                        }
                    }
                }
                PropAction::RemoveBackground => {
                    if let Some(id) = selected {
                        self.remove_background(id);
                    }
                }
                PropAction::MaskInvert => {
                    if let Some(m) = selected.and_then(|id| self.doc.find_mut(id)).and_then(|o| o.mask.as_mut()) {
                        imageops::invert_mask(Arc::make_mut(m));
                    }
                }
                PropAction::MaskClear => {
                    if let Some(o) = selected.and_then(|id| self.doc.find_mut(id)) {
                        o.mask = None;
                    }
                }
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
                PropAction::ResetBrushWork => {
                    if let Some(o) = selected.and_then(|id| self.doc.find_mut(id)) {
                        reset_brush_work(o);
                        self.status = "Brush work reset — back to the original.".into();
                    }
                }
                PropAction::AiRemoveBackground => {
                    if let Some(id) = selected {
                        self.start_ai_removal(id);
                    }
                }
                PropAction::CopyStyle => {
                    if let Some(o) = selected.and_then(|id| self.doc.find(id)) {
                        self.style_clipboard =
                            Some((o.fill.clone(), o.stroke.clone(), o.effects.clone(), o.transparency.clone()));
                        self.status = "Style copied".into();
                    }
                }
                PropAction::PasteStyle => {
                    if let (Some(o), Some((f, s, e, t))) =
                        (selected.and_then(|id| self.doc.find_mut(id)), self.style_clipboard.clone())
                    {
                        o.fill = f;
                        o.stroke = s;
                        o.effects = e;
                        o.transparency = t;
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
        match doc_act {
            Some(DocAction::CanvasDialog) => self.open_canvas_dialog(false),
            Some(DocAction::FitContent) => {
                // recorded by the change detection below as one undo step
                if self.fit_canvas_to_content() {
                    self.fit_pending = true;
                }
            }
            Some(DocAction::AutoGrow(g)) => {
                self.settings.auto_grow = g;
                self.settings.save();
            }
            None => {}
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
                if !self.press_ruler(p) {
                    let d = self.to_doc(p);
                    self.on_press(d, p, mods);
                }
            }
        }
        if self.pressing && down && !pressed {
            if let Some(p) = latest {
                let d = self.to_doc(p);
                self.on_drag(d, mods);
            }
        }
        if self.pressing && (released || !down) {
            // a guide dropped back on a ruler (or off the canvas area) is removed
            if let Drag::Guide { idx } = self.drag {
                let on_ruler = |p: Pos2| self.ruler_rects().is_some_and(|(t, l)| t.contains(p) || l.contains(p));
                if latest.is_none_or(|p| on_ruler(p) || !rect.contains(p)) && idx < self.doc.guides.len() {
                    self.commit_snapshot();
                    self.doc.guides.remove(idx);
                    self.status = "Guide removed".into();
                }
            }
            self.on_release(mods);
            self.pressing = false;
        }
        if !self.pressing && self.tool == Tool::Select {
            if let Some(p) = latest.filter(|p| rect.contains(*p)) {
                if let Some(i) = self.guide_at(p) {
                    ctx.set_cursor_icon(if self.doc.guides[i].vertical {
                        egui::CursorIcon::ResizeHorizontal
                    } else {
                        egui::CursorIcon::ResizeVertical
                    });
                }
            }
        }
        let right = ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary));
        if right && resp.hovered() && self.tool == Tool::Pen {
            self.close_pen_path();
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
        self.draw_grid_and_guides(&painter, doc_rect);
        self.draw_overlays(&painter);
        self.draw_rulers(&painter, latest);

        if self.tool == Tool::Brush {
            if let Some(p) = latest.filter(|p| rect.contains(*p)) {
                let r = self.brush.size * 0.5 * self.zoom;
                painter.circle_stroke(p, r, Stroke::new(1.0_f32, Color32::from_black_alpha(160)));
                painter.circle_stroke(p, r + 1.0, Stroke::new(1.0_f32, Color32::from_white_alpha(160)));
            }
        }
    }

    fn draw_grid_and_guides(&self, painter: &egui::Painter, doc_rect: Rect) {
        let view = self.canvas_rect;
        if self.settings.grid {
            let g = self.settings.grid_size.max(1.0);
            // skip lines so they stay at least 8 px apart on screen
            let mut step = g;
            while step * self.zoom < 8.0 {
                step *= 2.0;
            }
            let col = Color32::from_rgba_unmultiplied(0, 0, 0, 28);
            let r = doc_rect.intersect(view);
            let mut x = 0.0;
            while x <= self.doc.width as f32 {
                let sx = doc_rect.left() + x * self.zoom;
                if sx >= r.left() && sx <= r.right() {
                    painter.line_segment([Pos2::new(sx, r.top()), Pos2::new(sx, r.bottom())], Stroke::new(1.0_f32, col));
                }
                x += step;
            }
            let mut y = 0.0;
            while y <= self.doc.height as f32 {
                let sy = doc_rect.top() + y * self.zoom;
                if sy >= r.top() && sy <= r.bottom() {
                    painter.line_segment([Pos2::new(r.left(), sy), Pos2::new(r.right(), sy)], Stroke::new(1.0_f32, col));
                }
                y += step;
            }
        }
        let o = self.origin();
        let guide = Stroke::new(1.0_f32, Color32::from_rgb(0, 200, 230));
        for g in &self.doc.guides {
            if g.vertical {
                let x = o.x + g.pos * self.zoom;
                painter.line_segment([Pos2::new(x, view.top()), Pos2::new(x, view.bottom())], guide);
            } else {
                let y = o.y + g.pos * self.zoom;
                painter.line_segment([Pos2::new(view.left(), y), Pos2::new(view.right(), y)], guide);
            }
        }
        let snap = Stroke::new(1.0_f32, Color32::from_rgb(255, 40, 160));
        for &(vertical, pos) in &self.snap_lines {
            if vertical {
                let x = o.x + pos * self.zoom;
                painter.line_segment([Pos2::new(x, view.top()), Pos2::new(x, view.bottom())], snap);
            } else {
                let y = o.y + pos * self.zoom;
                painter.line_segment([Pos2::new(view.left(), y), Pos2::new(view.right(), y)], snap);
            }
        }
    }

    fn draw_rulers(&self, painter: &egui::Painter, pointer: Option<Pos2>) {
        let Some((top, left)) = self.ruler_rects() else { return };
        let bg = Color32::from_gray(36);
        let fg = Color32::from_gray(170);
        painter.rect_filled(top, 0.0, bg);
        painter.rect_filled(left, 0.0, bg);
        // major tick distance: a "nice" number of canvas pixels at least ~70 px apart on screen
        let mut step = 1.0f32;
        for s in [1.0, 2.0, 5.0, 10.0, 20.0, 25.0, 50.0, 100.0, 200.0, 250.0, 500.0, 1000.0, 2000.0, 5000.0] {
            step = s;
            if s * self.zoom >= 70.0 {
                break;
            }
        }
        let o = self.origin();
        let font = egui::FontId::proportional(10.0);
        let tick = Stroke::new(1.0_f32, fg);
        // top ruler
        let first = ((top.left() - o.x) / self.zoom / step).floor() as i64;
        let last = ((top.right() - o.x) / self.zoom / step).ceil() as i64;
        for i in first..=last {
            for k in 0..5 {
                let v = (i as f32 + k as f32 * 0.2) * step;
                let x = o.x + v * self.zoom;
                if x < top.left() + Self::RULER || x > top.right() {
                    continue;
                }
                let len = if k == 0 { Self::RULER * 0.6 } else { Self::RULER * 0.25 };
                painter.line_segment([Pos2::new(x, top.bottom() - len), Pos2::new(x, top.bottom())], tick);
                if k == 0 {
                    painter.text(Pos2::new(x + 3.0, top.top() + 1.0), egui::Align2::LEFT_TOP, format!("{}", v as i64), font.clone(), fg);
                }
            }
        }
        // left ruler
        let first = ((left.top() - o.y) / self.zoom / step).floor() as i64;
        let last = ((left.bottom() - o.y) / self.zoom / step).ceil() as i64;
        for i in first..=last {
            for k in 0..5 {
                let v = (i as f32 + k as f32 * 0.2) * step;
                let y = o.y + v * self.zoom;
                if y < left.top() + Self::RULER || y > left.bottom() {
                    continue;
                }
                let len = if k == 0 { Self::RULER * 0.6 } else { Self::RULER * 0.25 };
                painter.line_segment([Pos2::new(left.right() - len, y), Pos2::new(left.right(), y)], tick);
                if k == 0 {
                    painter.text(Pos2::new(left.left() + 1.0, y + 2.0), egui::Align2::LEFT_TOP, format!("{}", v as i64), font.clone(), fg);
                }
            }
        }
        // where the pointer is
        if let Some(p) = pointer.filter(|p| self.canvas_rect.contains(*p)) {
            let m = Stroke::new(1.0_f32, Color32::from_rgb(255, 140, 0));
            painter.line_segment([Pos2::new(p.x, top.top()), Pos2::new(p.x, top.bottom())], m);
            painter.line_segment([Pos2::new(left.left(), p.y), Pos2::new(left.right(), p.y)], m);
        }
        painter.rect_filled(
            Rect::from_min_size(top.left_top(), Vec2::splat(Self::RULER)),
            0.0,
            Color32::from_gray(30),
        );
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
        if self.tool == Tool::Transparency {
            self.draw_transparency_overlay(painter);
        }
        if self.tool == Tool::Light {
            self.draw_light_overlay(painter);
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

    /// A circle around local point `c` with radius `r` (in local pixels), seen through the object's
    /// perspective.
    fn local_circle_screen(&self, o: &Object, c: Pt, r: f32) -> Vec<Pos2> {
        let hm = o.homography();
        (0..48)
            .filter_map(|k| {
                let a = k as f32 / 48.0 * std::f32::consts::TAU;
                hm.apply_pt(c + Pt::new(a.cos(), a.sin()) * r).map(|p| self.to_screen(p))
            })
            .collect()
    }

    fn draw_transparency_overlay(&self, painter: &egui::Painter) {
        let Some(o) = self.selected.and_then(|id| self.doc.find(id)) else { return };
        let t = &o.transparency;
        if !matches!(t.kind, TransparencyKind::Linear | TransparencyKind::Radial) {
            return;
        }
        let (Some(a), Some(b)) = (self.norm_to_screen(o, t.p0), self.norm_to_screen(o, t.p1)) else { return };
        let line = Color32::from_rgb(255, 255, 255);
        painter.line_segment([a, b], Stroke::new(3.0_f32, Color32::from_black_alpha(140)));
        painter.line_segment([a, b], Stroke::new(1.5_f32, line));
        if t.kind == TransparencyKind::Radial {
            let c = Pt::new(t.p0[0] * o.size[0], t.p0[1] * o.size[1]);
            let e = Pt::new(t.p1[0] * o.size[0], t.p1[1] * o.size[1]);
            let ring = self.local_circle_screen(o, c, c.dist(e));
            painter.add(Shape::closed_line(ring, Stroke::new(1.0_f32, line)));
        }
        for (pos, opacity) in [(a, t.start), (b, t.end)] {
            let g = (opacity.clamp(0.0, 1.0) * 255.0) as u8;
            painter.circle_filled(pos, 8.0, Color32::from_gray(g));
            painter.circle_stroke(pos, 8.0, Stroke::new(2.0_f32, Color32::from_rgb(0, 170, 255)));
            painter.text(
                pos + Vec2::new(11.0, -11.0),
                egui::Align2::LEFT_BOTTOM,
                format!("{:.0}%", opacity * 100.0),
                egui::FontId::proportional(12.0),
                Color32::WHITE,
            );
        }
    }

    fn draw_light_overlay(&self, painter: &egui::Painter) {
        let Some(o) = self.selected.and_then(|id| self.doc.find(id)) else { return };
        for (idx, l) in o.effects.lighting.lights.iter().enumerate() {
            let c = Color32::from_rgb(l.color[0], l.color[1], l.color[2]);
            let ring = if self.light_sel == Some(idx) {
                Stroke::new(2.5_f32, Color32::WHITE)
            } else {
                Stroke::new(1.5_f32, Color32::from_black_alpha(200))
            };
            let lamp = |pos: Pos2| {
                for k in 0..8 {
                    let a = k as f32 * std::f32::consts::FRAC_PI_4;
                    let d = Vec2::new(a.cos(), a.sin());
                    painter.line_segment([pos + d * 11.0, pos + d * 15.0], Stroke::new(1.5_f32, c));
                }
                painter.circle_filled(pos, 8.0, c);
                painter.circle_stroke(pos, 8.0, ring);
            };
            match l.kind {
                LightKind::Point => {
                    if let Some(pos) = self.norm_to_screen(o, l.pos) {
                        lamp(pos);
                    }
                }
                LightKind::Spot => {
                    let (Some(pos), Some(tg)) = (self.norm_to_screen(o, l.pos), self.norm_to_screen(o, l.target))
                    else {
                        continue;
                    };
                    // footprint of the cone on the surface (approximate)
                    let scale = (o.size[0] + o.size[1]) * 0.5;
                    let lp = Pt::new(l.pos[0] * o.size[0], l.pos[1] * o.size[1]);
                    let tp = Pt::new(l.target[0] * o.size[0], l.target[1] * o.size[1]);
                    let dist = (lp.dist(tp).powi(2) + (l.height * scale).powi(2)).sqrt();
                    let r = dist * l.cone.clamp(1.0, 80.0).to_radians().tan();
                    painter.add(Shape::closed_line(
                        self.local_circle_screen(o, tp, r),
                        Stroke::new(1.0_f32, c.gamma_multiply(0.8)),
                    ));
                    painter.line_segment([pos, tg], Stroke::new(1.5_f32, c));
                    let d = Rect::from_center_size(tg, Vec2::splat(9.0));
                    painter.add(Shape::convex_polygon(
                        vec![d.center_top(), d.right_center(), d.center_bottom(), d.left_center()],
                        c,
                        Stroke::new(1.0_f32, Color32::BLACK),
                    ));
                    lamp(pos);
                }
                LightKind::Distant => {
                    let from = self.distant_handle(o, l);
                    let center = self.to_screen(o.center());
                    painter.line_segment([from, center], Stroke::new(1.5_f32, c));
                    let dir = (center - from).normalized();
                    let side = Vec2::new(-dir.y, dir.x);
                    painter.line_segment([center, center - dir * 10.0 + side * 5.0], Stroke::new(1.5_f32, c));
                    painter.line_segment([center, center - dir * 10.0 - side * 5.0], Stroke::new(1.5_f32, c));
                    lamp(from);
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
    pic: &mut PictureUi,
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
                    if obj.fill.texture == TextureKind::Image {
                        ui.horizontal(|ui| {
                            let label = if obj.fill.image.is_some() { "Change picture…" } else { "Load picture…" };
                            if ui.button(label).clicked() {
                                actions.push(PropAction::LoadFillImage);
                            }
                            if let Some(img) = &obj.fill.image {
                                ui.label(format!("{}×{}", img.0.width(), img.0.height()));
                            }
                        });
                        ui.checkbox(&mut obj.fill.image_stretch, "Stretch to fit the shape");
                        if !obj.fill.image_stretch {
                            ui.add(egui::Slider::new(&mut obj.fill.tex_scale, 0.05..=5.0).logarithmic(true).text("Tile size"));
                        }
                    } else {
                        rgba_label(ui, "Dark", &mut obj.fill.a);
                        rgba_label(ui, "Light", &mut obj.fill.b);
                        ui.add(egui::Slider::new(&mut obj.fill.tex_scale, 0.2..=5.0).logarithmic(true).text("Scale"));
                    }
                }
            }
        });
    }

    egui::CollapsingHeader::new("Outline").default_open(true).show(ui, |ui| {
        let s = &mut obj.stroke;
        ui.checkbox(&mut s.enabled, "Enabled");
        rgba_label(ui, "Colour", &mut s.color);
        ui.add(egui::Slider::new(&mut s.width, 0.5..=40.0).text("Width"));
        ui.horizontal(|ui| {
            ui.label("Position");
            egui::ComboBox::from_id_salt("stroke_pos")
                .selected_text(s.position.label())
                .show_ui(ui, |ui| {
                    for p in StrokePos::ALL {
                        ui.selectable_value(&mut s.position, p, p.label());
                    }
                });
        });
        if obj.kind != ShapeKind::Bitmap {
            ui.horizontal(|ui| {
                ui.label("Style");
                egui::ComboBox::from_id_salt("stroke_dash")
                    .selected_text(s.dash.label())
                    .show_ui(ui, |ui| {
                        for d in DashKind::ALL {
                            ui.selectable_value(&mut s.dash, d, d.label());
                        }
                    });
            });
        } else {
            ui.small("On a paint layer the outline traces around everything painted.");
        }
    });

    egui::CollapsingHeader::new("Transparency").default_open(true).show(ui, |ui| {
        let t = &mut obj.transparency;
        egui::ComboBox::from_id_salt("trans_kind")
            .selected_text(t.kind.label())
            .show_ui(ui, |ui| {
                for k in TransparencyKind::ALL {
                    ui.selectable_value(&mut t.kind, k, k.label());
                }
            });
        match t.kind {
            TransparencyKind::None => {}
            TransparencyKind::Flat => {
                ui.add(pct_slider(&mut t.start).text("Opacity"));
            }
            TransparencyKind::Linear | TransparencyKind::Radial => {
                ui.add(pct_slider(&mut t.start).text("Start opacity"));
                ui.add(pct_slider(&mut t.end).text("End opacity"));
                if ui.button("Reverse").clicked() {
                    std::mem::swap(&mut t.start, &mut t.end);
                }
                ui.small("Drag across the object with the Transparency tool (X) to set the direction.");
            }
            TransparencyKind::Texture => {
                egui::ComboBox::from_id_salt("trans_tex")
                    .selected_text(t.texture.label())
                    .show_ui(ui, |ui| {
                        for k in TextureKind::ALL {
                            ui.selectable_value(&mut t.texture, k, k.label());
                        }
                    });
                if t.texture == TextureKind::Image {
                    ui.horizontal(|ui| {
                        let label = if t.image.is_some() { "Change picture…" } else { "Load picture…" };
                        if ui.button(label).clicked() {
                            actions.push(PropAction::LoadTransImage);
                        }
                    });
                    ui.small("The picture's brightness sets the opacity (stretched over the object).");
                } else {
                    ui.add(egui::Slider::new(&mut t.tex_scale, 0.2..=5.0).logarithmic(true).text("Scale"));
                }
                ui.add(pct_slider(&mut t.start).text("Opacity (dark)"));
                ui.add(pct_slider(&mut t.end).text("Opacity (light)"));
            }
        }
        ui.separator();
        if obj.mask.is_some() {
            ui.horizontal_wrapped(|ui| {
                ui.label("Painted transparency:");
                if ui.small_button("Invert").clicked() {
                    actions.push(PropAction::MaskInvert);
                }
                if ui.small_button("Clear").on_hover_text("Make everything visible again").clicked() {
                    actions.push(PropAction::MaskClear);
                }
            });
        }
        ui.small("Paint transparency by hand with the Brush tool's Transparency: hide / reveal brushes.");
    });

    if obj.paint.is_some() && obj.kind == ShapeKind::Bitmap {
        egui::CollapsingHeader::new("Picture").default_open(true).show(ui, |ui| {
            if let Some(p) = &obj.paint {
                ui.label(format!("{} × {} px", p.width(), p.height()));
            }
            ui.add_space(4.0);
            ui.strong("Remove background with AI");
            ui.horizontal(|ui| {
                for m in AiModel::ALL {
                    ui.radio_value(&mut pic.ai_model, m, m.label());
                }
            });
            if pic.ai_busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Finding the subject…");
                });
            } else if ui
                .button("Remove background (AI)")
                .on_hover_text("Finds the main subject and hides everything else. Works on busy photos too.")
                .clicked()
            {
                actions.push(PropAction::AiRemoveBackground);
            }
            ui.small(ai::ATTRIBUTION);
            ui.add_space(6.0);
            ui.strong("Remove background by colour");
            ui.add(egui::Slider::new(&mut pic.tol, 0.01..=0.6).text("Tolerance"));
            ui.add(egui::Slider::new(&mut pic.feather, 0.0..=8.0).text("Soft edge"));
            if ui
                .button("Remove background (colour)")
                .on_hover_text("Hides everything that touches the border and has the border's colour")
                .clicked()
            {
                actions.push(PropAction::RemoveBackground);
            }
            ui.small("Best for plain or studio backgrounds. Touch up with the Brush tool's Magic wand or Transparency: hide / reveal — the original pixels are always kept.");
            ui.add_space(6.0);
            if ui
                .button("Reset brush work")
                .on_hover_text("Back to the picture as imported: removes painting and all hidden / revealed areas")
                .clicked()
            {
                actions.push(PropAction::ResetBrushWork);
            }
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

    let lit = obj.effects.lighting.enabled;
    egui::CollapsingHeader::new("Lights").default_open(lit).show(ui, |ui| {
        let lighting = &mut obj.effects.lighting;
        ui.checkbox(&mut lighting.enabled, "Enabled");
        ui.add(egui::Slider::new(&mut lighting.ambient, 0.0..=1.5).text("Ambient light"));
        ui.add(egui::Slider::new(&mut lighting.specular, 0.0..=1.5).text("Shine"));
        let mut remove = None;
        for (i, l) in lighting.lights.iter_mut().enumerate() {
            egui::CollapsingHeader::new(format!("{} {}", l.kind.label(), i + 1))
                .id_salt(("light", i))
                .default_open(true)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut l.enabled, "On");
                        ui.color_edit_button_srgba_unmultiplied(&mut l.color);
                        egui::ComboBox::from_id_salt(("light_kind", i))
                            .selected_text(l.kind.label())
                            .show_ui(ui, |ui| {
                                for k in LightKind::ALL {
                                    ui.selectable_value(&mut l.kind, k, k.label());
                                }
                            });
                        if ui.small_button("Remove").clicked() {
                            remove = Some(i);
                        }
                    });
                    ui.add(egui::Slider::new(&mut l.intensity, 0.0..=4.0).text("Strength"));
                    match l.kind {
                        LightKind::Point | LightKind::Spot => {
                            ui.add(egui::Slider::new(&mut l.height, 0.02..=2.0).text("Height"));
                            ui.add(egui::Slider::new(&mut l.range, 0.05..=4.0).logarithmic(true).text("Reach"));
                            if l.kind == LightKind::Spot {
                                ui.add(egui::Slider::new(&mut l.cone, 3.0..=80.0).text("Cone angle"));
                                ui.add(egui::Slider::new(&mut l.softness, 0.0..=1.0).text("Soft edge"));
                            }
                        }
                        LightKind::Distant => {
                            ui.add(egui::Slider::new(&mut l.angle, 0.0..=360.0).text("Direction"));
                            ui.add(egui::Slider::new(&mut l.elevation, 2.0..=90.0).text("Height"));
                        }
                    }
                });
        }
        if let Some(i) = remove {
            lighting.lights.remove(i);
        }
        ui.horizontal_wrapped(|ui| {
            for k in LightKind::ALL {
                if ui.button(format!("+ {}", k.label())).clicked() {
                    lighting.enabled = true;
                    lighting.lights.push(Light::new(k, [0.3, 0.3]));
                }
            }
        });
        ui.small("Or use the Lights tool (L): click on the object to place a light, drag it to move or aim it.");
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
            }
            None => {
                ui.label("No paint yet — use the Brush tool on this object.");
            }
        }
        if (obj.paint.is_some() || obj.mask.is_some())
            && ui
                .button("Reset brush work")
                .on_hover_text("Removes everything painted on this object and its painted transparency")
                .clicked()
        {
            actions.push(PropAction::ResetBrushWork);
        }
    });
}

fn doc_props(ui: &mut egui::Ui, doc: &mut Document, auto_grow: bool, act: &mut Option<DocAction>) {
    ui.strong("Document");
    rgba_label(ui, "Background", &mut doc.background);
    ui.add_space(4.0);
    ui.label(format!(
        "Canvas {} × {} px  ({})",
        doc.width,
        doc.height,
        canvas::aspect_label(doc.width, doc.height)
    ));
    ui.horizontal_wrapped(|ui| {
        if ui.button("Canvas size…").on_hover_text("Size, templates (1:1, 16:9, A4…) and anchor").clicked() {
            *act = Some(DocAction::CanvasDialog);
        }
        if ui.button("Fit to content").on_hover_text("Make the canvas just big enough for all objects").clicked() {
            *act = Some(DocAction::FitContent);
        }
    });
    let mut g = auto_grow;
    if ui
        .checkbox(&mut g, "Grow the canvas when objects go past the edge")
        .changed()
    {
        *act = Some(DocAction::AutoGrow(g));
    }
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
        self.handle_close(ctx);
        self.handle_dropped_files(ctx);
        self.poll_ai();
        self.canvas_dialog_ui(ctx);
        self.update_title(ctx);
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
                    ui.label("Written in Rust with egui, tiny-skia, tract and image.");
                    ui.separator();
                    ui.strong("Credits & licenses");
                    ui.label(ai::ATTRIBUTION);
                    ui.label(
                        "Built from open-source libraries under permissive licenses (MIT, Apache-2.0, BSD and \
                         similar). See THIRD_PARTY_NOTICES.md and the licenses folder that come with the program.",
                    );
                    ui.label("Text objects use the fonts installed on this computer; they are not part of Jabro Draw.");
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
        let mut app = JabroApp::with_worker(NprWorker::new(|| {}), AiWorker::new(|| {}));
        let mut doc = Document::new(800, 600);
        doc.next_id = 100;
        doc.objects = objects;
        app.doc = doc;
        app.canvas_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        app.zoom = 1.0;
        app.pan = Vec2::ZERO;
        app.settings.snap = false; // scripted drags use exact positions; snapping tests turn it on
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
    fn pen_right_click_closes_the_gap() {
        let mut app = app_with(vec![]);
        app.tool = Tool::Pen;
        click(&mut app, Pt::new(100.0, 100.0), Modifiers::NONE);
        app.close_pen_path(); // only one node: keeps drawing
        assert!(app.pen.is_some() && app.doc.objects.is_empty());
        click(&mut app, Pt::new(300.0, 120.0), Modifiers::NONE);
        click(&mut app, Pt::new(220.0, 300.0), Modifiers::NONE);
        app.close_pen_path(); // what a right-click does
        assert!(app.pen.is_none());
        let o = &app.doc.objects[0];
        let sp = &path_of(o)[0];
        assert!(sp.closed);
        assert_eq!(sp.nodes.len(), 3, "no extra node added at the right-click");
        assert_eq!(o.fill.kind, FillKind::Solid, "closed shapes are filled");
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
    fn reorder_ids_moves_blocks_to_a_gap() {
        let list = [4, 3, 2, 1];
        assert_eq!(reorder_ids(&list, &[1], 0), vec![1, 4, 3, 2], "bottom row to the top");
        assert_eq!(reorder_ids(&list, &[4], 4), vec![3, 2, 1, 4], "top row to the end");
        assert_eq!(reorder_ids(&list, &[4], 2), vec![3, 4, 2, 1], "down past one row");
        assert_eq!(reorder_ids(&list, &[3, 1], 0), vec![3, 1, 4, 2], "a selection moves as a block");
        assert_eq!(reorder_ids(&list, &[3], 1), list.to_vec(), "dropping in place changes nothing");
        assert_eq!(reorder_ids(&list, &[3], 2), list.to_vec(), "gap right below itself too");
    }

    #[test]
    fn dragging_in_the_object_list_restacks_with_one_undo_step() {
        let mut app = app_with(vec![rect(1, 0.0, 0.0), rect(2, 10.0, 0.0), rect(3, 20.0, 0.0)]);
        let order = |app: &JabroApp| app.doc.objects.iter().map(|o| o.id).collect::<Vec<_>>();
        // list shows 3,2,1 (top first); drop object 1 above everything
        app.reorder_display(&[1], 0);
        assert_eq!(order(&app), vec![2, 3, 1], "object 1 is now drawn on top");
        let undo_len = app.undo.len();
        app.reorder_display(&[1], 0);
        assert_eq!(app.undo.len(), undo_len, "a no-op drop records no undo step");
        app.undo();
        assert_eq!(order(&app), vec![1, 2, 3]);
    }

    #[test]
    fn transparency_tool_drag_sets_the_fade_direction() {
        let mut app = app_with(vec![rect(1, 100.0, 100.0)]);
        app.tool = Tool::Transparency;
        click(&mut app, Pt::new(150.0, 150.0), Modifiers::NONE);
        assert_eq!(app.selected, Some(1));
        assert_eq!(app.doc.objects[0].transparency.kind, TransparencyKind::None, "a click alone changes nothing");
        drag(&mut app, Pt::new(110.0, 150.0), Pt::new(190.0, 150.0), Modifiers::NONE);
        let t = &app.doc.objects[0].transparency;
        assert_eq!(t.kind, TransparencyKind::Linear);
        assert!((t.p0[0] - 0.1).abs() < 1e-3 && (t.p1[0] - 0.9).abs() < 1e-3, "{:?} {:?}", t.p0, t.p1);
        // move the end handle
        drag(&mut app, Pt::new(190.0, 150.0), Pt::new(150.0, 190.0), Modifiers::NONE);
        let t = &app.doc.objects[0].transparency;
        assert!((t.p1[0] - 0.5).abs() < 1e-3 && (t.p1[1] - 0.9).abs() < 1e-3, "{:?}", t.p1);
        app.undo();
        app.undo();
        assert_eq!(app.doc.objects[0].transparency.kind, TransparencyKind::None);
    }

    #[test]
    fn light_tool_places_moves_and_aims_lights() {
        let mut app = app_with(vec![rect(1, 100.0, 100.0)]);
        app.tool = Tool::Light;
        click(&mut app, Pt::new(125.0, 125.0), Modifiers::NONE);
        let l = &app.doc.objects[0].effects.lighting;
        assert!(l.enabled && l.lights.len() == 1 && l.lights[0].kind == LightKind::Point);
        assert!((l.lights[0].pos[0] - 0.25).abs() < 1e-3);
        drag(&mut app, Pt::new(125.0, 125.0), Pt::new(175.0, 150.0), Modifiers::NONE);
        let p = app.doc.objects[0].effects.lighting.lights[0].pos;
        assert!((p[0] - 0.75).abs() < 1e-3 && (p[1] - 0.5).abs() < 1e-3, "{p:?}");
        // a spotlight: press places it, the drag aims it
        app.new_light_kind = LightKind::Spot;
        drag(&mut app, Pt::new(110.0, 110.0), Pt::new(160.0, 190.0), Modifiers::NONE);
        let s = &app.doc.objects[0].effects.lighting.lights[1];
        assert_eq!(s.kind, LightKind::Spot);
        assert!((s.pos[0] - 0.1).abs() < 1e-3 && (s.target[1] - 0.9).abs() < 1e-3, "{s:?}");
    }

    #[test]
    fn unsaved_changes_are_tracked_until_saved() {
        let mut app = app_with(vec![rect(1, 10.0, 10.0)]);
        app.saved_doc = app.doc.clone();
        assert!(!app.has_unsaved_changes());
        app.tool = Tool::Select;
        drag(&mut app, Pt::new(20.0, 20.0), Pt::new(60.0, 20.0), Modifiers::NONE);
        assert!(app.has_unsaved_changes(), "moving an object is a change");
        let path = std::env::temp_dir().join("jabro_close_test.jabro");
        app.path = Some(path.clone());
        assert!(app.save(false));
        let _ = std::fs::remove_file(&path);
        assert!(!app.has_unsaved_changes(), "saving clears it");
        app.undo();
        assert!(app.has_unsaved_changes(), "undoing past the save is a change again");
    }

    fn picture_file() -> std::path::PathBuf {
        let mut pm = Pixmap::new(200, 100).unwrap();
        pm.fill(tiny_skia::Color::WHITE);
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(30, 160, 60, 255);
        let r = tiny_skia::Rect::from_xywh(70.0, 20.0, 60.0, 60.0).unwrap();
        pm.fill_rect(r, &paint, tiny_skia::Transform::identity(), None);
        let path = std::env::temp_dir().join(format!("jabro_pic_{}.png", std::process::id()));
        pm.save_png(&path).unwrap();
        path
    }

    #[test]
    fn import_picture_then_magic_wand_and_hide_brush() {
        let mut app = app_with(vec![]);
        let path = picture_file();
        app.import_image_object(&path, Some(Pt::new(400.0, 300.0)));
        let _ = std::fs::remove_file(&path);
        let o = &app.doc.objects[0];
        assert_eq!(o.kind, ShapeKind::Bitmap);
        assert_eq!(o.paint.as_ref().unwrap().width(), 200);
        assert!(o.center().dist(Pt::new(400.0, 300.0)) < 0.5, "placed where it was dropped");
        let id = o.id;

        // magic wand on the white background
        app.tool = Tool::Brush;
        app.brush.kind = BrushKind::MagicWand;
        click(&mut app, Pt::new(305.0, 255.0), Modifiers::NONE);
        let m = app.doc.objects[0].mask.as_ref().expect("mask created");
        assert_eq!(m.pixel(2, 2).unwrap().red(), 0, "background hidden");
        assert_eq!(m.pixel(100, 50).unwrap().red(), 255, "green square kept");
        app.undo();
        assert!(app.doc.objects[0].mask.is_none(), "one undo step");

        // paint transparency by hand onto the square
        app.brush.kind = BrushKind::Hide;
        app.brush.size = 20.0;
        app.brush.flow = 1.0;
        app.brush.hardness = 1.0;
        app.selected = Some(id);
        click(&mut app, Pt::new(400.0, 300.0), Modifiers::NONE);
        let m = app.doc.objects[0].mask.as_ref().unwrap();
        assert!(m.pixel(100, 50).unwrap().red() < 10, "hidden where painted");
        assert_eq!(m.pixel(10, 10).unwrap().red(), 255, "untouched elsewhere");
        app.brush.kind = BrushKind::Reveal;
        click(&mut app, Pt::new(400.0, 300.0), Modifiers::NONE);
        let m = app.doc.objects[0].mask.as_ref().unwrap();
        assert!(m.pixel(100, 50).unwrap().red() > 245, "revealed again");
        assert!(app.doc.objects[0].paint.as_ref().unwrap().pixel(100, 50).unwrap().green() > 100, "pixels untouched");
    }

    #[test]
    fn reset_brush_work_and_ai_removal_on_a_picture() {
        let mut app = app_with(vec![]);
        let path = picture_file();
        app.import_image_object(&path, Some(Pt::new(400.0, 300.0)));
        let _ = std::fs::remove_file(&path);
        let id = app.doc.objects[0].id;

        // paint on the picture and hide part of it
        app.tool = Tool::Brush;
        app.brush.kind = BrushKind::Round;
        drag(&mut app, Pt::new(320.0, 270.0), Pt::new(480.0, 330.0), Modifiers::NONE);
        app.brush.kind = BrushKind::Hide;
        click(&mut app, Pt::new(330.0, 330.0), Modifiers::NONE);
        let o = &app.doc.objects[0];
        assert!(!Arc::ptr_eq(o.paint.as_ref().unwrap(), &o.original.as_ref().unwrap().0), "pixels painted");
        assert!(o.mask.is_some());

        app.apply_to_selected(reset_brush_work);
        let o = &app.doc.objects[0];
        assert!(Arc::ptr_eq(o.paint.as_ref().unwrap(), &o.original.as_ref().unwrap().0), "back to the original");
        assert!(o.mask.is_none());

        // AI background removal runs on the worker thread
        app.ai_model = AiModel::Fast;
        app.start_ai_removal(id);
        assert_eq!(app.ai_busy, Some(id));
        let t0 = std::time::Instant::now();
        while app.ai_busy.is_some() && t0.elapsed().as_secs() < 60 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            app.poll_ai();
        }
        let m = app.doc.objects[0].mask.as_ref().expect("AI produced a mask");
        assert!(m.pixel(100, 50).unwrap().red() > 200, "green square kept");
        assert!(m.pixel(3, 3).unwrap().red() < 60, "white background hidden");
        app.undo();
        assert!(app.doc.objects[0].mask.is_none(), "AI result is one undo step");
    }

    #[test]
    fn untouched_pictures_are_saved_once() {
        let mut app = app_with(vec![]);
        let path = picture_file();
        app.import_image_object(&path, None);
        let _ = std::fs::remove_file(&path);
        let file = std::env::temp_dir().join(format!("jabro_dedupe_{}.jabro", std::process::id()));
        io::save(&app.doc, &file).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        let back = io::load(&file).unwrap();
        let _ = std::fs::remove_file(&file);
        assert!(text.contains("\"paint\":null") && text.contains("paint_is_original"), "stored once");
        let o = &back.objects[0];
        assert!(Arc::ptr_eq(o.paint.as_ref().unwrap(), &o.original.as_ref().unwrap().0));
    }

    #[test]
    fn aspect_labels_and_templates() {
        assert_eq!(canvas::aspect_label(1920, 1080), "16:9");
        assert_eq!(canvas::aspect_label(1080, 1080), "1:1");
        assert_eq!(canvas::aspect_label(1080, 1350), "4:5");
        assert_eq!(canvas::aspect_label(1080, 1920), "9:16");
        assert_eq!(canvas::aspect_label(1200, 630), "1.90:1");
        assert_eq!(canvas::aspect_label(2560, 1080), "21:9");
        for p in canvas::PRESETS {
            assert!(p.w >= canvas::MIN_SIZE && p.w <= canvas::MAX_SIZE && p.h <= canvas::MAX_SIZE, "{}", p.name);
        }
        let mut app = app_with(vec![rect(1, 10.0, 10.0)]);
        app.new_doc(1080, 1920);
        assert_eq!((app.doc.width, app.doc.height), (1080, 1920));
        assert!(app.doc.objects.is_empty());
        app.undo();
        assert_eq!(app.doc.objects.len(), 1, "New is undoable");
    }

    #[test]
    fn canvas_grows_when_objects_are_dropped_past_the_edge() {
        let mut app = app_with(vec![rect(1, 100.0, 100.0), rect(2, 400.0, 300.0)]);
        app.tool = Tool::Select;
        // setting off: nothing happens
        drag(&mut app, Pt::new(450.0, 350.0), Pt::new(850.0, 350.0), Modifiers::NONE);
        assert_eq!((app.doc.width, app.doc.height), (800, 600));
        app.undo();

        app.settings.auto_grow = true;
        drag(&mut app, Pt::new(450.0, 350.0), Pt::new(850.0, 650.0), Modifiers::NONE);
        assert!(app.doc.width >= 908 && app.doc.height >= 708, "{}x{}", app.doc.width, app.doc.height);
        app.undo();
        assert_eq!((app.doc.width, app.doc.height), (800, 600), "drop and growth undo together");
        assert!(app.doc.objects[1].quad[0].dist(Pt::new(400.0, 300.0)) < 1e-3);

        // past the top-left: everything shifts so the content stays where it is on screen
        let before = app.to_screen(app.doc.objects[0].quad[0]);
        drag(&mut app, Pt::new(150.0, 150.0), Pt::new(-50.0, -30.0), Modifiers::NONE);
        let o = &app.doc.objects[0];
        let (lo, _) = canvas::visual_bounds(o);
        assert!(lo.x >= 7.9 && lo.y >= 7.9, "object is inside the canvas now: {lo:?}");
        let moved_by = Pt::new(-200.0, -180.0);
        let expect = before + Vec2::new(moved_by.x, moved_by.y) * app.zoom;
        assert!(app.to_screen(o.quad[0]).distance(expect) < 0.5, "no visual jump");
    }

    #[test]
    fn canvas_resize_with_anchor_and_fit_to_content() {
        let mut app = app_with(vec![rect(1, 100.0, 100.0)]);
        app.resize_canvas(1000, 800, Pt::new(100.0, 100.0)); // centred anchor on +200 / +200
        assert!(app.doc.objects[0].quad[0].dist(Pt::new(200.0, 200.0)) < 1e-3);
        app.doc.objects[0].effects.shadow.enabled = true; // shadow reaches beyond the shape
        assert!(app.fit_canvas_to_content());
        let (lo, hi) = canvas::visual_bounds(&app.doc.objects[0]);
        assert!((lo.x - 16.0).abs() < 1.0 && (lo.y - 16.0).abs() < 1.0, "{lo:?}");
        assert!(hi.x <= app.doc.width as f32 - 15.0 && hi.y <= app.doc.height as f32 - 15.0);
        assert!(app.doc.width < 200 && app.doc.height < 200, "{}x{}", app.doc.width, app.doc.height);
    }

    #[test]
    fn copy_cut_and_paste_objects_and_text() {
        let mut app = app_with(vec![rect(1, 10.0, 10.0), rect(2, 200.0, 10.0), rect(3, 400.0, 10.0)]);
        app.select_only(Some(1));
        app.toggle_in_selection(2);
        assert!(app.copy_selected());
        app.paste(None);
        assert_eq!(app.doc.objects.len(), 5);
        let pasted: Vec<&Object> = app.doc.objects[3..].iter().collect();
        assert!(pasted[0].quad[0].dist(Pt::new(30.0, 30.0)) < 1e-3, "offset copy of object 1");
        assert!(pasted[1].quad[0].dist(Pt::new(220.0, 30.0)) < 1e-3);
        assert_eq!(app.selection().len(), 2, "the pasted copies are selected");
        app.paste(None);
        assert!(app.doc.objects[5].quad[0].dist(Pt::new(50.0, 50.0)) < 1e-3, "each paste steps further");
        app.undo();
        app.undo();
        assert_eq!(app.doc.objects.len(), 3);

        app.select_only(Some(3));
        app.cut_selected();
        assert_eq!(app.doc.objects.len(), 2);
        app.paste(None);
        assert_eq!(app.doc.objects.len(), 3, "cut + paste brings it back");

        app.paste(Some("Hello there".into()));
        let o = app.doc.objects.last().unwrap();
        assert!(matches!(&o.kind, ShapeKind::Text { text, .. } if text == "Hello there"));
    }

    #[test]
    fn copied_selection_becomes_a_cropped_transparent_picture() {
        let mut app = app_with(vec![rect(1, 100.0, 50.0)]);
        app.select_only(Some(1));
        let img = app.selection_image().unwrap();
        assert_eq!((img.width(), img.height()), (102, 102));
        assert_eq!(img.pixel(50, 50).unwrap().alpha(), 255);
        let mut app = app_with(vec![Object::new(1, ShapeKind::Ellipse, 0.0, 0.0, 100.0, 100.0)]);
        app.select_only(Some(1));
        let img = app.selection_image().unwrap();
        assert_eq!(img.pixel(1, 1).unwrap().alpha(), 0, "corners outside the ellipse are transparent");
    }

    #[test]
    fn moving_snaps_to_canvas_centre_objects_and_guides() {
        let mut app = app_with(vec![rect(1, 100.0, 100.0), rect(2, 500.0, 300.0)]);
        app.settings.snap = true;
        app.tool = Tool::Select;
        // centre of rect 1 ends 4 px from the canvas centre (400) -> snaps exactly
        drag(&mut app, Pt::new(150.0, 150.0), Pt::new(396.0, 150.0), Modifiers::NONE);
        let (lo, hi) = app.doc.objects[0].bounds();
        assert!(((lo.x + hi.x) / 2.0 - 400.0).abs() < 1e-3, "{lo:?} {hi:?}");
        // top edge 3 px above rect 2's top -> snaps to it
        drag(&mut app, Pt::new(400.0, 150.0), Pt::new(400.0, 347.0), Modifiers::NONE);
        assert!((app.doc.objects[0].bounds().0.y - 300.0).abs() < 1e-3);
        // Alt: no snapping
        drag(&mut app, Pt::new(400.0, 350.0), Pt::new(403.0, 350.0), Modifiers::ALT);
        assert!(((app.doc.objects[0].bounds().0.x + 50.0) - 403.0).abs() < 1e-3);

        // a guide from the left ruler, then snap to it
        app.canvas_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let ruler_point = Pos2::new(5.0, 300.0);
        assert!(app.press_ruler(ruler_point));
        app.on_drag(Pt::new(250.0, 300.0), Modifiers::NONE);
        app.on_release(Modifiers::NONE);
        assert_eq!(app.doc.guides, vec![Guide { vertical: true, pos: 250.0 }]);
        app.select_only(None);
        app.doc.objects[0].quad = Object::rect_quad(650.0, 480.0, 100.0, 100.0); // out of the way
        drag(&mut app, Pt::new(520.0, 320.0), Pt::new(274.0, 320.0), Modifiers::NONE); // left edge -> 254
        assert!((app.doc.objects[1].bounds().0.x - 250.0).abs() < 1e-3, "left edge on the guide");
    }

    #[test]
    fn grid_snaps_new_shapes() {
        let mut app = app_with(vec![]);
        app.settings.grid = true;
        app.settings.grid_size = 25.0;
        app.tool = Tool::Rect;
        drag(&mut app, Pt::new(103.0, 98.0), Pt::new(198.0, 147.0), Modifiers::NONE);
        let (lo, hi) = app.doc.objects[0].bounds();
        assert!(lo.dist(Pt::new(100.0, 100.0)) < 1e-3 && hi.dist(Pt::new(200.0, 150.0)) < 1e-3, "{lo:?} {hi:?}");
    }

    #[test]
    fn align_and_distribute() {
        let mut app = app_with(vec![rect(1, 10.0, 10.0), rect(2, 150.0, 80.0), rect(3, 500.0, 40.0)]);
        app.select_only(Some(1));
        app.toggle_in_selection(2);
        app.toggle_in_selection(3);
        app.align_selected(AlignTo::Top);
        assert!(app.doc.objects.iter().all(|o| (o.bounds().0.y - 10.0).abs() < 1e-3));
        app.distribute_selected(true);
        let xs: Vec<f32> = app.doc.objects.iter().map(|o| o.bounds().0.x).collect();
        assert!((xs[1] - 255.0).abs() < 1e-3, "middle one centred between: {xs:?}");
        app.select_only(Some(1));
        app.align_selected(AlignTo::Right);
        assert!((app.doc.objects[0].bounds().1.x - 800.0).abs() < 1e-3, "single object aligns to the canvas");
        app.undo();
        assert!((app.doc.objects[0].bounds().0.x - 10.0).abs() < 1e-3);
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
