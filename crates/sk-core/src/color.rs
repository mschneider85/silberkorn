//! Color science helpers: matrices, white balance via temperature/tint.

pub type Mat3 = [[f32; 3]; 3];

/// Linear sRGB (D65) to CIE XYZ.
pub const SRGB_TO_XYZ: Mat3 = [
    [0.412_456_4, 0.357_576_1, 0.180_437_5],
    [0.212_672_9, 0.715_152_2, 0.072_175_0],
    [0.019_333_9, 0.119_192, 0.950_304_1],
];

/// Linear Rec.2020 (D65) to CIE XYZ. Rec.2020 is the internal working space.
pub const REC2020_TO_XYZ: Mat3 = [
    [0.636_958, 0.144_617, 0.168_881],
    [0.262_700, 0.677_998, 0.059_302],
    [0.000_000, 0.028_073, 1.060_985],
];

pub const ADOBE_RGB_TO_XYZ: Mat3 = [
    [0.576_669, 0.185_558, 0.188_229],
    [0.297_345, 0.627_364, 0.075_291],
    [0.027_031, 0.070_689, 0.991_338],
];

pub const DISPLAY_P3_TO_XYZ: Mat3 = [
    [0.486_571, 0.265_668, 0.198_217],
    [0.228_975, 0.691_739, 0.079_287],
    [0.000_000, 0.045_113, 1.043_944],
];

/// ProPhoto RGB is defined relative to D50.
pub const PROPHOTO_TO_XYZ_D50: Mat3 = [
    [0.797_675, 0.135_192, 0.031_353],
    [0.288_040, 0.711_874, 0.000_086],
    [0.000_000, 0.000_000, 0.825_210],
];

/// Bradford chromatic adaptation D65 -> D50.
pub const BRADFORD_D65_TO_D50: Mat3 = [
    [1.047_811_2, 0.022_886_6, -0.050_127_0],
    [0.029_542_4, 0.990_484_4, -0.017_049_1],
    [-0.009_234_5, 0.015_043_6, 0.752_131_6],
];

/// Transfer function of an RGB space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Trc {
    Srgb,
    Gamma(f32),
}

impl Trc {
    #[inline]
    pub fn encode(self, v: f32) -> f32 {
        match self {
            Trc::Srgb => srgb_encode(v),
            Trc::Gamma(g) => v.max(0.0).powf(1.0 / g),
        }
    }

    #[inline]
    pub fn decode(self, v: f32) -> f32 {
        match self {
            Trc::Srgb => srgb_decode(v),
            Trc::Gamma(g) => v.max(0.0).powf(g),
        }
    }
}

/// Output color spaces for export and display.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum OutputSpace {
    #[default]
    Srgb,
    AdobeRgb,
    DisplayP3,
    ProPhoto,
}

impl OutputSpace {
    pub const ALL: [OutputSpace; 4] = [OutputSpace::Srgb, OutputSpace::AdobeRgb, OutputSpace::DisplayP3, OutputSpace::ProPhoto];

    pub fn label(self) -> &'static str {
        match self {
            OutputSpace::Srgb => "sRGB",
            OutputSpace::AdobeRgb => "Adobe RGB (1998)",
            OutputSpace::DisplayP3 => "Display P3",
            OutputSpace::ProPhoto => "ProPhoto RGB",
        }
    }

    pub fn trc(self) -> Trc {
        match self {
            OutputSpace::Srgb | OutputSpace::DisplayP3 => Trc::Srgb,
            OutputSpace::AdobeRgb => Trc::Gamma(563.0 / 256.0),
            OutputSpace::ProPhoto => Trc::Gamma(1.8),
        }
    }

    /// RGB -> XYZ relative to D50 (as needed for ICC profiles).
    pub fn to_xyz_d50(self) -> Mat3 {
        match self {
            OutputSpace::Srgb => mat_mul(&BRADFORD_D65_TO_D50, &SRGB_TO_XYZ),
            OutputSpace::AdobeRgb => mat_mul(&BRADFORD_D65_TO_D50, &ADOBE_RGB_TO_XYZ),
            OutputSpace::DisplayP3 => mat_mul(&BRADFORD_D65_TO_D50, &DISPLAY_P3_TO_XYZ),
            OutputSpace::ProPhoto => PROPHOTO_TO_XYZ_D50,
        }
    }

    /// Linear Rec.2020 working space -> linear output space.
    pub fn from_working(self) -> Mat3 {
        let working_d50 = mat_mul(&BRADFORD_D65_TO_D50, &REC2020_TO_XYZ);
        let inv = mat_inv(&self.to_xyz_d50()).expect("invertible");
        mat_mul(&inv, &working_d50)
    }
}

