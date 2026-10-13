//! HDR merge: an exposure bracket becomes one linear DNG with the dynamic range of all
//! exposures.
//!
//! 1. The relative exposure of each image is measured from the pixels both it and the
//!    reference (the middle exposure) show well; the EXIF exposure is the fallback.
//! 2. Handheld brackets are aligned by translation (median threshold bitmaps, which don't
//!    depend on the exposure).
//! 3. Per pixel the radiance estimates are averaged, weighted by how well exposed each image is
//!    there and by its exposure (longer exposures have less noise). Clipped values don't count.
//! 4. Deghosting: where an image disagrees with the reference (something moved), it is left out.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::detail::{Plane, gaussian_blur};
use crate::image::LinearImage;
use crate::raw::{self, RawMeta};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Deghost {
    Off,
    Low,
    #[default]
    Medium,
    High,
}

impl Deghost {
    pub const ALL: [Deghost; 4] = [Deghost::Off, Deghost::Low, Deghost::Medium, Deghost::High];

    pub fn label(self) -> &'static str {
        match self {
            Deghost::Off => "Aus",
            Deghost::Low => "Niedrig",
            Deghost::Medium => "Mittel",
            Deghost::High => "Hoch",
        }
    }

    /// Disagreement with the reference (EV) above which an image counts as showing something else.
    fn threshold(self) -> Option<f32> {
        match self {
            Deghost::Off => None,
            Deghost::Low => Some(1.0),
            Deghost::Medium => Some(0.6),
            Deghost::High => Some(0.35),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HdrOptions {
    pub align: bool,
    pub deghost: Deghost,
}

impl Default for HdrOptions {
    fn default() -> Self {
        Self { align: true, deghost: Deghost::Medium }
    }
}

/// Above this a raw value counts as clipped.
const CLIP: f32 = 0.92;
/// Resolution reduction for measuring exposure ratios and ghosts.
const SMALL: usize = 4;

/// Scene brightness factor from EXIF (exposure time × ISO / f-number²), if known.
fn exif_exposure(info: &crate::raw::CaptureInfo) -> Option<f64> {
    let (n, d) = info.exposure_rational?;
    let t = n as f64 / d as f64;
    let iso = info.iso.filter(|i| *i > 0).unwrap_or(100) as f64;
    let f = info.fnumber.filter(|f| *f > 0.0).unwrap_or(1.0) as f64;
    // A damaged exposure time (zero denominator or numerator) is no exposure at all.
    Some(t * iso / (f * f)).filter(|e| e.is_finite() && *e > 0.0)
}

#[inline]
fn max3(p: [f32; 3]) -> f32 {
    p[0].max(p[1]).max(p[2])
}

#[inline]
fn luma(p: [f32; 3]) -> f32 {
    0.3 * p[0] + 0.55 * p[1] + 0.15 * p[2]
}

/// Weight of a raw value for the merge: low near black (noise), zero when clipped.
#[inline]
fn well_exposed(m: f32) -> f32 {
    let dark = (m / 0.02).min(1.0);
    let bright = ((CLIP - m) / 0.12).clamp(0.0, 1.0);
    dark * bright * bright
}

/// Exposure of `img` relative to `reference` (both small): the median ratio over pixels that
/// both show well. `None` if they have too little in common.
pub fn exposure_ratio(img: &LinearImage, reference: &LinearImage) -> Option<f32> {
    let mut ratios: Vec<f32> = img
        .data
        .iter()
        .zip(&reference.data)
        .filter_map(|(a, b)| {
            let (ma, mb) = (max3(*a), max3(*b));
            (ma > 0.01 && ma < 0.85 && mb > 0.01 && mb < 0.85).then(|| luma(*a) / luma(*b).max(1e-6))
        })
        .collect();
    if ratios.len() < 200 {
        return None;
    }
    let mid = ratios.len() / 2;
    Some(*ratios.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1)
}

// ------------------------------------------------------------------ alignment (MTB)

/// Gamma-encoded luminance, which makes the median threshold meaningful.
fn gray(img: &LinearImage) -> Plane {
    Plane::new(img.width, img.height, img.data.par_iter().map(|p| luma(*p).max(0.0).powf(1.0 / 2.2)).collect())
}

fn half(p: &Plane) -> Plane {
    let (w, h) = (p.w / 2, p.h / 2);
    let data = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w * 2, i / w * 2);
            (p.data[y * p.w + x] + p.data[y * p.w + x + 1] + p.data[(y + 1) * p.w + x] + p.data[(y + 1) * p.w + x + 1]) * 0.25
        })
        .collect();
    Plane::new(w, h, data)
}

