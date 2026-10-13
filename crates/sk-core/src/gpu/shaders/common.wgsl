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

// Desaturates a color towards the gray of luminance y until it fits the 0..1 cube (color::gamut_map).
fn gamut_map(c: vec3<f32>, y_in: f32) -> vec3<f32> {
    if all(c >= vec3<f32>(0.0)) && all(c <= vec3<f32>(1.0)) {
        return c;
    }
    let y = clamp(y_in, 0.0, 1.0);
    var t = 1.0;
    for (var i = 0u; i < 3u; i++) {
        let v = c[i];
        if v > 1.0 {
            t = min(t, (1.0 - y) / max(v - y, 1e-9));
        } else if v < 0.0 {
            t = min(t, y / max(y - v, 1e-9));
        }
    }
    t = clamp(t, 0.0, 1.0);
    return clamp(vec3<f32>(y) + t * (c - vec3<f32>(y)), vec3<f32>(0.0), vec3<f32>(1.0));
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
    ks: f32,
    fish_f: f32,
    fish_fr: f32,
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
    // The camera's own correction (camera_lens::CameraLens): optical center offset and
    // polynomials in r² (half diagonals) per channel, vignetting gain.
    has_cam: u32,
    cam_cx: f32,
    cam_cy: f32,
    cam_warp: array<f32, 12>,
    cam_vig: array<f32, 5>,
}

// Rectilinear -> fisheye position (GeoMap::defish).
fn geo_defish(g: Geo, p: vec2<f32>) -> vec2<f32> {
    let r = length(p);
    if g.fish_f == 0.0 || r < 1e-6 {
        return p;
    }
    return p * (g.fish_f * atan(r / g.fish_fr) / r);
}

fn geo_centered(g: Geo, ox: f32, oy: f32) -> vec2<f32> {
    return geo_defish(g, geo_plane(g, ox, oy));
}

// Centered position before the fisheye and lens correction (GeoMap::frame_to_source).
fn geo_plane(g: Geo, ox: f32, oy: f32) -> vec2<f32> {
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
    return vec2<f32>(xn / d * hw * g.ks, yn / d * hh * g.ks);
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

fn cam_factor(g: Geo, c: u32, r2: f32) -> f32 {
    let k = c * 4u;
    return g.cam_warp[k] + r2 * (g.cam_warp[k + 1u] + r2 * (g.cam_warp[k + 2u] + r2 * g.cam_warp[k + 3u]));
}

fn cam_gain(g: Geo, r2: f32) -> f32 {
    let v = g.cam_vig;
    return 1.0 + r2 * (v[0] + r2 * (v[1] + r2 * (v[2] + r2 * (v[3] + r2 * v[4]))));
}

// Centered source position of channel `c` after the camera's correction (GeoMap::map), from the
// lens-corrected centered position `p`; `ca` is the slider's scale for that channel.
fn cam_point(g: Geo, p: vec2<f32>, c: u32, ca: f32) -> vec2<f32> {
    let center = vec2<f32>(g.cam_cx, g.cam_cy);
    let u = p / g.half_diag - center;
    return (u * cam_factor(g, c, dot(u, u)) + center) * ca * g.half_diag;
}

// Output position -> source pixel position of the green channel (GeoMap::source_point).
fn geo_source_point(g: Geo, ox: f32, oy: f32) -> vec2<f32> {
    var p = geo_lens(g, geo_centered(g, ox, oy));
    if g.has_cam != 0u {
        p = cam_point(g, p, 1u, 1.0);
    }
    return p + vec2<f32>(g.src_w / 2.0 - 0.5, g.src_h / 2.0 - 0.5);
}
