//! Automatic adjustments: white balance, exposure, matching images to a reference, and the
//! one-click auto adjustment that optimizes several settings together.

use serde::{Deserialize, Serialize};

use crate::detail::Plane;
use crate::image::LinearImage;
use crate::params::{Adjustments, Levels, WbMode, WhiteBalance};
use crate::pipeline::{Histogram, Processor, Rendered, StageCache};
use crate::raw::RawMeta;

/// White balance from the nearly neutral pixels of the developed (cropped) image (see
/// [`smart_white_balance`]); the camera's if there aren't enough usable pixels.
pub fn auto_white_balance(meta: &RawMeta, adj: &Adjustments, preview: &LinearImage) -> WhiteBalance {
    let src = reduced(preview, MEASURE_EDGE);
    smart_white_balance(meta, &camera_samples(meta, &measuring(adj), &src), &Focus::default()).unwrap_or_default()
}

/// Luminance of every `step`-th pixel in linear working space, with the image's white balance
/// (not its exposure).
fn luminances(meta: &RawMeta, wb: &WhiteBalance, preview: &LinearImage, step: usize) -> Vec<f32> {
    let wb = match wb.mode {
        WbMode::AsShot => meta.as_shot_wb,
        WbMode::Custom => meta.color.wb_for_temp_tint(wb.kelvin, wb.tint),
    };
    let m = &meta.color.cam_to_working;
    preview
        .data
        .iter()
        .step_by(step)
        .map(|p| {
            let b = [p[0] * wb[0], p[1] * wb[1], p[2] * wb[2]];
            let w = crate::color::mat_vec(m, b);
            (0.2627 * w[0] + 0.678 * w[1] + 0.0593 * w[2]).max(1e-5)
        })
        .collect()
}

/// Exposure (EV) that brings the scene's log-average luminance to a pleasant level,
/// limited so that the brightest 0.5 % do not end up far beyond the highlight shoulder.
pub fn auto_exposure(meta: &RawMeta, adj: &Adjustments, preview: &LinearImage) -> f32 {
    let mut lum = luminances(meta, &adj.white_balance, preview, 2);
    if lum.is_empty() {
        return 0.0;
    }
    let log_mean = lum.iter().map(|v| v.ln()).sum::<f32>() / lum.len() as f32;
    let mut ev = (0.16f32.ln() - log_mean) / std::f32::consts::LN_2;
    let idx = (((lum.len() as f32) * 0.995) as usize).min(lum.len() - 1);
    let (_, p995, _) = lum.select_nth_unstable_by(idx, |a, b| a.total_cmp(b));
    let p995 = *p995;
    // The film curve rolls off around 3-4x middle grey; keep the top highlights below ~4.
    let max_ev = (4.0f32 / p995.max(1e-4)).log2();
    ev = ev.min(max_ev.max(-1.0));
    (ev.clamp(-3.0, 3.0) * 20.0).round() / 20.0
}

// ------------------------------------------------------------------ one-click auto

/// What the one-click auto adjustment sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoOptions {
    pub white_balance: bool,
    pub exposure: bool,
    /// Highlights, shadows, whites and blacks (levels go back to neutral).
    pub tone: bool,
    pub contrast: bool,
    pub vibrance: bool,
}

impl Default for AutoOptions {
    fn default() -> Self {
        Self { white_balance: true, exposure: true, tone: true, contrast: true, vibrance: true }
    }
}

impl AutoOptions {
    pub fn any(&self) -> bool {
        self.white_balance || self.exposure || self.tone || self.contrast || self.vibrance
    }

    /// `current` with the settings these options cover `t` of the way from `before` to `auto`
    /// (0..1: the strength of the auto adjustment). The white balance blends in mired (as the eye
    /// sees steps of color temperature), `as_shot` (kelvin, tint) standing in for "as shot".
    pub fn blend(&self, current: &Adjustments, before: &Adjustments, auto: &Adjustments, t: f32, as_shot: (f32, f32)) -> Adjustments {
        let lerp = |a: f32, b: f32| a + (b - a) * t;
        let mut out = current.clone();
        if self.white_balance {
            out.white_balance = blend_white_balance(&before.white_balance, &auto.white_balance, t, as_shot);
        }
        if self.exposure {
            out.exposure = (lerp(before.exposure, auto.exposure) * 100.0).round() / 100.0 + 0.0;
        }
        let whole = |a: f32, b: f32| lerp(a, b).round() + 0.0;
        if self.tone {
            out.highlights = whole(before.highlights, auto.highlights);
            out.shadows = whole(before.shadows, auto.shadows);
            out.whites = whole(before.whites, auto.whites);
            out.blacks = whole(before.blacks, auto.blacks);
            let (a, b) = (&before.levels, &auto.levels);
            out.levels = Levels {
                rgb: a.rgb.lerp(&b.rgb, t),
                red: a.red.lerp(&b.red, t),
                green: a.green.lerp(&b.green, t),
                blue: a.blue.lerp(&b.blue, t),
            };
        }
        if self.contrast {
            out.contrast = whole(before.contrast, auto.contrast);
        }
        if self.vibrance {
            out.vibrance = whole(before.vibrance, auto.vibrance);
        }
        out
    }

