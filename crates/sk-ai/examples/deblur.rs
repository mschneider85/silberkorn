//! Removing camera shake from one raw file: `deblur <raw> <out-dir> [amount]`. Writes crops at
//! 100 % (center and upper left corner) without and with it as PNG. The model is downloaded if
//! needed.

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (path, out) = (std::path::PathBuf::from(&args[1]), std::path::PathBuf::from(&args[2]));
    let amount: f32 = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(100.0);
    if !sk_ai::deblur::available() {
        sk_ai::deblur::download(&|p| eprint!("\rModell laden {:3.0} %", p * 100.0))?;
        eprintln!();
    }
    sk_ai::deblur::register();
    let start = std::time::Instant::now();
    sk_core::ai_deblur::ensure(&path, &|p| eprint!("\r{:3.0} %", p * 100.0))?;
    eprintln!("\nberechnet in {:.1?} ({})", start.elapsed(), sk_ai::runtime::last_device().unwrap_or("?"));
    let (meta, full) = sk_core::raw::load_full(&path)?;
    let crop = |a: f32, center: bool| {
        let mut adj = sk_core::sidecar::load(&path).adjustments;
        adj.detail.ai_deblur = a;
        let p = sk_core::pipeline::Processor::new(&meta, &adj);
        let (w, h) = p.output_size(full.width, full.height);
        let (x, y) = if center { (w / 2 - 400, h / 2 - 300) } else { (0, 0) };
        let r = p.render(&full, Some(sk_core::geometry::Region { x, y, w: 800, h: 600 }), None);
        image::RgbaImage::from_raw(r.width as u32, r.height as u32, r.rgba).unwrap()
    };
    for (center, name) in [(true, "mitte"), (false, "ecke")] {
        crop(0.0, center).save(out.join(format!("{name}-ohne.png")))?;
        crop(amount, center).save(out.join(format!("{name}-mit.png")))?;
    }
    Ok(())
}
