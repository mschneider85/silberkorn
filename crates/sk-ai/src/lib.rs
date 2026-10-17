//! Masks computed from image content:
//!
//! - subject selection with a salient-object network (U²-Net, ONNX, run with tract),
//! - sky selection (heuristic: bright, smooth, blue or neutral regions connected to the top),
//! - "magic brush" region growing from a seed point,
//!
//! all refined at edges with a guided filter against the image luminance.

pub mod denoise;
pub mod auto;
pub mod depth;
pub mod faces;
pub mod inpaint;
pub mod models;
pub mod runtime;
pub mod segment;
pub mod tiles;
pub mod upscale;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sk_core::color::{Oklab, srgb_encode};
use sk_core::detail::{Plane, guided_filter};

/// Masks are produced in this resolution (long edge).
pub const MASK_EDGE: usize = 768;

/// An 8-bit mask in normalized source coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct MaskImage {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

/// Input for the mask generators: display-referred RGB (0..1, working primaries, sRGB encoded)
/// of the oriented image at any resolution.
pub struct Guide {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<[f32; 3]>,
}

impl Guide {
    /// Resamples to the mask resolution (area average).
    fn at_mask_resolution(&self) -> Guide {
        let k = MASK_EDGE as f32 / self.width.max(self.height) as f32;
        let (w, h) = (((self.width as f32 * k).round() as usize).max(1), ((self.height as f32 * k).round() as usize).max(1));
        let img = sk_core::image::LinearImage::new(self.width, self.height, self.rgb.clone());
        let r = if k < 1.0 { img.resize_area(w, h) } else { img };
        Guide { width: r.width, height: r.height, rgb: r.data }
    }

    fn luma(&self) -> Plane {
        Plane::new(self.width, self.height, self.rgb.iter().map(|p| 0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]).collect())
    }
}

fn to_mask(p: &Plane) -> MaskImage {
    MaskImage { width: p.w, height: p.h, data: p.data.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8).collect() }
}

/// Joint (guided) refinement of a soft mask with the image luminance as guide.
pub fn refine_edges(mask: &Plane, guide: &Plane, r: usize, eps: f32) -> Plane {
    // Guided filter with separate guide I and input p.
    let (w, h) = (mask.w, mask.h);
    let blur = |p: &Plane| sk_core::detail::box_blur(p, r);
    let mi = blur(guide);
    let mp = blur(mask);
    let ip = blur(&guide.zip(mask, |a, b| a * b));
    let ii = blur(&guide.map(|v| v * v));
    let mut a = vec![0.0f32; w * h];
    let mut b = vec![0.0f32; w * h];
    for i in 0..w * h {
        let cov = ip.data[i] - mi.data[i] * mp.data[i];
        let var = ii.data[i] - mi.data[i] * mi.data[i];
        a[i] = cov / (var + eps);
        b[i] = mp.data[i] - a[i] * mi.data[i];
    }
    let ma = blur(&Plane::new(w, h, a));
    let mb = blur(&Plane::new(w, h, b));
    Plane::new(w, h, (0..w * h).map(|i| (ma.data[i] * guide.data[i] + mb.data[i]).clamp(0.0, 1.0)).collect())
}

// ------------------------------------------------------------------ subject (U²-Net)

/// Where the subject model is expected (download target).
pub fn default_model_path() -> PathBuf {
    if let Some(p) = std::env::var_os("SILBERKORN_MODEL") {
        return PathBuf::from(p);
    }
    sk_core::catalog::default_catalog_path().with_file_name("models").join("u2netp.onnx")
}

/// Whether the subject model is there (at its full size, unless `SILBERKORN_MODEL` names one).
pub fn model_available() -> bool {
    let path = default_model_path();
    if std::env::var_os("SILBERKORN_MODEL").is_some() {
        return path.is_file();
    }
    std::fs::metadata(path).is_ok_and(|md| md.is_file() && md.len() == models::SUBJECT.bytes)
}

/// Downloads the subject model (U²-Net-p, Apache-2.0, ~4.6 MB).
pub fn download_model(progress: &dyn Fn(f32)) -> Result<PathBuf> {
    let path = default_model_path();
    models::download_to(&models::SUBJECT, &path, progress)?;
    Ok(path)
}

const NET: usize = 320;

pub struct SubjectModel {
    net: runtime::Net,
}

impl SubjectModel {
    pub fn load(path: &Path) -> Result<Self> {
        Ok(Self { net: runtime::Net::load(path, &[runtime::Input::F32([1, 3, NET, NET])])? })
    }

