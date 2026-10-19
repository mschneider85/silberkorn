//! Lens corrections that the camera stored in the raw file (distortion, lateral chromatic
//! aberration, vignetting).
//!
//! Every maker's model is converted to one form: per color channel, the source radius as an
//! even polynomial of the corrected radius, and the vignetting gain as an even polynomial.
//! Radii are relative to the half diagonal of the image, around the optical center.
//!
//! Sources: Sony ARW (tags 0x7032, 0x7035, 0x7037 in the raw SubIFD), Fujifilm RAF (0xF00B,
//! 0xF00F, 0xF010 in the Fuji IFD), Panasonic RW2 (0x0119, distortion only) and DNG
//! (WarpRectilinear and FixVignetteRadial in OpcodeList3). The maker formats are not
//! documented; their meaning follows the reverse engineering of darktable and ExifTool.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use rawler::bits::Endian;
use rayon::prelude::*;
use rawler::formats::tiff::{IFD, Value};

use crate::image::Orientation;

#[derive(Clone, Debug, PartialEq)]
pub struct CameraLens {
    /// Who measured it, for the user interface ("Sony", "DNG", …).
    pub source: &'static str,
    /// Optical center minus image center, in half diagonals (x right, y down).
    pub center: [f32; 2],
    /// Per channel R, G, B: r_source = r · (k0 + k1 r² + k2 r⁴ + k3 r⁶).
    pub warp: [[f32; 4]; 3],
    /// Brightening: gain = 1 + v1 r² + v2 r⁴ + … + v5 r¹⁰.
    pub vignette: [f32; 5],
}

impl CameraLens {
    pub fn has_warp(&self) -> bool {
        self.warp.iter().any(|k| *k != [1.0, 0.0, 0.0, 0.0])
    }

    pub fn has_vignette(&self) -> bool {
        self.vignette != [0.0; 5]
    }

    /// Source radius / corrected radius of channel `c` at r² (radius in half diagonals).
    #[inline]
    pub fn factor(&self, c: usize, r2: f32) -> f32 {
        let k = &self.warp[c];
        k[0] + r2 * (k[1] + r2 * (k[2] + r2 * k[3]))
    }

    #[inline]
    pub fn gain(&self, r2: f32) -> f32 {
        let v = &self.vignette;
        1.0 + r2 * (v[0] + r2 * (v[1] + r2 * (v[2] + r2 * (v[3] + r2 * v[4]))))
    }

    /// The same correction for the image turned by `o` (only the optical center moves).
    pub fn oriented(mut self, o: Orientation) -> Self {
        let [x, y] = self.center;
        self.center = match o {
            Orientation::Normal => [x, y],
            Orientation::FlipH => [-x, y],
            Orientation::Rotate180 => [-x, -y],
            Orientation::FlipV => [x, -y],
            Orientation::Transpose => [y, x],
            Orientation::Rotate90 => [-y, x],
            Orientation::Transverse => [-y, -x],
            Orientation::Rotate270 => [y, -x],
        };
        self
    }
}

/// Least-squares fit of `values` over `radii` with an even polynomial
/// c0 + c1 r² + … + c(N-1) r^(2N-2). Used to bring knot tables into the common form.
pub fn fit_even<const N: usize>(radii: &[f32], values: &[f32]) -> Option<[f32; N]> {
    fit_powers(radii, values, 0)
}

