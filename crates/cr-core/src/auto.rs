//! Automatic adjustments: white balance (grey world), exposure and levels.

use crate::image::LinearImage;
use crate::params::{Adjustments, Levels, WbMode, WhiteBalance};
use crate::pipeline::Histogram;
use crate::raw::RawMeta;

/// Grey-world white balance on unclipped, not too dark camera values.
pub fn auto_white_balance(meta: &RawMeta, preview: &LinearImage) -> WhiteBalance {
    let mut acc = [0.0f64; 3];
    let mut n = 0usize;
    for p in preview.data.iter().step_by(3) {
        let max = p[0].max(p[1]).max(p[2]);
        if max < 0.95 && p[1] > 0.02 {
            for c in 0..3 {
                acc[c] += p[c] as f64;
            }
            n += 1;
        }
    }
    if n < 100 {
        return WhiteBalance::default();
    }
    let wb = [(acc[1] / acc[0]) as f32, 1.0, (acc[1] / acc[2]) as f32];
    let (kelvin, tint) = meta.color.temp_tint_for_wb(wb);
    WhiteBalance { mode: WbMode::Custom, kelvin: kelvin.round(), tint: (tint * 10.0).round() / 10.0 }
}

/// Exposure (EV) that brings the scene's log-average luminance to a pleasant level,
/// limited so that the brightest 0.5 % do not end up far beyond the highlight shoulder.
pub fn auto_exposure(meta: &RawMeta, adj: &Adjustments, preview: &LinearImage) -> f32 {
    let wb = match adj.white_balance.mode {
        WbMode::AsShot => meta.as_shot_wb,
        WbMode::Custom => meta.color.wb_for_temp_tint(adj.white_balance.kelvin, adj.white_balance.tint),
    };
    let m = &meta.color.cam_to_working;
    let mut lum: Vec<f32> = preview
        .data
        .iter()
        .step_by(2)
        .map(|p| {
            let b = [p[0] * wb[0], p[1] * wb[1], p[2] * wb[2]];
            let w = crate::color::mat_vec(m, b);
            (0.2627 * w[0] + 0.678 * w[1] + 0.0593 * w[2]).max(1e-5)
        })
        .collect();
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

/// Levels from the displayed histogram: clip 0.05 % at both ends.
pub fn auto_levels(hist: &Histogram) -> Levels {
    let total: u64 = hist.luma.iter().map(|&v| v as u64).sum();
    if total == 0 {
        return Levels::default();
    }
    let thr = (total as f64 * 0.0005) as u64;
    let mut acc = 0u64;
    let mut lo = 0;
    for (i, &v) in hist.luma.iter().enumerate() {
        acc += v as u64;
        if acc > thr {
            lo = i;
            break;
        }
    }
    acc = 0;
    let mut hi = 255;
    for (i, &v) in hist.luma.iter().enumerate().rev() {
        acc += v as u64;
        if acc > thr {
            hi = i;
            break;
        }
    }
    if hi <= lo + 10 {
        return Levels::default();
    }
    Levels { in_black: (lo as f32 / 255.0).min(0.3), in_white: (hi as f32 / 255.0).max(0.7), ..Default::default() }
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
        let wb = auto_white_balance(&m, &img);
        let mult = m.color.wb_for_temp_tint(wb.kelvin, wb.tint);
        assert!((mult[0] / mult[1] - 0.5).abs() < 0.08, "{mult:?}");
    }

    #[test]
    fn levels_stretch_narrow_histogram() {
        let mut h = Histogram { r: vec![0; 256], g: vec![0; 256], b: vec![0; 256], luma: vec![0; 256] };
        for v in &mut h.luma[40..200] {
            *v = 100;
        }
        let l = auto_levels(&h);
        assert!(l.in_black > 0.14 && l.in_white < 0.8);
    }
}
