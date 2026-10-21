//! KI-Entrauschen and Verwacklung entfernen (see `sk_core::ai_product`, `sk_ai::denoise` and
//! `sk_ai::deblur`): the amount is an ordinary, undoable adjustment; what the network makes of
//! each image is computed once in the background (the image in the viewer first) and kept in a
//! cache. Switching it on downloads the model the first time (credited in the licenses dialog).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use egui::RichText;
use sk_core::params::Detail;

use crate::ai_models::AiFeature;
use crate::app::App;
use crate::widgets::{image_count, slider_row};

/// Title of the section in the Details panel.
pub const NOISE_SECTION: &str = "Rauschreduzierung";

/// A network result kept per image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiProduct {
    Denoise,
    Deblur,
}

impl AiProduct {
    const ALL: [AiProduct; 2] = [Self::Denoise, Self::Deblur];

    /// Name of the function (slider, jobs, messages).
    pub fn label(self) -> &'static str {
        match self {
            Self::Denoise => "KI-Entrauschen",
            Self::Deblur => "Verwacklung entfernen",
        }
    }

    fn feature(self) -> AiFeature {
        match self {
            Self::Denoise => AiFeature::Denoise,
            Self::Deblur => AiFeature::Deblur,
        }
    }

    fn amount(self, d: &Detail) -> f32 {
        match self {
            Self::Denoise => d.ai_denoise,
            Self::Deblur => d.ai_deblur,
        }
    }

    fn is_ready(self, path: &Path) -> bool {
        match self {
            Self::Denoise => sk_core::ai_denoise::is_ready(path),
            Self::Deblur => sk_core::ai_deblur::is_ready(path),
        }
    }

    fn ensure(self, path: &Path, progress: &dyn Fn(f32)) -> anyhow::Result<()> {
        match self {
            Self::Denoise => sk_core::ai_denoise::ensure(path, progress),
            Self::Deblur => sk_core::ai_deblur::ensure(path, progress),
        }
    }

    fn cache(self) -> (u64, u64) {
        match self {
            Self::Denoise => (sk_core::ai_denoise::cache_size(), sk_core::ai_denoise::CACHE_LIMIT),
            Self::Deblur => (sk_core::ai_deblur::cache_size(), sk_core::ai_deblur::CACHE_LIMIT),
        }
    }

    fn clear_cache(self) -> anyhow::Result<()> {
        match self {
            Self::Denoise => sk_core::ai_denoise::clear_cache(),
            Self::Deblur => sk_core::ai_deblur::clear_cache(),
        }
    }

    /// Of the feature whose model download was cancelled.
    fn of_feature(f: AiFeature) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.feature() == f)
    }
}

/// Images waiting for a network's result.
#[derive(Default)]
pub struct ProductQueue {
    pending: VecDeque<PathBuf>,
    /// Job id and image being computed.
    running: Option<(u64, PathBuf)>,
    /// Why an image could not be computed (not tried again until the app restarts).
    failed: HashMap<PathBuf, String>,
    /// The viewer's image was checked with this amount being on (avoids a file check per frame).
    checked: Option<PathBuf>,
    /// Cancelled by the user: not started again until its amount changes or it is transferred.
    cancelled: HashSet<PathBuf>,
    /// Whether the viewer image's result exists (the panel asks every frame).
    ready: crate::depth::ReadyCache,
}

