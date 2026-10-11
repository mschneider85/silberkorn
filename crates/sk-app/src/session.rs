//! Sessions: creating, opening and closing them, moving images between the session folders,
//! the trash, session favorites, and the matching menus, dialogs and library sections.

use std::path::{Path, PathBuf};

use egui::{Button, Color32, Modifiers, RichText, Ui, vec2};
use sk_core::cache;
use sk_core::catalog::{self, Catalog, Scope};
use sk_core::session::{self, Session, SessionFolder};
use sk_core::variant;

use crate::app::App;
use crate::theme::{self, icon};
use crate::widgets::section;
use crate::workers::IndexJob;

const MAX_RECENT: usize = 8;

/// State of the session dialogs.
#[derive(Default)]
pub struct SessionUi {
    /// The "new session" dialog is open.
    pub creating: bool,
    pub name: String,
    pub location: PathBuf,
    pub error: Option<String>,
    pub confirm_empty_trash: Option<usize>,
}

impl SessionUi {
    pub fn start_new(&mut self, near: &Path) {
        self.creating = true;
        self.name.clear();
        self.error = None;
        if !self.location.is_dir() {
            self.location = default_location(near);
        }
    }
}

/// The user's pictures folder if there is one, else the parent of the current folder.
fn default_location(near: &Path) -> PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    home.iter()
        .flat_map(|h| [h.join("Pictures"), h.join("Bilder")])
        .find(|p| p.is_dir())
        .or_else(|| near.parent().map(Path::to_path_buf))
        .or(home)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Browser actions chosen in a menu; run after the browser has been drawn, because they change its entries.
#[derive(Clone, Debug, PartialEq)]
pub enum PendingAction {
    MoveTo(PathBuf),
    MoveToDialog,
    NewVariant { clone: bool },
    DeleteVariant,
    Trash,
    Stack(crate::stacks::StackAction),
}

pub fn folder_icon(f: SessionFolder) -> &'static str {
    match f {
        SessionFolder::Capture => icon::CAMERA,
        SessionFolder::Selects => icon::CHECK_CIRCLE,
        SessionFolder::Output => icon::EXPORT,
        SessionFolder::Trash => icon::TRASH,
    }
}

fn dir_name(dir: &Path) -> String {
    dir.file_name().map_or_else(|| dir.display().to_string(), |n| n.to_string_lossy().to_string())
}

/// Shows a folder in the system file manager.
fn reveal(dir: &Path) -> Result<(), String> {
    let cmd = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(cmd).arg(dir).spawn().map(|_| ()).map_err(|e| format!("{cmd}: {e}"))
}

impl App {
    // ---------------------------------------------------------------- open / close

    /// Opens a session (its `.sksession` file or folder) and switches the library to its database.
    pub fn open_session(&mut self, path: &Path) {
        let opened = Session::open(path).and_then(|s| Ok((Catalog::open(&s.file)?, s)));
        let (catalog, s) = match opened {
            Ok(v) => v,
            Err(e) => {
                self.status = format!("Session nicht geöffnet: {e:#}");
                return;
            }
        };
        self.close_current();
        let _ = catalog.remove_missing();
        self.catalog = Some(catalog);
        self.remember_recent(&s.file);
        let (capture, root, name) = (s.folder(SessionFolder::Capture), s.root.clone(), s.name.clone());
        self.session = Some(s);
        self.selection.clear();
        self.open_folder(capture);
        // Index the whole session so that "all images", search and albums cover every folder.
        let paths = catalog::scan_folder(&root, true);
        if !paths.is_empty() {
            self.workers.index(IndexJob { catalog: self.catalog_path(), paths, import: false, copy_to: None });
        }
        self.status = format!("Session „{name}“ geöffnet");
    }

    pub fn create_session(&mut self, parent: &Path, name: &str) -> anyhow::Result<()> {
        let s = Session::create(parent, name)?;
        self.open_session(&s.file);
        Ok(())
    }

