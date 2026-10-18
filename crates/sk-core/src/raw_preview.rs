//! The embedded JPEG of a raw file without reading the whole file: only the TIFF structure
//! (a few kilobytes) and the JPEG itself are read. Over a network share or from a USB drive this
//! is many times faster than loading 30–60 MB per thumbnail.
//!
//! Covers TIFF-based raws (ARW, NEF, CR2, DNG, ORF, RW2, PEF, …) and Fujifilm RAF. Anything else,
//! or anything unexpected, returns `None` and the caller decodes the file the usual way.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// IFDs followed at most (IFD chains and SubIFDs of hostile files may loop).
const MAX_IFDS: usize = 32;
/// Entries read per IFD at most.
const MAX_ENTRIES: usize = 512;
/// Largest embedded JPEG accepted.
const MAX_JPEG: u64 = 64 << 20;
/// Smaller JPEGs are thumbnails, not previews.
const MIN_JPEG: u64 = 32 << 10;

/// The largest embedded JPEG (its bytes) and the EXIF orientation of the image (1..8).
pub fn read(path: &Path) -> Option<(Vec<u8>, u16)> {
    read_at_least(path, usize::MAX)
}

/// Like [`read`], but the smallest embedded JPEG whose longer edge has at least `min_edge`
/// pixels (the largest if none has): many raws hold a medium preview besides the full-size
/// one, and decoding that is many times faster for a thumbnail.
pub fn read_at_least(path: &Path, min_edge: usize) -> Option<(Vec<u8>, u16)> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut head = [0u8; 16];
    f.read_exact(&mut head).ok()?;
    if head.starts_with(b"FUJIFILMCCD-RAW") {
        return raf(&mut f, len);
    }
    let tiff = Tiff::at(&mut f, 0, len)?;
    let found = tiff.scan(&mut f)?;
    let (mut offset, mut size) = found.jpeg?;
    if min_edge < usize::MAX {
        let mut smaller: Vec<(u64, u64)> = found.jpegs.iter().copied().filter(|&(_, l)| (MIN_JPEG..size).contains(&l)).collect();
        smaller.sort_by_key(|&(_, l)| l);
        if let Some(&(o, l)) = smaller.iter().find(|&&(o, l)| jpeg_size(&mut f, o, l).is_some_and(|(w, h)| w.max(h) >= min_edge)) {
            (offset, size) = (o, l);
        }
    }
    let jpeg = read_range(&mut f, offset, size)?;
    Some((jpeg, found.orientation.unwrap_or(1)))
}

/// Width and height of the JPEG at `offset` from its frame header (within its first 64 KB).
fn jpeg_size(f: &mut File, offset: u64, size: u64) -> Option<(usize, usize)> {
    let mut buf = vec![0u8; size.min(64 << 10) as usize];
    f.seek(SeekFrom::Start(offset)).ok()?;
    f.read_exact(&mut buf).ok()?;
    if !buf.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let mut i = 2usize;
    while i + 9 <= buf.len() && buf[i] == 0xff {
        let marker = buf[i + 1];
        let seg = u16::from_be_bytes([buf[i + 2], buf[i + 3]]) as usize;
        // Start of frame (baseline, progressive, …; not DHT, JPG or DAC).
        if matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
            let h = u16::from_be_bytes([buf[i + 5], buf[i + 6]]) as usize;
            let w = u16::from_be_bytes([buf[i + 7], buf[i + 8]]) as usize;
            return Some((w, h));
        }
        if marker == 0xda {
            return None;
        }
        i += 2 + seg;
    }
    None
}

/// RAF: a fixed header points to the embedded JPEG (offset and length, big-endian, at bytes
/// 84 and 88); the orientation is in the JPEG's own EXIF data.
fn raf(f: &mut File, len: u64) -> Option<(Vec<u8>, u16)> {
    let mut h = [0u8; 92];
    f.seek(SeekFrom::Start(0)).ok()?;
    f.read_exact(&mut h).ok()?;
    let offset = u32::from_be_bytes(h[84..88].try_into().ok()?) as u64;
    let size = u32::from_be_bytes(h[88..92].try_into().ok()?) as u64;
    if offset.checked_add(size)? > len {
        return None;
    }
    let jpeg = read_range(f, offset, size)?;
    let orientation = jpeg_orientation(&jpeg).unwrap_or(1);
    Some((jpeg, orientation))
}

