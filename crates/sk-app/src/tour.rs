//! Tutorials: short tours that point at the part of the program to use next and go on when
//! the step is done (or with "Weiter"). The program stays usable all the time: the tour only
//! dims the rest and explains.
//!
//! Widgets that a tour points at register their place each frame with [`target`]: the tool
//! sections by their title (`section:Belichtung`), the tabs (`tab:Ebenen`) and a few buttons.

use std::collections::HashMap;
use std::path::PathBuf;

use egui::{Color32, Rect, RichText, Stroke, vec2};

use crate::app::{App, Tab};
use crate::dialog::{self, Dialog};
use crate::layers::LocalTool;
use crate::theme::{self, icon};

/// Where a widget a tour may point at was drawn this frame.
pub fn target(ctx: &egui::Context, id: impl Into<String>, rect: Rect) {
    let id = id.into();
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Targets>(targets_id()).0.insert(id, rect));
}

#[derive(Clone, Default)]
struct Targets(HashMap<String, Rect>);

fn targets_id() -> egui::Id {
    egui::Id::new("tour_targets")
}

pub struct Step {
    /// What is pointed at (see [`target`]); `None`: a step without a place (start, end).
    pub target: Option<&'static str>,
    pub title: &'static str,
    /// May hold `{taste:command-id}` (see `help`).
    pub text: &'static str,
    /// The step is done when this holds (then the tour goes on by itself); without it, "Weiter".
    pub done: Option<fn(&App) -> bool>,
    /// Run when the step starts (e.g. opening the tab the step is about).
    pub enter: Option<fn(&mut App)>,
}

pub struct Tour {
    pub id: &'static str,
    pub title: &'static str,
    pub icon: &'static str,
    pub summary: &'static str,
    pub steps: &'static [Step],
}

const fn step(target: Option<&'static str>, title: &'static str, text: &'static str) -> Step {
    Step { target, title, text, done: None, enter: None }
}

fn has_image(a: &App) -> bool {
    a.current.is_some()
}

/// The image's settings changed since the step began (on the same image).
fn edited(a: &App) -> bool {
    match (&a.current, &a.tour.base) {
        (Some(c), Some((path, adj))) => c.meta.path == *path && c.adjustments != *adj,
        _ => false,
    }
}

