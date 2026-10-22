//! Times what one waits for while editing: opening an image, the preview while a slider moves,
//! 100 % view, automatic adjustment, masks, depth, matching a look and removing. Each step runs
//! twice where the second run is what one sees after the first (caches, loaded models):
//! `cargo run --release -p sk-ai --example interactive -- <raw> <reference raw>`

use std::path::Path;
use std::time::{Duration, Instant};

use sk_ai::{SubjectModel, guide_from_rgba, sky_mask};
use sk_core::auto::{AutoOptions, auto_adjust, guide_image};
use sk_core::geometry::Region;
use sk_core::look_match::{LookOptions, LookStats, match_look};
use sk_core::params::{Adjustments, BrushStroke, Layer, LayerAdjustments, Mask, RadialGradient};
use sk_core::pipeline::{Processor, StageCache};
use sk_core::raw;

fn timed<R>(f: impl FnOnce() -> R) -> (R, Duration) {
    let t = Instant::now();
    let r = f();
    (r, t.elapsed())
}

fn row(name: &str, first: Duration, again: Option<Duration>) {
    match again {
        Some(a) => println!("  {name:<44} {:>8.0} ms   danach {:>6.0} ms", first.as_secs_f64() * 1000.0, a.as_secs_f64() * 1000.0),
        None => println!("  {name:<44} {:>8.0} ms", first.as_secs_f64() * 1000.0),
    }
}

