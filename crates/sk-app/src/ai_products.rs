//! Raw-Details and KI-Entrauschen (see `sk_core::ai_products`, `sk_ai::demosaic` and
//! `sk_ai::denoise`): the amounts are ordinary, undoable adjustments; the data of each image is
//! computed once in the background (the image in the viewer first, one computation at a time)
//! and kept in a cache. KI-Entrauschen downloads its models (GPL-3.0, credited in the licenses
//! dialog) the first time; Raw-Details' network comes with the program.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use egui::RichText;
use sk_core::ai_products::Products;
use sk_core::params::Detail;
use sk_core::raw::RawMeta;

use crate::ai_models::AiFeature;
use crate::app::App;

/// The kinds of AI data, in the order they are computed and blended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiProduct {
    Details,
    Denoise,
}

impl AiProduct {
    pub const ALL: [AiProduct; 2] = [AiProduct::Details, AiProduct::Denoise];

    pub fn products(self) -> &'static Products {
        match self {
            AiProduct::Details => &sk_core::ai_products::DETAILS,
            AiProduct::Denoise => &sk_core::ai_products::DENOISE,
        }
    }

    pub fn label(self) -> &'static str {
        self.products().label
    }

    pub fn amount(self, d: &Detail) -> f32 {
        match self {
            AiProduct::Details => d.raw_details,
            AiProduct::Denoise => d.ai_denoise,
        }
    }

    /// The model that is downloaded the first time (Raw-Details' comes with the program).
    fn feature(self) -> Option<AiFeature> {
        match self {
            AiProduct::Details => None,
            AiProduct::Denoise => Some(AiFeature::Denoise),
        }
    }

    /// The label of its slider.
    pub fn slider(self) -> &'static str {
        self.label()
    }

    /// Why it can't be used on an image with these settings (its slider is then shown, but disabled).
    pub fn unavailable(self, meta: &RawMeta, d: &Detail) -> Option<&'static str> {
        if meta.hdr {
            return Some("Für zusammengesetzte HDR-Bilder nicht verfügbar");
        }
        match self {
            AiProduct::Details if !meta.bayer => Some("Nur für Bayer-Sensoren (nicht für X-Trans und lineare DNGs)"),
            // (See `Detail::effective_raw_details`.)
            AiProduct::Details if d.ai_denoise >= 100.0 => Some("Bei 100 % KI-Entrauschen ohne Wirkung"),
            _ => None,
        }
    }
}

/// Images waiting for one kind of data.
#[derive(Default)]
pub struct ProductQueue {
    pending: VecDeque<PathBuf>,
    /// Job id and image being computed.
    running: Option<(u64, PathBuf)>,
    /// Why an image's data could not be computed (not tried again until the app restarts).
    failed: HashMap<PathBuf, String>,
    /// The viewer's image was checked with this amount being on (avoids a file check per frame).
    checked: Option<PathBuf>,
    /// Cancelled by the user: not started again until its amount changes or it is transferred.
    cancelled: HashSet<PathBuf>,
    /// Whether the viewer image's data exists (the panel asks every frame).
    ready: crate::depth::ReadyCache,
}

/// What the panel shows below the amount.
pub enum ProductState {
    NoModel,
    Downloading(f32),
    Waiting,
    Running(f32),
    Ready,
    Cancelled,
    Failed(String),
}

impl App {
    fn queue(&mut self, kind: AiProduct) -> &mut ProductQueue {
        &mut self.ai_products[kind as usize]
    }

    /// Queues the computation of an image's data (`first`: before all others).
    pub(crate) fn want_product(&mut self, kind: AiProduct, path: &Path, first: bool) {
        let q = self.queue(kind);
        if q.failed.contains_key(path) || q.cancelled.contains(path) || q.running.as_ref().is_some_and(|(_, p)| p == path) {
            return;
        }
        if let Some(i) = q.pending.iter().position(|p| p == path) {
            if !first {
                return;
            }
            q.pending.remove(i);
        }
        if kind.products().is_ready(path) {
            return;
        }
        let q = self.queue(kind);
        if first { q.pending.push_front(path.to_path_buf()) } else { q.pending.push_back(path.to_path_buf()) }
    }

    /// Each frame: queues the viewer's image where it wants data, starts the next computation.
    pub(crate) fn pump_products(&mut self) {
        for kind in AiProduct::ALL {
            let wanted = self
                .current
                .as_ref()
                .filter(|c| kind.amount(&c.adjustments.detail) > 0.0 && kind.unavailable(&c.meta, &c.adjustments.detail).is_none())
                .map(|c| c.meta.path.clone());
            if wanted != self.queue(kind).checked {
                self.queue(kind).checked = wanted.clone();
                if let Some(p) = wanted {
                    self.want_product(kind, &p, true);
                }
            }
        }
        if self.products_running() {
            return;
        }
        for kind in AiProduct::ALL {
            if self.queue(kind).pending.is_empty() || kind.feature().is_some_and(|f| !self.ensure_ai(f)) {
                continue;
            }
            let Some(path) = self.queue(kind).pending.pop_front() else { continue };
            let label = format!("{}: {}", kind.label(), path.file_name().unwrap_or_default().to_string_lossy());
            let file = path.clone();
            self.start_job(&label, Box::new(move |p| {
                kind.products().ensure(&file, p)?;
                Ok(Vec::new())
            }));
            let id = self.jobs.last().map_or(0, |j| j.id);
            self.queue(kind).running = Some((id, path));
            return;
        }
    }

