//! Generative removal ("Entfernen"): the marked area is filled with plausible content by an
//! inpainting network (MI-GAN) that sees the surroundings. The result is stored as a small
//! patch in the image's settings (`params::Removal`), so nothing has to be computed again.

use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use sk_core::color::{srgb_decode, srgb_encode};
use sk_core::detail::{Plane, gaussian_blur};
use sk_core::image::LinearImage;
use sk_core::mask::{Patch, rasterize_strokes_in};
use sk_core::params::{BrushStroke, Removal};
use std::sync::Arc;

/// Network size.
pub const NET: usize = 512;

pub struct Inpainter {
    net: crate::runtime::Net,
}

impl Inpainter {
    pub fn load(path: &std::path::Path) -> Result<Self> {
        use crate::runtime::{Input, Net};
        Ok(Self { net: Net::load(path, &[Input::U8([1, 3, NET, NET]), Input::U8([1, 1, NET, NET])])? })
    }

    /// Fills the holes (`hole[i]` true) of an 8-bit RGB image of NET x NET pixels.
    pub fn fill(&self, rgb: &[[u8; 3]], hole: &[bool]) -> Result<Vec<[u8; 3]>> {
        let n = NET * NET;
        let mut img = vec![0u8; 3 * n];
        for (i, p) in rgb.iter().enumerate() {
            for c in 0..3 {
                img[c * n + i] = p[c];
            }
        }
        // 0 marks what is to be filled.
        let mask: Vec<u8> = hole.iter().map(|h| if *h { 0 } else { 255 }).collect();
        use crate::runtime::Data;
        let out = self.net.run(&[Data::U8(&img), Data::U8(&mask)])?;
        let o = out.first().context("Keine Modellausgabe")?;
        let d = &o.data;
        anyhow::ensure!(d.len() == 3 * n, "Unerwartete Modellausgabe {:?}", o.shape);
        let px = |v: f32| v.round().clamp(0.0, 255.0) as u8;
        Ok((0..n).map(|i| [px(d[i]), px(d[n + i]), px(d[2 * n + i])]).collect())
    }
}

/// Bilinear sample of camera data at full-resolution pixel coordinates.
fn sample(img: &LinearImage, x: f32, y: f32) -> [f32; 3] {
    let x = (x - 0.5).clamp(0.0, (img.width - 1) as f32);
    let y = (y - 0.5).clamp(0.0, (img.height - 1) as f32);
    let (x0, y0) = (x as usize, y as usize);
    let (x1, y1) = ((x0 + 1).min(img.width - 1), (y0 + 1).min(img.height - 1));
    let (tx, ty) = (x - x0 as f32, y - y0 as f32);
    let at = |xx: usize, yy: usize| img.data[yy * img.width + xx];
    let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
    std::array::from_fn(|k| {
        let top = a[k] + (b[k] - a[k]) * tx;
        let bot = c[k] + (d[k] - c[k]) * tx;
        top + (bot - top) * ty
    })
}

fn dilate(mask: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    let rows: Vec<bool> = (0..w * h).map(|i| {
        let (x, y) = (i % w, i / w);
        (x.saturating_sub(r)..(x + r + 1).min(w)).any(|xx| mask[y * w + xx])
    }).collect();
    (0..w * h).map(|i| {
        let (x, y) = (i % w, i / w);
        (y.saturating_sub(r)..(y + r + 1).min(h)).any(|yy| rows[yy * w + x])
    }).collect()
}

/// Largest marked extent (full-resolution pixels) filled at full resolution in several
/// network windows; larger areas are filled at a reduced resolution.
const FULL_RES_LIMIT: f32 = 1800.0;
/// Most pixels stored in a patch (larger patches are reduced).
const PATCH_PIXELS: usize = 1_000_000;

/// The marked area of the image at the resolution it is filled at.
struct Work {
    /// Area in full-resolution pixels: left, top, width, height (float for the reduced case).
    rect: [f32; 4],
    w: usize,
    h: usize,
    /// White balanced, normalized camera RGB, filled step by step.
    rgb: Vec<[f32; 3]>,
    /// Stroke coverage.
    cover: Vec<f32>,
}

