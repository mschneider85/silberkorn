//! New versions from GitHub: a quiet check at startup (at most once a day), a button in the
//! status bar when one is out, and a dialog that downloads its installer (Windows) or opens its
//! page. The installer runs when the program closes: at once with "Jetzt neu starten", else when
//! the user quits.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Button, Color32, RichText, vec2};
use sk_core::update::{self, Release};

use crate::app::App;
use crate::theme::{self, icon};

/// Pause between automatic checks.
const CHECK_EVERY_SECS: u64 = 24 * 60 * 60;
const INSTALL_JOB: &str = "Update laden";

#[derive(Default)]
pub struct Updates {
    /// No check at startup.
    pub auto_off: bool,
    /// Last automatic check, in seconds since 1970.
    pub last_check: Option<u64>,
    /// A version the user doesn't want to be reminded of.
    pub skipped: Option<String>,
    checking: Option<Receiver<Result<Release, String>>>,
    /// Newer than this program.
    pub available: Option<Release>,
    /// What the last check found, for the dialog: up to date, or the error.
    result: Option<Result<(), String>>,
    pub dialog: bool,
    /// The downloaded installer, run when the program closes.
    setup: Option<PathBuf>,
    /// Start the program again after the installer.
    restart: bool,
}

impl Updates {
    pub fn new(auto_off: bool, last_check: Option<u64>, skipped: Option<String>) -> Self {
        Self { auto_off, last_check, skipped, ..Default::default() }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl App {
    /// At startup: clean up after an update and check for a new version if it's time.
    pub(crate) fn start_update_check(&mut self) {
        update::remove_leftovers();
        // Development builds don't remind of releases.
        let due = self.updates.last_check.is_none_or(|t| now_secs().saturating_sub(t) >= CHECK_EVERY_SECS);
        if !self.updates.auto_off && due && !cfg!(debug_assertions) {
            self.updates.last_check = Some(now_secs());
            self.check_for_update();
        }
    }

    /// Asks GitHub for the latest release, in the background.
    pub(crate) fn check_for_update(&mut self) {
        if self.updates.checking.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.workers.context();
        std::thread::spawn(move || {
            let _ = tx.send(update::latest().map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        self.updates.checking = Some(rx);
        self.updates.result = None;
    }

    /// "Nach Updates suchen …": checks now and shows the result.
    pub(crate) fn update_dialog_open(&mut self) {
        self.updates.dialog = true;
        if self.updates.setup.is_none() && self.jobs.iter().all(|j| j.label != INSTALL_JOB) {
            self.check_for_update();
        }
    }

    /// Takes the check's answer once it's there.
    fn poll_update_check(&mut self) {
        let Some(rx) = &self.updates.checking else { return };
        let answer = match rx.try_recv() {
            Ok(a) => a,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("Interner Fehler".into()),
        };
        self.updates.checking = None;
        match answer {
            Ok(r) if update::is_newer(&r.version, update::current_version()) => {
                if !self.updates.dialog && self.updates.skipped.as_deref() != Some(&r.version) {
                    self.status = format!("Silberkorn {} ist verfügbar", r.version);
                }
                self.updates.available = Some(r);
                self.updates.result = Some(Ok(()));
            }
            Ok(_) => {
                self.updates.available = None;
                self.updates.result = Some(Ok(()));
            }
            Err(e) => {
                log::info!("Update-Prüfung: {e}");
                self.updates.result = Some(Err(e));
            }
        }
    }

    /// Status bar: a button for a new version (unless skipped).
    pub(crate) fn update_status(&mut self, ui: &mut egui::Ui) {
        self.poll_update_check();
        let Some(r) = &self.updates.available else { return };
        let text = if self.updates.setup.is_some() {
            format!("{}  Neu starten für {}", icon::ARROW_CLOCKWISE, r.version)
        } else if self.updates.skipped.as_deref() == Some(&r.version) {
            return;
        } else {
            format!("{}  Update {}", icon::ARROW_CIRCLE_UP, r.version)
        };
        ui.separator();
        let b = Button::new(RichText::new(text).size(12.0).color(theme::ACCENT)).frame(false);
        if ui.add(b).on_hover_text("Was neu ist und installieren").clicked() {
            self.updates.dialog = true;
        }
    }

    pub(crate) fn update_job_done(&mut self, id: u64, label: &str, result: &Result<Vec<std::path::PathBuf>, String>) -> bool {
        if label != INSTALL_JOB {
            return false;
        }
        self.jobs.retain(|j| j.id != id);
        match result {
            Ok(files) => {
                self.updates.setup = files.first().cloned();
                self.updates.dialog = true;
                self.status = "Update geladen; es wird beim Neustart oder Beenden installiert".into();
            }
            Err(e) if e == "abgebrochen" => self.status = "Update abgebrochen".into(),
            Err(e) => {
                self.updates.result = Some(Err(e.clone()));
                self.status = format!("Update fehlgeschlagen: {e}");
            }
        }
        true
    }

    fn install_update(&mut self, release: Release) {
        self.updates.result = None;
        self.start_job(INSTALL_JOB, Box::new(move |p| update::download_setup(&release, p).map(|f| vec![f])));
    }

    /// Closes the program; `on_exit` starts it again.
    fn restart_for_update(&mut self, ctx: &egui::Context) {
        self.updates.restart = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// The program closes: runs a downloaded installer (which starts the new version after a
    /// restart for the update).
    pub(crate) fn install_update_on_exit(&self) {
        let Some(setup) = &self.updates.setup else { return };
        if let Err(e) = update::run_setup(setup, self.updates.restart) {
            log::error!("{e:#}");
        }
    }

    pub(crate) fn update_dialog(&mut self, ctx: &egui::Context) {
        self.poll_update_check();
        if !self.updates.dialog {
            return;
        }
        let installing = self.jobs.iter().find(|j| j.label == INSTALL_JOB).map(|j| (j.progress, j.cancel.clone()));
        let current = update::current_version();
        let mut auto = !self.updates.auto_off;
        let (mut close, mut install, mut skip, mut restart) = (false, false, false, false);
        let modal = egui::Modal::new(egui::Id::new("update")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(RichText::new(format!("{}  Updates", icon::ARROW_CIRCLE_UP)).size(17.0).strong().color(Color32::WHITE));
            ui.add_space(6.0);
            let dim = |t: String| RichText::new(t).color(Color32::from_gray(170));
            if self.updates.checking.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Suche nach einer neuen Version …");
                });
            } else if let Some(r) = &self.updates.available {
                if self.updates.setup.is_some() {
                    ui.label(format!("Silberkorn {} ist geladen und wird beim Neustart installiert – sonst beim Beenden.", r.version));
                } else {
                    ui.label(RichText::new(format!("Silberkorn {} ist verfügbar", r.version)).strong());
                    ui.label(dim(format!("Installiert ist Version {current}.")));
                }
                if !r.notes.is_empty() {
                    ui.add_space(6.0);
                    egui::Frame::new().fill(theme::BROWSER).inner_margin(8.0).corner_radius(4).show(ui, |ui| {
                        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(&r.notes);
                        });
                    });
                }
            } else {
                match &self.updates.result {
                    Some(Ok(())) => {
                        ui.label(format!("{}  Silberkorn {current} ist aktuell.", icon::CHECK));
                    }
                    Some(Err(e)) => {
                        ui.label(dim(format!("Keine Auskunft: {e}")));
                    }
                    None => {}
                }
            }
            if let (Some(Err(e)), Some(_)) = (&self.updates.result, &self.updates.available) {
                ui.add_space(4.0);
                ui.label(RichText::new(e).color(theme::DANGER));
            }
            if let Some((p, _)) = &installing {
                ui.add_space(6.0);
                ui.add(egui::ProgressBar::new(*p).text(format!("Wird geladen … {:.0} %", p * 100.0)));
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
            ui.add_space(10.0);
            ui.checkbox(&mut auto, "Beim Start nach Updates suchen (höchstens einmal am Tag)");
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let size = vec2(90.0, 26.0);
                let release = self.updates.available.as_ref();
                if self.updates.setup.is_some() {
                    if ui.add(Button::new("Jetzt neu starten").min_size(size)).clicked() {
                        restart = true;
                    }
                    if ui.add(Button::new("Beim Beenden").min_size(size)).clicked() {
                        close = true;
                    }
                    return;
                }
                if let Some((_, cancel)) = &installing {
                    if ui.add(Button::new("Abbrechen").min_size(size)).clicked() {
                        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    return;
                }
                if ui.add(Button::new("Schließen").min_size(size)).clicked() {
                    close = true;
                }
                let Some(r) = release else { return };
                if cfg!(windows) && r.asset.is_some() {
                    if ui.add(Button::new(format!("{}  Jetzt aktualisieren", icon::DOWNLOAD_SIMPLE)).min_size(size)).clicked() {
                        install = true;
                    }
                } else if ui.add(Button::new("Release-Seite öffnen").min_size(size)).clicked() {
                    ctx.open_url(egui::OpenUrl::new_tab(&r.page));
                }
                if self.updates.skipped.as_deref() != Some(&r.version)
                    && ui.add(Button::new("Überspringen").min_size(size)).on_hover_text("An diese Version nicht mehr erinnern").clicked()
                {
                    skip = true;
                }
            });
        });
        if auto == self.updates.auto_off {
            self.updates.auto_off = !auto;
            self.save_settings();
        }
        if install && let Some(r) = self.updates.available.clone() {
            self.install_update(r);
        }
        if skip {
            self.updates.skipped = self.updates.available.as_ref().map(|r| r.version.clone());
            self.save_settings();
            close = true;
        }
        if restart {
            self.restart_for_update(ctx);
        }
        if close || (modal.should_close() && installing.is_none()) {
            self.updates.dialog = false;
        }
    }
}
