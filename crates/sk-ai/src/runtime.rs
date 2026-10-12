//! Where the networks run: on the graphics card through ONNX Runtime with DirectML (Windows), or
//! on the processor with tract. ONNX Runtime is not linked but loaded at run time from DLLs that
//! are downloaded on request (`install_gpu`), so the program stays a single file and builds
//! with any toolchain. Without them, or if anything fails, everything runs on the processor.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result, anyhow, bail};
use tract_onnx::prelude::*;

/// Use the graphics card when possible (user setting).
static GPU_ENABLED: AtomicBool = AtomicBool::new(true);

pub fn set_gpu_enabled(on: bool) {
    if GPU_ENABLED.swap(on, Ordering::Relaxed) != on {
        CONFIG.fetch_add(1, Ordering::Relaxed);
    }
}

/// Changes whenever networks should be loaded again (setting changed, support installed).
static CONFIG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn config_generation() -> u64 {
    CONFIG.load(Ordering::Relaxed)
}

/// Where the most recently loaded network runs (for display).
static LAST_DEVICE: Mutex<Option<&'static str>> = Mutex::new(None);

pub fn last_device() -> Option<&'static str> {
    *LAST_DEVICE.lock().unwrap()
}

pub fn gpu_enabled() -> bool {
    GPU_ENABLED.load(Ordering::Relaxed)
}

/// ONNX Runtime with DirectML, and DirectML itself, from Microsoft's NuGet packages: (package
/// URL, file inside, file name on disk, SHA-256 of the file). The libraries are loaded into the
/// process, so only exactly these files are accepted.
const ORT_VERSION: &str = "1.24.4";
const DIRECTML_VERSION: &str = "1.15.4";

fn gpu_files() -> [(String, &'static str, &'static str, &'static str); 2] {
    [
        (
            format!("https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime.directml/{ORT_VERSION}/microsoft.ml.onnxruntime.directml.{ORT_VERSION}.nupkg"),
            "runtimes/win-x64/native/onnxruntime.dll",
            "onnxruntime.dll",
            "e7eedec6a6f26dc39dc948276a75ef6d2bee3fff944d874ceed0bbd3b97bff40",
        ),
        (
            format!("https://api.nuget.org/v3-flatcontainer/microsoft.ai.directml/{DIRECTML_VERSION}/microsoft.ai.directml.{DIRECTML_VERSION}.nupkg"),
            "bin/x64-win/DirectML.dll",
            "DirectML.dll",
            "9c9e6d822561c6c41b90e6994b3e8857cf1d66dbfb1e0c4c799c7c89b4e92da1",
        ),
    ]
}

/// Approximate download size of the GPU support.
pub const GPU_DOWNLOAD_MB: u32 = 28;

/// Folder of the downloaded ONNX Runtime files.
pub fn gpu_dir() -> PathBuf {
    sk_core::paths::data_dir().join("gpu")
}

/// The ONNX Runtime library to load: `SILBERKORN_ORT` (any platform, e.g. for tests), else the
/// downloaded DirectML build on Windows.
fn ort_library() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("SILBERKORN_ORT") {
        return Some(PathBuf::from(p));
    }
    cfg!(windows).then(|| gpu_dir().join("onnxruntime.dll")).filter(|p| p.is_file())
}

/// GPU support is possible on this system (Windows, or a library given for testing).
pub fn gpu_supported() -> bool {
    cfg!(windows) || std::env::var_os("SILBERKORN_ORT").is_some()
}

pub fn gpu_installed() -> bool {
    ort_library().is_some_and(|p| p.is_file()) && (!cfg!(windows) || std::env::var_os("SILBERKORN_ORT").is_some() || gpu_dir().join("DirectML.dll").is_file())
}

/// ONNX Runtime, loaded once: `Err` with the reason if it can't be used.
fn ort_ready() -> &'static std::result::Result<(), String> {
    static READY: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    READY.get_or_init(|| {
        let lib = ort_library().ok_or("ONNX Runtime ist nicht installiert")?;
        // DirectML.dll next to onnxruntime.dll must be found first (not an older one of the system).
        #[cfg(windows)]
        if let Some(dir) = lib.parent() {
            let dml = dir.join("DirectML.dll");
            if dml.is_file() {
                unsafe {
                    let _ = libloading::Library::new(&dml).map(std::mem::forget);
                }
            }
        }
        let env = ort::init_from(&lib).map_err(|e| format!("ONNX Runtime nicht ladbar: {e}"))?;
        env.with_name("silberkorn").commit();
        Ok(())
    })
}

