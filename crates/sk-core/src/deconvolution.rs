//! Sharpening by deconvolution (the "Details" of Schärfen, see `Detail::sharpen_split`): takes
//! back the slight blur every lens (and the sensor's own filters) adds, instead of raising the
//! edge contrast like the unsharp mask. Richardson-Lucy with a Gaussian
//! of the radius the image shows: the sharpest edges in the full-resolution data are measured
//! once per image (`estimate`). Works on the brightness of the linear camera data, before
//! everything else; flat (noisy) areas and clipped highlights are left alone, and how far a pixel
//! may change is limited, against halos.
//!
//! The deconvolution is computed once per source and radius and blended by the amount.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use rayon::prelude::*;

use crate::image::LinearImage;

/// Radius (Gaussian sigma, full-resolution pixels) when none could be measured.
pub const DEFAULT_SIGMA: f32 = 0.8;
/// Measured radii are kept within this.
const SIGMA_RANGE: (f32, f32) = (0.5, 1.6);
/// Share of a measured edge's width that is the lens's.
const EDGE_SHARE: f32 = 0.8;
/// Below this radius in the source's own pixels (a reduced preview) there is nothing to undo.
const MIN_SIGMA: f32 = 0.35;
const ITERATIONS: usize = 8;
/// A pixel's brightness changes at most by this factor either way.
const MAX_GAIN: f32 = 1.35;

/// Measured radii by image (file identity), see [`sigma_of`].
static SIGMAS: Mutex<Option<HashMap<String, f32>>> = Mutex::new(None);

/// The radius of an image in full-resolution pixels: measured from `full` if that is the
/// full-resolution source (and remembered), else what was measured before, else the default.
pub fn sigma_of(path: &Path, src: &LinearImage, sensor_long_edge: f32) -> f32 {
    let Some(id) = crate::cache::file_identity(path) else { return DEFAULT_SIGMA };
    if let Some(s) = SIGMAS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&id)) {
        return *s;
    }
    if (src.width.max(src.height) as f32) < sensor_long_edge * 0.9 {
        return DEFAULT_SIGMA;
    }
    let s = estimate(src).unwrap_or(DEFAULT_SIGMA);
    log::info!("{}: Dekonvolution, Radius {s:.2} px", path.display());
    SIGMAS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(id, s);
    s
}

/// Brightness of camera RGB (each channel counts alike: the white balance is left out, the
/// deconvolution only needs a plane that follows the detail).
#[inline]
fn luma(p: &[f32; 3]) -> f32 {
    (p[0] + p[1] + p[2]) / 3.0
}

/// Measures the blur of the sharpest edges: along strong horizontal and vertical edges, the
/// distance over which the brightness rises from 10 to 90 % of the step, as a Gaussian's sigma
/// (2.56 sigma for that rise); the sharpest quarter tells the lens (others are soft by nature
/// or out of focus). `None` without enough clean edges.
pub fn estimate(src: &LinearImage) -> Option<f32> {
    let (w, h) = (src.width, src.height);
    if w < 64 || h < 64 {
        return None;
    }
    // Square root: steps in shadows and highlights count alike.
    let y: Vec<f32> = src.data.par_iter().map(|p| luma(p).max(0.0).sqrt()).collect();
    const HALF: usize = 5;
    // Candidates on a sparse grid of rows and columns (every 3rd), the strongest gradient each.
    let widths: Vec<f32> = (HALF + 1..h - HALF - 1)
        .into_par_iter()
        .step_by(3)
        .flat_map_iter(|r| {
            let y = &y;
            (HALF + 1..w - HALF - 1).step_by(3).filter_map(move |c| {
                let at = |x: usize, yy: usize| y[yy * w + x];
                let gx = at(c + 1, r) - at(c - 1, r);
                let gy = at(c, r + 1) - at(c, r - 1);
                // A clear step across one axis, hardly any along the other.
                let (g, horizontal) = if gx.abs() > 3.0 * gy.abs() { (gx, true) } else if gy.abs() > 3.0 * gx.abs() { (gy, false) } else { return None };
                if g.abs() < 0.08 {
                    return None;
                }
                let profile: Vec<f32> = (0..=2 * HALF).map(|i| if horizontal { at(c + i - HALF, r) } else { at(c, r + i - HALF) }).collect();
                edge_width(&profile)
            })
        })
        .collect();
    if widths.len() < 50 {
        return None;
    }
    let mut widths = widths;
    widths.sort_by(f32::total_cmp);
    let sharp = widths[widths.len() / 4];
    // Even the sharpest edges in a photo are no perfect steps: a little less than measured.
    Some((sharp / 2.563 * EDGE_SHARE).clamp(SIGMA_RANGE.0, SIGMA_RANGE.1))
}

