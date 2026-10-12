// Shared helpers, prepended to every kernel. They mirror the CPU functions of the same
// name (pipeline.rs, geometry.rs, color.rs, detail.rs) as closely as possible.

const MID_GREY: f32 = 0.18;
const LUMA: vec3<f32> = vec3<f32>(0.2627, 0.6780, 0.0593);

// Index of a 1-D invocation (see Encoder::run_1d: up to 65535 x 64 per row of groups).
fn index_1d(gid: vec3<u32>) -> u32 {
    return gid.x + gid.y * 4194240u;
}

fn sstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn rem_euclid(x: f32, m: f32) -> f32 {
    let r = x - m * trunc(x / m);
    return select(r, r + m, r < 0.0);
}

fn max3(v: vec3<f32>) -> f32 {
    return max(v.x, max(v.y, v.z));
}

// Row-major 3x3 matrix times vector (Mat3 = [[f32; 3]; 3]).
fn mat_vec(m: array<f32, 9>, v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        m[0] * v.x + m[1] * v.y + m[2] * v.z,
        m[3] * v.x + m[4] * v.y + m[5] * v.z,
        m[6] * v.x + m[7] * v.y + m[8] * v.z,
    );
}

fn srgb_encode(v: f32) -> f32 {
    if v <= 0.0031308 {
        return v * 12.92;
    }
    return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

fn srgb_decode(v: f32) -> f32 {
    if v <= 0.04045 {
        return v / 12.92;
    }
    return pow((v + 0.055) / 1.055, 2.4);
}

// Cube root: bit-level estimate refined by two Halley steps (color::fast_cbrt).
fn fast_cbrt(x: f32) -> f32 {
    let a = abs(x);
    if a == 0.0 {
        return x;
    }
    var y = bitcast<f32>(bitcast<u32>(a) / 3u + 0x2a514067u);
    for (var k = 0; k < 2; k++) {
        let y3 = y * y * y;
        y *= (y3 + 2.0 * a) / (2.0 * y3 + a);
    }
    return select(y, -y, x < 0.0);
}

// ------------------------------------------------------------------ geometry (GeoMap)

struct Geo {
    src_w: f32,
    src_h: f32,
    frame_w: f32,
    frame_h: f32,
    crop_x: f32,
    crop_y: f32,
    cos: f32,
    sin: f32,
    fill: f32,
    kv: f32,
    kh: f32,
    norm: f32,
    half_diag: f32,
    k1: f32,
    pt_a: f32,
    pt_b: f32,
    pt_c: f32,
    p5_1: f32,
    p5_2: f32,
    lens_scale: f32,
    ca_r: f32,
    ca_b: f32,
    vig0: f32,
    vig1: f32,
    vig2: f32,
    turns: u32,
    flip_h: u32,
    flip_v: u32,
    has_pt: u32,
    has_p5: u32,
    identity_lens: u32,
    has_vig: u32,
    pixel_exact: u32,
}

fn geo_centered(g: Geo, ox: f32, oy: f32) -> vec2<f32> {
    let fx = g.crop_x * g.frame_w + ox - g.frame_w / 2.0;
    let fy = g.crop_y * g.frame_h + oy - g.frame_h / 2.0;
    var x = (g.cos * fx - g.sin * fy) / g.fill;
    var y = (g.sin * fx + g.cos * fy) / g.fill;
    if g.flip_h != 0u {
        x = -x;
    }
    if g.flip_v != 0u {
        y = -y;
    }
    var s = vec2<f32>(x, y);
    switch g.turns {
        case 1u: { s = vec2<f32>(y, -x); }
        case 2u: { s = vec2<f32>(-x, -y); }
        case 3u: { s = vec2<f32>(-y, x); }
        default: {}
    }
    if g.kv == 0.0 && g.kh == 0.0 {
        return s;
    }
    let hw = g.src_w / 2.0;
    let hh = g.src_h / 2.0;
    let xn = s.x / hw;
    let yn = s.y / hh;
    let d = max(1.0 + g.kv * yn + g.kh * xn, 0.2);
    let sc = 1.0 / (1.0 + max(abs(g.kv), abs(g.kh)) * 0.5);
    return vec2<f32>(xn / d * hw * sc, yn / d * hh * sc);
}

fn distortion_factor(g: Geo, r2: f32, r: f32) -> f32 {
    var f = 1.0 + g.k1 * r2;
    if g.has_pt != 0u {
        f *= g.pt_a * r2 * r + g.pt_b * r2 + g.pt_c * r + 1.0 - g.pt_a - g.pt_b - g.pt_c;
    }
    if g.has_p5 != 0u {
        f *= 1.0 + g.p5_1 * r2 + g.p5_2 * r2 * r2;
    }
    return f;
}

fn vignetting_gain(g: Geo, x: f32, y: f32) -> f32 {
    if g.has_vig == 0u {
        return 1.0;
    }
    let r2 = (x * x + y * y) / (g.half_diag * g.half_diag);
    let c = 1.0 + g.vig0 * r2 + g.vig1 * r2 * r2 + g.vig2 * r2 * r2 * r2;
    return 1.0 / max(c, 0.2);
}

// Lens correction of a centered source position (before the per-channel CA scale).
fn geo_lens(g: Geo, centered: vec2<f32>) -> vec2<f32> {
    var p = centered;
    if g.identity_lens == 0u {
        let rn = sqrt(p.x * p.x + p.y * p.y) / g.norm;
        p *= distortion_factor(g, rn * rn, rn) * g.lens_scale;
    }
    return p;
}

// Output position -> source pixel position of the green channel (GeoMap::source_point).
fn geo_source_point(g: Geo, ox: f32, oy: f32) -> vec2<f32> {
    return geo_lens(g, geo_centered(g, ox, oy)) + vec2<f32>(g.src_w / 2.0 - 0.5, g.src_h / 2.0 - 0.5);
}
