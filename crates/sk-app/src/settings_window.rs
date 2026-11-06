//! "Einstellungen …" (Strg+,): everything that sets up the program, in one window. On the left
//! the pages, on the right the chosen page in cards, as in the other dialogs.

use egui::{Color32, CornerRadius, Rect, RichText, Sense, Stroke, Ui, UiBuilder, pos2, vec2};

use crate::app::App;
use crate::dialog::{self, Tone};
use crate::theme::{self, icon};

/// Size of the window (smaller on a small screen).
const SIZE: egui::Vec2 = vec2(880.0, 600.0);
/// Width of the page list.
const SIDE: f32 = 224.0;
/// The page list, a little darker than the page.
const SIDE_FILL: Color32 = Color32::from_rgb(30, 30, 33);
const NAV_HEIGHT: f32 = 36.0;
/// Hairline above the buttons, button row and their margin.
const FOOTER: f32 = 62.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Page {
    #[default]
    General,
    Library,
    Gpu,
    Storage,
    Shortcuts,
}

impl Page {
    pub const ALL: [Page; 5] = [Page::General, Page::Library, Page::Gpu, Page::Storage, Page::Shortcuts];

    pub fn title(self) -> &'static str {
        match self {
            Page::General => "Allgemein",
            Page::Library => "Bibliothek",
            Page::Gpu => "Grafikkarte",
            Page::Storage => "Speicherplatz",
            Page::Shortcuts => "Tastenkürzel",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Page::General => icon::GEAR_SIX,
            Page::Library => icon::IMAGES,
            Page::Gpu => icon::LIGHTNING,
            Page::Storage => icon::HARD_DRIVES,
            Page::Shortcuts => icon::KEYBOARD,
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Page::General => "Updates und Programm",
            Page::Library => "Neue Bilder, Speicherkarten und XMP-Dateien",
            Page::Gpu => "Wo Silberkorn rechnet",
            Page::Storage => "Zwischenspeicher, um schneller zu sein",
            Page::Shortcuts => "Klick auf ein Kürzel ändert es, + fügt eines hinzu, Rechtsklick entfernt es",
        }
    }

    /// The page by its name or its English id (`SILBERKORN_DIALOG=settings:gpu`).
    pub fn find(name: &str) -> Option<Page> {
        let name = name.to_lowercase();
        Page::ALL.into_iter().find(|p| p.title().to_lowercase() == name || format!("{p:?}").to_lowercase() == name)
    }
}

#[derive(Default)]
pub struct SettingsWindow {
    pub open: bool,
    pub page: Page,
}

impl SettingsWindow {
    pub fn shows(&self, page: Page) -> bool {
        self.open && self.page == page
    }
}

impl App {
    pub fn open_settings(&mut self, page: Page) {
        self.settings.open = true;
        self.settings.page = page;
    }

