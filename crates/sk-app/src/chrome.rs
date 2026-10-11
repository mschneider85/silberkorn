//! Window chrome: title bar with menus, toolbar, tool tab rail, status bar and the help dialogs.

use egui::containers::menu::{self, MenuConfig, MenuState, SubMenu};
use egui::{
    Align, Button, Color32, FontId, Id, Key, Layout, Margin, Modifiers, Order, PointerButton, Popup, PopupCloseBehavior,
    Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder, UiKind, UiStackInfo, ViewportCommand, pos2, vec2,
};
use sk_core::params::Adjustments;
use sk_core::sidecar::ColorLabel;

use crate::app::{App, Tab};
use crate::library::scope_label;
use crate::theme::{self, icon};
use crate::widgets::image_count;

/// Keyboard shortcuts as shown in menus and in the help dialog.
const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    (
        "Datei",
        &[
            ("Strg+N", "Neue Session"),
            ("Strg+Umschalt+O", "Session öffnen"),
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
            ("Strg+J", "Nach Auswahl (Selects) verschieben"),
            ("Strg+Entf", "In den Session-Papierkorb"),
            ("Strg+[ / Strg+]", "Links / rechts drehen"),
            ("C", "Zuschneiden (Enter übernimmt)"),
            ("F2 / F3", "Neue Variante / Variante klonen"),
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
            ("Umschalt+U", "Vorher | Nachher nebeneinander"),
            ("Umschalt+Enter", "Referenzbild festlegen / entfernen"),
            ("Strg/Umschalt+Klick", "Mehrere Bilder nebeneinander vergleichen"),
            ("E", "Belichtungswarnung"),
            ("K", "Fokusmaske (scharfe Bereiche grün)"),
            ("P", "Proof: Bild wie vom Ausgabe-Rezept geschrieben"),
            ("O", "Hilfslinien wechseln (Drittel, Goldener Schnitt, Raster, Diagonalen)"),
            ("Umschalt+O", "Eigenes Overlay (Layout-PNG) ein/aus"),
            ("Umschalt+A", "Automatisch geraderichten (Drehung und stürzende Linien)"),
            ("Strg+Umschalt+R", "Auswahl umbenennen (mit Platzhaltern)"),
            ("F11 / Strg+Umschalt+F", "Vollbild ohne Ablenkung (Esc beendet)"),
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
        let title = if self.session.is_some() { "Bilder in die Session importieren" } else { "Ordner in Katalog importieren" };
        if let Some(dir) = rfd::FileDialog::new().set_title(title).set_directory(&self.folder).pick_folder() {
            // A session copies the files into its Capture folder; the catalog indexes them where they are.
            if self.session.is_none() {
                self.open_folder(dir.clone());
            }
            self.import_folder(&dir);
        }
    }

    // ------------------------------------------------------------ title bar

    /// Title bar: app icon, menus, window title and
    /// window buttons. The window has no system decorations, so this bar also moves it.
    pub fn title_bar(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let bar = ui.max_rect();
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));

        // The empty parts of the bar move the window; widgets added later take precedence.
        let drag = ui.interact(bar, ui.id().with("drag"), Sense::click_and_drag());
        if drag.double_clicked() {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        } else if drag.drag_started_by(PointerButton::Primary) {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }

        let buttons_left = bar.right() - 3.0 * WINDOW_BUTTON_WIDTH;
        let left = Rect::from_min_max(bar.min, pos2(buttons_left, bar.max.y));
        let menus_right = ui
            .scope_builder(UiBuilder::new().max_rect(left).layout(Layout::left_to_right(Align::Center)), |ui| {
                ui.add_space(10.0);
                ui.label(RichText::new(icon::APERTURE).size(17.0).color(theme::ACCENT));
                ui.add_space(4.0);
                self.menu_bar(ui)
            })
            .inner;

        // Window title: centered on the window like VS Code, shifted right if the menus are in the way.
        let font = FontId::proportional(12.5);
        let galley = ui.painter().layout_no_wrap(self.window_title.clone(), font, theme::TEXT_DIM);
        let w = galley.size().x;
        let x = (bar.center().x - w / 2.0).max(menus_right + 24.0);
        if x + w < buttons_left - 12.0 {
            ui.painter().galley(pos2(x, bar.center().y - galley.size().y / 2.0), galley, theme::TEXT_DIM);
        }

        let kinds = [WindowButton::Minimize, if maximized { WindowButton::Restore } else { WindowButton::Maximize }, WindowButton::Close];
        for (i, kind) in kinds.into_iter().enumerate() {
            let x = buttons_left + i as f32 * WINDOW_BUTTON_WIDTH;
            let rect = Rect::from_min_max(pos2(x, bar.min.y), pos2(x + WINDOW_BUTTON_WIDTH, bar.max.y));
            if window_button(ui, rect, kind).clicked() {
                ctx.send_viewport_cmd(match kind {
                    WindowButton::Minimize => ViewportCommand::Minimized(true),
                    WindowButton::Maximize | WindowButton::Restore => ViewportCommand::Maximized(!maximized),
                    WindowButton::Close => ViewportCommand::Close,
                });
            }
        }
    }

    /// The menus of the title bar; returns the right edge of the menu titles.
    fn menu_bar(&mut self, ui: &mut Ui) -> f32 {
        egui::MenuBar::new().ui(ui, |ui| {
            let (open, right) = bar_menus(ui);
            if let Some((i, button)) = open {
                let popup = Popup::menu(&button)
                    .id(menu_popup_id(ui, i))
                    .open_memory(None)
                    .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
                    .style(menu::menu_style)
                    .info(UiStackInfo::new(UiKind::Menu).with_tag_value(
                        MenuConfig::MENU_CONFIG_TAG,
                        MenuConfig::new().close_behavior(PopupCloseBehavior::CloseOnClickOutside),
                    ));
                popup.show(|ui| {
                    match i {
                        0 => self.file_menu(ui),
                        1 => self.edit_menu(ui),
                        2 => self.image_menu(ui),
                        3 => self.view_menu(ui),
                        _ => self.help_menu(ui),
                    }
                    close_on_activation(ui);
                });
            }
            right
        })
        .inner
    }

    fn file_menu(&mut self, ui: &mut Ui) {
        let n_sel = self.selected_paths().len();
        self.session_file_menu(ui);
        ui.separator();
        if item(ui, icon::FOLDER_OPEN, "Ordner öffnen …", "Strg+O", true) {
            self.open_folder_dialog();
        }
        let import = if self.session.is_some() { "In Session importieren …" } else { "Ordner in Katalog importieren …" };
        if item(ui, icon::FOLDER_PLUS, import, "", true) {
            self.import_folder_dialog();
        }
        ui.separator();
        if item(ui, icon::EXPORT, &format!("Verarbeiten ({})", image_count(n_sel)), "Strg+D", n_sel > 0) {
            self.export_selection();
        }
        ui.separator();
        if item(ui, icon::X, "Beenden", "Alt+F4", true) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn edit_menu(&mut self, ui: &mut Ui) {
        let has = self.current.is_some();
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
    }

    fn image_menu(&mut self, ui: &mut Ui) {
        let has = self.current.is_some();
        let n_sel = self.selected_paths().len();
        if item(ui, icon::ARROW_COUNTER_CLOCKWISE, "Links drehen", "Strg+[", has) {
            self.rotate(3);
        }
        if item(ui, icon::ARROW_CLOCKWISE, "Rechts drehen", "Strg+]", has) {
            self.rotate(1);
        }
        if item(ui, icon::CROP, "Zuschneiden", "C", has) {
            self.toggle_crop();
        }
        if item(ui, icon::PERSPECTIVE, "Automatisch geraderichten", "Umschalt+A", has) {
            self.auto_upright(sk_core::upright::UprightMode::Vertical);
        }
        if item(ui, icon::PENCIL_SIMPLE, "Umbenennen …", "Strg+Umschalt+R", n_sel > 0) {
            self.open_rename();
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
            ui.menu_button(format!("{}  XMP", icon::FILE_CODE), |ui| {
                if item(ui, icon::UPLOAD_SIMPLE, "Metadaten in XMP schreiben", "", true) {
                    self.write_xmp_selection();
                }
                if item(ui, icon::DOWNLOAD_SIMPLE, "Metadaten aus XMP lesen", "", true) {
                    self.read_xmp_selection();
                }
                ui.separator();
                let auto = sk_core::xmp::write_enabled();
                if check(ui, auto, "Änderungen automatisch in XMP schreiben", "") {
                    sk_core::xmp::set_write_enabled(!auto);
                    self.save_settings();
                }
            })
            .response
            .on_hover_text("Bewertung, Farb-Label und Keywords mit Lightroom, Bridge und Capture One austauschen");
        });
        ui.separator();
        self.variant_menu(ui);
        ui.separator();
        let ref_text = if self.compare_ref.is_some() && self.compare_ref == self.primary_path() { "Referenzbild entfernen" } else { "Als Referenzbild festlegen" };
        if item(ui, icon::PUSH_PIN, ref_text, "Umschalt+Enter", has) {
            self.toggle_reference();
        }
        let can_match = self.can_match();
        ui.add_enabled_ui(can_match, |ui| {
            ui.menu_button(format!("{}  An Referenz angleichen", icon::SCALES), |ui| {
                use sk_core::auto::MatchWhat;
                for (text, exposure, white_balance) in
                    [("Belichtung und Weißabgleich", true, true), ("Nur Belichtung", true, false), ("Nur Weißabgleich", false, true)]
                {
                    if ui.button(text).clicked() {
                        self.match_selection(MatchWhat { exposure, white_balance });
                    }
                }
            })
            .response
            .on_disabled_hover_text("Ein Referenzbild festlegen und weitere Bilder auswählen");
        });
        ui.separator();
        self.move_menu(ui);
        ui.separator();
        if item(ui, icon::CARET_LEFT, "Vorheriges Bild", icon::ARROW_LEFT, !self.visible.is_empty()) {
            self.step(-1);
        }
        if item(ui, icon::CARET_RIGHT, "Nächstes Bild", icon::ARROW_RIGHT, !self.visible.is_empty()) {
            self.step(1);
        }
    }

    fn view_menu(&mut self, ui: &mut Ui) {
        let has = self.current.is_some();
        if check(ui, self.grid_view, "Raster", "G") {
            self.grid_view = !self.grid_view;
        }
        if check(ui, self.show_filmstrip, "Filmstreifen", "") {
            self.show_filmstrip = !self.show_filmstrip;
        }
        if check(ui, self.show_navigator, "Navigator beim Zoomen", "") {
            self.show_navigator = !self.show_navigator;
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
        if check(ui, self.split_view && !self.split_side, "Geteilter Vergleich", "U") {
            self.toggle_split();
        }
        if check(ui, self.split_view && self.split_side, "Vorher | Nachher nebeneinander", "Umschalt+U") {
            self.toggle_side_by_side();
        }
        if check(ui, self.multi_view, "Auswahl nebeneinander vergleichen", "") {
            self.multi_view = !self.multi_view;
        }
        if self.compare_ref.is_some() && item(ui, icon::PUSH_PIN_SLASH, "Referenzbild entfernen", "", true) {
            self.compare_ref = None;
        }
        if check(ui, self.clip_warning, "Belichtungswarnung", "E") {
            self.toggle_clip_warning();
        }
        if check(ui, self.focus_mask, "Fokusmaske", "K") {
            self.toggle_focus_mask();
        }
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.label("Empfindlichkeit");
            ui.add(egui::Slider::new(&mut self.focus_sensitivity, 0.0..=100.0).fixed_decimals(0))
                .on_hover_text("Höher: auch weniger scharfe Bereiche markieren");
        });
        if check(ui, self.proof_view, "Proof (Ausgabe-Rezept)", "P") {
            self.toggle_proof();
        }
        ui.separator();
        ui.menu_button("Hilfslinien", |ui| {
            for g in crate::guides::Guides::ALL {
                if check(ui, self.guides == g, g.label(), if g == crate::guides::Guides::Off { "O" } else { "" }) {
                    self.guides = g;
                    self.save_settings();
                }
            }
        });
        if check(ui, self.overlay.shown, "Overlay anzeigen", "Umschalt+O") {
            self.toggle_overlay();
        }
        if item(ui, icon::IMAGE, "Overlay wählen …", "", true) {
            self.choose_overlay();
        }
        if let Some(e) = &self.overlay.error {
            ui.label(RichText::new(e).small().color(Color32::from_rgb(230, 110, 100)));
        }
        if self.overlay.file.is_some() {
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                ui.label("Deckkraft");
                if ui.add(egui::Slider::new(&mut self.overlay.opacity, 0.05..=1.0).fixed_decimals(2)).drag_stopped() {
                    self.save_settings();
                }
            });
        }
        ui.separator();
        if check(ui, self.fullscreen, "Vollbild ohne Ablenkung", "F11") {
            self.toggle_fullscreen(&ui.ctx().clone());
        }
        ui.separator();
        ui.menu_button(format!("{}  Werkzeug", icon::SIDEBAR_SIMPLE), |ui| {
            for tab in Tab::ALL {
                if ui.add(Button::selectable(self.tab == tab, format!("{}  {}", tab.icon(), tab.title()))).clicked() {
                    self.tab = tab;
                }
            }
        });
    }

    fn help_menu(&mut self, ui: &mut Ui) {
        if item(ui, icon::KEYBOARD, "Tastenkürzel", "F1", true) {
            self.show_shortcuts = true;
        }
        ui.separator();
        if item(ui, icon::INFO, "Über Silberkorn", "", true) {
            self.splash = Some(crate::splash::Splash::about());
        }
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
            if tool(ui, icon::SQUARE_SPLIT_HORIZONTAL, "Geteilter Vergleich (U)", self.split_view && !self.split_side, has).clicked() {
                self.toggle_split();
            }
            if tool(ui, icon::COLUMNS, "Vorher | Nachher nebeneinander (Umschalt+U)", self.split_view && self.split_side, has).clicked() {
                self.toggle_side_by_side();
            }
            if tool(ui, icon::WARNING, "Belichtungswarnung (E)", self.clip_warning, has).clicked() {
                self.toggle_clip_warning();
            }
            if tool(ui, icon::CROSSHAIR, "Fokusmaske: scharfe Bereiche markieren (K)", self.focus_mask, has).clicked() {
                self.toggle_focus_mask();
            }
            if tool(ui, icon::FILE_MAGNIFYING_GLASS, "Proof: wie vom Ausgabe-Rezept geschrieben (P)", self.proof_view, has).clicked() {
                self.toggle_proof();
            }
            if tool(ui, icon::ARROWS_OUT, "Vollbild ohne Ablenkung (F11)", false, has).clicked() {
                self.toggle_fullscreen(&ui.ctx().clone());
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
                let name = sk_core::variant::display_name(&cur.meta.path);
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
                let count = image_count(self.visible.len());
                ui.label(dim(format!("{}  ·  {count}{sel}", scope_label(&self.scope, &self.lists, self.session.as_ref()))));
                if let Some((done, total)) = self.export_progress.or(self.index_progress) {
                    ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32).desired_width(120.0).desired_height(12.0));
                } else if let Some((done, total)) = self.proxy_progress {
                    ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32).desired_width(80.0).desired_height(8.0))
                        .on_hover_text(format!("Vorschauen werden im Hintergrund erstellt ({done} von {total})"));
                }
            });
        });
    }

    // ------------------------------------------------------------ dialogs

    pub fn dialogs(&mut self, ctx: &egui::Context) {
        self.rename_dialog(ctx);
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
    }
}

