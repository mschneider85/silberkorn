//! Writes linear DNG files (demosaiced camera RGB, 16-bit floating point, Deflate-compressed
//! tiles), as produced by HDR merge, panorama stitching and denoising. Like other merged DNGs
//! they stay raw files: white balance, color and tone remain fully editable.
//!
//! Layout: IFD 0 holds an 8-bit RGB preview and the DNG metadata; the image itself is in a
//! SubIFD (LinearRaw, floating point with predictor 3).

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use rayon::prelude::*;

use crate::color::{Mat3, mat_inv};
use crate::raw::CaptureInfo;

/// What goes into the file.
pub struct LinearDng<'a> {
    pub width: usize,
    pub height: usize,
    /// Camera RGB, linear, oriented, white balance not applied. 1.0 is the white level of the
    /// reference exposure; brighter values are kept.
    pub data: &'a [[f32; 3]],
    /// XYZ (D65) -> camera matrix, as in `CameraColor`.
    pub xyz_to_cam: Mat3,
    /// As-shot camera multipliers (green = 1).
    pub as_shot_wb: [f32; 3],
    pub info: &'a CaptureInfo,
    /// Exposure correction a reader should apply by default (EV).
    pub baseline_exposure: f32,
    /// Preview shown by browsers (8-bit sRGB, any size).
    pub preview: Option<&'a ::image::RgbImage>,
}

const TILE: usize = 256;

enum Val {
    Byte(Vec<u8>),
    Ascii(String),
    Short(Vec<u16>),
    Long(Vec<u32>),
    Rational(Vec<(u32, u32)>),
    SRational(Vec<(i32, i32)>),
    /// Offset to be filled in (LONG), one per entry.
    Offsets(usize),
}

impl Val {
    fn type_count(&self) -> (u16, u32) {
        match self {
            Val::Byte(v) => (1, v.len() as u32),
            Val::Ascii(s) => (2, s.len() as u32 + 1),
            Val::Short(v) => (3, v.len() as u32),
            Val::Long(v) => (4, v.len() as u32),
            Val::Rational(v) => (5, v.len() as u32),
            Val::SRational(v) => (10, v.len() as u32),
            Val::Offsets(n) => (4, *n as u32),
        }
    }

