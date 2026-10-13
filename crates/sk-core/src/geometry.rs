//! Geometric transform: crop, straighten, flip, quarter turns, keystone and lens
//! correction (distortion, lateral chromatic aberration, vignetting).
//!
//! Everything is expressed as an inverse mapping from output pixel to source pixel,
//! so a single resampling pass applies the whole chain at any resolution.

use rayon::prelude::*;

use crate::camera_lens::CameraLens;
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
    /// Zoom after the keystone so that the corrected image covers the whole frame (see `keystone_fill`).
    ks: f32,
    /// Fisheye correction (see `fisheye_params`): focal length of the fisheye and of the
    /// rectilinear image in pixels; 0 = off.
    fish_f: f32,
    fish_fr: f32,
    /// Normalization radius for distortion/TCA (half of the shorter source side).
    norm: f32,
    half_diag: f32,
    k1: f32,
    ptlens: Option<(f32, f32, f32)>,
    poly5: Option<(f32, f32)>,
    lens_scale: f32,
    ca: [f32; 3],
    vig: [f32; 3],
    /// The camera's own correction (see `camera_lens`), applied closest to the source.
    camera: Option<CameraLens>,
    identity_lens: bool,
    /// Every output pixel is exactly one source pixel (only quarter turns, flips and a crop
    /// on whole pixels): resampling is a plain copy.
    pixel_exact: bool,
    pub out_w: usize,
    pub out_h: usize,
}

impl GeoMap {
    /// `camera`: the correction stored in the raw file (`RawMeta::lens`), used if `lens.camera` is on.
    pub fn new(geom: &Geometry, lens: &LensCorrection, camera: Option<&CameraLens>, turns: u8, src_w: usize, src_h: usize, ignore_crop: bool) -> Self {
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
        let camera = camera.filter(|c| lens.camera && (c.has_warp() || c.has_vignette())).cloned();
        let r_corner = half_diag / norm;
        let mut corner = distortion_factor(k1, ptlens, poly5, r_corner * r_corner, r_corner);
        if let Some(cam) = camera.as_ref().filter(|c| c.has_warp()) {
            // The farthest the four corners go (the optical center may be off the middle).
            let (hx, hy) = (src_w / 2.0 / half_diag, src_h / 2.0 / half_diag);
            corner = [(hx, hy), (-hx, hy), (hx, -hy), (-hx, -hy)]
                .iter()
                .map(|&(x, y)| {
                    let (x, y) = (x * corner - cam.center[0], y * corner - cam.center[1]);
                    let f = cam.factor(1, x * x + y * y);
                    // Corner position after both corrections, relative to the frame's corner.
                    ((x * f + cam.center[0]) / hx).abs().max(((y * f + cam.center[1]) / hy).abs())
                })
                .fold(0.0, f32::max);
        }
        let lens_scale = if corner > 1.0 { 1.0 / corner } else { 1.0 };
        let ca = [1.0 + lens.ca_red / 100.0 * 0.003 + coeffs.tca_red, 1.0, 1.0 + lens.ca_blue / 100.0 * 0.003 + coeffs.tca_blue];
        let v = lens.vignetting / 100.0;
        let vig = [coeffs.vig_k1, coeffs.vig_k2, coeffs.vig_k3];
        let vig = [vig[0] - v, vig[1], vig[2]];
        let (fish_f, fish_fr) = fisheye_params(lens.fisheye, src_w, src_h);
        let identity_lens = k1 == 0.0 && ptlens.is_none() && poly5.is_none() && ca == [1.0; 3] && camera.as_ref().is_none_or(|c| !c.has_warp());

        // Tolerance for float noise (0.4 * 10.0 is not exactly 4 in f32).
        let whole = |v: f32| (v - v.round()).abs() < 1e-3;
        let pixel_exact = theta == 0.0
            && fish_f == 0.0
            && geom.keystone_v == 0.0
            && geom.keystone_h == 0.0
            && identity_lens
            && vig == [0.0; 3]
            && camera.is_none()
            && whole(crop.x * frame_w)
            && whole(crop.y * frame_h);

        let out_w = ((crop.w * frame_w).round() as usize).max(1);
        let out_h = ((crop.h * frame_h).round() as usize).max(1);
        let mut map = Self {
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
            ks: 1.0,
            fish_f,
            fish_fr,
            norm,
            half_diag,
            k1,
            ptlens,
            poly5,
            lens_scale,
            ca,
            vig,
            camera,
            identity_lens,
            pixel_exact,
            out_w,
            out_h,
        };
        map.ks = map.keystone_fill();
        map
    }

