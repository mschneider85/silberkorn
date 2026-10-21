//! Window chrome: title bar with menus, toolbar, tool tab rail, status bar and the help dialogs.

use egui::containers::menu::{self, MenuConfig, MenuState, SubMenu};
use egui::{
    Align, Button, Color32, FontId, Id, Key, Layout, Margin, Modifiers, Order, PointerButton, Popup, PopupCloseBehavior,
    Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder, UiKind, UiStackInfo, ViewportCommand, pos2, vec2,
};
use sk_core::params::Adjustments;
use sk_core::sidecar::ColorLabel;

use crate::app::{App, Tab};
use crate::commands::shortcut_of as shortcut;
use crate::library::scope_label;
use crate::theme::{self, icon};
use crate::widgets::image_count;

impl App {
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || !self.extra_undo.is_empty() || self.current.as_ref().is_some_and(|c| c.adjustments != c.committed)
    }

    pub fn rotate(&mut self, quarter_turns: u8) {
        self.edit(|a| a.rotation = (a.rotation + quarter_turns) % 4);
    }

    pub fn reset_adjustments(&mut self) {
        // As a new image starts: with the camera's look, if new images get it.
        let looked = self.current.as_ref().and_then(|c| {
            sk_core::camera_look::apply_if_wanted(&c.meta, &c.preview, &Adjustments::default(), sk_core::camera_look::for_new())
        });
        self.edit(|a| *a = looked.clone().unwrap_or_default());
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
        let start = self.import_source.clone().unwrap_or_else(|| self.folder.clone());
        if let Some(dir) = rfd::FileDialog::new().set_title("Quelle wählen").set_directory(&start).pick_folder() {
            self.open_import(dir, true);
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
                let (rect, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                ui.painter().image(app_icon(ui.ctx()).id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), egui::Color32::WHITE);
                ui.add_space(6.0);
                self.menu_bar(ui)
            })
            .inner;

        // Search field with the window title, centered like in VS Code; it opens the command
        // palette. Shifted right if the menus are in the way.
        let w = 380.0f32.min(buttons_left - menus_right - 48.0);
        if w > 160.0 {
            let x = (bar.center().x - w / 2.0).max(menus_right + 24.0);
            let rect = Rect::from_min_size(pos2(x, bar.center().y - 11.0), vec2(w, 22.0));
            let resp = ui.interact(rect, ui.id().with("search"), Sense::click()).on_hover_text("Befehlssuche: Befehl, Werkzeug oder Einstellung suchen (Strg+K)");
            let p = ui.painter();
            let fill = if resp.hovered() { Color32::from_rgb(48, 48, 53) } else { Color32::from_rgb(38, 38, 42) };
            p.rect(rect, 6.0, fill, Stroke::new(1.0, if resp.hovered() { Color32::from_rgb(70, 70, 77) } else { theme::BORDER }), StrokeKind::Inside);
            p.text(rect.left_center() + vec2(12.0, 0.0), egui::Align2::CENTER_CENTER, icon::MAGNIFYING_GLASS, FontId::proportional(13.0), theme::TEXT_DIM);
            let title = p.layout_no_wrap(self.window_title.clone(), FontId::proportional(12.5), theme::TEXT_DIM);
            let key = p.layout_no_wrap("Strg+K".into(), FontId::proportional(11.0), theme::TEXT_FAINT);
            if title.size().x + key.size().x + 60.0 < w {
                p.galley(pos2(rect.center().x - title.size().x / 2.0, rect.center().y - title.size().y / 2.0), title, theme::TEXT_DIM);
            }
            p.galley(pos2(rect.right() - key.size().x - 9.0, rect.center().y - key.size().y / 2.0), key, theme::TEXT_FAINT);
            if resp.clicked() {
                self.palette.show();
            }
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
                        2 => self.photo_menu(ui),
                        3 => self.develop_menu(ui),
                        4 => self.view_menu(ui),
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
        let n_sel = self.selected_count();
        self.session_file_menu(ui);
        ui.separator();
        self.command_item(ui, "open-folder");
        let mut watch = self.hot_folder.on;
        if crate::widgets::checkbox(ui, &mut watch, "Ordner überwachen").on_hover_text("Neue Bilder, etwa aus der Kamerasoftware, erscheinen von selbst").changed() {
            self.hot_folder.on = watch;
            self.save_settings();
        }
        let import = if self.session.is_some() { "Bilder in die Session importieren …" } else { "Bilder in den Katalog importieren …" };
        if item(ui, icon::FOLDER_PLUS, import, "", true) {
            self.import_folder_dialog();
        }
        ui.separator();
        self.command_item_labeled(ui, "export", &format!("Exportieren ({})", image_count(n_sel)));
        ui.separator();
        if item(ui, icon::X, "Beenden", "Alt+F4", true) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn edit_menu(&mut self, ui: &mut Ui) {
        self.command_item(ui, "undo");
        self.command_item(ui, "redo");
        ui.separator();
        self.command_item(ui, "copy-adjustments");
        self.command_item(ui, "copy-adjustments-choose");
        self.command_item_labeled(ui, "paste-adjustments", "Anpassungen einfügen");
        self.command_item(ui, "reset");
        ui.separator();
        self.command_item(ui, "select-all");
        ui.separator();
        self.command_item(ui, "gpu");
    }

    /// Organizing: rating, labels, metadata, names, variants, stacks, where the files are.
    /// "Bewertung" and "Farb-Label" submenus for the selection (photo menu and thumbnails).
    pub(crate) fn rating_label_menus(&mut self, ui: &mut Ui) {
        ui.menu_button(format!("{}  Bewertung", icon::STAR), |ui| {
            for r in 0..=5u8 {
                let text = if r == 0 { "Keine".to_string() } else { "★".repeat(r as usize) };
                if ui.add(Button::new(text).shortcut_text(shortcut(&format!("rating-{r}")))).clicked() {
                    self.set_rating(r);
                }
            }
        });
        ui.menu_button(format!("{}  Farb-Label", icon::TAG), |ui| {
            for (l, id) in [
                (ColorLabel::None, "label-none"),
                (ColorLabel::Red, "label-red"),
                (ColorLabel::Yellow, "label-yellow"),
                (ColorLabel::Green, "label-green"),
                (ColorLabel::Blue, "label-blue"),
            ] {
                if ui.add(Button::new(l.label()).shortcut_text(shortcut(id))).clicked() {
                    self.set_color_label(l);
                }
            }
        });
    }

    fn photo_menu(&mut self, ui: &mut Ui) {
        let n_sel = self.selected_count();
        ui.add_enabled_ui(n_sel > 0, |ui| {
            self.rating_label_menus(ui);
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
        self.command_item(ui, "rename");
        ui.separator();
        self.variant_menu(ui);
        self.stack_menu(ui);
        ui.separator();
        self.move_menu(ui);
        ui.separator();
        self.command_item(ui, "previous");
        self.command_item(ui, "next");
    }

    /// Developing: geometry, white balance, noise, merging, matching to a reference.
    fn develop_menu(&mut self, ui: &mut Ui) {
        self.command_item(ui, "rotate-left");
        self.command_item(ui, "rotate-right");
        self.command_item(ui, "crop");
        self.command_item(ui, "straighten");
        self.command_item(ui, "auto-upright");
        ui.separator();
        self.command_item(ui, "auto-adjust");
        self.command_item_labeled(ui, "wb-picker", "Weißabgleich-Pipette");
        self.command_item(ui, "denoise");
        ui.separator();
        self.merge_menu(ui);
        ui.separator();
        let ref_text = if self.compare_ref.is_some() && self.compare_ref == self.primary_path() { "Referenzbild entfernen" } else { "Referenzbild festlegen" };
        self.command_item_labeled(ui, "reference", ref_text);
        if self.compare_ref.is_some() && self.compare_ref != self.primary_path() && item(ui, icon::PUSH_PIN_SLASH, "Referenzbild entfernen", "", true) {
            self.compare_ref = None;
        }
        self.command_item(ui, "match-look");
        self.command_item(ui, "look-add-image");
        self.command_item(ui, "look-add-reference");
        self.command_item(ui, "look-add-file");
        ui.separator();
        let process = self.current.as_ref().map(|c| c.adjustments.process);
        let label = match process {
            Some(p) if p < sk_core::params::PROCESS_LATEST => format!("Auf aktuelle Prozessversion heben (dieses Bild: {p})"),
            _ => "Auf aktuelle Prozessversion heben".to_string(),
        };
        self.command_item_labeled(ui, "process-update", &label);
    }

    fn view_menu(&mut self, ui: &mut Ui) {
        if check(ui, self.grid_view, "Raster", &shortcut("grid")) {
            self.grid_view = !self.grid_view;
        }
        if check(ui, self.show_filmstrip, "Filmstreifen", "") {
            self.show_filmstrip = !self.show_filmstrip;
        }
        if check(ui, self.show_navigator, "Navigator beim Zoomen", "") {
            self.show_navigator = !self.show_navigator;
        }
        ui.separator();
        for m in crate::side_column::SideColumn::ALL {
            if check(ui, self.side_column == m, m.label(), "") {
                self.side_column = m;
                self.save_settings();
            }
        }
        ui.separator();
        if check(ui, self.zoom.is_none(), "Einpassen", "") {
            self.zoom_fit();
        }
        self.command_item_labeled(ui, "zoom-100", "100 %");
        ui.separator();
        if check(ui, self.show_before, "Vorher / Nachher", &shortcut("before")) {
            self.toggle_before();
        }
        if check(ui, self.split_view && !self.split_side, "Geteilter Vergleich", &shortcut("split")) {
            self.toggle_split();
        }
        if check(ui, self.split_view && self.split_side, "Vorher | Nachher nebeneinander", &shortcut("side-by-side")) {
            self.toggle_side_by_side();
        }
        if check(ui, self.multi_view, "Auswahl nebeneinander vergleichen", "") {
            self.multi_view = !self.multi_view;
        }
        if check(ui, self.clip_warning, "Belichtungswarnung", &shortcut("clip-warning")) {
            self.toggle_clip_warning();
        }
        if check(ui, self.focus_mask, "Schärfemaske", &shortcut("focus-mask")) {
            self.toggle_focus_mask();
        }
        // The same slider as in the panels; without the mask it has no effect.
        ui.add_enabled_ui(self.focus_mask, |ui| {
            ui.set_min_width(260.0);
            crate::widgets::slider_row(ui, "Empfindlichkeit", &mut self.focus_sensitivity, 0.0..=100.0, 50.0, 0);
        })
        .response
        .on_hover_text("Höher: auch weniger scharfe Bereiche markieren");
        if check(ui, self.proof_view, "Ausgabevorschau", &shortcut("proof")) {
            self.toggle_proof();
        }
        if check(ui, self.softproof.on, "Softproof (Profil im Tab „Ausgabe“)", &shortcut("softproof")) {
            self.toggle_softproof();
        }
        if check(ui, self.print.on, "Druckvorschau (Format im Tab „Ausgabe“)", &shortcut("print-preview")) {
            self.toggle_print_preview();
        }
        if check(ui, self.print.zoom, "Druckgröße", &shortcut("print-zoom")) {
            self.toggle_print_zoom();
        }
        ui.separator();
        ui.menu_button("Hilfslinien", |ui| {
            for g in crate::guides::Guides::ALL {
                if check(ui, self.guides == g, g.label(), &if g == crate::guides::Guides::Off { shortcut("guides-next") } else { String::new() }) {
                    self.guides = g;
                    self.save_settings();
                }
            }
        });
        ui.separator();
        if check(ui, self.viewer_window.open, "Bild in eigenem Fenster", &shortcut("viewer-window")) {
            self.toggle_viewer_window();
        }
        if check(ui, self.fullscreen, "Vollbild ohne Ablenkung", &shortcut("fullscreen")) {
            self.toggle_fullscreen(&ui.ctx().clone());
        }
    }

    fn help_menu(&mut self, ui: &mut Ui) {
        if item(ui, icon::MAGNIFYING_GLASS, "Befehlssuche …", "Strg+K", true) {
            self.palette.show();
        }
        self.command_item(ui, "shortcuts");
        ui.separator();
        self.command_item(ui, "update");
        self.command_item(ui, "licenses");
        self.command_item(ui, "about");
    }

    // ------------------------------------------------------------ toolbar

    /// Toolbar in groups: browsing and the view mode on the left, editing in the middle,
    /// viewing aids, zoom and export on the right.
    pub fn toolbar(&mut self, ui: &mut Ui) {
        let has = self.current.is_some();
        // What changes the whole image is unavailable while a layer is chosen.
        let on_layer = self.on_layer();
        let image = has && !on_layer;
        let image_hover = |hover: &str| if on_layer { format!("{hover}\nWirkt nur aufs ganze Bild – erst den Hintergrund wählen") } else { hover.to_string() };
        let ctx = ui.ctx().clone();
        // Main image tools get a label when there is room for it.
        let wide = ui.available_width() > 1350.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if tool(ui, icon::FOLDER_OPEN, &crate::commands::hint("Ordner öffnen", "open-folder"), false, true).clicked() {
                self.open_folder_dialog();
            }
            let import = if self.session.is_some() { "Bilder in die Session importieren …" } else { "Bilder in den Katalog importieren …" };
            if tool(ui, icon::FOLDER_PLUS, import, false, true).clicked() {
                self.import_folder_dialog();
            }
            group_gap(ui);
            let modes = [
                Seg { icon: icon::SQUARES_FOUR, label: "Raster", hover: &crate::commands::hint("Raster: alle Bilder im Überblick", "grid"), active: self.grid_view, enabled: true },
                Seg { icon: icon::IMAGE, label: "Einzelbild", hover: &crate::commands::hint("Einzelbild: ein Bild groß bearbeiten", "grid"), active: !self.grid_view, enabled: true },
            ];
            if let Some(i) = segmented(ui, &modes) {
                self.grid_view = i == 0;
            }
            group_gap(ui);
            if tool(ui, icon::ARROW_U_UP_LEFT, &crate::commands::hint("Rückgängig", "undo"), false, self.can_undo()).clicked() {
                self.undo();
            }
            if tool(ui, icon::ARROW_U_UP_RIGHT, &crate::commands::hint("Wiederholen", "redo"), false, self.can_redo()).clicked() {
                self.redo();
            }
            group_gap(ui);
            if tool(ui, icon::ARROW_COUNTER_CLOCKWISE, &image_hover(&crate::commands::hint("Links drehen", "rotate-left")), false, image).clicked() {
                self.rotate(3);
            }
            if tool(ui, icon::ARROW_CLOCKWISE, &image_hover(&crate::commands::hint("Rechts drehen", "rotate-right")), false, image).clicked() {
                self.rotate(1);
            }
            let crop_hover = format!("{}, {}", crate::commands::hint("Zuschneiden", "crop"), crate::commands::hint("übernehmen", "finish-crop"));
            if labeled_tool(ui, wide, icon::CROP, "Zuschneiden", &image_hover(&crop_hover), self.crop_tool.is_some(), image).clicked() {
                self.toggle_crop();
            }
            if labeled_tool(ui, wide, icon::RULER, "Geraderichten", &image_hover("Geraderichten: eine Linie entlang des Horizonts ziehen"), self.straightening(), image).clicked() {
                self.toggle_straighten();
            }
            let wb_hover = crate::commands::hint("Weißabgleich-Pipette: auf eine neutrale Stelle klicken", "wb-picker");
            if labeled_tool(ui, wide, icon::EYEDROPPER, "Weißabgleich", &image_hover(&wb_hover), self.picking_wb, image).clicked() {
                self.toggle_wb_picker();
            }
            group_gap(ui);
            let copy = tool(ui, icon::COPY, &format!("{}\nRechtsklick: Werkzeuge auswählen", crate::commands::hint("Anpassungen kopieren", "copy-adjustments")), false, has);
            if copy.clicked() {
                self.copy_adjustments();
            }
            if copy.secondary_clicked() {
                self.open_copy_dialog();
            }
            let paste_hover = image_hover(&crate::commands::hint("Anpassungen einfügen", "paste-adjustments"));
            if tool(ui, icon::CLIPBOARD_TEXT, &paste_hover, false, self.clipboard.is_some() && !on_layer).clicked() {
                self.paste_adjustments();
            }
            if tool(ui, icon::ARROWS_COUNTER_CLOCKWISE, &image_hover("Alle Anpassungen zurücksetzen"), false, image).clicked() {
                self.reset_adjustments();
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let n = self.selected_count();
                let text = RichText::new(format!("{}  Exportieren", icon::EXPORT)).color(Color32::WHITE).strong();
                let button = Button::new(text).fill(theme::ACCENT).corner_radius(6).min_size(vec2(0.0, 28.0));
                let resp = ui.add_enabled(self.command_available("export"), button).on_disabled_hover_text(self.export_blocked_reason());
                if resp.on_hover_text(crate::commands::hint(&format!("{} mit den gewählten Rezepten exportieren", image_count(n)), "export")).clicked() {
                    self.export_selection();
                }
                ui.add_space(6.0);
                if tool(ui, icon::ARROWS_OUT, &crate::commands::hint("Vollbild ohne Ablenkung", "fullscreen"), false, has).clicked() {
                    self.toggle_fullscreen(&ctx);
                }
                group_gap(ui);
                // Right to left: the segments are listed in reverse.
                // 100 % only when it is 100 % (zoomed otherwise, neither segment is on).
                let at_100 = self.at_zoom_100();
                let zoom = [
                    Seg { icon: icon::MAGNIFYING_GLASS_PLUS, label: "", hover: &crate::commands::hint("100 % (auch Doppelklick ins Bild)", "zoom-100"), active: at_100, enabled: has },
                    Seg { icon: icon::CORNERS_OUT, label: "", hover: "Einpassen", active: self.zoom.is_none(), enabled: has },
                ];
                match segmented(ui, &zoom) {
                    Some(0) => self.zoom_to_100(),
                    Some(_) => self.zoom_fit(),
                    None => {}
                }
                group_gap(ui);
                let guides_on = self.guides != crate::guides::Guides::Off;
                let resp = tool(ui, icon::GRID_NINE, &crate::commands::hint("Hilfslinien", "guides-next"), guides_on, true);
                Popup::menu(&resp).show(|ui| {
                    for g in crate::guides::Guides::ALL {
                        if check(ui, self.guides == g, g.label(), "") {
                            self.guides = g;
                            self.save_settings();
                        }
                    }
                });
                if tool(ui, icon::FILE_MAGNIFYING_GLASS, &crate::commands::hint("Ausgabevorschau: so, wie das Ausgaberezept das Bild schreibt", "proof"), self.proof_view, has).clicked() {
                    self.toggle_proof();
                }
                if tool(ui, icon::CROSSHAIR, &crate::commands::hint("Schärfemaske: scharfe Bereiche markieren", "focus-mask"), self.focus_mask, has).clicked() {
                    self.toggle_focus_mask();
                }
                if tool(ui, icon::WARNING, &crate::commands::hint("Belichtungswarnung: ausgefressene und abgesoffene Stellen", "clip-warning"), self.clip_warning, has).clicked() {
                    self.toggle_clip_warning();
                }
                group_gap(ui);
                let compare = [
                    Seg { icon: icon::COLUMNS, label: "", hover: &crate::commands::hint("Vorher | Nachher nebeneinander", "side-by-side"), active: self.split_view && self.split_side, enabled: has },
                    Seg { icon: icon::SQUARE_SPLIT_HORIZONTAL, label: "", hover: &crate::commands::hint("Geteilter Vergleich mit Trennlinie", "split"), active: self.split_view && !self.split_side, enabled: has },
                    Seg { icon: icon::CIRCLE_HALF, label: "", hover: &crate::commands::hint("Vorher / Nachher", "before"), active: self.show_before, enabled: has },
                ];
                match segmented(ui, &compare) {
                    Some(0) => self.toggle_side_by_side(),
                    Some(1) => self.toggle_split(),
                    Some(_) => self.toggle_before(),
                    None => {}
                }
            });
        });
    }

    // ------------------------------------------------------------ tool tabs

    /// Vertical strip of tool tab icons, in groups.
    pub fn tab_rail(&mut self, ui: &mut Ui) {
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add_space(6.0);
            let mut group = Tab::ALL[0].group();
            let on_layer = self.on_layer();
            for tab in Tab::ALL {
                // On a layer, the tabs that only change the whole image are disabled, those that
                // work on it get a dot in the accent color.
                let available = self.tab_available(tab);
                if tab.group() != group {
                    group = tab.group();
                    ui.add_space(4.0);
                    let (r, _) = ui.allocate_exact_size(vec2(22.0, 1.0), Sense::hover());
                    ui.painter().rect_filled(r, 0.0, theme::BORDER);
                    ui.add_space(4.0);
                }
                let selected = self.tab == tab;
                let (rect, resp) = ui.allocate_exact_size(vec2(60.0, 46.0), if available { Sense::click() } else { Sense::hover() });
                let painter = ui.painter();
                let t = ui.ctx().animate_bool_with_time(resp.id, selected, 0.12);
                if resp.hovered() && !selected && available {
                    painter.rect_filled(rect, 7.0, Color32::from_rgb(38, 38, 42));
                }
                if t > 0.0 {
                    let bar = egui::Rect::from_center_size(pos2(rect.left() - 2.0, rect.center().y), vec2(3.0, 24.0 * t));
                    painter.rect_filled(bar, 1.5, theme::ACCENT);
                }
                let color = if !available {
                    theme::TEXT_FAINT.gamma_multiply(0.6)
                } else if selected {
                    Color32::WHITE
                } else if resp.hovered() {
                    theme::TEXT
                } else {
                    theme::TEXT_DIM
                };
                let symbol = painter.layout_no_wrap(tab.icon().to_string(), egui::FontId::proportional(19.0), color);
                let pos = rect.center() - vec2(0.0, 7.0) - symbol.size() / 2.0;
                if t > 0.0 {
                    // The selected tab's symbol glows in the accent color, like neon.
                    theme::paint_glow(painter, pos, &symbol, 6.0, t);
                }
                let symbol_rect = Rect::from_min_size(pos, symbol.size());
                painter.galley(pos, symbol, color);
                if on_layer && tab.on_layers() {
                    painter.circle_filled(symbol_rect.right_top() + vec2(2.0, 1.0), 3.0, theme::ACCENT);
                }
                painter.text(rect.center_bottom() - vec2(0.0, 9.0), egui::Align2::CENTER_CENTER, tab.short_title(), egui::FontId::proportional(10.0), color);
                let key = crate::palette::tab_shortcut(tab);
                let mut hover = if key.is_empty() { tab.title().to_string() } else { format!("{}  ({key})", tab.title()) };
                if !available {
                    hover = format!("{hover}\n{}", crate::widgets::ONLY_IMAGE);
                } else if on_layer && tab.on_layers() {
                    hover = format!("{hover}\nWirkt auf die gewählte Ebene");
                }
                if resp.on_hover_text(hover).clicked() {
                    self.tab = tab;
                }
            }
        });
    }

    pub fn tool_panel(&mut self, ui: &mut Ui) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(RichText::new(self.tab.icon()).size(17.0).color(theme::ACCENT));
            ui.label(RichText::new(self.tab.title()).size(15.0).strong().color(Color32::WHITE));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(4.0);
                let b = Button::new(RichText::new(icon::MAGNIFYING_GLASS).size(14.0).color(theme::TEXT_DIM)).frame_when_inactive(false);
                if ui.add(b).on_hover_text("Befehlssuche: Befehl, Werkzeug oder Einstellung suchen (Strg+K)").clicked() {
                    self.palette.show();
                }
            });
        });
        ui.add_space(8.0);
        // The tabs whose tools also work on layers (the layer tab has its list).
        if matches!(self.tab, Tab::Exposure | Tab::Color | Tab::Details) {
            self.edit_target_bar(ui);
        } else {
            self.peek_mask = false;
        }
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
            // An error is red, and stays for a while next to the messages that follow it.
            const ERROR_SECS: f32 = 12.0;
            let error = self.last_error.as_ref().filter(|(msg, at)| *msg == self.status || at.elapsed().as_secs_f32() < ERROR_SECS).map(|(msg, at)| (msg.clone(), *at));
            if let Some((msg, at)) = &error {
                ui.separator();
                ui.label(RichText::new(format!("{}  {msg}", icon::WARNING)).size(12.0).color(theme::DANGER));
                if *msg != self.status {
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs_f32((ERROR_SECS - at.elapsed().as_secs_f32()).max(0.1)));
                }
            }
            if !self.status.is_empty() && error.as_ref().is_none_or(|(msg, _)| *msg != self.status) {
                ui.separator();
                ui.label(dim(self.status.clone()));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(folder) = self.watched_folder().filter(|_| self.hot_folder.on) {
                    let name = folder.file_name().map_or_else(|| folder.display().to_string(), |n| n.to_string_lossy().to_string());
                    ui.label(RichText::new(format!("{}  Überwacht: {name}", icon::EYE)).size(12.0).color(theme::ACCENT))
                        .on_hover_text(format!("Neue Bilder in {} erscheinen von selbst", folder.display()));
                    ui.separator();
                }
                if self.grid_view {
                    let (lo, hi) = crate::app::GRID_THUMB;
                    ui.label(RichText::new(icon::IMAGE).size(14.0).color(theme::TEXT_DIM));
                    let changed = crate::widgets::compact_slider(ui, "Vorschaugröße", &mut self.grid_thumb, lo..=hi, 190.0, 90.0);
                    ui.label(RichText::new(icon::IMAGE).size(11.0).color(theme::TEXT_DIM));
                    if changed {
                        self.save_settings();
                    }
                    ui.separator();
                }
                if let Some(pct) = self.zoom_pct.filter(|_| !self.grid_view && self.current.is_some()) {
                    ui.label(RichText::new(format!("{pct:.0} %")).size(12.0).color(theme::TEXT));
                    ui.separator();
                }
                let n = self.selected_count();
                let sel = if n > 1 { format!("  ·  {n} ausgewählt") } else { String::new() };
                let count = image_count(self.visible.len());
                ui.label(dim(format!("{}  ·  {count}{sel}", scope_label(&self.scope, &self.lists, self.session.as_ref()))));
                self.update_status(ui);
                if !self.activities().is_empty() {
                    ui.separator();
                    self.activity_status(ui);
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
        self.import_dialog(ctx);
        self.gpu_dialog(ctx);
        self.licenses_dialog(ctx);
        self.update_dialog(ctx);
        self.copy_dialog(ctx);
        self.shortcut_editor(ctx);
        self.deletion_dialog(ctx);
    }
}

