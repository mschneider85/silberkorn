//! End-to-end tests of the application logic with real workers and sample raw files.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sk_core::catalog::{Filter, Scope, SortKey};
use sk_core::sidecar::{self, ColorLabel};
use sk_core::styles;

use crate::app::{App, AppOptions, MAX_THUMBS};

/// The test images: the sample raw files reduced to 2400 x 1600, as linear DNGs. Same scene,
/// capture data and camera color, but a sixth of the pixels, so the tests spend their time on
/// the app instead of on 24 megapixels. Still larger than the preview, so that full resolution
/// and tiles are real. Made once, kept in `target/`.
fn samples() -> Option<Vec<PathBuf>> {
    static FILES: std::sync::OnceLock<Option<Vec<PathBuf>>> = std::sync::OnceLock::new();
    FILES.get_or_init(make_samples).clone()
}

fn make_samples() -> Option<Vec<PathBuf>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    const EDGE: usize = 2400;
    let dir = root.join(format!("target/test-samples/{EDGE}"));
    std::fs::create_dir_all(&dir).unwrap();
    ["_DSC0009", "_DSC0010"]
        .iter()
        .map(|name| {
            let raw = root.join("samples").join(format!("{name}.ARW"));
            let dng = dir.join(format!("{name}.dng"));
            let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
            if modified(&dng).is_none() || modified(&dng) < modified(&raw) {
                let (meta, img) = sk_core::raw::load_preview(&raw, EDGE).ok()?;
                let preview = sk_core::dng::quick_preview(&meta, &img, 1024);
                let info = sk_core::raw::CaptureInfo { width: img.width, height: img.height, ..meta.info.clone() };
                sk_core::dng::write_linear_dng(
                    &dng,
                    &sk_core::dng::LinearDng {
                        width: img.width,
                        height: img.height,
                        data: &img.data,
                        xyz_to_cam: meta.color.xyz_to_cam,
                        as_shot_wb: meta.as_shot_wb,
                        info: &info,
                        baseline_exposure: 0.0,
                        preview: Some(&preview),
                    },
                )
                .unwrap();
            }
            Some(dng)
        })
        .collect()
}

/// How long a test waits for background work. Generous: real raw decoding, and the machine
/// may be busy with other work (parallel tests, builds).
const LIMIT: Duration = Duration::from_secs(180);

struct Fixture {
    dir: PathBuf,
    ctx: egui::Context,
    app: App,
}

impl Fixture {
    fn new(name: &str) -> Option<Self> {
        Self::with(name, samples()?)
    }

    /// With the sample raw files at their full 24 megapixels.
    fn full_size(name: &str) -> Option<Self> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples");
        let files: Vec<PathBuf> = ["_DSC0009.ARW", "_DSC0010.ARW"].iter().map(|f| dir.join(f)).collect();
        Self::with(name, files.iter().all(|f| f.exists()).then_some(files)?)
    }

    fn with(name: &str, files: Vec<PathBuf>) -> Option<Self> {
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
                session: None,
                catalog_path: dir.join("catalog.sqlite"),
                persist_settings: false,
            },
        );
        Some(Self { dir, ctx, app })
    }

    fn pump(&mut self, what: &str, done: impl Fn(&App) -> bool) {
        let start = Instant::now();
        while !done(&self.app) {
            // Generous: real raw decoding, and the machine may be busy with other work.
            assert!(start.elapsed() < LIMIT, "Zeitüberschreitung: {what}");
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
    let a = f.photo("_DSC0009.dng");
    let b = f.photo("_DSC0010.dng");
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
fn noise_settings_carry_over_to_the_selection() {
    let Some(mut f) = Fixture::new("noise") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    let other = f.app.entries.iter().map(|e| e.path.clone()).find(|p| Some(p) != f.app.current_path().as_ref()).unwrap();
    f.app.selection = f.app.entries.iter().map(|e| e.path.clone()).collect();
    f.app.transfer_noise((40.0, 25.0, 30.0));
    let d = sidecar::load(&other).adjustments.detail;
    assert_eq!((d.ai_denoise, d.nr_luma, d.nr_color), (40.0, 25.0, 30.0));
    let d = &f.app.current.as_ref().unwrap().adjustments.detail;
    assert_eq!((d.ai_denoise, d.nr_luma, d.nr_color), (40.0, 25.0, 30.0));
    assert!(f.app.can_undo());
}

#[test]
#[ignore = "langsam: läuft mit cargo t-all"]
fn edit_copy_apply_style_and_export() {
    let Some(mut f) = Fixture::new("edit") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    let a = f.photo("_DSC0009.dng");
    let b = f.photo("_DSC0010.dng");

    // Edit + commit writes the sidecar.
    f.app.edit(|adj| adj.exposure = 0.7);
    f.app.commit_and_save();
    assert_eq!(sidecar::load(&a).adjustments.exposure, 0.7);

    // Copy and apply to the other image.
    f.app.copy_adjustments();
    f.app.selection = [b.clone()].into_iter().collect();
    f.app.paste_adjustments();
    assert_eq!(sidecar::load(&b).adjustments.exposure, 0.7);

    // An edit saved before process versions keeps the original renderer until lifted.
    std::fs::write(sidecar::sidecar_path(&b), r#"{"version":1,"adjustments":{"exposure":0.3}}"#).unwrap();
    assert_eq!(sidecar::load(&b).adjustments.process, sk_core::params::PROCESS_LEGACY);
    f.app.update_process_version();
    let lifted = sidecar::load(&b).adjustments;
    assert_eq!((lifted.process, lifted.exposure), (sk_core::params::PROCESS_LATEST, 0.3));
    assert!(f.app.status.contains("1 Bild"), "{}", f.app.status);

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
#[ignore = "langsam: läuft mit cargo t-all"]
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
    f.app.update_tiles(viewport, img_rect, scale, true);
    // First the exact visible region, for a sharp view as fast as possible ...
    f.pump("Ausschnitt", |a| a.tiles.view.as_ref().is_some_and(|v| v.version == a.edit_version && v.level == 1));
    let region = f.app.tiles.view.as_ref().unwrap().region;
    assert!(region.w > 300 && region.w < f.app.tiles.full_size.0, "{region:?}");
    f.pump("Kachel", |a| !a.tiles.textures.is_empty());
    let (fw, fh) = f.app.tiles.full_size;
    let full = f.app.current.as_ref().unwrap().full.as_ref().unwrap();
    assert!(fw > w as usize && fw < full.width, "{fw} vs Vorschau {w}");
    // ... then the grid, starting with the tile under the view center ...
    let center = (fw / 2 / crate::workers::TILE, fh / 2 / crate::workers::TILE);
    assert!(f.app.tiles.textures.contains_key(&center), "{:?}", f.app.tiles.textures.keys().collect::<Vec<_>>());
    // ... then the rest of the image, so panning needs no rendering.
    let n = fw.div_ceil(crate::workers::TILE) * fh.div_ceil(crate::workers::TILE);
    f.pump("alle Kacheln", |a| a.tiles.textures.len() == n);
    let edge = f.app.tiles.textures[&(fw.div_ceil(crate::workers::TILE) - 1, 0)].size();
    assert_eq!(edge[0], fw - (fw.div_ceil(crate::workers::TILE) - 1) * crate::workers::TILE, "Randkachel ist kleiner");

    // An edit invalidates the tiles.
    f.app.edit(|adj| adj.exposure = 0.5);
    f.app.update_tiles(viewport, img_rect, scale, true);
    f.pump("neuer Ausschnitt", |a| a.tiles.view.as_ref().is_some_and(|v| v.version == a.edit_version));
    // Old tiles are no longer valid (they are not drawn; new ones replace them).
    assert!(f.app.tiles.version != f.app.edit_version || f.app.tiles.textures.len() < n || n == 1);
}

/// Runs one UI frame of the viewer with the given input events.
fn viewer_frame(f: &mut Fixture, events: Vec<egui::Event>) {
    run_viewer(f, egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
        events,
        ..Default::default()
    });
}

/// [`viewer_frame`] with modifier keys held.
fn viewer_frame_with(f: &mut Fixture, events: Vec<egui::Event>, modifiers: egui::Modifiers) {
    let events = std::iter::once(egui::Event::ModifiersChanged(modifiers)).chain(events).collect();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
        events,
        ..Default::default()
    };
    run_viewer(f, input);
}

fn run_viewer(f: &mut Fixture, input: egui::RawInput) {
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
#[ignore = "langsam: läuft mit cargo t-all"]
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
    f.pump("Zauberpinsel", |a| !a.ai_busy && a.current.as_ref().is_some_and(|c| c.adjustments.layers.first().is_some_and(|l| l.mask.ai.is_some())));
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
    // U and Shift+U switch between divider and side by side without a new before rendering.
    f.app.toggle_side_by_side();
    assert!(f.app.split_view && f.app.split_side && f.app.before_texture.is_some());
    f.app.toggle_split();
    assert!(f.app.split_view && !f.app.split_side);
    f.app.toggle_side_by_side();
    f.app.toggle_side_by_side();
    assert!(!f.app.split_view);
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
    f.app.selection = [f.photo("_DSC0009.dng")].into_iter().collect();
    f.app.export_selection();
    f.pump("Export", |a| a.export_progress.is_none() && a.export_log.len() == 2);
    assert!(out.join("_DSC0009.jpg").exists() && out.join("_DSC0009.png").exists(), "{:?}", f.app.export_log);
}

#[test]
fn session_workflow() {
    use sk_core::session::SessionFolder;
    let Some(mut f) = Fixture::new("session") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    f.app.set_rating(3);
    let global = f.app.catalog_path();

    // Creating a session switches to its database; importing copies into Capture.
    f.app.create_session(&f.dir, "Shooting").unwrap();
    let s = f.app.session.clone().unwrap();
    assert_eq!(f.app.catalog_path(), s.file);
    assert_eq!(f.app.folder, s.folder(SessionFolder::Capture));
    let photos = f.dir.join("photos");
    f.app.import_folder(&photos);
    f.pump("Import", |a| a.scope == Scope::LastImport && a.entries.len() == 2);
    let capture = s.folder(SessionFolder::Capture);
    let a = capture.join("_DSC0009.dng");
    let b = capture.join("_DSC0010.dng");
    assert!(a.exists() && b.exists() && photos.join("_DSC0009.dng").exists());
    assert_eq!(sidecar::load(&a).rating, 3, "Sidecar wird mitkopiert");

    // Move to Selects: file and sidecar move, the database follows, the next image opens.
    f.app.open_folder(capture.clone());
    f.app.selection = [a.clone()].into_iter().collect();
    f.app.move_to_session_folder(SessionFolder::Selects);
    let a2 = s.folder(SessionFolder::Selects).join("_DSC0009.dng");
    assert!(!a.exists() && a2.exists());
    assert_eq!(sidecar::load(&a2).rating, 3);
    assert_eq!(f.app.entries.len(), 1);
    assert_eq!(f.app.primary_path().as_ref(), Some(&b));
    let cat = f.app.catalog.as_ref().unwrap();
    let selects = Scope::Folder { path: s.folder(SessionFolder::Selects), recursive: false };
    assert_eq!(cat.query(&selects, &Filter::default(), SortKey::Name, false).unwrap().len(), 1);

    // Favorites live in the session.
    let fav = s.root.join("Kunde");
    std::fs::create_dir_all(&fav).unwrap();
    f.app.toggle_favorite(&fav);
    assert_eq!(f.app.lists.favorites, vec![fav.clone()]);

    // Processing goes to the session's Output folder.
    f.app.recipes[0].output_dir = "Output".into();
    f.app.recipes[0].long_edge = Some(300);
    f.app.selection = [b.clone()].into_iter().collect();
    f.app.export_selection();
    f.pump("Export", |app| app.export_progress.is_none() && app.export_log.len() == 1);
    assert!(s.folder(SessionFolder::Output).join("_DSC0010.jpg").exists(), "{:?}", f.app.export_log);

    // Trash, then empty it.
    f.app.move_to_session_folder(SessionFolder::Trash);
    let b2 = s.folder(SessionFolder::Trash).join("_DSC0010.dng");
    assert!(b2.exists() && f.app.entries.is_empty());
    f.app.empty_trash();
    assert!(!b2.exists() && !sidecar::sidecar_path(&b2).exists());

    // Closing returns to the catalog; reopening restores the session state.
    f.app.close_session();
    assert!(f.app.session.is_none());
    assert_eq!(f.app.catalog_path(), global);
    assert_eq!(f.app.folder, photos.canonicalize().unwrap());
    f.app.open_session(&s.root);
    assert_eq!(f.app.lists.favorites, vec![fav]);
    f.pump("Session indexiert", |a| a.lists.total == 1);
    assert_eq!(f.app.recent_sessions.first(), Some(&s.file));
    let cat = f.app.catalog.as_ref().unwrap();
    let album = cat.create_album("Favoriten", None).unwrap();
    cat.add_to_album(album, std::slice::from_ref(&a2)).unwrap();

    // Portable: the moved and renamed session keeps albums, favorites and its index.
    f.app.close_session();
    let moved = f.dir.join("Archiv");
    std::fs::create_dir_all(&moved).unwrap();
    std::fs::rename(&s.root, moved.join("Shooting 2026")).unwrap();
    f.app.open_session(&moved.join("Shooting 2026"));
    let s2 = f.app.session.clone().expect("Session geöffnet");
    assert_eq!(s2.name, "Shooting");
    assert_eq!(f.app.lists.favorites, vec![s2.root.join("Kunde")]);
    f.app.open_scope(Scope::Album(album));
    let a3 = s2.folder(SessionFolder::Selects).join("_DSC0009.dng");
    assert_eq!(f.app.entries.iter().map(|e| e.path.clone()).collect::<Vec<_>>(), vec![a3.clone()]);
    assert_eq!(f.app.entries[0].rating, 3);
    assert_eq!(f.app.lists.total, 1);

    // Thumbnails are cached inside the session and are still valid after another move.
    // As the browser asks for a visible cell.
    f.app.thumb_requested.insert(a3.clone());
    f.app.workers.thumbnail(a3.clone(), Some(s2.root.clone()));
    f.pump("Vorschaubild", |a| a.thumbs.contains_key(&a3));
    let adj = sidecar::load(&a3).adjustments;
    assert!(sk_core::cache::load(&a3, &adj, Some(&s2.root)).is_some());
    assert!(sk_core::session::thumbnail_dir(&s2.root).is_dir());
    f.app.close_session();
    let again = f.dir.join("Kopie");
    std::fs::rename(&s2.root, &again).unwrap();
    let a4 = again.join("Selects").join("_DSC0009.dng");
    assert!(sk_core::cache::load(&a4, &adj, Some(&again)).is_some(), "Cache wandert mit");
    f.app.open_session(&again);
    f.app.open_folder(again.clone());
    assert!(!f.app.subdirs.contains(&f.app.session.as_ref().unwrap().root.join("Silberkorn")));
    assert!(f.app.subdirs.iter().any(|d| d.ends_with("Selects")));
}

/// Drives the title bar with synthetic input and reports the labels of all widgets
/// (via AccessKit), so tests can see which menu is open.
struct TitleBarDriver;

impl TitleBarDriver {
    fn frame(&mut self, f: &mut Fixture, events: Vec<egui::Event>) -> Vec<(String, egui::Rect)> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1500.0, 950.0))),
            events,
            ..Default::default()
        };
        f.ctx.enable_accesskit();
        let mut out = f.ctx.run_ui(input, |ui| {
            egui::Panel::top("titlebar").exact_size(crate::chrome::TITLE_BAR_HEIGHT).show(ui, |ui| f.app.title_bar(ui));
        });
        out.textures_delta.clear();
        let update = out.platform_output.accesskit_update.expect("AccessKit-Baum");
        update
            .nodes
            .iter()
            .filter_map(|(_, n)| {
                let b = n.bounds()?;
                Some((n.label()?.to_string(), egui::Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32))))
            })
            .collect()
    }

    fn move_to(&mut self, f: &mut Fixture, pos: egui::Pos2) -> Vec<(String, egui::Rect)> {
        self.frame(f, vec![egui::Event::PointerMoved(pos)])
    }

    fn click(&mut self, f: &mut Fixture, pos: egui::Pos2) -> Vec<(String, egui::Rect)> {
        self.move_to(f, pos);
        let button = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        self.frame(f, vec![button(true)]);
        self.frame(f, vec![button(false)]);
        self.frame(f, vec![])
    }

    fn key(&mut self, f: &mut Fixture, key: egui::Key) -> Vec<(String, egui::Rect)> {
        let ev = |pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Default::default() };
        self.frame(f, vec![ev(true), ev(false)]);
        self.frame(f, vec![])
    }
}

