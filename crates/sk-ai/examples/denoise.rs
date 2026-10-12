//! AI denoising of one raw file: `denoise <raw> <out-dir> [amount] [--linear]`. Writes a crop at
//! 100 % without and with denoising as PNG (`--linear`: with the network for demosaiced data,
//! also for Bayer sensors). The models are downloaded if needed.

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().filter(|a| a != "--linear").collect();
    let linear = std::env::args().any(|a| a == "--linear");
    let (path, out) = (std::path::PathBuf::from(&args[1]), std::path::PathBuf::from(&args[2]));
    let amount: f32 = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(100.0);
    if !sk_ai::denoise::available() {
        sk_ai::denoise::download(&|p| eprint!("\rModelle laden {:3.0} %", p * 100.0))?;
        eprintln!();
    }
    let denoiser = sk_ai::denoise::Denoiser::default();
    let tag = format!("{}{}", sk_ai::denoise::tag(), if linear { "-linear" } else { "" });
    sk_core::ai_denoise::set_provider(&tag, Box::new(move |p, progress| denoiser.product_ext(p, linear, progress)));
    let start = std::time::Instant::now();
    sk_core::ai_denoise::ensure(&path, &|p| eprint!("\r{:3.0} %", p * 100.0))?;
    eprintln!("\nentrauscht in {:.1?}", start.elapsed());
    let (meta, full) = sk_core::raw::load_full(&path)?;
    let crop = |a: f32| {
        let mut adj = sk_core::sidecar::load(&path).adjustments;
        adj.detail.ai_denoise = a;
        let p = sk_core::pipeline::Processor::new(&meta, &adj);
        let (w, h) = p.output_size(full.width, full.height);
        let r = p.render(&full, Some(sk_core::geometry::Region { x: w / 2 - 400, y: h / 2 - 300, w: 800, h: 600 }), None);
        image::RgbaImage::from_raw(r.width as u32, r.height as u32, r.rgba).unwrap()
    };
    crop(0.0).save(out.join("ohne.png"))?;
    crop(amount).save(out.join("mit.png"))?;
    Ok(())
}
