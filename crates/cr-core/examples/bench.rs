//! Times the pipeline stages on a raw file: `cargo run --release --example bench -- <raw>`

use std::time::Instant;

use cr_core::detail::{Plane, box_blur, gaussian_blur};
use cr_core::params::{Adjustments, Detail};
use cr_core::pipeline::{Processor, StageCache};
use cr_core::raw;

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("usage: bench <raw>");
    let (meta, preview) = raw::load_preview(std::path::Path::new(&path), 2048)?;
    let (w, h) = (preview.width, preview.height);
    println!("preview {w}x{h}");
    let plane = Plane::new(w, h, vec![0.5; w * h]);
    let t = Instant::now();
    let _ = box_blur(&plane, 5);
    println!("box_blur r=5: {:?}", t.elapsed());
    let t = Instant::now();
    let _ = gaussian_blur(&plane, 1.0);
    println!("gaussian s=1: {:?}", t.elapsed());

    let none = Adjustments { detail: Detail { sharpen_amount: 0.0, nr_color: 0.0, ..Default::default() }, ..Default::default() };
    for (name, adj) in [
        ("ohne Details", none.clone()),
        ("Standard", Adjustments::default()),
        ("Klarheit+HDR", Adjustments { shadows: 40.0, detail: Detail { clarity: 40.0, ..Default::default() }, ..Default::default() }),
    ] {
        let p = Processor::new(&meta, &adj);
        let t = Instant::now();
        let _ = p.render(&preview, None, None);
        let cold = t.elapsed();
        let cache = StageCache::default();
        let _ = p.render(&preview, None, Some(&cache));
        let t = Instant::now();
        let _ = p.render(&preview, None, Some(&cache));
        println!("{name}: {cold:?} (mit Cache: {:?})", t.elapsed());
    }
    Ok(())
}
