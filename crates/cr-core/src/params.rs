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
    pub geometry: Geometry,
    pub lens: LensCorrection,
    pub detail: Detail,
    pub color_editor: ColorEditor,
    pub color_balance: ColorBalance,
    pub black_white: BlackWhite,
    /// Local adjustment layers (applied on top of the background adjustments).
    pub layers: Vec<Layer>,
    /// Heal and clone spots.
    pub retouch: Vec<Spot>,
}

/// Adjustments of a local layer, as deltas added to the background values.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LayerAdjustments {
    /// EV
    pub exposure: f32,
    pub contrast: f32,
    pub brightness: f32,
    pub saturation: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub clarity: f32,
    pub structure: f32,
    /// Warm (+) / cool (-), -100..100
    pub temperature: f32,
    /// Magenta (+) / green (-), -100..100
    pub tint: f32,
}

impl LayerAdjustments {
    pub fn is_neutral(&self) -> bool {
        *self == LayerAdjustments::default()
    }
}

/// A brush stroke in normalized source coordinates (0..1 of the oriented image).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrushStroke {
    pub points: Vec<[f32; 2]>,
    /// Radius relative to the long image edge.
    pub radius: f32,
    /// 0 (soft) .. 1 (hard)
    pub hardness: f32,
    /// 0..1
    pub flow: f32,
    pub erase: bool,
}

/// Linear gradient: fully masked at `start`, unmasked at `end` (normalized source coordinates).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinearGradient {
    pub start: [f32; 2],
    pub end: [f32; 2],
}

/// Radial gradient: masked inside the ellipse, `feather` (0..1) controls the soft edge.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RadialGradient {
    pub center: [f32; 2],
    /// Radii relative to the long image edge.
    pub radius_x: f32,
    pub radius_y: f32,
    pub feather: f32,
    pub invert: bool,
}

/// Luminance range refinement of a mask (0..1 display luminance).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LumaRange {
    pub low: f32,
    pub high: f32,
    /// Soft transition width.
    pub falloff: f32,
    pub invert: bool,
}

impl Default for LumaRange {
    fn default() -> Self {
        Self { low: 0.0, high: 1.0, falloff: 0.1, invert: false }
    }
}

/// Color range refinement: hue (degrees) with a tolerance, plus minimum chroma.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorRange {
    pub hue: f32,
    /// Half width in degrees.
    pub width: f32,
    pub falloff: f32,
    /// Minimum Oklab chroma (0..0.3).
    pub min_chroma: f32,
}

impl Default for ColorRange {
    fn default() -> Self {
        Self { hue: 30.0, width: 25.0, falloff: 15.0, min_chroma: 0.03 }
    }
}

/// A mask predicted by a neural network (subject, sky, ...), stored as a bitmap.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiMask {
    pub kind: String,
    pub width: usize,
    pub height: usize,
    /// 8-bit mask values, row-major.
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Mask {
    /// Start fully masked ("Maske füllen").
    pub fill: bool,
    pub strokes: Vec<BrushStroke>,
    pub linear: Vec<LinearGradient>,
    pub radial: Vec<RadialGradient>,
    pub ai: Option<AiMask>,
    pub luma_range: Option<LumaRange>,
    pub color_range: Option<ColorRange>,
    pub invert: bool,
}

