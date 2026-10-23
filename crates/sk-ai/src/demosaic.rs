//! Raw-Details: the Bayer mosaic demosaiced by a network instead of AHD. DemosaicNet (Michaël
//! Gharbi et al., "Deep Joint Demosaicking and Denoising", 2016; MIT, shipped with Silberkorn,
//! see `models/README.md`) was trained on the cases demosaicing gets wrong (fine lines, repeating
//! patterns): it leaves fewer false colors, zipper edges and mazes in fine detail. Computes the
//! product that `sk_core::ai_products::DETAILS` keeps and blends into the image.
//!
//! The network learned on white balanced, gamma encoded images: the mosaic is balanced and
//! encoded before, decoded and unbalanced after. It takes three planes (zero where a photosite
//! has another color) in the pattern G R / B G, and has no padding: each tile's output is its
//! input less [`MARGIN`] on every side, so tiles overlapping by that much fit without a seam.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rayon::prelude::*;
use sk_core::image::LinearImage;
use sk_core::raw::BayerMosaic;

use crate::runtime::{Data, Input, Net};

/// The network, exported from the published weights.
const MODEL: &[u8] = include_bytes!("../models/demosaicnet_bayer.onnx");
/// Its license, shown in the program (MIT asks for that with every copy).
pub const LICENSE: &str = include_str!("../models/demosaicnet_LICENSE");
/// Where it is written for loading (ONNX Runtime loads from a file).
const FILE: &str = "demosaicnet_bayer.onnx";
/// Input edge of a tile and how much the output is smaller on each side.
const TILE: usize = 768;
const MARGIN: usize = 31;
const OUT: usize = TILE - 2 * MARGIN;
/// At most this many tiles at once on the processor.
const PARALLEL_TILES: usize = 6;
/// Encoding of the network's input (it learned on sRGB images).
const GAMMA: f32 = 2.2;

/// Identifies the model for the product cache.
pub fn tag() -> String {
    format!("demosaicnet-{:016x}", sk_core::cache::fnv1a(MODEL))
}

/// The model file, written from the program if it is missing or differs.
fn model_file() -> Result<PathBuf> {
    let path = crate::models::models_dir().join(FILE);
    if std::fs::read(&path).ok().as_deref() != Some(MODEL) {
        std::fs::create_dir_all(crate::models::models_dir())?;
        sk_core::safe_file::write(&path, MODEL).with_context(|| format!("{} nicht schreibbar", path.display()))?;
    }
    Ok(path)
}

/// The loaded network and the runtime configuration it was loaded with.
#[derive(Default)]
pub struct Demosaicer {
    net: Mutex<Option<(u64, Arc<Net>)>>,
}

impl Demosaicer {
    fn net(&self) -> Result<Arc<Net>> {
        let generation = crate::runtime::config_generation();
        let mut slot = self.net.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((g, n)) = &*slot
            && *g == generation
        {
            return Ok(n.clone());
        }
        let net = Arc::new(Net::load(&model_file()?, &[Input::F32([1, 3, TILE, TILE])])?);
        *slot = Some((generation, net.clone()));
        Ok(net)
    }

    /// The product of a raw file: camera RGB, oriented, the size of `raw::load_full`.
    pub fn product(&self, path: &Path, progress: &dyn Fn(f32)) -> Result<LinearImage> {
        let context = || format!("{}", path.display());
        let (meta, mosaic) = sk_core::raw::load_bayer(path).with_context(context)?.context("Raw-Details gibt es nur für Bayer-Sensoren")?;
        anyhow::ensure!(!meta.hdr, "Für zusammengesetzte HDR-Bilder nicht verfügbar");
        progress(0.02);
        let mut img = self.demosaic(&mosaic, meta.as_shot_wb, &|p| progress(0.02 + 0.98 * p))?.oriented(meta.orientation);
        drop(mosaic);
        let (_, plain) = sk_core::raw::load_full(path).with_context(context)?;
        anyhow::ensure!((img.width, img.height) == (plain.width, plain.height), "Bildgröße passt nicht ({}×{})", img.width, img.height);
        // Clipped highlights, which the network knows nothing about, stay as AHD made them.
        img.data.par_iter_mut().zip(&plain.data).for_each(|(c, p)| {
            let keep = ((p.iter().copied().fold(0.0f32, f32::max) - 0.9) / 0.08).clamp(0.0, 1.0);
            *c = [0, 1, 2].map(|i| c[i] + (p[i] - c[i]) * keep);
        });
        Ok(img)
    }

