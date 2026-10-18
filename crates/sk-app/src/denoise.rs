//! KI-Entrauschen (see `sk_core::ai_denoise` and `sk_ai::denoise`): the amount is an ordinary,
//! undoable adjustment; the denoised data of each image is computed once in the background
//! (the image in the viewer first) and kept in a cache. Switching it on downloads the models
//! (GPL-3.0, credited in the licenses dialog) the first time.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use egui::RichText;
use sk_core::params::Detail;

use crate::ai_models::AiFeature;
use crate::app::App;
use crate::widgets::{image_count, slider_row};

/// Title of the section in the Details panel.
pub const NOISE_SECTION: &str = "Rauschen";

const JOB_LABEL: &str = "KI-Entrauschen";
/// Images waiting for their denoised data.
#[derive(Default)]
pub struct DenoiseQueue {
    pending: VecDeque<PathBuf>,
    /// Job id and image being computed.
    running: Option<(u64, PathBuf)>,
    /// Why an image could not be denoised (not tried again until the app restarts).
    failed: HashMap<PathBuf, String>,
    /// The viewer's image was checked with this amount being on (avoids a file check per frame).
    checked: Option<PathBuf>,
    /// Cancelled by the user: not started again until its amount changes or it is transferred.
    cancelled: HashSet<PathBuf>,
}

/// What the Details panel shows below the amount.
pub enum DenoiseState {
    NoModel,
    Downloading(f32),
    Waiting,
    Running(f32),
    Ready,
    Cancelled,
    Failed(String),
}

impl App {
    /// Command: shows the noise section of the Details panel.
    pub fn open_denoise(&mut self, ctx: &egui::Context) {
        self.tab = crate::app::Tab::Details;
        crate::widgets::reveal_section(ctx, NOISE_SECTION);
    }

    /// Queues the computation of an image's denoised data (`first`: before all others).
    pub(crate) fn want_denoise(&mut self, path: &Path, first: bool) {
        let q = &mut self.denoise;
        if q.failed.contains_key(path) || q.cancelled.contains(path) || q.running.as_ref().is_some_and(|(_, p)| p == path) {
            return;
        }
        if let Some(i) = q.pending.iter().position(|p| p == path) {
            if !first {
                return;
            }
            q.pending.remove(i);
        }
        if sk_core::ai_denoise::is_ready(path) {
            return;
        }
        if first { q.pending.push_front(path.to_path_buf()) } else { q.pending.push_back(path.to_path_buf()) }
    }

    /// Each frame: queues the viewer's image if it wants denoising, starts the next computation.
    pub(crate) fn pump_denoise(&mut self) {
        let wanted = self.current.as_ref().filter(|c| c.adjustments.detail.ai_denoise > 0.0 && !c.meta.hdr).map(|c| c.meta.path.clone());
        if wanted != self.denoise.checked {
            self.denoise.checked = wanted.clone();
            if let Some(p) = wanted {
                self.want_denoise(&p, true);
            }
        }
        if self.denoise.running.is_some() || self.denoise.pending.is_empty() || !self.ensure_ai(AiFeature::Denoise) {
            return;
        }
        let Some(path) = self.denoise.pending.pop_front() else { return };
        let label = format!("{JOB_LABEL}: {}", path.file_name().unwrap_or_default().to_string_lossy());
        let file = path.clone();
        self.start_job(&label, Box::new(move |p| {
            sk_core::ai_denoise::ensure(&file, p)?;
            Ok(Vec::new())
        }));
        let id = self.jobs.last().map_or(0, |j| j.id);
        self.denoise.running = Some((id, path));
    }

    /// The model download was cancelled: like cancelling the computation of the images waiting.
    pub(crate) fn denoise_download_cancelled(&mut self) {
        self.denoise.cancelled.extend(self.denoise.pending.drain(..));
    }

