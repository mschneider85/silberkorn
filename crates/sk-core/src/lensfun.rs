//! Lens profiles from the lensfun database (XML files, CC BY-SA 3.0, not bundled).
//!
//! Supported models: distortion `ptlens`/`poly3`/`poly5`, TCA `linear`/`poly3`,
//! vignetting `pa`. Coefficients are interpolated linearly over focal length.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::params::LensCoefficients;

#[derive(Clone, Debug, Default)]
pub struct LensProfile {
    pub maker: String,
    pub model: String,
    pub crop_factor: f32,
    /// (focal, [a, b, c, poly5 k1, poly5 k2]); poly3 is expressed in ptlens form.
    distortion: Vec<(f32, [f32; 5])>,
    /// (focal, vr - 1, vb - 1)
    tca: Vec<(f32, f32, f32)>,
    /// (focal, aperture, distance, k1, k2, k3)
    vignetting: Vec<(f32, f32, f32, f32, f32, f32)>,
}

impl LensProfile {
    pub fn name(&self) -> String {
        format!("{} {}", self.maker, self.model)
    }

    /// Coefficients for a focal length (mm) and aperture (f-number).
    pub fn coefficients(&self, focal: f32, aperture: Option<f32>) -> LensCoefficients {
        let mut c = LensCoefficients::default();
        if let Some([a, b, cc, k1, k2]) = interpolate(&self.distortion, focal) {
            (c.a, c.b, c.c, c.poly5_k1, c.poly5_k2) = (a, b, cc, k1, k2);
        }
        if let Some([r, b]) = interpolate(&self.tca.iter().map(|t| (t.0, [t.1, t.2])).collect::<Vec<_>>(), focal) {
            c.tca_red = r;
            c.tca_blue = b;
        }
        if !self.vignetting.is_empty() {
            // Nearest aperture, farthest distance, then interpolate over focal length.
            let ap = aperture.unwrap_or(8.0);
            let best_ap = self
                .vignetting
                .iter()
                .map(|v| v.1)
                .min_by(|x, y| (x.ln() - ap.ln()).abs().total_cmp(&(y.ln() - ap.ln()).abs()))
                .unwrap_or(ap);
            // Per focal length, the entry with the largest focus distance.
            let mut rows: Vec<(f32, f32, [f32; 3])> = Vec::new();
            for v in self.vignetting.iter().filter(|v| (v.1 - best_ap).abs() < 1e-3) {
                match rows.iter_mut().find(|r| (r.0 - v.0).abs() < 1e-3) {
                    Some(r) if v.2 > r.1 => *r = (v.0, v.2, [v.3, v.4, v.5]),
                    Some(_) => {}
                    None => rows.push((v.0, v.2, [v.3, v.4, v.5])),
                }
            }
            let rows: Vec<(f32, [f32; 3])> = rows.into_iter().map(|r| (r.0, r.2)).collect();
            if let Some([k1, k2, k3]) = interpolate(&rows, focal) {
                c.vig_k1 = k1;
                c.vig_k2 = k2;
                c.vig_k3 = k3;
            }
        }
        c
    }
}

/// Linear interpolation over the first tuple element (sorted internally).
fn interpolate<const N: usize>(rows: &[(f32, [f32; N])], x: f32) -> Option<[f32; N]> {
    if rows.is_empty() {
        return None;
    }
    let mut rows = rows.to_vec();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    if x <= rows[0].0 {
        return Some(rows[0].1);
    }
    if x >= rows[rows.len() - 1].0 {
        return Some(rows[rows.len() - 1].1);
    }
    let i = rows.partition_point(|r| r.0 <= x) - 1;
    let (x0, a) = rows[i];
    let (x1, b) = rows[i + 1];
    let t = if x1 > x0 { (x - x0) / (x1 - x0) } else { 0.0 };
    Some(std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t))
}

fn attr(n: &roxmltree::Node, name: &str) -> Option<f32> {
    n.attribute(name).and_then(|v| v.trim().parse().ok())
}

/// Text of a child element, preferring the untranslated variant.
fn child_text(n: &roxmltree::Node, name: &str) -> Option<String> {
    let mut fallback = None;
    for c in n.children().filter(|c| c.has_tag_name(name)) {
        let text = c.text().unwrap_or("").trim().to_string();
        if c.attribute("lang").is_none() {
            return Some(text);
        }
        fallback.get_or_insert(text);
    }
    fallback
}

