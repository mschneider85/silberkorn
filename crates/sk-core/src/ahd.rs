//! Adaptive homogeneity-directed demosaic for Bayer sensors (Hirakawa & Parks, 2005).
//!
//! Every pixel is interpolated twice, once along the row and once along the column. Of the two,
//! the one whose neighborhood is more homogeneous (in a perceptual space) is taken. This avoids
//! the zipper edges and maze patterns that simpler methods leave along fine lines.
//!
//! Works in tiles, so the memory stays small and the tiles run in parallel.

use rayon::prelude::*;

/// Tile size (output pixels) and the border each tile reads around it.
const TILE: usize = 256;
const BORDER: usize = 5;

/// A Bayer mosaic: one value per photosite and the color (0 red, 1 green, 2 blue) of the 2×2
/// pattern, indexed `[row % 2][col % 2]`.
pub struct Mosaic<'a> {
    pub data: &'a [f32],
    pub width: usize,
    pub height: usize,
    pub pattern: [[usize; 2]; 2],
}

impl Mosaic<'_> {
    #[inline]
    fn at(&self, x: isize, y: isize) -> f32 {
        self.data[mirror(y, self.height) * self.width + mirror(x, self.width)]
    }

    #[inline]
    fn color(&self, x: isize, y: isize) -> usize {
        self.pattern[y.rem_euclid(2) as usize][x.rem_euclid(2) as usize]
    }
}

/// Index `i` reflected into `0..n` around the edge photosites, so that it keeps its parity (and
/// with it the color of the CFA); clamped where `n` is too small for that.
#[inline]
fn mirror(i: isize, n: usize) -> usize {
    let n = n as isize;
    let i = if i < 0 { -i } else if i >= n { 2 * n - 2 - i } else { i };
    i.clamp(0, n - 1) as usize
}

/// Demosaics `m` into RGB (`width * height` pixels, row by row). `wb` scales the channels while
/// interpolating (color differences are only meaningful on balanced data); the result is in the
/// mosaic's own (unbalanced) scale again.
pub fn demosaic(m: &Mosaic, wb: [f32; 3]) -> Vec<[f32; 3]> {
    let (w, h) = (m.width, m.height);
    let mut out = vec![[0.0f32; 3]; w * h];
    if w == 0 || h == 0 || m.data.len() < w * h {
        return out;
    }
    let wb = wb.map(|v| if v.is_finite() && v > 0.0 { v } else { 1.0 });
    let tiles: Vec<(usize, usize)> = (0..h.div_ceil(TILE)).flat_map(|ty| (0..w.div_ceil(TILE)).map(move |tx| (tx * TILE, ty * TILE))).collect();
    let results: Vec<((usize, usize), Tile)> = tiles.into_par_iter().map(|(x0, y0)| ((x0, y0), tile(m, wb, x0, y0))).collect();
    for ((x0, y0), t) in results {
        for ty in 0..t.h {
            for tx in 0..t.w {
                let v = t.out[ty * t.w + tx];
                out[(y0 + ty) * w + x0 + tx] = [v[0] / wb[0], v[1] / wb[1], v[2] / wb[2]];
            }
        }
    }
    out
}

struct Tile {
    w: usize,
    h: usize,
    out: Vec<[f32; 3]>,
}

