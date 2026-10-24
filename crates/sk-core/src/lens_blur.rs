//! Lens blur: blurs the image by its depth map (see `depth`) like a wide aperture would, on the
//! linear source before the pipeline, so it shows in previews, thumbnails and exports alike.
//!
//! The blur radius grows with the distance in depth from the focus (the network's depth is
//! inverse depth, to which a lens's circle of confusion is proportional). Behind the focus each
//! pixel gathers from neighbors that are at least about as blurred as itself, so the sharp subject
//! does not bleed into the background as a halo. In front of the focus the blurred parts spread
//! over what lies behind them, like a near out-of-focus branch does: they are cut into layers by
//! radius, each blurred with its alpha and laid over the rest.
//!
//! Large radii are blurred on a reduced copy (a disc of at most a few pixels there), so the cost
//! hardly depends on the radius.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rayon::prelude::*;

use crate::depth::DepthMap;
use crate::image::LinearImage;
use crate::params::LensBlur;

/// Largest blur radius (at amount 100), relative to the long edge.
pub const MAX_RADIUS: f32 = 0.025;
/// Blur levels between sharp and the largest radius at most.
const MAX_LEVELS: usize = 6;
/// Depth distance beyond the sharp range at which the blur reaches its full radius.
const FULL_AT: f32 = 0.5;

/// Blur radius in pixels at depth `d`: negative behind the focus, positive in front of it.
pub fn radius(p: &LensBlur, d: f32, max_r: f32) -> f32 {
    let off = d - p.focus;
    let excess = (off.abs() - p.range.max(0.0) / 2.0).max(0.0);
    max_r * (excess / FULL_AT).min(1.0) * off.signum()
}

/// Blurred sources by (source, image, settings): renderings of the same source with the same
/// settings reuse one, so the pipeline's stage caches (keyed by the source) stay valid.
type BlurKey = (usize, usize, usize, u64, PathBuf, [u32; 4]);
static BLURRED: Mutex<Vec<(BlurKey, Arc<LinearImage>)>> = Mutex::new(Vec::new());
/// Memory for blurred sources (the newest are kept).
const BLUR_BUDGET: usize = 600 << 20;

/// The source of the image at `path` with the lens blur, or `None` if there is nothing to blur
/// or no depth map (yet).
pub fn blurred(path: &Path, src: &LinearImage, p: &LensBlur, clip: bool) -> Option<Arc<LinearImage>> {
    if p.is_off() || src.data.is_empty() {
        return None;
    }
    let depth = crate::depth::load(path)?;
    let bits = [p.amount, p.focus, p.range, p.bokeh].map(f32::to_bits);
    let key: BlurKey = (src.data.as_ptr() as usize, src.width, src.height, crate::ai_products::fingerprint(src), path.to_path_buf(), bits);
    {
        let mut done = BLURRED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = done.iter().position(|(k, _)| *k == key) {
            let e = done.remove(i);
            let img = e.1.clone();
            done.push(e);
            return Some(img);
        }
    }
    let img = Arc::new(apply(src, &depth, p, clip));
    let mut done = BLURRED.lock().unwrap_or_else(|e| e.into_inner());
    done.push((key, img.clone()));
    let mut total: usize = done.iter().map(|(_, i)| i.data.len() * 12).sum();
    while total > BLUR_BUDGET && done.len() > 1 {
        total -= done.remove(0).1.data.len() * 12;
    }
    Some(img)
}

