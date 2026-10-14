//! Copying adjustments between images: "Anpassungen kopieren" takes
//! the edited tools of the image, the dialog lets one choose the tools, and the button in a
//! tool's header carries that one tool over to the other selected images.

use std::collections::BTreeSet;

use egui::{Color32, RichText, vec2};
use sk_core::params::Adjustments;
use sk_core::tools::{self, Tool};

use crate::app::App;
use crate::theme::{self, icon};
use crate::widgets::image_count;

/// Copied adjustments and the tools that are applied from them.
#[derive(Clone)]
pub struct Clipboard {
    pub adjustments: Adjustments,
    pub tools: Vec<Tool>,
}

/// The dialog to choose the tools to copy (open while `Some`).
#[derive(Default)]
pub struct CopyDialog {
    pub tools: Option<BTreeSet<Tool>>,
}

/// "Belichtung, Körnung und 3 weitere".
pub fn tool_list(tools: &[Tool]) -> String {
    let names: Vec<&str> = tools.iter().map(|t| t.label()).collect();
    match names.len() {
        0 => "nichts".into(),
        1..=3 => names.join(", "),
        n => format!("{} und {} weitere", names[..2].join(", "), n - 2),
    }
}

/// Checkboxes for all tools in their groups, in two columns; tools changed in `adj` are shown
/// brighter.
pub fn tool_checkboxes(ui: &mut egui::Ui, adj: &Adjustments, chosen: &mut BTreeSet<Tool>) {
    ui.columns(2, |cols| {
        for (i, (group, group_tools)) in tools::GROUPS.iter().enumerate() {
            let ui = &mut cols[usize::from(i >= 2)];
            ui.label(RichText::new(*group).strong().color(Color32::WHITE));
            for &t in *group_tools {
                let edited = !t.is_default(adj);
                let text = RichText::new(t.label()).color(if edited { theme::TEXT } else { theme::TEXT_FAINT });
                let mut on = chosen.contains(&t);
                let r = ui.checkbox(&mut on, text);
                let r = match (edited, t.image_specific()) {
                    (_, true) => r.on_hover_text("Gehört zum Bildinhalt; nur dabei, wenn hier gewählt"),
                    (false, _) => r.on_hover_text("Nicht bearbeitet: setzt das Werkzeug beim Anwenden zurück"),
                    _ => r,
                };
                if r.changed() {
                    if on {
                        chosen.insert(t);
                    } else {
                        chosen.remove(&t);
                    }
                }
            }
            ui.add_space(8.0);
        }
    });
}

impl App {
    /// Copies the edited tools of the current image (not crop, rotation, perspective, layers and
    /// retouching, which belong to its content).
    pub(crate) fn copy_adjustments(&mut self) {
        let Some(cur) = &self.current else { return };
        let tools = tools::edited(&cur.adjustments);
        self.status = if tools.is_empty() {
            "Kopiert: keine bearbeiteten Werkzeuge".into()
        } else {
            format!("Kopiert: {}", tool_list(&tools))
        };
        self.clipboard = Some(Clipboard { adjustments: cur.adjustments.clone(), tools });
    }

    /// Applies the copied tools to the selected images; their other settings stay.
    pub(crate) fn paste_adjustments(&mut self) {
        let Some(clip) = self.clipboard.clone() else { return };
        let targets = self.selected_paths();
        self.set_adjustments_on(&targets, |a| tools::apply(&clip.tools, &clip.adjustments, a));
        self.status = format!("{} auf {} angewendet", tool_list(&clip.tools), image_count(targets.len()));
    }

    /// Opens the dialog with the edited tools of the current image chosen.
    pub(crate) fn open_copy_dialog(&mut self) {
        let Some(cur) = &self.current else { return };
        self.copy_dialog.tools = Some(tools::edited(&cur.adjustments).into_iter().collect());
    }

    /// Carries one tool of the current image over to the other selected images.
    pub(crate) fn transfer_tool(&mut self, tool: Tool) {
        let Some(cur) = &self.current else { return };
        if tool == Tool::NoiseReduction {
            let d = &cur.adjustments.detail;
            return self.transfer_noise((d.ai_denoise, d.nr_luma, d.nr_color));
        }
        let from = cur.adjustments.clone();
        let others = self.other_selected();
        self.set_adjustments_on(&others, |a| tools::apply(&[tool], &from, a));
        self.status = format!("„{}“ auf {} übertragen", tool.label(), image_count(others.len()));
    }

    /// The other selected images (not the one shown).
    fn other_selected(&self) -> Vec<std::path::PathBuf> {
        let current = self.current_path();
        self.selected_paths().into_iter().filter(|p| Some(p) != current.as_ref()).collect()
    }

    pub(crate) fn copy_dialog(&mut self, ctx: &egui::Context) {
        if self.copy_dialog.tools.is_none() {
            return;
        }
        let others = self.other_selected();
        let Some(cur) = self.current.as_ref() else {
            self.copy_dialog.tools = None;
            return;
        };
        let Some(chosen) = self.copy_dialog.tools.as_mut() else { return };
        let adj = &cur.adjustments;
        let source = sk_core::variant::display_name(&cur.meta.path);
        let (mut open, mut copy, mut apply) = (true, false, false);
        egui::Window::new(format!("{}  Anpassungen kopieren", icon::COPY))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(RichText::new(format!("Von {source}")).color(theme::TEXT_DIM));
                ui.add_space(6.0);
                tool_checkboxes(ui, adj, chosen);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Wählen:").color(theme::TEXT_DIM));
                    if ui.small_button("Bearbeitete").on_hover_text("Alle bearbeiteten Werkzeuge außer Zuschnitt, Drehen, Perspektive, Ebenen und Retusche").clicked() {
                        *chosen = tools::edited(adj).into_iter().collect();
                    }
                    if ui.small_button("Alle").clicked() {
                        *chosen = Tool::ALL.into_iter().collect();
                    }
                    if ui.small_button("Keine").clicked() {
                        chosen.clear();
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    copy = ui.add_enabled(!chosen.is_empty(), egui::Button::new("Kopieren")).on_hover_text("Mit Strg+Umschalt+V auf die Auswahl anwenden").clicked();
                    apply = ui
                        .add_enabled(!chosen.is_empty() && !others.is_empty(), egui::Button::new(if others.is_empty() { "Auf Auswahl anwenden".to_string() } else { format!("Auf {} anwenden", image_count(others.len())) }))
                        .on_disabled_hover_text("Weitere Bilder auswählen, um sie gleich zu übernehmen")
                        .clicked();
                });
            });
        if copy || apply {
            let tools: Vec<Tool> = chosen.iter().copied().collect();
            let from = adj.clone();
            self.status = format!("Kopiert: {}", tool_list(&tools));
            self.copy_dialog.tools = None;
            if apply {
                self.set_adjustments_on(&others, |a| tools::apply(&tools, &from, a));
                self.status = format!("{} auf {} angewendet", tool_list(&tools), image_count(others.len()));
            }
            self.clipboard = Some(Clipboard { adjustments: from, tools });
        }
        if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.copy_dialog.tools = None;
        }
    }
}
