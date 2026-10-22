//! The pipeline on the graphics card: the same stages as the CPU path (kernels in
//! `gpu/shaders`), which stays the reference and the fallback.
//!
//! Heal/clone areas and removals are applied on the CPU while resampling (only when there
//! are any), soft proofing converts the developed result on the CPU (Little CMS).

use std::sync::{LazyLock, Weak};

use anyhow::{Context, Result, bail};
use bytemuck::{Pod, Zeroable};
use wgpu::Buffer;

use super::*;
use crate::color::mat_inv;
use crate::geometry::GeoGpu;
use crate::gpu::{Encoder, Fallback, Gpu, Kernel};
use crate::mask::apply_removals;

macro_rules! kernel {
    ($name:literal) => {
        ($name, include_str!(concat!("../gpu/shaders/", $name, ".wgsl")))
    };
}

const RESAMPLE: Kernel = kernel!("resample");
const BOX_H: Kernel = kernel!("box_h");
const BOX_V: Kernel = kernel!("box_v");
const PLANE_OP: Kernel = kernel!("plane_op");
const IMG_PLANES: Kernel = kernel!("img_planes");
const DOWNSAMPLE: Kernel = kernel!("downsample");
const UPSAMPLE_APPLY: Kernel = kernel!("upsample_apply");
const NR_COMBINE: Kernel = kernel!("nr_combine");
const DEFRINGE: Kernel = kernel!("defringe");
const DARK_POOL: Kernel = kernel!("dark_pool");
const DEVELOP: Kernel = kernel!("develop");
const SHARPEN: Kernel = kernel!("sharpen");
const FINISH: Kernel = kernel!("finish");
const GLOW: Kernel = kernel!("glow");
const DISPLAY: Kernel = kernel!("display");
const OUTPUT: Kernel = kernel!("output");
const MEDIAN: Kernel = kernel!("median");

const ALL: [Kernel; 17] =
    [RESAMPLE, BOX_H, BOX_V, PLANE_OP, IMG_PLANES, DOWNSAMPLE, UPSAMPLE_APPLY, NR_COMBINE, DEFRINGE, DARK_POOL, DEVELOP, SHARPEN, GLOW, FINISH, DISPLAY, OUTPUT, MEDIAN];

/// Sets up the graphics card and compiles all kernels (takes a moment; call it early in
/// the background so that the first rendering does not wait for it).
pub fn prepare() {
    if let Some(g) = crate::gpu::active() {
        let t = std::time::Instant::now();
        g.prepare(&ALL);
        log::info!("Shader übersetzt in {:?}", t.elapsed());
    }
}

// plane_op.wgsl
const OP_SQUARE: u32 = 0;
const OP_GUIDED_A: u32 = 1;
const OP_GUIDED_B: u32 = 2;
const OP_GUIDED_OUT: u32 = 3;
const OP_DETAIL: u32 = 4;
const OP_KEEP: u32 = 5;

// img_planes.wgsl
const IMG_SQRT_LUMA: u32 = 0;
const IMG_CHROMA: u32 = 1;
const IMG_LOG_LUMA: u32 = 2;
const IMG_LUMA: u32 = 4;
/// Red; green and blue follow.
const IMG_RED: u32 = 5;

/// Region size above which exports are processed in tiles (output pixels per side).
const TILE: usize = 2048;
/// Largest processed region (with margin) rendered in one piece.
const MAX_PIXELS: usize = 12_000_000;

/// Results of the expensive stages kept on the graphics card between renderings.
#[derive(Default)]
pub(super) struct Cache {
    /// The camera image as uploaded: as it is, or resampled with heal/clone and removals.
    source: Option<(u64, Arc<Buffer>)>,
    early: Option<(u64, Arc<Buffer>)>,
    local: Option<(u64, Arc<Local>)>,
}

/// Local operator planes, concatenated in one buffer.
struct Local {
    buf: Buffer,
    /// Offsets (in floats) of: smoothed log luminance, clarity, structure, dark channel.
    off: [u32; 4],
    has: [bool; 4],
    dark: Option<DarkInfo>,
}

impl Local {
    /// The part `ext` of the planes of the whole image (`full_w` wide).
    fn crop(&self, ops: &mut Ops, full_w: usize, ext: Region) -> Local {
        let n = ext.w * ext.h;
        let planes = self.has[..3].iter().filter(|h| **h).count();
        let dark_len = self.dark.map_or(0, |d| (d.w * d.h * 3) as usize);
        let buf = ops.plane(planes * n + dark_len);
        let mut off = [0u32; 4];
        let mut at = 0usize;
        for ((has, from), to) in self.has.iter().zip(self.off).zip(off.iter_mut()).take(3) {
            if *has {
                *to = at as u32;
                ops.crop_into((&self.buf, from as usize), full_w, ext, 4, (&buf, at));
                at += n;
            }
        }
        // The dark channel is that of the whole image; only where the part starts in it changes.
        let dark = self.dark.map(|d| {
            off[3] = at as u32;
            ops.enc.copy_at(&self.buf, self.off[3] as u64 * 4, &buf, at as u64 * 4, dark_len as u64 * 4);
            DarkInfo { origin: (d.origin.0 + ext.x as u32, d.origin.1 + ext.y as u32), ..d }
        });
        Local { buf, off, has: self.has, dark }
    }
}

#[derive(Clone, Copy)]
struct DarkInfo {
    w: u32,
    h: u32,
    step: u32,
    airlight: [f32; 3],
    /// Offset of the processed region in the dark channel's domain.
    origin: (u32, u32),
}

/// An encoder with the buffer helpers the stages need.
struct Ops<'a> {
    g: &'a Gpu,
    enc: Encoder<'a>,
    /// Bound to unused bindings.
    dummy: Buffer,
    /// Buffers of this rendering, returned to the pool when it is done (unless kept).
    temps: Vec<Buffer>,
}

impl Drop for Ops<'_> {
    fn drop(&mut self) {
        self.g.release(std::mem::take(&mut self.temps));
    }
}

impl<'a> Ops<'a> {
    fn new(g: &'a Gpu) -> Self {
        Self { g, enc: g.encoder(), dummy: g.buffer(16), temps: Vec::new() }
    }

    /// A zeroed storage buffer, from the pool if possible.
    fn alloc(&mut self, bytes: u64) -> Buffer {
        let (b, reused) = self.g.pooled_buffer(bytes);
        if reused {
            self.enc.clear(&b);
        }
        self.temps.push(b.clone());
        b
    }

    /// The part `ext` of a raster `full_w` wide (elements of `size` bytes, starting at element
    /// `src.1` of `src.0`), written row after row from element `dst.1` of `dst.0`.
    fn crop_into(&mut self, (src, from): (&Buffer, usize), full_w: usize, ext: Region, size: usize, (dst, to): (&Buffer, usize)) {
        for y in 0..ext.h {
            let at = (from + (ext.y + y) * full_w + ext.x) * size;
            self.enc.copy_at(src, at as u64, dst, ((to + y * ext.w) * size) as u64, (ext.w * size) as u64);
        }
    }

    /// `b` outlives this rendering (kept in a cache): it must not go back to the pool.
    fn keep(&mut self, b: &Buffer) {
        self.temps.retain(|t| t != b);
    }

    /// Submits the recorded work and continues with a new encoder.
    fn flush(&mut self) {
        std::mem::replace(&mut self.enc, self.g.encoder()).submit();
    }

    fn plane(&mut self, n: usize) -> Buffer {
        self.alloc(n as u64 * 4)
    }

    fn image(&mut self, n: usize) -> Buffer {
        self.alloc(n as u64 * 16)
    }

    fn args<T: Pod>(&self, a: &T) -> Buffer {
        self.g.upload(std::slice::from_ref(a))
    }

    fn copy_plane(&mut self, p: &Buffer, n: usize) -> Buffer {
        let out = self.plane(n);
        self.enc.copy(p, &out, 0, n as u64 * 4);
        out
    }

    fn op(&mut self, op: u32, k: f32, x: &Buffer, y: &Buffer, z: &Buffer, n: usize) -> Buffer {
        #[repr(C)]
        #[derive(Clone, Copy, Pod, Zeroable)]
        struct Args {
            n: u32,
            op: u32,
            k: f32,
        }
        let out = self.plane(n);
        let a = self.args(&Args { n: n as u32, op, k });
        self.enc.run_1d(PLANE_OP, &[&a, x, y, z, &out], n);
        out
    }

    /// Planes derived from an image (`IMG_*`); the second only for `IMG_CHROMA`.
    fn img_planes(&mut self, op: u32, img: &Buffer, n: usize) -> (Buffer, Buffer) {
        #[repr(C)]
        #[derive(Clone, Copy, Pod, Zeroable)]
        struct Args {
            n: u32,
            op: u32,
        }
        let o0 = self.plane(n);
        let o1 = if op == IMG_CHROMA { self.plane(n) } else { self.g.buffer(16) };
        let a = self.args(&Args { n: n as u32, op });
        self.enc.run_1d(IMG_PLANES, &[&a, img, &o0, &o1], n);
        (o0, o1)
    }

    fn box_pass(&mut self, from: &Buffer, to: &Buffer, w: usize, h: usize, r: usize) {
        let a = self.args(&[w as u32, h as u32, r as u32]);
        let tmp = self.plane(w * h);
        self.enc.run_1d(BOX_H, &[&a, from, &tmp], h);
        self.enc.run_1d(BOX_V, &[&a, &tmp, to], w);
    }

    /// detail::median_hv
    fn median(&mut self, p: &Buffer, w: usize, h: usize, r: usize) -> Buffer {
        let tmp = self.plane(w * h);
        let out = self.plane(w * h);
        let a = self.args(&[w as u32, h as u32, r as u32, 0]);
        self.enc.run_1d(MEDIAN, &[&a, p, &tmp], w * h);
        let a = self.args(&[w as u32, h as u32, r as u32, 1]);
        self.enc.run_1d(MEDIAN, &[&a, &tmp, &out], w * h);
        out
    }

    /// detail::box_blur
    fn box_blur(&mut self, p: &Buffer, w: usize, h: usize, r: usize) -> Buffer {
        if r == 0 || w == 0 || h == 0 {
            return self.copy_plane(p, w * h);
        }
        let out = self.plane(w * h);
        self.box_pass(p, &out, w, h, r);
        out
    }

    /// detail::gaussian_blur (three box passes)
    fn gaussian(&mut self, p: &Buffer, w: usize, h: usize, sigma: f32) -> Buffer {
        if sigma < 0.3 || w == 0 || h == 0 {
            return self.copy_plane(p, w * h);
        }
        let r = (((12.0 * sigma * sigma / 3.0 + 1.0).sqrt() - 1.0) / 2.0).round().max(1.0) as usize;
        let a = self.plane(w * h);
        let b = self.plane(w * h);
        self.box_pass(p, &a, w, h, r);
        self.box_pass(&a, &b, w, h, r);
        self.box_pass(&b, &a, w, h, r);
        a
    }

