//! Automatic straightening and perspective correction: find straight edges in the image, then search the straighten
//! angle and keystone values that make them horizontal or vertical.
//!
//! The edges are found in the camera image (source coordinates); every candidate is judged
//! through the real geometric transform (`GeoMap`, including lens correction), so the result
//! is exactly what the pipeline will render.

use crate::geometry::GeoMap;
use crate::image::LinearImage;
use crate::params::{Geometry, KEYSTONE_MAX, LensCorrection};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UprightMode {
    /// Rotation only: horizon and edges level.
    Level,
    /// Rotation and vertical keystone: converging verticals become parallel.
    Vertical,
    /// Rotation and both keystones.
    Full,
}

/// A straight edge in source pixel coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub a: [f32; 2],
    pub b: [f32; 2],
}

impl Segment {
    fn length(&self) -> f32 {
        (self.b[0] - self.a[0]).hypot(self.b[1] - self.a[1])
    }
}

/// Longest side of the image the edges are searched in.
const DETECT_SIDE: usize = 1000;

/// Straight edges of `img` (camera RGB, linear), longest first.
pub fn detect_segments(img: &LinearImage) -> Vec<Segment> {
    let step = img.width.max(img.height).div_ceil(DETECT_SIDE).max(1);
    let (w, h) = (img.width / step, img.height / step);
    if w < 16 || h < 16 {
        return Vec::new();
    }
    // Box-averaged brightness with a gamma, so that shadows and highlights both have edges.
    let mut gray = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0.0;
            for sy in 0..step {
                let row = (y * step + sy) * img.width + x * step;
                for p in &img.data[row..row + step] {
                    sum += p[0] + p[1] + p[2];
                }
            }
            gray[y * w + x] = (sum / (3 * step * step) as f32).max(0.0).powf(1.0 / 2.2);
        }
    }

    // A light blur (as LSD's Gaussian subsampling): stair-stepped and noisy edges would
    // otherwise break into short pieces.
    for _ in 0..2 {
        gray = blur(&blur(&gray, w, h, true), w, h, false);
    }

    // 2x2 gradient (as in LSD), at the pixel corner (x + 0.5, y + 0.5).
    let (gw, gh) = (w - 1, h - 1);
    let mut mag = vec![0.0f32; gw * gh];
    let mut dir = vec![[0.0f32; 2]; gw * gh];
    for y in 0..gh {
        for x in 0..gw {
            let (p, r, d, rd) = (gray[y * w + x], gray[y * w + x + 1], gray[(y + 1) * w + x], gray[(y + 1) * w + x + 1]);
            let gx = (r + rd - p - d) / 2.0;
            let gy = (d + rd - p - r) / 2.0;
            let m = gx.hypot(gy);
            mag[y * gw + x] = m;
            // Direction along the edge (perpendicular to the gradient), as a doubled-angle unit
            // vector so that opposite directions count as the same line.
            if m > 0.0 {
                let (ex, ey) = (-gy / m, gx / m);
                dir[y * gw + x] = [ex * ex - ey * ey, 2.0 * ex * ey];
            }
        }
    }
    // Above the noise (as in LSD: a quantization step over sin 22.5°, on the gamma scale).
    let threshold = 0.02;
    let mut order: Vec<usize> = (0..mag.len()).filter(|&i| mag[i] >= threshold).collect();
    order.sort_unstable_by(|&a, &b| mag[b].total_cmp(&mag[a]));

    // Region growing: neighbors with about the same edge direction (within 22.5°).
    let cos_tol = (2.0 * 22.5f32.to_radians()).cos();
    let mut used = vec![false; mag.len()];
    let mut segments = Vec::new();
    let min_len = DETECT_SIDE.min(gw.max(gh)) as f32 * 0.04;
    let mut region = Vec::new();
    for &seed in &order {
        if used[seed] {
            continue;
        }
        used[seed] = true;
        region.clear();
        region.push(seed);
        let mut sum = dir[seed];
        let mut i = 0;
        while i < region.len() {
            let (x, y) = ((region[i] % gw) as i64, (region[i] / gw) as i64);
            i += 1;
            let norm = sum[0].hypot(sum[1]).max(1e-9);
            let mean = [sum[0] / norm, sum[1] / norm];
            for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= gw as i64 || ny >= gh as i64 {
                    continue;
                }
                let n = ny as usize * gw + nx as usize;
                if used[n] || mag[n] < threshold {
                    continue;
                }
                if dir[n][0] * mean[0] + dir[n][1] * mean[1] >= cos_tol {
                    used[n] = true;
                    region.push(n);
                    sum = [sum[0] + dir[n][0], sum[1] + dir[n][1]];
                }
            }
        }
        if (region.len() as f32) < min_len {
            continue;
        }
        if let Some(s) = fit_segment(&region, gw, &mag, min_len) {
            // Corner coordinates of the small image -> source pixel coordinates.
            let to_src = |p: [f32; 2]| p.map(|v| (v + 1.0) * step as f32 - 0.5);
            segments.push(Segment { a: to_src(s.a), b: to_src(s.b) });
        }
    }
    segments.sort_by(|a, b| b.length().total_cmp(&a.length()));
    segments.truncate(400);
    segments
}