    pub(crate) fn settings_window(&mut self, ctx: &egui::Context) {
        // Forget what only an open page needs: measured anew when it opens again.
        if !self.settings.shows(Page::Storage) {
            self.storage_sizes = None;
        }
        if !self.settings.shows(Page::Shortcuts) {
            self.shortcut_editor.recording = None;
            self.shortcut_editor.message = None;
        }
        if !self.settings.open {
            return;
        }
        let recording = self.shortcut_editor.recording.is_some();
        if recording && self.record_shortcut(ctx) {
            self.save_settings();
        }

        let screen = ctx.content_rect();
        let size = vec2(SIZE.x.min(screen.width() - 32.0), SIZE.y.min(screen.height() - 32.0)).max(vec2(560.0, 360.0));
        let mut close = false;
        let modal = dialog::modal("settings", dialog::frame()).show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            let side = Rect::from_min_max(rect.min, pos2(rect.left() + SIDE, rect.bottom()));
            let r = dialog::RADIUS;
            ui.painter().rect_filled(side, CornerRadius { nw: r, sw: r, ne: 0, se: 0 }, SIDE_FILL);
            ui.painter().vline(side.right(), side.y_range(), Stroke::new(1.0, theme::BORDER));

            let mut nav = ui.new_child(UiBuilder::new().max_rect(side.shrink2(vec2(12.0, 18.0))).layout(egui::Layout::top_down(egui::Align::Min)));
            self.settings_nav(&mut nav, side);

            let content = Rect::from_min_max(pos2(side.right(), rect.top()), rect.max).shrink2(vec2(dialog::PAD + 4.0, 22.0));
            let mut page = ui.new_child(UiBuilder::new().max_rect(content).layout(egui::Layout::top_down(egui::Align::Min)));
            close = self.settings_page(&mut page);
        });
        // Esc ends a recording first, and only then closes the window.
        if close || (modal.should_close() && !recording) {
            self.settings.open = false;
        }
    }

    /// The window's title and the page list on the left; the version at the bottom.
    fn settings_nav(&mut self, ui: &mut Ui, side: Rect) {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(RichText::new("Einstellungen").size(18.0).strong().color(Color32::WHITE));
        });
        ui.add_space(18.0);
        ui.spacing_mut().item_spacing.y = 3.0;
        for page in Page::ALL {
            if nav_item(ui, page, self.settings.page == page).clicked() && self.settings.page != page {
                self.settings.page = page;
            }
        }
        let version = format!("Silberkorn {}", env!("CARGO_PKG_VERSION"));
        ui.painter().text(pos2(side.left() + 22.0, side.bottom() - 20.0), egui::Align2::LEFT_CENTER, version, egui::FontId::proportional(11.5), theme::TEXT_FAINT);
    }

    /// Header, the page and, in a band of their own at the bottom, the buttons. Returns
    /// whether "Fertig" was pressed.
    fn settings_page(&mut self, ui: &mut Ui) -> bool {
        let page = self.settings.page;
        let area = ui.max_rect();
        ui.spacing_mut().item_spacing.y = 6.0;
        dialog::header(ui, page.icon(), page.title(), Some(page.subtitle()), Tone::Accent);
        ui.add_space(14.0);
        let body = Rect::from_min_max(pos2(area.left(), ui.cursor().top()), pos2(area.right(), area.bottom() - FOOTER));
        let mut body_ui = ui.new_child(UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
        body_ui.set_clip_rect(body.expand2(vec2(6.0, 0.0)).intersect(ui.clip_rect()));
        body_ui.spacing_mut().item_spacing.y = 6.0;
        match page {
            // Its list scrolls below the search field.
            Page::Shortcuts => self.shortcuts_page(&mut body_ui),
            _ => {
                egui::ScrollArea::vertical().id_salt(("settings", page as u8)).auto_shrink([false, false]).show(&mut body_ui, |ui| match page {
                    Page::General => self.general_page(ui),
                    Page::Library => self.library_page(ui),
                    Page::Gpu => self.gpu_page(ui),
                    Page::Storage => self.storage_page(ui),
                    Page::Shortcuts => {}
                });
            }
        }
        let footer = Rect::from_min_max(pos2(area.left(), area.bottom() - FOOTER), area.max);
        let mut footer_ui = ui.new_child(UiBuilder::new().max_rect(footer).layout(egui::Layout::top_down(egui::Align::Min)));
        let mut done = false;
        dialog::footer(&mut footer_ui, |ui| {
            done = dialog::default_button(ui, true, dialog::primary("Fertig"));
        });
        done
    }

    fn general_page(&mut self, ui: &mut Ui) {
        let mut check = false;
        let status = self.updates.available.as_ref().map(|r| format!("{} verfügbar", r.version));
        dialog::card(ui, "Updates", status.as_deref().map(|s| (Tone::Accent, s)), |ui| {
            dialog::note(ui, format!("Installiert ist Version {}. Neue Versionen kommen von GitHub; installiert wird beim Beenden.", env!("CARGO_PKG_VERSION")));
            let mut auto = !self.updates.auto_off;
            ui.horizontal(|ui| {
                crate::widgets::checkbox(ui, &mut auto, "Beim Start nach Updates suchen (höchstens einmal am Tag)");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    check = ui.add(dialog::secondary(format!("{}  Jetzt suchen", icon::ARROW_CIRCLE_UP))).clicked();
                });
            });
            if auto == self.updates.auto_off {
                self.updates.auto_off = !auto;
                self.save_settings();
            }
        });
        if check {
            // The update dialog takes over.
            self.settings.open = false;
            self.update_dialog_open();
        }
    }

    fn library_page(&mut self, ui: &mut Ui) {
        dialog::card(ui, "Neue Aufnahmen", None, |ui| {
            dialog::note(ui, "Bilder, die etwa die Kamerasoftware in den angezeigten Ordner legt, erscheinen von selbst. Auch im Menü Datei.");
            self.hot_folder_ui(ui);
        });
        let mut on_card = self.import_options.open_on_card;
        dialog::card(ui, "Speicherkarten", None, |ui| {
            dialog::note(ui, "Ziel, Umbenennen und Unterordner wählt der Importdialog; er merkt sie sich.");
            crate::widgets::checkbox(ui, &mut on_card, "Importdialog öffnen, wenn eine Speicherkarte erkannt wird");
        });
        if on_card != self.import_options.open_on_card {
            self.import_options.open_on_card = on_card;
            self.save_settings();
        }
        let mut xmp = sk_core::xmp::write_enabled();
        dialog::card(ui, "XMP-Dateien", None, |ui| {
            dialog::note(ui, "Bewertung, Farb-Label und Keywords mit Lightroom, Bridge und Capture One austauschen. Einzeln lesen und schreiben im Menü Foto unter XMP.");
            crate::widgets::checkbox(ui, &mut xmp, "Änderungen automatisch in XMP schreiben");
        });
        if xmp != sk_core::xmp::write_enabled() {
            sk_core::xmp::set_write_enabled(xmp);
            self.save_settings();
        }
    }
}

