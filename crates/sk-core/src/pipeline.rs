//! The development pipeline.
//!
//! 1. geometry: crop, straighten, flip, rotation, keystone, lens correction (resampling)
//! 2. white balance with highlight reconstruction, camera -> linear Rec.2020 working space
//! 3. noise reduction (luminance: guided filter, color: chroma smoothing)
//! 4. scene tone: exposure, local HDR (highlights/shadows/whites/blacks), clarity/structure,
//!    contrast, base curve (scene -> display)
//! 5. color in Oklab: color editor, color balance, saturation/vibrance, black & white
//! 6. encoding (sRGB transfer in working primaries), brightness, levels, curves
//! 7. sharpening, post-crop vignette, grain
//! 8. conversion to the output color space
//!
//! [`Processor::render`] and [`Processor::process`] run on the graphics card when there is
//! one (see `pipeline/gpu.rs`); the CPU implementation here is the reference and fallback.

use std::sync::{Arc, Mutex};

use rayon::prelude::*;

use crate::color::{Mat3, Oklab, OutputSpace, exp2, log2, luminance, mat_vec, srgb_decode, srgb_encode};
use crate::detail::{Plane, box_blur, fast_guided_filter, gaussian_blur, guided_filter, median_hv, value_noise};
use crate::geometry::{GeoMap, Region};
use crate::image::LinearImage;
use crate::mask::{MaskBitmap, apply_retouch, cached_bitmap};
use crate::params::{Adjustments, BaseCurve, ColorBalance, HUE_BANDS, HslBand, LayerAdjustments, Mask, WbMode};
use crate::raw::RawMeta;

mod gpu;

pub use gpu::prepare as prepare_gpu;

const MID_GREY: f32 = 0.18;

const LUT_SIZE: usize = 8192;
/// Rec.2020 luminance coefficients.
const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

#[derive(Clone, Default)]
pub struct RenderOptions {
    /// Show the whole frame (for the crop tool).
    pub ignore_crop: bool,
    /// Paint clipped pixels (display rendering only).
    pub clip_warning: bool,
    /// Tint the mask of this layer red (display rendering only).
    pub mask_overlay: Option<usize>,
    /// Show the image as in this output space or on this printer (display rendering only).
    pub softproof: Option<Arc<crate::softproof::SoftProof>>,
}

/// Result of a display rendering: 8-bit sRGB RGBA plus histogram.
pub struct Rendered {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    pub histogram: Histogram,
    /// Full output size at the resolution of the rendered source.
    pub full_size: (usize, usize),
}

#[derive(Clone, Default)]
pub struct Histogram {
    pub r: Vec<u32>,
    pub g: Vec<u32>,
    pub b: Vec<u32>,
    pub luma: Vec<u32>,
}

/// Normalized source position of each output pixel.
type SourcePositions = Vec<[f32; 2]>;

/// Caches the expensive early stages (geometry, white balance, noise reduction)
/// so that tone and color edits only re-run the later stages.
#[derive(Default)]
pub struct StageCache {
    entry: Mutex<Option<(u64, Arc<LinearImage>)>>,
    /// Resampled with heal/clone and removals, before white balance and noise reduction (so
    /// that changing those doesn't repeat it).
    resampled: Mutex<Option<(u64, Arc<LinearImage>)>>,
    /// Source position (normalized) of each output pixel, for layer masks.
    uv: Mutex<Option<(u64, Arc<SourcePositions>)>>,
    local: Mutex<Option<(u64, Arc<LocalPlanes>)>>,
    gpu: Mutex<gpu::Cache>,
}

/// The entry for `key` in `slot`, else what `make` gives, stored there. Made without holding the
/// lock: the work runs on rayon, where the thread waiting for the lock may be the one that has
/// to finish it (renderings at the same time, as in `auto`, would hang). Two renderings that
/// need the same entry at once may both make it.
fn cached<T>(slot: &Mutex<Option<(u64, Arc<T>)>>, key: u64, make: impl FnOnce() -> Arc<T>) -> Arc<T> {
    if let Some(v) = slot.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|(k, _)| *k == key).map(|(_, v)| v.clone()) {
        return v;
    }
    let v = make();
    *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some((key, v.clone()));
    v
}

/// What a rendering processes: see [`Processor::plan`].
struct Plan {
    geo: GeoMap,
    /// Full output size at the resolution of the source.
    full: (usize, usize),
    region: Region,
    /// `region` plus the margin needed by neighborhood operations.
    ext: Region,
    /// That margin (pixels per side, before clipping to the image).
    margin: usize,
    /// Source resolution relative to the sensor.
    scale: f32,
    /// Subsampling of the fast guided filter (HDR, clarity).
    guided_sub: usize,
    r_hdr: f32,
    r_clarity: f32,
    r_structure: f32,
    hdr_active: bool,
    clarity_active: bool,
    structure_active: bool,
    dehaze_active: bool,
}

impl Plan {
    /// Only a part of the image is rendered (a tile, the zoomed view).
    fn partial(&self) -> bool {
        self.ext.w < self.full.0 || self.ext.h < self.full.1
    }
}

/// Neighborhood planes derived from log luminance; independent of exposure and tone settings.
struct LocalPlanes {
    /// Smoothed log2 luminance relative to middle grey (before exposure).
    hdr_base: Option<Plane>,
    /// Local contrast detail in EV (clarity + structure combined, unscaled per kind).
    clarity: Option<Plane>,
    structure: Option<Plane>,
    dehaze: Option<Arc<DarkChannel>>,
    /// Where the planes' region starts in the region `dehaze` was made for (a part cut out of
    /// the whole image's planes, see [`LocalPlanes::crop`]).
    dark_origin: (usize, usize),
}

impl LocalPlanes {
    /// The part `ext` of the planes of the whole image (`full_w` wide).
    fn crop(&self, full_w: usize, ext: Region) -> LocalPlanes {
        let cut = |p: &Option<Plane>| p.as_ref().map(|p| crop_plane(&p.data, full_w, ext));
        LocalPlanes {
            hdr_base: cut(&self.hdr_base),
            clarity: cut(&self.clarity),
            structure: cut(&self.structure),
            dehaze: self.dehaze.clone(),
            dark_origin: (self.dark_origin.0 + ext.x, self.dark_origin.1 + ext.y),
        }
    }
}

/// The part `ext` of a `full_w` wide plane.
fn crop_plane(data: &[f32], full_w: usize, ext: Region) -> Plane {
    Plane::new(ext.w, ext.h, crop_rows(data, full_w, ext))
}

/// The part `ext` of a `full_w` wide raster, row after row.
fn crop_rows<T: Copy>(data: &[T], full_w: usize, ext: Region) -> Vec<T> {
    (ext.y..ext.y + ext.h).flat_map(|y| data[y * full_w + ext.x..y * full_w + ext.x + ext.w].iter().copied()).collect()
}


/// Low-resolution haze density and airlight for dehazing. The density follows the image's
/// edges: on the block grid it is fitted as a linear function of the luminance (guided
/// filter on the blocks' minima), evaluated per pixel with the pixel's own luminance, so
/// that haze is not left in a halo around objects in front of a hazy background.
struct DarkChannel {
    /// Density at a pixel: `a * guide + b` (`guide`: luminance relative to the airlight's).
    a: Plane,
    b: Plane,
    /// Largest guide the fit saw around each block: the pixel's guide is limited to it, a
    /// bright pixel among darker ones (fitted on the blocks' darkest) would extrapolate.
    top: Plane,
    /// Downsampling factor relative to the working region.
    step: usize,
    /// The haze's color (see `airlight_of`).
    airlight: [f32; 3],
}

/// Transmission below which dehazing eases off (at most about 4x the contrast).
const HAZE_T_MIN: f32 = 0.25;

/// Block size of the dark channel for an output of `full` pixels: the same for every part of
/// the image, so that tiles match.
fn dark_step(full: (usize, usize)) -> usize {
    (full.0.max(full.1) / 256).max(1)
}

fn lcm(a: usize, b: usize) -> usize {
    let gcd = |mut a: usize, mut b: usize| {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    };
    a / gcd(a, b) * b
}

impl DarkChannel {
    /// `airlight`: that of the whole image when `img` is only a part of it.
    fn new(img: &LinearImage, step: usize, airlight: Option<[f32; 3]>) -> Self {
        let (w, h) = (img.width, img.height);
        // Airlight samples: every 7th pixel.
        let samples: Vec<[f32; 3]> = img.data.iter().step_by(7).copied().collect();
        // Per block: smallest value of each channel and of the luminance.
        let (lw, lh) = (w.div_ceil(step), h.div_ceil(step));
        let minima: Vec<[f32; 4]> = (0..lw * lh)
            .into_par_iter()
            .map(|i| {
                let (bx, by) = (i % lw * step, i / lw * step);
                let mut m = [f32::MAX; 4];
                for y in by..(by + step).min(h) {
                    for p in &img.data[y * w + bx..y * w + (bx + step).min(w)] {
                        for (c, v) in [p[0], p[1], p[2], dot(*p, LUMA)].into_iter().enumerate() {
                            m[c] = m[c].min(v.max(0.0));
                        }
                    }
                }
                m
            })
            .collect();
        Self::from_parts(samples, minima, lw, lh, step, airlight)
    }

