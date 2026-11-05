#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Jabro Draw — a hybrid vector + paint editor inspired by the classic "VectorBit" idea:
//! every object is an editable vector shape that also carries its own paint layer,
//! non-destructive material effects (bevel, shadow, glow, textures), a free perspective
//! quad, and the whole picture can be rendered through an NPR "look".

mod app;
mod brush;
mod demo;
mod doc;
mod effects;
mod fonts;
mod geom;
mod io;
mod npr;
mod raster;
mod render;
mod shapes;
mod styles;
mod textures;
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

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Jabro Draw")
            .with_inner_size([1500.0, 950.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Jabro Draw",
        options,
        Box::new(|cc| Ok(Box::new(app::JabroApp::new(cc)))),
    )
}
