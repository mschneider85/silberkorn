// Crops the processed region and converts it into an output color space (Processor::process),
// as 16 bits per channel (as fine as the finest export, half the bytes to read back of floats).

struct Args {
    w: u32,
    h: u32,
    ext_w: u32,
    off_x: u32,
    off_y: u32,
    // 0: sRGB curve, otherwise a gamma.
    gamma: f32,
    // Gamut mapping instead of clipping (process version 2).
    gamut: u32,
    m: array<f32, 9>,
}

@group(0) @binding(0) var<storage, read> a: Args;
@group(0) @binding(1) var<storage, read> img: array<vec4<f32>>;
// Red and green, blue and the marker (checked on the CPU): 0xFFFF written, 0x8000 written but
// not finite.
@group(0) @binding(2) var<storage, read_write> out: array<vec2<u32>>;

fn trc_encode(v: f32) -> f32 {
    if a.gamma == 0.0 {
        return srgb_encode(v);
    }
    return pow(max(v, 0.0), 1.0 / a.gamma);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= a.w || gid.y >= a.h {
        return;
    }
    let s = img[(gid.y + a.off_y) * a.ext_w + gid.x + a.off_x];
    let lin = vec3<f32>(srgb_decode(s.x), srgb_decode(s.y), srgb_decode(s.z));
    var c = mat_vec(a.m, lin);
    if a.gamut != 0u {
        c = gamut_map(c, dot(lin, LUMA));
    }
    // Comparisons with NaN are false.
    let finite = all(abs(c) < vec3<f32>(3.0e38));
    c = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    let e = vec3<f32>(trc_encode(c.x), trc_encode(c.y), trc_encode(c.z));
    let marker = select(0x8000u, 0xFFFFu, finite);
    out[gid.y * a.w + gid.x] = vec2<u32>(pack2x16unorm(e.xy), pack2x16unorm(vec2<f32>(e.z, 0.0)) | (marker << 16u));
}
