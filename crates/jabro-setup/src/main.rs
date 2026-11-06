#![windows_subsystem = "windows"]

//! Jabro Draw Setup: a single .exe that carries the program and installs it for the current
//! Windows user (no administrator rights needed).
//!
//! `jabro-draw-setup --silent [--dir <folder>] [--no-shortcuts] [--no-desktop]
//! [--no-associate] [--no-register]` installs without asking (exit code 0 = done).

#[path = "../../../tools/install.rs"]
mod install;

use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

const VERSION: &str = env!("JABRO_VERSION");
const APP_EXE: &[u8] = include_bytes!(env!("JABRO_PAYLOAD_EXE"));
/// Notices and license texts that must ship with the program, plus the manual.
const FILES: &[(&str, &[u8])] = &[
    ("README.md", include_bytes!("../../../README.md")),
    ("LICENSE.txt", include_bytes!("../../../LICENSE")),
    ("THIRD_PARTY_NOTICES.md", include_bytes!("../../../THIRD_PARTY_NOTICES.md")),
    ("licenses/Apache-2.0.txt", include_bytes!("../../../licenses/Apache-2.0.txt")),
    ("licenses/rembg-MIT.txt", include_bytes!("../../../licenses/rembg-MIT.txt")),
    ("licenses/MobileSAM-ONNX-MIT.txt", include_bytes!("../../../licenses/MobileSAM-ONNX-MIT.txt")),
    ("licenses/Real-ESRGAN-BSD-3-Clause.txt", include_bytes!("../../../licenses/Real-ESRGAN-BSD-3-Clause.txt")),
];
const ICON_PNG: &[u8] = include_bytes!("../../../assets/icon-256.png");

#[derive(Clone)]
struct Choices {
    dir: PathBuf,
    shortcuts: bool,
    desktop: bool,
    associate: bool,
    register: bool,
}

/// Copy the program into `c.dir` and set up shortcuts and the Settings > Apps entry.
fn install(c: &Choices) -> Result<(), String> {
    let dir = &c.dir;
    std::fs::create_dir_all(dir.join("licenses")).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let exe = dir.join(install::EXE_NAME);
    let part = dir.join(format!("{}.new", install::EXE_NAME));
    std::fs::write(&part, APP_EXE).map_err(|e| format!("could not write {}: {e}", part.display()))?;
    if let Err(e) = std::fs::rename(&part, &exe) {
        let _ = std::fs::remove_file(&part);
        return Err(if e.kind() == std::io::ErrorKind::PermissionDenied {
            "Jabro Draw is running — close it and try again.".into()
        } else {
            format!("could not place {}: {e}", exe.display())
        });
    }
    for (name, bytes) in FILES {
        std::fs::write(dir.join(name), bytes).map_err(|e| format!("could not write {name}: {e}"))?;
    }
    std::fs::write(dir.join(install::MARKER), format!("Jabro Draw {VERSION}\n")).map_err(|e| e.to_string())?;
    if c.shortcuts {
        install::create_shortcuts(&exe, c.desktop, None)?;
    }
    if c.register {
        install::register(dir, VERSION, c.associate)?;
    }
    Ok(())
}

/// The folder a picked parent folder should get: `<picked>\Jabro Draw` unless it already is one.
fn install_dir_in(picked: &Path) -> PathBuf {
    if picked.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(install::APP_NAME)) {
        picked.to_path_buf()
    } else {
        picked.join(install::APP_NAME)
    }
}

enum Page {
    Ask,
    Working(Receiver<Result<(), String>>),
    Done,
    Failed(String),
}

struct Setup {
    c: Choices,
    page: Page,
    existing: bool,
    icon: Option<egui::TextureHandle>,
}

