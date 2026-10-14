//! Depth maps (see `sk_core::depth` and `sk_ai::depth`): computed in the background for the image
//! in the viewer when its lens blur or a depth mask needs one, and kept in a cache. Here also the
//! "Objektivunschärfe" section, its focus pipette, and the depth mask of a layer.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use egui::{RichText, Ui, Vec2};
use sk_core::params::{DepthRange, LensBlur};
use sk_core::pipeline::{Processor, RenderOptions};

use crate::ai_models::{self, AiFeature};
use crate::app::App;
use crate::widgets::slider_row;

const JOB_LABEL: &str = "Tiefenkarte";
/// Long edge of depth masks (like the other masks from image content).
const MASK_EDGE: usize = sk_ai::MASK_EDGE;

#[derive(Default)]
pub struct DepthQueue {
    /// Job id and image being computed.
    pub(crate) running: Option<(u64, PathBuf)>,
    /// Why an image's map could not be computed (not tried again until the app restarts).
    failed: HashMap<PathBuf, String>,
    /// Cancelled by the user: not started again until the lens blur changes or a depth mask is
    /// asked for.
    cancelled: HashSet<PathBuf>,
    /// Layer that gets a depth mask once the image's map exists.
    /// Image and layer id waiting for the depth map.
    mask_for: Option<(PathBuf, u32)>,
    /// Image whose focus is set to its subject once the map exists (the lens blur was just
    /// switched on).
    autofocus: Option<PathBuf>,
}

/// What the lens blur section shows.
enum DepthState {
    NoModel,
    Downloading(f32),
    Running(f32),
    Waiting,
    Ready,
    Cancelled,
    Failed(String),
}

impl App {
    fn depth_state(&self, path: &Path) -> DepthState {
        if let Some(p) = self.ai_download_progress(AiFeature::Depth) {
            return DepthState::Downloading(p);
        }
        if !AiFeature::Depth.available() {
            return DepthState::NoModel;
        }
        if let Some(e) = self.depth.failed.get(path) {
            return DepthState::Failed(e.clone());
        }
        if let Some((id, p)) = &self.depth.running
            && p == path
        {
            return DepthState::Running(self.jobs.iter().find(|j| j.id == *id).map_or(0.0, |j| j.progress));
        }
        if sk_core::depth::is_ready(path) {
            DepthState::Ready
        } else if self.depth.cancelled.contains(path) {
            DepthState::Cancelled
        } else {
            DepthState::Waiting
        }
    }

    /// Each frame: starts computing the viewer image's map when something needs it.
    pub(crate) fn pump_depth(&mut self) {
        if self.depth.running.is_some() {
            return;
        }
        let Some(cur) = &self.current else { return };
        let path = cur.meta.path.clone();
        let wanted = !cur.adjustments.lens_blur.is_off() || self.depth.mask_for.as_ref().is_some_and(|(p, _)| *p == path);
        if !wanted || self.depth.failed.contains_key(&path) || self.depth.cancelled.contains(&path) || sk_core::depth::is_ready(&path) || !self.ensure_ai(AiFeature::Depth) {
            return;
        }
        let label = format!("{JOB_LABEL}: {}", path.file_name().unwrap_or_default().to_string_lossy());
        let file = path.clone();
        self.start_job(&label, Box::new(move |p| {
            sk_core::depth::ensure(&file, p)?;
            Ok(Vec::new())
        }));
        let id = self.jobs.last().map_or(0, |j| j.id);
        self.depth.running = Some((id, path));
    }

    /// The model download was cancelled: like cancelling the viewer image's map.
    pub(crate) fn depth_download_cancelled(&mut self) {
        self.depth.mask_for = None;
        if let Some(path) = self.current_path() {
            self.depth.cancelled.insert(path);
        }
    }

