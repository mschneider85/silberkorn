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
    /// The tone of the camera's own JPEG, fitted per image (see [`CameraCurve`]); Film Standard
    /// while there is none.
    Camera,
}

impl BaseCurve {
    pub const ALL: [BaseCurve; 4] = [BaseCurve::Camera, BaseCurve::FilmStandard, BaseCurve::FilmHigh, BaseCurve::Linear];

    pub fn label(self) -> &'static str {
        match self {
            BaseCurve::FilmStandard => "Film Standard",
            BaseCurve::FilmHigh => "Film hoher Kontrast",
            BaseCurve::Linear => "Linear",
            BaseCurve::Camera => "Kamera",
        }
    }

    /// What the curve does, for the hover text.
    pub fn hint(self) -> &'static str {
        match self {
            BaseCurve::Camera => "Tonkurve des Kamera-JPEGs, das in der RAW-Datei steckt, für jedes Bild angepasst: Helligkeit, Kontrast und Lichter wie in der Kamera. Standard für neue Bilder.",
            BaseCurve::FilmStandard => "Filmähnliche Kurve mit weicher Schulter in den Lichtern",
            BaseCurve::FilmHigh => "Wie Film Standard, mit mehr Kontrast",
            BaseCurve::Linear => "Ohne Kurve, nur die Gammakodierung: Ausgangspunkt für eine eigene Gradation",
        }
    }
}

/// Tone curve of the camera's own JPEG (see `camera_look::fit`): display-linear luminance over
/// scene brightness in EV relative to middle grey, at strictly increasing EV. Below the first
/// point the curve runs towards black with the slope of its start, above the last it rolls off
/// towards white.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct CameraCurve {
    /// (EV, display-linear value 0..1)
    pub points: Vec<[f32; 2]>,
}

impl CameraCurve {
    /// Display-linear value for a scene-linear value `v` (middle grey 0.18).
    pub fn eval(&self, v: f32) -> f32 {
        let p = &self.points;
        if p.len() < 2 || v <= 0.0 {
            return 0.0;
        }
        let ev = (v / 0.18).log2();
        let n = p.len();
        if ev <= p[0][0] {
            // Power law through the first point with the slope (in log-log) of the first segment.
            let g = ((p[1][1] / p[0][1].max(1e-6)).log2() / (p[1][0] - p[0][0]).max(1e-3)).clamp(0.8, 3.0);
            return p[0][1] * (ev - p[0][0]).exp2().powf(g);
        }
        if ev >= p[n - 1][0] {
            // Exponential roll-off to 1 with the slope of the last segment (in linear terms).
            let (v0, v1) = (0.18 * p[n - 2][0].exp2(), 0.18 * p[n - 1][0].exp2());
            let y = p[n - 1][1];
            let slope = ((y - p[n - 2][1]) / (v1 - v0).max(1e-6)).max(1e-4);
            let k = slope / (1.0 - y).max(1e-4);
            return (1.0 - (1.0 - y) * (-(v - v1) * k).exp()).min(1.0);
        }
        let i = p.partition_point(|q| q[0] <= ev).clamp(1, n - 1);
        let (a, b) = (p[i - 1], p[i]);
        let t = (ev - a[0]) / (b[0] - a[0]).max(1e-6);
        a[1] + (b[1] - a[1]) * t
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

/// Levels in sRGB-encoded 0..1 space: for all channels, then per channel.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Levels {
    /// All channels; stored at the top level as before there were channels.
    #[serde(flatten)]
    pub rgb: LevelSet,
    #[serde(skip_serializing_if = "LevelSet::is_neutral")]
    pub red: LevelSet,
    #[serde(skip_serializing_if = "LevelSet::is_neutral")]
    pub green: LevelSet,
    #[serde(skip_serializing_if = "LevelSet::is_neutral")]
    pub blue: LevelSet,
}

impl Levels {
    /// `v` of channel `c` (0 red, 1 green, 2 blue) through the levels of all, then of `c`.
    pub fn apply(&self, c: usize, v: f32) -> f32 {
        [&self.red, &self.green, &self.blue][c].apply(self.rgb.apply(v))
    }
}

/// Black and white point, midtones and output range of one channel.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LevelSet {
    pub in_black: f32,
    pub in_white: f32,
    /// Midtone gamma, 1.0 = neutral; >1 brightens.
    pub midtone: f32,
    pub out_black: f32,
    pub out_white: f32,
}

impl Default for LevelSet {
    fn default() -> Self {
        Self { in_black: 0.0, in_white: 1.0, midtone: 1.0, out_black: 0.0, out_white: 1.0 }
    }
}

impl LevelSet {
    pub fn is_neutral(&self) -> bool {
        *self == Self::default()
    }

    /// Input range, midtone gamma, output range.
    pub fn apply(&self, v: f32) -> f32 {
        if self.is_neutral() {
            return v;
        }
        let range = (self.in_white - self.in_black).max(1e-4);
        let mut v = ((v - self.in_black) / range).clamp(0.0, 1.0);
        if self.midtone != 1.0 {
            v = v.powf(1.0 / self.midtone.max(0.05));
        }
        self.out_black + v * (self.out_white - self.out_black)
    }