/// The source blurred by `depth`. `clip`: values above 1 are clipped sensor data (not HDR),
/// so brightened highlights must not go beyond.
pub fn apply(src: &LinearImage, depth: &DepthMap, p: &LensBlur, clip: bool) -> LinearImage {
    let (w, h) = (src.width, src.height);
    let max_r = p.amount.clamp(0.0, 100.0) / 100.0 * MAX_RADIUS * w.max(h) as f32;
    if max_r < 0.75 || w < 2 || h < 2 {
        return src.clone();
    }
    let depth = Snapped::new(depth, src);
    let rad: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|i| radius(p, depth.sample(((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32, src.data[i]), max_r))
        .collect();
    let levels = ((max_r / 5.0).ceil() as usize).clamp(1, MAX_LEVELS);
    let step = max_r / levels as f32;
    let r_of = |k: usize| step * k as f32;
    // Highlights become brighter discs, as the sensor clipped them far below their real brightness.
    let boost = p.bokeh.clamp(0.0, 100.0) / 100.0 * 4.0;
    let lit = |c: [f32; 3]| {
        if boost <= 0.0 {
            return c;
        }
        let m = c[0].max(c[1]).max(c[2]);
        let t = ((m - 0.5) / 0.45).clamp(0.0, 1.0);
        let g = 1.0 + boost * t * t * (3.0 - 2.0 * t);
        c.map(|v| v * g)
    };

    // Behind the focus: one gathered blur per level, from neighbors about as blurred as the level.
    let far = || {
        gathered(w, h, levels, step, |i| -rad[i] / step, |i, rk| {
            let wgt = ((-rad[i]).max(0.0) / rk).clamp(0.0, 1.0).powi(2).max(1e-3);
            (lit(src.data[i]), wgt)
        })
    };
    // What the blurred foreground half covers: the neighbors that are not in front, at the
    // foreground's radius.
    let behind = || gathered(w, h, levels, step, |i| rad[i] / step, |i, _| (src.data[i], if rad[i] > 0.0 { 1e-3 } else { 1.0 }));
    // In front of the focus: layers by radius, each with its alpha.
    let near = || -> Vec<Grid> {
        (1..=levels)
            .filter_map(|k| {
                let rk = r_of(k);
                let alpha = |i: usize| (1.0 - (rad[i].max(0.0) - rk).abs() / step).max(0.0);
                // Only around the layer's own pixels (zero elsewhere).
                Grid::blurred(w, h, rk, |i| alpha(i) > 0.0, true, |i| {
                    let a = alpha(i);
                    let c = lit(src.data[i]);
                    [c[0] * a, c[1] * a, c[2] * a, a]
                })
            })
            .collect()
    };
    // Each grid is parallel inside. Side by side the three only add their transient buffers,
    // which at full resolution (small first radius, `Grid::blurred`'s f = 1) or on large images
    // triple the peak memory: then one after the other.
    let full_res = (step / 3.0).round() <= 1.0;
    let (far, (behind, near)) = if full_res || w * h > 8_000_000 {
        (far(), (behind(), near()))
    } else {
        rayon::join(far, || rayon::join(behind, near))
    };

    let data = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let orig = src.data[i];
            // Background: between the two levels around the pixel's own radius; in front of the
            // focus what lies behind it.
            let mut out = if rad[i] > 0.0 { pick(&behind, rad[i] / step, x, y, orig) } else { pick(&far, -rad[i] / step, x, y, orig) };
            // Foreground layers over it.
            if !near.is_empty() {
                let mut acc = [0.0f32; 4];
                for g in &near {
                    let v = g.sample(x, y);
                    acc = [acc[0] + v[0], acc[1] + v[1], acc[2] + v[2], acc[3] + v[3]];
                }
                let cover = acc[3].min(1.0);
                let norm = if acc[3] > 1.0 { 1.0 / acc[3] } else { 1.0 };
                out = [0, 1, 2].map(|c| acc[c] * norm + out[c] * (1.0 - cover));
            }
            if clip && boost > 0.0 {
                out = [0, 1, 2].map(|c| out[c].min(orig[c].max(1.0)));
            }
            out
        })
        .collect();
    LinearImage::new(w, h, data)
}

/// A depth map whose soft transitions at depth edges jump to the near or the far side: there is
/// nothing in between, and the values in between would pass through the focus and leave a sharp
/// seam around blurred foreground. Which side a pixel at an edge belongs to, its color decides
/// (the map is coarser than the image: its cells would show as steps along the edge).
struct Snapped {
    map: DepthMap,
    /// The image at the map's size, in [`Snapped::tone`].
    colors: Vec<[f32; 3]>,
}

/// Weight of the distance (in map cells, squared) against the color difference when a pixel
/// at an edge picks the side it belongs to: where the colors don't tell, the nearer cell does.
const NEARNESS: f32 = 2e-3;

/// Depth difference that counts as an edge.
const EDGE: f32 = 0.15;

