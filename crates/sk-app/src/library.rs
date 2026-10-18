//! Library: catalog scopes (folders, albums, smart albums), filtering, sorting,
//! metadata edits (rating, label, keywords) and styles.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sk_core::catalog::{self, Filter, ImageRecord, Scope, SortKey};
use sk_core::params::Adjustments;
use sk_core::sidecar::{self, ColorLabel, Sidecar};
use sk_core::styles::{self, Style};
use egui::{Color32, RichText, Ui};

use crate::app::App;
use crate::widgets::{self, section};
use crate::workers::IndexJob;

/// Catalog-derived lists shown in the library tab.
#[derive(Default)]
pub struct CatalogLists {
    pub albums: Vec<catalog::Album>,
    pub folders: Vec<(PathBuf, usize)>,
    pub keywords: Vec<(String, usize)>,
    pub total: usize,
    /// Session favorites (folders).
    pub favorites: Vec<PathBuf>,
    /// People recognized by face (named, or seen in several images).
    pub persons: Vec<catalog::Person>,
}

/// Transient UI state of the library tab.
#[derive(Default)]
pub struct LibraryUi {
    pub new_album: String,
    pub renaming: Option<(i64, String)>,
    pub new_style: String,
    /// The tools a new style takes; `None` until chosen: the edited ones.
    pub new_style_tools: Option<std::collections::BTreeSet<sk_core::tools::Tool>>,
    /// Person being named, with the text typed so far.
    pub naming: Option<(i64, String)>,
    /// The faces shown for the persons.
    pub faces: crate::culling::FaceThumbs,
    /// Two persons to be merged once confirmed: (into, from).
    pub merging: Option<(i64, i64)>,
    /// The faces of the chosen person and those chosen among them.
    pub person_faces: crate::culling::PersonFaces,
}

fn record_from_sidecar(path: PathBuf, sc: Sidecar) -> ImageRecord {
    ImageRecord {
        id: 0,
        name: sk_core::variant::display_name(&path),
        path,
        capture_date: None,
        camera: None,
        lens: None,
        iso: None,
        rating: sc.rating,
        color_label: sc.color_label,
        keywords: sc.keywords,
        stack: sc.stack,
        culling: None,
        persons: Vec::new(),
    }
}

pub fn scope_label(scope: &Scope, lists: &CatalogLists, session: Option<&sk_core::session::Session>) -> String {
    // Session folders are shown by their label ("Auswahl" rather than "Selects").
    if let (Scope::Folder { path, recursive: false }, Some(s)) = (scope, session)
        && let Some(f) = s.folder_of(path).filter(|f| s.folder(*f) == *path)
    {
        return f.label().into();
    }
    match scope {
        Scope::All => "Alle Bilder".into(),
        Scope::LastImport => "Letzter Import".into(),
        Scope::Folder { path, recursive } => {
            let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().to_string());
            if *recursive { format!("{name} (mit Unterordnern)") } else { name }
        }
        Scope::Album(id) => lists.albums.iter().find(|a| a.id == *id).map_or("Album".into(), |a| a.name.clone()),
        Scope::Person(id) => lists.persons.iter().find(|p| p.id == *id).and_then(|p| p.name.clone()).unwrap_or_else(|| "Person".into()),
    }
}

impl App {
    pub fn refresh_catalog_lists(&mut self) {
        let Some(c) = &self.catalog else { return };
        self.lists = CatalogLists {
            albums: c.albums().unwrap_or_default(),
            folders: c.folders().unwrap_or_default(),
            keywords: c.keywords().unwrap_or_default(),
            total: c.image_count().unwrap_or(0),
            favorites: c.favorites().unwrap_or_default(),
            persons: c.persons(2).unwrap_or_default(),
        };
    }

