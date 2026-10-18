//! Faces and culling in the library: analysing the shown images (faces, closed eyes,
//! sharpness), filtering by the result, and the people recognized (see `sk_ai::faces`).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{Color32, Rect, RichText, Sense, TextureHandle, Ui, Vec2};
use sk_core::catalog::{Catalog, CullFilter, FaceRecord, Person, PersonFace, Scope};

use crate::app::App;
use crate::theme::icon;
use crate::widgets::section_help_ai;

/// The picture faces are found in: the camera preview is enough (and fast); without one, a
/// rendering. About 1600 px are plenty for faces and sharpness.
fn analysis_image(path: &std::path::Path) -> anyhow::Result<::image::DynamicImage> {
    let img = match sk_core::raw::embedded_preview(path) {
        Ok(img) if img.width().max(img.height()) >= 800 => img,
        _ => {
            let (meta, preview) = sk_core::raw::load_preview(path, 1600)?;
            let r = sk_core::pipeline::Processor::new(&meta, &sk_core::params::Adjustments::default()).render(&preview, None, None);
            ::image::DynamicImage::ImageRgba8(::image::RgbaImage::from_raw(r.width as u32, r.height as u32, r.rgba).expect("size"))
        }
    };
    Ok(if img.width().max(img.height()) > 2000 { img.resize(1600, 1600, ::image::imageops::FilterType::Triangle) } else { img })
}

/// Analyses one image.
fn analyze_file(models: &sk_ai::faces::FaceModels, path: &std::path::Path) -> anyhow::Result<sk_ai::faces::Analysis> {
    let rgb8 = analysis_image(path)?.to_rgb8();
    let rgb: Vec<[f32; 3]> = rgb8.pixels().map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]).collect();
    models.analyze(&sk_ai::faces::Image { width: rgb8.width() as usize, height: rgb8.height() as usize, rgb: &rgb })
}

/// Edge of a face thumbnail in pixels.
const FACE_PX: u32 = 96;
/// Size of a person tile in the list.
const FACE_TILE: f32 = 64.0;

/// A face cut out square (with some margin) from the picture it was found in.
#[cfg(test)]
pub(crate) fn face_thumbnail(path: &std::path::Path, bbox: [f32; 4]) -> anyhow::Result<egui::ColorImage> {
    Ok(crop_face(&analysis_image(path)?, bbox))
}

/// The face at `bbox` (normalized x, y, w, h) of `img`, square with half its size as margin.
fn crop_face(img: &::image::DynamicImage, bbox: [f32; 4]) -> egui::ColorImage {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let [x, y, bw, bh] = bbox;
    let side = (bw * w).max(bh * h) * 1.5;
    let (cx, cy) = ((x + bw / 2.0) * w, (y + bh / 2.0) * h);
    let side = side.min(w).min(h).max(8.0);
    let left = (cx - side / 2.0).clamp(0.0, w - side);
    let top = (cy - side / 2.0).clamp(0.0, h - side);
    let crop = img.crop_imm(left as u32, top as u32, side as u32, side as u32).resize_exact(FACE_PX, FACE_PX, ::image::imageops::FilterType::Triangle).to_rgba8();
    egui::ColorImage::from_rgba_unmultiplied([FACE_PX as usize, FACE_PX as usize], crop.as_raw())
}

/// Faces of the persons, cut out in the background.
pub struct FaceThumbs {
    /// Per person sample: its face, or `None` if it could not be cut out.
    textures: HashMap<String, Option<TextureHandle>>,
    pending: HashSet<String>,
    tx: Sender<(String, Option<egui::ColorImage>)>,
    rx: Receiver<(String, Option<egui::ColorImage>)>,
}

impl Default for FaceThumbs {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self { textures: HashMap::new(), pending: HashSet::new(), tx, rx }
    }
}

/// Identifies a person's sample face (it changes when images are analysed again).
fn face_key(p: &Person) -> Option<String> {
    p.sample.as_ref().map(|(path, b)| format!("{}:{}:{:?}", p.id, path.display(), b))
}

