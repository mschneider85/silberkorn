//! Content search from the command line: `content "<Suchtext>" <jpeg|raw>...` lists the images by
//! how well the text describes them (probability and similarity), with the keywords suggested
//! for each and the time per image.

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let query = args.next().expect("Suchtext");
    let t = std::time::Instant::now();
    let images = sk_ai::content::ImageEncoder::load()?;
    let text = sk_ai::content::TextEncoder::load()?;
    eprintln!("Geladen in {:.1} s", t.elapsed().as_secs_f32());
    let t = std::time::Instant::now();
    let q = text.embed(&query)?;
    eprintln!("Text in {:.2} s", t.elapsed().as_secs_f32());
    let t = std::time::Instant::now();
    let mut found = Vec::new();
    for path in args.map(std::path::PathBuf::from) {
        let img = if sk_core::raw::is_raw_file(&path) { sk_core::raw::embedded_preview(&path)? } else { image::open(&path)? };
        let e = images.embed(&img.to_rgb8())?;
        let keywords = sk_ai::content::Keywords::shipped().map(|k| k.suggest(&e, &[], 8).join(", ")).unwrap_or_default();
        found.push((sk_ai::content::match_probability(&e, &q), sk_ai::content::similarity(&e, &q), path, keywords));
    }
    eprintln!("{:.2} s je Bild", t.elapsed().as_secs_f32() / found.len().max(1) as f32);
    found.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (p, s, path, keywords) in found {
        println!("{p:.4}  {s:.3}  {}  [{keywords}]", path.display());
    }
    Ok(())
}
