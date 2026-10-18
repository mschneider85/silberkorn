//! Background workers. The UI thread never decodes or processes images itself.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use sk_core::cache;
use sk_core::catalog::Catalog;
use sk_core::export::{self, ExportRecipe};
use sk_core::geometry::Region;
use sk_core::image::LinearImage;
use sk_core::params::Adjustments;
use sk_core::pipeline::{Histogram, Processor, RenderOptions, StageCache};
use sk_core::raw::{self, RawMeta};
use sk_core::session;
use sk_core::sidecar;
use egui::ColorImage;

/// Long edge of the interactive preview (before the half-size demosaic shortcut kicks in).
pub const PREVIEW_EDGE: usize = 2048;
pub const THUMB_EDGE: u32 = 256;
/// Edge of the full-resolution tiles (in output pixels).
pub const TILE: usize = 1024;

pub enum Event {
    Loaded { generation: u64, result: Result<(Arc<RawMeta>, Arc<LinearImage>), String> },
    /// `version` is the edit version the rendering was made for; `draft` marks a
    /// half-resolution rendering made while a control is being dragged.
    Rendered { generation: u64, version: u64, draft: bool, image: ColorImage, thumb: Option<ColorImage>, histogram: Histogram, before: bool },
    /// The part `x`, `y` of the preview rendering of `version` (see [`RenderJob::part`]).
    RenderedPart { generation: u64, version: u64, x: usize, y: usize, image: ColorImage },
    /// Stand-in shown immediately while the raw file loads: the cached preview, or the camera JPEG.
    Quick { generation: u64, image: ColorImage },
    /// A neighbouring image, loaded and rendered ahead of time.
    Prepared(Box<Prepared>),
    /// Background preview generation (`None` when finished).
    ProxyProgress(Option<(usize, usize)>),
    /// The current image as the selected recipe writes it (see [`ProofJob`]).
    Proof { generation: u64, version: u64, key: u64, result: Result<ProofImage, String> },
    Thumbnail { path: PathBuf, image: ColorImage },
    FullLoaded { generation: u64, image: Arc<LinearImage> },
    /// `index` is the grid position, or `None` for the view region. `region` is in full-resolution
    /// output pixels; the image has 1/`level` of its size (drafts while dragging).
    TileRendered {
        generation: u64,
        version: u64,
        index: Option<(usize, usize)>,
        region: Region,
        level: usize,
        full_size: (usize, usize),
        image: ColorImage,
        /// Sharpness map of `image` for the focus mask (full resolution only).
        focus: Option<Arc<Vec<u8>>>,
    },
    ExportProgress { done: usize, total: usize, last: Result<PathBuf, String> },
    /// The export was cancelled after `done` of `total` outputs.
    ExportCancelled { done: usize, total: usize },
    IndexProgress { done: usize, total: usize },
    /// The lensfun database was read (again after a download).
    LensDb(Arc<Vec<sk_core::lensfun::LensProfile>>),
    LensDbDownloading(bool),
    AiMask { generation: u64, layer: u32, result: Result<sk_ai::MaskImage, String> },
    /// An image of the compare view, rendered with its saved adjustments.
    CompareImage { path: PathBuf, image: ColorImage },
    /// A reference for "Look angleichen" is measured: its thumbnail, or why it can't be used.
    LookRef { r: crate::look::LookRef, result: Result<ColorImage, String> },
    /// How similar the open image is to each reference (`None`: not measured).
    LookSimilarity { path: PathBuf, refs: Vec<crate::look::LookRef>, values: Vec<Option<f32>> },
    /// The settings that give an image the look of the references.
    LookMatched { path: PathBuf, opts: sk_core::look_match::LookOptions, result: Result<crate::look_worker::LookResult, String> },
    /// The settings the auto adjustment chose for an image (see [`AutoJob`]).
    AutoAdjusted { path: PathBuf, opts: sk_core::auto::AutoOptions, result: Result<Box<Adjustments>, String> },
    /// Part of an image of the compare view at a higher resolution than its preview. `uv` is the
    /// part (0..1 of the output), `width` the whole output's width at this resolution.
    CompareDetail { path: PathBuf, version: Option<u64>, uv: [f32; 4], width: f32, full_res: bool, image: ColorImage, focus: Option<Arc<Vec<u8>>> },
    /// A style applied to the current image, for the style grid (see `style_grid`).
    StyleThumb { key: u64, id: String, image: ColorImage },
    /// Sharpness map of a displayed image for the focus mask.
    /// `version`: edit version of the rendering of the image being edited (`None` for others).
    Focus { path: PathBuf, version: Option<u64>, size: [usize; 2], sharpness: Arc<Vec<u8>> },
    /// Sharpness map of an image at thumbnail size, for the focus mask on the thumbnails.
    ThumbFocus { path: PathBuf, size: [usize; 2], sharpness: Arc<Vec<u8>> },
    /// Progress (0..1) of a generative removal.
    RemovalProgress { generation: u64, progress: f32 },
    /// Result of a generative removal (see `remove.rs`).
    Removal { generation: u64, result: Result<sk_core::params::Removal, String> },
    /// Progress (0..1) of a background job (see `jobs.rs`).
    JobProgress { id: u64, progress: f32 },
    /// A background job finished: the files it created, or why it failed.
    JobDone { id: u64, label: String, result: Result<Vec<PathBuf>, String> },
    /// `catalog` identifies the database, so that results for a closed session can be ignored.
    IndexDone { catalog: PathBuf, indexed: usize, failed: usize, import: bool },
}

/// Adds files to a catalog. `import` marks them as a new import ("last import").
pub struct IndexJob {
    /// Catalog or session database to write to.
    pub catalog: PathBuf,
    pub paths: Vec<PathBuf>,
    pub import: bool,
    /// Copy the files into this folder first and index the copies (session import).
    pub copy_to: Option<PathBuf>,
}

/// An image loaded and rendered with its saved adjustments, ready to show without waiting.
pub struct Prepared {
    /// [`App::relocations`](crate::app::App) when requested: a file renamed since may now go by this path.
    pub relocations: u64,
    pub meta: Arc<RawMeta>,
    pub preview: Arc<LinearImage>,
    pub adjustments: Adjustments,
    pub image: ColorImage,
    pub histogram: Histogram,
}

pub struct RenderJob {
    pub generation: u64,
    pub version: u64,
    pub meta: Arc<RawMeta>,
    pub preview: Arc<LinearImage>,
    pub adjustments: Adjustments,
    pub clip_warning: bool,
    pub before: bool,
    /// Render the whole frame (crop tool active).
    pub ignore_crop: bool,
    pub mask_overlay: Option<usize>,
    /// Image path and session root: store the result in the preview cache once editing pauses.
    pub cache_as: Option<(PathBuf, Option<PathBuf>)>,
    /// Reduction for fluid dragging (1 = full preview; 2 = a quarter of the pixels; 4 when
    /// zoomed in, where the view draft covers the screen and this only feeds the histogram).
    pub draft: usize,
    pub softproof: Option<Arc<sk_core::softproof::SoftProof>>,
    /// Only this part (output pixels of the preview's rendering) changed since the last job:
    /// rendered alone and sent as [`Event::RenderedPart`] (while a brush paints).
    pub part: Option<Region>,
}

/// The smallest region containing `a` and `b`.
fn union(a: Region, b: Region) -> Region {
    let (x, y) = (a.x.min(b.x), a.y.min(b.y));
    Region { x, y, w: (a.x + a.w).max(b.x + b.w) - x, h: (a.y + a.h).max(b.y + b.h) - y }
}

