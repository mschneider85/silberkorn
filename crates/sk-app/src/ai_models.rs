//! The AI functions and the models they need. All models (and on Windows the AI acceleration)
//! are fetched quietly at startup, without asking; the start screen shows the progress, then the
//! status bar. A function used before its model is there downloads it first and then carries on.
//! Model names and licenses appear in the licenses dialog (see `licenses`).

use sk_ai::models::{self, ModelInfo};

use crate::app::App;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AiFeature {
    Denoise,
    Subject,
    Object,
    Inpaint,
    Depth,
    Faces,
}

impl AiFeature {
    pub const ALL: [AiFeature; 6] = [Self::Denoise, Self::Subject, Self::Object, Self::Inpaint, Self::Depth, Self::Faces];

    /// What the user uses it for.
    pub fn label(self) -> &'static str {
        match self {
            Self::Denoise => "KI-Entrauschen",
            Self::Subject => "Motiv und Himmel auswählen",
            Self::Object => "Objekt auswählen",
            Self::Inpaint => "Entfernen",
            Self::Depth => "Tiefenkarte",
            Self::Faces => "Gesichter und Augen",
        }
    }

    fn files(self) -> &'static [&'static ModelInfo] {
        match self {
            Self::Inpaint => &[&models::INPAINT],
            Self::Depth => &[&models::DEPTH],
            Self::Faces => &sk_ai::faces::NEEDED,
            Self::Denoise | Self::Subject | Self::Object => &[],
        }
    }

    pub fn available(self) -> bool {
        match self {
            Self::Denoise => sk_ai::denoise::available(),
            Self::Subject => sk_ai::model_available(),
            Self::Object => sk_ai::segment::available(),
            _ => self.files().iter().all(|m| models::available(m)),
        }
    }

    pub fn download_mb(self) -> u64 {
        match self {
            Self::Denoise => sk_ai::denoise::download_mb(),
            Self::Subject => models::SUBJECT.size_mb(),
            Self::Object => sk_ai::segment::download_mb(),
            _ => self.files().iter().map(|m| m.bytes).sum::<u64>().div_ceil(1_000_000),
        }
    }

    fn download(self, progress: &dyn Fn(f32)) -> anyhow::Result<()> {
        match self {
            Self::Denoise => sk_ai::denoise::download(progress),
            Self::Subject => sk_ai::download_model(progress).map(|_| ()),
            Self::Object => sk_ai::segment::download(progress),
            _ => {
                let files: Vec<_> = self.files().iter().filter(|m| !models::available(m)).collect();
                for (i, m) in files.iter().enumerate() {
                    models::download(m, &|p| progress((i as f32 + p) / files.len() as f32))?;
                }
                Ok(())
            }
        }
    }

    pub(crate) fn job_label(self) -> String {
        format!("KI-Modell laden: {}", self.label())
    }
}

/// Something to do once a model is there.
pub type AfterDownload = Box<dyn FnOnce(&mut App)>;

/// Downloads waiting to finish: what to do then, and which ones started on their own.
#[derive(Default)]
pub struct AiDownloads {
    after: Vec<(AiFeature, AfterDownload)>,
    quiet: Vec<AiFeature>,
    /// When a download last failed: the features that want a model every frame (denoising,
    /// depth) try again only after a pause, not in a loop without network.
    failed: std::collections::HashMap<AiFeature, std::time::Instant>,
}

/// Pause before a failed download is tried again.
const RETRY: std::time::Duration = std::time::Duration::from_secs(30);

/// The face models that the face analysis needs besides `AiFeature::Faces`'s.
const FACES_AT_STARTUP: [&ModelInfo; 2] = [&models::FACE_DETECT, &models::EYE_STATE];

impl App {
    /// Whether `f` can run now; otherwise starts downloading its model (once).
    pub fn ensure_ai(&mut self, f: AiFeature) -> bool {
        if f.available() {
            return true;
        }
        let label = f.job_label();
        let failed_recently = self.ai_downloads.failed.get(&f).is_some_and(|t| t.elapsed() < RETRY);
        if !failed_recently && !self.jobs.iter().any(|j| j.label == label) {
            self.start_job(&label, Box::new(move |p| {
                f.download(p)?;
                Ok(Vec::new())
            }));
        }
        false
    }