    /// Salient-object probability map at the network resolution (320 x 320).
    fn predict(&self, guide: &Guide) -> Result<Plane> {
        // Resize to 320x320 (bilinear) and normalize like rembg (max-normalized, ImageNet mean/std).
        let max = guide.rgb.iter().flat_map(|p| p.iter()).fold(1e-6f32, |m, v| m.max(*v));
        let mean = [0.485, 0.456, 0.406];
        let std = [0.229, 0.224, 0.225];
        let sample = |x: f32, y: f32| {
            let fx = (x * guide.width as f32 - 0.5).clamp(0.0, (guide.width - 1) as f32);
            let fy = (y * guide.height as f32 - 0.5).clamp(0.0, (guide.height - 1) as f32);
            let (x0, y0) = (fx as usize, fy as usize);
            let (x1, y1) = ((x0 + 1).min(guide.width - 1), (y0 + 1).min(guide.height - 1));
            let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
            let at = |xx: usize, yy: usize| guide.rgb[yy * guide.width + xx];
            std::array::from_fn::<f32, 3, _>(|c| {
                let top = at(x0, y0)[c] * (1.0 - tx) + at(x1, y0)[c] * tx;
                let bot = at(x0, y1)[c] * (1.0 - tx) + at(x1, y1)[c] * tx;
                top * (1.0 - ty) + bot * ty
            })
        };
        let mut input = vec![0.0f32; 3 * NET * NET];
        for y in 0..NET {
            for x in 0..NET {
                let p = sample((x as f32 + 0.5) / NET as f32, (y as f32 + 0.5) / NET as f32);
                for c in 0..3 {
                    input[c * NET * NET + y * NET + x] = (p[c] / max - mean[c]) / std[c];
                }
            }
        }
        let out = self.net.run(&[runtime::Data::F32(&input)])?;
        let o = out.into_iter().next().context("Keine Modellausgabe")?;
        anyhow::ensure!(o.data.len() == NET * NET, "Unerwartete Modellausgabe {:?}", o.shape);
        let data = o.data;
        let (mi, ma) = data.iter().fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
        let range = (ma - mi).max(1e-6);
        Ok(Plane::new(NET, NET, data.into_iter().map(|v| (v - mi) / range).collect()))
    }

    /// Subject mask at mask resolution, edge-refined.
    pub fn subject_mask(&self, guide: &Guide) -> Result<MaskImage> {
        let small = guide.at_mask_resolution();
        let pred = self.predict(guide)?;
        // Upsample the prediction to the mask resolution.
        let (w, h) = (small.width, small.height);
        let up = Plane::new(
            w,
            h,
            (0..w * h)
                .map(|i| {
                    let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                    let fx = (x / w as f32 * NET as f32 - 0.5).clamp(0.0, (NET - 1) as f32);
                    let fy = (y / h as f32 * NET as f32 - 0.5).clamp(0.0, (NET - 1) as f32);
                    let (x0, y0) = (fx as usize, fy as usize);
                    let (x1, y1) = ((x0 + 1).min(NET - 1), (y0 + 1).min(NET - 1));
                    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
                    let at = |xx: usize, yy: usize| pred.data[yy * NET + xx];
                    let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
                    let bot = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
                    top * (1.0 - ty) + bot * ty
                })
                .collect(),
        );
        let refined = refine_edges(&up, &small.luma(), 4, 1e-3);
        Ok(to_mask(&refined))
    }
}

// ------------------------------------------------------------------ sky (heuristic)

