//! "Look angleichen": the selected images get the look of one or more references (images of the
//! library or picture files, see `sk_core::look_match`), each weighted by how similar it is to
//! the image. The work is done in the background (`look_worker`); the strength of the last match
//! of the open image can still be changed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Color32, Popup, Rect, RichText, Sense, Ui, Vec2};
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
            "Zuerst eine Vorlage hinzufügen: eine Bilddatei, dieses Bild oder das Referenzbild"
        } else if !self.look_options.any() {
            "Nichts ausgewählt, was angeglichen werden soll"
        } else {
            "Ein Bild auswählen, das keine Vorlage ist"
        }
    }

    /// Adds a reference (once) and has it measured.
    pub fn add_look_ref(&mut self, r: LookRef) {
        if self.look_refs.contains(&r) {
            self.status = format!("„{}“ ist schon eine Vorlage", r.name());
            return;
        }
        self.status = format!("Vorlage: {}", r.name());
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

    /// The reference image of the compare view, if it is in the library.
    pub fn reference_for_look(&self) -> Option<PathBuf> {
        self.compare_ref.clone().filter(|p| self.index_of(p).is_some())
    }

    /// The reference image of the compare view as a reference of the look.
    pub fn add_reference_as_look_ref(&mut self) {
        if let Some(p) = self.reference_for_look() {
            // As it is now, should it be the open image.
            self.commit_and_save();
            self.add_look_ref(LookRef::Image(p));
        }
    }

    /// Lets the user choose picture files as references; whether any was chosen.
    pub fn pick_look_files(&mut self) -> bool {
        let mut dialog = rfd::FileDialog::new().set_title("Vorlagen wählen").add_filter("Bilder", &PICTURE_FILES);
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
            self.status = "Look angleichen: zuerst eine Vorlage hinzufügen".into();
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
            self.status = "Look angleichen: ein Bild auswählen, das keine Vorlage ist".into();
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
        self.look_run += 1;
        self.batch_step = None;
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.look_cancel = Some(cancel.clone());
        self.status = if items.len() == 1 { "Gleiche den Look an …".into() } else { format!("Gleiche den Look von {} an …", widgets::image_count(items.len())) };
        self.workers.look(LookRequest::Match { run: self.look_run, refs: self.look_refs.clone(), items, opts: self.look_options, cancel });
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
    /// Results of another run (cancelled or replaced meanwhile) are dropped.
    pub(crate) fn look_matched(&mut self, run: u64, path: PathBuf, opts: LookOptions, result: Result<LookResult, String>) {
        if run != self.look_run {
            return;
        }
        let failed = match result {
            Ok(m) => {
                let before = self.current.as_ref().filter(|_| self.current_path().as_ref() == Some(&path)).map(|c| c.adjustments.clone());
                self.set_batch_result(&path, |a| opts.take(&m.adjustments, a));
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
            painter.text(area.center(), egui::Align2::CENTER_CENTER, "Hier ablegen: wird Vorlage", egui::FontId::proportional(13.0), Color32::WHITE);
        }
    }

    /// "Look angleichen" for the color tab: the templates as cards, an add menu, what to take
    /// over, the button, strength and what the last match changed.
    pub fn look_section(&mut self, ui: &mut Ui) {
        self.look_requests();
        let help = "Überträgt den Look von Vorlagen auf die Auswahl, mit den normalen Reglern. \
                    Ähnliche Vorlagen wirken stärker; bei einem anderen Motiv wird nur teilweise übernommen.";
        let mut card_action: Option<(CardAction, LookRef)> = None;
        let mut add: Option<AddChoice> = None;
        let mut run = false;
        let top = ui.cursor().min;
        widgets::section_help(ui, "Look angleichen", help, None, |ui| {
            // The share of each template in the open image's last match.
            let current = self.current_path();
            let shares: Vec<(LookRef, f32)> = self
                .look_mix
                .as_ref()
                .filter(|m| Some(&m.path) == current.as_ref() && m.shares.len() > 1)
                .map(|m| m.shares.iter().map(|(r, w, _)| (r.clone(), *w)).collect())
                .unwrap_or_default();
            let refs = self.look_refs.clone();
            let width = ui.available_width();
            if refs.is_empty() {
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 76.0), Sense::click());
                paint_add_tile(ui, rect, &resp, AddTile::Empty);
                add = self.look_add_menu(&resp);
            } else {
                // Two cards per row; with an odd number the add tile fills the last row,
                // otherwise a slim bar below does.
                let cell = Vec2::new(((width - CARD_GAP) / 2.0).floor(), ((width - CARD_GAP) / 2.0 * 0.66).round());
                let rows = refs.len().div_ceil(2);
                let (area, _) = ui.allocate_exact_size(Vec2::new(width, rows as f32 * (cell.y + CARD_GAP) - CARD_GAP), Sense::hover());
                let rect_of = |i: usize| Rect::from_min_size(area.min + Vec2::new((i % 2) as f32 * (cell.x + CARD_GAP), (i / 2) as f32 * (cell.y + CARD_GAP)), cell);
                for (i, r) in refs.iter().enumerate() {
                    let share = shares.iter().find(|(x, _)| x == r).map(|(_, w)| *w);
                    if let Some(a) = self.look_card(ui, rect_of(i), r, share) {
                        card_action = Some((a, r.clone()));
                    }
                }
                let resp = if refs.len() % 2 == 1 {
                    let rect = rect_of(refs.len());
                    let resp = ui.interact(rect, ui.id().with("look-add"), Sense::click());
                    paint_add_tile(ui, rect, &resp, AddTile::Cell);
                    resp
                } else {
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 28.0), Sense::click());
                    paint_add_tile(ui, rect, &resp, AddTile::Bar);
                    resp
                };
                add = self.look_add_menu(&resp);
            }
            ui.add_space(8.0);
            // What to take over: toggles as chips.
            ui.label(RichText::new("Übernehmen").small().color(theme::TEXT_FAINT));
            let mut opts = self.look_options;
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(5.0, 5.0);
                for (on, text, hover) in [
                    (&mut opts.exposure, "Belichtung", "Mittlere Helligkeit"),
                    (&mut opts.white_balance, "Weißabgleich", "Farbstich der fast neutralen Töne"),
                    (&mut opts.tone, "Tonwerte", "Schwarz- und Weißpunkt, Helligkeit und Kontrast als Luma-Kurve"),
                    (&mut opts.colors, "Farben", "Sättigung und Farbeditor"),
                    (&mut opts.color_balance, "Farbstimmung", "Farbbalance in Schatten, Mitten und Lichtern (Split-Toning)"),
                ] {
                    if option_chip(ui, *on, text).on_hover_text(hover).clicked() {
                        *on = !*on;
                    }
                }
            });
            if opts != self.look_options {
                self.look_options = opts;
                self.save_settings();
            }
            ui.add_space(8.0);
            let busy = self.look_progress.is_some();
            let n = self.look_targets().len();
            let text = match (busy, n) {
                (true, _) => "Gleiche an …".to_string(),
                (false, 0 | 1) => "Look angleichen".to_string(),
                (false, n) => format!("Look angleichen ({})", widgets::image_count(n)),
            };
            let enabled = self.can_match_look();
            let button = egui::Button::new(RichText::new(format!("{}  {text}", icon::SWATCHES)).strong().color(Color32::WHITE))
                .fill(theme::ACCENT)
                .corner_radius(CARD_RADIUS)
                .min_size(Vec2::new(ui.available_width(), 30.0));
            if ui.add_enabled(enabled, button).on_disabled_hover_text(self.look_blocked()).clicked() {
                run = true;
            }
            ui.add_space(2.0);
            self.look_strength(ui);
        });
        let area = Rect::from_min_max(top, egui::pos2(ui.max_rect().right(), ui.cursor().min.y));
        self.look_drops(ui, area);
        match card_action {
            Some((CardAction::Remove, r)) => self.remove_look_ref(&r),
            Some((CardAction::Open, r)) => self.select_path(&r.path().to_path_buf(), egui::Modifiers::NONE),
            None => {}
        }
        match add {
            Some(AddChoice::Files) => {
                self.pick_look_files();
            }
            Some(AddChoice::Current) => self.add_current_as_look_ref(),
            Some(AddChoice::Reference) => self.add_reference_as_look_ref(),
            None => {}
        }
        if run {
            self.match_look_selection();
        }
    }

    /// The menu of the add tile: a picture file, the open image or the reference image of the
    /// compare view (disabled when there is none, or when it is a template already).
    fn look_add_menu(&self, resp: &egui::Response) -> Option<AddChoice> {
        let resp = resp.clone().on_hover_text("Bilddatei, dieses Bild oder das Referenzbild als Vorlage");
        let mut choice = None;
        Popup::menu(&resp).show(|ui| {
            ui.set_min_width(220.0);
            if ui
                .button(format!("{}  Bilddatei …", icon::FOLDER_OPEN))
                .on_hover_text("JPEG, PNG, TIFF oder WebP, etwa ein Filmstill; mehrere möglich. Dateien lassen sich auch hierher ziehen.")
                .clicked()
            {
                choice = Some(AddChoice::Files);
            }
            let is_template = |p: &PathBuf| self.look_refs.contains(&LookRef::Image(p.clone()));
            let current = self.current_path();
            let current_in = current.as_ref().is_some_and(is_template);
            if ui
                .add_enabled(current.is_some() && !current_in, egui::Button::new(format!("{}  Dieses Bild", icon::IMAGE)))
                .on_hover_text("Das geöffnete Bild, so wie es bearbeitet ist")
                .on_disabled_hover_text(if current_in { "Dieses Bild ist schon eine Vorlage" } else { "Kein Bild geöffnet" })
                .clicked()
            {
                choice = Some(AddChoice::Current);
            }
            let reference = self.reference_for_look();
            let reference_in = reference.as_ref().is_some_and(is_template);
            let text = match &reference {
                Some(p) => format!("{}  Referenzbild · {}", icon::PUSH_PIN, sk_core::variant::display_name(p)),
                None => format!("{}  Referenzbild", icon::PUSH_PIN),
            };
            let no_reference = crate::commands::hint("Kein Referenzbild festgelegt", "reference");
            if ui
                .add_enabled(reference.is_some() && !reference_in, egui::Button::new(text))
                .on_hover_text("Das angeheftete Bild der Vergleichsansicht, so wie es bearbeitet ist")
                .on_disabled_hover_text(if reference_in { "Das Referenzbild ist schon eine Vorlage" } else { no_reference.as_str() })
                .clicked()
            {
                choice = Some(AddChoice::Reference);
            }
        });
        choice
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
        let from = *strength;
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
            let at = |t: f32| mix.opts.blend(&cur.adjustments, &mix.before, &mix.matched, t / 100.0, cur.as_shot_temp_tint);
            cur.adjustments = crate::develop_tools::remix(&cur.adjustments, &at(from), &at(mix.strength));
            self.request_render();
        }
    }
}

