//! Printing (Strg+P) and printing to a file (PDF or JPEG): the selected images, one per page,
//! laid out like the print preview shows them (see `sk_core::print` and `print_page`).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Result, bail};
use sk_core::color::OutputSpace;
use sk_core::image::LinearImage;
use sk_core::params::Adjustments;
use sk_core::print::PrintSettings;
use sk_core::print_page::{self, FILE_DPI, PROFILE_SOURCE, PrinterProfile};
use sk_core::raw::{CaptureInfo, RawMeta};

use crate::app::App;
use crate::printer::{Choice, Printer, Request};

/// Where the pages go.
#[derive(Clone, Debug, PartialEq)]
pub enum PrintTo {
    Printer,
    /// A PDF with all pages, or a JPEG per page (by the extension).
    File(PathBuf),
}

/// The job's name in the status bar.
const LABEL: &str = "Drucken";

/// Whether this system can print (the printer is driven through Windows).
pub const CAN_PRINT: bool = cfg!(windows);

impl App {
    /// Strg+P: first the print bar with the page in the print preview; from there (Strg+P again
    /// or Enter) the system's print dialog.
    pub fn print_command(&mut self) {
        if self.print.bar {
            self.print_selection();
        } else {
            self.enter_print_bar();
        }
    }

    /// The print bar in the image, over the print preview (turned on for it).
    pub fn enter_print_bar(&mut self) {
        if self.current.is_none() {
            self.status = "Kein Bild ausgewählt".into();
            return;
        }
        self.end_viewer_tools();
        self.grid_view = false;
        self.print.was_on = self.print.on;
        if !self.print.on {
            self.toggle_print_preview();
        }
        // The whole sheet in view, above the bar.
        self.print.zoom = false;
        self.zoom = None;
        self.pan = egui::Vec2::ZERO;
        self.print.bar = true;
    }

    /// Esc: the print bar closes, the print preview goes back to how it was.
    pub fn leave_print_bar(&mut self) {
        self.print.bar = false;
        if !self.print.was_on && self.print.on {
            self.toggle_print_preview();
        }
    }

    /// The system's print dialog, then the selected images.
    pub fn print_selection(&mut self) {
        if self.print.bar {
            self.leave_print_bar();
        }
        self.print_to(PrintTo::Printer);
    }

    /// The pages of the print: the selected images in browser order, and which one is shown.
    pub(crate) fn print_pages(&self) -> (usize, usize) {
        let pages = self.selected_paths();
        // The image asked for (it may still be loading).
        let shown = self.primary_path();
        (pages.iter().position(|p| Some(p) == shown.as_ref()).unwrap_or(0), pages.len())
    }

    /// The previous or next page of the print: the selection stays.
    pub(crate) fn print_page_step(&mut self, delta: isize) {
        let pages = self.selected_paths();
        let (at, n) = self.print_pages();
        if n > 1 {
            let next = (at as isize + delta).clamp(0, n as isize - 1) as usize;
            self.make_primary(&pages[next]);
        }
    }

