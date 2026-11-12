//! The Ausgabe tab: "Datei" (the export on top, the output recipes as cards, the files written
//! last) and "Druck" (see `print_preview`), each with the soft proof as one line at the end.
//! The cards, switches and folding rows here are drawn by hand, so that nothing in them widens
//! the panel.

use std::path::Path;

use egui::{Color32, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use sk_core::export::{ExportFormat, ExportRecipe, OutputSharpening, Resize};
use sk_core::watermark::{Anchor, WatermarkKind};

use crate::app::App;
use crate::theme::{self, icon};
use crate::widgets::{self, BarButton};

/// Section titles on the print page: the command palette switches to it for them.
pub(crate) const PRINT_SECTIONS: &[&str] = &["Papier", "Bild auf der Seite", "Druckanpassung", "Farben"];
/// Section titles on the file page.
const FILE_SECTIONS: &[&str] = &["Ausgaberezepte", "Wasserzeichen", "Metadaten", "Zuletzt exportiert"];

/// The edge of the open recipe card.
const OPEN_EDGE: Color32 = Color32::from_rgb(61, 95, 138);
const OK: Color32 = Color32::from_rgb(95, 184, 90);

/// What a click in a recipe card asks for.
enum CardAction {
    Toggle(usize),
    Open(usize),
    Duplicate(usize),
    Remove(usize),
}

/// The open image, for the file name shown under the template.
struct NameSample {
    path: std::path::PathBuf,
    info: Option<sk_core::raw::CaptureInfo>,
    session: String,
}

impl App {
    pub(crate) fn output_tab(&mut self, ui: &mut Ui) {
        if let Some(t) = widgets::pending_reveal(ui.ctx()) {
            let base = t.split('/').next().unwrap_or_default();
            if PRINT_SECTIONS.contains(&base) {
                self.output_print = true;
            } else if FILE_SECTIONS.contains(&base) {
                self.output_print = false;
                self.recipe_open |= base == "Wasserzeichen" || base == "Metadaten";
            }
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            let pages = [
                BarButton::new(icon::FILE_IMAGE, "Datei").name("Datei").hover("Exportieren mit den Ausgaberezepten").selected(!self.output_print),
                BarButton::new(icon::PRINTER, "Druck").name("Druck").hover("Druckvorschau und Drucken").selected(self.output_print),
            ];
            if let Some(i) = widgets::button_bar(ui, "output-page", &pages) {
                self.output_print = i == 1;
            }
        });
        ui.add_space(8.0);
        if self.output_print {
            self.print_page(ui);
        } else {
            self.file_page(ui);
        }
        ui.add_space(8.0);
        self.softproof_card(ui);
        if self.output_print {
            self.print_extra_buttons(ui);
        }
        ui.add_space(12.0);
    }

    fn file_page(&mut self, ui: &mut Ui) {
        self.export_card(ui);
        widgets::nav_header(ui, "Ausgaberezepte", "Rezepte", |_| {});
        let sample = self.current.as_ref().map(|c| NameSample {
            path: c.meta.path.clone(),
            info: Some(c.meta.info.clone()),
            session: self.session.as_ref().map(|s| s.name.clone()).unwrap_or_default(),
        });
        let mut action = None;
        let last = self.recipes.len() <= 1;
        for i in 0..self.recipes.len() {
            let open = self.recipe_open && self.selected_recipe == i;
            let mut r = self.recipes[i].clone();
            if let Some(a) = recipe_card(ui, i, &mut r, open, last, sample.as_ref()) {
                action = Some(a);
            }
            if r != self.recipes[i] {
                self.recipes[i] = r;
            }
            ui.add_space(6.0);
        }
        if add_button(ui).clicked() {
            let mut r = self.recipes.get(self.selected_recipe).cloned().unwrap_or_default();
            r.name = format!("Rezept {}", self.recipes.len() + 1);
            r.enabled = true;
            self.recipes.push(r);
            self.selected_recipe = self.recipes.len() - 1;
            self.recipe_open = true;
        }
        match action {
            Some(CardAction::Toggle(i)) => self.recipes[i].enabled ^= true,
            Some(CardAction::Open(i)) => {
                self.recipe_open = !(self.recipe_open && self.selected_recipe == i);
                self.selected_recipe = i;
            }
            Some(CardAction::Duplicate(i)) => {
                let mut r = self.recipes[i].clone();
                r.name = format!("{} Kopie", r.name);
                self.recipes.insert(i + 1, r);
                self.selected_recipe = i + 1;
                self.recipe_open = true;
            }
            Some(CardAction::Remove(i)) => {
                let name = self.recipes[i].name.clone();
                self.request_deletion(crate::deletion::Deletion::Recipe(i, name));
            }
            None => {}
        }
        self.selected_recipe = self.selected_recipe.min(self.recipes.len().saturating_sub(1));
        self.export_log_ui(ui);
    }

    /// The export button (or the progress), with how many images, recipes and files.
    fn export_card(&mut self, ui: &mut Ui) {
        let n = self.selected_count();
        let k = self.recipes.iter().filter(|r| r.enabled).count();
        let enabled = self.command_available("export");
        let blocked = self.export_blocked_reason();
        let mut export = false;
        card(ui, None, |ui| {
            if let (Some((done, total)), Some(f)) = (self.export_progress, self.export_fraction()) {
                ui.horizontal(|ui| {
                    let w = (ui.available_width() - 96.0).max(40.0);
                    ui.add(egui::ProgressBar::new(f).desired_width(w).text(format!("{done} / {total}")));
                    if let Some(c) = &self.export_cancel {
                        widgets::cancel_button(ui, c);
                    }
                });
            } else {
                let text = RichText::new(format!("{}  Exportieren", icon::EXPORT)).size(14.0).strong().color(Color32::WHITE);
                let button = egui::Button::new(text).fill(theme::ACCENT).corner_radius(8).min_size(vec2(ui.available_width(), 34.0));
                let r = ui.add_enabled(enabled, crate::dialog::unless_enabled(button, enabled)).on_disabled_hover_text(blocked);
                if r.on_hover_text(crate::commands::hint("Exportiert die Auswahl mit allen eingeschalteten Rezepten", "export")).clicked() {
                    export = true;
                }
            }
            ui.add_space(6.0);
            let recipes = if k == 1 { "1 Rezept".to_string() } else { format!("{k} Rezepte") };
            let files = n * k;
            let files = if files == 1 { "1 Datei".to_string() } else { format!("{files} Dateien") };
            split_line(ui, &format!("{} × {recipes}", widgets::image_count(n)), &files);
        });
        if export {
            self.export_selection();
        }
    }

    /// The files written last: name (whole path on hover), or the error in red.
    fn export_log_ui(&mut self, ui: &mut Ui) {
        if self.export_log.is_empty() {
            return;
        }
        let last_dir = self.export_log.iter().rev().find_map(|l| l.strip_prefix("✔ ")).and_then(|p| Path::new(p).parent().map(Path::to_path_buf));
        let mut reveal = false;
        widgets::nav_header(ui, "Zuletzt exportiert", "Zuletzt exportiert", |ui| {
            if let Some(dir) = &last_dir
                && widgets::nav_action(ui, icon::ARROW_SQUARE_OUT, &format!("Im Explorer zeigen\n{}", dir.display())).clicked()
            {
                reveal = true;
            }
        });
        for line in self.export_log.iter().rev().take(6) {
            let (ok, text) = match line.strip_prefix("✔ ") {
                Some(p) => (true, p),
                None => (false, line.strip_prefix("✖ ").unwrap_or(line)),
            };
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
            let p = ui.painter();
            let y = rect.center().y;
            let (ic, color) = if ok { (icon::CHECK_CIRCLE, OK) } else { (icon::WARNING_CIRCLE, theme::DANGER) };
            p.text(pos2(rect.left() + 16.0, y), egui::Align2::CENTER_CENTER, ic, egui::FontId::proportional(14.0), color);
            let name = if ok { Path::new(text).file_name().map_or(text.to_string(), |n| n.to_string_lossy().to_string()) } else { text.to_string() };
            let galley = truncated(ui, name, 13.0, if ok { theme::TEXT } else { theme::DANGER }, rect.right() - 8.0 - (rect.left() + 32.0));
            let cut = galley.elided;
            ui.painter().galley(pos2(rect.left() + 32.0, y - galley.size().y / 2.0), galley, theme::TEXT);
            if ok || cut {
                resp.on_hover_text(text);
            }
        }
        if reveal
            && let Some(dir) = last_dir
            && let Err(e) = crate::session::reveal(&dir)
        {
            self.report_error(format!("Ordner nicht geöffnet: {e}"));
        }
    }
}

