//! AI denoising with RawNIND (UtNet2 by Benoit Brummer, UCLouvain, as published for darktable;
//! weights GPL-3.0, downloaded only at the user's request). Computes the product that
//! `sk_core::ai_denoise` keeps and blends into the image:
//!
//! - Bayer sensors: the network takes the mosaic itself (four planes R, G, G, B of 2×2 cells,
//!   black level subtracted, not white balanced) and returns clean camera RGB at full resolution,
//!   so it also replaces the demosaic.
//! - Everything else (X-Trans, linear DNGs): a second network cleans demosaiced linear Rec.2020.
//!
//! Both work in overlapping tiles; the output's brightness is matched to the input per tile.

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use sk_core::color::{mat_inv, mat_vec};
use sk_core::image::LinearImage;

use crate::models::{self, DENOISE_BAYER, DENOISE_LINEAR};
use crate::runtime::{Data, Input, Net};

/// Network input edge (cells for the Bayer network, pixels for the linear one) and overlap.
const TILE: usize = 512;
const OVERLAP: usize = 64;
/// Input to output brightness of the networks, measured (the linear one even inverts the sign);
/// used only for tiles too dark to measure it.
const BAYER_GAIN: f32 = 1.0e-6;
const LINEAR_GAIN: f32 = -3.2e-10;
/// At most this many tiles at once on the processor (each needs several hundred MB).
const PARALLEL_TILES: usize = 6;

/// Identifies the models for the product cache.
pub fn tag() -> String {
    format!("rawnind-{}-{}", &DENOISE_BAYER.sha256[..12], &DENOISE_LINEAR.sha256[..12])
}

/// Both models are downloaded.
pub fn available() -> bool {
    models::available(&DENOISE_BAYER) && models::available(&DENOISE_LINEAR)
}

/// Downloads both models.
pub fn download(progress: &dyn Fn(f32)) -> Result<()> {
    for (i, m) in [DENOISE_BAYER, DENOISE_LINEAR].iter().enumerate() {
        if !models::available(m) {
            models::download(m, &|p| progress((i as f32 + p) / 2.0))?;
        }
    }
    Ok(())
}

/// Download size of both models in MB.
pub fn download_mb() -> u64 {
    // Compressed in the archive they are about 7 % smaller.
    (DENOISE_BAYER.bytes + DENOISE_LINEAR.bytes) * 93 / 100 / 1_000_000
}

/// A loaded network and the runtime configuration it was loaded with.
type Loaded = Option<(u64, Arc<Net>)>;

/// The loaded networks, kept between images (loaded again when the GPU setting changes).
#[derive(Default)]
pub struct Denoiser {
    nets: Mutex<[Loaded; 2]>,
}

impl Denoiser {
    fn net(&self, bayer: bool) -> Result<Arc<Net>> {
        let generation = crate::runtime::config_generation();
        let mut nets = self.nets.lock().unwrap();
        let slot = &mut nets[usize::from(!bayer)];
        if let Some((g, n)) = slot
            && *g == generation
        {
            return Ok(n.clone());
        }
        let (m, channels) = if bayer { (&DENOISE_BAYER, 4) } else { (&DENOISE_LINEAR, 3) };
        anyhow::ensure!(models::available(m), "{} ist nicht geladen", m.label);
        let net = Arc::new(Net::load(&models::path(m), &[Input::F32([1, channels, TILE, TILE])])?);
        *slot = Some((generation, net.clone()));
        Ok(net)
    }

    /// The product of a raw file: clean camera RGB, oriented, the size of `raw::load_full`.
    pub fn product(&self, path: &Path, progress: &dyn Fn(f32)) -> Result<LinearImage> {
        self.product_ext(path, false, progress)
    }

