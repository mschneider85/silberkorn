//! Faces and culling: face detection (YuNet), face embeddings for recognizing people (SFace),
//! closed eyes (open-closed-eye-0001) and image sharpness.


use anyhow::{Context, Result};
use crate::runtime::{Data, Input, Net};

use crate::models;

/// Detector input size (the model has a fixed input).
const DET: usize = 640;
const FACE: usize = 112;
const EYE: usize = 32;

/// A face found in an image (coordinates normalized to the image, 0..1).
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    /// x, y, width, height
    pub bbox: [f32; 4],
    /// Right eye, left eye (as seen in the image: left first), nose, mouth corners.
    pub landmarks: [[f32; 2]; 5],
    pub score: f32,
    /// L2-normalized SFace embedding (128 values).
    pub embedding: Vec<f32>,
    /// Probability that the eyes are closed (the more open eye counts), 0..1.
    pub eyes_closed: f32,
    /// Sharpness of the face (see [`sharpness`]).
    pub sharpness: f32,
}

/// Result of analysing an image.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Analysis {
    pub faces: Vec<Face>,
    /// Sharpness of the image: of the faces if there are any, else of its sharpest parts.
    pub sharpness: f32,
}

/// sRGB image (0..1) for analysis.
pub struct Image<'a> {
    pub width: usize,
    pub height: usize,
    pub rgb: &'a [[f32; 3]],
}

impl Image<'_> {
    fn at(&self, x: f32, y: f32) -> [f32; 3] {
        let x = (x - 0.5).clamp(0.0, (self.width - 1) as f32);
        let y = (y - 0.5).clamp(0.0, (self.height - 1) as f32);
        let (x0, y0) = (x as usize, y as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (tx, ty) = (x - x0 as f32, y - y0 as f32);
        let p = |xx: usize, yy: usize| self.rgb[yy * self.width + xx];
        let (a, b, c, d) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));
        std::array::from_fn(|k| {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bot = c[k] + (d[k] - c[k]) * tx;
            top + (bot - top) * ty
        })
    }

    fn gray(&self, x: f32, y: f32) -> f32 {
        let p = self.at(x, y);
        0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]
    }
}

/// A detected face in pixels: box, landmarks, score.
type Detection = ([f32; 4], [[f32; 2]; 5], f32);

pub struct FaceModels {
    det: Net,
    rec: Net,
    eye: Net,
}

/// The models face analysis needs.
pub const NEEDED: [&models::ModelInfo; 3] = [&models::FACE_DETECT, &models::FACE_EMBED, &models::EYE_STATE];


