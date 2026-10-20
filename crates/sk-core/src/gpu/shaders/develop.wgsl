// Tone, color, layers and encoding per pixel (the `pixel` closure of process_encoded).
// Output: encoded working RGB and the mask overlay weight in w.

struct Tone {
    exposure_ev: f32,
    contrast: f32,
    hdr: vec4<f32>,
    clarity: f32,
    structure: f32,
    saturation: f32,
    temperature: f32,
    tint: f32,
    brightness: f32,
    dehaze: f32,
    vibrance: f32,
    veil: f32,
    // Color editor per band: hue shift, saturation and lightness (both /100).
    bands: array<f32, 24>,
    // Color balance offsets per wheel: a, b, lightness.
    balance: array<f32, 12>,
}

// Layout as written by the CPU: only 4-byte members, no padding.
struct Params {
    w: u32,
    h: u32,
    ext_x: u32,
    ext_y: u32,
    n_layers: u32,
    overlay: i32,
    has_ev: u32,
    has_clarity: u32,
    has_structure: u32,
    has_dark: u32,
    color_active: u32,
    editor_active: u32,
    skin_active: u32,
    balance_active: u32,
    bw_enabled: u32,
    base_curve: u32,
    // Tone curve on the largest and smallest channel, the middle one follows (process 3).
    hue_preserve: u32,
    // Offsets into the local planes buffer.
    off_ev: u32,
    off_clarity: u32,
    off_structure: u32,
    off_dark: u32,
    dark_w: u32,
    dark_h: u32,
    dark_step: u32,
    // Offset of the working region in the dark channel's domain.
    dark_ox: u32,
    dark_oy: u32,
    airlight: array<f32, 3>,
    // The atmosphere's haze color (Processor::veil).
    veil: array<f32, 3>,
    // Offsets into the table buffer (u32::MAX: no table).
    lut_hdr_ev: u32,
    lut_hdr_px: u32,
    lut_hdr_len: u32,
    lut_curve: u32,
    lut_out: array<u32, 3>,
    lut_out_len: u32,
    // The luma curve (NO_LUT: a straight line), same length as the output curves.
    lut_luma: u32,
    // Background tone parameters (ToneParams).
    exposure_ev: f32,
    contrast: f32,
    hdr: array<f32, 4>,
    clarity: f32,
    structure: f32,
    saturation: f32,
    dehaze: f32,
    vibrance: f32,
    band_center: array<f32, 8>,
    band_hue: array<f32, 8>,
    band_sat: array<f32, 8>,
    band_light: array<f32, 8>,
    skin_hue: f32,
    skin_chroma: f32,
    skin_lightness: f32,
    skin_range: f32,
    skin_hue_uniformity: f32,
    skin_saturation_uniformity: f32,
    skin_lightness_uniformity: f32,
    skin_hue_shift: f32,
    skin_saturation: f32,
    skin_lightness_shift: f32,
    // master, shadows, midtones, highlights: a, b and lightness offsets
    balance: array<f32, 12>,
    bw_mix: array<f32, 8>,
    // shadow tint hue, amount, highlight tint hue, amount
    bw_tint: array<f32, 4>,
    // Color ranges of one's own: center hue, inner and outer edge, hue, saturation, lightness.
    n_zones: u32,
    zones: array<f32, 48>,
    to_lms: array<f32, 9>,
    from_lms: array<f32, 9>,
    src_w: f32,
    src_h: f32,
    geo: Geo,
}

struct Layer {
    mask_off: u32,
    mask_w: u32,
    mask_h: u32,
    has_bitmap: u32,
    invert: u32,
    has_luma: u32,
    luma_invert: u32,
    has_color: u32,
    neutral: u32,
    opacity: f32,
    luma_low: f32,
    luma_high: f32,
    luma_falloff: f32,
    color_hue: f32,
    color_width: f32,
    color_falloff: f32,
    color_min_chroma: f32,
    exposure: f32,
    contrast: f32,
    brightness: f32,
    saturation: f32,
    highlights: f32,
    shadows: f32,
    clarity: f32,
    structure: f32,
    temperature: f32,
    tint: f32,
    dehaze: f32,
    whites: f32,
    blacks: f32,
    vibrance: f32,
    veil: f32,
    bands: array<f32, 24>,
    balance: array<f32, 12>,
    // Offsets of the layer's curve tables (NO_LUT: none), as long as the output curves.
    curve_r: u32,
    curve_g: u32,
    curve_b: u32,
    curve_luma: u32,
}

