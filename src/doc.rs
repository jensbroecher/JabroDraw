//! Document model. Every object is a "hybrid" object:
//! a vector shape (resolution independent) + an optional paint layer (pixels, in the
//! object's own local space, clipped by the shape) + non-destructive effects + a free
//! placement quad that allows move / scale / rotate / perspective.

use crate::geom::{Homography, Pt};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tiny_skia::Pixmap;

/// Unmultiplied sRGB colour.
pub type Rgba = [u8; 4];

static REV: AtomicU64 = AtomicU64::new(1);

/// Globally unique content revision, used as a render-cache key.
pub fn next_rev() -> u64 {
    REV.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FontChoice {
    Sans,
    Bold,
    Black,
    Serif,
    Impact,
    Script,
}

impl FontChoice {
    pub const ALL: [FontChoice; 6] = [
        FontChoice::Sans,
        FontChoice::Bold,
        FontChoice::Black,
        FontChoice::Serif,
        FontChoice::Impact,
        FontChoice::Script,
    ];
    pub fn label(self) -> &'static str {
        match self {
            FontChoice::Sans => "Sans",
            FontChoice::Bold => "Sans Bold",
            FontChoice::Black => "Black",
            FontChoice::Serif => "Serif",
            FontChoice::Impact => "Impact",
            FontChoice::Script => "Script",
        }
    }
}

/// One Bézier node; all coordinates normalised to the object's local box (0..1).
/// A straight segment has its handles sitting on the nodes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PathNode {
    pub p: [f32; 2],
    pub h_in: [f32; 2],
    pub h_out: [f32; 2],
    #[serde(default)]
    pub smooth: bool,
}

