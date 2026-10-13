//! KI-Entrauschen (see `sk_core::ai_denoise` and `sk_ai::denoise`): the amount is an ordinary,
//! undoable adjustment; the denoised data of each image is computed once in the background
//! (the image in the viewer first) and kept in a cache. The models are GPL-3.0 and downloaded
//! only after the user has seen their terms.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use egui::{Button, Color32, RichText, vec2};
use sk_ai::models::{self, ModelInfo};
use sk_core::params::Detail;

use crate::app::App;
use crate::theme::icon;
use crate::widgets::{image_count, slider_row};

/// Title of the section in the Details panel.
pub const NOISE_SECTION: &str = "Rauschen";

const JOB_LABEL: &str = "KI-Entrauschen";
pub(crate) const DOWNLOAD_LABEL: &str = "Modell laden: KI-Entrauschen (RawNIND)";

/// The dialog: download the models, with their terms.
pub struct DenoiseDialog {
    pub accepted: bool,
}

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
}

/// What the Details panel shows below the amount.
pub enum DenoiseState {
    NoModel,
    Downloading(f32),
    Waiting,
    Running(f32),
    Ready,
    Failed(String),
}

impl App {
    /// Command: shows the noise section of the Details panel.
    pub fn open_denoise(&mut self, ctx: &egui::Context) {
        self.tab = crate::app::Tab::Details;
        crate::widgets::reveal_section(ctx, NOISE_SECTION);
    }

    /// The model download, with its terms.
    pub fn open_denoise_download(&mut self) {
        self.denoise_dialog = Some(DenoiseDialog { accepted: false });
    }

    /// Downloads a model in the background (one job per model).
    pub fn download_model_job(&mut self, m: &'static ModelInfo) {
        let label = format!("Modell laden: {}", m.label);
        if self.jobs.iter().any(|j| j.label == label) {
            return;
        }
        self.start_job(&label, Box::new(move |p| {
            models::download(m, p)?;
            Ok(Vec::new())
        }));
    }

    /// Progress of a running model download.
    pub fn model_download_progress(&self, m: &ModelInfo) -> Option<f32> {
        let label = format!("Modell laden: {}", m.label);
        self.jobs.iter().find(|j| j.label == label).map(|j| j.progress)
    }

    fn download_denoise_models(&mut self) {
        if self.jobs.iter().any(|j| j.label == DOWNLOAD_LABEL) {
            return;
        }
        self.start_job(DOWNLOAD_LABEL, Box::new(|p| {
            sk_ai::denoise::download(p)?;
            Ok(Vec::new())
        }));
    }

    /// Queues the computation of an image's denoised data (`first`: before all others).
    pub(crate) fn want_denoise(&mut self, path: &Path, first: bool) {
        let q = &mut self.denoise;
        if q.failed.contains_key(path) || q.running.as_ref().is_some_and(|(_, p)| p == path) {
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
        if self.denoise.running.is_some() || !sk_ai::denoise::available() {
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
                if e != "abgebrochen" {
                    self.denoise.failed.insert(path, e.clone());
                } else {
                    // Cancelled by the user: the rest of the queue too.
                    self.denoise.pending.clear();
                    self.denoise.checked = None;
                }
            }
        }
        true
    }

