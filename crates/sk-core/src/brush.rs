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
        let small = img.fitted(max_edge);
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
    smooth_path_stable(pts, spacing).0
}

/// [`smooth_path`], and how many of its points stay the same when further points are added:
/// all but those of the last segment, whose curve still depends on the next point.
fn smooth_path_stable(pts: &[(f32, f32)], spacing: f32) -> (Vec<(f32, f32)>, usize) {
    if pts.len() < 2 {
        return (pts.to_vec(), pts.len());
    }
    let spacing = spacing.max(1e-6);
    let mut out = vec![pts[0]];
    let mut stable = 1;
    for i in 0..pts.len() - 1 {
        if i == pts.len() - 2 {
            stable = out.len();
        }
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
    (out, stable)
}

/// Pixel area: columns x0..x1, rows y0..y1.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Area {
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
}

impl Area {
    fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }

    fn union(self, o: Area) -> Area {
        match (self.is_empty(), o.is_empty()) {
            (true, _) => o,
            (_, true) => self,
            _ => Area { x0: self.x0.min(o.x0), x1: self.x1.max(o.x1), y0: self.y0.min(o.y0), y1: self.y1.max(o.y1) },
        }
    }

    fn contains(&self, x: usize, y: usize) -> bool {
        (self.x0..self.x1).contains(&x) && (self.y0..self.y1).contains(&y)
    }
}

const NO_AREA: Area = Area { x0: 0, x1: 0, y0: 0, y1: 0 };

/// What a stroke's dabs left at a pixel: the coverage built up with the flow
/// (1 - Π(1 - flow · falloff)) and the largest falloff of any dab (the brush's shape across
/// the stroke). Both start at 0.
type Cover = [f32; 2];

/// The geometry of a stroke on a raster covering `rect` (x, y, w, h; normalized source
/// coordinates) of a source with aspect ratio `aspect`; positions in long-edge units.
struct Dabber<'a> {
    w: usize,
    h: usize,
    rect: [f32; 4],
    kx: f32,
    ky: f32,
    r: f32,
    inner: f32,
    flow: f32,
    guide: Option<&'a AutoGuide>,
}

impl<'a> Dabber<'a> {
    fn new(w: usize, h: usize, rect: [f32; 4], aspect: f32, stroke: &BrushStroke, guide: Option<&'a AutoGuide>) -> Self {
        let (kx, ky) = if aspect >= 1.0 { (1.0, 1.0 / aspect) } else { (aspect, 1.0) };
        let r = stroke.radius.max(1e-5);
        let inner = r * stroke.hardness.clamp(0.0, 0.99);
        Self { w, h, rect, kx, ky, r, inner, flow: stroke.flow.clamp(0.0, 1.0), guide: guide.filter(|_| stroke.auto_mask) }
    }

    /// Dabs every 15 % of the radius along the smoothed path, and how many stay the same when
    /// points are added.
    fn dabs(&self, points: &[[f32; 2]]) -> (Vec<(f32, f32)>, usize) {
        let pts: Vec<(f32, f32)> = points.iter().map(|p| (p[0] * self.kx, p[1] * self.ky)).collect();
        // Raster pixel size in long-edge units (at least a fraction of a pixel between dabs).
        let px = (self.rect[2] * self.kx / self.w as f32).min(self.rect[3] * self.ky / self.h as f32);
        smooth_path_stable(&pts, (self.r * 0.15).max(px * 0.5))
    }

    /// Color under the center of each dab (automatic masking).
    fn refs(&self, dabs: &[(f32, f32)]) -> Vec<[f32; 3]> {
        self.guide.map_or_else(Vec::new, |g| dabs.iter().map(|d| g.at(d.0 / self.kx, d.1 / self.ky)).collect())
    }

    fn to_x(&self, u: f32) -> f32 {
        (u / self.kx - self.rect[0]) / self.rect[2] * self.w as f32
    }

    fn to_y(&self, v: f32) -> f32 {
        (v / self.ky - self.rect[1]) / self.rect[3] * self.h as f32
    }

