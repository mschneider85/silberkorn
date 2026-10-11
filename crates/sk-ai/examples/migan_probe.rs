//! Runs the MI-GAN pipeline on a synthetic image: `migan_probe <model> <size> <hole value>`.

use tract_onnx::prelude::*;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("Modell");
    let size: usize = args.next().map(|s| s.parse().unwrap()).unwrap_or(512);
    let hole: u8 = args.next().map(|s| s.parse().unwrap()).unwrap_or(0);
    let t = std::time::Instant::now();
    let plan = tract_onnx::onnx()
        .model_for_path(&path)?
        .with_input_fact(0, u8::fact([1, 3, size, size]).into())?
        .with_input_fact(1, u8::fact([1, 1, size, size]).into())?
        .into_optimized()?
        .into_runnable()?;
    println!("geladen in {:.1} s", t.elapsed().as_secs_f32());
    // Horizontal stripes; a square hole in the middle.
    let img = tract_ndarray::Array4::<u8>::from_shape_fn((1, 3, size, size), |(_, c, y, _)| if (y / 16) % 2 == 0 { 200 - c as u8 * 30 } else { 60 });
    let (a, b) = (size * 3 / 8, size * 5 / 8);
    let mask = tract_ndarray::Array4::<u8>::from_shape_fn((1, 1, size, size), |(_, _, y, x)| if (a..b).contains(&x) && (a..b).contains(&y) { hole } else { 255 - hole });
    let t = std::time::Instant::now();
    let out = plan.run(tvec!(Tensor::from(img).into(), Tensor::from(mask).into()))?;
    println!("Lauf {:.2} s: {:?} {:?}", t.elapsed().as_secs_f32(), out[0].datum_type(), out[0].shape());
    let o = out[0].cast_to::<f32>()?;
    let v = o.to_plain_array_view::<f32>()?;
    let c = size / 2;
    let col: Vec<f32> = (a..b).step_by(8).map(|y| v[[0, 0, y, c]]).collect();
    println!("Spalte in der Mitte (Loch): {col:?}");
    println!("außen: {:?}", (0..64).step_by(8).map(|y| v[[0, 0, y, 10]]).collect::<Vec<_>>());
    Ok(())
}
