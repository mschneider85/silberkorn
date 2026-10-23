//! Automatic sharpening, noise reduction and "Verbessern" (Raw-Details, KI-Entrauschen): from the
//! noise of the full-resolution image and the blur of its lens (see `deconvolution`).
//!
//! The noise is measured in the square root of the brightness, where the shot noise of a sensor
//! is about the same in dark and bright parts: per block of 32×32 pixels the median of a
//! Laplacian that cancels smooth gradients (Immerkær's operator), and of those the quietest
//! blocks, so that texture and edges don't count as noise.

use rayon::prelude::*;

use crate::image::LinearImage;
use crate::params::{Adjustments, Detail};
use crate::raw::RawMeta;

/// What the automatic settings go by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measured {
    /// Noise: standard deviation in the square root of the brightness (0..1), at full resolution.
    pub noise: f32,
    /// Radius of the lens's blur in full-resolution pixels (see `deconvolution::sigma_of`).
    pub sigma: f32,
}

const BLOCK: usize = 32;
/// Share of the quietest blocks the noise is taken from.
const QUIET: f32 = 0.1;

/// The noise of `full` (camera RGB of the whole image), weighted with the white balance `wb`
/// and the exposure change `exposure` (EV) of the rendering, as they make it visible.
pub fn noise(full: &LinearImage, wb: [f32; 3], exposure: f32) -> f32 {
    let (w, h) = (full.width, full.height);
    if w < BLOCK * 4 || h < BLOCK * 4 {
        return 0.0;
    }
    let min = wb.iter().copied().fold(f32::MAX, f32::min).max(1e-6);
    let k = 2f32.powf(exposure) / wb.iter().map(|v| v / min).sum::<f32>();
    let wb = wb.map(|v| v / min * k);
    let level = |x: usize, y: usize| {
        let p = full.data[y * w + x];
        (p[0] * wb[0] + p[1] * wb[1] + p[2] * wb[2]).max(0.0).sqrt()
    };
    // Every block (with its border of one pixel inside the image): median of the operator,
    // without the blocks that clip (no noise left there).
    let (bw, bh) = ((w - 2) / BLOCK, (h - 2) / BLOCK);
    let mut medians: Vec<f32> = (0..bw * bh)
        .into_par_iter()
        .filter_map(|b| {
            let (x0, y0) = (1 + (b % bw) * BLOCK, 1 + (b / bw) * BLOCK);
            let mut v = Vec::with_capacity(BLOCK * BLOCK);
            for y in y0..y0 + BLOCK {
                for x in x0..x0 + BLOCK {
                    let c = level(x, y);
                    if c >= 0.98 {
                        return None;
                    }
                    let edge = level(x - 1, y) + level(x + 1, y) + level(x, y - 1) + level(x, y + 1);
                    let corner = level(x - 1, y - 1) + level(x + 1, y - 1) + level(x - 1, y + 1) + level(x + 1, y + 1);
                    v.push((4.0 * c - 2.0 * edge + corner).abs());
                }
            }
            let mid = v.len() / 2;
            Some(*v.select_nth_unstable_by(mid, f32::total_cmp).1)
        })
        .collect();
    if medians.is_empty() {
        return 0.0;
    }
    medians.sort_by(f32::total_cmp);
    let quiet = &medians[..((medians.len() as f32 * QUIET) as usize).max(1)];
    let median = quiet.iter().sum::<f32>() / quiet.len() as f32;
    // For gaussian noise the operator has 6 sigma, and the median of its absolute value is
    // 0.6745 of that.
    median / (6.0 * 0.6745)
}

/// Measures `full`, the full-resolution image of `meta`, as `adj` renders it.
pub fn measure(meta: &RawMeta, adj: &Adjustments, full: &LinearImage) -> Measured {
    let sensor = meta.info.width.max(meta.info.height).max(1) as f32;
    Measured { noise: noise(full, meta.as_shot_wb, adj.exposure), sigma: crate::deconvolution::sigma_of(&meta.path, full, sensor) }
}

/// Noise in thousandths: about 0.7 at ISO 100, 2 at ISO 1000 and 3 at ISO 2500 on a full-frame
/// Sony; the settings below go by that scale.
fn milli(noise: f32) -> f32 {
    noise * 1000.0
}

/// Rounds to steps of `step` (without float residue such as 0.90000004).
fn steps(v: f32, step: f32) -> f32 {
    ((v / step).round() * step * 1000.0).round() / 1000.0
}

