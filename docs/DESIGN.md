# Jabro Draw — design notes

## 1. What Real-DRAW PRO did (research summary)

Sources: mediachance.com help/feature pages, the Alphr review of v5 and forum snippets.
Archived pages could not be fetched, so items marked *(unconfirmed)* are inferred.

| Area | Real-DRAW PRO |
|---|---|
| Object model | **"VectorBit"**: every object is vector-editable (shape, order, resize, envelope) but carries bitmap-based properties. You could paint with brushes directly on vector objects; strokes were masked by the shape, and strokes could themselves become objects with effects. Groups were called **Packages** (nestable, effects on the whole package, clones). There was no classic layer panel. |
| Effects | Non-destructive per-object properties: colour, texture, outline, lights, material, transparency (incl. texture transparency), 3D bevel & extrude, drop shadow, glow, motion blur, sparkle, lens flare, bump mapping, per-object colour correction. Multiple point / spot / distant lights per object + material (metallic). Texture library + GenetX procedural textures, "area gradients". |
| Transform | Perspective transformation, envelope editing, "Liquid shape" brushes (move/grow/shrink/rotate), anti-aliased rotation, texture rotation/perspective. Mesh warp *(unconfirmed)*. |
| NPR | "Non-Photo-Realistic" styles that change the *drawing tool* the picture is rendered with: chalk, paintbrush, pencil, watercolor, engraving, oils, ink, crayon, sketch. A global style by default, overridable **per object / per package**, adjustable strength + fine-tune dialog, non-destructive, quick style library. It worked on the VectorBit data rather than being a flat post-filter. |
| Painting | Airbrushes, 3D brushes, artistic brushes, image nozzles (hose), cloning, blend modes. Tablet pressure *(unconfirmed)*. |
| Other | Text with all effects, photo tools (saturation, sharpen, smart crop), "Mega Render" (huge output from a screen-sized composition), web slicer/rollover buttons, PSD export with layers, SVG/PNG/JPG export, editable "E-JPG". |
| Praise / complaints | Praised for its hand-made NPR look, easy realistic metal, and the "no layer thinking" workflow. Criticised for its dated UI and narrower scope than Xara. It is now 32-bit and effectively unmaintained. |

## 2. Jabro Draw concept

Keep the soul (hybrid objects, material effects, paint-on-vector, perspective, NPR),
modernise the engine (64-bit, Rust, non-destructive everywhere, procedural assets, GPU-ready).

### The hybrid object

```
Object
 ├─ kind        vector geometry in local space (rect, ellipse, star, freehand curve, text, bitmap)
 ├─ fill        solid / linear / radial / procedural texture
 ├─ paint       optional RGBA paint layer in the object's own local pixels (clipped by the shape)
 ├─ stroke      outline
 ├─ effects     bevel + material (profile, lighting, gloss, chrome env), drop shadow, outer glow
 └─ quad        where the local rectangle lands on the canvas: 4 free corners
                => move / scale / rotate / shear / full perspective, paint follows automatically
```

### Render pipeline (`render.rs`)

1. **Local raster** (cached by content revision + light direction): fill → paint layer
   masked by the shape → outline → bevel/material lighting (exact distance transform →
   height profile → normals → diffuse/specular/environment).
2. **Canvas layer** (cached by placement): projective warp of the local raster with the
   quad's homography → drop shadow + glow computed in canvas space, so they always fall
   the same way whatever the object's rotation.
3. **Composite** in z-order onto the background.
4. **NPR look** on a background thread (`npr.rs`). While you drag, the clean render is
   shown; the painted version arrives when you let go.

Because shapes are stored as vectors and re-rasterised when their size changes, scaling stays crisp.
Moving or rotating only re-warps the cached raster.

### Module map

