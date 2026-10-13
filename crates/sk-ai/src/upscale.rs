//! KI-Vergrößern (see `sk_core::upscale`): RealPLKSR (MIT), which adds no invented texture. It
//! takes output-encoded RGB 0..1 in tiles of fixed size and returns twice or four times the
//! edges; the model for each factor is downloaded on its own when first needed.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use rayon::prelude::*;
use sk_core::image::LinearImage;
use sk_core::upscale::Upscale;

use crate::models::{self, ModelInfo, UPSCALE_X2, UPSCALE_X4};
use crate::runtime::{Input, Net};
use crate::tiles::{Tiling, run_tiled};

pub fn model(u: Upscale) -> &'static ModelInfo {
    if u.factor == 4 { &UPSCALE_X4 } else { &UPSCALE_X2 }
}

/// The networks take 512 x 512 (2×) or 256 x 256 (4×) and always return 1024 x 1024.
fn tiling(u: Upscale) -> Tiling {
    let tile = 1024 / u.factor as usize;
    Tiling { tile, overlap: tile / 8, scale: u.factor as usize, parallel: 6 }
}

/// The loaded networks with the runtime configuration they were loaded with.
#[derive(Default)]
pub struct Upscaler {
    nets: Mutex<Vec<(Upscale, u64, Arc<Net>)>>,
}

impl Upscaler {
    fn net(&self, u: Upscale) -> Result<Arc<Net>> {
        let generation = crate::runtime::config_generation();
        let mut nets = self.nets.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, _, n)) = nets.iter().find(|(k, g, _)| *k == u && *g == generation) {
            return Ok(n.clone());
        }
        let m = model(u);
        anyhow::ensure!(models::available(m), "{} ist nicht geladen", m.label);
        let t = tiling(u);
        let net = Arc::new(Net::load(&models::path(m), &[Input::F32([1, 3, t.tile, t.tile])])?);
        // One network at a time: each holds a lot of memory.
        nets.clear();
        nets.push((u, generation, net.clone()));
        Ok(net)
    }

    pub fn upscale(&self, img: &LinearImage, u: Upscale, progress: &dyn Fn(f32)) -> Result<LinearImage> {
        let (w, h) = (img.width, img.height);
        let planes: Vec<Vec<f32>> = (0..3).into_par_iter().map(|c| img.data.iter().map(|p| p[c].clamp(0.0, 1.0)).collect()).collect();
        let out = run_tiled(&*self.net(u)?, &planes, w, h, tiling(u), None, progress)?;
        let k = u.factor as usize;
        let data = (0..k * w * k * h).into_par_iter().map(|i| [0, 1, 2].map(|c| out[c][i].clamp(0.0, 1.0))).collect();
        Ok(LinearImage::new(k * w, k * h, data))
    }
}

/// Registers the networks as the provider of `sk_core::upscale` (computing fails with a message
/// while a model is not downloaded).
pub fn register() {
    let upscaler = Upscaler::default();
    sk_core::upscale::set_provider(Box::new(move |img, u, progress| upscaler.upscale(img, u, progress)));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With the downloaded model: a sharp edge stays sharp and in place, flat areas keep their value.
    #[test]
    fn enlarges_an_edge_if_the_model_is_available() {
        let u = Upscale::default();
        if !models::available(model(u)) {
            return;
        }
        let (w, h) = (96, 80);
        let img = LinearImage::new(w, h, (0..w * h).map(|i| if i % w < w / 2 { [0.2, 0.3, 0.4] } else { [0.8, 0.7, 0.6] }).collect());
        let out = Upscaler::default().upscale(&img, u, &|_| {}).unwrap();
        assert_eq!((out.width, out.height), (2 * w, 2 * h));
        let at = |x: usize, y: usize| out.data[y * out.width + x];
        for c in 0..3 {
            assert!((at(20, 80)[c] - img.data[0][c]).abs() < 0.03, "links {:?}", at(20, 80));
            assert!((at(170, 80)[c] - img.data[w - 1][c]).abs() < 0.03, "rechts {:?}", at(170, 80));
        }
        // The edge between pixel 95 and 96 of the output: at most two pixels of transition.
        assert!(at(93, 80)[0] < 0.3 && at(98, 80)[0] > 0.7, "{:?} {:?}", at(93, 80), at(98, 80));
    }
}
