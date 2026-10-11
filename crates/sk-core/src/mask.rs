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
pub const MASK_RES: usize = 1536;

#[derive(Clone, Debug)]
pub struct MaskBitmap {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl MaskBitmap {
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
pub fn rasterize(mask: &Mask, aspect: f32) -> MaskBitmap {
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
                    let e = (((px - cx) / rx).powi(2) + ((py - cy) / ry).powi(2)).sqrt();
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

    for stroke in &mask.strokes {
        let r = (stroke.radius * long).max(0.5);
        let inner = r * stroke.hardness.clamp(0.0, 0.99);
        // Bounding box of the stroke in bitmap pixels.
        let pts: Vec<(f32, f32)> = stroke.points.iter().map(|p| (p[0] * w as f32, p[1] * h as f32)).collect();
        if pts.is_empty() {
            continue;
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
            continue;
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
    MaskBitmap { w, h, data }
}

fn mask_key(mask: &Mask, aspect: f32) -> u64 {
    let drawn = serde_json::json!({ "f": mask.fill, "s": mask.strokes, "l": mask.linear, "r": mask.radial });
    let ai = mask.ai.as_ref().map_or(0, |a| crate::cache::fnv1a(&a.data) ^ (a.width as u64) << 32 ^ a.height as u64);
    crate::cache::fnv1a(format!("{drawn}|{ai}|{aspect:.5}").as_bytes())
}

type MaskCache = Mutex<Vec<(u64, Arc<MaskBitmap>)>>;

static MASK_CACHE: LazyLock<MaskCache> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Rasterized mask from a small LRU cache (rasterization only runs when the mask changes).
pub fn cached_bitmap(mask: &Mask, aspect: f32) -> Arc<MaskBitmap> {
    let key = mask_key(mask, aspect);
    {
        let mut cache = MASK_CACHE.lock().unwrap();
        if let Some(pos) = cache.iter().position(|(k, _)| *k == key) {
            let entry = cache.remove(pos);
            let bm = entry.1.clone();
            cache.push(entry);
            return bm;
        }
    }
    let bm = Arc::new(rasterize(mask, aspect));
    let mut cache = MASK_CACHE.lock().unwrap();
    cache.push((key, bm.clone()));
    if cache.len() > 24 {
        cache.remove(0);
    }
    bm
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

struct PreparedSpot {
    target: (f32, f32),
    offset: (f32, f32),
    r: f32,
    inner: f32,
    gain: [f32; 3],
}

/// Applies heal/clone spots to a resampled region (camera space).
pub fn apply_retouch(img: &mut LinearImage, src: &LinearImage, geo: &GeoMap, region: Region, spots: &[Spot]) {
    if spots.is_empty() {
        return;
    }
    let (sw, sh) = (src.width as f32, src.height as f32);
    let long = sw.max(sh);
    let prepared: Vec<PreparedSpot> = spots
        .iter()
        .map(|s| {
            let target = (s.target[0] * sw - 0.5, s.target[1] * sh - 0.5);
            let source = (s.source[0] * sw - 0.5, s.source[1] * sh - 0.5);
            let r = (s.radius * long).max(1.0);
            let gain = match s.mode {
                SpotMode::Clone => [1.0; 3],
                // Heal: take texture from the source but brightness/color from the target surroundings.
                SpotMode::Heal => {
                    let t = ring_mean(src, target, r * 1.05, r * 1.4);
                    let q = ring_mean(src, source, r * 1.05, r * 1.4);
                    std::array::from_fn(|c| if q[c] > 1e-5 { (t[c] / q[c]).clamp(0.25, 4.0) } else { 1.0 })
                }
            };
            PreparedSpot { target, offset: (source.0 - target.0, source.1 - target.1), r, inner: r * (1.0 - s.feather.clamp(0.0, 1.0)), gain }
        })
        .collect();
    img.data.par_chunks_mut(region.w).enumerate().for_each(|(ry, line)| {
        let oy = (region.y + ry) as f32 + 0.5;
        for (rx, px) in line.iter_mut().enumerate() {
            let ox = (region.x + rx) as f32 + 0.5;
            let mut pt: Option<(f32, f32)> = None;
            for s in &prepared {
                let (sx, sy) = *pt.get_or_insert_with(|| geo.source_point(ox, oy));
                let d = ((sx - s.target.0).powi(2) + (sy - s.target.1).powi(2)).sqrt();
                if d >= s.r {
                    continue;
                }
                let w = 1.0 - smoothstep(s.inner, s.r, d);
                let v = bilinear(src, sx + s.offset.0, sy + s.offset.1);
                for c in 0..3 {
                    let repl = v[c] * s.gain[c];
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
    let t = (target[0] * sw, target[1] * sh);
    let ring = ring_mean(src, t, r * 1.05, r * 1.4);
    let mut best = (f32::MAX, target);
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
                best = (score, [c.0 / sw, c.1 / sh]);
            }
        }
    }
    best.1
}

// ------------------------------------------------------------------ removals (generative fill)

/// Coverage of brush strokes over the part `rect` (x, y, w, h; normalized source coordinates)
/// of a source with aspect ratio `aspect`, rasterized at `w` x `h`.
pub fn rasterize_strokes_in(strokes: &[crate::params::BrushStroke], rect: [f32; 4], w: usize, h: usize, aspect: f32) -> Vec<f32> {
    // Distances in units of the long image edge.
    let (kx, ky) = if aspect >= 1.0 { (1.0, 1.0 / aspect) } else { (aspect, 1.0) };
    let mut data = vec![0.0f32; w * h];
    for stroke in strokes {
        let r = stroke.radius.max(1e-5);
        let inner = r * stroke.hardness.clamp(0.0, 0.99);
        let pts: Vec<(f32, f32)> = stroke.points.iter().map(|p| (p[0] * kx, p[1] * ky)).collect();
        if pts.is_empty() {
            continue;
        }
        let segs: Vec<((f32, f32), (f32, f32))> =
            if pts.len() == 1 { vec![(pts[0], pts[0])] } else { pts.windows(2).map(|p| (p[0], p[1])).collect() };
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
    data
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
    img.data.par_chunks_mut(region.w).enumerate().for_each(|(ry, line)| {
        let oy = (region.y + ry) as f32 + 0.5;
        for (rx, px) in line.iter_mut().enumerate() {
            let ox = (region.x + rx) as f32 + 0.5;
            let mut uv: Option<(f32, f32)> = None;
            for (r, p) in &patches {
                let (u, v) = *uv.get_or_insert_with(|| {
                    let (sx, sy) = geo.source_point(ox, oy);
                    ((sx + 0.5) / sw, (sy + 0.5) / sh)
                });
                let [x, y, w, h] = r.rect;
                if u < x || v < y || u >= x + w || v >= y + h {
                    continue;
                }
                let q = p.sample((u - x) / w * p.w as f32, (v - y) / h * p.h as f32);
                let q3 = q[3];
                for (v, q) in px.iter_mut().zip(q) {
                    *v += (q - *v) * q3;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{BrushStroke, LinearGradient, RadialGradient};

    #[test]
    fn brush_stroke_draws_and_erases() {
        let mut m = Mask {
            strokes: vec![BrushStroke { points: vec![[0.2, 0.5], [0.8, 0.5]], radius: 0.05, hardness: 0.5, flow: 1.0, erase: false }],
            ..Default::default()
        };
        let b = rasterize(&m, 1.5);
        assert!(b.sample(0.5, 0.5) > 0.99);
        assert!(b.sample(0.5, 0.1) < 0.01);
        m.strokes.push(BrushStroke { points: vec![[0.5, 0.5]], radius: 0.05, hardness: 0.9, flow: 1.0, erase: true });
        assert!(rasterize(&m, 1.5).sample(0.5, 0.5) < 0.05);
        assert!(rasterize(&m, 1.5).sample(0.3, 0.5) > 0.99);
    }

    #[test]
    fn gradients() {
        let m = Mask { linear: vec![LinearGradient { start: [0.0, 0.0], end: [0.0, 1.0] }], ..Default::default() };
        let b = rasterize(&m, 1.0);
        assert!(b.sample(0.5, 0.02) > 0.95 && b.sample(0.5, 0.98) < 0.05);
        let m = Mask {
            radial: vec![RadialGradient { center: [0.5, 0.5], radius_x: 0.2, radius_y: 0.2, feather: 0.5, invert: false }],
            ..Default::default()
        };
        let b = rasterize(&m, 1.0);
        assert!(b.sample(0.5, 0.5) > 0.99 && b.sample(0.05, 0.05) < 0.01);
    }

    #[test]
    fn cache_returns_same_bitmap() {
        let m = Mask { fill: true, ..Default::default() };
        let a = cached_bitmap(&m, 1.5);
        let b = cached_bitmap(&m, 1.5);
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn clone_spot_copies_source() {
        // Left half dark, right half bright; clone a bright spot onto the dark side.
        let (w, h) = (40, 20);
        let src = LinearImage::new(w, h, (0..w * h).map(|i| if i % w < w / 2 { [0.1; 3] } else { [0.8; 3] }).collect());
        let geo = GeoMap::new(&Default::default(), &Default::default(), 0, w, h, false);
        let region = Region { x: 0, y: 0, w, h };
        let mut img = geo.resample(&src, region);
        let spot = Spot { mode: SpotMode::Clone, target: [0.25, 0.5], source: [0.75, 0.5], radius: 0.1, feather: 0.2 };
        apply_retouch(&mut img, &src, &geo, region, &[spot]);
        assert!(img.data[10 * w + 10][0] > 0.7);
        assert!(img.data[10 * w + 2][0] < 0.15);
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
        let geo = GeoMap::new(&Default::default(), &Default::default(), 0, w, h, false);
        let region = Region { x: 0, y: 0, w, h };
        let mut img = geo.resample(&src, region);
        let spot = Spot { mode: SpotMode::Heal, target: [15.5 / 60.0, 15.5 / 30.0], source: [45.5 / 60.0, 15.5 / 30.0], radius: 0.06, feather: 0.3 };
        apply_retouch(&mut img, &src, &geo, region, &[spot]);
        let v = img.data[15 * w + 15][0];
        assert!((v - 0.1).abs() < 0.03, "healed value {v}");
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
}
