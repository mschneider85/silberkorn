//! "Merge" dialogs: HDR from an exposure bracket and panoramas (see `sk_core::hdr` and
//! `sk_core::panorama`). Both run as background jobs and create a linear DNG.

use std::path::PathBuf;

use egui::{Button, Color32, RichText, Ui, vec2};
use sk_core::hdr::Deghost;
use sk_core::panorama::Projection;

use crate::app::App;
use crate::theme::icon;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeKind {
    Hdr,
    Panorama,
}

pub struct MergeDialog {
    pub kind: MergeKind,
    pub files: Vec<PathBuf>,
}

impl App {
    pub fn open_merge(&mut self, kind: MergeKind) {
        self.commit_and_save();
        let files = sk_core::variant::files_of(&self.selected_paths());
        if files.len() < 2 {
            self.status = "Zum Zusammenfügen mindestens zwei Bilder auswählen".into();
            return;
        }
        self.merge_dialog = Some(MergeDialog { kind, files });
    }

    /// Entries of the image menu.
    pub fn merge_menu(&mut self, ui: &mut Ui) {
        let n = sk_core::variant::files_of(&self.selected_paths()).len();
        ui.add_enabled_ui(n >= 2, |ui| {
            ui.menu_button(format!("{}  Zusammenfügen", icon::STACK), |ui| {
                if ui.button(format!("{}  HDR …", icon::SUN_HORIZON)).clicked() {
                    self.open_merge(MergeKind::Hdr);
                }
                if ui.button(format!("{}  Panorama …", icon::PANORAMA)).clicked() {
                    self.open_merge(MergeKind::Panorama);
                }
            })
            .response
            .on_disabled_hover_text("Mindestens zwei Bilder auswählen");
        });
    }

    fn run_merge(&mut self, dialog: MergeDialog) {
        let files = dialog.files;
        self.save_settings();
        match dialog.kind {
            MergeKind::Hdr => {
                let opts = self.hdr_options;
                self.start_job("HDR zusammenfügen", Box::new(move |p| Ok(vec![sk_core::hdr::merge_files(&files, &opts, p)?])));
            }
            MergeKind::Panorama => {
                let opts = self.pano_options;
                self.start_job("Panorama zusammenfügen", Box::new(move |p| Ok(vec![sk_core::panorama::stitch_files(&files, &opts, p)?])));
            }
        }
    }

    pub(crate) fn merge_dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &self.merge_dialog else { return };
        let (kind, n) = (dialog.kind, dialog.files.len());
        let (mut run, mut cancel) = (false, false);
        let modal = egui::Modal::new(egui::Id::new("merge")).show(ctx, |ui| {
            ui.set_width(420.0);
            let (ic, title) = match kind {
                MergeKind::Hdr => (icon::SUN_HORIZON, "HDR zusammenfügen"),
                MergeKind::Panorama => (icon::PANORAMA, "Panorama zusammenfügen"),
            };
            ui.label(RichText::new(format!("{ic}  {title}")).size(17.0).strong().color(Color32::WHITE));
            ui.add_space(6.0);
            match kind {
                MergeKind::Hdr => {
                    ui.label(format!(
                        "{n} Belichtungen werden zu einem DNG mit dem Dynamikumfang aller Bilder zusammengefügt. \
                         Weißabgleich, Farbe und Tonwerte bleiben voll bearbeitbar."
                    ));
                    ui.add_space(8.0);
                    ui.checkbox(&mut self.hdr_options.align, "Automatisch ausrichten (aus der Hand fotografiert)");
                    egui::ComboBox::from_label("Geister entfernen").selected_text(self.hdr_options.deghost.label()).show_ui(ui, |ui| {
                        for d in Deghost::ALL {
                            ui.selectable_value(&mut self.hdr_options.deghost, d, d.label());
                        }
                    });
                    ui.label(RichText::new("Bewegte Motive (Menschen, Äste, Wolken) werden aus der mittleren Belichtung übernommen.").small().color(Color32::from_gray(150)));
                }
                MergeKind::Panorama => {
                    ui.label(format!(
                        "{n} überlappende Bilder werden zu einem Panorama-DNG zusammengefügt. Reihenfolge und Ausrichtung \
                         werden aus dem Bildinhalt bestimmt."
                    ));
                    ui.add_space(8.0);
                    egui::ComboBox::from_label("Projektion").selected_text(self.pano_options.projection.label()).show_ui(ui, |ui| {
                        for p in Projection::ALL {
                            ui.selectable_value(&mut self.pano_options.projection, p, p.label());
                        }
                    });
                    ui.label(RichText::new("Sphärisch für weite und mehrreihige Panoramen, zylindrisch für eine Reihe, perspektivisch für Architektur (gerade Linien, bis etwa 120°).").small().color(Color32::from_gray(150)));
                    ui.checkbox(&mut self.pano_options.auto_crop, "Rand automatisch beschneiden");
                    ui.checkbox(&mut self.pano_options.lens_correction, "Objektivkorrektur der Einzelbilder anwenden");
                }
            }
            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(Button::new("Zusammenfügen").min_size(vec2(110.0, 26.0))).clicked() {
                    run = true;
                }
                if ui.add(Button::new("Abbrechen").min_size(vec2(90.0, 26.0))).clicked() {
                    cancel = true;
                }
            });
        });
        if run {
            if let Some(d) = self.merge_dialog.take() {
                self.run_merge(d);
            }
        } else if cancel || modal.should_close() {
            self.merge_dialog = None;
        }
    }
}