    /// Haze density at pixel `(x, y)` of color `p`.
    #[inline]
    fn at(&self, x: usize, y: usize, p: [f32; 3]) -> f32 {
        let fx = ((x as f32 + 0.5) / self.step as f32 - 0.5).clamp(0.0, (self.a.w - 1) as f32);
        let fy = ((y as f32 + 0.5) / self.step as f32 - 0.5).clamp(0.0, (self.a.h - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(self.a.w - 1), (y0 + 1).min(self.a.h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let w = self.a.w;
        let lerp = |d: &[f32]| {
            let top = d[y0 * w + x0] * (1.0 - tx) + d[y0 * w + x1] * tx;
            let bot = d[y1 * w + x0] * (1.0 - tx) + d[y1 * w + x1] * tx;
            top * (1.0 - ty) + bot * ty
        };
        let guide = dot(p, LUMA).max(0.0) / dot(self.airlight, LUMA).max(1e-3);
        (lerp(&self.a.data) * guide.min(lerp(&self.top.data)) + lerp(&self.b.data)).clamp(0.0, 1.0)
    }

    /// Removes (strength > 0) or adds (strength < 0) haze.
    #[inline]
    fn apply(&self, p: [f32; 3], dark: f32, strength: f32) -> [f32; 3] {
        let al = self.airlight;
        if strength > 0.0 {
            let t0 = 1.0 - 0.95 * strength * dark;
            // Soft lower limit: no hard edge where the limit starts.
            let t = (0.5 * (t0 + HAZE_T_MIN + ((t0 - HAZE_T_MIN).powi(2) + 0.004).sqrt())).min(1.0);
            let j: [f32; 3] = std::array::from_fn(|c| (p[c] - al[c] * (1.0 - t)) / t);
            // Removing the haze darkens: brighten the dark part of the result somewhat (the
            // haze's own brightness stays, or the sky would burn out).
            let ya = dot(al, LUMA).max(1e-3);
            // t^-0.3 and t^0.4 from one logarithm.
            let ln_t = t.ln();
            let lift = 1.0 + ((-0.3 * ln_t).exp() - 1.0) * (1.0 - dot(j, LUMA) / ya).clamp(0.0, 1.0);
            // Colors gain less than contrast (1/t^0.6 instead of 1/t), or they turn garish.
            let y = dot(j, LUMA);
            let chroma = (0.4 * ln_t).exp();
            // Values below black run out softly instead of clipping.
            let toe = 0.05 * (1.0 - t) * ya;
            j.map(|v| haze_toe((y + (v - y) * chroma) * lift, toe))
        } else {
            let s = -strength * 0.6 * (1.0 - dark * 0.5);
            std::array::from_fn(|c| p[c] + (al[c] - p[c]) * s)
        }
    }
}

/// `p` with the share `s` of the haze color `veil` mixed in (see `Processor::veil`).
#[inline]
fn veiled(p: [f32; 3], veil: [f32; 3], s: f32) -> [f32; 3] {
    if s <= 0.0 {
        return p;
    }
    std::array::from_fn(|c| p[c] + (veil[c] - p[c]) * s)
}

/// `v` above `k`; below it runs smoothly towards 0 (same value and slope at `k`).
#[inline]
fn haze_toe(v: f32, k: f32) -> f32 {
    if v >= k {
        v
    } else if k < 1e-6 {
        v.max(0.0)
    } else {
        k * (v / k - 1.0).exp()
    }
}


/// Tone parameters that can vary per pixel (background + weighted layer deltas).
#[derive(Clone, Copy, Debug)]
struct ToneParams {
    exposure_ev: f32,
    contrast: f32,
    hdr: [f32; 4],
    clarity: f32,
    structure: f32,
    saturation: f32,
    temperature: f32,
    tint: f32,
    brightness: f32,
    dehaze: f32,
    vibrance: f32,
    /// Share of the atmosphere's haze color mixed in (see `Processor::veil`).
    veil: f32,
    /// Color editor: hue shift, saturation and lightness (both /100) per hue band.
    bands: [[f32; 3]; 8],
    /// Color balance offsets per wheel (master, shadows, midtones, highlights), see
    /// `ColorWheel::offset`: being cartesian, the wheels of layers add up.
    balance: [[f32; 3]; 4],
}

fn band_params(bands: &[HslBand; 8]) -> [[f32; 3]; 8] {
    bands.map(|b| [b.hue, b.saturation / 100.0, b.lightness / 100.0])
}

fn balance_params(cb: &ColorBalance) -> [[f32; 3]; 4] {
    [cb.master, cb.shadows, cb.midtones, cb.highlights].map(|w| w.offset())
}

/// Whether a layer changes anything of the color stage (bands, wheels, vibrance).
fn layer_colors(l: &LayerAdjustments) -> bool {
    l.vibrance != 0.0 || !l.color_balance.is_neutral() || l.color_bands.iter().any(|b| *b != HslBand::default())
}

impl ToneParams {
    fn add(&mut self, l: &PreparedLayer, w: f32) {
        self.veil += l.veil * w;
        let l = &l.adj;
        self.exposure_ev += l.exposure * w;
        self.contrast += l.contrast * w;
        self.hdr[0] += l.highlights / 100.0 * w;
        self.hdr[1] += l.shadows / 100.0 * w;
        self.clarity += l.clarity * w;
        self.structure += l.structure * w;
        self.saturation += l.saturation * w;
        self.temperature += l.temperature * w;
        self.tint += l.tint * w;
        self.brightness += l.brightness * w;
        self.dehaze += l.dehaze * w;
        self.hdr[2] += l.whites / 100.0 * w;
        self.hdr[3] += l.blacks / 100.0 * w;
        if layer_colors(l) {
            self.vibrance += l.vibrance * w;
            for (t, d) in self.bands.iter_mut().zip(band_params(&l.color_bands)) {
                t.iter_mut().zip(d).for_each(|(t, d)| *t += d * w);
            }
            for (t, d) in self.balance.iter_mut().zip(balance_params(&l.color_balance)) {
                t.iter_mut().zip(d).for_each(|(t, d)| *t += d * w);
            }
        }
    }
}

/// A layer ready for per-pixel evaluation.
struct PreparedLayer {
    /// Index in `Adjustments::layers`.
    index: usize,
    mask: Mask,
    opacity: f32,
    adj: LayerAdjustments,
    /// Share of the haze color mixed in (the atmosphere's layer only).
    veil: f32,
    /// `adj` or `veil` change the tone or color parameters (not only the curves).
    adds: bool,
    curve: Option<LayerCurve>,
}

impl PreparedLayer {
    fn new(index: usize, mask: Mask, opacity: f32, adj: LayerAdjustments) -> Self {
        Self { index, mask, opacity, adds: !adj.deltas_neutral(), curve: LayerCurve::new(&adj.curves), adj, veil: 0.0 }
    }
}

/// Most of the haze color the atmosphere mixes in (at full strength, in the farthest part).
const VEIL_MAX: f32 = 0.75;

/// A layer's curves as tables: the RGB curve followed by the channel's, then the luma curve
/// (as the background's curves in `encode`).
struct LayerCurve {
    channels: Option<[Vec<f32>; 3]>,
    luma: Option<Vec<f32>>,
}

impl LayerCurve {
    /// `None` for straight lines.
    fn new(c: &crate::params::Curves) -> Option<Self> {
        if c.is_identity() {
            return None;
        }
        let rgb_same = [&c.rgb, &c.red, &c.green, &c.blue].iter().all(|c| c.is_identity());
        let channels = (!rgb_same).then(|| {
            let master = c.rgb.build_lut(LUT_SIZE);
            [&c.red, &c.green, &c.blue].map(|ch| {
                let lut = ch.build_lut(LUT_SIZE);
                (0..LUT_SIZE).map(|i| lut_lookup(&lut, lut_lookup(&master, i as f32 / (LUT_SIZE - 1) as f32))).collect()
            })
        });
        let luma = (!c.luma.is_identity()).then(|| c.luma.build_lut(LUT_SIZE));
        Some(Self { channels, luma })
    }

    fn apply(&self, e: [f32; 3]) -> [f32; 3] {
        let e = match &self.channels {
            Some(t) => [lut_lookup(&t[0], e[0]), lut_lookup(&t[1], e[1]), lut_lookup(&t[2], e[2])],
            None => e,
        };
        match &self.luma {
            Some(lut) => apply_luma_curve(lut, e),
            None => e,
        }
    }
}

/// `PreparedLayer::index` of the layers made from tools (not in `Adjustments::layers`).
const TOOL_LAYER: usize = usize::MAX;

/// Relight and atmosphere as layers with masks from the depth map (none until the map exists),
/// and the map if the atmosphere needs it for its color.
fn depth_layers(meta: &RawMeta, adj: &Adjustments) -> (Vec<PreparedLayer>, Option<Arc<crate::depth::DepthMap>>) {
    let (rl, at) = (&adj.relight, &adj.atmosphere);
    if rl.is_off() && at.is_off() {
        return (Vec::new(), None);
    }
    crate::depth::load(&meta.path).map_or_else(|| (Vec::new(), None), |map| (depth_layers_with(&map, adj), (!at.is_off()).then_some(map)))
}

fn depth_layers_with(map: &Arc<crate::depth::DepthMap>, adj: &Adjustments) -> Vec<PreparedLayer> {
    let (rl, at) = (&adj.relight, &adj.atmosphere);
    let layer = |range: &crate::params::DepthRange, adj: LayerAdjustments| {
        PreparedLayer::new(TOOL_LAYER, Mask { ai: Some(crate::depth::range_mask_cached(map, range)), ..Default::default() }, 1.0, adj)
    };
    let mut out = Vec::new();
    if !rl.is_off() {
        let (near, far) = rl.ranges();
        for (range, ev, warmth) in [(near, rl.near, rl.near_warmth), (far, rl.far, rl.far_warmth)] {
            if ev != 0.0 || warmth != 0.0 {
                out.push(layer(&range, LayerAdjustments { exposure: ev, temperature: warmth, ..Default::default() }));
            }
        }
    }
    if !at.is_off() {
        let mask = Mask { ai: Some(crate::depth::haze_mask_cached(map, at)), ..Default::default() };
        let mut l = PreparedLayer::new(TOOL_LAYER, mask, 1.0, LayerAdjustments::default());
        l.veil = at.amount / 100.0 * VEIL_MAX;
        l.adds = true;
        out.push(l);
    }
    out
}

pub struct Processor {
    adj: Adjustments,
    /// The camera's lens correction (applied if `adj.lens.camera`).
    camera_lens: Option<crate::camera_lens::CameraLens>,
    base: ToneParams,
    layers: Vec<PreparedLayer>,
    wb: [f32; 3],
    cam_to_working: Mat3,
    oklab: Oklab,
    color_active: bool,
    /// Which color stages run (checked once instead of per pixel).
    editor_active: bool,
    skin_active: bool,
    balance_active: bool,
    /// display-linear working 0..1 -> encoded 0..1, per channel (brightness, levels, curves)
    lut: Arc<[Vec<f32>; 3]>,
    /// The luma curve (`None` when it is a straight line).
    luma_lut: Option<Vec<f32>>,
    /// Precomputed tone functions of the background settings (layers may deviate per pixel).
    tone_luts: Arc<ToneLuts>,
    /// Ratio of the rendered source to the full sensor resolution (for radii in full-res pixels).
    sensor_long_edge: f32,
    /// Unclipped floating point data: no highlight reconstruction.
    hdr: bool,
    opts: RenderOptions,
    /// Image and amount (0..1) of AI denoising (see `ai_denoise`).
    ai_denoise: Option<(std::path::PathBuf, f32)>,
    /// Image and amount (0..1) of the removed camera shake (see `ai_deblur`).
    ai_deblur: Option<(std::path::PathBuf, f32)>,
    /// Image and settings of the lens blur (see `lens_blur`).
    lens_blur: Option<(std::path::PathBuf, crate::params::LensBlur)>,
    /// The depth map while the atmosphere is on: its color comes from the far part (see `veil`).
    haze_map: Option<Arc<crate::depth::DepthMap>>,
}

/// The geometric map of [`Processor::geomap`] without building a processor (no tone tables, no
/// copy of the layers and their strokes): for tools that only map points, several times a frame.
pub fn geomap(meta: &RawMeta, adj: &Adjustments, ignore_crop: bool, src_w: usize, src_h: usize) -> GeoMap {
    let adj = adj.visible();
    GeoMap::new(&adj.geometry, &adj.lens, meta.lens.as_ref(), adj.rotation, src_w, src_h, ignore_crop)
}

impl Processor {
    pub fn new(meta: &RawMeta, adj: &Adjustments) -> Self {
        Self::with_options(meta, adj, RenderOptions::default())
    }

    pub fn with_options(meta: &RawMeta, adj: &Adjustments, opts: RenderOptions) -> Self {
        let visible = adj.visible();
        let adj: &Adjustments = &visible;
        let wb = match adj.white_balance.mode {
            WbMode::AsShot => meta.as_shot_wb,
            WbMode::Custom => meta.color.wb_for_temp_tint(adj.white_balance.kelvin, adj.white_balance.tint),
        };
        // dcraw convention: the smallest multiplier is 1.
        let min_wb = wb.iter().cloned().fold(f32::MAX, f32::min).max(1e-6);
        let wb = wb.map(|v| v / min_wb);
        let (mut layers, haze_map) = depth_layers(meta, adj);
        layers.extend(
            adj.layers
                .iter()
                .enumerate()
                .filter(|(i, l)| l.enabled && l.opacity > 0.0 && (!l.adjustments.is_neutral() || opts.mask_overlay == Some(*i)))
                .map(|(index, l)| PreparedLayer::new(index, l.mask.clone(), l.opacity / 100.0, l.adjustments.clone())),
        );
        let color_active = adj.saturation != 0.0
            || adj.vibrance != 0.0
            || !adj.color_editor.is_neutral()
            || !adj.skin_tone.is_neutral()
            || !adj.color_balance.is_neutral()
            || adj.black_white.enabled
            || layers.iter().any(|l| l.adj.saturation != 0.0 || layer_colors(&l.adj));
        let base = ToneParams {
            exposure_ev: adj.exposure,
            contrast: adj.contrast,
            hdr: [adj.highlights / 100.0, adj.shadows / 100.0, adj.whites / 100.0, adj.blacks / 100.0],
            clarity: adj.detail.clarity,
            structure: adj.detail.structure,
            saturation: adj.saturation,
            temperature: 0.0,
            tint: 0.0,
            brightness: 0.0,
            dehaze: adj.detail.dehaze,
            vibrance: adj.vibrance,
            veil: 0.0,
            bands: band_params(&adj.color_editor.bands),
            balance: balance_params(&adj.color_balance),
        };
        let layer_bands = layers.iter().any(|l| l.adj.color_bands.iter().any(|b| *b != HslBand::default()));
        let layer_balance = layers.iter().any(|l| !l.adj.color_balance.is_neutral());
        Self {
            base,
            layers,
            wb,
            // Process 3: the color matrix follows the light (daylight to tungsten).
            cam_to_working: match adj.white_balance.mode {
                _ if !adj.process_3() || meta.color.xyz_to_cam_a.is_none() => meta.color.cam_to_working,
                WbMode::Custom => meta.color.cam_to_working_at(adj.white_balance.kelvin),
                WbMode::AsShot => meta.color.cam_to_working_at(meta.color.temp_tint_for_wb(meta.as_shot_wb).0),
            },
            oklab: Oklab::default(),
            color_active,
            editor_active: !adj.color_editor.is_neutral() || layer_bands,
            skin_active: !adj.skin_tone.is_neutral(),
            balance_active: !adj.color_balance.is_neutral() || layer_balance,
            lut: cached_lut(serde_json::json!({ "b": adj.brightness, "l": adj.levels, "c": adj.curves }), || build_output_luts(adj)),
            luma_lut: (!adj.curves.luma.is_identity()).then(|| adj.curves.luma.build_lut(LUT_SIZE)),
            tone_luts: cached_lut(serde_json::json!({ "h": base.hdr, "c": base.contrast, "k": adj.base_curve, "cc": adj.camera_curve }), || {
                ToneLuts::new(&base, adj.base_curve, adj.camera_curve.as_ref())
            }),
            sensor_long_edge: meta.info.width.max(meta.info.height).max(1) as f32,
            hdr: meta.hdr,
            opts,
            ai_denoise: (adj.detail.ai_denoise > 0.0 && !meta.hdr).then(|| (meta.path.clone(), adj.detail.ai_denoise / 100.0)),
            ai_deblur: (adj.detail.ai_deblur > 0.0 && !meta.hdr).then(|| (meta.path.clone(), adj.detail.ai_deblur / 100.0)),
            lens_blur: (!adj.lens_blur.is_off()).then(|| (meta.path.clone(), adj.lens_blur)),
            haze_map,
            adj: adj.clone(),
            camera_lens: meta.lens.clone(),
        }
    }

    pub fn geomap(&self, src_w: usize, src_h: usize) -> GeoMap {
        GeoMap::new(&self.adj.geometry, &self.adj.lens, self.camera_lens.as_ref(), self.adj.rotation, src_w, src_h, self.opts.ignore_crop)
    }

    /// Output size (at the resolution of a `src_w` x `src_h` source).
    pub fn output_size(&self, src_w: usize, src_h: usize) -> (usize, usize) {
        let g = self.geomap(src_w, src_h);
        (g.out_w, g.out_h)
    }

    /// How far (output pixels of a rendering of `src`) a change of one pixel's development
    /// spreads through the operations after it (sharpening, glow): a part rendered after a
    /// change must reach that far beyond it.
    pub fn output_spread(&self, src_w: usize, src_h: usize) -> usize {
        let d = &self.adj.detail;
        let (w, h) = self.output_size(src_w, src_h);
        let scale = src_w.max(src_h) as f32 / self.sensor_long_edge;
        let mut spread: f32 = 0.0;
        if d.sharpen_amount > 0.0 {
            spread = spread.max(d.sharpen_radius * scale * 4.0 + 2.0);
        }
        if d.glow_amount > 0.0 {
            spread = spread.max(glow_sigma(d, w.max(h) as f32) * 3.0 + 2.0);
        }
        spread.ceil() as usize
    }

    // ------------------------------------------------------------ per-pixel stages

    /// Camera RGB -> linear working RGB with white balance and highlight reconstruction.
    #[inline]
    fn to_working(&self, raw: [f32; 3]) -> [f32; 3] {
        let mut b = [raw[0] * self.wb[0], raw[1] * self.wb[1], raw[2] * self.wb[2]];
        let max_raw = raw[0].max(raw[1]).max(raw[2]);
        if max_raw > 0.95 && !self.hdr {
            // Clipped channels are unreliable: estimate them as at least as bright as the
            // brightest channel, which keeps blown highlights neutral (not magenta) while
            // retaining the gradient of unclipped channels.
            let hi = b[0].max(b[1]).max(b[2]);
            for c in 0..3 {
                let t = smoothstep(0.95, 1.0, raw[c]);
                b[c] += (b[c].max(hi) - b[c]) * t;
            }
        }
        mat_vec(&self.cam_to_working, b).map(|v| v.max(0.0))
    }

    /// Scene-linear working RGB -> display-linear working RGB.
    #[inline]
    fn tone(&self, rgb: [f32; 3], local_ev: Option<f32>, detail_ev: [f32; 2], tp: &ToneParams) -> [f32; 3] {
        let gain = exp2(tp.exposure_ev + detail_ev[0] * tp.clarity / 100.0 * 0.6 + detail_ev[1] * tp.structure / 100.0 * 0.8);
        let mut rgb = rgb.map(|v| v * gain);
        if tp.temperature != 0.0 || tp.tint != 0.0 {
            let t = tp.temperature / 100.0;
            rgb[0] *= exp2(t * 0.35);
            rgb[2] *= exp2(-t * 0.35);
            rgb[1] *= exp2(-tp.tint / 100.0 * 0.25);
        }
        let l = dot(rgb, LUMA);
        if l > 1e-7 && tp.hdr.iter().any(|v| *v != 0.0) {
            let ev_px = log2(l / MID_GREY);
            // Weights come mostly from the smoothed neighborhood (local tone mapping).
            let ev = match local_ev {
                Some(e) => (e + tp.exposure_ev) * 0.75 + ev_px * 0.25,
                None => ev_px,
            };
            let gain_ev = match &self.tone_luts.hdr {
                Some([by_ev, by_px]) if tp.hdr == self.base.hdr => ev_lookup(by_ev, ev) + ev_lookup(by_px, ev_px),
                _ => hdr_gain_ev(tp.hdr, ev, ev_px),
            };
            let g = exp2(gain_ev);
            rgb.iter_mut().for_each(|v| *v *= g);
        }
        let lut = self.tone_luts.curve.as_ref().filter(|_| tp.contrast == self.base.contrast);
        let curve = |v: f32| match (&self.tone_luts.camera, lut) {
            (Some(cam), _) => curve_lookup(cam, contrast_warp(tp.contrast, v)),
            (None, Some(lut)) => curve_lookup(lut, v),
            (None, None) => contrast_and_base_curve(self.adj.base_curve, tp.contrast, v),
        };
        // The camera's curve per channel, as cameras apply it: highlights run into white and
        // shadows get richer, like in its JPEG.
        if self.adj.process_3() && self.tone_luts.camera.is_none() { hue_preserving(rgb, curve) } else { rgb.map(curve) }
    }

    /// Color adjustments in Oklab on display-linear working RGB.
    #[inline]
    fn color(&self, rgb: [f32; 3], tp: &ToneParams) -> [f32; 3] {
        if !self.color_active {
            return rgb;
        }
        let [mut l, a0, b0] = self.oklab.from_linear(rgb);
        let c0 = (a0 * a0 + b0 * b0).sqrt();
        let mut c = c0;
        // The hue angle is only needed by the color editor and the B&W mixer; otherwise
        // chroma changes simply scale a/b (saves atan2, sin and cos per pixel).
        let need_hue = self.editor_active || self.skin_active || self.adj.black_white.enabled;
        let mut h = if need_hue { b0.atan2(a0).to_degrees().rem_euclid(360.0) } else { 0.0 };
        // Hue is unreliable for nearly neutral colors.
        let chroma_w = smoothstep(0.0, 0.05, c);

        let ed = &self.adj.color_editor;
        if self.editor_active {
            let h_in = h;
            let w = band_weights(h);
            let (mut dh, mut ds, mut dl) = (0.0, 0.0, 0.0);
            for (w, band) in w.iter().zip(&tp.bands) {
                dh += w * band[0];
                ds += w * band[1];
                dl += w * band[2];
            }
            h += dh * chroma_w;
            c *= (1.0 + ds * chroma_w).max(0.0);
            l *= 1.0 + dl * 0.4 * chroma_w;
            // Ranges of one's own select by the color as it comes in (as picked in the image).
            for z in ed.active_zones() {
                let w = z.weight(h_in) * chroma_w;
                if w > 0.0 {
                    h += z.adjust.hue * w;
                    c *= (1.0 + z.adjust.saturation / 100.0 * w).max(0.0);
                    l *= 1.0 + z.adjust.lightness / 100.0 * 0.4 * w;
                }
            }
        }

        if self.skin_active {
            let st = &self.adj.skin_tone;
            let w = skin_weight(st, h, c);
            if w > 0.0 {
                let dh = (st.hue - h + 540.0).rem_euclid(360.0) - 180.0;
                h += (dh * st.hue_uniformity / 100.0 + st.hue_shift) * w;
                c += (st.chroma - c) * st.saturation_uniformity / 100.0 * w;
                c *= (1.0 + st.saturation / 100.0 * w).max(0.0);
                // Only partly towards the target lightness, so that faces keep their modelling.
                l += (st.lightness - l) * st.lightness_uniformity / 100.0 * 0.5 * w;
                l *= 1.0 + st.lightness_shift / 100.0 * 0.3 * w;
            }
        }

        let s = 1.0 + tp.saturation / 100.0;
        let vib = tp.vibrance / 100.0;
        if s != 1.0 || vib != 0.0 {
            let low = (1.0 - c / 0.2).clamp(0.0, 1.0);
            c *= (s * (1.0 + vib * low * low)).max(0.0);
        }

        let (mut a, mut b) = if need_hue {
            let hr = h.to_radians();
            (c * hr.cos(), c * hr.sin())
        } else if c0 > 0.0 {
            (a0 * (c / c0), b0 * (c / c0))
        } else {
            (a0, b0)
        };

        if self.balance_active {
            let lw = l.clamp(0.0, 1.0);
            let ws = (1.0 - lw) * (1.0 - lw);
            let wh = lw * lw;
            let wm = (1.0 - ws - wh).max(0.0);
            for (wheel, weight) in tp.balance.iter().zip([1.0, ws, wm, wh]) {
                a += wheel[0] * 0.08 * weight;
                b += wheel[1] * 0.08 * weight;
                l += wheel[2] * 0.15 * weight;
            }
        }

        let bw = &self.adj.black_white;
        if bw.enabled {
            let w = band_weights(h);
            let mix: f32 = w.iter().zip(&bw.mix).map(|(w, m)| w * m / 100.0).sum();
            l *= (1.0 + mix * 0.6 * smoothstep(0.0, 0.1, c)).max(0.0);
            a = 0.0;
            b = 0.0;
            let lw = l.clamp(0.0, 1.0);
            for (wheel, weight) in [(bw.shadow_tint, (1.0 - lw) * (1.0 - lw)), (bw.highlight_tint, lw * lw)] {
                let hr = wheel.hue.to_radians();
                let amt = wheel.amount / 100.0 * 0.06 * weight;
                a += amt * hr.cos();
                b += amt * hr.sin();
            }
        }
        self.oklab.to_linear([l, a, b]).map(|v| v.clamp(0.0, 1.0))
    }

    /// Scene-linear luminance (working space, before exposure) of a camera RGB value.
    pub(crate) fn scene_luminance(&self, raw: [f32; 3]) -> f32 {
        dot(self.to_working(raw), LUMA)
    }

    /// Oklab (L, a, b) of a camera RGB value at the input of the color adjustments (without
    /// local operators), e.g. to pick the skin tone target.
    pub fn color_input(&self, raw: [f32; 3]) -> [f32; 3] {
        self.oklab.from_linear(self.tone(self.to_working(raw), None, [0.0; 2], &self.base))
    }

    #[inline]
    fn encode(&self, rgb: [f32; 3]) -> [f32; 3] {
        let e = [lut_lookup(&self.lut[0], rgb[0]), lut_lookup(&self.lut[1], rgb[1]), lut_lookup(&self.lut[2], rgb[2])];
        match &self.luma_lut {
            Some(lut) => apply_luma_curve(lut, e),
            None => e,
        }
    }

    // ------------------------------------------------------------ image stages

    /// The atmosphere's haze color (scene-linear working RGB): the average color of the image
    /// where the haze is (weighted by its density), somewhat paler, as bright as the bright
    /// quarter there, then brightened and warmed by the tool's settings. Light scattered by the
    /// air has the color of the distance and is brighter than what it covers, but not the white
    /// of the brightest highlight. From the whole `src`, so parts and the whole get the same.
    fn veil(&self, src: &LinearImage) -> [f32; 3] {
        let Some(map) = &self.haze_map else { return [0.0; 3] };
        let at = &self.adj.atmosphere;
        let (w, h) = (src.width, src.height);
        let step = ((w * h) as f32 / 20_000.0).sqrt().max(1.0) as usize;
        let (mut sum, mut total, mut all, mut n) = ([0.0f32; 3], 0.0f32, [0.0f32; 3], 0.0f32);
        let mut lumas: Vec<(f32, f32)> = Vec::new();
        for y in (step / 2..h).step_by(step) {
            for x in (step / 2..w).step_by(step) {
                let p = self.to_working(src.data[y * w + x]);
                let k = at.density(map.sample((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32));
                for c in 0..3 {
                    sum[c] += p[c] * k;
                    all[c] += p[c];
                }
                total += k;
                n += 1.0;
                lumas.push((dot(p, LUMA), k));
            }
        }
        // Hardly any haze anywhere: the image's average.
        let hazy = total > 1e-3 * n;
        let m = if hazy { sum.map(|v| v / total) } else { all.map(|v| v / n.max(1.0)) };
        lumas.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        let weight = |l: &(f32, f32)| if hazy { l.1 } else { 1.0 };
        let all_weight: f32 = lumas.iter().map(weight).sum();
        let mut acc = 0.0;
        let bright = lumas.iter().find(|l| {
            acc += weight(l);
            acc >= 0.75 * all_weight
        });
        let y = dot(m, LUMA);
        let target = bright.map_or(y, |l| l.0).max(y);
        let m = m.map(|v| (y + (v - y) * 0.6) * target / y.max(1e-6));
        let y = target;
        let t = at.warmth / 100.0;
        let warm = [m[0] * exp2(t * 0.35), m[1], m[2] * exp2(-t * 0.35)];
        let k = y / dot(warm, LUMA).max(1e-6) * exp2(at.brightness / 100.0);
        warm.map(|v| (v * k).max(0.0))
    }

    /// Airlight (the haze's brightness) of the whole image, for every rendering (parts, tiles
    /// and the whole alike, so that they match): estimated from a small copy, cached per image
    /// and settings.
    fn whole_airlight(&self, src: &LinearImage) -> [f32; 3] {
        static CACHE: Mutex<Vec<(u64, [f32; 3])>> = Mutex::new(Vec::new());
        let key = self.stage_key(src, Region { x: 0, y: 0, w: 0, h: 0 });
        if let Some((_, a)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, _)| *k == key) {
            return *a;
        }
        let level = src.width.max(src.height).div_ceil(768).max(1);
        let small;
        let img = if level > 1 {
            small = src.box_downscaled(level);
            &small
        } else {
            src
        };
        let plan = self.plan(img, None);
        let working = self.early_stages(img, &plan.geo, plan.ext, plan.scale, None);
        let a = gpu::airlight_of(working.data.iter().step_by(7).copied().collect());
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        cache.push((key, a));
        if cache.len() > 8 {
            cache.remove(0);
        }
        a
    }

    /// What the resampled camera image with heal/clone and removals depends on (not white
    /// balance or noise reduction).
    fn retouch_relevant(&self) -> serde_json::Value {
        serde_json::json!({
            "g": self.adj.geometry, "l": self.adj.lens, "r": self.adj.rotation, "ic": self.opts.ignore_crop,
            "rt": self.adj.retouch, "rm": self.adj.removals.iter().map(|r| (r.id, r.rect, r.opacity)).collect::<Vec<_>>(),
        })
    }

    fn stage_key(&self, src: &LinearImage, region: Region) -> u64 {
        let relevant = serde_json::json!({
            "geo": self.retouch_relevant(),
            "wb": self.wb, "nl": self.adj.detail.nr_luma, "nc": self.adj.detail.nr_color,
            // Depends on the process version and the light, not only on the multipliers.
            "cw": self.cam_to_working,
        });
        Self::source_key(src, region, &relevant)
    }

    /// Key of what depends on `src` and the settings in `relevant` for `region`.
    fn source_key(src: &LinearImage, region: Region, relevant: &serde_json::Value) -> u64 {
        // The address alone could be that of a freed image whose memory went to the next one.
        let key = format!("{:p}|{}x{}|{}|{region:?}|{relevant}", src.data.as_ptr(), src.width, src.height, crate::ai_product::fingerprint(src));
        crate::cache::fnv1a(key.as_bytes())
    }

    /// Geometry + white balance + noise reduction -> linear working RGB.
    fn early_stages(&self, src: &LinearImage, geo: &GeoMap, region: Region, scale: f32, cache: Option<&StageCache>) -> LinearImage {
        let resample = || {
            let mut img = geo.resample(src, region);
            crate::mask::apply_removals(&mut img, src, geo, region, &self.adj.removals);
            apply_retouch(&mut img, src, geo, region, &self.adj.retouch);
            img
        };
        let mut img = match cache {
            Some(c) => {
                let key = Self::source_key(src, region, &self.retouch_relevant());
                (*cached(&c.resampled, key, || Arc::new(resample()))).clone()
            }
            None => resample(),
        };
        img.data.par_iter_mut().for_each(|p| *p = self.to_working(*p));
        self.defringe(&mut img, scale);
        self.noise_reduction(&mut img, scale);
        img
    }

    /// Takes the color out of purple and green fringes (longitudinal chromatic aberration):
    /// pixels of those hues next to strong contrast.
    fn defringe(&self, img: &mut LinearImage, scale: f32) {
        let Some(amounts) = defringe_amounts(&self.adj.lens) else { return };
        let (w, h) = (img.width, img.height);
        let s = Plane::new(w, h, img.data.par_iter().map(|p| dot(*p, LUMA).max(0.0).sqrt()).collect());
        let sq = s.map(|v| v * v);
        let sigma = defringe_sigma(scale);
        let (mean, mean_sq) = rayon::join(|| gaussian_blur(&s, sigma), || gaussian_blur(&sq, sigma));
        img.data.par_iter_mut().enumerate().for_each(|(i, p)| *p = defringe_pixel(*p, mean.data[i], mean_sq.data[i], amounts));
    }

    fn noise_reduction(&self, img: &mut LinearImage, scale: f32) {
        let d = &self.adj.detail;
        if d.nr_luma <= 0.0 && d.nr_color <= 0.0 {
            return;
        }
        let (w, h) = (img.width, img.height);
        // Work on a perceptual (square-root) luminance and log-free chroma ratios.
        let y_new = (d.nr_luma > 0.0).then(|| {
            let y = Plane::new(w, h, img.data.par_iter().map(|p| dot(*p, LUMA).max(0.0).sqrt()).collect());
            let r = ((2.0 * scale).round() as usize).max(1);
            let eps = (d.nr_luma / 100.0).powi(2) * 0.0025;
            let f = guided_filter(&y, r, eps);
            let keep = 1.0 - d.nr_luma / 100.0 * 0.85;
            y.zip(&f, |a, b| b + (a - b) * keep)
        });
        let chroma = (d.nr_color > 0.0).then(|| {
            let sigma = (d.nr_color / 100.0 * 6.0 * scale).max(0.5);
            let (cr, cb): (Vec<f32>, Vec<f32>) = img
                .data
                .par_iter()
                .map(|p| {
                    let l = dot(*p, LUMA);
                    (p[0] - l, p[2] - l)
                })
                .unzip();
            let (cr, cb) = rayon::join(|| gaussian_blur(&Plane::new(w, h, cr), sigma), || gaussian_blur(&Plane::new(w, h, cb), sigma));
            [cr, cb]
        });
        img.data.par_iter_mut().enumerate().for_each(|(i, p)| {
            let y_old = dot(*p, LUMA).max(0.0);
            let y2 = match &y_new {
                Some(y) => y.data[i].max(0.0).powi(2),
                None => y_old,
            };
            let (mut cr, mut cb) = (p[0] - y_old, p[2] - y_old);
            if let Some([r, b]) = &chroma {
                cr = r.data[i];
                cb = b.data[i];
            }
            let r = y2 + cr;
            let b = y2 + cb;
            let g = (y2 - LUMA[0] * r - LUMA[2] * b) / LUMA[1];
            *p = [r.max(0.0), g.max(0.0), b.max(0.0)];
        });
    }

    /// Regions, radii and active local operators of a rendering.
    fn plan(&self, src: &LinearImage, region: Option<Region>) -> Plan {
        let geo = self.geomap(src.width, src.height);
        let full = (geo.out_w, geo.out_h);
        let region = region.unwrap_or(Region { x: 0, y: 0, w: full.0, h: full.1 });
        let long_edge = full.0.max(full.1) as f32;
        let scale = src.width.max(src.height) as f32 / self.sensor_long_edge;
        let d = &self.adj.detail;

        // Which local operators are needed (by the background or by any layer).
        let any_layer = |f: &dyn Fn(&LayerAdjustments) -> bool| self.layers.iter().any(|l| f(&l.adj));
        let hdr_active = self.base.hdr.iter().any(|v| *v != 0.0) || any_layer(&|l| l.highlights != 0.0 || l.shadows != 0.0 || l.whites != 0.0 || l.blacks != 0.0);
        let clarity_active = d.clarity != 0.0 || any_layer(&|l| l.clarity != 0.0);
        let structure_active = d.structure != 0.0 || any_layer(&|l| l.structure != 0.0);
        let dehaze_active = d.dehaze != 0.0 || any_layer(&|l| l.dehaze != 0.0);

        // Margin for neighborhood operations, so that tiles match the full rendering. The
        // stages run one after another, so their reaches add up: defringe and noise reduction,
        // then the local operators (side by side on the same image, the widest counts), then
        // sharpening and glow.
        let r_hdr = (long_edge * 0.02).max(2.0);
        let r_clarity = (long_edge * 0.03).max(3.0);
        let r_structure = (3.0 * scale).max(1.0);
        let mut margin: f32 = 0.0;
        if defringe_amounts(&self.adj.lens).is_some() {
            margin += defringe_sigma(scale) * 3.0 + 4.0;
        }
        if d.nr_luma > 0.0 || d.nr_color > 0.0 {
            margin += 20.0 * scale + 4.0;
        }
        let mut local: f32 = 0.0;
        if hdr_active {
            let median = if self.adj.process_3() { median_radius(r_hdr) } else { 0 };
            local = local.max(r_hdr * 2.0 + median as f32);
        }
        if clarity_active {
            local = local.max(r_clarity * 2.0);
        }
        if structure_active {
            local = local.max(r_structure * 4.0);
        }
        if dehaze_active {
            local = local.max(long_edge / 64.0);
        }
        margin += local;
        if d.sharpen_amount > 0.0 {
            margin += d.sharpen_radius * scale * 4.0 + 2.0;
        }
        if d.glow_amount > 0.0 {
            margin += glow_sigma(d, long_edge) * 3.0 + 2.0;
        }
        let m = margin.ceil() as usize;
        // Parts start on the grid of the whole image: the fast guided filter's subsampling
        // blocks and the dark channel's blocks are counted from the start, so that they are
        // the same as for the whole image.
        let guided_sub = ((r_hdr.min(r_clarity) / 8.0) as usize).clamp(1, 8);
        let mut align = 1;
        if hdr_active || clarity_active {
            align = guided_sub;
        }
        if dehaze_active {
            align = lcm(align, dark_step(full));
        }
        let (x0, y0) = (region.x.saturating_sub(m) / align * align, region.y.saturating_sub(m) / align * align);
        let ext = Region { x: x0, y: y0, w: (region.x + region.w + m).min(full.0) - x0, h: (region.y + region.h + m).min(full.1) - y0 };
        Plan { geo, full, region, ext, margin: m, scale, guided_sub, r_hdr, r_clarity, r_structure, hdr_active, clarity_active, structure_active, dehaze_active }
    }

    /// Processes `region` (default: whole output) into encoded working RGB (sRGB transfer).
    #[allow(clippy::type_complexity)]
    fn process_encoded(
        &self,
        src: &LinearImage,
        region: Option<Region>,
        cache: Option<&StageCache>,
    ) -> (LinearImage, (usize, usize), Option<Vec<f32>>) {
        let plan = self.plan(src, region);
        let partial = plan.partial();
        let Plan { geo, full, region, ext, scale, guided_sub, r_hdr, r_clarity, r_structure, hdr_active, clarity_active, structure_active, dehaze_active, .. } = plan;
        let d = &self.adj.detail;

        // A part of the image whose whole is in the cache (painting renders only what the brush
        // changed): cut out of it, which is much faster and gives exactly the whole rendering.
        let whole_key = cache.filter(|_| partial).map(|_| self.stage_key(src, Region { x: 0, y: 0, w: full.0, h: full.1 }));
        let whole = cache.zip(whole_key).and_then(|(c, key)| c.entry.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|(k, _)| *k == key).map(|(_, img)| img.clone()));
        let working = match (&whole, cache) {
            (Some(img), _) => Arc::new(LinearImage::new(ext.w, ext.h, crop_rows(&img.data, full.0, ext))),
            (None, Some(c)) => {
                let key = self.stage_key(src, ext);
                cached(&c.entry, key, || Arc::new(self.early_stages(src, &geo, ext, scale, Some(c))))
            }
            (None, None) => Arc::new(self.early_stages(src, &geo, ext, scale, None)),
        };
        let (w, h) = (ext.w, ext.h);

        // Local operators on log luminance. They are shift invariant in log space, so they
        // are computed before exposure and cached independently of tone settings.
        let need_log = hdr_active || clarity_active || structure_active || dehaze_active;
        let local = if need_log {
            let local_key = |stage: u64| {
                let k = format!("{stage}|{hdr_active}|{clarity_active}|{structure_active}|{dehaze_active}|{}", self.adj.process_3());
                crate::cache::fnv1a(k.as_bytes())
            };
            let key = local_key(self.stage_key(src, ext));
            // Cut out of the whole image's planes as well (see `whole`).
            let whole_planes = whole.as_ref().and(cache).zip(whole_key).and_then(|(c, wk)| {
                let key = local_key(wk);
                c.local.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|(k, _)| *k == key).map(|(_, planes)| planes.clone())
            });
            let compute = || {
                let log_l = Plane::new(w, h, working.data.par_iter().map(|p| log2(dot(*p, LUMA).max(1e-6)) - MID_GREY.log2()).collect());
                let sub = guided_sub;
                LocalPlanes {
                    hdr_base: hdr_active.then(|| match median_radius(r_hdr) {
                        r if r > 0 && self.adj.process_3() => fast_guided_filter(&median_hv(&log_l, r), r_hdr as usize, 0.5, sub),
                        _ => fast_guided_filter(&log_l, r_hdr as usize, 0.5, sub),
                    }),
                    clarity: clarity_active.then(|| {
                        let base = fast_guided_filter(&log_l, r_clarity as usize, 0.3, sub);
                        log_l.zip(&base, |a, b| (a - b).clamp(-3.0, 3.0))
                    }),
                    structure: structure_active.then(|| {
                        let base = gaussian_blur(&log_l, r_structure);
                        log_l.zip(&base, |a, b| (a - b).clamp(-2.0, 2.0))
                    }),
                    // Haze measured on the whole, the same for every part and the whole rendering.
                    dehaze: dehaze_active.then(|| Arc::new(DarkChannel::new(&working, dark_step(full), Some(self.whole_airlight(src))))),
                    dark_origin: (0, 0),
                }
            };
            Some(match (whole_planes, cache) {
                (Some(planes), _) => Arc::new(planes.crop(full.0, ext)),
                (None, Some(c)) => cached(&c.local, key, || Arc::new(compute())),
                (None, None) => Arc::new(compute()),
            })
        } else {
            None
        };

        // Layer masks (rasterized once per mask change, sampled in source coordinates).
        let aspect = src.width as f32 / src.height.max(1) as f32;
        let bitmaps: Vec<Option<Arc<MaskBitmap>>> =
            self.layers.iter().map(|l| (!l.mask.is_empty()).then(|| cached_bitmap(&l.mask, aspect, Some(src)))).collect();
        let overlay_layer = self.opts.mask_overlay.and_then(|i| self.layers.iter().position(|l| l.index == i));
        let (sw, sh) = (src.width as f32, src.height as f32);
        // Source position of each pixel: the full lens mapping, kept while the geometry stays.
        let uv: Option<Arc<Vec<[f32; 2]>>> = (!self.layers.is_empty()).then(|| {
            let compute = || {
                Arc::new(
                    (0..w * h)
                        .into_par_iter()
                        .map(|i| {
                            let (sx, sy) = geo.source_point((ext.x + i % w) as f32 + 0.5, (ext.y + i / w) as f32 + 0.5);
                            [(sx + 0.5) / sw, (sy + 0.5) / sh]
                        })
                        .collect::<Vec<_>>(),
                )
            };
            match cache {
                Some(c) => {
                    let relevant = serde_json::json!({
                        "g": self.adj.geometry, "l": self.adj.lens, "r": self.adj.rotation, "ic": self.opts.ignore_crop,
                    });
                    let key = Self::source_key(src, ext, &relevant);
                    cached(&c.uv, key, compute)
                }
                None => compute(),
            }
        });

        let veil = self.veil(src);
        // Per pixel: encoded result and mask overlay weight.
        let pixel = |i: usize, p: [f32; 3]| -> ([f32; 3], f32) {
            let (mut ev, mut det) = (None, [0.0f32; 2]);
            if let Some(lp) = &local {
                ev = lp.hdr_base.as_ref().map(|pl| pl.data[i]);
                det = [lp.clarity.as_ref().map_or(0.0, |c| c.data[i]), lp.structure.as_ref().map_or(0.0, |c| c.data[i])];
            }
            let dark = local.as_ref().and_then(|lp| lp.dehaze.as_ref().map(|dc| (dc, dc.at(i % w + lp.dark_origin.0, i / w + lp.dark_origin.1, p))));
            let haze = |p: [f32; 3], strength: f32| match dark {
                Some((dc, dv)) if strength != 0.0 => dc.apply(p, dv, strength / 100.0),
                _ => p,
            };
            let base_disp = || self.color(self.tone(haze(p, self.base.dehaze), ev, det, &self.base), &self.base);
            if self.layers.is_empty() {
                return (self.encode(base_disp()), 0.0);
            }
            let [u, v] = uv.as_ref().expect("with layers")[i];
            let mut base_cache: Option<[f32; 3]> = None;
            // Mask weight of a layer at this pixel, times its opacity.
            let weight = |li: usize, layer: &PreparedLayer, base_cache: &mut Option<[f32; 3]>| {
                let mut wgt = bitmaps[li].as_ref().map_or(0.0, |b| b.sample(u, v));
                if layer.mask.invert {
                    wgt = 1.0 - wgt;
                }
                if wgt > 0.0 && (layer.mask.luma_range.is_some() || layer.mask.color_range.is_some()) {
                    let bd = *base_cache.get_or_insert_with(base_disp);
                    wgt *= self.range_weight(&layer.mask, bd);
                }
                wgt * layer.opacity
            };
            let mut tp = self.base;
            let mut any = false;
            let mut curved = false;
            let mut overlay = 0.0;
            for (li, layer) in self.layers.iter().enumerate() {
                let wgt = weight(li, layer, &mut base_cache);
                if Some(li) == overlay_layer {
                    overlay = wgt;
                }
                if wgt > 0.0 && layer.adds {
                    tp.add(layer, wgt);
                    any = true;
                }
                curved |= wgt > 0.0 && layer.curve.is_some();
            }
            let mut e = if any {
                let mut e = self.encode(self.color(self.tone(veiled(haze(p, tp.dehaze), veil, tp.veil), ev, det, &tp), &tp));
                if tp.brightness != 0.0 {
                    let g = 2f32.powf(-tp.brightness / 100.0 * 0.6);
                    e = e.map(|v| v.max(0.0).powf(g));
                }
                e
            } else {
                self.encode(base_cache.unwrap_or_else(base_disp))
            };
            // The layers' curves on the result, each blended in by its mask.
            if curved {
                for (li, layer) in self.layers.iter().enumerate() {
                    if let Some(c) = &layer.curve {
                        let wgt = weight(li, layer, &mut base_cache);
                        if wgt > 0.0 {
                            let to = c.apply(e);
                            e = std::array::from_fn(|k| e[k] + (to[k] - e[k]) * wgt);
                        }
                    }
                }
            }
            (e, overlay)
        };
        // Written row by row straight into the output (no intermediate buffers).
        let mut out = vec![[0.0f32; 3]; w * h];
        let mut mask_plane = overlay_layer.map(|_| vec![0.0f32; w * h]);
        let src_rows = working.data.par_chunks(w);
        match &mut mask_plane {
            Some(mp) => out.par_chunks_mut(w).zip(mp.par_chunks_mut(w)).zip(src_rows).enumerate().for_each(|(y, ((o, m), src))| {
                for (x, (o, m)) in o.iter_mut().zip(m.iter_mut()).enumerate() {
                    (*o, *m) = pixel(y * w + x, src[x]);
                }
            }),
            None => out.par_chunks_mut(w).zip(src_rows).enumerate().for_each(|(y, (o, src))| {
                for (x, o) in o.iter_mut().enumerate() {
                    *o = pixel(y * w + x, src[x]).0;
                }
            }),
        }

        if d.sharpen_amount > 0.0 {
            let sigma = d.sharpen_radius * scale;
            if sigma >= 0.25 {
                let y = Plane::new(w, h, out.par_iter().map(|p| dot(*p, LUMA)).collect());
                let blur = gaussian_blur(&y, sigma);
                let amount = d.sharpen_amount / 100.0;
                let thr = d.sharpen_threshold / 255.0;
                out.par_iter_mut().enumerate().for_each(|(i, p)| {
                    let diff = y.data[i] - blur.data[i];
                    let soft = if thr > 0.0 { (diff.abs() / thr).min(1.0) } else { 1.0 };
                    let delta = diff * amount * soft;
                    *p = p.map(|v| (v + delta).clamp(0.0, 1.0));
                });
            }
        }

        if d.glow_amount > 0.0 {
            let sigma = glow_sigma(d, full.0.max(full.1) as f32);
            let blurred: Vec<Plane> = (0..3).into_par_iter().map(|c| gaussian_blur(&Plane::new(w, h, out.iter().map(|p| p[c]).collect()), sigma)).collect();
            let k = d.glow_amount / 100.0;
            out.par_iter_mut().enumerate().for_each(|(i, p)| *p = glow(*p, [blurred[0].data[i], blurred[1].data[i], blurred[2].data[i]], k));
        }

        if d.vignette_amount != 0.0 || d.grain_amount > 0.0 {
            let (cx, cy) = (full.0 as f32 / 2.0, full.1 as f32 / 2.0);
            let half_diag = (cx * cx + cy * cy).sqrt();
            let va = d.vignette_amount / 100.0;
            let grain = d.grain_amount / 100.0 * 0.12;
            // Grain cell size in full-resolution pixels.
            let cell = (1.0 + d.grain_size / 100.0 * 4.0) * scale;
            out.par_chunks_mut(w).enumerate().for_each(|(yy, line)| {
                let oy = (ext.y + yy) as f32 + 0.5;
                for (xx, p) in line.iter_mut().enumerate() {
                    let ox = (ext.x + xx) as f32 + 0.5;
                    if va != 0.0 {
                        let r = (((ox - cx).powi(2) + (oy - cy).powi(2)).sqrt() / half_diag).min(1.0);
                        let f = smoothstep(0.35, 1.05, r) * va.abs();
                        *p = if va < 0.0 { p.map(|v| v * (1.0 - f)) } else { p.map(|v| v + (1.0 - v) * f) };
                    }
                    if grain > 0.0 {
                        let n = value_noise(ox / cell, oy / cell, 17) * 0.7 + value_noise(ox / cell * 2.3, oy / cell * 2.3, 29) * 0.3;
                        let l = dot(*p, LUMA);
                        let amp = grain * (4.0 * l * (1.0 - l)).max(0.15);
                        *p = p.map(|v| (v + n * amp).clamp(0.0, 1.0));
                    }
                }
            });
        }

        // Crop the margin away.
        if ext == region {
            return (LinearImage::new(w, h, out), full, mask_plane);
        }
        let (ox, oy) = (region.x - ext.x, region.y - ext.y);
        let mut data = Vec::with_capacity(region.w * region.h);
        for y in 0..region.h {
            let start = (oy + y) * w + ox;
            data.extend_from_slice(&out[start..start + region.w]);
        }
        if let Some(mp) = &mut mask_plane {
            let mut cropped = Vec::with_capacity(region.w * region.h);
            for y in 0..region.h {
                let start = (oy + y) * w + ox;
                cropped.extend_from_slice(&mp[start..start + region.w]);
            }
            *mp = cropped;
        }
        (LinearImage::new(region.w, region.h, data), full, mask_plane)
    }

    /// Luminance / color range refinement of a mask, evaluated on the background result.
    fn range_weight(&self, mask: &Mask, disp: [f32; 3]) -> f32 {
        let mut w = 1.0;
        if let Some(r) = &mask.luma_range {
            let l = srgb_encode(dot(disp, LUMA).clamp(0.0, 1.0));
            let f = r.falloff.max(1e-3);
            let inside = smoothstep(r.low - f, r.low, l) * (1.0 - smoothstep(r.high, r.high + f, l));
            w *= if r.invert { 1.0 - inside } else { inside };
        }
        if let Some(c) = &mask.color_range {
            let [_, a, b] = self.oklab.from_linear(disp);
            let chroma = (a * a + b * b).sqrt();
            let hue = b.atan2(a).to_degrees().rem_euclid(360.0);
            let dh = ((hue - c.hue + 540.0).rem_euclid(360.0) - 180.0).abs();
            let hue_w = 1.0 - smoothstep(c.width, c.width + c.falloff.max(1e-3), dh);
            let chroma_w = smoothstep(c.min_chroma * 0.5, c.min_chroma.max(1e-3), chroma);
            w *= hue_w * chroma_w;
        }
        w
    }

    /// The source with the AI denoised data blended in, the camera shake removed and the lens
    /// blur, as far as wanted and available (`None`: the source as it is).
    fn prepared(&self, src: &LinearImage) -> Option<Arc<LinearImage>> {
        let denoised = self.ai_denoise.as_ref().and_then(|(path, amount)| crate::ai_denoise::blended(path, src, *amount));
        let base = denoised.as_deref().unwrap_or(src);
        let sharp = self.ai_deblur.as_ref().and_then(|(path, amount)| crate::ai_deblur::blended(path, base, *amount)).or(denoised);
        let base = sharp.as_deref().unwrap_or(src);
        let blurred = self.lens_blur.as_ref().and_then(|(path, p)| crate::lens_blur::blurred(path, base, p, !self.hdr));
        blurred.or(sharp)
    }

    /// Full processing into an output color space (encoded 0..1 floats), for export.
    pub fn process(&self, src: &LinearImage, region: Option<Region>, space: OutputSpace) -> LinearImage {
        let prepared = self.prepared(src);
        let src = prepared.as_deref().unwrap_or(src);
        if let Some(g) = crate::gpu::active() {
            match self.process_gpu(g, src, region, space) {
                Ok(img) => return img,
                Err(e) => log::warn!("Export auf dem Prozessor: {e:#}"),
            }
        }
        self.process_source_cpu(src, region, space)
    }

    /// [`Self::process`] without the graphics card.
    pub fn process_cpu(&self, src: &LinearImage, region: Option<Region>, space: OutputSpace) -> LinearImage {
        let prepared = self.prepared(src);
        self.process_source_cpu(prepared.as_deref().unwrap_or(src), region, space)
    }

    fn process_source_cpu(&self, src: &LinearImage, region: Option<Region>, space: OutputSpace) -> LinearImage {
        let (enc, _, _) = self.process_encoded(src, region, None);
        let m = space.from_working();
        let trc = space.trc();
        let map = self.adj.gamut_mapping();
        let data = enc
            .data
            .par_iter()
            .map(|p| {
                // From the table (interpolated: exact enough for 16 bits); the pixels stay in 0..1.
                let lin = p.map(|v| lut_lookup(&DISPLAY_DECODE, v));
                let out = mat_vec(&m, lin);
                let out = if map { crate::color::gamut_map(out, dot(lin, LUMA)) } else { out };
                // NaN survives `clamp`: no such value reaches an exported file.
                out.map(|v| trc.encode(if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }))
            })
            .collect();
        LinearImage::new(enc.width, enc.height, data)
    }