/// Identifies one face of a person (see `Catalog::person_faces`).
fn single_face_key(f: &PersonFace) -> String {
    format!("face {}:{}:{:?}", f.id, f.path.display(), f.bbox)
}

impl FaceThumbs {
    /// Takes the faces cut out meanwhile and asks for those of `persons` and `faces` still
    /// missing.
    fn update(&mut self, ctx: &egui::Context, persons: &[Person], faces: &[PersonFace]) {
        for (key, image) in self.rx.try_iter() {
            self.pending.remove(&key);
            let tex = image.map(|i| ctx.load_texture(format!("face:{key}"), i, egui::TextureOptions::LINEAR));
            self.textures.insert(key, tex);
        }
        let wanted = persons
            .iter()
            .filter_map(|p| Some((face_key(p)?, p.sample.clone()?)))
            .chain(faces.iter().map(|f| (single_face_key(f), (f.path.clone(), f.bbox))));
        let mut missing: Vec<(String, PathBuf, [f32; 4])> =
            wanted.filter(|(key, _)| !self.textures.contains_key(key) && !self.pending.contains(key)).map(|(key, (path, bbox))| (key, path, bbox)).collect();
        if missing.is_empty() {
            return;
        }
        for (key, _, _) in &missing {
            self.pending.insert(key.clone());
        }
        // Faces of the same image one after the other: it is read once.
        missing.sort_by(|a, b| a.1.cmp(&b.1));
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut loaded: Option<(PathBuf, Option<::image::DynamicImage>)> = None;
            for (key, path, bbox) in missing {
                if loaded.as_ref().is_none_or(|(p, _)| *p != path) {
                    let img = crate::guard::catch(|| analysis_image(&path).map_err(|e| log::warn!("Gesicht {}: {e:#}", path.display())).ok()).ok().flatten();
                    loaded = Some((path.clone(), img));
                }
                let image = loaded.as_ref().and_then(|(_, img)| img.as_ref()).map(|img| crop_face(img, bbox));
                if tx.send((key, image)).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
    }

    fn get(&self, p: &Person) -> Option<&TextureHandle> {
        face_key(p).and_then(|k| self.textures.get(&k)).and_then(|t| t.as_ref())
    }

    fn get_face(&self, f: &PersonFace) -> Option<&TextureHandle> {
        self.textures.get(&single_face_key(f)).and_then(|t| t.as_ref())
    }
}

/// Faces being dragged onto a person (see `App::move_faces`).
#[derive(Clone, Debug)]
struct FacesDrag(Vec<i64>);

/// The faces of the chosen person, to split off those that belong to someone else.
#[derive(Default)]
pub struct PersonFaces {
    /// Of which person, and its faces.
    shown: Option<(i64, Vec<PersonFace>)>,
    /// Chosen faces (ids).
    selected: HashSet<i64>,
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

