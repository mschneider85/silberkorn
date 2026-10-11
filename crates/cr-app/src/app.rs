//! Application state, editing (undo/redo, copy/apply), events, shortcuts and toolbar.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use cr_core::catalog::{self, Catalog, Filter, ImageRecord, Scope, SortKey};
use cr_core::export::ExportRecipe;
use cr_core::image::LinearImage;
use cr_core::params::{Adjustments, WbMode, WhiteBalance};
use cr_core::pipeline::{Histogram, Processor, RenderOptions};
use cr_core::raw::RawMeta;
use cr_core::sidecar::{self, ColorLabel, Sidecar};
use cr_core::styles::Style;
use egui::{Color32, Key, Modifiers, RichText, TextureHandle, TextureOptions, Ui, Vec2, vec2};
use serde::{Deserialize, Serialize};

use crate::library::{CatalogLists, LibraryUi, scope_label};
use crate::workers::{Event, ExportJob, RenderJob, Workers};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Library,
    Lens,
    Exposure,
    Color,
    Details,
    Metadata,
    Styles,
    Output,
}

impl Tab {
    pub const ALL: [Tab; 8] =
        [Tab::Library, Tab::Lens, Tab::Exposure, Tab::Color, Tab::Details, Tab::Metadata, Tab::Styles, Tab::Output];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Library => "📚 Bibliothek",
            Tab::Lens => "📐 Objektiv",
            Tab::Details => "🔍 Details",
            Tab::Exposure => "☀ Belichtung",
            Tab::Color => "🎨 Farbe",
            Tab::Metadata => "ℹ Metadaten",
            Tab::Styles => "✨ Stile",
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

/// The image open in the viewer.
pub struct Current {
    pub meta: Arc<RawMeta>,
    pub preview: Arc<LinearImage>,
    /// Full-resolution camera image, loaded in the background for 100 % viewing.
    pub full: Option<Arc<LinearImage>>,
    pub adjustments: Adjustments,
    /// Last state pushed to the undo stack / sidecar.
    pub committed: Adjustments,
    pub rating: u8,
    pub color_label: ColorLabel,
    pub keywords: Vec<String>,
    pub as_shot_temp_tint: (f32, f32),
}

/// A full-resolution rendering of part of the output, drawn over the preview when zoomed in.
pub struct Tile {
    pub texture: TextureHandle,
    pub region: cr_core::geometry::Region,
    pub version: u64,
    pub full_size: (usize, usize),
}

/// Settings persisted between sessions.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    last_folder: Option<PathBuf>,
    recipe: ExportRecipe,
    sort: SortKey,
    sort_desc: bool,
}

fn settings_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("capture-room").join("settings.json"))
}

pub struct App {
    pub(crate) workers: Workers,
    persist_settings: bool,
    pub(crate) catalog: Option<Catalog>,
    pub lists: CatalogLists,
    pub library_ui: LibraryUi,
    pub styles: Vec<Style>,
    /// Lensfun profiles, loaded on first use.
    pub lens_db: Option<Arc<Vec<cr_core::lensfun::LensProfile>>>,
    pub lens_db_downloading: bool,
    pub lens_db_missing: bool,

    // browser
    pub scope: Scope,
    pub filter: Filter,
    pub sort: SortKey,
    pub sort_desc: bool,
    pub folder: PathBuf,
    pub folder_input: String,
    pub subdirs: Vec<PathBuf>,
    pub entries: Vec<ImageRecord>,
    /// Indices into `entries` that pass the filter, in display order.
    pub visible: Vec<usize>,
    pub selection: HashSet<PathBuf>,
    pub thumbs: HashMap<PathBuf, TextureHandle>,
    pub thumb_requested: HashSet<PathBuf>,
    pub grid_view: bool,
    pub index_progress: Option<(usize, usize)>,

