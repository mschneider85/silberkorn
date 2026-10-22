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
                        color: &meta.color,
                        as_shot_wb: meta.as_shot_wb,
                        info: &info,
                        baseline_exposure: 0.0,
                        preview: Some(&preview),
                        lens_corrected: false,
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
    /// Last: removes the folder only after the app (and its catalog) is gone.
    _cleanup: Cleanup,
}

struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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
        let mut f = Self { _cleanup: Cleanup(dir.clone()), dir, ctx, app };
        f.pump("Ordner geladen", |a| !a.folder_loading);
        Some(f)
    }

    /// Opens a folder and waits until it is read (see `App::open_scope`).
    fn open_folder(&mut self, folder: PathBuf) {
        self.app.open_folder(folder);
        self.pump("Ordner geladen", |a| !a.folder_loading);
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

    /// Takes the photos into the catalog where they are (browsing does not).
    fn index_photos(&mut self) {
        let paths = sk_core::catalog::scan_folder(&self.dir.join("photos"), false);
        self.app.workers.index(crate::workers::IndexJob { catalog: self.app.catalog_path(), paths, import: false, copy_to: None });
    }

    fn photo(&self, name: &str) -> PathBuf {
        self.dir.join("photos").join(name).canonicalize().unwrap()
    }
}


#[test]
fn library_workflow() {
    let Some(mut f) = Fixture::new("library") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    f.index_photos();
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

/// Raw-Details: disabled for images without a Bayer mosaic (the samples are linear DNGs); on,
/// the viewer's image is computed in the background, and what goes wrong is shown at the slider.
#[test]
fn raw_details_needs_a_bayer_mosaic() {
    use crate::ai_products::{AiProduct, ProductState};
    let Some(mut f) = Fixture::new("raw-details") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    f.app.tab = crate::app::Tab::Details;
    let amount = |f: &Fixture| f.app.current.as_ref().unwrap().adjustments.detail.raw_details;
    // (The samples count as HDR, which is not available either.)
    std::sync::Arc::make_mut(&mut f.app.current.as_mut().unwrap().meta).hdr = false;
    let meta = f.app.current.as_ref().unwrap().meta.clone();
    let d = sk_core::params::Detail::default();
    assert!(AiProduct::Details.unavailable(&meta, &d).is_some_and(|why| why.starts_with("Nur für Bayer-Sensoren")));
    let nodes = tool_panel_frame(&mut f, vec![]);
    click_slider(&mut f, slider(&nodes, "Raw-Details"));
    assert_eq!(amount(&f), 0.0, "gesperrt");

    // As if it were a raw file with a mosaic: the slider works and the computation starts (and
    // fails for this file, which has none).
    std::sync::Arc::make_mut(&mut f.app.current.as_mut().unwrap().meta).bayer = true;
    let meta = f.app.current.as_ref().unwrap().meta.clone();
    assert!(AiProduct::Details.unavailable(&meta, &d).is_none());
    // Under full KI-Entrauschen it does nothing: disabled.
    let full = sk_core::params::Detail { ai_denoise: 100.0, raw_details: 80.0, ..d.clone() };
    assert!(AiProduct::Details.unavailable(&meta, &full).is_some_and(|why| why.starts_with("Bei 100 %")));
    assert_eq!(full.effective_raw_details(), 0.0);
    let nodes = tool_panel_frame(&mut f, vec![]);
    click_slider(&mut f, slider(&nodes, "Raw-Details"));
    assert!(amount(&f) > 50.0, "{}", amount(&f));
    let path = f.app.current_path().unwrap();
    f.pump("Raw-Details versucht", |a| matches!(a.product_state(AiProduct::Details, &path), ProductState::Failed(_)));
    let ProductState::Failed(e) = f.app.product_state(AiProduct::Details, &path) else { unreachable!() };
    assert!(e.contains("nur für Bayer-Sensoren"), "{e}");
}

/// The white balance pipette previews the balance of the spot under it; a click sets it.
#[test]
fn white_balance_pipette_previews_before_the_click() {
    use sk_core::params::WbMode;
    let Some(mut f) = Fixture::new("wb-hover") else { return };
    f.pump("Bild gerendert", |a| a.current.is_some() && a.texture.is_some());
    f.app.toggle_wb_picker();
    // Somewhere neither too dark nor clipped.
    let spots = [(600.0, 420.0), (400.0, 300.0), (800.0, 500.0), (300.0, 600.0)].map(|(x, y)| egui::pos2(x, y));
    let at = spots.into_iter().find(|p| {
        viewer_frame(&mut f, vec![egui::Event::PointerMoved(*p)]);
        viewer_frame(&mut f, vec![]);
        f.app.style_preview.is_some()
    });
    let at = at.expect("eine brauchbare Stelle");
    let preview = f.app.style_preview.as_ref().unwrap().white_balance.clone();
    assert_eq!(preview.mode, WbMode::Custom);
    assert_eq!(f.app.current.as_ref().unwrap().adjustments.white_balance.mode, WbMode::AsShot, "nur Vorschau");
    viewer_frame(&mut f, vec![press(at, true)]);
    viewer_frame(&mut f, vec![press(at, false)]);
    let wb = &f.app.current.as_ref().unwrap().adjustments.white_balance;
    assert_eq!((wb.mode, wb.kelvin.round()), (WbMode::Custom, preview.kelvin.round()));
    assert!(!f.app.picking_wb);
}

/// I shows the picture's information, short, then with everything, then not.
#[test]
fn info_overlay_cycles_with_i() {
    use crate::app::InfoOverlay;
    let Some(mut f) = Fixture::new("info-overlay") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    assert_eq!(f.app.info_lines(), None);
    f.app.info_overlay = InfoOverlay::Short;
    let short = f.app.info_lines().unwrap();
    assert!(short[0].starts_with("_DSC00"), "{short:?}");
    f.app.info_overlay = InfoOverlay::Full;
    let full = f.app.info_lines().unwrap();
    assert!(full.len() > short.len() && full.iter().any(|l| l.contains("px")), "{full:?}");
    assert_eq!(crate::browser::exif_date("2024:05:17 14:03:22"), "17.05.2024 14:03");
    assert_eq!(crate::browser::exif_date("irgendwann"), "irgendwann");
}

/// Zielkorrektur: dragging up in the image raises the curve at the tone there, or the
/// saturation of the color there; the image itself is not moved.
#[test]
fn target_tool_changes_what_is_under_the_mouse() {
    use crate::develop_tools::ColorView;
    use crate::target_tool::TargetTool;
    let Some(mut f) = Fixture::new("target-tool") else { return };
    f.pump("Bild gerendert", |a| a.current.is_some() && a.texture.is_some());
    let adj = |f: &Fixture| f.app.current.as_ref().unwrap().adjustments.clone();
    f.app.toggle_target_tool(TargetTool::Curve);
    viewer_frame(&mut f, vec![]);
    drag(&mut f, egui::pos2(600.0, 420.0), egui::pos2(600.0, 360.0), 6);
    let curve = adj(&f).curves.rgb;
    assert_eq!(curve.points.len(), 3, "{:?}", curve.points);
    let p = curve.points[2];
    assert!(p[1] > p[0] + 0.1, "angehoben: {p:?}");
    assert_eq!(f.app.pan, egui::Vec2::ZERO, "Bild nicht verschoben");

    f.app.toggle_target_tool(TargetTool::Colors(ColorView::Saturation));
    assert_eq!(f.app.target_tool, Some(TargetTool::Colors(ColorView::Saturation)));
    // Somewhere with color (not every spot of the samples has some).
    let spot = [(600.0, 420.0), (400.0, 300.0), (800.0, 500.0), (300.0, 600.0)].into_iter().map(|(x, y)| egui::pos2(x, y));
    let mut changed = false;
    for at in spot {
        drag(&mut f, at, at - egui::vec2(0.0, 40.0), 4);
        if adj(&f).color_editor.bands.iter().any(|b| b.saturation > 0.0) {
            changed = true;
            break;
        }
    }
    assert!(changed, "{:?}", adj(&f).color_editor.bands);
    assert!(adj(&f).color_editor.bands.iter().all(|b| b.saturation >= 0.0 && b.hue == 0.0 && b.lightness == 0.0));
}

/// Snapshots are shown like the styles: a preview of the image as each was saved; a click
/// applies one.
#[test]
fn snapshots_show_as_previews() {
    let Some(mut f) = Fixture::new("snapshot-grid") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    f.app.add_snapshot(Some("Hell".into()));
    f.app.edit(|a| a.exposure = 1.5);
    f.app.commit_and_save();
    f.app.add_snapshot(Some("Heller".into()));
    f.app.edit(|a| a.exposure = 0.0);
    f.app.commit_and_save();
    f.app.tab = crate::app::Tab::History;
    tool_panel_frame(&mut f, vec![]);
    let shots = f.app.current.as_ref().unwrap().snapshots.clone();
    f.pump("Vorschauen", |a| shots.iter().enumerate().all(|(i, s)| a.snapshot_texture(i, &s.adjustments).is_some()));
    let nodes = tool_panel_frame(&mut f, vec![]);
    let tile = widget(&nodes, "Heller").expect("Kachel „Heller“");
    click_at(&mut f, tile.center());
    assert_eq!(f.app.current.as_ref().unwrap().adjustments.exposure, 1.5);
}

/// Hovering a step of the history shows it in the image without going there.
#[test]
fn hovering_a_history_step_previews_it() {
    let Some(mut f) = Fixture::new("history-hover") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    f.app.edit(|a| a.exposure = 1.0);
    f.app.commit_and_save();
    f.app.tab = crate::app::Tab::History;
    let nodes = tool_panel_frame(&mut f, vec![]);
    let (_, row) = nodes.iter().find(|(l, _)| l == "Geöffnet").expect("Schritt „Geöffnet“");
    let at = row.center();
    tool_panel_frame(&mut f, vec![egui::Event::PointerMoved(at)]);
    tool_panel_frame(&mut f, vec![]);
    assert_eq!(f.app.style_preview.as_ref().map(|a| a.exposure), Some(0.0));
    assert_eq!(f.app.current.as_ref().unwrap().adjustments.exposure, 1.0, "nur Vorschau");
}

/// The color under the mouse is noted for the histogram; a slider of a tonal range marks it
/// there, and dragging in the histogram moves that slider.
#[test]
fn histogram_shows_the_color_under_the_mouse_and_moves_tone_sliders() {
    use crate::widgets::ToneZone;
    let Some(mut f) = Fixture::new("histogram") else { return };
    f.pump("Bild gerendert", |a| a.current.is_some() && a.texture.is_some());
    // (egui knows what is under the mouse from the frame before.)
    viewer_frame(&mut f, vec![egui::Event::PointerMoved(egui::pos2(600.0, 400.0))]);
    viewer_frame(&mut f, vec![]);
    let rgb = f.app.hover_rgb.expect("Farbe unter der Maus");
    assert!(rgb.iter().any(|v| *v > 0), "{rgb:?}");
    viewer_frame(&mut f, vec![egui::Event::PointerGone]);
    viewer_frame(&mut f, vec![]);
    assert_eq!(f.app.hover_rgb, None);

    f.app.tab = crate::app::Tab::Exposure;
    let nodes = tool_panel_frame(&mut f, vec![]);
    let rail = slider(&nodes, "Lichter");
    tool_panel_frame(&mut f, vec![egui::Event::PointerMoved(rail.center())]);
    tool_panel_frame(&mut f, vec![]);
    assert_eq!(f.app.hover_zone, Some(ToneZone::Highlights));

    let before = f.app.current.as_ref().unwrap().adjustments.exposure;
    f.app.move_tone_zone(ToneZone::Exposure, 0.25);
    assert!((f.app.current.as_ref().unwrap().adjustments.exposure - before - 1.0).abs() < 1e-4);
    f.app.move_tone_zone(ToneZone::Whites, -0.1);
    assert_eq!(f.app.current.as_ref().unwrap().adjustments.whites, -10.0);
}

/// Holding a slider with Alt shows what it does instead of the image, as long as it is held.
#[test]
fn sliders_show_their_effect_with_alt() {
    use sk_core::pipeline::SliderView;
    let Some(mut f) = Fixture::new("sharpen-view") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    f.app.tab = crate::app::Tab::Details;
    let alt = egui::Modifiers::ALT;
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: alt };
    for (label, view) in [("Stärke", SliderView::Grey), ("Radius", SliderView::Sharpening), ("Schwelle", SliderView::SharpenMask), ("Details", SliderView::Sharpening)] {
        let nodes = tool_panel_frame(&mut f, vec![]);
        let rail = slider(&nodes, label);
        let at = egui::pos2(rail.left() + 7.0 + (rail.width() - 14.0) * 0.3, rail.center().y);
        tool_panel_frame_with(&mut f, vec![egui::Event::PointerMoved(at)], alt);
        tool_panel_frame_with(&mut f, vec![button(at, true)], alt);
        tool_panel_frame_with(&mut f, vec![egui::Event::PointerMoved(at + egui::vec2(3.0, 0.0))], alt);
        assert_eq!(f.app.slider_view, Some(view), "{label}");
        tool_panel_frame_with(&mut f, vec![button(at, false)], alt);
        tool_panel_frame(&mut f, vec![]);
        f.app.handle_events(&f.ctx);
        assert_eq!(f.app.slider_view, None, "{label} losgelassen");
    }
    // Belichtung, Lichter & Tiefen: where it clips; Rauschreduzierung: what it removes.
    f.app.tab = crate::app::Tab::Exposure;
    for (label, view) in [("Belichtung", SliderView::ClipHighlights), ("Weiß", SliderView::ClipHighlights), ("Tiefen", SliderView::ClipShadows), ("Schwarz", SliderView::ClipShadows)] {
        let nodes = tool_panel_frame(&mut f, vec![]);
        let rail = slider(&nodes, label);
        let at = egui::pos2(rail.left() + 7.0 + (rail.width() - 14.0) * 0.5, rail.center().y);
        tool_panel_frame_with(&mut f, vec![egui::Event::PointerMoved(at)], alt);
        tool_panel_frame_with(&mut f, vec![button(at, true)], alt);
        tool_panel_frame_with(&mut f, vec![egui::Event::PointerMoved(at + egui::vec2(2.0, 0.0))], alt);
        assert_eq!(f.app.slider_view, Some(view), "{label}");
        tool_panel_frame_with(&mut f, vec![button(at, false)], alt);
        tool_panel_frame(&mut f, vec![]);
        f.app.handle_events(&f.ctx);
        assert_eq!(f.app.slider_view, None, "{label} losgelassen");
    }
    f.app.tab = crate::app::Tab::Details;
    let nodes = tool_panel_frame(&mut f, vec![]);
    let rail = slider(&nodes, "Luminanz");
    let at = egui::pos2(rail.left() + 20.0, rail.center().y);
    tool_panel_frame_with(&mut f, vec![egui::Event::PointerMoved(at)], alt);
    tool_panel_frame_with(&mut f, vec![button(at, true)], alt);
    assert_eq!(f.app.slider_view, Some(SliderView::NoiseRemoved));
    tool_panel_frame_with(&mut f, vec![button(at, false)], alt);
    tool_panel_frame(&mut f, vec![]);
    f.app.handle_events(&f.ctx);

    // Strg does not do it (one key per function).
    let nodes = tool_panel_frame(&mut f, vec![]);
    let rail = slider(&nodes, "Stärke");
    let at = egui::pos2(rail.left() + 20.0, rail.center().y);
    let strg = egui::Modifiers::COMMAND;
    tool_panel_frame_with(&mut f, vec![egui::Event::PointerMoved(at)], strg);
    tool_panel_frame_with(&mut f, vec![egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: strg }], strg);
    assert_eq!(f.app.slider_view, None, "nicht mit Strg");
    tool_panel_frame_with(&mut f, vec![egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: strg }], strg);
    tool_panel_frame(&mut f, vec![]);
    f.app.handle_events(&f.ctx);
    // Without Alt: no grey view.
    let nodes = tool_panel_frame(&mut f, vec![]);
    let rail = slider(&nodes, "Stärke");
    let at = egui::pos2(rail.left() + 20.0, rail.center().y);
    tool_panel_frame(&mut f, vec![egui::Event::PointerMoved(at)]);
    tool_panel_frame(&mut f, vec![egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }]);
    assert_eq!(f.app.slider_view, None);
}

#[test]
fn enhance_settings_carry_over_to_the_selection() {
    let Some(mut f) = Fixture::new("enhance") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    let other = f.app.entries.iter().map(|e| e.path.clone()).find(|p| Some(p) != f.app.current_path().as_ref()).unwrap();
    f.app.selection = f.app.entries.iter().map(|e| e.path.clone()).collect();
    f.app.transfer_enhance((60.0, 40.0));
    let d = sidecar::load(&other).adjustments.detail;
    assert_eq!((d.raw_details, d.ai_denoise), (60.0, 40.0));
    let d = &f.app.current.as_ref().unwrap().adjustments.detail;
    assert_eq!((d.raw_details, d.ai_denoise), (60.0, 40.0));
    assert!(f.app.can_undo());
}