pub fn mat_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

/// Brings a linear color with luminance `y` (0..1) into the 0..1 cube of its color space by
/// desaturating it towards the gray of the same luminance, just as far as needed. Unlike clipping
/// each channel, this keeps the hue (a saturated blue stays blue instead of turning purple).
#[inline]
pub fn gamut_map(c: [f32; 3], y: f32) -> [f32; 3] {
    if c.iter().all(|v| (0.0..=1.0).contains(v)) {
        return c;
    }
    let y = y.clamp(0.0, 1.0);
    let mut t = 1.0f32;
    for v in c {
        if v > 1.0 {
            t = t.min((1.0 - y) / (v - y).max(1e-9));
        } else if v < 0.0 {
            t = t.min(y / (y - v).max(1e-9));
        }
    }
    let t = t.clamp(0.0, 1.0);
    c.map(|v| (y + t * (v - y)).clamp(0.0, 1.0))
}

#[inline]
pub fn mat_vec(m: &Mat3, v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

pub fn mat_inv(m: &Mat3) -> Option<Mat3> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let d = 1.0 / det;
    Some([
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * d,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * d,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * d,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * d,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * d,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * d,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * d,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * d,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * d,
        ],
    ])
}

/// Camera calibration derived from the camera's XYZ->camera matrix (usually daylight).
#[derive(Clone, Debug)]
pub struct CameraColor {
    pub xyz_to_cam: Mat3,
    /// Light the camera calibrated `xyz_to_cam` for, as an EXIF LightSource code (21 = D65,
    /// 20 = D55, 23 = D50, 1 = daylight, 17 = A).
    pub illuminant: u16,
    /// The camera's matrix for tungsten light (standard illuminant A, 2856 K), if known; renderings
    /// under warm light interpolate towards it (see [`CameraColor::cam_to_working_at`]).
    pub xyz_to_cam_a: Option<Mat3>,
    /// Maps white-balanced camera RGB to linear sRGB.
    pub cam_to_srgb: Mat3,
    /// Maps white-balanced camera RGB to the linear Rec.2020 working space.
    pub cam_to_working: Mat3,
}

/// Inverse of the row-normalized `xyz_to_cam * rgb_to_xyz` (dcraw approach): a
/// white-balanced neutral (1,1,1) in camera space maps to RGB white.
fn cam_to_rgb(xyz_to_cam: &Mat3, rgb_to_xyz: &Mat3) -> Mat3 {
    let mut cam_rgb = mat_mul(xyz_to_cam, rgb_to_xyz);
    for row in cam_rgb.iter_mut() {
        let s: f32 = row.iter().sum();
        if s.abs() > 1e-9 {
            row.iter_mut().for_each(|v| *v /= s);
        }
    }
    mat_inv(&cam_rgb).unwrap_or(IDENTITY)
}

pub const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

impl CameraColor {
    pub fn new(xyz_to_cam: Mat3) -> Self {
        Self {
            xyz_to_cam,
            illuminant: 21,
            xyz_to_cam_a: None,
            cam_to_srgb: cam_to_rgb(&xyz_to_cam, &SRGB_TO_XYZ),
            cam_to_working: cam_to_rgb(&xyz_to_cam, &REC2020_TO_XYZ),
        }
    }