/// Like [`fit_even`], with the powers r^(2·first) … r^(2·(first+N-1)).
fn fit_powers<const N: usize>(radii: &[f32], values: &[f32], first: i32) -> Option<[f32; N]> {
    // Normal equations in f64; the radii are at most ~1.5, so the system is well conditioned.
    let mut a = [[0.0f64; N]; N];
    let mut b = [0.0f64; N];
    for (&r, &v) in radii.iter().zip(values) {
        let r2 = (r as f64) * (r as f64);
        let mut basis = [r2.powi(first); N];
        for i in 1..N {
            basis[i] = basis[i - 1] * r2;
        }
        for i in 0..N {
            b[i] += basis[i] * v as f64;
            for j in 0..N {
                a[i][j] += basis[i] * basis[j];
            }
        }
    }
    // Gaussian elimination with partial pivoting.
    for col in 0..N {
        let pivot = (col..N).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        for row in col + 1..N {
            let f = a[row][col] / a[col][col];
            for k in col..N {
                a[row][k] -= f * a[col][k];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = [0.0f64; N];
    for row in (0..N).rev() {
        let s: f64 = (row + 1..N).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - s) / a[row][row];
    }
    Some(x.map(|v| v as f32))
}

/// Linear interpolation in a knot table (radii ascending); constant beyond the ends.
pub fn interpolate(radii: &[f32], values: &[f32], r: f32) -> f32 {
    let n = radii.len().min(values.len());
    if n == 0 {
        return 0.0;
    }
    if r <= radii[0] {
        return values[0];
    }
    for i in 1..n {
        if r <= radii[i] {
            let t = (r - radii[i - 1]) / (radii[i] - radii[i - 1]).max(1e-9);
            return values[i - 1] + (values[i] - values[i - 1]) * t;
        }
    }
    values[n - 1]
}

/// Distortion (per channel) and vignetting gain sampled at corrected radii, fitted into the
/// common form. `warp`: (corrected radius, [R, G, B] source radius factor); `gain`: (source
/// radius, gain).
fn from_samples(source: &'static str, warp: &[(f32, [f32; 3])], gain: &[(f32, f32)]) -> Option<CameraLens> {
    let mut lens = CameraLens { source, center: [0.0; 2], warp: [[1.0, 0.0, 0.0, 0.0]; 3], vignette: [0.0; 5] };
    if warp.len() >= 4 {
        let radii: Vec<f32> = warp.iter().map(|w| w.0).collect();
        for c in 0..3 {
            let values: Vec<f32> = warp.iter().map(|w| w.1[c]).collect();
            lens.warp[c] = fit_even::<4>(&radii, &values)?;
        }
    }
    if gain.len() >= 5 {
        let radii: Vec<f32> = gain.iter().map(|g| g.0).collect();
        let values: Vec<f32> = gain.iter().map(|g| g.1 - 1.0).collect();
        lens.vignette = fit_powers::<5>(&radii, &values, 1)?;
    }
    let sane = lens.warp.iter().flatten().chain(&lens.vignette).all(|v| v.is_finite())
        // At the corner, a few percent of distortion and at most about two stops of vignetting.
        && (0..3).all(|c| (0.7..1.3).contains(&lens.factor(c, 1.0)))
        && (0.9..4.5).contains(&lens.gain(1.0));
    (sane && (lens.has_warp() || lens.has_vignette())).then_some(lens)
}

/// Reads the lens correction stored in a raw file, if there is one. `width`, `height`: size
/// of the developed (cropped, not yet oriented) image.
pub fn read(path: &Path, width: usize, height: usize) -> Option<CameraLens> {
    let mut f = BufReader::new(File::open(path).ok()?);
    let mut head = [0u8; 16];
    f.read_exact(&mut head).ok()?;
    if head.starts_with(b"FUJIFILMCCD-RAW") {
        return fuji(&mut f);
    }
    let endian = match &head[..2] {
        b"II" => Endian::Little,
        b"MM" => Endian::Big,
        _ => return None,
    };
    let u16_at = |b: &[u8]| if endian == Endian::Little { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) };
    let u32_at = |b: &[u8]| {
        let b = [b[0], b[1], b[2], b[3]];
        if endian == Endian::Little { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }
    };
    let magic = u16_at(&head[2..4]);
    let ifds = all_ifds(&mut f, u32_at(&head[4..8]), 0, endian)?;
    let find = |tag: u16| ifds.iter().find_map(|i| i.get_entry(tag)).map(|e| &e.value);
    if magic == 0x55 {
        return find(0x0119).and_then(|v| panasonic(&numbers(v)));
    }
    if let Some(Value::Undefined(list)) = find(0xC74E) {
        return dng(list, width, height);
    }
    sony(find(0x7037).map(numbers), find(0x7035).map(numbers), find(0x7032).map(numbers))
}

/// IFD0, its chain and all SubIFDs (EXIF IFDs are skipped; they hold nothing used here).
fn all_ifds<R: Read + Seek>(r: &mut R, first: u32, base: u32, endian: Endian) -> Option<Vec<IFD>> {
    let mut out = Vec::new();
    let mut next = first;
    while next != 0 && out.len() < 8 {
        let ifd = IFD::new(r, next, base, 0, endian, &[0x014A]).ok()?;
        next = ifd.next_ifd;
        out.push(ifd);
    }
    let mut i = 0;
    while i < out.len() && out.len() < 32 {
        let subs: Vec<IFD> = out[i].sub_ifds().values().flatten().cloned().collect();
        out.extend(subs);
        i += 1;
    }
    Some(out)
}

/// All values of a numeric entry as floats.
fn numbers(v: &Value) -> Vec<f64> {
    match v {
        Value::Byte(v) => v.iter().map(|&x| x as f64).collect(),
        Value::Short(v) => v.iter().map(|&x| x as f64).collect(),
        Value::Long(v) => v.iter().map(|&x| x as f64).collect(),
        Value::SByte(v) => v.iter().map(|&x| x as f64).collect(),
        Value::SShort(v) => v.iter().map(|&x| x as f64).collect(),
        Value::SLong(v) => v.iter().map(|&x| x as f64).collect(),
        Value::Rational(v) => v.iter().map(|x| if x.d == 0 { 0.0 } else { x.n as f64 / x.d as f64 }).collect(),
        Value::SRational(v) => v.iter().map(|x| if x.d == 0 { 0.0 } else { x.n as f64 / x.d as f64 }).collect(),
        Value::Float(v) => v.iter().map(|&x| x as f64).collect(),
        Value::Double(v) => v.clone(),
        // Some files store the int16 tables as plain bytes.
        Value::Undefined(b) => b.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f64).collect(),
        _ => Vec::new(),
    }
}

