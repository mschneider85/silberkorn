//! "Merge" dialogs: HDR from an exposure bracket, panoramas, focus stacks and long exposures (see
//! `sk_core::hdr`, `sk_core::panorama`, `sk_core::focus_stack` and `sk_core::long_exposure`), and
//! Super Resolution of single images (`sk_ai::super_resolution`). All run as background jobs and
//! create linear DNGs.

use std::path::PathBuf;

use egui::Ui;
use sk_core::hdr::Deghost;
use sk_core::long_exposure::Blend;
use sk_core::panorama::Projection;

use crate::app::App;
use crate::dialog::{self, Dialog};
use crate::theme::icon;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeKind {
    Hdr,
    Panorama,
    FocusStack,
    LongExposure,
    /// Each image on its own, twice the edges.
    SuperResolution,
}

pub struct MergeDialog {
    pub kind: MergeKind,
    pub files: Vec<PathBuf>,
    /// Pixels of an image of the series (long exposure: how many fit into a median).
    pub pixels: Option<usize>,
    /// Size of the result (Super Resolution of one image).
    pub size: Option<(usize, usize)>,
}

impl App {
    pub fn open_merge(&mut self, kind: MergeKind) {
        self.commit_and_save();
        if kind == MergeKind::SuperResolution {
            let files = sk_core::variant::files_of(&self.selected_paths());
            if files.is_empty() {
                self.status = "Für Super Resolution ein Bild auswählen".into();
                return;
            }
            // The selected version of a single image, with its crop.
            let size = match files.as_slice() {
                [file] => self.selected_paths().into_iter().find(|p| sk_core::variant::file_of(p) == *file).and_then(|p| {
                    let meta = sk_core::raw::read_meta(file).ok()?;
                    Some(sk_ai::super_resolution::target_size(&meta, &sk_core::sidecar::load(&p).adjustments))
                }),
                _ => None,
            };
            self.merge_dialog = Some(MergeDialog { kind, files, pixels: None, size });
            return;
        }
        let files = self.merge_files();
        if files.len() < 2 {
            self.status = "Zum Zusammenfügen mindestens zwei Bilder oder einen Stapel auswählen".into();
            return;
        }
        let pixels = (kind == MergeKind::LongExposure)
            .then(|| sk_core::raw::read_meta(&files[0]).ok())
            .flatten()
            .map(|m| m.info.width * m.info.height)
            .filter(|&px| px > 0);
        self.merge_dialog = Some(MergeDialog { kind, files, pixels, size: None });
    }

