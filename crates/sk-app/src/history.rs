//! History tab: the edit steps of the current image to jump back and forth (the undo and redo
//! stacks), and named snapshots kept in the sidecar.

use egui::{Color32, RichText, Ui};
use sk_core::history::describe;
use sk_core::sidecar::{self, Snapshot};

use crate::app::App;
use crate::theme::icon;
use crate::widgets::section_help;

enum SnapshotAction {
    Add,
    Apply(usize),
    Delete(usize),
    StartRename(usize),
    Rename(usize, String),
}

impl App {
    /// Steps of the timeline, oldest first, and the index of the current one.
    pub(crate) fn history_steps(&self) -> Option<(Vec<&sk_core::params::Adjustments>, usize)> {
        let cur = self.current.as_ref()?;
        let steps: Vec<_> = self.undo.iter().chain(std::iter::once(&cur.committed)).chain(self.redo.iter().rev()).collect();
        Some((steps, self.undo.len()))
    }

    /// Goes to step `target` of the timeline (undo or redo as often as needed).
    pub fn jump_history(&mut self, target: usize) {
        self.commit_and_save();
        while self.undo.len() > target {
            let before = self.undo.len();
            self.undo_image();
            if self.undo.len() == before {
                break;
            }
        }
        while self.undo.len() < target && !self.redo.is_empty() {
            self.redo_image();
        }
    }

    /// Removes snapshot `i` of the open image.
    pub fn delete_snapshot(&mut self, i: usize) {
        if let Some(cur) = self.current.as_mut().filter(|c| i < c.snapshots.len()) {
            cur.snapshots.remove(i);
            self.save_snapshots();
        }
    }

    /// Asks before deleting snapshot `i` (it can't be restored).
    pub fn request_snapshot_deletion(&mut self, i: usize) {
        if let Some(name) = self.current.as_ref().and_then(|c| c.snapshots.get(i)).map(|s| s.name.clone()) {
            self.request_deletion(crate::deletion::Deletion::Snapshot(i, name));
        }
    }

    /// Saves the current adjustments as a snapshot.
    pub fn add_snapshot(&mut self, name: Option<String>) {
        self.commit_and_save();
        let Some(cur) = self.current.as_mut() else { return };
        let name = name.unwrap_or_else(|| format!("Schnappschuss {}", cur.snapshots.len() + 1));
        cur.snapshots.push(Snapshot { name, adjustments: cur.committed.clone() });
        self.save_snapshots();
    }

    /// "+ Schnappschuss": saves the current state and offers its name for editing.
    pub(crate) fn add_snapshot_to_name(&mut self) {
        self.add_snapshot(None);
        self.snapshot_rename = self.current.as_ref().and_then(|c| Some((c.snapshots.len().checked_sub(1)?, c.snapshots.last()?.name.clone())));
    }

    fn save_snapshots(&mut self) {
        let Some(cur) = &self.current else { return };
        if let Err(e) = sidecar::set_snapshots(&cur.meta.path, cur.snapshots.clone()) {
            self.report_error(format!("Schnappschuss nicht gespeichert: {e:#}"));
        }
    }

    /// The snapshots of the open image, in the history tab and the side column alike: "+" saves
    /// the current state and lets it be named right away, a click applies one, a double click
    /// renames it.
    pub(crate) fn snapshot_section(&mut self, ui: &mut Ui) {
        let mut action = None;
        let mut typed = None;
        let editing = self.snapshot_rename.clone();
        let Some(cur) = &self.current else { return };
        let help = "Ein Schnappschuss sichert den jetzigen Stand; ein Klick darauf stellt ihn wieder her.";
        section_help(ui, "Schnappschüsse", help, None, |ui| {
            if ui.button(format!("{}  Schnappschuss", icon::PLUS)).on_hover_text("Den jetzigen Stand unter einem Namen sichern").clicked() {
                action = Some(SnapshotAction::Add);
            }
            if cur.snapshots.is_empty() {
                crate::widgets::hint(ui, "Noch keine Schnappschüsse.");
            }
            for (i, s) in cur.snapshots.iter().enumerate() {
                ui.horizontal(|ui| {
                    match &editing {
                        Some((j, text)) if *j == i => {
                            let mut text = text.clone();
                            let r = ui.text_edit_singleline(&mut text);
                            r.request_focus();
                            if r.lost_focus() {
                                // Esc keeps the old name (an empty one is ignored), as for layers.
                                let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                                action = Some(SnapshotAction::Rename(i, if esc { String::new() } else { text }));
                            } else if r.changed() {
                                typed = Some((i, text));
                            }
                        }
                        _ => {
                            let current = s.adjustments == cur.adjustments;
                            let r = ui.selectable_label(current, format!("{}  {}", icon::CAMERA, s.name)).on_hover_text("Klick: anwenden · Doppelklick: umbenennen");
                            if r.double_clicked() {
                                action = Some(SnapshotAction::StartRename(i));
                            } else if r.clicked() {
                                action = Some(SnapshotAction::Apply(i));
                            }
                            r.context_menu(|ui| {
                                if ui.button("Anwenden").clicked() {
                                    action = Some(SnapshotAction::Apply(i));
                                }
                                if ui.button("Umbenennen").clicked() {
                                    action = Some(SnapshotAction::StartRename(i));
                                }
                                if ui.button(format!("{}  Löschen …", icon::TRASH)).clicked() {
                                    action = Some(SnapshotAction::Delete(i));
                                }
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button(icon::TRASH).on_hover_text("Schnappschuss löschen …").clicked() {
                                    action = Some(SnapshotAction::Delete(i));
                                }
                            });
                        }
                    }
                });
            }
        });

        if typed.is_some() {
            self.snapshot_rename = typed;
        }
        match action {
            Some(SnapshotAction::Add) => self.add_snapshot_to_name(),
            Some(SnapshotAction::Apply(i)) => {
                if let Some(adj) = self.current.as_ref().and_then(|c| c.snapshots.get(i)).map(|s| s.adjustments.clone()) {
                    self.apply_snapshot(adj);
                }
            }
            Some(SnapshotAction::Delete(i)) => self.request_snapshot_deletion(i),
            Some(SnapshotAction::StartRename(i)) => {
                self.snapshot_rename = self.current.as_ref().and_then(|c| c.snapshots.get(i)).map(|s| (i, s.name.clone()));
            }
            Some(SnapshotAction::Rename(i, name)) => {
                self.snapshot_rename = None;
                let name = name.trim().to_string();
                if let Some(s) = self.current.as_mut().and_then(|c| c.snapshots.get_mut(i)).filter(|_| !name.is_empty()) {
                    s.name = name;
                    self.save_snapshots();
                }
            }
            None => {}
        }
    }

    pub fn history_tab(&mut self, ui: &mut Ui) {
        if self.current.is_none() {
            crate::widgets::no_image(ui);
            return;
        }
        self.snapshot_section(ui);
        let mut jump = None;
        if let Some((steps, now)) = self.history_steps() {
            let help = "Klick springt zu diesem Stand; neue Änderungen verwerfen die Schritte darüber.";
            section_help(ui, "Verlauf", help, None, |ui| {
                for j in (0..steps.len()).rev() {
                    let label = if j == 0 { "Geöffnet".to_string() } else { describe(steps[j - 1], steps[j]) };
                    let text = if j > now { RichText::new(label).color(Color32::from_gray(120)).italics() } else { RichText::new(label) };
                    if ui.selectable_label(j == now, text).clicked() && j != now {
                        jump = Some(j);
                    }
                }
            });
        }
        if let Some(j) = jump {
            self.jump_history(j);
        }
    }
}