    // viewer / editing
    pub current: Option<Current>,
    pub loading: Option<PathBuf>,
    pub load_error: Option<String>,
    generation: u64,
    pub texture: Option<TextureHandle>,
    pub histogram: Option<Histogram>,
    pub tab: Tab,
    pub curve_channel: CurveChannel,
    /// Selected hue band in the color editor.
    pub color_band: usize,
    pub picking_wb: bool,
    /// Active crop tool state (the viewer shows the uncropped frame).
    pub crop_tool: Option<crate::crop::CropTool>,
    pub straighten_tool: Option<crate::crop::StraightenTool>,
    pub tile: Option<Tile>,
    /// Requested tile (version, region), to avoid resending identical jobs.
    pub tile_requested: Option<(u64, cr_core::geometry::Region)>,
    /// Incremented on every change that affects the rendering.
    pub edit_version: u64,
    pub pointer_down: bool,
    pub clip_warning: bool,
    pub show_before: bool,
    render_requested: bool,
    undo: Vec<Adjustments>,
    redo: Vec<Adjustments>,
    clipboard: Option<Adjustments>,
    pub sidecar_dirty: bool,
    pub keyword_input: String,
    /// None = fit to window; otherwise screen pixels per preview pixel.
    pub zoom: Option<f32>,
    pub pan: Vec2,
    pub scroll_to_primary: bool,

    // output
    pub recipe: ExportRecipe,
    pub export_progress: Option<(usize, usize)>,
    pub export_log: Vec<String>,

    pub status: String,
    /// Debug aid: `CAPTURE_ROOM_SCREENSHOT=<file.png>` saves a screenshot once the first image is shown, then quits.
    screenshot: Option<(PathBuf, Option<f64>)>,
}

/// Startup options (separate from `eframe` so that tests can construct the app).
pub struct AppOptions {
    pub start_folder: Option<PathBuf>,
    pub catalog_path: PathBuf,
    /// Read/write `settings.json` (disabled in tests).
    pub persist_settings: bool,
}

impl Default for AppOptions {
    fn default() -> Self {
        Self {
            start_folder: None,
            catalog_path: std::env::var_os("CAPTURE_ROOM_CATALOG").map(PathBuf::from).unwrap_or_else(catalog::default_catalog_path),
            persist_settings: true,
        }
    }
}

impl App {
    pub fn new(ctx: &egui::Context, opts: AppOptions) -> Self {
        setup_style(ctx);
        let settings: Settings = settings_path()
            .filter(|_| opts.persist_settings)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let folder = opts
            .start_folder
            .or(settings.last_folder.filter(|p| p.is_dir()))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let catalog_path = opts.catalog_path;
        let (catalog, status) = match Catalog::open(&catalog_path) {
            Ok(c) => (Some(c), String::new()),
            Err(e) => (None, format!("Katalog nicht verfügbar: {e:#}")),
        };
        let mut app = Self {
            workers: Workers::spawn(ctx.clone(), catalog_path),
            persist_settings: opts.persist_settings,
            catalog,
            lists: CatalogLists::default(),
            library_ui: LibraryUi::default(),
            styles: Vec::new(),
            lens_db: None,
            lens_db_downloading: false,
            lens_db_missing: false,
            scope: Scope::All,
            filter: Filter::default(),
            sort: settings.sort,
            sort_desc: settings.sort_desc,
            folder: PathBuf::new(),
            folder_input: String::new(),
            subdirs: Vec::new(),
            entries: Vec::new(),
            visible: Vec::new(),
            selection: HashSet::new(),
            thumbs: HashMap::new(),
            thumb_requested: HashSet::new(),
            grid_view: false,
            index_progress: None,
            current: None,
            loading: None,
            load_error: None,
            generation: 0,
            texture: None,
            histogram: None,
            tab: Tab::Exposure,
            curve_channel: CurveChannel::Rgb,
            color_band: 0,
            picking_wb: false,
            crop_tool: None,
            straighten_tool: None,
            tile: None,
            tile_requested: None,
            edit_version: 0,
            pointer_down: false,
            clip_warning: false,
            show_before: false,
            render_requested: false,
            undo: Vec::new(),
            redo: Vec::new(),
            clipboard: None,
            sidecar_dirty: false,
            keyword_input: String::new(),
            zoom: None,
            pan: Vec2::ZERO,
            scroll_to_primary: false,
            recipe: settings.recipe,
            export_progress: None,
            export_log: Vec::new(),
            status,
            screenshot: std::env::var_os("CAPTURE_ROOM_SCREENSHOT").map(|p| (PathBuf::from(p), None)),
        };
        app.reload_styles();
        app.open_folder(folder);
        if let Ok(tab) = std::env::var("CAPTURE_ROOM_TAB") {
            app.tab = Tab::ALL.into_iter().find(|t| t.label().to_lowercase().contains(&tab.to_lowercase())).unwrap_or(app.tab);
        }
        app.grid_view = std::env::var_os("CAPTURE_ROOM_GRID").is_some();
        app
    }

