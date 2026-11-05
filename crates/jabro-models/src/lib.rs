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