    /// Display rendering (sRGB, 8-bit) with histogram.
    pub fn render(&self, src: &LinearImage, region: Option<Region>, cache: Option<&StageCache>) -> Rendered {
        let prepared = self.prepared(src);
        let src = prepared.as_deref().unwrap_or(src);
        if let Some(g) = crate::gpu::active() {
            match self.render_gpu(g, src, region, cache) {
                Ok(r) => return r,
                Err(e) => log::warn!("Darstellung auf dem Prozessor: {e:#}"),
            }
        }
        let (enc, full, mask) = self.process_encoded(src, region, cache);
        self.display(enc, full, mask)
    }

    /// [`Self::render`] without the graphics card.
    pub fn render_cpu(&self, src: &LinearImage, region: Option<Region>, cache: Option<&StageCache>) -> Rendered {
        let prepared = self.prepared(src);
        let (enc, full, mask) = self.process_encoded(prepared.as_deref().unwrap_or(src), region, cache);
        self.display(enc, full, mask)
    }

    /// Encoded working RGB (sRGB transfer, 0..1) without the graphics card, the source as it is
    /// (no AI denoising or lens blur): for measuring renderings (see `auto`).
    pub(crate) fn encoded_cpu(&self, src: &LinearImage, cache: Option<&StageCache>) -> LinearImage {
        self.process_encoded(src, None, cache).0
    }

