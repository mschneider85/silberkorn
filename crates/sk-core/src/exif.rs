//! Minimal EXIF block (little-endian TIFF structure) for exported files: camera, lens,
//! exposure, capture date, author and copyright. No GPS, no maker notes.

use crate::raw::CaptureInfo;

/// What goes into the exported file's metadata.
#[derive(Clone, Debug, Default)]
pub struct ExifFields<'a> {
    /// Capture data (camera, lens, exposure, date); `None` writes only author and copyright.
    pub capture: Option<&'a CaptureInfo>,
    pub artist: &'a str,
    pub copyright: &'a str,
}

enum Value {
    Ascii(String),
    Short(u16),
    Long(u32),
    Rational(u32, u32),
    Undefined(Vec<u8>),
}

impl Value {
    fn kind_count_bytes(&self) -> (u16, u32, Vec<u8>) {
        match self {
            Value::Ascii(s) => {
                let mut b = s.as_bytes().to_vec();
                b.push(0);
                (2, b.len() as u32, b)
            }
            Value::Short(v) => (3, 1, v.to_le_bytes().to_vec()),
            Value::Long(v) => (4, 1, v.to_le_bytes().to_vec()),
            Value::Rational(n, d) => (5, 1, [n.to_le_bytes(), d.to_le_bytes()].concat()),
            Value::Undefined(b) => (7, b.len() as u32, b.clone()),
        }
    }
}

/// Rational approximation with a denominator that keeps typical values exact.
fn rational(v: f64, denominator: u32) -> Value {
    Value::Rational((v * denominator as f64).round().max(0.0) as u32, denominator)
}

/// The TIFF-structured EXIF data (without the JPEG "Exif\0\0" prefix), or empty if there is nothing to write.
pub fn build(f: &ExifFields) -> Vec<u8> {
    let ascii = |s: &str| (!s.trim().is_empty()).then(|| Value::Ascii(s.trim().to_string()));
    let mut ifd0: Vec<(u16, Value)> = Vec::new();
    let mut sub: Vec<(u16, Value)> = Vec::new();
    if let Some(c) = f.capture {
        ifd0.extend(ascii(&c.make).map(|v| (0x010F, v)));
        ifd0.extend(ascii(&c.model).map(|v| (0x0110, v)));
        if let Some(date) = c.date.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
            ifd0.push((0x0132, Value::Ascii(date.into())));
            sub.push((0x9003, Value::Ascii(date.into())));
        }
        if let Some((n, d)) = c.exposure_rational {
            sub.push((0x829A, Value::Rational(n, d)));
        }
        if let Some(n) = c.fnumber.filter(|n| *n > 0.0) {
            sub.push((0x829D, rational(n as f64, 10)));
        }
        if let Some(iso) = c.iso {
            sub.push((0x8827, Value::Short(iso.min(u16::MAX as u32) as u16)));
        }
        if let Some(mm) = c.focal_mm.filter(|mm| *mm > 0.0) {
            sub.push((0x920A, rational(mm as f64, 10)));
        }
        if let Some(lens) = c.lens.as_deref().and_then(ascii) {
            sub.push((0xA434, lens));
        }
    }
    ifd0.push((0x0131, Value::Ascii("Silberkorn".into())));
    ifd0.extend(ascii(f.artist).map(|v| (0x013B, v)));
    ifd0.extend(ascii(f.copyright).map(|v| (0x8298, v)));
    if ifd0.len() == 1 && sub.is_empty() {
        return Vec::new();
    }
    if !sub.is_empty() {
        sub.push((0x9000, Value::Undefined(b"0231".to_vec())));
    }

    // Layout: header, IFD0, its data, Exif IFD, its data.
    let ifd_len = |n: usize| 2 + n * 12 + 4;
    let has_sub = !sub.is_empty();
    if has_sub {
        ifd0.push((0x8769, Value::Long(0)));
    }
    ifd0.sort_by_key(|(t, _)| *t);
    sub.sort_by_key(|(t, _)| *t);
    let data_len = |entries: &[(u16, Value)]| entries.iter().map(|(_, v)| v.kind_count_bytes().2.len()).filter(|n| *n > 4).map(|n| n + n % 2).sum::<usize>();
    let ifd0_at = 8;
    let sub_at = ifd0_at + ifd_len(ifd0.len()) + data_len(&ifd0);
    if has_sub && let Some((_, v)) = ifd0.iter_mut().find(|(t, _)| *t == 0x8769) {
        *v = Value::Long(sub_at as u32);
    }
    let mut out = b"II*\0".to_vec();
    out.extend((ifd0_at as u32).to_le_bytes());
    write_ifd(&mut out, &ifd0);
    if has_sub {
        debug_assert_eq!(out.len(), sub_at);
        write_ifd(&mut out, &sub);
    }
    out
}

