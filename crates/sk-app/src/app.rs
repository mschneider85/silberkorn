//! Application state, editing (undo/redo, copy/apply), events, shortcuts and toolbar.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use sk_core::catalog::{self, Catalog, Filter, ImageRecord, Scope, SortKey};
use sk_core::export::ExportRecipe;
use sk_core::image::LinearImage;
use sk_core::params::{Adjustments, WbMode, WhiteBalance};
use sk_core::pipeline::{Histogram, Processor, RenderOptions};
use sk_core::raw::RawMeta;
use sk_core::session::Session;
use sk_core::sidecar::{self, ColorLabel, Sidecar};
use sk_core::styles::Style;
use egui::{Key, Modifiers, TextureHandle, TextureOptions, Ui, Vec2};
use serde::{Deserialize, Serialize};

use crate::library::{CatalogLists, LibraryUi};
use crate::session::{PendingAction, SessionUi};
use crate::workers::{Event, ExportJob, RenderJob, Workers};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Library,
    Lens,
    Exposure,
    Color,
    Details,
    Layers,
    Metadata,
    Styles,
    Output,
}

impl Tab {
    pub const ALL: [Tab; 9] =
        [Tab::Library, Tab::Lens, Tab::Exposure, Tab::Color, Tab::Details, Tab::Layers, Tab::Metadata, Tab::Styles, Tab::Output];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Library => "Bibliothek",
            Tab::Lens => "Objektiv & Geometrie",
            Tab::Exposure => "Belichtung",
            Tab::Color => "Farbe",
            Tab::Details => "Details",
            Tab::Layers => "Ebenen & Retusche",
            Tab::Metadata => "Metadaten",
            Tab::Styles => "Stile",
            Tab::Output => "Ausgabe",
        }
    }

    pub fn icon(self) -> &'static str {
        use crate::theme::icon;
        match self {
            Tab::Library => icon::IMAGES,
            Tab::Lens => icon::APERTURE,
            Tab::Exposure => icon::SUN,
            Tab::Color => icon::PALETTE,
            Tab::Details => icon::MAGNIFYING_GLASS_PLUS,
            Tab::Layers => icon::STACK,
            Tab::Metadata => icon::INFO,
            Tab::Styles => icon::SPARKLE,
            Tab::Output => icon::EXPORT,
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
    pub region: sk_core::geometry::Region,
    pub version: u64,
    pub full_size: (usize, usize),
}

/// Settings persisted between sessions.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    last_folder: Option<PathBuf>,
    /// Session file that was open when the app was closed.
    last_session: Option<PathBuf>,
    recent_sessions: Vec<PathBuf>,
    /// Legacy single recipe (before multiple recipes existed).
    recipe: Option<ExportRecipe>,
    recipes: Vec<ExportRecipe>,
    sort: SortKey,
    sort_desc: bool,
}

fn settings_path() -> Option<PathBuf> {
    Some(sk_core::paths::config_dir().join("settings.json"))
}

pub struct App {
    pub(crate) workers: Workers,
    persist_settings: bool,
    /// The catalog database, or the session database while a session is open.
    pub(crate) catalog: Option<Catalog>,
    /// Database used when no session is open.
    pub(crate) global_catalog: PathBuf,
    pub session: Option<Session>,
    pub recent_sessions: Vec<PathBuf>,
    pub session_ui: SessionUi,
    /// Action chosen in a browser context menu, run after the browser has been drawn.
    pub pending: Option<PendingAction>,
    pub lists: CatalogLists,
    pub library_ui: LibraryUi,
    pub styles: Vec<Style>,
    /// Lensfun profiles, loaded on first use.
    pub lens_db: Option<Arc<Vec<sk_core::lensfun::LensProfile>>>,
    pub lens_db_downloading: bool,
    pub lens_db_missing: bool,

