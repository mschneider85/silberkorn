//! Depth maps (see `sk_core::depth`): Depth Anything V2 Small (Apache-2.0) predicts the inverse
//! depth of every part of the image; it is scaled to 0 (far) .. 1 (near) and its edges are
//! moved onto the image's, so masks, relight, atmosphere and the lens blur follow the subject's
//! outline.

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rayon::prelude::*;
use sk_core::depth::DepthMap;
use sk_core::detail::Plane;

use crate::models::{self, DEPTH};
use crate::runtime::{Data, Input, Net};
use crate::{Guide, refine_edges_color};

/// The network sees the short edge at this size (as trained), the long edge in proportion.
const SHORT: usize = 518;
/// Its input sizes are multiples of the patch size.
const PATCH: usize = 14;
/// Long edge of the stored maps.
pub const MAP_EDGE: usize = 1536;

pub fn available() -> bool {
    models::available(&DEPTH)
}

/// Identifies the model's maps.
pub fn tag() -> String {
    format!("dav2s-{}", &DEPTH.sha256[..12])
}

/// Input size (width, height) of the network for an image of `w` x `h`.
fn input_size(w: usize, h: usize) -> (usize, usize) {
    let round = |v: f32| ((v / PATCH as f32).round() as usize).clamp(1, 2 * SHORT / PATCH) * PATCH;
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    if w >= h { (round(SHORT as f32 * w / h), SHORT) } else { (SHORT, round(SHORT as f32 * h / w)) }
}

/// A loaded network: its input size, the runtime configuration it was loaded with, the network.
type Loaded = ((usize, usize), u64, Arc<Net>);

/// Loaded networks kept at once (landscape, portrait, square …).
const KEEP: usize = 3;

/// The network, loaded per input size; the last [`KEEP`] sizes stay loaded.
#[derive(Default)]
pub struct DepthModel {
    /// Most recently used last.
    nets: Mutex<Vec<Loaded>>,
}

impl DepthModel {
    fn net(&self, size: (usize, usize)) -> Result<Arc<Net>> {
        let generation = crate::runtime::config_generation();
        let mut nets = self.nets.lock().unwrap_or_else(|e| e.into_inner());
        nets.retain(|(_, g, _)| *g == generation);
        if let Some(k) = nets.iter().position(|(s, _, _)| *s == size) {
            let hit = nets.remove(k);
            let n = hit.2.clone();
            nets.push(hit);
            return Ok(n);
        }
        anyhow::ensure!(available(), "{} ist nicht geladen", DEPTH.label);
        if nets.len() >= KEEP {
            nets.remove(0);
        }
        let n = Arc::new(Net::load(&models::path(&DEPTH), &[Input::F32([1, 3, size.1, size.0])])?);
        nets.push((size, generation, n.clone()));
        Ok(n)
    }

