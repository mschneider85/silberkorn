//! Capture Room: a raw developer and image manager.

mod app;
mod tools;
mod widgets;
mod workers;

use std::path::PathBuf;

fn main() -> eframe::Result {
    env_logger::init();
    let folder = std::env::args().nth(1).map(PathBuf::from);

    let run = |renderer: eframe::Renderer| {
        let folder = folder.clone();
        let options = eframe::NativeOptions {
            renderer,
            viewport: egui::ViewportBuilder::default()
                .with_title("Capture Room")
                .with_inner_size([1500.0, 950.0])
                .with_min_inner_size([900.0, 600.0]),
            ..Default::default()
        };
        eframe::run_native("Capture Room", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, folder)))))
    };

    // wgpu is the primary renderer; fall back to OpenGL if no suitable GPU adapter is available.
    let prefer_glow = std::env::var("CAPTURE_ROOM_RENDERER").is_ok_and(|v| v.eq_ignore_ascii_case("glow"));
    if prefer_glow {
        return run(eframe::Renderer::Glow);
    }
    run(eframe::Renderer::Wgpu).or_else(|e| {
        log::warn!("wgpu nicht verfügbar ({e}), verwende OpenGL");
        run(eframe::Renderer::Glow)
    })
}