/// Full-resolution rendering for zoomed-in viewing: first the exact visible region (fastest
/// way to a sharp view), then the output in a grid of [`TILE`]-sized tiles that are kept.
pub struct TileRequest {
    pub generation: u64,
    pub version: u64,
    pub meta: Arc<RawMeta>,
    pub full: Arc<LinearImage>,
    pub adjustments: Adjustments,
    pub clip_warning: bool,
    /// Layer whose mask is tinted (as in the preview rendering).
    pub mask_overlay: Option<usize>,
    pub before: bool,
    /// Visible region (full-resolution output pixels) not yet covered by tiles, and the
    /// reduction to render it at (1 = full resolution; more for drafts while dragging).
    pub view: Option<(Region, usize)>,
    /// Grid positions to render, most important first.
    pub tiles: Vec<(usize, usize)>,
    /// Also analyse the full-resolution renderings for the focus mask.
    pub focus: bool,
    pub softproof: Option<Arc<sk_core::softproof::SoftProof>>,
}

/// Data, live adjustments and edit version of the image being edited.
pub type LiveImage = (Arc<RawMeta>, Option<Arc<LinearImage>>, Adjustments, u64);

/// A part of an image of the compare view, rendered from the full resolution when zoomed in.
pub struct DetailJob {
    pub path: PathBuf,
    /// The image being edited: its data and live adjustments (else they come from the sidecar),
    /// and its edit version.
    pub live: Option<LiveImage>,
    /// Part of the output, 0..1: x0, y0, x1, y1.
    pub uv: [f32; 4],
    /// Wanted width of the whole output in pixels (the full resolution at most).
    pub width: f32,
    pub focus: bool,
}

/// Renders the image exactly as an export recipe would (size, color space, format).
pub struct ProofJob {
    pub generation: u64,
    pub version: u64,
    /// Identifies the recipe settings.
    pub key: u64,
    pub meta: Arc<RawMeta>,
    pub full: Arc<LinearImage>,
    pub adjustments: Adjustments,
    pub recipe: ExportRecipe,
    /// Mark the colors the file clips (see `export::clipped_in`).
    pub gamut_warning: bool,
    /// Largest texture the GPU takes (bigger proofs are reduced for display only).
    pub max_texture_side: usize,
}

pub struct ProofImage {
    pub image: ColorImage,
    /// Size of the exported image and its file.
    pub width: usize,
    pub height: usize,
    pub file_size: usize,
}

/// One-click auto adjustment of images (see `sk_core::auto::auto_adjust`).
pub struct AutoJob {
    pub items: Vec<AutoItem>,
    pub opts: sk_core::auto::AutoOptions,
    /// Set to stop before the next image (the finished ones stay).
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
}

pub struct AutoItem {
    pub path: PathBuf,
    /// The open image as it is now (with unsaved changes); other images are loaded.
    pub open: Option<(Arc<sk_core::raw::RawMeta>, Arc<LinearImage>, Adjustments)>,
}

/// Long edge of the images loaded for the auto adjustment (the face detector takes 640).
const AUTO_EDGE: usize = 1024;

pub struct ExportJob {
    pub items: Vec<(PathBuf, Adjustments)>,
    pub recipes: Vec<ExportRecipe>,
    /// For name templates.
    pub session: String,
    /// Set to stop after the image being processed.
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AiKind {
    Subject,
    Sky,
    /// The points of a painted stroke.
    Magic { seeds: Vec<[f32; 2]>, tolerance: f32 },
    /// The object at the clicks so far (MobileSAM).
    Object { clicks: Vec<sk_ai::segment::Click> },
    /// Analyzes the image for `Object` before the first click (no result event).
    ObjectPrepare,
}

/// Computes a content-based mask for a layer.
pub struct AiJob {
    pub generation: u64,
    /// Id of the layer that gets the mask (see `Layer::id`).
    pub layer: u32,
    pub kind: AiKind,
    pub meta: Arc<RawMeta>,
    pub preview: Arc<LinearImage>,
    /// White balance / exposure used to build the guide image (no geometry).
    pub adjustments: Adjustments,
}

/// Fills the area marked by `strokes` (see `remove.rs`).
pub struct InpaintJob {
    pub generation: u64,
    pub path: PathBuf,
    /// Full resolution, if already loaded.
    pub full: Option<Arc<LinearImage>>,
    pub wb: [f32; 3],
    pub strokes: Vec<sk_core::params::BrushStroke>,
}

/// The current image with each style applied, small (see `style_grid`); replaces earlier jobs.
pub struct StyleThumbJob {
    pub key: u64,
    pub meta: Arc<RawMeta>,
    pub source: Arc<LinearImage>,
    /// Style id and the adjustments it gives the image.
    pub variants: Vec<(String, Adjustments)>,
}

/// Long edge of the source the style previews are rendered from.
const STYLE_THUMB_SOURCE: usize = 360;

pub struct Workers {
    pub events: Receiver<Event>,
    ev_tx: Sender<Event>,
    ctx: egui::Context,
    load_tx: Sender<(u64, PathBuf)>,
    full_tx: Sender<(u64, PathBuf)>,
    /// `None` cancels the pending tiles.
    tile_tx: Sender<Option<TileRequest>>,
    render_tx: Sender<RenderJob>,
    /// Image and the root of the open session (whose images are cached inside it).
    thumb_queue: crate::thumb_queue::ThumbQueue,
    export_tx: Sender<ExportJob>,
    index_tx: Sender<IndexJob>,
    ai_tx: Sender<AiJob>,
    before_tx: Sender<RenderJob>,
    quick_tx: Sender<(u64, PathBuf, Option<PathBuf>)>,
    prefetch_tx: Sender<(Vec<PathBuf>, Option<PathBuf>, u64)>,
    proxy_tx: Sender<(Vec<PathBuf>, Option<PathBuf>)>,
    proof_tx: Sender<ProofJob>,
    compare_tx: Sender<(Vec<PathBuf>, Option<PathBuf>)>,
    detail_tx: Sender<Vec<DetailJob>>,
    focus_tx: Sender<(PathBuf, Arc<ColorImage>, Option<u64>)>,
    thumb_focus_tx: Sender<(PathBuf, Option<PathBuf>)>,
    look_tx: Sender<crate::look_worker::LookRequest>,
    auto_tx: Sender<AutoJob>,
    inpaint_tx: Sender<InpaintJob>,
    style_tx: Sender<StyleThumbJob>,
    activity: Activity,
    pub draft_speed: DraftSpeed,
}

/// Measured speed of draft renderings (while a control is dragged), in nanoseconds per output
/// pixel; it decides how small drafts must be to keep up, instead of a fixed reduction.
#[derive(Clone, Default)]
pub struct DraftSpeed(Arc<AtomicU64>);

impl DraftSpeed {
    fn record(&self, pixels: usize, elapsed: Duration) {
        let ns = elapsed.as_nanos() as f64 / pixels.max(1) as f64;
        let old = f64::from_bits(self.0.load(Ordering::Relaxed));
        let new = if old > 0.0 { old * 0.7 + ns * 0.3 } else { ns };
        self.0.store(new.to_bits(), Ordering::Relaxed);
    }

    /// Output pixels that render within `ms`, once measured.
    pub fn pixels_within(&self, ms: f32) -> Option<f32> {
        let ns = f64::from_bits(self.0.load(Ordering::Relaxed));
        (ns > 0.0).then(|| (ms as f64 * 1e6 / ns) as f32)
    }
}

/// When the user or a foreground worker last did something; background preview
/// generation waits until things have been quiet for a while.
#[derive(Clone)]
pub struct Activity {
    start: Instant,
    last_ms: Arc<AtomicU64>,
}

impl Activity {
    fn new() -> Self {
        Self { start: Instant::now(), last_ms: Arc::new(AtomicU64::new(0)) }
    }

