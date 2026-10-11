//! Writes subject/sky/magic masks of a raw file as PNGs:
//! `cargo run --release -p sk-ai --example masks -- <raw> <out_dir> [model.onnx]`

use std::path::Path;

use sk_ai::{MaskImage, SubjectModel, guide_from_rgba, magic_mask, sky_mask};
use sk_core::{params::Adjustments, pipeline::Processor, raw};

fn save(m: &MaskImage, path: &Path) -> anyhow::Result<()> {
    image::GrayImage::from_raw(m.width as u32, m.height as u32, m.data.clone()).unwrap().save(path)?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let out = Path::new(&args[2]);
    std::fs::create_dir_all(out)?;
    let (meta, preview) = raw::load_preview(Path::new(&args[1]), 1024)?;
    let r = Processor::new(&meta, &Adjustments::default()).render(&preview, None, None);
    let guide = guide_from_rgba(r.width, r.height, &r.rgba);
    let t = std::time::Instant::now();
    save(&sky_mask(&guide), &out.join("sky.png"))?;
    println!("sky {:?}", t.elapsed());
    let t = std::time::Instant::now();
    save(&magic_mask(&guide, [0.2, 0.85], 0.3), &out.join("magic.png"))?;
    println!("magic {:?}", t.elapsed());
    if let Some(model) = args.get(3) {
        let t = std::time::Instant::now();
        let m = SubjectModel::load(Path::new(model))?;
        save(&m.subject_mask(&guide)?, &out.join("subject.png"))?;
        println!("subject {:?}", t.elapsed());
    }
    image::RgbaImage::from_raw(r.width as u32, r.height as u32, r.rgba)
        .unwrap()
        .save(out.join("image.png"))?;
    Ok(())
}
