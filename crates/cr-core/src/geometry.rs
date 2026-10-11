//! Geometric transform: crop, straighten, flip, quarter turns, keystone and lens
//! correction (distortion, lateral chromatic aberration, vignetting).
//!
//! Everything is expressed as an inverse mapping from output pixel to source pixel,
//! so a single resampling pass applies the whole chain at any resolution.

use rayon::prelude::*;

use crate::image::LinearImage;
use crate::params::{CropRect, Geometry, LensCorrection};

/// Output region in output pixels (at the scale of the source being rendered).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Region {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

#[derive(Clone, Debug)]
pub struct GeoMap {
    src_w: f32,
    src_h: f32,
    frame_w: f32,
    frame_h: f32,
    crop: CropRect,
    cos: f32,
    sin: f32,
    fill: f32,
    turns: u8,
    flip_h: bool,
    flip_v: bool,
    kv: f32,
    kh: f32,
    /// Normalization radius for distortion/TCA (half of the shorter source side).
    norm: f32,
    half_diag: f32,
    k1: f32,
    ptlens: Option<(f32, f32, f32)>,
    poly5: Option<(f32, f32)>,
    lens_scale: f32,
    ca: [f32; 3],
    vig: [f32; 3],
    identity_lens: bool,
    pub out_w: usize,
    pub out_h: usize,
}

impl GeoMap {
    pub fn new(geom: &Geometry, lens: &LensCorrection, turns: u8, src_w: usize, src_h: usize, ignore_crop: bool) -> Self {
        let turns = turns % 4;
        let (src_w, src_h) = (src_w as f32, src_h as f32);
        let (frame_w, frame_h) = if turns % 2 == 1 { (src_h, src_w) } else { (src_w, src_h) };
        let theta = geom.straighten.clamp(-45.0, 45.0).to_radians();
        let (sin, cos) = theta.sin_cos();
        let (a, b) = (frame_w / 2.0, frame_h / 2.0);
        // Zoom so that the rotated image still covers the whole frame.
        let fill = cos + (a / b).max(b / a) * sin.abs();
        let crop = if ignore_crop { CropRect::default() } else { geom.crop.clamped() };

        let coeffs = lens.profile_coeffs.unwrap_or_default();
        let k1 = -lens.distortion / 100.0 * 0.15;
        let ptlens = (coeffs.a != 0.0 || coeffs.b != 0.0 || coeffs.c != 0.0).then_some((coeffs.a, coeffs.b, coeffs.c));
        let poly5 = (coeffs.poly5_k1 != 0.0 || coeffs.poly5_k2 != 0.0).then_some((coeffs.poly5_k1, coeffs.poly5_k2));
        // Scale so that the corners stay inside the source after barrel correction.
        let norm = src_w.min(src_h) / 2.0;
        let half_diag = (src_w * src_w + src_h * src_h).sqrt() / 2.0;
        let r_corner = half_diag / norm;
        let corner = distortion_factor(k1, ptlens, poly5, r_corner * r_corner, r_corner);
        let lens_scale = if corner > 1.0 { 1.0 / corner } else { 1.0 };
        let ca = [1.0 + lens.ca_red / 100.0 * 0.003 + coeffs.tca_red, 1.0, 1.0 + lens.ca_blue / 100.0 * 0.003 + coeffs.tca_blue];
        let v = lens.vignetting / 100.0;
        let vig = [coeffs.vig_k1, coeffs.vig_k2, coeffs.vig_k3];
        let vig = [vig[0] - v, vig[1], vig[2]];
        let identity_lens = k1 == 0.0 && ptlens.is_none() && poly5.is_none() && ca == [1.0; 3];

        let out_w = ((crop.w * frame_w).round() as usize).max(1);
        let out_h = ((crop.h * frame_h).round() as usize).max(1);
        Self {
            src_w,
            src_h,
            frame_w,
            frame_h,
            crop,
            cos,
            sin,
            fill,
            turns,
            flip_h: geom.flip_h,
            flip_v: geom.flip_v,
            kv: geom.keystone_v / 100.0 * 0.6,
            kh: geom.keystone_h / 100.0 * 0.6,
            norm,
            half_diag,
            k1,
            ptlens,
            poly5,
            lens_scale,
            ca,
            vig,
            identity_lens,
            out_w,
            out_h,
        }
    }

    pub fn frame_size(&self) -> (f32, f32) {
        (self.frame_w, self.frame_h)
    }