    pub fn touch(&self) {
        self.last_ms.store(self.start.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    fn quiet_for(&self) -> Duration {
        self.start.elapsed().saturating_sub(Duration::from_millis(self.last_ms.load(Ordering::Relaxed)))
    }
}

/// Background previews only start after this much quiet.
const PROXY_IDLE: Duration = Duration::from_millis(1500);

/// Receives the newest message, discarding older queued ones ("latest wins").
fn recv_latest<T>(rx: &Receiver<T>) -> Option<T> {
    let mut msg = rx.recv().ok()?;
    while let Ok(newer) = rx.try_recv() {
        msg = newer;
    }
    Some(msg)
}

impl Workers {
    /// An event as if a worker had sent it.
    #[cfg(test)]
    pub(crate) fn inject(&self, ev: Event) {
        let _ = self.ev_tx.send(ev);
    }

    pub fn spawn(ctx: egui::Context) -> Self {
        let (ev_tx, events) = channel();
        let (ev_tx_keep, ctx_keep) = (ev_tx.clone(), ctx.clone());
        let activity = Activity::new();
        let draft_speed = DraftSpeed::default();

        let (index_tx, index_rx) = channel::<IndexJob>();
        {
            let (ev_tx, ctx) = (ev_tx.clone(), ctx.clone());
            crate::guard::spawn("Index", move || {
                // The indexer owns its own connection (SQLite in WAL mode allows concurrent readers);
                // it is reopened when the app switches between catalog and session.
                let mut open: Option<(PathBuf, Catalog)> = None;
                while let Ok(job) = index_rx.recv() {
                    if open.as_ref().is_none_or(|(p, _)| *p != job.catalog) {
                        open = match Catalog::open(&job.catalog) {
                            Ok(c) => Some((job.catalog.clone(), c)),
                            Err(e) => {
                                log::error!("Katalog nicht verfügbar: {e:#}");
                                let _ = ev_tx.send(Event::IndexDone { catalog: job.catalog.clone(), indexed: 0, failed: job.paths.len(), import: job.import });
                                continue;
                            }
                        };
                    }
                    let catalog = &open.as_ref().expect("opened").1;
                    let import_id = if job.import { catalog.begin_import().unwrap_or(0) } else { 0 };
                    let total = job.paths.len();
                    let (mut indexed, mut failed) = (0, 0);
                    for (i, path) in job.paths.iter().enumerate() {
                        // A broken file (even one that panics) fails alone; the rest of the import goes on.
                        let path = match &job.copy_to {
                            Some(dest) => match crate::guard::catch(|| session::copy_image(path, dest)).and_then(|r| r.map_err(|e| format!("{e:#}"))) {
                                Ok(p) => p,
                                Err(e) => {
                                    log::warn!("{e}");
                                    failed += 1;
                                    continue;
                                }
                            },
                            None => path.clone(),
                        };
                        // A file comes with its variants.
                        let images = if sk_core::variant::number(&path) == 1 { sk_core::variant::of_file(&path) } else { vec![path] };
                        for path in images {
                            let needs = job.import || catalog.needs_index(&path).unwrap_or(true);
                            if needs {
                                match crate::guard::catch(|| catalog.index_file(&path, import_id)).and_then(|r| r.map_err(|e| format!("{e:#}"))) {
                                    Ok(_) => indexed += 1,
                                    Err(e) => {
                                        log::warn!("{}: {e}", path.display());
                                        failed += 1;
                                    }
                                }
                            }
                        }
                        if i % 10 == 9 || i + 1 == total {
                            let _ = ev_tx.send(Event::IndexProgress { done: i + 1, total });
                            ctx.request_repaint();
                        }
                    }
                    let _ = ev_tx.send(Event::IndexDone { catalog: job.catalog.clone(), indexed, failed, import: job.import });
                    ctx.request_repaint();
                }
            });
        }

        let (load_tx, load_rx) = channel::<(u64, PathBuf)>();
        {
            let (ev_tx, ctx, activity) = (ev_tx.clone(), ctx.clone(), activity.clone());
            crate::guard::spawn("Vorschau laden", move || {
                while let Some((generation, path)) = recv_latest(&load_rx) {
                    activity.touch();
                    // A panic (broken file) must still answer, or the viewer waits for ever.
                    let result = crate::guard::catch(|| {
                        let (meta, preview) = raw::load_preview(&path, PREVIEW_EDGE).map_err(|e| format!("{e:#}"))?;
                        // Fitted here rather than when the image is shown (see `App::install`).
                        if sk_core::camera_look::wanted(&sidecar::load(&path).adjustments, sk_core::camera_look::for_new()) {
                            sk_core::camera_look::fit_cached(&meta, &preview);
                        }
                        Ok((Arc::new(meta), Arc::new(preview)))
                    })
                    .and_then(|r| r);
                    let _ = ev_tx.send(Event::Loaded { generation, result });
                    ctx.request_repaint();
                }
            });
        }

        let (full_tx, full_rx) = channel::<(u64, PathBuf)>();
        {
            let (ev_tx, ctx, activity) = (ev_tx.clone(), ctx.clone(), activity.clone());
            crate::guard::spawn("Volle Auflösung laden", move || {
                while let Some((generation, path)) = recv_latest(&full_rx) {
                    activity.touch();
                    match raw::load_full(&path) {
                        Ok((_, img)) => {
                            let _ = ev_tx.send(Event::FullLoaded { generation, image: Arc::new(img) });
                            ctx.request_repaint();
                        }
                        Err(e) => log::warn!("Vollauflösung nicht geladen: {e:#}"),
                    }
                }
            });
        }

        let (tile_tx, tile_rx) = channel::<Option<TileRequest>>();
        {
            let (ev_tx, ctx, activity, speed) = (ev_tx.clone(), ctx.clone(), activity.clone(), draft_speed.clone());
            crate::guard::spawn("Kacheln", move || {
                let cache = StageCache::default();
                // Reduced copy of the full resolution for view drafts: (source, level, copy).
                let mut reduced: Option<(Arc<LinearImage>, usize, Arc<LinearImage>)> = None;
                while let Some(msg) = recv_latest(&tile_rx) {
                    let Some(mut req) = msg else { continue };
                    let mut queue = std::mem::take(&mut req.tiles);
                    queue.reverse();
                    loop {
                        let adj = if req.before {
                            let mut a = Adjustments::default();
                            a.keep_framing_from(&req.adjustments);
                            a
                        } else {
                            req.adjustments.clone()
                        };
                        let opts = RenderOptions { clip_warning: req.clip_warning, mask_overlay: req.mask_overlay, softproof: req.softproof.clone(), ..Default::default() };
                        let p = Processor::with_options(&req.meta, &adj, opts);
                        let (fw, fh) = p.output_size(req.full.width, req.full.height);
                        let job = match req.view.take() {
                            Some((region, level)) => Some((None, region, level.max(1))),
                            None => {
                                let Some((tx, ty)) = queue.pop() else { break };
                                let (x, y) = (tx * TILE, ty * TILE);
                                (x < fw && y < fh).then(|| (Some((tx, ty)), Region { x, y, w: TILE.min(fw - x), h: TILE.min(fh - y) }, 1))
                            }
                        };
                        if let Some((index, region, level)) = job {
                            activity.touch();
                            let src = if level == 1 {
                                req.full.clone()
                            } else {
                                match &reduced {
                                    Some((full, l, small)) if *l == level && Arc::ptr_eq(full, &req.full) => small.clone(),
                                    _ => {
                                        let small = Arc::new(req.full.box_downscaled(level));
                                        reduced = Some((req.full.clone(), level, small.clone()));
                                        small
                                    }
                                }
                            };
                            // The region at the reduced scale, within the reduced output.
                            let (lw, lh) = p.output_size(src.width, src.height);
                            let (x, y) = (region.x / level, region.y / level);
                            if x < lw && y < lh {
                                let scaled = Region { x, y, w: region.w.div_ceil(level).min(lw - x), h: region.h.div_ceil(level).min(lh - y) };
                                let t0 = Instant::now();
                                let r = p.render(&src, Some(scaled), Some(&cache));
                                if level > 1 {
                                    speed.record(r.width * r.height, t0.elapsed());
                                }
                                let image = ColorImage::from_rgba_unmultiplied([r.width, r.height], &r.rgba);
                                let focus = (req.focus && level == 1).then(|| crate::focus::sharpness_of(&image, r.full_size.0.max(r.full_size.1)));
                                let _ = ev_tx.send(Event::TileRendered {
                                    generation: req.generation,
                                    version: req.version,
                                    index,
                                    region: Region { x: x * level, y: y * level, w: scaled.w * level, h: scaled.h * level },
                                    level,
                                    full_size: (r.full_size.0 * level, r.full_size.1 * level),
                                    image,
                                    focus,
                                });
                                ctx.request_repaint();
                            }
                        }
                        // A newer request (other image, edit, or view) replaces the rest of the queue.
                        match tile_rx.try_iter().last() {
                            Some(Some(mut newer)) => {
                                queue = std::mem::take(&mut newer.tiles);
                                queue.reverse();
                                req = newer;
                            }
                            Some(None) => break,
                            None => {}
                        }
                    }
                }
            });
        }

        let (ai_tx, ai_rx) = channel::<AiJob>();
        {
            let (ev_tx, ctx) = (ev_tx.clone(), ctx.clone());
            crate::guard::spawn("KI-Maske", move || {
                let mut model: Option<(sk_ai::SubjectModel, u64)> = None;
                let mut object: Option<(sk_ai::segment::ObjectModel, u64)> = None;
                // The encoder's result for the last image (by its guide), reused for every click.
                let mut embedding: Option<(u64, sk_ai::segment::Embedding)> = None;
                while let Ok(job) = ai_rx.recv() {
                    let prepare = matches!(job.kind, AiKind::ObjectPrepare);
                    // A panic must still answer: the layer panel waits for the result.
                    let result = crate::guard::catch(|| {
                        // Guide: the image in source orientation, only white balance and exposure applied.
                        let guide_adj = Adjustments {
                            white_balance: job.adjustments.white_balance.clone(),
                            exposure: job.adjustments.exposure,
                            detail: sk_core::params::Detail { sharpen_amount: 0.0, nr_color: 0.0, ..Default::default() },
                            ..Default::default()
                        };
                        let r = Processor::new(&job.meta, &guide_adj).render(&job.preview, None, None);
                        let guide = sk_ai::guide_from_rgba(r.width, r.height, &r.rgba);
                        match job.kind {
                            AiKind::Object { .. } | AiKind::ObjectPrepare => {
                                let generation = sk_ai::runtime::config_generation();
                                if object.as_ref().is_none_or(|(_, g)| *g != generation) && sk_ai::segment::available() {
                                    object = sk_ai::segment::ObjectModel::load().map_err(|e| log::warn!("{e:#}")).ok().map(|m| (m, generation));
                                }
                                match &object {
                                    Some((m, _)) => {
                                        let key = cache::fnv1a(format!("{}|{}x{}", job.meta.path.display(), r.width, r.height).as_bytes())
                                            ^ cache::fnv1a(&r.rgba.iter().step_by(97).copied().collect::<Vec<u8>>());
                                        if embedding.as_ref().is_none_or(|(k, _)| *k != key) {
                                            embedding = None;
                                            match m.embed(&guide) {
                                                Ok(e) => embedding = Some((key, e)),
                                                Err(e) => log::warn!("Objekt auswählen: {e:#}"),
                                            }
                                        }
                                        match (&job.kind, &embedding) {
                                            (AiKind::Object { clicks }, Some((_, e))) => m.mask(e, clicks).map_err(|e| format!("{e:#}")),
                                            (AiKind::Object { .. }, None) => Err("Das Bild konnte nicht analysiert werden".into()),
                                            _ => Err(String::new()),
                                        }
                                    }
                                    None => Err("OBJECT_MODEL_MISSING".into()),
                                }
                            }
                            AiKind::Sky => Ok(sk_ai::sky_mask(&guide)),
                            AiKind::Magic { seeds, tolerance } => Ok(sk_ai::magic_mask(&guide, &seeds, tolerance)),
                            AiKind::Subject => {
                                let generation = sk_ai::runtime::config_generation();
                                if model.as_ref().is_none_or(|(_, g)| *g != generation) {
                                    model = sk_ai::SubjectModel::load(&sk_ai::default_model_path()).map_err(|e| log::warn!("{e:#}")).ok().map(|m| (m, generation));
                                }
                                match &model {
                                    Some((m, _)) => m.subject_mask(&guide).map_err(|e| format!("{e:#}")),
                                    None => Err("MODEL_MISSING".into()),
                                }
                            }
                        }
                    })
                    .and_then(|r| r);
                    if prepare {
                        continue;
                    }
                    let _ = ev_tx.send(Event::AiMask { generation: job.generation, layer: job.layer, result });
                    ctx.request_repaint();
                }
            });
        }

        // Separate renderer for the "before" image of the split view.
        let (before_tx, before_rx) = channel::<RenderJob>();
        {
            let (ev_tx, ctx) = (ev_tx.clone(), ctx.clone());
            crate::guard::spawn("Vorher", move || {
                while let Some(job) = recv_latest(&before_rx) {
                    let mut adj = Adjustments::default();
                    adj.keep_framing_from(&job.adjustments);
                    let r = Processor::new(&job.meta, &adj).render(&job.preview, None, None);
                    let image = ColorImage::from_rgba_unmultiplied([r.width, r.height], &r.rgba);
                    let _ = ev_tx.send(Event::Rendered { generation: job.generation, version: job.version, draft: false, image, thumb: None, histogram: r.histogram, before: true });
                    ctx.request_repaint();
                }
            });
        }

        let (render_tx, render_rx) = channel::<RenderJob>();
        {
            let (ev_tx, ctx, activity, speed) = (ev_tx.clone(), ctx.clone(), activity.clone(), draft_speed.clone());
            crate::guard::spawn("Rendern", move || {
                let cache = StageCache::default();
                // Drafts: reduced copy of the preview (made once per image and level) with its own stage cache.
                let draft_cache = StageCache::default();
                let mut draft_src: Option<(Arc<LinearImage>, usize, Arc<LinearImage>)> = None;
                // Last plain rendering, written to the preview cache once no new job arrives for a while
                // (not on every slider step).
                let mut pending: Option<(PathBuf, Option<PathBuf>, Adjustments, ::image::DynamicImage)> = None;
                // Image of the last whole rendering at full preview resolution: its stages are in
                // `cache`, which makes rendering parts fast (and exactly like the whole).
                let mut whole: Option<(u64, *const LinearImage)> = None;
                loop {
                    let job = if pending.is_some() {
                        match render_rx.recv_timeout(Duration::from_millis(800)) {
                            Ok(job) => job,
                            Err(RecvTimeoutError::Timeout) => {
                                if let Some((path, session, adj, img)) = pending.take() {
                                    cache::store_preview(&path, &adj, session.as_deref(), &img);
                                }
                                continue;
                            }
                            Err(RecvTimeoutError::Disconnected) => break,
                        }
                    } else {
                        match render_rx.recv() {
                            Ok(job) => job,
                            Err(_) => break,
                        }
                    };
                    // The newest job; the parts of those skipped are rendered with it.
                    let mut job = job;
                    for next in render_rx.try_iter() {
                        let part = match (job.part, next.part) {
                            (Some(a), Some(b)) if job.generation == next.generation => Some(union(a, b)),
                            _ => None,
                        };
                        job = RenderJob { part, ..next };
                    }
                    if job.part.is_some() && whole != Some((job.generation, Arc::as_ptr(&job.preview))) {
                        job.part = None;
                    }
                    activity.touch();
                    let adj = if job.before {
                        let mut a = Adjustments::default();
                        a.keep_framing_from(&job.adjustments);
                        a
                    } else {
                        job.adjustments.clone()
                    };
                    let opts = RenderOptions { clip_warning: job.clip_warning, ignore_crop: job.ignore_crop, mask_overlay: job.mask_overlay, softproof: job.softproof.clone() };
                    let processor = Processor::with_options(&job.meta, &adj, opts);
                    if let Some(part) = job.part {
                        // Sharpening and glow carry the change a little beyond what was painted.
                        let (w, h) = processor.output_size(job.preview.width, job.preview.height);
                        let m = processor.output_spread(job.preview.width, job.preview.height);
                        let (x, y) = (part.x.saturating_sub(m), part.y.saturating_sub(m));
                        let part = Region { x, y, w: (part.x + part.w + m).min(w).saturating_sub(x), h: (part.y + part.h + m).min(h).saturating_sub(y) };
                        if part.w == 0 || part.h == 0 {
                            continue;
                        }
                        let r = processor.render(&job.preview, Some(part), Some(&cache));
                        let image = ColorImage::from_rgba_unmultiplied([r.width, r.height], &r.rgba);
                        let _ = ev_tx.send(Event::RenderedPart { generation: job.generation, version: job.version, x: part.x, y: part.y, image });
                        ctx.request_repaint();
                        continue;
                    }
                    let draft = job.draft > 1;
                    let r = if draft {
                        let small = match &draft_src {
                            Some((src, level, small)) if *level == job.draft && Arc::ptr_eq(src, &job.preview) => small.clone(),
                            _ => {
                                let small = Arc::new(job.preview.box_downscaled(job.draft));
                                draft_src = Some((job.preview.clone(), job.draft, small.clone()));
                                small
                            }
                        };
                        let t0 = Instant::now();
                        let r = processor.render(&small, None, Some(&draft_cache));
                        speed.record(r.width * r.height, t0.elapsed());
                        r
                    } else {
                        // Measured as well, so that the first drag already knows the speed.
                        let t0 = Instant::now();
                        let r = processor.render(&job.preview, None, Some(&cache));
                        speed.record(r.width * r.height, t0.elapsed());
                        // (The "before" view has other early stages.)
                        whole = (!job.before).then_some((job.generation, Arc::as_ptr(&job.preview)));
                        r
                    };
                    let image = ColorImage::from_rgba_unmultiplied([r.width, r.height], &r.rgba);
                    // Thumbnails and cached previews only from regular renderings (no drafts, overlays, warnings or crop tool view).
                    let plain = !draft && !job.before && !job.clip_warning && job.mask_overlay.is_none() && !job.ignore_crop && job.softproof.is_none();
                    let full = plain
                        .then(|| ::image::RgbaImage::from_raw(r.width as u32, r.height as u32, r.rgba))
                        .flatten()
                        .map(::image::DynamicImage::ImageRgba8);
                    // (Not for style previews, which are rendered without `cache_as`.)
                    let thumb = full.as_ref().filter(|_| job.cache_as.is_some()).map(|img| to_color_image(&img.thumbnail(THUMB_EDGE, THUMB_EDGE)));
                    let _ = ev_tx.send(Event::Rendered {
                        generation: job.generation,
                        version: job.version,
                        draft,
                        image,
                        thumb,
                        histogram: r.histogram,
                        before: job.before,
                    });
                    ctx.request_repaint();
                    if let (Some(img), Some((path, session))) = (full, job.cache_as) {
                        // A different image: store the previous one right away.
                        if let Some((p, sess, a, prev)) = pending.take_if(|(p, ..)| *p != path) {
                            cache::store_preview(&p, &a, sess.as_deref(), &prev);
                        }
                        pending = Some((path, session, job.adjustments, img));
                    }
                }
                if let Some((path, session, adj, img)) = pending {
                    cache::store_preview(&path, &adj, session.as_deref(), &img);
                }
            });
        }

        let (quick_tx, quick_rx) = channel::<(u64, PathBuf, Option<PathBuf>)>();
        {
            let (ev_tx, ctx) = (ev_tx.clone(), ctx.clone());
            crate::guard::spawn("Schnellvorschau", move || {
                while let Some((generation, path, session)) = recv_latest(&quick_rx) {
                    if let Some(img) = quick_preview(&path, session.as_deref()) {
                        let _ = ev_tx.send(Event::Quick { generation, image: to_color_image(&img) });
                        ctx.request_repaint();
                    }
                }
            });
        }

        let (prefetch_tx, prefetch_rx) = channel::<(Vec<PathBuf>, Option<PathBuf>, u64)>();
        {
            let (ev_tx, ctx, activity) = (ev_tx.clone(), ctx.clone(), activity.clone());
            crate::guard::spawn("Vorladen", move || {
                while let Some((mut queue, mut session, mut relocations)) = recv_latest(&prefetch_rx) {
                    queue.reverse();
                    while let Some(path) = queue.pop() {
                        activity.touch();
                        if let Some(p) = prepare(&path, session.as_deref()) {
                            let _ = ev_tx.send(Event::Prepared(Box::new(Prepared { relocations, ..p })));
                            ctx.request_repaint();
                        }
                        // A newer request (the user moved on) replaces the rest of the queue.
                        if let Some((q, s, r)) = prefetch_rx.try_iter().last() {
                            (queue, session, relocations) = (q, s, r);
                            queue.reverse();
                        }
                    }
                }
            });
        }

        let (proof_tx, proof_rx) = channel::<ProofJob>();
        {
            let (ev_tx, ctx, activity) = (ev_tx.clone(), ctx.clone(), activity.clone());
            crate::guard::spawn("Softproof", move || {
                while let Some(job) = recv_latest(&proof_rx) {
                    activity.touch();
                    let processor = Processor::new(&job.meta, &job.adjustments);
                    let processed = processor.process(&job.full, None, job.recipe.output_space);
                    // Clipping shows only in a wider space: ProPhoto holds nearly all working colors.
                    let clipped = (job.gamut_warning && job.recipe.output_space != sk_core::color::OutputSpace::ProPhoto)
                        .then(|| export::clipped_in(&processor.process(&job.full, None, sk_core::color::OutputSpace::ProPhoto), job.recipe.output_space))
                        .transpose();
                    let result = clipped
                        .and_then(|c| export::proof(&processed, &job.recipe, &job.meta.info, c.as_deref()))
                        .map_err(|e| format!("{e:#}")).map(|p| {
                        let mut image = ColorImage::from_rgba_unmultiplied([p.width, p.height], &p.rgba);
                        if p.width.max(p.height) > job.max_texture_side
                            && let Some(img) = ::image::RgbaImage::from_raw(p.width as u32, p.height as u32, p.rgba)
                        {
                            let side = job.max_texture_side as u32;
                            image = to_color_image(&::image::DynamicImage::ImageRgba8(img).resize(side, side, ::image::imageops::FilterType::Triangle));
                        }
                        ProofImage { image, width: p.width, height: p.height, file_size: p.file_size }
                    });
                    let _ = ev_tx.send(Event::Proof { generation: job.generation, version: job.version, key: job.key, result });
                    ctx.request_repaint();
                }
            });
        }

        let (proxy_tx, proxy_rx) = channel::<(Vec<PathBuf>, Option<PathBuf>)>();
        {
            let (ev_tx, ctx, activity) = (ev_tx.clone(), ctx.clone(), activity.clone());
            crate::guard::spawn("Proxys", move || {
                while let Some(mut job) = recv_latest(&proxy_rx) {
                    'list: loop {
                        let (paths, session) = &job;
                        let total = paths.len();
                        for (done, path) in paths.iter().enumerate() {
                            // Only while nothing else is going on; a new list starts over.
                            while activity.quiet_for() < PROXY_IDLE {
                                thread::sleep(Duration::from_millis(200));
                                if let Some(newer) = proxy_rx.try_iter().last() {
                                    job = newer;
                                    continue 'list;
                                }
                            }
                            if let Some(newer) = proxy_rx.try_iter().last() {
                                job = newer;
                                continue 'list;
                            }
                            let adj = sidecar::load(path).adjustments;
                            if cache::has_preview(path, &adj, session.as_deref()) {
                                continue;
                            }
                            let _ = ev_tx.send(Event::ProxyProgress(Some((done, total))));
                            if let Some(p) = prepare(path, session.as_deref()) {
                                // The browser thumbnail then shows this rendering too (not the camera JPEG).
                                let thumb = ::image::RgbaImage::from_raw(p.image.size[0] as u32, p.image.size[1] as u32, p.image.as_raw().to_vec())
                                    .map(|img| ::image::DynamicImage::ImageRgba8(img).thumbnail(THUMB_EDGE, THUMB_EDGE));
                                if let Some(thumb) = thumb {
                                    cache::store(path, &adj, session.as_deref(), &thumb);
                                    let _ = ev_tx.send(Event::Thumbnail { path: path.clone(), image: to_color_image(&thumb) });
                                }
                            }
                            ctx.request_repaint();
                        }
                        let _ = ev_tx.send(Event::ProxyProgress(None));
                        ctx.request_repaint();
                        break;
                    }
                }
            });
        }

        let (thumb_queue, thumb_source) = crate::thumb_queue::queue();
        for _ in 0..thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 4)) {
            let (ev_tx, ctx, source) = (ev_tx.clone(), ctx.clone(), thumb_source.clone());
            crate::guard::spawn("Thumbnails", move || {
                while let Some((path, session)) = source.next() {
                    if let Some(image) = make_thumbnail(&path, session.as_deref()) {
                        let _ = ev_tx.send(Event::Thumbnail { path, image });
                        ctx.request_repaint();
                    }
                }
            });
        }

