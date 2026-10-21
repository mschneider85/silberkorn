//! Depth maps (see `sk_core::depth` and `sk_ai::depth`): computed in the background for the image
//! in the viewer when its lens blur, relight, atmosphere or a depth mask needs one, and kept in a
//! cache. Here also the sections of those tools, the focus pipette, and the depth mask of a layer.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Color32, RichText, Ui, Vec2};
use sk_core::depth::DepthMap;
use sk_core::params::{Atmosphere, DepthRange, LensBlur, Relight};

use crate::ai_models::{self, AiFeature};
use crate::app::App;
use crate::theme::icon;
use crate::widgets::{RAIL_KELVIN, RAIL_TONE, SliderOpts, slider_row, slider_row_with};

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
    /// Whether the viewer image's map exists (the panels ask every frame).
    ready: ReadyCache,
    /// The viewer image's map at mask size, with the "Fern" of a new depth mask: the depth mask
    /// sliders reuse it, so it is neither resized nor its range masks remade per frame.
    small: Option<(PathBuf, Arc<DepthMap>, f32)>,
    /// How the depths of that map are distributed (256 bins, far to near), for the range editor.
    small_bins: Option<Arc<Vec<f32>>>,
}

/// Whether an image's computed file exists, looked up on disk at most once a second (panels ask
/// every frame, the lookup reads file metadata); `forget` when a computation finished.
#[derive(Default)]
pub(crate) struct ReadyCache(RefCell<Option<(PathBuf, bool, Instant)>>);

impl ReadyCache {
    const FRESH: Duration = Duration::from_secs(1);

    pub(crate) fn get(&self, path: &Path, check: impl FnOnce(&Path) -> bool) -> bool {
        if let Some((p, ready, at)) = &*self.0.borrow()
            && p == path
            && at.elapsed() < Self::FRESH
        {
            return *ready;
        }
        let ready = check(path);
        *self.0.borrow_mut() = Some((path.to_path_buf(), ready, Instant::now()));
        ready
    }