@group(0) @binding(0) var<storage, read> P: Params;
@group(0) @binding(1) var<storage, read> working: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> local: array<f32>;
@group(0) @binding(3) var<storage, read> luts: array<f32>;
@group(0) @binding(4) var<storage, read> layers: array<Layer>;
@group(0) @binding(5) var<storage, read> masks: array<f32>;
@group(0) @binding(6) var<storage, read_write> out: array<vec4<f32>>;

const NO_LUT: u32 = 0xffffffffu;
const EV_MIN: f32 = -24.0;
const EV_STEPS: f32 = 128.0;
const CURVE_MAX_SQRT: f32 = 16.0;
const CURVE_SIZE: u32 = 16384u;
const RAD: f32 = 0.017453292519943295;
const DEG: f32 = 57.29577951308232;

fn base_tone() -> Tone {
    var bands: array<f32, 24>;
    for (var i = 0u; i < 8u; i++) {
        bands[i * 3u] = P.band_hue[i];
        bands[i * 3u + 1u] = P.band_sat[i] / 100.0;
        bands[i * 3u + 2u] = P.band_light[i] / 100.0;
    }
    return Tone(P.exposure_ev, P.contrast, vec4<f32>(P.hdr[0], P.hdr[1], P.hdr[2], P.hdr[3]), P.clarity, P.structure, P.saturation, 0.0, 0.0, 0.0, P.dehaze,
        P.vibrance, 0.0, bands, P.balance);
}

// ------------------------------------------------------------------ tables

fn ev_lookup(off: u32, ev: f32) -> f32 {
    let f = clamp((ev - EV_MIN) * EV_STEPS, 0.0, f32(P.lut_hdr_len - 1u));
    let i = min(u32(f), P.lut_hdr_len - 2u);
    let t = f - f32(i);
    return luts[off + i] + (luts[off + i + 1u] - luts[off + i]) * t;
}

fn curve_lookup(v: f32) -> f32 {
    let f = min(sqrt(max(v, 0.0)) * (f32(CURVE_SIZE - 1u) / CURVE_MAX_SQRT), f32(CURVE_SIZE - 1u));
    let i = min(u32(f), CURVE_SIZE - 2u);
    let t = f - f32(i);
    let o = P.lut_curve;
    return clamp(luts[o + i] + (luts[o + i + 1u] - luts[o + i]) * t, 0.0, 1.0);
}

fn lut_lookup(off: u32, len: u32, v: f32) -> f32 {
    let f = clamp(v, 0.0, 1.0) * f32(len - 1u);
    let i = u32(f);
    if i >= len - 1u {
        return luts[off + len - 1u];
    }
    let t = f - f32(i);
    return luts[off + i] * (1.0 - t) + luts[off + i + 1u] * t;
}

fn encode(rgb: vec3<f32>) -> vec3<f32> {
    let e = vec3<f32>(
        lut_lookup(P.lut_out[0], P.lut_out_len, rgb.x),
        lut_lookup(P.lut_out[1], P.lut_out_len, rgb.y),
        lut_lookup(P.lut_out[2], P.lut_out_len, rgb.z),
    );
    if P.lut_luma == NO_LUT {
        return e;
    }
    return luma_curve(P.lut_luma, e);
}

// Luma curve (apply_luma_curve): luminance along the curve, hue and saturation kept.
fn luma_curve(off: u32, e: vec3<f32>) -> vec3<f32> {
    let y = clamp(dot(e, LUMA), 0.0, 1.0);
    let y2 = lut_lookup(off, P.lut_out_len, y);
    if y < 1e-5 {
        return vec3<f32>(y2);
    }
    var o = e * (y2 / y);
    let m = max(o.x, max(o.y, o.z));
    if m > 1.0 {
        let t = clamp((m - 1.0) / max(m - y2, 1e-6), 0.0, 1.0);
        o = o + (vec3<f32>(y2) - o) * t;
    }
    return clamp(o, vec3<f32>(0.0), vec3<f32>(1.0));
}

// A layer's curves (LayerCurve::apply).
fn layer_curve(l: Layer, e: vec3<f32>) -> vec3<f32> {
    var o = e;
    if l.curve_r != NO_LUT {
        o = vec3<f32>(lut_lookup(l.curve_r, P.lut_out_len, e.x), lut_lookup(l.curve_g, P.lut_out_len, e.y), lut_lookup(l.curve_b, P.lut_out_len, e.z));
    }
    if l.curve_luma != NO_LUT {
        o = luma_curve(l.curve_luma, o);
    }
    return o;
}