    /// Maps an output position (continuous, pixel centers at +0.5) to the centered
    /// source position before lens correction.
    #[inline]
    fn to_source_centered(&self, ox: f32, oy: f32) -> (f32, f32) {
        let fx = self.crop.x * self.frame_w + ox - self.frame_w / 2.0;
        let fy = self.crop.y * self.frame_h + oy - self.frame_h / 2.0;
        // Undo straighten (positive angle rotates the image counter-clockwise).
        let mut x = (self.cos * fx - self.sin * fy) / self.fill;
        let mut y = (self.sin * fx + self.cos * fy) / self.fill;
        if self.flip_h {
            x = -x;
        }
        if self.flip_v {
            y = -y;
        }
        // Undo clockwise quarter turns.
        let (sx, sy) = match self.turns {
            1 => (y, -x),
            2 => (-x, -y),
            3 => (-y, x),
            _ => (x, y),
        };
        if self.kv == 0.0 && self.kh == 0.0 {
            return (sx, sy);
        }
        // Keystone: projective transform in normalized coordinates.
        let (hw, hh) = (self.src_w / 2.0, self.src_h / 2.0);
        let (xn, yn) = (sx / hw, sy / hh);
        let d = (1.0 + self.kv * yn + self.kh * xn).max(0.2);
        let s = 1.0 / (1.0 + self.kv.abs().max(self.kh.abs()) * 0.5);
        (xn / d * hw * s, yn / d * hh * s)
    }

    /// Source pixel coordinates per channel (pixel centers at integers) and vignetting gain.
    #[inline]
    pub fn map(&self, ox: f32, oy: f32) -> ([[f32; 2]; 3], f32) {
        let (mut x, mut y) = self.to_source_centered(ox, oy);
        let gain = self.vignetting_gain(x, y);
        if !self.identity_lens {
            let rn = (x * x + y * y).sqrt() / self.norm;
            let f = distortion_factor(self.k1, self.ptlens, self.poly5, rn * rn, rn) * self.lens_scale;
            x *= f;
            y *= f;
        }
        let (cx, cy) = (self.src_w / 2.0 - 0.5, self.src_h / 2.0 - 0.5);
        let p = |k: f32| [x * k + cx, y * k + cy];
        ([p(self.ca[0]), p(self.ca[1]), p(self.ca[2])], gain)
    }

    #[inline]
    fn vignetting_gain(&self, x: f32, y: f32) -> f32 {
        if self.vig == [0.0; 3] {
            return 1.0;
        }
        let r2 = (x * x + y * y) / (self.half_diag * self.half_diag);
        let c = 1.0 + self.vig[0] * r2 + self.vig[1] * r2 * r2 + self.vig[2] * r2 * r2 * r2;
        1.0 / c.max(0.2)
    }

    /// Output position -> source pixel position (green channel), e.g. for color pickers.
    pub fn source_point(&self, ox: f32, oy: f32) -> (f32, f32) {
        let ([_, g, _], _) = self.map(ox, oy);
        (g[0], g[1])
    }

    /// Resamples `src` (camera space) into the output region.
    pub fn resample(&self, src: &LinearImage, region: Region) -> LinearImage {
        let mut out = vec![[0.0f32; 3]; region.w * region.h];
        out.par_chunks_mut(region.w).enumerate().for_each(|(ry, line)| {
            let oy = (region.y + ry) as f32 + 0.5;
            for (rx, px) in line.iter_mut().enumerate() {
                let ox = (region.x + rx) as f32 + 0.5;
                let (pts, gain) = self.map(ox, oy);
                let mut v = [0.0f32; 3];
                if pts[0] == pts[2] {
                    let s = bilinear(src, pts[1][0], pts[1][1]);
                    v = s;
                } else {
                    for c in 0..3 {
                        v[c] = bilinear(src, pts[c][0], pts[c][1])[c];
                    }
                }
                *px = [v[0] * gain, v[1] * gain, v[2] * gain];
            }
        });
        LinearImage::new(region.w, region.h, out)
    }
}

/// Radial distortion factor r_src / r_out.
#[inline]
fn distortion_factor(k1: f32, ptlens: Option<(f32, f32, f32)>, poly5: Option<(f32, f32)>, r2: f32, r: f32) -> f32 {
    let mut f = 1.0 + k1 * r2;
    if let Some((a, b, c)) = ptlens {
        f *= a * r2 * r + b * r2 + c * r + 1.0 - a - b - c;
    }
    if let Some((p1, p2)) = poly5 {
        f *= 1.0 + p1 * r2 + p2 * r2 * r2;
    }
    f
}

