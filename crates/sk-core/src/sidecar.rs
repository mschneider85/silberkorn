//! Per-image sidecar files (`<image>.sk.json`; older `<image>.crs.json` files are still read) holding adjustments and metadata.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::params::Adjustments;

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
            version: 1,
            rating: self.rating,
            color_label: self.color_label,
            keywords: self.keywords.clone(),
            adjustments: self.adjustments.clone(),
            variants: Vec::new(),
            snapshots: self.snapshots.clone(),
            xmp_mtime: None,
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
    let file = load_file(image);
    match crate::variant::number(image) {
        1 => with_xmp(image, file),
        n => file.variants.iter().find(|v| v.id == n).map(Variant::to_sidecar).unwrap_or_default(),
    }
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
    let mut file = load_for_update(image)?;
    file.xmp_mtime = crate::xmp::write(image, sc.rating, sc.color_label, &sc.keywords)?;
    (file.rating, file.color_label, file.keywords) = (sc.rating, sc.color_label, sc.keywords);
    write_file(image, &file)
}

fn load_file(image: &Path) -> Sidecar {
    read_file(image).ok().flatten().unwrap_or_default()
}

/// The sidecar file of an image: `None` if there is none, an error if it can't be read.
fn read_file(image: &Path) -> Result<Option<Sidecar>> {
    for path in json_files_for(image) {
        match std::fs::read_to_string(&path) {
            Ok(text) => return Ok(Some(serde_json::from_str(&text).with_context(|| format!("{} ist beschädigt", path.display()))?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("{} kann nicht gelesen werden", path.display())),
        }
    }
    Ok(None)
}

/// The sidecar file before changing it. One that can't be parsed is kept as `.bak` (it may hold
/// the settings of all variants) instead of being overwritten without notice.
fn load_for_update(image: &Path) -> Result<Sidecar> {
    match read_file(image) {
        Ok(sc) => Ok(sc.unwrap_or_default()),
        Err(e) if e.chain().any(|c| c.is::<serde_json::Error>()) => {
            let _ = e;
            for path in json_files_for(image).into_iter().filter(|p| p.exists()) {
                let mut backup = path.clone().into_os_string();
                backup.push(".bak");
                std::fs::rename(&path, &backup).with_context(|| format!("{} kann nicht gesichert werden", path.display()))?;
            }
            Ok(Sidecar::default())
        }
        Err(e) => Err(e),
    }
}

/// Stores the settings of an image or variant. The list of further variants and the snapshots
/// are kept as they are (they change only through [`add_variant`], [`remove_variant`] and
/// [`set_snapshots`]).
pub fn save(image: &Path, sidecar: &Sidecar) -> Result<()> {
    let mut file = load_for_update(image)?;
    match crate::variant::number(image) {
        1 => {
            // From now on these settings count, not an older XMP file (written along if enabled).
            let xmp_mtime = if crate::xmp::write_enabled() {
                crate::xmp::write(image, sidecar.rating, sidecar.color_label, &sidecar.keywords).unwrap_or_else(|e| {
                    // The settings themselves are still saved.
                    log::warn!("{e:#}");
                    crate::xmp::mtime(image)
                })
            } else {
                crate::xmp::mtime(image)
            };
            file = Sidecar {
                variants: std::mem::take(&mut file.variants),
                snapshots: std::mem::take(&mut file.snapshots),
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

/// Adds a variant with these settings to an image; returns its path.
pub fn add_variant(image: &Path, settings: &Sidecar) -> Result<PathBuf> {
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
    let mut file = load_for_update(variant)?;
    file.variants.retain(|v| v.id != n);
    write_file(variant, &file)
}

fn write_file(image: &Path, sidecar: &Sidecar) -> Result<()> {
    let path = sidecar_path(image);
    let legacy = legacy_sidecar_path(image);
    if legacy.exists() {
        std::fs::remove_file(legacy)?;
    }
    if (Sidecar { version: 0, ..sidecar.clone() }) == Sidecar::default() {
        // Nothing to store; remove a stale file if present.
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        return Ok(());
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&Sidecar { version: 1, ..sidecar.clone() })?)?;
    std::fs::rename(tmp, path)?;
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
        write_xmp(crate::xmp::updated(None, 4, ColorLabel::Red, &["Lightroom".into()]).unwrap(), 1_000_000);
        let sc = load(&img);
        assert_eq!((sc.rating, sc.color_label, sc.keywords.clone()), (4, ColorLabel::Red, vec!["Lightroom".to_string()]));
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
        // Variants keep their own values.
        let v2 = add_variant(&img, &Sidecar { rating: 2, ..Default::default() }).unwrap();
        assert_eq!(load(&v2).rating, 2);
        // On request the XMP is read again although it is older.
        save(&img, &Sidecar { rating: 0, ..load(&img) }).unwrap();
        assert_eq!(load(&img).rating, 0);
        reread_xmp(&img).unwrap();
        assert_eq!(load(&img).rating, 5);
        std::fs::remove_dir_all(dir).ok();
    }
}
