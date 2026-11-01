//! Super Resolution: a raw file becomes a new linear DNG with twice the edges (four times the
//! pixels), next to the original as `<name>-SR.dng`. Like a merge, the result stays a raw file:
//! white balance, color and tone remain fully editable.
//!
//! The network is SPAN (Cheng Wan, Hongyuan Yu et al., NTIRE 2024; the official 2× weights,
//! Apache-2.0), small enough to come with the program: a 24 MP image takes about 30 s on a
//! GTX 1650 Ti. It is trained on finished pictures without noise, so it takes the image as
//! "Verbessern" leaves it (KI-Entrauschen and Raw-Details as set for the image; noise would
//! otherwise become sharp specks), white balanced and sRGB-encoded, scaled so that nothing
//! clips; that is undone afterwards. Clipped highlights, which the network knows nothing about,
//! are only enlarged smoothly, so the highlight reconstruction still finds them.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, ensure};
use rayon::prelude::*;
use sk_core::color::{srgb_decode, srgb_encode};
use sk_core::image::LinearImage;

use crate::runtime::{Input, Net};
use crate::tiles::{Tiling, run_tiled};

/// The network, exported from the published weights (see `models/README.md`).
const MODEL: &[u8] = include_bytes!("../models/span_x2.onnx");
/// Its license, shown in the program (Apache-2.0 asks for that with every copy).
pub const LICENSE: &str = include_str!("../models/span_LICENSE");
/// Where it is written for loading (ONNX Runtime loads from a file).
const FILE: &str = "span_x2.onnx";

/// Output pixels per input pixel, per edge.
pub const FACTOR: usize = 2;
/// Larger results are refused: computing them needs about 30 bytes per output pixel.
pub const MAX_PIXELS: usize = 250_000_000;
/// Name suffix of the new file.
pub const SUFFIX: &str = "-SR";

/// The network takes 512 x 512.
const TILING: Tiling = Tiling { tile: 512, overlap: 32, scale: FACTOR, parallel: 6 };

/// The model file, written from the program if it is missing or differs.
fn model_file() -> Result<PathBuf> {
    let path = crate::models::models_dir().join(FILE);
    if std::fs::read(&path).ok().as_deref() != Some(MODEL) {
        std::fs::create_dir_all(crate::models::models_dir())?;
        sk_core::safe_file::write(&path, MODEL).with_context(|| format!("{} nicht schreibbar", path.display()))?;
    }
    Ok(path)
}

/// The network, kept between images (loaded again when the GPU setting changes).
static NET: Mutex<Option<(u64, Arc<Net>)>> = Mutex::new(None);

fn net() -> Result<Arc<Net>> {
    let generation = crate::runtime::config_generation();
    let mut slot = NET.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((g, n)) = &*slot
        && *g == generation
    {
        return Ok(n.clone());
    }
    let t = TILING.tile;
    let net = Arc::new(Net::load(&model_file()?, &[Input::F32([1, 3, t, t])])?);
    *slot = Some((generation, net.clone()));
    Ok(net)
}

/// Enlarges camera RGB (linear, not white balanced) with the as-shot multipliers `wb`.
/// `clipped`: values near 1.0 are clipped sensor data (not a merged or floating point file).
pub fn enlarge(img: &LinearImage, wb: [f32; 3], clipped: bool, progress: &dyn Fn(f32)) -> Result<LinearImage> {
    let (w, h) = (img.width, img.height);
    let (ow, oh) = (FACTOR * w, FACTOR * h);
    ensure!(
        ow * oh <= MAX_PIXELS,
        "Super Resolution: das Ergebnis hätte {} MP, höchstens {} MP sind möglich",
        ow * oh / 1_000_000,
        MAX_PIXELS / 1_000_000
    );
    ensure!(w >= 2 && h >= 2, "Bild zu klein");
    // White balanced, the brightest value at 1.0 at most (merged files go above it).
    let peak = img.data.par_iter().map(|p| (0..3).map(|c| p[c] * wb[c]).fold(0.0f32, f32::max)).reduce(|| 0.0, f32::max);
    let scale = peak.max(wb.iter().copied().fold(0.0, f32::max)).max(1e-6);
    let gain = wb.map(|m| m / scale);
    let planes: Vec<Vec<f32>> = (0..3).into_par_iter().map(|c| img.data.iter().map(|p| srgb_encode((p[c] * gain[c]).clamp(0.0, 1.0))).collect()).collect();
    let out = run_tiled(&*net()?, &planes, w, h, TILING, None, progress)?;
    drop(planes);
    let data = (0..ow * oh)
        .into_par_iter()
        .map(|i| {
            let sr = [0, 1, 2].map(|c| srgb_decode(out[c][i].clamp(0.0, 1.0)) / gain[c]);
            if !clipped {
                return sr;
            }
            let smooth = bilinear(img, (i % ow) as f32 / FACTOR as f32, (i / ow) as f32 / FACTOR as f32);
            let keep = ((smooth.iter().copied().fold(0.0f32, f32::max) - 0.9) / 0.08).clamp(0.0, 1.0);
            [0, 1, 2].map(|c| (sr[c] + (smooth[c] - sr[c]) * keep).clamp(0.0, 1.0))
        })
        .collect();
    Ok(LinearImage::new(ow, oh, data))
}

