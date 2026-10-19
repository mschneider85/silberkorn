//! Per-image settings: adjustments and metadata. They live in sidecar files (`<image>.sk.json`;
//! older `<image>.crs.json` files are still read), unless a [`Store`] keeps them: the catalog
//! keeps those of its images in its database.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, RwLock, Weak};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::params::Adjustments;

/// Format version of the sidecar files written by this build. A file with a higher version was
/// written by a newer Silberkorn: it is read as far as understood, but never overwritten, since
/// that would silently drop what this build doesn't know.
pub const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ColorLabel {
    #[default]
    None,
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
    Pink,
    Purple,
}

impl ColorLabel {
    pub const ALL: [ColorLabel; 8] = [
        ColorLabel::None,
        ColorLabel::Red,
        ColorLabel::Orange,
        ColorLabel::Yellow,
        ColorLabel::Green,
        ColorLabel::Blue,
        ColorLabel::Pink,
        ColorLabel::Purple,
    ];

    pub fn rgb(self) -> Option<[u8; 3]> {
        Some(match self {
            ColorLabel::None => return None,
            ColorLabel::Red => [220, 50, 47],
            ColorLabel::Orange => [240, 140, 30],
            ColorLabel::Yellow => [230, 210, 40],
            ColorLabel::Green => [80, 180, 70],
            ColorLabel::Blue => [50, 120, 220],
            ColorLabel::Pink => [230, 100, 180],
            ColorLabel::Purple => [140, 80, 200],
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            ColorLabel::None => "Keine",
            ColorLabel::Red => "Rot",
            ColorLabel::Orange => "Orange",
            ColorLabel::Yellow => "Gelb",
            ColorLabel::Green => "Grün",
            ColorLabel::Blue => "Blau",
            ColorLabel::Pink => "Pink",
            ColorLabel::Purple => "Lila",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Sidecar {
    pub version: u32,
    /// 0..5 stars
    pub rating: u8,
    pub color_label: ColorLabel,
    pub keywords: Vec<String>,
    pub adjustments: Adjustments,
    /// Further variants of the image (see `variant`), only in the raw file's sidecar.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<Variant>,
    /// Saved states of the adjustments (snapshots).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub snapshots: Vec<Snapshot>,
    /// Modification time of the XMP file (see `xmp`) when Silberkorn last read or wrote it;
    /// a newer XMP file overrides rating, label and keywords.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xmp_mtime: Option<i64>,
    /// Stack the image belongs to (see `stack`); kept by [`save`], changed by [`set_stack`].
    /// Variants belong to the stack of their file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<Stack>,
}

/// Membership in a stack: images with the same id form a stack, shown collapsed as its top image.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Stack {
    pub id: String,
    /// The image shown for the collapsed stack.
    pub top: bool,
}

/// A named state of the adjustments.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Snapshot {
    pub name: String,
    pub adjustments: Adjustments,
}

/// Settings of a further variant (number `id` ≥ 2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Variant {
    pub id: u32,
    pub rating: u8,
    pub color_label: ColorLabel,
    pub keywords: Vec<String>,
    pub adjustments: Adjustments,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub snapshots: Vec<Snapshot>,
}

impl Variant {
    fn to_sidecar(&self) -> Sidecar {
        Sidecar {
            version: VERSION,
            rating: self.rating,
            color_label: self.color_label,
            keywords: self.keywords.clone(),
            adjustments: self.adjustments.clone(),
            variants: Vec::new(),
            snapshots: self.snapshots.clone(),
            xmp_mtime: None,
            stack: None,
        }
    }