/// One output recipe: the switch, name, format and what it writes; open, everything to set.
fn recipe_card(ui: &mut Ui, i: usize, r: &mut ExportRecipe, open: bool, last: bool, sample: Option<&NameSample>) -> Option<CardAction> {
    let mut action = None;
    ui.push_id(("recipe", i), |ui| {
        card(ui, open.then_some(OPEN_EDGE), |ui| {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
            let check = Rect::from_center_size(pos2(rect.left() + 9.0, rect.center().y), vec2(18.0, 18.0));
            let check_resp = ui.interact(check.expand(4.0), ui.id().with("on"), Sense::click());
            check_resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, r.enabled, format!("{} verwenden", r.name)));
            let p = ui.painter();
            if r.enabled {
                p.rect_filled(check, 4.0, theme::ACCENT);
                p.text(check.center(), egui::Align2::CENTER_CENTER, icon::CHECK, egui::FontId::proportional(12.0), Color32::WHITE);
            } else {
                let edge = if check_resp.hovered() { theme::TEXT_DIM } else { theme::TEXT_FAINT };
                p.rect_stroke(check, 4.0, Stroke::new(1.5, edge), StrokeKind::Inside);
            }
            let left = check.right() + 12.0;
            let right = rect.right() - 22.0;
            // Badge after the name, the name truncated where both don't fit.
            let badge = format_badge(r.format);
            let badge_g = p.layout_no_wrap(badge, egui::FontId::proportional(10.5), theme::ACCENT_TEXT);
            let name_color = if r.enabled { Color32::WHITE } else { theme::TEXT_DIM };
            let name_g = truncated(ui, r.name.clone(), 14.0, name_color, right - left - badge_g.size().x - 22.0);
            let top = rect.top() + 11.0;
            let name_w = name_g.size().x;
            let p = ui.painter();
            p.galley(pos2(left, top - name_g.size().y / 2.0), name_g, name_color);
            let b = Rect::from_min_size(pos2(left + name_w + 8.0, top - badge_g.size().y / 2.0 - 2.0), badge_g.size() + vec2(12.0, 4.0));
            p.rect_filled(b, 5.0, theme::ACCENT_SOFT);
            p.galley(b.center() - badge_g.size() / 2.0, badge_g, theme::ACCENT_TEXT);
            let line = truncated(ui, summary(r), 12.0, theme::TEXT_DIM, right - left);
            ui.painter().galley(pos2(left, rect.top() + 29.0 - line.size().y / 2.0), line, theme::TEXT_DIM);
            let caret = if open { icon::CARET_DOWN } else { icon::CARET_RIGHT };
            let caret_color = if resp.hovered() { Color32::WHITE } else { theme::TEXT_DIM };
            ui.painter().text(pos2(rect.right() - 6.0, rect.center().y), egui::Align2::CENTER_CENTER, caret, egui::FontId::proportional(13.0), caret_color);
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, open, format!("Rezept {}", r.name)));
            if check_resp.clicked() {
                action = Some(CardAction::Toggle(i));
            } else if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                action = Some(CardAction::Open(i));
            }
            if open {
                ui.add_space(4.0);
                hairline(ui);
                ui.add_space(8.0);
                if let Some(a) = recipe_editor(ui, i, r, last, sample) {
                    action = Some(a);
                }
            }
        });
    });
    action
}

