//! Per-image sidecar files (`<image>.crs.json`) holding adjustments and metadata.

use std::path::{Path, PathBuf};

use anyhow::Result;
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
    pub adjustments: Adjustments,
}

pub fn sidecar_path(image: &Path) -> PathBuf {
    let mut name = image.file_name().unwrap_or_default().to_os_string();
    name.push(".crs.json");
    image.with_file_name(name)
}

pub fn load(image: &Path) -> Sidecar {
    std::fs::read_to_string(sidecar_path(image))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(image: &Path, sidecar: &Sidecar) -> Result<()> {
    let path = sidecar_path(image);
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
        let dir = std::env::temp_dir().join(format!("crs-test-{}", std::process::id()));
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
}
