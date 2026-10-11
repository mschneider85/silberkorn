//! Faces and culling in the library: analysing the shown images (faces, closed eyes,
//! sharpness), filtering by the result, and the people recognized (see `sk_ai::faces`).

use std::path::PathBuf;

use egui::{Color32, RichText, Ui};
use sk_core::catalog::{Catalog, CullFilter, FaceRecord, Scope};

use crate::app::App;
use crate::theme::icon;
use crate::widgets::section;

/// Analyses one image: the camera preview is enough (and fast); without one, a rendering.
fn analyze_file(models: &sk_ai::faces::FaceModels, path: &std::path::Path) -> anyhow::Result<sk_ai::faces::Analysis> {
    let img = match sk_core::raw::embedded_preview(path) {
        Ok(img) if img.width().max(img.height()) >= 800 => img,
        _ => {
            let (meta, preview) = sk_core::raw::load_preview(path, 1600)?;
            let r = sk_core::pipeline::Processor::new(&meta, &sk_core::params::Adjustments::default()).render(&preview, None, None);
            ::image::DynamicImage::ImageRgba8(::image::RgbaImage::from_raw(r.width as u32, r.height as u32, r.rgba).expect("size"))
        }
    };
    // About 1600 px are plenty for faces and sharpness.
    let img = if img.width().max(img.height()) > 2000 { img.resize(1600, 1600, ::image::imageops::FilterType::Triangle) } else { img };
    let rgb8 = img.to_rgb8();
    let rgb: Vec<[f32; 3]> = rgb8.pixels().map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]).collect();
    models.analyze(&sk_ai::faces::Image { width: rgb8.width() as usize, height: rgb8.height() as usize, rgb: &rgb })
}

impl App {
    /// Analyses the images of the current view (those not analysed yet, or all if `again`).
    pub fn analyze_view(&mut self, again: bool) {
        let files: Vec<PathBuf> = sk_core::variant::files_of(&self.visible.iter().map(|&i| self.entries[i].path.clone()).collect::<Vec<_>>());
        let todo: Vec<PathBuf> = match &self.catalog {
            Some(c) if !again => files.into_iter().filter(|f| !c.is_analyzed(f)).collect(),
            _ => files,
        };
        if todo.is_empty() {
            self.status = "Alle Bilder dieser Ansicht sind bereits analysiert".into();
            return;
        }
        let catalog = self.catalog_path();
        let label = format!("Gesichter & Schärfe ({})", crate::widgets::image_count(todo.len()));
        self.start_job(&label, Box::new(move |progress| {
            // Missing models first (small).
            let missing: Vec<_> = sk_ai::faces::NEEDED.iter().filter(|m| !sk_ai::models::available(m)).collect();
            for m in &missing {
                sk_ai::models::download(m, &|_| {})?;
            }
            let models = sk_ai::faces::FaceModels::load()?;
            let db = Catalog::open(&catalog)?;
            for (i, path) in todo.iter().enumerate() {
                if db.needs_index(path).unwrap_or(true) {
                    let _ = db.index_file(path, 0);
                }
                match analyze_file(&models, path) {
                    Ok(a) => {
                        let faces: Vec<FaceRecord> =
                            a.faces.iter().map(|f| FaceRecord { bbox: f.bbox, embedding: f.embedding.clone(), eyes_closed: f.eyes_closed }).collect();
                        db.store_analysis(path, a.sharpness, &faces, sk_ai::faces::SAME_PERSON)?;
                    }
                    Err(e) => log::warn!("Analyse {}: {e:#}", path.display()),
                }
                progress((i + 1) as f32 / todo.len() as f32);
            }
            Ok(Vec::new())
        }));
    }