pub fn parse(xml: &str) -> Result<Vec<LensProfile>> {
    let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(xml, opts).context("Lensfun-XML ungültig")?;
    let mut out = Vec::new();
    for lens in doc.descendants().filter(|n| n.has_tag_name("lens")) {
        let mut p = LensProfile {
            maker: child_text(&lens, "maker").unwrap_or_default(),
            model: child_text(&lens, "model").unwrap_or_default(),
            crop_factor: child_text(&lens, "cropfactor").and_then(|s| s.parse().ok()).unwrap_or(1.0),
            ..Default::default()
        };
        for cal in lens.children().filter(|c| c.has_tag_name("calibration")) {
            for e in cal.children().filter(|c| c.is_element()) {
                let Some(focal) = attr(&e, "focal") else { continue };
                match (e.tag_name().name(), e.attribute("model")) {
                    ("distortion", Some("ptlens")) => p.distortion.push((
                        focal,
                        [attr(&e, "a").unwrap_or(0.0), attr(&e, "b").unwrap_or(0.0), attr(&e, "c").unwrap_or(0.0), 0.0, 0.0],
                    )),
                    // poly3: r_d = r_u (1 - k1 + k1 r^2) is ptlens with b = k1.
                    ("distortion", Some("poly3")) => p.distortion.push((focal, [0.0, attr(&e, "k1").unwrap_or(0.0), 0.0, 0.0, 0.0])),
                    // poly5: r_d = r_u (1 + k1 r^2 + k2 r^4)
                    ("distortion", Some("poly5")) => {
                        p.distortion.push((focal, [0.0, 0.0, 0.0, attr(&e, "k1").unwrap_or(0.0), attr(&e, "k2").unwrap_or(0.0)]))
                    }
                    ("tca", Some("linear")) => {
                        p.tca.push((focal, attr(&e, "kr").unwrap_or(1.0) - 1.0, attr(&e, "kb").unwrap_or(1.0) - 1.0));
                    }
                    ("tca", Some("poly3")) => {
                        p.tca.push((focal, attr(&e, "vr").unwrap_or(1.0) - 1.0, attr(&e, "vb").unwrap_or(1.0) - 1.0));
                    }
                    ("vignetting", Some("pa")) => p.vignetting.push((
                        focal,
                        attr(&e, "aperture").unwrap_or(8.0),
                        attr(&e, "distance").unwrap_or(1000.0),
                        attr(&e, "k1").unwrap_or(0.0),
                        attr(&e, "k2").unwrap_or(0.0),
                        attr(&e, "k3").unwrap_or(0.0),
                    )),
                    _ => {}
                }
            }
        }
        if !p.model.is_empty() {
            out.push(p);
        }
    }
    Ok(out)
}

/// Directories searched for lensfun XML files.
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![crate::catalog::default_catalog_path().with_file_name("lensfun")];
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join(".local/share/lensfun/updates/version_1"));
        dirs.push(home.join(".local/share/lensfun"));
    }
    dirs.push(PathBuf::from("/usr/share/lensfun/version_1"));
    dirs.push(PathBuf::from("/usr/share/lensfun"));
    dirs.push(PathBuf::from("/usr/local/share/lensfun/version_1"));
    dirs
}

pub fn load_dir(dir: &Path) -> Vec<LensProfile> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "xml")
            && let Ok(xml) = std::fs::read_to_string(&p)
            && let Ok(mut lenses) = parse(&xml)
        {
            out.append(&mut lenses);
        }
    }
    out
}

/// Loads all profiles from the first directories that contain any.
pub fn load_database() -> Vec<LensProfile> {
    let mut all = Vec::new();
    for d in search_dirs() {
        all.extend(load_dir(&d));
    }
    all
}

/// Downloads the lensfun XML files for common mounts into the Silberkorn data folder.
pub fn download_database() -> Result<PathBuf> {
    const FILES: &[&str] = &[
        "mil-sony.xml", "slr-sony.xml", "mil-canon.xml", "slr-canon.xml", "mil-nikon.xml", "slr-nikon.xml",
        "mil-fujifilm.xml", "mil-panasonic.xml", "mil-olympus.xml", "slr-pentax.xml", "mil-sigma.xml", "slr-sigma.xml",
        "mil-tamron.xml", "slr-tamron.xml", "mil-samyang.xml", "slr-samyang.xml", "mil-zeiss.xml", "slr-zeiss.xml",
        "mil-tokina.xml", "slr-tokina.xml", "mil-leica.xml", "compact-sony.xml", "mil-viltrox.xml", "mil-7artisans.xml",
    ];
    let dir = search_dirs().remove(0);
    std::fs::create_dir_all(&dir)?;
    let mut ok = 0;
    for f in FILES {
        let url = format!("https://raw.githubusercontent.com/lensfun/lensfun/master/data/db/{f}");
        if let Ok(mut resp) = ureq::get(&url).call()
            && let Ok(body) = resp.body_mut().read_to_string()
            && parse(&body).is_ok()
        {
            crate::safe_file::write_nosync(&dir.join(f), body)?;
            ok += 1;
        }
    }
    anyhow::ensure!(ok > 0, "Download der Objektivdatenbank fehlgeschlagen");
    Ok(dir)
}

