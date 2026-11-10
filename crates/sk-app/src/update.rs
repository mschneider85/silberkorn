//! New versions from GitHub: a quiet check at startup and every few hours while the program runs
//! (failures, e.g. offline, only go to the log), a banner once per new version, a button in the
//! status bar while one is out, and a dialog that downloads its installer (Windows) or opens its
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
const CHECK_EVERY_SECS: u64 = 60 * 60;
const INSTALL_JOB: &str = "Update laden";

#[derive(Default)]
pub struct Updates {
    /// No check at startup.
    pub auto_off: bool,
    /// Last automatic check, in seconds since 1970.
    pub last_check: Option<u64>,
    /// A version the user doesn't want to be reminded of.
    pub skipped: Option<String>,
    /// Automatic checks run (not in tests, which don't save settings).
    auto_checks: bool,
    /// The newest version the banner announced; each version gets it once.
    pub announced: Option<String>,
    /// The banner is showing.
    banner: bool,
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
    pub fn new(auto_off: bool, last_check: Option<u64>, skipped: Option<String>, announced: Option<String>) -> Self {
        Self { auto_off, last_check, skipped, announced, ..Default::default() }
    }

    /// Seconds until the next automatic check (0: due now); `None` when there are none.
    /// Development builds don't remind of releases.
    fn next_check_in(&self, now: u64) -> Option<u64> {
        if self.auto_off || cfg!(debug_assertions) {
            return None;
        }
        Some(self.last_check.map_or(0, |t| CHECK_EVERY_SECS.saturating_sub(now.saturating_sub(t))))
    }

    /// As if a check had just answered this.
    #[cfg(test)]
    pub(crate) fn fake_answer(&mut self, answer: Result<Release, String>) {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(answer).unwrap();
        self.checking = Some(rx);
    }

    #[cfg(test)]
    pub(crate) fn banner_shown(&self) -> bool {
        self.banner
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl App {
    /// At startup: clean up after an update and check for a new version if it's time.
    pub(crate) fn start_update_check(&mut self) {
        update::remove_leftovers();
        self.updates.auto_checks = true;
        self.auto_update_check(None);
    }

    /// Checks if it's time, and wakes the program up for the next one.
    fn auto_update_check(&mut self, ctx: Option<&egui::Context>) {
        if !self.updates.auto_checks {
            return;
        }
        let now = now_secs();
        let Some(wait) = self.updates.next_check_in(now) else { return };
        if wait == 0 {
            self.updates.last_check = Some(now);
            self.check_for_update();
        } else if let Some(ctx) = ctx {
            ctx.request_repaint_after(std::time::Duration::from_secs(wait));
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
                    if self.updates.announced.as_deref() == Some(&r.version) {
                        self.status = format!("Silberkorn {} ist verfügbar", r.version);
                    } else {
                        self.updates.banner = true;
                        self.updates.announced = Some(r.version.clone());
                        self.save_settings();
                    }
                }
                self.updates.available = Some(r);
                self.updates.result = Some(Ok(()));
            }
            Ok(_) => {
                self.updates.available = None;
                self.updates.result = Some(Ok(()));
            }
            // Offline or GitHub unreachable: only the dialog tells, if it is open; the next
            // automatic check tries again.
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

    /// Once per new version, bottom right: what's out, and a way to the dialog.
    pub(crate) fn update_banner(&mut self, ctx: &egui::Context) {
        self.auto_update_check(Some(ctx));
        self.poll_update_check();
        if self.updates.dialog || self.updates.setup.is_some() {
            self.updates.banner = false;
        }
        if !self.updates.banner {
            return;
        }
        let Some(r) = &self.updates.available else {
            self.updates.banner = false;
            return;
        };
        let version = r.version.clone();
        let (mut open, mut later) = (false, false);
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::new("update_banner"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::RIGHT_BOTTOM)
            .fixed_pos(screen.right_bottom() + egui::vec2(-16.0, -40.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).corner_radius(10).inner_margin(egui::Margin::same(14)).show(ui, |ui| {
                    ui.set_width(300.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icon::ARROW_CIRCLE_UP).size(18.0).color(theme::ACCENT));
                        ui.label(RichText::new(format!("Silberkorn {version} ist verfügbar")).size(14.0).strong().color(egui::Color32::WHITE));
                    });
                    ui.label(RichText::new(format!("Installiert ist Version {}.", update::current_version())).size(12.0).color(theme::TEXT_DIM));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.add(dialog::primary("Ansehen")).on_hover_text("Was neu ist und installieren").clicked() {
                            open = true;
                        }
                        if ui.add(dialog::secondary("Später")).on_hover_text("Bleibt als Knopf in der Statusleiste").clicked() {
                            later = true;
                        }
                    });
                });
            });
        if open {
            self.updates.dialog = true;
        }
        if open || later {
            self.updates.banner = false;
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

    /// Quitting for the update's restart was cancelled ("Weiterarbeiten"): quitting later only
    /// installs, as with "Beim Beenden".
    pub(crate) fn restart_cancelled(&mut self) {
        self.updates.restart = false;
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
            crate::widgets::checkbox(ui, &mut auto, "Automatisch nach Updates suchen");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_every_hour_unless_off() {
        let now = 1_000_000;
        let mut u = Updates::new(false, None, None, None);
        if cfg!(debug_assertions) {
            assert_eq!(u.next_check_in(now), None, "Entwicklungsversionen prüfen nicht");
            return;
        }
        assert_eq!(u.next_check_in(now), Some(0), "noch nie geprüft");
        u.last_check = Some(now - 20 * 60);
        assert_eq!(u.next_check_in(now), Some(40 * 60));
        u.last_check = Some(now - 2 * 60 * 60);
        assert_eq!(u.next_check_in(now), Some(0));
        u.auto_off = true;
        assert_eq!(u.next_check_in(now), None);
    }
}