/// Threshold bitmap and exclusion bitmap (values too close to the median are ignored).
fn bitmaps(p: &Plane) -> (Vec<bool>, Vec<bool>) {
    let mut v: Vec<f32> = p.data.iter().step_by(3).copied().collect();
    let mid = v.len() / 2;
    let median = *v.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1;
    let tol = 4.0 / 255.0;
    (p.data.iter().map(|x| *x > median).collect(), p.data.iter().map(|x| (*x - median).abs() > tol).collect())
}

fn mtb_error(a: &(Vec<bool>, Vec<bool>), b: &(Vec<bool>, Vec<bool>), w: usize, h: usize, dx: i32, dy: i32) -> u64 {
    let mut err = 0u64;
    let margin = 2;
    for y in margin..h.saturating_sub(margin) {
        let sy = y as i32 + dy;
        if sy < 0 || sy >= h as i32 {
            continue;
        }
        for x in margin..w.saturating_sub(margin) {
            let sx = x as i32 + dx;
            if sx < 0 || sx >= w as i32 {
                continue;
            }
            let (i, j) = (y * w + x, sy as usize * w + sx as usize);
            if a.1[i] && b.1[j] && a.0[i] != b.0[j] {
                err += 1;
            }
        }
    }
    err
}

/// Translation (in pixels of `img`) that aligns `img` to `reference`: sample `img` at
/// `(x + dx, y + dy)` for the reference pixel `(x, y)`.
pub fn align_translation(img: &LinearImage, reference: &LinearImage, max_levels: usize) -> (i32, i32) {
    let mut pa = vec![gray(reference)];
    let mut pb = vec![gray(img)];
    while pa.len() < max_levels && pa.last().unwrap().w.min(pa.last().unwrap().h) > 128 {
        let (a, b) = (half(pa.last().unwrap()), half(pb.last().unwrap()));
        pa.push(a);
        pb.push(b);
    }
    let (mut dx, mut dy) = (0i32, 0i32);
    for level in (0..pa.len()).rev() {
        dx *= 2;
        dy *= 2;
        let (a, b) = (&pa[level], &pb[level]);
        let (ba, bb) = (bitmaps(a), bitmaps(b));
        let mut best = (u64::MAX, dx, dy);
        for oy in -1..=1 {
            for ox in -1..=1 {
                let e = mtb_error(&ba, &bb, a.w, a.h, dx + ox, dy + oy);
                if e < best.0 {
                    best = (e, dx + ox, dy + oy);
                }
            }
        }
        (dx, dy) = (best.1, best.2);
    }
    (dx, dy)
}

/// Maximum over a square neighborhood.
fn dilate(p: &Plane, r: usize) -> Plane {
    let (w, h) = (p.w, p.h);
    let rows: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            (x.saturating_sub(r)..(x + r + 1).min(w)).map(|xx| p.data[y * w + xx]).fold(0.0, f32::max)
        })
        .collect();
    let data = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            (y.saturating_sub(r)..(y + r + 1).min(h)).map(|yy| rows[yy * w + x]).fold(0.0, f32::max)
        })
        .collect();
    Plane::new(w, h, data)
}

#[inline]
fn sample(img: &LinearImage, x: i32, y: i32) -> [f32; 3] {
    let x = x.clamp(0, img.width as i32 - 1) as usize;
    let y = y.clamp(0, img.height as i32 - 1) as usize;
    img.data[y * img.width + x]
}

fn shifted(img: &LinearImage, dx: i32, dy: i32) -> LinearImage {
    if dx == 0 && dy == 0 {
        return img.clone();
    }
    let w = img.width;
    let data = (0..img.data.len()).into_par_iter().map(|i| sample(img, (i % w) as i32 + dx, (i / w) as i32 + dy)).collect();
    LinearImage::new(img.width, img.height, data)
}