fn widget(nodes: &[(String, egui::Rect)], label: &str) -> Option<egui::Rect> {
    nodes.iter().find(|(l, _)| l.contains(label)).map(|(_, r)| *r)
}

#[test]
fn menu_bar_behaves_like_native() {
    let Some(mut f) = Fixture::new("menubar") else { return };
    let mut d = TitleBarDriver;
    let nodes = d.frame(&mut f, vec![]);
    let title = |l: &str| widget(&nodes, l).unwrap_or_else(|| panic!("Menütitel {l} fehlt")).center();
    let (datei, entwickeln, ansicht) = (title("Datei"), title("Entwickeln"), title("Ansicht"));

    // Hovering alone opens nothing.
    let nodes = d.move_to(&mut f, entwickeln);
    assert!(widget(&nodes, "Links drehen").is_none());

    // A click opens the menu, hovering another title switches to it.
    let nodes = d.click(&mut f, datei);
    assert!(widget(&nodes, "Beenden").is_some(), "Datei-Menü sollte offen sein");
    d.move_to(&mut f, entwickeln);
    let nodes = d.frame(&mut f, vec![]);
    assert!(widget(&nodes, "Links drehen").is_some(), "Hover sollte zum Entwickeln-Menü wechseln");
    assert!(widget(&nodes, "Beenden").is_none());

    // Arrow keys step to the neighbouring menu.
    let nodes = d.key(&mut f, egui::Key::ArrowRight);
    assert!(widget(&nodes, "Raster").is_some(), "Pfeil rechts sollte das Ansicht-Menü öffnen");

    // A command runs and closes the menu.
    let grid = f.app.grid_view;
    let raster = widget(&nodes, "Raster").unwrap().center();
    let nodes = d.click(&mut f, raster);
    assert_ne!(f.app.grid_view, grid, "Raster sollte umgeschaltet sein");
    assert!(widget(&nodes, "Raster").is_none(), "Ein Befehl schließt das Menü");

    // A click on a disabled entry keeps the menu open (nothing to undo yet).
    let nodes = d.click(&mut f, title("Bearbeiten"));
    let undo = widget(&nodes, "Rückgängig").expect("Bearbeiten-Menü sollte offen sein").center();
    let nodes = d.click(&mut f, undo);
    assert!(widget(&nodes, "Rückgängig").is_some(), "Deaktivierter Eintrag darf das Menü nicht schließen");
    d.key(&mut f, egui::Key::Escape);

    // Clicking the open menu's title closes it again.
    d.click(&mut f, ansicht);
    let nodes = d.click(&mut f, ansicht);
    assert!(widget(&nodes, "Raster").is_none(), "Zweiter Klick auf den Titel schließt das Menü");
}

#[test]
fn stepping_shows_prefetched_images_instantly() {
    let Some(mut f) = Fixture::new("prefetch") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    let (a, b) = (f.photo("_DSC0009.dng"), f.photo("_DSC0010.dng"));
    assert_eq!(f.app.current_path().as_ref(), Some(&a));

    // The neighbour is loaded and rendered in the background ...
    f.pump("Nachbar vorbereitet", |app| app.is_prepared(&b));
    // ... so stepping shows it without loading.
    f.app.step(1);
    assert!(f.app.loading.is_none(), "kein Laden nötig");
    assert_eq!(f.app.current_path().as_ref(), Some(&b));
    assert!(f.app.texture.is_some(), "fertig gerendert");

    // Going back is instant too: the previous image was kept with its rendering.
    f.app.step(-1);
    assert!(f.app.loading.is_none());
    assert_eq!(f.app.current_path().as_ref(), Some(&a));
    assert!(f.app.texture.is_some());

    // Both previews are now in the disk cache, so even a cold start shows them at once.
    let adj = sidecar::load(&a).adjustments;
    f.pump("Vorschau im Cache", |_| sk_core::cache::has_preview(&b, &adj, None));
}

#[test]
fn dragging_renders_drafts_then_full_resolution() {
    let Some(mut f) = Fixture::new("draft") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    let full = f.app.texture.as_ref().unwrap().size();
    assert_eq!(f.app.view_size, Some(egui::vec2(full[0] as f32, full[1] as f32)));

    // While a slider is dragged: quarter of the pixels, shown at the same size. (Without a
    // measured speed: a fast graphics card would not need a draft for this image.)
    f.app.workers.draft_speed = Default::default();
    f.app.pointer_down = true;
    f.app.edit(|adj| adj.exposure = 0.7);
    let v = f.app.edit_version;
    f.pump("Entwurf", |a| a.texture.as_ref().is_some_and(|t| t.size()[0] <= full[0] / 2 + 1) && a.edit_version == v);
    assert_eq!(f.app.image_size(), Some(egui::vec2(full[0] as f32, full[1] as f32)), "Koordinaten bleiben gleich");

    // Released: full resolution again, without a further edit.
    f.app.pointer_down = false;
    f.pump("volle Auflösung", |a| a.texture.as_ref().is_some_and(|t| t.size() == full));
    assert_eq!(f.app.edit_version, v);
}