        let (compare_tx, compare_rx) = channel::<(Vec<PathBuf>, Option<PathBuf>)>();
        {
            let (ev_tx, ctx, activity) = (ev_tx.clone(), ctx.clone(), activity.clone());
            crate::guard::spawn("Vergleich", move || {
                while let Some((mut queue, mut session)) = recv_latest(&compare_rx) {
                    queue.reverse();
                    while let Some(path) = queue.pop() {
                        activity.touch();
                        // The cached preview if it is up to date, else a fresh rendering.
                        let adj = sidecar::load(&path).adjustments;
                        let image = match cache::load_preview(&path, &adj, session.as_deref()) {
                            Some(img) => Some(to_color_image(&img)),
                            None => prepare(&path, session.as_deref()).map(|p| p.image),
                        };
                        if let Some(image) = image {
                            let _ = ev_tx.send(Event::CompareImage { path, image });
                            ctx.request_repaint();
                        }
                        // A newer request contains everything still missing.
                        if let Some((q, s)) = compare_rx.try_iter().last() {
                            (queue, session) = (q, s);
                            queue.reverse();
                        }
                    }
                }
            });
        }

        let (style_tx, style_rx) = channel::<StyleThumbJob>();
        {
            let (ev_tx, ctx, activity) = (ev_tx.clone(), ctx.clone(), activity.clone());
            crate::guard::spawn("Stilvorschau", move || {
                let mut next = recv_latest(&style_rx);
                while let Some(job) = next.take() {
                    let src = &job.source;
                    let k = STYLE_THUMB_SOURCE as f32 / src.width.max(src.height).max(1) as f32;
                    let small = if k < 1.0 {
                        src.resize_area(((src.width as f32 * k).round() as usize).max(1), ((src.height as f32 * k).round() as usize).max(1))
                    } else {
                        (**src).clone()
                    };
                    for (id, adj) in job.variants {
                        // A newer image or edit: these previews are no longer wanted.
                        if let Some(newer) = style_rx.try_iter().last() {
                            next = Some(newer);
                            break;
                        }
                        activity.touch();
                        let r = Processor::new(&job.meta, &adj).render(&small, None, None);
                        let image = ColorImage::from_rgba_unmultiplied([r.width, r.height], &r.rgba);
                        let _ = ev_tx.send(Event::StyleThumb { key: job.key, id, image });
                        ctx.request_repaint();
                    }
                    if next.is_none() {
                        next = recv_latest(&style_rx);
                    }
                }
            });
        }