    /// Switches the browser to a folder (like a session folder).
    pub fn open_folder(&mut self, folder: PathBuf) {
        let folder = folder.canonicalize().unwrap_or(folder);
        if !folder.is_dir() {
            self.status = format!("Ordner nicht lesbar: {}", folder.display());
            return;
        }
        self.folder_input = folder.display().to_string();
        self.subdirs = std::fs::read_dir(&folder)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
            // The session's own data folder (cache) is not for browsing.
            .filter(|p| !self.session.as_ref().is_some_and(|s| *p == s.root.join(sk_core::session::APP_FOLDER)))
            .collect();
        self.subdirs.sort();
        self.folder = folder.clone();
        if self.session.is_none() {
            self.catalog_folder = folder.clone();
        }
        self.save_settings();
        self.open_scope(Scope::Folder { path: folder.clone(), recursive: false });
        // Index silently so that dates, cameras and search work for this folder;
        // in a session only its own folders belong in its database.
        let paths = catalog::scan_folder(&folder, false);
        let belongs = self.session.as_ref().is_none_or(|s| folder.starts_with(&s.root));
        if !paths.is_empty() && belongs {
            self.workers.index(IndexJob { catalog: self.catalog_path(), paths, import: false, copy_to: None });
        }
    }

    pub fn open_scope(&mut self, scope: Scope) {
        self.commit_and_save();
        self.scope = scope;
        self.reload_entries();
        // After starting: the image that was selected last time, if it is shown here.
        let restore = self.restore_image.take().and_then(|p| self.index_of(&p)).filter(|i| self.visible.contains(i));
        let first = restore.or_else(|| self.visible.first().copied());
        let keep = self.current_path().is_some_and(|p| self.index_of(&p).is_some_and(|i| self.visible.contains(&i)));
        if !keep {
            self.close_current();
            if let Some(i) = first {
                let path = self.entries[i].path.clone();
                self.select_path(&path, egui::Modifiers::NONE);
            }
        }
    }

    /// Imports a folder tree (shown as "last import"). In a session the files are copied
    /// into its Capture folder, unless they already are inside the session.
    pub fn import_folder(&mut self, folder: &Path) {
        let paths = catalog::scan_folder(folder, true);
        if paths.is_empty() {
            self.status = "Keine RAW-Dateien gefunden".into();
            return;
        }
        let copy_to = self
            .session
            .as_ref()
            .filter(|s| !folder.starts_with(&s.root))
            .map(|s| s.folder(sk_core::session::SessionFolder::Capture));
        self.status = match &copy_to {
            Some(_) => format!("Kopiere {} in die Session…", crate::widgets::image_count(paths.len())),
            None => format!("Importiere {}…", crate::widgets::image_count(paths.len())),
        };
        self.workers.index(IndexJob { catalog: self.catalog_path(), paths, import: true, copy_to });
    }

