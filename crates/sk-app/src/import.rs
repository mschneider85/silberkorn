//! Importing into the catalog or a session: the files of a folder with their previews, to choose
//! from. Files already there (same name, size and capture time) are marked and not chosen. A session copies into its Capture folder; the catalog keeps the files
//! where they are or copies them, into its own picture folder or another one.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::Arc;

use egui::{Color32, ColorImage, Rect, RichText, Sense, Stroke, TextureHandle, vec2};
use serde::{Deserialize, Serialize};
use sk_core::catalog::{self, Fingerprint};
use sk_core::{naming, raw};

use crate::app::App;
use crate::dialog::{self, Dialog, Tone};
use crate::theme::{self, icon};

/// Edge of a preview tile (points) and of the previews read for it (pixels).
const TILE: f32 = 128.0;
const THUMB: u32 = 200;
const CAPTION: f32 = 18.0;
const GAP: f32 = 8.0;
/// Previews are read up to this many files before and after the first one in view, and dropped
/// beyond twice that (about 100 KB each; a card can hold thousands of pictures).
const NEAR: usize = 300;

/// Where the catalog import puts the files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    /// "Nicht kopieren": the files stay where they are.
    #[default]
    InPlace,
    /// "In den Bilderordner kopieren": copied into the catalog's picture folder.
    Library,
    /// "In einen Ordner kopieren": a folder of one's choice.
    Folder,
}

/// The choices of the catalog import, kept for the next one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportOptions {
    pub target: Target,
    /// The catalog's picture folder (`None`: [`default_library`]).
    pub library: Option<PathBuf>,
    /// The folder of [`Target::Folder`].
    pub folder: Option<PathBuf>,
    /// Copies go into subfolders by capture date (see [`date_folder`]).
    pub by_date: bool,
    /// Copies are named after `template` (see `naming`); also in a session.
    pub rename: bool,
    pub template: String,
}

/// Template proposed for renaming on import.
const TEMPLATE: &str = "{Datum}_{Zeit}_{Name}";

impl Default for ImportOptions {
    fn default() -> Self {
        Self { target: Target::InPlace, library: None, folder: None, by_date: true, rename: false, template: TEMPLATE.into() }
    }
}

impl ImportOptions {
    pub fn library(&self) -> PathBuf {
        self.library.clone().unwrap_or_else(default_library)
    }

    /// Where the copies go (`None`: they stay where they are, or no folder is chosen yet).
    pub fn copy_to(&self) -> Option<CopyTo> {
        let folder = match self.target {
            Target::InPlace => return None,
            Target::Library => self.library(),
            Target::Folder => self.folder.clone()?,
        };
        Some(CopyTo { folder, by_date: self.by_date, rename: self.template(), session: String::new() })
    }

    /// The template the copies are named after, if they are renamed.
    pub fn template(&self) -> Option<String> {
        (self.rename && !self.template.trim().is_empty()).then(|| self.template.clone())
    }
}

/// Copy the imported files into `folder` (into subfolders by capture date with `by_date`),
/// named after the template `rename` (`session` for its `{Session}`).
#[derive(Clone, Debug, PartialEq)]
pub struct CopyTo {
    pub folder: PathBuf,
    pub by_date: bool,
    pub rename: Option<String>,
    pub session: String,
}

impl CopyTo {
    /// The folder and the name (stem) for `image`, the `counter`th file copied.
    pub fn place_of(&self, image: &Path, counter: u32) -> (PathBuf, Option<String>) {
        let info = (self.by_date || self.rename.is_some())
            .then(|| crate::guard::catch(|| raw::read_meta(image).ok().map(|m| m.info)).ok().flatten())
            .flatten();
        let folder = if self.by_date { self.folder.join(date_folder(info.as_ref().and_then(|i| i.date.as_deref()))) } else { self.folder.clone() };
        let name = self.rename.as_ref().map(|t| naming::expand(t, &naming::NameFields { image, capture: info.as_ref(), counter, session: &self.session }));
        (folder, name)
    }
}

/// `Bilder/Silberkorn` (`Pictures/Silberkorn`) in the home folder.
pub fn default_library() -> PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default();
    let pictures = ["Bilder", "Pictures"].into_iter().map(|n| home.join(n)).find(|p| p.is_dir()).unwrap_or_else(|| home.join("Pictures"));
    pictures.join("Silberkorn")
}

