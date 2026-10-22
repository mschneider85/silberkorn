//! Generative removal ("Entfernen"): the marked area is filled with plausible content by an
//! inpainting network (LaMa) that sees the surroundings. The result is stored as a small
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
        // On the graphics card if possible (0.4 s per window on a GTX 1650 Ti, 3.8 s on the
        // processor; about 800 MB of graphics memory).
        let me = Self { net: Net::load(path, &[Input::F32([1, 3, NET, NET]), Input::F32([1, 1, NET, NET])])? };
        // The first run on the graphics card compiles its shaders (seconds): now, while the tool
        // is chosen, not with the first stroke.
        if me.net.on_gpu() {
            me.fill(&vec![[128; 3]; NET * NET], &vec![false; NET * NET])?;
        }
        Ok(me)
    }

    /// Fills the holes (`hole[i]` true) of an 8-bit RGB image of NET x NET pixels.
    pub fn fill(&self, rgb: &[[u8; 3]], hole: &[bool]) -> Result<Vec<[u8; 3]>> {
        let n = NET * NET;
        // The network takes 0..1 and a mask of 1 where to fill; it returns 0..255.
        let mut img = vec![0f32; 3 * n];
        for (i, p) in rgb.iter().enumerate() {
            for c in 0..3 {
                img[c * n + i] = p[c] as f32 / 255.0;
            }
        }
        let mask: Vec<f32> = hole.iter().map(|h| if *h { 1.0 } else { 0.0 }).collect();
        use crate::runtime::Data;
        let out = self.net.run(&[Data::F32(&img), Data::F32(&mask)])?;
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

/// Largest marked extent (full-resolution pixels) whose patch has full resolution; larger
/// areas get a reduced one.
const FULL_RES_LIMIT: f32 = 1800.0;
/// The network fills at a size where the marked area is at most this much of its window:
/// with the surroundings in view (and at the scale of the pictures it learned on), not a
/// window that is mostly hole. The fine structure comes back afterwards (see `add_texture`).
const NET_EXTENT: f32 = NET as f32 * 0.35;
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

impl Work {
    /// The area `rect` of `full` every `step` full-resolution pixels (averaged when larger than 1),
    /// white balanced by `wb`, with the coverage of the strokes.
    fn new(full: &LinearImage, wb: [f32; 3], rect: [f32; 4], step: f32, strokes: &[BrushStroke], guide: Option<&sk_core::brush::AutoGuide>) -> Self {
        let (fw, fh) = (full.width as f32, full.height as f32);
        let [rx0, ry0, rw, rh] = rect;
        let (w, h) = ((rw / step).round().max(1.0) as usize, (rh / step).round().max(1.0) as usize);
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
        let norm = [rx0 / fw, ry0 / fh, rw / fw, rh / fh];
        let cover = rasterize_strokes_in(strokes, norm, w, h, fw / fh, guide);
        Self { rect, w, h, rgb, cover }
    }

    /// What is filled: the marks, a little grown.
    fn hole(&self) -> Vec<bool> {
        dilate(&self.cover.iter().map(|c| *c > 0.05).collect::<Vec<_>>(), self.w, self.h, 4)
    }
}

/// Removes what the strokes mark from the image (camera RGB at full resolution, oriented).
/// `wb`: white balance multipliers used to show the network a natural image.
pub fn make_removal(full: &LinearImage, wb: [f32; 3], strokes: &[BrushStroke], model: &Inpainter, progress: &dyn Fn(f32)) -> Result<Removal> {
    remove_with(full, wb, strokes, &|rgb, hole| model.fill(rgb, hole), progress)
}

/// [`make_removal`] with any filler of network windows.
fn remove_with(full: &LinearImage, wb: [f32; 3], strokes: &[BrushStroke], fill: Fill, progress: &dyn Fn(f32)) -> Result<Removal> {
    let (fw, fh) = (full.width as f32, full.height as f32);
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
    // The patch's resolution, and the (coarser) one the network fills at.
    let fine = (extent / FULL_RES_LIMIT).max(1.0);
    let coarse = (extent / NET_EXTENT).max(fine);
    // Context around the marked area: as much as the network's window shows at its scale.
    let margin = (extent * 0.6).max((NET as f32 * coarse - extent) / 2.0).max(NET as f32 * 0.3);
    let (rx0, ry0) = ((x0 - margin).max(0.0).floor(), (y0 - margin).max(0.0).floor());
    let (rx1, ry1) = ((x1 + margin).min(fw).ceil(), (y1 + margin).min(fh).ceil());
    let rect = [rx0, ry0, rx1 - rx0, ry1 - ry0];
    let min_wb = wb.iter().copied().fold(f32::MAX, f32::min).max(1e-6);
    let wb = wb.map(|v| v / min_wb);
    let guide = strokes.iter().any(|s| s.auto_mask).then(|| sk_core::brush::AutoGuide::from_image(full, sk_core::mask::MASK_RES));
    let mut small = Work::new(full, wb, rect, coarse, strokes, guide.as_ref());
    fill_windows(&mut small, fill, &|p| progress(p * 0.8))?;
    let scaled = coarse > fine * 1.05;
    let mut work = if scaled {
        let mut w = Work::new(full, wb, rect, fine, strokes, guide.as_ref());
        upsample_fill(&mut w, &small);
        w
    } else {
        small
    };
    progress(0.85);
    let hole = work.hole();
    match_edges(&mut work, &hole);
    progress(0.9);
    // Scaled up from much smaller, the fill is soft: the fine structure comes from the
    // surroundings, fully from a third of the size on. Filled about at its size, the network's
    // own structure is better (it would only be blurred by that).
    let strength = if scaled { ((coarse / fine - 1.8) / 1.2).clamp(0.0, 1.0) } else { 0.0 };
    if strength > 0.0 {
        let before = work.rgb.clone();
        // The network's fill is kept only as far as this blur: the fine structure (and with it
        // its blotches) is replaced by that of the surroundings.
        add_texture(&mut work, &hole, 4.0 / fine.min(2.0));
        for (p, b) in work.rgb.iter_mut().zip(&before) {
            *p = std::array::from_fn(|c| b[c] + (p[c] - b[c]) * strength);
        }
    }
    let opacity = strokes.iter().filter(|s| !s.erase).map(|s| s.opacity).fold(0.0f32, f32::max).clamp(0.0, 1.0);
    let mut removal = finish(work, full, wb, opacity)?;
    removal.strokes = strokes.to_vec();
    Ok(removal)
}

/// The holes of `work` filled from the coarser `small` (bilinear; both cover the same area).
fn upsample_fill(work: &mut Work, small: &Work) {
    let hole = work.hole();
    let (sx, sy) = (small.w as f32 / work.w as f32, small.h as f32 / work.h as f32);
    let (sw, sh) = (small.w, small.h);
    work.rgb.par_iter_mut().enumerate().filter(|(i, _)| hole[*i]).for_each(|(i, p)| {
        let fx = (((i % work.w) as f32 + 0.5) * sx - 0.5).clamp(0.0, (sw - 1) as f32);
        let fy = (((i / work.w) as f32 + 0.5) * sy - 0.5).clamp(0.0, (sh - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let at = |x: usize, y: usize| small.rgb[y * sw + x];
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        *p = std::array::from_fn(|k| (a[k] + (b[k] - a[k]) * tx) * (1.0 - ty) + (c[k] + (d[k] - c[k]) * tx) * ty);
    });
}

/// A per-channel blur that only takes the pixels where `use_px` is true (normalized), so that
/// what is inside a hole doesn't leak into what lies around it.
fn masked_blur(rgb: &[[f32; 3]], use_px: &[bool], w: usize, h: usize, sigma: f32) -> Vec<[f32; 3]> {
    let weight = gaussian_blur(&Plane::new(w, h, use_px.iter().map(|u| if *u { 1.0 } else { 0.0 }).collect()), sigma);
    let ch: Vec<Plane> = (0..3)
        .into_par_iter()
        .map(|c| gaussian_blur(&Plane::new(w, h, rgb.iter().zip(use_px).map(|(p, u)| if *u { p[c] } else { 0.0 }).collect()), sigma))
        .collect();
    (0..w * h).map(|i| std::array::from_fn(|c| ch[c].data[i] / weight.data[i].max(1e-6))).collect()
}

/// Brightness and color of the fill matched to its surroundings: along the edge of the hole,
/// the ratio of what lies around to what was filled there, spread over the whole hole (push-
/// pull) and applied, so that no edge and no color cast shows.
fn match_edges(work: &mut Work, hole: &[bool]) {
    let (w, h) = (work.w, work.h);
    let known: Vec<bool> = hole.iter().map(|x| !x).collect();
    // A few pixels wide on either side of the edge.
    let sigma = 3.0;
    let outside = masked_blur(&work.rgb, &known, w, h, sigma);
    let inside = masked_blur(&work.rgb, hole, w, h, sigma);
    let near_known = dilate(&known, w, h, 2);
    let mut num = vec![[0.0f32; 3]; w * h];
    let mut den = vec![0.0f32; w * h];
    for i in 0..w * h {
        // Hole pixels at its edge carry the ratio; the rest of the hole gets it spread.
        if hole[i] && near_known[i] {
            num[i] = std::array::from_fn(|c| (outside[i][c].max(1e-6) / inside[i][c].max(1e-6)).clamp(0.1, 10.0));
            den[i] = 1.0;
        }
    }
    let Some(ratio) = sk_core::highlights::push_pull(num, den, w, h) else { return };
    for i in (0..w * h).filter(|&i| hole[i]) {
        work.rgb[i] = std::array::from_fn(|c| work.rgb[i][c] * ratio[i][c]);
    }
}

/// The fine structure of the surroundings on the fill: the fill keeps its smooth part (blurred
/// by `sigma`), the detail comes in overlapping blocks from places around the hole whose smooth
/// part has a similar color (forest from forest, not from a cloud or a rock beside it).
fn add_texture(work: &mut Work, hole: &[bool], sigma: f32) {
    let (w, h) = (work.w, work.h);
    let known: Vec<bool> = hole.iter().map(|x| !x).collect();
    // The smooth part of the image around the hole, and of the fill.
    let smooth_known = masked_blur(&work.rgb, &known, w, h, sigma);
    let smooth_fill = masked_blur(&work.rgb, hole, w, h, sigma);
    // Detail as a ratio (linear data: its contrast goes with the brightness).
    let detail = |i: usize| -> [f32; 3] { std::array::from_fn(|c| (work.rgb[i][c].max(1e-6) / smooth_known[i][c].max(1e-6)).clamp(0.3, 3.0)) };
    // Places to take blocks from: wholly outside the hole (with a little distance).
    let block = ((sigma * 5.0).round() as usize).clamp(12, 48);
    let step = block * 2 / 3;
    // A block's middle this far from the hole keeps all of it clear of the hole and of what was
    // left of the removed thing at its edge (a few pixels).
    let away = dilate(hole, w, h, block * 3 / 4 + 8);
    let sources: Vec<(usize, usize)> = (0..h.saturating_sub(block))
        .step_by(3)
        .flat_map(|y| (0..w.saturating_sub(block)).step_by(3).map(move |x| (x, y)))
        .filter(|&(x, y)| !away[(y + block / 2) * w + x + block / 2])
        .collect();
    if sources.is_empty() {
        return;
    }
    let mean = |m: &[[f32; 3]], x: usize, y: usize| -> [f32; 3] {
        let mut a = [0.0f32; 3];
        for (dy, dx) in [(0, 0), (block - 1, 0), (0, block - 1), (block - 1, block - 1), (block / 2, block / 2)] {
            let p = m[(y + dy).min(h - 1) * w + (x + dx).min(w - 1)];
            a = [a[0] + p[0], a[1] + p[1], a[2] + p[2]];
        }
        a.map(|v| (v / 5.0).max(1e-6).ln())
    };
    // Blocks over the hole, each with its best source among some candidates nearby.
    let (mut acc, mut wsum, mut trust) = (vec![[0.0f32; 3]; w * h], vec![0.0f32; w * h], vec![0.0f32; w * h]);
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for by in (0..h).step_by(step) {
        for bx in (0..w).step_by(step) {
            if !(by..(by + block).min(h)).any(|y| (bx..(bx + block).min(w)).any(|x| hole[y * w + x])) {
                continue;
            }
            let target = mean(&smooth_fill, bx, by);
            let dist = |s: (usize, usize)| {
                let m = mean(&smooth_known, s.0, s.1);
                (0..3).map(|c| (m[c] - target[c]).powi(2)).sum::<f32>()
            };
            let best = (0..48).map(|_| sources[(rnd() % sources.len() as u64) as usize]).min_by(|a, b| dist(*a).total_cmp(&dist(*b))).unwrap_or(sources[0]);
            // Without a source of the same color (log distance), the network's fill stays as it is.
            let alike = (1.0 - dist(best) / 0.05).clamp(0.0, 1.0);
            if alike <= 0.0 {
                continue;
            }
            for j in 0..block {
                for i in 0..block {
                    let (x, y) = (bx + i, by + j);
                    if x >= w || y >= h || !hole[y * w + x] {
                        continue;
                    }
                    // Feathered towards the block's edges, so that blocks blend into each other.
                    let fx = 1.0 - ((i as f32 + 0.5) / block as f32 - 0.5).abs() * 2.0;
                    let fy = 1.0 - ((j as f32 + 0.5) / block as f32 - 0.5).abs() * 2.0;
                    let wt = (fx * fy).max(0.02);
                    let d = detail((best.1 + j) * w + best.0 + i);
                    let k = y * w + x;
                    acc[k] = [acc[k][0] + d[0] * wt, acc[k][1] + d[1] * wt, acc[k][2] + d[2] * wt];
                    wsum[k] += wt;
                    trust[k] += alike * wt;
                }
            }
        }
    }
    for i in (0..w * h).filter(|&i| hole[i] && wsum[i] > 0.0) {
        // Averaging overlapping blocks lowers the detail's contrast: a little more of it.
        let d = acc[i].map(|v| v / wsum[i]);
        let textured: [f32; 3] = std::array::from_fn(|c| smooth_fill[i][c] * (1.0 + (d[c] - 1.0) * 1.25).max(0.0));
        let t = (trust[i] / wsum[i]).clamp(0.0, 1.0);
        work.rgb[i] = std::array::from_fn(|c| work.rgb[i][c] + (textured[c] - work.rgb[i][c]) * t);
    }
}

/// The network: one NET x NET window (sRGB) and its pixels to fill -> the filled window.
type Fill<'a> = &'a dyn Fn(&[[u8; 3]], &[bool]) -> Result<Vec<[u8; 3]>>;

/// Fills the marked area window by window (each NET x NET, with the surroundings and what
/// earlier windows filled as context).
fn fill_windows(work: &mut Work, fill: Fill, progress: &dyn Fn(f32)) -> Result<()> {
    let (w, h) = (work.w, work.h);
    let hole = work.hole();
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
    let mut centers = centers.into_iter();
    // Where the search for marked pixels left goes on (all before it are done).
    let mut scan = 0;
    for k in 0.. {
        // After the grid: a window on each marked pixel still left (a cell's window reaches only
        // half a window around the middle of its marks; the window always contains the pixel).
        let Some((cx, cy)) = centers.next().or_else(|| {
            scan = (scan..w * h).find(|&i| hole[i] && !done[i])?;
            Some((scan % w, scan / w))
        }) else { break };
        progress((0.1 + 0.85 * k as f32 / total as f32).min(0.95));
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
        let filled = fill(&rgb8, &todo)?;
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
fn finish(work: Work, full: &LinearImage, wb: [f32; 3], opacity: f32) -> Result<Removal> {
    let (w, h) = (work.w, work.h);
    // The fill reaches a little beyond the marks and fades over a few pixels (matched to the
    // surroundings, see `match_edges`, it may).
    let grown = dilate(&work.hole(), w, h, 3);
    let alpha = gaussian_blur(&Plane::new(w, h, grown.iter().map(|g| if *g { 1.0 } else { 0.0 }).collect()), 3.0);
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
            [p[0] / wb[0], p[1] / wb[1], p[2] / wb[2], (alpha.data[k] * 1.25).min(1.0) * opacity]
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
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_marked_pixel_is_filled() {
        // In one grid cell: a dense stroke at its right edge and a dot far from it, beyond the
        // reach of the window around the middle of the cell's marks.
        let (w, h) = (1200, 700);
        let mut cover = vec![0.0f32; w * h];
        for y in 0..320 {
            for x in 300..320 {
                cover[y * w + x] = 1.0;
            }
        }
        for y in 4..8 {
            for x in 4..8 {
                cover[y * w + x] = 1.0;
            }
        }
        let mut work = Work { rect: [0.0, 0.0, w as f32, h as f32], w, h, rgb: vec![[0.0; 3]; w * h], cover: cover.clone() };
        let windows = std::cell::Cell::new(0);
        let fill = |rgb: &[[u8; 3]], _: &[bool]| {
            windows.set(windows.get() + 1);
            Ok(vec![[255u8; 3]; rgb.len()])
        };
        fill_windows(&mut work, &fill, &|_| {}).unwrap();
        let missed = (0..w * h).filter(|&i| cover[i] > 0.05 && work.rgb[i][0] == 0.0).count();
        assert_eq!(missed, 0, "{} Fenster", windows.get());
    }

    /// A wrongly colored, flat fill of a large area of a textured surface (filled much reduced):
    /// afterwards it has the surroundings' color and about their grain.
    #[test]
    fn the_fill_takes_color_and_grain_from_the_surroundings() {
        let (w, h) = (2400, 1600);
        let grain = |x: usize, y: usize| 1.0 + 0.3 * sk_core::detail::hash_noise(x as i64, y as i64, 5);
        let data: Vec<[f32; 3]> = (0..w * h).map(|i| [0.1, 0.2, 0.3].map(|v| v * grain(i % w, i / w))).collect();
        let img = LinearImage::new(w, h, data);
        let stroke = BrushStroke { points: vec![[0.5, 0.3], [0.5, 0.7]], radius: 30.0 / 2400.0, hardness: 0.8, flow: 1.0, erase: false, opacity: 1.0, auto_mask: false, build_up: true, tolerance: 50.0 };
        // A network that fills with a flat, too bright and too warm color.
        let fill = |rgb: &[[u8; 3]], hole: &[bool]| Ok(rgb.iter().zip(hole).map(|(p, h)| if *h { [200, 150, 120] } else { *p }).collect());
        let r = remove_with(&img, [1.0; 3], &[stroke], &fill, &|_| {}).unwrap();
        let p = Patch::decode(&r.patch, r.scale).unwrap();
        let (cx, cy) = (p.w / 2, p.h / 2);
        let px: Vec<[f32; 4]> = (cy - 10..cy + 10).flat_map(|y| (cx - 5..cx + 5).map(move |x| (x, y))).map(|(x, y)| p.rgba[y * p.w + x]).collect();
        let n = px.len() as f32;
        let mean: [f32; 3] = std::array::from_fn(|c| px.iter().map(|q| q[c]).sum::<f32>() / n);
        let sd = (px.iter().map(|q| (q[1] - mean[1]).powi(2)).sum::<f32>() / n).sqrt() / mean[1];
        for (c, want) in [0.1, 0.2, 0.3].iter().enumerate() {
            assert!((mean[c] - want).abs() < 0.15 * want, "Farbe {mean:?}");
        }
        // The surroundings vary by about 0.3 / sqrt(3) (uniform noise).
        assert!(sd > 0.08, "Korn {sd}");
    }

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
        let stroke = BrushStroke { points: vec![[0.5, 0.5]], radius: 30.0 / 1200.0, hardness: 0.8, flow: 1.0, erase: false, opacity: 1.0, auto_mask: false, build_up: true, tolerance: 50.0 };
        let r = make_removal(&img, [1.0, 1.0, 1.0], &[stroke], &model, &|_| {}).unwrap();
        let p = Patch::decode(&r.patch, r.scale).unwrap();
        // The patch covers the blob and is bright there.
        let c = p.rgba[(p.h / 2) * p.w + p.w / 2];
        assert!(c[3] > 0.95 && c[1] > 0.1, "{c:?}");
        assert!(r.rect[0] < 0.49 && r.rect[0] + r.rect[2] > 0.51);
        // Applied through the pipeline stage, the blob is gone.
        let geo = sk_core::geometry::GeoMap::new(&Default::default(), &Default::default(), None, 0, w, h, true);
        let region = sk_core::geometry::Region { x: 0, y: 0, w, h };
        let mut out = geo.resample(&img, region);
        sk_core::mask::apply_removals(&mut out, &img, &geo, region, &[r]);
        assert!(out.data[400 * w + 600][1] > 0.1, "{:?}", out.data[400 * w + 600]);
        assert!((out.data[100 * w + 100][1] - 0.2).abs() < 1e-4);
    }
}
