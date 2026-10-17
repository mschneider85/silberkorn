//! Panorama stitching: overlapping images taken by turning the camera become one linear DNG.
//!
//! 1. Corners (Harris) with normalized patch descriptors are matched between all pairs;
//!    RANSAC finds the homography of each overlapping pair.
//! 2. The camera model is a pure rotation with a shared focal length: the focal length comes
//!    from the homographies, the rotations from a spanning tree of the best pairs, then a
//!    bundle adjustment (Levenberg-Marquardt) refines everything over all matches.
//! 3. Wave correction makes the horizon straight; the images are projected (spherical,
//!    cylindrical or perspective), their brightness matched, and blended with feathered
//!    weights in linear light. Optionally the result is cropped to the largest rectangle
//!    without empty borders.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::detail::{Plane, gaussian_blur};
use crate::image::LinearImage;
use crate::raw::{self, RawMeta};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Projection {
    #[default]
    Spherical,
    Cylindrical,
    Perspective,
}

impl Projection {
    pub const ALL: [Projection; 3] = [Projection::Spherical, Projection::Cylindrical, Projection::Perspective];

    pub fn label(self) -> &'static str {
        match self {
            Projection::Spherical => "Sphärisch",
            Projection::Cylindrical => "Zylindrisch",
            Projection::Perspective => "Perspektivisch",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanoramaOptions {
    pub projection: Projection,
    pub auto_crop: bool,
    /// Apply each image's lens correction (from its sidecar) before stitching.
    pub lens_correction: bool,
}

impl Default for PanoramaOptions {
    fn default() -> Self {
        Self { projection: Projection::Spherical, auto_crop: true, lens_correction: true }
    }
}

// ------------------------------------------------------------------ small linear algebra

pub(crate) type M3 = [[f64; 3]; 3];

fn mul(a: &M3, b: &M3) -> M3 {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

fn mulv(a: &M3, v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| a[i][0] * v[0] + a[i][1] * v[1] + a[i][2] * v[2])
}

fn transpose(a: &M3) -> M3 {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = a[j][i];
        }
    }
    r
}

fn inv(m: &M3) -> Option<M3> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-14 {
        return None;
    }
    let d = 1.0 / det;
    Some([
        [(m[1][1] * m[2][2] - m[1][2] * m[2][1]) * d, (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * d, (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * d],
        [(m[1][2] * m[2][0] - m[1][0] * m[2][2]) * d, (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * d, (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * d],
        [(m[1][0] * m[2][1] - m[1][1] * m[2][0]) * d, (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * d, (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * d],
    ])
}

const IDENTITY: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Nearest rotation (polar decomposition by iteration).
fn orthonormalize(m: &M3) -> M3 {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let s = det.abs().cbrt().max(1e-12) * det.signum();
    let mut r = m.map(|row| row.map(|v| v / s));
    for _ in 0..30 {
        let Some(it) = inv(&r).map(|i| transpose(&i)) else { break };
        for i in 0..3 {
            for j in 0..3 {
                r[i][j] = 0.5 * (r[i][j] + it[i][j]);
            }
        }
    }
    r
}

/// Rotation matrix of a rotation vector (Rodrigues).
fn rodrigues(w: [f64; 3]) -> M3 {
    let t = (w[0] * w[0] + w[1] * w[1] + w[2] * w[2]).sqrt();
    if t < 1e-12 {
        return IDENTITY;
    }
    let k = w.map(|v| v / t);
    let (s, c) = t.sin_cos();
    let kx = [[0.0, -k[2], k[1]], [k[2], 0.0, -k[0]], [-k[1], k[0], 0.0]];
    let kk = mul(&kx, &kx);
    let mut r = IDENTITY;
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] += s * kx[i][j] + (1.0 - c) * kk[i][j];
        }
    }
    r
}

/// Rotation vector of a rotation matrix.
fn log_rotation(r: &M3) -> [f64; 3] {
    let c = ((r[0][0] + r[1][1] + r[2][2] - 1.0) / 2.0).clamp(-1.0, 1.0);
    let t = c.acos();
    if t < 1e-9 {
        return [0.0; 3];
    }
    let s = 2.0 * t.sin();
    [(r[2][1] - r[1][2]) / s * t, (r[0][2] - r[2][0]) / s * t, (r[1][0] - r[0][1]) / s * t]
}

/// Solves a small dense system (Gaussian elimination with partial pivoting).
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-15 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..n {
            let f = a[row][col] / a[col][col];
            if f != 0.0 {
                let (top, bottom) = a.split_at_mut(row);
                for (x, p) in bottom[0][col..n].iter_mut().zip(&top[col][col..n]) {
                    *x -= f * p;
                }
                b[row] -= f * b[col];
            }
        }
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let s: f64 = (i + 1..n).map(|k| a[i][k] * x[k]).sum();
        x[i] = (b[i] - s) / a[i][i];
    }
    Some(x)
}

/// Eigenvector of the smallest eigenvalue of a symmetric 3x3 matrix (Jacobi rotations).
fn smallest_eigenvector(m: &M3) -> [f64; 3] {
    let mut a = *m;
    let mut v = IDENTITY;
    for _ in 0..50 {
        let (mut p, mut q) = (0, 1);
        for (i, j) in [(0, 1), (0, 2), (1, 2)] {
            if a[i][j].abs() > a[p][q].abs() {
                (p, q) = (i, j);
            }
        }
        if a[p][q].abs() < 1e-14 {
            break;
        }
        let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
        let t = theta.signum().max(0.0) * 2.0 - 1.0;
        let t = if theta == 0.0 { 1.0 } else { t / (theta.abs() + (theta * theta + 1.0).sqrt()) };
        let c = 1.0 / (t * t + 1.0).sqrt();
        let s = t * c;
        let mut j = IDENTITY;
        j[p][p] = c;
        j[q][q] = c;
        j[p][q] = s;
        j[q][p] = -s;
        a = mul(&transpose(&j), &mul(&a, &j));
        v = mul(&v, &j);
    }
    let k = (0..3).min_by(|&i, &j| a[i][i].total_cmp(&a[j][j])).unwrap_or(0);
    [v[0][k], v[1][k], v[2][k]]
}