// ------------------------------------------------------------------ tone

fn sigmoid(x: f32) -> f32 {
    return 1.0 / (1.0 + exp(-x));
}

fn hdr_gain_ev(hdr: vec4<f32>, ev: f32, ev_px: f32) -> f32 {
    let w_hi = sigmoid((ev - 0.5) * 1.6);
    let w_sh = sigmoid(-(ev + 2.0) * 1.4);
    let w_wh = sigmoid((ev_px - 2.0) * 2.5);
    let w_bl = sigmoid(-(ev_px + 4.5) * 2.0);
    return -hdr.x * 1.5 * w_hi + hdr.y * 1.8 * w_sh + hdr.z * 1.0 * w_wh + hdr.w * 1.5 * w_bl;
}

fn aces_fit(x: f32) -> f32 {
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0);
}

fn base_curve(x: f32) -> f32 {
    switch P.base_curve {
        case 0u: { return aces_fit(x * 0.8); }
        case 1u: {
            let y = aces_fit(x * 0.8);
            let s = y * y * (3.0 - 2.0 * y);
            return clamp(y * 0.6 + s * 0.4, 0.0, 1.0);
        }
        default: { return clamp(x, 0.0, 1.0); }
    }
}

// Floored like on the CPU: summed layer contrast must not flatten or invert the tones.
fn contrast_exponent(contrast: f32) -> f32 {
    return max(1.0 + contrast / 100.0 * 0.5, 0.2);
}

fn contrast_and_base_curve(contrast: f32, v0: f32) -> f32 {
    let e = contrast_exponent(contrast);
    var v = v0;
    if e != 1.0 {
        v = MID_GREY * pow(max(v / MID_GREY, 0.0), e);
    }
    if P.base_curve == 2u {
        return clamp(max(v, 0.0), 0.0, 1.0);
    }
    return clamp(base_curve(v), 0.0, 1.0);
}

fn tone(rgb0: vec3<f32>, has_ev: bool, local_ev: f32, detail: vec2<f32>, tp: Tone) -> vec3<f32> {
    let gain = exp2(tp.exposure_ev + detail.x * tp.clarity / 100.0 * 0.6 + detail.y * tp.structure / 100.0 * 0.8);
    var rgb = rgb0 * gain;
    if tp.temperature != 0.0 || tp.tint != 0.0 {
        let t = tp.temperature / 100.0;
        rgb.x *= exp2(t * 0.35);
        rgb.z *= exp2(-t * 0.35);
        rgb.y *= exp2(-tp.tint / 100.0 * 0.25);
    }
    let l = dot(rgb, LUMA);
    if l > 1e-7 && any(tp.hdr != vec4<f32>(0.0)) {
        let ev_px = log2(l / MID_GREY);
        var ev = ev_px;
        if has_ev {
            ev = (local_ev + tp.exposure_ev) * 0.75 + ev_px * 0.25;
        }
        var gain_ev: f32;
        if P.lut_hdr_ev != NO_LUT && all(tp.hdr == base_tone().hdr) {
            gain_ev = ev_lookup(P.lut_hdr_ev, ev) + ev_lookup(P.lut_hdr_px, ev_px);
        } else {
            gain_ev = hdr_gain_ev(tp.hdr, ev, ev_px);
        }
        rgb *= exp2(gain_ev);
    }
    if P.hue_preserve == 0u {
        return vec3<f32>(tone_curve(tp, rgb.x), tone_curve(tp, rgb.y), tone_curve(tp, rgb.z));
    }
    return hue_preserving(tp, rgb);
}

fn tone_curve(tp: Tone, v: f32) -> f32 {
    // The camera's curve: a table without contrast, which warps its input instead.
    if P.base_curve == 3u {
        let e = contrast_exponent(tp.contrast);
        var w = v;
        if e != 1.0 {
            w = MID_GREY * pow(max(v / MID_GREY, 0.0), e);
        }
        return curve_lookup(w);
    }
    if P.lut_curve != NO_LUT && tp.contrast == P.contrast {
        return curve_lookup(v);
    }
    return contrast_and_base_curve(tp.contrast, v);
}