    /// `onto` with the settings these options cover taken from `auto`.
    pub fn take(&self, auto: &Adjustments, onto: &Adjustments) -> Adjustments {
        let mut out = onto.clone();
        // The camera's look, if the auto adjustment was made on it (see `camera_look`).
        if auto.camera_curve.is_some() && onto.camera_curve.is_none() && crate::camera_look::wanted(onto, true) {
            (out.base_curve, out.camera_curve) = (auto.base_curve, auto.camera_curve.clone());
        }
        if self.white_balance {
            out.white_balance = auto.white_balance.clone();
        }
        if self.exposure {
            out.exposure = auto.exposure;
        }
        if self.tone {
            (out.highlights, out.shadows, out.whites, out.blacks) = (auto.highlights, auto.shadows, auto.whites, auto.blacks);
            out.levels = auto.levels.clone();
        }
        if self.contrast {
            out.contrast = auto.contrast;
        }
        if self.vibrance {
            out.vibrance = auto.vibrance;
        }
        out
    }
}

/// The white balance `t` of the way from `before` to `after`, in mired (as the eye sees steps of
/// color temperature); `as_shot` (kelvin, tint) stands in for "as shot".
pub fn blend_white_balance(before: &WhiteBalance, after: &WhiteBalance, t: f32, as_shot: (f32, f32)) -> WhiteBalance {
    if t <= 0.0 {
        return before.clone();
    }
    if t >= 1.0 {
        return after.clone();
    }
    let kt = |wb: &WhiteBalance| match wb.mode {
        WbMode::AsShot => as_shot,
        WbMode::Custom => (wb.kelvin, wb.tint),
    };
    let ((k0, t0), (k1, t1)) = (kt(before), kt(after));
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    let kelvin = 1e6 / lerp(1e6 / k0, 1e6 / k1);
    WhiteBalance { mode: WbMode::Custom, kelvin: kelvin.round(), tint: (lerp(t0, t1) * 10.0).round() / 10.0 }
}

/// Where the important parts of an image are, in normalized coordinates of the developed
/// (cropped) image: found by the face detector and the subject model, if available.
#[derive(Clone, Debug, Default)]
pub struct Focus {
    /// x, y, width, height (0..1).
    pub faces: Vec<[f32; 4]>,
    /// Probability of belonging to the main subject (0..1), stretched over the whole image.
    pub subject: Option<Plane>,
}

impl Focus {
    fn in_face(&self, u: f32, v: f32) -> bool {
        self.faces.iter().any(|f| u >= f[0] && u <= f[0] + f[2] && v >= f[1] && v <= f[1] + f[3])
    }

    fn subject_at(&self, u: f32, v: f32) -> f32 {
        self.subject.as_ref().map_or(0.0, |p| {
            let x = ((u * p.w as f32) as usize).min(p.w - 1);
            let y = ((v * p.h as f32) as usize).min(p.h - 1);
            p.data[y * p.w + x]
        })
    }

    /// Share of the image covered by faces.
    fn face_area(&self) -> f32 {
        self.faces.iter().map(|f| f[2] * f[3]).sum::<f32>().min(1.0)
    }
}

/// Long edge of the images the optimization renders (statistics need few pixels).
const MEASURE_EDGE: usize = 300;
/// Long edge of the image for the face detector and the subject model.
pub const GUIDE_EDGE: usize = 640;

/// `adj` without what is slow or irrelevant for measuring tones (noise reduction, AI, local
/// layers, retouching, sharpening, grain).
fn measuring(adj: &Adjustments) -> Adjustments {
    let mut a = adj.clone();
    a.detail.ai_denoise = 0.0;
    a.detail.nr_luma = 0.0;
    a.detail.nr_color = 0.0;
    a.detail.sharpen_amount = 0.0;
    a.detail.grain_amount = 0.0;
    a.detail.glow_amount = 0.0;
    a.layers.clear();
    a.retouch.clear();
    a.removals.clear();
    a.lens_blur = Default::default();
    a.relight = Default::default();
    a.atmosphere = Default::default();
    a
}

/// What the histogram of the levels depends on: `adj` as measured, with neutral levels and curves.
/// Equal for two settings, equal histograms (so dragging the levels doesn't measure again).
pub fn levels_input_settings(adj: &Adjustments) -> Adjustments {
    let mut a = measuring(adj);
    a.levels = Levels::default();
    a.curves = Default::default();
    a
}