/// Subfolder for a picture taken at `date` (EXIF "2026:10:18 14:03:00"): `2026/10-18`.
pub fn date_folder(date: Option<&str>) -> PathBuf {
    let digits = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_digit());
    let mut parts = date.and_then(|d| d.get(..10)).map(|d| d.split([':', '-'])).into_iter().flatten();
    match (parts.next(), parts.next(), parts.next()) {
        (Some(y), Some(m), Some(d)) if digits(y, 4) && digits(m, 2) && digits(d, 2) && y != "0000" => PathBuf::from(y).join(format!("{m}-{d}")),
        _ => PathBuf::from("Ohne Datum"),
    }
}

/// The open import dialog.
pub struct ImportDialog {
    pub source: PathBuf,
    pub recursive: bool,
    pub items: Vec<Item>,
    /// Last tile clicked, the start of a range clicked with Shift.
    anchor: Option<usize>,
    loader: Loader,
    /// Capture data of the files in the name preview.
    infos: std::collections::HashMap<PathBuf, Option<raw::CaptureInfo>>,
}

/// Files shown in the name preview.
const PREVIEW: usize = 3;

pub struct Item {
    pub path: PathBuf,
    name: String,
    pub chosen: bool,
    /// Chosen or not by hand: the duplicate check then leaves it as it is.
    touched: bool,
    /// Already in the session (`None` while checking).
    pub known: Option<bool>,
    thumb: Option<TextureHandle>,
}

enum Found {
    Known(usize, bool),
    Thumb(usize, ColorImage),
}

/// Reads previews and checks for duplicates in the background; stops when the dialog closes.
struct Loader {
    rx: Receiver<Found>,
    stop: Arc<AtomicBool>,
    previews: Arc<Previews>,
}

/// Which previews are read or being read, and where the view is: those from there on come
/// first.
struct Previews {
    taken: std::sync::Mutex<Vec<bool>>,
    first_shown: AtomicUsize,
    /// Woken when there may be more to read (scrolled, dropped, closed); with `taken`.
    more: std::sync::Condvar,
}

impl Previews {
    fn new(n: usize, first_shown: usize) -> Self {
        Self { taken: std::sync::Mutex::new(vec![false; n]), first_shown: AtomicUsize::new(first_shown), more: std::sync::Condvar::new() }
    }