#[inline]
fn sigmoid_free(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

/// Intersection over union of two boxes (x, y, w, h).
fn iou(a: [f32; 4], b: [f32; 4]) -> f32 {
    let (x0, y0) = (a[0].max(b[0]), a[1].max(b[1]));
    let (x1, y1) = ((a[0] + a[2]).min(b[0] + b[2]), (a[1] + a[3]).min(b[1] + b[3]));
    let inter = (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
    inter / (a[2] * a[3] + b[2] * b[3] - inter).max(1e-6)
}

/// Similarity transform (scale, rotation, translation) mapping `src` onto `dst` (least squares).
fn similarity(src: &[[f32; 2]], dst: &[[f32; 2]]) -> [f32; 4] {
    let n = src.len() as f32;
    let (msx, msy) = (src.iter().map(|p| p[0]).sum::<f32>() / n, src.iter().map(|p| p[1]).sum::<f32>() / n);
    let (mdx, mdy) = (dst.iter().map(|p| p[0]).sum::<f32>() / n, dst.iter().map(|p| p[1]).sum::<f32>() / n);
    let (mut a, mut b, mut norm) = (0.0f32, 0.0f32, 0.0f32);
    for (s, d) in src.iter().zip(dst) {
        let (sx, sy, dx, dy) = (s[0] - msx, s[1] - msy, d[0] - mdx, d[1] - mdy);
        a += sx * dx + sy * dy;
        b += sx * dy - sy * dx;
        norm += sx * sx + sy * sy;
    }
    let (a, b) = (a / norm.max(1e-9), b / norm.max(1e-9));
    // dst = [a -b; b a] src + t
    [a, b, mdx - (a * msx - b * msy), mdy - (b * msx + a * msy)]
}

/// Reference positions of the five landmarks in a 112 x 112 aligned face (ArcFace).
const TEMPLATE: [[f32; 2]; 5] = [[38.2946, 51.6963], [73.5318, 51.5014], [56.0252, 71.7366], [41.5493, 92.3655], [70.7299, 92.2041]];

/// Sharpness of a region: energy of the finest details (1 px) relative to coarser ones (4 px),
/// which doesn't depend on exposure or contrast. Measured at the image's own resolution on at
/// most `n` x `n` pixels in the middle of the region.
pub fn sharpness(img: &Image, rect: [f32; 4], n: usize) -> f32 {
    let [x, y, w, h] = rect;
    let (nx, ny) = ((w as usize).clamp(12, n), (h as usize).clamp(12, n));
    let (x0, y0) = (x + (w - nx as f32) / 2.0, y + (h - ny as f32) / 2.0);
    let g: Vec<f32> = (0..nx * ny).map(|i| img.gray(x0 + (i % nx) as f32 + 0.5, y0 + (i / nx) as f32 + 0.5)).collect();
    let lap = |xx: usize, yy: usize, d: usize| {
        let c = 4.0 * g[yy * nx + xx];
        (c - g[yy * nx + xx - d] - g[yy * nx + xx + d] - g[(yy - d) * nx + xx] - g[(yy + d) * nx + xx]).powi(2)
    };
    let (mut fine, mut coarse) = (0.0f32, 0.0f32);
    for yy in 4..ny - 4 {
        for xx in 4..nx - 4 {
            fine += lap(xx, yy, 1);
            coarse += lap(xx, yy, 4);
        }
    }
    (fine / coarse.max(1e-9)).sqrt()
}

/// Faces with their landmarks (pixel coordinates of `img`), before embedding.
fn detect(det: &Net, img: &Image) -> Result<Vec<Detection>> {
    let k = DET as f32 / img.width.max(img.height) as f32;
    let k = k.min(4.0);
    // BGR 0..255, image in the top left, the rest black.
    let n = DET * DET;
    let mut input = vec![0.0f32; 3 * n];
    let (dw, dh) = ((img.width as f32 * k).round() as usize, (img.height as f32 * k).round() as usize);
    for y in 0..dh.min(DET) {
        for x in 0..dw.min(DET) {
            let p = img.at((x as f32 + 0.5) / k, (y as f32 + 0.5) / k);
            for c in 0..3 {
                input[c * n + y * DET + x] = p[2 - c] * 255.0;
            }
        }
    }
    let out = det.run(&[Data::F32(&input)])?;
    anyhow::ensure!(out.len() >= 12, "Unerwartete Modellausgabe");
    let view = |i: usize| -> Result<&[f32]> { Ok(&out[i].data) };
    let mut faces = Vec::new();
    for (si, stride) in [8usize, 16, 32].into_iter().enumerate() {
        let (cls, obj, bbox, kps) = (view(si)?, view(3 + si)?, view(6 + si)?, view(9 + si)?);
        let cols = DET / stride;
        for i in 0..cls.len() {
            let score = (sigmoid_free(cls[i]) * sigmoid_free(obj[i])).sqrt();
            if score < 0.6 {
                continue;
            }
            let (col, row) = ((i % cols) as f32, (i / cols) as f32);
            let s = stride as f32;
            let (cx, cy) = ((col + bbox[i * 4]) * s, (row + bbox[i * 4 + 1]) * s);
            let (w, h) = (bbox[i * 4 + 2].exp() * s, bbox[i * 4 + 3].exp() * s);
            let mut lm = [[0.0f32; 2]; 5];
            for (j, l) in lm.iter_mut().enumerate() {
                *l = [(col + kps[i * 10 + 2 * j]) * s / k, (row + kps[i * 10 + 2 * j + 1]) * s / k];
            }
            faces.push(([(cx - w / 2.0) / k, (cy - h / 2.0) / k, w / k, h / k], lm, score));
        }
    }
    // Non-maximum suppression.
    faces.sort_by(|a, b| b.2.total_cmp(&a.2));
    let mut kept: Vec<Detection> = Vec::new();
    for f in faces {
        if kept.iter().all(|k| iou(k.0, f.0) < 0.3) {
            kept.push(f);
        }
    }
    Ok(kept)
}

/// Only finding faces (for the auto adjustment), without recognizing persons.
pub struct FaceDetector {
    det: Net,
}

impl FaceDetector {
    pub fn available() -> bool {
        models::available(&models::FACE_DETECT)
    }

    pub fn load() -> Result<Self> {
        Ok(Self { det: Net::load(&models::path(&models::FACE_DETECT), &[Input::F32([1, 3, DET, DET])])? })
    }

    /// Boxes (x, y, width, height normalized to the image) of the faces at least `min_px`
    /// pixels of `img` wide.
    pub fn faces(&self, img: &Image, min_px: f32) -> Result<Vec<[f32; 4]>> {
        let (w, h) = (img.width as f32, img.height as f32);
        Ok(detect(&self.det, img)?
            .into_iter()
            .filter(|d| d.0[2].min(d.0[3]) >= min_px)
            .map(|(b, _, _)| [b[0] / w, b[1] / h, b[2] / w, b[3] / h])
            .collect())
    }
}

impl FaceModels {
    pub fn load() -> Result<Self> {
        Ok(Self {
            det: Net::load(&models::path(&models::FACE_DETECT), &[Input::F32([1, 3, DET, DET])])?,
            rec: Net::load(&models::path(&models::FACE_EMBED), &[Input::F32([1, 3, FACE, FACE])])?,
            eye: Net::load(&models::path(&models::EYE_STATE), &[Input::F32([1, 3, EYE, EYE])])?,
        })
    }

    fn detect(&self, img: &Image) -> Result<Vec<Detection>> {
        detect(&self.det, img)
    }

    fn embed(&self, img: &Image, lm: &[[f32; 2]; 5]) -> Result<Vec<f32>> {
        // Inverse mapping: aligned face pixel -> image pixel.
        let [a, b, tx, ty] = similarity(&TEMPLATE, lm);
        let n = FACE * FACE;
        let mut input = vec![0.0f32; 3 * n];
        for y in 0..FACE {
            for x in 0..FACE {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let p = img.at(a * fx - b * fy + tx, b * fx + a * fy + ty);
                for c in 0..3 {
                    input[c * n + y * FACE + x] = p[c] * 255.0;
                }
            }
        }
        let out = self.rec.run(&[Data::F32(&input)])?;
        let v = &out.first().context("Keine Modellausgabe")?.data;
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
        Ok(v.iter().map(|x| x / norm).collect())
    }

    /// Probability that an eye (center, size in pixels) is closed.
    fn eye_closed(&self, img: &Image, center: [f32; 2], size: f32, angle: (f32, f32)) -> Result<f32> {
        // BGR, (v - 127) / 255 (the model's preprocessing).
        let norm = |p: [f32; 3]| [0, 1, 2].map(|c| (p[2 - c] * 255.0 - 127.0) / 255.0);
        let n = EYE * EYE;
        let mut input = vec![0.0f32; 3 * n];
        let (cos, sin) = angle;
        for y in 0..EYE {
            for x in 0..EYE {
                let (u, v) = (((x as f32 + 0.5) / EYE as f32 - 0.5) * size, ((y as f32 + 0.5) / EYE as f32 - 0.5) * size);
                let p = norm(img.at(center[0] + u * cos - v * sin, center[1] + u * sin + v * cos));
                for c in 0..3 {
                    input[c * n + y * EYE + x] = p[c];
                }
            }
        }
        let out = self.eye.run(&[Data::F32(&input)])?;
        let v = &out.first().context("Keine Modellausgabe")?.data;
        anyhow::ensure!(v.len() >= 2, "Unerwartete Modellausgabe");
        // [closed, open], as the OpenVINO gaze demo reads it (`open = v[0] < v[1]`).
        Ok(v[0] / (v[0] + v[1]).max(1e-9))
    }

    /// Analyses an image (any size; about 1500 px on the long edge is plenty).
    pub fn analyze(&self, img: &Image) -> Result<Analysis> {
        let (w, h) = (img.width as f32, img.height as f32);
        let mut faces = Vec::new();
        for (bbox, lm, score) in self.detect(img)? {
            // Tiny faces say nothing about eyes or identity.
            if bbox[2].min(bbox[3]) < 24.0 {
                continue;
            }
            let embedding = self.embed(img, &lm)?;
            let (dx, dy) = (lm[1][0] - lm[0][0], lm[1][1] - lm[0][1]);
            let iod = (dx * dx + dy * dy).sqrt().max(1.0);
            let angle = (dx / iod, dy / iod);
            // Square crops around the eyes, rotated upright. Profiles (eyes close together
            // relative to the face) can't be judged.
            let size = iod * 0.65;
            let closed = if iod / bbox[2] < 0.3 {
                0.0
            } else {
                // Closed only if both are.
                self.eye_closed(img, lm[0], size, angle)?.min(self.eye_closed(img, lm[1], size, angle)?)
            };
            let sharp = sharpness(img, bbox, 256);
            faces.push(Face {
                bbox: [bbox[0] / w, bbox[1] / h, bbox[2] / w, bbox[3] / h],
                landmarks: lm.map(|p| [p[0] / w, p[1] / h]),
                score,
                embedding,
                eyes_closed: closed,
                sharpness: sharp,
            });
        }
        let sharpness = if faces.is_empty() {
            // The sharpest of a grid of regions (the subject is usually sharp somewhere).
            let mut best = 0.0f32;
            for gy in 0..4 {
                for gx in 0..4 {
                    let r = [gx as f32 * w / 4.0, gy as f32 * h / 4.0, w / 4.0, h / 4.0];
                    best = best.max(sharpness(img, r, 256));
                }
            }
            best
        } else {
            faces.iter().map(|f| f.sharpness).fold(0.0, f32::max)
        };
        Ok(Analysis { faces, sharpness })
    }
}

/// Cosine similarity of two normalized embeddings.
pub fn similarity_of(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Above this similarity two faces are taken to be the same person (SFace, cosine).
pub const SAME_PERSON: f32 = 0.42;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similarity_transform_recovers_scale_and_rotation() {
        let (s, ang, t) = (2.0f32, 0.3f32, [10.0f32, -5.0]);
        let (a, b) = (s * ang.cos(), s * ang.sin());
        let dst: Vec<[f32; 2]> = TEMPLATE.iter().map(|p| [a * p[0] - b * p[1] + t[0], b * p[0] + a * p[1] + t[1]]).collect();
        let m = similarity(&TEMPLATE, &dst);
        assert!((m[0] - a).abs() < 1e-3 && (m[1] - b).abs() < 1e-3 && (m[2] - t[0]).abs() < 1e-2 && (m[3] - t[1]).abs() < 1e-2, "{m:?}");
    }

    #[test]
    fn sharpness_drops_with_blur() {
        let (w, h) = (200usize, 200usize);
        let sharp: Vec<[f32; 3]> = (0..w * h).map(|i| [sk_core::detail::hash_noise((i % w) as i64 / 2, (i / w) as i64 / 2, 3) * 0.3 + 0.5; 3]).collect();
        let plane = sk_core::detail::Plane::new(w, h, sharp.iter().map(|p| p[0]).collect());
        let blurred: Vec<[f32; 3]> = sk_core::detail::gaussian_blur(&plane, 2.0).data.iter().map(|v| [*v; 3]).collect();
        let s = |d: &[[f32; 3]]| sharpness(&Image { width: w, height: h, rgb: d }, [0.0, 0.0, w as f32, h as f32], 200);
        assert!(s(&blurred) < s(&sharp) * 0.6, "{} vs {}", s(&blurred), s(&sharp));
    }

    fn load_jpeg(path: &std::path::Path) -> Option<(usize, usize, Vec<[f32; 3]>)> {
        let img = ::image::open(path).ok()?.to_rgb8();
        let (w, h) = (img.width() as usize, img.height() as usize);
        Some((w, h, img.pixels().map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]).collect()))
    }

    /// With the models and test photos in `SILBERKORN_FACE_TEST` (not part of the repository).
    #[test]
    fn analyses_test_photos_if_available() {
        let Some(dir) = std::env::var_os("SILBERKORN_FACE_TEST").map(std::path::PathBuf::from) else { return };
        if !NEEDED.iter().all(|m| models::available(m)) {
            return;
        }
        let m = FaceModels::load().unwrap();
        let run = |name: &str| {
            let (w, h, rgb) = load_jpeg(&dir.join(name)).expect(name);
            let a = m.analyze(&Image { width: w, height: h, rgb: &rgb }).unwrap();
            eprintln!("{name}: {} Gesichter, Schärfe {:.3}, Augen zu {:?}", a.faces.len(), a.sharpness, a.faces.iter().map(|f| f.eyes_closed).collect::<Vec<_>>());
            a
        };
        // Four profiles (detected, eyes not judged).
        let crew = run("crew.jpg");
        assert_eq!(crew.faces.len(), 4);
        assert!(crew.faces.iter().all(|f| f.eyes_closed < 0.5));
        let portrait = run("obama.jpg");
        assert_eq!(portrait.faces.len(), 1);
        assert!(portrait.faces[0].eyes_closed < 0.5);
        for name in ["leblanc.jpg", "dreis.jpg"] {
            let closed = run(name);
            assert!(closed.faces.iter().any(|f| f.eyes_closed > 0.5), "{name}");
        }
        // Different people are told apart; the same face (mirrored) is recognized.
        for (i, a) in crew.faces.iter().enumerate() {
            for b in &crew.faces[i + 1..] {
                assert!(similarity_of(&a.embedding, &b.embedding) < SAME_PERSON);
            }
        }
        let (w, h, rgb) = load_jpeg(&dir.join("obama.jpg")).unwrap();
        let mirrored: Vec<[f32; 3]> = (0..w * h).map(|i| rgb[(i / w) * w + (w - 1 - i % w)]).collect();
        let again = m.analyze(&Image { width: w, height: h, rgb: &mirrored }).unwrap();
        assert!(similarity_of(&again.faces[0].embedding, &portrait.faces[0].embedding) > SAME_PERSON);
    }
}
