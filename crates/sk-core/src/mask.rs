//! Mask rasterization (brush strokes, gradients, AI bitmaps) and retouching (heal/clone).
//!
//! Masks live in normalized source coordinates, so they stay attached to the image
//! content regardless of crop, rotation or lens correction.

use std::sync::{Arc, LazyLock, Mutex};

use rayon::prelude::*;

use crate::geometry::{GeoMap, Region};
use crate::image::LinearImage;
use crate::params::{Mask, Spot, SpotMode};

/// Long edge of rasterized masks.
pub const MASK_RES: usize = 3072;

#[derive(Clone, Debug)]
pub struct MaskBitmap {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
    /// Identifies this content (a new number for every change), so that a copy elsewhere
    /// (on the graphics card) can tell whether it is current.
    pub version: u64,
    /// The bitmap of version `.0` with only the rows `.1` changed: a copy of that version
    /// needs only those rows (while a stroke is painted).
    pub changed: Option<(u64, std::ops::Range<usize>)>,
}

/// A new [`MaskBitmap::version`].
fn next_version() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl MaskBitmap {
    pub fn new(w: usize, h: usize, data: Vec<f32>) -> Self {
        Self { w, h, data, version: next_version(), changed: None }
    }

    /// Bilinear sample at normalized source coordinates.
    #[inline]
    pub fn sample(&self, u: f32, v: f32) -> f32 {
        let x = (u * self.w as f32 - 0.5).clamp(0.0, (self.w - 1) as f32);
        let y = (v * self.h as f32 - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (x as usize, y as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = (x - x0 as f32, y - y0 as f32);
        let at = |xx: usize, yy: usize| self.data[yy * self.w + xx];
        let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
        let bot = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
        top + (bot - top) * ty
    }
}

#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if e1 <= e0 {
        return if x < e0 { 0.0 } else { 1.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn bitmap_size(aspect: f32) -> (usize, usize) {
    if aspect >= 1.0 {
        (MASK_RES, ((MASK_RES as f32 / aspect).round() as usize).max(1))
    } else {
        (((MASK_RES as f32 * aspect).round() as usize).max(1), MASK_RES)
    }
}

/// Rasterizes the drawn parts of a mask (fill, gradients, AI, strokes) for a source of the
/// given aspect ratio (width / height). Range refinements and inversion are applied per pixel.
pub fn rasterize(mask: &Mask, aspect: f32, guide: Option<&crate::brush::AutoGuide>) -> MaskBitmap {
    let (w, h) = bitmap_size(aspect);
    let long = w.max(h) as f32;
    let mut data = vec![if mask.fill { 1.0f32 } else { 0.0 }; w * h];

    if !mask.linear.is_empty() || !mask.radial.is_empty() || mask.ai.is_some() {
        data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let py = y as f32 + 0.5;
            for (x, m) in row.iter_mut().enumerate() {
                let px = x as f32 + 0.5;
                for g in &mask.linear {
                    let (sx, sy) = (g.start[0] * w as f32, g.start[1] * h as f32);
                    let (dx, dy) = (g.end[0] * w as f32 - sx, g.end[1] * h as f32 - sy);
                    let len2 = (dx * dx + dy * dy).max(1e-6);
                    let t = ((px - sx) * dx + (py - sy) * dy) / len2;
                    *m = m.max(1.0 - smoothstep(0.0, 1.0, t));
                }
                for g in &mask.radial {
                    let (cx, cy) = (g.center[0] * w as f32, g.center[1] * h as f32);
                    let (rx, ry) = ((g.radius_x * long).max(1.0), (g.radius_y * long).max(1.0));
                    let (sin, cos) = g.angle.sin_cos();
                    let (dx, dy) = (px - cx, py - cy);
                    let (lx, ly) = (dx * cos + dy * sin, dy * cos - dx * sin);
                    let e = ((lx / rx).powi(2) + (ly / ry).powi(2)).sqrt();
                    let v = 1.0 - smoothstep(1.0 - g.feather.clamp(0.0, 1.0), 1.0, e);
                    *m = m.max(if g.invert { 1.0 - v } else { v });
                }
                if let Some(ai) = &mask.ai
                    && ai.width > 0
                    && ai.height > 0
                {
                    let ax = ((px / w as f32) * ai.width as f32) as usize;
                    let ay = ((py / h as f32) * ai.height as f32) as usize;
                    let v = ai.data[ay.min(ai.height - 1) * ai.width + ax.min(ai.width - 1)] as f32 / 255.0;
                    *m = m.max(v);
                }
            }
        });
    }

    let mut bm = MaskBitmap::new(w, h, data);
    for stroke in &mask.strokes {
        paint_stroke_into(&mut bm, stroke, aspect, guide);
    }
    bm
}

/// Adds one stroke to a bitmap of the whole source.
fn paint_stroke_into(bm: &mut MaskBitmap, stroke: &crate::params::BrushStroke, aspect: f32, guide: Option<&crate::brush::AutoGuide>) {
    let (w, h) = (bm.w, bm.h);
    if stroke.build_up {
        crate::brush::paint_stroke(&mut bm.data, w, h, [0.0, 0.0, 1.0, 1.0], aspect, stroke, guide);
        return;
    }
    let long = w.max(h) as f32;
    let data = &mut bm.data;
    {
        let r = (stroke.radius * long).max(0.5);
        let inner = r * stroke.hardness.clamp(0.0, 0.99);
        // Bounding box of the stroke in bitmap pixels.
        let pts: Vec<(f32, f32)> = stroke.points.iter().map(|p| (p[0] * w as f32, p[1] * h as f32)).collect();
        if pts.is_empty() {
            return;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &(x, y) in &pts {
            x0 = x0.min(x - r);
            y0 = y0.min(y - r);
            x1 = x1.max(x + r);
            y1 = y1.max(y + r);
        }
        let (bx0, by0) = (x0.floor().max(0.0) as usize, y0.floor().max(0.0) as usize);
        let (bx1, by1) = ((x1.ceil() as usize).min(w), (y1.ceil() as usize).min(h));
        if bx1 <= bx0 || by1 <= by0 {
            return;
        }
        let bw = bx1 - bx0;
        // Coverage of the stroke: max over the polyline (stamps do not accumulate within a stroke).
        let segs: Vec<((f32, f32), (f32, f32))> =
            if pts.len() == 1 { vec![(pts[0], pts[0])] } else { pts.windows(2).map(|p| (p[0], p[1])).collect() };
        let cov: Vec<f32> = (0..bw * (by1 - by0))
            .into_par_iter()
            .map(|i| {
                let (px, py) = ((bx0 + i % bw) as f32 + 0.5, (by0 + i / bw) as f32 + 0.5);
                let mut d2min = f32::MAX;
                for &((ax, ay), (cx, cy)) in &segs {
                    let (dx, dy) = (cx - ax, cy - ay);
                    let len2 = dx * dx + dy * dy;
                    let t = if len2 > 0.0 { (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
                    let (qx, qy) = (ax + dx * t - px, ay + dy * t - py);
                    d2min = d2min.min(qx * qx + qy * qy);
                }
                1.0 - smoothstep(inner, r, d2min.sqrt())
            })
            .collect();
        let flow = stroke.flow.clamp(0.0, 1.0);
        for y in by0..by1 {
            for x in bx0..bx1 {
                let c = cov[(y - by0) * bw + x - bx0] * flow;
                let m = &mut data[y * w + x];
                *m = if stroke.erase { *m * (1.0 - c) } else { *m + (1.0 - *m) * c };
            }
        }
    }
}

/// Keys of a mask and of the same mask without its last stroke (see [`cached_bitmap`]), in one
/// pass over the strokes.
fn mask_keys(mask: &Mask, aspect: f32, src: Option<&LinearImage>) -> (u64, u64) {
    let drawn = serde_json::json!({ "f": mask.fill, "l": mask.linear, "r": mask.radial });
    let ai = mask.ai.as_ref().map_or(0, |a| ai_hash(&a.data) ^ (a.width as u64) << 32 ^ a.height as u64);
    let base = crate::cache::fnv1a(format!("{drawn}|{ai}|{aspect:.5}").as_bytes());
    let fold = |strokes: &[crate::params::BrushStroke]| {
        strokes.iter().fold(base, |h, s| (h ^ crate::cache::fnv1a(&serde_json::to_vec(s).unwrap_or_default())).wrapping_mul(0x0100_0000_01b3))
    };
    // Automatic masking follows the image's edges: the same strokes on another image (pasted
    // layer, batch export) give another mask.
    let fingerprint = std::cell::OnceCell::new();
    let image = |strokes: &[crate::params::BrushStroke]| {
        src.filter(|_| strokes.iter().any(|s| s.auto_mask)).map_or(0, |src| *fingerprint.get_or_init(|| image_fingerprint(src)))
    };
    let rest = mask.strokes.split_last().map_or(&[][..], |(_, rest)| rest);
    let rest_hash = fold(rest);
    let all_hash = match mask.strokes.last() {
        Some(last) => (rest_hash ^ crate::cache::fnv1a(&serde_json::to_vec(last).unwrap_or_default())).wrapping_mul(0x0100_0000_01b3),
        None => rest_hash,
    };
    let key = |h: u64, image: u64| crate::cache::fnv1a(format!("{h}|{image}").as_bytes());
    (key(all_hash, image(&mask.strokes)), key(rest_hash, image(rest)))
}

/// Hash of the bitmap of an AI mask. Its data never changes once made (see `AiMask`), so the
/// hash is kept per allocation: hashing megabytes each rendering costs milliseconds.
fn ai_hash(data: &Arc<Vec<u8>>) -> u64 {
    type Hashes = Vec<(std::sync::Weak<Vec<u8>>, u64)>;
    static HASHES: Mutex<Hashes> = Mutex::new(Vec::new());
    let mut hashes = HASHES.lock().unwrap();
    // A weak reference keeps its allocation, so no other data can be at that address meanwhile.
    hashes.retain(|(w, _)| w.strong_count() > 0);
    if let Some((_, h)) = hashes.iter().find(|(w, _)| std::ptr::eq(w.as_ptr(), Arc::as_ptr(data))) {
        return *h;
    }
    let h = crate::cache::fnv1a(data);
    hashes.push((Arc::downgrade(data), h));
    h
}

/// Hash of the size and about 4096 evenly spread pixels of an image.
fn image_fingerprint(img: &LinearImage) -> u64 {
    let step = (img.data.len() / 4096).max(1);
    let bytes: Vec<u8> = img.data.iter().step_by(step).flat_map(|p| p.iter().flat_map(|v| v.to_le_bytes())).collect();
    crate::cache::fnv1a(&bytes) ^ ((img.width as u64) << 32 | img.height as u64)
}

type MaskCache = Mutex<Vec<(u64, Arc<MaskBitmap>)>>;

static MASK_CACHE: LazyLock<MaskCache> = LazyLock::new(|| Mutex::new(Vec::new()));

fn cache_lookup(key: u64) -> Option<Arc<MaskBitmap>> {
    let mut cache = MASK_CACHE.lock().unwrap();
    let pos = cache.iter().position(|(k, _)| *k == key)?;
    let entry = cache.remove(pos);
    let bm = entry.1.clone();
    cache.push(entry);
    Some(bm)
}

/// Rasterized mask from a small LRU cache (rasterization only runs when the mask changes).
/// While a stroke is being painted only that stroke is drawn again: the mask without it stays
/// in the cache, and the stroke itself only gets the dabs of its new points (see
/// [`crate::brush::LiveStroke`]). `src` (the oriented camera image) is needed for automatic
/// masking.
pub fn cached_bitmap(mask: &Mask, aspect: f32, src: Option<&LinearImage>) -> Arc<MaskBitmap> {
    let (key, prefix_key) = mask_keys(mask, aspect, src);
    if let Some(bm) = cache_lookup(key) {
        return bm;
    }
    let guide = mask.strokes.iter().any(|s| s.auto_mask).then(|| src.map(cached_guide)).flatten();
    let guide = guide.as_deref();
    let prefix = mask.strokes.split_last().map(|(last, rest)| {
        // Not in the cache (the first stroke of a layer, or a mask just loaded): made and kept,
        // so that the next points of the stroke find it.
        let base = cache_lookup(prefix_key).unwrap_or_else(|| {
            let without = Mask { strokes: rest.to_vec(), ..mask.clone() };
            let bm = Arc::new(rasterize(&without, aspect, guide));
            cache_insert(prefix_key, bm.clone());
            bm
        });
        (base, last, prefix_key)
    });
    let bm = match prefix {
        Some((base, last, prefix_key)) if last.build_up => paint_live(base, last, prefix_key, key, aspect, guide),
        Some((base, last, _)) => {
            let mut b = MaskBitmap::new(base.w, base.h, base.data.clone());
            paint_stroke_into(&mut b, last, aspect, guide);
            Arc::new(b)
        }
        None => Arc::new(rasterize(mask, aspect, guide)),
    };
    cache_insert(key, bm.clone());
    bm
}

fn cache_insert(key: u64, bm: Arc<MaskBitmap>) {
    let mut cache = MASK_CACHE.lock().unwrap();
    cache.push((key, bm));
    if cache.len() > 24 {
        cache.remove(0);
    }
}

/// The stroke being painted: the mask it is painted on (by its key), the last result and
/// the stroke's state.
struct Live {
    prefix_key: u64,
    out: Arc<MaskBitmap>,
    out_key: u64,
    stroke: crate::brush::LiveStroke,
}

static LIVE: LazyLock<Mutex<Option<Live>>> = LazyLock::new(|| Mutex::new(None));

/// `base` with the stroke `last` on top: continuing the stroke painted last time if `last`
/// extends it, else starting anew.
fn paint_live(
    base: Arc<MaskBitmap>,
    last: &crate::params::BrushStroke,
    prefix_key: u64,
    key: u64,
    aspect: f32,
    guide: Option<&crate::brush::AutoGuide>,
) -> Arc<MaskBitmap> {
    let mut live = LIVE.lock().unwrap();
    let (w, h) = (base.w, base.h);
    let continued = live.as_ref().is_some_and(|l| l.prefix_key == prefix_key && l.stroke.continues(last, w, h, aspect));
    if continued {
        // The stroke's previous state is not needed any more; dropping it keeps the cache from
        // filling with the in-between states of one stroke.
        let old = live.as_ref().unwrap().out_key;
        MASK_CACHE.lock().unwrap().retain(|(k, _)| *k != old);
    } else {
        *live = Some(Live { prefix_key, out: base.clone(), out_key: key, stroke: crate::brush::LiveStroke::new(w, h, aspect) });
    }
    let l = live.as_mut().unwrap();
    l.out_key = key;
    // Usually nothing else holds the previous state any more: then it is changed in place
    // instead of copying the whole mask.
    let mut out = std::mem::replace(&mut l.out, base.clone());
    let bm = Arc::make_mut(&mut out);
    let rows = l.stroke.update(&mut bm.data, &base.data, last, guide);
    // A new stroke starts from `base`; the rows are what changed against the bitmap copied.
    bm.changed = Some((bm.version, rows));
    bm.version = next_version();
    l.out = out.clone();
    out
}

type GuideCache = Mutex<Option<(u64, Arc<crate::brush::AutoGuide>)>>;

/// Automatic masking's view of the image, made once per image (not on every update of a stroke).
fn cached_guide(src: &LinearImage) -> Arc<crate::brush::AutoGuide> {
    static GUIDE: LazyLock<GuideCache> = LazyLock::new(|| Mutex::new(None));
    let key = image_fingerprint(src);
    let mut g = GUIDE.lock().unwrap();
    if let Some((k, guide)) = g.as_ref()
        && *k == key
    {
        return guide.clone();
    }
    let guide = Arc::new(crate::brush::AutoGuide::from_image(src, 1024));
    *g = Some((key, guide.clone()));
    guide
}

// ------------------------------------------------------------------ retouching

fn bilinear(img: &LinearImage, x: f32, y: f32) -> [f32; 3] {
    let x = x.clamp(0.0, (img.width - 1) as f32);
    let y = y.clamp(0.0, (img.height - 1) as f32);
    let (x0, y0) = (x as usize, y as usize);
    let (x1, y1) = ((x0 + 1).min(img.width - 1), (y0 + 1).min(img.height - 1));
    let (tx, ty) = (x - x0 as f32, y - y0 as f32);
    let at = |xx: usize, yy: usize| img.data[yy * img.width + xx];
    std::array::from_fn(|c| {
        let top = at(x0, y0)[c] + (at(x1, y0)[c] - at(x0, y0)[c]) * tx;
        let bot = at(x0, y1)[c] + (at(x1, y1)[c] - at(x0, y1)[c]) * tx;
        top + (bot - top) * ty
    })
}

/// Mean color of a ring around `c` (source pixels) with radii r0..r1.
fn ring_mean(src: &LinearImage, c: (f32, f32), r0: f32, r1: f32) -> [f32; 3] {
    let mut acc = [0.0f32; 3];
    let mut n = 0.0;
    for k in 0..64 {
        let a = k as f32 / 64.0 * std::f32::consts::TAU;
        for r in [r0, (r0 + r1) / 2.0, r1] {
            let p = bilinear(src, c.0 + a.cos() * r, c.1 + a.sin() * r);
            acc = [acc[0] + p[0], acc[1] + p[1], acc[2] + p[2]];
            n += 1.0;
        }
    }
    acc.map(|v| v / n)
}

/// A segment of an area's center line in source pixels, with its box grown by the radius.
type Segment = ((f32, f32), (f32, f32), (f32, f32, f32, f32));

/// Polylines with a radius, for the distance of a pixel from the nearest of them.
struct Shape {
    /// The segments in source pixels (a single point is one of zero length), each with its
    /// bounding box grown by the radius, (x0, y0, x1, y1): a pixel only needs the few
    /// segments whose box contains it.
    segments: Vec<Segment>,
    /// Grid over `bounds` with cells of `cell` pixels, listing the segments whose box
    /// overlaps each cell.
    grid: Vec<Vec<u32>>,
    grid_w: usize,
    cell: f32,
    /// Bounding box including the radius: (x0, y0, x1, y1).
    bounds: (f32, f32, f32, f32),
}

impl Shape {
    /// `None` without lines.
    fn new(lines: &[Line], r: f32) -> Option<Self> {
        let points = || lines.iter().flatten();
        points().next()?;
        let bounds = points().fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |b, p| (b.0.min(p.0 - r), b.1.min(p.1 - r), b.2.max(p.0 + r), b.3.max(p.1 + r)));
        let segments: Vec<Segment> = lines
            .iter()
            .flat_map(|pts| if pts.len() == 1 { vec![(pts[0], pts[0])] } else { pts.windows(2).map(|w| (w[0], w[1])).collect() })
            .map(|(a, b)| (a, b, (a.0.min(b.0) - r, a.1.min(b.1) - r, a.0.max(b.0) + r, a.1.max(b.1) + r)))
            .collect();
        let cell = (2.0 * r).max(4.0);
        let grid_w = ((bounds.2 - bounds.0) / cell).ceil().max(1.0) as usize;
        let grid_h = ((bounds.3 - bounds.1) / cell).ceil().max(1.0) as usize;
        let mut grid = vec![Vec::new(); grid_w * grid_h];
        let to_cell = |v: f32, o: f32, n: usize| (((v - o) / cell).max(0.0) as usize).min(n - 1);
        for (i, (_, _, (x0, y0, x1, y1))) in segments.iter().enumerate() {
            for gy in to_cell(*y0, bounds.1, grid_h)..=to_cell(*y1, bounds.1, grid_h) {
                for gx in to_cell(*x0, bounds.0, grid_w)..=to_cell(*x1, bounds.0, grid_w) {
                    grid[gy * grid_w + gx].push(i as u32);
                }
            }
        }
        Some(Self { segments, grid, grid_w, cell, bounds })
    }

    /// Distance of `p` from the nearest line (`f32::MAX` when farther than the radius).
    fn distance(&self, p: (f32, f32)) -> f32 {
        let (x0, y0, x1, y1) = self.bounds;
        if p.0 < x0 || p.0 > x1 || p.1 < y0 || p.1 > y1 {
            return f32::MAX;
        }
        let gx = ((p.0 - x0) / self.cell) as usize;
        let gy = ((p.1 - y0) / self.cell) as usize;
        let Some(list) = self.grid.get(gy * self.grid_w + gx.min(self.grid_w - 1)) else { return f32::MAX };
        let mut best = f32::MAX;
        for &(a, b, (x0, y0, x1, y1)) in list.iter().map(|&i| &self.segments[i as usize]) {
            if p.0 < x0 || p.0 > x1 || p.1 < y0 || p.1 > y1 {
                continue;
            }
            let q = closest_on_segment(p, a, b);
            best = best.min((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2));
        }
        if best == f32::MAX { best } else { best.sqrt() }
    }
}

struct PreparedSpot {
    /// The area, and what strokes took away from it.
    area: Shape,
    erased: Option<Shape>,
    offset: (f32, f32),
    r: f32,
    inner: f32,
    /// Per-pixel color gain for heal areas (none for clone).
    heal: Option<Arc<HealField>>,
    /// Clone: the vignetting gain of the source pixels over the area (see [`GainGrid`]).
    clone_gain: Option<GainGrid>,
    /// Output pixels that can show the area (see [`output_box`]).
    output: Option<(f32, f32, f32, f32)>,
    opacity: f32,
}

/// `GeoMap::source_gain` on a coarse grid over a box of source pixels: that takes a search
/// for the output position, too slow for each pixel, and vignetting changes slowly.
struct GainGrid {
    origin: (f32, f32),
    cell: f32,
    w: usize,
    h: usize,
    gain: Vec<f32>,
}

impl GainGrid {
    fn new(geo: &GeoMap, b: (f32, f32, f32, f32)) -> Self {
        let cell = 8.0;
        let (w, h) = (((b.2 - b.0) / cell).ceil() as usize + 2, ((b.3 - b.1) / cell).ceil() as usize + 2);
        let gain = (0..w * h).into_par_iter().map(|i| geo.source_gain(b.0 + (i % w) as f32 * cell, b.1 + (i / w) as f32 * cell)).collect();
        Self { origin: (b.0, b.1), cell, w, h, gain }
    }

    fn sample(&self, x: f32, y: f32) -> f32 {
        let gx = ((x - self.origin.0) / self.cell).clamp(0.0, (self.w - 1) as f32);
        let gy = ((y - self.origin.1) / self.cell).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (gx as usize, gy as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = (gx - x0 as f32, gy - y0 as f32);
        let at = |xx: usize, yy: usize| self.gain[yy * self.w + xx];
        let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
        let bot = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
        top + (bot - top) * ty
    }
}

/// Output pixels (x0, y0, x1, y1) that can show the box `b` of source pixels: its outline
/// mapped to the output, with a margin for the bending between the samples. `None` when that
/// can't be told reliably (then every pixel is checked).
fn output_box(geo: &GeoMap, b: (f32, f32, f32, f32)) -> Option<(f32, f32, f32, f32)> {
    let n = 32;
    let mut o = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for k in 0..4 * n {
        let t = (k % n) as f32 / n as f32;
        let p = match k / n {
            0 => (b.0 + (b.2 - b.0) * t, b.1),
            1 => (b.2, b.1 + (b.3 - b.1) * t),
            2 => (b.2 - (b.2 - b.0) * t, b.3),
            _ => (b.0, b.3 - (b.3 - b.1) * t),
        };
        let q = geo.output_point(p.0, p.1);
        let back = geo.source_point(q.0, q.1);
        if !q.0.is_finite() || !q.1.is_finite() || (back.0 - p.0).abs() > 0.5 || (back.1 - p.1).abs() > 0.5 {
            return None;
        }
        o = (o.0.min(q.0), o.1.min(q.1), o.2.max(q.0), o.3.max(q.1));
    }
    let m = 2.0 + 0.05 * (o.2 - o.0).max(o.3 - o.1);
    Some((o.0 - m, o.1 - m, o.2 + m, o.3 + m))
}

/// Whether the output pixel (x, y) can be in one of `boxes` (an unknown box takes all).
fn in_any(boxes: &[Option<(f32, f32, f32, f32)>], x: f32, y: f32) -> bool {
    boxes.iter().any(|b| b.is_none_or(|b| x >= b.0 && x <= b.2 && y >= b.1 && y <= b.3))
}

type HealCache = Mutex<Vec<(u64, Arc<HealField>)>>;

/// [`HealField::solve`] from a small cache: it is the slow part of a heal area and would
/// otherwise run again for every area on every change of any of them.
fn cached_heal(src: &LinearImage, segments: &[Segment], offset: (f32, f32), r: f32) -> Arc<HealField> {
    static CACHE: LazyLock<HealCache> = LazyLock::new(|| Mutex::new(Vec::new()));
    let mut h = image_fingerprint(src);
    let mut add = |v: f32| h = (h ^ v.to_bits() as u64).wrapping_mul(0x0100_0000_01b3);
    for (a, b, _) in segments {
        [a.0, a.1, b.0, b.1].into_iter().for_each(&mut add);
    }
    [offset.0, offset.1, r].into_iter().for_each(&mut add);
    if let Some((_, f)) = CACHE.lock().unwrap().iter().find(|(k, _)| *k == h) {
        return f.clone();
    }
    let f = Arc::new(HealField::solve(src, segments, offset, r));
    let mut cache = CACHE.lock().unwrap();
    cache.push((h, f.clone()));
    if cache.len() > 32 {
        cache.remove(0);
    }
    f
}

/// Color gain over a heal area on a coarse grid (source pixels): on a band just outside the
/// area it is the ratio of the target's to the source's surroundings, inside it is the
/// smooth (harmonic) interpolation of that edge. Like a Poisson blend, this keeps the
/// source texture while brightness and color gradients flow in from the edge, so no seam
/// remains even where the surroundings change across the area.
/// Grid cells of a heal area at most (about 40 bytes each).
const MAX_HEAL_CELLS: usize = 2_000_000;

struct HealField {
    origin: (f32, f32),
    cell: f32,
    w: usize,
    h: usize,
    gain: Vec<[f32; 3]>,
}

impl HealField {
    fn solve(src: &LinearImage, segments: &[Segment], offset: (f32, f32), r: f32) -> Self {
        let b = segments.iter().fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |b, (p, q, _)| {
            (b.0.min(p.0.min(q.0)), b.1.min(p.1.min(q.1)), b.2.max(p.0.max(q.0)), b.3.max(p.1.max(q.1)))
        });
        // The grid covers the stroke's bounding box: a thin stroke across the whole image would
        // take millions of cells (hundreds of MB) at the usual fineness, so it gets coarser there.
        let area = (b.2 - b.0 + 4.0 * r + 4.0) * (b.3 - b.1 + 4.0 * r + 4.0);
        let cell = (r / 10.0).max(1.0).max((area / MAX_HEAL_CELLS as f32).sqrt());
        // Cells up to this far from the center line are unknown, the next ring is the edge.
        let (inside, edge) = (r + 0.75 * cell, r + 1.75 * cell);
        let margin = edge + cell;
        let origin = (b.0 - margin, b.1 - margin);
        let w = ((b.2 - b.0 + 2.0 * margin) / cell).ceil() as usize + 1;
        let h = ((b.3 - b.1 + 2.0 * margin) / cell).ceil() as usize + 1;
        let center = |i: usize| (origin.0 + (i % w) as f32 * cell, origin.1 + (i / w) as f32 * cell);
        let dist = |p: (f32, f32)| {
            segments
                .iter()
                .filter(|(_, _, (x0, y0, x1, y1))| p.0 >= x0 - edge + r && p.0 <= x1 + edge - r && p.1 >= y0 - edge + r && p.1 <= y1 + edge - r)
                .map(|&(a, b, _)| {
                    let q = closest_on_segment(p, a, b);
                    ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)).sqrt()
                })
                .fold(f32::MAX, f32::min)
        };
        // 0 = outside, 1 = edge (fixed), 2 = unknown.
        let kind: Vec<u8> = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let (x, y) = (i % w, i / w);
                if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                    return 0;
                }
                match dist(center(i)) {
                    d if d < inside => 2,
                    d if d < edge => 1,
                    _ => 0,
                }
            })
            .collect();
        // Edge values: log ratio of the mean colors over the cell, averaged in linear light so
        // texture and noise do not leak into the gain.
        let n = cell.round().max(1.0) as usize;
        let mut field: Vec<[f32; 3]> = (0..w * h)
            .into_par_iter()
            .map(|i| {
                if kind[i] != 1 {
                    return [0.0; 3];
                }
                let c = center(i);
                let (mut t, mut s) = ([0.0f32; 3], [0.0f32; 3]);
                for k in 0..n * n {
                    let p = (c.0 + ((k % n) as f32 + 0.5) / n as f32 * cell - cell / 2.0, c.1 + ((k / n) as f32 + 0.5) / n as f32 * cell - cell / 2.0);
                    let (a, b) = (bilinear(src, p.0, p.1), bilinear(src, p.0 + offset.0, p.1 + offset.1));
                    for ch in 0..3 {
                        t[ch] += a[ch];
                        s[ch] += b[ch];
                    }
                }
                std::array::from_fn(|ch| (t[ch].max(1e-6) / s[ch].max(1e-6)).ln().clamp(-4f32.ln(), 4f32.ln()))
            })
            .collect();
        let edges: Vec<usize> = (0..w * h).filter(|&i| kind[i] == 1).collect();
        let start = edges.iter().fold([0.0f32; 3], |a, &i| std::array::from_fn(|c| a[c] + field[i][c] / edges.len().max(1) as f32));
        let unknown: Vec<usize> = (0..w * h).filter(|&i| kind[i] == 2).collect();
        for &i in &unknown {
            field[i] = start;
        }
        // Laplace equation by successive over-relaxation; the field is smooth, so this
        // converges quickly on the coarse grid.
        for _ in 0..500 {
            let mut change = 0.0f32;
            for &i in &unknown {
                let (a, b, c, d) = (field[i - 1], field[i + 1], field[i - w], field[i + w]);
                for ch in 0..3 {
                    let v = (a[ch] + b[ch] + c[ch] + d[ch]) / 4.0;
                    let delta = (v - field[i][ch]) * 1.85;
                    field[i][ch] += delta;
                    change = change.max(delta.abs());
                }
            }
            if change < 1e-4 {
                break;
            }
        }
        let gain = field.into_iter().map(|v| v.map(f32::exp)).collect();
        Self { origin, cell, w, h, gain }
    }

    fn sample(&self, x: f32, y: f32) -> [f32; 3] {
        let gx = ((x - self.origin.0) / self.cell).clamp(0.0, (self.w - 1) as f32);
        let gy = ((y - self.origin.1) / self.cell).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (gx as usize, gy as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = (gx - x0 as f32, gy - y0 as f32);
        let at = |xx: usize, yy: usize| self.gain[yy * self.w + xx];
        std::array::from_fn(|c| {
            let top = at(x0, y0)[c] + (at(x1, y0)[c] - at(x0, y0)[c]) * tx;
            let bot = at(x0, y1)[c] + (at(x1, y1)[c] - at(x0, y1)[c]) * tx;
            top + (bot - top) * ty
        })
    }
}