    /// The next preview to read: the first not taken from the view on, then those before it,
    /// all within [`NEAR`].
    #[cfg(test)]
    fn next(&self) -> Option<usize> {
        self.next_in(&mut self.taken.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn next_in(&self, taken: &mut [bool]) -> Option<usize> {
        let from = self.first_shown.load(Ordering::Relaxed).min(taken.len());
        let i = (from..(from + NEAR).min(taken.len())).chain((from.saturating_sub(NEAR)..from).rev()).find(|&i| !taken[i])?;
        taken[i] = true;
        Some(i)
    }

    /// Like `next`, but waits until there is one; `None` once `stop` is set.
    fn wait_next(&self, stop: &AtomicBool) -> Option<usize> {
        let mut taken = self.taken.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if stop.load(Ordering::Relaxed) {
                return None;
            }
            if let Some(i) = self.next_in(&mut taken) {
                return Some(i);
            }
            taken = self.more.wait(taken).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// The view now starts at preview `i`.
    fn show_from(&self, i: usize) {
        if self.first_shown.load(Ordering::Relaxed) != i {
            let _taken = self.taken.lock().unwrap_or_else(|e| e.into_inner());
            self.first_shown.store(i, Ordering::Relaxed);
            self.more.notify_all();
        }
    }

    /// Preview `i` was dropped: read it again when needed.
    fn forget(&self, i: usize) {
        if let Some(t) = self.taken.lock().unwrap_or_else(|e| e.into_inner()).get_mut(i) {
            *t = false;
            self.more.notify_all();
        }
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        // Set under the lock: a thread about to wait would miss the wake-up otherwise.
        let _taken = self.previews.taken.lock().unwrap_or_else(|e| e.into_inner());
        self.stop.store(true, Ordering::Relaxed);
        self.previews.more.notify_all();
    }
}

impl Loader {
    /// Files inside `inside` are known as they are (the session's own).
    fn start(ctx: &egui::Context, paths: Vec<PathBuf>, inside: Option<PathBuf>, known: HashSet<Fingerprint>) -> Self {
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let paths = Arc::new(paths);
        {
            let (tx, stop, paths, ctx) = (tx.clone(), stop.clone(), paths.clone(), ctx.clone());
            std::thread::Builder::new()
                .name("Import prüfen".into())
                .spawn(move || {
                    let names: HashSet<(u64, &str)> = known.iter().map(|k| (k.size, k.name.as_str())).collect();
                    for (i, path) in paths.iter().enumerate() {
                        if stop.load(Ordering::Relaxed) {
                            return;
                        }
                        let found = inside.as_ref().is_some_and(|r| path.starts_with(r)) || is_known(path, &names, &known);
                        if tx.send(Found::Known(i, found)).is_err() {
                            return;
                        }
                        if i % 20 == 19 || i + 1 == paths.len() {
                            ctx.request_repaint();
                        }
                    }
                })
                .ok();
        }
        let previews = Arc::new(Previews::new(paths.len(), 0));
        for _ in 0..std::thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 4)) {
            let (tx, stop, paths, previews, ctx) = (tx.clone(), stop.clone(), paths.clone(), previews.clone(), ctx.clone());
            std::thread::Builder::new()
                .name("Import-Vorschauen".into())
                .spawn(move || {
                    // All near the view read: waits for scrolling.
                    while let Some(i) = previews.wait_next(&stop) {
                        let path = &paths[i];
                        // A broken file shows no preview; the others go on.
                        let thumb = crate::guard::catch(|| raw::embedded_preview_at_least(path, THUMB as usize).ok().map(|img| img.thumbnail(THUMB, THUMB)));
                        if let Ok(Some(img)) = thumb {
                            if tx.send(Found::Thumb(i, crate::workers::to_color_image(&img))).is_err() {
                                return;
                            }
                            ctx.request_repaint();
                        }
                    }
                })
                .ok();
        }
        Self { rx, stop, previews }
    }
}

/// Whether a file with the name, size and capture time of `path` is in the catalog. Only files
/// whose name and size are found there are read.
fn is_known(path: &Path, names: &HashSet<(u64, &str)>, known: &HashSet<Fingerprint>) -> bool {
    let Ok(size) = std::fs::metadata(path).map(|m| m.len()) else { return false };
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    if !names.contains(&(size, name.as_str())) {
        return false;
    }
    let date = crate::guard::catch(|| raw::read_meta(path).ok().map(|m| m.info.date));
    match date {
        Ok(Some(date)) => known.contains(&Fingerprint { size, date, name }),
        _ => false,
    }
}

impl ImportDialog {
    fn chosen(&self) -> Vec<PathBuf> {
        self.items.iter().filter(|i| i.chosen).map(|i| i.path.clone()).collect()
    }

    pub(crate) fn checking(&self) -> bool {
        self.items.iter().any(|i| i.known.is_none())
    }

    /// Takes what the loader found so far.
    pub fn poll(&mut self, ctx: &egui::Context) {
        for found in self.loader.rx.try_iter() {
            match found {
                Found::Known(i, known) => {
                    let item = &mut self.items[i];
                    item.known = Some(known);
                    if known && !item.touched {
                        item.chosen = false;
                    }
                }
                Found::Thumb(i, image) => {
                    self.items[i].thumb = Some(ctx.load_texture(format!("import:{i}"), image, egui::TextureOptions::LINEAR));
                }
            }
        }
        // Far from the view: dropped (read again when scrolled back).
        let at = self.loader.previews.first_shown.load(Ordering::Relaxed);
        for (i, item) in self.items.iter_mut().enumerate() {
            if item.thumb.is_some() && i.abs_diff(at) > 2 * NEAR {
                item.thumb = None;
                self.loader.previews.forget(i);
            }
        }
    }