/// Sky mask: bright, low-texture pixels that are blue-ish or neutral and connected to the top edge.
pub fn sky_mask(guide: &Guide) -> MaskImage {
    let g = guide.at_mask_resolution();
    let (w, h) = (g.width, g.height);
    let ok = Oklab::default();
    let luma = g.luma();
    // Local texture: deviation from a small blur.
    let blur = sk_core::detail::gaussian_blur(&luma, 2.0);
    let mut score = vec![0.0f32; w * h];
    for (i, sc) in score.iter_mut().enumerate() {
        let p = g.rgb[i].map(sk_core::color::srgb_decode);
        let [l, a, b] = ok.from_linear(p);
        let c = (a * a + b * b).sqrt();
        let hue = b.atan2(a).to_degrees().rem_euclid(360.0);
        let blue = c > 0.02 && (180.0..290.0).contains(&hue);
        let neutral = c < 0.04;
        let texture = (luma.data[i] - blur.data[i]).abs();
        let bright = l > 0.6;
        let y = (i / w) as f32 / h as f32;
        let mut s = 0.0;
        if bright && (blue || neutral) && (texture < 0.02 || (l > 0.72 && texture < 0.04)) {
            s = 1.0 - y * 0.3;
        } else if bright && blue && texture < 0.05 {
            s = 0.6;
        }
        *sc = s;
    }
    // Keep only regions connected to the top edge (flood fill on score > 0.4).
    let mut keep = vec![false; w * h];
    let mut stack: Vec<usize> = (0..w).filter(|&x| score[x] > 0.4).collect();
    while let Some(i) = stack.pop() {
        if keep[i] {
            continue;
        }
        keep[i] = true;
        let (x, y) = (i % w, i / w);
        let mut push = |j: usize| {
            if !keep[j] && score[j] > 0.4 {
                stack.push(j);
            }
        };
        if x > 0 {
            push(i - 1);
        }
        if x + 1 < w {
            push(i + 1);
        }
        if y > 0 {
            push(i - w);
        }
        if y + 1 < h {
            push(i + w);
        }
    }
    let raw = Plane::new(w, h, keep.iter().map(|&k| if k { 1.0 } else { 0.0 }).collect());
    // Fill small holes softly and refine edges against the image, so that the mask
    // follows bright sky between dark branches.
    let soft = guided_filter(&raw, 3, 0.05);
    to_mask(&refine_edges(&soft, &luma, 8, 4e-3))
}

// ------------------------------------------------------------------ magic brush

/// Magic Brush: selects the connected region around the painted `seeds` (normalized points)
/// whose color resembles that under any of them, so a stroke across several tones takes all
/// of them. `tolerance` 0..1.
pub fn magic_mask(guide: &Guide, seeds: &[[f32; 2]], tolerance: f32) -> MaskImage {
    let g = guide.at_mask_resolution();
    let (w, h) = (g.width, g.height);
    let ok = Oklab::default();
    let lab_raw: Vec<[f32; 3]> = g.rgb.iter().map(|p| ok.from_linear(p.map(sk_core::color::srgb_decode))).collect();
    // Smooth first so that texture (grass, moss, foliage) does not stop the growth.
    let ch: Vec<Plane> = (0..3)
        .map(|c| sk_core::detail::gaussian_blur(&Plane::new(w, h, lab_raw.iter().map(|v| v[c]).collect()), 2.0))
        .collect();
    let lab: Vec<[f32; 3]> = (0..w * h).map(|i| [ch[0].data[i], ch[1].data[i], ch[2].data[i]]).collect();
    // Seed pixels, and the seed colors (averaged over 5x5) from up to 48 of them.
    let pixels: Vec<usize> = seeds
        .iter()
        .map(|s| ((s[1] * h as f32) as usize).min(h - 1) * w + ((s[0] * w as f32) as usize).min(w - 1))
        .collect();
    if pixels.is_empty() {
        return to_mask(&Plane::new(w, h, vec![0.0; w * h]));
    }
    let step = pixels.len().div_ceil(48);
    let colors: Vec<[f32; 3]> = pixels
        .iter()
        .step_by(step)
        .map(|&i| {
            let (sx, sy) = (i % w, i / w);
            let mut sc = [0.0f32; 3];
            let mut n = 0.0;
            for y in sy.saturating_sub(2)..(sy + 3).min(h) {
                for x in sx.saturating_sub(2)..(sx + 3).min(w) {
                    let v = lab[y * w + x];
                    sc = [sc[0] + v[0], sc[1] + v[1], sc[2] + v[2]];
                    n += 1.0;
                }
            }
            sc.map(|v| v / n)
        })
        .collect();
    let tol = 0.015 + tolerance.clamp(0.0, 1.0) * 0.12;
    let dist = |v: [f32; 3]| {
        colors
            .iter()
            .map(|sc| ((v[0] - sc[0]).powi(2) + (v[1] - sc[1]).powi(2) * 2.0 + (v[2] - sc[2]).powi(2) * 2.0).sqrt())
            .fold(f32::MAX, f32::min)
    };
    let mut val = vec![0.0f32; w * h];
    let mut seen = vec![false; w * h];
    let mut is_seed = vec![false; w * h];
    for &i in &pixels {
        is_seed[i] = true;
    }
    let mut stack = pixels.clone();
    while let Some(i) = stack.pop() {
        if seen[i] {
            continue;
        }
        seen[i] = true;
        let d = if is_seed[i] { 0.0 } else { dist(lab[i]) };
        if d > tol * 1.5 {
            continue;
        }
        val[i] = if d <= tol { 1.0 } else { 1.0 - (d - tol) / (tol * 0.5) };
        let (x, y) = (i % w, i / w);
        if x > 0 {
            stack.push(i - 1);
        }
        if x + 1 < w {
            stack.push(i + 1);
        }
        if y > 0 {
            stack.push(i - w);
        }
        if y + 1 < h {
            stack.push(i + w);
        }
    }
    let raw = Plane::new(w, h, val);
    to_mask(&refine_edges(&raw, &g.luma(), 2, 1e-3))
}