    /// Guided filter coefficients (mean a, mean b) of `p` with radius `r`.
    fn guided_coefficients(&mut self, p: &Buffer, w: usize, h: usize, r: usize, eps: f32) -> (Buffer, Buffer) {
        let n = w * h;
        let mean = self.box_blur(p, w, h, r);
        let sq = self.op(OP_SQUARE, 0.0, p, p, p, n);
        let sq = self.box_blur(&sq, w, h, r);
        let a = self.op(OP_GUIDED_A, eps, &mean, &sq, &sq, n);
        let b = self.op(OP_GUIDED_B, 0.0, &mean, &a, &a, n);
        (self.box_blur(&a, w, h, r), self.box_blur(&b, w, h, r))
    }

    /// detail::guided_filter
    fn guided(&mut self, p: &Buffer, w: usize, h: usize, r: usize, eps: f32) -> Buffer {
        if r == 0 {
            return self.copy_plane(p, w * h);
        }
        let (ma, mb) = self.guided_coefficients(p, w, h, r, eps);
        self.op(OP_GUIDED_OUT, 0.0, &ma, &mb, p, w * h)
    }

    /// detail::fast_guided_filter
    fn fast_guided(&mut self, p: &Buffer, w: usize, h: usize, r: usize, eps: f32, s: usize) -> Buffer {
        if s <= 1 || r / s < 2 || w < s * 4 || h < s * 4 {
            return self.guided(p, w, h, r, eps);
        }
        let (lw, lh) = (w / s, h / s);
        let low = self.plane(lw * lh);
        let a = self.args(&[w as u32, lw as u32, lh as u32, s as u32]);
        self.enc.run_1d(DOWNSAMPLE, &[&a, p, &low], lw * lh);
        let (ma, mb) = self.guided_coefficients(&low, lw, lh, r / s, eps);
        let out = self.plane(w * h);
        let a = self.args(&[w as u32, h as u32, lw as u32, lh as u32, s as u32]);
        self.enc.run_2d(UPSAMPLE_APPLY, &[&a, &ma, &mb, p, &out], w, h);
        out
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ResampleArgs {
    geo: GeoGpu,
    region: [u32; 4],
    win: [u32; 4],
    wb: [f32; 3],
    cam: [f32; 9],
    hdr: u32,
    to_working: u32,
    /// See `Processor::rebuilt`.
    rebuilt: u32,
}

/// Layout of `Params` in develop.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DevelopParams {
    w: u32,
    h: u32,
    ext_x: u32,
    ext_y: u32,
    n_layers: u32,
    overlay: i32,
    has_ev: u32,
    has_clarity: u32,
    has_structure: u32,
    has_dark: u32,
    color_active: u32,
    editor_active: u32,
    skin_active: u32,
    balance_active: u32,
    bw_enabled: u32,
    base_curve: u32,
    hue_preserve: u32,
    off_ev: u32,
    off_clarity: u32,
    off_structure: u32,
    off_dark: u32,
    dark_w: u32,
    dark_h: u32,
    dark_step: u32,
    dark_ox: u32,
    dark_oy: u32,
    airlight: [f32; 3],
    /// The atmosphere's haze color (`Processor::veil`).
    veil: [f32; 3],
    lut_hdr_ev: u32,
    lut_hdr_px: u32,
    lut_hdr_len: u32,
    lut_curve: u32,
    lut_out: [u32; 3],
    lut_out_len: u32,
    lut_luma: u32,
    exposure_ev: f32,
    contrast: f32,
    hdr: [f32; 4],
    clarity: f32,
    structure: f32,
    saturation: f32,
    dehaze: f32,
    vibrance: f32,
    band_center: [f32; 8],
    band_hue: [f32; 8],
    band_sat: [f32; 8],
    band_light: [f32; 8],
    /// hue, chroma, lightness, range, hue/saturation/lightness uniformity, hue shift,
    /// saturation, lightness shift
    skin: [f32; 10],
    balance: [f32; 12],
    bw_mix: [f32; 8],
    bw_tint: [f32; 4],
    /// Color ranges of one's own: center hue, inner and outer edge, hue, saturation, lightness.
    n_zones: u32,
    zones: [f32; 6 * crate::params::MAX_COLOR_ZONES],
    to_lms: [f32; 9],
    from_lms: [f32; 9],
    src_w: f32,
    src_h: f32,
    geo: GeoGpu,
}

/// Layout of `Layer` in develop.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LayerGpu {
    mask_off: u32,
    mask_w: u32,
    mask_h: u32,
    has_bitmap: u32,
    invert: u32,
    has_luma: u32,
    has_color: u32,
    neutral: u32,
    opacity: f32,
    /// Edges: falloff low, low, high, falloff high (`LumaRange::edges`).
    luma: [f32; 4],
    /// hue, width, falloff, minimum chroma
    color: [f32; 4],
    /// exposure, contrast, brightness, saturation, highlights, shadows, clarity, structure,
    /// temperature, tint, dehaze, whites, blacks, vibrance, veil
    adj: [f32; 15],
    /// Color editor per band: hue shift, saturation and lightness (both /100).
    bands: [f32; 24],
    /// Color balance offsets per wheel (`ColorWheel::offset`).
    balance: [f32; 12],
    /// Offsets of the layer's curve tables: red, green, blue, luma (`NO_LUT`: none).
    curve: [u32; 4],
}

const NO_LUT: u32 = u32::MAX;

fn flat(m: &Mat3) -> [f32; 9] {
    [m[0][0], m[0][1], m[0][2], m[1][0], m[1][1], m[1][2], m[2][0], m[2][1], m[2][2]]
}

fn key(s: &str) -> u64 {
    crate::cache::fnv1a(s.as_bytes())
}

/// Uploads a part (x, y, w, h) of an image as RGBA floats.
fn upload_window(g: &Gpu, img: &LinearImage, (x0, y0, w, h): (usize, usize, usize, usize)) -> Buffer {
    let mut data = vec![[0.0f32; 4]; w * h];
    data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let start = (y0 + y) * img.width + x0;
        for (o, p) in row.iter_mut().zip(&img.data[start..start + w]) {
            *o = [p[0], p[1], p[2], 0.0];
        }
    });
    g.upload(&data)
}

/// Rasterized masks on the graphics card (uploaded again only when the mask changes), per
/// device (identified by its address), with the version they hold.
type MaskBuffers = Mutex<Vec<(usize, u64, Weak<MaskBitmap>, Arc<Buffer>)>>;

static MASKS: LazyLock<MaskBuffers> = LazyLock::new(Default::default);

fn mask_buffer(g: &Gpu, bm: &Arc<MaskBitmap>) -> Arc<Buffer> {
    let device = g as *const Gpu as usize;
    let mut masks = MASKS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, _, _, b)) = masks.iter().find(|(d, v, ..)| *d == device && *v == bm.version) {
        return b.clone();
    }
    // While a stroke is painted only a few rows change: those are written into the buffer of
    // the version before, which no rendering asks for any more (and whose bitmap is usually
    // gone already: it became this one).
    let patched = bm.changed.as_ref().and_then(|(from, rows)| {
        let entry = masks.iter_mut().find(|(d, v, ..)| *d == device && v == from)?;
        if !rows.is_empty() {
            let part = &bm.data[rows.start * bm.w..rows.end * bm.w];
            g.queue.write_buffer(&entry.3, (rows.start * bm.w * 4) as u64, bytemuck::cast_slice(part));
        }
        entry.1 = bm.version;
        entry.2 = Arc::downgrade(bm);
        Some(entry.3.clone())
    });
    masks.retain(|(_, _, w, _)| w.strong_count() > 0);
    if let Some(b) = patched {
        return b;
    }
    let b = Arc::new(g.upload(&bm.data));
    masks.push((device, bm.version, Arc::downgrade(bm), b.clone()));
    if masks.len() > 16 {
        masks.remove(0);
    }
    b
}

/// Display tables (DISPLAY_DECODE, DISPLAY_ENCODE_U8) on the graphics card.
fn display_tables(g: &Gpu) -> &(Buffer, Buffer) {
    g.display_tables.get_or_init(|| {
        let enc: Vec<u32> = DISPLAY_ENCODE_U8.iter().map(|&v| v as u32).collect();
        (g.upload(&DISPLAY_DECODE), g.upload(&enc))
    })
}

impl DarkChannel {
    /// From per block minima (r, g, b, luminance) on a `lw` x `lh` grid and airlight
    /// samples; `airlight` given (that of the whole image, for a part of it) the samples are
    /// not used.
    pub(super) fn from_parts(samples: Vec<[f32; 3]>, minima: Vec<[f32; 4]>, lw: usize, lh: usize, step: usize, airlight: Option<[f32; 3]>) -> Self {
        let airlight = airlight.unwrap_or_else(|| airlight_of(samples));
        let ya = dot(airlight, LUMA).max(1e-3);
        // Raw density: the smallest channel relative to the haze's color.
        let dark = Plane::new(lw, lh, minima.iter().map(|m| (0..3).map(|c| m[c] / airlight[c]).fold(f32::MAX, f32::min).clamp(0.0, 1.0)).collect());
        // Guide: the darkest luminance, so that a block on an edge pairs with the density of
        // its dark side (the mean would make it an outlier that leaves a seam).
        let guide = Plane::new(lw, lh, minima.iter().map(|m| m[3] / ya).collect());
        // Guided filter on the block grid: density as a linear function of the guide.
        const R: usize = 2;
        const EPS: f32 = 2e-3;
        let (mi, md) = (box_blur(&guide, R), box_blur(&dark, R));
        let mii = box_blur(&guide.map(|v| v * v), R);
        let mid = box_blur(&guide.zip(&dark, |g, d| g * d), R);
        let a: Vec<f32> = (0..lw * lh)
            .map(|i| {
                let var = (mii.data[i] - mi.data[i] * mi.data[i]).max(0.0);
                (mid.data[i] - mi.data[i] * md.data[i]) / (var + EPS)
            })
            .collect();
        let b: Vec<f32> = (0..lw * lh).map(|i| md.data[i] - a[i] * mi.data[i]).collect();
        // Not smoothed a second time: that would mix in the coefficients of windows on the dark
        // side alone (no slope), which leave a seam on the bright side of an edge.
        let top = (0..lw * lh)
            .map(|i| {
                let (x, y) = ((i % lw) as isize, (i / lw) as isize);
                let mut m = 0.0f32;
                for yy in (y - R as isize).max(0)..=(y + R as isize).min(lh as isize - 1) {
                    for xx in (x - R as isize).max(0)..=(x + R as isize).min(lw as isize - 1) {
                        m = m.max(guide.data[yy as usize * lw + xx as usize]);
                    }
                }
                m
            })
            .collect();
        Self { a: Plane::new(lw, lh, a), b: Plane::new(lw, lh, b), top: Plane::new(lw, lh, top), step, airlight }
    }
}

/// Per block minima and airlight samples of a working region on the graphics card.
struct DarkPool {
    /// Per block: minima of r, g, b and luminance (as float bits).
    blocks: Buffer,
    /// Color of every 7th pixel.
    samples: Buffer,
    count: Buffer,
    lw: usize,
    lh: usize,
    step: usize,
    /// Domain (working region, or whole image for tiles).
    dom: (usize, usize),
}

