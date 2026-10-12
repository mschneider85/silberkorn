// Display conversion (8-bit sRGB with clip warning and mask overlay) and histogram.

struct Args {
    // Displayed region and its offset in the processed (extended) region.
    w: u32,
    h: u32,
    ext_w: u32,
    off_x: u32,
    off_y: u32,
    clip: u32,
    mask: u32,
    to_srgb: array<f32, 9>,
}

@group(0) @binding(0) var<storage, read> a: Args;
@group(0) @binding(1) var<storage, read> img: array<vec4<f32>>;
// sRGB-encoded -> linear and linear -> 8-bit sRGB, indexed like display_index.
@group(0) @binding(2) var<storage, read> decode: array<f32>;
@group(0) @binding(3) var<storage, read> encode8: array<u32>;
@group(0) @binding(4) var<storage, read_write> rgba: array<u32>;
// r, g, b, luma: 256 bins each.
@group(0) @binding(5) var<storage, read_write> hist: array<atomic<u32>>;

var<workgroup> local_hist: array<atomic<u32>, 1024>;

const LUT_MAX: f32 = 16383.0;

fn index(v: f32) -> u32 {
    return u32(clamp(v, 0.0, 1.0) * LUT_MAX + 0.5);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    for (var k = 0u; k < 4u; k++) {
        atomicStore(&local_hist[li * 4u + k], 0u);
    }
    workgroupBarrier();
    if gid.x < a.w && gid.y < a.h {
        let s = img[(gid.y + a.off_y) * a.ext_w + gid.x + a.off_x];
        let lin = mat_vec(a.to_srgb, vec3<f32>(decode[index(s.x)], decode[index(s.y)], decode[index(s.z)]));
        var px = vec3<u32>(encode8[index(lin.x)], encode8[index(lin.y)], encode8[index(lin.z)]);
        if a.mask != 0u {
            let al = clamp(s.w * 0.6, 0.0, 0.6);
            let f = vec3<f32>(f32(px.x) * (1.0 - al) + 255.0 * al, f32(px.y) * (1.0 - al), f32(px.z) * (1.0 - al));
            px = vec3<u32>(clamp(f + 1e-3, vec3<f32>(0.0), vec3<f32>(255.0)));
        }
        var o = px.x | (px.y << 8u) | (px.z << 16u) | 0xff000000u;
        if a.clip != 0u {
            if px.x == 255u || px.y == 255u || px.z == 255u {
                o = 0xff0000ffu;
            } else if px.x == 0u && px.y == 0u && px.z == 0u {
                o = 0xffff5000u;
            }
        }
        rgba[gid.y * a.w + gid.x] = o;
        // The histogram is taken from the displayed pixels (including clip colors).
        let r = o & 0xffu;
        let g = (o >> 8u) & 0xffu;
        let b = (o >> 16u) & 0xffu;
        atomicAdd(&local_hist[r], 1u);
        atomicAdd(&local_hist[256u + g], 1u);
        atomicAdd(&local_hist[512u + b], 1u);
        atomicAdd(&local_hist[768u + min((r * 54u + g * 183u + b * 19u) >> 8u, 255u)], 1u);
    }
    workgroupBarrier();
    for (var k = 0u; k < 4u; k++) {
        let v = atomicLoad(&local_hist[li * 4u + k]);
        if v != 0u {
            atomicAdd(&hist[li * 4u + k], v);
        }
    }
}