    /// Between `self` (t = 0) and `other` (t = 1).
    pub fn lerp(&self, other: &Self, t: f32) -> Self {
        let l = |a: f32, b: f32| a + (b - a) * t;
        Self {
            in_black: l(self.in_black, other.in_black),
            in_white: l(self.in_white, other.in_white),
            midtone: l(self.midtone, other.midtone),
            out_black: l(self.out_black, other.out_black),
            out_white: l(self.out_white, other.out_white),
        }
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
    /// Brightness only (luma curve): applied after the others to the
    /// luminance, colors are scaled with it, so hue and saturation stay.
    pub luma: Curve,
}

impl Curves {
    pub fn is_identity(&self) -> bool {
        [&self.rgb, &self.red, &self.green, &self.blue, &self.luma].iter().all(|c| c.is_identity())
    }
}

/// Process versions of the renderer: edits keep the look
/// they were made with, also when a later Silberkorn renders differently.
///
/// 1. Original renderer.
/// 2. Colors outside the output space are desaturated towards their own luminance (hue kept)
///    instead of clipped per channel, which shifted the hue of saturated light.
/// 3. The tone curve keeps hues: it is applied to the largest and smallest channel and the
///    middle one follows (instead of each channel on its own, which turned skin orange and
///    skies cyan). Highlights/shadows work on a median of the brightness, so fine lines and
///    texture keep their contrast.
/// 4. Partly clipped highlights are reconstructed from the unclipped channels and the color of
///    their surroundings (instead of all made white).
pub const PROCESS_LATEST: u32 = 4;
/// Version of settings saved before process versions existed.
pub const PROCESS_LEGACY: u32 = 1;

fn process_legacy() -> u32 {
    PROCESS_LEGACY
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Adjustments {
    /// Process version (see [`PROCESS_LATEST`]); new and reset images get the latest one.
    #[serde(default = "process_legacy")]
    pub process: u32,
    pub white_balance: WhiteBalance,
    pub base_curve: BaseCurve,
    /// The camera's tone for [`BaseCurve::Camera`], fitted to this image.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera_curve: Option<CameraCurve>,
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
    pub skin_tone: SkinTone,
    pub color_balance: ColorBalance,
    pub black_white: BlackWhite,
    /// Local adjustment layers (applied on top of the background adjustments).
    pub layers: Vec<Layer>,
    /// Heal and clone spots.
    pub retouch: Vec<Spot>,
    /// Objects removed by generative fill (applied before the spots).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removals: Vec<Removal>,
    /// Lens blur from the depth map.
    #[serde(skip_serializing_if = "LensBlur::is_default")]
    pub lens_blur: LensBlur,
    /// Near and far parts lit separately, by the depth map.
    #[serde(skip_serializing_if = "Relight::is_default")]
    pub relight: Relight,
    /// Haze growing with the distance, by the depth map.
    #[serde(skip_serializing_if = "Atmosphere::is_default")]
    pub atmosphere: Atmosphere,
    /// Tools switched off: their settings are kept, but the image is made (shown and exported)
    /// as if they were reset. See [`Adjustments::visible`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<crate::tools::Tool>,
}

/// Something removed from the image: the strokes that marked it and the generated content
/// shown there (see `mask::apply_removals`), in normalized source coordinates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Removal {
    /// Identifies the generated content (for caches).
    pub id: u64,
    pub strokes: Vec<BrushStroke>,
    /// Area of the patch: x, y, width, height (0..1 of the source).
    pub rect: [f32; 4],
    /// Camera RGB (divided by `scale`) and coverage as a 16-bit RGBA PNG, base64 encoded.
    pub patch: std::sync::Arc<String>,
    pub scale: f32,
    /// 0..1
    pub opacity: f32,
}

impl Default for Removal {
    fn default() -> Self {
        Self { id: 0, strokes: Vec::new(), rect: [0.0; 4], patch: Default::default(), scale: 0.0, opacity: 1.0 }
    }
}

/// Adjustments of a local layer, as deltas added to the background values; the curves act on
/// the result of the background's (see `pipeline`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
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
    /// Haze removal, -100..100
    pub dehaze: f32,
    #[serde(skip_serializing_if = "is_zero_f32")]
    pub whites: f32,
    #[serde(skip_serializing_if = "is_zero_f32")]
    pub blacks: f32,
    #[serde(skip_serializing_if = "is_zero_f32")]
    pub vibrance: f32,
    /// Shifts per hue band, added to those of the color editor.
    #[serde(skip_serializing_if = "bands_neutral")]
    pub color_bands: [HslBand; 8],
    /// Color wheels, added to those of the color balance (as offsets, see `ColorWheel::offset`).
    #[serde(skip_serializing_if = "ColorBalance::is_neutral")]
    pub color_balance: ColorBalance,
    #[serde(skip_serializing_if = "Curves::is_identity")]
    pub curves: Curves,
}

fn bands_neutral(b: &[HslBand; 8]) -> bool {
    b.iter().all(|b| *b == HslBand::default())
}

impl LayerAdjustments {
    pub fn is_neutral(&self) -> bool {
        *self == LayerAdjustments::default()
    }

    /// No deltas to the background values (curves aside).
    pub fn deltas_neutral(&self) -> bool {
        LayerAdjustments { curves: Curves::default(), ..self.clone() }.is_neutral()
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
    /// 0..1: how much each dab adds (coverage builds up within a stroke, see `brush`).
    pub flow: f32,
    pub erase: bool,
    /// 0..1: the most the stroke covers.
    #[serde(default = "one")]
    pub opacity: f32,
    /// Only where the image resembles what is under the brush's center.
    #[serde(default)]
    pub auto_mask: bool,
    /// Rasterized as smooth dabs with build-up (`brush::paint_stroke`). Strokes saved before
    /// had a polyline with `flow` as opacity; they are drawn as before.
    #[serde(default)]
    pub build_up: bool,
    /// How different the image may be from what is under the brush's center and still be
    /// painted (`auto_mask`), 0..100.
    #[serde(default = "default_tolerance", skip_serializing_if = "is_default_tolerance")]
    pub tolerance: f32,
}

/// The tolerance of automatic masking strokes had before it could be set.
pub const DEFAULT_TOLERANCE: f32 = 50.0;

fn default_tolerance() -> f32 {
    DEFAULT_TOLERANCE
}

fn is_default_tolerance(v: &f32) -> bool {
    *v == DEFAULT_TOLERANCE
}

fn one() -> f32 {
    1.0
}

/// Linear gradient: fully masked at `start`, unmasked at `end` (normalized source coordinates).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinearGradient {
    pub start: [f32; 2],
    pub end: [f32; 2],
}