/// Sony: n knots at radii (i + 0.5) / (n - 1) of the corrected image, values in 1/2^14
/// (distortion), 1/2^21 (red and blue relative to green) and a vignetting code per knot.
fn sony(distortion: Option<Vec<f64>>, ca: Option<Vec<f64>>, vignetting: Option<Vec<f64>>) -> Option<CameraLens> {
    let table = |v: &Option<Vec<f64>>, per_knot: usize| -> Option<Vec<f64>> {
        let v = v.as_ref()?;
        let n = *v.first()? as usize / per_knot;
        (2..=16).contains(&n).then(|| v.get(1..1 + n * per_knot)).flatten().map(|t| t.to_vec())
    };
    let knot = |i: usize, n: usize| (i as f32 + 0.5) / (n - 1) as f32;
    let mut warp = Vec::new();
    if let Some(d) = table(&distortion, 1) {
        let n = d.len();
        let ca = table(&ca, 2).filter(|c| c.len() == 2 * n);
        for (i, v) in d.iter().enumerate() {
            let g = 1.0 + *v as f32 / 16384.0;
            let (r, b) = ca.as_ref().map_or((0.0, 0.0), |c| (c[i] as f32, c[n + i] as f32));
            warp.push((knot(i, n), [g * (1.0 + r / 2097152.0), g, g * (1.0 + b / 2097152.0)]));
        }
    }
    let mut gain = Vec::new();
    if let Some(v) = table(&vignetting, 1) {
        let n = v.len();
        for (i, v) in v.iter().enumerate() {
            gain.push((knot(i, n), 2f32.powf(2f32.powf(*v as f32 / 8192.0 - 1.0) - 0.5)));
        }
    }
    from_samples("Sony", &warp, &gain)
}