impl Mask {
    pub fn is_empty(&self) -> bool {
        !self.fill && self.strokes.is_empty() && self.linear.is_empty() && self.radial.is_empty() && self.ai.is_none()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layer {
    pub name: String,
    pub enabled: bool,
    /// 0..100
    pub opacity: f32,
    pub adjustments: LayerAdjustments,
    pub mask: Mask,
}

impl Default for Layer {
    fn default() -> Self {
        Self {
            name: "Ebene".into(),
            enabled: true,
            opacity: 100.0,
            adjustments: LayerAdjustments::default(),
            mask: Mask::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SpotMode {
    #[default]
    Heal,
    Clone,
}

/// Retouch spot in normalized source coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Spot {
    pub mode: SpotMode,
    pub target: [f32; 2],
    pub source: [f32; 2],
    /// Radius relative to the long image edge.
    pub radius: f32,
    /// 0..1
    pub feather: f32,
}

/// Crop rectangle, normalized (0..1) to the rotated and straightened frame.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CropRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Default for CropRect {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0, w: 1.0, h: 1.0 }
    }
}

impl CropRect {
    pub fn is_full(&self) -> bool {
        *self == CropRect::default()
    }

    /// Clamps into the unit square with a minimum size.
    pub fn clamped(self) -> Self {
        let w = self.w.clamp(0.02, 1.0);
        let h = self.h.clamp(0.02, 1.0);
        Self { x: self.x.clamp(0.0, 1.0 - w), y: self.y.clamp(0.0, 1.0 - h), w, h }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AspectRatio {
    #[default]
    Free,
    Original,
    Ratio(u16, u16),
}

impl AspectRatio {
    pub const PRESETS: [AspectRatio; 9] = [
        AspectRatio::Free,
        AspectRatio::Original,
        AspectRatio::Ratio(1, 1),
        AspectRatio::Ratio(3, 2),
        AspectRatio::Ratio(4, 3),
        AspectRatio::Ratio(5, 4),
        AspectRatio::Ratio(7, 5),
        AspectRatio::Ratio(16, 9),
        AspectRatio::Ratio(65, 24),
    ];

    pub fn label(self) -> String {
        match self {
            AspectRatio::Free => "Frei".into(),
            AspectRatio::Original => "Original".into(),
            AspectRatio::Ratio(65, 24) => "Panorama 65:24".into(),
            AspectRatio::Ratio(a, b) => format!("{a}:{b}"),
        }
    }

    /// Width/height ratio in pixels, given the frame's pixel aspect.
    pub fn value(self, frame_aspect: f32, portrait: bool) -> Option<f32> {
        let r = match self {
            AspectRatio::Free => return None,
            AspectRatio::Original => return Some(frame_aspect),
            AspectRatio::Ratio(a, b) => a as f32 / b as f32,
        };
        Some(if portrait { 1.0 / r } else { r })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Geometry {
    pub crop: CropRect,
    pub aspect: AspectRatio,
    /// Portrait orientation of the aspect ratio.
    pub aspect_portrait: bool,
    /// Straighten angle in degrees (-45..45), positive rotates counter-clockwise.
    pub straighten: f32,
    pub flip_h: bool,
    pub flip_v: bool,
    /// Perspective (keystone) correction, -100..100.
    pub keystone_v: f32,
    pub keystone_h: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LensCorrection {
    /// Lensfun profile name, if one is used.
    pub profile: Option<String>,
    /// Barrel (+) / pincushion (-) correction, -100..100.
    pub distortion: f32,
    /// Lateral chromatic aberration (red/cyan and blue/yellow), -100..100.
    pub ca_red: f32,
    pub ca_blue: f32,
    /// Vignetting correction (brightens corners), -100..100.
    pub vignetting: f32,
    /// Profile coefficients (filled in from the lens profile, applied on top of the sliders).
    pub profile_coeffs: Option<LensCoefficients>,
}

/// Lens model coefficients in lensfun conventions. Distortion and TCA radii are
/// normalized to half the shorter image side, vignetting to the half diagonal.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LensCoefficients {
    /// ptlens model: r_d = r_u * (a r^3 + b r^2 + c r + 1 - a - b - c)
    pub a: f32,
    pub b: f32,
    pub c: f32,
    /// poly5 model: r_d = r_u (1 + k1 r^2 + k2 r^4)
    pub poly5_k1: f32,
    pub poly5_k2: f32,
    /// TCA linear terms as deviation from 1 (lensfun `vr - 1`, `vb - 1`).
    pub tca_red: f32,
    pub tca_blue: f32,
    /// pa vignetting: c = 1 + k1 r^2 + k2 r^4 + k3 r^6
    pub vig_k1: f32,
    pub vig_k2: f32,
    pub vig_k3: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Detail {
    /// Unsharp mask amount in percent, 0..500.
    pub sharpen_amount: f32,
    /// Radius in pixels (full resolution), 0.2..3.0.
    pub sharpen_radius: f32,
    /// Threshold in 8-bit levels, 0..20.
    pub sharpen_threshold: f32,
    /// Noise reduction, 0..100.
    pub nr_luma: f32,
    pub nr_color: f32,
    /// Local contrast, -100..100.
    pub clarity: f32,
    pub structure: f32,
    /// Film grain, 0..100, size 0..100.
    pub grain_amount: f32,
    pub grain_size: f32,
    /// Post-crop vignette, -100 (dark) .. 100 (light).
    pub vignette_amount: f32,
}

impl Default for Detail {
    fn default() -> Self {
        Self {
            sharpen_amount: 120.0,
            sharpen_radius: 0.8,
            sharpen_threshold: 1.0,
            nr_luma: 0.0,
            nr_color: 25.0,
            clarity: 0.0,
            structure: 0.0,
            grain_amount: 0.0,
            grain_size: 25.0,
            vignette_amount: 0.0,
        }
    }
}

/// Hue bands of the color editor and the black & white mixer.
pub const HUE_BANDS: [(&str, f32); 8] = [
    ("Rot", 25.0),
    ("Orange", 55.0),
    ("Gelb", 95.0),
    ("Grün", 140.0),
    ("Cyan", 195.0),
    ("Blau", 255.0),
    ("Lila", 300.0),
    ("Magenta", 340.0),
];

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HslBand {
    /// Hue shift in degrees, -30..30.
    pub hue: f32,
    /// -100..100
    pub saturation: f32,
    /// -100..100
    pub lightness: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ColorEditor {
    pub bands: [HslBand; 8],
}

impl ColorEditor {
    pub fn is_neutral(&self) -> bool {
        self.bands.iter().all(|b| *b == HslBand::default())
    }
}

/// A color wheel: hue in degrees (0..360) and amount (0..100), plus a lightness shift.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ColorWheel {
    pub hue: f32,
    pub amount: f32,
    pub lightness: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ColorBalance {
    pub master: ColorWheel,
    pub shadows: ColorWheel,
    pub midtones: ColorWheel,
    pub highlights: ColorWheel,
}

impl ColorBalance {
    pub fn is_neutral(&self) -> bool {
        *self == ColorBalance::default()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct BlackWhite {
    pub enabled: bool,
    /// Channel sensitivity per hue band, -100..100.
    pub mix: [f32; 8],
    /// Split toning.
    pub shadow_tint: ColorWheel,
    pub highlight_tint: ColorWheel,
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
            geometry: Geometry::default(),
            lens: LensCorrection::default(),
            detail: Detail::default(),
            color_editor: ColorEditor::default(),
            color_balance: ColorBalance::default(),
            black_white: BlackWhite::default(),
            layers: Vec::new(),
            retouch: Vec::new(),
        }
    }
}

impl Adjustments {
    /// Copies only the framing (rotation, crop, straighten, ...), e.g. for the before view.
    pub fn keep_framing_from(&mut self, other: &Adjustments) {
        self.rotation = other.rotation;
        self.geometry = other.geometry.clone();
    }

    /// Copies the image-specific settings (rotation, crop, layers, retouching) from `other`.
    /// Used when pasting adjustments or applying styles, which should not change framing or masks.
    pub fn keep_geometry_from(&mut self, other: &Adjustments) {
        self.rotation = other.rotation;
        self.geometry = other.geometry.clone();
        // Masks and retouching are tied to image content.
        self.layers = other.layers.clone();
        self.retouch = other.retouch.clone();
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