fn read_range(f: &mut File, offset: u64, size: u64) -> Option<Vec<u8>> {
    if !(MIN_JPEG..=MAX_JPEG).contains(&size) {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    f.seek(SeekFrom::Start(offset)).ok()?;
    f.read_exact(&mut buf).ok()?;
    // A JPEG starts with SOI.
    buf.starts_with(&[0xff, 0xd8]).then_some(buf)
}

/// EXIF orientation inside a JPEG (APP1 "Exif" segment with a TIFF structure).
fn jpeg_orientation(jpeg: &[u8]) -> Option<u16> {
    let mut i = 2usize;
    while i + 4 <= jpeg.len() && jpeg[i] == 0xff {
        let marker = jpeg[i + 1];
        let seg = u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
        let body = jpeg.get(i + 4..i + 2 + seg)?;
        if marker == 0xe1 && body.starts_with(b"Exif\0\0") {
            let mut cur = std::io::Cursor::new(&body[6..]);
            let tiff = Tiff::at(&mut cur, 0, body.len() as u64 - 6)?;
            return tiff.scan(&mut cur)?.orientation;
        }
        // Image data follows: no EXIF before it.
        if marker == 0xda {
            return None;
        }
        i += 2 + seg;
    }
    None
}

/// A TIFF structure: byte order and the offset of IFD0, relative to `base`.
struct Tiff {
    base: u64,
    len: u64,
    little: bool,
    ifd0: u64,
}

#[derive(Default)]
struct Found {
    /// Largest JPEG: (absolute offset, length).
    jpeg: Option<(u64, u64)>,
    /// All JPEGs found.
    jpegs: Vec<(u64, u64)>,
    orientation: Option<u16>,
}

impl Tiff {
    fn at<R: Read + Seek>(r: &mut R, base: u64, len: u64) -> Option<Self> {
        let mut h = [0u8; 8];
        r.seek(SeekFrom::Start(base)).ok()?;
        r.read_exact(&mut h).ok()?;
        let little = match &h[..2] {
            b"II" => true,
            b"MM" => false,
            _ => return None,
        };
        let t = Tiff { base, len, little, ifd0: 0 };
        // 42 for TIFF; ORF (0x4f52, 0x5352) and RW2 (0x55) use their own magic numbers.
        if !matches!(t.u16(&h[2..4]), 42 | 0x4f52 | 0x5352 | 0x55) {
            return None;
        }
        Some(Tiff { ifd0: t.u32(&h[4..8]) as u64, ..t })
    }

    fn u16(&self, b: &[u8]) -> u16 {
        let b = [b[0], b[1]];
        if self.little { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) }
    }

    fn u32(&self, b: &[u8]) -> u32 {
        let b = [b[0], b[1], b[2], b[3]];
        if self.little { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }
    }

    /// Walks IFD0, its chain and the SubIFDs (and EXIF IFD for the orientation of embedded
    /// EXIF), collecting the largest JPEG and the orientation of IFD0.
    fn scan<R: Read + Seek>(&self, r: &mut R) -> Option<Found> {
        let mut found = Found::default();
        let mut todo = vec![(self.ifd0, true)];
        let mut seen = Vec::new();
        while let Some((offset, first)) = todo.pop() {
            if offset == 0 || offset >= self.len || seen.contains(&offset) || seen.len() >= MAX_IFDS {
                continue;
            }
            seen.push(offset);
            let Some(ifd) = self.ifd(r, offset) else { continue };
            if first && found.orientation.is_none() {
                found.orientation = ifd.orientation.filter(|o| (1..=8).contains(o));
            }
            for (start, size) in ifd.jpegs() {
                let start = self.base + start;
                if start.checked_add(size).is_some_and(|end| end <= self.base + self.len) {
                    found.jpegs.push((start, size));
                    if found.jpeg.is_none_or(|(_, best)| size > best) {
                        found.jpeg = Some((start, size));
                    }
                }
            }
            todo.extend(ifd.sub_ifds.iter().map(|o| (*o, false)));
            todo.push((ifd.next, first));
        }
        Some(found)
    }

    fn ifd<R: Read + Seek>(&self, r: &mut R, offset: u64) -> Option<Ifd> {
        let mut n = [0u8; 2];
        r.seek(SeekFrom::Start(self.base + offset)).ok()?;
        r.read_exact(&mut n).ok()?;
        let count = (self.u16(&n) as usize).min(MAX_ENTRIES);
        let mut entries = vec![0u8; count * 12 + 4];
        r.read_exact(&mut entries).ok()?;
        let mut ifd = Ifd { next: self.u32(&entries[count * 12..]) as u64, ..Default::default() };
        for e in entries[..count * 12].chunks_exact(12) {
            let (tag, kind, n) = (self.u16(&e[0..2]), self.u16(&e[2..4]), self.u32(&e[4..8]));
            // SHORT or LONG values that fit into the entry.
            let value = match kind {
                3 => self.u16(&e[8..10]) as u64,
                4 | 13 => self.u32(&e[8..12]) as u64,
                _ => continue,
            };
            match tag {
                0x0103 => ifd.compression = Some(value),
                0x0112 => ifd.orientation = Some(value as u16),
                0x0201 => ifd.jpeg_offset = Some(value),
                0x0202 => ifd.jpeg_length = Some(value),
                0x0111 if n == 1 => ifd.strip_offset = Some(value),
                0x0117 if n == 1 => ifd.strip_length = Some(value),
                0x014a => {
                    if n == 1 {
                        ifd.sub_ifds.push(value);
                    } else if let Some(list) = self.longs(r, value, n.min(8) as usize) {
                        ifd.sub_ifds.extend(list);
                    }
                }
                _ => {}
            }
        }
        Some(ifd)
    }

    fn longs<R: Read + Seek>(&self, r: &mut R, offset: u64, n: usize) -> Option<Vec<u64>> {
        let mut b = vec![0u8; n * 4];
        r.seek(SeekFrom::Start(self.base + offset)).ok()?;
        r.read_exact(&mut b).ok()?;
        Some(b.chunks_exact(4).map(|c| self.u32(c) as u64).collect())
    }
}