// pipeline::hue_preserving
fn hue_preserving(tp: Tone, rgb: vec3<f32>) -> vec3<f32> {
    var hi = 0u;
    var mid = 1u;
    var lo = 2u;
    let r = rgb.x;
    let g = rgb.y;
    let b = rgb.z;
    if r >= g {
        if g >= b {
            hi = 0u; mid = 1u; lo = 2u;
        } else if r >= b {
            hi = 0u; mid = 2u; lo = 1u;
        } else {
            hi = 2u; mid = 0u; lo = 1u;
        }
    } else if r >= b {
        hi = 1u; mid = 0u; lo = 2u;
    } else if g >= b {
        hi = 1u; mid = 2u; lo = 0u;
    } else {
        hi = 2u; mid = 1u; lo = 0u;
    }
    let h = tone_curve(tp, rgb[hi]);
    let l = tone_curve(tp, rgb[lo]);
    let span = rgb[hi] - rgb[lo];
    let m = select(h, l + (h - l) * (rgb[mid] - rgb[lo]) / span, span > 1e-9);
    // Put together per component: DX12's FXC rejects writes like `out[hi] = h` ("array
    // reference cannot be used as an l-value"), and the pipeline then falls back to the CPU.
    let k = vec3<u32>(0u, 1u, 2u);
    return select(select(vec3<f32>(m), vec3<f32>(l), k == vec3<u32>(lo)), vec3<f32>(h), k == vec3<u32>(hi));
}

// ------------------------------------------------------------------ color

fn oklab_from_linear(rgb: vec3<f32>) -> vec3<f32> {
    let m = mat_vec(P.to_lms, rgb);
    let lms = vec3<f32>(fast_cbrt(m.x), fast_cbrt(m.y), fast_cbrt(m.z));
    return vec3<f32>(
        0.2104542553 * lms.x + 0.793617785 * lms.y - 0.0040720468 * lms.z,
        1.9779984951 * lms.x - 2.428592205 * lms.y + 0.4505937099 * lms.z,
        0.0259040371 * lms.x + 0.7827717662 * lms.y - 0.808675766 * lms.z,
    );
}

fn oklab_to_linear(lab: vec3<f32>) -> vec3<f32> {
    let l_ = lab.x + 0.3963377774 * lab.y + 0.2158037573 * lab.z;
    let m_ = lab.x - 0.1055613458 * lab.y - 0.0638541728 * lab.z;
    let s_ = lab.x - 0.0894841775 * lab.y - 1.291485548 * lab.z;
    return mat_vec(P.from_lms, vec3<f32>(l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_));
}

// Piecewise-linear weights over the circular hue bands (band_weights).
fn band_weights(h: f32) -> array<f32, 8> {
    // Outside all bands: everything on band 0 (lo = 0, t = 0). The weights are built without
    // writing into the array by a variable index, which DX12's FXC can't compile here.
    var lo = 0u;
    var t = 0.0;
    for (var i = 0u; i < 8u; i++) {
        let c0 = P.band_center[i];
        let c1 = P.band_center[(i + 1u) % 8u];
        let span = rem_euclid(c1 - c0, 360.0);
        let d = rem_euclid(h - c0, 360.0);
        if d < span {
            lo = i;
            t = d / span;
            break;
        }
    }
    let hi = (lo + 1u) % 8u;
    let a = vec4<u32>(0u, 1u, 2u, 3u);
    let b = a + 4u;
    let w0 = select(vec4<f32>(0.0), vec4<f32>(1.0 - t), a == vec4<u32>(lo)) + select(vec4<f32>(0.0), vec4<f32>(t), a == vec4<u32>(hi));
    let w1 = select(vec4<f32>(0.0), vec4<f32>(1.0 - t), b == vec4<u32>(lo)) + select(vec4<f32>(0.0), vec4<f32>(t), b == vec4<u32>(hi));
    return array<f32, 8>(w0.x, w0.y, w0.z, w0.w, w1.x, w1.y, w1.z, w1.w);
}

fn skin_weight(h: f32, c: f32) -> f32 {
    let d = abs(rem_euclid(h - P.skin_hue + 540.0, 360.0) - 180.0);
    let range = max(P.skin_range, 1.0);
    let by_hue = 1.0 - sstep(range * 0.5, range, d);
    let by_chroma = sstep(0.008, 0.025, c) * (1.0 - sstep(P.skin_chroma * 2.0 + 0.03, P.skin_chroma * 3.0 + 0.06, c));
    return by_hue * by_chroma;
}

