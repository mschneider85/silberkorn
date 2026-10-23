//! Sessions: a self-contained project folder with its own
//! catalog database and the fixed subfolders Capture, Selects, Output and Trash.
//!
//! ```text
//! Hochzeit Müller/
//!   Hochzeit Müller.sksession   SQLite database (same schema as the catalog)
//!   Capture/                    new images
//!   Selects/                    picks
//!   Output/                     processed files
//!   Trash/                      discarded images until the trash is emptied
//!   Silberkorn/Cache/           thumbnails, so that they travel with the session
//! ```
//!
//! The database stores paths relative to the session folder, so a session can be moved,
//! renamed or copied to another computer as a whole.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::sidecar;

pub const SESSION_EXT: &str = "sksession";
/// Folder for Silberkorn's own data inside a session.
pub const APP_FOLDER: &str = "Silberkorn";

pub fn thumbnail_dir(root: &Path) -> PathBuf {
    root.join(APP_FOLDER).join("Cache").join("Thumbnails")
}

/// Screen-sized viewer previews of the session's images.
pub fn preview_dir(root: &Path) -> PathBuf {
    root.join(APP_FOLDER).join("Cache").join("Previews")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionFolder {
    Capture,
    Selects,
    Output,
    Trash,
}

impl SessionFolder {
    pub const ALL: [SessionFolder; 4] = [SessionFolder::Capture, SessionFolder::Selects, SessionFolder::Output, SessionFolder::Trash];

    /// Folder name on disk (fixed, so sessions stay recognisable).
    pub fn dir_name(self) -> &'static str {
        match self {
            SessionFolder::Capture => "Capture",
            SessionFolder::Selects => "Selects",
            SessionFolder::Output => "Output",
            SessionFolder::Trash => "Trash",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SessionFolder::Capture => "Aufnahmen",
            SessionFolder::Selects => "Selects",
            SessionFolder::Output => "Ausgabe",
            SessionFolder::Trash => "Papierkorb",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    /// The `.sksession` database.
    pub file: PathBuf,
    /// Session folder.
    pub root: PathBuf,
    pub name: String,
}

pub fn is_session_file(path: &Path) -> bool {
    path.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case(SESSION_EXT))
}

/// The session database directly inside `dir`, if any.
pub fn find_session_file(dir: &Path) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| is_session_file(p)).collect();
    found.sort();
    found.into_iter().next()
}

/// Rejects names that cannot be used as a folder name on Windows or Linux.
pub fn validate_name(name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." {
        bail!("Bitte einen Namen eingeben");
    }
    if let Some(c) = name.chars().find(|c| r#"<>:"/\|?*"#.contains(*c) || c.is_control()) {
        bail!("Der Name darf „{c}“ nicht enthalten");
    }
    if name.ends_with('.') {
        bail!("Der Name darf nicht mit einem Punkt enden");
    }
    if crate::naming::is_reserved(name) {
        bail!("„{name}“ ist unter Windows reserviert");
    }
    Ok(())
}

impl Session {
    /// Creates `parent/name/` with the session database and the standard folders.
    pub fn create(parent: &Path, name: &str) -> Result<Session> {
        validate_name(name)?;
        let name = name.trim();
        let root = parent.join(name);
        if find_session_file(&root).is_some() {
            bail!("In {} gibt es bereits eine Session", root.display());
        }
        std::fs::create_dir_all(&root).with_context(|| format!("{} kann nicht angelegt werden", root.display()))?;
        let session = Session { file: root.join(format!("{name}.{SESSION_EXT}")), root, name: name.to_string() };
        session.ensure_folders()?;
        // Creating the catalog writes the database file.
        crate::catalog::Catalog::open(&session.file)?;
        Ok(session)
    }

    /// Opens a session from its `.sksession` file or from the session folder.
    pub fn open(path: &Path) -> Result<Session> {
        let file = if path.is_dir() {
            find_session_file(path).with_context(|| format!("{} enthält keine Session", path.display()))?
        } else if is_session_file(path) {
            path.to_path_buf()
        } else {
            bail!("{} ist keine Session-Datei", path.display());
        };
        let file = crate::paths::canonical(&file);
        let root = file.parent().map(Path::to_path_buf).unwrap_or_default();
        let name = file.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let session = Session { file, root, name };
        session.ensure_folders()?;
        Ok(session)
    }

