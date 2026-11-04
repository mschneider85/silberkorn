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
use crate::printer::{Choice, PaperSize, Printer, Printers, Request};

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
    /// Strg+P: the print bar with the page in the print preview. Printing is up to the bar (Enter
    /// or its button), so a second Strg+P doesn't print unseen pages.
    pub fn print_command(&mut self) {
        if !self.print.bar {
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
        // Printers come and go: asked for again each time. Each page starts turned like its image.
        self.print.printers_asked = false;
        self.print.turned.clear();
    }

    /// The installed printers with their papers (Windows): asked for in the background the first
    /// time, `None` until known.
    pub(crate) fn printers(&mut self, ctx: &egui::Context) -> Option<Arc<Printers>> {
        if !CAN_PRINT {
            return None;
        }
        if !self.print.printers_asked {
            self.print.printers_asked = true;
            let slot = self.print.printers.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let found = crate::printer::printers();
                log::info!("Drucker: {:?}", found.list.iter().map(|(n, p)| format!("{n} ({} Papiere)", p.len())).collect::<Vec<_>>());
                *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(found));
                ctx.request_repaint();
            });
        }
        self.print.printers.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Esc: the print bar closes, the print preview goes back to how it was.
    pub fn leave_print_bar(&mut self) {
        self.print.bar = false;
        if !self.print.was_on && self.print.on {
            self.toggle_print_preview();
        }
    }

    /// The selected images to the printer chosen in the print bar.
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

    /// The print bar at the bottom of the viewer: pages, printer, paper, orientation, fit,
    /// the driver's settings, then "Drucken" and "Abbrechen".
    pub(crate) fn print_bar(&mut self, ui: &mut egui::Ui, at: egui::Pos2) {
        let printers = self.printers(ui.ctx());
        let mut s = self.print.settings.clone();
        let (page, pages) = self.print_pages();
        let mut step = 0;
        let blocked = self.print_blocked_reason(false);
        let mut driver = false;
        let size = self.full_output_size();
        let portrait = self.page_portrait();
        let mut turn = None;
        // The fit's effect is shown for the page as turned.
        s.portrait = Some(portrait);
        let apply = (format!("{}  Drucken", crate::theme::icon::PRINTER), "Enter".to_string());
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
            if CAN_PRINT {
                let combo = egui::ComboBox::from_id_salt("print_bar_printer").selected_text(printer_text(&s, printers.as_deref())).width(150.0).truncate().show_ui(ui, |ui| {
                    printer_entries(ui, &mut s, printers.as_deref());
                });
                combo.response.on_hover_text("Drucker");
            }
            let (text, hint) = paper_text(&s, printers.as_deref());
            let combo = egui::ComboBox::from_id_salt("print_bar_paper").selected_text(text).width(110.0).truncate().show_ui(ui, |ui| {
                paper_entries(ui, &mut s, printers.as_deref());
            });
            combo.response.on_hover_text(hint.unwrap_or_else(|| "Papierformat".into()));
            let combo = egui::ComboBox::from_id_salt("print_bar_size").selected_text(crate::print_preview::image_size_label(s.image_size)).width(110.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut s.image_size, None, crate::print_preview::image_size_label(None));
                for (name, f) in crate::print_preview::PHOTO_SIZES {
                    ui.selectable_value(&mut s.image_size, Some(f), name);
                }
            });
            combo.response.on_hover_text("Bildgröße auf dem Papier");
            if let Some(p) = crate::widgets::orientation(ui, true, portrait, "") {
                turn = Some(p);
            }
            let combo = egui::ComboBox::from_id_salt("print_bar_fit").selected_text(s.fit.label()).width(90.0).show_ui(ui, |ui| crate::print_preview::fit_entries(ui, &mut s, size));
            combo.response.on_hover_text(format!("Anpassung: {}", crate::print_preview::fit_effect(&s, s.fit, size)));
            if CAN_PRINT {
                ui.separator();
                if ui.button("Einstellungen …").on_hover_text("Einstellungen des Druckers: Papiersorte, Qualität, Farbe").clicked() {
                    driver = true;
                }
            } else {
                ui.label(egui::RichText::new("Drucken nur unter Windows").small().color(crate::theme::TEXT_DIM));
            }
        });
        if driver {
            self.printer_properties();
        }
        s.portrait = None;
        if let Some(p) = turn {
            self.turn_page(p);
        }
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
        let turned = self.print.turned.clone();
        // The printer is set up for the page on screen, turned as it is.
        let paper = match self.full_output_size() {
            Some((w, h)) => sk_core::print::layout(w, h, &self.page_settings()).paper,
            None => [settings.format.w_mm.min(settings.format.h_mm), settings.format.w_mm.max(settings.format.h_mm)],
        };
        // The printer is put together here, on the UI thread; the job opens it.
        let printer = match &to {
            PrintTo::File(_) => None,
            PrintTo::Printer => match self.printer_for_job(paper, &settings) {
                Ok(chosen) => Some(chosen),
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
        self.start_job(LABEL, Box::new(move |progress| run(items, settings, turned, shown, to, paper, printer, &note, progress)));
        self.print.job = self.jobs.last().map(|j| j.id);
    }

    /// What the printer is asked for: the paper (turned like the page) and colors.
    fn printer_request(&self, paper: [f32; 2], s: &PrintSettings) -> Request {
        Request { printer: s.printer.clone(), paper_mm: paper, own_colors: s.profile.is_some() }
    }

    /// The printer chosen in the print bar, with the driver's settings from "Einstellungen …".
    /// `SILBERKORN_PRINT_TEST="Drucker|Datei"`: that printer, writing to the file (tests with
    /// "Microsoft Print to PDF").
    fn printer_for_job(&self, paper: [f32; 2], s: &PrintSettings) -> Result<(Choice, Option<PathBuf>)> {
        let mut req = self.printer_request(paper, s);
        if let Some((name, out)) = std::env::var("SILBERKORN_PRINT_TEST").ok().and_then(|v| v.split_once('|').map(|(a, b)| (a.to_string(), PathBuf::from(b)))) {
            req.printer = Some(name);
            return Ok((crate::printer::prepared(&req, None)?, Some(out)));
        }
        Ok((crate::printer::prepared(&req, self.print.driver.as_ref())?, None))
    }

    /// "Einstellungen …": the driver's own dialog for the chosen printer; what is set there
    /// holds for this session (paper and orientation still come from the print bar).
    pub(crate) fn printer_properties(&mut self) {
        let s = self.page_settings();
        let paper = match self.full_output_size() {
            Some((w, h)) => sk_core::print::layout(w, h, &s).paper,
            None => [s.format.w_mm.min(s.format.h_mm), s.format.w_mm.max(s.format.h_mm)],
        };
        let req = self.printer_request(paper, &s);
        let result = crate::printer::prepared(&req, self.print.driver.as_ref()).and_then(|mut c| Ok(crate::printer::properties(self.native_window.unwrap_or(0), &mut c)?.then_some(c)));
        match result {
            Ok(Some(c)) => self.print.driver = Some(c),
            Ok(None) => {}
            Err(e) => self.report_error(format!("Druckereinstellungen: {e:#}")),
        }
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

/// The printer a setting means, for its field: by name, else the default one.
pub(crate) fn printer_text(s: &PrintSettings, printers: Option<&Printers>) -> String {
    match (&s.printer, printers.and_then(|p| p.default.as_ref())) {
        (Some(name), _) if printers.is_some_and(|p| p.papers(Some(name)).is_none()) => format!("{name} (nicht gefunden)"),
        (Some(name), _) => name.clone(),
        (None, Some(default)) => default.clone(),
        (None, None) if printers.is_none() => "Drucker werden gesucht …".into(),
        (None, None) => "Kein Drucker".into(),
    }
}

/// The installed printers to choose from; the default one marked.
pub(crate) fn printer_entries(ui: &mut egui::Ui, s: &mut PrintSettings, printers: Option<&Printers>) {
    let Some(printers) = printers else {
        ui.label("Drucker werden gesucht …");
        return;
    };
    if printers.list.is_empty() {
        ui.label("Kein Drucker installiert");
    }
    let shown = s.printer.clone().or_else(|| printers.default.clone());
    for (name, _) in &printers.list {
        let default = printers.default.as_ref() == Some(name);
        let r = ui.selectable_label(shown.as_ref() == Some(name), name);
        let r = if default { r.on_hover_text("Standarddrucker von Windows") } else { r };
        if r.clicked() {
            // The default printer is followed when Windows' default changes.
            s.printer = (!default).then(|| name.clone());
        }
    }
}

/// The paper field: the printer's name for it, and a warning if the printer doesn't offer it.
pub(crate) fn paper_text(s: &PrintSettings, printers: Option<&Printers>) -> (egui::RichText, Option<String>) {
    let papers = printers.and_then(|p| p.papers(s.printer.as_deref())).filter(|p| !p.is_empty());
    match papers.map(|p| crate::printer::paper_of(p, s.format)) {
        Some(Some(paper)) => (egui::RichText::new(&paper.name), None),
        Some(None) => (
            egui::RichText::new(s.format.label()).color(crate::theme::GUIDE),
            Some(format!("{} führt {} nicht als Papier. Nimmt er kein eigenes Format an, druckt er auf sein Standardpapier.", printer_text(s, printers), s.format.label())),
        ),
        None => (egui::RichText::new(s.format.label()), None),
    }
}

/// The papers to choose from: the printer's, else the usual sizes.
pub(crate) fn paper_entries(ui: &mut egui::Ui, s: &mut PrintSettings, printers: Option<&Printers>) {
    let papers: &[PaperSize] = printers.and_then(|p| p.papers(s.printer.as_deref())).unwrap_or(&[]);
    if papers.is_empty() {
        for (name, f) in sk_core::print::PRESETS {
            ui.selectable_value(&mut s.format, f, name);
        }
        return;
    }
    let chosen = crate::printer::paper_of(papers, s.format).map(|p| p.name.clone());
    let cm = |mm: f32| if (mm / 10.0).fract().abs() < 0.05 { format!("{:.0}", mm / 10.0) } else { format!("{:.1}", mm / 10.0) };
    for p in papers {
        let label = format!("{}  ({} × {} cm)", p.name, cm(p.mm[0]), cm(p.mm[1]));
        if ui.selectable_label(chosen.as_ref() == Some(&p.name), label).clicked() {
            s.format = p.format();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    items: Vec<(PathBuf, Adjustments)>,
    s: PrintSettings,
    turned: std::collections::HashMap<PathBuf, bool>,
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
                // Each page lies like its image, or as turned by hand, even where the printer's
                // sheet lies the other way for the whole job; then it goes to the printer turned.
                let sheet = printer.paper_mm();
                let upright = turned.get(path).copied().unwrap_or(img.height > img.width);
                let turn = (sheet[0] - sheet[1]).abs() > 1.0 && upright != (sheet[1] > sheet[0]);
                let paper = if turn { [sheet[1], sheet[0]] } else { sheet };
                let layout = sk_core::print::layout_on(img.width as f32, img.height as f32, paper, &s);
                let page = print_page::render(&img, &layout, dpi, &s, s.caption.text(path, &info).as_deref(), profile.as_ref())?;
                drop(img);
                let page = if turn { page.turned() } else { page };
                printer.page(&page)?;
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
                let page = PrintSettings { portrait: turned.get(path).copied(), ..s.clone() };
                let layout = sk_core::print::layout(img.width as f32, img.height as f32, &page);
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