/// Schärfen: the radius after the measured blur of the lens, less amount and more threshold
/// with more noise (it would be sharpened as well), and part of it as deconvolution
/// ("Details") while the image is clean enough.
pub fn sharpen(m: &Measured, d: &mut Detail) {
    let n = milli(m.noise);
    d.sharpen_radius = steps((m.sigma * 0.75).clamp(0.5, 1.2), 0.1);
    d.sharpen_amount = steps((140.0 - (n - 1.0).max(0.0) * 20.0).clamp(60.0, 140.0), 5.0);
    // The threshold in display levels (0..255): two and a half times the noise there.
    d.sharpen_threshold = steps((m.noise * 255.0 * 2.5).clamp(1.0, 10.0), 0.5);
    d.sharpen_detail = steps((50.0 - (n - 1.0).max(0.0) * 20.0).clamp(0.0, 50.0), 5.0);
}

/// Rauschreduzierung: luminance from a little noise on, color always a little and more with
/// more noise. What KI-Entrauschen takes away already is left out.
pub fn noise_reduction(m: &Measured, d: &mut Detail) {
    let n = milli(m.noise) * (1.0 - 0.8 * (d.ai_denoise / 100.0).clamp(0.0, 1.0));
    d.nr_luma = steps(((n - 1.0) * 10.0).clamp(0.0, 50.0), 5.0);
    d.nr_color = steps((20.0 + (n - 1.0).max(0.0) * 7.5).clamp(20.0, 60.0), 5.0);
}

/// Verbessern: Raw-Details for clean Bayer images, KI-Entrauschen from clear noise on (about
/// ISO 1000; full from twice that noise), which then demosaics itself. Below, a few percent
/// wouldn't be worth its long computation. Not for merged HDR images (`hdr`); Raw-Details only
/// for `bayer` mosaics.
pub fn enhance(m: &Measured, bayer: bool, hdr: bool, d: &mut Detail) {
    if hdr {
        return;
    }
    let n = milli(m.noise);
    d.ai_denoise = steps(((n - 2.0) / 2.0 * 100.0).clamp(0.0, 100.0), 5.0);
    d.raw_details = if bayer && d.ai_denoise < 50.0 { 100.0 } else { 0.0 };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Noise of a known strength is measured as that, also on a gradient and next to texture.
    #[test]
    fn measures_added_noise() {
        let (w, h) = (640, 480);
        let mut seed = 12345u32;
        let mut rand = || {
            // Two uniform numbers to one gaussian (Box-Muller).
            let mut u = || {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed as f32 / u32::MAX as f32).max(1e-7)
            };
            let (a, b) = (u(), u());
            (-2.0 * a.ln()).sqrt() * (std::f32::consts::TAU * b).cos()
        };
        for sigma in [0.002f32, 0.01, 0.03] {
            let data = (0..w * h)
                .map(|i| {
                    let (x, y) = (i % w, i / w);
                    // A gradient on the left, stripes (texture) on the right.
                    let base: f32 = if x < w / 2 { 0.3 + 0.4 * y as f32 / h as f32 } else if (x / 3) % 2 == 0 { 0.2 } else { 0.7 };
                    let v = (base + sigma * rand()).max(0.0);
                    [v * v; 3]
                })
                .collect();
            let m = noise(&LinearImage { width: w, height: h, data }, [1.0; 3], 0.0);
            assert!((m / sigma - 1.0).abs() < 0.2, "{sigma}: {m}");
        }
    }

    /// Clean images get Raw-Details and crisp sharpening, noisy ones KI-Entrauschen, more noise
    /// reduction and gentler sharpening.
    #[test]
    fn settings_follow_the_noise() {
        let at = |noise: f32| {
            let m = Measured { noise, sigma: 1.05 };
            let mut d = Detail::default();
            sharpen(&m, &mut d);
            noise_reduction(&m, &mut d);
            d
        };
        let enhanced = |noise: f32| {
            let mut d = Detail::default();
            enhance(&Measured { noise, sigma: 1.05 }, true, false, &mut d);
            (d.raw_details, d.ai_denoise)
        };
        let (clean, noisy) = (at(0.0007), at(0.0045));
        assert_eq!((enhanced(0.0007), clean.nr_luma), ((100.0, 0.0), 0.0));
        assert_eq!(enhanced(0.0045), (0.0, 100.0));
        assert!(noisy.sharpen_amount < clean.sharpen_amount && noisy.sharpen_threshold > clean.sharpen_threshold);
        assert!(noisy.sharpen_detail < clean.sharpen_detail && noisy.nr_color > clean.nr_color);
        assert!((clean.sharpen_radius - 0.8).abs() < 1e-4);
        // With KI-Entrauschen at full, little noise is left for the classic noise reduction.
        let mut d = Detail { ai_denoise: 100.0, ..Detail::default() };
        noise_reduction(&Measured { noise: 0.0045, sigma: 1.0 }, &mut d);
        assert!(d.nr_luma < at(0.0045).nr_luma, "{}", d.nr_luma);
    }
}
