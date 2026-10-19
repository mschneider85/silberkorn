//! Styles: named adjustment presets stored as JSON files.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::params::{Adjustments, BaseCurve, ColorWheel, Curve, HslBand};
use crate::tools::Tool;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Style {
    pub name: String,
    pub adjustments: Adjustments,
    /// The tools the style sets; the image keeps all others, so styles can be applied on top of
    /// each other. Styles saved before this existed have none and set everything but the
    /// image-specific settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
}

pub fn styles_dir() -> PathBuf {
    crate::paths::config_dir().join("styles")
}

fn file_name(name: &str) -> String {
    let clean: String = name.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).collect();
    let clean = clean.trim();
    if crate::naming::is_reserved(clean) { format!("_{clean}.json") } else { format!("{clean}.json") }
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

/// The file holding the style called `name` (the name inside the file counts: different names
/// can give the same file name, e.g. "S/W" and "S_W").
fn file_of(dir: &Path, name: &str) -> Option<std::path::PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).find(|p| {
        std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str::<Style>(&t).ok()).is_some_and(|s| s.name == name)
    })
}

/// Saves a style: replaces the one of the same name, never another one whose name gives the same
/// file name (that gets a number appended).
pub fn save_in(dir: &Path, style: &Style) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = file_of(dir, &style.name).unwrap_or_else(|| {
        let base = file_name(&style.name);
        let stem = base.trim_end_matches(".json");
        std::iter::once(base.clone()).chain((2..).map(|n| format!("{stem} {n}.json"))).map(|f| dir.join(f)).find(|p| !p.exists()).expect("unbounded")
    });
    crate::safe_file::write(&path, serde_json::to_string_pretty(style)?)?;
    Ok(())
}

pub fn delete_in(dir: &Path, name: &str) -> Result<()> {
    let path = file_of(dir, name).with_context(|| format!("Stil „{name}“ nicht gefunden"))?;
    std::fs::remove_file(path)?;
    Ok(())
}

/// Applies a style to existing adjustments: only its tools, or (styles without a tool list)
/// everything but geometry, layers and retouching.
pub fn apply(style: &Style, target: &Adjustments) -> Adjustments {
    if let Some(tools) = &style.tools {
        return crate::tools::apply(tools, &style.adjustments, target);
    }
    let mut out = style.adjustments.clone();
    out.keep_geometry_from(target);
    out
}

/// Applies only the look of a style (tone curve, contrast, colors, black & white, grain);
/// exposure, white balance, highlight/shadow corrections, detail, geometry, lens corrections,
/// layers and retouching of the image are kept. Used for the film simulations.
pub fn apply_look(style: &Style, target: &Adjustments) -> Adjustments {
    let l = &style.adjustments;
    let mut out = target.clone();
    out.base_curve = l.base_curve;
    out.contrast = l.contrast;
    out.brightness = l.brightness;
    out.saturation = l.saturation;
    out.vibrance = l.vibrance;
    out.levels = l.levels.clone();
    out.curves = l.curves.clone();
    out.color_editor = l.color_editor.clone();
    out.color_balance = l.color_balance;
    out.black_white = l.black_white.clone();
    out.detail.grain_amount = l.detail.grain_amount;
    out.detail.grain_size = l.detail.grain_size;
    out
}

// Hue bands of the color editor (see `params::HUE_BANDS`).
const RED: usize = 0;
const ORANGE: usize = 1;
const YELLOW: usize = 2;
const GREEN: usize = 3;
const CYAN: usize = 4;
const BLUE: usize = 5;
const MAGENTA: usize = 7;

// Color wheel angles (Oklab hue).
const WARM: f32 = 70.0;
const TEAL: f32 = 205.0;
const COOL_BLUE: f32 = 250.0;
const MINT: f32 = 165.0;
const ROSE: f32 = 345.0;

fn band(hue: f32, saturation: f32, lightness: f32) -> HslBand {
    HslBand { hue, saturation, lightness }
}

fn wheel(hue: f32, amount: f32) -> ColorWheel {
    ColorWheel { hue, amount, lightness: 0.0 }
}

fn curve(points: &[[f32; 2]]) -> Curve {
    Curve { points: points.to_vec() }
}

fn film(name: &str, f: impl FnOnce(&mut Adjustments)) -> Style {
    let mut a = Adjustments::default();
    a.detail.grain_amount = 0.0;
    f(&mut a);
    Style { name: name.into(), adjustments: a, tools: None }
}