/// [1 2 1] / 4 along rows (`horizontal`) or columns, edges clamped.
fn blur(src: &[f32], w: usize, h: usize, horizontal: bool) -> Vec<f32> {
    let mut out = vec![0.0; src.len()];
    for y in 0..h {
        for x in 0..w {
            let at = |dx: i64, dy: i64| {
                let (nx, ny) = ((x as i64 + dx).clamp(0, w as i64 - 1), (y as i64 + dy).clamp(0, h as i64 - 1));
                src[ny as usize * w + nx as usize]
            };
            let (dx, dy) = if horizontal { (1, 0) } else { (0, 1) };
            out[y * w + x] = (at(-dx, -dy) + 2.0 * at(0, 0) + at(dx, dy)) / 4.0;
        }
    }
    out
}

/// The line through a region of edge pixels, if it is long and thin.
fn fit_segment(region: &[usize], gw: usize, mag: &[f32], min_len: f32) -> Option<Segment> {
    let (mut sw, mut sx, mut sy) = (0.0f64, 0.0f64, 0.0f64);
    for &i in region {
        let m = mag[i] as f64;
        sw += m;
        sx += m * (i % gw) as f64;
        sy += m * (i / gw) as f64;
    }
    let (cx, cy) = (sx / sw, sy / sw);
    let (mut xx, mut xy, mut yy) = (0.0f64, 0.0f64, 0.0f64);
    for &i in region {
        let m = mag[i] as f64;
        let (dx, dy) = ((i % gw) as f64 - cx, (i / gw) as f64 - cy);
        xx += m * dx * dx;
        xy += m * dx * dy;
        yy += m * dy * dy;
    }
    // Principal direction of the pixel cloud.
    let theta = 0.5 * (2.0 * xy).atan2(xx - yy);
    let (ux, uy) = (theta.cos(), theta.sin());
    let (mut lo, mut hi, mut wlo, mut whi) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for &i in region {
        let (dx, dy) = ((i % gw) as f64 - cx, (i / gw) as f64 - cy);
        let along = dx * ux + dy * uy;
        let across = -dx * uy + dy * ux;
        (lo, hi) = (lo.min(along), hi.max(along));
        (wlo, whi) = (wlo.min(across), whi.max(across));
    }
    let (len, width) = (hi - lo, (whi - wlo).max(1.0));
    if (len as f32) < min_len || len / width < 8.0 {
        return None;
    }
    let p = |t: f64| [(cx + ux * t) as f32, (cy + uy * t) as f32];
    Some(Segment { a: p(lo), b: p(hi) })
}

/// Angle of a line in degrees, folded to -90..90 (0 = horizontal, ±90 = vertical).
fn line_angle(a: (f32, f32), b: (f32, f32)) -> f32 {
    let mut d = (b.1 - a.1).atan2(b.0 - a.0).to_degrees();
    if d >= 90.0 {
        d -= 180.0;
    } else if d < -90.0 {
        d += 180.0;
    }
    d
}

/// Edges within this many degrees of an axis (without straighten and keystone) count for it;
/// clearly oblique edges, like a roof or a path seen from the side, are left alone. Verticals
/// may lean more: converging lines of a building shot from below.
const NEAR_HORIZONTAL: f32 = 8.0;
const NEAR_VERTICAL: f32 = 15.0;
/// Scale of the robust cost (degrees): edges much further off weigh little. The coarse
/// search uses a wider one, so that it sees the way to the minimum from further away.
const SOFT: f32 = 1.5;
const SOFT_COARSE: f32 = 5.0;

