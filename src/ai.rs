//! AI background removal with U²-Net (Qin et al., 2020, Apache-2.0), run locally with the
//! pure-Rust `tract` ONNX engine. The weights are embedded in the program (crate
//! `jabro-models`), so it works offline and pictures never leave the computer.

use crate::raster::gaussian;
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use tiny_skia::Pixmap;
use tract_onnx::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AiModel {
    Fast,
    Best,
}

impl AiModel {
    pub const ALL: [AiModel; 2] = [AiModel::Best, AiModel::Fast];
    pub fn label(self) -> &'static str {
        match self {
            AiModel::Fast => "Fast",
            AiModel::Best => "Best quality",
        }
    }
    fn weights(self) -> &'static [u8] {
        match self {
            AiModel::Fast => jabro_models::U2NETP,
            AiModel::Best => jabro_models::U2NET,
        }
    }
}

pub const ATTRIBUTION: &str = "AI background removal uses U²-Net by X. Qin et al. (Apache-2.0; \
ONNX conversion by the rembg project, MIT). It runs on this computer; pictures are never uploaded.";

pub(crate) type Plan = Arc<TypedRunnableModel>;

fn load_plan(m: AiModel) -> TractResult<Plan> {
    tract_onnx::onnx()
        .model_for_read(&mut std::io::Cursor::new(m.weights()))?
        .with_input_fact(0, f32::fact([1, 3, 320, 320]).into())?
        .into_optimized()?
        .into_runnable()
}

