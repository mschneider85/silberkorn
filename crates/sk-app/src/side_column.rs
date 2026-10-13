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
        if grid || self.current.is_none() {
            return false;
        }
        match self.side_column {
            SideColumn::Off => false,
            SideColumn::Always => true,
            SideColumn::Auto => {
                let size = match (&self.texture, &self.placeholder) {
                    (Some(tex), _) => self.view_size.unwrap_or(tex.size_vec2()),
                    (None, Some(ph)) => ph.size_vec2(),
                    // Nothing to measure (image loading): keep the current state.
                    (None, None) => return self.side_column_shown,
                };
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
            let r = widgets::histogram(ui, self.histogram.as_ref());
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
            if ui.selectable_label(zoomed, "100 %").on_hover_text("Z").clicked() && !zoomed {
                self.toggle_zoom_100();
            }
            if let Some(pct) = self.zoom_pct {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("{pct:.0} %")).size(12.0).color(theme::TEXT_DIM));
                });
            }
        });
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
        section(ui, "Bearbeitung", None, |ui| {
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
        if let Some((tab, title)) = open.and_then(tool_of) {
            self.tab = tab;
            if let Some(title) = title {
                widgets::reveal_section(ui.ctx(), title);
            }
        }
    }

    /// Saved states of the image; a click applies one, "+" saves the current state.
    fn side_snapshots(&mut self, ui: &mut Ui) {
        let Some(cur) = &self.current else { return };
        let rows: Vec<(String, bool)> = cur.snapshots.iter().map(|s| (s.name.clone(), s.adjustments == cur.adjustments)).collect();
        let mut apply = None;
        let mut add = false;
        section(ui, "Schnappschüsse", None, |ui| {
            for (i, (name, current)) in rows.iter().enumerate() {
                if ui.selectable_label(*current, format!("{}  {name}", theme::icon::CAMERA)).on_hover_text("Anwenden").clicked() && !current {
                    apply = Some(i);
                }
            }
            if ui.button(format!("{}  Schnappschuss", theme::icon::PLUS)).on_hover_text("Den jetzigen Stand unter einem Namen sichern").clicked() {
                add = true;
            }
        });
        if add {
            self.add_snapshot(None);
        }
        if let Some(adj) = apply.and_then(|i| self.current.as_ref()?.snapshots.get(i)).map(|s| s.adjustments.clone()) {
            self.edit(|a| *a = adj);
            self.commit_and_save();
        }
    }

    /// The last edit steps; a click jumps back (like the history tab).
    fn side_history(&mut self, ui: &mut Ui) {
        const SHOWN: usize = 8;
        let Some((steps, now)) = self.history_steps() else { return };
        if steps.len() <= 1 {
            return;
        }
        let start = steps.len().saturating_sub(SHOWN);
        let rows: Vec<(usize, String)> =
            (start..steps.len()).rev().map(|j| (j, if j == 0 { "Geöffnet".to_string() } else { describe(steps[j - 1], steps[j]) })).collect();
        let mut jump = None;
        let mut all = false;
        section(ui, "Verlauf", None, |ui| {
            for (j, label) in rows {
                let text = if j > now { RichText::new(label).color(theme::TEXT_FAINT).italics() } else { RichText::new(label) };
                if ui.selectable_label(j == now, text).clicked() && j != now {
                    jump = Some(j);
                }
            }
            if start > 0 && ui.small_button("Alle Schritte …").clicked() {
                all = true;
            }
        });
        if let Some(j) = jump {
            self.jump_history(j);
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
        "Belichtung" | "Kontrast" | "Helligkeit" | "Sättigung" => (Tab::Exposure, Some("Belichtung")),
        "Lichter" | "Tiefen" | "Weiß" | "Schwarz" => (Tab::Exposure, Some("Lichter und Tiefen")),
        "Tonwerte" => (Tab::Exposure, Some("Tonwerte")),
        "Gradationskurve" => (Tab::Exposure, Some("Gradationskurve")),
        "Weißabgleich" => (Tab::Color, Some("Weißabgleich")),
        "Basiskurve" => (Tab::Color, Some("Basismerkmale")),
        "Dynamik" => (Tab::Color, Some("Farbe")),
        // These sections are named like the tool.
        "Farbeditor" | "Hauttöne" | "Farbbalance" | "Schwarzweiß" => (Tab::Color, Some(name)),
        "Zuschnitt" => (Tab::Lens, Some("Zuschnitt")),
        "Drehen" | "Geraderichten" | "Spiegeln" => (Tab::Lens, Some("Drehen & Spiegeln")),
        "Perspektive" => (Tab::Lens, Some("Perspektive")),
        "Objektivkorrektur" => (Tab::Lens, Some("Objektivkorrektur")),
        "Schärfen" | "Körnung" | "Dunst entfernen" => (Tab::Details, Some(name)),
        "Rauschreduzierung" | "KI-Entrauschen" => (Tab::Details, Some(crate::denoise::NOISE_SECTION)),
        "Klarheit" | "Struktur" => (Tab::Details, Some("Klarheit")),
        "Vignette" => (Tab::Details, Some("Vignettierung")),
        "Objektivunschärfe" => (Tab::Details, Some("Objektivunschärfe")),
        "Ebenen" => (Tab::Layers, None),
        "Retusche" => (Tab::Retouch, Some("Reparieren & Klonen")),
        "Entfernen" => (Tab::Retouch, Some("Entfernen (KI)")),
        _ => return None,
    };
    Some(t)
}

