//! XMP sidecars (`IMG_0001.xmp`, as written by Lightroom, Bridge and Capture One, or
//! `IMG_0001.ARW.xmp` as written by darktable): rating, color label and keywords.
//!
//! Reading: [`crate::sidecar::load`] takes these values from the XMP file whenever it changed
//! since Silberkorn last looked at it. Writing (optional, see [`set_write_enabled`]) updates
//! only these properties and keeps everything else in the file (develop settings of other
//! programs, captions, …).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};

use crate::sidecar::ColorLabel;

const NS_RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const NS_XMP: &str = "http://ns.adobe.com/xap/1.0/";
const NS_DC: &str = "http://purl.org/dc/elements/1.1/";

/// Write rating, label and keywords into XMP sidecars whenever an image's sidecar is saved.
static WRITE: AtomicBool = AtomicBool::new(false);

pub fn set_write_enabled(on: bool) {
    WRITE.store(on, Ordering::Relaxed);
}

pub fn write_enabled() -> bool {
    WRITE.load(Ordering::Relaxed)
}

/// The values of an XMP file; `None` where the file says nothing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct XmpMeta {
    pub rating: Option<u8>,
    pub label: Option<ColorLabel>,
    pub keywords: Option<Vec<String>>,
}

/// Candidate XMP files of a raw file, the usual one first.
fn candidates(image: &Path) -> [PathBuf; 2] {
    let file = crate::variant::file_of(image);
    let mut long = file.file_name().unwrap_or_default().to_os_string();
    long.push(".xmp");
    [file.with_extension("xmp"), file.with_file_name(long)]
}

/// All XMP names an image can have, whether they exist or not (they move with the image).
pub fn files_for(image: &Path) -> [PathBuf; 2] {
    candidates(image)
}

/// The existing XMP file of an image.
pub fn existing(image: &Path) -> Option<PathBuf> {
    candidates(image).into_iter().find(|p| p.is_file())
}

/// Modification time (milliseconds) of the image's XMP file, if there is one.
pub fn mtime(image: &Path) -> Option<i64> {
    let path = existing(image)?;
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(t.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64)
}

/// Reads the image's XMP file.
pub fn read(image: &Path) -> Option<XmpMeta> {
    let text = std::fs::read_to_string(existing(image)?).ok()?;
    parse(&text).map_err(|e| log::warn!("XMP von {} nicht lesbar: {e:#}", image.display())).ok()
}

fn label_from_text(s: &str) -> Option<ColorLabel> {
    // Lightroom writes the label's name in the language of its label set.
    Some(match s.trim().to_lowercase().as_str() {
        "" => ColorLabel::None,
        "red" | "rot" => ColorLabel::Red,
        "orange" => ColorLabel::Orange,
        "yellow" | "gelb" => ColorLabel::Yellow,
        "green" | "grün" | "gruen" => ColorLabel::Green,
        "blue" | "blau" => ColorLabel::Blue,
        "pink" => ColorLabel::Pink,
        "purple" | "lila" | "violett" => ColorLabel::Purple,
        _ => return None,
    })
}

fn label_to_text(l: ColorLabel) -> Option<&'static str> {
    Some(match l {
        ColorLabel::None => return None,
        ColorLabel::Red => "Red",
        ColorLabel::Orange => "Orange",
        ColorLabel::Yellow => "Yellow",
        ColorLabel::Green => "Green",
        ColorLabel::Blue => "Blue",
        ColorLabel::Pink => "Pink",
        ColorLabel::Purple => "Purple",
    })
}

fn rating_from_text(s: &str) -> Option<u8> {
    // -1 means "rejected" in Lightroom; Silberkorn has no rejection.
    s.trim().parse::<f32>().ok().map(|r| r.clamp(0.0, 5.0).round() as u8)
}

