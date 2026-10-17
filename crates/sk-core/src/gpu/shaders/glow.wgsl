// Glow (Orton effect): the blurred image, mostly its brighter parts, screened over it.
// Same as `pipeline::glow`.

struct Args {
    n: u32,
    k: f32,
}

@group(0) @binding(0) var<storage, read> a: Args;
@group(0) @binding(1) var<storage, read_write> img: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> br: array<f32>;
@group(0) @binding(3) var<storage, read> bg: array<f32>;
@group(0) @binding(4) var<storage, read> bb: array<f32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = index_1d(gid);
    if i >= a.n {
        return;
    }
    let b = vec3<f32>(br[i], bg[i], bb[i]);
    let g = a.k * 0.8 * smoothstep(0.05, 0.85, dot(b, LUMA));
    let p = img[i];
    img[i] = vec4<f32>(1.0 - (1.0 - p.xyz) * (1.0 - g * clamp(b, vec3<f32>(0.0), vec3<f32>(1.0))), p.w);
}
