//! AI denoising: a neural network trained on real camera noise
//! (NAFNet, SIDD) cleans the demosaiced raw data; the result is a new linear DNG with the
//! settings of the original, so that it is developed like the original, only without noise.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};
use rayon::prelude::*;
use sk_core::color::{srgb_decode, srgb_encode};
use sk_core::image::LinearImage;

/// Network input size and overlap between neighbouring tiles.
const TILE: usize = 256;
const OVERLAP: usize = 16;

pub struct Denoiser {
    net: crate::runtime::Net,
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

/// Blending weight across a tile: 1 inside, falling linearly over the overlap towards
/// neighbouring tiles (not at the image border).
fn ramp(i: usize, at_start: bool, at_end: bool) -> f32 {
    let r = (i as f32 + 0.5) / OVERLAP as f32;
    let e = (TILE - i) as f32 / OVERLAP as f32 - 0.5 / OVERLAP as f32;
    let mut w = 1.0f32;
    if !at_start {
        w = w.min(r);
    }
    if !at_end {
        w = w.min(e);
    }
    w.clamp(1e-3, 1.0)
}

impl Denoiser {
    pub fn load(path: &Path) -> Result<Self> {
        Ok(Self { net: crate::runtime::Net::load(path, &[crate::runtime::Input::F32([1, 3, TILE, TILE])])? })
    }

    fn run_tile(&self, tile: &[[f32; 3]]) -> Result<Vec<[f32; 3]>> {
        let n = TILE * TILE;
        let mut input = vec![0.0f32; 3 * n];
        for (i, p) in tile.iter().enumerate() {
            for c in 0..3 {
                input[c * n + i] = p[c];
            }
        }
        let out = self.net.run(&[crate::runtime::Data::F32(&input)])?;
        let o = out.first().context("Keine Modellausgabe")?;
        let d = &o.data;
        anyhow::ensure!(d.len() == 3 * n, "Unerwartete Modellausgabe {:?}", o.shape);
        Ok((0..n).map(|i| [d[i], d[n + i], d[2 * n + i]].map(|x| x.clamp(0.0, 1.0))).collect())
    }