#[test]
fn previews_are_generated_in_the_background_when_idle() {
    let Some(files) = samples() else { return };
    let dir = std::env::temp_dir().join(format!("sk-app-test-proxies-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let paths: Vec<PathBuf> = files
        .iter()
        .map(|f| {
            let target = dir.join(f.file_name().unwrap());
            std::fs::copy(f, &target).unwrap();
            target
        })
        .collect();
    let adj = sidecar::load(&paths[0]).adjustments;
    assert!(paths.iter().all(|p| !sk_core::cache::has_preview(p, &adj, None)));

    let ctx = egui::Context::default();
    let workers = crate::workers::Workers::spawn(ctx);
    let start = Instant::now();
    workers.proxies(paths.clone(), None);
    let (mut thumbs, mut finished) = (0, false);
    while !finished {
        assert!(start.elapsed() < LIMIT, "Zeitüberschreitung");
        match workers.events.recv_timeout(Duration::from_millis(100)) {
            Ok(crate::workers::Event::Thumbnail { .. }) => thumbs += 1,
            Ok(crate::workers::Event::ProxyProgress(None)) => finished = true,
            _ => {}
        }
    }
    // Nothing starts before the app has been idle for a while.
    assert!(start.elapsed() >= Duration::from_millis(1500));
    assert!(paths.iter().all(|p| sk_core::cache::has_preview(p, &adj, None)));
    assert_eq!(thumbs, paths.len(), "Thumbnails passen zur Entwicklung");

    // A second pass finds everything done.
    workers.proxies(paths.clone(), None);
    let again = Instant::now();
    while !matches!(workers.events.recv_timeout(Duration::from_secs(10)), Ok(crate::workers::Event::ProxyProgress(None))) {
        assert!(again.elapsed() < Duration::from_secs(30));
    }
    for p in &paths {
        sk_core::cache::remove(p, None);
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn zoom_100_is_one_image_pixel_per_screen_pixel_with_display_scaling() {
    let Some(mut f) = Fixture::new("hidpi") else { return };
    f.pump("Bild gerendert", |a| a.texture.is_some());
    let size = f.app.image_size().unwrap();
    let full_w = f.app.current.as_ref().unwrap().meta.info.width as f32;
    for ppp in [1.0, 1.5, 2.0] {
        f.app.pixels_per_point = ppp;
        f.app.zoom = None;
        f.app.toggle_zoom_100();
        // Screen pixels per full-resolution pixel (sensor width ~ output width without crop).
        let screen_px_per_image_px = f.app.zoom.unwrap() * ppp * size.x / full_w;
        assert!((screen_px_per_image_px - 1.0).abs() < 0.02, "ppp {ppp}: {screen_px_per_image_px}");
    }
}

#[test]
fn zoomed_in_drafts_render_the_view_from_the_full_resolution() {
    let Some(mut f) = Fixture::new("zoomdraft") else { return };
    f.pump("Vollauflösung", |a| a.texture.is_some() && a.current.as_ref().is_some_and(|c| c.full.is_some()));
    let size = f.app.image_size().unwrap();
    let viewport = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1500.0, 900.0));
    // 100 %: one full-resolution pixel per screen pixel.
    let full_w = f.app.current.as_ref().unwrap().full.as_ref().unwrap().width as f32;
    let scale = full_w / size.x;
    let img_rect = egui::Rect::from_center_size(viewport.center(), size * scale);

    // Dragging a slider while zoomed in: the view comes from the full resolution, reduced to
    // the draft budget (far sharper than the 6x magnified whole-image draft).
    f.app.pointer_down = true;
    f.app.edit(|adj| adj.exposure = 0.4);
    f.app.update_tiles(viewport, img_rect, scale, true);
    f.pump("Ausschnitt-Entwurf", |a| a.tiles.view.as_ref().is_some_and(|v| v.version == a.edit_version));
    let v = f.app.tiles.view.as_ref().unwrap();
    let [w, h] = v.texture.size();
    assert!(v.level >= 2 && v.level <= 3, "Stufe {}", v.level);
    assert!(w * h <= 800_000, "{w}x{h}");
    assert_eq!(w, v.region.w / v.level, "Textur = Ausschnitt / Stufe");
    assert!(v.region.w >= 1500, "deckt die Ansicht ab: {:?}", v.region);

    // Released: the view at full resolution.
    f.app.pointer_down = false;
    f.app.update_tiles(viewport, img_rect, scale, true);
    f.pump("scharfer Ausschnitt", |a| a.tiles.view.as_ref().is_some_and(|v| v.level == 1 && v.version == a.edit_version));
}

#[test]
fn navigator_moves_the_zoomed_view() {
    let Some(mut f) = Fixture::new("navigator") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    let size = f.app.image_size().unwrap();
    let scale = 2.0;
    f.app.zoom = Some(scale);
    viewer_frame(&mut f, vec![]);
    // Bottom right of the 1200 x 800 viewer, 200 px wide for a 3:2 image.
    let nav = egui::Rect::from_min_size(egui::pos2(1200.0 - 16.0 - 200.0, 800.0 - 16.0 - 200.0 / (size.x / size.y)), egui::vec2(200.0, 200.0 / (size.x / size.y)));
    let target = nav.lerp_inside(egui::vec2(0.1, 0.2));
    viewer_frame(&mut f, vec![egui::Event::PointerMoved(target)]);
    viewer_frame(&mut f, vec![press(target, true)]);
    viewer_frame(&mut f, vec![press(target, false)]);
    // That point of the image is now in the middle of the viewer.
    let expected = size * scale * (egui::vec2(0.5, 0.5) - egui::vec2(0.1, 0.2));
    assert!((f.app.pan - expected).length() < 3.0, "{:?} vs {expected:?}", f.app.pan);

    // Not in distraction-free full screen: the same click doesn't move the view.
    f.app.fullscreen = true;
    f.app.pan = egui::Vec2::ZERO;
    viewer_frame(&mut f, vec![]);
    viewer_frame(&mut f, vec![press(target, true)]);
    viewer_frame(&mut f, vec![press(target, false)]);
    assert_eq!(f.app.pan, egui::Vec2::ZERO);
}

#[test]
fn hovering_a_film_look_previews_it_without_changing_the_image() {
    let Some(mut f) = Fixture::new("looks") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    let a = f.photo("_DSC0009.dng");
    f.app.edit(|adj| adj.exposure = 0.5);
    f.app.commit_and_save();
    let before = f.app.current.as_ref().unwrap().adjustments.clone();

    // Hover: the viewer renders the look, nothing is saved or cached.
    let (velvia, _) = f.app.film_looks.iter().find(|(s, _)| s.name.contains("Velvia")).unwrap().clone();
    let look = sk_core::styles::apply_look(&velvia, &before);
    f.app.preview_style(look.clone());
    let v = f.app.edit_version;
    f.pump("Vorschau gerendert", |app| app.texture_version_for_test() == v);
    assert_eq!(f.app.current.as_ref().unwrap().adjustments, before);
    assert_eq!(sidecar::load(&a).adjustments, before);
    std::thread::sleep(Duration::from_millis(1200));
    f.pump("Ereignisse", |_| true);
    assert!(!sk_core::cache::has_preview(&a, &look, None), "Vorschau landet nicht im Cache");

    // Apply: only the look changes, exposure is kept.
    f.app.style_preview = None;
    f.app.set_adjustments_on(std::slice::from_ref(&a), |adj| sk_core::styles::apply_look(&velvia, adj));
    f.app.commit_and_save(); // (the app does this once the mouse is released)
    let saved = sidecar::load(&a).adjustments;
    assert_eq!(saved.exposure, 0.5);
    assert_eq!(saved.saturation, velvia.adjustments.saturation);
    assert_eq!(f.app.current.as_ref().unwrap().adjustments, saved);
}

#[test]
#[ignore = "langsam: läuft mit cargo t-all"]
fn proof_shows_the_image_as_the_recipe_writes_it() {
    let Some(mut f) = Fixture::new("proof") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    f.app.recipes = vec![sk_core::export::ExportRecipe { long_edge: Some(1200), quality: 70, ..Default::default() }];
    f.app.selected_recipe = 0;
    f.app.proof_view = true;
    let proof_ready = |f: &mut Fixture, what: &str| {
        let start = Instant::now();
        loop {
            f.app.update_proof(&f.ctx.clone());
            f.app.handle_events(&f.ctx.clone());
            f.app.send_render();
            if f.app.proof.as_ref().is_some_and(|p| p.version == f.app.edit_version) {
                break;
            }
            assert!(start.elapsed() < LIMIT, "Zeitüberschreitung: {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    proof_ready(&mut f, "Proof");
    let p = f.app.proof.as_ref().unwrap();
    assert_eq!(p.width.max(p.height), 1200, "Größe aus dem Rezept");
    assert!(p.file_size > 20_000 && p.file_size < 2_000_000, "Dateigröße {}", p.file_size);
    assert_eq!(p.texture.size(), [p.width, p.height]);
    let (size, key) = (p.file_size, p.key);

    // An edit or another recipe makes a new proof.
    f.app.edit(|adj| adj.exposure = 1.0);
    proof_ready(&mut f, "Proof nach Bearbeitung");
    assert_ne!(f.app.proof.as_ref().unwrap().file_size, size);
    f.app.recipes[0].quality = 95;
    assert_ne!(f.app.proof_key(), Some(key));
    f.app.edit(|adj| adj.exposure = 1.0); // (unchanged value, but a new edit version)
    proof_ready(&mut f, "Proof mit anderer Qualität");
    assert!(f.app.proof.as_ref().unwrap().file_size > size, "Qualität 95 > 70");
}

#[test]
fn fullscreen_toggles_and_esc_leaves_it_unless_a_tool_is_active() {
    let Some(mut f) = Fixture::new("fullscreen") else { return };
    let key = |k: egui::Key| vec![egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }];
    use egui::ViewportCommand as Cmd;
    // Runs one frame (the window maximized or not) and returns the window commands.
    let frame_in = |f: &mut Fixture, events: Vec<egui::Event>, maximized: bool| {
        let mut input = egui::RawInput { events, ..Default::default() };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().maximized = Some(maximized);
        let app = &mut f.app;
        let mut out = f.ctx.run_ui(input, |ui| app.shortcuts(ui.ctx()));
        out.textures_delta.clear();
        out.viewport_output.values().flat_map(|v| v.commands.clone()).collect::<Vec<Cmd>>()
    };
    let frame = |f: &mut Fixture, events: Vec<egui::Event>| frame_in(f, events, false);

    // From a maximized window: leave maximized before full screen (else Windows keeps the
    // taskbar area free: black band at the bottom), drop the shadow, and restore afterwards.
    let cmds = frame_in(&mut f, key(egui::Key::F11), true);
    let pos = |c: &Cmd| cmds.iter().position(|x| x == c);
    assert!(pos(&Cmd::Maximized(false)).unwrap() < pos(&Cmd::Fullscreen(true)).unwrap(), "{cmds:?}");
    assert!(pos(&Cmd::Fullscreen(true)).unwrap() < pos(&Cmd::Decorations(false)).unwrap(), "{cmds:?}");
    assert!(f.app.fullscreen);
    let cmds = frame(&mut f, key(egui::Key::F11));
    assert_eq!(cmds, vec![Cmd::Fullscreen(false), Cmd::Decorations(false), Cmd::Maximized(true)]);
    assert!(!f.app.fullscreen);

    assert!(frame(&mut f, key(egui::Key::F11)).contains(&Cmd::Fullscreen(true)), "Fenster wird auf Vollbild geschaltet");
    assert!(f.app.fullscreen);
    // With the crop tool active, Esc only ends the tool.
    f.app.crop_tool = Some(Default::default());
    frame(&mut f, key(egui::Key::Escape));
    assert!(f.app.fullscreen && f.app.crop_tool.is_none());
    frame(&mut f, key(egui::Key::Escape));
    assert!(!f.app.fullscreen);
}

#[test]
fn fullscreen_switch_shows_black_until_the_window_has_settled() {
    let Some(mut f) = Fixture::new("switch") else { return };
    // One frame at this window size and state; returns whether the switch is still running.
    let frame = |f: &mut Fixture, size: egui::Vec2, fullscreen: bool| {
        let mut input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)), ..Default::default() };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().fullscreen = Some(fullscreen);
        let app = &mut f.app;
        let mut switching = false;
        let mut out = f.ctx.run_ui(input, |ui| switching = app.screen_switching(ui.ctx()));
        out.textures_delta.clear();
        switching
    };
    let (window, monitor) = (egui::vec2(1600.0, 860.0), egui::vec2(1600.0, 900.0));
    frame(&mut f, window, false);
    let ctx = f.ctx.clone();
    ctx.run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, window)), ..Default::default() }, |ui| f.app.toggle_fullscreen(ui.ctx()))
        .textures_delta
        .clear();

    // The window manager hasn't done anything yet: still the old size, keep waiting.
    for _ in 0..4 {
        assert!(frame(&mut f, window, true));
    }
    // An in-between size (leaving maximized), then full screen: settled after it stays put.
    assert!(frame(&mut f, egui::vec2(1200.0, 700.0), true));
    assert!(frame(&mut f, monitor, true));
    assert!(frame(&mut f, monitor, true));
    assert!(!frame(&mut f, monitor, true), "angekommen");
    assert!(f.app.fade_in_from.is_some(), "Bild wird eingeblendet");
}

