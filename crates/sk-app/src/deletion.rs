//! Deleting what can't be undone (a style file, an album, a snapshot, an output recipe) asks
//! first, in one dialog for all of them.

use sk_core::catalog::Scope;

use crate::app::App;
use crate::dialog::{self, Dialog, Tone};
use crate::theme::icon;

/// Something to delete for good, with its name for the question.
#[derive(Clone, Debug)]
pub enum Deletion {
    Style(String),
    Album(i64, String),
    /// Image, index and name: the image may have changed until the question is answered.
    Snapshot(std::path::PathBuf, usize, String),
    Recipe(usize, String),
}

impl Deletion {
    fn title(&self) -> &'static str {
        match self {
            Deletion::Style(_) => "Stil löschen",
            Deletion::Album(..) => "Album löschen",
            Deletion::Snapshot(..) => "Schnappschuss löschen",
            Deletion::Recipe(..) => "Ausgaberezept löschen",
        }
    }

    fn text(&self) -> String {
        match self {
            Deletion::Style(name) => format!("Der Stil „{name}“ wird gelöscht. Bilder, auf die er angewendet wurde, bleiben unverändert."),
            Deletion::Album(_, name) => format!("Das Album „{name}“ wird gelöscht. Die Bilder darin bleiben erhalten."),
            Deletion::Snapshot(_, _, name) => format!("Der Schnappschuss „{name}“ wird gelöscht. Die Einstellungen des Bildes bleiben, wie sie sind."),
            Deletion::Recipe(_, name) => format!("Das Ausgaberezept „{name}“ wird gelöscht."),
        }
    }
}

impl App {
    /// Asks before deleting `what` (see [`App::deletion_dialog`]).
    pub fn request_deletion(&mut self, what: Deletion) {
        self.pending_deletion = Some(what);
    }

    pub(crate) fn deletion_dialog(&mut self, ctx: &egui::Context) {
        let Some(what) = self.pending_deletion.clone() else { return };
        let (mut confirmed, mut cancel) = (false, false);
        let modal = Dialog::new("delete_confirm", icon::TRASH, what.title()).subtitle("Lässt sich nicht rückgängig machen").tone(Tone::Danger).width(420.0).show(ctx, |ui| {
            ui.label(what.text());
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
            self.delete_now(what);
        }
        if confirmed || cancel || modal.should_close() {
            self.pending_deletion = None;
        }
    }

    fn delete_now(&mut self, what: Deletion) {
        match what {
            Deletion::Style(name) => {
                if let Err(e) = sk_core::styles::delete_in(&sk_core::styles::styles_dir(), &name) {
                    self.report_error(format!("Stil „{name}“ konnte nicht gelöscht werden: {e:#}"));
                }
                self.reload_styles();
            }
            Deletion::Album(id, name) => {
                if let Some(c) = &self.catalog
                    && let Err(e) = c.delete_album(id)
                {
                    self.report_error(format!("Album „{name}“ konnte nicht gelöscht werden: {e:#}"));
                }
                if self.scope == Scope::Album(id) {
                    self.open_scope(Scope::All);
                } else {
                    self.refresh_catalog_lists();
                }
            }
            Deletion::Snapshot(path, i, name) => self.delete_snapshot_of(&path, i, &name),
            Deletion::Recipe(i, _) => {
                if self.recipes.len() > 1 && i < self.recipes.len() {
                    self.recipes.remove(i);
                    self.selected_recipe = self.selected_recipe.min(self.recipes.len() - 1);
                    self.save_settings();
                }
            }
        }
    }
}