fn closest_on_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-9 {
        return a;
    }
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0);
    (a.0 + dx * t, a.1 + dy * t)
}

type Line = Vec<(f32, f32)>;

/// The area's lines in source pixels: the first one starts at the target; then the lines
/// that took parts away.
fn spot_lines(s: &Spot, sw: f32, sh: f32) -> (Vec<Line>, Vec<Line>) {
    let px = |p: &[f32; 2]| (p[0] * sw - 0.5, p[1] * sh - 0.5);
    let mut area = vec![std::iter::once(&s.target).chain(&s.path).map(px).collect::<Vec<_>>()];
    let mut erased = Vec::new();
    for st in s.strokes.iter().filter(|st| !st.points.is_empty()) {
        let line = st.points.iter().map(px).collect();
        if st.erase { erased.push(line) } else { area.push(line) }
    }
    (area, erased)
}

/// Applies heal/clone areas to a resampled region (camera space).
pub fn apply_retouch(img: &mut LinearImage, src: &LinearImage, geo: &GeoMap, region: Region, spots: &[Spot]) {
    if spots.is_empty() {
        return;
    }
    let (sw, sh) = (src.width as f32, src.height as f32);
    let long = sw.max(sh);
    let prepared: Vec<PreparedSpot> = spots
        .iter()
        .map(|s| {
            let (lines, erased_lines) = spot_lines(s, sw, sh);
            let target = lines[0][0];
            let source = (s.source[0] * sw - 0.5, s.source[1] * sh - 0.5);
            let offset = (source.0 - target.0, source.1 - target.1);
            let r = (s.radius * long).max(1.0);
            let area = Shape::new(&lines, r).expect("the target is a point");
            let bounds = area.bounds;
            // Heal: take texture from the source but brightness/color from the target's
            // surroundings, matched along the whole edge and blended smoothly inwards.
            let heal = (s.mode == SpotMode::Heal).then(|| cached_heal(src, &area.segments, offset, r));
            let shifted = (bounds.0 + offset.0, bounds.1 + offset.1, bounds.2 + offset.0, bounds.3 + offset.1);
            let clone_gain = (s.mode != SpotMode::Heal && geo.has_vignetting()).then(|| GainGrid::new(geo, shifted));
            let output = output_box(geo, bounds);
            PreparedSpot {
                area,
                erased: Shape::new(&erased_lines, r),
                offset,
                r,
                inner: r * (1.0 - s.feather.clamp(0.0, 1.0)),
                heal,
                clone_gain,
                output,
                opacity: s.opacity.clamp(0.0, 1.0),
            }
        })
        .collect();
    let boxes: Vec<_> = prepared.iter().map(|s| s.output).collect();
    img.data.par_chunks_mut(region.w).enumerate().for_each(|(ry, line)| {
        let oy = (region.y + ry) as f32 + 0.5;
        // Rows and pixels away from every area skip the mapping to the source.
        if !boxes.iter().any(|b| b.is_none_or(|b| oy >= b.1 && oy <= b.3)) {
            return;
        }
        for (rx, px) in line.iter_mut().enumerate() {
            let ox = (region.x + rx) as f32 + 0.5;
            if !in_any(&boxes, ox, oy) {
                continue;
            }
            // Source position and the vignetting gain `resample` applied there.
            let mut pt: Option<(f32, f32, f32)> = None;
            for s in &prepared {
                let (sx, sy, vig) = *pt.get_or_insert_with(|| {
                    let ([_, g, _], gain) = geo.map(ox, oy);
                    (g[0], g[1], gain)
                });
                let d = s.area.distance((sx, sy));
                if d >= s.r {
                    continue;
                }
                let mut w = (1.0 - smoothstep(s.inner, s.r, d)) * s.opacity;
                // Taken away again: with the same soft edge.
                if let Some(e) = &s.erased {
                    w *= smoothstep(s.inner, s.r, e.distance((sx, sy)));
                    if w <= 0.0 {
                        continue;
                    }
                }
                let v = bilinear(src, sx + s.offset.0, sy + s.offset.1);
                // The source pixels lack the vignetting correction: heal matches the target's
                // uncorrected surroundings and then gets the target's correction, clone keeps the
                // brightness the source shows.
                let gain = match &s.heal {
                    Some(h) => h.sample(sx, sy).map(|g| g * vig),
                    None => [s.clone_gain.as_ref().map_or(1.0, |g| g.sample(sx + s.offset.0, sy + s.offset.1)); 3],
                };
                for c in 0..3 {
                    let repl = v[c] * gain[c];
                    px[c] += (repl - px[c]) * w;
                }
            }
        }
    });
}