// ------------------------------------------------------------------ features

/// Long edge of the images features are found in.
pub(crate) const FEATURE_EDGE: usize = 1600;
const DESC: usize = 8;
const DESC_STEP: f32 = 4.0;

pub(crate) struct Features {
    /// Positions relative to the image center (feature image pixels).
    pub(crate) pts: Vec<[f32; 2]>,
    desc: Vec<[f32; DESC * DESC]>,
}

fn bilinear(p: &Plane, x: f32, y: f32) -> f32 {
    let x = x.clamp(0.0, (p.w - 1) as f32);
    let y = y.clamp(0.0, (p.h - 1) as f32);
    let (x0, y0) = (x as usize, y as usize);
    let (x1, y1) = ((x0 + 1).min(p.w - 1), (y0 + 1).min(p.h - 1));
    let (tx, ty) = (x - x0 as f32, y - y0 as f32);
    let d = &p.data;
    let top = d[y0 * p.w + x0] * (1.0 - tx) + d[y0 * p.w + x1] * tx;
    let bot = d[y1 * p.w + x0] * (1.0 - tx) + d[y1 * p.w + x1] * tx;
    top * (1.0 - ty) + bot * ty
}

/// Gamma-encoded luminance of camera RGB (a robust guide; the exact color doesn't matter).
pub(crate) fn guide(img: &LinearImage) -> Plane {
    let mean = img.data.iter().step_by(17).map(|p| p[1]).sum::<f32>() / (img.data.len() / 17).max(1) as f32;
    let scale = 0.18 / mean.max(1e-6);
    Plane::new(img.width, img.height, img.data.par_iter().map(|p| ((p[0] + 2.0 * p[1] + p[2]) * 0.25 * scale).clamp(0.0, 1.0).powf(1.0 / 2.2)).collect())
}

pub(crate) fn detect(gray: &Plane) -> Features {
    let (w, h) = (gray.w, gray.h);
    let g = gaussian_blur(gray, 1.0);
    let mut ixx = vec![0.0f32; w * h];
    let mut iyy = vec![0.0f32; w * h];
    let mut ixy = vec![0.0f32; w * h];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            let gx = g.data[i + 1] - g.data[i - 1];
            let gy = g.data[i + w] - g.data[i - w];
            ixx[i] = gx * gx;
            iyy[i] = gy * gy;
            ixy[i] = gx * gy;
        }
    }
    let (sxx, syy, sxy) = (gaussian_blur(&Plane::new(w, h, ixx), 1.5), gaussian_blur(&Plane::new(w, h, iyy), 1.5), gaussian_blur(&Plane::new(w, h, ixy), 1.5));
    let resp: Vec<f32> = (0..w * h).map(|i| {
        let (a, b, c) = (sxx.data[i], syy.data[i], sxy.data[i]);
        a * b - c * c - 0.04 * (a + b) * (a + b)
    }).collect();
    let max = resp.iter().copied().fold(0.0f32, f32::max);
    // The strongest corner per cell (evenly spread), if it is a local maximum.
    let cell = 24;
    let margin = (DESC as f32 * DESC_STEP) as usize / 2 + 3;
    let mut pts = Vec::new();
    for cy in (margin..h.saturating_sub(margin)).step_by(cell) {
        for cx in (margin..w.saturating_sub(margin)).step_by(cell) {
            let mut best = (0.0f32, 0, 0);
            for y in cy..(cy + cell).min(h - margin) {
                for x in cx..(cx + cell).min(w - margin) {
                    let r = resp[y * w + x];
                    if r > best.0 {
                        best = (r, x, y);
                    }
                }
            }
            let (r, x, y) = best;
            if r <= max * 1e-4 {
                continue;
            }
            let local_max = (-1i32..=1).all(|dy| (-1i32..=1).all(|dx| resp[((y as i32 + dy) as usize) * w + (x as i32 + dx) as usize] <= r));
            if local_max {
                pts.push([x as f32, y as f32]);
            }
        }
    }
    let blurred = gaussian_blur(gray, 1.8);
    let half = (DESC as f32 - 1.0) / 2.0;
    let desc: Vec<[f32; DESC * DESC]> = pts
        .iter()
        .map(|&[x, y]| {
            let mut d = [0.0f32; DESC * DESC];
            for j in 0..DESC {
                for i in 0..DESC {
                    d[j * DESC + i] = bilinear(&blurred, x + (i as f32 - half) * DESC_STEP, y + (j as f32 - half) * DESC_STEP);
                }
            }
            let mean = d.iter().sum::<f32>() / d.len() as f32;
            let var = d.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / d.len() as f32;
            let s = 1.0 / var.sqrt().max(1e-4);
            d.map(|v| (v - mean) * s)
        })
        .collect();
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    Features { pts: pts.iter().map(|p| [p[0] - cx, p[1] - cy]).collect(), desc }
}

fn dist2(a: &[f32; DESC * DESC], b: &[f32; DESC * DESC]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()
}

/// Mutual best matches passing the ratio test: (index in a, index in b).
pub(crate) fn match_features(a: &Features, b: &Features) -> Vec<(usize, usize)> {
    let best = |from: &Features, to: &Features| -> Vec<Option<usize>> {
        from.desc
            .par_iter()
            .map(|d| {
                let (mut b1, mut b2, mut bi) = (f32::MAX, f32::MAX, 0);
                for (j, e) in to.desc.iter().enumerate() {
                    let dd = dist2(d, e);
                    if dd < b1 {
                        b2 = b1;
                        b1 = dd;
                        bi = j;
                    } else if dd < b2 {
                        b2 = dd;
                    }
                }
                (b1 < 0.6 * b2).then_some(bi)
            })
            .collect()
    };
    let ab = best(a, b);
    let ba = best(b, a);
    ab.iter().enumerate().filter_map(|(i, j)| j.filter(|&j| ba[j] == Some(i)).map(|j| (i, j))).collect()
}

