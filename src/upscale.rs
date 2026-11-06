//! AI upscaling with Real-ESRGAN general x4v3 (Wang et al., BSD-3-Clause), run locally with
//! `tract`. The picture is processed in overlapping tiles on every processor core.

use crate::ai::Plan;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use tiny_skia::Pixmap;
use tract_onnx::prelude::*;

pub const ATTRIBUTION: &str = "AI upscaling uses Real-ESRGAN by X. Wang et al. (BSD-3-Clause). It runs on this computer.";

/// Input tile size and how much neighbouring tiles overlap (input pixels).
const TILE: usize = 128;
const OVERLAP: usize = 8;
/// Largest picture side the result may have.
pub const MAX_SIDE: u32 = 4096;

pub fn load() -> Result<Plan, String> {
    tract_onnx::onnx()
        .model_for_read(&mut std::io::Cursor::new(jabro_models::ESRGAN_X4))
        .and_then(|m| m.with_input_fact(0, f32::fact([1, 3, TILE, TILE]).into()))
        .and_then(|m| m.into_optimized())
        .and_then(|m| m.into_runnable())
        .map_err(|e| format!("could not load the AI upscaling model: {e}"))
}

/// Is a picture of this size small enough to be enlarged `factor` times?
pub fn fits(w: u32, h: u32, factor: u32) -> bool {
    w * factor <= MAX_SIDE && h * factor <= MAX_SIDE
}

