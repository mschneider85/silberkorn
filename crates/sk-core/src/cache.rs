//! On-disk thumbnail and preview cache: one JPEG per image and kind, plus a small `.key` file
//! recording the file identity and adjustments it was made from.
//!
//! Images inside a session are cached in the session itself (`<Session>/Silberkorn/Cache/…`),
//! named after their path relative to the session, so the cache travels with the session.
//! All other images use the per-user cache.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::params::Adjustments;

/// What is cached: small browser thumbnails, or screen-sized previews for the viewer
/// (so that stepping through images shows them instantly).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Thumbnail,
    Preview,
}

impl Kind {
    const ALL: [Kind; 2] = [Kind::Thumbnail, Kind::Preview];

    fn user_dir(self) -> PathBuf {
        crate::paths::cache_dir().join(match self {
            Kind::Thumbnail => "thumbs",
            Kind::Preview => "previews",
        })
    }

    fn session_dir(self, root: &Path) -> PathBuf {
        match self {
            Kind::Thumbnail => crate::session::thumbnail_dir(root),
            Kind::Preview => crate::session::preview_dir(root),
        }
    }

    fn quality(self) -> u8 {
        match self {
            Kind::Thumbnail => 85,
            Kind::Preview => 90,
        }
    }
}

pub fn cache_dir() -> PathBuf {
    Kind::Thumbnail.user_dir()
}

/// FNV-1a, stable across builds (unlike `DefaultHasher`).
pub fn fnv1a(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// Identity of an image's file (for variants the raw file): path, size and modification time,
/// so data computed from it is not used once the file changes.
pub fn file_identity(path: &Path) -> Option<String> {
    let file = crate::variant::file_of(path);
    let file = file.canonicalize().unwrap_or(file);
    let md = std::fs::metadata(&file).ok()?;
    let mtime = md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    Some(format!("{}|{}|{mtime}", file.display(), md.len()))
}

/// Validity key: modification time (whole seconds, comparable across platforms), size and adjustments.
pub fn thumb_key(path: &Path, adj: &Adjustments) -> String {
    let meta = std::fs::metadata(crate::variant::file_of(path)).ok();
    let mtime = meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    let size = meta.map_or(0, |m| m.len());
    // AI denoising shows once its data has been computed.
    let denoised = if adj.detail.ai_denoise > 0.0 && crate::ai_denoise::is_ready(path) { "|dn" } else { "" };
    // So does the lens blur once the depth map exists.
    let blurred = if !adj.lens_blur.is_off() && crate::depth::is_ready(path) { "|lb" } else { "" };
    let adj = if *adj == Adjustments::default() { String::new() } else { serde_json::to_string(adj).unwrap_or_default() };
    format!("{:016x}", fnv1a(format!("{mtime}|{size}|{adj}{denoised}{blurred}").as_bytes()))
}

/// Thumbnail file of an image (without extension). `session` is the root of the open session, if any.
pub fn thumb_base(path: &Path, session: Option<&Path>) -> PathBuf {
    base(path, session, Kind::Thumbnail)
}

fn base(path: &Path, session: Option<&Path>, kind: Kind) -> PathBuf {
    let (dir, id) = match session.and_then(|root| path.strip_prefix(root).ok().map(|rel| (root, rel))) {
        Some((root, rel)) => {
            let rel: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
            (kind.session_dir(root), rel.join("/"))
        }
        None => (kind.user_dir(), path.to_string_lossy().to_string()),
    };
    dir.join(format!("{:016x}", fnv1a(id.as_bytes())))
}

fn files(base: &Path) -> (PathBuf, PathBuf) {
    (base.with_extension("jpg"), base.with_extension("key"))
}

/// Cached thumbnail made with these adjustments.
pub fn load(path: &Path, adj: &Adjustments, session: Option<&Path>) -> Option<::image::DynamicImage> {
    load_kind(path, adj, session, Kind::Thumbnail)
}

pub fn store(path: &Path, adj: &Adjustments, session: Option<&Path>, img: &::image::DynamicImage) {
    store_kind(path, adj, session, img, Kind::Thumbnail);
}

/// Cached viewer preview made with these adjustments.
pub fn load_preview(path: &Path, adj: &Adjustments, session: Option<&Path>) -> Option<::image::DynamicImage> {
    load_kind(path, adj, session, Kind::Preview)
}

pub fn store_preview(path: &Path, adj: &Adjustments, session: Option<&Path>, img: &::image::DynamicImage) {
    store_kind(path, adj, session, img, Kind::Preview);
}

/// Is there a valid cached preview for these adjustments? (Without decoding it.)
pub fn has_preview(path: &Path, adj: &Adjustments, session: Option<&Path>) -> bool {
    let (_, key) = files(&base(path, session, Kind::Preview));
    std::fs::read_to_string(key).is_ok_and(|k| k == thumb_key(path, adj))
}

fn load_kind(path: &Path, adj: &Adjustments, session: Option<&Path>, kind: Kind) -> Option<::image::DynamicImage> {
    let (jpg, key) = files(&base(path, session, kind));
    if std::fs::read_to_string(key).ok()? != thumb_key(path, adj) {
        return None;
    }
    let img = ::image::open(&jpg).ok()?;
    // Used now: the size limit of the per-user cache removes it last (see `trim_user_cache`).
    if session.is_none() {
        let _ = std::fs::File::options().write(true).open(&jpg).and_then(|f| f.set_modified(std::time::SystemTime::now()));
    }
    Some(img)
}

/// Default size limit of the per-user cache (thumbnails and previews of images outside sessions);
/// `SILBERKORN_CACHE_MB` overrides it.
pub const USER_CACHE_LIMIT: u64 = 4 << 30;

pub fn user_cache_limit() -> u64 {
    std::env::var("SILBERKORN_CACHE_MB").ok().and_then(|v| v.parse::<u64>().ok()).map_or(USER_CACHE_LIMIT, |mb| mb << 20)
}

/// Keeps the per-user cache within `limit` bytes: if it is larger, the images used longest ago
/// are removed until it is at 80 % of the limit. Session caches are left alone (they belong to
/// their session). Returns the bytes removed.
pub fn trim_user_cache(limit: u64) -> u64 {
    trim(&Kind::ALL.map(Kind::user_dir), limit)
}

fn trim(dirs: &[PathBuf], limit: u64) -> u64 {
    // (last use, bytes, files) per cached image; only files this cache writes are touched.
    let mut entries: Vec<(std::time::SystemTime, u64, [PathBuf; 2])> = Vec::new();
    let mut total = 0u64;
    for dir in dirs {
        let Ok(list) = std::fs::read_dir(dir) else { continue };
        for e in list.flatten() {
            let jpg = e.path();
            let name = jpg.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            if jpg.extension().is_none_or(|x| x != "jpg") || name.len() != 16 || !name.bytes().all(|b| b.is_ascii_hexdigit()) {
                continue;
            }
            let Ok(meta) = e.metadata() else { continue };
            let key = jpg.with_extension("key");
            let bytes = meta.len() + std::fs::metadata(&key).map_or(0, |m| m.len());
            total += bytes;
            entries.push((meta.modified().unwrap_or(UNIX_EPOCH), bytes, [jpg, key]));
        }
    }
    if total <= limit {
        return 0;
    }
    entries.sort_by_key(|e| e.0);
    let goal = limit / 5 * 4;
    let mut removed = 0;
    for (_, bytes, files) in entries {
        if total - removed <= goal {
            break;
        }
        if std::fs::remove_file(&files[0]).is_ok() {
            let _ = std::fs::remove_file(&files[1]);
            removed += bytes;
        }
    }
    removed
}

fn store_kind(path: &Path, adj: &Adjustments, session: Option<&Path>, img: &::image::DynamicImage, kind: Kind) {
    let (jpg, key) = files(&base(path, session, kind));
    let Some(dir) = jpg.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let mut bytes = Vec::new();
    let mut enc = ::image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, kind.quality());
    if enc.encode_image(&img.to_rgb8()).is_ok() && crate::safe_file::write_nosync(&jpg, &bytes).is_ok() {
        let _ = crate::safe_file::write_nosync(&key, thumb_key(path, adj));
    }
}