struct Problem<'a> {
    /// With their axis: `true` vertical.
    segments: Vec<(&'a Segment, bool)>,
    geometry: Geometry,
    lens: &'a LensCorrection,
    camera: Option<&'a crate::camera_lens::CameraLens>,
    turns: u8,
    src: (usize, usize),
    /// Weight of horizontal edges (verticals have weight 1).
    horizontal_weight: f32,
    soft: f32,
    /// Restraint on large keystone values (none for lines the user drew).
    restraint: f32,
}

impl Problem<'_> {
    fn geometry(&self, p: [f32; 3]) -> Geometry {
        Geometry { straighten: p[0], keystone_v: p[1], keystone_h: p[2], ..self.geometry.clone() }
    }

    /// Mean robust deviation of the edges from their axes, plus a little restraint on keystone.
    fn cost(&self, p: [f32; 3]) -> f32 {
        let map = GeoMap::new(&self.geometry(p), self.lens, self.camera, self.turns, self.src.0, self.src.1, true);
        let (mut sum, mut weight) = (0.0, 0.0);
        for &(s, vertical) in &self.segments {
            let a = map.output_point(s.a[0], s.a[1]);
            let b = map.output_point(s.b[0], s.b[1]);
            let angle = line_angle(a, b);
            let (dev, w) = if vertical { (angle - 90.0 * angle.signum(), 1.0) } else { (angle, self.horizontal_weight) };
            let w = w * s.length();
            let d2 = dev * dev;
            sum += w * d2 / (d2 + self.soft * self.soft);
            weight += w;
        }
        if weight == 0.0 {
            return f32::MAX;
        }
        sum / weight + self.restraint * ((p[1] / 100.0).powi(2) + (p[2] / 100.0).powi(2))
    }

    /// The parameters `free` (0 straighten, 1 vertical, 2 horizontal keystone) that minimize the
    /// cost: a coarse grid (rotation and the first keystone together, as they depend on each
    /// other; the second keystone on its own), then a pattern search with shrinking steps, from
    /// the wide robust cost to `soft`.
    fn search(&mut self, free: &[usize], limits: [f32; 3], soft: f32) -> [f32; 3] {
        let coarse = [1.0f32, 10.0, 10.0];
        let grid = |k: usize| {
            let n = (limits[k] / coarse[k]) as i32;
            (-n..=n).map(move |i| i as f32 * coarse[k])
        };
        self.soft = SOFT_COARSE;
        let mut p = [0.0f32; 3];
        let mut best = f32::MAX;
        let second = free.get(1).copied();
        for a in grid(0) {
            for b in second.map_or(vec![0.0], |k| grid(k).collect()) {
                let mut q = [0.0; 3];
                q[0] = a;
                if let Some(k) = second {
                    q[k] = b;
                }
                let c = self.cost(q);
                if c < best {
                    (best, p) = (c, q);
                }
            }
        }
        if let Some(&k) = free.get(2) {
            for v in grid(k) {
                let mut q = p;
                q[k] = v;
                let c = self.cost(q);
                if c < best {
                    (best, p) = (c, q);
                }
            }
        }
        let mut steps = coarse.map(|s| s / 2.0);
        for soft in [SOFT_COARSE, soft] {
            self.soft = soft;
            best = self.cost(p);
            while steps[0] > 0.01 {
                let mut moved = false;
                for &k in free {
                    for sign in [-1.0, 1.0] {
                        let mut q = p;
                        q[k] = (q[k] + sign * steps[k]).clamp(-limits[k], limits[k]);
                        let c = self.cost(q);
                        if c < best {
                            (best, p, moved) = (c, q, true);
                        }
                    }
                }
                if !moved {
                    steps = steps.map(|s| s / 2.0);
                }
            }
            steps = [0.25, 2.5, 2.5];
        }
        p
    }
}

/// `geometry` with the solved straighten and keystone values `p`, rounded like the sliders.
fn solved(geometry: &Geometry, p: [f32; 3]) -> Geometry {
    let round = |v: f32, k: f32| (v * k).round() / k;
    let mut out = geometry.clone();
    out.straighten = round(p[0], 100.0);
    out.keystone_v = round(p[1], 10.0);
    out.keystone_h = round(p[2], 10.0);
    out
}

