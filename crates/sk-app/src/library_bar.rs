//! The bar above the images: search, the filters as chips (an active one in the accent color,
//! with ×, and how many images are left) and the sort order. Visible
//! with every tab, so a filter never goes unnoticed.

use egui::{Color32, Popup, PopupCloseBehavior, Response, RichText, Sense, Stroke, StrokeKind, Ui, vec2};
use sk_core::catalog::{CullFilter, Filter, Scope, SortKey};
use sk_core::sidecar::ColorLabel;

use crate::app::App;
use crate::theme::{self, icon};
use crate::widgets;

const BAR_HEIGHT: f32 = 28.0;

/// What a click in the bar asks for, carried out after drawing.
#[derive(Default)]
struct Changes {
    refilter: bool,
    resort: bool,
    reset: bool,
    open: Option<Scope>,
    analyze: Option<bool>,
    filter_album: Option<String>,
}

impl App {
    /// Whether any filter or search limits the images shown.
    pub fn filter_active(&self) -> bool {
        !self.filter.is_empty() || self.content.is_active()
    }

    pub fn library_bar(&mut self, ui: &mut Ui) {
        let mut ch = Changes::default();
        let filtered = self.filter_active();
        crate::tour::target(ui.ctx(), "library_bar", ui.available_rect_before_wrap().with_max_y(ui.cursor().top() + BAR_HEIGHT + 12.0));
        ui.add_space(6.0);
        // One row: search and filters on the left (wrapping when narrow), the order on the right.
        ui.horizontal(|ui| {
            let right = 40.0;
            let left = (ui.available_width() - right).max(200.0);
            ui.allocate_ui_with_layout(vec2(left, BAR_HEIGHT), egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true), |ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                let active = !self.filter.text.trim().is_empty();
                let resp = search_field(ui, &mut self.filter.text, icon::MAGNIFYING_GLASS, theme::TEXT_DIM, "Name, Kamera, Keyword", 190.0, active)
                    .on_hover_text("Sucht in Dateiname, Kamera, Objektiv und Keywords (Strg+F)");
                if std::mem::take(&mut self.library_ui.focus_search) {
                    resp.request_focus();
                }
                ch.refilter |= resp.changed();
                self.content_field(ui);
                separator(ui);
                self.filter_pills(ui, &mut ch);
                if filtered {
                    separator(ui);
                    count_badge(ui, &format!("{} von {}", self.visible.len(), self.entries.len()));
                    self.filter_album_button(ui, &mut ch);
                    let b = egui::Button::new(RichText::new(format!("{}  Zurücksetzen", icon::X)).color(theme::ACCENT_TEXT)).frame_when_inactive(false).min_size(vec2(0.0, BAR_HEIGHT));
                    if ui.add(b).on_hover_text("Alle Filter und die Suche aufheben").clicked() {
                        ch.reset = true;
                    }
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                self.sort_part(ui, &mut ch);
            });
        });
        ui.add_space(6.0);
        // What is going on behind it, in a line of its own.
        let browsed = if self.session.is_none() && matches!(self.scope, Scope::Folder { .. }) { self.entries.iter().filter(|e| e.id == 0).count() } else { 0 };
        if browsed > 0 {
            info_line(ui, |ui| {
                ui.label(RichText::new(icon::INFO).color(theme::TEXT_DIM));
                ui.label(match browsed {
                    n if n == self.entries.len() => "Die Bilder dieses Ordners sind nur durchsucht, nicht im Katalog.".to_string(),
                    1 => "1 Bild dieses Ordners ist nur durchsucht, nicht im Katalog.".to_string(),
                    n => format!("{n} Bilder dieses Ordners sind nur durchsucht, nicht im Katalog."),
                });
                if ui.small_button("Importieren …").on_hover_text("Bilder dieses Ordners und seiner Unterordner in den Katalog aufnehmen").clicked() {
                    let folder = self.folder.clone();
                    self.open_import(folder, true);
                }
            });
        }
        if self.content.is_active() {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(ui.available_rect_before_wrap()));
            let shown = info_line(&mut child, |ui| self.content_status(ui));
            if shown {
                ui.advance_cursor_after_rect(child.min_rect());
            }
        }

        if let Some(name) = ch.filter_album {
            self.create_filter_album(&name);
        }
        if let Some(again) = ch.analyze {
            self.analyze_view(again);
        }
        if ch.reset {
            self.filter = Filter::default();
            // Refilters as well.
            self.clear_content_search();
        } else if let Some(s) = ch.open {
            self.open_scope(s);
        } else if ch.resort {
            self.resort();
        } else if ch.refilter {
            self.refilter();
        }
    }

    /// Bewertung, Farb-Label, Sichtung: a chip each, with its menu (keywords are found by the
    /// search field).
    fn filter_pills(&mut self, ui: &mut Ui, ch: &mut Changes) {
        // Rating.
        let r = self.filter.min_rating;
        let text = if r == 0 { "Bewertung".to_string() } else { rating_text(r) };
        let counts: Vec<usize> = (1..=5).map(|n| self.entries.iter().filter(|e| e.rating >= n).count()).collect();
        let (resp, clear) = pill(ui, &text, r > 0, None, "Bewertungsfilter aufheben");
        if clear {
            self.filter.min_rating = 0;
            ch.refilter = true;
        }
        pill_menu(&resp, |ui| {
            if option(ui, r == 0, "Alle", None, "") {
                self.filter.min_rating = 0;
                ch.refilter = true;
            }
            for n in 1..=5u8 {
                if option(ui, r == n, &rating_text(n), None, &counts[n as usize - 1].to_string()) {
                    self.filter.min_rating = n;
                    ch.refilter = true;
                }
            }
        });

        // Color label.
        let l = self.filter.color_label;
        let text = l.map_or("Farb-Label".to_string(), |l| l.label().to_string());
        let (resp, clear) = pill(ui, &text, l.is_some(), l.and_then(label_color), "Labelfilter aufheben");
        if clear {
            self.filter.color_label = None;
            ch.refilter = true;
        }
        pill_menu(&resp, |ui| {
            if option(ui, l.is_none(), "Alle", None, "") {
                self.filter.color_label = None;
                ch.refilter = true;
            }
            for c in ColorLabel::ALL {
                let n = self.entries.iter().filter(|e| e.color_label == c).count();
                if option(ui, l == Some(c), c.label(), Some(label_color(c).unwrap_or(theme::TEXT_FAINT)), &n.to_string()) {
                    self.filter.color_label = Some(c);
                    ch.refilter = true;
                }
            }
        });

        // Culling, with the analysis it needs.
        let c = self.filter.culling;
        let text = c.map_or("Sichtung".to_string(), |c| c.label().to_string());
        let (resp, clear) = pill(ui, &text, c.is_some(), None, "Sichtungsfilter aufheben");
        if clear {
            self.filter.culling = None;
            ch.refilter = true;
        }
        pill_menu(&resp, |ui| {
            ui.set_width(250.0);
            if option(ui, c.is_none(), "Alle", None, "") {
                self.filter.culling = None;
                ch.refilter = true;
            }
            for f in CullFilter::OFFERED {
                let n = self.entries.iter().filter(|e| f.matches(e.culling.as_ref())).count();
                if option_detail(ui, c == Some(f), f.label(), f.detail(), &n.to_string()) {
                    self.filter.culling = Some(f);
                    ch.refilter = true;
                }
            }
            ui.separator();
            let shown = self.visible.len();
            let analysed = self.visible.iter().filter(|&&i| self.entries[i].culling.is_some()).count();
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{analysed} von {shown} in dieser Ansicht analysiert")).small().color(theme::TEXT_DIM));
                ui.label(RichText::new(icon::SPARKLE).small().color(theme::AI)).on_hover_text(widgets::AI_HINT);
            });
            ui.add(egui::ProgressBar::new(if shown == 0 { 0.0 } else { analysed as f32 / shown as f32 }).desired_height(4.0));
            let help = "Erkennt Gesichter, geschlossene Augen und unscharfe Bilder und ordnet Gesichter Personen zu.\nBeim ersten Mal werden etwa 40 MB geladen.";
            if analysed < shown {
                if ui.add_sized([ui.available_width(), 26.0], egui::Button::new(format!("{}  Ansicht analysieren", icon::SCAN))).on_hover_text(help).clicked() {
                    ch.analyze = Some(false);
                    ui.close();
                }
            } else if shown > 0 && ui.add_sized([ui.available_width(), 26.0], egui::Button::new(format!("{}  Erneut analysieren", icon::ARROW_CLOCKWISE)).frame_when_inactive(false)).on_hover_text("Alle Bilder dieser Ansicht neu analysieren").clicked() {
                ch.analyze = Some(true);
                ui.close();
            }
        });
    }

    /// "Als Filteralbum": asks for a name, then saves the filter as an album.
    fn filter_album_button(&mut self, ui: &mut Ui, ch: &mut Changes) {
        let b = egui::Button::new(RichText::new(format!("{}  Als Filteralbum", icon::SLIDERS_HORIZONTAL)).color(theme::TEXT)).frame_when_inactive(false).min_size(vec2(0.0, BAR_HEIGHT));
        let resp = ui.add_enabled(!self.filter.is_empty(), b).on_hover_text("Album, das immer die Bilder dieses Filters zeigt").on_disabled_hover_text("Die Bildinhalt-Suche lässt sich nicht als Album speichern");
        Popup::menu(&resp).style(theme::menu_style).close_behavior(PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            ui.set_width(240.0);
            ui.label(RichText::new("Filteralbum anlegen").small().color(theme::TEXT_DIM));
            let field = ui.add(egui::TextEdit::singleline(&mut self.library_ui.new_album).hint_text("Name").desired_width(f32::INFINITY));
            let name = self.library_ui.new_album.trim().to_string();
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.add_enabled(!name.is_empty(), egui::Button::new("Anlegen")).clicked() || enter) && !name.is_empty() {
                ch.filter_album = Some(name);
                self.library_ui.new_album.clear();
                ui.close();
            }
        });
    }

    /// The sort order as an icon; its menu offers the key and the direction.
    fn sort_part(&mut self, ui: &mut Ui, ch: &mut Changes) {
        let dir = if self.sort_desc { icon::SORT_DESCENDING } else { icon::SORT_ASCENDING };
        let way = if self.sort_desc { "absteigend" } else { "aufsteigend" };
        let b = egui::Button::new(RichText::new(dir).size(16.0)).frame_when_inactive(false).min_size(vec2(BAR_HEIGHT, BAR_HEIGHT));
        let resp = ui.add(b).on_hover_text(format!("Sortierung: {}, {way}", self.sort.label()));
        widgets::menu_popup(&resp).show(|ui| {
            ui.set_width(180.0);
            for s in SortKey::ALL {
                if option(ui, self.sort == s, s.label(), None, "") {
                    self.sort = s;
                    ch.resort = true;
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                for (desc, ic, hover) in [(false, icon::SORT_ASCENDING, "Aufsteigend"), (true, icon::SORT_DESCENDING, "Absteigend")] {
                    let b = egui::Button::selectable(self.sort_desc == desc, RichText::new(ic).size(16.0)).min_size(vec2(36.0, 28.0));
                    if ui.add(b).on_hover_text(hover).clicked() {
                        self.sort_desc = desc;
                        ch.resort = true;
                        ui.close();
                    }
                }
            });
        });
    }
}

