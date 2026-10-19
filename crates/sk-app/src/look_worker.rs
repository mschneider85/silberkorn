//! The background side of "Look angleichen" (see `look`): measures the references (cached, and
//! measured again when a picture file or the settings of a reference image change), compares
//! them with the open image, and matches images to them.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::ColorImage;
use sk_core::image::LinearImage;
use sk_core::look_match::{LookOptions, LookStats, match_look, picture_thumbnail};
use sk_core::params::Adjustments;
use sk_core::pipeline::Processor;
use sk_core::raw::{self, RawMeta};
use sk_core::sidecar;

use crate::look::LookRef;
use crate::workers::{Activity, AutoItem, Event};

/// Long edge of the images loaded for matching (statistics need few pixels).
const LOAD_EDGE: usize = 640;
/// Long edge of a reference as measured.
const REFERENCE_EDGE: usize = 400;
/// Long edge of the thumbnails in the list.
const THUMB_EDGE: u32 = 128;

/// The open image as it is now (with unsaved changes).
pub type Open = (Arc<RawMeta>, Arc<LinearImage>, Adjustments);

pub enum LookRequest {
    /// Measures the references (if not yet) and sends their thumbnails.
    Prepare(Vec<LookRef>),
    /// How similar the open image is to each reference.
    Similarity { path: PathBuf, open: Box<Open>, refs: Vec<LookRef> },
    /// Gives `items` the look of `refs`.
    /// `cancel` stops before the next image (the finished ones stay).
    /// `run` comes back with each result (see `App::look_matched`).
    Match { run: u64, refs: Vec<LookRef>, items: Vec<AutoItem>, opts: LookOptions, cancel: Arc<std::sync::atomic::AtomicBool> },
}

/// What a match gives for one image.
pub struct LookResult {
    pub adjustments: Box<Adjustments>,
    /// Per reference used: how similar it was and its share of the look.
    pub refs: Vec<LookRef>,
    pub similarity: Vec<f32>,
    pub weights: Vec<f32>,
}

/// A measured reference and what it was measured from.
struct Measured {
    key: u64,
    stats: LookStats,
}

pub fn spawn(ev_tx: Sender<Event>, ctx: egui::Context, activity: Activity) -> Sender<LookRequest> {
    let (tx, rx) = channel::<LookRequest>();
    crate::guard::spawn("Look angleichen", move || run(&rx, &ev_tx, &ctx, &activity));
    tx
}

fn run(rx: &Receiver<LookRequest>, ev_tx: &Sender<Event>, ctx: &egui::Context, activity: &Activity) {
    // Loaded on first use (the subject model, if downloaded), kept for further pictures.
    let mut models = sk_ai::auto::AutoModels::default();
    let mut cache: HashMap<LookRef, Measured> = HashMap::new();
    while let Ok(first) = rx.recv() {
        // A burst of requests (browsing): only the last similarity counts.
        let mut batch = vec![first];
        batch.extend(rx.try_iter());
        let last_similarity = batch.iter().rposition(|r| matches!(r, LookRequest::Similarity { .. }));
        for (i, request) in batch.into_iter().enumerate() {
            activity.touch();
            let send = |e: Event| {
                let _ = ev_tx.send(e);
                ctx.request_repaint();
            };
            match request {
                LookRequest::Prepare(refs) => {
                    for r in refs {
                        let thumb = measure(&r, &mut cache, &mut models, true).and_then(|t| t.ok_or_else(|| "keine Vorschau".to_string()));
                        send(Event::LookRef { r, result: thumb });
                    }
                }
                LookRequest::Similarity { path, open, refs } => {
                    if Some(i) != last_similarity {
                        continue;
                    }
                    let values = crate::guard::catch(|| {
                        let target = LookStats::of_development(&open.0, &shown(&open.0, &open.1, open.2.clone()), &open.1, &mut |w, h, rgba| {
                            models.look_regions(w, h, rgba)
                        })?;
                        Some(refs.iter().map(|r| cache.get(r).map(|m| sk_core::look_match::similarity(&m.stats, &target))).collect::<Vec<_>>())
                    })
                    .ok()
                    .flatten();
                    if let Some(values) = values {
                        send(Event::LookSimilarity { path, refs, values });
                    }
                }
                LookRequest::Match { run, refs, items, opts, cancel } => {
                    // The references as they are now (a reference image may have been edited).
                    let mut usable = Vec::new();
                    let mut failed = Vec::new();
                    for r in &refs {
                        match measure(r, &mut cache, &mut models, false) {
                            Ok(thumb) => {
                                // Measured anew (edited since): the list shows it as it is now.
                                if let Some(t) = thumb {
                                    send(Event::LookRef { r: r.clone(), result: Ok(t) });
                                }
                                usable.push(r.clone());
                            }
                            Err(e) => failed.push(format!("{}: {e}", r.name())),
                        }
                    }
                    for item in items {
                        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                            break;
                        }
                        activity.touch();
                        let path = item.path.clone();
                        let result = if usable.is_empty() {
                            Err(if failed.is_empty() { "Keine Referenz".to_string() } else { failed.join("; ") })
                        } else {
                            let stats: Vec<LookStats> = usable.iter().filter_map(|r| cache.get(r).map(|m| m.stats.clone())).collect();
                            match_one(item, &stats, opts, &mut models).map(|m| LookResult {
                                adjustments: Box::new(m.adjustments),
                                refs: usable.clone(),
                                similarity: m.similarity,
                                weights: m.weights,
                            })
                        };
                        send(Event::LookMatched { run, path, opts, result });
                    }
                }
            }
        }
    }
}