/// Radial gradient: masked inside the ellipse, `feather` (0..1) controls the soft edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RadialGradient {
    pub center: [f32; 2],
    /// Radii relative to the long image edge.
    pub radius_x: f32,
    pub radius_y: f32,
    pub feather: f32,
    pub invert: bool,
    /// Rotation of the ellipse (radians, clockwise on screen).
    #[serde(default)]
    pub angle: f32,
}

/// Luminance range refinement of a mask (0..1 display luminance): fully in between `low` and
/// `high`, fading out towards `falloff_low` and `falloff_high` (as Capture One's Luma Range).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "LumaRangeStored")]
pub struct LumaRange {
    pub falloff_low: f32,
    pub low: f32,
    pub high: f32,
    pub falloff_high: f32,
}

impl Default for LumaRange {
    fn default() -> Self {
        Self { falloff_low: 0.0, low: 0.0, high: 1.0, falloff_high: 1.0 }
    }
}

impl LumaRange {
    /// The edges of the soft transitions, at least a hair outside the range (a hard edge).
    pub fn edges(&self) -> [f32; 4] {
        [self.falloff_low.min(self.low - 1e-3), self.low, self.high, self.falloff_high.max(self.high + 1e-3)]
    }

    /// How much of a display luminance `l` (0..1) is in the range.
    pub fn weight(&self, l: f32) -> f32 {
        trapezoid(self.edges(), l)
    }
}