/// Fujifilm: the Fuji IFD (RAF offset 100) holds knot radii of the uncorrected image and per
/// knot the distortion in percent, red/blue scale deviations and the brightness in percent.
fn fuji<R: Read + Seek>(f: &mut R) -> Option<CameraLens> {
    let mut b = [0u8; 4];
    f.seek(SeekFrom::Start(100)).ok()?;
    f.read_exact(&mut b).ok()?;
    let base = u32::from_be_bytes(b);
    let mut h = [0u8; 8];
    f.seek(SeekFrom::Start(base as u64)).ok()?;
    f.read_exact(&mut h).ok()?;
    let endian = match &h[..2] {
        b"II" => Endian::Little,
        b"MM" => Endian::Big,
        _ => return None,
    };
    let first = if endian == Endian::Little { u32::from_le_bytes(h[4..8].try_into().ok()?) } else { u32::from_be_bytes(h[4..8].try_into().ok()?) };
    let root = IFD::new(f, first, base, 0, endian, &[0xF000]).ok()?;
    // rawler files the Fuji IFD under the entry's type (13), not its tag.
    let ifd = root.sub_ifds().values().flatten().find(|i| i.get_entry(0xF010u16).is_some() || i.get_entry(0xF00Bu16).is_some()).unwrap_or(&root);
    let get = |tag: u16| ifd.get_entry(tag).map(|e| numbers(&e.value));
    let split = |v: &[f64]| -> Option<(Vec<f32>, Vec<f32>)> {
        let n = match v.len() {
            19 => 9,
            23 => 11,
            _ => return None,
        };
        Some((v[1..=n].iter().map(|&x| x as f32).collect(), v[n + 1..=2 * n].iter().map(|&x| x as f32).collect()))
    };
    let (mut knots, mut dist) = get(0xF00B).as_deref().and_then(split).unzip();
    // CA: 9 knots shared with the distortion, or 10 own knots without the center.
    let ca = get(0xF00F).and_then(|v| match v.len() {
        29 => Some((None, v[10..19].to_vec(), v[19..28].to_vec())),
        31 => Some((Some([&[0.0][..], &v[1..11]].concat()), [&[0.0][..], &v[11..21]].concat(), [&[0.0][..], &v[21..31]].concat())),
        _ => None,
    });
    let vig = get(0xF010).as_deref().and_then(split);
    if knots.is_none() {
        knots = vig.as_ref().map(|v| v.0.clone());
        dist = knots.as_ref().map(|k| vec![0.0; k.len()]);
    }
    let (knots, dist) = (knots?, dist?);
    // A table that doesn't start at the center gets a neutral knot there.
    let prepend = |k: &[f32], v: &[f32], neutral: f32| -> (Vec<f32>, Vec<f32>) {
        if k.first().is_some_and(|&r| r > 0.0) { ([&[0.0][..], k].concat(), [&[neutral][..], v].concat()) } else { (k.to_vec(), v.to_vec()) }
    };
    let (dk, dv) = prepend(&knots, &dist, 0.0);
    let ca = ca.map(|(k, r, b)| {
        let k: Vec<f32> = k.map_or(knots.clone(), |k| k.iter().map(|&x| x as f32).collect());
        let to32 = |v: Vec<f64>| v.into_iter().map(|x| x as f32).collect::<Vec<_>>();
        let (rk, r) = prepend(&k, &to32(r), 0.0);
        let (_, b) = prepend(&k, &to32(b), 0.0);
        (rk, r, b)
    });
    // Defined over the uncorrected radius: sample there, the corrected radius is r / factor.
    let mut warp = Vec::new();
    for i in 0..=15 {
        let rs = i as f32 / 15.0;
        let m = 1.0 + interpolate(&dk, &dv, rs) / 100.0;
        let (r, b) = ca.as_ref().map_or((0.0, 0.0), |(k, r, b)| (interpolate(k, r, rs), interpolate(k, b, rs)));
        warp.push((rs / m, [m * (1.0 + r), m, m * (1.0 + b)]));
    }
    let gain: Vec<(f32, f32)> = vig
        .map(|(k, v)| {
            let (k, v) = prepend(&k, &v, 100.0);
            k.iter().zip(&v).filter(|(_, v)| **v > 1.0).map(|(&r, &v)| (r, 100.0 / v)).collect()
        })
        .unwrap_or_default();
    let has_dist = dv.iter().any(|v| *v != 0.0) || ca.is_some();
    from_samples("Fujifilm", if has_dist { &warp } else { &[] }, &gain)
}

/// Panasonic: tag 0x0119, 16 int16. Distortion (on if word 7 is 1) as
/// r_corrected = r (1 + s (a r² + b r⁴ + c r⁶)) of the uncorrected radius r, with
/// a, b, c = words 8, 4, 11 and s = 1 / (1 + word 5), all in 1/32768.
fn panasonic(w: &[f64]) -> Option<CameraLens> {
    if w.len() < 16 || (w[7] as i64 & 0xF) != 1 {
        return None;
    }
    let q = |i: usize| w[i] / 32768.0;
    let s = 1.0 / (1.0 + q(5));
    let (a, b, c) = (q(8), q(4), q(11));
    let warp: Vec<(f32, [f32; 3])> = (0..=16)
        .map(|i| {
            let rd = i as f64 / 15.0;
            let r2 = rd * rd;
            let m = 1.0 + s * (a * r2 + b * r2 * r2 + c * r2 * r2 * r2);
            // Source radius / corrected radius = 1 / m, at the corrected radius rd * m.
            ((rd * m) as f32, [(1.0 / m) as f32; 3])
        })
        .collect();
    from_samples("Panasonic", &warp, &[])
}

