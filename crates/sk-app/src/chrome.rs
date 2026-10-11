//! Window chrome: menu bar, toolbar, tool tab rail, status bar and the help dialogs.

use egui::{Button, Color32, Margin, RichText, Sense, Ui, vec2};
use sk_core::params::Adjustments;
use sk_core::sidecar::ColorLabel;

use crate::app::{App, Tab};
use crate::library::scope_label;
use crate::theme::{self, icon};

/// Keyboard shortcuts as shown in menus and in the help dialog.
const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    (
        "Datei",
        &[
            ("Strg+O", "Ordner öffnen"),
            ("Strg+D", "Auswahl verarbeiten (exportieren)"),
            ("F1", "Diese Übersicht"),
            ("Alt+F4", "Beenden"),
        ],
    ),
    (
        "Bearbeiten",
        &[
            ("Strg+Z", "Rückgängig"),
            ("Strg+Y", "Wiederholen"),
            ("Strg+Umschalt+C", "Anpassungen kopieren"),
            ("Strg+Umschalt+V", "Anpassungen auf Auswahl anwenden"),
            ("Strg+A", "Alle auswählen"),
        ],
    ),
    (
        "Bild",
        &[
            ("\u{E058} / \u{E06C}", "Vorheriges / nächstes Bild"),
            ("0 – 5", "Bewertung"),
            ("6 – 9", "Farb-Label Rot, Gelb, Grün, Blau"),
            ("Strg+[ / Strg+]", "Links / rechts drehen"),
            ("C", "Zuschneiden (Enter übernimmt)"),
            ("W", "Weißabgleich-Pipette"),
        ],
    ),
    (
        "Ansicht",
        &[
            ("G", "Raster / Viewer"),
            ("Z", "100 % / Einpassen (auch Doppelklick)"),
            ("Y", "Vorher / Nachher"),
            ("U", "Geteilter Vergleich"),
            ("E", "Belichtungswarnung"),
        ],
    ),
    (
        "Ebenen & Retusche",
        &[
            ("B / N", "Pinsel / Radierer"),
            ("L / R", "Linearer / radialer Verlauf"),
            ("J", "Magic Brush"),
            ("H / S", "Reparieren / Klonen"),
            ("[ / ]", "Pinselgröße"),
            ("M", "Maske anzeigen"),
            ("Entf", "Retuschepunkt löschen"),
            ("Taste halten + Mausrad", "Speed Edit (Q, A, S, D …)"),
        ],
    ),
];

