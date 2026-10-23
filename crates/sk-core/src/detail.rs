//! Neighborhood filters on single-channel planes: box/gaussian blur and the
//! (self-guided) guided filter. All run in O(n), independent of the radius.

use rayon::prelude::*;

/// A single-channel float image.
#[derive(Clone, Debug)]
pub struct Plane {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Plane {
    pub fn new(w: usize, h: usize, data: Vec<f32>) -> Self {
        debug_assert_eq!(w * h, data.len());
        Self { w, h, data }
    }

    pub fn map(&self, f: impl Fn(f32) -> f32 + Sync) -> Plane {
        Plane::new(self.w, self.h, self.data.par_iter().map(|&v| f(v)).collect())
    }

    pub fn zip(&self, other: &Plane, f: impl Fn(f32, f32) -> f32 + Sync) -> Plane {
        Plane::new(self.w, self.h, self.data.par_iter().zip(&other.data).map(|(&a, &b)| f(a, b)).collect())
    }
}

/// Separable median: a running median of 2r+1 values along each row, then along each column
/// of that (edges clamped). Keeps lines and fine texture that a mean would spread out, while
/// removing isolated outliers. O(n · r).
pub fn median_hv(p: &Plane, r: usize) -> Plane {
    if r == 0 || p.w == 0 || p.h == 0 {
        return p.clone();
    }
    let (w, h) = (p.w, p.h);
    let mut rows = vec![0.0f32; w * h];
    rows.par_chunks_mut(w).zip(p.data.par_chunks(w)).for_each(|(out, row)| median_line(row, out, r));
    // Columns: gathered into the lines of a transposed plane, filtered there, written back
    // (both in parallel).
    let mut cols = vec![0.0f32; w * h];
    cols.par_chunks_mut(h).enumerate().for_each_init(
        || vec![0.0f32; h],
        |col, (x, out)| {
            col.iter_mut().enumerate().for_each(|(y, v)| *v = rows[y * w + x]);
            median_line(col, out, r);
        },
    );
    let mut data = vec![0.0f32; w * h];
    data.par_chunks_mut(w).enumerate().for_each(|(y, row)| row.iter_mut().enumerate().for_each(|(x, v)| *v = cols[x * h + y]));
    Plane::new(w, h, data)
}

/// Running median of `src` over 2r+1 values (edges clamped) into `out`: a sorted window, one
/// value leaves and one enters per step.
fn median_line(src: &[f32], out: &mut [f32], r: usize) {
    let n = src.len();
    let at = |i: isize| src[i.clamp(0, n as isize - 1) as usize];
    let pos = |win: &[f32], v: f32| win.partition_point(|x| x.total_cmp(&v).is_lt());
    let mut win: Vec<f32> = (-(r as isize)..=r as isize).map(at).collect();
    win.sort_by(f32::total_cmp);
    for (i, o) in out.iter_mut().enumerate() {
        *o = win[r];
        let (leave, enter) = (at(i as isize - r as isize), at(i as isize + r as isize + 1));
        // One shift instead of a removal and an insertion: the values between the leaving
        // one (at k) and where the entering one belongs (j) move by one.
        let (k, j) = (pos(&win, leave), pos(&win, enter));
        if j <= k {
            win.copy_within(j..k, j + 1);
            win[j] = enter;
        } else {
            win.copy_within(k + 1..j, k);
            win[j - 1] = enter;
        }
    }
}

/// Raw pointer that may be shared across threads for writes to disjoint ranges.
#[derive(Clone, Copy)]
struct SyncPtr(*mut f32);
unsafe impl Send for SyncPtr {}
unsafe impl Sync for SyncPtr {}

impl SyncPtr {
    // A method (instead of field access) makes closures capture the whole Sync wrapper.
    fn get(self) -> *mut f32 {
        self.0
    }
}

/// Box blur (mean over a (2r+1)^2 window, edges clamped).
pub fn box_blur(p: &Plane, r: usize) -> Plane {
    if r == 0 || p.w == 0 || p.h == 0 {
        return p.clone();
    }
    let mut data = p.data.clone();
    let mut tmp = vec![0.0f32; data.len()];
    box_blur_in_place(&mut data, &mut tmp, p.w, p.h, r);
    Plane::new(p.w, p.h, data)
}

/// Box blur of `data` in place; `tmp` is scratch space of the same size. (The source is
/// only read by the horizontal pass, so the vertical pass can write over it.)
fn box_blur_in_place(data: &mut [f32], tmp: &mut [f32], w: usize, h: usize, r: usize) {
    // Horizontal pass.
    tmp.par_chunks_mut(w).zip(data.par_chunks(w)).for_each(|(out, row)| box_line(row, out, r));
    let tmp = &*tmp;
    // Vertical pass: running sums over rows, on column stripes (cache friendly).
    let ptr = SyncPtr(data.as_mut_ptr());
    const STRIPE: usize = 256;
    let norm = 1.0 / (2 * r + 1) as f32;
    (0..w.div_ceil(STRIPE)).into_par_iter().for_each(|s| {
        let x0 = s * STRIPE;
        let sw = STRIPE.min(w - x0);
        let row = |y: isize| &tmp[(y.clamp(0, h as isize - 1) as usize) * w + x0..][..sw];
        let mut sum = vec![0.0f32; sw];
        for y in -(r as isize)..=r as isize {
            for (acc, v) in sum.iter_mut().zip(row(y)) {
                *acc += v;
            }
        }
        for y in 0..h {
            // SAFETY: each stripe writes only its own columns [x0, x0 + sw) of every row.
            let dst = unsafe { std::slice::from_raw_parts_mut(ptr.get().add(y * w + x0), sw) };
            for (d, acc) in dst.iter_mut().zip(&sum) {
                *d = acc * norm;
            }
            let (add, sub) = (row(y as isize + r as isize + 1), row(y as isize - r as isize));
            for i in 0..sw {
                sum[i] += add[i] - sub[i];
            }
        }
    });
}

/// 1-D running mean with clamped edges.
fn box_line(src: &[f32], dst: &mut [f32], r: usize) {
    let n = src.len();
    let at = |i: isize| src[i.clamp(0, n as isize - 1) as usize];
    let norm = 1.0 / (2 * r + 1) as f32;
    let mut sum: f32 = (-(r as isize)..=r as isize).map(at).sum();
    for (i, d) in dst.iter_mut().enumerate() {
        *d = sum * norm;
        let i = i as isize;
        sum += at(i + r as isize + 1) - at(i - r as isize);
    }
}

/// Radius of the kernel of [`gaussian_blur_exact`].
pub fn exact_radius(sigma: f32) -> usize {
    (sigma * 3.0).ceil().max(1.0) as usize
}

/// Gaussian blur with sampled weights, radius 3 sigma, clamped edges: exact also for small
/// sigma, where [`gaussian_blur`] can't go below about 1.4 (one box pass of radius 1, three
/// times) and leaves the plane as it is below 0.3. Fades smoothly to no blur at all.
pub fn gaussian_blur_exact(p: &Plane, sigma: f32) -> Plane {
    if p.w == 0 || p.h == 0 || sigma <= 0.0 {
        return p.clone();
    }
    let r = exact_radius(sigma) as isize;
    let weights: Vec<f32> = (-r..=r).map(|k| (-((k * k) as f32) / (2.0 * sigma * sigma)).exp()).collect();
    let norm: f32 = weights.iter().sum();
    let weights: Vec<f32> = weights.iter().map(|w| w / norm).collect();
    let (w, h) = (p.w as isize, p.h as isize);
    let mut tmp = vec![0.0f32; p.data.len()];
    tmp.par_chunks_mut(p.w).enumerate().for_each(|(y, row)| {
        let src = &p.data[y * p.w..(y + 1) * p.w];
        for (x, o) in row.iter_mut().enumerate() {
            *o = weights.iter().enumerate().map(|(k, wt)| wt * src[(x as isize + k as isize - r).clamp(0, w - 1) as usize]).sum();
        }
    });
    let mut out = vec![0.0f32; p.data.len()];
    out.par_chunks_mut(p.w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            *o = weights.iter().enumerate().map(|(k, wt)| wt * tmp[(y as isize + k as isize - r).clamp(0, h - 1) as usize * p.w + x]).sum();
        }
    });
    Plane::new(p.w, p.h, out)
}

