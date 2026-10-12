// Dark channel input: block minimum (as float bits, all values are >= 0) and every 7th
// pixel of the domain (for the airlight estimate). The domain is the working region, or
// the whole image when rendering in tiles.

struct Args {
    // Pixels of the working region that belong to this rendering.
    own_x: u32,
    own_y: u32,
    own_w: u32,
    own_h: u32,
    // Working region stride and its offset in the domain.
    w: u32,
    off_x: u32,
    off_y: u32,
    dom_w: u32,
    step: u32,
    // Block grid width.
    lw: u32,
}

@group(0) @binding(0) var<storage, read> a: Args;
@group(0) @binding(1) var<storage, read> min_c: array<f32>;
@group(0) @binding(2) var<storage, read_write> pooled: array<atomic<u32>>;
@group(0) @binding(3) var<storage, read_write> samples: array<f32>;
@group(0) @binding(4) var<storage, read_write> count: array<atomic<u32>>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= a.own_w || gid.y >= a.own_h {
        return;
    }
    let x = a.own_x + gid.x;
    let y = a.own_y + gid.y;
    let v = min_c[y * a.w + x];
    let dx = a.off_x + x;
    let dy = a.off_y + y;
    atomicMin(&pooled[(dy / a.step) * a.lw + dx / a.step], bitcast<u32>(max(v, 0.0)));
    if (dy * a.dom_w + dx) % 7u == 0u {
        let k = atomicAdd(&count[0], 1u);
        samples[k] = v;
    }
}