    /// Re-reads the entries of the current scope, keeping selection by path.
    pub fn reload_entries(&mut self) {
        let mut records = match &self.scope {
            Scope::Folder { path, recursive: false } => {
                // The file system is authoritative; the catalog adds metadata where indexed.
                let indexed: HashMap<PathBuf, ImageRecord> = self
                    .catalog
                    .as_ref()
                    .and_then(|c| c.query(&self.scope, &Filter::default(), SortKey::Name, false).ok())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|r| (r.path.clone(), r))
                    .collect();
                sidecar::load_all(&catalog::scan_folder(path, false))
                    .into_iter()
                    .map(|(p, sc)| indexed.get(&p).cloned().unwrap_or_else(|| record_from_sidecar(p, sc)))
                    .collect()
            }
            scope => self
                .catalog
                .as_ref()
                .and_then(|c| c.query(scope, &Filter::default(), SortKey::Name, false).ok())
                .unwrap_or_default(),
        };
        sort_records(&mut records, self.sort, self.sort_desc);
        self.entries = records;
        self.forget_unlisted_thumbs();
        self.selection.retain(|p| self.entries.iter().any(|e| &e.path == p));
        self.refilter();
        self.refresh_catalog_lists();
        self.request_proxies();
    }

    pub fn resort(&mut self) {
        sort_records(&mut self.entries, self.sort, self.sort_desc);
        self.refilter();
    }

    pub fn refilter(&mut self) {
        let matching = self.entries.iter().enumerate().filter(|(_, r)| self.filter.matches(r)).map(|(i, _)| i).collect();
        self.visible = self.arrange_stacks(matching);
    }

    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.entries.iter().position(|e| e.path == path)
    }

    /// Applies a metadata change to the given images (sidecar + catalog + entries).
    pub fn update_meta(&mut self, paths: &[PathBuf], f: impl Fn(&mut Sidecar)) {
        let current = self.current_path();
        for path in paths {
            let sc = if Some(path) == current.as_ref() {
                let cur = self.current.as_mut().expect("current");
                let mut sc = Sidecar {
                    rating: cur.rating,
                    color_label: cur.color_label,
                    keywords: cur.keywords.clone(),
                    ..Default::default()
                };
                f(&mut sc);
                cur.rating = sc.rating;
                cur.color_label = sc.color_label;
                cur.keywords = sc.keywords.clone();
                self.sidecar_dirty = true;
                sc
            } else {
                let mut sc = match self.sidecar_for_update(path) {
                    Ok(sc) => sc,
                    Err(e) => {
                        self.status = format!("Nicht gespeichert: {e:#}");
                        continue;
                    }
                };
                f(&mut sc);
                self.save_sidecar(path, &sc);
                sc
            };
            if let Some(i) = self.index_of(path) {
                let e = &mut self.entries[i];
                e.rating = sc.rating;
                e.color_label = sc.color_label;
                e.keywords = sc.keywords.clone();
            }
        }
        self.commit_and_save();
        self.refilter();
        self.refresh_catalog_lists();
    }

    /// Writes rating, label and keywords of the selected images into XMP files.
    pub fn write_xmp_selection(&mut self) {
        self.commit_and_save();
        let files = sk_core::variant::files_of(&self.selected_paths());
        let failed: Vec<String> = files.iter().filter_map(|f| sidecar::write_xmp(f).err().map(|e| format!("{e:#}"))).collect();
        self.status = match failed.first() {
            None => format!("XMP geschrieben: {}", crate::widgets::image_count(files.len())),
            Some(e) => format!("XMP nicht geschrieben ({} von {}): {e}", failed.len(), files.len()),
        };
    }

    /// Takes rating, label and keywords of the selected images from their XMP files again.
    pub fn read_xmp_selection(&mut self) {
        self.commit_and_save();
        let files = sk_core::variant::files_of(&self.selected_paths());
        let current = self.current_path();
        let mut read = 0;
        for f in &files {
            if sk_core::xmp::existing(f).is_none() {
                continue;
            }
            if let Err(e) = sidecar::reread_xmp(f) {
                self.status = format!("{e:#}");
                continue;
            }
            read += 1;
            let sc = sidecar::load(f);
            // Keep it from now on (also if the XMP file is older than the settings).
            let _ = sidecar::save(f, &sc);
            if let Some(c) = &self.catalog {
                let _ = c.sync_sidecar(f, &sc);
            }
            if current.as_ref() == Some(f)
                && let Some(cur) = self.current.as_mut()
            {
                (cur.rating, cur.color_label, cur.keywords) = (sc.rating, sc.color_label, sc.keywords.clone());
            }
            if let Some(i) = self.index_of(f) {
                let e = &mut self.entries[i];
                (e.rating, e.color_label, e.keywords) = (sc.rating, sc.color_label, sc.keywords);
            }
        }
        self.status = if read == 0 { "Keine XMP-Dateien gefunden".into() } else { format!("XMP gelesen: {}", crate::widgets::image_count(read)) };
        self.refilter();
        self.refresh_catalog_lists();
    }

    /// Selected paths in browser order (the current image if nothing is selected).
    pub fn selected_paths(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> =
            self.visible.iter().map(|&i| &self.entries[i].path).filter(|p| self.selection.contains(*p)).cloned().collect();
        if v.is_empty()
            && let Some(p) = self.current_path()
        {
            v.push(p);
        }
        v
    }

    pub fn set_rating(&mut self, rating: u8) {
        let paths = self.selected_paths();
        self.update_meta(&paths, |sc| sc.rating = rating);
    }

    pub fn set_color_label(&mut self, label: ColorLabel) {
        let paths = self.selected_paths();
        self.update_meta(&paths, |sc| sc.color_label = label);
    }

    pub fn add_keyword(&mut self, keyword: &str) {
        let k = keyword.trim().to_string();
        if k.is_empty() {
            return;
        }
        let paths = self.selected_paths();
        self.update_meta(&paths, |sc| {
            if !sc.keywords.iter().any(|x| x.eq_ignore_ascii_case(&k)) {
                sc.keywords.push(k.clone());
                sc.keywords.sort_by_key(|s| s.to_lowercase());
            }
        });
    }

    pub fn remove_keyword(&mut self, keyword: &str) {
        let paths = self.selected_paths();
        self.update_meta(&paths, |sc| sc.keywords.retain(|x| !x.eq_ignore_ascii_case(keyword)));
    }

    /// Writes new adjustments to images that are not open (e.g. paste, styles).
    pub fn set_adjustments_on(&mut self, paths: &[PathBuf], f: impl Fn(&Adjustments) -> Adjustments) {
        let current = self.current_path();
        for path in paths {
            if Some(path) == current.as_ref() {
                self.edit(|a| *a = f(a));
            } else {
                // Results arriving late (matching to a reference) must not bring back a deleted
                // variant or write a sidecar next to a file that was moved away.
                let (file, n) = sk_core::variant::split(path);
                if !file.exists() || (n > 1 && !sk_core::variant::of_file(&file).contains(path)) {
                    continue;
                }
                let mut sc = match self.sidecar_for_update(path) {
                    Ok(sc) => sc,
                    Err(e) => {
                        self.status = format!("Nicht gespeichert: {e:#}");
                        continue;
                    }
                };
                sc.adjustments = f(&sc.adjustments);
                self.save_sidecar(path, &sc);
                self.thumb_requested.remove(path);
                self.compare_textures.remove(path);
                self.compare_requested.remove(path);
                self.focus_maps.remove(path);
                self.compare_details.remove(path);
                self.forget_prepared(path);
            }
        }
    }

    pub fn reload_styles(&mut self) {
        self.styles = styles::list_in(&styles::styles_dir());
    }

    // ---------------------------------------------------------------- UI

    pub fn library_tab(&mut self, ui: &mut Ui) {
        let mut open: Option<Scope> = None;
        let (mut do_import, mut do_cleanup) = (false, false);

        self.session_sections(ui);
        let title = if self.session.is_some() { "Session-Katalog" } else { "Katalog" };
        section(ui, title, None, |ui| {
            let mut entry = |ui: &mut Ui, scope: Scope, text: String| {
                if ui.selectable_label(self.scope == scope, text).clicked() {
                    open = Some(scope);
                }
            };
            entry(ui, Scope::All, format!("\u{E468}  Alle Bilder ({})", self.lists.total));
            entry(ui, Scope::LastImport, "\u{E1A0}  Letzter Import".into());
            ui.horizontal(|ui| {
                if self.session.is_none()
                    && ui.button("\u{E20C}  Ordner importieren").on_hover_text("Aktuellen Ordner inkl. Unterordnern in den Katalog aufnehmen").clicked()
                {
                    do_import = true;
                }
                if ui.button("\u{EC54}  Bereinigen").on_hover_text("Fehlende Dateien aus dem Katalog entfernen").clicked() {
                    do_cleanup = true;
                }
            });
            if let Some((done, total)) = self.index_progress {
                ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32).text(format!("Indexiere {done} / {total}")));
            }
        });

        if do_import {
            let folder = self.folder.clone();
            self.import_folder(&folder);
        }
        // Filtering is used most, so it comes right after the catalog.
        let mut refilter = false;
        let mut resort = false;
        section(ui, "Filter & Suche", None, |ui| {
            refilter |= ui
                .add(egui::TextEdit::singleline(&mut self.filter.text).hint_text("\u{E30C}  Name, Kamera, Objektiv, Keyword"))
                .changed();
            widgets::field(ui, "Mindestens", |ui| refilter |= widgets::stars(ui, &mut self.filter.min_rating, 16.0));
            widgets::combo_field(ui, "Farb-Label", self.filter.color_label.map_or("Alle", |l| l.label()), |ui| {
                    refilter |= ui.selectable_value(&mut self.filter.color_label, None, "Alle").changed();
                    for l in ColorLabel::ALL {
                        refilter |= ui.selectable_value(&mut self.filter.color_label, Some(l), l.label()).changed();
                    }
                });
            widgets::combo_field(ui, "Keyword", self.filter.keyword.clone().unwrap_or_else(|| "Alle".into()), |ui| {
                    refilter |= ui.selectable_value(&mut self.filter.keyword, None, "Alle").changed();
                    for (k, n) in &self.lists.keywords {
                        refilter |= ui.selectable_value(&mut self.filter.keyword, Some(k.clone()), format!("{k} ({n})")).changed();
                    }
                });
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("sort").selected_text(self.sort.label()).show_ui(ui, |ui| {
                    for s in SortKey::ALL {
                        resort |= ui.selectable_value(&mut self.sort, s, s.label()).changed();
                    }
                });
                if ui.button(if self.sort_desc { "\u{E446}" } else { "\u{E444}" }).on_hover_text("Sortierrichtung").clicked() {
                    self.sort_desc = !self.sort_desc;
                    resort = true;
                }
                if !self.filter.is_empty() && ui.button("\u{E4F6}  Filter").clicked() {
                    self.filter = Filter::default();
                    refilter = true;
                }
            });
            ui.label(RichText::new(format!("{} von {} Bildern", self.visible.len(), self.entries.len())).color(Color32::from_gray(150)));
        });
        if resort {
            self.resort();
        } else if refilter {
            self.refilter();
        }

        self.culling_section(ui);
        if do_cleanup && let Some(c) = &self.catalog {
            let n = c.remove_missing().unwrap_or(0);
            self.status = format!("{n} fehlende Einträge entfernt");
            self.reload_entries();
        }

        enum AlbumAction {
            Create(String, bool),
            Rename(i64, String),
            Delete(i64, String),
            AddSelection(i64),
            RemoveSelection(i64),
        }
        let mut action: Option<AlbumAction> = None;
        section(ui, "Alben", None, |ui| {
            for album in &self.lists.albums {
                let scope = Scope::Album(album.id);
                if let Some((id, name)) = &mut self.library_ui.renaming
                    && *id == album.id
                {
                    let resp = ui.text_edit_singleline(name);
                    if resp.lost_focus() {
                        // Esc keeps the old name (an empty one is ignored), as for layers.
                        let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                        action = Some(AlbumAction::Rename(*id, if esc { String::new() } else { name.trim().to_string() }));
                    } else {
                        resp.request_focus();
                    }
                    continue;
                }
                let icon = if album.smart.is_some() { "\u{E272}" } else { "\u{E25A}" };
                let resp = ui.selectable_label(self.scope == scope, format!("{icon}  {} ({})", album.name, album.count)).on_hover_text("Doppelklick: umbenennen");
                if resp.double_clicked() {
                    self.library_ui.renaming = Some((album.id, album.name.clone()));
                } else if resp.clicked() {
                    open = Some(scope);
                }
                resp.context_menu(|ui| {
                    if album.smart.is_none() && ui.button("Auswahl hinzufügen").clicked() {
                        action = Some(AlbumAction::AddSelection(album.id));
                        ui.close();
                    }
                    if ui.button("Umbenennen").clicked() {
                        self.library_ui.renaming = Some((album.id, album.name.clone()));
                        ui.close();
                    }
                    if ui.button("Löschen …").clicked() {
                        action = Some(AlbumAction::Delete(album.id, album.name.clone()));
                        ui.close();
                    }
                });
            }
            if self.lists.albums.is_empty() {
                ui.label(RichText::new("Noch keine Alben.").color(Color32::from_gray(150)));
            }
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.library_ui.new_album).hint_text("Name").desired_width(110.0));
                let name = self.library_ui.new_album.trim().to_string();
                if ui.add_enabled(!name.is_empty(), egui::Button::new("+ Album")).on_hover_text("Neues Album mit der Auswahl").clicked() {
                    action = Some(AlbumAction::Create(name.clone(), false));
                }
                if ui
                    .add_enabled(!name.is_empty() && !self.filter.is_empty(), egui::Button::new("+ Filteralbum"))
                    .on_hover_text("Album, das immer die Bilder des aktuellen Filters zeigt")
                    .clicked()
                {
                    action = Some(AlbumAction::Create(name, true));
                }
            });
            if let Scope::Album(id) = self.scope
                && self.lists.albums.iter().any(|a| a.id == id && a.smart.is_none())
                && ui.button("Auswahl aus Album entfernen").clicked()
            {
                action = Some(AlbumAction::RemoveSelection(id));
            }
        });
        if let Some(action) = action {
            let selected = self.selected_paths();
            if let Some(c) = &self.catalog {
                match action {
                    AlbumAction::Create(name, smart) => {
                        if smart {
                            let _ = c.create_album(&name, Some(&self.filter));
                        } else if let Ok(id) = c.create_album(&name, None) {
                            let _ = c.add_to_album(id, &selected);
                        }
                        self.library_ui.new_album.clear();
                    }
                    AlbumAction::Rename(id, name) => {
                        if !name.is_empty() {
                            let _ = c.rename_album(id, &name);
                        }
                        self.library_ui.renaming = None;
                    }
                    AlbumAction::Delete(id, name) => self.pending_deletion = Some(crate::deletion::Deletion::Album(id, name)),
                    AlbumAction::AddSelection(id) => {
                        let n = c.add_to_album(id, &selected).unwrap_or(0);
                        self.status = format!("{n} Bild(er) zum Album hinzugefügt");
                    }
                    AlbumAction::RemoveSelection(id) => {
                        let _ = c.remove_from_album(id, &selected);
                    }
                }
            }
            if open.is_none() && matches!(self.scope, Scope::Album(_)) {
                self.reload_entries();
            } else {
                self.refresh_catalog_lists();
            }
        }

        let mut open_folder: Option<PathBuf> = None;
        let mut browse = false;
        section(ui, "Ordner", None, |ui| {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("\u{E256}  Durchsuchen …").on_hover_text(crate::commands::hint("Ordner auswählen", "open-folder")).clicked() {
                        browse = true;
                    }
                    let resp = ui.add(egui::TextEdit::singleline(&mut self.folder_input).desired_width(ui.available_width()));
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        open_folder = Some(self.folder_input.clone().into());
                    }
                });
            });
            ui.horizontal(|ui| {
                if let Some(parent) = self.folder.parent()
                    && ui.button("\u{E08E}  Übergeordnet").clicked()
                {
                    open_folder = Some(parent.to_path_buf());
                }
                let rec = Scope::Folder { path: self.folder.clone(), recursive: true };
                if ui.selectable_label(self.scope == rec, "mit Unterordnern").clicked() {
                    open = Some(rec);
                }
            });
            if self.session.is_none() {
                self.hot_folder_ui(ui);
            }
            egui::ScrollArea::vertical().id_salt("subdirs").max_height(180.0).show(ui, |ui| {
                for dir in &self.subdirs {
                    let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    if ui.selectable_label(false, format!("\u{E24A}  {name}")).clicked() {
                        open_folder = Some(dir.clone());
                    }
                }
            });
            if !self.lists.folders.is_empty() {
                ui.collapsing("Katalogordner", |ui| {
                    for (dir, n) in &self.lists.folders {
                        let scope = Scope::Folder { path: dir.clone(), recursive: false };
                        if ui.selectable_label(self.scope == scope, format!("{} ({n})", dir.display())).clicked() {
                            open_folder = Some(dir.clone());
                        }
                    }
                });
            }
        });

        if browse {
            self.open_folder_dialog();
        } else if let Some(f) = open_folder {
            self.open_folder(f);
        } else if let Some(s) = open {
            self.open_scope(s);
        }
    }

    pub fn styles_tab(&mut self, ui: &mut Ui) {
        // (style, look only), see `styles::apply_look`.
        let mut apply: Option<(Style, bool)> = None;
        let mut delete: Option<String> = None;
        let mut hovered: Option<(Style, bool)> = None;
        let current = self.current.as_ref().map(|c| c.adjustments.clone());
        ui.horizontal(|ui| {
            ui.label("Ansicht");
            let mut list = self.style_list;
            ui.selectable_value(&mut list, false, "Vorschauen").on_hover_text("Jeder Stil als kleines Bild des aktuellen Fotos");
            ui.selectable_value(&mut list, true, "Liste");
            if list != self.style_list {
                self.style_list = list;
                self.save_settings();
            }
        });
        let looks: Vec<(Style, bool, String)> = self
            .film_looks
            .iter()
            .map(|(s, d)| (s.clone(), true, format!("{d}\n\nNachempfunden, keine offizielle Simulation. Belichtung, Weißabgleich und Korrekturen des Bildes bleiben erhalten.")))
            .collect();
        let own: Vec<(Style, bool, String)> = self
            .styles
            .iter()
            .map(|s| {
                let sets = match &s.tools {
                    Some(t) => format!("Setzt: {}", t.iter().map(|t| t.label()).collect::<Vec<_>>().join(", ")),
                    None => "Setzt alle Werkzeuge außer Zuschnitt, Drehen, Perspektive, Ebenen und Retusche".into(),
                };
                (s.clone(), false, sets)
            })
            .collect();
        let grid = !self.style_list && self.current.is_some();
        if !self.style_list && self.current.is_none() {
            crate::widgets::hint(ui, "Die Vorschauen erscheinen, sobald ein Bild geöffnet ist.");
        }
        if grid {
            self.pump_style_thumbs();
        }
        let mut take = |(c, h, d): crate::style_grid::Picks| {
            apply = apply.take().or(c);
            hovered = hovered.take().or(h);
            delete = delete.take().or(d);
        };
        let help = if grid {
            "Zeiger auf einen Stil: Vorschau im Bild. Ein Klick übernimmt ihn für die Auswahl."
        } else {
            "Zeiger auf einen Stil: Vorschau im Bild. „Anwenden“ übernimmt ihn für die Auswahl."
        };
        crate::widgets::section_help(ui, "Filmsimulationen", help, None, |ui| {
            if grid {
                take(self.style_tiles(ui, &looks));
            } else {
                take(style_rows(ui, &looks));
            }
        });
        section(ui, "Eigene Stile", None, |ui| {
            if own.is_empty() {
                ui.label(RichText::new("Noch keine Stile gespeichert.").color(Color32::from_gray(150)));
            } else if grid {
                take(self.style_tiles(ui, &own));
                ui.label(RichText::new("Rechtsklick auf einen Stil: löschen").small().color(crate::theme::TEXT_FAINT));
            } else {
                take(style_rows(ui, &own));
            }
        });
        if let (Some((style, look)), Some(current)) = (hovered, current)
            && apply.is_none()
        {
            self.preview_style(crate::style_grid::applied(&style, look, &current));
        }
        let mut save = None;
        section(ui, "Neuer Stil", None, |ui| {
            let Some(cur) = &self.current else {
                crate::widgets::hint(ui, "Ein Bild öffnen, um seine Einstellungen als Stil zu speichern.");
                return;
            };
            ui.add(egui::TextEdit::singleline(&mut self.library_ui.new_style).hint_text("Name des Stils"));
            let name = self.library_ui.new_style.trim().to_string();
            // Until chosen by hand: the edited tools of the image.
            let auto: std::collections::BTreeSet<_> = sk_core::tools::edited(&cur.adjustments).into_iter().collect();
            let mut chosen = self.library_ui.new_style_tools.clone().unwrap_or(auto);
            let list: Vec<sk_core::tools::Tool> = chosen.iter().copied().collect();
            egui::CollapsingHeader::new(format!("Enthält: {}", crate::clipboard::tool_list(&list)))
                .id_salt("new_style_tools")
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("Nur diese Werkzeuge werden gesetzt, alle anderen bleiben, wie sie sind; so lassen sich Stile übereinander anwenden.")
                            .small()
                            .color(crate::theme::TEXT_FAINT),
                    );
                    let before = chosen.clone();
                    crate::clipboard::tool_checkboxes(ui, &cur.adjustments, &mut chosen);
                    if chosen != before {
                        self.library_ui.new_style_tools = Some(chosen.clone());
                    }
                });
            if ui.add_enabled(!name.is_empty() && !list.is_empty(), egui::Button::new("Aktuelle Anpassungen speichern")).clicked() {
                save = Some(Style { name, adjustments: cur.adjustments.clone(), tools: Some(list) });
            }
        });
        if let Some(style) = save {
            match styles::save_in(&styles::styles_dir(), &style) {
                Ok(()) => self.status = format!("Stil „{}“ gespeichert", style.name),
                Err(e) => self.status = format!("Fehler: {e:#}"),
            }
            self.library_ui.new_style.clear();
            self.library_ui.new_style_tools = None;
            self.reload_styles();
        }
        if let Some((style, look)) = apply {
            let paths = self.selected_paths();
            self.set_adjustments_on(&paths, |a| if look { styles::apply_look(&style, a) } else { styles::apply(&style, a) });
            self.status = format!("Stil „{}“ auf {} angewendet", style.name, crate::widgets::image_count(paths.len()));
        }
        if let Some(name) = delete {
            self.request_deletion(crate::deletion::Deletion::Style(name));
        }
    }
}