#[test]
fn copy_some_tools_and_transfer_one() {
    let Some(mut f) = Fixture::new("copytools") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    let other = f.app.entries.iter().map(|e| e.path.clone()).find(|p| Some(p) != f.app.current_path().as_ref()).unwrap();
    f.app.set_adjustments_on(std::slice::from_ref(&other), |a| sk_core::params::Adjustments { exposure: -0.5, contrast: 10.0, rotation: 1, ..a.clone() });

    // Copying takes the edited tools only; the other image keeps its exposure and rotation.
    f.app.edit(|a| {
        a.contrast = 40.0;
        a.detail.grain_amount = 20.0;
        a.rotation = 2;
    });
    f.app.copy_adjustments();
    let tools = &f.app.clipboard.as_ref().unwrap().tools;
    assert_eq!(tools, &vec![sk_core::tools::Tool::Exposure, sk_core::tools::Tool::Grain]);
    f.app.selection = [other.clone()].into_iter().collect();
    f.app.clipboard.as_mut().unwrap().tools = vec![sk_core::tools::Tool::Grain];
    f.app.paste_adjustments();
    let a = sidecar::load(&other).adjustments;
    assert_eq!((a.exposure, a.contrast, a.detail.grain_amount, a.rotation), (-0.5, 10.0, 20.0, 1));

    // The button of a tool carries just that tool over to the other selected images.
    f.app.selection = f.app.entries.iter().map(|e| e.path.clone()).collect();
    f.app.transfer_tool(sk_core::tools::Tool::Exposure);
    let a = sidecar::load(&other).adjustments;
    assert_eq!((a.exposure, a.contrast, a.rotation), (0.0, 40.0, 1));
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
    let style = styles::Style { name: "Kontrast".into(), adjustments: sk_core::params::Adjustments { contrast: 30.0, ..Default::default() }, tools: None };
    styles::save_in(&style_dir, &style).unwrap();
    let loaded = styles::list_in(&style_dir);
    f.app.set_adjustments_on(std::slice::from_ref(&b), |adj| styles::apply(&loaded[0], adj));
    assert_eq!(sidecar::load(&b).adjustments.contrast, 30.0);

    // Export both images.
    f.app.recipes[0].output_dir = f.dir.join("out");
    f.app.recipes[0].size = Some(800);
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

/// [`drag`] with modifier keys held.
fn drag_with(f: &mut Fixture, a: egui::Pos2, b: egui::Pos2, steps: usize, modifiers: egui::Modifiers) {
    viewer_frame_with(f, vec![egui::Event::PointerMoved(a)], modifiers);
    viewer_frame_with(f, vec![press(a, true)], modifiers);
    for i in 1..=steps {
        let p = a + (b - a) * (i as f32 / steps as f32);
        viewer_frame_with(f, vec![egui::Event::PointerMoved(p)], modifiers);
    }
    viewer_frame_with(f, vec![press(b, false)], modifiers);
    viewer_frame_with(f, vec![], egui::Modifiers::NONE);
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

/// Dragging with the right mouse button changes the brush instead of painting: left/right
/// the size, up/down the hardness; in the remove tool it does not mark anything either.
#[test]
fn right_drag_changes_brush_size_and_hardness() {
    let Some(mut f) = Fixture::new("brush_adjust") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    viewer_frame(&mut f, vec![]);
    let c = egui::pos2(600.0, 400.0);
    let right = |p, pressed| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed, modifiers: Default::default() };
    let right_drag = |f: &mut Fixture, d: egui::Vec2| {
        viewer_frame(f, vec![egui::Event::PointerMoved(c)]);
        viewer_frame(f, vec![right(c, true)]);
        for i in 1..=6 {
            viewer_frame(f, vec![egui::Event::PointerMoved(c + d * (i as f32 / 6.0))]);
        }
        viewer_frame(f, vec![right(c + d, false)]);
        viewer_frame(f, vec![]);
    };

    f.app.set_local_tool(crate::layers::LocalTool::Brush);
    // Sideways (drifting a little up): only the size.
    let (size, hardness) = (f.app.brushes.paint.size, f.app.brushes.paint.hardness);
    right_drag(&mut f, egui::vec2(60.0, -15.0));
    assert!((f.app.brushes.paint.size - (size + 60.0)).abs() < 1.0, "{} -> {}", size, f.app.brushes.paint.size);
    assert_eq!(f.app.brushes.paint.hardness, hardness);
    // Up (drifting sideways): only the hardness.
    let size = f.app.brushes.paint.size;
    right_drag(&mut f, egui::vec2(-20.0, -40.0));
    assert!(f.app.brushes.paint.hardness > hardness + 0.1, "up is harder: {hardness} -> {}", f.app.brushes.paint.hardness);
    assert_eq!(f.app.brushes.paint.size, size);
    // Both in one drag: first sideways, then down.
    let (size, hardness) = (f.app.brushes.paint.size, f.app.brushes.paint.hardness);
    let path = [c, c + egui::vec2(15.0, 0.0), c + egui::vec2(30.0, 2.0), c + egui::vec2(31.0, 20.0), c + egui::vec2(29.0, 40.0)];
    viewer_frame(&mut f, vec![egui::Event::PointerMoved(c)]);
    viewer_frame(&mut f, vec![right(c, true)]);
    for p in &path[1..] {
        viewer_frame(&mut f, vec![egui::Event::PointerMoved(*p)]);
    }
    viewer_frame(&mut f, vec![right(path[4], false)]);
    viewer_frame(&mut f, vec![]);
    assert!(f.app.brushes.paint.size > size + 20.0 && f.app.brushes.paint.hardness < hardness - 0.15, "beides: {} {}", f.app.brushes.paint.size, f.app.brushes.paint.hardness);
    assert!(f.app.current.as_ref().unwrap().adjustments.layers.iter().all(|l| l.mask.strokes.is_empty()), "nothing painted");
    assert!(!f.app.adjusting_brush());

    f.app.set_local_tool(crate::layers::LocalTool::Remove);
    let size = f.app.brushes.remove.size;
    right_drag(&mut f, egui::vec2(-20.0, 0.0));
    assert!((f.app.brushes.remove.size - (size - 20.0)).abs() < 1.0);
    assert!(f.app.remove_strokes.is_empty() && !f.app.inpaint_busy);
}

/// With the brush, Alt held erases (with the eraser's settings).
#[test]
fn brush_erases_with_alt() {
    let Some(mut f) = Fixture::new("brush-keys") else { return };
    f.pump("Bild gerendert", |a| a.current.is_some() && a.texture.is_some());
    f.app.set_local_tool(crate::layers::LocalTool::Brush);
    f.app.brushes.eraser.size = 17.0;
    let strokes = |f: &Fixture| f.app.current.as_ref().unwrap().adjustments.layers.last().map(|l| l.mask.strokes.clone()).unwrap_or_default();
    let click = |f: &mut Fixture, p: egui::Pos2, m: egui::Modifiers| {
        let button = |pressed| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: m };
        viewer_frame_with(f, vec![egui::Event::PointerMoved(p)], m);
        viewer_frame_with(f, vec![button(true)], m);
        viewer_frame_with(f, vec![button(false)], m);
        viewer_frame_with(f, vec![], egui::Modifiers::NONE);
    };
    click(&mut f, egui::pos2(500.0, 400.0), egui::Modifiers::NONE);
    click(&mut f, egui::pos2(700.0, 300.0), egui::Modifiers::ALT);
    let s = strokes(&f);
    assert!(!s[0].erase && s[1].erase, "Alt radiert");
    assert!(s[1].radius < s[0].radius, "mit der Größe des Radierers: {} {}", s[1].radius, s[0].radius);
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
    assert!(merged.data.iter().zip(ai.data.iter()).all(|(m, a)| m >= a), "union keeps the magic selection");

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
    // The divider (in the middle of the fitted image) moves when grabbed a little beside it,
    // also where the held button would otherwise show the loupe.
    f.app.local_tool = None;
    f.app.split_pos = 0.5;
    viewer_frame(&mut f, vec![]);
    drag(&mut f, egui::pos2(611.0, 400.0), egui::pos2(800.0, 400.0), 4);
    assert!(f.app.split_pos > 0.6, "Trennlinie bei {}", f.app.split_pos);
    assert!(!f.app.split_dragging);

    // Two recipes -> two files per image.
    let out = f.dir.join("out5");
    f.app.recipes = vec![
        sk_core::export::ExportRecipe { output_dir: out.clone(), size: Some(300), ..Default::default() },
        sk_core::export::ExportRecipe {
            name: "PNG".into(),
            format: sk_core::export::ExportFormat::Png,
            output_dir: out.clone(),
            size: Some(200),
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

    // Importing again: the dialog finds the copies and does not choose them.
    f.app.open_import(photos.clone(), true);
    let start = Instant::now();
    while f.app.import.as_ref().unwrap().checking() {
        assert!(start.elapsed() < LIMIT, "Zeitüberschreitung: Duplikate prüfen");
        f.app.import.as_mut().unwrap().poll(&f.ctx);
        std::thread::sleep(Duration::from_millis(20));
    }
    let items = &f.app.import.as_ref().unwrap().items;
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|i| i.known == Some(true) && !i.chosen));
    f.app.import = None;

    // Move to Selects: file and sidecar move, the database follows, the next image opens.
    f.open_folder(capture.clone());
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

    // An empty folder of the session shows what it is, not the start screen.
    f.open_folder(s.folder(SessionFolder::Trash));
    assert!(f.app.entries.is_empty());
    assert_eq!(f.app.empty_session_folder().map(|(_, t, _)| t).as_deref(), Some("Der Papierkorb ist leer"));
    f.open_folder(capture.clone());

    // Favorites live in the session.
    let fav = s.root.join("Kunde");
    std::fs::create_dir_all(&fav).unwrap();
    f.app.toggle_favorite(&fav);
    assert_eq!(f.app.lists.favorites, vec![fav.clone()]);

    // Processing goes to the session's Output folder.
    f.app.recipes[0].output_dir = "Output".into();
    f.app.recipes[0].size = Some(300);
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
    f.open_folder(again.clone());
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
    f.app.step(1, false);
    assert!(f.app.loading.is_none(), "kein Laden nötig");
    assert_eq!(f.app.current_path().as_ref(), Some(&b));
    assert!(f.app.texture.is_some(), "fertig gerendert");

    // Going back is instant too: the previous image was kept with its rendering.
    f.app.step(-1, false);
    assert!(f.app.loading.is_none());
    assert_eq!(f.app.current_path().as_ref(), Some(&a));
    assert!(f.app.texture.is_some());

    // Both previews are now in the disk cache, so even a cold start shows them at once.
    let adj = sidecar::load(&a).adjustments;
    f.pump("Vorschau im Cache", |_| sk_core::cache::has_preview(&b, &crate::workers::preview_key(&adj), None));
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

/// All movements the system reports in a frame go into the stroke, not only where the pointer
/// ended up: a fast circle stays round.
#[test]
fn fast_strokes_keep_every_reported_movement() {
    let Some(mut f) = Fixture::new("fast_stroke") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    viewer_frame(&mut f, vec![]);
    f.app.set_local_tool(crate::layers::LocalTool::Brush);
    let c = egui::pos2(600.0, 400.0);
    let on_circle = |k: usize| c + egui::vec2(150.0 * (k as f32 * 0.3).cos(), 150.0 * (k as f32 * 0.3).sin());
    viewer_frame(&mut f, vec![egui::Event::PointerMoved(on_circle(0))]);
    viewer_frame(&mut f, vec![press(on_circle(0), true)]);
    // One slow frame: the pointer went a third of the way round.
    viewer_frame(&mut f, (1..=7).map(|k| egui::Event::PointerMoved(on_circle(k))).collect());
    let n = f.app.current.as_ref().unwrap().adjustments.layers[0].mask.strokes[0].points.len();
    assert_eq!(n, 8, "jede Bewegung ein Punkt");
    viewer_frame(&mut f, vec![press(on_circle(7), false)]);
}

/// Painting with the brush renders only what it changed, at full resolution on top of the
/// shown image, instead of drafts of the whole image.
#[test]
fn painting_renders_only_the_painted_part() {
    let Some(mut f) = Fixture::new("paint_part") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    viewer_frame(&mut f, vec![]);
    f.pump("volle Auflösung", |a| a.texture_version_shown() == a.edit_version && !a.texture_draft);
    let full = f.app.texture.as_ref().unwrap().size();
    // Without a measured speed, dragging a control would render a draft.
    f.app.workers.draft_speed = Default::default();
    f.app.set_local_tool(crate::layers::LocalTool::Brush);
    let c = egui::pos2(600.0, 400.0);
    viewer_frame(&mut f, vec![egui::Event::PointerMoved(c)]);
    viewer_frame(&mut f, vec![press(c, true)]);
    for i in 1..=6 {
        viewer_frame(&mut f, vec![egui::Event::PointerMoved(c + egui::vec2(25.0 * i as f32, 8.0 * i as f32))]);
        f.pump("Teil gerendert", |a| a.texture_version_shown() == a.edit_version);
        assert!(!f.app.texture_draft && f.app.texture.as_ref().unwrap().size() == full, "Punkt {i}: kein Entwurf");
    }
    assert!(f.app.current.as_ref().unwrap().adjustments.layers[0].mask.strokes[0].points.len() >= 4);
    viewer_frame(&mut f, vec![press(c + egui::vec2(150.0, 48.0), false)]);
    f.pump("fertig", |a| a.texture_version_shown() == a.edit_version && !a.drag_rendered);
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
    assert!(paths.iter().all(|p| !sk_core::cache::has_preview(p, &crate::workers::preview_key(&adj), None)));

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
    assert!(paths.iter().all(|p| sk_core::cache::has_preview(p, &crate::workers::preview_key(&adj), None)));
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
fn zoom_percent_can_be_set_and_keeps_the_middle() {
    let Some(mut f) = Fixture::new("zoom_pct") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    f.app.zoom = Some(1.0);
    f.app.pan = egui::vec2(40.0, -20.0);
    viewer_frame(&mut f, vec![]);
    f.app.set_zoom_pct(100.0);
    let k = f.app.zoom.unwrap();
    assert!((f.app.pan - egui::vec2(40.0, -20.0) * k).length() < 0.01, "{:?}", f.app.pan);
    viewer_frame(&mut f, vec![]);
    let pct = f.app.zoom_pct.unwrap();
    assert!((pct - 100.0).abs() < 0.5, "{pct}");
}

/// A layer made by Motiv, Himmel or Tiefe is named after it (numbered when taken); the active
/// layer keeps its name.
#[test]
fn automatic_selection_names_its_new_layer() {
    let Some(mut f) = Fixture::new("layer-names") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    let names = |f: &Fixture| f.app.current.as_ref().unwrap().adjustments.layers.iter().map(|l| l.name.clone()).collect::<Vec<_>>();
    f.app.ensure_layer_named(Some("Motiv"));
    f.app.active_layer = None;
    f.app.ensure_layer_named(Some("Motiv"));
    f.app.active_layer = None;
    f.app.ensure_layer();
    assert_eq!(names(&f), ["Motiv", "Motiv 2", "Ebene 3"]);
    // On the active layer: no new one, the name stays.
    f.app.ensure_layer_named(Some("Himmel"));
    assert_eq!(names(&f), ["Motiv", "Motiv 2", "Ebene 3"]);
}

/// The "100 %" switches are on only at 100 %, not at another zoom (221 %), and bring the view
/// there from it.
#[test]
fn hundred_percent_switch_only_at_100() {
    let Some(mut f) = Fixture::new("zoom_100") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    viewer_frame(&mut f, vec![]);
    assert!(!f.app.at_zoom_100(), "eingepasst");
    f.app.set_zoom_pct(221.0);
    viewer_frame(&mut f, vec![]);
    assert!(!f.app.at_zoom_100(), "{:?}", f.app.zoom_pct);
    f.app.zoom_to_100();
    viewer_frame(&mut f, vec![]);
    assert!(f.app.at_zoom_100(), "{:?}", f.app.zoom_pct);
}

/// The slider keeps two decimals of log2: its upper end and 100 % still come out exactly.
#[test]
fn zoom_slider_reaches_whole_percents() {
    let round2 = |v: f32| (v * 100.0).round() / 100.0;
    assert_eq!(crate::browser::slider_pct(round2(400f32.log2())), 400.0);
    assert_eq!(crate::browser::slider_pct(round2(100f32.log2())), 100.0);
}

/// Before the full resolution is loaded, the full size comes from the file, not from the
/// rounded preview (which gave 3999 for 4000 px).
#[test]
fn full_size_before_the_full_resolution_is_exact() {
    let Some(mut f) = Fixture::new("full_size") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    // As long as it is not loaded (it may already be, depending on timing).
    f.app.current.as_mut().unwrap().full = None;
    let cur = f.app.current.as_ref().unwrap();
    let info = (cur.meta.info.width as f32, cur.meta.info.height as f32);
    let (w, h) = f.app.full_output_size().unwrap();
    assert_eq!((w.max(h), w.min(h)), (info.0.max(info.1), info.0.min(info.1)));
}

#[test]
fn navigator_moves_the_zoomed_view() {
    let Some(mut f) = Fixture::new("navigator") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    let size = f.app.image_size().unwrap();
    let scale = 2.0;
    f.app.zoom = Some(scale);
    viewer_frame(&mut f, vec![]);
    // Bottom right of the 1200 x 800 viewer above the zoom row, 200 px wide for a 3:2 image.
    let nav = egui::Rect::from_min_size(egui::pos2(1200.0 - 16.0 - 200.0, 800.0 - 16.0 - 26.0 - 200.0 / (size.x / size.y)), egui::vec2(200.0, 200.0 / (size.x / size.y)));
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
    assert!(!sk_core::cache::has_preview(&a, &crate::workers::preview_key(&look), None), "Vorschau landet nicht im Cache");

    // Apply: only the look changes, exposure is kept.
    f.app.style_preview = None;
    f.app.set_adjustments_on(std::slice::from_ref(&a), |adj| sk_core::styles::apply_look(&velvia, adj));
    f.app.commit_and_save(); // (the app does this once the mouse is released)
    let saved = sidecar::load(&a).adjustments;
    assert_eq!(saved.exposure, 0.5);
    assert_eq!(saved.saturation, velvia.adjustments.saturation);
    assert_eq!(f.app.current.as_ref().unwrap().adjustments, saved);
}

/// The style grid renders every film look on the current image in the background, and again
/// once the image's adjustments are committed; results of an older request are dropped.
#[test]
fn style_grid_previews_the_current_image() {
    let Some(mut f) = Fixture::new("stilraster") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    let n = f.app.film_looks.len();
    f.app.pump_style_thumbs();
    let first = f.app.style_grid_key_for_test();
    f.pump("Vorschauen", |a| a.style_grid_count_for_test() == n);
    // Nothing changed: no new request.
    f.app.pump_style_thumbs();
    assert_eq!(f.app.style_grid_key_for_test(), first);

    // A committed edit renders them again; the old previews stay until replaced.
    f.app.edit(|a| a.exposure = 1.0);
    f.app.commit_and_save();
    f.app.pump_style_thumbs();
    let second = f.app.style_grid_key_for_test();
    assert!(second > first);
    assert_eq!(f.app.style_grid_count_for_test(), n);
    let ctx = f.ctx.clone();
    f.app.style_thumb_arrived(&ctx, first, "film:alt".into(), egui::ColorImage::filled([2, 2], egui::Color32::RED));
    assert_eq!(f.app.style_grid_count_for_test(), n, "veraltetes Ergebnis verworfen");
}

#[test]
#[ignore = "langsam: läuft mit cargo t-all"]
fn proof_shows_the_image_as_the_recipe_writes_it() {
    let Some(mut f) = Fixture::new("proof") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    f.app.recipes = vec![sk_core::export::ExportRecipe { size: Some(1200), quality: 70, ..Default::default() }];
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

    // Back to the first state: its proof is shown again at once, without computing it.
    let exposure = f.app.current.as_ref().unwrap().adjustments.exposure;
    f.app.recipes[0].quality = 70;
    f.app.edit(|adj| adj.exposure = 0.0);
    assert_ne!(exposure, 0.0);
    f.app.update_proof(&f.ctx.clone());
    let p = f.app.proof.as_ref().expect("Proof aus dem Zwischenspeicher");
    assert_eq!((p.version, p.key, p.file_size), (f.app.edit_version, key, size));
}

#[test]
fn fullscreen_toggles_and_esc_leaves_it_unless_a_tool_is_active() {
    let Some(mut f) = Fixture::new("fullscreen") else { return };
    // Full screen shows the image: without one there is nothing to switch to.
    f.pump("Bild geladen", |a| a.current.is_some());
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

    // With the first image as look reference, "Look angleichen" changes exposure and white
    // balance of the current image (the same scene: matched fully), as one undo step.
    assert!(!f.app.can_match_look(), "noch keine Vorlage");
    f.app.add_look_ref(crate::look::LookRef::Image(a.clone()));
    assert!(f.app.can_match_look());
    f.app.match_look_selection();
    f.pump("angeglichen", |app| app.look_progress.is_none());
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
fn look_from_a_picture_file_with_strength() {
    let Some(mut f) = Fixture::new("look") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    // Without a reference nothing to match to.
    f.app.look_refs.clear();
    assert!(!f.app.can_match_look());
    // A dark, warm, low-contrast picture.
    let file = f.dir.join("look.png");
    image::RgbImage::from_fn(96, 64, |x, y| {
        let v = 40 + (x + y) as u8 / 2;
        image::Rgb([v + 30, v + 10, v])
    })
    .save(&file)
    .unwrap();
    f.app.add_look_ref(crate::look::LookRef::File(file.clone()));
    f.pump("Vorlage gemessen", |app| matches!(app.look_ref_state.get(&crate::look::LookRef::File(file.clone())), Some(crate::look::LookRefState::Ready(_))));
    assert!(f.app.can_match_look());
    // Twice the same: kept once.
    f.app.add_look_ref(crate::look::LookRef::File(file.clone()));
    assert_eq!(f.app.look_refs.len(), 1);
    let before = f.app.current.as_ref().unwrap().adjustments.clone();
    f.app.match_look_selection();
    f.pump("angeglichen", |app| app.look_progress.is_none());
    let matched = f.app.current.as_ref().unwrap().adjustments.clone();
    assert!(matched.exposure < before.exposure, "Belichtung {}", matched.exposure);
    assert!(!matched.curves.luma.is_identity());
    let mix = f.app.look_mix.as_ref().expect("Stärke");
    assert_eq!(mix.strength, 100.0);
    assert!(mix.summary.contains("Belichtung"), "{}", mix.summary);
    // Strength 0: back to before; the slider leaves the rest alone.
    let (opts, b, m) = (mix.opts, mix.before.clone(), mix.matched.clone());
    let cur = f.app.current.as_ref().unwrap();
    let zero = opts.blend(&cur.adjustments, &b, &m, 0.0, cur.as_shot_temp_tint);
    assert_eq!(zero.exposure, before.exposure);
    assert!(zero.curves.luma.is_identity());
    // One undo step.
    f.app.undo();
    assert_eq!(f.app.current.as_ref().unwrap().adjustments.exposure, before.exposure);
}

#[test]
fn color_zone_pipette_adds_and_moves_a_range() {
    use crate::develop_tools::ColorPick;
    let Some(mut f) = Fixture::new("zone") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    let size = f.app.image_size().unwrap();
    // The first colorful spot of a grid over the image.
    let spots: Vec<egui::Vec2> = (1..6).flat_map(|y| (1..6).map(move |x| egui::vec2(x as f32 / 6.0, y as f32 / 6.0) * size)).collect();
    let mut picked = None;
    for &pos in &spots {
        f.app.toggle_color_picker(ColorPick::NewZone);
        assert!(f.app.picking_skin);
        f.app.pick_skin_tone(pos);
        if !f.app.current.as_ref().unwrap().adjustments.color_editor.zones.is_empty() {
            picked = Some(pos);
            break;
        }
        assert!(f.app.status.contains("zu farblos"), "{}", f.app.status);
        f.app.end_viewer_tools();
    }
    let Some(_) = picked else { return };
    assert!(!f.app.picking_skin);
    let zones = f.app.current.as_ref().unwrap().adjustments.color_editor.zones.clone();
    assert_eq!((zones.len(), f.app.color_zone), (1, 0));
    // A new range changes nothing until one of its sliders moves.
    assert!(f.app.current.as_ref().unwrap().adjustments.color_editor.is_neutral());
    // Picking again for the range moves it instead of adding one.
    f.app.current.as_mut().unwrap().adjustments.color_editor.zones[0].hue = (zones[0].hue + 90.0) % 360.0;
    f.app.toggle_color_picker(ColorPick::Zone(0));
    f.app.pick_skin_tone(picked.unwrap());
    let again = &f.app.current.as_ref().unwrap().adjustments.color_editor.zones;
    assert_eq!((again.len(), again[0].hue), (1, zones[0].hue));
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
    f.index_photos();
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
    f.pump("Schärfemaske berechnet", |app| app.focus_maps.contains_key(&a));
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
fn thumbnails_show_the_focus_mask() {
    let Some(mut f) = Fixture::new("thumb_focus") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    let b = f.photo("_DSC0010.dng");
    let ctx = f.ctx.clone();
    // Off: nothing is asked for.
    assert!(f.app.thumb_focus_overlay(&ctx, &b, 1.5).is_none());
    assert!(f.app.thumb_focus_requested.is_empty());
    f.app.toggle_focus_mask();
    assert!(f.app.thumb_focus_overlay(&ctx, &b, 1.5).is_none());
    f.pump("Schärfemaske Vorschaubild", |app| app.thumb_focus.contains_key(&b));
    let [w, h] = f.app.thumb_focus[&b].size;
    assert!(w.max(h) <= crate::workers::THUMB_EDGE as usize);
    assert!(f.app.thumb_focus_overlay(&ctx, &b, w as f32 / h as f32).is_some());
    // A new thumbnail (the image was edited) makes it again.
    f.app.set_thumb(&ctx, b.clone(), egui::ColorImage::new([4, 4], vec![egui::Color32::BLACK; 16]));
    assert!(!f.app.thumb_focus.contains_key(&b));
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
    f.pump("Schärfemaske", |app| app.focus_map_current(&a));
    // Off, edited (e.g. rotated), on again: the old mask doesn't fit and isn't shown ...
    f.app.toggle_focus_mask();
    f.app.edit(|adj| adj.exposure = 0.7);
    f.pump("neu gerendert", |app| app.texture_version_shown() == app.edit_version && !app.texture_draft);
    f.app.toggle_focus_mask();
    assert!(!f.app.focus_map_current(&a));
    // ... until it is computed again for the shown rendering.
    f.app.ensure_focus_of_current();
    f.pump("Schärfemaske neu", |app| app.focus_map_current(&a));
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
    // Together with another image: the variant's file would go with all its variants.
    let b = f.photo("_DSC0010.dng");
    f.app.select_path(&b, egui::Modifiers::COMMAND);
    assert_eq!(f.app.selected_paths().len(), 2);
    f.app.trash_selection();
    assert!(f.app.status.contains("getrennt"), "{}", f.app.status);
    assert_eq!(f.app.confirm_delete_variant, None);
    assert!(a.exists() && b.exists());
    f.app.select_path(&v2, egui::Modifiers::NONE);
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
    f.app.auto_tool(sk_core::tools::Tool::Perspective);
    f.pump("ausgerichtet", |app| app.auto_progress.is_none());
    assert!(f.app.status.starts_with("Perspektive automatisch"), "{}", f.app.status);
    let g = f.app.current.as_ref().unwrap().adjustments.geometry.clone();
    assert!(g.straighten.abs() <= 15.0 && g.keystone_h == 0.0);
    if g != Default::default() {
        f.app.undo();
        assert_eq!(f.app.current.as_ref().unwrap().adjustments.geometry, Default::default());
    }
}

/// Keystone lines: two converging lines laid on "edges" become vertical in one undo step; the
/// amount the user set stays and weakens the correction.
#[test]
fn keystone_lines_correct_converging_verticals() {
    let Some(mut f) = Fixture::new("keystone-lines") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    f.app.edit(|a| a.geometry.keystone_amount = 80.0);
    f.app.commit_and_save();
    let undo = f.app.undo.len();
    f.app.start_keystone_guides(crate::crop::GuideMode::Vertical);
    assert!(f.app.keystone_guides_active() && !f.app.straightening());
    // Lines leaning towards each other at the top, as in a building shot from below.
    let lines = &mut f.app.straighten_tool.as_mut().unwrap().guides.as_mut().unwrap().lines;
    assert_eq!(lines.len(), 2);
    let dx = (lines[1].0[0][0] - lines[0].0[0][0]) * 0.1;
    lines[0].0[0][0] += dx;
    lines[1].0[0][0] -= dx;
    f.app.finish_viewer_tool();
    assert!(f.app.straighten_tool.is_none());
    let g = f.app.current.as_ref().unwrap().adjustments.geometry.clone();
    assert!(g.keystone_v.abs() > 5.0 && g.keystone_h == 0.0, "{g:?}");
    assert_eq!(g.keystone_amount, 80.0);
    assert!(f.app.status.starts_with("Trapezkorrektur"), "{}", f.app.status);
    assert_eq!(f.app.undo.len(), undo + 1);
}

/// Moved keystone lines are not lost when the image is closed (other folder, quitting).
#[test]
fn moved_keystone_lines_are_applied_on_close() {
    let Some(mut f) = Fixture::new("keystone-close") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    let path = f.app.current_path().unwrap();
    f.app.start_keystone_guides(crate::crop::GuideMode::Vertical);
    let guides = f.app.straighten_tool.as_mut().unwrap().guides.as_mut().unwrap();
    let dx = (guides.lines[1].0[0][0] - guides.lines[0].0[0][0]) * 0.1;
    guides.lines[0].0[0][0] += dx;
    guides.lines[1].0[0][0] -= dx;
    guides.moved = true;
    f.app.close_current();
    assert!(f.app.straighten_tool.is_none());
    assert!(sidecar::load(&path).adjustments.geometry.keystone_v.abs() > 5.0);
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
fn undo_history_is_bounded_and_shares_masks() {
    let Some(mut f) = Fixture::new("undo-limit") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    let mask = sk_core::params::AiMask { kind: "ai".into(), width: 32, height: 32, data: vec![200u8; 32 * 32].into(), depth: None };
    f.app.edit(|a| {
        a.layers.push(Default::default());
        a.layers[0].mask.ai = Some(mask.clone());
    });
    f.app.commit_and_save();
    for i in 0..crate::app::UNDO_LIMIT + 20 {
        f.app.edit(|a| a.exposure = i as f32 / 1000.0);
        f.app.commit_and_save();
    }
    assert_eq!(f.app.undo.len(), crate::app::UNDO_LIMIT);
    // Every step refers to the same mask data instead of a copy of its own.
    let data = |a: &sk_core::params::Adjustments| a.layers[0].mask.ai.as_ref().unwrap().data.clone();
    assert!(f.app.undo.iter().all(|a| std::sync::Arc::ptr_eq(&data(a), &mask.data)));
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
    assert_eq!(sidecar::load(&path).adjustments.exposure, 0.5, "der Sprung wird gespeichert");
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
    f.open_folder(folder.clone());
    assert_eq!(f.app.primary_path(), Some(b.clone()));
    f.pump("B geladen", |app| app.current_path().as_ref() == Some(&b));
    // Only once: opening the folder later starts with the current image as usual.
    assert!(f.app.restore_image.is_none());
}

#[test]
fn folder_loads_in_the_background_and_late_results_are_dropped() {
    let Some(mut f) = Fixture::new("folder-load") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    let photos = f.dir.join("photos").canonicalize().unwrap();
    let empty = f.dir.join("leer");
    std::fs::create_dir_all(&empty).unwrap();
    f.app.open_folder(photos.clone());
    let stale = f.app.scope_run;
    assert!(f.app.folder_loading);
    assert!(f.app.activities().iter().any(|a| a.label == "Lade Ordner"));
    // Clicked on before the first folder arrived: only the second one is shown.
    f.open_folder(empty.canonicalize().unwrap());
    assert!(f.app.entries.is_empty() && f.app.current.is_none());
    f.app.workers.inject(crate::workers::Event::FolderLoaded { run: stale, files: sidecar::load_all(&sk_core::catalog::scan_folder(&photos, false)) });
    f.app.handle_events(&f.ctx);
    assert!(f.app.entries.is_empty(), "veraltetes Ergebnis verworfen");
    f.open_folder(photos);
    assert_eq!(f.app.entries.len(), 2);
    assert!(f.app.primary_path().is_some(), "das erste Bild ist gewählt");
}

/// A chosen retouch area grows with Shift and shrinks with Alt (or as chosen in the panel);
/// without a modifier a new area is painted.
#[test]
fn painting_extends_and_shrinks_the_chosen_retouch_area() {
    let Some(mut f) = Fixture::new("spot-extend") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    viewer_frame(&mut f, vec![]);
    let c = egui::pos2(600.0, 400.0);
    let spots = |f: &Fixture| f.app.current.as_ref().unwrap().adjustments.retouch.clone();
    f.app.set_local_tool(crate::layers::LocalTool::Clone);
    drag(&mut f, c, c + egui::vec2(100.0, 0.0), 6);
    assert_eq!(spots(&f).len(), 1);
    assert_eq!(f.app.selected_spot, Some(0));

    drag_with(&mut f, c + egui::vec2(50.0, 0.0), c + egui::vec2(50.0, 120.0), 6, egui::Modifiers::SHIFT);
    let s = spots(&f);
    assert_eq!(s.len(), 1, "erweitert statt neu");
    assert_eq!(s[0].strokes.len(), 1);
    assert!(!s[0].strokes[0].erase && s[0].strokes[0].points.len() >= 3);
    drag_with(&mut f, c + egui::vec2(90.0, 0.0), c + egui::vec2(110.0, 0.0), 3, egui::Modifiers::ALT);
    assert!(spots(&f)[0].strokes[1].erase);

    // As chosen in the panel, without a key.
    f.app.area_paint = crate::layers::AreaPaint::Extend;
    drag(&mut f, c + egui::vec2(0.0, 60.0), c + egui::vec2(-60.0, 60.0), 4);
    assert_eq!(spots(&f)[0].strokes.len(), 3);

    // Moving the area moves what was painted onto it.
    let before = spots(&f)[0].strokes[0].points[0];
    drag(&mut f, c, c + egui::vec2(-40.0, 0.0), 4);
    let after = spots(&f)[0].strokes[0].points[0];
    assert!(after[0] < before[0] - 0.01 && (after[1] - before[1]).abs() < 1e-4, "{before:?} -> {after:?}");

    // A new area again with "Neu".
    f.app.area_paint = crate::layers::AreaPaint::New;
    drag(&mut f, c + egui::vec2(-200.0, -150.0), c + egui::vec2(-150.0, -150.0), 3);
    assert_eq!(spots(&f).len(), 2);
}

/// A removed area can be chosen, extended (filled anew in its place, keeping its opacity) and
/// deleted on its own.
#[test]
fn removed_areas_can_be_extended_and_deleted() {
    if !sk_ai::models::available(&sk_ai::models::INPAINT) {
        return;
    }
    let Some(mut f) = Fixture::new("remove-extend") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    viewer_frame(&mut f, vec![]);
    let c = egui::pos2(600.0, 400.0);
    let removals = |f: &Fixture| f.app.current.as_ref().unwrap().adjustments.removals.clone();
    f.app.set_local_tool(crate::layers::LocalTool::Remove);
    drag(&mut f, c, c + egui::vec2(40.0, 0.0), 3);
    f.pump("Entfernt", |a| !a.inpaint_busy);
    assert_eq!(removals(&f).len(), 1, "{}", f.app.status);
    assert_eq!(f.app.selected_removal, Some(0));
    let first = removals(&f)[0].clone();
    f.app.edit(|a| a.removals[0].opacity = 0.6);

    drag_with(&mut f, c + egui::vec2(40.0, 20.0), c + egui::vec2(40.0, 60.0), 3, egui::Modifiers::SHIFT);
    assert_eq!(f.app.remove_replaces, Some(first.id));
    assert_eq!(f.app.remove_strokes.len(), first.strokes.len() + 1);
    f.pump("Neu gefüllt", |a| !a.inpaint_busy);
    let r = removals(&f);
    assert_eq!(r.len(), 1, "an seiner Stelle");
    assert_ne!(r[0].id, first.id);
    assert_eq!(r[0].strokes.len(), first.strokes.len() + 1);
    assert_eq!(r[0].opacity, 0.6);
    assert!(r[0].rect[3] > first.rect[3], "größer: {:?} {:?}", r[0].rect, first.rect);
    assert!(f.app.remove_replaces.is_none());

    // Entf deletes the chosen one only.
    drag(&mut f, c + egui::vec2(-200.0, -100.0), c + egui::vec2(-180.0, -100.0), 2);
    f.pump("Zweiter Bereich", |a| !a.inpaint_busy);
    assert_eq!(removals(&f).len(), 2);
    f.app.selected_removal = Some(0);
    f.app.delete_selected_spot();
    assert_eq!(removals(&f).len(), 1);
    assert_ne!(removals(&f)[0].id, r[0].id);
}

/// Cancelling an export stops before the next image and says how far it got; the next export
/// runs through.
#[test]
fn export_can_be_cancelled() {
    let Some(mut f) = Fixture::new("export-cancel") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    let photos = f.dir.join("photos");
    let source = f.photo("_DSC0009.dng");
    for i in 0..4 {
        let target = photos.join(format!("_KOP{i:04}.dng"));
        if std::fs::hard_link(&source, &target).is_err() {
            std::fs::copy(&source, &target).unwrap();
        }
    }
    f.open_folder(photos.canonicalize().unwrap());
    assert_eq!(f.app.entries.len(), 6);
    let out = f.dir.join("out");
    f.app.recipes[0].output_dir = out.clone();
    f.app.recipes[0].size = Some(300);
    f.app.selection = f.app.entries.iter().map(|e| e.path.clone()).collect();
    f.app.export_selection();
    f.pump("Erstes Bild", |a| !a.export_log.is_empty());
    f.app.export_cancel.as_ref().expect("abbrechbar").store(true, std::sync::atomic::Ordering::Relaxed);
    f.pump("Abgebrochen", |a| a.export_cancel.is_none());
    assert!(f.app.status.contains("abgebrochen"), "{}", f.app.status);
    let written = std::fs::read_dir(&out).unwrap().count();
    assert!(f.app.export_log.len() < 6 && written == f.app.export_log.len(), "{} geschrieben, {:?}", written, f.app.export_log);

    // The next export runs through, in the order of the selection.
    f.app.export_selection();
    f.pump("Export", |a| a.export_progress.is_none() && a.export_cancel.is_none() && a.export_log.len() >= 6);
    assert!(f.app.export_log.iter().all(|l| l.starts_with('✔')), "{:?}", f.app.export_log);
}

#[test]
fn xmp_exchange() {
    let Some(mut f) = Fixture::new("xmp") else { return };
    f.index_photos();
    f.pump("Indexiert", |a| a.lists.total == 2);
    let a = f.photo("_DSC0009.dng");
    let b = f.photo("_DSC0010.dng");
    // Written by another program: read on request and mirrored in catalog and browser.
    std::fs::write(f.dir.join("photos/_DSC0010.xmp"), sk_core::xmp::updated(None, 3, ColorLabel::Green, &["Urlaub".into()]).unwrap()).unwrap();
    f.app.selection = [b.clone()].into_iter().collect();
    f.app.read_xmp_selection();
    let e = &f.app.entries[f.app.index_of(&b).unwrap()];
    assert_eq!((e.rating, e.color_label, e.keywords.clone()), (3, ColorLabel::Green, vec!["Urlaub".to_string()]));
    assert_eq!(sidecar::load(&b).rating, 3);
    assert!(f.app.lists.keywords.iter().any(|(k, _)| k == "Urlaub"));
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
    f.index_photos();
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
    f.index_photos();
    f.pump("Indexiert", |a| a.lists.total == 2);
    f.app.select_all();
    f.app.open_merge(crate::merge::MergeKind::Hdr);
    let dialog = f.app.merge_dialog.take().expect("Dialog");
    assert_eq!(dialog.files.len(), 2);
    let files = dialog.files.clone();
    let opts = f.app.hdr_options;
    f.app.start_job("HDR zusammenfügen", Box::new(move |p| Ok(vec![sk_core::hdr::merge_files(&files, &opts, p)?])));
    assert!(f.app.activities().iter().any(|a| a.label == "Füge HDR zusammen" && a.progress.is_some() && !a.in_viewer));
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
    f.app.remove_strokes = vec![sk_core::params::BrushStroke { points: vec![[0.4, 0.5], [0.45, 0.52]], radius: 0.01, hardness: 0.7, flow: 1.0, erase: false, opacity: 1.0, auto_mask: false, build_up: true, tolerance: 50.0 }];
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
    f.index_photos();
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
    sk_core::dng::write_linear_dng(out, &sk_core::dng::LinearDng { width: w, height: h, data: &data, color: &sk_core::color::CameraColor::new(xyz_to_cam), as_shot_wb: [1.0; 3], info: &info, baseline_exposure: 0.0, preview: Some(&img), lens_corrected: false }).unwrap();
}

/// One person recognized as two is merged by hand: one person with the images of both, the
/// keyword of the name on all of them.
#[test]
fn persons_merged_by_hand() {
    let Some(mut f) = Fixture::new("merge-persons") else { return };
    f.index_photos();
    f.pump("Indexiert", |a| a.lists.total == 2);
    let (a, b) = (f.photo("_DSC0009.dng"), f.photo("_DSC0010.dng"));
    let face = |k: usize| sk_core::catalog::FaceRecord { bbox: [0.4, 0.3, 0.2, 0.25], embedding: (0..8).map(|i| if i == k { 1.0 } else { 0.0 }).collect(), eyes_closed: 0.0 };
    {
        let c = f.app.catalog.as_ref().unwrap();
        c.store_analysis(&a, 0.5, &[face(0)], 0.42).unwrap();
        c.store_analysis(&b, 0.5, &[face(4)], 0.42).unwrap();
    }
    f.app.refresh_catalog_lists();
    let ids = |app: &App| app.catalog.as_ref().unwrap().persons(1).unwrap().into_iter().map(|p| p.id).collect::<Vec<_>>();
    let found = ids(&f.app);
    assert_eq!(found.len(), 2);
    let (anna, other) = (found[0], found[1]);
    f.app.name_person(anna, "Anna");
    f.app.open_scope(sk_core::catalog::Scope::Person(other));
    f.app.merge_persons(anna, other);
    assert_eq!(ids(&f.app), vec![anna]);
    for p in [&a, &b] {
        assert!(sidecar::load(p).keywords.contains(&"Anna".to_string()), "{}", p.display());
    }
    // The view of the person that is gone shows the merged one.
    assert_eq!(f.app.scope, sk_core::catalog::Scope::Person(anna));
    assert_eq!(f.app.entries.len(), 2);

    // And back: the face in the second image is someone else, split off as a new person to name.
    let faces = f.app.catalog.as_ref().unwrap().person_faces(anna).unwrap();
    let in_b: Vec<i64> = faces.iter().filter(|x| x.path == b).map(|x| x.id).collect();
    f.app.move_faces(anna, &in_b, None);
    let ids_now = ids(&f.app);
    assert_eq!(ids_now.len(), 2);
    let ben = *ids_now.iter().find(|id| **id != anna).unwrap();
    assert_eq!(f.app.library_ui.naming.as_ref().map(|(id, _)| *id), Some(ben), "gleich benennen");
    assert!(f.app.lists.persons.iter().any(|p| p.id == ben), "mit einem Bild in der Liste");
    assert!(sidecar::load(&a).keywords.contains(&"Anna".to_string()));
    assert!(!sidecar::load(&b).keywords.contains(&"Anna".to_string()), "Bild ohne Anna verliert den Namen");
    f.app.name_person(ben, "Ben");
    assert!(sidecar::load(&b).keywords.contains(&"Ben".to_string()));
    // Given back to Anna: Ben is gone, the image gets her name again.
    f.app.move_faces(ben, &in_b, Some(anna));
    assert_eq!(ids(&f.app), vec![anna]);
    assert!(sidecar::load(&b).keywords.contains(&"Anna".to_string()));
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
    f.open_folder(photos.clone());
    f.index_photos();
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
    // The list shows the person's face, cut out square.
    let (path, bbox) = person.sample.clone().expect("Beispielgesicht");
    let face = crate::culling::face_thumbnail(&path, bbox).unwrap();
    assert_eq!(face.size, [96, 96]);
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
        ("Hautton-Pipette", |a| a.toggle_color_picker(crate::develop_tools::ColorPick::Skin), |a| a.picking_skin),
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
/// Alt+click takes part of it away, and Shift+click starts another object, keeping the first.
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

    // Shift+click elsewhere: a new object of its own (one click), added to the first.
    let shift = egui::Modifiers::SHIFT;
    let d = egui::pos2(300.0, 250.0);
    viewer_frame_with(&mut f, vec![egui::Event::PointerMoved(d)], shift);
    viewer_frame_with(&mut f, vec![egui::Event::PointerButton { pos: d, button: egui::PointerButton::Primary, pressed: true, modifiers: shift }], shift);
    viewer_frame_with(&mut f, vec![egui::Event::PointerButton { pos: d, button: egui::PointerButton::Primary, pressed: false, modifiers: shift }], shift);
    viewer_frame_with(&mut f, vec![], egui::Modifiers::NONE);
    assert_eq!(f.app.object_session.as_ref().map(|s| s.clicks.len()), Some(1), "neues Objekt");
    f.pump("weiteres Objekt", |a| !a.ai_busy);
    let both = mask(&f).unwrap();
    let kept = first.data.iter().zip(both.data.iter()).filter(|(a, b)| **a > 128 && **b > 128).count();
    assert!(kept * 10 >= n * 9, "das erste Objekt bleibt: {kept} von {n}");
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

#[test]
fn dragging_sharpening_at_100_percent_renders_the_view_sharp() {
    let Some(mut f) = Fixture::full_size("sharpen-drag") else { return };
    let step = |f: &mut Fixture, events: Vec<egui::Event>| {
        let screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0)));
        run_viewer(f, egui::RawInput { screen_rect, max_texture_side: Some(8192), events, ..Default::default() });
        f.app.handle_events(&f.ctx.clone());
        f.app.send_render();
        std::thread::sleep(Duration::from_millis(20));
    };
    let start = Instant::now();
    while f.app.image_size().is_none() || f.app.view_geometry.is_none() {
        assert!(start.elapsed() < LIMIT, "Zeitüberschreitung: Bild");
        step(&mut f, vec![]);
    }
    f.app.toggle_zoom_100();
    while !f.app.tiles.view.as_ref().is_some_and(|v| v.level == 1 && v.version == f.app.edit_version) {
        assert!(start.elapsed() < LIMIT, "Zeitüberschreitung: scharfe Ansicht");
        step(&mut f, vec![]);
    }
    let p = egui::pos2(600.0, 400.0);
    step(&mut f, vec![egui::Event::PointerMoved(p)]);
    let drag = |f: &mut Fixture, edit: fn(&mut sk_core::params::Adjustments)| {
        // Without a measured speed: drafts would be coarse.
        f.app.workers.draft_speed = Default::default();
        step(f, vec![press(p, true)]);
        f.app.edit(edit);
        while !f.app.tiles.view.as_ref().is_some_and(|v| v.version == f.app.edit_version) {
            assert!(start.elapsed() < LIMIT, "Zeitüberschreitung: Ansicht beim Ziehen");
            step(f, vec![]);
        }
        let level = f.app.tiles.view.as_ref().unwrap().level;
        step(f, vec![press(p, false)]);
        // As the app does once the mouse is up.
        (f.app.drag_draft_level, f.app.drag_view_level) = (None, None);
        f.app.commit_and_save();
        level
    };
    assert_eq!(drag(&mut f, |a| a.detail.sharpen_amount = 300.0), 1, "Schärfen in voller Auflösung");
    assert!(drag(&mut f, |a| a.exposure = 0.4) > 1, "andere Regler weiter als Entwurf");
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

#[test]
fn sidecars_that_cant_be_read_or_saved() {
    let Some(mut f) = Fixture::new("unreadable") else { return };
    // Sidecar files, as for images outside a session while it is open (the catalog would take
    // these in and keep their settings).
    f.app.settings_store.as_ref().unwrap().set_adopt(false);
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    let b = f.photo("_DSC0010.dng");

    // Settings of an image that couldn't be saved stay when it is changed from the grid.
    let mut pending = sidecar::Sidecar::default();
    pending.adjustments.exposure = 0.9;
    f.app.unsaved.insert(b.clone(), pending);
    f.app.selection = [b.clone()].into_iter().collect();
    f.app.set_rating(2);
    let saved = sidecar::load(&b);
    assert_eq!((saved.rating, saved.adjustments.exposure), (2, 0.9));
    assert!(f.app.unsaved.is_empty());

    // A sidecar that can't be read is not replaced with the defaults shown instead.
    let file = sidecar::sidecar_path(&b);
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    f.app.select_path(&b, egui::Modifiers::NONE);
    f.pump("zweites Bild", |app| app.current_path().as_ref() == Some(&b));
    f.app.set_rating(5);
    assert!(f.app.status.contains("nicht lesbar"), "{}", f.app.status);
    assert!(file.is_dir());
    assert!(f.app.unsaved.is_empty());
}

/// A 200 pt wide slider over 0 … 100 at the top left (rail from x = 7 to 193, handle at 7 + 1.86 · value).
struct SliderHarness {
    ctx: egui::Context,
    value: f32,
    time: f64,
    mods: egui::Modifiers,
}

impl SliderHarness {
    fn new(value: f32) -> Self {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut h = SliderHarness { ctx, value, time: 0.0, mods: egui::Modifiers::NONE };
        h.frame(vec![]);
        h
    }

    fn x(v: f32) -> f32 {
        7.0 + 1.86 * v
    }

    fn frame(&mut self, events: Vec<egui::Event>) {
        self.time += 1.0 / 60.0;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))),
            events: std::iter::once(egui::Event::ModifiersChanged(self.mods)).chain(events).collect(),
            time: Some(self.time),
            ..Default::default()
        };
        let value = &mut self.value;
        let out = self.ctx.run_ui(input, |ui| {
            crate::widgets::begin_wheel_gesture(ui.ctx());
            egui::CentralPanel::no_frame().show(ui, |ui| {
                crate::widgets::compact_slider(ui, "x", value, 0.0..=100.0, 50.0, 200.0);
            });
        });
        let mut delta = out.textures_delta;
        delta.clear();
    }

    fn button(&mut self, p: egui::Pos2, pressed: bool) {
        let modifiers = self.mods;
        self.frame(vec![egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers }]);
    }

    fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
        self.frame(vec![egui::Event::PointerMoved(from)]);
        self.button(from, true);
        for i in 1..=10 {
            self.frame(vec![egui::Event::PointerMoved(from + (to - from) * i as f32 / 10.0)]);
        }
        self.button(to, false);
    }

    fn key(&mut self, key: egui::Key) {
        let modifiers = self.mods;
        self.frame(vec![
            egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers },
            egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers },
        ]);
    }

    fn wheel(&mut self, at: egui::Pos2, lines: f32) {
        let modifiers = self.mods;
        self.frame(vec![
            egui::Event::PointerMoved(at),
            egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: egui::vec2(0.0, lines), phase: egui::TouchPhase::Move, modifiers },
        ]);
    }
}

