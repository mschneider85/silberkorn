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
        // Also a printer's sheet, measured in its own pixels (210.01 × 297.02).
        let near = |f: &PrintFormat| (f.w_mm.min(f.h_mm) - self.w_mm.min(self.h_mm)).abs() < 1.0 && (f.w_mm.max(f.h_mm) - self.w_mm.max(self.h_mm)).abs() < 1.0;
        if let Some((name, _)) = PRESETS.iter().find(|(_, f)| near(f)) {
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

/// The text under the image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Caption {
    #[default]
    Off,
    FileName,
    Capture,
    Both,
}

impl Caption {
    pub const ALL: [Caption; 4] = [Caption::Off, Caption::FileName, Caption::Capture, Caption::Both];

    pub fn label(self) -> &'static str {
        match self {
            Caption::Off => "Keine",
            Caption::FileName => "Dateiname",
            Caption::Capture => "Aufnahmedaten",
            Caption::Both => "Dateiname und Daten",
        }
    }

    /// The caption of an image, or `None` (off, or nothing to write).
    pub fn text(self, file: &std::path::Path, info: &crate::raw::CaptureInfo) -> Option<String> {
        let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let camera = if info.model.is_empty() {
            None
        } else if info.model.to_lowercase().starts_with(&info.make.to_lowercase()) || info.make.is_empty() {
            Some(info.model.clone())
        } else {
            Some(format!("{} {}", info.make, info.model))
        };
        // EXIF writes "2024:05:17 14:03:22".
        let date = info.date.as_ref().map(|d| match d.as_bytes() {
            [_, _, _, _, b':', _, _, b':', ..] => format!("{}-{}-{}", &d[..4], &d[5..7], &d[8..]),
            _ => d.clone(),
        });
        let capture: Vec<String> = [camera, info.focal_length.clone(), info.aperture.clone(), info.exposure_time.clone(), info.iso.map(|i| format!("ISO {i}")), date]
            .into_iter()
            .flatten()
            .filter(|s| !s.trim().is_empty())
            .collect();
        let capture = capture.join("  ·  ");
        let text = match self {
            Caption::Off => String::new(),
            Caption::FileName => name,
            Caption::Capture => capture,
            Caption::Both if capture.is_empty() => name,
            Caption::Both => format!("{name}  ·  {capture}"),
        };
        (!text.trim().is_empty()).then_some(text)
    }
}

/// How strongly the print is sharpened, for the paper it is printed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PrintSharpening {
    Off,
    Low,
    #[default]
    Standard,
    High,
}

impl PrintSharpening {
    pub const ALL: [PrintSharpening; 4] = [PrintSharpening::Off, PrintSharpening::Low, PrintSharpening::Standard, PrintSharpening::High];

