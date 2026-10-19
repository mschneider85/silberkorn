//! File names from templates with placeholders, for batch renaming and export recipes.

use std::path::Path;

use crate::raw::CaptureInfo;

/// Placeholders and what they stand for (for the help text in the UI).
pub const TOKENS: [(&str, &str); 11] = [
    ("{Name}", "ursprünglicher Dateiname ohne Endung"),
    ("{Zähler}", "fortlaufende Nummer, {Zähler:3} mit 3 Stellen"),
    ("{Datum}", "Aufnahmedatum 2026-10-11"),
    ("{Zeit}", "Aufnahmezeit 143015"),
    ("{Jahr}", "Aufnahmejahr"),
    ("{Monat}", "Aufnahmemonat (01–12)"),
    ("{Tag}", "Aufnahmetag (01–31)"),
    ("{Kamera}", "Kameramodell"),
    ("{Objektiv}", "Objektiv"),
    ("{Session}", "Name der Session"),
    ("{Variante}", "Nummer der Variante"),
];

/// Everything a template can refer to.
#[derive(Clone, Copy, Debug)]
pub struct NameFields<'a> {
    /// The image (a variant path is fine, see `variant`).
    pub image: &'a Path,
    pub capture: Option<&'a CaptureInfo>,
    pub counter: u32,
    pub session: &'a str,
}

/// The template with its placeholders replaced, as a valid file name (without extension).
/// Unknown placeholders stay as they are; an empty result falls back to the original name.
pub fn expand(template: &str, f: &NameFields) -> String {
    let (file, variant) = crate::variant::split(f.image);
    let name = file.file_stem().unwrap_or_default().to_string_lossy().to_string();
    // "2026:10:11 14:30:15" (EXIF) -> parts.
    let date = f.capture.and_then(|c| c.date.as_deref()).unwrap_or("");
    let digits: Vec<&str> = date.split([':', ' ', '-', 'T']).filter(|s| !s.is_empty()).collect();
    let part = |i: usize| digits.get(i).copied().unwrap_or("");
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let Some(len) = rest[start..].find('}') else {
            rest = &rest[start..];
            break;
        };
        let token = &rest[start + 1..start + len];
        let (key, arg) = token.split_once(':').unwrap_or((token, ""));
        let value = match key.to_lowercase().as_str() {
            "name" => Some(name.clone()),
            "zähler" | "zaehler" => Some(format!("{:0width$}", f.counter, width = arg.parse::<usize>().unwrap_or(1).min(9))),
            "datum" if digits.len() >= 3 => Some(format!("{}-{}-{}", part(0), part(1), part(2))),
            "zeit" if digits.len() >= 6 => Some(format!("{}{}{}", part(3), part(4), part(5))),
            "jahr" => Some(part(0).to_string()),
            "monat" => Some(part(1).to_string()),
            "tag" => Some(part(2).to_string()),
            "datum" | "zeit" => Some(String::new()),
            "kamera" => Some(f.capture.map(|c| c.model.clone()).unwrap_or_default()),
            "objektiv" => Some(f.capture.and_then(|c| c.lens.clone()).unwrap_or_default()),
            "session" => Some(f.session.to_string()),
            "variante" => Some(variant.to_string()),
            _ => None,
        };
        match value {
            Some(v) => out.push_str(&v),
            None => out.push_str(&rest[start..=start + len]),
        }
        rest = &rest[start + len + 1..];
    }
    out.push_str(rest);
    let clean = sanitize(&out);
    if clean.is_empty() { sanitize(&name) } else { clean }
}

/// Characters that Windows (and partly macOS/Linux) don't allow in file names become `_`;
/// leading and trailing dots and spaces go.
pub fn sanitize(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
    let s = s.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if is_reserved(s) { format!("_{s}") } else { s.to_string() }
}

/// Whether Windows reserves `name` for a device (`CON`, `NUL`, `COM1` …), also with an extension.
pub fn is_reserved(name: &str) -> bool {
    let base = name.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
    let numbered = |prefix: &str| base.len() == 4 && base.starts_with(prefix) && matches!(base.as_bytes()[3], b'1'..=b'9');
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered("COM") || numbered("LPT")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> CaptureInfo {
        CaptureInfo { model: "ILCE-7M3".into(), lens: Some("FE 24-70mm F2.8 GM".into()), date: Some("2026:10:11 14:30:15".into()), ..Default::default() }
    }

    #[test]
    fn expands_placeholders() {
        let c = info();
        let f = NameFields { image: Path::new("/x/_DSC0042.ARW"), capture: Some(&c), counter: 7, session: "Hochzeit Meier" };
        assert_eq!(expand("{Session}_{Datum}_{Zähler:3}", &f), "Hochzeit Meier_2026-10-11_007");
        assert_eq!(expand("{Jahr}{Monat}{Tag}-{Zeit} {Kamera} {Name}", &f), "20261011-143015 ILCE-7M3 _DSC0042");
        assert_eq!(expand("{name}-{unbekannt}", &f), "_DSC0042-{unbekannt}");
        assert_eq!(expand("{Zähler}", &f), "7");
    }

    #[test]
    fn variants_and_missing_data() {
        let v = crate::variant::path_for(Path::new("/x/a.ARW"), 3);
        let f = NameFields { image: &v, capture: None, counter: 1, session: "" };
        assert_eq!(expand("{Name}-v{Variante}", &f), "a-v3");
        // No capture date: the placeholders are empty; nothing left at all: the original name.
        assert_eq!(expand("{Datum}_{Name}", &f), "_a");
        assert_eq!(expand("{Datum}", &f), "a");
    }

    #[test]
    fn file_names_stay_valid() {
        let f = NameFields { image: Path::new("/x/a.ARW"), capture: None, counter: 1, session: "Kunde: A/B?" };
        assert_eq!(expand("{Session}", &f), "Kunde_ A_B_");
        assert_eq!(sanitize(" ..Bild. "), "Bild");
        assert_eq!(expand("{Session}", &NameFields { session: "nul", ..f }), "_nul");
        assert_eq!(sanitize("COM1.tar"), "_COM1.tar");
        assert_eq!(sanitize("Console"), "Console");
        assert_eq!(sanitize("COM0"), "COM0");
    }
}
