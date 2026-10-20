//! What the app is busy with: everything that runs longer than a moment, shown in the status
//! bar (spinner, label, progress where known) and, when the user waits for it in the viewer,
//! in the viewer too.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use egui::{Color32, Rect, RichText, Ui, pos2, vec2};

use crate::app::App;

/// A job's name as it runs, verb first like the other activities ("Update laden" becomes
/// "Lade Update"). The name itself stays for the messages after it ("Update laden fehlgeschlagen").
pub(crate) fn running(label: &str) -> String {
    if let Some(what) = label.strip_suffix(" laden") {
        format!("Lade {what}")
    } else if let Some(what) = label.strip_suffix(" zusammenfügen") {
        format!("Füge {what} zusammen")
    } else {
        label.to_string()
    }
}

/// A running activity.
pub struct Activity {
    pub label: String,
    /// 0..1 if known.
    pub progress: Option<f32>,
    /// The user waits for it in the viewer (shown there too).
    pub in_viewer: bool,
    /// Stops it; `None` if it can't be stopped (short or not interruptible).
    pub cancel: Option<Arc<AtomicBool>>,
}

impl App {
    pub fn activities(&self) -> Vec<Activity> {
        let mut v = Vec::new();
        if self.inpaint_busy {
            v.push(Activity { label: "Entfernen".into(), progress: Some(self.removal_progress), in_viewer: true, cancel: None });
        }
        if self.ai_busy {
            v.push(Activity { label: "Berechne Maske".into(), progress: None, in_viewer: true, cancel: None });
        }
        if let Some((done, total)) = self.look_progress {
            v.push(Activity { label: "Look angleichen".into(), progress: Some(done as f32 / total.max(1) as f32), in_viewer: true, cancel: self.look_cancel.clone() });
        }
        if let Some((done, total)) = self.auto_progress {
            v.push(Activity { label: "Automatisch anpassen".into(), progress: Some(done as f32 / total.max(1) as f32), in_viewer: true, cancel: self.auto_cancel.clone() });
        }
        if self.spot_search.is_some() {
            v.push(Activity { label: "Suche Staub und Flecken".into(), progress: None, in_viewer: false, cancel: self.spot_cancel.clone() });
        }
        for j in &self.jobs {
            v.push(Activity { label: running(&j.label), progress: Some(j.progress), in_viewer: false, cancel: Some(j.cancel.clone()) });
        }
        if let Some((done, total)) = self.export_progress {
            v.push(Activity {
                label: format!("Exportiere {done} / {total}"),
                progress: self.export_fraction(),
                in_viewer: false,
                cancel: self.export_cancel.clone(),
            });
        }
        if self.folder_loading {
            v.push(Activity { label: "Lade Ordner".into(), progress: None, in_viewer: false, cancel: None });
        }
        if let Some((done, total)) = self.index_progress {
            v.push(Activity { label: format!("Indexiere {done} / {total}"), progress: Some(done as f32 / total.max(1) as f32), in_viewer: false, cancel: None });
        }
        if self.lens_db_downloading {
            v.push(Activity { label: "Lade Objektivprofile".into(), progress: None, in_viewer: false, cancel: None });
        }
        v
    }

    /// A cancel switch was set (X in the status bar or a panel): the work ends here; what was
    /// finished stays.
    pub(crate) fn poll_cancelled(&mut self) {
        let set = |c: &Option<Arc<AtomicBool>>| c.as_ref().is_some_and(|c| c.load(Ordering::Relaxed));
        if set(&self.auto_cancel) {
            self.auto_cancel = None;
            // The image in work when cancelled no longer counts (see `auto_adjusted`).
            self.auto_run += 1;
            if let Some((done, total)) = self.auto_progress.take() {
                self.status = format!("Automatisch anpassen abgebrochen ({done} von {total} fertig)");
            }
        }
        if set(&self.look_cancel) {
            self.look_cancel = None;
            self.look_run += 1;
            if let Some((done, total)) = self.look_progress.take() {
                self.status = format!("Look angleichen abgebrochen ({done} von {total} fertig)");
            }
        }
        if set(&self.spot_cancel) {
            self.spot_cancel = None;
            if self.spot_search.take().is_some() {
                self.status = "Suche nach Staub und Flecken abgebrochen".into();
            }
        }
    }

