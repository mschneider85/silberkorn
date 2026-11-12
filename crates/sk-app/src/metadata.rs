//! The Metadaten tab: the image (name, date, format), rating and color label, keywords as chips,
//! the capture values as tiles, gear and file. With several images selected, what only some of
//! them have is drawn dashed: stars up to the highest rating, rings at the labels present,
//! keywords with "1/3".

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use egui::{Color32, RichText, Sense, Stroke, Ui, vec2};
use sk_core::sidecar::ColorLabel;

use crate::app::App;
use crate::theme::{self, icon};
use crate::widgets;

/// Rating, label and keywords of the selection.
pub(crate) struct MetaSummary {
    /// The images it is about (the selection, or the open image).
    pub paths: Vec<PathBuf>,
    /// Lowest and highest rating (equal: all have it).
    pub rating: (u8, u8),
    /// The labels present (one: all have it).
    pub labels: Vec<ColorLabel>,
    /// Each keyword (sorted) and how many of the images have it.
    pub keywords: Vec<(String, usize)>,
}

impl MetaSummary {
    pub fn n(&self) -> usize {
        self.paths.len()
    }
}

/// What a click on a keyword chip asks for.
enum ChipClick {
    Remove,
    GiveAll,
}

impl App {
    /// Rating, label and keywords of the selection (of the open image, or else of the selected
    /// one; they are set for the whole selection either way).
    pub(crate) fn meta_summary(&self) -> Option<MetaSummary> {
        let paths = self.selected_paths();
        if paths.len() <= 1 {
            let (rating, label, keywords, path) = match &self.current {
                Some(cur) => (cur.rating, cur.color_label, cur.keywords.clone(), cur.meta.path.clone()),
                None => {
                    let e = self.entries.iter().find(|e| Some(&e.path) == paths.first())?;
                    (e.rating, e.color_label, e.keywords.clone(), e.path.clone())
                }
            };
            return Some(MetaSummary { paths: vec![path], rating: (rating, rating), labels: vec![label], keywords: keywords.into_iter().map(|k| (k, 1)).collect() });
        }
        let set: HashSet<&PathBuf> = paths.iter().collect();
        let mut rating = (u8::MAX, 0);
        let mut labels = Vec::new();
        // Keyed in lower case: the first spelling met is shown.
        let mut keywords: BTreeMap<String, (String, usize)> = BTreeMap::new();
        for e in self.entries.iter().filter(|e| set.contains(&e.path)) {
            rating = (rating.0.min(e.rating), rating.1.max(e.rating));
            if !labels.contains(&e.color_label) {
                labels.push(e.color_label);
            }
            for k in &e.keywords {
                keywords.entry(k.to_lowercase()).or_insert_with(|| (k.clone(), 0)).1 += 1;
            }
        }
        if rating.0 > rating.1 {
            rating = (0, 0);
        }
        Some(MetaSummary { paths, rating, labels, keywords: keywords.into_values().collect() })
    }

