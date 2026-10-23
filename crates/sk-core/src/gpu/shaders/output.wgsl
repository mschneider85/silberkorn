// Crops the processed region, unchanged (encoded working RGB and mask weight).

struct Args {
    w: u32,
    h: u32,
    ext_w: u32,
    off_x: u32,
    off_y: u32,
}

@group(0) @binding(0) var<storage, read> a: Args;
@group(0) @binding(1) var<storage, read> img: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> out: array<vec4<f32>>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= a.w || gid.y >= a.h {
        return;
    }
    out[gid.y * a.w + gid.x] = img[(gid.y + a.off_y) * a.ext_w + gid.x + a.off_x];
}