/// One tile: output `[x0, x0 + TILE) × [y0, y0 + TILE)`, computed on an area `BORDER` larger.
fn tile(m: &Mosaic, wb: [f32; 3], x0: usize, y0: usize) -> Tile {
    let (ow, oh) = (TILE.min(m.width - x0), TILE.min(m.height - y0));
    let (ex, ey) = (x0 as isize - BORDER as isize, y0 as isize - BORDER as isize);
    let (ew, eh) = (ow + 2 * BORDER, oh + 2 * BORDER);
    let idx = |x: usize, y: usize| y * ew + x;
    // Balanced mosaic of the extended area (edges mirrored).
    let raw: Vec<f32> = (0..eh)
        .flat_map(|y| (0..ew).map(move |x| (x, y)))
        .map(|(x, y)| {
            let (gx, gy) = (ex + x as isize, ey + y as isize);
            m.at(gx, gy) * wb[m.color(gx, gy)]
        })
        .collect();
    let color = |x: usize, y: usize| m.color(ex + x as isize, ey + y as isize);
    let r = |x: usize, y: usize| raw[idx(x, y)];

    // 1. Green along rows and along columns, with a second-order correction from the pixel's
    //    own color, limited to the range of its green neighbors.
    let mut green = [vec![0.0f32; ew * eh], vec![0.0f32; ew * eh]];
    for y in 2..eh - 2 {
        for x in 2..ew - 2 {
            let i = idx(x, y);
            if color(x, y) == 1 {
                green[0][i] = raw[i];
                green[1][i] = raw[i];
                continue;
            }
            let c = raw[i];
            let (l, rr) = (r(x - 1, y), r(x + 1, y));
            let gh = (l + rr) / 2.0 + (2.0 * c - r(x - 2, y) - r(x + 2, y)) / 4.0;
            green[0][i] = gh.clamp(l.min(rr), l.max(rr));
            let (u, d) = (r(x, y - 1), r(x, y + 1));
            let gv = (u + d) / 2.0 + (2.0 * c - r(x, y - 2) - r(x, y + 2)) / 4.0;
            green[1][i] = gv.clamp(u.min(d), u.max(d));
        }
    }

    // 2. Red and blue for each direction from the color differences to its green.
    let mut rgb = [vec![[0.0f32; 3]; ew * eh], vec![[0.0f32; 3]; ew * eh]];
    for d in 0..2 {
        let g = &green[d];
        let diff = |x: usize, y: usize| raw[idx(x, y)] - g[idx(x, y)];
        for y in 3..eh - 3 {
            for x in 3..ew - 3 {
                let i = idx(x, y);
                let own = color(x, y);
                let mut p = [0.0f32; 3];
                p[1] = g[i];
                if own == 1 {
                    // Green photosite: one color left/right, the other above/below.
                    let hc = color(x + 1, y);
                    let vc = color(x, y + 1);
                    p[hc] = g[i] + (diff(x - 1, y) + diff(x + 1, y)) / 2.0;
                    p[vc] = g[i] + (diff(x, y - 1) + diff(x, y + 1)) / 2.0;
                } else {
                    // Red or blue photosite: the other color on the four diagonals.
                    let other = 2 - own;
                    p[own] = raw[i];
                    p[other] = g[i] + (diff(x - 1, y - 1) + diff(x + 1, y - 1) + diff(x - 1, y + 1) + diff(x + 1, y + 1)) / 4.0;
                }
                rgb[d][i] = p.map(|v| v.max(0.0));
            }
        }
    }

    // 3. Both candidates in a perceptual space (cube roots: lightness and two opponent axes).
    let lab = |p: [f32; 3]| {
        let c = p.map(|v| v.max(0.0).cbrt());
        [(c[0] + 2.0 * c[1] + c[2]) / 4.0, c[0] - c[1], c[1] - c[2]]
    };
    let labs: [Vec<[f32; 3]>; 2] = [rgb[0].iter().map(|p| lab(*p)).collect(), rgb[1].iter().map(|p| lab(*p)).collect()];

    // 4. Homogeneity: neighbors within the smaller of the two directions' typical differences.
    let mut homo = [vec![0u8; ew * eh], vec![0u8; ew * eh]];
    for y in 4..eh - 4 {
        for x in 4..ew - 4 {
            let i = idx(x, y);
            let dist = |d: usize, j: usize| {
                let (a, b) = (labs[d][i], labs[d][j]);
                ((a[0] - b[0]).abs(), (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2))
            };
            let (left, right, up, down) = (idx(x - 1, y), idx(x + 1, y), idx(x, y - 1), idx(x, y + 1));
            let (hl, hr) = (dist(0, left), dist(0, right));
            let (vu, vd) = (dist(1, up), dist(1, down));
            let eps_l = hl.0.max(hr.0).min(vu.0.max(vd.0));
            let eps_c = hl.1.max(hr.1).min(vu.1.max(vd.1));
            for (d, h) in homo.iter_mut().enumerate() {
                h[i] = [left, right, up, down].iter().filter(|j| {
                    let (dl, dc) = dist(d, **j);
                    dl <= eps_l && dc <= eps_c
                }).count() as u8;
            }
        }
    }

    // 5. Per pixel the direction more homogeneous over 3×3; the mean where both are equal.
    let mut out = vec![[0.0f32; 3]; ow * oh];
    for ty in 0..oh {
        for tx in 0..ow {
            let (x, y) = (tx + BORDER, ty + BORDER);
            let sum = |d: usize| -> u32 { (y - 1..=y + 1).flat_map(|yy| (x - 1..=x + 1).map(move |xx| (xx, yy))).map(|(xx, yy)| homo[d][idx(xx, yy)] as u32).sum() };
            let (sh, sv) = (sum(0), sum(1));
            let i = idx(x, y);
            out[ty * ow + tx] = match sh.cmp(&sv) {
                std::cmp::Ordering::Greater => rgb[0][i],
                std::cmp::Ordering::Less => rgb[1][i],
                std::cmp::Ordering::Equal => [0, 1, 2].map(|c| (rgb[0][i][c] + rgb[1][i][c]) / 2.0),
            };
        }
    }
    Tile { w: ow, h: oh, out }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RGGB: [[usize; 2]; 2] = [[0, 1], [1, 2]];

    fn mosaic_of(img: &[[f32; 3]], w: usize, h: usize) -> Vec<f32> {
        (0..w * h).map(|i| img[i][RGGB[(i / w) % 2][(i % w) % 2]]).collect()
    }

    fn error(a: &[[f32; 3]], b: &[[f32; 3]], w: usize, h: usize) -> f32 {
        // Without the outermost pixels (mirrored edges).
        let mut sum = 0.0;
        let mut n = 0;
        for y in 3..h - 3 {
            for x in 3..w - 3 {
                for c in 0..3 {
                    sum += (a[y * w + x][c] - b[y * w + x][c]).abs();
                    n += 1;
                }
            }
        }
        sum / n as f32
    }

    #[test]
    fn smooth_images_are_reconstructed_closely() {
        let (w, h) = (300, 270);
        let img: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
                [0.2 + 0.5 * x, 0.3 + 0.3 * (y * 3.0).sin().abs(), 0.6 - 0.4 * x * y]
            })
            .collect();
        let data = mosaic_of(&img, w, h);
        let out = demosaic(&Mosaic { data: &data, width: w, height: h, pattern: RGGB }, [1.0; 3]);
        assert!(error(&out, &img, w, h) < 0.003, "{}", error(&out, &img, w, h));
    }

    #[test]
    fn sharp_edges_have_less_error_than_bilinear() {
        // Gray vertical and horizontal lines one pixel wide, and a diagonal edge.
        let (w, h) = (120, 100);
        let img: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let v = if x % 5 == 0 || y % 7 == 0 { 0.8 } else if x + y > 110 { 0.5 } else { 0.1 };
                [v; 3]
            })
            .collect();
        let data = mosaic_of(&img, w, h);
        let m = Mosaic { data: &data, width: w, height: h, pattern: RGGB };
        let ahd = demosaic(&m, [1.0; 3]);
        // Bilinear: the mean of the same color within 3×3.
        let bilinear: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as isize, (i / w) as isize);
                let mut acc = [0.0f32; 3];
                let mut n = [0.0f32; 3];
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let c = m.color(x + dx, y + dy);
                        acc[c] += m.at(x + dx, y + dy);
                        n[c] += 1.0;
                    }
                }
                [0, 1, 2].map(|c| acc[c] / n[c])
            })
            .collect();
        let (e_ahd, e_bil) = (error(&ahd, &img, w, h), error(&bilinear, &img, w, h));
        assert!(e_ahd < e_bil * 0.7, "AHD {e_ahd}, bilinear {e_bil}");
    }

    /// Mirrored edges keep the CFA colors: a flat color is exact up to the outermost pixels.
    #[test]
    fn flat_color_is_exact_at_the_edges() {
        let (w, h) = (37, 29);
        let img = vec![[0.2f32, 0.4, 0.3]; w * h];
        for pattern in [RGGB, [[1, 0], [2, 1]], [[2, 1], [1, 0]]] {
            let data: Vec<f32> = (0..w * h).map(|i| img[i][pattern[(i / w) % 2][(i % w) % 2]]).collect();
            let out = demosaic(&Mosaic { data: &data, width: w, height: h, pattern }, [1.0; 3]);
            for (i, p) in out.iter().enumerate() {
                assert!((0..3).all(|c| (p[c] - img[i][c]).abs() < 1e-5), "{pattern:?} ({}, {}): {p:?}", i % w, i / w);
            }
        }
        assert_eq!((mirror(-2, 10), mirror(-5, 10), mirror(10, 10), mirror(12, 10), mirror(3, 1)), (2, 5, 8, 6, 0));
    }

    #[test]
    fn white_balance_scales_back() {
        let (w, h) = (40, 40);
        let img = vec![[0.2f32, 0.4, 0.3]; w * h];
        let data = mosaic_of(&img, w, h);
        let out = demosaic(&Mosaic { data: &data, width: w, height: h, pattern: RGGB }, [2.0, 1.0, 1.5]);
        assert!(error(&out, &img, w, h) < 1e-5);
    }
}
