// AHD step 5: per pixel the direction more homogeneous over 3x3, the mean where both are equal;
// back to the mosaic's own (unbalanced) scale.

@group(0) @binding(2) var<storage, read> green: array<vec2<f32>>;
@group(0) @binding(3) var<storage, read> homo: array<u32>;
@group(0) @binding(4) var<storage, read_write> rgb: array<f32>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= a.w || gid.y >= a.h {
        return;
    }
    let x = i32(gid.x);
    let y = i32(gid.y);
    var sh = 0u;
    var sv = 0u;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let h = homo[at(x + dx, y + dy)];
            sh += h & 0xffu;
            sv += h >> 8u;
        }
    }
    var p: vec3<f32>;
    if sh > sv {
        p = rgb_dir(x, y, 0u);
    } else if sh < sv {
        p = rgb_dir(x, y, 1u);
    } else {
        p = (rgb_dir(x, y, 0u) + rgb_dir(x, y, 1u)) / 2.0;
    }
    let i = (gid.y * a.w + gid.x) * 3u;
    rgb[i] = p.x / a.wb0;
    rgb[i + 1u] = p.y / a.wb1;
    rgb[i + 2u] = p.z / a.wb2;
}
