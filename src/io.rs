//! Native .jabro format (JSON with embedded PNG paint layers) and PNG export.

use crate::doc::Document;
use std::path::Path;
use tiny_skia::Pixmap;

pub fn save(doc: &Document, path: &Path) -> Result<(), String> {
    let json = serde_json::to_vec(doc).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

pub fn load(path: &Path) -> Result<Document, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut doc: Document = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let max_id = doc.objects.iter().map(|o| o.id).max().unwrap_or(0);
    doc.next_id = doc.next_id.max(max_id + 1);
    for o in &mut doc.objects {
        o.touch();
    }
    Ok(doc)
}

pub fn export_png(pm: &Pixmap, path: &Path) -> Result<(), String> {
    pm.save_png(path).map_err(|e| e.to_string())
}