/// Airlight: the bright end of the smallest channel, neutral (a colored airlight shifts every
/// hue of the dehazed image: a slightly blue sky turns the rest magenta).
pub(super) fn airlight_of(samples: Vec<[f32; 3]>) -> [f32; 3] {
    let mut min_c: Vec<f32> = samples.iter().map(|p| p[0].min(p[1]).min(p[2])).collect();
    let idx = ((min_c.len() as f32 * 0.997) as usize).min(min_c.len().saturating_sub(1));
    let a = if min_c.is_empty() { 1.0 } else { *min_c.select_nth_unstable_by(idx, |a, b| a.total_cmp(b)).1 };
    [a.max(0.05); 3]
}

impl DarkPool {
    /// `step`: block size (see `dark_step`).
    fn new(g: &Gpu, dom: (usize, usize), step: usize) -> Self {
        let (lw, lh) = (dom.0.div_ceil(step), dom.1.div_ceil(step));
        Self {
            blocks: g.upload(&vec![f32::MAX.to_bits(); lw * lh * 4]),
            samples: g.buffer((dom.0 * dom.1).div_ceil(7) as u64 * 12),
            count: g.upload(&[0u32; 4]),
            lw,
            lh,
            step,
            dom,
        }
    }

    /// Adds the pixels `own` (in working region coordinates) of a working region at `origin`.
    fn add(&self, ops: &mut Ops, working: &Buffer, ext: Region, own: Region, origin: (usize, usize)) {
        let a = ops.args(&[
            own.x as u32,
            own.y as u32,
            own.w as u32,
            own.h as u32,
            ext.w as u32,
            origin.0 as u32,
            origin.1 as u32,
            self.dom.0 as u32,
            self.step as u32,
            self.lw as u32,
        ]);
        ops.enc.run_2d(DARK_POOL, &[&a, working, &self.blocks, &self.samples, &self.count], own.w, own.h);
    }

    fn finish(self, g: &Gpu, airlight: Option<[f32; 3]>) -> Result<DarkChannel> {
        let count = g.read::<u32>(&self.count, 1)?[0] as usize;
        // Samples and block minima with one wait.
        let [samples, blocks] = g.read_bytes([(&self.samples, count * 12), (&self.blocks, self.lw * self.lh * 16)])?;
        let samples = bytemuck::pod_collect_to_vec::<u8, f32>(&samples).chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
        let blocks = bytemuck::pod_collect_to_vec::<u8, f32>(&blocks);
        let minima = blocks.chunks_exact(4).map(|b| [b[0], b[1], b[2], b[3]]).collect();
        Ok(DarkChannel::from_parts(samples, minima, self.lw, self.lh, self.step, airlight))
    }
}

impl Processor {
    /// Geometry, heal/clone/removals, white balance, noise reduction -> working RGB of `ext`.
    fn early_gpu(&self, ops: &mut Ops, src: &LinearImage, plan: &Plan, cache: Option<&StageCache>) -> Buffer {
        let g = ops.g;
        let ext = plan.ext;
        let out = ops.image(ext.w * ext.h);
        let mut args = ResampleArgs {
            geo: plan.geo.gpu(),
            region: [ext.x as u32, ext.y as u32, ext.w as u32, ext.h as u32],
            win: [0; 4],
            wb: self.wb,
            cam: flat(&self.cam_to_working),
            hdr: self.hdr as u32,
            to_working: 1,
            rebuilt: self.rebuilt as u32,
        };
        let input = if self.adj.retouch.is_empty() && self.adj.removals.is_empty() {
            let win = plan.geo.source_window(ext);
            args.win = [win.0 as u32, win.1 as u32, win.2 as u32, win.3 as u32];
            let k = key(&format!("{:p}|{}x{}|{}|{win:?}", src.data.as_ptr(), src.width, src.height, crate::ai_products::fingerprint(src)));
            let mut c = cache.map(|c| c.gpu.lock().unwrap_or_else(|e| e.into_inner()));
            match c.as_mut().and_then(|c| c.source.as_ref().filter(|(ck, _)| *ck == k).map(|(_, b)| b.clone())) {
                Some(b) => b,
                None => {
                    let b = Arc::new(upload_window(g, src, win));
                    if let Some(c) = c.as_mut() {
                        c.source = Some((k, b.clone()));
                    }
                    b
                }
            }
        } else {
            // Heal/clone and removals stay on the CPU: resample there and let the graphics
            // card only copy pixel by pixel. Kept apart from white balance and noise
            // reduction, so that moving those sliders doesn't repeat it.
            let id = GeoMap::new(&Default::default(), &Default::default(), None, 0, ext.w, ext.h, false);
            args.geo = id.gpu();
            args.region = [0, 0, ext.w as u32, ext.h as u32];
            args.win = [0, 0, ext.w as u32, ext.h as u32];
            let k = key(&format!(
                "{:p}|{}x{}|{}|{ext:?}|{}",
                src.data.as_ptr(),
                src.width,
                src.height,
                crate::ai_products::fingerprint(src),
                self.retouch_relevant()
            ));
            let mut c = cache.map(|c| c.gpu.lock().unwrap_or_else(|e| e.into_inner()));
            match c.as_mut().and_then(|c| c.source.as_ref().filter(|(ck, _)| *ck == k).map(|(_, b)| b.clone())) {
                Some(b) => b,
                None => {
                    let mut img = plan.geo.resample(src, ext);
                    apply_removals(&mut img, src, &plan.geo, ext, &self.adj.removals);
                    apply_retouch(&mut img, src, &plan.geo, ext, &self.adj.retouch);
                    let b = Arc::new(upload_window(g, &img, (0, 0, ext.w, ext.h)));
                    if let Some(c) = c.as_mut() {
                        c.source = Some((k, b.clone()));
                    }
                    b
                }
            }
        };
        let a = ops.args(&args);
        ops.enc.run_2d(RESAMPLE, &[&a, &input, &out], ext.w, ext.h);
        self.defringe_gpu(ops, &out, ext.w, ext.h, plan.scale);
        self.noise_reduction_gpu(ops, &out, ext.w, ext.h, plan.scale);
        out
    }

    fn defringe_gpu(&self, ops: &mut Ops, img: &Buffer, w: usize, h: usize, scale: f32) {
        #[repr(C)]
        #[derive(Clone, Copy, Pod, Zeroable)]
        struct Args {
            n: u32,
            purple: f32,
            green: f32,
        }
        let Some([purple, green]) = defringe_amounts(&self.adj.lens) else { return };
        let n = w * h;
        let s = ops.img_planes(IMG_SQRT_LUMA, img, n).0;
        let sq = ops.op(OP_SQUARE, 0.0, &s, &s, &s, n);
        let sigma = defringe_sigma(scale);
        let mean = ops.gaussian(&s, w, h, sigma);
        let mean_sq = ops.gaussian(&sq, w, h, sigma);
        let a = ops.args(&Args { n: n as u32, purple, green });
        ops.enc.run_1d(DEFRINGE, &[&a, img, &mean, &mean_sq], n);
    }

    fn noise_reduction_gpu(&self, ops: &mut Ops, img: &Buffer, w: usize, h: usize, scale: f32) {
        let d = &self.adj.detail;
        if d.nr_luma <= 0.0 && d.nr_color <= 0.0 {
            return;
        }
        let n = w * h;
        let y_new = (d.nr_luma > 0.0).then(|| {
            let y = ops.img_planes(IMG_SQRT_LUMA, img, n).0;
            let r = ((2.0 * scale).round() as usize).max(1);
            let eps = (d.nr_luma / 100.0).powi(2) * 0.0025;
            let f = ops.guided(&y, w, h, r, eps);
            let keep = 1.0 - d.nr_luma / 100.0 * 0.85;
            ops.op(OP_KEEP, keep, &y, &f, &f, n)
        });
        let chroma = (d.nr_color > 0.0).then(|| {
            let sigma = (d.nr_color / 100.0 * 6.0 * scale).max(0.5);
            let (cr, cb) = ops.img_planes(IMG_CHROMA, img, n);
            (ops.gaussian(&cr, w, h, sigma), ops.gaussian(&cb, w, h, sigma))
        });
        let a = ops.args(&[n as u32, y_new.is_some() as u32, chroma.is_some() as u32]);
        let dummy = &ops.dummy;
        let y = y_new.as_ref().unwrap_or(dummy);
        let (cr, cb) = chroma.as_ref().map_or((dummy, dummy), |(r, b)| (r, b));
        ops.enc.run_1d(NR_COMBINE, &[&a, img, y, cr, cb], n);
    }

    /// Local operator planes (smoothed log luminance, clarity, structure, dark channel).
    fn local_gpu(&self, ops: &mut Ops, working: &Buffer, plan: &Plan, global_dark: Option<&DarkChannel>, airlight: Option<[f32; 3]>) -> Result<Local> {
        let (w, h) = (plan.ext.w, plan.ext.h);
        let n = w * h;
        let log_l = ops.img_planes(IMG_LOG_LUMA, working, n).0;
        let sub = plan.guided_sub;
        let ev = plan.hdr_active.then(|| {
            let base = match super::median_radius(plan.r_hdr) {
                r if r > 0 && self.adj.process_3() => ops.median(&log_l, w, h, r),
                _ => log_l.clone(),
            };
            ops.fast_guided(&base, w, h, plan.r_hdr as usize, 0.5, sub)
        });
        let clarity = plan.clarity_active.then(|| {
            let base = ops.fast_guided(&log_l, w, h, plan.r_clarity as usize, 0.3, sub);
            ops.op(OP_DETAIL, 3.0, &log_l, &base, &base, n)
        });
        let structure = plan.structure_active.then(|| {
            let base = ops.gaussian(&log_l, w, h, plan.r_structure);
            ops.op(OP_DETAIL, 2.0, &log_l, &base, &base, n)
        });
        let dark = if !plan.dehaze_active {
            None
        } else if let Some(dc) = global_dark {
            Some(([dc.a.clone(), dc.b.clone(), dc.top.clone()], dc.step, dc.airlight, (plan.ext.x as u32, plan.ext.y as u32)))
        } else {
            let pool = DarkPool::new(ops.g, (w, h), super::dark_step(plan.full));
            pool.add(ops, working, plan.ext, Region { x: 0, y: 0, w, h }, (0, 0));
            ops.flush();
            let dc = pool.finish(ops.g, airlight)?;
            Some(([dc.a, dc.b, dc.top], dc.step, dc.airlight, (0, 0)))
        };
        let planes = [ev.as_ref(), clarity.as_ref(), structure.as_ref()];
        let dark_len = dark.as_ref().map_or(0, |d| d.0[0].data.len() * 3);
        let total = planes.iter().flatten().count() * n + dark_len;
        let buf = ops.plane(total);
        let mut off = [0u32; 4];
        let mut at = 0usize;
        for (i, p) in planes.iter().enumerate() {
            if let Some(p) = p {
                off[i] = at as u32;
                ops.enc.copy(p, &buf, at as u64 * 4, n as u64 * 4);
                at += n;
            }
        }
        // The guided filter's coefficients a and b, then the guide's limit.
        let dark = dark.map(|(planes, step, airlight, origin)| {
            off[3] = at as u32;
            let d = ops.g.upload(&planes.iter().flat_map(|p| p.data.iter().copied()).collect::<Vec<f32>>());
            ops.enc.copy(&d, &buf, at as u64 * 4, planes[0].data.len() as u64 * 12);
            let (w, h) = (planes[0].w as u32, planes[0].h as u32);
            DarkInfo { w, h, step: step as u32, airlight, origin }
        });
        Ok(Local { buf, off, has: [ev.is_some(), clarity.is_some(), structure.is_some(), dark.is_some()], dark })
    }