    /// A click on tile `i`: toggles it, or with Shift the range from the last click.
    fn click(&mut self, i: usize, shift: bool) {
        let chosen = !self.items[i].chosen;
        let range = match self.anchor.filter(|_| shift) {
            Some(a) => a.min(i)..=a.max(i),
            None => i..=i,
        };
        for item in &mut self.items[range] {
            item.chosen = chosen;
            item.touched = true;
        }
        self.anchor = Some(i);
    }
}

impl App {
    /// "Bilder in die Session importieren …" / "Bilder in den Katalog importieren …": the files of `source` to choose from.
    pub fn open_import(&mut self, source: PathBuf, recursive: bool) {
        let paths = catalog::scan_folder(&source, recursive);
        let known = self.catalog.as_ref().and_then(|c| c.fingerprints().ok()).unwrap_or_default();
        let items = paths
            .iter()
            .map(|p| Item {
                path: p.clone(),
                name: p.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                chosen: true,
                touched: false,
                known: None,
                thumb: None,
            })
            .collect();
        let inside = self.session.as_ref().map(|s| s.root.clone());
        let loader = Loader::start(&self.workers.context(), paths, inside, known);
        self.import_source = Some(source.clone());
        self.import = Some(ImportDialog { source, recursive, items, anchor: None, loader, infos: Default::default() });
    }

