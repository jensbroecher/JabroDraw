//! Font loading (system fonts with a built-in fallback).

use crate::doc::FontChoice;
use ab_glyph::FontVec;
use std::path::PathBuf;
use std::sync::OnceLock;

static FONTS: OnceLock<Vec<Option<FontVec>>> = OnceLock::new();

fn candidates(c: FontChoice) -> &'static [&'static str] {
    match c {
        FontChoice::Sans => &["arial.ttf", "segoeui.ttf", "DejaVuSans.ttf", "Arial.ttf"],
        FontChoice::Bold => &["arialbd.ttf", "segoeuib.ttf", "DejaVuSans-Bold.ttf", "Arial Bold.ttf"],
        FontChoice::Black => &["ariblk.ttf", "seguibl.ttf", "arialbd.ttf", "Arial Black.ttf", "DejaVuSans-Bold.ttf"],
        FontChoice::Serif => &["georgiab.ttf", "timesbd.ttf", "times.ttf", "DejaVuSerif-Bold.ttf", "Georgia Bold.ttf"],
        FontChoice::Impact => &["impact.ttf", "Impact.ttf", "arialbd.ttf", "DejaVuSans-Bold.ttf"],
        FontChoice::Script => &["segoesc.ttf", "segoescb.ttf", "comic.ttf", "Brush Script.ttf", "DejaVuSerif-Italic.ttf"],
    }
}

fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(windir) = std::env::var("WINDIR") {
        dirs.push(PathBuf::from(windir).join("Fonts"));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("Microsoft").join("Windows").join("Fonts"));
    }
    for d in [
        "/usr/share/fonts/truetype/dejavu",
        "/usr/share/fonts/TTF",
        "/Library/Fonts",
        "/System/Library/Fonts/Supplemental",
    ] {
        dirs.push(PathBuf::from(d));
    }
    dirs
}

fn fallback() -> Option<FontVec> {
    let defs = eframe::egui::FontDefinitions::default();
    let fd = defs.font_data.get("Ubuntu-Light")?;
    FontVec::try_from_vec(fd.font.to_vec()).ok()
}

fn load(c: FontChoice) -> Option<FontVec> {
    let dirs = font_dirs();
    for name in candidates(c) {
        for dir in &dirs {
            if let Ok(bytes) = std::fs::read(dir.join(name)) {
                if let Ok(f) = FontVec::try_from_vec(bytes) {
                    return Some(f);
                }
            }
        }
    }
    fallback()
}

pub fn get(c: FontChoice) -> Option<&'static FontVec> {
    FONTS
        .get_or_init(|| FontChoice::ALL.iter().map(|&c| load(c)).collect())
        .get(c as usize)?
        .as_ref()
}
