//! The "Remove" tool (generative fill, see `sk_ai::inpaint`): paint over what should go, the
//! network fills it when the mouse is released. Each removal is an undo step.

use std::sync::Arc;

use egui::{Color32, Rect, Response, Ui};
use sk_ai::models;
use sk_core::brush::AutoGuide;
use sk_core::params::{BrushStroke, Removal, WbMode};

use crate::ai_models::AiFeature;
use crate::app::App;
use crate::layers::{AreaPaint, LocalDrag};
use crate::workers::InpaintJob;

impl App {
    /// Viewer interaction of the remove tool: painting the strokes, then the request. The
    /// areas removed so far are outlined; a click on a pin chooses one, which Shift then
    /// extends and Alt makes smaller (filled anew as a whole).
    pub(crate) fn remove_interaction(&mut self, ui: &mut Ui, response: &Response, img_rect: Rect, scale: f32) {
        let painter = ui.painter_at(response.rect);
        let Some(map) = self.uv_mapper(img_rect, scale) else { return };
        let removals = self.current.as_ref().map(|c| c.adjustments.removals.clone()).unwrap_or_default();
        let selected = self.selected_removal.filter(|i| *i < removals.len());
        // Each area as its strokes on screen: (points, radius, taken away); the pin on the first.
        let shown: Vec<Vec<(Vec<egui::Pos2>, f32, bool)>> = removals
            .iter()
            .map(|r| r.strokes.iter().filter(|s| !s.points.is_empty()).map(|s| (s.points.iter().map(|p| map.map(*p)).collect(), self.radius_screen(s.radius, scale), s.erase)).collect())
            .collect();
        let pin_at = |i: usize| shown[i].iter().find(|(_, _, erase)| !erase).map(|(pts, ..)| pts[0]);
        let pin_hit = |p: egui::Pos2| (0..shown.len()).rev().find(|i| pin_at(*i).is_some_and(|q| (p - q).length() < crate::layers::PIN_HIT));
        let hovered_pin = response.hover_pos().and_then(pin_hit).filter(|_| self.remove_strokes.is_empty());
        // While an area is filled anew, its strokes show in red instead.
        let replacing = self.remove_replaces.and_then(|id| removals.iter().position(|r| r.id == id));
        for (i, strokes) in shown.iter().enumerate().filter(|(i, _)| Some(*i) != replacing) {
            let sel = selected == Some(i);
            for (pts, r, erase) in strokes {
                if *erase && sel {
                    crate::layers::paint_area(&painter, pts, *r, crate::layers::ERASED);
                } else if !erase {
                    crate::layers::paint_area(&painter, pts, *r, if sel { Color32::WHITE } else { Color32::from_white_alpha(150) });
                }
            }
            if let Some(p) = pin_at(i) {
                crate::layers::pin(&painter, p, if sel { Color32::WHITE } else { Color32::from_white_alpha(170) }, hovered_pin == Some(i));
            }
        }
        // The marked area in red, as it will be removed (soft edge, opacity, automatic masking).
        self.draw_remove_overlay(ui, &painter, img_rect, scale);
        if self.inpaint_busy {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Progress);
            return;
        }
        let b = self.brushes.remove.clone();
        if hovered_pin.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        } else if let Some(p) = response.hover_pos().filter(|_| !self.adjusting_brush()) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
            crate::layers::brush_cursor(&painter, p, &b);
        }
        // Only the left button paints; the right one changes the brush. From the press on, like
        // the brush: a click marks a dab, and the start of the stroke isn't lost.
        if let Some(p) = crate::layers::pressed_on(ui, response) {
            if let Some(i) = pin_hit(p).filter(|_| self.remove_strokes.is_empty()) {
                self.selected_removal = Some(i);
                self.selected_spot = None;
                return;
            }
            let Some(uv) = self.screen_to_uv(p, img_rect, scale) else { return };
            let mode = self.stroke_mode(ui, selected.is_some());
            // Onto a chosen area: its strokes and this one are filled anew together.
            if mode != AreaPaint::New
                && self.remove_strokes.is_empty()
                && let Some(r) = selected.map(|i| &removals[i])
            {
                self.remove_strokes = r.strokes.clone();
                self.remove_replaces = Some(r.id);
            }
            self.remove_strokes.push(BrushStroke {
                points: vec![uv],
                radius: self.radius_norm(b.size, scale),
                hardness: b.hardness,
                flow: b.flow,
                erase: mode == AreaPaint::Shrink,
                opacity: b.opacity,
                auto_mask: b.auto_mask,
                build_up: true,
                tolerance: b.tolerance,
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
        if matches!(self.local_drag, LocalDrag::Stroke(_)) && ui.input(|i| i.pointer.primary_released()) && !self.remove_strokes.is_empty() {
            self.request_removal();
        }
    }

    /// Guide for automatic masking from the current image's preview (made once per image).
    pub(crate) fn viewer_auto_guide(&mut self) -> Option<Arc<AutoGuide>> {
        let cur = self.current.as_ref()?;
        let generation = self.generation();
        if self.auto_guide.as_ref().is_none_or(|(g, _)| *g != generation) {
            self.auto_guide = Some((generation, Arc::new(AutoGuide::from_image(&cur.preview, sk_core::mask::MASK_RES))));
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
        let key = strokes_key(&self.remove_strokes) ^ (scale.to_bits() as u64);
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
            // The texture of the last overlay is filled again rather than a new one made.
            let tex = match self.remove_overlay.take() {
                Some((_, mut tex, _)) => {
                    tex.set(image, egui::TextureOptions::LINEAR);
                    tex
                }
                None => ui.ctx().load_texture("remove overlay", image, egui::TextureOptions::LINEAR),
            };
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
        self.status = "Entferne …".into();
        self.workers.inpaint(InpaintJob {
            generation: self.generation(),
            path: cur.meta.path.clone(),
            full: cur.full.clone(),
            wb,
            // Kept on screen until the result arrives.
            strokes: self.remove_strokes.clone(),
            replaces: self.remove_replaces,
        });
    }

    pub(crate) fn removal_done(&mut self, generation: u64, replaces: Option<u64>, result: Result<sk_core::params::Removal, String>) {
        self.inpaint_busy = false;
        self.remove_strokes.clear();
        self.remove_replaces = None;
        if generation != self.generation() {
            return;
        }
        match result {
            Ok(r) => {
                // An area filled anew keeps its place and opacity (unless it was deleted meanwhile).
                let at = replaces.and_then(|id| self.current.as_ref()?.adjustments.removals.iter().position(|old| old.id == id));
                match at {
                    Some(i) => self.edit(|a| a.removals[i] = Removal { opacity: a.removals[i].opacity, ..r }),
                    None => self.edit(|a| a.removals.push(r)),
                }
                self.selected_removal = at.or_else(|| self.current.as_ref().map(|c| c.adjustments.removals.len() - 1));
                self.selected_spot = None;
                self.status = if at.is_some() { "Bereich neu gefüllt".into() } else { "Entfernt".into() };
            }
            Err(e) => self.report_error(format!("Entfernen fehlgeschlagen: {e}")),
        }
    }

    /// Deletes the chosen removal area (undoable).
    pub fn delete_selected_removal(&mut self) {
        if let Some(i) = self.selected_removal.take() {
            self.edit(|a| {
                if i < a.removals.len() {
                    a.removals.remove(i);
                }
            });
        }
    }
}

/// The network, loaded unless it is (again, when the runtime settings changed).
pub fn load_model(model: &mut Option<(sk_ai::inpaint::Inpainter, u64)>) -> Result<(), String> {
    let generation = sk_ai::runtime::config_generation();
    if model.as_ref().is_none_or(|(_, g)| *g != generation) {
        *model = Some((sk_ai::inpaint::Inpainter::load(&models::path(&models::INPAINT)).map_err(|e| format!("{e:#}"))?, generation));
    }
    Ok(())
}

/// Removal computed in the background (see `Workers::inpaint`).
pub fn compute(job: &InpaintJob, model: &mut Option<(sk_ai::inpaint::Inpainter, u64)>, progress: &dyn Fn(f32)) -> Result<sk_core::params::Removal, String> {
    load_model(model)?;
    let full = match &job.full {
        Some(f) => f.clone(),
        None => Arc::new(sk_core::raw::load_full(&job.path).map_err(|e| format!("{e:#}"))?.1),
    };
    progress(0.05);
    sk_ai::inpaint::make_removal(&full, job.wb, &job.strokes, &model.as_ref().expect("loaded").0, progress).map_err(|e| format!("{e:#}"))
}

/// Identifies the strokes (every frame while the tool is on: without serializing them).
fn strokes_key(strokes: &[sk_core::params::BrushStroke]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut add = |v: u32| h = (h ^ v as u64).wrapping_mul(0x0100_0000_01b3);
    for s in strokes {
        for v in [s.radius, s.hardness, s.flow, s.opacity, s.tolerance] {
            add(v.to_bits());
        }
        add(s.erase as u32 | (s.auto_mask as u32) << 1 | (s.build_up as u32) << 2);
        add(s.points.len() as u32);
        for p in &s.points {
            add(p[0].to_bits());
            add(p[1].to_bits());
        }
    }
    h
}
