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

    /// Shows step `j` of the timeline in the image while it is hovered.
    pub(crate) fn preview_history_step(&mut self, j: usize) {
        if let Some(adj) = self.history_steps().and_then(|(steps, _)| steps.get(j).map(|a| (*a).clone())) {
            self.preview_style(adj);
        }
    }

    /// Goes to step `target` of the timeline (undo or redo as often as needed), saving once.
    pub fn jump_history(&mut self, target: usize) {
        self.commit_and_save();
        let active = self.active_layer_id();
        let mut moved = false;
        while self.undo.len() > target && self.move_image_step(true) {
            moved = true;
        }
        while self.undo.len() < target && self.move_image_step(false) {
            moved = true;
        }
        if moved {
            self.image_stepped(active);
            self.commit_and_save();
        }
    }

    /// Removes snapshot `i` of the open image.
    pub fn delete_snapshot(&mut self, i: usize) {
        if let Some(cur) = self.current.as_mut().filter(|c| i < c.snapshots.len()) {
            cur.snapshots.remove(i);
            self.save_snapshots();
        }
    }

    /// Removes snapshot `i` named `name` of image `path`, only if that image is still open and
    /// the snapshot still there (another image may have opened while the question was asked).
    pub(crate) fn delete_snapshot_of(&mut self, path: &std::path::Path, i: usize, name: &str) {
        let still = self.current.as_ref().is_some_and(|c| c.meta.path == path && c.snapshots.get(i).is_some_and(|s| s.name == name));
        if still {
            self.delete_snapshot(i);
        }
    }

    /// Asks before deleting snapshot `i` (it can't be restored).
    pub fn request_snapshot_deletion(&mut self, i: usize) {
        if let Some((path, name)) = self.current.as_ref().and_then(|c| Some((c.meta.path.clone(), c.snapshots.get(i)?.name.clone()))) {
            self.request_deletion(crate::deletion::Deletion::Snapshot(path, i, name));
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
    /// renames it. In the side column it starts closed, with their number in the header.
    pub(crate) fn snapshot_section(&mut self, ui: &mut Ui, side: bool) {
        let mut action = None;
        let mut typed = None;
        // Hovered: the image shows it until the mouse leaves (see `App::preview_style`).
        let mut hovered = None;
        let editing = self.snapshot_rename.clone();
        // As the styles: small previews of the image, or (with „Liste“ there) a list.
        let grid = !self.style_list;
        if grid {
            self.pump_snapshot_thumbs();
        }
        let Some(cur) = &self.current else { return };
        let app = &*self;
        let help = "Ein Schnappschuss sichert den jetzigen Stand; ein Klick darauf stellt ihn wieder her.";
        let body = |ui: &mut Ui| {
            if ui.button(format!("{}  Schnappschuss", icon::PLUS)).on_hover_text("Den jetzigen Stand unter einem Namen sichern").clicked() {
                action = Some(SnapshotAction::Add);
            }
            if cur.snapshots.is_empty() {
                crate::widgets::hint(ui, "Noch keine Schnappschüsse.");
            }
            if grid {
                ui.add_space(4.0);
                // The name being edited above the previews.
                if let Some((i, text)) = &editing {
                    let mut text = text.clone();
                    let r = ui.text_edit_singleline(&mut text);
                    r.request_focus();
                    if r.lost_focus() {
                        let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                        action = Some(SnapshotAction::Rename(*i, if esc { String::new() } else { text }));
                    } else if r.changed() {
                        typed = Some((*i, text));
                    }
                }
                let (cols, tile, image_h) = crate::style_grid::tile_size(ui.available_width());
                let shots: Vec<(usize, &sk_core::sidecar::Snapshot)> = cur.snapshots.iter().enumerate().collect();
                for row in shots.chunks(cols) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        for (i, s) in row {
                            let current = s.adjustments == cur.adjustments;
                            let r = crate::style_grid::thumb_tile(ui, app.snapshot_texture(*i, &s.adjustments), &s.name, tile, image_h, current)
                                .on_hover_text("Klick: anwenden · Doppelklick: umbenennen");
                            if r.hovered() && !current {
                                hovered = Some(*i);
                            }
                            if r.double_clicked() {
                                action = Some(SnapshotAction::StartRename(*i));
                            } else if r.clicked() {
                                action = Some(SnapshotAction::Apply(*i));
                            }
                            crate::widgets::context_menu(&r, |ui| {
                                if ui.button("Anwenden").clicked() {
                                    action = Some(SnapshotAction::Apply(*i));
                                }
                                if ui.button("Umbenennen").clicked() {
                                    action = Some(SnapshotAction::StartRename(*i));
                                }
                                if ui.button(format!("{}  Löschen …", icon::TRASH)).clicked() {
                                    action = Some(SnapshotAction::Delete(*i));
                                }
                            });
                        }
                    });
                    ui.add_space(6.0);
                }
                return;
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
                            if r.hovered() && !current {
                                hovered = Some(i);
                            }
                            if r.double_clicked() {
                                action = Some(SnapshotAction::StartRename(i));
                            } else if r.clicked() {
                                action = Some(SnapshotAction::Apply(i));
                            }
                            crate::widgets::context_menu(&r, |ui| {
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
        };
        if side {
            crate::widgets::section_counted(ui, "Schnappschüsse", Some(help), cur.snapshots.len(), body);
        } else {
            section_help(ui, "Schnappschüsse", help, None, body);
        }

        if typed.is_some() {
            self.snapshot_rename = typed;
        }
        if let Some(adj) = hovered.filter(|_| action.is_none()).and_then(|i| self.current.as_ref()?.snapshots.get(i)).map(|s| s.adjustments.clone()) {
            self.preview_style(adj);
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
        self.snapshot_section(ui, false);
        let (mut jump, mut hovered) = (None, None);
        if let Some((steps, now)) = self.history_steps() {
            let help = "Klick springt zu diesem Stand; neue Änderungen verwerfen die Schritte darüber. Überfahren zeigt ihn im Bild.";
            section_help(ui, "Verlauf", help, None, |ui| {
                for j in (0..steps.len()).rev() {
                    let label = if j == 0 { "Geöffnet".to_string() } else { describe(steps[j - 1], steps[j]) };
                    let text = if j > now { RichText::new(label).color(Color32::from_gray(120)).italics() } else { RichText::new(label) };
                    let r = ui.selectable_label(j == now, text);
                    if r.clicked() && j != now {
                        jump = Some(j);
                    } else if r.hovered() && j != now {
                        hovered = Some(j);
                    }
                }
            });
        }
        if let Some(j) = jump {
            self.jump_history(j);
        } else if let Some(j) = hovered {
            self.preview_history_step(j);
        }
    }
}
