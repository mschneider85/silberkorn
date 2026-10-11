//! The viewer in a window of its own, e.g. on a second monitor. While it is open, the main window shows the browser grid.

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
    /// Umschalt+F11: opens or closes the viewer window.
    pub fn toggle_viewer_window(&mut self) {
        self.viewer_window.open = !self.viewer_window.open;
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
            return;
        }
        let w = self.viewer_window;
        let mut builder = ViewportBuilder::default()
            .with_title(format!("{} – Viewer", self.window_title))
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
            if ui.input(|i| i.viewport().close_requested()) {
                self.viewer_window.open = false;
                self.save_settings();
                return;
            }
            if class != ViewportClass::EmbeddedWindow {
                self.track_viewer_window(&vctx);
            }
            // F11 and Esc concern this window's full screen; the other keys work as in the main window.
            let fullscreen = ui.input(|i| i.viewport().fullscreen.unwrap_or(false));
            let toggle = vctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F11))
                || (fullscreen && vctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)));
            if toggle {
                vctx.send_viewport_cmd(ViewportCommand::Fullscreen(!fullscreen));
                self.viewer_window.fullscreen = !fullscreen;
                self.save_settings();
            }
            if vctx.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::F11)) {
                self.toggle_viewer_window();
                return;
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