#[test]
fn slider_handle_is_dragged_from_where_it_is() {
    // Grabbing the handle a little off its centre does not jump, the value follows the motion.
    let mut h = SliderHarness::new(50.0);
    let grab = egui::pos2(SliderHarness::x(50.0) + 4.0, 10.0);
    h.drag(grab, grab + egui::vec2(18.6, 0.0));
    assert!((h.value - 60.0).abs() < 0.05, "{}", h.value);
    // Shift: a tenth of that.
    h.mods = egui::Modifiers::SHIFT;
    let grab = egui::pos2(SliderHarness::x(60.0), 10.0);
    h.drag(grab, grab + egui::vec2(18.6, 0.0));
    assert!((h.value - 61.0).abs() < 0.05, "{}", h.value);
    // Past the end and back: the handle is under the pointer again.
    h.mods = egui::Modifiers::NONE;
    let grab = egui::pos2(SliderHarness::x(61.0), 10.0);
    h.frame(vec![egui::Event::PointerMoved(grab)]);
    h.button(grab, true);
    h.frame(vec![egui::Event::PointerMoved(egui::pos2(390.0, 10.0))]);
    assert_eq!(h.value, 100.0);
    h.frame(vec![egui::Event::PointerMoved(egui::pos2(SliderHarness::x(80.0), 10.0))]);
    h.button(egui::pos2(SliderHarness::x(80.0), 10.0), false);
    assert!((h.value - 80.0).abs() < 0.05, "{}", h.value);
}