    /// Denoises display-referred RGB (0..1, sRGB-like encoding), in overlapping tiles on all cores.
    pub fn run(&self, w: usize, h: usize, rgb: &[[f32; 3]], progress: &dyn Fn(f32)) -> Result<Vec<[f32; 3]>> {
        let (xs, ys) = (origins(w), origins(h));
        let total = xs.len() * ys.len();
        let done = AtomicUsize::new(0);
        let mut acc = vec![[0.0f32; 3]; w * h];
        let mut wsum = vec![0.0f32; w * h];
        for (yi, &ty) in ys.iter().enumerate() {
            // With ONNX Runtime one tile after the other (it uses the card or all cores itself),
            // with tract a tile per core.
            let tile_of = |&tx: &usize| -> Result<(usize, Vec<[f32; 3]>)> {
                    // Mirrored at the border if the image is smaller than a tile.
                    let tile: Vec<[f32; 3]> = (0..TILE * TILE)
                        .map(|i| {
                            let (x, y) = (tx + i % TILE, ty + i / TILE);
                            let x = if x < w { x } else { (2 * w).saturating_sub(x + 2).min(w - 1) };
                            let y = if y < h { y } else { (2 * h).saturating_sub(y + 2).min(h - 1) };
                            rgb[y * w + x]
                        })
                        .collect();
                    let out = self.run_tile(&tile);
                    done.fetch_add(1, Ordering::Relaxed);
                    out.map(|o| (tx, o))
                };
            let row: Vec<(usize, Vec<[f32; 3]>)> =
                if self.net.prefers_serial() { xs.iter().map(tile_of).collect::<Result<_>>()? } else { xs.par_iter().map(tile_of).collect::<Result<_>>()? };
            for (xi, (tx, out)) in row.into_iter().enumerate() {
                for j in 0..TILE {
                    let y = ty + j;
                    if y >= h {
                        break;
                    }
                    let wy = ramp(j, yi == 0, yi + 1 == ys.len());
                    for i in 0..TILE {
                        let x = tx + i;
                        if x >= w {
                            break;
                        }
                        let wt = wy * ramp(i, xi == 0, xi + 1 == xs.len());
                        let o = out[j * TILE + i];
                        let a = &mut acc[y * w + x];
                        *a = [a[0] + o[0] * wt, a[1] + o[1] * wt, a[2] + o[2] * wt];
                        wsum[y * w + x] += wt;
                    }
                }
            }
            progress(done.load(Ordering::Relaxed) as f32 / total as f32);
        }
        Ok(acc.iter().zip(&wsum).map(|(a, s)| a.map(|v| v / s.max(1e-6))).collect())
    }
}

/// Camera data as the network expects it: white balanced, brought to a normal brightness and
/// encoded; returns the encoded image and the brightness factor.
fn to_network(img: &LinearImage, wb: [f32; 3]) -> (Vec<[f32; 3]>, f32) {
    let mut lum: Vec<f32> = img.data.iter().step_by(31).map(|p| 0.25 * p[0] * wb[0] + 0.5 * p[1] * wb[1] + 0.25 * p[2] * wb[2]).collect();
    let mid = lum.len() / 2;
    let median = if lum.is_empty() { 0.18 } else { *lum.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1 };
    let scale = (0.18 / median.max(1e-5)).clamp(0.5, 16.0);
    let enc = img.data.par_iter().map(|p| [0, 1, 2].map(|c| srgb_encode((p[c] * wb[c] * scale).clamp(0.0, 1.0)))).collect();
    (enc, scale)
}

/// Denoises a raw file into `<name>-Entrauscht.dng` next to it. `strength` 0..1 blends with the
/// original. The new file gets the original's settings (without its noise reduction).
pub fn denoise_file(path: &Path, strength: f32, model: &Denoiser, progress: &dyn Fn(f32)) -> Result<PathBuf> {
    let (meta, img) = sk_core::raw::load_full(path).with_context(|| format!("{}", path.display()))?;
    progress(0.03);
    let wb = meta.as_shot_wb;
    let min_wb = wb.iter().copied().fold(f32::MAX, f32::min).max(1e-6);
    let wb = wb.map(|v| v / min_wb);
    let (enc, scale) = to_network(&img, wb);
    let out = model.run(img.width, img.height, &enc, &|p| progress(0.03 + 0.9 * p))?;
    let strength = strength.clamp(0.0, 1.0);
    let data: Vec<[f32; 3]> = img
        .data
        .par_iter()
        .zip(&out)
        .zip(&enc)
        .map(|((orig, den), e)| {
            // Clipped highlights stay as they were (the network only knows 0..1).
            let clip = e.iter().copied().fold(0.0f32, f32::max);
            let keep = ((clip - 0.92) / 0.06).clamp(0.0, 1.0);
            let t = strength * (1.0 - keep);
            [0, 1, 2].map(|c| {
                let d = srgb_decode(den[c]) / scale / wb[c];
                (orig[c] + (d - orig[c]) * t).clamp(0.0, 1.0)
            })
        })
        .collect();
    let out_path = sk_core::hdr::output_path(path, "-Entrauscht");
    let denoised = LinearImage::new(img.width, img.height, data);
    let preview = sk_core::dng::quick_preview(&meta, &denoised, 1024);
    sk_core::dng::write_linear_dng(
        &out_path,
        &sk_core::dng::LinearDng {
            width: denoised.width,
            height: denoised.height,
            data: &denoised.data,
            xyz_to_cam: meta.color.xyz_to_cam,
            as_shot_wb: meta.as_shot_wb,
            info: &meta.info,
            baseline_exposure: 0.0,
            preview: Some(&preview),
        },
    )?;
    // Developed like the original.
    let mut sc = sk_core::sidecar::load(path);
    sc.adjustments.detail.nr_luma = 0.0;
    sc.adjustments.detail.nr_color = 0.0;
    sc.variants.clear();
    sc.snapshots.clear();
    sc.stack = None;
    sc.xmp_mtime = None;
    sk_core::sidecar::save(&out_path, &sc)?;
    progress(1.0);
    Ok(out_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_cover_the_image_and_blend_to_one() {
        assert_eq!(origins(100), vec![0]);
        assert_eq!(origins(256), vec![0]);
        let o = origins(1000);
        assert_eq!(*o.last().unwrap(), 1000 - TILE);
        assert!(o.windows(2).all(|w| w[1] - w[0] <= TILE - OVERLAP));
        // Inside a tile the weight is 1; at a shared edge both tiles contribute.
        assert_eq!(ramp(TILE / 2, false, false), 1.0);
        assert!(ramp(0, false, false) < 0.1 && ramp(0, true, false) == 1.0);
    }

    #[test]
    fn denoises_with_model_if_available() {
        let path = crate::models::path(&crate::models::DENOISE);
        if !path.exists() {
            return;
        }
        let model = Denoiser::load(&path).unwrap();
        // A flat grey patch with noise.
        let (w, h) = (300, 280);
        let noisy: Vec<[f32; 3]> = (0..w * h).map(|i| {
            let n = sk_core::detail::hash_noise((i % w) as i64, (i / w) as i64, 1) * 0.08;
            [0.5 + n; 3]
        }).collect();
        let out = model.run(w, h, &noisy, &|_| {}).unwrap();
        let dev = |d: &[[f32; 3]]| (d.iter().map(|p| (p[1] - 0.5).powi(2)).sum::<f32>() / d.len() as f32).sqrt();
        assert!(dev(&out) < dev(&noisy) * 0.5, "{} vs {}", dev(&out), dev(&noisy));
    }
}