    fn from_sidecar(id: u32, sc: &Sidecar) -> Self {
        Variant {
            id,
            rating: sc.rating,
            color_label: sc.color_label,
            keywords: sc.keywords.clone(),
            adjustments: sc.adjustments.clone(),
            snapshots: sc.snapshots.clone(),
        }
    }
}

/// Keeps the settings of some images instead of their sidecar files (see `catalog::CatalogStore`).
/// A record is what a raw file's sidecar file holds: its own settings and its variants'.
pub trait Store: Send + Sync {
    /// The record of raw file `raw`: `None` if this store doesn't keep it, else as [`read_file`]
    /// gives it (`Ok(None)` for none yet).
    fn read(&self, raw: &Path) -> Option<Result<Option<Sidecar>>>;
    /// Stores the record of `raw`; `None` if this store doesn't keep it.
    fn write(&self, raw: &Path, record: &Sidecar) -> Option<Result<()>>;
    /// Whether [`Store::write`] would keep the record of `raw`.
    fn keeps(&self, raw: &Path) -> bool;
    /// Forgets `raw` (a copy taken in whose copying then failed).
    fn forget(&self, raw: &Path);
}

/// The stores in use; the files keep everything else.
static STORES: RwLock<Vec<Weak<dyn Store>>> = RwLock::new(Vec::new());

/// Uses `store` until it is dropped.
pub fn add_store(store: &Arc<dyn Store>) {
    let mut stores = STORES.write().unwrap_or_else(|e| e.into_inner());
    stores.retain(|s| s.strong_count() > 0);
    stores.push(Arc::downgrade(store));
}

fn stores() -> Vec<Arc<dyn Store>> {
    STORES.read().unwrap_or_else(|e| e.into_inner()).iter().filter_map(Weak::upgrade).collect()
}

/// The record of raw file `raw` from the store keeping it, if one does.
fn stored(raw: &Path) -> Option<Result<Option<Sidecar>>> {
    // Never taken for a damaged file (see `load_for_update`): that would replace the stored
    // record with the defaults.
    stores().iter().find_map(|s| s.read(raw)).map(|record| record.map(|r| r.map(normalized)).map_err(|e| anyhow::anyhow!("{e:#}")))
}

/// Whether a store, not a sidecar file, keeps (or would keep) the settings of `image`.
pub fn kept_elsewhere(image: &Path) -> bool {
    let raw = crate::variant::file_of(image);
    stores().iter().any(|s| s.keeps(&raw))
}

/// Makes the stores forget `image` (see [`Store::forget`]).
pub fn forget(image: &Path) {
    let raw = crate::variant::file_of(image);
    for s in stores() {
        s.forget(&raw);
    }
}

/// Sidecar file of an image; variants share the one of their raw file.
pub fn sidecar_path(image: &Path) -> PathBuf {
    let image = &crate::variant::file_of(image);
    let mut name = image.file_name().unwrap_or_default().to_os_string();
    name.push(".sk.json");
    image.with_file_name(name)
}

/// Sidecar name used before the rename to Silberkorn.
fn legacy_sidecar_path(image: &Path) -> PathBuf {
    let image = &crate::variant::file_of(image);
    let mut name = image.file_name().unwrap_or_default().to_os_string();
    name.push(".crs.json");
    image.with_file_name(name)
}

/// Silberkorn's own sidecar names of an image (current and legacy).
fn json_files_for(image: &Path) -> [PathBuf; 2] {
    [sidecar_path(image), legacy_sidecar_path(image)]
}

/// All sidecar names an image can have (current and legacy, and XMP), whether they exist or
/// not; they move with the image.
pub fn sidecar_files_for(image: &Path) -> [PathBuf; 4] {
    let [json, legacy] = json_files_for(image);
    let [xmp, xmp_long] = crate::xmp::files_for(image);
    [json, legacy, xmp, xmp_long]
}

/// Settings of an image or variant. The raw file's sidecar also lists its further variants.
/// Rating, label and keywords come from the XMP file instead if it changed since Silberkorn last
/// read or wrote it (see `xmp`).
pub fn load(image: &Path) -> Sidecar {
    from_file(image, load_file(image))
}

/// Like [`load`], but an error if the sidecar file exists and can't be read or is from a newer
/// Silberkorn: whoever saves what this returns would otherwise replace the stored settings with
/// defaults. A damaged file counts as none, as when saving (which keeps it as `.bak`).
pub fn try_load(image: &Path) -> Result<Sidecar> {
    let file = match read_file(image) {
        Ok(Some(sc)) if sc.version > VERSION => anyhow::bail!(
            "{} wurde von einer neueren Silberkorn-Version gespeichert und wird nicht überschrieben. Bitte Silberkorn aktualisieren.",
            sidecar_path(image).display()
        ),
        Ok(sc) => sc.unwrap_or_default(),
        Err(e) if is_damaged(&e) => {
            log::warn!("{e:#}");
            Sidecar::default()
        }
        Err(e) => return Err(e),
    };
    Ok(from_file(image, file))
}

/// The settings of `image` (raw file or variant) from its raw file's sidecar.
fn from_file(image: &Path, file: Sidecar) -> Sidecar {
    match crate::variant::number(image) {
        1 => with_xmp(image, file),
        n => variant_of(&file, n),
    }
}

/// Whether a [`read_file`] error means the file is damaged (not unreadable or newer).
pub(crate) fn is_damaged(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.is::<serde_json::Error>())
}

/// The settings of variant `n` (2 or more) in a raw file's record.
pub fn load_variant_of(record: &Sidecar, n: u32) -> Sidecar {
    variant_of(record, n)
}