/// Builds the guide image from a rendered display image (8-bit sRGB RGBA).
pub fn guide_from_rgba(width: usize, height: usize, rgba: &[u8]) -> Guide {
    Guide { width, height, rgb: rgba.as_chunks::<4>().0.iter().map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]).collect() }
}

/// Builds the guide from linear working RGB.
pub fn guide_from_linear(img: &sk_core::image::LinearImage) -> Guide {
    Guide { width: img.width, height: img.height, rgb: img.data.iter().map(|p| p.map(|v| srgb_encode(v.clamp(0.0, 1.0)))).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guide(w: usize, h: usize, f: impl Fn(usize, usize) -> [f32; 3]) -> Guide {
        Guide { width: w, height: h, rgb: (0..w * h).map(|i| f(i % w, i / w)).collect() }
    }

    fn at(m: &MaskImage, u: f32, v: f32) -> u8 {
        m.data[((v * m.height as f32) as usize).min(m.height - 1) * m.width + ((u * m.width as f32) as usize).min(m.width - 1)]
    }

    #[test]
    fn sky_mask_finds_blue_top() {
        // Blue sky above, dark textured ground below.
        let g = guide(300, 200, |x, y| {
            if y < 80 { [0.55, 0.7, 0.95] } else { [0.2 + ((x * 7 + y * 13) % 10) as f32 * 0.02, 0.25, 0.15] }
        });
        let m = sky_mask(&g);
        assert!(at(&m, 0.5, 0.15) > 200, "sky");
        assert!(at(&m, 0.5, 0.8) < 30, "ground");
    }

    #[test]
    fn magic_stroke_across_two_colors_selects_both() {
        // Three vertical bands: red | green | blue. A stroke over red and green takes both.
        let g = guide(240, 80, |x, _| match x {
            0..80 => [0.8, 0.1, 0.1],
            80..160 => [0.1, 0.7, 0.1],
            _ => [0.1, 0.1, 0.8],
        });
        let m = magic_mask(&g, &[[0.15, 0.5], [0.5, 0.5]], 0.3);
        assert!(at(&m, 0.1, 0.5) > 200 && at(&m, 0.5, 0.5) > 200, "rot und grün gewählt");
        assert!(at(&m, 0.9, 0.5) < 50, "blau nicht");
    }

    #[test]
    fn magic_mask_selects_connected_region() {
        // Red disc on grey, plus a separate red square that must not be selected.
        let g = guide(200, 200, |x, y| {
            let d = ((x as f32 - 60.0).powi(2) + (y as f32 - 100.0).powi(2)).sqrt();
            if d < 30.0 || (x > 150 && x < 180 && y > 20 && y < 50) { [0.8, 0.15, 0.1] } else { [0.5, 0.5, 0.5] }
        });
        let m = magic_mask(&g, &[[0.3, 0.5]], 0.3);
        assert!(at(&m, 0.3, 0.5) > 200);
        assert!(at(&m, 0.8, 0.5) < 20);
        assert!(at(&m, 0.82, 0.17) < 20, "unconnected region");
    }

    #[test]
    fn subject_model_if_available() {
        let Some(path) = std::env::var_os("SK_MODEL_TEST") else { return };
        let model = SubjectModel::load(Path::new(&path)).unwrap();
        // A bright disc on a dark background is the salient object.
        let g = guide(400, 300, |x, y| {
            let d = ((x as f32 - 200.0).powi(2) + (y as f32 - 150.0).powi(2)).sqrt();
            if d < 70.0 { [0.9, 0.6, 0.3] } else { [0.1, 0.12, 0.1] }
        });
        let t = std::time::Instant::now();
        let m = model.subject_mask(&g).unwrap();
        eprintln!("subject mask in {:?}", t.elapsed());
        assert!(at(&m, 0.5, 0.5) > 150, "center {}", at(&m, 0.5, 0.5));
        assert!(at(&m, 0.05, 0.05) < 60, "corner {}", at(&m, 0.05, 0.05));
    }
}