    /// A computation finished (`true` if the job was one).
    pub(crate) fn depth_job_done(&mut self, id: u64, result: &Result<Vec<PathBuf>, String>) -> bool {
        if self.depth.running.as_ref().is_none_or(|(i, _)| *i != id) {
            return false;
        }
        let (_, path) = self.depth.running.take().unwrap();
        self.jobs.retain(|j| j.id != id);
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        match result {
            Ok(_) => {
                self.status = format!("{JOB_LABEL}: {name} fertig");
                // Renderings of the image so far were made without the lens blur.
                self.thumb_requested.remove(&path);
                self.compare_textures.remove(&path);
                self.compare_requested.remove(&path);
                self.compare_details.remove(&path);
                self.forget_prepared(&path);
                if self.current_path().as_ref() == Some(&path) {
                    if self.depth.autofocus.take_if(|p| *p == path).is_some()
                        && self.current.as_ref().is_some_and(|c| c.adjustments.lens_blur.focus == LensBlur::default().focus)
                    {
                        self.focus_on_subject();
                    }
                    if let Some((_, id)) = self.depth.mask_for.take_if(|(p, _)| *p == path)
                        && let Some(layer) = self.layer_index(id)
                    {
                        self.set_depth_mask(layer, None);
                    }
                    self.request_render();
                }
            }
            Err(e) => {
                self.status = format!("{JOB_LABEL} für {name} fehlgeschlagen: {e}");
                self.depth.mask_for = None;
                if e != "abgebrochen" {
                    self.depth.failed.insert(path, e.clone());
                } else {
                    self.depth.cancelled.insert(path);
                }
            }
        }
        true
    }

    /// Focus on what is probably the subject: the nearest tenth of the image.
    fn focus_on_subject(&mut self) {
        let Some(path) = self.current_path() else { return };
        let Some(map) = sk_core::depth::load(&path) else { return };
        let focus = (map.quantile(0.9) * 100.0).round() / 100.0;
        self.edit(|a| a.lens_blur.focus = focus);
    }

    /// The focus pipette on or off.
    pub fn toggle_focus_picker(&mut self) {
        let on = !self.picking_focus;
        self.end_viewer_tools();
        self.picking_focus = on;
    }