    /// Bayer mosaic -> camera RGB of the mosaic's size (not oriented). `wb`: camera multipliers
    /// relative to green.
    fn demosaic(&self, m: &BayerMosaic, wb: [f32; 3], progress: &dyn Fn(f32)) -> Result<LinearImage> {
        let (rx, ry) = m.rggb_offset().context("Unbekanntes Bayer-Muster")?;
        // Green then red in the first row of each tile.
        let phase = ((rx + 1) % 2, ry);
        let (w, h) = (m.width, m.height);
        anyhow::ensure!(w >= 2 && h >= 2, "Bild zu klein");
        let wb = wb.map(|v| if v.is_finite() && v > 0.0 { v } else { 1.0 });
        let (xs, ys) = (starts(w, phase.0), starts(h, phase.1));
        let tiles: Vec<(isize, isize)> = ys.iter().flat_map(|&y| xs.iter().map(move |&x| (x, y))).collect();
        let net = self.net()?;
        let out = Mutex::new(vec![[0.0f32; 3]; w * h]);
        let run = |&(tx, ty): &(isize, isize)| -> Result<()> {
            let n = TILE * TILE;
            let mut input = vec![0.0f32; 3 * n];
            for j in 0..TILE {
                let y = reflect(ty + j as isize, h);
                for i in 0..TILE {
                    let x = reflect(tx + i as isize, w);
                    let c = m.pattern[y % 2][x % 2];
                    input[c * n + j * TILE + i] = (m.data[y * w + x] * wb[c]).clamp(0.0, 1.0).powf(1.0 / GAMMA);
                }
            }
            let res = net.run(&[Data::F32(&input)])?;
            let res = res.first().context("Keine Modellausgabe")?;
            let k = OUT * OUT;
            anyhow::ensure!(res.data.len() == 3 * k, "Unerwartete Modellausgabe {:?}", res.shape);
            anyhow::ensure!(res.data.iter().all(|v| v.is_finite()), "Das Modell lieferte ungültige Werte");
            let mut out = out.lock().unwrap_or_else(|e| e.into_inner());
            for j in 0..OUT {
                let y = ty + (MARGIN + j) as isize;
                if y < 0 || y as usize >= h {
                    continue;
                }
                for i in 0..OUT {
                    let x = tx + (MARGIN + i) as isize;
                    if x < 0 || x as usize >= w {
                        continue;
                    }
                    out[y as usize * w + x as usize] = [0, 1, 2].map(|c| res.data[c * k + j * OUT + i].max(0.0).powf(GAMMA) / wb[c]);
                }
            }
            Ok(())
        };
        // On ONNX Runtime the network runs one tile at a time; a second tile in flight prepares
        // its input and takes in its result meanwhile, so the card doesn't wait for that.
        let threads = if net.prefers_serial() { 2 } else { PARALLEL_TILES.min(rayon::current_num_threads()).max(1) };
        crate::tiles::run_all(&tiles, threads, run, progress)?;
        Ok(LinearImage::new(w, h, out.into_inner().unwrap_or_else(|e| e.into_inner())))
    }
}

