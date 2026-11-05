//! Makes sure the model weights are present and genuine before they get embedded.
//! They are too big for git (GitHub's limit is 100 MB per file), so a fresh checkout downloads
//! them once from the rembg project's release page and checks their SHA-256.

use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

const BASE: &str = "https://github.com/danielgatis/rembg/releases/download/v0.0.0/";
const MODELS: &[(&str, u64, &str)] = &[
    ("u2netp.onnx", 4_574_861, "309c8469258dda742793dce0ebea8e6dd393174f89934733ecc8b14c76f4ddd8"),
    ("u2net.onnx", 175_997_641, "8d10d2f3bb75ae3b6d527c77944fc5e7dcd94b29809d47a739a7a728a912b491"),
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
    for (name, size, digest) in MODELS {
        let path = dir.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        if path.metadata().map(|m| m.len()).ok() != Some(*size) {
            let part = dir.join(format!("{name}.part"));
            let url = format!("{BASE}{name}");
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