    /// [`Self::product`]; `linear` uses the linear network also for Bayer sensors (for comparisons).
    pub fn product_ext(&self, path: &Path, linear: bool, progress: &dyn Fn(f32)) -> Result<LinearImage> {
        let (meta, plain) = sk_core::raw::load_full(path).with_context(|| format!("{}", path.display()))?;
        anyhow::ensure!(!meta.hdr, "Zusammengesetzte HDR-Bilder werden nicht entrauscht");
        progress(0.05);
        let progress = |p: f32| progress(0.05 + 0.95 * p);
        let clean = match sk_core::raw::load_bayer(path)?.filter(|_| !linear) {
            Some((_, mosaic)) => {
                let img = self.bayer(&mosaic, &progress)?.oriented(meta.orientation);
                anyhow::ensure!((img.width, img.height) == (plain.width, plain.height), "Bildgröße passt nicht ({}×{})", img.width, img.height);
                img
            }
            None => self.linear(&meta, &plain, &progress)?,
        };
        // Pixels the network didn't produce (an odd border) and clipped highlights, which it knows
        // nothing about, stay as they were.
        let data = plain
            .data
            .par_iter()
            .zip(&clean.data)
            .map(|(p, c)| {
                if c[0].is_nan() {
                    return *p;
                }
                let keep = ((p.iter().copied().fold(0.0f32, f32::max) - 0.9) / 0.08).clamp(0.0, 1.0);
                [0, 1, 2].map(|i| (c[i].max(0.0) + (p[i] - c[i].max(0.0)) * keep).min(1.0))
            })
            .collect();
        Ok(LinearImage::new(plain.width, plain.height, data))
    }

    /// Bayer mosaic -> camera RGB of the mosaic's size (not oriented; NaN where not covered).
    fn bayer(&self, m: &sk_core::raw::BayerMosaic, progress: &dyn Fn(f32)) -> Result<LinearImage> {
        let (ox, oy) = m.rggb_offset().context("Unbekanntes Bayer-Muster")?;
        let (cw, ch) = ((m.width - ox) / 2, (m.height - oy) / 2);
        anyhow::ensure!(cw > 0 && ch > 0, "Bild zu klein");
        let planes: Vec<Vec<f32>> = [(0, 0), (1, 0), (0, 1), (1, 1)]
            .into_par_iter()
            .map(|(dx, dy)| (0..cw * ch).map(|i| m.data[(oy + 2 * (i / cw) + dy) * m.width + ox + 2 * (i % cw) + dx]).collect())
            .collect();
        let out = run_tiled(&*self.net(true)?, &planes, cw, ch, 2, [0.25, 0.5, 0.25], BAYER_GAIN, progress)?;
        let (w2, h2) = (2 * cw, 2 * ch);
        let mut data = vec![[f32::NAN; 3]; m.width * m.height];
        data.par_chunks_mut(m.width).enumerate().for_each(|(y, row)| {
            if y < oy || y - oy >= h2 {
                return;
            }
            for x in 0..w2 {
                let i = (y - oy) * w2 + x;
                row[ox + x] = [out[0][i], out[1][i], out[2][i]];
            }
        });
        Ok(LinearImage::new(m.width, m.height, data))
    }

    /// Demosaiced camera RGB -> clean camera RGB, through the network for linear Rec.2020.
    fn linear(&self, meta: &sk_core::raw::RawMeta, plain: &LinearImage, progress: &dyn Fn(f32)) -> Result<LinearImage> {
        let wb = meta.as_shot_wb;
        let m = meta.color.cam_to_working;
        let inv = mat_inv(&m).context("Farbmatrix nicht umkehrbar")?;
        let (w, h) = (plain.width, plain.height);
        let rec2020: Vec<[f32; 3]> = plain.data.par_iter().map(|p| mat_vec(&m, [p[0] * wb[0], p[1] * wb[1], p[2] * wb[2]])).collect();
        let planes: Vec<Vec<f32>> = (0..3).into_par_iter().map(|c| rec2020.iter().map(|p| p[c]).collect()).collect();
        drop(rec2020);
        let out = run_tiled(&*self.net(false)?, &planes, w, h, 1, [1.0 / 3.0; 3], LINEAR_GAIN, progress)?;
        let data = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let c = mat_vec(&inv, [out[0][i], out[1][i], out[2][i]]);
                [c[0] / wb[0], c[1] / wb[1], c[2] / wb[2]]
            })
            .collect();
        Ok(LinearImage::new(w, h, data))
    }
}