/// As shown: with the camera's look, if the image gets it (see `camera_look`).
fn shown(meta: &RawMeta, preview: &LinearImage, adj: Adjustments) -> Adjustments {
    sk_core::camera_look::apply_if_wanted(meta, preview, &adj, sk_core::camera_look::for_new()).unwrap_or(adj)
}

fn match_one(item: AutoItem, refs: &[LookStats], opts: LookOptions, models: &mut sk_ai::auto::AutoModels) -> Result<sk_core::look_match::LookMatch, String> {
    crate::guard::catch(|| {
        let (meta, preview, adj) = match item.open {
            Some(open) => open,
            None => {
                let (meta, preview) = raw::load_preview(&item.path, LOAD_EDGE).map_err(|e| format!("{}: {e:#}", item.path.display()))?;
                (Arc::new(meta), Arc::new(preview), sidecar::load(&item.path).adjustments)
            }
        };
        let adj = shown(&meta, &preview, adj);
        match_look(refs, &meta, &adj, &preview, opts, &mut |w, h, rgba| models.look_regions(w, h, rgba)).ok_or_else(|| "Das Bild ist zu klein".to_string())
    })
    .and_then(|r| r)
}

/// What a reference is measured from: the file's time, and for an image of the library its
/// settings. Equal keys: the cached measures still hold.
fn key_of(r: &LookRef) -> Result<u64, String> {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).map_err(|_| "nicht gefunden".to_string());
    match r {
        LookRef::File(p) => modified(p)?.hash(&mut h),
        LookRef::Image(p) => {
            let (file, _) = sk_core::variant::split(p);
            modified(&file)?.hash(&mut h);
            serde_json::to_string(&sidecar::load(p).adjustments).unwrap_or_default().hash(&mut h);
        }
    }
    Ok(h.finish())
}

/// Measures `r` unless its measures are cached and still hold; with `thumb` (or when measured
/// anew), also its thumbnail.
fn measure(r: &LookRef, cache: &mut HashMap<LookRef, Measured>, models: &mut sk_ai::auto::AutoModels, thumb: bool) -> Result<Option<ColorImage>, String> {
    let key = key_of(r)?;
    if !thumb && cache.get(r).is_some_and(|m| m.key == key) {
        return Ok(None);
    }
    let rgba = crate::guard::catch(|| -> Result<image::RgbaImage, String> {
        match r {
            LookRef::File(p) => picture_thumbnail(p, REFERENCE_EDGE as u32).map_err(|e| format!("{e:#}")),
            LookRef::Image(p) => {
                let (meta, preview) = raw::load_preview(p, LOAD_EDGE).map_err(|e| format!("{e:#}"))?;
                let adj = shown(&meta, &preview, sidecar::load(p).adjustments);
                let small = preview.fit_within(REFERENCE_EDGE);
                let out = Processor::new(&meta, &adj).render_cpu(&small, None, None);
                image::RgbaImage::from_raw(out.width as u32, out.height as u32, out.rgba).ok_or_else(|| "Rendern fehlgeschlagen".to_string())
            }
        }
    })
    .and_then(|r| r)?;
    if cache.get(r).is_none_or(|m| m.key != key) {
        let (w, h) = (rgba.width() as usize, rgba.height() as usize);
        let regions = models.look_regions(w, h, rgba.as_raw());
        let stats = LookStats::from_rgba(w, h, rgba.as_raw(), &regions).ok_or_else(|| "Das Bild ist zu klein".to_string())?;
        cache.insert(r.clone(), Measured { key, stats });
    }
    let k = (THUMB_EDGE as f32 / rgba.width().max(rgba.height()) as f32).min(1.0);
    let small = image::imageops::thumbnail(&rgba, ((rgba.width() as f32 * k).round() as u32).max(1), ((rgba.height() as f32 * k).round() as u32).max(1));
    Ok(Some(ColorImage::from_rgba_unmultiplied([small.width() as usize, small.height() as usize], small.as_raw())))
}