impl eframe::App for Setup {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.icon.is_none() {
            if let Ok(img) = image::load_from_memory(ICON_PNG) {
                let img = img.resize(96, 96, image::imageops::FilterType::Lanczos3).to_rgba8();
                let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
                self.icon = Some(ctx.load_texture("icon", ci, egui::TextureOptions::LINEAR));
            }
        }
        if let Page::Working(rx) = &self.page {
            if let Ok(res) = rx.try_recv() {
                self.page = match res {
                    Ok(()) => Page::Done,
                    Err(e) => Page::Failed(e),
                };
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if let Some(t) = &self.icon {
                    ui.image((t.id(), egui::vec2(72.0, 72.0)));
                }
                ui.vertical(|ui| {
                    ui.add_space(10.0);
                    ui.heading(format!("Jabro Draw {VERSION}"));
                    ui.label("Vector shapes you can paint on, bevel, bend into perspective and render as watercolor, oil or pencil.");
                });
            });
            ui.add_space(12.0);
            match &self.page {
                Page::Ask => {
                    ui.label("Install to:");
                    ui.horizontal(|ui| {
                        let mut s = self.c.dir.to_string_lossy().to_string();
                        if ui.add(egui::TextEdit::singleline(&mut s).desired_width(380.0)).changed() {
                            self.c.dir = PathBuf::from(s);
                        }
                        if ui.button("Change…").clicked() {
                            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                                self.c.dir = install_dir_in(&p);
                            }
                        }
                    });
                    ui.add_space(6.0);
                    ui.checkbox(&mut self.c.desktop, "Desktop shortcut");
                    ui.checkbox(&mut self.c.associate, "Open .jabro drawings with Jabro Draw");
                    ui.add_space(6.0);
                    ui.small("Installs just for you, with no administrator rights needed, and uses about 230 MB. Remove it any time in Settings › Apps.");
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        let label = if self.existing { "Update" } else { "Install" };
                        if ui.add(egui::Button::new(egui::RichText::new(label).strong()).min_size(egui::vec2(110.0, 30.0))).clicked() {
                            let (tx, rx) = channel();
                            let c = self.c.clone();
                            let ctx2 = ctx.clone();
                            std::thread::spawn(move || {
                                let _ = tx.send(install(&c));
                                ctx2.request_repaint();
                            });
                            self.page = Page::Working(rx);
                        }
                        if ui.add(egui::Button::new("Cancel").min_size(egui::vec2(90.0, 30.0))).clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                }
                Page::Working(_) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Installing…");
                    });
                }
                Page::Done => {
                    ui.label(egui::RichText::new("Jabro Draw is installed.").strong());
                    ui.label("You find it in the Start menu. Have fun!");
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(egui::RichText::new("Start Jabro Draw").strong()).min_size(egui::vec2(140.0, 30.0))).clicked() {
                            let _ = std::process::Command::new(self.c.dir.join(install::EXE_NAME)).current_dir(&self.c.dir).spawn();
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if ui.add(egui::Button::new("Close").min_size(egui::vec2(90.0, 30.0))).clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                }
                Page::Failed(e) => {
                    ui.colored_label(egui::Color32::from_rgb(255, 120, 110), "The installation did not finish:");
                    ui.label(e.clone());
                    ui.add_space(12.0);
                    if ui.button("Back").clicked() {
                        self.page = Page::Ask;
                    }
                }
            }
        });
    }
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let existing_dir = install::installed_dir().filter(|d| install::is_install_dir(d));
    let dir = args
        .iter()
        .position(|a| a == "--dir")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .or(existing_dir.clone())
        .unwrap_or_else(install::default_dir);
    let c = Choices {
        dir,
        shortcuts: !has("--no-shortcuts"),
        desktop: !has("--no-desktop"),
        associate: !has("--no-associate"),
        register: !has("--no-register"),
    };
    if has("--silent") {
        std::process::exit(match install(&c) {
            Ok(()) => 0,
            Err(_) => 1,
        });
    }
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Jabro Draw Setup")
        .with_inner_size([600.0, 330.0])
        .with_resizable(false);
    if let Ok(img) = image::load_from_memory(ICON_PNG) {
        let img = img.to_rgba8();
        viewport = viewport.with_icon(egui::IconData { width: img.width(), height: img.height(), rgba: img.into_raw() });
    }
    eframe::run_native(
        "Jabro Draw Setup",
        eframe::NativeOptions { viewport, ..Default::default() },
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(Setup { c, page: Page::Ask, existing: existing_dir.is_some(), icon: None }))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("jabro-setup-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn installs_the_program_and_its_notices_into_a_folder() {
        let dir = scratch("files");
        let c = Choices { dir: dir.clone(), shortcuts: false, desktop: false, associate: false, register: false };
        install(&c).unwrap();
        assert_eq!(std::fs::metadata(dir.join(install::EXE_NAME)).unwrap().len(), APP_EXE.len() as u64);
        for (name, bytes) in FILES {
            assert_eq!(std::fs::read(dir.join(name)).unwrap(), *bytes, "{name}");
        }
        assert!(install::is_install_dir(&dir));
        // installing again (an update) works over the old copy
        install(&c).unwrap();
        // the uninstaller removes the folder once the program has exited
        install::delete_dir_after_exit(&dir).unwrap();
        let start = std::time::Instant::now();
        while dir.exists() && start.elapsed().as_secs() < 20 {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        assert!(!dir.exists(), "install folder removed");
    }

    #[test]
    fn the_uninstaller_never_deletes_a_folder_it_did_not_make() {
        let dir = scratch("foreign");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(install::EXE_NAME), b"not ours").unwrap();
        assert!(install::delete_dir_after_exit(&dir).is_err(), "no marker, no deleting");
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(dir.join(install::EXE_NAME).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shortcuts_point_at_the_program() {
        let dir = scratch("links");
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join(install::EXE_NAME);
        std::fs::write(&exe, b"x").unwrap();
        install::create_shortcuts(&exe, false, Some(&dir)).unwrap();
        let lnk = dir.join(format!("{}.lnk", install::APP_NAME));
        let bytes = std::fs::read(&lnk).expect("shortcut written");
        assert_eq!(&bytes[0..4], &[0x4C, 0, 0, 0], "a Windows shortcut");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn registry_values_round_trip_under_a_throwaway_key() {
        let key = r"Software\JabroDraw-setup-selftest";
        let cmd = "\"C:\\Program Files\\x y\\jabro-draw.exe\" \"%1\"";
        install::reg_set(&format!(r"{key}\shell\open\command"), None, install::Value::Text(cmd)).unwrap();
        install::reg_set(key, Some("N"), install::Value::Number(7)).unwrap();
        assert_eq!(install::reg_get(&format!(r"{key}\shell\open\command"), None).as_deref(), Some(cmd));
        install::reg_delete_tree(key);
        assert_eq!(install::reg_get(&format!(r"{key}\shell\open\command"), None), None, "cleaned up");
    }

    #[test]
    fn a_picked_parent_folder_gets_a_jabro_draw_subfolder() {
        assert_eq!(install_dir_in(Path::new(r"D:\Apps")), PathBuf::from(r"D:\Apps\Jabro Draw"));
        assert_eq!(install_dir_in(Path::new(r"D:\Apps\Jabro Draw")), PathBuf::from(r"D:\Apps\Jabro Draw"));
    }
}
