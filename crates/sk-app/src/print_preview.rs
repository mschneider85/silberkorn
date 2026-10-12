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

/// Paper drawn around the image in the viewer.
pub struct PaperFrame {
    /// Paper size relative to the image's size on screen, per axis (below 1 where filling cuts).
    pub rel: Vec2,
    pub color: Color32,
}

impl App {
    /// Strg+P: print preview on or off.
    pub fn toggle_print_preview(&mut self) {
        self.print.on = !self.print.on;
        if !self.print.on && self.print.zoom {
            self.print.zoom = false;
            self.zoom = None;
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
        let color = soft.map_or(Color32::WHITE, |t| Color32::from_rgb(t.paper[0], t.paper[1], t.paper[2]));
        if let Some(l) = self.print_layout(size) {
            return Some(PaperFrame { rel: vec2(l.paper[0] / l.image[0], l.paper[1] / l.image[1]), color });
        }
        if soft.is_some() && self.softproof.settings.paper_margin && !self.proof_view {
            let m = 0.06 * size.x.max(size.y);
            return Some(PaperFrame { rel: (size + Vec2::splat(2.0 * m)) / size, color });
        }
        None
    }

    /// Scale (points per image texture pixel) at which the image appears at its printed size.
    pub(crate) fn print_size_scale(&self, ctx: &egui::Context, size: Vec2) -> Option<f32> {
        let l = self.print_layout(size)?;
        let (px_per_mm, _) = self.screen_px_per_mm(ctx);
        Some(l.image[0] * px_per_mm / self.pixels_per_point / size.x.max(1.0))
    }

    /// Label in the viewer: format, resolution on paper and its rating (background color).
    pub(crate) fn print_label(&self, ctx: &egui::Context, size: Vec2) -> Option<(String, Color32)> {
        let l = self.print_layout(size)?;
        let s = &self.print.settings;
        let off = (l.off * 100.0).round();
        let off = match s.fit {
            _ if off < 1.0 => String::new(),
            Fit::Contain => format!("  ·  Rand {off:.0} %"),
            Fit::Fill => format!("  ·  schneidet {off:.0} % ab"),
        };
        let zoom = if self.print.zoom {
            if self.screen_px_per_mm(ctx).1 { "  ·  Druckgröße" } else { "  ·  Druckgröße ≈ (Bildschirm nicht kalibriert)" }
        } else {
            ""
        };
        let color = if l.ppi < PPI_POOR {
            theme::DANGER.gamma_multiply(0.9)
        } else if l.ppi < PPI_LOW {
            Color32::from_rgb(170, 130, 30)
        } else {
            Color32::from_rgb(50, 120, 90)
        };
        Some((format!("DRUCK  ·  {}  ·  {}  ·  {:.0} ppi{off}{zoom}", s.format.label(), s.fit.label(), l.ppi), color))
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
            if ui.checkbox(&mut self.print.on.clone(), "Druckformat im Viewer (Strg+P)").changed() {
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
            ui.horizontal(|ui| {
                if ui.selectable_label(self.print.zoom, "Druckgröße zeigen (Umschalt+Z)").clicked() {
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
        let modal = egui::Modal::new(egui::Id::new("print-calibration")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(RichText::new("Bildschirm kalibrieren").size(17.0).strong().color(Color32::WHITE));
            ui.add_space(6.0);
            ui.label("Halte eine Scheckkarte (EC- oder Kreditkarte) an den Bildschirm und ändere die Größe, bis das Rechteck genau so groß ist wie die Karte.");
            ui.add_space(10.0);
            let card = CARD_MM * px_per_mm / ppp;
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), card.y + 16.0), egui::Sense::hover());
            let r = egui::Rect::from_center_size(rect.center(), card);
            ui.painter().rect_filled(r, 3.5 * px_per_mm / ppp, theme::ACCENT_SOFT);
            ui.painter().rect_stroke(r, 3.5 * px_per_mm / ppp, egui::Stroke::new(1.0, theme::ACCENT), egui::StrokeKind::Inside);
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "85,6 × 54 mm", egui::FontId::proportional(13.0), theme::TEXT);
            ui.add_space(6.0);
            widgets::field(ui, "Größe", |ui| {
                ui.add(egui::Slider::new(&mut px_per_mm, 1.5..=12.0).logarithmic(true).show_value(false));
            });
            ui.label(RichText::new(format!("{:.0} dpi", px_per_mm * 25.4)).small().color(theme::TEXT_FAINT));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("Übernehmen").clicked() {
                    save = true;
                }
                if ui.button("Abbrechen").clicked() {
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