/// Removes what the strokes mark from the image (camera RGB at full resolution, oriented).
/// `wb`: white balance multipliers used to show the network a natural image.
pub fn make_removal(full: &LinearImage, wb: [f32; 3], strokes: &[BrushStroke], model: &Inpainter, progress: &dyn Fn(f32)) -> Result<Removal> {
    let (fw, fh) = (full.width as f32, full.height as f32);
    let aspect = fw / fh;
    let long = fw.max(fh);
    // Bounding box of the strokes in full-resolution pixels.
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for s in strokes.iter().filter(|s| !s.erase) {
        let r = s.radius * long;
        for p in &s.points {
            x0 = x0.min(p[0] * fw - r);
            y0 = y0.min(p[1] * fh - r);
            x1 = x1.max(p[0] * fw + r);
            y1 = y1.max(p[1] * fh + r);
        }
    }
    if x1 <= x0 || y1 <= y0 {
        bail!("Nichts markiert");
    }
    let extent = (x1 - x0).max(y1 - y0);
    // Context around the marked area (the network needs to see the surroundings).
    let margin = (extent * 0.6).max(NET as f32 * 0.3);
    let (rx0, ry0) = ((x0 - margin).max(0.0).floor(), (y0 - margin).max(0.0).floor());
    let (rx1, ry1) = ((x1 + margin).min(fw).ceil(), (y1 + margin).min(fh).ceil());
    // Full resolution, or reduced so that the area is at most FULL_RES_LIMIT across.
    let step = (extent / FULL_RES_LIMIT).max(1.0);
    let (w, h) = (((rx1 - rx0) / step).round().max(1.0) as usize, ((ry1 - ry0) / step).round().max(1.0) as usize);
    let rect = [rx0, ry0, rx1 - rx0, ry1 - ry0];
    let min_wb = wb.iter().copied().fold(f32::MAX, f32::min).max(1e-6);
    let wb = wb.map(|v| v / min_wb);
    let rgb: Vec<[f32; 3]> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (rx0 + (i % w) as f32 * step, ry0 + (i / w) as f32 * step);
            let p = if step > 1.0 {
                let k = step.ceil() as usize;
                let mut acc = [0.0f32; 3];
                for j in 0..k {
                    for l in 0..k {
                        let q = sample(full, x + (l as f32 + 0.5) * step / k as f32, y + (j as f32 + 0.5) * step / k as f32);
                        acc = [acc[0] + q[0], acc[1] + q[1], acc[2] + q[2]];
                    }
                }
                acc.map(|v| v / (k * k) as f32)
            } else {
                sample(full, x + 0.5, y + 0.5)
            };
            [p[0] * wb[0], p[1] * wb[1], p[2] * wb[2]]
        })
        .collect();
    let norm = [rect[0] / fw, rect[1] / fh, rect[2] / fw, rect[3] / fh];
    let cover = rasterize_strokes_in(strokes, norm, w, h, aspect);
    let mut work = Work { rect, w, h, rgb, cover };
    fill_windows(&mut work, model, progress)?;
    let mut removal = finish(work, full, wb)?;
    removal.strokes = strokes.to_vec();
    Ok(removal)
}

/// Fills the marked area window by window (each NET x NET, with the surroundings and what
/// earlier windows filled as context).
fn fill_windows(work: &mut Work, model: &Inpainter, progress: &dyn Fn(f32)) -> Result<()> {
    let (w, h) = (work.w, work.h);
    let hole0: Vec<bool> = work.cover.iter().map(|c| *c > 0.05).collect();
    let hole = dilate(&hole0, w, h, 4);
    if !hole.iter().any(|x| *x) {
        bail!("Nichts markiert");
    }
    // Brightness for the network from the surroundings.
    let mut lum: Vec<f32> = work.rgb.iter().zip(&hole).filter(|(_, h)| !**h).map(|(p, _)| 0.25 * p[0] + 0.5 * p[1] + 0.25 * p[2]).collect();
    let gain = if lum.is_empty() {
        1.0
    } else {
        let mid = lum.len() / 2;
        (0.18 / lum.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1.max(1e-5)).clamp(0.25, 32.0)
    };
    // Window centers: per cell of a grid over the area, the middle of its marked pixels.
    let cell = NET * 5 / 8;
    let mut centers: Vec<(usize, usize)> = Vec::new();
    for cy in (0..h).step_by(cell) {
        for cx in (0..w).step_by(cell) {
            let (mut sx, mut sy, mut n) = (0usize, 0usize, 0usize);
            for y in cy..(cy + cell).min(h) {
                for x in cx..(cx + cell).min(w) {
                    if hole[y * w + x] {
                        sx += x;
                        sy += y;
                        n += 1;
                    }
                }
            }
            if let (Some(x), Some(y)) = (sx.checked_div(n), sy.checked_div(n)) {
                centers.push((x, y));
            }
        }
    }
    let mut done = vec![false; w * h];
    let total = centers.len();
    for (k, (cx, cy)) in centers.into_iter().enumerate() {
        progress(0.1 + 0.85 * k as f32 / total as f32);
        // Window origin (may extend beyond the area: those pixels are mirrored inside).
        let ox = cx as i64 - NET as i64 / 2;
        let oy = cy as i64 - NET as i64 / 2;
        let ox = if w >= NET { ox.clamp(0, (w - NET) as i64) } else { -(((NET - w) / 2) as i64) };
        let oy = if h >= NET { oy.clamp(0, (h - NET) as i64) } else { -(((NET - h) / 2) as i64) };
        let idx = |i: usize| -> usize {
            let x = (ox + (i % NET) as i64).clamp(0, w as i64 - 1) as usize;
            let y = (oy + (i / NET) as i64).clamp(0, h as i64 - 1) as usize;
            y * w + x
        };
        let inside = |i: usize| {
            let (x, y) = (ox + (i % NET) as i64, oy + (i / NET) as i64);
            x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h
        };
        let todo: Vec<bool> = (0..NET * NET).map(|i| inside(i) && hole[idx(i)] && !done[idx(i)]).collect();
        if !todo.iter().any(|t| *t) {
            continue;
        }
        let rgb8: Vec<[u8; 3]> = (0..NET * NET).map(|i| work.rgb[idx(i)].map(|v| (srgb_encode((v * gain).clamp(0.0, 1.0)) * 255.0 + 0.5) as u8)).collect();
        let filled = model.fill(&rgb8, &todo)?;
        for i in (0..NET * NET).filter(|&i| todo[i]) {
            let k = idx(i);
            work.rgb[k] = filled[i].map(|v| srgb_decode(v as f32 / 255.0) / gain);
            done[k] = true;
        }
    }
    Ok(())
}