/// Follows an image that was moved (keeps its thumbnail and preview).
pub fn rename(from: &Path, to: &Path, session: Option<&Path>) {
    for kind in Kind::ALL {
        let (a, b) = (files(&base(from, session, kind)), files(&base(to, session, kind)));
        if std::fs::rename(&a.0, &b.0).is_ok() {
            let _ = std::fs::rename(&a.1, &b.1);
        }
    }
}

/// Drops the thumbnail and preview of a deleted image.
pub fn remove(path: &Path, session: Option<&Path>) {
    for kind in Kind::ALL {
        let (jpg, key) = files(&base(path, session, kind));
        let _ = std::fs::remove_file(jpg);
        let _ = std::fs::remove_file(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trimming_removes_the_least_recently_used() {
        let dir = std::env::temp_dir().join(format!("sk-trim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Ten cached images of 1000 bytes (with key), used one after the other.
        for i in 0..10u64 {
            let jpg = dir.join(format!("{i:016x}.jpg"));
            std::fs::write(&jpg, vec![0u8; 990]).unwrap();
            std::fs::write(jpg.with_extension("key"), vec![0u8; 10]).unwrap();
            let t = UNIX_EPOCH + std::time::Duration::from_secs(1_000_000 + i);
            std::fs::File::options().write(true).open(&jpg).unwrap().set_modified(t).unwrap();
        }
        // Not ours: never removed.
        std::fs::write(dir.join("notizen.txt"), vec![0u8; 5000]).unwrap();
        assert_eq!(trim(&[dir.clone()], 20_000), 0);
        // Over the limit: down to 80 % of it, oldest first.
        assert_eq!(trim(&[dir.clone()], 5_000), 6_000);
        let left: Vec<u64> = (0..10).filter(|i| dir.join(format!("{i:016x}.jpg")).exists()).collect();
        assert_eq!(left, [6, 7, 8, 9]);
        assert!(!dir.join(format!("{:016x}.key", 0)).exists());
        assert!(dir.join("notizen.txt").exists());
        std::fs::remove_dir_all(dir).ok();
    }

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
        store_preview(&img, &adj, Some(&old), &::image::DynamicImage::new_rgb8(8, 6));
        assert!(base(&img, Some(&old), Kind::Preview).starts_with(crate::session::preview_dir(&old)));
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
        assert_eq!(load_preview(&sel, &adj, Some(&new)).unwrap().width(), 8);
        remove(&sel, Some(&new));
        assert!(load(&sel, &adj, Some(&new)).is_none());
        assert!(load_preview(&sel, &adj, Some(&new)).is_none());
        std::fs::remove_dir_all(tmp).ok();
    }
}
