// One pass of a gaussian with sampled weights and clamped edges, one invocation per pixel
// (detail::gaussian_blur_exact).

struct Args {
    w: u32,
    h: u32,
    r: u32,
    vertical: u32,
    sigma: f32,
}

@group(0) @binding(0) var<storage, read> a: Args;
@group(0) @binding(1) var<storage, read> src: array<f32>;
@group(0) @binding(2) var<storage, read_write> dst: array<f32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = index_1d(gid);
    if i >= a.w * a.h {
        return;
    }
    let x = i32(i % a.w);
    let y = i32(i / a.w);
    let r = i32(a.r);
    let k2 = 1.0 / (2.0 * a.sigma * a.sigma);
    var sum = 0.0;
    var norm = 0.0;
    for (var k = -r; k <= r; k++) {
        let wgt = exp(-f32(k * k) * k2);
        var j: u32;
        if a.vertical == 1u {
            j = u32(clamp(y + k, 0, i32(a.h) - 1)) * a.w + u32(x);
        } else {
            j = u32(y) * a.w + u32(clamp(x + k, 0, i32(a.w) - 1));
        }
        sum += wgt * src[j];
        norm += wgt;
    }
    dst[i] = sum / norm;
}