/// Histogram of what goes into the levels (encoded working RGB, as the levels see it), measured
/// on a reduced `preview` with `settings` from [`levels_input_settings`]. Unlike the histogram of
/// the rendering, it shows what the black and white points cut off.
pub fn levels_input_histogram(meta: &RawMeta, settings: &Adjustments, preview: &LinearImage) -> Histogram {
    let src = reduced(preview, 400);
    let img = Processor::new(meta, settings).encoded_cpu(&src, None);
    let mut h = Histogram { r: vec![0; 256], g: vec![0; 256], b: vec![0; 256], luma: vec![0; 256] };
    let bin = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as usize;
    for p in &img.data {
        h.r[bin(p[0])] += 1;
        h.g[bin(p[1])] += 1;
        h.b[bin(p[2])] += 1;
        // Weights as in the display histogram.
        h.luma[bin((p[0] * 54.0 + p[1] * 183.0 + p[2] * 19.0) / 256.0)] += 1;
    }
    h
}

fn reduced(preview: &LinearImage, edge: usize) -> LinearImage {
    let k = edge as f32 / preview.width.max(preview.height) as f32;
    if k >= 1.0 {
        return preview.clone();
    }
    preview.resize_area(((preview.width as f32 * k).round() as usize).max(1), ((preview.height as f32 * k).round() as usize).max(1))
}

/// The image as the face detector and the subject model see it: cropped like `adj`, with a
/// reasonable exposure.
pub fn guide_image(meta: &RawMeta, adj: &Adjustments, preview: &LinearImage, opts: &AutoOptions) -> Rendered {
    let mut a = measuring(adj);
    if opts.exposure {
        a.exposure = auto_exposure(meta, &a, preview);
    }
    Processor::new(meta, &a).render_cpu(&reduced(preview, GUIDE_EDGE), None, None)
}

/// Camera values of the developed (cropped) image on a grid, with their normalized position.
fn camera_samples(meta: &RawMeta, adj: &Adjustments, src: &LinearImage) -> Vec<([f32; 3], f32, f32)> {
    let geo = Processor::new(meta, adj).geomap(src.width, src.height);
    let (w, h) = (geo.out_w, geo.out_h);
    let step = ((w * h) as f32 / 40_000.0).sqrt().max(1.0);
    let mut out = Vec::new();
    let mut y = step / 2.0;
    while y < h as f32 {
        let mut x = step / 2.0;
        while x < w as f32 {
            let (sx, sy) = geo.source_point(x, y);
            if sx >= 0.0 && sy >= 0.0 && sx < src.width as f32 && sy < src.height as f32 {
                out.push((src.data[sy as usize * src.width + sx as usize], x / w as f32, y / h as f32));
            }
            x += step;
        }
        y += step;
    }
    out
}

const MIRED_COOL: f32 = 1e6 / 6500.0;
const MIRED_WARM: f32 = 1e6 / 4300.0;

/// White balance of the developed image from its nearly neutral pixels (stones, water, white
/// walls, clouds): starting from the camera's white balance, pixels within a shrinking distance
/// of neutral move the estimate until it settles. Brighter pixels count more (their color is
/// more reliable), clipped ones, nearly black ones and faces not at all. Unlike the grey world,
/// large colored areas (forest, wood) don't pull it away. Only if the camera's white balance is
/// so far off that hardly anything looks neutral (a daylight preset under tungsten light), the
/// search starts from the grey world instead. Warm and cool light keep part of their mood (a
/// sunset stays warm).
fn smart_white_balance(meta: &RawMeta, samples: &[([f32; 3], f32, f32)], focus: &Focus) -> Option<WhiteBalance> {
    let usable: Vec<[f32; 3]> = samples
        .iter()
        .filter(|(p, u, v)| {
            let max = p[0].max(p[1]).max(p[2]);
            max < 0.95 && p[1] > 0.01 && !focus.in_face(*u, *v)
        })
        .map(|s| s.0)
        .collect();
    if usable.len() < 200 {
        return None;
    }
    // White balances as the log of the red and blue multipliers relative to green.
    let log_wb = |wb: [f32; 3]| [(wb[0] / wb[1]).ln(), (wb[2] / wb[1]).ln()];
    let shot = log_wb(meta.as_shot_wb);
    let from_camera = neutral_search(&usable, shot);
    let mean = usable.iter().fold([0.0f32; 3], |a, p| [a[0] + p[0], a[1] + p[1], a[2] + p[2]]);
    let from_grey = neutral_search(&usable, log_wb([mean[1] / mean[0].max(1e-6), 1.0, mean[1] / mean[2].max(1e-6)]));
    let camera_start = from_camera.1 >= 0.03 || from_camera.1 * 1.5 >= from_grey.1;
    let (wb, share, start) = if camera_start { (from_camera.0, from_camera.1, shot) } else { (from_grey.0, from_grey.1, shot) };
    // The fewer pixels are that neutral in the end, the more the camera's white balance stays.
    // Even with many, it moves only part of the way from the camera's: in mixed light (sun and
    // blue shade) which pixels are neutral is ambiguous, and the camera's is a good guess.
    let trust = smoothstep(0.03, 0.2, share) * if camera_start { 0.6 } else { 1.0 };
    let wb = [start[0] + (wb[0] - start[0]) * trust, start[1] + (wb[1] - start[1]) * trust];
    let (kelvin, mut tint) = meta.color.temp_tint_for_wb([wb[0].exp(), 1.0, wb[1].exp()]);
    if camera_start {
        // Light sources rarely lie far off the daylight/tungsten line: the tint moves away from
        // the camera's only a little (green foliage and magenta flowers fool the estimate there).
        let shot_tint = meta.color.temp_tint_for_wb(meta.as_shot_wb).1;
        tint = shot_tint + (tint - shot_tint).clamp(-12.0, 12.0);
    }
    let mut mired = 1e6 / kelvin;
    // Keep part of the mood: correct warm and cool light only partly beyond the neutral range.
    if mired > MIRED_WARM {
        mired -= (mired - MIRED_WARM) * 0.35;
    } else if mired < MIRED_COOL {
        mired += (MIRED_COOL - mired) * 0.25;
    }
    let kelvin = (1e6 / mired).clamp(2000.0, 15000.0);
    Some(WhiteBalance { mode: WbMode::Custom, kelvin: kelvin.round(), tint: (tint.clamp(-80.0, 80.0) * 10.0).round() / 10.0 })
}