    /// The depth map of an image (display-referred, any size), at most [`MAP_EDGE`] on the long edge.
    pub fn predict(&self, guide: &Guide) -> Result<DepthMap> {
        let (iw, ih) = input_size(guide.width, guide.height);
        let net = self.net((iw, ih))?;
        // Resize (bilinear, area-averaged first when shrinking a lot) and normalize like ImageNet.
        let src = sk_core::image::LinearImage::new(guide.width, guide.height, guide.rgb.clone());
        let src = if guide.width > 2 * iw { src.resize_area(2 * iw, (2 * iw * guide.height / guide.width).max(1)) } else { src };
        let mean = [0.485, 0.456, 0.406];
        let std = [0.229, 0.224, 0.225];
        let mut input = vec![0.0f32; 3 * iw * ih];
        for y in 0..ih {
            for x in 0..iw {
                let p = bilinear(&src, (x as f32 + 0.5) / iw as f32, (y as f32 + 0.5) / ih as f32);
                for c in 0..3 {
                    input[c * iw * ih + y * iw + x] = (p[c].clamp(0.0, 1.0) - mean[c]) / std[c];
                }
            }
        }
        let out = net.run(&[Data::F32(&input)])?.into_iter().next().context("Keine Modellausgabe")?;
        anyhow::ensure!(out.data.len() == iw * ih, "Unerwartete Modellausgabe {:?}", out.shape);
        // Scale to 0..1 by the nearly farthest and nearly nearest parts (single outliers aside).
        let mut sorted: Vec<f32> = out.data.iter().copied().filter(|v| v.is_finite()).collect();
        anyhow::ensure!(!sorted.is_empty(), "Ungültige Modellausgabe");
        sorted.sort_by(f32::total_cmp);
        let q = |f: f32| sorted[((sorted.len() - 1) as f32 * f) as usize];
        let (lo, hi) = (q(0.005), q(0.995));
        let range = (hi - lo).max(1e-6);
        let pred = Plane::new(iw, ih, out.data.iter().map(|v| ((v - lo) / range).clamp(0.0, 1.0)).collect());

        // Up to the map size, with edges taken from the image.
        let k = (MAP_EDGE as f32 / guide.width.max(guide.height) as f32).min(1.0);
        let (mw, mh) = (((guide.width as f32 * k).round() as usize).max(1), ((guide.height as f32 * k).round() as usize).max(1));
        let small = if k < 1.0 { guide_resized(guide, mw, mh) } else { sk_core::image::LinearImage::new(guide.width, guide.height, guide.rgb.clone()) };
        let up = Plane::new(
            mw,
            mh,
            (0..mw * mh).into_par_iter().map(|i| sample(&pred, ((i % mw) as f32 + 0.5) / mw as f32, ((i / mw) as f32 + 0.5) / mh as f32)).collect(),
        );
        // Map pixels per network pixel: the network's edges are a few of its pixels soft and off.
        let scale = mw.max(mh) as f32 / iw.max(ih) as f32;
        let snapped = snap_edges(&up, &small.data, (scale * 5.0).round().max(3.0) as usize, 0.08);
        // Smooths the snapped steps along the edges again.
        let refined = refine_edges_color(&snapped, &small.data, scale.round().max(2.0) as usize, 1e-3);
        Ok(DepthMap { width: mw, height: mh, data: refined.data })
    }

    /// The depth map of an image file: its preview, developed plainly in source orientation.
    pub fn of_file(&self, path: &Path, progress: &dyn Fn(f32)) -> Result<DepthMap> {
        let (meta, preview) = sk_core::raw::load_preview(path, MAP_EDGE).with_context(|| format!("{}", path.display()))?;
        progress(0.2);
        // No lens correction: the map must match the source like masks do.
        let adj = sk_core::params::Adjustments {
            lens: sk_core::params::LensCorrection { camera: false, ..Default::default() },
            detail: sk_core::params::Detail { sharpen_amount: 0.0, ..Default::default() },
            ..Default::default()
        };
        let r = sk_core::pipeline::Processor::new(&meta, &adj).render(&preview, None, None);
        let guide = crate::guide_from_rgba(r.width, r.height, &r.rgba);
        progress(0.3);
        let map = self.predict(&guide)?;
        progress(1.0);
        Ok(map)
    }
}