    /// Raster pixels the dabs touch.
    fn area(&self, dabs: &[(f32, f32)]) -> Area {
        if dabs.is_empty() {
            return NO_AREA;
        }
        let (mut x0, mut x1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for d in dabs {
            x0 = x0.min(self.to_x(d.0 - self.r));
            x1 = x1.max(self.to_x(d.0 + self.r));
            y0 = y0.min(self.to_y(d.1 - self.r));
            y1 = y1.max(self.to_y(d.1 + self.r));
        }
        let clamp = |v: f32, n: usize| (v.max(0.0) as usize).min(n);
        Area { x0: clamp(x0.floor(), self.w), x1: clamp(x1.ceil(), self.w), y0: clamp(y0.floor(), self.h), y1: clamp(y1.ceil(), self.h) }
    }

    /// Adds the dabs to the pixels `row` of raster row `y`, which start at column `x_off`.
    /// Returns the columns touched.
    fn add_row(&self, y: usize, dabs: &[(f32, f32)], refs: &[[f32; 3]], row: &mut [Cover], x_off: usize) -> (usize, usize) {
        let v = (self.rect[1] + (y as f32 + 0.5) / self.h as f32 * self.rect[3]) * self.ky;
        let r = self.r;
        let mut span = (usize::MAX, 0usize);
        for (k, d) in dabs.iter().enumerate() {
            let dy = v - d.1;
            if dy.abs() >= r {
                continue;
            }
            let half = (r * r - dy * dy).sqrt();
            let x0 = (self.to_x(d.0 - half).floor().max(0.0) as usize).max(x_off);
            let x1 = (self.to_x(d.0 + half).ceil().max(0.0) as usize).min(x_off + row.len());
            if x0 >= x1 {
                continue;
            }
            span = (span.0.min(x0), span.1.max(x1));
            for x in x0..x1 {
                let u = (self.rect[0] + (x as f32 + 0.5) / self.w as f32 * self.rect[2]) * self.kx;
                let dist = ((u - d.0).powi(2) + dy * dy).sqrt();
                let mut f = 1.0 - smoothstep(self.inner, r, dist);
                if let Some(g) = self.guide
                    && f > 0.0
                {
                    f *= similarity(g.at(u / self.kx, v / self.ky), refs[k]);
                }
                let c = &mut row[x - x_off];
                c[0] += self.flow * f * (1.0 - c[0]);
                c[1] = c[1].max(f);
            }
        }
        span
    }
}

/// Runs `f` on each row (`w` wide) of `data` with its index, in parallel only for large
/// areas: waking the threads costs more than painting a few dabs.
fn for_rows<T: Send>(data: &mut [T], w: usize, work: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    if work < 400_000 {
        data.chunks_mut(w).enumerate().for_each(|(i, row)| f(i, row));
    } else {
        data.par_chunks_mut(w).enumerate().for_each(|(i, row)| f(i, row));
    }
}

/// Puts a stroke's coverage onto a mask value.
#[inline]
fn compose(m: f32, c: Cover, stroke: &BrushStroke) -> f32 {
    let c = c[0].min(c[1]) * stroke.opacity.clamp(0.0, 1.0);
    if stroke.erase { m * (1.0 - c) } else { m + (1.0 - m) * c }
}

/// Paints a stroke into a raster covering `rect` (x, y, w, h; normalized source coordinates)
/// of a source with aspect ratio `aspect`. Dabs every 15 % of the radius along the smoothed
/// path; within the stroke, coverage builds up with the flow (1 - Π(1 - flow · falloff)), but
/// never beyond the brush's own falloff (the most of any dab), so the soft edge stays as soft as
/// the hardness says however densely the dabs overlap; capped by the opacity.
pub fn paint_stroke(data: &mut [f32], w: usize, h: usize, rect: [f32; 4], aspect: f32, stroke: &BrushStroke, guide: Option<&AutoGuide>) {
    if stroke.points.is_empty() || w == 0 || h == 0 {
        return;
    }
    let dabber = Dabber::new(w, h, rect, aspect, stroke, guide);
    let (dabs, _) = dabber.dabs(&stroke.points);
    let refs = dabber.refs(&dabs);
    let area = dabber.area(&dabs);
    if area.is_empty() {
        return;
    }
    data[area.y0 * w..area.y1 * w].par_chunks_mut(w).enumerate().for_each(|(i, row)| {
        let mut cover = vec![[0.0f32; 2]; area.x1 - area.x0];
        let (x0, x1) = dabber.add_row(area.y0 + i, &dabs, &refs, &mut cover, area.x0);
        for x in x0..x1 {
            row[x] = compose(row[x], cover[x - area.x0], stroke);
        }
    });
}

/// A stroke being painted on a whole-source raster, kept between updates: each new point
/// only adds its dabs instead of the whole stroke being painted again (which made long strokes
/// slower and slower).
pub struct LiveStroke {
    w: usize,
    h: usize,
    aspect: f32,
    /// The stroke as last painted.
    stroke: Option<BrushStroke>,
    /// Coverage of the dabs that no longer change (see [`smooth_path_stable`]), whole raster.
    cover: Vec<Cover>,
    /// How many dabs `cover` holds.
    stable: usize,
    /// Pixels the changing last dabs touched in the last output.
    tail: Area,
}

impl LiveStroke {
    pub fn new(w: usize, h: usize, aspect: f32) -> Self {
        Self { w, h, aspect, stroke: None, cover: Vec::new(), stable: 0, tail: NO_AREA }
    }