/// Everything of an open recipe card.
fn recipe_editor(ui: &mut Ui, i: usize, r: &mut ExportRecipe, last: bool, sample: Option<&NameSample>) -> Option<CardAction> {
    let mut action = None;
    widgets::field(ui, "Name", |ui| ui.text_edit_singleline(&mut r.name));
    // Format: TIFF as one, its bit depth below.
    let tiff = matches!(r.format, ExportFormat::Tiff8 | ExportFormat::Tiff16);
    let formats = [ExportFormat::Jpeg, ExportFormat::Tiff16, ExportFormat::Png, ExportFormat::WebP, ExportFormat::Avif];
    let names = ["JPEG", "TIFF", "PNG", "WebP", "AVIF"];
    // Across the card: five formats don't fit next to a label.
    let picked = ui
        .horizontal(|ui| {
            let buttons: Vec<BarButton> = formats
                .iter()
                .zip(names)
                .map(|(f, n)| BarButton::new("", n).selected(if *f == ExportFormat::Tiff16 { tiff } else { r.format == *f }))
                .collect();
            widgets::button_bar(ui, "format", &buttons)
        })
        .inner;
    if let Some(k) = picked {
        let f = formats[k];
        if !(f == ExportFormat::Tiff16 && tiff) {
            r.format = f;
        }
        if !r.format.supports(r.output_space) {
            r.output_space = sk_core::color::OutputSpace::Srgb;
        }
    }
    if tiff {
        let bits = widgets::field(ui, "Bittiefe", |ui| {
            let b = [BarButton::new("", "8 Bit").selected(r.format == ExportFormat::Tiff8), BarButton::new("", "16 Bit").selected(r.format == ExportFormat::Tiff16)];
            widgets::button_bar(ui, "bits", &b)
        });
        if let Some(k) = bits {
            r.format = if k == 0 { ExportFormat::Tiff8 } else { ExportFormat::Tiff16 };
        }
    }
    // Lossless formats have no quality: shown, but disabled.
    ui.add_enabled_ui(r.format.has_quality(), |ui| {
        let mut q = r.quality as f32;
        if widgets::slider_row(ui, "Qualität", &mut q, 1.0..=100.0, 90.0, 0) {
            r.quality = q.round() as _;
        }
    })
    .response
    .on_disabled_hover_text("Verlustfreies Format: ohne Qualitätsstufe");
    widgets::combo_field(ui, "Farbraum", r.output_space.label(), |ui| {
        for sp in sk_core::color::OutputSpace::ALL {
            ui.add_enabled_ui(r.format.supports(sp), |ui| ui.selectable_value(&mut r.output_space, sp, sp.label()))
                .response
                .on_disabled_hover_text("AVIF-Dateien kennzeichnen den Farbraum ohne ICC-Profil; das geht nur für sRGB und Display P3");
        }
    });
    size_row(ui, r);
    widgets::combo_field(ui, "Schärfen", r.output_sharpening.label(), |ui| {
        for v in OutputSharpening::ALL {
            ui.selectable_value(&mut r.output_sharpening, v, v.label()).on_hover_text("Ausgabeschärfung: gleicht die Weichheit des Verkleinerns aus");
        }
    });
    ui.add_space(4.0);
    hairline(ui);
    ui.add_space(6.0);
    widgets::field(ui, "Ordner", |ui| {
        let mut dir = r.output_dir.to_string_lossy().to_string();
        let edit = egui::TextEdit::singleline(&mut dir).desired_width(ui.available_width() - 38.0);
        if ui.add(edit).on_hover_text("Relativ zum Bildordner, z. B. „Output“").changed() {
            r.output_dir = dir.into();
        }
        if small_icon_button(ui, icon::FOLDER_OPEN, "Ordner wählen …").clicked()
            && let Some(d) = rfd::FileDialog::new().set_title("Zielordner").pick_folder()
        {
            r.output_dir = d;
        }
    });
    widgets::field(ui, "Dateiname", |ui| {
        let edit = egui::TextEdit::singleline(&mut r.name_template).hint_text("{Name}").desired_width(ui.available_width() - 38.0);
        ui.add(edit);
        let b = egui::Button::new(RichText::new(icon::PLUS).size(13.0)).min_size(vec2(26.0, 22.0));
        let resp = ui.add(b).on_hover_text("Platzhalter einfügen");
        widgets::menu_popup(&resp).show(|ui| {
            for (token, help) in sk_core::naming::TOKENS {
                if ui.button(format!("{token}   {help}")).clicked() {
                    r.name_template.push_str(token);
                    ui.close();
                }
            }
        });
    });
    widgets::field(ui, "Suffix", |ui| ui.add(egui::TextEdit::singleline(&mut r.suffix).hint_text("z. B. _web")));
    if let Some(s) = sample {
        let path = r.output_path(&sk_core::naming::NameFields { image: &s.path, capture: s.info.as_ref(), counter: 1, session: &s.session });
        let file = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        let shown = if r.output_dir.is_absolute() { path.display().to_string() } else { Path::new(&r.output_dir).join(&file).display().to_string().replace('\\', "/") };
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
        let p = ui.painter();
        p.text(pos2(rect.left() + 6.0, rect.center().y), egui::Align2::CENTER_CENTER, icon::ARROW_RIGHT, egui::FontId::proportional(11.0), theme::TEXT_DIM);
        let g = truncated(ui, shown, 12.0, theme::ACCENT_TEXT, rect.width() - 20.0);
        let cut = g.elided;
        ui.painter().galley(pos2(rect.left() + 18.0, rect.center().y - g.size().y / 2.0), g, theme::ACCENT_TEXT);
        if cut {
            resp.on_hover_text(path.display().to_string());
        }
    }

    ui.add_space(4.0);
    hairline(ui);
    ui.add_space(4.0);
    watermark_part(ui, r);
    let has_meta = r.format.has_metadata();
    let meta_line = if !has_meta {
        "TIFF ohne Metadaten".to_string()
    } else {
        let mut parts = Vec::new();
        if r.capture_metadata {
            parts.push("Aufnahmedaten".to_string());
        }
        if !r.copyright.trim().is_empty() {
            parts.push(r.copyright.trim().to_string());
        }
        if parts.is_empty() { "Aus".into() } else { parts.join(" · ") }
    };
    let mut on = r.capture_metadata;
    let open = fold_row(ui, "Metadaten", icon::CLIPBOARD_TEXT, &meta_line, Some((&mut on, has_meta, "Aufnahmedaten übernehmen: Kamera, Objektiv, Belichtung und Datum; GPS nie")));
    r.capture_metadata = on;
    if open {
        ui.add_enabled_ui(has_meta, |ui| {
            widgets::field(ui, "Fotograf", |ui| ui.text_edit_singleline(&mut r.artist));
            widgets::field(ui, "Copyright", |ui| ui.text_edit_singleline(&mut r.copyright));
        });
    }

    ui.add_space(6.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.add_space(2.0);
        let del = egui::Button::new(RichText::new(format!("{}  Löschen …", icon::TRASH)).size(12.0)).fill(Color32::TRANSPARENT).stroke(Stroke::new(1.0, theme::BORDER_STRONG)).corner_radius(7.0);
        if ui.add_enabled(!last, del).on_disabled_hover_text("Das einzige Rezept bleibt").clicked() {
            action = Some(CardAction::Remove(i));
        }
        let dup = egui::Button::new(RichText::new(format!("{}  Duplizieren", icon::COPY)).size(12.0)).fill(Color32::TRANSPARENT).stroke(Stroke::new(1.0, theme::BORDER_STRONG)).corner_radius(7.0);
        if ui.add(dup).clicked() {
            action = Some(CardAction::Duplicate(i));
        }
    });
    action
}

