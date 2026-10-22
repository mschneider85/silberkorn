//! Right column next to the image: navigator, histogram, image info, an overview of the edits,
//! snapshots and the last edit steps.
//! By default it only appears when the image leaves room at the side (portrait images, wide
//! windows), so it never makes the image smaller.

use egui::{Color32, Pos2, Rect, RichText, Sense, Ui, Vec2, pos2, vec2};
use sk_core::history::{changes, describe, number};
use sk_core::params::Adjustments;
use serde::{Deserialize, Serialize};

use crate::app::{App, Tab};
use crate::theme;
use crate::widgets::{self, section};

pub const WIDTH: f32 = 260.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SideColumn {
    /// Only when the image would not get smaller.
    #[default]
    Auto,
    Always,
    Off,
}

impl SideColumn {
    pub const ALL: [SideColumn; 3] = [SideColumn::Auto, SideColumn::Always, SideColumn::Off];

    pub fn label(self) -> &'static str {
        match self {
            SideColumn::Auto => "Seitenleiste: wenn Platz ist",
            SideColumn::Always => "Seitenleiste: immer",
            SideColumn::Off => "Seitenleiste: aus",
        }
    }
}

impl App {
    /// Whether the column should be shown this frame (`grid`: the grid fills the center).
    pub(crate) fn side_column_wanted(&self, grid: bool) -> bool {
        // While the next image loads there is no current one for a moment: the column stays.
        if grid || (self.current.is_none() && self.loading.is_none()) {
            return false;
        }
        match self.side_column {
            SideColumn::Off => false,
            SideColumn::Always => true,
            SideColumn::Auto => {
                // Several images side by side fill the viewer whatever their shape: keep the
                // current state, else it comes and goes while stepping through them.
                if !self.compare_cells().is_empty() {
                    return self.side_column_shown;
                }
                // While the next image loads, the texture is still the previous one's.
                let loading = self.loading.is_some();
                let size = match (&self.texture, &self.placeholder) {
                    (_, Some(ph)) if loading => ph.size_vec2(),
                    (Some(_), _) if loading => return self.side_column_shown,
                    (Some(tex), _) => self.view_size.unwrap_or(tex.size_vec2()),
                    (None, Some(ph)) => ph.size_vec2(),
                    // Nothing to measure (image loading): keep the current state.
                    (None, None) => return self.side_column_shown,
                };
                // With the print preview, the paper around the image counts.
                let size = size * self.paper_frame(size).map_or(Vec2::splat(1.0), |p| p.rel.max(Vec2::splat(1.0)));
                let Some(r) = self.viewer_rect else { return false };
                // Viewer width without the column; last frame's rect may already exclude it.
                let w = r.width() + if self.side_column_shown { WIDTH } else { 0.0 };
                let fit = |w: f32| ((w - 20.0) / size.x).min((r.height() - 20.0) / size.y);
                fit(w - WIDTH) >= fit(w) * 0.98
            }
        }
    }

