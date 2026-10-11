//! Viewer, filmstrip and grid browser.

use std::path::PathBuf;

use egui::{Color32, Modifiers, Pos2, Rect, Sense, Ui, Vec2, pos2, vec2};

use crate::app::App;
use crate::widgets;

impl App {
    pub fn viewer(&mut self, ui: &mut Ui) {
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_gray(38));

        let Some(tex) = &self.texture else {
            let msg = if let Some(e) = &self.load_error {
                format!("Fehler: {e}")
            } else if self.loading.is_some() {
                "Lade…".into()
            } else if self.entries.is_empty() {
                "Keine Bilder.\nIm Tab „Bibliothek“ einen Ordner öffnen oder importieren.".into()
            } else {
                String::new()
            };
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, msg, egui::FontId::proportional(16.0), Color32::from_gray(170));
            return;
        };
        let size = tex.size_vec2();
        let tex_id = tex.id();
        let fit = ((rect.width() - 20.0) / size.x).min((rect.height() - 20.0) / size.y).max(0.01);
        let scale = self.zoom.unwrap_or(fit);

        // Mouse wheel zoom around the cursor.
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let new_scale = (scale * (1.0 + scroll * 0.0015)).clamp(fit.min(0.05), 8.0);
                if let Some(cursor) = response.hover_pos() {
                    let center = rect.center() + self.pan;
                    let rel = (cursor - center) / scale;
                    self.pan = cursor - rect.center() - rel * new_scale;
                }
                self.zoom = Some(new_scale);
            }
        }
        if response.dragged() && !self.picking_wb {
            self.pan += response.drag_delta();
        }
        let scale = self.zoom.unwrap_or(fit);
        if self.zoom.is_none() {
            self.pan = Vec2::ZERO;
        }
        let img_rect = Rect::from_center_size(rect.center() + self.pan, size * scale);
        painter.image(tex_id, img_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);

        if self.picking_wb {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            if response.clicked()
                && let Some(pos) = response.interact_pointer_pos()
                && img_rect.contains(pos)
            {
                self.pick_white_balance((pos - img_rect.min) / scale);
            }
        } else if response.double_clicked() {
            self.zoom = if self.zoom.is_some() { None } else { Some(1.0) };
            self.pan = Vec2::ZERO;
        }

        // Overlay: file name, zoom, before marker.
        if let Some(cur) = &self.current {
            let full_w = if (cur.adjustments.rotation % 2) == 1 { cur.meta.info.height } else { cur.meta.info.width } as f32;
            // Zoom relative to the full sensor resolution.
            let pct = scale * size.x / full_w.max(1.0) * 100.0;
            let text = format!(
                "{}   {pct:.0} %{}",
                cur.meta.path.file_name().unwrap_or_default().to_string_lossy(),
                if self.show_before { "   VORHER" } else { "" }
            );
            let galley = painter.layout_no_wrap(text, egui::FontId::proportional(13.0), Color32::from_gray(220));
            let pos = rect.left_top() + vec2(10.0, 8.0);
            painter.rect_filled(Rect::from_min_size(pos, galley.size()).expand(4.0), 3.0, Color32::from_black_alpha(160));
            painter.galley(pos, galley, Color32::from_gray(220));
        }
        if self.loading.is_some() {
            painter.text(rect.right_top() + vec2(-10.0, 8.0), egui::Align2::RIGHT_TOP, "Lade…", egui::FontId::proportional(13.0), Color32::from_gray(200));
        }
    }

    /// Paints one browser cell; returns the click modifiers if clicked, and whether it was double-clicked.
    fn thumb_cell(&mut self, ui: &mut Ui, idx: usize, cell: Vec2, primary: Option<&PathBuf>) -> (Option<Modifiers>, bool) {
        let (rect, resp) = ui.allocate_exact_size(cell, Sense::click());
        if !ui.is_rect_visible(rect) {
            return (None, false);
        }
        let entry = &self.entries[idx];
        if !self.thumb_requested.contains(&entry.path) {
            self.thumb_requested.insert(entry.path.clone());
            self.workers.thumbnail(entry.path.clone());
        }
        let selected = self.selection.contains(&entry.path);
        let painter = ui.painter_at(rect);
        let bg = if Some(&entry.path) == primary {
            Color32::from_gray(85)
        } else if selected {
            Color32::from_gray(62)
        } else if resp.hovered() {
            Color32::from_gray(50)
        } else {
            Color32::from_gray(32)
        };
        painter.rect_filled(rect, 3.0, bg);
        let img_area = Rect::from_min_max(rect.min + vec2(5.0, 5.0), rect.max - vec2(5.0, 20.0));
        if let Some(t) = self.thumbs.get(&entry.path) {
            let s = t.size_vec2();
            let k = (img_area.width() / s.x).min(img_area.height() / s.y);
            let r = Rect::from_center_size(img_area.center(), s * k);
            painter.image(t.id(), r, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
        }
        painter.text(
            pos2(rect.center().x, rect.bottom() - 10.0),
            egui::Align2::CENTER_CENTER,
            &entry.name,
            egui::FontId::proportional(11.0),
            Color32::from_gray(190),
        );
        widgets::paint_badges(ui, img_area, entry.rating, entry.color_label.rgb());
        let tooltip = [
            Some(entry.name.clone()),
            entry.capture_date.clone(),
            entry.camera.clone(),
            entry.lens.clone(),
            (!entry.keywords.is_empty()).then(|| entry.keywords.join(", ")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n");
        if Some(&entry.path) == primary && self.scroll_to_primary {
            resp.scroll_to_me(Some(egui::Align::Center));
        }
        let resp = resp.on_hover_text_at_pointer(tooltip);
        let mods = resp.clicked().then(|| ui.input(|i| i.modifiers));
        (mods, resp.double_clicked())
    }

    pub fn filmstrip(&mut self, ui: &mut Ui) {
        let visible = self.visible.clone();
        let mut clicked: Option<(PathBuf, Modifiers)> = None;
        let primary = self.primary_path();
        let cell = vec2(150.0, ui.available_height().clamp(90.0, 260.0) - 8.0);
        egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, |ui| {
            ui.horizontal(|ui| {
                for &i in &visible {
                    if let (Some(m), _) = self.thumb_cell(ui, i, cell, primary.as_ref()) {
                        clicked = Some((self.entries[i].path.clone(), m));
                    }
                }
            });
        });
        self.scroll_to_primary = false;
        if let Some((p, m)) = clicked {
            self.select_path(&p, m);
        }
    }

    pub fn grid(&mut self, ui: &mut Ui) {
        let rect = ui.available_rect_before_wrap();
        ui.painter().rect_filled(rect, 0.0, Color32::from_gray(28));
        let visible = self.visible.clone();
        let primary = self.primary_path();
        let mut clicked: Option<(PathBuf, Modifiers)> = None;
        let mut open: Option<PathBuf> = None;
        let cell = vec2(200.0, 180.0);
        let cols = ((rect.width() - 16.0) / (cell.x + 6.0)).floor().max(1.0) as usize;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(8.0);
            for row in visible.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    for &i in row {
                        let (m, double) = self.thumb_cell(ui, i, cell, primary.as_ref());
                        if let Some(m) = m {
                            clicked = Some((self.entries[i].path.clone(), m));
                        }
                        if double {
                            open = Some(self.entries[i].path.clone());
                        }
                    }
                });
            }
            if visible.is_empty() {
                ui.centered_and_justified(|ui| ui.label("Keine Bilder in dieser Ansicht."));
            }
        });
        self.scroll_to_primary = false;
        if let Some((p, m)) = clicked {
            self.select_path(&p, m);
        }
        if let Some(p) = open {
            self.select_path(&p, Modifiers::NONE);
            self.grid_view = false;
        }
    }
}
