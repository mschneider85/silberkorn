// Recombines the image from denoised luminance and color differences (noise_reduction).

struct Args {
    n: u32,
    has_y: u32,
    has_c: u32,
}

@group(0) @binding(0) var<storage, read> a: Args;
@group(0) @binding(1) var<storage, read_write> img: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> ynew: array<f32>;
@group(0) @binding(3) var<storage, read> cr: array<f32>;
@group(0) @binding(4) var<storage, read> cb: array<f32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = index_1d(gid);
    if i >= a.n {
        return;
    }
    let p = img[i].xyz;
    let y_old = max(dot(p, LUMA), 0.0);
    var y2 = y_old;
    if a.has_y != 0u {
        let v = max(ynew[i], 0.0);
        y2 = v * v;
    }
    var dr = p.x - y_old;
    var db = p.z - y_old;
    if a.has_c != 0u {
        dr = cr[i];
        db = cb[i];
    }
    let r = y2 + dr;
    let b = y2 + db;
    let g = (y2 - LUMA.x * r - LUMA.z * b) / LUMA.y;
    img[i] = vec4<f32>(max(r, 0.0), max(g, 0.0), max(b, 0.0), 0.0);
}
