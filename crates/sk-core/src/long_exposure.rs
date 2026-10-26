//! Long exposure from a series: many short shots become one linear DNG.
//!
//! - **Mittelwert:** the average, as one long exposure would record it (water and clouds smooth,
//!   noise lower), without an ND filter.
//! - **Median:** per pixel the shot in the middle by brightness: what passes through (people,
//!   cars) disappears, what stays remains.
//! - **Hellste:** per pixel the brightest shot: light trails, star trails, fireworks.
//!
//! The middle shot of the series is the reference. Others are aligned to it (as for focus
//! stacking) and brought to its exposure if the camera changed it. Mean and brightest keep only
//! running results; the median needs all shots at once, stored with 16 bits per channel, and uses
//! at most as many shots as fit into [`MEDIAN_BYTES`].

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::focus_stack::Aligner;
use crate::image::LinearImage;
use crate::raw::{self, RawMeta};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Blend {
    #[default]
    Mean,
    Median,
    Brightest,
}

impl Blend {
    pub const ALL: [Blend; 3] = [Blend::Mean, Blend::Median, Blend::Brightest];

    pub fn label(self) -> &'static str {
        match self {
            Blend::Mean => "Mittelwert",
            Blend::Median => "Median",
            Blend::Brightest => "Hellste",
        }
    }

    pub fn note(self) -> &'static str {
        match self {
            Blend::Mean => "Wie eine lange Belichtung: Wasser und Wolken werden weich, das Rauschen sinkt.",
            Blend::Median => "Was sich bewegt (Passanten, Autos), verschwindet; was bleibt, bleibt.",
            Blend::Brightest => "Lichtspuren, Sternspuren, Feuerwerk. Vom Stativ ohne Ausrichten, sonst richten sich die Sterne aus.",
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Blend::Mean => "-Langzeit",
            Blend::Median => "-Median",
            Blend::Brightest => "-Lichtspuren",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LongExposureOptions {
    pub blend: Blend,
    pub align: bool,
}

impl Default for LongExposureOptions {
    fn default() -> Self {
        Self { blend: Blend::Mean, align: true }
    }
}

/// Memory for the shots of a median (16 bits per channel).
pub const MEDIAN_BYTES: usize = 2 << 30;
/// Exposure differences below this (natural log, about 1/14 EV) are left alone.
const SAME_EXPOSURE: f32 = 0.05;
/// Linear values up to this are kept by the 16-bit encoding of the median.
const MEDIAN_RANGE: f32 = 4.0;
/// Marks a pixel without data (outside an aligned shot).
const NO_DATA: u16 = u16::MAX;

fn luma(p: [f32; 3]) -> f32 {
    0.25 * p[0] + 0.5 * p[1] + 0.25 * p[2]
}

/// Square root encoding: fine steps in the shadows.
fn encode(v: f32) -> u16 {
    ((v.max(0.0) / MEDIAN_RANGE).sqrt().min(1.0) * (NO_DATA - 1) as f32).round() as u16
}

fn decode(e: u16) -> f32 {
    let t = e as f32 / (NO_DATA - 1) as f32;
    t * t * MEDIAN_RANGE
}

/// Combines the shots of a series, one after another.
pub struct Combiner {
    blend: Blend,
    width: usize,
    height: usize,
    /// Mean: sums and counts. Brightest: the brightest color and its luma (as count).
    sum: Vec<[f32; 3]>,
    count: Vec<f32>,
    /// Median: the encoded shots.
    shots: Vec<Vec<[u16; 3]>>,
}

impl Combiner {
    pub fn new(blend: Blend, width: usize, height: usize) -> Self {
        let running = blend != Blend::Median;
        let n = if running { width * height } else { 0 };
        let start = if blend == Blend::Brightest { -1.0 } else { 0.0 };
        Self { blend, width, height, sum: vec![[0.0; 3]; n], count: vec![start; n], shots: Vec::new() }
    }

    /// Adds a shot (in the reference's frame); `valid` marks where it has data.
    pub fn add(&mut self, img: &LinearImage, valid: Option<&[bool]>) {
        debug_assert_eq!((img.width, img.height), (self.width, self.height));
        let has = |i: usize| valid.is_none_or(|v| v[i]);
        match self.blend {
            Blend::Mean => self.sum.par_iter_mut().zip(self.count.par_iter_mut()).enumerate().for_each(|(i, (s, n))| {
                if has(i) {
                    let p = img.data[i];
                    *s = [s[0] + p[0], s[1] + p[1], s[2] + p[2]];
                    *n += 1.0;
                }
            }),
            Blend::Brightest => self.sum.par_iter_mut().zip(self.count.par_iter_mut()).enumerate().for_each(|(i, (s, best))| {
                let l = luma(img.data[i]);
                if has(i) && l > *best {
                    *s = img.data[i];
                    *best = l;
                }
            }),
            Blend::Median => {
                let shot = img.data.par_iter().enumerate().map(|(i, p)| if has(i) { p.map(encode) } else { [NO_DATA; 3] }).collect();
                self.shots.push(shot);
            }
        }
    }

    pub fn finish(self) -> LinearImage {
        let (w, h) = (self.width, self.height);
        let data = match self.blend {
            Blend::Mean => self.sum.into_par_iter().zip(self.count).map(|(s, n)| s.map(|v| v / n.max(1.0))).collect(),
            Blend::Brightest => self.sum,
            Blend::Median => {
                let shots = self.shots;
                (0..w * h)
                    .into_par_iter()
                    .map_init(Vec::new, |vals: &mut Vec<(f32, [f32; 3])>, i| {
                        vals.clear();
                        vals.extend(shots.iter().map(|s| s[i]).filter(|e| e[0] != NO_DATA).map(|e| {
                            let p = e.map(decode);
                            (luma(p), p)
                        }));
                        if vals.is_empty() {
                            return [0.0; 3];
                        }
                        // The shot in the middle by brightness: its color stays whole.
                        let mid = vals.len() / 2;
                        vals.select_nth_unstable_by(mid, |a, b| a.0.total_cmp(&b.0)).1.1
                    })
                    .collect()
            }
        };
        LinearImage::new(w, h, data)
    }
}

/// The shots to use for a median of `n` shots of `pixels` pixels: all, or evenly spread ones
/// that fit into [`MEDIAN_BYTES`].
pub fn median_shots(n: usize, pixels: usize) -> Vec<usize> {
    let fit = (MEDIAN_BYTES / (pixels * 6).max(1)).max(3);
    if n <= fit {
        return (0..n).collect();
    }
    (0..fit).map(|k| (k * (n - 1) + (fit - 1) / 2) / (fit - 1)).collect()
}

/// Combines the series into `<first image>-Langzeit.dng` (`-Median`, `-Lichtspuren`).
/// `progress` gets 0..1.
pub fn combine_files(paths: &[PathBuf], opts: &LongExposureOptions, progress: &dyn Fn(f32)) -> Result<PathBuf> {
    let files = crate::variant::files_of(paths);
    if files.len() < 2 {
        bail!("Für eine Langzeitbelichtung mindestens zwei Bilder auswählen");
    }
    let load = |path: &PathBuf| -> Result<(RawMeta, LinearImage)> { raw::load_full(path).with_context(|| format!("{}", path.display())) };
    let reference = files.len() / 2;
    let (ref_meta, ref_img) = load(&files[reference])?;
    let (w, h) = (ref_img.width, ref_img.height);
    let mut others: Vec<usize> = if opts.blend == Blend::Median { median_shots(files.len(), w * h) } else { (0..files.len()).collect() };
    if others.len() < files.len() {
        log::info!("Median aus {} von {} Aufnahmen (Speicher)", others.len(), files.len());
    }
    others.retain(|&k| k != reference);
    let ref_small = ref_img.box_downscaled(4);
    let aligner = opts.align.then(|| Aligner::new(&ref_img));
    let mut combiner = Combiner::new(opts.blend, w, h);
    combiner.add(&ref_img, None);
    drop(ref_img);
    let total = others.len() as f32 + 1.0;
    let mut unaligned = 0;
    for (done, &k) in others.iter().enumerate() {
        let path = &files[k];
        let (_, mut img) = load(path)?;
        if (img.width, img.height) != (w, h) {
            bail!("{} hat eine andere Bildgröße als die übrigen Aufnahmen", path.display());
        }
        // A camera on automatic may have exposed the shots differently.
        if let Some(r) = crate::hdr::exposure_ratio(&img.box_downscaled(4), &ref_small).filter(|r| r.is_finite() && r.ln().abs() > SAME_EXPOSURE) {
            img.data.par_iter_mut().for_each(|p| *p = p.map(|v| v / r));
        }
        match aligner.as_ref().map(|a| a.align(&img)) {
            Some(Some((warped, valid))) => combiner.add(&warped, Some(&valid)),
            Some(None) => {
                unaligned += 1;
                combiner.add(&img, None);
            }
            None => combiner.add(&img, None),
        }
        progress((done as f32 + 2.0) / total * 0.85);
    }
    if unaligned > 0 {
        log::warn!("Langzeitbelichtung: {unaligned} Bilder ließen sich nicht ausrichten");
    }
    let combined = combiner.finish();
    progress(0.9);
    let out = crate::hdr::output_path(&files[0], opts.blend.suffix());
    let preview = crate::dng::quick_preview(&ref_meta, &combined, 1024);
    crate::dng::write_linear_dng(
        &out,
        &crate::dng::LinearDng {
            width: combined.width,
            height: combined.height,
            data: &combined.data,
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

    /// A textured gray scene with a bright square at `square` (left corner), shifted by `shift`.
    fn shot(w: usize, h: usize, square: Option<usize>, shift: [f32; 2], seed: u32) -> LinearImage {
        LinearImage::new(
            w,
            h,
            (0..w * h)
                .map(|i| {
                    let (x, y) = ((i % w) as f32 + shift[0], (i / w) as f32 + shift[1]);
                    let tex = value_noise(x / 5.0, y / 5.0, 11) + 0.6 * value_noise(x / 13.0, y / 13.0, 12);
                    let noise = 0.01 * value_noise(i as f32 * 0.37, seed as f32 * 7.1, seed);
                    let v = 0.2 * 2f32.powf(tex) + noise;
                    let lit = square.is_some_and(|s| (s..s + 20).contains(&(i % w)) && (40..60).contains(&(i / w)));
                    if lit { [0.9, 0.9, 0.9] } else { [v, v * 0.9, v * 0.8] }
                })
                .collect(),
        )
    }

    fn combine(blend: Blend, shots: &[LinearImage]) -> LinearImage {
        let mut c = Combiner::new(blend, shots[0].width, shots[0].height);
        for s in shots {
            c.add(s, None);
        }
        c.finish()
    }

    /// Mean green in the square of `combine`'s test images.
    fn at_square(img: &LinearImage, x: usize) -> f32 {
        let mut sum = 0.0;
        for y in 42..58 {
            for xx in x + 2..x + 18 {
                sum += img.data[y * img.width + xx][1];
            }
        }
        sum / 256.0
    }

    #[test]
    fn median_removes_what_passes_and_mean_smears_it() {
        // Something bright crosses the frame: in each shot at another place.
        let shots: Vec<LinearImage> = (0..5).map(|k| shot(200, 100, Some(20 + 35 * k), [0.0; 2], k as u32 + 1)).collect();
        let empty = shot(200, 100, None, [0.0; 2], 9);
        let median = combine(Blend::Median, &shots);
        let mean = combine(Blend::Mean, &shots);
        let brightest = combine(Blend::Brightest, &shots);
        let bg = at_square(&empty, 90);
        assert!((at_square(&median, 90) - bg).abs() < 0.03, "Median {} statt {bg}", at_square(&median, 90));
        // A fifth of the time there: a fifth of its brightness on top.
        let m = at_square(&mean, 90);
        assert!((m - (0.8 * bg + 0.2 * 0.9)).abs() < 0.03, "Mittelwert {m}");
        // Every place it passed stays bright.
        for k in 0..5 {
            assert!(at_square(&brightest, 20 + 35 * k) > 0.85);
        }
    }

    #[test]
    fn mean_lowers_noise() {
        let shots: Vec<LinearImage> = (0..8).map(|k| shot(120, 80, None, [0.0; 2], k + 1)).collect();
        let clean = {
            let mut c = shots[0].clone();
            for (i, p) in c.data.iter_mut().enumerate() {
                let (x, y) = ((i % 120) as f32, (i / 120) as f32);
                let v = 0.2 * 2f32.powf(value_noise(x / 5.0, y / 5.0, 11) + 0.6 * value_noise(x / 13.0, y / 13.0, 12));
                *p = [v, v * 0.9, v * 0.8];
            }
            c
        };
        let err = |img: &LinearImage| img.data.iter().zip(&clean.data).map(|(a, b)| (a[1] - b[1]).abs()).sum::<f32>();
        let mean = combine(Blend::Mean, &shots);
        assert!(err(&mean) < err(&shots[0]) * 0.6, "{} gegen {}", err(&mean), err(&shots[0]));
    }

    #[test]
    fn median_keeps_whole_colors_and_skips_missing_data() {
        let a = LinearImage::new(1, 1, vec![[0.1, 0.2, 0.3]]);
        let b = LinearImage::new(1, 1, vec![[0.5, 0.5, 0.5]]);
        let c = LinearImage::new(1, 1, vec![[0.9, 0.1, 0.1]]);
        let mut m = Combiner::new(Blend::Median, 1, 1);
        m.add(&a, None);
        m.add(&b, None);
        m.add(&c, Some(&[false]));
        // Of the two with data, the brighter (upper middle) – as a whole.
        let p = m.finish().data[0];
        for (got, want) in p.iter().zip([0.5, 0.5, 0.5]) {
            assert!((got - want).abs() < 1e-3, "{p:?}");
        }
    }

    #[test]
    fn encoding_keeps_shadows_and_highlights() {
        for v in [0.0, 1e-4, 0.01, 0.18, 1.0, 3.5] {
            let d = decode(encode(v));
            assert!((d - v).abs() <= v * 0.01 + 1e-6, "{v} → {d}");
        }
        assert!(encode(100.0) < NO_DATA);
    }

    #[test]
    fn median_uses_evenly_spread_shots_when_memory_is_short() {
        assert_eq!(median_shots(5, 1000), vec![0, 1, 2, 3, 4]);
        let px = MEDIAN_BYTES / 6 / 4;
        let k = median_shots(10, px);
        assert_eq!(k.len(), 4);
        assert_eq!((k[0], k[3]), (0, 9));
        assert!(k.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn shots_from_the_hand_are_aligned() {
        let (w, h) = (480, 320);
        let reference = shot(w, h, None, [0.0; 2], 1);
        let moved = shot(w, h, None, [5.0, -3.0], 2);
        let aligner = Aligner::new(&reference);
        let (warped, valid) = aligner.align(&moved).expect("ausgerichtet");
        let mean = combine(Blend::Mean, &[reference.clone(), warped]);
        let unaligned = combine(Blend::Mean, &[reference.clone(), moved]);
        let err = |img: &LinearImage| {
            (0..w * h).filter(|&i| valid[i]).map(|i| (img.data[i][1] - reference.data[i][1]).abs()).sum::<f32>()
        };
        assert!(err(&mean) < err(&unaligned) * 0.3, "{} gegen {}", err(&mean), err(&unaligned));
    }
}