/// Picks a source position for a heal spot: the nearby candidate whose surroundings are
/// most similar to the target's (in mean color) with the least texture energy.
pub fn auto_source(src: &LinearImage, target: [f32; 2], radius: f32) -> [f32; 2] {
    let (sw, sh) = (src.width as f32, src.height as f32);
    let r = (radius * sw.max(sh)).max(2.0);
    let best = best_source_px(src, (target[0] * sw, target[1] * sh), r);
    [best.0 / sw, best.1 / sh]
}

/// Source position for a painted area (`points`, normalized; the first is the target): the
/// area as a whole is treated like one large spot around its center.
pub fn auto_source_area(src: &LinearImage, points: &[[f32; 2]], radius: f32) -> [f32; 2] {
    if points.len() < 2 {
        return auto_source(src, points.first().copied().unwrap_or([0.5, 0.5]), radius);
    }
    let (sw, sh) = (src.width as f32, src.height as f32);
    let px: Vec<(f32, f32)> = points.iter().map(|p| (p[0] * sw, p[1] * sh)).collect();
    let n = px.len() as f32;
    let c = (px.iter().map(|p| p.0).sum::<f32>() / n, px.iter().map(|p| p.1).sum::<f32>() / n);
    let extent = px.iter().map(|p| ((p.0 - c.0).powi(2) + (p.1 - c.1).powi(2)).sqrt()).fold(0.0, f32::max);
    let r = (radius * sw.max(sh)).max(2.0) + extent;
    let best = best_source_px(src, c, r);
    [points[0][0] + (best.0 - c.0) / sw, points[0][1] + (best.1 - c.1) / sh]
}

