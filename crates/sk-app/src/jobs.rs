//! Long-running background jobs that create new files (HDR merge, panorama, denoising): each
//! runs in a thread of its own and reports its progress; the new files are added to the
//! catalog and selected when done.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::app::App;
use crate::workers::{Event, IndexJob};

/// A job as shown in the status bar.
pub struct RunningJob {
    pub id: u64,
    pub label: String,
    pub progress: f32,
    /// Set to stop the job (at its next progress report).
    pub cancel: Arc<AtomicBool>,
}

/// Unwinds a cancelled job from its progress report (not a panic: no hook, no log).
struct Cancelled;

pub type JobFn = Box<dyn FnOnce(&dyn Fn(f32)) -> anyhow::Result<Vec<PathBuf>> + Send>;

impl crate::workers::Workers {
    /// Runs `f` in the background; returns the job's id and the flag that cancels it. A
    /// cancelled job stops at its next progress report (it unwinds from there, so files it
    /// writes atomically are never left half-written).
    pub fn job(&self, label: String, f: JobFn) -> (u64, Arc<AtomicBool>) {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let (tx, ctx) = (self.event_sender(), self.context());
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::spawn(move || {
            let progress = |p: f32| {
                if flag.load(Ordering::Relaxed) {
                    std::panic::resume_unwind(Box::new(Cancelled));
                }
                let _ = tx.send(Event::JobProgress { id, progress: p });
                ctx.request_repaint();
            };
            let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&progress))) {
                Ok(r) => r.map_err(|e| format!("{e:#}")),
                Err(p) if p.is::<Cancelled>() => Err("abgebrochen".to_string()),
                Err(_) => Err("Interner Fehler (Details in panic.log)".to_string()),
            };
            let _ = tx.send(Event::JobDone { id, label, result });
            ctx.request_repaint();
        });
        (id, cancel)
    }
}

impl App {
    pub fn start_job(&mut self, label: &str, f: JobFn) {
        let (id, cancel) = self.workers.job(label.to_string(), f);
        self.jobs.push(RunningJob { id, label: label.to_string(), progress: 0.0, cancel });
        self.status = format!("{label} …");
    }

    pub(crate) fn job_progress(&mut self, id: u64, progress: f32) {
        if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
            j.progress = progress;
        }
    }

    pub(crate) fn job_done(&mut self, id: u64, label: String, result: Result<Vec<PathBuf>, String>) {
        if self.ai_download_done(id, &label, &result) || self.update_job_done(id, &label, &result) || self.denoise_job_done(id, &result) || self.depth_job_done(id, &result) {
            return;
        }
        self.jobs.retain(|j| j.id != id);
        match result {
            Ok(files) => {
                self.status = match files.as_slice() {
                    [] => format!("{label}: fertig"),
                    [f] => format!("{label}: {}", f.file_name().unwrap_or_default().to_string_lossy()),
                    _ => format!("{label}: {}", crate::widgets::image_count(files.len())),
                };
                if files.is_empty() {
                    // The job changed the catalog (analysis) or nothing visible.
                    self.reload_entries();
                } else {
                    self.files_created(files);
                }
            }
            Err(e) => self.status = format!("{label} fehlgeschlagen: {e}"),
        }
    }

    /// New files next to the originals: into the catalog, shown and selected.
    pub(crate) fn files_created(&mut self, files: Vec<PathBuf>) {
        if files.is_empty() {
            return;
        }
        self.workers.index(IndexJob { catalog: self.catalog_path(), paths: files.clone(), import: false, copy_to: None });
        self.reload_entries();
        self.select_when_listed = files.first().cloned();
        self.select_created();
    }

    /// Selects a created file once it is in the browser (after indexing in catalog scopes).
    pub(crate) fn select_created(&mut self) {
        let Some(p) = self.select_when_listed.clone() else { return };
        let p = p.canonicalize().unwrap_or(p);
        if self.index_of(&p).is_some() {
            self.select_when_listed = None;
            self.select_path(&p, egui::Modifiers::NONE);
            self.scroll_to_primary = true;
        }
    }
}