/// The 10–90 % rise of a step in `p` (centered on it), if `p` is one: flat on both ends,
/// monotonic between, high contrast.
fn edge_width(p: &[f32]) -> Option<f32> {
    let n = p.len();
    let (a, b) = ((p[0] + p[1]) / 2.0, (p[n - 1] + p[n - 2]) / 2.0);
    let step = b - a;
    if step.abs() < 0.1 || (p[1] - p[0]).abs() > 0.1 * step.abs() || (p[n - 1] - p[n - 2]).abs() > 0.1 * step.abs() {
        return None;
    }
    // Monotonic within a little noise.
    let s = step.signum();
    if p.windows(2).any(|d| (d[1] - d[0]) * s < -0.03 * step.abs()) {
        return None;
    }
    let cross = |f: f32| -> Option<f32> {
        let t = a + step * f;
        p.windows(2).enumerate().find_map(|(i, d)| ((d[0] - t) * s <= 0.0 && (d[1] - t) * s > 0.0).then(|| i as f32 + (t - d[0]) / (d[1] - d[0])))
    };
    Some(cross(0.9)? - cross(0.1)?)
}

/// Gains (deconvolved / original brightness, masked and limited) by (source, radius).
type Key = (usize, usize, usize, u64, u32);
type Gains = Option<Arc<Vec<f32>>>;
static GAINS: Mutex<Vec<(Key, Gains)>> = Mutex::new(Vec::new());
const KEEP: usize = 2;
/// Sources with the gains applied by (source, radius, amount).
type Applied = ((Key, u32), Arc<LinearImage>);
static APPLIED: Mutex<Vec<Applied>> = Mutex::new(Vec::new());

/// The source sharpened by `amount` (0..1) with a Gaussian of `sigma` source pixels; `None`
/// if there is nothing to do.
pub fn sharpened(src: &LinearImage, sigma: f32, amount: f32) -> Option<Arc<LinearImage>> {
    if amount <= 0.0 || sigma < MIN_SIGMA || src.data.is_empty() {
        return None;
    }
    let key: Key = (src.data.as_ptr() as usize, src.width, src.height, crate::ai_products::fingerprint(src), sigma.to_bits());
    let full = (key, amount.to_bits());
    if let Some((_, img)) = APPLIED.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, _)| *k == full) {
        return Some(img.clone());
    }
    let cached = GAINS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, _)| *k == key).map(|(_, g)| g.clone());
    let gains = match cached {
        Some(g) => g,
        None => {
            let g = gains(src, sigma).map(Arc::new);
            let mut c = GAINS.lock().unwrap_or_else(|e| e.into_inner());
            c.push((key, g.clone()));
            if c.len() > KEEP {
                c.remove(0);
            }
            g
        }
    }?;
    let t = amount.clamp(0.0, 1.0);
    let data = src.data.par_iter().zip(gains.par_iter()).map(|(p, g)| p.map(|v| v * (1.0 + (g - 1.0) * t))).collect();
    let img = Arc::new(LinearImage::new(src.width, src.height, data));
    let mut a = APPLIED.lock().unwrap_or_else(|e| e.into_inner());
    a.push((full, img.clone()));
    if a.len() > KEEP {
        a.remove(0);
    }
    Some(img)
}

/// Per pixel: deconvolved brightness over the original one, 1 where it must not change.
fn gains(src: &LinearImage, sigma: f32) -> Option<Vec<f32>> {
    let (w, h) = (src.width, src.height);
    if w < 8 || h < 8 {
        return None;
    }
    let y: Vec<f32> = src.data.par_iter().map(|p| luma(p).max(1e-6)).collect();
    let kernel = gaussian(sigma);
    let mut est = y.clone();
    // The buffers are reused (allocating hundreds of MB per pass costs more than the arithmetic).
    let (mut a, mut b, mut tmp) = (vec![0.0f32; w * h], vec![0.0f32; w * h], vec![0.0f32; w * h]);
    for _ in 0..ITERATIONS {
        blur_into(&est, &mut a, &mut tmp, w, h, &kernel);
        a.par_iter_mut().zip(&y).for_each(|(v, o)| *v = o / v.max(1e-6));
        blur_into(&a, &mut b, &mut tmp, w, h, &kernel);
        est.par_iter_mut().zip(&b).for_each(|(e, r)| *e *= r);
    }
    drop((a, b, tmp));
    // Where there is detail: the local contrast of the original (smoothed, so that single noisy
    // pixels don't count) well above the noise.
    let wide = gaussian(sigma * 2.0);
    let smooth = blur(&y, w, h, &wide);
    let contrast: Vec<f32> = y.par_iter().zip(&smooth).map(|(o, s)| (o - s).abs() / s.max(1e-4)).collect();
    let contrast = blur(&contrast, w, h, &wide);
    Some(
        (0..w * h)
            .into_par_iter()
            .map(|i| {
                let detail = ((contrast[i] - 0.01) / 0.03).clamp(0.0, 1.0);
                let unclipped = 1.0 - crate::highlights::clipped(src.data[i].iter().copied().fold(0.0f32, f32::max));
                let g = (est[i] / y[i]).clamp(1.0 / MAX_GAIN, MAX_GAIN);
                1.0 + (g - 1.0) * detail * unclipped
            })
            .collect(),
    )
}

