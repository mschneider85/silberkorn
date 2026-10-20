//! Measuring lateral chromatic aberration (the "Chromatische Aberration" of the lens correction):
//! the lens images red and blue slightly larger or smaller than green, so towards the edges of
//! the image their edges lie beside the green ones. At many contrasty edges across the radius,
//! the offset of red and blue against green is found; it grows with the distance from the
//! center, and its slope is the scale the channel needs.
//!
//! Measured on the image with its current lens correction (the camera's or a profile's), so the
//! result is what is still left; with no fault left it stays near zero.

use rayon::prelude::*;

use crate::geometry::{GeoMap, Region};
use crate::image::LinearImage;
use crate::params::{Geometry, LensCorrection};

/// One step of the "Rot/Cyan" and "Blau/Gelb" sliders as a scale of the channel (see
/// `GeoMap::new`).
pub const SCALE_PER_STEP: f32 = 0.003 / 100.0;

/// Points needed for a result.
const MIN_POINTS: usize = 40;

/// The sliders ("Rot/Cyan", "Blau/Gelb") that leave no lateral chromatic aberration in `src`
/// (the oriented camera image, preview size) with the lens correction `lens`; None without
/// enough edges to tell.
pub fn measure(src: &LinearImage, lens: &LensCorrection, camera: Option<&crate::camera_lens::CameraLens>) -> Option<(f32, f32)> {
    let geo = GeoMap::new(&Geometry::default(), lens, camera, 0, src.width, src.height, true);
    let (w, h) = (geo.out_w, geo.out_h);
    let img = geo.resample(src, Region { x: 0, y: 0, w, h });
    let [red, blue] = residual_scale(&img)?;
    let step = |v: f32, e: f32| (v + e / SCALE_PER_STEP).round().clamp(-100.0, 100.0) + 0.0;
    Some((step(lens.ca_red, red), step(lens.ca_blue, blue)))
}