/// Moves the depth's edges onto the image's: where the depth changes within `r`, each pixel
/// takes the weighted median of its neighbors' depths, weighted by how alike their colors are.
/// The network's edges are soft and lie a few pixels off; a median makes a hard step where the
/// colors change.
fn snap_edges(depth: &Plane, rgb: &[[f32; 3]], r: usize, sigma: f32) -> Plane {
    let (w, h) = (depth.w, depth.h);
    let ri = r as isize;
    let k = 1.0 / (2.0 * sigma * sigma);
    let data = (0..w * h)
        .into_par_iter()
        .map_init(Vec::new, |near: &mut Vec<(f32, f32)>, i| {
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            let d0 = depth.data[i];
            let at = |xx: isize, yy: isize| (xx.clamp(0, w as isize - 1) + yy.clamp(0, h as isize - 1) * w as isize) as usize;
            // Only near edges: the corners and middles of the window differ.
            let (mut lo, mut hi) = (d0, d0);
            for (dx, dy) in [(-ri, 0), (ri, 0), (0, -ri), (0, ri), (-ri, -ri), (ri, ri), (-ri, ri), (ri, -ri)] {
                let d = depth.data[at(x + dx, y + dy)];
                lo = lo.min(d);
                hi = hi.max(d);
            }
            if hi - lo < 0.02 {
                return d0;
            }
            let c = rgb[i];
            near.clear();
            for dy in -ri..=ri {
                for dx in -ri..=ri {
                    let j = at(x + dx, y + dy);
                    let q = rgb[j];
                    let e = (c[0] - q[0]).powi(2) + (c[1] - q[1]).powi(2) + (c[2] - q[2]).powi(2);
                    near.push((depth.data[j], (-e * k).exp()));
                }
            }
            let half = near.iter().map(|n| n.1).sum::<f32>() / 2.0;
            weighted_median(near, half).unwrap_or(d0)
        })
        .collect();
    Plane::new(w, h, data)
}

/// The smallest value whose weight together with that of all smaller ones reaches `half`
/// (what sorting and adding up would find), by partitioning instead of sorting.
fn weighted_median(mut v: &mut [(f32, f32)], half: f32) -> Option<f32> {
    let mut below = 0.0;
    while !v.is_empty() {
        let pivot = v[v.len() / 2].0;
        // Three parts: smaller, equal, larger than the pivot.
        let (mut lt, mut i, mut gt) = (0, 0, v.len());
        let (mut w_lt, mut w_eq) = (0.0, 0.0);
        while i < gt {
            let d = v[i].0;
            if d < pivot {
                w_lt += v[i].1;
                v.swap(i, lt);
                lt += 1;
                i += 1;
            } else if d > pivot {
                gt -= 1;
                v.swap(i, gt);
            } else {
                w_eq += v[i].1;
                i += 1;
            }
        }
        if below + w_lt >= half {
            v = &mut v[..lt];
        } else if below + w_lt + w_eq >= half {
            return Some(pivot);
        } else {
            below += w_lt + w_eq;
            v = &mut v[gt..];
        }
    }
    None
}

fn guide_resized(g: &Guide, w: usize, h: usize) -> sk_core::image::LinearImage {
    sk_core::image::LinearImage::new(g.width, g.height, g.rgb.clone()).resize_area(w, h)
}

fn bilinear(img: &sk_core::image::LinearImage, u: f32, v: f32) -> [f32; 3] {
    let x = (u * img.width as f32 - 0.5).clamp(0.0, (img.width - 1) as f32);
    let y = (v * img.height as f32 - 0.5).clamp(0.0, (img.height - 1) as f32);
    let (x0, y0) = (x as usize, y as usize);
    let (x1, y1) = ((x0 + 1).min(img.width - 1), (y0 + 1).min(img.height - 1));
    let (tx, ty) = (x - x0 as f32, y - y0 as f32);
    let at = |xx: usize, yy: usize| img.data[yy * img.width + xx];
    std::array::from_fn(|c| {
        let top = at(x0, y0)[c] + (at(x1, y0)[c] - at(x0, y0)[c]) * tx;
        let bot = at(x0, y1)[c] + (at(x1, y1)[c] - at(x0, y1)[c]) * tx;
        top + (bot - top) * ty
    })
}

fn sample(p: &Plane, u: f32, v: f32) -> f32 {
    let x = (u * p.w as f32 - 0.5).clamp(0.0, (p.w - 1) as f32);
    let y = (v * p.h as f32 - 0.5).clamp(0.0, (p.h - 1) as f32);
    let (x0, y0) = (x as usize, y as usize);
    let (x1, y1) = ((x0 + 1).min(p.w - 1), (y0 + 1).min(p.h - 1));
    let (tx, ty) = (x - x0 as f32, y - y0 as f32);
    let at = |xx: usize, yy: usize| p.data[yy * p.w + xx];
    let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
    let bot = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
    top + (bot - top) * ty
}

