//! Application state, layout, viewer, filmstrip and keyboard shortcuts.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use cr_core::export::ExportRecipe;
use cr_core::image::{LinearImage, Orientation};
use cr_core::params::{Adjustments, WbMode, WhiteBalance};
use cr_core::raw::{self, RawMeta};
use cr_core::sidecar::{self, ColorLabel, Sidecar};
use egui::{Color32, Key, Modifiers, Pos2, Rect, RichText, Sense, TextureHandle, TextureOptions, Ui, Vec2, pos2, vec2};
use serde::{Deserialize, Serialize};

use crate::widgets;
use crate::workers::{Event, ExportJob, RenderJob, Workers};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Library,
    Exposure,
    Color,
    Metadata,
    Output,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::Library, Tab::Exposure, Tab::Color, Tab::Metadata, Tab::Output];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Library => "📚 Bibliothek",
            Tab::Exposure => "☀ Belichtung",
            Tab::Color => "🎨 Farbe",
            Tab::Metadata => "ℹ Metadaten",
            Tab::Output => "💾 Ausgabe",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurveChannel {
    Rgb,
    Red,
    Green,
    Blue,
}

impl CurveChannel {
    pub const ALL: [CurveChannel; 4] = [CurveChannel::Rgb, CurveChannel::Red, CurveChannel::Green, CurveChannel::Blue];

    pub fn label(self) -> &'static str {
        match self {
            CurveChannel::Rgb => "RGB",
            CurveChannel::Red => "Rot",
            CurveChannel::Green => "Grün",
            CurveChannel::Blue => "Blau",
        }
    }
}

pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub rating: u8,
    pub color_label: ColorLabel,
    pub thumb: Option<TextureHandle>,
    pub thumb_requested: bool,
}

/// The image open in the viewer.
pub struct Current {
    pub index: usize,
    pub meta: Arc<RawMeta>,
    pub preview: Arc<LinearImage>,
    pub adjustments: Adjustments,
    /// Last state pushed to the undo stack / sidecar.
    pub committed: Adjustments,
    pub rating: u8,
    pub color_label: ColorLabel,
    pub as_shot_temp_tint: (f32, f32),
}

/// Settings persisted between sessions.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    last_folder: Option<PathBuf>,
    recipe: ExportRecipe,
}

fn settings_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("capture-room").join("settings.json"))
}