        let (detail_tx, detail_rx) = channel::<Vec<DetailJob>>();
        {
            let (ev_tx, ctx, activity) = (ev_tx.clone(), ctx.clone(), activity.clone());
            crate::guard::spawn("Vergleichsdetails", move || {
                // Recently decoded full resolutions (the newest last), as panning needs them again.
                let mut fulls: Vec<(PathBuf, Arc<RawMeta>, Arc<LinearImage>)> = Vec::new();
                while let Some(mut queue) = recv_latest(&detail_rx) {
                    queue.reverse();
                    while let Some(job) = queue.pop() {
                        activity.touch();
                        if let Some(event) = render_detail(job, &mut fulls) {
                            let _ = ev_tx.send(event);
                            ctx.request_repaint();
                        }
                        // A newer request contains everything still missing.
                        if let Some(q) = detail_rx.try_iter().last() {
                            queue = q;
                            queue.reverse();
                        }
                    }
                }
            });
        }

        let (thumb_focus_tx, thumb_focus_rx) = channel::<(PathBuf, Option<PathBuf>)>();
        {
            let (ev_tx, ctx) = (ev_tx.clone(), ctx.clone());
            crate::guard::spawn("Fokus Vorschaubilder", move || {
                while let Ok(first) = thumb_focus_rx.recv() {
                    let mut jobs = vec![first];
                    for job in thumb_focus_rx.try_iter() {
                        jobs.retain(|j| j.0 != job.0);
                        jobs.push(job);
                    }
                    for (path, session) in jobs {
                        if let Some((size, sharpness)) = thumb_sharpness(&path, session.as_deref()) {
                            let _ = ev_tx.send(Event::ThumbFocus { path, size, sharpness });
                            ctx.request_repaint();
                        }
                    }
                }
            });
        }