    /// The zoom applied after the keystone correction (1 without one).
    pub fn keystone_zoom(&self) -> f32 {
        self.ks
    }

    /// Zoom for the keystone correction: the largest at which the whole frame still lies on the
    /// image, so that no empty areas appear. The keystone maps the frame (after straighten) to
    /// a quadrilateral; its border is checked, the corners being the farthest points.
    fn keystone_fill(&self) -> f32 {
        if self.kv == 0.0 && self.kh == 0.0 {
            return 1.0;
        }
        let (hw, hh) = (self.frame_w / 2.0, self.frame_h / 2.0);
        let (sw, sh) = (self.src_w / 2.0, self.src_h / 2.0);
        const N: usize = 16;
        let mut k = f32::MAX;
        for i in 0..=N {
            let t = i as f32 / N as f32 * 2.0 - 1.0;
            for (fx, fy) in [(t * hw, -hh), (t * hw, hh), (-hw, t * hh), (hw, t * hh)] {
                let (x, y) = self.frame_to_source(fx, fy, 1.0);
                k = k.min(sw / x.abs().max(1e-6)).min(sh / y.abs().max(1e-6));
            }
        }
        // A hair inside, so that interpolation at the border never reaches past the image.
        k * 0.999
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
        let (x, y) = self.frame_to_source(fx, fy, self.ks);
        self.defish(x, y)
    }

    /// Rectilinear -> fisheye position (centered): the angle from the axis stays, the radius
    /// follows the equidistant projection r = f·θ.
    #[inline]
    fn defish(&self, x: f32, y: f32) -> (f32, f32) {
        if self.fish_f == 0.0 {
            return (x, y);
        }
        let r = (x * x + y * y).sqrt();
        if r < 1e-6 {
            return (x, y);
        }
        let k = self.fish_f * (r / self.fish_fr).atan() / r;
        (x * k, y * k)
    }

    /// Frame position relative to its center -> centered source position (before lens
    /// correction), with `ks` as the keystone zoom.
    #[inline]
    fn frame_to_source(&self, fx: f32, fy: f32, ks: f32) -> (f32, f32) {
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
        (xn / d * hw * ks, yn / d * hh * ks)
    }

