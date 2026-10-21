//! Print preview in the viewer (see `sk_core::print`): the paper format around the image, the
//! resolution on paper and, on a calibrated screen, the image at its printed size.

use egui::{Color32, RichText, Ui, Vec2, vec2};
use sk_core::print::{Fit, Layout, PPI_LOW, PPI_POOR, PRESETS, PrintFormat, PrintSettings};

use crate::app::App;
use crate::theme;
use crate::widgets::{self, section};

/// Print preview state of the app.
#[derive(Default)]
pub struct PrintPreviewState {
    pub on: bool,
    pub settings: PrintSettings,
    /// The viewer shows the image at its printed size (until zoomed otherwise).
    pub zoom: bool,
    /// The calibration dialog is open, with the physical pixels per millimeter being tried.
    pub calibrating: Option<f32>,
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
    pub color: Color32,
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
        let s = &self.print.settings;
        Some(sk_core::print::layout(w, w * size.y / size.x.max(1.0), s.format, s.fit))
    }

    /// The paper around the image (`size`: the image on screen at scale 1): the print format
    /// with the print preview, else a margin while soft proofing.
    pub(crate) fn paper_frame(&self, size: Vec2) -> Option<PaperFrame> {
        if self.crop_tool.is_some() || self.straighten_tool.is_some() {
            return None;
        }
        let soft = self.softproof.transform.as_ref().filter(|_| self.softproof.on);
        let color = self.print_background().unwrap_or_else(|| soft.map_or(Color32::WHITE, |t| Color32::from_rgb(t.paper[0], t.paper[1], t.paper[2])));
        if let Some(l) = self.print_layout(size) {
            return Some(PaperFrame { rel: vec2(l.paper[0] / l.image[0], l.paper[1] / l.image[1]), color });
        }
        if soft.is_some() && self.softproof.settings.paper_margin && !self.proof_view {
            let m = 0.06 * size.x.max(size.y);
            return Some(PaperFrame { rel: (size + Vec2::splat(2.0 * m)) / size, color });
        }
        None
    }

    /// Background of the viewer in the print preview: the paper's color.
    pub(crate) fn print_background(&self) -> Option<Color32> {
        if !self.print.on || self.current.is_none() || self.crop_tool.is_some() || self.straighten_tool.is_some() {
            return None;
        }
        let soft = self.softproof.transform.as_ref().filter(|_| self.softproof.on);
        Some(soft.map_or(Color32::WHITE, |t| Color32::from_rgb(t.paper[0], t.paper[1], t.paper[2])))
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
        let text = format!("DRUCK  ·  {}  ·  {}  ·  {:.0} ppi{off}{zoom}{colors}", s.format.label(), s.fit.label(), l.ppi);
        Some((text, color, (!hints.is_empty()).then(|| hints.join("\n"))))
    }

    fn change_print_settings(&mut self, f: impl FnOnce(&mut PrintSettings)) {
        let before = self.print.settings.clone();
        f(&mut self.print.settings);
        if self.print.settings != before {
            self.save_settings();
        }
    }

    /// Controls in the output tab.
    pub fn print_section(&mut self, ui: &mut Ui) {
        let mut s = self.print.settings.clone();
        let (mut toggle, mut zoom, mut calibrate) = (false, false, false);
        section(ui, "Druckvorschau", None, |ui| {
            let mut on = self.print.on;
            let checkbox = crate::widgets::Check::new(&mut on, crate::commands::hint("Druckvorschau in Druckgröße", "print-preview"));
            if ui.add_enabled(self.current.is_some() || self.print.on, checkbox).changed() {
                toggle = true;
            }
            widgets::combo_field(ui, "Papierformat", s.format.label(), |ui| {
                for (name, f) in PRESETS {
                    ui.selectable_value(&mut s.format, f, name);
                }
            });
            widgets::field(ui, "Eigenes (cm)", |ui| {
                let (mut w, mut h) = (s.format.w_mm.min(s.format.h_mm) / 10.0, s.format.w_mm.max(s.format.h_mm) / 10.0);
                let a = ui.add(egui::DragValue::new(&mut w).range(2.0..=200.0).speed(0.1).max_decimals(1));
                ui.label("×");
                let b = ui.add(egui::DragValue::new(&mut h).range(2.0..=200.0).speed(0.1).max_decimals(1));
                if a.changed() || b.changed() {
                    s.format = PrintFormat::new(w * 10.0, h * 10.0);
                }
            });
            widgets::combo_field(ui, "Anpassung", s.fit.label(), |ui| {
                ui.selectable_value(&mut s.fit, Fit::Contain, Fit::Contain.label()).on_hover_text("Ganzes Bild, Rand wo die Seitenverhältnisse abweichen");
                ui.selectable_value(&mut s.fit, Fit::Fill, Fit::Fill.label()).on_hover_text("Randlos wie im Fotolabor, das Bild wird beschnitten");
            });
            // Wraps instead of widening the panel (the label carries the shortcut).
            ui.horizontal_wrapped(|ui| {
                if ui.selectable_label(self.print.zoom, crate::commands::hint("Druckgröße", "print-zoom")).clicked() {
                    zoom = true;
                }
                if ui.button("Bildschirm kalibrieren …").clicked() {
                    calibrate = true;
                }
            });
            ui.label(
                RichText::new(format!("Unter {PPI_LOW:.0} ppi gelb, unter {PPI_POOR:.0} ppi rot: dort werden Details im Druck weich."))
                    .small()
                    .color(theme::TEXT_FAINT),
            );
        });
        self.change_print_settings(|old| *old = s);
        if toggle {
            self.toggle_print_preview();
        }
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