    pub(crate) fn metadata_tab(&mut self, ui: &mut Ui) {
        let Some(sum) = self.meta_summary() else {
            widgets::no_image(ui);
            return;
        };
        ui.add_space(2.0);
        self.meta_header(ui, &sum);

        // Rating and color label.
        let (lo, hi) = sum.rating;
        let mixed = (lo != hi).then_some(hi);
        widgets::nav_header(ui, "Bewertung", "Bewertung", |ui| {
            if mixed.is_some() {
                ui.label(RichText::new(format!("{lo}–{hi} ★")).size(11.5).color(theme::TEXT_DIM));
            }
        });
        let mut new_rating = None;
        let mut new_label = None;
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            new_rating = widgets::rating_stars(ui, lo, mixed, 24.0);
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.spacing_mut().item_spacing.x = 4.0;
            let one = sum.labels.len() == 1;
            for l in ColorLabel::ALL {
                let has = sum.labels.contains(&l);
                if widgets::color_label_dot_mixed(ui, l, one && has, !one && has).clicked() {
                    new_label = Some(l);
                }
            }
        });
        if let Some(r) = new_rating {
            self.set_rating(r);
        }
        if let Some(l) = new_label {
            self.set_color_label(l);
        }

        widgets::nav_header(ui, "Keywords", "Keywords", |_| {});
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width() - 6.0);
                self.keywords_ui(ui);
                if sum.n() == 1 {
                    self.keyword_suggestions_ui(ui);
                }
            });
        });

        let Some(cur) = self.current.as_ref() else { return };
        let info = cur.meta.info.clone();
        let path = cur.meta.path.clone();
        let of_primary = |ui: &mut Ui| {
            if sum.n() > 1 {
                ui.label(RichText::new("des Hauptbilds").size(11.5).color(theme::TEXT_DIM));
            }
        };
        widgets::nav_header(ui, "Aufnahme", "Aufnahme", of_primary);
        let tiles = [
            (icon::APERTURE, "Blende", info.aperture.clone()),
            (icon::TIMER, "Belichtungszeit", info.exposure_time.clone()),
            (icon::SUN_DIM, "ISO", info.iso.map(|v| v.to_string())),
            (icon::RULER, "Brennweite", info.focal_length.clone()),
        ];
        for row in tiles.chunks(2) {
            ui.horizontal(|ui| {
                ui.add_space(6.0);
                ui.spacing_mut().item_spacing.x = 6.0;
                let w = ((ui.available_width() - 10.0) / 2.0).max(60.0);
                for (ic, label, value) in row {
                    value_tile(ui, w, ic, label, value.as_deref().unwrap_or("–"));
                }
            });
            ui.add_space(2.0);
        }
        ui.add_space(4.0);
        let camera = format!("{} {}", info.make, info.model).trim().to_string();
        info_row(ui, icon::CAMERA, "Kamera", if camera.is_empty() { "–" } else { &camera }, None);
        info_row(ui, icon::CROSSHAIR, "Objektiv", info.lens.as_deref().unwrap_or("–"), None);

        widgets::nav_header(ui, "Datei", "Datei", of_primary);
        let (w, h) = (info.width, info.height);
        info_row(ui, icon::IMAGE, "Sensor", &format!("{w} × {h}"), Some(&sensor_detail(w, h)));
        let size = self.file_size(&path).map_or_else(|| "–".to_string(), file_size_text);
        info_row(ui, icon::HARD_DRIVE, "Größe", &size, None);
        if let Some(dir) = path.parent() {
            self.folder_row(ui, dir);
        }
        ui.add_space(12.0);
    }

    /// The card at the top: name and format, the capture date (several images: their names).
    fn meta_header(&self, ui: &mut Ui, sum: &MetaSummary) {
        let names: Vec<String> = sum.paths.iter().map(|p| sk_core::variant::display_name(p)).collect();
        let mut formats: Vec<String> = Vec::new();
        for p in &sum.paths {
            let ext = p.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();
            if !ext.is_empty() && !formats.contains(&ext) {
                formats.push(ext);
            }
        }
        let (title, badge, sub) = if sum.n() == 1 {
            let raw = sk_core::raw::is_raw_file(&sum.paths[0]);
            let badge = match formats.first() {
                Some(f) if raw => format!("{f} · RAW"),
                Some(f) => f.clone(),
                None => String::new(),
            };
            let date = self.current.as_ref().and_then(|c| c.meta.info.date.as_deref().map(crate::browser::exif_date));
            (names[0].clone(), badge, date.map(|d| (icon::CALENDAR_BLANK, d)))
        } else {
            let badge = if formats.len() > 3 { format!("{} Formate", formats.len()) } else { formats.join(" · ") };
            (widgets::image_count(sum.n()), badge, Some((icon::IMAGES_SQUARE, names.join(", "))))
        };
        // Painted, so that nothing in it can widen the panel.
        let width = ui.available_width() - 8.0;
        let height = if sub.is_some() { 62.0 } else { 44.0 };
        let (outer, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let rect = egui::Rect::from_min_size(outer.min + vec2(4.0, 0.0), vec2(width, height));
        let p = ui.painter();
        p.rect_filled(rect, 10.0, theme::CARD);
        let mut right = rect.right() - 12.0;
        if !badge.is_empty() {
            let galley = p.layout_no_wrap(badge, egui::FontId::proportional(10.5), theme::ACCENT_TEXT);
            let r = egui::Rect::from_min_size(egui::pos2(right - galley.size().x - 14.0, rect.top() + 22.0 - 10.0), galley.size() + vec2(14.0, 6.0));
            p.rect_filled(r, 5.0, theme::ACCENT_SOFT);
            p.galley(r.center() - galley.size() / 2.0, galley, theme::ACCENT_TEXT);
            right = r.left() - 10.0;
        }
        let truncated = |text: String, size: f32, color: Color32, width: f32| {
            let mut job = egui::text::LayoutJob::simple_singleline(text, egui::FontId::proportional(size), color);
            job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(20.0));
            ui.painter().layout_job(job)
        };
        let name = truncated(title, 16.5, Color32::WHITE, right - rect.left() - 14.0);
        ui.painter().galley(egui::pos2(rect.left() + 14.0, rect.top() + 22.0 - name.size().y / 2.0), name, Color32::WHITE);
        if let Some((ic, text)) = sub {
            let y = rect.top() + 44.0;
            ui.painter().text(egui::pos2(rect.left() + 14.0, y), egui::Align2::LEFT_CENTER, ic, egui::FontId::proportional(12.5), theme::TEXT_DIM);
            let line = truncated(text, 12.0, theme::TEXT_DIM, rect.width() - 46.0);
            ui.painter().galley(egui::pos2(rect.left() + 32.0, y - line.size().y / 2.0), line, theme::TEXT_DIM);
        }
    }

    /// The keywords of the selection as chips (× removes; one only some images have is dashed,
    /// with "1/3" giving it to all), the field to add one and matches from the catalog.
    pub(crate) fn keywords_ui(&mut self, ui: &mut Ui) {
        let Some(sum) = self.meta_summary() else { return };
        let n = sum.n();
        let mut click = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for (k, have) in &sum.keywords {
                if let Some(c) = keyword_chip(ui, k, *have, n) {
                    click = Some((k.clone(), c));
                }
            }
            if sum.keywords.is_empty() {
                ui.label(RichText::new("Noch keine Keywords").size(12.0).color(theme::TEXT_FAINT));
            }
        });
        ui.add_space(6.0);
        let width = ui.available_width();
        let resp = crate::library_bar::search_field(ui, &mut self.keyword_input, icon::PLUS, theme::TEXT_DIM, "Keyword hinzufügen", width, false)
            .on_hover_text("Enter fügt es hinzu");
        let mut add = None;
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            add = Some(self.keyword_input.clone());
            resp.request_focus();
        }
        // Matches from the catalog, without those all images have.
        let typed = self.keyword_input.trim().to_lowercase();
        if !typed.is_empty() {
            let all: Vec<String> = sum.keywords.iter().filter(|(_, c)| *c == n).map(|(k, _)| k.to_lowercase()).collect();
            let matches: Vec<String> =
                self.lists.keywords.iter().map(|(k, _)| k).filter(|k| k.to_lowercase().starts_with(&typed) && !all.contains(&k.to_lowercase())).take(6).cloned().collect();
            if !matches.is_empty() {
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for k in matches {
                        if suggestion_chip(ui, &k).on_hover_text("Hinzufügen").clicked() {
                            add = Some(k);
                        }
                    }
                });
            }
        }
        match click {
            Some((k, ChipClick::Remove)) => self.remove_keyword(&k),
            Some((k, ChipClick::GiveAll)) => self.add_keyword(&k),
            None => {}
        }
        if let Some(k) = add {
            self.add_keyword(&k);
            self.keyword_input.clear();
        }
    }

    /// The size of the file on disk (read once per file).
    fn file_size(&mut self, path: &Path) -> Option<u64> {
        if let Some((p, size)) = &self.library_ui.file_size
            && p == path
        {
            return *size;
        }
        let size = std::fs::metadata(path).ok().map(|m| m.len());
        self.library_ui.file_size = Some((path.to_path_buf(), size));
        size
    }

    /// The folder's name (the whole path on hover) and a button showing it in the Explorer.
    fn folder_row(&mut self, ui: &mut Ui, dir: &Path) {
        let name = dir.file_name().map_or_else(|| dir.display().to_string(), |n| n.to_string_lossy().to_string());
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
        let button = egui::Rect::from_min_size(egui::pos2(rect.right() - 110.0, rect.center().y - 13.0), vec2(104.0, 26.0));
        let b = egui::Button::new(RichText::new(format!("{}  Im Explorer", icon::ARROW_SQUARE_OUT)).size(12.0))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0, theme::BORDER_STRONG))
            .corner_radius(7.0);
        let reveal = ui.put(button, b).on_hover_text("Öffnet den Ordner im Explorer").clicked();
        let x = row_start(ui, rect, icon::FOLDER, "Ordner");
        let mut job = egui::text::LayoutJob::simple_singleline(name, egui::FontId::proportional(13.0), Color32::WHITE);
        job.wrap = egui::text::TextWrapping::truncate_at_width((button.left() - 10.0 - x).max(20.0));
        let galley = ui.painter().layout_job(job);
        ui.painter().galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, Color32::WHITE);
        resp.on_hover_text(dir.display().to_string());
        if reveal && let Err(e) = crate::session::reveal(dir) {
            self.report_error(format!("Ordner nicht geöffnet: {e}"));
        }
    }
}

