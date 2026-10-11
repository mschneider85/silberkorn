//! Face and culling analysis of image files from the command line: `faces <jpeg|raw>...`.

fn main() -> anyhow::Result<()> {
    let models = sk_ai::faces::FaceModels::load()?;
    for path in std::env::args().skip(1).map(std::path::PathBuf::from) {
        let img = if sk_core::raw::is_raw_file(&path) { sk_core::raw::embedded_preview(&path)? } else { image::open(&path)? };
        let rgb8 = img.to_rgb8();
        let rgb: Vec<[f32; 3]> = rgb8.pixels().map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]).collect();
        let a = models.analyze(&sk_ai::faces::Image { width: rgb8.width() as usize, height: rgb8.height() as usize, rgb: &rgb })?;
        println!(
            "{}: {} Gesicht(er), Schärfe {:.3}, Augen geschlossen {:?}",
            path.display(),
            a.faces.len(),
            a.sharpness,
            a.faces.iter().map(|f| format!("{:.2}", f.eyes_closed)).collect::<Vec<_>>()
        );
    }
    Ok(())
}