// ------------------------------------------------------------------ homographies

/// Homography mapping `src` to `dst` (least squares, h33 = 1, normalized coordinates).
fn fit_homography(src: &[[f64; 2]], dst: &[[f64; 2]]) -> Option<M3> {
    let norm = |pts: &[[f64; 2]]| {
        let n = pts.len() as f64;
        let (mx, my) = (pts.iter().map(|p| p[0]).sum::<f64>() / n, pts.iter().map(|p| p[1]).sum::<f64>() / n);
        let d = pts.iter().map(|p| ((p[0] - mx).powi(2) + (p[1] - my).powi(2)).sqrt()).sum::<f64>() / n;
        let s = if d > 1e-9 { std::f64::consts::SQRT_2 / d } else { 1.0 };
        [[s, 0.0, -s * mx], [0.0, s, -s * my], [0.0, 0.0, 1.0]]
    };
    let (ts, td) = (norm(src), norm(dst));
    let mut ata = vec![vec![0.0; 8]; 8];
    let mut atb = vec![0.0; 8];
    for (s, d) in src.iter().zip(dst) {
        let s = mulv(&ts, [s[0], s[1], 1.0]);
        let d = mulv(&td, [d[0], d[1], 1.0]);
        let (x, y, u, v) = (s[0], s[1], d[0], d[1]);
        for (row, rhs) in [([x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y], u), ([0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y], v)] {
            for i in 0..8 {
                for j in 0..8 {
                    ata[i][j] += row[i] * row[j];
                }
                atb[i] += row[i] * rhs;
            }
        }
    }
    let h = solve(ata, atb)?;
    let hn = [[h[0], h[1], h[2]], [h[3], h[4], h[5]], [h[6], h[7], 1.0]];
    let hm = mul(&inv(&td)?, &mul(&hn, &ts));
    let s = hm[2][2];
    (s.abs() > 1e-12).then(|| hm.map(|r| r.map(|v| v / s)))
}

pub(crate) fn project_h(h: &M3, p: [f64; 2]) -> Option<[f64; 2]> {
    let q = mulv(h, [p[0], p[1], 1.0]);
    (q[2].abs() > 1e-12).then(|| [q[0] / q[2], q[1] / q[2]])
}

/// Small deterministic random numbers (RANSAC sampling).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) % n as u64) as usize
    }
}

/// RANSAC: homography and inlier matches.
pub(crate) fn ransac(src: &[[f64; 2]], dst: &[[f64; 2]], threshold: f64) -> Option<(M3, Vec<usize>)> {
    let n = src.len();
    if n < 8 {
        return None;
    }
    let mut rng = Lcg(n as u64 * 7919 + 17);
    let inliers_of = |h: &M3| -> Vec<usize> {
        (0..n).filter(|&i| project_h(h, src[i]).is_some_and(|q| (q[0] - dst[i][0]).powi(2) + (q[1] - dst[i][1]).powi(2) < threshold * threshold)).collect()
    };
    let mut best: Vec<usize> = Vec::new();
    for _ in 0..1500 {
        let idx: [usize; 4] = std::array::from_fn(|_| rng.next(n));
        if (0..4).any(|a| (a + 1..4).any(|b| idx[a] == idx[b])) {
            continue;
        }
        let s: Vec<[f64; 2]> = idx.iter().map(|&i| src[i]).collect();
        let d: Vec<[f64; 2]> = idx.iter().map(|&i| dst[i]).collect();
        let Some(h) = fit_homography(&s, &d) else { continue };
        let inl = inliers_of(&h);
        if inl.len() > best.len() {
            best = inl;
        }
    }
    if best.len() < 8 {
        return None;
    }
    // Refit on all inliers (twice, the set may grow).
    let mut h = IDENTITY;
    for _ in 0..2 {
        let s: Vec<[f64; 2]> = best.iter().map(|&i| src[i]).collect();
        let d: Vec<[f64; 2]> = best.iter().map(|&i| dst[i]).collect();
        h = fit_homography(&s, &d)?;
        best = inliers_of(&h);
    }
    Some((h, best))
}

/// Focal lengths of the two images from a homography between them (coordinates centered),
/// as in Szeliski's "Creating full view panoramic image mosaics".
fn focals_from_homography(h: &M3) -> Option<f64> {
    let h = [h[0][0], h[0][1], h[0][2], h[1][0], h[1][1], h[1][2], h[2][0], h[2][1], h[2][2]];
    let pick = |d1: f64, d2: f64, v1: f64, v2: f64| -> Option<f64> {
        let (v1, v2) = if v1 < v2 { (v2, v1) } else { (v1, v2) };
        if v1 > 0.0 && v2 > 0.0 {
            Some(if d1.abs() > d2.abs() { v1.sqrt() } else { v2.sqrt() })
        } else if v1 > 0.0 {
            Some(v1.sqrt())
        } else {
            None
        }
    };
    let (d1, d2) = (h[6] * h[7], (h[7] - h[6]) * (h[7] + h[6]));
    let f1 = pick(d1, d2, -(h[0] * h[1] + h[3] * h[4]) / d1, (h[0] * h[0] + h[3] * h[3] - h[1] * h[1] - h[4] * h[4]) / d2);
    let (d1, d2) = (h[0] * h[3] + h[1] * h[4], h[0] * h[0] + h[1] * h[1] - h[3] * h[3] - h[4] * h[4]);
    let f0 = pick(d1, d2, -h[2] * h[5] / d1, (h[5] * h[5] - h[2] * h[2]) / d2);
    match (f0, f1) {
        (Some(a), Some(b)) if a.is_finite() && b.is_finite() => Some((a * b).sqrt()),
        _ => None,
    }
}