    /// Source pixel coordinates per channel (pixel centers at integers) and vignetting gain.
    #[inline]
    pub fn map(&self, ox: f32, oy: f32) -> ([[f32; 2]; 3], f32) {
        let (mut x, mut y) = self.to_source_centered(ox, oy);
        let mut gain = self.vignetting_gain(x, y);
        if !self.identity_lens {
            let rn = (x * x + y * y).sqrt() / self.norm;
            let f = distortion_factor(self.k1, self.ptlens, self.poly5, rn * rn, rn) * self.lens_scale;
            x *= f;
            y *= f;
        }
        let (cx, cy) = (self.src_w / 2.0 - 0.5, self.src_h / 2.0 - 0.5);
        let Some(cam) = &self.camera else {
            let p = |k: f32| [x * k + cx, y * k + cy];
            return ([p(self.ca[0]), p(self.ca[1]), p(self.ca[2])], gain);
        };
        // Around the optical center, in half diagonals.
        let (ux, uy) = (x / self.half_diag - cam.center[0], y / self.half_diag - cam.center[1]);
        let r2 = ux * ux + uy * uy;
        let p = |c: usize| {
            let f = cam.factor(c, r2);
            let k = self.ca[c] * self.half_diag;
            [(ux * f + cam.center[0]) * k + cx, (uy * f + cam.center[1]) * k + cy]
        };
        let pts = [p(0), p(1), p(2)];
        // Vignetting where the light fell on the sensor.
        let (gx, gy) = ((pts[1][0] - cx) / self.half_diag - cam.center[0], (pts[1][1] - cy) / self.half_diag - cam.center[1]);
        gain *= cam.gain(gx * gx + gy * gy);
        (pts, gain)
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

    /// Source pixels per output pixel in the middle of the output: straightening, keystone and
    /// fisheye correction enlarge the image, so a brush of a given size on screen covers less
    /// of the source.
    pub fn source_scale(&self) -> f32 {
        let (cx, cy) = (self.out_w as f32 / 2.0, self.out_h as f32 / 2.0);
        let p = self.source_point(cx, cy);
        let dist = |q: (f32, f32)| ((q.0 - p.0).powi(2) + (q.1 - p.1).powi(2)).sqrt();
        let k = (dist(self.source_point(cx + 1.0, cy)) + dist(self.source_point(cx, cy + 1.0))) / 2.0;
        if k.is_finite() && k > 0.0 { k } else { 1.0 }
    }

    /// Inverse of `source_point`: output position of a source pixel position (Newton iteration).
    pub fn output_point(&self, sx: f32, sy: f32) -> (f32, f32) {
        // Initial guess: proportional position in the output frame.
        let mut p = ((sx + 0.5) / self.src_w * self.out_w as f32, (sy + 0.5) / self.src_h * self.out_h as f32);
        if self.turns % 2 == 1 {
            p = ((sy + 0.5) / self.src_h * self.out_w as f32, (sx + 0.5) / self.src_w * self.out_h as f32);
        }
        for _ in 0..20 {
            let f = self.source_point(p.0, p.1);
            let (ex, ey) = (sx - f.0, sy - f.1);
            if ex * ex + ey * ey < 1e-6 {
                break;
            }
            let h = 0.5;
            let fx = self.source_point(p.0 + h, p.1);
            let fy = self.source_point(p.0, p.1 + h);
            let (a, c) = ((fx.0 - f.0) / h, (fx.1 - f.1) / h);
            let (b, d) = ((fy.0 - f.0) / h, (fy.1 - f.1) / h);
            let det = a * d - b * c;
            if det.abs() < 1e-9 {
                break;
            }
            p.0 += (d * ex - b * ey) / det;
            p.1 += (-c * ex + a * ey) / det;
        }
        p
    }

    /// Resamples `src` (camera space) into the output region.
    pub fn resample(&self, src: &LinearImage, region: Region) -> LinearImage {
        let mut out = vec![[0.0f32; 3]; region.w * region.h];
        if self.pixel_exact {
            let (cx, cy) = (self.src_w / 2.0 - 0.5, self.src_h / 2.0 - 0.5);
            out.par_chunks_mut(region.w).enumerate().for_each(|(ry, line)| {
                let oy = (region.y + ry) as f32 + 0.5;
                for (rx, px) in line.iter_mut().enumerate() {
                    let (x, y) = self.to_source_centered((region.x + rx) as f32 + 0.5, oy);
                    let (sx, sy) = ((x + cx).round(), (y + cy).round());
                    if sx >= 0.0 && sy >= 0.0 && (sx as usize) < src.width && (sy as usize) < src.height {
                        *px = src.data[sy as usize * src.width + sx as usize];
                    }
                }
            });
            return LinearImage::new(region.w, region.h, out);
        }
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

/// Focal lengths (pixels) for the fisheye correction with the given diagonal angle of view
/// (degrees): that of the fisheye, f = half diagonal / half angle, and that of the rectilinear
/// image. The latter is chosen so that the rectilinear image as large as the source lies
/// completely on the fisheye image (no empty areas). The projection only shrinks radii, most
/// at the shortest distance to the border (the middle of the long sides), so that one
/// decides: f·atan(m / fr) = m. Beyond 80° from the axis the rectilinear image would grow
/// without bound; it is cut off there. (0, 0) when off.
fn fisheye_params(fov: f32, src_w: f32, src_h: f32) -> (f32, f32) {
    if fov <= 0.0 {
        return (0.0, 0.0);
    }
    let half_diag = (src_w * src_w + src_h * src_h).sqrt() / 2.0;
    let f = half_diag / (fov.min(360.0) / 2.0).to_radians();
    let m = src_w.min(src_h) / 2.0;
    let fr = m / (m / f).min(80f32.to_radians()).tan();
    (f, fr)
}

/// [`GeoMap`] as laid out in the `Geo` struct of the compute shaders (common.wgsl).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct GeoGpu {
    src_w: f32,
    src_h: f32,
    frame_w: f32,
    frame_h: f32,
    crop_x: f32,
    crop_y: f32,
    cos: f32,
    sin: f32,
    fill: f32,
    kv: f32,
    kh: f32,
    ks: f32,
    fish_f: f32,
    fish_fr: f32,
    norm: f32,
    half_diag: f32,
    k1: f32,
    pt: [f32; 3],
    p5: [f32; 2],
    lens_scale: f32,
    ca_r: f32,
    ca_b: f32,
    vig: [f32; 3],
    turns: u32,
    flip_h: u32,
    flip_v: u32,
    has_pt: u32,
    has_p5: u32,
    identity_lens: u32,
    has_vig: u32,
    pixel_exact: u32,
    has_cam: u32,
    cam_cx: f32,
    cam_cy: f32,
    cam_warp: [f32; 12],
    cam_vig: [f32; 5],
}

impl GeoMap {
    pub(crate) fn gpu(&self) -> GeoGpu {
        let neutral = CameraLens { source: "", center: [0.0; 2], warp: [[1.0, 0.0, 0.0, 0.0]; 3], vignette: [0.0; 5] };
        let cam = self.camera.as_ref().unwrap_or(&neutral);
        let (pt, has_pt) = self.ptlens.map_or(([0.0; 3], 0), |(a, b, c)| ([a, b, c], 1));
        let (p5, has_p5) = self.poly5.map_or(([0.0; 2], 0), |(a, b)| ([a, b], 1));
        GeoGpu {
            src_w: self.src_w,
            src_h: self.src_h,
            frame_w: self.frame_w,
            frame_h: self.frame_h,
            crop_x: self.crop.x,
            crop_y: self.crop.y,
            cos: self.cos,
            sin: self.sin,
            fill: self.fill,
            kv: self.kv,
            kh: self.kh,
            ks: self.ks,
            fish_f: self.fish_f,
            fish_fr: self.fish_fr,
            norm: self.norm,
            half_diag: self.half_diag,
            k1: self.k1,
            pt,
            p5,
            lens_scale: self.lens_scale,
            ca_r: self.ca[0],
            ca_b: self.ca[2],
            vig: self.vig,
            turns: self.turns as u32,
            flip_h: self.flip_h as u32,
            flip_v: self.flip_v as u32,
            has_pt,
            has_p5,
            identity_lens: self.identity_lens as u32,
            has_vig: (self.vig != [0.0; 3]) as u32,
            pixel_exact: self.pixel_exact as u32,
            has_cam: self.camera.is_some() as u32,
            cam_cx: cam.center[0],
            cam_cy: cam.center[1],
            cam_warp: bytemuck::cast(cam.warp),
            cam_vig: cam.vignette,
        }
    }

    /// Source pixels needed to resample `region` (x, y, w, h), so that only that part of a
    /// large source has to go to the graphics card. The mapping is smooth, so a grid over
    /// the region (with all of its border) bounds it; two pixels are added for interpolation.
    pub(crate) fn source_window(&self, region: Region) -> (usize, usize, usize, usize) {
        const N: usize = 32;
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for j in 0..=N {
            for i in 0..=N {
                let ox = region.x as f32 + region.w as f32 * i as f32 / N as f32;
                let oy = region.y as f32 + region.h as f32 * j as f32 / N as f32;
                let (pts, _) = self.map(ox, oy);
                for p in pts {
                    x0 = x0.min(p[0]);
                    y0 = y0.min(p[1]);
                    x1 = x1.max(p[0]);
                    y1 = y1.max(p[1]);
                }
            }
        }
        let clamp = |v: f32, max: f32| v.clamp(0.0, max) as usize;
        let (x0, y0) = (clamp(x0.floor() - 2.0, self.src_w - 1.0), clamp(y0.floor() - 2.0, self.src_h - 1.0));
        let (x1, y1) = (clamp(x1.ceil() + 2.0, self.src_w - 1.0), clamp(y1.ceil() + 2.0, self.src_h - 1.0));
        (x0, y0, x1 - x0 + 1, y1 - y0 + 1)
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

    #[test]
    fn pixel_exact_resampling_matches_bilinear() {
        // Crop offsets on whole pixels in both orientations (10 x 5 and 5 x 10).
        let src = LinearImage::new(10, 5, (0..50).map(|i| [i as f32, (i * 3 % 11) as f32, 1.0 / (i + 1) as f32]).collect());
        for turns in 0..4 {
            for (flip_h, flip_v) in [(false, false), (true, false), (false, true)] {
                let geom = Geometry {
                    flip_h,
                    flip_v,
                    crop: CropRect { x: 0.4, y: 0.2, w: 0.6, h: 0.6 },
                    ..Default::default()
                };
                let map = GeoMap::new(&geom, &LensCorrection::default(), None, turns, 10, 5, false);
                assert!(map.pixel_exact, "Drehung {turns}");
                let region = Region { x: 0, y: 0, w: map.out_w, h: map.out_h };
                let fast = map.resample(&src, region);
                let slow = GeoMap { pixel_exact: false, ..map.clone() }.resample(&src, region);
                let max_diff = fast.data.iter().zip(&slow.data).flat_map(|(a, b)| (0..3).map(move |c| (a[c] - b[c]).abs())).fold(0.0, f32::max);
                assert!(max_diff < 0.05, "Drehung {turns}, Spiegelung {flip_h}/{flip_v}: {max_diff}");
            }
        }
        let tilted = Geometry { straighten: 1.0, ..Default::default() };
        assert!(!GeoMap::new(&tilted, &LensCorrection::default(), None, 0, 10, 5, false).pixel_exact);
    }

    use super::*;

    #[test]
    fn source_scale_follows_straightening() {
        let map = |g: &Geometry| GeoMap::new(g, &LensCorrection::default(), None, 0, 6000, 4000, false);
        assert!((map(&Geometry::default()).source_scale() - 1.0).abs() < 1e-3);
        // Straightening zooms in: one output pixel covers less than one source pixel.
        let tilted = Geometry { straighten: 10.0, ..Default::default() };
        let k = map(&tilted).source_scale();
        assert!(k > 0.6 && k < 0.95, "{k}");
    }

    fn gradient(w: usize, h: usize) -> LinearImage {
        LinearImage::new(w, h, (0..w * h).map(|i| [(i % w) as f32, (i / w) as f32, 0.0]).collect())
    }

    fn full(w: usize, h: usize) -> Region {
        Region { x: 0, y: 0, w, h }
    }

    #[test]
    fn identity_is_exact() {
        let src = gradient(8, 6);
        let g = GeoMap::new(&Geometry::default(), &LensCorrection::default(), None, 0, 8, 6, false);
        assert_eq!((g.out_w, g.out_h), (8, 6));
        assert_eq!(g.resample(&src, full(8, 6)).data, src.data);
    }

    #[test]
    fn quarter_turn_matches_orientation() {
        let src = gradient(8, 6);
        let g = GeoMap::new(&Geometry::default(), &LensCorrection::default(), None, 1, 8, 6, false);
        assert_eq!((g.out_w, g.out_h), (6, 8));
        let out = g.resample(&src, full(6, 8));
        let expected = src.clone().oriented(crate::image::Orientation::Rotate90);
        assert_eq!(out.data, expected.data);
    }

    #[test]
    fn crop_selects_subregion() {
        let src = gradient(10, 10);
        let geom = Geometry { crop: CropRect { x: 0.2, y: 0.5, w: 0.5, h: 0.3 }, ..Default::default() };
        let g = GeoMap::new(&geom, &LensCorrection::default(), None, 0, 10, 10, false);
        assert_eq!((g.out_w, g.out_h), (5, 3));
        let out = g.resample(&src, full(5, 3));
        assert_eq!(out.data[0][0], 2.0);
        assert_eq!(out.data[0][1], 5.0);
        // ignore_crop shows the full frame
        assert_eq!(GeoMap::new(&geom, &LensCorrection::default(), None, 0, 10, 10, true).out_w, 10);
    }

    #[test]
    fn straighten_keeps_frame_filled() {
        let src = LinearImage::new(40, 30, vec![[1.0; 3]; 1200]);
        let geom = Geometry { straighten: 10.0, ..Default::default() };
        let g = GeoMap::new(&geom, &LensCorrection::default(), None, 0, 40, 30, false);
        let out = g.resample(&src, full(40, 30));
        assert!(out.data.iter().all(|p| p[0] > 0.99), "no black corners");
    }

    #[test]
    fn keystone_keeps_frame_filled() {
        let src = LinearImage::new(60, 40, vec![[1.0; 3]; 2400]);
        for (v, h, angle) in [(60.0, 0.0, 0.0), (-100.0, 0.0, 0.0), (0.0, 80.0, 0.0), (-70.0, 50.0, 5.0), (100.0, -100.0, -8.0)] {
            let geom = Geometry { keystone_v: v, keystone_h: h, straighten: angle, ..Default::default() };
            let g = GeoMap::new(&geom, &LensCorrection::default(), None, 0, 60, 40, false);
            let out = g.resample(&src, full(60, 40));
            assert!(out.data.iter().all(|p| p[0] > 0.99), "empty area at {v} / {h} / {angle}°");
            // ... and not zoomed in further than needed: an edge of the image is reached.
            let reach = [(0.0, 0.0), (60.0, 0.0), (0.0, 40.0), (60.0, 40.0)]
                .iter()
                .map(|&(x, y)| {
                    let (sx, sy) = g.source_point(x, y);
                    ((sx + 0.5) / 30.0 - 1.0).abs().max(((sy + 0.5) / 20.0 - 1.0).abs())
                })
                .fold(0.0, f32::max);
            assert!(reach > 0.99, "zoomed in too far at {v} / {h} / {angle}°: {reach}");
        }
    }

    #[test]
    fn fisheye_keeps_frame_filled_and_straightens_lines() {
        let src = LinearImage::new(60, 40, vec![[1.0; 3]; 2400]);
        for fov in [30.0, 120.0, 180.0, 220.0] {
            let lens = LensCorrection { fisheye: fov, ..Default::default() };
            let g = GeoMap::new(&Geometry { straighten: 3.0, keystone_v: 30.0, ..Default::default() }, &lens, None, 0, 60, 40, false);
            let out = g.resample(&src, full(60, 40));
            assert!(out.data.iter().all(|p| p[0] > 0.99), "empty area at {fov}°");
        }
        // A straight line in the scene is curved on the fisheye image; corrected, it is straight
        // again. Scene: points on the line y = 0.5·h (rectilinear), seen through a 180° fisheye.
        let (w, h) = (600.0f32, 400.0f32);
        let (f, _) = fisheye_params(180.0, w, h);
        let lens = LensCorrection { fisheye: 180.0, ..Default::default() };
        let g = GeoMap::new(&Geometry::default(), &lens, None, 0, w as usize, h as usize, false);
        let pts: Vec<(f32, f32)> = [-0.9f32, -0.5, 0.0, 0.5, 0.9]
            .iter()
            .map(|&t| {
                // A ray through (t·f·2, 0.5·f) at distance f: its angle from the axis.
                let (x, y) = (t * 2.0, 0.5);
                let theta = (x * x + y * y).sqrt().atan();
                let r = f * theta;
                let n = (x * x + y * y).sqrt();
                // Its position on the fisheye image, then back to the output.
                let (sx, sy) = (x / n * r + w / 2.0 - 0.5, y / n * r + h / 2.0 - 0.5);
                g.output_point(sx, sy)
            })
            .collect();
        let ys: Vec<f32> = pts.iter().map(|p| p.1).collect();
        let spread = ys.iter().cloned().fold(f32::MIN, f32::max) - ys.iter().cloned().fold(f32::MAX, f32::min);
        assert!(spread < 0.5, "line still curved: {ys:?}");
    }

    #[test]
    fn flip_mirrors() {
        let src = gradient(4, 2);
        let geom = Geometry { flip_h: true, ..Default::default() };
        let out = GeoMap::new(&geom, &LensCorrection::default(), None, 0, 4, 2, false).resample(&src, full(4, 2));
        assert_eq!(out.data[0][0], 3.0);
    }

    #[test]
    fn barrel_correction_keeps_corners_and_center() {
        let src = LinearImage::new(60, 40, vec![[1.0; 3]; 2400]);
        let lens = LensCorrection { distortion: -100.0, ..Default::default() };
        let g = GeoMap::new(&Geometry::default(), &lens, None, 0, 60, 40, false);
        let out = g.resample(&src, full(60, 40));
        assert!(out.data.iter().all(|p| p[0] > 0.99), "corners stay inside the source");
        let (x, y) = g.source_point(30.0, 20.0);
        assert!((x - 29.5).abs() < 1e-3 && (y - 19.5).abs() < 1e-3);
    }

    #[test]
    fn vignetting_correction_brightens_corners() {
        let src = LinearImage::new(20, 20, vec![[0.5; 3]; 400]);
        let lens = LensCorrection { vignetting: 50.0, ..Default::default() };
        let out = GeoMap::new(&Geometry::default(), &lens, None, 0, 20, 20, false).resample(&src, full(20, 20));
        assert!(out.data[0][0] > out.data[210][0]);
    }

    #[test]
    fn output_point_inverts_source_point() {
        let geom = Geometry {
            straighten: 7.0,
            keystone_v: 30.0,
            crop: CropRect { x: 0.1, y: 0.2, w: 0.7, h: 0.6 },
            flip_h: true,
            ..Default::default()
        };
        let lens = LensCorrection { distortion: 40.0, ..Default::default() };
        for turns in 0..4 {
            let g = GeoMap::new(&geom, &lens, None, turns, 300, 200, false);
            for (ox, oy) in [(10.0, 20.0), (100.5, 60.25), (150.0, 100.0)] {
                let (sx, sy) = g.source_point(ox, oy);
                let (bx, by) = g.output_point(sx, sy);
                assert!((bx - ox).abs() < 0.05 && (by - oy).abs() < 0.05, "turns {turns}: ({ox},{oy}) -> ({bx},{by})");
            }
        }
    }

    #[test]
    fn region_matches_full_render() {
        let src = gradient(30, 20);
        let geom = Geometry { straighten: 5.0, crop: CropRect { x: 0.1, y: 0.1, w: 0.8, h: 0.8 }, ..Default::default() };
        let g = GeoMap::new(&geom, &LensCorrection::default(), None, 1, 30, 20, false);
        let fullimg = g.resample(&src, full(g.out_w, g.out_h));
        let part = g.resample(&src, Region { x: 3, y: 4, w: 5, h: 6 });
        for y in 0..6 {
            for x in 0..5 {
                assert_eq!(part.data[y * 5 + x], fullimg.data[(y + 4) * g.out_w + x + 3]);
            }
        }
    }

    #[test]
    fn camera_correction_moves_the_corners_and_follows_the_switch() {
        let cam = CameraLens { source: "Test", center: [0.0; 2], warp: [[1.0, -0.06, 0.0, 0.0]; 3], vignette: [0.2, 0.0, 0.0, 0.0, 0.0] };
        let on = LensCorrection::default();
        assert!(on.camera);
        let off = LensCorrection { camera: false, ..Default::default() };
        let (w, h) = (300, 200);
        let plain = GeoMap::new(&Geometry::default(), &off, Some(&cam), 0, w, h, false);
        assert!(plain.pixel_exact);
        let g = GeoMap::new(&Geometry::default(), &on, Some(&cam), 0, w, h, false);
        assert!(!g.pixel_exact);
        // Barrel correction: the output's corner comes from inside the source, the center stays.
        let (pts, gain) = g.map(0.5, 0.5);
        assert!(pts[1][0] > 0.0 && pts[1][1] > 0.0);
        assert!(gain > 1.1);
        let ([_, c, _], center_gain) = g.map(150.0, 100.0);
        assert!((c[0] - 149.5).abs() < 1e-3 && (c[1] - 99.5).abs() < 1e-3 && (center_gain - 1.0).abs() < 1e-6);
        // Inverse mapping still works.
        let (ox, oy) = g.output_point(40.0, 30.0);
        let back = g.source_point(ox, oy);
        assert!((back.0 - 40.0).abs() < 0.01 && (back.1 - 30.0).abs() < 0.01);
    }
}