/// A capture value large, with its icon and name above.
fn value_tile(ui: &mut Ui, width: f32, ic: &str, label: &str, value: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 58.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 9.0, theme::CARD);
    let top = rect.left_top() + vec2(12.0, 16.0);
    p.text(top, egui::Align2::LEFT_CENTER, ic, egui::FontId::proportional(12.5), theme::TEXT_DIM);
    p.text(top + vec2(18.0, 0.0), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(11.0), theme::TEXT_DIM);
    let galley = p.layout_no_wrap(value.to_string(), egui::FontId::proportional(19.0), Color32::WHITE);
    p.galley(rect.left_top() + vec2(12.0, 26.0), galley, Color32::WHITE);
}

/// A row: icon, name and value (with more, dimmed, after it); the whole value on hover.
fn info_row(ui: &mut Ui, ic: &str, label: &str, value: &str, more: Option<&str>) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    let x = row_start(ui, rect, ic, label);
    let mut job = egui::text::LayoutJob::default();
    job.append(value, 0.0, egui::TextFormat::simple(egui::FontId::proportional(13.0), Color32::WHITE));
    if let Some(m) = more {
        job.append(m, 5.0, egui::TextFormat::simple(egui::FontId::proportional(13.0), theme::TEXT_DIM));
    }
    job.wrap = egui::text::TextWrapping::truncate_at_width((rect.right() - 8.0 - x).max(20.0));
    let galley = ui.painter().layout_job(job);
    let cut = galley.elided;
    ui.painter().galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, Color32::WHITE);
    if cut {
        resp.on_hover_text(value);
    }
}

