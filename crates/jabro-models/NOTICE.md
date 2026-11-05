# Notice — embedded model weights

This crate embeds, unmodified:

| File | Model | Weights license | ONNX conversion |
|---|---|---|---|
| `models/u2net.onnx` (175,997,641 bytes) | U²-Net | Apache-2.0 — X. Qin et al., <https://github.com/xuebinqin/U-2-Net> | rembg, MIT — © 2020 Daniel Gatis |
| `models/u2netp.onnx` (4,574,861 bytes) | U²-Net small | Apache-2.0 — same authors | rembg, MIT |

The license texts are in `../../licenses/` (`Apache-2.0.txt` and `rembg-MIT.txt`).
The files are not in git because they exceed GitHub's 100 MB limit. `build.rs` downloads them
once from <https://github.com/danielgatis/rembg/releases/tag/v0.0.0> and checks their SHA-256.