fn color(rgb: vec3<f32>, tp: Tone) -> vec3<f32> {
    if P.color_active == 0u {
        return rgb;
    }
    let lab0 = oklab_from_linear(rgb);
    var l = lab0.x;
    let a0 = lab0.y;
    let b0 = lab0.z;
    let c0 = sqrt(a0 * a0 + b0 * b0);
    var c = c0;
    let need_hue = P.editor_active != 0u || P.skin_active != 0u || P.bw_enabled != 0u;
    var h = 0.0;
    if need_hue {
        h = rem_euclid(atan2(b0, a0) * DEG, 360.0);
    }
    let chroma_w = sstep(0.0, 0.05, c);

    if P.editor_active != 0u {
        let h_in = h;
        let w = band_weights(h);
        var dh = 0.0;
        var ds = 0.0;
        var dl = 0.0;
        for (var i = 0u; i < 8u; i++) {
            dh += w[i] * tp.bands[i * 3u];
            ds += w[i] * tp.bands[i * 3u + 1u];
            dl += w[i] * tp.bands[i * 3u + 2u];
        }
        h += dh * chroma_w;
        c *= max(1.0 + ds * chroma_w, 0.0);
        l *= 1.0 + dl * 0.4 * chroma_w;
        // Ranges of one's own select by the color as it comes in.
        for (var i = 0u; i < P.n_zones; i++) {
            let o = i * 6u;
            let d = abs(rem_euclid(h_in - P.zones[o] + 540.0, 360.0) - 180.0);
            let t = clamp((d - P.zones[o + 1u]) / (P.zones[o + 2u] - P.zones[o + 1u]), 0.0, 1.0);
            let zw = (1.0 - t * t * (3.0 - 2.0 * t)) * chroma_w;
            if zw > 0.0 {
                h += P.zones[o + 3u] * zw;
                c *= max(1.0 + P.zones[o + 4u] / 100.0 * zw, 0.0);
                l *= 1.0 + P.zones[o + 5u] / 100.0 * 0.4 * zw;
            }
        }
    }

    if P.skin_active != 0u {
        let w = skin_weight(h, c);
        if w > 0.0 {
            let dh = rem_euclid(P.skin_hue - h + 540.0, 360.0) - 180.0;
            h += (dh * P.skin_hue_uniformity / 100.0 + P.skin_hue_shift) * w;
            c += (P.skin_chroma - c) * P.skin_saturation_uniformity / 100.0 * w;
            c *= max(1.0 + P.skin_saturation / 100.0 * w, 0.0);
            l += (P.skin_lightness - l) * P.skin_lightness_uniformity / 100.0 * 0.5 * w;
            l *= 1.0 + P.skin_lightness_shift / 100.0 * 0.3 * w;
        }
    }

    let s = 1.0 + tp.saturation / 100.0;
    let vib = tp.vibrance / 100.0;
    if s != 1.0 || vib != 0.0 {
        let low = clamp(1.0 - c / 0.2, 0.0, 1.0);
        c *= max(s * (1.0 + vib * low * low), 0.0);
    }

    var a: f32;
    var b: f32;
    if need_hue {
        let hr = h * RAD;
        a = c * cos(hr);
        b = c * sin(hr);
    } else if c0 > 0.0 {
        a = a0 * (c / c0);
        b = b0 * (c / c0);
    } else {
        a = a0;
        b = b0;
    }

    if P.balance_active != 0u {
        let lw = clamp(l, 0.0, 1.0);
        let ws = (1.0 - lw) * (1.0 - lw);
        let wh = lw * lw;
        let wm = max(1.0 - ws - wh, 0.0);
        let weights = array<f32, 4>(1.0, ws, wm, wh);
        for (var k = 0u; k < 4u; k++) {
            a += tp.balance[k * 3u] * 0.08 * weights[k];
            b += tp.balance[k * 3u + 1u] * 0.08 * weights[k];
            l += tp.balance[k * 3u + 2u] * 0.15 * weights[k];
        }
    }

    if P.bw_enabled != 0u {
        let w = band_weights(h);
        var mix = 0.0;
        for (var i = 0u; i < 8u; i++) {
            mix += w[i] * P.bw_mix[i] / 100.0;
        }
        l *= max(1.0 + mix * 0.6 * sstep(0.0, 0.1, c), 0.0);
        a = 0.0;
        b = 0.0;
        let lw = clamp(l, 0.0, 1.0);
        let weights = array<f32, 2>((1.0 - lw) * (1.0 - lw), lw * lw);
        for (var k = 0u; k < 2u; k++) {
            let hr = P.bw_tint[k * 2u] * RAD;
            let amt = P.bw_tint[k * 2u + 1u] / 100.0 * 0.06 * weights[k];
            a += amt * cos(hr);
            b += amt * sin(hr);
        }
    }
    return clamp(oklab_to_linear(vec3<f32>(l, a, b)), vec3<f32>(0.0), vec3<f32>(1.0));
}

