// Planes derived from an RGB image.

struct Args {
    n: u32,
    op: u32,
}

@group(0) @binding(0) var<storage, read> a: Args;
@group(0) @binding(1) var<storage, read> img: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> o0: array<f32>;
@group(0) @binding(3) var<storage, read_write> o1: array<f32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = index_1d(gid);
    if i >= a.n {
        return;
    }
    let p = img[i].xyz;
    let l = dot(p, LUMA);
    switch a.op {
        // Perceptual luminance (noise reduction).
        case 0u: { o0[i] = sqrt(max(l, 0.0)); }
        // Color differences (noise reduction).
        case 1u: {
            o0[i] = p.x - l;
            o1[i] = p.z - l;
        }
        // log2 luminance relative to mid grey (local operators).
        case 2u: { o0[i] = log2(max(l, 1e-6)) - log2(MID_GREY); }
        // One channel (glow).
        case 5u: { o0[i] = p.x; }
        case 6u: { o0[i] = p.y; }
        case 7u: { o0[i] = p.z; }
        // Luminance (sharpening).
        default: { o0[i] = l; }
    }
}