    /// Camera RGB (white balanced) to working RGB for light of `kelvin`: between the daylight
    /// and the tungsten matrix, weighted by inverse color temperature as in the DNG
    /// specification. Without a tungsten matrix, or in daylight, the daylight one.
    pub fn cam_to_working_at(&self, kelvin: f32) -> Mat3 {
        const A: f32 = 2856.0;
        let first = self.illuminant_kelvin();
        let Some(a) = self.xyz_to_cam_a else { return self.cam_to_working };
        if !kelvin.is_finite() || kelvin <= 0.0 || first <= A + 1.0 {
            return self.cam_to_working;
        }
        let w = ((1.0 / kelvin - 1.0 / first) / (1.0 / A - 1.0 / first)).clamp(0.0, 1.0);
        if w <= 0.0 {
            return self.cam_to_working;
        }
        let mut m = self.xyz_to_cam;
        for (i, row) in m.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = *v * (1.0 - w) + a[i][j] * w;
            }
        }
        cam_to_rgb(&m, &REC2020_TO_XYZ)
    }

    /// Color temperature of [`CameraColor::illuminant`] (D65 for unknown codes).
    pub fn illuminant_kelvin(&self) -> f32 {
        match self.illuminant {
            17 => 2856.0,
            3 | 24 => 3200.0,
            20 => 5503.0,
            23 => 5003.0,
            1 | 4 | 9 => 5500.0,
            22 => 7504.0,
            _ => 6504.0,
        }
    }

    /// Per-channel camera multipliers that neutralize the given illuminant (green = 1).
    pub fn wb_for_xy(&self, x: f32, y: f32) -> [f32; 3] {
        let xyz = [x / y, 1.0, (1.0 - x - y) / y];
        let cam = mat_vec(&self.xyz_to_cam, xyz);
        let g = cam[1].max(1e-9);
        [g / cam[0].max(1e-9), 1.0, g / cam[2].max(1e-9)]
    }

    pub fn wb_for_temp_tint(&self, kelvin: f32, tint: f32) -> [f32; 3] {
        let (x, y) = temp_tint_to_xy(kelvin, tint);
        self.wb_for_xy(x, y)
    }

    /// Estimates temperature/tint that reproduce the given camera multipliers.
    pub fn temp_tint_for_wb(&self, wb: [f32; 3]) -> (f32, f32) {
        let target = [(wb[0] / wb[1]).ln(), (wb[2] / wb[1]).ln()];
        let err = |k: f32, t: f32| {
            let m = self.wb_for_temp_tint(k, t);
            let a = (m[0] / m[1]).ln() - target[0];
            let b = (m[2] / m[1]).ln() - target[1];
            a * a + b * b
        };
        // Coarse grid in mired space, then local refinement.
        let mut best = (5000.0, 0.0, f32::MAX);
        let mut mired = 1e6 / 15000.0;
        while mired <= 1e6 / 2000.0 {
            let k = 1e6 / mired;
            let mut t = -150.0;
            while t <= 150.0 {
                let e = err(k, t);
                if e < best.2 {
                    best = (k, t, e);
                }
                t += 5.0;
            }
            mired += 2.0;
        }
        let (mut k, mut t) = (best.0, best.1);
        let (mut dk, mut dt) = (1e6 / k * 0.02, 2.5);
        for _ in 0..40 {
            let mut improved = false;
            for (nk, nt) in [(k * (1.0 + dk / 1e3), t), (k / (1.0 + dk / 1e3), t), (k, t + dt), (k, t - dt)] {
                if err(nk, nt) < err(k, t) {
                    k = nk;
                    t = nt;
                    improved = true;
                }
            }
            if !improved {
                dk *= 0.5;
                dt *= 0.5;
            }
        }
        (k.clamp(2000.0, 15000.0), t.clamp(-150.0, 150.0))
    }
}

/// Planckian locus chromaticity (Kim et al. approximation), valid 1667K..25000K.
pub fn planck_xy(kelvin: f32) -> (f32, f32) {
    let t = kelvin.clamp(1667.0, 25000.0) as f64;
    let (t2, t3) = (t * t, t * t * t);
    let x = if t <= 4000.0 {
        -0.266_123_9e9 / t3 - 0.234_358_9e6 / t2 + 0.877_695_6e3 / t + 0.179_910
    } else {
        -3.025_846_9e9 / t3 + 2.107_037_9e6 / t2 + 0.222_634_7e3 / t + 0.240_390
    };
    let (x2, x3) = (x * x, x * x * x);
    let y = if t <= 2222.0 {
        -1.106_381_4 * x3 - 1.348_110_20 * x2 + 2.185_558_32 * x - 0.202_196_83
    } else if t <= 4000.0 {
        -0.954_947_6 * x3 - 1.374_185_93 * x2 + 2.091_370_15 * x - 0.167_488_67
    } else {
        3.081_758_0 * x3 - 5.873_386_70 * x2 + 3.751_129_97 * x - 0.370_014_83
    };
    (x as f32, y as f32)
}