/// Straighten angle and keystone values that make the lines the user drew vertical (`true`)
/// or horizontal: both keystones are free as far as there are lines for them. Solved for the
/// full keystone, so that a smaller `keystone_amount` still weakens the result.
pub fn solve_guides(
    guides: &[(Segment, bool)],
    geometry: &Geometry,
    lens: &LensCorrection,
    camera: Option<&crate::camera_lens::CameraLens>,
    turns: u8,
    src: (usize, usize),
) -> Option<Geometry> {
    let guides: Vec<(&Segment, bool)> = guides.iter().filter(|(s, _)| s.length() > 2.0).map(|(s, v)| (s, *v)).collect();
    if guides.is_empty() {
        return None;
    }
    let mut problem = Problem {
        segments: guides,
        geometry: Geometry { keystone_amount: 100.0, ..geometry.clone() },
        lens,
        camera,
        turns,
        src,
        horizontal_weight: 1.0,
        soft: SOFT_COARSE,
        restraint: 0.0,
    };
    // Keystone works in sensor coordinates: with a quarter turn the "vertical" one is the
    // horizontal slider.
    let (vertical, horizontal) = if turns % 2 == 1 { (2, 1) } else { (1, 2) };
    let count = |axis: bool| problem.segments.iter().filter(|(_, v)| *v == axis).count();
    let mut free = vec![0];
    // One line only sets the rotation; two converging ones also the keystone.
    if count(true) >= 2 {
        free.push(vertical);
    }
    if count(false) >= 2 {
        free.push(horizontal);
    }
    // Exact lines: a cost that keeps growing with the deviation.
    let p = problem.search(&free, [45.0, KEYSTONE_MAX, KEYSTONE_MAX], 20.0);
    let mut out = solved(geometry, p);
    if !free.contains(&1) {
        out.keystone_v = geometry.keystone_v;
    }
    if !free.contains(&2) {
        out.keystone_h = geometry.keystone_h;
    }
    Some(out)
}

