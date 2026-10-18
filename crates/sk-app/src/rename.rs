//! Batch renaming of the selected files with a name template.

use std::path::PathBuf;

use egui::{Modifiers, RichText, vec2};
use sk_core::naming::{self, NameFields};
use sk_core::{raw, session, variant};

use crate::app::App;
use crate::dialog::{self, Dialog};
use crate::theme::icon;

/// The open rename dialog.
pub struct RenameDialog {
    pub template: String,
    pub start: u32,
    /// The files in display order (variants are renamed with their file).
    files: Vec<PathBuf>,
    /// Capture data of the first files, for the preview.
    preview: Vec<(PathBuf, Option<raw::CaptureInfo>)>,
}

/// Files shown in the dialog's preview.
const PREVIEW: usize = 5;

impl App {
    /// "Umbenennen …" for the selection.
    pub fn open_rename(&mut self) {
        let selected = self.selected_paths();
        let order: Vec<PathBuf> = self.visible.iter().map(|&i| self.entries[i].path.clone()).collect();
        let mut files: Vec<PathBuf> = variant::files_of(&selected);
        files.sort_by_key(|f| order.iter().position(|p| p == f).unwrap_or(usize::MAX));
        if files.is_empty() {
            return;
        }
        let preview = files.iter().take(PREVIEW).map(|f| (f.clone(), raw::read_meta(f).ok().map(|m| m.info))).collect();
        let template = if self.rename_template.is_empty() { "{Name}".into() } else { self.rename_template.clone() };
        self.rename = Some(RenameDialog { template, start: 1, files, preview });
    }

    fn session_name(&self) -> String {
        self.session.as_ref().map(|s| s.name.clone()).unwrap_or_default()
    }

    /// Renames `files` (in this order) after `template`; returns (renamed, first error).
    pub fn rename_files(&mut self, files: &[PathBuf], template: &str, start: u32) -> (usize, Option<String>) {
        let session = self.session_name();
        let primary = self.primary_path();
        if primary.as_ref().is_some_and(|p| files.contains(&variant::file_of(p))) {
            self.close_current();
        } else {
            self.commit_and_save();
        }
        let (mut renamed, mut error, mut new_primary) = (0, None, None);
        for (i, file) in files.iter().enumerate() {
            let info = raw::read_meta(file).ok().map(|m| m.info);
            let fields = NameFields { image: file, capture: info.as_ref(), counter: start + i as u32, session: &session };
            let stem = naming::expand(template, &fields);
            match session::rename_image(file, &stem) {
                Ok(new) if new == *file => {}
                Ok(new) => {
                    renamed += 1;
                    self.file_relocated(file, &new, true);
                    if let Some(p) = primary.as_ref().filter(|p| variant::file_of(p) == *file) {
                        new_primary = Some(variant::path_for(&new, variant::number(p)));
                    }
                }
                Err(e) => {
                    error.get_or_insert(format!("{}: {e:#}", file.display()));
                }
            }
        }
        self.reload_entries();
        if let Some(p) = new_primary.or(primary).filter(|p| self.index_of(p).is_some()) {
            self.select_path(&p, Modifiers::NONE);
        }
        (renamed, error)
    }

    pub(crate) fn rename_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.rename else { return };
        let session = self.session.as_ref().map(|s| s.name.clone()).unwrap_or_default();
        let (mut apply, mut cancel) = (false, false);
        let n = dialog.files.len();
        let title = if n == 1 { "Datei umbenennen".to_string() } else { format!("{n} Dateien umbenennen") };
        let modal = Dialog::new("rename", icon::PENCIL_SIMPLE, title).subtitle("Varianten und Einstellungen wandern mit").width(500.0).show(ctx, |ui| {
            dialog::card(ui, "Neuer Name", None, |ui| {
                egui::Grid::new("rename_fields").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(&mut dialog.template).desired_width(340.0));
                    ui.end_row();
                    ui.label("Zähler ab");
                    ui.add(egui::DragValue::new(&mut dialog.start).range(0..=999_999));
                    ui.end_row();
                });
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (token, help) in naming::TOKENS {
                        if ui.small_button(token).on_hover_text(help).clicked() {
                            dialog.template.push_str(token);
                        }
                    }
                });
            });
            dialog::card(ui, "Vorschau", None, |ui| {
                for (i, (file, info)) in dialog.preview.iter().enumerate() {
                    let fields = NameFields { image: file, capture: info.as_ref(), counter: dialog.start + i as u32, session: &session };
                    let ext = file.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
                    let old = file.file_name().unwrap_or_default().to_string_lossy();
                    ui.label(RichText::new(format!("{old}  {}  {}{ext}", icon::ARROW_RIGHT, naming::expand(&dialog.template, &fields))).monospace().small());
                }
                if n > PREVIEW {
                    dialog::note(ui, format!("… und {} weitere", n - PREVIEW));
                }
            });
            dialog::note(ui, "Ist ein Name schon vergeben, wird „-2“ usw. angehängt.");
            dialog::footer(ui, |ui| {
                if dialog::default_button(ui, true, dialog::primary("Umbenennen")) {
                    apply = true;
                }
                if ui.add(dialog::secondary("Abbrechen")).clicked() {
                    cancel = true;
                }
            });
        });
        if apply && let Some(d) = self.rename.take() {
            self.rename_template = d.template.clone();
            self.save_settings();
            let (n, error) = self.rename_files(&d.files, &d.template, d.start);
            self.status = match error {
                Some(e) => format!("{n} umbenannt, Fehler: {e}"),
                None if n == 1 => "1 Datei umbenannt".into(),
                None => format!("{n} Dateien umbenannt"),
            };
        } else if cancel || modal.should_close() {
            self.rename = None;
        }
    }
}
