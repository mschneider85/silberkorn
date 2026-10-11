//! Removes a vertical line from a raw file and writes a JPEG before/after crop:
//! `remove <raw> <u> <v0> <v1> <radius> <out.png>`.

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let path = std::path::PathBuf::from(&a[0]);
    let (u, v0, v1, r): (f32, f32, f32, f32) = (a[1].parse()?, a[2].parse()?, a[3].parse()?, a[4].parse()?);
    let (meta, full) = sk_core::raw::load_full(&path)?;
    let model = sk_ai::inpaint::Inpainter::load(&sk_ai::models::path(&sk_ai::models::INPAINT))?;
    let points: Vec<[f32; 2]> = (0..=20).map(|i| [u, v0 + (v1 - v0) * i as f32 / 20.0]).collect();
    let stroke = sk_core::params::BrushStroke { points, radius: r, hardness: 0.7, flow: 1.0, erase: false };
    let t = std::time::Instant::now();
    let removal = sk_ai::inpaint::make_removal(&full, meta.as_shot_wb, &[stroke], &model)?;
    eprintln!("Entfernt in {:.1} s, Flicken {} Zeichen", t.elapsed().as_secs_f32(), removal.patch.len());
    let mut adj = sk_core::params::Adjustments::default();
    let before = sk_core::pipeline::Processor::new(&meta, &adj).render(&full, None, None);
    adj.removals.push(removal);
    let after = sk_core::pipeline::Processor::new(&meta, &adj).render(&full, None, None);
    let (w, h) = (before.width as u32, before.height as u32);
    let crop = |r: &sk_core::pipeline::Rendered| {
        let img = image::RgbaImage::from_raw(w, h, r.rgba.clone()).unwrap();
        let (cx, cy) = ((u * w as f32) as u32, ((v0 + v1) / 2.0 * h as f32) as u32);
        image::imageops::crop_imm(&img, cx.saturating_sub(400), cy.saturating_sub(450), 800, 900).to_image()
    };
    let (b, af) = (crop(&before), crop(&after));
    let mut out = image::RgbaImage::new(b.width() * 2, b.height());
    image::imageops::replace(&mut out, &b, 0, 0);
    image::imageops::replace(&mut out, &af, b.width() as i64, 0);
    image::imageops::resize(&out, out.width() / 2, out.height() / 2, image::imageops::FilterType::Triangle).save(&a[5])?;
    Ok(())
}