pub struct App {
    workers: Workers,
    pub folder: PathBuf,
    pub folder_input: String,
    pub subdirs: Vec<PathBuf>,
    pub entries: Vec<Entry>,
    pub selection: BTreeSet<usize>,
    pub current: Option<Current>,
    loading: Option<usize>,
    load_error: Option<String>,
    generation: u64,
    texture: Option<TextureHandle>,
    pub histogram: Option<cr_core::pipeline::Histogram>,
    pub tab: Tab,
    pub curve_channel: CurveChannel,
    pub picking_wb: bool,
    clip_warning: bool,
    show_before: bool,
    render_requested: bool,
    undo: Vec<Adjustments>,
    redo: Vec<Adjustments>,
    clipboard: Option<Adjustments>,
    pub sidecar_dirty: bool,
    pub recipe: ExportRecipe,
    pub export_progress: Option<(usize, usize)>,
    pub export_log: Vec<String>,
    pub filter_rating: u8,
    pub filter_label: Option<ColorLabel>,
    /// None = fit to window; otherwise screen pixels per preview pixel.
    zoom: Option<f32>,
    pan: Vec2,
    scroll_to_primary: bool,
    status: String,
    /// Debug aid: `CAPTURE_ROOM_SCREENSHOT=<file.png>` saves a screenshot once the first image is shown, then quits.
    screenshot: Option<(PathBuf, Option<f64>)>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, start_folder: Option<PathBuf>) -> Self {
        setup_style(&cc.egui_ctx);
        let settings: Settings = settings_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let folder = start_folder
            .or(settings.last_folder.filter(|p| p.is_dir()))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let mut app = Self {
            workers: Workers::spawn(cc.egui_ctx.clone()),
            folder: PathBuf::new(),
            folder_input: String::new(),
            subdirs: Vec::new(),
            entries: Vec::new(),
            selection: BTreeSet::new(),
            current: None,
            loading: None,
            load_error: None,
            generation: 0,
            texture: None,
            histogram: None,
            tab: Tab::Exposure,
            curve_channel: CurveChannel::Rgb,
            picking_wb: false,
            clip_warning: false,
            show_before: false,
            render_requested: false,
            undo: Vec::new(),
            redo: Vec::new(),
            clipboard: None,
            sidecar_dirty: false,
            recipe: settings.recipe,
            export_progress: None,
            export_log: Vec::new(),
            filter_rating: 0,
            filter_label: None,
            zoom: None,
            pan: Vec2::ZERO,
            scroll_to_primary: false,
            status: String::new(),
            screenshot: std::env::var_os("CAPTURE_ROOM_SCREENSHOT").map(|p| (PathBuf::from(p), None)),
        };
        app.open_folder(folder);
        if let Ok(tab) = std::env::var("CAPTURE_ROOM_TAB") {
            app.tab = Tab::ALL.into_iter().find(|t| t.label().to_lowercase().contains(&tab.to_lowercase())).unwrap_or(app.tab);
        }
        app
    }

    fn save_settings(&self) {
        let Some(path) = settings_path() else { return };
        let s = Settings { last_folder: Some(self.folder.clone()), recipe: self.recipe.clone() };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(&s) {
            let _ = std::fs::write(path, json);
        }
    }

    // ---------------------------------------------------------------- library

    pub fn open_folder(&mut self, folder: PathBuf) {
        self.commit_and_save();
        let folder = folder.canonicalize().unwrap_or(folder);
        let Ok(read) = std::fs::read_dir(&folder) else {
            self.status = format!("Ordner nicht lesbar: {}", folder.display());
            return;
        };
        let mut files = Vec::new();
        let mut dirs = Vec::new();
        for entry in read.flatten() {
            let path = entry.path();
            let hidden = path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if path.is_dir() && !hidden {
                dirs.push(path);
            } else if raw::is_raw_file(&path) {
                files.push(path);
            }
        }
        files.sort();
        dirs.sort();
        self.entries = files
            .into_iter()
            .map(|path| {
                let sc = sidecar::load(&path);
                Entry {
                    name: path.file_name().unwrap_or_default().to_string_lossy().to_string(),
                    path,
                    rating: sc.rating,
                    color_label: sc.color_label,
                    thumb: None,
                    thumb_requested: false,
                }
            })
            .collect();
        self.subdirs = dirs;
        self.folder_input = folder.display().to_string();
        self.folder = folder;
        self.selection.clear();
        self.current = None;
        self.texture = None;
        self.histogram = None;
        self.loading = None;
        self.status = format!("{} RAW-Dateien", self.entries.len());
        self.save_settings();
        if let Some(&first) = self.visible_indices().first() {
            self.select(first, Modifiers::NONE);
        }
    }

    pub fn visible_indices(&self) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.rating >= self.filter_rating && self.filter_label.is_none_or(|l| e.color_label == l))
            .map(|(i, _)| i)
            .collect()
    }

    fn primary(&self) -> Option<usize> {
        self.loading.or(self.current.as_ref().map(|c| c.index))
    }

    fn select(&mut self, index: usize, mods: Modifiers) {
        if mods.command {
            if !self.selection.insert(index) {
                self.selection.remove(&index);
            }
            return;
        }
        if mods.shift
            && let Some(anchor) = self.primary()
        {
            let visible = self.visible_indices();
            let (a, b) = (visible.iter().position(|&i| i == anchor), visible.iter().position(|&i| i == index));
            if let (Some(a), Some(b)) = (a, b) {
                self.selection.extend(visible[a.min(b)..=a.max(b)].iter().copied());
            }
            return;
        }
        self.selection = BTreeSet::from([index]);
        if self.primary() != Some(index) {
            self.commit_and_save();
            self.generation += 1;
            self.loading = Some(index);
            self.load_error = None;
            self.picking_wb = false;
            self.workers.load(self.generation, self.entries[index].path.clone());
        }
    }

    fn step(&mut self, delta: isize) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        let pos = self.primary().and_then(|p| visible.iter().position(|&i| i == p)).unwrap_or(0);
        let next = (pos as isize + delta).clamp(0, visible.len() as isize - 1) as usize;
        self.scroll_to_primary = true;
        self.select(visible[next], Modifiers::NONE);
    }

    // ---------------------------------------------------------------- editing

    pub fn request_render(&mut self) {
        self.render_requested = true;
    }

    /// Pushes finished edits to the undo stack and writes the sidecar.
    fn commit_and_save(&mut self) {
        let Some(cur) = self.current.as_mut() else { return };
        if cur.adjustments != cur.committed {
            self.undo.push(std::mem::replace(&mut cur.committed, cur.adjustments.clone()));
            self.redo.clear();
            self.sidecar_dirty = true;
        }
        if self.sidecar_dirty {
            self.sidecar_dirty = false;
            let sc = Sidecar {
                rating: cur.rating,
                color_label: cur.color_label,
                adjustments: cur.committed.clone(),
                ..Default::default()
            };
            if let Err(e) = sidecar::save(&cur.meta.path, &sc) {
                self.status = format!("Speichern fehlgeschlagen: {e:#}");
            }
            if let Some(entry) = self.entries.get_mut(cur.index) {
                entry.rating = cur.rating;
                entry.color_label = cur.color_label;
            }
        }
    }

    fn undo(&mut self) {
        self.commit_and_save();
        let Some(cur) = self.current.as_mut() else { return };
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut cur.adjustments, prev.clone()));
            cur.committed = prev;
            self.sidecar_dirty = true;
            self.request_render();
        }
    }

    fn redo(&mut self) {
        let Some(cur) = self.current.as_mut() else { return };
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut cur.adjustments, next.clone()));
            cur.committed = next;
            self.sidecar_dirty = true;
            self.request_render();
        }
    }

    fn edit(&mut self, f: impl FnOnce(&mut Adjustments)) {
        if let Some(cur) = self.current.as_mut() {
            f(&mut cur.adjustments);
            self.request_render();
        }
    }

    fn set_rating(&mut self, rating: u8) {
        if let Some(cur) = self.current.as_mut() {
            cur.rating = rating;
            self.sidecar_dirty = true;
        }
        // Apply to the rest of the selection as well.
        let primary = self.primary();
        for &i in &self.selection {
            if Some(i) != primary {
                let e = &mut self.entries[i];
                e.rating = rating;
                let mut sc = sidecar::load(&e.path);
                sc.rating = rating;
                let _ = sidecar::save(&e.path, &sc);
            }
        }
    }

    fn copy_adjustments(&mut self) {
        if let Some(cur) = &self.current {
            self.clipboard = Some(cur.adjustments.clone());
            self.status = "Anpassungen kopiert".into();
        }
    }

    fn paste_adjustments(&mut self) {
        let Some(clip) = self.clipboard.clone() else { return };
        let primary = self.current.as_ref().map(|c| c.index);
        let targets: Vec<usize> = self.selection.iter().copied().collect();
        for i in &targets {
            if Some(*i) == primary {
                self.edit(|a| *a = Adjustments { rotation: a.rotation, ..clip.clone() });
            } else {
                let path = self.entries[*i].path.clone();
                let mut sc = sidecar::load(&path);
                sc.adjustments = Adjustments { rotation: sc.adjustments.rotation, ..clip.clone() };
                let _ = sidecar::save(&path, &sc);
                self.entries[*i].thumb_requested = false;
            }
        }
        self.status = format!("Anpassungen auf {} Bild(er) angewendet", targets.len());
    }

    pub fn export_selection(&mut self) {
        self.commit_and_save();
        let items: Vec<(PathBuf, Adjustments)> = self
            .selection
            .iter()
            .map(|&i| {
                let path = self.entries[i].path.clone();
                let adj = match &self.current {
                    Some(c) if c.index == i => c.committed.clone(),
                    _ => sidecar::load(&path).adjustments,
                };
                (path, adj)
            })
            .collect();
        if items.is_empty() {
            return;
        }
        self.export_progress = Some((0, items.len()));
        self.status = format!("Verarbeite {} Bild(er)…", items.len());
        self.save_settings();
        self.workers.export(ExportJob { items, recipe: self.recipe.clone() });
    }

    fn pick_white_balance(&mut self, image_pos: Vec2) {
        let Some(cur) = self.current.as_mut() else { return };
        let p = &cur.preview;
        let rot = cur.adjustments.rotation % 4;
        let o = Orientation::from_quarter_turns(rot);
        let (dw, dh) = if o.swaps_dimensions() { (p.height, p.width) } else { (p.width, p.height) };
        let x = (image_pos.x as usize).min(dw - 1);
        let y = (image_pos.y as usize).min(dh - 1);
        let mut acc = [0.0f32; 3];
        let mut n = 0.0;
        for yy in y.saturating_sub(3)..(y + 4).min(dh) {
            for xx in x.saturating_sub(3)..(x + 4).min(dw) {
                let (sx, sy) = o.source_coord(xx, yy, p.width, p.height);
                let v = p.data[sy * p.width + sx];
                acc = [acc[0] + v[0], acc[1] + v[1], acc[2] + v[2]];
                n += 1.0;
            }
        }
        let avg = acc.map(|v| v / n);
        if avg.iter().any(|v| *v <= 1e-5) || avg.iter().any(|v| *v >= 0.98) {
            self.status = "Pipette: Stelle ist zu dunkel oder ausgebrannt".into();
            return;
        }
        let wb = [avg[1] / avg[0], 1.0, avg[1] / avg[2]];
        let (kelvin, tint) = cur.meta.color.temp_tint_for_wb(wb);
        cur.adjustments.white_balance = WhiteBalance { mode: WbMode::Custom, kelvin, tint };
        self.status = format!("Weißabgleich: {kelvin:.0} K, Tint {tint:.1}");
        self.picking_wb = false;
        self.request_render();
    }

    // ---------------------------------------------------------------- events

    fn handle_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.workers.events.try_recv() {
            match ev {
                Event::Loaded { generation, result } if generation == self.generation => {
                    let index = self.loading.take().unwrap_or(0);
                    match result {
                        Ok((meta, preview)) => {
                            let sc = sidecar::load(&meta.path);
                            // Rounded to slider precision so that displaying it never counts as an edit.
                            let (k, t) = meta.color.temp_tint_for_wb(meta.as_shot_wb);
                            let as_shot_temp_tint = (k.round(), (t * 10.0).round() / 10.0);
                            self.current = Some(Current {
                                index,
                                meta,
                                preview,
                                adjustments: sc.adjustments.clone(),
                                committed: sc.adjustments,
                                rating: sc.rating,
                                color_label: sc.color_label,
                                as_shot_temp_tint,
                            });
                            self.undo.clear();
                            self.redo.clear();
                            self.zoom = None;
                            self.pan = Vec2::ZERO;
                            self.request_render();
                        }
                        Err(e) => {
                            self.current = None;
                            self.texture = None;
                            self.load_error = Some(e);
                        }
                    }
                }
                Event::Loaded { .. } => {}
                Event::Rendered { generation, image, thumb, histogram, before } => {
                    if generation != self.generation || before != self.show_before {
                        continue;
                    }
                    match &mut self.texture {
                        Some(t) => t.set(image, TextureOptions::LINEAR),
                        None => self.texture = Some(ctx.load_texture("viewer", image, TextureOptions::LINEAR)),
                    }
                    self.histogram = Some(histogram);
                    if let (Some(thumb), Some(cur)) = (thumb, &self.current)
                        && let Some(entry) = self.entries.get_mut(cur.index)
                    {
                        set_thumb(ctx, entry, thumb);
                    }
                }
                Event::Thumbnail { path, image } => {
                    if let Some(entry) = self.entries.iter_mut().find(|e| e.path == path) {
                        set_thumb(ctx, entry, image);
                    }
                }
                Event::ExportProgress { done, total, last } => {
                    self.export_progress = (done < total).then_some((done, total));
                    match last {
                        Ok(p) => self.export_log.push(format!("✔ {}", p.display())),
                        Err(e) => self.export_log.push(format!("✖ {e}")),
                    }
                    if done == total {
                        self.status = format!("Verarbeitung abgeschlossen ({total})");
                    }
                }
            }
        }
    }

    fn send_render(&mut self) {
        if !self.render_requested {
            return;
        }
        self.render_requested = false;
        if let Some(cur) = &self.current {
            self.workers.render(RenderJob {
                generation: self.generation,
                meta: cur.meta.clone(),
                preview: cur.preview.clone(),
                adjustments: cur.adjustments.clone(),
                clip_warning: self.clip_warning,
                before: self.show_before,
            });
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let pressed = |k: Key, m: Modifiers| ctx.input_mut(|i| i.consume_key(m, k));
        if pressed(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT) || pressed(Key::Y, Modifiers::COMMAND) {
            self.redo();
        }
        if pressed(Key::Z, Modifiers::COMMAND) {
            self.undo();
        }
        if pressed(Key::C, Modifiers::COMMAND | Modifiers::SHIFT) {
            self.copy_adjustments();
        }
        if pressed(Key::V, Modifiers::COMMAND | Modifiers::SHIFT) {
            self.paste_adjustments();
        }
        if pressed(Key::D, Modifiers::COMMAND) {
            self.export_selection();
        }
        if pressed(Key::A, Modifiers::COMMAND) {
            self.selection = self.visible_indices().into_iter().collect();
        }
        if pressed(Key::OpenBracket, Modifiers::COMMAND) {
            self.edit(|a| a.rotation = (a.rotation + 3) % 4);
        }
        if pressed(Key::CloseBracket, Modifiers::COMMAND) {
            self.edit(|a| a.rotation = (a.rotation + 1) % 4);
        }
        if pressed(Key::ArrowRight, Modifiers::NONE) {
            self.step(1);
        }
        if pressed(Key::ArrowLeft, Modifiers::NONE) {
            self.step(-1);
        }
        for (k, r) in [(Key::Num0, 0), (Key::Num1, 1), (Key::Num2, 2), (Key::Num3, 3), (Key::Num4, 4), (Key::Num5, 5)] {
            if pressed(k, Modifiers::NONE) {
                self.set_rating(r);
            }
        }
        if pressed(Key::E, Modifiers::NONE) {
            self.clip_warning = !self.clip_warning;
            self.request_render();
        }
        if pressed(Key::Y, Modifiers::NONE) {
            self.show_before = !self.show_before;
            self.request_render();
        }
        if pressed(Key::W, Modifiers::NONE) {
            self.picking_wb = !self.picking_wb;
        }
        if pressed(Key::Z, Modifiers::NONE) {
            self.zoom = if self.zoom.is_some() { None } else { Some(1.0) };
            self.pan = Vec2::ZERO;
        }
        if pressed(Key::Escape, Modifiers::NONE) {
            self.picking_wb = false;
        }
    }

    // ---------------------------------------------------------------- layout

    fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Capture Room").strong().size(15.0));
            ui.separator();
            let has = self.current.is_some();
            if ui.add_enabled(!self.undo.is_empty() || self.current.as_ref().is_some_and(|c| c.adjustments != c.committed), egui::Button::new("↩"))
                .on_hover_text("Rückgängig (Strg+Z)")
                .clicked()
            {
                self.undo();
            }
            if ui.add_enabled(!self.redo.is_empty(), egui::Button::new("↪")).on_hover_text("Wiederholen (Strg+Y)").clicked() {
                self.redo();
            }
            ui.separator();
            if ui.add_enabled(has, egui::Button::new("⟲")).on_hover_text("Links drehen (Strg+[)").clicked() {
                self.edit(|a| a.rotation = (a.rotation + 3) % 4);
            }
            if ui.add_enabled(has, egui::Button::new("⟳")).on_hover_text("Rechts drehen (Strg+])").clicked() {
                self.edit(|a| a.rotation = (a.rotation + 1) % 4);
            }
            ui.separator();
            if ui.add_enabled(has, egui::Button::new("📋 Kopieren")).on_hover_text("Anpassungen kopieren (Strg+Umschalt+C)").clicked() {
                self.copy_adjustments();
            }
            if ui
                .add_enabled(self.clipboard.is_some() && !self.selection.is_empty(), egui::Button::new("📥 Anwenden"))
                .on_hover_text("Auf Auswahl anwenden (Strg+Umschalt+V)")
                .clicked()
            {
                self.paste_adjustments();
            }
            if ui.add_enabled(has, egui::Button::new("↺ Zurücksetzen")).on_hover_text("Alle Anpassungen zurücksetzen").clicked() {
                self.edit(|a| *a = Adjustments { rotation: a.rotation, ..Default::default() });
            }
            ui.separator();
            if ui.selectable_label(self.show_before, "◐ Vorher").on_hover_text("Vorher/Nachher (Y)").clicked() {
                self.show_before = !self.show_before;
                self.request_render();
            }
            if ui.selectable_label(self.clip_warning, "⚠ Warnung").on_hover_text("Belichtungswarnung (E)").clicked() {
                self.clip_warning = !self.clip_warning;
                self.request_render();
            }
            if ui.selectable_label(self.zoom.is_none(), "⛶ Einpassen").on_hover_text("Einpassen / 100 % (Z)").clicked() {
                self.zoom = None;
                self.pan = Vec2::ZERO;
            }
            ui.separator();
            if ui.add_enabled(!self.selection.is_empty(), egui::Button::new("⚙ Verarbeiten")).on_hover_text("Auswahl exportieren (Strg+D)").clicked() {
                self.export_selection();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(&self.status).color(Color32::from_gray(160)));
            });
        });
    }

    fn viewer(&mut self, ui: &mut Ui) {
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_gray(38));

        let Some(tex) = &self.texture else {
            let msg = if let Some(e) = &self.load_error {
                format!("Fehler: {e}")
            } else if self.loading.is_some() {
                "Lade…".into()
            } else if self.entries.is_empty() {
                "Keine RAW-Dateien in diesem Ordner.\nIm Tab „Bibliothek“ einen Ordner öffnen.".into()
            } else {
                String::new()
            };
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, msg, egui::FontId::proportional(16.0), Color32::from_gray(170));
            return;
        };
        let size = tex.size_vec2();
        let fit = ((rect.width() - 20.0) / size.x).min((rect.height() - 20.0) / size.y).max(0.01);
        let scale = self.zoom.unwrap_or(fit);

        // Mouse wheel zoom around the cursor.
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let new_scale = (scale * (1.0 + scroll * 0.0015)).clamp(fit.min(0.05), 8.0);
                if let Some(cursor) = response.hover_pos() {
                    let center = rect.center() + self.pan;
                    let rel = (cursor - center) / scale;
                    self.pan = cursor - rect.center() - rel * new_scale;
                }
                self.zoom = Some(new_scale);
            }
        }
        if response.dragged() && !self.picking_wb {
            self.pan += response.drag_delta();
        }
        let scale = self.zoom.unwrap_or(fit);
        if self.zoom.is_none() {
            self.pan = Vec2::ZERO;
        }
        let img_rect = Rect::from_center_size(rect.center() + self.pan, size * scale);
        painter.image(tex.id(), img_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);

        if self.picking_wb {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            if response.clicked()
                && let Some(pos) = response.interact_pointer_pos()
                && img_rect.contains(pos)
            {
                self.pick_white_balance((pos - img_rect.min) / scale);
            }
        } else if response.double_clicked() {
            self.zoom = if self.zoom.is_some() { None } else { Some(1.0) };
            self.pan = Vec2::ZERO;
        }

        // Overlay: file name, zoom, before marker.
        if let Some(cur) = &self.current {
            let full_w = if (cur.adjustments.rotation % 2) == 1 { cur.meta.info.height } else { cur.meta.info.width } as f32;
            // Zoom relative to the full sensor resolution.
            let pct = scale * size.x / full_w.max(1.0) * 100.0;
            let text = format!(
                "{}   {pct:.0} %{}",
                cur.meta.path.file_name().unwrap_or_default().to_string_lossy(),
                if self.show_before { "   VORHER" } else { "" }
            );
            let galley = painter.layout_no_wrap(text, egui::FontId::proportional(13.0), Color32::from_gray(220));
            let pos = rect.left_top() + vec2(10.0, 8.0);
            painter.rect_filled(Rect::from_min_size(pos, galley.size()).expand(4.0), 3.0, Color32::from_black_alpha(160));
            painter.galley(pos, galley, Color32::from_gray(220));
        }
        if self.loading.is_some() {
            painter.text(rect.right_top() + vec2(-10.0, 8.0), egui::Align2::RIGHT_TOP, "Lade…", egui::FontId::proportional(13.0), Color32::from_gray(200));
        }
    }

    fn filmstrip(&mut self, ui: &mut Ui) {
        let visible = self.visible_indices();
        let mut clicked: Option<(usize, Modifiers)> = None;
        let primary = self.primary();
        let cell = vec2(150.0, ui.available_height().clamp(90.0, 260.0) - 8.0);
        egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, |ui| {
            ui.horizontal(|ui| {
                for &i in &visible {
                    let (rect, resp) = ui.allocate_exact_size(cell, Sense::click());
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    let entry = &mut self.entries[i];
                    if !entry.thumb_requested {
                        entry.thumb_requested = true;
                        self.workers.thumbnail(entry.path.clone());
                    }
                    let selected = self.selection.contains(&i);
                    let painter = ui.painter_at(rect);
                    let bg = if Some(i) == primary {
                        Color32::from_gray(85)
                    } else if selected {
                        Color32::from_gray(62)
                    } else if resp.hovered() {
                        Color32::from_gray(50)
                    } else {
                        Color32::from_gray(32)
                    };
                    painter.rect_filled(rect, 3.0, bg);
                    let img_area = Rect::from_min_max(rect.min + vec2(5.0, 5.0), rect.max - vec2(5.0, 20.0));
                    if let Some(t) = &entry.thumb {
                        let s = t.size_vec2();
                        let k = (img_area.width() / s.x).min(img_area.height() / s.y);
                        let r = Rect::from_center_size(img_area.center(), s * k);
                        painter.image(t.id(), r, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    painter.text(
                        pos2(rect.center().x, rect.bottom() - 10.0),
                        egui::Align2::CENTER_CENTER,
                        &entry.name,
                        egui::FontId::proportional(11.0),
                        Color32::from_gray(190),
                    );
                    widgets::paint_badges(ui, img_area, entry.rating, entry.color_label.rgb());
                    if resp.clicked() {
                        clicked = Some((i, ui.input(|inp| inp.modifiers)));
                    }
                    if Some(i) == primary && self.scroll_to_primary {
                        resp.scroll_to_me(Some(egui::Align::Center));
                    }
                }
            });
        });
        self.scroll_to_primary = false;
        if let Some((i, m)) = clicked {
            self.select(i, m);
        }
    }
}