fn xy_to_uv(x: f32, y: f32) -> (f32, f32) {
    let d = -2.0 * x + 12.0 * y + 3.0;
    (4.0 * x / d, 6.0 * y / d)
}

fn uv_to_xy(u: f32, v: f32) -> (f32, f32) {
    let d = 2.0 * u - 8.0 * v + 4.0;
    (3.0 * u / d, 2.0 * v / d)
}

/// Delta-uv per tint unit; positive tint means a greener illuminant (image turns magenta).
const TINT_SCALE: f32 = 0.0003;

pub fn temp_tint_to_xy(kelvin: f32, tint: f32) -> (f32, f32) {
    let (x, y) = planck_xy(kelvin);
    let (u, v) = xy_to_uv(x, y);
    let (x2, y2) = planck_xy(1e6 / (1e6 / kelvin + 1.0));
    let (u2, v2) = xy_to_uv(x2, y2);
    let (tu, tv) = (u2 - u, v2 - v);
    let len = (tu * tu + tv * tv).sqrt().max(1e-12);
    // Perpendicular, oriented towards +v (green).
    let (mut nu, mut nv) = (-tv / len, tu / len);
    if nv < 0.0 {
        nu = -nu;
        nv = -nv;
    }
    uv_to_xy(u + nu * tint * TINT_SCALE, v + nv * tint * TINT_SCALE)
}

#[inline]
pub fn luminance(rgb: [f32; 3]) -> f32 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

