//! Library: catalog scopes (folders, albums, smart albums), filtering, sorting,
//! metadata edits (rating, label, keywords) and styles.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sk_core::catalog::{self, Filter, ImageRecord, Scope, SortKey};
use sk_core::params::Adjustments;
use sk_core::sidecar::{self, ColorLabel, Sidecar};
use sk_core::styles::{self, Style};
use egui::{Rect, RichText, Ui};

use crate::app::App;
use crate::undo::Meta;
use crate::widgets::{self, section};
use crate::workers::IndexJob;

/// Catalog-derived lists shown in the library tab.
#[derive(Default)]
pub struct CatalogLists {
    pub albums: Vec<catalog::Album>,
    pub folders: Vec<(PathBuf, usize)>,
    pub keywords: Vec<(String, usize)>,
    pub total: usize,
    /// Images of the last import.
    pub last_import: usize,
    /// Session favorites (folders).
    pub favorites: Vec<PathBuf>,
    /// People recognized by face (named, or seen in several images).
    pub persons: Vec<catalog::Person>,
}

/// (catalog folders, folders shown) and the tree built from them.
pub type FolderTreeCache = ((Vec<(PathBuf, usize)>, Vec<PathBuf>), Vec<FolderNode>);

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
    /// Strg+F: the search field of the bar takes the keyboard.
    pub focus_search: bool,
    /// Folders open in the folder tree, and the folder they were last opened for.
    pub expanded: std::collections::HashSet<PathBuf>,
    pub expanded_for: PathBuf,
    /// The folder tree and what it was built from (built again when that changes).
    pub tree: Option<FolderTreeCache>,
    /// The subfolders read from disk for the tree below the folder shown, per folder (read
    /// when a folder becomes visible in it; forgotten when another folder is opened).
    pub listed: std::collections::HashMap<PathBuf, Vec<PathBuf>>,
    /// The size of the file shown in the Metadaten tab (read once per file).
    pub file_size: Option<(PathBuf, Option<u64>)>,
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
    // Session folders are shown by their label ("Aufnahmen" rather than "Capture").
    if let (Scope::Folder { path, .. }, Some(s)) = (scope, session)
        && let Some(f) = s.folder_of(path).filter(|f| s.folder(*f) == *path)
    {
        return f.label().into();
    }
    match scope {
        Scope::All => "Alle Bilder".into(),
        Scope::LastImport => "Letzter Import".into(),
        Scope::Folder { path, .. } => {
            path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().to_string())
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
            last_import: c.last_import_count().unwrap_or(0),
            favorites: c.favorites().unwrap_or_default(),
            persons: c.persons(2).unwrap_or_default(),
        };
    }

    /// Switches the browser to a folder (like a session folder).
    pub fn open_folder(&mut self, folder: PathBuf) {
        let folder = sk_core::paths::canonical(&folder);
        if !folder.is_dir() {
            self.report_error(format!("Ordner nicht lesbar: {}", folder.display()));
            return;
        }
        self.folder_input = folder.display().to_string();
        self.subdirs = subdirs_of(&folder, self.session.as_ref());
        self.library_ui.listed.clear();
        self.library_ui.listed.insert(folder.clone(), self.subdirs.clone());
        self.folder = folder.clone();
        if self.session.is_none() {
            self.catalog_folder = folder.clone();
        }
        self.save_settings();
        self.open_scope(Scope::Folder { path: folder.clone(), recursive: self.include_subfolders });
        // A session's own folders are its database: indexed silently, so that dates, cameras
        // and search work. The catalog only holds what was imported.
        let belongs = self.session.as_ref().is_some_and(|s| folder.starts_with(&s.root));
        let paths = if belongs { catalog::scan_folder(&folder, false) } else { Vec::new() };
        if !paths.is_empty() {
            self.workers.index(IndexJob { catalog: self.catalog_path(), paths, import: false, copy_to: None });
        }
    }

    /// Shows `scope`. A folder is read in the background ("Lade Ordner"); until it arrives,
    /// the browser keeps showing what it showed.
    pub fn open_scope(&mut self, scope: Scope) {
        self.commit_and_save();
        self.scope = scope;
        self.scope_run += 1;
        match self.scope_on_disk() {
            Some((path, recursive)) => {
                self.folder_loading = true;
                self.workers.load_folder(self.scope_run, path, recursive);
                // Albums and favorites at once (another session's, say), not with the folder.
                self.refresh_catalog_lists();
            }
            None => {
                self.folder_loading = false;
                self.reload_entries();
                self.select_in_scope();
            }
        }
    }

    /// The folder of `open_scope` was read (`run` tells for which scope).
    pub(crate) fn folder_loaded(&mut self, run: u64, files: Vec<(PathBuf, Sidecar)>) {
        if run != self.scope_run {
            return;
        }
        self.folder_loading = false;
        let records = self.with_catalog_details(files);
        self.set_entries(records);
        self.select_in_scope();
    }

    /// Keeps the open image if the scope shows it, else opens the one selected last time
    /// (after starting) or the first.
    fn select_in_scope(&mut self) {
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
    /// into its Capture folder, unless they already are inside the session; the catalog does
    /// what its import dialog was last set to.
    #[cfg(test)]
    pub fn import_folder(&mut self, folder: &Path) {
        let copy_to = self.import_copy_to(folder);
        self.import_files(catalog::scan_folder(folder, true), copy_to);
    }

    /// Imports `paths`, copying them first with `copy_to` (see `import.rs`).
    pub fn import_files(&mut self, paths: Vec<PathBuf>, copy_to: Option<crate::import::CopyTo>) {
        if paths.is_empty() {
            self.status = "Keine RAW-Dateien gefunden".into();
            return;
        }
        self.status = match &copy_to {
            Some(_) => format!("Kopiere {} …", crate::widgets::image_count(paths.len())),
            None => format!("Importiere {} …", crate::widgets::image_count(paths.len())),
        };
        self.workers.index(IndexJob { catalog: self.catalog_path(), paths, import: true, copy_to });
    }

    /// Re-reads the entries of the current scope, keeping selection by path.
    pub fn reload_entries(&mut self) {
        let records = match self.scope_on_disk() {
            Some((path, recursive)) => self.with_catalog_details(sidecar::load_all(&catalog::scan_folder(&path, recursive))),
            None => self
                .catalog
                .as_ref()
                .and_then(|c| c.query(&self.scope, &Filter::default(), SortKey::Name, false).ok())
                .unwrap_or_default(),
        };
        self.set_entries(records);
    }

    /// The folder and subfolder switch if the scope is read from the file system rather than
    /// the catalog: a folder always, as the file system is authoritative (a variant or a renamed
    /// file is there before the catalog knows it). With its subfolders a large tree takes a
    /// while; `open_scope` reads it in the background.
    fn scope_on_disk(&self) -> Option<(PathBuf, bool)> {
        match &self.scope {
            Scope::Folder { path, recursive } => Some((path.clone(), *recursive)),
            _ => None,
        }
    }

    /// The file system is authoritative; the catalog adds metadata where indexed.
    fn with_catalog_details(&self, files: Vec<(PathBuf, Sidecar)>) -> Vec<ImageRecord> {
        let indexed: HashMap<PathBuf, ImageRecord> = self
            .catalog
            .as_ref()
            .and_then(|c| c.query(&self.scope, &Filter::default(), SortKey::Name, false).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|r| (r.path.clone(), r))
            .collect();
        files.into_iter().map(|(p, sc)| indexed.get(&p).cloned().unwrap_or_else(|| record_from_sidecar(p, sc))).collect()
    }

    fn set_entries(&mut self, mut records: Vec<ImageRecord>) {
        sort_records(&mut records, self.sort, self.sort_desc);
        self.entries = records;
        self.forget_unlisted_thumbs();
        let listed: std::collections::HashSet<&PathBuf> = self.entries.iter().map(|e| &e.path).collect();
        self.selection.retain(|p| listed.contains(p));
        self.refilter_with_content();
        self.refresh_catalog_lists();
        self.request_proxies();
    }

    /// Adds new files of the folder shown (from their sidecars) without reading the whole
    /// folder again; the catalog's details follow when they are indexed.
    pub(crate) fn add_entries(&mut self, paths: &[PathBuf]) {
        let listed: std::collections::HashSet<&PathBuf> = self.entries.iter().map(|e| &e.path).collect();
        let new: Vec<PathBuf> = paths.iter().filter(|p| !listed.contains(p)).cloned().collect();
        self.entries.extend(sidecar::load_all(&new).into_iter().map(|(p, sc)| record_from_sidecar(p, sc)));
        sort_records(&mut self.entries, self.sort, self.sort_desc);
        self.refilter_with_content();
        self.request_proxies();
    }

    pub fn resort(&mut self) {
        sort_records(&mut self.entries, self.sort, self.sort_desc);
        self.refilter();
    }

    pub fn refilter(&mut self) {
        let mut matching: Vec<usize> = self.entries.iter().enumerate().filter(|(_, r)| self.filter.matches(r)).map(|(i, _)| i).collect();
        if let Some(ranked) = self.content_order(&matching) {
            matching = ranked;
        }
        self.visible = self.arrange_stacks(matching);
    }

    /// [`Self::refilter`] after the entries changed: a content search also needs the new ones
    /// analysed.
    fn refilter_with_content(&mut self) {
        if self.content.is_active() { self.update_content_search() } else { self.refilter() }
    }

    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.entries.iter().position(|e| e.path == path)
    }

    /// Applies a metadata change to the given images (sidecar + catalog + entries).
    /// Changes rating, label or keywords of `paths` (one undo step).
    pub fn update_meta(&mut self, paths: &[PathBuf], f: impl Fn(&mut Sidecar) + Sync) {
        let changed = self.update_meta_unrecorded(paths, f).into_iter().filter(|(_, before, after)| before != after).map(|(p, before, _)| (p, before)).collect();
        self.record_step(crate::undo::Step::Meta(changed));
    }

    /// [`Self::update_meta`] without an undo step (for undo itself). Returns rating, label and
    /// keywords before and after for each image that could be read.
    pub(crate) fn update_meta_unrecorded(&mut self, paths: &[PathBuf], f: impl Fn(&mut Sidecar) + Sync) -> Vec<(PathBuf, Meta, Meta)> {
        use rayon::prelude::*;
        let current = self.current_path();
        // The other images in parallel: each sidecar write waits for the disk.
        let others: Vec<(&PathBuf, Option<&Sidecar>)> =
            paths.iter().filter(|p| Some(*p) != current.as_ref()).map(|p| (p, self.unsaved.get(p))).collect();
        let written: Vec<_> = others
            .into_par_iter()
            .map(|(path, unsaved)| {
                let mut sc = crate::app::with_unsaved(unsaved, sidecar::try_load(path)?);
                let before = Meta::of(&sc);
                f(&mut sc);
                let saved = sidecar::save(path, &sc);
                Ok::<_, anyhow::Error>((path, before, sc, saved))
            })
            .collect();
        let mut written = written.into_iter();
        let mut out = Vec::new();
        // One transaction for the whole batch instead of one commit per statement.
        let batch = self.catalog.as_ref().is_some_and(|c| c.begin_batch().is_ok());
        let wanted: std::collections::HashSet<&PathBuf> = paths.iter().collect();
        let index: HashMap<PathBuf, usize> =
            self.entries.iter().enumerate().filter(|(_, e)| wanted.contains(&e.path)).map(|(i, e)| (e.path.clone(), i)).collect();
        for path in paths {
            let (before, sc) = if Some(path) == current.as_ref() {
                let cur = self.current.as_mut().expect("current");
                let mut sc = Sidecar {
                    rating: cur.rating,
                    color_label: cur.color_label,
                    keywords: cur.keywords.clone(),
                    ..Default::default()
                };
                let before = Meta::of(&sc);
                f(&mut sc);
                cur.rating = sc.rating;
                cur.color_label = sc.color_label;
                cur.keywords = sc.keywords.clone();
                self.sidecar_dirty = true;
                (before, sc)
            } else {
                match written.next().expect("one result per other path") {
                    Ok((_, before, sc, saved)) => {
                        self.saved_sidecar(path, &sc, saved);
                        (before, sc)
                    }
                    Err(e) => {
                        self.report_error(format!("Nicht gespeichert: {e:#}"));
                        continue;
                    }
                }
            };
            if let Some(&i) = index.get(path) {
                let e = &mut self.entries[i];
                e.rating = sc.rating;
                e.color_label = sc.color_label;
                e.keywords = sc.keywords.clone();
            }
            out.push((path.clone(), before, Meta::of(&sc)));
        }
        if batch && let Some(c) = &self.catalog {
            let _ = c.end_batch();
        }
        self.commit_and_save();
        self.refilter();
        self.refresh_catalog_lists();
        out
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

    /// `selected_paths().len()` without collecting them (for labels drawn every frame).
    pub fn selected_count(&self) -> usize {
        let n = self.visible.iter().filter(|&&i| self.selection.contains(&self.entries[i].path)).count();
        if n == 0 && self.current.is_some() { 1 } else { n }
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
    /// Changes the adjustments of `paths`, the open image's included, as one undo step (also
    /// for the images that are not open: Ctrl+Z takes the whole change back).
    pub fn set_adjustments_on(&mut self, paths: &[PathBuf], f: impl Fn(&Adjustments) -> Adjustments + Sync) {
        use rayon::prelude::*;
        let current = self.current_path();
        // The images that are not open in parallel (each sidecar write waits for the disk),
        // the bookkeeping afterwards in one catalog transaction.
        let others: Vec<(&PathBuf, Option<&Sidecar>)> =
            paths.iter().filter(|p| Some(*p) != current.as_ref()).map(|p| (p, self.unsaved.get(p))).collect();
        let written: Vec<_> = others
            .into_par_iter()
            .map(|(path, unsaved)| {
                if !still_there(path) {
                    return None;
                }
                let mut sc = match sidecar::try_load(path) {
                    Ok(sc) => crate::app::with_unsaved(unsaved, sc),
                    Err(e) => return Some(Err(e)),
                };
                let new = f(&sc.adjustments);
                let prev = std::mem::replace(&mut sc.adjustments, new);
                let saved = sidecar::save(path, &sc);
                Some(Ok((prev, sc, saved)))
            })
            .collect();
        let mut before = Vec::new();
        // The open image first, outside the batch (saving it has transactions of its own).
        if let Some(path) = current.as_ref().filter(|c| paths.contains(c)) {
            self.commit_and_save();
            if let Some(old) = self.current.as_ref().map(|c| c.committed.clone())
                && let Some(prev) = self.write_adjustments(path, f(&old))
            {
                before.push((path.clone(), prev));
            }
        }
        let batch = self.catalog.as_ref().is_some_and(|c| c.begin_batch().is_ok());
        for (path, result) in paths.iter().filter(|p| Some(*p) != current.as_ref()).zip(written) {
            match result {
                Some(Ok((prev, sc, saved))) => {
                    self.saved_sidecar(path, &sc, saved);
                    self.forget_rendered(path);
                    before.push((path.clone(), prev));
                }
                Some(Err(e)) => self.report_error(format!("Nicht gespeichert: {e:#}")),
                None => {}
            }
        }
        if batch && let Some(c) = &self.catalog {
            let _ = c.end_batch();
        }
        self.record_step(crate::undo::Step::Adjustments(before));
    }

    /// Sets the adjustments of one image (open or not) and returns the ones before, or None when
    /// they couldn't be written. No undo step of its own (see `set_adjustments_on`, `undo`).
    pub(crate) fn write_adjustments(&mut self, path: &PathBuf, adj: Adjustments) -> Option<Adjustments> {
        if self.current_path().as_ref() == Some(path) {
            self.commit_and_save();
            let cur = self.current.as_mut()?;
            let prev = std::mem::replace(&mut cur.committed, adj.clone());
            cur.adjustments = adj;
            self.sidecar_dirty = true;
            self.commit_and_save();
            self.request_render();
            return Some(prev);
        }
        if !still_there(path) {
            return None;
        }
        let mut sc = match self.sidecar_for_update(path) {
            Ok(sc) => sc,
            Err(e) => {
                self.report_error(format!("Nicht gespeichert: {e:#}"));
                return None;
            }
        };
        let prev = std::mem::replace(&mut sc.adjustments, adj);
        self.save_sidecar(path, &sc);
        self.forget_rendered(path);
        Some(prev)
    }

    /// Drops what was shown of an image that is not open, after its settings changed.
    fn forget_rendered(&mut self, path: &PathBuf) {
        self.thumb_requested.remove(path);
        self.compare_textures.remove(path);
        self.compare_requested.remove(path);
        self.focus_maps.remove(path);
        self.compare_details.remove(path);
        self.forget_prepared(path);
    }

    pub fn reload_styles(&mut self) {
        self.styles = styles::list_in(&styles::styles_dir());
    }

    // ---------------------------------------------------------------- UI

    pub fn library_tab(&mut self, ui: &mut Ui) {
        use crate::theme::icon;
        let mut open: Option<Scope> = None;
        let mut open_folder: Option<PathBuf> = None;
        let mut do_cleanup = false;

        self.session_sections(ui);

        // Catalog.
        let title = if self.session.is_some() { "Session-Katalog" } else { "Katalog" };
        widgets::nav_header(ui, "Katalog", title, |ui| {
            let resp = widgets::nav_action(ui, icon::DOTS_THREE, "Weitere Aktionen");
            widgets::menu_popup(&resp).show(|ui| {
                if ui.button(format!("{}  Bereinigen", icon::BROOM)).on_hover_text("Fehlende Dateien aus dem Katalog entfernen").clicked() {
                    do_cleanup = true;
                    ui.close();
                }
            });
        });
        if widgets::nav_row(ui, icon::STACK_SIMPLE, "Alle Bilder", Some(self.lists.total), self.scope == Scope::All, 0.0, false).clicked() {
            open = Some(Scope::All);
        }
        if widgets::nav_row(ui, icon::CLOCK_COUNTER_CLOCKWISE, "Letzter Import", Some(self.lists.last_import), self.scope == Scope::LastImport, 0.0, false).clicked() {
            open = Some(Scope::LastImport);
        }

        // Folders as a tree: those of the catalog, and the one shown with its subfolders.
        let mut browse = false;
        widgets::nav_header(ui, "Ordner", "Ordner", |ui| {
            let resp = widgets::nav_action(ui, icon::PLUS, &crate::commands::hint("Ordner öffnen", "open-folder"));
            widgets::menu_popup(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                ui.set_width(250.0);
                ui.label(RichText::new("Ordner öffnen").small().color(crate::theme::TEXT_DIM));
                let resp = ui.add(egui::TextEdit::singleline(&mut self.folder_input).hint_text("Pfad, Enter öffnet").desired_width(f32::INFINITY));
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    open_folder = Some(self.folder_input.clone().into());
                    ui.close();
                }
                if ui.add_sized([ui.available_width(), 26.0], egui::Button::new(format!("{}  Durchsuchen …", icon::FOLDER_OPEN))).clicked() {
                    browse = true;
                    ui.close();
                }
            });
        });
        // In a session its own folders (Aufnahmen …) are above already.
        let standard: Vec<PathBuf> = self.session.as_ref().map(|s| sk_core::session::SessionFolder::ALL.iter().map(|f| s.folder(*f)).collect()).unwrap_or_default();
        let own = |p: &Path| standard.iter().any(|d| p.starts_with(d));
        let folders: Vec<(PathBuf, usize)> = self.lists.folders.iter().filter(|(p, _)| !own(p)).cloned().collect();
        let mut extra = vec![self.folder.clone()];
        // Below the folder shown, from disk: the subfolders of each folder visible in the tree,
        // so that those with subfolders of their own can be opened.
        let mut visible = vec![(self.folder.clone(), true)];
        while let Some((dir, open)) = visible.pop() {
            let session = self.session.as_ref();
            let kids = self.library_ui.listed.entry(dir).or_insert_with_key(|d| subdirs_of(d, session));
            extra.extend(kids.iter().cloned());
            if open {
                visible.extend(kids.iter().map(|k| (k.clone(), self.library_ui.expanded.contains(k))));
            }
        }
        extra.retain(|p| !own(p) && self.session.as_ref().is_none_or(|s| *p != s.root));
        let key = (self.lists.folders.clone(), extra.clone());
        if self.library_ui.tree.as_ref().is_none_or(|(k, _)| *k != key) {
            self.library_ui.tree = Some((key, folder_tree(&folders, &extra)));
        }
        let tree = std::mem::take(&mut self.library_ui.tree.as_mut().unwrap().1);
        if self.library_ui.expanded_for != self.folder {
            // The way to the folder shown is open.
            self.library_ui.expanded_for = self.folder.clone();
            self.library_ui.expanded.extend(self.folder.ancestors().map(Path::to_path_buf));
        }
        let shown = match &self.scope {
            Scope::Folder { path, .. } => Some(path.clone()),
            _ => None,
        };
        let mut toggle: Option<PathBuf> = None;
        for node in &tree {
            folder_rows(ui, node, 0, &self.library_ui.expanded, shown.as_deref(), &mut open_folder, &mut toggle);
        }
        if let Some((_, t)) = &mut self.library_ui.tree {
            *t = tree;
        }
        let tree_empty = self.library_ui.tree.as_ref().is_none_or(|(_, t)| t.is_empty());
        if tree_empty && self.session.is_some() {
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                widgets::hint(ui, "Weitere Ordner: +");
            });
        }
        if let Some(t) = toggle
            && !self.library_ui.expanded.remove(&t)
        {
            self.library_ui.expanded.insert(t);
        }

        // Albums.
        enum AlbumAction {
            Create(String),
            Rename(i64, String),
            Delete(i64, String),
            AddSelection(i64),
            RemoveSelection(i64),
        }
        let mut action: Option<AlbumAction> = None;
        let selected_n = self.selected_count();
        widgets::nav_header(ui, "Alben", "Alben", |ui| {
            let resp = widgets::nav_action(ui, icon::PLUS, "Neues Album mit der Auswahl");
            widgets::menu_popup(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                ui.set_width(250.0);
                ui.label(RichText::new(format!("Neues Album mit der Auswahl ({})", widgets::image_count(selected_n))).small().color(crate::theme::TEXT_DIM));
                let resp = ui.add(egui::TextEdit::singleline(&mut self.library_ui.new_album).hint_text("Name").desired_width(f32::INFINITY));
                let name = self.library_ui.new_album.trim().to_string();
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.add_enabled(!name.is_empty(), egui::Button::new("Album anlegen")).clicked() || enter) && !name.is_empty() {
                    action = Some(AlbumAction::Create(name));
                    ui.close();
                }
            });
        });
        for album in &self.lists.albums {
            let scope = Scope::Album(album.id);
            if let Some((id, name)) = &mut self.library_ui.renaming
                && *id == album.id
            {
                let resp = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
                if resp.lost_focus() {
                    // Esc keeps the old name (an empty one is ignored), as for layers.
                    let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                    action = Some(AlbumAction::Rename(*id, if esc { String::new() } else { name.trim().to_string() }));
                } else {
                    resp.request_focus();
                }
                continue;
            }
            let ic = if album.smart.is_some() { icon::SLIDERS_HORIZONTAL } else { icon::IMAGES_SQUARE };
            let hover = if album.smart.is_some() { "Filteralbum: zeigt immer die Bilder seines Filters\nDoppelklick: umbenennen" } else { "Doppelklick: umbenennen" };
            let resp = widgets::nav_row(ui, ic, &album.name, Some(album.count), self.scope == scope, 0.0, false).on_hover_text(hover);
            if resp.double_clicked() {
                self.library_ui.renaming = Some((album.id, album.name.clone()));
            } else if resp.clicked() {
                open = Some(scope);
            }
            widgets::context_menu(&resp, |ui| {
                if album.smart.is_none() {
                    if ui.button("Auswahl hinzufügen").clicked() {
                        action = Some(AlbumAction::AddSelection(album.id));
                        ui.close();
                    }
                    if ui.button("Auswahl aus Album entfernen").clicked() {
                        action = Some(AlbumAction::RemoveSelection(album.id));
                        ui.close();
                    }
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
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                widgets::hint(ui, "Noch keine Alben. + legt eines mit der Auswahl an.");
            });
        }
        if let Some(action) = action {
            let selected = self.selected_paths();
            let mut created = None;
            if let Some(c) = &self.catalog {
                match action {
                    AlbumAction::Create(name) => {
                        if let Ok(id) = c.create_album(&name, None) {
                            index_missing(c, &selected);
                            let _ = c.add_to_album(id, &selected);
                            created = Some(id);
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
                        index_missing(c, &selected);
                        let n = c.add_to_album(id, &selected).unwrap_or(0);
                        self.status = format!("{} zum Album hinzugefügt", widgets::image_count(n));
                    }
                    AlbumAction::RemoveSelection(id) => {
                        let _ = c.remove_from_album(id, &selected);
                    }
                }
            }
            if let Some(id) = created {
                self.refresh_catalog_lists();
                open = open.or(Some(Scope::Album(id)));
            } else if open.is_none() && matches!(self.scope, Scope::Album(_)) {
                self.reload_entries();
            } else {
                self.refresh_catalog_lists();
            }
        }

        self.persons_section(ui);

        if do_cleanup && let Some(c) = &self.catalog {
            let n = c.remove_missing().unwrap_or(0);
            self.status = format!("{n} fehlende Einträge entfernt");
            self.reload_entries();
        }
        if browse {
            self.open_folder_dialog();
        } else if let Some(f) = open_folder {
            self.open_folder(f);
        } else if let Some(s) = open {
            self.open_scope(s);
        }
    }

    /// Saves the current filter as a filter album named `name` and shows it.
    pub(crate) fn create_filter_album(&mut self, name: &str) {
        let Some(c) = &self.catalog else { return };
        match c.create_album(name, Some(&self.filter)) {
            Ok(id) => {
                self.refresh_catalog_lists();
                self.status = format!("Filteralbum „{name}“ angelegt");
                self.open_scope(Scope::Album(id));
            }
            Err(e) => self.report_error(format!("Filteralbum nicht angelegt: {e}")),
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
                crate::widgets::hint(ui, "Noch keine Stile gespeichert.");
            } else if grid {
                take(self.style_tiles(ui, &own));
                crate::widgets::hint(ui, "Rechtsklick auf einen Stil: löschen");
            } else {
                take(style_rows(ui, &own));
            }
        });
        if let (Some((style, look)), Some(current)) = (hovered, current)
            && apply.is_none()
        {
            self.preview_style(crate::style_grid::applied_to(&style, look, &current, self.active_layer));
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
                Err(e) => self.report_error(format!("Fehler: {e:#}")),
            }
            self.library_ui.new_style.clear();
            self.library_ui.new_style_tools = None;
            self.reload_styles();
        }
        // A layer chosen: onto it, on the current image (layers belong to one image).
        if let Some((style, look)) = &apply
            && let Some(li) = self.active_layer.filter(|i| self.current.as_ref().is_some_and(|c| *i < c.adjustments.layers.len()))
        {
            self.apply_style_to_layer(style, *look, li);
        } else if let Some((style, look)) = apply {
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
                    if widgets::small_button(ui, crate::theme::icon::TRASH, "Stil löschen …").clicked() {
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

/// Takes images of a folder that was only browsed into the catalog, where they are: an album
/// holds catalog images.
fn index_missing(c: &sk_core::catalog::Catalog, paths: &[PathBuf]) {
    // A variant comes in with its file (it has no file of its own to read).
    for file in sk_core::variant::files_of(paths) {
        for p in sk_core::variant::of_file(&file) {
            if c.needs_index(&p).unwrap_or(true) {
                let _ = crate::guard::catch(|| c.index_file(&p, 0));
            }
        }
    }
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

/// Results arriving late (matching to a reference) must not bring back a deleted variant or
/// write a sidecar next to a file that was moved away.
fn still_there(path: &Path) -> bool {
    let (file, n) = sk_core::variant::split(path);
    file.exists() && (n == 1 || sk_core::variant::of_file(&file).iter().any(|p| p == path))
}

/// "Ordner überwachen" and its option, as items of a menu.
pub(crate) fn hot_folder_items(ui: &mut Ui, (on, open_new): &mut (bool, bool)) {
    widgets::checkbox(ui, on, "Ordner überwachen").on_hover_text("Neue Bilder, etwa aus der Kamerasoftware, erscheinen von selbst in der Übersicht");
    ui.add_enabled_ui(*on, |ui| {
        ui.indent("hot_folder", |ui| widgets::checkbox(ui, open_new, "Neue Aufnahme gleich öffnen"));
    });
}

/// A folder of the folder tree: its images (with those of its subfolders), `None` for a folder
/// not in the catalog.
#[derive(Debug, PartialEq)]
pub struct FolderNode {
    pub path: PathBuf,
    pub count: Option<usize>,
    pub children: Vec<FolderNode>,
}

/// The folder tree of the catalog's `folders` and the `extra` ones (the folder shown and its
/// subfolders): the way from the drive to the first folder that holds images or branches is
/// one root, below it every level.
/// The subfolders of `folder` (not hidden ones, nor a session's own data folder), sorted.
fn subdirs_of(folder: &Path, session: Option<&sk_core::session::Session>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        // The type from the listing (no further look at each file, slow on network drives);
        // links are followed.
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir() || (t.is_symlink() && e.path().is_dir())))
        .map(|e| e.path())
        .filter(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
        // The session's own data folder (cache) is not for browsing.
        .filter(|p| !session.is_some_and(|s| *p == s.root.join(sk_core::session::APP_FOLDER)))
        .collect();
    dirs.sort();
    dirs
}

pub fn folder_tree(folders: &[(PathBuf, usize)], extra: &[PathBuf]) -> Vec<FolderNode> {
    #[derive(Default)]
    struct Trie {
        own: Option<usize>,
        known: bool,
        children: std::collections::BTreeMap<std::ffi::OsString, Trie>,
    }
    let mut root = Trie::default();
    let mut add = |path: &Path, n: Option<usize>| {
        let mut t = &mut root;
        for c in path.components() {
            t = t.children.entry(c.as_os_str().to_os_string()).or_default();
        }
        t.known = true;
        if let Some(n) = n {
            *t.own.get_or_insert(0) += n;
        }
    };
    for (p, n) in folders {
        add(p, Some(*n));
    }
    for p in extra {
        add(p, None);
    }
    fn build(t: &Trie, path: PathBuf) -> FolderNode {
        let children: Vec<FolderNode> = t.children.iter().map(|(name, c)| build(c, path.join(name))).collect();
        let below: usize = children.iter().filter_map(|c| c.count).sum();
        let count = (t.own.is_some() || children.iter().any(|c| c.count.is_some())).then(|| t.own.unwrap_or(0) + below);
        FolderNode { path, count, children }
    }
    // Down from the drive while a folder only leads on to one other; where it branches (not at
    // the drive itself) is a root.
    fn roots(mut t: &Trie, mut path: PathBuf) -> Vec<FolderNode> {
        while !t.known && t.children.len() == 1 {
            let (name, c) = t.children.iter().next().unwrap();
            path.push(name);
            t = c;
        }
        if !t.known && (path.as_os_str().is_empty() || path.parent().is_none()) {
            return t.children.iter().flat_map(|(name, c)| roots(c, path.join(name))).collect();
        }
        vec![build(t, path)]
    }
    roots(&root, PathBuf::new())
}

/// The rows of `node` and, if it is open, of its subfolders.
fn folder_rows(ui: &mut Ui, node: &FolderNode, depth: usize, expanded: &std::collections::HashSet<PathBuf>, shown: Option<&Path>, open: &mut Option<PathBuf>, toggle: &mut Option<PathBuf>) {
    use crate::theme::icon;
    let indent = depth as f32 * 14.0 + 12.0;
    let is_open = expanded.contains(&node.path);
    let selected = shown == Some(node.path.as_path());
    let name = node.path.file_name().map_or_else(|| node.path.display().to_string(), |n| n.to_string_lossy().to_string());
    let ic = if is_open && !node.children.is_empty() { icon::FOLDER_OPEN } else { icon::FOLDER };
    let resp = widgets::nav_row(ui, ic, &name, node.count, selected, indent, false).on_hover_text(node.path.display().to_string());
    if !node.children.is_empty() {
        let r = Rect::from_center_size(egui::pos2(resp.rect.left() + indent - 2.0, resp.rect.center().y), egui::vec2(16.0, 22.0));
        let caret = ui.interact(r, resp.id.with("caret"), egui::Sense::click());
        let c = if caret.hovered() { crate::theme::TEXT } else { crate::theme::TEXT_FAINT };
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, if is_open { icon::CARET_DOWN } else { icon::CARET_RIGHT }, egui::FontId::proportional(10.0), c);
        if caret.clicked() {
            *toggle = Some(node.path.clone());
        } else if resp.clicked() {
            *open = Some(node.path.clone());
        }
    } else if resp.clicked() {
        *open = Some(node.path.clone());
    }
    if is_open {
        for c in &node.children {
            folder_rows(ui, c, depth + 1, expanded, shown, open, toggle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_tree_starts_where_folders_branch() {
        let p = |s: &str| PathBuf::from(s);
        let folders = vec![(p("/home/m/Bilder/2026/Italien"), 5), (p("/home/m/Bilder/2026/Nürnberg"), 3), (p("/home/m/Bilder/Archiv"), 2)];
        let tree = folder_tree(&folders, &[]);
        // The way down to "Bilder" is one root; "2026" has no images of its own but counts its subfolders.
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].path, p("/home/m/Bilder"));
        assert_eq!(tree[0].count, Some(10));
        let y2026 = &tree[0].children[0];
        assert_eq!((y2026.path.clone(), y2026.count), (p("/home/m/Bilder/2026"), Some(8)));
        assert_eq!(y2026.children.len(), 2);
        // A folder only browsed comes in without a count, beside the catalog's.
        let tree = folder_tree(&folders, &[p("/mnt/karte/DCIM")]);
        assert_eq!(tree.len(), 2, "{tree:?}");
        let card = tree.iter().find(|n| n.path.starts_with("/mnt")).unwrap();
        assert_eq!(card.count, None);
        // Nothing at all: no tree.
        assert!(folder_tree(&[], &[]).is_empty());
    }
}