    /// State of an image's denoised data, for the Details panel.
    pub fn denoise_state(&self, path: &Path) -> DenoiseState {
        if let Some(p) = self.jobs.iter().find(|j| j.label == DOWNLOAD_LABEL).map(|j| j.progress) {
            return DenoiseState::Downloading(p);
        }
        if !sk_ai::denoise::available() {
            return DenoiseState::NoModel;
        }
        if let Some(e) = self.denoise.failed.get(path) {
            return DenoiseState::Failed(e.clone());
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
    /// reduction, which can be carried over to the other selected images; returns whether
    /// something changed.
    pub(crate) fn noise_section(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(cur) = self.current.as_ref() else { return false };
        let (path, hdr) = (cur.meta.path.clone(), cur.meta.hdr);
        let mut d = cur.adjustments.detail.clone();
        let defaults = Detail::default();
        let n = self.selected_paths().len();
        let (mut reset, mut transfer, mut changed) = (false, false, false);
        let help = "KI-Entrauschen: ein neuronales Netz säubert die RAW-Daten, einmal je Bild im Hintergrund; das Original bleibt unverändert.\n\nLuminanz und Farbe: die klassische Rauschreduzierung, auch zusätzlich.";
        crate::widgets::section_help(ui, NOISE_SECTION, help, Some(&mut || reset = true), |ui| {
            changed |= self.denoise_controls(ui, &mut d.ai_denoise, &path, hdr);
            changed |= slider_row(ui, "Luminanz", &mut d.nr_luma, 0.0..=100.0, defaults.nr_luma, 0);
            changed |= slider_row(ui, "Farbe", &mut d.nr_color, 0.0..=100.0, defaults.nr_color, 0);
            if n > 1 {
                ui.add_space(4.0);
                let hover = "KI-Entrauschen, Luminanz und Farbe dieses Bildes für alle ausgewählten Bilder übernehmen";
                transfer = ui.button(format!("Auf {} übertragen", image_count(n))).on_hover_text(hover).clicked();
            }
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
        if transfer {
            self.transfer_noise(noise);
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
        if ai > 0.0 && sk_ai::denoise::available() {
            for f in &files {
                self.want_denoise(f, false);
            }
        }
        self.status = format!("Rauschen übertragen: {}", image_count(files.len()));
    }

    /// The KI-Entrauschen amount and the state of its computation; returns whether it changed.
    fn denoise_controls(&mut self, ui: &mut egui::Ui, amount: &mut f32, path: &Path, hdr: bool) -> bool {
        let before = *amount;
        let changed = slider_row(ui, "KI-Entrauschen", amount, 0.0..=100.0, 0.0, 0);
        let faint = |t: String| RichText::new(t).small().color(crate::theme::TEXT_FAINT);
        if hdr {
            ui.label(faint("Für zusammengesetzte HDR-Bilder nicht verfügbar".into()));
            return changed;
        }
        match self.denoise_state(path) {
            DenoiseState::NoModel => {
                let m = &sk_ai::models::DENOISE_BAYER;
                if crate::widgets::model_download(ui, "RawNIND", sk_ai::denoise::download_mb(), m.license, None)
                    || (changed && before == 0.0 && *amount > 0.0)
                {
                    self.open_denoise_download();
                }
            }
            DenoiseState::Downloading(p) => {
                crate::widgets::model_download(ui, "", 0, "", Some(p));
            }
            _ if *amount <= 0.0 => {}
            DenoiseState::Waiting => {
                ui.label(faint("Wird gleich berechnet …".into()));
            }
            DenoiseState::Running(p) => {
                ui.add(egui::ProgressBar::new(p).desired_height(6.0));
                ui.label(faint(format!("Wird berechnet … {:.0} %", p * 100.0)));
            }
            DenoiseState::Ready => {}
            DenoiseState::Failed(e) => {
                ui.label(RichText::new(format!("Fehlgeschlagen: {e}")).small().color(crate::theme::DANGER));
            }
        }
        changed
    }

    pub(crate) fn denoise_dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.denoise_dialog else { return };
        let (mut close, mut download, mut show_gpu) = (false, false, false);
        let modal = egui::Modal::new(egui::Id::new("denoise")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(RichText::new(format!("{}  KI-Entrauschen: Modell laden", icon::SPARKLE)).size(17.0).strong().color(Color32::WHITE));
            ui.add_space(6.0);
            ui.label(
                "Ein neuronales Netz, trainiert auf echten Aufnahmepaaren mit und ohne Rauschen, säubert die RAW-Daten \
                 und rechnet sie dabei selbst in Farbe um. Das Original bleibt unverändert; die Stärke steht unter \
                 Details › Rauschen.",
            );
            let on_gpu = sk_ai::runtime::gpu_enabled() && sk_ai::runtime::gpu_installed();
            let where_ = if on_gpu {
                "Die Berechnung läuft einmal je Bild im Hintergrund auf der Grafikkarte."
            } else {
                "Die Berechnung läuft einmal je Bild im Hintergrund auf dem Prozessor (etwa eine halbe Minute für 24 Megapixel)."
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(where_).small().color(Color32::from_gray(150)));
                if !on_gpu && sk_ai::runtime::gpu_supported() && ui.small_button(crate::gpu::DIALOG_TITLE).clicked() {
                    show_gpu = true;
                }
            });
            ui.add_space(10.0);
            ui.label(RichText::new("Modell und Lizenz").strong());
            ui.label(format!(
                "Verwendet wird RawNIND (UtNet2) von Benoit Brummer (UCLouvain), wie es darktable veröffentlicht. \
                 Das Modell ist nicht Teil von Silberkorn: es wird auf Ihren Wunsch von GitHub (darktable-ai) \
                 heruntergeladen (etwa {} MB) und nur auf diesem Computer verwendet.",
                sk_ai::denoise::download_mb()
            ));
            ui.label(
                "Die Gewichte stehen unter der GNU General Public License 3.0. Wer die Modelldateien weitergibt, \
                 muss deren Bedingungen einhalten. Die Trainingsbilder stehen unter CC BY 4.0 und CC0.",
            );
            ui.hyperlink_to("Lizenztext (GPL 3.0)", "https://www.gnu.org/licenses/gpl-3.0.html");
            ui.checkbox(&mut dialog.accepted, "Ich habe die Lizenzbedingungen gelesen");
            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let load = Button::new(format!("{}  Modell laden", icon::DOWNLOAD_SIMPLE)).min_size(vec2(110.0, 26.0));
                if ui.add_enabled(dialog.accepted, load).clicked() {
                    download = true;
                }
                if ui.add(Button::new("Abbrechen").min_size(vec2(90.0, 26.0))).clicked() {
                    close = true;
                }
            });
        });
        if show_gpu {
            self.show_gpu_dialog = true;
        }
        if download {
            self.download_denoise_models();
        }
        if download || close || modal.should_close() {
            self.denoise_dialog = None;
        }
    }

    /// The cache of denoised data (in the dialog for the graphics card and the AI models).
    pub(crate) fn denoise_cache_ui(&mut self, ui: &mut egui::Ui) {
        let used = sk_core::ai_denoise::cache_size();
        let mut clear = false;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!(
                "Zwischenspeicher KI-Entrauschen: {:.1} GB (höchstens {} GB, die ältesten fallen heraus)",
                used as f64 / 1e9,
                sk_core::ai_denoise::CACHE_LIMIT >> 30
            ))
            .small()
            .color(Color32::from_gray(150)));
            if used > 0 && self.denoise.running.is_none() && ui.small_button("Leeren").clicked() {
                clear = true;
            }
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