impl App {
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.current.as_ref().is_some_and(|c| c.adjustments != c.committed)
    }

    pub fn rotate(&mut self, quarter_turns: u8) {
        self.edit(|a| a.rotation = (a.rotation + quarter_turns) % 4);
    }

    pub fn reset_adjustments(&mut self) {
        self.edit(|a| {
            let mut n = Adjustments::default();
            n.keep_geometry_from(a);
            *a = n;
        });
    }

    pub fn toggle_crop(&mut self) {
        if self.crop_tool.is_some() {
            self.finish_crop_tool();
        } else {
            self.start_crop_tool();
        }
    }

    pub fn toggle_before(&mut self) {
        self.show_before = !self.show_before;
        self.split_view = false;
        self.request_render();
    }

    pub fn toggle_clip_warning(&mut self) {
        self.clip_warning = !self.clip_warning;
        self.request_render();
    }

    pub fn zoom_fit(&mut self) {
        self.zoom = None;
        self.pan = egui::Vec2::ZERO;
    }

    pub fn select_all(&mut self) {
        self.selection = self.visible.iter().map(|&i| self.entries[i].path.clone()).collect();
    }

    /// Native "choose folder" dialog.
    pub fn open_folder_dialog(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().set_title("Ordner öffnen").set_directory(&self.folder).pick_folder() {
            self.grid_view = true;
            self.open_folder(dir);
        }
    }

    pub fn import_folder_dialog(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().set_title("Ordner in Katalog importieren").set_directory(&self.folder).pick_folder() {
            self.open_folder(dir.clone());
            self.import_folder(&dir);
        }
    }

    // ------------------------------------------------------------ menu bar

    pub fn menu_bar(&mut self, ui: &mut Ui) {
        let has = self.current.is_some();
        let n_sel = self.selected_paths().len();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("Datei", |ui| {
                if item(ui, icon::FOLDER_OPEN, "Ordner öffnen …", "Strg+O", true) {
                    self.open_folder_dialog();
                }
                if item(ui, icon::FOLDER_PLUS, "Ordner in Katalog importieren …", "", true) {
                    self.import_folder_dialog();
                }
                ui.separator();
                if item(ui, icon::EXPORT, &format!("Verarbeiten ({n_sel} Bilder)"), "Strg+D", n_sel > 0) {
                    self.export_selection();
                }
                ui.separator();
                if item(ui, icon::X, "Beenden", "Alt+F4", true) {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Bearbeiten", |ui| {
                if item(ui, icon::ARROW_U_UP_LEFT, "Rückgängig", "Strg+Z", self.can_undo()) {
                    self.undo();
                }
                if item(ui, icon::ARROW_U_UP_RIGHT, "Wiederholen", "Strg+Y", !self.redo.is_empty()) {
                    self.redo();
                }
                ui.separator();
                if item(ui, icon::COPY, "Anpassungen kopieren", "Strg+Umschalt+C", has) {
                    self.copy_adjustments();
                }
                if item(ui, icon::CLIPBOARD_TEXT, "Anpassungen anwenden", "Strg+Umschalt+V", self.clipboard.is_some()) {
                    self.paste_adjustments();
                }
                if item(ui, icon::ARROW_COUNTER_CLOCKWISE, "Alle Anpassungen zurücksetzen", "", has) {
                    self.reset_adjustments();
                }
                ui.separator();
                if item(ui, icon::CHECK_SQUARE, "Alle auswählen", "Strg+A", !self.visible.is_empty()) {
                    self.select_all();
                }
            });
            ui.menu_button("Bild", |ui| {
                if item(ui, icon::ARROW_COUNTER_CLOCKWISE, "Links drehen", "Strg+[", has) {
                    self.rotate(3);
                }
                if item(ui, icon::ARROW_CLOCKWISE, "Rechts drehen", "Strg+]", has) {
                    self.rotate(1);
                }
                if item(ui, icon::CROP, "Zuschneiden", "C", has) {
                    self.toggle_crop();
                }
                if item(ui, icon::EYEDROPPER, "Weißabgleich aufnehmen", "W", has) {
                    self.tab = Tab::Color;
                    self.picking_wb = true;
                }
                ui.separator();
                ui.add_enabled_ui(n_sel > 0, |ui| {
                    ui.menu_button(format!("{}  Bewertung", icon::STAR), |ui| {
                        for r in 0..=5u8 {
                            let text = if r == 0 { "Keine".to_string() } else { "★".repeat(r as usize) };
                            if ui.add(Button::new(text).shortcut_text(r.to_string())).clicked() {
                                self.set_rating(r);
                            }
                        }
                    });
                    ui.menu_button(format!("{}  Farb-Label", icon::TAG), |ui| {
                        for (l, key) in [
                            (ColorLabel::None, ""),
                            (ColorLabel::Red, "6"),
                            (ColorLabel::Yellow, "7"),
                            (ColorLabel::Green, "8"),
                            (ColorLabel::Blue, "9"),
                        ] {
                            if ui.add(Button::new(l.label()).shortcut_text(key)).clicked() {
                                self.set_color_label(l);
                            }
                        }
                    });
                });
                ui.separator();
                if item(ui, icon::CARET_LEFT, "Vorheriges Bild", icon::ARROW_LEFT, !self.visible.is_empty()) {
                    self.step(-1);
                }
                if item(ui, icon::CARET_RIGHT, "Nächstes Bild", icon::ARROW_RIGHT, !self.visible.is_empty()) {
                    self.step(1);
                }
            });
            ui.menu_button("Ansicht", |ui| {
                if check(ui, self.grid_view, "Raster", "G") {
                    self.grid_view = !self.grid_view;
                }
                if check(ui, self.show_filmstrip, "Filmstreifen", "") {
                    self.show_filmstrip = !self.show_filmstrip;
                }
                ui.separator();
                if check(ui, self.zoom.is_none(), "Einpassen", "") {
                    self.zoom_fit();
                }
                if item(ui, icon::MAGNIFYING_GLASS_PLUS, "100 %", "Z", has) {
                    self.zoom = None;
                    self.toggle_zoom_100();
                }
                ui.separator();
                if check(ui, self.show_before, "Vorher anzeigen", "Y") {
                    self.toggle_before();
                }
                if check(ui, self.split_view, "Geteilter Vergleich", "U") {
                    self.toggle_split();
                }
                if check(ui, self.clip_warning, "Belichtungswarnung", "E") {
                    self.toggle_clip_warning();
                }
                ui.separator();
                ui.menu_button(format!("{}  Werkzeug", icon::SIDEBAR_SIMPLE), |ui| {
                    for tab in Tab::ALL {
                        if ui.add(Button::selectable(self.tab == tab, format!("{}  {}", tab.icon(), tab.title()))).clicked() {
                            self.tab = tab;
                        }
                    }
                });
            });
            ui.menu_button("Hilfe", |ui| {
                if item(ui, icon::KEYBOARD, "Tastenkürzel", "F1", true) {
                    self.show_shortcuts = true;
                }
                ui.separator();
                if item(ui, icon::INFO, "Über Silberkorn", "", true) {
                    self.show_about = true;
                }
            });
        });
    }

    // ------------------------------------------------------------ toolbar

    pub fn toolbar(&mut self, ui: &mut Ui) {
        let has = self.current.is_some();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if tool(ui, icon::FOLDER_OPEN, "Ordner öffnen (Strg+O)", false, true).clicked() {
                self.open_folder_dialog();
            }
            group_gap(ui);
            if tool(ui, icon::ARROW_U_UP_LEFT, "Rückgängig (Strg+Z)", false, self.can_undo()).clicked() {
                self.undo();
            }
            if tool(ui, icon::ARROW_U_UP_RIGHT, "Wiederholen (Strg+Y)", false, !self.redo.is_empty()).clicked() {
                self.redo();
            }
            group_gap(ui);
            if tool(ui, icon::ARROW_COUNTER_CLOCKWISE, "Links drehen (Strg+[)", false, has).clicked() {
                self.rotate(3);
            }
            if tool(ui, icon::ARROW_CLOCKWISE, "Rechts drehen (Strg+])", false, has).clicked() {
                self.rotate(1);
            }
            if tool(ui, icon::CROP, "Zuschneiden (C)", self.crop_tool.is_some(), has).clicked() {
                self.toggle_crop();
            }
            if tool(ui, icon::EYEDROPPER, "Weißabgleich aufnehmen (W)", self.picking_wb, has).clicked() {
                self.picking_wb = !self.picking_wb;
            }
            group_gap(ui);
            if tool(ui, icon::COPY, "Anpassungen kopieren (Strg+Umschalt+C)", false, has).clicked() {
                self.copy_adjustments();
            }
            if tool(ui, icon::CLIPBOARD_TEXT, "Anpassungen auf Auswahl anwenden (Strg+Umschalt+V)", false, self.clipboard.is_some()).clicked() {
                self.paste_adjustments();
            }
            if tool(ui, icon::ARROWS_COUNTER_CLOCKWISE, "Alle Anpassungen zurücksetzen", false, has).clicked() {
                self.reset_adjustments();
            }
            group_gap(ui);
            if tool(ui, icon::SQUARES_FOUR, "Raster (G)", self.grid_view, true).clicked() {
                self.grid_view = true;
            }
            if tool(ui, icon::IMAGE, "Viewer (G)", !self.grid_view, true).clicked() {
                self.grid_view = false;
            }
            group_gap(ui);
            if tool(ui, icon::CIRCLE_HALF, "Vorher / Nachher (Y)", self.show_before, has).clicked() {
                self.toggle_before();
            }
            if tool(ui, icon::COLUMNS, "Geteilter Vergleich (U)", self.split_view, has).clicked() {
                self.toggle_split();
            }
            if tool(ui, icon::WARNING, "Belichtungswarnung (E)", self.clip_warning, has).clicked() {
                self.toggle_clip_warning();
            }
            group_gap(ui);
            if tool(ui, icon::CORNERS_OUT, "Einpassen", self.zoom.is_none(), has).clicked() {
                self.zoom_fit();
            }
            if tool(ui, icon::MAGNIFYING_GLASS_PLUS, "100 % (Z, Doppelklick)", self.zoom.is_some(), has).clicked() {
                self.zoom = None;
                self.toggle_zoom_100();
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let n = self.selected_paths().len();
                let text = RichText::new(format!("{}  Verarbeiten", icon::EXPORT)).color(Color32::WHITE);
                let button = Button::new(text).fill(theme::ACCENT).min_size(vec2(0.0, 28.0));
                if ui.add_enabled(n > 0, button).on_hover_text(format!("{n} Bild(er) mit dem gewählten Rezept exportieren (Strg+D)")).clicked() {
                    self.export_selection();
                }
            });
        });
    }

    // ------------------------------------------------------------ tool tabs

    /// Vertical strip of tool tab icons.
    pub fn tab_rail(&mut self, ui: &mut Ui) {
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add_space(4.0);
            for tab in Tab::ALL {
                let selected = self.tab == tab;
                let (rect, resp) = ui.allocate_exact_size(vec2(40.0, 38.0), Sense::click());
                let painter = ui.painter();
                if selected {
                    painter.rect_filled(rect, 4.0, theme::PANEL);
                    painter.rect_filled(egui::Rect::from_min_size(rect.left_top() + vec2(0.0, 7.0), vec2(3.0, 24.0)), 1.5, theme::ACCENT);
                } else if resp.hovered() {
                    painter.rect_filled(rect, 4.0, Color32::from_rgb(44, 44, 47));
                }
                let color = if selected { Color32::WHITE } else if resp.hovered() { theme::TEXT } else { theme::TEXT_DIM };
                painter.text(rect.center(), egui::Align2::CENTER_CENTER, tab.icon(), egui::FontId::proportional(20.0), color);
                if resp.on_hover_text(tab.title()).clicked() {
                    self.tab = tab;
                }
            }
        });
    }

    pub fn tool_panel(&mut self, ui: &mut Ui) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(RichText::new(format!("{}  {}", self.tab.icon(), self.tab.title())).size(15.0).strong().color(Color32::WHITE));
        });
        ui.add_space(6.0);
        self.tools_ui(ui);
    }

    // ------------------------------------------------------------ status bar

    pub fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let dim = |t: String| RichText::new(t).size(12.0).color(theme::TEXT_DIM);
            if let Some(cur) = &self.current {
                let info = &cur.meta.info;
                let name = cur.meta.path.file_name().unwrap_or_default().to_string_lossy().to_string();
                ui.label(RichText::new(name).size(12.0).color(theme::TEXT));
                let exif: Vec<String> = [
                    Some(format!("{} {}", info.make, info.model).trim().to_string()).filter(|s| !s.is_empty()),
                    info.iso.map(|i| format!("ISO {i}")),
                    info.exposure_time.clone(),
                    info.aperture.clone(),
                    info.focal_length.clone(),
                    Some(format!("{} × {}", info.width, info.height)),
                ]
                .into_iter()
                .flatten()
                .collect();
                ui.label(dim(exif.join("  ·  ")));
            }
            if !self.status.is_empty() {
                ui.separator();
                ui.label(dim(self.status.clone()));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(pct) = self.zoom_pct.filter(|_| !self.grid_view && self.current.is_some()) {
                    ui.label(RichText::new(format!("{pct:.0} %")).size(12.0).color(theme::TEXT));
                    ui.separator();
                }
                let n = self.selected_paths().len();
                let sel = if n > 1 { format!("  ·  {n} ausgewählt") } else { String::new() };
                ui.label(dim(format!("{}  ·  {} Bilder{sel}", scope_label(&self.scope, &self.lists), self.visible.len())));
                if let Some((done, total)) = self.export_progress.or(self.index_progress) {
                    ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32).desired_width(120.0).desired_height(12.0));
                }
            });
        });
    }

    // ------------------------------------------------------------ dialogs

    pub fn dialogs(&mut self, ctx: &egui::Context) {
        if self.show_shortcuts {
            let mut open = true;
            egui::Window::new(format!("{}  Tastenkürzel", icon::KEYBOARD))
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().max_height(ctx.content_rect().height() - 120.0).show(ui, |ui| {
                        ui.columns(3, |cols| {
                            for (i, (group, keys)) in SHORTCUTS.iter().enumerate() {
                                let ui = &mut cols[[0, 0, 1, 1, 2][i]];
                                ui.label(RichText::new(*group).strong().color(Color32::WHITE));
                                egui::Grid::new(group).num_columns(2).spacing(vec2(16.0, 3.0)).show(ui, |ui| {
                                    for (key, what) in *keys {
                                        ui.label(RichText::new(*key).monospace().color(theme::ACCENT));
                                        ui.label(*what);
                                        ui.end_row();
                                    }
                                });
                                ui.add_space(10.0);
                            }
                        });
                    });
                });
            self.show_shortcuts = open && !ctx.input(|i| i.key_pressed(egui::Key::Escape));
        }
        if self.show_about {
            let modal = egui::Modal::new(egui::Id::new("about")).show(ctx, |ui| {
                ui.set_width(320.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(icon::APERTURE).size(42.0).color(theme::ACCENT));
                    ui.label(RichText::new("Silberkorn").size(22.0).strong().color(Color32::WHITE));
                    ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(theme::TEXT_DIM));
                    ui.add_space(8.0);
                    ui.label("RAW-Entwickler und Bildverwaltung in Rust.");
                    ui.add_space(12.0);
                    if ui.add(Button::new("OK").min_size(vec2(90.0, 26.0))).clicked() {
                        self.show_about = false;
                    }
                });
            });
            if modal.should_close() {
                self.show_about = false;
            }
        }
    }
}

