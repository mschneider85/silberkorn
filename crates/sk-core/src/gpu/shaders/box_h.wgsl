// Horizontal running mean with clamped edges, one invocation per row (detail::box_line).

struct Args {
    w: u32,
    h: u32,
    r: u32,
}

@group(0) @binding(0) var<storage, read> a: Args;
@group(0) @binding(1) var<storage, read> src: array<f32>;
@group(0) @binding(2) var<storage, read_write> dst: array<f32>;

fn at(row: u32, i: i32) -> f32 {
    return src[row + u32(clamp(i, 0, i32(a.w) - 1))];
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let y = index_1d(gid);
    if y >= a.h {
        return;
    }
    let row = y * a.w;
    let r = i32(a.r);
    let norm = 1.0 / f32(2 * r + 1);
    var sum = 0.0;
    for (var k = -r; k <= r; k++) {
        sum += at(row, k);
    }
    for (var i = 0; i < i32(a.w); i++) {
        dst[row + u32(i)] = sum * norm;
        sum += at(row, i + r + 1) - at(row, i - r);
    }
}