/// Foreground probability (0..1) for every pixel of the picture.
pub fn segment(plan: &Plan, img: &Pixmap) -> Result<Vec<f32>, String> {
    const S: u32 = 320;
    let (w, h) = (img.width(), img.height());
    // un-premultiply, then let the image crate do a good-quality downscale
    let mut rgb = image::RgbImage::new(w, h);
    for (d, s) in rgb.pixels_mut().zip(img.data().chunks_exact(4)) {
        let a = s[3] as f32 / 255.0;
        let un = |v: u8| if a > 0.0 { (v as f32 / a).min(255.0) as u8 } else { 0 };
        *d = image::Rgb([un(s[0]), un(s[1]), un(s[2])]);
    }
    let small = image::imageops::resize(&rgb, S, S, image::imageops::FilterType::Triangle);
    let max = small.as_raw().iter().copied().max().unwrap_or(1).max(1) as f32;
    let mean = [0.485, 0.456, 0.406];
    let std = [0.229, 0.224, 0.225];
    let input: Tensor = tract_ndarray::Array4::from_shape_fn((1, 3, S as usize, S as usize), |(_, c, y, x)| {
        let v = small.get_pixel(x as u32, y as u32)[c] as f32 / max;
        (v - mean[c]) / std[c]
    })
    .into();
    let out = plan.run(tvec!(input.into())).map_err(|e| format!("AI model failed: {e}"))?;
    let pred = out[0].to_plain_array_view::<f32>().map_err(|e| e.to_string())?;
    let vals: Vec<f32> = pred.iter().copied().collect();
    if vals.len() != (S * S) as usize {
        return Err(format!("unexpected model output ({} values)", vals.len()));
    }
    let (lo, hi) = vals.iter().fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    let span = (hi - lo).max(1e-6);
    // bilinear upscale to the picture size
    let n = S as usize;
    let mut alpha = vec![0f32; (w * h) as usize];
    for y in 0..h {
        let fy = ((y as f32 + 0.5) / h as f32 * S as f32 - 0.5).clamp(0.0, (S - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        let y1 = (y0 + 1).min(n - 1);
        for x in 0..w {
            let fx = ((x as f32 + 0.5) / w as f32 * S as f32 - 0.5).clamp(0.0, (S - 1) as f32);
            let (x0, tx) = (fx.floor() as usize, fx.fract());
            let x1 = (x0 + 1).min(n - 1);
            let g = |yy: usize, xx: usize| (vals[yy * n + xx] - lo) / span;
            let top = g(y0, x0) * (1.0 - tx) + g(y0, x1) * tx;
            let bottom = g(y1, x0) * (1.0 - tx) + g(y1, x1) * tx;
            alpha[(y * w + x) as usize] = top * (1.0 - ty) + bottom * ty;
        }
    }
    Ok(refine(&alpha, img))
}

/// Guided filter (He et al.): snaps the low-resolution prediction to the picture's real edges.
pub(crate) fn refine(p: &[f32], img: &Pixmap) -> Vec<f32> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let guide: Vec<f32> = img
        .data()
        .chunks_exact(4)
        .map(|c| (0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32) / 255.0)
        .collect();
    let sigma = (w.max(h) as f32 / 320.0 * 2.0).clamp(1.5, 12.0);
    let eps = 1e-3;
    let blur = |v: &[f32]| {
        let mut b = v.to_vec();
        gaussian(&mut b, w, h, sigma, true);
        b
    };
    let mean_i = blur(&guide);
    let mean_p = blur(p);
    let ii: Vec<f32> = guide.iter().map(|v| v * v).collect();
    let ip: Vec<f32> = guide.iter().zip(p).map(|(a, b)| a * b).collect();
    let corr_i = blur(&ii);
    let corr_ip = blur(&ip);
    let mut a = vec![0f32; w * h];
    let mut b = vec![0f32; w * h];
    for i in 0..w * h {
        let var = corr_i[i] - mean_i[i] * mean_i[i];
        let cov = corr_ip[i] - mean_i[i] * mean_p[i];
        a[i] = cov / (var + eps);
        b[i] = mean_p[i] - a[i] * mean_i[i];
    }
    let (ma, mb) = (blur(&a), blur(&b));
    (0..w * h)
        .map(|i| {
            let q = ma[i] * guide[i] + mb[i];
            // keep the model's confident areas, use the refined edge in between
            let v = if p[i] > 0.97 || p[i] < 0.03 { p[i] } else { q };
            ((v - 0.08) / 0.84).clamp(0.0, 1.0)
        })
        .collect()
}

/// Turn a foreground probability map into a grey mask pixmap.
pub fn alpha_to_mask(alpha: &[f32], w: u32, h: u32) -> Option<Pixmap> {
    let mut m = Pixmap::new(w, h)?;
    for (px, a) in m.data_mut().chunks_exact_mut(4).zip(alpha) {
        let v = (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        px.copy_from_slice(&[v, v, v, 255]);
    }
    Some(m)
}

/// Command-line helper: cut the subject out of a picture file and save it as a transparent PNG.
pub fn cutout_file(input: &str, output: &str, model: AiModel) -> Result<(), String> {
    let img = crate::imageops::load_image(Path::new(input))?;
    let t0 = std::time::Instant::now();
    let plan = load_plan(model).map_err(|e| e.to_string())?;
    let t1 = t0.elapsed();
    let alpha = segment(&plan, &img)?;
    println!("model load {:?}, cut-out {:?}", t1, t0.elapsed() - t1);
    let mut out = img.clone();
    for (px, a) in out.data_mut().chunks_exact_mut(4).zip(&alpha) {
        for v in px.iter_mut() {
            *v = (*v as f32 * a + 0.5) as u8;
        }
    }
    out.save_png(output).map_err(|e| e.to_string())
}

pub struct AiJob {
    pub id: u64,
    pub model: AiModel,
    pub img: Arc<Pixmap>,
}

pub enum AiEvent {
    Mask { id: u64, mask: Pixmap },
    Error(String),
}

/// Background thread for inference, so the editor never freezes. Loaded models stay cached.
pub struct AiWorker {
    tx: Sender<AiJob>,
    pub rx: Receiver<AiEvent>,
}

impl AiWorker {
    pub fn new(on_event: impl Fn() + Send + 'static) -> Self {
        let (jtx, jrx) = channel::<AiJob>();
        let (etx, erx) = channel::<AiEvent>();
        std::thread::spawn(move || {
            let mut plans: HashMap<AiModel, Plan> = HashMap::new();
            while let Ok(job) = jrx.recv() {
                let plan = match plans.get(&job.model) {
                    Some(p) => Ok(p.clone()),
                    None => load_plan(job.model)
                        .map_err(|e| format!("could not load the AI model: {e}"))
                        .inspect(|p| {
                            plans.insert(job.model, p.clone());
                        }),
                };
                let ev = match plan.and_then(|p| segment(&p, &job.img)) {
                    Ok(alpha) => match alpha_to_mask(&alpha, job.img.width(), job.img.height()) {
                        Some(mask) => AiEvent::Mask { id: job.id, mask },
                        None => AiEvent::Error("picture too large".into()),
                    },
                    Err(e) => AiEvent::Error(e),
                };
                if etx.send(ev).is_err() {
                    break;
                }
                on_event();
            }
        });
        Self { tx: jtx, rx: erx }
    }

    pub fn submit(&self, job: AiJob) {
        let _ = self.tx.send(job);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A red ball on a pale, slightly noisy background.
    fn photo() -> Pixmap {
        let mut pm = Pixmap::new(240, 180).unwrap();
        for (i, px) in pm.data_mut().chunks_exact_mut(4).enumerate() {
            let n = (crate::textures::rand01((i % 240) as i32, (i / 240) as i32, 9) * 20.0) as u8;
            px.copy_from_slice(&[225 + n / 2, 228 + n / 2, 215 + n / 2, 255]);
        }
        let mut paint = tiny_skia::Paint::default();
        paint.shader = tiny_skia::RadialGradient::new(
                tiny_skia::Point::from_xy(105.0, 75.0),
                tiny_skia::Point::from_xy(120.0, 90.0),
                55.0,
                vec![
                    tiny_skia::GradientStop::new(0.0, tiny_skia::Color::from_rgba8(255, 120, 100, 255)),
                    tiny_skia::GradientStop::new(1.0, tiny_skia::Color::from_rgba8(150, 20, 20, 255)),
                ],
                tiny_skia::SpreadMode::Pad,
                tiny_skia::Transform::identity(),
            )
            .unwrap();
        let ball = tiny_skia::PathBuilder::from_circle(120.0, 90.0, 50.0).unwrap();
        pm.fill_path(&ball, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
        pm
    }

    #[test]
    fn u2net_finds_the_subject_with_both_embedded_models() {
        let img = photo();
        for model in AiModel::ALL {
            let plan = load_plan(model).expect("embedded model loads");
            let a = segment(&plan, &img).unwrap();
            let at = |x: usize, y: usize| a[y * 240 + x];
            assert!(at(120, 90) > 0.8, "{model:?}: ball kept ({})", at(120, 90));
            assert!(at(5, 5) < 0.1 && at(230, 170) < 0.1, "{model:?}: background removed");
        }
    }
}