/// `img` at output pixel centers mapped back into it (`x`, `y` in input pixels from the corner).
fn bilinear(img: &LinearImage, x: f32, y: f32) -> [f32; 3] {
    let (w, h) = (img.width, img.height);
    let fx = (x + 0.5 / FACTOR as f32 - 0.5).clamp(0.0, (w - 1) as f32);
    let fy = (y + 0.5 / FACTOR as f32 - 0.5).clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let at = |x: usize, y: usize| img.data[y * w + x];
    let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
    [0, 1, 2].map(|k| (a[k] * (1.0 - tx) + b[k] * tx) * (1.0 - ty) + (c[k] * (1.0 - tx) + d[k] * tx) * ty)
}

/// What "Verbessern" adds to an image before it is enlarged (amounts 0..1, as set for it).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Enhanced {
    pub raw_details: f32,
    pub ai_denoise: f32,
}

impl Enhanced {
    pub fn of(adj: &sk_core::params::Adjustments) -> Self {
        Self { raw_details: adj.detail.effective_raw_details() / 100.0, ai_denoise: adj.detail.ai_denoise / 100.0 }
    }
}

/// Enlarges the raw file `path` into `<name>-SR.dng` next to it, with Raw-Details and
/// KI-Entrauschen by `enhanced` (computed first if needed). `progress` gets 0..1.
pub fn enlarge_file(path: &Path, enhanced: Enhanced, progress: &dyn Fn(f32)) -> Result<PathBuf> {
    let context = || format!("{}", path.display());
    let meta = sk_core::raw::read_meta(path).with_context(context)?;
    // Merged files have no raw data to improve (as in the pipeline).
    let details = if meta.bayer && !meta.hdr { enhanced.raw_details } else { 0.0 };
    let denoise = if meta.hdr { 0.0 } else { enhanced.ai_denoise };
    // Raw-Details and KI-Entrauschen as the image shows them, computed first if needed.
    let products: Vec<&sk_core::ai_products::Products> =
        [(details, &sk_core::ai_products::DETAILS), (denoise, &sk_core::ai_products::DENOISE)].into_iter().filter(|(a, _)| *a > 0.0).map(|(_, p)| p).collect();
    let share = 0.4 * products.len() as f32 / 2.0;
    for (i, p) in products.iter().enumerate() {
        let n = products.len() as f32;
        p.ensure(path, &|q| progress(share * (i as f32 + q) / n)).context(p.label)?;
    }
    let (meta, plain) = sk_core::raw::load_full(path).with_context(context)?;
    let prepared = sk_core::ai_products::prepared(path, &plain, details, denoise);
    let src = prepared.as_deref().unwrap_or(&plain);
    let rest = 1.0 - share;
    progress(share + 0.05 * rest);
    let big = enlarge(src, meta.as_shot_wb, !meta.hdr, &|p| progress(share + rest * (0.05 + 0.85 * p)))?;
    drop(prepared);
    drop(plain);
    let out = sk_core::hdr::output_path(path, SUFFIX);
    let preview = sk_core::dng::quick_preview(&meta, &big, 1024);
    sk_core::dng::write_linear_dng(
        &out,
        &sk_core::dng::LinearDng {
            width: big.width,
            height: big.height,
            data: &big.data,
            color: &meta.color,
            as_shot_wb: meta.as_shot_wb,
            info: &meta.info,
            baseline_exposure: 0.0,
            preview: Some(&preview),
            lens_corrected: meta.lens_corrected,
        },
    )?;
    progress(1.0);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Twice the size, colors kept, a sharp edge stays sharp and in
    /// place, clipped highlights stay clipped.
    #[test]
    fn enlarges_an_edge() {
        let (w, h) = (96, 80);
        let (dark, light) = ([0.05, 0.12, 0.08], [0.3, 0.55, 0.4]);
        let img = LinearImage::new(
            w,
            h,
            (0..w * h)
                .map(|i| match (i % w, i / w) {
                    (x, y) if x >= 70 && y < 20 => [1.0; 3],
                    (x, _) if x < w / 2 => dark,
                    _ => light,
                })
                .collect(),
        );
        let wb = [2.0, 1.0, 1.6];
        let out = enlarge(&img, wb, true, &|_| {}).unwrap();
        assert_eq!((out.width, out.height), (2 * w, 2 * h));
        let at = |x: usize, y: usize| out.data[y * out.width + x];
        for c in 0..3 {
            assert!((at(20, 120)[c] - dark[c]).abs() < 0.01, "links {:?}", at(20, 120));
            assert!((at(130, 120)[c] - light[c]).abs() < 0.02, "rechts {:?}", at(130, 120));
            assert!(at(170, 10)[c] > 0.98, "Lichter {:?}", at(170, 10));
        }
        // The edge between pixel 95 and 96 of the output: at most two pixels of transition.
        assert!(at(93, 120)[1] < 0.2 && at(98, 120)[1] > 0.45, "{:?} {:?}", at(93, 120), at(98, 120));
    }

    #[test]
    fn bilinear_matches_flat_and_corners() {
        let img = LinearImage::new(2, 1, vec![[0.0; 3], [1.0; 3]]);
        assert_eq!(bilinear(&img, 0.0, 0.0), [0.0; 3]);
        assert_eq!(bilinear(&img, 1.5, 0.0), [1.0; 3]);
        // Output pixel 1 (center 0.75 in input pixels) lies a quarter between the two centers.
        assert!((bilinear(&img, 0.5, 0.0)[0] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn refuses_results_too_large() {
        let huge = LinearImage { width: 10_000, height: 8_000, data: Vec::new() };
        let e = enlarge(&huge, [1.0; 3], true, &|_| {}).unwrap_err();
        assert!(format!("{e:#}").contains("höchstens"), "{e:#}");
    }
}
