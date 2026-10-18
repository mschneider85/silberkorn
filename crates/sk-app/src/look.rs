//! "Look angleichen": the selected images get the look of one or more references (images of the
//! library or picture files, see `sk_core::look_match`), each weighted by how similar it is to
//! the image. The work is done in the background (`look_worker`); the strength of the last match
//! of the open image can still be changed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Color32, Rect, RichText, Sense, Ui, Vec2};
use serde::{Deserialize, Serialize};
use sk_core::look_match::LookOptions;
use sk_core::params::Adjustments;

use crate::app::App;
use crate::look_worker::{LookRequest, LookResult};
use crate::theme::{self, icon};
use crate::widgets;

/// A reference of the look.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LookRef {
    /// An image of the library, as developed.
    Image(PathBuf),
    /// A picture file (JPEG, PNG, TIFF, WebP), e.g. a film still.
    File(PathBuf),
}

impl LookRef {
    pub fn path(&self) -> &Path {
        match self {
            LookRef::Image(p) | LookRef::File(p) => p,
        }
    }

    pub fn name(&self) -> String {
        sk_core::variant::display_name(self.path())
    }
}

/// What the app knows of a reference.
pub enum LookRefState {
    Loading,
    Ready(egui::TextureHandle),
    Failed(String),
}

/// The last look match of an image: its strength can still be changed, between the settings
/// before and the full match.
pub struct LookMix {
    pub path: PathBuf,
    pub before: Adjustments,
    pub matched: Adjustments,
    pub opts: LookOptions,
    /// 0..100 %
    pub strength: f32,
    /// What changed, in words.
    pub summary: String,
    /// The references with their share of the look and their similarity to the image.
    pub shares: Vec<(LookRef, f32, f32)>,
}

/// How similar the open image is to the references.
pub struct LookSimilarity {
    pub path: PathBuf,
    pub refs: Vec<LookRef>,
    pub values: Vec<Option<f32>>,
}

/// Picture files a look can be taken from.
const PICTURE_FILES: [&str; 6] = ["jpg", "jpeg", "png", "tif", "tiff", "webp"];
/// Thumbnails in the list.
const THUMB: Vec2 = Vec2::new(64.0, 44.0);

fn is_picture_file(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| PICTURE_FILES.contains(&e.to_ascii_lowercase().as_str()))
}

impl App {
    /// The selected images, without the references among them.
    fn look_targets(&self) -> Vec<PathBuf> {
        self.selected_paths().into_iter().filter(|p| !self.look_refs.contains(&LookRef::Image(p.clone()))).collect()
    }

    pub fn can_match_look(&self) -> bool {
        self.look_progress.is_none() && self.look_options.any() && !self.look_refs.is_empty() && !self.look_targets().is_empty()
    }