#[test]
#[ignore = "langsam: läuft mit cargo t-all"]
fn compare_view_and_matching() {
    let Some(mut f) = Fixture::new("compare") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    f.pump("Indexiert", |a| a.entries.len() == 2);
    let a = f.photo("_DSC0009.dng");
    let b = f.photo("_DSC0010.dng");
    assert!(f.app.compare_cells().is_empty(), "ein Bild: Einzelansicht");

    // Ctrl+click adds the second image: both side by side, the first stays the one being edited.
    f.app.select_path(&b, egui::Modifiers::COMMAND);
    assert_eq!(f.app.compare_cells(), vec![a.clone(), b.clone()]);
    assert_eq!(f.app.current_path().as_ref(), Some(&a));
    let cells = f.app.compare_cells();
    f.app.request_compare_images(&cells);
    f.pump("Vergleichsbild", |app| app.compare_textures.contains_key(&b));
    // Turned off: back to the single view.
    f.app.multi_view = false;
    assert!(f.app.compare_cells().is_empty());
    f.app.multi_view = true;

    // The first image, brightened, becomes the reference; it stays next to the second.
    f.app.edit(|adj| adj.exposure = 1.0);
    f.app.commit_and_save();
    f.pump("neu gerendert", |app| app.texture_version_for_test() == app.edit_version && !app.texture_draft);
    f.app.toggle_reference();
    assert_eq!(f.app.compare_ref.as_ref(), Some(&a));
    f.app.select_path(&b, egui::Modifiers::NONE);
    f.pump("zweites Bild", |app| app.current_path().as_ref() == Some(&b) && app.texture.is_some());
    assert_eq!(f.app.compare_cells(), vec![a.clone(), b.clone()]);
    // Leaving the edited image keeps its new rendering for the compare view.
    assert!(f.app.compare_textures.contains_key(&a));

    // Matching changes exposure and white balance of the current image, as one undo step.
    assert!(f.app.can_match());
    f.app.match_selection(sk_core::auto::MatchWhat { exposure: true, white_balance: true });
    f.pump("angeglichen", |app| app.match_progress.is_none());
    let adj = f.app.current.as_ref().unwrap().adjustments.clone();
    assert!(adj.exposure > 0.3, "Belichtung {}", adj.exposure);
    assert_eq!(adj.white_balance.mode, sk_core::params::WbMode::Custom);
    f.app.commit_and_save();
    assert_eq!(sidecar::load(&b).adjustments.exposure, adj.exposure);
    f.app.undo();
    assert_eq!(f.app.current.as_ref().unwrap().adjustments.exposure, 0.0);

    // Shift+Enter on the reference unpins it.
    f.app.select_path(&a, egui::Modifiers::NONE);
    f.app.toggle_reference();
    assert_eq!(f.app.compare_ref, None);
}

#[test]
fn skin_tone_pipette_sets_target() {
    let Some(mut f) = Fixture::new("skin") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    let size = f.app.image_size().unwrap();
    f.app.picking_skin = true;
    let before = f.app.current.as_ref().unwrap().adjustments.skin_tone;
    f.app.pick_skin_tone(size / 2.0);
    let st = f.app.current.as_ref().unwrap().adjustments.skin_tone;
    // Either a target was taken (and the pipette ends), or the spot was too grey.
    if f.app.status.starts_with("Hautton: Farbton") {
        assert!(!f.app.picking_skin);
        assert!(st.hue != before.hue || st.chroma != before.chroma);
        assert!((0.0..360.0).contains(&st.hue) && st.chroma > 0.0);
    } else {
        assert_eq!(st, before);
    }
}

#[test]
fn variants_workflow() {
    let Some(mut f) = Fixture::new("variants") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    f.pump("Indexiert", |a| a.lists.total == 2);
    let a = f.photo("_DSC0009.dng");
    f.app.edit(|adj| adj.exposure = 0.7);
    f.app.set_rating(3);
    f.app.commit_and_save();

    // A clone takes the settings; it is shown and listed right after its file.
    f.app.new_variant(true);
    let v2 = sk_core::variant::path_for(&a, 2);
    assert_eq!(f.app.primary_path().as_ref(), Some(&v2));
    let names: Vec<String> = f.app.visible.iter().map(|&i| f.app.entries[i].name.clone()).collect();
    assert_eq!(names, ["_DSC0009.dng", "_DSC0009.dng (2)", "_DSC0010.dng"]);
    f.pump("Variante geladen", |app| app.current.is_some() && app.texture.is_some());
    assert_eq!(f.app.current.as_ref().unwrap().adjustments.exposure, 0.7);
    assert_eq!(f.app.current.as_ref().unwrap().rating, 3);

    // Editing the variant leaves the file's own settings alone.
    f.app.edit(|adj| adj.black_white.enabled = true);
    f.app.set_rating(5);
    f.app.commit_and_save();
    assert!(sidecar::load(&v2).adjustments.black_white.enabled);
    assert!(!sidecar::load(&a).adjustments.black_white.enabled);
    assert_eq!(sidecar::load(&a).rating, 3);
    assert_eq!(f.app.lists.total, 3, "Variante im Katalog");

    // A new variant starts from default settings.
    f.app.new_variant(false);
    let v3 = sk_core::variant::path_for(&a, 3);
    f.pump("neue Variante geladen", |app| app.current_path().as_ref() == Some(&v3));
    assert_eq!(f.app.current.as_ref().map(|c| c.adjustments.exposure), Some(0.0));

    // Export names variants apart.
    let recipe = sk_core::export::ExportRecipe::default();
    let names = |p: &PathBuf| recipe.output_path(&sk_core::naming::NameFields { image: p, capture: None, counter: 1, session: "" });
    assert_ne!(names(&a), names(&v2));

    // Deleting asks first, then removes only that variant and shows the file.
    f.app.request_delete_variant();
    assert_eq!(f.app.confirm_delete_variant, Some(vec![v3.clone()]));
    f.app.delete_variants(std::slice::from_ref(&v3));
    assert_eq!(f.app.primary_path().as_ref(), Some(&a));
    assert_eq!(sk_core::variant::of_file(&a), vec![a.clone(), v2.clone()]);
    assert_eq!(f.app.lists.total, 3);
    // The file's first variant can't be deleted that way.
    f.pump("Datei geladen", |app| app.current_path().as_ref() == Some(&a));
    f.app.request_delete_variant();
    assert!(f.app.confirm_delete_variant.is_none());

    // Moving the file takes its variants along.
    let dest = f.dir.join("moved");
    f.app.select_path(&v2, egui::Modifiers::NONE);
    f.app.move_selection_to(&dest);
    let moved = dest.canonicalize().unwrap().join("_DSC0009.dng");
    assert!(moved.exists());
    assert!(sidecar::load(&sk_core::variant::path_for(&moved, 2)).adjustments.black_white.enabled);
}

#[test]
fn focus_mask_marks_sharp_areas() {
    let Some(mut f) = Fixture::new("focus") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    let a = f.photo("_DSC0009.dng");
    f.app.toggle_focus_mask();
    // The viewer asks for a rendering whose sharpness then gets analysed.
    f.app.ensure_focus_of_current();
    f.pump("Fokusmaske berechnet", |app| app.focus_maps.contains_key(&a));
    let map = &f.app.focus_maps[&a];
    let size = f.app.image_size().unwrap();
    assert!(((map.size[0] as f32 / map.size[1] as f32) - size.x / size.y).abs() < 0.01);
    // Some, but not all of a real photo is in focus; more sensitivity marks more.
    let share = |s: f32| {
        let t = sk_core::focus::threshold(s);
        map.sharpness.iter().filter(|v| **v >= t).count() as f32 / map.sharpness.len() as f32
    };
    let (low, mid, high) = (share(0.0), share(50.0), share(100.0));
    assert!(mid > 0.001 && mid < 0.8, "{mid}");
    assert!(low <= mid && mid <= high);
    let id = f.app.focus_overlay(&f.ctx.clone(), &a, size.x / size.y);
    assert!(id.is_some());
    // A crop changes the framing: the old mask no longer fits.
    assert!(f.app.focus_overlay(&f.ctx.clone(), &a, size.x / size.y * 0.5).is_none());
}

#[test]
fn focus_maps_of_images_out_of_view_are_dropped() {
    let Some(mut f) = Fixture::new("prune") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    let shown = f.photo("_DSC0009.dng");
    let mut received = |path: PathBuf| {
        f.app.focus_pending.insert(path.clone());
        f.app.focus_received(path, None, [1, 1], std::sync::Arc::new(vec![0]));
    };
    received(shown.clone());
    for i in 0..100 {
        received(f.dir.join(format!("weg{i}.dng")));
    }
    f.app.prune_view_caches();
    assert!(f.app.focus_maps.len() < 30, "{}", f.app.focus_maps.len());
    assert!(f.app.focus_maps.contains_key(&shown));
}

/// One frame of the compare view on a 1600x1000 screen.
fn compare_frame(f: &mut Fixture, cells: &[PathBuf]) {
    let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))), ..Default::default() };
    let ctx = f.ctx.clone();
    let out = ctx.run_ui(input, |ui| {
        let rect = ui.max_rect();
        f.app.compare_view(ui, rect, cells);
    });
    let mut delta = out.textures_delta;
    delta.clear();
}

#[test]
#[ignore = "langsam: läuft mit cargo t-all"]
fn compare_zoom_shows_full_resolution() {
    let Some(mut f) = Fixture::new("compare-detail") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    f.pump("Indexiert", |a| a.entries.len() == 2);
    let (a, b) = (f.photo("_DSC0009.dng"), f.photo("_DSC0010.dng"));
    f.app.select_path(&b, egui::Modifiers::COMMAND);
    let cells = f.app.compare_cells();
    assert_eq!(cells.len(), 2);
    f.app.toggle_focus_mask();
    // Fitted: the previews are enough.
    compare_frame(&mut f, &cells);
    assert!(f.app.details_requested.is_empty());
    // Zoomed far in: the visible parts come from the full resolution, with their focus mask.
    f.app.compare_zoom = 12.0;
    f.app.compare_center = egui::vec2(0.4, 0.5);
    let start = Instant::now();
    while !cells.iter().all(|p| f.app.compare_details.contains_key(p)) {
        assert!(start.elapsed() < LIMIT, "Zeitüberschreitung: Details");
        f.app.handle_events(&f.ctx);
        compare_frame(&mut f, &cells);
        std::thread::sleep(Duration::from_millis(20));
    }
    for p in [&a, &b] {
        let d = &f.app.compare_details[p];
        assert!(d.full_res, "{}", p.display());
        assert!(d.width > crate::workers::PREVIEW_EDGE as f32, "Breite {}", d.width);
        assert!(d.uv.contains(egui::pos2(0.4, 0.5)), "{:?}", d.uv);
        assert!(d.uv.width() < 0.5);
        assert!(d.focus.is_some());
    }
    // An edit of the image being edited makes its part outdated: rendered again.
    f.app.edit(|adj| adj.exposure = 0.5);
    compare_frame(&mut f, &cells);
    let v = f.app.edit_version;
    f.pump("Detail neu", |app| app.compare_details[&a].version == Some(v));
}

