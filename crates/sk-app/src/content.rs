//! Content search in the library ("Bildinhalt", "Ähnliche finden"): the shown images are
//! analysed once by the image half of SigLIP 2 (see `sk_ai::content`), in the background and
//! only when a search needs them; their embeddings are kept in the catalog or session. A
//! search then filters and orders the view by similarity to its text or image.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sk_ai::content::{ImageEncoder, Search, TextEncoder};
use sk_core::catalog::Catalog;

use crate::ai_models::AiFeature;
use crate::app::App;

/// Pause after typing before the text is encoded (each change would cost a run).
const TYPING_PAUSE: Duration = Duration::from_millis(400);

/// What the view is searched for.
#[derive(Clone, Debug, PartialEq)]
pub enum Query {
    Text(String),
    /// Images like this one (a raw file or variant).
    Similar(PathBuf),
}

#[derive(Default)]
pub struct ContentSearch {
    /// The text of the "Bildinhalt" field.
    pub text: String,
    /// "Ähnliche finden": the image searched for (the text is not used then).
    pub similar: Option<PathBuf>,
    /// The embedding of the query, once known.
    target: Option<(Query, Vec<f32>)>,
    /// A text being encoded.
    encoding: Option<(Query, Receiver<anyhow::Result<Vec<f32>>>)>,
    /// A text waiting for its model.
    waiting: Option<Query>,
    /// A text whose model could not be loaded: not tried again before the user types or
    /// presses Enter (the download reported its error already).
    gave_up: Option<Query>,
    /// When the text last changed.
    typed_at: Option<Instant>,
    /// Embeddings of the raw files shown (by file, not variant).
    vectors: HashMap<PathBuf, Vec<f32>>,
    /// Files looked up in the catalog already (found or not).
    looked_up: HashSet<PathBuf>,
    /// Files analysed in this session without result (unreadable): not tried again.
    failed: HashSet<PathBuf>,
    /// The analysis was cancelled: not started again before the next search.
    cancelled: bool,
    /// Shown files without an embedding (analysis cancelled, or unreadable): not among the hits.
    missing: usize,
    /// Embeddings the running analysis computed since the last frame.
    incoming: Incoming,
    /// The analysis running: job id and the files it computes.
    job: Option<(u64, HashSet<PathBuf>)>,
    /// The text half of the model, loaded with the first search and kept.
    encoder: Arc<Mutex<Option<TextEncoder>>>,
    /// The image half, loaded once for the analysis and single images.
    image_encoder: SharedImageEncoder,
    /// Single images computed (or being computed) outside an analysis, for keyword suggestions.
    single: HashSet<PathBuf>,
}

type SharedImageEncoder = Arc<Mutex<Option<Arc<ImageEncoder>>>>;
/// Embeddings computed in the background, by raw file, until the next frame takes them.
type Incoming = Arc<Mutex<Vec<(PathBuf, Vec<f32>)>>>;

/// The image encoder of `slot`, loaded (and downloaded, with `progress`) on first use.
fn image_encoder(slot: &SharedImageEncoder, progress: &dyn Fn(f32)) -> anyhow::Result<Arc<ImageEncoder>> {
    let mut slot = slot.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(e) = &*slot {
        return Ok(e.clone());
    }
    if !ImageEncoder::available() {
        sk_ai::models::download(&sk_ai::models::CONTENT_IMAGE, progress)?;
    }
    Ok(slot.insert(Arc::new(ImageEncoder::load()?)).clone())
}

impl ContentSearch {
    /// The search in effect, if any.
    pub fn query(&self) -> Option<Query> {
        match &self.similar {
            Some(p) => Some(Query::Similar(p.clone())),
            None => Some(self.text.trim()).filter(|t| !t.is_empty()).map(|t| Query::Text(t.to_string())),
        }
    }

    pub fn is_active(&self) -> bool {
        self.query().is_some()
    }

    /// Images still to be analysed for the search, if they are being analysed.
    pub fn analysing(&self) -> Option<usize> {
        self.job.as_ref().map(|(_, files)| files.iter().filter(|f| !self.vectors.contains_key(*f)).count())
    }