impl PathNode {
    pub fn corner(p: [f32; 2]) -> Self {
        Self {
            p,
            h_in: p,
            h_out: p,
            smooth: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubPath {
    pub nodes: Vec<PathNode>,
    pub closed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ShapeKind {
    /// `corner` = corner radius as a fraction (0..1) of half the shorter side.
    Rect { corner: f32 },
    Ellipse,
    /// `inner` = 1 gives a regular polygon, < 1 a star.
    Polygon { sides: u32, inner: f32 },
    /// Closed smooth curve through normalised (0..1) points.
    Freehand { points: Vec<[f32; 2]> },
    Text { text: String, font: FontChoice },
    /// Pure paint object; its shape is the alpha of its paint layer.
    Bitmap,
    /// Editable Bézier path (pen tool, boolean results, converted shapes).
    Path { subpaths: Vec<SubPath> },
}

impl ShapeKind {
    pub fn label(&self) -> &'static str {
        match self {
            ShapeKind::Rect { .. } => "Rectangle",
            ShapeKind::Ellipse => "Ellipse",
            ShapeKind::Polygon { .. } => "Star",
            ShapeKind::Freehand { .. } => "Shape",
            ShapeKind::Text { .. } => "Text",
            ShapeKind::Bitmap => "Paint layer",
            ShapeKind::Path { .. } => "Path",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FillKind {
    None,
    Solid,
    Linear,
    Radial,
    Texture,
}

impl FillKind {
    pub const ALL: [FillKind; 5] = [
        FillKind::None,
        FillKind::Solid,
        FillKind::Linear,
        FillKind::Radial,
        FillKind::Texture,
    ];
    pub fn label(self) -> &'static str {
        match self {
            FillKind::None => "None",
            FillKind::Solid => "Solid",
            FillKind::Linear => "Linear gradient",
            FillKind::Radial => "Radial gradient",
            FillKind::Texture => "Texture",
        }
    }
}

/// An imported picture shared between documents / undo steps (compared by identity).
#[derive(Clone)]
pub struct ImageData(pub Arc<Pixmap>);

impl PartialEq for ImageData {
    fn eq(&self, o: &Self) -> bool {
        Arc::ptr_eq(&self.0, &o.0)
    }
}

impl std::fmt::Debug for ImageData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ImageData({}x{})", self.0.width(), self.0.height())
    }
}

impl Serialize for ImageData {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        pixmap_serde::serialize(&Some(self.0.clone()), s)
    }
}

impl<'de> Deserialize<'de> for ImageData {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        pixmap_serde::deserialize(d)?
            .map(ImageData)
            .ok_or_else(|| serde::de::Error::custom("missing image"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextureKind {
    Clouds,
    Wood,
    Marble,
    Canvas,
    Paper,
    BrushedMetal,
    Cells,
    /// An imported picture (see `FillStyle::image` / `Transparency::image`).
    Image,
}

impl TextureKind {
    pub const ALL: [TextureKind; 8] = [
        TextureKind::Image,
        TextureKind::Clouds,
        TextureKind::Wood,
        TextureKind::Marble,
        TextureKind::Canvas,
        TextureKind::Paper,
        TextureKind::BrushedMetal,
        TextureKind::Cells,
    ];
    pub fn label(self) -> &'static str {
        match self {
            TextureKind::Clouds => "Clouds",
            TextureKind::Wood => "Wood",
            TextureKind::Marble => "Marble",
            TextureKind::Canvas => "Canvas weave",
            TextureKind::Paper => "Paper",
            TextureKind::BrushedMetal => "Brushed metal",
            TextureKind::Cells => "Stone cells",
            TextureKind::Image => "Image file…",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FillStyle {
    pub kind: FillKind,
    pub a: Rgba,
    pub b: Rgba,
    /// Gradient angle in degrees (90 = top to bottom).
    pub angle: f32,
    pub texture: TextureKind,
    pub tex_scale: f32,
    /// Picture used when `texture` is `Image`.
    pub image: Option<ImageData>,
    /// Stretch the picture over the object instead of tiling it.
    pub image_stretch: bool,
    /// Colour stops (position 0..1, colour). Empty = just `a` to `b`.
    pub stops: Vec<(f32, Rgba)>,
    /// Gradient start / end, normalised to the object box (set with the gradient tool).
    /// None = use `angle` across the whole object.
    pub points: Option<([f32; 2], [f32; 2])>,
}

impl Default for FillStyle {
    fn default() -> Self {
        Self {
            kind: FillKind::Solid,
            a: [70, 130, 220, 255],
            b: [20, 40, 120, 255],
            angle: 90.0,
            texture: TextureKind::Clouds,
            tex_scale: 1.0,
            image: None,
            image_stretch: true,
            stops: Vec::new(),
            points: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StrokePos {
    #[default]
    Center,
    Inside,
    Outside,
}

impl StrokePos {
    pub const ALL: [StrokePos; 3] = [StrokePos::Outside, StrokePos::Center, StrokePos::Inside];
    pub fn label(self) -> &'static str {
        match self {
            StrokePos::Center => "Centered",
            StrokePos::Inside => "Inside",
            StrokePos::Outside => "Outside",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DashKind {
    #[default]
    Solid,
    Dashed,
    Dotted,
}

impl DashKind {
    pub const ALL: [DashKind; 3] = [DashKind::Solid, DashKind::Dashed, DashKind::Dotted];
    pub fn label(self) -> &'static str {
        match self {
            DashKind::Solid => "Solid",
            DashKind::Dashed => "Dashed",
            DashKind::Dotted => "Dotted",
        }
    }
}

impl FillStyle {
    /// Colour stops sorted by position (falls back to `a` → `b`).
    pub fn gradient_stops(&self) -> Vec<(f32, Rgba)> {
        if self.stops.len() < 2 {
            return vec![(0.0, self.a), (1.0, self.b)];
        }
        let mut s = self.stops.clone();
        s.sort_by(|x, y| x.0.total_cmp(&y.0));
        s
    }

    /// Colour of the gradient at position t (0..1).
    pub fn color_at(&self, t: f32) -> Rgba {
        let s = self.gradient_stops();
        let t = t.clamp(0.0, 1.0);
        if t <= s[0].0 {
            return s[0].1;
        }
        for w in s.windows(2) {
            let (a, b) = (w[0], w[1]);
            if t <= b.0 {
                let k = if b.0 > a.0 { (t - a.0) / (b.0 - a.0) } else { 0.0 };
                return std::array::from_fn(|i| (a.1[i] as f32 + (b.1[i] as f32 - a.1[i] as f32) * k).round() as u8);
            }
        }
        s[s.len() - 1].1
    }
}

/// Free-form warp: a grid of control points (normalised to the object box) that bends the
/// object before its perspective placement. Identity = points on a regular grid.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mesh {
    pub cols: usize,
    pub rows: usize,
    /// (cols + 1) × (rows + 1) points, row by row.
    pub pts: Vec<[f32; 2]>,
}

impl Mesh {
    pub fn identity(cols: usize, rows: usize) -> Self {
        let mut pts = Vec::with_capacity((cols + 1) * (rows + 1));
        for j in 0..=rows {
            for i in 0..=cols {
                pts.push([i as f32 / cols as f32, j as f32 / rows as f32]);
            }
        }
        Self { cols, rows, pts }
    }

    pub fn is_identity(&self) -> bool {
        *self == Mesh::identity(self.cols, self.rows)
    }

    /// Where the undeformed point (u, v) goes (bilinear between control points; extrapolates
    /// outside 0..1 so outlines and anti-aliased edges keep up).
    pub fn map(&self, u: f32, v: f32) -> [f32; 2] {
        let fx = u * self.cols as f32;
        let fy = v * self.rows as f32;
        let i = (fx.floor() as isize).clamp(0, self.cols as isize - 1) as usize;
        let j = (fy.floor() as isize).clamp(0, self.rows as isize - 1) as usize;
        let (tx, ty) = (fx - i as f32, fy - j as f32);
        let p = |i: usize, j: usize| self.pts[j * (self.cols + 1) + i];
        let (a, b, c, d) = (p(i, j), p(i + 1, j), p(i, j + 1), p(i + 1, j + 1));
        let lerp = |k: usize| {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bottom = c[k] + (d[k] - c[k]) * tx;
            top + (bottom - top) * ty
        };
        [lerp(0), lerp(1)]
    }

    /// Same deformation on a different grid density (for liquify, which needs a fine grid).
    pub fn resampled(&self, cols: usize, rows: usize) -> Mesh {
        let mut m = Mesh::identity(cols, rows);
        for p in m.pts.iter_mut() {
            *p = self.map(p[0], p[1]);
        }
        m
    }
}

/// How an object mixes with what is underneath it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Blend {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl Blend {
    pub const ALL: [Blend; 16] = [
        Blend::Normal,
        Blend::Multiply,
        Blend::Screen,
        Blend::Overlay,
        Blend::Darken,
        Blend::Lighten,
        Blend::ColorDodge,
        Blend::ColorBurn,
        Blend::HardLight,
        Blend::SoftLight,
        Blend::Difference,
        Blend::Exclusion,
        Blend::Hue,
        Blend::Saturation,
        Blend::Color,
        Blend::Luminosity,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Blend::Normal => "Normal",
            Blend::Multiply => "Multiply",
            Blend::Screen => "Screen",
            Blend::Overlay => "Overlay",
            Blend::Darken => "Darken",
            Blend::Lighten => "Lighten",
            Blend::ColorDodge => "Colour dodge",
            Blend::ColorBurn => "Colour burn",
            Blend::HardLight => "Hard light",
            Blend::SoftLight => "Soft light",
            Blend::Difference => "Difference",
            Blend::Exclusion => "Exclusion",
            Blend::Hue => "Hue",
            Blend::Saturation => "Saturation",
            Blend::Color => "Colour",
            Blend::Luminosity => "Luminosity",
        }
    }
    pub fn to_skia(self) -> tiny_skia::BlendMode {
        use tiny_skia::BlendMode as B;
        match self {
            Blend::Normal => B::SourceOver,
            Blend::Multiply => B::Multiply,
            Blend::Screen => B::Screen,
            Blend::Overlay => B::Overlay,
            Blend::Darken => B::Darken,
            Blend::Lighten => B::Lighten,
            Blend::ColorDodge => B::ColorDodge,
            Blend::ColorBurn => B::ColorBurn,
            Blend::HardLight => B::HardLight,
            Blend::SoftLight => B::SoftLight,
            Blend::Difference => B::Difference,
            Blend::Exclusion => B::Exclusion,
            Blend::Hue => B::Hue,
            Blend::Saturation => B::Saturation,
            Blend::Color => B::Color,
            Blend::Luminosity => B::Luminosity,
        }
    }
}

/// Non-destructive colour adjustments of one object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Adjust {
    /// Degrees around the colour wheel.
    pub hue: f32,
    /// -1 (grey) .. +1 (twice as colourful).
    pub saturation: f32,
    /// -1 (black) .. +1 (white).
    pub lightness: f32,
    /// -1 .. +1.
    pub contrast: f32,
    /// Levels: input black point, white point (0..1) and gamma (midtones).
    pub black: f32,
    pub white: f32,
    pub gamma: f32,
}

impl Default for Adjust {
    fn default() -> Self {
        Self {
            hue: 0.0,
            saturation: 0.0,
            lightness: 0.0,
            contrast: 0.0,
            black: 0.0,
            white: 1.0,
            gamma: 1.0,
        }
    }
}

impl Adjust {
    pub fn is_identity(&self) -> bool {
        *self == Adjust::default()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StrokeStyle {
    pub enabled: bool,
    pub color: Rgba,
    pub width: f32,
    pub position: StrokePos,
    pub dash: DashKind,
}

impl Default for StrokeStyle {
    fn default() -> Self {
        Self {
            enabled: false,
            color: [30, 30, 30, 255],
            width: 3.0,
            position: StrokePos::Center,
            dash: DashKind::Solid,
        }
    }
}

impl StrokeStyle {
    /// How far the outline reaches outside the shape.
    pub fn outer_extent(&self) -> f32 {
        if !self.enabled {
            return 0.0;
        }
        match self.position {
            StrokePos::Center => self.width * 0.5,
            StrokePos::Outside => self.width,
            StrokePos::Inside => 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransparencyKind {
    None,
    Flat,
    Linear,
    Radial,
    Texture,
}

impl TransparencyKind {
    pub const ALL: [TransparencyKind; 5] = [
        TransparencyKind::None,
        TransparencyKind::Flat,
        TransparencyKind::Linear,
        TransparencyKind::Radial,
        TransparencyKind::Texture,
    ];
    pub fn label(self) -> &'static str {
        match self {
            TransparencyKind::None => "None (opaque)",
            TransparencyKind::Flat => "Flat",
            TransparencyKind::Linear => "Linear fade",
            TransparencyKind::Radial => "Radial fade",
            TransparencyKind::Texture => "Texture",
        }
    }
}

/// Object transparency, painted like a fill: opacity `start` at `p0` fading to `end` at `p1`.
/// Points are normalised to the object's local box, so the fade follows moves and perspective.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Transparency {
    pub kind: TransparencyKind,
    pub start: f32,
    pub end: f32,
    pub p0: [f32; 2],
    pub p1: [f32; 2],
    pub texture: TextureKind,
    pub tex_scale: f32,
    /// Picture whose brightness drives the opacity when `texture` is `Image`.
    pub image: Option<ImageData>,
}

impl Default for Transparency {
    fn default() -> Self {
        Self {
            kind: TransparencyKind::None,
            start: 1.0,
            end: 0.0,
            p0: [0.5, 0.0],
            p1: [0.5, 1.0],
            texture: TextureKind::Clouds,
            tex_scale: 1.0,
            image: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LightKind {
    Point,
    Spot,
    Distant,
}

impl LightKind {
    pub const ALL: [LightKind; 3] = [LightKind::Point, LightKind::Spot, LightKind::Distant];
    pub fn label(self) -> &'static str {
        match self {
            LightKind::Point => "Point light",
            LightKind::Spot => "Spotlight",
            LightKind::Distant => "Distant light",
        }
    }
}

/// A coloured light shining on one object. Positions are normalised to the object's local box;
/// `height` and `range` are in object sizes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Light {
    pub kind: LightKind,
    pub enabled: bool,
    pub color: Rgba,
    pub intensity: f32,
    pub pos: [f32; 2],
    pub target: [f32; 2],
    pub height: f32,
    pub range: f32,
    /// Spotlight half-angle in degrees and how soft its edge is (0..1).
    pub cone: f32,
    pub softness: f32,
    /// Distant light direction (degrees, like the bevel light) and height above the horizon.
    pub angle: f32,
    pub elevation: f32,
}

impl Light {
    pub fn new(kind: LightKind, pos: [f32; 2]) -> Self {
        Self {
            kind,
            pos,
            target: [pos[0], (pos[1] + 0.35).min(1.0)],
            ..Default::default()
        }
    }
}

impl Default for Light {
    fn default() -> Self {
        Self {
            kind: LightKind::Point,
            enabled: true,
            color: [255, 244, 220, 255],
            intensity: 1.2,
            pos: [0.3, 0.3],
            target: [0.5, 0.6],
            height: 0.35,
            range: 0.8,
            cone: 28.0,
            softness: 0.4,
            angle: 135.0,
            elevation: 45.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lighting {
    pub enabled: bool,
    /// Light that reaches the object when no lamp shines on it (0 = black).
    pub ambient: f32,
    pub specular: f32,
    pub lights: Vec<Light>,
}

impl Default for Lighting {
    fn default() -> Self {
        Self {
            enabled: false,
            ambient: 0.3,
            specular: 0.35,
            lights: Vec::new(),
        }
    }
}

impl Lighting {
    pub fn active(&self) -> bool {
        self.enabled && self.lights.iter().any(|l| l.enabled)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BevelProfile {
    Round,
    Chisel,
    Cove,
    Ridge,
    Pillow,
}

impl BevelProfile {
    pub const ALL: [BevelProfile; 5] = [
        BevelProfile::Round,
        BevelProfile::Chisel,
        BevelProfile::Cove,
        BevelProfile::Ridge,
        BevelProfile::Pillow,
    ];
    pub fn label(self) -> &'static str {
        match self {
            BevelProfile::Round => "Round",
            BevelProfile::Chisel => "Chisel",
            BevelProfile::Cove => "Cove",
            BevelProfile::Ridge => "Ridge",
            BevelProfile::Pillow => "Pillow (inflate)",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bevel {
    pub enabled: bool,
    pub profile: BevelProfile,
    pub width: f32,
    pub depth: f32,
    pub softness: f32,
    /// Light azimuth in degrees, 0 = from the right, 90 = from the top.
    pub light_angle: f32,
    pub light_elevation: f32,
    pub intensity: f32,
    pub highlight: Rgba,
    pub shadow: Rgba,
    pub gloss: f32,
    pub shininess: f32,
    /// Amount of (procedural) environment reflection — chrome / metal look.
    pub reflection: f32,
}

impl Default for Bevel {
    fn default() -> Self {
        Self {
            enabled: false,
            profile: BevelProfile::Round,
            width: 8.0,
            depth: 0.8,
            softness: 1.0,
            light_angle: 135.0,
            light_elevation: 40.0,
            intensity: 0.8,
            highlight: [255, 255, 255, 255],
            shadow: [0, 0, 0, 255],
            gloss: 0.3,
            shininess: 20.0,
            reflection: 0.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shadow {
    pub enabled: bool,
    pub color: Rgba,
    pub dx: f32,
    pub dy: f32,
    pub blur: f32,
    pub opacity: f32,
}

impl Default for Shadow {
    fn default() -> Self {
        Self {
            enabled: false,
            color: [0, 0, 0, 255],
            dx: 8.0,
            dy: 10.0,
            blur: 10.0,
            opacity: 0.5,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Glow {
    pub enabled: bool,
    pub color: Rgba,
    pub size: f32,
    pub opacity: f32,
}

impl Default for Glow {
    fn default() -> Self {
        Self {
            enabled: false,
            color: [255, 230, 120, 255],
            size: 16.0,
            opacity: 0.8,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Effects {
    pub bevel: Bevel,
    pub shadow: Shadow,
    pub glow: Glow,
    pub lighting: Lighting,
}

fn one() -> f32 {
    1.0
}
fn yes() -> bool {
    true
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Object {
    pub id: u64,
    pub name: String,
    pub kind: ShapeKind,
    /// Resolution of the object's local raster (w, h) in pixels.
    pub size: [f32; 2],
    /// Where the local rectangle lands on the canvas: TL, TR, BR, BL.
    pub quad: [Pt; 4],
    #[serde(default)]
    pub fill: FillStyle,
    #[serde(default)]
    pub stroke: StrokeStyle,
    #[serde(default)]
    pub effects: Effects,
    #[serde(default)]
    pub transparency: Transparency,
    #[serde(default)]
    pub blend: Blend,
    #[serde(default)]
    pub adjust: Adjust,
    /// Free-form warp (warp tool / liquify); None = straight.
    #[serde(default)]
    pub mesh: Option<Mesh>,
    #[serde(default = "one")]
    pub opacity: f32,
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default, with = "pixmap_serde")]
    pub paint: Option<Arc<Pixmap>>,
    /// Painted transparency mask (grey: white = visible, black = hidden), stretched over the
    /// object. Background removal and the Hide / Reveal brushes write here, never into pixels.
    #[serde(default, with = "pixmap_serde")]
    pub mask: Option<Arc<Pixmap>>,
    /// The picture as imported, so brush work on it can be reset.
    #[serde(default)]
    pub original: Option<ImageData>,
    /// Save-file shortcut: the paint layer is still the untouched original (stored once).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub paint_is_original: bool,
    /// Objects with the same group id are selected and transformed together.
    #[serde(default)]
    pub group: Option<u64>,
    /// Linked clones share an instance id: editing one updates all of them.
    #[serde(default)]
    pub instance_of: Option<u64>,
    /// How the NPR look treats this object: like the document, not at all, or its own look.
    #[serde(default)]
    pub npr_mode: NprMode,
    #[serde(default)]
    pub npr_own: NprSettings,
    #[serde(skip)]
    pub rev: u64,
}

impl Object {
    pub fn new(id: u64, kind: ShapeKind, x: f32, y: f32, w: f32, h: f32) -> Self {
        let w = w.max(1.0);
        let h = h.max(1.0);
        Self {
            id,
            name: format!("{} {}", kind.label(), id),
            kind,
            size: [w.round(), h.round()],
            quad: Self::rect_quad(x, y, w, h),
            fill: FillStyle::default(),
            stroke: StrokeStyle::default(),
            effects: Effects::default(),
            transparency: Transparency::default(),
            blend: Blend::Normal,
            adjust: Adjust::default(),
            mesh: None,
            opacity: 1.0,
            visible: true,
            paint: None,
            mask: None,
            original: None,
            paint_is_original: false,
            group: None,
            instance_of: None,
            npr_mode: NprMode::Inherit,
            npr_own: NprSettings::default(),
            rev: next_rev(),
        }
    }

    pub fn rect_quad(x: f32, y: f32, w: f32, h: f32) -> [Pt; 4] {
        [
            Pt::new(x, y),
            Pt::new(x + w, y),
            Pt::new(x + w, y + h),
            Pt::new(x, y + h),
        ]
    }

    pub fn touch(&mut self) {
        self.rev = next_rev();
    }

    pub fn homography(&self) -> Homography {
        Homography::from_quad(&self.quad, self.size[0], self.size[1])
    }

    pub fn raster_size(&self) -> (u32, u32) {
        (
            self.size[0].round().clamp(1.0, 4096.0) as u32,
            self.size[1].round().clamp(1.0, 4096.0) as u32,
        )
    }

    pub fn center(&self) -> Pt {
        let q = &self.quad;
        Pt::new(
            (q[0].x + q[1].x + q[2].x + q[3].x) * 0.25,
            (q[0].y + q[1].y + q[2].y + q[3].y) * 0.25,
        )
    }

    /// Unit vectors of the local x / y axes on the canvas.
    pub fn axes(&self) -> (Pt, Pt) {
        let q = &self.quad;
        (
            ((q[1] - q[0]) + (q[2] - q[3])).normalized(),
            ((q[3] - q[0]) + (q[2] - q[1])).normalized(),
        )
    }

    pub fn rotation(&self) -> f32 {
        (self.quad[1] - self.quad[0]).angle()
    }

    pub fn bounds(&self) -> (Pt, Pt) {
        let mut lo = self.quad[0];
        let mut hi = self.quad[0];
        for p in &self.quad[1..] {
            lo = Pt::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = Pt::new(hi.x.max(p.x), hi.y.max(p.y));
        }
        (lo, hi)
    }

    /// Re-derive raster resolution from the placement (after a scale).
    pub fn update_size_from_quad(&mut self) {
        let q = &self.quad;
        let w = ((q[1] - q[0]).len() + (q[2] - q[3]).len()) * 0.5;
        let h = ((q[3] - q[0]).len() + (q[2] - q[1]).len()) * 0.5;
        let new = [w.round().clamp(1.0, 4096.0), h.round().clamp(1.0, 4096.0)];
        if new != self.size {
            self.size = new;
            self.touch();
        }
    }

    /// Turn any perspective / shear back into a plain (possibly rotated) rectangle.
    pub fn reset_perspective(&mut self) {
        let c = self.center();
        let rot = self.rotation();
        let q = &self.quad;
        let w = ((q[1] - q[0]).len() + (q[2] - q[3]).len()) * 0.5;
        let h = ((q[3] - q[0]).len() + (q[2] - q[1]).len()) * 0.5;
        let corners = [
            Pt::new(-w / 2.0, -h / 2.0),
            Pt::new(w / 2.0, -h / 2.0),
            Pt::new(w / 2.0, h / 2.0),
            Pt::new(-w / 2.0, h / 2.0),
        ];
        self.quad = corners.map(|p| c + p.rotate(rot));
        self.update_size_from_quad();
    }

    /// Keep the text box proportional to the text (adjusts the width, keeps the height).
    pub fn fit_text_width(&mut self) {
        if let ShapeKind::Text { text, font } = &self.kind {
            if let Some(aspect) = crate::shapes::text_aspect(text, *font) {
                let new_w = (self.size[1] * aspect).max(4.0);
                let k = new_w / self.size[0].max(1.0);
                let q = self.quad;
                self.quad[1] = q[0] + (q[1] - q[0]) * k;
                self.quad[2] = q[3] + (q[2] - q[3]) * k;
                self.update_size_from_quad();
                self.touch();
            }
        }
    }

    /// Does this object look the same as `o` (shape, fill, effects, paint…), ignoring where it
    /// is placed? Linked clones keep exactly this part in sync.
    pub fn same_look(&self, o: &Object) -> bool {
        let same = |a: &Option<Arc<Pixmap>>, b: &Option<Arc<Pixmap>>| match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        self.kind == o.kind
            && self.fill == o.fill
            && self.stroke == o.stroke
            && self.effects == o.effects
            && self.transparency == o.transparency
            && self.blend == o.blend
            && self.adjust == o.adjust
            && self.mesh == o.mesh
            && self.original == o.original
            && self.opacity == o.opacity
            && self.npr_mode == o.npr_mode
            && self.npr_own == o.npr_own
            && same(&self.paint, &o.paint)
            && same(&self.mask, &o.mask)
    }

    /// Take over the look of `src` (keeps placement, name, group and identity).
    pub fn copy_look(&mut self, src: &Object) {
        self.kind = src.kind.clone();
        self.fill = src.fill.clone();
        self.stroke = src.stroke.clone();
        self.effects = src.effects.clone();
        self.transparency = src.transparency.clone();
        self.blend = src.blend;
        self.adjust = src.adjust.clone();
        self.mesh = src.mesh.clone();
        self.original = src.original.clone();
        self.opacity = src.opacity;
        self.npr_mode = src.npr_mode;
        self.npr_own = src.npr_own.clone();
        self.paint = src.paint.clone();
        self.mask = src.mask.clone();
    }

    /// Compare everything the user can edit (ignores the cache revision).
    pub fn same_content(&self, o: &Object) -> bool {
        let same = |a: &Option<Arc<Pixmap>>, b: &Option<Arc<Pixmap>>| match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        let paint_same = same(&self.paint, &o.paint) && same(&self.mask, &o.mask);
        self.id == o.id
            && self.name == o.name
            && self.kind == o.kind
            && self.size == o.size
            && self.quad == o.quad
            && self.fill == o.fill
            && self.stroke == o.stroke
            && self.effects == o.effects
            && self.transparency == o.transparency
            && self.blend == o.blend
            && self.adjust == o.adjust
            && self.mesh == o.mesh
            && self.original == o.original
            && self.group == o.group
            && self.instance_of == o.instance_of
            && self.opacity == o.opacity
            && self.npr_mode == o.npr_mode
            && self.npr_own == o.npr_own
            && self.visible == o.visible
            && paint_same
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NprMode {
    /// Painted with the document's look.
    #[default]
    Inherit,
    /// Stays clean even when the document has a look.
    Off,
    /// Painted with its own look.
    Own,
}

impl NprMode {
    pub const ALL: [NprMode; 3] = [NprMode::Inherit, NprMode::Off, NprMode::Own];
    pub fn label(self) -> &'static str {
        match self {
            NprMode::Inherit => "Document look",
            NprMode::Off => "No look (stay clean)",
            NprMode::Own => "Its own look",
        }
    }
}

impl Document {
    /// The looks the document paints with (main look first, then the stack), ignoring "None".
    pub fn looks(&self) -> Vec<NprSettings> {
        std::iter::once(&self.npr)
            .chain(&self.npr_stack)
            .filter(|s| s.style != NprStyle::None && s.strength > 0.0)
            .cloned()
            .collect()
    }

    /// The looks a given object is painted with.
    pub fn looks_for(&self, o: &Object) -> Vec<NprSettings> {
        match o.npr_mode {
            NprMode::Inherit => self.looks(),
            NprMode::Off => Vec::new(),
            NprMode::Own if o.npr_own.style != NprStyle::None => vec![o.npr_own.clone()],
            NprMode::Own => Vec::new(),
        }
    }

    /// Is any look painted anywhere?
    pub fn any_look(&self) -> bool {
        !self.looks().is_empty() || self.objects.iter().any(|o| o.visible && !self.looks_for(o).is_empty())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NprStyle {
    None,
    Watercolor,
    OilPaint,
    Impressionist,
    Pointillism,
    Pencil,
    ColorPencil,
    ComicInk,
}

impl NprStyle {
    pub const ALL: [NprStyle; 8] = [
        NprStyle::None,
        NprStyle::Watercolor,
        NprStyle::OilPaint,
        NprStyle::Impressionist,
        NprStyle::Pointillism,
        NprStyle::Pencil,
        NprStyle::ColorPencil,
        NprStyle::ComicInk,
    ];
    pub fn label(self) -> &'static str {
        match self {
            NprStyle::None => "None (clean)",
            NprStyle::Watercolor => "Watercolor",
            NprStyle::OilPaint => "Oil paint",
            NprStyle::Impressionist => "Impressionist strokes",
            NprStyle::Pointillism => "Pointillism",
            NprStyle::Pencil => "Pencil sketch",
            NprStyle::ColorPencil => "Colored pencil",
            NprStyle::ComicInk => "Comic ink",
        }
    }
    pub fn from_name(s: &str) -> Option<NprStyle> {
        let s = s.to_ascii_lowercase();
        Some(match s.as_str() {
            "none" => NprStyle::None,
            "watercolor" | "watercolour" => NprStyle::Watercolor,
            "oil" | "oilpaint" => NprStyle::OilPaint,
            "impressionist" => NprStyle::Impressionist,
            "pointillism" => NprStyle::Pointillism,
            "pencil" => NprStyle::Pencil,
            "colorpencil" | "colourpencil" => NprStyle::ColorPencil,
            "comic" | "ink" => NprStyle::ComicInk,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NprSettings {
    pub style: NprStyle,
    pub strength: f32,
    /// 0 = fine detail, 1 = big brushes.
    pub brush: f32,
    pub paper: f32,
    pub seed: u32,
    /// Brush sizes are multiplied by this (high-resolution export). Not saved.
    #[serde(skip, default = "one")]
    pub px_scale: f32,
}

impl Default for NprSettings {
    fn default() -> Self {
        Self {
            style: NprStyle::None,
            strength: 1.0,
            brush: 0.4,
            paper: 0.5,
            seed: 1,
            px_scale: 1.0,
        }
    }
}

/// A ruler guide: a vertical line at x = pos, or a horizontal one at y = pos.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Guide {
    pub vertical: bool,
    pub pos: f32,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Document {
    #[serde(default)]
    pub version: u32,
    pub width: u32,
    pub height: u32,
    pub background: Rgba,
    #[serde(default)]
    pub objects: Vec<Object>,
    #[serde(default)]
    pub npr: NprSettings,
    #[serde(default)]
    pub next_id: u64,
    #[serde(default)]
    pub guides: Vec<Guide>,
    /// Further looks painted on top of `npr` (e.g. ink outlines over watercolor).
    #[serde(default)]
    pub npr_stack: Vec<NprSettings>,
}

impl Document {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            version: 1,
            width,
            height,
            background: [255, 255, 255, 255],
            objects: Vec::new(),
            npr: NprSettings::default(),
            next_id: 1,
            guides: Vec::new(),
            npr_stack: Vec::new(),
        }
    }

    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id.max(1);
        self.next_id = id + 1;
        id
    }

    pub fn find(&self, id: u64) -> Option<&Object> {
        self.objects.iter().find(|o| o.id == id)
    }

    pub fn find_mut(&mut self, id: u64) -> Option<&mut Object> {
        self.objects.iter_mut().find(|o| o.id == id)
    }

    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.objects.iter().position(|o| o.id == id)
    }

    pub fn same_content(&self, o: &Document) -> bool {
        self.width == o.width
            && self.height == o.height
            && self.background == o.background
            && self.npr == o.npr
            && self.guides == o.guides
            && self.npr_stack == o.npr_stack
            && self.objects.len() == o.objects.len()
            && self
                .objects
                .iter()
                .zip(&o.objects)
                .all(|(a, b)| a.same_content(b))
    }
}

/// Paint layers are stored as base64 PNG inside the JSON document.
mod pixmap_serde {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};
    use std::sync::Arc;
    use tiny_skia::Pixmap;

    pub fn serialize<S: Serializer>(v: &Option<Arc<Pixmap>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            None => s.serialize_none(),
            Some(p) => {
                let png = p.encode_png().map_err(serde::ser::Error::custom)?;
                s.serialize_some(&base64::engine::general_purpose::STANDARD.encode(png))
            }
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Arc<Pixmap>>, D::Error> {
        let o: Option<String> = Option::deserialize(d)?;
        match o {
            None => Ok(None),
            Some(b) => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b)
                    .map_err(serde::de::Error::custom)?;
                let p = Pixmap::decode_png(&bytes).map_err(serde::de::Error::custom)?;
                Ok(Some(Arc::new(p)))
            }
        }
    }
}
