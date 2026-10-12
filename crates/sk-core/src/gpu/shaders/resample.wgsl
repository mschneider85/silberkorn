// Geometry (GeoMap::resample) and, optionally, white balance + camera matrix (to_working).

struct Args {
    geo: Geo,
    // Output region in output pixels.
    region_x: u32,
    region_y: u32,
    region_w: u32,
    region_h: u32,
    // Part of the source that was uploaded (source pixels).
    win_x: u32,
    win_y: u32,
    win_w: u32,
    win_h: u32,
    wb: array<f32, 3>,
    cam: array<f32, 9>,
    hdr: u32,
    to_working: u32,
}

@group(0) @binding(0) var<storage, read> a: Args;
@group(0) @binding(1) var<storage, read> src: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> out: array<vec4<f32>>;

fn at(xx: i32, yy: i32) -> vec3<f32> {
    let x = clamp(xx, 0, i32(a.geo.src_w) - 1) - i32(a.win_x);
    let y = clamp(yy, 0, i32(a.geo.src_h) - 1) - i32(a.win_y);
    let wx = clamp(x, 0, i32(a.win_w) - 1);
    let wy = clamp(y, 0, i32(a.win_h) - 1);
    return src[u32(wy) * a.win_w + u32(wx)].xyz;
}

fn bilinear(x: f32, y: f32) -> vec3<f32> {
    if x < -0.5 || y < -0.5 || x > a.geo.src_w - 0.5 || y > a.geo.src_h - 0.5 {
        return vec3<f32>(0.0);
    }
    let x0 = floor(x);
    let y0 = floor(y);
    let tx = x - x0;
    let ty = y - y0;
    let xi = i32(x0);
    let yi = i32(y0);
    let p00 = at(xi, yi);
    let p10 = at(xi + 1, yi);
    let p01 = at(xi, yi + 1);
    let p11 = at(xi + 1, yi + 1);
    let top = p00 + (p10 - p00) * tx;
    let bot = p01 + (p11 - p01) * tx;
    return top + (bot - top) * ty;
}

fn to_working(raw: vec3<f32>) -> vec3<f32> {
    var b = raw * vec3<f32>(a.wb[0], a.wb[1], a.wb[2]);
    if max3(raw) > 0.95 && a.hdr == 0u {
        let hi = max3(b);
        for (var c = 0; c < 3; c++) {
            let t = sstep(0.95, 1.0, raw[c]);
            b[c] += (max(b[c], hi) - b[c]) * t;
        }
    }
    return max(mat_vec(a.cam, b), vec3<f32>(0.0));
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= a.region_w || gid.y >= a.region_h {
        return;
    }
    let i = gid.y * a.region_w + gid.x;
    let ox = f32(a.region_x + gid.x) + 0.5;
    let oy = f32(a.region_y + gid.y) + 0.5;
    let g = a.geo;
    let cx = g.src_w / 2.0 - 0.5;
    let cy = g.src_h / 2.0 - 0.5;
    var v = vec3<f32>(0.0);
    if g.pixel_exact != 0u {
        let p = geo_centered(g, ox, oy);
        let sx = floor(p.x + cx + 0.5);
        let sy = floor(p.y + cy + 0.5);
        if sx >= 0.0 && sy >= 0.0 && sx < g.src_w && sy < g.src_h {
            v = at(i32(sx), i32(sy));
        }
    } else {
        let c = geo_centered(g, ox, oy);
        let gain = vignetting_gain(g, c.x, c.y);
        let p = geo_lens(g, c);
        if g.ca_r == g.ca_b {
            // Red and blue at the same position as green (as on the CPU).
            v = bilinear(p.x + cx, p.y + cy);
        } else {
            v.x = bilinear(p.x * g.ca_r + cx, p.y * g.ca_r + cy).x;
            v.y = bilinear(p.x + cx, p.y + cy).y;
            v.z = bilinear(p.x * g.ca_b + cx, p.y * g.ca_b + cy).z;
        }
        v *= gain;
    }
    if a.to_working != 0u {
        v = to_working(v);
    }
    out[i] = vec4<f32>(v, 0.0);
}
