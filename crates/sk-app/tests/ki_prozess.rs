//! The networks of ONNX Runtime in the separate process (`sk_ai::worker`), with the real program
//! as that process. Needs ONNX Runtime (`SILBERKORN_ORT`) and the eye model; skipped without.

use sk_ai::runtime::{Data, Input, Net};

/// Loading and running there; if the process is killed, the network goes on with tract and
/// gives the same result, and the next network starts a new process.
#[test]
#[cfg(unix)]
fn networks_survive_the_process_ending() {
    let path = sk_ai::models::path(&sk_ai::models::EYE_STATE);
    if std::env::var_os("SILBERKORN_ORT").is_none() || !path.exists() {
        return;
    }
    sk_ai::worker::set_command(env!("CARGO_BIN_EXE_silberkorn").into(), vec![sk_ai::worker::ARG.into()]);
    let inputs = [Input::F32([1, 3, 32, 32])];
    let data: Vec<f32> = (0..3 * 32 * 32).map(|i| (i % 7) as f32 / 7.0).collect();

    let net = Net::load(&path, &inputs).unwrap();
    assert_eq!(net.device(), "Prozessor (ONNX Runtime)");
    let remote = net.run(&[Data::F32(&data)]).unwrap();
    assert_eq!(remote[0].data.len(), 2);

    let killed = std::process::Command::new("pkill").args(["-KILL", "-f", "--", sk_ai::worker::ARG]).status().unwrap();
    assert!(killed.success());
    let after = net.run(&[Data::F32(&data)]).unwrap();
    assert_eq!(net.device(), "Prozessor");
    for (a, b) in remote[0].data.iter().zip(&after[0].data) {
        assert!((a - b).abs() < 1e-4, "{a} {b}");
    }

    // A new process for the next network.
    let again = Net::load(&path, &inputs).unwrap();
    assert_eq!(again.device(), "Prozessor (ONNX Runtime)");
    assert_eq!(again.run(&[Data::F32(&data)]).unwrap()[0].data.len(), 2);
    drop(again);
    sk_ai::worker::shutdown();
}