/// The white balance (log red and blue multipliers) that makes the nearly neutral pixels around
/// `start` neutral, and the share of pixels that are neutral then.
fn neutral_search(usable: &[[f32; 3]], start: [f32; 2]) -> ([f32; 2], f32) {
    let (mut wb, mut share) = (start, 0.0);
    for radius in [0.4f32, 0.3, 0.2, 0.14, 0.1, 0.1] {
        let (mut acc, mut sw, mut n) = ([0.0f32; 2], 0.0f32, 0usize);
        for p in usable {
            let g = p[1].max(1e-6);
            let (r, b) = ((p[0] / g).max(1e-6).ln() + wb[0], (p[2] / g).max(1e-6).ln() + wb[1]);
            if r * r + b * b < radius * radius {
                let w = p[0] + p[1] + p[2];
                acc[0] += r * w;
                acc[1] += b * w;
                sw += w;
                n += 1;
            }
        }
        if n < usable.len() / 50 || n < 50 {
            break;
        }
        wb = [wb[0] - acc[0] / sw, wb[1] - acc[1] / sw];
        share = n as f32 / usable.len() as f32;
    }
    (wb, share)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Statistics of a rendering that the auto adjustment judges.
struct Judged {
    /// Mean of the (sRGB-encoded) luma: half plain, half weighted by [`weights`] (the subject
    /// matters more, but a dark subject should not brighten the whole image as far as it could).
    mean: f32,
    plain_mean: f32,
    /// Mean luma of the faces.
    face_mean: Option<f32>,
    /// Weighted share of pixels with a clipped channel, and of nearly black ones.
    clipped: f32,
    crushed: f32,
    /// Luma percentiles 0.5 % and 99.5 %.
    p_lo: f32,
    p_hi: f32,
    /// Standard deviation of the luma.
    spread: f32,
}

/// Per-pixel weights of a `w` x `h` rendering: subjects count more, faces most.
fn weights(focus: &Focus, w: usize, h: usize) -> (Vec<f32>, Vec<bool>) {
    let mut wt = Vec::with_capacity(w * h);
    let mut face = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
            let f = focus.in_face(u, v);
            wt.push(1.0 + 2.0 * focus.subject_at(u, v) + if f { 6.0 } else { 0.0 });
            face.push(f);
        }
    }
    (wt, face)
}