impl Snapped {
    fn new(d: &DepthMap, src: &LinearImage) -> Self {
        let (w, h) = (d.width, d.height);
        let colors = src.resize_area(w, h).data.into_iter().map(Self::tone).collect();
        let r = 2usize;
        // Single specks of another depth (stray pixels along edges) go first.
        let d = &DepthMap {
            width: w,
            height: h,
            data: (0..w * h)
                .into_par_iter()
                .map(|i| {
                    let (x, y) = (i % w, i / w);
                    let mut v: Vec<f32> = Vec::with_capacity(25);
                    for yy in y.saturating_sub(r)..(y + r + 1).min(h) {
                        for xx in x.saturating_sub(r)..(x + r + 1).min(w) {
                            v.push(d.data[yy * w + xx]);
                        }
                    }
                    let m = v.len() / 2;
                    *v.select_nth_unstable_by(m, f32::total_cmp).1
                })
                .collect(),
        };
        let data = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let (mut lo, mut hi) = (f32::MAX, f32::MIN);
                for yy in y.saturating_sub(r)..(y + r + 1).min(h) {
                    for xx in x.saturating_sub(r)..(x + r + 1).min(w) {
                        let v = d.data[yy * w + xx];
                        (lo, hi) = (lo.min(v), hi.max(v));
                    }
                }
                let v = d.data[i];
                if hi - lo < EDGE { v } else if v - lo < hi - v { lo } else { hi }
            })
            .collect();
        Self { map: DepthMap { width: w, height: h, data }, colors }
    }

    /// Colors compared roughly as seen (linear values would make dark differences vanish).
    fn tone(c: [f32; 3]) -> [f32; 3] {
        c.map(|v| v.max(0.0).sqrt())
    }

    /// Bilinear, but across an edge the value of the cell nearby whose color is most like the
    /// pixel's (`color`).
    fn sample(&self, u: f32, v: f32, color: [f32; 3]) -> f32 {
        let m = &self.map;
        let x = (u * m.width as f32 - 0.5).clamp(0.0, (m.width - 1) as f32);
        let y = (v * m.height as f32 - 0.5).clamp(0.0, (m.height - 1) as f32);
        let (x0, y0) = (x as usize, y as usize);
        let (x1, y1) = ((x0 + 1).min(m.width - 1), (y0 + 1).min(m.height - 1));
        let c = [m.data[y0 * m.width + x0], m.data[y0 * m.width + x1], m.data[y1 * m.width + x0], m.data[y1 * m.width + x1]];
        let spread = c.iter().cloned().fold(f32::MIN, f32::max) - c.iter().cloned().fold(f32::MAX, f32::min);
        if spread < EDGE {
            return m.sample(u, v);
        }
        let tone = Self::tone(color);
        let mut best = (f32::MAX, 0.0);
        for yy in y0.saturating_sub(1)..(y0 + 3).min(m.height) {
            for xx in x0.saturating_sub(1)..(x0 + 3).min(m.width) {
                let j = yy * m.width + xx;
                let c = self.colors[j];
                let (dx, dy) = (xx as f32 - x, yy as f32 - y);
                let score = (0..3).map(|k| (c[k] - tone[k]).powi(2)).sum::<f32>() + NEARNESS * (dx * dx + dy * dy);
                if score < best.0 {
                    best = (score, m.data[j]);
                }
            }
        }
        best.1
    }
}

/// One gathered blur per level 1..=`levels` (radius `step` * level): each pixel's color with its
/// weight from `value(pixel, radius)`, normalized by the weights. A level is computed only where
/// [`pick`] takes it, by the pixels' levels `level(pixel)`.
fn gathered(
    w: usize,
    h: usize,
    levels: usize,
    step: f32,
    level: impl Fn(usize) -> f32 + Sync,
    value: impl Fn(usize, f32) -> ([f32; 3], f32) + Sync,
) -> Vec<Option<Grid>> {
    (0..=levels)
        .map(|k| {
            let uses = |i: usize| {
                let t = level(i);
                t > 0.0 && t > k as f32 - 1.0 && t < k as f32 + 1.0
            };
            if k == 0 {
                return None;
            }
            let rk = step * k as f32;
            Grid::blurred(w, h, rk, uses, false, |i| {
                let (c, wgt) = value(i, rk);
                [c[0] * wgt, c[1] * wgt, c[2] * wgt, wgt]
            })
        })
        .collect()
}

