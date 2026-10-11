//! Prints capture metadata: `cargo run --release --example meta -- <raw>...`

fn main() -> anyhow::Result<()> {
    for arg in std::env::args().skip(1) {
        let t = std::time::Instant::now();
        let meta = cr_core::raw::read_meta(std::path::Path::new(&arg))?;
        println!("{arg}: {:?} ({:?})", meta.info, t.elapsed());
    }
    Ok(())
}