/// Parses XMP data. Properties may be attributes of `rdf:Description` or elements.
pub fn parse(text: &str) -> Result<XmpMeta> {
    let doc = roxmltree::Document::parse(text).context("Kein gültiges XML")?;
    let mut meta = XmpMeta::default();
    for desc in doc.descendants().filter(|n| n.has_tag_name((NS_RDF, "Description"))) {
        for a in desc.attributes() {
            match (a.namespace(), a.name()) {
                (Some(NS_XMP), "Rating") => meta.rating = rating_from_text(a.value()).or(meta.rating),
                (Some(NS_XMP), "Label") => meta.label = label_from_text(a.value()).or(meta.label),
                _ => {}
            }
        }
        for child in desc.children().filter(|n| n.is_element()) {
            let ns = child.tag_name().namespace();
            match (ns, child.tag_name().name()) {
                (Some(NS_XMP), "Rating") => meta.rating = rating_from_text(child.text().unwrap_or("")).or(meta.rating),
                (Some(NS_XMP), "Label") => meta.label = label_from_text(child.text().unwrap_or("")).or(meta.label),
                (Some(NS_DC), "subject") => {
                    let words: Vec<String> = child
                        .descendants()
                        .filter(|n| n.has_tag_name((NS_RDF, "li")))
                        .filter_map(|n| n.text())
                        .map(|t| t.trim().to_string())
                        .filter(|t| !t.is_empty())
                        .collect();
                    meta.keywords = Some(words);
                }
                _ => {}
            }
        }
    }
    Ok(meta)
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// `rdf:Description` with the given values (namespaces declared on it).
fn description(rating: u8, label: ColorLabel, keywords: &[String], indent: &str) -> String {
    let mut s = format!("{indent}<rdf:Description rdf:about=\"\" xmlns:xmp=\"{NS_XMP}\" xmlns:dc=\"{NS_DC}\">\n");
    s += &format!("{indent} <xmp:Rating>{rating}</xmp:Rating>\n");
    if let Some(l) = label_to_text(label) {
        s += &format!("{indent} <xmp:Label>{l}</xmp:Label>\n");
    }
    if !keywords.is_empty() {
        s += &format!("{indent} <dc:subject>\n{indent}  <rdf:Bag>\n");
        for k in keywords {
            s += &format!("{indent}   <rdf:li>{}</rdf:li>\n", escape(k));
        }
        s += &format!("{indent}  </rdf:Bag>\n{indent} </dc:subject>\n");
    }
    s += &format!("{indent}</rdf:Description>\n");
    s
}

/// XMP text with these values: an existing document is updated (other content kept), else a
/// new one is created.
pub fn updated(existing: Option<&str>, rating: u8, label: ColorLabel, keywords: &[String]) -> Result<String> {
    let new_doc = || {
        format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Silberkorn\">\n <rdf:RDF xmlns:rdf=\"{NS_RDF}\">\n{} </rdf:RDF>\n</x:xmpmeta>\n",
            description(rating, label, keywords, "  ")
        )
    };
    let Some(text) = existing.filter(|t| !t.trim().is_empty()) else { return Ok(new_doc()) };
    let doc = roxmltree::Document::parse(text).context("Vorhandene XMP-Datei ist kein gültiges XML")?;
    let Some(rdf) = doc.descendants().find(|n| n.has_tag_name((NS_RDF, "RDF"))) else { return Ok(new_doc()) };
    // Byte ranges to remove: our properties wherever they are.
    let mut cuts: Vec<std::ops::Range<usize>> = Vec::new();
    let ours_attr = |a: &roxmltree::Attribute| a.namespace() == Some(NS_XMP) && matches!(a.name(), "Rating" | "Label");
    let ours_elem = |n: &roxmltree::Node| {
        let t = n.tag_name();
        matches!((t.namespace(), t.name()), (Some(NS_XMP), "Rating" | "Label") | (Some(NS_DC), "subject"))
    };
    // From the start of the line (indentation and line break before it) to the end of the node.
    let line_start = |start: usize| {
        let s = text[..start].trim_end_matches([' ', '\t']).len();
        if text[..s].ends_with('\n') { s - 1 } else { s }
    };
    for desc in rdf.descendants().filter(|n| n.has_tag_name((NS_RDF, "Description"))) {
        let only_ours = desc.attributes().all(|a| ours_attr(&a) || (a.namespace() == Some(NS_RDF) && a.name() == "about"))
            && desc.children().all(|c| if c.is_element() { ours_elem(&c) } else { c.text().is_none_or(|t| t.trim().is_empty()) });
        if only_ours {
            // A description of ours (or one left empty): away with it.
            let r = desc.range();
            cuts.push(line_start(r.start)..r.end);
            continue;
        }
        for a in desc.attributes().filter(|a| ours_attr(a)) {
            // With the whitespace before it.
            let r = a.range();
            let start = text[..r.start].trim_end().len();
            cuts.push(start..r.end);
        }
        for child in desc.children().filter(|n| n.is_element() && ours_elem(n)) {
            let r = child.range();
            cuts.push(line_start(r.start)..r.end);
        }
    }
    // A new description goes before the closing tag of rdf:RDF.
    let rdf_end = rdf.range().end;
    let close = text[..rdf_end].rfind("</").context("rdf:RDF ohne schließendes Tag")?;
    let insert_at = text[..close].trim_end_matches([' ', '\t']).len();
    let indent = &text[insert_at..close];
    let prefix = rdf.tag_name().namespace().and_then(|ns| rdf.lookup_prefix(ns)).unwrap_or("rdf");
    let mut addition = description(rating, label, keywords, &format!("{indent} "));
    if prefix != "rdf" {
        addition = addition.replace("rdf:", &format!("{prefix}:"));
    }
    cuts.sort_by_key(|r| r.start);
    let mut out = String::with_capacity(text.len() + addition.len());
    let mut pos = 0;
    for c in cuts {
        if c.start < pos {
            continue;
        }
        if pos <= insert_at && insert_at < c.start {
            out += &text[pos..insert_at];
            out += &addition;
            pos = insert_at;
        }
        out += &text[pos..c.start];
        pos = c.end;
    }
    if pos <= insert_at {
        out += &text[pos..insert_at];
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out += &addition;
        pos = insert_at;
    }
    out += &text[pos..];
    // Must still be valid.
    parse(&out).context("XMP-Aktualisierung fehlgeschlagen")?;
    Ok(out)
}