    /// The print bar at the bottom of the viewer: pages, paper, orientation and fit, then
    /// "Drucken …" (the system's dialog) and "Abbrechen".
    pub(crate) fn print_bar(&mut self, ui: &mut egui::Ui, at: egui::Pos2) {
        let mut s = self.print.settings.clone();
        let (page, pages) = self.print_pages();
        let mut step = 0;
        let blocked = self.print_blocked_reason(false);
        let apply = (format!("{}  Drucken …", crate::theme::icon::PRINTER), "Enter".to_string());
        let done = crate::crop::image_bar_at(ui, at, apply, blocked.is_none(), |ui| {
            if pages > 1 {
                if ui.add_enabled(page > 0, egui::Button::new(crate::theme::icon::CARET_LEFT)).on_hover_text("Vorherige Seite").clicked() {
                    step = -1;
                }
                ui.label(format!("Seite {} von {pages}", page + 1));
                if ui.add_enabled(page + 1 < pages, egui::Button::new(crate::theme::icon::CARET_RIGHT)).on_hover_text("Nächste Seite").clicked() {
                    step = 1;
                }
                ui.separator();
            }
            let combo = egui::ComboBox::from_id_salt("print_bar_paper").selected_text(s.format.label()).width(110.0).show_ui(ui, |ui| {
                for (name, f) in sk_core::print::PRESETS {
                    ui.selectable_value(&mut s.format, f, name);
                }
            });
            combo.response.on_hover_text("Papierformat");
            let combo = egui::ComboBox::from_id_salt("print_bar_size").selected_text(crate::print_preview::image_size_label(s.image_size)).width(110.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut s.image_size, None, crate::print_preview::image_size_label(None));
                for (name, f) in crate::print_preview::PHOTO_SIZES {
                    ui.selectable_value(&mut s.image_size, Some(f), name);
                }
            });
            combo.response.on_hover_text("Bildgröße auf dem Papier");
            if let Some(turn) = crate::widgets::orientation_or_auto(ui, s.portrait, "Wie das Bild") {
                s.portrait = turn;
            }
            let combo = egui::ComboBox::from_id_salt("print_bar_fit").selected_text(s.fit.label()).width(90.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut s.fit, sk_core::print::Fit::Contain, sk_core::print::Fit::Contain.label()).on_hover_text("Ganzes Bild, Rand wo die Seitenverhältnisse abweichen");
                ui.selectable_value(&mut s.fit, sk_core::print::Fit::Fill, sk_core::print::Fit::Fill.label()).on_hover_text("Randlos wie im Fotolabor, das Bild wird beschnitten");
            });
            combo.response.on_hover_text("Anpassung");
            if !CAN_PRINT {
                ui.label(egui::RichText::new("Drucken nur unter Windows").small().color(crate::theme::TEXT_DIM));
            }
        });
        if s != self.print.settings {
            self.print.settings = s;
            self.save_settings();
        }
        if step != 0 {
            self.print_page_step(step);
        }
        match done {
            Some(true) => self.print_selection(),
            Some(false) => self.leave_print_bar(),
            None => {}
        }
    }

    /// "In Datei drucken …": asks for the file (PDF or JPEG).
    pub fn print_to_file_dialog(&mut self) {
        let name = self.selected_paths().first().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Druck".into());
        let mut dialog = rfd::FileDialog::new().set_title("In Datei drucken").set_file_name(format!("{name}.pdf")).add_filter("PDF", &["pdf"]).add_filter("JPEG", &["jpg", "jpeg"]);
        if let Some(dir) = self.selected_paths().first().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.save_file() {
            self.print_to(PrintTo::File(path));
        }
    }

    /// Why printing is not possible right now (on the disabled buttons), or `None`.
    pub fn print_blocked_reason(&self, to_file: bool) -> Option<&'static str> {
        if !to_file && !CAN_PRINT {
            Some("Drucken gibt es nur unter Windows; „In Datei …“ schreibt ein PDF")
        } else if self.print.job.is_some() {
            Some("Es wird bereits gedruckt")
        } else if self.current.is_none() && self.selected_paths().is_empty() {
            Some("Kein Bild ausgewählt")
        } else {
            None
        }
    }

    pub fn print_to(&mut self, to: PrintTo) {
        if let Some(reason) = self.print_blocked_reason(to != PrintTo::Printer) {
            self.status = reason.into();
            return;
        }
        let mut items = self.selection_for_output();
        // Nothing selected: the image on screen.
        if items.is_empty()
            && let Some(c) = &self.current
        {
            items.push((c.meta.path.clone(), c.committed.clone()));
        }
        if items.is_empty() {
            return;
        }
        log::info!("Drucken: {} Bild(er) {}", items.len(), match &to {
            PrintTo::Printer => "an den Drucker".to_string(),
            PrintTo::File(f) => format!("in {}", f.display()),
        });
        let settings = self.print.settings.clone();
        // The dialog shows the paper turned like the image on screen.
        let paper = match self.full_output_size() {
            Some((w, h)) => sk_core::print::layout(w, h, &settings).paper,
            None => [settings.format.w_mm.min(settings.format.h_mm), settings.format.w_mm.max(settings.format.h_mm)],
        };
        // The printer is chosen here, on the UI thread that owns the window; the job opens it.
        let printer = match &to {
            PrintTo::File(_) => None,
            PrintTo::Printer => match self.choose_printer(paper, settings.profile.is_some()) {
                Ok(Some(chosen)) => Some(chosen),
                Ok(None) => {
                    self.status = "Drucken abgebrochen".into();
                    return;
                }
                Err(e) => {
                    self.report_error(format!("Drucken fehlgeschlagen: {e:#}"));
                    return;
                }
            },
        };
        let shown = self.current.as_ref().and_then(|c| Some((c.meta.clone(), c.full.clone()?)));
        self.save_settings();
        let note = self.print.note.clone();
        *note.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.start_job(LABEL, Box::new(move |progress| run(items, settings, shown, to, paper, printer, &note, progress)));
        self.print.job = self.jobs.last().map(|j| j.id);
    }

    /// The system's print dialog for `paper` (turned like the page); `None` if cancelled.
    /// `SILBERKORN_PRINT_TEST="Drucker|Datei"`: that printer without the dialog, writing to the
    /// file (tests with "Microsoft Print to PDF").
    fn choose_printer(&self, paper: [f32; 2], own_colors: bool) -> Result<Option<(Choice, Option<PathBuf>)>> {
        let req = Request { paper_mm: paper, own_colors };
        if let Some((name, out)) = std::env::var("SILBERKORN_PRINT_TEST").ok().and_then(|v| v.split_once('|').map(|(a, b)| (a.to_string(), PathBuf::from(b)))) {
            return Ok(Some((crate::printer::named(&name, &req)?, Some(out))));
        }
        Ok(crate::printer::choose(self.native_window.unwrap_or(0), &req)?.map(|c| (c, None)))
    }

    /// The print job ended: what came out.
    pub(crate) fn print_job_done(&mut self, id: u64, result: &Result<Vec<PathBuf>, String>) -> bool {
        if self.print.job != Some(id) {
            return false;
        }
        self.print.job = None;
        self.jobs.retain(|j| j.id != id);
        log::info!("Drucken: {result:?}");
        match result {
            Ok(files) => {
                self.status = match files.as_slice() {
                    [] => self.print.note.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_else(|| "An den Drucker gesendet".into()),
                    [f] => format!("Gespeichert: {}", f.display()),
                    [f, ..] => format!("{} Dateien gespeichert in {}", files.len(), f.parent().map(|d| d.display().to_string()).unwrap_or_default()),
                }
            }
            Err(e) if e == crate::jobs::CANCELLED => self.status = "Drucken abgebrochen".into(),
            Err(e) => self.report_error(format!("Drucken fehlgeschlagen: {e}")),
        }
        true
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    items: Vec<(PathBuf, Adjustments)>,
    s: PrintSettings,
    shown: Option<(Arc<RawMeta>, Arc<LinearImage>)>,
    to: PrintTo,
    paper: [f32; 2],
    printer: Option<(Choice, Option<PathBuf>)>,
    note: &std::sync::Mutex<Option<String>>,
    progress: &dyn Fn(f32),
) -> Result<Vec<PathBuf>> {
    let profile = s.profile.as_ref().map(|p| PrinterProfile::new(p, s.intent)).transpose()?;
    let space = if profile.is_some() { PROFILE_SOURCE } else { OutputSpace::Srgb };
    let develop = |path: &PathBuf, adj: &Adjustments| -> Result<(CaptureInfo, LinearImage)> {
        let adj = crate::workers::output_adjustments(path, adj);
        let loaded = shown.as_ref().filter(|(m, _)| m.path == *path).map(|(m, f)| (&**m, &**f));
        sk_core::export::develop(path, loaded, &adj, space)
    };
    let n = items.len() as f32;
    match to {
        PrintTo::Printer => {
            let Some((choice, output)) = printer else { bail!("Kein Drucker gewählt") };
            let mut printer = Printer::open(&choice, output.as_deref())?;
            progress(0.0);
            // A driver that doesn't know the paper prints on its own: the layout follows it.
            let got = printer.paper_mm();
            let same = |a: [f32; 2], b: [f32; 2]| (a[0].min(a[1]) - b[0].min(b[1])).abs() < 3.0 && (a[0].max(a[1]) - b[0].max(b[1])).abs() < 3.0;
            if !same(got, paper) {
                let size = sk_core::print::PrintFormat::new(got[0], got[1]).label();
                let wanted = sk_core::print::PrintFormat::new(paper[0], paper[1]).label();
                *note.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!("An den Drucker gesendet, auf {size}: Er bietet {wanted} nicht an, die Seite ist dafür neu aufgeteilt"));
            }
            let title = match items.as_slice() {
                [(p, _)] => format!("Silberkorn – {}", p.file_name().unwrap_or_default().to_string_lossy()),
                _ => format!("Silberkorn – {} Bilder", items.len()),
            };
            printer.start(&title)?;
            let dpi = crate::printer::page_dpi(printer.dpi());
            for (i, (path, adj)) in items.iter().enumerate() {
                let (info, img) = develop(path, adj)?;
                progress((i as f32 + 0.6) / n);
                // "Wie das Bild": the page is turned like the image even where the printer's sheet
                // lies the other way for the whole job; then it goes to the printer turned.
                let sheet = printer.paper_mm();
                let across = |a: f32, b: f32| (a - b).abs() > 1.0 && a > b;
                let turn = s.portrait.is_none() && across(img.width as f32, img.height as f32) != across(sheet[0], sheet[1]) && img.width != img.height;
                let paper = if turn { [sheet[1], sheet[0]] } else { sheet };
                let layout = sk_core::print::layout_on(img.width as f32, img.height as f32, paper, &s);
                let page = print_page::render(&img, &layout, dpi, &s, s.caption.text(path, &info).as_deref(), profile.as_ref())?;
                drop(img);
                let page = if turn { page.turned() } else { page };
                for _ in 0..printer.copies() {
                    printer.page(&page)?;
                }
                progress((i + 1) as f32 / n);
            }
            printer.finish()?;
            Ok(Vec::new())
        }
        PrintTo::File(file) => {
            let icc = print_page::page_icc(profile.as_ref());
            let pdf = !file.extension().is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"));
            let mut pages = Vec::new();
            let mut written = Vec::new();
            for (i, (path, adj)) in items.iter().enumerate() {
                let (info, img) = develop(path, adj)?;
                progress((i as f32 + 0.6) / n);
                let layout = sk_core::print::layout(img.width as f32, img.height as f32, &s);
                let page = print_page::render(&img, &layout, FILE_DPI, &s, s.caption.text(path, &info).as_deref(), profile.as_ref())?;
                drop(img);
                let jpeg = print_page::jpeg(&page, &icc)?;
                if pdf {
                    pages.push((jpeg, page.width, page.height, page.paper));
                } else {
                    // One file per page: numbered when there are several.
                    let out = if items.len() == 1 {
                        file.clone()
                    } else {
                        let stem = file.file_stem().unwrap_or_default().to_string_lossy().to_string();
                        let ext = file.extension().unwrap_or_default().to_string_lossy().to_string();
                        file.with_file_name(format!("{stem}-{}.{ext}", i + 1))
                    };
                    sk_core::safe_file::write(&out, &jpeg)?;
                    written.push(out);
                }
                progress((i + 1) as f32 / n);
            }
            if pdf {
                sk_core::safe_file::write(&file, &print_page::pdf(&pages, &icc)?)?;
                written.push(file);
            }
            Ok(written)
        }
    }
}