/// Icon and name of a row in `rect`; returns where its value starts.
fn row_start(ui: &Ui, rect: egui::Rect, ic: &str, label: &str) -> f32 {
    let y = rect.center().y;
    let p = ui.painter();
    p.text(egui::pos2(rect.left() + 16.0, y), egui::Align2::CENTER_CENTER, ic, egui::FontId::proportional(14.0), theme::TEXT_DIM);
    p.text(egui::pos2(rect.left() + 34.0, y), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(13.0), theme::TEXT_DIM);
    rect.left() + 108.0
}

/// "· 24.0 MP · 3:2" (the side ratio only when it is a usual one).
fn sensor_detail(w: usize, h: usize) -> String {
    if w == 0 || h == 0 {
        return String::new();
    }
    let mp = format!("· {:.1} MP", (w * h) as f64 / 1e6);
    let (long, short) = (w.max(h) as f64, w.min(h) as f64);
    let usual = [(1, 1), (5, 4), (4, 3), (3, 2), (16, 9), (2, 1)];
    match usual.iter().find(|(a, b)| ((long / short) / (*a as f64 / *b as f64) - 1.0).abs() < 0.015) {
        Some((a, b)) if w >= h => format!("{mp} · {a}:{b}"),
        Some((a, b)) => format!("{mp} · {b}:{a}"),
        None => mp,
    }
}