/// Registers the network as the provider of `sk_core::depth` (computing fails with a message
/// while it is not downloaded).
pub fn register() {
    let model = DepthModel::default();
    sk_core::depth::set_provider(&tag(), Box::new(move |path, progress| model.of_file(path, progress)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_edges_move_onto_the_image_edges() {
        // The image changes from red to gray at x = 30, the depth (soft, as the network's) from
        // 31 to 35; the window as for a map twice the network's size.
        let (w, h) = (64, 16);
        let rgb: Vec<[f32; 3]> = (0..w * h).map(|i| if i % w < 30 { [0.7, 0.3, 0.3] } else { [0.5, 0.5, 0.5] }).collect();
        let depth = Plane::new(w, h, (0..w * h).map(|i| (1.0 - ((i % w) as f32 - 31.0) / 4.0).clamp(0.0, 1.0) * 0.5 + 0.25).collect());
        let snapped = snap_edges(&depth, &rgb, 10, 0.08);
        let refined = refine_edges_color(&snapped, &rgb, 2, 1e-3);
        let at = |x: usize| refined.data[8 * w + x];
        // Near (high) up to the color edge, far behind it, across the old soft edge.
        assert!(at(29) > 0.7, "vor der Kante {}", at(29));
        for x in 30..40 {
            assert!(at(x) < 0.3, "hinter der Kante bei {x}: {}", at(x));
        }
    }

    #[test]
    fn weighted_median_finds_what_sorting_finds() {
        let mut seed = 3u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1 << 24) as f32
        };
        for n in [1, 2, 5, 441] {
            for _ in 0..50 {
                // Few distinct depths, so that equal ones occur.
                let v: Vec<(f32, f32)> = (0..n).map(|_| ((next() * 8.0).floor() / 8.0, next())).collect();
                let half = v.iter().map(|n| n.1).sum::<f32>() / 2.0;
                let mut sorted = v.clone();
                sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
                let mut acc = 0.0;
                let expected = sorted.iter().find(|n| {
                    acc += n.1;
                    acc >= half
                });
                assert_eq!(weighted_median(&mut v.clone(), half), expected.map(|n| n.0));
            }
        }
        assert_eq!(weighted_median(&mut [], 0.0), None);
    }

    #[test]
    fn input_keeps_the_aspect_in_patches() {
        assert_eq!(input_size(6000, 4000), (784, 518));
        assert_eq!(input_size(4000, 6000), (518, 784));
        assert_eq!(input_size(1000, 1000), (518, 518));
        // Panoramas: at most twice as long.
        assert_eq!(input_size(9000, 1000), (1036, 518));
    }

    /// With the downloaded model: the floor of a corridor-like scene rises towards the bottom edge.
    #[test]
    fn near_bottom_far_top_if_the_model_is_available() {
        if !available() {
            return;
        }
        // A floor of tiles receding to a horizon, the sky above.
        let (w, h) = (600, 400);
        let rgb = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                if y < 160.0 {
                    return [0.55, 0.7, 0.9];
                }
                let z = 1.0 / ((y - 159.0) / 240.0);
                let (u, v) = ((x - 300.0) * z / 60.0, z * 4.0);
                if (u.floor() as i64 + v.floor() as i64) % 2 == 0 { [0.8, 0.75, 0.7] } else { [0.3, 0.25, 0.2] }
            })
            .collect();
        let map = DepthModel::default().predict(&Guide { width: w, height: h, rgb }).unwrap();
        let (near, far) = (map.sample(0.5, 0.95), map.sample(0.5, 0.45));
        assert!(near > far + 0.2, "unten {near}, Horizont {far}");
    }
}
