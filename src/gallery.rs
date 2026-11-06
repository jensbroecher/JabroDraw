//! The object gallery: objects kept for reuse in other drawings.
//!
//! Each item is a small, self-contained `.jabro` document: the objects, cropped to their size,
//! with a name, a category and a stable id in its `gallery` field. So an item can be opened like
//! any drawing, e-mailed, dropped into someone else's gallery, or later shared through a server,
//! always as a single file. The folder also holds a cache (`index.json` and `thumbs/`) that is
//! rebuilt from the item files whenever it is missing or out of date.

use crate::canvas;
use crate::doc::{Animation, Document, GalleryMeta, Object};
use crate::export::scale_object;
use crate::geom::Pt;
use crate::io;
use crate::render::Renderer;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tiny_skia::Pixmap;

/// Longest side of a thumbnail in pixels.
pub const THUMB: u32 = 160;
const INDEX: &str = "index.json";
const THUMBS: &str = "thumbs";
/// Empty space around the objects of an item, in pixels.
const PAD: f32 = 2.0;

/// One gallery item as listed in the panel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    /// File name inside the gallery folder.
    pub file: String,
    pub id: String,
    pub name: String,
    pub category: String,
    pub created: u64,
    /// Size of the saved objects in pixels.
    pub size: [u32; 2],
    pub objects: usize,
    /// Modification time (ms) and length of the file, to notice when it changes.
    modified: u64,
    len: u64,
}

impl Item {
    /// Changes whenever the item file does (for caching its thumbnail texture).
    pub fn stamp(&self) -> u64 {
        self.modified ^ self.len.rotate_left(32)
    }
}

pub struct Gallery {
    pub dir: PathBuf,
    /// Newest first.
    pub items: Vec<Item>,
}