    /// Why "Look angleichen" can't run now (for the disabled button).
    fn look_blocked(&self) -> &'static str {
        if self.look_progress.is_some() {
            "Der Look wird gerade angeglichen"
        } else if self.look_refs.is_empty() {
            "Zuerst eine Referenz hinzufügen: eine Bilddatei oder ein Bild der Bibliothek"
        } else if !self.look_options.any() {
            "Nichts ausgewählt, was angeglichen werden soll"
        } else {
            "Ein Bild auswählen, das keine Referenz ist"
        }
    }

    /// Adds a reference (once) and has it measured.
    pub fn add_look_ref(&mut self, r: LookRef) {
        if self.look_refs.contains(&r) {
            self.status = format!("„{}“ ist schon eine Referenz", r.name());
            return;
        }
        self.status = format!("Look-Referenz: {}", r.name());
        self.look_ref_state.insert(r.clone(), LookRefState::Loading);
        self.workers.look(LookRequest::Prepare(vec![r.clone()]));
        self.look_refs.push(r);
        self.save_settings();
    }

    pub fn remove_look_ref(&mut self, r: &LookRef) {
        self.look_refs.retain(|x| x != r);
        self.look_ref_state.remove(r);
        self.save_settings();
    }

    /// The open image as a reference.
    pub fn add_current_as_look_ref(&mut self) {
        if let Some(p) = self.current_path() {
            // As it is now.
            self.commit_and_save();
            self.add_look_ref(LookRef::Image(p));
        }
    }

    /// Lets the user choose picture files as references; whether any was chosen.
    pub fn pick_look_files(&mut self) -> bool {
        let mut dialog = rfd::FileDialog::new().set_title("Look-Referenzen wählen").add_filter("Bilder", &PICTURE_FILES);
        if let Some(dir) = self.look_refs.iter().rev().find_map(|r| matches!(r, LookRef::File(_)).then(|| r.path().parent()).flatten()) {
            dialog = dialog.set_directory(dir);
        }
        let Some(files) = dialog.pick_files() else { return false };
        for f in files {
            self.add_look_ref(LookRef::File(f));
        }
        true
    }

    /// Matches the look of the selected images to the references.
    pub fn match_look_selection(&mut self) {
        if self.look_progress.is_some() {
            return;
        }
        if self.look_refs.is_empty() {
            self.status = "Look angleichen: zuerst eine Referenz hinzufügen".into();
            return;
        }
        if !self.look_options.any() {
            self.status = "Look angleichen: nichts ausgewählt, was angeglichen werden soll".into();
            return;
        }
        // The references and the current image as they are now.
        self.commit_and_save();
        let targets = self.look_targets();
        if targets.is_empty() {
            self.status = "Look angleichen: ein Bild auswählen, das keine Referenz ist".into();
            return;
        }
        let current = self.current_path();
        let items: Vec<crate::workers::AutoItem> = targets
            .into_iter()
            .map(|path| {
                let open = (Some(&path) == current.as_ref())
                    .then(|| self.current.as_ref().map(|c| (c.meta.clone(), c.preview.clone(), c.adjustments.clone())))
                    .flatten();
                crate::workers::AutoItem { path, open }
            })
            .collect();
        self.look_progress = Some((0, items.len()));
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.look_cancel = Some(cancel.clone());
        self.status = if items.len() == 1 { "Gleiche den Look an …".into() } else { format!("Gleiche den Look von {} an …", widgets::image_count(items.len())) };
        self.workers.look(LookRequest::Match { refs: self.look_refs.clone(), items, opts: self.look_options, cancel });
    }

    /// A reference has been measured.
    pub(crate) fn look_ref_arrived(&mut self, ctx: &egui::Context, r: LookRef, result: Result<egui::ColorImage, String>) {
        if !self.look_refs.contains(&r) {
            return;
        }
        let state = match result {
            Ok(image) => LookRefState::Ready(ctx.load_texture(format!("look:{}", r.path().display()), image, egui::TextureOptions::LINEAR)),
            Err(e) => LookRefState::Failed(e),
        };
        self.look_ref_state.insert(r, state);
    }

    /// The look of one image has been matched: its settings change (the open image's as one
    /// undo step). Only what the options cover changes, so edits made meanwhile stay.
    pub(crate) fn look_matched(&mut self, path: PathBuf, opts: LookOptions, result: Result<LookResult, String>) {
        let failed = match result {
            Ok(m) => {
                let before = self.current.as_ref().filter(|_| self.current_path().as_ref() == Some(&path)).map(|c| c.adjustments.clone());
                self.set_adjustments_on(std::slice::from_ref(&path), |a| opts.take(&m.adjustments, a));
                if let Some(before) = before
                    && let Some(cur) = &self.current
                {
                    let shares = m.refs.iter().zip(&m.weights).zip(&m.similarity).map(|((r, w), s)| (r.clone(), *w, *s)).collect();
                    let summary = summary(&before, &cur.adjustments, opts);
                    self.look_mix = Some(LookMix { path, before, matched: cur.adjustments.clone(), opts, strength: 100.0, summary, shares });
                }
                false
            }
            Err(e) => {
                log::warn!("Look angleichen: {e}");
                self.report_error(format!("Look angleichen fehlgeschlagen: {e}"));
                true
            }
        };
        if let Some((done, total)) = self.look_progress {
            let done = done + 1;
            self.look_progress = (done < total).then_some((done, total));
            if done == total && !failed && !self.status.starts_with("Look angleichen fehlgeschlagen") {
                self.status = if total == 1 { "Look angeglichen".into() } else { format!("Look von {} angeglichen", widgets::image_count(total)) };
            }
        }
    }

    /// Asks for what the list shows: thumbnails of new references, the similarity to the open image.
    fn look_requests(&mut self) {
        let missing: Vec<LookRef> = self.look_refs.iter().filter(|r| !self.look_ref_state.contains_key(*r)).cloned().collect();
        if !missing.is_empty() {
            for r in &missing {
                self.look_ref_state.insert(r.clone(), LookRefState::Loading);
            }
            self.workers.look(LookRequest::Prepare(missing));
        }
        let Some(path) = self.current_path() else { return };
        if self.look_refs.is_empty() {
            return;
        }
        let asked = (path.clone(), self.look_refs.clone());
        if self.look_similarity_asked.as_ref() == Some(&asked) {
            return;
        }
        let Some(cur) = &self.current else { return };
        self.workers.look(LookRequest::Similarity { path, open: Box::new((cur.meta.clone(), cur.preview.clone(), cur.adjustments.clone())), refs: self.look_refs.clone() });
        self.look_similarity_asked = Some(asked);
    }

    /// Similarity of `r` to the open image, if known.
    fn look_similarity_of(&self, r: &LookRef) -> Option<f32> {
        let s = self.look_similarity.as_ref().filter(|s| Some(&s.path) == self.current_path().as_ref())?;
        s.refs.iter().position(|x| x == r).and_then(|i| s.values.get(i).copied().flatten())
    }

    /// Picture files (or images of the library) dropped onto the app become references.
    fn look_drops(&mut self, ui: &Ui, area: Rect) {
        let dropped: Vec<PathBuf> = ui.ctx().input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        for p in dropped {
            if self.index_of(&p).is_some() {
                self.add_look_ref(LookRef::Image(p));
            } else if is_picture_file(&p) {
                self.add_look_ref(LookRef::File(p));
            } else {
                self.status = format!("„{}“ ist keine Bilddatei (JPEG, PNG, TIFF, WebP)", sk_core::variant::display_name(&p));
            }
        }
        if ui.ctx().input(|i| !i.raw.hovered_files.is_empty()) {
            let painter = ui.painter();
            painter.rect_filled(area, 4.0, theme::ACCENT.gamma_multiply(0.15));
            painter.rect_stroke(area, 4.0, egui::Stroke::new(1.5, theme::ACCENT), egui::StrokeKind::Inside);
            painter.text(area.center(), egui::Align2::CENTER_CENTER, "Hier ablegen: wird Look-Referenz", egui::FontId::proportional(13.0), Color32::WHITE);
        }
    }

    /// "Look angleichen" for the color tab: references, what to match, the button, strength and
    /// what the last match changed.
    pub fn look_section(&mut self, ui: &mut Ui) {
        self.look_requests();
        let help = "Überträgt den Look von Referenzbildern auf die Auswahl, mit den normalen Reglern. \
                    Ähnliche Referenzen wirken stärker; bei einem anderen Motiv wird nur teilweise übernommen.";
        let mut remove: Option<LookRef> = None;
        let mut open: Option<PathBuf> = None;
        let mut add_file = false;
        let mut add_current = false;
        let mut run = false;
        let top = ui.cursor().min;
        widgets::section_help(ui, "Look angleichen", help, None, |ui| {
            if self.look_refs.is_empty() {
                ui.label(RichText::new("Noch keine Referenz. Eine Bilddatei hinzufügen oder hierher ziehen, oder ein Bild der Bibliothek.").weak().small());
            }
            // The share of each reference in the open image's last match.
            let current = self.current_path();
            let shares: Vec<(LookRef, f32)> = self
                .look_mix
                .as_ref()
                .filter(|m| Some(&m.path) == current.as_ref() && m.shares.len() > 1)
                .map(|m| m.shares.iter().map(|(r, w, _)| (r.clone(), *w)).collect())
                .unwrap_or_default();
            for r in self.look_refs.clone() {
                let share = shares.iter().find(|(x, _)| *x == r).map(|(_, w)| *w);
                match self.look_ref_row(ui, &r, share) {
                    Some(RowAction::Remove) => remove = Some(r.clone()),
                    Some(RowAction::Open) => open = Some(r.path().to_path_buf()),
                    None => {}
                }
            }
            ui.add_space(2.0);
            let gap = ui.spacing().item_spacing.x;
            let half = ((ui.available_width() - gap) / 2.0).floor();
            ui.horizontal(|ui| {
                if ui
                    .add_sized([half, 24.0], egui::Button::new(format!("{}  Bilddatei …", icon::PLUS)))
                    .on_hover_text("JPEG, PNG, TIFF oder WebP als Referenz (mehrere möglich; Dateien lassen sich auch hierher ziehen)")
                    .clicked()
                {
                    add_file = true;
                }
                let current_is_ref = self.current_path().is_some_and(|p| self.look_refs.contains(&LookRef::Image(p)));
                if ui
                    .add_enabled_ui(self.current.is_some() && !current_is_ref, |ui| ui.add_sized([half, 24.0], egui::Button::new(format!("{}  Dieses Bild", icon::PLUS))))
                    .inner
                    .on_hover_text("Das geöffnete Bild, wie es bearbeitet ist, als Referenz")
                    .on_disabled_hover_text(if current_is_ref { "Dieses Bild ist schon eine Referenz" } else { "Kein Bild geöffnet" })
                    .clicked()
                {
                    add_current = true;
                }
            });
            ui.add_space(6.0);
            // What to match: toggles in three equal columns.
            let mut opts = self.look_options;
            {
                let mut toggles = [
                    (&mut opts.exposure, "Belichtung", "Mittlere Helligkeit"),
                    (&mut opts.white_balance, "Weißabgleich", "Farbstich der fast neutralen Töne"),
                    (&mut opts.tone, "Tonwerte", "Schwarz- und Weißpunkt, Helligkeit und Kontrast als Luma-Kurve"),
                    (&mut opts.colors, "Farben", "Sättigung und Farbeditor"),
                    (&mut opts.color_balance, "Farbstimmung", "Farbbalance in Schatten, Mitten und Lichtern (Split-Toning)"),
                ];
                let gap = 4.0;
                let width = ((ui.available_width() - 2.0 * gap) / 3.0).floor();
                for row in toggles.chunks_mut(3) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(gap, gap);
                        for (on, text, hover) in row.iter_mut() {
                            if ui.add_sized([width, 22.0], egui::Button::selectable(**on, *text)).on_hover_text(*hover).clicked() {
                                **on = !**on;
                            }
                        }
                    });
                }
            }
            if opts != self.look_options {
                self.look_options = opts;
                self.save_settings();
            }
            ui.add_space(4.0);
            let busy = self.look_progress.is_some();
            let n = self.look_targets().len();
            let text = match (busy, n) {
                (true, _) => "Gleiche an …".to_string(),
                (false, 0 | 1) => "Look angleichen".to_string(),
                (false, n) => format!("Look angleichen ({})", widgets::image_count(n)),
            };
            let enabled = self.can_match_look();
            let button = egui::Button::new(RichText::new(format!("{}  {text}", icon::SWATCHES)).strong()).min_size(Vec2::new(ui.available_width(), 26.0));
            if ui.add_enabled(enabled, button).on_disabled_hover_text(self.look_blocked()).clicked() {
                run = true;
            }
            self.look_strength(ui);
        });
        let area = Rect::from_min_max(top, egui::pos2(ui.max_rect().right(), ui.cursor().min.y));
        self.look_drops(ui, area);
        if let Some(r) = remove {
            self.remove_look_ref(&r);
        }
        if add_file {
            self.pick_look_files();
        }
        if add_current {
            self.add_current_as_look_ref();
        }
        if let Some(p) = open {
            self.select_path(&p, egui::Modifiers::NONE);
        }
        if run {
            self.match_look_selection();
        }
    }

    /// Strength of the last look match of the open image, and what it changed; disabled until
    /// there is one.
    fn look_strength(&mut self, ui: &mut Ui) {
        let current = self.current_path();
        let mix = self.look_mix.as_mut().filter(|m| Some(&m.path) == current.as_ref());
        let enabled = mix.is_some();
        let mut idle = 100.0;
        let mut changed = false;
        let strength = match mix {
            Some(m) => &mut m.strength,
            None => &mut idle,
        };
        ui.add_enabled_ui(enabled, |ui| {
            changed = widgets::slider_row_with(ui, "Stärke", strength, 0.0..=100.0, 100.0, 0, widgets::SliderOpts { suffix: " %", ..Default::default() });
        })
        .response
        .on_disabled_hover_text("Wie stark der zuletzt angeglichene Look wirkt: erst nach „Look angleichen“");
        let Some(mix) = self.look_mix.as_ref().filter(|_| enabled) else { return };
        ui.label(RichText::new(&mix.summary).small().weak());
        let alike: f32 = mix.shares.iter().map(|(_, w, s)| w * s).sum();
        if alike < 0.4 {
            ui.label(RichText::new("Anderes Motiv: Belichtung, Tonwerte und Farben wurden nur teilweise übernommen").small().weak());
        }
        let Some(cur) = self.current.as_mut() else { return };
        if changed {
            cur.adjustments = mix.opts.blend(&cur.adjustments, &mix.before, &mix.matched, mix.strength / 100.0, cur.as_shot_temp_tint);
            self.request_render();
        }
    }
}