/// Center (pixels) of the best source circle of radius `r` for the circle around `t`.
fn best_source_px(src: &LinearImage, t: (f32, f32), r: f32) -> (f32, f32) {
    let (sw, sh) = (src.width as f32, src.height as f32);
    let ring = ring_mean(src, t, r * 1.05, r * 1.4);
    let mut best = (f32::MAX, t);
    for dist in [2.2f32, 3.0] {
        for k in 0..12 {
            let a = k as f32 / 12.0 * std::f32::consts::TAU;
            let c = (t.0 + a.cos() * r * dist, t.1 + a.sin() * r * dist);
            if c.0 < r || c.1 < r || c.0 > sw - r || c.1 > sh - r {
                continue;
            }
            let m = ring_mean(src, c, 0.0, r);
            let e = ring_mean(src, c, r * 0.5, r);
            let color = (0..3).map(|i| (m[i] - ring[i]).abs() / ring[i].max(1e-3)).sum::<f32>();
            let texture = (0..3).map(|i| (m[i] - e[i]).abs() / m[i].max(1e-3)).sum::<f32>();
            let score = color + texture * 0.5 + dist * 0.05;
            if score < best.0 {
                best = (score, c);
            }
        }
    }
    best.1
}

// ------------------------------------------------------------------ removals (generative fill)

