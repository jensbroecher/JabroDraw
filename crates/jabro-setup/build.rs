//! Embeds the finished Jabro Draw program (built with `cargo build --release` first, or the file
//! named by JABRO_APP_EXE) and gives the setup its icon and version information.

#[path = "../../tools/winres.rs"]
mod winres;

use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    println!("cargo:rerun-if-env-changed=JABRO_APP_EXE");
    let exe = std::env::var_os("JABRO_APP_EXE").map(PathBuf::from).unwrap_or_else(|| root.join("target/release/jabro-draw.exe"));
    if !exe.is_file() {
        panic!("{} is missing: build Jabro Draw first with `cargo build --release`", exe.display());
    }
    let exe = exe.canonicalize().expect("payload path");
    println!("cargo:rerun-if-changed={}", exe.display());
    println!("cargo:rustc-env=JABRO_PAYLOAD_EXE={}", exe.display());
    // the version of the program being installed
    let toml = std::fs::read_to_string(root.join("Cargo.toml")).expect("root Cargo.toml");
    let version = toml
        .lines()
        .find_map(|l| l.strip_prefix("version = \"").and_then(|v| v.strip_suffix('"')))
        .unwrap_or("0.1.0")
        .to_string();
    println!("cargo:rustc-env=JABRO_VERSION={version}");
    println!("cargo:rerun-if-changed=../../Cargo.toml");
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    if std::env::var("TARGET").unwrap_or_default().contains("windows-msvc") {
        let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("setup.res");
        winres::write(&root.join("assets/icon.ico"), &out, &version, "Jabro Draw Setup", "jabro-draw-setup.exe");
        println!("cargo:rustc-link-arg-bins={}", out.display());
    }
}
