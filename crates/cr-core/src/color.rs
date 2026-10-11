//! Color science helpers: matrices, white balance via temperature/tint.

pub type Mat3 = [[f32; 3]; 3];

/// Linear sRGB (D65) to CIE XYZ.
pub const SRGB_TO_XYZ: Mat3 = [
    [0.412_456_4, 0.357_576_1, 0.180_437_5],
    [0.212_672_9, 0.715_152_2, 0.072_175_0],
    [0.019_333_9, 0.119_192, 0.950_304_1],
];

pub fn mat_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
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

/// Camera calibration derived from the camera's XYZ->camera matrix (D65).
#[derive(Clone, Debug)]
pub struct CameraColor {
    pub xyz_to_cam: Mat3,
    /// Maps white-balanced camera RGB to linear sRGB.
    pub cam_to_srgb: Mat3,
}

impl CameraColor {
    pub fn new(xyz_to_cam: Mat3) -> Self {
        // dcraw approach: normalize rows of (xyz->cam * srgb->xyz) so that a
        // white-balanced neutral (1,1,1) in camera space maps to sRGB white.
        let mut cam_rgb = mat_mul(&xyz_to_cam, &SRGB_TO_XYZ);
        for row in cam_rgb.iter_mut() {
            let s: f32 = row.iter().sum();
            if s.abs() > 1e-9 {
                row.iter_mut().for_each(|v| *v /= s);
            }
        }
        let cam_to_srgb = mat_inv(&cam_rgb).unwrap_or([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        Self { xyz_to_cam, cam_to_srgb }
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

#[cfg(test)]
mod tests {
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
    fn neutral_maps_to_white() {
        let cc = CameraColor::new(A7M3);
        let w = mat_vec(&cc.cam_to_srgb, [1.0, 1.0, 1.0]);
        for c in w {
            assert!((c - 1.0).abs() < 1e-4);
        }
    }
}