/// "Größe": the kind and, unless it is the original size, the value in pixels or percent.
fn size_row(ui: &mut Ui, r: &mut ExportRecipe) {
    widgets::field(ui, "Größe", |ui| {
        let label = if r.size.is_some() { r.resize.label() } else { "Originalgröße" };
        let mut resize = r.size.map(|_| r.resize);
        let w = if r.size.is_some() { ui.available_width() - 92.0 } else { ui.available_width() - 8.0 };
        let c = egui::ComboBox::from_id_salt("Größe").selected_text(label).width(w.max(60.0)).show_ui(ui, |ui| {
            ui.selectable_value(&mut resize, None, "Originalgröße");
            for m in Resize::ALL {
                ui.selectable_value(&mut resize, Some(m), m.label());
            }
        });
        c.response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, ui.is_enabled(), "Größe"));
        match resize {
            Some(m) => {
                let percent = m == Resize::Percent;
                // The value is kept in its unit when switching between the pixel modes.
                let mut v = match r.size {
                    Some(v) if (r.resize == Resize::Percent) == percent => v,
                    _ if percent => 50,
                    _ => 2048,
                };
                let drag = if percent { egui::DragValue::new(&mut v).range(1..=100).suffix(" %") } else { egui::DragValue::new(&mut v).range(16..=20000).suffix(" px") };
                ui.add_sized(vec2(76.0, 22.0), drag).on_hover_text("Nie größer als das entwickelte Bild");
                r.resize = m;
                r.size = Some(v);
            }
            None => r.size = None,
        }
    });
}