fn rating_text(n: u8) -> String {
    if n >= 5 { "= 5 ★".to_string() } else { format!("≥ {n} ★") }
}

fn label_color(l: ColorLabel) -> Option<Color32> {
    l.rgb().map(|[r, g, b]| Color32::from_rgb(r, g, b))
}

fn separator(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(1.0, BAR_HEIGHT), Sense::hover());
    ui.painter().vline(r.center().x, r.y_range().shrink(5.0), Stroke::new(1.0, theme::BORDER));
}

/// A line below the bar (a hint and what to do about it); returns what `body` returns.
fn info_line<R>(ui: &mut Ui, body: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(theme::PANEL)
        .inner_margin(egui::Margin::symmetric(10, 5))
        .corner_radius(6.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(body).inner
        })
        .inner
}

/// "7 von 12" in the accent colors.
fn count_badge(ui: &mut Ui, text: &str) {
    let galley = ui.painter().layout_no_wrap(text.to_string(), egui::FontId::proportional(12.0), theme::ACCENT_TEXT);
    let (r, _) = ui.allocate_exact_size(vec2(galley.size().x + 18.0, 22.0), Sense::hover());
    ui.painter().rect_filled(r, 11.0, theme::ACCENT_SOFT);
    ui.painter().galley(r.center() - galley.size() / 2.0, galley, theme::ACCENT_TEXT);
}

