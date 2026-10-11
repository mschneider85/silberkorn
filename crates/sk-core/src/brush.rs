//! Brush strokes: smooth dabs along the painted path with size, hardness, flow (build-up within
//! a stroke), opacity (the most a stroke covers) and automatic masking (only where the image is
//! similar to what is under the brush's center).

use rayon::prelude::*;

use crate::image::LinearImage;
use crate::params::BrushStroke;

/// Image content for automatic masking: log camera RGB at a reduced size.
pub struct AutoGuide {
    w: usize,
    h: usize,
    data: Vec<[f32; 3]>,
}

impl AutoGuide {
    /// From the (oriented, camera RGB) source image.
    pub fn from_image(img: &LinearImage, max_edge: usize) -> Self {
        let small = img.clone().fit_within(max_edge);
        let data = small.data.par_iter().map(|p| p.map(|v| (v.max(0.0) + 0.002).ln())).collect();
        Self { w: small.width, h: small.height, data }
    }

    /// Bilinear sample at normalized source coordinates.
    fn at(&self, u: f32, v: f32) -> [f32; 3] {
        let x = (u * self.w as f32 - 0.5).clamp(0.0, (self.w - 1) as f32);
        let y = (v * self.h as f32 - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (x as usize, y as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = (x - x0 as f32, y - y0 as f32);
        let p = |xx: usize, yy: usize| self.data[yy * self.w + xx];
        let (a, b, c, d) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));
        std::array::from_fn(|k| {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bot = c[k] + (d[k] - c[k]) * tx;
            top + (bot - top) * ty
        })
    }
}

/// How much a pixel resembles the color under the brush (1 = same, 0 = different).
#[inline]
fn similarity(p: [f32; 3], r: [f32; 3]) -> f32 {
    // Brightness and color differences in log space (exposure independent).
    let d: f32 = (0..3).map(|k| (p[k] - r[k]).powi(2)).sum();
    1.0 - smoothstep(0.03, 0.15, d)
}

