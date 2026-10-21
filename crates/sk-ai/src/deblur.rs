//! Removing camera shake and motion blur with NAFNet (Liangyu Chen et al., Megvii; MIT), the
//! model trained on REDS as exported to ONNX by Qualcomm AI Hub. Computes the difference that
//! `sk_core::ai_deblur` keeps and adds to the image.
//!
//! The network knows 8-bit sRGB photos: the demosaiced data is white balanced, brought to the
//! working space, sRGB encoded, sharpened in overlapping tiles and decoded again. Only what the
//! network changed is kept, in camera RGB like the source.

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rayon::prelude::*;
use sk_core::color::{mat_inv, mat_vec, srgb_decode, srgb_encode};
use sk_core::image::LinearImage;

use crate::models::{self, DEBLUR, DEBLUR_WEIGHTS};
use crate::runtime::{Input, Net};
use crate::tiles::{Tiling, run_tiled};

/// The network's fixed input (width x height).
const WIDTH: usize = 640;
const HEIGHT: usize = 360;
const OVERLAP: usize = 48;
/// At most this many tiles at once on the processor (each needs several hundred MB).
const PARALLEL_TILES: usize = 4;

/// Identifies the model for the result cache.
pub fn tag() -> String {
    format!("nafnet-reds-{}-{}", &DEBLUR.sha256[..12], &DEBLUR_WEIGHTS.sha256[..12])
}

/// Both files of the model are downloaded.
pub fn available() -> bool {
    models::available(&DEBLUR) && models::available(&DEBLUR_WEIGHTS)
}

/// Downloads the model: the network and its weights (a file of their own).
pub fn download(progress: &dyn Fn(f32)) -> Result<()> {
    let total = (DEBLUR.bytes + DEBLUR_WEIGHTS.bytes) as f32;
    let mut done = 0.0;
    for m in [DEBLUR, DEBLUR_WEIGHTS] {
        if !models::available(&m) {
            models::download(&m, &|p| progress((done + p * m.bytes as f32) / total))?;
        }
        done += m.bytes as f32;
    }
    Ok(())
}

/// Download size in MB (compressed in the archive the weights are about 7 % smaller).
pub fn download_mb() -> u64 {
    (DEBLUR.bytes + DEBLUR_WEIGHTS.bytes) * 93 / 100 / 1_000_000
}

/// The loaded network, kept between images (loaded again when the GPU setting changes).
#[derive(Default)]
pub struct Deblurrer {
    net: Mutex<Option<(u64, Arc<Net>)>>,
}

impl Deblurrer {
    fn net(&self) -> Result<Arc<Net>> {
        let generation = crate::runtime::config_generation();
        let mut slot = self.net.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((g, n)) = &*slot
            && *g == generation
        {
            return Ok(n.clone());
        }
        anyhow::ensure!(available(), "{} ist nicht geladen", DEBLUR.label);
        let net = Arc::new(Net::load(&models::path(&DEBLUR), &[Input::F32([1, 3, HEIGHT, WIDTH])])?);
        *slot = Some((generation, net.clone()));
        Ok(net)
    }

    /// What the network changes in a raw file: camera RGB, oriented, the size of `raw::load_full`.
    pub fn product(&self, path: &Path, progress: &dyn Fn(f32)) -> Result<LinearImage> {
        let (meta, plain) = sk_core::raw::load_full(path).with_context(|| format!("{}", path.display()))?;
        anyhow::ensure!(!meta.hdr, "Für zusammengesetzte HDR-Bilder nicht verfügbar");
        progress(0.02);
        self.difference(&meta, &plain, &|p| progress(0.02 + 0.98 * p))
    }

    /// The network's change of `plain` (demosaiced camera RGB).
    fn difference(&self, meta: &sk_core::raw::RawMeta, plain: &LinearImage, progress: &dyn Fn(f32)) -> Result<LinearImage> {
        let wb = meta.as_shot_wb;
        let m = meta.color.cam_to_working;
        let inv = mat_inv(&m).context("Farbmatrix nicht umkehrbar")?;
        let (w, h) = (plain.width, plain.height);
        let working: Vec<[f32; 3]> = plain.data.par_iter().map(|p| mat_vec(&m, [p[0] * wb[0], p[1] * wb[1], p[2] * wb[2]])).collect();
        let planes: Vec<Vec<f32>> = (0..3).into_par_iter().map(|c| working.iter().map(|p| srgb_encode(p[c].clamp(0.0, 1.0))).collect()).collect();
        let tiling = Tiling { width: WIDTH, height: HEIGHT, overlap: OVERLAP, scale: 1, parallel: PARALLEL_TILES };
        let out = run_tiled(&*self.net()?, &planes, w, h, tiling, None, progress)?;
        drop(planes);
        let data = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let p = working[i];
                // Clipped and out-of-gamut parts were clipped for the network: they stay as they were.
                let bright = p.iter().copied().fold(f32::MIN, f32::max);
                let dark = p.iter().copied().fold(f32::MAX, f32::min);
                let keep = (1.0 - (bright - 0.9) / 0.08).clamp(0.0, 1.0) * (dark / 1e-4 + 1.0).clamp(0.0, 1.0);
                let sharp: [f32; 3] = std::array::from_fn(|c| srgb_decode(out[c][i].clamp(0.0, 1.0)));
                let d = mat_vec(&inv, [0, 1, 2].map(|c| (sharp[c] - p[c].clamp(0.0, 1.0)) * keep));
                [d[0] / wb[0], d[1] / wb[1], d[2] / wb[2]]
            })
            .collect();
        Ok(LinearImage::new(w, h, data))
    }
}

/// Registers NAFNet as the provider of `sk_core::ai_deblur` (whether or not it is downloaded:
/// computing fails with a message until it is).
pub fn register() {
    let deblurrer = Deblurrer::default();
    sk_core::ai_deblur::set_provider(&tag(), Box::new(move |path, progress| deblurrer.product(path, progress)));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With the downloaded model: a blurred edge gets steeper, flat parts stay as they were.
    #[test]
    #[ignore = "langsam: läuft mit cargo t-all"]
    fn sharpens_a_blurred_edge_if_the_model_is_available() {
        if !available() {
            return;
        }
        let d = Deblurrer::default();
        let (w, h) = (700, 400);
        // A vertical edge from dark to light, blurred over 12 pixels.
        let level = |x: usize| 0.05 + 0.3 * ((x as f32 - 344.0) / 12.0).clamp(0.0, 1.0);
        let plain = LinearImage::new(w, h, (0..w * h).map(|i| [level(i % w); 3]).collect());
        let meta = sk_core::raw::RawMeta {
            path: Default::default(),
            info: Default::default(),
            color: sk_core::color::CameraColor::new(mat_inv(&sk_core::color::REC2020_TO_XYZ).unwrap()),
            as_shot_wb: [1.0; 3],
            orientation: sk_core::image::Orientation::Normal,
            hdr: false,
            lens: None,
            lens_corrected: false,
        };
        let diff = d.difference(&meta, &plain, &|_| {}).unwrap();
        let at = |x: usize| plain.data[200 * w + x][1] + diff.data[200 * w + x][1];
        let (before, after) = (level(353) - level(347), at(353) - at(347));
        assert!(after > before * 1.15, "Kante vorher {before}, nachher {after}");
        assert!((at(100) - level(100)).abs() < 0.01 && (at(600) - level(600)).abs() < 0.02, "{} {}", at(100), at(600));
    }
}