/// Input origins of the tiles along an axis of `len` photosites: each output starts where the
/// previous one ended, the first covers position 0, and every origin has the parity of `phase`
/// (so every tile sees the mosaic in the network's pattern).
fn starts(len: usize, phase: usize) -> Vec<isize> {
    // OUT is even: the parity stays.
    let first = phase as isize - MARGIN as isize - 1;
    (0..).map(|k| first + (k * OUT) as isize).take_while(|&s| s + (MARGIN as isize) < len as isize).collect()
}

/// Mirrors a position outside `0..len` back in without repeating the edge, which keeps its
/// parity and with it the color of the photosite.
fn reflect(i: isize, len: usize) -> usize {
    let n = len as isize;
    let period = 2 * (n - 1);
    let mut i = i.rem_euclid(period.max(1));
    if i >= n {
        i = period - i;
    }
    i as usize
}

/// Registers DemosaicNet as the provider of `sk_core::ai_products::DETAILS`.
pub fn register() {
    let d = Demosaicer::default();
    sk_core::ai_products::DETAILS.set_provider(&tag(), Box::new(move |path, progress| d.product(path, progress)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_cover_every_photosite_once_in_phase() {
        for (len, phase) in [(10, 0), (10, 1), (OUT, 0), (OUT + 1, 1), (3000, 0), (3001, 1)] {
            let s = starts(len, phase);
            assert!(s.iter().all(|&x| (x - phase as isize).rem_euclid(2) == 0), "{s:?}");
            assert!(s[0] + MARGIN as isize <= 0);
            assert!(s.windows(2).all(|p| p[1] - p[0] == OUT as isize));
            assert!(*s.last().unwrap() + (MARGIN + OUT) as isize >= len as isize);
        }
        assert_eq!((0..8).map(|i| reflect(i - 3, 5)).collect::<Vec<_>>(), [3, 2, 1, 0, 1, 2, 3, 4]);
        assert_eq!((reflect(5, 5), reflect(6, 5), reflect(-1, 1)), (3, 2, 0));
    }

    /// A mosaic of a sharp-edged colored image: the network brings back its colors at the
    /// right brightness, also across tile borders, and edges as sharp as the AHD.
    #[test]
    fn demosaics_a_mosaic_like_ahd() {
        let (w, h) = (OUT + 100, 300);
        let truth = |x: usize, y: usize| -> [f32; 3] {
            let a = if (x / 37 + y / 23).is_multiple_of(2) { [0.40, 0.20, 0.08] } else { [0.05, 0.12, 0.30] };
            if x > w / 2 && y > h / 2 { [0.2, 0.2, 0.2] } else { a }
        };
        // R G / G B, white balance already in the data.
        let pattern = [[0, 1], [1, 2]];
        let data: Vec<f32> = (0..w * h).map(|i| truth(i % w, i / w)[pattern[(i / w) % 2][(i % w) % 2]]).collect();
        let m = BayerMosaic { data, width: w, height: h, pattern };
        let d = Demosaicer::default();
        let out = d.demosaic(&m, [1.0; 3], &|_| {}).unwrap();
        let ahd = sk_core::ahd::demosaic(&sk_core::ahd::Mosaic { data: &m.data, width: w, height: h, pattern }, [1.0; 3]);
        let error = |img: &[[f32; 3]]| -> f32 {
            let mut e = 0.0;
            for y in 4..h - 4 {
                for x in 4..w - 4 {
                    let (p, t) = (img[y * w + x], truth(x, y));
                    e += (0..3).map(|c| (p[c] - t[c]).abs()).sum::<f32>();
                }
            }
            e / ((w - 8) * (h - 8)) as f32
        };
        let (net, ahd) = (error(&out.data), error(&ahd));
        assert!(net < ahd * 1.2, "Netz {net}, AHD {ahd}");
        // Inside a flat area the colors are right.
        let p = out.data[(3 * h / 4) * w + 3 * w / 4];
        assert!(p.iter().all(|v| (v - 0.2).abs() < 0.01), "{p:?}");
    }
}
