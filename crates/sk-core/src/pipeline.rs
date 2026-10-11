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

use std::sync::{Arc, Mutex};

use rayon::prelude::*;

use crate::color::{Mat3, Oklab, OutputSpace, luminance, mat_vec, srgb_decode, srgb_encode};
use crate::detail::{Plane, fast_guided_filter, gaussian_blur, guided_filter, value_noise};
use crate::geometry::{GeoMap, Region};
use crate::image::LinearImage;
use crate::mask::{MaskBitmap, apply_retouch, cached_bitmap};
use crate::params::{Adjustments, BaseCurve, HUE_BANDS, LayerAdjustments, Mask, WbMode};
use crate::raw::RawMeta;

const MID_GREY: f32 = 0.18;

/// Stage timing output, enabled with `SK_TRACE=1`.
static TRACE: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var_os("SK_TRACE").is_some());

struct Timer(std::time::Instant);

impl Timer {
    fn start() -> Self {
        Self(std::time::Instant::now())
    }

    fn lap(&mut self, what: &str) {
        if *TRACE {
            eprintln!("  {what}: {:?}", self.0.elapsed());
        }
        self.0 = std::time::Instant::now();
    }
}
const LUT_SIZE: usize = 8192;
/// Rec.2020 luminance coefficients.
const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

#[derive(Clone, Copy, Debug, Default)]
pub struct RenderOptions {
    /// Show the whole frame (for the crop tool).
    pub ignore_crop: bool,
    /// Paint clipped pixels (display rendering only).
    pub clip_warning: bool,
    /// Tint the mask of this layer red (display rendering only).
    pub mask_overlay: Option<usize>,
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

/// Caches the expensive early stages (geometry, white balance, noise reduction)
/// so that tone and color edits only re-run the later stages.
#[derive(Default)]
pub struct StageCache {
    entry: Mutex<Option<(u64, Arc<LinearImage>)>>,
    local: Mutex<Option<(u64, Arc<LocalPlanes>)>>,
}

/// Neighborhood planes derived from log luminance; independent of exposure and tone settings.
struct LocalPlanes {
    /// Smoothed log2 luminance relative to middle grey (before exposure).
    hdr_base: Option<Plane>,
    /// Local contrast detail in EV (clarity + structure combined, unscaled per kind).
    clarity: Option<Plane>,
    structure: Option<Plane>,
    dehaze: Option<DarkChannel>,
}

/// Low-resolution dark channel (haze density) and airlight for dehazing.
struct DarkChannel {
    plane: Plane,
    /// Downsampling factor relative to the working region.
    step: usize,
    airlight: f32,
}

impl DarkChannel {
    fn new(img: &LinearImage) -> Self {
        let (w, h) = (img.width, img.height);
        let step = (w.max(h) / 256).max(1);
        let min_c: Vec<f32> = img.data.par_iter().map(|p| p[0].min(p[1]).min(p[2])).collect();
        // Airlight: bright end of the minimum channel.
        let mut sorted: Vec<f32> = min_c.iter().step_by(7).copied().collect();
        let idx = ((sorted.len() as f32 * 0.997) as usize).min(sorted.len().saturating_sub(1));
        let airlight = if sorted.is_empty() { 1.0 } else { *sorted.select_nth_unstable_by(idx, |a, b| a.total_cmp(b)).1 }.max(0.05);
        // Min-pool into blocks, then smooth.
        let (lw, lh) = (w.div_ceil(step), h.div_ceil(step));
        let pooled: Vec<f32> = (0..lw * lh)
            .into_par_iter()
            .map(|i| {
                let (bx, by) = (i % lw * step, i / lw * step);
                let mut m = f32::MAX;
                for y in by..(by + step).min(h) {
                    for x in bx..(bx + step).min(w) {
                        m = m.min(min_c[y * w + x]);
                    }
                }
                (m / airlight).clamp(0.0, 1.0)
            })
            .collect();
        let plane = gaussian_blur(&Plane::new(lw, lh, pooled), 1.5);
        Self { plane, step, airlight }
    }

