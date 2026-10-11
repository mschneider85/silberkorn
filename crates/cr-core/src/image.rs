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
    fn downscale_averages() {
        let i = LinearImage::new(2, 2, vec![[0.0; 3], [1.0; 3], [2.0; 3], [3.0; 3]]).box_downscale(2);
        assert_eq!(i.data, vec![[1.5; 3]]);
    }
}