fn tokens(s: &str) -> Vec<String> {
    s.to_lowercase()
        .replace("f/", "f")
        .replace(['(', ')', ',', '/'], " ")
        .split_whitespace()
        .map(|t| t.trim_end_matches(".0").to_string())
        .collect()
}

/// Finds the best matching profile for an EXIF lens name.
pub fn find<'a>(db: &'a [LensProfile], lens: &str) -> Option<&'a LensProfile> {
    let want = tokens(lens);
    let key = |t: &str| t.contains("mm") || (t.starts_with('f') && t[1..].chars().next().is_some_and(|c| c.is_ascii_digit()));
    db.iter()
        .filter_map(|p| {
            let have = tokens(&format!("{} {}", p.maker, p.model));
            // Focal range and aperture tokens must all match.
            if !have.iter().filter(|t| key(t)).all(|t| want.contains(t)) || !want.iter().filter(|t| key(t)).all(|t| have.contains(t)) {
                return None;
            }
            let common = have.iter().filter(|t| want.contains(t)).count();
            let score = common as f32 / have.len().max(want.len()) as f32;
            (score > 0.5).then_some((score, p))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, p)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<lensdatabase>
        <lens>
            <maker>Sony</maker>
            <model>FE 16-35mm f/2.8 GM</model>
            <model lang="de">FE 16-35mm f/2.8 GM (de)</model>
            <cropfactor>1.0</cropfactor>
            <calibration>
                <distortion model="ptlens" focal="16" a="0.02" b="-0.05" c="0.0"/>
                <distortion model="ptlens" focal="35" a="0.0" b="0.01" c="0.0"/>
                <tca model="poly3" focal="16" vr="1.0002" vb="0.9998"/>
                <vignetting model="pa" focal="16" aperture="2.8" distance="1" k1="-0.5" k2="0" k3="0"/>
                <vignetting model="pa" focal="16" aperture="2.8" distance="100" k1="-0.4" k2="0" k3="0"/>
                <vignetting model="pa" focal="16" aperture="8" distance="100" k1="-0.1" k2="0" k3="0"/>
            </calibration>
        </lens>
        <lens>
            <maker>Sony</maker>
            <model>FE 16-35mm f/4 ZA OSS</model>
        </lens>
    </lensdatabase>"#;

    #[test]
    fn parse_and_interpolate() {
        let db = parse(XML).unwrap();
        assert_eq!(db.len(), 2);
        let p = &db[0];
        assert_eq!(p.model, "FE 16-35mm f/2.8 GM");
        let c = p.coefficients(25.5, Some(2.8));
        assert!((c.a - 0.01).abs() < 1e-5 && (c.b - (-0.02)).abs() < 1e-5);
        assert!((c.tca_red - 0.0002).abs() < 1e-6);
        // Farthest distance at the nearest aperture.
        assert!((c.vig_k1 + 0.4).abs() < 1e-5);
        assert!((p.coefficients(16.0, Some(11.0)).vig_k1 + 0.1).abs() < 1e-5);
    }

    #[test]
    fn matches_exif_lens_names() {
        let db = parse(XML).unwrap();
        assert_eq!(find(&db, "Sony FE 16-35mm F2.8 GM").unwrap().model, "FE 16-35mm f/2.8 GM");
        assert_eq!(find(&db, "FE 16-35mm F4 ZA OSS").unwrap().model, "FE 16-35mm f/4 ZA OSS");
        assert!(find(&db, "FE 24-70mm F2.8 GM").is_none());
    }

    #[test]
    fn real_database_if_available() {
        let dir = std::env::var_os("SK_LENSFUN_TEST_DIR");
        let Some(dir) = dir else { return };
        let db = load_dir(Path::new(&dir));
        assert!(db.len() > 50);
        let p = find(&db, "Sony FE 16-35mm F2.8 GM").expect("profile");
        let c = p.coefficients(32.0, Some(4.0));
        assert!(c.a != 0.0 || c.b != 0.0);
    }
}