/// 1 between the inner `edges` (1 and 2), fading out smoothly to the outer ones (0 and 3).
fn trapezoid([a, b, c, d]: [f32; 4], x: f32) -> f32 {
    let smoothstep = |e0: f32, e1: f32, x: f32| {
        let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    smoothstep(a, b, x) * (1.0 - smoothstep(c, d, x))
}

/// A luminance range as stored: before 0.2.11 one soft width for both sides and an "invert".
#[derive(Deserialize)]
#[serde(default)]
struct LumaRangeStored {
    falloff_low: Option<f32>,
    low: f32,
    high: f32,
    falloff_high: Option<f32>,
    falloff: f32,
    invert: bool,
}

impl Default for LumaRangeStored {
    fn default() -> Self {
        Self { falloff_low: None, low: 0.0, high: 1.0, falloff_high: None, falloff: 0.1, invert: false }
    }
}

impl From<LumaRangeStored> for LumaRange {
    fn from(s: LumaRangeStored) -> Self {
        if let (Some(falloff_low), Some(falloff_high)) = (s.falloff_low, s.falloff_high) {
            return Self { falloff_low, low: s.low, high: s.high, falloff_high };
        }
        let (lo, hi, f) = (s.low, s.high, s.falloff.max(1e-3));
        // Inverted, what was outside the range: the part above it, or below it. Both at once is
        // no longer possible; the larger part is kept.
        if s.invert {
            if lo <= 0.0 || (hi < 1.0 && 1.0 - hi >= lo) {
                return Self { falloff_low: hi, low: (hi + f).min(1.0), high: 1.0, falloff_high: 1.0 };
            }
            return Self { falloff_low: 0.0, low: 0.0, high: (lo - f).max(0.0), falloff_high: lo };
        }
        Self { falloff_low: lo - f, low: lo, high: hi, falloff_high: hi + f }
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
    /// 8-bit mask values, row-major. Shared between copies (undo steps, history): it doesn't
    /// change once made.
    pub data: std::sync::Arc<Vec<u8>>,
    /// The depth range the mask was made from (kind "depth"), so it can be changed later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<DepthRange>,
}

/// A range of the depth map (see `depth`): 0 is the farthest part of the image, 1 the nearest.
/// Fully in between `far` and `near`, fading out towards `falloff_far` and `falloff_near` (as
/// the luminance range).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "DepthRangeStored")]
pub struct DepthRange {
    pub falloff_far: f32,
    pub far: f32,
    pub near: f32,
    pub falloff_near: f32,
}

impl Default for DepthRange {
    fn default() -> Self {
        Self::nearer_than(0.6)
    }
}

impl DepthRange {
    /// Everything nearer than `far`, with a soft edge behind it.
    pub fn nearer_than(far: f32) -> Self {
        Self { falloff_far: far - 0.1, far, near: 1.0, falloff_near: 1.0 }
    }

    /// The edges of the soft transitions, at least a hair outside the range (a hard edge).
    pub fn edges(&self) -> [f32; 4] {
        [self.falloff_far.min(self.far - 1e-3), self.far, self.near, self.falloff_near.max(self.near + 1e-3)]
    }

    /// Mask value at depth `d`.
    pub fn weight(&self, d: f32) -> f32 {
        trapezoid(self.edges(), d)
    }
}

/// A depth range as stored: before 0.2.12 one soft width for both sides, and the ends either
/// way round.
#[derive(Deserialize)]
#[serde(default)]
struct DepthRangeStored {
    falloff_far: Option<f32>,
    far: f32,
    near: f32,
    falloff_near: Option<f32>,
    falloff: f32,
}

impl Default for DepthRangeStored {
    fn default() -> Self {
        Self { falloff_far: None, far: 0.6, near: 1.0, falloff_near: None, falloff: 0.1 }
    }
}

impl From<DepthRangeStored> for DepthRange {
    fn from(s: DepthRangeStored) -> Self {
        if let (Some(falloff_far), Some(falloff_near)) = (s.falloff_far, s.falloff_near) {
            return Self { falloff_far, far: s.far, near: s.near, falloff_near };
        }
        let (lo, hi, f) = (s.far.min(s.near), s.far.max(s.near), s.falloff.max(1e-3));
        Self { falloff_far: lo - f, far: lo, near: hi, falloff_near: hi + f }
    }
}

/// Lens blur from the depth map (see `lens_blur`): parts in front of and behind the focus are
/// blurred like by a wide aperture.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LensBlur {
    /// 0..100; 0 is off.
    pub amount: f32,
    /// Depth in focus (0 far .. 1 near).
    pub focus: f32,
    /// Depth range around the focus that stays sharp.
    pub range: f32,
    /// Brightens highlights in the blurred parts into discs of light (bokeh), 0..100.
    pub bokeh: f32,
}

impl Default for LensBlur {
    fn default() -> Self {
        Self { amount: 0.0, focus: 0.9, range: 0.1, bokeh: 0.0 }
    }
}

impl LensBlur {
    pub fn is_off(&self) -> bool {
        self.amount <= 0.0
    }

    /// Unchanged (not saved): switched off, the focus and range stay saved for later.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Lights the near and the far part of the image separately, by the
/// depth map: internally two layers with depth masks (see `pipeline`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Relight {
    /// Exposure of the near part in EV, -2..2.
    pub near: f32,
    /// Warm (+) / cool (-) of the near part, -100..100.
    pub near_warmth: f32,
    pub far: f32,
    pub far_warmth: f32,
    /// Depth where near turns into far (0 far .. 1 near).
    pub split: f32,
    /// Width of the transition.
    pub falloff: f32,
}

impl Default for Relight {
    fn default() -> Self {
        Self { near: 0.0, near_warmth: 0.0, far: 0.0, far_warmth: 0.0, split: 0.6, falloff: 0.3 }
    }
}

impl Relight {
    pub fn is_off(&self) -> bool {
        self.near == 0.0 && self.near_warmth == 0.0 && self.far == 0.0 && self.far_warmth == 0.0
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The depth ranges of the near and the far part; their weights add up to one.
    pub fn ranges(&self) -> (DepthRange, DepthRange) {
        let f = self.falloff.max(0.02);
        let (a, b) = (self.split - f / 2.0, self.split + f / 2.0);
        let near = DepthRange { falloff_far: a, far: b, near: 1.0, falloff_near: 1.0 };
        let far = DepthRange { falloff_far: 0.0, far: 0.0, near: a, falloff_near: b };
        (near, far)
    }
}

/// Haze that grows with the distance, by the depth map: internally a layer that veils the
/// image with the haze's color through a mask of its density (see `pipeline`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Atmosphere {
    /// 0..100; 0 is off.
    pub amount: f32,
    /// How far towards the front the haze reaches (0 only the farthest part .. 1 everything).
    pub reach: f32,
    /// Brightens the haze, -100..100.
    pub brightness: f32,
    /// Warm (+) / cool (-), -100..100.
    pub warmth: f32,
}

impl Default for Atmosphere {
    fn default() -> Self {
        Self { amount: 0.0, reach: 0.5, brightness: 0.0, warmth: 0.0 }
    }
}

impl Atmosphere {
    pub fn is_off(&self) -> bool {
        self.amount <= 0.0
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Density of the haze at a depth (0 far .. 1 near): 1 at the farthest part, 0 from `reach`
    /// on. The depth is inverse (1 / distance), so the distance falls fast from the far end:
    /// the haze thins out quickly first and then slowly towards `reach` (as light through air,
    /// `1 - exp(-k * distance)`), the middle distance stays clearer than the far one.
    pub fn density(&self, d: f32) -> f32 {
        let u = (d / self.reach.clamp(0.02, 1.0)).clamp(0.0, 1.0);
        // Distance from `reach` (0) to the farthest part (1).
        let z = (1.0 - u) / (1.0 + 3.0 * u);
        const K: f32 = 2.0;
        (1.0 - (-K * z).exp()) / (1.0 - (-K).exp())
    }
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
    /// Identifies the layer while it is edited (its index changes when layers are deleted or
    /// edits undone): unique within the image, 0 until [`Adjustments::assign_layer_ids`].
    #[serde(skip_serializing_if = "is_zero")]
    pub id: u32,
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
            id: 0,
            name: "Ebene".into(),
            enabled: true,
            opacity: 100.0,
            adjustments: LayerAdjustments::default(),
            mask: Mask::default(),
        }
    }
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SpotMode {
    #[default]
    Heal,
    Clone,
}

/// Retouch area in normalized source coordinates: a round spot, or a painted stroke when
/// `path` has points.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Spot {
    pub mode: SpotMode,
    /// Start of the area (the spot's center, or the first point of a stroke).
    pub target: [f32; 2],
    /// Where `target` takes its pixels from; the whole area is shifted by the same offset.
    pub source: [f32; 2],
    /// Further points of a painted stroke after `target`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<[f32; 2]>,
    /// Strokes painted onto the area afterwards, with its radius and source offset: they add
    /// to it or take away from it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strokes: Vec<SpotStroke>,
    /// Radius relative to the long image edge.
    pub radius: f32,
    /// 0..1
    pub feather: f32,
    /// 0..1
    #[serde(default = "one")]
    pub opacity: f32,
}

