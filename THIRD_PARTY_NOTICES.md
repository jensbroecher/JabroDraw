# Third-party notices

Jabro Draw is built from open-source parts. All of them use permissive licenses that allow use,
modification and redistribution, including commercially, as long as their notices are kept.
This file is those notices. Ship it, together with the `licenses/` folder, with every copy of the
program you give away.

## Embedded AI model: U²-Net

The program contains the weights of the U²-Net salient-object detection network, which it uses
for **Remove background (AI)**.

- **U²-Net** by Xuebin Qin, Zichen Zhang, Chenyang Huang, Masood Dehghan, Osmar R. Zaiane and
  Martin Jagersand. *U²-Net: Going Deeper with Nested U-Structure for Salient Object Detection*,
  Pattern Recognition 106, 2020. <https://github.com/xuebinqin/U-2-Net>.
  Licensed under the **Apache License 2.0** (`licenses/Apache-2.0.txt`).
- **ONNX conversions** (`u2net.onnx`, `u2netp.onnx`) from the **rembg** project by Daniel Gatis,
  <https://github.com/danielgatis/rembg>. Licensed under the **MIT License**
  (`licenses/rembg-MIT.txt`).
- The weights are redistributed **unmodified** (SHA-256 checked at build time; see
  `crates/jabro-models/build.rs`).
- Inference runs locally. No picture is ever sent anywhere.

## Embedded AI models: MobileSAM and Segment Anything (object selection)

Used by the Brush tool's **AI object select**.

- **MobileSAM** image encoder by Chaoning Zhang, Dongshen Han, Yu Qiao, Jung Uk Kim, Sung-Ho Bae,
  Seungkyu Lee and Choong Seon Hong. *Faster Segment Anything: Towards Lightweight SAM for Mobile
  Applications*, 2023. <https://github.com/ChaoningZhang/MobileSAM>. Licensed under the **Apache
  License 2.0** (`licenses/Apache-2.0.txt`).
- **Segment Anything** mask decoder by Meta AI Research (Kirillov et al., 2023), as shipped with
  MobileSAM. <https://github.com/facebookresearch/segment-anything>. **Apache License 2.0**.
- **ONNX exports** (`mobile_sam_image_encoder.onnx`, `sam_mask_decoder_single.onnx`) by Acly,
  <https://huggingface.co/Acly/MobileSAM>. **MIT License** (`licenses/MobileSAM-ONNX-MIT.txt`).

## Embedded AI model: Real-ESRGAN (picture enlarging)

Used by **Enlarge with AI** in the Picture section.

- **Real-ESRGAN** general x4v3 by Xintao Wang, Liangbin Xie, Chao Dong and Ying Shan. *Real-ESRGAN:
  Training Real-World Blind Super-Resolution with Pure Synthetic Data*, ICCVW 2021.
  <https://github.com/xinntao/Real-ESRGAN>. Copyright (c) 2021, Xintao Wang. Licensed under the
  **BSD 3-Clause License** (`licenses/Real-ESRGAN-BSD-3-Clause.txt`).
- **ONNX export** (`realesr-general-x4v3-dn50.onnx`) of the official checkpoints, blended 50/50 as
  the official script does at its default denoise strength,
  <https://huggingface.co/techycoder/realesrgan-general-x4v3> (BSD 3-Clause).

All three files are redistributed **unmodified** (SHA-256 checked at build time). Like the
background removal, these models run on the computer; no picture is ever sent anywhere.

## Fonts

- Text objects use the fonts installed on your computer (for example Arial or Georgia). They are
  **not** shipped with Jabro Draw. Check a font's own license before you sell designs made with it;
  normal system fonts generally allow that.
- The user interface uses fonts bundled in the `egui` library: Ubuntu-Light (Ubuntu Font Licence
  1.0), Hack (MIT) and Noto Emoji / emoji-icon-font (SIL Open Font License 1.1).

## Rust libraries

The program links about 500 crates. Their licenses (checked with `cargo metadata`, 2026-11-06)
are all permissive:

| License | Notes |
|---|---|
| MIT and/or Apache-2.0 | the large majority (egui, eframe, tract, image, serde, i_overlay, …) |
| BSD-2-Clause / BSD-3-Clause | e.g. tiny-skia (BSD-3-Clause) |
| ISC, Zlib, BSL-1.0, 0BSD, Unlicense, CC0-1.0 | small helper crates |
| Unicode-3.0 | ICU4X data crates (Unicode data license, permissive) |
| CDLA-Permissive-2.0 | `webpki-roots` (root certificate list), only if TLS is ever used |
| `MIT OR Apache-2.0 OR LGPL-2.1-or-later` | `r-efi`: we use it under MIT / Apache-2.0 |

None of them is GPL or AGPL only. Before publishing a release, generate the full license texts
of every crate with [`cargo about`](https://github.com/EmbarkStudios/cargo-about) and include them:

```bash
cargo install cargo-about
cargo about generate about.hbs > licenses/third-party-crates.html
```

## Algorithms

The NPR looks, bevels and filters are original implementations of published techniques: the
Kuwahara filter, Hertzmann's painterly rendering, extended difference-of-Gaussians edges, the
guided filter (He et al.), Felzenszwalb's distance transform and value / Worley noise. No code or
assets from other drawing programs are used.

## Name

Jabro Draw is an independent program. It is inspired by the workflow of classic hybrid
vector/bitmap editors, but it does not use their code, artwork, UI design or trademarks. The
research notes in `docs/DESIGN.md` mention Real-DRAW PRO only to describe the features that
inspired this project. Any trademarks named there belong to their owners.