// ------------------------------------------------------------------ local planes

// Plane k of the dark channel's coefficients, bilinear between (x0, y0) and (x1, y1).
fn dark_plane(k: u32, x0: u32, x1: u32, y0: u32, y1: u32, tx: f32, ty: f32) -> f32 {
    let o = P.off_dark + k * P.dark_w * P.dark_h;
    let top = local[o + y0 * P.dark_w + x0] * (1.0 - tx) + local[o + y0 * P.dark_w + x1] * tx;
    let bot = local[o + y1 * P.dark_w + x0] * (1.0 - tx) + local[o + y1 * P.dark_w + x1] * tx;
    return top * (1.0 - ty) + bot * ty;
}

// Haze density at (x, y) of the dark channel's domain for a pixel of color p: the guided
// filter's coefficients a and b applied to the pixel's guide, limited to the third plane.
fn dark_at(x: u32, y: u32, p: vec3<f32>) -> f32 {
    let step = f32(P.dark_step);
    let fx = clamp((f32(x) + 0.5) / step - 0.5, 0.0, f32(P.dark_w - 1u));
    let fy = clamp((f32(y) + 0.5) / step - 0.5, 0.0, f32(P.dark_h - 1u));
    let x0 = u32(fx);
    let y0 = u32(fy);
    let x1 = min(x0 + 1u, P.dark_w - 1u);
    let y1 = min(y0 + 1u, P.dark_h - 1u);
    let tx = fx - f32(x0);
    let ty = fy - f32(y0);
    let ab = vec3<f32>(dark_plane(0u, x0, x1, y0, y1, tx, ty), dark_plane(1u, x0, x1, y0, y1, tx, ty), dark_plane(2u, x0, x1, y0, y1, tx, ty));
    let al = vec3<f32>(P.airlight[0], P.airlight[1], P.airlight[2]);
    let guide = max(dot(p, LUMA), 0.0) / max(dot(al, LUMA), 1e-3);
    return clamp(ab.x * min(guide, ab.z) + ab.y, 0.0, 1.0);
}

// pipeline.rs haze_toe
fn haze_toe(v: f32, k: f32) -> f32 {
    if v >= k {
        return v;
    }
    if k < 1e-6 {
        return max(v, 0.0);
    }
    return k * exp(v / k - 1.0);
}

const HAZE_T_MIN: f32 = 0.25;

// DarkChannel::apply
fn haze(p: vec3<f32>, dark: f32, strength0: f32) -> vec3<f32> {
    if P.has_dark == 0u || strength0 == 0.0 {
        return p;
    }
    let strength = strength0 / 100.0;
    let al = vec3<f32>(P.airlight[0], P.airlight[1], P.airlight[2]);
    if strength > 0.0 {
        let t0 = 1.0 - 0.95 * strength * dark;
        let t = min(0.5 * (t0 + HAZE_T_MIN + sqrt((t0 - HAZE_T_MIN) * (t0 - HAZE_T_MIN) + 0.004)), 1.0);
        let j = (p - al * (1.0 - t)) / t;
        let ya = max(dot(al, LUMA), 1e-3);
        let lift = 1.0 + (pow(t, -0.3) - 1.0) * clamp(1.0 - dot(j, LUMA) / ya, 0.0, 1.0);
        let y = dot(j, LUMA);
        let toe = 0.05 * (1.0 - t) * ya;
        let v = (y + (j - y) * pow(t, 0.4)) * lift;
        return vec3<f32>(haze_toe(v.x, toe), haze_toe(v.y, toe), haze_toe(v.z, toe));
    }
    let s = -strength * 0.6 * (1.0 - dark * 0.5);
    return p + (al - p) * s;
}

// pipeline.rs veiled
fn veiled(p: vec3<f32>, s: f32) -> vec3<f32> {
    if s <= 0.0 {
        return p;
    }
    return mix(p, vec3<f32>(P.veil[0], P.veil[1], P.veil[2]), s);
}

// ------------------------------------------------------------------ masks

