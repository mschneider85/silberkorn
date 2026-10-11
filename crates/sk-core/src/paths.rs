//! Per-user application folders, with a one-time move from the old "capture-room" name.

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
}