pub const TOURS: &[Tour] = &[
    Tour {
        id: "erste-schritte",
        title: "Erste Schritte",
        icon: icon::SHOOTING_STAR,
        summary: "Ein Bild bewerten, entwickeln, vergleichen und exportieren.",
        steps: &[
            step(None, "Willkommen", "Diese Tour zeigt in wenigen Schritten den Weg vom RAW zum fertigen Bild. Alles bleibt dabei bedienbar; die Tour geht weiter, sobald ein Schritt erledigt ist, oder mit **Weiter**."),
            Step {
                target: Some("filmstrip"),
                title: "Ein Bild wählen",
                text: "Unten im Filmstreifen ein Bild anklicken. Mit {taste:grid} wechselt die Ansicht zwischen Raster und Einzelbild.",
                done: Some(has_image),
                enter: None,
            },
            step(Some("rail"), "Die Tabs", "Links stehen die Werkzeuge nach Aufgaben: Bibliothek und Metadaten, dann Objektiv, Belichtung, Farbe, Details, Effekte, Ebenen und Retusche, zuletzt Stile, Verlauf und Ausgabe."),
            Step {
                target: None,
                title: "Bewerten",
                text: "Mit den Tasten {taste:rating-1} bis {taste:rating-5} bekommt das Bild Sterne. Gute Bilder lassen sich später danach filtern.",
                done: Some(|a| a.current.as_ref().is_some_and(|c| c.rating > 0)),
                enter: None,
            },
            Step {
                target: Some("tab:Belichtung"),
                title: "Zum Tab Belichtung",
                text: "Den Tab **Belichtung** öffnen.",
                done: Some(|a| a.tab == Tab::Exposure),
                enter: None,
            },
            Step {
                target: Some("auto-adjust"),
                title: "Automatisch anpassen",
                text: "**Automatisch anpassen** stellt Weißabgleich, Belichtung, Kontrast und Dynamik in einem Schritt ein, ein guter Anfang.",
                done: Some(edited),
                enter: Some(|a| a.tab = Tab::Exposure),
            },
            step(Some("section:Belichtung"), "Fein einstellen", "Die Regler verfeinern das Ergebnis. Ein Doppelklick auf einen Regler setzt ihn zurück, ↺ im Kopf das ganze Werkzeug. Das \u{24D8} im Kopf erklärt jedes Werkzeug, ein Klick darauf öffnet die Hilfe."),
            step(Some("compare"), "Vorher und Nachher", "Mit {taste:before} oder diesen Knöpfen das Original zum Vergleich zeigen, auch nebeneinander oder geteilt."),
            step(Some("export"), "Exportieren", "**Exportieren** schreibt die gewählten Bilder mit den Ausgaberezepten (Tab **Ausgabe**) als JPEG, TIFF oder PNG."),
            step(None, "Geschafft", "Jede Änderung wird sofort gespeichert, das RAW bleibt unverändert. Weitere Tutorials und das Handbuch ({taste:help}) stehen im Menü **Hilfe**."),
        ],
    },
    Tour {
        id: "ebenen",
        title: "Ebenen und Masken",
        icon: icon::STACK,
        summary: "Nur einen Teil des Bildes ändern: Ebene anlegen, Maske wählen, anpassen.",
        steps: &[
            step(None, "Ebenen", "Eine Ebene ändert nur einen Teil des Bildes, etwa den Himmel oder ein Gesicht. Was sie trifft, bestimmt ihre Maske."),
            Step {
                target: Some("tab:Ebenen"),
                title: "Zum Tab Ebenen",
                text: "Den Tab **Ebenen** öffnen.",
                done: Some(|a| a.tab == Tab::Layers),
                enter: None,
            },
            Step {
                target: Some("new-layer"),
                title: "Eine Ebene anlegen",
                text: "Mit **+** und **Neue Ebene** eine Ebene anlegen. Auch ein Maskenwerkzeug legt eine an, wenn noch keine gewählt ist.",
                done: Some(|a| a.current.as_ref().is_some_and(|c| !c.adjustments.layers.is_empty())),
                enter: Some(|a| a.tab = Tab::Layers),
            },
            step(Some("section:Maske"), "Die Maske", "Hier wird gewählt, was die Ebene trifft: mit dem Pinsel malen, einen Verlauf ziehen oder automatisch Motiv, Himmel oder einen Farbbereich wählen. {taste:show-mask} zeigt die Maske rot im Bild."),
            step(Some("section:Anpassungen"), "Anpassen", "Diese Regler wirken nur innerhalb der Maske. Auch die Tabs Belichtung, Farbe und Details wirken auf die gewählte Ebene; ein Punkt an ihrem Symbol zeigt das."),
            step(None, "Fertig", "Mehr über Masken steht im Handbuch im Kapitel **Ebenen und Masken**."),
        ],
    },
    Tour {
        id: "retusche",
        title: "Retusche und Entfernen",
        icon: icon::BANDAIDS,
        summary: "Flecken reparieren und störende Dinge entfernen.",
        steps: &[
            Step {
                target: Some("tab:Retusche"),
                title: "Zum Tab Retusche",
                text: "Den Tab **Retusche** öffnen.",
                done: Some(|a| a.tab == Tab::Retouch),
                enter: None,
            },
            step(Some("section:Werkzeuge"), "Drei Werkzeuge", "**Reparieren** für kleine Flecken, **Klonen** kopiert eine Stelle genau, **Entfernen** füllt auch größere Bereiche mit KI passend zur Umgebung."),
            Step {
                target: Some("section:Werkzeuge"),
                title: "Entfernen wählen",
                text: "**Entfernen** wählen (Taste {taste:tool-remove}).",
                done: Some(|a| a.local_tool == Some(LocalTool::Remove)),
                enter: None,
            },
            Step {
                target: Some("viewer"),
                title: "Darübermalen",
                text: "Im Bild über das Störende malen. Die Pinselgröße ändern die Tasten {taste:brush-smaller} und {taste:brush-larger} oder Ziehen mit der rechten Maustaste.",
                done: Some(|a| a.current.as_ref().is_some_and(|c| !c.adjustments.removals.is_empty() || !c.adjustments.retouch.is_empty())),
                enter: None,
            },
            step(Some("section:Retuschen"), "Die Retuschen", "Jede Retusche bleibt einzeln änderbar: anklicken, dann verschieben, nachbessern oder löschen."),
            step(None, "Fertig", "Mehr im Handbuch im Kapitel **Retusche**."),
        ],
    },
    Tour {
        id: "bibliothek",
        title: "Bibliothek, Katalog und Sessions",
        icon: icon::IMAGES,
        summary: "Bilder importieren, ordnen und wiederfinden.",
        steps: &[
            step(None, "Drei Arten zu arbeiten", "**Ordner** zeigt einfach die Bilder eines Ordners. Der **Katalog** nimmt Bilder aus vielen Ordnern auf und findet sie wieder. Eine **Session** ist ein Projektordner mit eigenen Unterordnern, etwa für ein Shooting."),
            step(Some("import"), "Importieren", "Hier werden Bilder in den Katalog oder die geöffnete Session übernommen, auf Wunsch kopiert und umbenannt. Beim Einstecken einer Speicherkarte öffnet sich der Import von selbst."),
            Step {
                target: Some("tab:Bibliothek"),
                title: "Zum Tab Bibliothek",
                text: "Den Tab **Bibliothek** öffnen.",
                done: Some(|a| a.tab == Tab::Library),
                enter: None,
            },
            step(Some("section:Filter & Suche"), "Filtern und suchen", "Nach Sternen, Farbmarkierung, Kamera, Datum oder Text filtern. Alben sammeln Bilder, Smart-Alben tun es nach Regeln von selbst."),
            step(None, "Fertig", "Mehr im Handbuch im Kapitel **Bibliothek**."),
        ],
    },
];

