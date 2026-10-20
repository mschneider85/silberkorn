//! Stacks in the browser (see `sk_core::stack`): collapsed to their top image unless expanded,
//! created by hand or automatically from the capture times.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use egui::{Button, Ui};
use rayon::prelude::*;
use sk_core::catalog::ImageRecord;
use sk_core::sidecar::Stack;
use sk_core::{stack, variant};

use crate::app::App;
use crate::dialog::{self, Dialog};
use crate::commands::shortcut_of;
use crate::session::PendingAction;
use crate::theme::icon;

/// Commands of the stack menu (run after the browser has been drawn, see `PendingAction`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackAction {
    Create,
    Dissolve,
    RemoveFromStack,
    MakeTop,
    /// The sharpest image (by the culling analysis) of each selected stack on top.
    SharpestTop,
    Toggle,
    ExpandAll,
    CollapseAll,
    AutoDialog,
}

/// The "automatically stack" dialog.
pub struct AutoStackDialog {
    /// Largest gap between two images of a stack.
    pub seconds: f32,
}

/// Images taken at most this many seconds apart form a stack by default.
pub const AUTO_STACK_SECONDS: f32 = 2.0;

impl App {
    /// Visible entries (indices into `entries`) with stacks applied: the members of a stack
    /// follow each other, top image first; a collapsed stack shows only its top image (or the
    /// first member that passes the filter).
    pub(crate) fn arrange_stacks(&mut self, matching: Vec<usize>) -> Vec<usize> {
        self.stack_sizes.clear();
        for e in &self.entries {
            if let Some(s) = &e.stack {
                *self.stack_sizes.entry(s.id.clone()).or_default() += 1;
            }
        }
        let mut members: HashMap<&str, Vec<usize>> = HashMap::new();
        for &i in &matching {
            if let Some(s) = &self.entries[i].stack {
                members.entry(s.id.as_str()).or_default().push(i);
            }
        }
        let is_top = |i: usize| self.entries[i].stack.as_ref().is_some_and(|s| s.top) && variant::number(&self.entries[i].path) == 1;
        let mut placed: HashSet<&str> = HashSet::new();
        let mut out = Vec::with_capacity(matching.len());
        for &i in &matching {
            let Some(s) = &self.entries[i].stack else {
                out.push(i);
                continue;
            };
            if !placed.insert(s.id.as_str()) {
                continue;
            }
            let group = &members[s.id.as_str()];
            let top = group.iter().copied().find(|&j| is_top(j)).unwrap_or(group[0]);
            out.push(top);
            if self.expanded_stacks.contains(&s.id) {
                out.extend(group.iter().copied().filter(|&j| j != top));
            }
        }
        out
    }

    /// Size of the stack an entry belongs to (0 if none).
    pub fn stack_size(&self, idx: usize) -> usize {
        self.entries[idx].stack.as_ref().and_then(|s| self.stack_sizes.get(&s.id)).copied().unwrap_or(0)
    }

    /// Files (not variants) of the shown entries in these stacks, with their membership.
    fn stack_members(&self, ids: &HashSet<String>) -> Vec<(PathBuf, Stack)> {
        self.entries
            .iter()
            .filter(|e| variant::number(&e.path) == 1)
            .filter_map(|e| e.stack.clone().filter(|s| ids.contains(&s.id)).map(|s| (e.path.clone(), s)))
            .collect()
    }

    pub(crate) fn selected_stack_ids(&self) -> HashSet<String> {
        let selected: HashSet<PathBuf> = self.selected_paths().into_iter().collect();
        self.entries.iter().filter(|e| selected.contains(&e.path)).filter_map(|e| e.stack.as_ref().map(|s| s.id.clone())).collect()
    }

    /// Takes changed memberships into the catalog and the browser (variants follow their file).
    fn apply_stack_changes(&mut self, changes: Vec<(PathBuf, Option<Stack>)>) {
        let by_file: HashMap<PathBuf, Option<Stack>> = changes.into_iter().collect();
        for e in &mut self.entries {
            let (file, n) = variant::split(&e.path);
            if let Some(s) = by_file.get(&file) {
                let s = s.clone().map(|s| Stack { top: s.top && n == 1, ..s });
                if let Some(c) = &self.catalog {
                    let _ = c.set_stack(&e.path, s.as_ref());
                }
                e.stack = s;
            }
        }
        self.refilter();
    }