    pub(crate) fn side_column(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(10.0);
            self.side_navigator(ui);
            ui.add_space(8.0);
            let r = self.histogram_widget(ui);
            if widgets::clip_triangles(ui, r, self.histogram.as_ref(), self.clip_warning) {
                self.toggle_clip_warning();
            }
            ui.add_space(8.0);
            self.side_info(ui);
            self.side_overview(ui);
            // The history tab shows snapshots and steps itself.
            if self.tab != Tab::History {
                self.side_snapshots(ui);
                self.side_history(ui);
            }
        });
    }

    /// Overview of the whole image; while zoomed in, a frame marks the visible part.
    /// Click: 100 % at that spot (or move there when zoomed) · drag: move the view.
    fn side_navigator(&mut self, ui: &mut Ui) {
        let Some(tex) = &self.texture else { return };
        let tex_id = tex.id();
        let img_size = self.view_size.unwrap_or(tex.size_vec2());
        let w = ui.available_width();
        let h = (w / img_size.x * img_size.y).min(200.0);
        let (area, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        let nav = Rect::from_center_size(area.center(), vec2(h * img_size.x / img_size.y, h));
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 4.0, theme::BROWSER);
        painter.image(tex_id, nav, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        let zoomed = self.zoom.is_some();
        let geometry = self.view_geometry.filter(|_| zoomed);
        if let Some((viewport, img_rect)) = geometry {
            // The visible part, everything else dimmed.
            let to_nav = |p: Pos2| nav.min + (p - img_rect.min) / img_rect.size() * nav.size();
            let vis = viewport.intersect(img_rect);
            let frame = Rect::from_min_max(to_nav(vis.min), to_nav(vis.max)).intersect(nav);
            let dim = Color32::from_black_alpha(130);
            for r in [
                Rect::from_min_max(nav.min, pos2(nav.max.x, frame.min.y)),
                Rect::from_min_max(pos2(nav.min.x, frame.max.y), nav.max),
                Rect::from_min_max(pos2(nav.min.x, frame.min.y), pos2(frame.min.x, frame.max.y)),
                Rect::from_min_max(pos2(frame.max.x, frame.min.y), pos2(nav.max.x, frame.max.y)),
            ] {
                if r.is_positive() {
                    painter.rect_filled(r, 0.0, dim);
                }
            }
            painter.rect_stroke(frame, 0.0, egui::Stroke::new(1.5, Color32::WHITE), egui::StrokeKind::Inside);
        }
        let resp = ui.interact(nav, ui.id().with("side_navigator"), Sense::click_and_drag());
        if resp.hovered() || resp.dragged() {
            let icon = if !zoomed {
                egui::CursorIcon::ZoomIn
            } else if resp.dragged() {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            };
            ui.ctx().set_cursor_icon(icon);
        }
        let hover = if zoomed { "Klicken oder ziehen: Ausschnitt verschieben" } else { "Klick: diese Stelle in 100 % ansehen" };
        let resp = resp.on_hover_text(hover);
        if let Some(p) = resp.interact_pointer_pos().filter(|_| resp.clicked() || resp.dragged()) {
            let f = ((p - nav.min) / nav.size()).clamp(Vec2::ZERO, vec2(1.0, 1.0));
            match geometry {
                Some((viewport, img_rect)) => self.pan += viewport.center() - (img_rect.min + f * img_rect.size()),
                None if resp.clicked() => {
                    self.toggle_zoom_100();
                    if let Some(z) = self.zoom {
                        self.pan = (vec2(0.5, 0.5) - f) * img_size * z;
                    }
                }
                None => {}
            }
            ui.ctx().request_repaint();
        }
        // Zoom switch below the overview.
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.selectable_label(!zoomed, "Einpassen").clicked() && zoomed {
                self.zoom_fit();
            }
            // Only at 100 % on (at another zoom neither of the two).
            let at_100 = self.at_zoom_100();
            if ui.selectable_label(at_100, "100 %").on_hover_text(crate::commands::hint("100 %", "zoom-100")).clicked() && !at_100 {
                self.zoom_to_100();
            }
        });
        self.zoom_control(ui);
    }

    /// Everything that differs from an unedited image; a click opens the tool.
    fn side_overview(&mut self, ui: &mut Ui) {
        let Some(cur) = &self.current else { return };
        let adj = &cur.adjustments;
        let rows: Vec<(&'static str, String)> = changes(&Adjustments::default(), adj)
            .into_iter()
            .map(|(name, v)| {
                let value = match name {
                    "Ebenen" => adj.layers.len().to_string(),
                    "Retusche" => adj.retouch.len().to_string(),
                    "Entfernen" => adj.removals.len().to_string(),
                    _ => v.map(|v| if v > 0.0 { format!("+{}", number(v)) } else { number(v) }).unwrap_or_default(),
                };
                (name, value)
            })
            .collect();
        let mut open = None;
        widgets::section_counted(ui, "Bearbeitung", None, rows.len(), |ui| {
            if rows.is_empty() {
                widgets::hint(ui, "Unbearbeitet");
            }
            ui.spacing_mut().item_spacing.y = 0.0;
            let w = ui.available_width();
            for (name, value) in &rows {
                let (rect, resp) = ui.allocate_exact_size(vec2(w, 22.0), Sense::click());
                let p = ui.painter();
                if resp.hovered() {
                    p.rect_filled(rect, 4.0, Color32::from_rgb(46, 46, 51));
                }
                let color = if resp.hovered() { Color32::WHITE } else { theme::TEXT };
                p.text(rect.left_center() + vec2(6.0, 0.0), egui::Align2::LEFT_CENTER, *name, egui::FontId::proportional(12.5), color);
                p.text(rect.right_center() - vec2(6.0, 0.0), egui::Align2::RIGHT_CENTER, value, egui::FontId::proportional(12.0), theme::TEXT_DIM);
                if resp.on_hover_text("Zum Werkzeug").clicked() {
                    open = Some(*name);
                }
            }
        });
        if let Some((tab, title)) = open.and_then(tool_of)
            && self.section_available(tab, title.unwrap_or_default())
            && self.open_tab(tab)
            && let Some(title) = title
        {
            widgets::reveal_section(ui.ctx(), title);
        }
    }

    /// Saved states of the image: the same list as in the history tab.
    fn side_snapshots(&mut self, ui: &mut Ui) {
        if self.current.is_some() {
            self.snapshot_section(ui, true);
        }
    }

    /// The last edit steps; a click jumps back (like the history tab).
    fn side_history(&mut self, ui: &mut Ui) {
        const SHOWN: usize = 8;
        let Some((steps, now)) = self.history_steps() else { return };
        let start = steps.len().saturating_sub(SHOWN);
        let rows: Vec<(usize, String)> =
            (start..steps.len()).rev().map(|j| (j, if j == 0 { "Geöffnet".to_string() } else { describe(steps[j - 1], steps[j]) })).collect();
        let (mut jump, mut hovered) = (None, None);
        let mut all = false;
        widgets::section_counted(ui, "Verlauf", None, steps.len() - 1, |ui| {
            for (j, label) in rows {
                let text = if j > now { RichText::new(label).color(theme::TEXT_FAINT).italics() } else { RichText::new(label) };
                let r = ui.selectable_label(j == now, text);
                if r.clicked() && j != now {
                    jump = Some(j);
                } else if r.hovered() && j != now {
                    hovered = Some(j);
                }
            }
            if start > 0 && ui.small_button("Alle Schritte …").clicked() {
                all = true;
            }
        });
        if let Some(j) = jump {
            self.jump_history(j);
        } else if let Some(j) = hovered {
            self.preview_history_step(j);
        }
        if all {
            self.tab = Tab::History;
        }
    }

    fn side_info(&mut self, ui: &mut Ui) {
        let Some(cur) = &self.current else { return };
        let info = &cur.meta.info;
        let name = sk_core::variant::display_name(&cur.meta.path);
        let camera = format!("{} {}", info.make, info.model).trim().to_string();
        let exposure: Vec<String> = [info.iso.map(|i| format!("ISO {i}")), info.exposure_time.clone(), info.aperture.clone(), info.focal_length.clone()]
            .into_iter()
            .flatten()
            .collect();
        let size = format!("{} × {}", info.width, info.height);
        let (mut rating, label) = (cur.rating, cur.color_label);
        let mut new_rating = None;
        let mut new_label = None;
        let keywords = self.tab != Tab::Metadata;
        section(ui, "Bild", None, |ui| {
            ui.add(egui::Label::new(RichText::new(name).strong().color(theme::TEXT)).truncate());
            for line in [camera, exposure.join("  ·  "), size] {
                if !line.is_empty() {
                    ui.add(egui::Label::new(RichText::new(line).size(12.0).color(theme::TEXT_DIM)).truncate());
                }
            }
            ui.add_space(4.0);
            if widgets::stars(ui, &mut rating, 18.0) {
                new_rating = Some(rating);
            }
            ui.horizontal_wrapped(|ui| {
                for l in sk_core::sidecar::ColorLabel::ALL {
                    if widgets::color_label_dot(ui, l, label == l).clicked() {
                        new_label = Some(l);
                    }
                }
            });
            // The info tab has its own keyword field (same input buffer).
            if keywords {
                ui.add_space(4.0);
                self.keywords_ui(ui);
            }
        });
        if let Some(r) = new_rating {
            self.set_rating(r);
        }
        if let Some(l) = new_label {
            self.set_color_label(l);
        }
    }
}