/// Tile origins along one axis: `0, step, …` with the last tile ending at the border.
fn origins(len: usize) -> Vec<usize> {
    if len <= TILE {
        return vec![0];
    }
    let step = TILE - OVERLAP;
    let mut v: Vec<usize> = (0..).map(|i| i * step).take_while(|&o| o + TILE < len).collect();
    v.push(len - TILE);
    v
}

/// Blending weight at position `i` (0..`len`) of a tile: 1 inside, falling linearly over the
/// overlap towards neighbouring tiles (not at the image border).
fn ramp(i: usize, len: usize, at_start: bool, at_end: bool) -> f32 {
    let o = (OVERLAP * len / TILE) as f32;
    let mut w = 1.0f32;
    if !at_start {
        w = w.min((i as f32 + 0.5) / o);
    }
    if !at_end {
        w = w.min((len - i) as f32 / o - 0.5 / o);
    }
    w.clamp(1e-3, 1.0)
}

/// Mirrors a position outside `0..len` back in (for images smaller than a tile).
fn mirror(i: usize, len: usize) -> usize {
    if i < len { i } else { (2 * len).saturating_sub(i + 2).min(len - 1) }
}

/// Runs a network over `planes` (`w` x `h` each) in overlapping tiles; returns three planes of
/// `scale * w` x `scale * h`. Each tile's output is scaled so that its brightness (channels
/// weighted by `weights`) matches the input's mean; `nominal` is the scale for black tiles.
#[allow(clippy::too_many_arguments)]
fn run_tiled(
    net: &Net,
    planes: &[Vec<f32>],
    w: usize,
    h: usize,
    scale: usize,
    weights: [f32; 3],
    nominal: f32,
    progress: &dyn Fn(f32),
) -> Result<[Vec<f32>; 3]> {
    let (xs, ys) = (origins(w), origins(h));
    let tiles: Vec<(usize, usize, usize, usize)> =
        ys.iter().enumerate().flat_map(|(yi, &ty)| xs.iter().enumerate().map(move |(xi, &tx)| (xi, yi, tx, ty))).collect();
    let (ow, oh, ot) = (scale * w, scale * h, scale * TILE);
    let acc = Mutex::new(([vec![0.0f32; ow * oh], vec![0.0f32; ow * oh], vec![0.0f32; ow * oh]], vec![0.0f32; ow * oh]));
    let run = |&(xi, yi, tx, ty): &(usize, usize, usize, usize)| -> Result<()> {
        let c = planes.len();
        let mut input = vec![0.0f32; c * TILE * TILE];
        for (p, plane) in planes.iter().enumerate() {
            for j in 0..TILE {
                let y = mirror(ty + j, h);
                for i in 0..TILE {
                    input[p * TILE * TILE + j * TILE + i] = plane[y * w + mirror(tx + i, w)];
                }
            }
        }
        let out = net.run(&[Data::F32(&input)])?;
        let o = out.first().context("Keine Modellausgabe")?;
        let n = ot * ot;
        anyhow::ensure!(o.data.len() == 3 * n, "Unerwartete Modellausgabe {:?}", o.shape);
        let mean_in = input.iter().sum::<f32>() / input.len() as f32;
        let mean_out = (0..3).map(|k| weights[k] * o.data[k * n..(k + 1) * n].iter().sum::<f32>() / n as f32).sum::<f32>();
        let gain = mean_in / mean_out;
        let gain = if mean_in > 1e-4 && gain.is_finite() && gain != 0.0 { gain } else { nominal };
        let mut acc = acc.lock().unwrap();
        let (sum, wsum) = &mut *acc;
        for j in 0..ot {
            let y = scale * ty + j;
            if y >= oh {
                break;
            }
            let wy = ramp(j, ot, yi == 0, yi + 1 == ys.len());
            for i in 0..ot {
                let x = scale * tx + i;
                if x >= ow {
                    break;
                }
                let wt = wy * ramp(i, ot, xi == 0, xi + 1 == xs.len());
                for (k, plane) in sum.iter_mut().enumerate() {
                    plane[y * ow + x] += o.data[k * n + j * ot + i] * gain * wt;
                }
                wsum[y * ow + x] += wt;
            }
        }
        Ok(())
    };
    // Progress (and with it cancelling) is reported from this thread, after each round of tiles.
    if net.prefers_serial() {
        for (i, t) in tiles.iter().enumerate() {
            run(t)?;
            progress((i + 1) as f32 / tiles.len() as f32);
        }
    } else {
        let threads = PARALLEL_TILES.min(rayon::current_num_threads()).max(1);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build()?;
        let mut done = 0;
        for round in tiles.chunks(threads) {
            pool.install(|| round.par_iter().try_for_each(run))?;
            done += round.len();
            progress(done as f32 / tiles.len() as f32);
        }
    }
    let (mut sum, wsum) = acc.into_inner().unwrap();
    for plane in &mut sum {
        plane.par_iter_mut().zip(&wsum).for_each(|(v, s)| *v /= s.max(1e-6));
    }
    if sum.iter().any(|p| p.iter().any(|v| !v.is_finite())) {
        bail!("Das Modell lieferte ungültige Werte");
    }
    Ok(sum)
}