/// One page in the list: icon and name; the chosen one on an accent tint with a bar on the left.
fn nav_item(ui: &mut Ui, page: Page, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), NAV_HEIGHT), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let ctx = ui.ctx();
    let on = ctx.animate_bool_with_time(resp.id.with("on"), selected, 0.12);
    let hover = ctx.animate_bool_with_time(resp.id.with("hover"), resp.hovered() && !selected, 0.08);
    let p = ui.painter();
    if hover > 0.0 {
        p.rect_filled(rect, 8, Color32::from_white_alpha((10.0 * hover) as u8));
    }
    if on > 0.0 {
        p.rect_filled(rect, 8, theme::ACCENT.gamma_multiply(0.16 * on));
        let bar = Rect::from_center_size(pos2(rect.left() + 1.5, rect.center().y), vec2(3.0, (NAV_HEIGHT - 14.0) * on));
        p.rect_filled(bar, 2, theme::ACCENT);
    }
    let icon_color = theme::TEXT_DIM.lerp_to_gamma(theme::ACCENT, on);
    let text_color = theme::TEXT.lerp_to_gamma(Color32::WHITE, on);
    p.text(pos2(rect.left() + 16.0, rect.center().y), egui::Align2::LEFT_CENTER, page.icon(), egui::FontId::proportional(17.0), icon_color);
    p.text(pos2(rect.left() + 44.0, rect.center().y), egui::Align2::LEFT_CENTER, page.title(), egui::FontId::proportional(13.5), text_color);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_by_name_and_id() {
        assert_eq!(Page::find("gpu"), Some(Page::Gpu));
        assert_eq!(Page::find("Tastenkürzel"), Some(Page::Shortcuts));
        assert_eq!(Page::find("bibliothek"), Some(Page::Library));
        assert_eq!(Page::find("nichts"), None);
    }
}