#[test]
#[ignore = "langsam: läuft mit cargo t-all"]
fn focus_mask_at_full_resolution_when_zoomed_in() {
    let Some(mut f) = Fixture::new("focus-tiles") else { return };
    f.pump("Vollauflösung", |a| a.texture.is_some() && a.current.as_ref().is_some_and(|c| c.full.is_some()));
    f.app.pixels_per_point = 1.0;
    f.app.toggle_zoom_100();
    // Zoomed in without the mask: plain tiles.
    let start = Instant::now();
    while f.app.tiles.view.is_none() && f.app.tiles.textures.is_empty() {
        assert!(start.elapsed() < LIMIT, "Zeitüberschreitung: Kacheln");
        viewer_frame(&mut f, vec![]);
        f.app.handle_events(&f.ctx);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(f.app.tiles.focus.is_empty() && f.app.tiles.view.as_ref().is_none_or(|v| v.focus.is_none()));
    // With the mask, the visible part is rendered again and analysed at full resolution.
    f.app.toggle_focus_mask();
    let start = Instant::now();
    while !(f.app.tiles.view.as_ref().is_some_and(|v| v.focus.is_some()) || !f.app.tiles.focus.is_empty()) {
        assert!(start.elapsed() < LIMIT, "Zeitüberschreitung: Fokus der Kacheln");
        viewer_frame(&mut f, vec![]);
        f.app.handle_events(&f.ctx);
        std::thread::sleep(Duration::from_millis(20));
    }
    let map = f.app.tiles.view.as_ref().and_then(|v| v.focus.as_ref()).or_else(|| f.app.tiles.focus.values().next()).unwrap();
    assert!(map.size[0] >= 512, "{:?}", map.size);
    viewer_frame(&mut f, vec![]);
}

#[test]
fn focus_mask_follows_edits_made_while_it_was_off() {
    let Some(mut f) = Fixture::new("focus-stale") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    let a = f.photo("_DSC0009.dng");
    f.app.toggle_focus_mask();
    f.app.ensure_focus_of_current();
    f.pump("Fokusmaske", |app| app.focus_map_current(&a));
    // Off, edited (e.g. rotated), on again: the old mask doesn't fit and isn't shown ...
    f.app.toggle_focus_mask();
    f.app.edit(|adj| adj.exposure = 0.7);
    f.pump("neu gerendert", |app| app.texture_version_shown() == app.edit_version && !app.texture_draft);
    f.app.toggle_focus_mask();
    assert!(!f.app.focus_map_current(&a));
    // ... until it is computed again for the shown rendering.
    f.app.ensure_focus_of_current();
    f.pump("Fokusmaske neu", |app| app.focus_map_current(&a));
    assert_eq!(f.app.focus_maps[&a].version, Some(f.app.texture_version_shown()));
}

#[test]
#[ignore = "langsam: läuft mit cargo t-all"]
fn changed_settings_drop_images_kept_in_memory() {
    let Some(mut f) = Fixture::new("prepared") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    f.pump("Indexiert", |a| a.entries.len() == 2);
    let (a, b) = (f.photo("_DSC0009.dng"), f.photo("_DSC0010.dng"));
    f.app.select_path(&b, egui::Modifiers::NONE);
    f.pump("zweites Bild", |app| app.current_path().as_ref() == Some(&b) && app.texture.is_some());
    assert!(f.app.prepared_texture(&a).is_some());
    // E.g. matched to a reference: the kept rendering is outdated.
    f.app.set_adjustments_on(std::slice::from_ref(&a), |adj| sk_core::params::Adjustments { exposure: 1.0, ..adj.clone() });
    assert!(f.app.prepared_texture(&a).is_none());
    assert!(!f.app.compare_textures.contains_key(&a));
}

#[test]
fn trashing_variants_deletes_only_them() {
    let Some(mut f) = Fixture::new("trash-variant") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    let a = f.photo("_DSC0009.dng");
    f.app.new_variant(true);
    let v2 = sk_core::variant::path_for(&a, 2);
    assert_eq!(f.app.primary_path().as_ref(), Some(&v2));
    f.app.trash_selection();
    assert_eq!(f.app.confirm_delete_variant, Some(vec![v2.clone()]));
    f.app.delete_variants(&[v2]);
    assert!(a.exists());
    assert_eq!(sk_core::variant::of_file(&a), vec![a.clone()]);
}

#[test]
fn auto_upright_sets_geometry_as_one_undo_step() {
    let Some(mut f) = Fixture::new("upright") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    let undo = f.app.undo.len();
    f.app.auto_upright(sk_core::upright::UprightMode::Vertical);
    assert!(f.app.status.starts_with("Automatisch geraderichten"), "{}", f.app.status);
    let g = f.app.current.as_ref().unwrap().adjustments.geometry.clone();
    assert!(g.straighten.abs() <= 15.0 && g.keystone_h == 0.0);
    if g != Default::default() {
        assert_eq!(f.app.undo.len(), undo + 1);
        f.app.undo();
        assert_eq!(f.app.current.as_ref().unwrap().adjustments.geometry, Default::default());
    }
}

#[test]
fn batch_rename_keeps_settings_variants_and_selection() {
    let Some(mut f) = Fixture::new("rename") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    let a = f.photo("_DSC0009.dng");
    f.app.select_path(&a, egui::Modifiers::NONE);
    f.pump("A geladen", |app| app.current_path().as_ref() == Some(&a) && app.loading.is_none());
    f.app.edit(|adj| adj.exposure = 0.8);
    f.app.new_variant(true);
    let v2 = sk_core::variant::path_for(&a, 2);
    f.pump("Variante", |app| app.current_path().as_ref() == Some(&v2) && app.loading.is_none());

    f.app.select_all();
    f.app.open_rename();
    let files = f.app.rename.as_ref().map(|d| d.template.clone());
    assert_eq!(files.as_deref(), Some("{Name}"));
    let all: Vec<PathBuf> = sk_core::variant::files_of(&f.app.selected_paths());
    let (n, error) = f.app.rename_files(&all, "Test_{Zähler:2}", 1);
    assert_eq!((n, error), (2, None));
    let new = f.dir.join("photos").join("Test_01.dng").canonicalize().unwrap();
    assert!(!a.exists() && new.exists());
    assert_eq!(sidecar::load(&new).adjustments.exposure, 0.8, "Einstellungen wandern mit");
    assert_eq!(sidecar::load(&sk_core::variant::path_for(&new, 2)).adjustments.exposure, 0.8, "Variante auch");
    // The image being edited stays open under its new name.
    let v2_new = sk_core::variant::path_for(&new, 2);
    assert_eq!(f.app.primary_path(), Some(v2_new.clone()));
    f.pump("neu geladen", |app| app.current_path().as_ref() == Some(&v2_new));
    assert!(f.app.entries.iter().any(|e| e.path == new) && !f.app.entries.iter().any(|e| e.path == a));
}

#[test]
fn renamed_files_do_not_inherit_what_was_kept_for_their_new_path() {
    let Some(mut f) = Fixture::new("rename-reuse") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    let (a, b) = (f.photo("_DSC0009.dng"), f.photo("_DSC0010.dng"));
    assert_eq!(f.app.current_path().as_ref(), Some(&a));
    f.pump("Nachbar vorbereitet", |app| app.is_prepared(&b));
    // Another file takes the neighbour's name (like renumbering to close gaps).
    std::fs::copy(&a, f.dir.join("photos").join("c.dng")).unwrap();
    let c = f.photo("c.dng");
    assert_eq!(f.app.rename_files(std::slice::from_ref(&b), "Weg", 1), (1, None));
    assert_eq!(f.app.rename_files(std::slice::from_ref(&c), "_DSC0010", 1), (1, None));
    assert!(!f.app.is_prepared(&b), "die Vorschau gehört zur anderen Datei");
    // A rendering requested before the renames is dropped when it arrives.
    f.app.workers.prefetch(vec![b.clone()], None, f.app.relocations - 1);
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        f.app.handle_events(&f.ctx);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!f.app.is_prepared(&b), "veraltetes Ergebnis verworfen");
}

#[test]
fn history_jumps_and_snapshots_persist() {
    let Some(mut f) = Fixture::new("history") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    let path = f.app.current_path().unwrap();
    for ev in [0.5, 1.0, 1.5] {
        f.app.edit(|a| a.exposure = ev);
        f.app.commit_and_save();
    }
    let (steps, now) = f.app.history_steps().unwrap();
    assert_eq!((steps.len(), now), (4, 3));
    assert_eq!(sk_core::history::describe(steps[1], steps[2]), "Belichtung 1");

    // Back to the first edit, then forward again.
    f.app.jump_history(1);
    assert_eq!(f.app.current.as_ref().unwrap().adjustments.exposure, 0.5);
    assert_eq!(f.app.history_steps().unwrap().0.len(), 4, "spätere Schritte bleiben erreichbar");
    f.app.jump_history(3);
    assert_eq!(f.app.current.as_ref().unwrap().adjustments.exposure, 1.5);

    // A snapshot lives in the sidecar and survives ordinary saving.
    f.app.add_snapshot(Some("Hell".into()));
    f.app.edit(|a| a.contrast = 30.0);
    f.app.commit_and_save();
    let sc = sidecar::load(&path);
    assert_eq!(sc.snapshots.len(), 1);
    assert_eq!((sc.snapshots[0].name.as_str(), sc.snapshots[0].adjustments.exposure, sc.snapshots[0].adjustments.contrast), ("Hell", 1.5, 0.0));
    assert_eq!(sc.adjustments.contrast, 30.0);
}

#[test]
fn showing_a_mask_renders_preview_and_tiles_again() {
    let Some(mut f) = Fixture::new("mask-tiles") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    f.app.edit(|a| a.layers.push(sk_core::params::Layer::default()));
    f.app.active_layer = Some(0);
    f.app.send_render();
    let v = f.app.edit_version;
    // M: everything rendered so far (also the full-resolution tiles) lacks the tint.
    f.app.show_mask = true;
    f.app.send_render();
    assert!(f.app.edit_version > v, "Maske an: neu rendern");
    let v = f.app.edit_version;
    f.app.send_render();
    assert_eq!(f.app.edit_version, v, "nur einmal");
    f.app.show_mask = false;
    f.app.send_render();
    assert!(f.app.edit_version > v, "Maske aus: neu rendern");
}

#[test]
fn last_selected_image_is_selected_again_after_starting() {
    let Some(mut f) = Fixture::new("restore") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    let (a, b) = (f.photo("_DSC0009.dng"), f.photo("_DSC0010.dng"));
    assert_eq!(f.app.primary_path(), Some(a.clone()), "ohne Vorgabe das erste");
    // As if started with `last_image` = b in the settings.
    f.app.restore_image = Some(b.clone());
    let folder = f.dir.join("photos").canonicalize().unwrap();
    f.app.close_current();
    f.app.open_folder(folder.clone());
    assert_eq!(f.app.primary_path(), Some(b.clone()));
    f.pump("B geladen", |app| app.current_path().as_ref() == Some(&b));
    // Only once: opening the folder later starts with the current image as usual.
    assert!(f.app.restore_image.is_none());
}

#[test]
fn xmp_exchange() {
    let Some(mut f) = Fixture::new("xmp") else { return };
    f.pump("Indexiert", |a| a.lists.total == 2);
    let a = f.photo("_DSC0009.dng");
    let b = f.photo("_DSC0010.dng");
    // Written by another program: read on request and mirrored in catalog and browser.
    std::fs::write(f.dir.join("photos/_DSC0010.xmp"), sk_core::xmp::updated(None, 3, ColorLabel::Green, &["Lightroom".into()]).unwrap()).unwrap();
    f.app.selection = [b.clone()].into_iter().collect();
    f.app.read_xmp_selection();
    let e = &f.app.entries[f.app.index_of(&b).unwrap()];
    assert_eq!((e.rating, e.color_label, e.keywords.clone()), (3, ColorLabel::Green, vec!["Lightroom".to_string()]));
    assert_eq!(sidecar::load(&b).rating, 3);
    assert!(f.app.lists.keywords.iter().any(|(k, _)| k == "Lightroom"));
    // Written on request for another image.
    f.app.selection = [a.clone()].into_iter().collect();
    f.app.set_rating(5);
    f.app.write_xmp_selection();
    let x = sk_core::xmp::parse(&std::fs::read_to_string(f.dir.join("photos/_DSC0009.xmp")).unwrap()).unwrap();
    assert_eq!(x.rating, Some(5));
}