#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if e1 <= e0 {
        return if x < e0 { 0.0 } else { 1.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Points along a Catmull-Rom spline through `pts`, about `spacing` apart.
pub fn smooth_path(pts: &[(f32, f32)], spacing: f32) -> Vec<(f32, f32)> {
    if pts.len() < 2 {
        return pts.to_vec();
    }
    let spacing = spacing.max(1e-6);
    let mut out = vec![pts[0]];
    for i in 0..pts.len() - 1 {
        let p0 = pts[i.saturating_sub(1)];
        let (p1, p2) = (pts[i], pts[i + 1]);
        let p3 = pts[(i + 2).min(pts.len() - 1)];
        let len = ((p2.0 - p1.0).powi(2) + (p2.1 - p1.1).powi(2)).sqrt();
        let n = ((len / spacing).ceil() as usize).clamp(1, 10_000);
        for s in 1..=n {
            let t = s as f32 / n as f32;
            let (t2, t3) = (t * t, t * t * t);
            let f = |a: f32, b: f32, c: f32, d: f32| 0.5 * (2.0 * b + (-a + c) * t + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2 + (-a + 3.0 * b - 3.0 * c + d) * t3);
            out.push((f(p0.0, p1.0, p2.0, p3.0), f(p0.1, p1.1, p2.1, p3.1)));
        }
    }
    out
}

/// Paints a stroke into a raster covering `rect` (x, y, w, h; normalized source coordinates)
/// of a source with aspect ratio `aspect`. Dabs every 15 % of the radius along the smoothed
/// path; within the stroke, coverage builds up with the flow (1 - Π(1 - flow · falloff)), capped
/// by the opacity.
pub fn paint_stroke(data: &mut [f32], w: usize, h: usize, rect: [f32; 4], aspect: f32, stroke: &BrushStroke, guide: Option<&AutoGuide>) {
    if stroke.points.is_empty() || w == 0 || h == 0 {
        return;
    }
    // Long-edge units.
    let (kx, ky) = if aspect >= 1.0 { (1.0, 1.0 / aspect) } else { (aspect, 1.0) };
    let r = stroke.radius.max(1e-5);
    let inner = r * stroke.hardness.clamp(0.0, 0.99);
    let pts: Vec<(f32, f32)> = stroke.points.iter().map(|p| (p[0] * kx, p[1] * ky)).collect();
    // Raster pixel size in long-edge units (at least a fraction of a pixel between dabs).
    let px = (rect[2] * kx / w as f32).min(rect[3] * ky / h as f32);
    let dabs = smooth_path(&pts, (r * 0.15).max(px * 0.5));
    let flow = stroke.flow.clamp(0.0, 1.0);
    let opacity = stroke.opacity.clamp(0.0, 1.0);
    let auto = guide.filter(|_| stroke.auto_mask);
    // Color under the center of each dab.
    let refs: Vec<[f32; 3]> = match auto {
        Some(g) => dabs.iter().map(|d| g.at(d.0 / kx, d.1 / ky)).collect(),
        None => Vec::new(),
    };
    // Raster rows the stroke touches.
    let to_x = |u: f32| (u / kx - rect[0]) / rect[2] * w as f32;
    let to_y = |v: f32| (v / ky - rect[1]) / rect[3] * h as f32;
    let (mut y0, mut y1) = (f32::MAX, f32::MIN);
    for d in &dabs {
        y0 = y0.min(to_y(d.1 - r));
        y1 = y1.max(to_y(d.1 + r));
    }
    let (ry0, ry1) = (y0.floor().max(0.0) as usize, (y1.ceil().max(0.0) as usize).min(h));
    if ry0 >= ry1 {
        return;
    }
    data[ry0 * w..ry1 * w].par_chunks_mut(w).enumerate().for_each(|(i, row)| {
        let y = ry0 + i;
        let v = (rect[1] + (y as f32 + 0.5) / h as f32 * rect[3]) * ky;
        // 1 - coverage of this stroke, per pixel of the row.
        let mut keep: Vec<f32> = Vec::new();
        let mut span = (usize::MAX, 0usize);
        for (k, d) in dabs.iter().enumerate() {
            let dy = v - d.1;
            if dy.abs() >= r {
                continue;
            }
            let half = (r * r - dy * dy).sqrt();
            let (x0, x1) = (to_x(d.0 - half).floor().max(0.0) as usize, (to_x(d.0 + half).ceil().max(0.0) as usize).min(w));
            if x0 >= x1 {
                continue;
            }
            if keep.is_empty() {
                keep = vec![1.0; w];
            }
            span = (span.0.min(x0), span.1.max(x1));
            for (x, kp) in keep.iter_mut().enumerate().take(x1).skip(x0) {
                let u = (rect[0] + (x as f32 + 0.5) / w as f32 * rect[2]) * kx;
                let dist = ((u - d.0).powi(2) + dy * dy).sqrt();
                let mut a = flow * (1.0 - smoothstep(inner, r, dist));
                if let Some(g) = auto
                    && a > 0.0
                {
                    a *= similarity(g.at(u / kx, v / ky), refs[k]);
                }
                *kp *= 1.0 - a;
            }
        }
        if keep.is_empty() {
            return;
        }
        for x in span.0..span.1 {
            let c = (1.0 - keep[x]) * opacity;
            let m = &mut row[x];
            *m = if stroke.erase { *m * (1.0 - c) } else { *m + (1.0 - *m) * c };
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(points: Vec<[f32; 2]>) -> BrushStroke {
        BrushStroke { points, radius: 0.05, hardness: 0.5, flow: 1.0, erase: false, opacity: 1.0, auto_mask: false, build_up: true }
    }

    #[test]
    fn smooth_path_passes_through_points_densely() {
        let p = smooth_path(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)], 0.1);
        assert!(p.len() >= 20);
        assert_eq!(*p.last().unwrap(), (1.0, 1.0));
        assert!(p.windows(2).all(|w| ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt() < 0.15));
    }

    #[test]
    fn opacity_caps_and_flow_builds_up() {
        let (w, h) = (200, 200);
        let mut full = vec![0.0; w * h];
        paint_stroke(&mut full, w, h, [0.0, 0.0, 1.0, 1.0], 1.0, &stroke(vec![[0.2, 0.5], [0.8, 0.5]]), None);
        assert!(full[100 * w + 100] > 0.99 && full[10 * w + 10] == 0.0);
        let mut half = vec![0.0; w * h];
        paint_stroke(&mut half, w, h, [0.0, 0.0, 1.0, 1.0], 1.0, &BrushStroke { opacity: 0.5, ..stroke(vec![[0.2, 0.5], [0.8, 0.5]]) }, None);
        assert!((half[100 * w + 100] - 0.5).abs() < 0.01);
        // Low flow: a single dab covers little, a stroke over the same spot builds up.
        let mut dab = vec![0.0; w * h];
        paint_stroke(&mut dab, w, h, [0.0, 0.0, 1.0, 1.0], 1.0, &BrushStroke { flow: 0.1, ..stroke(vec![[0.5, 0.5]]) }, None);
        let mut line = vec![0.0; w * h];
        paint_stroke(&mut line, w, h, [0.0, 0.0, 1.0, 1.0], 1.0, &BrushStroke { flow: 0.1, ..stroke(vec![[0.4, 0.5], [0.6, 0.5]]) }, None);
        assert!(dab[100 * w + 100] < 0.15 && line[100 * w + 100] > 0.4, "{} {}", dab[100 * w + 100], line[100 * w + 100]);
        // Soft edge: hardness 0 falls off from the center.
        let mut soft = vec![0.0; w * h];
        paint_stroke(&mut soft, w, h, [0.0, 0.0, 1.0, 1.0], 1.0, &BrushStroke { hardness: 0.0, ..stroke(vec![[0.5, 0.5]]) }, None);
        assert!(soft[100 * w + 100] > 0.9 && soft[100 * w + 106] < soft[100 * w + 102]);
    }

    #[test]
    fn auto_mask_stays_on_the_color_under_the_brush() {
        // Left half dark green, right half bright red.
        let (w, h) = (200, 100);
        let img = LinearImage::new(w, h, (0..w * h).map(|i| if i % w < w / 2 { [0.02, 0.1, 0.03] } else { [0.6, 0.1, 0.08] }).collect());
        let guide = AutoGuide::from_image(&img, 256);
        let mut m = vec![0.0; w * h];
        // A big brush centered in the left half, reaching into the right half.
        let s = BrushStroke { radius: 0.2, auto_mask: true, ..stroke(vec![[0.35, 0.5], [0.4, 0.5]]) };
        paint_stroke(&mut m, w, h, [0.0, 0.0, 1.0, 1.0], 2.0, &s, Some(&guide));
        assert!(m[50 * w + 80] > 0.9, "links {}", m[50 * w + 80]);
        assert!(m[50 * w + 120] < 0.05, "rechts {}", m[50 * w + 120]);
    }
}