/// Variant `n` (2 or more) of a raw file's sidecar.
fn variant_of(file: &Sidecar, n: u32) -> Sidecar {
    Sidecar {
        stack: file.stack.as_ref().map(|s| Stack { id: s.id.clone(), top: false }),
        ..file.variants.iter().find(|v| v.id == n).map(Variant::to_sidecar).unwrap_or_default()
    }
}

/// Raw files with all their variants (each file followed by its variants, like
/// [`crate::variant::expand`]) and the settings of each, as [`load`] gives them; every sidecar
/// file is read once, several at a time (for folders on slow drives).
pub fn load_all(files: &[PathBuf]) -> Vec<(PathBuf, Sidecar)> {
    use rayon::prelude::*;
    let per_file: Vec<Vec<(PathBuf, Sidecar)>> = files
        .par_iter()
        .map(|raw| {
            let file = load_file(raw);
            let mut ids: Vec<u32> = file.variants.iter().map(|v| v.id).filter(|n| *n >= 2).collect();
            ids.sort_unstable();
            ids.dedup();
            let variants: Vec<(PathBuf, Sidecar)> = ids.into_iter().map(|n| (crate::variant::path_for(raw, n), variant_of(&file, n))).collect();
            std::iter::once((raw.clone(), with_xmp(raw, file))).chain(variants).collect()
        })
        .collect();
    per_file.into_iter().flatten().collect()
}

/// Takes rating, label and keywords from a changed XMP file.
fn with_xmp(image: &Path, mut sc: Sidecar) -> Sidecar {
    let Some(mtime) = crate::xmp::mtime(image) else { return sc };
    if sc.xmp_mtime == Some(mtime) {
        return sc;
    }
    if let Some(x) = crate::xmp::read(image) {
        sc.rating = x.rating.unwrap_or(sc.rating);
        sc.color_label = x.label.unwrap_or(sc.color_label);
        if let Some(k) = x.keywords {
            sc.keywords = k;
        }
    }
    sc
}

/// Reads the image's XMP file again on the next load, even if it is older than the last save.
pub fn reread_xmp(image: &Path) -> Result<()> {
    let _lock = lock_record(image);
    let mut file = load_for_update(image)?;
    if file.xmp_mtime.take().is_some() {
        write_file(image, &file)?;
    }
    Ok(())
}

/// Writes rating, label and keywords of an image into its XMP file now (also when automatic
/// writing is off).
pub fn write_xmp(image: &Path) -> Result<()> {
    let sc = load(image);
    let _lock = lock_record(image);
    let mut file = load_for_update(image)?;
    file.xmp_mtime = crate::xmp::write(image, sc.rating, sc.color_label, &sc.keywords)?;
    (file.rating, file.color_label, file.keywords) = (sc.rating, sc.color_label, sc.keywords);
    write_file(image, &file)
}

fn load_file(image: &Path) -> Sidecar {
    read_file(image).ok().flatten().unwrap_or_default()
}

/// The record of an image (its raw file's): `None` if there is none, an error if it can't be read.
fn read_file(image: &Path) -> Result<Option<Sidecar>> {
    let raw = crate::variant::file_of(image);
    stored(&raw).unwrap_or_else(|| read_sidecar_file(&raw))
}

/// The sidecar file of an image itself, whatever keeps its settings: `None` if there is none,
/// an error if it can't be read.
pub fn read_sidecar_file(image: &Path) -> Result<Option<Sidecar>> {
    for path in json_files_for(image) {
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let sc: Sidecar = match serde_json::from_str(&text) {
                    Ok(sc) => sc,
                    // A newer Silberkorn may use values this build doesn't know: not damaged.
                    Err(_) if newer_version(&text) => anyhow::bail!(
                        "{} wurde von einer neueren Silberkorn-Version gespeichert und wird nicht überschrieben. Bitte Silberkorn aktualisieren.",
                        path.display()
                    ),
                    Err(e) => return Err(e).with_context(|| format!("{} ist beschädigt", path.display())),
                };
                return Ok(Some(normalized(sc)));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("{} kann nicht gelesen werden", path.display())),
        }
    }
    Ok(None)
}

fn normalized(mut sc: Sidecar) -> Sidecar {
    sc.adjustments = sc.adjustments.normalized();
    for v in &mut sc.variants {
        v.adjustments = std::mem::take(&mut v.adjustments).normalized();
        for s in &mut v.snapshots {
            s.adjustments = std::mem::take(&mut s.adjustments).normalized();
        }
    }
    for s in &mut sc.snapshots {
        s.adjustments = std::mem::take(&mut s.adjustments).normalized();
    }
    sc
}