    fn bytes(&self, offsets: &[u32]) -> Vec<u8> {
        match self {
            Val::Byte(v) => v.clone(),
            Val::Ascii(s) => {
                let mut b = s.as_bytes().to_vec();
                b.push(0);
                b
            }
            Val::Short(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
            Val::Long(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
            Val::Rational(v) => v.iter().flat_map(|(n, d)| [n.to_le_bytes(), d.to_le_bytes()].concat()).collect(),
            Val::SRational(v) => v.iter().flat_map(|(n, d)| [n.to_le_bytes(), d.to_le_bytes()].concat()).collect(),
            Val::Offsets(_) => offsets.iter().flat_map(|x| x.to_le_bytes()).collect(),
        }
    }
}

/// A directory being written: its entries (sorted on output) and the data blocks its offset
/// entries point to.
#[derive(Default)]
struct Ifd {
    entries: Vec<(u16, Val)>,
    /// For the `Offsets` entry with this tag: the blocks it points to.
    blocks: Vec<(u16, Vec<Vec<u8>>)>,
}

impl Ifd {
    fn add(&mut self, tag: u16, v: Val) {
        self.entries.push((tag, v));
    }
}

/// Little-endian TIFF writer: places directories and their data one after another.
struct TiffOut {
    buf: Vec<u8>,
}

impl TiffOut {
    fn new() -> Self {
        let mut buf = b"II".to_vec();
        buf.extend(42u16.to_le_bytes());
        buf.extend(0u32.to_le_bytes()); // offset of IFD 0, patched
        Self { buf }
    }

    fn align(&mut self) {
        if self.buf.len() % 2 == 1 {
            self.buf.push(0);
        }
    }

    /// Writes the data blocks and the directory; returns the directory's offset.
    fn write_ifd(&mut self, mut ifd: Ifd) -> u32 {
        // Data blocks first, so their offsets are known.
        let mut offsets: Vec<(u16, Vec<u32>, Vec<u32>)> = Vec::new();
        for (tag, blocks) in std::mem::take(&mut ifd.blocks) {
            let mut offs = Vec::new();
            let mut counts = Vec::new();
            for b in blocks {
                self.align();
                offs.push(self.buf.len() as u32);
                counts.push(b.len() as u32);
                self.buf.extend(b);
            }
            offsets.push((tag, offs, counts));
        }
        // Byte counts go with their offsets (TileOffsets/TileByteCounts, StripOffsets/StripByteCounts).
        for (tag, _, counts) in &offsets {
            let count_tag = match *tag {
                324 => 325,
                273 => 279,
                _ => continue,
            };
            ifd.add(count_tag, Val::Long(counts.clone()));
        }
        ifd.entries.sort_by_key(|(t, _)| *t);
        // Values that don't fit into an entry go before the directory.
        let mut out_values: Vec<Vec<u8>> = Vec::new();
        let mut value_offsets: Vec<Option<u32>> = Vec::new();
        for (tag, v) in &ifd.entries {
            let offs = offsets.iter().find(|(t, ..)| t == tag).map(|(_, o, _)| o.clone()).unwrap_or_default();
            let bytes = v.bytes(&offs);
            if bytes.len() > 4 {
                self.align();
                value_offsets.push(Some(self.buf.len() as u32));
                self.buf.extend(&bytes);
            } else {
                value_offsets.push(None);
            }
            out_values.push(bytes);
        }
        self.align();
        let at = self.buf.len() as u32;
        self.buf.extend((ifd.entries.len() as u16).to_le_bytes());
        for (((tag, v), bytes), off) in ifd.entries.iter().zip(&out_values).zip(&value_offsets) {
            let (typ, count) = v.type_count();
            self.buf.extend(tag.to_le_bytes());
            self.buf.extend(typ.to_le_bytes());
            self.buf.extend(count.to_le_bytes());
            match off {
                Some(o) => self.buf.extend(o.to_le_bytes()),
                None => {
                    let mut b = bytes.clone();
                    b.resize(4, 0);
                    self.buf.extend(b);
                }
            }
        }
        self.buf.extend(0u32.to_le_bytes()); // next IFD
        at
    }
}

fn srational(v: f32) -> (i32, i32) {
    ((v as f64 * 10000.0).round() as i32, 10000)
}

fn rational(v: f64, d: u32) -> (u32, u32) {
    ((v * d as f64).round().max(0.0) as u32, d)
}

/// One tile: 16-bit floats, byte planes per row, horizontal byte differences (predictor 3),
/// zlib-compressed.
fn encode_tile(data: &[[f32; 3]], width: usize, height: usize, tx: usize, ty: usize) -> Vec<u8> {
    let line = TILE * 3;
    let mut raw = Vec::with_capacity(line * 2 * TILE);
    let mut row = vec![0u8; line * 2];
    for y in 0..TILE {
        let sy = (ty * TILE + y).min(height - 1);
        for x in 0..TILE {
            let sx = (tx * TILE + x).min(width - 1);
            let p = data[sy * width + sx];
            for c in 0..3 {
                let h = half::f16::from_f32(p[c].max(0.0)).to_bits().to_be_bytes();
                let col = x * 3 + c;
                row[col] = h[0];
                row[col + line] = h[1];
            }
        }
        for i in (3..row.len()).rev() {
            row[i] = row[i].wrapping_sub(row[i - 3]);
        }
        raw.extend_from_slice(&row);
    }
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    enc.write_all(&raw).expect("in memory");
    enc.finish().expect("in memory")
}

/// Writes a linear DNG file.
pub fn write_linear_dng(path: &Path, d: &LinearDng) -> Result<()> {
    anyhow::ensure!(d.width > 0 && d.height > 0 && d.data.len() == d.width * d.height, "Ungültige Bildgröße");
    let info = d.info;
    let mut out = TiffOut::new();

    // Image tiles.
    let (cols, rows) = (d.width.div_ceil(TILE), d.height.div_ceil(TILE));
    let tiles: Vec<Vec<u8>> = (0..cols * rows).into_par_iter().map(|i| encode_tile(d.data, d.width, d.height, i % cols, i / cols)).collect();
    let mut raw = Ifd::default();
    raw.add(254, Val::Long(vec![0]));
    raw.add(256, Val::Long(vec![d.width as u32]));
    raw.add(257, Val::Long(vec![d.height as u32]));
    raw.add(258, Val::Short(vec![16; 3]));
    raw.add(259, Val::Short(vec![8]));
    raw.add(262, Val::Short(vec![34892]));
    raw.add(277, Val::Short(vec![3]));
    raw.add(284, Val::Short(vec![1]));
    raw.add(317, Val::Short(vec![3]));
    raw.add(322, Val::Long(vec![TILE as u32]));
    raw.add(323, Val::Long(vec![TILE as u32]));
    raw.add(324, Val::Offsets(tiles.len()));
    raw.add(339, Val::Short(vec![3; 3]));
    raw.add(50717, Val::Long(vec![1; 3])); // WhiteLevel: floats are normalized
    raw.blocks.push((324, tiles));
    let raw_at = out.write_ifd(raw);

    // EXIF directory.
    let mut exif = Ifd::default();
    if let Some((n, den)) = info.exposure_rational {
        exif.add(33434, Val::Rational(vec![(n, den)]));
    }
    if let Some(f) = info.fnumber {
        exif.add(33437, Val::Rational(vec![rational(f as f64, 10)]));
    }
    if let Some(iso) = info.iso {
        exif.add(34855, Val::Short(vec![iso.min(65535) as u16]));
    }
    if let Some(date) = &info.date {
        exif.add(36867, Val::Ascii(date.clone()));
    }
    if let Some(f) = info.focal_mm {
        exif.add(37386, Val::Rational(vec![rational(f as f64, 10)]));
    }
    if let Some(l) = &info.lens {
        exif.add(42036, Val::Ascii(l.clone()));
    }
    let exif_at = (!exif.entries.is_empty()).then(|| out.write_ifd(exif));

    // IFD 0: preview and DNG metadata.
    let mut ifd0 = Ifd::default();
    ifd0.add(254, Val::Long(vec![1]));
    let preview = d.preview.cloned().unwrap_or_else(|| ::image::RgbImage::new(1, 1));
    let (pw, ph) = (preview.width(), preview.height());
    ifd0.add(256, Val::Long(vec![pw]));
    ifd0.add(257, Val::Long(vec![ph]));
    ifd0.add(258, Val::Short(vec![8; 3]));
    ifd0.add(259, Val::Short(vec![1]));
    ifd0.add(262, Val::Short(vec![2]));
    ifd0.add(271, Val::Ascii(if info.make.is_empty() { "Unknown".into() } else { info.make.clone() }));
    ifd0.add(272, Val::Ascii(if info.model.is_empty() { "Unknown".into() } else { info.model.clone() }));
    ifd0.add(273, Val::Offsets(1));
    ifd0.add(274, Val::Short(vec![1]));
    ifd0.add(277, Val::Short(vec![3]));
    ifd0.add(278, Val::Long(vec![ph]));
    ifd0.add(284, Val::Short(vec![1]));
    ifd0.add(305, Val::Ascii(format!("Silberkorn {}", env!("CARGO_PKG_VERSION"))));
    if let Some(date) = &info.date {
        ifd0.add(306, Val::Ascii(date.clone()));
    }
    ifd0.add(330, Val::Long(vec![raw_at]));
    if let Some(e) = exif_at {
        ifd0.add(34665, Val::Long(vec![e]));
    }
    ifd0.add(50706, Val::Byte(vec![1, 4, 0, 0]));
    ifd0.add(50707, Val::Byte(vec![1, 4, 0, 0]));
    ifd0.add(50708, Val::Ascii(format!("{} {}", info.make, info.model).trim().to_string()));
    let m = d.xyz_to_cam;
    ifd0.add(50721, Val::SRational(m.iter().flatten().map(|v| srational(*v)).collect()));
    // As-shot neutral: the camera values of white, i.e. the inverse of the multipliers.
    let wb = d.as_shot_wb;
    ifd0.add(50728, Val::Rational(wb.iter().map(|v| rational(1.0 / v.max(1e-4) as f64, 1_000_000)).collect()));
    ifd0.add(50730, Val::SRational(vec![srational(d.baseline_exposure)]));
    ifd0.add(50778, Val::Short(vec![21])); // D65
    ifd0.blocks.push((273, vec![preview.into_raw()]));
    let ifd0_at = out.write_ifd(ifd0);
    out.buf[4..8].copy_from_slice(&ifd0_at.to_le_bytes());
    anyhow::ensure!(mat_inv(&m).is_some(), "Farbmatrix nicht invertierbar");

    let tmp = path.with_extension("dng.part");
    std::fs::write(&tmp, &out.buf).with_context(|| format!("{} kann nicht geschrieben werden", path.display()))?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Small sRGB preview of camera data (for the file's embedded preview).
pub fn quick_preview(meta: &crate::raw::RawMeta, img: &crate::image::LinearImage, max_edge: usize) -> ::image::RgbImage {
    let small = img.clone().fit_within(max_edge);
    let r = crate::pipeline::Processor::new(meta, &crate::params::Adjustments::default()).render(&small, None, None);
    let rgb: Vec<u8> = r.rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    ::image::RgbImage::from_raw(r.width as u32, r.height as u32, rgb).expect("size")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_dng_reads_back_with_values_above_white() {
        let (w, h) = (300, 200);
        let data: Vec<[f32; 3]> = (0..w * h).map(|i| {
            let (x, y) = (i % w, i / w);
            [x as f32 / w as f32 * 4.0, y as f32 / h as f32, 0.25]
        }).collect();
        let info = CaptureInfo { make: "Testkamera".into(), model: "X1".into(), iso: Some(200), date: Some("2024:05:01 12:00:00".into()), exposure_rational: Some((1, 100)), ..Default::default() };
        let xyz_to_cam = [[0.7, 0.2, -0.1], [-0.3, 1.2, 0.1], [0.0, 0.1, 0.8]];
        let dir = std::env::temp_dir().join(format!("sk-dng-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.dng");
        let preview = ::image::RgbImage::from_pixel(30, 20, ::image::Rgb([200, 100, 50]));
        write_linear_dng(&path, &LinearDng { width: w, height: h, data: &data, xyz_to_cam, as_shot_wb: [2.0, 1.0, 1.5], info: &info, baseline_exposure: 0.0, preview: Some(&preview) }).unwrap();
        let (meta, img) = crate::raw::load_full(&path).unwrap();
        assert_eq!((img.width, img.height), (w, h));
        assert!(meta.hdr, "Float-Daten ohne Clipping");
        assert_eq!(meta.info.iso, Some(200));
        assert_eq!(meta.info.date.as_deref(), Some("2024:05:01 12:00:00"));
        assert!((meta.as_shot_wb[0] - 2.0).abs() < 0.01 && (meta.as_shot_wb[2] - 1.5).abs() < 0.01, "{:?}", meta.as_shot_wb);
        for (i, (a, b)) in img.data.iter().zip(&data).enumerate().step_by(997) {
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() <= b[c].abs() * 0.002 + 1e-3, "{i}: {a:?} vs {b:?}");
            }
        }
        let (_, small) = crate::raw::load_preview(&path, 100).unwrap();
        assert_eq!(small.width, 100);
        assert!(small.data.iter().any(|p| p[0] > 2.0));
        let thumb = crate::raw::embedded_preview(&path).unwrap();
        assert_eq!((thumb.width(), thumb.height()), (30, 20));
        std::fs::remove_dir_all(dir).ok();
    }
}