#[test]
fn slider_click_on_the_rail_jumps_there() {
    let mut h = SliderHarness::new(50.0);
    let p = egui::pos2(SliderHarness::x(10.0), 10.0);
    h.frame(vec![egui::Event::PointerMoved(p)]);
    h.button(p, true);
    h.button(p, false);
    assert!((h.value - 10.0).abs() < 0.05, "{}", h.value);
}

#[test]
fn clicked_slider_takes_the_arrow_keys() {
    let mut h = SliderHarness::new(50.0);
    let p = egui::pos2(SliderHarness::x(50.0), 10.0);
    h.frame(vec![egui::Event::PointerMoved(p)]);
    h.button(p, true);
    h.button(p, false);
    h.frame(vec![]);
    assert!(crate::widgets::slider_has_focus(&h.ctx));
    h.key(egui::Key::ArrowRight);
    assert!((h.value - 51.0).abs() < 0.005, "{}", h.value);
    h.mods = egui::Modifiers::SHIFT;
    h.key(egui::Key::ArrowLeft);
    assert!((h.value - 46.0).abs() < 0.005, "{}", h.value);
    h.mods = egui::Modifiers::ALT;
    h.key(egui::Key::ArrowUp);
    assert!((h.value - 46.1).abs() < 0.005, "{}", h.value);
    // A click elsewhere gives the keys back.
    h.mods = egui::Modifiers::NONE;
    let away = egui::pos2(300.0, 200.0);
    h.frame(vec![egui::Event::PointerMoved(away)]);
    h.button(away, true);
    h.button(away, false);
    h.frame(vec![]);
    assert!(!crate::widgets::slider_has_focus(&h.ctx));
}

#[test]
fn wheel_adjusts_a_slider_only_after_a_click_on_it() {
    let handle = egui::pos2(SliderHarness::x(50.0), 10.0);
    let rail = egui::pos2(SliderHarness::x(50.0) + 30.0, 10.0);
    let mut h = SliderHarness::new(50.0);
    // Merely hovering: the wheel scrolls the panel, the slider stays.
    h.frame(vec![egui::Event::PointerMoved(rail)]);
    h.wheel(rail, 1.0);
    assert_eq!(h.value, 50.0);
    // A click on the handle (no jump) makes it take the wheel.
    h.time += 1.0;
    h.frame(vec![egui::Event::PointerMoved(handle)]);
    h.button(handle, true);
    h.button(handle, false);
    assert_eq!(h.value, 50.0);
    h.wheel(rail, 1.0);
    assert!((h.value - 51.0).abs() < 0.005, "{}", h.value);
    h.wheel(rail, -3.0);
    assert!((h.value - 48.0).abs() < 0.005, "{}", h.value);
    // A gesture that began elsewhere (scrolling the panel) passes over it unchanged.
    let elsewhere = egui::pos2(300.0, 200.0);
    h.time += 1.0;
    h.frame(vec![egui::Event::PointerMoved(elsewhere)]);
    h.wheel(elsewhere, 1.0);
    h.wheel(rail, 1.0);
    h.wheel(rail, 1.0);
    assert!((h.value - 48.0).abs() < 0.005, "{}", h.value);
    // A click elsewhere ends it: the wheel over the rail scrolls again.
    h.time += 1.0;
    h.frame(vec![egui::Event::PointerMoved(elsewhere)]);
    h.button(elsewhere, true);
    h.button(elsewhere, false);
    h.wheel(rail, 1.0);
    assert!((h.value - 48.0).abs() < 0.005, "{}", h.value);
}

