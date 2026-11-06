//! Click-to-select with MobileSAM (Zhang et al., Apache-2.0) and the Segment Anything mask
//! decoder (Meta AI, Apache-2.0), run locally with `tract`. The picture is analysed once (about
//! three seconds); after that every click turns into an object mask in a fraction of a second.

use crate::ai::{refine, Plan};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use tiny_skia::Pixmap;
use tract_onnx::prelude::*;

pub const ATTRIBUTION: &str = "AI object selection uses MobileSAM by C. Zhang et al. and the Segment Anything \
mask decoder by Meta AI (both Apache-2.0; ONNX export by Acly, MIT). It runs on this computer.";

/// The encoder works on a 1024 x 1024 square: the picture is scaled to fit and padded.
const SIZE: usize = 1024;
/// The decoder always gets this many points; unused ones are marked as padding.
const POINTS: usize = 8;
const MEAN: [f32; 3] = [123.675, 116.28, 103.53];

pub struct Sam {
    enc: Plan,
    dec: Plan,
}

/// What the encoder made of one picture.
pub struct Embedding {
    data: TValue,
    w: u32,
    h: u32,
    /// Picture pixels -> encoder pixels.
    k: f32,
}

/// A click: picture pixel position and whether it marks the object (true) or not (false).
pub type Point = (f32, f32, bool);

impl Sam {
    pub fn load() -> Result<Sam, String> {
        let err = |e: TractError| format!("could not load the AI selection model: {e}");
        let enc = tract_onnx::onnx()
            .model_for_read(&mut std::io::Cursor::new(jabro_models::SAM_ENCODER))
            .and_then(|m| m.with_input_fact(0, f32::fact([SIZE, SIZE, 3]).into()))
            .and_then(|m| m.into_optimized())
            .and_then(|m| m.into_runnable())
            .map_err(err)?;
        let dec = tract_onnx::onnx()
            .model_for_read(&mut std::io::Cursor::new(jabro_models::SAM_DECODER))
            .and_then(|m| {
                // only the 256 x 256 mask is needed; we scale it to the picture ourselves
                let low_res = m.output_outlets()?[2];
                m.with_input_fact(0, f32::fact([1, 256, 64, 64]).into())?
                    .with_input_fact(1, f32::fact([1, POINTS, 2]).into())?
                    .with_input_fact(2, f32::fact([1, POINTS]).into())?
                    .with_input_fact(3, f32::fact([1, 1, 256, 256]).into())?
                    .with_input_fact(4, f32::fact([1]).into())?
                    .with_input_fact(5, f32::fact([2]).into())?
                    .with_output_outlets(&[low_res])
            })
            .and_then(|m| m.into_optimized())
            .and_then(|m| m.into_runnable())
            .map_err(err)?;
        Ok(Sam { enc, dec })
    }

    pub fn embed(&self, img: &Pixmap) -> Result<Embedding, String> {
        let (w, h) = (img.width(), img.height());
        let k = SIZE as f32 / w.max(h) as f32;
        let (sw, sh) = (((w as f32 * k).round() as u32).clamp(1, SIZE as u32), ((h as f32 * k).round() as u32).clamp(1, SIZE as u32));
        let mut rgb = image::RgbImage::new(w, h);
        for (d, s) in rgb.pixels_mut().zip(img.data().chunks_exact(4)) {
            let a = s[3] as f32 / 255.0;
            let un = |v: u8| if a > 0.0 { (v as f32 / a).min(255.0) as u8 } else { 0 };
            *d = image::Rgb([un(s[0]), un(s[1]), un(s[2])]);
        }
        let small = image::imageops::resize(&rgb, sw, sh, image::imageops::FilterType::Triangle);
        // padding with the mean colour equals the model's own zero padding after normalising
        let input: Tensor = tract_ndarray::Array3::from_shape_fn((SIZE, SIZE, 3), |(y, x, c)| {
            if (x as u32) < sw && (y as u32) < sh {
                small.get_pixel(x as u32, y as u32)[c] as f32
            } else {
                MEAN[c]
            }
        })
        .into();
        let out = self.enc.run(tvec!(input.into())).map_err(|e| format!("AI selection failed: {e}"))?;
        Ok(Embedding { data: out[0].clone(), w, h, k })
    }

