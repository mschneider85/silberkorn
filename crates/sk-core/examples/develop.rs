//! Develops a raw file from the command line: `cargo run --release --example develop -- <raw> [out.jpg]`

use std::time::Instant;

use sk_core::{export, params::Adjustments, pipeline::Processor, raw};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let path = std::path::Path::new(args.get(1).expect("usage: develop <raw> [out.jpg]"));

    let t = Instant::now();
    let (meta, preview) = raw::load_preview(path, 2048)?;
    println!("preview {}x{} in {:?}", preview.width, preview.height, t.elapsed());
    println!("{:?}", meta.info);
    println!("as-shot wb {:?} -> {:?}", meta.as_shot_wb, meta.color.temp_tint_for_wb(meta.as_shot_wb));

    // Optional adjustments as JSON, e.g. SK_ADJ='{"exposure":0.5,"shadows":60}'
    let adj: Adjustments = match std::env::var("SK_ADJ") {
        Ok(json) => serde_json::from_str(&json)?,
        Err(_) => Adjustments::default(),
    };
    let t = Instant::now();
    let r = Processor::new(&meta, &adj).render(&preview, None, None);
    println!("render {}x{} in {:?}", r.width, r.height, t.elapsed());

    let recipe = export::ExportRecipe {
        output_dir: args.get(2).map(std::path::PathBuf::from).unwrap_or("Output".into()),
        long_edge: Some(1200),
        suffix: std::env::var("SK_SUFFIX").unwrap_or_default(),
        ..Default::default()
    };
    let t = Instant::now();
    let out = export::export(path, &adj, &recipe)?;
    println!("export {} in {:?}", out.display(), t.elapsed());
    Ok(())
}
