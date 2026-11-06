//! Animation and time-lapse export: every frame is rendered and written as an animated GIF
//! or as numbered PNG files (for any video editor). Runs on a worker thread.

use crate::doc::Document;
use crate::export::{self, Options};
use crate::render::Renderer;
use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame, RgbaImage};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use tiny_skia::Pixmap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimFormat {
    Gif,
    PngSequence,
}

pub enum Msg {
    Progress(u32, u32),
    Done(String),
    Failed(String),
}

fn to_image(pm: &Pixmap) -> RgbaImage {
    RgbaImage::from_raw(pm.width(), pm.height(), export::straight_rgba(pm)).expect("frame size")
}

/// Path of frame `i` in a PNG sequence: `name_0001.png` next to the chosen file.
pub fn frame_path(base: &Path, i: u32) -> PathBuf {
    let stem = base.file_stem().and_then(|s| s.to_str()).unwrap_or("frame");
    base.with_file_name(format!("{stem}_{:04}.png", i + 1))
}

struct Sink {
    gif: Option<GifEncoder<std::io::BufWriter<std::fs::File>>>,
    base: PathBuf,
    delay: Delay,
    count: u32,
}

impl Sink {
    fn new(path: &Path, format: AnimFormat, fps: u32) -> Result<Sink, String> {
        let delay = Delay::from_numer_denom_ms(1000, fps.clamp(1, 60));
        let gif = match format {
            AnimFormat::Gif => {
                let f = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
                let mut enc = GifEncoder::new_with_speed(std::io::BufWriter::new(f), 10);
                enc.set_repeat(Repeat::Infinite).map_err(|e| e.to_string())?;
                Some(enc)
            }
            AnimFormat::PngSequence => None,
        };
        Ok(Sink { gif, base: path.to_path_buf(), delay, count: 0 })
    }

    fn push(&mut self, img: RgbaImage, delay: Option<Delay>) -> Result<(), String> {
        match &mut self.gif {
            Some(enc) => enc
                .encode_frame(Frame::from_parts(img, 0, 0, delay.unwrap_or(self.delay)))
                .map_err(|e| e.to_string())?,
            None => img.save(frame_path(&self.base, self.count)).map_err(|e| e.to_string())?,
        }
        self.count += 1;
        Ok(())
    }
}

/// Render every frame of the document's animation.
pub fn export_animation(doc: &Document, opts: &Options, format: AnimFormat, path: &Path, tx: &Sender<Msg>) -> Result<String, String> {
    let n = doc.anim.frames.max(1);
    let mut d = export::prepare(doc, None, opts.scale)?;
    let mut sink = Sink::new(path, format, doc.anim.fps)?;
    let mut r = Renderer::default();
    for f in 0..n {
        d.apply_frame(f);
        let pm = export::render_prepared(&mut r, &d, opts);
        sink.push(to_image(&pm), None)?;
        let _ = tx.send(Msg::Progress(f + 1, n));
    }
    Ok(match format {
        AnimFormat::Gif => format!("Saved {n} frames to {}", path.display()),
        AnimFormat::PngSequence => format!("Saved {n} PNG frames next to {}", frame_path(path, 0).display()),
    })
}

/// Replay of the painting process from the recorded snapshots (PNG bytes, oldest first).
/// Plays in about `seconds`, then holds the finished picture for a moment.
pub fn export_timelapse(frames: &[std::sync::Arc<Vec<u8>>], seconds: f32, format: AnimFormat, path: &Path, tx: &Sender<Msg>) -> Result<String, String> {
    if frames.is_empty() {
        return Err("nothing recorded yet".into());
    }
    // at most 15 frames a second; skip evenly if there are more snapshots than that
    let fps = 15u32;
    let wanted = ((seconds.max(1.0) * fps as f32) as usize).clamp(2, 900);
    let pick: Vec<usize> = if frames.len() <= wanted {
        (0..frames.len()).collect()
    } else {
        (0..wanted).map(|i| i * (frames.len() - 1) / (wanted - 1)).collect()
    };
    let per = (seconds.max(1.0) * 1000.0 / pick.len() as f32).clamp(1000.0 / fps as f32, 1000.0) as u32;
    let mut sink = Sink::new(path, format, fps)?;
    let first = image::load_from_memory(&frames[0]).map_err(|e| e.to_string())?.to_rgba8();
    let (w, h) = first.dimensions();
    let total = pick.len() as u32;
    for (k, &i) in pick.iter().enumerate() {
        let mut img = image::load_from_memory(&frames[i]).map_err(|e| e.to_string())?.to_rgba8();
        if img.dimensions() != (w, h) {
            // the canvas was resized while painting: fit every frame into the first one's size
            img = image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle);
        }
        let last = k + 1 == pick.len();
        let delay = Delay::from_numer_denom_ms(if last { 2500 } else { per }, 1);
        sink.push(img, Some(delay))?;
        let _ = tx.send(Msg::Progress(k as u32 + 1, total));
    }
    Ok(format!("Saved a {}-frame time-lapse to {}", sink.count, path.display()))
}

/// Run `job` on a worker thread; progress and the result arrive on the receiver.
pub fn spawn(job: impl FnOnce(&Sender<Msg>) -> Result<String, String> + Send + 'static) -> Receiver<Msg> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let msg = match job(&tx) {
            Ok(s) => Msg::Done(s),
            Err(e) => Msg::Failed(e),
        };
        let _ = tx.send(msg);
    });
    rx
}

/// A small snapshot of the canvas for the time-lapse (longest side `max_side`), as PNG bytes.
pub fn snapshot(pm: &Pixmap, max_side: u32, background: [u8; 4]) -> Option<Vec<u8>> {
    let k = (max_side as f32 / pm.width().max(pm.height()) as f32).min(1.0);
    let (w, h) = (((pm.width() as f32 * k).round() as u32).max(1), ((pm.height() as f32 * k).round() as u32).max(1));
    let mut small = Pixmap::new(w, h)?;
    small.fill(tiny_skia::Color::from_rgba8(background[0], background[1], background[2], 255));
    let paint = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..Default::default() };
    small.draw_pixmap(0, 0, pm.as_ref(), &paint, tiny_skia::Transform::from_scale(k, k), None);
    let img = to_image(&small);
    let mut out = Vec::new();
    let enc = image::codecs::png::PngEncoder::new_with_quality(
        &mut out,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Sub,
    );
    image::ImageEncoder::write_image(enc, img.as_raw(), w, h, image::ExtendedColorType::Rgba8).ok()?;
    Some(out)
}