    /// Runs `then` once the model of `f` is there: at once, or after its download.
    pub fn with_ai(&mut self, f: AiFeature, then: impl FnOnce(&mut App) + 'static) {
        if self.ensure_ai(f) {
            then(self);
        } else if self.ai_download_progress(f).is_none() {
            // Failed a moment ago (see `RETRY`).
            self.status = format!("{}: KI-Modell konnte nicht geladen werden – Internetverbindung prüfen", f.label());
        } else {
            self.ai_downloads.quiet.retain(|q| *q != f);
            self.ai_downloads.after.push((f, Box::new(then)));
        }
    }

    /// Progress of the download of `f`'s model, if it runs.
    pub fn ai_download_progress(&self, f: AiFeature) -> Option<f32> {
        let label = f.job_label();
        self.jobs.iter().find(|j| j.label == label).map(|j| j.progress)
    }

    /// Fetches every missing model in the background (at startup), and on Windows the AI
    /// acceleration; quiet also without network (a function used later tries again).
    pub fn preload_ai(&mut self) {
        let status = self.status.clone();
        for f in AiFeature::ALL {
            if !f.available() {
                self.ai_downloads.quiet.push(f);
                self.ensure_ai(f);
            }
        }
        if sk_ai::runtime::gpu_supported() && !sk_ai::runtime::gpu_installed() {
            self.install_gpu_job();
            self.quiet_jobs.push(crate::gpu::GPU_JOB.to_string());
        }
        if FACES_AT_STARTUP.iter().any(|m| !models::available(m)) {
            let label = "KI-Modell laden: Gesichtserkennung";
            self.start_job(label, Box::new(|_| {
                for m in FACES_AT_STARTUP.iter().filter(|m| !models::available(m)) {
                    models::download(m, &|_| {})?;
                }
                Ok(Vec::new())
            }));
            self.quiet_jobs.push(label.to_string());
        }
        self.status = status;
    }

    /// Progress of the downloads started at startup (by size), while any runs.
    pub fn ai_preload_progress(&self) -> Option<f32> {
        let mut parts: Vec<(f32, f32)> =
            AiFeature::ALL.iter().filter_map(|f| self.ai_download_progress(*f).map(|p| (f.download_mb() as f32, p))).collect();
        let gpu = self.jobs.iter().find(|j| j.label == crate::gpu::GPU_JOB).map(|j| (sk_ai::runtime::GPU_DOWNLOAD_MB as f32, j.progress));
        parts.extend(gpu);
        let total: f32 = parts.iter().map(|(mb, _)| mb).sum();
        (total > 0.0).then(|| parts.iter().map(|(mb, p)| mb * p).sum::<f32>() / total)
    }

    /// A model download finished (`true` if the job was one): runs what waited for it.
    pub(crate) fn ai_download_done(&mut self, id: u64, label: &str, result: &Result<Vec<std::path::PathBuf>, String>) -> bool {
        if let Some(i) = self.quiet_jobs.iter().position(|l| l == label) {
            self.quiet_jobs.remove(i);
            self.jobs.retain(|j| j.id != id);
            return true;
        }
        let Some(f) = AiFeature::ALL.into_iter().find(|f| f.job_label() == label) else { return false };
        self.jobs.retain(|j| j.id != id);
        let quiet = self.ai_downloads.quiet.contains(&f);
        self.ai_downloads.quiet.retain(|q| *q != f);
        let (now, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.ai_downloads.after).into_iter().partition(|(g, _)| *g == f);
        self.ai_downloads.after = later;
        match result {
            Ok(_) => {
                self.ai_downloads.failed.remove(&f);
                if !quiet {
                    self.status = format!("{}: Modell geladen", f.label());
                }
                for (_, then) in now {
                    then(self);
                }
            }
            Err(e) if e == crate::jobs::CANCELLED => {
                // Not tried again by itself: only when the user asks for the feature again.
                self.status = format!("{}: Laden des KI-Modells abgebrochen", f.label());
                match f {
                    AiFeature::Denoise => self.denoise_download_cancelled(),
                    AiFeature::Depth => self.depth_download_cancelled(),
                    _ => {}
                }
            }
            Err(e) => {
                self.ai_downloads.failed.insert(f, std::time::Instant::now());
                if !quiet {
                    self.status = format!("{}: KI-Modell konnte nicht geladen werden ({e})", f.label());
                }
            }
        }
        true
    }
}

/// Progress of a model download below a tool: what is loaded and how far.
pub fn download_progress(ui: &mut egui::Ui, f: AiFeature, progress: f32) {
    ui.add(egui::ProgressBar::new(progress).desired_height(6.0));
    crate::widgets::hint(ui, format!("Lade KI-Modell ({} MB, einmalig) … {:.0} %", f.download_mb(), progress * 100.0));
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
}