#[test]
fn grid_rows_are_stepped_with_up_and_down() {
    let Some(mut f) = Fixture::new("grid-rows") else { return };
    let (a, b) = (f.photo("_DSC0009.dng"), f.photo("_DSC0010.dng"));
    // Wait until the background work is done (the neighbour prepared), as the other tests do.
    f.pump("Nachbar vorbereitet", |app| app.current.is_some() && app.is_prepared(&b));
    f.app.grid_view = true;
    // Two columns: both images in one row, so there is nothing above or below.
    f.app.grid_cols = 2;
    f.app.step_row(1, false);
    assert_eq!(f.app.current_path().as_ref(), Some(&a));
    f.app.step_row(-1, false);
    assert_eq!(f.app.current_path().as_ref(), Some(&a));
    // One column: down is the next image, nothing past the end, up goes back.
    f.app.grid_cols = 1;
    f.app.step_row(1, false);
    assert_eq!(f.app.primary_path().as_ref(), Some(&b));
    f.app.step_row(1, false);
    assert_eq!(f.app.primary_path().as_ref(), Some(&b));
    f.app.step_row(-1, false);
    assert_eq!(f.app.primary_path().as_ref(), Some(&a));
    // ↑/↓ belong to the grid only; in the single view they stay free.
    assert!((crate::commands::get("row-down").unwrap().enabled)(&f.app));
    f.app.grid_view = false;
    assert!(!(crate::commands::get("row-down").unwrap().enabled)(&f.app));
}

/// Arrow key and wheel steps of one control are one undo step; another change ends them first.
#[test]
fn key_and_wheel_steps_are_one_undo_step_apart_from_other_changes() {
    let Some(mut f) = Fixture::new("steps-undo") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    let undo = f.app.undo.len();
    let (contrast, saturation) = {
        let a = &f.app.current.as_ref().unwrap().adjustments;
        (a.contrast + 20.0, a.saturation)
    };
    // One frame at time `t`: `change`, then the end of the frame.
    let frame = |f: &mut Fixture, t: f64, change: &dyn Fn(&mut App, &egui::Context)| {
        let input = egui::RawInput { time: Some(t), ..Default::default() };
        let app = &mut f.app;
        let mut out = f.ctx.run_ui(input, |ui| {
            change(app, ui.ctx());
            app.finish_frame(ui.ctx());
        });
        out.textures_delta.clear();
    };
    let step = |id: &'static str| {
        move |a: &mut App, ctx: &egui::Context| {
            a.current.as_mut().unwrap().adjustments.exposure += 0.1;
            crate::widgets::mark_stepped(ctx, egui::Id::new(id));
        }
    };
    let nothing = |_: &mut App, _: &egui::Context| {};
    for t in [10.0, 10.1, 10.2] {
        frame(&mut f, t, &step("a"));
    }
    assert_eq!(f.app.undo.len(), undo, "Schritte laufen noch");
    // An action: the steps are one undo step, the action another.
    frame(&mut f, 10.3, &|a: &mut App, _: &egui::Context| a.edit(|adj| adj.contrast = contrast));
    assert_eq!(f.app.undo.len(), undo + 2);
    // A change made in a panel (not via `edit`) ends them too.
    frame(&mut f, 10.4, &step("a"));
    frame(&mut f, 10.5, &|a: &mut App, _: &egui::Context| a.current.as_mut().unwrap().adjustments.saturation += 10.0);
    assert_eq!(f.app.undo.len(), undo + 4);
    // Steps of another control start new steps.
    frame(&mut f, 10.6, &step("a"));
    frame(&mut f, 10.7, &step("b"));
    assert_eq!(f.app.undo.len(), undo + 5);
    // After the pause they are committed.
    frame(&mut f, 12.0, &nothing);
    assert_eq!(f.app.undo.len(), undo + 6);
    f.app.undo();
    f.app.undo();
    f.app.undo();
    assert_eq!(f.app.current.as_ref().unwrap().adjustments.saturation, saturation, "die Sättigung ist ein eigener Schritt");
    assert_eq!(f.app.current.as_ref().unwrap().adjustments.contrast, contrast);
}

#[test]
fn shift_with_the_arrow_keys_extends_the_selection() {
    let Some(mut f) = Fixture::new("shift-step") else { return };
    let (a, b) = (f.photo("_DSC0009.dng"), f.photo("_DSC0010.dng"));
    f.pump("Nachbar vorbereitet", |app| app.current.is_some() && app.is_prepared(&b));
    f.app.step(1, true);
    assert_eq!(f.app.primary_path().as_ref(), Some(&b));
    assert_eq!(f.app.selection, std::collections::HashSet::from([a.clone(), b.clone()]));
    // Without Shift the selection is the one image again.
    f.app.step(-1, false);
    assert_eq!(f.app.selection, std::collections::HashSet::from([a]));
}

#[test]
fn esc_cancels_the_crop_but_another_tool_keeps_it() {
    let Some(mut f) = Fixture::new("crop-cancel") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    let crop = |a: &App| a.current.as_ref().unwrap().adjustments.geometry.crop;
    let before = crop(&f.app);
    let smaller = sk_core::params::CropRect { x: 0.1, y: 0.1, w: 0.5, h: 0.5 };
    // Esc: back to the crop from before the tool.
    f.app.start_crop_tool();
    f.app.current.as_mut().unwrap().adjustments.geometry.crop = smaller;
    f.app.cancel_viewer_tools();
    assert!(f.app.crop_tool.is_none());
    assert_eq!(crop(&f.app), before);
    // Another tool: the crop stays.
    f.app.start_crop_tool();
    f.app.current.as_mut().unwrap().adjustments.geometry.crop = smaller;
    f.app.toggle_wb_picker();
    assert!(f.app.crop_tool.is_none() && f.app.picking_wb);
    assert_eq!(crop(&f.app), smaller);
}

/// Switching from the grid to the single image shows the filmstrip at the selected image,
/// even when it lies far outside the strip's visible part.
#[test]
fn filmstrip_starts_at_the_image_from_the_grid() {
    let Some(mut f) = Fixture::new("filmstrip_scroll") else { return };
    let photos = f.dir.join("photos");
    let source = f.photo("_DSC0009.dng");
    for i in 0..30 {
        let target = photos.join(format!("_KOP{i:04}.dng"));
        if std::fs::hard_link(&source, &target).is_err() {
            std::fs::copy(&source, &target).unwrap();
        }
    }
    f.open_folder(photos);
    assert!(f.app.visible.len() >= 32);
    let mut frame = eframe::Frame::_new_kittest();
    let mut t = 10.0;
    f.app.splash = None;
    f.app.grid_view = true;
    window_frame(&mut f, &mut frame, t, vec![]);
    let (first, last) = (f.app.entries[f.app.visible[0]].path.clone(), f.app.entries[*f.app.visible.last().unwrap()].path.clone());
    f.app.select_path(&last, egui::Modifiers::NONE);
    window_frame(&mut f, &mut frame, t, vec![]);

    // The strip scrolls there animated, so give it a second.
    f.app.grid_view = false;
    for _ in 0..10 {
        t += 0.1;
        window_frame(&mut f, &mut frame, t, vec![]);
    }
    let pass = f.ctx.cumulative_pass_nr();
    window_frame(&mut f, &mut frame, t + 0.1, vec![]);
    assert!(f.app.thumb_seen.get(&last).is_some_and(|&p| p >= pass), "Das gewählte Bild sollte im Filmstreifen zu sehen sein");
    assert!(f.app.thumb_seen.get(&first).is_none_or(|&p| p < pass), "Der Filmstreifen sollte zum Bild gescrollt sein");
}

/// Changing to another tab ends the layer or retouch tool of the old one.
#[test]
fn another_tab_ends_the_retouch_tool() {
    use crate::app::Tab;
    use crate::layers::LocalTool;
    let Some(mut f) = Fixture::new("tool_tab") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    let mut frame = eframe::Frame::_new_kittest();
    f.app.splash = None;
    f.app.tab = Tab::Retouch;
    window_frame(&mut f, &mut frame, 10.0, vec![]);
    f.app.set_local_tool(LocalTool::Heal);
    window_frame(&mut f, &mut frame, 10.1, vec![]);
    assert_eq!(f.app.local_tool, Some(LocalTool::Heal), "Im eigenen Bereich bleibt das Werkzeug aktiv");
    f.app.tab = Tab::Color;
    window_frame(&mut f, &mut frame, 10.2, vec![]);
    assert_eq!(f.app.local_tool, None, "Im Farbe-Bereich ist Reparieren aus");
    // A command that opens a tool's tab and the tool together keeps it.
    f.app.tab = Tab::Layers;
    f.app.set_local_tool(LocalTool::Brush);
    window_frame(&mut f, &mut frame, 10.3, vec![]);
    assert_eq!(f.app.local_tool, Some(LocalTool::Brush));
}

/// Holding the space bar shows the move cursor over a mask brush, also while a control (such
/// as the brush size slider just used) has the keyboard focus.
#[test]
fn space_bar_pans_with_a_focused_control() {
    use crate::layers::LocalTool;
    let Some(mut f) = Fixture::new("space_focus") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    let mut frame = eframe::Frame::_new_kittest();
    f.app.splash = None;
    f.app.tab = crate::app::Tab::Layers;
    f.app.set_local_tool(LocalTool::Brush);
    let p = egui::pos2(800.0, 400.0);
    let key = |key, pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Default::default() };
    let mut cursor = |f: &mut Fixture, i: usize, events| {
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1500.0, 950.0))), time: Some(10.0 + i as f64 * 0.1), events, ..Default::default() };
        let app = &mut f.app;
        let out = f.ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
        let mut delta = out.textures_delta;
        delta.clear();
        out.platform_output.cursor_icon
    };
    cursor(&mut f, 0, vec![egui::Event::PointerMoved(p)]);
    // Tab gives a control the keyboard focus, as a click on a slider does.
    cursor(&mut f, 1, vec![key(egui::Key::Tab, true), key(egui::Key::Tab, false)]);
    cursor(&mut f, 2, vec![]);
    assert!(f.ctx.memory(|m| m.focused()).is_some(), "Ein Bedienelement sollte den Fokus haben");
    assert_eq!(cursor(&mut f, 3, vec![key(egui::Key::Space, true)]), egui::CursorIcon::Grab);
    assert_eq!(cursor(&mut f, 4, vec![]), egui::CursorIcon::Grab);
    assert_eq!(f.app.local_tool, Some(LocalTool::Brush));
    assert_eq!(cursor(&mut f, 5, vec![key(egui::Key::Space, false)]), egui::CursorIcon::None, "Danach wieder der Pinselkreis");
}

/// With the viewer on a second screen the main window starts with the grid, but can show the
/// single image (with filmstrip) too; the viewer window keeps zoom and tools.
#[test]
fn main_window_switches_to_the_image_with_the_viewer_window_open() {
    let Some(mut f) = Fixture::new("viewer_window_grid") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    let mut frame = eframe::Frame::_new_kittest();
    f.app.splash = None;
    f.app.grid_view = false;
    f.app.toggle_viewer_window();
    assert!(f.app.viewer_detached());
    assert!(f.app.grid_view, "Beim Öffnen des Bildfensters zeigt das Hauptfenster das Raster");
    window_frame(&mut f, &mut frame, 10.0, vec![]);
    assert!(!f.app.filmstrip_shown);
    f.app.grid_view = false;
    window_frame(&mut f, &mut frame, 10.1, vec![]);
    assert!(f.app.viewer_detached(), "Das Bildfenster bleibt offen");
    assert!(f.app.filmstrip_shown, "Im Einzelbild zeigt das Hauptfenster den Filmstreifen");
}

#[test]
fn auto_adjust_current_image_and_selection() {
    let Some(mut f) = Fixture::new("auto") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    f.pump("Indexiert", |a| a.entries.len() == 2);
    let a = f.photo("_DSC0009.dng");
    let b = f.photo("_DSC0010.dng");
    f.app.edit(|adj| adj.saturation = 15.0);
    f.app.commit_and_save();
    let before = f.app.current.as_ref().unwrap().adjustments.clone();

    // The open image: one undo step, other settings stay.
    f.app.auto_adjust_selection();
    assert!(f.app.auto_progress.is_some());
    f.pump("angepasst", |app| app.auto_progress.is_none());
    let adj = f.app.current.as_ref().unwrap().adjustments.clone();
    assert_ne!(adj, before, "{}", f.app.status);
    assert_eq!(adj.white_balance.mode, sk_core::params::WbMode::Custom);
    assert_eq!(adj.saturation, 15.0);
    // Half the strength: half way, as one more undo step; full strength again restores it.
    let mix = f.app.auto_mix.as_ref().expect("Stärke");
    let (opts, mix_before, mix_auto) = (mix.opts, mix.before.clone(), mix.auto.clone());
    assert_eq!(mix_auto, adj);
    let half = opts.blend(&adj, &mix_before, &mix_auto, 0.5, f.app.current.as_ref().unwrap().as_shot_temp_tint);
    assert!((half.exposure - (before.exposure + adj.exposure) / 2.0).abs() < 0.011, "{} {}", half.exposure, adj.exposure);
    assert_eq!(opts.blend(&adj, &mix_before, &mix_auto, 1.0, (0.0, 0.0)), adj);
    assert_eq!(opts.blend(&adj, &mix_before, &mix_auto, 0.0, (0.0, 0.0)).exposure, before.exposure);
    f.app.undo();
    assert_eq!(f.app.current.as_ref().unwrap().adjustments, before);

    // Only the exposure, on both selected images: the other one is saved to its sidecar.
    f.app.auto_options = sk_core::auto::AutoOptions { exposure: true, ..sk_core::auto::AutoOptions::NONE };
    f.app.select_path(&b, egui::Modifiers::COMMAND);
    assert_eq!(f.app.selected_paths().len(), 2);
    f.app.auto_adjust_selection();
    f.pump("beide angepasst", |app| app.auto_progress.is_none());
    assert!(f.app.status.contains("2 Bilder"), "{}", f.app.status);
    assert_eq!(f.app.current_path().as_ref(), Some(&a));
    let other = sidecar::load(&b).adjustments;
    assert_ne!(other.exposure, 0.0);
    assert_eq!((other.white_balance.mode, other.contrast), (sk_core::params::WbMode::AsShot, 0.0));
}

/// The "A" in a tool header sets only that tool, on all selected images; on a layer it is
/// disabled.
#[test]
fn tool_auto_sets_only_its_tool_on_the_selection() {
    let Some(mut f) = Fixture::new("tool-auto") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    f.pump("Indexiert", |a| a.entries.len() == 2);
    let b = f.photo("_DSC0010.dng");
    f.app.tab = crate::app::Tab::Exposure;
    let adj = |f: &Fixture| f.app.current.as_ref().unwrap().adjustments.clone();
    let before = adj(&f);

    // The topmost "A" of the exposure tab is the one of "Belichtung".
    let nodes = tool_panel_frame(&mut f, vec![]);
    let a = nodes.iter().filter(|(l, _)| l == "A").map(|(_, r)| *r).min_by(|a, b| a.top().total_cmp(&b.top())).expect("A im Kopf");
    click_at(&mut f, a.center());
    assert!(f.app.auto_progress.is_some(), "{}", f.app.status);
    f.pump("Belichtung angepasst", |app| app.auto_progress.is_none());
    let after = adj(&f);
    assert_ne!((after.exposure, after.contrast), (before.exposure, before.contrast), "{}", f.app.status);
    assert_eq!(sk_core::params::Adjustments { exposure: before.exposure, contrast: before.contrast, ..after.clone() }, before);
    assert_eq!(f.app.auto_mix.as_ref().map(|m| m.opts.exposure), Some(true), "Stärke auch nach einem A");

    // Levels on both selected images: only the levels change, the other one's in its sidecar.
    f.app.select_path(&b, egui::Modifiers::COMMAND);
    f.app.auto_tool(sk_core::tools::Tool::Levels);
    f.pump("Tonwerte angepasst", |app| app.auto_progress.is_none());
    assert!(f.app.status.contains("2 Bilder"), "{}", f.app.status);
    assert_eq!(sk_core::params::Adjustments { levels: after.levels.clone(), ..adj(&f) }, after);
    let other = sidecar::load(&b).adjustments;
    assert_eq!((other.exposure, other.contrast, other.white_balance.mode), (0.0, 0.0, sk_core::params::WbMode::AsShot));

    // On a layer the "A" does nothing (and its command is off).
    f.app.edit(|a| a.layers = vec![sk_core::params::Layer { name: "Himmel".into(), ..Default::default() }]);
    f.app.select_layer(Some(0));
    let nodes = tool_panel_frame(&mut f, vec![]);
    let a = nodes.iter().filter(|(l, _)| l == "A").map(|(_, r)| *r).min_by(|a, b| a.top().total_cmp(&b.top())).expect("A im Kopf");
    click_at(&mut f, a.center());
    assert!(f.app.auto_progress.is_none());
    assert!(!f.app.command_available("auto-exposure"));
}

/// Draws the tool panel (with AccessKit) and reports the labels of its widgets.
fn tool_panel_frame(f: &mut Fixture, events: Vec<egui::Event>) -> Vec<(String, egui::Rect)> {
    tool_panel_frame_with(f, events, Default::default())
}