    /// A computation finished (`true` if the job was one).
    pub(crate) fn denoise_job_done(&mut self, id: u64, result: &Result<Vec<PathBuf>, String>) -> bool {
        if self.denoise.running.as_ref().is_none_or(|(i, _)| *i != id) {
            return false;
        }
        let (_, path) = self.denoise.running.take().unwrap();
        self.jobs.retain(|j| j.id != id);
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        match result {
            Ok(_) => {
                self.status = format!("{JOB_LABEL}: {name} fertig");
                // Renderings of the image so far were made without.
                self.thumb_requested.remove(&path);
                self.compare_textures.remove(&path);
                self.compare_requested.remove(&path);
                self.compare_details.remove(&path);
                self.forget_prepared(&path);
                if self.current_path().as_ref() == Some(&path) {
                    self.request_render();
                }
            }
            Err(e) => {
                self.status = format!("{JOB_LABEL} für {name} fehlgeschlagen: {e}");
                if e != crate::jobs::CANCELLED {
                    self.denoise.failed.insert(path, e.clone());
                } else {
                    // Cancelled by the user: the rest of the queue too; this image stays so.
                    self.denoise.pending.clear();
                    self.denoise.cancelled.insert(path);
                }
            }
        }
        true
    }

    /// State of an image's denoised data, for the Details panel.
    pub fn denoise_state(&self, path: &Path) -> DenoiseState {
        if let Some(p) = self.ai_download_progress(AiFeature::Denoise) {
            return DenoiseState::Downloading(p);
        }
        if !AiFeature::Denoise.available() {
            return DenoiseState::NoModel;
        }
        if let Some(e) = self.denoise.failed.get(path) {
            return DenoiseState::Failed(e.clone());
        }
        if self.denoise.cancelled.contains(path) {
            return DenoiseState::Cancelled;
        }
        if let Some((id, p)) = &self.denoise.running
            && p == path
        {
            return DenoiseState::Running(self.jobs.iter().find(|j| j.id == *id).map_or(0.0, |j| j.progress));
        }
        if self.denoise.pending.contains(&path.to_path_buf()) {
            return DenoiseState::Waiting;
        }
        if sk_core::ai_denoise::is_ready(path) { DenoiseState::Ready } else { DenoiseState::Waiting }
    }

    /// The "Rauschen" section of the Details panel: KI-Entrauschen and the classic noise
    /// reduction (carried over to the other selected images with the button in its header, see
    /// `App::transfer_tool`); returns whether something changed.
    pub(crate) fn noise_section(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(cur) = self.current.as_ref() else { return false };
        let (path, hdr) = (cur.meta.path.clone(), cur.meta.hdr);
        let mut d = cur.adjustments.detail.clone();
        let defaults = Detail::default();
        let (mut reset, mut changed) = (false, false);
        let help = "KI-Entrauschen säubert die RAW-Daten im Hintergrund; das Original bleibt unverändert.\n\nLuminanz und Farbe: klassische Rauschreduzierung, auch zusätzlich.";
        crate::widgets::tool_section_help(ui, sk_core::tools::Tool::NoiseReduction, help, Some(&mut || reset = true), |ui| {
            changed |= self.denoise_controls(ui, &mut d.ai_denoise, &path, hdr);
            changed |= slider_row(ui, "Luminanz", &mut d.nr_luma, 0.0..=100.0, defaults.nr_luma, 0);
            changed |= slider_row(ui, "Farbe", &mut d.nr_color, 0.0..=100.0, defaults.nr_color, 0);
        });
        if reset {
            (d.ai_denoise, d.nr_luma, d.nr_color) = (0.0, defaults.nr_luma, defaults.nr_color);
            changed = true;
        }
        let noise = (d.ai_denoise, d.nr_luma, d.nr_color);
        if let Some(cur) = self.current.as_mut() {
            let c = &mut cur.adjustments.detail;
            (c.ai_denoise, c.nr_luma, c.nr_color) = noise;
        }
        changed
    }