#[derive(Default)]
struct Ifd {
    next: u64,
    sub_ifds: Vec<u64>,
    compression: Option<u64>,
    orientation: Option<u16>,
    jpeg_offset: Option<u64>,
    jpeg_length: Option<u64>,
    strip_offset: Option<u64>,
    strip_length: Option<u64>,
}

impl Ifd {
    /// JPEGs this IFD points to: the JPEGInterchangeFormat pair, or a single JPEG strip
    /// (compression 6 or 7, as in DNG preview IFDs).
    fn jpegs(&self) -> Vec<(u64, u64)> {
        let mut v = Vec::new();
        if let (Some(o), Some(l)) = (self.jpeg_offset, self.jpeg_length) {
            v.push((o, l));
        }
        if matches!(self.compression, Some(6 | 7))
            && let (Some(o), Some(l)) = (self.strip_offset, self.strip_length)
        {
            v.push((o, l));
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A little-endian TIFF with IFD0 (orientation 6, a small JPEG) and a SubIFD (larger JPEG).
    fn sample(small: usize, large: usize) -> Vec<u8> {
        let jpeg = |n: usize| {
            let mut j = vec![0xff, 0xd8];
            j.resize(n, 0x55);
            j
        };
        let mut f = b"II*\0".to_vec();
        f.extend(8u32.to_le_bytes());
        // IFD0 at 8: 4 entries.
        let ifd0_len = 2 + 4 * 12 + 4;
        let sub_at = 8 + ifd0_len;
        let sub_len = 2 + 3 * 12 + 4;
        let small_at = sub_at + sub_len;
        let large_at = small_at + small;
        let entry = |f: &mut Vec<u8>, tag: u16, kind: u16, v: u32| {
            f.extend(tag.to_le_bytes());
            f.extend(kind.to_le_bytes());
            f.extend(1u32.to_le_bytes());
            f.extend(v.to_le_bytes());
        };
        f.extend(4u16.to_le_bytes());
        entry(&mut f, 0x0112, 3, 6);
        entry(&mut f, 0x014a, 4, sub_at as u32);
        entry(&mut f, 0x0201, 4, small_at as u32);
        entry(&mut f, 0x0202, 4, small as u32);
        f.extend(0u32.to_le_bytes());
        f.extend(3u16.to_le_bytes());
        entry(&mut f, 0x0103, 3, 7);
        entry(&mut f, 0x0111, 4, large_at as u32);
        entry(&mut f, 0x0117, 4, large as u32);
        f.extend(0u32.to_le_bytes());
        f.extend(jpeg(small));
        f.extend(jpeg(large));
        f
    }

    fn temp(name: &str, data: &[u8]) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("sk-rawprev-{name}-{}", std::process::id()));
        std::fs::write(&p, data).unwrap();
        p
    }

