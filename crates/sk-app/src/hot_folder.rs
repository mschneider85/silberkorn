//! Watching the open folder for new pictures (e.g. the capture folder of the camera software):
//! they appear in the browser by themselves, and the newest is opened if wanted. A file counts as
//! complete once its size stays the same from one look to the next; the browser is updated (which
//! lists the whole folder) only while no file is growing.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use sk_core::catalog::Scope;

use crate::app::App;
use crate::workers::IndexJob;

/// How often the folder is looked at.
const INTERVAL: Duration = Duration::from_secs(1);

pub struct HotFolder {
    pub on: bool,
    /// Open each new picture as it arrives.
    pub open_new: bool,
    /// The folder looked at last, and its modification time then.
    folder: Option<PathBuf>,
    modified: Option<SystemTime>,
    /// New files that may still be being written, with their size at the last look.
    pending: HashMap<PathBuf, u64>,
    /// Complete new files, waiting until no other file is growing.
    ready: Vec<PathBuf>,
    next: Option<Instant>,
}

impl HotFolder {
    pub fn new(on: bool, open_new: bool) -> Self {
        Self { on, open_new, folder: None, modified: None, pending: HashMap::new(), ready: Vec::new(), next: None }
    }
}

impl App {
    /// The folder that is watched while watching is on: the one the browser shows (without its
    /// subfolders).
    pub(crate) fn watched_folder(&self) -> Option<PathBuf> {
        match &self.scope {
            Scope::Folder { path, recursive: false } => Some(path.clone()),
            _ => None,
        }
    }

    /// Each frame: looks for new pictures once a second while watching.
    pub(crate) fn pump_hot_folder(&mut self, ctx: &egui::Context) {
        if !self.hot_folder.on {
            return;
        }
        let Some(folder) = self.watched_folder() else { return };
        ctx.request_repaint_after(INTERVAL);
        let h = &mut self.hot_folder;
        if h.next.is_some_and(|t| Instant::now() < t) {
            return;
        }
        h.next = Some(Instant::now() + INTERVAL);
        if h.folder.as_ref() != Some(&folder) {
            h.folder = Some(folder.clone());
            h.modified = None;
            h.pending.clear();
            h.ready.clear();
        }
        // Waiting files are looked at again; the folder is only read when something changed in it.
        let mut growing = false;
        for (path, size) in std::mem::take(&mut h.pending) {
            match std::fs::metadata(&path).map(|m| m.len()) {
                Ok(now) if now == size && now > 0 => h.ready.push(path),
                Ok(now) => {
                    growing |= now != size;
                    h.pending.insert(path, now);
                }
                // Gone again (a temporary file renamed by the camera software).
                Err(_) => {}
            }
        }
        let modified = std::fs::metadata(&folder).and_then(|m| m.modified()).ok();
        if modified.is_none() || modified != h.modified {
            h.modified = modified;
            let listed: HashSet<&PathBuf> = self.entries.iter().map(|e| &e.path).collect();
            for path in sk_core::catalog::scan_folder(&folder, false) {
                if !listed.contains(&path) && !h.ready.contains(&path) && !h.pending.contains_key(&path) {
                    let size = std::fs::metadata(&path).map_or(0, |m| m.len());
                    growing |= size > 0;
                    h.pending.insert(path, size);
                }
            }
        }
        if h.ready.is_empty() || growing {
            return;
        }
        let mut ready = std::mem::take(&mut h.ready);
        ready.sort();
        let n = ready.len();
        self.status = if n == 1 { "1 neue Aufnahme".into() } else { format!("{n} neue Aufnahmen") };
        let newest = ready.iter().max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok()).cloned();
        self.workers.index(IndexJob { catalog: self.catalog_path(), paths: ready, import: false, copy_to: None });
        self.reload_entries();
        if self.hot_folder.open_new {
            self.select_when_listed = newest;
            self.select_created();
        }
    }

    /// The switches in the library tab (and the file menu).
    pub(crate) fn hot_folder_ui(&mut self, ui: &mut egui::Ui) {
        let (mut on, mut open_new) = (self.hot_folder.on, self.hot_folder.open_new);
        ui.checkbox(&mut on, "Ordner überwachen")
            .on_hover_text("Neue Bilder, etwa aus der Kamerasoftware, erscheinen von selbst in der Übersicht");
        ui.add_enabled_ui(on, |ui| {
            ui.indent("hot_folder", |ui| ui.checkbox(&mut open_new, "Neue Aufnahme gleich öffnen"));
        });
        if on && self.watched_folder().is_none() {
            ui.label(egui::RichText::new("Wirkt, sobald ein Ordner (ohne Unterordner) angezeigt wird").small().color(crate::theme::TEXT_FAINT));
        }
        if (on, open_new) != (self.hot_folder.on, self.hot_folder.open_new) {
            self.hot_folder.on = on;
            self.hot_folder.open_new = open_new;
            self.hot_folder.modified = None;
            self.save_settings();
        }
    }
}
