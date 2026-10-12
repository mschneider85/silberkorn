// Block average by s (fast guided filter).

struct Args {
    w: u32,
    lw: u32,
    lh: u32,
    s: u32,
}

@group(0) @binding(0) var<storage, read> a: Args;
@group(0) @binding(1) var<storage, read> src: array<f32>;
@group(0) @binding(2) var<storage, read_write> dst: array<f32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = index_1d(gid);
    if i >= a.lw * a.lh {
        return;
    }
    let x = i % a.lw * a.s;
    let y = i / a.lw * a.s;
    var acc = 0.0;
    for (var yy = y; yy < y + a.s; yy++) {
        var row = 0.0;
        for (var xx = x; xx < x + a.s; xx++) {
            row += src[yy * a.w + xx];
        }
        acc += row;
    }
    dst[i] = acc / f32(a.s * a.s);
}