/// Menu entry with icon and shortcut; returns true when clicked.
fn item(ui: &mut Ui, ic: &str, text: &str, shortcut: &str, enabled: bool) -> bool {
    ui.add_enabled(enabled, Button::new(format!("{ic}  {text}")).shortcut_text(shortcut)).clicked()
}

/// Menu entry with a check mark.
fn check(ui: &mut Ui, on: bool, text: &str, shortcut: &str) -> bool {
    let mark = if on { icon::CHECK } else { "   " };
    ui.add(Button::new(format!("{mark}  {text}")).shortcut_text(shortcut)).clicked()
}

/// Square toolbar button; `active` draws it as pressed.
fn tool(ui: &mut Ui, ic: &str, hover: &str, active: bool, enabled: bool) -> egui::Response {
    let mut b = Button::new(RichText::new(ic).size(18.0)).min_size(vec2(32.0, 28.0)).frame_when_inactive(active);
    if active {
        b = b.fill(Color32::from_rgb(56, 76, 100)).stroke(egui::Stroke::new(1.0, theme::ACCENT));
    }
    ui.add_enabled(enabled, b).on_hover_text(hover).on_disabled_hover_text(hover)
}

fn group_gap(ui: &mut Ui) {
    ui.add_space(6.0);
    let (rect, _) = ui.allocate_exact_size(vec2(1.0, 20.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(58, 58, 62));
    ui.add_space(6.0);
}

pub fn bar_margin() -> Margin {
    Margin::symmetric(8, 4)
}