    /// Encoded working RGB -> display sRGB (or soft proof), clip warning, mask overlay, histogram.
    fn display(&self, enc: LinearImage, full: (usize, usize), mask: Option<Vec<f32>>) -> Rendered {
        let to_srgb = OutputSpace::Srgb.from_working();
        // Direct (not interpolated) tables: fine enough for 8-bit output and much faster.
        let (dec, enc8) = (&*DISPLAY_DECODE, &*DISPLAY_ENCODE_U8);
        let clip = self.opts.clip_warning;
        let map = self.adj.gamut_mapping();
        let w = enc.width;
        // Soft proof: through the proofing profile instead of straight to sRGB.
        let proofed: Option<Vec<[u8; 3]>> = self.opts.softproof.as_ref().map(|sp| {
            let mut out = vec![[0u8; 3]; enc.data.len()];
            out.par_chunks_mut(w * 16).zip(enc.data.par_chunks(w * 16)).for_each(|(o, s)| sp.apply(s, o));
            out
        });
        let mut rgba = vec![0u8; enc.data.len() * 4];
        rgba.par_chunks_mut(w * 4).zip(enc.data.par_chunks(w)).enumerate().for_each(|(y, (row, src))| {
            for (x, (out, p)) in row.as_chunks_mut::<4>().0.iter_mut().zip(src).enumerate() {
                let mut px = match &proofed {
                    Some(pr) => pr[y * w + x],
                    None => {
                        let lin = p.map(|v| dec[display_index(v)]);
                        let out = mat_vec(&to_srgb, lin);
                        let out = if map { crate::color::gamut_map(out, dot(lin, LUMA)) } else { out };
                        out.map(|v| enc8[display_index(v)])
                    }
                };
                if let Some(m) = &mask {
                    let a = (m[y * w + x] * 0.6).clamp(0.0, 0.6);
                    // The small offset keeps exact results (255 for a red pixel) from rounding down.
                    px = [px[0] as f32 * (1.0 - a) + 255.0 * a, px[1] as f32 * (1.0 - a), px[2] as f32 * (1.0 - a)].map(|v| (v + 1e-3) as u8);
                }
                if clip && (px[0] == 255 || px[1] == 255 || px[2] == 255) {
                    out.copy_from_slice(&[255, 0, 0, 255]);
                } else if clip && px == [0, 0, 0] {
                    out.copy_from_slice(&[0, 80, 255, 255]);
                } else {
                    out.copy_from_slice(&[px[0], px[1], px[2], 255]);
                }
            }
        });
        let histogram = histogram_rgba(&rgba);
        Rendered { width: enc.width, height: enc.height, rgba, histogram, full_size: full }
    }
}