    pub(crate) fn import_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.import else { return };
        dialog.poll(ctx);
        let (mut apply, mut cancel, mut change_source, mut rescan) = (false, false, false, false);
        let mut pick: Option<Target> = None;
        let n = dialog.items.len();
        let chosen = dialog.items.iter().filter(|i| i.chosen).count();
        let known = dialog.items.iter().filter(|i| i.known == Some(true)).count();
        let checking = dialog.checking();
        let (source, recursive) = (dialog.source.clone(), dialog.recursive);
        let in_session = self.session.is_some();
        let session_name = self.session.as_ref().map(|s| s.name.clone()).unwrap_or_default();
        // A session copies unless the files already are in it.
        let session_copies = self.session.as_ref().is_some_and(|s| !source.starts_with(&s.root));
        let options = &mut self.import_options;
        let before = options.clone();
        // Choosing "In einen Ordner kopieren" without a folder yet asks for one.
        let ready = in_session || options.target != Target::Folder || options.folder.is_some();
        let (title, subtitle, place, badge) = if in_session {
            ("Bilder in die Session importieren", "Die gewählten Bilder werden in den Ordner „Aufnahmen“ kopiert", "in der Session", "In Session")
        } else {
            ("Bilder in den Katalog importieren", "Die gewählten Bilder werden in den Katalog aufgenommen", "im Katalog", "Im Katalog")
        };
        let modal = Dialog::new("import", icon::FOLDER_PLUS, title).subtitle(subtitle).width(820.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                // The controls first (from the right), the source in the room left, shortened in front.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Andere Quelle …").clicked() {
                        change_source = true;
                    }
                    if crate::widgets::checkbox(ui, &mut dialog.recursive, "Unterordner einbeziehen").changed() {
                        rescan = true;
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.label(RichText::new(icon::FOLDER).color(theme::TEXT_DIM));
                        let source = dialog.source.display().to_string();
                        let shown = crate::widgets::elide_start(ui, &source, ui.available_width() - 12.0);
                        ui.label(RichText::new(shown).color(theme::TEXT)).on_hover_text(format!("Quelle: {source}"));
                    });
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(n > 0, egui::Button::new("Alle")).clicked() {
                    dialog.items.iter_mut().for_each(|i| (i.chosen, i.touched) = (true, true));
                }
                if ui.add_enabled(n > 0, egui::Button::new("Keine")).clicked() {
                    dialog.items.iter_mut().for_each(|i| (i.chosen, i.touched) = (false, true));
                }
                if ui.add_enabled(known > 0, egui::Button::new("Nur neue")).on_hover_text(format!("Alle Bilder wählen, die noch nicht {place} sind")).clicked() {
                    dialog.items.iter_mut().for_each(|i| (i.chosen, i.touched) = (i.known != Some(true), true));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if checking {
                        ui.spinner();
                        dialog::note(ui, "Prüfe …");
                    } else if known > 0 {
                        dialog::pill(ui, Tone::Warn, &format!("{} bereits {place}", crate::widgets::image_count(known)));
                    }
                    dialog::note(ui, format!("{chosen} von {n} gewählt"));
                });
            });
            ui.add_space(4.0);
            grid(ui, dialog, badge, if in_session { 340.0 } else { 270.0 });
            dialog::note(ui, "Klick wählt ein Bild ab oder wieder an, Umschalt+Klick einen Bereich.");
            if !in_session {
                ui.add_space(4.0);
                // Each choice says what happens to the files ("Im Katalog" sounded like "only
                // take in", which is what leaving them does).
                dialog::card(ui, "Dateien", None, |ui| {
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut options.target, Target::InPlace, "Nicht kopieren").on_hover_text("Die Bilder bleiben, wo sie sind");
                        ui.radio_value(&mut options.target, Target::Library, "In den Bilderordner kopieren").on_hover_text("In den Bilderordner des Katalogs kopieren");
                        if ui.radio(options.target == Target::Folder, "In einen Ordner kopieren").on_hover_text("In einen Ordner nach Wahl kopieren").clicked() {
                            options.target = Target::Folder;
                            if options.folder.is_none() {
                                pick = Some(Target::Folder);
                            }
                        }
                    });
                    let folder = match options.target {
                        Target::InPlace => None,
                        Target::Library => Some(Some(options.library())),
                        Target::Folder => Some(options.folder.clone()),
                    };
                    match folder {
                        None => dialog::note(ui, "Die Bilder werden nur in den Katalog aufgenommen, die Dateien bleiben unverändert."),
                        Some(folder) => {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(icon::FOLDER).color(theme::TEXT_DIM));
                                match &folder {
                                    Some(f) => ui.label(RichText::new(f.display().to_string()).color(theme::TEXT)),
                                    None => ui.label(RichText::new("Noch kein Ordner gewählt").color(theme::TEXT_DIM)),
                                };
                                if ui.button(if folder.is_some() { "Ändern …" } else { "Wählen …" }).clicked() {
                                    pick = Some(options.target);
                                }
                            });
                            let example = date_folder(Some("2026:10:18")).display().to_string();
                            crate::widgets::checkbox(ui, &mut options.by_date, format!("Unterordner nach Aufnahmedatum ({example})"));
                        }
                    }
                });
            }
            let copies = if in_session { session_copies } else { options.target != Target::InPlace };
            ui.add_space(4.0);
            names_card(ui, dialog, options, copies, &session_name);
            dialog::footer(ui, |ui| {
                let label = if checking { "Prüfe …".to_string() } else { format!("{} importieren", crate::widgets::image_count(chosen)) };
                if dialog::default_button(ui, chosen > 0 && !checking && ready, dialog::primary(label)) {
                    apply = true;
                }
                if ui.add(dialog::secondary("Abbrechen")).clicked() {
                    cancel = true;
                }
            });
        });
        if let Some(target) = pick {
            let start = match target {
                Target::Folder => options.folder.clone(),
                _ => Some(options.library()),
            };
            let picked = rfd::FileDialog::new().set_title("Ziel wählen").set_directory(start.unwrap_or_else(default_library)).pick_folder();
            match (target, picked) {
                (Target::Folder, Some(dir)) => options.folder = Some(dir),
                (_, Some(dir)) => options.library = Some(dir),
                // Without a folder "In einen Ordner kopieren" can't be used.
                (Target::Folder, None) if options.folder.is_none() => options.target = before.target,
                _ => {}
            }
        }
        if *options != before {
            self.save_settings();
        }
        if apply && let Some(d) = self.import.take() {
            let copy_to = self.import_copy_to(&d.source);
            self.import_files(d.chosen(), copy_to);
        } else if change_source {
            if let Some(dir) = rfd::FileDialog::new().set_title("Quelle wählen").set_directory(&source).pick_folder() {
                self.open_import(dir, recursive);
            }
        } else if rescan {
            // The box was just ticked or cleared.
            self.open_import(source, !recursive);
        } else if cancel || modal.should_close() {
            self.import = None;
        }
    }

    /// Where an import from `source` copies to: a session's Capture folder (unless the files
    /// already are in the session), or the catalog's choice.
    pub fn import_copy_to(&self, source: &Path) -> Option<CopyTo> {
        match &self.session {
            Some(s) => (!source.starts_with(&s.root)).then(|| CopyTo {
                folder: s.folder(sk_core::session::SessionFolder::Capture),
                by_date: false,
                rename: self.import_options.template(),
                session: s.name.clone(),
            }),
            None => self.import_options.copy_to(),
        }
    }
}

