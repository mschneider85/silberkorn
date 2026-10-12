// Unsharp mask on luminance.

struct Args {
    n: u32,
    amount: f32,
    thr: f32,
}

@group(0) @binding(0) var<storage, read> a: Args;
@group(0) @binding(1) var<storage, read_write> img: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> y: array<f32>;
@group(0) @binding(3) var<storage, read> blur: array<f32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = index_1d(gid);
    if i >= a.n {
        return;
    }
    let diff = y[i] - blur[i];
    var soft = 1.0;
    if a.thr > 0.0 {
        soft = min(abs(diff) / a.thr, 1.0);
    }
    let delta = diff * a.amount * soft;
    let p = img[i];
    img[i] = vec4<f32>(clamp(p.xyz + delta, vec3<f32>(0.0), vec3<f32>(1.0)), p.w);
}
