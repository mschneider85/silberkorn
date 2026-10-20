//! A look at the TIFF structure of a raw file before rawler reads it. rawler trusts a few values
//! of damaged files that end the process, not with a panic but with an abort, which nothing can
//! catch: an IFD chain looping back loops (and allocates) forever, a tag count in the billions is
//! allocated before its data is read, and image sizes are allocated as given.
//!
//! Covers what rawler reads of TIFF-based raws (ARW, NEF, CR2, DNG, ORF, RW2, PEF, …): IFD0, its
//! chain, SubIFDs and the EXIF IFD. Maker notes are not checked; other formats pass unchecked.

/// IFDs checked at most; beyond that the file is let through (rawler stops by itself then).
const MAX_IFDS: usize = 256;
/// Largest image side and pixel count accepted (the largest sensors have about 20000 px).
const MAX_SIDE: u64 = 1 << 17;
const MAX_PIXELS: u64 = 2 << 30;

/// A tag may hold more data than the file only up to this (its allocation does no harm; refusing
/// it could refuse files rawler reads by skipping the tag).
const MAX_TAG_BYTES: u64 = 256 << 20;
/// Bytes per value of each TIFF type, as rawler counts them (unknown types as bytes).
const TYPE_SIZE: [u64; 14] = [1, 1, 1, 2, 4, 8, 1, 1, 2, 4, 8, 4, 8, 4];

/// `Err` with the reason when rawler must not get the file.
pub fn check(data: &[u8]) -> Result<(), String> {
    let Some(t) = Tiff::new(data) else { return Ok(()) };
    let mut todo = vec![t.u32(4) as usize];
    let mut checked = 0;
    while let Some(start) = todo.pop() {
        // One chain: the IFDs its `next` pointers lead to must not come round again.
        let mut chain = Vec::new();
        let mut offset = start;
        while offset != 0 && offset < data.len() {
            if chain.contains(&offset) {
                return Err("Verweise in der Datei laufen im Kreis".into());
            }
            checked += 1;
            if checked > MAX_IFDS {
                return Ok(());
            }
            chain.push(offset);
            let Some(ifd) = t.ifd(offset)? else { break };
            todo.extend(ifd.sub_ifds);
            offset = ifd.next;
        }
    }
    Ok(())
}

struct Tiff<'a> {
    data: &'a [u8],
    little: bool,
}

struct Ifd {
    next: usize,
    sub_ifds: Vec<usize>,
}

impl<'a> Tiff<'a> {
    fn new(data: &'a [u8]) -> Option<Self> {
        let little = match data.get(..2)? {
            b"II" => true,
            b"MM" => false,
            _ => return None,
        };
        let t = Tiff { data, little };
        // 42 for TIFF; ORF (0x4f52, 0x5352) and RW2 (0x55) use their own magic numbers.
        matches!(t.u16(2), 42 | 0x4f52 | 0x5352 | 0x55).then_some(t)
    }

    fn u16(&self, at: usize) -> u16 {
        let b = self.data.get(at..at + 2).map_or([0; 2], |b| [b[0], b[1]]);
        if self.little { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) }
    }

    fn u32(&self, at: usize) -> u32 {
        let b = self.data.get(at..at + 4).map_or([0; 4], |b| [b[0], b[1], b[2], b[3]]);
        if self.little { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }
    }

    /// The IFD at `offset`; `None` if it lies beyond the end (rawler reports that as an error).
    fn ifd(&self, offset: usize) -> Result<Option<Ifd>, String> {
        let len = self.data.len() as u64;
        let count = self.u16(offset) as usize;
        let end = offset + 2 + count * 12;
        if end + 4 > self.data.len() {
            return Ok(None);
        }
        let mut ifd = Ifd { next: self.u32(end) as usize, sub_ifds: Vec::new() };
        let (mut width, mut height) = (0u64, 0u64);
        for e in (offset + 2..end).step_by(12) {
            let (tag, kind, n) = (self.u16(e), self.u16(e + 2), self.u32(e + 4) as u64);
            let size = TYPE_SIZE.get(kind as usize).copied().unwrap_or(1);
            if n * size > len.max(MAX_TAG_BYTES) {
                return Err(format!("Eintrag {tag:#06x} ist größer als die Datei"));
            }
            let value = match kind {
                3 => self.u16(e + 8) as u64,
                4 | 13 => self.u32(e + 8) as u64,
                _ => continue,
            };
            match tag {
                0x0100 if n == 1 => width = value,
                0x0101 if n == 1 => height = value,
                // EXIF IFD, single SubIFD
                0x8769 | 0x014a if n == 1 => ifd.sub_ifds.push(value as usize),
                0x014a => {
                    // Several SubIFDs: a list of offsets (its size was checked above).
                    let at = value as usize;
                    ifd.sub_ifds.extend((0..n.min(64) as usize).map(|i| self.u32(at + 4 * i) as usize));
                }
                _ => {}
            }
        }
        if width > MAX_SIDE || height > MAX_SIDE || width * height > MAX_PIXELS {
            return Err(format!("Unplausible Bildgröße {width}×{height}"));
        }
        Ok(Some(ifd))
    }
}

#[cfg(test)]
mod tests {
    use super::check;

    /// A little-endian TIFF with one IFD at 8 holding `entries` (tag, type, count, value) and
    /// pointing to `next`.
    fn tiff(entries: &[(u16, u16, u32, u32)], next: u32) -> Vec<u8> {
        let mut f = b"II*\0".to_vec();
        f.extend(8u32.to_le_bytes());
        f.extend((entries.len() as u16).to_le_bytes());
        for &(tag, kind, n, v) in entries {
            f.extend(tag.to_le_bytes());
            f.extend(kind.to_le_bytes());
            f.extend(n.to_le_bytes());
            f.extend(v.to_le_bytes());
        }
        f.extend(next.to_le_bytes());
        f.resize(f.len() + 64, 0);
        f
    }

    #[test]
    fn plain_file_passes() {
        assert_eq!(check(&tiff(&[(0x100, 4, 1, 6000), (0x101, 4, 1, 4000), (0x10f, 2, 8, 100)], 0)), Ok(()));
        // Not TIFF-based: not checked.
        assert_eq!(check(b"FUJIFILMCCD-RAW 0201"), Ok(()));
        assert_eq!(check(&[]), Ok(()));
    }

    /// The sample raws pass (skipped without them).
    #[test]
    fn samples_pass() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples");
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if crate::raw::is_raw_file(&path) {
                assert_eq!(check(&std::fs::read(&path).unwrap()), Ok(()), "{}", path.display());
            }
        }
    }

    #[test]
    fn looping_chain_is_refused() {
        assert!(check(&tiff(&[(0x10f, 2, 4, 0)], 8)).is_err());
    }

    #[test]
    fn subifd_pointing_back_is_no_loop() {
        // rawler reads a SubIFD once, it does not follow it again from there.
        assert_eq!(check(&tiff(&[(0x14a, 4, 1, 8)], 0)), Ok(()));
    }

    #[test]
    fn huge_count_is_refused() {
        // A flipped high bit: 268 million doubles, 2 GB.
        assert!(check(&tiff(&[(0x829a, 12, 0x1000_0000, 100)], 0)).is_err());
        // 16 million doubles are only allocated in vain.
        assert_eq!(check(&tiff(&[(0x829a, 12, 0x0100_0000, 100)], 0)), Ok(()));
    }

    #[test]
    fn huge_image_is_refused() {
        assert!(check(&tiff(&[(0x100, 4, 1, 100_000), (0x101, 4, 1, 100_000)], 0)).is_err());
    }
}