    pub fn save_settings(&self) {
        let Some(path) = settings_path().filter(|_| self.persist_settings) else { return };
        let s = Settings {
            last_folder: Some(self.folder.clone()),
            recipe: self.recipe.clone(),
            sort: self.sort,
            sort_desc: self.sort_desc,
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(&s) {
            let _ = std::fs::write(path, json);
        }
    }

    // ---------------------------------------------------------------- selection

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn current_path(&self) -> Option<PathBuf> {
        self.current.as_ref().map(|c| c.meta.path.clone())
    }

    /// The image shown (or being loaded) in the viewer.
    pub fn primary_path(&self) -> Option<PathBuf> {
        self.loading.clone().or_else(|| self.current_path())
    }

    pub fn close_current(&mut self) {
        self.commit_and_save();
        self.current = None;
        self.loading = None;
        self.texture = None;
        self.histogram = None;
        self.generation += 1;
    }

    pub fn select_path(&mut self, path: &PathBuf, mods: Modifiers) {
        if mods.command {
            if !self.selection.remove(path) {
                self.selection.insert(path.clone());
            }
            return;
        }
        if mods.shift
            && let Some(anchor) = self.primary_path()
        {
            let order: Vec<&PathBuf> = self.visible.iter().map(|&i| &self.entries[i].path).collect();
            if let (Some(a), Some(b)) = (order.iter().position(|p| **p == anchor), order.iter().position(|p| *p == path)) {
                let range: Vec<PathBuf> = order[a.min(b)..=a.max(b)].iter().map(|p| (*p).clone()).collect();
                self.selection.extend(range);
            }
            return;
        }
        self.selection = HashSet::from([path.clone()]);
        if self.primary_path().as_ref() != Some(path) {
            self.commit_and_save();
            self.generation += 1;
            self.loading = Some(path.clone());
            self.load_error = None;
            self.picking_wb = false;
            self.workers.load(self.generation, path.clone());
        }
    }

    pub fn step(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let primary = self.primary_path();
        let pos = self.visible.iter().position(|&i| Some(&self.entries[i].path) == primary.as_ref()).unwrap_or(0);
        let next = (pos as isize + delta).clamp(0, self.visible.len() as isize - 1) as usize;
        let path = self.entries[self.visible[next]].path.clone();
        self.scroll_to_primary = true;
        self.select_path(&path, Modifiers::NONE);
    }

    // ---------------------------------------------------------------- editing

    pub fn request_render(&mut self) {
        self.render_requested = true;
        self.edit_version += 1;
    }

    /// Pushes finished edits to the undo stack and writes the sidecar.
    pub fn commit_and_save(&mut self) {
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
                keywords: cur.keywords.clone(),
                adjustments: cur.committed.clone(),
                ..Default::default()
            };
            if let Err(e) = sidecar::save(&cur.meta.path, &sc) {
                self.status = format!("Speichern fehlgeschlagen: {e:#}");
            }
            if let Some(c) = &self.catalog {
                let _ = c.sync_sidecar(&cur.meta.path, &sc);
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

    pub fn edit(&mut self, f: impl FnOnce(&mut Adjustments)) {
        if let Some(cur) = self.current.as_mut() {
            f(&mut cur.adjustments);
            self.request_render();
        }
    }

    pub(crate) fn copy_adjustments(&mut self) {
        if let Some(cur) = &self.current {
            self.clipboard = Some(cur.adjustments.clone());
            self.status = "Anpassungen kopiert".into();
        }
    }

    pub(crate) fn paste_adjustments(&mut self) {
        let Some(clip) = self.clipboard.clone() else { return };
        let targets = self.selected_paths();
        self.set_adjustments_on(&targets, |a| {
            let mut n = clip.clone();
            n.keep_geometry_from(a);
            n
        });
        self.status = format!("Anpassungen auf {} Bild(er) angewendet", targets.len());
    }

    pub fn export_selection(&mut self) {
        self.commit_and_save();
        let current = self.current.as_ref().map(|c| (c.meta.path.clone(), c.committed.clone()));
        let items: Vec<(PathBuf, Adjustments)> = self
            .selected_paths()
            .into_iter()
            .map(|path| {
                let adj = match &current {
                    Some((p, a)) if *p == path => a.clone(),
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

    /// White balance from a neutral spot; `image_pos` is in displayed preview pixels.
    pub fn pick_white_balance(&mut self, image_pos: Vec2) {
        let Some(cur) = self.current.as_mut() else { return };
        let p = &cur.preview;
        let opts = RenderOptions { ignore_crop: self.crop_tool.is_some(), ..Default::default() };
        let geo = Processor::with_options(&cur.meta, &cur.adjustments, opts).geomap(p.width, p.height);
        let mut acc = [0.0f32; 3];
        let mut n = 0.0;
        for dy in -3..=3 {
            for dx in -3..=3 {
                let (sx, sy) = geo.source_point(image_pos.x + dx as f32, image_pos.y + dy as f32);
                let (sx, sy) = (sx.round(), sy.round());
                if sx < 0.0 || sy < 0.0 || sx >= p.width as f32 || sy >= p.height as f32 {
                    continue;
                }
                let v = p.data[sy as usize * p.width + sx as usize];
                acc = [acc[0] + v[0], acc[1] + v[1], acc[2] + v[2]];
                n += 1.0;
            }
        }
        if n == 0.0 {
            return;
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

    pub(crate) fn handle_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.workers.events.try_recv() {
            match ev {
                Event::Loaded { generation, result } if generation == self.generation => {
                    self.loading = None;
                    match result {
                        Ok((meta, preview)) => {
                            // The rendered preview must fit into a single GPU texture.
                            let max_side = ctx.input(|i| i.max_texture_side).max(512);
                            let long = preview.width.max(preview.height);
                            let preview = if long > max_side {
                                Arc::new((*preview).clone().box_downscale(long.div_ceil(max_side)))
                            } else {
                                preview
                            };
                            let sc = sidecar::load(&meta.path);
                            // Rounded to slider precision so that displaying it never counts as an edit.
                            let (k, t) = meta.color.temp_tint_for_wb(meta.as_shot_wb);
                            let as_shot_temp_tint = (k.round(), (t * 10.0).round() / 10.0);
                            self.workers.load_full(self.generation, meta.path.clone());
                            self.tile = None;
                            self.tile_requested = None;
                            self.crop_tool = None;
                            self.straighten_tool = None;
                            self.current = Some(Current {
                                meta,
                                preview,
                                full: None,
                                adjustments: sc.adjustments.clone(),
                                committed: sc.adjustments,
                                rating: sc.rating,
                                color_label: sc.color_label,
                                keywords: sc.keywords,
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
                    if let (Some(thumb), Some(path)) = (thumb, self.current_path()) {
                        self.set_thumb(ctx, path, thumb);
                    }
                }
                Event::Thumbnail { path, image } => self.set_thumb(ctx, path, image),
                Event::FullLoaded { generation, image } => {
                    if generation == self.generation
                        && let Some(cur) = &mut self.current
                    {
                        cur.full = Some(image);
                    }
                }
                Event::TileRendered { generation, version, region, full_size, image } => {
                    if generation != self.generation {
                        continue;
                    }
                    match &mut self.tile {
                        Some(t) => {
                            t.texture.set(image, TextureOptions::LINEAR);
                            t.region = region;
                            t.version = version;
                            t.full_size = full_size;
                        }
                        None => {
                            let texture = ctx.load_texture("tile", image, TextureOptions::LINEAR);
                            self.tile = Some(Tile { texture, region, version, full_size });
                        }
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
                Event::LensDbDownloaded(r) => {
                    self.lens_db_downloading = false;
                    self.lens_db = None;
                    match r {
                        Ok(dir) => {
                            self.status = format!("Objektivdatenbank geladen ({})", dir.display());
                            self.apply_lens_profile();
                        }
                        Err(e) => self.status = e,
                    }
                }
                Event::IndexProgress { done, total } => {
                    self.index_progress = (done < total).then_some((done, total));
                }
                Event::IndexDone { indexed, failed, import } => {
                    self.index_progress = None;
                    if import {
                        self.status = format!("Import: {indexed} Bilder{}", if failed > 0 { format!(", {failed} Fehler") } else { String::new() });
                        self.open_scope(Scope::LastImport);
                    } else if indexed > 0 || failed > 0 {
                        self.reload_entries();
                    }
                }
            }
        }
    }

    fn set_thumb(&mut self, ctx: &egui::Context, path: PathBuf, image: egui::ColorImage) {
        match self.thumbs.get_mut(&path) {
            Some(t) => t.set(image, TextureOptions::LINEAR),
            None => {
                let tex = ctx.load_texture(format!("thumb:{}", path.display()), image, TextureOptions::LINEAR);
                self.thumbs.insert(path, tex);
            }
        }
    }

    pub(crate) fn send_render(&mut self) {
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
                ignore_crop: self.crop_tool.is_some(),
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
            self.selection = self.visible.iter().map(|&i| self.entries[i].path.clone()).collect();
        }
        if pressed(Key::F, Modifiers::COMMAND) {
            self.tab = Tab::Library;
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
        for (k, l) in [
            (Key::Num6, ColorLabel::Red),
            (Key::Num7, ColorLabel::Yellow),
            (Key::Num8, ColorLabel::Green),
            (Key::Num9, ColorLabel::Blue),
        ] {
            if pressed(k, Modifiers::NONE) {
                self.set_color_label(l);
            }
        }
        if pressed(Key::G, Modifiers::NONE) {
            self.grid_view = !self.grid_view;
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
            self.toggle_zoom_100();
        }
        if pressed(Key::C, Modifiers::NONE) {
            if self.crop_tool.is_some() {
                self.finish_crop_tool();
            } else {
                self.start_crop_tool();
            }
        }
        if pressed(Key::Enter, Modifiers::NONE) {
            self.finish_crop_tool();
        }
        if pressed(Key::Escape, Modifiers::NONE) {
            self.picking_wb = false;
            self.straighten_tool = None;
            self.finish_crop_tool();
        }
    }

    // ---------------------------------------------------------------- layout

    fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Capture Room").strong().size(15.0));
            ui.separator();
            let has = self.current.is_some();
            let can_undo = !self.undo.is_empty() || self.current.as_ref().is_some_and(|c| c.adjustments != c.committed);
            if ui.add_enabled(can_undo, egui::Button::new("↩")).on_hover_text("Rückgängig (Strg+Z)").clicked() {
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
                .add_enabled(self.clipboard.is_some(), egui::Button::new("📥 Anwenden"))
                .on_hover_text("Auf Auswahl anwenden (Strg+Umschalt+V)")
                .clicked()
            {
                self.paste_adjustments();
            }
            if ui.add_enabled(has, egui::Button::new("↺ Zurücksetzen")).on_hover_text("Alle Anpassungen zurücksetzen").clicked() {
                self.edit(|a| {
                    let mut n = Adjustments::default();
                    n.keep_geometry_from(a);
                    *a = n;
                });
            }
            ui.separator();
            if ui.selectable_label(self.grid_view, "⊞ Raster").on_hover_text("Raster / Viewer (G)").clicked() {
                self.grid_view = !self.grid_view;
            }
            if ui.selectable_label(self.show_before, "◑ Vorher").on_hover_text("Vorher/Nachher (Y)").clicked() {
                self.show_before = !self.show_before;
                self.request_render();
            }
            if ui.selectable_label(self.clip_warning, "⚠ Warnung").on_hover_text("Belichtungswarnung (E)").clicked() {
                self.clip_warning = !self.clip_warning;
                self.request_render();
            }
            if ui.selectable_label(self.zoom.is_none(), "⛶ Einpassen").on_hover_text("Einpassen").clicked() {
                self.zoom = None;
                self.pan = Vec2::ZERO;
            }
            if ui.button("1:1").on_hover_text("100 % (Z)").clicked() {
                self.zoom = None;
                self.toggle_zoom_100();
            }
            if ui.selectable_label(self.crop_tool.is_some(), "⛶ Zuschneiden").on_hover_text("Zuschnitt-Werkzeug (C)").clicked() {
                if self.crop_tool.is_some() {
                    self.finish_crop_tool();
                } else {
                    self.start_crop_tool();
                }
            }
            ui.separator();
            let n = self.selected_paths().len();
            if ui.add_enabled(n > 0, egui::Button::new("⚙ Verarbeiten")).on_hover_text("Auswahl exportieren (Strg+D)").clicked() {
                self.export_selection();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(&self.status).color(Color32::from_gray(160)));
                ui.separator();
                ui.label(RichText::new(format!("{} · {} Bilder", scope_label(&self.scope, &self.lists), self.visible.len())).color(Color32::from_gray(190)));
            });
        });
    }

    fn debug_screenshot(&mut self, ctx: &egui::Context) {
        let Some((_, shown_at)) = &mut self.screenshot else { return };
        let now = ctx.input(|i| i.time);
        if (self.texture.is_some() || self.grid_view) && shown_at.is_none() {
            *shown_at = Some(now);
            // Optional tool states for screenshots.
            if std::env::var_os("CAPTURE_ROOM_CROP").is_some() {
                self.start_crop_tool();
            }
        }
        let wants_zoom = std::env::var_os("CAPTURE_ROOM_ZOOM").is_some();
        if wants_zoom && self.zoom.is_none() && self.current.as_ref().is_some_and(|c| c.full.is_some()) {
            self.toggle_zoom_100();
            if let Some((_, Some(t))) = &mut self.screenshot {
                *t = now;
            }
        }
        if wants_zoom && self.zoom.is_none() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        let Some((path, shown_at)) = &mut self.screenshot else { return };
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
            if let Some(buf) = ::image::RgbaImage::from_raw(img.size[0] as u32, img.size[1] as u32, img.as_raw().to_vec()) {
                let _ = buf.save(&*path);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
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
        self.pointer_down = ctx.input(|i| i.pointer.any_down());
        self.handle_events(&ctx);
        self.shortcuts(&ctx);

        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
        egui::Panel::left("tools").resizable(true).default_size(340.0).min_size(260.0).show(ui, |ui| self.tools_ui(ui));
        if !self.grid_view {
            egui::Panel::bottom("filmstrip").resizable(true).default_size(150.0).min_size(100.0).show(ui, |ui| self.filmstrip(ui));
        }
        egui::CentralPanel::no_frame().show(ui, |ui| if self.grid_view { self.grid(ui) } else { self.viewer(ui) });

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