    pub fn label(self) -> &'static str {
        match self {
            PrintSharpening::Off => "Aus",
            PrintSharpening::Low => "Wenig",
            PrintSharpening::Standard => "Normal",
            PrintSharpening::High => "Stark",
        }
    }

    /// Unsharp mask amount and radius (mm on paper): matte paper soaks up ink and softens more.
    pub fn strength(self, paper: Paper) -> Option<(f32, f32)> {
        let amount = match self {
            PrintSharpening::Off => return None,
            PrintSharpening::Low => 0.5,
            PrintSharpening::Standard => 0.9,
            PrintSharpening::High => 1.4,
        };
        Some(match paper {
            Paper::Glossy => (amount, 0.085),
            Paper::Matte => (amount * 1.25, 0.12),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Paper {
    #[default]
    Glossy,
    Matte,
}

impl Paper {
    pub fn label(self) -> &'static str {
        match self {
            Paper::Glossy => "Glänzend",
            Paper::Matte => "Matt",
        }
    }
}

/// Print settings (kept between sessions).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrintSettings {
    pub format: PrintFormat,
    /// Stored under a new name: the old `fit` held the earlier default (`Contain`), not a choice.
    #[serde(rename = "fit_mode")]
    pub fit: Fit,
    /// Physical screen pixels per millimeter, per monitor (see `monitor_key` in the app).
    pub calibration: Vec<(String, f32)>,
    /// White border around the image on every side (mm).
    pub margin_mm: f32,
    /// A thin black line around the image (mm wide; 0: none).
    pub border_mm: f32,
    /// Lines at the image's corners, outside it, to cut along.
    pub crop_marks: bool,
    pub caption: Caption,
    /// Print adjustment (-50..50): prints often look darker and flatter than the screen.
    pub brightness: f32,
    pub contrast: f32,
    pub sharpening: PrintSharpening,
    pub paper: Paper,
    /// The printer's ICC profile (printer and paper): Silberkorn converts the colors and the
    /// driver's color management is switched off. `None`: the printer gets sRGB and manages.
    pub profile: Option<std::path::PathBuf>,
    pub intent: crate::softproof::RenderIntent,
    /// The paper upright (`Some(true)`) or across; `None`: turned like the image. Chosen per
    /// page and not kept: each print starts turned like its images.
    #[serde(skip)]
    pub portrait: Option<bool>,
    /// The image's size on the paper (turned like the image), e.g. 10 × 15 on A4, in the middle;
    /// `None`: as large as the paper and its margin allow.
    pub image_size: Option<PrintFormat>,
    /// The printer by name (Windows); `None`: the system's default printer.
    pub printer: Option<String>,
}

impl Default for PrintSettings {
    fn default() -> Self {
        Self {
            format: PrintFormat::default(),
            fit: Fit::default(),
            calibration: Vec::new(),
            margin_mm: 0.0,
            border_mm: 0.0,
            crop_marks: false,
            caption: Caption::Off,
            brightness: 0.0,
            contrast: 0.0,
            sharpening: PrintSharpening::Standard,
            paper: Paper::Glossy,
            profile: None,
            intent: crate::softproof::RenderIntent::Perceptual,
            portrait: None,
            image_size: None,
            printer: None,
        }
    }
}

/// Height of the caption's letters and its distance from the image (mm).
pub const CAPTION_MM: f32 = 3.0;
pub const CAPTION_GAP_MM: f32 = 2.5;
/// Crop marks: distance from the image's corner, length and line width (mm).
pub const MARK_GAP_MM: f32 = 2.0;
pub const MARK_LEN_MM: f32 = 6.0;
/// 0.1 mm (0.28 pt): about the usual 0.25 pt of print shops and layout programs.
pub const MARK_LINE_MM: f32 = 0.1;

/// Resolutions below these are worth a warning (pixels per inch on paper).
pub const PPI_LOW: f32 = 200.0;
pub const PPI_POOR: f32 = 150.0;

/// An image of `px_w` × `px_h` pixels on a page. Everything is centered on the paper.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// The paper, turned like the image (mm).
    pub paper: [f32; 2],
    /// The printed image (mm); larger than the paper when filling.
    pub image: [f32; 2],
    /// The part of the image that is printed (mm): the image when containing, the room inside
    /// the margins when filling.
    pub window: [f32; 2],
    /// What the print is cut to (mm), where the crop marks go: the fixed image size (with white
    /// strips where the image is contained), else the printed image.
    pub cut: [f32; 2],
    /// Pixels per inch on paper.
    pub ppi: f32,
    /// Share of the image area that is cut off (filling) or of the paper left blank (containing).
    pub off: f32,
}

/// The layout on the settings' paper format, turned like the image.
pub fn layout(px_w: f32, px_h: f32, s: &PrintSettings) -> Layout {
    let (short, long) = (s.format.w_mm.min(s.format.h_mm), s.format.w_mm.max(s.format.h_mm));
    let portrait = s.portrait.unwrap_or(px_h > px_w);
    let paper = if portrait { [short, long] } else { [long, short] };
    layout_on(px_w, px_h, paper, s)
}