    /// Entries of the image menu.
    pub fn merge_menu(&mut self, ui: &mut Ui) {
        let n = self.merge_files().len();
        ui.add_enabled_ui(n >= 2, |ui| {
            crate::widgets::menu_button(ui, format!("{}  Zusammenfügen", icon::UNITE), |ui| {
                if ui.button(format!("{}  HDR …", icon::SUN_HORIZON)).clicked() {
                    self.open_merge(MergeKind::Hdr);
                }
                if ui.button(format!("{}  Panorama …", icon::PANORAMA)).clicked() {
                    self.open_merge(MergeKind::Panorama);
                }
                if ui.button(format!("{}  Focus Stacking …", icon::APERTURE)).clicked() {
                    self.open_merge(MergeKind::FocusStack);
                }
                if ui.button(format!("{}  Langzeitbelichtung …", icon::HOURGLASS_MEDIUM)).clicked() {
                    self.open_merge(MergeKind::LongExposure);
                }
            })
            .response
            .on_disabled_hover_text("Mindestens zwei Bilder oder einen Stapel auswählen");
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
            MergeKind::LongExposure => {
                let opts = self.long_exposure_options;
                self.start_job("Langzeitbelichtung", Box::new(move |p| Ok(vec![sk_core::long_exposure::combine_files(&files, &opts, p)?])));
            }
            MergeKind::SuperResolution => {
                // Each file once, with the settings of the first selected version of it.
                let mut jobs: Vec<(PathBuf, sk_core::params::Adjustments)> = Vec::new();
                for p in self.selected_paths() {
                    let file = sk_core::variant::file_of(&p);
                    if files.contains(&file) && !jobs.iter().any(|(f, _)| *f == file) {
                        jobs.push((file, sk_core::sidecar::load(&p).adjustments));
                    }
                }
                let denoise = jobs.iter().any(|(_, a)| sk_ai::super_resolution::Enhanced::of(&a.visible()).ai_denoise > 0.0);
                let start = move |app: &mut App| {
                    app.start_job("Super Resolution", Box::new(move |p| {
                        let n = jobs.len() as f32;
                        let mut out = Vec::new();
                        for (k, (f, adj)) in jobs.iter().enumerate() {
                            let (dng, adjustments) = sk_ai::super_resolution::enlarge_file(f, adj, &|q| p((k as f32 + q) / n))?;
                            // Looks like the original right away.
                            sk_core::sidecar::save(&dng, &sk_core::sidecar::Sidecar { adjustments, ..Default::default() })?;
                            out.push(dng);
                        }
                        Ok(out)
                    }));
                };
                // The denoised data first needs its model.
                if denoise {
                    self.with_ai(crate::ai_models::AiFeature::Denoise, start);
                } else {
                    start(self);
                }
            }
        }
    }

    pub(crate) fn merge_dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &self.merge_dialog else { return };
        let (kind, n, pixels, size) = (dialog.kind, dialog.files.len(), dialog.pixels, dialog.size);
        let (mut run, mut cancel) = (false, false);
        let (ic, title) = match kind {
            MergeKind::Hdr => (icon::SUN_HORIZON, "HDR zusammenfügen"),
            MergeKind::Panorama => (icon::PANORAMA, "Panorama zusammenfügen"),
            MergeKind::FocusStack => (icon::APERTURE, "Focus Stacking"),
            MergeKind::LongExposure => (icon::HOURGLASS_MEDIUM, "Langzeitbelichtung"),
            MergeKind::SuperResolution => (icon::RESIZE, "Super Resolution"),
        };
        let subtitle = match kind {
            MergeKind::SuperResolution if n == 1 => "ergibt ein DNG".to_string(),
            MergeKind::SuperResolution => format!("{n} Bilder · ergibt je ein DNG"),
            _ => format!("{n} Bilder · ergibt ein DNG"),
        };
        let modal = Dialog::new("merge", ic, title).subtitle(subtitle).width(460.0).show(ctx, |ui| {
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
                MergeKind::LongExposure => {
                    ui.label(format!("{n} Aufnahmen derselben Szene werden zu einem DNG zusammengefügt."));
                    ui.add_space(6.0);
                    dialog::card(ui, "Optionen", None, |ui| {
                        let opts = &mut self.long_exposure_options;
                        crate::widgets::combo_field(ui, "Verfahren", opts.blend.label(), |ui| {
                            for b in Blend::ALL {
                                ui.selectable_value(&mut opts.blend, b, b.label());
                            }
                        });
                        dialog::note(ui, opts.blend.note());
                        crate::widgets::checkbox(ui, &mut opts.align, "Automatisch ausrichten (aus der Hand fotografiert)");
                        let used = match pixels {
                            Some(px) if opts.blend == Blend::Median => sk_core::long_exposure::median_shots(n, px).len(),
                            _ => n,
                        };
                        if used < n {
                            dialog::note(ui, format!("Der Arbeitsspeicher reicht für den Median nur für {used} der {n} Aufnahmen. Sie werden gleichmäßig über die Serie verteilt ausgewählt."));
                        }
                    });
                }
                MergeKind::SuperResolution => {
                    ui.label(
                        "Die KI verdoppelt Breite und Höhe (viermal so viele Pixel) und arbeitet feine Details heraus, \
                         ohne welche zu erfinden. Das Ergebnis ist ein neues DNG neben dem Original (Name mit „-SR“). \
                         Weißabgleich, Farbe und Tonwerte bleiben voll bearbeitbar.",
                    );
                    if let Some((w, h)) = size {
                        ui.add_space(4.0);
                        ui.label(format!("Zielgröße: {w} × {h}"));
                    }
                    dialog::note(
                        ui,
                        "KI-Entrauschen und Raw-Details (Verbessern) werden so eingerechnet, wie sie am Bild eingestellt sind, \
                         sonst würde das Rauschen mit vergrößert. Die übrige Bearbeitung wird übernommen.",
                    );
                    dialog::note(
                        ui,
                        "Ist das Bild zugeschnitten, wird nur der Ausschnitt vergrößert. Begradigen, Perspektive und \
                         Objektivkorrektur werden dann fest eingerechnet, Ebenen und Retusche nicht übernommen.",
                    );
                }
            }
            dialog::footer(ui, |ui| {
                let action = if kind == MergeKind::SuperResolution { "Vergrößern" } else { "Zusammenfügen" };
                if dialog::default_button(ui, true, dialog::primary(action)) {
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