    /// Back to the catalog and the folder browsed before the session.
    pub fn close_session(&mut self) {
        if self.session.is_none() {
            return;
        }
        self.close_current();
        self.session = None;
        self.catalog = match Catalog::open(&self.global_catalog) {
            Ok(c) => Some(c),
            Err(e) => {
                self.status = format!("Katalog nicht verfügbar: {e:#}");
                None
            }
        };
        self.selection.clear();
        let folder = self.catalog_folder.clone();
        self.open_folder(folder);
        self.save_settings();
        self.status = "Session geschlossen".into();
    }

    fn remember_recent(&mut self, file: &Path) {
        self.recent_sessions.retain(|p| p != file);
        self.recent_sessions.insert(0, file.to_path_buf());
        self.recent_sessions.truncate(MAX_RECENT);
    }

    pub fn open_session_dialog(&mut self) {
        let start = self.session.as_ref().and_then(|s| s.root.parent().map(Path::to_path_buf)).unwrap_or_else(|| self.folder.clone());
        if let Some(f) = rfd::FileDialog::new()
            .set_title("Session öffnen")
            .add_filter("Silberkorn-Session", &[session::SESSION_EXT])
            .set_directory(start)
            .pick_file()
        {
            self.open_session(&f);
        }
    }

    // ---------------------------------------------------------------- moving images

    /// Moves the selected images (with their sidecars) into `dest`; the database follows them.
    pub fn move_selection_to(&mut self, dest: &Path) {
        // Files move with all their variants.
        let paths: Vec<PathBuf> = variant::files_of(&self.selected_paths()).into_iter().filter(|p| p.parent() != Some(dest)).collect();
        if paths.is_empty() {
            return;
        }
        let moving = |p: &PathBuf| paths.contains(&variant::file_of(p));
        // Afterwards show the next remaining image, like after deleting in a file browser.
        let order: Vec<PathBuf> = self.visible.iter().map(|&i| self.entries[i].path.clone()).collect();
        let next = order.iter().rposition(moving).and_then(|last| {
            order[last + 1..].iter().chain(order[..last].iter().rev()).find(|p| !moving(p)).cloned()
        });
        if self.primary_path().is_some_and(|p| moving(&p)) {
            self.close_current();
        } else {
            self.commit_and_save();
        }
        let (mut moved, mut error) = (0, None);
        let root = self.session.as_ref().map(|s| s.root.clone());
        for p in &paths {
            match session::move_image(p, dest) {
                Ok(new) => {
                    moved += 1;
                    let keep_cache = root.as_ref().is_some_and(|r| new.starts_with(r));
                    self.file_relocated(p, &new, keep_cache);
                }
                Err(e) => error = Some(format!("{e:#}")),
            }
        }
        self.reload_entries();
        if self.primary_path().is_none() {
            let target = next.filter(|n| self.index_of(n).is_some()).or_else(|| self.visible.first().map(|&i| self.entries[i].path.clone()));
            if let Some(t) = target {
                self.select_path(&t, Modifiers::NONE);
            }
        }
        let target = match self.session.as_ref().and_then(|s| s.folder_of(dest).filter(|f| s.folder(*f) == dest)) {
            Some(f) => f.label().to_string(),
            None => dir_name(dest),
        };
        self.status = match error {
            Some(e) => format!("{moved} verschoben, Fehler: {e}"),
            None if moved == 1 => format!("1 Bild nach „{target}“ verschoben"),
            None => format!("{moved} Bilder nach „{target}“ verschoben"),
        };
    }

