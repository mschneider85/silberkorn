//! "Merge" dialogs: HDR from an exposure bracket, panoramas and focus stacks (see
//! `sk_core::hdr`, `sk_core::panorama` and `sk_core::focus_stack`). All run as background jobs
//! and create a linear DNG.

use std::path::PathBuf;

use egui::Ui;
use sk_core::hdr::Deghost;
use sk_core::panorama::Projection;

use crate::app::App;
use crate::dialog::{self, Dialog};
use crate::theme::icon;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeKind {
    Hdr,
    Panorama,
    FocusStack,
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
            crate::widgets::menu_button(ui, format!("{}  Zusammenfügen", icon::STACK), |ui| {
                if ui.button(format!("{}  HDR …", icon::SUN_HORIZON)).clicked() {
                    self.open_merge(MergeKind::Hdr);
                }
                if ui.button(format!("{}  Panorama …", icon::PANORAMA)).clicked() {
                    self.open_merge(MergeKind::Panorama);
                }
                if ui.button(format!("{}  Focus Stacking …", icon::APERTURE)).clicked() {
                    self.open_merge(MergeKind::FocusStack);
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
            MergeKind::FocusStack => {
                self.start_job("Focus Stacking", Box::new(move |p| Ok(vec![sk_core::focus_stack::stack_files(&files, p)?])));
            }
        }
    }

    pub(crate) fn merge_dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &self.merge_dialog else { return };
        let (kind, n) = (dialog.kind, dialog.files.len());
        let (mut run, mut cancel) = (false, false);
        let (ic, title) = match kind {
            MergeKind::Hdr => (icon::SUN_HORIZON, "HDR zusammenfügen"),
            MergeKind::Panorama => (icon::PANORAMA, "Panorama zusammenfügen"),
            MergeKind::FocusStack => (icon::APERTURE, "Focus Stacking"),
        };
        let modal = Dialog::new("merge", ic, title).subtitle(format!("{n} Bilder · ergibt ein DNG")).width(460.0).show(ctx, |ui| {
            match kind {
                MergeKind::Hdr => {
                    ui.label(format!(
                        "{n} Belichtungen werden zu einem DNG mit dem Dynamikumfang aller Bilder zusammengefügt. \
                         Weißabgleich, Farbe und Tonwerte bleiben voll bearbeitbar."
                    ));
                    ui.add_space(6.0);
                    dialog::card(ui, "Optionen", None, |ui| {
                        crate::widgets::checkbox(ui, &mut self.hdr_options.align, "Automatisch ausrichten (aus der Hand fotografiert)");
                        egui::ComboBox::from_label("Geister entfernen").selected_text(self.hdr_options.deghost.label()).show_ui(ui, |ui| {
                            for d in Deghost::ALL {
                                ui.selectable_value(&mut self.hdr_options.deghost, d, d.label());
                            }
                        });
                        dialog::note(ui, "Bewegte Motive (Menschen, Äste, Wolken) werden aus der mittleren Belichtung übernommen.");
                    });
                }
                MergeKind::Panorama => {
                    ui.label(format!(
                        "{n} überlappende Bilder werden zu einem Panorama-DNG zusammengefügt. Reihenfolge und Ausrichtung \
                         werden aus dem Bildinhalt bestimmt."
                    ));
                    ui.add_space(6.0);
                    dialog::card(ui, "Optionen", None, |ui| {
                        // Label in front, as everywhere.
                        crate::widgets::combo_field(ui, "Projektion", self.pano_options.projection.label(), |ui| {
                            for p in Projection::ALL {
                                ui.selectable_value(&mut self.pano_options.projection, p, p.label());
                            }
                        });
                        dialog::note(ui, "Sphärisch für weite und mehrreihige Panoramen, zylindrisch für eine Reihe, perspektivisch für Architektur (gerade Linien, bis etwa 120°).");
                        crate::widgets::checkbox(ui, &mut self.pano_options.auto_crop, "Rand automatisch beschneiden");
                        crate::widgets::checkbox(ui, &mut self.pano_options.lens_correction, "Objektivkorrektur der Einzelbilder anwenden");
                    });
                }
                MergeKind::FocusStack => {
                    ui.label(format!(
                        "{n} Aufnahmen mit unterschiedlicher Schärfeebene werden zu einem überall scharfen DNG zusammengefügt."
                    ));
                    dialog::note(ui, "Verwacklung und die Größenänderung beim Fokussieren werden ausgeglichen.");
                }
            }
            dialog::footer(ui, |ui| {
                if dialog::default_button(ui, true, dialog::primary("Zusammenfügen")) {
                    run = true;
                }
                if ui.add(dialog::secondary("Abbrechen")).clicked() {
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
