//! Viewer, filmstrip and grid browser.

use std::path::PathBuf;

use egui::{Color32, Modifiers, Pos2, Rect, Sense, Ui, Vec2, pos2, vec2};

use crate::app::App;
use crate::widgets;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeedParam {
    Exposure,
    Contrast,
    Saturation,
    Highlights,
    Shadows,
    Temperature,
}

pub const SPEED_KEYS: [(egui::Key, SpeedParam); 6] = [
    (egui::Key::Q, SpeedParam::Exposure),
    (egui::Key::A, SpeedParam::Contrast),
    (egui::Key::D, SpeedParam::Saturation),
    (egui::Key::F, SpeedParam::Highlights),
    (egui::Key::V, SpeedParam::Shadows),
    (egui::Key::T, SpeedParam::Temperature),
];

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
        let tools_active = self.crop_tool.is_some() || self.straighten_tool.is_some() || self.local_tool.is_some();
        let margin = if self.crop_tool.is_some() { 40.0 } else { 20.0 };
        let fit = ((rect.width() - margin) / size.x).min((rect.height() - margin) / size.y).max(0.01);
        let scale = self.zoom.unwrap_or(fit);

        // Speed Edit: hold a key and scroll to change a value.
        let speed = SPEED_KEYS.iter().find(|(k, _)| ui.input(|i| i.key_down(*k))).map(|(_, p)| *p);
        if let Some(param) = speed
            && response.hovered()
        {
            // Discrete wheel notches from the raw events (not the smoothed scroll).
            let notches: f32 = ui.input(|i| {
                i.raw.events.iter().filter_map(|e| match e {
                    egui::Event::MouseWheel { delta, .. } if delta.y != 0.0 => Some(delta.y.signum()),
                    _ => None,
                }).sum()
            });
            if notches != 0.0 {
                self.speed_edit(param, notches);
                // egui smooths scrolling over several frames; ignore the remainder for zooming.
                self.zoom_blocked_until = ui.input(|i| i.time) + 0.5;
            }
            ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
        }

        // Mouse wheel zoom around the cursor.
        let zoom_blocked = ui.input(|i| i.time) < self.zoom_blocked_until;
        if response.hovered() && speed.is_none() && !zoom_blocked && self.crop_tool.is_none() && self.straighten_tool.is_none() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let new_scale = (scale * (1.0 + scroll * 0.0015)).clamp(fit.min(0.05), 16.0);
                if let Some(cursor) = response.hover_pos() {
                    let center = rect.center() + self.pan;
                    let rel = (cursor - center) / scale;
                    self.pan = cursor - rect.center() - rel * new_scale;
                }
                self.zoom = Some(new_scale);
            }
        }
        if (response.dragged() && !self.picking_wb && !tools_active && !self.split_dragging) || response.dragged_by(egui::PointerButton::Middle) {
            self.pan += response.drag_delta();
        }
        let scale = self.zoom.unwrap_or(fit);
        if self.zoom.is_none() {
            self.pan = Vec2::ZERO;
        }
        let img_rect = Rect::from_center_size(rect.center() + self.pan, size * scale);
        painter.image(tex_id, img_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);

        // Full-resolution tile when the preview would be magnified.
        self.update_tile(rect, img_rect, scale);
        if let Some(tile) = &self.tile
            && tile.version == self.edit_version
            && self.crop_tool.is_none()
            && self.straighten_tool.is_none()
        {
            let k = tile.full_size.0 as f32 / size.x;
            let r = tile.region;
            let tr = Rect::from_min_size(
                img_rect.min + vec2(r.x as f32, r.y as f32) / k * scale,
                vec2(r.w as f32, r.h as f32) / k * scale,
            );
            painter.image(tile.texture.id(), tr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }

        // Split before/after view.
        let mut split_drag = false;
        if self.split_view
            && let Some(before) = &self.before_texture
            && before.size_vec2() == size
        {
            let x = img_rect.left() + img_rect.width() * self.split_pos;
            let left = Rect::from_min_max(img_rect.min, pos2(x, img_rect.bottom()));
            painter.image(before.id(), left, Rect::from_min_max(pos2(0.0, 0.0), pos2(self.split_pos, 1.0)), Color32::WHITE);
            painter.line_segment([pos2(x, img_rect.top()), pos2(x, img_rect.bottom())], egui::Stroke::new(2.0, Color32::WHITE));
            painter.circle_filled(pos2(x, img_rect.center().y), 7.0, Color32::WHITE);
            for (text, align, dx) in [("Vorher", egui::Align2::RIGHT_TOP, -10.0), ("Nachher", egui::Align2::LEFT_TOP, 10.0)] {
                let galley = painter.layout_no_wrap(text.into(), egui::FontId::proportional(12.0), Color32::WHITE);
                let r = align.anchor_size(pos2(x + dx, img_rect.top() + 40.0), galley.size());
                painter.rect_filled(r.expand(4.0), 3.0, Color32::from_black_alpha(160));
                painter.galley(r.min, galley, Color32::WHITE);
            }
            let near = response.hover_pos().is_some_and(|p| (p.x - x).abs() < 10.0);
            if near || self.split_dragging {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            if response.drag_started() && near {
                self.split_dragging = true;
            }
            if self.split_dragging {
                split_drag = true;
                if let Some(p) = response.interact_pointer_pos() {
                    self.split_pos = ((p.x - img_rect.left()) / img_rect.width()).clamp(0.0, 1.0);
                }
                if response.drag_stopped() {
                    self.split_dragging = false;
                }
            }
        }

        if split_drag {
            // The divider takes the drag.
        } else if self.local_tool.is_some() {
            self.local_overlay(ui, &response, img_rect, scale);
        } else if self.crop_tool.is_some() {
            self.crop_overlay(ui, &response, img_rect);
        } else if self.straighten_tool.is_some() {
            self.straighten_overlay(ui, &response);
        } else if self.picking_wb {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            if response.clicked()
                && let Some(pos) = response.interact_pointer_pos()
                && img_rect.contains(pos)
            {
                self.pick_white_balance((pos - img_rect.min) / scale);
            }
        } else if response.double_clicked() {
            self.toggle_zoom_100();
        }

        // Overlay: file name, zoom, mode markers.
        if let Some(cur) = &self.current {
            let full_w = self.full_output_width().unwrap_or(size.x);
            // Zoom relative to the full sensor resolution.
            let pct = scale * size.x / full_w.max(1.0) * 100.0;
            let mode = if let Some(msg) = &self.speed_message {
                msg.as_str()
            } else if self.crop_tool.is_some() {
                "   ZUSCHNEIDEN – Enter: übernehmen"
            } else if self.straighten_tool.is_some() {
                "   GERADERICHTEN – Linie ziehen"
            } else if self.show_before {
                "   VORHER"
            } else {
                ""
            };
            let text = format!("{}   {pct:.0} %{mode}", cur.meta.path.file_name().unwrap_or_default().to_string_lossy());
            let galley = painter.layout_no_wrap(text, egui::FontId::proportional(13.0), Color32::from_gray(220));
            let pos = rect.left_top() + vec2(10.0, 8.0);
            painter.rect_filled(Rect::from_min_size(pos, galley.size()).expand(4.0), 3.0, Color32::from_black_alpha(160));
            painter.galley(pos, galley, Color32::from_gray(220));
        }
        if self.loading.is_some() {
            painter.text(rect.right_top() + vec2(-10.0, 8.0), egui::Align2::RIGHT_TOP, "Lade…", egui::FontId::proportional(13.0), Color32::from_gray(200));
        }
    }

    /// Changes a value by one step: on the active layer, or on the background.
    pub fn speed_edit(&mut self, param: SpeedParam, dir: f32) {
        let layer = self.active_layer;
        let Some(cur) = self.current.as_mut() else { return };
        let as_shot = cur.as_shot_temp_tint;
        let msg;
        let adj = &mut cur.adjustments;
        if let Some(l) = layer.and_then(|i| adj.layers.get_mut(i)) {
            let a = &mut l.adjustments;
            let (name, v, step, lo, hi) = match param {
                SpeedParam::Exposure => ("Belichtung", &mut a.exposure, 0.05, -4.0, 4.0),
                SpeedParam::Contrast => ("Kontrast", &mut a.contrast, 2.0, -100.0, 100.0),
                SpeedParam::Saturation => ("Sättigung", &mut a.saturation, 2.0, -100.0, 100.0),
                SpeedParam::Highlights => ("Lichter", &mut a.highlights, 2.0, -100.0, 100.0),
                SpeedParam::Shadows => ("Tiefen", &mut a.shadows, 2.0, -100.0, 100.0),
                SpeedParam::Temperature => ("Temperatur", &mut a.temperature, 2.0, -100.0, 100.0),
            };
            *v = (*v + step * dir).clamp(lo, hi);
            msg = format!("   SPEED EDIT ({}) {name}: {:+.2}", l.name, *v);
        } else {
            if param == SpeedParam::Temperature && adj.white_balance.mode == sk_core::params::WbMode::AsShot {
                adj.white_balance = sk_core::params::WhiteBalance { mode: sk_core::params::WbMode::Custom, kelvin: as_shot.0, tint: as_shot.1 };
            }
            let (name, v, step, lo, hi) = match param {
                SpeedParam::Exposure => ("Belichtung", &mut adj.exposure, 0.05, -4.0, 4.0),
                SpeedParam::Contrast => ("Kontrast", &mut adj.contrast, 2.0, -100.0, 100.0),
                SpeedParam::Saturation => ("Sättigung", &mut adj.saturation, 2.0, -100.0, 100.0),
                SpeedParam::Highlights => ("Lichter", &mut adj.highlights, 2.0, -100.0, 100.0),
                SpeedParam::Shadows => ("Tiefen", &mut adj.shadows, 2.0, -100.0, 100.0),
                SpeedParam::Temperature => ("Kelvin", &mut adj.white_balance.kelvin, 50.0, 2000.0, 15000.0),
            };
            *v = (*v + step * dir).clamp(lo, hi);
            msg = format!("   SPEED EDIT {name}: {:+.2}", *v);
        }
        self.speed_message = Some(msg);
        self.request_render();
    }

    /// Width of the output at full sensor resolution (estimated from the sensor size until loaded).
    fn full_output_width(&self) -> Option<f32> {
        let cur = self.current.as_ref()?;
        let opts = sk_core::pipeline::RenderOptions { ignore_crop: self.crop_tool.is_some(), ..Default::default() };
        let p = sk_core::pipeline::Processor::with_options(&cur.meta, &cur.adjustments, opts);
        let (w, h) = match &cur.full {
            Some(f) => (f.width, f.height),
            None => {
                let k = cur.meta.info.width.max(cur.meta.info.height) as f32 / cur.preview.width.max(cur.preview.height) as f32;
                ((cur.preview.width as f32 * k) as usize, (cur.preview.height as f32 * k) as usize)
            }
        };
        Some(p.output_size(w, h).0 as f32)
    }

    /// Zoom to 100 % (one full-resolution pixel per screen pixel) or back to fit.
    pub fn toggle_zoom_100(&mut self) {
        if self.zoom.is_some() {
            self.zoom = None;
        } else if let (Some(full_w), Some(tex)) = (self.full_output_width(), &self.texture) {
            self.zoom = Some(full_w / tex.size_vec2().x);
        }
        self.pan = Vec2::ZERO;
    }

    /// Requests a full-resolution tile for the visible area when zoomed in.
    pub(crate) fn update_tile(&mut self, viewport: Rect, img_rect: Rect, scale: f32) {
        let Some(cur) = &self.current else { return };
        let Some(full) = &cur.full else { return };
        if scale <= 1.05 || self.crop_tool.is_some() || self.straighten_tool.is_some() {
            return;
        }
        let Some(tex) = &self.texture else { return };
        let p = sk_core::pipeline::Processor::new(&cur.meta, &cur.adjustments);
        let (fw, fh) = p.output_size(full.width, full.height);
        let k = fw as f32 / tex.size_vec2().x;
        let vis = viewport.intersect(img_rect);
        if vis.width() <= 0.0 || vis.height() <= 0.0 {
            return;
        }
        let to_full = |pt: Pos2| ((pt - img_rect.min) / scale) * k;
        let (a, b) = (to_full(vis.min), to_full(vis.max));
        let pad = 32.0;
        let x0 = (a.x - pad).max(0.0) as usize;
        let y0 = (a.y - pad).max(0.0) as usize;
        let x1 = ((b.x + pad) as usize).min(fw);
        let y1 = ((b.y + pad) as usize).min(fh);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let region = sk_core::geometry::Region { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
        // Reuse the current tile if it already covers the visible area.
        let covers = |r: &sk_core::geometry::Region| {
            r.x <= a.x.max(0.0) as usize && r.y <= a.y.max(0.0) as usize && r.x + r.w >= (b.x as usize).min(fw) && r.y + r.h >= (b.y as usize).min(fh)
        };
        if let Some((v, r)) = &self.tile_requested
            && *v == self.edit_version
            && covers(r)
        {
            return;
        }
        // Don't render tiles while a slider is being dragged; the preview updates first.
        if self.pointer_down {
            return;
        }
        self.tile_requested = Some((self.edit_version, region));
        self.workers.tile(crate::workers::TileJob {
            generation: self.generation(),
            version: self.edit_version,
            meta: cur.meta.clone(),
            full: full.clone(),
            adjustments: cur.adjustments.clone(),
            region,
            clip_warning: self.clip_warning,
            before: self.show_before,
        });
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