/// The layout on `paper` as it is (the printer's sheet, in its orientation).
pub fn layout_on(px_w: f32, px_h: f32, paper: [f32; 2], s: &PrintSettings) -> Layout {
    let (px_w, px_h) = (px_w.max(1.0), px_h.max(1.0));
    let margin = if s.margin_mm.is_finite() { s.margin_mm.clamp(0.0, 100.0) } else { 0.0 };
    // The caption below, and the same room above: the image stays in the middle.
    let caption = if s.caption == Caption::Off { 0.0 } else { 2.0 * (CAPTION_GAP_MM + CAPTION_MM) };
    let inner = match s.image_size {
        // A fixed size (at most the paper): the margin is what is left around it.
        Some(f) => {
            let (short, long) = (f.w_mm.min(f.h_mm).max(1.0), f.w_mm.max(f.h_mm).max(1.0));
            let size = if px_h > px_w { [short, long] } else { [long, short] };
            [size[0].min(paper[0]), size[1].min(paper[1] - caption).max(1.0)]
        }
        None => [(paper[0] - 2.0 * margin).max(1.0), (paper[1] - 2.0 * margin - caption).max(1.0)],
    };
    let (sx, sy) = (inner[0] / px_w, inner[1] / px_h);
    // Millimeters per pixel.
    let mm = match s.fit {
        Fit::Contain => sx.min(sy),
        Fit::Fill => sx.max(sy),
    };
    let image = [px_w * mm, px_h * mm];
    let window = match s.fit {
        Fit::Contain => image,
        Fit::Fill => inner,
    };
    let (pa, ia, wa) = (paper[0] * paper[1], image[0] * image[1], window[0] * window[1]);
    let off = match s.fit {
        Fit::Contain => 1.0 - ia / pa,
        Fit::Fill => 1.0 - wa / ia,
    };
    let cut = if s.image_size.is_some() { inner } else { window };
    Layout { paper, image, window, cut, ppi: 25.4 / mm, off: off.max(0.0) }
}

impl Layout {
    /// The printed part of the image on the paper (mm from the paper's top left): x, y, w, h.
    pub fn window_rect(&self) -> [f32; 4] {
        [(self.paper[0] - self.window[0]) / 2.0, (self.paper[1] - self.window[1]) / 2.0, self.window[0], self.window[1]]
    }

    /// The printed part as a share of the image: left, top, width, height (0..1).
    pub fn window_in_image(&self) -> [f32; 4] {
        let (w, h) = ((self.window[0] / self.image[0]).min(1.0), (self.window[1] / self.image[1]).min(1.0));
        [(1.0 - w) / 2.0, (1.0 - h) / 2.0, w, h]
    }

    /// What the print is cut to on the paper (mm from the paper's top left): x, y, w, h.
    pub fn cut_rect(&self) -> [f32; 4] {
        [(self.paper[0] - self.cut[0]) / 2.0, (self.paper[1] - self.cut[1]) / 2.0, self.cut[0], self.cut[1]]
    }

    /// White strips (containing) or what is cut off the image (filling), per side (mm):
    /// across and up-down.
    pub fn trim(&self) -> [f32; 2] {
        [(self.cut[0] - self.image[0]).abs() / 2.0, (self.cut[1] - self.image[1]).abs() / 2.0]
    }

    /// Where the caption's letters sit (mm from the paper's top): its top, below the cut.
    pub fn caption_top(&self) -> f32 {
        let [_, y, _, h] = self.cut_rect();
        y + h + CAPTION_GAP_MM
    }