/// "Dateinamen": renaming the copies after a template, with a preview of the first ones. Only
/// copies are renamed: the originals stay as they are.
fn names_card(ui: &mut egui::Ui, dialog: &mut ImportDialog, options: &mut ImportOptions, copies: bool, session: &str) {
    dialog::card(ui, "Dateinamen", None, |ui| {
        ui.add_enabled_ui(copies, |ui| {
            crate::widgets::checkbox(ui, &mut options.rename, "Beim Kopieren umbenennen").on_disabled_hover_text("Nur Kopien werden umbenannt, die Originale bleiben, wie sie sind");
            ui.add_enabled_ui(options.rename, |ui| {
                ui.add(egui::TextEdit::singleline(&mut options.template).desired_width(340.0));
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (token, help) in naming::TOKENS.iter().filter(|(t, _)| *t != "{Variante}") {
                        if ui.small_button(*token).on_hover_text(*help).clicked() {
                            options.template.push_str(token);
                        }
                    }
                });
                if options.rename {
                    let chosen: Vec<PathBuf> = dialog.items.iter().filter(|i| i.chosen).take(PREVIEW).map(|i| i.path.clone()).collect();
                    for (n, path) in chosen.iter().enumerate() {
                        let info = dialog.infos.entry(path.clone()).or_insert_with(|| raw::read_meta(path).ok().map(|m| m.info));
                        let fields = naming::NameFields { image: path, capture: info.as_ref(), counter: n as u32 + 1, session };
                        let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
                        let old = path.file_name().unwrap_or_default().to_string_lossy();
                        ui.label(RichText::new(format!("{old}  {}  {}{ext}", icon::ARROW_RIGHT, naming::expand(&options.template, &fields))).monospace().small());
                    }
                }
            });
        });
        if !copies {
            dialog::note(ui, "Nur beim Kopieren: Die Originale bleiben, wie sie sind.");
        }
    });
}

/// The previews, as many per row as fit; only the rows in view are drawn.
fn grid(ui: &mut egui::Ui, dialog: &mut ImportDialog, badge: &str, height: f32) {
    if dialog.items.is_empty() {
        dialog::well(ui, |ui| {
            ui.set_min_height(height);
            ui.centered_and_justified(|ui| dialog::note(ui, "Keine RAW-Dateien in diesem Ordner"));
        });
        return;
    }
    let width = ui.available_width();
    let cols = (((width + GAP) / (TILE + GAP)).floor() as usize).max(1);
    let rows = dialog.items.len().div_ceil(cols);
    let mut clicked = None;
    egui::Frame::new().fill(theme::BROWSER).corner_radius(8).inner_margin(8).show(ui, |ui| {
        egui::ScrollArea::vertical().max_height(height).min_scrolled_height(height).auto_shrink([false, false]).show_rows(ui, TILE + CAPTION + GAP, rows, |ui, range| {
            dialog.loader.previews.show_from(range.start * cols);
            for row in range {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    for i in row * cols..((row + 1) * cols).min(dialog.items.len()) {
                        if tile(ui, &dialog.items[i], badge).clicked() {
                            clicked = Some((i, ui.input(|i| i.modifiers.shift)));
                        }
                    }
                });
                ui.add_space(GAP - ui.spacing().item_spacing.y);
            }
        });
    });
    if let Some((i, shift)) = clicked {
        dialog.click(i, shift);
    }
}