/// A record as a [`Store`] keeps it (as a sidecar file holds it).
pub fn to_json(record: &Sidecar) -> Result<String> {
    Ok(serde_json::to_string(&Sidecar { version: VERSION, ..record.clone() })?)
}

/// Inverse of [`to_json`]; an error for a record of a newer Silberkorn (see [`VERSION`]).
pub fn from_json(text: &str) -> Result<Sidecar> {
    match serde_json::from_str(text) {
        Ok(sc) => Ok(sc),
        Err(_) if newer_version(text) => anyhow::bail!("Die Einstellungen wurden von einer neueren Silberkorn-Version gespeichert. Bitte Silberkorn aktualisieren."),
        Err(e) => Err(e).context("Einstellungen beschädigt"),
    }
}

/// The whole record of an image's raw file (its settings and its variants'), to take along
/// when the file is copied (see [`write_record`]).
pub fn read_record(image: &Path) -> Result<Sidecar> {
    let _lock = lock_record(image);
    load_for_update(image)
}

/// Gives the copy `to` of an image the settings of `from` (wherever each keeps them). A sidecar
/// file this build can't read (damaged, or from a newer Silberkorn) is copied as it is.
pub fn copy_settings(from: &Path, to: &Path) -> Result<()> {
    let raw = crate::variant::file_of(from);
    let record = match stored(&raw) {
        // A store that can't be read is an error: its sidecar file, if any, is outdated.
        Some(record) => record?,
        None => match read_sidecar_file(&raw) {
            Ok(record) if record.as_ref().is_none_or(|r| r.version <= VERSION) => record,
            _ => {
                for (a, b) in json_files_for(from).into_iter().zip(json_files_for(to)) {
                    if a.exists() {
                        crate::safe_file::copy(&a, &b)?;
                    }
                }
                return Ok(());
            }
        },
    };
    match record {
        Some(record) if (Sidecar { version: 0, ..record.clone() }) != Sidecar::default() => {
            let _lock = lock_record(to);
            write_file(to, &record)
        }
        _ => Ok(()),
    }
}

/// Stores a whole record (see [`read_record`]).
pub fn write_record(image: &Path, record: &Sidecar) -> Result<()> {
    let _lock = lock_record(image);
    write_file(image, record)
}

/// Whether `text` is a sidecar with a format version above [`VERSION`].
fn newer_version(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| v.get("version")?.as_u64())
        .is_some_and(|v| v > u64::from(VERSION))
}

/// Ids of the further variants stored with a raw file; an error if its sidecar can't be read.
pub fn variant_ids(raw: &Path) -> Result<Vec<u32>> {
    Ok(read_file(raw)?.map(|sc| sc.variants.iter().map(|v| v.id).collect()).unwrap_or_default())
}

/// Raw files whose record is being changed (see [`lock_record`]).
static LOCKED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static UNLOCKED: Condvar = Condvar::new();

/// Holds the record of a raw file for one read-modify-write (see [`lock_record`]).
struct RecordLock(PathBuf);

/// Waits until no other thread changes the record of `image`'s raw file, then holds it until
/// dropped: variants share one record, and saving several at once (rating all variants) would
/// otherwise lose all but one change. Not reentrant.
fn lock_record(image: &Path) -> RecordLock {
    let raw = crate::variant::file_of(image);
    let mut locked = LOCKED.lock().unwrap_or_else(|e| e.into_inner());
    while locked.contains(&raw) {
        locked = UNLOCKED.wait(locked).unwrap_or_else(|e| e.into_inner());
    }
    locked.push(raw.clone());
    RecordLock(raw)
}

impl Drop for RecordLock {
    fn drop(&mut self) {
        let mut locked = LOCKED.lock().unwrap_or_else(|e| e.into_inner());
        locked.retain(|p| *p != self.0);
        UNLOCKED.notify_all();
    }
}

/// Keeps the damaged sidecar files of `image` as `.bak` (they may hold the settings of all
/// variants), so that they aren't read or overwritten again.
pub(crate) fn back_up_damaged(image: &Path) -> Result<()> {
    for path in json_files_for(image).into_iter().filter(|p| p.exists()) {
        // Never replaces an earlier backup.
        let backup = (1..)
            .map(|n| {
                let mut b = path.clone().into_os_string();
                b.push(if n == 1 { ".bak".to_string() } else { format!(".{n}.bak") });
                PathBuf::from(b)
            })
            .find(|b| !b.exists())
            .expect("free backup name");
        std::fs::rename(&path, &backup).with_context(|| format!("{} kann nicht gesichert werden", path.display()))?;
    }
    Ok(())
}

