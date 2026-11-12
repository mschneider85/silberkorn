//! Print preview in the viewer (see `sk_core::print`): the paper format around the image, the
//! resolution on paper and, on a calibrated screen, the image at its printed size.

use egui::{Color32, RichText, Ui, Vec2, vec2};
use sk_core::print::{Caption, Fit, Layout, PPI_LOW, PPI_POOR, Paper, PrintFormat, PrintSettings, PrintSharpening};
use sk_core::softproof::RenderIntent;

use crate::app::App;
use crate::theme;
use crate::widgets;

/// Print preview state of the app.
#[derive(Default)]
pub struct PrintPreviewState {
    pub on: bool,
    pub settings: PrintSettings,
    /// The viewer shows the image at its printed size (until zoomed otherwise).
    pub zoom: bool,
    /// The calibration dialog is open, with the physical pixels per millimeter being tried.
    pub calibrating: Option<f32>,
    /// The running print job.
    pub job: Option<u64>,
    /// Strg+P: the print bar in the image, before the system's dialog.
    pub bar: bool,
    /// The print preview was on before the print bar turned it on.
    pub was_on: bool,
    /// What the job wants said when it is done (the printer took other paper).
    pub note: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    /// The installed printers and their papers, once asked for (in the background).
    pub printers: std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<crate::printer::Printers>>>>,
    pub printers_asked: bool,
    /// Pages turned by hand (upright or not) against their image, until the print bar opens again.
    pub turned: std::collections::HashMap<std::path::PathBuf, bool>,
    /// The driver's settings from "Einstellungen …" (paper type, quality), for this session.
    pub driver: Option<crate::printer::Choice>,
}

/// Photo sizes for the image on the paper (as in a lab).
pub const PHOTO_SIZES: [(&str, PrintFormat); 6] = [
    ("9 × 13 cm", PrintFormat::new(89.0, 127.0)),
    ("10 × 15 cm", PrintFormat::new(102.0, 152.0)),
    ("13 × 18 cm", PrintFormat::new(127.0, 178.0)),
    ("15 × 20 cm", PrintFormat::new(152.0, 203.0)),
    ("20 × 30 cm", PrintFormat::new(203.0, 305.0)),
    ("A5", PrintFormat::new(148.0, 210.0)),
];

/// What fitting does to an image of `size` (pixels) with these settings: "ganzes Bild, oben und
/// unten je 2 mm weiß", "links und rechts je 3 mm abgeschnitten".
pub fn fit_effect(s: &PrintSettings, fit: Fit, size: Option<(f32, f32)>) -> String {
    let Some((w, h)) = size else {
        return match fit {
            Fit::Contain => "ganzes Bild".into(),
            Fit::Fill => "füllt das Format, das Bild wird beschnitten".into(),
        };
    };
    let l = sk_core::print::layout(w, h, &PrintSettings { fit, ..s.clone() });
    let [across, updown] = l.trim();
    let (sides, mm) = if across >= updown { ("links und rechts", across) } else { ("oben und unten", updown) };
    match fit {
        Fit::Contain if mm < 0.5 => "ganzes Bild, füllt das Format".into(),
        Fit::Contain => format!("ganzes Bild, {sides} je {mm:.0} mm weiß"),
        Fit::Fill if mm < 0.5 => "füllt das Format, nichts abgeschnitten".into(),
        Fit::Fill => format!("füllt das Format, {sides} je {mm:.0} mm abgeschnitten"),
    }
}

/// The two ways of fitting, each with what it does to the image shown.
pub fn fit_entries(ui: &mut Ui, s: &mut PrintSettings, size: Option<(f32, f32)>) {
    for fit in [Fit::Contain, Fit::Fill] {
        let text = format!("{} – {}", fit.label(), fit_effect(s, fit, size));
        ui.selectable_value(&mut s.fit, fit, text);
    }
}

/// "Ganzes Blatt" or the image size ("10 × 15 cm").
pub fn image_size_label(f: Option<PrintFormat>) -> String {
    match f {
        None => "Ganzes Blatt".into(),
        Some(f) => PHOTO_SIZES.iter().find(|(_, p)| *p == f).map_or_else(|| f.label(), |(n, _)| n.to_string()),
    }
}

/// A credit card (ISO/IEC 7810 ID-1): something everyone has at hand to measure the screen.
const CARD_MM: Vec2 = vec2(85.6, 53.98);