fn tile(ui: &mut egui::Ui, item: &Item, badge: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(TILE, TILE + CAPTION), Sense::click());
    let painter = ui.painter_at(rect);
    let frame = Rect::from_min_size(rect.min, vec2(TILE, TILE));
    let known = item.known == Some(true);
    painter.rect_filled(frame, 6, Color32::from_rgb(38, 38, 42));
    match &item.thumb {
        Some(t) => {
            let size = t.size_vec2();
            let scale = ((TILE - 10.0) / size.x).min((TILE - 10.0) / size.y);
            let tint = if item.chosen { Color32::WHITE } else { Color32::from_gray(90) };
            painter.image(t.id(), Rect::from_center_size(frame.center(), size * scale), Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), tint);
        }
        None => {
            painter.text(frame.center(), egui::Align2::CENTER_CENTER, icon::IMAGE, egui::FontId::proportional(26.0), theme::TEXT_FAINT);
        }
    }
    let border = if item.chosen {
        Stroke::new(2.0, theme::ACCENT)
    } else if resp.hovered() {
        Stroke::new(1.0, theme::TEXT_FAINT)
    } else {
        Stroke::new(1.0, theme::BORDER)
    };
    painter.rect_stroke(frame.shrink(1.0), 6, border, egui::StrokeKind::Inside);

    // The check box in the corner.
    let check = Rect::from_min_size(frame.min + vec2(7.0, 7.0), vec2(18.0, 18.0));
    if item.chosen {
        painter.rect_filled(check, 4, theme::ACCENT);
        painter.text(check.center(), egui::Align2::CENTER_CENTER, icon::CHECK, egui::FontId::proportional(13.0), Color32::WHITE);
    } else {
        painter.rect_filled(check, 4, Color32::from_black_alpha(140));
        painter.rect_stroke(check, 4, Stroke::new(1.5, Color32::from_gray(200)), egui::StrokeKind::Inside);
    }

    if known {
        let galley = painter.layout_no_wrap(badge.into(), egui::FontId::proportional(11.0), theme::AI);
        let pill = Rect::from_min_size(egui::pos2(frame.left() + 7.0, frame.bottom() - 7.0 - 17.0), galley.size() + vec2(12.0, 4.0));
        painter.rect_filled(pill, 8, Color32::from_black_alpha(190));
        painter.galley(pill.min + vec2(6.0, 2.0), galley, theme::AI);
    }

    let caption = Rect::from_min_max(egui::pos2(rect.left(), frame.bottom()), rect.max);
    let color = if item.chosen { theme::TEXT } else { theme::TEXT_FAINT };
    let mut job = egui::text::LayoutJob::simple_singleline(item.name.clone(), egui::FontId::proportional(11.0), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(TILE);
    let galley = painter.layout_job(job);
    painter.galley(egui::pos2(caption.center().x - galley.size().x / 2.0, caption.top() + 3.0), galley, color);

    let hover = if known { format!("{}\n{badge}", item.path.display()) } else { item.path.display().to_string() };
    resp.on_hover_text(hover)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Previews from the view on first, then those before it; none far from it.
    #[test]
    fn previews_near_the_view_first() {
        let p = Previews::new(2000, 1000);
        assert_eq!(p.next(), Some(1000));
        assert_eq!(p.next(), Some(1001));
        let order: Vec<usize> = std::iter::from_fn(|| p.next()).collect();
        assert_eq!(order.len(), 2 * NEAR - 2);
        assert_eq!(order[NEAR - 2], 999);
        assert!(order.iter().all(|i| i.abs_diff(1000) <= NEAR));
        // Dropped and scrolled back to: read again.
        p.forget(1001);
        assert_eq!(p.next(), Some(1001));
    }

    /// A reader with nothing left waits, and wakes for scrolling and for closing.
    #[test]
    fn preview_readers_wait_for_scrolling() {
        let p = Arc::new(Previews::new(3 * NEAR, 0));
        let stop = Arc::new(AtomicBool::new(false));
        while p.next().is_some_and(|i| i < NEAR - 1) {}
        let reader = {
            let (p, stop) = (p.clone(), stop.clone());
            std::thread::spawn(move || std::iter::from_fn(|| p.wait_next(&stop)).collect::<Vec<_>>())
        };
        std::thread::sleep(std::time::Duration::from_millis(50));
        p.show_from(2 * NEAR);
        while !p.taken.lock().unwrap()[3 * NEAR - 1] {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        {
            let _taken = p.taken.lock().unwrap();
            stop.store(true, Ordering::Relaxed);
            p.more.notify_all();
        }
        let read = reader.join().unwrap();
        assert!(read.contains(&(2 * NEAR)) && read.contains(&(3 * NEAR - 1)));
    }

    #[test]
    fn date_folders() {
        assert_eq!(date_folder(Some("2026:10:18 14:03:00")), Path::new("2026").join("10-18"));
        assert_eq!(date_folder(Some("2026-10-18T14:03:00")), Path::new("2026").join("10-18"));
        assert_eq!(date_folder(Some("0000:00:00 00:00:00")), Path::new("Ohne Datum"));
        assert_eq!(date_folder(Some("kaputt")), Path::new("Ohne Datum"));
        assert_eq!(date_folder(None), Path::new("Ohne Datum"));
    }
}
