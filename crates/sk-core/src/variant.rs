//! Variants: several developments of one raw file (virtual
//! copies).
//!
//! The first variant is the raw file itself. Further variants are addressed by a virtual path,
//! the raw path with `#<id>` appended to the file name (`IMG_0001.ARW#2`), so that everything
//! keyed by path (browser, selection, caches, catalog) treats them as images of their own. Their
//! settings are stored in the raw file's sidecar, so they move and get deleted with it.

use std::path::{Path, PathBuf};

/// Separates the raw file name from the variant number in a virtual path.
const SEP: char = '#';

/// The raw file and the variant number (1 = the file itself) of a path.
pub fn split(path: &Path) -> (PathBuf, u32) {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy()) else { return (path.to_path_buf(), 1) };
    match name.rsplit_once(SEP).and_then(|(file, n)| Some((file, n.parse::<u32>().ok().filter(|n| *n >= 2)?))) {
        Some((file, n)) if !file.is_empty() => (path.with_file_name(file), n),
        _ => (path.to_path_buf(), 1),
    }
}

/// The raw file of a path (the path itself unless it is a variant).
pub fn file_of(path: &Path) -> PathBuf {
    split(path).0
}

/// Variant number of a path: 1 for the raw file itself.
pub fn number(path: &Path) -> u32 {
    split(path).1
}

/// Path of variant `n` of a raw file.
pub fn path_for(raw: &Path, n: u32) -> PathBuf {
    if n <= 1 {
        return raw.to_path_buf();
    }
    let mut name = raw.file_name().unwrap_or_default().to_os_string();
    name.push(format!("{SEP}{n}"));
    raw.with_file_name(name)
}

/// Name shown in the browser: `IMG_0001.ARW`, `IMG_0001.ARW (2)`.
pub fn display_name(path: &Path) -> String {
    let (raw, n) = split(path);
    let name = raw.file_name().unwrap_or_default().to_string_lossy().to_string();
    if n > 1 { format!("{name} ({n})") } else { name }
}

/// All variants of a raw file, the file itself first.
pub fn of_file(raw: &Path) -> Vec<PathBuf> {
    let mut ids: Vec<u32> = crate::sidecar::load(raw).variants.iter().map(|v| v.id).filter(|n| *n >= 2).collect();
    ids.sort_unstable();
    ids.dedup();
    std::iter::once(raw.to_path_buf()).chain(ids.into_iter().map(|n| path_for(raw, n))).collect()
}

/// Raw files with all their variants (each file followed by its variants).
pub fn expand(files: Vec<PathBuf>) -> Vec<PathBuf> {
    files.iter().flat_map(|f| of_file(f)).collect()
}

/// The distinct raw files of some paths, in order.
pub fn files_of(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for p in paths {
        let f = file_of(p);
        if !out.contains(&f) {
            out.push(f);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_paths_roundtrip() {
        let raw = Path::new("/fotos/IMG_0001.ARW");
        assert_eq!(split(raw), (raw.to_path_buf(), 1));
        let v = path_for(raw, 3);
        assert_eq!(v, Path::new("/fotos/IMG_0001.ARW#3"));
        assert_eq!(split(&v), (raw.to_path_buf(), 3));
        assert_eq!(display_name(&v), "IMG_0001.ARW (3)");
        assert_eq!(display_name(raw), "IMG_0001.ARW");
        assert_eq!(path_for(raw, 1), raw);
        // Real file names with '#' stay files.
        for name in ["/a/x#1.ARW", "/a/#2", "/a/x#1"] {
            assert_eq!(split(Path::new(name)), (PathBuf::from(name), 1), "{name}");
        }
        assert_eq!(files_of(&[v.clone(), raw.to_path_buf(), path_for(raw, 2)]), vec![raw.to_path_buf()]);
    }
}