/// Gaussian approximation by three box blurs.
pub fn gaussian_blur(p: &Plane, sigma: f32) -> Plane {
    if sigma < 0.3 {
        return p.clone();
    }
    // Box radius for three passes approximating the given sigma.
    let r = (((12.0 * sigma * sigma / 3.0 + 1.0).sqrt() - 1.0) / 2.0).round().max(1.0) as usize;
    if p.w == 0 || p.h == 0 {
        return p.clone();
    }
    // Three passes over two buffers (instead of two fresh allocations per pass).
    let mut data = p.data.clone();
    let mut tmp = vec![0.0f32; data.len()];
    for _ in 0..3 {
        box_blur_in_place(&mut data, &mut tmp, p.w, p.h, r);
    }
    Plane::new(p.w, p.h, data)
}

/// Self-guided filter (He et al.): edge-preserving smoothing of `p` with radius `r`
/// and regularization `eps` (in squared units of `p`).
pub fn guided_filter(p: &Plane, r: usize, eps: f32) -> Plane {
    if r == 0 {
        return p.clone();
    }
    let mean = box_blur(p, r);
    let sq = box_blur(&p.map(|v| v * v), r);
    let a = sq.zip(&mean, |s, m| {
        let var = (s - m * m).max(0.0);
        var / (var + eps)
    });
    let b = mean.zip(&a, |m, a| m - a * m);
    let (ma, mb) = (box_blur(&a, r), box_blur(&b, r));
    let mut out = vec![0.0f32; p.data.len()];
    out.par_iter_mut().enumerate().for_each(|(i, o)| *o = ma.data[i] * p.data[i] + mb.data[i]);
    Plane::new(p.w, p.h, out)
}

