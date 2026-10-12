//! Soft proofing in the viewer (see `sk_core::softproof`): settings, the transform in use and
//! the controls in the output tab.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Color32, RichText, Ui};
use sk_core::color::OutputSpace;
use sk_core::softproof::{ProofSettings, ProofTarget, RenderIntent, SoftProof};

use crate::app::App;
use crate::theme::icon;
use crate::widgets::{self, section};

/// Soft proof state of the app.
#[derive(Default)]
pub struct SoftProofState {
    pub on: bool,
    pub settings: ProofSettings,
    /// Transform for `settings`, made when needed.
    pub transform: Option<Arc<SoftProof>>,
    pub error: Option<String>,
    /// The target `transform` was made for (`Recipe` resolved to the recipe's color space).
    pub resolved: Option<ProofTarget>,
    /// ICC profiles found in the system folders (searched once, when the controls are first shown).
    pub found: Option<Vec<(PathBuf, String)>>,
}

impl App {
    /// Umschalt+P: soft proof on or off.
    pub fn toggle_softproof(&mut self) {
        self.softproof.on = !self.softproof.on;
        self.softproof.error = None;
        self.request_render();
    }

    /// Color space of the selected output recipe (what the target `Recipe` stands for).
    fn recipe_space(&self) -> OutputSpace {
        self.recipes.get(self.selected_recipe).map(|r| r.output_space).unwrap_or_default()
    }

    /// Follows the selected recipe while the target is `Recipe`: a changed color space makes a
    /// new transform and rendering. Called every frame.
    pub fn sync_softproof(&mut self) {
        let want = self.softproof.settings.target.resolved(self.recipe_space());
        if self.softproof.resolved.as_ref().is_some_and(|r| *r != want) {
            self.softproof.transform = None;
            self.softproof.error = None;
            self.softproof.resolved = None;
            if self.softproof.on {
                self.request_render();
            }
        }
    }

    /// Transform for the rendering, if soft proofing is on (made on first use).
    pub fn active_softproof(&mut self) -> Option<Arc<SoftProof>> {
        if !self.softproof.on {
            return None;
        }
        let space = self.recipe_space();
        let sp = &mut self.softproof;
        if sp.transform.is_none() && sp.error.is_none() {
            let target = sp.settings.target.resolved(space);
            match SoftProof::new(&ProofSettings { target: target.clone(), ..sp.settings.clone() }) {
                Ok(mut t) => {
                    if sp.settings.target == ProofTarget::Recipe {
                        t.name = format!("Rezept: {}", t.name);
                    }
                    sp.transform = Some(Arc::new(t));
                }
                Err(e) => sp.error = Some(format!("{e:#}")),
            }
            sp.resolved = Some(target);
        }
        sp.transform.clone()
    }

    fn change_proof_settings(&mut self, f: impl FnOnce(&mut ProofSettings)) {
        let before = self.softproof.settings.clone();
        f(&mut self.softproof.settings);
        if self.softproof.settings != before {
            self.softproof.transform = None;
            self.softproof.error = None;
            self.softproof.resolved = None;
            self.save_settings();
            if self.softproof.on {
                self.request_render();
            }
        }
    }

    /// Controls in the output tab.
    pub fn softproof_section(&mut self, ui: &mut Ui) {
        if self.softproof.found.is_none() {
            self.softproof.found = Some(sk_core::softproof::find_profiles());
        }
        let mut s = self.softproof.settings.clone();
        let mut toggle = false;
        let mut pick = false;
        section(ui, "Softproof", None, |ui| {
            if ui.checkbox(&mut self.softproof.on.clone(), "Softproof im Viewer (Umschalt+P)").changed() {
                toggle = true;
            }
            let name = match &s.target {
                ProofTarget::Recipe => format!("Wie Ausgabe-Rezept ({})", self.recipe_space().label()),
                t => sk_core::softproof::target_name(t),
            };
            widgets::combo_field(ui, "Profil", name, |ui| {
                ui.selectable_value(&mut s.target, ProofTarget::Recipe, "Wie Ausgabe-Rezept")
                    .on_hover_text("Farbraum des gewählten Ausgabe-Rezepts");
                ui.separator();
                for sp in [OutputSpace::Srgb, OutputSpace::AdobeRgb, OutputSpace::DisplayP3] {
                    ui.selectable_value(&mut s.target, ProofTarget::Space(sp), sp.label());
                }
                let mut files: Vec<(PathBuf, String)> = s.profiles.iter().map(|p| (p.clone(), sk_core::softproof::profile_name(p))).collect();
                files.extend(self.softproof.found.iter().flatten().filter(|(p, _)| !s.profiles.contains(p)).cloned());
                if !files.is_empty() {
                    ui.separator();
                }
                for (p, n) in files {
                    ui.selectable_value(&mut s.target, ProofTarget::Icc(p.clone()), n).on_hover_text(p.display().to_string());
                }
                ui.separator();
                if ui.button(format!("{}  ICC-Profil laden …", icon::FOLDER_OPEN)).clicked() {
                    pick = true;
                }
            });
            widgets::combo_field(ui, "Priorität", s.intent.label(), |ui| {
                for i in RenderIntent::ALL {
                    ui.selectable_value(&mut s.intent, i, i.label());
                }
            });
            ui.checkbox(&mut s.black_point_compensation, "Tiefenkompensierung");
            ui.checkbox(&mut s.simulate_paper, "Papier und Druckfarbe simulieren")
                .on_hover_text("Zeigt den Papierton und das tatsächliche Schwarz des Drucks (absolut farbmetrisch)");
            ui.checkbox(&mut s.gamut_warning, "Farbumfang-Warnung")
                .on_hover_text("Farben, die das Profil nicht wiedergeben kann, erscheinen magenta; im Proof: was die Datei beschneidet");
            ui.checkbox(&mut s.paper_margin, "Papierrand zeigen")
                .on_hover_text("Papier um das Bild, im Papierton, wenn er simuliert wird");
            if let Some(e) = &self.softproof.error {
                ui.label(RichText::new(e).small().color(Color32::from_rgb(230, 110, 100)));
            }
        });
        if pick
            && let Some(p) = rfd::FileDialog::new().add_filter("ICC-Profil", &["icc", "icm"]).set_title("ICC-Profil wählen").pick_file()
        {
            if !s.profiles.contains(&p) {
                s.profiles.insert(0, p.clone());
            }
            s.target = ProofTarget::Icc(p);
        }
        self.change_proof_settings(|old| *old = s);
        if toggle {
            self.toggle_softproof();
        }
    }

    /// Label in the viewer while soft proofing.
    pub(crate) fn softproof_label(&self) -> Option<String> {
        if !self.softproof.on {
            return None;
        }
        Some(match (&self.softproof.transform, &self.softproof.error) {
            (_, Some(_)) => "SOFTPROOF  ·  Profil nicht verwendbar".into(),
            (Some(t), _) => {
                let warn = if self.softproof.settings.gamut_warning { "  ·  außerhalb: magenta" } else { "" };
                format!("SOFTPROOF  ·  {}  ·  {}{warn}", t.name, self.softproof.settings.intent.label())
            }
            (None, None) => "SOFTPROOF".into(),
        })
    }
}
