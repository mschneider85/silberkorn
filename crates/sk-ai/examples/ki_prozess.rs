//! The depth network in this process and in the separate one (`sk_ai::worker`), timed; then the
//! separate process is killed and the network must go on with tract:
//! `cargo run --release -p sk-ai --example ki_prozess -- <silberkorn executable> [runs]`
//! (Windows: ends the process with PowerShell; elsewhere with pkill).

use std::time::Instant;

use sk_ai::runtime::{Data, Input, Net};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let runs: usize = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(5);
    let path = sk_ai::models::path(&sk_ai::models::DEPTH);
    let (w, h) = (518, 392);
    let inputs = [Input::F32([1, 3, h, w])];
    let data: Vec<f32> = (0..3 * w * h).map(|i| (i % 255) as f32 / 255.0).collect();
    let time = |name: &str, net: &Net| -> anyhow::Result<Vec<f32>> {
        let t = Instant::now();
        let out = net.run(&[Data::F32(&data)])?;
        let first = t.elapsed();
        let t = Instant::now();
        for _ in 1..runs {
            net.run(&[Data::F32(&data)])?;
        }
        println!("{name} ({}): erster Lauf {first:?}, danach je {:?}", net.device(), t.elapsed() / (runs.max(2) - 1) as u32);
        Ok(out.into_iter().next().map(|o| o.data).unwrap_or_default())
    };

    let t = Instant::now();
    let local = Net::load(&path, &inputs)?;
    println!("Laden im Prozess {:?}", t.elapsed());
    let a = time("im Prozess", &local)?;
    drop(local);

    sk_ai::worker::set_command(args[1].clone().into(), vec![sk_ai::worker::ARG.into()]);
    let t = Instant::now();
    let remote = Net::load(&path, &inputs)?;
    println!("Laden im KI-Prozess {:?}", t.elapsed());
    let b = time("KI-Prozess", &remote)?;
    let diff = a.iter().zip(&b).map(|(x, y)| (x - y).abs()).fold(0f32, f32::max);
    println!("größte Abweichung {diff}");

    let killed = if cfg!(windows) {
        std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", "Get-CimInstance Win32_Process | Where-Object CommandLine -like '*--ki-prozess*' | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }"])
            .status()?
    } else {
        std::process::Command::new("pkill").args(["-KILL", "-f", "--", sk_ai::worker::ARG]).status()?
    };
    println!("KI-Prozess beendet: {killed}");
    let c = time("nach dem Ende", &remote)?;
    let diff = a.iter().zip(&c).map(|(x, y)| (x - y).abs()).fold(0f32, f32::max);
    println!("größte Abweichung {diff}; Grund: {:?}", sk_ai::runtime::gpu_problem());
    Ok(())
}