    /// Everything up to the encoded result (with mask overlay weight in w) of `plan.ext`.
    /// `early`: the early stages of `plan.ext`, when already computed.
    fn develop_gpu(
        &self,
        ops: &mut Ops,
        src: &LinearImage,
        plan: &Plan,
        cache: Option<&StageCache>,
        global_dark: Option<&DarkChannel>,
        early: Option<Arc<Buffer>>,
    ) -> Result<Buffer> {
        let g = ops.g;
        let ext = plan.ext;
        let n = ext.w * ext.h;
        if n as u64 * 16 > g.max_binding {
            return Err(Fallback(format!("Ausschnitt {}x{} zu groß für die Grafikkarte", ext.w, ext.h)).into());
        }
        let stage_key = self.stage_key(src, ext);
        // A part of the image whose whole is in the cache (painting renders only what the brush
        // changed): cut out of it, which is much faster and gives exactly the whole rendering.
        let full_w = plan.full.0;
        let whole_key = cache.filter(|_| plan.partial() && global_dark.is_none()).map(|_| self.stage_key(src, Region { x: 0, y: 0, w: full_w, h: plan.full.1 }));
        let whole = cache.zip(whole_key).and_then(|(c, k)| c.gpu.lock().unwrap_or_else(|e| e.into_inner()).early.as_ref().filter(|(ck, _)| *ck == k).map(|(_, b)| b.clone()));
        let cached = early.or_else(|| cache.and_then(|c| c.gpu.lock().unwrap_or_else(|e| e.into_inner()).early.as_ref().filter(|(k, _)| *k == stage_key).map(|(_, b)| b.clone())));
        let working = match (&whole, cached) {
            (Some(full), _) => {
                let b = ops.image(n);
                ops.crop_into((full, 0), full_w, ext, 16, (&b, 0));
                Arc::new(b)
            }
            (None, Some(b)) => b,
            (None, None) => {
                let b = Arc::new(self.early_gpu(ops, src, plan, cache));
                if let Some(c) = cache {
                    ops.keep(&b);
                    c.gpu.lock().unwrap_or_else(|e| e.into_inner()).early = Some((stage_key, b.clone()));
                }
                b
            }
        };


        let need_log = plan.hdr_active || plan.clarity_active || plan.structure_active || plan.dehaze_active;
        let local = if need_log {
            let local_key = |stage: u64| key(&format!("{stage}|{}|{}|{}|{}|{}", plan.hdr_active, plan.clarity_active, plan.structure_active, plan.dehaze_active, self.adj.process_3()));
            let k = local_key(stage_key);
            // Cut out of the whole image's planes as well (see `whole`).
            let whole_local = whole.as_ref().and(cache).zip(whole_key).and_then(|(c, wk)| {
                let k = local_key(wk);
                c.gpu.lock().unwrap_or_else(|e| e.into_inner()).local.as_ref().filter(|(ck, _)| *ck == k).map(|(_, l)| l.clone())
            });
            let cached = cache.and_then(|c| c.gpu.lock().unwrap_or_else(|e| e.into_inner()).local.as_ref().filter(|(ck, _)| *ck == k).map(|(_, l)| l.clone()));
            match (whole_local, cached) {
                (Some(full), _) => Some(Arc::new(full.crop(ops, full_w, ext))),
                (None, Some(l)) if global_dark.is_none() => Some(l),
                _ => {
                    // Haze measured on the whole (the same for every part), unless the tiles
                    // bring it with the global dark channel.
                    let airlight = (plan.dehaze_active && global_dark.is_none()).then(|| self.whole_airlight(src));
                    let l = Arc::new(self.local_gpu(ops, &working, plan, global_dark, airlight)?);
                    if let Some(c) = cache.filter(|_| global_dark.is_none()) {
                        ops.keep(&l.buf);
                        c.gpu.lock().unwrap_or_else(|e| e.into_inner()).local = Some((k, l.clone()));
                    }
                    Some(l)
                }
            }
        } else {
            None
        };


        // Tables: HDR gain, contrast/base curve, output curves.
        let mut luts: Vec<f32> = Vec::new();
        let mut push = |t: &[f32]| {
            let off = luts.len() as u32;
            luts.extend_from_slice(t);
            off
        };
        let (lut_hdr_ev, lut_hdr_px, lut_hdr_len) = match &self.tone_luts.hdr {
            Some([a, b]) => (push(a), push(b), a.len() as u32),
            None => (NO_LUT, NO_LUT, 2),
        };
        // The camera's curve takes the place of the contrast/base curve (base_curve 3).
        let lut_curve = self.tone_luts.camera.as_ref().or(self.tone_luts.curve.as_ref()).map_or(NO_LUT, |c| push(c));
        let lut_out = [push(&self.lut[0]), push(&self.lut[1]), push(&self.lut[2])];
        let lut_luma = self.luma_lut.as_ref().map_or(NO_LUT, |l| push(l));
        let layer_curves: Vec<[u32; 4]> = self
            .layers
            .iter()
            .map(|l| {
                let Some(c) = &l.curve else { return [NO_LUT; 4] };
                let [r, g, b] = c.channels.as_ref().map_or([NO_LUT; 3], |t| [push(&t[0]), push(&t[1]), push(&t[2])]);
                [r, g, b, c.luma.as_ref().map_or(NO_LUT, |t| push(t))]
            })
            .collect();
        let luts = g.upload(&luts);

        // Layers and their masks.
        let aspect = src.width as f32 / src.height.max(1) as f32;
        let mut masks: Vec<(Arc<Buffer>, usize)> = Vec::new();
        let mut layers: Vec<LayerGpu> = Vec::new();
        let mut mask_len = 0usize;
        for (l, curve) in self.layers.iter().zip(layer_curves) {
            let mut lg = LayerGpu::zeroed();
            lg.curve = curve;
            if !l.mask.is_empty() {
                let bm = cached_bitmap(&l.mask, aspect, Some(src));
                lg.mask_off = mask_len as u32;
                lg.mask_w = bm.w as u32;
                lg.mask_h = bm.h as u32;
                lg.has_bitmap = 1;
                masks.push((mask_buffer(g, &bm), bm.data.len()));
                mask_len += bm.data.len();
            }
            lg.invert = l.mask.invert as u32;
            if let Some(r) = &l.mask.luma_range {
                lg.has_luma = 1;
                lg.luma = r.edges();
            }
            if let Some(c) = &l.mask.color_range {
                lg.has_color = 1;
                lg.color = [c.hue, c.width, c.falloff, c.min_chroma];
            }
            lg.neutral = !l.adds as u32;
            lg.opacity = l.opacity;
            let a = &l.adj;
            lg.adj = [
                a.exposure, a.contrast, a.brightness, a.saturation, a.highlights, a.shadows, a.clarity, a.structure, a.temperature, a.tint, a.dehaze,
                a.whites, a.blacks, a.vibrance, l.veil,
            ];
            for (i, b) in a.color_bands.iter().enumerate() {
                lg.bands[i * 3..i * 3 + 3].copy_from_slice(&[b.hue, b.saturation / 100.0, b.lightness / 100.0]);
            }
            let cb = &a.color_balance;
            for (i, w) in [cb.master, cb.shadows, cb.midtones, cb.highlights].iter().enumerate() {
                lg.balance[i * 3..i * 3 + 3].copy_from_slice(&w.offset());
            }
            layers.push(lg);
        }
        let layer_buf = if layers.is_empty() { g.upload(&[LayerGpu::zeroed()]) } else { g.upload(&layers) };
        let mask_buf = match masks.as_slice() {
            [] => Arc::new(g.buffer(16)),
            [(b, _)] => b.clone(),
            _ => {
                let all = ops.plane(mask_len);
                let mut at = 0u64;
                for (b, len) in &masks {
                    ops.enc.copy(b, &all, at * 4, *len as u64 * 4);
                    at += *len as u64;
                }
                Arc::new(all)
            }
        };


        let adj = &self.adj;
        let oklab_to = crate::color::working_to_lms();
        let st = &adj.skin_tone;
        let cb = &adj.color_balance;
        let bw = &adj.black_white;
        let local_info = local.as_ref();
        let has = |i: usize| local_info.is_some_and(|l| l.has[i]) as u32;
        let off = |i: usize| local_info.map_or(0, |l| l.off[i]);
        let dark = local_info.and_then(|l| l.dark);
        let params = DevelopParams {
            w: ext.w as u32,
            h: ext.h as u32,
            ext_x: ext.x as u32,
            ext_y: ext.y as u32,
            n_layers: self.layers.len() as u32,
            overlay: self.opts.mask_overlay.and_then(|i| self.layers.iter().position(|l| l.index == i)).map_or(-1, |p| p as i32),
            has_ev: has(0),
            has_clarity: has(1),
            has_structure: has(2),
            has_dark: has(3),
            color_active: self.color_active as u32,
            editor_active: self.editor_active as u32,
            skin_active: self.skin_active as u32,
            balance_active: self.balance_active as u32,
            bw_enabled: bw.enabled as u32,
            base_curve: match adj.base_curve {
                BaseCurve::Camera if self.tone_luts.camera.is_some() => 3,
                BaseCurve::Camera => BaseCurve::FilmStandard as u32,
                b => b as u32,
            },
            hue_preserve: (adj.process_3() && self.tone_luts.camera.is_none()) as u32,
            off_ev: off(0),
            off_clarity: off(1),
            off_structure: off(2),
            off_dark: off(3),
            dark_w: dark.map_or(1, |d| d.w),
            dark_h: dark.map_or(1, |d| d.h),
            dark_step: dark.map_or(1, |d| d.step),
            dark_ox: dark.map_or(0, |d| d.origin.0),
            dark_oy: dark.map_or(0, |d| d.origin.1),
            airlight: dark.map_or([1.0; 3], |d| d.airlight),
            veil: self.veil(src),
            lut_hdr_ev,
            lut_hdr_px,
            lut_hdr_len,
            lut_curve,
            lut_out,
            lut_out_len: LUT_SIZE as u32,
            lut_luma,
            exposure_ev: self.base.exposure_ev,
            contrast: self.base.contrast,
            hdr: self.base.hdr,
            clarity: self.base.clarity,
            structure: self.base.structure,
            saturation: self.base.saturation,
            dehaze: self.base.dehaze,
            vibrance: adj.vibrance,
            band_center: HUE_BANDS.map(|b| b.1),
            band_hue: adj.color_editor.bands.map(|b| b.hue),
            band_sat: adj.color_editor.bands.map(|b| b.saturation),
            band_light: adj.color_editor.bands.map(|b| b.lightness),
            skin: [
                st.hue,
                st.chroma,
                st.lightness,
                st.range,
                st.hue_uniformity,
                st.saturation_uniformity,
                st.lightness_uniformity,
                st.hue_shift,
                st.saturation,
                st.lightness_shift,
            ],
            balance: [cb.master, cb.shadows, cb.midtones, cb.highlights]
                .iter()
                .flat_map(|w| w.offset())
                .collect::<Vec<_>>()
                .try_into()
                .expect("4 wheels"),
            bw_mix: bw.mix,
            bw_tint: [bw.shadow_tint.hue, bw.shadow_tint.amount, bw.highlight_tint.hue, bw.highlight_tint.amount],
            n_zones: adj.color_editor.active_zones().count() as u32,
            zones: {
                let mut z = [0.0; 6 * crate::params::MAX_COLOR_ZONES];
                for (i, zone) in adj.color_editor.active_zones().enumerate() {
                    let (inner, outer) = zone.edges();
                    z[i * 6..i * 6 + 6].copy_from_slice(&[zone.hue, inner, outer, zone.adjust.hue, zone.adjust.saturation, zone.adjust.lightness]);
                }
                z
            },
            to_lms: flat(&oklab_to),
            from_lms: flat(&mat_inv(&oklab_to).expect("invertible")),
            src_w: src.width as f32,
            src_h: src.height as f32,
            geo: plan.geo.gpu(),
        };
        let p = ops.args(&params);
        let out = ops.image(n);
        let local_buf = local.as_ref().map(|l| &l.buf).unwrap_or(&ops.dummy);
        ops.enc.run_2d(DEVELOP, &[&p, &working, local_buf, &luts, &layer_buf, &mask_buf, &out], ext.w, ext.h);

        let d = &adj.detail;
        let unsharp = d.sharpen_split().0;
        if unsharp > 0.0 {
            let sigma = d.sharpen_radius * plan.scale;
            if sigma >= 0.25 {
                let y = ops.img_planes(IMG_LUMA, &out, n).0;
                let blur = ops.gaussian(&y, ext.w, ext.h, sigma);
                #[repr(C)]
                #[derive(Clone, Copy, Pod, Zeroable)]
                struct Args {
                    n: u32,
                    amount: f32,
                    thr: f32,
                }
                let a = ops.args(&Args { n: n as u32, amount: unsharp / 100.0, thr: d.sharpen_threshold / 255.0 });
                ops.enc.run_1d(SHARPEN, &[&a, &out, &y, &blur], n);
            }
        }
        if d.glow_amount > 0.0 {
            let sigma = super::glow_sigma(d, plan.full.0.max(plan.full.1) as f32);
            let blurred: Vec<Buffer> = (0..3).map(|c| {
                let ch = ops.img_planes(IMG_RED + c, &out, n).0;
                ops.gaussian(&ch, ext.w, ext.h, sigma)
            }).collect();
            #[repr(C)]
            #[derive(Clone, Copy, Pod, Zeroable)]
            struct Args {
                n: u32,
                k: f32,
            }
            let a = ops.args(&Args { n: n as u32, k: d.glow_amount / 100.0 });
            ops.enc.run_1d(GLOW, &[&a, &out, &blurred[0], &blurred[1], &blurred[2]], n);
        }
        if d.vignette_amount != 0.0 || d.grain_amount > 0.0 {
            #[repr(C)]
            #[derive(Clone, Copy, Pod, Zeroable)]
            struct Args {
                w: u32,
                h: u32,
                ext_x: u32,
                ext_y: u32,
                full_w: f32,
                full_h: f32,
                vignette: f32,
                grain: f32,
                cell: f32,
            }
            let a = ops.args(&Args {
                w: ext.w as u32,
                h: ext.h as u32,
                ext_x: ext.x as u32,
                ext_y: ext.y as u32,
                full_w: plan.full.0 as f32,
                full_h: plan.full.1 as f32,
                vignette: d.vignette_amount / 100.0,
                grain: d.grain_amount / 100.0 * 0.12,
                cell: (1.0 + d.grain_size / 100.0 * 4.0) * plan.scale,
            });
            ops.enc.run_2d(FINISH, &[&a, &out], ext.w, ext.h);
        }
        Ok(out)
    }