/// "Wasserzeichen" in the recipe card: a switch, opened text or logo, place, size and opacity.
fn watermark_part(ui: &mut Ui, r: &mut ExportRecipe) {
    let wm = &mut r.watermark;
    let line = if !wm.enabled {
        "Aus".to_string()
    } else {
        let what = match wm.kind {
            WatermarkKind::Text => format!("„{}“", wm.text.trim()),
            WatermarkKind::Image => wm.image.file_name().map_or("Logo".into(), |n| n.to_string_lossy().to_string()),
        };
        format!("{what} · {}", wm.anchor.label())
    };
    let mut on = wm.enabled;
    let open = fold_row(ui, "Wasserzeichen", icon::STAMP, &line, Some((&mut on, true, "Wasserzeichen einfügen")));
    wm.enabled = on;
    if !open {
        return;
    }
    ui.add_enabled_ui(wm.enabled, |ui| {
        let kind = widgets::field(ui, "Art", |ui| {
            let b = [BarButton::new("", "Text").selected(wm.kind == WatermarkKind::Text), BarButton::new("", "Logo").selected(wm.kind == WatermarkKind::Image)];
            widgets::button_bar(ui, "wm-kind", &b)
        });
        if let Some(k) = kind {
            wm.kind = if k == 0 { WatermarkKind::Text } else { WatermarkKind::Image };
        }
        match wm.kind {
            WatermarkKind::Text => {
                widgets::field(ui, "Text", |ui| ui.text_edit_singleline(&mut wm.text));
                let ink = widgets::field(ui, "Farbe", |ui| {
                    let b = [BarButton::new("", "Weiß").selected(!wm.dark), BarButton::new("", "Schwarz").selected(wm.dark)];
                    widgets::button_bar(ui, "wm-ink", &b)
                });
                if let Some(k) = ink {
                    wm.dark = k == 1;
                }
            }
            WatermarkKind::Image => {
                widgets::field(ui, "Logo", |ui| {
                    let name = wm.image.file_name().map_or("Logo wählen …".into(), |n| n.to_string_lossy().to_string());
                    let b = egui::Button::new(RichText::new(format!("{}  {name}", icon::FOLDER_OPEN))).truncate();
                    if ui.add_sized(vec2(ui.available_width() - 8.0, 22.0), b).on_hover_text("PNG mit Transparenz empfohlen").clicked()
                        && let Some(f) = rfd::FileDialog::new().set_title("Logo für das Wasserzeichen").add_filter("Bilder", &["png", "jpg", "jpeg", "tif", "tiff"]).pick_file()
                    {
                        wm.image = f;
                    }
                });
            }
        }
        widgets::field(ui, "Position", |ui| {
            if let Some(a) = anchor_grid(ui, wm.anchor) {
                wm.anchor = a;
            }
        });
        let pct = widgets::SliderOpts { suffix: " %", ..Default::default() };
        let d = sk_core::watermark::Watermark::default();
        widgets::slider_row_with(ui, "Größe", &mut wm.size, 1.0..=30.0, d.size, 1, pct);
        widgets::slider_row_with(ui, "Abstand", &mut wm.margin, 0.0..=20.0, d.margin, 1, widgets::SliderOpts { suffix: " %", ..Default::default() });
        let mut opacity = wm.opacity * 100.0;
        if widgets::slider_row_with(ui, "Deckkraft", &mut opacity, 5.0..=100.0, d.opacity * 100.0, 0, widgets::SliderOpts { suffix: " %", ..Default::default() }) {
            wm.opacity = opacity / 100.0;
        }
    })
    .response
    .on_disabled_hover_text("Erst einschalten");
}