        let (focus_tx, focus_rx) = channel::<(PathBuf, Arc<ColorImage>, Option<u64>)>();
        {
            let (ev_tx, ctx) = (ev_tx.clone(), ctx.clone());
            crate::guard::spawn("Fokus", move || {
                while let Ok(first) = focus_rx.recv() {
                    // Only the newest rendering of each image is worth analysing.
                    let mut jobs = vec![first];
                    for job in focus_rx.try_iter() {
                        jobs.retain(|j| j.0 != job.0);
                        jobs.push(job);
                    }
                    for (path, image, version) in jobs {
                        let [w, h] = image.size;
                        let sharpness = crate::focus::sharpness_of(&image, w.max(h));
                        let _ = ev_tx.send(Event::Focus { path, version, size: [w, h], sharpness });
                        ctx.request_repaint();
                    }
                }
            });
        }

        let look_tx = crate::look_worker::spawn(ev_tx.clone(), ctx.clone(), activity.clone());

        let (auto_tx, auto_rx) = channel::<AutoJob>();
        {
            let (ev_tx, ctx, activity) = (ev_tx.clone(), ctx.clone(), activity.clone());
            crate::guard::spawn("Automatisch", move || {
                // Loaded on first use, kept for further images.
                let mut models = sk_ai::auto::AutoModels::default();
                while let Ok(job) = auto_rx.recv() {
                    for item in job.items {
                        if job.cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        activity.touch();
                        let path = item.path;
                        let result = crate::guard::catch(|| {
                            let (meta, preview, adj) = match item.open {
                                Some(open) => open,
                                None => {
                                    let (meta, preview) = raw::load_preview(&path, AUTO_EDGE).map_err(|e| format!("{}: {e:#}", path.display()))?;
                                    (Arc::new(meta), Arc::new(preview), sidecar::load(&path).adjustments)
                                }
                            };
                            // On the camera's look, if the image is to get it (see `camera_look`).
                            let adj = sk_core::camera_look::apply_if_wanted(&meta, &preview, &adj, sk_core::camera_look::for_new()).unwrap_or(adj);
                            let guide = sk_core::auto::guide_image(&meta, &adj, &preview, &job.opts);
                            let focus = models.focus(&guide);
                            let camera = sk_core::auto::camera_brightness(&meta);
                            Ok(Box::new(sk_core::auto::auto_adjust(&meta, &adj, &preview, &job.opts, &focus, camera)))
                        })
                        .and_then(|r| r);
                        let _ = ev_tx.send(Event::AutoAdjusted { path, opts: job.opts, result });
                        ctx.request_repaint();
                    }
                }
            });
        }

