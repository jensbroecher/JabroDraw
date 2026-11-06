//! U²-Net salient-object segmentation weights, embedded into Jabro Draw so AI background
//! removal works offline without any separate download.
//!
//! * U²-Net: Xuebin Qin, Zichen Zhang, Chenyang Huang, Masood Dehghan, Osmar R. Zaiane,
//!   Martin Jagersand — "U²-Net: Going Deeper with Nested U-Structure for Salient Object
//!   Detection", Pattern Recognition 2020. Code and weights: https://github.com/xuebinqin/U-2-Net,
//!   licensed under the Apache License 2.0.
//! * ONNX conversions: the rembg project, https://github.com/danielgatis/rembg (MIT).
//!
//! The weights are redistributed unmodified; see NOTICE.md in this crate.

/// Small model (u2netp, 4.6 MB): fast, a bit rougher.
pub static U2NETP: &[u8] = include_bytes!("../models/u2netp.onnx");

/// Full model (u2net, 176 MB): best quality.
pub static U2NET: &[u8] = include_bytes!("../models/u2net.onnx");

/// MobileSAM image encoder (28 MB): turns a 1024 x 1024 picture into an embedding that the
/// decoder can cut objects out of. MobileSAM by Chaoning Zhang et al.,
/// https://github.com/ChaoningZhang/MobileSAM (Apache-2.0); ONNX export by Acly,
/// https://huggingface.co/Acly/MobileSAM (MIT).
pub static SAM_ENCODER: &[u8] = include_bytes!("../models/mobile_sam_image_encoder.onnx");

/// Segment Anything mask decoder (16.5 MB, single-mask variant): clicked points -> object mask.
/// Segment Anything by Meta AI (Apache-2.0), as shipped with MobileSAM; ONNX export by Acly (MIT).
pub static SAM_DECODER: &[u8] = include_bytes!("../models/sam_mask_decoder_single.onnx");

/// Real-ESRGAN general x4v3 (4.9 MB, SRVGGNetCompact, denoise 0.5): 4x picture upscaling.
/// Real-ESRGAN by Xintao Wang et al., https://github.com/xinntao/Real-ESRGAN (BSD-3-Clause);
/// ONNX export from the official checkpoints, https://huggingface.co/techycoder/realesrgan-general-x4v3.
pub static ESRGAN_X4: &[u8] = include_bytes!("../models/realesr-general-x4v3-dn50.onnx");
