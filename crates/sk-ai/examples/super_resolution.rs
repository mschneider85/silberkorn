//! Super Resolution of one raw file: `super_resolution <raw> <out-dir>`. Writes `<raw>-SR.dng`
//! next to the file and a crop of 800 x 600 output pixels as PNG: the original enlarged with
//! Lanczos (`lanczos.png`) and the result (`sr.png`), and the crop of the original without
//! enlarging (`original.png`). `--nur-ausschnitt` writes only `original.png`.
//! `--entrauschen` enlarges the image with KI-Entrauschen at 100 % (the model is downloaded if
//! needed).

use sk_core::geometry::Region;

fn main() -> anyhow::Result<()> {
    let only_crop = std::env::args().any(|a| a == "--nur-ausschnitt");
    let denoise = std::env::args().any(|a| a == "--entrauschen");
    let args: Vec<String> = std::env::args().filter(|a| !a.starts_with("--")).collect();
    let (path, out) = (std::path::PathBuf::from(&args[1]), std::path::PathBuf::from(&args[2]));
    let render = |p: &std::path::Path, k: usize| -> anyhow::Result<image::RgbaImage> {
        let (meta, full) = sk_core::raw::load_full(p)?;
        let pr = sk_core::pipeline::Processor::new(&meta, &sk_core::params::Adjustments::default());
        let (w, h) = pr.output_size(full.width, full.height);
        let (cw, ch) = (800 / k, 600 / k);
        let r = pr.render(&full, Some(Region { x: w / 2 - cw / 2, y: h / 2 - ch / 2, w: cw, h: ch }), None);
        Ok(image::RgbaImage::from_raw(r.width as u32, r.height as u32, r.rgba).unwrap())
    };
    if only_crop {
        return Ok(render(&path, 2)?.save(out.join("original.png"))?);
    }
    let mut enhanced = sk_ai::super_resolution::Enhanced::default();
    if denoise {
        if !sk_ai::denoise::available() {
            sk_ai::denoise::download(&|p| eprint!("\rModelle laden {:3.0} %", p * 100.0))?;
            eprintln!();
        }
        sk_ai::denoise::register();
        enhanced.ai_denoise = 1.0;
    }
    let start = std::time::Instant::now();
    let dng = sk_ai::super_resolution::enlarge_file(&path, enhanced, &|p| eprint!("\r{:3.0} %", p * 100.0))?;
    eprintln!("\n{} in {:.1?}", dng.display(), start.elapsed());
    let small = render(&path, 2)?;
    small.save(out.join("original.png"))?;
    image::imageops::resize(&small, 800, 600, image::imageops::FilterType::Lanczos3).save(out.join("lanczos.png"))?;
    render(&dng, 1)?.save(out.join("sr.png"))?;
    Ok(())
}
