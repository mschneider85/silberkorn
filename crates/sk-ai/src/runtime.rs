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
    *LAST_DEVICE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn gpu_enabled() -> bool {
    GPU_ENABLED.load(Ordering::Relaxed)
}

/// Windows reset the graphics card while a network ran on it (a command took too long, e.g. for
/// lack of graphics memory): until the program is started again, networks run on the processor.
static GPU_RESET: AtomicBool = AtomicBool::new(false);

/// Whether an error of ONNX Runtime is the graphics card being reset or removed
/// (DXGI_ERROR_DEVICE_REMOVED, _HUNG, _RESET).
fn is_device_reset(e: &str) -> bool {
    ["887A0005", "887A0006", "887A0007"].iter().any(|c| e.to_ascii_uppercase().contains(c))
}

fn note_device_reset(e: &str) {
    if is_device_reset(e) && !GPU_RESET.swap(true, Ordering::Relaxed) {
        log::error!("Die Grafikkarte wurde zurückgesetzt: KI rechnet bis zum Neustart auf dem Prozessor");
        set_gpu_error(Some("Grafikkarte wurde zurückgesetzt, KI bis zum Neustart auf dem Prozessor".into()));
        CONFIG.fetch_add(1, Ordering::Relaxed);
    }
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
    // With the separate process, ONNX Runtime is never loaded into this one: its problems are
    // known once a network was loaded there.
    if crate::worker::configured() {
        return LAST_GPU_ERROR.lock().unwrap_or_else(|e| e.into_inner()).clone();
    }
    match ort_ready() {
        Ok(()) => LAST_GPU_ERROR.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        Err(e) => Some(e.clone()),
    }
}

/// Sets the reason the graphics card isn't used (`None`: it is).
pub(crate) fn set_gpu_error(e: Option<String>) {
    *LAST_GPU_ERROR.lock().unwrap_or_else(|e| e.into_inner()) = e;
}

/// The latest reason a network fell back to the processor.
static LAST_GPU_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// Size limit for a file of the GPU support (the libraries are some tens of MB).
const GPU_FILE_LIMIT: usize = 256 << 20;

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
        let data = zip_entry(url, inner, GPU_FILE_LIMIT, &|p| progress((i as f32 + p) / n))?;
        anyhow::ensure!(crate::models::sha256(&data) == *sha256, "{name}: Prüfsumme stimmt nicht, die Datei wird nicht verwendet");
        sk_core::safe_file::write(&dir.join(name), &data)?;
    }
    CONFIG.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// HTTP client for downloads. Every request has time limits, so that a connection that stops
/// delivering (e.g. a dropped network) ends with an error instead of waiting forever; large files
/// are therefore fetched in parts of a few MB, each with its own request.
pub(crate) fn http() -> &'static ureq::Agent {
    static AGENT: std::sync::LazyLock<ureq::Agent> = std::sync::LazyLock::new(|| {
        use std::time::Duration;
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .timeout_recv_body(Some(Duration::from_secs(120)))
            .build()
            .into()
    });
    &AGENT
}

/// Size of the file at `url`, from the `Content-Range` of a one-byte request.
fn total_size(url: &str) -> Result<usize> {
    let resp = http().get(url).header("Range", "bytes=0-0").call().with_context(|| format!("Download von {url} fehlgeschlagen"))?;
    anyhow::ensure!(resp.status().as_u16() == 206, "Der Server unterstützt keine Teil-Downloads");
    let range = resp.headers().get("content-range").and_then(|v| v.to_str().ok()).unwrap_or_default();
    range.rsplit('/').next().and_then(|n| n.trim().parse().ok()).filter(|&n: &usize| n > 0).context("Dateigröße unbekannt")
}

fn get_range(url: &str, range: &str) -> Result<Vec<u8>> {
    let mut resp = http().get(url).header("Range", range).call().with_context(|| format!("Download von {url} fehlgeschlagen"))?;
    anyhow::ensure!(resp.status().as_u16() == 206, "Der Server unterstützt keine Teil-Downloads");
    Ok(resp.body_mut().with_config().limit(256 << 20).read_to_vec()?)
}

