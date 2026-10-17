//! Writes "as shot | auto" side by side for raw files, with the settings auto chose:
//! `cargo run --release -p sk-ai --example auto -- <out_dir> <raw>...` (`--no-ai`: without faces
//! and subject, `--film`: on Film Standard instead of the camera's look).

use std::path::Path;

use sk_core::auto::{AutoOptions, Focus, auto_adjust, guide_image};
use sk_core::{params::Adjustments, pipeline::Processor, raw};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let no_ai = args.iter().any(|a| a == "--no-ai");
    let film = args.iter().any(|a| a == "--film");
    let args: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let out = Path::new(args[0]);
    std::fs::create_dir_all(out)?;
    let opts = AutoOptions::default();
    let mut models = sk_ai::auto::AutoModels::default();
    for path in &args[1..] {
        let path = Path::new(path);
        let (meta, preview) = raw::load_preview(path, 2048)?;
        // As the app opens a new image: with the camera's look (`--film`: without).
        let adj = Adjustments::default();
        let adj = if film { adj } else { sk_core::camera_look::apply_if_wanted(&meta, &preview, &adj, true).unwrap_or(adj) };
        let t = std::time::Instant::now();
        let focus = if no_ai { Focus::default() } else { models.focus(&guide_image(&meta, &adj, &preview, &opts)) };
        let t_focus = t.elapsed();
        let camera = sk_core::auto::camera_brightness(&meta);
        let auto = auto_adjust(&meta, &adj, &preview, &opts, &focus, camera);
        println!(
            "{}: {} Gesichter, Motiv {} | {:?} + {:?} | WB {}K/{} (Kamera {:?}) Bel {:+.2} L {} S {} W {} B {} K {} D {}",
            path.file_name().unwrap().to_string_lossy(),
            focus.faces.len(),
            focus.subject.is_some(),
            t_focus,
            t.elapsed() - t_focus,
            auto.white_balance.kelvin,
            auto.white_balance.tint,
            meta.color.temp_tint_for_wb(meta.as_shot_wb),
            auto.exposure,
            auto.highlights,
            auto.shadows,
            auto.whites,
            auto.blacks,
            auto.contrast,
            auto.vibrance
        );
        let mean_luma = |rgba: &[u8]| rgba.as_chunks::<4>().0.iter().map(|p| 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32).sum::<f32>() / (rgba.len() / 4) as f32 / 255.0;
        let small_m = preview.resize_area(preview.width * 300 / preview.width.max(preview.height), preview.height * 300 / preview.width.max(preview.height));
        let def = mean_luma(&Processor::new(&meta, &adj).render_cpu(&small_m, None, None).rgba);
        let aut = mean_luma(&Processor::new(&meta, &auto).render_cpu(&small_m, None, None).rgba);
        let jpg = raw::embedded_preview(path).map(|i| {
            let i = i.to_rgba8();
            mean_luma(i.as_raw())
        });
        println!("  Mittel: Kamera-JPEG {:.3} / {:.3} · 0 EV {:.3} · Auto {:.3}", jpg.unwrap_or(f32::NAN), camera.unwrap_or(f32::NAN), def, aut);
        let small = preview.resize_area(preview.width * 700 / preview.width.max(preview.height), preview.height * 700 / preview.width.max(preview.height));
        let a = Processor::new(&meta, &adj).render_cpu(&small, None, None);
        let b = Processor::new(&meta, &auto).render_cpu(&small, None, None);
        let (w, h) = (a.width + b.width + 8, a.height.max(b.height));
        let mut img = image::RgbImage::new(w as u32, h as u32);
        for (r, x0) in [(&a, 0), (&b, a.width + 8)] {
            for y in 0..r.height {
                for x in 0..r.width {
                    let p = &r.rgba[(y * r.width + x) * 4..];
                    img.put_pixel((x0 + x) as u32, y as u32, image::Rgb([p[0], p[1], p[2]]));
                }
            }
        }
        img.save(out.join(format!("{}.png", path.file_stem().unwrap().to_string_lossy())))?;
        if let Ok(j) = raw::embedded_preview(path) {
            j.thumbnail(700, 700).to_rgb8().save(out.join(format!("{}-kamera.png", path.file_stem().unwrap().to_string_lossy())))?;
        }
    }
    Ok(())
}