/// A stroke that extends a retouch area or (`erase`) takes part of it away.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpotStroke {
    pub points: Vec<[f32; 2]>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub erase: bool,
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
        // NaN (a damaged sidecar, a zero-sized frame) would make the bounds below NaN, and
        // `clamp` panics on those.
        let finite = |v: f32, or: f32| if v.is_finite() { v } else { or };
        let w = finite(self.w, 1.0).clamp(0.02, 1.0);
        let h = finite(self.h, 1.0).clamp(0.02, 1.0);
        Self { x: finite(self.x, 0.0).clamp(0.0, 1.0 - w), y: finite(self.y, 0.0).clamp(0.0, 1.0 - h), w, h }
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

/// Largest keystone value the sliders and the automatic correction set: beyond it the image
/// is stretched more than it looks good.
pub const KEYSTONE_MAX: f32 = 50.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// Perspective (keystone) correction, -KEYSTONE_MAX..KEYSTONE_MAX (older edits may hold
    /// up to ±100, which still render as before).
    pub keystone_v: f32,
    pub keystone_h: f32,
    /// How much of the keystone correction is applied, 0..100 %: a little less than all
    /// often looks more natural (a building still narrowing slightly towards the top).
    pub keystone_amount: f32,
}

impl Default for Geometry {
    fn default() -> Self {
        Self {
            crop: CropRect::default(),
            aspect: AspectRatio::default(),
            aspect_portrait: false,
            straighten: 0.0,
            flip_h: false,
            flip_v: false,
            keystone_v: 0.0,
            keystone_h: 0.0,
            keystone_amount: 100.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LensCorrection {
    /// Apply the correction the camera stored in the raw file (`RawMeta::lens`), if any. On for
    /// new images; settings saved before it existed keep their look.
    #[serde(default)]
    pub camera: bool,
    /// Lensfun profile name, if one is used.
    pub profile: Option<String>,
    /// Barrel (+) / pincushion (-) correction, -100..100.
    pub distortion: f32,
    /// Fisheye to rectilinear: diagonal angle of view of the fisheye in degrees (0 = off,
    /// 180 = a typical full-frame fisheye). Assumes the equidistant projection.
    pub fisheye: f32,
    /// Lateral chromatic aberration (red/cyan and blue/yellow), -100..100.
    pub ca_red: f32,
    pub ca_blue: f32,
    /// Removing purple and green fringes along edges (longitudinal chromatic aberration), 0..100.
    pub defringe_purple: f32,
    pub defringe_green: f32,
    /// Vignetting correction (brightens corners), -100..100.
    pub vignetting: f32,
    /// Profile coefficients (filled in from the lens profile, applied on top of the sliders).
    pub profile_coeffs: Option<LensCoefficients>,
}

impl Default for LensCorrection {
    fn default() -> Self {
        Self { camera: true, profile: None, distortion: 0.0, fisheye: 0.0, ca_red: 0.0, ca_blue: 0.0, defringe_purple: 0.0, defringe_green: 0.0, vignetting: 0.0, profile_coeffs: None }
    }
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
    /// Radius in pixels (full resolution), the sigma of the blur: 0.2..1.5 (older edits up to 3.0).
    pub sharpen_radius: f32,
    /// Threshold in 8-bit levels, 0..20.
    pub sharpen_threshold: f32,
    /// How the sharpening is made, 0 (unsharp mask) .. 100 (deconvolution), see [`Self::sharpen_split`].
    #[serde(skip_serializing_if = "is_zero_f32")]
    pub sharpen_detail: f32,
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
    /// Haze removal (+) or added haze (-), -100..100.
    pub dehaze: f32,
    /// AI denoising (see `ai_products`): how much of the cleaned raw data replaces the plain one, 0..100.
    pub ai_denoise: f32,
    /// Raw-Details (see `ai_products`): how much of the network's demosaic replaces AHD's, 0..100.
    #[serde(skip_serializing_if = "is_zero_f32")]
    pub raw_details: f32,

    /// Glow (Orton effect): a blurred copy of the bright parts screened over the image, 0..100.
    #[serde(skip_serializing_if = "is_zero_f32")]
    pub glow_amount: f32,
    /// Size of the glow, 0..100.
    #[serde(skip_serializing_if = "is_default_glow_size")]
    pub glow_size: f32,
}

/// Default size of the glow.
pub const GLOW_SIZE: f32 = 40.0;

fn is_zero_f32(v: &f32) -> bool {
    *v == 0.0
}

fn is_default_glow_size(v: &f32) -> bool {
    *v == GLOW_SIZE
}

/// Sharpening amount (percent) as sharp as the full deconvolution.
pub const DECONVOLUTION_FULL: f32 = 120.0;

impl Detail {
    /// The sharpening by method: (unsharp mask amount in percent, deconvolution 0..1). Details
    /// shares the amount between them; the deconvolution is complete at [`DECONVOLUTION_FULL`],
    /// beyond that more comes as unsharp mask, so that a larger amount is always sharper.
    pub fn sharpen_split(&self) -> (f32, f32) {
        let (s, d) = (self.sharpen_amount.max(0.0), (self.sharpen_detail / 100.0).clamp(0.0, 1.0));
        let deconvolution = d * (s / DECONVOLUTION_FULL).min(1.0);
        let unsharp = s * (1.0 - d) + d * (s - DECONVOLUTION_FULL).max(0.0);
        (unsharp, deconvolution)
    }

    /// Raw-Details as it acts: not at all under full KI-Entrauschen, whose network demosaics
    /// Bayer mosaics itself and so replaces everything Raw-Details does.
    pub fn effective_raw_details(&self) -> f32 {
        if self.ai_denoise >= 100.0 { 0.0 } else { self.raw_details }
    }
}

impl Default for Detail {
    fn default() -> Self {
        Self {
            sharpen_amount: 120.0,
            sharpen_radius: 0.8,
            sharpen_threshold: 1.0,
            sharpen_detail: 0.0,
            nr_luma: 0.0,
            nr_color: 25.0,
            clarity: 0.0,
            structure: 0.0,
            grain_amount: 0.0,
            grain_size: 25.0,
            vignette_amount: 0.0,
            dehaze: 0.0,
            ai_denoise: 0.0,
            raw_details: 0.0,
            glow_amount: 0.0,
            glow_size: GLOW_SIZE,
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
    /// Color ranges of one's own, applied after
    /// the bands.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub zones: Vec<ColorZone>,
}

/// The most color ranges of one's own (the GPU reserves room for these).
pub const MAX_COLOR_ZONES: usize = 8;

impl ColorEditor {
    pub fn is_neutral(&self) -> bool {
        self.bands.iter().all(|b| *b == HslBand::default()) && self.zones.iter().all(|z| !z.is_active())
    }

    /// The zones that change something, at most [`MAX_COLOR_ZONES`].
    pub fn active_zones(&self) -> impl Iterator<Item = &ColorZone> {
        self.zones.iter().filter(|z| z.is_active()).take(MAX_COLOR_ZONES)
    }
}

/// A color range of one's own: a hue (Oklab degrees) with a width and a soft edge, and how its
/// colors change.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorZone {
    pub enabled: bool,
    /// Center hue in degrees, 0..360.
    pub hue: f32,
    /// Half width in degrees, 3..90: fully affected up to the soft edge, not at all beyond.
    pub width: f32,
    /// Share of the width that fades out, 0..100.
    pub softness: f32,
    pub adjust: HslBand,
}

impl Default for ColorZone {
    fn default() -> Self {
        Self { enabled: true, hue: 30.0, width: 25.0, softness: 50.0, adjust: HslBand::default() }
    }
}

impl ColorZone {
    pub fn is_active(&self) -> bool {
        self.enabled && self.adjust != HslBand::default()
    }

    /// Hue distance (degrees) up to which the range has full effect, and where it ends.
    pub fn edges(&self) -> (f32, f32) {
        let outer = self.width.max(0.5);
        let inner = outer * (1.0 - (self.softness / 100.0).clamp(0.0, 1.0));
        (inner, outer.max(inner + 0.01))
    }

    /// How much a hue (degrees) belongs to the range, 0..1.
    pub fn weight(&self, h: f32) -> f32 {
        let d = ((h - self.hue + 540.0).rem_euclid(360.0) - 180.0).abs();
        let (inner, outer) = self.edges();
        let t = ((d - inner) / (outer - inner)).clamp(0.0, 1.0);
        1.0 - t * t * (3.0 - 2.0 * t)
    }
}

/// Skin tone: evens out colors around a target (usually picked from a face).
/// Colors are in Oklab at the input of the color stage.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkinTone {
    /// Target hue in degrees (Oklab), chroma and lightness.
    pub hue: f32,
    pub chroma: f32,
    pub lightness: f32,
    /// Half width of the affected hue range in degrees, 10..60.
    pub range: f32,
    /// Pull towards the target, 0..100.
    pub hue_uniformity: f32,
    pub saturation_uniformity: f32,
    pub lightness_uniformity: f32,
    /// Shifts of the whole range: hue in degrees -30..30, saturation and lightness -100..100.
    pub hue_shift: f32,
    pub saturation: f32,
    pub lightness_shift: f32,
}

impl Default for SkinTone {
    fn default() -> Self {
        Self {
            hue: 60.0,
            chroma: 0.07,
            lightness: 0.7,
            range: 25.0,
            hue_uniformity: 0.0,
            saturation_uniformity: 0.0,
            lightness_uniformity: 0.0,
            hue_shift: 0.0,
            saturation: 0.0,
            lightness_shift: 0.0,
        }
    }
}

impl SkinTone {
    /// Nothing is changed (the target and range alone have no effect).
    pub fn is_neutral(&self) -> bool {
        [self.hue_uniformity, self.saturation_uniformity, self.lightness_uniformity, self.hue_shift, self.saturation, self.lightness_shift]
            .iter()
            .all(|v| *v == 0.0)
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

impl ColorWheel {
    /// The wheel as offsets that can be added up: a and b (amount 0..1 in the hue's
    /// direction) and lightness (-1..1).
    pub fn offset(&self) -> [f32; 3] {
        let hr = self.hue.to_radians();
        let amt = self.amount / 100.0;
        [amt * hr.cos(), amt * hr.sin(), self.lightness / 100.0]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
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
            process: PROCESS_LATEST,
            white_balance: WhiteBalance::default(),
            base_curve: BaseCurve::default(),
            camera_curve: None,
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
            skin_tone: SkinTone::default(),
            color_balance: ColorBalance::default(),
            black_white: BlackWhite::default(),
            layers: Vec::new(),
            retouch: Vec::new(),
            removals: Vec::new(),
            lens_blur: LensBlur::default(),
            relight: Relight::default(),
            atmosphere: Atmosphere::default(),
            hidden: Vec::new(),
        }
    }
}

impl Adjustments {
    /// The settings that take effect: the switched-off tools as on a new image.
    pub fn visible(&self) -> std::borrow::Cow<'_, Adjustments> {
        if self.hidden.is_empty() {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut out = crate::tools::apply(&self.hidden, &Adjustments::default(), self);
        out.hidden.clear();
        std::borrow::Cow::Owned(out)
    }

    /// Switches a tool off, or on again.
    pub fn toggle_hidden(&mut self, tool: crate::tools::Tool) {
        match self.hidden.iter().position(|t| *t == tool) {
            Some(i) => {
                self.hidden.remove(i);
            }
            None => self.hidden.push(tool),
        }
    }

    /// Something uses the depth map (lens blur, relight, atmosphere); not counting depth masks
    /// of layers, which keep their own bitmap.
    pub fn needs_depth(&self) -> bool {
        !self.lens_blur.is_off() || !self.relight.is_off() || !self.atmosphere.is_off()
    }

    /// Copies only the framing (rotation, crop, straighten, ...), e.g. for the before view.
    pub fn keep_framing_from(&mut self, other: &Adjustments) {
        self.process = other.process;
        self.rotation = other.rotation;
        self.geometry = other.geometry.clone();
    }

    /// Settings as read from a file: without any edit they count as made with the latest process
    /// version (an older file that only holds a rating is not an edit to preserve); damaged AI
    /// masks are dropped.
    pub fn normalized(mut self) -> Self {
        // A damaged or hand-edited sidecar: a bitmap whose data doesn't match its size would be
        // read past its end.
        for l in &mut self.layers {
            l.mask.ai.take_if(|a| a.data.len() != a.width * a.height);
        }
        if (Adjustments { process: PROCESS_LATEST, ..self.clone() }) == Adjustments::default() {
            self.process = PROCESS_LATEST;
        }
        self
    }

    /// Gives every layer without an id (new, pasted, from an older sidecar), or with one another
    /// layer has too, a new one. Returns whether anything changed.
    ///
    /// New ids are not reused within the process, so that an id kept by a running job (or an
    /// undone state) never meets a different layer.
    pub fn assign_layer_ids(&mut self) -> bool {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(1);
        let mut changed = false;
        for i in 0..self.layers.len() {
            let id = self.layers[i].id;
            NEXT.fetch_max(id.saturating_add(1), Ordering::Relaxed);
            if id == 0 || self.layers[..i].iter().any(|l| l.id == id) {
                let max = self.layers.iter().map(|l| l.id).max().unwrap_or(0);
                let new = NEXT.fetch_add(1, Ordering::Relaxed).max(max + 1);
                NEXT.fetch_max(new + 1, Ordering::Relaxed);
                self.layers[i].id = new;
                changed = true;
            }
        }
        changed
    }

    /// Index of the layer with this id.
    pub fn layer_index(&self, id: u32) -> Option<usize> {
        (id != 0).then(|| self.layers.iter().position(|l| l.id == id)).flatten()
    }

    /// Colors outside the output space are mapped instead of clipped (process version 2).
    pub fn gamut_mapping(&self) -> bool {
        self.process >= 2
    }

    /// Hue-preserving tone curve and median-based highlights/shadows (process version 3).
    pub fn process_3(&self) -> bool {
        self.process >= 3
    }

    /// Partly clipped highlights reconstructed (process version 4, see `highlights`).
    pub fn process_4(&self) -> bool {
        self.process >= 4
    }

    /// Copies the image-specific settings (rotation, crop, layers, retouching) from `other`.
    /// Used when pasting adjustments or applying styles, which should not change framing or masks.
    pub fn keep_geometry_from(&mut self, other: &Adjustments) {
        // The process version belongs to the image, a pasted look is rendered with it.
        self.process = other.process;
        self.rotation = other.rotation;
        self.geometry = other.geometry.clone();
        // Masks and retouching are tied to image content.
        self.layers = other.layers.clone();
        self.retouch = other.retouch.clone();
        self.removals = other.removals.clone();
        // The camera's curve was fitted to this image (another one gets its own when opened).
        self.camera_curve = other.camera_curve.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luma_range_from_before_0_2_11() {
        let load = |json: &str| serde_json::from_str::<LumaRange>(json).unwrap();
        let close = |a: LumaRange, b: [f32; 4]| {
            let v = [a.falloff_low, a.low, a.high, a.falloff_high];
            assert!(v.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5), "{v:?} != {b:?}");
        };
        // One soft width on both sides.
        close(load(r#"{"low":0.2,"high":0.6,"falloff":0.1,"invert":false}"#), [0.1, 0.2, 0.6, 0.7]);
        // Inverted, open at the bottom: what was above the range.
        let r = load(r#"{"low":0.0,"high":0.4,"falloff":0.1,"invert":true}"#);
        close(r, [0.4, 0.5, 1.0, 1.0]);
        // Inverted, open at the top: what was below it; the same weights as before.
        let r = load(r#"{"low":0.7,"high":1.0,"falloff":0.1,"invert":true}"#);
        close(r, [0.0, 0.0, 0.6, 0.7]);
        let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
        for l in [0.0, 0.5, 0.62, 0.65, 0.68, 0.9] {
            let old = 1.0 - smooth(((l - 0.6) / 0.1f32).clamp(0.0, 1.0));
            assert!((r.weight(l) - old).abs() < 1e-3, "{l}: {} vs {old}", r.weight(l));
        }
        // Stored anew: as it is.
        let r = LumaRange { falloff_low: 0.1, low: 0.3, high: 0.5, falloff_high: 0.9 };
        assert_eq!(load(&serde_json::to_string(&r).unwrap()), r);
        // A hard edge.
        let r = LumaRange { falloff_low: 0.3, low: 0.3, high: 1.0, falloff_high: 1.0 };
        assert_eq!(r.weight(0.29), 0.0);
        assert_eq!(r.weight(0.31), 1.0);
    }

    /// A damaged crop (NaN, too big) becomes a valid one instead of panicking in `clamp`.
    #[test]
    fn damaged_crop_is_clamped() {
        let c = CropRect { x: f32::NAN, y: 0.5, w: f32::NAN, h: 3.0 }.clamped();
        assert_eq!((c.x, c.y, c.w, c.h), (0.0, 0.0, 1.0, 1.0));
    }

    #[test]
    fn levels_of_old_files_load_and_channels_stay_out_until_used() {
        let old: Levels = serde_json::from_str(r#"{"in_black":0.1,"in_white":0.9,"midtone":1.2,"out_black":0.0,"out_white":1.0}"#).unwrap();
        assert_eq!((old.rgb.in_black, old.rgb.midtone), (0.1, 1.2));
        assert!(old.red.is_neutral() && old.blue.is_neutral());
        let json = serde_json::to_string(&old).unwrap();
        assert!(json.starts_with(r#"{"in_black":0.1"#) && !json.contains("red"), "{json}");
        let mut lv = Levels::default();
        lv.red.in_white = 0.5;
        let back: Levels = serde_json::from_str(&serde_json::to_string(&lv).unwrap()).unwrap();
        assert_eq!(back, lv);
        // Only the red channel: 0.25 becomes 0.5 there, the others stay.
        assert!((lv.apply(0, 0.25) - 0.5).abs() < 1e-6);
        assert_eq!(lv.apply(1, 0.25), 0.25);
    }

    #[test]
    fn layer_adjustments_of_old_files_load_and_new_fields_stay_out_until_used() {
        let old: LayerAdjustments = serde_json::from_str(r#"{"exposure":0.5,"dehaze":10.0}"#).unwrap();
        assert_eq!(old, LayerAdjustments { exposure: 0.5, dehaze: 10.0, ..Default::default() });
        let json = serde_json::to_string(&old).unwrap();
        for key in ["whites", "blacks", "vibrance", "color_bands", "color_balance"] {
            assert!(!json.contains(key), "{json}");
        }
        let mut used = old;
        used.color_balance.shadows.amount = 20.0;
        used.color_bands[2].hue = 5.0;
        assert!(!used.is_neutral());
        let back: LayerAdjustments = serde_json::from_str(&serde_json::to_string(&used).unwrap()).unwrap();
        assert_eq!(back, used);
    }

    #[test]
    fn damaged_ai_mask_is_dropped_on_load() {
        let ai = |data: Vec<u8>| AiMask { kind: "ai".into(), width: 4, height: 4, data: data.into(), depth: None };
        let layer = |a: AiMask| Layer { mask: Mask { ai: Some(a), ..Default::default() }, ..Default::default() };
        let adj = Adjustments { layers: vec![layer(ai(vec![0; 10])), layer(ai(vec![0; 16]))], ..Default::default() }.normalized();
        assert!(adj.layers[0].mask.ai.is_none());
        assert!(adj.layers[1].mask.ai.is_some());
    }

    /// Details shares the amount between the methods; beyond the full deconvolution the unsharp
    /// mask adds more.
    #[test]
    fn sharpening_is_shared_between_the_methods() {
        let d = |amount: f32, detail: f32| Detail { sharpen_amount: amount, sharpen_detail: detail, ..Default::default() }.sharpen_split();
        assert_eq!(d(120.0, 0.0), (120.0, 0.0));
        assert_eq!(d(120.0, 100.0), (0.0, 1.0));
        assert_eq!(d(60.0, 100.0), (0.0, 0.5));
        assert_eq!(d(120.0, 50.0), (60.0, 0.5));
        assert_eq!(d(200.0, 100.0), (80.0, 1.0));
        assert_eq!(d(0.0, 100.0), (0.0, 0.0));
    }

    #[test]
    fn process_version_of_old_and_new_settings() {
        // Saved before process versions: the original renderer, unless nothing was edited.
        let old: Adjustments = serde_json::from_str(r#"{"exposure":0.5}"#).unwrap();
        assert_eq!(old.process, PROCESS_LEGACY);
        assert_eq!(old.clone().normalized().process, PROCESS_LEGACY);
        let untouched: Adjustments = serde_json::from_str(r#"{"exposure":0.0}"#).unwrap();
        assert_eq!(untouched.normalized(), Adjustments::default());
        // New settings get the latest one and keep it through a save.
        let new = Adjustments { exposure: 0.5, ..Default::default() };
        assert_eq!(new.process, PROCESS_LATEST);
        let back: Adjustments = serde_json::from_str(&serde_json::to_string(&new).unwrap()).unwrap();
        assert_eq!(back.process, PROCESS_LATEST);
        // A pasted look is rendered with the image's version.
        let mut pasted = new.clone();
        pasted.keep_geometry_from(&old);
        assert_eq!(pasted.process, PROCESS_LEGACY);
    }

    #[test]
    fn lens_blur_focus_is_kept_while_off() {
        let mut a = Adjustments::default();
        assert!(!serde_json::to_string(&a).unwrap().contains("lens_blur"));
        a.lens_blur = LensBlur { amount: 0.0, focus: 0.42, ..Default::default() };
        let back: Adjustments = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(back.lens_blur.focus, 0.42);
    }

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