/// A search field with an icon in front; outlined in the accent color while it filters.
pub(crate) fn search_field(ui: &mut Ui, text: &mut String, ic: &str, ic_color: Color32, hint: &str, width: f32, active: bool) -> Response {
    let resp = ui.add(
        egui::TextEdit::singleline(text)
            .hint_text(hint)
            .desired_width(width)
            .min_size(vec2(width, BAR_HEIGHT))
            .vertical_align(egui::Align::Center)
            .margin(egui::Margin { left: 26, right: 6, top: 0, bottom: 0 })
            .background_color(theme::PANEL),
    );
    ui.painter().text(resp.rect.left_center() + vec2(13.0, 0.0), egui::Align2::CENTER_CENTER, ic, egui::FontId::proportional(13.0), ic_color);
    let stroke = if active { theme::ACCENT } else if resp.has_focus() { theme::TEXT_DIM } else { theme::BORDER_STRONG };
    ui.painter().rect_stroke(resp.rect, 6.0, Stroke::new(1.0, stroke), StrokeKind::Inside);
    resp
}

/// An active filter: accent chip, the text (a click opens its menu) and × (returns true when
/// clicked).
pub(crate) fn active_chip(ui: &mut Ui, text: &str, swatch: Option<Color32>, clear_hover: &str) -> (Response, bool) {
    let mut cleared = false;
    let inner = egui::Frame::new()
        .fill(theme::ACCENT_SOFT)
        .stroke(Stroke::new(1.0, theme::ACCENT))
        .corner_radius(BAR_HEIGHT / 2.0)
        .inner_margin(egui::Margin { left: 10, right: 2, top: 0, bottom: 0 })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.set_height(BAR_HEIGHT - 2.0);
            if let Some(c) = swatch {
                let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 5.0, c);
            }
            let main = ui.add(egui::Label::new(RichText::new(text).color(Color32::WHITE)).sense(Sense::click()).selectable(false));
            let x = egui::Button::new(RichText::new(icon::X).size(11.0).color(theme::ACCENT_TEXT)).frame_when_inactive(false).min_size(vec2(22.0, 22.0));
            if ui.add(x).on_hover_text(clear_hover).clicked() {
                cleared = true;
            }
            main
        });
    let resp = inner.inner.union(inner.response);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    (resp, cleared)
}