/// What was clicked in a row of the reference list.
enum RowAction {
    Remove,
    Open,
}

/// Words and color for a similarity.
fn similarity_words(s: f32) -> (&'static str, Color32) {
    if s >= 0.75 {
        ("sehr ähnlich", Color32::from_rgb(110, 190, 120))
    } else if s >= 0.4 {
        ("ähnlich", Color32::from_rgb(205, 180, 90))
    } else {
        ("anderes Motiv", theme::TEXT_DIM)
    }
}

impl App {
    /// One reference: thumbnail (filled, larger on hover), name, similarity to the open image
    /// (and its share of the last match), remove button. Laid out by hand so that nothing is
    /// pushed out at the right edge.
    fn look_ref_row(&self, ui: &mut Ui, r: &LookRef, share: Option<f32>) -> Option<RowAction> {
        let mut action = None;
        // Click sense for the context menu (the thumbnail and the ✕ take their own clicks).
        let (row, row_resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), THUMB.y + 6.0), Sense::click());
        let painter = ui.painter_at(row);
        let hovered = ui.rect_contains_pointer(row);
        if hovered {
            painter.rect_filled(row, 4.0, Color32::from_gray(40));
        }
        let thumb = Rect::from_min_size(row.min + Vec2::new(3.0, 3.0), THUMB);
        let close = Rect::from_center_size(egui::pos2(row.right() - 12.0, row.center().y), Vec2::splat(20.0));
        let text_left = thumb.right() + 8.0;
        let text_width = (close.left() - 6.0 - text_left).max(10.0);

        // Thumbnail, cropped to fill its box.
        painter.rect_filled(thumb, 3.0, Color32::from_gray(24));
        let thumb_resp = ui.interact(thumb, row_resp.id.with("thumb"), Sense::click());
        match self.look_ref_state.get(r) {
            Some(LookRefState::Ready(t)) => {
                let [w, h] = t.size().map(|v| v as f32);
                let (ta, ba) = (w / h, thumb.width() / thumb.height());
                let uv = if ta > ba {
                    let u = (1.0 - ba / ta) / 2.0;
                    Rect::from_min_max(egui::pos2(u, 0.0), egui::pos2(1.0 - u, 1.0))
                } else {
                    let v = (1.0 - ta / ba) / 2.0;
                    Rect::from_min_max(egui::pos2(0.0, v), egui::pos2(1.0, 1.0 - v))
                };
                egui::Image::new((t.id(), thumb.size())).uv(uv).corner_radius(3.0).paint_at(ui, thumb);
                let t = t.clone();
                let thumb_resp = thumb_resp.on_hover_ui(|ui| {
                    let k = (240.0 / w.max(h)).max(1.0);
                    ui.image((t.id(), Vec2::new(w * k, h * k)));
                    if matches!(r, LookRef::Image(_)) {
                        ui.label(RichText::new("Klick: dieses Bild öffnen").small().weak());
                    }
                });
                if matches!(r, LookRef::Image(_)) && thumb_resp.clicked() {
                    action = Some(RowAction::Open);
                }
            }
            Some(LookRefState::Failed(_)) => {
                painter.text(thumb.center(), egui::Align2::CENTER_CENTER, icon::WARNING, egui::FontId::proportional(18.0), theme::DANGER);
            }
            _ => {
                painter.text(thumb.center(), egui::Align2::CENTER_CENTER, "…", egui::FontId::proportional(14.0), theme::TEXT_FAINT);
            }
        }

        // Name.
        let mut job = egui::text::LayoutJob::simple_singleline(r.name(), egui::FontId::proportional(12.5), theme::TEXT);
        job.wrap = egui::text::TextWrapping::truncate_at_width(text_width);
        let name = painter.layout_job(job);
        let name_pos = egui::pos2(text_left, row.top() + 7.0);
        let name_rect = Rect::from_min_size(name_pos, name.size());
        painter.galley(name_pos, name, theme::TEXT);
        ui.interact(name_rect, row_resp.id.with("name"), Sense::hover()).on_hover_text(r.path().display().to_string());

        // Similarity (or why it can't be used, or what it is while measuring).
        let y = row.top() + 29.0;
        let small = egui::FontId::proportional(11.0);
        match (self.look_ref_state.get(r), self.look_similarity_of(r)) {
            (Some(LookRefState::Failed(e)), _) => {
                let mut job = egui::text::LayoutJob::simple_singleline(format!("Nicht verwendbar: {e}"), small, theme::DANGER);
                job.wrap = egui::text::TextWrapping::truncate_at_width(text_width);
                painter.galley(egui::pos2(text_left, y - 6.0), painter.layout_job(job), theme::DANGER);
            }
            (_, Some(s)) => {
                let (word, color) = similarity_words(s);
                let bar = Rect::from_min_size(egui::pos2(text_left, y - 2.0), Vec2::new(36.0, 4.0));
                painter.rect_filled(bar, 2.0, Color32::from_gray(60));
                painter.rect_filled(Rect::from_min_size(bar.min, Vec2::new((bar.width() * s.clamp(0.0, 1.0)).max(2.0), bar.height())), 2.0, color);
                let mut text = format!("{word} · {:.0} %", s * 100.0);
                if let Some(w) = share {
                    text += &format!(" · Anteil {:.0} %", w * 100.0);
                }
                let mut job = egui::text::LayoutJob::simple_singleline(text, small, color);
                job.wrap = egui::text::TextWrapping::truncate_at_width((text_width - 42.0).max(10.0));
                let g = painter.layout_job(job);
                let pos = egui::pos2(bar.right() + 6.0, y - g.size().y / 2.0);
                let rect = Rect::from_min_size(pos, g.size()).union(bar);
                painter.galley(pos, g, color);
                ui.interact(rect, row_resp.id.with("similarity"), Sense::hover()).on_hover_text(
                    "Wie sehr die Referenz dem aktuellen Bild gleicht. Ähnliche Referenzen bestimmen den Look stärker.",
                );
            }
            _ => {
                let kind = match r {
                    LookRef::Image(_) => "Bild der Bibliothek",
                    LookRef::File(_) => "Bilddatei",
                };
                painter.text(egui::pos2(text_left, y), egui::Align2::LEFT_CENTER, kind, small, theme::TEXT_FAINT);
            }
        }

        // Remove: always at the right edge, brighter on hover of the row.
        let close_resp = ui.interact(close, row_resp.id.with("remove"), Sense::click());
        let color = if close_resp.hovered() { theme::DANGER } else if hovered { theme::TEXT_DIM } else { theme::TEXT_FAINT };
        if close_resp.hovered() {
            painter.rect_filled(close, 3.0, Color32::from_gray(55));
        }
        painter.text(close.center(), egui::Align2::CENTER_CENTER, icon::X, egui::FontId::proportional(13.0), color);
        if close_resp.on_hover_text("Referenz entfernen").clicked() {
            action = Some(RowAction::Remove);
        }
        row_resp.context_menu(|ui| {
            if ui.button("Öffnen").clicked() {
                action = Some(RowAction::Open);
            }
            if ui.button(format!("{}  Referenz entfernen", icon::X)).clicked() {
                action = Some(RowAction::Remove);
            }
        });
        action
    }
}