    /// Probability (0..1) that each picture pixel belongs to the clicked object.
    pub fn mask(&self, e: &Embedding, img: &Pixmap, points: &[Point]) -> Result<Vec<f32>, String> {
        let mut coords = vec![0f32; POINTS * 2];
        let mut labels = vec![-1f32; POINTS];
        for (i, &(x, y, on)) in points.iter().take(POINTS - 1).enumerate() {
            coords[i * 2] = x * e.k;
            coords[i * 2 + 1] = y * e.k;
            labels[i] = if on { 1.0 } else { 0.0 };
        }
        let t = |v: Vec<f32>, shape: &[usize]| -> Result<TValue, String> {
            Tensor::from_shape(shape, &v).map(|t| t.into()).map_err(|e| e.to_string())
        };
        let inputs = tvec!(
            e.data.clone(),
            t(coords, &[1, POINTS, 2])?,
            t(labels, &[1, POINTS])?,
            t(vec![0.0; 256 * 256], &[1, 1, 256, 256])?,
            t(vec![0.0], &[1])?,
            t(vec![e.h as f32, e.w as f32], &[2])?,
        );
        let out = self.dec.run(inputs).map_err(|e| format!("AI selection failed: {e}"))?;
        let low = out[0].to_plain_array_view::<f32>().map_err(|e| e.to_string())?;
        let low: Vec<f32> = low.iter().copied().collect();
        if low.len() != 256 * 256 {
            return Err(format!("unexpected mask size ({} values)", low.len()));
        }
        // the 256 x 256 logits cover the padded 1024 square at a quarter of its size
        let (w, h) = (e.w as usize, e.h as usize);
        let s = e.k / 4.0;
        let mut p = vec![0f32; w * h];
        for y in 0..h {
            let fy = ((y as f32 + 0.5) * s - 0.5).clamp(0.0, 255.0);
            let (y0, ty) = (fy.floor() as usize, fy.fract());
            let y1 = (y0 + 1).min(255);
            for x in 0..w {
                let fx = ((x as f32 + 0.5) * s - 0.5).clamp(0.0, 255.0);
                let (x0, tx) = (fx.floor() as usize, fx.fract());
                let x1 = (x0 + 1).min(255);
                let g = |yy: usize, xx: usize| low[yy * 256 + xx];
                let top = g(y0, x0) * (1.0 - tx) + g(y0, x1) * tx;
                let bottom = g(y1, x0) * (1.0 - tx) + g(y1, x1) * tx;
                let logit = top * (1.0 - ty) + bottom * ty;
                p[y * w + x] = 1.0 / (1.0 + (-logit * 1.5).exp());
            }
        }
        Ok(refine(&p, img))
    }
}

pub struct SelectJob {
    pub id: u64,
    pub img: Arc<Pixmap>,
    pub points: Vec<Point>,
}

pub enum SelectEvent {
    /// The picture is being analysed (first click on it).
    Analysing,
    Mask { id: u64, points: usize, alpha: Vec<f32>, w: u32, h: u32 },
    Error(String),
}

/// Background thread; keeps the last picture's analysis so further clicks are quick.
pub struct SelectWorker {
    tx: Sender<SelectJob>,
    pub rx: Receiver<SelectEvent>,
}

impl SelectWorker {
    pub fn new(on_event: impl Fn() + Send + 'static) -> Self {
        let (jtx, jrx) = channel::<SelectJob>();
        let (etx, erx) = channel::<SelectEvent>();
        std::thread::spawn(move || {
            let mut sam: Option<Sam> = None;
            let mut cache: Option<(Arc<Pixmap>, Embedding)> = None;
            while let Ok(mut job) = jrx.recv() {
                while let Ok(newer) = jrx.try_recv() {
                    job = newer;
                }
                let ev = (|| {
                    if sam.is_none() {
                        let _ = etx.send(SelectEvent::Analysing);
                        on_event();
                        sam = Some(Sam::load()?);
                    }
                    let s = sam.as_ref().expect("loaded");
                    if !cache.as_ref().is_some_and(|(img, _)| Arc::ptr_eq(img, &job.img)) {
                        let _ = etx.send(SelectEvent::Analysing);
                        on_event();
                        cache = Some((job.img.clone(), s.embed(&job.img)?));
                    }
                    let (img, emb) = cache.as_ref().expect("embedded");
                    let alpha = s.mask(emb, img, &job.points)?;
                    Ok::<_, String>(SelectEvent::Mask {
                        id: job.id,
                        points: job.points.len(),
                        alpha,
                        w: img.width(),
                        h: img.height(),
                    })
                })()
                .unwrap_or_else(SelectEvent::Error);
                if etx.send(ev).is_err() {
                    break;
                }
                on_event();
            }
        });
        Self { tx: jtx, rx: erx }
    }

    pub fn submit(&self, job: SelectJob) {
        let _ = self.tx.send(job);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_selects_the_object_under_it() {
        // two separate discs on a textured backdrop: clicking one selects only that one
        let mut img = Pixmap::new(320, 200).unwrap();
        for (i, px) in img.data_mut().chunks_exact_mut(4).enumerate() {
            let n = (crate::textures::rand01((i % 320) as i32, (i / 320) as i32, 3) * 30.0) as u8;
            px.copy_from_slice(&[200 + n / 2, 205 + n / 2, 190 + n / 2, 255]);
        }
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(200, 40, 40, 255);
        let a = tiny_skia::PathBuilder::from_circle(90.0, 100.0, 50.0).unwrap();
        img.fill_path(&a, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
        paint.set_color_rgba8(30, 60, 190, 255);
        let b = tiny_skia::PathBuilder::from_circle(240.0, 100.0, 45.0).unwrap();
        img.fill_path(&b, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
        let sam = Sam::load().expect("embedded models load");
        let e = sam.embed(&img).unwrap();
        let m = sam.mask(&e, &img, &[(90.0, 100.0, true)]).unwrap();
        let at = |x: usize, y: usize| m[y * 320 + x];
        assert!(at(90, 100) > 0.8 && at(70, 90) > 0.8, "clicked disc selected ({})", at(90, 100));
        assert!(at(240, 100) < 0.2, "the other disc is not ({})", at(240, 100));
        assert!(at(10, 10) < 0.2 && at(310, 190) < 0.2, "nor the background");
    }
}
