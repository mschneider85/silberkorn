//! Focus mask: marks the areas of an image that are in focus.
//!
//! Sharpness is the local energy of the finest details (the image minus a slightly blurred
//! copy): defocus removes these frequencies first, whatever the subject. Flat areas with a
//! little noise stay below the thresholds.

use rayon::prelude::*;

use crate::detail::{Plane, box_blur, gaussian_blur};

/// Mean fine-detail amplitude (0..1 luminance) that counts as fully sharp, in a rendering
/// whose long edge is [`REFERENCE_EDGE`] pixels.
const DETAIL_FULL: f32 = 0.12;

/// Long edge of the renderings [`DETAIL_FULL`] is tuned for (the app's previews).
pub const REFERENCE_EDGE: f32 = 2048.0;

/// Sharpness 0..255 per pixel of a displayed (sRGB-encoded) luminance image, values 0..1, of a
/// rendering at about [`REFERENCE_EDGE`] pixels. Show pixels at or above a threshold from
/// [`threshold`].
pub fn sharpness(luma: &[f32], width: usize, height: usize) -> Vec<u8> {
    sharpness_at(luma, width, height, 1.0)
}

/// Like [`sharpness`], for a rendering (or a part of one) at `resolution` times the reference
/// size. Pixel-level detail is stronger there (noise, sharpening), so more of it is needed to
/// count as sharp; this keeps the marked areas about the same when zooming in.
pub fn sharpness_at(luma: &[f32], width: usize, height: usize, resolution: f32) -> Vec<u8> {
    let full = DETAIL_FULL * resolution.max(1.0).powf(0.3);
    if width < 3 || height < 3 {
        return vec![0; width * height];
    }
    let l = Plane::new(width, height, luma.to_vec());
    let detail = l.zip(&gaussian_blur(&l, 1.0), |a, b| (a - b).abs());
    let energy = box_blur(&detail, 1);
    // Spread over 3x3 so that thin sharp lines stay visible on screen.
    let e = &energy.data;
    let mut out = vec![0u8; width * height];
    out.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut m = 0.0f32;
            for yy in y.saturating_sub(1)..(y + 2).min(height) {
                for xx in x.saturating_sub(1)..(x + 2).min(width) {
                    m = m.max(e[yy * width + xx]);
                }
            }
            *o = ((m / full).min(1.0) * 255.0).round() as u8;
        }
    });
    out
}

/// Minimum sharpness shown for a sensitivity of 0..100 (higher shows more).
pub fn threshold(sensitivity: f32) -> u8 {
    let s = (sensitivity / 100.0).clamp(0.0, 1.0);
    (245.0 - s * 170.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same checkerboard twice, the right half blurred.
    fn half_blurred(w: usize, h: usize) -> Vec<f32> {
        let check: Vec<f32> = (0..w * h).map(|i| if ((i % w) / 4 + (i / w) / 4).is_multiple_of(2) { 0.3 } else { 0.7 }).collect();
        let blurred = gaussian_blur(&Plane::new(w, h, check.clone()), 1.5);
        (0..w * h).map(|i| if i % w < w / 2 { check[i] } else { blurred.data[i] }).collect()
    }

    #[test]
    fn sharp_half_is_marked_blurred_half_is_not() {
        let (w, h) = (64, 32);
        let s = sharpness(&half_blurred(w, h), w, h);
        let t = threshold(50.0);
        let share = |x0: usize, x1: usize| {
            let n = (4..h - 4).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| s[y * w + x] >= t).count();
            n as f32 / ((x1 - x0) * (h - 8)) as f32
        };
        assert!(share(4, w / 2 - 4) > 0.5, "scharf: {}", share(4, w / 2 - 4));
        assert!(share(w / 2 + 4, w - 4) < 0.05, "unscharf: {}", share(w / 2 + 4, w - 4));
    }

    #[test]
    fn flat_noise_is_not_marked() {
        let (w, h) = (48, 48);
        let noise: Vec<f32> = (0..w * h).map(|i| 0.5 + crate::detail::hash_noise((i % w) as i64, (i / w) as i64, 7) * 0.008).collect();
        let s = sharpness(&noise, w, h);
        assert!(s.iter().all(|v| *v < threshold(100.0)));
    }

    #[test]
    fn higher_resolution_needs_more_detail() {
        let (w, h) = (64, 32);
        let img = half_blurred(w, h);
        let sum = |s: Vec<u8>| s.iter().map(|v| *v as u32).sum::<u32>();
        assert!(sum(sharpness_at(&img, w, h, 3.0)) < sum(sharpness(&img, w, h)));
        assert_eq!(sharpness_at(&img, w, h, 0.5), sharpness(&img, w, h));
    }
}