    /// Focus on the depth at `image_pos` (displayed preview pixels).
    pub fn pick_focus(&mut self, image_pos: Vec2) {
        self.picking_focus = false;
        let Some(cur) = &self.current else { return };
        let Some(map) = sk_core::depth::load(&cur.meta.path) else {
            self.status = "Fokus: Die Tiefenkarte wird noch berechnet".into();
            return;
        };
        let p = &cur.preview;
        let opts = RenderOptions { ignore_crop: self.crop_tool.is_some(), ..Default::default() };
        let geo = Processor::with_options(&cur.meta, &cur.adjustments, opts).geomap(p.width, p.height);
        let (sx, sy) = geo.source_point(image_pos.x, image_pos.y);
        let (u, v) = (sx / p.width as f32, sy / p.height as f32);
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return;
        }
        // The middle of a small neighborhood: a click next to a thin edge still hits the object.
        let mut around: Vec<f32> = (-2..=2)
            .flat_map(|dy| (-2..=2).map(move |dx| (dx, dy)))
            .map(|(dx, dy)| map.sample(u + dx as f32 * 0.002, v + dy as f32 * 0.002))
            .collect();
        around.sort_by(f32::total_cmp);
        let focus = (around[around.len() / 2] * 100.0).round() / 100.0;
        self.edit(|a| a.lens_blur.focus = focus);
        self.status = format!("Fokus: Tiefe {:.0} %", focus * 100.0);
    }

    /// The "Objektivunschärfe" section of the Details panel.
    pub(crate) fn lens_blur_section(&mut self, ui: &mut Ui) {
        let Some(cur) = &self.current else { return };
        let path = cur.meta.path.clone();
        let mut lb = cur.adjustments.lens_blur;
        let before = lb;
        let mut reset = false;
        let mut toggle_pick = false;
        let picking = self.picking_focus;
        let state = self.depth_state(&path);
        let help = "Zeichnet Vorder- und Hintergrund weich wie eine offene Blende. Wie nah jede Stelle ist, schätzt ein neuronales Netz einmal je Bild; die Tiefenkarte wird im Hintergrund berechnet und zwischengespeichert.\n\nFokus: Pipette oder Regler (0 ist der fernste, 1 der nächste Teil des Bildes). Schärfebereich: wie weit davor und dahinter es scharf bleibt. Lichter: helle Stellen werden in der Unschärfe zu leuchtenden Scheiben.";
        crate::widgets::tool_section_help(ui, sk_core::tools::Tool::LensBlur, help, Some(&mut || reset = true), |ui| {
            let d = LensBlur::default();
            slider_row(ui, "Stärke", &mut lb.amount, 0.0..=100.0, 0.0, 0);
            ui.horizontal(|ui| {
                let pipette = format!("{}  Fokus setzen", crate::theme::icon::EYEDROPPER);
                if ui.selectable_label(picking, pipette).on_hover_text("Auf die Stelle klicken, die scharf bleiben soll").clicked() {
                    toggle_pick = true;
                }
            });
            slider_row(ui, "Fokus", &mut lb.focus, 0.0..=1.0, d.focus, 2);
            slider_row(ui, "Schärfebereich", &mut lb.range, 0.0..=1.0, d.range, 2);
            slider_row(ui, "Lichter", &mut lb.bokeh, 0.0..=100.0, 0.0, 0);
            let faint = |t: &str| RichText::new(t).small().color(crate::theme::TEXT_FAINT);
            match &state {
                DepthState::NoModel => {
                    ui.label(faint(&format!("Lädt beim ersten Einschalten ein KI-Modell ({} MB)", AiFeature::Depth.download_mb())));
                }
                DepthState::Downloading(p) => ai_models::download_progress(ui, AiFeature::Depth, *p),
                _ if lb.is_off() => {}
                DepthState::Running(p) => {
                    ui.add(egui::ProgressBar::new(*p).desired_height(6.0));
                    ui.label(faint("Tiefenkarte wird berechnet …"));
                }
                DepthState::Waiting => {
                    ui.label(faint("Tiefenkarte wird gleich berechnet …"));
                }
                DepthState::Ready => {}
                DepthState::Cancelled => {
                    ui.label(faint("Abgebrochen – zum Berechnen einen Regler ändern"));
                }
                DepthState::Failed(e) => {
                    ui.label(RichText::new(format!("Fehlgeschlagen: {e}")).small().color(crate::theme::DANGER));
                }
            }
        });
        if reset {
            lb = LensBlur::default();
        }
        if toggle_pick {
            self.toggle_focus_picker();
        }
        if lb != before {
            self.depth.cancelled.remove(&path);
            // Switched on for the first time: focus on the subject (once the map exists). A focus
            // chosen before stays, also after the amount was at 0 for a while.
            if before.is_off() && !lb.is_off() && lb.focus == LensBlur::default().focus {
                if sk_core::depth::is_ready(&path) {
                    if let Some(map) = sk_core::depth::load(&path) {
                        lb.focus = (map.quantile(0.9) * 100.0).round() / 100.0;
                    }
                } else {
                    self.depth.autofocus = Some(path.clone());
                }
            }
            self.edit(|a| a.lens_blur = lb);
        }
    }

    /// Gives a layer the mask of a depth range (`None`: the nearest part of the image) once the
    /// image's depth map exists; computes the map first if needed.
    pub(crate) fn set_depth_mask(&mut self, layer: usize, range: Option<DepthRange>) {
        let Some(path) = self.current_path() else { return };
        let Some(map) = sk_core::depth::load(&path) else {
            if !self.ensure_ai(AiFeature::Depth) {
                self.status = format!("Tiefe auswählen: KI-Modell wird geladen ({} MB, einmalig) …", AiFeature::Depth.download_mb());
            } else {
                self.status = "Tiefe auswählen: Tiefenkarte wird berechnet …".into();
            }
            self.depth.failed.remove(&path);
            self.depth.cancelled.remove(&path);
            self.depth.mask_for = self.layer_id(layer).map(|id| (path, id));
            return;
        };
        // By default the nearest fifth of the image (usually the subject).
        let range = range.unwrap_or_else(|| DepthRange { near: 1.0, far: (map.quantile(0.8) * 100.0).round() / 100.0, ..Default::default() });
        let small = map.resized(MASK_EDGE);
        self.edit(|a| {
            if let Some(l) = a.layers.get_mut(layer) {
                l.mask.ai = Some(small.range_mask(&range));
            }
        });
    }

    /// Range sliders of a layer's depth mask; returns the changed range.
    pub(crate) fn depth_mask_controls(ui: &mut Ui, range: &DepthRange) -> Option<DepthRange> {
        let mut r = *range;
        let d = DepthRange::default();
        let mut changed = false;
        ui.label(RichText::new("Tiefe (0 fern, 1 nah)").strong());
        changed |= slider_row(ui, "Nah", &mut r.near, 0.0..=1.0, d.near, 2);
        changed |= slider_row(ui, "Fern", &mut r.far, 0.0..=1.0, d.far, 2);
        changed |= slider_row(ui, "Weichheit", &mut r.falloff, 0.0..=0.5, d.falloff, 2);
        changed.then_some(r)
    }
}
