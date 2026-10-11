//! History tab: the edit steps of the current image to jump back and forth (the undo and redo
//! stacks), and named snapshots kept in the sidecar.

use egui::{Color32, RichText, Ui};
use sk_core::history::describe;
use sk_core::sidecar::{self, Snapshot};

use crate::app::App;
use crate::theme::icon;
use crate::widgets::section;

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
            self.undo();
            if self.undo.len() == before {
                break;
            }
        }
        while self.undo.len() < target && !self.redo.is_empty() {
            self.redo();
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

    fn save_snapshots(&mut self) {
        let Some(cur) = &self.current else { return };
        if let Err(e) = sidecar::set_snapshots(&cur.meta.path, cur.snapshots.clone()) {
            self.status = format!("Schnappschuss nicht gespeichert: {e:#}");
        }
    }

    pub fn history_tab(&mut self, ui: &mut Ui) {
        if self.current.is_none() {
            ui.label("Kein Bild ausgewählt.");
            return;
        }
        let mut action = None;
        let mut typed = None;
        let editing = self.snapshot_rename.clone();
        let Some(cur) = &self.current else { return };
        section(ui, "Schnappschüsse", None, |ui| {
            if ui.button(format!("{}  Schnappschuss", icon::PLUS)).on_hover_text("Den jetzigen Stand unter einem Namen sichern").clicked() {
                action = Some(SnapshotAction::Add);
            }
            if cur.snapshots.is_empty() {
                ui.label(RichText::new("Noch keine. Ein Schnappschuss sichert den jetzigen Stand; ein Klick darauf stellt ihn wieder her.").small().color(Color32::from_gray(140)));
            }
            for (i, s) in cur.snapshots.iter().enumerate() {
                ui.horizontal(|ui| {
                    match &editing {
                        Some((j, text)) if *j == i => {
                            let mut text = text.clone();
                            let r = ui.text_edit_singleline(&mut text);
                            r.request_focus();
                            if r.lost_focus() {
                                action = Some(SnapshotAction::Rename(i, text));
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
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button(icon::TRASH).on_hover_text("Schnappschuss löschen").clicked() {
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
        let mut jump = None;
        if let Some((steps, now)) = self.history_steps() {
            section(ui, "Verlauf", None, |ui| {
                ui.label(RichText::new("Klick springt zu diesem Stand; neue Änderungen verwerfen die Schritte darüber.").small().color(Color32::from_gray(140)));
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
        match action {
            Some(SnapshotAction::Add) => self.add_snapshot(None),
            Some(SnapshotAction::Apply(i)) => {
                if let Some(adj) = self.current.as_ref().and_then(|c| c.snapshots.get(i)).map(|s| s.adjustments.clone()) {
                    self.edit(|a| *a = adj);
                    self.commit_and_save();
                }
            }
            Some(SnapshotAction::Delete(i)) => {
                if let Some(cur) = self.current.as_mut().filter(|c| i < c.snapshots.len()) {
                    cur.snapshots.remove(i);
                    self.save_snapshots();
                }
            }
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
}
