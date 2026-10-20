//! Writes the depth map of a raw file and the image with lens blur as PNGs:
//! `cargo run --release -p sk-ai --example depth -- <raw> <out_dir> [amount] [focus] [bokeh]`
//! (focus 0 far .. 1 near; without it the nearest tenth of the image is in focus).
//! With `SK_ADJ` (adjustments as JSON, e.g. `{"atmosphere":{"amount":60}}`) those are rendered instead.

use std::path::Path;

use sk_core::params::{Adjustments, LensBlur};
use sk_core::pipeline::Processor;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let path = Path::new(&args[1]);
    let out = Path::new(&args[2]);
    std::fs::create_dir_all(out)?;
    let num = |i: usize, d: f32| args.get(i).and_then(|v| v.parse().ok()).unwrap_or(d);
    sk_ai::depth::register();
    let t = std::time::Instant::now();
    sk_core::depth::ensure(path, &|_| {})?;
    println!("Tiefenkarte {:?}", t.elapsed());
    let map = sk_core::depth::load(path).expect("eben berechnet");
    let gray: Vec<u8> = map.data.iter().map(|d| (d * 255.0 + 0.5) as u8).collect();
    image::GrayImage::from_raw(map.width as u32, map.height as u32, gray).unwrap().save(out.join("depth.png"))?;
    let (meta, preview) = sk_core::raw::load_preview(path, 2400)?;
    let lens_blur = LensBlur { amount: num(3, 60.0), focus: num(4, map.quantile(0.9)), range: 0.1, bokeh: num(5, 30.0) };
    println!("{lens_blur:?}");
    let adj = match std::env::var("SK_ADJ") {
        Ok(json) => serde_json::from_str(&json)?,
        Err(_) => Adjustments { lens_blur, ..Default::default() },
    };
    let t = std::time::Instant::now();
    let r = Processor::new(&meta, &adj).render_cpu(&preview, None, None);
    println!("Objektivunschärfe {}×{} {:?}", r.width, r.height, t.elapsed());
    image::RgbaImage::from_raw(r.width as u32, r.height as u32, r.rgba).unwrap().save(out.join("blur.png"))?;
    Ok(())
}
