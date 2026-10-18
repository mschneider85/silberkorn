//! The graphics card: development pipeline (see `sk_core::gpu`) and the AI networks
//! (see `sk_ai::runtime`).

use egui::RichText;
use sk_ai::runtime;

use crate::app::App;
use crate::dialog::{self, Dialog, Tone};
use crate::theme::{self, icon};

impl App {
    pub fn set_ai_gpu(&mut self, on: bool) {
        runtime::set_gpu_enabled(on);
        self.save_settings();
    }

    pub fn set_develop_gpu(&mut self, on: bool) {
        sk_core::gpu::set_enabled(on);
        self.save_settings();
        if on {
            std::thread::spawn(sk_core::pipeline::prepare_gpu);
        }
    }

    pub(crate) fn install_gpu_job(&mut self) {
        if self.jobs.iter().any(|j| j.label == GPU_JOB) {
            return;
        }
        self.start_job(GPU_JOB, Box::new(|p| {
            runtime::install_gpu(p)?;
            Ok(Vec::new())
        }));
    }

    pub(crate) fn gpu_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_gpu_dialog {
            return;
        }
        let installing = self.jobs.iter().find(|j| j.label == GPU_JOB).map(|j| j.progress);
        let (mut close, mut install) = (false, false);
        let mut enabled = runtime::gpu_enabled();
        let mut develop = sk_core::gpu::enabled();
        let title = DIALOG_TITLE.trim_end_matches(" …");
        let modal = Dialog::new("gpu", icon::LIGHTNING, title).subtitle("Wo Silberkorn rechnet").width(460.0).show(ctx, |ui| {
            let (tone, pill, detail) = match sk_core::gpu::status() {
                _ if !develop => (Tone::Off, "Aus", "Alles rechnet auf dem Prozessor.".to_string()),
                None => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(200));
                    (Tone::Accent, "Wird eingerichtet", "Die Grafikkarte wird vorbereitet …".to_string())
                }
                Some(None) => (Tone::Warn, "Prozessor", "Keine geeignete Grafikkarte gefunden; es rechnet der Prozessor.".to_string()),
                Some(Some(name)) => match sk_core::gpu::problem() {
                    Some(why) => (Tone::Warn, "Prozessor", format!("{name}: {why}. Bis zum Neustart rechnet der Prozessor.")),
                    None => (Tone::Good, "Aktiv", name),
                },
            };
            dialog::card(ui, "Bildberechnung", Some((tone, pill)), |ui| {
                dialog::note(ui, "Regler, Vorschau und Export rechnen auf der Grafikkarte, mit dem Prozessor als Rückfall.");
                ui.checkbox(&mut develop, "Grafikkarte verwenden");
                ui.label(RichText::new(detail).size(12.0).color(theme::TEXT));
            });

            let ai = if !runtime::gpu_supported() {
                (Tone::Off, "Nur Windows")
            } else if !runtime::gpu_installed() {
                (Tone::Off, "Nicht geladen")
            } else if !enabled {
                (Tone::Off, "Aus")
            } else if runtime::gpu_problem().is_some() {
                (Tone::Warn, "Prozessor")
            } else {
                (Tone::Good, "Bereit")
            };
            dialog::card(ui, "KI-Berechnung", Some(ai), |ui| {
                dialog::note(ui, "KI-Werkzeuge rechnen auf der Grafikkarte (jede Karte mit DirectX 12) ein Vielfaches schneller als auf dem Prozessor.");
                if !runtime::gpu_supported() {
                    ui.label(RichText::new("Hier rechnet alles auf dem Prozessor.").size(12.0).color(theme::TEXT));
                } else if !runtime::gpu_installed() {
                    match installing {
                        Some(p) => {
                            ui.add(egui::ProgressBar::new(p).desired_height(8.0).text(format!("Wird geladen … {:.0} %", p * 100.0)));
                            ctx.request_repaint_after(std::time::Duration::from_millis(200));
                        }
                        None => {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(format!("Einmalig etwa {} MB von Microsoft", runtime::GPU_DOWNLOAD_MB)).size(12.0).color(theme::TEXT));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.add(dialog::secondary(format!("{}  Laden", icon::DOWNLOAD_SIMPLE))).clicked() {
                                        install = true;
                                    }
                                });
                            });
                        }
                    }
                } else {
                    ui.checkbox(&mut enabled, "Grafikkarte verwenden");
                    let detail = match (runtime::gpu_problem(), runtime::last_device()) {
                        (_, _) if !enabled => "Alles rechnet auf dem Prozessor.".to_string(),
                        (Some(p), _) => format!("Grafikkarte nicht verwendbar: {p}. Es wird auf dem Prozessor gerechnet."),
                        (None, Some(d)) => format!("Zuletzt verwendet: {d}"),
                        (None, None) => "Wird beim nächsten KI-Werkzeug verwendet.".to_string(),
                    };
                    ui.label(RichText::new(detail).size(12.0).color(theme::TEXT));
                }
            });

            dialog::card(ui, "Zwischenspeicher", None, |ui| self.denoise_cache_ui(ui));
            dialog::footer(ui, |ui| {
                if dialog::default_button(ui, true, dialog::primary("Schließen")) {
                    close = true;
                }
            });
        });
        if enabled != runtime::gpu_enabled() {
            self.set_ai_gpu(enabled);
        }
        if develop != sk_core::gpu::enabled() {
            self.set_develop_gpu(develop);
        }
        if install {
            self.install_gpu_job();
        }
        if close || modal.should_close() {
            self.show_gpu_dialog = false;
        }
    }
}

pub(crate) const GPU_JOB: &str = "KI-Beschleunigung laden";

/// Menu entry and title of the dialog.
pub const DIALOG_TITLE: &str = "Grafikkarte …";