/// Straighten angle and keystone values for `mode`, starting from the current geometry (only
/// those three values change). `None` if the image has too few straight edges.
pub fn solve(
    segments: &[Segment],
    geometry: &Geometry,
    lens: &LensCorrection,
    camera: Option<&crate::camera_lens::CameraLens>,
    turns: u8,
    src: (usize, usize),
    mode: UprightMode,
) -> Option<Geometry> {
    // Each edge's axis, from its direction without straighten and keystone.
    let plain = Geometry { straighten: 0.0, keystone_v: 0.0, keystone_h: 0.0, ..geometry.clone() };
    let base = GeoMap::new(&plain, lens, camera, turns, src.0, src.1, true);
    let segments: Vec<(&Segment, bool)> = segments
        .iter()
        .filter_map(|s| {
            let angle = line_angle(base.output_point(s.a[0], s.a[1]), base.output_point(s.b[0], s.b[1]));
            if angle.abs() <= NEAR_HORIZONTAL {
                Some((s, false))
            } else if angle.abs() >= 90.0 - NEAR_VERTICAL {
                Some((s, true))
            } else {
                None
            }
        })
        .collect();
    let total: f32 = segments.iter().map(|(s, _)| s.length()).sum();
    if segments.len() < 2 || total < src.0.max(src.1) as f32 * 0.3 {
        return None;
    }
    let mut problem = Problem {
        segments,
        // Solved for the full keystone (see `solve_guides`).
        geometry: Geometry { keystone_amount: 100.0, ..geometry.clone() },
        lens,
        camera,
        turns,
        src,
        horizontal_weight: if mode == UprightMode::Vertical { 0.3 } else { 1.0 },
        soft: SOFT_COARSE,
        restraint: 0.01,
    };
    // Keystone works in sensor coordinates: with a quarter turn the "vertical" one is the
    // horizontal slider.
    let (vertical, horizontal) = if turns % 2 == 1 { (2, 1) } else { (1, 2) };
    // Converging lines only show where there are several of them, spread across the image:
    // otherwise a keystone would just bend a few edges into place.
    let supported = |axis_vertical: bool| {
        let lines: Vec<(f32, f32)> = problem
            .segments
            .iter()
            .filter(|(_, v)| *v == axis_vertical)
            .map(|(s, _)| {
                let (a, b) = (base.output_point(s.a[0], s.a[1]), base.output_point(s.b[0], s.b[1]));
                // Position across the lines (x for verticals, y for horizontals), relative.
                let across = if axis_vertical { (a.0 + b.0) / 2.0 / base.out_w as f32 } else { (a.1 + b.1) / 2.0 / base.out_h as f32 };
                (across, s.length())
            })
            .collect();
        let length: f32 = lines.iter().map(|l| l.1).sum();
        let (lo, hi) = lines.iter().fold((f32::MAX, f32::MIN), |(lo, hi), l| (lo.min(l.0), hi.max(l.0)));
        lines.len() >= 3 && length >= src.0.max(src.1) as f32 * 0.4 && hi - lo >= 0.3
    };
    let mut free = vec![0];
    if mode != UprightMode::Level && supported(true) {
        free.push(vertical);
    }
    if mode == UprightMode::Full && supported(false) {
        free.push(horizontal);
    }
    let p = problem.search(&free, [NEAR_VERTICAL, KEYSTONE_MAX, KEYSTONE_MAX], SOFT);
    let mut out = solved(geometry, p);
    if mode == UprightMode::Level {
        (out.keystone_v, out.keystone_h) = (geometry.keystone_v, geometry.keystone_h);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic scene: dark vertical bars and horizontal bands on a light background,
    /// rendered through `geometry` (so the solver has to find its inverse).
    fn scene(geometry: &Geometry, w: usize, h: usize) -> LinearImage {
        let map = GeoMap::new(&geometry.clone(), &LensCorrection::default(), None, 0, w, h, true);
        let mut data = vec![[0.0f32; 3]; w * h];
        for y in 0..h {
            for x in 0..w {
                // Where this source pixel lands in the straight output.
                let (ox, oy) = map.output_point(x as f32, y as f32);
                // Undo the keystone zoom, so that the scene is the same whatever it is.
                let k = map.keystone_zoom();
                let (u, v) = (0.5 + (ox / map.out_w as f32 - 0.5) * k, 0.5 + (oy / map.out_h as f32 - 0.5) * k);
                let bars = (u * 12.0).fract() < 0.3;
                let bands = (v * 6.0).fract() < 0.15;
                let val = if bars || bands { 0.05 } else { 0.6 };
                data[y * w + x] = [val; 3];
            }
        }
        LinearImage::new(w, h, data)
    }

    fn solve_scene(truth: &Geometry, mode: UprightMode) -> Geometry {
        let (w, h) = (600, 400);
        let img = scene(truth, w, h);
        let segments = detect_segments(&img);
        assert!(segments.len() >= 6, "Kanten: {}", segments.len());
        solve(&segments, &Geometry::default(), &LensCorrection::default(), None, 0, (w, h), mode).expect("Lösung")
    }

    #[test]
    fn finds_a_tilted_horizon() {
        // The scene is straight when rendered with this geometry, so that's what the solver must find.
        let truth = Geometry { straighten: 3.0, ..Default::default() };
        let g = solve_scene(&truth, UprightMode::Level);
        assert!((g.straighten - 3.0).abs() < 0.2, "{}", g.straighten);
        assert_eq!((g.keystone_v, g.keystone_h), (0.0, 0.0));
    }

    #[test]
    fn corrects_converging_verticals() {
        let truth = Geometry { straighten: -1.5, keystone_v: 30.0, ..Default::default() };
        let g = solve_scene(&truth, UprightMode::Vertical);
        assert!((g.straighten + 1.5).abs() < 0.3, "{}", g.straighten);
        assert!((g.keystone_v - 30.0).abs() < 4.0, "{}", g.keystone_v);
    }

    /// Lines that `truth` makes vertical (`v`) or horizontal, in source coordinates.
    fn guides_of(truth: &Geometry, w: usize, h: usize) -> Vec<(Segment, bool)> {
        let map = GeoMap::new(truth, &LensCorrection::default(), None, 0, w, h, true);
        let (ow, oh) = (map.out_w as f32, map.out_h as f32);
        let seg = |a: (f32, f32), b: (f32, f32)| {
            let (a, b) = (map.source_point(a.0, a.1), map.source_point(b.0, b.1));
            Segment { a: [a.0, a.1], b: [b.0, b.1] }
        };
        vec![
            (seg((0.25 * ow, 0.2 * oh), (0.25 * ow, 0.8 * oh)), true),
            (seg((0.75 * ow, 0.15 * oh), (0.75 * ow, 0.85 * oh)), true),
            (seg((0.2 * ow, 0.3 * oh), (0.8 * ow, 0.3 * oh)), false),
            (seg((0.2 * ow, 0.7 * oh), (0.8 * ow, 0.7 * oh)), false),
        ]
    }

    #[test]
    fn drawn_lines_give_their_correction() {
        let (w, h) = (600, 400);
        let solve = |truth: &Geometry, lines: &[(Segment, bool)]| {
            let start = Geometry { keystone_amount: 80.0, ..Default::default() };
            solve_guides(lines, &start, &LensCorrection::default(), None, 0, (w, h)).expect("Lösung")
        };
        // Two vertical lines: rotation and vertical keystone; the amount stays as it was.
        let truth = Geometry { straighten: 2.0, keystone_v: 35.0, ..Default::default() };
        let g = solve(&truth, &guides_of(&truth, w, h)[..2]);
        assert!((g.straighten - 2.0).abs() < 0.3 && (g.keystone_v - 35.0).abs() < 2.0 && g.keystone_h == 0.0, "{g:?}");
        assert_eq!(g.keystone_amount, 80.0);
        // Four lines: both keystones.
        let truth = Geometry { straighten: -1.0, keystone_v: -20.0, keystone_h: 25.0, ..Default::default() };
        let g = solve(&truth, &guides_of(&truth, w, h));
        assert!((g.straighten + 1.0).abs() < 0.3 && (g.keystone_v + 20.0).abs() < 2.0 && (g.keystone_h - 25.0).abs() < 2.0, "{g:?}");
    }

    #[test]
    fn keystone_amount_weakens_the_correction() {
        let at = |amount: f32| {
            let g = Geometry { keystone_v: 40.0, keystone_amount: amount, ..Default::default() };
            GeoMap::new(&g, &LensCorrection::default(), None, 0, 600, 400, true).source_point(100.0, 50.0)
        };
        let plain = GeoMap::new(&Geometry::default(), &LensCorrection::default(), None, 0, 600, 400, true).source_point(100.0, 50.0);
        let (full, half, none) = (at(100.0), at(50.0), at(0.0));
        assert_eq!(none, plain);
        assert!((half.0 - plain.0).abs() > 1.0 && (half.0 - plain.0).abs() < (full.0 - plain.0).abs(), "{plain:?} {half:?} {full:?}");
        // Settings saved before the amount existed apply in full.
        let old: Geometry = serde_json::from_str(r#"{"keystone_v":40.0}"#).unwrap();
        assert_eq!(old.keystone_amount, 100.0);
    }

    #[test]
    fn full_mode_corrects_both_keystones() {
        let truth = Geometry { straighten: 1.0, keystone_v: -25.0, keystone_h: 20.0, ..Default::default() };
        let g = solve_scene(&truth, UprightMode::Full);
        assert!((g.straighten - 1.0).abs() < 0.4, "{}", g.straighten);
        assert!((g.keystone_v + 25.0).abs() < 5.0 && (g.keystone_h - 20.0).abs() < 5.0, "{} {}", g.keystone_v, g.keystone_h);
    }

    #[test]
    fn real_photo_in_reasonable_time() {
        let sample = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/_DSC0010.ARW");
        if !sample.exists() {
            return;
        }
        let (meta, preview) = crate::raw::load_preview(&sample, 1600).unwrap();
        let t = std::time::Instant::now();
        let segments = detect_segments(&preview);
        let g = solve(&segments, &Geometry::default(), &LensCorrection::default(), None, 0, (preview.width, preview.height), UprightMode::Full);
        let ms = t.elapsed().as_millis();
        eprintln!("{} Kanten, {g:?}, {ms} ms ({})", segments.len(), meta.info.model);
        assert!(ms < 3000);
    }

    #[test]
    fn plain_image_has_no_solution() {
        let img = LinearImage::new(300, 200, vec![[0.3; 3]; 300 * 200]);
        let segments = detect_segments(&img);
        assert!(segments.is_empty());
        assert!(solve(&segments, &Geometry::default(), &LensCorrection::default(), None, 0, (300, 200), UprightMode::Full).is_none());
    }
}
