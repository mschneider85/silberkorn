//! Merges images from the command line: `merge hdr|pano <raw>...` (writes next to the first).

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_default();
    let files: Vec<PathBuf> = args.map(PathBuf::from).collect();
    let progress = |p: f32| eprint!("\r{:3.0} %", p * 100.0);
    let t = std::time::Instant::now();
    let out = match mode.as_str() {
        "hdr" => sk_core::hdr::merge_files(&files, &Default::default(), &progress)?,
        "pano" => sk_core::panorama::stitch_files(&files, &Default::default(), &progress)?,
        _ => anyhow::bail!("Aufruf: merge hdr|pano <raw>..."),
    };
    eprintln!("\n{} in {:.1} s", out.display(), t.elapsed().as_secs_f32());
    Ok(())
}