    /// Gives a person a name; the name becomes a keyword of the person's images (the previous
    /// name is removed).
    pub fn name_person(&mut self, id: i64, name: &str) {
        let Some(c) = &self.catalog else { return };
        let old = c.person_name(id);
        let paths = match c.name_person(id, name) {
            Ok(p) => p,
            Err(e) => {
                self.status = format!("{e:#}");
                return;
            }
        };
        let name = name.trim().to_string();
        self.update_meta(&paths, |sc| {
            if let Some(o) = &old {
                sc.keywords.retain(|k| !k.eq_ignore_ascii_case(o));
            }
            if !name.is_empty() && !sc.keywords.iter().any(|k| k.eq_ignore_ascii_case(&name)) {
                sc.keywords.push(name.clone());
                sc.keywords.sort_by_key(|s| s.to_lowercase());
            }
        });
        self.refresh_catalog_lists();
    }

    /// Section of the library tab.
    pub(crate) fn culling_section(&mut self, ui: &mut Ui) {
        let mut analyze: Option<bool> = None;
        let mut cull: Option<Option<CullFilter>> = None;
        let mut open: Option<Scope> = None;
        let mut rename: Option<(i64, String)> = None;
        let shown: Vec<&sk_core::catalog::ImageRecord> = self.visible.iter().map(|&i| &self.entries[i]).collect();
        let analysed = shown.iter().filter(|r| r.culling.is_some()).count();
        let count = |f: CullFilter| self.entries.iter().filter(|r| f.matches(r.culling.as_ref())).count();
        section(ui, "Gesichter & Sichtung", None, |ui| {
            ui.label(RichText::new("Erkennt Gesichter, geschlossene Augen und unscharfe Bilder und ordnet Gesichter Personen zu.").small().color(Color32::from_gray(150)));
            ui.horizontal(|ui| {
                if ui.button(format!("{}  Ansicht analysieren", icon::SCAN)).on_hover_text("Noch nicht analysierte Bilder dieser Ansicht").clicked() {
                    analyze = Some(false);
                }
                if analysed > 0 && ui.small_button("Erneut").on_hover_text("Alle Bilder dieser Ansicht neu analysieren").clicked() {
                    analyze = Some(true);
                }
            });
            ui.label(RichText::new(format!("{analysed} von {} analysiert", shown.len())).small().color(Color32::from_gray(150)));
            if !sk_ai::faces::NEEDED.iter().all(|m| sk_ai::models::available(m)) {
                ui.label(RichText::new("Beim ersten Mal werden die Modelle geladen (YuNet, SFace, open-closed-eye; zusammen etwa 40 MB).").small().color(Color32::from_gray(150)));
            }
            ui.horizontal_wrapped(|ui| {
                for f in CullFilter::ALL {
                    let on = self.filter.culling == Some(f);
                    if ui.selectable_label(on, format!("{} ({})", f.label(), count(f))).clicked() {
                        cull = Some(if on { None } else { Some(f) });
                    }
                }
            });
            if !self.lists.persons.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new("Personen").strong());
                for p in &self.lists.persons {
                    let scope = Scope::Person(p.id);
                    let name = p.name.clone().unwrap_or_else(|| "Unbekannt".into());
                    if let Some((id, text)) = &mut self.library_ui.naming
                        && *id == p.id
                    {
                        let resp = ui.add(egui::TextEdit::singleline(text).hint_text("Name"));
                        if resp.lost_focus() {
                            rename = Some((*id, text.clone()));
                        } else {
                            resp.request_focus();
                        }
                        continue;
                    }
                    let resp = ui.selectable_label(self.scope == scope, format!("{}  {name} ({})", icon::USER_CIRCLE, p.count));
                    if resp.clicked() {
                        open = Some(scope);
                    }
                    if resp.double_clicked() {
                        self.library_ui.naming = Some((p.id, p.name.clone().unwrap_or_default()));
                    }
                    resp.on_hover_text("Doppelklick: benennen (der Name wird Keyword der Bilder)");
                }
            }
        });
        if let Some((id, name)) = rename {
            self.library_ui.naming = None;
            self.name_person(id, &name);
        }
        if let Some(again) = analyze {
            self.analyze_view(again);
        }
        if let Some(c) = cull {
            self.filter.culling = c;
            self.refilter();
        }
        if let Some(s) = open {
            self.open_scope(s);
        }
    }
}