    // browser
    pub scope: Scope,
    pub filter: Filter,
    pub sort: SortKey,
    pub sort_desc: bool,
    pub folder: PathBuf,
    /// Last folder browsed outside a session (restored when the session is closed).
    pub(crate) catalog_folder: PathBuf,
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
    pub local_tool: Option<crate::layers::LocalTool>,
    pub local_drag: crate::layers::LocalDrag,
    pub active_layer: Option<usize>,
    pub brush: crate::layers::BrushSettings,
    pub spot_size: f32,
    pub selected_spot: Option<usize>,
    pub show_mask: bool,
    pub split_view: bool,
    /// Divider position (0..1 of the image width).
    pub split_pos: f32,
    pub split_dragging: bool,
    /// Shown while a Speed Edit key is held.
    pub speed_message: Option<String>,
    pub zoom_blocked_until: f64,
    pub before_texture: Option<TextureHandle>,
    before_key: Option<u64>,
    pub magic_tolerance: f32,
    pub ai_busy: bool,
    pub model_missing: bool,
    pub model_downloading: bool,
    pub tile: Option<Tile>,
    /// Requested tile (version, region), to avoid resending identical jobs.
    pub tile_requested: Option<(u64, sk_core::geometry::Region)>,
    /// Incremented on every change that affects the rendering.
    pub edit_version: u64,
    pub pointer_down: bool,
    pub clip_warning: bool,
    pub show_before: bool,
    render_requested: bool,
    pub(crate) undo: Vec<Adjustments>,
    pub(crate) redo: Vec<Adjustments>,
    pub(crate) clipboard: Option<Adjustments>,
    pub sidecar_dirty: bool,
    pub keyword_input: String,
    /// None = fit to window; otherwise screen pixels per preview pixel.
    pub zoom: Option<f32>,
    pub pan: Vec2,
    pub scroll_to_primary: bool,

    // output
    pub recipes: Vec<ExportRecipe>,
    pub selected_recipe: usize,
    pub export_progress: Option<(usize, usize)>,
    pub export_log: Vec<String>,

    pub status: String,
    pub show_filmstrip: bool,
    pub show_shortcuts: bool,
    pub show_about: bool,
    window_title: String,
    /// Zoom of the viewer relative to the full sensor resolution, for the status bar.
    pub zoom_pct: Option<f32>,
    /// Debug aid: `SILBERKORN_SCREENSHOT=<file.png>` saves a screenshot once the first image is shown, then quits.
    screenshot: Option<(PathBuf, Option<f64>)>,
}

/// Startup options (separate from `eframe` so that tests can construct the app).
pub struct AppOptions {
    pub start_folder: Option<PathBuf>,
    /// Session file or folder to open at startup (takes precedence over `start_folder`).
    pub session: Option<PathBuf>,
    pub catalog_path: PathBuf,
    /// Read/write `settings.json` (disabled in tests).
    pub persist_settings: bool,
}

impl Default for AppOptions {
    fn default() -> Self {
        Self {
            start_folder: None,
            session: None,
            catalog_path: std::env::var_os("SILBERKORN_CATALOG").map(PathBuf::from).unwrap_or_else(catalog::default_catalog_path),
            persist_settings: true,
        }
    }
}

