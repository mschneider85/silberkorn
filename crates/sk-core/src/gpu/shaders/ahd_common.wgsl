// AHD demosaic on the card (ahd.rs): shared parts of its three kernels. The whole mosaic at
// once; neighbors beyond the edge are mirrored as on the CPU (the formulas are symmetric, so a
// mirrored neighbor's value is the one the CPU computes there).

struct AhdArgs {
    w: u32,
    h: u32,
    // Color (0 red, 1 green, 2 blue) of [row % 2][col % 2], two bits each, row-major.
    pattern: u32,
    wb0: f32,
    wb1: f32,
    wb2: f32,
}

@group(0) @binding(0) var<storage, read> a: AhdArgs;
@group(0) @binding(1) var<storage, read> raw: array<f32>;

fn mirror(i: i32, n: i32) -> i32 {
    var j = i;
    if j < 0 {
        j = -j;
    } else if j >= n {
        j = 2 * n - 2 - j;
    }
    return clamp(j, 0, n - 1);
}

fn at(x: i32, y: i32) -> u32 {
    return u32(mirror(y, i32(a.h))) * a.w + u32(mirror(x, i32(a.w)));
}

fn cfa(x: i32, y: i32) -> u32 {
    return (a.pattern >> (2u * u32((y & 1) * 2 + (x & 1)))) & 3u;
}

fn wb(c: u32) -> f32 {
    if c == 0u {
        return a.wb0;
    }
    if c == 1u {
        return a.wb1;
    }
    return a.wb2;
}

// The balanced photosite (Mosaic::at times the white balance of its color).
fn rawv(x: i32, y: i32) -> f32 {
    return raw[at(x, y)] * wb(cfa(x, y));
}

// (fast_cbrt: common.wgsl, as color::fast_cbrt.)
fn lab(p: vec3<f32>) -> vec3<f32> {
    let c = vec3<f32>(fast_cbrt(max(p.x, 0.0)), fast_cbrt(max(p.y, 0.0)), fast_cbrt(max(p.z, 0.0)));
    return vec3<f32>((c.x + 2.0 * c.y + c.z) / 4.0, c.x - c.y, c.y - c.z);
}
