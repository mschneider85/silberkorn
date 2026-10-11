//! Background workers. The UI thread never decodes or processes images itself.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use cr_core::cache;
use cr_core::catalog::Catalog;
use cr_core::export::{self, ExportRecipe};
use cr_core::geometry::Region;
use cr_core::image::LinearImage;
use cr_core::params::Adjustments;
use cr_core::pipeline::{Histogram, Processor, RenderOptions, StageCache};
use cr_core::raw::{self, RawMeta};
use cr_core::sidecar;
use egui::ColorImage;

/// Long edge of the interactive preview (before the half-size demosaic shortcut kicks in).
pub const PREVIEW_EDGE: usize = 2048;
pub const THUMB_EDGE: u32 = 256;

pub enum Event {
    Loaded { generation: u64, result: Result<(Arc<RawMeta>, Arc<LinearImage>), String> },
    Rendered { generation: u64, image: ColorImage, thumb: Option<ColorImage>, histogram: Histogram, before: bool },
    Thumbnail { path: PathBuf, image: ColorImage },
    FullLoaded { generation: u64, image: Arc<LinearImage> },
    TileRendered { generation: u64, version: u64, region: Region, full_size: (usize, usize), image: ColorImage },
    ExportProgress { done: usize, total: usize, last: Result<PathBuf, String> },
    IndexProgress { done: usize, total: usize },
    LensDbDownloaded(Result<PathBuf, String>),
    IndexDone { indexed: usize, failed: usize, import: bool },
}

/// Adds files to the catalog. `import` marks them as a new import ("last import").
pub struct IndexJob {
    pub paths: Vec<PathBuf>,
    pub import: bool,
}

pub struct RenderJob {
    pub generation: u64,
    pub meta: Arc<RawMeta>,
    pub preview: Arc<LinearImage>,
    pub adjustments: Adjustments,
    pub clip_warning: bool,
    pub before: bool,
    /// Render the whole frame (crop tool active).
    pub ignore_crop: bool,
    pub mask_overlay: Option<usize>,
}

/// Full-resolution rendering of a region of the output (for zoomed-in viewing).
pub struct TileJob {
    pub generation: u64,
    pub version: u64,
    pub meta: Arc<RawMeta>,
    pub full: Arc<LinearImage>,
    pub adjustments: Adjustments,
    pub region: Region,
    pub clip_warning: bool,
    pub before: bool,
}

pub struct ExportJob {
    pub items: Vec<(PathBuf, Adjustments)>,
    pub recipe: ExportRecipe,
}

pub struct Workers {
    pub events: Receiver<Event>,
    ev_tx: Sender<Event>,
    ctx: egui::Context,
    load_tx: Sender<(u64, PathBuf)>,
    full_tx: Sender<(u64, PathBuf)>,
    tile_tx: Sender<TileJob>,
    render_tx: Sender<RenderJob>,
    thumb_tx: Sender<PathBuf>,
    export_tx: Sender<ExportJob>,
    index_tx: Sender<IndexJob>,
}

/// Receives the newest message, discarding older queued ones ("latest wins").
fn recv_latest<T>(rx: &Receiver<T>) -> Option<T> {
    let mut msg = rx.recv().ok()?;
    while let Ok(newer) = rx.try_recv() {
        msg = newer;
    }
    Some(msg)
}