/// A typical edit: tone, clarity, and a layer with a radial mask.
fn edited() -> Adjustments {
    let mut adj = Adjustments { shadows: 30.0, highlights: -20.0, vibrance: 15.0, ..Default::default() };
    adj.detail.clarity = 20.0;
    adj.layers.push(Layer {
        adjustments: LayerAdjustments { exposure: 0.4, ..Default::default() },
        mask: Mask {
            radial: vec![RadialGradient { center: [0.5, 0.5], radius_x: 0.3, radius_y: 0.25, feather: 0.6, invert: false, angle: 0.0 }],
            ..Default::default()
        },
        ..Default::default()
    });
    adj
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (path, reference) = (Path::new(&args[0]), Path::new(&args[1]));
    println!(
        "Kerne: {}, Grafikkarte: {}, KI-Grafikkarte installiert: {}",
        std::thread::available_parallelism().map_or(0, |n| n.get()),
        sk_core::gpu::adapter_name().as_deref().unwrap_or("keine"),
        sk_ai::runtime::gpu_installed()
    );
    sk_core::pipeline::prepare_gpu();

    println!("\nBild öffnen und bearbeiten:");
    let (_, t1) = timed(|| raw::load_preview(path, 2048));
    let ((meta, preview), t2) = timed(|| raw::load_preview(path, 2048).expect("Vorschau"));
    row("Vorschau laden (2048 px)", t1, Some(t2));
    let adj = edited();
    let cache = StageCache::default();
    let (_, t1) = timed(|| Processor::new(&meta, &adj).render(&preview, None, Some(&cache)));
    let (_, t2) = timed(|| Processor::new(&meta, &adj).render(&preview, None, Some(&cache)));
    row("Vorschau rendern (typische Bearbeitung)", t1, Some(t2));
    // As while dragging: every step a new value, the stages before it from the cache.
    let mut steps = Vec::new();
    for i in 0..8 {
        let a = Adjustments { exposure: i as f32 * 0.1, ..adj.clone() };
        steps.push(timed(|| Processor::new(&meta, &a).render(&preview, None, Some(&cache))).1);
    }
    row("Belichtung ziehen, je Schritt", steps.iter().sum::<Duration>() / steps.len() as u32, None);
    let mut steps = Vec::new();
    for i in 0..8 {
        let a = Adjustments { detail: sk_core::params::Detail { clarity: 20.0 + i as f32 * 5.0, ..adj.detail }, ..adj.clone() };
        steps.push(timed(|| Processor::new(&meta, &a).render(&preview, None, Some(&cache))).1);
    }
    row("Klarheit ziehen, je Schritt", steps.iter().sum::<Duration>() / steps.len() as u32, None);
    let mut steps = Vec::new();
    for i in 0..8 {
        let mut a = adj.clone();
        a.layers[0].adjustments.exposure = 0.4 + i as f32 * 0.1;
        steps.push(timed(|| Processor::new(&meta, &a).render(&preview, None, Some(&cache))).1);
    }
    row("Ebenen-Belichtung ziehen, je Schritt", steps.iter().sum::<Duration>() / steps.len() as u32, None);

    println!("\n100 %-Ansicht:");
    let ((_, full), t) = timed(|| raw::load_full(path).expect("volle Auflösung"));
    row("Volle Auflösung laden", t, None);
    let full_cache = StageCache::default();
    let p = Processor::new(&meta, &adj);
    let tile = Region { x: full.width / 2, y: full.height / 2, w: 1024, h: 1024 };
    let (_, t1) = timed(|| p.render(&full, Some(tile), Some(&full_cache)));
    let (_, t2) = timed(|| p.render(&full, Some(tile), Some(&full_cache)));
    row("Kachel 1024² rendern", t1, Some(t2));

    println!("\nKI und Werkzeuge:");
    let opts = AutoOptions::default();
    let mut models = sk_ai::auto::AutoModels::default();
    let guide = guide_image(&meta, &Adjustments::default(), &preview, &opts);
    // As the app does at start (in the background).
    let (_, t) = timed(|| models.preload());
    row("Automatisch: Modelle vorladen (beim Start)", t, None);
    let (focus, t1) = timed(|| models.focus(&guide));
    let (_, t2) = timed(|| models.focus(&guide));
    row("Automatisch: Gesichter und Motiv finden", t1, Some(t2));
    let (_, t) = timed(|| auto_adjust(&meta, &Adjustments::default(), &preview, &opts, &focus, sk_core::auto::camera_brightness(&meta)));
    row("Automatisch: Einstellungen berechnen", t, None);

    let r = Processor::new(&meta, &Adjustments::default()).render(&preview, None, None);
    let mask_guide = guide_from_rgba(r.width, r.height, &r.rgba);
    let (_, t) = timed(|| sky_mask(&mask_guide));
    row("Maske: Himmel", t, None);
    match SubjectModel::load(&sk_ai::default_model_path()) {
        Ok(m) => {
            let (_, t1) = timed(|| m.subject_mask(&mask_guide));
            let (_, t2) = timed(|| m.subject_mask(&mask_guide));
            row("Maske: Motiv", t1, Some(t2));
        }
        Err(e) => println!("  Maske: Motiv – {e:#}"),
    }
    match sk_ai::segment::ObjectModel::load() {
        Ok(m) => {
            let (emb, t) = timed(|| m.embed(&mask_guide));
            row("Maske: Objekt, Bild analysieren", t, None);
            if let Ok(emb) = emb {
                let click = [sk_ai::segment::Click { uv: [0.5, 0.5], include: true }];
                let (_, t1) = timed(|| m.mask(&emb, &click));
                let (_, t2) = timed(|| m.mask(&emb, &click));
                row("Maske: Objekt, je Klick", t1, Some(t2));
            }
        }
        Err(e) => println!("  Maske: Objekt – {e:#}"),
    }
    sk_ai::depth::register();
    let (res, t) = timed(|| sk_core::depth::ensure(path, &|_| {}));
    match res {
        Ok(_) => row("Tiefenkarte (beim ersten Mal)", t, None),
        Err(e) => println!("  Tiefenkarte – {e:#}"),
    }

    let mut regions = |w: usize, h: usize, rgba: &[u8]| models.look_regions(w, h, rgba);
    let (ref_meta, ref_preview) = raw::load_preview(reference, 600)?;
    let (refs, t1) = timed(|| LookStats::of_development(&ref_meta, &edited(), &ref_preview, &mut regions).expect("Referenz"));
    row("Look angleichen: Referenz messen", t1, None);
    let (small_meta, small) = raw::load_preview(path, 600)?;
    let (_, t) = timed(|| match_look(&[refs], &small_meta, &Adjustments::default(), &small, LookOptions::default(), &mut regions));
    row("Look angleichen: Bild angleichen", t, None);

    match sk_ai::inpaint::Inpainter::load(&sk_ai::models::path(&sk_ai::models::INPAINT)) {
        Ok(model) => {
            let points: Vec<[f32; 2]> = (0..=20).map(|i| [0.3, 0.4 + 0.1 * i as f32 / 20.0]).collect();
            let stroke = BrushStroke { points, radius: 0.01, hardness: 0.7, flow: 1.0, erase: false, opacity: 1.0, auto_mask: false, build_up: true, tolerance: 50.0 };
            let (_, t) = timed(|| sk_ai::inpaint::make_removal(&full, meta.as_shot_wb, &[stroke], &model, &|_| {}));
            row("Entfernen (kleiner Strich)", t, None);
        }
        Err(e) => println!("  Entfernen – {e:#}"),
    }
    println!("\nKI lief zuletzt auf: {}", sk_ai::runtime::last_device().unwrap_or("?"));
    if let Some(p) = sk_core::gpu::problem() {
        println!("Grafikkarte nicht mehr benutzt: {p}");
    }
    Ok(())
}