/// Coverage of brush strokes over the part `rect` (x, y, w, h; normalized source coordinates)
/// of a source with aspect ratio `aspect`, rasterized at `w` x `h`.
pub fn rasterize_strokes_in(
    strokes: &[crate::params::BrushStroke],
    rect: [f32; 4],
    w: usize,
    h: usize,
    aspect: f32,
    guide: Option<&crate::brush::AutoGuide>,
) -> Vec<f32> {
    let mut data = vec![0.0f32; w * h];
    for stroke in strokes {
        if stroke.build_up {
            crate::brush::paint_stroke(&mut data, w, h, rect, aspect, stroke, guide);
        } else {
            legacy_stroke_in(&mut data, stroke, rect, w, h, aspect);
        }
    }
    data
}

/// Strokes saved before smooth dabs: coverage of the polyline times the flow.
fn legacy_stroke_in(data: &mut [f32], stroke: &crate::params::BrushStroke, rect: [f32; 4], w: usize, h: usize, aspect: f32) {
    let (kx, ky) = if aspect >= 1.0 { (1.0, 1.0 / aspect) } else { (aspect, 1.0) };
    let r = stroke.radius.max(1e-5);
    let inner = r * stroke.hardness.clamp(0.0, 0.99);
    let pts: Vec<(f32, f32)> = stroke.points.iter().map(|p| (p[0] * kx, p[1] * ky)).collect();
    if pts.is_empty() {
        return;
    }
    let segs: Vec<((f32, f32), (f32, f32))> = if pts.len() == 1 { vec![(pts[0], pts[0])] } else { pts.windows(2).map(|p| (p[0], p[1])).collect() };
    let flow = stroke.flow.clamp(0.0, 1.0);
    data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let py = (rect[1] + (y as f32 + 0.5) / h as f32 * rect[3]) * ky;
        for (x, m) in row.iter_mut().enumerate() {
            let px = (rect[0] + (x as f32 + 0.5) / w as f32 * rect[2]) * kx;
            let mut d2min = f32::MAX;
            for &((ax, ay), (cx, cy)) in &segs {
                let (dx, dy) = (cx - ax, cy - ay);
                let len2 = dx * dx + dy * dy;
                let t = if len2 > 0.0 { (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
                let (qx, qy) = (ax + dx * t - px, ay + dy * t - py);
                d2min = d2min.min(qx * qx + qy * qy);
            }
            let c = (1.0 - smoothstep(inner, r, d2min.sqrt())) * flow;
            *m = if stroke.erase { *m * (1.0 - c) } else { *m + (1.0 - *m) * c };
        }
    });
}