impl Workers {
    pub fn spawn(ctx: egui::Context, catalog_path: PathBuf) -> Self {
        let (ev_tx, events) = channel();
        let (ev_tx_keep, ctx_keep) = (ev_tx.clone(), ctx.clone());

        let (index_tx, index_rx) = channel::<IndexJob>();
        {
            let (ev_tx, ctx) = (ev_tx.clone(), ctx.clone());
            thread::spawn(move || {
                // The indexer owns its own connection (SQLite in WAL mode allows concurrent readers).
                let catalog = match Catalog::open(&catalog_path) {
                    Ok(c) => c,
                    Err(e) => {
                        log::error!("Katalog nicht verfügbar: {e:#}");
                        return;
                    }
                };
                while let Ok(job) = index_rx.recv() {
                    let import_id = if job.import { catalog.begin_import().unwrap_or(0) } else { 0 };
                    let total = job.paths.len();
                    let (mut indexed, mut failed) = (0, 0);
                    for (i, path) in job.paths.iter().enumerate() {
                        let needs = job.import || catalog.needs_index(path).unwrap_or(true);
                        if needs {
                            match catalog.index_file(path, import_id) {
                                Ok(_) => indexed += 1,
                                Err(e) => {
                                    log::warn!("{}: {e:#}", path.display());
                                    failed += 1;
                                }
                            }
                        }
                        if i % 10 == 9 || i + 1 == total {
                            let _ = ev_tx.send(Event::IndexProgress { done: i + 1, total });
                            ctx.request_repaint();
                        }
                    }
                    let _ = ev_tx.send(Event::IndexDone { indexed, failed, import: job.import });
                    ctx.request_repaint();
                }
            });
        }

        let (load_tx, load_rx) = channel::<(u64, PathBuf)>();
        {
            let (ev_tx, ctx) = (ev_tx.clone(), ctx.clone());
            thread::spawn(move || {
                while let Some((generation, path)) = recv_latest(&load_rx) {
                    let result = raw::load_preview(&path, PREVIEW_EDGE)
                        .map(|(m, p)| (Arc::new(m), Arc::new(p)))
                        .map_err(|e| format!("{e:#}"));
                    let _ = ev_tx.send(Event::Loaded { generation, result });
                    ctx.request_repaint();
                }
            });
        }

        let (full_tx, full_rx) = channel::<(u64, PathBuf)>();
        {
            let (ev_tx, ctx) = (ev_tx.clone(), ctx.clone());
            thread::spawn(move || {
                while let Some((generation, path)) = recv_latest(&full_rx) {
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

        let (tile_tx, tile_rx) = channel::<TileJob>();
        {
            let (ev_tx, ctx) = (ev_tx.clone(), ctx.clone());
            thread::spawn(move || {
                let cache = StageCache::default();
                while let Some(job) = recv_latest(&tile_rx) {
                    let adj = if job.before {
                        let mut a = Adjustments::default();
                        a.keep_framing_from(&job.adjustments);
                        a
                    } else {
                        job.adjustments
                    };
                    let opts = RenderOptions { clip_warning: job.clip_warning, ..Default::default() };
                    let r = Processor::with_options(&job.meta, &adj, opts).render(&job.full, Some(job.region), Some(&cache));
                    let image = ColorImage::from_rgba_unmultiplied([r.width, r.height], &r.rgba);
                    let _ = ev_tx.send(Event::TileRendered {
                        generation: job.generation,
                        version: job.version,
                        region: job.region,
                        full_size: r.full_size,
                        image,
                    });
                    ctx.request_repaint();
                }
            });
        }

        let (render_tx, render_rx) = channel::<RenderJob>();
        {
            let (ev_tx, ctx) = (ev_tx.clone(), ctx.clone());
            thread::spawn(move || {
                let cache = StageCache::default();
                while let Some(job) = recv_latest(&render_rx) {
                    let adj = if job.before {
                        let mut a = Adjustments::default();
                        a.keep_framing_from(&job.adjustments);
                        a
                    } else {
                        job.adjustments
                    };
                    let opts = RenderOptions { clip_warning: job.clip_warning, ignore_crop: job.ignore_crop, mask_overlay: job.mask_overlay };
                    let r = Processor::with_options(&job.meta, &adj, opts).render(&job.preview, None, Some(&cache));
                    let image = ColorImage::from_rgba_unmultiplied([r.width, r.height], &r.rgba);
                    let thumb = (!job.before)
                        .then(|| ::image::RgbaImage::from_raw(r.width as u32, r.height as u32, r.rgba))
                        .flatten()
                        .map(|img| to_color_image(&::image::DynamicImage::ImageRgba8(img).thumbnail(THUMB_EDGE, THUMB_EDGE)));
                    let _ = ev_tx.send(Event::Rendered {
                        generation: job.generation,
                        image,
                        thumb,
                        histogram: r.histogram,
                        before: job.before,
                    });
                    ctx.request_repaint();
                }
            });
        }

        let (thumb_tx, thumb_rx) = channel::<PathBuf>();
        let thumb_rx = Arc::new(Mutex::new(thumb_rx));
        for _ in 0..thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 4)) {
            let (ev_tx, ctx, rx) = (ev_tx.clone(), ctx.clone(), thumb_rx.clone());
            thread::spawn(move || {
                loop {
                    let Ok(path) = rx.lock().unwrap().recv() else { break };
                    if let Some(image) = make_thumbnail(&path) {
                        let _ = ev_tx.send(Event::Thumbnail { path, image });
                        ctx.request_repaint();
                    }
                }
            });
        }

        let (export_tx, export_rx) = channel::<ExportJob>();
        {
            let (ev_tx, ctx) = (ev_tx, ctx);
            thread::spawn(move || {
                while let Ok(job) = export_rx.recv() {
                    let total = job.items.len();
                    for (i, (path, adj)) in job.items.into_iter().enumerate() {
                        let last = export::export(&path, &adj, &job.recipe).map_err(|e| format!("{}: {e:#}", path.display()));
                        let _ = ev_tx.send(Event::ExportProgress { done: i + 1, total, last });
                        ctx.request_repaint();
                    }
                }
            });
        }

        Self { events, ev_tx: ev_tx_keep, ctx: ctx_keep, load_tx, full_tx, tile_tx, render_tx, thumb_tx, export_tx, index_tx }
    }

    pub fn load(&self, generation: u64, path: PathBuf) {
        let _ = self.load_tx.send((generation, path));
    }

    pub fn load_full(&self, generation: u64, path: PathBuf) {
        let _ = self.full_tx.send((generation, path));
    }

    pub fn tile(&self, job: TileJob) {
        let _ = self.tile_tx.send(job);
    }

    pub fn render(&self, job: RenderJob) {
        let _ = self.render_tx.send(job);
    }

    pub fn thumbnail(&self, path: PathBuf) {
        let _ = self.thumb_tx.send(path);
    }

    pub fn export(&self, job: ExportJob) {
        let _ = self.export_tx.send(job);
    }

    /// Downloads the lensfun database in the background.
    pub fn download_lens_db(&self) {
        let (tx, ctx) = (self.ev_tx.clone(), self.ctx.clone());
        thread::spawn(move || {
            let r = cr_core::lensfun::download_database().map_err(|e| format!("{e:#}"));
            let _ = tx.send(Event::LensDbDownloaded(r));
            ctx.request_repaint();
        });
    }

    pub fn index(&self, job: IndexJob) {
        let _ = self.index_tx.send(job);
    }
}

/// Thumbnail: from the disk cache, else the embedded camera JPEG, or a real
/// development if the image has edits.
fn make_thumbnail(path: &std::path::Path) -> Option<ColorImage> {
    let sc = sidecar::load(path);
    if let Some(img) = cache::load(path, &sc.adjustments) {
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
    cache::store(path, &sc.adjustments, &thumb);
    Some(to_color_image(&thumb))
}

pub fn to_color_image(img: &::image::DynamicImage) -> ColorImage {
    let rgba = img.to_rgba8();
    ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw())
}
