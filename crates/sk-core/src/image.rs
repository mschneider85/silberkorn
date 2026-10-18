//! Simple float RGB image buffer and geometric helpers.

use rayon::prelude::*;

#[derive(Clone, Debug, Default)]
pub struct LinearImage {
    pub width: usize,
    pub height: usize,
    pub data: Vec<[f32; 3]>,
}

impl LinearImage {
    pub fn new(width: usize, height: usize, data: Vec<[f32; 3]>) -> Self {
        debug_assert_eq!(width * height, data.len());
        Self { width, height, data }
    }

    /// Averages `factor x factor` blocks.
    pub fn box_downscale(self, factor: usize) -> Self {
        if factor <= 1 {
            return self;
        }
        self.box_downscaled(factor)
    }

    /// Like [`Self::box_downscale`], but leaves `self` alone (no copy of a large source needed).
    pub fn box_downscaled(&self, factor: usize) -> Self {
        // Never below one pixel: a narrow strip (e.g. a panorama) keeps its short side.
        let factor = factor.min(self.width).min(self.height);
        if factor <= 1 {
            return self.clone();
        }
        let (ow, oh) = (self.width / factor, self.height / factor);
        let inv = 1.0 / (factor * factor) as f32;
        let mut out = vec![[0.0f32; 3]; ow * oh];
        out.par_chunks_mut(ow).enumerate().for_each(|(oy, line)| {
            for (ox, px) in line.iter_mut().enumerate() {
                let mut acc = [0.0f32; 3];
                for y in oy * factor..(oy + 1) * factor {
                    let row = &self.data[y * self.width + ox * factor..y * self.width + (ox + 1) * factor];
                    for p in row {
                        acc[0] += p[0];
                        acc[1] += p[1];
                        acc[2] += p[2];
                    }
                }
                *px = [acc[0] * inv, acc[1] * inv, acc[2] * inv];
            }
        });
        Self::new(ow, oh, out)
    }

    /// Area-averaging downscale so that the long edge is at most `max_edge` (fractional factors).
    /// Like [`Self::fit_within`] without taking the image: reduces first, so that a full-size
    /// image is never copied just to make a small version of it.
    pub fn fitted(&self, max_edge: usize) -> Self {
        let long = self.width.max(self.height);
        if long <= max_edge || max_edge == 0 {
            return self.clone();
        }
        let pre = long / max_edge;
        if pre >= 2 { self.box_downscaled(pre).fit_within(max_edge) } else { self.clone().fit_within(max_edge) }
    }

    pub fn fit_within(self, max_edge: usize) -> Self {
        let long = self.width.max(self.height);
        if long <= max_edge || max_edge == 0 {
            return self;
        }
        // Integer pre-reduction keeps the fractional pass cheap.
        let pre = long / max_edge;
        let img = if pre >= 2 { self.box_downscale(pre) } else { self };
        let k = max_edge as f32 / img.width.max(img.height) as f32;
        let (ow, oh) = (((img.width as f32 * k).round() as usize).max(1), ((img.height as f32 * k).round() as usize).max(1));
        img.resize_area(ow, oh)
    }

    /// Separable area-averaging resample to `ow` x `oh` (downscaling only).
    pub fn resize_area(&self, ow: usize, oh: usize) -> Self {
        // Weights for one axis: output i covers source interval [i*s, (i+1)*s).
        fn taps(src: usize, dst: usize) -> Vec<Vec<(usize, f32)>> {
            let s = src as f32 / dst as f32;
            (0..dst)
                .map(|i| {
                    let (a, b) = (i as f32 * s, (i + 1) as f32 * s);
                    let mut t = Vec::new();
                    let mut x = a.floor() as usize;
                    while (x as f32) < b && x < src {
                        let w = (b.min(x as f32 + 1.0) - a.max(x as f32)).max(0.0);
                        if w > 0.0 {
                            t.push((x, w / s));
                        }
                        x += 1;
                    }
                    t
                })
                .collect()
        }
        let (tx, ty) = (taps(self.width, ow), taps(self.height, oh));
        // Horizontal pass.
        let mut tmp = vec![[0.0f32; 3]; ow * self.height];
        tmp.par_chunks_mut(ow).enumerate().for_each(|(y, line)| {
            let row = &self.data[y * self.width..(y + 1) * self.width];
            for (x, px) in line.iter_mut().enumerate() {
                let mut acc = [0.0f32; 3];
                for &(sx, w) in &tx[x] {
                    let p = row[sx];
                    acc = [acc[0] + p[0] * w, acc[1] + p[1] * w, acc[2] + p[2] * w];
                }
                *px = acc;
            }
        });
        // Vertical pass.
        let mut out = vec![[0.0f32; 3]; ow * oh];
        out.par_chunks_mut(ow).enumerate().for_each(|(y, line)| {
            for &(sy, w) in &ty[y] {
                let row = &tmp[sy * ow..(sy + 1) * ow];
                for (px, p) in line.iter_mut().zip(row) {
                    *px = [px[0] + p[0] * w, px[1] + p[1] * w, px[2] + p[2] * w];
                }
            }
        });
        Self::new(ow, oh, out)
    }

    pub fn oriented(self, o: Orientation) -> Self {
        let (w, h) = (self.width, self.height);
        let swap = o.swaps_dimensions();
        let (ow, oh) = if swap { (h, w) } else { (w, h) };
        if o == Orientation::Normal {
            return self;
        }
        let mut out = vec![[0.0f32; 3]; ow * oh];
        out.par_chunks_mut(ow).enumerate().for_each(|(y, line)| {
            for (x, px) in line.iter_mut().enumerate() {
                let (sx, sy) = o.source_coord(x, y, w, h);
                *px = self.data[sy * w + sx];
            }
        });
        Self::new(ow, oh, out)
    }
}

