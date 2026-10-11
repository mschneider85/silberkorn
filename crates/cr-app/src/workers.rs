//! Background workers. The UI thread never decodes or processes images itself.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use cr_core::export::{self, ExportRecipe};
use cr_core::image::LinearImage;
use cr_core::params::Adjustments;
use cr_core::pipeline::{Histogram, Processor};
use cr_core::raw::{self, RawMeta};
use cr_core::sidecar;
use egui::ColorImage;

/// Long edge of the interactive preview (before the half-size demosaic shortcut kicks in).
pub const PREVIEW_EDGE: usize = 3000;
pub const THUMB_EDGE: u32 = 256;

pub enum Event {
    Loaded { generation: u64, result: Result<(Arc<RawMeta>, Arc<LinearImage>), String> },
    Rendered { generation: u64, image: ColorImage, thumb: Option<ColorImage>, histogram: Histogram, before: bool },
    Thumbnail { path: PathBuf, image: ColorImage },
    ExportProgress { done: usize, total: usize, last: Result<PathBuf, String> },
}

pub struct RenderJob {
    pub generation: u64,
    pub meta: Arc<RawMeta>,
    pub preview: Arc<LinearImage>,
    pub adjustments: Adjustments,
    pub clip_warning: bool,
    pub before: bool,
}

pub struct ExportJob {
    pub items: Vec<(PathBuf, Adjustments)>,
    pub recipe: ExportRecipe,
}

pub struct Workers {
    pub events: Receiver<Event>,
    load_tx: Sender<(u64, PathBuf)>,
    render_tx: Sender<RenderJob>,
    thumb_tx: Sender<PathBuf>,
    export_tx: Sender<ExportJob>,
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
    pub fn spawn(ctx: egui::Context) -> Self {
        let (ev_tx, events) = channel();

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

        let (render_tx, render_rx) = channel::<RenderJob>();
        {
            let (ev_tx, ctx) = (ev_tx.clone(), ctx.clone());
            thread::spawn(move || {
                while let Some(job) = recv_latest(&render_rx) {
                    let adj = if job.before {
                        Adjustments { rotation: job.adjustments.rotation, ..Default::default() }
                    } else {
                        job.adjustments
                    };
                    let r = Processor::new(&job.meta, &adj).render(&job.preview, job.clip_warning);
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

        Self { events, load_tx, render_tx, thumb_tx, export_tx }
    }

    pub fn load(&self, generation: u64, path: PathBuf) {
        let _ = self.load_tx.send((generation, path));
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
}

/// Thumbnail: the embedded camera JPEG, or a real development if the image has edits.
fn make_thumbnail(path: &std::path::Path) -> Option<ColorImage> {
    let sc = sidecar::load(path);
    let img = if sc.adjustments != Adjustments::default() {
        let (meta, preview) = raw::load_preview(path, THUMB_EDGE as usize * 2).ok()?;
        let r = Processor::new(&meta, &sc.adjustments).render(&preview, false);
        ::image::DynamicImage::ImageRgba8(::image::RgbaImage::from_raw(r.width as u32, r.height as u32, r.rgba)?)
    } else {
        raw::embedded_preview(path).ok()?
    };
    Some(to_color_image(&img.thumbnail(THUMB_EDGE, THUMB_EDGE)))
}

pub fn to_color_image(img: &::image::DynamicImage) -> ColorImage {
    let rgba = img.to_rgba8();
    ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw())
}