/// Screen resolutions the calibration offers (physical pixels per inch).
const DPI_RANGE: std::ops::RangeInclusive<f32> = 40.0..=320.0;
/// Space around the largest card in the calibration dialog.
const CAL_PAD: f32 = 12.0;

/// Paper drawn around the image in the viewer.
pub struct PaperFrame {
    /// Paper size relative to the image's size on screen, per axis (below 1 where filling cuts).
    pub rel: Vec2,
    /// The printed part of the image, relative to its size (1: all of it).
    pub window: Vec2,
    pub color: Color32,
    /// The print layout (with the print preview), for border, crop marks and caption.
    pub layout: Option<Layout>,
}

impl App {
    /// Strg+P: print preview on (at the printed size) or off.
    pub fn toggle_print_preview(&mut self) {
        self.print.on = !self.print.on;
        if self.print.zoom != self.print.on {
            self.print.zoom = self.print.on;
            self.zoom = None;
            self.pan = Vec2::ZERO;
        }
    }

    /// Umschalt+Z: the image at its printed size (turns the print preview on) or back to fit.
    pub fn toggle_print_zoom(&mut self) {
        self.print.zoom = !self.print.zoom;
        self.print.on |= self.print.zoom;
        if !self.print.zoom {
            self.zoom = None;
        }
        self.pan = Vec2::ZERO;
    }

    /// Identifies the monitor the app is on, for its calibration.
    fn monitor_key(&self, ctx: &egui::Context) -> Option<String> {
        let size = ctx.input(|i| i.viewport().monitor_size)?;
        Some(format!("{:.0}x{:.0}@{:.2}", size.x, size.y, self.pixels_per_point))
    }

    /// Physical screen pixels per millimeter, and whether they were measured (else 96 dpi).
    pub(crate) fn screen_px_per_mm(&self, ctx: &egui::Context) -> (f32, bool) {
        let key = self.monitor_key(ctx);
        match self.print.settings.calibration.iter().find(|(k, _)| Some(k) == key.as_ref()) {
            Some((_, v)) => (*v, true),
            None => (96.0 / 25.4 * self.pixels_per_point, false),
        }
    }

    /// The print layout of the current image, if the print preview is on.
    pub(crate) fn print_layout(&self, size: Vec2) -> Option<Layout> {
        if !self.print.on || self.current.is_none() {
            return None;
        }
        let w = self.full_output_width()?;
        Some(sk_core::print::layout(w, w * size.y / size.x.max(1.0), &self.page_settings()))
    }

    /// The print settings for the page shown: turned as chosen for it, else like its image.
    pub(crate) fn page_settings(&self) -> PrintSettings {
        let path = self.current.as_ref().map(|c| &c.meta.path);
        PrintSettings { portrait: path.and_then(|p| self.print.turned.get(p).copied()), ..self.print.settings.clone() }
    }

    /// Whether the page shown is upright: as chosen, else like its image.
    pub(crate) fn page_portrait(&self) -> bool {
        let (w, h) = self.full_output_size().unwrap_or((3.0, 2.0));
        self.page_settings().portrait.unwrap_or(h > w)
    }

    /// Hochformat/Querformat for the page shown; back to "like the image" when it matches.
    pub(crate) fn turn_page(&mut self, portrait: bool) {
        let Some(path) = self.current.as_ref().map(|c| c.meta.path.clone()) else { return };
        let natural = self.full_output_size().is_some_and(|(w, h)| h > w);
        if portrait == natural {
            self.print.turned.remove(&path);
        } else {
            self.print.turned.insert(path, portrait);
        }
    }

    /// The paper around the image (`size`: the image on screen at scale 1): the print format
    /// with the print preview, else a margin while soft proofing.
    pub(crate) fn paper_frame(&self, size: Vec2) -> Option<PaperFrame> {
        if self.crop_tool.is_some() || self.straighten_tool.is_some() {
            return None;
        }
        let soft = self.softproof.transform.as_ref().filter(|_| self.softproof.on);
        // White, or the paper's tone when the soft proof simulates it.
        let color = soft.map_or(Color32::WHITE, |t| Color32::from_rgb(t.paper[0], t.paper[1], t.paper[2]));
        if let Some(l) = self.print_layout(size) {
            return Some(PaperFrame {
                rel: vec2(l.paper[0] / l.image[0], l.paper[1] / l.image[1]),
                window: vec2(l.window[0] / l.image[0], l.window[1] / l.image[1]),
                color,
                layout: Some(l),
            });
        }
        if soft.is_some() && self.softproof.settings.paper_margin && !self.proof_view {
            let m = 0.06 * size.x.max(size.y);
            return Some(PaperFrame { rel: (size + Vec2::splat(2.0 * m)) / size, window: Vec2::splat(1.0), color, layout: None });
        }
        None
    }