    /// Recreates missing standard folders (e.g. after the user deleted Trash).
    pub fn ensure_folders(&self) -> Result<()> {
        for f in SessionFolder::ALL {
            let dir = self.folder(f);
            std::fs::create_dir_all(&dir).with_context(|| format!("{} kann nicht angelegt werden", dir.display()))?;
        }
        Ok(())
    }

    pub fn folder(&self, f: SessionFolder) -> PathBuf {
        self.root.join(f.dir_name())
    }

    /// The standard folder that contains `path` (directly or in a subfolder).
    pub fn folder_of(&self, path: &Path) -> Option<SessionFolder> {
        SessionFolder::ALL.into_iter().find(|f| path.starts_with(self.folder(*f)))
    }

    /// Output folder for an export recipe: relative recipe folders are resolved against the
    /// session folder, so the default "Output" lands in the session's Output folder.
    pub fn output_dir(&self, recipe_dir: &Path) -> PathBuf {
        if recipe_dir.is_absolute() { recipe_dir.to_path_buf() } else { self.root.join(recipe_dir) }
    }
}

// ---------------------------------------------------------------- file operations

/// `dir/name`, or `dir/stem-2.ext`, `-3` … if that name (or one of its sidecars) is taken.
fn free_target(dir: &Path, file_name: &std::ffi::OsStr) -> PathBuf {
    let taken = |p: &Path| p.exists() || sidecar::sidecar_files_for(p).iter().any(|s| s.exists());
    let first = dir.join(file_name);
    if !taken(&first) {
        return first;
    }
    let p = Path::new(file_name);
    let stem = p.file_stem().unwrap_or_default().to_string_lossy();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..).map(|n| dir.join(format!("{stem}-{n}{ext}"))).find(|c| !taken(c)).expect("unbounded")
}

/// Rename with a copy+delete fallback for moves across file systems. The original is deleted
/// only once the copy is on the disk.
fn move_file(from: &Path, to: &Path) -> Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {}
        Err(e) => return Err(e).with_context(|| format!("{} kann nicht nach {} verschoben werden", from.display(), to.display())),
    }
    let copied = std::fs::copy(from, to).and_then(|_| std::fs::File::open(to)?.sync_all());
    if let Err(e) = copied {
        let _ = std::fs::remove_file(to);
        return Err(e).with_context(|| format!("{} kann nicht nach {} kopiert werden", from.display(), to.display()));
    }
    std::fs::remove_file(from).with_context(|| format!("{} kann nicht entfernt werden", from.display()))?;
    Ok(())
}

/// Moves an image and its sidecar into `dest`. Returns the new path (renamed on name clashes).
pub fn move_image(image: &Path, dest: &Path) -> Result<PathBuf> {
    if image.parent() == Some(dest) {
        return Ok(image.to_path_buf());
    }
    std::fs::create_dir_all(dest)?;
    let target = free_target(dest, image.file_name().context("kein Dateiname")?);
    relocate(image, &target)?;
    Ok(target)
}

/// Renames an image and its sidecar within its folder to `stem` plus the old extension.
/// Returns the new path (`stem-2` … if the name is taken).
pub fn rename_image(image: &Path, stem: &str) -> Result<PathBuf> {
    let dir = image.parent().context("kein Ordner")?;
    let ext = image.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let name = format!("{stem}{ext}");
    if image.file_name() == Some(std::ffi::OsStr::new(&name)) {
        return Ok(image.to_path_buf());
    }
    // Only the case changes: on Windows the "other" name is the image itself, not a clash.
    let same = dir.join(&name);
    let target = match (same.canonicalize(), image.canonicalize()) {
        (Ok(a), Ok(b)) if a == b => same,
        _ => free_target(dir, std::ffi::OsStr::new(&name)),
    };
    relocate(image, &target)?;
    Ok(target)
}

/// Moves the image file and its sidecars to `target`, all or nothing.
fn relocate(image: &Path, target: &Path) -> Result<()> {
    let target = target.to_path_buf();
    move_file(image, &target)?;
    let mut moved = vec![(image.to_path_buf(), target.clone())];
    for (from, to) in sidecar::sidecar_files_for(image).into_iter().zip(sidecar::sidecar_files_for(&target)).filter(|(from, _)| crate::xmp::owned(image, from)) {
        if !from.exists() {
            continue;
        }
        if let Err(e) = move_file(&from, &to) {
            // Without its sidecars (settings, variants) the image must not move: put everything back.
            for (from, to) in moved.iter().rev() {
                let _ = move_file(to, from);
            }
            return Err(e);
        }
        moved.push((from, to));
    }
    Ok(())
}

