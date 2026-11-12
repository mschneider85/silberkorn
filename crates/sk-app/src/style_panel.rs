//! The Stile tab: where a click goes ("Wirkt auf"), the film simulations and the own styles as
//! previews or a list (a check on those that already act), and "Als Stil sichern" as a dashed
//! tile that opens the card for a new style.

use std::collections::BTreeSet;

use egui::{Color32, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle, Ui, pos2, vec2};
use sk_core::styles::{self, Style};
use sk_core::tools::Tool;

use crate::app::App;
use crate::output::{card_button, card_buttons, truncated};
use crate::style_grid::{Picks, applied_to, tile_size};
use crate::theme::{self, icon};
use crate::widgets;

const CHOSEN_EDGE: Color32 = Color32::from_rgb(61, 95, 138);
const CHOSEN_FILL: Color32 = Color32::from_rgb(38, 48, 61);
const TILE_GAP: f32 = 6.0;

/// A style as the tab shows it.
struct Entry {
    style: Style,
    /// A film simulation (see `styles::apply_look`).
    look: bool,
    /// Second line: the film's description, or how many tools a style sets.
    sub: String,
    hint: String,
}

/// Which of the shown styles already act on the image: applying them again would change nothing.
pub(crate) struct AppliedCache {
    key: (u64, std::path::PathBuf, Option<usize>, usize),
    on: Vec<bool>,
}

impl App {
    pub fn styles_tab(&mut self, ui: &mut Ui) {
        let layer = self.active_layer.filter(|i| self.current.as_ref().is_some_and(|c| *i < c.adjustments.layers.len()));
        self.target_row(ui, layer);
        if self.current.is_some() {
            self.pump_style_thumbs();
        }
        let looks: Vec<Entry> = self
            .film_looks
            .iter()
            .map(|(s, d)| Entry {
                style: s.clone(),
                look: true,
                sub: d.to_string(),
                hint: format!("{d}\n\nNachempfunden, keine offizielle Simulation. Belichtung, Weißabgleich und Korrekturen des Bildes bleiben."),
            })
            .collect();
        let own: Vec<Entry> = self
            .styles
            .iter()
            .map(|s| {
                let (sub, hint) = match &s.tools {
                    Some(t) => (format!("{} Werkzeuge", t.len()), format!("Setzt: {}", t.iter().map(|t| t.label()).collect::<Vec<_>>().join(", "))),
                    None => ("Alle Werkzeuge".to_string(), "Setzt alle Werkzeuge außer Zuschnitt, Drehen, Perspektive, Ebenen und Retusche".to_string()),
                };
                Entry { style: s.clone(), look: false, sub, hint }
            })
            .collect();
        let on = self.applied_styles(layer);
        let (on_looks, on_own) = on.split_at(looks.len().min(on.len()));

        let mut list = self.style_list;
        widgets::nav_header(ui, "Filmsimulationen", &format!("Filmsimulationen {}", looks.len()), |ui| {
            for (is_list, ic, name) in [(true, icon::LIST_BULLETS, "Liste"), (false, icon::SQUARES_FOUR, "Vorschauen")] {
                let color = if list == is_list { Color32::WHITE } else { theme::TEXT_DIM };
                let b = egui::Button::new(RichText::new(ic).size(14.0).color(color)).selected(list == is_list).frame_when_inactive(false).min_size(vec2(24.0, 22.0));
                if ui.add(b).on_hover_text(name).clicked() {
                    list = is_list;
                }
            }
        });
        if list != self.style_list {
            self.style_list = list;
            self.save_settings();
        }
        let (mut apply, mut hovered, mut delete) = (None, None, None);
        let mut take = |(c, h, d): Picks| {
            apply = apply.take().or(c);
            hovered = hovered.take().or(h);
            delete = delete.take().or(d);
        };
        let mut add = false;
        take(self.style_entries(ui, &looks, on_looks, None));
        let shown = if own.is_empty() { "Eigene Stile".to_string() } else { format!("Eigene Stile {}", own.len()) };
        widgets::nav_header(ui, "Eigene Stile", &shown, |_| {});
        let can_add = self.current.is_some();
        take(self.style_entries(ui, &own, on_own, can_add.then_some(&mut add)));
        if add {
            self.library_ui.new_style_open = true;
            self.library_ui.new_style_focus = true;
        }
        let save = if self.library_ui.new_style_open && self.current.is_some() { self.new_style_card(ui) } else { None };
        ui.add_space(12.0);

        if let (Some((style, look)), Some(cur)) = (&hovered, self.current.as_ref())
            && apply.is_none()
        {
            let preview = applied_to(style, *look, &cur.adjustments, layer);
            self.preview_style(preview);
        }
        if let Some(style) = save {
            match styles::save_in(&styles::styles_dir(), &style) {
                Ok(()) => self.status = format!("Stil „{}“ gespeichert", style.name),
                Err(e) => self.report_error(format!("Fehler: {e:#}")),
            }
            self.library_ui.new_style.clear();
            self.library_ui.new_style_tools = None;
            self.library_ui.new_style_open = false;
            self.reload_styles();
        }
        // A layer chosen: onto it, on the current image (layers belong to one image).
        if let Some((style, look)) = &apply
            && let Some(li) = layer
        {
            self.apply_style_to_layer(style, *look, li);
        } else if let Some((style, look)) = apply {
            let paths = self.selected_paths();
            self.set_adjustments_on(&paths, |a| if look { styles::apply_look(&style, a) } else { styles::apply(&style, a) });
            self.status = format!("Stil „{}“ auf {} angewendet", style.name, widgets::image_count(paths.len()));
        }
        if let Some(name) = delete {
            self.request_deletion(crate::deletion::Deletion::Style(name));
        }
    }