    /// Waiting for the query's embedding (text being encoded or its model loaded).
    pub fn preparing(&self) -> bool {
        self.encoding.is_some() || self.waiting.is_some()
    }

    /// The text field changed.
    pub fn typed(&mut self) {
        self.typed_at = Some(Instant::now());
        self.cancelled = false;
        self.gave_up = None;
    }

    /// Enter in the text field: searched at once.
    pub fn submit(&mut self) {
        self.typed_at = Some(Instant::now() - TYPING_PAUSE);
        self.cancelled = false;
        self.gave_up = None;
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.similar = None;
    }
}

/// Images analysed per round of the job (read in parallel; the view updates after each).
const CHUNK: usize = 8;
/// Part of the analysis's progress that loading its missing model takes.
const MODEL_SHARE: f32 = 0.5;

impl App {
    /// "Ähnliche finden": shows the images like `path`, most similar first.
    pub fn find_similar(&mut self, path: PathBuf) {
        self.content.similar = Some(path);
        self.content.cancelled = false;
        self.tab = crate::app::Tab::Library;
        self.update_content_search();
    }

    /// Ends the content search (all images again).
    pub fn clear_content_search(&mut self) {
        self.content.clear();
        self.update_content_search();
    }

    /// Takes new embeddings and an encoded text, and starts what the search needs; every frame.
    pub(crate) fn pump_content(&mut self, ctx: &egui::Context) {
        let mut changed = false;
        let new: Vec<_> = std::mem::take(&mut *self.content.incoming.lock().unwrap_or_else(|e| e.into_inner()));
        if !new.is_empty() {
            self.content.vectors.extend(new);
            changed = true;
        }
        if let Some((query, rx)) = &self.content.encoding
            && let Ok(result) = rx.try_recv()
        {
            let query = query.clone();
            self.content.encoding = None;
            match result {
                Ok(v) => {
                    self.content.target = Some((query, v));
                    changed = true;
                }
                Err(e) => {
                    log::warn!("Inhaltssuche: {e:#}");
                    self.report_error(format!("Inhaltssuche fehlgeschlagen: {e:#}"));
                    self.content.text.clear();
                }
            }
        }
        if let Some(at) = self.content.typed_at {
            if at.elapsed() >= TYPING_PAUSE {
                self.content.typed_at = None;
                self.update_content_search();
            } else {
                ctx.request_repaint_after(TYPING_PAUSE - at.elapsed());
            }
        }
        // The model download failed or was cancelled: what waited for it was dropped.
        if self.content.waiting.is_some() && self.ai_download_progress(AiFeature::Content).is_none() {
            self.content.gave_up = self.content.waiting.take();
        }
        if changed {
            self.update_content_search();
        }
    }

    /// Brings the search up to date with its query: the query's embedding, the images'
    /// (looked up, or analysed in the background), and the view.
    pub(crate) fn update_content_search(&mut self) {
        let query = self.content.query();
        if query.is_none() {
            self.content.target = None;
            self.content.waiting = None;
            self.refilter();
            return;
        }
        self.ensure_content_vectors();
        match query {
            Some(Query::Similar(path)) => {
                let file = sk_core::variant::file_of(&path);
                let known = self.content.vectors.get(&file).cloned();
                if let Some(v) = known {
                    self.content.target = Some((Query::Similar(path), v));
                } else if self.content.failed.contains(&file) {
                    self.report_error(format!("Ähnliche finden: {} ließ sich nicht lesen", sk_core::variant::display_name(&path)));
                    self.clear_content_search();
                    return;
                }
            }
            Some(q @ Query::Text(_)) => {
                let current = self.content.target.as_ref().is_some_and(|(t, _)| *t == q);
                let underway = self.content.encoding.as_ref().is_some_and(|(t, _)| *t == q) || self.content.waiting.as_ref() == Some(&q);
                if !current && !underway && self.content.typed_at.is_none() && self.content.gave_up.as_ref() != Some(&q) {
                    self.content.waiting = Some(q);
                    self.with_ai(AiFeature::Content, |app| app.encode_query());
                    // The download failed a moment ago (`with_ai` reported it).
                    if self.content.waiting.is_some() && self.ai_download_progress(AiFeature::Content).is_none() {
                        self.content.gave_up = self.content.waiting.take();
                    }
                }
            }
            None => {}
        }
        self.refilter();
    }

