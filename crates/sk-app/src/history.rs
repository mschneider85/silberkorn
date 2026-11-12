//! History tab: the edit steps of the current image to jump back and forth (the undo and redo
//! stacks), and named snapshots kept in the sidecar.

use egui::{Color32, Rect, Response, RichText, Sense, Stroke, Ui, pos2, vec2};
use sk_core::history::{changes, describe};
use sk_core::sidecar::{self, Snapshot};

use crate::app::{App, Tab};
use crate::theme::{self, icon};

enum SnapshotAction {
    Add,
    Apply(usize),
    Delete(usize),
    StartRename(usize),
    Rename(usize, String),
}

impl App {
    /// Steps of the timeline, oldest first, and the index of the current one.
    pub(crate) fn history_steps(&self) -> Option<(Vec<&sk_core::params::Adjustments>, usize)> {
        let cur = self.current.as_ref()?;
        let steps: Vec<_> = self.undo.iter().chain(std::iter::once(&cur.committed)).chain(self.redo.iter().rev()).collect();
        Some((steps, self.undo.len()))
    }

    /// Shows step `j` of the timeline in the image while it is hovered.
    pub(crate) fn preview_history_step(&mut self, j: usize) {
        if let Some(adj) = self.history_steps().and_then(|(steps, _)| steps.get(j).map(|a| (*a).clone())) {
            self.preview_style(adj);
        }
    }

    /// Goes to step `target` of the timeline (undo or redo as often as needed), saving once.
    pub fn jump_history(&mut self, target: usize) {
        self.commit_and_save();
        let active = self.active_layer_id();
        let mut moved = false;
        while self.undo.len() > target && self.move_image_step(true) {
            moved = true;
        }
        while self.undo.len() < target && self.move_image_step(false) {
            moved = true;
        }
        if moved {
            self.image_stepped(active);
            self.commit_and_save();
        }
    }

    /// Removes snapshot `i` of the open image.
    pub fn delete_snapshot(&mut self, i: usize) {
        if let Some(cur) = self.current.as_mut().filter(|c| i < c.snapshots.len()) {
            cur.snapshots.remove(i);
            self.save_snapshots();
        }
    }

    /// Removes snapshot `i` named `name` of image `path`, only if that image is still open and
    /// the snapshot still there (another image may have opened while the question was asked).
    pub(crate) fn delete_snapshot_of(&mut self, path: &std::path::Path, i: usize, name: &str) {
        let still = self.current.as_ref().is_some_and(|c| c.meta.path == path && c.snapshots.get(i).is_some_and(|s| s.name == name));
        if still {
            self.delete_snapshot(i);
        }
    }

    /// Asks before deleting snapshot `i` (it can't be restored).
    pub fn request_snapshot_deletion(&mut self, i: usize) {
        if let Some((path, name)) = self.current.as_ref().and_then(|c| Some((c.meta.path.clone(), c.snapshots.get(i)?.name.clone()))) {
            self.request_deletion(crate::deletion::Deletion::Snapshot(path, i, name));
        }
    }

    /// Saves the current adjustments as a snapshot.
    pub fn add_snapshot(&mut self, name: Option<String>) {
        self.commit_and_save();
        let Some(cur) = self.current.as_mut() else { return };
        let name = name.unwrap_or_else(|| format!("Schnappschuss {}", cur.snapshots.len() + 1));
        cur.snapshots.push(Snapshot { name, adjustments: cur.committed.clone() });
        self.save_snapshots();
    }

    /// "Jetzigen Stand sichern": saves the current state and offers its name for editing.
    pub(crate) fn add_snapshot_to_name(&mut self) {
        self.add_snapshot(None);
        self.snapshot_rename = self.current.as_ref().and_then(|c| Some((c.snapshots.len().checked_sub(1)?, c.snapshots.last()?.name.clone())));
    }

    fn save_snapshots(&mut self) {
        let Some(cur) = &self.current else { return };
        if let Err(e) = sidecar::set_snapshots(&cur.meta.path, cur.snapshots.clone()) {
            self.report_error(format!("Schnappschuss nicht gespeichert: {e:#}"));
        }
    }