    /// Catalog, caches and in-memory state follow a file (with its variants) that was moved
    /// or renamed. `keep_cache`: its cached previews move along (same cache).
    pub(crate) fn file_relocated(&mut self, old_file: &Path, new_file: &Path, keep_cache: bool) {
        let root = self.session.as_ref().map(|s| s.root.clone());
        if let Some(c) = &self.catalog {
            let _ = c.rename_path(old_file, new_file);
        }
        for new in variant::of_file(new_file) {
            let old = variant::path_for(old_file, variant::number(&new));
            if keep_cache {
                cache::rename(&old, &new, root.as_deref());
            }
            if let Some(t) = self.thumbs.remove(&old) {
                self.thumbs.insert(new.clone(), t);
                self.thumb_requested.insert(new);
            }
            self.thumb_requested.remove(&old);
            self.selection.remove(&old);
            self.compare_textures.remove(&old);
            self.compare_details.remove(&old);
        }
    }

    /// "In Papierkorb verschieben": files go to the session trash with all their variants; a
    /// selection of further variants only deletes those variants (after asking).
    pub fn trash_selection(&mut self) {
        let selected = self.selected_paths();
        if !selected.is_empty() && selected.iter().all(|p| variant::number(p) > 1) {
            self.request_delete_variant();
        } else {
            self.move_to_session_folder(SessionFolder::Trash);
        }
    }

    pub fn move_to_session_folder(&mut self, f: SessionFolder) {
        match self.session.as_ref().map(|s| s.folder(f)) {
            Some(dest) => self.move_selection_to(&dest),
            None => self.status = format!("„{}“ gibt es nur in einer Session", f.label()),
        }
    }

    pub fn move_to_folder_dialog(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().set_title("Auswahl verschieben nach").set_directory(&self.folder).pick_folder() {
            self.move_selection_to(&dir);
        }
    }

    /// Asks before emptying the trash.
    pub fn request_empty_trash(&mut self) {
        if let Some(trash) = self.session.as_ref().map(|s| s.folder(SessionFolder::Trash)) {
            self.session_ui.confirm_empty_trash = Some(catalog::scan_folder(&trash, true).len());
        }
    }

    /// Deletes the images in the session trash for good.
    pub fn empty_trash(&mut self) {
        let Some((trash, root)) = self.session.as_ref().map(|s| (s.folder(SessionFolder::Trash), s.root.clone())) else { return };
        if self.primary_path().is_some_and(|p| p.starts_with(&trash)) {
            self.close_current();
        }
        let (mut n, mut error) = (0, None);
        for f in catalog::scan_folder(&trash, true) {
            let variants = variant::of_file(&f);
            match session::delete_image(&f) {
                Ok(()) => {
                    n += 1;
                    if let Some(c) = &self.catalog {
                        let _ = c.remove_path(&f);
                    }
                    for v in variants {
                        cache::remove(&v, Some(&root));
                        self.thumbs.remove(&v);
                        self.selection.remove(&v);
                    }
                }
                Err(e) => error = Some(format!("{e:#}")),
            }
        }
        self.reload_entries();
        self.status = match error {
            Some(e) => format!("{n} gelöscht, Fehler: {e}"),
            None => format!("Papierkorb geleert ({n})"),
        };
    }

    pub fn toggle_favorite(&mut self, folder: &Path) {
        let Some(c) = &self.catalog else { return };
        let _ = if self.lists.favorites.iter().any(|f| f == folder) { c.remove_favorite(folder) } else { c.add_favorite(folder) };
        self.refresh_catalog_lists();
    }

    pub(crate) fn run_pending(&mut self) {
        match self.pending.take() {
            Some(PendingAction::MoveTo(dest)) => self.move_selection_to(&dest),
            Some(PendingAction::MoveToDialog) => self.move_to_folder_dialog(),
            Some(PendingAction::NewVariant { clone }) => self.new_variant(clone),
            Some(PendingAction::DeleteVariant) => self.request_delete_variant(),
            Some(PendingAction::Trash) => self.trash_selection(),
            Some(PendingAction::Stack(a)) => self.run_stack_action(a),
            None => {}
        }
    }

    // ---------------------------------------------------------------- menus

