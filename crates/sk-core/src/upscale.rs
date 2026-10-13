//! KI-Vergrößern on export ("Super Resolution"): a neural network (in `sk-ai`) doubles or
//! quadruples the edges of the developed image before it is written. The networks are trained
//! on finished pictures, so they run at the very end, on the output-encoded pixels; the
//! development itself is unchanged.
//!
//! sk-core cannot run networks; the application registers the function that does it
//! ([`set_provider`]).

use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::image::LinearImage;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Upscale {
    /// 2 or 4 (per edge).
    pub factor: u8,
}

impl Default for Upscale {
    fn default() -> Self {
        Self { factor: 2 }
    }
}

/// Larger results are refused: computing them needs about 30 bytes per output pixel.
pub const MAX_PIXELS: usize = 250_000_000;

/// Enlarges output-encoded RGB (0..1) by the factor (with progress 0..1).
pub type UpscaleFn = dyn Fn(&LinearImage, Upscale, &dyn Fn(f32)) -> Result<LinearImage> + Send + Sync;

static PROVIDER: RwLock<Option<Arc<UpscaleFn>>> = RwLock::new(None);

/// Registers the function that enlarges.
pub fn set_provider(f: Box<UpscaleFn>) {
    *PROVIDER.write().unwrap() = Some(Arc::from(f));
}

/// Enlarges `img` (output-encoded, 0..1) by `u.factor` per edge.
pub fn upscale(img: &LinearImage, u: Upscale, progress: &dyn Fn(f32)) -> Result<LinearImage> {
    anyhow::ensure!(matches!(u.factor, 2 | 4), "KI-Vergrößern: Faktor {} wird nicht unterstützt", u.factor);
    let pixels = img.width * img.height * (u.factor as usize).pow(2);
    anyhow::ensure!(
        pixels <= MAX_PIXELS,
        "KI-Vergrößern: {} MP sind zu groß (höchstens {} MP). Kleineren Faktor wählen oder vorher beschneiden.",
        pixels / 1_000_000,
        MAX_PIXELS / 1_000_000
    );
    let f = PROVIDER.read().unwrap().clone().context("KI-Vergrößern ist nicht eingerichtet")?;
    let out = f(img, u, progress)?;
    let k = u.factor as usize;
    anyhow::ensure!((out.width, out.height) == (k * img.width, k * img.height), "KI-Vergrößern: falsche Bildgröße {}×{}", out.width, out.height);
    Ok(out)
}

/// Nearest neighbour as a stand-in for the network (tests).
#[cfg(test)]
pub(crate) fn register_stand_in() {
    set_provider(Box::new(|img, u, progress| {
        let k = u.factor as usize;
        let (w, h) = (img.width * k, img.height * k);
        progress(0.5);
        Ok(LinearImage::new(w, h, (0..w * h).map(|i| img.data[(i / w / k) * img.width + (i % w) / k]).collect()))
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_factor_size_and_result() {
        let img = LinearImage::new(4, 2, vec![[0.5; 3]; 8]);
        let bad = Upscale { factor: 3 };
        assert!(format!("{:#}", upscale(&img, bad, &|_| {}).unwrap_err()).contains("Faktor 3"));
        let huge = LinearImage { width: 20_000, height: 10_000, data: Vec::new() };
        assert!(format!("{:#}", upscale(&huge, Upscale::default(), &|_| {}).unwrap_err()).contains("zu groß"));
        register_stand_in();
        let out = upscale(&img, Upscale { factor: 4 }, &|_| {}).unwrap();
        assert_eq!((out.width, out.height), (16, 8));
    }
}
