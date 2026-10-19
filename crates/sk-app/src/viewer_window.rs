//! The viewer in a window of its own, e.g. on a second monitor. While it is open,
//! the main window starts with the browser grid; switched to the single image, it shows the image only to look at (zoom and tools stay
//! in the viewer window, one viewer state can't serve two windows).

use egui::{Color32, Key, Modifiers, ViewportBuilder, ViewportClass, ViewportCommand, ViewportId};
use serde::{Deserialize, Serialize};

use crate::app::App;

/// Position, size and state of the viewer window, kept between sessions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewerWindow {
    pub open: bool,
    /// Outer position and inner size in points.
    pub pos: Option<[f32; 2]>,
    pub size: Option<[f32; 2]>,
    pub fullscreen: bool,
}

pub fn viewport_id() -> ViewportId {
    ViewportId::from_hash_of("silberkorn viewer window")
}

impl App {
    /// Umschalt+F11: opens or closes the viewer window. Opening it shows the grid in the main window.
    pub fn toggle_viewer_window(&mut self) {
        self.viewer_window.open = !self.viewer_window.open;
        if self.viewer_window.open {
            self.grid_view = true;
        }
        self.viewer_window_shown = false;
        self.save_settings();
    }

    /// The main window shows the browser instead of the viewer.
    pub fn viewer_detached(&self) -> bool {
        self.viewer_window.open && !self.fullscreen
    }

    /// Shows the viewer window (call every frame of the main window).
    pub(crate) fn show_viewer_window(&mut self, ctx: &egui::Context) {
        if !self.viewer_detached() {
            self.viewer_pointer_down = false;
            return;
        }
        // A monitor unplugged since: the window would open where no one sees it.
        if !self.viewer_window_shown
            && let Some(p) = self.viewer_window.pos
            && !on_a_monitor(p, &self.monitors)
        {
            (self.viewer_window.pos, self.viewer_window.fullscreen) = (None, false);
        }
        let w = self.viewer_window;
        let mut builder = ViewportBuilder::default()
            .with_title(format!("{} – Bild", self.window_title))
            .with_inner_size(w.size.unwrap_or([1200.0, 800.0]))
            .with_min_inner_size([320.0, 240.0]);
        if let Some(p) = w.pos {
            builder = builder.with_position(p);
        }
        // Only on opening: the window manager may move the window, which must not be undone.
        if !self.viewer_window_shown {
            builder = builder.with_fullscreen(w.fullscreen);
        }
        let main_ppp = self.pixels_per_point;
        ctx.show_viewport_immediate(viewport_id(), builder, |ui, class| {
            let vctx = ui.ctx().clone();
            self.pixels_per_point = vctx.pixels_per_point();
            // The main window's input doesn't see a drag in here: one gesture must stay one
            // undo step and use drafts like in the main window.
            self.viewer_pointer_down = vctx.input(|i| i.pointer.any_down());
            self.pointer_down |= self.viewer_pointer_down;
            if ui.input(|i| i.viewport().close_requested()) {
                self.viewer_window.open = false;
                self.save_settings();
                return;
            }
            if class != ViewportClass::EmbeddedWindow {
                self.track_viewer_window(&vctx);
            }
            // The full screen keys (F11) and Esc concern this window's full screen; the other keys
            // work as in the main window. The keys come from the commands, so a changed binding
            // works here too.
            let command = pressed(&vctx, &["viewer-window", "fullscreen"]);
            if command == Some("viewer-window") {
                self.toggle_viewer_window();
                return;
            }
            let fullscreen = ui.input(|i| i.viewport().fullscreen.unwrap_or(false));
            // As in the main window, Esc first ends a tool (in `shortcuts`), then the full screen.
            let esc = fullscreen && !self.viewer_tool_active() && vctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
            let toggle = command == Some("fullscreen") || esc;
            if toggle {
                vctx.send_viewport_cmd(ViewportCommand::Fullscreen(!fullscreen));
                self.viewer_window.fullscreen = !fullscreen;
                self.save_settings();
            }
            self.shortcuts(&vctx);
            egui::CentralPanel::no_frame().show(ui, |ui| {
                ui.painter().rect_filled(ui.max_rect(), 0.0, Color32::from_gray(10));
                self.viewer(ui);
            });
        });
        self.pixels_per_point = main_ppp;
        self.viewer_window_shown = self.viewer_window.open;
    }

