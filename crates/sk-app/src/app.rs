//! Application state, editing (undo/redo, copy/apply), events, shortcuts and toolbar.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    Retouch,
    Metadata,
    Styles,
    History,
    Output,
}

impl Tab {
    /// In rail order: organizing, developing, presets and history, output.
    pub const ALL: [Tab; 11] = [
        Tab::Library,
        Tab::Metadata,
        Tab::Lens,
        Tab::Exposure,
        Tab::Color,
        Tab::Details,
        Tab::Layers,
        Tab::Retouch,
        Tab::Styles,
        Tab::History,
        Tab::Output,
    ];

    /// Group of the tab in the rail; groups are set apart by a gap.
    pub fn group(self) -> u8 {
        match self {
            Tab::Library | Tab::Metadata => 0,
            Tab::Lens | Tab::Exposure | Tab::Color | Tab::Details | Tab::Layers | Tab::Retouch => 1,
            Tab::Styles | Tab::History => 2,
            Tab::Output => 3,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Tab::Library => "Bibliothek",
            Tab::Lens => "Objektiv & Geometrie",
            Tab::Exposure => "Belichtung",
            Tab::Color => "Farbe",
            Tab::Details => "Details",
            Tab::Layers => "Ebenen",
            Tab::Retouch => "Retusche",
            Tab::Metadata => "Metadaten",
            Tab::Styles => "Stile",
            Tab::History => "Verlauf",
            Tab::Output => "Ausgabe",
        }
    }

    /// Label under the icon in the tab rail.
    pub fn short_title(self) -> &'static str {
        match self {
            Tab::Library => "Bibliothek",
            Tab::Lens => "Objektiv",
            Tab::Exposure => "Belichtung",
            Tab::Color => "Farbe",
            Tab::Details => "Details",
            Tab::Layers => "Ebenen",
            Tab::Retouch => "Retusche",
            Tab::Metadata => "Info",
            Tab::Styles => "Stile",
            Tab::History => "Verlauf",
            Tab::Output => "Ausgabe",
        }
    }

    /// Ctrl+1 … Ctrl+0 in rail order; the history has none (it is also in the side column).
    pub fn key(self) -> Option<Key> {
        const KEYS: [Key; 10] = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9, Key::Num0];
        let i = Tab::ALL.into_iter().filter(|t| *t != Tab::History).position(|t| t == self)?;
        KEYS.get(i).copied()
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
            Tab::Retouch => icon::BANDAIDS,
            Tab::Metadata => icon::INFO,
            Tab::Styles => icon::SWATCHES,
            Tab::History => icon::CLOCK_COUNTER_CLOCKWISE,
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
    pub snapshots: Vec<sk_core::sidecar::Snapshot>,
    /// Why the sidecar couldn't be read when the image was opened: then nothing is saved, so
    /// that the stored settings aren't replaced with the defaults shown instead.
    pub unreadable: Option<String>,
}

/// Undo steps kept per image (each a full copy of the adjustments; masks are shared).
pub(crate) const UNDO_LIMIT: usize = 500;

/// An image that can be shown without loading: recently viewed or prefetched (see `App::prepared`).
struct Ready {
    meta: Arc<RawMeta>,
    preview: Arc<LinearImage>,
    adjustments: Adjustments,
    /// Rendering with `adjustments`, if still valid.
    rendered: Option<(TextureHandle, Histogram)>,
}

/// Edit version, view region with its reduction, and grid tiles (in order) of a tile request.
pub(crate) type TileKey = (u64, Option<(sk_core::geometry::Region, usize)>, Vec<(usize, usize)>);

/// A window that was maximized before full screen.
enum Restore {
    /// Maximize again via egui (it was un-maximized the ordinary way).
    Maximize,
    /// Windows: maximize again in one step, with this normal rect.
    Placement(crate::window_placement::NormalRect),
}

/// A full screen switch in progress (see `App::screen_transition`).
pub(crate) struct ScreenTransition {
    started: Instant,
    /// Window size when the switch was requested; the new size must differ from it.
    initial: egui::Rect,
    /// Screen rect of the last frame and for how many frames it hasn't changed.
    last: Option<egui::Rect>,
    stable_frames: u32,
}

/// The window counts as settled after this many frames without a size change ...
const SETTLED_FRAMES: u32 = 2;
/// ... or after this time at the latest.
const TRANSITION_MAX: Duration = Duration::from_millis(600);
/// Fade-in of the image after the switch.
const FADE_IN: Duration = Duration::from_millis(150);

/// A finished proof of the current image (see `App::proof_view`).
pub struct ProofView {
    pub version: u64,
    pub key: u64,
    pub texture: TextureHandle,
    /// Size of the exported image and its file.
    pub width: usize,
    pub height: usize,
    pub file_size: usize,
}

/// How many recently viewed or prefetched images are kept in memory.
const READY_MAX: usize = 6;
/// The full resolution is only decoded once the user stays on an image this long (or zooms in),
/// so that stepping through images isn't slowed down by it.
const FULL_DELAY: Duration = Duration::from_millis(400);

/// Full-resolution rendering of the current image in a grid of `workers::TILE`-sized tiles,
/// drawn over the preview when zoomed in. Kept until the next edit, so zooming and panning
/// don't render anything again.
#[derive(Default)]
pub struct Tiles {
    /// Edit version the grid tiles were rendered for.
    pub version: u64,
    /// Output size at full resolution.
    pub full_size: (usize, usize),
    pub textures: HashMap<(usize, usize), TextureHandle>,
    /// Focus mask of the grid tiles (when it was on while they were rendered).
    pub focus: HashMap<(usize, usize), crate::focus::FocusMap>,
    /// The visible region, rendered first when zooming in and while dragging a control
    /// (drawn over the grid).
    pub view: Option<ViewTile>,
}

pub struct ViewTile {
    /// In full-resolution output pixels.
    pub region: sk_core::geometry::Region,
    /// The texture has 1/`level` of the region's size (drafts while dragging).
    pub level: usize,
    pub version: u64,
    pub texture: TextureHandle,
    pub focus: Option<crate::focus::FocusMap>,
}

/// Smallest and largest thumbnail size in the grid.
pub const GRID_THUMB: (f32, f32) = (110.0, 420.0);

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
    /// The navigator (minimap) is on by default.
    hide_navigator: bool,
    focus_sensitivity: Option<f32>,
    guides: crate::guides::Guides,
    overlay_file: Option<PathBuf>,
    overlay_opacity: Option<f32>,
    rename_template: String,
    /// The image selected when the app was closed.
    last_image: Option<PathBuf>,
    /// Write rating, label and keywords into XMP sidecars automatically.
    write_xmp: bool,
    viewer_window: crate::viewer_window::ViewerWindow,
    softproof: sk_core::softproof::ProofSettings,
    print: sk_core::print::PrintSettings,
    hdr_options: sk_core::hdr::HdrOptions,
    pano_options: sk_core::panorama::PanoramaOptions,
    /// AI on the graphics card (if available).
    ai_gpu: Option<bool>,
    /// Development pipeline on the graphics card (if available).
    develop_gpu: Option<bool>,
    brushes: crate::layers::Brushes,
    /// Thumbnail size in the grid.
    grid_thumb: Option<f32>,
    side_column: crate::side_column::SideColumn,
}

/// Thumbnail textures kept at most (see `App::trim_thumbs`).
pub(crate) const MAX_THUMBS: usize = 2000;

fn settings_path() -> Option<PathBuf> {
    Some(sk_core::paths::config_dir().join("settings.json"))
}