/// [`tool_panel_frame`] with modifier keys held.
fn tool_panel_frame_with(f: &mut Fixture, events: Vec<egui::Event>, modifiers: egui::Modifiers) -> Vec<(String, egui::Rect)> {
    let events = std::iter::once(egui::Event::ModifiersChanged(modifiers)).chain(events).collect();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1500.0, 2400.0))),
        events,
        // Frames a quarter second apart: pressing and releasing is a click, two clicks on the
        // same slider are no double click (which resets).
        time: Some(f.ctx.input(|i| i.time) + 0.25),
        ..Default::default()
    };
    f.ctx.enable_accesskit();
    let mut out = f.ctx.run_ui(input, |ui| {
        egui::Panel::right("tools").exact_size(330.0).show(ui, |ui| f.app.tool_panel(ui));
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

/// The topmost slider with this label (AccessKit names sliders after their label).
fn slider(nodes: &[(String, egui::Rect)], label: &str) -> egui::Rect {
    nodes.iter().filter(|(l, _)| l == label).map(|(_, r)| *r).min_by(|a, b| a.top().total_cmp(&b.top())).unwrap_or_else(|| panic!("Regler {label} fehlt"))
}

/// Clicks at 80 % of a slider's rail.
fn click_slider(f: &mut Fixture, rail: egui::Rect) {
    click_at(f, egui::pos2(rail.left() + rail.width() * 0.8, rail.center().y));
}

fn click_at(f: &mut Fixture, pos: egui::Pos2) -> Vec<(String, egui::Rect)> {
    let button = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    tool_panel_frame(f, vec![egui::Event::PointerMoved(pos)]);
    tool_panel_frame(f, vec![button(true)]);
    tool_panel_frame(f, vec![button(false)]);
    tool_panel_frame(f, vec![])
}

/// With a layer chosen, the develop tabs change the layer and say so; tools without an effect on
/// layers stay visible but do nothing.
#[test]
fn develop_tools_change_the_chosen_layer() {
    let Some(mut f) = Fixture::new("layer-tools") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    f.app.edit(|a| a.layers = vec![sk_core::params::Layer { name: "Himmel".into(), ..Default::default() }]);
    f.app.tab = crate::app::Tab::Exposure;
    let adj = |f: &Fixture| f.app.current.as_ref().unwrap().adjustments.clone();

    // On the background: the bar offers the layers, the sliders change the image.
    let nodes = tool_panel_frame(&mut f, vec![]);
    assert!(widget(&nodes, "Hintergrund").is_some(), "Leiste zeigt den Hintergrund");
    click_slider(&mut f, slider(&nodes, "Belichtung"));
    assert!(adj(&f).exposure > 0.5, "Hintergrund: {}", adj(&f).exposure);
    assert_eq!(adj(&f).layers[0].adjustments.exposure, 0.0);

    // On the layer: the same slider changes the layer, the background stays.
    f.app.edit(|a| a.exposure = 0.0);
    f.app.select_layer(Some(0));
    let nodes = tool_panel_frame(&mut f, vec![]);
    assert!(widget(&nodes, "Ebene „Himmel“").is_some(), "Leiste zeigt die Ebene");
    click_slider(&mut f, slider(&nodes, "Belichtung"));
    assert_eq!(adj(&f).exposure, 0.0);
    assert!(adj(&f).layers[0].adjustments.exposure > 0.5, "Ebene: {}", adj(&f).layers[0].adjustments.exposure);

    // Levels only change the whole image: disabled and closed, so their handles are not there;
    // opened (closed by default), they are.
    assert!(widget(&nodes, "Eingang Schwarz").is_none(), "Tonwerte sind zu");
    f.app.select_layer(None);
    crate::widgets::reveal_section(&f.ctx, "Tonwerte");
    let nodes = tool_panel_frame(&mut f, vec![]);
    assert!(widget(&nodes, "Eingang Schwarz").is_some(), "Tonwerte wieder offen");

    // The layer below the lowest one is the background.
    f.app.select_layer(Some(0));
    f.app.step_layer(false);
    assert_eq!(f.app.active_layer, None);
}

/// On a layer, the tabs that only change the whole image are disabled, so are their commands;
/// going to such a tab does nothing.
#[test]
fn layer_mode_leaves_out_whole_image_tabs_and_commands() {
    use crate::app::Tab;
    let Some(mut f) = Fixture::new("layer-mode") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    f.app.edit(|a| a.layers = vec![sk_core::params::Layer { name: "Himmel".into(), ..Default::default() }]);
    let image_tabs = [Tab::Lens, Tab::Effects, Tab::Retouch, Tab::Styles];
    assert!(image_tabs.iter().all(|t| f.app.tab_available(*t)));
    let enabled = |f: &Fixture, id: &str| (crate::commands::COMMANDS.iter().find(|c| c.id == id).unwrap().enabled)(&f.app);
    assert!(enabled(&f, "crop") && enabled(&f, "tool-heal"));

    f.app.tab = Tab::Lens;
    f.app.step_layer(true);
    assert_eq!(f.app.active_layer, Some(0));
    for t in image_tabs {
        assert!(!f.app.tab_available(t), "{t:?}");
    }
    for t in [Tab::Exposure, Tab::Color, Tab::Details, Tab::Layers, Tab::Library, Tab::History, Tab::Output] {
        assert!(f.app.tab_available(t), "{t:?}");
    }
    for id in ["crop", "straighten", "rotate-left", "wb-picker", "tool-heal", "tool-clone", "tool-remove", "find-spots", "reset"] {
        assert!(!enabled(&f, id), "{id}");
    }
    // The open tab was one of them: the next frame shows the layer tab instead.
    let mut frame = eframe::Frame::_new_kittest();
    let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1500.0, 950.0))), ..Default::default() };
    let app = &mut f.app;
    let mut out = f.ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
    out.textures_delta.clear();
    assert_eq!(f.app.tab, Tab::Layers);
    assert!(!f.app.open_tab(Tab::Effects));
    assert_ne!(f.app.tab, Tab::Effects);
    assert!(f.app.status.contains("ganze Bild"));
    assert!(!f.app.section_available(Tab::Exposure, "Tonwerte"));
    assert!(f.app.section_available(Tab::Exposure, "Belichtung"));
    assert!(f.app.section_available(Tab::Exposure, "Gradationskurve"));
}

/// M shows the mask also after a click on a slider (which keeps the keyboard focus).
#[test]
fn mask_key_works_with_a_focused_slider() {
    let Some(mut f) = Fixture::new("mask-key") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    f.app.edit(|a| a.layers = vec![sk_core::params::Layer { name: "Himmel".into(), ..Default::default() }]);
    f.app.select_layer(Some(0));
    f.app.tab = crate::app::Tab::Exposure;
    let nodes = tool_panel_frame(&mut f, vec![]);
    click_slider(&mut f, slider(&nodes, "Belichtung"));
    assert!(crate::widgets::slider_has_focus(&f.ctx), "Regler hat den Fokus");
    let key = |pressed| egui::Event::Key { key: egui::Key::M, physical_key: None, pressed, repeat: false, modifiers: Default::default() };
    let input = egui::RawInput { events: vec![key(true), key(false)], time: Some(f.ctx.input(|i| i.time) + 1.0), ..Default::default() };
    let app = &mut f.app;
    let mut out = f.ctx.run_ui(input, |ui| app.shortcuts(ui.ctx()));
    out.textures_delta.clear();
    assert!(f.app.show_mask);
}

#[test]
fn every_tool_says_whether_it_works_on_layers() {
    use sk_core::tools::{LayerSupport, Tool};
    // The tools whose sliders the develop tabs send to a layer.
    for t in Tool::ALL {
        let on_layer = matches!(
            t,
            Tool::Exposure
                | Tool::HighlightsShadows
                | Tool::Curves
                | Tool::WhiteBalance
                | Tool::Color
                | Tool::ColorEditor
                | Tool::ColorBalance
                | Tool::Clarity
                | Tool::Dehaze
        );
        assert_eq!(t.layer_support() != LayerSupport::None, on_layer, "{t:?}");
    }
}

/// "Neue Ebene" in the bar above the tools goes on to the mask: the new layer changes nothing
/// before it has one.
#[test]
fn new_layer_from_the_bar_goes_to_the_mask() {
    let Some(mut f) = Fixture::new("bar-new-layer") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    f.app.tab = crate::app::Tab::Exposure;
    let nodes = tool_panel_frame(&mut f, vec![]);
    let bar = widget(&nodes, "Hintergrund").expect("Leiste");
    let nodes = click_at(&mut f, bar.center());
    let add = widget(&nodes, "Neue Ebene").expect("Menüeintrag „Neue Ebene“");
    click_at(&mut f, add.center());
    assert_eq!(f.app.current.as_ref().unwrap().adjustments.layers.len(), 1);
    assert_eq!(f.app.active_layer, Some(0));
    assert_eq!(f.app.tab, crate::app::Tab::Layers);
}

/// A long lens profile name does not widen the tool panel; only dragging its edge does.
#[test]
fn long_lens_profile_keeps_the_tool_panel_width() {
    let Some(mut f) = Fixture::new("lens_width") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    let mut frame = eframe::Frame::_new_kittest();
    f.app.splash = None;
    f.app.grid_view = false;
    f.app.tab = crate::app::Tab::Lens;
    let width = |f: &Fixture| egui::containers::panel::PanelState::load(&f.ctx, egui::Id::new("tools")).expect("Werkzeugpanel").size().x;
    window_frame(&mut f, &mut frame, 10.0, vec![]);
    let before = width(&f);
    f.app.current.as_mut().unwrap().adjustments.lens.profile = Some("Sehr langer Objektivname ".repeat(8));
    for i in 1..4 {
        window_frame(&mut f, &mut frame, 10.0 + i as f64 * 0.1, vec![]);
    }
    assert_eq!(width(&f), before);
}

#[test]
fn watched_folder_shows_new_pictures() {
    let Some(mut f) = Fixture::new("hot-folder") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    f.app.hot_folder.on = true;
    let photos = f.dir.join("photos");
    // An empty file (the camera software just started writing) is not opened.
    std::fs::write(photos.join("_DSC0012.dng"), b"").unwrap();
    // A new picture: listed and opened.
    std::fs::copy(f.photo("_DSC0010.dng"), photos.join("_DSC0011.dng")).unwrap();
    let new = photos.join("_DSC0011.dng");
    f.pump("Neue Aufnahme geöffnet", |a| a.current_path().is_some_and(|p| p.ends_with("_DSC0011.dng")));
    assert!(f.app.index_of(&new).is_some() || f.app.index_of(&new.canonicalize().unwrap()).is_some());
    // Without opening: listed, the open picture stays.
    f.app.hot_folder.open_new = false;
    std::fs::copy(f.photo("_DSC0009.dng"), photos.join("_DSC0013.dng")).unwrap();
    f.pump("Zweite Aufnahme gelistet", |a| a.entries.iter().any(|e| e.path.ends_with("_DSC0013.dng")));
    assert!(f.app.current_path().is_some_and(|p| p.ends_with("_DSC0011.dng")));
}

/// Commands that could do nothing right now are unavailable, not silently ignored.
#[test]
fn commands_without_effect_are_unavailable() {
    let Some(mut f) = Fixture::new("unavailable") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    let enabled = |f: &Fixture, id: &str| f.app.command_available(id);
    // Exporting needs a recipe switched on, and runs once at a time.
    assert!(enabled(&f, "export"));
    for r in &mut f.app.recipes {
        r.enabled = false;
    }
    assert!(!enabled(&f, "export"));
    assert!(f.app.export_blocked_reason().contains("Ausgaberezept"));
    // Entf only with a spot or gradient to delete.
    assert!(!enabled(&f, "delete-spot"));
    f.app.selected_spot = Some(0);
    assert!(enabled(&f, "delete-spot"));
    // The trash is a session folder.
    assert!(!enabled(&f, "trash"));
    let a = f.photo("_DSC0009.dng");
    f.app.create_session(&f.dir, "Shooting").unwrap();
    f.app.selection = [a].into_iter().collect();
    assert!(enabled(&f, "trash"));
    // In the grid Entf trashes the selection like Strg+Entf; in the viewer it needs a spot.
    f.app.selected_spot = None;
    f.app.grid_view = false;
    assert!(!enabled(&f, "delete-spot"));
    f.app.grid_view = true;
    assert!(enabled(&f, "delete-spot"));
    f.app.grid_view = false;
    // Full screen shows the image: without one it can only be left.
    f.app.selection.clear();
    f.app.close_current();
    assert!(!enabled(&f, "fullscreen"));
    f.app.fullscreen = true;
    assert!(enabled(&f, "fullscreen"));
}

/// The about card takes the keyboard like a dialog: no shortcuts and no palette behind it.
#[test]
fn about_card_blocks_shortcuts() {
    let Some(mut f) = Fixture::new("about-keys") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    f.app.splash = Some(crate::splash::Splash::about());
    let grid = f.app.grid_view;
    let key = |key: egui::Key, modifiers: egui::Modifiers| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
    let input = egui::RawInput { events: vec![key(egui::Key::K, egui::Modifiers::COMMAND), key(egui::Key::G, egui::Modifiers::NONE)], ..Default::default() };
    let app = &mut f.app;
    let mut out = f.ctx.run_ui(input, |ui| app.shortcuts(ui.ctx()));
    out.textures_delta.clear();
    assert!(!f.app.palette.open);
    assert_eq!(f.app.grid_view, grid);
}

/// Esc ends a tool first, then the views (before/after, split, output preview, 100 %, but not
/// the softproof), then full screen.
#[test]
fn escape_leaves_tool_then_view_then_fullscreen() {
    let Some(mut f) = Fixture::new("esc-views") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    let esc = |f: &mut Fixture| {
        let input = egui::RawInput {
            events: vec![egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }],
            ..Default::default()
        };
        let app = &mut f.app;
        let mut out = f.ctx.run_ui(input, |ui| app.shortcuts(ui.ctx()));
        out.textures_delta.clear();
    };
    f.app.fullscreen = true;
    f.app.toggle_split();
    f.app.proof_view = true;
    f.app.zoom = Some(2.0);
    f.app.softproof.on = true;
    f.app.picking_wb = true;
    esc(&mut f);
    assert!(!f.app.picking_wb);
    assert!(f.app.split_view && f.app.proof_view && f.app.zoom.is_some(), "erst nur das Werkzeug");
    esc(&mut f);
    assert!(!f.app.split_view && !f.app.proof_view && f.app.zoom.is_none());
    assert!(f.app.softproof.on && f.app.fullscreen);
    f.app.show_before = true;
    esc(&mut f);
    assert!(!f.app.show_before && f.app.fullscreen);
    esc(&mut f);
    assert!(!f.app.fullscreen);
}

/// However often the crop frame is adjusted, the crop is one undo step.
#[test]
fn a_crop_is_one_undo_step() {
    let Some(mut f) = Fixture::new("crop-undo") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    let frame = |f: &mut Fixture| {
        let app = &mut f.app;
        let mut out = f.ctx.run_ui(Default::default(), |ui| app.finish_frame(ui.ctx()));
        out.textures_delta.clear();
    };
    frame(&mut f);
    let steps = f.app.undo.len();
    f.app.start_crop_tool();
    for w in [0.9, 0.8, 0.7] {
        f.app.current.as_mut().unwrap().adjustments.geometry.crop = sk_core::params::CropRect { x: 0.0, y: 0.0, w, h: w };
        frame(&mut f);
    }
    assert_eq!(f.app.undo.len(), steps, "Während des Zuschneidens kein Schritt");
    f.app.finish_crop_tool();
    frame(&mut f);
    assert_eq!(f.app.undo.len(), steps + 1);
    f.app.undo();
    assert_eq!(f.app.current.as_ref().unwrap().adjustments.geometry.crop, sk_core::params::CropRect::default());
}

/// Cropping: Alt at a corner keeps the middle, Shift keeps free proportions.
#[test]
fn crop_corner_with_alt_keeps_the_middle_and_shift_the_proportions() {
    use sk_core::params::CropRect;
    let Some(mut f) = Fixture::new("crop-keys") else { return };
    f.pump("Bild gerendert", |a| a.current.is_some() && a.texture.is_some());
    f.app.start_crop_tool();
    let start = CropRect { x: 0.25, y: 0.25, w: 0.5, h: 0.4 };
    let drag_corner = |f: &mut Fixture, m: egui::Modifiers, by: egui::Vec2| {
        f.app.current.as_mut().unwrap().adjustments.geometry.crop = start;
        viewer_frame(f, vec![]);
        let (_, img) = f.app.view_geometry.unwrap();
        let corner = img.min + egui::vec2(img.width() * (start.x + start.w), img.height() * (start.y + start.h));
        let button = |p, pressed| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: m };
        viewer_frame_with(f, vec![egui::Event::PointerMoved(corner)], m);
        viewer_frame_with(f, vec![button(corner, true)], m);
        for i in 1..=5 {
            viewer_frame_with(f, vec![egui::Event::PointerMoved(corner + by * (i as f32 / 5.0))], m);
        }
        viewer_frame_with(f, vec![button(corner + by, false)], m);
        f.app.current.as_ref().unwrap().adjustments.geometry.crop
    };
    let c = drag_corner(&mut f, egui::Modifiers::ALT, egui::vec2(40.0, 25.0));
    assert!((c.x + c.w / 2.0 - 0.5).abs() < 0.005 && (c.y + c.h / 2.0 - 0.45).abs() < 0.005, "Mitte bleibt: {c:?}");
    assert!(c.w > start.w && c.x < start.x, "{c:?}");
    let c = drag_corner(&mut f, egui::Modifiers::SHIFT, egui::vec2(60.0, 0.0));
    assert!((c.w / c.h - start.w / start.h).abs() < 0.01, "Verhältnis bleibt: {c:?}");
    assert!((c.x - start.x).abs() < 1e-4 && (c.y - start.y).abs() < 1e-4, "obere linke Ecke bleibt: {c:?}");
    let c = drag_corner(&mut f, egui::Modifiers::NONE, egui::vec2(60.0, 0.0));
    assert!((c.h - start.h).abs() < 1e-4 && c.w > start.w, "ohne Taste frei: {c:?}");
}

/// Klonen: an Alt-click puts the source of the next area there, and of a chosen one.
#[test]
fn alt_click_sets_the_clone_source() {
    let Some(mut f) = Fixture::new("clone-source") else { return };
    f.pump("Bild gerendert", |a| a.current.is_some() && a.texture.is_some());
    f.app.set_local_tool(crate::layers::LocalTool::Clone);
    let click = |f: &mut Fixture, p: egui::Pos2, m: egui::Modifiers| {
        let button = |pressed| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: m };
        viewer_frame_with(f, vec![egui::Event::PointerMoved(p)], m);
        viewer_frame_with(f, vec![button(true)], m);
        viewer_frame_with(f, vec![button(false)], m);
        viewer_frame_with(f, vec![], egui::Modifiers::NONE);
    };
    let (src, dst, other) = (egui::pos2(450.0, 350.0), egui::pos2(700.0, 450.0), egui::pos2(400.0, 500.0));
    click(&mut f, src, egui::Modifiers::ALT);
    let picked = f.app.next_source.expect("Quelle vorgemerkt");
    assert!(f.app.current.as_ref().unwrap().adjustments.retouch.is_empty(), "kein Bereich");
    click(&mut f, dst, egui::Modifiers::NONE);
    let spots = f.app.current.as_ref().unwrap().adjustments.retouch.clone();
    assert_eq!(spots.len(), 1);
    assert_eq!(spots[0].source, picked);
    assert_eq!(f.app.next_source, None, "nur für den nächsten Bereich");
    // The area is chosen now: an Alt-click moves its source.
    click(&mut f, other, egui::Modifiers::ALT);
    let spots = f.app.current.as_ref().unwrap().adjustments.retouch.clone();
    assert_eq!(spots.len(), 1, "kein neuer Bereich");
    assert_ne!(spots[0].source, picked, "Quelle versetzt");
}