#[inline]
pub fn srgb_encode(v: f32) -> f32 {
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

#[inline]
pub fn srgb_decode(v: f32) -> f32 {
    if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

// ------------------------------------------------------------------ Oklab

/// Linear Rec.2020 -> Oklab LMS (the Oklab M1 matrix composed with Rec.2020 -> sRGB).
#[allow(clippy::excessive_precision)] // published Oklab constants
pub fn working_to_lms() -> Mat3 {
    const M1: Mat3 = [
        [0.412_221_470_8, 0.536_332_536_3, 0.051_445_992_9],
        [0.211_903_498_2, 0.680_699_545_1, 0.107_396_956_6],
        [0.088_302_461_9, 0.281_718_837_6, 0.629_978_700_5],
    ];
    let srgb_from_working = mat_mul(&mat_inv(&SRGB_TO_XYZ).expect("invertible"), &REC2020_TO_XYZ);
    mat_mul(&M1, &srgb_from_working)
}

#[derive(Clone, Debug)]
pub struct Oklab {
    to_lms: Mat3,
    from_lms: Mat3,
}

impl Default for Oklab {
    fn default() -> Self {
        let to_lms = working_to_lms();
        Self { from_lms: mat_inv(&to_lms).expect("invertible"), to_lms }
    }
}

#[allow(clippy::excessive_precision)] // published Oklab constants
impl Oklab {
    /// Linear working RGB -> (L, a, b).
    #[inline]
    pub fn from_linear(&self, rgb: [f32; 3]) -> [f32; 3] {
        let lms = mat_vec(&self.to_lms, rgb).map(fast_cbrt);
        [
            0.210_454_255_3 * lms[0] + 0.793_617_785 * lms[1] - 0.004_072_046_8 * lms[2],
            1.977_998_495_1 * lms[0] - 2.428_592_205 * lms[1] + 0.450_593_709_9 * lms[2],
            0.025_904_037_1 * lms[0] + 0.782_771_766_2 * lms[1] - 0.808_675_766 * lms[2],
        ]
    }

    #[inline]
    pub fn to_linear(&self, lab: [f32; 3]) -> [f32; 3] {
        let [l, a, b] = lab;
        let l_ = l + 0.396_337_777_4 * a + 0.215_803_757_3 * b;
        let m_ = l - 0.105_561_345_8 * a - 0.063_854_172_8 * b;
        let s_ = l - 0.089_484_177_5 * a - 1.291_485_548 * b;
        mat_vec(&self.from_lms, [l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_])
    }
}

/// 2^x, computed as e^(x ln 2): the static C runtime of the Windows build (`crt-static`) has a slow
/// `exp2f` (about 40 ns against 6 ns for `expf`); glibc and the system's ucrtbase.dll are fast at
/// both. For every pixel.
#[inline]
pub fn exp2(x: f32) -> f32 {
    (x * std::f32::consts::LN_2).exp()
}

/// log2(x), computed as ln(x) / ln 2 for the same reason as [`exp2`] (`log2f` about 23 ns there).
#[inline]
pub fn log2(x: f32) -> f32 {
    x.ln() * std::f32::consts::LOG2_E
}

/// Cube root to full `f32` precision, several times faster than `f32::cbrt` (libm):
/// a bit-level estimate refined by two Halley steps (cubic convergence).
#[inline]
pub fn fast_cbrt(x: f32) -> f32 {
    let a = x.abs();
    if a == 0.0 || !a.is_finite() {
        return x;
    }
    // Dividing the exponent by three gives an estimate within a few percent.
    let mut y = f32::from_bits(a.to_bits() / 3 + 0x2a51_4067);
    for _ in 0..2 {
        let y3 = y * y * y;
        y *= (y3 + 2.0 * a) / (2.0 * y3 + a);
    }
    y.copysign(x)
}

#[cfg(test)]
mod tests {

    #[test]
    fn color_matrix_follows_the_light() {
        use super::*;
        let d65 = [[0.7, -0.1, -0.05], [-0.4, 1.2, 0.2], [-0.05, 0.15, 0.6]];
        let a = [[0.8, -0.2, -0.05], [-0.35, 1.15, 0.2], [-0.1, 0.25, 0.45]];
        let mut c = CameraColor::new(d65);
        // Without a tungsten matrix: always the daylight one.
        assert_eq!(c.cam_to_working_at(3000.0), c.cam_to_working);
        c.xyz_to_cam_a = Some(a);
        assert_eq!(c.cam_to_working_at(6504.0), c.cam_to_working);
        assert_eq!(c.cam_to_working_at(9000.0), c.cam_to_working);
        let tungsten = CameraColor::new(a).cam_to_working;
        let at_a = c.cam_to_working_at(2856.0);
        assert!(at_a.iter().flatten().zip(tungsten.iter().flatten()).all(|(x, y)| (x - y).abs() < 1e-5));
        // In between, and neutral stays neutral.
        let mid = c.cam_to_working_at(4000.0);
        assert!(mid != at_a && mid != c.cam_to_working);
        let white = mat_vec(&mid, [1.0, 1.0, 1.0]);
        assert!(white.iter().all(|v| (v - 1.0).abs() < 1e-4), "{white:?}");
        // A D50 matrix holds down to 5003 K, not 6504 K.
        c.illuminant = 23;
        assert_eq!(c.cam_to_working_at(5100.0), c.cam_to_working);
        assert!(c.cam_to_working_at(4900.0) != c.cam_to_working);
        // Only a tungsten matrix: no interpolation.
        let mut only_a = CameraColor::new(a);
        only_a.illuminant = 17;
        only_a.xyz_to_cam_a = Some(a);
        assert_eq!(only_a.cam_to_working_at(2000.0), only_a.cam_to_working);
    }

    #[test]
    fn gamut_map_keeps_luminance_and_hue() {
        use super::gamut_map;
        let luma = [0.2126, 0.7152, 0.0722];
        let y_of = |c: [f32; 3]| c[0] * luma[0] + c[1] * luma[1] + c[2] * luma[2];
        // In gamut: unchanged.
        assert_eq!(gamut_map([0.2, 0.5, 0.9], 0.4), [0.2, 0.5, 0.9]);
        for c in [[-0.3f32, 0.2, 1.4], [1.6, 0.3, -0.1], [0.1, 1.2, 0.05]] {
            let y = y_of(c);
            let m = gamut_map(c, y);
            assert!(m.iter().all(|v| (0.0..=1.0).contains(v)), "{m:?}");
            assert!((y_of(m) - y).abs() < 1e-4, "{c:?} -> {m:?}");
            // Same direction away from gray: the differences keep their ratios.
            let (d, e) = (c.map(|v| v - y), m.map(|v| v - y));
            let k = e[0] / d[0];
            assert!((e[1] - d[1] * k).abs() < 1e-4 && (e[2] - d[2] * k).abs() < 1e-4, "{c:?} -> {m:?}");
        }
    }

    #[test]
    fn fast_cbrt_is_accurate() {
        for i in -2000..=2000 {
            let x = (i as f32 / 100.0).exp2() * if i % 2 == 0 { 1.0 } else { -1.0 };
            let (fast, exact) = (fast_cbrt(x), x.cbrt());
            assert!((fast - exact).abs() <= exact.abs() * 2e-7, "{x}: {fast} vs {exact}");
        }
        assert_eq!(fast_cbrt(0.0), 0.0);
        assert!((fast_cbrt(27.0) - 3.0).abs() < 1e-6);
    }

    use super::*;

    // Sony ILCE-7M3 D65 matrix (Adobe DNG converter values).
    const A7M3: Mat3 = [[0.7374, -0.2389, -0.0551], [-0.5435, 1.3162, 0.2519], [-0.1006, 0.1795, 0.6552]];

    #[test]
    fn inverse_roundtrip() {
        let inv = mat_inv(&SRGB_TO_XYZ).unwrap();
        let id = mat_mul(&inv, &SRGB_TO_XYZ);
        for (i, row) in id.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                assert!((v - if i == j { 1.0 } else { 0.0 }).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn temp_tint_roundtrip() {
        let cc = CameraColor::new(A7M3);
        for (k, t) in [(3200.0, 0.0), (5500.0, 10.0), (7500.0, -20.0)] {
            let wb = cc.wb_for_temp_tint(k, t);
            let (k2, t2) = cc.temp_tint_for_wb(wb);
            assert!((k2 - k).abs() / k < 0.01, "{k} -> {k2}");
            assert!((t2 - t).abs() < 1.0, "{t} -> {t2}");
        }
    }

    #[test]
    fn tint_and_temperature_directions() {
        let cc = CameraColor::new(A7M3);
        // Greener assumed illuminant -> less green gain relative to red/blue.
        let a = cc.wb_for_temp_tint(5500.0, 0.0);
        let b = cc.wb_for_temp_tint(5500.0, 50.0);
        assert!(b[0] > a[0] && b[2] > a[2]);
        // Warmer assumed illuminant (lower K) -> more blue gain.
        let warm = cc.wb_for_temp_tint(3000.0, 0.0);
        assert!(warm[2] > a[2] && warm[0] < a[0]);
    }

    #[test]
    fn oklab_roundtrip_and_neutral_axis() {
        let ok = Oklab::default();
        let grey = ok.from_linear([0.18, 0.18, 0.18]);
        assert!(grey[1].abs() < 1e-3 && grey[2].abs() < 1e-3, "{grey:?}");
        let c = [0.3, 0.12, 0.05];
        let back = ok.to_linear(ok.from_linear(c));
        for i in 0..3 {
            assert!((back[i] - c[i]).abs() < 1e-4);
        }
    }

    #[test]
    fn output_space_white_stays_white() {
        for space in OutputSpace::ALL {
            let w = mat_vec(&space.from_working(), [1.0, 1.0, 1.0]);
            for c in w {
                assert!((c - 1.0).abs() < 2e-3, "{space:?}: {w:?}");
            }
        }
        // sRGB red is inside Rec.2020: converting it back must give pure red.
        let to_working = mat_inv(&OutputSpace::Srgb.from_working()).unwrap();
        let red = mat_vec(&OutputSpace::Srgb.from_working(), mat_vec(&to_working, [1.0, 0.0, 0.0]));
        assert!((red[0] - 1.0).abs() < 1e-3 && red[1].abs() < 1e-3);
    }

    #[test]
    fn neutral_maps_to_white() {
        let cc = CameraColor::new(A7M3);
        let w = mat_vec(&cc.cam_to_srgb, [1.0, 1.0, 1.0]);
        for c in w {
            assert!((c - 1.0).abs() < 1e-4);
        }
    }
}