    /// Crops `plan.region` out of a developed buffer, optionally converted into `space`.
    fn output_gpu(&self, ops: &mut Ops, developed: &Buffer, plan: &Plan, space: Option<OutputSpace>) -> Result<Vec<[f32; 4]>> {
        #[repr(C)]
        #[derive(Clone, Copy, Pod, Zeroable)]
        struct Args {
            w: u32,
            h: u32,
            ext_w: u32,
            off_x: u32,
            off_y: u32,
            convert: u32,
            gamma: f32,
            gamut: u32,
            m: [f32; 9],
        }
        let (r, ext) = (plan.region, plan.ext);
        let a = ops.args(&Args {
            w: r.w as u32,
            h: r.h as u32,
            ext_w: ext.w as u32,
            off_x: (r.x - ext.x) as u32,
            off_y: (r.y - ext.y) as u32,
            convert: space.is_some() as u32,
            gamma: match space.map(|s| s.trc()) {
                Some(crate::color::Trc::Gamma(g)) => g,
                _ => 0.0,
            },
            gamut: self.adj.gamut_mapping() as u32,
            m: flat(&space.unwrap_or_default().from_working()),
        });
        let out = ops.image(r.w * r.h);
        ops.enc.run_2d(OUTPUT, &[&a, developed, &out], r.w, r.h);
        ops.flush();
        #[allow(unused_mut)]
        let mut data = ops.g.read::<[f32; 4]>(&out, r.w * r.h)?;
        if space.is_some() {
            #[cfg(test)]
            if ops.g.inject_incomplete.swap(false, std::sync::atomic::Ordering::Relaxed) {
                data[0][3] = 0.0;
            }
            check_output(ops.g, &data)?;
        }
        Ok(data)
    }

    pub(super) fn render_gpu(&self, g: &Gpu, src: &LinearImage, region: Option<Region>, cache: Option<&StageCache>) -> Result<Rendered> {
        let r = self.render_gpu_inner(g, src, region, cache);
        // Buffers are cached while the work is recorded: after a failure some may never have
        // been computed (an invalid command buffer runs none of its work). A fallback that
        // isn't the card's fault keeps them.
        if let Err(e) = &r
            && e.downcast_ref::<Fallback>().is_none()
            && let Some(c) = cache
        {
            *c.gpu.lock().unwrap_or_else(|e| e.into_inner()) = Cache::default();
        }
        r
    }

    fn render_gpu_inner(&self, g: &Gpu, src: &LinearImage, region: Option<Region>, cache: Option<&StageCache>) -> Result<Rendered> {
        g.checked(|| {
            let plan = self.plan(src, region);
            let mut ops = Ops::new(g);
            let developed = self.develop_gpu(&mut ops, src, &plan, cache, None, None)?;
            let (r, ext) = (plan.region, plan.ext);
            let has_mask = self.opts.mask_overlay.is_some_and(|i| self.layers.iter().any(|l| l.index == i));
            if self.opts.softproof.is_some() {
                let data = self.output_gpu(&mut ops, &developed, &plan, None)?;
                let enc = LinearImage::new(r.w, r.h, data.iter().map(|p| [p[0], p[1], p[2]]).collect());
                let mask = has_mask.then(|| data.iter().map(|p| p[3]).collect());
                return Ok(self.display(enc, plan.full, mask));
            }
            #[repr(C)]
            #[derive(Clone, Copy, Pod, Zeroable)]
            struct Args {
                w: u32,
                h: u32,
                ext_w: u32,
                off_x: u32,
                off_y: u32,
                clip: u32,
                mask: u32,
                gamut: u32,
                to_srgb: [f32; 9],
            }
            let a = ops.args(&Args {
                w: r.w as u32,
                h: r.h as u32,
                ext_w: ext.w as u32,
                off_x: (r.x - ext.x) as u32,
                off_y: (r.y - ext.y) as u32,
                clip: self.opts.clip_warning as u32,
                mask: has_mask as u32,
                gamut: self.adj.gamut_mapping() as u32,
                to_srgb: flat(&OutputSpace::Srgb.from_working()),
            });
            let (decode, encode8) = display_tables(g);
            let rgba = g.buffer(r.w as u64 * r.h as u64 * 4);
            let hist = g.upload(&[0u32; 1024]);
            ops.enc.run_2d(DISPLAY, &[&a, &developed, decode, encode8, &rgba, &hist], r.w, r.h);
            ops.flush();
            // Image and histogram with one wait.
            #[allow(unused_mut)]
            let [mut rgba, h] = g.read_bytes([(&rgba, r.w * r.h * 4), (&hist, 1024 * 4)])?;
            let h = bytemuck::pod_collect_to_vec::<u8, u32>(&h);
            #[cfg(test)]
            if g.inject_incomplete.swap(false, std::sync::atomic::Ordering::Relaxed) {
                rgba[3] = 0;
            }
            // A part of the image may well be black (a keystone corner, a night sky).
            check_display(g, &rgba, plan.region.w < plan.full.0 || plan.region.h < plan.full.1)?;
            let histogram = Histogram { r: h[0..256].to_vec(), g: h[256..512].to_vec(), b: h[512..768].to_vec(), luma: h[768..1024].to_vec() };
            Ok(Rendered { width: r.w, height: r.h, rgba, histogram, full_size: plan.full })
        })
    }

    pub(super) fn process_gpu(&self, g: &Gpu, src: &LinearImage, region: Option<Region>, space: OutputSpace) -> Result<LinearImage> {
        g.checked(|| {
            let whole = self.plan(src, region);
            let r = whole.region;
            let ext_px = whole.ext.w * whole.ext.h;
            let tiles: Vec<Region> = if ext_px <= MAX_PIXELS && ext_px as u64 * 16 <= g.max_binding {
                vec![r]
            } else {
                let tile = tile_size(whole.margin, g.max_binding)
                    .with_context(|| format!("Rand von {} Pixeln zu groß für die Grafikkarte", whole.margin))?;
                (0..r.h.div_ceil(tile))
                    .flat_map(|ty| (0..r.w.div_ceil(tile)).map(move |tx| (tx, ty)))
                    .map(|(tx, ty)| {
                        let (x, y) = (r.x + tx * tile, r.y + ty * tile);
                        Region { x, y, w: tile.min(r.x + r.w - x), h: tile.min(r.y + r.h - y) }
                    })
                    .collect()
            };
            // Dehaze needs the dark channel and airlight of the whole image.
            let mut last_early = None;
            let dark = if tiles.len() > 1 && whole.dehaze_active {
                let pool = DarkPool::new(g, whole.full, super::dark_step(whole.full));
                for (i, t) in tiles.iter().enumerate() {
                    let plan = self.plan(src, Some(*t));
                    let mut ops = Ops::new(g);
                    let working = self.early_gpu(&mut ops, src, &plan, None);
                    let own = Region { x: t.x - plan.ext.x, y: t.y - plan.ext.y, w: t.w, h: t.h };
                    pool.add(&mut ops, &working, plan.ext, own, (plan.ext.x, plan.ext.y));
                    ops.flush();
                    // The last tile is developed first, without its early stages again.
                    if i + 1 == tiles.len() {
                        ops.keep(&working);
                        last_early = Some(Arc::new(working));
                    }
                }
                Some(pool.finish(g, Some(self.whole_airlight(src)))?)
            } else {
                None
            };
            let mut out = vec![[0.0f32; 3]; r.w * r.h];
            for t in tiles.iter().rev() {
                let plan = self.plan(src, Some(*t));
                let mut ops = Ops::new(g);
                let developed = self.develop_gpu(&mut ops, src, &plan, None, dark.as_ref(), last_early.take())?;
                let data = self.output_gpu(&mut ops, &developed, &plan, Some(space))?;
                for (y, row) in data.chunks(t.w).enumerate() {
                    let start = (t.y - r.y + y) * r.w + t.x - r.x;
                    for (o, p) in out[start..start + t.w].iter_mut().zip(row) {
                        *o = [p[0], p[1], p[2]];
                    }
                }
            }
            // Over the whole image: a single tile may well be black (e.g. a panorama's corner).
            if out.iter().all(|p| *p == [0.0; 3]) {
                bail!("Grafikkarte: schwarzes Ergebnis, wird auf dem Prozessor geprüft");
            }
            Ok(LinearImage::new(r.w, r.h, out))
        })
    }
}