/// The sidecar file before changing it (under [`lock_record`]). One that can't be parsed is
/// kept as `.bak` (see [`back_up_damaged`]) instead of being overwritten without notice; one
/// from a newer Silberkorn is an error (see [`VERSION`]).
fn load_for_update(image: &Path) -> Result<Sidecar> {
    match read_file(image) {
        Ok(Some(sc)) if sc.version > VERSION => anyhow::bail!(
            "{} wurde von einer neueren Silberkorn-Version gespeichert und wird nicht überschrieben. Bitte Silberkorn aktualisieren.",
            sidecar_path(image).display()
        ),
        Ok(sc) => Ok(sc.unwrap_or_default()),
        Err(e) if is_damaged(&e) => {
            back_up_damaged(image)?;
            Ok(Sidecar::default())
        }
        Err(e) => Err(e),
    }
}

/// Stores the settings of an image or variant. The list of further variants and the snapshots
/// are kept as they are (they change only through [`add_variant`], [`remove_variant`] and
/// [`set_snapshots`]).
pub fn save(image: &Path, sidecar: &Sidecar) -> Result<()> {
    let _lock = lock_record(image);
    let mut file = load_for_update(image)?;
    match crate::variant::number(image) {
        1 => {
            let current = crate::xmp::mtime(image);
            let own_change = (sidecar.rating, sidecar.color_label, &sidecar.keywords) != (file.rating, file.color_label, &file.keywords);
            // From now on these settings count, not an older XMP file (written along if enabled).
            // Unless the XMP file was changed elsewhere since it was last read and rating, label
            // and keywords weren't changed here: then that change still counts on the next load.
            // (Not recognized: a caller holding values from an older XMP change that wasn't saved
            // yet; they count as changed here.)
            let xmp_mtime = if current.is_some() && current != file.xmp_mtime && !own_change {
                file.xmp_mtime
            } else if crate::xmp::write_enabled() {
                crate::xmp::write(image, sidecar.rating, sidecar.color_label, &sidecar.keywords).unwrap_or_else(|e| {
                    // The settings themselves are still saved.
                    log::warn!("{e:#}");
                    crate::xmp::mtime(image)
                })
            } else {
                current
            };
            file = Sidecar {
                variants: std::mem::take(&mut file.variants),
                snapshots: std::mem::take(&mut file.snapshots),
                stack: file.stack.take(),
                xmp_mtime,
                ..sidecar.clone()
            }
        }
        n => match file.variants.iter_mut().find(|v| v.id == n) {
            Some(v) => *v = Variant { snapshots: std::mem::take(&mut v.snapshots), ..Variant::from_sidecar(n, sidecar) },
            None => file.variants.push(Variant { snapshots: Vec::new(), ..Variant::from_sidecar(n, sidecar) }),
        },
    }
    write_file(image, &file)
}

/// Replaces the snapshots of an image or variant.
pub fn set_snapshots(image: &Path, snapshots: Vec<Snapshot>) -> Result<()> {
    let _lock = lock_record(image);
    let mut file = load_for_update(image)?;
    match crate::variant::number(image) {
        1 => file.snapshots = snapshots,
        n => match file.variants.iter_mut().find(|v| v.id == n) {
            Some(v) => v.snapshots = snapshots,
            None => anyhow::bail!("Variante {n} gibt es nicht"),
        },
    }
    write_file(image, &file)
}

/// Sets or removes the stack of an image's file.
pub fn set_stack(image: &Path, stack: Option<Stack>) -> Result<()> {
    let _lock = lock_record(image);
    let mut file = load_for_update(image)?;
    if file.stack != stack {
        file.stack = stack;
        write_file(image, &file)?;
    }
    Ok(())
}

/// Adds a variant with these settings to an image; returns its path.
pub fn add_variant(image: &Path, settings: &Sidecar) -> Result<PathBuf> {
    let _lock = lock_record(image);
    let raw = crate::variant::file_of(image);
    let mut file = load_for_update(&raw)?;
    let id = file.variants.iter().map(|v| v.id).max().unwrap_or(1).max(1) + 1;
    file.variants.push(Variant::from_sidecar(id, settings));
    write_file(&raw, &file)?;
    Ok(crate::variant::path_for(&raw, id))
}