fn gaussian(sigma: f32) -> Vec<f32> {
    let r = (sigma * 3.0).ceil().max(1.0) as isize;
    let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let s: f32 = k.iter().sum();
    k.into_iter().map(|v| v / s).collect()
}

/// Separable blur of a plane with `kernel` (odd length), edges clamped: shifted rows added up,
/// which the compiler vectorizes.
fn blur(p: &[f32], w: usize, h: usize, kernel: &[f32]) -> Vec<f32> {
    let (mut out, mut tmp) = (vec![0.0f32; w * h], vec![0.0f32; w * h]);
    blur_into(p, &mut out, &mut tmp, w, h, kernel);
    out
}

/// [`blur`] into `out`, with `tmp` for the rows (both `w * h`, overwritten).
fn blur_into(p: &[f32], out: &mut [f32], tmp: &mut [f32], w: usize, h: usize, kernel: &[f32]) {
    let r = kernel.len() / 2;
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        row.fill(0.0);
        let src = &p[y * w..(y + 1) * w];
        // The row with its edge values repeated, so every tap is a shifted slice.
        let padded: Vec<f32> = (0..w + 2 * r).map(|i| src[(i as isize - r as isize).clamp(0, w as isize - 1) as usize]).collect();
        for (k, kv) in kernel.iter().enumerate() {
            for (o, v) in row.iter_mut().zip(&padded[k..k + w]) {
                *o += kv * v;
            }
        }
    });
    let tmp = &*tmp;
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        row.fill(0.0);
        for (k, kv) in kernel.iter().enumerate() {
            let sy = (y as isize + k as isize - r as isize).clamp(0, h as isize - 1) as usize;
            for (o, v) in row.iter_mut().zip(&tmp[sy * w..(sy + 1) * w]) {
                *o += kv * v;
            }
        }
    });
}

/// For tests: forgets the measured radii.
#[cfg(test)]
pub(crate) fn forget() {
    SIGMAS.lock().unwrap_or_else(|e| e.into_inner()).take();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sharp step blurred by a Gaussian of known sigma: the estimate finds about that sigma.
    fn blurred_steps(sigma: f32) -> LinearImage {
        let (w, h) = (300, 300);
        // Squares of different brightness: steps in both directions.
        let sharp: Vec<f32> = (0..w * h).map(|i| if ((i % w) / 30 + (i / w) / 30) % 2 == 0 { 0.6 } else { 0.08 }).collect();
        let b = blur(&sharp, w, h, &gaussian(sigma));
        LinearImage::new(w, h, b.into_iter().map(|v| [v; 3]).collect())
    }

    #[test]
    fn the_radius_of_the_blur_is_measured() {
        for sigma in [0.7f32, 1.0, 1.4] {
            let est = estimate(&blurred_steps(sigma)).unwrap() / EDGE_SHARE;
            assert!((est - sigma).abs() < 0.2, "{sigma}: {est}");
        }
    }

    /// Deconvolution makes a blurred edge steeper, leaves flat areas as they are.
    #[test]
    fn deconvolution_restores_edges_not_flat_areas() {
        let img = blurred_steps(1.0);
        let out = sharpened(&img, 1.0, 1.0).unwrap();
        let w = img.width;
        let row = 45 * w;
        // Across the edge at x = 30: steeper.
        let slope = |d: &[[f32; 3]]| (d[row + 31][0] - d[row + 28][0]).abs();
        assert!(slope(&out.data) > slope(&img.data) * 1.15, "{} {}", slope(&out.data), slope(&img.data));
        // Inside a square: unchanged.
        assert!((out.data[row + 15][0] - img.data[row + 15][0]).abs() < 1e-3);
        assert!(sharpened(&img, 1.0, 0.0).is_none() && sharpened(&img, 0.2, 1.0).is_none());
        forget();
    }
}
