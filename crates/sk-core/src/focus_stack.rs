//! Focus stacking: a series focused at different distances becomes one linear DNG that is sharp
//! throughout.
//!
//! 1. The middle image of the series is the reference. Every other image is aligned to it by a
//!    homography (corners and RANSAC as for panoramas), which also covers the slight change of
//!    magnification when the focus moves ("focus breathing") and small handheld shifts.
//! 2. Per pixel each image counts by how sharp it is there: the local energy of the Laplacian of
//!    its (gamma-encoded) luminance, smoothed so that the choice does not follow noise. The
//!    weights are a high power of it, so the sharpest image dominates, while flat areas (sky,
//!    out of focus everywhere) average all images and lose noise.
//! 3. The images are added one after another; only the sums are kept, not the whole series.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use rayon::prelude::*;

use crate::detail::{Plane, gaussian_blur};
use crate::image::LinearImage;
use crate::panorama::{FEATURE_EDGE, Features, M3, detect, guide, match_features, project_h, ransac};
use crate::raw::{self, RawMeta};

/// Weights are the sharpness to this power (the sharpness is already squared contrast).
const POWER: i32 = 2;
/// Largest shift of a corner of the image by the alignment, relative to the long edge; beyond
/// it the alignment counts as failed.
const MAX_WARP: f64 = 0.08;
/// Softening of the images corners are found in (pixels of the feature image).
const FEATURE_BLUR: f32 = 2.0;

/// Accumulates aligned images weighted by their sharpness.
pub struct Stacker {
    width: usize,
    height: usize,
    /// Corners of the reference (in its feature image) and that image's size.
    reference: Features,
    guide_size: (usize, usize),
    sum: Vec<[f32; 3]>,
    weights: Vec<f32>,
    /// Images that could not be aligned (added unaligned).
    pub unaligned: usize,
}

impl Stacker {
    /// Starts with the reference image.
    pub fn new(reference: &LinearImage) -> Self {
        let (reference_features, guide_size) = features(reference);
        let mut s = Self {
            width: reference.width,
            height: reference.height,
            guide_size,
            reference: reference_features,
            sum: vec![[0.0; 3]; reference.width * reference.height],
            weights: vec![0.0; reference.width * reference.height],
            unaligned: 0,
        };
        s.accumulate(reference, None);
        s
    }

    /// Adds an image of the series (same size as the reference).
    pub fn add(&mut self, img: &LinearImage) -> Result<()> {
        if (img.width, img.height) != (self.width, self.height) {
            bail!("andere Bildgröße als die Referenz");
        }
        let h = self.alignment(img);
        if h.is_none() {
            self.unaligned += 1;
        }
        let warped = h.map(|h| self.warp(img, &h));
        let valid = warped.as_ref().map(|(_, v)| v.as_slice());
        self.accumulate(warped.as_ref().map_or(img, |(w, _)| w), valid);
        Ok(())
    }

    /// The homography from reference to `img` in the feature images (centered coordinates).
    fn alignment(&self, img: &LinearImage) -> Option<M3> {
        let (feats, size) = features(img);
        if size != self.guide_size {
            return None;
        }
        let m = match_features(&self.reference, &feats);
        let src: Vec<[f64; 2]> = m.iter().map(|&(a, _)| self.reference.pts[a].map(|v| v as f64)).collect();
        let dst: Vec<[f64; 2]> = m.iter().map(|&(_, b)| feats.pts[b].map(|v| v as f64)).collect();
        let (_, inliers) = ransac(&src, &dst, 2.0)?;
        if inliers.len() < 12 {
            return None;
        }
        // Moving the focus changes the scale, a hand shifts and turns the camera a little: a
        // similarity fitted to all matches is far steadier than the homography of a few corners.
        let mut inliers = inliers;
        let mut h = similarity(&inliers.iter().map(|&k| (src[k], dst[k])).collect::<Vec<_>>())?;
        for _ in 0..2 {
            inliers = (0..src.len()).filter(|&k| project_h(&h, src[k]).is_some_and(|q| (q[0] - dst[k][0]).hypot(q[1] - dst[k][1]) < 1.5)).collect();
            if inliers.len() < 12 {
                return None;
            }
            h = similarity(&inliers.iter().map(|&k| (src[k], dst[k])).collect::<Vec<_>>())?;
        }
        // Plausible: the corners move only a little (no mirror, no wild perspective).
        let (hw, hh) = (size.0 as f64 / 2.0, size.1 as f64 / 2.0);
        let limit = MAX_WARP * size.0.max(size.1) as f64;
        let ok = [[-hw, -hh], [hw, -hh], [-hw, hh], [hw, hh]]
            .iter()
            .all(|c| project_h(&h, *c).is_some_and(|q| (q[0] - c[0]).hypot(q[1] - c[1]) < limit));
        ok.then_some(h)
    }