    /// "Move to" entries for the image menu and the thumbnail context menu (they act on the selection).
    pub fn move_menu(&mut self, ui: &mut Ui) {
        let n = self.selected_paths().len();
        ui.add_enabled_ui(n > 0, |ui| {
            if let Some(s) = &self.session {
                let selects = s.folder(SessionFolder::Selects);
                if ui.add(Button::new(format!("{}  Nach Auswahl verschieben", icon::CHECK_CIRCLE)).shortcut_text("Strg+J")).clicked() {
                    self.pending = Some(PendingAction::MoveTo(selects));
                }
                ui.menu_button(format!("{}  Verschieben nach", icon::ARROW_BEND_UP_RIGHT), |ui| {
                    for f in [SessionFolder::Capture, SessionFolder::Selects] {
                        if ui.add_enabled(self.folder != s.folder(f), Button::new(format!("{}  {}", folder_icon(f), f.label()))).clicked() {
                            self.pending = Some(PendingAction::MoveTo(s.folder(f)));
                        }
                    }
                    if !self.lists.favorites.is_empty() {
                        ui.separator();
                    }
                    for fav in &self.lists.favorites {
                        if ui.add_enabled(self.folder != *fav, Button::new(format!("{}  {}", icon::STAR, dir_name(fav)))).clicked() {
                            self.pending = Some(PendingAction::MoveTo(fav.clone()));
                        }
                    }
                    ui.separator();
                    if ui.button(format!("{}  Ordner wählen …", icon::FOLDER_OPEN)).clicked() {
                        self.pending = Some(PendingAction::MoveToDialog);
                    }
                });
                if ui.add(Button::new(format!("{}  In Papierkorb verschieben", icon::TRASH)).shortcut_text("Strg+Entf")).clicked() {
                    self.pending = Some(PendingAction::Trash);
                }
            } else if ui.button(format!("{}  Verschieben nach …", icon::ARROW_BEND_UP_RIGHT)).clicked() {
                self.pending = Some(PendingAction::MoveToDialog);
            }
        });
    }

    /// Session entries of the "Datei" menu.
    pub fn session_file_menu(&mut self, ui: &mut Ui) {
        if ui.add(Button::new(format!("{}  Neue Session …", icon::FOLDER_STAR)).shortcut_text("Strg+N")).clicked() {
            self.session_ui.start_new(&self.folder);
        }
        if ui.add(Button::new(format!("{}  Session öffnen …", icon::FOLDER_OPEN)).shortcut_text("Strg+Umschalt+O")).clicked() {
            self.open_session_dialog();
        }
        let recent: Vec<PathBuf> = self.recent_sessions.iter().filter(|p| Some(*p) != self.session.as_ref().map(|s| &s.file)).cloned().collect();
        ui.add_enabled_ui(!recent.is_empty(), |ui| {
            ui.menu_button(format!("{}  Zuletzt verwendet", icon::CLOCK_COUNTER_CLOCKWISE), |ui| {
                for file in &recent {
                    let name = file.file_stem().unwrap_or_default().to_string_lossy().to_string();
                    if ui.button(name).on_hover_text(file.display().to_string()).clicked() {
                        self.open_session(file);
                    }
                }
            });
        });
        if self.session.is_some() {
            if ui.button(format!("{}  Papierkorb leeren …", icon::TRASH)).clicked() {
                self.request_empty_trash();
            }
            if ui.button(format!("{}  Session schließen", icon::SIGN_OUT)).on_hover_text("Zurück zum Katalog").clicked() {
                self.close_session();
            }
        }
    }

    // ---------------------------------------------------------------- library tab

