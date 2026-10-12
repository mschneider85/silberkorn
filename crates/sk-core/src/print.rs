//! Print sizes: how an image sits on a paper format and the resolution it is printed at.

use serde::{Deserialize, Serialize};

/// A paper format in millimeters (in any orientation; it is turned to match the image).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrintFormat {
    pub w_mm: f32,
    pub h_mm: f32,
}

impl PrintFormat {
    pub const fn new(w_mm: f32, h_mm: f32) -> Self {
        Self { w_mm, h_mm }
    }

    /// "10 × 15 cm" or "A4", for display.
    pub fn label(self) -> String {
        if let Some((name, _)) = PRESETS.iter().find(|(_, f)| *f == self) {
            return name.to_string();
        }
        let cm = |v: f32| {
            let c = v / 10.0;
            if (c - c.round()).abs() < 0.05 { format!("{c:.0}") } else { format!("{c:.1}").replace('.', ",") }
        };
        format!("{} × {} cm", cm(self.w_mm.min(self.h_mm)), cm(self.w_mm.max(self.h_mm)))
    }
}

impl Default for PrintFormat {
    fn default() -> Self {
        PRESETS[1].1
    }
}

/// The usual photo and paper formats.
pub const PRESETS: [(&str, PrintFormat); 11] = [
    ("9 × 13 cm", PrintFormat::new(89.0, 127.0)),
    ("10 × 15 cm", PrintFormat::new(102.0, 152.0)),
    ("13 × 18 cm", PrintFormat::new(127.0, 178.0)),
    ("15 × 20 cm", PrintFormat::new(152.0, 203.0)),
    ("20 × 30 cm", PrintFormat::new(203.0, 305.0)),
    ("A4", PrintFormat::new(210.0, 297.0)),
    ("A3", PrintFormat::new(297.0, 420.0)),
    ("A3+", PrintFormat::new(329.0, 483.0)),
    ("A2", PrintFormat::new(420.0, 594.0)),
    ("30 × 45 cm", PrintFormat::new(300.0, 450.0)),
    ("40 × 60 cm", PrintFormat::new(400.0, 600.0)),
];

/// How the image is put on the paper.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Fit {
    /// Whole image, white border where the aspect ratios differ.
    Contain,
    /// Paper filled (borderless lab print), the image is cut where the aspect ratios differ.
    #[default]
    Fill,
}

impl Fit {
    pub fn label(self) -> &'static str {
        match self {
            Fit::Contain => "Einpassen",
            Fit::Fill => "Füllen",
        }
    }
}

/// Print settings (kept between sessions).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PrintSettings {
    pub format: PrintFormat,
    /// Stored under a new name: the old `fit` held the earlier default (`Contain`), not a choice.
    #[serde(rename = "fit_mode")]
    pub fit: Fit,
    /// Physical screen pixels per millimeter, per monitor (see `monitor_key` in the app).
    pub calibration: Vec<(String, f32)>,
}

/// Resolutions below these are worth a warning (pixels per inch on paper).
pub const PPI_LOW: f32 = 200.0;
pub const PPI_POOR: f32 = 150.0;

/// An image of `px_w` × `px_h` pixels on `format`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// The paper, turned like the image (mm).
    pub paper: [f32; 2],
    /// The printed image (mm); larger than the paper when filling.
    pub image: [f32; 2],
    /// Pixels per inch on paper.
    pub ppi: f32,
    /// Share of the image area that is cut off (filling) or of the paper left blank (containing).
    pub off: f32,
}

pub fn layout(px_w: f32, px_h: f32, format: PrintFormat, fit: Fit) -> Layout {
    let (short, long) = (format.w_mm.min(format.h_mm), format.w_mm.max(format.h_mm));
    let paper = if px_w >= px_h { [long, short] } else { [short, long] };
    let (px_w, px_h) = (px_w.max(1.0), px_h.max(1.0));
    let (sx, sy) = (paper[0] / px_w, paper[1] / px_h);
    // Millimeters per pixel.
    let mm = match fit {
        Fit::Contain => sx.min(sy),
        Fit::Fill => sx.max(sy),
    };
    let image = [px_w * mm, px_h * mm];
    let (pa, ia) = (paper[0] * paper[1], image[0] * image[1]);
    let off = match fit {
        Fit::Contain => 1.0 - ia / pa,
        Fit::Fill => 1.0 - pa / ia,
    };
    Layout { paper, image, ppi: 25.4 / mm, off: off.max(0.0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_to_two_fits_ten_by_fifteen_exactly() {
        let l = layout(6000.0, 4000.0, PrintFormat::new(100.0, 150.0), Fit::Contain);
        assert_eq!(l.paper, [150.0, 100.0]);
        assert!((l.image[0] - 150.0).abs() < 1e-3 && (l.image[1] - 100.0).abs() < 1e-3);
        assert!((l.ppi - 1016.0).abs() < 0.5, "{}", l.ppi);
        assert!(l.off < 1e-4);
    }

    #[test]
    fn other_aspect_leaves_border_or_cuts() {
        // 3:2 portrait on 13 × 18 (7:5).
        let f = PrintFormat::new(130.0, 180.0);
        let c = layout(2000.0, 3000.0, f, Fit::Contain);
        assert_eq!(c.paper, [130.0, 180.0]);
        assert!((c.image[1] - 180.0).abs() < 1e-3 && (c.image[0] - 120.0).abs() < 1e-3);
        assert!((c.off - (1.0 - 120.0 / 130.0)).abs() < 1e-4);
        let fill = layout(2000.0, 3000.0, f, Fit::Fill);
        assert!((fill.image[0] - 130.0).abs() < 1e-3 && fill.image[1] > 180.0);
        assert!(fill.ppi < c.ppi);
        assert!((fill.off - (1.0 - 180.0 / 195.0)).abs() < 1e-4, "{}", fill.off);
    }

    #[test]
    fn fill_is_the_default_also_for_old_settings() {
        assert_eq!(PrintSettings::default().fit, Fit::Fill);
        let old: PrintSettings = serde_json::from_str(r#"{"fit":"Contain"}"#).unwrap();
        assert_eq!(old.fit, Fit::Fill);
        let s = PrintSettings { fit: Fit::Contain, ..Default::default() };
        let back: PrintSettings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.fit, Fit::Contain);
    }

    #[test]
    fn labels() {
        assert_eq!(PrintFormat::new(210.0, 297.0).label(), "A4");
        assert_eq!(PrintFormat::new(240.0, 180.0).label(), "18 × 24 cm");
        assert_eq!(PrintFormat::new(95.0, 130.0).label(), "9,5 × 13 cm");
    }
}