impl Gallery {
    /// Open (and create) the gallery in `dir`, reading the cache where it is still valid.
    pub fn open(dir: &Path) -> Result<Gallery, String> {
        std::fs::create_dir_all(dir.join(THUMBS)).map_err(|e| format!("Can't create the gallery folder: {e}"))?;
        let cached: Vec<Item> = std::fs::read(dir.join(INDEX))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut g = Gallery { dir: dir.to_path_buf(), items: Vec::new() };
        let mut changed = false;
        let mut files: Vec<String> = std::fs::read_dir(dir)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|f| f.to_ascii_lowercase().ends_with(".jabro"))
            .collect();
        files.sort();
        for file in files {
            let (modified, len) = stamp(&dir.join(&file));
            let hit = cached
                .iter()
                .find(|c| c.file == file && c.modified == modified && c.len == len)
                .filter(|_| g.thumb_path(&file).exists());
            match hit {
                Some(c) => g.items.push(c.clone()),
                None => {
                    // a new or changed file; unreadable files are left alone
                    if let Ok(item) = g.index_file(&file) {
                        g.items.push(item);
                        changed = true;
                    }
                }
            }
        }
        changed |= g.items.len() != cached.len();
        g.sort();
        if changed {
            g.save_index();
        }
        Ok(g)
    }

    pub fn thumb_path(&self, file: &str) -> PathBuf {
        let stem = file.strip_suffix(".jabro").unwrap_or(file);
        self.dir.join(THUMBS).join(format!("{stem}.png"))
    }

    pub fn find(&self, file: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.file == file)
    }

    /// All categories in use, sorted.
    pub fn categories(&self) -> Vec<String> {
        let mut c: Vec<String> = self.items.iter().map(|i| i.category.clone()).filter(|c| !c.is_empty()).collect();
        c.sort_by_key(|c| c.to_lowercase());
        c.dedup();
        c
    }

    /// Items whose name or category contains `search`, in `category` (None = all).
    pub fn filtered(&self, search: &str, category: Option<&str>) -> Vec<&Item> {
        let s = search.trim().to_lowercase();
        self.items
            .iter()
            .filter(|i| category.is_none_or(|c| i.category == c))
            .filter(|i| s.is_empty() || i.name.to_lowercase().contains(&s) || i.category.to_lowercase().contains(&s))
            .collect()
    }

    /// Keep `objects` (from a drawing whose animation settings are `anim`) as a new item.
    pub fn add(&mut self, objects: &[Object], anim: &Animation, name: &str, category: &str) -> Result<String, String> {
        let meta = GalleryMeta {
            id: new_id(),
            name: clean(name, "Untitled"),
            category: clean(category, ""),
            created: now(),
            app: env!("CARGO_PKG_VERSION").into(),
        };
        let doc = item_document(objects, anim, meta)?;
        self.store(doc)
    }

    /// Copy a .jabro file into the gallery (an item someone sent, or any drawing: its objects
    /// become the item). An item that is already here is not added twice.
    pub fn import(&mut self, path: &Path) -> Result<String, String> {
        let doc = io::load(path)?;
        if doc.objects.is_empty() {
            return Err(format!("{} has no objects", path.display()));
        }
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let mut meta = doc.gallery.clone().unwrap_or_default();
        if meta.id.is_empty() {
            meta.id = new_id();
        }
        if let Some(have) = self.items.iter().find(|i| i.id == meta.id) {
            return Err(format!("\u{201c}{}\u{201d} is already in the gallery", have.name));
        }
        meta.name = clean(&meta.name, &clean(&stem, "Imported"));
        if meta.category.trim().is_empty() {
            meta.category = "Imported".into();
        }
        if meta.created == 0 {
            meta.created = now();
        }
        let item = item_document(&doc.objects, &doc.anim, meta)?;
        self.store(item)
    }

    /// The item's drawing.
    pub fn load(&self, file: &str) -> Result<Document, String> {
        io::load(&self.dir.join(file))
    }

    /// Rename an item or file it under another category.
    pub fn update(&mut self, file: &str, name: &str, category: &str) -> Result<(), String> {
        let mut doc = self.load(file)?;
        let meta = doc.gallery.get_or_insert_with(GalleryMeta::default);
        meta.name = clean(name, &meta.name.clone());
        meta.category = clean(category, "");
        let (new_name, new_category) = (meta.name.clone(), meta.category.clone());
        io::save(&doc, &self.dir.join(file))?;
        let (modified, len) = stamp(&self.dir.join(file));
        if let Some(i) = self.items.iter_mut().find(|i| i.file == file) {
            (i.name, i.category) = (new_name, new_category);
            (i.modified, i.len) = (modified, len);
        }
        self.save_index();
        Ok(())
    }

    /// Delete an item's file.
    pub fn remove(&mut self, file: &str) -> Result<(), String> {
        std::fs::remove_file(self.dir.join(file)).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(self.thumb_path(file));
        self.items.retain(|i| i.file != file);
        self.save_index();
        Ok(())
    }

    /// Save a copy of an item somewhere else (to send it to someone).
    pub fn export(&self, file: &str, to: &Path) -> Result<(), String> {
        std::fs::copy(self.dir.join(file), to).map(|_| ()).map_err(|e| e.to_string())
    }

    fn store(&mut self, doc: Document) -> Result<String, String> {
        let meta = doc.gallery.clone().unwrap_or_default();
        let short: String = meta.id.chars().take(8).collect();
        let base = format!("{}-{short}", slug(&meta.name));
        let mut file = format!("{base}.jabro");
        let mut n = 2;
        while self.dir.join(&file).exists() {
            file = format!("{base}-{n}.jabro");
            n += 1;
        }
        io::save(&doc, &self.dir.join(&file))?;
        let item = self.describe(&file, &doc)?;
        self.items.push(item);
        self.sort();
        self.save_index();
        Ok(file)
    }

    /// Read an item file and (re)make its thumbnail. Files without gallery details (copied
    /// into the folder by hand) get them written in.
    fn index_file(&self, file: &str) -> Result<Item, String> {
        let path = self.dir.join(file);
        let mut doc = io::load(&path)?;
        if doc.gallery.as_ref().is_none_or(|m| m.id.is_empty()) {
            let stem = file.strip_suffix(".jabro").unwrap_or(file);
            let mut meta = doc.gallery.clone().unwrap_or_default();
            meta.id = new_id();
            meta.name = clean(&meta.name, stem);
            meta.created = (stamp(&path).0 / 1000).max(1);
            meta.app = env!("CARGO_PKG_VERSION").into();
            doc.gallery = Some(meta);
            io::save(&doc, &path)?;
        }
        self.describe(file, &doc)
    }

    fn describe(&self, file: &str, doc: &Document) -> Result<Item, String> {
        let meta = doc.gallery.clone().unwrap_or_default();
        let thumb = thumbnail(doc).ok_or("could not draw a thumbnail")?;
        thumb.save_png(self.thumb_path(file)).map_err(|e| e.to_string())?;
        let (modified, len) = stamp(&self.dir.join(file));
        Ok(Item {
            file: file.to_string(),
            id: meta.id,
            name: meta.name,
            category: meta.category,
            created: meta.created,
            size: [doc.width, doc.height],
            objects: doc.objects.len(),
            modified,
            len,
        })
    }

    fn sort(&mut self) {
        self.items.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| a.name.cmp(&b.name)));
    }

    fn save_index(&self) {
        if let Ok(json) = serde_json::to_vec_pretty(&self.items) {
            let _ = std::fs::write(self.dir.join(INDEX), json);
        }
    }
}