/// Registers RawNIND as the provider of `sk_core::ai_denoise` (whether or not it is downloaded:
/// computing fails with a message until it is).
pub fn register() {
    let denoiser = Denoiser::default();
    sk_core::ai_denoise::set_provider(&tag(), Box::new(move |path, progress| denoiser.product(path, progress)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_cover_the_image_and_blend_to_one() {
        assert_eq!(origins(100), vec![0]);
        assert_eq!(origins(TILE), vec![0]);
        let o = origins(3000);
        assert_eq!(*o.last().unwrap(), 3000 - TILE);
        assert!(o.windows(2).all(|w| w[1] - w[0] <= TILE - OVERLAP));
        // Inside a tile the weight is 1; at a shared edge both tiles contribute.
        assert_eq!(ramp(TILE / 2, TILE, false, false), 1.0);
        assert!(ramp(0, TILE, false, false) < 0.1 && ramp(0, TILE, true, false) == 1.0);
        // Scaled tiles (the Bayer network doubles the size) fall over a doubled overlap.
        assert_eq!(ramp(2 * OVERLAP, 2 * TILE, false, false), 1.0);
        assert!(ramp(OVERLAP, 2 * TILE, false, false) < 0.6);
        assert_eq!((mirror(5, 10), mirror(10, 10), mirror(11, 10)), (5, 8, 7));
    }

    /// With the downloaded model: a noisy flat mosaic comes out flat, at the same brightness.
    #[test]
    fn denoises_a_noisy_mosaic_if_the_model_is_available() {
        if !models::available(&DENOISE_BAYER) {
            return;
        }
        let d = Denoiser::default();
        let (w, h) = (600, 520);
        let data: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let base = match (x % 2, y % 2) {
                    (0, 0) => 0.10,
                    (1, 1) => 0.07,
                    _ => 0.20,
                };
                base + sk_core::detail::hash_noise(x as i64, y as i64, 3) * 0.03
            })
            .collect();
        let m = sk_core::raw::BayerMosaic { data, width: w, height: h, pattern: [[0, 1], [1, 2]] };
        let out = d.bayer(&m, &|_| {}).unwrap();
        let n = out.data.len() as f32;
        let mean: [f32; 3] = std::array::from_fn(|c| out.data.iter().map(|p| p[c]).sum::<f32>() / n);
        let dev = (out.data.iter().map(|p| (p[1] - mean[1]).powi(2)).sum::<f32>() / n).sqrt();
        assert!((mean[0] - 0.10).abs() < 0.01 && (mean[1] - 0.20).abs() < 0.015 && (mean[2] - 0.07).abs() < 0.01, "{mean:?}");
        assert!(dev < 0.03 * 0.5, "{dev}");
    }
}