/// What the Details panel shows below the amount.
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
    fn queue(&self, p: AiProduct) -> &ProductQueue {
        match p {
            AiProduct::Denoise => &self.denoise,
            AiProduct::Deblur => &self.deblur,
        }
    }

    fn queue_mut(&mut self, p: AiProduct) -> &mut ProductQueue {
        match p {
            AiProduct::Denoise => &mut self.denoise,
            AiProduct::Deblur => &mut self.deblur,
        }
    }

    /// Command: shows the noise section of the Details panel.
    pub fn open_denoise(&mut self, ctx: &egui::Context) {
        self.tab = crate::app::Tab::Details;
        crate::widgets::reveal_section(ctx, NOISE_SECTION);
    }

    /// Command: shows the sharpening section of the Details panel (with Verwacklung entfernen).
    pub fn open_deblur(&mut self, ctx: &egui::Context) {
        self.tab = crate::app::Tab::Details;
        crate::widgets::reveal_section(ctx, sk_core::tools::Tool::Sharpening.label());
    }

    /// Queues the computation of an image's result (`first`: before all others).
    pub(crate) fn want_product(&mut self, p: AiProduct, path: &Path, first: bool) {
        let q = self.queue_mut(p);
        if q.failed.contains_key(path) || q.cancelled.contains(path) || q.running.as_ref().is_some_and(|(_, r)| r == path) {
            return;
        }
        if let Some(i) = q.pending.iter().position(|r| r == path) {
            if !first {
                return;
            }
            q.pending.remove(i);
        }
        if p.is_ready(path) {
            return;
        }
        if first { q.pending.push_front(path.to_path_buf()) } else { q.pending.push_back(path.to_path_buf()) }
    }

    /// Each frame: queues the viewer's image where it wants a result, starts the next computations.
    pub(crate) fn pump_products(&mut self) {
        for p in AiProduct::ALL {
            self.pump_product(p);
        }
    }

    fn pump_product(&mut self, p: AiProduct) {
        let wanted = self.current.as_ref().filter(|c| p.amount(&c.adjustments.detail) > 0.0 && !c.meta.hdr).map(|c| c.meta.path.clone());
        if wanted != self.queue(p).checked {
            self.queue_mut(p).checked = wanted.clone();
            if let Some(path) = wanted {
                self.want_product(p, &path, true);
            }
        }
        let q = self.queue(p);
        if q.running.is_some() || q.pending.is_empty() || !self.ensure_ai(p.feature()) {
            return;
        }
        let Some(path) = self.queue_mut(p).pending.pop_front() else { return };
        let label = format!("{}: {}", p.label(), path.file_name().unwrap_or_default().to_string_lossy());
        let file = path.clone();
        self.start_job(&label, Box::new(move |progress| {
            p.ensure(&file, progress)?;
            Ok(Vec::new())
        }));
        let id = self.jobs.last().map_or(0, |j| j.id);
        self.queue_mut(p).running = Some((id, path));
    }

    /// The model download of `f` was cancelled: like cancelling the computation of the images waiting.
    pub(crate) fn product_download_cancelled(&mut self, f: AiFeature) {
        if let Some(p) = AiProduct::of_feature(f) {
            let q = self.queue_mut(p);
            q.cancelled.extend(q.pending.drain(..));
        }
    }

    /// A computation finished (`true` if the job was one).
    pub(crate) fn product_job_done(&mut self, id: u64, result: &Result<Vec<PathBuf>, String>) -> bool {
        let Some(p) = AiProduct::ALL.into_iter().find(|p| self.queue(*p).running.as_ref().is_some_and(|(i, _)| *i == id)) else { return false };
        let (_, path) = self.queue_mut(p).running.take().unwrap();
        self.jobs.retain(|j| j.id != id);
        self.queue(p).ready.forget();
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        match result {
            Ok(_) => {
                self.status = format!("{}: {name} fertig", p.label());
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
                self.report_error(format!("{} für {name} fehlgeschlagen: {e}", p.label()));
                let q = self.queue_mut(p);
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

    /// State of an image's result, for the Details panel.
    pub fn product_state(&self, p: AiProduct, path: &Path) -> ProductState {
        let f = p.feature();
        if let Some(progress) = self.ai_download_progress(f) {
            return ProductState::Downloading(progress);
        }
        if !f.available() {
            return ProductState::NoModel;
        }
        let q = self.queue(p);
        if let Some(e) = q.failed.get(path) {
            return ProductState::Failed(e.clone());
        }
        if q.cancelled.contains(path) {
            return ProductState::Cancelled;
        }
        if let Some((id, r)) = &q.running
            && r == path
        {
            return ProductState::Running(self.jobs.iter().find(|j| j.id == *id).map_or(0.0, |j| j.progress));
        }
        if q.pending.contains(&path.to_path_buf()) {
            return ProductState::Waiting;
        }
        if q.ready.get(path, |path| p.is_ready(path)) { ProductState::Ready } else { ProductState::Waiting }
    }

    /// The "Rauschreduzierung" section of the Details panel: KI-Entrauschen and the classic noise
    /// reduction (carried over to the other selected images with the button in its header, see
    /// `App::transfer_tool`); returns whether something changed.
    pub(crate) fn noise_section(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(cur) = self.current.as_ref() else { return false };
        let (path, hdr) = (cur.meta.path.clone(), cur.meta.hdr);
        let mut d = cur.adjustments.detail.clone();
        let defaults = Detail::default();
        let (mut reset, mut changed) = (false, false);
        let help = "KI-Entrauschen säubert die RAW-Daten im Hintergrund; das Original bleibt unverändert.\n\nLuminanz und Farbe: klassische Rauschreduzierung, auch zusätzlich.";
        crate::widgets::tool_section_help(ui, sk_core::tools::Tool::NoiseReduction, help, Some(&mut || reset = true), |ui| {
            changed |= self.product_controls(ui, AiProduct::Denoise, &mut d.ai_denoise, &path, hdr);
            changed |= slider_row(ui, "Luminanz", &mut d.nr_luma, 0.0..=100.0, defaults.nr_luma, 0);
            changed |= slider_row(ui, "Farbe", &mut d.nr_color, 0.0..=100.0, defaults.nr_color, 0);
        });
        if reset {
            (d.ai_denoise, d.nr_luma, d.nr_color) = (0.0, defaults.nr_luma, defaults.nr_color);
            changed = true;
        }
        let noise = (d.ai_denoise, d.nr_luma, d.nr_color);
        if let Some(cur) = self.current.as_mut() {
            let c = &mut cur.adjustments.detail;
            (c.ai_denoise, c.nr_luma, c.nr_color) = noise;
        }
        changed
    }

    /// The "Schärfen" section of the Details panel: Verwacklung entfernen and the unsharp mask;
    /// returns whether something changed.
    pub(crate) fn sharpen_section(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(cur) = self.current.as_ref() else { return false };
        let (path, hdr) = (cur.meta.path.clone(), cur.meta.hdr);
        let mut d = cur.adjustments.detail.clone();
        let defaults = Detail::default();
        let (mut reset, mut changed) = (false, false);
        let help = "Verwacklung entfernen: Ein KI-Modell schärft verwackelte oder bewegte Aufnahmen im Hintergrund nach; das Original bleibt unverändert. Bei scharfen Bildern glättet es feine Details.\n\nStärke, Radius und Schwelle: klassisches Schärfen, auch zusätzlich.";
        crate::widgets::tool_section_help(ui, sk_core::tools::Tool::Sharpening, help, Some(&mut || reset = true), |ui| {
            changed |= self.product_controls(ui, AiProduct::Deblur, &mut d.ai_deblur, &path, hdr);
            changed |= crate::widgets::percent_row(ui, "Stärke", &mut d.sharpen_amount, 0.0..=500.0, defaults.sharpen_amount);
            // Without an effect, what shapes it is shown but disabled.
            ui.add_enabled_ui(d.sharpen_amount > 0.0, |ui| {
                changed |= slider_row(ui, "Radius", &mut d.sharpen_radius, 0.2..=3.0, defaults.sharpen_radius, 1);
                changed |= slider_row(ui, "Schwelle", &mut d.sharpen_threshold, 0.0..=20.0, defaults.sharpen_threshold, 1);
            });
            crate::widgets::hint(ui, crate::commands::hint("Wirkung bei 100 % prüfen", "zoom-100"));
        });
        if reset {
            (d.ai_deblur, d.sharpen_amount, d.sharpen_radius, d.sharpen_threshold) = (0.0, defaults.sharpen_amount, defaults.sharpen_radius, defaults.sharpen_threshold);
            changed = true;
        }
        let sharpen = (d.ai_deblur, d.sharpen_amount, d.sharpen_radius, d.sharpen_threshold);
        if let Some(cur) = self.current.as_mut() {
            let c = &mut cur.adjustments.detail;
            (c.ai_deblur, c.sharpen_amount, c.sharpen_radius, c.sharpen_threshold) = sharpen;
        }
        changed
    }

    /// Sets the noise settings on the selected images and queues their denoising.
    pub(crate) fn transfer_noise(&mut self, (ai, luma, color): (f32, f32, f32)) {
        self.commit_and_save();
        let files = self.selected_paths();
        self.set_adjustments_on(&files, |a| {
            let mut a = a.clone();
            (a.detail.ai_denoise, a.detail.nr_luma, a.detail.nr_color) = (ai, luma, color);
            a
        });
        self.commit_and_save();
        if ai > 0.0 {
            for f in &files {
                self.denoise.cancelled.remove(f);
                self.want_product(AiProduct::Denoise, f, false);
            }
        }
        self.status = format!("Rauschen übertragen: {}", image_count(files.len()));
    }

    /// The amount of a network's result and the state of its computation; returns whether it changed.
    fn product_controls(&mut self, ui: &mut egui::Ui, p: AiProduct, amount: &mut f32, path: &Path, hdr: bool) -> bool {
        let before = *amount;
        // Merged HDR images can't be computed: the slider is shown, but disabled.
        let opts = crate::widgets::SliderOpts { ai: true, ..Default::default() };
        let changed = ui.add_enabled_ui(!hdr, |ui| crate::widgets::slider_row_with(ui, p.label(), amount, 0.0..=100.0, 0.0, 0, opts)).inner;
        if changed && self.queue_mut(p).cancelled.remove(path) {
            // Queued again by `pump_products`.
            self.queue_mut(p).checked = None;
        }
        let faint = |t: String| RichText::new(t).small().color(crate::theme::TEXT_FAINT);
        if hdr {
            ui.label(faint("Für zusammengesetzte HDR-Bilder nicht verfügbar".into()));
            return changed;
        }
        let f = p.feature();
        match self.product_state(p, path) {
            ProductState::NoModel => {
                if changed && before == 0.0 && *amount > 0.0 {
                    self.ensure_ai(f);
                } else {
                    ui.label(faint(format!("Lädt beim ersten Einschalten ein KI-Modell ({} MB)", f.download_mb())));
                }
            }
            ProductState::Downloading(progress) => crate::ai_models::download_progress(ui, f, progress, self.job_cancel(None, Some(&f.job_label()))),
            _ if *amount <= 0.0 => {}
            ProductState::Waiting => {
                ui.label(faint("Berechne gleich …".into()));
            }
            ProductState::Running(progress) => {
                ui.add(egui::ProgressBar::new(progress).desired_height(6.0));
                ui.horizontal(|ui| {
                    ui.label(faint(format!("Berechne … {:.0} %", progress * 100.0)));
                    if let Some(c) = self.job_cancel(self.queue(p).running.as_ref().map(|r| r.0), None) {
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

    /// The caches of the network results (in the dialog for the graphics card): how full, and
    /// emptying them.
    pub(crate) fn product_cache_ui(&mut self, ui: &mut egui::Ui) {
        for p in AiProduct::ALL {
            let (used, limit) = p.cache();
            let mut clear = false;
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.label(RichText::new(format!("{} · {:.1} von {} GB", p.label(), used as f64 / 1e9, limit >> 30)).size(12.0).color(crate::theme::TEXT));
                    ui.add(egui::ProgressBar::new((used as f64 / limit as f64) as f32).desired_width(260.0).desired_height(4.0));
                    if p == AiProduct::Deblur {
                        crate::dialog::note(ui, "Die ältesten Ergebnisse fallen heraus, wenn er voll ist.");
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can = used > 0 && self.queue(p).running.is_none();
                    if ui.add_enabled(can, crate::dialog::secondary("Leeren")).clicked() {
                        clear = true;
                    }
                });
            });
            if clear {
                match p.clear_cache() {
                    Ok(()) => {
                        self.status = format!("{}: Zwischenspeicher geleert", p.label());
                        // Images still set to it compute again when shown.
                        self.queue_mut(p).checked = None;
                        self.request_render();
                    }
                    Err(e) => self.status = format!("Zwischenspeicher nicht geleert: {e:#}"),
                }
            }
        }
    }
}