// ------------------------------------------------------------------ camera estimation

/// Inlier matches between two images: points (centered, feature image pixels) in i and j.
struct PairMatches {
    i: usize,
    j: usize,
    h: M3,
    pts: Vec<([f64; 2], [f64; 2])>,
}

/// Camera rotations (world -> camera) and the shared focal length (feature image pixels).
struct Cameras {
    rot: Vec<M3>,
    focal: f64,
}

/// Bundle adjustment of rotation vectors (and the focal length, if `refine_focal`) on the
/// reprojection error of all matches, both ways, in pixels.
fn bundle_adjust(cams: &mut Cameras, pairs: &[PairMatches], fixed: usize, refine_focal: bool) {
    let n = cams.rot.len();
    let mut params: Vec<f64> = cams.rot.iter().flat_map(log_rotation).collect();
    params.push(cams.focal);
    let f0 = cams.focal;
    let residuals = |p: &[f64]| -> Vec<f64> {
        let f = p[3 * n].clamp(f0 * 0.2, f0 * 5.0);
        let rots: Vec<M3> = (0..n).map(|i| rodrigues([p[3 * i], p[3 * i + 1], p[3 * i + 2]])).collect();
        let reproject = |from: usize, to: usize, a: [f64; 2], b: [f64; 2], r: &mut Vec<f64>| {
            let q = mulv(&rots[to], mulv(&transpose(&rots[from]), [a[0], a[1], f]));
            if q[2] > 1e-6 {
                r.push(q[0] / q[2] * f - b[0]);
                r.push(q[1] / q[2] * f - b[1]);
            } else {
                r.extend([1e4, 1e4]);
            }
        };
        let mut r = Vec::new();
        for pm in pairs {
            for (a, b) in &pm.pts {
                reproject(pm.i, pm.j, *a, *b, &mut r);
                reproject(pm.j, pm.i, *b, *a, &mut r);
            }
        }
        r
    };
    let cost = |r: &[f64]| r.iter().map(|v| v * v).sum::<f64>();
    let mut r = residuals(&params);
    let mut c = cost(&r);
    let mut lambda = 1e-3;
    let free: Vec<usize> = (0..3 * n + 1).filter(|&k| if k == 3 * n { refine_focal } else { k / 3 != fixed }).collect();
    for _ in 0..60 {
        // Numeric Jacobian.
        let cols: Vec<Vec<f64>> = free
            .par_iter()
            .map(|&k| {
                let mut p = params.clone();
                let eps = if k == 3 * n { 1e-3 * params[k].abs().max(1.0) } else { 1e-6 };
                p[k] += eps;
                residuals(&p).iter().zip(&r).map(|(a, b)| (a - b) / eps).collect()
            })
            .collect();
        let m = free.len();
        let mut jtj = vec![vec![0.0; m]; m];
        let mut jtr = vec![0.0; m];
        for a in 0..m {
            for b in a..m {
                let v: f64 = cols[a].iter().zip(&cols[b]).map(|(x, y)| x * y).sum();
                jtj[a][b] = v;
                jtj[b][a] = v;
            }
            jtr[a] = -cols[a].iter().zip(&r).map(|(x, y)| x * y).sum::<f64>();
        }
        let mut improved = false;
        for _ in 0..8 {
            let mut aug = jtj.clone();
            for (k, row) in aug.iter_mut().enumerate() {
                row[k] = row[k] * (1.0 + lambda) + 1e-12;
            }
            let Some(delta) = solve(aug, jtr.clone()) else { break };
            let mut p = params.clone();
            for (k, d) in free.iter().zip(&delta) {
                p[*k] += d;
            }
            let r2 = residuals(&p);
            let c2 = cost(&r2);
            if c2 < c {
                let rel = (c - c2) / c.max(1e-12);
                params = p;
                r = r2;
                c = c2;
                lambda = (lambda * 0.3).max(1e-9);
                improved = rel > 1e-7;
                break;
            }
            lambda *= 10.0;
        }
        if !improved {
            break;
        }
    }
    cams.focal = params[3 * n].clamp(f0 * 0.2, f0 * 5.0);
    for i in 0..n {
        cams.rot[i] = rodrigues([params[3 * i], params[3 * i + 1], params[3 * i + 2]]);
    }
}

/// Initial rotations along a maximum spanning tree (by inlier count) from the best connected image.
fn initial_rotations(n: usize, pairs: &[PairMatches], focal: f64) -> Result<(Vec<M3>, usize)> {
    let weight = |i: usize| pairs.iter().filter(|p| p.i == i || p.j == i).map(|p| p.pts.len()).sum::<usize>();
    let center = (0..n).max_by_key(|&i| weight(i)).unwrap_or(0);
    let k = [[focal, 0.0, 0.0], [0.0, focal, 0.0], [0.0, 0.0, 1.0]];
    let kinv = inv(&k).context("Brennweite")?;
    let mut rot: Vec<Option<M3>> = vec![None; n];
    rot[center] = Some(IDENTITY);
    loop {
        // The strongest edge from the tree to an image not yet in it.
        let next = pairs
            .iter()
            .filter(|p| rot[p.i].is_some() != rot[p.j].is_some())
            .max_by_key(|p| p.pts.len());
        let Some(p) = next else { break };
        // x_j ~ H x_i  =>  R_j R_i^T = K^-1 H K
        let rel = orthonormalize(&mul(&kinv, &mul(&p.h, &k)));
        if let Some(ri) = rot[p.i] {
            rot[p.j] = Some(orthonormalize(&mul(&rel, &ri)));
        } else {
            let rj = rot[p.j].expect("in tree");
            rot[p.i] = Some(orthonormalize(&mul(&transpose(&rel), &rj)));
        }
    }
    let missing = rot.iter().filter(|r| r.is_none()).count();
    if missing > 0 {
        bail!("{missing} Bild(er) überlappen mit keinem anderen ausreichend");
    }
    Ok((rot.into_iter().map(|r| r.expect("all")).collect(), center))
}