/// Applies a tone curve to the largest and smallest channel; the middle one keeps its relative
/// position between them, so the hue stays (the DNG reference tone curve method). Applying the
/// curve to each channel on its own shifts hues towards the primaries as contrast rises.
#[inline]
fn hue_preserving(rgb: [f32; 3], curve: impl Fn(f32) -> f32) -> [f32; 3] {
    let [r, g, b] = rgb;
    let (hi, mid, lo) = if r >= g {
        if g >= b { (0, 1, 2) } else if r >= b { (0, 2, 1) } else { (2, 0, 1) }
    } else if r >= b {
        (1, 0, 2)
    } else if g >= b {
        (1, 2, 0)
    } else {
        (2, 1, 0)
    };
    let (h, l) = (curve(rgb[hi]), curve(rgb[lo]));
    let span = rgb[hi] - rgb[lo];
    let mut out = [0.0; 3];
    out[hi] = h;
    out[lo] = l;
    out[mid] = if span > 1e-9 { l + (h - l) * (rgb[mid] - rgb[lo]) / span } else { h };
    out
}

/// Radius of the median under the highlights/shadows base (process 3): 0.15 % of the long
/// edge, from the base's own radius (2 %); at most 24 pixels.
/// Blur of the glow in pixels, for an output with this long edge.
fn glow_sigma(d: &crate::params::Detail, long_edge: f32) -> f32 {
    long_edge * (0.003 + d.glow_size.clamp(0.0, 100.0) / 100.0 * 0.03)
}

/// Glow (Orton effect): the blurred image `b`, mostly its brighter parts, screened over the
/// encoded pixel `p` with strength `k` (0..1). Same as `glow.wgsl`.
#[inline]
fn glow(p: [f32; 3], b: [f32; 3], k: f32) -> [f32; 3] {
    let l = dot(b, LUMA);
    let g = k * 0.8 * smoothstep(0.05, 0.85, l);
    std::array::from_fn(|c| 1.0 - (1.0 - p[c]) * (1.0 - g * b[c].clamp(0.0, 1.0)))
}

fn median_radius(r_hdr: f32) -> usize {
    ((r_hdr * 0.075).round() as usize).min(24)
}

/// Size of the direct display tables.
const DISPLAY_LUT: usize = 16384;
/// sRGB-encoded 0..1 -> linear, indexed by [`display_index`] (or interpolated by `lut_lookup`).
static DISPLAY_DECODE: std::sync::LazyLock<Vec<f32>> =
    std::sync::LazyLock::new(|| (0..DISPLAY_LUT).map(|i| srgb_decode(i as f32 / (DISPLAY_LUT - 1) as f32)).collect());
/// Linear 0..1 -> sRGB-encoded 8-bit, indexed by [`display_index`].
static DISPLAY_ENCODE_U8: std::sync::LazyLock<Vec<u8>> =
    std::sync::LazyLock::new(|| (0..DISPLAY_LUT).map(|i| (srgb_encode(i as f32 / (DISPLAY_LUT - 1) as f32) * 255.0 + 0.5) as u8).collect());

#[inline]
fn display_index(v: f32) -> usize {
    (v.clamp(0.0, 1.0) * (DISPLAY_LUT - 1) as f32 + 0.5) as usize
}

/// How much a color (Oklab hue in degrees and chroma) belongs to the skin tone range.
#[inline]
fn skin_weight(st: &crate::params::SkinTone, h: f32, c: f32) -> f32 {
    let d = ((h - st.hue + 540.0).rem_euclid(360.0) - 180.0).abs();
    let range = st.range.max(1.0);
    let by_hue = 1.0 - smoothstep(range * 0.5, range, d);
    // Neither nearly neutral colors nor much more saturated ones (clothes, lips).
    let by_chroma = smoothstep(0.008, 0.025, c) * (1.0 - smoothstep(st.chroma * 2.0 + 0.03, st.chroma * 3.0 + 0.06, c));
    by_hue * by_chroma
}

/// Piecewise-linear weights over the circular hue bands (they sum to 1).
#[inline]
pub fn band_weights(h: f32) -> [f32; 8] {
    let mut w = [0.0f32; 8];
    let n = HUE_BANDS.len();
    for i in 0..n {
        let (c0, c1) = (HUE_BANDS[i].1, HUE_BANDS[(i + 1) % n].1);
        let span = (c1 - c0).rem_euclid(360.0);
        let d = (h - c0).rem_euclid(360.0);
        if d < span {
            let t = d / span;
            w[i] = 1.0 - t;
            w[(i + 1) % n] = t;
            return w;
        }
    }
    w[0] = 1.0;
    w
}

#[inline]
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Purple and green amounts of the defringing (0..1), `None` when it is off.
fn defringe_amounts(l: &crate::params::LensCorrection) -> Option<[f32; 2]> {
    (l.defringe_purple > 0.0 || l.defringe_green > 0.0).then(|| [l.defringe_purple, l.defringe_green].map(|v| v.clamp(0.0, 100.0) / 100.0))
}

/// Size (in pixels of a rendering at `scale`) of the neighbourhood in which defringing looks for
/// contrast: fringes reach a few sensor pixels beyond an edge.
fn defringe_sigma(scale: f32) -> f32 {
    (3.0 * scale).max(0.8)
}

/// Weight 1 for hues (degrees) inside `lo..hi`, falling to 0 over `soft` degrees outside.
fn hue_band(hue: f32, lo: f32, hi: f32, soft: f32) -> f32 {
    let d = if hue < lo { lo - hue } else { (hue - hi).max(0.0) };
    1.0 - smoothstep(0.0, soft, d)
}

/// One pixel of the defringing (also `defringe.wgsl`): `mean` and `mean_sq` are the blurred
/// perceptual luminance and its square, which give the local contrast.
#[inline]
fn defringe_pixel(p: [f32; 3], mean: f32, mean_sq: f32, [purple, green]: [f32; 2]) -> [f32; 3] {
    let y = dot(p, LUMA).max(0.0);
    let (cr, cb) = (p[0] - y, p[2] - y);
    if cr == 0.0 && cb == 0.0 {
        return p;
    }
    let edge = smoothstep(0.015, 0.06, (mean_sq - mean * mean).max(0.0).sqrt());
    // Hue in the color difference plane: magenta at 45°, blue at 95°, cyan at 160°, green at 225°.
    let hue = cb.atan2(cr).to_degrees().rem_euclid(360.0);
    let k = (edge * (purple * hue_band(hue, 25.0, 115.0, 20.0) + green * hue_band(hue, 195.0, 250.0, 20.0))).min(1.0);
    p.map(|v| v + (y - v) * k)
}

fn histogram_rgba(rgba: &[u8]) -> Histogram {
    let init = || Histogram { r: vec![0; 256], g: vec![0; 256], b: vec![0; 256], luma: vec![0; 256] };
    rgba.par_chunks(4 * 4096)
        .fold(init, |mut h, chunk| {
            for p in chunk.as_chunks::<4>().0 {
                h.r[p[0] as usize] += 1;
                h.g[p[1] as usize] += 1;
                h.b[p[2] as usize] += 1;
                let l = (p[0] as u32 * 54 + p[1] as u32 * 183 + p[2] as u32 * 19) >> 8;
                h.luma[l.min(255) as usize] += 1;
            }
            h
        })
        .reduce(init, |mut a, b| {
            for i in 0..256 {
                a.r[i] += b.r[i];
                a.g[i] += b.g[i];
                a.b[i] += b.b[i];
                a.luma[i] += b.luma[i];
            }
            a
        })
}

#[inline]
fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Highlights / shadows / whites / blacks as an exposure change in EV. `ev` is the
/// (locally smoothed) brightness, `ev_px` that of the pixel itself, both relative to mid grey.
fn hdr_gain_ev(hdr: [f32; 4], ev: f32, ev_px: f32) -> f32 {
    let [hi, sh, wh, bl] = hdr;
    let w_hi = sigmoid((ev - 0.5) * 1.6);
    let w_sh = sigmoid(-(ev + 2.0) * 1.4);
    let w_wh = sigmoid((ev_px - 2.0) * 2.5);
    let w_bl = sigmoid(-(ev_px + 4.5) * 2.0);
    -hi * 1.5 * w_hi + sh * 1.8 * w_sh + wh * 1.0 * w_wh + bl * 1.5 * w_bl
}

/// Contrast around mid grey, then the base curve.
fn contrast_and_base_curve(kind: BaseCurve, contrast: f32, v: f32) -> f32 {
    contrast_and_base_curve_unclipped(kind, contrast, v).clamp(0.0, 1.0)
}

/// Like [`contrast_and_base_curve`], but without clipping the linear curve at 1, so that a
/// table of it can be interpolated without a kink.
fn contrast_and_base_curve_unclipped(kind: BaseCurve, contrast: f32, v: f32) -> f32 {
    let v = contrast_warp(contrast, v);
    match kind {
        BaseCurve::Linear => v.max(0.0),
        _ => base_curve(kind, v),
    }
}

/// Contrast around mid grey (before the base curve).
#[inline]
fn contrast_warp(contrast: f32, v: f32) -> f32 {
    let e = contrast_exponent(contrast);
    if e != 1.0 { MID_GREY * (v.max(0.0) / MID_GREY).powf(e) } else { v }
}

/// Exponent of the contrast warp. Background and layer contrast add up, so the sum can
/// leave -100..100; the floor keeps tones ordered (at e <= 0 they would flatten or invert).
#[inline]
fn contrast_exponent(contrast: f32) -> f32 {
    (1.0 + contrast / 100.0 * 0.5).max(0.2)
}

/// EV range and resolution of the HDR tables (the weights are saturated beyond it).
const EV_MIN: f32 = -24.0;
const EV_MAX: f32 = 16.0;
const EV_STEPS: f32 = 128.0;
/// Scene values up to this are covered by the curve table (the base curves are flat beyond).
const CURVE_MAX: f32 = 256.0;
const CURVE_SIZE: usize = 16384;

/// Tables of a processor from a small cache by what they are made of (`key`): building them
/// costs about as much as rendering a small image, which auto adjustment and look matching do
/// many times with the same tables.
fn cached_lut<T: Send + Sync + 'static>(key: serde_json::Value, build: impl FnOnce() -> T) -> Arc<T> {
    use std::any::{Any, TypeId};
    type Entries = Vec<(TypeId, u64, Arc<dyn Any + Send + Sync>)>;
    static CACHE: Mutex<Entries> = Mutex::new(Vec::new());
    let key = crate::cache::fnv1a(key.to_string().as_bytes());
    let found = CACHE.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(t, k, _)| *t == TypeId::of::<T>() && *k == key).map(|(_, _, v)| v.clone());
    if let Some(v) = found.and_then(|v| v.downcast::<T>().ok()) {
        return v;
    }
    let v = Arc::new(build());
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.push((TypeId::of::<T>(), key, v.clone()));
    if cache.len() > 16 {
        cache.remove(0);
    }
    v
}

/// Tables that replace transcendental functions per pixel by a lookup.
struct ToneLuts {
    /// HDR gain in EV: the part depending on the smoothed brightness, and the part
    /// depending on the pixel itself (see [`hdr_gain_ev`]).
    hdr: Option<[Vec<f32>; 2]>,
    /// Contrast and base curve over sqrt(scene value), see [`curve_lookup`].
    curve: Option<Vec<f32>>,
    /// The camera's tone curve (without contrast, which warps its input), same layout.
    camera: Option<Vec<f32>>,
}

impl ToneLuts {
    fn new(base: &ToneParams, kind: BaseCurve, camera: Option<&crate::params::CameraCurve>) -> Self {
        let n = ((EV_MAX - EV_MIN) * EV_STEPS) as usize + 1;
        let ev_at = |i: usize| EV_MIN + i as f32 / EV_STEPS;
        let [hi, sh, wh, bl] = base.hdr;
        let hdr = base.hdr.iter().any(|v| *v != 0.0).then(|| {
            [
                (0..n).map(|i| hdr_gain_ev([hi, sh, 0.0, 0.0], ev_at(i), 0.0)).collect(),
                (0..n).map(|i| hdr_gain_ev([0.0, 0.0, wh, bl], 0.0, ev_at(i))).collect(),
            ]
        });
        let curve = (base.contrast != 0.0).then(|| {
            (0..CURVE_SIZE)
                .map(|i| {
                    let s = i as f32 / (CURVE_SIZE - 1) as f32 * CURVE_MAX.sqrt();
                    contrast_and_base_curve_unclipped(kind, base.contrast, s * s)
                })
                .collect()
        });
        let camera = camera.filter(|_| kind == BaseCurve::Camera).map(|c| {
            (0..CURVE_SIZE)
                .map(|i| {
                    let s = i as f32 / (CURVE_SIZE - 1) as f32 * CURVE_MAX.sqrt();
                    c.eval(s * s)
                })
                .collect()
        });
        Self { hdr, curve, camera }
    }
}

#[inline]
fn ev_lookup(lut: &[f32], ev: f32) -> f32 {
    let f = ((ev - EV_MIN) * EV_STEPS).clamp(0.0, (lut.len() - 1) as f32);
    let i = (f as usize).min(lut.len() - 2);
    let t = f - i as f32;
    lut[i] + (lut[i + 1] - lut[i]) * t
}

/// The table is indexed by sqrt(v), which gives the shadows more resolution.
#[inline]
fn curve_lookup(lut: &[f32], v: f32) -> f32 {
    let f = (v.max(0.0).sqrt() * ((CURVE_SIZE - 1) as f32 / CURVE_MAX.sqrt())).min((CURVE_SIZE - 1) as f32);
    let i = (f as usize).min(CURVE_SIZE - 2);
    let t = f - i as f32;
    (lut[i] + (lut[i + 1] - lut[i]) * t).clamp(0.0, 1.0)
}

/// Scene-linear to display-linear.
#[inline]
pub fn base_curve(kind: BaseCurve, x: f32) -> f32 {
    match kind {
        BaseCurve::Linear => x.clamp(0.0, 1.0),
        // The camera's curve is a table (see `ToneLuts`); without one, Film Standard.
        BaseCurve::FilmStandard | BaseCurve::Camera => aces_fit(x * 0.8),
        BaseCurve::FilmHigh => {
            let y = aces_fit(x * 0.8);
            // extra S-shape in display space
            let s = y * y * (3.0 - 2.0 * y);
            (y * 0.6 + s * 0.4).clamp(0.0, 1.0)
        }
    }
}

/// Narkowicz ACES filmic approximation.
#[inline]
fn aces_fit(x: f32) -> f32 {
    let (a, b, c, d, e) = (2.51, 0.03, 2.43, 0.59, 0.14);
    ((x * (a * x + b)) / (x * (c * x + d) + e)).clamp(0.0, 1.0)
}