// ------------------------------------------------------------------ merge

/// One image of the bracket, aligned to the reference, with its exposure relative to it.
struct Exposure {
    img: LinearImage,
    k: f32,
    /// Weight multiplier (1 - ghost) at `SMALL` resolution, if deghosting.
    keep: Option<Plane>,
}

/// Merges the images (already loaded, oriented, same size). The first returned value is the
/// merged camera RGB at the exposure of `images[reference]`.
fn merge(images: Vec<(LinearImage, f32)>, reference: usize, opts: &HdrOptions, progress: &dyn Fn(f32)) -> LinearImage {
    let n = images.len();
    let (w, h) = (images[reference].0.width, images[reference].0.height);
    let ref_small = images[reference].0.box_downscaled(SMALL);
    let mut exposures: Vec<Exposure> = Vec::new();
    for (i, (img, k)) in images.into_iter().enumerate() {
        let keep = (i != reference).then(|| opts.deghost.threshold()).flatten().map(|thr| {
            let small = img.box_downscaled(SMALL);
            // Disagreement in EV where both show the scene well.
            let ghost: Vec<f32> = small
                .data
                .par_iter()
                .zip(&ref_small.data)
                .map(|(a, b)| {
                    let (ma, mb) = (max3(*a), max3(*b));
                    if mb > CLIP || ma > CLIP || ma < 0.003 {
                        return 0.0;
                    }
                    let ea = luma(*a) / k;
                    let eb = luma(*b);
                    let d = ((ea + 1e-4) / (eb + 1e-4)).log2().abs();
                    ((d - thr) / (thr * 0.5)).clamp(0.0, 1.0)
                })
                .collect();
            // Grown and softened, so that moving things go out completely and without edges.
            let g = dilate(&Plane::new(small.width, small.height, ghost), 3);
            gaussian_blur(&g, 1.5).map(|v| 1.0 - (v * 1.5).clamp(0.0, 1.0))
        });
        exposures.push(Exposure { img, k, keep });
        progress(0.5 + 0.2 * (i + 1) as f32 / n as f32);
    }
    let darkest = (0..n).min_by(|&a, &b| exposures[a].k.total_cmp(&exposures[b].k)).unwrap_or(0);
    let (sw, sh) = (ref_small.width.max(1), ref_small.height.max(1));
    let keep_at = |p: &Plane, x: usize, y: usize| -> f32 {
        let fx = ((x as f32 + 0.5) / SMALL as f32 - 0.5).clamp(0.0, (sw - 1) as f32);
        let fy = ((y as f32 + 0.5) / SMALL as f32 - 0.5).clamp(0.0, (sh - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(p.w - 1), (y0 + 1).min(p.h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let d = &p.data;
        let top = d[y0 * p.w + x0] * (1.0 - tx) + d[y0 * p.w + x1] * tx;
        let bot = d[y1 * p.w + x0] * (1.0 - tx) + d[y1 * p.w + x1] * tx;
        top * (1.0 - ty) + bot * ty
    };
    let mut out = vec![[0.0f32; 3]; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, line)| {
        for (x, o) in line.iter_mut().enumerate() {
            let i = y * w + x;
            let mut acc = [0.0f32; 3];
            let mut wsum = 0.0f32;
            for e in &exposures {
                let p = e.img.data[i];
                let mut wt = well_exposed(max3(p)) * e.k;
                if let Some(keep) = &e.keep {
                    wt *= keep_at(keep, x, y);
                }
                if wt > 0.0 {
                    let inv = wt / e.k;
                    acc = [acc[0] + p[0] * inv, acc[1] + p[1] * inv, acc[2] + p[2] * inv];
                    wsum += wt;
                }
            }
            *o = if wsum > 1e-9 {
                acc.map(|v| v / wsum)
            } else {
                // Clipped everywhere (or black): the darkest exposure is the best guess.
                let e = &exposures[darkest];
                e.img.data[i].map(|v| v / e.k)
            };
        }
    });
    LinearImage::new(w, h, out)
}

/// A free file name `<stem><suffix>.dng` next to `first` (`-2`, `-3` … if taken).
pub fn output_path(first: &Path, suffix: &str) -> PathBuf {
    let dir = first.parent().unwrap_or(Path::new("."));
    let stem = crate::variant::file_of(first).file_stem().unwrap_or_default().to_string_lossy().to_string();
    let candidate = |n: u32| if n < 2 { dir.join(format!("{stem}{suffix}.dng")) } else { dir.join(format!("{stem}{suffix}-{n}.dng")) };
    (1..).map(candidate).find(|p| !p.exists()).expect("unbounded")
}

/// Merges an exposure bracket into `<first image>-HDR.dng`. `progress` gets 0..1.
pub fn merge_files(paths: &[PathBuf], opts: &HdrOptions, progress: &dyn Fn(f32)) -> Result<PathBuf> {
    let files = crate::variant::files_of(paths);
    if files.len() < 2 {
        bail!("Für HDR mindestens zwei Belichtungen auswählen");
    }
    // Reference: the middle exposure (by EXIF; else the middle of the selection).
    let metas: Vec<RawMeta> = files.iter().map(|p| raw::read_meta(p)).collect::<Result<_>>()?;
    let mut order: Vec<usize> = (0..files.len()).collect();
    if metas.iter().all(|m| exif_exposure(&m.info).is_some()) {
        order.sort_by(|&a, &b| exif_exposure(&metas[a].info).unwrap().total_cmp(&exif_exposure(&metas[b].info).unwrap()));
    }
    let reference = order[order.len() / 2];
    progress(0.02);
    let (ref_meta, ref_img) = raw::load_full(&files[reference]).with_context(|| format!("{}", files[reference].display()))?;
    let ref_small = ref_img.box_downscaled(SMALL);
    let total = files.len() as f32;
    let mut images: Vec<(LinearImage, f32)> = Vec::new();
    let mut ref_pos = 0;
    for (i, path) in files.iter().enumerate() {
        if i == reference {
            ref_pos = images.len();
            images.push((ref_img.clone(), 1.0));
            continue;
        }
        let (meta, img) = raw::load_full(path).with_context(|| format!("{}", path.display()))?;
        if (img.width, img.height) != (ref_img.width, ref_img.height) {
            bail!("{} hat eine andere Bildgröße als die übrigen Belichtungen", path.display());
        }
        let small = img.box_downscaled(SMALL);
        let (dx, dy) = if opts.align { align_translation(&small, &ref_small, 6) } else { (0, 0) };
        let (dx, dy) = (dx * SMALL as i32, dy * SMALL as i32);
        let img = shifted(&img, dx, dy);
        let small = shifted(&small, dx / SMALL as i32, dy / SMALL as i32);
        let exif = exif_exposure(&meta.info).zip(exif_exposure(&ref_meta.info)).map(|(a, b)| (a / b) as f32);
        let usable = |k: &f32| k.is_finite() && *k > 0.0;
        let k = exposure_ratio(&small, &ref_small).filter(usable).or(exif.filter(usable)).unwrap_or(1.0).max(1e-4);
        images.push((img, k));
        progress(0.05 + 0.45 * (i + 1) as f32 / total);
    }
    drop(ref_img);
    let merged = merge(images, ref_pos, opts, progress);
    progress(0.75);
    let out = output_path(&files[0], "-HDR");
    let preview = crate::dng::quick_preview(&ref_meta, &merged, 1024);
    crate::dng::write_linear_dng(
        &out,
        &crate::dng::LinearDng {
            width: merged.width,
            height: merged.height,
            data: &merged.data,
            xyz_to_cam: ref_meta.color.xyz_to_cam,
            as_shot_wb: ref_meta.as_shot_wb,
            info: &ref_meta.info,
            baseline_exposure: 0.0,
            preview: Some(&preview),
        },
    )?;
    progress(1.0);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic scene (radiance up to 16) as a camera would record it at exposure `k`, shifted.
    fn shot(scene: &[f32], w: usize, h: usize, k: f32, dx: i32, dy: i32) -> LinearImage {
        let data = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as i32 + dx, (i / w) as i32 + dy);
                let (x, y) = (x.clamp(0, w as i32 - 1) as usize, y.clamp(0, h as i32 - 1) as usize);
                let v = (scene[y * w + x] * k).min(1.0);
                [v, v * 0.9, v * 0.8]
            })
            .collect();
        LinearImage::new(w, h, data)
    }

    fn scene(w: usize, h: usize) -> Vec<f32> {
        (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                // Gradient over 10 EV plus (non-periodic) texture for alignment.
                let base = 2f32.powf(x as f32 / w as f32 * 10.0 - 6.0);
                let tex = 1.0 + 0.4 * crate::detail::value_noise(x as f32 / 6.0, y as f32 / 6.0, 3);
                base * tex
            })
            .collect()
    }

    #[test]
    fn damaged_exif_gives_no_exposure() {
        let mut info = crate::raw::CaptureInfo { exposure_rational: Some((1, 100)), iso: Some(200), ..Default::default() };
        assert!(exif_exposure(&info).is_some_and(|e| e > 0.0));
        // ISO 0 counts as unknown (like a missing value), an exposure time of 0 as no exposure.
        info.iso = Some(0);
        assert_eq!(exif_exposure(&info), exif_exposure(&crate::raw::CaptureInfo { iso: None, ..info.clone() }));
        info.exposure_rational = Some((0, 100));
        assert_eq!(exif_exposure(&info), None);
    }

    #[test]
    fn ratio_and_alignment() {
        let (w, h) = (320, 240);
        // Structure in both directions (the gradient scene only has it across).
        let s: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let n = crate::detail::value_noise(x / 9.0, y / 9.0, 5) + 0.5 * crate::detail::value_noise(x / 4.0, y / 4.0, 6);
                0.15 * 2f32.powf(2.0 * n)
            })
            .collect();
        let a = shot(&s, w, h, 1.0, 0, 0);
        let b = shot(&s, w, h, 4.0, 3, -2);
        let (dx, dy) = align_translation(&b, &a, 5);
        // b shows the scene shifted by (3, -2): b(x - 3, y + 2) = a(x, y).
        assert_eq!((dx, dy), (-3, 2));
        // Measured on the aligned images, as when merging.
        let k = exposure_ratio(&shifted(&b, dx, dy), &a).unwrap();
        assert!((k - 4.0).abs() < 0.2, "{k}");
    }

    #[test]
    fn merge_recovers_highlights_and_shadows() {
        let (w, h) = (256, 64);
        let s = scene(w, h);
        let shots = vec![(shot(&s, w, h, 0.25, 0, 0), 0.25), (shot(&s, w, h, 1.0, 0, 0), 1.0), (shot(&s, w, h, 4.0, 0, 0), 4.0)];
        let merged = merge(shots, 1, &HdrOptions { align: false, deghost: Deghost::Off }, &|_| {});
        for x in [5usize, 60, 128, 200, 250] {
            let i = 30 * w + x;
            let want = s[i];
            let got = merged.data[i][0];
            // Beyond the darkest exposure's clipping nothing can be recovered.
            if want * 0.25 < CLIP {
                assert!((got - want).abs() <= want * 0.05 + 2e-4, "x={x}: {got} vs {want}");
            }
        }
        assert!(merged.data.iter().any(|p| p[0] > 2.0), "Lichter über dem Weißpunkt der Referenz");
    }

    #[test]
    fn deghosting_ignores_moving_objects() {
        let (w, h) = (128, 128);
        let s: Vec<f32> = vec![0.3; w * h];
        let a = shot(&s, w, h, 1.0, 0, 0);
        let mut b = shot(&s, w, h, 2.0, 0, 0);
        // Something dark passes through the second exposure.
        for y in 50..70 {
            for x in 50..70 {
                b.data[y * w + x] = [0.01; 3];
            }
        }
        let merged = merge(vec![(a.clone(), 1.0), (b.clone(), 2.0)], 0, &HdrOptions { align: false, deghost: Deghost::Medium }, &|_| {});
        let c = merged.data[60 * w + 60][0];
        assert!((c - 0.3).abs() < 0.02, "{c}");
        let ghosted = merge(vec![(a, 1.0), (b, 2.0)], 0, &HdrOptions { align: false, deghost: Deghost::Off }, &|_| {});
        assert!(ghosted.data[60 * w + 60][0] < 0.2);
    }
}