/// DNG OpcodeList3 (big-endian): WarpRectilinear (1) and FixVignetteRadial (3), radii relative to
/// the distance from the optical center to the farthest corner.
fn dng(list: &[u8], width: usize, height: usize) -> Option<CameraLens> {
    let mut lens = CameraLens { source: "DNG", center: [0.0; 2], warp: [[1.0, 0.0, 0.0, 0.0]; 3], vignette: [0.0; 5] };
    let (w, h) = (width as f64, height as f64);
    let half_diag = (w * w + h * h).sqrt() / 2.0;
    // Scale of a DNG radius into ours, given the normalized optical center.
    let center = |cx: f64, cy: f64| {
        let (px, py) = (cx * w, cy * h);
        let m = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)].iter().map(|(x, y)| ((x - px).powi(2) + (y - py).powi(2)).sqrt()).fold(0.0, f64::max);
        ([((px - w / 2.0) / half_diag) as f32, ((py - h / 2.0) / half_diag) as f32], half_diag / m.max(1.0))
    };
    for op in opcodes(list)? {
        let d = |i: usize| op.params.get(i * 8..i * 8 + 8).map(|b| f64::from_be_bytes(b.try_into().unwrap()));
        match op.id {
            1 => {
                let n = u32::from_be_bytes(op.params.get(0..4)?.try_into().ok()?) as usize;
                if !(n == 1 || n == 3) || op.params.len() < 4 + 48 * n + 16 {
                    continue;
                }
                let at = |i: usize| f64::from_be_bytes(op.params[4 + i * 8..12 + i * 8].try_into().unwrap());
                let (c, scale) = center(at(6 * n), at(6 * n + 1));
                lens.center = c;
                for ch in 0..3 {
                    let k = if n == 3 { ch } else { 0 };
                    lens.warp[ch] = std::array::from_fn(|i| (at(6 * k + i) * scale.powi(2 * i as i32)) as f32);
                    if at(6 * k + 4) != 0.0 || at(6 * k + 5) != 0.0 {
                        log::debug!("DNG WarpRectilinear: tangentiale Verzeichnung wird ignoriert");
                    }
                }
            }
            3 => {
                let (Some(k), Some(cx), Some(cy)) = ((0..5).map(d).collect::<Option<Vec<f64>>>(), d(5), d(6)) else { continue };
                let (c, scale) = center(cx, cy);
                lens.center = c;
                lens.vignette = std::array::from_fn(|i| (k[i] * scale.powi(2 * (i as i32 + 1))) as f32);
            }
            _ => {}
        }
    }
    let finite = lens.warp.iter().flatten().chain(&lens.vignette).all(|v| v.is_finite());
    (finite && (lens.has_warp() || lens.has_vignette())).then_some(lens)
}

/// One entry of a DNG opcode list.
pub(crate) struct Opcode<'a> {
    pub id: u32,
    /// Bit 0: optional; bit 1: may be skipped for previews.
    pub flags: u32,
    pub params: &'a [u8],
}

/// Splits a DNG opcode list (always big-endian).
pub(crate) fn opcodes(list: &[u8]) -> Option<Vec<Opcode<'_>>> {
    let be = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let count = be(list.get(0..4)?) as usize;
    let mut out = Vec::new();
    let mut at = 4;
    for _ in 0..count.min(256) {
        let head = list.get(at..at + 16)?;
        let size = be(&head[12..16]) as usize;
        let params = list.get(at + 16..at + 16 + size)?;
        out.push(Opcode { id: be(&head[0..4]), flags: be(&head[8..12]), params });
        at += 16 + size;
    }
    Some(out)
}

/// DNG GainMap opcode (9): a grid of gains over the image, for lens shading. Applied to the
/// scaled sensor data (0..1) of the active area, before demosaicing (OpcodeList2).
#[derive(Clone, Debug, PartialEq)]
pub struct GainMap {
    /// Affected area (bottom and right exclusive) and every `pitch`-th row/column in it.
    pub area: [u32; 4],
    pub plane: u32,
    pub planes: u32,
    pub row_pitch: u32,
    pub col_pitch: u32,
    pub points: [u32; 2],
    pub spacing: [f64; 2],
    pub origin: [f64; 2],
    pub map_planes: u32,
    /// Row-major, plane innermost.
    pub gains: Vec<f32>,
}