    /// The snapshots of the open image, in the history tab and the side column alike: "+" saves
    /// the current state and lets it be named right away, a click applies one, a double click
    /// renames it. In the side column it starts closed, with their number in the header.
    pub(crate) fn snapshot_section(&mut self, ui: &mut Ui, side: bool) {
        let mut action = None;
        let mut typed = None;
        // Hovered: the image shows it until the mouse leaves (see `App::preview_style`).
        let mut hovered = None;
        let editing = self.snapshot_rename.clone();
        // As the styles: small previews of the image, or (with „Liste“ there) a list.
        let grid = !self.style_list;
        if grid {
            self.pump_snapshot_thumbs();
        }
        let Some(cur) = &self.current else { return };
        let app = &*self;
        let help = "Ein Schnappschuss sichert den jetzigen Stand; ein Klick darauf stellt ihn wieder her.";
        let mut body = |ui: &mut Ui| {
            if grid {
                ui.add_space(4.0);
                // The name being edited above the previews.
                if let Some((i, text)) = &editing {
                    match rename_field(ui, *i, text) {
                        Naming::Typed(t) => typed = Some((*i, t)),
                        Naming::Done(t) => action = Some(SnapshotAction::Rename(*i, t)),
                        Naming::Going => {}
                    }
                }
                let (cols, tile, image_h) = crate::style_grid::tile_size(ui.available_width());
                // The snapshots, then the tile that adds one.
                let shots: Vec<Option<(usize, &sk_core::sidecar::Snapshot)>> = cur.snapshots.iter().enumerate().map(Some).chain(std::iter::once(None)).collect();
                for row in shots.chunks(cols) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        for shot in row {
                            let Some((i, s)) = shot else {
                                if add_tile(ui, tile, image_h + 18.0).clicked() {
                                    action = Some(SnapshotAction::Add);
                                }
                                continue;
                            };
                            let current = s.adjustments == cur.adjustments;
                            let r = crate::style_grid::thumb_tile(ui, app.snapshot_texture(*i, &s.adjustments), &s.name, tile, image_h, current)
                                .on_hover_text("Klick: anwenden · Doppelklick: umbenennen");
                            if current {
                                current_badge(ui, r.rect, image_h);
                            }
                            if r.hovered() && !current {
                                hovered = Some(*i);
                            }
                            if r.double_clicked() {
                                action = Some(SnapshotAction::StartRename(*i));
                            } else if r.clicked() {
                                action = Some(SnapshotAction::Apply(*i));
                            }
                            crate::widgets::context_menu(&r, |ui| {
                                if ui.button("Anwenden").clicked() {
                                    action = Some(SnapshotAction::Apply(*i));
                                }
                                if ui.button("Umbenennen").clicked() {
                                    action = Some(SnapshotAction::StartRename(*i));
                                }
                                if ui.button(format!("{}  Löschen …", icon::TRASH)).clicked() {
                                    action = Some(SnapshotAction::Delete(*i));
                                }
                            });
                        }
                    });
                    ui.add_space(6.0);
                }
                return;
            }
            for (i, s) in cur.snapshots.iter().enumerate() {
                ui.horizontal(|ui| {
                    match &editing {
                        Some((j, text)) if *j == i => match rename_field(ui, i, text) {
                            Naming::Typed(t) => typed = Some((i, t)),
                            Naming::Done(t) => action = Some(SnapshotAction::Rename(i, t)),
                            Naming::Going => {}
                        },
                        _ => {
                            let current = s.adjustments == cur.adjustments;
                            let r = ui.selectable_label(current, format!("{}  {}", icon::CAMERA, s.name)).on_hover_text("Klick: anwenden · Doppelklick: umbenennen");
                            if r.hovered() && !current {
                                hovered = Some(i);
                            }
                            if r.double_clicked() {
                                action = Some(SnapshotAction::StartRename(i));
                            } else if r.clicked() {
                                action = Some(SnapshotAction::Apply(i));
                            }
                            crate::widgets::context_menu(&r, |ui| {
                                if ui.button("Anwenden").clicked() {
                                    action = Some(SnapshotAction::Apply(i));
                                }
                                if ui.button("Umbenennen").clicked() {
                                    action = Some(SnapshotAction::StartRename(i));
                                }
                                if ui.button(format!("{}  Löschen …", icon::TRASH)).clicked() {
                                    action = Some(SnapshotAction::Delete(i));
                                }
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button(icon::TRASH).on_hover_text("Schnappschuss löschen …").clicked() {
                                    action = Some(SnapshotAction::Delete(i));
                                }
                            });
                        }
                    }
                });
            }
            ui.add_space(4.0);
            if add_tile(ui, ui.available_width() - 4.0, 30.0).clicked() {
                action = Some(SnapshotAction::Add);
            }
        };
        if side {
            crate::widgets::section_counted(ui, "Schnappschüsse", Some(help), cur.snapshots.len(), body);
        } else {
            let n = cur.snapshots.len();
            crate::widgets::nav_header(ui, "Schnappschüsse", "Schnappschüsse", |ui| {
                if n > 0 {
                    ui.label(RichText::new(n.to_string()).size(11.5).color(theme::TEXT_DIM));
                }
            });
            ui.horizontal(move |ui| {
                ui.add_space(4.0);
                ui.vertical(move |ui| {
                    ui.set_width(ui.available_width() - 4.0);
                    body(ui);
                });
            });
        }

        if typed.is_some() {
            self.snapshot_rename = typed;
        }
        if let Some(adj) = hovered.filter(|_| action.is_none()).and_then(|i| self.current.as_ref()?.snapshots.get(i)).map(|s| s.adjustments.clone()) {
            self.preview_style(adj);
        }
        match action {
            Some(SnapshotAction::Add) => self.add_snapshot_to_name(),
            Some(SnapshotAction::Apply(i)) => {
                if let Some(adj) = self.current.as_ref().and_then(|c| c.snapshots.get(i)).map(|s| s.adjustments.clone()) {
                    self.apply_snapshot(adj);
                }
            }
            Some(SnapshotAction::Delete(i)) => self.request_snapshot_deletion(i),
            Some(SnapshotAction::StartRename(i)) => {
                self.snapshot_rename = self.current.as_ref().and_then(|c| c.snapshots.get(i)).map(|s| (i, s.name.clone()));
            }
            Some(SnapshotAction::Rename(i, name)) => {
                self.snapshot_rename = None;
                let name = name.trim().to_string();
                if let Some(s) = self.current.as_mut().and_then(|c| c.snapshots.get_mut(i)).filter(|_| !name.is_empty()) {
                    s.name = name;
                    self.save_snapshots();
                }
            }
            None => {}
        }
    }

    pub fn history_tab(&mut self, ui: &mut Ui) {
        if self.current.is_none() {
            crate::widgets::no_image(ui);
            return;
        }
        self.snapshot_section(ui, false);
        let (mut jump, mut hovered) = (None, None);
        if let Some((steps, now)) = self.history_steps() {
            let n = steps.len();
            let count = match n {
                1 => "nur geöffnet".to_string(),
                2 => "1 Schritt".to_string(),
                n => format!("{} Schritte", n - 1),
            };
            ui.add_space(8.0);
            crate::widgets::nav_header(ui, "Verlauf", "Schritte", |ui| {
                ui.label(RichText::new(count).size(11.5).color(theme::TEXT_DIM));
            });
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width() - 4.0);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for j in (0..n).rev() {
                        let step = if j == 0 { Step::opened() } else { Step::of(steps[j - 1], steps[j]) };
                        let state = if j == now {
                            State::Now
                        } else if j > now {
                            State::Undone
                        } else {
                            State::Past
                        };
                        let r = step_row(ui, &step, state, j + 1 < n, j > 0, j + 1 > now, j > now);
                        if r.clicked() && j != now {
                            jump = Some(j);
                        } else if r.hovered() && j != now {
                            hovered = Some(j);
                        }
                    }
                });
            });
            ui.add_space(12.0);
        }
        if let Some(j) = jump {
            self.jump_history(j);
        } else if let Some(j) = hovered {
            self.preview_history_step(j);
        }
    }
}

