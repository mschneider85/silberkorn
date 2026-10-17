//! Writes "Film Standard | camera curve | embedded JPEG" side by side for raw files:
//! `cargo run --release -p sk-core --example camera_look -- <out_dir> <raw>...`

use std::path::Path;

use sk_core::params::{Adjustments, BaseCurve};
use sk_core::{camera_look, pipeline::Processor, raw};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = Path::new(&args[0]);
    std::fs::create_dir_all(out)?;
    for path in &args[1..] {
        let path = Path::new(path);
        let (meta, preview) = raw::load_preview(path, 1024)?;
        let t = std::time::Instant::now();
        let curve = camera_look::fit_from_file(&meta, &preview);
        let took = t.elapsed();
        let film = Adjustments::default();
        let camera = Adjustments { base_curve: BaseCurve::Camera, camera_curve: curve.clone(), ..Default::default() };
        let small = preview.resize_area(preview.width * 600 / preview.width.max(preview.height), preview.height * 600 / preview.width.max(preview.height));
        let a = Processor::new(&meta, &film).render_cpu(&small, None, None);
        let b = Processor::new(&meta, &camera).render_cpu(&small, None, None);
        let j = raw::embedded_preview(path)?.resize_exact(a.width as u32, a.height as u32, image::imageops::FilterType::Triangle).to_rgb8();
        let mean = |px: &mut dyn Iterator<Item = [u8; 3]>| {
            let (s, n) = px.fold((0.0f32, 0), |(s, n), p| (s + 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32, n + 1));
            s / n as f32 / 255.0
        };
        let m = |r: &sk_core::pipeline::Rendered| mean(&mut r.rgba.as_chunks::<4>().0.iter().map(|p| [p[0], p[1], p[2]]));
        println!(
            "{}: {:?} Punkte in {took:?} | Mittel Film {:.3} · Kamera-Kurve {:.3} · JPEG {:.3} | Grau -> {:.3}",
            path.file_name().unwrap().to_string_lossy(),
            curve.as_ref().map(|c| c.points.len()),
            m(&a),
            m(&b),
            mean(&mut j.pixels().map(|p| p.0)),
            curve.as_ref().map_or(f32::NAN, |c| c.eval(0.18))
        );
        let (w, h) = (a.width, a.height);
        let mut img = image::RgbImage::new((w * 3 + 16) as u32, h as u32);
        for (k, r) in [&a.rgba, &b.rgba].into_iter().enumerate() {
            for y in 0..h {
                for x in 0..w {
                    let p = &r[(y * w + x) * 4..];
                    img.put_pixel((k * (w + 8) + x) as u32, y as u32, image::Rgb([p[0], p[1], p[2]]));
                }
            }
        }
        for y in 0..h {
            for x in 0..w {
                img.put_pixel((2 * (w + 8) + x) as u32, y as u32, *j.get_pixel(x as u32, y as u32));
            }
        }
        img.save(out.join(format!("{}.png", path.file_stem().unwrap().to_string_lossy())))?;
    }
    Ok(())
}
