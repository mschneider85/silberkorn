//! End-to-end tests of the application logic with real workers and sample raw files.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cr_core::catalog::{Filter, Scope, SortKey};
use cr_core::sidecar::{self, ColorLabel};
use cr_core::styles;

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
        let dir = std::env::temp_dir().join(format!("cr-app-test-{name}-{}", std::process::id()));
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
    let style = styles::Style { name: "Kontrast".into(), adjustments: cr_core::params::Adjustments { contrast: 30.0, ..Default::default() } };
    styles::save_in(&style_dir, &style).unwrap();
    let loaded = styles::list_in(&style_dir);
    f.app.set_adjustments_on(std::slice::from_ref(&b), |adj| styles::apply(&loaded[0], adj));
    assert_eq!(sidecar::load(&b).adjustments.contrast, 30.0);

    // Export both images.
    f.app.recipe.output_dir = f.dir.join("out");
    f.app.recipe.long_edge = Some(800);
    f.app.selection = [a.clone(), b.clone()].into_iter().collect();
    f.app.export_selection();
    f.pump("Export", |app| app.export_progress.is_none() && app.export_log.len() == 2);
    assert!(f.app.export_log.iter().all(|l| l.starts_with('✔')), "{:?}", f.app.export_log);
    for name in ["_DSC0009.jpg", "_DSC0010.jpg"] {
        let img = ::image::open(f.dir.join("out").join(name)).unwrap();
        assert_eq!(img.width().max(img.height()), 800);
    }
}