/// "+ Jetzigen Stand sichern": dashed, as large as a snapshot (or a row in the list).
fn add_tile(ui: &mut Ui, width: f32, height: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Jetzigen Stand sichern"));
    let hot = resp.hovered();
    let pts = crate::widgets::rounded_rect_points(rect.shrink(0.5), 6.0);
    ui.painter().extend(crate::widgets::dashed_closed(&pts, Stroke::new(1.0, if hot { theme::TEXT_DIM } else { theme::BORDER_STRONG }), 4.0, 3.0));
    let color = if hot { Color32::WHITE } else { theme::TEXT_DIM };
    let font = egui::FontId::proportional(11.5);
    if height > 40.0 {
        ui.painter().text(rect.center() - vec2(0.0, 9.0), egui::Align2::CENTER_CENTER, icon::PLUS, egui::FontId::proportional(16.0), color);
        let mut job = egui::text::LayoutJob::simple_singleline("Jetzigen Stand sichern".into(), font, color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(width - 8.0);
        let g = ui.painter().layout_job(job);
        ui.painter().galley(pos2(rect.center().x - g.size().x / 2.0, rect.center().y + 6.0), g, color);
    } else {
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, format!("{}  Jetzigen Stand sichern", icon::PLUS), font, color);
    }
    resp.on_hover_text("Den jetzigen Stand unter einem Namen sichern").on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The mark of the snapshot that is the current state: an accent frame and a check.
fn current_badge(ui: &Ui, tile: Rect, image_h: f32) {
    let img = Rect::from_min_size(tile.min, vec2(tile.width(), image_h));
    ui.painter().rect_stroke(img, 3.0, Stroke::new(2.0, theme::ACCENT), egui::StrokeKind::Inside);
    let c = pos2(img.right() - 12.0, img.top() + 12.0);
    ui.painter().circle_filled(c, 9.0, theme::ACCENT);
    ui.painter().text(c, egui::Align2::CENTER_CENTER, icon::CHECK, egui::FontId::proportional(11.0), Color32::WHITE);
}

/// One step of the timeline, as shown: the symbol of its tab, the layer, name and value.
struct Step {
    icon: &'static str,
    layer: Option<String>,
    label: String,
    value: String,
}

impl Step {
    fn opened() -> Self {
        Step { icon: icon::FILE_IMAGE, layer: None, label: "Geöffnet".into(), value: String::new() }
    }

    fn of(before: &sk_core::params::Adjustments, after: &sk_core::params::Adjustments) -> Self {
        let text = describe(before, after);
        let found = changes(before, after);
        let icon = step_icon(found.first().map(|c| c.0), found.len());
        // "Ebene „Himmel“: Belichtung 0.4": the layer apart.
        let (layer, rest) = match text.strip_prefix("Ebene „").and_then(|t| t.split_once('“')) {
            Some((name, rest)) => (Some(name.to_string()), rest.trim_start_matches(':').trim().to_string()),
            None => (None, text),
        };
        let rest = if rest.is_empty() { "geändert".to_string() } else { rest };
        // A single value after the name ("Belichtung 0.4").
        let (label, value) = match rest.rsplit_once(' ') {
            Some((l, v)) if v.parse::<f32>().is_ok() => (l.to_string(), v.replace('-', "−")),
            _ => (rest, String::new()),
        };
        Step { icon, layer, label, value }
    }
}

/// The symbol of the tab a change belongs to (several tools: the history's own).
fn step_icon(first: Option<&str>, n: usize) -> &'static str {
    if n > 2 {
        return Tab::History.icon();
    }
    let tab = match first.unwrap_or_default() {
        "Belichtung" | "Kontrast" | "Helligkeit" | "Lichter" | "Tiefen" | "Weiß" | "Schwarz" | "Tonwerte" | "Gradationskurve" | "Basismerkmale" => Tab::Exposure,
        "Sättigung" | "Dynamik" | "Weißabgleich" | "Farbeditor" | "Hauttöne" | "Farbbalance" | "Schwarzweiß" => Tab::Color,
        "Schärfen" | "Raw-Details" | "KI-Entrauschen" | "Rauschreduzierung" | "Klarheit" | "Struktur" | "Dunst entfernen" => Tab::Details,
        "Körnung" | "Vignette" | "Leuchten" | "Objektivunschärfe" | "Atmosphäre" | "Neu ausleuchten" => Tab::Effects,
        "Drehen" | "Zuschnitt" | "Geraderichten" | "Spiegeln" | "Trapezkorrektur" | "Objektivkorrektur" => Tab::Lens,
        "Ebenen" => Tab::Layers,
        "Retusche" | "Entfernen" => Tab::Retouch,
        "Ein-/Ausblenden" => return icon::EYE,
        _ => Tab::History,
    };
    tab.icon()
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Past,
    Now,
    /// Undone: comes back with redo.
    Undone,
}

