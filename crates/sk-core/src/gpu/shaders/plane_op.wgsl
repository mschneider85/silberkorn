// Element-wise operations on planes (the closures of the CPU filters).

struct Args {
    n: u32,
    op: u32,
    k: f32,
}

@group(0) @binding(0) var<storage, read> a: Args;
@group(0) @binding(1) var<storage, read> x: array<f32>;
@group(0) @binding(2) var<storage, read> y: array<f32>;
@group(0) @binding(3) var<storage, read> z: array<f32>;
@group(0) @binding(4) var<storage, read_write> out: array<f32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = index_1d(gid);
    if i >= a.n {
        return;
    }
    switch a.op {
        // x²
        case 0u: { out[i] = x[i] * x[i]; }
        // Guided filter coefficient a from mean x and mean of squares y.
        case 1u: {
            let v = max(y[i] - x[i] * x[i], 0.0);
            out[i] = v / (v + a.k);
        }
        // Guided filter coefficient b from mean x and coefficient y.
        case 2u: { out[i] = x[i] - y[i] * x[i]; }
        // Guided filter output from mean coefficients x, y and guide z.
        case 3u: { out[i] = x[i] * z[i] + y[i]; }
        // Detail (difference to the smoothed plane y), clamped to ±k.
        case 4u: { out[i] = clamp(x[i] - y[i], -a.k, a.k); }
        // Partly back towards the original x from the filtered y (keep = k).
        case 5u: { out[i] = y[i] + (x[i] - y[i]) * a.k; }
        default: {}
    }
}