/// The nine places of the watermark as a 3 × 3 grid; the clicked one.
fn anchor_grid(ui: &mut Ui, current: Anchor) -> Option<Anchor> {
    let cell = vec2(20.0, 14.0);
    let gap = 3.0;
    let (rect, _) = ui.allocate_exact_size(vec2(3.0 * cell.x + 2.0 * gap + 8.0, 3.0 * cell.y + 2.0 * gap + 8.0), Sense::hover());
    ui.painter().rect_filled(rect, 6.0, theme::CHROME);
    ui.painter().rect_stroke(rect, 6.0, Stroke::new(1.0, theme::BORDER), StrokeKind::Inside);
    let mut picked = None;
    for (k, a) in Anchor::ALL.iter().enumerate() {
        let (col, row) = ((k % 3) as f32, (k / 3) as f32);
        let r = Rect::from_min_size(rect.min + vec2(4.0 + col * (cell.x + gap), 4.0 + row * (cell.y + gap)), cell);
        let resp = ui.interact(r, ui.id().with(("anchor", k)), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, ui.is_enabled(), *a == current, a.label()));
        let fill = if *a == current {
            theme::ACCENT
        } else if resp.hovered() {
            theme::BORDER_STRONG
        } else {
            theme::CONTROL
        };
        ui.painter().rect_filled(r, 3.0, if ui.is_enabled() { fill } else { fill.gamma_multiply(0.5) });
        if resp.on_hover_text(a.label()).clicked() {
            picked = Some(*a);
        }
    }
    picked
}

/// "TIFF 16", "JPEG" …: the format, short.
fn format_badge(f: ExportFormat) -> String {
    match f {
        ExportFormat::Tiff8 => "TIFF 8".into(),
        ExportFormat::Tiff16 => "TIFF 16".into(),
        f => f.label().to_string(),
    }
}