/// Copies an image with its settings and XMP file into `dest`. Returns the new path. The copy
/// is all or nothing: no half-written image (full disk, crash) and no image without its settings.
/// The settings go where the copy keeps them (see `sidecar::copy_settings`): its sidecar file,
/// or the catalog, which takes the copy in for them.
pub fn copy_image(image: &Path, dest: &Path) -> Result<PathBuf> {
    copy_image_as(image, dest, None)
}

/// [`copy_image`] under the name `stem` (plus the file's extension; `-2` … if it is taken).
pub fn copy_image_as(image: &Path, dest: &Path, stem: Option<&str>) -> Result<PathBuf> {
    std::fs::create_dir_all(dest)?;
    let name = match stem {
        Some(stem) => {
            let ext = image.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
            std::ffi::OsString::from(format!("{stem}{ext}"))
        }
        None => image.file_name().context("kein Dateiname")?.to_os_string(),
    };
    let target = free_target(dest, &name);
    // The catalog needs the image to take it in; a sidecar file comes first, so that the image
    // appears last, and only once everything is there.
    let in_store = sidecar::kept_elsewhere(&target);
    let mut copied = Vec::new();
    let settings = |copied: &mut Vec<PathBuf>| {
        let r = sidecar::copy_settings(image, &target);
        copied.extend(sidecar::sidecar_files_for(&target).into_iter().filter(|p| p.exists()));
        r
    };
    let mut result = if in_store { Ok(()) } else { settings(&mut copied) };
    if result.is_ok() {
        let pairs = crate::xmp::files_for(image).into_iter().zip(crate::xmp::files_for(&target));
        result = pairs.filter(|(from, _)| from.exists() && crate::xmp::owned(image, from)).chain([(image.to_path_buf(), target.clone())]).try_for_each(|(from, to)| {
            crate::safe_file::copy(&from, &to)?;
            copied.push(to);
            anyhow::Ok(())
        });
    }
    if result.is_ok() && in_store {
        result = settings(&mut copied);
    }
    if let Err(e) = result {
        if in_store {
            sidecar::forget(&target);
        }
        for p in copied {
            let _ = std::fs::remove_file(p);
        }
        return Err(e);
    }
    Ok(target)
}

