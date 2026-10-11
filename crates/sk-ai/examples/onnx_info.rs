//! Prints the inputs and outputs of an ONNX model: `onnx_info <model>`.

fn main() -> anyhow::Result<()> {
    use tract_onnx::prelude::*;
    let path = std::env::args().nth(1).expect("Modell");
    let model = tract_onnx::onnx().model_for_path(&path)?;
    for inp in &model.inputs {
        println!("input  {:?}: {:?}", model.node(inp.node).name, model.outlet_fact(*inp)?);
    }
    for out in &model.outputs {
        println!("output {:?}: {:?}", model.node(out.node).name, model.outlet_fact(*out)?);
    }
    let mut ops: Vec<String> = model.nodes.iter().map(|n| n.op.name().to_string()).collect();
    ops.sort();
    ops.dedup();
    println!("ops: {}", ops.join(" "));
    Ok(())
}