    /// Scale (points per image texture pixel) at which the image appears at its printed size.
    pub(crate) fn print_size_scale(&self, ctx: &egui::Context, size: Vec2) -> Option<f32> {
        let l = self.print_layout(size)?;
        let (px_per_mm, _) = self.screen_px_per_mm(ctx);
        Some(l.image[0] * px_per_mm / self.pixels_per_point / size.x.max(1.0))
    }

    /// Label in the viewer: format, resolution on paper and its rating (background color), with
    /// the explanation of its short hints as tooltip.
    pub(crate) fn print_label(&self, ctx: &egui::Context, size: Vec2) -> Option<(String, Color32, Option<String>)> {
        let l = self.print_layout(size)?;
        let s = &self.print.settings;
        let off = (l.off * 100.0).round();
        let off = match s.fit {
            _ if off < 1.0 => String::new(),
            Fit::Contain => format!("  ·  Rand {off:.0} %"),
            Fit::Fill => format!("  ·  schneidet {off:.0} % ab"),
        };
        let mut hints = Vec::new();
        if s.fit == Fit::Fill && !off.is_empty() {
            hints.push("Umschalt gedrückt halten: zeigt, was abgeschnitten wird".to_string());
        }
        let zoom = match (self.print.zoom, self.screen_px_per_mm(ctx).1) {
            (false, _) => "",
            (true, true) => "  ·  Druckgröße",
            (true, false) => {
                hints.push(format!("Bildschirm nicht kalibriert, die Druckgröße ist geschätzt (Tab „Ausgabe“ {} Bildschirm kalibrieren)", theme::icon::ARROW_RIGHT));
                "  ·  Druckgröße geschätzt"
            }
        };
        let color = if l.ppi < PPI_POOR {
            theme::DANGER.gamma_multiply(0.9)
        } else if l.ppi < PPI_LOW {
            Color32::from_rgb(170, 130, 30)
        } else {
            Color32::from_rgb(50, 120, 90)
        };
        // Size and cut are judged here, colors only with the soft proof.
        let colors = if self.softproof.on {
            ""
        } else {
            hints.push(crate::commands::hint("Softproof aus: die Farben zeigen den Bildschirm, nicht das Papier", "softproof"));
            "  ·  Softproof aus"
        };
        let size = s.image_size.map(|f| format!("  ·  Bild {}", f.label())).unwrap_or_default();
        let text = format!("DRUCK  ·  {}{size}  ·  {}  ·  {:.0} ppi{off}{zoom}{colors}", s.format.label(), s.fit.label(), l.ppi);
        Some((text, color, (!hints.is_empty()).then(|| hints.join("\n"))))
    }

    /// Border, crop marks and caption of the print preview, around the image on screen
    /// (`img_rect`: the whole image, as laid out).
    pub(crate) fn draw_print_extras(&self, painter: &egui::Painter, img_rect: egui::Rect, l: &Layout) {
        let s = &self.print.settings;
        // Points per millimeter on screen.
        let k = img_rect.width() / l.image[0].max(1e-3);
        let paper = egui::Rect::from_center_size(img_rect.center(), vec2(l.paper[0], l.paper[1]) * k);
        let at = |x: f32, y: f32| paper.min + vec2(x, y) * k;
        let [x, y, w, h] = l.window_rect();
        let window = egui::Rect::from_min_max(at(x, y), at(x + w, y + h));
        let ink = Color32::from_gray(20);
        if s.border_mm > 0.0 {
            let b = (s.border_mm * k).max(1.0 / self.pixels_per_point);
            painter.rect_stroke(window, 0.0, egui::Stroke::new(b, ink), egui::StrokeKind::Outside);
        }
        if s.crop_marks {
            let line = egui::Stroke::new((sk_core::print::MARK_LINE_MM * k).max(1.0 / self.pixels_per_point), ink);
            for (a, b) in l.crop_marks(s.border_mm) {
                painter.line_segment([at(a[0], a[1]), at(b[0], b[1])], line);
            }
        }
        if let Some(cur) = &self.current
            && let Some(text) = s.caption.text(&cur.meta.path, &cur.meta.info)
        {
            let size = sk_core::print::CAPTION_MM * k;
            if size >= 3.0 {
                painter.text(egui::pos2(paper.center().x, at(0.0, l.caption_top()).y), egui::Align2::CENTER_TOP, text, egui::FontId::proportional(size), Color32::from_gray(77));
            }
        }
    }

