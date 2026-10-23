//! Look angleichen: an image gets the look of a reference (another developed image or any picture
//! file, e.g. a film still), as ordinary settings that stay editable.
//!
//! Both are compared as they are shown (sRGB), by statistics that need no pixel-exact match:
//!
//! - brightness: its distribution (quantiles of the luminance the luma curve works on),
//! - the color cast of nearly neutral colors, overall and in shadows, midtones and highlights,
//! - colorfulness (mean chroma) and per hue band of the color editor: hue, chroma, lightness.
//!
//! The sky is left out of the measures (it says more about the weather than about the look), and
//! the main subject counts more, if the caller finds them (see [`RegionMasks`]).
//!
//! The settings are found by rendering the target small, measuring, and correcting, a few rounds
//! per stage: exposure and white balance (together, by Gauss-Newton), the luma curve (black and
//! white point, gamma and contrast), saturation, the color editor bands, the color balance
//! wheels. How far the tone follows depends on how similar the pictures are ([`similarity`]):
//! a picture of the same shoot fully, another subject only partly. Several references are mixed,
//! each weighted by its similarity to the image.

use std::path::Path;

use anyhow::{Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::color::{Oklab, OutputSpace, mat_inv, mat_vec, srgb_decode, srgb_encode};
use crate::image::LinearImage;
use crate::params::{Adjustments, ColorWheel, Curve, WbMode, WhiteBalance};
use crate::pipeline::{Processor, band_weights};
use crate::raw::RawMeta;

/// Long edge of the renderings that are measured.
const MEASURE_EDGE: usize = 320;
/// Long edge a reference picture is reduced to.
const REFERENCE_EDGE: usize = 400;
/// Colors kept to compare what pictures show.
const SIGNATURE: usize = 3000;
/// At most this many pixels are measured.
const MAX_SAMPLES: usize = 80_000;
/// Luminance weights of the working space (as in the pipeline).
const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];
/// Positions of the points of the fitted luma curve.
const CURVE_X: [f32; 12] = [0.0, 0.03, 0.08, 0.15, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 0.95, 1.0];

/// What "Look angleichen" sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LookOptions {
    pub exposure: bool,
    pub white_balance: bool,
    /// The luma curve (contrast, black and white point, roll-off).
    pub tone: bool,
    /// Saturation and the color editor bands.
    pub colors: bool,
    /// The color balance wheels of shadows, midtones and highlights (split toning).
    pub color_balance: bool,
}

impl Default for LookOptions {
    fn default() -> Self {
        Self { exposure: true, white_balance: true, tone: true, colors: true, color_balance: true }
    }
}

impl LookOptions {
    pub fn any(&self) -> bool {
        self.exposure || self.white_balance || self.tone || self.colors || self.color_balance
    }

    /// `onto` with the settings these options cover taken from `matched`.
    pub fn take(&self, matched: &Adjustments, onto: &Adjustments) -> Adjustments {
        let mut out = onto.clone();
        // The camera's look, if the match was made on it (see `camera_look`).
        if matched.camera_curve.is_some() && onto.camera_curve.is_none() && crate::camera_look::wanted(onto, true) {
            (out.base_curve, out.camera_curve) = (matched.base_curve, matched.camera_curve.clone());
        }
        if self.exposure {
            out.exposure = matched.exposure;
        }
        if self.white_balance {
            out.white_balance = matched.white_balance.clone();
        }
        if self.tone {
            out.curves.luma = matched.curves.luma.clone();
        }
        if self.colors {
            out.saturation = matched.saturation;
            out.color_editor.bands = matched.color_editor.bands;
        }
        if self.color_balance {
            out.color_balance = matched.color_balance;
        }
        out
    }

    /// `current` with the settings these options cover `t` of the way from `before` to `matched`
    /// (0..1: the strength of the look); `as_shot` (kelvin, tint) stands in for "as shot".
    pub fn blend(&self, current: &Adjustments, before: &Adjustments, matched: &Adjustments, t: f32, as_shot: (f32, f32)) -> Adjustments {
        let t = t.clamp(0.0, 1.0);
        let lerp = |a: f32, b: f32| a + (b - a) * t;
        let mut out = current.clone();
        if self.exposure {
            out.exposure = (lerp(before.exposure, matched.exposure) * 100.0).round() / 100.0 + 0.0;
        }
        if self.white_balance {
            out.white_balance = crate::auto::blend_white_balance(&before.white_balance, &matched.white_balance, t, as_shot);
        }
        if self.tone {
            out.curves.luma = if t >= 1.0 {
                matched.curves.luma.clone()
            } else {
                let lut = before.curves.luma.build_lut(256);
                let points = matched.curves.luma.points.iter().map(|p| [p[0], round4(lerp(lut_at(&lut, p[0]), p[1]))]).collect();
                Curve { points }
            };
        }
        if self.colors {
            out.saturation = lerp(before.saturation, matched.saturation).round() + 0.0;
            for ((o, b), m) in out.color_editor.bands.iter_mut().zip(&before.color_editor.bands).zip(&matched.color_editor.bands) {
                o.hue = round1(lerp(b.hue, m.hue));
                o.saturation = lerp(b.saturation, m.saturation).round() + 0.0;
                o.lightness = lerp(b.lightness, m.lightness).round() + 0.0;
            }
        }
        if self.color_balance {
            let (b, m) = (&before.color_balance, &matched.color_balance);
            let mix = |b: &ColorWheel, m: &ColorWheel| {
                let (ob, om) = (b.offset(), m.offset());
                wheel([lerp(ob[0], om[0]), lerp(ob[1], om[1])], lerp(b.lightness, m.lightness))
            };
            out.color_balance.master = mix(&b.master, &m.master);
            out.color_balance.shadows = mix(&b.shadows, &m.shadows);
            out.color_balance.midtones = mix(&b.midtones, &m.midtones);
            out.color_balance.highlights = mix(&b.highlights, &m.highlights);
        }
        out
    }
}