/// Rotates the world so that the cameras' horizontal axes lie in the horizontal plane
/// (straight horizon), and the panorama is centered.
fn wave_correct(rot: &mut [M3]) {
    // Up vector: perpendicular to all camera x axes.
    let mut cov = [[0.0; 3]; 3];
    for r in rot.iter() {
        let x = r[0];
        for i in 0..3 {
            for j in 0..3 {
                cov[i][j] += x[i] * x[j];
            }
        }
    }
    let mut up = smallest_eigenvector(&cov);
    // With all horizontal axes (nearly) alike – a vertical panorama, or two frames side by side –
    // two eigenvalues are about zero and "up" would be a random mix of the cameras' up and
    // viewing directions. Then the cameras' mean up direction is taken (made perpendicular to
    // their horizontal axis): the middle frame's horizon stays level.
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let normalized = |a: [f64; 3]| {
        let n = dot(a, a).sqrt();
        (n > 1e-9).then(|| a.map(|v| v / n))
    };
    let sum = |k: usize| rot.iter().fold([0.0; 3], |s, r| [s[0] + r[k][0], s[1] + r[k][1], s[2] + r[k][2]]);
    if let Some(mx) = normalized(sum(0)) {
        let other = [mx[1] * up[2] - mx[2] * up[1], mx[2] * up[0] - mx[0] * up[2], mx[0] * up[1] - mx[1] * up[0]];
        let cov_other = [dot(cov[0], other), dot(cov[1], other), dot(cov[2], other)];
        // Mean squared sine of the horizontal axes' spread around their mean: below ~10°.
        if dot(other, cov_other) / rot.len() as f64 <= 0.03 {
            let y = sum(1);
            let d = dot(y, mx);
            if let Some(y) = normalized([y[0] - d * mx[0], y[1] - d * mx[1], y[2] - d * mx[2]]) {
                up = y;
            }
        }
    }
    // Pointing like the cameras' y axes (down in image coordinates).
    let ysum: f64 = rot.iter().map(|r| r[1][0] * up[0] + r[1][1] * up[1] + r[1][2] * up[2]).sum();
    if ysum < 0.0 {
        up = up.map(|v| -v);
    }
    // Mean viewing direction, made perpendicular to up.
    let mut z = [0.0; 3];
    for r in rot.iter() {
        for k in 0..3 {
            z[k] += r[2][k];
        }
    }
    let d = z[0] * up[0] + z[1] * up[1] + z[2] * up[2];
    let mut z = [z[0] - d * up[0], z[1] - d * up[1], z[2] - d * up[2]];
    let zn = (z[0] * z[0] + z[1] * z[1] + z[2] * z[2]).sqrt().max(1e-12);
    z = z.map(|v| v / zn);
    let x = [up[1] * z[2] - up[2] * z[1], up[2] * z[0] - up[0] * z[2], up[0] * z[1] - up[1] * z[0]];
    // World basis (x right, y down, z forward) in old world coordinates.
    let basis = [x, up, z];
    for r in rot.iter_mut() {
        *r = mul(r, &transpose(&basis));
    }
}

/// Brightness gains of the images from their matched points (log least squares with a prior of 1).
fn exposure_gains(n: usize, pairs: &[PairMatches], guides: &[Plane]) -> Vec<f32> {
    let mut a = vec![vec![0.0; n]; n];
    let mut b = vec![0.0; n];
    for (k, row) in a.iter_mut().enumerate() {
        row[k] += 1.0;
    }
    for p in pairs {
        let (gi, gj) = (&guides[p.i], &guides[p.j]);
        let mut ratios: Vec<f64> = p
            .pts
            .iter()
            .filter_map(|(x, y)| {
                let vi = bilinear(gi, x[0] as f32 + gi.w as f32 / 2.0, x[1] as f32 + gi.h as f32 / 2.0) as f64;
                let vj = bilinear(gj, y[0] as f32 + gj.w as f32 / 2.0, y[1] as f32 + gj.h as f32 / 2.0) as f64;
                (vi > 0.05 && vj > 0.05 && vi < 0.95 && vj < 0.95).then(|| (vi / vj).ln() * 2.2)
            })
            .collect();
        if ratios.len() < 10 {
            continue;
        }
        let mid = ratios.len() / 2;
        let r = *ratios.select_nth_unstable_by(mid, |x, y| x.total_cmp(y)).1;
        // log g_i + log I_i = log g_j + log I_j  =>  log g_i - log g_j = -r
        let w = 10.0;
        a[p.i][p.i] += w;
        a[p.j][p.j] += w;
        a[p.i][p.j] -= w;
        a[p.j][p.i] -= w;
        b[p.i] -= w * r;
        b[p.j] += w * r;
    }
    solve(a, b).map(|x| x.iter().map(|v| v.exp() as f32).collect()).unwrap_or_else(|| vec![1.0; n])
}

// ------------------------------------------------------------------ projection and blending

fn direction(proj: Projection, u: f64, v: f64) -> [f64; 3] {
    match proj {
        Projection::Spherical => {
            let (st, ct) = u.sin_cos();
            let (sp, cp) = v.sin_cos();
            [st * cp, sp, ct * cp]
        }
        Projection::Cylindrical => {
            let (st, ct) = u.sin_cos();
            [st, v, ct]
        }
        Projection::Perspective => [u, v, 1.0],
    }
}

fn unproject(proj: Projection, d: [f64; 3]) -> Option<[f64; 2]> {
    match proj {
        Projection::Spherical => {
            let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            Some([d[0].atan2(d[2]), (d[1] / n).asin()])
        }
        Projection::Cylindrical => {
            let r = (d[0] * d[0] + d[2] * d[2]).sqrt();
            (r > 1e-9).then(|| [d[0].atan2(d[2]), d[1] / r])
        }
        Projection::Perspective => (d[2] > 1e-3).then(|| [d[0] / d[2], d[1] / d[2]]),
    }
}