/// Titles of the top-level menus, in bar order (see `App::menu_bar`).
const MENUS: [&str; 5] = ["Datei", "Bearbeiten", "Bild", "Ansicht", "Hilfe"];

fn menu_popup_id(ui: &Ui, i: usize) -> Id {
    ui.id().with(("bar_menu", i))
}

/// Draws the menu titles and returns the menu that is open this frame together with its
/// title button, plus the right edge of the titles. Behaves like a native menu bar: a click opens or closes a menu, while one
/// is open hovering another title switches to it, and the left/right arrow keys step through the menus.
fn bar_menus(ui: &mut Ui) -> (Option<(usize, Response)>, f32) {
    let ctx = ui.ctx().clone();
    let ids: Vec<Id> = (0..MENUS.len()).map(|i| menu_popup_id(ui, i)).collect();
    let was_open = ids.iter().position(|&id| Popup::is_id_open(&ctx, id));
    let buttons: Vec<Response> =
        MENUS.iter().enumerate().map(|(i, title)| ui.add(Button::selectable(was_open == Some(i), *title))).collect();

    // Only a moving pointer switches menus, so a resting one doesn't undo the arrow keys.
    let moved = ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO);
    let mut open = was_open;
    for (i, b) in buttons.iter().enumerate() {
        if b.clicked() {
            open = if was_open == Some(i) { None } else { Some(i) };
        } else if was_open.is_some() && moved && b.hovered() {
            open = Some(i);
        }
    }
    if let Some(i) = open {
        let n = MENUS.len();
        if ui.input_mut(|inp| inp.consume_key(Modifiers::NONE, Key::ArrowRight)) {
            open = Some((i + 1) % n);
        } else if ui.input_mut(|inp| inp.consume_key(Modifiers::NONE, Key::ArrowLeft)) {
            open = Some((i + n - 1) % n);
        }
    }
    if open != was_open {
        match open {
            Some(i) => Popup::open_id(&ctx, ids[i]),
            None => Popup::close_all(&ctx),
        }
        ctx.request_repaint();
    }
    let right = buttons.last().map_or(ui.min_rect().left(), |b| b.rect.right());
    (open.map(|i| (i, buttons[i].clone())), right)
}