/// Judges an encoded rendering (see `Processor::encoded_cpu`).
fn judge(img: &LinearImage, wt: &[f32], face: &[bool]) -> Judged {
    let px = &img.data;
    let mut luma = Vec::with_capacity(px.len());
    let (mut sw, mut sum, mut clipped, mut crushed) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    let (mut fsum, mut fn_) = (0.0f32, 0usize);
    for (i, p) in px.iter().enumerate() {
        let l = 0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2];
        luma.push(l);
        let w = wt[i];
        sw += w;
        sum += w * l;
        let max = p[0].max(p[1]).max(p[2]);
        if max >= 0.98 {
            clipped += w;
        }
        if max <= 0.012 {
            crushed += w;
        }
        if face[i] {
            fsum += l;
            fn_ += 1;
        }
    }
    let n = luma.len().max(1);
    let plain_mean = luma.iter().sum::<f32>() / n as f32;
    let spread = (luma.iter().map(|l| (l - plain_mean).powi(2)).sum::<f32>() / n as f32).sqrt();
    let lo = (n as f32 * 0.005) as usize;
    let hi = ((n as f32 * 0.995) as usize).min(n - 1);
    let p_lo = *luma.select_nth_unstable_by(lo, |a, b| a.total_cmp(b)).1;
    let p_hi = *luma.select_nth_unstable_by(hi, |a, b| a.total_cmp(b)).1;
    Judged {
        mean: 0.5 * (sum / sw.max(1e-6) + plain_mean),
        plain_mean,
        face_mean: (fn_ > 0).then(|| fsum / fn_ as f32),
        clipped: clipped / sw.max(1e-6),
        crushed: crushed / sw.max(1e-6),
        p_lo,
        p_hi,
        spread,
    }
}

/// The settings the optimization varies, with their range and first step.
#[derive(Clone, Copy)]
enum Knob {
    Exposure,
    Highlights,
    Shadows,
    Whites,
    Blacks,
    Contrast,
}

impl Knob {
    fn range(self) -> (f32, f32, f32) {
        match self {
            Knob::Exposure => (-3.0, 3.0, 0.4),
            Knob::Highlights => (0.0, 90.0, 20.0),
            Knob::Shadows => (0.0, 80.0, 20.0),
            Knob::Whites => (-50.0, 50.0, 15.0),
            Knob::Blacks => (-50.0, 40.0, 15.0),
            Knob::Contrast => (-30.0, 50.0, 12.0),
        }
    }

    fn get(self, a: &Adjustments) -> f32 {
        match self {
            Knob::Exposure => a.exposure,
            Knob::Highlights => a.highlights,
            Knob::Shadows => a.shadows,
            Knob::Whites => a.whites,
            Knob::Blacks => a.blacks,
            Knob::Contrast => a.contrast,
        }
    }

    fn set(self, a: &mut Adjustments, v: f32) {
        let (lo, hi, _) = self.range();
        let v = v.clamp(lo, hi);
        match self {
            Knob::Exposure => a.exposure = v,
            Knob::Highlights => a.highlights = v,
            Knob::Shadows => a.shadows = v,
            Knob::Whites => a.whites = v,
            Knob::Blacks => a.blacks = v,
            Knob::Contrast => a.contrast = v,
        }
    }

    /// Cost of moving away from neutral: auto should not look overdone.
    fn cost(self, v: f32) -> f32 {
        match self {
            Knob::Exposure => 0.0,
            Knob::Highlights => (v / 70.0).powi(2),
            Knob::Shadows => (v / 60.0).powi(2),
            Knob::Whites | Knob::Blacks => (v / 45.0).powi(2),
            Knob::Contrast => (v / 40.0).powi(2),
        }
    }
}

/// How far a rendering is from a well developed image (smaller is better).
fn badness(j: &Judged, target: f32, face_area: f32) -> f32 {
    let mut e = ((j.mean - target) / 0.04).powi(2);
    if let Some(f) = j.face_mean {
        // A range, not a target: skin tones differ. Faces should be neither dark nor blown out.
        let k = (face_area / 0.01).min(1.0) * 2.0;
        e += k * ((0.36 - f).max(0.0) / 0.04).powi(2) + k * ((f - 0.8).max(0.0) / 0.04).powi(2);
    }
    // Specular highlights and the sun may clip, large areas should not.
    e += ((j.clipped - 0.004).max(0.0) / 0.006).powi(2);
    e += ((j.crushed - 0.008).max(0.0) / 0.015).powi(2);
    // Use the tonal range, but only against flat, grey images: a dull, moody scene need not get
    // white highlights (that only brightens it).
    e += ((0.82 - j.p_hi).max(0.0) / 0.06).powi(2) * 0.5;
    e += ((j.p_lo - 0.07).max(0.0) / 0.04).powi(2) * 0.7;
    e += ((j.spread - 0.2) / 0.06).powi(2) * 0.3;
    e
}

/// Mean (sRGB-encoded) luma a well developed image has without a reference.
const TARGET: f32 = 0.42;