fn build_output_luts(adj: &Adjustments) -> [Vec<f32>; 3] {
    let bright_gamma = 2f32.powf(-adj.brightness / 100.0 * 0.6);
    let master = adj.curves.rgb.build_lut(LUT_SIZE);
    let chans = [&adj.curves.red, &adj.curves.green, &adj.curves.blue].map(|c| c.build_lut(LUT_SIZE));
    std::array::from_fn(|c| {
        (0..LUT_SIZE)
            .map(|i| {
                let lin = i as f32 / (LUT_SIZE - 1) as f32;
                let mut v = srgb_encode(lin);
                if adj.brightness != 0.0 {
                    v = v.powf(bright_gamma);
                }
                v = adj.levels.apply(c, v);
                v = lut_lookup(&master, v);
                v = lut_lookup(&chans[c], v);
                v.clamp(0.0, 1.0)
            })
            .collect()
    })
}

/// The luma curve on encoded RGB: the luminance follows the curve, the channels are scaled with
/// it (same hue and saturation); a channel that would pass white pulls the color towards the
/// new gray instead, so the luminance still lands where the curve says.
#[inline]
fn apply_luma_curve(lut: &[f32], e: [f32; 3]) -> [f32; 3] {
    let y = dot(e, LUMA).clamp(0.0, 1.0);
    let y2 = lut_lookup(lut, y);
    if y < 1e-5 {
        return [y2; 3];
    }
    let mut out = e.map(|v| v * (y2 / y));
    let max = out[0].max(out[1]).max(out[2]);
    if max > 1.0 {
        let t = ((max - 1.0) / (max - y2).max(1e-6)).clamp(0.0, 1.0);
        out = out.map(|v| v + (y2 - v) * t);
    }
    out.map(|v| v.clamp(0.0, 1.0))
}

#[inline]
fn lut_lookup(lut: &[f32], v: f32) -> f32 {
    let f = v.clamp(0.0, 1.0) * (lut.len() - 1) as f32;
    let i = f as usize;
    if i >= lut.len() - 1 {
        return lut[lut.len() - 1];
    }
    let t = f - i as f32;
    lut[i] * (1.0 - t) + lut[i + 1] * t
}

/// Luminance helper for callers working in sRGB.
pub fn srgb_luminance(rgb: [f32; 3]) -> f32 {
    luminance(rgb)
}

#[cfg(test)]
mod tests {

    #[test]
    fn tone_tables_match_the_exact_functions() {
        for contrast in [-100.0, -30.0, 25.0, 100.0] {
            for kind in [BaseCurve::Linear, BaseCurve::FilmStandard, BaseCurve::FilmHigh] {
                let base = ToneParams {
                    exposure_ev: 0.0,
                    contrast,
                    hdr: [-0.6, 0.8, 0.3, -0.4],
                    clarity: 0.0,
                    structure: 0.0,
                    saturation: 0.0,
                    temperature: 0.0,
                    tint: 0.0,
                    brightness: 0.0,
                    dehaze: 0.0,
                    vibrance: 0.0,
                    veil: 0.0,
                    bands: [[0.0; 3]; 8],
                    balance: [[0.0; 3]; 4],
                };
                let luts = ToneLuts::new(&base, kind, None);
                let curve = luts.curve.as_ref().unwrap();
                for i in 0..4000 {
                    let v = (i as f32 / 4000.0 * 20.0 - 14.0).exp2();
                    let (exact, table) = (contrast_and_base_curve(kind, contrast, v), curve_lookup(curve, v));
                    assert!((exact - table).abs() < 1e-4, "Kurve {kind:?} {contrast} bei {v}: {exact} vs {table}");
                }
                let [by_ev, by_px] = luts.hdr.as_ref().unwrap();
                for i in 0..4000 {
                    let ev = i as f32 / 4000.0 * 40.0 - 26.0;
                    let px = ev * 0.7 + 1.0;
                    let (exact, table) = (hdr_gain_ev(base.hdr, ev, px), ev_lookup(by_ev, ev) + ev_lookup(by_px, px));
                    assert!((exact - table).abs() < 1e-4, "HDR bei {ev}/{px}: {exact} vs {table}");
                }
            }
        }
    }

    use super::*;
    use crate::color::{CameraColor, IDENTITY, REC2020_TO_XYZ, mat_inv};
    use crate::image::Orientation;
    use crate::params::{CropRect, Detail, HslBand};
    use crate::raw::CaptureInfo;
    use std::path::PathBuf;

    /// A camera whose native space is linear Rec.2020: camera values == working values.
    fn meta() -> RawMeta {
        let xyz_to_cam = mat_inv(&REC2020_TO_XYZ).unwrap();
        let mut color = CameraColor::new(xyz_to_cam);
        color.cam_to_working = IDENTITY;
        RawMeta {
            path: PathBuf::new(),
            info: CaptureInfo { width: 64, height: 48, ..Default::default() },
            color,
            as_shot_wb: [1.0, 1.0, 1.0],
            orientation: Orientation::Normal,
            hdr: false, lens: None, lens_corrected: false,
        }
    }

    /// No sharpening/NR so that single pixels can be checked.
    fn plain() -> Adjustments {
        Adjustments { detail: Detail { sharpen_amount: 0.0, nr_color: 0.0, ..Default::default() }, ..Default::default() }
    }

    fn px(p: &Processor, v: [f32; 3]) -> [f32; 3] {
        p.encode(p.color(p.tone(p.to_working(v), None, [0.0; 2], &p.base), &p.base))
    }

    /// The decoding table, interpolated, is exact enough for 16-bit files.
    #[test]
    fn interpolated_decode_table_matches_srgb_decode() {
        for i in 0..=100_000 {
            let v = i as f32 / 100_000.0;
            let (table, exact) = (lut_lookup(&DISPLAY_DECODE, v), srgb_decode(v));
            assert!((table - exact).abs() < 1e-6, "{v}: {table} vs {exact}");
        }
    }

    #[test]
    fn neutral_stays_neutral_and_mid_grey_is_mid() {
        let p = Processor::new(&meta(), &plain());
        let out = px(&p, [0.18; 3]);
        assert!((out[0] - out[1]).abs() < 1e-4 && (out[1] - out[2]).abs() < 1e-4);
        assert!(out[0] > 0.4 && out[0] < 0.6, "mid grey encoded to {}", out[0]);
    }

    #[test]
    fn exposure_brightens() {
        let a = Processor::new(&meta(), &plain());
        let b = Processor::new(&meta(), &Adjustments { exposure: 1.0, ..plain() });
        assert!(px(&b, [0.1; 3])[0] > px(&a, [0.1; 3])[0]);
    }

    #[test]
    fn shadows_lift_dark_tones_more_than_highlights() {
        let a = Processor::new(&meta(), &plain());
        let b = Processor::new(&meta(), &Adjustments { shadows: 100.0, ..plain() });
        let dark = px(&b, [0.01; 3])[0] / px(&a, [0.01; 3])[0];
        let bright = px(&b, [0.8; 3])[0] / px(&a, [0.8; 3])[0];
        assert!(dark > 1.3 && bright < 1.05, "dark {dark} bright {bright}");
    }

    #[test]
    fn highlight_recovery_darkens_highlights() {
        let a = Processor::new(&meta(), &plain());
        let b = Processor::new(&meta(), &Adjustments { highlights: 100.0, ..plain() });
        assert!(px(&b, [0.9; 3])[0] < px(&a, [0.9; 3])[0]);
    }

    #[test]
    fn clipped_highlights_stay_neutral() {
        let mut m = meta();
        m.as_shot_wb = [2.0, 1.0, 1.6];
        let p = Processor::new(&m, &plain());
        let w = p.to_working([1.0, 1.0, 1.0]);
        assert!((w[0] - w[1]).abs() < 1e-4 && (w[1] - w[2]).abs() < 1e-4, "{w:?}");
        // Unclipped pixels keep their white balance.
        let u = p.to_working([0.2, 0.2, 0.2]);
        assert!((u[0] - 0.4).abs() < 1e-4 && (u[2] - 0.32).abs() < 1e-4);
    }

    #[test]
    fn crop_and_rotation_change_output_size() {
        let src = LinearImage::new(64, 48, vec![[0.1; 3]; 64 * 48]);
        let adj = Adjustments {
            rotation: 1,
            geometry: crate::params::Geometry { crop: CropRect { x: 0.0, y: 0.0, w: 0.5, h: 0.5 }, ..Default::default() },
            ..plain()
        };
        let r = Processor::new(&meta(), &adj).render(&src, None, None);
        assert_eq!((r.width, r.height), (24, 32));
        assert_eq!(r.histogram.luma.iter().sum::<u32>(), 24 * 32);
    }

    #[test]
    fn hidden_tools_are_left_out_of_the_rendering() {
        let src = LinearImage::new(64, 48, vec![[0.1; 3]; 64 * 48]);
        let mut adj = Adjustments {
            exposure: 1.0,
            geometry: crate::params::Geometry { crop: CropRect { x: 0.0, y: 0.0, w: 0.5, h: 0.5 }, ..Default::default() },
            ..plain()
        };
        adj.hidden = vec![crate::tools::Tool::Crop, crate::tools::Tool::Exposure];
        let r = Processor::new(&meta(), &adj).render(&src, None, None);
        let plain_r = Processor::new(&meta(), &plain()).render(&src, None, None);
        assert_eq!((r.width, r.height), (64, 48));
        assert_eq!(r.rgba, plain_r.rgba);
    }

