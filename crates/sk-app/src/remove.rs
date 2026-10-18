//! The "Remove" tool (generative fill, see `sk_ai::inpaint`): paint over what should go, the
//! network fills it when the mouse is released. Each removal is an undo step.

use std::sync::Arc;

use egui::{Color32, Rect, Response, Ui};
use sk_ai::models;
use sk_core::brush::AutoGuide;
use sk_core::params::{BrushStroke, WbMode};

use crate::ai_models::AiFeature;
use crate::app::App;
use crate::layers::{LocalDrag, LocalTool};
use crate::widgets::section_help_ai;
use crate::workers::InpaintJob;

impl App {
    /// Viewer interaction of the remove tool: painting the strokes, then the request.
    pub(crate) fn remove_interaction(&mut self, ui: &mut Ui, response: &Response, img_rect: Rect, scale: f32) {
        let painter = ui.painter_at(response.rect);
        // The marked area in red, as it will be removed (soft edge, opacity, automatic masking).
        self.draw_remove_overlay(ui, &painter, img_rect, scale);
        if self.inpaint_busy {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Progress);
            return;
        }
        let b = self.brushes.remove.clone();
        if let Some(p) = response.hover_pos().filter(|_| !self.adjusting_brush()) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
            crate::layers::brush_cursor(&painter, p, &b);
        }
        // Only the left button paints; the right one changes the brush.
        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(p) = response.interact_pointer_pos()
            && let Some(uv) = self.screen_to_uv(p, img_rect, scale)
        {
            self.remove_strokes.push(BrushStroke {
                points: vec![uv],
                radius: self.radius_norm(b.size, scale),
                hardness: b.hardness,
                flow: b.flow,
                erase: false,
                opacity: b.opacity,
                auto_mask: b.auto_mask,
                build_up: true,
            });
            self.local_drag = LocalDrag::Stroke(p);
        }
        if response.dragged()
            && let LocalDrag::Stroke(mut last) = self.local_drag
        {
            // Every movement of the frame, so that fast strokes stay round.
            for p in crate::layers::drag_trail(ui, response) {
                if (p - last).length() > crate::layers::point_spacing(b.size)
                    && let Some(uv) = self.screen_to_uv(p, img_rect, scale)
                    && let Some(s) = self.remove_strokes.last_mut()
                {
                    s.points.push(uv);
                    last = p;
                }
            }
            self.local_drag = LocalDrag::Stroke(last);
        }
        if response.drag_stopped_by(egui::PointerButton::Primary) && !self.remove_strokes.is_empty() {
            self.request_removal();
        }
    }

    /// Guide for automatic masking from the current image's preview (made once per image).
    pub(crate) fn viewer_auto_guide(&mut self) -> Option<Arc<AutoGuide>> {
        let cur = self.current.as_ref()?;
        let generation = self.generation();
        if self.auto_guide.as_ref().is_none_or(|(g, _)| *g != generation) {
            self.auto_guide = Some((generation, Arc::new(AutoGuide::from_image(&cur.preview, 1024))));
        }
        self.auto_guide.as_ref().map(|(_, g)| g.clone())
    }

    /// Draws the strokes' coverage, rasterized for the area they touch at about screen resolution.
    fn draw_remove_overlay(&mut self, ui: &Ui, painter: &egui::Painter, img_rect: Rect, scale: f32) {
        if self.remove_strokes.is_empty() {
            self.remove_overlay = None;
            return;
        }
        let Some(cur) = &self.current else { return };
        let (pw, ph) = (cur.preview.width as f32, cur.preview.height as f32);
        let aspect = pw / ph;
        let (kx, ky) = if aspect >= 1.0 { (1.0, 1.0 / aspect) } else { (aspect, 1.0) };
        let key = sk_core::cache::fnv1a(serde_json::to_string(&self.remove_strokes).unwrap_or_default().as_bytes()) ^ (scale.to_bits() as u64);
        if self.remove_overlay.as_ref().is_none_or(|(k, ..)| *k != key) {
            // Area of the strokes (normalized), with their radius.
            let (mut u0, mut v0, mut u1, mut v1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for s in &self.remove_strokes {
                for p in &s.points {
                    u0 = u0.min(p[0] - s.radius / kx);
                    v0 = v0.min(p[1] - s.radius / ky);
                    u1 = u1.max(p[0] + s.radius / kx);
                    v1 = v1.max(p[1] + s.radius / ky);
                }
            }
            let (u0, v0, u1, v1) = (u0.max(0.0), v0.max(0.0), u1.min(1.0), v1.min(1.0));
            if u1 <= u0 || v1 <= v0 {
                return;
            }
            let rect = [u0, v0, u1 - u0, v1 - v0];
            // About one texel per screen pixel, at most 1024 across.
            let w = ((rect[2] * pw * scale).round() as usize).clamp(4, 1024);
            let h = ((rect[3] * ph * scale).round() as usize).clamp(4, 1024);
            let guide = self.remove_strokes.iter().any(|s| s.auto_mask).then(|| self.viewer_auto_guide()).flatten();
            let cover = sk_core::mask::rasterize_strokes_in(&self.remove_strokes, rect, w, h, aspect, guide.as_deref());
            let pixels: Vec<Color32> = cover.iter().map(|c| Color32::from_rgba_unmultiplied(235, 45, 45, (c.clamp(0.0, 1.0) * 150.0) as u8)).collect();
            let image = egui::ColorImage { size: [w, h], source_size: egui::vec2(w as f32, h as f32), pixels };
            let tex = ui.ctx().load_texture("remove overlay", image, egui::TextureOptions::LINEAR);
            self.remove_overlay = Some((key, tex, rect));
        }
        let Some((_, tex, rect)) = &self.remove_overlay else { return };
        // A quad through the mapped corners (follows rotation and straightening).
        let [x, y, w, h] = *rect;
        let corners = [[x, y], [x + w, y], [x + w, y + h], [x, y + h]];
        let Some(map) = self.uv_mapper(img_rect, scale) else { return };
        let screen: Vec<Option<egui::Pos2>> = corners.iter().map(|c| Some(map.map(*c))).collect();
        if screen.iter().any(|p| p.is_none()) {
            return;
        }
        let mut mesh = egui::Mesh::with_texture(tex.id());
        let uvs = [egui::pos2(0.0, 0.0), egui::pos2(1.0, 0.0), egui::pos2(1.0, 1.0), egui::pos2(0.0, 1.0)];
        for (p, uv) in screen.iter().zip(uvs) {
            mesh.vertices.push(egui::epaint::Vertex { pos: p.expect("checked"), uv, color: Color32::WHITE });
        }
        mesh.indices.extend([0, 1, 2, 0, 2, 3]);
        painter.add(egui::Shape::mesh(mesh));
    }

    /// Fills the painted area in the background.
    pub(crate) fn request_removal(&mut self) {
        if self.current.is_none() {
            return;
        }
        if !self.ensure_ai(AiFeature::Inpaint) {
            // The painted area stays on screen and is filled once the model is there.
            let generation = self.generation();
            self.with_ai(AiFeature::Inpaint, move |app| {
                if app.generation() == generation && !app.inpaint_busy && !app.remove_strokes.is_empty() {
                    app.request_removal();
                }
            });
            self.status = format!("Entfernen: lade KI-Modell ({} MB, einmalig) …", AiFeature::Inpaint.download_mb());
            return;
        }
        let Some(cur) = &self.current else { return };
        let wb = match cur.adjustments.white_balance.mode {
            WbMode::AsShot => cur.meta.as_shot_wb,
            WbMode::Custom => cur.meta.color.wb_for_temp_tint(cur.adjustments.white_balance.kelvin, cur.adjustments.white_balance.tint),
        };
        self.inpaint_busy = true;
        self.removal_progress = 0.0;
        self.status = "Entfernen …".into();
        self.workers.inpaint(InpaintJob {
            generation: self.generation(),
            path: cur.meta.path.clone(),
            full: cur.full.clone(),
            wb,
            // Kept on screen until the result arrives.
            strokes: self.remove_strokes.clone(),
        });
    }

    pub(crate) fn removal_done(&mut self, generation: u64, result: Result<sk_core::params::Removal, String>) {
        self.inpaint_busy = false;
        self.remove_strokes.clear();
        if generation != self.generation() {
            return;
        }
        match result {
            Ok(r) => {
                self.edit(|a| a.removals.push(r));
                self.status = "Entfernt".into();
            }
            Err(e) => self.status = format!("Entfernen fehlgeschlagen: {e}"),
        }
    }

    /// Section in the layers tab.
    pub(crate) fn remove_section(&mut self, ui: &mut Ui) {
        let n = self.current.as_ref().map_or(0, |c| c.adjustments.removals.len());
        let mut clear: Option<bool> = None;
        let downloading = self.ai_download_progress(AiFeature::Inpaint);
        let active = self.local_tool == Some(LocalTool::Remove);
        // Always there, like the other sections: the brush only works while the tool is on, the
        // buttons only with something removed (shown, but disabled otherwise).
        let help = "Über das Störende malen; beim Loslassen füllt die KI die Stelle passend zur Umgebung.";
        let mut reset = false;
        section_help_ai(ui, "Entfernen", help, Some(&mut || reset = true), |ui| {
            let brush = ui.add_enabled_ui(active, |ui| crate::layers::brush_controls(ui, &mut self.brushes.remove, Some(LocalTool::Remove), true));
            if brush.inner {
                self.brushes_changed_at = Some(std::time::Instant::now());
            }
            brush.response.on_disabled_hover_text(crate::commands::hint("Pinsel des Werkzeugs „Entfernen“", "tool-remove"));
            if let Some(p) = downloading {
                crate::ai_models::download_progress(ui, AiFeature::Inpaint, p);
            }
            ui.horizontal(|ui| {
                ui.label(match n {
                    0 => "Noch nichts entfernt".to_string(),
                    1 => "1 Bereich entfernt".to_string(),
                    _ => format!("{n} Bereiche entfernt"),
                });
                ui.add_enabled_ui(n > 0, |ui| {
                    if ui.small_button("Letzten zurücknehmen").clicked() {
                        clear = Some(false);
                    }
                    if ui.small_button("Alle löschen").on_hover_text("Alle entfernten Bereiche wiederherstellen").clicked() {
                        clear = Some(true);
                    }
                });
            });
        });
        if reset {
            clear = Some(true);
        }
        match clear {
            Some(true) => self.edit(|a| a.removals.clear()),
            Some(false) => self.edit(|a| {
                a.removals.pop();
            }),
            None => {}
        }
    }
}

/// Removal computed in the background (see `Workers::inpaint`).
pub fn compute(job: &InpaintJob, model: &mut Option<(sk_ai::inpaint::Inpainter, u64)>, progress: &dyn Fn(f32)) -> Result<sk_core::params::Removal, String> {
    let generation = sk_ai::runtime::config_generation();
    if model.as_ref().is_none_or(|(_, g)| *g != generation) {
        *model = Some((sk_ai::inpaint::Inpainter::load(&models::path(&models::INPAINT)).map_err(|e| format!("{e:#}"))?, generation));
    }
    let full = match &job.full {
        Some(f) => f.clone(),
        None => Arc::new(sk_core::raw::load_full(&job.path).map_err(|e| format!("{e:#}"))?.1),
    };
    progress(0.05);
    sk_ai::inpaint::make_removal(&full, job.wb, &job.strokes, &model.as_ref().expect("loaded").0, progress).map_err(|e| format!("{e:#}"))
}
