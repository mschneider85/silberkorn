//! Viewer, filmstrip and grid browser.

use std::path::PathBuf;

use egui::{Color32, Modifiers, Pos2, Rect, Sense, Ui, Vec2, pos2, vec2};

use crate::app::App;
use crate::workers::TILE;
use crate::{theme, widgets};

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

/// "850 KB", "3,4 MB"
fn file_size(bytes: usize) -> String {
    if bytes < 1024 * 1024 {
        format!("{} KB", bytes.div_ceil(1024))
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0)).replace('.', ",")
    }
}

/// Time a draft rendering may take while a control is dragged; with the measured speed it gives
/// the pixel budget of drafts.
const DRAFT_MS: f32 = 40.0;
/// Pixel budget of a view draft while dragging a control, until the speed is measured.
const DRAFT_PIXELS: usize = 700_000;

impl App {
    pub fn viewer(&mut self, ui: &mut Ui) {
        let rect = ui.available_rect_before_wrap();
        self.viewer_rect = Some(rect);
        self.viewer_content(ui);
        // What the user waits for here (mask, removal, matching).
        self.activity_overlay(ui, rect);
    }

    fn viewer_content(&mut self, ui: &mut Ui) {
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        // Several selected images and/or the reference side by side.
        let cells = self.compare_cells();
        // The print preview judges the image against the paper, as it will be seen.
        let bg = match self.print_background().filter(|_| cells.is_empty()) {
            Some(c) => c,
            None if self.fullscreen => Color32::from_gray(10),
            None => theme::VIEWER,
        };
        painter.rect_filled(rect, 0.0, bg);
        if !cells.is_empty() {
            self.compare_view(ui, rect, &cells);
            return;
        }

        // While the selected image loads: its thumbnail, cached preview or camera JPEG, no tools.
        if (self.texture.is_none() || self.loading.is_some())
            && let Some(ph) = &self.placeholder
        {
            let size = ph.size_vec2();
            let fit = ((rect.width() - 20.0) / size.x).min((rect.height() - 20.0) / size.y).max(0.01);
            let r = Rect::from_center_size(rect.center(), size * fit);
            painter.image(ph.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            return;
        }
        let Some(tex) = &self.texture else {
            let msg = if let Some(e) = &self.load_error {
                format!("Fehler: {e}")
            } else if self.loading.is_some() {
                "Lade…".into()
            } else if self.entries.is_empty() {
                self.welcome(ui, rect);
                return;
            } else {
                String::new()
            };
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, msg, egui::FontId::proportional(16.0), Color32::from_gray(170));
            return;
        };
        // Before | after side by side: the single view runs in the right half, the left one
        // shows the before rendering at the same zoom and position.
        let side = (self.split_view && self.split_side).then(|| {
            let mid = rect.center().x;
            (Rect::from_min_max(rect.min, pos2(mid - 2.0, rect.bottom())), Rect::from_min_max(pos2(mid + 2.0, rect.top()), rect.max))
        });
        let (rect, painter) = match side {
            Some((_, right)) => (right, ui.painter_at(right)),
            None => (rect, painter),
        };
        // Drafts are smaller textures drawn at the size of the full rendering.
        let size = self.view_size.unwrap_or(tex.size_vec2());
        let tex_id = tex.id();
        let tools_active = self.crop_tool.is_some() || self.straighten_tool.is_some() || self.local_tool.is_some();
        let margin = if self.crop_tool.is_some() {
            40.0
        } else if self.fullscreen {
            0.0
        } else {
            20.0
        };
        // Fitted, the paper around the image stays in view too.
        let paper = self.paper_frame(size);
        let extent = size * paper.as_ref().map_or(Vec2::splat(1.0), |p| p.rel.max(Vec2::splat(1.0)));
        let fit = ((rect.width() - margin) / extent.x).min((rect.height() - margin) / extent.y).max(0.01);
        if self.print.zoom {
            match self.print_size_scale(ui.ctx(), size) {
                Some(s) => self.zoom = Some(s),
                None => self.print.zoom = false,
            }
        }
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
        if response.hovered() && speed.is_none() && !zoom_blocked {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let new_scale = (scale * (1.0 + scroll * 0.0015)).clamp(fit.min(0.05), 16.0);
                if let Some(cursor) = response.hover_pos() {
                    // Over the before half: the same spot of the image.
                    let cursor = side.filter(|(l, _)| l.contains(cursor)).map_or(cursor, |(l, r)| cursor + (r.center() - l.center()));
                    let center = rect.center() + self.pan;
                    let rel = (cursor - center) / scale;
                    self.pan = cursor - rect.center() - rel * new_scale;
                }
                self.zoom = Some(new_scale);
                self.print.zoom = false;
            }
        }
        // Holding the space bar: dragging moves the image, whatever tool is active.
        let space_pan = ui.input(|i| i.key_down(egui::Key::Space)) && !ui.ctx().egui_wants_keyboard_input();
        if space_pan && (response.hovered() || response.dragged()) {
            ui.ctx().set_cursor_icon(if response.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
        }
        if (response.dragged() && (space_pan || (!self.picking_wb && !self.picking_skin && !tools_active && !self.split_dragging)))
            || response.dragged_by(egui::PointerButton::Middle)
        {
            self.pan += response.drag_delta();
        }
        let scale = self.zoom.unwrap_or(fit);
        if self.zoom.is_none() {
            self.pan = Vec2::ZERO;
        }
        // On whole physical pixels, so that 100 % maps each image pixel to exactly one screen pixel.
        let ppp = self.pixels_per_point;
        let img_rect = Rect::from_center_size(rect.center() + self.pan, size * scale);
        let img_rect = img_rect.translate((img_rect.min.to_vec2() * ppp).round() / ppp - img_rect.min.to_vec2());
        self.view_geometry = Some((rect, img_rect));
        // Proof view: the exported image instead of the preview, once it is ready.
        let proof_key = self.proof_key();
        let proof = self.proof.as_ref().filter(|p| self.proof_view && p.version == self.edit_version && Some(p.key) == proof_key);
        let shown = proof.map_or(tex_id, |p| p.texture.id());
        let paper_rect = paper.as_ref().map(|p| Rect::from_center_size(img_rect.center(), img_rect.size() * p.rel));
        if let (Some(p), Some(r)) = (&paper, paper_rect) {
            painter.rect_filled(r, 0.0, p.color);
            // On a background in the paper's color, a fine line shows the format.
            if p.color == bg {
                painter.rect_stroke(r, 0.0, egui::Stroke::new(1.0, Color32::from_gray(185)), egui::StrokeKind::Outside);
            }
        }
        painter.image(shown, img_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        let proof_ready = proof.is_some();

        // Full-resolution tiles where the preview would be magnified on screen.
        let magnified = scale * ppp > 1.0;
        self.view_magnified = magnified
            && self.crop_tool.is_none()
            && self.straighten_tool.is_none()
            && self.current.as_ref().is_some_and(|c| c.full.is_some());
        if magnified {
            self.want_full_now();
        }
        self.update_tiles(rect, img_rect, scale, magnified);
        let full_uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        self.ensure_focus_of_current();
        let preview_mask = self.current_path().filter(|_| !proof_ready).and_then(|path| self.focus_overlay(ui.ctx(), &path, size.x / size.y));
        if let Some(mask) = preview_mask {
            painter.image(mask, img_rect, full_uv, Color32::WHITE);
        }
        if magnified && !proof_ready && self.tiles.full_size.0 > 0 && self.crop_tool.is_none() && self.straighten_tool.is_none() {
            let k = self.tiles.full_size.0 as f32 / size.x;
            let (fw, fh) = (self.tiles.full_size.0 as f32, self.tiles.full_size.1 as f32);
            // `None`: the view region.
            let grid = self.tiles.textures.iter().filter(|_| self.tiles.version == self.edit_version).map(|(&(tx, ty), t)| {
                let [w, h] = t.size();
                (sk_core::geometry::Region { x: tx * TILE, y: ty * TILE, w, h }, t.id(), Some((tx, ty)))
            });
            // While dragging, the latest (draft) view rendering stands in for the current one.
            let view = self.tiles.view.iter().filter(|v| v.version == self.edit_version || self.texture_draft).map(|v| (v.region, v.texture.id(), None));
            let shown: Vec<_> = grid.chain(view).collect();
            let (sensitivity, ctx) = (self.focus_sensitivity, ui.ctx().clone());
            for (r, texture, index) in shown {
                let (x, y, w, h) = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
                let tr = Rect::from_min_size(img_rect.min + vec2(x, y) / k * scale, vec2(w, h) / k * scale);
                if !tr.intersects(rect) {
                    continue;
                }
                painter.image(texture, tr, full_uv, Color32::WHITE);
                // The focus mask at full resolution, else the preview's part of it.
                let own = match index {
                    _ if !self.focus_mask => None,
                    Some(i) => self.tiles.focus.get_mut(&i).map(|f| f.overlay(&ctx, &format!("focus tile{i:?}"), sensitivity)),
                    None => self.tiles.view.as_mut().and_then(|v| v.focus.as_mut()).map(|f| f.overlay(&ctx, "focus view tile", sensitivity)),
                };
                if let Some(mask) = own {
                    painter.image(mask, tr, full_uv, Color32::WHITE);
                } else if let Some(mask) = preview_mask {
                    let uv = Rect::from_min_size(pos2(x / fw, y / fh), vec2(w / fw, h / fh));
                    painter.image(mask, tr, uv, Color32::WHITE);
                }
            }
        }

        // Split before/after view.
        let mut split_drag = false;
        if let Some((left, right)) = side {
            if let Some(before) = &self.before_texture
                && before.size_vec2() == size
            {
                ui.painter_at(left).image(before.id(), img_rect.translate(left.center() - right.center()), full_uv, Color32::WHITE);
            }
            for (text, half) in [("Vorher", left), ("Nachher", right)] {
                let galley = painter.layout_no_wrap(text.into(), egui::FontId::proportional(12.0), Color32::WHITE);
                let r = egui::Align2::LEFT_BOTTOM.anchor_size(pos2(half.left() + 14.0, half.bottom() - 14.0), galley.size());
                let p = ui.painter_at(half);
                p.rect_filled(r.expand(4.0), 3.0, Color32::from_black_alpha(160));
                p.galley(r.min, galley, Color32::WHITE);
            }
        } else if self.split_view
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

        // Filling the paper: what the print cuts off is dimmed.
        if let Some(paper) = paper_rect.filter(|r| !r.contains_rect(img_rect)) {
            let cut = paper.intersect(img_rect);
            let dim = Color32::from_black_alpha(170);
            for r in [
                Rect::from_min_max(img_rect.min, pos2(img_rect.max.x, cut.min.y)),
                Rect::from_min_max(pos2(img_rect.min.x, cut.max.y), img_rect.max),
                Rect::from_min_max(pos2(img_rect.min.x, cut.min.y), pos2(cut.min.x, cut.max.y)),
                Rect::from_min_max(pos2(cut.max.x, cut.min.y), pos2(img_rect.max.x, cut.max.y)),
            ] {
                if r.is_positive() {
                    painter.rect_filled(r, 0.0, dim);
                }
            }
            painter.rect_stroke(cut, 0.0, egui::Stroke::new(1.0, Color32::from_white_alpha(140)), egui::StrokeKind::Outside);
        }

        // Over the before image of the divider view as well.
        // The crop and straighten tools draw their own guides.
        if self.crop_tool.is_none() && self.straighten_tool.is_none() {
            self.draw_guides(ui.ctx(), &painter, img_rect);
        }

        // Not in distraction-free full screen.
        if self.show_navigator && !self.fullscreen && !self.side_column_shown && self.zoom.is_some() && !rect.contains_rect(img_rect) && !tools_active {
            self.navigator(ui, rect, img_rect, tex_id);
        }

        // Side by side, the before half only zooms and pans: tools and pickers act on the
        // after half (where a drag started, while it lasts).
        let in_after = side.is_none_or(|(_, r)| ui.input(|i| i.pointer.press_origin()).or(response.hover_pos()).is_none_or(|p| r.contains(p)));
        // Loupe: holding the mouse button on the fitted image shows it at 100 % under the cursor.
        let loupe = self.zoom.is_none()
            && !tools_active
            && !self.picking_wb
            && !self.picking_skin
            && !split_drag
            && !space_pan
            && in_after
            && response.is_pointer_button_down_on()
            && ui.input(|i| i.pointer.primary_down());
        // Debug aid (screenshots): a loupe at a relative image position.
        let forced = self.debug_loupe.map(|v| img_rect.min + v * img_rect.size());
        if let Some(p) = forced.or_else(|| response.interact_pointer_pos().filter(|p| loupe && img_rect.contains(*p))) {
            self.loupe(ui, rect, p, img_rect, scale, size, shown);
        }
        if split_drag || !in_after {
            // The divider takes the drag.
        } else if space_pan {
            // Only panning while the space bar is held.
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
        } else if self.picking_skin {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            if response.clicked()
                && let Some(pos) = response.interact_pointer_pos()
                && img_rect.contains(pos)
            {
                self.pick_skin_tone((pos - img_rect.min) / scale);
            }
        } else {
            // No tool: zoomed in, the image can be moved; fitted, it can be magnified
            // (double-click, or hold the button for the loupe).
            let free = ui.ctx().output(|o| o.cursor_icon) == egui::CursorIcon::Default;
            if free && (response.dragged() || response.hover_pos().is_some_and(|p| img_rect.contains(p))) {
                let icon = match (self.zoom.is_some(), response.dragged()) {
                    (true, true) => egui::CursorIcon::Grabbing,
                    (true, false) => egui::CursorIcon::Grab,
                    (false, _) => egui::CursorIcon::ZoomIn,
                };
                ui.ctx().set_cursor_icon(icon);
            }
            if response.double_clicked() {
                self.toggle_zoom_100();
            }
        }

        // File name and zoom are shown in the status bar; the viewer only marks active modes.
        let full_w = self.full_output_width().unwrap_or(size.x);
        self.zoom_pct = Some(scale * ppp * size.x / full_w.max(1.0) * 100.0);
        let mode = if let Some(msg) = &self.speed_message {
            Some(msg.trim().to_string())
        } else if self.crop_tool.is_some() {
            Some("Zuschneiden – Enter übernimmt, Esc beendet".into())
        } else if self.straighten_tool.is_some() {
            Some("Geraderichten – Linie entlang des Horizonts ziehen".into())
        } else if self.picking_wb {
            Some("Weißabgleich – auf eine neutrale Fläche klicken".into())
        } else if self.picking_skin {
            Some("Hautton – auf eine typische Hautstelle klicken".into())
        } else if self.show_before {
            Some("Vorher".into())
        } else {
            None
        };
        if let Some(text) = mode {
            let galley = painter.layout_no_wrap(text, egui::FontId::proportional(13.0), Color32::WHITE);
            let pos = pos2(rect.center().x - galley.size().x / 2.0, rect.top() + 12.0);
            painter.rect_filled(Rect::from_min_size(pos, galley.size()).expand2(vec2(10.0, 5.0)), 12.0, Color32::from_black_alpha(170));
            painter.galley(pos, galley, Color32::WHITE);
        }
        if self.loading.is_some() {
            painter.text(rect.right_top() + vec2(-10.0, 8.0), egui::Align2::RIGHT_TOP, "Lade…", egui::FontId::proportional(13.0), Color32::from_gray(200));
        }
        let mut labels: Vec<(String, Color32, Option<String>)> = Vec::new();
        if self.proof_view {
            let text = match (&self.proof, self.recipes.get(self.selected_recipe)) {
                (Some(p), Some(r)) if proof_ready => {
                    let format = match r.format {
                        sk_core::export::ExportFormat::Jpeg => format!("JPEG {}", r.quality),
                        f => f.label().to_string(),
                    };
                    // The recipe's name only if it says more than the format.
                    let name = if r.name.is_empty() || r.name.eq_ignore_ascii_case(r.format.label()) { String::new() } else { format!("{}  ·  ", r.name) };
                    let warn = if self.softproof.settings.gamut_warning && r.output_space != sk_core::color::OutputSpace::ProPhoto { "  ·  beschnitten: magenta" } else { "" };
                    format!("PROOF  ·  {name}{format}  ·  {} × {} px  ·  {}  ·  {}{warn}", p.width, p.height, r.output_space.label(), file_size(p.file_size))
                }
                (_, Some(r)) => format!("PROOF  ·  {}  ·  wird berechnet …", r.name),
                (_, None) => "PROOF  ·  kein Ausgabe-Rezept".into(),
            };
            labels.push((text, theme::ACCENT.gamma_multiply(0.85), None));
        }
        if let Some(text) = self.softproof_label() {
            labels.push((text, Color32::from_rgb(150, 60, 150).gamma_multiply(0.9), None));
        }
        if let Some(label) = self.print_label(ui.ctx(), size) {
            labels.push(label);
        }
        // Active output previews, one below the other.
        for (i, (text, fill, tip)) in labels.into_iter().enumerate() {
            let galley = painter.layout_no_wrap(text, egui::FontId::proportional(12.0), Color32::WHITE);
            let pos = rect.left_top() + vec2(14.0, 12.0 + 28.0 * i as f32);
            let r = Rect::from_min_size(pos, galley.size()).expand2(vec2(8.0, 4.0));
            painter.rect_filled(r, 4.0, fill);
            painter.galley(pos, galley, Color32::WHITE);
            if let Some(tip) = tip {
                ui.interact(r, ui.id().with(("output-label", i)), Sense::hover()).on_hover_text(tip);
            }
        }
        if let Some(until) = self.fullscreen_hint_until {
            let left = until.saturating_duration_since(std::time::Instant::now()).as_secs_f32();
            if left > 0.0 {
                let alpha = left.min(0.6) / 0.6;
                let galley = painter.layout_no_wrap("Vollbild  ·  Esc oder F11 beendet".into(), egui::FontId::proportional(13.0), Color32::WHITE.gamma_multiply(alpha));
                let pos = pos2(rect.center().x - galley.size().x / 2.0, rect.top() + 16.0);
                painter.rect_filled(Rect::from_min_size(pos, galley.size()).expand2(vec2(12.0, 6.0)), 12.0, Color32::from_black_alpha((170.0 * alpha) as u8));
                painter.galley(pos, galley, Color32::WHITE.gamma_multiply(alpha));
                ui.ctx().request_repaint();
            }
        }
    }

    /// Minimap of the whole image in the corner of the viewer, with the visible part framed.
    /// Clicking or dragging in it moves the view there.
    fn navigator(&mut self, ui: &mut Ui, viewport: Rect, img_rect: Rect, texture: egui::TextureId) {
        let aspect = img_rect.width() / img_rect.height();
        let max = vec2(200.0, 140.0);
        let size = if aspect > max.x / max.y { vec2(max.x, max.x / aspect) } else { vec2(max.y * aspect, max.y) };
        let nav = Rect::from_min_size(viewport.right_bottom() - size - vec2(16.0, 16.0), size);
        let painter = ui.painter_at(viewport);
        painter.rect_filled(nav.expand(4.0), 5.0, Color32::from_black_alpha(200));
        painter.image(texture, nav, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);

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

        let resp = ui.interact(nav.expand(4.0), ui.id().with("navigator"), Sense::click_and_drag());
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
        }
        if (resp.clicked() || resp.dragged())
            && let Some(p) = resp.interact_pointer_pos()
        {
            // Center the view on the chosen point.
            let f = ((p - nav.min) / nav.size()).clamp(Vec2::ZERO, vec2(1.0, 1.0));
            let target = img_rect.min + f * img_rect.size();
            self.pan += viewport.center() - target;
            ui.ctx().request_repaint();
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
    /// The image at 100 % (one image pixel per screen pixel) in a square around `cursor`, from
    /// the full-resolution tiles where they are ready, else from the magnified preview.
    #[allow(clippy::too_many_arguments)]
    fn loupe(&mut self, ui: &Ui, viewport: Rect, cursor: Pos2, img_rect: Rect, scale: f32, size: Vec2, preview: egui::TextureId) {
        const SIDE: f32 = 260.0;
        let Some(full_w) = self.full_output_width() else { return };
        let ppp = self.pixels_per_point;
        // Full-resolution pixels per preview pixel, and the full-resolution position under the cursor.
        let k = full_w / size.x;
        let at = (((cursor - img_rect.min) / scale) * k).round();
        // Ask for the full resolution of this spot (also while the button is held).
        self.want_full_now();
        let spot = Rect::from_center_size(cursor, Vec2::splat(SIDE * ppp / k * scale));
        self.update_tiles(spot, img_rect, scale, true);

        let r = Rect::from_center_size(((cursor.to_vec2() * ppp).round() / ppp).to_pos2(), Vec2::splat(SIDE));
        let painter = ui.painter_at(r.intersect(viewport));
        painter.rect_filled(r, 0.0, theme::VIEWER);
        let to_screen = |full: Vec2| r.center() + (full - at) / ppp;
        let full_uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        painter.image(preview, Rect::from_min_max(to_screen(Vec2::ZERO), to_screen(size * k)), full_uv, Color32::WHITE);
        if self.tiles.version == self.edit_version && self.tiles.full_size.0 > 0 {
            let grid = self.tiles.textures.iter().map(|(&(tx, ty), t)| {
                let [w, h] = t.size();
                (sk_core::geometry::Region { x: tx * TILE, y: ty * TILE, w, h }, t.id())
            });
            let view = self.tiles.view.iter().filter(|v| v.version == self.edit_version && v.level == 1).map(|v| (v.region, v.texture.id()));
            for (reg, texture) in grid.chain(view) {
                let min = to_screen(vec2(reg.x as f32, reg.y as f32));
                let tr = Rect::from_min_size(min, vec2(reg.w as f32, reg.h as f32) / ppp);
                if tr.intersects(r) {
                    painter.image(texture, tr, full_uv, Color32::WHITE);
                }
            }
        }
        // Frame and scale.
        let p = ui.painter_at(viewport);
        p.rect_stroke(r, 4.0, egui::Stroke::new(2.0, Color32::from_gray(230)), egui::StrokeKind::Outside);
        let label = p.layout_no_wrap("100 %".into(), egui::FontId::proportional(11.0), Color32::WHITE);
        let lr = Rect::from_min_size(r.left_bottom() + vec2(6.0, -6.0 - label.size().y), label.size());
        p.rect_filled(lr.expand(3.0), 3.0, Color32::from_black_alpha(160));
        p.galley(lr.min, label, Color32::WHITE);
    }

    pub(crate) fn full_output_width(&self) -> Option<f32> {
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

    /// Zoom to 100 % (one full-resolution pixel per physical screen pixel) or back to fit.
    pub fn toggle_zoom_100(&mut self) {
        self.print.zoom = false;
        if self.zoom.is_some() {
            self.zoom = None;
        } else if let (Some(full_w), Some(size)) = (self.full_output_width(), self.image_size()) {
            self.zoom = Some(full_w / size.x / self.pixels_per_point);
        }
        self.pan = Vec2::ZERO;
    }

    /// Pixels a draft may have to keep up with a dragged control.
    fn draft_budget(&self) -> f32 {
        self.workers.draft_speed.pixels_within(DRAFT_MS).unwrap_or(DRAFT_PIXELS as f32).max(200_000.0)
    }

    /// Reduction of the whole-image draft while a control is dragged: as little as the measured
    /// speed allows, and never finer than the screen shows. Fixed for the whole drag.
    pub(crate) fn drag_draft_level(&mut self) -> usize {
        if let Some(l) = self.drag_draft_level {
            return l;
        }
        let Some(size) = self.view_size else { return 2 };
        let screen_w = self.view_geometry.map_or(size.x, |(_, r)| r.width() * self.pixels_per_point);
        let perf = (size.x * size.y / self.draft_budget()).sqrt().ceil();
        let screen = (size.x / screen_w.max(1.0)).floor();
        let level = perf.max(screen).clamp(1.0, 8.0) as usize;
        self.drag_draft_level = Some(level);
        level
    }

    /// Requests the missing full-resolution renderings: when zoomed in, the exact visible
    /// region (unless tiles already cover it), then the grid tiles nearest to the view
    /// center; otherwise all tiles once editing pauses, so that zooming in later is instant.
    pub(crate) fn update_tiles(&mut self, viewport: Rect, img_rect: Rect, scale: f32, zoomed: bool) {
        let Some(cur) = &self.current else { return };
        let Some(full) = &cur.full else { return };
        let Some(size) = self.image_size() else { return };
        // A style preview has no full-resolution rendering (only while hovering).
        if self.crop_tool.is_some() || self.straighten_tool.is_some() || (self.pointer_down && !zoomed) || self.style_preview.is_some() {
            return;
        }
        // Not zoomed: the order doesn't depend on the view, one request per edit is enough.
        let requested = self.tiles_requested.as_ref().is_some_and(|(v, ..)| *v == self.edit_version);
        if !zoomed && (requested || self.edited_at.elapsed() < std::time::Duration::from_secs(1)) {
            return;
        }
        let p = sk_core::pipeline::Processor::new(&cur.meta, &cur.adjustments);
        let (fw, fh) = p.output_size(full.width, full.height);
        let k = fw as f32 / size.x;
        let to_full = |pt: Pos2| (((pt - img_rect.min) / scale) * k).to_pos2();
        let center = to_full(viewport.center());
        let vis = viewport.intersect(img_rect);
        let (a, b) = (to_full(vis.min), to_full(vis.max));
        let current = self.tiles.version == self.edit_version;
        let mut todo: Vec<(usize, usize)> = (0..fh.div_ceil(TILE))
            .flat_map(|y| (0..fw.div_ceil(TILE)).map(move |x| (x, y)))
            .filter(|t| !(current && self.tiles.textures.contains_key(t) && (!self.focus_mask || self.tiles.focus.contains_key(t))))
            .collect();
        if todo.is_empty() {
            return;
        }
        let tile_rect = |(x, y): (usize, usize)| Rect::from_min_size(pos2((x * TILE) as f32, (y * TILE) as f32), vec2(TILE as f32, TILE as f32));
        let view = Rect::from_min_max(a, b);
        let visible = |t: (usize, usize)| tile_rect(t).intersects(view);
        todo.sort_by_key(|&t| (!(zoomed && visible(t)), tile_rect(t).center().distance_sq(center) as u64));
        // The exact view region, unless the tiles or the previous view region cover it.
        let pad = 32.0;
        let (x0, y0) = ((a.x - pad).max(0.0) as usize, (a.y - pad).max(0.0) as usize);
        let (x1, y1) = (((b.x + pad) as usize).min(fw), ((b.y + pad) as usize).min(fh));
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let region = sk_core::geometry::Region { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
        // Something was edited while the mouse is down (not just panning): no sharp rendering yet.
        let sharp = self.tiles.version == self.edit_version || self.tiles.view.as_ref().is_some_and(|v| v.version == self.edit_version && v.level == 1);
        if self.pointer_down && !sharp {
            // Dragging a control: only the view, reduced as far as the measured speed requires
            // (and never finer than the screen), so it keeps up but stays as sharp as possible.
            let level = match self.drag_view_level {
                Some(l) => l,
                None => {
                    let perf = ((region.w * region.h) as f32 / self.draft_budget()).sqrt().ceil();
                    let screen = (region.w as f32 / (vis.width() * self.pixels_per_point).max(1.0)).floor();
                    let l = perf.max(screen).clamp(1.0, 16.0) as usize;
                    self.drag_view_level = Some(l);
                    l
                }
            };
            let key = (self.edit_version, Some((region, level)), Vec::new());
            if self.tiles_requested.as_ref() != Some(&key) {
                self.tiles_requested = Some(key);
                self.send_tiles(Some((region, level)), Vec::new());
            }
            return;
        }
        // Fitted on a high-density screen (the preview is magnified): first the view reduced to
        // about the screen resolution, which is quick, then the full resolution. Without this
        // step the magnified preview stays blurry until the whole image is rendered in full.
        let screen_level = (region.w as f32 / (vis.width() * self.pixels_per_point).max(1.0)).floor().max(1.0) as usize;
        // A coarser draft left over from dragging doesn't count: it would stay on screen until
        // the full resolution is done.
        let have_view = self.tiles.view.as_ref().is_some_and(|v| v.version == self.edit_version && v.level <= screen_level);
        if zoomed && !sharp && screen_level >= 2 && !have_view {
            let key = (self.edit_version, Some((region, screen_level)), Vec::new());
            if self.tiles_requested.as_ref() != Some(&key) {
                self.tiles_requested = Some(key);
                self.send_tiles(Some((region, screen_level)), Vec::new());
            }
            return;
        }
        let covered_by_view = self.tiles.view.as_ref().is_some_and(|v| {
            let r = v.region;
            v.level == 1
                && v.version == self.edit_version
                && (!self.focus_mask || v.focus.is_some())
                && r.x <= a.x.max(0.0) as usize
                && r.y <= a.y.max(0.0) as usize
                && r.x + r.w >= (b.x as usize).min(fw)
                && r.y + r.h >= (b.y as usize).min(fh)
        });
        let view_region = (zoomed && !covered_by_view && todo.iter().any(|&t| visible(t))).then_some((region, 1));
        let key = (self.edit_version, view_region, todo);
        // Panning within the requested view region needs no new request.
        let contains = |r: &sk_core::geometry::Region, n: &sk_core::geometry::Region| r.x <= n.x && r.y <= n.y && r.x + r.w >= n.x + n.w && r.y + r.h >= n.y + n.h;
        if self.tiles_requested.as_ref().is_some_and(|(v, r, t)| {
            *v == key.0 && *t == key.2 && (*r == key.1 || r.is_some_and(|(r, l)| l == 1 && key.1.is_some_and(|(n, _)| contains(&r, &n))))
        }) {
            return;
        }
        let (_, view_region, todo) = key.clone();
        self.tiles_requested = Some(key);
        self.send_tiles(view_region, todo);
    }

    fn send_tiles(&self, view: Option<(sk_core::geometry::Region, usize)>, tiles: Vec<(usize, usize)>) {
        let Some(cur) = &self.current else { return };
        let Some(full) = &cur.full else { return };
        self.workers.tiles(crate::workers::TileRequest {
            generation: self.generation(),
            version: self.edit_version,
            meta: cur.meta.clone(),
            full: full.clone(),
            adjustments: cur.adjustments.clone(),
            clip_warning: self.clip_warning,
            mask_overlay: self.mask_overlay(),
            before: self.show_before,
            view,
            tiles,
            focus: self.focus_mask,
            softproof: self.softproof.transform.clone().filter(|_| self.softproof.on),
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
            self.workers.thumbnail(entry.path.clone(), self.session.as_ref().map(|s| s.root.clone()));
        }
        let selected = self.selection.contains(&entry.path);
        let painter = ui.painter_at(rect);
        let is_primary = Some(&entry.path) == primary;
        let bg = if is_primary {
            Color32::from_rgb(52, 56, 64)
        } else if selected {
            Color32::from_rgb(46, 47, 52)
        } else if resp.hovered() {
            Color32::from_rgb(40, 40, 44)
        } else {
            Color32::from_rgb(33, 33, 36)
        };
        painter.rect_filled(rect, 8.0, bg);
        if is_primary || selected {
            let stroke = if is_primary { egui::Stroke::new(2.0, theme::ACCENT) } else { egui::Stroke::new(1.0, Color32::from_rgb(96, 104, 120)) };
            painter.rect_stroke(rect, 8.0, stroke, egui::StrokeKind::Inside);
        }
        let img_area = Rect::from_min_max(rect.min + vec2(7.0, 7.0), rect.max - vec2(7.0, 22.0));
        if let Some(t) = self.thumbs.get(&entry.path) {
            let s = t.size_vec2();
            let k = (img_area.width() / s.x).min(img_area.height() / s.y);
            let r = Rect::from_center_size(img_area.center(), s * k);
            painter.rect_filled(r.translate(vec2(0.0, 2.0)).expand(1.0), 2.0, Color32::from_black_alpha(70));
            painter.image(t.id(), r, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
        } else {
            // Not loaded yet: a quiet placeholder of the usual 3:2 shape.
            let k = (img_area.width() / 3.0).min(img_area.height() / 2.0);
            painter.rect_filled(Rect::from_center_size(img_area.center(), vec2(3.0 * k, 2.0 * k)), 2.0, Color32::from_rgb(42, 42, 46));
        }
        let name_font = egui::FontId::proportional(11.5);
        let mut job = egui::text::LayoutJob::simple_singleline(entry.name.clone(), name_font, if is_primary { Color32::WHITE } else { theme::TEXT_DIM });
        job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width() - 12.0);
        let name = painter.layout_job(job);
        painter.galley(pos2(rect.center().x - name.size().x / 2.0, rect.bottom() - 11.0 - name.size().y / 2.0), name, theme::TEXT_DIM);
        widgets::paint_badges(ui, img_area, entry.rating, entry.color_label.rgb());
        let stack_toggle = self.stack_badge(ui, idx, img_area);
        self.culling_badges(ui, idx, img_area);
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
        // Right-click acts on the selection; an unselected image becomes the selection first.
        // It also becomes the image in the viewer, which the variant commands act on.
        if resp.secondary_clicked() {
            let path = entry.path.clone();
            if selected {
                self.make_primary(&path);
            } else {
                self.select_path(&path, Modifiers::NONE);
            }
        }
        resp.context_menu(|ui| {
            self.variant_menu(ui);
            self.stack_menu(ui);
            ui.separator();
            self.move_menu(ui);
        });
        if let Some(id) = stack_toggle {
            self.toggle_stack(&id);
            return (None, false);
        }
        let mods = resp.clicked().then(|| ui.input(|i| i.modifiers));
        (mods, resp.double_clicked())
    }

    /// Culling marks (top right): closed eyes, blurry.
    fn culling_badges(&self, ui: &Ui, idx: usize, img_area: Rect) {
        let Some(c) = self.entries[idx].culling else { return };
        let mut marks: Vec<(&str, Color32)> = Vec::new();
        if c.eyes_closed > 0 {
            marks.push((theme::icon::EYE_CLOSED, Color32::from_rgb(230, 150, 60)));
        }
        if c.blurry() {
            marks.push((theme::icon::DROP_HALF, Color32::from_rgb(220, 80, 80)));
        }
        let mut x = img_area.right() - 4.0;
        for (ic, color) in marks {
            let galley = ui.painter().layout_no_wrap(ic.to_string(), egui::FontId::proportional(13.0), Color32::WHITE);
            let rect = Rect::from_min_size(pos2(x - galley.size().x - 6.0, img_area.top() + 2.0), galley.size() + vec2(6.0, 2.0));
            ui.painter().rect_filled(rect, 3.0, color.gamma_multiply(0.9));
            ui.painter().galley(rect.min + vec2(3.0, 1.0), galley, Color32::WHITE);
            x = rect.left() - 3.0;
        }
    }

    /// Badge of a stacked image (top left): the number of images, with an arrow to expand or
    /// collapse. Returns the stack id if the badge was clicked.
    fn stack_badge(&self, ui: &mut Ui, idx: usize, img_area: Rect) -> Option<String> {
        let entry = &self.entries[idx];
        let s = entry.stack.as_ref()?;
        let n = self.stack_size(idx);
        if n < 2 {
            return None;
        }
        let expanded = self.expanded_stacks.contains(&s.id);
        let is_top = s.top && sk_core::variant::number(&entry.path) == 1;
        let text = if is_top || !expanded {
            format!("{} {n} {}", theme::icon::STACK_SIMPLE, if expanded { theme::icon::CARET_LEFT } else { theme::icon::CARET_RIGHT })
        } else {
            theme::icon::STACK_SIMPLE.to_string()
        };
        let font = egui::FontId::proportional(11.0);
        let galley = ui.painter().layout_no_wrap(text, font, Color32::WHITE);
        let rect = Rect::from_min_size(img_area.min + vec2(2.0, 2.0), galley.size() + vec2(8.0, 4.0));
        let resp = ui.interact(rect, ui.id().with(("stack badge", &entry.path)), Sense::click());
        let fill = if is_top || !expanded { theme::ACCENT.gamma_multiply(if resp.hovered() { 1.0 } else { 0.85 }) } else { Color32::from_black_alpha(150) };
        ui.painter().rect_filled(rect, 3.0, fill);
        ui.painter().galley(rect.min + vec2(4.0, 2.0), galley, Color32::WHITE);
        let hover = if expanded { "Stapel zuklappen (Umschalt+G)" } else { "Stapel aufklappen (Umschalt+G)" };
        resp.on_hover_text(hover).clicked().then(|| s.id.clone())
    }

    /// Start screen when no folder with images is open.
    fn welcome(&mut self, ui: &mut Ui, rect: Rect) {
        let inner = Rect::from_center_size(rect.center(), vec2(rect.width().min(440.0), 300.0));
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Center)));
        let ui = &mut ui;
        ui.label(egui::RichText::new(theme::icon::APERTURE).size(56.0).color(theme::ACCENT));
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Willkommen bei Silberkorn").size(22.0).color(Color32::WHITE));
        ui.add_space(2.0);
        ui.label(egui::RichText::new("Einen Ordner mit RAW-Dateien öffnen oder eine Session anlegen.").color(theme::TEXT_DIM));
        ui.add_space(18.0);
        let big = |text: String, primary: bool| {
            let b = egui::Button::new(egui::RichText::new(text).size(14.0).color(Color32::WHITE)).min_size(vec2(220.0, 36.0)).corner_radius(8);
            if primary { b.fill(theme::ACCENT) } else { b }
        };
        if ui.add(big(format!("{}  Ordner öffnen …", theme::icon::FOLDER_OPEN), true)).on_hover_text("Strg+O").clicked() {
            self.open_folder_dialog();
        }
        ui.add_space(4.0);
        if ui.add(big(format!("{}  Neue Session …", theme::icon::FOLDER_STAR), false)).on_hover_text("Strg+N").clicked() {
            let folder = self.folder.clone();
            self.session_ui.start_new(&folder);
        }
        ui.add_space(4.0);
        if ui.add(big(format!("{}  Session öffnen …", theme::icon::FOLDER_OPEN), false)).on_hover_text("Strg+Umschalt+O").clicked() {
            self.open_session_dialog();
        }
        ui.add_space(18.0);
        ui.label(egui::RichText::new("Tipp: Strg+K findet jede Funktion, F1 zeigt alle Tastenkürzel.").small().color(theme::TEXT_FAINT));
    }

    pub fn filmstrip(&mut self, ui: &mut Ui) {
        let visible = self.visible.clone();
        let mut clicked: Option<(PathBuf, Modifiers)> = None;
        let primary = self.primary_path();
        let cell = vec2(150.0, ui.available_height().clamp(90.0, 260.0) - 8.0);
        // The mouse wheel scrolls the strip sideways (egui only does that with Shift).
        if ui.rect_contains_pointer(ui.available_rect_before_wrap()) {
            ui.input_mut(|i| {
                if i.smooth_scroll_delta.x == 0.0 {
                    (i.smooth_scroll_delta.x, i.smooth_scroll_delta.y) = (i.smooth_scroll_delta.y, 0.0);
                }
            });
        }
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
        ui.painter().rect_filled(rect, 0.0, theme::BROWSER);
        let visible = self.visible.clone();
        let primary = self.primary_path();
        let mut clicked: Option<(PathBuf, Modifiers)> = None;
        let mut open: Option<PathBuf> = None;
        // Ctrl + mouse wheel changes the thumbnail size.
        if ui.rect_contains_pointer(rect) {
            let zoom = ui.input(|i| i.zoom_delta());
            if zoom != 1.0 {
                self.grid_thumb = (self.grid_thumb * zoom).clamp(crate::app::GRID_THUMB.0, crate::app::GRID_THUMB.1);
            }
        }
        let gap = 8.0;
        let cols = ((rect.width() - 24.0 + gap) / (self.grid_thumb + gap)).floor().max(1.0) as usize;
        // Stretch the cells so the rows fill the width.
        let w = ((rect.width() - 24.0 + gap) / cols as f32 - gap).max(60.0);
        let cell = vec2(w, (w * 0.78 + 24.0).round());
        if visible.is_empty() {
            let r = ui.available_rect_before_wrap();
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink2(vec2(0.0, r.height() * 0.25))));
            widgets::empty_state(&mut child, theme::icon::IMAGES, "Keine Bilder in dieser Ansicht", "Filter lockern oder in der Bibliothek einen anderen Ordner wählen");
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(12.0);
            for row in visible.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    ui.add_space(12.0);
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