#[test]
fn stacks_collapse_and_expand() {
    use crate::stacks::StackAction;
    let Some(mut f) = Fixture::new("stacks") else { return };
    f.pump("Indexiert", |a| a.lists.total == 2 && a.entries.iter().all(|e| e.capture_date.is_some()));
    let a = f.photo("_DSC0009.dng");
    let b = f.photo("_DSC0010.dng");
    f.app.selection = [a.clone(), b.clone()].into_iter().collect();
    f.app.make_primary(&b);
    f.app.run_stack_action(StackAction::Create);
    // Collapsed: only the top image (the one in the viewer) is shown.
    assert_eq!(f.app.visible.len(), 1);
    assert_eq!(f.app.entries[f.app.visible[0]].path, b);
    assert!(sidecar::load(&a).stack.is_some_and(|s| !s.top));
    assert_eq!(f.app.stack_size(f.app.visible[0]), 2);
    // A variant joins its file's stack.
    f.app.select_path(&a, egui::Modifiers::NONE);
    f.pump("a geladen", |app| app.current.is_some() && app.loading.is_none());
    f.app.new_variant(false);
    assert_eq!(f.app.stack_size(f.app.visible[0]), 3);
    // Expanded: all of them, the top image first.
    f.app.run_stack_action(StackAction::ExpandAll);
    assert_eq!(f.app.visible.len(), 3);
    assert_eq!(f.app.entries[f.app.visible[0]].path, b);
    // The catalog knows the stack after reloading.
    f.app.reload_entries();
    assert_eq!(f.app.visible.len(), 3);
    f.app.run_stack_action(StackAction::CollapseAll);
    assert_eq!(f.app.visible.len(), 1);
    // Another top image.
    f.app.run_stack_action(StackAction::ExpandAll);
    f.app.select_path(&a, egui::Modifiers::NONE);
    f.app.run_stack_action(StackAction::MakeTop);
    f.app.run_stack_action(StackAction::CollapseAll);
    assert_eq!(f.app.entries[f.app.visible[0]].path, a);
    // Dissolved.
    f.app.run_stack_action(StackAction::Dissolve);
    assert_eq!(f.app.visible.len(), 3);
    assert!(sidecar::load(&a).stack.is_none() && sidecar::load(&b).stack.is_none());
    // Both samples were taken within a short time: automatic stacking finds them.
    f.app.selection.clear();
    f.app.auto_stack_by_time(3600.0);
    assert_eq!(f.app.visible.len(), 1, "{}", f.app.status);
}

#[test]
fn hdr_merge_job_creates_and_selects_dng() {
    let Some(mut f) = Fixture::new("hdr") else { return };
    f.pump("Indexiert", |a| a.lists.total == 2);
    f.app.select_all();
    f.app.open_merge(crate::merge::MergeKind::Hdr);
    let dialog = f.app.merge_dialog.take().expect("Dialog");
    assert_eq!(dialog.files.len(), 2);
    let files = dialog.files.clone();
    let opts = f.app.hdr_options;
    f.app.start_job("HDR zusammenfügen", Box::new(move |p| Ok(vec![sk_core::hdr::merge_files(&files, &opts, p)?])));
    assert!(f.app.activities().iter().any(|a| a.label == "HDR zusammenfügen" && a.progress.is_some() && !a.in_viewer));
    f.pump("HDR fertig", |a| a.jobs.is_empty());
    let dng = f.dir.join("photos/_DSC0009-HDR.dng");
    assert!(dng.exists(), "{}", f.app.status);
    f.pump("DNG ausgewählt", |a| a.current_path().is_some_and(|p| p.ends_with("_DSC0009-HDR.dng")) && a.texture.is_some());
    assert!(f.app.current.as_ref().unwrap().meta.hdr);
    f.pump("Indexiert", |a| a.lists.total == 3);
}

#[test]
fn remove_tool_fills_the_painted_area() {
    if !sk_ai::models::available(&sk_ai::models::INPAINT) {
        return;
    }
    let Some(mut f) = Fixture::new("remove") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    f.app.set_local_tool(crate::layers::LocalTool::Remove);
    f.app.remove_strokes = vec![sk_core::params::BrushStroke { points: vec![[0.4, 0.5], [0.45, 0.52]], radius: 0.01, hardness: 0.7, flow: 1.0, erase: false, opacity: 1.0, auto_mask: false, build_up: true }];
    f.app.request_removal();
    assert!(f.app.inpaint_busy);
    // Shown in the viewer and the status bar while it runs.
    assert!(f.app.activities().iter().any(|a| a.label == "Entfernen" && a.in_viewer));
    f.pump("Entfernt", |a| !a.inpaint_busy);
    let cur = f.app.current.as_ref().unwrap();
    assert_eq!(cur.adjustments.removals.len(), 1, "{}", f.app.status);
    assert!(f.app.remove_strokes.is_empty());
    // Saved with the image, and one undo step.
    f.app.commit_and_save();
    let path = f.app.current_path().unwrap();
    assert_eq!(sidecar::load(&path).adjustments.removals.len(), 1);
    f.app.undo();
    assert!(f.app.current.as_ref().unwrap().adjustments.removals.is_empty());
}

#[test]
fn culling_analysis_filters_and_sharpest_on_top() {
    use sk_core::catalog::CullFilter;
    if !sk_ai::faces::NEEDED.iter().all(|m| sk_ai::models::available(m)) {
        return;
    }
    let Some(mut f) = Fixture::new("culling") else { return };
    f.pump("Indexiert", |a| a.lists.total == 2);
    f.app.analyze_view(false);
    f.pump("Analysiert", |a| a.jobs.is_empty());
    f.pump("Neu geladen", |a| a.entries.iter().all(|e| e.culling.is_some()));
    for e in &f.app.entries {
        let c = e.culling.unwrap();
        assert_eq!((c.faces, c.eyes_closed), (0, 0), "{}", e.name);
        assert!(!c.blurry(), "{}: {}", e.name, c.sharpness);
    }
    // Filters.
    f.app.filter.culling = Some(CullFilter::Blurry);
    f.app.refilter();
    assert!(f.app.visible.is_empty());
    f.app.filter.culling = Some(CullFilter::NotAnalyzed);
    f.app.refilter();
    assert!(f.app.visible.is_empty());
    f.app.filter.culling = None;
    f.app.refilter();
    // The sharper of the two goes on top of their stack.
    f.app.select_all();
    f.app.run_stack_action(crate::stacks::StackAction::Create);
    f.app.run_stack_action(crate::stacks::StackAction::SharpestTop);
    let sharpest = f.app.entries.iter().max_by(|a, b| a.culling.unwrap().sharpness.total_cmp(&b.culling.unwrap().sharpness)).unwrap().path.clone();
    assert!(sidecar::load(&sharpest).stack.unwrap().top);
    // Nothing left to analyse.
    f.app.analyze_view(false);
    assert!(f.app.jobs.is_empty());
}

/// Wraps a JPEG into a DNG (the JPEG as its preview), so that the app treats it as a raw file.
fn dng_from_jpeg(jpeg: &Path, out: &Path, mirror: bool) {
    let mut img = image::open(jpeg).unwrap().to_rgb8();
    if mirror {
        img = image::imageops::flip_horizontal(&img);
    }
    let (w, h) = (img.width() as usize, img.height() as usize);
    let data: Vec<[f32; 3]> = img.pixels().map(|p| p.0.map(|v| sk_core::color::srgb_decode(v as f32 / 255.0))).collect();
    let info = sk_core::raw::CaptureInfo { make: "Test".into(), model: "JPEG".into(), ..Default::default() };
    let xyz_to_cam = sk_core::color::mat_inv(&sk_core::color::SRGB_TO_XYZ).unwrap();
    sk_core::dng::write_linear_dng(out, &sk_core::dng::LinearDng { width: w, height: h, data: &data, xyz_to_cam, as_shot_wb: [1.0; 3], info: &info, baseline_exposure: 0.0, preview: Some(&img) }).unwrap();
}

#[test]
fn faces_become_named_persons() {
    let Some(faces) = std::env::var_os("SILBERKORN_FACE_TEST").map(PathBuf::from) else { return };
    if !sk_ai::faces::NEEDED.iter().all(|m| sk_ai::models::available(m)) {
        return;
    }
    let Some(mut f) = Fixture::new("persons") else { return };
    let photos = f.dir.join("photos");
    dng_from_jpeg(&faces.join("obama.jpg"), &photos.join("p1.dng"), false);
    dng_from_jpeg(&faces.join("obama.jpg"), &photos.join("p2.dng"), true);
    dng_from_jpeg(&faces.join("dreis.jpg"), &photos.join("p3.dng"), false);
    f.app.open_folder(photos.clone());
    f.pump("Indexiert", |a| a.lists.total == 5);
    f.app.analyze_view(false);
    f.pump("Analysiert", |a| a.jobs.is_empty());
    f.pump("Neu geladen", |a| a.entries.iter().all(|e| e.culling.is_some()));
    let rec = |a: &App, n: &str| a.entries.iter().find(|e| e.name == n).unwrap().clone();
    assert_eq!(rec(&f.app, "p1.dng").culling.unwrap().faces, 1);
    assert_eq!(rec(&f.app, "p3.dng").culling.unwrap().eyes_closed, 1);
    // The same man twice: one person (seen in two images); she is seen once.
    assert_eq!(f.app.lists.persons.len(), 1, "{:?}", f.app.lists.persons);
    let person = f.app.lists.persons[0].clone();
    assert_eq!(person.count, 2);
    f.app.name_person(person.id, "Barack");
    for p in ["p1.dng", "p2.dng"] {
        assert!(sidecar::load(&f.photo(p)).keywords.contains(&"Barack".to_string()), "{p}");
    }
    assert_eq!(f.app.lists.persons[0].name.as_deref(), Some("Barack"));
    f.app.open_scope(sk_core::catalog::Scope::Person(person.id));
    assert_eq!(f.app.entries.len(), 2);
    // Renamed: the keyword follows.
    f.app.name_person(person.id, "Obama");
    let kw = sidecar::load(&f.photo("p1.dng")).keywords;
    assert!(kw.contains(&"Obama".to_string()) && !kw.contains(&"Barack".to_string()), "{kw:?}");
}