/// Why the GPU isn't used, or `None` if it is (once a network has been loaded on it).
pub fn gpu_problem() -> Option<String> {
    if !gpu_supported() {
        return Some("Nur unter Windows (DirectML)".into());
    }
    if !gpu_installed() {
        return Some("Nicht installiert".into());
    }
    match ort_ready() {
        Ok(()) => LAST_GPU_ERROR.lock().unwrap().clone(),
        Err(e) => Some(e.clone()),
    }
}

/// The latest reason a network fell back to the processor.
static LAST_GPU_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// Downloads the GPU support (only the needed files from the NuGet packages, by HTTP ranges).
pub fn install_gpu(progress: &dyn Fn(f32)) -> Result<()> {
    if !cfg!(windows) {
        bail!("Die Grafikkarten-Unterstützung gibt es nur unter Windows");
    }
    let dir = gpu_dir();
    std::fs::create_dir_all(&dir)?;
    let files = gpu_files();
    let n = files.len() as f32;
    for (i, (url, inner, name, sha256)) in files.iter().enumerate() {
        let data = zip_entry(url, inner, &|p| progress((i as f32 + p) / n))?;
        anyhow::ensure!(crate::models::sha256(&data) == *sha256, "{name}: Prüfsumme stimmt nicht, die Datei wird nicht verwendet");
        sk_core::safe_file::write(&dir.join(name), &data)?;
    }
    CONFIG.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

fn get_range(url: &str, range: &str) -> Result<Vec<u8>> {
    let mut resp = ureq::get(url).header("Range", range).call().with_context(|| format!("Download von {url} fehlgeschlagen"))?;
    anyhow::ensure!(resp.status().as_u16() == 206, "Der Server unterstützt keine Teil-Downloads");
    Ok(resp.body_mut().with_config().limit(256 << 20).read_to_vec()?)
}

fn u16le(b: &[u8], i: usize) -> usize {
    u16::from_le_bytes([b[i], b[i + 1]]) as usize
}

fn u32le(b: &[u8], i: usize) -> usize {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize
}

/// One file from a ZIP archive on a web server, without downloading the whole archive: the
/// central directory from its end, then only the file's compressed data.
pub fn zip_entry(url: &str, name: &str, progress: &dyn Fn(f32)) -> Result<Vec<u8>> {
    let tail = get_range(url, "bytes=-65536")?;
    let eocd = tail.windows(4).rposition(|w| w == b"PK\x05\x06").context("Kein ZIP-Archiv")?;
    let (cd_size, cd_off) = (u32le(&tail, eocd + 12), u32le(&tail, eocd + 16));
    let cd = if cd_size <= eocd { tail[eocd - cd_size..eocd].to_vec() } else { get_range(url, &format!("bytes={cd_off}-{}", cd_off + cd_size - 1))? };
    let mut p = 0;
    while p + 46 <= cd.len() && u32le(&cd, p) == 0x0201_4b50 {
        let (method, csize, usize_) = (u16le(&cd, p + 10), u32le(&cd, p + 20), u32le(&cd, p + 24));
        let (nl, el, cl) = (u16le(&cd, p + 28), u16le(&cd, p + 30), u16le(&cd, p + 32));
        let local = u32le(&cd, p + 42);
        let entry = String::from_utf8_lossy(&cd[p + 46..p + 46 + nl]);
        if entry == name {
            // Local header (30 bytes plus its own name and extra field), then the data.
            let head = get_range(url, &format!("bytes={local}-{}", local + 29))?;
            let start = local + 30 + u16le(&head, 26) + u16le(&head, 28);
            let mut data = Vec::with_capacity(csize);
            let chunk = 1 << 20;
            let mut off = 0;
            while off < csize {
                let end = (off + chunk).min(csize);
                data.extend(get_range(url, &format!("bytes={}-{}", start + off, start + end - 1))?);
                off = end;
                progress(off as f32 / csize as f32);
            }
            return match method {
                0 => Ok(data),
                8 => {
                    let mut out = Vec::with_capacity(usize_);
                    flate2::read::DeflateDecoder::new(&data[..]).read_to_end(&mut out)?;
                    anyhow::ensure!(out.len() == usize_, "{name}: unvollständig");
                    Ok(out)
                }
                m => bail!("{name}: Kompression {m} wird nicht unterstützt"),
            };
        }
        p += 46 + nl + el + cl;
    }
    bail!("{name} nicht im Archiv gefunden")
}

// ------------------------------------------------------------------ networks

/// Element type and fixed shape of a network input.
#[derive(Clone, Copy, Debug)]
pub enum Input {
    F32([usize; 4]),
    U8([usize; 4]),
}

/// Input data for a run (in the order of the network's inputs).
pub enum Data<'a> {
    F32(&'a [f32]),
    U8(&'a [u8]),
}

/// A network output, converted to f32.
pub struct Output {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

enum Backend {
    /// tract on the processor.
    Cpu(Arc<TypedRunnableModel>),
    /// ONNX Runtime, on the graphics card (`gpu`) or the processor.
    Ort { session: Mutex<ort::session::Session>, gpu: bool },
}

/// A loaded network, on the graphics card or the processor.
pub struct Net {
    backend: Backend,
    inputs: Vec<Input>,
}

/// An ONNX Runtime session: with DirectML on the graphics card (Windows), else on the processor.
/// Returns the session and whether it runs on the graphics card.
fn ort_session(path: &Path) -> Result<(ort::session::Session, bool)> {
    ort_ready().as_ref().map_err(|e| anyhow!("{e}"))?;
    let new = || ort::session::Session::builder().map_err(|e| anyhow!("{e}"));
    let mut dml_error = None;
    if cfg!(windows) {
        // DirectML needs these off; prefer the dedicated card of laptops with two.
        let dml = new()?
            .with_memory_pattern(false)
            .map_err(|e| anyhow!("{e}"))?
            .with_parallel_execution(false)
            .map_err(|e| anyhow!("{e}"))?
            .with_execution_providers([ort::ep::DirectML::default()
                .with_performance_preference(ort::ep::directml::PerformancePreference::HighPerformance)
                .build()
                .error_on_failure()])
            .map_err(|e| anyhow!("{e}"))
            .and_then(|mut b| b.commit_from_file(path).map_err(|e| anyhow!("{e}")));
        match dml {
            Ok(s) => return Ok((s, true)),
            Err(e) => dml_error = Some(e),
        }
    }
    let session = new()?.commit_from_file(path).map_err(|e| anyhow!("{e}"))?;
    if let Some(e) = dml_error {
        log::warn!("{}: DirectML nicht verwendbar, ONNX Runtime rechnet auf dem Prozessor: {e:#}", path.display());
        *LAST_GPU_ERROR.lock().unwrap() = Some(format!("{e:#}"));
    }
    Ok((session, false))
}

impl Net {
    /// Loads a network with fixed input shapes: on the graphics card if possible, else with
    /// ONNX Runtime on the processor, else with tract.
    pub fn load(path: &Path, inputs: &[Input]) -> Result<Net> {
        if gpu_enabled() && gpu_installed() {
            match ort_session(path) {
                Ok((session, gpu)) => {
                    if gpu {
                        *LAST_GPU_ERROR.lock().unwrap() = None;
                    }
                    let net = Net { backend: Backend::Ort { session: Mutex::new(session), gpu }, inputs: inputs.to_vec() };
                    *LAST_DEVICE.lock().unwrap() = Some(net.device());
                    return Ok(net);
                }
                Err(e) => {
                    log::warn!("{}: ONNX Runtime nicht verwendbar, rechne mit tract: {e:#}", path.display());
                    *LAST_GPU_ERROR.lock().unwrap() = Some(format!("{e:#}"));
                }
            }
        }
        let mut model = tract_onnx::onnx().model_for_path(path).with_context(|| format!("Modell {} nicht lesbar", path.display()))?;
        for (i, inp) in inputs.iter().enumerate() {
            model = match inp {
                Input::F32(s) => model.with_input_fact(i, f32::fact(*s).into())?,
                Input::U8(s) => model.with_input_fact(i, u8::fact(*s).into())?,
            };
        }
        let net = Net { backend: Backend::Cpu(model.into_optimized()?.into_runnable()?), inputs: inputs.to_vec() };
        *LAST_DEVICE.lock().unwrap() = Some(net.device());
        Ok(net)
    }

    /// Runs on the graphics card.
    pub fn on_gpu(&self) -> bool {
        matches!(self.backend, Backend::Ort { gpu: true, .. })
    }

    /// One run at a time is best (ONNX Runtime parallelizes itself, the graphics card works
    /// through one request after the other); tract runs are parallelized by the caller.
    pub fn prefers_serial(&self) -> bool {
        matches!(self.backend, Backend::Ort { .. })
    }

    /// Where it runs, for display.
    pub fn device(&self) -> &'static str {
        match self.backend {
            Backend::Ort { gpu: true, .. } => "Grafikkarte",
            Backend::Ort { gpu: false, .. } => "Prozessor (ONNX Runtime)",
            Backend::Cpu(_) => "Prozessor",
        }
    }

    pub fn run(&self, data: &[Data]) -> Result<Vec<Output>> {
        anyhow::ensure!(data.len() == self.inputs.len(), "Falsche Anzahl Eingaben");
        match &self.backend {
            Backend::Cpu(plan) => {
                let mut ins: TVec<TValue> = TVec::new();
                for (inp, d) in self.inputs.iter().zip(data) {
                    let t = match (inp, d) {
                        (Input::F32(s), Data::F32(v)) => Tensor::from_shape(s, v)?,
                        (Input::U8(s), Data::U8(v)) => Tensor::from_shape(s, v)?,
                        _ => bail!("Falscher Eingabetyp"),
                    };
                    ins.push(t.into());
                }
                plan.run(ins)?
                    .iter()
                    .map(|o| {
                        let f = o.cast_to::<f32>()?;
                        let v = f.to_plain_array_view::<f32>()?;
                        Ok(Output { shape: v.shape().to_vec(), data: v.iter().copied().collect() })
                    })
                    .collect()
            }
            Backend::Ort { session, .. } => {
                let mut session = session.lock().unwrap();
                let names: Vec<String> = session.inputs().iter().map(|i| i.name().to_string()).collect();
                let mut ins: Vec<(String, ort::value::DynValue)> = Vec::new();
                for ((inp, d), name) in self.inputs.iter().zip(data).zip(names) {
                    let v: ort::value::DynValue = match (inp, d) {
                        (Input::F32(s), Data::F32(v)) => ort::value::Tensor::from_array((s.to_vec(), v.to_vec())).map_err(|e| anyhow!("{e}"))?.into_dyn(),
                        (Input::U8(s), Data::U8(v)) => ort::value::Tensor::from_array((s.to_vec(), v.to_vec())).map_err(|e| anyhow!("{e}"))?.into_dyn(),
                        _ => bail!("Falscher Eingabetyp"),
                    };
                    ins.push((name, v));
                }
                let outputs = session.run(ins).map_err(|e| anyhow!("{e}"))?;
                let mut out = Vec::new();
                for i in 0..outputs.len() {
                    let v = &outputs[i];
                    let (shape, data) = match v.try_extract_tensor::<f32>() {
                        Ok((s, d)) => (s.to_vec(), d.to_vec()),
                        Err(_) => {
                            let (s, d) = v.try_extract_tensor::<u8>().map_err(|e| anyhow!("{e}"))?;
                            (s.to_vec(), d.iter().map(|x| *x as f32).collect())
                        }
                    };
                    out.push(Output { shape: shape.iter().map(|d| *d as usize).collect(), data });
                }
                Ok(out)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Downloads one DLL from the real NuGet package (network; `cargo test -- --ignored`).
    #[test]
    #[ignore]
    fn zip_entry_from_nuget() {
        let [(url, inner, _, _), _] = gpu_files();
        let dll = zip_entry(&url, inner, &|_| {}).unwrap();
        assert!(dll.len() > 5_000_000 && dll.starts_with(b"MZ"), "{}", dll.len());
    }

    #[test]
    fn uses_onnx_runtime_when_given() {
        if std::env::var_os("SILBERKORN_ORT").is_none() {
            return;
        }
        let path = crate::models::path(&crate::models::EYE_STATE);
        if !path.exists() {
            return;
        }
        let net = Net::load(&path, &[Input::F32([1, 3, 32, 32])]).unwrap();
        assert!(net.prefers_serial(), "{}", net.device());
        let out = net.run(&[Data::F32(&vec![0.0; 3 * 32 * 32])]).unwrap();
        assert_eq!(out[0].data.len(), 2);
        // Off: tract.
        set_gpu_enabled(false);
        let cpu = Net::load(&path, &[Input::F32([1, 3, 32, 32])]).unwrap();
        set_gpu_enabled(true);
        assert!(!cpu.prefers_serial());
        let b = cpu.run(&[Data::F32(&vec![0.0; 3 * 32 * 32])]).unwrap();
        assert!((out[0].data[0] - b[0].data[0]).abs() < 1e-4);
    }
}