impl App {
    fn debug_screenshot(&mut self, ctx: &egui::Context) {
        let Some((path, shown_at)) = &mut self.screenshot else { return };
        let now = ctx.input(|i| i.time);
        if self.texture.is_some() && shown_at.is_none() {
            *shown_at = Some(now);
        }
        if let Some(t) = *shown_at {
            if now - t > 2.5 && now - t < 100.0 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                *shown_at = Some(t - 1000.0);
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let shot = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            let buf = ::image::RgbaImage::from_raw(img.size[0] as u32, img.size[1] as u32, img.as_raw().to_vec());
            if let Some(buf) = buf {
                let _ = buf.save(&*path);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

fn set_thumb(ctx: &egui::Context, entry: &mut Entry, image: egui::ColorImage) {
    match &mut entry.thumb {
        Some(t) => t.set(image, TextureOptions::LINEAR),
        None => entry.thumb = Some(ctx.load_texture(format!("thumb:{}", entry.path.display()), image, TextureOptions::LINEAR)),
    }
}

fn setup_style(ctx: &egui::Context) {
    ctx.set_visuals(egui::Visuals::dark());
    ctx.global_style_mut(|s| {
        s.spacing.slider_width = 150.0;
        s.spacing.item_spacing = vec2(6.0, 4.0);
    });
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_events(&ctx);
        self.shortcuts(&ctx);

        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
        egui::Panel::left("tools").resizable(true).default_size(330.0).min_size(260.0).show(ui, |ui| self.tools_ui(ui));
        egui::Panel::bottom("filmstrip").resizable(true).default_size(150.0).min_size(100.0).show(ui, |ui| self.filmstrip(ui));
        egui::CentralPanel::no_frame().show(ui, |ui| self.viewer(ui));

        // Commit edits once the user lets go of the mouse (one undo step per gesture).
        if !ctx.input(|i| i.pointer.any_down()) {
            self.commit_and_save();
        }
        self.send_render();
        self.debug_screenshot(&ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.commit_and_save();
        self.save_settings();
    }
}
