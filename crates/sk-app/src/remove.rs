//! The "Remove" tool (generative fill, see `sk_ai::inpaint`): paint over what should go, the
//! network fills it when the mouse is released. Each removal is an undo step.

use std::sync::Arc;

use egui::{Color32, Rect, Response, RichText, Stroke, Ui};
use sk_ai::models;
use sk_core::params::{BrushStroke, WbMode};

use crate::app::App;
use crate::layers::{LocalDrag, LocalTool};
use crate::theme::icon;
use crate::widgets::{section, slider_row};
use crate::workers::InpaintJob;

impl App {
    /// Viewer interaction of the remove tool: painting the strokes, then the request.
    pub(crate) fn remove_interaction(&mut self, ui: &mut Ui, response: &Response, img_rect: Rect, scale: f32) {
        let painter = ui.painter_at(response.rect);
        // The marked area in red (also while it is being filled).
        for s in &self.remove_strokes {
            let r = self.radius_screen(s.radius, scale);
            let pts: Vec<egui::Pos2> = s.points.iter().filter_map(|p| self.uv_to_screen(*p, img_rect, scale)).collect();
            let fill = Color32::from_rgba_unmultiplied(230, 40, 40, 110);
            for p in &pts {
                painter.circle_filled(*p, r, fill);
            }
            for w in pts.windows(2) {
                painter.line_segment([w[0], w[1]], Stroke::new(2.0 * r, fill));
            }
        }
        if self.inpaint_busy {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Progress);
            return;
        }
        if let Some(p) = response.hover_pos() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
            painter.circle_stroke(p, self.brush.size, Stroke::new(1.5, Color32::WHITE));
            painter.circle_stroke(p, self.brush.size, Stroke::new(1.0, Color32::from_rgb(230, 40, 40)));
        }
        if response.drag_started()
            && let Some(p) = response.interact_pointer_pos()
            && let Some(uv) = self.screen_to_uv(p, img_rect, scale)
        {
            self.remove_strokes.push(BrushStroke { points: vec![uv], radius: self.radius_norm(self.brush.size, scale), hardness: 0.7, flow: 1.0, erase: false });
            self.local_drag = LocalDrag::Stroke(p);
        }
        if response.dragged()
            && let (Some(p), LocalDrag::Stroke(last)) = (response.interact_pointer_pos(), self.local_drag.clone())
            && (p - last).length() > (self.brush.size * 0.15).max(2.0)
            && let Some(uv) = self.screen_to_uv(p, img_rect, scale)
        {
            if let Some(s) = self.remove_strokes.last_mut() {
                s.points.push(uv);
            }
            self.local_drag = LocalDrag::Stroke(p);
        }
        if response.drag_stopped() && !self.remove_strokes.is_empty() {
            self.request_removal();
        }
    }

    /// Fills the painted area in the background.
    pub(crate) fn request_removal(&mut self) {
        let Some(cur) = &self.current else { return };
        if !models::available(&models::INPAINT) {
            self.status = "KI-Modell für „Entfernen“ fehlt – bitte im Tab „Ebenen & Retusche“ laden".into();
            self.remove_strokes.clear();
            return;
        }
        let wb = match cur.adjustments.white_balance.mode {
            WbMode::AsShot => cur.meta.as_shot_wb,
            WbMode::Custom => cur.meta.color.wb_for_temp_tint(cur.adjustments.white_balance.kelvin, cur.adjustments.white_balance.tint),
        };
        self.inpaint_busy = true;
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
        let mut tool = false;
        let mut download = false;
        let downloading = self.model_download_progress(&models::INPAINT);
        section(ui, "Entfernen (KI)", None, |ui| {
            if ui.selectable_label(self.local_tool == Some(LocalTool::Remove), LocalTool::Remove.label()).clicked() {
                tool = true;
            }
            if self.local_tool == Some(LocalTool::Remove) {
                slider_row(ui, "Größe", &mut self.brush.size, 2.0..=400.0, 40.0, 0);
                ui.label(RichText::new("Über das Störende malen; beim Loslassen füllt die KI die Stelle passend zur Umgebung.").small().color(Color32::from_gray(140)));
            }
            if !models::available(&models::INPAINT) {
                match downloading {
                    Some(p) => {
                        ui.add(egui::ProgressBar::new(p).text(format!("Modell wird geladen … {:.0} %", p * 100.0)));
                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
                    }
                    None => {
                        ui.label(RichText::new(format!("Benötigt das Modell {} ({} MB, Lizenz {}).", models::INPAINT.label, models::INPAINT.size_mb, models::INPAINT.license)).small());
                        if ui.button(format!("{}  Modell laden", icon::DOWNLOAD_SIMPLE)).clicked() {
                            download = true;
                        }
                    }
                }
            }
            if n > 0 {
                ui.horizontal(|ui| {
                    ui.label(format!("{n} {} entfernt", if n == 1 { "Bereich" } else { "Bereiche" }));
                    if ui.small_button("Letzten zurücknehmen").clicked() {
                        clear = Some(false);
                    }
                    if ui.small_button("Alle").on_hover_text("Alle entfernten Bereiche wiederherstellen").clicked() {
                        clear = Some(true);
                    }
                });
            }
        });
        if tool {
            self.set_local_tool(LocalTool::Remove);
        }
        if download {
            self.download_model_job(&models::INPAINT);
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
pub fn compute(job: &InpaintJob, model: &mut Option<sk_ai::inpaint::Inpainter>) -> Result<sk_core::params::Removal, String> {
    if model.is_none() {
        *model = Some(sk_ai::inpaint::Inpainter::load(&models::path(&models::INPAINT)).map_err(|e| format!("{e:#}"))?);
    }
    let full = match &job.full {
        Some(f) => f.clone(),
        None => Arc::new(sk_core::raw::load_full(&job.path).map_err(|e| format!("{e:#}"))?.1),
    };
    sk_ai::inpaint::make_removal(&full, job.wb, &job.strokes, model.as_ref().expect("loaded")).map_err(|e| format!("{e:#}"))
}