    /// `img` resampled into the reference's frame by `h`, and where it has data.
    fn warp(&self, img: &LinearImage, h: &M3) -> (LinearImage, Vec<bool>) {
        let (w, ht) = (self.width, self.height);
        let (gw, gh) = (self.guide_size.0 as f64, self.guide_size.1 as f64);
        let (fx, fy) = (w as f64 / gw, ht as f64 / gh);
        let (data, valid): (Vec<[f32; 3]>, Vec<bool>) = (0..w * ht)
            .into_par_iter()
            .map(|i| {
                let (x, y) = ((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
                let Some(q) = project_h(h, [x / fx - gw / 2.0, y / fy - gh / 2.0]) else { return ([0.0; 3], false) };
                let (sx, sy) = ((q[0] + gw / 2.0) * fx - 0.5, (q[1] + gh / 2.0) * fy - 0.5);
                if sx < -0.5 || sy < -0.5 || sx > w as f64 - 0.5 || sy > ht as f64 - 0.5 {
                    return ([0.0; 3], false);
                }
                (bilinear(img, sx as f32, sy as f32), true)
            })
            .unzip();
        (LinearImage::new(w, ht, data), valid)
    }

    fn accumulate(&mut self, img: &LinearImage, valid: Option<&[bool]>) {
        let weights = sharpness(img);
        self.sum.par_iter_mut().zip(self.weights.par_iter_mut()).enumerate().for_each(|(i, (s, ws))| {
            if valid.is_some_and(|v| !v[i]) {
                return;
            }
            let wt = (weights.data[i] + 1e-7).powi(POWER);
            let p = img.data[i];
            for c in 0..3 {
                s[c] += p[c] * wt;
            }
            *ws += wt;
        });
    }

    pub fn finish(self) -> LinearImage {
        let data = self.sum.into_par_iter().zip(self.weights).map(|(s, w)| s.map(|v| v / w.max(1e-30))).collect();
        LinearImage::new(self.width, self.height, data)
    }
}

/// Least-squares similarity (scale, rotation, shift) mapping the first points to the second.
fn similarity(pairs: &[([f64; 2], [f64; 2])]) -> Option<M3> {
    let n = pairs.len() as f64;
    let mean = |pts: &mut dyn Iterator<Item = [f64; 2]>| {
        let s = pts.fold([0.0; 2], |a, p| [a[0] + p[0], a[1] + p[1]]);
        [s[0] / n, s[1] / n]
    };
    let (ms, md) = (mean(&mut pairs.iter().map(|p| p.0)), mean(&mut pairs.iter().map(|p| p.1)));
    let (mut a, mut b, mut d) = (0.0, 0.0, 0.0);
    for (p, q) in pairs {
        let (px, py, qx, qy) = (p[0] - ms[0], p[1] - ms[1], q[0] - md[0], q[1] - md[1]);
        a += px * qx + py * qy;
        b += px * qy - py * qx;
        d += px * px + py * py;
    }
    if d < 1e-9 {
        return None;
    }
    let (a, b) = (a / d, b / d);
    Some([[a, -b, md[0] - (a * ms[0] - b * ms[1])], [b, a, md[1] - (b * ms[0] + a * ms[1])], [0.0, 0.0, 1.0]])
}

/// Corners of an image for the alignment, and the size of the image they were found in. That
/// image is softened, so that a part in focus and the same part out of focus look more alike.
fn features(img: &LinearImage) -> (Features, (usize, usize)) {
    let g = gaussian_blur(&guide(&img.fitted(FEATURE_EDGE)), FEATURE_BLUR);
    (detect(&g), (g.w, g.h))
}

fn bilinear(img: &LinearImage, x: f32, y: f32) -> [f32; 3] {
    let x = x.clamp(0.0, (img.width - 1) as f32);
    let y = y.clamp(0.0, (img.height - 1) as f32);
    let (x0, y0) = (x as usize, y as usize);
    let (x1, y1) = ((x0 + 1).min(img.width - 1), (y0 + 1).min(img.height - 1));
    let (tx, ty) = (x - x0 as f32, y - y0 as f32);
    let at = |xx: usize, yy: usize| img.data[yy * img.width + xx];
    std::array::from_fn(|c| {
        let top = at(x0, y0)[c] + (at(x1, y0)[c] - at(x0, y0)[c]) * tx;
        let bot = at(x0, y1)[c] + (at(x1, y1)[c] - at(x0, y1)[c]) * tx;
        top + (bot - top) * ty
    })
}

/// Local sharpness: smoothed squared Laplacian of the gamma-encoded luminance.
fn sharpness(img: &LinearImage) -> Plane {
    let (w, h) = (img.width, img.height);
    let l: Vec<f32> = img.data.par_iter().map(|p| ((p[0] + 2.0 * p[1] + p[2]) * 0.25).max(0.0).powf(1.0 / 2.2)).collect();
    let lap: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let at = |xx: usize, yy: usize| l[yy * w + xx];
            let n = at(x, y.saturating_sub(1)) + at(x, (y + 1).min(h - 1)) + at(x.saturating_sub(1), y) + at((x + 1).min(w - 1), y);
            let d = 4.0 * l[i] - n;
            d * d
        })
        .collect();
    gaussian_blur(&Plane::new(w, h, lap), (w.max(h) as f32 / 600.0).max(2.0))
}