/// Deletes an image and its sidecar for good.
pub fn delete_image(image: &Path) -> Result<()> {
    std::fs::remove_file(image).with_context(|| format!("{} kann nicht gelöscht werden", image.display()))?;
    for s in sidecar::sidecar_files_for(image) {
        if s.exists() && crate::xmp::owned(image, &s) {
            std::fs::remove_file(s)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sksession-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rename_takes_the_sidecar_along_and_avoids_clashes() {
        let dir = temp("rename");
        let (a, b) = (dir.join("a.ARW"), dir.join("b.ARW"));
        for p in [&a, &b] {
            std::fs::write(p, b"raw").unwrap();
        }
        std::fs::write(sidecar::sidecar_path(&a), b"{}").unwrap();
        let new = rename_image(&a, "Hochzeit_001").unwrap();
        assert_eq!(new, dir.join("Hochzeit_001.ARW"));
        assert!(!a.exists() && sidecar::sidecar_path(&new).exists());
        // Taken name: a number is appended; the same name: nothing happens.
        assert_eq!(rename_image(&b, "Hochzeit_001").unwrap(), dir.join("Hochzeit_001-2.ARW"));
        assert_eq!(rename_image(&new, "Hochzeit_001").unwrap(), new);
        // Only the case changed: the same file where names ignore case (Windows), else a clash
        // only if another file has that name.
        let upper = rename_image(&new, "HOCHZEIT_001").unwrap();
        assert_eq!(upper, dir.join("HOCHZEIT_001.ARW"));
        assert!(sidecar::sidecar_path(&upper).exists());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn copy_takes_the_sidecar_along_and_avoids_clashes() {
        let dir = temp("copy");
        let (src, dest) = (dir.join("src"), dir.join("dest"));
        std::fs::create_dir_all(&src).unwrap();
        let a = src.join("a.ARW");
        std::fs::write(&a, b"raw").unwrap();
        std::fs::write(sidecar::sidecar_path(&a), br#"{"rating": 2}"#).unwrap();
        let first = copy_image(&a, &dest).unwrap();
        assert_eq!(std::fs::read(&first).unwrap(), b"raw");
        assert_eq!(sidecar::load(&first).rating, 2);
        assert_eq!(copy_image(&a, &dest).unwrap(), dest.join("a-2.ARW"));
        // Nothing but the copies (no temporary files) in the target folder.
        assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 4);
        // A source that can't be read leaves nothing behind.
        assert!(copy_image(&src.join("fehlt.ARW"), &dest).is_err());
        assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 4);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn raw_files_of_the_same_name_do_not_share_an_xmp_file() {
        let dir = temp("xmp-pair");
        let (arw, dng, jpg) = (dir.join("IMG_0001.ARW"), dir.join("IMG_0001.DNG"), dir.join("IMG_0002.JPG"));
        for p in [&arw, &dng, &jpg] {
            std::fs::write(p, b"raw").unwrap();
        }
        let shared = dir.join("IMG_0001.xmp");
        std::fs::write(&shared, crate::xmp::updated(None, 4, sidecar::ColorLabel::None, &[]).unwrap()).unwrap();
        // Neither reads it; ratings are written to a file of its own.
        assert_eq!(crate::xmp::existing(&arw), None);
        crate::xmp::write(&arw, 2, sidecar::ColorLabel::None, &[]).unwrap();
        assert_eq!(crate::xmp::existing(&arw), Some(dir.join("IMG_0001.ARW.xmp")));
        assert_eq!(crate::xmp::existing(&dng), None);
        // Renaming or deleting one leaves it to the other.
        let renamed = rename_image(&arw, "Neu").unwrap();
        assert!(shared.exists() && dir.join("Neu.ARW.xmp").exists());
        delete_image(&renamed).unwrap();
        assert!(shared.exists());
        // Alone (a JPEG of the same name doesn't count), the file is the raw's own.
        std::fs::write(dir.join("IMG_0001.JPG"), b"jpg").unwrap();
        assert_eq!(crate::xmp::existing(&dng), Some(shared.clone()));
        delete_image(&dng).unwrap();
        assert!(!shared.exists());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn create_and_open() {
        let base = temp("create");
        let s = Session::create(&base, " Hochzeit ").unwrap();
        assert_eq!(s.name, "Hochzeit");
        assert!(s.file.is_file());
        for f in SessionFolder::ALL {
            assert!(s.folder(f).is_dir());
        }
        assert!(Session::create(&base, "Hochzeit").is_err());

        std::fs::remove_dir(s.folder(SessionFolder::Trash)).unwrap();
        let o = Session::open(&s.root).unwrap();
        assert_eq!(o.name, "Hochzeit");
        assert!(o.folder(SessionFolder::Trash).is_dir());
        let o2 = Session::open(&o.file).unwrap();
        assert_eq!(o2.root, o.root);
        assert_eq!(o.folder_of(&o.folder(SessionFolder::Selects).join("x/a.ARW")), Some(SessionFolder::Selects));
        assert_eq!(o.folder_of(&o.root.join("a.ARW")), None);
        assert_eq!(o.output_dir(Path::new("Output")), o.folder(SessionFolder::Output));
        assert!(Session::open(&base).is_err());
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn names() {
        assert!(validate_name("Shooting 2026-10").is_ok());
        for bad in ["", "  ", "..", "a/b", "a\\b", "a:b", "x.", "CON", "nul.txt", "Com3"] {
            assert!(validate_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn move_copy_delete_with_sidecars() {
        let base = temp("move");
        let (a, b) = (base.join("a"), base.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        let img = a.join("x.ARW");
        std::fs::write(&img, b"raw").unwrap();
        std::fs::write(sidecar::sidecar_path(&img), br#"{"rating": 2}"#).unwrap();
        // Clash in the destination: the moved file gets a suffix.
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("x.ARW"), b"other").unwrap();

        let moved = move_image(&img, &b).unwrap();
        assert_eq!(moved, b.join("x-2.ARW"));
        assert!(!img.exists() && !sidecar::sidecar_path(&img).exists());
        assert!(sidecar::sidecar_path(&moved).exists());
        assert_eq!(move_image(&moved, &b).unwrap(), moved);

        let copied = copy_image(&moved, &a).unwrap();
        assert_eq!(copied, a.join("x-2.ARW"));
        assert!(moved.exists() && sidecar::sidecar_path(&copied).exists());

        delete_image(&copied).unwrap();
        assert!(!copied.exists() && !sidecar::sidecar_path(&copied).exists());
        std::fs::remove_dir_all(base).ok();
    }
}
