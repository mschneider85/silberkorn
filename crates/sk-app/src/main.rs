//! Silberkorn: a raw developer and image manager.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
#[cfg(test)]
mod app_tests;
mod browser;
mod chrome;
mod compare;
mod crop;
mod develop_tools;
mod focus;
mod guides;
mod history;
mod layers;
mod library;
mod rename;
mod session;
mod splash;
mod stacks;
mod theme;
mod tools;
mod variants;
mod widgets;
mod window_placement;
mod workers;

use std::path::PathBuf;

fn main() -> eframe::Result {
    env_logger::init();
    // The argument is a folder to browse, or a session (its `.sksession` file or folder).
    let arg = std::env::args().nth(1).map(PathBuf::from);
    let is_session = arg.as_ref().is_some_and(|p| sk_core::session::is_session_file(p) || (p.is_dir() && sk_core::session::find_session_file(p).is_some()));
    let (folder, session) = if is_session { (None, arg) } else { (arg, None) };

    let run = |renderer: eframe::Renderer| {
        let (folder, session) = (folder.clone(), session.clone());
        let options = eframe::NativeOptions {
            renderer,
            viewport: egui::ViewportBuilder::default()
                .with_title("Silberkorn")
                .with_inner_size([1500.0, 950.0])
                .with_min_inner_size([900.0, 600.0])
                // Maximized, with the size above when restored; debug screenshots keep that size.
                .with_maximized(std::env::var_os("SILBERKORN_SCREENSHOT").is_none())
                // The app draws its own title bar with the menus (see `chrome::title_bar`).
                .with_decorations(false),
            ..Default::default()
        };
        eframe::run_native("Silberkorn", options, Box::new(move |cc| {
            let opts = app::AppOptions { start_folder: folder, session, ..Default::default() };
            Ok(Box::new(app::App::new(&cc.egui_ctx, opts)))
        }))
    };

    // wgpu is the primary renderer; fall back to OpenGL if no suitable GPU adapter is available.
    let prefer_glow = std::env::var("SILBERKORN_RENDERER").is_ok_and(|v| v.eq_ignore_ascii_case("glow"));
    if prefer_glow {
        return run(eframe::Renderer::Glow);
    }
    run(eframe::Renderer::Wgpu).or_else(|e| {
        log::warn!("wgpu nicht verfügbar ({e}), verwende OpenGL");
        run(eframe::Renderer::Glow)
    })
}

#[cfg(test)]
mod tests {
    /// Every non-Latin symbol used in the UI sources must exist in egui's bundled fonts,
    /// otherwise it renders as an empty box.
    #[test]
    fn ui_symbols_have_glyphs() {
        let sources = [
            include_str!("app.rs"),
            include_str!("browser.rs"),
            include_str!("crop.rs"),
            include_str!("develop_tools.rs"),
            include_str!("layers.rs"),
            include_str!("library.rs"),
            include_str!("tools.rs"),
            include_str!("widgets.rs"),
            include_str!("chrome.rs"),
            include_str!("session.rs"),
            include_str!("splash.rs"),
        ];
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        let font = egui::FontId::proportional(14.0);
        // A missing glyph is rendered with the replacement character '◻' (same atlas region).
        let uv = |c: char| {
            ctx.fonts_mut(|f| {
                let galley = f.layout_no_wrap(c.to_string(), font.clone(), egui::Color32::WHITE);
                galley.rows[0].row.glyphs[0].uv_rect
            })
        };
        let replacement = uv('◻');
        let mut missing: Vec<char> = sources
            .iter()
            .flat_map(|s| s.chars())
            .filter(|c| (*c as u32) >= 0x2000 && !matches!(c, '–' | '—' | '„' | '“' | '”' | '…' | '’' | '×' | '◻'))
            .filter(|c| uv(*c) == replacement)
            .collect();
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "Fehlende Glyphen: {missing:?}");
    }
}