    /// "Wirkt auf": the chosen layer, or the images chosen.
    fn target_row(&self, ui: &mut Ui, layer: Option<usize>) {
        let (ic, text) = match layer.and_then(|li| self.current.as_ref().map(|c| c.adjustments.layers[li].name.clone())) {
            Some(name) => (icon::STACK, format!("Ebene „{name}“")),
            None => (icon::IMAGES, widgets::image_count(self.selected_paths().len())),
        };
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("Wirkt auf").size(12.0).color(theme::TEXT_DIM));
            let pill = egui::Frame::new()
                .fill(theme::CHROME)
                .stroke(Stroke::new(1.0, theme::BORDER_STRONG))
                .corner_radius(11.0)
                .inner_margin(egui::Margin { left: 9, right: 10, top: 3, bottom: 3 })
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.label(RichText::new(ic).size(13.0).color(theme::ACCENT));
                    ui.label(RichText::new(&text).size(12.0).color(Color32::WHITE));
                });
            pill.response.on_hover_text("Ein Klick auf einen Stil setzt ihn hier; der Zeiger darauf zeigt ihn als Vorschau im Bild");
        });
    }

    /// Which film looks (first) and own styles already act on the image or the chosen layer,
    /// worked out again only when something was edited.
    pub(crate) fn applied_styles(&mut self, layer: Option<usize>) -> Vec<bool> {
        let Some(cur) = self.current.as_ref() else { return Vec::new() };
        let n = self.film_looks.len() + self.styles.len();
        let key = (self.edit_version, cur.meta.path.clone(), layer, n);
        if let Some(c) = self.style_applied.as_ref().filter(|c| c.key == key) {
            return c.on.clone();
        }
        let a = &cur.adjustments;
        let on: Vec<bool> = self.film_looks.iter().map(|(s, _)| (s, true)).chain(self.styles.iter().map(|s| (s, false))).map(|(s, look)| applied_to(s, look, a, layer) == *a).collect();
        self.style_applied = Some(AppliedCache { key, on: on.clone() });
        on
    }

    /// The styles as tiles (two or more per row) or rows; `add`: the dashed tile at the end
    /// that opens the card for a new style.
    fn style_entries(&self, ui: &mut Ui, list: &[Entry], on: &[bool], add: Option<&mut bool>) -> Picks {
        let (mut clicked, mut hovered, mut delete) = (None, None, None);
        let texture = |e: &Entry| self.style_grid.texture(&e.style, e.look);
        let mut react = |e: &Entry, resp: &egui::Response, del: bool| {
            if resp.hovered() {
                hovered = Some((e.style.clone(), e.look));
            }
            if resp.clicked() {
                clicked = Some((e.style.clone(), e.look));
            }
            if del {
                delete = Some(e.style.name.clone());
            }
            if !e.look {
                widgets::context_menu(resp, |ui| {
                    if ui.button(format!("{}  Stil löschen …", icon::TRASH)).clicked() {
                        delete = Some(e.style.name.clone());
                        ui.close();
                    }
                });
            }
        };
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width() - 4.0);
                if self.style_list {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for (i, e) in list.iter().enumerate() {
                        let (resp, del) = style_row(ui, texture(e), e, on.get(i).copied().unwrap_or(false));
                        let resp = resp.on_hover_text(e.hint.as_str());
                        react(e, &resp, del);
                    }
                    if let Some(add) = add {
                        ui.add_space(2.0);
                        if add_tile(ui, vec2(ui.available_width(), 38.0)).clicked() {
                            *add = true;
                        }
                    }
                } else {
                    let (cols, tile, image_h) = tile_size(ui.available_width());
                    let label_h = if list.first().is_some_and(|e| !e.look) { 34.0 } else { 22.0 };
                    let mut cells: Vec<Option<&Entry>> = list.iter().map(Some).collect();
                    if add.is_some() {
                        cells.push(None);
                    }
                    let mut add = add;
                    for (r, row) in cells.chunks(cols).enumerate() {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = TILE_GAP;
                            for (c, cell) in row.iter().enumerate() {
                                let size = vec2(tile, image_h + label_h);
                                match cell {
                                    Some(e) => {
                                        let i = r * cols + c;
                                        let (resp, del) = style_tile(ui, texture(e), e, size, image_h, on.get(i).copied().unwrap_or(false));
                                        let resp = resp.on_hover_text(e.hint.as_str());
                                        react(e, &resp, del);
                                    }
                                    None => {
                                        if add_tile(ui, size).clicked()
                                            && let Some(a) = add.as_deref_mut()
                                        {
                                            *a = true;
                                        }
                                    }
                                }
                            }
                        });
                        ui.add_space(TILE_GAP);
                    }
                }
            });
        });
        (clicked, hovered, delete)
    }

    /// "Neuer Stil": name, the tools it takes as chips (the edited ones), Abbrechen | Speichern.
    fn new_style_card(&mut self, ui: &mut Ui) -> Option<Style> {
        let cur = self.current.as_ref()?;
        let edited: Vec<Tool> = sk_core::tools::edited(&cur.adjustments);
        let adjustments = cur.adjustments.clone();
        let lib = &mut self.library_ui;
        let mut chosen: BTreeSet<Tool> = lib.new_style_tools.clone().unwrap_or_else(|| edited.iter().copied().collect());
        let mut save = None;
        let mut close = false;
        ui.add_space(4.0);
        crate::output::card(ui, Some(CHOSEN_EDGE), |ui| {
            ui.label(RichText::new("Neuer Stil").size(13.5).strong().color(Color32::WHITE));
            ui.add_space(4.0);
            widgets::field(ui, "Name", |ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut lib.new_style).hint_text("Name des Stils"));
                if std::mem::take(&mut lib.new_style_focus) {
                    r.request_focus();
                }
            });
            ui.add_space(2.0);
            ui.horizontal_top(|ui| {
                let w = widgets::label_width(ui);
                ui.allocate_ui_with_layout(vec2(w, 22.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_min_width(w);
                    ui.label(RichText::new("Enthält").color(theme::TEXT_DIM))
                        .on_hover_text("Nur diese Werkzeuge werden gesetzt, alle anderen bleiben, wie sie sind; so lassen sich Stile übereinander anwenden");
                });
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(5.0, 5.0);
                    if edited.is_empty() {
                        ui.label(RichText::new("Nichts bearbeitet").color(theme::TEXT_FAINT));
                    }
                    for t in &edited {
                        let on = chosen.contains(t);
                        let hover = if on { "Wird gesetzt · Klick: weglassen" } else { "Bleibt, wie es ist · Klick: aufnehmen" };
                        if tool_chip(ui, t.label(), on).on_hover_text(hover).clicked() {
                            if on {
                                chosen.remove(t);
                            } else {
                                chosen.insert(*t);
                            }
                            lib.new_style_tools = Some(chosen.clone());
                        }
                    }
                });
            });
            ui.add_space(6.0);
            let name = lib.new_style.trim().to_string();
            let ready = !name.is_empty() && !chosen.is_empty();
            card_buttons(ui, |ui| {
                let b = egui::Button::new(RichText::new("Speichern").size(12.0).strong().color(Color32::WHITE)).fill(theme::ACCENT).corner_radius(7.0);
                if ui.add_enabled(ready, b).clicked() {
                    let tools: Vec<Tool> = chosen.iter().copied().collect();
                    save = Some(Style { name: name.clone(), adjustments: adjustments.clone(), tools: Some(tools) });
                }
                if ui.add(card_button("", "Abbrechen")).clicked() {
                    close = true;
                }
            });
        });
        if close {
            lib.new_style_open = false;
            lib.new_style.clear();
            lib.new_style_tools = None;
        }
        save
    }
}

