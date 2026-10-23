//! Times an export: the stages of one image (load, develop on the CPU and on the graphics card,
//! JPEG), then a batch exported as the app does it, with and without the card. Files go to
//! `<out>`. On a laptop the results vary with its temperature: compare variants in the order
//! A B B A.
//!
//! `cargo run --release -p sk-core --example export_pipeline -- <out> <raw>...`

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sk_core::color::OutputSpace;
use sk_core::export::{self, ExportRecipe};
use sk_core::image::LinearImage;
use sk_core::params::Adjustments;
use sk_core::pipeline::Processor;
use sk_core::raw::{self, RawMeta};

/// Images of the batch: the given files, repeated to at least this many.
const BATCH: usize = 6;

fn load(path: &Path) -> (RawMeta, LinearImage) {
    raw::load_full(path).unwrap_or_else(|e| panic!("{}: {e:#}", path.display()))
}

fn develop(meta: &RawMeta, linear: &LinearImage) -> LinearImage {
    Processor::new(meta, &Adjustments::default()).process(linear, None, OutputSpace::Srgb)
}

fn encode(img: &LinearImage) -> usize {
    let rgb: Vec<u8> = img.data.iter().flat_map(|p| p.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)).collect();
    export::encode_jpeg(&rgb, img.width as u32, img.height as u32, 90, None, &[]).expect("in memory").len()
}

fn timed<R>(f: impl FnOnce() -> R) -> (R, Duration) {
    let t = Instant::now();
    let r = f();
    (r, t.elapsed())
}

/// The batch as the app's export does it: one image after the other.
fn batch_export(paths: &[PathBuf], recipe: &ExportRecipe) -> Duration {
    let adj = Adjustments::default();
    timed(|| {
        let mut written = HashSet::new();
        for (i, p) in paths.iter().enumerate() {
            for r in export::export_many_with(p, &adj, std::slice::from_ref(recipe), i as u32 + 1, "", &mut written) {
                r.expect("exported");
            }
        }
    })
    .1
}

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).expect("usage: export_pipeline <out> <raw>..."));
    let files: Vec<PathBuf> = std::env::args().skip(2).map(PathBuf::from).collect();
    assert!(!files.is_empty(), "usage: export_pipeline <out> <raw>...");
    let cores = std::thread::available_parallelism().map_or(0, |n| n.get());
    let card = sk_core::gpu::adapter_name();
    println!("Kerne: {cores}, Grafikkarte: {}", card.as_deref().unwrap_or("keine"));
    sk_core::pipeline::prepare_gpu();

    println!("\nEinzelne Schritte (erstes Bild, je zweimal, der zweite Lauf zählt):");
    let (meta, linear) = load(&files[0]);
    let (_, t_load) = timed(|| load(&files[0]));
    let mut developed = None;
    for on in [false, true] {
        if on && card.is_none() {
            continue;
        }
        sk_core::gpu::set_enabled(on);
        develop(&meta, &linear);
        let (img, t) = timed(|| develop(&meta, &linear));
        println!("  entwickeln {}: {t:.2?}", if on { "Grafikkarte" } else { "Prozessor  " });
        developed = Some(img);
    }
    let (_, t_encode) = timed(|| encode(developed.as_ref().expect("developed")));
    println!("  laden: {t_load:.2?}, JPEG kodieren: {t_encode:.2?}");

    let batch: Vec<PathBuf> = files.iter().cycle().take(BATCH.max(files.len())).cloned().collect();
    println!("\nExport von {} Bildern:", batch.len());
    for on in [true, false] {
        if on && card.is_none() {
            continue;
        }
        sk_core::gpu::set_enabled(on);
        // A folder of its own (same names would get -2, -3 …).
        let recipe = ExportRecipe { output_dir: out.join(if on { "grafikkarte" } else { "prozessor" }), ..Default::default() };
        let t = batch_export(&batch, &recipe);
        println!("  {}: {t:.1?} ({:.2?}/Bild)", if on { "mit Grafikkarte" } else { "nur Prozessor  " }, t / batch.len() as u32);
    }
    if let Some(p) = sk_core::gpu::problem() {
        println!("\nGrafikkarte nicht mehr benutzt: {p}");
    }
}