    /// A sample raw's medium preview is enough for a thumbnail, and it is smaller than the
    /// largest (skipped without the samples).
    #[test]
    fn smallest_preview_large_enough() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/_DSC0010.ARW");
        let Some((largest, _)) = read(&path) else { return };
        let (small, _) = read_at_least(&path, 400).unwrap();
        let size = |jpeg: &[u8]| ::image::load_from_memory(jpeg).map(|i| i.width().max(i.height())).unwrap();
        assert!(size(&small) >= 400);
        assert!(small.len() <= largest.len());
        eprintln!("Vorschau {} px ({} KB) statt {} px ({} KB)", size(&small), small.len() >> 10, size(&largest), largest.len() >> 10);
    }

    #[test]
    fn finds_the_largest_jpeg_and_the_orientation() {
        let data = sample(40_000, 90_000);
        let p = temp("tiff", &data);
        let (jpeg, orientation) = read(&p).unwrap();
        assert_eq!((jpeg.len(), orientation), (90_000, 6));
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn hostile_files_give_none_instead_of_panicking() {
        let good = sample(40_000, 90_000);
        // Truncated anywhere, and garbage at every position of the header.
        for cut in [0, 3, 8, 20, 60, 100, 40_000] {
            let p = temp("cut", &good[..cut.min(good.len())]);
            let _ = read(&p);
            std::fs::remove_file(p).ok();
        }
        for i in 0..120 {
            let mut bad = good.clone();
            bad[i] ^= 0xa5;
            let p = temp("bad", &bad);
            let _ = read(&p);
            std::fs::remove_file(p).ok();
        }
        // An IFD pointing to itself.
        let mut loop_ = b"II*\0".to_vec();
        loop_.extend(8u32.to_le_bytes());
        loop_.extend(0u16.to_le_bytes());
        loop_.extend(8u32.to_le_bytes());
        let p = temp("loop", &loop_);
        assert!(read(&p).is_none());
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn raf_header_points_to_the_jpeg() {
        let mut f = b"FUJIFILMCCD-RAW 0201FF383501".to_vec();
        f.resize(100, 0);
        let mut jpeg = vec![0xff, 0xd8];
        jpeg.resize(50_000, 1);
        f[84..88].copy_from_slice(&100u32.to_be_bytes());
        f[88..92].copy_from_slice(&(jpeg.len() as u32).to_be_bytes());
        f.extend(&jpeg);
        let p = temp("raf", &f);
        let (j, orientation) = read(&p).unwrap();
        assert_eq!((j.len(), orientation), (50_000, 1));
        std::fs::remove_file(p).ok();
    }
}