/// A style as a tile: its preview, the check if it already acts, name (and for own styles how
/// many tools it sets); an own style shows a delete button while the pointer is on it.
fn style_tile(ui: &mut Ui, texture: Option<&TextureHandle>, e: &Entry, size: egui::Vec2, image_h: f32, on: bool) -> (egui::Response, bool) {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, on, &e.style.name));
    let p = ui.painter_at(rect.expand(1.0));
    let hot = resp.hovered();
    if on {
        p.rect_filled(rect, 9.0, CHOSEN_FILL);
        p.rect_stroke(rect, 9.0, Stroke::new(1.5, theme::ACCENT), StrokeKind::Inside);
    } else if hot {
        p.rect_filled(rect, 9.0, Color32::from_rgb(44, 44, 49));
    }
    let img = Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(rect.width() - 8.0, image_h - 4.0));
    paint_preview(ui, &p, texture, img, 6.0);
    if on {
        let c = pos2(img.right() - 13.0, img.top() + 13.0);
        p.circle_filled(c, 10.0, theme::ACCENT);
        p.text(c, egui::Align2::CENTER_CENTER, icon::CHECK, egui::FontId::proportional(12.0), Color32::WHITE);
    }
    let color = if hot || on { Color32::WHITE } else { theme::TEXT };
    let g = truncated(ui, e.style.name.clone(), 12.0, color, rect.width() - 8.0);
    p.galley(pos2(rect.center().x - g.size().x / 2.0, img.bottom() + 5.0), g, color);
    if !e.look {
        let g = truncated(ui, e.sub.clone(), 11.0, theme::TEXT_DIM, rect.width() - 8.0);
        p.galley(pos2(rect.center().x - g.size().x / 2.0, img.bottom() + 20.0), g, theme::TEXT_DIM);
    }
    let del = !e.look && ui.rect_contains_pointer(rect) && delete_button(ui, Rect::from_min_size(img.min + vec2(5.0, 5.0), vec2(24.0, 24.0)), &e.style.name);
    (resp, del)
}