    /// Sets the noise settings on the selected images and queues their denoising.
    pub(crate) fn transfer_noise(&mut self, (ai, luma, color): (f32, f32, f32)) {
        self.commit_and_save();
        let files = self.selected_paths();
        self.set_adjustments_on(&files, |a| {
            let mut a = a.clone();
            (a.detail.ai_denoise, a.detail.nr_luma, a.detail.nr_color) = (ai, luma, color);
            a
        });
        self.commit_and_save();
        if ai > 0.0 {
            for f in &files {
                self.denoise.cancelled.remove(f);
                self.want_denoise(f, false);
            }
        }
        self.status = format!("Rauschen übertragen: {}", image_count(files.len()));
    }

    /// The KI-Entrauschen amount and the state of its computation; returns whether it changed.
    fn denoise_controls(&mut self, ui: &mut egui::Ui, amount: &mut f32, path: &Path, hdr: bool) -> bool {
        let before = *amount;
        // Merged HDR images can't be denoised: the slider is shown, but disabled.
        let opts = crate::widgets::SliderOpts { ai: true, ..Default::default() };
        let changed = ui.add_enabled_ui(!hdr, |ui| crate::widgets::slider_row_with(ui, "KI-Entrauschen", amount, 0.0..=100.0, 0.0, 0, opts)).inner;
        if changed && self.denoise.cancelled.remove(path) {
            // Queued again by `pump_denoise`.
            self.denoise.checked = None;
        }
        let faint = |t: String| RichText::new(t).small().color(crate::theme::TEXT_FAINT);
        if hdr {
            ui.label(faint("Für zusammengesetzte HDR-Bilder nicht verfügbar".into()));
            return changed;
        }
        match self.denoise_state(path) {
            DenoiseState::NoModel => {
                if changed && before == 0.0 && *amount > 0.0 {
                    self.ensure_ai(AiFeature::Denoise);
                } else {
                    ui.label(faint(format!("Lädt beim ersten Einschalten ein KI-Modell ({} MB)", AiFeature::Denoise.download_mb())));
                }
            }
            DenoiseState::Downloading(p) => crate::ai_models::download_progress(ui, AiFeature::Denoise, p),
            _ if *amount <= 0.0 => {}
            DenoiseState::Waiting => {
                ui.label(faint("Wird gleich berechnet …".into()));
            }
            DenoiseState::Running(p) => {
                ui.add(egui::ProgressBar::new(p).desired_height(6.0));
                ui.label(faint(format!("Wird berechnet … {:.0} %", p * 100.0)));
            }
            DenoiseState::Ready => {}
            DenoiseState::Cancelled => {
                ui.label(faint("Abgebrochen – zum Berechnen den Regler ändern".into()));
            }
            DenoiseState::Failed(e) => {
                ui.label(RichText::new(format!("Fehlgeschlagen: {e}")).small().color(crate::theme::DANGER));
            }
        }
        changed
    }

    /// The cache of denoised data (in the dialog for the graphics card): how full, and emptying it.
    pub(crate) fn denoise_cache_ui(&mut self, ui: &mut egui::Ui) {
        let used = sk_core::ai_denoise::cache_size();
        let limit = sk_core::ai_denoise::CACHE_LIMIT;
        let mut clear = false;
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.label(RichText::new(format!("KI-Entrauschen · {:.1} von {} GB", used as f64 / 1e9, limit >> 30)).size(12.0).color(crate::theme::TEXT));
                ui.add(egui::ProgressBar::new((used as f64 / limit as f64) as f32).desired_width(260.0).desired_height(4.0));
                crate::dialog::note(ui, "Die ältesten Ergebnisse fallen heraus, wenn er voll ist.");
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can = used > 0 && self.denoise.running.is_none();
                if ui.add_enabled(can, crate::dialog::secondary("Leeren")).clicked() {
                    clear = true;
                }
            });
        });
        if clear {
            match sk_core::ai_denoise::clear_cache() {
                Ok(()) => {
                    self.status = "KI-Entrauschen: Zwischenspeicher geleert".into();
                    // Images still set to denoise compute again when shown.
                    self.denoise.checked = None;
                    self.request_render();
                }
                Err(e) => self.status = format!("Zwischenspeicher nicht geleert: {e:#}"),
            }
        }
    }
}