    /// Encodes the text waiting for it (in the background).
    fn encode_query(&mut self) {
        let Some(Query::Text(text)) = self.content.waiting.take() else { return };
        let query = Query::Text(text.clone());
        let (tx, rx) = channel();
        let encoder = self.content.encoder.clone();
        let ctx = self.workers.context();
        std::thread::spawn(move || {
            let result = (|| {
                let mut encoder = encoder.lock().unwrap_or_else(|e| e.into_inner());
                if encoder.is_none() {
                    *encoder = Some(TextEncoder::load()?);
                }
                encoder.as_ref().expect("loaded").embed(&text)
            })();
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.content.encoding = Some((query, rx));
    }

    /// Looks up the embeddings of the shown files (and of the image searched for) not looked up
    /// yet, and starts analysing those without one (unless an analysis runs; the next one
    /// follows when it is done).
    fn ensure_content_vectors(&mut self) {
        let mut files: Vec<PathBuf> = sk_core::variant::files_of(&self.entries.iter().map(|e| e.path.clone()).collect::<Vec<_>>());
        // Searched for first: the search can start with it (it may be of another folder).
        if let Some(Query::Similar(p)) = self.content.query() {
            let file = sk_core::variant::file_of(&p);
            files.retain(|f| *f != file);
            files.insert(0, file);
        }
        let new: Vec<PathBuf> = files.iter().filter(|f| !self.content.looked_up.contains(*f)).cloned().collect();
        if !new.is_empty() {
            if let Some(c) = &self.catalog {
                match c.contents(&new, &sk_ai::content::tag()) {
                    Ok(found) => self.content.vectors.extend(found),
                    Err(e) => log::warn!("Inhaltssuche: {e:#}"),
                }
            }
            self.content.looked_up.extend(new);
        }
        self.content.missing = files.iter().filter(|f| !self.content.vectors.contains_key(*f)).count();
        if self.content.job.is_some() || self.content.cancelled {
            return;
        }
        let todo: Vec<PathBuf> = files.into_iter().filter(|f| !self.content.vectors.contains_key(f) && !self.content.failed.contains(f)).collect();
        if !todo.is_empty() {
            self.start_content_analysis(todo);
        }
    }

    fn start_content_analysis(&mut self, todo: Vec<PathBuf>) {
        let catalog = self.catalog_path();
        let incoming = self.content.incoming.clone();
        let slot = self.content.image_encoder.clone();
        let label = format!("Bildinhalt erfassen ({})", crate::widgets::image_count(todo.len()));
        let files: HashSet<PathBuf> = todo.iter().cloned().collect();
        self.start_job(&label, Box::new(move |progress| {
            // A missing model takes the first part of the progress (and can be cancelled).
            let share = if ImageEncoder::available() { 0.0 } else { MODEL_SHARE };
            let encoder = image_encoder(&slot, &|p| progress(p * share))?;
            let db = Catalog::open(&catalog)?;
            let tag = sk_ai::content::tag();
            use rayon::prelude::*;
            let mut done = 0;
            for chunk in todo.chunks(CHUNK) {
                for path in chunk {
                    if db.needs_index(path).unwrap_or(true) {
                        let _ = db.index_file(path, 0);
                    }
                }
                let images: Vec<_> = chunk.par_iter().map(|p| crate::culling::analysis_image(p).map(|i| i.to_rgb8())).collect();
                let embed = |img: &anyhow::Result<::image::RgbImage>| img.as_ref().map_err(|e| anyhow::anyhow!("{e:#}")).and_then(|img| encoder.embed(img));
                let results: Vec<_> = if encoder.prefers_serial() { images.iter().map(embed).collect() } else { images.par_iter().map(embed).collect() };
                let mut found = Vec::new();
                for (path, result) in chunk.iter().zip(results) {
                    match result {
                        Ok(v) => {
                            // Not kept for the next session then, but the search has it.
                            if let Err(e) = db.store_content(path, &tag, &v) {
                                log::warn!("Bildinhalt {} nicht gespeichert: {e:#}", path.display());
                            }
                            found.push((path.clone(), v));
                        }
                        Err(e) => log::warn!("Bildinhalt {}: {e:#}", path.display()),
                    }
                }
                incoming.lock().unwrap_or_else(|e| e.into_inner()).extend(found);
                done += chunk.len();
                progress(share + (1.0 - share) * done as f32 / todo.len() as f32);
            }
            Ok(Vec::new())
        }));
        let id = self.jobs.last().map(|j| j.id).expect("started");
        self.content.job = Some((id, files));
    }

    /// The embedding of the image `path`, if known; else looks it up or computes it in the
    /// background (once per session), so that a later frame has it.
    pub(crate) fn content_vector(&mut self, path: &std::path::Path) -> Option<Vec<f32>> {
        let file = sk_core::variant::file_of(path);
        if let Some(v) = self.content.vectors.get(&file) {
            return Some(v.clone());
        }
        if self.content.looked_up.insert(file.clone())
            && let Some(v) = self.catalog.as_ref().and_then(|c| c.contents(std::slice::from_ref(&file), &sk_ai::content::tag()).ok()).and_then(|mut f| f.remove(&file))
        {
            self.content.vectors.insert(file, v.clone());
            return Some(v);
        }
        let analysing = self.content.job.as_ref().is_some_and(|(_, files)| files.contains(&file));
        if analysing || !ImageEncoder::available() || !self.content.single.insert(file.clone()) {
            return None;
        }
        let (catalog, slot, incoming, ctx) = (self.catalog_path(), self.content.image_encoder.clone(), self.content.incoming.clone(), self.workers.context());
        std::thread::spawn(move || {
            let result = (|| {
                let img = crate::culling::analysis_image(&file)?.to_rgb8();
                let v = image_encoder(&slot, &|_| {})?.embed(&img)?;
                let db = Catalog::open(&catalog)?;
                if db.needs_index(&file).unwrap_or(true) {
                    db.index_file(&file, 0)?;
                }
                db.store_content(&file, &sk_ai::content::tag(), &v)?;
                anyhow::Ok(v)
            })();
            match result {
                Ok(v) => incoming.lock().unwrap_or_else(|e| e.into_inner()).push((file, v)),
                Err(e) => log::warn!("Bildinhalt {}: {e:#}", file.display()),
            }
            ctx.request_repaint();
        });
        None
    }

    /// Keywords suggested for the open image from what it shows (one click adds one); they
    /// appear once the image is analysed.
    pub(crate) fn keyword_suggestions_ui(&mut self, ui: &mut egui::Ui) {
        let (Some(path), Some(keywords)) = (self.current_path(), sk_ai::content::Keywords::shipped()) else { return };
        let Some(v) = self.content_vector(&path) else { return };
        let has = self.current.as_ref().map(|c| c.keywords.clone()).unwrap_or_default();
        let suggested = keywords.suggest(&v, &has, 8);
        if suggested.is_empty() {
            return;
        }
        ui.label(crate::widgets::ai_text(ui, "Vorschläge")).on_hover_text(crate::widgets::AI_HINT);
        let mut add = None;
        ui.horizontal_wrapped(|ui| {
            for k in suggested {
                if ui.add(egui::Button::new(format!("{}  {k}", crate::theme::icon::PLUS)).small()).on_hover_text("Hinzufügen").clicked() {
                    add = Some(k);
                }
            }
        });
        if let Some(k) = add {
            self.add_keyword(k);
        }
    }

    /// The analysis finished (`true` if the job was it): what it computed is in the view
    /// already; images added meanwhile come next.
    pub(crate) fn content_job_done(&mut self, id: u64, label: &str, result: &Result<Vec<PathBuf>, String>) -> bool {
        if self.content.job.as_ref().is_none_or(|(j, _)| *j != id) {
            return false;
        }
        self.jobs.retain(|j| j.id != id);
        let (_, files) = self.content.job.take().expect("checked");
        let new: Vec<_> = std::mem::take(&mut *self.content.incoming.lock().unwrap_or_else(|e| e.into_inner()));
        self.content.vectors.extend(new);
        match result {
            Ok(_) => {
                self.status = format!("{label}: fertig");
                // Those without result could not be read.
                self.content.failed.extend(files.into_iter().filter(|f| !self.content.vectors.contains_key(f)));
            }
            Err(e) => {
                self.content.cancelled = true;
                if e == crate::jobs::CANCELLED {
                    self.status = format!("{label} abgebrochen");
                } else {
                    self.report_error(format!("{label} fehlgeschlagen: {e}"));
                }
            }
        }
        if self.content.is_active() {
            self.update_content_search();
        }
        true
    }

    /// The "Bildinhalt" field (or the image searched for) and what the search is doing.
    pub(crate) fn content_search_ui(&mut self, ui: &mut egui::Ui) {
        use crate::theme::icon;
        if let Some(p) = self.content.similar.clone() {
            let mut clear = false;
            ui.horizontal(|ui| {
                let text = crate::widgets::ai_text(ui, &format!("Ähnlich wie {}", sk_core::variant::display_name(&p)));
                ui.label(text).on_hover_text(crate::widgets::AI_HINT);
                clear = ui.button(icon::X).on_hover_text("Ähnlichkeitssuche beenden").clicked();
            });
            if clear {
                self.clear_content_search();
            }
        } else {
            let resp = ui
                .add(egui::TextEdit::singleline(&mut self.content.text).hint_text(format!("{}  Bildinhalt", icon::SPARKLE)))
                .on_hover_text("Sucht nach dem, was auf den Bildern zu sehen ist (KI, in jeder Sprache). Die Treffer stehen nach Relevanz geordnet.");
            if resp.changed() {
                self.content.typed();
            }
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.content.submit();
            }
        }
        if let Some(p) = self.ai_download_shown(AiFeature::Content) {
            crate::ai_models::download_progress(ui, AiFeature::Content, p, None);
        } else if self.content.preparing() {
            crate::widgets::hint(ui, "Suche wird vorbereitet …");
        }
        if let Some(n) = self.content.analysing().filter(|&n| n > 0) {
            let text = if ImageEncoder::available() {
                format!("Bildinhalt wird erfasst, noch {} …", crate::widgets::image_count(n))
            } else {
                format!("Lade KI-Modell ({} MB, einmalig) …", sk_ai::models::CONTENT_IMAGE.bytes.div_ceil(1_000_000))
            };
            crate::widgets::hint(ui, text);
        } else if self.content.is_active() && self.content.missing > 0 {
            let mut resume = false;
            ui.horizontal_wrapped(|ui| {
                crate::widgets::hint(ui, format!("{} ohne erfassten Bildinhalt, nicht unter den Treffern", crate::widgets::image_count(self.content.missing)));
                resume = self.content.cancelled && ui.small_button("Erfassen").on_hover_text("Den Bildinhalt der übrigen Bilder erfassen").clicked();
            });
            if resume {
                self.content.cancelled = false;
                self.update_content_search();
            }
        }
    }

    /// The entries in the order of the content search (indices into `entries`, from those that
    /// pass the filter), if a search is in effect and its query known.
    pub(crate) fn content_order(&self, matching: &[usize]) -> Option<Vec<usize>> {
        let query = self.content.query()?;
        let (target_query, target) = self.content.target.as_ref()?;
        if *target_query != query {
            return None;
        }
        let search = if matches!(query, Query::Similar(_)) { Search::Image } else { Search::Text };
        let scored: Vec<(usize, f32)> = matching
            .iter()
            .filter_map(|&i| {
                let v = self.content.vectors.get(&sk_core::variant::file_of(&self.entries[i].path))?;
                Some((i, sk_ai::content::similarity(v, target)))
            })
            .collect();
        Some(sk_ai::content::ranked(scored, search))
    }
}