/// Built-in looks inspired by well-known film stocks (approximations, not official
/// simulations), with a short description. Applied with [`apply_look`].
pub fn film_simulations() -> Vec<(Style, &'static str)> {
    vec![
        (
            film("Kodak Portra 400", |a| {
                a.contrast = -10.0;
                a.saturation = -8.0;
                a.vibrance = 6.0;
                a.curves.rgb = curve(&[[0.0, 0.03], [0.25, 0.24], [0.75, 0.77], [1.0, 0.97]]);
                let b = &mut a.color_editor.bands;
                b[RED] = band(2.0, -5.0, 0.0);
                b[ORANGE] = band(0.0, 6.0, 6.0);
                b[YELLOW] = band(-5.0, -10.0, 0.0);
                b[GREEN] = band(-10.0, -25.0, -5.0);
                b[CYAN] = band(0.0, -15.0, 0.0);
                b[BLUE] = band(-5.0, -15.0, 0.0);
                a.color_balance.highlights = wheel(WARM, 15.0);
                a.color_balance.shadows = wheel(TEAL, 8.0);
                (a.detail.grain_amount, a.detail.grain_size) = (12.0, 30.0);
            }),
            "Weiche Kontraste, warme Hauttöne, gedämpftes Grün – der Porträtfilm",
        ),
        (
            film("Kodak Ektar 100", |a| {
                a.contrast = 20.0;
                a.saturation = 18.0;
                a.vibrance = 10.0;
                let b = &mut a.color_editor.bands;
                b[RED] = band(0.0, 15.0, 0.0);
                b[ORANGE] = band(-2.0, 5.0, 0.0);
                b[CYAN] = band(5.0, 10.0, 0.0);
                b[BLUE] = band(-5.0, 20.0, -5.0);
                (a.detail.grain_amount, a.detail.grain_size) = (5.0, 15.0);
            }),
            "Kräftige Farben, satte Rot- und Blautöne, sehr feines Korn",
        ),
        (
            film("Kodachrome 64", |a| {
                a.contrast = 25.0;
                a.saturation = 10.0;
                a.curves.rgb = curve(&[[0.0, 0.0], [0.2, 0.16], [0.8, 0.82], [1.0, 1.0]]);
                let b = &mut a.color_editor.bands;
                b[RED] = band(0.0, 15.0, -5.0);
                b[ORANGE] = band(-3.0, 0.0, 0.0);
                b[YELLOW] = band(0.0, 10.0, -5.0);
                b[GREEN] = band(-8.0, -10.0, -10.0);
                b[CYAN] = band(10.0, -5.0, 0.0);
                b[BLUE] = band(0.0, 20.0, -15.0);
                a.color_balance.shadows = wheel(COOL_BLUE, 6.0);
                a.color_balance.highlights = wheel(WARM, 10.0);
                (a.detail.grain_amount, a.detail.grain_size) = (10.0, 25.0);
            }),
            "Klassischer Diafilm: tiefe Blautöne, warme Rottöne, kräftiger Kontrast",
        ),
        (
            film("Fuji Velvia 50", |a| {
                a.base_curve = BaseCurve::FilmHigh;
                a.contrast = 15.0;
                a.saturation = 30.0;
                a.vibrance = 15.0;
                let b = &mut a.color_editor.bands;
                b[GREEN] = band(-5.0, 20.0, 0.0);
                b[BLUE] = band(0.0, 20.0, -10.0);
                b[MAGENTA] = band(0.0, 10.0, 0.0);
                a.color_balance.shadows = wheel(ROSE, 8.0);
                (a.detail.grain_amount, a.detail.grain_size) = (4.0, 15.0);
            }),
            "Sehr gesättigter Landschafts-Diafilm mit hohem Kontrast",
        ),
        (
            film("Fuji Provia 100F", |a| {
                a.contrast = 10.0;
                a.saturation = 8.0;
                let b = &mut a.color_editor.bands;
                b[GREEN] = band(0.0, 5.0, 0.0);
                b[BLUE] = band(0.0, 8.0, 0.0);
                (a.detail.grain_amount, a.detail.grain_size) = (4.0, 15.0);
            }),
            "Neutraler Diafilm: natürliche Farben, moderater Kontrast",
        ),
        (
            film("Classic Chrome", |a| {
                a.contrast = 15.0;
                a.saturation = -25.0;
                a.curves.rgb = curve(&[[0.0, 0.02], [0.25, 0.22], [0.75, 0.78], [1.0, 0.98]]);
                let b = &mut a.color_editor.bands;
                b[RED] = band(0.0, -10.0, -10.0);
                b[ORANGE] = band(0.0, -5.0, 0.0);
                b[YELLOW] = band(-8.0, -20.0, 0.0);
                b[GREEN] = band(-10.0, -30.0, -10.0);
                b[BLUE] = band(-10.0, -10.0, -10.0);
                a.color_balance.shadows = wheel(TEAL, 10.0);
                a.color_balance.highlights = wheel(WARM, 6.0);
                (a.detail.grain_amount, a.detail.grain_size) = (8.0, 25.0);
            }),
            "Reportage-Look: zurückgenommene Farben, harte Tonwerte, kühle Schatten",
        ),
        (
            film("Fuji Pro 400H", |a| {
                a.contrast = -15.0;
                a.saturation = -10.0;
                a.curves.rgb = curve(&[[0.0, 0.06], [0.3, 0.31], [0.8, 0.82], [1.0, 0.98]]);
                let b = &mut a.color_editor.bands;
                b[ORANGE] = band(0.0, 0.0, 5.0);
                b[GREEN] = band(10.0, -10.0, 5.0);
                b[CYAN] = band(0.0, 5.0, 0.0);
                b[BLUE] = band(-5.0, -10.0, 0.0);
                a.color_balance.shadows = wheel(MINT, 12.0);
                a.color_balance.highlights = wheel(WARM, 6.0);
                (a.detail.grain_amount, a.detail.grain_size) = (12.0, 30.0);
            }),
            "Pastellig und luftig, mintgrüne Schatten – beliebt für Hochzeiten",
        ),
        (
            film("CineStill 800T", |a| {
                a.contrast = 5.0;
                a.saturation = -5.0;
                a.curves.blue = curve(&[[0.0, 0.05], [0.5, 0.52], [1.0, 1.0]]);
                let b = &mut a.color_editor.bands;
                b[RED] = band(-5.0, 10.0, 0.0);
                b[ORANGE] = band(0.0, -5.0, 0.0);
                b[GREEN] = band(15.0, -20.0, 0.0);
                b[CYAN] = band(0.0, 10.0, 0.0);
                a.color_balance.shadows = wheel(TEAL, 20.0);
                a.color_balance.highlights = wheel(WARM, 10.0);
                (a.detail.grain_amount, a.detail.grain_size) = (25.0, 40.0);
            }),
            "Kunstlicht-Kinofilm: türkise Schatten, warme Lichter, deutliches Korn",
        ),
        (
            film("Ilford HP5 Plus", |a| {
                a.black_white.enabled = true;
                a.contrast = 15.0;
                a.curves.rgb = curve(&[[0.0, 0.02], [0.25, 0.22], [0.75, 0.78], [1.0, 1.0]]);
                (a.detail.grain_amount, a.detail.grain_size) = (30.0, 35.0);
            }),
            "Schwarzweiß: ausgewogene Tonwerte, kräftiges Korn",
        ),
        (
            film("Kodak Tri-X 400", |a| {
                a.black_white.enabled = true;
                a.base_curve = BaseCurve::FilmHigh;
                a.contrast = 30.0;
                a.black_white.mix = [10.0, 10.0, 10.0, 0.0, -10.0, -15.0, -5.0, 0.0];
                (a.detail.grain_amount, a.detail.grain_size) = (35.0, 40.0);
            }),
            "Schwarzweiß: harter Kontrast, dunkle Himmel, grobes Korn – der Reportage-Klassiker",
        ),
        (
            film("Ilford Pan F Plus 50", |a| {
                a.black_white.enabled = true;
                a.contrast = 20.0;
                a.black_white.mix = [5.0, 5.0, 0.0, 0.0, 0.0, -5.0, 0.0, 0.0];
                a.black_white.highlight_tint = wheel(WARM, 4.0);
                (a.detail.grain_amount, a.detail.grain_size) = (6.0, 15.0);
            }),
            "Schwarzweiß: feines Korn, brillante Mitteltöne, leicht warm",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_list_delete() {
        let dir = std::env::temp_dir().join(format!("skstyles-{}", std::process::id()));
        let s = Style { name: "Warm / Kontrast".into(), adjustments: Adjustments { contrast: 20.0, ..Default::default() }, tools: None };
        save_in(&dir, &s).unwrap();
        assert_eq!(list_in(&dir), vec![s.clone()]);
        delete_in(&dir, &s.name).unwrap();
        assert!(list_in(&dir).is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn reserved_windows_names_get_a_prefix() {
        assert_eq!(file_name("Aux"), "_Aux.json");
        assert_eq!(file_name("Auxiliar"), "Auxiliar.json");
    }

    #[test]
    fn names_with_the_same_file_name_stay_apart() {
        let dir = std::env::temp_dir().join(format!("sk-styles-clash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let style = |name: &str, contrast: f32| Style { name: name.into(), adjustments: Adjustments { contrast, ..Default::default() }, tools: None };
        for (i, name) in ["S/W", "S&W", "S_W"].into_iter().enumerate() {
            save_in(&dir, &style(name, i as f32)).unwrap();
        }
        assert_eq!(list_in(&dir).len(), 3);
        // Saving under an existing name replaces that style only.
        save_in(&dir, &style("S&W", 50.0)).unwrap();
        let all = list_in(&dir);
        assert_eq!(all.len(), 3);
        assert_eq!(all.iter().find(|s| s.name == "S&W").unwrap().adjustments.contrast, 50.0);
        // Deleting one leaves the others.
        delete_in(&dir, "S/W").unwrap();
        let names: Vec<String> = list_in(&dir).into_iter().map(|s| s.name).collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"S&W".to_string()) && names.contains(&"S_W".to_string()));
        assert!(delete_in(&dir, "S/W").is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn looks_keep_the_corrections_of_the_image() {
        let mut target = Adjustments { exposure: 0.7, highlights: -40.0, rotation: 1, ..Default::default() };
        target.detail.sharpen_amount = 80.0;
        target.white_balance.kelvin = 4200.0;
        let sims = film_simulations();
        let (portra, _) = &sims[0];
        let r = apply_look(portra, &target);
        assert_eq!((r.exposure, r.highlights, r.rotation, r.detail.sharpen_amount, r.white_balance.kelvin), (0.7, -40.0, 1, 80.0, 4200.0));
        assert_eq!((r.contrast, r.curves.clone()), (portra.adjustments.contrast, portra.adjustments.curves.clone()));
        assert!(r.detail.grain_amount > 0.0);
    }

    #[test]
    fn film_simulations_are_distinct_and_valid() {
        let sims = film_simulations();
        assert!(sims.len() >= 10);
        for (i, (a, desc)) in sims.iter().enumerate() {
            assert!(!desc.is_empty());
            for (b, _) in &sims[i + 1..] {
                assert_ne!(a.name, b.name);
                assert_ne!(a.adjustments, b.adjustments, "{} = {}", a.name, b.name);
            }
            for c in [&a.adjustments.curves.rgb, &a.adjustments.curves.red, &a.adjustments.curves.green, &a.adjustments.curves.blue] {
                assert!(c.points.windows(2).all(|w| w[0][0] < w[1][0]), "{}: Kurvenpunkte sortiert", a.name);
            }
        }
    }

    #[test]
    fn apply_keeps_rotation() {
        let s = Style { name: "x".into(), adjustments: Adjustments { contrast: 20.0, rotation: 2, ..Default::default() }, tools: None };
        let r = apply(&s, &Adjustments { rotation: 1, ..Default::default() });
        assert_eq!((r.contrast, r.rotation), (20.0, 1));
    }

    #[test]
    fn styles_with_tools_stack() {
        let warm = Style {
            name: "Warm".into(),
            adjustments: Adjustments { contrast: 50.0, color_balance: crate::params::ColorBalance { master: wheel(WARM, 20.0), ..Default::default() }, ..Default::default() },
            tools: Some(vec![Tool::ColorBalance]),
        };
        let grain = Style { name: "Korn".into(), adjustments: film("k", |a| a.detail.grain_amount = 40.0).adjustments, tools: Some(vec![Tool::Grain]) };
        let target = Adjustments { exposure: 0.5, contrast: 10.0, ..Default::default() };
        let r = apply(&grain, &apply(&warm, &target));
        assert_eq!((r.exposure, r.contrast, r.detail.grain_amount), (0.5, 10.0, 40.0));
        assert_eq!(r.color_balance.master.amount, 20.0);
        // Saved and read back with its tools.
        let back: Style = serde_json::from_str(&serde_json::to_string(&warm).unwrap()).unwrap();
        assert_eq!(back, warm);
    }
}
