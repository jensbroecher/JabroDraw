//! Makes sure the model weights are present and genuine before they get embedded.
//! They are too big for git (GitHub's limit is 100 MB per file), so a fresh checkout downloads
//! them once from where they are published and checks their SHA-256.

use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

const REMBG: &str = "https://github.com/danielgatis/rembg/releases/download/v0.0.0/";
const SAM: &str = "https://huggingface.co/Acly/MobileSAM/resolve/main/";
const ESRGAN: &str = "https://huggingface.co/techycoder/realesrgan-general-x4v3/resolve/main/";

/// (file, where it is published, size in bytes, SHA-256)
const MODELS: &[(&str, &str, u64, &str)] = &[
    ("u2netp.onnx", REMBG, 4_574_861, "309c8469258dda742793dce0ebea8e6dd393174f89934733ecc8b14c76f4ddd8"),
    ("u2net.onnx", REMBG, 175_997_641, "8d10d2f3bb75ae3b6d527c77944fc5e7dcd94b29809d47a739a7a728a912b491"),
    ("mobile_sam_image_encoder.onnx", SAM, 28_157_093, "580f5fb648ea1062c0aabc26217aed56921985f03f0cbbd852bba81d760cc749"),
    ("sam_mask_decoder_single.onnx", SAM, 16_501_323, "93915fc7c993ab9d59ab8c9ccd3bce37f7509c81ab4150a74abd4d2abbd8570d"),
    ("realesr-general-x4v3-dn50.onnx", ESRGAN, 4_858_936, "152a44621a7906878d0a580c0347e8bcce78bafeb949af99383fba13e81f253a"),
];

fn sha256(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn main() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("models");
    std::fs::create_dir_all(&dir).expect("models folder");
    for (name, base, size, digest) in MODELS {
        let path = dir.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        if path.metadata().map(|m| m.len()).ok() != Some(*size) {
            let part = dir.join(format!("{name}.part"));
            let url = format!("{base}{name}");
            println!("cargo:warning=downloading {name} ({} MB) from {url}", size / 1_000_000);
            let ok = Command::new("curl")
                .args(["-sSfL", "-o"])
                .arg(&part)
                .arg(&url)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok {
                panic!("could not download {url} — put {name} into {} by hand", dir.display());
            }
            std::fs::rename(&part, &path).expect("rename model");
        }
        let got = sha256(&path).expect("read model");
        if got != *digest {
            let _ = std::fs::remove_file(&path);
            panic!("{name} has the wrong checksum ({got}); deleted it — build again to re-download");
        }
    }
}