    pub(crate) fn forget(&self) {
        self.0.take();
    }
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
        if self.depth_ready(path) {
            DepthState::Ready
        } else if self.depth.cancelled.contains(path) {
            DepthState::Cancelled
        } else {
            DepthState::Waiting
        }
    }

    /// The image's map exists (see [`ReadyCache`]).
    fn depth_ready(&self, path: &Path) -> bool {
        self.depth.ready.get(path, sk_core::depth::is_ready)
    }

    /// The image's map at mask size and the "Fern" of a new depth mask (the nearest fifth of the
    /// image), kept for the image in the viewer.
    pub(crate) fn depth_mask_map(&mut self, path: &Path) -> Option<(Arc<DepthMap>, f32)> {
        if let Some((p, small, far)) = &self.depth.small
            && p == path
        {
            return Some((small.clone(), *far));
        }
        let map = sk_core::depth::load(path)?;
        let far = (map.quantile(0.8) * 100.0).round() / 100.0;
        let small = Arc::new(map.resized(MASK_EDGE));
        let mut bins = vec![0.0f32; 256];
        for d in &small.data {
            bins[((d.clamp(0.0, 1.0) * 255.0).round()) as usize] += 1.0;
        }
        self.depth.small_bins = Some(Arc::new(bins));
        self.depth.small = Some((path.to_path_buf(), small.clone(), far));
        Some((small, far))
    }

    /// Each frame: starts computing the viewer image's map when something needs it.
    pub(crate) fn pump_depth(&mut self) {
        if self.depth.running.is_some() {
            return;
        }
        let Some(cur) = &self.current else { return };
        let path = cur.meta.path.clone();
        let wanted = cur.adjustments.needs_depth() || self.depth.mask_for.as_ref().is_some_and(|(p, _)| *p == path);
        if !wanted || self.depth.failed.contains_key(&path) || self.depth.cancelled.contains(&path) || self.depth_ready(&path) || !self.ensure_ai(AiFeature::Depth) {
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
        self.depth.ready.forget();
        self.depth.small.take_if(|(p, ..)| *p == path);
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
                self.report_error(format!("{JOB_LABEL} für {name} fehlgeschlagen: {e}"));
                self.depth.mask_for = None;
                if e != crate::jobs::CANCELLED {
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
            self.status = "Fokus: die Tiefenkarte wird noch berechnet".into();
            return;
        };
        let p = &cur.preview;
        let geo = sk_core::pipeline::geomap(&cur.meta, &cur.adjustments, self.crop_tool.is_some(), p.width, p.height);
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
        let cancel = self.depth_cancel();
        let help = "Zeichnet Vorder- und Hintergrund weich wie eine offene Blende.\n\nFokus: Pipette oder Regler (0 fern, 1 nah). Schärfebereich: wie viel davor und dahinter scharf bleibt. Lichter: helle Stellen werden zu leuchtenden Scheiben.";
        crate::widgets::tool_section_help(ui, sk_core::tools::Tool::LensBlur, help, Some(&mut || reset = true), |ui| {
            let d = LensBlur::default();
            crate::widgets::percent_row(ui, "Stärke", &mut lb.amount, 0.0..=100.0, 0.0);
            ui.add_enabled_ui(!lb.is_off(), |ui| {
                ui.horizontal(|ui| {
                    let pipette = format!("{}  Fokus setzen", crate::theme::icon::EYEDROPPER);
                    if ui.add(egui::Button::new(pipette).selected(picking)).on_hover_text("Auf die Stelle klicken, die scharf bleiben soll").clicked() {
                        toggle_pick = true;
                    }
                });
                crate::widgets::unit_percent_row(ui, "Fokus", &mut lb.focus, 0.0..=1.0, d.focus, Default::default());
                crate::widgets::unit_percent_row(ui, "Schärfebereich", &mut lb.range, 0.0..=1.0, d.range, Default::default());
                slider_row(ui, "Lichter", &mut lb.bokeh, 0.0..=100.0, 0.0, 0);
            });
            depth_status(ui, &state, !lb.is_off(), cancel.as_ref());
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
                if self.depth_ready(&path) {
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

    /// The "Neu ausleuchten" section of the exposure panel.
    pub(crate) fn relight_section(&mut self, ui: &mut Ui) {
        let Some(cur) = &self.current else { return };
        let path = cur.meta.path.clone();
        let mut rl = cur.adjustments.relight;
        let before = rl;
        let mut reset = false;
        let state = self.depth_state(&path);
        let cancel = self.depth_cancel();
        let help = "Hellt Vorder- und Hintergrund getrennt auf oder ab und färbt sie wärmer oder kühler, etwa bei Gegenlicht.\n\nGrenze: wo der Vordergrund aufhört (0 fern, 1 nah). Übergang: wie weich er in den Hintergrund übergeht.";
        crate::widgets::tool_section_help(ui, sk_core::tools::Tool::Relight, help, Some(&mut || reset = true), |ui| {
            let d = Relight::default();
            ui.add_space(4.0);
            relight_card(ui, icon::PERSON_SIMPLE, "Vordergrund", NEAR_COLOR, &mut rl.near, &mut rl.near_warmth);
            ui.add_space(6.0);
            relight_card(ui, icon::MOUNTAINS, "Hintergrund", FAR_COLOR, &mut rl.far, &mut rl.far_warmth);
            ui.add_space(4.0);
            ui.add_enabled_ui(!rl.is_off(), |ui| {
                crate::widgets::subheading(ui, "Trennung");
                crate::widgets::unit_percent_row(ui, "Grenze", &mut rl.split, 0.0..=1.0, d.split, Default::default());
                crate::widgets::unit_percent_row(ui, "Übergang", &mut rl.falloff, 0.02..=1.0, d.falloff, Default::default());
            });
            depth_status(ui, &state, !rl.is_off(), cancel.as_ref());
        });
        if reset {
            rl = Relight::default();
        }
        if rl != before {
            self.depth.cancelled.remove(&path);
            self.edit(|a| a.relight = rl);
        }
    }

    /// The "Atmosphäre" section of the details panel.
    pub(crate) fn atmosphere_section(&mut self, ui: &mut Ui) {
        let Some(cur) = &self.current else { return };
        let path = cur.meta.path.clone();
        let mut at = cur.adjustments.atmosphere;
        let before = at;
        let mut reset = false;
        let state = self.depth_state(&path);
        let cancel = self.depth_cancel();
        let help = "Legt Dunst über das Bild, der mit der Entfernung dichter wird. Seine Farbe kommt aus der Ferne des Bildes.\n\nReichweite: wie weit nach vorn er reicht. Helligkeit und Wärme verändern seine Farbe.";
        crate::widgets::tool_section_help(ui, sk_core::tools::Tool::Atmosphere, help, Some(&mut || reset = true), |ui| {
            let d = Atmosphere::default();
            crate::widgets::percent_row(ui, "Stärke", &mut at.amount, 0.0..=100.0, 0.0);
            ui.add_enabled_ui(!at.is_off(), |ui| {
                crate::widgets::unit_percent_row(ui, "Reichweite", &mut at.reach, 0.02..=1.0, d.reach, Default::default());
                slider_row_with(ui, "Helligkeit", &mut at.brightness, -100.0..=100.0, 0.0, 0, SliderOpts { rail: Some(RAIL_TONE), ..Default::default() });
                slider_row_with(ui, "Wärme", &mut at.warmth, -100.0..=100.0, 0.0, 0, SliderOpts { rail: Some(RAIL_KELVIN), ..Default::default() });
            });
            depth_status(ui, &state, !at.is_off(), cancel.as_ref());
        });
        if reset {
            at = Atmosphere::default();
        }
        if at != before {
            self.depth.cancelled.remove(&path);
            self.edit(|a| a.atmosphere = at);
        }
    }

    /// Gives a layer the mask of a depth range (`None`: the nearest part of the image) once the
    /// image's depth map exists; computes the map first if needed.
    pub(crate) fn set_depth_mask(&mut self, layer: usize, range: Option<DepthRange>) {
        let Some(path) = self.current_path() else { return };
        let Some((small, far)) = self.depth_mask_map(&path) else {
            if !self.ensure_ai(AiFeature::Depth) {
                self.status = format!("Tiefe auswählen: lade KI-Modell ({} MB, einmalig) …", AiFeature::Depth.download_mb());
            } else {
                self.status = "Tiefe auswählen: berechne Tiefenkarte …".into();
            }
            self.depth.failed.remove(&path);
            self.depth.cancelled.remove(&path);
            self.depth.mask_for = self.layer_id(layer).map(|id| (path, id));
            return;
        };
        // By default the nearest fifth of the image (usually the subject).
        let range = range.unwrap_or(DepthRange::nearer_than(far));
        let mask = sk_core::depth::range_mask_cached(&small, &range);
        self.edit(|a| {
            if let Some(l) = a.layers.get_mut(layer) {
                l.mask.ai = Some(mask);
            }
        });
    }

    /// How the depths of the viewer image's map are distributed (see `depth_mask_map`).
    pub(crate) fn depth_mask_bins(&self) -> Option<Arc<Vec<f32>>> {
        self.depth.small_bins.clone().filter(|_| self.depth.small.is_some())
    }

    /// The range of a layer's depth mask as a trapezoid over the image's depths, far (cool, left)
    /// to near (warm, right); returns the changed range. Resetting gives the range of a new
    /// depth mask on this image (everything nearer than `far`).
    pub(crate) fn depth_mask_controls(ui: &mut Ui, range: &DepthRange, far: f32, bins: Option<&[f32]>) -> Option<DepthRange> {
        let d = DepthRange::nearer_than(far);
        let scale = crate::widgets::RangeScale {
            defaults: [d.falloff_far, d.far, d.near, d.falloff_near],
            colors: [FAR_COLOR, NEAR_COLOR],
            percent: true,
            hint: "Ziehen: Bereich verschieben – Doppelklick: der nahe Teil des Bildes",
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new("Tiefe").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new("fern 0 % … 100 % nah").small().color(crate::theme::TEXT_FAINT));
            });
        });
        let mut e = [range.falloff_far, range.far, range.near, range.falloff_near];
        let changed = crate::widgets::range_editor(ui, "depth-range", &mut e, bins, &scale);
        let [falloff_far, far, near, falloff_near] = e;
        changed.then_some(DepthRange { falloff_far, far, near, falloff_near })
    }
}