    /// Whether `stroke` is the painted stroke with points added (or the same), on a raster of
    /// this size.
    pub fn continues(&self, stroke: &BrushStroke, w: usize, h: usize, aspect: f32) -> bool {
        let Some(last) = &self.stroke else { return false };
        let brush = |s: &BrushStroke| (s.radius, s.hardness, s.flow, s.erase, s.opacity, s.auto_mask, s.build_up);
        (w, h, aspect) == (self.w, self.h, self.aspect) && brush(stroke) == brush(last) && stroke.points.starts_with(&last.points)
    }

    /// Brings `out` (`base` with the stroke as last painted, or `base` itself for a new one)
    /// to `base` with `stroke`, which must continue the last one (see [`Self::continues`]).
    /// Returns the rows that changed.
    pub fn update(&mut self, out: &mut [f32], base: &[f32], stroke: &BrushStroke, guide: Option<&AutoGuide>) -> std::ops::Range<usize> {
        let w = self.w;
        if stroke.points.is_empty() || w == 0 || self.h == 0 {
            return 0..0;
        }
        if self.cover.is_empty() {
            self.cover = vec![[0.0; 2]; w * self.h];
        }
        let dabber = Dabber::new(w, self.h, [0.0, 0.0, 1.0, 1.0], self.aspect, stroke, guide);
        let (dabs, stable) = dabber.dabs(&stroke.points);
        // The dabs that became stable go into the kept coverage.
        let fresh = &dabs[self.stable.min(stable)..stable];
        let fresh_area = dabber.area(fresh);
        if !fresh_area.is_empty() {
            let refs = dabber.refs(fresh);
            let work = (fresh_area.x1 - fresh_area.x0) * (fresh_area.y1 - fresh_area.y0) * fresh.len();
            for_rows(&mut self.cover[fresh_area.y0 * w..fresh_area.y1 * w], w, work, |i, row| {
                dabber.add_row(fresh_area.y0 + i, fresh, &refs, row, 0);
            });
        }
        // The last dabs on a copy of the kept coverage around them.
        let tail_dabs = &dabs[stable..];
        let tail = dabber.area(tail_dabs);
        let tw = tail.x1.saturating_sub(tail.x0);
        let mut tail_cover: Vec<Cover> = Vec::new();
        if !tail.is_empty() {
            let refs = dabber.refs(tail_dabs);
            tail_cover = (tail.y0..tail.y1).flat_map(|y| self.cover[y * w + tail.x0..y * w + tail.x1].iter().copied()).collect();
            let work = tail_cover.len() * tail_dabs.len();
            for_rows(&mut tail_cover, tw, work, |i, row| {
                dabber.add_row(tail.y0 + i, tail_dabs, &refs, row, tail.x0);
            });
        }
        // Everything that changed since the last output, composed anew from the base.
        let changed = fresh_area.union(self.tail).union(tail);
        if !changed.is_empty() {
            let cover = &self.cover;
            let work = (changed.x1 - changed.x0) * (changed.y1 - changed.y0);
            for_rows(&mut out[changed.y0 * w..changed.y1 * w], w, work, |i, row| {
                let y = changed.y0 + i;
                for x in changed.x0..changed.x1 {
                    let c = if tail.contains(x, y) { tail_cover[(y - tail.y0) * tw + x - tail.x0] } else { cover[y * w + x] };
                    row[x] = compose(base[y * w + x], c, stroke);
                }
            });
        }
        self.stroke = Some(stroke.clone());
        self.stable = stable;
        self.tail = tail;
        if changed.is_empty() { 0..0 } else { changed.y0..changed.y1 }
    }
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

    /// Across a stroke the coverage falls off like a single dab: the overlapping dabs along
    /// the path must not fill up the soft edge (hardness 0 looked like about 0.5 before).
    #[test]
    fn stroke_keeps_the_soft_edge_of_its_dabs() {
        let (w, h) = (200, 200);
        let soft = BrushStroke { hardness: 0.0, ..stroke(vec![[0.2, 0.5], [0.8, 0.5]]) };
        let mut line = vec![0.0; w * h];
        paint_stroke(&mut line, w, h, [0.0, 0.0, 1.0, 1.0], 1.0, &soft, None);
        let mut dab = vec![0.0; w * h];
        paint_stroke(&mut dab, w, h, [0.0, 0.0, 1.0, 1.0], 1.0, &BrushStroke { points: vec![[0.5, 0.5]], ..soft.clone() }, None);
        // Radius 10 px: compare 3, 5 and 7 px off the path.
        for dy in [3, 5, 7] {
            let (l, d) = (line[(100 + dy) * w + 100], dab[(100 + dy) * w + 100]);
            assert!((l - d).abs() < 0.03, "{dy} px: Strich {l}, Tupfer {d}");
        }
        assert!(line[105 * w + 100] < 0.6, "halber Radius: {}", line[105 * w + 100]);
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
