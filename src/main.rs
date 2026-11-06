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
#[path = "../tools/install.rs"]
mod install;
mod imageops;
mod io;
mod library;
mod npr;
mod raster;
mod render;
mod sam;
mod shapes;
mod styles;
mod textures;
mod trace;
mod upscale;
mod wic;
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

    // Remove the installed program (Settings > Apps runs this)
    if args.len() >= 2 && args[1] == "--uninstall" {
        uninstall(args.iter().any(|a| a == "--quiet"));
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

    // a drawing or picture double-clicked in Explorer
    let start_file = args.get(1).filter(|a| !a.starts_with("--")).map(std::path::PathBuf::from).filter(|p| p.is_file());
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
        Box::new(move |cc| {
            let mut app = app::JabroApp::new(cc);
            if let Some(p) = start_file {
                app.open_at_start(p);
            }
            Ok(Box::new(app))
        }),
    )
}

/// `--uninstall`: remove shortcuts, the Settings > Apps entry, the file type and the program
/// folder (only if our setup made it). Drawings and settings are left alone.
fn uninstall(quiet: bool) {
    let ask = |text: &str| {
        quiet
            || rfd::MessageDialog::new()
                .set_title("Uninstall Jabro Draw")
                .set_description(text)
                .set_buttons(rfd::MessageButtons::YesNo)
                .show()
                == rfd::MessageDialogResult::Yes
    };
    if !ask("Remove Jabro Draw from this computer?\n\nYour drawings are not touched.") {
        return;
    }
    install::remove_shortcuts();
    install::unregister();
    let dir = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf()));
    let removed = dir.as_deref().is_some_and(|d| install::delete_dir_after_exit(d).is_ok());
    if !quiet {
        let text = if removed {
            "Jabro Draw has been removed."
        } else {
            "Shortcuts and the Settings entry are removed. This copy was not installed by the setup, so its files were left in place."
        };
        rfd::MessageDialog::new().set_title("Uninstall Jabro Draw").set_description(text).show();
    }
}