/// What a match changed, in words.
fn summary(before: &Adjustments, after: &Adjustments, opts: LookOptions) -> String {
    let num = |v: f32, digits: usize| format!("{v:+.digits$}");
    let mut parts = Vec::new();
    if opts.exposure && (after.exposure - before.exposure).abs() >= 0.01 {
        parts.push(format!("Belichtung {}", num(after.exposure - before.exposure, 2)));
    }
    if opts.white_balance && after.white_balance != before.white_balance {
        parts.push(format!("Weißabgleich {:.0} K, Tönung {}", after.white_balance.kelvin, num(after.white_balance.tint, 1)));
    }
    if opts.tone && after.curves.luma != before.curves.luma {
        parts.push("Luma-Kurve".into());
    }
    if opts.colors {
        if after.saturation != before.saturation {
            parts.push(format!("Sättigung {}", num(after.saturation - before.saturation, 0)));
        }
        let bands = after.color_editor.bands.iter().zip(&before.color_editor.bands).filter(|(a, b)| a != b).count();
        if bands > 0 {
            parts.push(format!("Farbeditor: {bands} {}", if bands == 1 { "Farbe" } else { "Farben" }));
        }
    }
    if opts.color_balance && after.color_balance != before.color_balance {
        parts.push("Farbbalance".into());
    }
    if parts.is_empty() { "Keine Änderung nötig: das Bild hat den Look schon".into() } else { format!("Geändert: {}", parts.join(" · ")) }
}
