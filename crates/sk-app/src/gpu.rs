//! The graphics card: development pipeline (see `sk_core::gpu`) and the AI networks
//! (see `sk_ai::runtime`).

use egui::{Button, Color32, RichText, vec2};
use sk_ai::runtime;

use crate::ai_models::AiFeature;
use crate::app::App;
use crate::theme::icon;

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
        let mut model_action = None;
        let mut enabled = runtime::gpu_enabled();
        let mut develop = sk_core::gpu::enabled();
        let modal = egui::Modal::new(egui::Id::new("gpu")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(RichText::new(format!("{}  {}", icon::LIGHTNING, DIALOG_TITLE.trim_end_matches(" …"))).size(17.0).strong().color(Color32::WHITE));
            ui.add_space(6.0);
            ui.label(RichText::new("Bildberechnung").strong());
            ui.label("Die Entwicklung (Regler, Vorschau, Export) rechnet auf der Grafikkarte, mit dem Prozessor als Rückfall.");
            ui.checkbox(&mut develop, "Grafikkarte verwenden");
            let status = match sk_core::gpu::status() {
                _ if !develop => "Aus: alles rechnet auf dem Prozessor.".to_string(),
                None => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(200));
                    "Wird eingerichtet …".to_string()
                }
                Some(None) => "Keine geeignete Grafikkarte gefunden; es rechnet der Prozessor.".to_string(),
                Some(Some(name)) => match sk_core::gpu::problem() {
                    Some(why) => format!("{name}: {why}. Bis zum Neustart rechnet der Prozessor."),
                    None => format!("Aktiv: {name}"),
                },
            };
            ui.label(RichText::new(status).small().color(Color32::from_gray(170)));
            ui.add_space(10.0);
            ui.label(RichText::new("KI-Berechnung").strong());
            ui.label(
                "KI-Werkzeuge rechnen auf der Grafikkarte (jede Karte mit DirectX 12) ein Vielfaches schneller als auf dem Prozessor.",
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
                            "Dazu werden einmalig etwa {} MB von Microsoft geladen.",
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
                    (Some(p), _) => format!("Grafikkarte nicht verwendbar: {p}. Es wird auf dem Prozessor gerechnet."),
                    (None, Some(d)) => format!("Zuletzt verwendet: {d}"),
                    (None, None) => "Bereit; wird beim nächsten KI-Werkzeug verwendet.".to_string(),
                };
                ui.label(RichText::new(status).small().color(Color32::from_gray(170)));
            }
            ui.add_space(10.0);
            ui.label(RichText::new("KI-Modelle").strong());
            ui.label("Jedes Modell wird beim ersten Gebrauch geladen; hier lässt es sich auch vorab laden.");
            ui.add_space(4.0);
            model_action = self.models_ui(ui);
            self.denoise_cache_ui(ui);
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
        if develop != sk_core::gpu::enabled() {
            self.set_develop_gpu(develop);
        }
        if install {
            self.install_gpu_job();
        }
        match model_action {
            Some(ModelAction::One(f)) => {
                self.ensure_ai(f);
            }
            Some(ModelAction::All) => self.download_all_ai(),
            None => {}
        }
        if close || modal.should_close() {
            self.show_gpu_dialog = false;
        }
    }
}

pub(crate) const GPU_JOB: &str = "KI-Beschleunigung laden";

/// Menu entry and title of the dialog.
pub const DIALOG_TITLE: &str = "Grafikkarte und KI-Modelle …";

/// What the model list asks for.
enum ModelAction {
    One(AiFeature),
    All,
}

impl App {
    /// The AI models: what each is for, model and license, size, loaded or not (with a button).
    fn models_ui(&self, ui: &mut egui::Ui) -> Option<ModelAction> {
        let mut action = None;
        egui::Grid::new("models").num_columns(3).spacing([12.0, 4.0]).show(ui, |ui| {
            for f in AiFeature::ALL {
                let (name, license) = f.credits();
                ui.label(f.label());
                ui.label(RichText::new(format!("{name} · {} MB · {license}", f.download_mb())).small().color(Color32::from_gray(150)));
                if f.available() {
                    ui.label(RichText::new(format!("{}  geladen", icon::CHECK)).small().color(Color32::from_gray(170)));
                } else if let Some(p) = self.ai_download_progress(f) {
                    ui.add(egui::ProgressBar::new(p).desired_width(90.0).desired_height(6.0));
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
                } else if ui.small_button(format!("{}  Laden", icon::DOWNLOAD_SIMPLE)).clicked() {
                    action = Some(ModelAction::One(f));
                }
                ui.end_row();
            }
        });
        let missing = AiFeature::ALL.iter().filter(|f| !f.available() && self.ai_download_progress(**f).is_none()).count();
        if missing > 1 {
            ui.add_space(4.0);
            let text = format!("{}  Alle vorab laden ({} MB)", icon::DOWNLOAD_SIMPLE, self.missing_ai_mb());
            if ui.button(text).on_hover_text("Damit alle KI-Funktionen auch ohne Internet gehen").clicked() {
                action = Some(ModelAction::All);
            }
        }
        action
    }
}