/// Only one viewer tool is active at a time: the viewer hands the mouse to the first active
/// one, so a tool left on (e.g. the brush) would swallow the input of the one chosen after it.
#[test]
#[ignore = "langsam: läuft mit cargo t-all"]
fn choosing_a_viewer_tool_ends_the_others() {
    use crate::layers::LocalTool;
    let Some(mut f) = Fixture::new("viewer_tools") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    let active = |a: &App| {
        [a.local_tool.is_some(), a.crop_tool.is_some(), a.straighten_tool.is_some(), a.picking_wb, a.picking_skin]
            .iter()
            .filter(|on| **on)
            .count()
    };
    type Switch = (&'static str, fn(&mut App), fn(&App) -> bool);
    let tools: [Switch; 6] = [
        ("Pinsel", |a| a.set_local_tool(LocalTool::Brush), |a| a.local_tool == Some(LocalTool::Brush)),
        ("Verlauf", |a| a.set_local_tool(LocalTool::Linear), |a| a.local_tool == Some(LocalTool::Linear)),
        ("Zuschneiden", |a| a.start_crop_tool(), |a| a.crop_tool.is_some()),
        ("Geraderichten", |a| a.toggle_straighten(), |a| a.straighten_tool.is_some()),
        ("WB-Pipette", |a| a.toggle_wb_picker(), |a| a.picking_wb),
        ("Hautton-Pipette", |a| a.toggle_skin_picker(), |a| a.picking_skin),
    ];
    for (first, on_first, _) in &tools {
        for (second, on_second, is_on) in &tools {
            if first == second {
                continue;
            }
            f.app.end_viewer_tools();
            on_first(&mut f.app);
            on_second(&mut f.app);
            assert!(is_on(&f.app), "{second} nach {first} sollte aktiv sein");
            assert_eq!(active(&f.app), 1, "nach {first} → {second} ist mehr als ein Werkzeug aktiv");
        }
    }
    // Choosing the active tool again turns it off.
    f.app.end_viewer_tools();
    f.app.toggle_straighten();
    f.app.toggle_straighten();
    assert_eq!(active(&f.app), 0);
}

/// One frame of the whole window (all panels), `t` seconds after the start.
fn window_frame(f: &mut Fixture, frame: &mut eframe::Frame, t: f64, events: Vec<egui::Event>) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1500.0, 950.0))),
        time: Some(t),
        events,
        ..Default::default()
    };
    let app = &mut f.app;
    let out = f.ctx.run_ui(input, |ui| eframe::App::ui(app, ui, frame));
    let mut delta = out.textures_delta;
    delta.clear();
}

/// Presses at `a`, moves to `b` over `hold` seconds (frames every 40 ms) and releases.
fn window_stroke(f: &mut Fixture, frame: &mut eframe::Frame, t: &mut f64, a: egui::Pos2, b: egui::Pos2, hold: f64) {
    *t += 0.05;
    window_frame(f, frame, *t, vec![egui::Event::PointerMoved(a)]);
    *t += 0.05;
    window_frame(f, frame, *t, vec![press(a, true)]);
    let steps = (hold / 0.04).ceil().max(1.0) as usize;
    for i in 1..=steps {
        *t += 0.04;
        window_frame(f, frame, *t, vec![egui::Event::PointerMoved(a + (b - a) * (i as f32 / steps as f32))]);
    }
    window_frame(f, frame, *t, vec![press(b, false)]);
    *t += 0.05;
    window_frame(f, frame, *t, vec![]);
}

/// Heal, clone and the Magic Brush are painted like brushes, and they
/// respond to a press even when the mouse moves a little or the button is held (which egui
/// does not count as a click).
#[test]
fn heal_clone_and_magic_brush_are_painted() {
    use crate::layers::LocalTool;
    use sk_core::params::SpotMode;
    let Some(mut f) = Fixture::new("painted_tools") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    let mut frame = eframe::Frame::_new_kittest();
    let mut t = 10.0;
    f.app.splash = None;
    f.app.tab = crate::app::Tab::Layers;
    window_frame(&mut f, &mut frame, t, vec![]);
    let spots = |f: &Fixture| f.app.current.as_ref().unwrap().adjustments.retouch.clone();

    // A painted heal stroke becomes one area along the stroke, with a source elsewhere.
    f.app.set_local_tool(LocalTool::Heal);
    window_stroke(&mut f, &mut frame, &mut t, egui::pos2(700.0, 250.0), egui::pos2(850.0, 300.0), 0.4);
    let s = spots(&f);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].mode, SpotMode::Heal);
    assert!(s[0].path.len() >= 3, "Pfad: {:?}", s[0].path);
    assert_ne!(s[0].source, s[0].target);
    assert_eq!(f.app.selected_spot, Some(0));

    // A held click with a little jitter still makes a (round) clone spot.
    f.app.set_local_tool(LocalTool::Clone);
    window_stroke(&mut f, &mut frame, &mut t, egui::pos2(1000.0, 500.0), egui::pos2(1002.0, 501.0), 1.0);
    let s = spots(&f);
    assert_eq!(s.len(), 2);
    assert_eq!(s[1].mode, SpotMode::Clone);

    // Dragging the first area's pin moves the whole area (path and source alike).
    f.app.selected_spot = Some(0);
    let before = spots(&f)[0].clone();
    let pin = before.target;
    let start = egui::pos2(700.0, 250.0);
    window_stroke(&mut f, &mut frame, &mut t, start, start + egui::vec2(60.0, 40.0), 0.3);
    let after = spots(&f)[0].clone();
    assert_eq!(spots(&f).len(), 2, "Pin ziehen malt keinen neuen Bereich");
    let d = [after.target[0] - pin[0], after.target[1] - pin[1]];
    assert!(d[0] > 0.0 && d[1] > 0.0, "verschoben: {d:?}");
    let moved = |a: [f32; 2], b: [f32; 2]| ((a[0] - b[0] - d[0]).abs() < 1e-4) && ((a[1] - b[1] - d[1]).abs() < 1e-4);
    assert!(moved(after.source, before.source) && moved(after.path[0], before.path[0]));

    // Magic Brush: a painted stroke selects on the active layer.
    f.app.set_local_tool(LocalTool::Magic);
    window_stroke(&mut f, &mut frame, &mut t, egui::pos2(600.0, 650.0), egui::pos2(680.0, 660.0), 0.9);
    assert!(f.app.ai_busy, "Zauberpinsel sollte die Auswahl anstoßen");
    f.pump("Zauberpinsel", |a| !a.ai_busy);
    let layers = &f.app.current.as_ref().unwrap().adjustments.layers;
    assert!(layers.iter().any(|l| l.mask.ai.is_some()));
}


/// Object selection (with the downloaded MobileSAM models): a click selects the object, an
/// Alt+click takes part of it away, and "next object" keeps the result while starting anew.
#[test]
#[ignore = "langsam: läuft mit cargo t-all"]
fn object_clicks_select_and_refine() {
    if !sk_ai::segment::available() {
        return;
    }
    let Some(mut f) = Fixture::new("object") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    viewer_frame(&mut f, vec![]);
    let c = egui::pos2(600.0, 400.0);
    f.app.set_local_tool(crate::layers::LocalTool::Object);
    let mask = |f: &Fixture| f.app.current.as_ref().unwrap().adjustments.layers.first().and_then(|l| l.mask.ai.clone());
    let covered = |m: &sk_core::params::AiMask| m.data.iter().filter(|v| **v > 128).count();

    click(&mut f, c);
    f.pump("Objekt", |a| !a.ai_busy && a.current.as_ref().is_some_and(|c| c.adjustments.layers.first().is_some_and(|l| l.mask.ai.is_some())));
    let first = mask(&f).unwrap();
    let n = covered(&first);
    assert!(n > 0 && n < first.data.len(), "{n} von {}", first.data.len());
    assert_eq!(f.app.object_session.as_ref().map(|s| s.clicks.len()), Some(1));
    // What the app does each frame once the mouse is up: one undo step per click.
    f.app.commit_and_save();

    // Alt+click on the same spot: the object is computed anew from both clicks and replaces the
    // first result instead of adding to it.
    let alt = egui::Modifiers::ALT;
    viewer_frame_with(&mut f, vec![egui::Event::PointerMoved(c)], alt);
    viewer_frame_with(&mut f, vec![egui::Event::PointerButton { pos: c, button: egui::PointerButton::Primary, pressed: true, modifiers: alt }], alt);
    viewer_frame_with(&mut f, vec![egui::Event::PointerButton { pos: c, button: egui::PointerButton::Primary, pressed: false, modifiers: alt }], alt);
    viewer_frame_with(&mut f, vec![], egui::Modifiers::NONE);
    assert_eq!(f.app.object_session.as_ref().map(|s| s.clicks.iter().filter(|c| !c.include).count()), Some(1));
    f.pump("Objekt verfeinert", |a| !a.ai_busy);
    let second = mask(&f).unwrap();
    assert!(covered(&second) < n, "Alt+Klick nimmt etwas weg: {} vs {n}", covered(&second));

    // Undo brings back the first result.
    f.app.commit_and_save();
    f.app.undo();
    assert_eq!(mask(&f).map(|m| covered(&m)), Some(n));
}

/// Lens blur: switching it on computes the image's depth map in the background, the focus
/// pipette takes the depth under the click, and "Tiefe auswählen" gives a layer an adjustable
/// depth mask.
#[test]
#[ignore = "langsam: läuft mit cargo t-all"]
fn lens_blur_and_depth_mask() {
    if !sk_ai::depth::available() {
        return;
    }
    let Some(mut f) = Fixture::new("depth") else { return };
    // Not in the user's cache (the test's files are new each run).
    unsafe { std::env::set_var("SILBERKORN_DEPTH_CACHE", f.dir.join("depth")) };
    f.pump("Bild geladen", |a| a.texture.is_some());
    let path = f.app.current_path().unwrap();
    f.app.edit(|a| a.lens_blur.amount = 60.0);
    f.pump("Tiefenkarte", |a| a.depth.running.is_none() && sk_core::depth::is_ready(&path));
    assert!(f.app.status.contains("fertig"), "{}", f.app.status);

    // Focus by click: on the tree in the middle-right, then on the wall at the left edge
    // (much nearer).
    viewer_frame(&mut f, vec![]);
    let at = |f: &mut Fixture, p: egui::Pos2| {
        f.app.toggle_focus_picker();
        assert!(f.app.picking_focus);
        click(f, p);
        assert!(!f.app.picking_focus);
        f.app.current.as_ref().unwrap().adjustments.lens_blur.focus
    };
    let tree = at(&mut f, egui::pos2(760.0, 300.0));
    let wall = at(&mut f, egui::pos2(30.0, 400.0));
    assert!(wall > tree + 0.2, "Mauer {wall}, Baum {tree}");

    // A depth mask on a new layer, then a narrower range.
    f.app.edit(|a| a.layers.push(Default::default()));
    f.app.active_layer = Some(0);
    f.app.set_depth_mask(0, None);
    let mask = |f: &Fixture| f.app.current.as_ref().unwrap().adjustments.layers[0].mask.ai.clone().unwrap();
    let m = mask(&f);
    let range = m.depth.expect("Tiefenbereich gespeichert");
    let covered = |m: &sk_core::params::AiMask| m.data.iter().filter(|v| **v > 128).count();
    let n = covered(&m);
    assert!(n > m.data.len() / 20 && n < m.data.len() / 2, "{n} von {}", m.data.len());
    f.app.set_depth_mask(0, Some(sk_core::params::DepthRange { far: range.far + 0.1, ..range }));
    assert!(covered(&mask(&f)) < n);
}

/// "Staub und Flecken finden" runs in the background and adds heal spots once; searching
/// again does not add the same spots twice.
#[test]
fn spot_search_adds_heal_spots_once() {
    let Some(mut f) = Fixture::new("spots") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    f.app.spot_sensitivity = 1.0;
    f.app.find_spots();
    assert!(f.app.spot_search.is_some());
    f.pump("Flecken", |a| a.spot_search.is_none());
    let spots = f.app.current.as_ref().unwrap().adjustments.retouch.clone();
    assert!(!spots.is_empty(), "{}", f.app.status);
    assert!(spots.iter().all(|s| s.mode == sk_core::params::SpotMode::Heal));
    assert_eq!(f.app.local_tool, Some(crate::layers::LocalTool::Heal), "Stellen zum Prüfen sichtbar");
    f.app.find_spots();
    f.pump("Flecken", |a| a.spot_search.is_none());
    assert_eq!(f.app.current.as_ref().unwrap().adjustments.retouch.len(), spots.len());
}