    /// Status bar: one activity with a spinner (and its progress), the others in the tooltip.
    /// One that can be cancelled comes first, so that its X is always within reach.
    pub(crate) fn activity_status(&self, ui: &mut Ui) {
        let all = self.activities();
        let Some(first) = all.iter().find(|a| a.cancel.is_some()).or(all.first()) else { return };
        // The status bar's right part is laid out from right to left: last item first.
        let resp = ui
            .horizontal(|ui| {
                if let Some(cancel) = &first.cancel {
                    let stopping = cancel.load(Ordering::Relaxed);
                    let b = ui.add_enabled(!stopping, egui::Button::new(RichText::new(crate::theme::icon::X).size(11.0)).small().frame(false));
                    if b.on_hover_text(if stopping { "Wird abgebrochen …" } else { "Abbrechen" }).clicked() {
                        cancel.store(true, Ordering::Relaxed);
                    }
                }
                if let Some(p) = first.progress {
                    ui.label(RichText::new(format!("{:.0} %", p * 100.0)).size(11.0).color(crate::theme::TEXT_DIM));
                    ui.add(egui::ProgressBar::new(p).desired_width(110.0).desired_height(10.0));
                }
                let more = if all.len() > 1 { format!("  (+{})", all.len() - 1) } else { String::new() };
                ui.label(RichText::new(format!("{}{more}", first.label)).size(12.0).color(crate::theme::TEXT));
                spinner(ui, 14.0);
            })
            .response;
        let text: Vec<String> =
            all.iter().map(|a| match a.progress { Some(p) => format!("{} – {:.0} %", a.label, p * 100.0), None => a.label.clone() }).collect();
        resp.on_hover_text(text.join("\n"));
    }

    /// Viewer: a small panel at the top for what the user waits for there.
    pub(crate) fn activity_overlay(&self, ui: &Ui, rect: Rect) {
        let Some(a) = self.activities().into_iter().find(|a| a.in_viewer) else { return };
        let text = match a.progress {
            Some(p) => format!("{} …  {:.0} %", a.label, p * 100.0),
            None => format!("{} …", a.label),
        };
        let painter = ui.painter_at(rect);
        let galley = painter.layout_no_wrap(text, egui::FontId::proportional(13.0), Color32::WHITE);
        let spinner = 14.0;
        let w = spinner + 8.0 + galley.size().x;
        let top = pos2(rect.center().x - w / 2.0, rect.top() + 14.0);
        let bar_h = if a.progress.is_some() { 4.0 } else { 0.0 };
        let bg = Rect::from_min_size(top, vec2(w, galley.size().y.max(spinner) + bar_h)).expand2(vec2(12.0, 6.0));
        painter.rect_filled(bg, 8.0, Color32::from_black_alpha(185));
        let c = top + vec2(spinner / 2.0, galley.size().y.max(spinner) / 2.0);
        paint_arc(ui, &painter, c, spinner);
        painter.galley(top + vec2(spinner + 8.0, (spinner - galley.size().y).max(0.0) / 2.0), galley.clone(), Color32::WHITE);
        if let Some(p) = a.progress {
            let y = bg.bottom() - 6.0;
            let track = Rect::from_min_max(pos2(bg.left() + 10.0, y - 2.0), pos2(bg.right() - 10.0, y));
            painter.rect_filled(track, 1.0, Color32::from_white_alpha(40));
            painter.rect_filled(Rect::from_min_max(track.min, pos2(track.left() + track.width() * p.clamp(0.0, 1.0), track.bottom())), 1.0, crate::theme::ACCENT);
        }
        ui.ctx().request_repaint();
    }
}

/// A rotating arc ("busy"), centered at `c`.
fn paint_arc(ui: &Ui, painter: &egui::Painter, c: egui::Pos2, size: f32) {
    let t = ui.input(|i| i.time) as f32;
    let pts: Vec<egui::Pos2> = (0..=16)
        .map(|k| {
            let a = t * 6.0 + k as f32 / 16.0 * 4.5;
            c + vec2(a.cos(), a.sin()) * (size / 2.0 - 1.0)
        })
        .collect();
    painter.add(egui::Shape::line(pts, egui::Stroke::new(2.0, crate::theme::ACCENT)));
    ui.ctx().request_repaint();
}

fn spinner(ui: &mut Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), egui::Sense::hover());
    paint_arc(ui, &ui.painter().clone(), rect.center(), size);
}