    /// The model download was cancelled: like cancelling the computation of the images waiting.
    pub(crate) fn product_download_cancelled(&mut self, kind: AiProduct) {
        let q = self.queue(kind);
        q.cancelled.extend(q.pending.drain(..));
    }

    /// A computation finished (`true` if the job was one).
    pub(crate) fn product_job_done(&mut self, id: u64, result: &Result<Vec<PathBuf>, String>) -> bool {
        let Some(kind) = AiProduct::ALL.into_iter().find(|&k| self.ai_products[k as usize].running.as_ref().is_some_and(|(i, _)| *i == id)) else {
            return false;
        };
        let (_, path) = self.queue(kind).running.take().unwrap();
        self.jobs.retain(|j| j.id != id);
        self.queue(kind).ready.forget();
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        match result {
            Ok(_) => {
                self.status = format!("{}: {name} fertig", kind.label());
                // Renderings of the image so far were made without.
                self.thumb_requested.remove(&path);
                self.compare_textures.remove(&path);
                self.compare_requested.remove(&path);
                self.compare_details.remove(&path);
                self.forget_prepared(&path);
                if self.current_path().as_ref() == Some(&path) {
                    self.request_render();
                }
            }
            Err(e) => {
                self.report_error(format!("{} für {name} fehlgeschlagen: {e}", kind.label()));
                let q = self.queue(kind);
                if e != crate::jobs::CANCELLED {
                    q.failed.insert(path, e.clone());
                } else {
                    // Cancelled by the user: the rest of the queue too; this image stays so.
                    q.pending.clear();
                    q.cancelled.insert(path);
                }
            }
        }
        true
    }

    /// State of an image's data, for the panel.
    pub fn product_state(&self, kind: AiProduct, path: &Path) -> ProductState {
        if let Some(f) = kind.feature() {
            if let Some(p) = self.ai_download_shown(f) {
                return ProductState::Downloading(p);
            }
            if !f.available() {
                return ProductState::NoModel;
            }
        }
        let q = &self.ai_products[kind as usize];
        if let Some(e) = q.failed.get(path) {
            return ProductState::Failed(e.clone());
        }
        if q.cancelled.contains(path) {
            return ProductState::Cancelled;
        }
        if let Some((id, p)) = &q.running
            && p == path
        {
            return ProductState::Running(self.jobs.iter().find(|j| j.id == *id).map_or(0.0, |j| j.progress));
        }
        if q.pending.contains(&path.to_path_buf()) {
            return ProductState::Waiting;
        }
        if q.ready.get(path, |p| kind.products().is_ready(p)) { ProductState::Ready } else { ProductState::Waiting }
    }

    /// Queues the data of `files` whose amount is on (after the amount was carried over to them).
    pub(crate) fn want_products_of(&mut self, kind: AiProduct, files: &[PathBuf], amount: f32) {
        if amount > 0.0 {
            for f in files {
                self.queue(kind).cancelled.remove(f);
                self.want_product(kind, f, false);
            }
        }
    }

