//! "AI acceleration": the networks on the graphics card (see `sk_ai::runtime`).

use egui::{Button, Color32, RichText, vec2};
use sk_ai::runtime;

use crate::app::App;
use crate::theme::icon;

impl App {
    pub fn set_ai_gpu(&mut self, on: bool) {
        runtime::set_gpu_enabled(on);
        self.save_settings();
    }

    fn install_gpu_job(&mut self) {
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
        let modal = egui::Modal::new(egui::Id::new("gpu")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(RichText::new(format!("{}  KI-Beschleunigung", icon::LIGHTNING)).size(17.0).strong().color(Color32::WHITE));
            ui.add_space(6.0);
            ui.label(
                "Entrauschen, Entfernen, Motivmasken und die Gesichtserkennung können auf der Grafikkarte rechnen \
                 (DirectML: jede Karte mit DirectX 12, auch AMD und Intel). Das ist ein Vielfaches schneller als auf dem Prozessor.",
            );
            ui.add_space(8.0);
            if !runtime::gpu_supported() {
                ui.label(RichText::new("Nur unter Windows verfügbar; hier rechnet alles auf dem Prozessor.").color(Color32::from_gray(160)));
            } else if !runtime::gpu_installed() {
                match installing {
                    Some(p) => {
                        ui.add(egui::ProgressBar::new(p).text(format!("Wird geladen … {:.0} %", p * 100.0)));
                        ctx.request_repaint_after(std::time::Duration::from_millis(200));
                    }
                    None => {
                        ui.label(format!(
                            "Dazu werden ONNX Runtime und DirectML von Microsoft geladen (einmalig etwa {} MB, Lizenzen MIT bzw. Microsoft-Redistributable).",
                            runtime::GPU_DOWNLOAD_MB
                        ));
                        if ui.button(format!("{}  Laden", icon::DOWNLOAD_SIMPLE)).clicked() {
                            install = true;
                        }
                    }
                }
            } else {
                ui.checkbox(&mut enabled, "Grafikkarte verwenden");
                let status = match (runtime::gpu_problem(), runtime::last_device()) {
                    (_, _) if !enabled => "Aus: alles rechnet auf dem Prozessor.".to_string(),
                    (Some(p), _) => format!("Grafikkarte nicht verwendbar: {p}. Es wird auf dem Prozessor gerechnet (ONNX Runtime, schneller als ohne)."),
                    (None, Some(d)) => format!("Zuletzt verwendet: {d}"),
                    (None, None) => "Bereit; wird beim nächsten KI-Werkzeug verwendet.".to_string(),
                };
                ui.label(RichText::new(status).small().color(Color32::from_gray(170)));
            }
            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(Button::new("Schließen").min_size(vec2(90.0, 26.0))).clicked() {
                    close = true;
                }
            });
        });
        if enabled != runtime::gpu_enabled() {
            self.set_ai_gpu(enabled);
        }
        if install {
            self.install_gpu_job();
        }
        if close || modal.should_close() {
            self.show_gpu_dialog = false;
        }
    }
}

const GPU_JOB: &str = "KI-Beschleunigung laden";
