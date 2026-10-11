//! On-disk thumbnail cache, keyed by file identity and adjustments.

use std::path::{Path, PathBuf};

use crate::params::Adjustments;

pub fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("capture-room").join("thumbs")
}

/// FNV-1a, stable across builds (unlike `DefaultHasher`).
pub fn fnv1a(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// Cache key: path, modification time, size and the adjustments in effect.
pub fn thumb_key(path: &Path, adj: &Adjustments) -> String {
    let meta = std::fs::metadata(path).ok();
    let mtime = meta.as_ref().and_then(|m| m.modified().ok()).map(|t| format!("{t:?}")).unwrap_or_default();
    let size = meta.map_or(0, |m| m.len());
    let adj = if *adj == Adjustments::default() { String::new() } else { serde_json::to_string(adj).unwrap_or_default() };
    let key = format!("{}|{mtime}|{size}|{adj}", path.display());
    format!("{:016x}", fnv1a(key.as_bytes()))
}

pub fn thumb_path(path: &Path, adj: &Adjustments) -> PathBuf {
    cache_dir().join(format!("{}.jpg", thumb_key(path, adj)))
}

pub fn load(path: &Path, adj: &Adjustments) -> Option<::image::DynamicImage> {
    ::image::open(thumb_path(path, adj)).ok()
}

pub fn store(path: &Path, adj: &Adjustments, img: &::image::DynamicImage) {
    let target = thumb_path(path, adj);
    if let Some(dir) = target.parent()
        && std::fs::create_dir_all(dir).is_ok()
    {
        let tmp = target.with_extension("tmp");
        let file = std::fs::File::create(&tmp).ok();
        if let Some(file) = file {
            let mut enc = ::image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::BufWriter::new(file), 85);
            if enc.encode_image(&img.to_rgb8()).is_ok() {
                let _ = std::fs::rename(&tmp, &target);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_changes_with_adjustments() {
        let p = Path::new("/nonexistent/a.ARW");
        let a = thumb_key(p, &Adjustments::default());
        let b = thumb_key(p, &Adjustments { exposure: 1.0, ..Default::default() });
        assert_ne!(a, b);
        assert_eq!(a, thumb_key(p, &Adjustments::default()));
    }
}