#[test]
fn wheel_zooms_while_cropping_and_space_pans() {
    let Some(mut f) = Fixture::new("spacepan") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    f.app.start_crop_tool();
    let c = egui::pos2(600.0, 400.0);
    viewer_frame(&mut f, vec![egui::Event::PointerMoved(c)]);
    let wheel = egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, 120.0),
        phase: egui::TouchPhase::Move,
        modifiers: Default::default(),
    };
    viewer_frame(&mut f, vec![wheel]);
    for _ in 0..30 {
        viewer_frame(&mut f, vec![]);
    }
    assert!(f.app.zoom.is_some(), "Mausrad zoomt auch beim Zuschneiden");

    // Space held: dragging moves the image instead of the crop frame.
    let geometry = f.app.current.as_ref().unwrap().adjustments.geometry.clone();
    let pan = f.app.pan;
    let space = |pressed| egui::Event::Key { key: egui::Key::Space, physical_key: None, pressed, repeat: false, modifiers: Default::default() };
    viewer_frame(&mut f, vec![space(true)]);
    drag(&mut f, c, c + egui::vec2(-150.0, 60.0), 6);
    viewer_frame(&mut f, vec![space(false)]);
    assert!((f.app.pan - pan - egui::vec2(-150.0, 60.0)).length() < 2.0, "verschoben: {:?} → {:?}", pan, f.app.pan);
    assert_eq!(f.app.current.as_ref().unwrap().adjustments.geometry, geometry, "Zuschnitt unverändert");
    assert!(f.app.crop_tool.is_some());
}

/// Cursor of a few frames with the pointer at `p` over `ui`.
fn cursor_over(p: egui::Pos2, add: impl Fn(&mut egui::Ui)) -> egui::CursorIcon {
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    let mut cursor = egui::CursorIcon::Default;
    for _ in 0..3 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))),
            events: vec![egui::Event::PointerMoved(p)],
            ..Default::default()
        };
        let out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::no_frame().show(ui, |ui| add(ui));
            crate::app::hover_cursor(ui.ctx());
        });
        cursor = out.platform_output.cursor_icon;
        let mut delta = out.textures_delta;
        delta.clear();
    }
    cursor
}

#[test]
fn cursors_match_what_can_be_done() {
    // Rating stars (clickable labels): the hand.
    let stars = |ui: &mut egui::Ui| {
        let mut r = 2;
        crate::widgets::stars(ui, &mut r, 18.0);
    };
    assert_eq!(cursor_over(egui::pos2(8.0, 8.0), stars), egui::CursorIcon::PointingHand);
    // Plain text: the arrow.
    assert_eq!(cursor_over(egui::pos2(8.0, 8.0), |ui| { ui.label("Text"); }), egui::CursorIcon::Default);
    // A slider rail: drag sideways.
    let slider = |ui: &mut egui::Ui| {
        let mut v = 0.0;
        crate::widgets::compact_slider(ui, "x", &mut v, 0.0..=1.0, 0.0, 200.0);
    };
    assert_eq!(cursor_over(egui::pos2(100.0, 10.0), slider), egui::CursorIcon::ResizeHorizontal);
}

#[test]
fn cancelled_model_download_is_not_restarted_by_itself() {
    use crate::ai_models::AiFeature;
    if AiFeature::Depth.available() {
        return;
    }
    let Some(mut f) = Fixture::new("depth-cancel") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    // Lens blur is on and its model was being downloaded when the user cancelled.
    f.app.edit(|a| a.lens_blur.amount = 60.0);
    assert!(f.app.ai_download_done(0, &AiFeature::Depth.job_label(), &Err("abgebrochen".into())));
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(500) {
        f.app.handle_events(&f.ctx);
        assert!(f.app.ai_download_progress(AiFeature::Depth).is_none(), "Download neu gestartet");
        assert!(f.app.depth.running.is_none());
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(f.app.status.contains("abgebrochen"), "{}", f.app.status);
}

#[test]
fn running_jobs_can_be_cancelled() {
    let Some(mut f) = Fixture::new("cancel") else { return };
    let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done = finished.clone();
    f.app.start_job(
        "Langer Auftrag",
        Box::new(move |progress| {
            for i in 0..10_000 {
                progress(i as f32 / 10_000.0);
                std::thread::sleep(Duration::from_millis(5));
            }
            done.store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(Vec::new())
        }),
    );
    let job = f.app.activities().into_iter().find(|a| a.label == "Langer Auftrag").unwrap();
    job.cancel.unwrap().store(true, std::sync::atomic::Ordering::Relaxed);
    f.pump("Abbruch", |a| a.jobs.is_empty());
    assert!(f.app.status.contains("abgebrochen"), "{}", f.app.status);
    assert!(!finished.load(std::sync::atomic::Ordering::Relaxed));
}

#[test]
fn released_drag_shows_screen_resolution_before_full_resolution() {
    // Needs the full 24 megapixels: fitted on a high-density screen, the 2048 px preview is
    // magnified and the full resolution has more than twice the screen's pixels. The GPU takes
    // the whole view as one texture.
    let Some(mut f) = Fixture::full_size("released") else { return };
    let step = |f: &mut Fixture, events: Vec<egui::Event>| {
        f.app.pixels_per_point = 2.0;
        let screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0)));
        run_viewer(f, egui::RawInput { screen_rect, max_texture_side: Some(8192), events, ..Default::default() });
        f.app.handle_events(&f.ctx.clone());
        f.app.send_render();
        std::thread::sleep(Duration::from_millis(20));
    };
    let start = Instant::now();
    while !f.app.tiles.view.as_ref().is_some_and(|v| v.level == 1 && v.version == f.app.edit_version) {
        assert!(start.elapsed() < LIMIT, "Zeitüberschreitung: scharfe Ansicht");
        step(&mut f, vec![]);
    }

    // First drag without a measured speed: a coarse view draft.
    f.app.workers.draft_speed = Default::default();
    let p = egui::pos2(600.0, 400.0);
    step(&mut f, vec![egui::Event::PointerMoved(p)]);
    step(&mut f, vec![press(p, true)]);
    f.app.edit(|adj| adj.exposure = 0.4);
    while !f.app.tiles.view.as_ref().is_some_and(|v| v.version == f.app.edit_version) {
        assert!(start.elapsed() < LIMIT, "Zeitüberschreitung: Entwurf");
        step(&mut f, vec![]);
    }
    assert!(f.app.tiles.view.as_ref().unwrap().level > 2);

    // Released: the coarse draft doesn't stay until the full resolution is done.
    step(&mut f, vec![press(p, false)]);
    let mut levels = vec![];
    while levels.last() != Some(&1) {
        assert!(start.elapsed() < LIMIT, "Zeitüberschreitung: nach dem Loslassen");
        step(&mut f, vec![]);
        let level = f.app.tiles.view.as_ref().unwrap().level;
        if levels.last() != Some(&level) {
            levels.push(level);
        }
    }
    assert!(levels.contains(&2), "Stufen: {levels:?}");
}

/// A GPU whose textures are smaller than the view at full resolution: the tiles show it sharp
/// (one texture of the whole view would be too large for the GPU).
#[test]
fn view_larger_than_the_gpu_texture_limit_is_shown_by_tiles() {
    let Some(mut f) = Fixture::new("texture-limit") else { return };
    let start = Instant::now();
    loop {
        assert!(start.elapsed() < LIMIT, "Zeitüberschreitung: scharfe Ansicht");
        f.app.pixels_per_point = 2.0;
        // egui's default limit: 2048, smaller than the full resolution.
        viewer_frame(&mut f, vec![]);
        f.app.handle_events(&f.ctx.clone());
        f.app.send_render();
        let max = f.app.max_texture_side;
        assert!(f.app.tiles.view.as_ref().is_none_or(|v| v.texture.size().iter().all(|&s| s <= max)));
        let (fw, fh) = f.app.tiles.full_size;
        let all = fw.div_ceil(crate::workers::TILE) * fh.div_ceil(crate::workers::TILE);
        if f.app.tiles.version == f.app.edit_version && fw > max && f.app.tiles.textures.len() == all {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Undo brings a deleted layer back: the active layer stays the same layer, not the same index.
#[test]
fn active_layer_follows_undo_and_redo() {
    let Some(mut f) = Fixture::new("layer-undo") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    let named = |n: &str| sk_core::params::Layer { name: n.into(), ..Default::default() };
    f.app.edit(|a| a.layers = vec![named("A"), named("B"), named("C")]);
    f.app.commit_and_save();
    f.app.active_layer = Some(2);
    // Delete A, as the layer list does.
    f.app.edit(|a| {
        a.layers.remove(0);
    });
    f.app.active_layer = Some(1);
    f.app.commit_and_save();
    let active = |f: &Fixture| f.app.active_layer.map(|i| f.app.current.as_ref().unwrap().adjustments.layers[i].name.clone());
    f.app.undo();
    assert_eq!(active(&f).as_deref(), Some("C"));
    f.app.redo();
    assert_eq!(active(&f).as_deref(), Some("C"));
    // Undoing the creation of the active layer leaves no active layer.
    f.app.undo();
    f.app.undo();
    assert_eq!(f.app.active_layer, None);
}

/// A content-based mask that arrives after a layer before it was deleted goes to its layer.
#[test]
fn ai_mask_lands_on_its_layer_after_deleting_another() {
    let Some(mut f) = Fixture::new("ai-layer") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    let named = |n: &str| sk_core::params::Layer { name: n.into(), ..Default::default() };
    f.app.edit(|a| a.layers = vec![named("A"), named("B"), named("C")]);
    f.app.commit_and_save();
    let target = f.app.layer_id(2).unwrap();
    f.app.edit(|a| {
        a.layers.remove(0);
    });
    let mask = sk_ai::MaskImage { width: 4, height: 4, data: vec![255; 16] };
    f.app.workers.inject(crate::workers::Event::AiMask { generation: f.app.generation(), layer: target, result: Ok(mask) });
    f.app.handle_events(&f.ctx.clone());
    let layers = &f.app.current.as_ref().unwrap().adjustments.layers;
    assert!(layers[0].mask.ai.is_none(), "B unverändert");
    assert!(layers[1].mask.ai.is_some(), "C hat die Maske");
}

/// Only a limited number of thumbnails stays in memory; those not drawn for the longest go first.
#[test]
fn thumbnail_textures_are_limited() {
    let Some(mut f) = Fixture::new("thumb-limit") else { return };
    let img = || egui::ColorImage::new([2, 2], vec![egui::Color32::WHITE; 4]);
    let path = |i: usize| PathBuf::from(format!("/t/{i}.ARW"));
    // Not asked for by the browser (background previews): not kept.
    f.app.set_thumb(&f.ctx.clone(), path(0), img());
    assert!(f.app.thumbs.is_empty());
    for i in 0..=MAX_THUMBS {
        f.app.thumb_requested.insert(path(i));
        f.app.thumb_seen.insert(path(i), if i == 0 { 1_000_000 } else { i as u64 });
        f.app.set_thumb(&f.ctx.clone(), path(i), img());
    }
    assert!(f.app.thumbs.len() <= MAX_THUMBS);
    assert!(f.app.thumbs.contains_key(&path(0)), "zuletzt gezeigt bleibt");
    assert!(!f.app.thumbs.contains_key(&path(1)), "am längsten nicht gezeigt geht");
    assert!(!f.app.thumb_requested.contains(&path(1)), "wird bei Bedarf neu angefordert");
}