        let (inpaint_tx, inpaint_rx) = channel::<InpaintJob>();
        {
            let (ev_tx, ctx, activity) = (ev_tx.clone(), ctx.clone(), activity.clone());
            crate::guard::spawn("Entfernen", move || {
                let mut model: Option<(sk_ai::inpaint::Inpainter, u64)> = None;
                while let Ok(job) = inpaint_rx.recv() {
                    activity.touch();
                    let progress = |p: f32| {
                        let _ = ev_tx.send(Event::RemovalProgress { generation: job.generation, progress: p });
                        ctx.request_repaint();
                    };
                    let result = crate::guard::catch(|| crate::remove::compute(&job, &mut model, &progress)).and_then(|r| r);
                    let _ = ev_tx.send(Event::Removal { generation: job.generation, result });
                    ctx.request_repaint();
                }
            });
        }

        let (export_tx, export_rx) = channel::<ExportJob>();
        {
            let (ev_tx, ctx) = (ev_tx, ctx);
            crate::guard::spawn("Export", move || {
                while let Ok(job) = export_rx.recv() {
                    let total = job.items.len() * job.recipes.len();
                    let mut done = 0;
                    let mut written = HashSet::new();
                    for (i, (path, adj)) in job.items.into_iter().enumerate() {
                        if job.cancel.load(Ordering::Relaxed) {
                            let _ = ev_tx.send(Event::ExportCancelled { done, total });
                            ctx.request_repaint();
                            break;
                        }
                        // An image never opened gets the camera's look it would show (see `camera_look`).
                        let for_new = sk_core::camera_look::for_new();
                        let adj = match sk_core::camera_look::wanted(&adj, for_new) {
                            true => raw::load_preview(&path, 1024)
                                .ok()
                                .and_then(|(meta, preview)| sk_core::camera_look::apply_if_wanted(&meta, &preview, &adj, for_new))
                                .unwrap_or(adj),
                            false => adj,
                        };
                        // A bug on one image fails its outputs, not the whole export.
                        let results =
                            crate::guard::catch(|| export::export_many_with(&path, &adj, &job.recipes, i as u32 + 1, &job.session, &mut written));
                        if results.is_err() && job.cancel.load(Ordering::Relaxed) {
                            let _ = ev_tx.send(Event::ExportCancelled { done, total });
                            ctx.request_repaint();
                            break;
                        }
                        let results = results.unwrap_or_else(|e| job.recipes.iter().map(|_| Err(anyhow::anyhow!("{e}"))).collect());
                        for r in results {
                            done += 1;
                            let last = r.map_err(|e| format!("{}: {e:#}", path.display()));
                            let _ = ev_tx.send(Event::ExportProgress { done, total, last });
                            ctx.request_repaint();
                        }
                    }
                }
            });
        }

        Self {
            events,
            ev_tx: ev_tx_keep,
            ctx: ctx_keep,
            load_tx,
            full_tx,
            tile_tx,
            render_tx,
            thumb_queue,
            export_tx,
            index_tx,
            ai_tx,
            before_tx,
            quick_tx,
            prefetch_tx,
            proxy_tx,
            proof_tx,
            compare_tx,
            style_tx,
            detail_tx,
            focus_tx,
            thumb_focus_tx,
            look_tx,
            auto_tx,
            inpaint_tx,
            activity,
            draft_speed,
        }
    }

    pub fn event_sender(&self) -> Sender<Event> {
        self.ev_tx.clone()
    }

    pub fn context(&self) -> egui::Context {
        self.ctx.clone()
    }

    pub fn proof(&self, job: ProofJob) {
        let _ = self.proof_tx.send(job);
    }

    /// Something is going on (input, loading, rendering): background previews pause.
    pub fn touch(&self) {
        self.activity.touch();
    }

    /// Creates the missing viewer previews (and matching thumbnails) of these images in the
    /// background, in order, whenever the app is idle; replaces earlier requests.
    pub fn proxies(&self, paths: Vec<PathBuf>, session: Option<PathBuf>) {
        let _ = self.proxy_tx.send((paths, session));
    }

    /// Shows a stand-in for the image as fast as possible (see [`Event::Quick`]).
    pub fn quick(&self, generation: u64, path: PathBuf, session: Option<PathBuf>) {
        let _ = self.quick_tx.send((generation, path, session));
    }

    /// Loads and renders these images ahead of time, in order; replaces earlier requests.
    pub fn prefetch(&self, paths: Vec<PathBuf>, session: Option<PathBuf>, relocations: u64) {
        let _ = self.prefetch_tx.send((paths, session, relocations));
    }

    /// Renders these images for the compare view, in order; replaces earlier requests.
    pub fn compare(&self, paths: Vec<PathBuf>, session: Option<PathBuf>) {
        let _ = self.compare_tx.send((paths, session));
    }

    /// Computes the focus mask of a displayed image (see [`Event::Focus`]).
    /// Renders parts of compare images at a higher resolution (replaces earlier requests).
    pub fn compare_details(&self, jobs: Vec<DetailJob>) {
        let _ = self.detail_tx.send(jobs);
    }

    pub fn focus(&self, path: PathBuf, image: Arc<ColorImage>, version: Option<u64>) {
        let _ = self.focus_tx.send((path, image, version));
    }

    /// Computes the focus mask of a thumbnail (see [`Event::ThumbFocus`]).
    pub fn thumb_focus(&self, path: PathBuf, session: Option<PathBuf>) {
        let _ = self.thumb_focus_tx.send((path, session));
    }

    pub fn inpaint(&self, job: InpaintJob) {
        let _ = self.inpaint_tx.send(job);
    }

    pub fn style_thumbs(&self, job: StyleThumbJob) {
        let _ = self.style_tx.send(job);
    }

    pub fn look(&self, request: crate::look_worker::LookRequest) {
        let _ = self.look_tx.send(request);
    }

    pub fn auto_adjust(&self, job: AutoJob) {
        let _ = self.auto_tx.send(job);
    }

    pub fn load(&self, generation: u64, path: PathBuf) {
        let _ = self.load_tx.send((generation, path));
    }

    pub fn load_full(&self, generation: u64, path: PathBuf) {
        let _ = self.full_tx.send((generation, path));
    }

    pub fn tiles(&self, req: TileRequest) {
        let _ = self.tile_tx.send(Some(req));
    }

    pub fn cancel_tiles(&self) {
        let _ = self.tile_tx.send(None);
    }

    pub fn render(&self, job: RenderJob) {
        let _ = self.render_tx.send(job);
    }

    pub fn thumbnail(&self, path: PathBuf, session: Option<PathBuf>) {
        // Newest first: cells just scrolled into view before those scrolled past.
        self.thumb_queue.request(self.ctx.cumulative_pass_nr(), path, session);
    }

    pub fn export(&self, job: ExportJob) {
        let _ = self.export_tx.send(job);
    }

    pub fn ai_mask(&self, job: AiJob) {
        let _ = self.ai_tx.send(job);
    }

    pub fn render_before(&self, job: RenderJob) {
        let _ = self.before_tx.send(job);
    }