    /// The single image in the main window while the viewer window is open: fitted, without
    /// zoom or tools (those work in the viewer window).
    pub(crate) fn viewer_preview(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, egui::Sense::click());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, crate::theme::VIEWER);
        // While the next image loads: its placeholder, as in the viewer.
        let loading = self.texture.is_none() || self.loading.is_some();
        let tex = if loading { self.placeholder.as_ref().or(self.texture.as_ref()) } else { self.texture.as_ref() };
        let Some(tex) = tex else {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(egui::vec2(0.0, rect.height() * 0.25))));
            crate::widgets::no_image(&mut child);
            return;
        };
        let size = tex.size_vec2();
        let fit = ((rect.width() - 20.0) / size.x).min((rect.height() - 20.0) / size.y).max(0.01);
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        painter.image(tex.id(), egui::Rect::from_center_size(rect.center(), size * fit), uv, Color32::WHITE);
        response.on_hover_text(format!("Zoomen und Werkzeuge im Bildfenster ({} schließt es)", crate::commands::key_or("viewer-window", "Menü Ansicht")));
    }

    /// Remembers where the window is (saved when it changes).
    fn track_viewer_window(&mut self, ctx: &egui::Context) {
        let (outer, inner, fullscreen) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().inner_rect, i.viewport().fullscreen.unwrap_or(false)));
        if fullscreen {
            return;
        }
        let pos = outer.map(|r| [r.min.x, r.min.y]);
        let size = inner.map(|r| [r.width(), r.height()]);
        if (pos.is_some() && pos != self.viewer_window.pos) || (size.is_some() && size != self.viewer_window.size) {
            self.viewer_window.pos = pos.or(self.viewer_window.pos);
            self.viewer_window.size = size.or(self.viewer_window.size);
            self.viewer_window_moved = Some(std::time::Instant::now());
        }
    }
}

/// The monitors in points (`ppp` of the main window), empty without a native window.
pub(crate) fn monitors(frame: &eframe::Frame, ppp: f32) -> Vec<egui::Rect> {
    let Some(window) = frame.winit_window() else { return Vec::new() };
    window
        .available_monitors()
        .map(|m| {
            let (p, s) = (m.position(), m.size());
            egui::Rect::from_min_size(egui::pos2(p.x as f32 / ppp, p.y as f32 / ppp), egui::vec2(s.width as f32 / ppp, s.height as f32 / ppp))
        })
        .collect()
}

/// Whether a window at `pos` (outer top left) has its title bar on one of `monitors` (true
/// when they are unknown).
fn on_a_monitor(pos: [f32; 2], monitors: &[egui::Rect]) -> bool {
    let title = egui::pos2(pos[0] + 100.0, pos[1] + 15.0);
    monitors.is_empty() || monitors.iter().any(|m| m.contains(title))
}

/// Which of the commands had a shortcut pressed (it is consumed). Shortcuts with more modifiers
/// first, as in `command_shortcuts` (Shift+F11 would also match F11).
fn pressed(ctx: &egui::Context, ids: &[&'static str]) -> Option<&'static str> {
    let mut keys: Vec<(&'static str, (Modifiers, Key))> =
        ids.iter().filter_map(|id| crate::commands::get(id)).flat_map(|c| c.bindings().into_iter().map(move |k| (c.id, k))).collect();
    keys.sort_by_key(|(_, (m, _))| std::cmp::Reverse(m.command as u8 + m.ctrl as u8 + m.shift as u8 + m.alt as u8));
    keys.into_iter().find(|(_, (m, k))| ctx.input_mut(|i| i.consume_key(*m, *k))).map(|(id, _)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_position_off_all_monitors_is_dropped() {
        let left = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1920.0, 1080.0));
        let right = egui::Rect::from_min_size(egui::pos2(1920.0, 0.0), egui::vec2(2560.0, 1440.0));
        assert!(on_a_monitor([2000.0, 100.0], &[left, right]));
        assert!(!on_a_monitor([2000.0, 100.0], &[left]), "rechter Monitor abgesteckt");
        assert!(on_a_monitor([-50.0, 0.0], &[left]), "etwas über den Rand");
        assert!(on_a_monitor([5000.0, 5000.0], &[]), "unbekannt: wie gespeichert");
    }
}