/// Largest axis-aligned rectangle of `true` cells: (x, y, w, h).
fn largest_rectangle(mask: &[bool], w: usize, h: usize) -> (usize, usize, usize, usize) {
    let mut heights = vec![0usize; w];
    let mut best = (0, 0, 0, 0, 0usize);
    for y in 0..h {
        for x in 0..w {
            heights[x] = if mask[y * w + x] { heights[x] + 1 } else { 0 };
        }
        let mut stack: Vec<usize> = Vec::new();
        for x in 0..=w {
            let cur = if x < w { heights[x] } else { 0 };
            while let Some(&top) = stack.last() {
                if heights[top] <= cur {
                    break;
                }
                stack.pop();
                let height = heights[top];
                let left = stack.last().map_or(0, |&l| l + 1);
                let width = x - left;
                if width * height > best.4 {
                    best = (left, y + 1 - height, width, height, width * height);
                }
            }
            stack.push(x);
        }
    }
    (best.0, best.1, best.2, best.3)
}

struct Source {
    img: LinearImage,
    /// world -> camera
    rot: M3,
    gain: f32,
}

/// Renders the panorama: output size, data (camera RGB) and coverage.
fn render(sources: &[Source], focal: f64, proj: Projection, auto_crop: bool, max_pixels: usize) -> Result<LinearImage> {
    // Extent: the borders of all images, projected.
    let (mut umin, mut umax, mut vmin, mut vmax) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for s in sources {
        let (w, h) = (s.img.width as f64, s.img.height as f64);
        let rt = transpose(&s.rot);
        for k in 0..=40 {
            let t = k as f64 / 40.0;
            for (x, y) in [(t * w, 0.0), (t * w, h), (0.0, t * h), (w, t * h)] {
                let d = mulv(&rt, [x - w / 2.0, y - h / 2.0, focal]);
                if let Some([u, v]) = unproject(proj, d) {
                    umin = umin.min(u);
                    umax = umax.max(u);
                    vmin = vmin.min(v);
                    vmax = vmax.max(v);
                }
            }
        }
    }
    if !(umin < umax && vmin < vmax) {
        bail!("Keine gültige Projektion");
    }
    // One output pixel per source pixel at the center; less if the result would be huge.
    let mut scale = focal;
    let (mut ow, mut oh) = (((umax - umin) * scale).ceil() as usize, ((vmax - vmin) * scale).ceil() as usize);
    if ow * oh > max_pixels {
        let k = (max_pixels as f64 / (ow * oh) as f64).sqrt();
        scale *= k;
        ow = ((umax - umin) * scale).ceil() as usize;
        oh = ((vmax - vmin) * scale).ceil() as usize;
    }
    if ow == 0 || oh == 0 || ow > 65000 || oh > 65000 {
        bail!("Das Panorama wäre {ow} × {oh} Pixel groß – bitte eine andere Projektion wählen");
    }
    let rots: Vec<M3> = sources.iter().map(|s| s.rot).collect();
    let mut data = vec![[0.0f32; 3]; ow * oh];
    let mut covered = vec![false; ow * oh];
    data.par_chunks_mut(ow).zip(covered.par_chunks_mut(ow)).enumerate().for_each(|(y, (line, cov))| {
        let v = vmin + (y as f64 + 0.5) / scale;
        for (x, (o, c)) in line.iter_mut().zip(cov.iter_mut()).enumerate() {
            let u = umin + (x as f64 + 0.5) / scale;
            let d = direction(proj, u, v);
            let mut acc = [0.0f32; 3];
            let mut wsum = 0.0f32;
            for (s, r) in sources.iter().zip(&rots) {
                let q = mulv(r, d);
                if q[2] <= 1e-6 {
                    continue;
                }
                let (w, h) = (s.img.width as f64, s.img.height as f64);
                let px = q[0] / q[2] * focal + w / 2.0 - 0.5;
                let py = q[1] / q[2] * focal + h / 2.0 - 0.5;
                if px < 0.0 || py < 0.0 || px > w - 1.0 || py > h - 1.0 {
                    continue;
                }
                // Feathering: weight falls towards the image border.
                let ex = (px.min(w - 1.0 - px) / (w * 0.5)) as f32;
                let ey = (py.min(h - 1.0 - py) / (h * 0.5)) as f32;
                let wt = (ex * ey).max(1e-6).powi(2);
                let (x0, y0) = (px as usize, py as usize);
                let (x1, y1) = ((x0 + 1).min(s.img.width - 1), (y0 + 1).min(s.img.height - 1));
                let (tx, ty) = ((px - x0 as f64) as f32, (py - y0 as f64) as f32);
                let iw = s.img.width;
                let at = |xx: usize, yy: usize| s.img.data[yy * iw + xx];
                let (a, b, cc, dd) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
                for k in 0..3 {
                    let top = a[k] * (1.0 - tx) + b[k] * tx;
                    let bot = cc[k] * (1.0 - tx) + dd[k] * tx;
                    acc[k] += (top * (1.0 - ty) + bot * ty) * s.gain * wt;
                }
                wsum += wt;
            }
            if wsum > 0.0 {
                *o = acc.map(|v| v / wsum);
                *c = true;
            }
        }
    });
    let mut img = LinearImage::new(ow, oh, data);
    if auto_crop {
        // On a reduced mask (fast), then shrunk by a pixel of that grid to stay inside.
        let step = (ow.max(oh) / 1500).max(1);
        let (mw, mh) = (ow / step, oh / step);
        let mask: Vec<bool> = (0..mw * mh)
            .map(|i| {
                let (bx, by) = (i % mw * step, i / mw * step);
                (by..(by + step).min(oh)).all(|y| (bx..(bx + step).min(ow)).all(|x| covered[y * ow + x]))
            })
            .collect();
        let (x, y, w, h) = largest_rectangle(&mask, mw, mh);
        if w > 2 && h > 2 {
            let (x0, y0) = ((x + 1) * step, (y + 1) * step);
            let (cw, ch) = ((w - 2) * step, (h - 2) * step);
            let mut out = Vec::with_capacity(cw * ch);
            for yy in y0..y0 + ch {
                out.extend_from_slice(&img.data[yy * ow + x0..yy * ow + x0 + cw]);
            }
            img = LinearImage::new(cw, ch, out);
        }
    }
    Ok(img)
}

