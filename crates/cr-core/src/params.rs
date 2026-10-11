//! Non-destructive adjustment parameters. Everything here is serialized into sidecars.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WbMode {
    #[default]
    AsShot,
    Custom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BaseCurve {
    /// Film-like curve with a soft highlight shoulder.
    #[default]
    FilmStandard,
    /// Higher contrast variant.
    FilmHigh,
    /// Pure linear scene response (gamma encoded only).
    Linear,
}

impl BaseCurve {
    pub const ALL: [BaseCurve; 3] = [BaseCurve::FilmStandard, BaseCurve::FilmHigh, BaseCurve::Linear];

    pub fn label(self) -> &'static str {
        match self {
            BaseCurve::FilmStandard => "Film Standard",
            BaseCurve::FilmHigh => "Film hoher Kontrast",
            BaseCurve::Linear => "Linear",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WhiteBalance {
    pub mode: WbMode,
    pub kelvin: f32,
    pub tint: f32,
}

impl Default for WhiteBalance {
    fn default() -> Self {
        Self { mode: WbMode::AsShot, kelvin: 5500.0, tint: 0.0 }
    }
}

/// Levels in sRGB-encoded 0..1 space.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Levels {
    pub in_black: f32,
    pub in_white: f32,
    /// Midtone gamma, 1.0 = neutral; >1 brightens.
    pub midtone: f32,
    pub out_black: f32,
    pub out_white: f32,
}

impl Default for Levels {
    fn default() -> Self {
        Self { in_black: 0.0, in_white: 1.0, midtone: 1.0, out_black: 0.0, out_white: 1.0 }
    }
}

/// A tone curve defined by control points in 0..1 (sRGB-encoded space).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Curve {
    pub points: Vec<[f32; 2]>,
}

impl Default for Curve {
    fn default() -> Self {
        Self { points: vec![[0.0, 0.0], [1.0, 1.0]] }
    }
}

impl Curve {
    pub fn is_identity(&self) -> bool {
        self.points.iter().all(|p| (p[0] - p[1]).abs() < 1e-6)
    }

    /// Monotone cubic (Fritsch-Carlson) interpolation through the control points.
    pub fn build_lut(&self, n: usize) -> Vec<f32> {
        let mut pts = self.points.clone();
        pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
        pts.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-6);
        if pts.len() < 2 {
            return (0..n).map(|i| i as f32 / (n - 1) as f32).collect();
        }
        let m = pts.len();
        let dx: Vec<f32> = (0..m - 1).map(|i| pts[i + 1][0] - pts[i][0]).collect();
        let slope: Vec<f32> = (0..m - 1).map(|i| (pts[i + 1][1] - pts[i][1]) / dx[i]).collect();
        let mut tan = vec![0.0f32; m];
        tan[0] = slope[0];
        tan[m - 1] = slope[m - 2];
        for i in 1..m - 1 {
            tan[i] = if slope[i - 1] * slope[i] <= 0.0 { 0.0 } else { (slope[i - 1] + slope[i]) / 2.0 };
        }
        for i in 0..m - 1 {
            if slope[i].abs() < 1e-9 {
                tan[i] = 0.0;
                tan[i + 1] = 0.0;
            } else {
                let a = tan[i] / slope[i];
                let b = tan[i + 1] / slope[i];
                let h = a * a + b * b;
                if h > 9.0 {
                    let t = 3.0 / h.sqrt();
                    tan[i] = t * a * slope[i];
                    tan[i + 1] = t * b * slope[i];
                }
            }
        }
        (0..n)
            .map(|i| {
                let x = i as f32 / (n - 1) as f32;
                if x <= pts[0][0] {
                    return pts[0][1];
                }
                if x >= pts[m - 1][0] {
                    return pts[m - 1][1];
                }
                let k = pts.partition_point(|p| p[0] <= x).saturating_sub(1).min(m - 2);
                let h = dx[k];
                let t = (x - pts[k][0]) / h;
                let (t2, t3) = (t * t, t * t * t);
                let y = (2.0 * t3 - 3.0 * t2 + 1.0) * pts[k][1]
                    + (t3 - 2.0 * t2 + t) * h * tan[k]
                    + (-2.0 * t3 + 3.0 * t2) * pts[k + 1][1]
                    + (t3 - t2) * h * tan[k + 1];
                y.clamp(0.0, 1.0)
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Curves {
    pub rgb: Curve,
    pub red: Curve,
    pub green: Curve,
    pub blue: Curve,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Adjustments {
    pub white_balance: WhiteBalance,
    pub base_curve: BaseCurve,
    /// Exposure in EV, -4..4
    pub exposure: f32,
    /// -100..100
    pub contrast: f32,
    /// -100..100
    pub brightness: f32,
    /// -100..100
    pub saturation: f32,
    /// -100..100
    pub vibrance: f32,
    /// HDR: positive recovers (darkens) highlights. -100..100
    pub highlights: f32,
    /// HDR: positive lifts shadows. -100..100
    pub shadows: f32,
    /// HDR: affects the brightest tones. -100..100
    pub whites: f32,
    /// HDR: affects the darkest tones. -100..100
    pub blacks: f32,
    pub levels: Levels,
    pub curves: Curves,
    /// Clockwise quarter turns applied after the EXIF orientation.
    pub rotation: u8,
}

impl Default for Adjustments {
    fn default() -> Self {
        Self {
            white_balance: WhiteBalance::default(),
            base_curve: BaseCurve::default(),
            exposure: 0.0,
            contrast: 0.0,
            brightness: 0.0,
            saturation: 0.0,
            vibrance: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
            levels: Levels::default(),
            curves: Curves::default(),
            rotation: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_curve_lut() {
        let lut = Curve::default().build_lut(256);
        for (i, v) in lut.iter().enumerate() {
            assert!((v - i as f32 / 255.0).abs() < 1e-5);
        }
    }

    #[test]
    fn curve_passes_through_points_and_is_monotone() {
        let c = Curve { points: vec![[0.0, 0.0], [0.25, 0.15], [0.75, 0.9], [1.0, 1.0]] };
        let lut = c.build_lut(1025);
        assert!((lut[256] - 0.15).abs() < 1e-3);
        assert!((lut[768] - 0.9).abs() < 1e-3);
        assert!(lut.windows(2).all(|w| w[1] >= w[0] - 1e-6));
    }
}