/// "2048 px lang · sRGB · Output": what a recipe writes, in one line.
fn summary(r: &ExportRecipe) -> String {
    let size = match r.size {
        None => "Original".to_string(),
        Some(v) => match r.resize {
            Resize::Percent => format!("{v} %"),
            Resize::LongEdge => format!("{v} px lang"),
            Resize::ShortEdge => format!("{v} px kurz"),
            Resize::Width => format!("{v} px breit"),
            Resize::Height => format!("{v} px hoch"),
        },
    };
    let space = match r.output_space {
        sk_core::color::OutputSpace::AdobeRgb => "Adobe RGB",
        sk_core::color::OutputSpace::DisplayP3 => "P3",
        sk_core::color::OutputSpace::ProPhoto => "ProPhoto",
        s => s.label(),
    };
    let dir = r.output_dir.display().to_string().replace('\\', "/");
    let dir = if dir.is_empty() { "Bildordner".to_string() } else { dir };
    format!("{size} · {space} · {dir}")
}

// ---------------------------------------------------------------- building blocks

/// A rounded card across the panel (`edge`: a colored outline).
pub(crate) fn card<R>(ui: &mut Ui, edge: Option<Color32>, body: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(10.0)
        .stroke(Stroke::new(1.0, edge.unwrap_or(Color32::TRANSPARENT)))
        .inner_margin(egui::Margin { left: 12, right: 10, top: 10, bottom: 10 })
        .outer_margin(egui::Margin { left: 4, right: 4, top: 0, bottom: 0 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui)
        })
        .inner
}

fn paint_switch(ui: &Ui, rect: Rect, resp: &Response, on: bool, name: &str) {
    let enabled = ui.is_enabled();
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, on, name));
    let t = ui.ctx().animate_bool_with_time(resp.id, on, 0.12);
    let off_fill = if resp.hovered() && enabled { theme::BORDER_STRONG } else { theme::CONTROL };
    let fill = off_fill.lerp_to_gamma(theme::ACCENT, t);
    let fill = if enabled { fill } else { fill.gamma_multiply(0.45) };
    ui.painter().rect_filled(rect, rect.height() / 2.0, fill);
    let x = egui::lerp(rect.left() + 9.0..=rect.right() - 9.0, t);
    ui.painter().circle_filled(pos2(x, rect.center().y), 7.0, if enabled { Color32::WHITE } else { theme::TEXT_DIM });
}

/// A row with a text and a switch on the right; true when switched.
pub(crate) fn switch_row(ui: &mut Ui, text: &str, on: bool, hover: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
    let sw = Rect::from_center_size(pos2(rect.right() - 17.0, rect.center().y), vec2(30.0, 18.0));
    let color = if ui.is_enabled() { theme::TEXT } else { theme::TEXT_FAINT };
    ui.painter().text(pos2(rect.left() + 2.0, rect.center().y), egui::Align2::LEFT_CENTER, text, egui::FontId::proportional(13.0), color);
    paint_switch(ui, sw, &resp, on, text);
    let resp = if hover.is_empty() { resp } else { resp.on_hover_text(hover) };
    resp.clicked()
}

/// A row that opens the part below it: icon, title, a short summary and on the right a switch
/// (`(on, enabled, hover)`) or a caret. Returns whether it is open. The command palette opens it
/// by its title.
pub(crate) fn fold_row(ui: &mut Ui, title: &str, ic: &str, summary: &str, switch: Option<(&mut bool, bool, &str)>) -> bool {
    let id = egui::Id::new(("output-fold", title));
    let mut open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    if widgets::take_reveal(ui, title) {
        open = true;
        ui.scroll_to_rect(rect, Some(egui::Align::Center));
    }
    let sw_rect = Rect::from_center_size(pos2(rect.right() - 17.0, rect.center().y), vec2(30.0, 18.0));
    let row = Rect::from_min_max(rect.min, pos2(if switch.is_some() { sw_rect.left() - 6.0 } else { rect.right() }, rect.max.y));
    let row_resp = ui.interact(row, id.with("row"), Sense::click());
    row_resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::CollapsingHeader, true, open, title));
    if row_resp.hovered() {
        ui.painter().rect_filled(row, 6.0, Color32::from_rgb(52, 52, 57));
    }
    let p = ui.painter();
    let y = rect.center().y;
    p.text(pos2(rect.left() + 9.0, y), egui::Align2::CENTER_CENTER, ic, egui::FontId::proportional(14.0), theme::TEXT_DIM);
    let title_g = p.layout_no_wrap(title.to_string(), egui::FontId::proportional(13.0), theme::TEXT);
    let tx = rect.left() + 26.0;
    let title_w = title_g.size().x;
    p.galley(pos2(tx, y - title_g.size().y / 2.0), title_g, theme::TEXT);
    let caret_x = if switch.is_some() { sw_rect.left() - 14.0 } else { rect.right() - 8.0 };
    let sx = tx + title_w + 10.0;
    let g = truncated(ui, summary.to_string(), 12.0, theme::TEXT_DIM, caret_x - 10.0 - sx);
    ui.painter().galley(pos2(sx, y - g.size().y / 2.0), g, theme::TEXT_DIM);
    let caret = if open { icon::CARET_DOWN } else { icon::CARET_RIGHT };
    ui.painter().text(pos2(caret_x, y), egui::Align2::CENTER_CENTER, caret, egui::FontId::proportional(12.0), theme::TEXT_DIM);
    if let Some((on, enabled, hover)) = switch {
        let sw = ui.interact(sw_rect, id.with("switch"), if enabled { Sense::click() } else { Sense::hover() });
        ui.add_enabled_ui(enabled, |ui| paint_switch(ui, sw_rect, &sw, *on, title));
        if sw.on_hover_text(hover).clicked() {
            *on = !*on;
            // Switched on: open, to set it up.
            open |= *on;
        }
    }
    if row_resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        open = !open;
    }
    ui.data_mut(|d| d.insert_temp(id, open));
    open
}

