//! Loads an ONNX model with a fixed input size and times one run: `onnx_probe <model> <size>`.

use tract_onnx::prelude::*;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("Modell");
    let size: usize = args.next().map(|s| s.parse().unwrap()).unwrap_or(256);
    let model = tract_onnx::onnx().model_for_path(&path)?;
    for (i, inp) in model.inputs.iter().enumerate() {
        println!("input {i}: {:?} {:?}", model.node(inp.node).name, model.outlet_fact(*inp)?);
    }
    for (i, out) in model.outputs.iter().enumerate() {
        println!("output {i}: {:?} {:?}", model.node(out.node).name, model.outlet_fact(*out)?);
    }
    let t = std::time::Instant::now();
    let plan = model.with_input_fact(0, f32::fact([1, 3, size, size]).into())?.into_optimized()?.into_runnable()?;
    println!("optimiert in {:.1} s", t.elapsed().as_secs_f32());
    let input: Tensor = tract_ndarray::Array4::<f32>::from_shape_fn((1, 3, size, size), |(_, c, y, x)| 0.4 + 0.1 * c as f32 + 0.05 * (((x * 7 + y * 13) % 5) as f32 - 2.0) / 2.0).into();
    for _ in 0..2 {
        let t = std::time::Instant::now();
        let out = plan.run(tvec!(input.clone().into()))?;
        let o = out[0].to_plain_array_view::<f32>()?;
        println!("Lauf: {:.2} s, Ausgabe {:?}, Mittel {:.3}, Werte {:?}", t.elapsed().as_secs_f32(), o.shape(), o.mean().unwrap_or(0.0), o.iter().step_by(o.len() / 6).collect::<Vec<_>>());
    }
    Ok(())
}
