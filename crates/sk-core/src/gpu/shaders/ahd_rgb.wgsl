// The color of a pixel for one direction `d` from the greens (step 2 of ahd.rs), for the kernels
// that read the greens.

fn gv(x: i32, y: i32, d: u32) -> f32 {
    return green[at(x, y)][d];
}

fn rgb_dir(x: i32, y: i32, d: u32) -> vec3<f32> {
    let g = gv(x, y, d);
    let own = cfa(x, y);
    var p = array<f32, 3>(0.0, g, 0.0);
    if own == 1u {
        let hc = cfa(x + 1, y);
        let vc = cfa(x, y + 1);
        p[hc] = g + ((rawv(x - 1, y) - gv(x - 1, y, d)) + (rawv(x + 1, y) - gv(x + 1, y, d))) / 2.0;
        p[vc] = g + ((rawv(x, y - 1) - gv(x, y - 1, d)) + (rawv(x, y + 1) - gv(x, y + 1, d))) / 2.0;
    } else {
        let other = 2u - own;
        p[own] = rawv(x, y);
        let s = (rawv(x - 1, y - 1) - gv(x - 1, y - 1, d)) + (rawv(x + 1, y - 1) - gv(x + 1, y - 1, d))
            + (rawv(x - 1, y + 1) - gv(x - 1, y + 1, d)) + (rawv(x + 1, y + 1) - gv(x + 1, y + 1, d));
        p[other] = g + s / 4.0;
    }
    return max(vec3<f32>(p[0], p[1], p[2]), vec3<f32>(0.0));
}