    /// The crop marks as lines (mm on the paper): ((x0, y0), (x1, y1)), around the cut (and the
    /// border, where it lies on the cut).
    pub fn crop_marks(&self, border_mm: f32) -> Vec<([f32; 2], [f32; 2])> {
        let [x, y, w, h] = self.cut_rect();
        let b = if self.cut == self.window { border_mm.max(0.0) } else { 0.0 };
        let (gap, len) = (MARK_GAP_MM + b, MARK_LEN_MM);
        let mut lines = Vec::new();
        for (cx, sx) in [(x, -1.0), (x + w, 1.0)] {
            for (cy, sy) in [(y, -1.0), (y + h, 1.0)] {
                // Along the horizontal edge, outside to the side; along the vertical one, above or below.
                lines.push(([cx + sx * gap, cy], [cx + sx * (gap + len), cy]));
                lines.push(([cx, cy + sy * gap], [cx, cy + sy * (gap + len)]));
            }
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_to_two_fits_ten_by_fifteen_exactly() {
        let l = layout(6000.0, 4000.0, &PrintSettings { format: PrintFormat::new(100.0, 150.0), fit: Fit::Contain, ..Default::default() });
        assert_eq!(l.paper, [150.0, 100.0]);
        assert!((l.image[0] - 150.0).abs() < 1e-3 && (l.image[1] - 100.0).abs() < 1e-3);
        assert!((l.ppi - 1016.0).abs() < 0.5, "{}", l.ppi);
        assert!(l.off < 1e-4);
    }

    #[test]
    fn other_aspect_leaves_border_or_cuts() {
        // 3:2 portrait on 13 × 18 (7:5).
        let f = PrintFormat::new(130.0, 180.0);
        let c = layout(2000.0, 3000.0, &PrintSettings { format: f, fit: Fit::Contain, ..Default::default() });
        assert_eq!(c.paper, [130.0, 180.0]);
        assert!((c.image[1] - 180.0).abs() < 1e-3 && (c.image[0] - 120.0).abs() < 1e-3);
        assert!((c.off - (1.0 - 120.0 / 130.0)).abs() < 1e-4);
        let fill = layout(2000.0, 3000.0, &PrintSettings { format: f, fit: Fit::Fill, ..Default::default() });
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
    fn margins_and_caption_leave_room_and_keep_the_image_centered() {
        let s = PrintSettings { format: PrintFormat::new(210.0, 297.0), fit: Fit::Contain, margin_mm: 15.0, caption: Caption::FileName, ..Default::default() };
        let l = layout(6000.0, 4000.0, &s);
        assert_eq!(l.paper, [297.0, 210.0]);
        let room = 210.0 - 30.0 - 2.0 * (CAPTION_GAP_MM + CAPTION_MM);
        assert!((l.image[1] - room).abs() < 1e-3, "{:?}", l.image);
        assert_eq!(l.window, l.image);
        let [x, y, w, h] = l.window_rect();
        assert!((x + w / 2.0 - 148.5).abs() < 1e-3 && (y + h / 2.0 - 105.0).abs() < 1e-3);
        assert!(l.caption_top() + CAPTION_MM < 210.0 - 15.0 + 1e-3);
        // Filling: the window is the room inside the margins, the image larger.
        let f = layout(6000.0, 4000.0, &PrintSettings { fit: Fit::Fill, caption: Caption::Off, ..s });
        assert!((f.window[0] - 267.0).abs() < 1e-3 && (f.window[1] - 180.0).abs() < 1e-3);
        assert!(f.image[0] >= f.window[0] - 1e-3 && f.image[1] >= f.window[1] - 1e-3);
        let [_, _, ww, wh] = f.window_in_image();
        assert!(ww < 1.0 && wh <= 1.0);
        // Eight lines, all outside the image.
        let marks = f.crop_marks(0.0);
        assert_eq!(marks.len(), 8);
        let [x, y, w, h] = f.window_rect();
        for (a, b) in marks {
            for p in [a, b] {
                assert!(p[0] <= x + 1e-3 || p[0] >= x + w - 1e-3 || p[1] <= y + 1e-3 || p[1] >= y + h - 1e-3);
            }
        }
    }

    #[test]
    fn the_paper_turns_like_the_image_unless_set() {
        let s = PrintSettings { format: PrintFormat::new(100.0, 150.0), ..Default::default() };
        assert_eq!(layout(600.0, 400.0, &s).paper, [150.0, 100.0]);
        assert_eq!(layout(400.0, 600.0, &s).paper, [100.0, 150.0]);
        let upright = PrintSettings { portrait: Some(true), ..s };
        assert_eq!(layout(600.0, 400.0, &upright).paper, [100.0, 150.0]);
    }

    #[test]
    fn a_photo_size_on_larger_paper() {
        // 10 × 15 on A4, cut to size like a lab print, in the middle of the sheet.
        let ten = PrintFormat::new(100.0, 150.0);
        let s = PrintSettings { format: PRESETS[5].1, fit: Fit::Fill, image_size: Some(ten), margin_mm: 40.0, ..Default::default() };
        let l = layout(6000.0, 4000.0, &s);
        assert_eq!(l.paper, [297.0, 210.0]);
        assert!((l.window[0] - 150.0).abs() < 1e-3 && (l.window[1] - 100.0).abs() < 1e-3, "{:?}", l.window);
        let [x, y, ..] = l.window_rect();
        assert!((x - 73.5).abs() < 1e-3 && (y - 55.0).abs() < 1e-3);
        // A portrait image gets a portrait 10 × 15.
        let p = layout(4000.0, 6000.0, &s);
        assert!((p.window[0] - 100.0).abs() < 1e-3 && (p.window[1] - 150.0).abs() < 1e-3);
        // Contained in 9 × 13: the whole 3:2 image, white strips above and below, cut to 9 × 13.
        let nine = layout(6000.0, 4000.0, &PrintSettings { image_size: Some(PrintFormat::new(89.0, 127.0)), fit: Fit::Contain, ..s.clone() });
        assert_eq!(nine.cut, [127.0, 89.0]);
        assert!((nine.window[1] - 127.0 / 1.5).abs() < 1e-3);
        let trim = nine.trim();
        assert!(trim[0] < 1e-3 && (trim[1] - (89.0 - 127.0 / 1.5) / 2.0).abs() < 1e-3, "{trim:?}");
        let [x, ..] = nine.cut_rect();
        assert!(nine.crop_marks(0.0).iter().any(|(a, _)| (a[0] - x).abs() < 1e-3));
        // Never larger than the paper.
        let big = layout(6000.0, 4000.0, &PrintSettings { image_size: Some(PrintFormat::new(400.0, 600.0)), ..s });
        assert!(big.window[0] <= 297.0 + 1e-3 && big.window[1] <= 210.0 + 1e-3);
    }

    #[test]
    fn captions() {
        let info = crate::raw::CaptureInfo {
            make: "Sony".into(),
            model: "ILCE-7M3".into(),
            iso: Some(100),
            aperture: Some("f/5.6".into()),
            exposure_time: Some("1/800 s".into()),
            focal_length: Some("85 mm".into()),
            date: Some("2025:09:11 13:30:02".into()),
            ..Default::default()
        };
        let f = std::path::Path::new("/x/DSC09197.ARW");
        assert_eq!(Caption::Off.text(f, &info), None);
        assert_eq!(Caption::FileName.text(f, &info).unwrap(), "DSC09197.ARW");
        assert_eq!(Caption::Capture.text(f, &info).unwrap(), "Sony ILCE-7M3  ·  85 mm  ·  f/5.6  ·  1/800 s  ·  ISO 100  ·  2025-09-11 13:30:02");
        assert!(Caption::Both.text(f, &info).unwrap().starts_with("DSC09197.ARW  ·  Sony"));
        assert_eq!(Caption::Capture.text(f, &Default::default()), None);
    }

    #[test]
    fn labels() {
        assert_eq!(PrintFormat::new(210.0, 297.0).label(), "A4");
        assert_eq!(PrintFormat::new(297.02, 210.01).label(), "A4");
        assert_eq!(PrintFormat::new(240.0, 180.0).label(), "18 × 24 cm");
        assert_eq!(PrintFormat::new(95.0, 130.0).label(), "9,5 × 13 cm");
    }
}
