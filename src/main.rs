#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Jabro Draw — a hybrid vector + paint editor: every object is an editable vector shape
//! that also carries its own paint layer, transparency mask, lights and other
//! non-destructive material effects (bevel, shadow, glow, textures), a free perspective
//! quad, and the whole picture can be rendered through an NPR "look".

mod ai;
mod anim;
mod app;
mod brush;
mod canvas;
mod demo;
mod doc;
mod effects;
mod export;
mod fonts;
mod geom;
mod icon;
mod imageops;
mod io;
mod npr;
mod raster;
mod render;
mod sam;
mod shapes;
mod styles;
mod textures;
mod trace;
mod upscale;
mod vector;
#[cfg(test)]
mod tests;

use eframe::egui;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    // Headless render of the demo scene: jabro-draw --render-demo out.png [npr-style]
    if args.len() >= 3 && args[1] == "--render-demo" {
        let style = args.get(3).map(|s| s.as_str()).unwrap_or("none");
        if let Err(e) = demo::render_demo_to_png(&args[2], style) {
            eprintln!("render failed: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }

    // Icon artwork: jabro-draw --make-icon assets
    if args.len() >= 3 && args[1] == "--make-icon" {
        if let Err(e) = icon::make_icon(std::path::Path::new(&args[2])) {
            eprintln!("icon failed: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }

    // Headless AI cut-out: jabro-draw --ai-cutout in.jpg out.png [fast|best]
    if args.len() >= 4 && args[1] == "--ai-cutout" {
        let model = if args.get(4).map(|s| s.as_str()) == Some("fast") { ai::AiModel::Fast } else { ai::AiModel::Best };
        if let Err(e) = ai::cutout_file(&args[2], &args[3], model) {
            eprintln!("cut-out failed: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Jabro Draw")
        .with_inner_size([1500.0, 950.0])
        .with_min_inner_size([900.0, 600.0])
        .with_drag_and_drop(true);
    if let Ok(img) = image::load_from_memory(include_bytes!("../assets/icon-256.png")) {
        let img = img.to_rgba8();
        viewport = viewport.with_icon(egui::IconData { width: img.width(), height: img.height(), rgba: img.into_raw() });
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "Jabro Draw",
        options,
        Box::new(|cc| Ok(Box::new(app::JabroApp::new(cc)))),
    )
}
