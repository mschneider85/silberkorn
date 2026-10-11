//! The development pipeline: camera RGB -> white balance -> sRGB -> tone -> color -> output.
//!
//! Stages per pixel:
//! 1. camera multipliers (white balance) and camera->linear sRGB matrix
//! 2. exposure, contrast (power around middle grey), HDR tone gains (luminance based)
//! 3. base curve (scene -> display linear)
//! 4. saturation / vibrance
//! 5. sRGB encoding, brightness, levels, RGB + per-channel curves (one combined LUT per channel)

use rayon::prelude::*;

use crate::color::{Mat3, luminance, mat_vec, srgb_encode};
use crate::image::{LinearImage, rotate_pixels};
use crate::params::{Adjustments, BaseCurve, WbMode};
use crate::raw::RawMeta;

const MID_GREY: f32 = 0.18;
const LUT_SIZE: usize = 8192;

pub struct Processor {
    /// camera multipliers, scaled so the smallest is 1
    wb: [f32; 3],
    matrix: Mat3,
    exposure_gain: f32,
    contrast_exp: f32,
    hdr: [f32; 4],
    base_curve: BaseCurve,
    saturation: f32,
    vibrance: f32,
    /// display-linear 0..1 -> encoded 0..1, per channel
    lut: [Vec<f32>; 3],
    rotation: u8,
}

/// Result of processing: 8-bit RGBA for display plus histogram.
pub struct Rendered {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    pub histogram: Histogram,
}

#[derive(Clone, Default)]
pub struct Histogram {
    pub r: Vec<u32>,
    pub g: Vec<u32>,
    pub b: Vec<u32>,
    pub luma: Vec<u32>,
}

impl Processor {
    pub fn new(meta: &RawMeta, adj: &Adjustments) -> Self {
        let wb = match adj.white_balance.mode {
            WbMode::AsShot => meta.as_shot_wb,
            WbMode::Custom => meta.color.wb_for_temp_tint(adj.white_balance.kelvin, adj.white_balance.tint),
        };
        // dcraw convention: the smallest multiplier is 1, and white-balanced values are
        // clipped at 1.0 so that blown highlights stay neutral instead of turning magenta.
        let min_wb = wb.iter().cloned().fold(f32::MAX, f32::min).max(1e-6);
        let wb = wb.map(|v| v / min_wb);

        Self {
            wb,
            matrix: meta.color.cam_to_srgb,
            exposure_gain: 2f32.powf(adj.exposure),
            contrast_exp: 1.0 + adj.contrast / 100.0 * 0.5,
            hdr: [adj.highlights / 100.0, adj.shadows / 100.0, adj.whites / 100.0, adj.blacks / 100.0],
            base_curve: adj.base_curve,
            saturation: 1.0 + adj.saturation / 100.0,
            vibrance: adj.vibrance / 100.0,
            lut: build_output_luts(adj),
            rotation: adj.rotation % 4,
        }
    }

    /// Processes one camera-space pixel into display-linear RGB (0..1).
    #[inline]
    fn scene_to_display(&self, cam: [f32; 3]) -> [f32; 3] {
        let balanced = [
            (cam[0] * self.wb[0]).min(1.0),
            (cam[1] * self.wb[1]).min(1.0),
            (cam[2] * self.wb[2]).min(1.0),
        ];
        let mut rgb = mat_vec(&self.matrix, balanced);
        for v in rgb.iter_mut() {
            *v = v.max(0.0) * self.exposure_gain;
        }

        // HDR tone gains in log2 space around middle grey.
        let l = luminance(rgb);
        if l > 1e-7 && self.hdr.iter().any(|v| *v != 0.0) {
            let ev = (l / MID_GREY).log2();
            let [hi, sh, wh, bl] = self.hdr;
            let w_hi = sigmoid((ev - 0.5) * 1.6);
            let w_sh = sigmoid(-(ev + 2.0) * 1.4);
            let w_wh = sigmoid((ev - 2.0) * 2.5);
            let w_bl = sigmoid(-(ev + 4.5) * 2.0);
            let gain_ev = -hi * 1.5 * w_hi + sh * 1.8 * w_sh + wh * 1.0 * w_wh + bl * 1.5 * w_bl;
            let g = 2f32.powf(gain_ev);
            rgb.iter_mut().for_each(|v| *v *= g);
        }

        if self.contrast_exp != 1.0 {
            for v in rgb.iter_mut() {
                *v = MID_GREY * (*v / MID_GREY).powf(self.contrast_exp);
            }
        }

        for v in rgb.iter_mut() {
            *v = base_curve(self.base_curve, *v);
        }

        if self.saturation != 1.0 || self.vibrance != 0.0 {
            let l = luminance(rgb);
            let max = rgb[0].max(rgb[1]).max(rgb[2]);
            let min = rgb[0].min(rgb[1]).min(rgb[2]);
            let sat = if max > 1e-6 { (max - min) / max } else { 0.0 };
            let s = self.saturation * (1.0 + self.vibrance * (1.0 - sat).powi(2));
            for v in rgb.iter_mut() {
                *v = (l + (*v - l) * s).clamp(0.0, 1.0);
            }
        }
        rgb
    }