| File | Role |
|---|---|
| `doc.rs` | data model, serde (.jabro = JSON + base64 PNG paint layers) |
| `geom.rs` | points, homography (perspective), polygon tests, RDP simplify |
| `shapes.rs` / `fonts.rs` | vector paths incl. text outlines from system fonts |
| `render.rs` | object pipeline + caches, hit testing |
| `effects.rs` | bevel/material, chrome environment, shadow, glow, EDT |
| `textures.rs` | procedural noise textures (clouds, wood, marble, canvas, paper, metal, stone) |
| `brush.rs` | stamp brush engine (round, airbrush, chalk, smudge, eraser) |
| `npr.rs` | watercolor, oil, impressionist, pointillism, pencil, colored pencil, comic ink |
| `styles.rs` | one-click material presets |
| `app.rs` | egui UI, tools, undo/redo |

## 3. Status of v0.1 (this prototype)

Done: rect / ellipse / star / freehand / text objects; solid, gradient and procedural texture fills;
outline; 5 bevel profiles with lighting, gloss and chrome reflection; drop shadow and glow;
paint on any object (clipped), or on free paint layers that can themselves be beveled;
move, scale, rotate, free perspective; 9 style presets plus copy/paste style; 7 NPR looks with
strength, brush size and paper controls; undo/redo; .jabro save/load; PNG export;
pen pressure (if the tablet driver reports it through Windows touch/pen events); headless
`--render-demo` for testing.

## 4. Roadmap

### Phase 2: editing depth
- ✅ **v0.2** Bézier pen tool, node editing (drag nodes and handles, insert on a segment,
  smooth/corner, delete), convert any shape or text to a path, multi-selection (Shift+click, marquee)
  with group move, and boolean union / subtract / intersect / exclude via `i_overlay`.
  - Paths store nodes normalised to the object's local box (`vector.rs`). After each edit
    `normalize_bounds` refits the box to the path's tight bounds. Because the new quad comes from
    the old homography, nothing moves on the canvas even in perspective, and paint layers are
    re-mapped along with it.
  - Booleans run on outlines flattened into canvas space, so perspective and rotation are respected.
    The resulting polygons are simplified (Douglas-Peucker) and rebuilt as smooth Bézier nodes where
    the outline turns gently, with corners where it turns sharply.
  - Still to do: true curve fitting for boolean results; booleans that keep paint layers; a
    continue-path pen mode; join/break nodes; stroke-to-path.
- **Packages** (groups) that take effects and NPR as a unit, plus clones/instances.
- **Per-object NPR override** (Real-DRAW's best trick): each object chooses "inherit / none / style X".
- Envelope + mesh warp in addition to the 4-corner perspective.
- Multi-stop gradient editor, transparency gradients, blend modes, inner shadow / inner glow,
  multiple coloured lights, bump map from texture.
- Import PNG / JPG as bitmap objects; use any image as a texture.

### Phase 3: painting
- Proper Wintab / Windows Ink pressure + tilt.
- Image-hose (nozzle) brushes, clone brush, *liquify* ("liquid shape") brushes.
- A wet watercolor brush with pigment flow on a paper height field.
- Brush presets, symmetry/mirror painting, lazy-mouse stabiliser.

### Phase 4: output & speed
- GPU path (wgpu compute) for blur, bevel and NPR, with a live NPR preview while dragging.
- **Mega render**: export at 2–8× resolution, since everything except paint layers is resolution-independent.
- SVG export of vector parts, PSD export with one layer per object (+ shadow layers), layered PNG.

### Own ideas beyond Real-DRAW
- **NPR stacks**: combine looks (e.g. watercolor wash + ink outlines), with per-object strength.
- **Painting replay**: record the impressionist / watercolor stroke generation and export a
  time-lapse video of the picture "being painted".
- **Paper as a document property**: one paper texture drives brush grain, watercolor granulation
  and the NPR looks consistently.
- **Procedural texture graph**: noise nodes mixed visually, saved in the style library.
- **Style library with thumbnails** rendered by the engine itself; share styles as small JSON files.
- **3D extrude** objects with simple lighting, consistent with the bevel light.
- **Scripting** (Rhai) for generative art and batch jobs.
