//! Writes "before | after | references" side by side for "Look angleichen", with sky and subject
//! left out or weighted as in the app:
//! `cargo run --release -p sk-ai --example look -- <out.png> <raw> <reference raw or picture>...`
//! (`--no-ai`: without regions; `SK_LOOK_DEBUG=1` prints the parts of the similarity).

use std::path::Path;

use sk_core::look_match::{LookOptions, LookStats, match_look, no_regions, picture_thumbnail};
use sk_core::{pipeline::Processor, raw, sidecar};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let no_ai = args.iter().any(|a| a == "--no-ai");
    let args: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let (out, target) = (Path::new(args[0]), Path::new(args[1]));
    let mut models = sk_ai::auto::AutoModels::default();
    let mut regions = |w: usize, h: usize, rgba: &[u8]| if no_ai { no_regions(w, h, rgba) } else { models.look_regions(w, h, rgba) };
    let mut refs = Vec::new();
    let mut thumbs = Vec::new();
    for r in &args[2..] {
        let r = Path::new(r);
        if let Ok((meta, preview)) = raw::load_preview(r, 600) {
            let adj = sidecar::load(r).adjustments;
            refs.push(LookStats::of_development(&meta, &adj, &preview, &mut regions).expect("Referenz"));
            let x = Processor::new(&meta, &adj).render_cpu(&preview, None, None);
            thumbs.push(image::RgbaImage::from_raw(x.width as u32, x.height as u32, x.rgba).unwrap());
        } else {
            refs.push(LookStats::of_file(r, &mut regions)?);
            thumbs.push(picture_thumbnail(r, 600)?);
        }
    }
    let (meta, preview) = raw::load_preview(target, 600)?;
    let before = sidecar::load(target).adjustments;
    let t = std::time::Instant::now();
    let m = match_look(&refs, &meta, &before, &preview, LookOptions::default(), &mut regions).expect("angeglichen");
    println!("in {:?} · Ähnlichkeit {:?} · Gewichte {:?}", t.elapsed(), m.similarity, m.weights);
    let after = m.adjustments;
    println!("Belichtung {:+.2} · {:?} · Sättigung {} · Kurve {:?}", after.exposure, after.white_balance, after.saturation, after.curves.luma.points);
    println!("Bänder {:?}", after.color_editor.bands.map(|b| (b.hue, b.saturation, b.lightness)));
    println!("Farbbalance {:?}", after.color_balance);
    let a = Processor::new(&meta, &before).render_cpu(&preview, None, None);
    let b = Processor::new(&meta, &after).render_cpu(&preview, None, None);
    let (w, h) = (a.width as u32, a.height as u32);
    let width = 2 * (w + 8) + thumbs.iter().map(|t| t.width() * h / t.height() + 8).sum::<u32>();
    let mut img = image::RgbaImage::from_pixel(width, h, image::Rgba([0, 0, 0, 255]));
    image::imageops::overlay(&mut img, &image::RgbaImage::from_raw(w, h, a.rgba).unwrap(), 0, 0);
    image::imageops::overlay(&mut img, &image::RgbaImage::from_raw(w, h, b.rgba).unwrap(), (w + 8) as i64, 0);
    let mut x = 2 * (w + 8);
    for t in thumbs {
        let t = image::imageops::resize(&t, t.width() * h / t.height(), h, image::imageops::FilterType::Triangle);
        image::imageops::overlay(&mut img, &t, x as i64, 0);
        x += t.width() + 8;
    }
    image::DynamicImage::ImageRgba8(img).to_rgb8().save(out)?;
    Ok(())
}
