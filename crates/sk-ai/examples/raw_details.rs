//! Raw-Details of one raw file: `raw_details <raw> <out-dir> [x y]`. Writes a crop at 100 %
//! (centered at `x`, `y` in 0..1 of the image, default the middle) with AHD and with the
//! network as PNG.

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (path, out) = (std::path::PathBuf::from(&args[1]), std::path::PathBuf::from(&args[2]));
    let at = |i: usize| args.get(i).and_then(|a| a.parse::<f32>().ok()).unwrap_or(0.5);
    sk_ai::demosaic::register();
    let start = std::time::Instant::now();
    sk_core::ai_products::DETAILS.ensure(&path, &|p| eprint!("\r{:3.0} %", p * 100.0))?;
    eprintln!("\nberechnet in {:.1?} ({})", start.elapsed(), sk_ai::runtime::last_device().unwrap_or("?"));
    let (meta, full) = sk_core::raw::load_full(&path)?;
    let crop = |a: f32| {
        let mut adj = sk_core::params::Adjustments::default();
        adj.detail.raw_details = a;
        let p = sk_core::pipeline::Processor::new(&meta, &adj);
        let (w, h) = p.output_size(full.width, full.height);
        let (cx, cy) = ((w as f32 * at(3)) as usize, (h as f32 * at(4)) as usize);
        let region = sk_core::geometry::Region { x: cx.clamp(200, w - 200) - 200, y: cy.clamp(150, h - 150) - 150, w: 400, h: 300 };
        let r = p.render(&full, Some(region), None);
        image::RgbaImage::from_raw(r.width as u32, r.height as u32, r.rgba).unwrap()
    };
    crop(0.0).save(out.join("ahd.png"))?;
    crop(100.0).save(out.join("raw_details.png"))?;
    Ok(())
}