/// A style as a row: small preview, name and description (or how many tools), the check.
fn style_row(ui: &mut Ui, texture: Option<&TextureHandle>, e: &Entry, on: bool) -> (egui::Response, bool) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, on, &e.style.name));
    let p = ui.painter_at(rect);
    if on {
        p.rect_filled(rect, 7.0, CHOSEN_FILL);
    } else if resp.hovered() {
        p.rect_filled(rect, 7.0, Color32::from_rgb(44, 44, 49));
    }
    let img = Rect::from_min_size(pos2(rect.left() + 6.0, rect.center().y - 14.0), vec2(42.0, 28.0));
    paint_preview(ui, &p, texture, img, 4.0);
    let mut right = rect.right() - 8.0;
    let pointer = ui.rect_contains_pointer(rect);
    let del = !e.look && pointer && delete_button(ui, Rect::from_center_size(pos2(right - 12.0, rect.center().y), vec2(24.0, 24.0)), &e.style.name);
    if !e.look {
        right -= 30.0;
    }
    if on {
        p.text(pos2(right - 7.0, rect.center().y), egui::Align2::CENTER_CENTER, icon::CHECK, egui::FontId::proportional(14.0), theme::ACCENT);
        right -= 22.0;
    }
    let x = img.right() + 10.0;
    let g = truncated(ui, e.style.name.clone(), 13.0, Color32::WHITE, right - x);
    p.galley(pos2(x, rect.top() + 4.0), g, Color32::WHITE);
    let g = truncated(ui, e.sub.clone(), 11.5, theme::TEXT_DIM, right - x);
    p.galley(pos2(x, rect.top() + 21.0), g, theme::TEXT_DIM);
    (resp, del)
}

