//! One-click material presets ("style library").

use crate::doc::*;

pub type Preset = (&'static str, fn(&mut Object));

pub const PRESETS: &[Preset] = &[
    ("Gold", gold),
    ("Chrome", chrome),
    ("Glass", glass),
    ("Candy plastic", plastic),
    ("Wood", wood),
    ("Stone", stone),
    ("Marble", marble),
    ("Neon", neon),
    ("Plain", plain),
];

fn bevel_width(o: &Object) -> f32 {
    (o.size[0].min(o.size[1]) * 0.07).clamp(3.0, 14.0)
}

fn drop_shadow(o: &mut Object, opacity: f32) {
    o.effects.shadow = Shadow {
        enabled: true,
        opacity,
        ..Default::default()
    };
}

pub fn gold(o: &mut Object) {
    o.fill = FillStyle {
        kind: FillKind::Linear,
        a: [255, 228, 125, 255],
        b: [190, 120, 20, 255],
        angle: 90.0,
        ..o.fill.clone()
    };
    o.stroke.enabled = false;
    o.effects.bevel = Bevel {
        enabled: true,
        profile: BevelProfile::Round,
        width: bevel_width(o),
        depth: 0.9,
        softness: 1.0,
        intensity: 0.85,
        highlight: [255, 252, 225, 255],
        shadow: [95, 50, 0, 255],
        gloss: 0.8,
        shininess: 28.0,
        reflection: 0.35,
        ..o.effects.bevel.clone()
    };
    drop_shadow(o, 0.45);
    o.effects.glow.enabled = false;
}

pub fn chrome(o: &mut Object) {
    o.fill = FillStyle {
        kind: FillKind::Linear,
        a: [235, 238, 245, 255],
        b: [150, 155, 165, 255],
        angle: 90.0,
        ..o.fill.clone()
    };
    o.stroke.enabled = false;
    o.effects.bevel = Bevel {
        enabled: true,
        profile: BevelProfile::Round,
        width: bevel_width(o) * 1.2,
        depth: 1.0,
        softness: 1.0,
        intensity: 0.5,
        highlight: [255, 255, 255, 255],
        shadow: [20, 20, 30, 255],
        gloss: 0.9,
        shininess: 40.0,
        reflection: 0.9,
        ..o.effects.bevel.clone()
    };
    drop_shadow(o, 0.45);
    o.effects.glow.enabled = false;
}

pub fn glass(o: &mut Object) {
    o.fill = FillStyle {
        kind: FillKind::Radial,
        a: [190, 230, 255, 120],
        b: [40, 120, 220, 200],
        ..o.fill.clone()
    };
    o.stroke.enabled = false;
    o.effects.bevel = Bevel {
        enabled: true,
        profile: BevelProfile::Pillow,
        depth: 0.35,
        softness: 1.5,
        intensity: 0.6,
        highlight: [255, 255, 255, 255],
        shadow: [0, 30, 90, 255],
        gloss: 1.0,
        shininess: 60.0,
        reflection: 0.25,
        ..o.effects.bevel.clone()
    };
    drop_shadow(o, 0.25);
    o.effects.glow.enabled = false;
}

pub fn plastic(o: &mut Object) {
    let base = if o.fill.kind == FillKind::None { [230, 40, 90, 255] } else { o.fill.a };
    o.fill = FillStyle {
        kind: FillKind::Solid,
        a: base,
        ..o.fill.clone()
    };
    o.stroke.enabled = false;
    o.effects.bevel = Bevel {
        enabled: true,
        profile: BevelProfile::Pillow,
        depth: 0.25,
        softness: 1.5,
        intensity: 0.7,
        highlight: [255, 255, 255, 255],
        shadow: [0, 0, 0, 255],
        gloss: 0.95,
        shininess: 35.0,
        reflection: 0.0,
        ..o.effects.bevel.clone()
    };
    drop_shadow(o, 0.4);
    o.effects.glow.enabled = false;
}

pub fn wood(o: &mut Object) {
    o.fill = FillStyle {
        kind: FillKind::Texture,
        a: [105, 58, 24, 255],
        b: [196, 132, 70, 255],
        texture: TextureKind::Wood,
        tex_scale: 1.0,
        ..o.fill.clone()
    };
    o.stroke = StrokeStyle {
        enabled: true,
        color: [70, 38, 15, 255],
        width: 3.0,
    };
    o.effects.bevel = Bevel {
        enabled: true,
        profile: BevelProfile::Chisel,
        width: bevel_width(o),
        depth: 0.7,
        softness: 0.8,
        intensity: 0.75,
        highlight: [255, 230, 190, 255],
        shadow: [40, 20, 5, 255],
        gloss: 0.15,
        shininess: 12.0,
        reflection: 0.0,
        ..o.effects.bevel.clone()
    };
    drop_shadow(o, 0.5);
    o.effects.glow.enabled = false;
}

pub fn stone(o: &mut Object) {
    o.fill = FillStyle {
        kind: FillKind::Texture,
        a: [70, 68, 64, 255],
        b: [190, 186, 176, 255],
        texture: TextureKind::Cells,
        tex_scale: 1.0,
        ..o.fill.clone()
    };
    o.stroke.enabled = false;
    o.effects.bevel = Bevel {
        enabled: true,
        profile: BevelProfile::Chisel,
        width: bevel_width(o) * 1.2,
        depth: 0.8,
        softness: 1.0,
        intensity: 0.9,
        highlight: [255, 255, 250, 255],
        shadow: [0, 0, 0, 255],
        gloss: 0.05,
        shininess: 8.0,
        reflection: 0.0,
        ..o.effects.bevel.clone()
    };
    drop_shadow(o, 0.55);
    o.effects.glow.enabled = false;
}

pub fn marble(o: &mut Object) {
    o.fill = FillStyle {
        kind: FillKind::Texture,
        a: [95, 100, 115, 255],
        b: [248, 246, 242, 255],
        texture: TextureKind::Marble,
        tex_scale: 1.0,
        ..o.fill.clone()
    };
    o.stroke.enabled = false;
    o.effects.bevel = Bevel {
        enabled: true,
        profile: BevelProfile::Round,
        width: bevel_width(o) * 0.8,
        depth: 0.6,
        softness: 1.0,
        intensity: 0.6,
        highlight: [255, 255, 255, 255],
        shadow: [30, 30, 40, 255],
        gloss: 0.6,
        shininess: 30.0,
        reflection: 0.1,
        ..o.effects.bevel.clone()
    };
    drop_shadow(o, 0.4);
    o.effects.glow.enabled = false;
}

pub fn neon(o: &mut Object) {
    let c = if o.fill.kind == FillKind::None { [80, 255, 220, 255] } else { o.fill.a };
    o.fill = FillStyle {
        kind: FillKind::Solid,
        a: [
            (c[0] as u16 / 2 + 128) as u8,
            (c[1] as u16 / 2 + 128) as u8,
            (c[2] as u16 / 2 + 128) as u8,
            255,
        ],
        ..o.fill.clone()
    };
    o.stroke.enabled = false;
    o.effects.bevel = Bevel {
        enabled: true,
        profile: BevelProfile::Round,
        width: bevel_width(o) * 0.6,
        depth: 0.5,
        intensity: 0.3,
        gloss: 0.7,
        shininess: 20.0,
        reflection: 0.0,
        ..o.effects.bevel.clone()
    };
    o.effects.shadow.enabled = false;
    o.effects.glow = Glow {
        enabled: true,
        color: c,
        size: 22.0,
        opacity: 0.95,
    };
}

pub fn plain(o: &mut Object) {
    o.effects = Effects::default();
    o.stroke.enabled = false;
}
