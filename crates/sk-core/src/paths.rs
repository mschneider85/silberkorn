//! Per-user application folders, with a one-time move from the old "capture-room" name.
//!
//! Linux: `$XDG_CONFIG_HOME`, `$XDG_DATA_HOME`, `$XDG_CACHE_HOME` (default `~/.config`, `~/.local/share`, `~/.cache`).
//! Windows: `%APPDATA%` for settings and styles, `%LOCALAPPDATA%` for catalog, models and thumbnails.

use std::path::{Path, PathBuf};

pub const APP_DIR: &str = "silberkorn";
const LEGACY_APP_DIR: &str = "capture-room";

/// `base/silberkorn`; moves `base/capture-room` there first if only the old folder exists.
pub fn app_dir(base: &Path) -> PathBuf {
    let dir = base.join(APP_DIR);
    let legacy = base.join(LEGACY_APP_DIR);
    if !dir.exists() && legacy.is_dir() {
        if let Err(e) = std::fs::rename(&legacy, &dir) {
            eprintln!("Konnte {} nicht nach {} verschieben: {e}", legacy.display(), dir.display());
            return legacy;
        }
    }
    dir
}

/// The absolute path with links resolved (`path` itself if that fails), without Windows'
/// `\\?\` prefix: paths from dialogs and the command line come without it, and the same file
/// must give the same text (catalog keys, comparisons).
pub fn canonical(path: &Path) -> PathBuf {
    let full = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    match full.to_str().and_then(without_verbatim) {
        Some(plain) => PathBuf::from(plain),
        None => full,
    }
}

/// `\\?\C:\a` -> `C:\a`, `\\?\UNC\server\share` -> `\\server\share`; `None` for other paths
/// and for those too long without the prefix.
pub fn without_verbatim(path: &str) -> Option<String> {
    let rest = path.strip_prefix(r"\\?\")?;
    let plain = match rest.strip_prefix(r"UNC\") {
        Some(unc) => format!(r"\\{unc}"),
        None if rest.as_bytes().get(1) == Some(&b':') && rest.as_bytes()[0].is_ascii_alphabetic() => rest.to_string(),
        None => return None,
    };
    (plain.len() < 260).then_some(plain)
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn home_join(rel: &str) -> Option<PathBuf> {
    env_dir("HOME").map(|h| h.join(rel))
}

/// Settings and styles.
pub fn config_dir() -> PathBuf {
    let base = if cfg!(windows) { env_dir("APPDATA") } else { env_dir("XDG_CONFIG_HOME").or_else(|| home_join(".config")) };
    app_dir(&base.unwrap_or_else(|| PathBuf::from(".")))
}

/// Catalog, downloaded models and lens profiles.
pub fn data_dir() -> PathBuf {
    let base = if cfg!(windows) { env_dir("LOCALAPPDATA") } else { env_dir("XDG_DATA_HOME").or_else(|| home_join(".local/share")) };
    app_dir(&base.unwrap_or_else(|| PathBuf::from(".")))
}

/// Disposable data such as thumbnails.
pub fn cache_dir() -> PathBuf {
    if cfg!(windows) {
        return data_dir().join("cache");
    }
    app_dir(&env_dir("XDG_CACHE_HOME").or_else(|| home_join(".cache")).unwrap_or_else(std::env::temp_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_legacy_dir() {
        let base = std::env::temp_dir().join(format!("skpaths-{}", std::process::id()));
        std::fs::create_dir_all(base.join(LEGACY_APP_DIR)).unwrap();
        std::fs::write(base.join(LEGACY_APP_DIR).join("catalog.sqlite"), b"x").unwrap();
        let dir = app_dir(&base);
        assert_eq!(dir, base.join(APP_DIR));
        assert!(dir.join("catalog.sqlite").exists());
        assert!(!base.join(LEGACY_APP_DIR).exists());
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn verbatim_prefix_is_dropped() {
        assert_eq!(without_verbatim(r"\\?\C:\Fotos\a.ARW").as_deref(), Some(r"C:\Fotos\a.ARW"));
        assert_eq!(without_verbatim(r"\\?\UNC\nas\fotos\a.ARW").as_deref(), Some(r"\\nas\fotos\a.ARW"));
        assert_eq!(without_verbatim(r"C:\Fotos"), None);
        assert_eq!(without_verbatim("/home/a"), None);
        // Device paths and paths only reachable with the prefix keep it.
        assert_eq!(without_verbatim(r"\\?\Volume{1234}\a"), None);
        assert_eq!(without_verbatim(&format!(r"\\?\C:\{}", "a".repeat(300))), None);
    }
}
