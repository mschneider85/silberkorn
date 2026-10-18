// Takes the color out of purple and green fringes along edges (defringe_pixel).

struct Args {
    n: u32,
    purple: f32,
    green: f32,
}

@group(0) @binding(0) var<storage, read> a: Args;
@group(0) @binding(1) var<storage, read_write> img: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> mean: array<f32>;
@group(0) @binding(3) var<storage, read> mean_sq: array<f32>;

fn hue_band(hue: f32, lo: f32, hi: f32, soft: f32) -> f32 {
    var d = max(hue - hi, 0.0);
    if hue < lo {
        d = lo - hue;
    }
    return 1.0 - smoothstep(0.0, soft, d);
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = index_1d(gid);
    if i >= a.n {
        return;
    }
    let p = img[i].xyz;
    let y = max(dot(p, LUMA), 0.0);
    let cr = p.x - y;
    let cb = p.z - y;
    if cr == 0.0 && cb == 0.0 {
        return;
    }
    let edge = smoothstep(0.015, 0.06, sqrt(max(mean_sq[i] - mean[i] * mean[i], 0.0)));
    var hue = degrees(atan2(cb, cr));
    if hue < 0.0 {
        hue += 360.0;
    }
    let k = min(edge * (a.purple * hue_band(hue, 25.0, 115.0, 20.0) + a.green * hue_band(hue, 195.0, 250.0, 20.0)), 1.0);
    img[i] = vec4<f32>(p + (vec3<f32>(y) - p) * k, 0.0);
}
