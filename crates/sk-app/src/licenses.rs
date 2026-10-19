//! The licenses dialog: what Silberkorn uses from others and under which license (the AI models,
//! the lens database, the AI acceleration, the start screen's photo).

use egui::RichText;
use sk_ai::models;

use crate::app::App;
use crate::dialog::{self, Dialog};
use crate::theme::{self, icon};

/// Menu entry and title of the dialog.
pub const DIALOG_TITLE: &str = "Lizenzen …";

/// One work: name and authors, what it is used for, license.
type Entry = (&'static str, &'static str, &'static str);

fn sections() -> [(&'static str, Vec<Entry>); 3] {
    [
        (
            "KI-Modelle",
            vec![
                ("RawNIND (Benoit Brummer, UCLouvain)", "KI-Entrauschen", models::DENOISE_BAYER.license),
                ("U²-Net (Xuebin Qin et al.)", "Motiv und Himmel auswählen", models::SUBJECT.license),
                ("MobileSAM (Chaoning Zhang et al.), Segment Anything (Meta)", "Objekt auswählen", models::OBJECT_ENCODER.license),
                ("MI-GAN (Picsart)", "Objekt entfernen", models::INPAINT.license),
                ("Depth Anything V2 Small (Lihe Yang et al.)", "Tiefenkarte", models::DEPTH.license),
                ("YuNet (OpenCV Zoo)", "Gesichtserkennung", models::FACE_DETECT.license),
                ("SFace (OpenCV Zoo)", "Gesichtsvergleich", models::FACE_EMBED.license),
                ("open-closed-eye-0001 (Intel OpenVINO)", "Augen offen/geschlossen", models::EYE_STATE.license),
            ],
        ),
        (
            "Daten und Bibliotheken",
            vec![
                ("lensfun", "Objektivprofile", "CC BY-SA 3.0"),
                ("ONNX Runtime, DirectML (Microsoft)", "KI-Beschleunigung unter Windows", "MIT"),
            ],
        ),
        ("Bilder", vec![("Ansel Adams, The Tetons and the Snake River (1942)", "Startbildschirm", "gemeinfrei")]),
    ]
}

impl App {
    pub(crate) fn licenses_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_licenses {
            return;
        }
        let mut close = false;
        let title = DIALOG_TITLE.trim_end_matches(" …");
        let modal = Dialog::new("licenses", icon::SCALES, title).subtitle("Was Silberkorn von anderen verwendet").width(560.0).show(ctx, |ui| {
            for (title, entries) in sections() {
                dialog::card(ui, title, None, |ui| {
                    egui::Grid::new(title).num_columns(2).spacing([16.0, 8.0]).min_col_width(360.0).show(ui, |ui| {
                        for (name, used_for, license) in entries {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                ui.label(RichText::new(name).color(theme::TEXT));
                                ui.label(RichText::new(used_for).size(11.5).color(theme::TEXT_DIM));
                            });
                            dialog::tag(ui, license);
                            ui.end_row();
                        }
                    });
                });
            }
            dialog::note(ui, "Die KI-Modelle, die Objektivprofile und die KI-Beschleunigung werden nicht mitgeliefert, sondern bei Bedarf geladen.");
            dialog::footer(ui, |ui| {
                if dialog::default_button(ui, true, dialog::primary("Schließen")) {
                    close = true;
                }
            });
        });
        if close || modal.should_close() {
            self.show_licenses = false;
        }
    }
}