/// Alt-click on a layer's eye: only that layer shows; once more, all again.
#[test]
fn alt_click_on_an_eye_shows_only_that_layer() {
    use crate::layers::LayerAction;
    let Some(mut f) = Fixture::new("layer-solo") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    f.app.edit(|a| a.layers = (0..3).map(|i| sk_core::params::Layer { name: format!("{i}"), enabled: i != 2, ..Default::default() }).collect());
    let shown = |f: &Fixture| f.app.current.as_ref().unwrap().adjustments.layers.iter().map(|l| l.enabled).collect::<Vec<_>>();
    f.app.apply_layer_action(Some(LayerAction::Solo(2)));
    assert_eq!(shown(&f), [false, false, true]);
    f.app.apply_layer_action(Some(LayerAction::Solo(2)));
    assert_eq!(shown(&f), [true, true, true]);
}

/// Zoomed in, the image can be moved up to its edges, not out of sight; zoomed far out, it stays
/// wholly in view.
#[test]
fn the_image_cannot_be_pushed_out_of_view() {
    use crate::browser::clamp_pan;
    let (image, view) = (egui::vec2(3000.0, 2000.0), egui::vec2(1000.0, 800.0));
    assert_eq!(clamp_pan(egui::vec2(5000.0, -5000.0), image, view, 20.0), egui::vec2(1020.0, -620.0));
    assert_eq!(clamp_pan(egui::vec2(-300.0, 0.0), image, view, 20.0), egui::vec2(-300.0, 0.0), "innerhalb frei");
    assert_eq!(clamp_pan(egui::vec2(900.0, 900.0), egui::vec2(200.0, 100.0), view, 20.0), egui::vec2(400.0, 350.0));

    let Some(mut f) = Fixture::new("pan-limit") else { return };
    f.pump("Bild gerendert", |a| a.current.is_some() && a.texture.is_some());
    f.app.set_zoom_pct(100.0);
    viewer_frame(&mut f, vec![]);
    drag(&mut f, egui::pos2(600.0, 400.0), egui::pos2(3600.0, 2900.0), 10);
    let (view, img) = f.app.view_geometry.unwrap();
    assert!(img.left() <= view.left() + 20.5 && img.top() <= view.top() + 20.5, "Bild bleibt im Blick: {view:?} {img:?}");
    assert!(img.right() > view.right() && img.bottom() > view.bottom());
}

/// Every tool opens the tab with its settings, however it is started.
#[test]
fn tools_open_their_tab() {
    use crate::app::Tab;
    let Some(mut f) = Fixture::new("tool-tabs") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    for (id, tab) in [("crop", Tab::Lens), ("straighten", Tab::Lens), ("wb-picker", Tab::Color)] {
        f.app.end_viewer_tools();
        f.app.tab = Tab::Library;
        f.app.run_command_id(&f.ctx.clone(), id);
        assert_eq!(f.app.tab, tab, "{id}");
    }
}

/// What can't be undone asks first: deleting a snapshot only happens after confirming.
#[test]
fn deleting_a_snapshot_asks_first() {
    let Some(mut f) = Fixture::new("delete-confirm") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    f.app.add_snapshot(Some("Erster Stand".into()));
    let count = |a: &App| a.current.as_ref().unwrap().snapshots.len();
    assert_eq!(count(&f.app), 1);
    f.app.request_snapshot_deletion(0);
    assert_eq!(count(&f.app), 1, "erst nach der Rückfrage");
    assert!(matches!(f.app.pending_deletion, Some(crate::deletion::Deletion::Snapshot(_, 0, _))));
    // Confirmed for another image (or after the list changed): nothing is deleted.
    let path = f.app.current_path().unwrap();
    f.app.delete_snapshot_of(&path.with_file_name("anderes.jpg"), 0, "Erster Stand");
    f.app.delete_snapshot_of(&path, 0, "Anderer Name");
    assert_eq!(count(&f.app), 1, "nicht das Bild oder der Schnappschuss, nach dem gefragt wurde");
    f.app.delete_snapshot_of(&path, 0, "Erster Stand");
    assert_eq!(count(&f.app), 0);
}

/// "+ Schnappschuss" saves the state and offers its name for editing right away, in the side
/// column as in the history tab (both draw the same list).
#[test]
fn a_new_snapshot_can_be_named_right_away() {
    let Some(mut f) = Fixture::new("snapshot-name") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    f.app.add_snapshot_to_name();
    f.app.add_snapshot_to_name();
    assert_eq!(f.app.snapshot_rename, Some((1, "Schnappschuss 2".to_string())));
}

/// Tools in the viewer end the same way when their panel goes: the crop stays as it was set.
#[test]
fn another_tab_ends_the_crop_and_keeps_it() {
    use crate::app::Tab;
    let Some(mut f) = Fixture::new("tab-ends-crop") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    let smaller = sk_core::params::CropRect { x: 0.1, y: 0.1, w: 0.5, h: 0.5 };
    f.app.start_crop_tool();
    f.app.current.as_mut().unwrap().adjustments.geometry.crop = smaller;
    f.app.tab = Tab::Color;
    let mut frame = eframe::Frame::_new_kittest();
    let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1500.0, 950.0))), ..Default::default() };
    let app = &mut f.app;
    let mut out = f.ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
    out.textures_delta.clear();
    assert!(f.app.crop_tool.is_none());
    assert_eq!(f.app.current.as_ref().unwrap().adjustments.geometry.crop, smaller);
}

/// Entf deletes what is chosen: in the layer tab the chosen layer (undoable).
#[test]
fn delete_key_deletes_the_chosen_layer() {
    use crate::app::Tab;
    let Some(mut f) = Fixture::new("delete-layer") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    f.app.edit(|a| a.layers = vec![sk_core::params::Layer { name: "Himmel".into(), ..Default::default() }]);
    f.app.commit_and_save();
    f.app.active_layer = Some(0);
    f.app.tab = Tab::Exposure;
    assert!(!f.app.command_available("delete-spot"), "nur im Ebenen-Tab");
    f.app.tab = Tab::Layers;
    assert!(f.app.command_available("delete-spot"));
    f.app.run_command_id(&f.ctx.clone(), "delete-spot");
    assert!(f.app.current.as_ref().unwrap().adjustments.layers.is_empty());
    assert_eq!(f.app.active_layer, None);
    f.app.undo();
    assert_eq!(f.app.current.as_ref().unwrap().adjustments.layers.len(), 1);
}

/// Batch work can be cancelled from the status bar: it ends at once, what was done stays.
#[test]
fn batch_work_can_be_cancelled() {
    let Some(mut f) = Fixture::new("cancel-batch") else { return };
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    f.app.auto_progress = Some((2, 5));
    f.app.auto_cancel = Some(cancel.clone());
    let activity = f.app.activities().into_iter().find(|a| a.label == "Automatisch anpassen").unwrap();
    assert!(activity.cancel.is_some(), "mit ✕ in der Statusleiste");
    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    f.app.poll_cancelled();
    assert!(f.app.auto_progress.is_none());
    assert!(f.app.status.contains("abgebrochen (2 von 5 fertig)"), "{}", f.app.status);
}

/// Errors are remembered for the status bar (red, and visible for a while after them).
#[test]
fn errors_are_reported_and_remembered() {
    let Some(mut f) = Fixture::new("report-error") else { return };
    f.app.report_error("Speichern fehlgeschlagen: kein Platz");
    assert_eq!(f.app.status, "Speichern fehlgeschlagen: kein Platz");
    f.app.status = "Nächste Meldung".into();
    assert_eq!(f.app.last_error.as_ref().map(|e| e.0.as_str()), Some("Speichern fehlgeschlagen: kein Platz"));
}

/// A change to several images is one undo step: Ctrl+Z restores all of them, also those not
/// open; Ctrl+Y brings it back.
#[test]
fn undo_takes_back_a_change_to_several_images() {
    let Some(mut f) = Fixture::new("undo-batch") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    let (a, b) = (f.photo("_DSC0009.dng"), f.photo("_DSC0010.dng"));
    let exposure = |f: &Fixture, p: &PathBuf| match f.app.current_path().as_ref() == Some(p) {
        true => f.app.current.as_ref().unwrap().adjustments.exposure,
        false => sidecar::load(p).adjustments.exposure,
    };
    f.app.set_adjustments_on(&[a.clone(), b.clone()], |adj| sk_core::params::Adjustments { exposure: 1.5, ..adj.clone() });
    assert_eq!((exposure(&f, &a), exposure(&f, &b)), (1.5, 1.5));
    f.app.undo();
    assert_eq!((exposure(&f, &a), exposure(&f, &b)), (0.0, 0.0), "ein Schritt für beide Bilder");
    f.app.redo();
    assert_eq!((exposure(&f, &a), exposure(&f, &b)), (1.5, 1.5));
}

/// Ratings and labels can be undone; Ctrl+Z after a rating takes back the rating, not the last
/// edit of the image.
#[test]
fn undo_takes_back_a_rating() {
    let Some(mut f) = Fixture::new("undo-rating") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    f.app.edit(|a| a.exposure = 0.7);
    f.app.commit_and_save();
    f.app.set_rating(4);
    assert_eq!(f.app.current.as_ref().unwrap().rating, 4);
    f.app.undo();
    assert_eq!(f.app.current.as_ref().unwrap().rating, 0, "die Bewertung");
    assert_eq!(f.app.current.as_ref().unwrap().adjustments.exposure, 0.7, "nicht die Bearbeitung");
    f.app.undo();
    assert_eq!(f.app.current.as_ref().unwrap().adjustments.exposure, 0.0);
    f.app.redo();
    f.app.redo();
    assert_eq!(f.app.current.as_ref().unwrap().rating, 4);
}

#[test]
fn catalog_import() {
    use crate::import::{ImportOptions, Target, date_folder};
    let Some(mut f) = Fixture::new("catalog-import") else { return };
    let photos = f.dir.join("photos");
    f.pump("Bild geladen", |a| a.current.is_some());
    // Browsing a folder does not take it into the catalog.
    assert_eq!(f.app.catalog.as_ref().unwrap().image_count().unwrap(), 0);
    assert_eq!(f.app.entries.len(), 2);

    // Copies into the catalog's picture folder, by capture date and renamed; the originals stay.
    let library = f.dir.join("Bilder");
    f.app.import_options = ImportOptions {
        target: Target::Library,
        library: Some(library.clone()),
        by_date: true,
        rename: true,
        template: "{Jahr}{Monat}{Tag}_{Name}".into(),
        ..Default::default()
    };
    f.app.import_folder(&photos);
    f.pump("Import", |a| a.scope == Scope::LastImport && a.entries.len() == 2);
    for name in ["_DSC0009", "_DSC0010"] {
        let original = photos.join(format!("{name}.dng"));
        let date = sk_core::raw::read_meta(&original).unwrap().info.date;
        let day: String = date.as_deref().unwrap_or("").chars().take(10).filter(|c| c.is_ascii_digit()).collect();
        let copy = library.join(date_folder(date.as_deref())).join(format!("{day}_{name}.dng"));
        assert!(f.app.entries.iter().any(|e| e.path == copy), "{} fehlt: {:?}", copy.display(), f.app.entries.iter().map(|e| &e.path).collect::<Vec<_>>());
        assert!(original.exists());
    }

    // Importing again: the dialog knows them (by the names they had), but not another picture of the same size and
    // second (a burst of uncompressed raws).
    std::fs::copy(photos.join("_DSC0009.dng"), photos.join("Serie.dng")).unwrap();
    f.app.open_import(photos.clone(), true);
    let start = Instant::now();
    while f.app.import.as_ref().unwrap().checking() {
        assert!(start.elapsed() < LIMIT, "Zeitüberschreitung: Duplikate prüfen");
        f.app.import.as_mut().unwrap().poll(&f.ctx);
        std::thread::sleep(Duration::from_millis(20));
    }
    let items = &f.app.import.as_ref().unwrap().items;
    assert_eq!(items.len(), 3);
    for i in items {
        let burst = i.path.ends_with("Serie.dng");
        assert_eq!((i.known, i.chosen), (Some(!burst), burst), "{}", i.path.display());
    }
}

#[test]
fn catalog_keeps_the_settings() {
    use sk_core::session::SessionFolder;
    let Some(mut f) = Fixture::new("catalog-settings") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    let path = f.app.primary_path().unwrap();
    // Viewing an image of a folder only browsed takes nothing in.
    assert!(!f.app.catalog.as_ref().unwrap().contains(&path));

    // The first edit takes it in; its settings go to the catalog, not beside the image, so a
    // folder that can't be written to is no obstacle.
    #[cfg(unix)]
    let set_mode = |mode| {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(f.dir.join("photos"), std::fs::Permissions::from_mode(mode)).unwrap();
    };
    #[cfg(unix)]
    set_mode(0o555);
    f.app.set_rating(4);
    #[cfg(unix)]
    set_mode(0o755);
    assert!(f.app.catalog.as_ref().unwrap().contains(&path));
    assert_eq!(sidecar::load(&path).rating, 4);
    assert!(!sidecar::sidecar_path(&path).exists());
    assert!(f.app.unsaved.is_empty(), "{}", f.app.status);

    // Copied into a session, the image takes them along into a sidecar file.
    f.app.create_session(&f.dir, "Shooting").unwrap();
    f.app.import_folder(&f.dir.join("photos"));
    f.pump("Import", |a| a.scope == Scope::LastImport && a.entries.len() == 2);
    let copy = f.app.session.as_ref().unwrap().folder(SessionFolder::Capture).join(path.file_name().unwrap());
    assert_eq!(sidecar::read_sidecar_file(&copy).unwrap().map(|s| s.rating), Some(4));
}

#[test]
fn auto_stack_only_stacks_the_shown_images() {
    let Some(mut f) = Fixture::new("auto_stack_view") else { return };
    f.index_photos();
    f.pump("Indexiert", |a| a.lists.total == 2);
    let a = f.photo("_DSC0009.dng");
    let b = f.photo("_DSC0010.dng");
    f.app.selection = [a.clone()].into_iter().collect();
    f.app.set_rating(5);
    // Only `a` is shown: `b` is filtered out and must not be stacked with it.
    f.app.filter.min_rating = 4;
    f.app.refilter();
    assert_eq!(f.app.visible.len(), 1);
    f.app.selection.clear();
    f.app.auto_stack_by_time(3600.0);
    assert!(sidecar::load(&a).stack.is_none() && sidecar::load(&b).stack.is_none(), "{}", f.app.status);
    // The whole view: both are stacked.
    f.app.filter = Filter::default();
    f.app.refilter();
    f.app.auto_stack_by_time(3600.0);
    assert!(sidecar::load(&a).stack.is_some() && sidecar::load(&b).stack.is_some(), "{}", f.app.status);
}

/// Entf in the layer tab deletes the chosen layer, not a retouch area chosen earlier and no
/// longer shown; another tool ends the area's selection.
#[test]
fn delete_in_layer_tab_ignores_hidden_spot() {
    use crate::app::Tab;
    use crate::layers::LocalTool;
    use sk_core::params::{Spot, SpotMode};
    let Some(mut f) = Fixture::new("delete-hidden-spot") else { return };
    f.pump("Bild geladen", |a| a.current.is_some() && a.texture.is_some());
    let spot = Spot { mode: SpotMode::Heal, target: [0.3, 0.3], source: [0.6, 0.6], path: Vec::new(), strokes: Vec::new(), radius: 0.02, feather: 0.5, opacity: 1.0 };
    f.app.edit(|a| a.retouch.push(spot));
    f.app.set_local_tool(LocalTool::Heal);
    f.app.selected_spot = Some(0);
    f.app.set_local_tool(LocalTool::Clone);
    assert_eq!(f.app.selected_spot, Some(0), "Reparieren und Klonen zeigen die Bereiche weiter");
    f.app.set_local_tool(LocalTool::Brush);
    assert_eq!(f.app.selected_spot, None);
    assert!(f.app.current.as_ref().unwrap().adjustments.layers.is_empty(), "das Werkzeug allein legt keine Ebene an");
    f.app.ensure_layer();
    // Left over (as from before this fix): the layer goes, the area stays.
    f.app.end_viewer_tools();
    f.app.tab = Tab::Layers;
    f.app.selected_spot = Some(0);
    assert_eq!(f.app.current.as_ref().unwrap().adjustments.layers.len(), 1);
    f.app.delete_selected_spot();
    let adj = &f.app.current.as_ref().unwrap().adjustments;
    assert!(adj.layers.is_empty());
    assert_eq!(adj.retouch.len(), 1);
}

/// The retouch tab lists removals and heal/clone areas together: a click chooses one (and the
/// tool that shows it), ↺ in the header clears both kinds.
#[test]
fn retouch_tab_lists_all_areas_together() {
    use crate::app::Tab;
    use crate::layers::LocalTool;
    use sk_core::params::{Removal, Spot, SpotMode};
    let Some(mut f) = Fixture::new("retouch-areas") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    let spot = Spot { mode: SpotMode::Heal, target: [0.3, 0.3], source: [0.6, 0.6], path: Vec::new(), strokes: Vec::new(), radius: 0.02, feather: 0.5, opacity: 1.0 };
    f.app.edit(|a| {
        a.retouch.push(spot);
        a.removals.push(Removal { id: 7, opacity: 1.0, ..Default::default() });
    });
    f.app.tab = Tab::Retouch;
    let nodes = tool_panel_frame(&mut f, vec![]);
    assert!(widget(&nodes, "Objekt entfernen").is_none() && widget(&nodes, "Reparieren & Klonen").is_none());
    let removal = widget(&nodes, "Entfernen 1").expect("Entfernen 1 in der Liste");
    let heal = widget(&nodes, "Reparieren 2").expect("Reparieren 2 in der Liste");
    assert!(removal.top() < heal.top(), "Entfernen zuerst, wie es angewandt wird");

    click_at(&mut f, removal.center());
    assert_eq!((f.app.selected_removal, f.app.selected_spot), (Some(0), None));
    assert_eq!(f.app.local_tool, Some(LocalTool::Remove));
    // The tool's brush settings now show above the list.
    let nodes = tool_panel_frame(&mut f, vec![]);
    let nodes = click_at(&mut f, widget(&nodes, "Reparieren 2").unwrap().center());
    assert_eq!((f.app.selected_removal, f.app.selected_spot), (None, Some(0)));
    assert_eq!(f.app.local_tool, Some(LocalTool::Heal));

    // The rows show the opacity, like the layers; ↺ in the header deletes all.
    let first = widget(&nodes, "Entfernen 1").unwrap();
    let reset = nodes
        .iter()
        .filter(|(l, r)| l == crate::theme::icon::ARROWS_COUNTER_CLOCKWISE && r.bottom() <= first.top())
        .max_by(|a, b| a.1.top().total_cmp(&b.1.top()))
        .map(|(_, r)| *r)
        .expect("↺ im Kopf der Retuschen");
    click_at(&mut f, reset.center());
    let adj = &f.app.current.as_ref().unwrap().adjustments;
    assert!(adj.retouch.is_empty() && adj.removals.is_empty());
}