    /// Session folders and favorites at the top of the library tab.
    pub fn session_sections(&mut self, ui: &mut Ui) {
        let Some(s) = self.session.clone() else { return };
        let mut open: Option<PathBuf> = None;
        let mut toggle_fav: Option<PathBuf> = None;
        let mut empty_trash = false;
        let mut close = false;
        section(ui, &format!("Session „{}“", s.name), None, |ui| {
            for f in SessionFolder::ALL {
                let dir = s.folder(f);
                let count: usize = self.lists.folders.iter().filter(|(p, _)| p.starts_with(&dir)).map(|(_, n)| n).sum();
                let text = match f {
                    SessionFolder::Output => format!("{}  {}", folder_icon(f), f.label()),
                    _ => format!("{}  {} ({count})", folder_icon(f), f.label()),
                };
                let selected = matches!(&self.scope, Scope::Folder { path, .. } if *path == dir);
                let mut resp = ui.selectable_label(selected, text);
                if f == SessionFolder::Output {
                    resp = resp.on_hover_text("Verarbeitete Bilder im Dateimanager zeigen");
                }
                if resp.clicked() {
                    if f == SessionFolder::Output {
                        if let Err(e) = reveal(&dir) {
                            self.status = format!("Dateimanager nicht verfügbar: {e}");
                        }
                    } else {
                        open = Some(dir.clone());
                    }
                }
                resp.context_menu(|ui| {
                    if f == SessionFolder::Trash && ui.add_enabled(count > 0, Button::new("Papierkorb leeren …")).clicked() {
                        empty_trash = true;
                        ui.close();
                    }
                    if ui.button("Im Dateimanager zeigen").clicked() {
                        if let Err(e) = reveal(&dir) {
                            self.status = format!("Dateimanager nicht verfügbar: {e}");
                        }
                        ui.close();
                    }
                });
            }
            ui.horizontal(|ui| {
                if ui.button(format!("{}  Importieren …", icon::FOLDER_PLUS)).on_hover_text("Bilder in den Ordner „Aufnahmen“ kopieren").clicked() {
                    self.import_folder_dialog();
                }
                if ui.button(format!("{}  Schließen", icon::SIGN_OUT)).on_hover_text("Session schließen, zurück zum Katalog").clicked() {
                    close = true;
                }
            });
        });

        section(ui, "Session-Favoriten", None, |ui| {
            for fav in &self.lists.favorites {
                let selected = matches!(&self.scope, Scope::Folder { path, .. } if path == fav);
                let mut text = RichText::new(format!("{}  {}", icon::STAR, dir_name(fav)));
                if !fav.is_dir() {
                    text = text.color(theme::TEXT_DIM).strikethrough();
                }
                let resp = ui.selectable_label(selected, text).on_hover_text(fav.display().to_string());
                if resp.clicked() && fav.is_dir() {
                    open = Some(fav.clone());
                }
                resp.context_menu(|ui| {
                    if ui.button("Aus Favoriten entfernen").clicked() {
                        toggle_fav = Some(fav.clone());
                        ui.close();
                    }
                });
            }
            if self.lists.favorites.is_empty() {
                ui.label(RichText::new("Häufig genutzte Ordner hier ablegen.").color(Color32::from_gray(150)));
            }
            let is_fav = self.lists.favorites.contains(&self.folder);
            let is_std = SessionFolder::ALL.iter().any(|f| s.folder(*f) == self.folder);
            if ui
                .add_enabled(!is_fav && !is_std, Button::new(format!("{}  Aktuellen Ordner hinzufügen", icon::STAR)))
                .on_hover_text(self.folder.display().to_string())
                .clicked()
            {
                toggle_fav = Some(self.folder.clone());
            }
        });

        if let Some(f) = toggle_fav {
            self.toggle_favorite(&f);
        }
        if empty_trash {
            self.request_empty_trash();
        }
        if close {
            self.close_session();
        } else if let Some(dir) = open {
            self.open_folder(dir);
        }
    }

    // ---------------------------------------------------------------- dialogs

