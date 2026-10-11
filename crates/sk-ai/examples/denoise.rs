//! AI denoising from the command line: `denoise <raw> [strength 0..1]` (the model must be downloaded).

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = std::path::PathBuf::from(args.next().expect("RAW-Datei"));
    let strength: f32 = args.next().map(|s| s.parse()).transpose()?.unwrap_or(0.7);
    let model_path = sk_ai::models::path(&sk_ai::models::DENOISE);
    if !model_path.exists() {
        eprintln!("Lade Modell …");
        sk_ai::models::download(&sk_ai::models::DENOISE, &|p| eprint!("\r{:3.0} %", p * 100.0))?;
    }
    let t = std::time::Instant::now();
    let model = sk_ai::denoise::Denoiser::load(&model_path)?;
    let out = sk_ai::denoise::denoise_file(&path, strength, &model, &|p| eprint!("\r{:3.0} %", p * 100.0))?;
    eprintln!("\n{} in {:.1} s", out.display(), t.elapsed().as_secs_f32());
    Ok(())
}