/// The job that downloads the example image.
const DOWNLOAD: &str = "Beispielbild laden";

/// The running tour.
#[derive(Default)]
pub struct TourState {
    pub running: Option<(usize, usize)>,
    pub dialog_open: bool,
    /// The step whose `enter` already ran.
    entered: Option<(usize, usize)>,
    /// The image and its settings when the step began (or when that image was opened).
    base: Option<(PathBuf, sk_core::params::Adjustments)>,
    /// The step was already done when it began (or was gone back to): then it does not go on
    /// by itself, only with "Weiter".
    done_on_entry: bool,
    /// The download of the example image (job id), for a tour started without an image.
    download: Option<u64>,
}

/// The example image of the tours: a raw file of the author's (a mountain lake under clouds),
/// provided with a release of Silberkorn and downloaded the first time a tour starts without an
/// open image.
pub const EXAMPLE: sk_ai::models::ModelInfo = sk_ai::models::ModelInfo {
    label: "Beispielbild",
    file: "DSC09197.ARW",
    url: "https://github.com/mschneider85/silberkorn/releases/download/beispiel-1/DSC09197.ARW",
    bytes: 25_324_288,
    sha256: "05194398cbeaf06b38f82cddd83b5916197109468caa38f832cb7af056ea8b47",
    license: "Martin Schneider, zur Nutzung mit Silberkorn",
    entry: None,
};

/// Where the example image is kept (its own folder, so that it is opened alone).
fn example_dir() -> PathBuf {
    sk_core::paths::data_dir().join("Beispiel")
}

/// The folder with the example image, once it is downloaded.
pub fn example_folder() -> Option<PathBuf> {
    let dir = example_dir();
    std::fs::metadata(dir.join(EXAMPLE.file)).is_ok_and(|m| m.len() == EXAMPLE.bytes).then_some(dir)
}

impl App {
    pub fn start_tour(&mut self, i: usize) {
        self.tour.dialog_open = false;
        self.tour.running = Some((i, 0));
        self.tour.entered = None;
        // Without an open image the example is opened, downloaded first if need be.
        if self.current.is_none() {
            match example_folder() {
                Some(dir) => self.open_folder(dir),
                None if self.tour.download.is_none() => {
                    let target = example_dir().join(EXAMPLE.file);
                    self.start_job(DOWNLOAD, Box::new(move |progress| {
                        sk_ai::models::download_to(&EXAMPLE, &target, progress)?;
                        Ok(Vec::new())
                    }));
                    self.tour.download = self.jobs.last().map(|j| j.id);
                }
                None => {}
            }
        }
    }