/// What was clicked on a template card.
enum CardAction {
    Remove,
    Open,
}

/// What the add menu adds.
enum AddChoice {
    Files,
    Current,
    Reference,
}

/// The add tile: alone (no template yet), in the last grid cell, or as a bar below the grid.
#[derive(Clone, Copy, PartialEq)]
enum AddTile {
    Empty,
    Cell,
    Bar,
}

const CARD_GAP: f32 = 6.0;
const CARD_RADIUS: u8 = 6;

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

/// A small label on a dark translucent pill, placed at `pos` by `align`, with an optional colored
/// dot in front.
fn chip(painter: &egui::Painter, pos: egui::Pos2, align: egui::Align2, dot: Option<Color32>, text: String, color: Color32, max_width: f32) {
    let dot_w = if dot.is_some() { 10.0 } else { 0.0 };
    let mut job = egui::text::LayoutJob::simple_singleline(text, egui::FontId::proportional(11.0), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width((max_width - 12.0 - dot_w).max(10.0));
    let g = painter.layout_job(job);
    let rect = align.anchor_size(pos, Vec2::new(g.size().x + 12.0 + dot_w, 18.0));
    painter.rect_filled(rect, 9.0, Color32::from_black_alpha(170));
    if let Some(c) = dot {
        painter.circle_filled(egui::pos2(rect.left() + 9.0, rect.center().y), 3.0, c);
    }
    painter.galley(egui::pos2(rect.left() + 6.0 + dot_w, rect.center().y - g.size().y / 2.0), g, color);
}

/// The add tile: dashed outline (it also takes dropped files), plus and text, in the accent
/// color on hover.
fn paint_add_tile(ui: &Ui, rect: Rect, resp: &egui::Response, kind: AddTile) {
    let painter = ui.painter();
    let hot = resp.hovered();
    let color = if hot { theme::ACCENT } else { theme::TEXT_DIM };
    if hot {
        painter.rect_filled(rect, CARD_RADIUS, theme::ACCENT.gamma_multiply(0.08));
    }
    let r = rect.shrink(0.5);
    let outline = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    let edge = if hot { theme::ACCENT } else { Color32::from_gray(78) };
    painter.extend(egui::Shape::dashed_line(&outline, egui::Stroke::new(1.0, edge), 4.0, 3.0));
    let title = format!("Vorlage hinzufügen  {}", icon::CARET_DOWN);
    let font = egui::FontId::proportional(12.5);
    match kind {
        AddTile::Bar => {
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, format!("{}  {title}", icon::PLUS), font, color);
        }
        AddTile::Cell => {
            painter.text(rect.center() - Vec2::new(0.0, 9.0), egui::Align2::CENTER_CENTER, icon::PLUS, egui::FontId::proportional(20.0), color);
            painter.text(rect.center() + Vec2::new(0.0, 13.0), egui::Align2::CENTER_CENTER, title, font, color);
        }
        AddTile::Empty => {
            painter.text(rect.center() - Vec2::new(0.0, 18.0), egui::Align2::CENTER_CENTER, icon::PLUS, egui::FontId::proportional(20.0), color);
            painter.text(rect.center() + Vec2::new(0.0, 4.0), egui::Align2::CENTER_CENTER, title, font, color);
            painter.text(
                rect.center() + Vec2::new(0.0, 21.0),
                egui::Align2::CENTER_CENTER,
                "Auswählen oder Bilddatei hierher ziehen",
                egui::FontId::proportional(11.0),
                theme::TEXT_FAINT,
            );
        }
    }
}