    /// Gives faces of the person `from` to another person, or (`to` = `None`) to a new one that
    /// is then named. The keywords follow: images that no longer show the person lose its name,
    /// those of the moved faces get the name of the person they now belong to.
    pub fn move_faces(&mut self, from: i64, faces: &[i64], to: Option<i64>) {
        let Some(c) = &self.catalog else { return };
        let all = c.person_faces(from).unwrap_or_default();
        let moved_paths: Vec<PathBuf> = all.iter().filter(|f| faces.contains(&f.id)).map(|f| f.path.clone()).collect();
        if moved_paths.is_empty() {
            return;
        }
        let from_name = c.person_name(from);
        let target = match c.move_faces(faces, to) {
            Ok(t) => t,
            Err(e) => {
                self.status = format!("Gesichter verschieben fehlgeschlagen: {e:#}");
                return;
            }
        };
        let to_name = c.person_name(target);
        let still: HashSet<PathBuf> = c.person_images(from).unwrap_or_default().into_iter().collect();
        // Images that no longer show the person lose its name.
        if let Some(name) = from_name {
            let lost: Vec<PathBuf> = moved_paths.iter().filter(|p| !still.contains(*p)).cloned().collect();
            self.update_meta(&lost, |sc| sc.keywords.retain(|k| !k.eq_ignore_ascii_case(&name)));
        }
        if let Some(name) = &to_name {
            self.update_meta(&moved_paths, |sc| {
                if !sc.keywords.iter().any(|k| k.eq_ignore_ascii_case(name)) {
                    sc.keywords.push(name.clone());
                    sc.keywords.sort_by_key(|s| s.to_lowercase());
                }
            });
        }
        let n = faces.len();
        let what = if n == 1 { "1 Gesicht".to_string() } else { format!("{n} Gesichter") };
        self.status = match (to, &to_name) {
            (None, _) => format!("{what} als neue Person abgetrennt: Namen eingeben"),
            (Some(_), Some(name)) => format!("{what} zu „{name}“ verschoben"),
            (Some(_), None) => format!("{what} zu einer anderen Person verschoben"),
        };
        self.refresh_catalog_lists();
        // Read again (the faces changed).
        self.library_ui.person_faces = PersonFaces::default();
        if to.is_none() {
            // A new person: right away to be named.
            self.library_ui.naming = Some((target, String::new()));
        }
        // The chosen person may be gone (all its faces moved).
        if self.catalog.as_ref().is_some_and(|c| c.person_images(from).map(|v| v.is_empty()).unwrap_or(true)) {
            self.open_scope(Scope::Person(target));
        } else {
            self.reload_entries();
        }
    }