/// The patch: filled pixels (camera RGB again) with a soft-edged coverage, cut to the marked
/// part and reduced if very large.
fn finish(work: Work, full: &LinearImage, wb: [f32; 3]) -> Result<Removal> {
    let (w, h) = (work.w, work.h);
    let hole0: Vec<bool> = work.cover.iter().map(|c| *c > 0.05).collect();
    let grown = dilate(&dilate(&hole0, w, h, 4), w, h, 2);
    let alpha = gaussian_blur(&Plane::new(w, h, grown.iter().map(|g| if *g { 1.0 } else { 0.0 }).collect()), 2.0);
    let (mut bx0, mut by0, mut bx1, mut by1) = (w, h, 0, 0);
    for (i, a) in alpha.data.iter().enumerate() {
        if *a > 0.002 {
            let (x, y) = (i % w, i / w);
            (bx0, by0, bx1, by1) = (bx0.min(x), by0.min(y), bx1.max(x + 1), by1.max(y + 1));
        }
    }
    let (cw, ch) = (bx1 - bx0, by1 - by0);
    let red = ((cw * ch) as f32 / PATCH_PIXELS as f32).sqrt().max(1.0);
    let (pw, ph) = (((cw as f32 / red).round() as usize).max(1), ((ch as f32 / red).round() as usize).max(1));
    let rgba: Vec<[f32; 4]> = (0..pw * ph)
        .map(|i| {
            let x = (bx0 as f32 + ((i % pw) as f32 + 0.5) * red) as usize;
            let y = (by0 as f32 + ((i / pw) as f32 + 0.5) * red) as usize;
            let k = y.min(h - 1) * w + x.min(w - 1);
            let p = work.rgb[k];
            [p[0] / wb[0], p[1] / wb[1], p[2] / wb[2], (alpha.data[k] * 1.25).min(1.0)]
        })
        .collect();
    let scale = rgba.iter().flat_map(|p| [p[0], p[1], p[2]]).fold(1e-6f32, f32::max);
    let patch = Patch { w: pw, h: ph, rgba };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64) ^ NEXT.fetch_add(1, Ordering::Relaxed);
    let (fw, fh) = (full.width as f32, full.height as f32);
    let [rx, ry, rw, rh] = work.rect;
    let (sx, sy) = (rw / w as f32, rh / h as f32);
    Ok(Removal {
        id,
        strokes: Vec::new(),
        rect: [(rx + bx0 as f32 * sx) / fw, (ry + by0 as f32 * sy) / fh, cw as f32 * sx / fw, ch as f32 * sy / fh],
        patch: Arc::new(patch.encode(scale)),
        scale,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_a_dark_spot_with_model_if_available() {
        let path = crate::models::path(&crate::models::INPAINT);
        if !path.exists() {
            return;
        }
        let model = Inpainter::load(&path).unwrap();
        // A grey surface with a dark blob.
        let (w, h) = (1200, 800);
        let mut data = vec![[0.18f32, 0.2, 0.16]; w * h];
        for y in 380..420 {
            for x in 580..620 {
                data[y * w + x] = [0.01; 3];
            }
        }
        let img = LinearImage::new(w, h, data);
        let stroke = BrushStroke { points: vec![[0.5, 0.5]], radius: 30.0 / 1200.0, hardness: 0.8, flow: 1.0, erase: false };
        let r = make_removal(&img, [1.0, 1.0, 1.0], &[stroke], &model, &|_| {}).unwrap();
        let p = Patch::decode(&r.patch, r.scale).unwrap();
        // The patch covers the blob and is bright there.
        let c = p.rgba[(p.h / 2) * p.w + p.w / 2];
        assert!(c[3] > 0.95 && c[1] > 0.1, "{c:?}");
        assert!(r.rect[0] < 0.49 && r.rect[0] + r.rect[2] > 0.51);
        // Applied through the pipeline stage, the blob is gone.
        let geo = sk_core::geometry::GeoMap::new(&Default::default(), &Default::default(), 0, w, h, true);
        let region = sk_core::geometry::Region { x: 0, y: 0, w, h };
        let mut out = geo.resample(&img, region);
        sk_core::mask::apply_removals(&mut out, &img, &geo, region, &[r]);
        assert!(out.data[400 * w + 600][1] > 0.1, "{:?}", out.data[400 * w + 600]);
        assert!((out.data[100 * w + 100][1] - 0.2).abs() < 1e-4);
    }
}