/// Titles of the top-level menus, in bar order (see `App::menu_bar`).
const MENUS: [&str; 6] = ["Datei", "Bearbeiten", "Foto", "Entwickeln", "Ansicht", "Hilfe"];

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
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 28.0), if enabled { Sense::click() } else { Sense::hover() });
    paint_tool(ui, rect, &resp, ic, "", active, enabled, 7.0);
    resp.on_hover_text(hover)
}

/// [`tool`] with its name next to the icon when `with_label`.
fn labeled_tool(ui: &mut Ui, with_label: bool, ic: &str, label: &str, hover: &str, active: bool, enabled: bool) -> egui::Response {
    if !with_label {
        return tool(ui, ic, hover, active, enabled);
    }
    let w = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(13.0), Color32::WHITE).size().x;
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 42.0, 28.0), if enabled { Sense::click() } else { Sense::hover() });
    paint_tool(ui, rect, &resp, ic, label, active, enabled, 7.0);
    resp.on_hover_text(hover)
}

/// One segment of a [`segmented`] control.
struct Seg<'a> {
    icon: &'a str,
    /// Text after the icon; empty for an icon-only segment.
    label: &'a str,
    hover: &'a str,
    active: bool,
    enabled: bool,
}

/// Buttons joined into one pill (mode switches and related toggles); returns the clicked one.
fn segmented(ui: &mut Ui, segs: &[Seg]) -> Option<usize> {
    let mut clicked = None;
    egui::Frame::new().fill(Color32::from_rgb(34, 34, 37)).corner_radius(8).inner_margin(Margin::same(2)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (i, s) in segs.iter().enumerate() {
            let font = FontId::proportional(13.0);
            let text_w = if s.label.is_empty() { 0.0 } else { ui.painter().layout_no_wrap(s.label.to_string(), font, Color32::WHITE).size().x + 8.0 };
            let size = vec2(if s.label.is_empty() { 30.0 } else { 34.0 + text_w }, 24.0);
            let (rect, resp) = ui.allocate_exact_size(size, if s.enabled { Sense::click() } else { Sense::hover() });
            paint_tool(ui, rect, &resp, s.icon, s.label, s.active, s.enabled, 6.0);
            if resp.on_hover_text(s.hover).clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// Background and icon (plus optional label) of a toolbar button.
#[allow(clippy::too_many_arguments)]
fn paint_tool(ui: &Ui, rect: Rect, resp: &Response, ic: &str, label: &str, active: bool, enabled: bool, radius: f32) {
    let p = ui.painter();
    let hovered = enabled && resp.hovered();
    if active {
        p.rect_filled(rect, radius, if hovered { Color32::from_rgb(52, 80, 116) } else { theme::ACCENT_SOFT });
    } else if hovered {
        p.rect_filled(rect, radius, Color32::from_rgb(46, 46, 51));
    }
    let color = if !enabled {
        Color32::from_rgb(78, 78, 84)
    } else if active {
        Color32::WHITE
    } else if hovered {
        theme::TEXT
    } else {
        Color32::from_rgb(186, 186, 192)
    };
    if label.is_empty() {
        p.text(rect.center(), egui::Align2::CENTER_CENTER, ic, FontId::proportional(17.0), color);
    } else {
        p.text(rect.left_center() + vec2(16.0, 0.0), egui::Align2::CENTER_CENTER, ic, FontId::proportional(16.0), color);
        p.text(rect.left_center() + vec2(30.0, 0.0), egui::Align2::LEFT_CENTER, label, FontId::proportional(13.0), color);
    }
}

fn group_gap(ui: &mut Ui) {
    ui.add_space(7.0);
    let (rect, _) = ui.allocate_exact_size(vec2(1.0, 18.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, theme::BORDER);
    ui.add_space(7.0);
}

pub fn bar_margin() -> Margin {
    Margin::symmetric(8, 5)
}

/// The app's icon as a texture (with mipmaps: it is drawn much smaller than it is).
fn app_icon(ctx: &egui::Context) -> egui::TextureHandle {
    let id = egui::Id::new("app-icon");
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return t;
    }
    let img = image::load_from_memory(include_bytes!("../assets/icon-256.png")).map(|i| i.to_rgba8()).unwrap_or_default();
    let image = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
    let options = egui::TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear));
    let t = ctx.load_texture("app-icon", image, options);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}
