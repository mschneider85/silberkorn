//! Variants of an image (see `sk_core::variant`): creating, cloning and deleting them.

use std::path::PathBuf;

use egui::{Button, Ui};
use sk_core::catalog::Scope;
use sk_core::sidecar::{self, Sidecar};
use sk_core::{cache, variant};

use crate::app::App;
use crate::dialog::{self, Dialog, Tone};
use crate::session::PendingAction;
use crate::theme::icon;

impl App {
    /// Adds a variant of the current image and shows it: with default settings, or (`clone`)
    /// with those of the current variant.
    pub fn new_variant(&mut self, clone: bool) {
        let Some(path) = self.current_path() else { return };
        self.commit_and_save();
        let Some(cur) = &self.current else { return };
        let settings = if clone {
            Sidecar {
                rating: cur.rating,
                color_label: cur.color_label,
                keywords: cur.keywords.clone(),
                adjustments: cur.committed.clone(),
                ..Default::default()
            }
        } else {
            Sidecar { keywords: cur.keywords.clone(), ..Default::default() }
        };
        let new = match sidecar::add_variant(&path, &settings) {
            Ok(p) => p,
            Err(e) => {
                self.status = format!("Variante nicht angelegt: {e:#}");
                return;
            }
        };
        if let Some(c) = &self.catalog {
            // Same camera data as the file; no need to read it again.
            let mut meta = (*cur.meta).clone();
            meta.path = new.clone();
            let _ = c.upsert(&meta, &settings, 0);
            if let Scope::Album(id) = self.scope {
                let _ = c.add_to_album(id, std::slice::from_ref(&new));
            }
        }
        self.reload_entries();
        self.select_path(&new, egui::Modifiers::NONE);
        self.scroll_to_primary = true;
        self.status = format!("Neue Variante: {}", variant::display_name(&new));
    }

    /// Asks before deleting the selected further variants (the file's own first variant can't
    /// be deleted; it goes with the file).
    pub fn request_delete_variant(&mut self) {
        let selected = self.selected_paths();
        let variants: Vec<PathBuf> = selected.iter().filter(|p| variant::number(p) > 1).cloned().collect();
        if variants.is_empty() {
            if !selected.is_empty() {
                self.status = "Die erste Variante gehört zur Datei selbst und kann nicht gelöscht werden".into();
            }
            return;
        }
        self.confirm_delete_variant = Some(variants);
    }

    /// Deletes further variants with their settings; for a shown one, its file is shown instead.
    pub fn delete_variants(&mut self, paths: &[PathBuf]) {
        self.confirm_delete_variant = None;
        let shown = self.primary_path().filter(|p| paths.contains(p));
        if shown.is_some() {
            self.close_current();
        } else {
            self.commit_and_save();
        }
        let mut deleted = 0;
        for path in paths {
            if let Err(e) = sidecar::remove_variant(path) {
                self.status = format!("Variante {} nicht gelöscht: {e:#}", variant::display_name(path));
                continue;
            }
            deleted += 1;
            cache::remove(path, self.session.as_ref().map(|s| s.root.as_path()));
            if let Some(c) = &self.catalog {
                let _ = c.remove_path(path);
            }
            self.thumbs.remove(path);
            self.thumb_requested.remove(path);
            self.selection.remove(path);
            self.compare_textures.remove(path);
            self.compare_requested.remove(path);
            self.compare_details.remove(path);
            self.focus_maps.remove(path);
            self.forget_prepared(path);
            if self.compare_ref.as_ref() == Some(path) {
                self.compare_ref = None;
            }
        }
        self.reload_entries();
        if let Some(p) = shown {
            self.select_path(&variant::file_of(&p), egui::Modifiers::NONE);
        }
        if deleted == paths.len() {
            self.status = match paths {
                [p] => format!("Variante {} gelöscht", variant::display_name(p)),
                _ => format!("{deleted} Varianten gelöscht"),
            };
        }
    }

    /// Variant entries of the image menu and the thumbnail context menu (they act on the
    /// image in the viewer).
    pub fn variant_menu(&mut self, ui: &mut Ui) {
        let current = self.current_path();
        let has = current.is_some();
        if ui.add_enabled(has, Button::new(format!("{}  Neue Variante", icon::COPY_SIMPLE)).shortcut_text(crate::commands::shortcut_of("new-variant"))).clicked() {
            self.pending = Some(PendingAction::NewVariant { clone: false });
        }
        if ui.add_enabled(has, Button::new(format!("{}  Variante klonen", icon::COPY)).shortcut_text(crate::commands::shortcut_of("clone-variant"))).clicked() {
            self.pending = Some(PendingAction::NewVariant { clone: true });
        }
        let deletable = current.as_ref().is_some_and(|p| variant::number(p) > 1);
        let delete = ui
            .add_enabled(deletable, Button::new(format!("{}  Variante löschen …", icon::X_SQUARE)))
            .on_disabled_hover_text("Nur weitere Varianten lassen sich löschen; die erste gehört zur Datei.");
        if delete.clicked() {
            self.pending = Some(PendingAction::DeleteVariant);
        }
    }

    pub(crate) fn variant_dialogs(&mut self, ctx: &egui::Context) {
        let Some(paths) = self.confirm_delete_variant.clone() else { return };
        let (mut confirmed, mut cancel) = (false, false);
        let title = if paths.len() == 1 { "Variante löschen".to_string() } else { format!("{} Varianten löschen", paths.len()) };
        let modal = Dialog::new("delete_variant", icon::X_SQUARE, title).subtitle("Die RAW-Dateien bleiben erhalten").tone(Tone::Danger).width(420.0).show(ctx, |ui| {
            let what = match paths.as_slice() {
                [p] => format!("„{}“ wird", variant::display_name(p)),
                _ => format!("{} Varianten werden", paths.len()),
            };
            ui.label(format!("{what} mit allen Einstellungen gelöscht. Die RAW-Dateien und ihre anderen Varianten bleiben erhalten."));
            dialog::footer(ui, |ui| {
                if ui.add(dialog::danger("Löschen")).clicked() {
                    confirmed = true;
                }
                if ui.add(dialog::secondary("Abbrechen")).clicked() {
                    cancel = true;
                }
            });
        });
        if confirmed {
            self.delete_variants(&paths);
        }
        if confirmed || cancel || modal.should_close() {
            self.confirm_delete_variant = None;
        }
    }
}