/// One IFD at the end of `out`, its larger values right behind it, no next IFD.
fn write_ifd(out: &mut Vec<u8>, entries: &[(u16, Value)]) {
    let start = out.len();
    let mut data_at = start + 2 + entries.len() * 12 + 4;
    let mut data = Vec::new();
    out.extend((entries.len() as u16).to_le_bytes());
    for (tag, v) in entries {
        let (kind, count, mut bytes) = v.kind_count_bytes();
        out.extend(tag.to_le_bytes());
        out.extend(kind.to_le_bytes());
        out.extend(count.to_le_bytes());
        if bytes.len() <= 4 {
            bytes.resize(4, 0);
            out.extend(bytes);
        } else {
            out.extend((data_at as u32).to_le_bytes());
            if bytes.len() % 2 == 1 {
                bytes.push(0);
            }
            data_at += bytes.len();
            data.extend(bytes);
        }
    }
    out.extend(0u32.to_le_bytes());
    out.extend(data);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads the IFD at `at`: (tag, type, count, value or offset).
    fn entries(b: &[u8], at: usize) -> Vec<(u16, u16, u32, u32)> {
        let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        (0..u16_at(at) as usize).map(|i| at + 2 + i * 12).map(|e| (u16_at(e), u16_at(e + 2), u32_at(e + 4), u32_at(e + 8))).collect()
    }

    fn ascii_at(b: &[u8], (_, _, count, off): (u16, u16, u32, u32)) -> String {
        let s = if count <= 4 { off.to_le_bytes()[..count as usize].to_vec() } else { b[off as usize..(off + count) as usize].to_vec() };
        String::from_utf8(s).unwrap().trim_end_matches('\0').to_string()
    }

    #[test]
    fn writes_capture_data_and_copyright() {
        let info = CaptureInfo {
            make: "Sony".into(),
            model: "ILCE-7M3".into(),
            lens: Some("FE 24-70mm F2.8 GM".into()),
            iso: Some(400),
            exposure_rational: Some((1, 50)),
            fnumber: Some(4.0),
            focal_mm: Some(32.0),
            date: Some("2024:03:01 10:20:30".into()),
            ..Default::default()
        };
        let b = build(&ExifFields { capture: Some(&info), artist: "M. Schneider", copyright: "© 2026 M. Schneider" });
        assert_eq!(&b[..4], b"II*\0");
        let ifd0 = entries(&b, 8);
        assert!(ifd0.windows(2).all(|w| w[0].0 < w[1].0), "Tags aufsteigend");
        let get = |es: &[(u16, u16, u32, u32)], t| *es.iter().find(|e| e.0 == t).unwrap();
        assert_eq!(ascii_at(&b, get(&ifd0, 0x010F)), "Sony");
        assert_eq!(ascii_at(&b, get(&ifd0, 0x8298)), "© 2026 M. Schneider");
        let sub = entries(&b, get(&ifd0, 0x8769).3 as usize);
        assert_eq!(get(&sub, 0x8827).3, 400);
        assert_eq!(ascii_at(&b, get(&sub, 0xA434)), "FE 24-70mm F2.8 GM");
        let off = get(&sub, 0x829A).3 as usize;
        assert_eq!((u32::from_le_bytes(b[off..off + 4].try_into().unwrap()), u32::from_le_bytes(b[off + 4..off + 8].try_into().unwrap())), (1, 50));
    }

    #[test]
    fn without_capture_data_only_author_fields() {
        let b = build(&ExifFields { capture: None, artist: "", copyright: "" });
        assert!(b.is_empty());
        let b = build(&ExifFields { capture: None, artist: "", copyright: "CC BY" });
        let tags: Vec<u16> = entries(&b, 8).iter().map(|e| e.0).collect();
        assert_eq!(tags, [0x0131, 0x8298]);
    }
}