/// Measures of a picture as shown, to compare looks.
#[derive(Clone, Debug, PartialEq)]
pub struct LookStats {
    /// Luminance of the encoded working RGB (what the luma curve works on), sorted.
    luma: Vec<f32>,
    /// Mean a/b of nearly neutral midtones: the overall color cast.
    neutral: [f32; 2],
    /// Shadows, midtones, highlights (the weights of the color balance wheels): share of the
    /// nearly neutral pixels, their mean a/b.
    zones: [Zone; 3],
    /// How much measuring with the weight of zone i sees of a wheel acting with the weight of
    /// zone j (the zones overlap).
    cross: [[f32; 3]; 3],
    /// Mean Oklab chroma.
    chroma: f32,
    /// Share of the picture that is sky (left out of all other measures).
    sky: f32,
    /// A sample of the Oklab colors, to compare what the pictures show (see [`similarity`]).
    samples: Vec<[f32; 3]>,
    /// Where it is bright and dark: the mean brightness of a coarse grid over the whole picture
    /// (see [`layout_distance`]).
    layout: Vec<f32>,
    /// How far colors spread around the mean color of their brightness: next to nothing for a
    /// black-and-white picture, also a toned one.
    spread: f32,
    bands: [Band; 8],
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Zone {
    share: f32,
    ab: [f32; 2],
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Band {
    /// Share of the pixels (weighted by band and colorfulness).
    share: f32,
    /// Mean hue in degrees, chroma and lightness.
    hue: f32,
    chroma: f32,
    lightness: f32,
}

/// An 8-bit mask (0 = not, 255 = fully), in coordinates of the picture it was found in.
#[derive(Clone, Debug, PartialEq)]
pub struct Mask8 {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl Mask8 {
    /// At normalized coordinates (nearest).
    fn at(&self, u: f32, v: f32) -> f32 {
        let x = ((u * self.width as f32) as usize).min(self.width.saturating_sub(1));
        let y = ((v * self.height as f32) as usize).min(self.height.saturating_sub(1));
        self.data.get(y * self.width + x).map_or(0.0, |v| *v as f32 / 255.0)
    }

    fn mean(&self) -> f32 {
        self.data.iter().map(|v| *v as f32).sum::<f32>() / (255.0 * self.data.len().max(1) as f32)
    }
}

/// Where sky and the main subject are in a picture (found by the caller, e.g. with the masks of
/// `sk-ai`); either may be missing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RegionMasks {
    pub sky: Option<Mask8>,
    pub subject: Option<Mask8>,
}

/// Finds the regions of an 8-bit sRGB RGBA picture of the given size.
pub type FindRegions<'a> = &'a mut dyn FnMut(usize, usize, &[u8]) -> RegionMasks;

/// No regions: the whole picture counts alike.
pub fn no_regions(_: usize, _: usize, _: &[u8]) -> RegionMasks {
    RegionMasks::default()
}

impl LookStats {
    /// From an 8-bit sRGB RGBA picture; without sky, the main subject three times.
    pub fn from_rgba(width: usize, height: usize, rgba: &[u8], regions: &RegionMasks) -> Option<Self> {
        let n = width * height;
        if n == 0 || rgba.len() < n * 4 {
            return None;
        }
        let step = n.div_ceil(MAX_SAMPLES).max(1);
        // A sky covering nearly everything, a subject covering nearly everything or nothing: no
        // use for weighting.
        let sky = regions.sky.as_ref().filter(|m| m.mean() < 0.8);
        let subject = regions.subject.as_ref().filter(|m| (0.02..0.7).contains(&m.mean()));
        let decode: Vec<f32> = (0..256).map(|v| srgb_decode(v as f32 / 255.0)).collect();
        let to_working = mat_inv(&OutputSpace::Srgb.from_working()).expect("invertible");
        let oklab = Oklab::default();
        let mut luma = Vec::with_capacity(n / step + 1);
        let mut labs = Vec::with_capacity(n / step + 1);
        let mut acc = Acc::default();
        let (mut samples, mut in_sky) = (0usize, 0usize);
        for i in (0..n).step_by(step) {
            let (u, v) = (((i % width) as f32 + 0.5) / width as f32, ((i / width) as f32 + 0.5) / height as f32);
            samples += 1;
            if sky.is_some_and(|m| m.at(u, v) >= 0.5) {
                in_sky += 1;
                continue;
            }
            let count = if subject.is_some_and(|m| m.at(u, v) >= 0.5) { 3 } else { 1 };
            let p = &rgba[i * 4..i * 4 + 3];
            let lin = mat_vec(&to_working, [p[0], p[1], p[2]].map(|v| decode[v as usize])).map(|v| v.max(0.0));
            let enc = lin.map(|v| srgb_encode(v.min(1.0)));
            let y = LUMA[0] * enc[0] + LUMA[1] * enc[1] + LUMA[2] * enc[2];
            let lab = oklab.from_linear(lin);
            for _ in 0..count {
                luma.push(y);
                labs.push(lab);
                acc.add(lab);
            }
        }
        if luma.len() < 500 {
            // Almost all sky: then the sky is the picture.
            return if sky.is_some() { Self::from_rgba(width, height, rgba, &RegionMasks::default()) } else { None };
        }
        luma.sort_unstable_by(|a, b| a.total_cmp(b));
        let sky_share = in_sky as f32 / samples.max(1) as f32;
        let samples = labs.iter().step_by(labs.len().div_ceil(SIGNATURE).max(1)).copied().collect();
        Some(LookStats { sky: sky_share, samples, layout: layout(width, height, rgba), ..acc.finish(luma) })
    }

    /// Of an image as developed with `adj`.
    pub fn of_development(meta: &RawMeta, adj: &Adjustments, preview: &LinearImage, find: FindRegions) -> Option<Self> {
        let small = crate::camera_look::reduced(preview, REFERENCE_EDGE);
        let r = Processor::new(meta, adj).render_cpu(&small, None, None);
        Self::from_rgba(r.width, r.height, &r.rgba, &find(r.width, r.height, &r.rgba))
    }

    /// Of a picture file (JPEG, PNG, TIFF, WebP), taken as sRGB.
    pub fn of_file(path: &Path, find: FindRegions) -> Result<Self> {
        let rgba = picture_thumbnail(path, REFERENCE_EDGE as u32)?;
        let (w, h) = (rgba.width() as usize, rgba.height() as usize);
        Self::from_rgba(w, h, rgba.as_raw(), &find(w, h, rgba.as_raw())).context("Das Bild ist zu klein")
    }

    /// Several references as one, each with its weight (the weights need not add up to 1).
    pub fn combine(refs: &[(&LookStats, f32)]) -> Option<LookStats> {
        let total: f32 = refs.iter().map(|(_, w)| w.max(0.0)).sum();
        if refs.is_empty() || total <= 0.0 {
            return None;
        }
        if refs.len() == 1 {
            return Some(refs[0].0.clone());
        }
        let w: Vec<f32> = refs.iter().map(|(_, w)| w.max(0.0) / total).collect();
        let sum = |f: &dyn Fn(&LookStats) -> f32| refs.iter().zip(&w).map(|((s, _), w)| f(s) * w).sum::<f32>();
        const Q: usize = 1024;
        let luma = (0..Q).map(|i| sum(&|s| s.quantile(i as f32 / (Q - 1) as f32))).collect();
        let zones = std::array::from_fn(|z| {
            let share = sum(&|s| s.zones[z].share);
            let ab = std::array::from_fn(|c| if share > 1e-6 { sum(&|s| s.zones[z].share * s.zones[z].ab[c]) / share } else { 0.0 });
            Zone { share, ab }
        });
        let bands = std::array::from_fn(|k| {
            let share = sum(&|s| s.bands[k].share);
            if share <= 1e-6 {
                return Band::default();
            }
            let by_share = |f: &dyn Fn(&Band) -> f32| sum(&|s| s.bands[k].share * f(&s.bands[k])) / share;
            let (ca, cb) = (by_share(&|b| b.hue.to_radians().cos()), by_share(&|b| b.hue.to_radians().sin()));
            Band { share, hue: cb.atan2(ca).to_degrees().rem_euclid(360.0), chroma: by_share(&|b| b.chroma), lightness: by_share(&|b| b.lightness) }
        });
        Some(LookStats {
            luma,
            neutral: [sum(&|s| s.neutral[0]), sum(&|s| s.neutral[1])],
            zones,
            cross: std::array::from_fn(|i| std::array::from_fn(|j| sum(&|s| s.cross[i][j]))),
            chroma: sum(&|s| s.chroma),
            sky: sum(&|s| s.sky),
            samples: refs.iter().flat_map(|(s, _)| s.samples.iter().copied()).collect(),
            layout: Vec::new(),
            spread: sum(&|s| s.spread),
            bands,
        })
    }

    /// The reference has (almost) no color: a black-and-white picture, maybe toned.
    fn monochrome(&self) -> bool {
        self.spread < 0.008
    }

    fn quantile(&self, q: f32) -> f32 {
        quantile(&self.luma, q)
    }

    /// Mean brightness of the middle 96 % (clipped ends say little).
    fn mean_luma(&self) -> f32 {
        let n = self.luma.len();
        mean_std(self.luma[n / 50..n - n / 50].iter().copied()).0
    }
}

/// Sums over the pixels.
#[derive(Default)]
struct Acc {
    n: f32,
    neutral: [f32; 3],
    zones: [[f32; 3]; 3],
    cross: [[f32; 3]; 3],
    chroma: f32,
    /// Per brightness step: count, sum of a, of b, of a² + b².
    steps: [[f32; 4]; 16],
    /// Per band: weight, weight × chroma, × a, × b, × lightness.
    bands: [[f32; 5]; 8],
}

impl Acc {
    fn add(&mut self, [l, a, b]: [f32; 3]) {
        let c = (a * a + b * b).sqrt();
        self.n += 1.0;
        self.chroma += c;
        let step = &mut self.steps[((l.clamp(0.0, 1.0) * 16.0) as usize).min(15)];
        step[0] += 1.0;
        step[1] += a;
        step[2] += b;
        step[3] += a * a + b * b;
        // Nearly neutral colors show the cast; colorful ones are mostly what was photographed.
        let neutral = 1.0 / (1.0 + (c / 0.05).powi(2));
        let lw = l.clamp(0.0, 1.0);
        let mid = smoothstep(0.1, 0.25, lw) * (1.0 - smoothstep(0.85, 0.97, lw));
        let w = neutral * mid;
        self.neutral[0] += w;
        self.neutral[1] += w * a;
        self.neutral[2] += w * b;
        let zone = zone_weights(lw);
        for ((z, cross), zw) in self.zones.iter_mut().zip(&mut self.cross).zip(zone) {
            let wi = zw * neutral;
            z[0] += wi;
            z[1] += wi * a;
            z[2] += wi * b;
            for (v, zj) in cross.iter_mut().zip(zone) {
                *v += wi * zj;
            }
        }
        let colorful = smoothstep(0.02, 0.06, c);
        if colorful > 0.0 {
            let h = b.atan2(a).to_degrees().rem_euclid(360.0);
            for (band, w) in self.bands.iter_mut().zip(band_weights(h)) {
                let w = w * colorful;
                if w > 0.0 {
                    band[0] += w;
                    band[1] += w * c;
                    band[2] += w * a;
                    band[3] += w * b;
                    band[4] += w * l;
                }
            }
        }
    }

    fn finish(self, luma: Vec<f32>) -> LookStats {
        let n = self.n.max(1.0);
        let mean = |s: [f32; 3]| if s[0] > 1e-6 { [s[1] / s[0], s[2] / s[0]] } else { [0.0; 2] };
        let zones = self.zones.map(|z| Zone { share: z[0] / n, ab: mean(z) });
        let cross = std::array::from_fn(|i| {
            std::array::from_fn(|j| if self.zones[i][0] > 1e-6 { self.cross[i][j] / self.zones[i][0] } else { (i == j) as u8 as f32 })
        });
        let bands = self.bands.map(|s| {
            if s[0] < 1e-6 {
                return Band::default();
            }
            Band { share: s[0] / n, hue: s[3].atan2(s[2]).to_degrees().rem_euclid(360.0), chroma: s[1] / s[0], lightness: s[4] / s[0] }
        });
        let variance: f32 = self.steps.iter().filter(|s| s[0] > 0.0).map(|s| (s[3] - (s[1] * s[1] + s[2] * s[2]) / s[0]).max(0.0)).sum();
        LookStats { luma, neutral: mean(self.neutral), zones, cross, chroma: self.chroma / n, sky: 0.0, samples: Vec::new(), layout: Vec::new(), spread: (variance / n).sqrt(), bands }
    }
}

/// Weights of the color balance wheels for shadows, midtones and highlights at Oklab lightness
/// `l` (as in the pipeline).
fn zone_weights(l: f32) -> [f32; 3] {
    let s = (1.0 - l) * (1.0 - l);
    let h = l * l;
    [s, (1.0 - s - h).max(0.0), h]
}

/// Columns and rows of [`LookStats::layout`].
const LAYOUT: (usize, usize) = (16, 12);

/// Mean brightness of each cell of a grid over the picture.
fn layout(width: usize, height: usize, rgba: &[u8]) -> Vec<f32> {
    let (gw, gh) = LAYOUT;
    let mut sum = vec![0.0f32; gw * gh];
    let mut count = vec![0.0f32; gw * gh];
    for y in (0..height).step_by((height / 96).max(1)) {
        for x in (0..width).step_by((width / 128).max(1)) {
            let p = &rgba[(y * width + x) * 4..];
            let cell = (y * gh / height) * gw + x * gw / width;
            sum[cell] += 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32;
            count[cell] += 1.0;
        }
    }
    sum.iter().zip(&count).map(|(s, c)| s / c.max(1.0)).collect()
}

/// Ranks of the values, 0..1 (so that neither exposure nor curves change them).
fn ranks(v: &[f32]) -> Vec<f32> {
    let mut order: Vec<usize> = (0..v.len()).collect();
    order.sort_by(|a, b| v[*a].total_cmp(&v[*b]));
    let mut rank = vec![0.0; v.len()];
    let n = (v.len().max(2) - 1) as f32;
    for (r, i) in order.into_iter().enumerate() {
        rank[i] = r as f32 / n;
    }
    rank
}

/// How differently two pictures are laid out, 0 (alike) to about 0.33 (unrelated): the brightness
/// ranks of their grids, compared where they overlap after the best shift (up to a fifth of the
/// picture) and zoom (0.8 to 1.25), so that a slightly other framing of the same scene counts
/// as the same.
fn layout_distance(a: &[f32], b: &[f32]) -> f32 {
    let (gw, gh) = LAYOUT;
    if a.len() != gw * gh || b.len() != gw * gh {
        return 0.33;
    }
    let mut best = f32::MAX;
    let (mut va, mut vb) = (Vec::with_capacity(gw * gh), Vec::with_capacity(gw * gh));
    for scale in [0.8f32, 0.9, 1.0, 1.1, 1.25] {
        for sy in -4..=4 {
            for sx in -4..=4 {
                let (dx, dy) = (sx as f32 * 0.05, sy as f32 * 0.05);
                va.clear();
                vb.clear();
                for y in 0..gh {
                    for x in 0..gw {
                        // Where this cell of `a` lies in `b`.
                        let u = 0.5 + ((x as f32 + 0.5) / gw as f32 - 0.5) * scale + dx;
                        let v = 0.5 + ((y as f32 + 0.5) / gh as f32 - 0.5) * scale + dy;
                        if (0.0..1.0).contains(&u) && (0.0..1.0).contains(&v) {
                            va.push(a[y * gw + x]);
                            vb.push(b[(v * gh as f32) as usize * gw + (u * gw as f32) as usize]);
                        }
                    }
                }
                // Too little in common says nothing.
                if va.len() < gw * gh * 2 / 5 {
                    continue;
                }
                let d = ranks(&va).iter().zip(ranks(&vb)).map(|(x, y)| (x - y).abs()).sum::<f32>() / va.len() as f32;
                // Shifting and zooming cost a little, so that unrelated pictures do not find a
                // good match by chance.
                best = best.min(d + 0.02 * (dx.abs() + dy.abs()) + 0.03 * scale.ln().abs());
            }
        }
    }
    if best == f32::MAX { 0.33 } else { best }
}

/// A picture file reduced to fit `edge`, as 8-bit sRGB RGBA.
pub fn picture_thumbnail(path: &Path, edge: u32) -> Result<image::RgbaImage> {
    let img = image::open(path).with_context(|| format!("{} lässt sich nicht öffnen", path.display()))?;
    Ok(img.thumbnail(edge, edge).to_rgba8())
}

/// How alike two pictures are, 0..1, regardless of their look: the shape of the brightness
/// distribution (between its black and white point), which colors there are and how much of
/// them, how much sky. About 0.9 and more for pictures of the same scene, low for another subject.
pub fn similarity(a: &LookStats, b: &LookStats) -> f32 {
    let shape = |s: &LookStats| {
        let (lo, hi) = (s.quantile(0.01), s.quantile(0.99));
        let range = (hi - lo).max(0.05);
        (1..10).map(|i| (s.quantile(i as f32 / 10.0) - lo) / range).collect::<Vec<f32>>()
    };
    let d_shape = shape(a).iter().zip(shape(b)).map(|(x, y)| (x - y).abs()).sum::<f32>() / 9.0;
    // What colors there are, with each picture's own cast taken out like a white balance (a
    // series under other light shows the same things): shares of twelve hue sectors and of
    // "hardly any color".
    let oklab = Oklab::default();
    let balanced = |s: &LookStats| -> Vec<[f32; 3]> {
        let grey = oklab.to_linear([0.6, s.neutral[0], s.neutral[1]]).map(|v| v.max(1e-4));
        let y = LUMA[0] * grey[0] + LUMA[1] * grey[1] + LUMA[2] * grey[2];
        s.samples
            .iter()
            .map(|p| {
                let lin = oklab.to_linear(*p);
                oklab.from_linear([0, 1, 2].map(|c| (lin[c] * y / grey[c]).max(0.0)))
            })
            .collect()
    };
    let (sa, sb) = (balanced(a), balanced(b));
    let hues = |samples: &[[f32; 3]]| {
        let mut v = [0.0f32; 13];
        for p in samples {
            let c = (p[1] * p[1] + p[2] * p[2]).sqrt();
            let bin = if c < 0.03 { 12 } else { ((p[2].atan2(p[1]).to_degrees().rem_euclid(360.0) / 30.0) as usize).min(11) };
            v[bin] += 1.0;
        }
        let total = v.iter().sum::<f32>().max(1.0);
        v.map(|x| x / total)
    };
    let d_hues = 0.5 * hues(&sa).iter().zip(hues(&sb)).map(|(x, y)| (x - y).abs()).sum::<f32>();
    let colorful = |samples: &[[f32; 3]]| (samples.iter().map(|p| (p[1] * p[1] + p[2] * p[2]).sqrt()).sum::<f32>() / samples.len().max(1) as f32).max(0.005);
    let d_layout = layout_distance(&a.layout, &b.layout);
    let d_sky = (a.sky - b.sky).abs();
    let d_color = (colorful(&sa) / colorful(&sb)).ln().abs();
    let d = 1.2 * d_layout + 0.8 * d_shape + 0.5 * d_hues + 0.6 * d_sky + 0.15 * d_color;
    let kind = (-(d / 0.3).powi(2)).exp();
    // Nearly the same composition: a series, whatever the light and the look.
    let series = (-(d_layout / 0.14).powi(2)).exp() * (-(d_sky / 0.15).powi(2)).exp();
    kind.max(series)
}

/// The result of [`match_look`].
#[derive(Clone, Debug)]
pub struct LookMatch {
    pub adjustments: Adjustments,
    /// Per reference: how similar it is to the image, and its share of the look (adding up to 1).
    pub similarity: Vec<f32>,
    pub weights: Vec<f32>,
}

/// `adj` changed so that the image looks like the references (mixed by their similarity to it).
pub fn match_look(references: &[LookStats], meta: &RawMeta, adj: &Adjustments, preview: &LinearImage, opts: LookOptions, find: FindRegions) -> Option<LookMatch> {
    let small = crate::camera_look::reduced(preview, MEASURE_EDGE);
    let first = Processor::new(meta, adj).render_cpu(&small, None, None);
    // The regions are found once: every rendering has the same size and framing.
    let regions = find(first.width, first.height, &first.rgba);
    let measure = |a: &Adjustments| {
        let r = Processor::new(meta, a).render_cpu(&small, None, None);
        LookStats::from_rgba(r.width, r.height, &r.rgba, &regions)
    };
    let mut now = LookStats::from_rgba(first.width, first.height, &first.rgba, &regions)?;
    let similarity: Vec<f32> = references.iter().map(|r| self::similarity(r, &now)).collect();
    // The more alike, the more weight: a reference of the same kind of scene sets the look.
    let raw: Vec<f32> = similarity.iter().map(|s| s.powi(3) + 1e-4).collect();
    let total: f32 = raw.iter().sum();
    let weights: Vec<f32> = raw.iter().map(|w| w / total).collect();
    let pairs: Vec<(&LookStats, f32)> = references.iter().zip(&weights).map(|(r, w)| (r, *w)).collect();
    let reference = &LookStats::combine(&pairs)?;
    let alike: f32 = similarity.iter().zip(&weights).map(|(s, w)| s * w).sum();

    let mut out = adj.clone();
    // A black-and-white reference: the colors go, the white balance does not matter.
    let opts = LookOptions { white_balance: opts.white_balance && !(opts.colors && reference.monochrome()), ..opts };
    let bounds = Bounds::for_similarity(alike);
    if opts.exposure || opts.white_balance {
        now = match_exposure_and_white_balance(reference, meta, &mut out, now, opts, &bounds, &measure);
    }
    // The curve is always fitted to the image without it (or with the one it had), so that a
    // second fit replaces the first instead of adding to it.
    let base = out.curves.luma.clone();
    if opts.tone {
        fit_luma_curve(reference, &now, &base, &mut out, &bounds);
        now = measure(&out).unwrap_or(now);
    }
    if opts.colors {
        for _ in 0..2 {
            match_saturation(reference, &now, &mut out, &bounds);
            now = measure(&out).unwrap_or(now);
            if reference.monochrome() {
                break;
            }
            match_bands(reference, &now, &mut out, &bounds);
            now = measure(&out).unwrap_or(now);
        }
    }
    if opts.color_balance {
        match_color_balance(reference, now, &mut out, &measure);
    }
    // Colors change the brightness a little: once more on the curve.
    if opts.tone && (opts.colors || opts.color_balance) {
        let without = Adjustments { curves: crate::params::Curves { luma: base.clone(), ..out.curves.clone() }, ..out.clone() };
        if let Some(plain) = measure(&without) {
            fit_luma_curve(reference, &plain, &base, &mut out, &bounds);
        }
    }
    Some(LookMatch { adjustments: out, similarity, weights })
}

/// How far the image follows the reference.
#[derive(Clone, Copy, Debug)]
struct Bounds {
    /// Exposure: at most this many EV, and this share of the way.
    ev: f32,
    ev_share: f32,
    /// Share of the way to the reference's color cast.
    cast_share: f32,
    /// Limits of the range stretch, the gamma and the contrast of the curve.
    stretch: (f32, f32),
    gamma: (f32, f32),
    contrast: (f32, f32),
    /// How far saturation and the color editor bands follow (exponent of the ratios, share of
    /// the hue difference).
    color_share: f32,
}

impl Bounds {
    /// A picture of the same scene (a series): matched fully.
    const SERIES: Bounds = Bounds { ev: 1.5, ev_share: 1.0, cast_share: 1.0, stretch: (0.75, 1.33), gamma: (0.7, 1.45), contrast: (-0.5, 0.6), color_share: 1.0 };
    /// Another subject, whose brightness, range and colors say much about what is in it, and
    /// less about the look.
    const OTHER: Bounds = Bounds { ev: 0.7, ev_share: 0.5, cast_share: 0.7, stretch: (0.85, 1.2), gamma: (0.8, 1.25), contrast: (-0.3, 0.4), color_share: 0.6 };

    /// Between the two, by how alike the pictures are (see [`similarity`]).
    fn for_similarity(s: f32) -> Bounds {
        let t = smoothstep(0.3, 0.85, s);
        let (a, b) = (Self::OTHER, Self::SERIES);
        let l = |x: f32, y: f32| x + (y - x) * t;
        let lp = |x: (f32, f32), y: (f32, f32)| (l(x.0, y.0), l(x.1, y.1));
        Bounds {
            ev: l(a.ev, b.ev),
            ev_share: l(a.ev_share, b.ev_share),
            cast_share: l(a.cast_share, b.cast_share),
            stretch: lp(a.stretch, b.stretch),
            gamma: lp(a.gamma, b.gamma),
            contrast: lp(a.contrast, b.contrast),
            color_share: l(a.color_share, b.color_share),
        }
    }
}

/// Exposure brings the mean brightness (within `bounds`), white balance the cast of the nearly neutral midtones
/// to those of the reference (the cast as shown, so that a deliberately warm reference makes the
/// image equally warm, whatever light it was shot in).
fn match_exposure_and_white_balance(
    reference: &LookStats,
    meta: &RawMeta,
    out: &mut Adjustments,
    mut now: LookStats,
    opts: LookOptions,
    bounds: &Bounds,
    measure: &Measure,
) -> LookStats {
    // Variables: exposure (EV), white balance (mired, tint); residuals: brightness, cast a, cast b.
    let (k0, t0) = match out.white_balance.mode {
        WbMode::AsShot => meta.color.temp_tint_for_wb(meta.as_shot_wb),
        WbMode::Custom => (out.white_balance.kelvin, out.white_balance.tint),
    };
    let mut x = [out.exposure, 1e6 / k0, t0];
    let ev0 = out.exposure;
    let used = [opts.exposure, opts.white_balance, opts.white_balance];
    let delta = [0.3, 20.0, 8.0];
    let max_step = [1.5, 80.0, 30.0];
    let set = |out: &mut Adjustments, x: &[f32; 3]| {
        if opts.exposure {
            out.exposure = (x[0].clamp(ev0 - bounds.ev, ev0 + bounds.ev).clamp(-4.0, 4.0) * 100.0).round() / 100.0 + 0.0;
        }
        if opts.white_balance {
            let kelvin = (1e6 / x[1].max(1e6 / 15000.0)).clamp(2000.0, 15000.0);
            out.white_balance = WhiteBalance { mode: WbMode::Custom, kelvin: kelvin.round(), tint: round1(x[2].clamp(-150.0, 150.0)) };
        }
    };
    // The targets: part of the way from where the image starts.
    let toward = |from: f32, to: f32, share: f32| from + (to - from) * share;
    let target = [
        toward(now.mean_luma(), reference.mean_luma(), bounds.ev_share),
        toward(now.neutral[0], reference.neutral[0], bounds.cast_share),
        toward(now.neutral[1], reference.neutral[1], bounds.cast_share),
    ];
    let residual = |s: &LookStats| {
        let r = [target[0] - s.mean_luma(), target[1] - s.neutral[0], target[2] - s.neutral[1]];
        std::array::from_fn::<f32, 3, _>(|i| if used[i] { r[i] } else { 0.0 })
    };
    for _ in 0..4 {
        let r = residual(&now);
        if r[0].abs() < 0.004 && r[1].abs().max(r[2].abs()) < 0.0015 {
            break;
        }
        // Jacobian by finite differences, column by column (measured at the same time).
        let mut jac = [[0.0f32; 3]; 3];
        let columns: Vec<usize> = (0..3).filter(|j| used[*j]).collect();
        let probes: Vec<Adjustments> = columns
            .iter()
            .map(|&j| {
                let mut xj = x;
                xj[j] += delta[j];
                let mut a = out.clone();
                set(&mut a, &xj);
                a
            })
            .collect();
        for (j, s) in columns.into_iter().zip(measure_all(measure, &probes)) {
            let Some(s) = s else { return now };
            let rj = residual(&s);
            // Change of the measures per finite step (in units of the step, which keeps the
            // equations well scaled).
            for i in 0..3 {
                jac[i][j] = r[i] - rj[i];
            }
        }
        let Some(step) = solve_least_squares(&jac, &r, &used, 1e-3) else { break };
        for j in 0..3 {
            x[j] += (step[j] * delta[j]).clamp(-max_step[j], max_step[j]);
        }
        x[0] = x[0].clamp(ev0 - bounds.ev, ev0 + bounds.ev);
        set(out, &x);
        match measure(out) {
            Some(s) => now = s,
            None => break,
        }
    }
    now
}

/// Renders settings and measures their look (see `match_look`).
type Measure<'a> = dyn Fn(&Adjustments) -> Option<LookStats> + Sync + 'a;

/// `measure` of each of `probes`, at the same time: one rendering this small keeps few cores busy.
fn measure_all(measure: &Measure, probes: &[Adjustments]) -> Vec<Option<LookStats>> {
    probes.par_iter().map(measure).collect()
}

/// Damped normal equations (Levenberg) restricted to the used variables (the others stay put).
fn solve_least_squares(jac: &[[f32; 3]; 3], r: &[f32; 3], used: &[bool; 3], damping: f32) -> Option<[f32; 3]> {
    let mut jtj = [[0.0f32; 3]; 3];
    let mut jtr = [0.0f32; 3];
    for a in 0..3 {
        for b in 0..3 {
            jtj[a][b] = (0..3).map(|i| jac[i][a] * jac[i][b]).sum();
        }
        jtr[a] = (0..3).map(|i| jac[i][a] * r[i]).sum();
    }
    let trace: f32 = (0..3).filter(|j| used[*j]).map(|j| jtj[j][j]).sum();
    if trace <= 1e-12 {
        return None;
    }
    for j in 0..3 {
        if used[j] {
            jtj[j][j] += damping * trace;
        } else {
            jtj[j] = [0.0; 3];
            for row in &mut jtj {
                row[j] = 0.0;
            }
            jtj[j][j] = 1.0;
            jtr[j] = 0.0;
        }
    }
    solve3(jtj, jtr)
}

/// `a x = b` by Gaussian elimination with partial pivoting (in f64: the entries of the normal
/// equations can be tiny, too small for a fixed threshold on the determinant).
fn solve3(a: [[f32; 3]; 3], b: [f32; 3]) -> Option<[f32; 3]> {
    let mut m: [[f64; 4]; 3] = std::array::from_fn(|i| [a[i][0] as f64, a[i][1] as f64, a[i][2] as f64, b[i] as f64]);
    let scale = m.iter().flat_map(|r| r[..3].iter()).fold(0.0f64, |s, v| s.max(v.abs()));
    if scale <= 0.0 {
        return None;
    }
    for c in 0..3 {
        let p = (c..3).max_by(|&i, &j| m[i][c].abs().total_cmp(&m[j][c].abs()))?;
        if m[p][c].abs() < scale * 1e-12 {
            return None;
        }
        m.swap(c, p);
        for r in 0..3 {
            if r != c {
                let (f, pivot) = (m[r][c] / m[c][c], m[c]);
                for (v, p) in m[r][c..].iter_mut().zip(&pivot[c..]) {
                    *v -= f * p;
                }
            }
        }
    }
    let x = [m[0][3] / m[0][0], m[1][3] / m[1][1], m[2][3] / m[2][2]].map(|v| v as f32);
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Composes the luma curve with a smooth tone curve that gives the image the reference's black
/// and white point (1st and 99th percentile), mean brightness (a gamma) and contrast (an S-curve
/// matching the spread). Copying the whole distribution would copy the picture's content: a
/// reference half bright sky, half dark water would turn any image into two tones.
fn fit_luma_curve(reference: &LookStats, now: &LookStats, base: &Curve, out: &mut Adjustments, bounds: &Bounds) {
    const N: usize = 64;
    let (t_lo, t_hi) = (now.quantile(0.01), now.quantile(0.99));
    if t_hi - t_lo < 0.05 {
        return;
    }
    // The reference's black and white point, but the range stretched or squeezed only a
    // little (around its middle): the range of a picture is also a matter of its content.
    let (ref_lo, ref_hi) = (reference.quantile(0.01).min(0.3), reference.quantile(0.99).max(0.6));
    let k = ((ref_hi - ref_lo) / (t_hi - t_lo)).clamp(bounds.stretch.0, bounds.stretch.1);
    let mid = (ref_lo + ref_hi) / 2.0;
    let half = k * (t_hi - t_lo) / 2.0;
    let (r_lo, r_hi) = ((mid - half).max(0.0), (mid + half).min(1.0));
    // Both normalized to their black and white point.
    let normalized = |s: &LookStats, lo: f32, hi: f32| -> Vec<f32> {
        let step = (s.luma.len() / 2000).max(1);
        s.luma.iter().step_by(step).map(|v| ((v - lo) / (hi - lo)).clamp(0.0, 1.0)).collect()
    };
    let (u, v) = (normalized(now, t_lo, t_hi), normalized(reference, reference.quantile(0.01), reference.quantile(0.99)));
    let (mean_v, std_v) = mean_std(v.iter().copied());
    // Gamma and contrast, searched on a grid; a little pull towards no change.
    let mut best = (f32::MAX, 1.0, 0.0);
    for gi in 0..=24 {
        let gamma = bounds.gamma.0 * (bounds.gamma.1 / bounds.gamma.0).powf(gi as f32 / 24.0);
        // Once per gamma, not per contrast.
        let lifted: Vec<f32> = u.iter().map(|x| x.powf(gamma)).collect();
        for ci in 0..=15 {
            let c = bounds.contrast.0 + (bounds.contrast.1 - bounds.contrast.0) * ci as f32 / 15.0;
            let (m, sd) = mean_std(lifted.iter().map(|x| s_curve(*x, c)));
            let cost = (m - mean_v).powi(2) + (sd - std_v).powi(2) + 0.002 * (gamma.ln().powi(2) + c * c);
            if cost < best.0 {
                best = (cost, gamma, c);
            }
        }
    }
    let (_, gamma, c) = best;
    let mut f: Vec<f32> = (0..=N)
        .map(|i| {
            let x = i as f32 / N as f32;
            if x < t_lo {
                r_lo - (t_lo - x)
            } else if x > t_hi {
                r_hi + (x - t_hi)
            } else {
                r_lo + (r_hi - r_lo) * s_curve(((x - t_lo) / (t_hi - t_lo)).powf(gamma), c)
            }
        })
        .collect();
    // Rising, with slopes between 0.25 and 3.
    let dx = 1.0 / N as f32;
    f[0] = f[0].clamp(0.0, 0.3);
    for i in 1..=N {
        f[i] = f[i].clamp(f[i - 1] + 0.25 * dx, f[i - 1] + 3.0 * dx).min(1.0);
    }
    let old = base.build_lut(256);
    let points = CURVE_X.iter().map(|&x| [x, round4(lut_at(&f, lut_at(&old, x)))]).collect();
    out.curves.luma = Curve { points };
}

/// Contrast around the middle: steeper there for `c` > 0, flatter for `c` < 0 (-0.9..0.9 keeps
/// it rising); 0 and 1 stay.
fn s_curve(u: f32, c: f32) -> f32 {
    u + c * u * (1.0 - u) * (2.0 * u - 1.0)
}

fn mean_std(values: impl Iterator<Item = f32>) -> (f32, f32) {
    let (mut n, mut sum, mut sq) = (0.0f32, 0.0f32, 0.0f32);
    for v in values {
        n += 1.0;
        sum += v;
        sq += v * v;
    }
    if n == 0.0 {
        return (0.0, 0.0);
    }
    let mean = sum / n;
    (mean, (sq / n - mean * mean).max(0.0).sqrt())
}

/// Saturation brings the mean chroma to the reference's; a black-and-white reference takes all
/// color away (the color balance can then tone it).
fn match_saturation(reference: &LookStats, now: &LookStats, out: &mut Adjustments, bounds: &Bounds) {
    if reference.monochrome() {
        out.saturation = -100.0;
        return;
    }
    if now.chroma < 0.004 {
        return;
    }
    // Partly: the mean chroma depends on what was photographed, too.
    let ratio = (reference.chroma / now.chroma).clamp(0.6, 1.6).powf(0.75 * bounds.color_share);
    let factor = (1.0 + out.saturation / 100.0) * ratio;
    out.saturation = ((factor - 1.0) * 100.0).clamp(-50.0, 50.0).round() + 0.0;
}

/// Each color editor band takes the reference's hue, chroma and lightness of that band, as far
/// as both pictures have enough of the band.
fn match_bands(reference: &LookStats, now: &LookStats, out: &mut Adjustments, bounds: &Bounds) {
    for (k, band) in out.color_editor.bands.iter_mut().enumerate() {
        let (r, t) = (reference.bands[k], now.bands[k]);
        let g = smoothstep(0.01, 0.05, r.share.min(t.share)) * bounds.color_share;
        if g <= 0.0 || t.chroma < 1e-4 || t.lightness < 1e-3 {
            continue;
        }
        // Not further than to the neighbouring bands.
        let dh = ((r.hue - t.hue + 540.0).rem_euclid(360.0) - 180.0) * 0.8 * g;
        band.hue = round1((band.hue + dh).clamp(-30.0, 30.0));
        let s = (1.0 + band.saturation / 100.0) * (r.chroma / t.chroma).clamp(0.5, 2.0).powf(g);
        band.saturation = ((s - 1.0) * 100.0).clamp(-60.0, 60.0).round() + 0.0;
        let l = (1.0 + 0.4 * band.lightness / 100.0) * (r.lightness / t.lightness).clamp(0.6, 1.6).powf(g);
        band.lightness = ((l - 1.0) / 0.4 * 100.0).clamp(-60.0, 60.0).round() + 0.0;
    }
}

/// The wheels of shadows, midtones and highlights take away the difference between the casts
/// of the nearly neutral colors in each. Solved together, as the zones overlap, with the effect
/// of each wheel measured (the curves after the color stage amplify it, in the shadows most).
fn match_color_balance(reference: &LookStats, mut now: LookStats, out: &mut Adjustments, measure: &Measure) -> LookStats {
    const PROBE: f32 = 0.15;
    for _ in 0..2 {
        let valid: [bool; 3] = std::array::from_fn(|i| reference.zones[i].share.min(now.zones[i].share) >= 0.02);
        let d: [[f32; 3]; 2] = std::array::from_fn(|c| std::array::from_fn(|i| if valid[i] { reference.zones[i].ab[c] - now.zones[i].ab[c] } else { 0.0 }));
        if d.iter().flatten().all(|v| v.abs() < 0.0015) {
            break;
        }
        let mut step = [[0.0f32; 3]; 2];
        // Each wheel nudged along a and b, all measured at the same time.
        let nudges: Vec<(usize, usize)> = (0..2).flat_map(|c| (0..3).filter(|j| valid[*j]).map(move |j| (c, j))).collect();
        let probes: Vec<Adjustments> = nudges
            .iter()
            .map(|&(c, j)| {
                let mut probe = out.clone();
                let w = wheels(&mut probe)[j].offset();
                let mut ab = [w[0], w[1]];
                ab[c] += PROBE;
                *wheels(&mut probe)[j] = wheel(ab, w[2] * 100.0);
                probe
            })
            .collect();
        let mut measured = nudges.into_iter().zip(measure_all(measure, &probes)).peekable();
        for c in 0..2 {
            let mut jac = [[0.0f32; 3]; 3];
            while let Some(((_, j), s)) = measured.next_if(|((pc, _), _)| *pc == c) {
                let Some(s) = s else { return now };
                for ((row, zs), zn) in jac.iter_mut().zip(&s.zones).zip(&now.zones) {
                    row[j] = (zs.ab[c] - zn.ab[c]) / PROBE;
                }
            }
            // Neighbouring zones see much the same of each wheel: damped, so that the wheels
            // do not work against each other.
            match solve_least_squares(&jac, &d[c], &valid, 0.05) {
                Some(x) => step[c] = x,
                None => return now,
            }
        }
        for (j, w) in wheels(out).into_iter().enumerate() {
            let o = w.offset();
            *w = wheel([o[0] + step[0][j].clamp(-0.5, 0.5), o[1] + step[1][j].clamp(-0.5, 0.5)], w.lightness);
        }
        match measure(out) {
            Some(s) => now = s,
            None => break,
        }
    }
    now
}

fn wheels(adj: &mut Adjustments) -> [&mut ColorWheel; 3] {
    let cb = &mut adj.color_balance;
    [&mut cb.shadows, &mut cb.midtones, &mut cb.highlights]
}

/// A wheel from its a/b offset (amount 0..1 in the hue's direction).
fn wheel(ab: [f32; 2], lightness: f32) -> ColorWheel {
    let amount = (ab[0] * ab[0] + ab[1] * ab[1]).sqrt() * 100.0;
    if amount < 0.05 {
        return ColorWheel { hue: 0.0, amount: 0.0, lightness };
    }
    ColorWheel { hue: ab[1].atan2(ab[0]).to_degrees().rem_euclid(360.0).round(), amount: round1(amount.min(100.0)), lightness }
}

fn quantile(sorted: &[f32], q: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let f = q.clamp(0.0, 1.0) * (sorted.len() - 1) as f32;
    let i = f as usize;
    let j = (i + 1).min(sorted.len() - 1);
    sorted[i] + (sorted[j] - sorted[i]) * (f - i as f32)
}

/// A table over 0..1 at `x`, linearly interpolated.
fn lut_at(lut: &[f32], x: f32) -> f32 {
    let f = x.clamp(0.0, 1.0) * (lut.len() - 1) as f32;
    let i = (f as usize).min(lut.len() - 2);
    lut[i] + (lut[i + 1] - lut[i]) * (f - i as f32)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn round1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0 + 0.0
}

fn round4(v: f32) -> f32 {
    (v * 1e4).round() / 1e4 + 0.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::CameraColor;
    use crate::image::Orientation;
    use crate::params::{ColorBalance, Detail};
    use crate::raw::CaptureInfo;

    fn meta() -> RawMeta {
        let xyz_to_cam = mat_inv(&crate::color::REC2020_TO_XYZ).unwrap();
        RawMeta {
            path: Default::default(),
            info: CaptureInfo { width: 240, height: 160, ..Default::default() },
            color: CameraColor::new(xyz_to_cam),
            as_shot_wb: [1.0; 3],
            orientation: Orientation::Normal,
            hdr: false,
            lens: None,
            lens_corrected: false, bayer: false,
        }
    }

    /// A scene with a range of brightness, grey areas and patches of several colors.
    fn scene(seed: u32) -> LinearImage {
        let (w, h) = (240, 160);
        let colors = [[0.9, 0.35, 0.2], [0.25, 0.6, 0.25], [0.2, 0.35, 0.9], [0.85, 0.7, 0.3], [0.6, 0.5, 0.45], [1.0, 1.0, 1.0]];
        let data = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let level = 0.002 * 400f32.powf(x as f32 / w as f32);
                let c = colors[(y / 27 + (x / 60) * seed as usize) % colors.len()];
                c.map(|v| v * level)
            })
            .collect();
        LinearImage::new(w, h, data)
    }

    fn plain() -> Adjustments {
        Adjustments { detail: Detail { sharpen_amount: 0.0, nr_color: 0.0, ..Default::default() }, ..Default::default() }
    }

    fn stats(meta: &RawMeta, adj: &Adjustments, img: &LinearImage) -> LookStats {
        let r = Processor::new(meta, adj).render_cpu(img, None, None);
        LookStats::from_rgba(r.width, r.height, &r.rgba, &RegionMasks::default()).unwrap()
    }

    fn matched(reference: &LookStats, meta: &RawMeta, adj: &Adjustments, img: &LinearImage, opts: LookOptions) -> Adjustments {
        match_look(std::slice::from_ref(reference), meta, adj, img, opts, &mut no_regions).unwrap().adjustments
    }

    /// How far two looks are apart: brightness quantiles, casts, chroma.
    fn distance(a: &LookStats, b: &LookStats) -> f32 {
        let tone: f32 = (1..10).map(|i| (a.quantile(i as f32 / 10.0) - b.quantile(i as f32 / 10.0)).abs()).sum::<f32>() / 9.0;
        let cast: f32 = (0..3).map(|i| (a.zones[i].ab[0] - b.zones[i].ab[0]).abs() + (a.zones[i].ab[1] - b.zones[i].ab[1]).abs()).sum::<f32>() * 3.0;
        tone + cast + (a.chroma - b.chroma).abs() * 3.0
    }

    fn graded() -> Adjustments {
        Adjustments {
            exposure: 0.6,
            white_balance: WhiteBalance { mode: WbMode::Custom, kelvin: 5200.0, tint: 6.0 },
            saturation: 30.0,
            curves: crate::params::Curves { luma: Curve { points: vec![[0.0, 0.08], [0.25, 0.22], [0.75, 0.82], [1.0, 0.95]] }, ..Default::default() },
            color_balance: ColorBalance {
                shadows: ColorWheel { hue: 230.0, amount: 30.0, lightness: 0.0 },
                highlights: ColorWheel { hue: 60.0, amount: 20.0, lightness: 0.0 },
                ..Default::default()
            },
            ..plain()
        }
    }

    #[test]
    fn takes_on_the_look_of_a_graded_picture() {
        let m = meta();
        // The reference: another scene, graded.
        let reference = stats(&m, &graded(), &scene(2));
        let target = scene(1);
        let before = stats(&m, &plain(), &target);
        let adj = matched(&reference, &m, &plain(), &target, LookOptions::default());
        let after = stats(&m, &adj, &target);
        let (d0, d1) = (distance(&reference, &before), distance(&reference, &after));
        assert!(d1 < d0 * 0.25, "Abstand vorher {d0}, nachher {d1}: {adj:?}");
        assert!(adj.exposure > 0.2, "Belichtung {}", adj.exposure);
        assert!(adj.saturation > 5.0, "Sättigung {}", adj.saturation);
        // Raised blacks of the reference.
        assert!(adj.curves.luma.build_lut(256)[0] > 0.03, "Schwarzpunkt {:?}", adj.curves.luma);
        // Cool shadows, warm highlights.
        let (s, h) = (adj.color_balance.shadows.offset(), adj.color_balance.highlights.offset());
        assert!(s[1] < h[1], "Schatten {:?}, Lichter {:?}", adj.color_balance.shadows, adj.color_balance.highlights);
    }

    #[test]
    fn its_own_look_changes_little() {
        let m = meta();
        let img = scene(1);
        let reference = stats(&m, &plain(), &img);
        let adj = matched(&reference, &m, &plain(), &img, LookOptions::default());
        assert!(adj.exposure.abs() <= 0.05, "{adj:?}");
        assert!(adj.saturation.abs() <= 3.0, "{adj:?}");
        let lut = adj.curves.luma.build_lut(64);
        assert!(lut.iter().enumerate().all(|(i, v)| (v - i as f32 / 63.0).abs() < 0.03), "{:?}", adj.curves.luma);
        for b in adj.color_editor.bands {
            assert!(b.hue.abs() <= 3.0 && b.saturation.abs() <= 5.0 && b.lightness.abs() <= 5.0, "{b:?}");
        }
        for w in [adj.color_balance.shadows, adj.color_balance.midtones, adj.color_balance.highlights] {
            assert!(w.amount <= 3.0, "{w:?}");
        }
    }

    #[test]
    fn a_series_under_other_light_gets_the_same_brightness_and_cast() {
        let m = meta();
        let warm = Adjustments { exposure: 0.5, white_balance: WhiteBalance { mode: WbMode::Custom, kelvin: 5000.0, tint: 5.0 }, ..plain() };
        let img = scene(1);
        let reference = stats(&m, &warm, &img);
        // The same scene, a stop darker and under reddish light.
        let other = LinearImage::new(img.width, img.height, img.data.iter().map(|p| [p[0] * 0.5, p[1] * 0.3, p[2] * 0.3]).collect());
        let opts = LookOptions { exposure: true, white_balance: true, tone: false, colors: false, color_balance: false };
        let adj = matched(&reference, &m, &plain(), &other, opts);
        let after = stats(&m, &adj, &other);
        assert!((after.quantile(0.5) - reference.quantile(0.5)).abs() < 0.01, "{adj:?}");
        assert!((after.neutral[0] - reference.neutral[0]).abs() < 0.003 && (after.neutral[1] - reference.neutral[1]).abs() < 0.003, "{adj:?}");
        assert!(adj.exposure > 1.0, "{adj:?}");
    }

    #[test]
    fn a_black_and_white_reference_takes_the_color_away() {
        let m = meta();
        let bw = Adjustments { saturation: -100.0, ..plain() };
        let reference = stats(&m, &bw, &scene(2));
        assert!(reference.monochrome());
        let adj = matched(&reference, &m, &plain(), &scene(1), LookOptions::default());
        assert_eq!(adj.saturation, -100.0);
        assert!(!stats(&m, &plain(), &scene(1)).monochrome());

        // Toned (sepia): also black and white, the toning comes from the color balance.
        let sepia = ColorBalance { highlights: ColorWheel { hue: 75.0, amount: 35.0, lightness: 0.0 }, ..Default::default() };
        let reference = stats(&m, &Adjustments { color_balance: sepia, ..bw }, &scene(2));
        assert!(reference.monochrome(), "Streuung {}", reference.spread);
        let adj = matched(&reference, &m, &plain(), &scene(1), LookOptions::default());
        assert_eq!(adj.saturation, -100.0);
        let h = adj.color_balance.highlights.offset();
        assert!(h[1] > 0.1, "{:?}", adj.color_balance);
    }

    /// The scene turned on its side: the same colors, another composition.
    fn other_scene() -> LinearImage {
        let img = scene(2);
        let (w, h) = (img.width, img.height);
        LinearImage::new(w, h, (0..w * h).map(|i| img.data[(i % w) * h / w * w + (i / w) * w / h]).collect())
    }

    #[test]
    fn the_similar_reference_sets_the_look() {
        let m = meta();
        let target = scene(1);
        let warm = Adjustments { white_balance: WhiteBalance { mode: WbMode::Custom, kelvin: 4200.0, tint: 0.0 }, ..plain() };
        let cool = Adjustments { white_balance: WhiteBalance { mode: WbMode::Custom, kelvin: 8000.0, tint: 0.0 }, ..plain() };
        // The same scene, cool; another scene, warm.
        let refs = [stats(&m, &cool, &target), stats(&m, &warm, &other_scene())];
        let mine = stats(&m, &plain(), &target);
        assert!(similarity(&refs[0], &mine) > 0.8, "{}", similarity(&refs[0], &mine));
        assert!(similarity(&refs[1], &mine) < 0.5, "{}", similarity(&refs[1], &mine));
        let result = match_look(&refs, &m, &plain(), &target, LookOptions::default(), &mut no_regions).unwrap();
        assert!(result.weights[0] > 0.8, "{:?}", result.weights);
        assert!((result.weights.iter().sum::<f32>() - 1.0).abs() < 1e-4);
        // Cooler than as shot (5500 K or so), as the similar reference.
        assert!(result.adjustments.white_balance.kelvin > 6000.0, "{:?}", result.adjustments.white_balance);
    }

    #[test]
    fn sky_is_left_out() {
        let m = meta();
        let img = scene(1);
        let r = Processor::new(&m, &plain()).render_cpu(&img, None, None);
        // The top third is sky.
        let sky = Mask8 { width: 3, height: 3, data: vec![255, 255, 255, 0, 0, 0, 0, 0, 0] };
        let all = LookStats::from_rgba(r.width, r.height, &r.rgba, &RegionMasks::default()).unwrap();
        let ground = LookStats::from_rgba(r.width, r.height, &r.rgba, &RegionMasks { sky: Some(sky), subject: None }).unwrap();
        assert!((ground.sky - 1.0 / 3.0).abs() < 0.02, "{}", ground.sky);
        assert!(ground.luma.len() < all.luma.len() * 3 / 4);
    }

    #[test]
    fn only_what_is_chosen_changes() {
        let m = meta();
        let reference = stats(&m, &graded(), &scene(2));
        let opts = LookOptions { exposure: true, white_balance: false, tone: false, colors: false, color_balance: false };
        let adj = matched(&reference, &m, &plain(), &scene(1), opts);
        assert!(adj.exposure > 0.2);
        assert_eq!(Adjustments { exposure: 0.0, ..adj }, plain());
    }

    #[test]
    fn strength_blends_between_before_and_matched() {
        let m = meta();
        let reference = stats(&m, &graded(), &scene(2));
        let before = plain();
        let matched = matched(&reference, &m, &before, &scene(1), LookOptions::default());
        let opts = LookOptions::default();
        let as_shot = m.color.temp_tint_for_wb(m.as_shot_wb);
        let zero = opts.blend(&matched, &before, &matched, 0.0, as_shot);
        assert_eq!(zero.exposure, before.exposure);
        assert_eq!(zero.saturation, before.saturation);
        assert!(zero.curves.luma.is_identity());
        assert!(zero.color_balance.is_neutral(), "{:?}", zero.color_balance);
        assert_eq!(opts.blend(&before, &before, &matched, 1.0, as_shot), matched);
        let half = opts.blend(&matched, &before, &matched, 0.5, as_shot);
        assert!((half.exposure - matched.exposure / 2.0).abs() < 0.011);
    }

    #[test]
    fn reads_a_picture_file() {
        let dir = std::env::temp_dir().join(format!("sk-look-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("film.png");
        image::RgbImage::from_fn(64, 48, |x, y| image::Rgb([(x * 4) as u8, (y * 5) as u8, 90])).save(&path).unwrap();
        let s = LookStats::of_file(&path, &mut no_regions);
        std::fs::remove_dir_all(&dir).ok();
        let s = s.unwrap();
        assert!(s.quantile(0.9) > s.quantile(0.1));
    }
}