impl App {
    pub fn new(ctx: &egui::Context, opts: AppOptions) -> Self {
        crate::theme::install(ctx);
        let settings: Settings = settings_path()
            .filter(|_| opts.persist_settings)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        // Without an explicit folder, reopen the session that was open last time.
        let session = opts.session.or_else(|| settings.last_session.clone().filter(|p| p.exists() && opts.start_folder.is_none()));
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
            workers: Workers::spawn(ctx.clone()),
            persist_settings: opts.persist_settings,
            catalog,
            global_catalog: catalog_path,
            session: None,
            recent_sessions: settings.recent_sessions.into_iter().filter(|p| p.exists()).collect(),
            session_ui: SessionUi::default(),
            pending: None,
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
            catalog_folder: folder.clone(),
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
            local_tool: None,
            local_drag: Default::default(),
            active_layer: None,
            brush: Default::default(),
            spot_size: 25.0,
            selected_spot: None,
            show_mask: false,
            split_view: false,
            split_pos: 0.5,
            split_dragging: false,
            speed_message: None,
            zoom_blocked_until: 0.0,
            before_texture: None,
            before_key: None,
            magic_tolerance: 0.3,
            ai_busy: false,
            model_missing: false,
            model_downloading: false,
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
            show_filmstrip: true,
            show_shortcuts: false,
            show_about: false,
            window_title: "Silberkorn".into(),
            zoom_pct: None,
            sidecar_dirty: false,
            keyword_input: String::new(),
            zoom: None,
            pan: Vec2::ZERO,
            scroll_to_primary: false,
            recipes: if !settings.recipes.is_empty() {
                settings.recipes
            } else {
                vec![settings.recipe.unwrap_or_default()]
            },
            selected_recipe: 0,
            export_progress: None,
            export_log: Vec::new(),
            status,
            screenshot: std::env::var_os("SILBERKORN_SCREENSHOT").map(|p| (PathBuf::from(p), None)),
        };
        app.reload_styles();
        match session {
            Some(s) => {
                app.open_session(&s);
                if app.session.is_none() {
                    app.open_folder(folder);
                }
            }
            None => app.open_folder(folder),
        }
        if let Ok(tab) = std::env::var("SILBERKORN_TAB") {
            app.tab = Tab::ALL.into_iter().find(|t| t.title().to_lowercase().contains(&tab.to_lowercase())).unwrap_or(app.tab);
        }
        app.grid_view = std::env::var_os("SILBERKORN_GRID").is_some();
        match std::env::var("SILBERKORN_DIALOG").as_deref() {
            Ok("shortcuts") => app.show_shortcuts = true,
            Ok("about") => app.show_about = true,
            _ => {}
        }
        app
    }

    /// Database file in use: the session's, or the catalog's.
    pub fn catalog_path(&self) -> PathBuf {
        self.session.as_ref().map_or_else(|| self.global_catalog.clone(), |s| s.file.clone())
    }

    pub fn save_settings(&self) {
        let Some(path) = settings_path().filter(|_| self.persist_settings) else { return };
        let s = Settings {
            last_folder: Some(self.catalog_folder.clone()),
            last_session: self.session.as_ref().map(|s| s.file.clone()),
            recent_sessions: self.recent_sessions.clone(),
            recipe: None,
            recipes: self.recipes.clone(),
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

    pub fn toggle_split(&mut self) {
        self.split_view = !self.split_view;
        self.show_before = false;
        self.before_key = None;
        self.request_render();
    }

    /// Layer whose mask is shown as overlay.
    pub fn mask_overlay(&self) -> Option<usize> {
        let drawing = self.local_tool.is_some_and(|t| t.is_mask_tool()) && self.pointer_down;
        if self.show_mask || drawing { self.active_layer } else { None }
    }

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

    pub(crate) fn undo(&mut self) {
        self.commit_and_save();
        let Some(cur) = self.current.as_mut() else { return };
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut cur.adjustments, prev.clone()));
            cur.committed = prev;
            self.sidecar_dirty = true;
            self.request_render();
        }
    }

    pub(crate) fn redo(&mut self) {
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
        let mut recipes: Vec<ExportRecipe> = self.recipes.iter().filter(|r| r.enabled).cloned().collect();
        // In a session, relative output folders belong to the session (default: its Output folder).
        if let Some(s) = &self.session {
            for r in &mut recipes {
                r.output_dir = s.output_dir(&r.output_dir);
            }
        }
        if recipes.is_empty() {
            self.status = "Kein Ausgaberezept aktiviert".into();
            self.export_progress = None;
            return;
        }
        self.workers.export(ExportJob { items, recipes });
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
                            self.active_layer = None;
                            self.selected_spot = None;
                            self.before_texture = None;
                            self.before_key = None;
                            self.local_drag = Default::default();
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
                    if generation != self.generation {
                        continue;
                    }
                    if before && self.split_view {
                        match &mut self.before_texture {
                            Some(t) => t.set(image, TextureOptions::LINEAR),
                            None => self.before_texture = Some(ctx.load_texture("before", image, TextureOptions::LINEAR)),
                        }
                        continue;
                    }
                    if before != self.show_before {
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
                Event::AiMask { generation, layer, result } => {
                    self.ai_busy = false;
                    if generation != self.generation {
                        continue;
                    }
                    match result {
                        Ok(m) => {
                            self.model_missing = false;
                            self.edit(|a| {
                                if let Some(l) = a.layers.get_mut(layer) {
                                    l.mask.ai = Some(merge_ai(l.mask.ai.take(), m));
                                }
                            });
                            self.status = "Maske erstellt".into();
                        }
                        Err(e) if e == "MODEL_MISSING" => {
                            self.model_missing = true;
                            self.status = "KI-Modell fehlt – bitte im Tab „Ebenen“ herunterladen".into();
                        }
                        Err(e) => self.status = format!("Maske fehlgeschlagen: {e}"),
                    }
                }
                Event::ModelDownloaded(r) => {
                    self.model_downloading = false;
                    match r {
                        Ok(p) => {
                            self.model_missing = false;
                            self.status = format!("KI-Modell geladen ({})", p.display());
                        }
                        Err(e) => self.status = format!("Download fehlgeschlagen: {e}"),
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
                Event::IndexDone { catalog, indexed, failed, import } => {
                    self.index_progress = None;
                    if catalog != self.catalog_path() {
                        continue;
                    }
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
        if self.split_view
            && let Some(cur) = &self.current
        {
            let key = sk_core::cache::fnv1a(
                format!("{}|{}|{}", self.generation, cur.adjustments.rotation, serde_json::to_string(&cur.adjustments.geometry).unwrap_or_default()).as_bytes(),
            );
            if self.before_key != Some(key) {
                self.before_key = Some(key);
                self.workers.render_before(RenderJob {
                    generation: self.generation,
                    meta: cur.meta.clone(),
                    preview: cur.preview.clone(),
                    adjustments: cur.adjustments.clone(),
                    clip_warning: false,
                    before: true,
                    ignore_crop: false,
                    mask_overlay: None,
                });
            }
        }
        if let Some(cur) = &self.current {
            self.workers.render(RenderJob {
                generation: self.generation,
                meta: cur.meta.clone(),
                preview: cur.preview.clone(),
                adjustments: cur.adjustments.clone(),
                clip_warning: self.clip_warning,
                before: self.show_before,
                ignore_crop: self.crop_tool.is_some(),
                mask_overlay: self.mask_overlay(),
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
        if pressed(Key::O, Modifiers::COMMAND | Modifiers::SHIFT) {
            self.open_session_dialog();
        }
        if pressed(Key::O, Modifiers::COMMAND) {
            self.open_folder_dialog();
        }
        if pressed(Key::N, Modifiers::COMMAND) {
            self.session_ui.start_new(&self.folder);
        }
        if pressed(Key::J, Modifiers::COMMAND) {
            self.move_to_session_folder(sk_core::session::SessionFolder::Selects);
        }
        if pressed(Key::Delete, Modifiers::COMMAND) || pressed(Key::Backspace, Modifiers::COMMAND) {
            self.move_to_session_folder(sk_core::session::SessionFolder::Trash);
        }
        if pressed(Key::F1, Modifiers::NONE) {
            self.show_shortcuts = !self.show_shortcuts;
        }
        if pressed(Key::A, Modifiers::COMMAND) {
            self.select_all();
        }
        if pressed(Key::F, Modifiers::COMMAND) {
            self.tab = Tab::Library;
        }
        if pressed(Key::OpenBracket, Modifiers::COMMAND) {
            self.rotate(3);
        }
        if pressed(Key::CloseBracket, Modifiers::COMMAND) {
            self.rotate(1);
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
        use crate::layers::LocalTool;
        for (k, t) in [
            (Key::B, LocalTool::Brush),
            (Key::N, LocalTool::Eraser),
            (Key::L, LocalTool::Linear),
            (Key::R, LocalTool::Radial),
            (Key::H, LocalTool::Heal),
            (Key::S, LocalTool::Clone),
        ] {
            if pressed(k, Modifiers::NONE) {
                self.tab = Tab::Layers;
                self.set_local_tool(t);
            }
        }
        if pressed(Key::U, Modifiers::NONE) {
            self.toggle_split();
        }
        if pressed(Key::J, Modifiers::NONE) {
            self.tab = Tab::Layers;
            self.set_local_tool(LocalTool::Magic);
        }
        if pressed(Key::M, Modifiers::NONE) {
            self.show_mask = !self.show_mask;
            self.request_render();
        }
        if pressed(Key::OpenBracket, Modifiers::NONE) {
            self.brush.size = (self.brush.size / 1.2).max(2.0);
            self.spot_size = (self.spot_size / 1.2).max(3.0);
        }
        if pressed(Key::CloseBracket, Modifiers::NONE) {
            self.brush.size = (self.brush.size * 1.2).min(400.0);
            self.spot_size = (self.spot_size * 1.2).min(300.0);
        }
        if pressed(Key::Delete, Modifiers::NONE) || pressed(Key::Backspace, Modifiers::NONE) {
            self.delete_selected_spot();
        }
        if pressed(Key::G, Modifiers::NONE) {
            self.grid_view = !self.grid_view;
        }
        if pressed(Key::E, Modifiers::NONE) {
            self.toggle_clip_warning();
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
            self.toggle_crop();
        }
        if pressed(Key::Enter, Modifiers::NONE) {
            self.finish_crop_tool();
        }
        if pressed(Key::Escape, Modifiers::NONE) {
            self.picking_wb = false;
            self.local_tool = None;
            self.straighten_tool = None;
            self.finish_crop_tool();
        }
    }

    // ---------------------------------------------------------------- layout

    fn debug_screenshot(&mut self, ctx: &egui::Context) {
        let Some((_, shown_at)) = &mut self.screenshot else { return };
        let now = ctx.input(|i| i.time);
        if (self.texture.is_some() || self.grid_view) && shown_at.is_none() {
            *shown_at = Some(now);
            // Optional tool states for screenshots.
            if std::env::var_os("SILBERKORN_CROP").is_some() {
                self.start_crop_tool();
            }
            if let Some(i) = std::env::var("SILBERKORN_LAYER").ok().and_then(|v| v.parse().ok()) {
                self.active_layer = Some(i);
                self.show_mask = std::env::var_os("SILBERKORN_MASK").is_some();
                self.request_render();
            }
            if std::env::var_os("SILBERKORN_SPLIT").is_some() {
                self.toggle_split();
            }
        }
        let wants_zoom = std::env::var_os("SILBERKORN_ZOOM").is_some();
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

/// Union (max) of an existing AI mask with a new one of the same kind of resolution.
fn merge_ai(old: Option<sk_core::params::AiMask>, new: sk_ai::MaskImage) -> sk_core::params::AiMask {
    let mut out = sk_core::params::AiMask { kind: "ai".into(), width: new.width, height: new.height, data: new.data };
    if let Some(o) = old
        && o.width > 0
        && o.height > 0
    {
        for (i, v) in out.data.iter_mut().enumerate() {
            let (x, y) = (i % out.width, i / out.width);
            let ox = (x * o.width / out.width).min(o.width - 1);
            let oy = (y * o.height / out.height).min(o.height - 1);
            *v = (*v).max(o.data[oy * o.width + ox]);
        }
    }
    out
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pointer_down = ctx.input(|i| i.pointer.any_down());
        if !crate::browser::SPEED_KEYS.iter().any(|(k, _)| ctx.input(|i| i.key_down(*k))) {
            self.speed_message = None;
        }
        self.handle_events(&ctx);
        self.shortcuts(&ctx);
        let title = self.session.as_ref().map_or_else(|| "Silberkorn".to_string(), |s| format!("{} – Silberkorn", s.name));
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }

        use crate::chrome::bar_margin;
        use crate::theme::{self, chrome_frame};
        egui::Panel::top("menu").frame(chrome_frame(egui::Margin::symmetric(6, 2))).show_separator_line(false).show(ui, |ui| self.menu_bar(ui));
        egui::Panel::top("toolbar").frame(chrome_frame(bar_margin())).show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status").frame(chrome_frame(egui::Margin::symmetric(10, 3))).show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("tabrail").frame(chrome_frame(egui::Margin::symmetric(2, 2))).resizable(false).exact_size(46.0).show(ui, |ui| self.tab_rail(ui));
        egui::Panel::left("tools")
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin { left: 8, right: 6, top: 0, bottom: 0 }))
            .resizable(true)
            .default_size(340.0)
            .size_range(280.0..=560.0)
            .show(ui, |ui| self.tool_panel(ui));
        if !self.grid_view && self.show_filmstrip {
            egui::Panel::bottom("filmstrip")
                .frame(egui::Frame::new().fill(theme::BROWSER).inner_margin(egui::Margin::same(6)))
                .resizable(true)
                .default_size(150.0)
                .min_size(100.0)
                .show(ui, |ui| self.filmstrip(ui));
        }
        egui::CentralPanel::no_frame().show(ui, |ui| if self.grid_view { self.grid(ui) } else { self.viewer(ui) });
        self.dialogs(&ctx);
        self.session_dialogs(&ctx);
        self.run_pending();

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