/// Reads the settings. A file that can't be parsed (e.g. written by a newer version) is kept as
/// `settings.json.bak` before the defaults replace it, so recipes and brushes aren't lost for
/// good; the error is returned as a status message.
fn load_settings(path: &Path) -> (Settings, Option<String>) {
    let Ok(text) = std::fs::read_to_string(path) else { return (Settings::default(), None) };
    match serde_json::from_str(&text) {
        Ok(s) => (s, None),
        Err(e) => {
            let mut backup = path.as_os_str().to_owned();
            backup.push(".bak");
            let kept = std::fs::rename(path, &backup).is_ok();
            log::warn!("Einstellungen nicht lesbar: {e}");
            let note = if kept { format!(", gesichert als {}", Path::new(&backup).display()) } else { String::new() };
            (Settings::default(), Some(format!("Einstellungen nicht lesbar ({e}){note}")))
        }
    }
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
    /// Built-in film looks with descriptions (see `styles::film_simulations`).
    pub film_looks: Vec<(Style, &'static str)>,
    /// Shown instead of the real adjustments while the pointer is over a style.
    pub(crate) style_preview: Option<Adjustments>,
    /// A style row is hovered this frame (otherwise the preview ends).
    pub(crate) style_hovered: bool,
    /// Lensfun profiles, read in the background at startup.
    pub lens_db: Option<Arc<Vec<sk_core::lensfun::LensProfile>>>,
    pub lens_db_downloading: bool,
    /// Lensfun profile name for an EXIF lens name (the search takes about a millisecond).
    pub lens_match: Option<(String, Option<String>)>,

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
    /// Stacks shown with all their images (see `stacks.rs`); others show only their top image.
    pub expanded_stacks: HashSet<String>,
    /// Number of images in each stack of `entries`.
    pub stack_sizes: HashMap<String, usize>,
    pub auto_stack: Option<crate::stacks::AutoStackDialog>,
    pub thumbs: HashMap<PathBuf, TextureHandle>,
    /// When each thumbnail was last drawn (egui pass), to drop the oldest beyond [`MAX_THUMBS`].
    pub(crate) thumb_seen: HashMap<PathBuf, u64>,
    pub thumb_requested: HashSet<PathBuf>,
    pub grid_view: bool,
    pub index_progress: Option<(usize, usize)>,
    /// Background generation of viewer previews.
    pub proxy_progress: Option<(usize, usize)>,

    // viewer / editing
    pub current: Option<Current>,
    pub loading: Option<PathBuf>,
    pub load_error: Option<String>,
    generation: u64,
    pub texture: Option<TextureHandle>,
    /// Edit version of the rendering in `texture`.
    texture_version: u64,
    /// `texture` is a half-resolution draft (made while dragging a control).
    pub(crate) texture_draft: bool,
    /// Size of the full (non-draft) rendering: the coordinate system of the viewer, also
    /// while a smaller draft texture is shown.
    pub view_size: Option<Vec2>,
    /// The viewer shows the image magnified (full-resolution renderings cover the screen).
    pub(crate) view_magnified: bool,
    /// Shown while the selected image is loading or rendering: its thumbnail, then its
    /// cached preview or camera JPEG.
    pub placeholder: Option<TextureHandle>,
    /// Recently viewed and prefetched images, most recent last.
    prepared: Vec<Ready>,
    /// Files moved or renamed so far: results requested before may belong to another file now
    /// going by the same path.
    pub(crate) relocations: u64,
    /// Generation for which neighbours have been prefetched.
    prefetched_for: u64,
    /// Direction of the last step through the images (for prefetching).
    step_dir: isize,
    /// When to start decoding the full resolution of the current image.
    full_due: Option<Instant>,
    pub histogram: Option<Histogram>,
    pub tab: Tab,
    /// The tab of the last frame, to notice a change of tab.
    tab_shown: Tab,
    pub curve_channel: CurveChannel,
    /// Selected hue band in the color editor.
    pub color_band: usize,
    /// Color editor view: all colors for one property, or one color with all properties.
    pub color_view: crate::develop_tools::ColorView,
    pub picking_wb: bool,
    /// The skin tone pipette is active (color tab).
    pub picking_skin: bool,
    /// The focus pipette of the lens blur is active (details tab).
    pub picking_focus: bool,
    /// Active crop tool state (the viewer shows the uncropped frame).
    pub crop_tool: Option<crate::crop::CropTool>,
    pub straighten_tool: Option<crate::crop::StraightenTool>,
    pub local_tool: Option<crate::layers::LocalTool>,
    pub local_drag: crate::layers::LocalDrag,
    pub active_layer: Option<usize>,
    /// Layer whose name is being edited in the layer list.
    pub(crate) renaming_layer: Option<crate::layers::Rename>,
    /// Gradient of the active layer shown with its handles (linear and radial tools).
    pub(crate) selected_gradient: Option<crate::gradients::GradRef>,
    /// Settings of each brush tool.
    pub brushes: crate::layers::Brushes,
    /// Brush settings changed at this time (saved once they rest).
    pub(crate) brushes_changed_at: Option<Instant>,
    /// Painted area of the remove tool as shown (stroke key, texture, area).
    pub(crate) remove_overlay: Option<(u64, TextureHandle, [f32; 4])>,
    /// Guide for automatic masking in the viewer (generation, guide).
    pub(crate) auto_guide: Option<(u64, Arc<sk_core::brush::AutoGuide>)>,
    pub selected_spot: Option<usize>,
    pub show_mask: bool,
    /// Mask overlay of the latest render request (see `send_render`).
    rendered_mask_overlay: Option<usize>,
    pub split_view: bool,
    /// Before and after next to each other instead of the divider.
    pub split_side: bool,
    /// Divider position (0..1 of the image width).
    pub split_pos: f32,
    pub split_dragging: bool,
    /// Shown while a Speed Edit key is held.
    pub speed_message: Option<String>,
    pub zoom_blocked_until: f64,
    pub before_texture: Option<TextureHandle>,
    before_key: Option<u64>,
    pub magic_tolerance: f32,
    /// Strokes of the remove tool not yet filled (or being filled).
    pub remove_strokes: Vec<sk_core::params::BrushStroke>,
    pub inpaint_busy: bool,
    pub removal_progress: f32,
    pub show_gpu_dialog: bool,
    pub ai_busy: bool,
    /// Clicks of the object tool on the active layer (see `layers::ObjectSession`).
    pub object_session: Option<crate::layers::ObjectSession>,
    /// Running search for dust and blemishes: the image and the heal spots found.
    pub spot_search: Option<std::sync::mpsc::Receiver<(PathBuf, Vec<sk_core::params::Spot>)>>,
    /// 0..1, see `sk_core::spots::find`.
    pub spot_sensitivity: f32,
    pub tiles: Tiles,
    /// Last tile request, to avoid resending identical requests.
    pub(crate) tiles_requested: Option<TileKey>,
    /// Time of the last change to the rendering; tiles outside the view wait until editing pauses.
    pub(crate) edited_at: Instant,
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
    /// Settings whose sidecar couldn't be written (read-only folder, newer version): kept here so
    /// that switching images doesn't lose them, and written again with the next change.
    pub(crate) unsaved: HashMap<PathBuf, Sidecar>,
    pub keyword_input: String,
    /// None = fit to window; otherwise screen pixels per preview pixel.
    pub zoom: Option<f32>,
    pub pan: Vec2,
    pub scroll_to_primary: bool,
    /// Whether the filmstrip was shown last frame; when it appears, it scrolls to the image.
    pub filmstrip_shown: bool,

    // compare view (see `compare.rs`)
    /// Pinned reference image, shown next to the current one; also the source for matching.
    pub compare_ref: Option<PathBuf>,
    /// Several selected images are shown side by side.
    pub multi_view: bool,
    /// Renderings of the other images in the compare view.
    pub(crate) compare_textures: HashMap<PathBuf, TextureHandle>,
    pub(crate) compare_requested: HashSet<PathBuf>,
    /// Zoomed-in parts of the compare images at a higher resolution, and what was asked for.
    pub(crate) compare_details: HashMap<PathBuf, crate::compare::CompareDetail>,
    pub(crate) details_requested: Vec<crate::compare::DetailWant>,
    /// Zoom of all compare cells relative to fitting (1 = fit) ...
    pub compare_zoom: f32,
    /// ... and the image point (0..1) at the center of the cells.
    pub compare_center: Vec2,
    /// Images still being matched to the reference: (done, total).
    pub match_progress: Option<(usize, usize)>,
    /// Sharp areas are marked in green (focus mask).
    pub focus_mask: bool,
    /// 0..100: higher marks more.
    pub focus_sensitivity: f32,
    pub(crate) focus_maps: HashMap<PathBuf, crate::focus::FocusMap>,
    /// Images whose focus mask is being computed.
    pub(crate) focus_pending: HashSet<PathBuf>,
    /// Generation for which a rendering was requested to compute the focus mask.
    pub(crate) focus_render_for: Option<(u64, u64)>,
    /// Variant waiting for confirmation of its deletion.
    pub confirm_delete_variant: Option<Vec<PathBuf>>,
    pub rename: Option<crate::rename::RenameDialog>,
    /// Image to select once its folder or session is shown after starting (the last one).
    pub(crate) restore_image: Option<PathBuf>,
    /// Snapshot being renamed in the history tab, with the text typed so far.
    pub snapshot_rename: Option<(usize, String)>,
    /// `SILBERKORN_LOUPE=x,y`: loupe shown at this relative position (screenshots).
    pub debug_loupe: Option<Vec2>,
    /// Last template of the rename dialog.
    pub rename_template: String,

    // output
    pub recipes: Vec<ExportRecipe>,
    pub selected_recipe: usize,
    pub export_progress: Option<(usize, usize)>,
    /// Progress within the image being exported (while enlarging), and its number of outputs.
    pub export_step: Option<f32>,
    pub export_recipes: usize,
    /// Cancels the running export.
    pub export_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Closing was requested while work was running; the user is asked first.
    pub quit_asked: bool,
    quit_confirmed: bool,
    pub export_log: Vec<String>,

    pub status: String,
    pub show_filmstrip: bool,
    /// Thumbnail size (long edge) in the grid.
    pub grid_thumb: f32,
    /// Columns of the grid as last drawn (for the up/down keys).
    pub grid_cols: usize,
    /// Command palette (Ctrl+K).
    pub palette: crate::palette::Palette,
    /// Minimap of the whole image while zoomed in.
    pub show_navigator: bool,
    pub side_column: crate::side_column::SideColumn,
    /// The right column is shown this frame.
    pub side_column_shown: bool,
    /// Viewer area of the last frame (decides whether the right column fits).
    pub viewer_rect: Option<egui::Rect>,
    /// Viewport and image rectangle of the last frame (for the navigator in the right column).
    pub view_geometry: Option<(egui::Rect, egui::Rect)>,
    /// Reduction of the whole-image draft and of the view draft, fixed while a control is dragged.
    pub(crate) drag_draft_level: Option<usize>,
    pub(crate) drag_view_level: Option<usize>,
    /// Rendered while the mouse was down (perhaps without reduction): render once more after release.
    pub(crate) drag_rendered: bool,
    pub guides: crate::guides::Guides,
    pub overlay: crate::guides::Overlay,
    /// Distraction-free full screen: only the image.
    pub fullscreen: bool,
    /// The "Esc / F11" hint in full screen is shown until then.
    pub(crate) fullscreen_hint_until: Option<Instant>,
    /// How to bring back a window that was maximized before full screen.
    restore: Option<Restore>,
    /// Windows: maximize again once winit has left full screen (see `window_placement`).
    pending_maximize: Option<crate::window_placement::NormalRect>,
    /// Native handle of the app window (Windows), for `window_placement`.
    native_window: Option<isize>,
    /// The window manager's animations are off for a full screen switch (back on afterwards).
    animations_off: bool,
    /// Switching full screen: the window passes through several sizes, so only black is shown
    /// until it has settled; then the image fades in from this time.
    pub(crate) screen_transition: Option<ScreenTransition>,
    pub(crate) fade_in_from: Option<Instant>,
    /// Shows the image as the selected export recipe writes it.
    pub proof_view: bool,
    pub(crate) proof: Option<ProofView>,
    /// (generation, edit version, recipe key) of the last proof request.
    proof_requested: Option<(u64, u64, u64)>,
    pub show_shortcuts: bool,
    /// Start screen (also the "about" dialog).
    pub splash: Option<crate::splash::Splash>,
    pub(crate) window_title: String,
    /// Zoom of the viewer relative to the full sensor resolution, for the status bar.
    pub zoom_pct: Option<f32>,
    /// Physical pixels per UI point (Windows display scaling), for true 100 % viewing.
    pub(crate) pixels_per_point: f32,
    /// Largest texture side the GPU takes: a view rendering must fit into one texture.
    pub(crate) max_texture_side: usize,
    /// Debug aid: `SILBERKORN_SCREENSHOT=<file.png>` saves a screenshot once the first image is shown, then quits.
    screenshot: Option<(PathBuf, Option<f64>)>,
    /// The viewer in a window of its own (see `viewer_window.rs`).
    pub viewer_window: crate::viewer_window::ViewerWindow,
    /// The viewer window has been created (its full screen state is only set on creation).
    pub(crate) viewer_window_shown: bool,
    /// A mouse button is held in the viewer window (as of its last frame).
    pub(crate) viewer_pointer_down: bool,
    /// The viewer window was moved or resized at this time (saved once it rests).
    pub(crate) viewer_window_moved: Option<Instant>,
    pub softproof: crate::softproof::SoftProofState,
    pub print: crate::print_preview::PrintPreviewState,
    /// Background jobs creating files (see `jobs.rs`).
    pub jobs: Vec<crate::jobs::RunningJob>,
    /// A created file to select once it is listed.
    pub(crate) select_when_listed: Option<PathBuf>,
    pub merge_dialog: Option<crate::merge::MergeDialog>,
    pub hdr_options: sk_core::hdr::HdrOptions,
    pub pano_options: sk_core::panorama::PanoramaOptions,
    /// Model downloads and what waits for them (see `ai_models`).
    pub ai_downloads: crate::ai_models::AiDownloads,
    /// Background jobs whose end is not reported in the status bar.
    pub quiet_jobs: Vec<String>,
    /// Images waiting for their AI denoised data.
    pub denoise: crate::denoise::DenoiseQueue,
    /// Depth maps being computed (lens blur, depth masks).
    pub depth: crate::depth::DepthQueue,
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
        let (settings, settings_error) = settings_path().filter(|_| opts.persist_settings).map_or((Settings::default(), None), |p| load_settings(&p));
        // Without an explicit folder, reopen the session that was open last time.
        sk_core::xmp::set_write_enabled(settings.write_xmp && opts.persist_settings);
        sk_ai::runtime::set_gpu_enabled(settings.ai_gpu.unwrap_or(true));
        sk_ai::denoise::register();
        sk_ai::upscale::register();
        sk_ai::depth::register();
        sk_core::gpu::set_enabled(settings.develop_gpu.unwrap_or(true));
        // Sets up the graphics card and compiles the shaders while the window opens.
        std::thread::spawn(sk_core::pipeline::prepare_gpu);
        // Keeps the per-user preview cache within its size limit (not in tests: it is the user's cache).
        if opts.persist_settings {
            std::thread::spawn(|| {
                let removed = sk_core::cache::trim_user_cache(sk_core::cache::user_cache_limit());
                if removed > 0 {
                    log::info!("Vorschau-Cache: {} MB alter Vorschauen entfernt", removed >> 20);
                }
            });
        }
        let session = opts.session.or_else(|| settings.last_session.clone().filter(|p| p.exists() && opts.start_folder.is_none()));
        let folder = opts
            .start_folder
            .or(settings.last_folder.filter(|p| p.is_dir()))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let catalog_path = opts.catalog_path;
        let (catalog, status) = match Catalog::open(&catalog_path) {
            Ok(c) => (Some(c), settings_error.unwrap_or_default()),
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
            film_looks: sk_core::styles::film_simulations(),
            style_preview: None,
            style_hovered: false,
            lens_db: None,
            lens_db_downloading: false,
            lens_match: None,
            scope: Scope::All,
            filter: Filter::default(),
            sort: settings.sort,
            sort_desc: settings.sort_desc,
            show_navigator: !settings.hide_navigator,
            side_column: settings.side_column,
            side_column_shown: false,
            viewer_rect: None,
            view_geometry: None,
            drag_draft_level: None,
            drag_view_level: None,
            drag_rendered: false,
            guides: settings.guides,
            overlay: crate::guides::Overlay::new(settings.overlay_file.clone(), settings.overlay_opacity),
            folder: PathBuf::new(),
            catalog_folder: folder.clone(),
            folder_input: String::new(),
            subdirs: Vec::new(),
            entries: Vec::new(),
            visible: Vec::new(),
            selection: HashSet::new(),
            expanded_stacks: HashSet::new(),
            stack_sizes: HashMap::new(),
            auto_stack: None,
            thumbs: HashMap::new(),
            thumb_seen: HashMap::new(),
            thumb_requested: HashSet::new(),
            grid_view: false,
            index_progress: None,
            proxy_progress: None,
            current: None,
            loading: None,
            load_error: None,
            generation: 0,
            texture: None,
            texture_version: 0,
            texture_draft: false,
            view_size: None,
            view_magnified: false,
            placeholder: None,
            prepared: Vec::new(),
            relocations: 0,
            prefetched_for: 0,
            step_dir: 1,
            full_due: None,
            histogram: None,
            tab: Tab::Exposure,
            tab_shown: Tab::Exposure,
            curve_channel: CurveChannel::Rgb,
            color_band: 0,
            color_view: Default::default(),
            picking_wb: false,
            picking_skin: false,
            picking_focus: false,
            crop_tool: None,
            straighten_tool: None,
            local_tool: None,
            local_drag: Default::default(),
            active_layer: None,
            renaming_layer: None,
            selected_gradient: None,
            brushes: settings.brushes.clone(),
            brushes_changed_at: None,
            remove_overlay: None,
            auto_guide: None,
            selected_spot: None,
            show_mask: false,
            rendered_mask_overlay: None,
            split_view: false,
            split_side: false,
            split_pos: 0.5,
            split_dragging: false,
            speed_message: None,
            zoom_blocked_until: 0.0,
            before_texture: None,
            before_key: None,
            magic_tolerance: 0.3,
            object_session: None,
            spot_search: None,
            spot_sensitivity: 0.5,
            remove_strokes: Vec::new(),
            inpaint_busy: false,
            removal_progress: 0.0,
            show_gpu_dialog: false,
            ai_busy: false,
            tiles: Tiles::default(),
            tiles_requested: None,
            edited_at: Instant::now(),
            edit_version: 0,
            pointer_down: false,
            clip_warning: false,
            show_before: false,
            render_requested: false,
            undo: Vec::new(),
            redo: Vec::new(),
            clipboard: None,
            show_filmstrip: true,
            grid_thumb: settings.grid_thumb.unwrap_or(190.0).clamp(GRID_THUMB.0, GRID_THUMB.1),
            grid_cols: 1,
            palette: Default::default(),
            fullscreen: false,
            fullscreen_hint_until: None,
            restore: None,
            pending_maximize: None,
            native_window: None,
            animations_off: false,
            screen_transition: None,
            fade_in_from: None,
            proof_view: false,
            proof: None,
            proof_requested: None,
            show_shortcuts: false,
            // Debug screenshots should show the app, not the start screen.
            splash: std::env::var_os("SILBERKORN_SCREENSHOT").is_none().then(crate::splash::Splash::startup),
            window_title: "Silberkorn".into(),
            zoom_pct: None,
            pixels_per_point: 1.0,
            // egui's assumption until the viewer is first shown.
            max_texture_side: 2048,
            sidecar_dirty: false,
            unsaved: HashMap::new(),
            keyword_input: String::new(),
            zoom: None,
            pan: Vec2::ZERO,
            scroll_to_primary: false,
            filmstrip_shown: false,
            compare_ref: None,
            multi_view: true,
            compare_textures: HashMap::new(),
            compare_requested: HashSet::new(),
            compare_details: HashMap::new(),
            details_requested: Vec::new(),
            compare_zoom: 1.0,
            compare_center: Vec2::splat(0.5),
            match_progress: None,
            confirm_delete_variant: None,
            rename: None,
            restore_image: settings.last_image.clone(),
            snapshot_rename: None,
            debug_loupe: std::env::var("SILBERKORN_LOUPE").ok().and_then(|v| {
                let (x, y) = v.split_once(',')?;
                Some(Vec2::new(x.parse().ok()?, y.parse().ok()?))
            }),
            rename_template: settings.rename_template.clone(),
            focus_mask: false,
            focus_sensitivity: settings.focus_sensitivity.unwrap_or(50.0).clamp(0.0, 100.0),
            focus_maps: HashMap::new(),
            focus_pending: HashSet::new(),
            focus_render_for: None,
            recipes: if !settings.recipes.is_empty() {
                settings.recipes
            } else {
                vec![settings.recipe.unwrap_or_default()]
            },
            selected_recipe: 0,
            export_progress: None,
            export_step: None,
            export_recipes: 1,
            export_cancel: None,
            quit_asked: false,
            quit_confirmed: false,
            export_log: Vec::new(),
            status,
            screenshot: std::env::var_os("SILBERKORN_SCREENSHOT").map(|p| (PathBuf::from(p), None)),
            viewer_window: crate::viewer_window::ViewerWindow {
                open: (settings.viewer_window.open && opts.persist_settings) || std::env::var_os("SILBERKORN_VIEWER_WINDOW").is_some(),
                ..settings.viewer_window
            },
            viewer_window_shown: false,
            viewer_pointer_down: false,
            viewer_window_moved: None,
            softproof: crate::softproof::SoftProofState { settings: settings.softproof.clone(), ..Default::default() },
            print: crate::print_preview::PrintPreviewState { settings: settings.print.clone(), ..Default::default() },
            jobs: Vec::new(),
            select_when_listed: None,
            merge_dialog: None,
            hdr_options: settings.hdr_options,
            pano_options: settings.pano_options,
            ai_downloads: Default::default(),
            quiet_jobs: Vec::new(),
            denoise: Default::default(),
            depth: Default::default(),
        };
        app.reload_styles();
        if opts.persist_settings {
            app.workers.load_lens_db();
            app.preload_small_ai();
        }
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
            Ok("about") => app.splash = Some(crate::splash::Splash::about()),
            Ok("gpu") => app.show_gpu_dialog = true,
            // `calibrate` or `calibrate:<dpi>`.
            Ok(d) if d.starts_with("calibrate") => {
                app.print.calibrating = Some(d.trim_start_matches("calibrate").trim_start_matches(':').parse().unwrap_or(96.0) / 25.4);
            }
            Ok(d) if d.starts_with("palette") => app.palette.show_with(d.trim_start_matches("palette").trim_start_matches(':')),
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
            hide_navigator: !self.show_navigator,
            side_column: self.side_column,
            focus_sensitivity: Some(self.focus_sensitivity),
            guides: self.guides,
            overlay_file: self.overlay.file.clone(),
            overlay_opacity: Some(self.overlay.opacity),
            rename_template: self.rename_template.clone(),
            last_image: self.primary_path().or_else(|| self.restore_image.clone()),
            write_xmp: sk_core::xmp::write_enabled(),
            viewer_window: self.viewer_window,
            softproof: self.softproof.settings.clone(),
            print: self.print.settings.clone(),
            hdr_options: self.hdr_options,
            pano_options: self.pano_options,
            ai_gpu: Some(sk_ai::runtime::gpu_enabled()),
            develop_gpu: Some(sk_core::gpu::enabled()),
            brushes: self.brushes.clone(),
            grid_thumb: Some(self.grid_thumb),
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(&s) {
            if let Err(e) = sk_core::safe_file::write(&path, json) {
                log::warn!("{e:#}");
            }
        }
    }

    // ---------------------------------------------------------------- selection

    /// U: the divider view on or off (from side by side it switches to the divider).
    pub fn toggle_split(&mut self) {
        self.set_split((!self.split_view || self.split_side).then_some(false));
    }

    /// Umschalt+U: before and after side by side on or off.
    pub fn toggle_side_by_side(&mut self) {
        self.set_split((!self.split_view || !self.split_side).then_some(true));
    }

    /// `None`: off, `Some(side)`: divider or side by side.
    fn set_split(&mut self, mode: Option<bool>) {
        self.split_side = mode.unwrap_or(self.split_side);
        if self.split_view == mode.is_some() {
            return;
        }
        self.split_view = mode.is_some();
        self.split_dragging = false;
        self.show_before = false;
        self.before_key = None;
        self.request_render();
    }

    /// Layer whose mask is shown as overlay: only while "Maske anzeigen" (M) is on.
    pub fn mask_overlay(&self) -> Option<usize> {
        if self.show_mask { self.active_layer } else { None }
    }

    /// Size of the shown image in the viewer's coordinates (preview pixels).
    pub fn image_size(&self) -> Option<Vec2> {
        self.view_size.or_else(|| self.texture.as_ref().map(|t| t.size_vec2()))
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
        self.placeholder = None;
        self.histogram = None;
        // Paths may have changed (moved images, other folder).
        self.prepared.clear();
        self.workers.cancel_tiles();
        self.full_due = None;
        self.generation += 1;
    }

    /// Creates missing viewer previews in the background, starting at the current image.
    pub(crate) fn request_proxies(&self) {
        let order: Vec<PathBuf> = self.visible.iter().map(|&i| self.entries[i].path.clone()).collect();
        let start = self.primary_path().and_then(|p| order.iter().position(|q| *q == p)).unwrap_or(0);
        let mut paths = order[start..].to_vec();
        paths.extend_from_slice(&order[..start]);
        self.workers.proxies(paths, self.session_root());
    }

    fn session_root(&self) -> Option<PathBuf> {
        self.session.as_ref().map(|s| s.root.clone())
    }

    /// Keeps the current image in memory (with its rendering, if up to date), so that coming
    /// back to it is instant.
    fn stash_current(&mut self) {
        let Some(cur) = self.current.take() else { return };
        let plain = self.texture_version == self.edit_version
            && !self.texture_draft
            && self.style_preview.is_none()
            && !self.clip_warning
            && !self.show_before
            && self.crop_tool.is_none()
            && !self.softproof.on
            && self.mask_overlay().is_none();
        let texture = self.texture.take();
        let rendered = texture.filter(|_| plain).zip(self.histogram.clone());
        // Its focus mask stays only if it belongs to this rendering.
        if self.focus_maps.get(&cur.meta.path).is_some_and(|m| !plain || m.version != Some(self.texture_version)) {
            self.focus_maps.remove(&cur.meta.path);
        }
        // The compare view shows this image as it is now.
        if self.compare_details.get(&cur.meta.path).is_some_and(|d| d.version != Some(self.edit_version)) {
            self.compare_details.remove(&cur.meta.path);
        }
        match &rendered {
            Some((t, _)) => {
                self.compare_textures.insert(cur.meta.path.clone(), t.clone());
            }
            None => {
                self.compare_textures.remove(&cur.meta.path);
                self.compare_requested.remove(&cur.meta.path);
            }
        }
        self.remember(Ready { meta: cur.meta, preview: cur.preview, adjustments: cur.committed, rendered });
    }

    /// Drops compare renderings and focus maps of images that are neither shown nor kept in
    /// memory, so that browsing through many images doesn't pile up textures.
    pub(crate) fn prune_view_caches(&mut self) {
        const LIMIT: usize = crate::compare::MAX_CELLS + READY_MAX + 4;
        if self.compare_textures.len() <= LIMIT && self.focus_maps.len() <= LIMIT && self.compare_details.len() <= LIMIT {
            return;
        }
        let mut keep: HashSet<PathBuf> = self.prepared.iter().map(|r| r.meta.path.clone()).collect();
        keep.extend(self.compare_ref.iter().cloned());
        keep.extend(self.primary_path());
        keep.extend(self.selected_paths().into_iter().take(crate::compare::MAX_CELLS));
        self.compare_textures.retain(|p, _| keep.contains(p));
        self.focus_maps.retain(|p, _| keep.contains(p));
        self.compare_details.retain(|p, _| keep.contains(p));
    }

    fn remember(&mut self, ready: Ready) {
        self.prepared.retain(|r| r.meta.path != ready.meta.path);
        self.prepared.push(ready);
        if self.prepared.len() > READY_MAX {
            self.prepared.remove(0);
        }
    }

    /// Shows a loaded image. `rendered` is used if it was made with the saved adjustments.
    fn install(&mut self, meta: Arc<RawMeta>, preview: Arc<LinearImage>, rendered: Option<(Adjustments, TextureHandle, Histogram)>) {
        let (sc, unreadable) = match sidecar::try_load(&meta.path) {
            Ok(sc) => (sc, None),
            Err(e) => {
                let e = format!("{e:#}");
                self.status = format!("Einstellungen nicht lesbar, Änderungen werden nicht gespeichert: {e}");
                (sidecar::load(&meta.path), Some(e))
            }
        };
        // Settings that couldn't be saved win over the file (and are tried again).
        if self.unsaved.contains_key(&meta.path) {
            self.sidecar_dirty = true;
        }
        let mut sc = self.with_unsaved(&meta.path, sc);
        // Rounded to slider precision so that displaying it never counts as an edit.
        let (k, t) = meta.color.temp_tint_for_wb(meta.as_shot_wb);
        let as_shot_temp_tint = (k.round(), (t * 10.0).round() / 10.0);
        let rendered = rendered.filter(|(adj, ..)| *adj == sc.adjustments);
        // Layer ids for everything that can be restored, so that the active layer follows undo.
        sc.adjustments.assign_layer_ids();
        for s in &mut sc.snapshots {
            s.adjustments.assign_layer_ids();
        }
        self.loading = None;
        self.full_due = Some(Instant::now() + FULL_DELAY);
        self.tiles = Tiles::default();
        self.tiles_requested = None;
        self.workers.cancel_tiles();
        self.proof = None;
        self.proof_requested = None;
        self.crop_tool = None;
        self.straighten_tool = None;
        self.active_layer = None;
        self.selected_spot = None;
        self.before_texture = None;
        self.before_key = None;
        self.local_drag = Default::default();
        self.snapshot_rename = None;
        // Areas painted for removal belong to the previous image.
        self.remove_strokes.clear();
        self.remove_overlay = None;
        self.current = Some(Current {
            meta,
            preview,
            full: None,
            adjustments: sc.adjustments.clone(),
            committed: sc.adjustments,
            rating: sc.rating,
            color_label: sc.color_label,
            keywords: sc.keywords,
            snapshots: sc.snapshots,
            as_shot_temp_tint,
            unreadable,
        });
        // The prefetched rendering lacks the profile.
        let rendered = rendered.filter(|_| !self.auto_lens_profile());
        self.undo.clear();
        self.redo.clear();
        self.zoom = None;
        self.pan = Vec2::ZERO;
        match rendered {
            Some((_, texture, histogram)) if !self.clip_warning && !self.show_before && !self.split_view => {
                self.view_size = Some(texture.size_vec2());
                self.texture_draft = false;
                self.texture = Some(texture);
                self.texture_version = self.edit_version;
                self.histogram = Some(histogram);
                self.placeholder = None;
                self.prefetch_neighbours();
            }
            other => {
                // Keep something on screen until the new rendering arrives.
                if let Some((_, texture, _)) = other {
                    self.placeholder = Some(texture);
                }
                self.texture = None;
                self.view_size = None;
                self.request_render();
            }
        }
    }

    /// Loads and renders the images next to the current one in the background, in the
    /// direction the user is moving.
    fn prefetch_neighbours(&mut self) {
        if self.prefetched_for == self.generation {
            return;
        }
        self.prefetched_for = self.generation;
        let Some(path) = self.current_path() else { return };
        let Some(pos) = self.visible.iter().position(|&i| self.entries[i].path == path) else { return };
        let d = if self.step_dir < 0 { -1 } else { 1 };
        let paths: Vec<PathBuf> = [d, 2 * d, -d]
            .into_iter()
            .filter_map(|off| self.visible.get(pos.checked_add_signed(off)?))
            .map(|&i| self.entries[i].path.clone())
            .filter(|p| !self.prepared.iter().any(|r| r.meta.path == *p && r.rendered.is_some()))
            .collect();
        self.workers.prefetch(paths, self.session_root(), self.relocations);
    }

    /// Starts decoding the full resolution once its time has come (see [`FULL_DELAY`]).
    fn load_full_when_due(&mut self, ctx: &egui::Context) {
        let Some(due) = self.full_due else { return };
        let now = Instant::now();
        if now < due {
            ctx.request_repaint_after(due - now);
            return;
        }
        self.full_due = None;
        if let Some(cur) = &self.current {
            self.workers.load_full(self.generation, cur.meta.path.clone());
        }
    }

    /// Rendering of a recently viewed or prefetched image, if still valid.
    /// Edit version of the rendering on screen.
    pub(crate) fn texture_version_shown(&self) -> u64 {
        self.texture_version
    }

    /// Forgets an image kept in memory (its settings were changed elsewhere).
    pub(crate) fn forget_prepared(&mut self, path: &std::path::Path) {
        self.prepared.retain(|r| r.meta.path != path);
    }

    pub(crate) fn prepared_texture(&self, path: &std::path::Path) -> Option<&TextureHandle> {
        self.prepared.iter().find(|r| r.meta.path == path).and_then(|r| r.rendered.as_ref()).map(|(t, _)| t)
    }

    #[cfg(test)]
    pub(crate) fn texture_version_for_test(&self) -> u64 {
        self.texture_version
    }

    /// Is this image in memory, ready to be shown with its rendering?
    #[cfg(test)]
    pub(crate) fn is_prepared(&self, path: &std::path::Path) -> bool {
        self.prepared.iter().any(|r| r.meta.path == path && r.rendered.is_some())
    }

    pub fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        use egui::ViewportCommand as Cmd;
        self.fullscreen = !self.fullscreen;
        // On Windows a window that is still maximized keeps its client area to the work area
        // (screen minus taskbar), which left a black band at the bottom: leave maximized first,
        // where possible straight to the monitor size (no visible detour via the normal size).
        // `Decorations(false)` makes egui-winit drop the drop shadow of the undecorated window in
        // full screen (its 1 px frame offset) and restore it afterwards.
        if self.fullscreen {
            self.restore = None;
            if ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
                self.window_animations(false);
                self.restore = Some(match self.native_window.and_then(crate::window_placement::leave_maximized_to_monitor) {
                    Some(normal) => Restore::Placement(normal),
                    None => {
                        ctx.send_viewport_cmd(Cmd::Maximized(false));
                        Restore::Maximize
                    }
                });
            }
            ctx.send_viewport_cmd(Cmd::Fullscreen(true));
            ctx.send_viewport_cmd(Cmd::Decorations(false));
        } else {
            ctx.send_viewport_cmd(Cmd::Fullscreen(false));
            ctx.send_viewport_cmd(Cmd::Decorations(false));
            match self.restore.take() {
                Some(Restore::Maximize) => ctx.send_viewport_cmd(Cmd::Maximized(true)),
                Some(Restore::Placement(normal)) => self.pending_maximize = Some(normal),
                None => {}
            }
        }
        self.fullscreen_hint_until = self.fullscreen.then(|| Instant::now() + Duration::from_millis(2500));
        let initial = ctx.input(|i| i.viewport_rect());
        self.screen_transition = Some(ScreenTransition { started: Instant::now(), initial, last: None, stable_frames: 0 });
        self.fade_in_from = None;
    }

    /// Windows: maximizes again once winit has left full screen (doing it earlier would be
    /// undone by winit restoring the window from full screen).
    fn finish_leaving_fullscreen(&mut self, ctx: &egui::Context) {
        let Some(normal) = self.pending_maximize else { return };
        if ctx.input(|i| i.viewport().fullscreen.unwrap_or(false)) {
            return;
        }
        self.pending_maximize = None;
        if let Some(hwnd) = self.native_window {
            // Without the maximize animation (a zoom from the full screen size).
            self.window_animations(false);
            crate::window_placement::maximize_again(hwnd, normal);
        }
    }

    fn window_animations(&mut self, enabled: bool) {
        if let Some(hwnd) = self.native_window {
            crate::window_placement::set_animations(hwnd, enabled);
            self.animations_off = !enabled;
        }
    }

    /// Animations back on once a full screen switch has completely settled.
    fn restore_window_animations(&mut self) {
        if self.animations_off && self.screen_transition.is_none() && self.pending_maximize.is_none() {
            self.window_animations(true);
        }
    }

    /// While the window switches to or from full screen: true until it has settled (the
    /// window manager reports the new state, the size has changed and then stays the same for
    /// a few frames).
    pub(crate) fn screen_switching(&mut self, ctx: &egui::Context) -> bool {
        let Some(t) = &mut self.screen_transition else { return false };
        let (reported, rect) = ctx.input(|i| (i.viewport().fullscreen.unwrap_or(false), i.viewport_rect()));
        if t.last == Some(rect) {
            t.stable_frames += 1;
        } else {
            (t.last, t.stable_frames) = (Some(rect), 0);
        }
        let settled = reported == self.fullscreen && rect != t.initial && t.stable_frames >= SETTLED_FRAMES;
        if settled || t.started.elapsed() > TRANSITION_MAX {
            self.screen_transition = None;
            self.fade_in_from = Some(Instant::now());
            return false;
        }
        ctx.request_repaint();
        true
    }

    /// Fades the freshly laid out window in from black after a full screen switch.
    fn fade_in(&mut self, ctx: &egui::Context) {
        let Some(from) = self.fade_in_from else { return };
        let t = from.elapsed().as_secs_f32() / FADE_IN.as_secs_f32();
        if t >= 1.0 {
            self.fade_in_from = None;
            return;
        }
        let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("fade in"));
        ctx.layer_painter(layer).rect_filled(ctx.content_rect(), 0.0, egui::Color32::from_black_alpha(((1.0 - t) * 255.0) as u8));
        ctx.request_repaint();
    }

    pub fn toggle_proof(&mut self) {
        self.proof_view = !self.proof_view;
    }

    /// Identifies the settings of the selected recipe (a changed recipe needs a new proof).
    pub(crate) fn proof_key(&self) -> Option<u64> {
        let recipe = self.recipes.get(self.selected_recipe)?;
        // A logo replaced under the same name changes the proof as well.
        let stamp = recipe.watermark.file_stamp();
        let gamut = self.softproof.settings.gamut_warning;
        Some(sk_core::cache::fnv1a(format!("{}|{stamp:?}|{gamut}", serde_json::to_string(recipe).ok()?).as_bytes()))
    }

    /// Requests a proof of the current state when the proof view is on.
    pub(crate) fn update_proof(&mut self, ctx: &egui::Context) {
        if !self.proof_view || self.pointer_down || self.style_preview.is_some() {
            return;
        }
        let Some(cur) = &self.current else { return };
        let Some(full) = cur.full.clone() else {
            self.want_full_now();
            return;
        };
        let (Some(key), Some(recipe)) = (self.proof_key(), self.recipes.get(self.selected_recipe)) else { return };
        let want = (self.generation, self.edit_version, key);
        if self.proof_requested == Some(want) {
            return;
        }
        self.proof_requested = Some(want);
        self.workers.proof(crate::workers::ProofJob {
            generation: self.generation,
            version: self.edit_version,
            key,
            meta: cur.meta.clone(),
            full,
            adjustments: cur.adjustments.clone(),
            recipe: recipe.clone(),
            gamut_warning: self.softproof.settings.gamut_warning,
            max_texture_side: ctx.input(|i| i.max_texture_side),
        });
    }

    /// Shows the current image with these adjustments while a style is hovered.
    pub(crate) fn preview_style(&mut self, adjustments: Adjustments) {
        self.style_hovered = true;
        if self.style_preview.as_ref() != Some(&adjustments) {
            self.style_preview = Some(adjustments);
            self.request_render();
        }
    }

    /// Requests the full resolution right away (the user zooms in).
    pub(crate) fn want_full_now(&mut self) {
        if let Some(due) = &mut self.full_due {
            *due = Instant::now();
        }
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
        self.make_primary(path);
    }

    /// Shows this image in the viewer (for editing) without changing the selection.
    pub fn make_primary(&mut self, path: &PathBuf) {
        if self.primary_path().as_ref() != Some(path) {
            self.workers.touch();
            self.commit_and_save();
            self.stash_current();
            self.workers.cancel_tiles();
            self.generation += 1;
            self.load_error = None;
            self.picking_wb = false;
            self.picking_skin = false;
            self.picking_focus = false;
            self.full_due = None;
            match self.prepared.iter().position(|r| r.meta.path == *path).map(|i| self.prepared.remove(i)) {
                Some(r) => {
                    let rendered = r.rendered.map(|(t, h)| (r.adjustments, t, h));
                    self.install(r.meta, r.preview, rendered);
                }
                None => {
                    // Show the thumbnail right away, then the cached preview, then the real thing.
                    self.placeholder = self.thumbs.get(path).cloned();
                    self.texture = None;
                    self.loading = Some(path.clone());
                    self.workers.quick(self.generation, path.clone(), self.session_root());
                    self.workers.load(self.generation, path.clone());
                }
            }
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
        self.step_dir = delta.signum();
        self.scroll_to_primary = true;
        self.select_path(&path, Modifiers::NONE);
    }

    /// One row up or down in the grid. Above the first row nothing happens; below the last full
    /// column the last image of the next row is taken, as in file managers.
    pub fn step_row(&mut self, dir: isize) {
        if self.visible.is_empty() {
            return;
        }
        let cols = self.grid_cols.max(1) as isize;
        let primary = self.primary_path();
        let pos = self.visible.iter().position(|&i| Some(&self.entries[i].path) == primary.as_ref()).unwrap_or(0) as isize;
        let last = self.visible.len() as isize - 1;
        let target = pos + dir * cols;
        let target = if target > last && last / cols > pos / cols { last } else { target };
        if (0..=last).contains(&target) && target != pos {
            self.step(target - pos);
        }
    }

    // ---------------------------------------------------------------- editing

    pub fn request_render(&mut self) {
        self.render_requested = true;
        self.edit_version += 1;
        self.edited_at = Instant::now();
    }

    /// Pushes finished edits to the undo stack and writes the sidecar.
    pub fn commit_and_save(&mut self) {
        let Some(cur) = self.current.as_mut() else { return };
        // New and pasted layers.
        cur.adjustments.assign_layer_ids();
        if cur.adjustments != cur.committed {
            self.undo.push(std::mem::replace(&mut cur.committed, cur.adjustments.clone()));
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
            self.redo.clear();
            self.sidecar_dirty = true;
        }
        if self.sidecar_dirty
            && let Some(e) = &cur.unreadable
        {
            self.sidecar_dirty = false;
            self.status = format!("Nicht gespeichert, die Einstellungen waren beim Öffnen nicht lesbar: {e}");
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
            let path = cur.meta.path.clone();
            self.save_sidecar(&path, &sc);
        }
    }

    /// `sc` with the settings of `path` that couldn't be saved yet: they win over the file.
    pub(crate) fn with_unsaved(&self, path: &Path, sc: Sidecar) -> Sidecar {
        match self.unsaved.get(path) {
            Some(u) => Sidecar { rating: u.rating, color_label: u.color_label, keywords: u.keywords.clone(), adjustments: u.adjustments.clone(), ..sc },
            None => sc,
        }
    }

    /// Settings of an image for changing them: an error if its sidecar can't be read (saving
    /// would replace it with defaults).
    pub(crate) fn sidecar_for_update(&self, path: &Path) -> anyhow::Result<Sidecar> {
        Ok(self.with_unsaved(path, sidecar::try_load(path)?))
    }

    /// Writes the sidecar of an image and syncs the catalog; settings that can't be saved are
    /// kept in `unsaved` (and tried again when the image is opened).
    pub(crate) fn save_sidecar(&mut self, path: &Path, sc: &Sidecar) {
        match sidecar::save(path, sc) {
            Ok(()) => {
                self.unsaved.remove(path);
            }
            Err(e) => {
                self.status = format!("Speichern fehlgeschlagen: {e:#}");
                self.unsaved.insert(path.to_path_buf(), sc.clone());
            }
        }
        if let Some(c) = &self.catalog {
            let _ = c.sync_sidecar(path, sc);
        }
    }

    pub(crate) fn undo(&mut self) {
        self.commit_and_save();
        let active = self.active_layer_id();
        let Some(cur) = self.current.as_mut() else { return };
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut cur.adjustments, prev.clone()));
            cur.committed = prev;
            self.sidecar_dirty = true;
            self.layers_replaced(active);
            self.request_render();
        }
    }

    pub(crate) fn redo(&mut self) {
        self.commit_and_save();
        let active = self.active_layer_id();
        let Some(cur) = self.current.as_mut() else { return };
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut cur.adjustments, next.clone()));
            cur.committed = next;
            self.sidecar_dirty = true;
            self.layers_replaced(active);
            self.request_render();
        }
    }

    /// Sets the adjustments to a snapshot's.
    pub(crate) fn apply_snapshot(&mut self, adj: Adjustments) {
        self.commit_and_save();
        let active = self.active_layer_id();
        self.edit(|a| *a = adj);
        self.commit_and_save();
        self.layers_replaced(active);
    }

    fn active_layer_id(&self) -> Option<u32> {
        let cur = self.current.as_ref()?;
        cur.adjustments.layers.get(self.active_layer?).map(|l| l.id)
    }

    /// After the layers were replaced as a whole (undo, redo, snapshot), indices no longer mean
    /// the same layer: the active layer is found again by its id, other selections end.
    fn layers_replaced(&mut self, active: Option<u32>) {
        self.active_layer = active.and_then(|id| self.layer_index(id));
        self.selected_gradient = None;
        self.selected_spot = None;
        self.object_session = None;
        // A drag still under way (undo with the mouse held) must not go on with an old index.
        self.local_drag = Default::default();
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

    /// Renders the selected images with the latest process version (see
    /// `sk_core::params::PROCESS_LATEST`); their look may change slightly.
    pub(crate) fn update_process_version(&mut self) {
        let latest = sk_core::params::PROCESS_LATEST;
        let targets = self.selected_paths();
        let current = self.current.as_ref().map(|c| (c.meta.path.clone(), c.adjustments.process));
        let old = targets
            .iter()
            .filter(|p| match &current {
                Some((c, process)) if c == *p => *process < latest,
                _ => sidecar::load(p).adjustments.process < latest,
            })
            .cloned()
            .collect::<Vec<_>>();
        self.status.clear();
        self.set_adjustments_on(&old, |a| Adjustments { process: latest, ..a.clone() });
        if self.status.is_empty() {
            self.status = match old.len() {
                0 => format!("Alle ausgewählten Bilder nutzen schon Prozessversion {latest}"),
                n => format!("{n} Bild(er) auf Prozessversion {latest} gehoben"),
            };
        }
    }

    /// Export progress 0..1 (outputs done, plus the part of the image being enlarged).
    pub fn export_fraction(&self) -> Option<f32> {
        let (done, total) = self.export_progress?;
        let step = self.export_step.unwrap_or(0.0) * self.export_recipes as f32;
        Some(((done as f32 + step) / total.max(1) as f32).min(1.0))
    }

    pub fn export_selection(&mut self) {
        // One export at a time: a second would take over the first's progress and cancel switch,
        // and neither could be cancelled any more.
        if self.export_progress.is_some() {
            self.status = "Es läuft bereits ein Export – bitte warten oder abbrechen".into();
            return;
        }
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
        self.status = format!("Exportiere {}…", crate::widgets::image_count(items.len()));
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
        if let Some(f) = recipes.iter().filter_map(|r| r.upscale).map(crate::ai_models::AiFeature::for_upscale).find(|f| !f.available()) {
            // Exports once the model is there, if the selection is still the same.
            self.export_progress = None;
            let selection = self.selected_paths();
            self.with_ai(f, move |app| {
                if app.selected_paths() == selection {
                    app.export_selection();
                }
            });
            self.status = format!("{}: KI-Modell wird geladen ({} MB, einmalig), danach wird exportiert …", f.label(), f.download_mb());
            return;
        }
        let session = self.session.as_ref().map(|s| s.name.clone()).unwrap_or_default();
        self.export_recipes = recipes.len();
        self.export_step = None;
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.export_cancel = Some(cancel.clone());
        self.workers.export(ExportJob { items, recipes, session, cancel });
    }

    /// Average camera RGB of a 7x7 spot; `image_pos` is in displayed preview pixels.
    fn sample_camera_rgb(&self, image_pos: Vec2) -> Option<[f32; 3]> {
        let cur = self.current.as_ref()?;
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
        (n > 0.0).then(|| acc.map(|v| v / n))
    }

    /// White balance from a neutral spot; `image_pos` is in displayed preview pixels.
    pub fn pick_white_balance(&mut self, image_pos: Vec2) {
        let Some(avg) = self.sample_camera_rgb(image_pos) else { return };
        let Some(cur) = self.current.as_mut() else { return };
        if avg.iter().any(|v| *v <= 1e-5) || avg.iter().any(|v| *v >= 0.98) {
            self.status = "Pipette: Stelle ist zu dunkel oder ausgebrannt".into();
            return;
        }
        let wb = [avg[1] / avg[0], 1.0, avg[1] / avg[2]];
        let (kelvin, tint) = cur.meta.color.temp_tint_for_wb(wb);
        cur.adjustments.white_balance = WhiteBalance { mode: WbMode::Custom, kelvin, tint };
        self.status = format!("Weißabgleich: {kelvin:.0} K, Tönung {tint:.1}");
        self.picking_wb = false;
        self.request_render();
    }

    /// Skin tone target from a spot of skin; `image_pos` is in displayed preview pixels.
    pub fn pick_skin_tone(&mut self, image_pos: Vec2) {
        let Some(avg) = self.sample_camera_rgb(image_pos) else { return };
        let Some(cur) = self.current.as_mut() else { return };
        // Where the color adjustments see it, before the skin tone tool itself.
        let mut adj = cur.adjustments.clone();
        adj.skin_tone = Default::default();
        let [l, a, b] = Processor::new(&cur.meta, &adj).color_input(avg);
        let chroma = (a * a + b * b).sqrt();
        if chroma < 0.01 {
            self.status = "Hautton: Stelle ist zu farblos".into();
            return;
        }
        let st = &mut cur.adjustments.skin_tone;
        st.hue = (b.atan2(a).to_degrees().rem_euclid(360.0) * 10.0).round() / 10.0;
        st.chroma = (chroma * 1000.0).round() / 1000.0;
        st.lightness = (l * 1000.0).round() / 1000.0;
        self.status = format!("Hautton: Farbton {:.0}°", st.hue);
        self.picking_skin = false;
        self.request_render();
    }

    // ---------------------------------------------------------------- events

    pub(crate) fn handle_events(&mut self, ctx: &egui::Context) {
        if let Some(panic) = crate::guard::take_panics().pop() {
            self.status = format!("{panic} (Details in panic.log)");
        }
        self.load_full_when_due(ctx);
        self.pump_denoise();
        self.pump_depth();
        self.poll_spot_search();
        // The rendered preview must fit into a single GPU texture.
        let max_side = ctx.input(|i| i.max_texture_side).max(512);
        let fit_texture = |preview: Arc<LinearImage>| {
            let long = preview.width.max(preview.height);
            if long > max_side { Arc::new((*preview).clone().box_downscale(long.div_ceil(max_side))) } else { preview }
        };
        while let Ok(ev) = self.workers.events.try_recv() {
            match ev {
                // `loading` is unset if a prefetched copy was shown in the meantime.
                Event::Loaded { generation, result } if generation == self.generation && self.loading.is_some() => match result {
                    Ok((meta, preview)) => self.install(meta, fit_texture(preview), None),
                    Err(e) => {
                        self.loading = None;
                        self.current = None;
                        self.texture = None;
                        self.placeholder = None;
                        self.load_error = Some(e);
                    }
                },
                Event::Loaded { .. } => {}
                Event::Quick { generation, image } => {
                    if generation == self.generation && self.texture.is_none() {
                        self.placeholder = Some(ctx.load_texture("quick", image, TextureOptions::LINEAR));
                    }
                }
                Event::Prepared(p) if p.relocations != self.relocations => {}
                Event::Prepared(p) => {
                    let p = *p;
                    let too_big = p.preview.width.max(p.preview.height) > max_side;
                    let preview = fit_texture(p.preview);
                    let rendered = (!too_big).then(|| (ctx.load_texture("prepared", p.image, TextureOptions::LINEAR), p.histogram));
                    if self.loading.as_ref() == Some(&p.meta.path) {
                        self.install(p.meta, preview, rendered.map(|(t, h)| (p.adjustments, t, h)));
                    } else if self.current_path().as_ref() != Some(&p.meta.path) {
                        self.remember(Ready { meta: p.meta, preview, adjustments: p.adjustments, rendered });
                    }
                }
                Event::Rendered { generation, version, draft, image, thumb, histogram, before } => {
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
                    let [image_w, image_h] = image.size;
                    // Also with the clip warning or a mask overlay: they hardly change the sharpness,
                    // and the framing (crop, rotation) must be current.
                    if !draft
                        && !before
                        && let Some(path) = self.current_path()
                    {
                        self.request_focus(&path, &image, Some(version));
                    }
                    match &mut self.texture {
                        Some(t) => t.set(image, TextureOptions::LINEAR),
                        None => self.texture = Some(ctx.load_texture("viewer", image, TextureOptions::LINEAR)),
                    }
                    self.texture_version = version;
                    self.texture_draft = draft;
                    let size = Vec2::new(image_w as f32, image_h as f32);
                    let draft_fits = self.view_size.is_some_and(|v| (v.x / v.y - size.x / size.y).abs() < 0.01);
                    if !draft {
                        self.view_size = Some(size);
                    } else if !draft_fits {
                        // Unknown size of the full rendering: estimate it from the preview.
                        let k = self.current.as_ref().map_or(2.0, |c| c.preview.width as f32 / size.x).round().max(1.0);
                        self.view_size = Some(size * k);
                    }
                    self.placeholder = None;
                    self.histogram = Some(histogram);
                    self.prefetch_neighbours();
                    if let (Some(thumb), Some(path)) = (thumb, self.current_path()) {
                        self.set_thumb(ctx, path, thumb);
                    }
                }
                Event::Thumbnail { path, image } => self.set_thumb(ctx, path, image),
                Event::ProxyProgress(p) => self.proxy_progress = p,
                Event::Proof { generation, version, key, result } => {
                    if generation != self.generation {
                        continue;
                    }
                    match result {
                        Ok(p) => {
                            let texture = ctx.load_texture("proof", p.image, TextureOptions::LINEAR);
                            self.proof = Some(ProofView { version, key, texture, width: p.width, height: p.height, file_size: p.file_size });
                        }
                        Err(e) => self.status = format!("Ausgabevorschau fehlgeschlagen: {e}"),
                    }
                }
                Event::FullLoaded { generation, image } => {
                    if generation == self.generation
                        && let Some(cur) = &mut self.current
                    {
                        cur.full = Some(image);
                    }
                }
                Event::TileRendered { generation, version, index, region, level, full_size, image, focus } => {
                    if generation != self.generation {
                        continue;
                    }
                    if level == 1 || self.tiles.full_size == (0, 0) {
                        self.tiles.full_size = full_size;
                    }
                    match index {
                        Some(index) => {
                            if version != self.edit_version {
                                continue;
                            }
                            if self.tiles.version != version {
                                self.tiles.version = version;
                                self.tiles.textures.clear();
                                self.tiles.focus.clear();
                            }
                            match focus {
                                Some(f) => {
                                    self.tiles.focus.insert(index, crate::focus::FocusMap::new(image.size, f));
                                }
                                None => {
                                    self.tiles.focus.remove(&index);
                                }
                            }
                            let texture = ctx.load_texture(format!("tile{index:?}"), image, TextureOptions::LINEAR);
                            self.tiles.textures.insert(index, texture);
                        }
                        // The newest view rendering is shown even if the next edit is already on its
                        // way: while dragging, versions change faster than they can be rendered.
                        None => match &mut self.tiles.view {
                            Some(v) if v.version > version => {}
                            Some(v) => {
                                v.focus = focus.map(|f| crate::focus::FocusMap::new(image.size, f));
                                v.texture.set(image, TextureOptions::LINEAR);
                                (v.region, v.level, v.version) = (region, level, version);
                            }
                            None => {
                                let focus = focus.map(|f| crate::focus::FocusMap::new(image.size, f));
                                let texture = ctx.load_texture("view tile", image, TextureOptions::LINEAR);
                                self.tiles.view = Some(ViewTile { region, level, version, texture, focus });
                            }
                        },
                    }
                }
                Event::ExportStep(p) => self.export_step = (p < 1.0).then_some(p),
                Event::ExportProgress { done, total, last } => {
                    self.export_progress = (done < total).then_some((done, total));
                    match last {
                        Ok(p) => self.export_log.push(format!("✔ {}", p.display())),
                        Err(e) => self.export_log.push(format!("✖ {e}")),
                    }
                    if done == total {
                        self.status = format!("Export abgeschlossen ({total})");
                        self.export_cancel = None;
                    }
                }
                Event::ExportCancelled { done, total } => {
                    self.export_progress = None;
                    self.export_step = None;
                    self.export_cancel = None;
                    self.status = format!("Export abgebrochen ({done} von {total} fertig)");
                }
                Event::AiMask { generation, layer, result } => {
                    self.ai_busy = false;
                    if generation != self.generation {
                        continue;
                    }
                    match result {
                        Ok(m) => {
                            // An object replaces its previous result (what the mask held before stays).
                            // The layer may have moved (others deleted) or be gone (undone).
                            let Some(index) = self.layer_index(layer) else { continue };
                            let base = self.object_session.as_ref().filter(|s| s.layer == layer).map(|s| s.base.clone());
                            self.edit(|a| {
                                if let Some(l) = a.layers.get_mut(index) {
                                    let old = match base {
                                        Some(b) => b,
                                        None => l.mask.ai.take(),
                                    };
                                    l.mask.ai = Some(merge_ai(old, m));
                                }
                            });
                            self.status = "Maske erstellt".into();
                        }
                        Err(e) if e == "OBJECT_MODEL_MISSING" => {
                            self.status = "Objekt auswählen: Das KI-Modell wird noch geladen …".into();
                        }
                        Err(e) if e == "MODEL_MISSING" => {
                            self.status = "Motiv und Himmel: Das KI-Modell wird noch geladen …".into();
                        }
                        Err(e) => self.status = format!("Maske fehlgeschlagen: {e}"),
                    }
                }
                Event::LensDb(db) => {
                    self.lens_db = Some(db);
                    self.lens_match = None;
                    if self.auto_lens_profile() {
                        self.request_render();
                    }
                }
                Event::LensDbDownloading(on) => self.lens_db_downloading = on,
                Event::CompareImage { path, image } => {
                    // The current image is shown from its live rendering.
                    if self.compare_requested.remove(&path) && self.current_path().as_ref() != Some(&path) {
                        self.request_focus(&path, &image, None);
                        match self.compare_textures.get_mut(&path) {
                            Some(t) => t.set(image, TextureOptions::LINEAR),
                            None => {
                                let t = ctx.load_texture(format!("compare:{}", path.display()), image, TextureOptions::LINEAR);
                                self.compare_textures.insert(path, t);
                            }
                        }
                    }
                }
                Event::CompareDetail { path, version, uv, width, full_res, image, focus } => {
                    self.detail_received(ctx, path, version, uv, width, full_res, image, focus);
                }
                Event::Matched { path, result } => self.matched(path, result),
                Event::Focus { path, version, size, sharpness } => self.focus_received(path, version, size, sharpness),
                Event::IndexProgress { done, total } => {
                    self.index_progress = (done < total).then_some((done, total));
                }
                Event::IndexDone { catalog, indexed, failed, import } => {
                    self.index_progress = None;
                    if catalog != self.catalog_path() {
                        continue;
                    }
                    if import {
                        self.status = format!("Import: {}{}", crate::widgets::image_count(indexed), if failed > 0 { format!(", {failed} Fehler") } else { String::new() });
                        self.open_scope(Scope::LastImport);
                    } else if indexed > 0 || failed > 0 {
                        self.reload_entries();
                    }
                    self.select_created();
                }
                Event::JobProgress { id, progress } => self.job_progress(id, progress),
                Event::Removal { generation, result } => self.removal_done(generation, result),
                Event::RemovalProgress { generation, progress } => {
                    if generation == self.generation {
                        self.removal_progress = progress;
                    }
                }
                Event::JobDone { id, label, result } => self.job_done(id, label, result),
            }
        }
    }

    pub(crate) fn set_thumb(&mut self, ctx: &egui::Context, path: PathBuf, image: egui::ColorImage) {
        match self.thumbs.get_mut(&path) {
            Some(t) => t.set(image, TextureOptions::LINEAR),
            // Previews made in the background announce every image: only those the browser has
            // shown are kept (the others come from the disk cache when they appear).
            None if !self.thumb_requested.contains(&path) => {}
            None => {
                let tex = ctx.load_texture(format!("thumb:{}", path.display()), image, TextureOptions::LINEAR);
                self.thumbs.insert(path, tex);
                self.trim_thumbs();
            }
        }
    }

    /// Drops the thumbnails not drawn for the longest time once there are more than
    /// [`MAX_THUMBS`] (up to 256 KB each): scrolling through a large catalog must not fill the memory.
    fn trim_thumbs(&mut self) {
        if self.thumbs.len() <= MAX_THUMBS {
            return;
        }
        let mut by_age: Vec<(u64, PathBuf)> = self.thumbs.keys().map(|p| (self.thumb_seen.get(p).copied().unwrap_or(0), p.clone())).collect();
        by_age.sort_unstable_by_key(|(seen, _)| *seen);
        for (_, p) in by_age.into_iter().take(MAX_THUMBS / 4) {
            self.thumbs.remove(&p);
            self.thumb_seen.remove(&p);
            self.thumb_requested.remove(&p);
        }
    }

    /// Thumbnails of images no longer listed (another folder or album) are dropped.
    pub(crate) fn forget_unlisted_thumbs(&mut self) {
        let listed: HashSet<&PathBuf> = self.entries.iter().map(|e| &e.path).collect();
        self.thumbs.retain(|p, _| listed.contains(p));
        self.thumb_seen.retain(|p, _| listed.contains(p));
        self.thumb_requested.retain(|p| listed.contains(p));
    }

    pub(crate) fn send_render(&mut self) {
        // The mask overlay comes and goes (M, or while painting a mask): preview and tiles
        // must be rendered again with or without it.
        let overlay = self.mask_overlay();
        if overlay != self.rendered_mask_overlay {
            self.rendered_mask_overlay = overlay;
            self.request_render();
        }
        // A draft was shown while dragging: render at full resolution once released (also
        // without reduction, so that the final state is cached).
        if (self.texture_draft || self.drag_rendered) && !self.pointer_down && !self.render_requested {
            self.texture_draft = false;
            self.drag_rendered = false;
            self.render_requested = true;
        }
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
                    version: self.edit_version,
                    meta: cur.meta.clone(),
                    preview: cur.preview.clone(),
                    adjustments: cur.adjustments.clone(),
                    clip_warning: false,
                    before: true,
                    ignore_crop: false,
                    mask_overlay: None,
                    cache_as: None,
                    draft: 1,
                    softproof: None,
                });
            }
        }
        let softproof = self.active_softproof();
        let draft = match (self.pointer_down, self.view_magnified) {
            (false, _) => 1,
            (true, false) => self.drag_draft_level(),
            // Zoomed in, the view draft covers the screen; this only feeds the histogram.
            (true, true) => 4,
        };
        self.drag_rendered |= self.pointer_down && self.current.is_some();
        if let Some(cur) = &self.current {
            self.workers.render(RenderJob {
                generation: self.generation,
                version: self.edit_version,
                // A style preview is shown, but never cached.
                adjustments: self.style_preview.clone().unwrap_or_else(|| cur.adjustments.clone()),
                draft,
                cache_as: (!self.pointer_down && self.style_preview.is_none()).then(|| (cur.meta.path.clone(), self.session_root())),
                meta: cur.meta.clone(),
                preview: cur.preview.clone(),
                clip_warning: self.clip_warning,
                before: self.show_before,
                ignore_crop: self.crop_tool.is_some(),
                mask_overlay: self.mask_overlay(),
                softproof,
            });
        }
    }

    pub(crate) fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::K)) {
            self.palette.toggle();
        }
        // While a menu, a dialog or the palette is open the keyboard belongs to it, as in native
        // applications (Ctrl+Entf must not trash the selection behind the rename dialog).
        if self.palette.open || ctx.egui_wants_keyboard_input() || egui::Popup::is_any_open(ctx) || ctx.memory(|m| m.top_modal_layer()).is_some() {
            return;
        }
        // Ctrl+1 … Ctrl+0: the tool tabs in rail order.
        for tab in Tab::ALL {
            if let Some(key) = tab.key()
                && ctx.input_mut(|inp| inp.consume_key(Modifiers::COMMAND, key))
            {
                self.tab = tab;
            }
        }
        // Everything else comes from the command list (see `commands`).
        self.command_shortcuts(ctx);
        let pressed = |k: Key, m: Modifiers| ctx.input_mut(|i| i.consume_key(m, k));
        if pressed(Key::Escape, Modifiers::NONE) {
            // Esc first ends a tool; with none active it leaves full screen.
            let tool_active = self.picking_wb || self.picking_skin || self.picking_focus || self.local_tool.is_some() || self.straighten_tool.is_some() || self.crop_tool.is_some();
            if !tool_active && self.fullscreen {
                self.toggle_fullscreen(ctx);
            }
            self.end_viewer_tools();
        }
    }

    // ---------------------------------------------------------------- layout

    /// Closing while work is running (export, merge, denoising …) asks first; quitting then
    /// cancels it.
    fn confirm_quit(&mut self, ctx: &egui::Context) {
        let mut busy: Vec<String> = self.activities().into_iter().filter(|a| a.cancel.is_some()).map(|a| a.label).collect();
        if !self.unsaved.is_empty() {
            busy.push(format!("Einstellungen von {} Bild(ern) konnten nicht gespeichert werden und gehen verloren", self.unsaved.len()));
        }
        if ctx.input(|i| i.viewport().close_requested()) && !busy.is_empty() && !self.quit_confirmed {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.quit_asked = true;
        }
        if !self.quit_asked {
            return;
        }
        if busy.is_empty() {
            self.quit_asked = false;
            return;
        }
        let modal = egui::Modal::new(egui::Id::new("quit")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(egui::RichText::new("Silberkorn beenden?").size(16.0).strong());
            ui.add_space(6.0);
            ui.label("Noch nicht fertig:");
            for b in &busy {
                ui.label(format!("  •  {b}"));
            }
            ui.add_space(4.0);
            ui.label("Beim Beenden wird abgebrochen; bereits fertige Dateien bleiben erhalten.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Abbrechen und beenden").clicked() {
                    return Some(true);
                }
                if ui.button("Weiterarbeiten").clicked() {
                    return Some(false);
                }
                None
            })
            .inner
        });
        match modal.inner {
            Some(true) => {
                for a in self.activities() {
                    if let Some(c) = a.cancel {
                        c.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                self.quit_confirmed = true;
                self.quit_asked = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Some(false) => self.quit_asked = false,
            None if modal.should_close() => self.quit_asked = false,
            None => {}
        }
    }

    /// End of every frame (also in full screen): pending actions, saving and rendering.
    fn finish_frame(&mut self, ctx: &egui::Context) {
        self.splash_ui(ctx);
        hover_cursor(ctx);
        self.run_pending();
        // The pointer left the styles: back to the real adjustments.
        if !self.style_hovered && self.style_preview.take().is_some() {
            self.request_render();
        }

        // Commit edits once the user lets go of the mouse (one undo step per gesture).
        if !ctx.input(|i| i.pointer.any_down()) && !self.viewer_pointer_down {
            self.commit_and_save();
        }
        self.sync_softproof();
        self.send_render();
        self.update_proof(ctx);
        self.fade_in(ctx);
        if self.brushes_changed_at.is_some_and(|t| t.elapsed() > Duration::from_secs(1)) {
            self.brushes_changed_at = None;
            self.save_settings();
        }
        if self.viewer_window_moved.is_some_and(|t| t.elapsed() > Duration::from_secs(1)) {
            self.viewer_window_moved = None;
            self.save_settings();
        }
        self.debug_screenshot(ctx);
    }

    fn debug_screenshot(&mut self, ctx: &egui::Context) {
        let Some((_, shown_at)) = &mut self.screenshot else { return };
        let now = ctx.input(|i| i.time);
        let empty = self.entries.is_empty() && self.loading.is_none();
        if (self.texture.is_some() || self.grid_view || empty) && shown_at.is_none() {
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
            if std::env::var_os("SILBERKORN_FOCUS").is_some() {
                self.toggle_focus_mask();
            }
            if std::env::var_os("SILBERKORN_VIEW100").is_some() {
                self.toggle_zoom_100();
            }
            if std::env::var_os("SILBERKORN_COMPARE").is_some() {
                self.select_all();
                self.toggle_reference();
                if std::env::var_os("SILBERKORN_ZOOM").is_some() {
                    (self.compare_zoom, self.compare_center) = (8.0, egui::vec2(0.45, 0.35));
                }
            }
            if std::env::var_os("SILBERKORN_SPLIT").is_some() {
                self.toggle_split();
            }
            if std::env::var_os("SILBERKORN_SIDE").is_some() {
                self.toggle_side_by_side();
            }
            if let Ok(template) = std::env::var("SILBERKORN_RENAME") {
                self.select_all();
                self.open_rename();
                if let Some(d) = &mut self.rename {
                    d.template = template;
                }
            }
            if std::env::var_os("SILBERKORN_UPRIGHT").is_some() {
                self.auto_upright(sk_core::upright::UprightMode::Vertical);
            }
            if std::env::var_os("SILBERKORN_GUIDES").is_some() {
                self.guides = crate::guides::Guides::Thirds;
            }
            if let Some(f) = std::env::var_os("SILBERKORN_OVERLAY") {
                (self.overlay.file, self.overlay.shown) = (Some(f.into()), true);
            }
            if let Some(text) = std::env::var("SILBERKORN_WATERMARK").ok()
                && let Some(r) = self.recipes.get_mut(self.selected_recipe)
            {
                r.watermark = sk_core::watermark::Watermark { enabled: true, text, size: 6.0, opacity: 0.8, ..Default::default() };
            }
            self.proof_view |= std::env::var_os("SILBERKORN_PROOF").is_some();
            // `contain` or `fill`: print preview on 13 × 18 cm.
            if let Ok(fit) = std::env::var("SILBERKORN_PRINT") {
                if !self.print.on {
                    self.toggle_print_preview();
                }
                self.print.settings.format = sk_core::print::PRESETS[2].1;
                self.print.settings.fit = if fit == "fill" { sk_core::print::Fit::Fill } else { sk_core::print::Fit::Contain };
            }
            // Brush rendering check: a curved remove stroke and a soft mask stroke.
            if std::env::var_os("SILBERKORN_BRUSHDEMO").is_some() {
                let curve = |y: f32, amp: f32| (0..60).map(|i| {
                    let t = i as f32 / 59.0;
                    [0.15 + 0.7 * t, y + amp * (t * 9.0).sin()]
                }).collect::<Vec<_>>();
                let stroke = |pts, r, hard, flow, op| sk_core::params::BrushStroke { points: pts, radius: r, hardness: hard, flow, erase: false, opacity: op, auto_mask: false, build_up: true };
                self.set_local_tool(crate::layers::LocalTool::Remove);
                self.remove_strokes = vec![stroke(curve(0.3, 0.08), 0.012, 0.8, 1.0, 1.0)];
                // Automatic masking: a big brush along the sky above the trees.
                if std::env::var_os("SILBERKORN_AUTOMASK").is_some() {
                    self.remove_strokes = vec![sk_core::params::BrushStroke { auto_mask: true, ..stroke(curve(0.12, 0.02), 0.06, 0.5, 1.0, 1.0) }];
                }
                self.edit(|a| a.layers.push(sk_core::params::Layer {
                    name: "Demo".into(),
                    mask: sk_core::params::Mask { strokes: vec![stroke(curve(0.7, 0.06), 0.03, 0.0, 0.3, 0.8)], ..Default::default() },
                    adjustments: sk_core::params::LayerAdjustments { exposure: 1.0, ..Default::default() },
                    ..Default::default()
                }));
                self.active_layer = Some(0);
                self.show_mask = true;
                self.request_render();
            }
            // Shows the activity display (as during a removal).
            if std::env::var_os("SILBERKORN_BUSY").is_some() {
                (self.inpaint_busy, self.removal_progress) = (true, 0.4);
            }
            if std::env::var_os("SILBERKORN_MERGE").is_some() {
                self.select_all();
                self.open_merge(crate::merge::MergeKind::Panorama);
            }
            if std::env::var_os("SILBERKORN_SOFTPROOF").is_some() {
                self.tab = Tab::Output;
                self.edit(|a| a.saturation = 60.0);
                self.toggle_softproof();
            }
            // Layout only (the window itself stays as it is).
            self.fullscreen |= std::env::var_os("SILBERKORN_FULLSCREEN").is_some();
        }
        // Wait for the sharp view region when zoomed in.
        if std::env::var_os("SILBERKORN_VIEW100").is_some() && !self.tiles.view.as_ref().is_some_and(|v| v.version == self.edit_version && v.level == 1) {
            if let Some((_, Some(t))) = &mut self.screenshot {
                *t = now;
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        // Wait for the proof (it needs the full resolution).
        if self.proof_view && !self.proof.as_ref().is_some_and(|p| p.version == self.edit_version) {
            if let Some((_, Some(t))) = &mut self.screenshot {
                *t = now;
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
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
    let mut data = new.data;
    if let Some(o) = old
        && o.width > 0
        && o.height > 0
    {
        for (i, v) in data.iter_mut().enumerate() {
            let (x, y) = (i % new.width, i / new.width);
            let ox = (x * o.width / new.width).min(o.width - 1);
            let oy = (y * o.height / new.height).min(o.height - 1);
            *v = (*v).max(o.data[oy * o.width + ox]);
        }
    }
    sk_core::params::AiMask { kind: "ai".into(), width: new.width, height: new.height, data: data.into(), depth: None }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.native_window = crate::window_placement::native_handle(frame);
        self.finish_leaving_fullscreen(&ctx);
        self.restore_window_animations();
        self.pixels_per_point = ctx.pixels_per_point();
        // Background previews wait while the user is doing something.
        let input = ctx.input(|i| i.pointer.any_down() || i.events.iter().any(|e| !matches!(e, egui::Event::PointerMoved(_) | egui::Event::MouseMoved(_))));
        if input || self.loading.is_some() || self.render_requested {
            self.workers.touch();
        }
        self.pointer_down = ctx.input(|i| i.pointer.any_down()) || self.viewer_pointer_down;
        if !self.pointer_down {
            (self.drag_draft_level, self.drag_view_level) = (None, None);
        }
        // The space bar pans the image: it must not also press the focused button again.
        if ctx.input(|i| i.key_pressed(egui::Key::Space))
            && !ctx.egui_wants_keyboard_input()
            && let Some(id) = ctx.memory(|m| m.focused())
        {
            ctx.memory_mut(|m| m.surrender_focus(id));
        }
        if !crate::browser::SPEED_KEYS.iter().any(|(k, _)| ctx.input(|i| i.key_down(*k))) {
            self.speed_message = None;
        }
        self.handle_events(&ctx);
        // Another tab: a layer or retouch tool of the old one ends (its panel is gone).
        if self.tab != self.tab_shown {
            self.tab_shown = self.tab;
            if let Some(t) = self.local_tool.filter(|t| t.tab() != self.tab) {
                self.set_local_tool(t);
            }
        }
        self.confirm_quit(&ctx);
        self.prune_view_caches();
        self.style_hovered = false;
        self.shortcuts(&ctx);
        crate::widgets::begin_wheel_gesture(&ctx);
        let title = self.session.as_ref().map_or_else(|| "Silberkorn".to_string(), |s| format!("{} – Silberkorn", s.name));
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }

        use crate::chrome::bar_margin;
        use crate::theme::{self, chrome_frame};
        // Full screen switch in progress: black only, no image at the in-between window sizes.
        if self.screen_switching(&ctx) {
            ui.painter().rect_filled(ctx.content_rect(), 0.0, egui::Color32::BLACK);
            return;
        }
        if self.fullscreen {
            // Distraction-free: only the image.
            egui::CentralPanel::no_frame().show(ui, |ui| self.viewer(ui));
            self.dialogs(&ctx);
            self.session_dialogs(&ctx);
            self.variant_dialogs(&ctx);
            self.stack_dialogs(&ctx);
            self.merge_dialogs(&ctx);
            self.print_dialogs(&ctx);
            self.finish_frame(&ctx);
            return;
        }
        egui::Panel::top("titlebar")
            .frame(chrome_frame(egui::Margin::ZERO))
            .exact_size(crate::chrome::TITLE_BAR_HEIGHT)
            .resizable(false)
            .show_separator_line(false)
            .show(ui, |ui| self.title_bar(ui));
        egui::Panel::top("toolbar").frame(chrome_frame(bar_margin())).show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status").frame(chrome_frame(egui::Margin::symmetric(10, 3))).show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("tabrail").frame(chrome_frame(egui::Margin::symmetric(2, 2))).resizable(false).exact_size(66.0).show(ui, |ui| self.tab_rail(ui));
        egui::Panel::left("tools")
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin { left: 8, right: 6, top: 0, bottom: 0 }))
            .resizable(true)
            .default_size(340.0)
            .size_range(280.0..=560.0)
            .show(ui, |ui| self.tool_panel(ui));
        let grid = self.grid_view || self.viewer_detached();
        self.side_column_shown = self.side_column_wanted(grid);
        if self.side_column_shown {
            egui::Panel::right("side_column")
                .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin { left: 8, right: 8, top: 0, bottom: 0 }))
                .resizable(false)
                .exact_size(crate::side_column::WIDTH)
                .show(ui, |ui| self.side_column(ui));
        }
        let filmstrip = !grid && self.show_filmstrip && !self.entries.is_empty();
        // Coming from the grid (or with the strip switched on) the strip starts at the image.
        if filmstrip && !self.filmstrip_shown {
            self.scroll_to_primary = true;
        }
        self.filmstrip_shown = filmstrip;
        if filmstrip {
            egui::Panel::bottom("filmstrip")
                .frame(egui::Frame::new().fill(theme::BROWSER).inner_margin(egui::Margin::same(6)))
                .resizable(true)
                .default_size(150.0)
                .min_size(100.0)
                .show(ui, |ui| self.filmstrip(ui));
        }
        egui::CentralPanel::no_frame().show(ui, |ui| if grid { self.grid(ui) } else { self.viewer(ui) });
        self.show_viewer_window(&ctx);
        crate::chrome::resize_edges(ui);
        self.dialogs(&ctx);
        self.palette_ui(&ctx);
        self.session_dialogs(&ctx);
        self.variant_dialogs(&ctx);
        self.stack_dialogs(&ctx);
        self.merge_dialogs(&ctx);
        self.print_dialogs(&ctx);
        self.finish_frame(&ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.commit_and_save();
        self.save_settings();
    }
}

/// Everything clickable shows the hand under the pointer (egui only does that for buttons, and
/// only when asked), unless a widget chose its own cursor. Widgets that are dragged (sliders,
/// the image, the curve) set theirs themselves.
pub(crate) fn hover_cursor(ctx: &egui::Context) {
    if ctx.output(|o| o.cursor_icon) != egui::CursorIcon::Default {
        return;
    }
    let (hit, hovered) = ctx.viewport(|v| (v.hits.click, v.interact_widgets.hovered.clone()));
    if let Some(w) = hit
        && w.enabled
        && w.sense.senses_click()
        && !w.sense.senses_drag()
        && hovered.contains(&w.id)
    {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}