    /// Source and adjustments with every neighborhood operation and a painted layer.
    fn neighborhood_scene() -> (LinearImage, Adjustments) {
        let (w, h) = (200, 150);
        let src = LinearImage::new(
            w,
            h,
            (0..w * h).map(|i| {
                let v = 0.05 + 0.3 * (((i % w) / 9 + (i / w) / 7) % 2) as f32 + 0.002 * (i % w) as f32;
                [v, v * 0.9, v * 0.7]
            }).collect(),
        );
        let mut adj = Adjustments {
            shadows: 40.0,
            detail: Detail {
                clarity: 50.0,
                structure: 30.0,
                dehaze: 40.0,
                sharpen_amount: 150.0,
                sharpen_radius: 1.0,
                nr_luma: 30.0,
                nr_color: 30.0,
                glow_amount: 60.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let stroke = crate::params::BrushStroke { points: vec![[0.3, 0.4], [0.6, 0.5]], radius: 0.08, hardness: 0.3, flow: 1.0, erase: false, opacity: 1.0, auto_mask: false, build_up: true };
        let mut layer = crate::params::Layer { adjustments: LayerAdjustments { exposure: 1.0, clarity: 30.0, ..Default::default() }, ..Default::default() };
        layer.mask.strokes.push(stroke);
        adj.layers.push(layer);
        adj.assign_layer_ids();
        (src, adj)
    }

    fn max_region_diff(part: &Rendered, full: &Rendered, region: Region) -> i32 {
        let mut max_diff = 0i32;
        for y in 0..region.h {
            for x in 0..region.w {
                for c in 0..3 {
                    let a = part.rgba[(y * region.w + x) * 4 + c] as i32;
                    let b = full.rgba[((y + region.y) * full.width + x + region.x) * 4 + c] as i32;
                    max_diff = max_diff.max((a - b).abs());
                }
            }
        }
        max_diff
    }

    /// The resampled image with heal/clone and the layers' source positions stay cached when
    /// the white balance changes, and the result is that of a rendering without cache.
    #[test]
    fn resampling_stays_cached_across_white_balance() {
        let (src, mut adj) = neighborhood_scene();
        adj.geometry.straighten = 3.0;
        adj.retouch = vec![crate::params::Spot {
            mode: crate::params::SpotMode::Clone,
            target: [0.3, 0.3],
            source: [0.6, 0.6],
            path: vec![],
            strokes: Vec::new(),
            radius: 0.05,
            feather: 0.5,
            opacity: 1.0,
        }];
        let cache = StageCache::default();
        Processor::new(&meta(), &adj).render_cpu(&src, None, Some(&cache));
        let keys = |c: &StageCache| (c.resampled.lock().unwrap_or_else(|e| e.into_inner()).as_ref().unwrap().0, c.uv.lock().unwrap_or_else(|e| e.into_inner()).as_ref().unwrap().0);
        let before = keys(&cache);
        adj.white_balance.mode = crate::params::WbMode::Custom;
        adj.white_balance.kelvin = 3500.0;
        let p = Processor::new(&meta(), &adj);
        let cached = p.render_cpu(&src, None, Some(&cache));
        assert_eq!(keys(&cache), before, "not resampled again");
        assert_eq!(cached.rgba, p.render_cpu(&src, None, None).rgba);
    }

    /// With the whole image in the cache, a part of it (what the brush changed) is cut out of
    /// the cached stages: exactly the whole rendering there, and the cache keeps the whole.
    #[test]
    fn part_with_cached_whole_matches_the_whole_rendering() {
        let (src, adj) = neighborhood_scene();
        let p = Processor::new(&meta(), &adj);
        let cache = StageCache::default();
        let full = p.render_cpu(&src, None, Some(&cache));
        let whole_key = cache.entry.lock().unwrap_or_else(|e| e.into_inner()).as_ref().unwrap().0;
        for region in [Region { x: 50, y: 40, w: 40, h: 30 }, Region { x: 0, y: 0, w: 25, h: 20 }, Region { x: 170, y: 120, w: 30, h: 30 }] {
            let part = p.render_cpu(&src, Some(region), Some(&cache));
            assert_eq!((part.width, part.height), (region.w, region.h));
            assert!(max_region_diff(&part, &full, region) <= 1, "{region:?}: {}", max_region_diff(&part, &full, region));
        }
        assert_eq!(cache.entry.lock().unwrap_or_else(|e| e.into_inner()).as_ref().unwrap().0, whole_key, "the whole stays cached");
    }

    #[test]
    fn tiles_match_full_render_with_neighborhood_ops() {
        let src = LinearImage::new(
            64,
            48,
            (0..64 * 48).map(|i| {
                let v = 0.05 + 0.3 * (((i % 64) / 8 + (i / 64) / 8) % 2) as f32;
                [v, v * 0.9, v * 0.7]
            }).collect(),
        );
        let adj = Adjustments {
            shadows: 40.0,
            detail: Detail { clarity: 50.0, structure: 30.0, sharpen_amount: 150.0, sharpen_radius: 1.0, nr_luma: 30.0, nr_color: 30.0, glow_amount: 60.0, ..Default::default() },
            ..Default::default()
        };
        let p = Processor::new(&meta(), &adj);
        let full = p.render(&src, None, None);
        let region = Region { x: 20, y: 10, w: 16, h: 12 };
        let tile = p.render(&src, Some(region), None);
        let mut max_diff = 0i32;
        for y in 0..region.h {
            for x in 0..region.w {
                for c in 0..3 {
                    let a = tile.rgba[(y * region.w + x) * 4 + c] as i32;
                    let b = full.rgba[((y + region.y) * full.width + x + region.x) * 4 + c] as i32;
                    max_diff = max_diff.max((a - b).abs());
                }
            }
        }
        assert!(max_diff <= 2, "tile differs from full render by {max_diff}");
    }

    /// A grey image whose left half is near and right half far (by its depth map).
    fn near_and_far() -> (LinearImage, Arc<crate::depth::DepthMap>) {
        let src = LinearImage::new(64, 32, vec![[0.15, 0.15, 0.15]; 64 * 32]);
        let map = crate::depth::DepthMap { width: 64, height: 32, data: (0..64 * 32).map(|i| if i % 64 < 32 { 1.0 } else { 0.0 }).collect() };
        (src, Arc::new(map))
    }

    fn with_depth_layers(adj: &Adjustments, map: &Arc<crate::depth::DepthMap>) -> Processor {
        let mut p = Processor::new(&meta(), adj);
        p.layers.splice(0..0, depth_layers_with(map, adj));
        p.haze_map = (!adj.atmosphere.is_off()).then(|| map.clone());
        p
    }

    /// Mean of a row's left and right ends (near and far part).
    fn ends(r: &Rendered) -> ([f32; 3], [f32; 3]) {
        let at = |x: usize| -> [f32; 3] { std::array::from_fn(|c| r.rgba[(r.height / 2 * r.width + x) * 4 + c] as f32) };
        (at(4), at(r.width - 5))
    }

    #[test]
    fn relight_lights_near_and_far_separately() {
        let (src, map) = near_and_far();
        let plain = ends(&Processor::new(&meta(), &Adjustments::default()).render_cpu(&src, None, None));
        let adj = Adjustments { relight: crate::params::Relight { near: 1.0, far_warmth: 60.0, ..Default::default() }, ..Default::default() };
        let (near, far) = ends(&with_depth_layers(&adj, &map).render_cpu(&src, None, None));
        assert!(near[1] > plain.0[1] + 20.0, "Vordergrund {near:?} statt {:?}", plain.0);
        assert!((near[0] - near[2]).abs() < 2.0, "Vordergrund bleibt neutral: {near:?}");
        assert!((far[1] - plain.1[1]).abs() < 4.0, "Hintergrund {far:?} statt {:?}", plain.1);
        assert!(far[0] > far[2] + 5.0, "Hintergrund wärmer: {far:?}");
        // Off: no layers, even with a map.
        assert!(depth_layers_with(&map, &Adjustments::default()).is_empty());
    }

    #[test]
    fn atmosphere_hazes_only_the_far_part() {
        let (mut src, map) = near_and_far();
        // Texture, so that the haze shows as lower contrast.
        for (i, p) in src.data.iter_mut().enumerate() {
            if (i % 64 / 2 + i / 64 / 2) % 2 == 0 {
                *p = [0.02, 0.02, 0.02];
            }
        }
        let contrast = |r: &Rendered, x0: usize| {
            let row = r.height / 2;
            let v: Vec<f32> = (x0..x0 + 8).map(|x| r.rgba[(row * r.width + x) * 4 + 1] as f32).collect();
            v.iter().cloned().fold(0.0, f32::max) - v.iter().cloned().fold(255.0, f32::min)
        };
        let plain = Processor::new(&meta(), &Adjustments::default()).render_cpu(&src, None, None);
        let adj = Adjustments { atmosphere: crate::params::Atmosphere { amount: 80.0, reach: 0.3, ..Default::default() }, ..Default::default() };
        let hazy = with_depth_layers(&adj, &map).render_cpu(&src, None, None);
        assert!(contrast(&hazy, 48) < contrast(&plain, 48) * 0.8, "fern: {} statt {}", contrast(&hazy, 48), contrast(&plain, 48));
        assert!((contrast(&hazy, 4) - contrast(&plain, 4)).abs() < 3.0, "nah: {} statt {}", contrast(&hazy, 4), contrast(&plain, 4));
    }

    #[test]
    fn atmosphere_takes_its_color_from_the_distance() {
        // The far half: dark blue-gray with a few blown-out highlights (a backlit sky).
        let (mut src, map) = near_and_far();
        for (i, p) in src.data.iter_mut().enumerate() {
            if i % 64 >= 32 {
                *p = if i % 7 == 0 { [1.0; 3] } else { [0.08, 0.1, 0.14] };
            }
        }
        let veil = |at: crate::params::Atmosphere| with_depth_layers(&Adjustments { atmosphere: at, ..Default::default() }, &map).veil(&src);
        let at = crate::params::Atmosphere { amount: 80.0, ..Default::default() };
        let v = veil(at);
        let y = dot(v, LUMA);
        // Brighter than the far part's average, far from the white of the highlights, bluish.
        assert!(y > 0.1 && y < 0.5, "Helligkeit {y}");
        assert!(v[2] > v[0], "Farbe der Ferne: {v:?}");
        let brighter = veil(crate::params::Atmosphere { brightness: 50.0, ..at });
        assert!(dot(brighter, LUMA) > y * 1.3);
        let warm = veil(crate::params::Atmosphere { warmth: 60.0, ..at });
        assert!(warm[0] / warm[2] > v[0] / v[2] * 1.2, "wärmer: {warm:?} statt {v:?}");
        assert!((dot(warm, LUMA) - y).abs() < 0.01 * y.max(0.1) + 1e-4, "Wärme ändert die Helligkeit nicht");
    }

    #[test]
    fn glow_brightens_around_highlights() {
        // A bright square on a dark ground: the glow spreads light beyond its edge.
        let src = LinearImage::new(96, 64, (0..96 * 64).map(|i| if (40..56).contains(&(i % 96)) && (24..40).contains(&(i / 96)) { [0.9; 3] } else { [0.01; 3] }).collect());
        let at = |r: &Rendered, x: usize| r.rgba[(32 * r.width + x) * 4 + 1];
        let unsharp = Adjustments { detail: Detail { sharpen_amount: 0.0, ..Default::default() }, ..Default::default() };
        let plain = Processor::new(&meta(), &unsharp).render_cpu(&src, None, None);
        let mut adj = unsharp.clone();
        (adj.detail.glow_amount, adj.detail.glow_size) = (80.0, 100.0);
        let glow = Processor::new(&meta(), &adj).render_cpu(&src, None, None);
        assert!(at(&glow, 58) > at(&plain, 58) + 3, "neben dem Quadrat {} statt {}", at(&glow, 58), at(&plain, 58));
        assert!(at(&glow, 48) > at(&plain, 48), "im Quadrat {} statt {}", at(&glow, 48), at(&plain, 48));
        assert!(at(&glow, 2) <= at(&plain, 2) + 1, "weit weg {} statt {}", at(&glow, 2), at(&plain, 2));
    }

    #[test]
    fn invalid_values_do_not_reach_the_export() {
        let (w, h) = (64, 48);
        let mut data = vec![[0.3f32; 3]; w * h];
        data[w * 20 + 30] = [f32::NAN, f32::INFINITY, 0.3];
        let src = LinearImage::new(w, h, data);
        let adj = Adjustments { detail: Detail { clarity: 40.0, ..Default::default() }, ..Default::default() };
        let out = Processor::new(&meta(), &adj).process_cpu(&src, None, OutputSpace::Srgb);
        assert!(out.data.iter().flatten().all(|v| v.is_finite()));
    }

    #[test]
    fn dehaze_tiles_match_full_render() {
        // Bright haze on the left, dark ground on the right: a tile of the ground alone would
        // estimate a much darker airlight. The tile sits on the edge, off the dark channel's
        // block grid (8 px here), so blocks must be counted from the image's origin.
        let (w, h) = (2048, 128);
        let edge = 1900;
        let src = LinearImage::new(
            w,
            h,
            (0..w * h)
                .map(|i| {
                    let (x, y) = (i % w, i / w);
                    let base = if x < edge { 0.8 } else { 0.08 };
                    let v = base + 0.04 * (((x / 4 + y / 4) % 2) as f32);
                    [v, v * 0.95, v * 0.9]
                })
                .collect(),
        );
        let adj = Adjustments { detail: Detail { dehaze: 60.0, ..Default::default() }, ..Default::default() };
        let p = Processor::new(&meta(), &adj);
        let region = Region { x: 1889, y: 51, w: 24, h: 24 };
        // On the processor and (if there is one) on the graphics card.
        type Render<'a> = &'a dyn Fn(Option<Region>) -> Rendered;
        let renderers: [(&str, Render); 2] = [("CPU", &|r| p.render_cpu(&src, r, None)), ("GPU", &|r| p.render(&src, r, None))];
        for (name, render) in renderers {
            let full = render(None);
            let tile = render(Some(region));
            let mut max_diff = 0i32;
            for y in 0..region.h {
                for x in 0..region.w {
                    for c in 0..3 {
                        let a = tile.rgba[(y * region.w + x) * 4 + c] as i32;
                        let b = full.rgba[((y + region.y) * full.width + x + region.x) * 4 + c] as i32;
                        max_diff = max_diff.max((a - b).abs());
                    }
                }
            }
            assert!(max_diff <= 3, "{name}: Kachel weicht um {max_diff} vom ganzen Bild ab");
        }
    }

    #[test]
    fn dehaze_uses_one_airlight_for_parts_and_whole() {
        // Sparse bright specks: at full resolution they set the airlight, in the small copy
        // they average away. Parts and the whole must use the same estimate anyway.
        let (w, h) = (2048, 96);
        let src = LinearImage::new(
            w,
            h,
            (0..w * h).map(|i| if i % 97 == 0 { [3.0; 3] } else { [0.3 + 0.05 * ((i % w / 8 + i / w / 8) % 2) as f32; 3] }).collect(),
        );
        let adj = Adjustments { detail: Detail { dehaze: 60.0, ..Default::default() }, ..Default::default() };
        let p = Processor::new(&meta(), &adj);
        let full = p.render_cpu(&src, None, None);
        let region = Region { x: 600, y: 30, w: 40, h: 30 };
        let part = p.render_cpu(&src, Some(region), None);
        assert!(max_region_diff(&part, &full, region) <= 1, "{}", max_region_diff(&part, &full, region));
    }

    /// The stages run one after another, so their reaches add up; and the fast guided filter's
    /// blocks (3 px here) are counted from the whole image's origin.
    #[test]
    fn parts_match_whole_with_chained_neighborhood_ops() {
        let (w, h) = (1300, 72);
        let src = LinearImage::new(
            w,
            h,
            (0..w * h)
                .map(|i| {
                    let (x, y) = (i % w, i / w);
                    let v = 0.03 + 0.4 * ((x / 5 + y / 7) % 2) as f32 + 0.2 * crate::detail::hash_noise(x as i64, y as i64, 5).abs();
                    [v, v * 0.9, v * 0.7]
                })
                .collect(),
        );
        let adj = Adjustments {
            shadows: 50.0,
            highlights: -40.0,
            detail: Detail { clarity: 60.0, sharpen_amount: 150.0, sharpen_radius: 1.0, nr_luma: 40.0, nr_color: 40.0, ..Default::default() },
            ..Default::default()
        };
        let p = Processor::new(&meta(), &adj);
        assert_eq!(p.plan(&src, None).guided_sub, 3);
        let full = p.render_cpu(&src, None, None);
        for region in [Region { x: 601, y: 20, w: 37, h: 31 }, Region { x: 1000, y: 0, w: 50, h: 72 }] {
            let part = p.render_cpu(&src, Some(region), None);
            assert!(max_region_diff(&part, &full, region) <= 1, "{region:?}: {}", max_region_diff(&part, &full, region));
        }
    }

    #[test]
    fn stage_cache_gives_identical_results() {
        let src = LinearImage::new(32, 24, (0..768).map(|i| [(i % 32) as f32 / 40.0; 3]).collect());
        let cache = StageCache::default();
        let a = Processor::new(&meta(), &Adjustments { exposure: 0.5, ..Default::default() });
        let b = Processor::new(&meta(), &Adjustments { exposure: 0.5, contrast: 20.0, ..Default::default() });
        let r1 = a.render(&src, None, Some(&cache));
        let r2 = b.render(&src, None, Some(&cache));
        assert_eq!(r1.rgba, a.render(&src, None, None).rgba);
        assert_eq!(r2.rgba, b.render(&src, None, None).rgba);
    }

    #[test]
    fn band_weights_partition_unity() {
        for h in (0..360).step_by(7) {
            let w = band_weights(h as f32);
            assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        }
        assert!(band_weights(25.0)[0] > 0.99);
    }

    #[test]
    fn color_editor_targets_its_band() {
        let mut adj = plain();
        adj.color_editor.bands[5] = HslBand { saturation: -100.0, ..Default::default() }; // blue
        let p = Processor::new(&meta(), &adj);
        let base = Processor::new(&meta(), &plain());
        let ok = Oklab::default();
        let chroma = |v: [f32; 3]| {
            let l = ok.from_linear(v.map(srgb_decode));
            (l[1] * l[1] + l[2] * l[2]).sqrt()
        };
        let blue = [0.02, 0.05, 0.4];
        let red = [0.4, 0.03, 0.02];
        assert!(chroma(px(&p, blue)) < chroma(px(&base, blue)) * 0.3);
        assert!((chroma(px(&p, red)) - chroma(px(&base, red))).abs() < 0.01);
    }

    #[test]
    fn luma_curve_changes_brightness_not_color() {
        let base = Processor::new(&meta(), &plain());
        let mut adj = plain();
        // Brighter midtones.
        adj.curves.luma = crate::params::Curve { points: vec![[0.0, 0.0], [0.4, 0.55], [1.0, 1.0]] };
        let p = Processor::new(&meta(), &adj);
        let ok = Oklab::default();
        let lch = |v: [f32; 3]| {
            let [l, a, b] = ok.from_linear(v.map(srgb_decode));
            (l, (a * a + b * b).sqrt(), b.atan2(a).to_degrees())
        };
        for v in [[0.2, 0.08, 0.04], [0.03, 0.06, 0.15], [0.1, 0.1, 0.1]] {
            let ((l0, c0, h0), (l1, c1, h1)) = (lch(px(&base, v)), lch(px(&p, v)));
            assert!(l1 > l0 + 0.03, "{v:?}: {l0} -> {l1}");
            if c0 > 0.02 {
                assert!((h1 - h0).abs() < 1.5, "{v:?}: Farbton {h0} -> {h1}");
                // Saturation (chroma relative to lightness) stays about the same.
                assert!(((c1 / l1) / (c0 / l0) - 1.0).abs() < 0.12, "{v:?}: {} -> {}", c0 / l0, c1 / l1);
            } else {
                assert!(c1 < 0.01);
            }
        }
        // A bright saturated color that would pass white stays inside, with its hue.
        let lut = crate::params::Curve { points: vec![[0.0, 0.0], [0.5, 0.9], [1.0, 1.0]] }.build_lut(LUT_SIZE);
        let out = apply_luma_curve(&lut, [1.0, 0.5, 0.2]);
        assert!(out.iter().all(|v| (0.0..=1.0).contains(v)) && out[0] >= out[1] && out[1] >= out[2]);
        assert!((dot(out, LUMA) - lut_lookup(&lut, dot([1.0, 0.5, 0.2], LUMA))).abs() < 1e-3);
        // A straight line costs nothing.
        assert!(base.luma_lut.is_none());
    }

    #[test]
    fn color_zone_changes_only_its_range() {
        let base = Processor::new(&meta(), &plain());
        let hue_of = |p: &Processor, v: [f32; 3]| {
            let [_, a, b] = p.oklab.from_linear(px(p, v).map(srgb_decode));
            (b.atan2(a).to_degrees().rem_euclid(360.0), (a * a + b * b).sqrt())
        };
        // A narrow range around an orange; a yellow 40° away stays.
        let (orange, yellow) = ([0.5, 0.2, 0.03], [0.45, 0.4, 0.03]);
        let (h_orange, _) = hue_of(&base, orange);
        let (h_yellow, c_yellow) = hue_of(&base, yellow);
        assert!((h_yellow - h_orange).abs() > 30.0);
        let mut adj = plain();
        let zone = crate::params::ColorZone { hue: h_orange, width: 15.0, softness: 50.0, adjust: HslBand { saturation: -100.0, ..Default::default() }, ..Default::default() };
        adj.color_editor.zones.push(zone);
        let p = Processor::new(&meta(), &adj);
        assert!(hue_of(&p, orange).1 < 0.01, "{:?}", hue_of(&p, orange));
        let (h, c) = hue_of(&p, yellow);
        assert!((h - h_yellow).abs() < 0.5 && (c - c_yellow).abs() < 0.002);
        // Switched off, it does nothing (and no hue work is done at all).
        adj.color_editor.zones[0].enabled = false;
        assert!(adj.color_editor.is_neutral());
        // The weight: full inside, soft edge, nothing beyond, across 0°.
        let z = crate::params::ColorZone { hue: 355.0, width: 20.0, softness: 50.0, ..Default::default() };
        assert_eq!(z.weight(5.0), 1.0);
        assert!(z.weight(10.0) > 0.0 && z.weight(10.0) < 1.0);
        assert_eq!(z.weight(16.0), 0.0);
    }

    #[test]
    fn skin_tone_uniformity_evens_out_skin_only() {
        let hue_chroma = |p: &Processor, v: [f32; 3]| {
            let [_, a, b] = p.oklab.from_linear(px(p, v).map(srgb_decode));
            (b.atan2(a).to_degrees().rem_euclid(360.0), (a * a + b * b).sqrt())
        };
        // A reddish and a yellowish skin tone, and a blue.
        let (pink, tan, blue) = ([0.5, 0.25, 0.2], [0.45, 0.32, 0.16], [0.02, 0.05, 0.4]);
        let base = Processor::new(&meta(), &plain());
        let target = base.color_input([0.47, 0.29, 0.18]);
        let mut adj = plain();
        adj.skin_tone = crate::params::SkinTone {
            hue: target[2].atan2(target[1]).to_degrees().rem_euclid(360.0),
            chroma: (target[1] * target[1] + target[2] * target[2]).sqrt(),
            lightness: target[0],
            range: 40.0,
            hue_uniformity: 100.0,
            saturation_uniformity: 100.0,
            ..Default::default()
        };
        let p = Processor::new(&meta(), &adj);
        let spread = |p: &Processor| {
            let (a, b) = (hue_chroma(p, pink), hue_chroma(p, tan));
            ((a.0 - b.0).abs(), (a.1 - b.1).abs())
        };
        let (before, after) = (spread(&base), spread(&p));
        assert!(after.0 < before.0 * 0.4, "hue spread {before:?} -> {after:?}");
        let (b0, b1) = (hue_chroma(&base, blue), hue_chroma(&p, blue));
        assert!((b0.0 - b1.0).abs() < 0.5 && (b0.1 - b1.1).abs() < 0.002, "{b0:?} {b1:?}");
    }

    #[test]
    fn black_white_removes_color() {
        let mut adj = plain();
        adj.black_white.enabled = true;
        let p = Processor::new(&meta(), &adj);
        let o = px(&p, [0.4, 0.1, 0.05]);
        assert!((o[0] - o[1]).abs() < 0.01 && (o[1] - o[2]).abs() < 0.01, "{o:?}");
    }

    fn layer(mask: Mask, adj: LayerAdjustments) -> crate::params::Layer {
        crate::params::Layer { mask, adjustments: adj, ..Default::default() }
    }

    #[test]
    fn filled_layer_equals_global_adjustment() {
        let src = LinearImage::new(16, 12, (0..192).map(|i| [(i % 16) as f32 / 40.0 + 0.01; 3]).collect());
        let global = Processor::new(&meta(), &Adjustments { exposure: 1.0, contrast: 20.0, ..plain() }).render(&src, None, None);
        let mut adj = plain();
        adj.layers.push(layer(Mask { fill: true, ..Default::default() }, LayerAdjustments { exposure: 1.0, contrast: 20.0, ..Default::default() }));
        let local = Processor::new(&meta(), &adj).render(&src, None, None);
        let max_diff = global.rgba.iter().zip(&local.rgba).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(max_diff <= 1, "{max_diff}");
    }

    #[test]
    fn summed_layer_contrast_keeps_tones_ordered() {
        // Background -100 plus two filled layers at -100: the sum (-300) must not flatten or invert.
        let src = LinearImage::new(16, 1, (0..16).map(|i| [0.005 * 1.5f32.powi(i); 3]).collect());
        let mut adj = Adjustments { contrast: -100.0, ..plain() };
        for _ in 0..2 {
            adj.layers.push(layer(Mask { fill: true, ..Default::default() }, LayerAdjustments { contrast: -100.0, ..Default::default() }));
        }
        let out = Processor::new(&meta(), &adj).render(&src, None, None);
        let g: Vec<u8> = out.rgba.chunks(4).map(|p| p[1]).collect();
        assert!(g.windows(2).all(|w| w[0] <= w[1]), "{g:?}");
        assert!(g[15] > g[0] + 20, "{g:?}");
        for c in [-300.0, -200.0, -160.0, 100.0] {
            let (a, b) = (contrast_warp(c, 0.05), contrast_warp(c, 0.5));
            assert!(a.is_finite() && b.is_finite() && a < b, "{c}: {a} {b}");
        }
    }

    /// A colorful test image: hues around the circle, from dark to bright.
    fn colorful() -> LinearImage {
        let data = (0..24 * 16)
            .map(|i| {
                let (x, y) = ((i % 24) as f32, (i / 24) as f32);
                let h = x / 24.0 * std::f32::consts::TAU;
                let v = 0.02 + y / 16.0 * 0.6;
                [v * (1.0 + 0.8 * h.cos()), v * (1.0 + 0.8 * (h - 2.1).cos()), v * (1.0 + 0.8 * (h + 2.1).cos())]
            })
            .collect();
        LinearImage::new(24, 16, data)
    }

    fn max_diff(a: &Rendered, b: &Rendered) -> i32 {
        a.rgba.iter().zip(&b.rgba).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap()
    }

    #[test]
    fn filled_layer_equals_global_tone_and_color() {
        use crate::params::ColorWheel;
        let src = colorful();
        let filled = Mask { fill: true, ..Default::default() };
        let wheel = ColorWheel { hue: 210.0, amount: 40.0, lightness: 10.0 };
        let band = HslBand { hue: 10.0, saturation: -40.0, lightness: 20.0 };
        let cases: Vec<(Adjustments, LayerAdjustments)> = vec![
            (Adjustments { whites: 40.0, ..plain() }, LayerAdjustments { whites: 40.0, ..Default::default() }),
            (Adjustments { blacks: 40.0, ..plain() }, LayerAdjustments { blacks: 40.0, ..Default::default() }),
            (Adjustments { vibrance: 50.0, ..plain() }, LayerAdjustments { vibrance: 50.0, ..Default::default() }),
            {
                let mut g = plain();
                g.color_editor.bands[3] = band;
                let mut l = LayerAdjustments::default();
                l.color_bands[3] = band;
                (g, l)
            },
            {
                let mut g = plain();
                g.color_balance.shadows = wheel;
                let mut l = LayerAdjustments::default();
                l.color_balance.shadows = wheel;
                (g, l)
            },
        ];
        for (global, local) in cases {
            let want = Processor::new(&meta(), &global).render(&src, None, None);
            let mut adj = plain();
            adj.layers.push(layer(filled.clone(), local.clone()));
            let got = Processor::new(&meta(), &adj).render(&src, None, None);
            assert!(max_diff(&want, &got) <= 1, "{local:?}: {}", max_diff(&want, &got));
            assert!(max_diff(&want, &Processor::new(&meta(), &plain()).render(&src, None, None)) > 3, "{local:?} has no effect");
        }
    }

    /// A layer's curves act on the result like the background's curves, blended by the mask.
    #[test]
    fn layer_curves_equal_global_curves_and_follow_the_mask() {
        use crate::params::{Curve, Curves};
        let src = colorful();
        let curves = Curves {
            rgb: Curve { points: vec![[0.0, 0.0], [0.4, 0.3], [1.0, 1.0]] },
            red: Curve { points: vec![[0.0, 0.03], [0.6, 0.7], [1.0, 1.0]] },
            luma: Curve { points: vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]] },
            ..Default::default()
        };
        let want = Processor::new(&meta(), &Adjustments { curves: curves.clone(), ..plain() }).render(&src, None, None);
        let curved = LayerAdjustments { curves, ..Default::default() };
        assert!(!curved.is_neutral() && curved.deltas_neutral());
        let mut adj = plain();
        adj.layers.push(layer(Mask { fill: true, ..Default::default() }, curved.clone()));
        let got = Processor::new(&meta(), &adj).render(&src, None, None);
        assert!(max_diff(&want, &got) <= 1, "{}", max_diff(&want, &got));
        let none = Processor::new(&meta(), &plain()).render(&src, None, None);
        assert!(max_diff(&want, &none) > 3, "Kurve ohne Wirkung");
        // Inverted fill: no part of the image is in the mask.
        adj.layers[0].mask.invert = true;
        assert_eq!(max_diff(&none, &Processor::new(&meta(), &adj).render(&src, None, None)), 0);
        // Half opacity: about halfway (blended in the working space, which the output
        // conversion mixes, so not exactly per channel).
        adj.layers[0].mask.invert = false;
        adj.layers[0].opacity = 50.0;
        let half = Processor::new(&meta(), &adj).render(&src, None, None);
        let mean = |a: &Rendered, b: &Rendered| a.rgba.iter().zip(&b.rgba).map(|(x, y)| (*x as f64 - *y as f64).abs()).sum::<f64>() / a.rgba.len() as f64;
        let whole = mean(&none, &want);
        for d in [mean(&half, &none), mean(&half, &want)] {
            assert!(d > whole * 0.3 && d < whole * 0.7, "{d} von {whole}");
        }
    }