/// Cells of a `w` x `h` grid within `d` cells (a square) of a set one.
fn grown(set: &[bool], w: usize, h: usize, d: usize) -> Vec<bool> {
    if d == 0 {
        return set.to_vec();
    }
    // Rows, then columns, each with a running count.
    let pass = |src: &[bool], along_rows: bool| -> Vec<bool> {
        let (n, len) = if along_rows { (h, w) } else { (w, h) };
        let at = |line: usize, i: usize| if along_rows { line * w + i } else { i * w + line };
        let mut out = vec![false; w * h];
        let lines: Vec<Vec<bool>> = (0..n)
            .into_par_iter()
            .map(|line| {
                let mut prefix = vec![0u32; len + 1];
                for i in 0..len {
                    prefix[i + 1] = prefix[i] + src[at(line, i)] as u32;
                }
                (0..len).map(|i| prefix[(i + d + 1).min(len)] > prefix[i.saturating_sub(d)]).collect()
            })
            .collect();
        for (line, v) in lines.into_iter().enumerate() {
            for (i, b) in v.into_iter().enumerate() {
                out[at(line, i)] = b;
            }
        }
        out
    };
    pass(&pass(set, true), false)
}

/// A pixel of gathered blurs at level `t` (fractional, between the two levels around it); level 0
/// and missing levels are `orig`.
fn pick(grids: &[Option<Grid>], t: f32, x: usize, y: usize, orig: [f32; 3]) -> [f32; 3] {
    let levels = grids.len() - 1;
    let level = |k: usize| match grids.get(k).and_then(|g| g.as_ref()) {
        Some(g) => {
            let v = g.sample(x, y);
            if v[3] <= 0.0 {
                return orig;
            }
            [v[0] / v[3], v[1] / v[3], v[2] / v[3]]
        }
        None => orig,
    };
    let k0 = (t.floor().max(0.0) as usize).min(levels);
    let f = t - k0 as f32;
    if k0 == levels || f <= 0.0 {
        return level(k0);
    }
    let (a, b) = (level(k0), level(k0 + 1));
    [0, 1, 2].map(|c| a[c] + (b[c] - a[c]) * f)
}

/// A blurred image on a grid reduced by `f`: premultiplied color plus weight per cell.
struct Grid {
    w: usize,
    h: usize,
    f: usize,
    data: Vec<[f32; 4]>,
}