    pub fn run_stack_action(&mut self, action: StackAction) {
        let result = match action {
            StackAction::Create => self.stack_selection(),
            StackAction::Dissolve => {
                let ids = self.selected_stack_ids();
                let members = self.stack_members(&ids);
                let paths: Vec<PathBuf> = members.iter().map(|(p, _)| p.clone()).collect();
                let n = ids.len();
                stack::remove(&paths, &members).map(|c| {
                    self.apply_stack_changes(c);
                    self.expanded_stacks.retain(|id| !ids.contains(id));
                    if n > 0 { format!("{n} Stapel aufgelöst") } else { String::new() }
                })
            }
            StackAction::RemoveFromStack => {
                let members = self.stack_members(&self.selected_stack_ids());
                let paths = self.selected_paths();
                stack::remove(&paths, &members).map(|c| {
                    self.apply_stack_changes(c);
                    String::new()
                })
            }
            StackAction::MakeTop => match self.primary_path() {
                Some(top) => {
                    let ids: HashSet<String> =
                        self.index_of(&top).and_then(|i| self.entries[i].stack.as_ref()).map(|s| s.id.clone()).into_iter().collect();
                    let members = self.stack_members(&ids);
                    stack::set_top(&top, &members).map(|c| {
                        self.apply_stack_changes(c);
                        String::new()
                    })
                }
                None => Ok(String::new()),
            },
            StackAction::SharpestTop => {
                let ids = self.selected_stack_ids();
                let members = self.stack_members(&ids);
                let mut changes = Vec::new();
                let mut missing = 0;
                for id in &ids {
                    let best = members
                        .iter()
                        .filter(|(_, s)| s.id == *id)
                        .filter_map(|(p, _)| {
                            let c = self.index_of(p).and_then(|i| self.entries[i].culling);
                            if c.is_none() {
                                missing += 1;
                            }
                            c.map(|c| (p.clone(), c.sharpness))
                        })
                        .max_by(|a, b| a.1.total_cmp(&b.1));
                    if let Some((p, _)) = best {
                        match stack::set_top(&p, &members) {
                            Ok(c) => changes.extend(c),
                            Err(e) => self.status = format!("Stapel: {e:#}"),
                        }
                    }
                }
                self.apply_stack_changes(changes);
                Ok(if missing > 0 { format!("{} noch nicht analysiert (Bibliothek {} Gesichter & Sichtung)", crate::widgets::image_count(missing), crate::theme::icon::ARROW_RIGHT) } else { String::new() })
            }
            StackAction::Toggle => {
                self.toggle_stack_of_primary();
                Ok(String::new())
            }
            StackAction::ExpandAll => {
                self.expanded_stacks = self.entries.iter().filter_map(|e| e.stack.as_ref().map(|s| s.id.clone())).collect();
                self.refilter();
                Ok(String::new())
            }
            StackAction::CollapseAll => {
                self.expanded_stacks.clear();
                self.refilter();
                Ok(String::new())
            }
            StackAction::AutoDialog => {
                self.auto_stack = Some(AutoStackDialog { seconds: AUTO_STACK_SECONDS });
                Ok(String::new())
            }
        };
        match result {
            Ok(msg) if !msg.is_empty() => self.status = msg,
            Ok(_) => {}
            Err(e) => self.status = format!("Stapel: {e:#}"),
        }
    }

    /// Strg+G: the selected images become a stack with the image in the viewer on top.
    fn stack_selection(&mut self) -> anyhow::Result<String> {
        let paths = self.selected_paths();
        if variant::files_of(&paths).len() < 2 {
            return Ok("Für einen Stapel mindestens zwei Bilder auswählen".into());
        }
        // Images taken from other stacks leave those properly (new top, or dissolved).
        let members = self.stack_members(&self.selected_stack_ids());
        let mut changes = stack::remove(&paths, &members)?;
        let made = stack::create(&paths, self.primary_path().as_deref())?;
        let n = made.len();
        changes.extend(made);
        self.apply_stack_changes(changes);
        Ok(format!("Stapel aus {} gebildet", crate::widgets::image_count(n)))
    }

    /// Umschalt+G, or a click on the stack badge: expands or collapses a stack.
    pub fn toggle_stack(&mut self, id: &str) {
        if !self.expanded_stacks.remove(id) {
            self.expanded_stacks.insert(id.to_string());
        }
        self.refilter();
    }

    fn toggle_stack_of_primary(&mut self) {
        let id = self.primary_path().and_then(|p| self.index_of(&p)).and_then(|i| self.entries[i].stack.as_ref()).map(|s| s.id.clone());
        if let Some(id) = id {
            self.toggle_stack(&id);
        }
    }