/// How much larger red and blue are drawn than green (relative, 0 = the same).
pub fn residual_scale(img: &LinearImage) -> Option<[f32; 2]> {
    let (w, h) = (img.width, img.height);
    if w < 64 || h < 64 {
        return None;
    }
    // Perceptual scale, so that dark and light edges count alike.
    let plane = |c: usize| img.data.par_iter().map(|p| p[c].max(0.0).sqrt()).collect::<Vec<f32>>();
    let (r, g, b) = (plane(0), plane(1), plane(2));
    let at = |p: &[f32], x: f32, y: f32| {
        let x = x.clamp(0.0, (w - 1) as f32);
        let y = y.clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (x as usize, y as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = (x - x0 as f32, y - y0 as f32);
        let top = p[y0 * w + x0] + (p[y0 * w + x1] - p[y0 * w + x0]) * tx;
        let bot = p[y1 * w + x0] + (p[y1 * w + x1] - p[y1 * w + x0]) * tx;
        top + (bot - top) * ty
    };
    let (cx, cy) = (w as f32 / 2.0 - 0.5, h as f32 / 2.0 - 0.5);
    let half_diag = ((w * w + h * h) as f32).sqrt() / 2.0;
    // Lighter than this is clipped or nearly so: its edges don't show the lens.
    let bright = 0.97f32.sqrt();

    // Edges across the radius, on a coarse grid (one per cell at most).
    let cell = 6;
    let points: Vec<(f32, f32, f32)> = (1..h / cell - 1)
        .into_par_iter()
        .flat_map_iter(|cy_i| {
            let (g, r, b) = (&g, &r, &b);
            (1..w / cell - 1).filter_map(move |cx_i| {
                // The strongest green edge of the cell.
                let mut best = (0.0f32, 0usize, 0usize, 0.0f32, 0.0f32);
                for y in cy_i * cell..(cy_i + 1) * cell {
                    for x in cx_i * cell..(cx_i + 1) * cell {
                        let gx = g[y * w + x + 1] - g[y * w + x - 1];
                        let gy = g[(y + 1) * w + x] - g[(y - 1) * w + x];
                        let m = gx * gx + gy * gy;
                        if m > best.0 {
                            best = (m, x, y, gx, gy);
                        }
                    }
                }
                let (m, x, y, gx, gy) = best;
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                let radius = (dx * dx + dy * dy).sqrt();
                // Strong, across the radius, not too near the center (the offset grows outward).
                if m < 0.004 || radius < 0.3 * half_diag {
                    return None;
                }
                let (ux, uy) = (dx / radius, dy / radius);
                let along = (gx * ux + gy * uy).abs() / m.sqrt();
                if along < 0.8 {
                    return None;
                }
                let i = y * w + x;
                if [r[i], g[i], b[i]].iter().any(|v| *v > bright) {
                    return None;
                }
                let (x, y) = (x as f32, y as f32);
                let profile = |p: &[f32], shift: f32| -> [f32; 13] { std::array::from_fn(|k| at(p, x + ux * (k as f32 - 6.0 + shift), y + uy * (k as f32 - 6.0 + shift))) };
                let green = profile(g, 0.0);
                let red = best_shift(&green, |s| profile(r, s))?;
                let blue = best_shift(&green, |s| profile(b, s))?;
                Some((radius, red, blue))
            })
        })
        .collect();
    if points.len() < MIN_POINTS {
        return None;
    }
    // Offset = scale × radius: the median of the ratios ignores the odd edge that is colored.
    let slope = |pick: fn(&(f32, f32, f32)) -> f32| {
        let mut v: Vec<f32> = points.iter().map(|p| pick(p) / p.0).collect();
        let mid = v.len() / 2;
        *v.select_nth_unstable_by(mid, f32::total_cmp).1
    };
    Some([slope(|p| p.1), slope(|p| p.2)])
}

/// Shift (pixels outward, -3..3) at which `channel` matches `green` best, if it matches well.
fn best_shift(green: &[f32; 13], channel: impl Fn(f32) -> [f32; 13]) -> Option<f32> {
    let mut best = (f32::MIN, 0.0);
    for i in -60..=60 {
        let s = i as f32 * 0.05;
        let c = correlation(&green[2..11], &channel(s)[2..11]);
        if c > best.0 {
            best = (c, s);
        }
    }
    // Shapes that differ (a colored edge, not a neutral one) say nothing about the lens.
    (best.0 > 0.95 && best.1.abs() < 2.9).then_some(best.1)
}

/// Normalized cross-correlation of two profiles.
fn correlation(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len() as f32;
    let (ma, mb) = (a.iter().sum::<f32>() / n, b.iter().sum::<f32>() / n);
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (x - ma, y - mb);
        ab += x * y;
        aa += x * x;
        bb += y * y;
    }
    if aa < 1e-8 || bb < 1e-8 { -1.0 } else { ab / (aa * bb).sqrt() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Grey squares on a light ground, all over the image.
    fn checker(w: usize, h: usize) -> LinearImage {
        LinearImage::new(
            w,
            h,
            (0..w * h)
                .map(|i| {
                    let (x, y) = (i % w, i / w);
                    let v = if (x / 37 + y / 29) % 2 == 0 { 0.08 } else { 0.6 };
                    [v; 3]
                })
                .collect(),
        )
    }

    #[test]
    fn finds_the_aberration_it_is_given() {
        let clean = checker(1200, 800);
        assert_eq!(measure(&clean, &LensCorrection::default(), None), Some((0.0, 0.0)));
        // Red drawn larger, blue smaller: the sliders that undo it are the opposite.
        let lens = LensCorrection { ca_red: 60.0, ca_blue: -40.0, ..Default::default() };
        let geo = GeoMap::new(&Geometry::default(), &lens, None, 0, 1200, 800, true);
        let faulty = geo.resample(&clean, Region { x: 0, y: 0, w: geo.out_w, h: geo.out_h });
        let (red, blue) = measure(&faulty, &LensCorrection::default(), None).expect("Kanten");
        assert!((red + 60.0).abs() <= 8.0, "{red}");
        assert!((blue - 40.0).abs() <= 8.0, "{blue}");
        // Measured with that correction set, nothing is left to change.
        let undo = LensCorrection { ca_red: red, ca_blue: blue, ..Default::default() };
        let (r2, b2) = measure(&faulty, &undo, None).unwrap();
        assert!((r2 - red).abs() <= 6.0 && (b2 - blue).abs() <= 6.0, "{r2} {b2}");
    }

    #[test]
    fn a_flat_image_tells_nothing() {
        let flat = LinearImage::new(400, 300, vec![[0.3; 3]; 400 * 300]);
        assert_eq!(measure(&flat, &LensCorrection::default(), None), None);
    }
}
