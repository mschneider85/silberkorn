//! Writing files so that a crash, a full disk or a second writer never leaves a half-written file
//! behind: the data goes into a temporary file of its own next to the target and replaces the
//! target only when it is complete.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};

/// Writes `data` to `path` and makes sure it is on disk before it replaces the old file. For
/// files that can't be recreated: sidecars, XMP files, settings, styles.
pub fn write(path: &Path, data: impl AsRef<[u8]>) -> Result<()> {
    write_with(path, data.as_ref(), true)
}

/// Like [`write`] but leaves flushing to the system. For output that can be produced again
/// (exports, caches): still never half-written, but faster.
pub fn write_nosync(path: &Path, data: impl AsRef<[u8]>) -> Result<()> {
    write_with(path, data.as_ref(), false)
}

fn write_with(path: &Path, data: &[u8], sync: bool) -> Result<()> {
    let tmp = temp_path(path);
    let result = (|| -> std::io::Result<()> {
        // `create_new`: the name is unique to this call, two writers never share it.
        let mut file = std::fs::File::options().write(true).create_new(true).open(&tmp)?;
        write_all(&mut file, data)?;
        if sync {
            file.sync_all()?;
        }
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("{} kann nicht geschrieben werden", path.display()));
    }
    if sync {
        sync_dir(path);
    }
    Ok(())
}

/// `<dir>/.<name>.<pid>-<n>.tmp`: hidden, in the same folder (so the rename stays on one file
/// system) and different for every write of this process and every other process.
fn temp_path(path: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut name = std::ffi::OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(format!(".{}-{n}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// Makes the rename itself durable (only possible on Unix; Windows has no handle for folders).
fn sync_dir(path: &Path) {
    #[cfg(unix)]
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty())
        && let Ok(d) = std::fs::File::open(dir)
    {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(not(test))]
fn write_all(file: &mut std::fs::File, data: &[u8]) -> std::io::Result<()> {
    file.write_all(data)
}

/// In tests a full disk can be simulated with [`fail_writes_after`].
#[cfg(test)]
fn write_all(file: &mut std::fs::File, data: &[u8]) -> std::io::Result<()> {
    let limit = FAIL_AFTER.with(|f| f.get());
    if let Some(limit) = limit.filter(|l| data.len() > *l) {
        file.write_all(&data[..limit])?;
        return Err(std::io::Error::new(std::io::ErrorKind::StorageFull, "Datenträger voll (Test)"));
    }
    file.write_all(data)
}

#[cfg(test)]
thread_local! {
    static FAIL_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// Lets every write of this thread fail after `bytes` bytes (`None`: normal again).
#[cfg(test)]
pub(crate) fn fail_writes_after(bytes: Option<usize>) {
    FAIL_AFTER.with(|f| f.set(bytes));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sk-safe-file-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    #[test]
    fn replaces_the_file_and_leaves_nothing_behind() {
        let dir = temp("replace");
        let path = dir.join("a.json");
        write(&path, "alt").unwrap();
        write(&path, "neu").unwrap();
        write_nosync(&dir.join("b.jpg"), [1u8, 2, 3]).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "neu");
        assert_eq!(entries(&dir), ["a.json", "b.jpg"]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn full_disk_keeps_the_old_file() {
        let dir = temp("full");
        let path = dir.join("a.json");
        write(&path, "der alte Inhalt").unwrap();
        fail_writes_after(Some(4));
        let err = write(&path, "ein neuer, längerer Inhalt").unwrap_err();
        fail_writes_after(None);
        assert!(format!("{err:#}").contains("a.json"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "der alte Inhalt");
        assert_eq!(entries(&dir), ["a.json"]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn parallel_writers_never_mix() {
        let dir = temp("parallel");
        let path = dir.join("a.json");
        std::thread::scope(|s| {
            for i in 0..8 {
                let path = &path;
                s.spawn(move || {
                    for _ in 0..20 {
                        write(path, format!("{i}").repeat(1000)).unwrap();
                    }
                });
            }
        });
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.len(), 1000);
        assert!(text.chars().all(|c| c == text.chars().next().unwrap()));
        assert_eq!(entries(&dir), ["a.json"]);
        std::fs::remove_dir_all(dir).ok();
    }
}