fn file_size_text(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else {
        format!("{:.0} KB", (b / 1e3).max(1.0))
    }
}

/// A keyword as a chip: all images have it (accent), or only `have` of `n` (dashed, with
/// "have/n" to give it to all).
fn keyword_chip(ui: &mut Ui, text: &str, have: usize, n: usize) -> Option<ChipClick> {
    let partial = have < n;
    let mut click = None;
    let frame = egui::Frame::new()
        .fill(if partial { Color32::TRANSPARENT } else { theme::ACCENT_SOFT })
        .stroke(if partial { Stroke::NONE } else { Stroke::new(1.0, Color32::from_rgb(61, 95, 138)) })
        .corner_radius(12.0)
        .inner_margin(egui::Margin { left: 10, right: 2, top: 0, bottom: 0 });
    let inner = frame.show(ui, |ui| {
        ui.set_height(24.0);
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.label(RichText::new(text).size(12.5).color(if partial { theme::TEXT } else { theme::ACCENT_TEXT }));
        if partial {
            let b = egui::Button::new(RichText::new(format!("{have}/{n}")).size(11.0).color(theme::TEXT_DIM)).frame_when_inactive(false).min_size(vec2(0.0, 20.0));
            if ui.add(b).on_hover_text(format!("Nur in {have} von {n} Bildern. Klick: allen geben")).clicked() {
                click = Some(ChipClick::GiveAll);
            }
        }
        let x = egui::Button::new(RichText::new(icon::X).size(10.0).color(theme::TEXT_DIM)).frame_when_inactive(false).corner_radius(10.0).min_size(vec2(20.0, 20.0));
        if ui.add(x).on_hover_text("Entfernen").clicked() {
            click = Some(ChipClick::Remove);
        }
    });
    if partial {
        let pts = widgets::rounded_rect_points(inner.response.rect.shrink(0.5), 12.0);
        ui.painter().extend(widgets::dashed_closed(&pts, Stroke::new(1.0, theme::TEXT_FAINT), 3.0, 2.5));
    }
    click
}

/// A proposed keyword: dashed, a click adds it.
pub(crate) fn suggestion_chip(ui: &mut Ui, text: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), egui::FontId::proportional(12.0), theme::TEXT);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + vec2(20.0, 10.0), Sense::click());
    let color = if resp.hovered() { theme::ACCENT } else { theme::BORDER_STRONG };
    let pts = widgets::rounded_rect_points(rect.shrink(0.5), rect.height() / 2.0);
    ui.painter().extend(widgets::dashed_closed(&pts, Stroke::new(1.0, color), 3.0, 2.5));
    let text_color = if resp.hovered() { Color32::WHITE } else { theme::TEXT };
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, text_color);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensor_and_file_size_texts() {
        assert_eq!(sensor_detail(6000, 4000), "· 24.0 MP · 3:2");
        assert_eq!(sensor_detail(4000, 6000), "· 24.0 MP · 2:3");
        assert_eq!(sensor_detail(5184, 3888), "· 20.2 MP · 4:3");
        assert_eq!(sensor_detail(1000, 777), "· 0.8 MP");
        assert_eq!(file_size_text(24_600_000), "24.6 MB");
        assert_eq!(file_size_text(1_260_000_000), "1.3 GB");
        assert_eq!(file_size_text(900), "1 KB");
    }
}