/// Progress of the depth map below a tool that uses it (`active`: the tool is switched on).
/// Marks of the near and the far part of "Neu ausleuchten": warm near, cool and hazy far, as in
/// a landscape painting.
const NEAR_COLOR: Color32 = Color32::from_rgb(232, 170, 92);
const FAR_COLOR: Color32 = Color32::from_rgb(118, 150, 220);

/// One part of "Neu ausleuchten": a card with a colored edge, its name and its two sliders.
fn relight_card(ui: &mut Ui, symbol: &str, title: &str, color: Color32, ev: &mut f32, warmth: &mut f32) {
    let active = *ev != 0.0 || *warmth != 0.0;
    let fill = if active { color.gamma_multiply(0.10) } else { Color32::from_white_alpha(6) };
    let frame = egui::Frame::new().fill(fill).corner_radius(6).inner_margin(egui::Margin { left: 12, right: 4, top: 6, bottom: 4 });
    let resp = frame.show(ui, |ui| {
        ui.label(RichText::new(format!("{symbol}  {title}")).size(12.5).strong().color(if active { color } else { color.gamma_multiply(0.8) }));
        ui.add_space(2.0);
        slider_row_with(ui, "Belichtung", ev, -2.0..=2.0, 0.0, 2, SliderOpts { suffix: " EV", rail: Some(RAIL_TONE), ..Default::default() });
        let opts = SliderOpts { rail: Some(RAIL_KELVIN), ..Default::default() };
        slider_row_with(ui, "Wärme", warmth, -100.0..=100.0, 0.0, 0, opts);
    });
    let r = resp.response.rect;
    let edge = egui::Rect::from_min_max(r.left_top(), egui::pos2(r.left() + 3.0, r.bottom()));
    ui.painter().rect_filled(edge, egui::CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 }, color);
}

impl App {
    /// The cancel switch of the depth map being computed or its model being downloaded.
    fn depth_cancel(&self) -> Option<std::sync::Arc<std::sync::atomic::AtomicBool>> {
        self.job_cancel(self.depth.running.as_ref().map(|r| r.0), Some(&AiFeature::Depth.job_label()))
    }
}

fn depth_status(ui: &mut Ui, state: &DepthState, active: bool, cancel: Option<&std::sync::Arc<std::sync::atomic::AtomicBool>>) {
    let faint = |t: &str| RichText::new(t).small().color(crate::theme::TEXT_FAINT);
    match state {
        DepthState::NoModel => {
            ui.label(faint(&format!("Lädt beim ersten Einschalten ein KI-Modell ({} MB)", AiFeature::Depth.download_mb())));
        }
        DepthState::Downloading(p) => ai_models::download_progress(ui, AiFeature::Depth, *p, cancel.cloned()),
        _ if !active => {}
        DepthState::Running(p) => {
            ui.add(egui::ProgressBar::new(*p).desired_height(6.0));
            ui.horizontal(|ui| {
                ui.label(faint("Berechne Tiefenkarte …"));
                if let Some(c) = cancel {
                    crate::widgets::cancel_button(ui, c);
                }
            });
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
}