    fn change_print_settings(&mut self, f: impl FnOnce(&mut PrintSettings)) {
        let before = self.print.settings.clone();
        f(&mut self.print.settings);
        if self.print.settings != before {
            self.save_settings();
        }
    }

    /// The "Druck" page of the Ausgabe tab: the page at a glance with the print buttons, paper,
    /// the image on the page, and the print adjustment and colors folded.
    pub fn print_page(&mut self, ui: &mut Ui) {
        if self.softproof.found.is_none() {
            self.softproof.found = Some(sk_core::softproof::find_profiles());
        }
        let printers = self.printers(ui.ctx());
        let mut s = self.print.settings.clone();
        let portrait = self.page_portrait();
        let size = self.full_output_size();
        let (mut turn, mut toggle, mut print, mut to_file, mut pick) = (None, false, false, false, false);

        // The page at a glance, the buttons and the preview switch.
        let layout = size.map(|(w, h)| sk_core::print::layout(w, h, &self.page_settings()));
        let thumb = self.current.as_ref().and_then(|c| self.thumbs.get(&c.meta.path)).map(|t| t.id());
        let print_blocked = self.current.is_none() || self.print.bar;
        let file_blocked = self.print_blocked_reason(true);
        crate::output::card(ui, None, |ui| {
            ui.horizontal(|ui| {
                page_thumb(ui, layout.as_ref(), &s, thumb);
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    page_texts(ui, layout.as_ref(), &s, portrait);
                });
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let file_w = 96.0;
                let label = RichText::new(format!("{}  Drucken …", theme::icon::PRINTER)).size(14.0).strong().color(Color32::WHITE);
                let b = egui::Button::new(label).fill(theme::ACCENT).corner_radius(8).min_size(vec2(ui.available_width() - file_w - 6.0, 34.0));
                let r = ui.add_enabled(!print_blocked, crate::dialog::unless_enabled(b, !print_blocked));
                let r = r.on_hover_text(crate::commands::hint("Zeigt die Seite mit Drucker und Papier, dann wird gedruckt; die ausgewählten Bilder, eines je Seite", "print"));
                if r.clicked() {
                    print = true;
                }
                let b = egui::Button::new(format!("{}  In Datei …", theme::icon::FILE_PDF)).corner_radius(8).min_size(vec2(file_w, 34.0));
                let r = ui.add_enabled(file_blocked.is_none(), b);
                let r = match file_blocked {
                    Some(why) => r.on_disabled_hover_text(why),
                    None => r.on_hover_text("Die Seiten als PDF oder JPEG speichern, etwa für ein Fotolabor"),
                };
                if r.clicked() {
                    to_file = true;
                }
            });
            ui.add_space(6.0);
            ui.add_enabled_ui(self.current.is_some() || self.print.on, |ui| {
                if crate::output::switch_row(ui, "Druckvorschau", self.print.on, &crate::commands::hint("Druckvorschau anzeigen", "print-preview")) {
                    toggle = true;
                }
            });
        });

        widgets::nav_header(ui, "Papier", "Papier", |_| {});
        if crate::printing::CAN_PRINT {
            widgets::combo_field(ui, "Drucker", crate::printing::printer_text(&s, printers.as_deref()), |ui| {
                crate::printing::printer_entries(ui, &mut s, printers.as_deref());
            });
        }
        let (text, hint) = crate::printing::paper_text(&s, printers.as_deref());
        widgets::combo_field(ui, "Format", text, |ui| {
            crate::printing::paper_entries(ui, &mut s, printers.as_deref());
        });
        if let Some(hint) = hint {
            ui.label(RichText::new(hint).small().color(theme::GUIDE));
        }
        widgets::field(ui, "Eigenes", |ui| {
            if let Some(f) = cm_pair(ui, s.format) {
                s.format = f;
            }
        });
        let turned = widgets::field(ui, "Ausrichtung", |ui| {
            let b = [
                widgets::BarButton::new("", "Hoch").name("Hochformat").selected(portrait),
                widgets::BarButton::new("", "Quer").name("Querformat").selected(!portrait),
            ];
            ui.add_enabled_ui(self.current.is_some(), |ui| widgets::button_bar(ui, "page-turn", &b)).inner
        });
        if let Some(k) = turned {
            turn = Some(k == 0);
        }

        widgets::nav_header(ui, "Bild auf der Seite", "Bild auf der Seite", |_| {});
        widgets::combo_field(ui, "Bildgröße", image_size_label(s.image_size), |ui| {
            ui.selectable_value(&mut s.image_size, None, image_size_label(None)).on_hover_text("So groß, wie Papier und Rand erlauben");
            for (name, f) in PHOTO_SIZES {
                ui.selectable_value(&mut s.image_size, Some(f), name);
            }
        });
        if let Some(f) = s.image_size {
            widgets::field(ui, "Eigene", |ui| {
                if let Some(f) = cm_pair(ui, f) {
                    s.image_size = Some(f);
                }
            });
        }
        s.portrait = Some(portrait);
        let fit = widgets::field(ui, "Anpassung", |ui| {
            let b: Vec<widgets::BarButton> = [Fit::Contain, Fit::Fill].iter().map(|f| widgets::BarButton::new("", f.label()).hover(fit_effect(&s, *f, size)).selected(s.fit == *f)).collect();
            widgets::button_bar(ui, "fit", &b)
        });
        s.portrait = None;
        if let Some(k) = fit {
            s.fit = [Fit::Contain, Fit::Fill][k];
        }
        let mm = widgets::SliderOpts { suffix: " mm", ..Default::default() };
        // With a fixed image size the margin is what is left around it.
        ui.add_enabled_ui(s.image_size.is_none(), |ui| {
            widgets::slider_row_with(ui, "Rand", &mut s.margin_mm, 0.0..=50.0, 0.0, 0, mm);
        })
        .response
        .on_disabled_hover_text("Mit fester Bildgröße ergibt sich der Rand von selbst");
        let kontur = widgets::SliderOpts { suffix: " mm", help: Some("Feine schwarze Linie um das Bild: grenzt helle Bildränder vom Papier ab. 0: keine."), ..Default::default() };
        widgets::slider_row_with(ui, "Kontur", &mut s.border_mm, 0.0..=2.0, 0.0, 1, kontur);
        widgets::combo_field(ui, "Beschriftung", s.caption.label(), |ui| {
            for c in Caption::ALL {
                ui.selectable_value(&mut s.caption, c, c.label());
            }
        });
        let marks_hover = "Linien an den Ecken, außerhalb des Bildes: zum Zuschneiden, wenn das Papier größer ist als das Bild. Brauchen einen Rand.";
        if crate::output::switch_row(ui, "Schnittmarken", s.crop_marks, marks_hover) {
            s.crop_marks = !s.crop_marks;
        }

        ui.add_space(10.0);
        let mut adj = Vec::new();
        if s.brightness.round() != 0.0 {
            adj.push(format!("Hell {:+.0}", s.brightness));
        }
        if s.contrast.round() != 0.0 {
            adj.push(format!("Kontrast {:+.0}", s.contrast));
        }
        if s.sharpening != PrintSharpening::Off {
            adj.push(format!("{}, {}", s.sharpening.label(), s.paper.label()));
        }
        let adj = if adj.is_empty() { "Aus".to_string() } else { adj.join(" · ") };
        crate::output::fold_card(ui, "Druckanpassung", theme::icon::SUN, &adj, None, |ui| {
            widgets::slider_row(ui, "Helligkeit", &mut s.brightness, -50.0..=50.0, 0.0, 0);
            widgets::slider_row(ui, "Kontrast", &mut s.contrast, -50.0..=50.0, 0.0, 0);
            let sharp = widgets::field(ui, "Schärfen", |ui| {
                let b: Vec<widgets::BarButton> = PrintSharpening::ALL.iter().map(|v| widgets::BarButton::new("", v.label()).selected(s.sharpening == *v)).collect();
                widgets::button_bar(ui, "print-sharp", &b)
            });
            if let Some(k) = sharp {
                s.sharpening = PrintSharpening::ALL[k];
            }
            ui.add_enabled_ui(s.sharpening != PrintSharpening::Off, |ui| {
                let paper = widgets::field(ui, "Oberfläche", |ui| {
                    let b = [widgets::BarButton::new("", Paper::Glossy.label()).selected(s.paper == Paper::Glossy), widgets::BarButton::new("", Paper::Matte.label()).selected(s.paper == Paper::Matte)];
                    widgets::button_bar(ui, "print-paper", &b)
                });
                if let Some(k) = paper {
                    s.paper = if k == 0 { Paper::Glossy } else { Paper::Matte };
                }
            })
            .response
            .on_hover_text("Mattes Papier saugt die Tinte auf und braucht mehr Schärfe");
        })
        ;
        ui.add_space(6.0);
        let colors = s.profile.as_ref().map_or_else(|| "Drucker steuert".to_string(), |p| sk_core::softproof::profile_name(p));
        crate::output::fold_card(ui, "Farben", theme::icon::PALETTE, &colors, None, |ui| {
            widgets::field(ui, "Profil", |ui| {
                let r = egui::ComboBox::from_id_salt("print-profile").selected_text(colors.clone()).width(ui.available_width() - 8.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.profile, None, "Drucker steuert").on_hover_text("Der Drucker bekommt sRGB und rechnet die Farben selbst um");
                    let known = &self.softproof.settings.profiles;
                    let mut files: Vec<(std::path::PathBuf, String)> = known.iter().map(|p| (p.clone(), sk_core::softproof::profile_name(p))).collect();
                    files.extend(self.softproof.found.iter().flatten().filter(|(p, _)| !known.contains(p)).cloned());
                    if !files.is_empty() {
                        ui.separator();
                    }
                    for (p, n) in files {
                        ui.selectable_value(&mut s.profile, Some(p.clone()), n).on_hover_text(p.display().to_string());
                    }
                    ui.separator();
                    if ui.button(format!("{}  ICC-Profil laden …", theme::icon::FOLDER_OPEN)).clicked() {
                        pick = true;
                    }
                });
                r.response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "Profil"));
                r.response.on_hover_text("Mit Profil rechnet Silberkorn die Farben um: im Druckertreiber „Keine Farbkorrektur“ wählen, falls er es anbietet, sonst wird doppelt umgerechnet.");
            });
            if s.profile.is_some() {
                let intent = widgets::field(ui, "Priorität", |ui| {
                    let b = [
                        widgets::BarButton::new("", "Wahrnehmung").name(RenderIntent::Perceptual.label()).selected(s.intent == RenderIntent::Perceptual),
                        widgets::BarButton::new("", "Relativ").name(RenderIntent::RelativeColorimetric.label()).selected(s.intent == RenderIntent::RelativeColorimetric),
                    ];
                    widgets::button_bar(ui, "print-intent", &b)
                });
                if let Some(k) = intent {
                    s.intent = if k == 0 { RenderIntent::Perceptual } else { RenderIntent::RelativeColorimetric };
                }
            }
        });

        if pick
            && let Some(p) = rfd::FileDialog::new().add_filter("ICC-Profil", &["icc", "icm"]).set_title("ICC-Profil des Druckers wählen").pick_file()
        {
            s.profile = Some(p);
        }
        self.change_print_settings(|old| *old = s);
        if let Some(p) = turn {
            self.turn_page(p);
        }
        if toggle {
            self.toggle_print_preview();
        }
        if print {
            self.print_command();
        }
        if to_file {
            self.print_to_file_dialog();
        }
    }

    /// Below the soft proof on the print page: the printed size and the screen calibration.
    pub(crate) fn print_extra_buttons(&mut self, ui: &mut Ui) {
        ui.add_space(8.0);
        let (mut zoom, mut calibrate) = (false, false);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.spacing_mut().item_spacing.x = 6.0;
            let w = ((ui.available_width() - 14.0) / 2.0).max(60.0);
            let b = egui::Button::new(crate::commands::hint("Druckgröße", "print-zoom")).selected(self.print.zoom).min_size(vec2(w, 28.0)).truncate();
            if ui.add(b).on_hover_text("Zeigt das Bild in echter Druckgröße").clicked() {
                zoom = true;
            }
            if ui.add(egui::Button::new("Bildschirm kalibrieren …").min_size(vec2(w, 28.0)).truncate()).clicked() {
                calibrate = true;
            }
        });
        if zoom {
            self.toggle_print_zoom();
        }
        if calibrate {
            self.print.calibrating = Some(self.screen_px_per_mm(ui.ctx()).0);
        }
    }

    /// The calibration dialog: a credit card drawn on screen, resized until it matches a real one.
    pub(crate) fn print_dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut px_per_mm) = self.print.calibrating else { return };
        let ppp = self.pixels_per_point;
        let (mut save, mut cancel) = (false, false);
        // A fixed measuring area, made for the largest card the slider offers, so the dialog keeps
        // its size: up to twice the resolution the system scaling suggests, as far as the screen allows.
        let screen = ctx.content_rect().size();
        let fit_dpi = |pt: f32, mm: f32| pt * ppp * 25.4 / mm;
        let max_dpi = (96.0 * ppp * 2.0)
            .min(*DPI_RANGE.end())
            .min(fit_dpi(screen.x - 2.0 * CAL_PAD - 120.0, CARD_MM.x))
            .min(fit_dpi(screen.y - 2.0 * CAL_PAD - 280.0, CARD_MM.y))
            .max(*DPI_RANGE.start() + 1.0);
        let range = *DPI_RANGE.start()..=max_dpi;
        let area = CARD_MM * max_dpi / 25.4 / ppp + Vec2::splat(2.0 * CAL_PAD);
        px_per_mm = px_per_mm.clamp(*range.start() / 25.4, max_dpi / 25.4);
        let dialog = crate::dialog::Dialog::new("print-calibration", crate::theme::icon::RULER, "Bildschirm kalibrieren")
            .subtitle("Damit 100 % in der Druckvorschau echte Zentimeter sind")
            .width(area.x.max(460.0));
        let modal = dialog.show(ctx, |ui| {
            ui.label("Eine Scheckkarte (EC- oder Kreditkarte) an den Bildschirm halten und die Größe ändern, bis das Rechteck genau so groß ist wie die Karte.");
            ui.add_space(6.0);
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), area.y), egui::Sense::hover());
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 6.0, theme::VIEWER);
            let card = CARD_MM * px_per_mm / ppp;
            let r = egui::Rect::from_center_size(rect.center(), card);
            let rounding = 3.18 * px_per_mm / ppp;
            painter.rect_filled(r, rounding, theme::ACCENT_SOFT);
            painter.rect_stroke(r, rounding, egui::Stroke::new(1.0, theme::ACCENT), egui::StrokeKind::Inside);
            painter.text(r.center(), egui::Align2::CENTER_CENTER, "85,6 × 54 mm", egui::FontId::proportional(13.0), theme::TEXT);
            ui.add_space(6.0);
            // In dpi, like the other sliders: Shift while dragging for fine steps, the number to type.
            let mut dpi = px_per_mm * 25.4;
            let opts = widgets::SliderOpts { suffix: " dpi", ..Default::default() };
            widgets::slider_row_with(ui, "Auflösung", &mut dpi, range.clone(), (96.0 * ppp).clamp(*range.start(), *range.end()), 1, opts);
            px_per_mm = dpi.clamp(*range.start(), *range.end()) / 25.4;
            crate::dialog::note(ui, "Fein: Umschalt beim Ziehen halten, oder den Wert eintippen.");
            crate::dialog::footer(ui, |ui| {
                if crate::dialog::default_button(ui, true, crate::dialog::primary("Übernehmen")) {
                    save = true;
                }
                if ui.add(crate::dialog::secondary("Abbrechen")).clicked() {
                    cancel = true;
                }
            });
        });
        if modal.should_close() {
            cancel = true;
        }
        self.print.calibrating = Some(px_per_mm);
        if save {
            if let Some(key) = self.monitor_key(ctx) {
                self.change_print_settings(|s| {
                    s.calibration.retain(|(k, _)| *k != key);
                    s.calibration.push((key, px_per_mm));
                });
            }
            self.print.calibrating = None;
        } else if cancel {
            self.print.calibrating = None;
        }
    }
}