    /// Reads the lensfun database in the background and downloads it first if it is missing or
    /// older than 30 days. Without network the existing copy is used.
    pub fn load_lens_db(&self) {
        let (tx, ctx) = (self.ev_tx.clone(), self.ctx.clone());
        thread::spawn(move || {
            let send = |ev| {
                let _ = tx.send(ev);
                ctx.request_repaint();
            };
            let db = crate::guard::catch(sk_core::lensfun::load_database).unwrap_or_default();
            let download = sk_core::lensfun::needs_download(&db);
            let had_db = !db.is_empty();
            if !db.is_empty() || !download {
                send(Event::LensDb(Arc::new(db)));
            }
            if download {
                send(Event::LensDbDownloading(true));
                let ok = crate::guard::catch(|| sk_core::lensfun::download_database().is_ok()).unwrap_or(false);
                send(Event::LensDbDownloading(false));
                // Also without network: an empty database ends the waiting in the panel.
                let db = if ok { sk_core::lensfun::load_database() } else { Vec::new() };
                if ok || !had_db {
                    send(Event::LensDb(Arc::new(db)));
                }
            }
        });
    }

    pub fn index(&self, job: IndexJob) {
        let _ = self.index_tx.send(job);
    }
}

/// The cached preview with the saved adjustments, or the camera JPEG for unedited images.
fn quick_preview(path: &std::path::Path, session: Option<&std::path::Path>) -> Option<::image::DynamicImage> {
    let adj = sidecar::load(path).adjustments;
    cache::load_preview(path, &adj, session).or_else(|| (adj == Adjustments::default()).then(|| raw::embedded_preview(path).ok()).flatten())
}

/// Loads and renders an image with its saved adjustments; also fills the preview cache.
fn prepare(path: &std::path::Path, session: Option<&std::path::Path>) -> Option<Prepared> {
    let adjustments = sidecar::load(path).adjustments;
    let (meta, preview) = raw::load_preview(path, PREVIEW_EDGE).map_err(|e| log::warn!("{}: {e:#}", path.display())).ok()?;
    // As it will look when opened (see `App::install`).
    let adjustments = sk_core::camera_look::apply_if_wanted(&meta, &preview, &adjustments, sk_core::camera_look::for_new()).unwrap_or(adjustments);
    let r = Processor::new(&meta, &adjustments).render(&preview, None, None);
    let image = ColorImage::from_rgba_unmultiplied([r.width, r.height], &r.rgba);
    if !cache::has_preview(path, &adjustments, session)
        && let Some(img) = ::image::RgbaImage::from_raw(r.width as u32, r.height as u32, r.rgba)
    {
        cache::store_preview(path, &adjustments, session, &::image::DynamicImage::ImageRgba8(img));
    }
    Some(Prepared { relocations: 0, meta: Arc::new(meta), preview: Arc::new(preview), adjustments, image, histogram: r.histogram })
}

/// Thumbnail: from the disk cache, else the embedded camera JPEG, or a real
/// development if the image has edits.
fn make_thumbnail(path: &std::path::Path, session: Option<&std::path::Path>) -> Option<ColorImage> {
    let sc = sidecar::load(path);
    if let Some(img) = cache::load(path, &sc.adjustments, session) {
        return Some(to_color_image(&img));
    }
    let img = if sc.adjustments != Adjustments::default() {
        let (meta, preview) = raw::load_preview(path, THUMB_EDGE as usize * 2).ok()?;
        let r = Processor::new(&meta, &sc.adjustments).render(&preview, None, None);
        ::image::DynamicImage::ImageRgba8(::image::RgbaImage::from_raw(r.width as u32, r.height as u32, r.rgba)?)
    } else {
        raw::embedded_preview(path).ok()?
    };
    let thumb = img.thumbnail(THUMB_EDGE, THUMB_EDGE);
    cache::store(path, &sc.adjustments, session, &thumb);
    Some(to_color_image(&thumb))
}

/// Sharpness map of an image at thumbnail size: analysed in a preview of the size the focus
/// mask is tuned for (thumbnails themselves are too small to tell sharp from soft), then reduced.
fn thumb_sharpness(path: &std::path::Path, session: Option<&std::path::Path>) -> Option<([usize; 2], Arc<Vec<u8>>)> {
    let edge = sk_core::focus::REFERENCE_EDGE as u32;
    let img = match quick_preview(path, session) {
        Some(img) => img,
        None => {
            let (meta, preview) = raw::load_preview(path, edge as usize).ok()?;
            let r = Processor::new(&meta, &sidecar::load(path).adjustments).render(&preview, None, None);
            ::image::DynamicImage::ImageRgba8(::image::RgbaImage::from_raw(r.width as u32, r.height as u32, r.rgba)?)
        }
    };
    let img = if img.width().max(img.height()) > edge { img.thumbnail(edge, edge) } else { img };
    let image = to_color_image(&img);
    let [w, h] = image.size;
    let sharpness = crate::focus::sharpness_of(&image, w.max(h));
    let (reduced, size) = sk_core::focus::reduce(&sharpness, w, h, THUMB_EDGE as usize);
    Some((size, Arc::new(reduced)))
}

pub fn to_color_image(img: &::image::DynamicImage) -> ColorImage {
    let rgba = img.to_rgba8();
    ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw())
}

/// How many decoded full resolutions the compare view keeps (each is large).
const DETAIL_FULLS: usize = 2;

fn render_detail(job: DetailJob, fulls: &mut Vec<(PathBuf, Arc<RawMeta>, Arc<LinearImage>)>) -> Option<Event> {
    let (live_full, adj, version) = match job.live {
        Some((meta, full, adj, version)) => (full.map(|f| (meta, f)), adj, Some(version)),
        None => (None, sidecar::load(&job.path).adjustments, None),
    };
    let (meta, full) = match live_full {
        Some(mf) => mf,
        None => {
            let pos = fulls.iter().position(|(p, ..)| *p == job.path);
            let entry = match pos {
                Some(i) => fulls.remove(i),
                None => match raw::load_full(&job.path) {
                    Ok((meta, full)) => (job.path.clone(), Arc::new(meta), Arc::new(full)),
                    Err(e) => {
                        log::warn!("Vergleich, volle Auflösung: {}: {e:#}", job.path.display());
                        return None;
                    }
                },
            };
            let mf = (entry.1.clone(), entry.2.clone());
            fulls.push(entry);
            if fulls.len() > DETAIL_FULLS {
                fulls.remove(0);
            }
            mf
        }
    };
    let p = Processor::new(&meta, &adj);
    let (fw, _) = p.output_size(full.width, full.height);
    let level = ((fw as f32 / job.width.max(1.0)).floor() as usize).max(1);
    let src = if level == 1 { full } else { Arc::new(full.box_downscaled(level)) };
    let (lw, lh) = p.output_size(src.width, src.height);
    let [u0, v0, u1, v1] = job.uv.map(|v| v.clamp(0.0, 1.0));
    let (x, y) = ((u0 * lw as f32) as usize, (v0 * lh as f32) as usize);
    let (x1, y1) = (((u1 * lw as f32).ceil() as usize).min(lw), ((v1 * lh as f32).ceil() as usize).min(lh));
    if x1 <= x || y1 <= y {
        return None;
    }
    let r = p.render(&src, Some(Region { x, y, w: x1 - x, h: y1 - y }), None);
    let image = ColorImage::from_rgba_unmultiplied([r.width, r.height], &r.rgba);
    let focus = job.focus.then(|| crate::focus::sharpness_of(&image, lw.max(lh)));
    let (lw, lh) = (lw as f32, lh as f32);
    Some(Event::CompareDetail {
        path: job.path,
        version,
        uv: [x as f32 / lw, y as f32 / lh, (x + r.width) as f32 / lw, (y + r.height) as f32 / lh],
        width: lw,
        full_res: level == 1,
        image,
        focus,
    })
}
