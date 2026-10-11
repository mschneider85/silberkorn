//! On-disk thumbnail cache: one JPEG per image, plus a small `.key` file recording the file
//! identity and adjustments it was made from.
//!
//! Images inside a session are cached in the session itself (`<Session>/Silberkorn/Cache/Thumbnails`),
//! named after their path relative to the session, so the cache travels with the session.
//! All other images use the per-user cache.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::params::Adjustments;

pub fn cache_dir() -> PathBuf {
    crate::paths::cache_dir().join("thumbs")
}

/// FNV-1a, stable across builds (unlike `DefaultHasher`).
pub fn fnv1a(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// Validity key: modification time (whole seconds, comparable across platforms), size and adjustments.
pub fn thumb_key(path: &Path, adj: &Adjustments) -> String {
    let meta = std::fs::metadata(path).ok();
    let mtime = meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    let size = meta.map_or(0, |m| m.len());
    let adj = if *adj == Adjustments::default() { String::new() } else { serde_json::to_string(adj).unwrap_or_default() };
    format!("{:016x}", fnv1a(format!("{mtime}|{size}|{adj}").as_bytes()))
}

/// Cache file of an image (without extension). `session` is the root of the open session, if any.
pub fn thumb_base(path: &Path, session: Option<&Path>) -> PathBuf {
    let (dir, id) = match session.and_then(|root| path.strip_prefix(root).ok().map(|rel| (root, rel))) {
        Some((root, rel)) => {
            let rel: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
            (crate::session::thumbnail_dir(root), rel.join("/"))
        }
        None => (cache_dir(), path.to_string_lossy().to_string()),
    };
    dir.join(format!("{:016x}", fnv1a(id.as_bytes())))
}

fn files(base: &Path) -> (PathBuf, PathBuf) {
    (base.with_extension("jpg"), base.with_extension("key"))
}

pub fn load(path: &Path, adj: &Adjustments, session: Option<&Path>) -> Option<::image::DynamicImage> {
    let (jpg, key) = files(&thumb_base(path, session));
    if std::fs::read_to_string(key).ok()? != thumb_key(path, adj) {
        return None;
    }
    ::image::open(jpg).ok()
}

pub fn store(path: &Path, adj: &Adjustments, session: Option<&Path>, img: &::image::DynamicImage) {
    let (jpg, key) = files(&thumb_base(path, session));
    let Some(dir) = jpg.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = jpg.with_extension("tmp");
    let Ok(file) = std::fs::File::create(&tmp) else { return };
    let mut enc = ::image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::BufWriter::new(file), 85);
    if enc.encode_image(&img.to_rgb8()).is_ok() && std::fs::rename(&tmp, &jpg).is_ok() {
        let _ = std::fs::write(key, thumb_key(path, adj));
    }
}

/// Follows an image that was moved (keeps its thumbnail).
pub fn rename(from: &Path, to: &Path, session: Option<&Path>) {
    let (a, b) = (files(&thumb_base(from, session)), files(&thumb_base(to, session)));
    if std::fs::rename(&a.0, &b.0).is_ok() {
        let _ = std::fs::rename(&a.1, &b.1);
    }
}

/// Drops the thumbnail of a deleted image.
pub fn remove(path: &Path, session: Option<&Path>) {
    let (jpg, key) = files(&thumb_base(path, session));
    let _ = std::fs::remove_file(jpg);
    let _ = std::fs::remove_file(key);
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

    #[test]
    fn session_thumbnails_travel_with_the_session() {
        let tmp = std::env::temp_dir().join(format!("skcache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let (old, new) = (tmp.join("A"), tmp.join("B"));
        std::fs::create_dir_all(old.join("Capture")).unwrap();
        let img = old.join("Capture").join("x.ARW");
        std::fs::write(&img, b"raw").unwrap();
        let thumb = ::image::DynamicImage::new_rgb8(4, 3);
        let adj = Adjustments::default();

        store(&img, &adj, Some(&old), &thumb);
        assert!(thumb_base(&img, Some(&old)).starts_with(crate::session::thumbnail_dir(&old)));
        assert!(load(&img, &adj, Some(&old)).is_some());
        assert!(load(&img, &Adjustments { exposure: 1.0, ..Default::default() }, Some(&old)).is_none());

        // Same relative path in the moved session: cache hit.
        std::fs::rename(&old, &new).unwrap();
        let moved = new.join("Capture").join("x.ARW");
        assert_eq!(load(&moved, &adj, Some(&new)).unwrap().width(), 4);

        // Moving the image within the session keeps its thumbnail; deleting drops it.
        std::fs::create_dir_all(new.join("Selects")).unwrap();
        let sel = new.join("Selects").join("x.ARW");
        std::fs::rename(&moved, &sel).unwrap();
        rename(&moved, &sel, Some(&new));
        assert!(load(&sel, &adj, Some(&new)).is_some());
        remove(&sel, Some(&new));
        assert!(load(&sel, &adj, Some(&new)).is_none());
        std::fs::remove_dir_all(tmp).ok();
    }
}