/// The gain maps of a DNG OpcodeList2. An unknown opcode that isn't optional means the list
/// can't be applied faithfully: then nothing is applied.
pub fn gain_maps(list: &[u8]) -> Vec<GainMap> {
    let Some(ops) = opcodes(list) else { return Vec::new() };
    let mut maps = Vec::new();
    for op in ops {
        let p = op.params;
        let u = |i: usize| p.get(i..i + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()));
        let d = |i: usize| p.get(i..i + 8).map(|b| f64::from_be_bytes(b.try_into().unwrap()));
        let parsed = (|| {
            if op.id != 9 {
                return None;
            }
            let points = [u(32)?, u(36)?];
            let map_planes = u(72)?;
            // Untrusted sizes: the product must not overflow.
            let n = (points[0] as usize).checked_mul(points[1] as usize)?.checked_mul(map_planes as usize)?;
            if n == 0 || n > 1 << 22 || p.len() < 76 + 4 * n {
                return None;
            }
            // A gain that isn't a number leaves the pixel as it is.
            let gains = p[76..76 + 4 * n].chunks_exact(4).map(|b| f32::from_be_bytes(b.try_into().unwrap())).map(|g| if g.is_finite() { g } else { 1.0 }).collect();
            let fix = |pts: u32, s: f64, o: f64| if pts == 1 { (1.0, 0.0) } else { (s, o) };
            let (sv, ov) = fix(points[0], d(40)?, d(56)?);
            let (sh, oh) = fix(points[1], d(48)?, d(64)?);
            if !(sv.is_finite() && sv > 0.0 && sh.is_finite() && sh > 0.0 && ov.is_finite() && oh.is_finite()) {
                return None;
            }
            Some(GainMap {
                area: [u(0)?, u(4)?, u(8)?, u(12)?],
                plane: u(16)?,
                planes: u(20)?,
                row_pitch: u(24)?.max(1),
                col_pitch: u(28)?.max(1),
                points,
                spacing: [sv, sh],
                origin: [ov, oh],
                map_planes,
                gains,
            })
        })();
        match parsed {
            Some(m) => maps.push(m),
            None if op.flags & 1 == 0 => {
                log::warn!("DNG-Opcode {} in OpcodeList2 nicht unterstützt; Liste wird nicht angewendet", op.id);
                return Vec::new();
            }
            None => {}
        }
    }
    maps
}

