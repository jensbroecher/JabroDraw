# Jabro Draw

A hybrid **vector + paint** graphics editor written in Rust. It is inspired by the classic
Real-DRAW PRO "VectorBit" idea:

- draw **vector shapes and text** that stay editable
- give them **materials**: bevels (round, chisel, cove, ridge, pillow), gloss, chrome reflection,
  procedural textures (wood, marble, stone, canvas, clouds…), drop shadows and glows
- **paint on them** with brushes. The paint is clipped to the shape and stays attached to it.
- bend any object into **perspective** by dragging its corners, and the paint follows
- render the whole picture through an **NPR look**: watercolor, oil paint, impressionist
  strokes, pointillism, pencil, colored pencil or comic ink

| Clean | Watercolor | Oil |
|---|---|---|
| ![](docs/look-none.png) | ![](docs/look-watercolor.png) | ![](docs/look-oil.png) |
| **Impressionist** | **Pencil** | **Comic ink** |
| ![](docs/look-impressionist.png) | ![](docs/look-pencil.png) | ![](docs/look-comic.png) |

## Build & run

Requires Rust (stable) and, on Windows, the MSVC build tools.

```bash
cargo run --release
```

The editor opens with an editable demo scene. Headless render of the demo scene (handy for tests):

```bash
cargo run --release -- --render-demo out.png watercolor
```

Styles: `none watercolor oil impressionist pointillism pencil colorpencil comic`.

## Using it

| Key | Tool |
|---|---|
| V | Select. Drag to move; handles scale (Shift keeps the aspect); the round handle rotates (Shift snaps to 15°). Shift+click or drag a box on empty canvas to select several objects. |
| A | Edit nodes. Drag nodes and Bézier handles (Alt breaks a smooth handle). Click a segment to add a node, double-click a node to switch smooth/corner, Del removes it. |
| P | Pen. Click for corners, click+drag for curves. Click the first node to close (fills it), Enter for an open stroke, Backspace removes the last node, Esc cancels. |
| Q | Perspective. Drag the 4 orange corners. |
| R / E / S | Rectangle / ellipse / star-polygon. Drag to draw, or click for a default size. |
| F | Freehand closed shape (becomes a smooth vector curve). |
| T | Text. Edit the text and font in the properties panel. |
| B | Brush. Paints into the selected object, or into a new paint layer. Alt+click picks a colour. |

- **Path menu**: Union / Subtract / Intersect / Exclude on 2+ selected objects (also in the properties
  panel). The result is one editable path that keeps the bottom object's style. **Convert to path**
  (Ctrl+Shift+C) turns rectangles, ellipses, stars and text into editable nodes, and keeps perspective.
- Mouse wheel zooms, middle-drag or Space+drag pans, and `0` fits the view.
- Ctrl+Z / Ctrl+Y undo/redo, Ctrl+D duplicates, Del deletes, Ctrl+S saves (`.jabro`), Ctrl+O opens, Ctrl+E exports a PNG.
- The **NPR look** bar at the top applies to the whole picture. The "painting…" spinner shows
  while the look renders in the background.

## Tests

```bash
cargo test --release
```

See [docs/DESIGN.md](docs/DESIGN.md) for the research notes on Real-DRAW PRO, the architecture
and the roadmap.
