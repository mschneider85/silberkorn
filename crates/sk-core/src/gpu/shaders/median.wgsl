// Median of 2r+1 values (r <= 24) along rows (dir 0) or columns (dir 1), edges
// clamped, one invocation per pixel (detail::median_hv).

struct Args {
    w: u32,
    h: u32,
    r: u32,
    dir: u32,
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
    let r = i32(min(a.r, 24u));
    let n = 2 * r + 1;
    var win: array<f32, 49>;
    for (var k = 0; k < n; k++) {
        if a.dir == 0u {
            win[k] = src[u32(y) * a.w + u32(clamp(x + k - r, 0, i32(a.w) - 1))];
        } else {
            win[k] = src[u32(clamp(y + k - r, 0, i32(a.h) - 1)) * a.w + u32(x)];
        }
    }
    // The median is the value with at most r smaller ones and more than r smaller or equal.
    var med = win[0];
    for (var i = 0; i < n; i++) {
        var less = 0;
        var equal = 0;
        for (var j = 0; j < n; j++) {
            less += select(0, 1, win[j] < win[i]);
            equal += select(0, 1, win[j] == win[i]);
        }
        if less <= r && less + equal > r {
            med = win[i];
        }
    }
    dst[i] = med;
}