    #[inline]
    fn encode(&self, rgb: [f32; 3]) -> [f32; 3] {
        [lut_lookup(&self.lut[0], rgb[0]), lut_lookup(&self.lut[1], rgb[1]), lut_lookup(&self.lut[2], rgb[2])]
    }

    /// Full pipeline producing encoded sRGB floats (0..1), rotation applied.
    pub fn process_f32(&self, img: &LinearImage) -> LinearImage {
        let data: Vec<[f32; 3]> = img.data.par_iter().map(|&p| self.encode(self.scene_to_display(p))).collect();
        let (data, w, h) = rotate_pixels(&data, img.width, img.height, self.rotation);
        LinearImage::new(w, h, data)
    }

    /// Display rendering: 8-bit RGBA plus histogram. `clip_warning` paints clipped pixels.
    pub fn render(&self, img: &LinearImage, clip_warning: bool) -> Rendered {
        let mut rgba = vec![0u8; img.data.len() * 4];
        rgba.par_chunks_mut(4).zip(img.data.par_iter()).for_each(|(out, &p)| {
            let e = self.encode(self.scene_to_display(p));
            let r = (e[0] * 255.0 + 0.5) as u8;
            let g = (e[1] * 255.0 + 0.5) as u8;
            let b = (e[2] * 255.0 + 0.5) as u8;
            if clip_warning && (r == 255 || g == 255 || b == 255) {
                out.copy_from_slice(&[255, 0, 0, 255]);
            } else if clip_warning && r == 0 && g == 0 && b == 0 {
                out.copy_from_slice(&[0, 80, 255, 255]);
            } else {
                out.copy_from_slice(&[r, g, b, 255]);
            }
        });
        let histogram = histogram_rgba(&rgba);
        let (px, w, h) = rotate_pixels(bytemuck_rgba(&rgba).as_slice(), img.width, img.height, self.rotation);
        let rgba = px.into_iter().flatten().collect();
        Rendered { width: w, height: h, rgba, histogram }
    }
}

fn bytemuck_rgba(rgba: &[u8]) -> Vec<[u8; 4]> {
    rgba.as_chunks::<4>().0.iter().map(|c| [c[0], c[1], c[2], c[3]]).collect()
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
    let sample = |lut: &[f32], v: f32| lut_lookup(lut, v);
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
                v = sample(&master, v);
                v = sample(&chans[c], v);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::CameraColor;
    use crate::image::Orientation;
    use crate::raw::CaptureInfo;
    use std::path::PathBuf;

    fn meta() -> RawMeta {
        let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        RawMeta {
            path: PathBuf::new(),
            info: CaptureInfo::default(),
            color: CameraColor { xyz_to_cam: identity, cam_to_srgb: identity },
            as_shot_wb: [1.0, 1.0, 1.0],
            orientation: Orientation::Normal,
        }
    }

    #[test]
    fn neutral_stays_neutral_and_mid_grey_is_mid() {
        let p = Processor::new(&meta(), &Adjustments::default());
        let out = p.encode(p.scene_to_display([0.18; 3]));
        assert!((out[0] - out[1]).abs() < 1e-4 && (out[1] - out[2]).abs() < 1e-4);
        assert!(out[0] > 0.4 && out[0] < 0.6, "mid grey encoded to {}", out[0]);
    }

    #[test]
    fn exposure_brightens() {
        let a = Processor::new(&meta(), &Adjustments::default());
        let b = Processor::new(&meta(), &Adjustments { exposure: 1.0, ..Default::default() });
        assert!(b.scene_to_display([0.1; 3])[0] > a.scene_to_display([0.1; 3])[0]);
    }

    #[test]
    fn shadows_lift_dark_tones_more_than_highlights() {
        let a = Processor::new(&meta(), &Adjustments::default());
        let b = Processor::new(&meta(), &Adjustments { shadows: 100.0, ..Default::default() });
        let dark = b.scene_to_display([0.01; 3])[0] / a.scene_to_display([0.01; 3])[0];
        let bright = b.scene_to_display([0.8; 3])[0] / a.scene_to_display([0.8; 3])[0];
        assert!(dark > 1.5 && bright < 1.05, "dark {dark} bright {bright}");
    }

    #[test]
    fn highlight_recovery_darkens_highlights() {
        let a = Processor::new(&meta(), &Adjustments::default());
        let b = Processor::new(&meta(), &Adjustments { highlights: 100.0, ..Default::default() });
        assert!(b.scene_to_display([1.0; 3])[0] < a.scene_to_display([1.0; 3])[0]);
    }

    #[test]
    fn rotation_swaps_dimensions() {
        let img = LinearImage::new(4, 2, vec![[0.1; 3]; 8]);
        let p = Processor::new(&meta(), &Adjustments { rotation: 1, ..Default::default() });
        let r = p.render(&img, false);
        assert_eq!((r.width, r.height), (2, 4));
        assert_eq!(r.histogram.luma.iter().sum::<u32>(), 8);
    }
}