    /// The slider of an amount in `d` and the state of its computation; returns whether it changed.
    pub(crate) fn product_controls(&mut self, ui: &mut egui::Ui, kind: AiProduct, d: &mut Detail, meta: &RawMeta) -> bool {
        let path = meta.path.as_path();
        let unavailable = kind.unavailable(meta, d);
        let amount = match kind {
            AiProduct::Details => &mut d.raw_details,
            AiProduct::Denoise => &mut d.ai_denoise,
        };
        let before = *amount;
        // Where it can't be used, the slider is shown, but disabled.
        let changed = ui.add_enabled_ui(unavailable.is_none(), |ui| crate::widgets::slider_row(ui, kind.slider(), amount, 0.0..=100.0, 0.0, 0)).inner;
        if changed && self.queue(kind).cancelled.remove(path) {
            // Queued again by `pump_products`.
            self.queue(kind).checked = None;
        }
        let faint = |t: String| RichText::new(t).small().color(crate::theme::TEXT_FAINT);
        if let Some(why) = unavailable {
            ui.label(faint(why.into()));
            return changed;
        }
        match self.product_state(kind, path) {
            ProductState::NoModel => {
                let f = kind.feature().expect("only for downloaded models");
                if changed && before == 0.0 && *amount > 0.0 {
                    self.ensure_ai(f);
                } else {
                    ui.label(faint(format!("Lädt beim ersten Einschalten ein KI-Modell ({} MB)", f.download_mb())));
                }
            }
            ProductState::Downloading(p) => {
                let f = kind.feature().expect("only for downloaded models");
                crate::ai_models::download_progress(ui, f, p, self.job_cancel(None, Some(&f.job_label())));
            }
            _ if *amount <= 0.0 => {}
            ProductState::Waiting => {
                ui.label(faint("Berechne gleich …".into()));
            }
            ProductState::Running(p) => {
                ui.add(egui::ProgressBar::new(p).desired_height(6.0));
                ui.horizontal(|ui| {
                    ui.label(faint(format!("Berechne … {:.0} %", p * 100.0)));
                    if let Some(c) = self.job_cancel(self.ai_products[kind as usize].running.as_ref().map(|r| r.0), None) {
                        crate::widgets::cancel_button(ui, &c);
                    }
                });
            }
            ProductState::Ready => {}
            ProductState::Cancelled => {
                ui.label(faint("Abgebrochen – zum Berechnen den Regler ändern".into()));
            }
            ProductState::Failed(e) => {
                ui.label(RichText::new(format!("Fehlgeschlagen: {e}")).small().color(crate::theme::DANGER));
            }
        }
        changed
    }

    /// Whether any data is being computed.
    pub(crate) fn products_running(&self) -> bool {
        self.ai_products.iter().any(|q| q.running.is_some())
    }

    /// Whether data of `kind` is being computed (its cache can't be emptied meanwhile).
    pub(crate) fn product_running(&self, kind: AiProduct) -> bool {
        self.ai_products[kind as usize].running.is_some()
    }

    /// Empties the cache of `kind`; images still set to use it compute again when shown.
    pub(crate) fn clear_product_cache(&mut self, kind: AiProduct) -> anyhow::Result<()> {
        kind.products().clear_cache()?;
        self.queue(kind).checked = None;
        self.request_render();
        Ok(())
    }
}

/// Title of the section in the Details panel (the tool's name).
pub const ENHANCE_SECTION: &str = "Verbessern";

impl App {
    /// Command: shows the "Verbessern" section of the Details panel.
    pub fn open_enhance(&mut self, ctx: &egui::Context) {
        self.tab = crate::app::Tab::Details;
        crate::widgets::reveal_section(ctx, ENHANCE_SECTION);
    }

    /// The "Verbessern" section of the Details panel: Raw-Details and KI-Entrauschen (carried over
    /// to the other selected images with the button in its header, see [`Self::transfer_enhance`]);
    /// returns whether something changed.
    pub(crate) fn enhance_section(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(cur) = self.current.as_ref() else { return false };
        let meta = cur.meta.clone();
        let mut d = cur.adjustments.detail.clone();
        let (mut reset, mut changed) = (false, false);
        let help = "Neuronale Netze verbessern die RAW-Daten, einmal im Hintergrund berechnet; das Original bleibt unverändert.\n\n\
                    Raw-Details: setzt die Farben neu zusammen (Demosaicing) – feine Details mit weniger Farbsäumen, Treppen und Moiré, \
                    ohne zu glätten. Für rauscharme Bilder.\n\n\
                    KI-Entrauschen: entfernt das Rauschen und setzt dabei die Farben selbst neu zusammen; bei 100 % ist Raw-Details darum ohne Wirkung.";
        crate::widgets::tool_section_help(ui, sk_core::tools::Tool::Enhance, help, Some(&mut || reset = true), |ui| {
            for kind in AiProduct::ALL {
                changed |= self.product_controls(ui, kind, &mut d, &meta);
            }
        });
        if reset {
            (d.raw_details, d.ai_denoise) = (0.0, 0.0);
            changed = true;
        }
        if let Some(cur) = self.current.as_mut() {
            let c = &mut cur.adjustments.detail;
            (c.raw_details, c.ai_denoise) = (d.raw_details, d.ai_denoise);
        }
        changed
    }

    /// Sets Raw-Details and KI-Entrauschen on the selected images and queues their computation.
    pub(crate) fn transfer_enhance(&mut self, (details, denoise): (f32, f32)) {
        self.commit_and_save();
        let files = self.selected_paths();
        self.set_adjustments_on(&files, |a| {
            let mut a = a.clone();
            (a.detail.raw_details, a.detail.ai_denoise) = (details, denoise);
            a
        });
        self.commit_and_save();
        self.want_products_of(AiProduct::Details, &files, if denoise >= 100.0 { 0.0 } else { details });
        self.want_products_of(AiProduct::Denoise, &files, denoise);
        self.status = format!("„{ENHANCE_SECTION}“ übertragen: {}", crate::widgets::image_count(files.len()));
    }
}