/// Enlarge `img` 2x or 4x. `progress(done, total)` is called per tile; `cancel` stops early.
pub fn upscale(
    plan: &Plan,
    img: &Pixmap,
    factor: u32,
    progress: &(dyn Fn(u32, u32) + Sync),
    cancel: &AtomicBool,
) -> Result<Pixmap, String> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if !fits(w as u32, h as u32, factor) {
        return Err(format!("too large: at most {} px per side for {factor}×", MAX_SIDE / factor));
    }
    // straight colour 0..1 per channel (planar), and the alpha on its own
    let mut rgb = vec![0f32; w * h * 3];
    let mut alpha = image::GrayImage::new(w as u32, h as u32);
    for (i, s) in img.data().chunks_exact(4).enumerate() {
        let a = s[3] as f32 / 255.0;
        for c in 0..3 {
            rgb[c * w * h + i] = if a > 0.0 { (s[c] as f32 / 255.0 / a).min(1.0) } else { 0.0 };
        }
        alpha.as_mut()[i] = s[3];
    }
    let step = TILE - 2 * OVERLAP;
    let starts = |n: usize| -> Vec<usize> {
        if n <= TILE {
            return vec![0];
        }
        let mut v: Vec<usize> = (0..).map(|i| i * step).take_while(|&s| s + TILE < n).collect();
        v.push(n - TILE);
        v
    };
    let tiles: Vec<(usize, usize)> = starts(h).into_iter().flat_map(|y| starts(w).into_iter().map(move |x| (x, y))).collect();
    let (ow, oh) = (w * 4, h * 4);
    let out = Mutex::new(vec![0f32; ow * oh * 3]);
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let failed: Mutex<Option<String>> = Mutex::new(None);
    let total = tiles.len() as u32;
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 16).min(tiles.len());
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= tiles.len() || cancel.load(Ordering::Relaxed) || failed.lock().unwrap().is_some() {
                    break;
                }
                let (tx, ty) = tiles[i];
                // pictures smaller than a tile are padded by repeating their edge
                let input = tract_ndarray::Array4::from_shape_fn((1, 3, TILE, TILE), |(_, c, y, x)| {
                    let (sx, sy) = ((tx + x).min(w - 1), (ty + y).min(h - 1));
                    rgb[c * w * h + sy * w + sx]
                });
                let res = plan.run(tvec!(Tensor::from(input).into()));
                let res = match res {
                    Ok(r) => r,
                    Err(e) => {
                        *failed.lock().unwrap() = Some(format!("AI upscaling failed: {e}"));
                        break;
                    }
                };
                let Ok(view) = res[0].to_plain_array_view::<f32>() else { break };
                // keep the tile's middle; its edges are covered by the neighbours (except at the border)
                let keep = |s: usize, n: usize| {
                    let lo = if s == 0 { 0 } else { OVERLAP };
                    let hi = if s + TILE >= n { TILE } else { TILE - OVERLAP };
                    (lo, hi.min(n - s))
                };
                let (x_lo, x_hi) = keep(tx, w);
                let (y_lo, y_hi) = keep(ty, h);
                let mut o = out.lock().unwrap();
                for c in 0..3 {
                    for y in y_lo * 4..y_hi * 4 {
                        let row = (ty * 4 + y) * ow;
                        for x in x_lo * 4..x_hi * 4 {
                            o[c * ow * oh + row + tx * 4 + x] = view[[0, c, y, x]];
                        }
                    }
                }
                drop(o);
                let d = done.fetch_add(1, Ordering::Relaxed) as u32 + 1;
                progress(d, total);
            });
        }
    });
    if let Some(e) = failed.into_inner().unwrap() {
        return Err(e);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    let out = out.into_inner().unwrap();
    let big_alpha = image::imageops::resize(&alpha, ow as u32, oh as u32, image::imageops::FilterType::CatmullRom);
    let mut big = image::RgbaImage::new(ow as u32, oh as u32);
    for (i, px) in big.pixels_mut().enumerate() {
        let v = |c: usize| (out[c * ow * oh + i].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        *px = image::Rgba([v(0), v(1), v(2), big_alpha.as_raw()[i]]);
    }
    if factor == 2 {
        big = image::imageops::resize(&big, (w * 2) as u32, (h * 2) as u32, image::imageops::FilterType::Lanczos3);
    }
    let (bw, bh) = big.dimensions();
    let mut pm = Pixmap::new(bw, bh).ok_or("picture too large")?;
    for (d, s) in pm.data_mut().chunks_exact_mut(4).zip(big.pixels()) {
        let a = s[3] as f32 / 255.0;
        for c in 0..3 {
            d[c] = (s[c] as f32 * a + 0.5) as u8;
        }
        d[3] = s[3];
    }
    Ok(pm)
}

pub struct UpscaleJob {
    pub id: u64,
    pub img: Arc<Pixmap>,
    pub factor: u32,
    pub cancel: Arc<AtomicBool>,
}

pub enum UpscaleEvent {
    Progress(u32, u32),
    Done { id: u64, img: Pixmap },
    Error(String),
}

pub struct UpscaleWorker {
    tx: Sender<UpscaleJob>,
    pub rx: Receiver<UpscaleEvent>,
}

impl UpscaleWorker {
    pub fn new(on_event: impl Fn() + Send + Sync + 'static) -> Self {
        let (jtx, jrx) = channel::<UpscaleJob>();
        let (etx, erx) = channel::<UpscaleEvent>();
        std::thread::spawn(move || {
            let mut plan: Option<Plan> = None;
            while let Ok(job) = jrx.recv() {
                let ev = (|| {
                    if plan.is_none() {
                        plan = Some(load()?);
                    }
                    let p = plan.as_ref().expect("loaded");
                    let tx = Mutex::new(etx.clone());
                    let report = |d: u32, n: u32| {
                        let _ = tx.lock().unwrap().send(UpscaleEvent::Progress(d, n));
                        on_event();
                    };
                    let img = upscale(p, &job.img, job.factor, &report, &job.cancel)?;
                    Ok::<_, String>(UpscaleEvent::Done { id: job.id, img })
                })()
                .unwrap_or_else(UpscaleEvent::Error);
                if etx.send(ev).is_err() {
                    break;
                }
                on_event();
            }
        });
        Self { tx: jtx, rx: erx }
    }

    pub fn submit(&self, job: UpscaleJob) {
        let _ = self.tx.send(job);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upscaling_makes_a_sharper_bigger_picture() {
        // a soft-edged disc on a 150 x 100 picture (so it needs more than one tile)
        let mut img = Pixmap::new(150, 100).unwrap();
        img.fill(tiny_skia::Color::from_rgba8(240, 240, 235, 255));
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(30, 90, 200, 255);
        let disc = tiny_skia::PathBuilder::from_circle(75.0, 50.0, 30.0).unwrap();
        img.fill_path(&disc, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
        let plan = load().expect("embedded model loads");
        let calls = AtomicUsize::new(0);
        let report = |_: u32, _: u32| {
            calls.fetch_add(1, Ordering::Relaxed);
        };
        let big = upscale(&plan, &img, 4, &report, &AtomicBool::new(false)).unwrap();
        assert_eq!((big.width(), big.height()), (600, 400));
        assert_eq!(calls.load(Ordering::Relaxed), 2, "two tiles across, one down");
        let px = |x: u32, y: u32| big.pixel(x, y).unwrap().demultiply();
        assert!(px(300, 200).blue() > 150 && px(300, 200).red() < 80, "disc centre {:?}", px(300, 200));
        assert!(px(20, 20).red() > 200, "background {:?}", px(20, 20));
        // tiles join without a seam: no jump along the column where they meet
        let mut worst = 0;
        for y in 0..400 {
            let a = px(16 * 4 + 0, y).red() as i32; // inside the first tile's margin region
            let b = px(16 * 4 + 1, y).red() as i32;
            worst = worst.max((a - b).abs());
        }
        assert!(worst < 60, "seam jump {worst}");
        let half = upscale(&plan, &img, 2, &|_, _| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!((half.width(), half.height()), (300, 200));
        assert!(!fits(1500, 900, 4) && fits(1500, 900, 2));
    }
}
