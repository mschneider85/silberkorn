//! Highlight reconstruction (process version 4). Where one or two channels of the camera data
//! clipped, the image still holds the others: the clipped ones are estimated from them and from
//! the color of the unclipped surroundings, so a sky next to the sun stays blue and skin keeps
//! its tone instead of both turning white, and the brightness keeps rising into the highlight
//! (values above 1, for the highlights slider to bring back). Where all channels clipped there
//! is nothing to go by: those stay for `Processor::to_working`, which makes them white.
//!
//! The color of the surroundings is gathered on a coarse grid and spread into the clipped areas
//! (push-pull: averaged down until every cell has some, then filled back up, so each takes it
//! from its nearest surroundings), as the share of each channel in the white balanced sum, the
//! bright pixels weighing most. Works on the source at any size.

use std::sync::{Arc, Mutex};

use rayon::prelude::*;

use crate::image::LinearImage;

/// Where clipping begins (camera data scaled to 1 at the white level).
pub const CLIP: f32 = 0.95;
/// Below this in every channel a pixel tells the color of its surroundings.
const RELIABLE: f32 = 0.9;
/// Grid cells along the long edge.
const GRID: usize = 384;

/// Weight (0..1) of how far a channel clipped.
#[inline]
pub fn clipped(v: f32) -> f32 {
    let t = ((v - CLIP) / (1.0 - CLIP)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Reconstructed sources by (source, white balance), so that renderings of one source share one.
type Key = (usize, usize, usize, u64, [u32; 3]);
static CACHE: Mutex<Vec<(Key, Option<Arc<LinearImage>>)>> = Mutex::new(Vec::new());
const KEEP: usize = 3;

/// [`reconstruct`], remembered for the last few sources.
pub fn reconstructed(src: &LinearImage, wb: [f32; 3]) -> Option<Arc<LinearImage>> {
    let key: Key = (src.data.as_ptr() as usize, src.width, src.height, crate::ai_products::fingerprint(src), wb.map(f32::to_bits));
    if let Some((_, img)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, _)| *k == key) {
        return img.clone();
    }
    let img = reconstruct(src, wb).map(Arc::new);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.push((key, img.clone()));
    if cache.len() > KEEP {
        cache.remove(0);
    }
    img
}

/// The source (camera RGB) with its partly clipped highlights reconstructed; `None` if nothing
/// clipped or there are no unclipped surroundings to go by. `wb`: the multipliers that make
/// the camera data neutral.
pub fn reconstruct(src: &LinearImage, wb: [f32; 3]) -> Option<LinearImage> {
    let (w, h) = (src.width, src.height);
    if w == 0 || h == 0 || !src.data.par_iter().any(|p| p.iter().any(|&v| v > CLIP)) {
        return None;
    }
    let cell = w.max(h).div_ceil(GRID).max(1);
    let (gw, gh) = (w.div_ceil(cell), h.div_ceil(cell));
    // Per cell: the colors of its bright unclipped pixels (cells without any take them from the
    // nearest that have, see `push_pull`).
    let (num, den): (Vec<[f32; 3]>, Vec<f32>) = (0..gw * gh)
        .into_par_iter()
        .map(|c| {
            let (cx, cy) = (c % gw, c / gw);
            let (mut num, mut den) = ([0.0f32; 3], 0.0f32);
            for y in cy * cell..((cy + 1) * cell).min(h) {
                for p in &src.data[y * w + cx * cell..y * w + ((cx + 1) * cell).min(w)] {
                    if p.iter().all(|&v| v < RELIABLE) {
                        let b = [p[0] * wb[0], p[1] * wb[1], p[2] * wb[2]];
                        let s = b[0] + b[1] + b[2];
                        if s > 1e-4 {
                            // The brighter, the more like the highlight next to it.
                            let wt = s * s;
                            num = [0, 1, 2].map(|i| num[i] + b[i] / s * wt);
                            den += wt;
                        }
                    }
                }
            }
            (num, den)
        })
        .unzip();
    let share = push_pull(num, den, gw, gh)?;

    let sample = |x: usize, y: usize| -> [f32; 3] {
        let fx = ((x as f32 + 0.5) / cell as f32 - 0.5).clamp(0.0, (gw - 1) as f32);
        let fy = ((y as f32 + 0.5) / cell as f32 - 0.5).clamp(0.0, (gh - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(gw - 1), (y0 + 1).min(gh - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let at = |x: usize, y: usize| share[y * gw + x];
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        [0, 1, 2].map(|i| (a[i] + (b[i] - a[i]) * tx) * (1.0 - ty) + (c[i] + (d[i] - c[i]) * tx) * ty)
    };
    let mut out = src.clone();
    out.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, p) in row.iter_mut().enumerate() {
            let t = p.map(clipped);
            if t.iter().all(|&t| t == 0.0) {
                continue;
            }
            // How much the unclipped channels can tell (nothing when all clipped).
            let known = [0, 1, 2].map(|i| 1.0 - t[i]);
            let k: f32 = known.iter().sum();
            let trust = ((k - 0.1) / 0.7).clamp(0.0, 1.0);
            if trust == 0.0 {
                continue;
            }
            let n = sample(x, y);
            let b = [p[0] * wb[0], p[1] * wb[1], p[2] * wb[2]];
            let seen: f32 = (0..3).map(|i| known[i] * n[i]).sum();
            if seen < 1e-4 {
                continue;
            }
            let sum = (0..3).map(|i| known[i] * b[i]).sum::<f32>() / seen;
            // A clipped channel was at least as bright as where it clipped: only ever raised.
            *p = [0, 1, 2].map(|i| (b[i] + ((n[i] * sum).max(b[i]) - b[i]) * t[i] * trust) / wb[i]);
        }
    });
    Some(out)
}

/// Fills a grid of weighted sums (`num` / `den`) everywhere from where it has data: halved
/// until every cell has some, then interpolated back up into the cells without. `None` if
/// there is no data at all.
fn push_pull(num: Vec<[f32; 3]>, den: Vec<f32>, w: usize, h: usize) -> Option<Vec<[f32; 3]>> {
    let total: f32 = den.iter().sum();
    if total <= 0.0 {
        return None;
    }
    // A cell is trusted fully from a quarter of the average weight of the cells with data.
    let with = den.iter().filter(|&&d| d > 0.0).count().max(1);
    let full = 0.25 * total / with as f32;
    let mut levels = vec![(num, den, w, h)];
    while {
        let (_, d, w, h) = levels.last().unwrap();
        (*w > 1 || *h > 1) && d.iter().any(|&v| v <= 0.0)
    } {
        let (n, d, w, h) = levels.last().unwrap();
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut cn = vec![[0.0f32; 3]; cw * ch];
        let mut cd = vec![0.0f32; cw * ch];
        for y in 0..*h {
            for x in 0..*w {
                let (i, j) = (y * w + x, (y / 2) * cw + x / 2);
                cn[j] = [0, 1, 2].map(|c| cn[j][c] + n[i][c]);
                cd[j] += d[i];
            }
        }
        levels.push((cn, cd, cw, ch));
    }
    let (n, d, _, _) = levels.pop().unwrap();
    let mut filled: Vec<[f32; 3]> = n.iter().zip(&d).map(|(n, &d)| if d > 0.0 { n.map(|v| v / d) } else { [1.0 / 3.0; 3] }).collect();
    while let Some((n, d, w, h)) = levels.pop() {
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let coarse = &filled;
        filled = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let fx = ((x as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (cw - 1) as f32);
                let fy = ((y as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (ch - 1) as f32);
                let (x0, y0) = (fx as usize, fy as usize);
                let (x1, y1) = ((x0 + 1).min(cw - 1), (y0 + 1).min(ch - 1));
                let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
                let at = |x: usize, y: usize| coarse[y * cw + x];
                let (a, b, c, e) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
                let up = [0, 1, 2].map(|k| (a[k] + (b[k] - a[k]) * tx) * (1.0 - ty) + (c[k] + (e[k] - c[k]) * tx) * ty);
                let alpha = (d[i] / full).min(1.0);
                if d[i] > 0.0 { [0, 1, 2].map(|k| n[i][k] / d[i] * alpha + up[k] * (1.0 - alpha)) } else { up }
            })
            .collect();
    }
    Some(filled)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A blue sky whose blue channel clipped around the sun: the clipped part is blue again and
    /// brighter than where it clipped; where everything clipped nothing changes.
    #[test]
    fn a_clipped_blue_sky_stays_blue() {
        let (w, h) = (120, 80);
        // White balanced sky: blue twice red, green in between; brightness rising to the right.
        let wb = [2.0, 1.0, 1.5];
        let sky = |x: usize| {
            let s = 0.3 + 2.2 * x as f32 / w as f32;
            [0.25 * s / wb[0], 0.4 * s / wb[1], 0.8 * s / wb[2]]
        };
        let data: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let x = i % w;
                if x >= 110 { [1.0; 3] } else { sky(x).map(|v| v.min(1.0)) }
            })
            .collect();
        let src = LinearImage::new(w, h, data);
        let out = reconstruct(&src, wb).unwrap();
        // Where only blue clipped, blue comes back in its share.
        let x = 100;
        let (p, truth) = (out.data[40 * w + x], sky(x));
        assert!(src.data[40 * w + x][2] >= 0.999 && truth[2] > 1.0, "Testaufbau");
        assert!((p[2] - truth[2]).abs() < 0.05 * truth[2], "{p:?} {truth:?}");
        assert_eq!((p[0], p[1]), (src.data[40 * w + x][0], src.data[40 * w + x][1]), "Ungeclippte Kanäle bleiben");
        // Fully clipped and unclipped pixels stay as they were.
        assert_eq!(out.data[40 * w + 115], [1.0; 3]);
        assert_eq!(out.data[40 * w + 20], src.data[40 * w + 20]);
        // Nothing clipped: nothing to do.
        assert!(reconstruct(&LinearImage::new(4, 4, vec![[0.5; 3]; 16]), wb).is_none());
    }

    #[test]
    fn push_pull_fills_holes_from_the_nearest_data() {
        let (w, h) = (9, 1);
        let mut num = vec![[0.0f32; 3]; w];
        let mut den = vec![0.0f32; w];
        (num[0], den[0]) = ([1.0, 0.0, 0.0], 1.0);
        (num[8], den[8]) = ([0.0, 0.0, 2.0], 2.0);
        let f = push_pull(num, den, w, h).unwrap();
        assert!((f[0][0] - 1.0).abs() < 1e-5 && (f[8][2] - 1.0).abs() < 1e-5);
        assert!(f[1][0] > f[7][0] && f[7][2] > f[1][2], "{f:?}");
        assert!(f.iter().all(|p| (p.iter().sum::<f32>() - 1.0).abs() < 1e-4));
        assert!(push_pull(vec![[0.0; 3]; 4], vec![0.0; 4], 2, 2).is_none());
    }
}