// ------------------------------------------------------------------ entry points

/// Estimated cameras of a set of images (feature resolution guides), with their matches.
fn estimate(guides: &[Plane], progress: &dyn Fn(f32)) -> Result<(Cameras, Vec<PairMatches>)> {
    let n = guides.len();
    let feats: Vec<Features> = guides.par_iter().map(detect).collect();
    progress(0.25);
    let pairs_idx: Vec<(usize, usize)> = (0..n).flat_map(|i| (i + 1..n).map(move |j| (i, j))).collect();
    let pairs: Vec<PairMatches> = pairs_idx
        .par_iter()
        .filter_map(|&(i, j)| {
            let m = match_features(&feats[i], &feats[j]);
            let src: Vec<[f64; 2]> = m.iter().map(|&(a, _)| feats[i].pts[a].map(|v| v as f64)).collect();
            let dst: Vec<[f64; 2]> = m.iter().map(|&(_, b)| feats[j].pts[b].map(|v| v as f64)).collect();
            let (h, inl) = ransac(&src, &dst, 2.5)?;
            // Brown & Lowe: enough inliers relative to all matches.
            if (inl.len() as f64) < 12.0 + 0.25 * m.len() as f64 {
                return None;
            }
            Some(PairMatches { i, j, h, pts: inl.iter().map(|&k| (src[k], dst[k])).collect() })
        })
        .collect();
    progress(0.4);
    if pairs.is_empty() {
        bail!("Die Bilder überlappen nicht ausreichend");
    }
    let mut focals: Vec<f64> = pairs.iter().filter_map(|p| focals_from_homography(&p.h)).filter(|f| *f > 50.0 && *f < 1e5).collect();
    let long = guides[0].w.max(guides[0].h) as f64;
    // Without a measurable focal length (images hardly rotated against each other) it stays fixed.
    let refine_focal = !focals.is_empty();
    let focal = if focals.is_empty() {
        long // about 53° horizontal field of view
    } else {
        let mid = focals.len() / 2;
        *focals.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1
    };
    let (rot, center) = initial_rotations(n, &pairs, focal)?;
    let mut cams = Cameras { rot, focal };
    // Fewer matches per pair keep the adjustment fast.
    let sampled: Vec<PairMatches> = pairs
        .iter()
        .map(|p| {
            let step = (p.pts.len() / 80).max(1);
            PairMatches { i: p.i, j: p.j, h: p.h, pts: p.pts.iter().step_by(step).copied().collect() }
        })
        .collect();
    if std::env::var_os("SK_TRACE").is_some() {
        eprintln!("\npanorama: {} Paare, Brennweite {focal:.1} -> ", pairs.len());
        for p in &pairs {
            eprintln!("  {}-{}: {} Inlier, H={:?}", p.i, p.j, p.pts.len(), p.h);
        }
    }
    bundle_adjust(&mut cams, &sampled, center, refine_focal);
    if std::env::var_os("SK_TRACE").is_some() {
        eprintln!("  nach Ausgleich: Brennweite {:.1}, Rotationen {:?}", cams.focal, cams.rot.iter().map(log_rotation).collect::<Vec<_>>());
    }
    progress(0.5);
    Ok((cams, pairs))
}