    /// The faces of the chosen person, to choose and split off those that belong to someone
    /// else; returns what to move where.
    fn person_faces_ui(&mut self, ui: &mut Ui, person: &Person, faces: &[PersonFace]) -> Option<(Vec<i64>, Option<i64>)> {
        let mut action = None;
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Gesichter von „{}“ ({})", person_name(person), faces.len())).strong());
        })
        .response
        .on_hover_text("Falsch zugeordnete Gesichter auf die richtige Person ziehen oder abtrennen (Strg+Klick für mehrere). Doppelklick öffnet das Bild.");
        let mut open_image = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
            for f in faces {
                let selected = self.library_ui.person_faces.selected.contains(&f.id);
                let resp = face_tile(ui, self.library_ui.faces.get_face(f), selected);
                // Dragging a chosen face takes all chosen ones, another one only itself.
                let drag: Vec<i64> = if selected { self.library_ui.person_faces.selected.iter().copied().collect() } else { vec![f.id] };
                resp.dnd_set_drag_payload(FacesDrag(drag.clone()));
                if resp.dragged() {
                    drag_ghost(ui, self.library_ui.faces.get_face(f));
                }
                let resp = resp.on_hover_text(sk_core::variant::display_name(&f.path));
                if resp.double_clicked() {
                    open_image = Some(f.path.clone());
                } else if resp.clicked() {
                    let sel = &mut self.library_ui.person_faces.selected;
                    if ui.input(|i| i.modifiers.command) {
                        if !sel.remove(&f.id) {
                            sel.insert(f.id);
                        }
                    } else if selected && sel.len() == 1 {
                        sel.clear();
                    } else {
                        sel.clear();
                        sel.insert(f.id);
                    }
                }
            }
        });
        let n = self.library_ui.person_faces.selected.len();
        ui.horizontal(|ui| {
            let text = if n > 1 { format!("{}  Als neue Person ({n})", icon::USER_PLUS) } else { format!("{}  Als neue Person", icon::USER_PLUS) };
            if ui
                .add_enabled(n > 0 && n < faces.len(), egui::Button::new(text))
                .on_hover_text("Die gewählten Gesichter gehören zu jemand anderem: sie werden eine eigene Person")
                .on_disabled_hover_text(if n == 0 { "Erst Gesichter anklicken, die zu jemand anderem gehören" } else { "Alle Gesichter gewählt: dann einfach die Person umbenennen" })
                .clicked()
            {
                action = Some((self.library_ui.person_faces.selected.iter().copied().collect(), None));
            }
            if n > 0 && ui.button("Auswahl aufheben").clicked() {
                self.library_ui.person_faces.selected.clear();
            }
        });
        if let Some(p) = open_image {
            self.select_path(&p, egui::Modifiers::NONE);
        }
        action
    }

    /// Merges the person `from` into `into` (one person recognized as two): the faces, the
    /// images and the name (`into` keeps its own, or takes that of `from`). The keyword of the
    /// images follows the name.
    pub fn merge_persons(&mut self, into: i64, from: i64) {
        let Some(c) = &self.catalog else { return };
        let from_name = c.person_name(from);
        let paths = match c.merge_persons(into, from) {
            Ok(p) => p,
            Err(e) => {
                self.status = format!("Zusammenführen fehlgeschlagen: {e:#}");
                return;
            }
        };
        let name = c.person_name(into);
        self.update_meta(&paths, |sc| {
            if let Some(o) = from_name.as_ref().filter(|o| name.as_ref().is_none_or(|n| !n.eq_ignore_ascii_case(o))) {
                sc.keywords.retain(|k| !k.eq_ignore_ascii_case(o));
            }
            if let Some(n) = &name
                && !sc.keywords.iter().any(|k| k.eq_ignore_ascii_case(n))
            {
                sc.keywords.push(n.clone());
                sc.keywords.sort_by_key(|s| s.to_lowercase());
            }
        });
        self.status = format!("Personen zusammengeführt: {}", name.as_deref().unwrap_or("Unbenannt"));
        self.refresh_catalog_lists();
        if self.scope == Scope::Person(from) {
            self.open_scope(Scope::Person(into));
        }
    }

    /// Asks before two persons are merged (it can't be undone).
    pub(crate) fn person_dialogs(&mut self, ctx: &egui::Context) {
        let Some((into, from)) = self.library_ui.merging else { return };
        let find = |id: i64| self.lists.persons.iter().find(|p| p.id == id).cloned();
        let (Some(a), Some(b)) = (find(into), find(from)) else {
            self.library_ui.merging = None;
            return;
        };
        let (mut confirmed, mut cancel) = (false, false);
        let dialog = crate::dialog::Dialog::new("merge_persons", icon::USERS, "Personen zusammenführen").subtitle("Lässt sich nicht rückgängig machen").width(420.0);
        let modal = dialog.show(ctx, |ui| {
            crate::dialog::well(ui, |ui| {
                ui.horizontal(|ui| {
                    person_tile(ui, &b, self.library_ui.faces.get(&b), false);
                    ui.add_space(6.0);
                    ui.label(RichText::new(icon::ARROW_RIGHT).size(20.0).color(crate::theme::TEXT_DIM));
                    ui.add_space(6.0);
                    person_tile(ui, &a, self.library_ui.faces.get(&a), true);
                });
            });
            ui.add_space(4.0);
            let kept = a.name.clone().or(b.name.clone()).unwrap_or_else(|| "Unbenannt".into());
            ui.label(format!(
                "Beide Gesichter gehören dann zu einer Person „{kept}“ ({}).",
                crate::widgets::image_count(a.count + b.count)
            ));
            crate::dialog::footer(ui, |ui| {
                if ui.add(crate::dialog::danger("Zusammenführen")).clicked() {
                    confirmed = true;
                }
                if ui.add(crate::dialog::secondary("Abbrechen")).clicked() {
                    cancel = true;
                }
            });
        });
        if confirmed {
            self.merge_persons(into, from);
        }
        if confirmed || cancel || modal.should_close() {
            self.library_ui.merging = None;
        }
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
        // Faces of the chosen person to give to another person (or to a new one).
        let mut move_faces: Option<(Vec<i64>, Option<i64>)> = None;
        let chosen_person = match self.scope {
            Scope::Person(id) => Some(id),
            _ => None,
        };
        let shown = self.visible.len();
        let analysed = self.visible.iter().filter(|&&i| self.entries[i].culling.is_some()).count();
        let counts: Vec<usize> = CullFilter::ALL.iter().map(|f| self.entries.iter().filter(|r| f.matches(r.culling.as_ref())).count()).collect();
        let count = |f: CullFilter| CullFilter::ALL.iter().position(|x| *x == f).map_or(0, |i| counts[i]);
        let help = "Erkennt Gesichter, geschlossene Augen und unscharfe Bilder und ordnet Gesichter Personen zu.\n\nBeim ersten Mal werden etwa 40 MB geladen.";
        section_help_ai(ui, "Gesichter & Sichtung", help, None, |ui| {
            ui.horizontal(|ui| {
                if ui.button(format!("{}  Ansicht analysieren", icon::SCAN)).on_hover_text("Noch nicht analysierte Bilder dieser Ansicht").clicked() {
                    analyze = Some(false);
                }
                if analysed > 0 && ui.small_button("Erneut").on_hover_text("Alle Bilder dieser Ansicht neu analysieren").clicked() {
                    analyze = Some(true);
                }
            });
            ui.label(RichText::new(format!("{analysed} von {shown} analysiert")).small().color(Color32::from_gray(150)));
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
                // The chosen person's faces (read again when another person is chosen).
                let chosen = match self.scope {
                    Scope::Person(id) => Some(id),
                    _ => None,
                };
                if self.library_ui.person_faces.shown.as_ref().map(|(id, _)| *id) != chosen {
                    self.library_ui.person_faces = PersonFaces {
                        shown: chosen.and_then(|id| Some((id, self.catalog.as_ref()?.person_faces(id).ok()?))),
                        selected: HashSet::new(),
                    };
                }
                let faces: Vec<PersonFace> = self.library_ui.person_faces.shown.as_ref().map(|(_, f)| f.clone()).unwrap_or_default();
                self.library_ui.faces.update(ui.ctx(), &self.lists.persons, &faces);
                // Being named: a text field above the faces.
                if let Some((id, text)) = &mut self.library_ui.naming {
                    let resp = ui.add(egui::TextEdit::singleline(text).hint_text("Name").desired_width(ui.available_width()));
                    if resp.lost_focus() {
                        rename = Some((*id, text.clone()));
                    } else {
                        resp.request_focus();
                    }
                }
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                    let persons = &self.lists.persons;
                    for p in persons {
                        let scope = Scope::Person(p.id);
                        let on = self.scope == scope;
                        let naming = self.library_ui.naming.as_ref().is_some_and(|(id, _)| *id == p.id);
                        let face = self.library_ui.faces.get(p);
                        let resp = person_tile(ui, p, face, on || naming);
                        // Dragged onto another person: the two are one.
                        resp.dnd_set_drag_payload(p.id);
                        if resp.dragged() {
                            drag_ghost(ui, face);
                        }
                        if let Some(from) = resp.dnd_release_payload::<i64>()
                            && *from != p.id
                        {
                            self.library_ui.merging = Some((p.id, *from));
                        }
                        // Faces of another person dropped here: they belong to this one.
                        if let Some(drag) = resp.dnd_release_payload::<FacesDrag>()
                            && chosen != Some(p.id)
                        {
                            move_faces = Some((drag.0.clone(), Some(p.id)));
                        }
                        let resp = resp.on_hover_text(format!(
                            "{}: {}\nKlick: nur diese Person, noch einmal: alle\n\
                             Doppelklick: benennen (wird Keyword der Bilder)\n\
                             Auf eine andere Person ziehen: zusammenführen",
                            person_name(p),
                            crate::widgets::image_count(p.count)
                        ));
                        resp.context_menu(|ui| {
                            if ui.button(format!("{}  Benennen …", icon::PENCIL_SIMPLE)).clicked() {
                                self.library_ui.naming = Some((p.id, p.name.clone().unwrap_or_default()));
                                ui.close();
                            }
                            ui.menu_button(format!("{}  Zusammenführen mit", icon::USERS), |ui| {
                                ui.label(RichText::new("Dieselbe Person, doppelt erkannt:").small().weak());
                                for other in persons.iter().filter(|o| o.id != p.id) {
                                    let text = format!("{} ({})", person_name(other), crate::widgets::image_count(other.count));
                                    let clicked = match self.library_ui.faces.get(other) {
                                        Some(t) => ui.add(egui::Button::image_and_text(egui::Image::new((t.id(), Vec2::splat(28.0))).corner_radius(4.0), text)).clicked(),
                                        None => ui.button(format!("{}  {text}", icon::USER_CIRCLE)).clicked(),
                                    };
                                    if clicked {
                                        self.library_ui.merging = Some((other.id, p.id));
                                        ui.close();
                                    }
                                }
                            });
                        });
                        if resp.double_clicked() {
                            // The first click has opened the person already; the second only names it.
                            self.library_ui.naming = Some((p.id, p.name.clone().unwrap_or_default()));
                        } else if resp.clicked() {
                            // Clicked again: back to all images of the folder.
                            open = Some(if on { Scope::Folder { path: self.folder.clone(), recursive: false } } else { scope });
                        }
                    }
                });
                if let Some(person) = chosen.and_then(|id| self.lists.persons.iter().find(|p| p.id == id)).cloned()
                    && faces.len() > 1
                    && let Some(action) = self.person_faces_ui(ui, &person, &faces)
                {
                    move_faces = Some(action);
                }
            }
        });
        if let Some((id, name)) = rename {
            self.library_ui.naming = None;
            self.name_person(id, &name);
        }
        if let Some((faces, to)) = move_faces
            && let Some(from) = chosen_person
        {
            self.move_faces(from, &faces, to);
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

/// Edge of a face in the list of a person's faces.
const FACE_SMALL: f32 = 48.0;

/// One face of a person: chosen ones with a frame.
fn face_tile(ui: &mut Ui, face: Option<&TextureHandle>, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(FACE_SMALL), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, Color32::from_gray(30));
    match face {
        Some(t) => {
            egui::Image::new((t.id(), rect.size())).corner_radius(4.0).paint_at(ui, rect);
        }
        None => {
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, "…", egui::FontId::proportional(14.0), crate::theme::TEXT_FAINT);
        }
    }
    if selected {
        painter.rect_stroke(rect, 4.0, egui::Stroke::new(2.5, crate::theme::ACCENT), egui::StrokeKind::Inside);
        painter.circle_filled(rect.right_top() + Vec2::new(-8.0, 8.0), 6.0, crate::theme::ACCENT);
        painter.text(rect.right_top() + Vec2::new(-8.0, 8.0), egui::Align2::CENTER_CENTER, icon::CHECK, egui::FontId::proportional(9.0), Color32::WHITE);
    } else if resp.hovered() {
        painter.rect_stroke(rect, 4.0, egui::Stroke::new(1.0, crate::theme::TEXT_DIM), egui::StrokeKind::Inside);
    }
    resp
}