/// EXIF orientation (1..8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Orientation {
    #[default]
    Normal,
    FlipH,
    Rotate180,
    FlipV,
    Transpose,
    Rotate90,
    Transverse,
    Rotate270,
}

impl Orientation {
    pub fn from_exif(v: u16) -> Self {
        match v {
            2 => Self::FlipH,
            3 => Self::Rotate180,
            4 => Self::FlipV,
            5 => Self::Transpose,
            6 => Self::Rotate90,
            7 => Self::Transverse,
            8 => Self::Rotate270,
            _ => Self::Normal,
        }
    }

    /// Clockwise quarter turns.
    pub fn from_quarter_turns(n: u8) -> Self {
        match n % 4 {
            1 => Self::Rotate90,
            2 => Self::Rotate180,
            3 => Self::Rotate270,
            _ => Self::Normal,
        }
    }

    pub fn swaps_dimensions(self) -> bool {
        matches!(self, Self::Transpose | Self::Rotate90 | Self::Transverse | Self::Rotate270)
    }

    /// Maps an output coordinate to the source coordinate (source size `w` x `h`).
    #[inline]
    pub fn source_coord(self, x: usize, y: usize, w: usize, h: usize) -> (usize, usize) {
        match self {
            Self::Normal => (x, y),
            Self::FlipH => (w - 1 - x, y),
            Self::Rotate180 => (w - 1 - x, h - 1 - y),
            Self::FlipV => (x, h - 1 - y),
            Self::Transpose => (y, x),
            Self::Rotate90 => (y, h - 1 - x),
            Self::Transverse => (w - 1 - y, h - 1 - x),
            Self::Rotate270 => (w - 1 - y, x),
        }
    }

    pub fn apply_dynamic(self, img: ::image::DynamicImage) -> ::image::DynamicImage {
        match self {
            Self::Normal => img,
            Self::FlipH => img.fliph(),
            Self::Rotate180 => img.rotate180(),
            Self::FlipV => img.flipv(),
            Self::Transpose => img.rotate90().fliph(),
            Self::Rotate90 => img.rotate90(),
            Self::Transverse => img.rotate270().fliph(),
            Self::Rotate270 => img.rotate270(),
        }
    }
}

/// Rotates an interleaved 8-bit (or any `Copy`) pixel buffer by clockwise quarter turns.
pub fn rotate_pixels<T: Copy + Send + Sync + Default>(
    data: &[T],
    w: usize,
    h: usize,
    turns: u8,
) -> (Vec<T>, usize, usize) {
    let o = Orientation::from_quarter_turns(turns);
    if o == Orientation::Normal {
        return (data.to_vec(), w, h);
    }
    let (ow, oh) = if o.swaps_dimensions() { (h, w) } else { (w, h) };
    let mut out = vec![T::default(); ow * oh];
    out.par_chunks_mut(ow).enumerate().for_each(|(y, line)| {
        for (x, px) in line.iter_mut().enumerate() {
            let (sx, sy) = o.source_coord(x, y, w, h);
            *px = data[sy * w + sx];
        }
    });
    (out, ow, oh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn very_narrow_images_can_be_downscaled() {
        let strip = LinearImage::new(60, 40000, vec![[0.5; 3]; 60 * 40000]);
        let small = strip.fit_within(256);
        assert!(small.width >= 1 && small.height <= 256, "{}x{}", small.width, small.height);
        let tiny = LinearImage::new(3, 2, vec![[0.5; 3]; 6]).box_downscaled(4);
        assert_eq!((tiny.width, tiny.height), (1, 1));
    }

    fn img() -> LinearImage {
        // 3x2 image, value = index
        LinearImage::new(3, 2, (0..6).map(|i| [i as f32; 3]).collect())
    }

    #[test]
    fn rotate90_clockwise() {
        // 0 1 2      3 0
        // 3 4 5  ->  4 1
        //            5 2
        let r = img().oriented(Orientation::Rotate90);
        assert_eq!((r.width, r.height), (2, 3));
        let v: Vec<f32> = r.data.iter().map(|p| p[0]).collect();
        assert_eq!(v, vec![3.0, 0.0, 4.0, 1.0, 5.0, 2.0]);
    }

    #[test]
    fn rotate270_inverts_rotate90() {
        let r = img().oriented(Orientation::Rotate90).oriented(Orientation::Rotate270);
        assert_eq!(r.data, img().data);
    }

    #[test]
    fn resize_area_preserves_mean() {
        let img = LinearImage::new(10, 7, (0..70).map(|i| [i as f32; 3]).collect());
        let mean = |i: &LinearImage| i.data.iter().map(|p| p[0]).sum::<f32>() / i.data.len() as f32;
        let r = img.resize_area(4, 3);
        assert_eq!((r.width, r.height), (4, 3));
        assert!((mean(&r) - mean(&img)).abs() < 1e-3);
        let f = img.clone().fit_within(5);
        assert_eq!(f.width.max(f.height), 5);
    }

    #[test]
    fn downscale_averages() {
        let i = LinearImage::new(2, 2, vec![[0.0; 3], [1.0; 3], [2.0; 3], [3.0; 3]]).box_downscale(2);
        assert_eq!(i.data, vec![[1.5; 3]]);
    }

    #[test]
    fn fitted_equals_fit_within() {
        let img = LinearImage::new(130, 70, (0..130 * 70).map(|i| [(i % 13) as f32, (i % 7) as f32, (i % 5) as f32]).collect());
        for edge in [200, 130, 60, 31, 9] {
            let a = img.fitted(edge);
            let b = img.clone().fit_within(edge);
            assert_eq!((a.width, a.height, &a.data), (b.width, b.height, &b.data), "Kante {edge}");
        }
    }
}