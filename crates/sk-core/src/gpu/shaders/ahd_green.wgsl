// AHD step 1: green along the row and along the column, with a second-order correction from
// the pixel's own color, limited to the range of its green neighbors.

@group(0) @binding(2) var<storage, read_write> green: array<vec2<f32>>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= a.w || gid.y >= a.h {
        return;
    }
    let x = i32(gid.x);
    let y = i32(gid.y);
    let c = rawv(x, y);
    if cfa(x, y) == 1u {
        green[gid.y * a.w + gid.x] = vec2<f32>(c, c);
        return;
    }
    let l = rawv(x - 1, y);
    let r = rawv(x + 1, y);
    let gh = clamp((l + r) / 2.0 + (2.0 * c - rawv(x - 2, y) - rawv(x + 2, y)) / 4.0, min(l, r), max(l, r));
    let u = rawv(x, y - 1);
    let d = rawv(x, y + 1);
    let gv = clamp((u + d) / 2.0 + (2.0 * c - rawv(x, y - 2) - rawv(x, y + 2)) / 4.0, min(u, d), max(u, d));
    green[gid.y * a.w + gid.x] = vec2<f32>(gh, gv);
}