    /// The example image arrived: opened, if the tour still waits for an image.
    pub(crate) fn example_job_done(&mut self, id: u64, result: &Result<Vec<PathBuf>, String>) -> bool {
        if self.tour.download != Some(id) {
            return false;
        }
        self.tour.download = None;
        self.jobs.retain(|j| j.id != id);
        match result {
            Ok(_) => {
                self.status = "Beispielbild geladen".into();
                if self.tour.running.is_some()
                    && self.current.is_none()
                    && let Some(dir) = example_folder()
                {
                    self.open_folder(dir);
                }
            }
            Err(e) if e == crate::jobs::CANCELLED => self.status = format!("{DOWNLOAD} abgebrochen"),
            Err(e) => self.report_error(format!("{DOWNLOAD} fehlgeschlagen: {e}")),
        }
        true
    }

    /// "Tutorials …": the tours to choose from.
    pub(crate) fn tutorials_dialog(&mut self, ctx: &egui::Context) {
        if !self.tour.dialog_open {
            return;
        }
        let mut start = None;
        let mut close = false;
        let modal = Dialog::new("tutorials", icon::GRADUATION_CAP, "Tutorials").subtitle("Kurze Touren durch das Programm, Schritt für Schritt").width(520.0).show(ctx, |ui| {
            for (i, t) in TOURS.iter().enumerate() {
                dialog::card(ui, &format!("{}  {}", t.icon, t.title), None, |ui| {
                    ui.horizontal(|ui| {
                        // The summary wraps in the room the button leaves.
                        let w = ui.available_width() - 80.0;
                        ui.allocate_ui(vec2(w, 0.0), |ui| ui.add(egui::Label::new(RichText::new(t.summary).color(theme::TEXT_DIM)).wrap()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Starten").clicked() {
                                start = Some(i);
                            }
                        });
                    });
                });
            }
            if self.current.is_none() && example_folder().is_none() {
                dialog::note(ui, format!("Ohne geöffnetes Bild laden die Touren ein Beispielbild herunter ({} MB).", EXAMPLE.size_mb()));
            }
            dialog::footer(ui, |ui| {
                if ui.add(dialog::secondary("Schließen")).clicked() {
                    close = true;
                }
            });
        });
        if let Some(i) = start {
            self.start_tour(i);
        } else if close || modal.should_close() {
            self.tour.dialog_open = false;
        }
    }

    /// The running tour over everything: the rest dimmed, the target outlined, the explanation
    /// beside it. Drawn last in the frame, when the targets of this frame are known.
    pub(crate) fn tour_overlay(&mut self, ctx: &egui::Context) {
        let targets = ctx.data_mut(|d| d.remove_temp::<Targets>(targets_id())).unwrap_or_default();
        let Some((t, s)) = self.tour.running else { return };
        let Some(tour) = TOURS.get(t) else {
            self.tour.running = None;
            return;
        };
        let Some(step) = tour.steps.get(s) else {
            self.tour.running = None;
            return;
        };
        let fresh = self.tour.entered != Some((t, s));
        if fresh {
            self.tour.entered = Some((t, s));
            self.tour.base = None;
            if let Some(enter) = step.enter {
                enter(self);
            }
        }
        // Another image (or the first one loaded) is where changes are counted from.
        if let Some(c) = &self.current
            && self.tour.base.as_ref().is_none_or(|(p, _)| *p != c.meta.path)
        {
            self.tour.base = Some((c.meta.path.clone(), c.adjustments.clone()));
        }
        // Done: on to the next step (the check is shown for a moment).
        let done = step.done.is_some_and(|d| d(self));
        if fresh {
            self.tour.done_on_entry = done;
        }
        let screen = ctx.content_rect();
        let rect = step.target.and_then(|id| targets.0.get(id).copied()).filter(|r| r.is_positive());

        // Dimmed around the target; nothing takes the clicks, the program stays usable.
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("tour_dim")));
        let dim = Color32::from_black_alpha(110);
        match rect {
            Some(r) => {
                let r = r.expand(4.0);
                painter.rect_filled(Rect::from_min_max(screen.min, egui::pos2(screen.right(), r.top())), 0.0, dim);
                painter.rect_filled(Rect::from_min_max(egui::pos2(screen.left(), r.bottom()), screen.max), 0.0, dim);
                painter.rect_filled(Rect::from_min_max(egui::pos2(screen.left(), r.top()), egui::pos2(r.left(), r.bottom())), 0.0, dim);
                painter.rect_filled(Rect::from_min_max(egui::pos2(r.right(), r.top()), egui::pos2(screen.right(), r.bottom())), 0.0, dim);
                let pulse = (ctx.input(|i| i.time) * 3.0).sin() as f32 * 0.5 + 0.5;
                painter.rect_stroke(r, 6.0, Stroke::new(2.0 + pulse, theme::ACCENT), egui::StrokeKind::Outside);
                ctx.request_repaint();
            }
            None if step.target.is_none() => {
                painter.rect_filled(screen, 0.0, dim);
            }
            None => {}
        }

        // The explanation: beside the target where there is room, else in the middle.
        let size = vec2(340.0, 0.0);
        let pos = match rect {
            Some(r) if r.right() + 16.0 + size.x < screen.right() => egui::pos2(r.right() + 16.0, r.top().clamp(screen.top() + 8.0, screen.bottom() - 220.0)),
            Some(r) if r.left() - 16.0 - size.x > screen.left() => egui::pos2(r.left() - 16.0 - size.x, r.top().clamp(screen.top() + 8.0, screen.bottom() - 220.0)),
            Some(r) if r.bottom() + 230.0 < screen.bottom() => egui::pos2(r.center().x.clamp(screen.left() + 8.0, screen.right() - size.x - 8.0), r.bottom() + 16.0),
            Some(r) => egui::pos2(r.center().x.clamp(screen.left() + 8.0, screen.right() - size.x - 8.0), (r.top() - 230.0).max(screen.top() + 8.0)),
            None => screen.center() - vec2(size.x / 2.0, 100.0),
        };
        let (mut next, mut back, mut end) = (false, false, false);
        egui::Area::new(egui::Id::new("tour_card")).order(egui::Order::Tooltip).fixed_pos(pos).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).corner_radius(10).inner_margin(egui::Margin::same(14)).show(ui, |ui| {
                ui.set_width(size.x);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tour.icon).color(theme::ACCENT));
                    ui.label(RichText::new(format!("{} · {} von {}", tour.title, s + 1, tour.steps.len())).small().color(theme::TEXT_DIM));
                });
                ui.add_space(2.0);
                let title = if done { format!("{}  {}", icon::CHECK, step.title) } else { step.title.to_string() };
                ui.label(RichText::new(title).size(16.0).strong().color(Color32::WHITE));
                ui.add_space(4.0);
                for b in crate::help::parse(&crate::help::fill_keys(step.text)) {
                    if let crate::help::Block::Para(spans) = b {
                        crate::help::show_spans(ui, &spans);
                    }
                }
                let loading = self.tour.download.and_then(|id| self.jobs.iter().find(|j| j.id == id));
                if let Some(job) = loading {
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!("Beispielbild wird geladen … {:.0} %", job.progress * 100.0)).small().color(theme::TEXT_DIM));
                } else if step.target.is_some() && rect.is_none() {
                    ui.label(RichText::new("Die Stelle ist gerade nicht zu sehen.").small().color(theme::TEXT_FAINT));
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Beenden").clicked() {
                        end = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let last = s + 1 == tour.steps.len();
                        let label = if last { "Fertig" } else { "Weiter" };
                        let primary = done || step.done.is_none();
                        let button = if primary { dialog::primary(label.to_string()) } else { egui::Button::new(label) };
                        if ui.add(button).clicked() {
                            next = true;
                        }
                        if s > 0 && ui.button("Zurück").clicked() {
                            back = true;
                        }
                    });
                });
            });
        });
        // A step done goes on by itself after a moment.
        let key = egui::Id::new(("tour_done", t, s));
        if done && !self.tour.done_on_entry {
            let now = ctx.input(|i| i.time);
            let since = ctx.data_mut(|d| *d.get_temp_mut_or(key, now));
            if now - since > 1.2 {
                next = true;
            } else {
                ctx.request_repaint();
            }
        }
        if end {
            self.tour.running = None;
        } else if next {
            ctx.data_mut(|d| d.remove::<f64>(key));
            self.tour.running = (s + 1 < tour.steps.len()).then_some((t, s + 1));
        } else if back {
            self.tour.running = Some((t, s - 1));
            self.tour.entered = Some((t, s - 1));
            self.tour.done_on_entry = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every placeholder in the tours names a command.
    #[test]
    fn tour_texts_name_commands() {
        for t in TOURS {
            for s in t.steps {
                assert!(!crate::help::fill_keys(s.text).contains("{taste:"), "{} / {}", t.title, s.title);
            }
        }
    }
}
