// Crops the processed region and converts it: unchanged (encoded working RGB and mask
// weight), or into an output color space (Processor::process).

struct Args {
    w: u32,
    h: u32,
    ext_w: u32,
    off_x: u32,
    off_y: u32,
    convert: u32,
    // 0: sRGB curve, otherwise a gamma.
    gamma: f32,
    // Gamut mapping instead of clipping (process version 2).
    gamut: u32,
    m: array<f32, 9>,
}

@group(0) @binding(0) var<storage, read> a: Args;
@group(0) @binding(1) var<storage, read> img: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> out: array<vec4<f32>>;

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
    var o = s;
    if a.convert != 0u {
        let lin = vec3<f32>(srgb_decode(s.x), srgb_decode(s.y), srgb_decode(s.z));
        var c = mat_vec(a.m, lin);
        if a.gamut != 0u {
            c = gamut_map(c, dot(lin, LUMA));
        }
        c = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
        o = vec4<f32>(trc_encode(c.x), trc_encode(c.y), trc_encode(c.z), 0.0);
    }
    out[gid.y * a.w + gid.x] = o;
}