/// A drawing that holds just `objects`, moved so they start at the top left, with `meta`.
pub fn item_document(objects: &[Object], anim: &Animation, meta: GalleryMeta) -> Result<Document, String> {
    let mut d = Document::new(1, 1);
    d.objects = objects.to_vec();
    let (lo, hi) = canvas::content_bounds(&d).ok_or("Nothing visible to keep in the gallery")?;
    d.width = ((hi.x - lo.x + 2.0 * PAD).ceil() as u32).clamp(canvas::MIN_SIZE, canvas::MAX_SIZE);
    d.height = ((hi.y - lo.y + 2.0 * PAD).ceil() as u32).clamp(canvas::MIN_SIZE, canvas::MAX_SIZE);
    for o in &mut d.objects {
        o.translate(Pt::new(PAD - lo.x, PAD - lo.y));
    }
    d.next_id = d.objects.iter().map(|o| o.id).max().unwrap_or(0) + 1;
    d.anim = Animation { current: 0, ..anim.clone() };
    d.gallery = Some(meta);
    Ok(d)
}

/// Put the objects of gallery item `item` into `doc`, centred on `at`, as new objects (new ids,
/// their own groups and linked clones). Items larger than the page are scaled down to fit.
/// Returns the new ids, bottom-most first.
pub fn place(doc: &mut Document, item: &Document, at: Pt) -> Vec<u64> {
    let (w, h) = (item.width.max(1) as f32, item.height.max(1) as f32);
    let k = (0.9 * doc.width as f32 / w).min(0.9 * doc.height as f32 / h).min(1.0);
    let centre = Pt::new(w * 0.5, h * 0.5) * k;
    let mut groups = HashMap::new();
    let mut clones = HashMap::new();
    let mut ids = Vec::new();
    let first = doc.objects.len();
    for src in &item.objects {
        let mut o = src.clone();
        if k < 1.0 {
            scale_object(&mut o, k);
        }
        o.translate(at - centre);
        o.id = doc.alloc_id();
        o.group = o.group.map(|g| *groups.entry(g).or_insert_with(|| doc.alloc_id()));
        o.instance_of = o.instance_of.map(|r| *clones.entry(r).or_insert_with(|| doc.alloc_id()));
        o.touch();
        ids.push(o.id);
        doc.objects.push(o);
    }
    // a linked clone whose partners stayed behind is a normal object
    let mut count: HashMap<u64, usize> = HashMap::new();
    for o in &doc.objects[first..] {
        if let Some(r) = o.instance_of {
            *count.entry(r).or_default() += 1;
        }
    }
    for o in &mut doc.objects[first..] {
        if o.instance_of.is_some_and(|r| count[&r] < 2) {
            o.instance_of = None;
        }
    }
    ids
}

/// The item drawn small on a transparent background (longest side `THUMB`).
pub fn thumbnail(doc: &Document) -> Option<Pixmap> {
    // draw at twice the thumbnail size, then halve, for smooth edges (small items are drawn
    // bigger, so they stay sharp in the panel)
    let big = THUMB as f32 * 2.0;
    let k = (big / doc.width.max(doc.height).max(1) as f32).min(8.0);
    let mut d = doc.clone();
    d.guides.clear();
    if k != 1.0 {
        d.width = ((d.width as f32 * k).round() as u32).max(1);
        d.height = ((d.height as f32 * k).round() as u32).max(1);
        for o in &mut d.objects {
            scale_object(o, k);
        }
    }
    let pm = Renderer::default().render_opts(&d, false);
    let s = (THUMB as f32 / pm.width().max(pm.height()) as f32).min(1.0);
    let (tw, th) = (((pm.width() as f32 * s).round() as u32).max(1), ((pm.height() as f32 * s).round() as u32).max(1));
    let mut out = Pixmap::new(tw, th)?;
    let paint = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bicubic, ..Default::default() };
    out.draw_pixmap(0, 0, pm.as_ref(), &paint, tiny_skia::Transform::from_scale(s, s), None);
    Some(out)
}