/// The results of an auto run join one undo step, a paste onto several images meanwhile stays a
/// step of its own; a result of a cancelled or older run is dropped.
#[test]
fn auto_run_keeps_foreign_steps_and_results_apart() {
    use crate::auto_tools::AutoKind;
    use sk_core::params::Adjustments;
    let Some(mut f) = Fixture::new("auto-run") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    let (a, b) = (f.photo("_DSC0009.dng"), f.photo("_DSC0010.dng"));
    let mut opts = sk_core::auto::AutoOptions::NONE;
    opts.exposure = true;
    let kind = AutoKind::Adjust(opts);
    let result = |ev: f32| Ok(Some(Box::new(Adjustments { exposure: ev, ..Default::default() })));
    // As `run_auto` starts a run, without the worker.
    f.app.auto_run += 1;
    let run = f.app.auto_run;
    f.app.auto_progress = Some((0, 2));
    f.app.batch_step = None;

    f.app.auto_adjusted(run - 1, b.clone(), kind, result(2.0));
    assert_eq!(f.app.auto_progress, Some((0, 2)), "Ergebnis eines alten Laufs zählt nicht");
    assert_eq!(sidecar::load(&b).adjustments.exposure, 0.0);

    f.app.auto_adjusted(run, a.clone(), kind, result(1.0));
    let steps = f.app.extra_undo.len();
    f.app.set_adjustments_on(&[a.clone(), b.clone()], |x| Adjustments { contrast: 20.0, ..x.clone() });
    assert_eq!(f.app.extra_undo.len(), steps + 1, "Einfügen bleibt ein eigener Schritt");
    f.app.auto_adjusted(run, b.clone(), kind, result(1.0));
    assert_eq!(f.app.extra_undo.len(), steps + 1, "das zweite Ergebnis kommt zum Schritt des Laufs");
    assert!(f.app.auto_progress.is_none());
    assert_eq!(sidecar::load(&b).adjustments.exposure, 1.0);
}

/// Presses on a slider's rail at `from` (share of the rail), moves to `to` in ten steps (ending
/// `dy` below the row), releases; returns the value read by `get` after each step.
fn drag_slider(f: &mut Fixture, label: &str, from: f32, to: f32, dy: f32, get: impl Fn(&Fixture) -> f32) -> Vec<f32> {
    let nodes = tool_panel_frame(f, vec![]);
    let rail = slider(&nodes, label);
    let at = |s: f32, y: f32| egui::pos2(rail.left() + 7.0 + (rail.width() - 14.0) * s, rail.center().y + y);
    let button = |p, pressed| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    let a = at(from, 0.0);
    tool_panel_frame(f, vec![egui::Event::PointerMoved(a)]);
    tool_panel_frame(f, vec![button(a, true)]);
    let mut seen = vec![get(f)];
    for k in 1..=10 {
        let t = k as f32 / 10.0;
        tool_panel_frame(f, vec![egui::Event::PointerMoved(at(from + (to - from) * t, dy * t))]);
        seen.push(get(f));
    }
    tool_panel_frame(f, vec![button(at(to, dy), false)]);
    seen
}

/// A drag keeps going when the tool header changes under it: with the first change of a tool,
/// when the value passes its default, and when the pointer leaves the row. (The header's buttons
/// shifted egui's automatic ids, which ended the drag.)
#[test]
fn slider_drags_survive_header_changes() {
    let Some(mut f) = Fixture::new("slider-drags") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    let adj = |f: &Fixture| f.app.current.as_ref().unwrap().adjustments.clone();
    let follows = |seen: &[f32], label: &str| {
        assert!(seen.windows(2).all(|w| w[1] > w[0]), "{label} folgt der Maus nicht: {seen:?}");
    };

    // The untouched tool, from below its default to above.
    f.app.tab = crate::app::Tab::Exposure;
    let seen = drag_slider(&mut f, "Belichtung", 0.2, 0.8, 0.0, |f| adj(f).exposure);
    follows(&seen, "Belichtung");
    assert!(seen[0] < 0.0 && seen[10] > 0.0, "{seen:?}");

    // The color editor's bands, also when the pointer wanders over the rows below.
    f.app.tab = crate::app::Tab::Color;
    follows(&drag_slider(&mut f, "Rot", 0.3, 0.7, 0.0, |f| adj(f).color_editor.bands[0].hue), "Rot");
    follows(&drag_slider(&mut f, "Orange", 0.3, 0.7, 80.0, |f| adj(f).color_editor.bands[1].hue), "Orange");

    // To zero (the tool becomes unchanged) and back up in one drag.
    f.app.tab = crate::app::Tab::Details;
    f.app.edit(|a| a.detail.ai_denoise = 50.0);
    std::sync::Arc::make_mut(&mut f.app.current.as_mut().unwrap().meta).hdr = false;
    let down = drag_slider(&mut f, "KI-Entrauschen", 0.5, 0.0, 0.0, |f| adj(f).detail.ai_denoise);
    assert_eq!(*down.last().unwrap(), 0.0, "{down:?}");
    follows(&drag_slider(&mut f, "KI-Entrauschen", 0.0, 0.4, 0.0, |f| adj(f).detail.ai_denoise), "KI-Entrauschen");
}

/// Shift and the mouse button held on a tool's reset button leave the tool out of the view
/// until released; the settings stay. Without Shift the button resets.
#[test]
fn shift_on_reset_leaves_the_tool_out_while_held() {
    let Some(mut f) = Fixture::new("tool-peek") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    f.app.tab = crate::app::Tab::Exposure;
    f.app.edit(|a| a.exposure = 1.0);
    let nodes = tool_panel_frame(&mut f, vec![]);
    // The reset button in the header right above the exposure slider.
    let rail = slider(&nodes, "Belichtung");
    let reset = nodes
        .iter()
        .filter(|(l, r)| l == crate::theme::icon::ARROWS_COUNTER_CLOCKWISE && r.bottom() <= rail.top())
        .map(|(_, r)| *r)
        .max_by(|a, b| a.top().total_cmp(&b.top()))
        .expect("Zurücksetzen im Kopf")
        .center();
    let button = |pressed| egui::Event::PointerButton { pos: reset, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::SHIFT };
    tool_panel_frame_with(&mut f, vec![egui::Event::PointerMoved(reset)], egui::Modifiers::SHIFT);
    tool_panel_frame_with(&mut f, vec![button(true)], egui::Modifiers::SHIFT);
    tool_panel_frame_with(&mut f, vec![], egui::Modifiers::SHIFT);
    assert_eq!(f.app.tool_peek, Some(sk_core::tools::Tool::Exposure));
    tool_panel_frame_with(&mut f, vec![button(false)], egui::Modifiers::SHIFT);
    tool_panel_frame(&mut f, vec![]);
    assert_eq!(f.app.tool_peek, None);
    let adj = |f: &Fixture| f.app.current.as_ref().unwrap().adjustments.clone();
    assert_eq!((adj(&f).exposure, adj(&f).hidden.len()), (1.0, 0), "nur vorübergehend");
    click_at(&mut f, reset);
    assert_eq!(adj(&f).exposure, 0.0, "ohne Shift: zurückgesetzt");
}

/// Leaving a tool out with Shift held on its reset button lasts while the button is held, also
/// beyond the 0.8 s after which egui no longer takes the press for a click (frames here are a
/// quarter second apart).
#[test]
fn leaving_enhance_out_lasts_while_held() {
    let Some(mut f) = Fixture::new("tool-peek-enhance") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    f.app.tab = crate::app::Tab::Details;
    let nodes = tool_panel_frame(&mut f, vec![]);
    let rail = slider(&nodes, "Raw-Details");
    let reset = nodes
        .iter()
        .filter(|(l, r)| l == crate::theme::icon::ARROWS_COUNTER_CLOCKWISE && r.bottom() <= rail.top())
        .map(|(_, r)| *r)
        .max_by(|a, b| a.top().total_cmp(&b.top()))
        .expect("Zurücksetzen von Verbessern")
        .center();
    let button = |pressed| egui::Event::PointerButton { pos: reset, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::SHIFT };
    tool_panel_frame_with(&mut f, vec![egui::Event::PointerMoved(reset)], egui::Modifiers::SHIFT);
    tool_panel_frame_with(&mut f, vec![button(true)], egui::Modifiers::SHIFT);
    for i in 0..12 {
        f.app.handle_events(&f.ctx);
        f.app.send_render();
        tool_panel_frame_with(&mut f, vec![], egui::Modifiers::SHIFT);
        assert_eq!(f.app.tool_peek, Some(sk_core::tools::Tool::Enhance), "Bild {i}");
        std::thread::sleep(Duration::from_millis(40));
    }
    tool_panel_frame_with(&mut f, vec![button(false)], egui::Modifiers::SHIFT);
    tool_panel_frame(&mut f, vec![]);
    assert_eq!(f.app.tool_peek, None);
}

/// Zoomed in, holding Shift on a tool's reset button renders the view sharp, without the tool,
/// not coarse as while dragging a slider (the mouse button is down all the while).
#[test]
fn leaving_a_tool_out_stays_sharp_when_zoomed_in() {
    let Some(mut f) = Fixture::new("tool-peek-zoomed") else { return };
    f.pump("Vollauflösung", |a| a.texture.is_some() && a.current.as_ref().is_some_and(|c| c.full.is_some()));
    f.app.edit(|a| a.exposure = 1.0);
    f.app.commit_and_save();
    f.app.tool_peek = Some(sk_core::tools::Tool::Exposure);
    f.app.request_render();
    f.app.pointer_down = true;
    assert!(f.app.shown_adjustments().hidden.contains(&sk_core::tools::Tool::Exposure));
    let tex = f.app.texture.as_ref().unwrap().size_vec2();
    let viewport = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
    let img_rect = egui::Rect::from_center_size(viewport.center(), tex * 3.0);
    f.app.update_tiles(viewport, img_rect, 3.0, true);
    assert_eq!(f.app.drag_view_level, None, "kein Entwurf wie beim Ziehen");
    f.pump("scharfer Ausschnitt", |a| a.tiles.view.as_ref().is_some_and(|v| v.version == a.edit_version && v.level == 1));
}

/// The lens correction comes from one source, chosen in a drop-down: none, the camera's data or
/// a local profile.
#[test]
fn lens_correction_source_is_chosen_in_a_list() {
    let Some(mut f) = Fixture::new("lens-source") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    f.app.tab = crate::app::Tab::Lens;
    let lens = |f: &Fixture| f.app.current.as_ref().unwrap().adjustments.lens.clone();
    // A correction stored by the camera (the sample files have none).
    let neutral = sk_core::camera_lens::CameraLens { source: "Test", center: [0.0; 2], warp: [[1.0, 0.0, 0.0, 0.0]; 3], vignette: [0.0; 5] };
    std::sync::Arc::make_mut(&mut f.app.current.as_mut().unwrap().meta).lens = Some(neutral);
    let choose = |f: &mut Fixture, entry: &str| {
        let nodes = tool_panel_frame(f, vec![]);
        let combo = widget(&nodes, "Korrektur").expect("Auswahl der Korrektur");
        let nodes = click_at(f, combo.center());
        let item = nodes.iter().filter(|(l, r)| l == entry && r.top() > combo.bottom() - 1.0).map(|(_, r)| *r).next().unwrap_or_else(|| panic!("Eintrag {entry} fehlt"));
        click_at(f, item.center());
    };
    let profile = f.app.lens_profile_name().is_some();
    choose(&mut f, "Kamera");
    assert!(lens(&f).camera && lens(&f).profile.is_none(), "{:?}", lens(&f));
    choose(&mut f, "Keine");
    assert!(!lens(&f).camera && lens(&f).profile.is_none(), "{:?}", lens(&f));
    // The profile only where the lens database is there (not in every test environment).
    if profile {
        choose(&mut f, "Lokales Profil");
        assert!(!lens(&f).camera && lens(&f).profile.is_some(), "{:?}", lens(&f));
        choose(&mut f, "Keine");
        assert!(lens(&f).profile.is_none() && lens(&f).profile_coeffs.is_none(), "{:?}", lens(&f));
    }
}

#[test]
fn storage_dialog_measures_the_caches_once() {
    let Some(mut f) = Fixture::new("storage") else { return };
    f.app.show_storage_dialog = true;
    let app = &mut f.app;
    f.ctx.run_ui(Default::default(), |ui| app.storage_dialog(ui.ctx())).textures_delta.clear();
    assert!(f.app.storage_sizes.is_some(), "beim Öffnen gemessen");
    // Closed: measured anew next time.
    f.app.show_storage_dialog = false;
    let app = &mut f.app;
    f.ctx.run_ui(Default::default(), |ui| app.storage_dialog(ui.ctx())).textures_delta.clear();
    assert!(f.app.storage_sizes.is_none());
}

#[test]
fn relight_split_is_drawn_without_a_depth_map() {
    let Some(mut f) = Fixture::new("relight") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    f.app.edit(|a| a.relight.near = 0.5);
    let app = &mut f.app;
    f.ctx.run_ui(Default::default(), |ui| app.relight_section(ui)).textures_delta.clear();
    // Drawing changes nothing.
    assert_eq!(f.app.current.as_ref().unwrap().adjustments.relight.split, sk_core::params::Relight::default().split);
}

#[test]
fn side_column_stays_while_the_next_image_loads() {
    let Some(mut f) = Fixture::new("side-column") else { return };
    f.pump("Bild geladen", |a| a.current.is_some());
    f.app.side_column = crate::side_column::SideColumn::Always;
    assert!(f.app.side_column_wanted(false));
    // Between two images: the previous one put aside, the next one not loaded yet.
    let path = f.app.current_path();
    f.app.current = None;
    f.app.loading = path;
    assert!(f.app.side_column_wanted(false), "bleibt beim Laden");
    f.app.loading = None;
    assert!(!f.app.side_column_wanted(false), "ohne Bild nicht");
}

/// Zoomed in on a split view, the before image gets the same sharpness as the after image: its
/// visible part is rendered from the full resolution too.
#[test]
fn split_view_renders_the_before_image_sharp_when_zoomed_in() {
    let Some(mut f) = Fixture::new("before-detail") else { return };
    f.pump("Bild geladen", |a| a.texture.is_some());
    f.app.edit(|a| a.exposure = 1.0);
    f.app.toggle_split();
    f.app.send_render();
    f.pump("Vorher", |a| a.before_texture.is_some());
    f.app.want_full_now();
    f.pump("volle Auflösung", |a| {
        a.current.as_ref().is_some_and(|c| c.full.is_some())
    });
    // Zoomed in far: the preview is magnified on screen.
    f.app.zoom = Some(4.0);
    let start = Instant::now();
    while f.app.before_detail.is_none() {
        assert!(start.elapsed() < LIMIT, "Zeitüberschreitung: Vorher scharf");
        viewer_frame(&mut f, vec![]);
        f.app.handle_events(&f.ctx.clone());
        std::thread::sleep(Duration::from_millis(20));
    }
    let d = f.app.before_detail.as_ref().unwrap();
    assert_eq!(d.version, f.app.before_key);
    assert!(d.width > f.app.before_texture.as_ref().unwrap().size_vec2().x, "schärfer als die Vorschau");
}

/// Zoomed in, the sharp tiles stay on screen while the view without the tool (Shift held on its
/// reset button) is rendered, and again when it is let go: never the magnified preview.
#[test]
fn leaving_a_tool_out_keeps_the_view_sharp_meanwhile() {
    let Some(mut f) = Fixture::new("tool-peek-sharp") else { return };
    f.pump("Vollauflösung", |a| a.texture.is_some() && a.current.as_ref().is_some_and(|c| c.full.is_some()));
    f.app.edit(|a| a.detail.sharpen_amount = 150.0);
    f.app.commit_and_save();
    let mut frame = eframe::Frame::_new_kittest();
    let mut t = 10.0;
    f.app.splash = None;
    f.app.set_zoom_pct(100.0);
    let mut frames = |f: &mut Fixture, t: &mut f64, peek: Option<sk_core::tools::Tool>, n: usize| {
        for _ in 0..n {
            *t += 0.05;
            if let Some(tool) = peek {
                f.ctx.data_mut(|d| d.insert_temp(egui::Id::new("tool_peek"), tool));
            }
            window_frame(f, &mut frame, *t, vec![]);
            f.app.handle_events(&f.ctx);
            std::thread::sleep(Duration::from_millis(30));
        }
    };
    let sharp_now = |a: &App| a.tiles.view.as_ref().is_some_and(|v| v.level == 1 && v.version == a.edit_version);
    let sharp_shown = |a: &App| {
        let stale = a.tiles_stale_ok && a.tiles.version != a.edit_version;
        a.tiles.view.as_ref().is_some_and(|v| v.level == 1 && (v.version == a.edit_version || stale))
    };
    let start = Instant::now();
    while !sharp_now(&f.app) {
        assert!(start.elapsed() < LIMIT, "scharfer Ausschnitt");
        frames(&mut f, &mut t, None, 1);
    }
    for peek in [Some(sk_core::tools::Tool::Sharpening), None] {
        let version = f.app.edit_version;
        let start = Instant::now();
        loop {
            frames(&mut f, &mut t, peek, 1);
            assert!(f.app.edit_version > version, "neue Darstellung");
            assert!(sharp_shown(&f.app), "{peek:?}: zwischendurch die vergrößerte Vorschau");
            if sharp_now(&f.app) {
                break;
            }
            assert!(start.elapsed() < LIMIT, "{peek:?}: neuer scharfer Ausschnitt");
        }
    }
}