/// Stacks the images into `<first image>-Fokus.dng`. `progress` gets 0..1.
pub fn stack_files(paths: &[PathBuf], progress: &dyn Fn(f32)) -> Result<PathBuf> {
    let files = crate::variant::files_of(paths);
    if files.len() < 2 {
        bail!("Für Focus Stacking mindestens zwei Bilder auswählen");
    }
    // The middle of the series: the others differ least from it.
    let reference = files.len() / 2;
    let load = |path: &PathBuf| -> Result<(RawMeta, LinearImage)> { raw::load_full(path).with_context(|| format!("{}", path.display())) };
    let (ref_meta, ref_img) = load(&files[reference])?;
    let mut stacker = Stacker::new(&ref_img);
    drop(ref_img);
    let total = files.len() as f32;
    progress(1.0 / total * 0.9);
    for (k, path) in files.iter().enumerate().filter(|(k, _)| *k != reference) {
        let (_, img) = load(path)?;
        stacker.add(&img).with_context(|| format!("{}", path.display()))?;
        progress((k as f32 + 2.0).min(total) / total * 0.9);
    }
    if stacker.unaligned > 0 {
        log::warn!("Focus Stacking: {} Bilder ließen sich nicht ausrichten", stacker.unaligned);
    }
    let stacked = stacker.finish();
    let out = crate::hdr::output_path(&files[0], "-Fokus");
    let preview = crate::dng::quick_preview(&ref_meta, &stacked, 1024);
    crate::dng::write_linear_dng(
        &out,
        &crate::dng::LinearDng {
            width: stacked.width,
            height: stacked.height,
            data: &stacked.data,
            color: &ref_meta.color,
            as_shot_wb: ref_meta.as_shot_wb,
            info: &ref_meta.info,
            baseline_exposure: 0.0,
            preview: Some(&preview),
            lens_corrected: false,
        },
    )?;
    progress(1.0);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detail::value_noise;

    /// A textured scene, seen through a lens scaled by `zoom` and shifted by `shift` pixels.
    fn scene(w: usize, h: usize, zoom: f32, shift: [f32; 2]) -> LinearImage {
        let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
        LinearImage::new(
            w,
            h,
            (0..w * h)
                .map(|i| {
                    let (x, y) = (((i % w) as f32 - cx) / zoom + cx + shift[0], ((i / w) as f32 - cy) / zoom + cy + shift[1]);
                    let n = value_noise(x / 5.0, y / 5.0, 11) + 0.6 * value_noise(x / 2.0, y / 2.0, 12) + 0.5 * value_noise(x / 13.0, y / 13.0, 13);
                    let v = 0.2 * 2f32.powf(1.5 * n);
                    [v, v * 0.9, v * 0.8]
                })
                .collect(),
        )
    }

    /// `img` blurred where `blur` is true (out of focus there).
    fn defocused(img: &LinearImage, blur: impl Fn(usize, usize) -> bool) -> LinearImage {
        let soft: Vec<Plane> = (0..3).map(|c| gaussian_blur(&Plane::new(img.width, img.height, img.data.iter().map(|p| p[c]).collect()), 3.0)).collect();
        let mut out = img.clone();
        for (i, p) in out.data.iter_mut().enumerate() {
            if blur(i % img.width, i / img.width) {
                *p = [soft[0].data[i], soft[1].data[i], soft[2].data[i]];
            }
        }
        out
    }

    /// Mean absolute difference to `want` in a rectangle.
    fn error(img: &LinearImage, want: &LinearImage, xs: std::ops::Range<usize>, ys: std::ops::Range<usize>) -> f32 {
        let mut sum = 0.0;
        let mut n = 0;
        for y in ys {
            for x in xs.clone() {
                let i = y * img.width + x;
                sum += (img.data[i][1] - want.data[i][1]).abs();
                n += 1;
            }
        }
        sum / n as f32
    }

    #[test]
    fn sharp_parts_of_each_image_are_combined() {
        let (w, h) = (400, 300);
        let sharp = scene(w, h, 1.0, [0.0, 0.0]);
        let near = defocused(&sharp, |x, _| x >= w / 2);
        let far = defocused(&sharp, |x, _| x < w / 2);
        let mut s = Stacker::new(&near);
        s.add(&far).unwrap();
        let out = s.finish();
        for (xs, name) in [(30..170, "links"), (230..370, "rechts")] {
            let (e_out, e_blur) = (error(&out, &sharp, xs.clone(), 30..270), error(&near, &sharp, 230..370, 30..270));
            assert!(e_out < e_blur * 0.3, "{name}: Fehler {e_out}, unscharf {e_blur}");
        }
    }

    #[test]
    fn breathing_and_shift_are_aligned() {
        let (w, h) = (480, 320);
        // The reference sees the scene as it is; the other image is magnified by 2 % and shifted,
        // and sharp only where the reference is not.
        let reference = defocused(&scene(w, h, 1.0, [0.0, 0.0]), |x, _| x >= w / 2);
        let other = defocused(&scene(w, h, 1.02, [4.0, -3.0]), |x, _| x < w / 2);
        let mut s = Stacker::new(&reference);
        s.add(&other).unwrap();
        assert_eq!(s.unaligned, 0);
        let out = s.finish();
        let sharp = scene(w, h, 1.0, [0.0, 0.0]);
        let (e_out, e_blur) = (error(&out, &sharp, 280..420, 40..280), error(&reference, &sharp, 280..420, 40..280));
        assert!(e_out < e_blur * 0.5, "Fehler {e_out}, unscharf {e_blur}");
    }
}