    #[test]
    fn color_wheels_of_layers_add_up() {
        use crate::params::ColorWheel;
        let src = colorful();
        let filled = Mask { fill: true, ..Default::default() };
        let (a, b) = (ColorWheel { hue: 30.0, amount: 30.0, lightness: 5.0 }, ColorWheel { hue: 120.0, amount: 30.0, lightness: 5.0 });
        let mut two = plain();
        for w in [a, b] {
            let mut l = LayerAdjustments::default();
            l.color_balance.master = w;
            two.layers.push(layer(filled.clone(), l));
        }
        // The cartesian sum of both wheels.
        let [oa, ob] = [a.offset(), b.offset()];
        let (x, y) = (oa[0] + ob[0], oa[1] + ob[1]);
        let sum = ColorWheel { hue: y.atan2(x).to_degrees(), amount: (x * x + y * y).sqrt() * 100.0, lightness: 10.0 };
        let mut one = plain();
        one.color_balance.master = sum;
        let d = max_diff(&Processor::new(&meta(), &one).render(&src, None, None), &Processor::new(&meta(), &two).render(&src, None, None));
        assert!(d <= 1, "{d}");
    }

    #[test]
    fn radial_layer_affects_only_inside_and_opacity_scales() {
        let src = LinearImage::new(40, 40, vec![[0.1; 3]; 1600]);
        let radial = crate::params::RadialGradient { center: [0.5, 0.5], radius_x: 0.2, radius_y: 0.2, feather: 0.2, invert: false, angle: 0.0 };
        let mut adj = plain();
        adj.layers.push(layer(Mask { radial: vec![radial], ..Default::default() }, LayerAdjustments { exposure: 1.5, ..Default::default() }));
        let base = Processor::new(&meta(), &plain()).render(&src, None, None);
        let r = Processor::new(&meta(), &adj).render(&src, None, None);
        let at = |img: &Rendered, x: usize, y: usize| img.rgba[(y * 40 + x) * 4];
        assert!(at(&r, 20, 20) > at(&base, 20, 20) + 30);
        assert_eq!(at(&r, 1, 1), at(&base, 1, 1));
        adj.layers[0].opacity = 50.0;
        let half = Processor::new(&meta(), &adj).render(&src, None, None);
        assert!(at(&half, 20, 20) > at(&base, 20, 20) && at(&half, 20, 20) < at(&r, 20, 20));
        adj.layers[0].enabled = false;
        assert_eq!(Processor::new(&meta(), &adj).render(&src, None, None).rgba, base.rgba);
    }

    #[test]
    fn luma_range_restricts_layer_to_shadows() {
        let src = LinearImage::new(20, 1, (0..20).map(|i| [0.005 + i as f32 * 0.05; 3]).collect());
        let mut adj = plain();
        let mask = Mask { fill: true, luma_range: Some(crate::params::LumaRange { low: 0.0, high: 0.3, falloff: 0.05, invert: false }), ..Default::default() };
        adj.layers.push(layer(mask, LayerAdjustments { exposure: 1.0, ..Default::default() }));
        let base = Processor::new(&meta(), &plain()).render(&src, None, None);
        let r = Processor::new(&meta(), &adj).render(&src, None, None);
        assert!(r.rgba[0] > base.rgba[0], "dark pixel brightened");
        assert_eq!(r.rgba[19 * 4], base.rgba[19 * 4], "bright pixel untouched");
    }

    #[test]
    fn mask_overlay_tints_masked_area() {
        let src = LinearImage::new(10, 10, vec![[0.18; 3]; 100]);
        let mut adj = plain();
        adj.layers.push(layer(
            Mask { linear: vec![crate::params::LinearGradient { start: [0.0, 0.0], end: [0.0, 0.5] }], ..Default::default() },
            LayerAdjustments::default(),
        ));
        let opts = RenderOptions { mask_overlay: Some(0), ..Default::default() };
        let r = Processor::with_options(&meta(), &adj, opts).render(&src, None, None);
        let top = &r.rgba[0..4];
        let bottom = &r.rgba[(9 * 10) * 4..(9 * 10) * 4 + 4];
        assert!(top[0] > top[1] + 50, "{top:?}");
        assert_eq!(bottom[0], bottom[1]);
    }

    #[test]
    fn retouch_is_part_of_the_render() {
        let mut data = vec![[0.1f32; 3]; 400];
        data[10 * 20 + 10] = [0.9; 3];
        let src = LinearImage::new(20, 20, data);
        let mut adj = plain();
        adj.retouch.push(crate::params::Spot {
            mode: crate::params::SpotMode::Heal,
            target: [10.5 / 20.0, 10.5 / 20.0],
            source: [4.5 / 20.0, 4.5 / 20.0],
            radius: 0.1,
            feather: 0.3,
            opacity: 1.0,
            path: Vec::new(),
            strokes: Vec::new(),
        });
        let base = Processor::new(&meta(), &plain()).render(&src, None, None);
        let r = Processor::new(&meta(), &adj).render(&src, None, None);
        let i = (10 * 20 + 10) * 4;
        assert!(base.rgba[i] > 200 && r.rgba[i] < 120, "{} -> {}", base.rgba[i], r.rgba[i]);
    }

    #[test]
    fn dehaze_increases_contrast_of_hazy_image() {
        // A low-contrast, bright ("hazy") image.
        let src = LinearImage::new(32, 32, (0..1024).map(|i| {
            let v = 0.45 + 0.1 * ((i % 32) / 16) as f32;
            [v, v, v * 1.05]
        }).collect());
        let spread = |r: &Rendered| {
            let l: Vec<i32> = r.rgba.chunks(4).map(|p| p[1] as i32).collect();
            l.iter().max().unwrap() - l.iter().min().unwrap()
        };
        let base = Processor::new(&meta(), &plain()).render(&src, None, None);
        let mut adj = plain();
        adj.detail.dehaze = 80.0;
        let d = Processor::new(&meta(), &adj).render(&src, None, None);
        assert!(spread(&d) > spread(&base) + 10, "{} vs {}", spread(&d), spread(&base));
        adj.detail.dehaze = -80.0;
        let f = Processor::new(&meta(), &adj).render(&src, None, None);
        assert!(spread(&f) < spread(&base));
    }

    #[test]
    fn dehaze_has_no_halo_around_dark_objects() {
        // Hazy background with a dark object in it: the background right next to the object
        // must be dehazed like the rest of it (no bright seam along the edge).
        let (w, h) = (2048, 64);
        let src = LinearImage::new(
            w,
            h,
            (0..w * h)
                .map(|i| {
                    let (x, y) = (i % w, i / w);
                    // Sky (the haze's color) on the left, hazy land with the object right of it.
                    let v = match x {
                        0..300 => 0.8,
                        900..1100 => 0.04,
                        _ => 0.35 + 0.03 * (((x / 3 + y / 3) % 2) as f32),
                    };
                    [v * 0.9, v * 0.95, v]
                })
                .collect(),
        );
        let mut adj = plain();
        adj.detail.dehaze = 80.0;
        let r = Processor::new(&meta(), &adj).render_cpu(&src, None, None);
        let at = |x: usize| -> f32 { (0..h).map(|y| r.rgba[(y * w + x) * 4 + 1] as f32).sum::<f32>() / h as f32 };
        let (near, far) = (at(1103), at(1600));
        assert!((near - far).abs() < 5.0, "neben dem Objekt {near}, weiter weg {far}");
    }

    #[test]
    fn defringe_takes_out_purple_and_green_only_at_edges() {
        // Dark left half, bright right half; a purple and a green fringe next to the edge, and
        // wide areas of the same colors far from it.
        let (w, h) = (200, 8);
        let (purple, green) = ([0.3, 0.08, 0.35], [0.05, 0.25, 0.06]);
        let src = LinearImage::new(
            w,
            h,
            (0..w * h)
                .map(|i| match i % w {
                    98 => purple,
                    102 => green,
                    10..40 => purple,
                    160..195 => green,
                    x if x < 100 => [0.01; 3],
                    _ => [0.8; 3],
                })
                .collect(),
        );
        let chroma = |p: [f32; 3]| p.iter().fold(0.0f32, |m, v| m.max(*v)) - p.iter().fold(1.0f32, |m, v| m.min(*v));
        let mut adj = plain();
        adj.lens.defringe_purple = 100.0;
        adj.lens.defringe_green = 100.0;
        let mut img = src.clone();
        Processor::new(&meta(), &adj).defringe(&mut img, 1.0);
        let at = |x: usize| img.data[4 * w + x];
        assert!(chroma(at(98)) < 0.1 * chroma(purple), "lila am Rand {:?}", at(98));
        assert!(chroma(at(102)) < 0.1 * chroma(green), "grün am Rand {:?}", at(102));
        assert!(chroma(at(25)) > 0.9 * chroma(purple), "lila in der Fläche {:?}", at(25));
        assert!(chroma(at(177)) > 0.9 * chroma(green), "grün in der Fläche {:?}", at(177));
        // Luminance stays, and red is no fringe color.
        assert!((dot(at(98), LUMA) - dot(purple, LUMA)).abs() < 1e-4);
        assert_eq!(defringe_pixel([0.4, 0.05, 0.05], 0.3, 0.3, [1.0, 1.0]), [0.4, 0.05, 0.05]);
        adj.lens.defringe_green = 0.0;
        let mut img = src.clone();
        Processor::new(&meta(), &adj).defringe(&mut img, 1.0);
        assert_eq!(img.data[4 * w + 102], green, "grün bleibt");
    }

    #[test]
    fn output_spaces_keep_neutrals() {
        let src = LinearImage::new(4, 4, vec![[0.18; 3]; 16]);
        let p = Processor::new(&meta(), &plain());
        for space in OutputSpace::ALL {
            let out = p.process(&src, None, space);
            let v = out.data[5];
            assert!((v[0] - v[1]).abs() < 2e-3 && (v[1] - v[2]).abs() < 2e-3, "{space:?} {v:?}");
        }
    }
}