impl Grid {
    /// `value` of each pixel of a `w` x `h` image, reduced so that the radius `r` is a disc of
    /// 4 to 8 cells, and blurred with that disc. Only where `need(pixel)`: around those pixels
    /// (`spread`: they are the only ones with a value, which spreads by the radius), elsewhere the
    /// grid stays empty. `None` if no pixel needs it.
    fn blurred(w: usize, h: usize, r: f32, need: impl Fn(usize) -> bool + Sync, spread: bool, value: impl Fn(usize) -> [f32; 4] + Sync) -> Option<Grid> {
        let f = ((r / 3.0).round() as usize).max(1);
        let (gw, gh) = (w.div_ceil(f), h.div_ceil(f));
        let rr = r / f as f32;
        let ri = rr.ceil() as isize;
        let marked: Vec<bool> = (0..gw * gh)
            .into_par_iter()
            .map(|j| {
                let (gx, gy) = (j % gw, j / gw);
                (gy * f..((gy + 1) * f).min(h)).any(|y| (gx * f..((gx + 1) * f).min(w)).any(|x| need(y * w + x)))
            })
            .collect();
        if !marked.contains(&true) {
            return None;
        }
        // Cells to compute (with one more for the bilinear sampling), and the cells they read.
        let (used, input) = if spread {
            (grown(&marked, gw, gh, ri as usize + 1), marked)
        } else {
            let used = grown(&marked, gw, gh, 1);
            let input = grown(&used, gw, gh, ri as usize);
            (used, input)
        };
        let small: Vec<[f32; 4]> = (0..gw * gh)
            .into_par_iter()
            .map(|j| {
                if !input[j] {
                    return [0.0; 4];
                }
                let (gx, gy) = (j % gw, j / gw);
                let mut acc = [0.0f32; 4];
                let mut n = 0.0;
                for y in gy * f..((gy + 1) * f).min(h) {
                    for x in gx * f..((gx + 1) * f).min(w) {
                        let v = value(y * w + x);
                        acc = [acc[0] + v[0], acc[1] + v[1], acc[2] + v[2], acc[3] + v[3]];
                        n += 1.0;
                    }
                }
                acc.map(|v| v / n)
            })
            .collect();
        // Disc with an antialiased rim.
        let kernel: Vec<(isize, isize, f32)> = (-ri..=ri)
            .flat_map(|dy| (-ri..=ri).map(move |dx| (dx, dy)))
            .filter_map(|(dx, dy)| {
                let k = (rr + 0.5 - ((dx * dx + dy * dy) as f32).sqrt()).clamp(0.0, 1.0);
                (k > 0.0).then_some((dx, dy, k))
            })
            .collect();
        let data = (0..gw * gh)
            .into_par_iter()
            .map(|j| {
                if !used[j] {
                    return [0.0; 4];
                }
                let (gx, gy) = ((j % gw) as isize, (j / gw) as isize);
                let mut acc = [0.0f32; 4];
                let mut ksum = 0.0;
                for &(dx, dy, k) in &kernel {
                    let (x, y) = (gx + dx, gy + dy);
                    if x < 0 || y < 0 || x >= gw as isize || y >= gh as isize {
                        continue;
                    }
                    let v = small[y as usize * gw + x as usize];
                    acc = [acc[0] + v[0] * k, acc[1] + v[1] * k, acc[2] + v[2] * k, acc[3] + v[3] * k];
                    ksum += k;
                }
                acc.map(|v| v / ksum.max(1e-9))
            })
            .collect();
        Some(Grid { w: gw, h: gh, f, data })
    }