/// Writes rating, label and keywords into the image's XMP file (the existing one, else
/// `<name>.xmp`). Returns the file's new modification time.
pub fn write(image: &Path, rating: u8, label: ColorLabel, keywords: &[String]) -> Result<Option<i64>> {
    let path = existing(image).unwrap_or_else(|| candidates(image)[0].clone());
    let old = std::fs::read_to_string(&path).ok();
    let text = updated(old.as_deref(), rating, label, keywords)?;
    if old.as_deref() != Some(text.as_str()) {
        crate::safe_file::write(&path, &text)?;
    }
    Ok(mtime(image))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIGHTROOM: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
   xmp:Rating="4"
   xmp:Label="Rot"
   crs:Exposure2012="+0.50">
   <dc:subject>
    <rdf:Bag>
     <rdf:li>Urlaub</rdf:li>
     <rdf:li>Berge &amp; Seen</rdf:li>
    </rdf:Bag>
   </dc:subject>
   <dc:creator><rdf:Seq><rdf:li>Martin</rdf:li></rdf:Seq></dc:creator>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#;

    #[test]
    fn parses_attributes_and_elements() {
        let m = parse(LIGHTROOM).unwrap();
        assert_eq!(m.rating, Some(4));
        assert_eq!(m.label, Some(ColorLabel::Red));
        assert_eq!(m.keywords, Some(vec!["Urlaub".to_string(), "Berge & Seen".to_string()]));
        let elements = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
            <rdf:Description xmlns:xmp="http://ns.adobe.com/xap/1.0/"><xmp:Rating>-1</xmp:Rating><xmp:Label>Blue</xmp:Label></rdf:Description>
            </rdf:RDF></x:xmpmeta>"#;
        let m = parse(elements).unwrap();
        assert_eq!((m.rating, m.label, m.keywords), (Some(0), Some(ColorLabel::Blue), None));
    }

    #[test]
    fn update_keeps_foreign_content() {
        let out = updated(Some(LIGHTROOM), 2, ColorLabel::Green, &["Neu".into(), "a<b".into()]).unwrap();
        let m = parse(&out).unwrap();
        assert_eq!(m.rating, Some(2));
        assert_eq!(m.label, Some(ColorLabel::Green));
        assert_eq!(m.keywords, Some(vec!["Neu".to_string(), "a<b".to_string()]));
        assert!(out.contains("crs:Exposure2012=\"+0.50\""), "{out}");
        assert!(out.contains("<rdf:li>Martin</rdf:li>"));
        assert!(!out.contains("Urlaub") && !out.contains("Rot"));
        // Updating again doesn't pile up descriptions.
        let again = updated(Some(&out), 2, ColorLabel::Green, &["Neu".into(), "a<b".into()]).unwrap();
        assert_eq!(again.matches("<xmp:Rating>").count(), 1);
        assert_eq!(again, out);
        // A label can be removed again.
        let none = updated(Some(&out), 0, ColorLabel::None, &[]).unwrap();
        assert_eq!(parse(&none).unwrap(), XmpMeta { rating: Some(0), label: None, keywords: None });
    }

    #[test]
    fn new_document_and_other_prefix() {
        let out = updated(None, 5, ColorLabel::Purple, &["x".into()]).unwrap();
        assert_eq!(parse(&out).unwrap(), XmpMeta { rating: Some(5), label: Some(ColorLabel::Purple), keywords: Some(vec!["x".into()]) });
        let odd = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><R:RDF xmlns:R="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><R:Description R:about=""/></R:RDF></x:xmpmeta>"#;
        let out = updated(Some(odd), 3, ColorLabel::None, &[]).unwrap();
        assert_eq!(parse(&out).unwrap().rating, Some(3));
    }

    #[test]
    fn file_names() {
        let [a, b] = files_for(Path::new("/f/IMG_1.ARW#2"));
        assert_eq!(a, Path::new("/f/IMG_1.xmp"));
        assert_eq!(b, Path::new("/f/IMG_1.ARW.xmp"));
    }
}