fn mask_sample(l: Layer, u: f32, v: f32) -> f32 {
    let x = clamp(u * f32(l.mask_w) - 0.5, 0.0, f32(l.mask_w - 1u));
    let y = clamp(v * f32(l.mask_h) - 0.5, 0.0, f32(l.mask_h - 1u));
    let x0 = u32(x);
    let y0 = u32(y);
    let x1 = min(x0 + 1u, l.mask_w - 1u);
    let y1 = min(y0 + 1u, l.mask_h - 1u);
    let tx = x - f32(x0);
    let ty = y - f32(y0);
    let o = l.mask_off;
    let p00 = masks[o + y0 * l.mask_w + x0];
    let p10 = masks[o + y0 * l.mask_w + x1];
    let p01 = masks[o + y1 * l.mask_w + x0];
    let p11 = masks[o + y1 * l.mask_w + x1];
    let top = p00 + (p10 - p00) * tx;
    let bot = p01 + (p11 - p01) * tx;
    return top + (bot - top) * ty;
}

// Luminance / color range refinement on the background result (range_weight).
fn range_weight(l: Layer, disp: vec3<f32>) -> f32 {
    var w = 1.0;
    if l.has_luma != 0u {
        let y = srgb_encode(clamp(dot(disp, LUMA), 0.0, 1.0));
        let f = max(l.luma_falloff, 1e-3);
        let inside = sstep(l.luma_low - f, l.luma_low, y) * (1.0 - sstep(l.luma_high, l.luma_high + f, y));
        w *= select(inside, 1.0 - inside, l.luma_invert != 0u);
    }
    if l.has_color != 0u {
        let lab = oklab_from_linear(disp);
        let chroma = sqrt(lab.y * lab.y + lab.z * lab.z);
        let hue = rem_euclid(atan2(lab.z, lab.y) * DEG, 360.0);
        let dh = abs(rem_euclid(hue - l.color_hue + 540.0, 360.0) - 180.0);
        let hue_w = 1.0 - sstep(l.color_width, l.color_width + max(l.color_falloff, 1e-3), dh);
        let chroma_w = sstep(l.color_min_chroma * 0.5, max(l.color_min_chroma, 1e-3), chroma);
        w *= hue_w * chroma_w;
    }
    return w;
}

// Mask weight of a layer before its ranges and opacity.
fn mask_weight(l: Layer, u: f32, v: f32) -> f32 {
    var wgt = 0.0;
    if l.has_bitmap != 0u {
        wgt = mask_sample(l, u, v);
    }
    if l.invert != 0u {
        wgt = 1.0 - wgt;
    }
    return wgt;
}

fn add_layer(tp: ptr<function, Tone>, l: Layer, w: f32) {
    (*tp).exposure_ev += l.exposure * w;
    (*tp).contrast += l.contrast * w;
    (*tp).hdr.x += l.highlights / 100.0 * w;
    (*tp).hdr.y += l.shadows / 100.0 * w;
    (*tp).clarity += l.clarity * w;
    (*tp).structure += l.structure * w;
    (*tp).saturation += l.saturation * w;
    (*tp).temperature += l.temperature * w;
    (*tp).tint += l.tint * w;
    (*tp).brightness += l.brightness * w;
    (*tp).dehaze += l.dehaze * w;
    (*tp).hdr.z += l.whites / 100.0 * w;
    (*tp).hdr.w += l.blacks / 100.0 * w;
    (*tp).vibrance += l.vibrance * w;
    (*tp).veil += l.veil * w;
    (*tp).bands = add_bands((*tp).bands, l.bands, w);
    (*tp).balance = add_balance((*tp).balance, l.balance, w);
}

// a + b * w for the band and balance arrays, written out: a loop writing `a[i]` inside the loop
// over the layers is more than DX12's FXC compiles ("unable to unroll loop").
fn add_bands(a: array<f32, 24>, b: array<f32, 24>, w: f32) -> array<f32, 24> {
    return array<f32, 24>(
        a[0] + b[0] * w,
        a[1] + b[1] * w,
        a[2] + b[2] * w,
        a[3] + b[3] * w,
        a[4] + b[4] * w,
        a[5] + b[5] * w,
        a[6] + b[6] * w,
        a[7] + b[7] * w,
        a[8] + b[8] * w,
        a[9] + b[9] * w,
        a[10] + b[10] * w,
        a[11] + b[11] * w,
        a[12] + b[12] * w,
        a[13] + b[13] * w,
        a[14] + b[14] * w,
        a[15] + b[15] * w,
        a[16] + b[16] * w,
        a[17] + b[17] * w,
        a[18] + b[18] * w,
        a[19] + b[19] * w,
        a[20] + b[20] * w,
        a[21] + b[21] * w,
        a[22] + b[22] * w,
        a[23] + b[23] * w,
    );
}

