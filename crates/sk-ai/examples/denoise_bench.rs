//! Measures the denoiser's throughput with 1 and n threads: `denoise_bench <n>`.

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> anyhow::Result<()> {
    use rayon::prelude::*;
    let n: usize = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(12);
    let model = sk_ai::denoise::Denoiser::load(&sk_ai::models::path(&sk_ai::models::DENOISE))?;
    let tile = 256usize;
    let img: Vec<[f32; 3]> = (0..tile * tile).map(|i| [0.4 + ((i * 7) % 13) as f32 * 0.01; 3]).collect();
    let t = std::time::Instant::now();
    model.run(tile, tile, &img, &|_| {})?;
    let one = t.elapsed().as_secs_f32();
    println!("1 Kachel: {one:.2} s");
    let pool = rayon::ThreadPoolBuilder::new().num_threads(n).build()?;
    let t = std::time::Instant::now();
    pool.install(|| (0..n).into_par_iter().try_for_each(|_| model.run(tile, tile, &img, &|_| {}).map(|_| ())))?;
    let par = t.elapsed().as_secs_f32();
    println!("{n} Kacheln parallel: {par:.2} s (Beschleunigung {:.1}×)", one * n as f32 / par);
    Ok(())
}
