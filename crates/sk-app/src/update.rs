//! New versions from GitHub: a quiet check at startup (at most once a day), a button in the
//! status bar when one is out, and a dialog that downloads its installer (Windows) or opens its
//! page. The installer runs when the program closes: at once with "Jetzt neu starten", else when
//! the user quits.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Button, RichText};
use sk_core::update::{self, Release};

use crate::app::App;
use crate::dialog::{self, Dialog, Tone};
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
            Err(e) if e == crate::jobs::CANCELLED => self.status = "Update abgebrochen".into(),
            Err(e) => {
                self.updates.result = Some(Err(e.clone()));
                self.report_error(format!("Update fehlgeschlagen: {e}"));
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
        let modal = Dialog::new("update", icon::ARROW_CIRCLE_UP, "Updates").subtitle(format!("Installiert ist Version {current}")).width(480.0).show(ctx, |ui| {
            if self.updates.checking.is_some() {
                dialog::card(ui, "Suche nach einer neuen Version …", Some((Tone::Accent, "Prüft")), |ui| {
                    ui.spinner();
                });
            } else if let Some(r) = &self.updates.available {
                let ready = self.updates.setup.is_some();
                let status = if ready { (Tone::Good, "Geladen") } else { (Tone::Accent, "Neu") };
                dialog::card(ui, &format!("Silberkorn {}", r.version), Some(status), |ui| {
                    if ready {
                        dialog::note(ui, "Wird beim Neustart installiert – sonst beim Beenden.");
                    }
                    if !r.notes.is_empty() {
                        dialog::well(ui, |ui| {
                            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(&r.notes);
                            });
                        });
                    }
                    if let Some((p, _)) = &installing {
                        ui.add(egui::ProgressBar::new(*p).desired_height(8.0).text(format!("Lade … {:.0} %", p * 100.0)));
                        ctx.request_repaint_after(std::time::Duration::from_millis(200));
                    }
                    if let Some(Err(e)) = &self.updates.result {
                        ui.label(RichText::new(e).size(12.0).color(theme::DANGER));
                    }
                });
            } else {
                match &self.updates.result {
                    Some(Ok(())) => dialog::card(ui, &format!("Silberkorn {current}"), Some((Tone::Good, "Aktuell")), |ui| {
                        dialog::note(ui, "Es gibt keine neuere Version.");
                    }),
                    Some(Err(e)) => dialog::card(ui, "Keine Auskunft", Some((Tone::Warn, "Fehler")), |ui| {
                        dialog::note(ui, e.to_string());
                    }),
                    None => {}
                }
            }
            ui.add_space(4.0);
            ui.checkbox(&mut auto, "Beim Start nach Updates suchen (höchstens einmal am Tag)");
            dialog::footer(ui, |ui| {
                let release = self.updates.available.as_ref();
                if self.updates.setup.is_some() {
                    if dialog::default_button(ui, true, dialog::primary("Jetzt neu starten")) {
                        restart = true;
                    }
                    if ui.add(dialog::secondary("Beim Beenden")).clicked() {
                        close = true;
                    }
                    return;
                }
                if let Some((_, cancel)) = &installing {
                    if ui.add(dialog::secondary("Abbrechen")).clicked() {
                        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    return;
                }
                let Some(r) = release else {
                    if dialog::default_button(ui, true, dialog::primary("Schließen")) {
                        close = true;
                    }
                    return;
                };
                if cfg!(windows) && r.asset.is_some() {
                    if dialog::default_button(ui, true, dialog::primary(format!("{}  Jetzt aktualisieren", icon::DOWNLOAD_SIMPLE))) {
                        install = true;
                    }
                } else if ui.add(dialog::primary("Release-Seite öffnen")).clicked() {
                    ctx.open_url(egui::OpenUrl::new_tab(&r.page));
                }
                if ui.add(dialog::secondary("Schließen")).clicked() {
                    close = true;
                }
                if self.updates.skipped.as_deref() != Some(&r.version)
                    && ui.add(dialog::secondary("Überspringen")).on_hover_text("An diese Version nicht mehr erinnern").clicked()
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
