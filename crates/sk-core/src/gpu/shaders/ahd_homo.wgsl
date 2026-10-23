// AHD steps 3 and 4: how many of the four neighbors are as close as the smaller of the two
// directions' typical differences (lightness and color apart), per direction (packed: row
// direction in the low byte).

@group(0) @binding(2) var<storage, read> green: array<vec2<f32>>;
@group(0) @binding(3) var<storage, read_write> homo: array<u32>;

fn dist(p: vec3<f32>, q: vec3<f32>) -> vec2<f32> {
    return vec2<f32>(abs(p.x - q.x), (p.y - q.y) * (p.y - q.y) + (p.z - q.z) * (p.z - q.z));
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= a.w || gid.y >= a.h {
        return;
    }
    let x = i32(gid.x);
    let y = i32(gid.y);
    let offs = array<vec2<i32>, 4>(vec2<i32>(-1, 0), vec2<i32>(1, 0), vec2<i32>(0, -1), vec2<i32>(0, 1));
    var c: array<vec3<f32>, 2>;
    var n: array<array<vec3<f32>, 4>, 2>;
    for (var d = 0u; d < 2u; d++) {
        c[d] = lab(rgb_dir(x, y, d));
        for (var k = 0; k < 4; k++) {
            n[d][k] = lab(rgb_dir(x + offs[k].x, y + offs[k].y, d));
        }
    }
    let hl = dist(c[0], n[0][0]);
    let hr = dist(c[0], n[0][1]);
    let vu = dist(c[1], n[1][2]);
    let vd = dist(c[1], n[1][3]);
    let eps_l = min(max(hl.x, hr.x), max(vu.x, vd.x));
    let eps_c = min(max(hl.y, hr.y), max(vu.y, vd.y));
    var packed = 0u;
    for (var d = 0u; d < 2u; d++) {
        var count = 0u;
        for (var k = 0; k < 4; k++) {
            let e = dist(c[d], n[d][k]);
            if e.x <= eps_l && e.y <= eps_c {
                count += 1u;
            }
        }
        packed |= count << (8u * d);
    }
    homo[gid.y * a.w + gid.x] = packed;
}