/// Width × height in cm (short side first); the new format when changed.
fn cm_pair(ui: &mut Ui, f: PrintFormat) -> Option<PrintFormat> {
    let (mut w, mut h) = (f.w_mm.min(f.h_mm) / 10.0, f.w_mm.max(f.h_mm) / 10.0);
    let a = ui.add(egui::DragValue::new(&mut w).range(2.0..=200.0).speed(0.1).max_decimals(1));
    ui.label("×");
    let b = ui.add(egui::DragValue::new(&mut h).range(2.0..=200.0).speed(0.1).max_decimals(1));
    ui.label(RichText::new("cm").color(theme::TEXT_DIM));
    (a.changed() || b.changed()).then(|| PrintFormat::new(w * 10.0, h * 10.0))
}

/// The page small, to scale: paper, the image (its thumbnail), Kontur, crop marks and caption.
fn page_thumb(ui: &mut Ui, layout: Option<&Layout>, s: &PrintSettings, thumb: Option<egui::TextureId>) {
    let (rect, _) = ui.allocate_exact_size(vec2(104.0, 104.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 8.0, theme::CHROME);
    let Some(l) = layout else { return };
    let k = 88.0 / l.paper[0].max(l.paper[1]).max(1.0);
    let paper = egui::Rect::from_center_size(rect.center(), vec2(l.paper[0], l.paper[1]) * k);
    p.rect_filled(paper, 1.0, Color32::from_gray(244));
    let at = |x: f32, y: f32| paper.min + vec2(x, y) * k;
    let [x, y, w, h] = l.window_rect();
    let window = egui::Rect::from_min_max(at(x, y), at(x + w, y + h));
    match thumb {
        Some(id) => {
            let [u, v, uw, vh] = l.window_in_image();
            p.image(id, window, egui::Rect::from_min_size(egui::pos2(u, v), vec2(uw, vh)), Color32::WHITE);
        }
        None => {
            p.rect_filled(window, 0.0, Color32::from_rgb(91, 107, 75));
        }
    }
    let ink = Color32::from_gray(20);
    if s.border_mm > 0.0 {
        p.rect_stroke(window, 0.0, egui::Stroke::new((s.border_mm * k).max(1.0), ink), egui::StrokeKind::Outside);
    }
    if s.crop_marks {
        for (a, b) in l.crop_marks(s.border_mm) {
            p.line_segment([at(a[0], a[1]), at(b[0], b[1])], egui::Stroke::new(1.0, Color32::from_gray(110)));
        }
    }
    if s.caption != Caption::Off {
        let top = at(0.0, l.caption_top()).y + 1.0;
        let cw = (window.width() * 0.5).min(40.0);
        p.rect_filled(egui::Rect::from_min_size(egui::pos2(paper.center().x - cw / 2.0, top), vec2(cw, 2.0)), 0.0, Color32::from_gray(185));
    }
}

/// Paper and orientation, the image's printed size and the resolution as a colored chip.
fn page_texts(ui: &mut Ui, layout: Option<&Layout>, s: &PrintSettings, portrait: bool) {
    let width = ui.available_width();
    let paper = format!("{} {}", s.format.label(), if portrait { "hoch" } else { "quer" });
    let g = crate::output::truncated(ui, paper, 15.5, Color32::WHITE, width);
    ui.add(egui::Label::new(egui::WidgetText::Galley(g)).selectable(false));
    let Some(l) = layout else {
        ui.label(RichText::new("Kein Bild").size(12.0).color(theme::TEXT_DIM));
        return;
    };
    let cut = if s.fit == Fit::Fill && l.off > 0.005 { ", beschnitten" } else { "" };
    let line = format!("Bild {:.1} × {:.1} cm{cut}", l.window[0] / 10.0, l.window[1] / 10.0);
    let g = crate::output::truncated(ui, line, 12.0, theme::TEXT_DIM, width);
    ui.add(egui::Label::new(egui::WidgetText::Galley(g)).selectable(false));
    ui.add_space(4.0);
    let color = if l.ppi < PPI_POOR {
        theme::DANGER
    } else if l.ppi < PPI_LOW {
        Color32::from_rgb(230, 200, 74)
    } else {
        Color32::from_rgb(95, 184, 90)
    };
    let g = ui.painter().layout_no_wrap(format!("{:.0} ppi", l.ppi), egui::FontId::proportional(12.0), color);
    let (rect, resp) = ui.allocate_exact_size(g.size() + vec2(16.0, 6.0), egui::Sense::hover());
    ui.painter().rect_stroke(rect, rect.height() / 2.0, egui::Stroke::new(1.0, color), egui::StrokeKind::Inside);
    ui.painter().galley(rect.center() - g.size() / 2.0, g, color);
    resp.on_hover_text(format!("Auflösung auf dem Papier. Unter {PPI_LOW:.0} ppi gelb, unter {PPI_POOR:.0} ppi rot: dort werden Details im Druck weich."));
}
