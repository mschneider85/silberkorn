//! End-to-end tests of the application logic with real workers and sample raw files.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sk_core::catalog::{Filter, Scope, SortKey};
use sk_core::sidecar::{self, ColorLabel};
use sk_core::styles;

use crate::app::{App, AppOptions};

fn samples() -> Option<Vec<PathBuf>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples");
    let files: Vec<PathBuf> = ["_DSC0009.ARW", "_DSC0010.ARW"].iter().map(|f| dir.join(f)).collect();
    files.iter().all(|f| f.exists()).then_some(files)
}

struct Fixture {
    dir: PathBuf,
    ctx: egui::Context,
    app: App,
}

impl Fixture {
    fn new(name: &str) -> Option<Self> {
        let files = samples()?;
        let dir = std::env::temp_dir().join(format!("sk-app-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("photos")).unwrap();
        for f in &files {
            let target = dir.join("photos").join(f.file_name().unwrap());
            if std::fs::hard_link(f, &target).is_err() {
                std::fs::copy(f, &target).unwrap();
            }
        }
        let ctx = egui::Context::default();
        let app = App::new(
            &ctx,
            AppOptions {
                start_folder: Some(dir.join("photos")),
                catalog_path: dir.join("catalog.sqlite"),
                persist_settings: false,
            },
        );
        Some(Self { dir, ctx, app })
    }

    fn pump(&mut self, what: &str, done: impl Fn(&App) -> bool) {
        let start = Instant::now();
        while !done(&self.app) {
            assert!(start.elapsed() < Duration::from_secs(60), "Zeitüberschreitung: {what}");
            self.app.handle_events(&self.ctx);
            self.app.send_render();
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn photo(&self, name: &str) -> PathBuf {
        self.dir.join("photos").join(name).canonicalize().unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn library_workflow() {
    let Some(mut f) = Fixture::new("library") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    f.pump("Indexiert", |a| a.lists.total == 2 && a.entries.iter().all(|e| e.capture_date.is_some()));
    assert_eq!(f.app.entries.len(), 2);
    let a = f.photo("_DSC0009.ARW");
    let b = f.photo("_DSC0010.ARW");
    assert_eq!(f.app.current_path().as_ref(), Some(&a));

    // Rating on the open image goes to its sidecar and the catalog.
    f.app.set_rating(4);
    assert_eq!(sidecar::load(&a).rating, 4);
    f.app.filter.min_rating = 4;
    f.app.refilter();
    assert_eq!(f.app.visible.len(), 1);
    f.app.filter = Filter::default();
    f.app.refilter();

    // Keywords, then search via the catalog.
    f.app.add_keyword("Park");
    assert_eq!(sidecar::load(&a).keywords, vec!["Park".to_string()]);
    let hits = f.app.catalog.as_ref().unwrap().query(&Scope::All, &Filter { text: "park".into(), ..Default::default() }, SortKey::Name, false).unwrap();
    assert_eq!(hits.len(), 1);

    // Multi-selection: color label applies to both images.
    f.app.selection = [a.clone(), b.clone()].into_iter().collect();
    f.app.set_color_label(ColorLabel::Green);
    assert_eq!(sidecar::load(&a).color_label, ColorLabel::Green);
    assert_eq!(sidecar::load(&b).color_label, ColorLabel::Green);

    // Album with the selection.
    let cat = f.app.catalog.as_ref().unwrap();
    let album = cat.create_album("Test", None).unwrap();
    cat.add_to_album(album, &f.app.selected_paths()).unwrap();
    f.app.open_scope(Scope::Album(album));
    assert_eq!(f.app.entries.len(), 2);

    // Sorting by date descending puts the later shot first.
    f.app.sort = SortKey::CaptureDate;
    f.app.sort_desc = true;
    f.app.resort();
    assert_eq!(f.app.entries[f.app.visible[0]].path, b);
}

#[test]
fn edit_copy_apply_style_and_export() {
    let Some(mut f) = Fixture::new("edit") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    let a = f.photo("_DSC0009.ARW");
    let b = f.photo("_DSC0010.ARW");

    // Edit + commit writes the sidecar.
    f.app.edit(|adj| adj.exposure = 0.7);
    f.app.commit_and_save();
    assert_eq!(sidecar::load(&a).adjustments.exposure, 0.7);

    // Copy and apply to the other image.
    f.app.copy_adjustments();
    f.app.selection = [b.clone()].into_iter().collect();
    f.app.paste_adjustments();
    assert_eq!(sidecar::load(&b).adjustments.exposure, 0.7);

    // Styles round-trip through a style directory.
    let style_dir = f.dir.join("styles");
    let style = styles::Style { name: "Kontrast".into(), adjustments: sk_core::params::Adjustments { contrast: 30.0, ..Default::default() } };
    styles::save_in(&style_dir, &style).unwrap();
    let loaded = styles::list_in(&style_dir);
    f.app.set_adjustments_on(std::slice::from_ref(&b), |adj| styles::apply(&loaded[0], adj));
    assert_eq!(sidecar::load(&b).adjustments.contrast, 30.0);

    // Export both images.
    f.app.recipes[0].output_dir = f.dir.join("out");
    f.app.recipes[0].long_edge = Some(800);
    f.app.selection = [a.clone(), b.clone()].into_iter().collect();
    f.app.export_selection();
    f.pump("Export", |app| app.export_progress.is_none() && app.export_log.len() == 2);
    assert!(f.app.export_log.iter().all(|l| l.starts_with('✔')), "{:?}", f.app.export_log);
    for name in ["_DSC0009.jpg", "_DSC0010.jpg"] {
        let img = ::image::open(f.dir.join("out").join(name)).unwrap();
        assert_eq!(img.width().max(img.height()), 800);
    }
}

#[test]
fn crop_full_resolution_and_tiles() {
    let Some(mut f) = Fixture::new("tiles") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    let full_w = f.app.texture.as_ref().unwrap().size()[0];

    // Cropping to half the width halves the rendered preview.
    f.app.edit(|adj| adj.geometry.crop = sk_core::params::CropRect { x: 0.25, y: 0.0, w: 0.5, h: 1.0 });
    let v = f.app.edit_version;
    f.pump("Zuschnitt gerendert", |a| a.texture.as_ref().is_some_and(|t| t.size()[0] < full_w * 6 / 10) && a.edit_version == v);
    let w = f.app.texture.as_ref().unwrap().size()[0];
    assert!((w as i32 - full_w as i32 / 2).abs() <= 2, "{w} vs {full_w}");

    // The full-resolution image arrives in the background; a zoomed view requests a tile.
    f.pump("Vollauflösung", |a| a.current.as_ref().is_some_and(|c| c.full.is_some()));
    let tex = f.app.texture.as_ref().unwrap().size_vec2();
    let viewport = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
    let scale = 3.0;
    let img_rect = egui::Rect::from_center_size(viewport.center(), tex * scale);
    f.app.update_tile(viewport, img_rect, scale);
    f.pump("Kachel", |a| a.tile.as_ref().is_some_and(|t| t.version == a.edit_version));
    let tile = f.app.tile.as_ref().unwrap();
    let full = f.app.current.as_ref().unwrap().full.as_ref().unwrap();
    assert!(tile.full_size.0 > 2000 && tile.full_size.0 < full.width, "{:?}", tile.full_size);
    assert!(tile.region.w > 300 && tile.region.w < tile.full_size.0);
}

/// Runs one UI frame of the viewer with the given input events.
fn viewer_frame(f: &mut Fixture, events: Vec<egui::Event>) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
        events,
        ..Default::default()
    };
    let app = &mut f.app;
    let out = f.ctx.run_ui(input, |ui| {
        app.pointer_down = ui.ctx().input(|i| i.pointer.any_down());
        egui::CentralPanel::no_frame().show(ui, |ui| app.viewer(ui));
    });
    let mut delta = out.textures_delta;
    delta.clear();
}

fn press(p: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
}

/// Drags the mouse from `a` to `b` in `steps` frames.
fn drag(f: &mut Fixture, a: egui::Pos2, b: egui::Pos2, steps: usize) {
    viewer_frame(f, vec![egui::Event::PointerMoved(a)]);
    viewer_frame(f, vec![press(a, true)]);
    for i in 1..=steps {
        let p = a + (b - a) * (i as f32 / steps as f32);
        viewer_frame(f, vec![egui::Event::PointerMoved(p)]);
    }
    viewer_frame(f, vec![press(b, false)]);
    viewer_frame(f, vec![]);
}

#[test]
fn viewer_tools_brush_heal_crop() {
    let Some(mut f) = Fixture::new("viewer") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    viewer_frame(&mut f, vec![]);
    let c = egui::pos2(600.0, 400.0);

    // Brush stroke creates a layer with one stroke of several points.
    f.app.set_local_tool(crate::layers::LocalTool::Brush);
    drag(&mut f, c, c + egui::vec2(200.0, 50.0), 12);
    let adj = &f.app.current.as_ref().unwrap().adjustments;
    assert_eq!(adj.layers.len(), 1);
    assert_eq!(adj.layers[0].mask.strokes.len(), 1);
    assert!(adj.layers[0].mask.strokes[0].points.len() >= 4, "{:?}", adj.layers[0].mask.strokes[0].points.len());
    let uv = adj.layers[0].mask.strokes[0].points[0];
    assert!((uv[0] - 0.5).abs() < 0.1 && (uv[1] - 0.5).abs() < 0.1, "stroke starts near the image center: {uv:?}");

    // Linear gradient on the same layer.
    f.app.set_local_tool(crate::layers::LocalTool::Linear);
    drag(&mut f, c, c + egui::vec2(0.0, 150.0), 5);
    assert_eq!(f.app.current.as_ref().unwrap().adjustments.layers[0].mask.linear.len(), 1);

    // A click with the heal tool adds a spot with an automatically chosen source.
    f.app.set_local_tool(crate::layers::LocalTool::Heal);
    let p = c + egui::vec2(-150.0, -100.0);
    viewer_frame(&mut f, vec![egui::Event::PointerMoved(p)]);
    viewer_frame(&mut f, vec![press(p, true)]);
    viewer_frame(&mut f, vec![press(p, false)]);
    viewer_frame(&mut f, vec![]);
    let spots = &f.app.current.as_ref().unwrap().adjustments.retouch;
    assert_eq!(spots.len(), 1);
    assert_ne!(spots[0].source, spots[0].target);
    f.app.delete_selected_spot();
    assert!(f.app.current.as_ref().unwrap().adjustments.retouch.is_empty());

    // Crop tool: drawing a new rectangle sets the crop.
    f.app.local_tool = None;
    f.app.start_crop_tool();
    f.pump("Unbeschnittene Vorschau", |a| a.texture.is_some());
    viewer_frame(&mut f, vec![]);
    drag(&mut f, egui::pos2(150.0, 120.0), egui::pos2(500.0, 400.0), 6);
    let crop = f.app.current.as_ref().unwrap().adjustments.geometry.crop;
    assert!(crop.w < 0.8 && crop.h < 0.8 && crop.x > 0.0, "{crop:?}");
    f.app.finish_crop_tool();
}

fn key(k: egui::Key, pressed: bool) -> egui::Event {
    egui::Event::Key { key: k, physical_key: None, pressed, repeat: false, modifiers: Default::default() }
}

fn wheel(dy: f32) -> egui::Event {
    egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: egui::vec2(0.0, dy),
        phase: egui::TouchPhase::Move,
        modifiers: Default::default(),
    }
}

fn click(f: &mut Fixture, p: egui::Pos2) {
    viewer_frame(f, vec![egui::Event::PointerMoved(p)]);
    viewer_frame(f, vec![press(p, true)]);
    viewer_frame(f, vec![press(p, false)]);
    viewer_frame(f, vec![]);
}

#[test]
fn phase5_speed_edit_ai_masks_split_and_recipes() {
    let Some(mut f) = Fixture::new("phase5") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    viewer_frame(&mut f, vec![]);
    let c = egui::pos2(600.0, 400.0);

    // Speed Edit: hold Q and scroll two notches up -> exposure +0.10.
    viewer_frame(&mut f, vec![egui::Event::PointerMoved(c), key(egui::Key::Q, true)]);
    viewer_frame(&mut f, vec![wheel(1.0)]);
    viewer_frame(&mut f, vec![wheel(1.0)]);
    viewer_frame(&mut f, vec![key(egui::Key::Q, false)]);
    let ev = f.app.current.as_ref().unwrap().adjustments.exposure;
    assert!((ev - 0.1).abs() < 1e-4, "exposure {ev}");
    assert!(f.app.zoom.is_none(), "scrolling with a speed key must not zoom");

    // Magic brush click -> AI mask on a new layer.
    f.app.set_local_tool(crate::layers::LocalTool::Magic);
    click(&mut f, c + egui::vec2(-300.0, 250.0));
    f.pump("Magic Brush", |a| !a.ai_busy && a.current.as_ref().is_some_and(|c| c.adjustments.layers.first().is_some_and(|l| l.mask.ai.is_some())));
    let ai = f.app.current.as_ref().unwrap().adjustments.layers[0].mask.ai.clone().unwrap();
    assert!(ai.data.iter().any(|v| *v > 200) && ai.data.iter().any(|v| *v < 20));

    // Sky selection merges into the same layer's AI mask.
    f.app.request_ai(crate::workers::AiKind::Sky);
    f.pump("Himmel", |a| !a.ai_busy);
    let merged = f.app.current.as_ref().unwrap().adjustments.layers[0].mask.ai.clone().unwrap();
    assert!(merged.data.iter().zip(&ai.data).all(|(m, a)| m >= a), "union keeps the magic selection");

    // Color pick sets a color range on the active layer.
    f.app.set_local_tool(crate::layers::LocalTool::ColorPick);
    click(&mut f, c);
    assert!(f.app.current.as_ref().unwrap().adjustments.layers[0].mask.color_range.is_some());

    // Split view gets its own "before" texture.
    f.app.toggle_split();
    f.app.send_render();
    f.pump("Vergleich", |a| a.before_texture.is_some());

    // Two recipes -> two files per image.
    let out = f.dir.join("out5");
    f.app.recipes = vec![
        sk_core::export::ExportRecipe { output_dir: out.clone(), long_edge: Some(300), ..Default::default() },
        sk_core::export::ExportRecipe {
            name: "PNG".into(),
            format: sk_core::export::ExportFormat::Png,
            output_dir: out.clone(),
            long_edge: Some(200),
            ..Default::default()
        },
    ];
    f.app.selection = [f.photo("_DSC0009.ARW")].into_iter().collect();
    f.app.export_selection();
    f.pump("Export", |a| a.export_progress.is_none() && a.export_log.len() == 2);
    assert!(out.join("_DSC0009.jpg").exists() && out.join("_DSC0009.png").exists(), "{:?}", f.app.export_log);
}