/// The preview of a style (dark with "…" while it is rendered), filling `rect`.
fn paint_preview(ui: &Ui, p: &egui::Painter, texture: Option<&TextureHandle>, rect: Rect, radius: f32) {
    p.rect_filled(rect, radius, Color32::from_gray(24));
    match texture {
        Some(t) => {
            // Filled, cut to the shape of the box.
            let [w, h] = t.size().map(|v| v as f32);
            let (bw, bh) = (rect.width(), rect.height());
            let k = (bw / w).max(bh / h);
            let (uw, uh) = (bw / (w * k), bh / (h * k));
            let uv = Rect::from_center_size(pos2(0.5, 0.5), vec2(uw, uh));
            egui::Image::new((t.id(), rect.size())).uv(uv).corner_radius(radius).paint_at(ui, rect);
        }
        None => {
            p.text(rect.center(), egui::Align2::CENTER_CENTER, "…", egui::FontId::proportional(14.0), theme::TEXT_FAINT);
        }
    }
}

/// The delete button on an own style; true when clicked.
fn delete_button(ui: &mut Ui, rect: Rect, name: &str) -> bool {
    let resp = ui.interact(rect, ui.id().with(("style-delete", name)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{name} löschen")));
    let p = ui.painter();
    p.rect_filled(rect, 6.0, if resp.hovered() { Color32::from_rgb(58, 58, 64) } else { Color32::from_black_alpha(200) });
    p.text(rect.center(), egui::Align2::CENTER_CENTER, icon::TRASH, egui::FontId::proportional(13.0), if resp.hovered() { Color32::WHITE } else { theme::TEXT });
    resp.on_hover_text("Stil löschen …").clicked()
}

/// "Als Stil sichern": dashed, the size of a tile or a row.
fn add_tile(ui: &mut Ui, size: egui::Vec2) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Als Stil sichern"));
    let color = if resp.hovered() { theme::TEXT_DIM } else { theme::BORDER_STRONG };
    let pts = widgets::rounded_rect_points(rect.shrink(0.5), 9.0);
    ui.painter().extend(widgets::dashed_closed(&pts, Stroke::new(1.0, color), 4.0, 3.0));
    let text = if resp.hovered() { Color32::WHITE } else { theme::TEXT_DIM };
    if size.y > 60.0 {
        ui.painter().text(rect.center() - vec2(0.0, 10.0), egui::Align2::CENTER_CENTER, icon::PLUS, egui::FontId::proportional(18.0), text);
        ui.painter().text(rect.center() + vec2(0.0, 12.0), egui::Align2::CENTER_CENTER, "Als Stil sichern", egui::FontId::proportional(12.0), text);
    } else {
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, format!("{}  Als Stil sichern", icon::PLUS), egui::FontId::proportional(12.5), text);
    }
    resp.on_hover_text("Speichert die Einstellungen des Bildes als Stil").on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A tool of a new style: filled while it is taken.
fn tool_chip(ui: &mut Ui, name: &str, on: bool) -> egui::Response {
    let (fill, stroke, text) = if on { (theme::ACCENT_SOFT, CHOSEN_EDGE, Color32::WHITE) } else { (Color32::TRANSPARENT, theme::BORDER_STRONG, theme::TEXT_DIM) };
    let b = egui::Button::new(RichText::new(name).size(12.0).color(text)).fill(fill).stroke(Stroke::new(1.0, stroke)).corner_radius(11.0).min_size(vec2(0.0, 22.0));
    let r = ui.add(b);
    r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, name));
    r
}