    /// Bilinear sample at a pixel of the full image.
    #[inline]
    fn sample(&self, x: usize, y: usize) -> [f32; 4] {
        if self.f == 1 {
            return self.data[y * self.w + x];
        }
        let gx = ((x as f32 + 0.5) / self.f as f32 - 0.5).clamp(0.0, (self.w - 1) as f32);
        let gy = ((y as f32 + 0.5) / self.f as f32 - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (gx as usize, gy as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = (gx - x0 as f32, gy - y0 as f32);
        let at = |xx: usize, yy: usize| self.data[yy * self.w + xx];
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        std::array::from_fn(|i| {
            let top = a[i] + (b[i] - a[i]) * tx;
            let bot = c[i] + (d[i] - c[i]) * tx;
            top + (bot - top) * ty
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sharp bright square in the middle (near, in focus) on a striped background (far).
    fn scene() -> (LinearImage, DepthMap) {
        let (w, h) = (200, 120);
        let data = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                if (80..120).contains(&x) && (40..80).contains(&y) { [0.9, 0.9, 0.9] } else if (x / 4) % 2 == 0 { [0.1, 0.1, 0.1] } else { [0.5, 0.5, 0.5] }
            })
            .collect();
        let depth = (0..w * h).map(|i| if (80..120).contains(&(i % w)) && (40..80).contains(&(i / w)) { 1.0 } else { 0.0 }).collect();
        (LinearImage::new(w, h, data), DepthMap { width: w, height: h, data: depth })
    }

    fn contrast(img: &LinearImage, x0: usize, x1: usize, y: usize) -> f32 {
        let row: Vec<f32> = (x0..x1).map(|x| img.data[y * img.width + x][1]).collect();
        row.iter().cloned().fold(f32::MIN, f32::max) - row.iter().cloned().fold(f32::MAX, f32::min)
    }

    #[test]
    fn background_blurs_and_focus_stays_sharp() {
        let (img, depth) = scene();
        let p = LensBlur { amount: 100.0, focus: 1.0, range: 0.1, bokeh: 0.0 };
        let out = apply(&img, &depth, &p, true);
        // Stripes behind are smoothed out.
        assert!(contrast(&img, 10, 40, 20) > 0.35);
        assert!(contrast(&out, 10, 40, 20) < 0.08, "{}", contrast(&out, 10, 40, 20));
        // The subject is untouched.
        assert_eq!(out.data[60 * 200 + 100], img.data[60 * 200 + 100]);
        // No halo: the background right next to the subject keeps the background's brightness.
        let next = out.data[60 * 200 + 124][1];
        assert!((next - 0.3).abs() < 0.08, "Hof um das Motiv: {next}");
    }

    #[test]
    fn foreground_spreads_over_sharp_background() {
        let (img, depth) = scene();
        // Focus on the background: the near square blurs and spreads beyond its edge.
        let p = LensBlur { amount: 100.0, focus: 0.0, range: 0.1, bokeh: 0.0 };
        let out = apply(&img, &depth, &p, true);
        assert!(contrast(&out, 10, 40, 20) > 0.35, "Hintergrund bleibt scharf");
        let inside_edge = out.data[60 * 200 + 81][1];
        let outside_edge = out.data[60 * 200 + 78][1];
        assert!(inside_edge < 0.85, "Kante weich: {inside_edge}");
        assert!(outside_edge > 0.45, "Vordergrund greift über: {outside_edge}");
    }

    #[test]
    fn no_sharp_seam_where_the_depth_passes_through_the_focus() {
        // Near square on far background, with the depth map's soft transition: the pixels in
        // between have the focus depth, but must not stay sharp.
        let (w, h) = (200, 120);
        let depth = DepthMap {
            width: w,
            height: h,
            data: (0..w * h)
                .map(|i| {
                    let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                    let dist = (79.5 - x).max(x - 120.5).max(39.5 - y).max(y - 80.5);
                    (0.5 - dist / 4.0).clamp(0.0, 1.0)
                })
                .collect(),
        };
        let p = LensBlur { amount: 100.0, focus: 0.5, range: 0.1, bokeh: 0.0 };
        let img = LinearImage::new(w, h, vec![[0.5; 3]; w * h]);
        let snapped = Snapped::new(&depth, &img);
        let rad: Vec<f32> = (0..w * h).map(|i| radius(&p, snapped.sample(((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32, img.data[i]), 5.0)).collect();
        // Every pixel across the square's edge is blurred, in front or behind.
        for x in 72..88 {
            assert!(rad[60 * w + x].abs() > 2.0, "x {x}: {}", rad[60 * w + x]);
        }
    }

    #[test]
    fn edges_follow_the_image_not_the_coarser_map() {
        // A diagonal edge: bright and near above it, dark and far below; the map has a quarter
        // of the resolution (its cells would show as steps of 4 pixels).
        let (w, n) = (80usize, 20usize);
        let img = LinearImage::new(w, w, (0..w * w).map(|i| if i % w > i / w { [0.9; 3] } else { [0.1; 3] }).collect());
        let depth = DepthMap { width: n, height: n, data: (0..n * n).map(|i| if i % n > i / n { 1.0 } else { 0.0 }).collect() };
        let snapped = Snapped::new(&depth, &img);
        for y in 0..w {
            for x in (0..w).filter(|x| x.abs_diff(y) > 1) {
                let d = snapped.sample((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / w as f32, img.data[y * w + x]);
                assert_eq!(d > 0.5, x > y, "({x}, {y}): {d}");
            }
        }
    }

    #[test]
    fn nothing_without_amount_and_no_brightening_beyond_clip() {
        let (img, depth) = scene();
        assert_eq!(apply(&img, &depth, &LensBlur::default(), true).data, img.data);
        let p = LensBlur { amount: 100.0, focus: 0.0, range: 0.1, bokeh: 100.0 };
        let out = apply(&img, &depth, &p, true);
        assert!(out.data.iter().all(|v| v.iter().all(|c| *c <= 1.0)));
        // The bright square got brighter as it blurred (a disc of light).
        let plain = apply(&img, &depth, &LensBlur { bokeh: 0.0, ..p }, true);
        assert!(out.data[60 * 200 + 100][1] > plain.data[60 * 200 + 100][1]);
    }
}
