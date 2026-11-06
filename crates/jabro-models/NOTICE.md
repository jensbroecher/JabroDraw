# Notice — embedded model weights

This crate embeds, unmodified:

| File | Model | Weights license | ONNX conversion |
|---|---|---|---|
| `models/u2net.onnx` (175,997,641 bytes) | U²-Net | Apache-2.0 — X. Qin et al., <https://github.com/xuebinqin/U-2-Net> | rembg, MIT — © 2020 Daniel Gatis |
| `models/u2netp.onnx` (4,574,861 bytes) | U²-Net small | Apache-2.0 — same authors | rembg, MIT |
| `models/mobile_sam_image_encoder.onnx` (28,157,093 bytes) | MobileSAM encoder | Apache-2.0 — C. Zhang et al., <https://github.com/ChaoningZhang/MobileSAM> | Acly, MIT — <https://huggingface.co/Acly/MobileSAM> |
| `models/sam_mask_decoder_single.onnx` (16,501,323 bytes) | Segment Anything mask decoder | Apache-2.0 — Meta AI | Acly, MIT |
| `models/realesr-general-x4v3-dn50.onnx` (4,858,936 bytes) | Real-ESRGAN general x4v3 | BSD-3-Clause — © 2021 Xintao Wang, <https://github.com/xinntao/Real-ESRGAN> | <https://huggingface.co/techycoder/realesrgan-general-x4v3>, BSD-3-Clause |

The license texts are in `../../licenses/`.
The files are not in git because they exceed GitHub's 100 MB limit. `build.rs` downloads them
once from where they are published (see `build.rs`) and checks their SHA-256.