fn person_name(p: &Person) -> String {
    p.name.clone().unwrap_or_else(|| "Unbenannt".into())
}

/// The face of a person being dragged, at the pointer.
fn drag_ghost(ui: &Ui, face: Option<&TextureHandle>) {
    let Some(pos) = ui.ctx().pointer_interact_pos() else { return };
    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("person_drag")));
    let r = Rect::from_center_size(pos + Vec2::new(18.0, 18.0), Vec2::splat(44.0));
    painter.rect_filled(r.expand(2.0), 6.0, crate::theme::ACCENT);
    match face {
        Some(t) => {
            painter.image(t.id(), r, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE.gamma_multiply(0.9));
        }
        None => {
            painter.text(r.center(), egui::Align2::CENTER_CENTER, icon::USER_CIRCLE, egui::FontId::proportional(26.0), Color32::WHITE);
        }
    }
}

/// A person as a tile: the face (or a placeholder), the name below, the number of images in
/// the corner. Highlighted while another person is dragged over it.
fn person_tile(ui: &mut Ui, p: &Person, face: Option<&TextureHandle>, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(FACE_TILE, FACE_TILE + 16.0), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let img = Rect::from_min_size(rect.min, Vec2::splat(FACE_TILE));
    painter.rect_filled(img, 6.0, Color32::from_gray(30));
    match face {
        Some(t) => {
            egui::Image::new((t.id(), img.size())).corner_radius(6.0).paint_at(ui, img);
        }
        None => {
            painter.text(img.center(), egui::Align2::CENTER_CENTER, icon::USER_CIRCLE, egui::FontId::proportional(30.0), crate::theme::TEXT_FAINT);
        }
    }
    let drop_here = resp.dnd_hover_payload::<i64>().is_some_and(|from| *from != p.id) || resp.dnd_hover_payload::<FacesDrag>().is_some();
    if drop_here {
        painter.rect_filled(img, 6.0, crate::theme::ACCENT.gamma_multiply(0.25));
        painter.rect_stroke(img, 6.0, egui::Stroke::new(2.5, crate::theme::ACCENT), egui::StrokeKind::Inside);
    } else if selected {
        painter.rect_stroke(img, 6.0, egui::Stroke::new(2.0, crate::theme::ACCENT), egui::StrokeKind::Inside);
    } else if resp.hovered() {
        painter.rect_stroke(img, 6.0, egui::Stroke::new(1.0, crate::theme::TEXT_DIM), egui::StrokeKind::Inside);
    }
    // Number of images.
    let count = painter.layout_no_wrap(p.count.to_string(), egui::FontId::proportional(10.5), Color32::WHITE);
    let badge = Rect::from_min_size(img.right_bottom() - count.size() - Vec2::new(8.0, 6.0), count.size() + Vec2::new(6.0, 2.0));
    painter.rect_filled(badge, 3.0, Color32::from_black_alpha(170));
    painter.galley(badge.min + Vec2::new(3.0, 1.0), count, Color32::WHITE);
    // Name.
    let (name, color) = match &p.name {
        Some(n) => (n.clone(), if selected { Color32::WHITE } else { crate::theme::TEXT }),
        None => ("Unbenannt".to_string(), crate::theme::TEXT_FAINT),
    };
    let mut job = egui::text::LayoutJob::simple_singleline(name, egui::FontId::proportional(11.0), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(FACE_TILE);
    let galley = painter.layout_job(job);
    painter.galley(egui::pos2(img.center().x - galley.size().x / 2.0, img.bottom() + 2.0), galley, color);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_is_cut_out_square_around_its_box() {
        // A white square "face" on black, in the right half.
        let img = ::image::DynamicImage::ImageRgb8(::image::RgbImage::from_fn(400, 200, |x, y| {
            if (280..320).contains(&x) && (80..120).contains(&y) { ::image::Rgb([255, 255, 255]) } else { ::image::Rgb([0, 0, 0]) }
        }));
        let face = crop_face(&img, [0.7, 0.4, 0.1, 0.2]);
        assert_eq!(face.size, [FACE_PX as usize, FACE_PX as usize]);
        // White in the middle, black at the corners (the margin).
        let at = |x: usize, y: usize| face.pixels[y * face.size[0] + x].r();
        assert!(at(48, 48) > 200, "{}", at(48, 48));
        assert!(at(3, 3) < 40 && at(92, 92) < 40);
    }

    #[test]
    fn face_at_the_edge_stays_inside_the_picture() {
        let img = ::image::DynamicImage::ImageRgb8(::image::RgbImage::new(300, 200));
        assert_eq!(crop_face(&img, [0.95, 0.9, 0.1, 0.2]).size, [FACE_PX as usize, FACE_PX as usize]);
    }
}
