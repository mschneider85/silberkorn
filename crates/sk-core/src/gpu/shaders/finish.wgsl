// Post-crop vignette and grain (in output pixel coordinates).

struct Args {
    w: u32,
    h: u32,
    ext_x: u32,
    ext_y: u32,
    full_w: f32,
    full_h: f32,
    vignette: f32,
    grain: f32,
    cell: f32,
}

@group(0) @binding(0) var<storage, read> a: Args;
@group(0) @binding(1) var<storage, read_write> img: array<vec4<f32>>;

// 64-bit integers as (low, high) halves.
fn mul32(a: u32, b: u32) -> vec2<u32> {
    let al = a & 0xffffu;
    let ah = a >> 16u;
    let bl = b & 0xffffu;
    let bh = b >> 16u;
    let ll = al * bl;
    let lh = al * bh;
    let hl = ah * bl;
    let hh = ah * bh;
    let mid = (ll >> 16u) + (lh & 0xffffu) + (hl & 0xffffu);
    return vec2<u32>((ll & 0xffffu) | (mid << 16u), hh + (lh >> 16u) + (hl >> 16u) + (mid >> 16u));
}

fn mul64(a: vec2<u32>, b: vec2<u32>) -> vec2<u32> {
    let p = mul32(a.x, b.x);
    return vec2<u32>(p.x, p.y + a.x * b.y + a.y * b.x);
}

fn xor_shift33(h: vec2<u32>) -> vec2<u32> {
    return vec2<u32>(h.x ^ (h.y >> 1u), h.y);
}

fn to_u64(v: i32) -> vec2<u32> {
    return vec2<u32>(bitcast<u32>(v), select(0u, 0xffffffffu, v < 0));
}

// detail::hash_noise
fn hash_noise(x: i32, y: i32, seed: u32) -> f32 {
    var h = mul64(to_u64(x), vec2<u32>(0x7F4A7C15u, 0x9E3779B9u)) ^ mul64(to_u64(y), vec2<u32>(0x27D4EB4Fu, 0xC2B2AE3Du));
    h.x ^= seed;
    h = xor_shift33(h);
    h = mul64(h, vec2<u32>(0xED558CCDu, 0xFF51AFD7u));
    h = xor_shift33(h);
    return f32(h.x & 0xffffu) / 32767.5 - 1.0;
}

// detail::value_noise
fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let x0 = floor(x);
    let y0 = floor(y);
    let tx = x - x0;
    let ty = y - y0;
    let sx = tx * tx * (3.0 - 2.0 * tx);
    let sy = ty * ty * (3.0 - 2.0 * ty);
    let xi = i32(x0);
    let yi = i32(y0);
    let p00 = hash_noise(xi, yi, seed);
    let p10 = hash_noise(xi + 1, yi, seed);
    let p01 = hash_noise(xi, yi + 1, seed);
    let p11 = hash_noise(xi + 1, yi + 1, seed);
    let top = p00 + (p10 - p00) * sx;
    let bot = p01 + (p11 - p01) * sx;
    return top + (bot - top) * sy;
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= a.w || gid.y >= a.h {
        return;
    }
    let i = gid.y * a.w + gid.x;
    var p = img[i].xyz;
    let ox = f32(a.ext_x + gid.x) + 0.5;
    let oy = f32(a.ext_y + gid.y) + 0.5;
    if a.vignette != 0.0 {
        let cx = a.full_w / 2.0;
        let cy = a.full_h / 2.0;
        let half_diag = sqrt(cx * cx + cy * cy);
        let r = min(sqrt((ox - cx) * (ox - cx) + (oy - cy) * (oy - cy)) / half_diag, 1.0);
        let f = sstep(0.35, 1.05, r) * abs(a.vignette);
        if a.vignette < 0.0 {
            p = p * (1.0 - f);
        } else {
            p = p + (1.0 - p) * f;
        }
    }
    if a.grain > 0.0 {
        let n = value_noise(ox / a.cell, oy / a.cell, 17u) * 0.7 + value_noise(ox / a.cell * 2.3, oy / a.cell * 2.3, 29u) * 0.3;
        let l = dot(p, LUMA);
        let amp = a.grain * max(4.0 * l * (1.0 - l), 0.15);
        p = clamp(p + n * amp, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    img[i] = vec4<f32>(p, img[i].w);
}
