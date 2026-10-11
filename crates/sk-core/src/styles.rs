//! Styles: named adjustment presets stored as JSON files.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::params::Adjustments;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub name: String,
    pub adjustments: Adjustments,
}

pub fn styles_dir() -> PathBuf {
    crate::paths::config_dir().join("styles")
}

fn file_name(name: &str) -> String {
    let clean: String = name.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).collect();
    format!("{}.json", clean.trim())
}

pub fn list_in(dir: &Path) -> Vec<Style> {
    let mut styles: Vec<Style> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok())
        .collect();
    styles.sort_by_key(|s| s.name.to_lowercase());
    styles
}

pub fn save_in(dir: &Path, style: &Style) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(file_name(&style.name)), serde_json::to_string_pretty(style)?)?;
    Ok(())
}

pub fn delete_in(dir: &Path, name: &str) -> Result<()> {
    std::fs::remove_file(dir.join(file_name(name)))?;
    Ok(())
}

/// Applies a style to existing adjustments. Geometry (rotation, crop) is kept.
pub fn apply(style: &Style, target: &Adjustments) -> Adjustments {
    let mut out = style.adjustments.clone();
    out.keep_geometry_from(target);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_list_delete() {
        let dir = std::env::temp_dir().join(format!("skstyles-{}", std::process::id()));
        let s = Style { name: "Warm / Kontrast".into(), adjustments: Adjustments { contrast: 20.0, ..Default::default() } };
        save_in(&dir, &s).unwrap();
        assert_eq!(list_in(&dir), vec![s.clone()]);
        delete_in(&dir, &s.name).unwrap();
        assert!(list_in(&dir).is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn apply_keeps_rotation() {
        let s = Style { name: "x".into(), adjustments: Adjustments { contrast: 20.0, rotation: 2, ..Default::default() } };
        let r = apply(&s, &Adjustments { rotation: 1, ..Default::default() });
        assert_eq!((r.contrast, r.rotation), (20.0, 1));
    }
}