/// Every displayed pixel is written with alpha 255 into a buffer that starts out zeroed: any
/// other alpha means the card didn't finish the work (driver problem), and it is taken out of
/// use. An all-black whole image is suspicious too; it is redone on the CPU once, without
/// blaming the card (the image may simply be black).
fn check_display(g: &Gpu, rgba: &[u8], partial: bool) -> Result<()> {
    if rgba.chunks_exact(4).any(|p| p[3] != 255) {
        g.fail("Grafikkarte lieferte ein unvollständiges Bild");
        bail!("Grafikkarte: unvollständiges Bild");
    }
    if !partial && rgba.chunks_exact(4).all(|p| p[..3] == [0, 0, 0]) {
        bail!("Grafikkarte: schwarzes Bild, wird auf dem Prozessor geprüft");
    }
    Ok(())
}

/// Like [`check_display`] for converted output: the kernel marks each written pixel with w = 1.
/// (An all-black result is checked over the whole image, see `process_gpu`.) A value that is not
/// finite is redone on the CPU without blaming the card: it may come from the image data.
fn check_output(g: &Gpu, data: &[[f32; 4]]) -> Result<()> {
    if data.iter().any(|p| p[3] != 1.0) {
        g.fail("Grafikkarte lieferte ein unvollständiges Ergebnis");
        bail!("Grafikkarte: unvollständiges Ergebnis");
    }
    if data.iter().any(|p| !p[..3].iter().all(|v| v.is_finite())) {
        return Err(Fallback("Grafikkarte: ungültige Werte im Ergebnis, wird auf dem Prozessor gerechnet".into()).into());
    }
    Ok(())
}