/// Tab and section of a tool named by [`changes`].
fn tool_of(name: &'static str) -> Option<(Tab, Option<&'static str>)> {
    let t = match name {
        "Belichtung" | "Kontrast" | "Helligkeit" => (Tab::Exposure, Some("Belichtung")),
        "Lichter" | "Tiefen" | "Weiß" | "Schwarz" => (Tab::Exposure, Some("Lichter & Tiefen")),
        "Tonwerte" => (Tab::Exposure, Some("Tonwerte")),
        "Gradationskurve" => (Tab::Exposure, Some("Gradationskurve")),
        "Weißabgleich" => (Tab::Color, Some("Weißabgleich")),
        "Basismerkmale" => (Tab::Color, Some(name)),
        "Sättigung" | "Dynamik" => (Tab::Color, Some("Farbe")),
        // These sections are named like the tool.
        "Farbeditor" | "Hauttöne" | "Farbbalance" | "Schwarzweiß" => (Tab::Color, Some(name)),
        "Zuschnitt" => (Tab::Lens, Some("Zuschnitt")),
        "Drehen" | "Geraderichten" | "Spiegeln" => (Tab::Lens, Some("Drehen & Spiegeln")),
        "Trapezkorrektur" => (Tab::Lens, Some("Trapezkorrektur")),
        "Objektivkorrektur" => (Tab::Lens, Some("Objektivkorrektur")),
        "Neu ausleuchten" => (Tab::Exposure, Some("Neu ausleuchten")),
        "Schärfen" | "Dunst entfernen" => (Tab::Details, Some(name)),
        "Raw-Details" | "KI-Entrauschen" => (Tab::Details, Some(crate::ai_products::ENHANCE_SECTION)),
        "Körnung" | "Leuchten" | "Atmosphäre" => (Tab::Effects, Some(name)),
        "Rauschreduzierung" => (Tab::Details, Some(crate::noise::NOISE_SECTION)),
        "Klarheit" | "Struktur" => (Tab::Details, Some("Klarheit")),
        "Vignette" => (Tab::Effects, Some("Vignette")),
        "Objektivunschärfe" => (Tab::Effects, Some("Objektivunschärfe")),
        "Ebenen" => (Tab::Layers, None),
        "Retusche" | "Entfernen" => (Tab::Retouch, Some(crate::layers::RETOUCH_SECTION)),
        _ => return None,
    };
    Some(t)
}