/// A card holding a [`fold_row`] and, open, its body.
pub(crate) fn fold_card(ui: &mut Ui, title: &str, ic: &str, summary: &str, switch: Option<(&mut bool, bool, &str)>, body: impl FnOnce(&mut Ui)) {
    let inner = egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(10.0)
        .inner_margin(egui::Margin { left: 6, right: 6, top: 4, bottom: 4 })
        .outer_margin(egui::Margin { left: 4, right: 4, top: 0, bottom: 0 });
    inner.show(ui, |ui| {
        ui.set_width(ui.available_width());
        if fold_row(ui, title, ic, summary, switch) {
            egui::Frame::new().inner_margin(egui::Margin { left: 6, right: 2, top: 4, bottom: 6 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                body(ui);
            });
        }
    });
}

/// Text on the left and dim text on the right of one line.
fn split_line(ui: &mut Ui, left: &str, right: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
    let p = ui.painter();
    let r = p.layout_no_wrap(right.to_string(), egui::FontId::proportional(12.0), theme::TEXT_DIM);
    let rw = r.size().x;
    p.galley(pos2(rect.right() - rw, rect.center().y - r.size().y / 2.0), r, theme::TEXT_DIM);
    let l = truncated(ui, left.to_string(), 12.0, theme::TEXT, rect.width() - rw - 10.0);
    ui.painter().galley(pos2(rect.left(), rect.center().y - l.size().y / 2.0), l, theme::TEXT);
}

/// "+ Rezept": dashed, across the panel.
fn add_button(ui: &mut Ui) -> Response {
    let (outer, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    let rect = outer.shrink2(vec2(4.0, 0.0));
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Neues Rezept"));
    let color = if resp.hovered() { theme::TEXT_DIM } else { theme::BORDER_STRONG };
    let pts = widgets::rounded_rect_points(rect.shrink(0.5), 9.0);
    ui.painter().extend(widgets::dashed_closed(&pts, Stroke::new(1.0, color), 4.0, 3.0));
    let text = if resp.hovered() { Color32::WHITE } else { theme::TEXT_DIM };
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, format!("{}  Rezept", icon::PLUS), egui::FontId::proportional(13.0), text);
    resp.on_hover_text("Legt eine Kopie des gewählten Rezepts an")
}

fn small_icon_button(ui: &mut Ui, ic: &str, hover: &str) -> Response {
    let b = egui::Button::new(RichText::new(ic).size(13.0)).min_size(vec2(26.0, 22.0));
    ui.add(b).on_hover_text(hover)
}

/// A thin line across the card.
pub(crate) fn hairline(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, theme::BORDER));
}

/// `text` on one line, cut with "…" at `width`.
pub(crate) fn truncated(ui: &Ui, text: String, size: f32, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text, egui::FontId::proportional(size), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(10.0));
    ui.painter().layout_job(job)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_summary() {
        let mut r = ExportRecipe { size: Some(2048), resize: Resize::LongEdge, output_dir: "Output/Web".into(), ..Default::default() };
        assert_eq!(summary(&r), "2048 px lang · sRGB · Output/Web");
        r.size = None;
        r.output_space = sk_core::color::OutputSpace::AdobeRgb;
        assert_eq!(summary(&r), "Original · Adobe RGB · Output/Web");
        assert_eq!(format_badge(ExportFormat::Tiff16), "TIFF 16");
    }
}