/// Output pixels per side of an export tile: at most [`TILE`], and small enough that the tile
/// with `margin` on every side fits into one buffer binding (16 bytes per pixel). `None` if not
/// even a small tile fits.
fn tile_size(margin: usize, max_binding: u64) -> Option<usize> {
    const MIN: usize = 256;
    let fits = |t: usize| ((t + 2 * margin) as u64).pow(2) * 16 <= max_binding;
    (MIN..=TILE).rev().step_by(64).find(|&t| fits(t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::{CameraColor, IDENTITY, REC2020_TO_XYZ};
    use crate::detail::hash_noise;
    use crate::image::Orientation;
    use crate::params::*;
    use crate::raw::CaptureInfo;

    /// A device for the comparisons with the CPU: the graphics card, or a software adapter
    /// (lavapipe / WARP) on machines without one. Only these tests use it; everything else in
    /// the tests runs on the CPU, which is much faster than a software adapter. `None` (the
    /// test is skipped) with `SILBERKORN_GPU=0` or without any adapter.
    fn gpu() -> Option<&'static Gpu> {
        static TEST_GPU: std::sync::OnceLock<Option<Gpu>> = std::sync::OnceLock::new();
        let g = TEST_GPU.get_or_init(Gpu::new_for_test).as_ref();
        if g.is_none() {
            eprintln!("keine Grafikkarte: Vergleich übersprungen");
        }
        g
    }

    #[test]
    fn export_tiles_leave_room_for_the_margin() {
        assert_eq!(tile_size(0, 1 << 31), Some(TILE));
        // 6 % clarity margin of a 40000 px panorama on a card with 2 GB bindings.
        assert_eq!(tile_size(2400, 1 << 31), Some(TILE));
        let t = tile_size(500, 128 << 20).unwrap();
        assert!(((t + 1000) as u64).pow(2) * 16 <= 128 << 20 && ((t + 64 + 1000) as u64).pow(2) * 16 > 128 << 20, "{t}");
        assert_eq!(tile_size(2400, 256 << 20), None);
    }

    #[test]
    fn a_black_tile_does_not_send_the_export_to_the_cpu() {
        // A device of its own with small bindings, for small tiles.
        let Some(mut g) = Gpu::new_for_test() else { return };
        g.max_binding = 4 << 20;
        // Black left part (like a panorama's empty corner), wide enough for whole black tiles.
        let (w, h) = (TILE * 2 + 64, 300);
        let src = LinearImage::new(w, h, (0..w * h).map(|i| if i % w < TILE + 32 { [0.0; 3] } else { [0.3; 3] }).collect());
        let p = Processor::new(&meta(w, h), &Adjustments::default());
        let out = p.process_gpu(&g, &src, None, OutputSpace::Srgb).expect("auf der Grafikkarte");
        assert!(out.data[w - 1][0] > 0.1);
    }

    #[test]
    fn incomplete_results_take_the_card_out_of_use() {
        // A device of its own: the shared one stays usable for the other tests.
        let Some(g) = Gpu::new_for_test() else { return };
        let (w, h) = (120, 80);
        let src = image(w, h);
        let p = Processor::new(&meta(w, h), &Adjustments::default());
        assert!(p.render_gpu(&g, &src, None, None).is_ok());
        g.inject_incomplete.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(p.render_gpu(&g, &src, None, None).is_err());
        assert!(g.problem().is_some_and(|why| why.contains("unvollständig")));
        // Exports are checked the same way.
        let Some(g) = Gpu::new_for_test() else { return };
        g.inject_incomplete.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(p.process_gpu(&g, &src, None, OutputSpace::Srgb).is_err());
        assert!(g.problem().is_some());
    }

    /// Running out of video memory is not a defect: that rendering fails (and the CPU takes
    /// over), but the card stays in use. (Where even the device doesn't fit into the budget, as
    /// on Vulkan, there is nothing to check; it must just not panic.)
    #[test]
    fn out_of_memory_keeps_the_card() {
        let Some(g) = Gpu::new_starved_for_test() else { return };
        let (w, h) = (1200, 800);
        let src = image(w, h);
        let p = Processor::new(&meta(w, h), &Adjustments::default());
        match p.render_gpu(&g, &src, None, None) {
            // Without a budget from the driver (no VK_EXT_memory_budget) nothing is refused.
            Ok(_) => eprintln!("kein Speicherbudget, Test übersprungen"),
            Err(e) => {
                assert!(format!("{e:#}").contains("Grafikspeicher voll"), "{e:#}");
                assert_eq!(g.problem(), None);
            }
        }
    }

    #[test]
    fn failed_render_empties_the_stage_cache() {
        let Some(g) = Gpu::new_for_test() else { return };
        let (w, h) = (120, 80);
        let src = image(w, h);
        let p = Processor::new(&meta(w, h), &Adjustments::default());
        let cache = StageCache::default();
        assert!(p.render_gpu(&g, &src, None, Some(&cache)).is_ok());
        assert!(cache.gpu.lock().unwrap_or_else(|e| e.into_inner()).early.is_some());
        g.inject_incomplete.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(p.render_gpu(&g, &src, None, Some(&cache)).is_err());
        let c = cache.gpu.lock().unwrap_or_else(|e| e.into_inner());
        assert!(c.source.is_none() && c.early.is_none() && c.local.is_none());
    }

    #[test]
    fn black_part_and_too_large_part_keep_the_stage_cache() {
        let Some(mut g) = Gpu::new_for_test() else { return };
        let (w, h) = (120, 80);
        // Black left half (a keystone corner, a night sky).
        let src = LinearImage::new(w, h, (0..w * h).map(|i| if i % w < 60 { [0.0; 3] } else { [0.3; 3] }).collect());
        let p = Processor::new(&meta(w, h), &Adjustments::default());
        let cache = StageCache::default();
        assert!(p.render_gpu(&g, &src, None, Some(&cache)).is_ok());
        let black = p.render_gpu(&g, &src, Some(Region { x: 0, y: 0, w: 40, h: 30 }), Some(&cache)).expect("schwarzer Ausschnitt auf der Grafikkarte");
        assert!(black.rgba.chunks_exact(4).all(|p| p[..3] == [0, 0, 0]));
        assert!(cache.gpu.lock().unwrap_or_else(|e| e.into_inner()).early.is_some(), "Cache bleibt");
        // A part too large for the card goes to the CPU, without emptying the cache.
        let small = Region { x: 70, y: 40, w: 20, h: 20 };
        let ext = p.plan(&src, Some(small)).ext;
        g.max_binding = (ext.w * ext.h * 16) as u64;
        if let Err(e) = p.render_gpu(&g, &src, Some(small), Some(&cache)) {
            panic!("{ext:?}: {e:#}");
        }
        let Err(e) = p.render_gpu(&g, &src, None, Some(&cache)) else { panic!("zu groß") };
        assert!(e.downcast_ref::<Fallback>().is_some(), "{e:#}");
        assert!(cache.gpu.lock().unwrap_or_else(|e| e.into_inner()).early.is_some(), "Cache bleibt");
        assert!(g.problem().is_none());
    }

    fn painted_scene() -> Adjustments {
        let mut adj = Adjustments {
            shadows: 40.0,
            detail: Detail { clarity: 40.0, structure: 30.0, dehaze: 30.0, sharpen_amount: 120.0, sharpen_radius: 1.0, nr_luma: 20.0, ..Default::default() },
            ..Default::default()
        };
        let stroke = BrushStroke { points: vec![[0.3, 0.4], [0.6, 0.5]], radius: 0.08, hardness: 0.3, flow: 1.0, erase: false, opacity: 1.0, auto_mask: false, build_up: true, tolerance: 50.0 };
        let mut layer = Layer { adjustments: LayerAdjustments { exposure: 1.0, ..Default::default() }, ..Default::default() };
        layer.mask.strokes.push(stroke);
        adj.layers.push(layer);
        adj.assign_layer_ids();
        adj
    }

    /// With the whole image in the cache, a part of it is cut out of the cached buffers on
    /// the card as well: exactly the whole rendering there.
    #[test]
    fn part_with_cached_whole_matches_the_whole_rendering_on_the_card() {
        let Some(g) = gpu() else { return };
        let (w, h) = (200, 150);
        let src = image(w, h);
        let p = Processor::new(&meta(w, h), &painted_scene());
        let cache = StageCache::default();
        let full = p.render_gpu(g, &src, None, Some(&cache)).unwrap();
        let whole_key = cache.gpu.lock().unwrap_or_else(|e| e.into_inner()).early.as_ref().unwrap().0;
        for region in [Region { x: 50, y: 40, w: 40, h: 30 }, Region { x: 0, y: 0, w: 25, h: 20 }, Region { x: 170, y: 120, w: 30, h: 30 }] {
            let part = p.render_gpu(g, &src, Some(region), Some(&cache)).unwrap();
            let mut max = 0;
            for y in 0..region.h {
                for x in 0..region.w {
                    for c in 0..3 {
                        let a = part.rgba[(y * region.w + x) * 4 + c] as i32;
                        let b = full.rgba[((y + region.y) * w + x + region.x) * 4 + c] as i32;
                        max = max.max((a - b).abs());
                    }
                }
            }
            assert!(max <= 1, "{region:?}: {max}");
        }
        assert_eq!(cache.gpu.lock().unwrap_or_else(|e| e.into_inner()).early.as_ref().unwrap().0, whole_key, "the whole stays cached");
    }

    /// While a stroke is painted only its changed rows go to the card: the result is that of
    /// the whole mask.
    #[test]
    fn growing_stroke_updates_the_mask_on_the_card() {
        let Some(g) = gpu() else { return };
        let (w, h) = (200, 150);
        let src = image(w, h);
        let mut adj = painted_scene();
        adj.detail = Detail::default();
        let cache = StageCache::default();
        for i in 0..8 {
            adj.layers[0].mask.strokes[0].points.push([0.6 - i as f32 * 0.04, 0.5 + i as f32 * 0.03]);
            let bm = crate::mask::cached_bitmap(&adj.layers[0].mask, w as f32 / h as f32, Some(&src));
            assert!(i == 0 || bm.changed.is_some(), "Punkt {i}: nur die geänderten Zeilen");
            let p = Processor::new(&meta(w, h), &adj);
            let on_card = p.render_gpu(g, &src, None, Some(&cache)).unwrap();
            let cpu = p.render_cpu(&src, None, None);
            let max = on_card.rgba.iter().zip(&cpu.rgba).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap_or(0);
            assert!(max <= 4, "Punkt {i}: {max}");
        }
    }

    /// Heal/clone stays cached when only the white balance changes, and the result is that
    /// of a rendering without cache.
    #[test]
    fn retouch_stays_cached_across_white_balance() {
        let Some(g) = gpu() else { return };
        let (w, h) = (200, 150);
        let src = image(w, h);
        let mut adj = Adjustments {
            retouch: vec![Spot { mode: SpotMode::Clone, target: [0.3, 0.3], source: [0.6, 0.6], path: vec![], strokes: vec![], radius: 0.05, feather: 0.5, opacity: 1.0 }],
            ..Default::default()
        };
        let cache = StageCache::default();
        Processor::new(&meta(w, h), &adj).render_gpu(g, &src, None, Some(&cache)).unwrap();
        let source_key = cache.gpu.lock().unwrap_or_else(|e| e.into_inner()).source.as_ref().unwrap().0;
        adj.white_balance.mode = crate::params::WbMode::Custom;
        adj.white_balance.kelvin = 3500.0;
        let p = Processor::new(&meta(w, h), &adj);
        let cached = p.render_gpu(g, &src, None, Some(&cache)).unwrap();
        assert_eq!(cache.gpu.lock().unwrap_or_else(|e| e.into_inner()).source.as_ref().unwrap().0, source_key, "retouch not done again");
        let fresh = p.render_gpu(g, &src, None, None).unwrap();
        assert_eq!(cached.rgba, fresh.rgba);
    }

    #[test]
    fn median_matches_cpu() {
        let Some(g) = gpu() else { return };
        let (w, h) = (97, 61);
        let p = Plane::new(w, h, (0..w * h).map(|i| hash_noise(i as i64, 2, 7) * 3.0 - 1.0).collect());
        for r in [1, 3, 24] {
            let mut ops = Ops::new(g);
            let buf = ops.g.upload(&p.data);
            let out = ops.median(&buf, w, h, r);
            ops.flush();
            let got = g.read::<f32>(&out, w * h).unwrap();
            let want = crate::detail::median_hv(&p, r);
            assert_eq!(got, want.data, "r {r}");
        }
    }

    #[test]
    fn reused_buffers_give_the_same_result() {
        let Some(g) = Gpu::new_for_test() else { return };
        let (w, h) = (420, 300);
        let src = image(w, h);
        let adj = Adjustments { highlights: -60.0, shadows: 40.0, detail: Detail { clarity: 30.0, ..Default::default() }, ..Default::default() };
        let p = Processor::new(&meta(w, h), &adj);
        let first = p.render_gpu(&g, &src, None, None).unwrap();
        assert!(g.pooled_bytes() > 0, "nichts im Pool");
        // Reused buffers hold old data until cleared: the result must not change.
        let second = p.render_gpu(&g, &src, None, None).unwrap();
        assert!(first.rgba == second.rgba);
        let other = Processor::new(&meta(w, h), &Adjustments { exposure: 0.5, ..adj.clone() }).render_gpu(&g, &src, None, None).unwrap();
        let cpu = Processor::new(&meta(w, h), &Adjustments { exposure: 0.5, ..adj }).render_cpu(&src, None, None);
        let max = other.rgba.iter().zip(&cpu.rgba).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap_or(0);
        assert!(max <= 4, "max {max}");
    }

    #[test]
    fn black_results_are_redone_without_blaming_the_card() {
        let Some(g) = Gpu::new_for_test() else { return };
        let (w, h) = (64, 48);
        let black = LinearImage::new(w, h, vec![[0.0; 3]; w * h]);
        let p = Processor::new(&meta(w, h), &Adjustments::default());
        assert!(p.render_gpu(&g, &black, None, None).is_err());
        assert!(g.problem().is_none());
    }

    fn meta(w: usize, h: usize) -> RawMeta {
        let mut color = CameraColor::new(mat_inv(&REC2020_TO_XYZ).unwrap());
        color.cam_to_working = IDENTITY;
        RawMeta {
            path: Default::default(),
            info: CaptureInfo { width: w * 4, height: h * 4, ..Default::default() },
            color,
            as_shot_wb: [1.6, 1.0, 1.3],
            orientation: Orientation::Normal,
            hdr: false, lens: None, lens_corrected: false, bayer: false,
        }
    }

    /// Colorful content with edges, smooth gradients, noise and clipped highlights (all channels
    /// in one corner, only blue in another).
    fn image(w: usize, h: usize) -> LinearImage {
        LinearImage::new(
            w,
            h,
            (0..w * h)
                .map(|i| {
                    let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
                    let n = hash_noise(i as i64, 3, 9) * 0.01;
                    let block = (((i % w) / 23 + (i / w) / 17) % 3) as f32 * 0.15;
                    let base = [x * 0.6 + block, (1.0 - y) * 0.4 + 0.02, (x * 7.0 + y * 3.0).sin().abs() * 0.5];
                    let hi = if x > 0.85 && y < 0.2 { 1.0 } else { 0.0 };
                    let mut p = base.map(|v| (v + n).max(0.0).max(hi));
                    if x < 0.15 && y > 0.8 {
                        p[2] = 1.0;
                    }
                    p
                })
                .collect(),
        )
    }

    /// `None` if the renderings match, else what differs.
    fn compare(name: &str, g: &Gpu, adj: &Adjustments, opts: RenderOptions, region: Option<Region>) -> Option<String> {
        let (w, h) = (300, 200);
        let src = image(w, h);
        let p = Processor::with_options(&meta(w, h), adj, opts);
        let cpu = p.render_cpu(&src, region, None);
        let gpu = p.render_gpu(g, &src, region, None).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        assert_eq!((gpu.width, gpu.height, gpu.full_size), (cpu.width, cpu.height, cpu.full_size), "{name}");
        let diffs: Vec<i32> = cpu.rgba.iter().zip(&gpu.rgba).map(|(a, b)| (*a as i32 - *b as i32).abs()).collect();
        let max = diffs.iter().copied().max().unwrap_or(0);
        let mean = diffs.iter().map(|d| *d as f64).sum::<f64>() / diffs.len() as f64;
        let off = diffs.iter().filter(|d| **d > 1).count() as f64 / diffs.len() as f64;
        eprintln!("{name}: max {max}, mittel {mean:.4}, >1: {:.3} %", off * 100.0);
        assert_eq!(gpu.histogram.luma.iter().sum::<u32>(), (gpu.width * gpu.height) as u32, "{name}");
        (max > 4 || mean >= 0.05 || off >= 0.002).then(|| format!("{name}: max {max}, mittel {mean}, >1: {off}"))
    }

    fn layer(adjustments: LayerAdjustments, mask: Mask) -> Layer {
        Layer { adjustments, mask, opacity: 80.0, ..Default::default() }
    }

    #[test]
    fn rendering_matches_cpu() {
        let Some(g) = gpu() else { return };
        let plain = Adjustments::default();
        let radial = Mask {
            radial: vec![RadialGradient { center: [0.4, 0.5], radius_x: 0.3, radius_y: 0.25, feather: 0.6, invert: false, angle: 0.0 }],
            ..Default::default()
        };
        let cases: Vec<(&str, Adjustments, RenderOptions, Option<Region>)> = vec![
            ("Standard", plain.clone(), Default::default(), None),
            (
                "Ton",
                Adjustments { exposure: 0.7, contrast: 35.0, highlights: 60.0, shadows: 50.0, whites: -20.0, blacks: 15.0, base_curve: BaseCurve::FilmHigh, ..plain.clone() },
                Default::default(),
                None,
            ),
            (
                "Linear, Kurven",
                Adjustments {
                    base_curve: BaseCurve::Linear,
                    contrast: -30.0,
                    brightness: 20.0,
                    levels: Levels {
                        rgb: LevelSet { in_black: 0.05, in_white: 0.9, midtone: 1.2, out_black: 0.02, out_white: 0.97 },
                        red: LevelSet { in_white: 0.95, midtone: 1.1, ..Default::default() },
                        ..Default::default()
                    },
                    ..plain.clone()
                },
                Default::default(),
                None,
            ),
            (
                // The camera's curve, also where a layer changes the contrast (no table there).
                "Kamera-Kurve",
                Adjustments {
                    base_curve: BaseCurve::Camera,
                    camera_curve: Some(crate::params::CameraCurve { points: (0..14).map(|i| [-5.0 + i as f32 * 0.5, 0.01 * 1.42f32.powi(i)]).collect() }),
                    contrast: 20.0,
                    layers: vec![layer(LayerAdjustments { contrast: -40.0, exposure: 0.5, ..Default::default() }, radial.clone())],
                    ..plain.clone()
                },
                Default::default(),
                None,
            ),
            (
                "Leuchten",
                Adjustments { detail: Detail { glow_amount: 70.0, glow_size: 60.0, ..Default::default() }, ..plain.clone() },
                Default::default(),
                None,
            ),
            (
                "Leuchten, Ausschnitt",
                Adjustments { detail: Detail { glow_amount: 70.0, ..Default::default() }, ..plain.clone() },
                Default::default(),
                Some(Region { x: 40, y: 30, w: 200, h: 120 }),
            ),
            (
                "Details",
                Adjustments {
                    detail: Detail { clarity: 45.0, structure: 30.0, dehaze: 40.0, nr_luma: 40.0, nr_color: 50.0, sharpen_amount: 120.0, sharpen_threshold: 2.0, ..Default::default() },
                    ..plain.clone()
                },
                Default::default(),
                None,
            ),
            (
                "Dunst hinzu, Körnung, Vignette",
                Adjustments { detail: Detail { dehaze: -50.0, grain_amount: 40.0, grain_size: 30.0, vignette_amount: -40.0, ..Default::default() }, ..plain.clone() },
                Default::default(),
                None,
            ),
            (
                "Kurven und Luma-Kurve",
                {
                    let mut a = Adjustments { brightness: 15.0, ..plain.clone() };
                    a.levels.rgb.in_black = 0.03;
                    a.curves.rgb = Curve { points: vec![[0.0, 0.0], [0.3, 0.25], [1.0, 1.0]] };
                    a.curves.red = Curve { points: vec![[0.0, 0.02], [0.6, 0.65], [1.0, 1.0]] };
                    a.curves.luma = Curve { points: vec![[0.0, 0.05], [0.25, 0.18], [0.75, 0.88], [1.0, 1.0]] };
                    a
                },
                Default::default(),
                None,
            ),
            (
                "Farbe",
                {
                    let mut a = Adjustments { saturation: 25.0, vibrance: 30.0, ..plain.clone() };
                    a.color_editor.bands[1] = HslBand { hue: 10.0, saturation: 30.0, lightness: -20.0 };
                    a.color_editor.bands[5] = HslBand { hue: -15.0, saturation: -40.0, lightness: 10.0 };
                    a.color_balance.shadows = ColorWheel { hue: 200.0, amount: 40.0, lightness: 10.0 };
                    a.color_balance.highlights = ColorWheel { hue: 50.0, amount: 30.0, lightness: -5.0 };
                    a.skin_tone.saturation = 30.0;
                    a.skin_tone.hue_uniformity = 50.0;
                    let zone = |hue, width, softness, adjust| crate::params::ColorZone { hue, width, softness, adjust, enabled: true };
                    a.color_editor.zones = vec![
                        zone(60.0, 20.0, 60.0, HslBand { hue: -12.0, saturation: 40.0, lightness: 15.0 }),
                        zone(350.0, 35.0, 0.0, HslBand { hue: 8.0, saturation: -50.0, lightness: -10.0 }),
                        crate::params::ColorZone { enabled: false, ..zone(200.0, 30.0, 50.0, HslBand { saturation: -100.0, ..Default::default() }) },
                    ];
                    a
                },
                Default::default(),
                None,
            ),
            (
                "Schwarzweiß",
                {
                    let mut a = plain.clone();
                    a.black_white.enabled = true;
                    a.black_white.mix = [20.0, -10.0, 30.0, 0.0, -40.0, 10.0, 0.0, 5.0];
                    a.black_white.shadow_tint = ColorWheel { hue: 220.0, amount: 30.0, lightness: 0.0 };
                    a
                },
                Default::default(),
                None,
            ),
            (
                "Geometrie",
                Adjustments {
                    rotation: 1,
                    geometry: Geometry {
                        straighten: 4.0,
                        keystone_v: 20.0,
                        flip_h: true,
                        crop: CropRect { x: 0.1, y: 0.05, w: 0.8, h: 0.85 },
                        ..Default::default()
                    },
                    lens: LensCorrection { distortion: 30.0, fisheye: 150.0, ca_red: 40.0, ca_blue: -30.0, defringe_purple: 80.0, defringe_green: 60.0, vignetting: 40.0, ..Default::default() },
                    ..plain.clone()
                },
                Default::default(),
                None,
            ),
            (
                "Ebenen",
                Adjustments {
                    layers: vec![
                        layer(LayerAdjustments { exposure: 0.8, saturation: -30.0, temperature: 40.0, clarity: 30.0, ..Default::default() }, radial.clone()),
                        layer(
                            LayerAdjustments { brightness: 30.0, shadows: 40.0, dehaze: 30.0, ..Default::default() },
                            Mask { fill: true, luma_range: Some(LumaRange { falloff_low: 0.2, low: 0.3, high: 0.8, falloff_high: 0.9 }), ..Default::default() },
                        ),
                        layer(
                            LayerAdjustments { contrast: 40.0, tint: -30.0, ..Default::default() },
                            Mask { fill: true, color_range: Some(ColorRange::default()), invert: true, ..Default::default() },
                        ),
                    ],
                    ..plain.clone()
                },
                Default::default(),
                None,
            ),
            (
                // Curves of a layer on the result: alone, and together with deltas.
                "Ebenen mit Kurven",
                {
                    let mut a = plain.clone();
                    a.curves.rgb = Curve { points: vec![[0.0, 0.0], [0.5, 0.55], [1.0, 1.0]] };
                    let mut only = LayerAdjustments::default();
                    only.curves.rgb = Curve { points: vec![[0.0, 0.0], [0.3, 0.2], [1.0, 1.0]] };
                    only.curves.blue = Curve { points: vec![[0.0, 0.05], [0.5, 0.6], [1.0, 1.0]] };
                    let mut both = LayerAdjustments { exposure: 0.4, ..Default::default() };
                    both.curves.luma = Curve { points: vec![[0.0, 0.0], [0.4, 0.55], [1.0, 1.0]] };
                    a.layers = vec![
                        layer(only, radial.clone()),
                        layer(both, Mask { fill: true, luma_range: Some(LumaRange { falloff_low: 0.1, low: 0.2, high: 0.7, falloff_high: 0.8 }), ..Default::default() }),
                    ];
                    a
                },
                Default::default(),
                None,
            ),
            (
                // Summed contrast far below -100: the warp's exponent is floored.
                "Ebenen, Kontrast summiert",
                Adjustments {
                    contrast: -100.0,
                    layers: vec![
                        layer(LayerAdjustments { contrast: -100.0, ..Default::default() }, Mask { fill: true, ..Default::default() }),
                        layer(LayerAdjustments { contrast: -100.0, ..Default::default() }, radial.clone()),
                    ],
                    ..plain.clone()
                },
                Default::default(),
                None,
            ),
            (
                "Ebenen mit Weiß, Schwarz und Farbe",
                Adjustments {
                    vibrance: 10.0,
                    color_balance: ColorBalance { midtones: ColorWheel { hue: 40.0, amount: 20.0, lightness: 0.0 }, ..Default::default() },
                    layers: vec![
                        layer(
                            LayerAdjustments {
                                whites: 30.0,
                                blacks: -20.0,
                                vibrance: 40.0,
                                color_bands: std::array::from_fn(|i| HslBand { hue: i as f32 * 2.0, saturation: -20.0, lightness: 10.0 }),
                                color_balance: ColorBalance {
                                    master: ColorWheel { hue: 220.0, amount: 25.0, lightness: 5.0 },
                                    highlights: ColorWheel { hue: 60.0, amount: 30.0, lightness: -10.0 },
                                    ..Default::default()
                                },
                                ..Default::default()
                            },
                            radial.clone(),
                        ),
                        layer(
                            LayerAdjustments { color_balance: ColorBalance { shadows: ColorWheel { hue: 300.0, amount: 40.0, lightness: 0.0 }, ..Default::default() }, ..Default::default() },
                            Mask { fill: true, ..Default::default() },
                        ),
                    ],
                    ..plain.clone()
                },
                Default::default(),
                None,
            ),
            (
                "Maskenanzeige, Clipping",
                Adjustments { exposure: 1.0, layers: vec![layer(LayerAdjustments::default(), radial.clone())], ..plain.clone() },
                RenderOptions { clip_warning: true, mask_overlay: Some(0), ..Default::default() },
                None,
            ),
            (
                "Ausschnitt",
                Adjustments { shadows: 40.0, detail: Detail { clarity: 40.0, ..Default::default() }, ..plain.clone() },
                Default::default(),
                Some(Region { x: 70, y: 40, w: 90, h: 60 }),
            ),
            (
                "Klonen",
                Adjustments {
                    retouch: vec![Spot { mode: SpotMode::Clone, target: [0.3, 0.3], source: [0.6, 0.6], path: vec![], strokes: vec![], radius: 0.05, feather: 0.5, opacity: 1.0 }],
                    ..plain.clone()
                },
                Default::default(),
                None,
            ),
        ];
        // All cases, so that one difference doesn't hide the others.
        let failed: Vec<String> = cases.into_iter().filter_map(|(name, adj, opts, region)| compare(name, g, &adj, opts, region)).collect();
        assert!(failed.is_empty(), "{}", failed.join("\n"));
    }

    #[test]
    fn atmosphere_matches_cpu() {
        let Some(g) = gpu() else { return };
        let (w, h) = (300, 200);
        let src = image(w, h);
        // Far at the top, near at the bottom.
        let map = Arc::new(crate::depth::DepthMap { width: 60, height: 40, data: (0..60 * 40).map(|i| (i / 60) as f32 / 39.0).collect() });
        let adj = Adjustments { atmosphere: crate::params::Atmosphere { amount: 70.0, reach: 0.7, brightness: 20.0, warmth: 30.0 }, ..Default::default() };
        let mut p = Processor::new(&meta(w, h), &adj);
        p.layers.splice(0..0, super::depth_layers_with(&map, &adj));
        p.haze_map = Some(map);
        let cpu = p.render_cpu(&src, None, None);
        let gpu = p.render_gpu(g, &src, None, None).unwrap();
        let max = cpu.rgba.iter().zip(&gpu.rgba).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap_or(0);
        assert!(max <= 4, "größte Abweichung {max}");
    }

    #[test]
    fn tiled_export_matches_cpu() {
        let Some(g) = gpu() else { return };
        // Large enough for several tiles.
        let (w, h) = (4600, 3100);
        let src = image(w, h);
        let adj = Adjustments {
            shadows: 40.0,
            detail: Detail { clarity: 40.0, dehaze: 30.0, grain_amount: 20.0, vignette_amount: -30.0, glow_amount: 50.0, ..Default::default() },
            ..Default::default()
        };
        let p = Processor::new(&meta(w, h), &adj);
        let cpu = p.process_cpu(&src, None, OutputSpace::AdobeRgb);
        let gpu = match p.process_gpu(g, &src, None, OutputSpace::AdobeRgb) {
            Ok(gpu) => gpu,
            // Beyond the memory budget the system grants (other programs use the card too).
            Err(e) if format!("{e:#}").contains("Grafikspeicher voll") => return eprintln!("{e:#}, Test übersprungen"),
            Err(e) => panic!("{e:#}"),
        };
        assert_eq!((gpu.width, gpu.height), (cpu.width, cpu.height));
        let mut max = 0.0f32;
        let mut sum = 0.0f64;
        for (a, b) in cpu.data.iter().zip(&gpu.data) {
            for c in 0..3 {
                let d = (a[c] - b[c]).abs();
                max = max.max(d);
                sum += d as f64;
            }
        }
        let mean = sum / (cpu.data.len() * 3) as f64;
        eprintln!("Export in Kacheln: max {max}, mittel {mean:.6}");
        assert!(max < 0.02 && mean < 1e-4, "max {max}, mittel {mean}");
    }
}