/// A decoded removal patch: camera RGB and coverage.
pub struct Patch {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<[f32; 4]>,
}

impl Patch {
    /// Encodes camera RGB (values up to `scale`) and coverage as a base64 16-bit PNG.
    pub fn encode(&self, scale: f32) -> String {
        use base64::Engine;
        let mut raw: Vec<u16> = Vec::with_capacity(self.w * self.h * 4);
        for p in &self.rgba {
            for v in &p[..3] {
                raw.push(((v / scale).clamp(0.0, 1.0) * 65535.0 + 0.5) as u16);
            }
            raw.push((p[3].clamp(0.0, 1.0) * 65535.0 + 0.5) as u16);
        }
        let img = ::image::ImageBuffer::<::image::Rgba<u16>, Vec<u16>>::from_raw(self.w as u32, self.h as u32, raw).expect("size");
        let mut png = std::io::Cursor::new(Vec::new());
        ::image::DynamicImage::ImageRgba16(img).write_to(&mut png, ::image::ImageFormat::Png).expect("in memory");
        base64::engine::general_purpose::STANDARD.encode(png.into_inner())
    }

    pub fn decode(data: &str, scale: f32) -> Option<Patch> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(data).ok()?;
        let img = ::image::load_from_memory_with_format(&bytes, ::image::ImageFormat::Png).ok()?.into_rgba16();
        let (w, h) = (img.width() as usize, img.height() as usize);
        let rgba = img.pixels().map(|p| [p[0] as f32 / 65535.0 * scale, p[1] as f32 / 65535.0 * scale, p[2] as f32 / 65535.0 * scale, p[3] as f32 / 65535.0]).collect();
        Some(Patch { w, h, rgba })
    }

    #[inline]
    fn sample(&self, x: f32, y: f32) -> [f32; 4] {
        let x = (x - 0.5).clamp(0.0, (self.w - 1) as f32);
        let y = (y - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (x as usize, y as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = (x - x0 as f32, y - y0 as f32);
        let at = |xx: usize, yy: usize| self.rgba[yy * self.w + xx];
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        std::array::from_fn(|k| {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bot = c[k] + (d[k] - c[k]) * tx;
            top + (bot - top) * ty
        })
    }
}

type PatchCache = Mutex<Vec<(u64, Arc<Patch>)>>;

static PATCH_CACHE: LazyLock<PatchCache> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Decoded patch of a removal from a small cache.
fn cached_patch(r: &crate::params::Removal) -> Option<Arc<Patch>> {
    let key = r.id ^ (r.patch.len() as u64).rotate_left(32);
    if let Some((_, p)) = PATCH_CACHE.lock().unwrap().iter().find(|(k, _)| *k == key) {
        return Some(p.clone());
    }
    let p = Arc::new(Patch::decode(&r.patch, r.scale)?);
    let mut cache = PATCH_CACHE.lock().unwrap();
    cache.push((key, p.clone()));
    if cache.len() > 32 {
        cache.remove(0);
    }
    Some(p)
}

/// Replaces removed objects by their generated content (camera RGB, before white balance).
pub fn apply_removals(img: &mut LinearImage, src: &LinearImage, geo: &GeoMap, region: Region, removals: &[crate::params::Removal]) {
    let patches: Vec<(&crate::params::Removal, Arc<Patch>)> = removals.iter().filter_map(|r| cached_patch(r).map(|p| (r, p))).collect();
    if patches.is_empty() {
        return;
    }
    let (sw, sh) = (src.width as f32, src.height as f32);
    let boxes: Vec<_> = patches
        .iter()
        .map(|(r, _)| {
            let [x, y, w, h] = r.rect;
            output_box(geo, (x * sw - 0.5, y * sh - 0.5, (x + w) * sw - 0.5, (y + h) * sh - 0.5))
        })
        .collect();
    img.data.par_chunks_mut(region.w).enumerate().for_each(|(ry, line)| {
        let oy = (region.y + ry) as f32 + 0.5;
        // Rows and pixels away from every removal skip the mapping to the source.
        if !boxes.iter().any(|b| b.is_none_or(|b| oy >= b.1 && oy <= b.3)) {
            return;
        }
        for (rx, px) in line.iter_mut().enumerate() {
            let ox = (region.x + rx) as f32 + 0.5;
            if !in_any(&boxes, ox, oy) {
                continue;
            }
            // Source position and the vignetting gain `resample` applied there: the patch is
            // made in camera space without it.
            let mut uv: Option<(f32, f32, f32)> = None;
            for (r, p) in &patches {
                let (u, v, vig) = *uv.get_or_insert_with(|| {
                    let ([_, g, _], gain) = geo.map(ox, oy);
                    ((g[0] + 0.5) / sw, (g[1] + 0.5) / sh, gain)
                });
                let [x, y, w, h] = r.rect;
                if u < x || v < y || u >= x + w || v >= y + h {
                    continue;
                }
                let q = p.sample((u - x) / w * p.w as f32, (v - y) / h * p.h as f32);
                let q3 = q[3] * r.opacity.clamp(0.0, 1.0);
                for (v, q) in px.iter_mut().zip(q) {
                    *v += (q * vig - *v) * q3;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every output pixel whose source lies in a box is within its output box, also with
    /// straightening, keystone, lens distortion and a quarter turn.
    #[test]
    fn output_box_holds_all_pixels_of_the_area() {
        use crate::params::{Geometry, LensCorrection};
        let (sw, sh) = (600usize, 400usize);
        let mut boxes = 0;
        for turns in [0u8, 1] {
            let geom = Geometry { straighten: 7.0, keystone_v: 20.0, keystone_amount: 100.0, ..Default::default() };
            let lens = LensCorrection { distortion: 40.0, vignetting: 50.0, ..Default::default() };
            let geo = GeoMap::new(&geom, &lens, None, turns, sw, sh, false);
            for b in [(100.0, 80.0, 180.0, 140.0), (300.0, 200.0, 360.0, 250.0), (400.0, 90.0, 490.0, 170.0), (480.0, 300.0, 590.0, 395.0), (5.0, 5.0, 40.0, 30.0)] {
                // Areas cut off by the framing are left to the check of every pixel.
                let Some(o) = output_box(&geo, b) else { continue };
                boxes += 1;
                for oy in 0..geo.out_h {
                    for ox in 0..geo.out_w {
                        let (x, y) = (ox as f32 + 0.5, oy as f32 + 0.5);
                        let (sx, sy) = geo.source_point(x, y);
                        if sx >= b.0 && sx <= b.2 && sy >= b.1 && sy <= b.3 {
                            assert!(x >= o.0 && x <= o.2 && y >= o.1 && y <= o.3, "{turns} {b:?}: ({x}, {y}) außerhalb {o:?}");
                        }
                    }
                }
            }
        }
        assert!(boxes >= 6, "nur {boxes} Rahmen bestimmt");
        // Without any geometry every box is known.
        let geo = GeoMap::new(&Default::default(), &Default::default(), None, 0, sw, sh, false);
        assert!(output_box(&geo, (5.0, 5.0, 40.0, 30.0)).is_some() && output_box(&geo, (480.0, 300.0, 590.0, 395.0)).is_some());
    }

    #[test]
    fn long_thin_heal_stroke_stays_within_the_cell_limit() {
        let src = LinearImage::new(64, 64, vec![[0.5; 3]; 64 * 64]);
        let r = 5.0;
        let (a, b) = ((0.0, 0.0), (6000.0, 4000.0));
        let seg = (a, b, (-r, -r, 6000.0 + r, 4000.0 + r));
        let f = HealField::solve(&src, &[seg], (0.0, 0.0), r);
        assert!(f.w * f.h <= MAX_HEAL_CELLS * 11 / 10, "{} Zellen", f.w * f.h);
        assert!(f.gain.iter().all(|g| g.iter().all(|v| v.is_finite())));
    }
    use crate::params::{BrushStroke, LinearGradient, RadialGradient};

    #[test]
    fn brush_stroke_draws_and_erases() {
        let mut m = Mask {
            strokes: vec![BrushStroke { points: vec![[0.2, 0.5], [0.8, 0.5]], radius: 0.05, hardness: 0.5, flow: 1.0, erase: false, opacity: 1.0, auto_mask: false, build_up: true }],
            ..Default::default()
        };
        let b = rasterize(&m, 1.5, None);
        assert!(b.sample(0.5, 0.5) > 0.99);
        assert!(b.sample(0.5, 0.1) < 0.01);
        m.strokes.push(BrushStroke { points: vec![[0.5, 0.5]], radius: 0.05, hardness: 0.9, flow: 1.0, erase: true, opacity: 1.0, auto_mask: false, build_up: true });
        assert!(rasterize(&m, 1.5, None).sample(0.5, 0.5) < 0.05);
        assert!(rasterize(&m, 1.5, None).sample(0.3, 0.5) > 0.99);
    }

    #[test]
    fn gradients() {
        let m = Mask { linear: vec![LinearGradient { start: [0.0, 0.0], end: [0.0, 1.0] }], ..Default::default() };
        let b = rasterize(&m, 1.0, None);
        assert!(b.sample(0.5, 0.02) > 0.95 && b.sample(0.5, 0.98) < 0.05);
        let m = Mask {
            radial: vec![RadialGradient { center: [0.5, 0.5], radius_x: 0.2, radius_y: 0.2, feather: 0.5, invert: false, angle: 0.0 }],
            ..Default::default()
        };
        let b = rasterize(&m, 1.0, None);
        assert!(b.sample(0.5, 0.5) > 0.99 && b.sample(0.05, 0.05) < 0.01);
    }

    #[test]
    fn rotated_ellipse() {
        // Wide ellipse turned by 90°: it now reaches up and down, not sideways.
        let g = RadialGradient { center: [0.5, 0.5], radius_x: 0.3, radius_y: 0.1, feather: 0.2, invert: false, angle: std::f32::consts::FRAC_PI_2 };
        let b = rasterize(&Mask { radial: vec![g], ..Default::default() }, 1.0, None);
        assert!(b.sample(0.5, 0.72) > 0.9, "{}", b.sample(0.5, 0.72));
        assert!(b.sample(0.72, 0.5) < 0.05, "{}", b.sample(0.72, 0.5));
    }

    #[test]
    fn stroke_painted_point_by_point_equals_painting_it_at_once() {
        let aspect = 1.5;
        let st = |pts: Vec<[f32; 2]>, erase: bool, opacity: f32| crate::params::BrushStroke {
            points: pts,
            radius: 0.04,
            hardness: 0.2,
            flow: 0.6,
            erase,
            opacity,
            auto_mask: false,
            build_up: true,
        };
        let path = |i: usize| [0.15 + i as f32 * 0.01, 0.45 + 0.15 * (i as f32 * 0.15).sin()];
        let mut m = Mask { strokes: vec![st(vec![[0.2, 0.3], [0.7, 0.6]], false, 1.0)], ..Default::default() };
        let _ = cached_bitmap(&m, aspect, None);
        for (erase, opacity) in [(false, 0.8), (true, 1.0)] {
            m.strokes.push(st(vec![path(0)], erase, opacity));
            for i in 1..60 {
                let live = cached_bitmap(&m, aspect, None);
                let whole = rasterize(&m, aspect, None);
                let diff = live.data.iter().zip(&whole.data).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
                assert!(diff < 1e-4, "Punkt {i}, löschen {erase}: Abweichung {diff}");
                m.strokes.last_mut().unwrap().points.push(path(i));
            }
        }
    }

    #[test]
    fn cache_returns_same_bitmap() {
        let m = Mask { fill: true, ..Default::default() };
        let a = cached_bitmap(&m, 1.5, None);
        let b = cached_bitmap(&m, 1.5, None);
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn clone_spot_copies_source() {
        // Left half dark, right half bright; clone a bright spot onto the dark side.
        let (w, h) = (40, 20);
        let src = LinearImage::new(w, h, (0..w * h).map(|i| if i % w < w / 2 { [0.1; 3] } else { [0.8; 3] }).collect());
        let geo = GeoMap::new(&Default::default(), &Default::default(), None, 0, w, h, false);
        let region = Region { x: 0, y: 0, w, h };
        let mut img = geo.resample(&src, region);
        let spot = Spot { mode: SpotMode::Clone, target: [0.25, 0.5], source: [0.75, 0.5], radius: 0.1, feather: 0.2, opacity: 1.0, path: Vec::new(), strokes: Vec::new() };
        apply_retouch(&mut img, &src, &geo, region, &[spot]);
        assert!(img.data[10 * w + 10][0] > 0.7);
        assert!(img.data[10 * w + 2][0] < 0.15);
    }

    #[test]
    fn retouch_gets_the_vignetting_correction() {
        // Uniform grey with a blemish in a corner, vignetting correction on: the healed area
        // must match its corrected surroundings, a clone from the center the center's brightness.
        let (w, h) = (80, 60);
        let src = LinearImage::new(w, h, (0..w * h).map(|i| if (i % w, i / w) == (8, 8) { [0.9; 3] } else { [0.2; 3] }).collect());
        let lens = crate::params::LensCorrection { vignetting: 80.0, ..Default::default() };
        let geo = GeoMap::new(&Default::default(), &lens, None, 0, w, h, false);
        let region = Region { x: 0, y: 0, w, h };
        let resampled = geo.resample(&src, region);
        // What the corrected grey is at the blemish and at the center.
        let around = 0.2 * geo.map(8.5, 8.5).1;
        let center = resampled.data[30 * w + 40][0];
        assert!(around > center * 1.1, "Ecke {around}, Mitte {center}");
        let spot = |mode, source: [f32; 2]| Spot { mode, target: [8.5 / 80.0, 8.5 / 60.0], source, radius: 0.04, feather: 0.2, opacity: 1.0, path: Vec::new(), strokes: Vec::new() };
        let mut img = resampled.clone();
        apply_retouch(&mut img, &src, &geo, region, &[spot(SpotMode::Heal, [40.5 / 80.0, 30.5 / 60.0])]);
        let v = img.data[8 * w + 8][0];
        assert!((v / around - 1.0).abs() < 0.05, "geheilt {v}, Umgebung {around}");
        let mut img = resampled.clone();
        apply_retouch(&mut img, &src, &geo, region, &[spot(SpotMode::Clone, [40.5 / 80.0, 30.5 / 60.0])]);
        let v = img.data[8 * w + 8][0];
        assert!((v / center - 1.0).abs() < 0.05, "geklont {v}, Quelle {center}");
    }

    #[test]
    fn heal_matches_target_brightness() {
        let (w, h) = (60, 30);
        // Dark region with a small bright blemish; textured source region on the right is brighter.
        let src = LinearImage::new(
            w,
            h,
            (0..w * h)
                .map(|i| {
                    let (x, y) = (i % w, i / w);
                    if (x as i32 - 15).abs() < 2 && (y as i32 - 15).abs() < 2 {
                        [0.9; 3]
                    } else if x < w / 2 {
                        [0.1; 3]
                    } else {
                        [0.4; 3]
                    }
                })
                .collect(),
        );
        let geo = GeoMap::new(&Default::default(), &Default::default(), None, 0, w, h, false);
        let region = Region { x: 0, y: 0, w, h };
        let mut img = geo.resample(&src, region);
        let spot = Spot { mode: SpotMode::Heal, target: [15.5 / 60.0, 15.5 / 30.0], source: [45.5 / 60.0, 15.5 / 30.0], radius: 0.06, feather: 0.3, opacity: 1.0, path: Vec::new(), strokes: Vec::new() };
        apply_retouch(&mut img, &src, &geo, region, &[spot]);
        let v = img.data[15 * w + 15][0];
        assert!((v - 0.1).abs() < 0.03, "healed value {v}");
    }

    #[test]
    fn heal_follows_a_gradient_across_the_area() {
        // The target sits on a left-to-right ramp, the source on flat grey with fine texture.
        // A constant gain would leave a step at both sides; healing follows the ramp.
        let (w, h) = (120usize, 60usize);
        let ramp = |x: usize| 0.05 + 0.5 * x as f32 / 60.0;
        let src = LinearImage::new(
            w,
            h,
            (0..w * h)
                .map(|i| {
                    let (x, y) = (i % w, i / w);
                    if x < 60 {
                        if (x as i32 - 30).abs() < 3 && (y as i32 - 30).abs() < 3 { [0.9; 3] } else { [ramp(x); 3] }
                    } else {
                        [0.3 * if (x + y) % 2 == 0 { 1.1 } else { 0.9 }; 3]
                    }
                })
                .collect(),
        );
        let geo = GeoMap::new(&Default::default(), &Default::default(), None, 0, w, h, false);
        let region = Region { x: 0, y: 0, w, h };
        let mut img = geo.resample(&src, region);
        let spot = Spot { mode: SpotMode::Heal, target: [30.5 / 120.0, 30.5 / 60.0], source: [90.5 / 120.0, 30.5 / 60.0], radius: 10.0 / 120.0, feather: 0.1, opacity: 1.0, path: Vec::new(), strokes: Vec::new() };
        apply_retouch(&mut img, &src, &geo, region, &[spot]);
        // Average over a 2x2 block to cancel the checkerboard texture.
        let at = |x: usize| (img.data[30 * w + x][0] + img.data[30 * w + x + 1][0] + img.data[31 * w + x][0] + img.data[31 * w + x + 1][0]) / 4.0;
        for x in [22, 26, 30, 34, 38] {
            let (v, want) = (at(x), (ramp(x) + ramp(x + 1)) / 2.0);
            assert!((v - want).abs() < 0.12 * want, "x={x}: {v} statt {want}");
        }
    }

    #[test]
    fn painted_clone_area_follows_the_stroke() {
        // Left half dark, right half bright; a horizontal stroke in the dark half clones from
        // the bright half. Pixels along the stroke change, those beside it do not.
        let (w, h) = (80usize, 40usize);
        let data = (0..w * h).map(|i| if i % w < 40 { [0.1f32; 3] } else { [0.8; 3] }).collect();
        let src = LinearImage::new(w, h, data);
        let mut img = src.clone();
        let geo = GeoMap::new(&Default::default(), &Default::default(), None, 0, w, h, false);
        let region = Region { x: 0, y: 0, w, h };
        let y = 20.5 / h as f32;
        let spot = Spot {
            mode: SpotMode::Clone,
            target: [5.5 / w as f32, y],
            source: [45.5 / w as f32, y],
            path: vec![[15.5 / w as f32, y], [25.5 / w as f32, y]],
            strokes: Vec::new(),
            radius: 3.0 / w as f32,
            feather: 0.0,
            opacity: 1.0,
        };
        apply_retouch(&mut img, &src, &geo, region, &[spot]);
        let at = |x: usize, y: usize| img.data[y * w + x][0];
        for x in [6, 12, 20, 25] {
            assert!(at(x, 20) > 0.7, "x={x}: {}", at(x, 20));
        }
        assert!(at(15, 30) < 0.2 && at(33, 20) < 0.2, "außerhalb unverändert");
    }

    /// Strokes painted onto an area afterwards add to it with the same source offset, or take
    /// part of it away again.
    #[test]
    fn painted_strokes_extend_and_shrink_an_area() {
        use crate::params::SpotStroke;
        let (w, h) = (80usize, 40usize);
        let data = (0..w * h).map(|i| if i % w < 40 { [0.1f32; 3] } else { [0.8; 3] }).collect();
        let src = LinearImage::new(w, h, data);
        let geo = GeoMap::new(&Default::default(), &Default::default(), None, 0, w, h, false);
        let region = Region { x: 0, y: 0, w, h };
        let p = |x: f32, y: f32| [(x + 0.5) / w as f32, (y + 0.5) / h as f32];
        let spot = Spot {
            mode: SpotMode::Clone,
            target: p(5.0, 10.0),
            source: p(45.0, 10.0),
            path: vec![p(25.0, 10.0)],
            strokes: vec![
                // Down from the middle of the line, then across below it.
                SpotStroke { points: vec![p(15.0, 10.0), p(15.0, 30.0)], erase: false },
                // The left end taken away.
                SpotStroke { points: vec![p(5.0, 10.0), p(8.0, 10.0)], erase: true },
            ],
            radius: 3.0 / w as f32,
            feather: 0.0,
            opacity: 1.0,
        };
        let mut img = src.clone();
        apply_retouch(&mut img, &src, &geo, region, &[spot]);
        let at = |x: usize, y: usize| img.data[y * w + x][0];
        assert!(at(20, 10) > 0.7 && at(15, 25) > 0.7, "Linie und Erweiterung kopiert");
        assert!(at(6, 10) < 0.2, "weggenommen: {}", at(6, 10));
        assert!(at(25, 25) < 0.2, "außerhalb unverändert");
    }

    /// A removal shows its patch with its opacity.
    #[test]
    fn removal_opacity_blends_the_patch() {
        let (w, h) = (40usize, 40usize);
        let src = LinearImage::new(w, h, vec![[0.1f32; 3]; w * h]);
        let geo = GeoMap::new(&Default::default(), &Default::default(), None, 0, w, h, false);
        let region = Region { x: 0, y: 0, w, h };
        let patch = Patch { w: 8, h: 8, rgba: vec![[0.9, 0.9, 0.9, 1.0]; 64] };
        let removal = |opacity| crate::params::Removal {
            id: 7_000_001,
            rect: [0.25, 0.25, 0.5, 0.5],
            patch: Arc::new(patch.encode(1.0)),
            scale: 1.0,
            opacity,
            ..Default::default()
        };
        let at = |opacity| {
            let mut img = src.clone();
            apply_removals(&mut img, &src, &geo, region, &[removal(opacity)]);
            (img.data[20 * w + 20][0], img.data[2 * w + 2][0])
        };
        let (full, outside) = at(1.0);
        assert!((full - 0.9).abs() < 0.01 && (outside - 0.1).abs() < 1e-6, "{full} {outside}");
        assert!((at(0.5).0 - 0.5).abs() < 0.01, "{}", at(0.5).0);
        assert!((at(0.0).0 - 0.1).abs() < 1e-6);
    }

    #[test]
    fn auto_source_avoids_different_regions() {
        let (w, h) = (80, 80);
        // Uniform grey with a bright band at the top.
        let src = LinearImage::new(w, h, (0..w * h).map(|i| if i / w < 20 { [0.9; 3] } else { [0.3; 3] }).collect());
        let s = auto_source(&src, [0.5, 0.6], 0.05);
        assert!(s[1] * h as f32 > 25.0, "source in the bright band: {s:?}");
        assert!(s != [0.5, 0.6]);
    }

    #[test]
    fn painting_stroke_by_stroke_matches_full_rasterization() {
        let st = |pts: Vec<[f32; 2]>, erase: bool| BrushStroke { points: pts, radius: 0.04, hardness: 0.3, flow: 0.4, erase, opacity: 0.8, auto_mask: false, build_up: true };
        let mut m = Mask { linear: vec![crate::params::LinearGradient { start: [0.0, 0.0], end: [0.0, 0.3] }], ..Default::default() };
        m.strokes.push(st(vec![[0.2, 0.5], [0.4, 0.55]], false));
        let _ = cached_bitmap(&m, 1.5, None);
        // The next stroke is drawn on top of the cached mask without it.
        m.strokes.push(st(vec![[0.3, 0.4], [0.35, 0.6], [0.5, 0.62]], false));
        m.strokes.push(st(vec![[0.33, 0.5]], true));
        let _ = cached_bitmap(&Mask { strokes: m.strokes[..2].to_vec(), ..m.clone() }, 1.5, None);
        let incremental = cached_bitmap(&m, 1.5, None);
        let full = rasterize(&m, 1.5, None);
        let diff = incremental.data.iter().zip(&full.data).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(diff < 1e-5, "{diff}");
    }
}