/// Removes a further variant (not the raw file's own settings).
pub fn remove_variant(variant: &Path) -> Result<()> {
    let n = crate::variant::number(variant);
    if n < 2 {
        anyhow::bail!("Die erste Variante gehört zur Datei selbst");
    }
    let _lock = lock_record(variant);
    let mut file = load_for_update(variant)?;
    file.variants.retain(|v| v.id != n);
    write_file(variant, &file)
}

fn write_file(image: &Path, sidecar: &Sidecar) -> Result<()> {
    let raw = crate::variant::file_of(image);
    for store in stores() {
        if let Some(written) = store.write(&raw, sidecar) {
            return written;
        }
    }
    let path = sidecar_path(image);
    if (Sidecar { version: 0, ..sidecar.clone() }) == Sidecar::default() {
        // Nothing to store; remove a stale file if present.
        if path.exists() {
            std::fs::remove_file(path)?;
        }
    } else {
        crate::safe_file::write(&path, serde_json::to_string_pretty(&Sidecar { version: VERSION, ..sidecar.clone() })?)?;
    }
    // The legacy file goes only once its content is safely in the new one.
    let legacy = legacy_sidecar_path(image);
    if legacy.exists() {
        std::fs::remove_file(legacy)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join(format!("sk-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("a.ARW");
        let mut sc = Sidecar { rating: 4, color_label: ColorLabel::Green, ..Default::default() };
        sc.adjustments.exposure = 0.7;
        save(&img, &sc).unwrap();
        let loaded = load(&img);
        assert_eq!(loaded.rating, 4);
        assert_eq!(loaded.adjustments.exposure, 0.7);
        save(&img, &Sidecar::default()).unwrap();
        assert!(!sidecar_path(&img).exists());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn try_load_refuses_unreadable_files_only() {
        let dir = std::env::temp_dir().join(format!("sk-tryload-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("u.ARW");
        assert_eq!(try_load(&img).unwrap().rating, 0);
        save(&img, &Sidecar { rating: 3, ..Default::default() }).unwrap();
        assert_eq!(try_load(&img).unwrap().rating, 3);
        // Damaged: counts as none (saving keeps it as `.bak`).
        std::fs::write(sidecar_path(&img), "{ kaputt").unwrap();
        assert_eq!(try_load(&img).unwrap().rating, 0);
        // Can't be read at all: an error, not the defaults.
        std::fs::remove_file(sidecar_path(&img)).unwrap();
        std::fs::create_dir(sidecar_path(&img)).unwrap();
        assert!(try_load(&img).is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn snapshots_survive_ordinary_saves() {
        let dir = std::env::temp_dir().join(format!("sk-snapshot-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("s.ARW");
        save(&img, &Sidecar { rating: 1, ..Default::default() }).unwrap();
        let v2 = add_variant(&img, &Sidecar::default()).unwrap();
        let snap = |name: &str, ev: f32| Snapshot { name: name.into(), adjustments: Adjustments { exposure: ev, ..Default::default() } };
        set_snapshots(&img, vec![snap("Farbe", 0.5)]).unwrap();
        set_snapshots(&v2, vec![snap("SW", -0.3)]).unwrap();
        // Rating or adjustments saved the usual way (snapshots empty there) keep them.
        save(&img, &Sidecar { rating: 4, ..Default::default() }).unwrap();
        save(&v2, &Sidecar { rating: 2, ..Default::default() }).unwrap();
        assert_eq!(load(&img).snapshots, vec![snap("Farbe", 0.5)]);
        assert_eq!(load(&v2).snapshots, vec![snap("SW", -0.3)]);
        assert_eq!((load(&img).rating, load(&v2).rating), (4, 2));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn variants_saved_at_once_keep_every_change() {
        use rayon::prelude::*;
        let dir = std::env::temp_dir().join(format!("sk-race-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("r.ARW");
        let mut all = vec![img.clone()];
        for _ in 0..15 {
            all.push(add_variant(&img, &Sidecar::default()).unwrap());
        }
        for round in 1..=3u8 {
            all.par_iter().for_each(|p| save(p, &Sidecar { rating: round, ..Default::default() }).unwrap());
            assert!(all.iter().all(|p| load(p).rating == round), "Runde {round}");
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn variants_live_in_the_raw_sidecar() {
        let dir = std::env::temp_dir().join(format!("sk-variant-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("c.ARW");
        save(&img, &Sidecar { rating: 2, ..Default::default() }).unwrap();
        let mut bw = Sidecar { rating: 5, ..Default::default() };
        bw.adjustments.black_white.enabled = true;
        let v2 = add_variant(&img, &bw).unwrap();
        assert_eq!(crate::variant::number(&v2), 2);
        assert_eq!(sidecar_path(&v2), sidecar_path(&img));
        let v3 = add_variant(&v2, &Sidecar::default()).unwrap();
        assert_eq!(crate::variant::number(&v3), 3);
        assert_eq!(crate::variant::of_file(&img), vec![img.clone(), v2.clone(), v3.clone()]);
        // Each variant has its own settings ...
        assert_eq!(load(&v2).rating, 5);
        assert!(load(&v2).adjustments.black_white.enabled);
        assert_eq!(load(&img).rating, 2);
        // ... and saving one keeps the others, also saving the file's own settings without the list.
        save(&v2, &Sidecar { rating: 1, ..Default::default() }).unwrap();
        save(&img, &Sidecar { rating: 4, ..Default::default() }).unwrap();
        assert_eq!(load(&v2).rating, 1);
        assert_eq!(load(&img).rating, 4);
        assert_eq!(load(&img).variants.len(), 2);
        // Default settings of the file don't remove the sidecar while variants exist.
        save(&img, &Sidecar::default()).unwrap();
        assert!(sidecar_path(&img).exists());
        remove_variant(&v2).unwrap();
        assert!(remove_variant(&img).is_err());
        assert_eq!(crate::variant::of_file(&img), vec![img.clone(), v3.clone()]);
        // A new variant never reuses a number still in use.
        assert_eq!(crate::variant::number(&add_variant(&img, &Sidecar::default()).unwrap()), 4);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn reads_and_replaces_legacy_sidecar() {
        let dir = std::env::temp_dir().join(format!("sk-legacy-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("b.ARW");
        std::fs::write(legacy_sidecar_path(&img), r#"{"version":1,"rating":3}"#).unwrap();
        let mut sc = load(&img);
        assert_eq!(sc.rating, 3);
        sc.rating = 5;
        save(&img, &sc).unwrap();
        assert!(!legacy_sidecar_path(&img).exists());
        assert_eq!(load(&img).rating, 5);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn legacy_sidecar_stays_when_writing_fails() {
        let dir = std::env::temp_dir().join(format!("sk-legacy-fail-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("c.ARW");
        std::fs::write(legacy_sidecar_path(&img), r#"{"version":1,"rating":3}"#).unwrap();
        // A folder in place of the new sidecar: writing it fails.
        std::fs::create_dir_all(sidecar_path(&img).join("x")).unwrap();
        let sc = Sidecar { rating: 5, ..load(&img) };
        assert!(save(&img, &sc).is_err());
        assert!(legacy_sidecar_path(&img).exists());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn damaged_sidecar_is_kept_as_backup() {
        let dir = std::env::temp_dir().join(format!("sk-damaged-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("d.ARW");
        std::fs::write(sidecar_path(&img), "{ \"variants\": [ kaputt").unwrap();
        save(&img, &Sidecar { rating: 3, ..Default::default() }).unwrap();
        assert_eq!(load(&img).rating, 3);
        let backup = dir.join("d.ARW.sk.json.bak");
        assert!(std::fs::read_to_string(backup).unwrap().contains("kaputt"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn newer_sidecar_is_not_overwritten() {
        let dir = std::env::temp_dir().join(format!("sk-newer-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("n.ARW");
        let text = format!(r#"{{"version":{},"rating":4,"zukunft":{{"a":1}}}}"#, VERSION + 1);
        std::fs::write(sidecar_path(&img), &text).unwrap();
        // Read as far as understood ...
        assert_eq!(load(&img).rating, 4);
        // ... but every change is refused and the file stays as it is.
        let err = save(&img, &Sidecar { rating: 1, ..Default::default() }).unwrap_err();
        assert!(format!("{err:#}").contains("neueren Silberkorn-Version"));
        assert!(add_variant(&img, &Sidecar::default()).is_err());
        assert!(set_stack(&img, None).is_err());
        assert_eq!(std::fs::read_to_string(sidecar_path(&img)).unwrap(), text);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn newer_sidecar_with_unknown_values_is_not_overwritten() {
        let dir = std::env::temp_dir().join(format!("sk-newer-enum-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("u.ARW");
        let text = format!(r#"{{"version":{},"rating":4,"color_label":"Türkis"}}"#, VERSION + 1);
        std::fs::write(sidecar_path(&img), &text).unwrap();
        let err = save(&img, &Sidecar { rating: 1, ..Default::default() }).unwrap_err();
        assert!(format!("{err:#}").contains("neueren Silberkorn-Version"));
        assert!(variant_ids(&img).is_err());
        assert_eq!(std::fs::read_to_string(sidecar_path(&img)).unwrap(), text);
        assert!(!dir.join("u.ARW.sk.json.bak").exists());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn damaged_sidecars_keep_every_backup() {
        let dir = std::env::temp_dir().join(format!("sk-bak2-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("b.ARW");
        std::fs::write(sidecar_path(&img), "erst kaputt").unwrap();
        save(&img, &Sidecar { rating: 3, ..Default::default() }).unwrap();
        std::fs::write(sidecar_path(&img), "wieder kaputt").unwrap();
        save(&img, &Sidecar { rating: 2, ..Default::default() }).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("b.ARW.sk.json.bak")).unwrap(), "erst kaputt");
        assert_eq!(std::fs::read_to_string(dir.join("b.ARW.sk.json.2.bak")).unwrap(), "wieder kaputt");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn load_all_matches_loading_one_by_one() {
        let dir = std::env::temp_dir().join(format!("sk-loadall-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.ARW"), dir.join("b.ARW"));
        for p in [&a, &b] {
            std::fs::write(p, b"raw").unwrap();
        }
        save(&a, &Sidecar { rating: 2, ..Default::default() }).unwrap();
        add_variant(&a, &Sidecar { rating: 4, ..Default::default() }).unwrap();
        add_variant(&a, &Sidecar { rating: 5, ..Default::default() }).unwrap();
        let files = vec![a.clone(), b.clone()];
        let one_by_one: Vec<(PathBuf, Sidecar)> = crate::variant::expand(files.clone()).into_iter().map(|p| { let sc = load(&p); (p, sc) }).collect();
        assert_eq!(one_by_one.len(), 4);
        assert_eq!(load_all(&files), one_by_one);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn failed_write_keeps_the_old_sidecar() {
        let dir = std::env::temp_dir().join(format!("sk-fullsc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("f.ARW");
        save(&img, &Sidecar { rating: 2, ..Default::default() }).unwrap();
        crate::safe_file::fail_writes_after(Some(10));
        let result = save(&img, &Sidecar { rating: 5, ..Default::default() });
        crate::safe_file::fail_writes_after(None);
        assert!(result.is_err());
        assert_eq!(load(&img).rating, 2);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn xmp_changes_override_rating_label_keywords() {
        let dir = std::env::temp_dir().join(format!("sk-xmp-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("x.ARW");
        let xmp = dir.join("x.xmp");
        let write_xmp = |text: String, secs: u64| {
            std::fs::write(&xmp, text).unwrap();
            let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
            std::fs::File::options().write(true).open(&xmp).unwrap().set_modified(t).unwrap();
        };
        write_xmp(crate::xmp::updated(None, 4, ColorLabel::Red, &["Urlaub".into()]).unwrap(), 1_000_000);
        let sc = load(&img);
        assert_eq!((sc.rating, sc.color_label, sc.keywords.clone()), (4, ColorLabel::Red, vec!["Urlaub".to_string()]));
        // Changed in Silberkorn: the older XMP no longer counts.
        save(&img, &Sidecar { rating: 1, ..sc }).unwrap();
        assert_eq!(load(&img).rating, 1);
        // Changed again elsewhere: the XMP wins, the adjustments stay.
        let mut sc = load(&img);
        sc.adjustments.exposure = 0.3;
        save(&img, &sc).unwrap();
        write_xmp(crate::xmp::updated(None, 5, ColorLabel::Blue, &[]).unwrap(), 2_000_000);
        let sc = load(&img);
        assert_eq!((sc.rating, sc.color_label, sc.adjustments.exposure), (5, ColorLabel::Blue, 0.3));
        // Changed elsewhere while open here, and only the adjustments are saved: the change
        // isn't lost.
        save(&img, &sc).unwrap();
        let mut open = load(&img);
        write_xmp(crate::xmp::updated(None, 3, ColorLabel::Green, &[]).unwrap(), 2_500_000);
        open.adjustments.exposure = 0.4;
        save(&img, &open).unwrap();
        let sc = load(&img);
        assert_eq!((sc.rating, sc.color_label, sc.adjustments.exposure), (3, ColorLabel::Green, 0.4));
        // Variants keep their own values.
        let v2 = add_variant(&img, &Sidecar { rating: 2, ..Default::default() }).unwrap();
        assert_eq!(load(&v2).rating, 2);
        // On request the XMP is read again although it is older.
        save(&img, &Sidecar { rating: 0, ..load(&img) }).unwrap();
        assert_eq!(load(&img).rating, 0);
        reread_xmp(&img).unwrap();
        assert_eq!(load(&img).rating, 3);
        std::fs::remove_dir_all(dir).ok();
    }
}