/// Stitches the images into `<first image>-Pano.dng`. `progress` gets 0..1.
pub fn stitch_files(paths: &[PathBuf], opts: &PanoramaOptions, progress: &dyn Fn(f32)) -> Result<PathBuf> {
    let files = crate::variant::files_of(paths);
    if files.len() < 2 {
        bail!("Für ein Panorama mindestens zwei Bilder auswählen");
    }
    let mut metas: Vec<RawMeta> = Vec::new();
    let mut images: Vec<LinearImage> = Vec::new();
    for (k, path) in files.iter().enumerate() {
        let (meta, mut img) = raw::load_full(path).with_context(|| format!("{}", path.display()))?;
        if opts.lens_correction {
            let lens = crate::sidecar::load(path).adjustments.lens;
            let geo = crate::geometry::GeoMap::new(&Default::default(), &lens, meta.lens.as_ref(), 0, img.width, img.height, true);
            if (geo.out_w, geo.out_h) != (0, 0) {
                img = geo.resample(&img, crate::geometry::Region { x: 0, y: 0, w: geo.out_w, h: geo.out_h });
            }
        }
        if let Some(first) = images.first()
            && (first.width, first.height) != (img.width, img.height)
        {
            bail!("{} hat eine andere Größe oder Ausrichtung als die übrigen Bilder", path.display());
        }
        metas.push(meta);
        images.push(img);
        progress(0.15 * (k + 1) as f32 / files.len() as f32);
    }
    let full_long = images[0].width.max(images[0].height);
    let factor = (full_long as f32 / FEATURE_EDGE as f32).max(1.0);
    let guides: Vec<Plane> = images.par_iter().map(|img| guide(&img.clone().fit_within(FEATURE_EDGE))).collect();
    let (cams, pairs) = estimate(&guides, &|p| progress(0.15 + p * 0.5))?;
    let gains = exposure_gains(images.len(), &pairs, &guides);
    let mut rot = cams.rot;
    wave_correct(&mut rot);
    let gw = guides[0].w as f64;
    let focal_full = cams.focal * images[0].width as f64 / gw;
    let _ = factor;
    let sources: Vec<Source> = images.into_iter().zip(rot).zip(&gains).map(|((img, rot), g)| Source { img, rot, gain: *g }).collect();
    progress(0.7);
    let pano = render(&sources, focal_full, opts.projection, opts.auto_crop, 250_000_000)?;
    drop(sources);
    progress(0.9);
    let reference = &metas[0];
    let out = crate::hdr::output_path(&files[0], "-Pano");
    let preview = crate::dng::quick_preview(reference, &pano, 1024);
    crate::dng::write_linear_dng(
        &out,
        &crate::dng::LinearDng {
            width: pano.width,
            height: pano.height,
            data: &pano.data,
            xyz_to_cam: reference.color.xyz_to_cam,
            as_shot_wb: reference.as_shot_wb,
            info: &reference.info,
            baseline_exposure: 0.0,
            preview: Some(&preview),
            lens_corrected: true,
        },
    )?;
    progress(1.0);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertical_panorama_keeps_the_middle_frame_level() {
        // Three frames tilted upwards (about the shared x axis), with a little jitter.
        let frame = |pitch: f64, jitter: f64| -> M3 {
            let (c, s) = (pitch.to_radians().cos(), pitch.to_radians().sin());
            let x = [1.0, jitter, 0.0];
            let n = (1.0 + jitter * jitter).sqrt();
            [x.map(|v| v / n), [-jitter / n * c, c / n, -s], [-jitter / n * s, s / n, c]]
        };
        let mut rot = vec![frame(0.0, 1e-4), frame(30.0, -2e-4), frame(60.0, 1e-4)];
        wave_correct(&mut rot);
        // The middle frame looks straight ahead: its up axis is the world's.
        assert!(rot[1][1][1] > 0.99, "{:?}", rot[1]);
        assert!(rot[1][0][0] > 0.99, "{:?}", rot[1]);
    }
    use crate::detail::value_noise;

    /// A textured world: brightness as a function of the viewing direction.
    fn world(d: [f64; 3]) -> f32 {
        let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let (lon, lat) = (d[0].atan2(d[2]) as f32, (d[1] / n).asin() as f32);
        let mut v = 0.0;
        let mut amp = 1.0;
        let mut freq = 12.0;
        for o in 0..5 {
            v += value_noise(lon * freq, lat * freq, 7 + o) * amp;
            amp *= 0.55;
            freq *= 2.1;
        }
        (0.3 + 0.18 * v).clamp(0.01, 1.0)
    }

    /// A camera image of the world looking along `yaw`/`pitch`/`roll`.
    fn view(w: usize, h: usize, f: f64, rot: &M3) -> LinearImage {
        let rt = transpose(rot);
        let data = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let (x, y) = ((i % w) as f64 + 0.5 - w as f64 / 2.0, (i / w) as f64 + 0.5 - h as f64 / 2.0);
                let v = world(mulv(&rt, [x, y, f]));
                [v, v, v]
            })
            .collect();
        LinearImage::new(w, h, data)
    }

    #[test]
    fn linear_algebra() {
        let r = rodrigues([0.1, -0.3, 0.2]);
        let back = log_rotation(&r);
        assert!((back[0] - 0.1).abs() < 1e-9 && (back[1] + 0.3).abs() < 1e-9 && (back[2] - 0.2).abs() < 1e-9);
        let noisy = r.map(|row| row.map(|v| v * 1.01 + 0.001));
        let o = orthonormalize(&noisy);
        let i = mul(&o, &transpose(&o));
        assert!((0..3).all(|a| (0..3).all(|b| (i[a][b] - if a == b { 1.0 } else { 0.0 }).abs() < 1e-6)));
        let m = [[2.0, 0.0, 0.0], [0.0, 0.5, 0.0], [0.0, 0.0, 1.0]];
        let e = smallest_eigenvector(&m);
        assert!(e[1].abs() > 0.99);
        let mask = vec![false, true, true, true, true, true, true, false, true, true, true, false];
        assert_eq!(largest_rectangle(&mask, 4, 3), (1, 0, 2, 3));
    }

    #[test]
    fn recovers_rotations_of_three_views() {
        let (w, h, f) = (480usize, 320usize, 420.0);
        let truth: Vec<M3> = [-0.45f64, 0.0, 0.42].iter().map(|yaw| rodrigues([0.02, *yaw, 0.0])).collect();
        let views: Vec<LinearImage> = truth.iter().map(|r| view(w, h, f, r)).collect();
        let guides: Vec<Plane> = views.iter().map(guide).collect();
        let (cams, pairs) = estimate(&guides, &|_| {}).unwrap();
        assert!(pairs.len() >= 2, "{} Paare", pairs.len());
        assert!((cams.focal - f).abs() < f * 0.05, "Brennweite {}", cams.focal);
        // Relative rotations match the truth.
        for (a, b) in [(0usize, 1usize), (1, 2)] {
            let est = mul(&cams.rot[b], &transpose(&cams.rot[a]));
            let tru = mul(&truth[b], &transpose(&truth[a]));
            let diff = log_rotation(&mul(&est, &transpose(&tru)));
            let ang = (diff[0] * diff[0] + diff[1] * diff[1] + diff[2] * diff[2]).sqrt();
            assert!(ang < 0.01, "{a}-{b}: {ang}");
        }
        // Stitched: wider than one view, the world visible without seams.
        let mut rot = cams.rot.clone();
        wave_correct(&mut rot);
        let sources: Vec<Source> = views.into_iter().zip(rot).map(|(img, rot)| Source { img, rot, gain: 1.0 }).collect();
        let pano = render(&sources, cams.focal, Projection::Spherical, true, 10_000_000).unwrap();
        // Field of view: the yaw range plus one view (2·atan(240/420)), times the focal length.
        let expect = (0.87 + 2.0 * (240.0f64 / 420.0).atan()) * f;
        assert!((pano.width as f64 - expect).abs() < expect * 0.05 && pano.height > h * 8 / 10, "{} × {}", pano.width, pano.height);
        assert!(pano.data.iter().all(|p| p[0] > 0.0), "Ränder abgeschnitten");
    }
}