    pub(crate) fn session_dialogs(&mut self, ctx: &egui::Context) {
        if self.session_ui.creating {
            let mut create = false;
            let mut cancel = false;
            let modal = egui::Modal::new(egui::Id::new("new_session")).show(ctx, |ui| {
                ui.set_width(440.0);
                ui.label(RichText::new(format!("{}  Neue Session", icon::FOLDER_STAR)).size(17.0).strong().color(Color32::WHITE));
                ui.add_space(10.0);
                egui::Grid::new("new_session_grid").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
                    ui.label("Name");
                    let resp = ui.add(egui::TextEdit::singleline(&mut self.session_ui.name).hint_text("z. B. Hochzeit Müller").desired_width(320.0));
                    if !resp.has_focus() && self.session_ui.name.is_empty() {
                        resp.request_focus();
                    }
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        create = true;
                    }
                    ui.end_row();
                    ui.label("Speicherort");
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(self.session_ui.location.display().to_string()).color(theme::TEXT_DIM));
                        if ui.button("Ändern …").clicked()
                            && let Some(dir) = rfd::FileDialog::new().set_title("Speicherort der Session").set_directory(&self.session_ui.location).pick_folder()
                        {
                            self.session_ui.location = dir;
                        }
                    });
                    ui.end_row();
                });
                let name = self.session_ui.name.trim();
                if !name.is_empty() {
                    ui.add_space(6.0);
                    let root = self.session_ui.location.join(name);
                    let folders = SessionFolder::ALL.map(|f| f.dir_name()).join(", ");
                    ui.label(RichText::new(format!("Wird angelegt: {}\nmit den Ordnern {folders}", root.display())).small().color(theme::TEXT_DIM));
                }
                if let Some(e) = &self.session_ui.error {
                    ui.add_space(6.0);
                    ui.label(RichText::new(e).color(Color32::from_rgb(230, 110, 100)));
                }
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ok = Button::new(RichText::new("Anlegen").color(Color32::WHITE)).fill(theme::ACCENT).min_size(vec2(90.0, 26.0));
                    if ui.add_enabled(!name.is_empty(), ok).clicked() {
                        create = true;
                    }
                    if ui.add(Button::new("Abbrechen").min_size(vec2(90.0, 26.0))).clicked() {
                        cancel = true;
                    }
                });
            });
            if create {
                let (location, name) = (self.session_ui.location.clone(), self.session_ui.name.clone());
                match self.create_session(&location, &name) {
                    Ok(()) => self.session_ui.creating = false,
                    Err(e) => self.session_ui.error = Some(format!("{e:#}")),
                }
            }
            if cancel || modal.should_close() {
                self.session_ui.creating = false;
            }
        }

        if let Some(n) = self.session_ui.confirm_empty_trash {
            let mut confirmed = false;
            let mut cancel = false;
            let modal = egui::Modal::new(egui::Id::new("empty_trash")).show(ctx, |ui| {
                ui.set_width(360.0);
                ui.label(RichText::new(format!("{}  Papierkorb leeren", icon::TRASH)).size(17.0).strong().color(Color32::WHITE));
                ui.add_space(8.0);
                ui.label(match n {
                    0 => "Der Papierkorb ist leer.".to_string(),
                    1 => "1 Bild wird endgültig gelöscht.".to_string(),
                    n => format!("{n} Bilder werden endgültig gelöscht."),
                });
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let del = Button::new(RichText::new("Löschen").color(Color32::WHITE)).fill(Color32::from_rgb(180, 60, 50)).min_size(vec2(90.0, 26.0));
                    if ui.add_enabled(n > 0, del).clicked() {
                        confirmed = true;
                    }
                    if ui.add(Button::new("Abbrechen").min_size(vec2(90.0, 26.0))).clicked() {
                        cancel = true;
                    }
                });
            });
            if confirmed {
                self.empty_trash();
            }
            if confirmed || cancel || modal.should_close() {
                self.session_ui.confirm_empty_trash = None;
            }
        }
    }
}
