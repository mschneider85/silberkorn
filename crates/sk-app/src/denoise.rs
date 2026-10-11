//! "Denoise (AI)": dialog and background job (see `sk_ai::denoise`). The model is downloaded
//! on request.

use std::path::PathBuf;

use egui::{Button, Color32, RichText, vec2};
use sk_ai::models::{self, ModelInfo};

use crate::app::App;
use crate::theme::icon;

pub struct DenoiseDialog {
    pub files: Vec<PathBuf>,
}

impl App {
    pub fn open_denoise(&mut self) {
        self.commit_and_save();
        let files = sk_core::variant::files_of(&self.selected_paths());
        if files.is_empty() {
            return;
        }
        self.denoise_dialog = Some(DenoiseDialog { files });
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

    fn run_denoise(&mut self, files: Vec<PathBuf>) {
        let strength = self.denoise_strength / 100.0;
        self.save_settings();
        let label = if files.len() == 1 { "Entrauschen".to_string() } else { format!("Entrauschen ({} Bilder)", files.len()) };
        self.start_job(&label, Box::new(move |p| {
            let model = sk_ai::denoise::Denoiser::load(&models::path(&models::DENOISE))?;
            let n = files.len() as f32;
            let mut out = Vec::new();
            for (i, f) in files.iter().enumerate() {
                out.push(sk_ai::denoise::denoise_file(f, strength, &model, &|q| p((i as f32 + q) / n))?);
            }
            Ok(out)
        }));
    }

    pub(crate) fn denoise_dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &self.denoise_dialog else { return };
        let n = dialog.files.len();
        let available = models::available(&models::DENOISE);
        let downloading = self.model_download_progress(&models::DENOISE);
        let (mut run, mut cancel, mut download) = (false, false, false);
        let modal = egui::Modal::new(egui::Id::new("denoise")).show(ctx, |ui| {
            ui.set_width(430.0);
            ui.label(RichText::new(format!("{}  Entrauschen (KI)", icon::SPARKLE)).size(17.0).strong().color(Color32::WHITE));
            ui.add_space(6.0);
            ui.label(format!(
                "Ein neuronales Netz, trainiert auf echtem Kamerarauschen, entfernt Rauschen aus den RAW-Daten. \
                 Es entsteht für {} ein neues DNG („…-Entrauscht.dng“) mit den Einstellungen des Originals.",
                if n == 1 { "das Bild".to_string() } else { format!("jedes der {n} Bilder") }
            ));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Stärke");
                ui.add(egui::Slider::new(&mut self.denoise_strength, 0.0..=100.0).fixed_decimals(0));
            });
            ui.label(RichText::new("Die Berechnung läuft im Hintergrund auf allen Prozessorkernen und dauert je Bild einige Minuten.").small().color(Color32::from_gray(150)));
            if !available {
                ui.add_space(8.0);
                match downloading {
                    Some(p) => {
                        ui.add(egui::ProgressBar::new(p).text(format!("Modell wird geladen … {:.0} %", p * 100.0)));
                    }
                    None => {
                        ui.label(format!(
                            "Das Modell ({}, {} MB, Lizenz {}) wird einmalig heruntergeladen.",
                            models::DENOISE.label,
                            models::DENOISE.size_mb,
                            models::DENOISE.license
                        ));
                        if ui.button(format!("{}  Modell laden", icon::DOWNLOAD_SIMPLE)).clicked() {
                            download = true;
                        }
                    }
                }
            }
            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(available, Button::new("Entrauschen").min_size(vec2(110.0, 26.0))).clicked() {
                    run = true;
                }
                if ui.add(Button::new("Abbrechen").min_size(vec2(90.0, 26.0))).clicked() {
                    cancel = true;
                }
            });
        });
        if download {
            self.download_model_job(&models::DENOISE);
        }
        if downloading.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        if run {
            if let Some(d) = self.denoise_dialog.take() {
                self.run_denoise(d.files);
            }
        } else if cancel || modal.should_close() {
            self.denoise_dialog = None;
        }
    }
}