impl GainMap {
    /// Multiplies the pixels of `data` (`width` x `height`, `cpp` values per pixel, 0..1).
    pub fn apply(&self, data: &mut [f32], width: usize, height: usize, cpp: usize) {
        let [top, left, bottom, right] = self.area.map(|v| v as usize);
        let (bottom, right) = (bottom.min(height), right.min(width));
        let (pv, ph) = (self.points[0] as usize, self.points[1] as usize);
        let mp = self.map_planes.max(1) as usize;
        // The map's planes count from the first plane it applies to; a single one serves all.
        let at = |r: usize, c: usize, plane: usize| self.gains[(r * ph + c) * mp + (plane - self.plane as usize).min(mp - 1)];
        // Grid position of a pixel center, clamped to the grid.
        let grid = |pos: usize, size: usize, axis: usize, n: usize| {
            let g = (((pos as f64 + 0.5) / size as f64 - self.origin[axis]) / self.spacing[axis]).clamp(0.0, (n - 1) as f64);
            let i = (g.floor() as usize).min(n.saturating_sub(2));
            (i, (g - i as f64) as f32)
        };
        // The grid position of each column once; the rows in parallel.
        let cols: Vec<(usize, usize, usize, f32)> = (left..right)
            .step_by(self.col_pitch as usize)
            .map(|col| {
                let (c0, tc) = grid(col, width, 1, ph);
                (col, c0, (c0 + 1).min(ph - 1), tc)
            })
            .collect();
        let pitch = self.row_pitch as usize;
        data.par_chunks_mut(width * cpp).enumerate().filter(|(row, _)| (top..bottom).contains(row) && (row - top) % pitch == 0).for_each(
            |(row, line)| {
                let (r0, tr) = grid(row, height, 0, pv);
                let r1 = (r0 + 1).min(pv - 1);
                for &(col, c0, c1, tc) in &cols {
                    for plane in self.plane as usize..self.plane.saturating_add(self.planes) as usize {
                        if plane >= cpp {
                            break;
                        }
                        let top = at(r0, c0, plane) + (at(r0, c1, plane) - at(r0, c0, plane)) * tc;
                        let bot = at(r1, c0, plane) + (at(r1, c1, plane) - at(r1, c0, plane)) * tc;
                        let v = &mut line[col * cpp + plane];
                        *v = (*v * (top + (bot - top) * tr)).min(1.0);
                    }
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn even_fit_recovers_a_polynomial() {
        let radii: Vec<f32> = (0..=20).map(|i| i as f32 / 16.0).collect();
        let values: Vec<f32> = radii.iter().map(|r| 1.0 + 0.02 * r * r - 0.005 * r.powi(4) + 0.001 * r.powi(6)).collect();
        let k = fit_even::<4>(&radii, &values).unwrap();
        for (want, got) in [1.0, 0.02, -0.005, 0.001].iter().zip(k) {
            assert!((want - got).abs() < 1e-4, "{k:?}");
        }
        assert_eq!(interpolate(&[0.0, 1.0], &[1.0, 3.0], 0.25), 1.5);
        assert_eq!(interpolate(&[0.0, 1.0], &[1.0, 3.0], 2.0), 3.0);
    }

    #[test]
    fn sony_tables_become_polynomials() {
        // 16 knots: +2 % pincushion correction at the corner (rising with r²), red 0.1 % wider.
        let n = 16;
        let r = |i: usize| (i as f32 + 0.5) / (n - 1) as f32;
        let d: Vec<f64> = [n as f64].into_iter().chain((0..n).map(|i| (0.02 * r(i).powi(2) * 16384.0) as f64)).collect();
        let ca: Vec<f64> = [2.0 * n as f64].into_iter().chain((0..n).map(|_| 0.001 * 2097152.0)).chain((0..n).map(|_| 0.0)).collect();
        // Code 0 in the center, 8192 (half a stop brighter) at the corner.
        let v: Vec<f64> = [n as f64].into_iter().chain((0..n).map(|i| (8192.0 * r(i).powi(2)) as f64)).collect();
        let lens = sony(Some(d), Some(ca), Some(v)).unwrap();
        assert!((lens.factor(1, 1.0) - 1.02).abs() < 1e-3, "{lens:?}");
        assert!((lens.factor(0, 1.0) / lens.factor(1, 1.0) - 1.001).abs() < 1e-4);
        assert!((lens.gain(1.0) - 2f32.sqrt()).abs() < 0.02, "{}", lens.gain(1.0));
        // Implausible tables are dropped.
        assert!(sony(Some(vec![16.0; 17].into_iter().map(|_| 9000.0).collect()), None, None).is_none());
    }

    #[test]
    fn sony_sample_matches_the_lensfun_profile() {
        let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/_DSC0009.ARW");
        if !sample.exists() {
            return;
        }
        // FE 16-35mm F2.8 GM at 32 mm: lensfun's profile moves the corner by +2.1 %.
        let lens = read(&sample, 6000, 4000).expect("Sony-Korrekturdaten");
        assert_eq!(lens.source, "Sony");
        assert!((1.015..1.03).contains(&lens.factor(1, 1.0)), "{lens:?}");
        assert!(lens.gain(1.0) > 1.05);
    }

    #[test]
    fn panasonic_distortion_corrects_barrel() {
        let mut w = [0.0f64; 16];
        w[7] = 1.0;
        w[8] = 0.1 * 32768.0;
        let lens = panasonic(&w).unwrap();
        // r_corrected = r (1 + 0.1 r²): the corner of the result comes from further inside.
        assert!(lens.factor(1, 1.0) < 0.93 && lens.factor(1, 1.0) > 0.88, "{lens:?}");
        w[7] = 0.0;
        assert!(panasonic(&w).is_none());
    }

    fn opcode(id: u32, flags: u32, params: &[u8]) -> Vec<u8> {
        [id, 0x0103_0000, flags, params.len() as u32].iter().flat_map(|v| v.to_be_bytes()).chain(params.iter().copied()).collect()
    }

    fn list(ops: &[Vec<u8>]) -> Vec<u8> {
        (ops.len() as u32).to_be_bytes().into_iter().chain(ops.iter().flatten().copied()).collect()
    }

    #[test]
    fn dng_warp_and_vignette_use_our_normalization() {
        let doubles = |v: &[f64]| v.iter().flat_map(|d| d.to_be_bytes()).collect::<Vec<u8>>();
        // One set for all planes, centered: r_src = r (1 - 0.04 r²) in DNG radii (= half diagonal).
        let warp = [1u32.to_be_bytes().to_vec(), doubles(&[1.0, -0.04, 0.0, 0.0, 0.0, 0.0, 0.5, 0.5])].concat();
        let vig = doubles(&[0.3, 0.0, 0.0, 0.0, 0.0, 0.5, 0.5]);
        let l = list(&[opcode(1, 1, &warp), opcode(3, 1, &vig), opcode(99, 1, &[])]);
        let lens = dng(&l, 300, 200).unwrap();
        assert_eq!(lens.center, [0.0, 0.0]);
        assert!((lens.factor(2, 1.0) - 0.96).abs() < 1e-6 && (lens.gain(1.0) - 1.3).abs() < 1e-6);

        // Off-center: the farthest corner is the normalization, our coefficients scale accordingly.
        let warp = [1u32.to_be_bytes().to_vec(), doubles(&[1.0, -0.04, 0.0, 0.0, 0.0, 0.0, 0.25, 0.5])].concat();
        let lens = dng(&list(&[opcode(1, 0, &warp)]), 300, 200).unwrap();
        let half_diag = (300f32.powi(2) + 200f32.powi(2)).sqrt() / 2.0;
        let m = (225f32.powi(2) + 100f32.powi(2)).sqrt();
        assert!((lens.center[0] + 75.0 / half_diag).abs() < 1e-5);
        assert!((lens.warp[1][1] + 0.04 * (half_diag / m).powi(2)).abs() < 1e-6);
    }

    #[test]
    fn gain_map_brightens_its_pixels_only() {
        let mut p: Vec<u8> = [0u32, 0, 4, 4, 0, 1, 2, 2, 2, 2].iter().flat_map(|v| v.to_be_bytes()).collect();
        p.extend([1.0f64, 1.0, 0.0, 0.0].iter().flat_map(|v| v.to_be_bytes()));
        p.extend(1u32.to_be_bytes());
        // Gain 1 at the top, 2 at the bottom.
        p.extend([1.0f32, 1.0, 2.0, 2.0].iter().flat_map(|v| v.to_be_bytes()));
        let maps = gain_maps(&list(&[opcode(9, 0, &p)]));
        assert_eq!(maps.len(), 1);
        let mut data = vec![0.25f32; 16];
        maps[0].apply(&mut data, 4, 4, 1);
        // Every second pixel of every second row, interpolated by the pixel center's row.
        assert!((data[0] - 0.25 * 1.125).abs() < 1e-6, "{data:?}");
        assert_eq!(data[1], 0.25);
        assert_eq!(data[4], 0.25);
        assert!((data[8] - 0.25 * 1.625).abs() < 1e-6, "{data:?}");
        // An unknown opcode that isn't optional: nothing is applied.
        assert!(gain_maps(&list(&[opcode(9, 0, &p), opcode(77, 0, &[])])).is_empty());
    }

    /// A map for planes 1 and 2 starts its own planes at 0.
    #[test]
    fn gain_map_planes_count_from_its_first_plane() {
        let map = GainMap {
            area: [0, 0, 1, 1],
            plane: 1,
            planes: 2,
            row_pitch: 1,
            col_pitch: 1,
            points: [1, 1],
            spacing: [1.0, 1.0],
            origin: [0.0, 0.0],
            map_planes: 2,
            gains: vec![2.0, 3.0],
        };
        let mut data = vec![0.25f32; 3];
        map.apply(&mut data, 1, 1, 3);
        assert_eq!(data, [0.25, 0.5, 0.75]);
    }

    #[test]
    fn damaged_gain_maps_are_not_applied() {
        let params = |points: [u32; 2], spacing: f64, planes: u32| {
            let mut p: Vec<u8> = [0u32, 0, 4, 4, 0, 1, 1, 1, points[0], points[1]].iter().flat_map(|v| v.to_be_bytes()).collect();
            p.extend([spacing, spacing, 0.0, 0.0].iter().flat_map(|v| v.to_be_bytes()));
            p.extend(planes.to_be_bytes());
            p.extend([1.0f32; 4].iter().flat_map(|v| v.to_be_bytes()));
            p
        };
        assert_eq!(gain_maps(&list(&[opcode(9, 0, &params([2, 2], 1.0, 1))])).len(), 1);
        // Sizes whose product overflows, a spacing of 0 or not a number.
        assert!(gain_maps(&list(&[opcode(9, 0, &params([1 << 31, 1 << 31], 1.0, 5))])).is_empty());
        assert!(gain_maps(&list(&[opcode(9, 0, &params([2, 2], 0.0, 1))])).is_empty());
        assert!(gain_maps(&list(&[opcode(9, 0, &params([2, 2], f64::NAN, 1))])).is_empty());
    }

    #[test]
    fn orientation_moves_the_center_with_the_image() {
        let lens = CameraLens { source: "Test", center: [0.1, 0.0], warp: [[1.0, 0.0, 0.0, 0.0]; 3], vignette: [0.0; 5] };
        // Turned clockwise, a center right of the middle ends up below it.
        assert_eq!(lens.clone().oriented(Orientation::Rotate90).center, [-0.0, 0.1]);
        assert_eq!(lens.oriented(Orientation::FlipH).center, [-0.1, 0.0]);
    }
}