/// A row of the timeline: the line through the step's mark (dashed into undone steps), the
/// layer as a chip, name and value; the current one highlighted with "JETZT".
fn step_row(ui: &mut Ui, step: &Step, state: State, line_up: bool, line_down: bool, up_dashed: bool, down_dashed: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
    let hint = match state {
        State::Now => "Jetziger Stand",
        State::Undone => "Rückgängig gemacht · Klick stellt bis hierher wieder her",
        State::Past => "Klick springt zu diesem Stand · Überfahren zeigt ihn im Bild",
    };
    let name = match &step.layer {
        Some(l) => format!("Ebene {l}: {} {}", step.label, step.value),
        None => format!("{} {}", step.label, step.value),
    };
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, state == State::Now, name.trim()));
    let p = ui.painter();
    if state == State::Now {
        p.rect_filled(rect, 8.0, Color32::from_rgb(38, 48, 61));
    } else if resp.hovered() {
        p.rect_filled(rect, 8.0, theme::CARD);
    }
    let cx = rect.left() + 14.0;
    let cy = rect.center().y;
    let line = |p: &egui::Painter, a: f32, b: f32, dashed: bool| {
        let (a, b) = (pos2(cx, a), pos2(cx, b));
        if dashed {
            p.extend(egui::Shape::dashed_line(&[a, b], Stroke::new(1.5, theme::BORDER_STRONG), 3.0, 3.0));
        } else {
            p.line_segment([a, b], Stroke::new(2.0, theme::BORDER_STRONG));
        }
    };
    if line_up {
        line(p, rect.top(), cy - 11.0, up_dashed);
    }
    if line_down {
        line(p, cy + 11.0, rect.bottom(), down_dashed);
    }
    let c = pos2(cx, cy);
    let icon_color = match state {
        State::Now => {
            p.circle_filled(c, 11.0, theme::ACCENT);
            Color32::WHITE
        }
        State::Past => {
            p.circle_filled(c, 11.0, theme::CONTROL);
            Color32::from_gray(180)
        }
        State::Undone => {
            let pts: Vec<egui::Pos2> = (0..=32).map(|k| c + vec2((k as f32 / 32.0 * std::f32::consts::TAU).cos(), (k as f32 / 32.0 * std::f32::consts::TAU).sin()) * 10.5).collect();
            p.extend(egui::Shape::dashed_line(&pts, Stroke::new(1.2, theme::BORDER_STRONG), 2.5, 2.5));
            theme::TEXT_FAINT
        }
    };
    p.text(c, egui::Align2::CENTER_CENTER, step.icon, egui::FontId::proportional(12.0), icon_color);
    let mut x = rect.left() + 34.0;
    let mut right = rect.right() - 8.0;
    if state == State::Now {
        let g = p.layout_no_wrap("JETZT".into(), egui::FontId::proportional(10.0), theme::ACCENT_TEXT);
        let b = Rect::from_min_size(pos2(right - g.size().x - 12.0, cy - g.size().y / 2.0 - 3.0), g.size() + vec2(12.0, 6.0));
        p.rect_filled(b, 5.0, theme::ACCENT_SOFT);
        p.galley(b.center() - g.size() / 2.0, g, theme::ACCENT_TEXT);
        right = b.left() - 8.0;
    }
    if let Some(l) = &step.layer {
        let color = if state == State::Undone { theme::TEXT_FAINT } else { theme::ACCENT_TEXT };
        let mut job = egui::text::LayoutJob::simple_singleline(l.clone(), egui::FontId::proportional(11.0), color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(90.0);
        let g = p.layout_job(job);
        let b = Rect::from_min_size(pos2(x, cy - g.size().y / 2.0 - 2.0), g.size() + vec2(14.0, 4.0));
        if state == State::Undone {
            p.rect_stroke(b, b.height() / 2.0, Stroke::new(1.0, theme::BORDER_STRONG), egui::StrokeKind::Inside);
        } else {
            p.rect_filled(b, b.height() / 2.0, theme::ACCENT_SOFT);
        }
        p.galley(b.center() - g.size() / 2.0, g, color);
        x = b.right() + 6.0;
    }
    let (label_color, value_color, italics) = match state {
        State::Now => (Color32::WHITE, theme::ACCENT_TEXT, false),
        State::Past => (theme::TEXT, theme::TEXT_DIM, false),
        State::Undone => (theme::TEXT_FAINT, theme::TEXT_FAINT, true),
    };
    let mut job = egui::text::LayoutJob::default();
    let mut fmt = egui::TextFormat::simple(egui::FontId::proportional(13.0), label_color);
    fmt.italics = italics;
    job.append(&step.label, 0.0, fmt);
    if !step.value.is_empty() {
        let mut fmt = egui::TextFormat::simple(egui::FontId::proportional(13.0), value_color);
        fmt.italics = italics;
        job.append(&step.value, 6.0, fmt);
    }
    job.wrap = egui::text::TextWrapping::truncate_at_width((right - x).max(10.0));
    let g = p.layout_job(job);
    p.galley(pos2(x, cy - g.size().y / 2.0), g, label_color);
    resp.on_hover_text(hint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_name_layer_tool_and_value() {
        let a = sk_core::params::Adjustments::default();
        let b = sk_core::params::Adjustments { exposure: -0.4, ..a.clone() };
        let s = Step::of(&a, &b);
        assert_eq!((s.label.as_str(), s.value.as_str(), s.layer), ("Belichtung", "−0.4", None));
        assert_eq!(s.icon, Tab::Exposure.icon());
        let sky = sk_core::params::Layer { id: 1, name: "Himmel".into(), ..Default::default() };
        let c = sk_core::params::Adjustments { layers: vec![sky], ..a.clone() };
        let mut d = c.clone();
        d.layers[0].adjustments.exposure = 0.5;
        let s = Step::of(&c, &d);
        assert_eq!((s.layer.as_deref(), s.label.as_str(), s.value.as_str()), (Some("Himmel"), "Belichtung", "0.5"));
        let mut e = a.clone();
        e.geometry.straighten = 1.0;
        e.geometry.keystone_v = 5.0;
        assert_eq!(Step::of(&a, &e).label, "Geraderichten, Trapezkorrektur");
    }
}

/// What the name field of a snapshot did this frame.
enum Naming {
    Going,
    Typed(String),
    /// Ended: by Enter or a click elsewhere with the name typed, by Esc with "" (the old name stays).
    Done(String),
}

/// The name of snapshot `i` being edited: focused with the name selected when it appears (once,
/// so that a click elsewhere ends it), Enter or a click elsewhere keeps what was typed.
fn rename_field(ui: &mut Ui, i: usize, text: &str) -> Naming {
    let id = egui::Id::new(("snapshot-name", i));
    let started = id.with("started");
    let mut t = text.to_string();
    let mut out = egui::TextEdit::singleline(&mut t).id(id).desired_width(ui.available_width() - 8.0).show(ui);
    if !ui.data(|d| d.get_temp::<bool>(started).unwrap_or(false)) {
        ui.data_mut(|d| d.insert_temp(started, true));
        out.response.request_focus();
        let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(t.chars().count()));
        out.state.cursor.set_char_range(Some(all));
        out.state.store(ui.ctx(), out.response.id);
        return Naming::Going;
    }
    if out.response.lost_focus() || !out.response.has_focus() {
        ui.data_mut(|d| d.remove::<bool>(started));
        let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
        return Naming::Done(if esc { String::new() } else { t });
    }
    if out.response.changed() { Naming::Typed(t) } else { Naming::Going }
}