/// Styles as rows with an "Anwenden" button; returns the clicked one, the hovered one and one to
/// delete, like `App::style_tiles`.
fn style_rows(ui: &mut Ui, list: &[(Style, bool, String)]) -> crate::style_grid::Picks {
    let (mut clicked, mut hovered, mut delete) = (None, None, None);
    for (style, look, help) in list {
        let row = ui.horizontal(|ui| {
            if ui.button("Anwenden").on_hover_text("Auf Auswahl anwenden").clicked() {
                clicked = Some((style.clone(), *look));
            }
            ui.label(&style.name).on_hover_text(help.as_str());
            if !look {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::small_button(ui, "\u{E4A6}", "Stil löschen …").clicked() {
                        delete = Some(style.name.clone());
                    }
                });
            }
        });
        if ui.rect_contains_pointer(row.response.rect) {
            hovered = Some((style.clone(), *look));
        }
    }
    (clicked, hovered, delete)
}

pub fn sort_records(records: &mut [ImageRecord], key: SortKey, desc: bool) {
    records.sort_by(|a, b| {
        let ord = match key {
            SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortKey::CaptureDate => a.capture_date.cmp(&b.capture_date).then_with(|| a.name.cmp(&b.name)),
            SortKey::Rating => a.rating.cmp(&b.rating).then_with(|| a.name.cmp(&b.name)),
        };
        if desc { ord.reverse() } else { ord }
    });
}
