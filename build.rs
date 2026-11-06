//! Puts the program icon and version information into the Windows .exe (see tools/winres.rs).

#[path = "tools/winres.rs"]
mod winres;

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=tools/winres.rs");
    if !std::env::var("TARGET").unwrap_or_default().contains("windows-msvc") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("jabro-draw.res");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.1.0".into());
    winres::write(std::path::Path::new("assets/icon.ico"), &out, &version, "Jabro Draw", "jabro-draw.exe");
    println!("cargo:rustc-link-arg-bins={}", out.display());
}
