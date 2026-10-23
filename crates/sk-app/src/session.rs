//! Sessions: creating, opening and closing them, moving images between the session folders,
//! the trash, session favorites, and the matching menus, dialogs and library sections.

use std::path::{Path, PathBuf};

use egui::{Button, Modifiers, RichText, Ui, vec2};
use sk_core::cache;
use sk_core::catalog::{self, Catalog, Scope};
use sk_core::session::{self, Session, SessionFolder};
use sk_core::variant;

use crate::app::App;
use crate::dialog::{self, Dialog, Tone};
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
    ToSelects,
    NewVariant { clone: bool },
    DeleteVariant,
    Trash,
    Stack(crate::stacks::StackAction),
}

/// Folder icons, so that the session's parts read as the folders on disk they are.
pub fn folder_icon(f: SessionFolder) -> &'static str {
    match f {
        SessionFolder::Capture => icon::FOLDER_SIMPLE,
        SessionFolder::Selects => icon::FOLDER_SIMPLE_STAR,
        // A click shows it in the file manager.
        SessionFolder::Output => icon::FOLDER_NOTCH_OPEN,
        SessionFolder::Trash => icon::FOLDER_SIMPLE_MINUS,
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
    let mut child = std::process::Command::new(cmd).arg(dir).spawn().map_err(|e| format!("{cmd}: {e}"))?;
    // Reaped, or it would stay a zombie until the program ends.
    std::thread::spawn(move || child.wait());
    Ok(())
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
        // Images outside the session that are edited keep their sidecar files.
        if let Some(s) = &self.settings_store {
            s.set_adopt(false);
        }
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
        if let Some(s) = &self.settings_store {
            s.set_adopt(true);
        }
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
        // In-memory data of both paths is dropped: the new one may have belonged to another file
        // (renumbering), the old one may be taken by one next (see `relocations`).
        self.relocations += 1;
        for new in variant::of_file(new_file) {
            let old = variant::path_for(old_file, variant::number(&new));
            for p in [&old, &new] {
                self.forget_prepared(p);
                self.focus_maps.remove(p);
                self.focus_pending.remove(p);
                self.thumb_focus.remove(p);
                self.thumb_focus_requested.remove(p);
                self.compare_requested.remove(p);
            }
        }
        for new in variant::of_file(new_file) {
            let old = variant::path_for(old_file, variant::number(&new));
            if keep_cache {
                cache::rename(&old, &new, root.as_deref());
            }
            if let Some(u) = self.unsaved.remove(&old) {
                self.unsaved.insert(new.clone(), u);
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
        } else if selected.iter().any(|p| variant::number(p) > 1 && !selected.contains(&variant::file_of(p))) {
            // The file would go to the trash with all its variants, not only the selected one.
            self.status = "Die Auswahl enthält weitere Varianten ohne ihr Bild: Varianten und Bilder bitte getrennt löschen".into();
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
    /// Images in the session trash (from the folder counts of the catalog).
    pub fn trash_count(&self) -> usize {
        let Some(trash) = self.session.as_ref().map(|s| s.folder(SessionFolder::Trash)) else { return 0 };
        self.lists.folders.iter().filter(|(p, _)| p.starts_with(&trash)).map(|(_, n)| n).sum()
    }

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
            Some(PendingAction::ToSelects) => self.move_to_session_folder(SessionFolder::Selects),
            Some(PendingAction::Trash) => self.trash_selection(),
            Some(PendingAction::Stack(a)) => self.run_stack_action(a),
            None => {}
        }
    }

    // ---------------------------------------------------------------- menus

    /// "Move to" and trash entries for the image menu and the thumbnail context menu (they act
    /// on the selection). They look like the commands but run after the browser is drawn.
    pub fn move_menu(&mut self, ui: &mut Ui) {
        let n = self.selected_count();
        ui.add_enabled_ui(n > 0, |ui| {
            if self.session.is_some() && self.command_button(ui, "to-selects") {
                self.pending = Some(PendingAction::ToSelects);
            }
            if let Some(s) = &self.session {
                crate::widgets::menu_button(ui, format!("{}  Verschieben nach", icon::ARROW_BEND_UP_RIGHT), |ui| {
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
            } else if ui.button(format!("{}  Verschieben nach …", icon::ARROW_BEND_UP_RIGHT)).clicked() {
                self.pending = Some(PendingAction::MoveToDialog);
            }
            if self.command_button(ui, "trash") {
                self.pending = Some(PendingAction::Trash);
            }
        });
    }

    /// Session entries of the "Datei" menu.
    pub fn session_file_menu(&mut self, ui: &mut Ui) {
        if ui.add(Button::new(format!("{}  Neue Session …", icon::FOLDER_STAR)).shortcut_text(crate::commands::shortcut_of("new-session"))).clicked() {
            self.session_ui.start_new(&self.folder);
        }
        if ui.add(Button::new(format!("{}  Session öffnen …", icon::FOLDER_OPEN)).shortcut_text(crate::commands::shortcut_of("open-session"))).clicked() {
            self.open_session_dialog();
        }
        let recent: Vec<PathBuf> = self.recent_sessions.iter().filter(|p| Some(*p) != self.session.as_ref().map(|s| &s.file)).cloned().collect();
        ui.add_enabled_ui(!recent.is_empty(), |ui| {
            crate::widgets::menu_button(ui, format!("{}  Zuletzt verwendet", icon::CLOCK_COUNTER_CLOCKWISE), |ui| {
                for file in &recent {
                    let name = file.file_stem().unwrap_or_default().to_string_lossy().to_string();
                    if ui.button(name).on_hover_text(file.display().to_string()).clicked() {
                        self.open_session(file);
                    }
                }
            });
        });
        if self.session.is_some() && ui.add_enabled(self.trash_count() > 0, Button::new(format!("{}  Papierkorb leeren …", icon::TRASH))).clicked() {
            self.request_empty_trash();
        }
        self.command_item(ui, "close-session");
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
                let resp = ui.selectable_label(selected, text);
                let resp = match f {
                    SessionFolder::Output => resp.on_hover_text(format!("Ordner {}\nKlick: im Dateimanager zeigen", dir.display())),
                    _ => resp.on_hover_text(format!("Ordner {}", dir.display())),
                };
                if resp.clicked() {
                    if f == SessionFolder::Output {
                        if let Err(e) = reveal(&dir) {
                            self.status = format!("Dateimanager nicht verfügbar: {e}");
                        }
                    } else {
                        open = Some(dir.clone());
                    }
                }
                crate::widgets::context_menu(&resp, |ui| {
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
            self.hot_folder_ui(ui);
            ui.horizontal(|ui| {
                if ui.button(format!("{}  Importieren …", icon::FOLDER_PLUS)).on_hover_text("Bilder in den Ordner „Aufnahmen“ kopieren").clicked() {
                    self.import_folder_dialog();
                }
                if ui.button(format!("{}  Session schließen", icon::SIGN_OUT)).on_hover_text(crate::commands::hint("Zurück zum Katalog", "close-session")).clicked() {
                    close = true;
                }
            });
        });

        let reachable = favorites_reachable(ui, &self.lists.favorites);
        section(ui, "Session-Favoriten", None, |ui| {
            for (fav, reachable) in self.lists.favorites.iter().zip(reachable) {
                let selected = matches!(&self.scope, Scope::Folder { path, .. } if path == fav);
                let mut text = RichText::new(format!("{}  {}", icon::STAR, dir_name(fav)));
                if !reachable {
                    text = text.color(theme::TEXT_DIM).strikethrough();
                }
                let resp = ui.selectable_label(selected, text).on_hover_text(fav.display().to_string());
                if resp.clicked() && reachable {
                    open = Some(fav.clone());
                }
                crate::widgets::context_menu(&resp, |ui| {
                    if ui.button("Aus Favoriten entfernen").clicked() {
                        toggle_fav = Some(fav.clone());
                        ui.close();
                    }
                });
            }
            if self.lists.favorites.is_empty() {
                crate::widgets::hint(ui, "Häufig genutzte Ordner hier ablegen.");
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
            let modal = Dialog::new("new_session", icon::FOLDER_STAR, "Neue Session").subtitle("Ein Ordner für ein Shooting, mit festen Unterordnern").width(470.0).show(ctx, |ui| {
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
                    dialog::note(ui, format!("Wird angelegt: {}\nmit den Ordnern {folders}", root.display()));
                }
                if let Some(e) = &self.session_ui.error {
                    ui.add_space(6.0);
                    ui.label(RichText::new(e).color(theme::DANGER));
                }
                dialog::footer(ui, |ui| {
                    if dialog::default_button(ui, !name.is_empty(), dialog::primary("Anlegen")) {
                        create = true;
                    }
                    if ui.add(dialog::secondary("Abbrechen")).clicked() {
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
            let modal = Dialog::new("empty_trash", icon::TRASH, "Papierkorb leeren").subtitle("Lässt sich nicht rückgängig machen").tone(Tone::Danger).width(400.0).show(ctx, |ui| {
                ui.label(match n {
                    0 => "Der Papierkorb ist leer.".to_string(),
                    1 => "1 Bild wird endgültig gelöscht.".to_string(),
                    n => format!("{n} Bilder werden endgültig gelöscht."),
                });
                dialog::footer(ui, |ui| {
                    if ui.add_enabled(n > 0, dialog::danger("Löschen")).clicked() {
                        confirmed = true;
                    }
                    if ui.add(dialog::secondary("Abbrechen")).clicked() {
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

/// Whether the favorites are reachable: looked at every few seconds in a thread of its own (an
/// unreachable network path takes Windows seconds to answer); taken as reachable until known.
fn favorites_reachable(ui: &egui::Ui, favorites: &[PathBuf]) -> Vec<bool> {
    /// A check's result, filled in by its thread.
    type Pending = std::sync::Arc<std::sync::Mutex<Option<Vec<bool>>>>;
    #[derive(Clone, Default)]
    struct Reach {
        checked: f64,
        favorites: Vec<PathBuf>,
        reachable: Vec<bool>,
        /// The check under way: for which favorites, and its result once there.
        running: Option<(Vec<PathBuf>, Pending)>,
    }
    let id = egui::Id::new("favorites reachable");
    let now = ui.input(|i| i.time);
    let mut r: Reach = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    if let Some((of, done)) = r.running.as_ref().and_then(|(of, m)| Some((of.clone(), m.lock().unwrap_or_else(|e| e.into_inner()).take()?))) {
        r.running = None;
        // Of a list changed meanwhile: looked at again at once.
        r.checked = if of == favorites { now } else { f64::NEG_INFINITY };
        if of == favorites {
            r.reachable = done;
        }
    }
    if r.favorites != favorites {
        r = Reach { checked: f64::NEG_INFINITY, favorites: favorites.to_vec(), running: r.running, ..Default::default() };
    }
    if r.running.is_none() && now - r.checked >= 5.0 {
        let result = std::sync::Arc::new(std::sync::Mutex::new(None));
        let (paths, out, ctx) = (favorites.to_vec(), result.clone(), ui.ctx().clone());
        let spawned = std::thread::Builder::new().name("Favoriten prüfen".into()).spawn(move || {
            let reachable = paths.iter().map(|p| p.is_dir()).collect();
            *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(reachable);
            ctx.request_repaint();
        });
        if spawned.is_ok() {
            r.running = Some((favorites.to_vec(), result));
        }
        r.checked = now;
    }
    let mut reachable = r.reachable.clone();
    reachable.resize(favorites.len(), true);
    ui.data_mut(|d| d.insert_temp(id, r));
    reachable
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The favorites are looked at in the background; until then they count as reachable.
    #[test]
    fn favorites_are_checked_in_the_background() {
        let ctx = egui::Context::default();
        let favorites = vec![std::env::temp_dir(), PathBuf::from("/gibt/es/nicht")];
        let mut seen = Vec::new();
        for _ in 0..300 {
            ctx.run_ui(Default::default(), |ui| seen.push(favorites_reachable(ui, &favorites))).textures_delta.clear();
            if seen.last().is_some_and(|r| r == &[true, false]) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(seen[0], [true, true]);
        assert_eq!(seen.last().unwrap(), &[true, false]);
    }
}