#[inline]
fn bilinear(img: &LinearImage, x: f32, y: f32) -> [f32; 3] {
    let (w, h) = (img.width as isize, img.height as isize);
    if x < -0.5 || y < -0.5 || x > w as f32 - 0.5 || y > h as f32 - 0.5 {
        return [0.0; 3];
    }
    let x0 = x.floor();
    let y0 = y.floor();
    let (tx, ty) = (x - x0, y - y0);
    let (xi, yi) = (x0 as isize, y0 as isize);
    let at = |xx: isize, yy: isize| img.data[(yy.clamp(0, h - 1) * w + xx.clamp(0, w - 1)) as usize];
    let (a, b, c, d) = (at(xi, yi), at(xi + 1, yi), at(xi, yi + 1), at(xi + 1, yi + 1));
    let mut o = [0.0f32; 3];
    for i in 0..3 {
        let top = a[i] + (b[i] - a[i]) * tx;
        let bot = c[i] + (d[i] - c[i]) * tx;
        o[i] = top + (bot - top) * ty;
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: usize, h: usize) -> LinearImage {
        LinearImage::new(w, h, (0..w * h).map(|i| [(i % w) as f32, (i / w) as f32, 0.0]).collect())
    }

    fn full(w: usize, h: usize) -> Region {
        Region { x: 0, y: 0, w, h }
    }

    #[test]
    fn identity_is_exact() {
        let src = gradient(8, 6);
        let g = GeoMap::new(&Geometry::default(), &LensCorrection::default(), 0, 8, 6, false);
        assert_eq!((g.out_w, g.out_h), (8, 6));
        assert_eq!(g.resample(&src, full(8, 6)).data, src.data);
    }

    #[test]
    fn quarter_turn_matches_orientation() {
        let src = gradient(8, 6);
        let g = GeoMap::new(&Geometry::default(), &LensCorrection::default(), 1, 8, 6, false);
        assert_eq!((g.out_w, g.out_h), (6, 8));
        let out = g.resample(&src, full(6, 8));
        let expected = src.clone().oriented(crate::image::Orientation::Rotate90);
        assert_eq!(out.data, expected.data);
    }

    #[test]
    fn crop_selects_subregion() {
        let src = gradient(10, 10);
        let geom = Geometry { crop: CropRect { x: 0.2, y: 0.5, w: 0.5, h: 0.3 }, ..Default::default() };
        let g = GeoMap::new(&geom, &LensCorrection::default(), 0, 10, 10, false);
        assert_eq!((g.out_w, g.out_h), (5, 3));
        let out = g.resample(&src, full(5, 3));
        assert_eq!(out.data[0][0], 2.0);
        assert_eq!(out.data[0][1], 5.0);
        // ignore_crop shows the full frame
        assert_eq!(GeoMap::new(&geom, &LensCorrection::default(), 0, 10, 10, true).out_w, 10);
    }

    #[test]
    fn straighten_keeps_frame_filled() {
        let src = LinearImage::new(40, 30, vec![[1.0; 3]; 1200]);
        let geom = Geometry { straighten: 10.0, ..Default::default() };
        let g = GeoMap::new(&geom, &LensCorrection::default(), 0, 40, 30, false);
        let out = g.resample(&src, full(40, 30));
        assert!(out.data.iter().all(|p| p[0] > 0.99), "no black corners");
    }

    #[test]
    fn flip_mirrors() {
        let src = gradient(4, 2);
        let geom = Geometry { flip_h: true, ..Default::default() };
        let out = GeoMap::new(&geom, &LensCorrection::default(), 0, 4, 2, false).resample(&src, full(4, 2));
        assert_eq!(out.data[0][0], 3.0);
    }

    #[test]
    fn barrel_correction_keeps_corners_and_center() {
        let src = LinearImage::new(60, 40, vec![[1.0; 3]; 2400]);
        let lens = LensCorrection { distortion: -100.0, ..Default::default() };
        let g = GeoMap::new(&Geometry::default(), &lens, 0, 60, 40, false);
        let out = g.resample(&src, full(60, 40));
        assert!(out.data.iter().all(|p| p[0] > 0.99), "corners stay inside the source");
        let (x, y) = g.source_point(30.0, 20.0);
        assert!((x - 29.5).abs() < 1e-3 && (y - 19.5).abs() < 1e-3);
    }

    #[test]
    fn vignetting_correction_brightens_corners() {
        let src = LinearImage::new(20, 20, vec![[0.5; 3]; 400]);
        let lens = LensCorrection { vignetting: 50.0, ..Default::default() };
        let out = GeoMap::new(&Geometry::default(), &lens, 0, 20, 20, false).resample(&src, full(20, 20));
        assert!(out.data[0][0] > out.data[210][0]);
    }

    #[test]
    fn region_matches_full_render() {
        let src = gradient(30, 20);
        let geom = Geometry { straighten: 5.0, crop: CropRect { x: 0.1, y: 0.1, w: 0.8, h: 0.8 }, ..Default::default() };
        let g = GeoMap::new(&geom, &LensCorrection::default(), 1, 30, 20, false);
        let fullimg = g.resample(&src, full(g.out_w, g.out_h));
        let part = g.resample(&src, Region { x: 3, y: 4, w: 5, h: 6 });
        for y in 0..6 {
            for x in 0..5 {
                assert_eq!(part.data[y * 5 + x], fullimg.data[(y + 4) * g.out_w + x + 3]);
            }
        }
    }
}
