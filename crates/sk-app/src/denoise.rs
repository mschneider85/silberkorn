//! KI-Entrauschen (see `sk_core::ai_denoise` and `sk_ai::denoise`): the amount is an ordinary,
//! undoable adjustment; the denoised data of each image is computed once in the background
//! (the image in the viewer first) and kept in a cache. The models are GPL-3.0 and downloaded
//! only after the user has seen their terms.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use egui::{Button, Color32, RichText, vec2};
use sk_ai::models::{self, ModelInfo};

use crate::app::App;
use crate::theme::icon;

const JOB_LABEL: &str = "KI-Entrauschen";
const DOWNLOAD_LABEL: &str = "Modell laden: KI-Entrauschen (RawNIND)";

/// The dialog: download the models (with their terms) and, opened for a selection, apply the amount.
pub struct DenoiseDialog {
    /// Images to apply the amount to (empty: only the download).
    pub files: Vec<PathBuf>,
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
    /// Command: the amount for the selected images.
    pub fn open_denoise(&mut self) {
        self.commit_and_save();
        let files = self.selected_paths();
        if files.is_empty() {
            return;
        }
        self.denoise_dialog = Some(DenoiseDialog { files, accepted: false });
    }

    /// Only the model download (from the Details panel).
    pub fn open_denoise_download(&mut self) {
        self.denoise_dialog = Some(DenoiseDialog { files: Vec::new(), accepted: false });
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

    /// The "KI-Entrauschen" part of the Details panel; returns whether the amount changed.
    pub(crate) fn denoise_controls(&mut self, ui: &mut egui::Ui, amount: &mut f32, path: &Path, hdr: bool) -> bool {
        let before = *amount;
        let changed = crate::widgets::slider_row(ui, "Stärke", amount, 0.0..=100.0, 0.0, 0);
        let faint = |t: String| RichText::new(t).small().color(crate::theme::TEXT_FAINT);
        if hdr {
            ui.label(faint("Für zusammengesetzte HDR-Bilder nicht verfügbar".into()));
            return changed;
        }
        match self.denoise_state(path) {
            DenoiseState::NoModel => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(faint("Das Modell ist noch nicht geladen.".into()));
                    if ui.small_button("Modell laden …").clicked() {
                        self.open_denoise_download();
                    }
                });
                if changed && before == 0.0 && *amount > 0.0 {
                    self.open_denoise_download();
                }
            }
            DenoiseState::Downloading(p) => {
                ui.add(egui::ProgressBar::new(p).desired_height(6.0));
                ui.label(faint(format!("Modell wird geladen … {:.0} %", p * 100.0)));
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

    /// Sets the amount on the dialog's images and queues them.
    fn apply_denoise(&mut self, files: Vec<PathBuf>) {
        let amount = self.denoise_strength;
        self.save_settings();
        self.set_adjustments_on(&files, |a| {
            let mut a = a.clone();
            a.detail.ai_denoise = amount;
            a
        });
        self.commit_and_save();
        if amount > 0.0 {
            for f in &files {
                self.want_denoise(f, false);
            }
        }
        self.status = format!("{JOB_LABEL}: {}", crate::widgets::image_count(files.len()));
    }

    pub(crate) fn denoise_dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.denoise_dialog else { return };
        let n = dialog.files.len();
        let available = sk_ai::denoise::available();
        let downloading = self.jobs.iter().find(|j| j.label == DOWNLOAD_LABEL).map(|j| j.progress);
        let (mut apply, mut close, mut download, mut clear) = (false, false, false, false);
        let modal = egui::Modal::new(egui::Id::new("denoise")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(RichText::new(format!("{}  KI-Entrauschen", icon::SPARKLE)).size(17.0).strong().color(Color32::WHITE));
            ui.add_space(6.0);
            ui.label(
                "Ein neuronales Netz, trainiert auf echten Aufnahmepaaren mit und ohne Rauschen, säubert die RAW-Daten \
                 und rechnet sie dabei selbst in Farbe um. Das Original bleibt unverändert; die Stärke lässt sich \
                 jederzeit ändern (Details › KI-Entrauschen).",
            );
            if n > 0 {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(if n == 1 { "Stärke".to_string() } else { format!("Stärke für {n} Bilder") });
                    ui.add(egui::Slider::new(&mut self.denoise_strength, 0.0..=100.0).fixed_decimals(0));
                });
                let on_gpu = sk_ai::runtime::gpu_enabled() && sk_ai::runtime::gpu_installed();
                let where_ = if on_gpu {
                    "Die Berechnung läuft einmal je Bild im Hintergrund auf der Grafikkarte."
                } else {
                    "Die Berechnung läuft einmal je Bild im Hintergrund auf dem Prozessor (etwa eine halbe Minute für 24 Megapixel)."
                };
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(where_).small().color(Color32::from_gray(150)));
                    if !on_gpu && sk_ai::runtime::gpu_supported() && ui.small_button("KI-Beschleunigung …").clicked() {
                        self.show_gpu_dialog = true;
                    }
                });
            }
            if !available {
                ui.add_space(10.0);
                match downloading {
                    Some(p) => {
                        ui.add(egui::ProgressBar::new(p).text(format!("Modell wird geladen … {:.0} %", p * 100.0)));
                    }
                    None => {
                        let dialog = self.denoise_dialog.as_mut().unwrap();
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
                        if ui.add_enabled(dialog.accepted, Button::new(format!("{}  Modell laden", icon::DOWNLOAD_SIMPLE))).clicked() {
                            download = true;
                        }
                    }
                }
            }
            if available {
                ui.add_space(8.0);
                let used = sk_core::ai_denoise::cache_size();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!(
                        "Zwischenspeicher: {:.1} GB (höchstens {} GB, die ältesten fallen heraus)",
                        used as f64 / 1e9,
                        sk_core::ai_denoise::CACHE_LIMIT >> 30
                    ))
                    .small()
                    .color(Color32::from_gray(150)));
                    if used > 0 && self.denoise.running.is_none() && ui.small_button("Leeren").clicked() {
                        clear = true;
                    }
                });
            }
            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if n > 0 {
                    if ui.add_enabled(available, Button::new("Anwenden").min_size(vec2(110.0, 26.0))).clicked() {
                        apply = true;
                    }
                    if ui.add(Button::new("Abbrechen").min_size(vec2(90.0, 26.0))).clicked() {
                        close = true;
                    }
                } else if ui.add(Button::new(if available { "Fertig" } else { "Schließen" }).min_size(vec2(90.0, 26.0))).clicked() {
                    close = true;
                }
            });
        });
        if download {
            self.download_denoise_models();
        }
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
        if downloading.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        if apply {
            if let Some(d) = self.denoise_dialog.take() {
                self.apply_denoise(d.files);
            }
        } else if close || modal.should_close() {
            self.denoise_dialog = None;
        }
    }
}
