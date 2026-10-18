//! Diagnosis of a misplaced pointer (clicks landing beside their target on some machines):
//! with `SILBERKORN_POINTER=1` a crosshair marks where the app takes the pointer to be, and
//! scale, window and monitor sizes are shown and written to the log whenever they change.

use std::sync::OnceLock;

use egui::{Color32, Stroke, pos2, vec2};

fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("SILBERKORN_POINTER").is_some_and(|v| v != "0"))
}

/// Draws the crosshair and the numbers (only with `SILBERKORN_POINTER` set).
pub fn show(ctx: &egui::Context) {
    if !enabled() {
        return;
    }
    // Outside the input closure: the context's own getters would lock it a second time.
    let (zoom, area) = (ctx.zoom_factor(), ctx.content_rect());
    let info = ctx.input(|i| {
        let v = i.viewport();
        format!(
            "Skalierung {:.3} (System {:?}, Zoom {:.2})\nFläche {:?}\nInnen {:?}\nAußen {:?}\nMonitor {:?}\nMaximiert {:?}, Vollbild {:?}\nZeiger {:?}",
            i.pixels_per_point,
            v.native_pixels_per_point,
            zoom,
            area,
            v.inner_rect,
            v.outer_rect,
            v.monitor_size,
            v.maximized,
            v.fullscreen,
            i.pointer.latest_pos(),
        )
    });
    // In the log once per change (without the pointer, which changes all the time).
    static LAST: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
    let lasting = info.rsplit_once('\n').map_or(info.as_str(), |(a, _)| a).to_string();
    let mut last = LAST.lock().unwrap();
    if *last != lasting {
        log::warn!("Zeiger-Diagnose:\n{lasting}");
        *last = lasting;
    }
    drop(last);
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Debug, egui::Id::new("pointer debug")));
    if let Some(p) = ctx.input(|i| i.pointer.latest_pos()) {
        let s = Stroke::new(1.5, Color32::from_rgb(255, 40, 200));
        painter.line_segment([p - vec2(14.0, 0.0), p + vec2(14.0, 0.0)], s);
        painter.line_segment([p - vec2(0.0, 14.0), p + vec2(0.0, 14.0)], s);
        painter.circle_stroke(p, 5.0, s);
    }
    let r = ctx.content_rect();
    let pos = pos2(r.center().x - 200.0, r.bottom() - 190.0);
    let galley = painter.layout_no_wrap(info, egui::FontId::monospace(12.0), Color32::WHITE);
    painter.rect_filled(egui::Rect::from_min_size(pos, galley.size()).expand(6.0), 4.0, Color32::from_black_alpha(200));
    painter.galley(pos, galley, Color32::WHITE);
    ctx.request_repaint();
}
