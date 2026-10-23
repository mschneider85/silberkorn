//! The AHD demosaic (see `crate::ahd`) on the graphics card: about a second of the CPU's per 24
//! megapixels is the largest part of loading a raw file. The same steps on the whole mosaic in
//! three kernels; only the greens of both directions and the homogeneity are kept between them
//! (the colors are recomputed from the greens where needed), about 40 bytes per pixel.

use anyhow::{Result, bail};
use bytemuck::{Pod, Zeroable};

use super::Ops;
use crate::ahd::Mosaic;
use crate::gpu::{Gpu, Kernel};

macro_rules! ahd_kernel {
    ($name:literal $(, $part:literal)*) => {
        (
            $name,
            concat!(include_str!("../../gpu/shaders/ahd_common.wgsl"), $(include_str!(concat!("../../gpu/shaders/", $part, ".wgsl")),)* include_str!(concat!("../../gpu/shaders/", $name, ".wgsl"))),
        )
    };
}

const GREEN: Kernel = ahd_kernel!("ahd_green");
const HOMO: Kernel = ahd_kernel!("ahd_homo", "ahd_rgb");
const OUT: Kernel = ahd_kernel!("ahd_out", "ahd_rgb");
pub(super) const KERNELS: [Kernel; 3] = [GREEN, HOMO, OUT];

/// Video memory per pixel: the mosaic, the greens, the homogeneity, the result and its copy
/// for reading back.
const BYTES_PER_PIXEL: u64 = 4 + 8 + 4 + 12 + 12;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Args {
    w: u32,
    h: u32,
    pattern: u32,
    wb: [f32; 3],
}

/// [`crate::ahd::demosaic`] on `g` (the same result, but for rounding). Fails where the free
/// video memory is too small (the CPU does it then).
pub(crate) fn demosaic(g: &Gpu, m: &Mosaic, wb: [f32; 3]) -> Result<Vec<[f32; 3]>> {
    let (w, h) = (m.width, m.height);
    let n = w * h;
    if let Some(free) = g.available_bytes()
        && free < n as u64 * BYTES_PER_PIXEL / 10 * 12
    {
        bail!("zu wenig Grafikspeicher frei ({} MB)", free >> 20);
    }
    if n as u64 * 12 > g.max_binding {
        bail!("Bild zu groß für die Grafikkarte");
    }
    g.checked(|| {
        let mut ops = Ops::new(g);
        let pattern = (0..4).fold(0u32, |acc, k| acc | (m.pattern[k / 2][k % 2] as u32) << (2 * k));
        let args = ops.args(&Args { w: w as u32, h: h as u32, pattern, wb });
        let raw = g.upload(&m.data[..n]);
        let green = ops.alloc(n as u64 * 8);
        let homo = ops.plane(n);
        let out = ops.alloc(n as u64 * 12);
        ops.enc.run_2d(GREEN, &[&args, &raw, &green], w, h);
        ops.enc.run_2d(HOMO, &[&args, &raw, &green, &homo], w, h);
        ops.enc.run_2d(OUT, &[&args, &raw, &green, &homo, &out], w, h);
        ops.flush();
        g.read::<[f32; 3]>(&out, n)
    })
}
