//! The shortcut window (F1): every command with its shortcuts, which can be changed, and the
//! keys that are fixed.

use egui::{Key, RichText, vec2};

use crate::app::App;
use crate::dialog::{self, Dialog};
use crate::browser::{SPEED_KEYS, SpeedParam};
use crate::commands::{self, COMMANDS};
use crate::theme::{self, icon};

#[derive(Default)]
pub struct ShortcutEditor {
    pub filter: String,
    /// The shortcut being recorded: command id, and which of its shortcuts it replaces (`None`:
    /// an additional one).
    pub recording: Option<(&'static str, Option<usize>)>,
    /// What the last change did, e.g. that a key was taken from another command.
    pub message: Option<String>,
}

/// Keys that work without the command list.
fn fixed_keys() -> Vec<(String, String)> {
    let speed: Vec<String> = SPEED_KEYS
        .iter()
        .map(|(k, p)| {
            let what = match p {
                SpeedParam::Exposure => "Belichtung",
                SpeedParam::Contrast => "Kontrast",
                SpeedParam::Saturation => "Sättigung",
                SpeedParam::Highlights => "Lichter",
                SpeedParam::Shadows => "Tiefen",
                SpeedParam::Temperature => "Temperatur",
            };
            format!("{} {what}", k.name())
        })
        .collect();
    [
        ("Strg+K", "Befehl oder Werkzeug suchen"),
        ("Strg+1 – Strg+0", "Werkzeug-Tabs"),
        ("Esc", "Werkzeug beenden, Vollbild verlassen"),
        ("Leertaste + Ziehen", "Bild verschieben"),
        ("Doppelklick", "100 % / Einpassen"),
        ("Strg+Mausrad", "Vorschaugröße im Raster"),
        ("Maustaste halten", "Lupe (100 %) im eingepassten Bild"),
        ("Strg/Umschalt+Klick", "Mehrere Bilder auswählen und nebeneinander vergleichen"),
        ("Alt+F4", "Beenden"),
    ]
    .into_iter()
    .map(|(k, w)| (k.to_string(), w.to_string()))
    .chain([(
        SPEED_KEYS.iter().map(|(k, _)| k.name()).collect::<Vec<_>>().join(" "),
        format!("Taste halten + Mausrad: Schnellbearbeitung ({})", speed.join(", ")),
    )])
    .collect()
}

impl App {
    pub(crate) fn shortcut_editor(&mut self, ctx: &egui::Context) {
        if !self.show_shortcuts {
            self.shortcut_editor.recording = None;
            return;
        }
        let mut changed = false;
        if let Some((id, replace)) = self.shortcut_editor.recording
            && let Some(key) = commands::pressed_key(ctx)
        {
            let ed = &mut self.shortcut_editor;
            match key {
                (m, Key::Escape) if m.is_none() => ed.recording = None,
                k if commands::reserved(k).is_some() => {
                    ed.message = Some(format!("{} ist fest vergeben: {}", commands::key_text(k), commands::reserved(k).unwrap_or_default()));
                }
                k => {
                    let label = commands::get(id).map_or("", |c| c.label);
                    ed.message = Some(match commands::assign(id, k, replace) {
                        Some(previous) => format!("{} gehört jetzt zu „{label}“ (vorher „{previous}“)", commands::key_text(k)),
                        None => format!("{} gehört jetzt zu „{label}“", commands::key_text(k)),
                    });
                    ed.recording = None;
                    changed = true;
                }
            }
            // The key belongs to the recording, not to a focused button or the window.
            ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::Text(_))));
        }

        let (mut close, mut reset_all) = (false, false);
        let dialog = Dialog::new("shortcuts", icon::KEYBOARD, "Tastenkürzel")
            .subtitle("Klick auf ein Kürzel ändert es, + fügt eines hinzu, Rechtsklick entfernt es")
            .width(600.0);
        let modal = dialog.show(ctx, |ui| {
            let ed = &mut self.shortcut_editor;
            ui.add(egui::TextEdit::singleline(&mut ed.filter).hint_text(format!("{}  Befehl suchen", icon::MAGNIFYING_GLASS)).desired_width(f32::INFINITY));
            match &ed.message {
                Some(m) => ui.label(RichText::new(m).color(theme::ACCENT)),
                None => ui.label(RichText::new("Esc bricht eine Aufnahme ab.").size(12.0).color(theme::TEXT_DIM)),
            };
            ui.add_space(2.0);
            let filter = ed.filter.trim().to_lowercase();
            let mut groups: Vec<&str> = Vec::new();
            for c in COMMANDS {
                if !groups.contains(&c.group) {
                    groups.push(c.group);
                }
            }
            // One width for the names in every group, so that the keys line up in one column.
            let font = egui::TextStyle::Body.resolve(ui.style());
            let width = |t: String| ui.painter().layout_no_wrap(t, font.clone(), egui::Color32::WHITE).size().x;
            let label_w = COMMANDS
                .iter()
                .map(|c| width(format!("{}  {}", c.icon, c.label.trim_end_matches(" …"))))
                .fold(270.0, f32::max);
            let height = (ctx.content_rect().height() - 260.0).clamp(200.0, 640.0);
            egui::ScrollArea::vertical().auto_shrink([true, false]).max_height(height).min_scrolled_height(height).show(ui, |ui| {
                for group in groups {
                    let cmds: Vec<_> = COMMANDS
                        .iter()
                        .filter(|c| c.group == group)
                        .filter(|c| filter.is_empty() || c.label.to_lowercase().contains(&filter) || c.keywords.contains(&filter) || c.shortcut().to_lowercase().contains(&filter))
                        .collect();
                    if cmds.is_empty() {
                        continue;
                    }
                    ui.add_space(10.0);
                    ui.label(RichText::new(group).size(12.0).strong().color(theme::TEXT_DIM));
                    egui::Grid::new(group).num_columns(2).spacing(vec2(16.0, 3.0)).min_col_width(label_w).show(ui, |ui| {
                        for c in cmds {
                            ui.label(format!("{}  {}", c.icon, c.label.trim_end_matches(" …")));
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                let keys = c.bindings();
                                for (i, k) in keys.iter().enumerate() {
                                    let rec = ed.recording == Some((c.id, Some(i)));
                                    let text = if rec { "Taste drücken …".to_string() } else { commands::key_text(*k) };
                                    let r = ui.add(egui::Button::new(RichText::new(text).monospace().color(theme::ACCENT)).selected(rec));
                                    if r.clicked() {
                                        ed.recording = Some((c.id, Some(i)));
                                        ed.message = None;
                                        r.surrender_focus();
                                    }
                                    if r.secondary_clicked() {
                                        commands::set_bindings(c.id, keys.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, k)| *k).collect());
                                        ed.message = Some(format!("{} entfernt", commands::key_text(*k)));
                                        changed = true;
                                    }
                                }
                                let rec = ed.recording == Some((c.id, None));
                                let add = egui::Button::new(if rec { RichText::new("Taste drücken …").monospace().color(theme::ACCENT) } else { RichText::new(icon::PLUS).color(theme::TEXT_FAINT) })
                                    .selected(rec)
                                    .frame_when_inactive(rec);
                                let r = ui.add(add).on_hover_text("Weiteres Tastenkürzel");
                                if r.clicked() {
                                    ed.recording = Some((c.id, None));
                                    ed.message = None;
                                    r.surrender_focus();
                                }
                                if commands::is_changed(c.id) {
                                    let default = c.keys.iter().map(|k| commands::key_text(*k)).collect::<Vec<_>>().join(", ");
                                    let hover = if default.is_empty() { "Zurücksetzen (ohne Kürzel)".to_string() } else { format!("Zurücksetzen auf {default}") };
                                    if ui.small_button(icon::ARROW_COUNTER_CLOCKWISE).on_hover_text(hover).clicked() {
                                        commands::set_bindings(c.id, c.keys.to_vec());
                                        ed.message = None;
                                        changed = true;
                                    }
                                }
                            });
                            ui.end_row();
                        }
                    });
                }
                if filter.is_empty() {
                    ui.add_space(14.0);
                    ui.label(RichText::new("Fest vergeben").size(12.0).strong().color(theme::TEXT_DIM));
                    egui::Grid::new("fixed_keys").num_columns(2).spacing(vec2(16.0, 3.0)).min_col_width(label_w).show(ui, |ui| {
                        for (key, what) in fixed_keys() {
                            ui.label(what);
                            ui.label(RichText::new(key).monospace().color(theme::TEXT_DIM));
                            ui.end_row();
                        }
                    });
                }
            });
            let any = COMMANDS.iter().any(|c| commands::is_changed(c.id));
            dialog::footer(ui, |ui| {
                close = ui.add(dialog::primary("Fertig")).clicked();
                reset_all = ui.add_enabled(any, dialog::secondary("Alle zurücksetzen")).clicked();
            });
        });
        if reset_all {
            commands::reset_all_bindings();
            self.shortcut_editor.message = Some("Alle Tastenkürzel zurückgesetzt".into());
            changed = true;
        }
        if changed {
            self.save_settings();
        }
        // Esc closes the window, unless it ended a recording.
        let esc = self.shortcut_editor.recording.is_none() && modal.should_close();
        if close || esc {
            self.show_shortcuts = false;
            self.shortcut_editor.recording = None;
            self.shortcut_editor.message = None;
        }
    }
}