/// Closes the open menu when one of its entries was clicked. Clicks on disabled entries,
/// separators and submenu titles keep it open, as in native menus.
fn close_on_activation(ui: &Ui) {
    let ctx = ui.ctx();
    let Some(id) = ctx.interaction_snapshot(|s| s.clicked) else { return };
    // A widget inside a menu, not the menu's own background (which catches clicks on disabled entries).
    let in_menu = ctx.read_response(id).is_some_and(|r| {
        let area = r.layer_id.id;
        r.layer_id.order == Order::Foreground && id != area && id != area.with("move")
    });
    let submenu = ctx.data(|d| d.get_temp::<MenuState>(SubMenu::id_from_widget_id(id).with(MenuState::ID)).is_some());
    if in_menu && !submenu {
        ui.close();
    }
}

/// Height of the title bar, see `App::title_bar`.
pub const TITLE_BAR_HEIGHT: f32 = 32.0;
/// Width of the minimize, maximize and close buttons (as on Windows).
const WINDOW_BUTTON_WIDTH: f32 = 46.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum WindowButton {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// Window button with a Windows-style line glyph; close turns red on hover.
fn window_button(ui: &mut Ui, rect: Rect, kind: WindowButton) -> Response {
    let resp = ui.interact(rect, ui.id().with(kind), Sense::click());
    let close = kind == WindowButton::Close;
    let pressed = resp.is_pointer_button_down_on();
    let bg = match (close, pressed, resp.hovered()) {
        (true, true, _) => Some(Color32::from_rgb(148, 30, 22)),
        (true, false, true) => Some(Color32::from_rgb(196, 43, 28)),
        (false, true, _) => Some(Color32::from_white_alpha(22)),
        (false, false, true) => Some(Color32::from_white_alpha(14)),
        _ => None,
    };
    let p = ui.painter();
    if let Some(bg) = bg {
        p.rect_filled(rect, 0.0, bg);
    }
    let fg = if close && resp.hovered() { Color32::WHITE } else { theme::TEXT };
    let stroke = Stroke::new(1.0, fg);
    // Glyphs on pixel centers so the 1 px lines stay crisp.
    let c = pos2(p.round_to_pixel_center(rect.center().x), p.round_to_pixel_center(rect.center().y));
    let at = |dx: f32, dy: f32| c + vec2(dx, dy);
    match kind {
        WindowButton::Minimize => {
            p.line_segment([at(-5.0, 0.0), at(5.0, 0.0)], stroke);
        }
        WindowButton::Maximize => {
            p.rect_stroke(Rect::from_min_max(at(-5.0, -5.0), at(5.0, 5.0)), 0.0, stroke, StrokeKind::Middle);
        }
        WindowButton::Restore => {
            p.rect_stroke(Rect::from_min_max(at(-5.0, -3.0), at(3.0, 5.0)), 0.0, stroke, StrokeKind::Middle);
            p.add(egui::Shape::line(vec![at(-3.0, -3.0), at(-3.0, -5.0), at(5.0, -5.0), at(5.0, 3.0), at(3.0, 3.0)], stroke));
        }
        WindowButton::Close => {
            p.line_segment([at(-5.0, -5.0), at(5.0, 5.0)], stroke);
            p.line_segment([at(-5.0, 5.0), at(5.0, -5.0)], stroke);
        }
    }
    resp
}

/// Invisible resize handles along the window edges, needed because the window has no
/// system frame. Call after all panels so the handles take precedence over their widgets.
pub fn resize_edges(ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    if ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)) {
        return;
    }
    let r = ctx.content_rect();
    // Thin border so the window stays distinguishable from what lies behind it.
    ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("window_border")))
        .rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_rgb(62, 62, 66)), StrokeKind::Inside);

    const EDGE: f32 = 5.0;
    const CORNER: f32 = 12.0;
    use egui::{CursorIcon as C, ResizeDirection as D};
    let x = |a: f32, b: f32| [a, b];
    let edges = [
        (D::North, x(r.left(), r.right()), x(r.top(), r.top() + EDGE), C::ResizeNorth),
        (D::South, x(r.left(), r.right()), x(r.bottom() - EDGE, r.bottom()), C::ResizeSouth),
        (D::West, x(r.left(), r.left() + EDGE), x(r.top(), r.bottom()), C::ResizeWest),
        (D::East, x(r.right() - EDGE, r.right()), x(r.top(), r.bottom()), C::ResizeEast),
        // Corners last so they win over the edges.
        (D::NorthWest, x(r.left(), r.left() + CORNER), x(r.top(), r.top() + CORNER), C::ResizeNorthWest),
        (D::NorthEast, x(r.right() - CORNER, r.right()), x(r.top(), r.top() + CORNER), C::ResizeNorthEast),
        (D::SouthWest, x(r.left(), r.left() + CORNER), x(r.bottom() - CORNER, r.bottom()), C::ResizeSouthWest),
        (D::SouthEast, x(r.right() - CORNER, r.right()), x(r.bottom() - CORNER, r.bottom()), C::ResizeSouthEast),
    ];
    for (i, (dir, [x0, x1], [y0, y1], cursor)) in edges.into_iter().enumerate() {
        let rect = Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
        let resp = ui.interact(rect, Id::new(("resize_edge", i)), Sense::drag());
        if resp.hovered() || resp.is_pointer_button_down_on() {
            ctx.set_cursor_icon(cursor);
        }
        if resp.is_pointer_button_down_on() && ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
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
