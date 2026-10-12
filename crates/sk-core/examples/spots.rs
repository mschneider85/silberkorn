//! Finds dust and blemishes in a raw file and marks them: `spots <raw> <out.png> [sensitivity 0..1]`.

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let sensitivity: f32 = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(0.5);
    let (meta, preview) = sk_core::raw::load_preview(std::path::Path::new(&args[1]), 2048)?;
    let t = std::time::Instant::now();
    let found = sk_core::spots::find(&preview, meta.as_shot_wb, sensitivity, 60);
    println!("{} Stellen in {:?}", found.len(), t.elapsed());
    for f in &found {
        println!("  {:.3} {:.3}  r {:.4}  {:.1}", f.center[0], f.center[1], f.radius, f.score);
    }
    let adj = sk_core::params::Adjustments::default();
    let r = sk_core::pipeline::Processor::new(&meta, &adj).render(&preview, None, None);
    let mut img = image::RgbaImage::from_raw(r.width as u32, r.height as u32, r.rgba).unwrap();
    let long = r.width.max(r.height) as f32;
    for f in &found {
        let (cx, cy, rad) = (f.center[0] * r.width as f32, f.center[1] * r.height as f32, (f.radius * long).max(4.0) + 3.0);
        for a in 0..90 {
            let t = a as f32 / 90.0 * std::f32::consts::TAU;
            let (x, y) = ((cx + t.cos() * rad) as i64, (cy + t.sin() * rad) as i64);
            if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
                img.put_pixel(x as u32, y as u32, image::Rgba([255, 40, 40, 255]));
            }
        }
    }
    img.save(&args[2])?;
    Ok(())
}