/// A toggle as a rounded chip, filled with the accent when on.
fn option_chip(ui: &mut Ui, on: bool, text: &str) -> egui::Response {
    let font = egui::FontId::proportional(12.0);
    let width = ui.painter().layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE).size().x;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width + 22.0, 24.0), Sense::click());
    let (fill, edge, color) = if on {
        (theme::ACCENT_SOFT, theme::ACCENT, Color32::WHITE)
    } else if resp.hovered() {
        (Color32::from_rgb(54, 54, 60), Color32::from_gray(80), theme::TEXT)
    } else {
        (Color32::TRANSPARENT, Color32::from_gray(64), theme::TEXT_DIM)
    };
    let painter = ui.painter();
    painter.rect(rect, 12.0, fill, egui::Stroke::new(1.0, edge), egui::StrokeKind::Inside);
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, text, font, color);
    resp
}

impl App {
    /// One template as a card: the picture filling it, its name at the bottom, the similarity
    /// to the open image at the top (or why it can't be used), its share of the last match as a
    /// thin bar, and on hover an outline and a remove button.
    fn look_card(&self, ui: &mut Ui, rect: Rect, r: &LookRef, share: Option<f32>) -> Option<CardAction> {
        let id = ui.id().with(("look-card", r));
        let resp = ui.interact(rect, id, Sense::click());
        let close = Rect::from_center_size(rect.right_top() + Vec2::new(-14.0, 14.0), Vec2::splat(20.0));
        let close_resp = ui.interact(close, id.with("remove"), Sense::click());
        let hovered = resp.hovered() || close_resp.hovered();
        let painter = ui.painter_at(rect.expand(1.0));
        painter.rect_filled(rect, CARD_RADIUS, Color32::from_gray(24));
        let state = self.look_ref_state.get(r);
        match state {
            Some(LookRefState::Ready(t)) => {
                // Cropped to fill the card.
                let [w, h] = t.size().map(|v| v as f32);
                let (ta, ba) = (w / h, rect.width() / rect.height());
                let uv = if ta > ba {
                    let u = (1.0 - ba / ta) / 2.0;
                    Rect::from_min_max(egui::pos2(u, 0.0), egui::pos2(1.0 - u, 1.0))
                } else {
                    let v = (1.0 - ta / ba) / 2.0;
                    Rect::from_min_max(egui::pos2(0.0, v), egui::pos2(1.0, 1.0 - v))
                };
                egui::Image::new((t.id(), rect.size())).uv(uv).corner_radius(CARD_RADIUS).paint_at(ui, rect);
            }
            Some(LookRefState::Failed(_)) => {
                painter.text(rect.center(), egui::Align2::CENTER_CENTER, icon::WARNING, egui::FontId::proportional(22.0), theme::DANGER);
            }
            _ => {
                painter.text(rect.center(), egui::Align2::CENTER_CENTER, "…", egui::FontId::proportional(16.0), theme::TEXT_FAINT);
            }
        }

        let inner = rect.width() - 10.0;
        chip(&painter, rect.left_bottom() + Vec2::new(5.0, -7.0), egui::Align2::LEFT_BOTTOM, None, r.name(), theme::TEXT, inner);
        let similarity = self.look_similarity_of(r);
        match (state, similarity) {
            (Some(LookRefState::Failed(_)), _) => {
                chip(&painter, rect.left_top() + Vec2::new(5.0, 5.0), egui::Align2::LEFT_TOP, None, "Nicht verwendbar".into(), theme::DANGER, inner - 24.0);
            }
            (_, Some(s)) => {
                let (_, color) = similarity_words(s);
                chip(&painter, rect.left_top() + Vec2::new(5.0, 5.0), egui::Align2::LEFT_TOP, Some(color), format!("{:.0} %", s * 100.0), theme::TEXT, inner - 24.0);
            }
            _ => {}
        }
        if let Some(w) = share {
            let track = Rect::from_min_size(egui::pos2(rect.left() + 6.0, rect.bottom() - 4.0), Vec2::new(rect.width() - 12.0, 2.0));
            painter.rect_filled(track, 1.0, Color32::from_white_alpha(40));
            painter.rect_filled(Rect::from_min_size(track.min, Vec2::new((track.width() * w.clamp(0.0, 1.0)).max(2.0), 2.0)), 1.0, theme::ACCENT);
        }
        let edge = if hovered { egui::Stroke::new(1.5, theme::ACCENT) } else { egui::Stroke::new(1.0, Color32::from_white_alpha(16)) };
        painter.rect_stroke(rect, CARD_RADIUS, edge, egui::StrokeKind::Inside);
        if hovered {
            let fill = if close_resp.hovered() { theme::DANGER_FILL } else { Color32::from_black_alpha(170) };
            painter.circle_filled(close.center(), 10.0, fill);
            painter.text(close.center(), egui::Align2::CENTER_CENTER, icon::X, egui::FontId::proportional(12.0), Color32::WHITE);
        }

        let mut action = None;
        if close_resp.on_hover_text("Vorlage entfernen").clicked() {
            action = Some(CardAction::Remove);
        }
        let is_image = matches!(r, LookRef::Image(_));
        let resp = resp.on_hover_ui(|ui| {
            ui.label(RichText::new(r.name()).strong());
            ui.label(RichText::new(if is_image { "Bild der Bibliothek" } else { "Bilddatei" }).small().weak());
            match (state, similarity) {
                (Some(LookRefState::Failed(e)), _) => {
                    ui.label(RichText::new(format!("Nicht verwendbar: {e}")).color(theme::DANGER));
                }
                (_, Some(s)) => {
                    ui.label(format!("{} zum aktuellen Bild ({:.0} %)", similarity_words(s).0, s * 100.0));
                    ui.label(RichText::new("Ähnliche Vorlagen bestimmen den Look stärker.").small().weak());
                }
                _ => {}
            }
            if let Some(w) = share {
                ui.label(format!("Anteil am letzten Angleichen: {:.0} %", w * 100.0));
            }
            if is_image {
                ui.label(RichText::new("Klick: dieses Bild öffnen").small().weak());
            }
        });
        if is_image && resp.clicked() {
            action = Some(CardAction::Open);
        }
        resp.context_menu(|ui| {
            if is_image && ui.button(format!("{}  Öffnen", icon::IMAGE)).clicked() {
                action = Some(CardAction::Open);
            }
            if ui.button(format!("{}  Vorlage entfernen", icon::X)).clicked() {
                action = Some(CardAction::Remove);
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