    /// Stacks the images (the selection, or all shown ones) by capture time: a gap of more
    /// than `seconds` starts a new stack. Images already in a stack keep it.
    pub fn auto_stack_by_time(&mut self, seconds: f32) {
        // Same condition as the dialog: two or more selected images shown, else the whole view.
        let selected: Vec<usize> = self.visible.iter().copied().filter(|&i| self.selection.contains(&self.entries[i].path)).collect();
        let pool = if selected.len() >= 2 { selected } else { self.visible.clone() };
        let entries: Vec<&ImageRecord> = pool.iter().map(|&i| &self.entries[i]).filter(|e| e.stack.is_none() && variant::number(&e.path) == 1).collect();
        // Missing capture dates come from the files' headers, read in parallel.
        let items: Vec<(PathBuf, Option<i64>)> = entries
            .par_iter()
            .map(|e| {
                let date = e.capture_date.clone().or_else(|| sk_core::raw::read_meta(&e.path).ok().and_then(|m| m.info.date));
                (e.path.clone(), date.as_deref().and_then(stack::parse_capture_time))
            })
            .collect();
        let groups = stack::group_by_time(&items, seconds.round() as i64);
        let mut changes = Vec::new();
        for g in &groups {
            match stack::create(g, None) {
                Ok(c) => changes.extend(c),
                Err(e) => {
                    self.status = format!("Stapel: {e:#}");
                    break;
                }
            }
        }
        let n = groups.len();
        self.apply_stack_changes(changes);
        self.status = match n {
            0 => "Keine Serien gefunden".into(),
            1 => "1 Stapel gebildet".into(),
            n => format!("{n} Stapel gebildet"),
        };
    }

    /// Stack entries of the image menu and the thumbnail context menu.
    pub fn stack_menu(&mut self, ui: &mut Ui) {
        let n_files = variant::files_of(&self.selected_paths()).len();
        let in_stack = !self.selected_stack_ids().is_empty();
        let mut act = |ui: &mut Ui, ic: &str, text: &str, shortcut: String, enabled: bool, a: StackAction| {
            if ui.add_enabled(enabled, Button::new(format!("{ic}  {text}")).shortcut_text(shortcut)).clicked() {
                self.pending = Some(PendingAction::Stack(a));
            }
        };
        ui.menu_button(format!("{}  Stapel", icon::STACK_SIMPLE), |ui| {
            act(ui, icon::STACK_PLUS, "Stapel bilden", shortcut_of("stack-create"), n_files >= 2, StackAction::Create);
            act(ui, icon::STACK_MINUS, "Stapel auflösen", shortcut_of("stack-dissolve"), in_stack, StackAction::Dissolve);
            act(ui, icon::ARROW_SQUARE_OUT, "Aus Stapel entfernen", String::new(), in_stack, StackAction::RemoveFromStack);
            act(ui, icon::ARROW_LINE_UP, "Als oberstes Bild", String::new(), in_stack, StackAction::MakeTop);
            act(ui, icon::SCAN, "Schärfstes Bild nach oben", String::new(), in_stack, StackAction::SharpestTop);
            ui.separator();
            act(ui, icon::ARROWS_OUT_SIMPLE, "Stapel auf-/zuklappen", shortcut_of("stack-toggle"), in_stack, StackAction::Toggle);
            act(ui, icon::ARROWS_OUT, "Alle Stapel aufklappen", String::new(), true, StackAction::ExpandAll);
            act(ui, icon::ARROWS_IN, "Alle Stapel zuklappen", String::new(), true, StackAction::CollapseAll);
            ui.separator();
            act(ui, icon::CLOCK, "Automatisch nach Aufnahmezeit …", String::new(), true, StackAction::AutoDialog);
        });
    }

    pub(crate) fn stack_dialogs(&mut self, ctx: &egui::Context) {
        let selected = self.selected_count() >= 2;
        let Some(dialog) = &mut self.auto_stack else { return };
        let (mut run, mut cancel) = (false, false);
        let modal = Dialog::new("auto_stack", icon::STACK_SIMPLE, "Automatisch stapeln").subtitle("Serien nach Aufnahmezeit zusammenfassen").width(440.0).show(ctx, |ui| {
            let scope = if selected { "Die ausgewählten Bilder" } else { "Alle Bilder dieser Ansicht" };
            ui.label(format!("{scope} werden nach Aufnahmezeit gestapelt. Bilder, die schon in einem Stapel sind, bleiben dort."));
            dialog::card(ui, "Trennen", None, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Neuer Stapel nach einer Pause von mehr als");
                    ui.add(egui::DragValue::new(&mut dialog.seconds).range(0.0..=3600.0).speed(0.2).max_decimals(0).suffix(" s"));
                });
            });
            dialog::footer(ui, |ui| {
                if dialog::default_button(ui, true, dialog::primary("Stapeln")) {
                    run = true;
                }
                if ui.add(dialog::secondary("Abbrechen")).clicked() {
                    cancel = true;
                }
            });
        });
        if run {
            let seconds = dialog.seconds;
            self.auto_stack = None;
            self.auto_stack_by_time(seconds);
        } else if cancel || modal.should_close() {
            self.auto_stack = None;
        }
    }
}