fn add_balance(a: array<f32, 12>, b: array<f32, 12>, w: f32) -> array<f32, 12> {
    return array<f32, 12>(
        a[0] + b[0] * w,
        a[1] + b[1] * w,
        a[2] + b[2] * w,
        a[3] + b[3] * w,
        a[4] + b[4] * w,
        a[5] + b[5] * w,
        a[6] + b[6] * w,
        a[7] + b[7] * w,
        a[8] + b[8] * w,
        a[9] + b[9] * w,
        a[10] + b[10] * w,
        a[11] + b[11] * w,
    );
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= P.w || gid.y >= P.h {
        return;
    }
    let i = gid.y * P.w + gid.x;
    let p = working[i].xyz;
    var ev = 0.0;
    if P.has_ev != 0u {
        ev = local[P.off_ev + i];
    }
    var det = vec2<f32>(0.0);
    if P.has_clarity != 0u {
        det.x = local[P.off_clarity + i];
    }
    if P.has_structure != 0u {
        det.y = local[P.off_structure + i];
    }
    var dark = 0.0;
    if P.has_dark != 0u {
        dark = dark_at(P.dark_ox + gid.x, P.dark_oy + gid.y, p);
    }
    let base = base_tone();
    let has_ev = P.has_ev != 0u;
    if P.n_layers == 0u {
        out[i] = vec4<f32>(encode(color(tone(haze(p, dark, base.dehaze), has_ev, ev, det, base), base)), 0.0);
        return;
    }
    let ox = f32(P.ext_x + gid.x) + 0.5;
    let oy = f32(P.ext_y + gid.y) + 0.5;
    let sp = geo_source_point(P.geo, ox, oy);
    let u = (sp.x + 0.5) / P.src_w;
    let v = (sp.y + 0.5) / P.src_h;
    var tp = base;
    var any_layer = false;
    var curved = false;
    var overlay = 0.0;
    var have_base = false;
    var base_disp = vec3<f32>(0.0);
    for (var li = 0u; li < P.n_layers; li++) {
        let l = layers[li];
        var wgt = mask_weight(l, u, v);
        if wgt > 0.0 && (l.has_luma != 0u || l.has_color != 0u) {
            if !have_base {
                base_disp = color(tone(haze(p, dark, base.dehaze), has_ev, ev, det, base), base);
                have_base = true;
            }
            wgt *= range_weight(l, base_disp);
        }
        wgt *= l.opacity;
        if i32(li) == P.overlay {
            overlay = wgt;
        }
        if wgt > 0.0 && l.neutral == 0u {
            add_layer(&tp, l, wgt);
            any_layer = true;
        }
        if wgt > 0.0 && (l.curve_r != NO_LUT || l.curve_luma != NO_LUT) {
            curved = true;
        }
    }
    var e: vec3<f32>;
    if any_layer {
        e = encode(color(tone(veiled(haze(p, dark, tp.dehaze), tp.veil), has_ev, ev, det, tp), tp));
        if tp.brightness != 0.0 {
            let g = exp2(-tp.brightness / 100.0 * 0.6);
            e = pow(max(e, vec3<f32>(0.0)), vec3<f32>(g));
        }
    } else {
        if !have_base {
            base_disp = color(tone(haze(p, dark, base.dehaze), has_ev, ev, det, base), base);
            have_base = true;
        }
        e = encode(base_disp);
    }
    // The layers' curves on the result, each blended in by its mask.
    if curved {
        for (var li = 0u; li < P.n_layers; li++) {
            let l = layers[li];
            if l.curve_r == NO_LUT && l.curve_luma == NO_LUT {
                continue;
            }
            var wgt = mask_weight(l, u, v);
            if wgt > 0.0 && (l.has_luma != 0u || l.has_color != 0u) {
                if !have_base {
                    base_disp = color(tone(haze(p, dark, base.dehaze), has_ev, ev, det, base), base);
                    have_base = true;
                }
                wgt *= range_weight(l, base_disp);
            }
            wgt *= l.opacity;
            if wgt > 0.0 {
                e = mix(e, layer_curve(l, e), wgt);
            }
        }
    }
    out[i] = vec4<f32>(e, overlay);
}
