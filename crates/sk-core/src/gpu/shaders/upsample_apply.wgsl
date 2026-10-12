// Bilinearly upsampled guided filter coefficients applied to the full-resolution guide.

struct Args {
    w: u32,
    h: u32,
    lw: u32,
    lh: u32,
    s: u32,
}

@group(0) @binding(0) var<storage, read> a: Args;
@group(0) @binding(1) var<storage, read> ma: array<f32>;
@group(0) @binding(2) var<storage, read> mb: array<f32>;
@group(0) @binding(3) var<storage, read> guide: array<f32>;
@group(0) @binding(4) var<storage, read_write> out: array<f32>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= a.w || gid.y >= a.h {
        return;
    }
    let s = f32(a.s);
    let fy = clamp((f32(gid.y) + 0.5) / s - 0.5, 0.0, f32(a.lh - 1u));
    let fx = clamp((f32(gid.x) + 0.5) / s - 0.5, 0.0, f32(a.lw - 1u));
    let y0 = u32(floor(fy));
    let x0 = u32(floor(fx));
    let ty = fract(fy);
    let tx = fract(fx);
    let y1 = min(y0 + 1u, a.lh - 1u);
    let x1 = min(x0 + 1u, a.lw - 1u);
    let i00 = y0 * a.lw + x0;
    let i10 = y0 * a.lw + x1;
    let i01 = y1 * a.lw + x0;
    let i11 = y1 * a.lw + x1;
    let ca = (ma[i00] * (1.0 - tx) + ma[i10] * tx) * (1.0 - ty) + (ma[i01] * (1.0 - tx) + ma[i11] * tx) * ty;
    let cb = (mb[i00] * (1.0 - tx) + mb[i10] * tx) * (1.0 - ty) + (mb[i01] * (1.0 - tx) + mb[i11] * tx) * ty;
    let i = gid.y * a.w + gid.x;
    out[i] = ca * guide[i] + cb;
}