/// Mean (sRGB-encoded) luma of the camera's own JPEG embedded in the raw file, the brightness
/// the camera aimed at. `None` for files without one, and for merged images (their preview is
/// Silberkorn's).
pub fn camera_brightness(meta: &RawMeta) -> Option<f32> {
    if meta.hdr {
        return None;
    }
    let img = crate::raw::embedded_preview_at_least(&meta.path, 320).ok()?.thumbnail(320, 320).to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let px = img.as_raw().as_chunks::<3>().0;
    // Some previews have black bars (another aspect ratio than the sensor): left out.
    let black = |p: &[u8; 3]| p[0].max(p[1]).max(p[2]) <= 4;
    let row_black = |y: usize| px[y * w..(y + 1) * w].iter().all(black);
    let col_black = |x: usize| (0..h).all(|y| black(&px[y * w + x]));
    let (mut y0, mut y1, mut x0, mut x1) = (0, h, 0, w);
    while y0 < y1 && row_black(y0) {
        y0 += 1;
    }
    while y1 > y0 && row_black(y1 - 1) {
        y1 -= 1;
    }
    while x0 < x1 && col_black(x0) {
        x0 += 1;
    }
    while x1 > x0 && col_black(x1 - 1) {
        x1 -= 1;
    }
    if x1 - x0 < 64 || y1 - y0 < 64 {
        return None;
    }
    let mut sum = 0.0f64;
    for y in y0..y1 {
        for p in &px[y * w + x0..y * w + x1] {
            sum += (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) as f64 / 255.0;
        }
    }
    Some((sum / ((x1 - x0) * (y1 - y0)) as f64) as f32)
}

/// One-click auto adjustment (white balance, exposure, highlights, shadows, whites, blacks,
/// contrast, vibrance), as far as `opts` allow.
///
/// Instead of deriving each setting from the histogram on its own, the settings are optimized
/// together: a small rendering through the real pipeline is judged for the brightness of the
/// midtones, clipping, the use of the tonal range and contrast, and the settings are varied
/// (coordinate descent) until it is as good as it gets without overdoing any of them. Faces and
/// the main subject (`focus`) count more than the rest of the image. The brightness aimed at
/// follows the camera's own JPEG (`camera`, see [`camera_brightness`]) where there is one.
pub fn auto_adjust(meta: &RawMeta, adj: &Adjustments, preview: &LinearImage, opts: &AutoOptions, focus: &Focus, camera: Option<f32>) -> Adjustments {
    let src = reduced(preview, MEASURE_EDGE);
    let mut a = measuring(adj);
    if opts.white_balance
        && let Some(wb) = smart_white_balance(meta, &camera_samples(meta, &a, &src), focus)
    {
        a.white_balance = wb;
    }
    if opts.tone {
        (a.highlights, a.shadows, a.whites, a.blacks) = (0.0, 0.0, 0.0, 0.0);
        a.levels = Levels::default();
    }
    if opts.contrast {
        a.contrast = 0.0;
    }

    let mut knobs = Vec::new();
    if opts.exposure {
        knobs.push(Knob::Exposure);
    }
    if opts.tone {
        knobs.extend([Knob::Highlights, Knob::Shadows, Knob::Whites, Knob::Blacks]);
    }
    if opts.contrast {
        knobs.push(Knob::Contrast);
    }
    if !knobs.is_empty() {
        optimize_tone(meta, &mut a, &src, focus, &knobs, opts.exposure, camera);
    }
    if opts.vibrance {
        a.vibrance = auto_vibrance(meta, &a, &src, focus);
    }
    let mut out = opts.take(&a, adj);
    out.exposure = (out.exposure * 20.0).round() / 20.0 + 0.0;
    for v in [&mut out.highlights, &mut out.shadows, &mut out.whites, &mut out.blacks, &mut out.contrast, &mut out.vibrance] {
        *v = v.round();
    }
    out
}

fn optimize_tone(meta: &RawMeta, a: &mut Adjustments, src: &LinearImage, focus: &Focus, knobs: &[Knob], exposure: bool, camera: Option<f32>) {
    if exposure {
        a.exposure = auto_exposure(meta, a, src);
    }
    let cache = StageCache::default();
    let render = |a: &Adjustments| Processor::new(meta, a).encoded_cpu(src, Some(&cache));
    let first = render(a);
    let (wt, face) = weights(focus, first.width, first.height);
    // How bright the image should be: like the camera's own JPEG (the maker's tuning, the
    // photographer's exposure compensation; a forest stays darker than a beach), only pulled a
    // little towards the middle. Without one, the brightness as metered says some of that.
    let target = match camera {
        Some(c) => (c + (TARGET - c) * 0.15).clamp(0.24, 0.58),
        None => {
            let mut as_shot = a.clone();
            as_shot.exposure = 0.0;
            let metered = judge(&render(&as_shot), &wt, &face).plain_mean;
            (TARGET + (metered - TARGET) * 0.4).clamp(0.32, 0.52)
        }
    };
    let face_area = focus.face_area();
    let score = |a: &Adjustments| {
        let j = judge(&render(a), &wt, &face);
        badness(&j, target, face_area) + knobs.iter().map(|k| k.cost(k.get(a))).sum::<f32>()
    };
    let mut best = score(a);
    let mut steps: Vec<f32> = knobs.iter().map(|k| k.range().2).collect();
    let mut evals = 0;
    while evals < 160 {
        let mut improved = false;
        for (k, step) in knobs.iter().zip(&steps) {
            for dir in [1.0, -1.0] {
                let mut t = a.clone();
                k.set(&mut t, k.get(a) + dir * step);
                if k.get(&t) == k.get(a) {
                    continue;
                }
                let s = score(&t);
                evals += 1;
                if s < best - 1e-4 {
                    (best, *a) = (s, t);
                    improved = true;
                    break;
                }
            }
        }
        if !improved {
            steps.iter_mut().for_each(|s| *s *= 0.5);
            if steps.iter().zip(knobs).all(|(s, k)| *s < k.range().2 / 10.0) {
                break;
            }
        }
    }
}