fn stamp(path: &Path) -> (u64, u64) {
    let Ok(m) = std::fs::metadata(path) else { return (0, 0) };
    let modified = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64);
    (modified, m.len())
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// 32 random hex digits.
fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    (0..2u64)
        .map(|i| {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u128(nanos);
            h.write_u32(std::process::id());
            h.write_u64(i);
            format!("{:016x}", h.finish())
        })
        .collect()
}

/// Trimmed text of at most 80 characters, or `fallback` when empty.
fn clean(s: &str, fallback: &str) -> String {
    let t: String = s.split_whitespace().collect::<Vec<_>>().join(" ").chars().filter(|c| !c.is_control()).take(80).collect();
    if t.trim().is_empty() { fallback.to_string() } else { t.trim().to_string() }
}

/// A safe file name part: lowercase letters, digits and dashes.
fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            s.push(c);
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
        if s.len() >= 32 {
            break;
        }
    }
    let s = s.trim_end_matches('-').to_string();
    if s.is_empty() { "item".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{FillKind, Object, ShapeKind};

    fn temp_gallery(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jabro_gallery_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn red_box(id: u64, x: f32, y: f32, w: f32, h: f32) -> Object {
        let mut o = Object::new(id, ShapeKind::Rect { corner: 0.0 }, x, y, w, h);
        o.fill.kind = FillKind::Solid;
        o.fill.a = [220, 30, 30, 255];
        o
    }

    #[test]
    fn items_are_kept_listed_and_reopened() {
        let dir = temp_gallery("keep");
        let mut g = Gallery::open(&dir).unwrap();
        assert!(g.items.is_empty());
        let mut a = red_box(7, 300.0, 200.0, 100.0, 50.0);
        let mut b = red_box(8, 450.0, 260.0, 40.0, 40.0);
        a.group = Some(99);
        b.group = Some(99);
        let file = g.add(&[a, b], &Animation::default(), "  Red  boxes ", "Shapes").unwrap();
        assert!(file.starts_with("red-boxes-") && file.ends_with(".jabro"), "{file}");
        assert!(g.thumb_path(&file).exists());
        let item = g.find(&file).unwrap().clone();
        assert_eq!((item.name.as_str(), item.category.as_str(), item.objects), ("Red boxes", "Shapes", 2));
        assert_eq!(item.size, [194, 104]);

        // the file is a normal drawing with the objects moved to the top left
        let doc = g.load(&file).unwrap();
        assert_eq!(doc.gallery.as_ref().unwrap().id, item.id);
        assert_eq!(doc.objects[0].quad[0], Pt::new(PAD, PAD));

        // reopening reads the cache, and the thumbnail shows the red box
        let g2 = Gallery::open(&dir).unwrap();
        assert_eq!(g2.items, g.items);
        let thumb = Pixmap::load_png(g.thumb_path(&file)).unwrap();
        assert_eq!(thumb.width(), THUMB);
        let px = thumb.pixel(20, 20).unwrap().demultiply();
        assert!(px.red() > 200 && px.green() < 60 && px.alpha() == 255, "{px:?}");
        assert_eq!(thumb.pixel(THUMB - 2, 2).unwrap().alpha(), 0, "transparent where nothing is");

        assert_eq!(g.filtered("RED", None).len(), 1);
        assert_eq!(g.filtered("", Some("Other")).len(), 0);
        assert_eq!(g.categories(), vec!["Shapes".to_string()]);

        g.update(&file, "Two boxes", "Basics").unwrap();
        assert_eq!(Gallery::open(&dir).unwrap().items[0].name, "Two boxes");
        assert_eq!(g.load(&file).unwrap().gallery.unwrap().category, "Basics");

        g.remove(&file).unwrap();
        assert!(!dir.join(&file).exists() && !g.thumb_path(&file).exists());
        assert!(Gallery::open(&dir).unwrap().items.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn placing_an_item_makes_new_objects_in_the_middle() {
        let mut a = red_box(1, 0.0, 0.0, 100.0, 100.0);
        let mut b = red_box(2, 100.0, 0.0, 100.0, 100.0);
        let mut c = red_box(3, 0.0, 100.0, 50.0, 50.0);
        (a.group, b.group) = (Some(5), Some(5));
        (b.instance_of, c.instance_of) = (Some(2), Some(2));
        a.instance_of = Some(40); // its partner was not saved
        let item = item_document(&[a, b, c], &Animation::default(), GalleryMeta::default()).unwrap();

        let mut doc = Document::new(800, 600);
        let first = doc.alloc_id();
        doc.objects.push(red_box(first, 10.0, 10.0, 10.0, 10.0));
        let ids = place(&mut doc, &item, Pt::new(400.0, 300.0));
        assert_eq!(ids.len(), 3);
        assert_eq!(doc.objects.len(), 4);
        let mut all: Vec<u64> = doc.objects.iter().map(|o| o.id).collect();
        all.dedup();
        assert_eq!(all.len(), 4, "ids are unique");
        let new = &doc.objects[1..];
        assert!(new[0].group.is_some() && new[0].group == new[1].group && new[2].group.is_none());
        assert!(new[1].instance_of.is_some() && new[1].instance_of == new[2].instance_of);
        assert_eq!(new[0].instance_of, None);
        // centred on the point
        let mut d2 = Document::new(1, 1);
        d2.objects = new.to_vec();
        let (lo, hi) = canvas::content_bounds(&d2).unwrap();
        let mid = (lo + hi) * 0.5;
        assert!((mid.x - 400.0).abs() < 1.5 && (mid.y - 300.0).abs() < 1.5, "{mid:?}");

        // too big for a small page: scaled down to fit
        let mut small = Document::new(100, 100);
        place(&mut small, &item, Pt::new(50.0, 50.0));
        let (lo, hi) = canvas::content_bounds(&small).unwrap();
        assert!(hi.x - lo.x <= 91.0 && lo.x >= 4.0, "{lo:?} {hi:?}");
    }

    #[test]
    fn imports_files_once_and_names_them() {
        let dir = temp_gallery("import");
        let mut g = Gallery::open(&dir).unwrap();
        // a plain drawing someone sent
        let mut drawing = Document::new(500, 400);
        drawing.objects.push(red_box(1, 50.0, 60.0, 80.0, 40.0));
        let sent = std::env::temp_dir().join(format!("Blue sky {}.jabro", std::process::id()));
        io::save(&drawing, &sent).unwrap();
        let file = g.import(&sent).unwrap();
        let item = g.find(&file).unwrap().clone();
        assert!(item.name.starts_with("Blue sky"), "{}", item.name);
        assert_eq!(item.category, "Imported");
        assert_eq!(item.size, [84, 44]);
        // a gallery item comes in under its own name, and only once
        let shared = dir.join(&file);
        let copy = std::env::temp_dir().join(format!("shared_{}.jabro", std::process::id()));
        std::fs::copy(&shared, &copy).unwrap();
        assert!(g.import(&copy).unwrap_err().contains("already in the gallery"));
        let mut other = Gallery::open(&temp_gallery("import2")).unwrap();
        let f2 = other.import(&copy).unwrap();
        assert_eq!(other.find(&f2).unwrap().id, item.id);
        assert_eq!(other.find(&f2).unwrap().name, item.name);

        // a file copied into the folder by hand is picked up
        std::fs::copy(&sent, dir.join("hand made.jabro")).unwrap();
        let g3 = Gallery::open(&dir).unwrap();
        assert_eq!(g3.items.len(), 2);
        assert!(g3.items.iter().any(|i| i.name == "hand made" && !i.id.is_empty()));

        for p in [&sent, &copy] {
            let _ = std::fs::remove_file(p);
        }
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&other.dir);
    }

    #[test]
    fn slugs_and_ids() {
        assert_eq!(slug("Über Stern!! 2"), "ber-stern-2");
        assert_eq!(slug("***"), "item");
        let (a, b) = (new_id(), new_id());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert_eq!(clean("   ", "x"), "x");
    }
}