    #[inline]
    fn at(&self, x: usize, y: usize) -> f32 {
        let fx = ((x as f32 + 0.5) / self.step as f32 - 0.5).clamp(0.0, (self.plane.w - 1) as f32);
        let fy = ((y as f32 + 0.5) / self.step as f32 - 0.5).clamp(0.0, (self.plane.h - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(self.plane.w - 1), (y0 + 1).min(self.plane.h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let d = &self.plane.data;
        let w = self.plane.w;
        let top = d[y0 * w + x0] * (1.0 - tx) + d[y0 * w + x1] * tx;
        let bot = d[y1 * w + x0] * (1.0 - tx) + d[y1 * w + x1] * tx;
        top * (1.0 - ty) + bot * ty
    }

    /// Removes (strength > 0) or adds (strength < 0) haze.
    #[inline]
    fn apply(&self, p: [f32; 3], dark: f32, strength: f32) -> [f32; 3] {
        let a = self.airlight;
        if strength > 0.0 {
            let t = (1.0 - 0.95 * strength * dark).max(0.12);
            p.map(|v| ((v - a * (1.0 - t)) / t).max(0.0))
        } else {
            let s = -strength * 0.6 * (1.0 - dark * 0.5);
            p.map(|v| v + (a - v) * s)
        }
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
}

impl ToneParams {
    fn add(&mut self, l: &LayerAdjustments, w: f32) {
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
    }
}

/// A layer ready for per-pixel evaluation.
struct PreparedLayer {
    /// Index in `Adjustments::layers`.
    index: usize,
    mask: Mask,
    opacity: f32,
    adj: LayerAdjustments,
}

pub struct Processor {
    adj: Adjustments,
    base: ToneParams,
    layers: Vec<PreparedLayer>,
    wb: [f32; 3],
    cam_to_working: Mat3,
    oklab: Oklab,
    color_active: bool,
    /// display-linear working 0..1 -> encoded 0..1, per channel (brightness, levels, curves)
    lut: [Vec<f32>; 3],
    /// Ratio of the rendered source to the full sensor resolution (for radii in full-res pixels).
    sensor_long_edge: f32,
    opts: RenderOptions,
}

impl Processor {
    pub fn new(meta: &RawMeta, adj: &Adjustments) -> Self {
        Self::with_options(meta, adj, RenderOptions::default())
    }

    pub fn with_options(meta: &RawMeta, adj: &Adjustments, opts: RenderOptions) -> Self {
        let wb = match adj.white_balance.mode {
            WbMode::AsShot => meta.as_shot_wb,
            WbMode::Custom => meta.color.wb_for_temp_tint(adj.white_balance.kelvin, adj.white_balance.tint),
        };
        // dcraw convention: the smallest multiplier is 1.
        let min_wb = wb.iter().cloned().fold(f32::MAX, f32::min).max(1e-6);
        let wb = wb.map(|v| v / min_wb);
        let layers: Vec<PreparedLayer> = adj
            .layers
            .iter()
            .enumerate()
            .filter(|(i, l)| l.enabled && l.opacity > 0.0 && (!l.adjustments.is_neutral() || opts.mask_overlay == Some(*i)))
            .map(|(index, l)| PreparedLayer { index, mask: l.mask.clone(), opacity: l.opacity / 100.0, adj: l.adjustments })
            .collect();
        let color_active = adj.saturation != 0.0
            || adj.vibrance != 0.0
            || !adj.color_editor.is_neutral()
            || !adj.color_balance.is_neutral()
            || adj.black_white.enabled
            || layers.iter().any(|l| l.adj.saturation != 0.0);
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
        };
        Self {
            base,
            layers,
            wb,
            cam_to_working: meta.color.cam_to_working,
            oklab: Oklab::default(),
            color_active,
            lut: build_output_luts(adj),
            sensor_long_edge: meta.info.width.max(meta.info.height).max(1) as f32,
            opts,
            adj: adj.clone(),
        }
    }

    pub fn geomap(&self, src_w: usize, src_h: usize) -> GeoMap {
        GeoMap::new(&self.adj.geometry, &self.adj.lens, self.adj.rotation, src_w, src_h, self.opts.ignore_crop)
    }

    /// Output size (at the resolution of a `src_w` x `src_h` source).
    pub fn output_size(&self, src_w: usize, src_h: usize) -> (usize, usize) {
        let g = self.geomap(src_w, src_h);
        (g.out_w, g.out_h)
    }

    // ------------------------------------------------------------ per-pixel stages

    /// Camera RGB -> linear working RGB with white balance and highlight reconstruction.
    #[inline]
    fn to_working(&self, raw: [f32; 3]) -> [f32; 3] {
        let mut b = [raw[0] * self.wb[0], raw[1] * self.wb[1], raw[2] * self.wb[2]];
        let max_raw = raw[0].max(raw[1]).max(raw[2]);
        if max_raw > 0.95 {
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
        let gain = 2f32.powf(tp.exposure_ev + detail_ev[0] * tp.clarity / 100.0 * 0.6 + detail_ev[1] * tp.structure / 100.0 * 0.8);
        let mut rgb = rgb.map(|v| v * gain);
        if tp.temperature != 0.0 || tp.tint != 0.0 {
            let t = tp.temperature / 100.0;
            rgb[0] *= 2f32.powf(t * 0.35);
            rgb[2] *= 2f32.powf(-t * 0.35);
            rgb[1] *= 2f32.powf(-tp.tint / 100.0 * 0.25);
        }
        let l = dot(rgb, LUMA);
        if l > 1e-7 && tp.hdr.iter().any(|v| *v != 0.0) {
            let ev_px = (l / MID_GREY).log2();
            // Weights come mostly from the smoothed neighborhood (local tone mapping).
            let ev = match local_ev {
                Some(e) => (e + tp.exposure_ev) * 0.75 + ev_px * 0.25,
                None => ev_px,
            };
            let [hi, sh, wh, bl] = tp.hdr;
            let w_hi = sigmoid((ev - 0.5) * 1.6);
            let w_sh = sigmoid(-(ev + 2.0) * 1.4);
            let w_wh = sigmoid((ev_px - 2.0) * 2.5);
            let w_bl = sigmoid(-(ev_px + 4.5) * 2.0);
            let gain_ev = -hi * 1.5 * w_hi + sh * 1.8 * w_sh + wh * 1.0 * w_wh + bl * 1.5 * w_bl;
            let g = 2f32.powf(gain_ev);
            rgb.iter_mut().for_each(|v| *v *= g);
        }
        let contrast_exp = 1.0 + tp.contrast / 100.0 * 0.5;
        if contrast_exp != 1.0 {
            for v in rgb.iter_mut() {
                *v = MID_GREY * (*v / MID_GREY).powf(contrast_exp);
            }
        }
        rgb.map(|v| base_curve(self.adj.base_curve, v))
    }

    /// Color adjustments in Oklab on display-linear working RGB.
    #[inline]
    fn color(&self, rgb: [f32; 3], saturation: f32) -> [f32; 3] {
        if !self.color_active {
            return rgb;
        }
        let [mut l, a, b] = self.oklab.from_linear(rgb);
        let mut c = (a * a + b * b).sqrt();
        let mut h = b.atan2(a).to_degrees().rem_euclid(360.0);
        // Hue is unreliable for nearly neutral colors.
        let chroma_w = smoothstep(0.0, 0.05, c);

        let ed = &self.adj.color_editor;
        if !ed.is_neutral() {
            let w = band_weights(h);
            let (mut dh, mut ds, mut dl) = (0.0, 0.0, 0.0);
            for (i, band) in ed.bands.iter().enumerate() {
                dh += w[i] * band.hue;
                ds += w[i] * band.saturation / 100.0;
                dl += w[i] * band.lightness / 100.0;
            }
            h += dh * chroma_w;
            c *= (1.0 + ds * chroma_w).max(0.0);
            l *= 1.0 + dl * 0.4 * chroma_w;
        }

        let s = 1.0 + saturation / 100.0;
        let vib = self.adj.vibrance / 100.0;
        if s != 1.0 || vib != 0.0 {
            let low = (1.0 - c / 0.2).clamp(0.0, 1.0);
            c *= (s * (1.0 + vib * low * low)).max(0.0);
        }

        let (mut a, mut b) = {
            let hr = h.to_radians();
            (c * hr.cos(), c * hr.sin())
        };

        let cb = &self.adj.color_balance;
        if !cb.is_neutral() {
            let lw = l.clamp(0.0, 1.0);
            let ws = (1.0 - lw) * (1.0 - lw);
            let wh = lw * lw;
            let wm = (1.0 - ws - wh).max(0.0);
            for (wheel, weight) in [(cb.master, 1.0), (cb.shadows, ws), (cb.midtones, wm), (cb.highlights, wh)] {
                let hr = wheel.hue.to_radians();
                let amt = wheel.amount / 100.0 * 0.08 * weight;
                a += amt * hr.cos();
                b += amt * hr.sin();
                l += wheel.lightness / 100.0 * 0.15 * weight;
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

    #[inline]
    fn encode(&self, rgb: [f32; 3]) -> [f32; 3] {
        [lut_lookup(&self.lut[0], rgb[0]), lut_lookup(&self.lut[1], rgb[1]), lut_lookup(&self.lut[2], rgb[2])]
    }

    // ------------------------------------------------------------ image stages

    fn stage_key(&self, src: &LinearImage, region: Region) -> u64 {
        let relevant = serde_json::json!({
            "g": self.adj.geometry, "l": self.adj.lens, "r": self.adj.rotation, "ic": self.opts.ignore_crop,
            "wb": self.wb, "nl": self.adj.detail.nr_luma, "nc": self.adj.detail.nr_color, "rt": self.adj.retouch,
        });
        let key = format!("{:p}|{}x{}|{region:?}|{relevant}", src.data.as_ptr(), src.width, src.height);
        crate::cache::fnv1a(key.as_bytes())
    }

    /// Geometry + white balance + noise reduction -> linear working RGB.
    fn early_stages(&self, src: &LinearImage, geo: &GeoMap, region: Region, scale: f32) -> LinearImage {
        let mut img = geo.resample(src, region);
        apply_retouch(&mut img, src, geo, region, &self.adj.retouch);
        img.data.par_iter_mut().for_each(|p| *p = self.to_working(*p));
        self.noise_reduction(&mut img, scale);
        img
    }

    fn noise_reduction(&self, img: &mut LinearImage, scale: f32) {
        let d = &self.adj.detail;
        if d.nr_luma <= 0.0 && d.nr_color <= 0.0 {
            return;
        }
        let (w, h) = (img.width, img.height);
        // Work on a perceptual (square-root) luminance and log-free chroma ratios.
        let y = Plane::new(w, h, img.data.par_iter().map(|p| dot(*p, LUMA).max(0.0).sqrt()).collect());
        let y_new = if d.nr_luma > 0.0 {
            let r = ((2.0 * scale).round() as usize).max(1);
            let eps = (d.nr_luma / 100.0).powi(2) * 0.0025;
            let f = guided_filter(&y, r, eps);
            let keep = 1.0 - d.nr_luma / 100.0 * 0.85;
            y.zip(&f, |a, b| b + (a - b) * keep)
        } else {
            y.clone()
        };
        let chroma = if d.nr_color > 0.0 {
            let sigma = (d.nr_color / 100.0 * 6.0 * scale).max(0.5);
            let ch = |c: usize| {
                let p = Plane::new(w, h, img.data.par_iter().map(|p| p[c] - dot(*p, LUMA)).collect());
                gaussian_blur(&p, sigma)
            };
            Some([ch(0), ch(2)])
        } else {
            None
        };
        img.data.par_iter_mut().enumerate().for_each(|(i, p)| {
            let y_old = dot(*p, LUMA).max(0.0);
            let y2 = y_new.data[i].max(0.0);
            let y2 = y2 * y2;
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

    /// Processes `region` (default: whole output) into encoded working RGB (sRGB transfer).
    #[allow(clippy::type_complexity)]
    fn process_encoded(
        &self,
        src: &LinearImage,
        region: Option<Region>,
        cache: Option<&StageCache>,
    ) -> (LinearImage, (usize, usize), Option<Vec<f32>>) {
        let geo = self.geomap(src.width, src.height);
        let full = (geo.out_w, geo.out_h);
        let region = region.unwrap_or(Region { x: 0, y: 0, w: full.0, h: full.1 });
        let long_edge = full.0.max(full.1) as f32;
        let scale = src.width.max(src.height) as f32 / self.sensor_long_edge;
        let d = &self.adj.detail;

        // Which local operators are needed (by the background or by any layer).
        let any_layer = |f: &dyn Fn(&LayerAdjustments) -> bool| self.layers.iter().any(|l| f(&l.adj));
        let hdr_active = self.base.hdr.iter().any(|v| *v != 0.0) || any_layer(&|l| l.highlights != 0.0 || l.shadows != 0.0);
        let clarity_active = d.clarity != 0.0 || any_layer(&|l| l.clarity != 0.0);
        let structure_active = d.structure != 0.0 || any_layer(&|l| l.structure != 0.0);
        let dehaze_active = d.dehaze != 0.0 || any_layer(&|l| l.dehaze != 0.0);

        // Margin for neighborhood operations, so that tiles match the full rendering.
        let r_hdr = (long_edge * 0.02).max(2.0);
        let r_clarity = (long_edge * 0.03).max(3.0);
        let r_structure = (3.0 * scale).max(1.0);
        let mut margin: f32 = 0.0;
        if hdr_active {
            margin = margin.max(r_hdr * 2.0);
        }
        if clarity_active {
            margin = margin.max(r_clarity * 2.0);
        }
        if structure_active {
            margin = margin.max(r_structure * 4.0);
        }
        if d.sharpen_amount > 0.0 {
            margin = margin.max(d.sharpen_radius * scale * 4.0 + 2.0);
        }
        if d.nr_luma > 0.0 || d.nr_color > 0.0 {
            margin = margin.max(20.0 * scale + 4.0);
        }
        if dehaze_active {
            margin = margin.max(long_edge / 64.0);
        }
        let m = margin.ceil() as usize;
        let ext = Region {
            x: region.x.saturating_sub(m),
            y: region.y.saturating_sub(m),
            w: (region.x + region.w + m).min(full.0) - region.x.saturating_sub(m),
            h: (region.y + region.h + m).min(full.1) - region.y.saturating_sub(m),
        };

        let mut timer = Timer::start();
        let working = match cache {
            Some(c) => {
                let key = self.stage_key(src, ext);
                let mut entry = c.entry.lock().unwrap();
                match &*entry {
                    Some((k, img)) if *k == key => img.clone(),
                    _ => {
                        let img = Arc::new(self.early_stages(src, &geo, ext, scale));
                        *entry = Some((key, img.clone()));
                        img
                    }
                }
            }
            None => Arc::new(self.early_stages(src, &geo, ext, scale)),
        };
        let (w, h) = (ext.w, ext.h);
        timer.lap("early stages");

        // Local operators on log luminance. They are shift invariant in log space, so they
        // are computed before exposure and cached independently of tone settings.
        let need_log = hdr_active || clarity_active || structure_active || dehaze_active;
        let local = if need_log {
            let key = {
                let k = format!("{}|{hdr_active}|{clarity_active}|{structure_active}|{dehaze_active}", self.stage_key(src, ext));
                crate::cache::fnv1a(k.as_bytes())
            };
            let compute = || {
                let log_l = Plane::new(w, h, working.data.par_iter().map(|p| dot(*p, LUMA).max(1e-6).log2() - MID_GREY.log2()).collect());
                let sub = ((r_hdr.min(r_clarity) / 8.0) as usize).clamp(1, 8);
                LocalPlanes {
                    hdr_base: hdr_active.then(|| fast_guided_filter(&log_l, r_hdr as usize, 0.5, sub)),
                    clarity: clarity_active.then(|| {
                        let base = fast_guided_filter(&log_l, r_clarity as usize, 0.3, sub);
                        log_l.zip(&base, |a, b| (a - b).clamp(-3.0, 3.0))
                    }),
                    structure: structure_active.then(|| {
                        let base = gaussian_blur(&log_l, r_structure);
                        log_l.zip(&base, |a, b| (a - b).clamp(-2.0, 2.0))
                    }),
                    dehaze: dehaze_active.then(|| DarkChannel::new(&working)),
                }
            };
            Some(match cache {
                Some(c) => {
                    let mut entry = c.local.lock().unwrap();
                    match &*entry {
                        Some((k, planes)) if *k == key => planes.clone(),
                        _ => {
                            let planes = Arc::new(compute());
                            *entry = Some((key, planes.clone()));
                            planes
                        }
                    }
                }
                None => Arc::new(compute()),
            })
        } else {
            None
        };
        timer.lap("local operators");

        // Layer masks (rasterized once per mask change, sampled in source coordinates).
        let aspect = src.width as f32 / src.height.max(1) as f32;
        let bitmaps: Vec<Option<Arc<MaskBitmap>>> =
            self.layers.iter().map(|l| (!l.mask.is_empty()).then(|| cached_bitmap(&l.mask, aspect))).collect();
        let overlay_layer = self.opts.mask_overlay.and_then(|i| self.layers.iter().position(|l| l.index == i));
        let (sw, sh) = (src.width as f32, src.height as f32);

        let results: Vec<([f32; 3], f32)> = working
            .data
            .par_iter()
            .enumerate()
            .map(|(i, &p)| {
                let (mut ev, mut det) = (None, [0.0f32; 2]);
                if let Some(lp) = &local {
                    ev = lp.hdr_base.as_ref().map(|pl| pl.data[i]);
                    det = [lp.clarity.as_ref().map_or(0.0, |c| c.data[i]), lp.structure.as_ref().map_or(0.0, |c| c.data[i])];
                }
                let dark = local.as_ref().and_then(|lp| lp.dehaze.as_ref()).map(|dc| (dc, dc.at(i % w, i / w)));
                let haze = |p: [f32; 3], strength: f32| match dark {
                    Some((dc, dv)) if strength != 0.0 => dc.apply(p, dv, strength / 100.0),
                    _ => p,
                };
                let base_disp = || self.color(self.tone(haze(p, self.base.dehaze), ev, det, &self.base), self.base.saturation);
                if self.layers.is_empty() {
                    return (self.encode(base_disp()), 0.0);
                }
                let (ox, oy) = ((ext.x + i % w) as f32 + 0.5, (ext.y + i / w) as f32 + 0.5);
                let (sx, sy) = geo.source_point(ox, oy);
                let (u, v) = ((sx + 0.5) / sw, (sy + 0.5) / sh);
                let mut tp = self.base;
                let mut any = false;
                let mut overlay = 0.0;
                let mut base_cache: Option<[f32; 3]> = None;
                for (li, layer) in self.layers.iter().enumerate() {
                    let mut wgt = bitmaps[li].as_ref().map_or(0.0, |b| b.sample(u, v));
                    if layer.mask.invert {
                        wgt = 1.0 - wgt;
                    }
                    if wgt > 0.0 && (layer.mask.luma_range.is_some() || layer.mask.color_range.is_some()) {
                        let bd = *base_cache.get_or_insert_with(base_disp);
                        wgt *= self.range_weight(&layer.mask, bd);
                    }
                    wgt *= layer.opacity;
                    if Some(li) == overlay_layer {
                        overlay = wgt;
                    }
                    if wgt > 0.0 && !layer.adj.is_neutral() {
                        tp.add(&layer.adj, wgt);
                        any = true;
                    }
                }
                if !any {
                    let bd = base_cache.unwrap_or_else(base_disp);
                    return (self.encode(bd), overlay);
                }
                let mut e = self.encode(self.color(self.tone(haze(p, tp.dehaze), ev, det, &tp), tp.saturation));
                if tp.brightness != 0.0 {
                    let g = 2f32.powf(-tp.brightness / 100.0 * 0.6);
                    e = e.map(|v| v.max(0.0).powf(g));
                }
                (e, overlay)
            })
            .collect();
        let mut mask_plane = overlay_layer.map(|_| results.iter().map(|r| r.1).collect::<Vec<f32>>());
        let mut out: Vec<[f32; 3]> = results.into_iter().map(|r| r.0).collect();
        timer.lap("tone + color + encode");

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

        timer.lap("sharpen / vignette / grain");
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

    /// Full processing into an output color space (encoded 0..1 floats), for export.
    pub fn process(&self, src: &LinearImage, region: Option<Region>, space: OutputSpace) -> LinearImage {
        let (enc, _, _) = self.process_encoded(src, region, None);
        let m = space.from_working();
        let trc = space.trc();
        let data = enc
            .data
            .par_iter()
            .map(|p| {
                let lin = p.map(srgb_decode);
                mat_vec(&m, lin).map(|v| trc.encode(v.clamp(0.0, 1.0)))
            })
            .collect();
        LinearImage::new(enc.width, enc.height, data)
    }

    /// Display rendering (sRGB, 8-bit) with histogram.
    pub fn render(&self, src: &LinearImage, region: Option<Region>, cache: Option<&StageCache>) -> Rendered {
        let (enc, full, mask) = self.process_encoded(src, region, cache);
        let mut timer = Timer::start();
        let to_srgb = OutputSpace::Srgb.from_working();
        let dec = &*DECODE_LUT;
        let srgb = &*SRGB_ENCODE_LUT;
        let clip = self.opts.clip_warning;
        let mut rgba = vec![0u8; enc.data.len() * 4];
        rgba.par_chunks_mut(4).zip(enc.data.par_iter()).enumerate().for_each(|(i, (out, p))| {
            let lin = p.map(|v| lut_lookup(dec, v));
            let s = mat_vec(&to_srgb, lin);
            let mut px = s.map(|v| (lut_lookup(srgb, v.clamp(0.0, 1.0)) * 255.0 + 0.5) as u8);
            if let Some(m) = &mask {
                let a = (m[i] * 0.6).clamp(0.0, 0.6);
                px = [px[0] as f32 * (1.0 - a) + 255.0 * a, px[1] as f32 * (1.0 - a), px[2] as f32 * (1.0 - a)].map(|v| v as u8);
            }
            if clip && (px[0] == 255 || px[1] == 255 || px[2] == 255) {
                out.copy_from_slice(&[255, 0, 0, 255]);
            } else if clip && px == [0, 0, 0] {
                out.copy_from_slice(&[0, 80, 255, 255]);
            } else {
                out.copy_from_slice(&[px[0], px[1], px[2], 255]);
            }
        });
        timer.lap("display conversion");
        let histogram = histogram_rgba(&rgba);
        timer.lap("histogram");
        Rendered { width: enc.width, height: enc.height, rgba, histogram, full_size: full }
    }
}

static DECODE_LUT: std::sync::LazyLock<Vec<f32>> =
    std::sync::LazyLock::new(|| (0..LUT_SIZE).map(|i| srgb_decode(i as f32 / (LUT_SIZE - 1) as f32)).collect());
static SRGB_ENCODE_LUT: std::sync::LazyLock<Vec<f32>> =
    std::sync::LazyLock::new(|| (0..LUT_SIZE).map(|i| srgb_encode(i as f32 / (LUT_SIZE - 1) as f32)).collect());

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

/// Scene-linear to display-linear.
#[inline]
pub fn base_curve(kind: BaseCurve, x: f32) -> f32 {
    match kind {
        BaseCurve::Linear => x.clamp(0.0, 1.0),
        BaseCurve::FilmStandard => aces_fit(x * 0.8),
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
    let lv = &adj.levels;
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
                // Levels: input range, midtone gamma, output range.
                let range = (lv.in_white - lv.in_black).max(1e-4);
                v = ((v - lv.in_black) / range).clamp(0.0, 1.0);
                if lv.midtone != 1.0 {
                    v = v.powf(1.0 / lv.midtone.max(0.05));
                }
                v = lv.out_black + v * (lv.out_white - lv.out_black);
                v = lut_lookup(&master, v);
                v = lut_lookup(&chans[c], v);
                v.clamp(0.0, 1.0)
            })
            .collect()
    })
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
        }
    }

    /// No sharpening/NR so that single pixels can be checked.
    fn plain() -> Adjustments {
        Adjustments { detail: Detail { sharpen_amount: 0.0, nr_color: 0.0, ..Default::default() }, ..Default::default() }
    }

    fn px(p: &Processor, v: [f32; 3]) -> [f32; 3] {
        p.encode(p.color(p.tone(p.to_working(v), None, [0.0; 2], &p.base), p.base.saturation))
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
            detail: Detail { clarity: 50.0, structure: 30.0, sharpen_amount: 150.0, sharpen_radius: 1.0, nr_luma: 30.0, nr_color: 30.0, ..Default::default() },
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
    fn radial_layer_affects_only_inside_and_opacity_scales() {
        let src = LinearImage::new(40, 40, vec![[0.1; 3]; 1600]);
        let radial = crate::params::RadialGradient { center: [0.5, 0.5], radius_x: 0.2, radius_y: 0.2, feather: 0.2, invert: false };
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