/// Vibrance for images with few colors: more for dull images, nothing for colorful ones, less
/// with faces (vibrance also strengthens skin).
fn auto_vibrance(meta: &RawMeta, a: &Adjustments, src: &LinearImage, focus: &Focus) -> f32 {
    if a.black_white.enabled {
        return 0.0;
    }
    let mut t = a.clone();
    t.vibrance = 0.0;
    let r = Processor::new(meta, &t).render_cpu(src, None, None);
    let to_working = crate::color::mat_inv(&crate::color::OutputSpace::Srgb.from_working()).unwrap_or_default();
    let ok = crate::color::Oklab::default();
    let px = r.rgba.as_chunks::<4>().0;
    let chroma: f32 = px
        .iter()
        .map(|p| {
            let lin = [p[0], p[1], p[2]].map(|v| crate::color::srgb_decode(v as f32 / 255.0));
            let [_, a, b] = ok.from_linear(crate::color::mat_vec(&to_working, lin));
            (a * a + b * b).sqrt()
        })
        .sum::<f32>()
        / px.len().max(1) as f32;
    let v = ((0.075 - chroma) / 0.05 * 35.0).clamp(0.0, 30.0);
    if focus.faces.is_empty() { v } else { v * 0.5 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::CameraColor;
    use crate::image::Orientation;
    use crate::raw::CaptureInfo;

    fn meta() -> RawMeta {
        // An XYZ->camera matrix close to identity in Rec.2020 terms.
        let xyz_to_cam = crate::color::mat_inv(&crate::color::REC2020_TO_XYZ).unwrap();
        RawMeta {
            path: Default::default(),
            info: CaptureInfo::default(),
            color: CameraColor::new(xyz_to_cam),
            as_shot_wb: [1.0; 3],
            orientation: Orientation::Normal,
            hdr: false, lens: None, lens_corrected: false,
        }
    }

    #[test]
    fn dark_image_gets_positive_exposure() {
        let img = LinearImage::new(40, 40, vec![[0.02; 3]; 1600]);
        let ev = auto_exposure(&meta(), &Adjustments::default(), &img);
        assert!(ev > 2.0, "{ev}");
    }

    #[test]
    fn grey_world_neutralizes_cast() {
        let m = meta();
        // A neutral scene under a light that makes the camera see red twice as strong.
        let img = LinearImage::new(40, 40, vec![[0.4, 0.2, 0.2]; 1600]);
        let wb = auto_white_balance(&m, &Adjustments::default(), &img);
        let mult = m.color.wb_for_temp_tint(wb.kelvin, wb.tint);
        assert!((mult[0] / mult[1] - 0.5).abs() < 0.08, "{mult:?}");
    }

    /// A scene with a gradient from dark to bright in camera values (neutral under `cast`).
    fn scene(w: usize, h: usize, scale: f32, cast: [f32; 3]) -> LinearImage {
        LinearImage::new(
            w,
            h,
            (0..w * h)
                .map(|i| {
                    let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
                    let v = scale * (0.05 + 0.9 * x * x) * (0.8 + 0.2 * y);
                    [v * cast[0], v * cast[1], v * cast[2]]
                })
                .collect(),
        )
    }

    #[test]
    fn levels_histogram_shows_what_the_levels_cut_off() {
        let m = meta();
        let img = scene(120, 80, 0.3, [1.0; 3]);
        let plain = Adjustments::default();
        let mut cut = plain.clone();
        cut.levels.rgb.in_black = 0.5;
        cut.curves.rgb.points.insert(1, [0.5, 0.7]);
        // Levels and curves don't change what goes into the levels: no new measurement.
        assert_eq!(levels_input_settings(&plain), levels_input_settings(&cut));
        let h = levels_input_histogram(&m, &levels_input_settings(&cut), &img);
        assert_eq!(h.luma.iter().sum::<u32>(), 120 * 80);
        // The dark half that the black point turns black is still there.
        let below: u32 = h.luma[..120].iter().sum();
        assert!(below > 120 * 80 / 10, "{below}");
    }

    #[test]
    fn auto_adjust_brightens_a_dark_scene_and_respects_the_options() {
        let m = meta();
        let img = scene(120, 80, 0.08, [1.0; 3]);
        let adj = Adjustments { saturation: 20.0, ..Default::default() };
        let out = auto_adjust(&m, &adj, &img, &AutoOptions::default(), &Focus::default(), None);
        assert!(out.exposure > 1.0, "{out:?}");
        assert_eq!(out.saturation, 20.0, "other settings stay");
        for v in [out.highlights, out.shadows, out.whites, out.blacks, out.contrast, out.vibrance] {
            assert!((-50.0..=90.0).contains(&v) && v == v.round(), "{out:?}");
        }
        // Only the exposure: nothing else changes.
        let only = AutoOptions { white_balance: false, exposure: true, tone: false, contrast: false, vibrance: false };
        let start = Adjustments { contrast: 7.0, highlights: 3.0, ..Default::default() };
        let out = auto_adjust(&m, &start, &img, &only, &Focus::default(), None);
        assert!(out.exposure > 1.0);
        assert_eq!(Adjustments { exposure: start.exposure, ..out }, start);
    }

    #[test]
    fn faces_are_exposed_well_even_against_a_bright_background() {
        let mut m = meta();
        // The size of the sensor (radii of sharpening and local tone mapping are relative to it).
        (m.info.width, m.info.height) = (120, 80);
        // Bright background with a dark face-sized patch in the middle (backlight).
        let (w, h) = (120, 80);
        let img = LinearImage::new(
            w,
            h,
            (0..w * h)
                .map(|i| {
                    let (x, y) = (i % w, i / w);
                    let face = (45..75).contains(&x) && (20..60).contains(&y);
                    [if face { 0.012 } else { 0.25 + (x % 7) as f32 * 0.02 }; 3]
                })
                .collect(),
        );
        let opts = AutoOptions { white_balance: false, vibrance: false, ..Default::default() };
        let plain = auto_adjust(&m, &Adjustments::default(), &img, &opts, &Focus::default(), None);
        let focus = Focus { faces: vec![[45.0 / 120.0, 20.0 / 80.0, 30.0 / 120.0, 40.0 / 80.0]], subject: None };
        let with_face = auto_adjust(&m, &Adjustments::default(), &img, &opts, &focus, None);
        let face_luma = |a: &Adjustments| {
            let r = Processor::new(&m, a).render_cpu(&img, None, None);
            let p = &r.rgba[(40 * r.width + 60) * 4..];
            p[1] as f32 / 255.0
        };
        assert!(face_luma(&with_face) > face_luma(&plain) + 0.05, "{} vs {}", face_luma(&with_face), face_luma(&plain));
        // 4.5 EV darker than the background: lifted a lot, without blowing out the background.
        assert!(face_luma(&with_face) > 0.2, "{}", face_luma(&with_face));
        let r = Processor::new(&m, &with_face).render_cpu(&img, None, None);
        let clipped = r.rgba.as_chunks::<4>().0.iter().filter(|p| p[1] >= 250).count();
        assert!(clipped < r.width * r.height / 50, "{clipped}");
    }

    #[test]
    fn auto_white_balance_keeps_a_good_camera_balance_with_large_colored_areas() {
        let mut m = meta();
        // Mostly green foliage, a few grey stones; the camera's white balance is right.
        m.as_shot_wb = [1.0; 3];
        let (w, h) = (100, 100);
        let img = LinearImage::new(
            w,
            h,
            (0..w * h).map(|i| if i % 10 == 0 { [0.3, 0.3, 0.3] } else { [0.08, 0.2, 0.05].map(|v| v * (1.0 + (i % 13) as f32 * 0.03)) }).collect(),
        );
        let wb = auto_white_balance(&m, &Adjustments::default(), &img);
        let mult = m.color.wb_for_temp_tint(wb.kelvin, wb.tint);
        let shot = m.color.temp_tint_for_wb(m.as_shot_wb);
        assert!((mult[0] / mult[1] - 1.0).abs() < 0.1 && (mult[2] / mult[1] - 1.0).abs() < 0.1, "{mult:?} {wb:?} {shot:?}");
    }

    #[test]
    fn take_copies_only_what_the_options_cover() {
        let auto = Adjustments { exposure: 1.0, contrast: 20.0, vibrance: 10.0, shadows: 30.0, ..Default::default() };
        let onto = Adjustments { saturation: 5.0, ..Default::default() };
        let opts = AutoOptions { white_balance: false, exposure: true, tone: false, contrast: true, vibrance: false };
        let out = opts.take(&auto, &onto);
        assert_eq!((out.exposure, out.contrast, out.vibrance, out.shadows, out.saturation), (1.0, 20.0, 0.0, 0.0, 5.0));
    }
}