/// Little-endian numbers in data from the server: out of range is an error, not a panic.
fn u16le(b: &[u8], i: usize) -> Result<usize> {
    let v = b.get(i..i + 2).context("ZIP-Archiv beschädigt")?;
    Ok(u16::from_le_bytes([v[0], v[1]]) as usize)
}

fn u32le(b: &[u8], i: usize) -> Result<usize> {
    let v = b.get(i..i + 4).context("ZIP-Archiv beschädigt")?;
    Ok(u32::from_le_bytes([v[0], v[1], v[2], v[3]]) as usize)
}

/// One file from a ZIP archive on a web server, without downloading the whole archive: the
/// central directory from its end, then only the file's compressed data. The archive is not
/// trusted before the caller checks the checksum: a file above `max_bytes` (packed or unpacked)
/// is refused instead of taking all memory.
pub fn zip_entry(url: &str, name: &str, max_bytes: usize, progress: &dyn Fn(f32)) -> Result<Vec<u8>> {
    // Explicit ranges only: GitHub's download servers refuse suffix ranges ("bytes=-N").
    let len = total_size(url)?;
    let tail_start = len.saturating_sub(65536);
    let tail = get_range(url, &format!("bytes={tail_start}-{}", len - 1))?;
    let eocd = tail.windows(4).rposition(|w| w == b"PK\x05\x06").context("Kein ZIP-Archiv")?;
    let (cd_size, cd_off) = (u32le(&tail, eocd + 12)?, u32le(&tail, eocd + 16)?);
    anyhow::ensure!(cd_size > 0 && cd_size <= 64 << 20, "ZIP-Archiv beschädigt");
    let cd = if cd_size <= eocd { tail[eocd - cd_size..eocd].to_vec() } else { get_range(url, &format!("bytes={cd_off}-{}", cd_off + cd_size - 1))? };
    let mut p = 0;
    while p + 46 <= cd.len() && u32le(&cd, p)? == 0x0201_4b50 {
        let (method, csize, usize_) = (u16le(&cd, p + 10)?, u32le(&cd, p + 20)?, u32le(&cd, p + 24)?);
        let (nl, el, cl) = (u16le(&cd, p + 28)?, u16le(&cd, p + 30)?, u16le(&cd, p + 32)?);
        let local = u32le(&cd, p + 42)?;
        let entry = String::from_utf8_lossy(cd.get(p + 46..p + 46 + nl).context("ZIP-Archiv beschädigt")?);
        if entry == name {
            anyhow::ensure!(csize <= max_bytes && usize_ <= max_bytes, "{name}: größer als erwartet, wird nicht geladen");
            // Local header (30 bytes plus its own name and extra field), then the data.
            let head = get_range(url, &format!("bytes={local}-{}", local + 29))?;
            let start = local + 30 + u16le(&head, 26)? + u16le(&head, 28)?;
            let mut data = Vec::with_capacity(csize);
            let chunk = 1 << 20;
            let mut off = 0;
            while off < csize {
                let end = (off + chunk).min(csize);
                data.extend(get_range(url, &format!("bytes={}-{}", start + off, start + end - 1))?);
                anyhow::ensure!(data.len() <= csize, "{name}: unerwartete Antwort des Servers");
                off = end;
                progress(off as f32 / csize as f32);
            }
            return match method {
                0 => Ok(data),
                8 => {
                    let mut out = Vec::with_capacity(usize_);
                    // At most the announced size (plus one byte to notice more).
                    flate2::read::DeflateDecoder::new(&data[..]).take(usize_ as u64 + 1).read_to_end(&mut out)?;
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
    /// f32 of any rank.
    F32Shape(&'static [usize]),
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

/// Input data with its shape, owned (for ONNX Runtime and the separate process).
pub(crate) enum Tensor {
    F32(Vec<usize>, Vec<f32>),
    U8(Vec<usize>, Vec<u8>),
}

enum Backend {
    /// tract on the processor.
    Cpu(Arc<TypedRunnableModel>),
    /// ONNX Runtime in this process, on the graphics card (`gpu`) or the processor.
    Ort { session: Mutex<ort::session::Session>, gpu: bool },
    /// ONNX Runtime in the separate process (see `worker`). If that ends, the network goes on
    /// with tract (`fallback`, made then).
    Remote { session: crate::worker::Session, gpu: bool, fallback: Mutex<Option<Arc<TypedRunnableModel>>> },
}

/// A loaded network, on the graphics card or the processor.
pub struct Net {
    backend: Backend,
    inputs: Vec<Input>,
    path: PathBuf,
    keep: Option<Vec<usize>>,
}

/// An ONNX Runtime session: with DirectML on the graphics card (Windows), else on the processor.
/// Returns the session, whether it runs on the graphics card, and why DirectML could not be used.
/// `gpu` false: on the processor only.
pub(crate) fn ort_session(path: &Path, gpu: bool) -> Result<(ort::session::Session, bool, Option<String>)> {
    ort_ready().as_ref().map_err(|e| anyhow!("{e}"))?;
    let new = || ort::session::Session::builder().map_err(|e| anyhow!("{e}"));
    let mut dml_error = None;
    if cfg!(windows) && gpu {
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
            Ok(s) => return Ok((s, true, None)),
            Err(e) => dml_error = Some(format!("{e:#}")),
        }
    }
    let session = new()?.commit_from_file(path).map_err(|e| anyhow!("{e}"))?;
    Ok((session, false, dml_error))
}

/// Runs an ONNX Runtime session; outputs converted to f32.
pub(crate) fn ort_run(session: &mut ort::session::Session, data: Vec<Tensor>) -> Result<Vec<Output>> {
    let names: Vec<String> = session.inputs().iter().map(|i| i.name().to_string()).collect();
    anyhow::ensure!(names.len() == data.len(), "Falsche Anzahl Eingaben");
    let mut ins: Vec<(String, ort::value::DynValue)> = Vec::new();
    for (t, name) in data.into_iter().zip(names) {
        let v: ort::value::DynValue = match t {
            Tensor::F32(s, v) => ort::value::Tensor::from_array((s, v)).map_err(|e| anyhow!("{e}"))?.into_dyn(),
            Tensor::U8(s, v) => ort::value::Tensor::from_array((s, v)).map_err(|e| anyhow!("{e}"))?.into_dyn(),
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

fn tract_run(plan: &Arc<TypedRunnableModel>, inputs: &[Input], data: &[Data]) -> Result<Vec<Output>> {
    let mut ins: TVec<TValue> = TVec::new();
    for (inp, d) in inputs.iter().zip(data) {
        let t = match (inp, d) {
            (Input::F32(s), Data::F32(v)) => tract_onnx::prelude::Tensor::from_shape(s, v)?,
            (Input::U8(s), Data::U8(v)) => tract_onnx::prelude::Tensor::from_shape(s, v)?,
            (Input::F32Shape(s), Data::F32(v)) => tract_onnx::prelude::Tensor::from_shape(s, v)?,
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

/// The input data with their shapes, owned.
fn tensors(inputs: &[Input], data: &[Data]) -> Result<Vec<Tensor>> {
    inputs
        .iter()
        .zip(data)
        .map(|(inp, d)| match (inp, d) {
            (Input::F32(s), Data::F32(v)) => Ok(Tensor::F32(s.to_vec(), v.to_vec())),
            (Input::U8(s), Data::U8(v)) => Ok(Tensor::U8(s.to_vec(), v.to_vec())),
            (Input::F32Shape(s), Data::F32(v)) => Ok(Tensor::F32(s.to_vec(), v.to_vec())),
            _ => bail!("Falscher Eingabetyp"),
        })
        .collect()
}

/// A network for tract with fixed input shapes (`plain`: without the shape hints of the file).
fn tract_plan(path: &Path, inputs: &[Input], keep: Option<&[usize]>, plain: bool) -> Result<Arc<TypedRunnableModel>> {
    let onnx = tract_onnx::onnx().with_ignore_output_shapes(plain).with_ignore_value_info(plain);
    let mut model = onnx.model_for_path(path).with_context(|| format!("Modell {} nicht lesbar", path.display()))?;
    for (i, inp) in inputs.iter().enumerate() {
        model = match inp {
            Input::F32(s) => model.with_input_fact(i, f32::fact(*s).into())?,
            Input::U8(s) => model.with_input_fact(i, u8::fact(*s).into())?,
            Input::F32Shape(s) => model.with_input_fact(i, f32::fact(*s).into())?,
        };
    }
    if let Some(keep) = keep {
        let outlets: Vec<OutletId> = keep.iter().filter_map(|&i| model.outputs.get(i).copied()).collect();
        model.select_output_outlets(&outlets)?;
    }
    model.into_optimized()?.into_runnable()
}

impl Net {
    /// Loads a network with fixed input shapes: on the graphics card if possible, else with
    /// ONNX Runtime on the processor, else with tract.
    pub fn load(path: &Path, inputs: &[Input]) -> Result<Net> {
        Self::load_outputs(path, inputs, None)
    }

    /// [`Self::load`]; with tract only the outputs `keep` (by index) are computed, for networks
    /// whose other outputs tract cannot handle. ONNX Runtime returns all of them.
    pub fn load_outputs(path: &Path, inputs: &[Input], keep: Option<&[usize]>) -> Result<Net> {
        Self::load_with(path, inputs, keep, true)
    }

    /// [`Self::load`], but never on the graphics card: for networks too large for the graphics
    /// memory of smaller cards (Windows resets a card that takes too long for one command).
    pub fn load_processor(path: &Path, inputs: &[Input]) -> Result<Net> {
        Self::load_with(path, inputs, None, false)
    }

    fn load_with(path: &Path, inputs: &[Input], keep: Option<&[usize]>, gpu: bool) -> Result<Net> {
        let gpu = gpu && !GPU_RESET.load(Ordering::Relaxed);
        let net = |backend| Net { backend, inputs: inputs.to_vec(), path: path.to_path_buf(), keep: keep.map(<[usize]>::to_vec) };
        // ONNX Runtime and DirectML in the separate process if there is one (a crash in them or
        // in the graphics driver then ends only that), else in this one.
        let ort = if !gpu_enabled() || !gpu_installed() {
            None
        } else if crate::worker::configured() {
            crate::worker::usable().then(|| crate::worker::load(path, gpu).map(|(session, gpu, dml)| (Backend::Remote { session, gpu, fallback: Mutex::default() }, gpu, dml)))
        } else {
            Some(ort_session(path, gpu).map(|(session, gpu, dml)| (Backend::Ort { session: Mutex::new(session), gpu }, gpu, dml)))
        };
        match ort {
            Some(Ok((backend, gpu, dml_error))) => {
                if let Some(e) = &dml_error {
                    log::warn!("{}: DirectML nicht verwendbar, ONNX Runtime rechnet auf dem Prozessor: {e}", path.display());
                }
                if gpu || dml_error.is_some() {
                    set_gpu_error(dml_error);
                }
                let net = net(backend);
                *LAST_DEVICE.lock().unwrap_or_else(|e| e.into_inner()) = Some(net.device());
                return Ok(net);
            }
            Some(Err(e)) => {
                log::warn!("{}: ONNX Runtime nicht verwendbar, rechne mit tract: {e:#}", path.display());
                set_gpu_error(Some(format!("{e:#}")));
            }
            None => {}
        }
        let net = net(Backend::Cpu(tract_plan_any(path, inputs, keep)?));
        *LAST_DEVICE.lock().unwrap_or_else(|e| e.into_inner()) = Some(net.device());
        Ok(net)
    }

    /// Runs on the graphics card.
    pub fn on_gpu(&self) -> bool {
        match &self.backend {
            Backend::Ort { gpu, .. } => *gpu,
            Backend::Remote { gpu, fallback, .. } => *gpu && fallback.lock().unwrap_or_else(|e| e.into_inner()).is_none(),
            Backend::Cpu(_) => false,
        }
    }

    /// One run at a time is best (ONNX Runtime parallelizes itself, the graphics card works
    /// through one request after the other); tract runs are parallelized by the caller.
    pub fn prefers_serial(&self) -> bool {
        matches!(self.backend, Backend::Ort { .. } | Backend::Remote { .. })
    }

    /// Where it runs, for display.
    pub fn device(&self) -> &'static str {
        match &self.backend {
            Backend::Remote { fallback, .. } if fallback.lock().unwrap_or_else(|e| e.into_inner()).is_some() => "Prozessor",
            Backend::Ort { gpu: true, .. } | Backend::Remote { gpu: true, .. } => "Grafikkarte",
            Backend::Ort { gpu: false, .. } | Backend::Remote { gpu: false, .. } => "Prozessor (ONNX Runtime)",
            Backend::Cpu(_) => "Prozessor",
        }
    }

    pub fn run(&self, data: &[Data]) -> Result<Vec<Output>> {
        anyhow::ensure!(data.len() == self.inputs.len(), "Falsche Anzahl Eingaben");
        match &self.backend {
            Backend::Cpu(plan) => tract_run(plan, &self.inputs, data),
            Backend::Ort { session, .. } => ort_run(&mut session.lock().unwrap_or_else(|e| e.into_inner()), tensors(&self.inputs, data)?).inspect_err(|e| note_device_reset(&format!("{e:#}"))),
            Backend::Remote { session, fallback, .. } => {
                let mut fallback = fallback.lock().unwrap_or_else(|e| e.into_inner());
                let plan = match &*fallback {
                    Some(plan) => plan.clone(),
                    None => match crate::worker::run(session, tensors(&self.inputs, data)?) {
                        Ok(out) => return Ok(out),
                        Err(e) => {
                            note_device_reset(&format!("{e:#}"));
                            log::warn!("{}: {e:#}, rechne mit tract weiter", self.path.display());
                            fallback.insert(tract_plan_any(&self.path, &self.inputs, self.keep.as_deref())?).clone()
                        }
                    },
                };
                drop(fallback);
                tract_run(&plan, &self.inputs, data)
            }
        }
    }
}

/// [`tract_plan`]; shape hints inside some exports (symbolic batch sizes) contradict the fixed
/// input shapes, such networks are loaded again without them.
fn tract_plan_any(path: &Path, inputs: &[Input], keep: Option<&[usize]>) -> Result<Arc<TypedRunnableModel>> {
    tract_plan(path, inputs, keep, false).or_else(|e| tract_plan(path, inputs, keep, true).map_err(|_| e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Downloads one DLL from the real NuGet package and checks it against the pinned hash
    /// (network: only with `SK_NETWORK_TESTS=1 cargo t-all`).
    #[test]
    #[ignore = "Netzwerk: SK_NETWORK_TESTS=1 cargo t-all"]
    fn zip_entry_from_nuget() {
        if std::env::var_os("SK_NETWORK_TESTS").is_none() {
            return;
        }
        let [(url, inner, _, sha256), _] = gpu_files();
        let dll = zip_entry(&url, inner, GPU_FILE_LIMIT, &|_| {}).unwrap();
        assert!(dll.len() > 5_000_000 && dll.starts_with(b"MZ"), "{}", dll.len());
        assert_eq!(crate::models::sha256(&dll), sha256);
    }

    /// The message of the reset that happened on a GTX 1650 Ti, and others that are none.
    #[test]
    fn recognizes_a_reset_graphics_card() {
        assert!(is_device_reset("Exception(2) tid(716c) 887A0006 Die GPU antwortet auf keine weiteren Befehle mehr."));
        assert!(is_device_reset("hr 0x887a0005"));
        assert!(!is_device_reset("Non-zero status code returned while running Pad node"));
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