/// Fast guided filter: coefficients are computed on a `s`-times subsampled plane and
/// bilinearly upsampled (He & Sun 2015). Suited for large radii.
pub fn fast_guided_filter(p: &Plane, r: usize, eps: f32, s: usize) -> Plane {
    if s <= 1 || r / s < 2 || p.w < s * 4 || p.h < s * 4 {
        return guided_filter(p, r, eps);
    }
    let (lw, lh) = (p.w / s, p.h / s);
    // Subsample by block averaging.
    let low = Plane::new(
        lw,
        lh,
        (0..lw * lh)
            .into_par_iter()
            .map(|i| {
                let (x, y) = (i % lw * s, i / lw * s);
                let mut acc = 0.0;
                for yy in y..y + s {
                    acc += p.data[yy * p.w + x..yy * p.w + x + s].iter().sum::<f32>();
                }
                acc / (s * s) as f32
            })
            .collect(),
    );
    let lr = r / s;
    let mean = box_blur(&low, lr);
    let sq = box_blur(&low.map(|v| v * v), lr);
    let a = sq.zip(&mean, |q, m| {
        let var = (q - m * m).max(0.0);
        var / (var + eps)
    });
    let b = mean.zip(&a, |m, a| m - a * m);
    let (ma, mb) = (box_blur(&a, lr), box_blur(&b, lr));
    // Upsample coefficients bilinearly and apply to the full-resolution guide.
    let mut out = vec![0.0f32; p.data.len()];
    out.par_chunks_mut(p.w).enumerate().for_each(|(y, line)| {
        let fy = ((y as f32 + 0.5) / s as f32 - 0.5).clamp(0.0, (lh - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        let y1 = (y0 + 1).min(lh - 1);
        for (x, o) in line.iter_mut().enumerate() {
            let fx = ((x as f32 + 0.5) / s as f32 - 0.5).clamp(0.0, (lw - 1) as f32);
            let (x0, tx) = (fx.floor() as usize, fx.fract());
            let x1 = (x0 + 1).min(lw - 1);
            let bil = |pl: &Plane| {
                let top = pl.data[y0 * lw + x0] * (1.0 - tx) + pl.data[y0 * lw + x1] * tx;
                let bot = pl.data[y1 * lw + x0] * (1.0 - tx) + pl.data[y1 * lw + x1] * tx;
                top * (1.0 - ty) + bot * ty
            };
            *o = bil(&ma) * p.data[y * p.w + x] + bil(&mb);
        }
    });
    Plane::new(p.w, p.h, out)
}

/// Deterministic per-coordinate hash noise in [-1, 1].
#[inline]
pub fn hash_noise(x: i64, y: i64, seed: u32) -> f32 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ seed as u64;
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    (h & 0xFFFF) as f32 / 32767.5 - 1.0
}

/// Smooth value noise at continuous coordinates (bilinear between hashed lattice points).
#[inline]
pub fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (tx, ty) = (x - x0, y - y0);
    let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let (xi, yi) = (x0 as i64, y0 as i64);
    let a = hash_noise(xi, yi, seed);
    let b = hash_noise(xi + 1, yi, seed);
    let c = hash_noise(xi, yi + 1, seed);
    let d = hash_noise(xi + 1, yi + 1, seed);
    let top = a + (b - a) * sx;
    let bot = c + (d - c) * sx;
    top + (bot - top) * sy
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact gaussian spreads a point by its sigma, also below a pixel (where the box
    /// approximation is stuck at about 1.4), and fades to no blur.
    #[test]
    fn exact_gaussian_follows_small_sigmas() {
        let mut p = Plane::new(21, 21, vec![0.0; 21 * 21]);
        p.data[10 * 21 + 10] = 1.0;
        let spread = |sigma: f32| {
            let b = gaussian_blur_exact(&p, sigma);
            let row = &b.data[10 * 21..11 * 21];
            let sum: f32 = row.iter().sum();
            (row.iter().enumerate().map(|(x, v)| (x as f32 - 10.0).powi(2) * v).sum::<f32>() / sum).sqrt()
        };
        for sigma in [0.8, 1.5] {
            assert!((spread(sigma) - sigma).abs() < 0.02, "{sigma}: {}", spread(sigma));
        }
        // Sampled, a sigma below a pixel spreads less, but steadily more with the sigma.
        let small: Vec<f32> = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.8].iter().map(|&s| spread(s)).collect();
        assert!(small.windows(2).all(|w| w[1] > w[0]), "{small:?}");
        assert!(small[0] < 0.01);
        assert!((gaussian_blur(&p, 0.5).data[10 * 21 + 10] - gaussian_blur(&p, 1.2).data[10 * 21 + 10]).abs() < 1e-6, "Kastenfilter: gleich");
        let total: f32 = gaussian_blur_exact(&p, 0.8).data.iter().sum();
        assert!((total - 1.0).abs() < 1e-5);
    }

    #[test]
    fn median_hv_matches_a_direct_median() {
        let (w, h) = (23, 17);
        let p = Plane::new(w, h, (0..w * h).map(|i| hash_noise(i as i64, 5, 1) + if i % 7 == 0 { 4.0 } else { 0.0 }).collect());
        let r = 3;
        let med = |v: &mut Vec<f32>| {
            v.sort_by(f32::total_cmp);
            v[v.len() / 2]
        };
        let clamp = |i: isize, n: usize| i.clamp(0, n as isize - 1) as usize;
        let mut rows = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut v: Vec<f32> = (-3..=3).map(|d| p.data[y * w + clamp(x as isize + d, w)]).collect();
                rows[y * w + x] = med(&mut v);
            }
        }
        let got = median_hv(&p, r);
        for y in 0..h {
            for x in 0..w {
                let mut v: Vec<f32> = (-3..=3).map(|d| rows[clamp(y as isize + d, h) * w + x]).collect();
                assert_eq!(got.data[y * w + x], med(&mut v), "{x},{y}");
            }
        }
        // Outliers (every 7th value +4) are gone (away from the edges, which clamping repeats).
        assert!((3..h - 3).all(|y| (3..w - 3).all(|x| got.data[y * w + x] < 2.0)));
    }

    #[test]
    fn box_blur_preserves_constant_and_mean() {
        let p = Plane::new(37, 23, vec![0.5; 37 * 23]);
        assert!(box_blur(&p, 5).data.iter().all(|v| (v - 0.5).abs() < 1e-5));
        let ramp = Plane::new(100, 1, (0..100).map(|i| i as f32).collect());
        let b = box_blur(&ramp, 3);
        assert!((b.data[50] - 50.0).abs() < 1e-3);
    }

    #[test]
    fn box_blur_matches_naive() {
        let (w, h, r) = (70, 9, 2);
        let p = Plane::new(w, h, (0..w * h).map(|i| ((i * 7919) % 101) as f32).collect());
        let b = box_blur(&p, r);
        let at = |x: isize, y: isize| p.data[(y.clamp(0, h as isize - 1) as usize) * w + x.clamp(0, w as isize - 1) as usize];
        for (x, y) in [(0usize, 0usize), (69, 8), (33, 4), (64, 3)] {
            let mut s = 0.0;
            for dy in -(r as isize)..=r as isize {
                for dx in -(r as isize)..=r as isize {
                    s += at(x as isize + dx, y as isize + dy);
                }
            }
            let naive = s / ((2 * r + 1) * (2 * r + 1)) as f32;
            assert!((b.data[y * w + x] - naive).abs() < 1e-3, "({x},{y})");
        }
    }

    #[test]
    fn guided_filter_preserves_edges_and_smooths_noise() {
        let w = 64;
        let p = Plane::new(
            w,
            w,
            (0..w * w).map(|i| if i % w < w / 2 { 0.0 } else { 1.0 } + hash_noise(i as i64, 0, 1) * 0.02).collect(),
        );
        let q = guided_filter(&p, 4, 0.01);
        // Edge stays sharp, noise is reduced in flat areas.
        assert!(q.data[32 * w + 28] < 0.1 && q.data[32 * w + 36] > 0.9);
        let var = |d: &[f32]| {
            let m = d.iter().sum::<f32>() / d.len() as f32;
            d.iter().map(|v| (v - m).powi(2)).sum::<f32>() / d.len() as f32
        };
        let flat_p: Vec<f32> = (10..20).flat_map(|y| (4..20).map(move |x| y * w + x)).map(|i| p.data[i]).collect();
        let flat_q: Vec<f32> = (10..20).flat_map(|y| (4..20).map(move |x| y * w + x)).map(|i| q.data[i]).collect();
        assert!(var(&flat_q) < var(&flat_p) * 0.5);
    }

    #[test]
    fn fast_guided_filter_approximates_guided_filter() {
        let w = 128;
        let p = Plane::new(w, w, (0..w * w).map(|i| ((i % w) as f32 / 16.0).sin() + hash_noise(i as i64, 2, 3) * 0.05).collect());
        let exact = guided_filter(&p, 16, 0.05);
        let fast = fast_guided_filter(&p, 16, 0.05, 4);
        let err = exact.data.iter().zip(&fast.data).map(|(a, b)| (a - b).abs()).sum::<f32>() / exact.data.len() as f32;
        assert!(err < 0.03, "mean abs error {err}");
    }

    #[test]
    fn noise_is_deterministic_and_bounded() {
        assert_eq!(hash_noise(3, 4, 7), hash_noise(3, 4, 7));
        assert!((0..1000).map(|i| hash_noise(i, i * 3, 1)).all(|v| (-1.0..=1.0).contains(&v)));
    }
}