/// A filter as a chip: inactive, an outlined button with ▾; active, an [`active_chip`].
fn pill(ui: &mut Ui, text: &str, active: bool, swatch: Option<Color32>, clear_hover: &str) -> (Response, bool) {
    if active {
        return active_chip(ui, text, swatch, clear_hover);
    }
    let b = egui::Button::new(RichText::new(format!("{text}  {}", icon::CARET_DOWN)).color(theme::TEXT))
        .frame_when_inactive(true)
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::new(1.0, theme::BORDER_STRONG))
        .corner_radius(BAR_HEIGHT / 2.0)
        .min_size(vec2(0.0, BAR_HEIGHT));
    (ui.add(b), false)
}

fn pill_menu(resp: &Response, body: impl FnOnce(&mut Ui)) {
    Popup::menu(resp).style(theme::menu_style).close_behavior(PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        // A fixed width: the entries take what is there, which would let the menu grow.
        ui.set_width(220.0);
        body(ui);
    });
}

/// An entry of a filter menu: a check if chosen, an optional color dot, the text and the number
/// of images on the right. Closes the menu when clicked.
fn option(ui: &mut Ui, on: bool, text: &str, swatch: Option<Color32>, count: &str) -> bool {
    option_inner(ui, on, text, None, swatch, count)
}

fn option_detail(ui: &mut Ui, on: bool, text: &str, detail: &str, count: &str) -> bool {
    option_inner(ui, on, text, Some(detail), None, count)
}

fn option_inner(ui: &mut Ui, on: bool, text: &str, detail: Option<&str>, swatch: Option<Color32>, count: &str) -> bool {
    let h = if detail.is_some() { 38.0 } else { 26.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 4.0, Color32::from_rgb(58, 58, 64));
    }
    let mut x = rect.left() + 8.0;
    if on {
        p.text(egui::pos2(x + 6.0, rect.center().y), egui::Align2::CENTER_CENTER, icon::CHECK, egui::FontId::proportional(12.0), theme::ACCENT);
    }
    x += 20.0;
    if let Some(c) = swatch {
        p.circle_filled(egui::pos2(x + 5.0, rect.center().y), 5.0, c);
        x += 16.0;
    }
    let color = if on { Color32::WHITE } else { theme::TEXT };
    match detail {
        Some(d) => {
            p.text(egui::pos2(x, rect.top() + 11.0), egui::Align2::LEFT_CENTER, text, egui::FontId::proportional(13.0), color);
            p.text(egui::pos2(x, rect.bottom() - 11.0), egui::Align2::LEFT_CENTER, d, egui::FontId::proportional(11.0), theme::TEXT_DIM);
        }
        None => {
            p.text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, text, egui::FontId::proportional(13.0), color);
        }
    }
    if !count.is_empty() {
        p.text(egui::pos2(rect.right() - 8.0, rect.center().y), egui::Align2::RIGHT_CENTER, count, egui::FontId::proportional(11.5), theme::TEXT_DIM);
    }
    if resp.clicked() {
        ui.close();
        return true;
    }
    false
}
