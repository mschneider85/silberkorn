//! Compute shaders for the development pipeline (wgpu, headless).
//!
//! The device is created on first use, independently of the window's renderer. Without a
//! usable graphics card, or with `SILBERKORN_GPU=0`, everything runs on the CPU. Software
//! adapters (lavapipe, WARP) are slower than the CPU path and only used when
//! `SILBERKORN_GPU=software` (for tests on machines without a graphics card).
//!
//! A graphics card that stops working (driver reset, device lost, a readback that doesn't come
//! back, an incomplete result) is not used again in this session; everything continues on the
//! CPU, which stays the reference.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};

/// Helpers shared by all kernels (prepended to every shader).
const COMMON: &str = include_str!("shaders/common.wgsl");

pub struct Gpu {
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    /// Adapter name and backend, for display.
    pub name: String,
    kernels: Mutex<HashMap<&'static str, wgpu::ComputePipeline>>,
    /// Largest storage buffer that can be bound.
    pub(crate) max_binding: u64,
    /// OpenGL can only be used from one thread at a time.
    serial: Option<Mutex<()>>,
    /// Why the device must not be used any more (see [`Gpu::fail`]).
    broken: Arc<Mutex<Option<String>>>,
    /// Buffers of finished renderings, reused instead of allocated anew (see [`Pool`]).
    pool: Mutex<Pool>,
    /// Display tables of the pipeline (uploaded once).
    pub(crate) display_tables: OnceLock<(wgpu::Buffer, wgpu::Buffer)>,
    /// Tests: the next result counts as incomplete.
    #[cfg(test)]
    pub(crate) inject_incomplete: AtomicBool,
}

/// Released buffers for reuse: allocating (and zeroing) large buffers anew for every rendering
/// costs about as much as some of the kernels. Storage buffers are cleared on reuse, so they
/// behave like new ones; readback buffers are overwritten completely anyway.
#[derive(Default)]
struct Pool {
    free: Vec<(wgpu::Buffer, bool, Instant)>,
    bytes: u64,
}

/// Most memory the pool keeps, and how long an unused buffer is kept.
const POOL_LIMIT: u64 = 768 << 20;
const POOL_IDLE: Duration = Duration::from_secs(3);

impl Pool {
    /// The smallest free buffer of the kind that holds `bytes` without wasting more than a
    /// quarter.
    fn take(&mut self, bytes: u64, readback: bool) -> Option<wgpu::Buffer> {
        self.trim();
        let i = self
            .free
            .iter()
            .enumerate()
            .filter(|(_, (b, r, _))| *r == readback && b.size() >= bytes && b.size() <= bytes + bytes / 4)
            .min_by_key(|(_, (b, _, _))| b.size())
            .map(|(i, _)| i)?;
        let (b, _, _) = self.free.swap_remove(i);
        self.bytes -= b.size();
        Some(b)
    }

    fn give(&mut self, b: wgpu::Buffer, readback: bool) {
        // Small buffers are cheap to allocate; huge ones would crowd out the rest.
        if b.size() < 1 << 20 || b.size() > POOL_LIMIT / 2 {
            return;
        }
        self.bytes += b.size();
        self.free.push((b, readback, Instant::now()));
        self.trim();
    }

    /// Drops buffers unused for a while, and the oldest beyond the limit.
    fn trim(&mut self) {
        self.free.sort_by_key(|e| std::cmp::Reverse(e.2));
        let mut kept = 0u64;
        self.free.retain(|(b, _, t)| {
            let keep = t.elapsed() < POOL_IDLE && kept + b.size() <= POOL_LIMIT;
            if keep {
                kept += b.size();
            }
            keep
        });
        self.bytes = kept;
    }
}

/// Longest wait for results; a healthy card needs a fraction of it even for large exports.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

static ENABLED: AtomicBool = AtomicBool::new(true);

static GPU: OnceLock<Option<Gpu>> = OnceLock::new();

/// The device (created on first use, which takes a moment).
fn device() -> Option<&'static Gpu> {
    GPU.get_or_init(|| {
        if env_setting().as_deref() == Some("0") {
            return None;
        }
        match Gpu::new(env_setting().as_deref() == Some("software")) {
            Ok(g) => {
                log::info!("Grafikkarte für die Entwicklung: {}", g.name);
                Some(g)
            }
            Err(e) => {
                log::info!("Entwicklung auf dem Prozessor: {e}");
                None
            }
        }
    })
    .as_ref()
}

fn env_setting() -> Option<String> {
    std::env::var("SILBERKORN_GPU").ok()
}

/// Switches the graphics card on or off for the development pipeline.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Name of the usable graphics card (initializes the device).
pub fn adapter_name() -> Option<String> {
    device().map(|g| g.name.clone())
}

/// Without waiting: `None` while the device is not set up yet, then the adapter name if usable.
pub fn status() -> Option<Option<String>> {
    GPU.get().map(|g| g.as_ref().map(|g| g.name.clone()))
}

/// The device, when the graphics card is switched on and still works.
pub(crate) fn active() -> Option<&'static Gpu> {
    if enabled() { device().filter(|g| g.problem().is_none()) } else { None }
}

/// Why the graphics card stopped being used in this session, if it did.
pub fn problem() -> Option<String> {
    GPU.get().and_then(|g| g.as_ref()).and_then(Gpu::problem)
}

/// Ranks adapters: real graphics cards first, native APIs before OpenGL.
fn rank(info: &wgpu::AdapterInfo) -> u32 {
    let kind = match info.device_type {
        wgpu::DeviceType::DiscreteGpu => 40,
        wgpu::DeviceType::IntegratedGpu => 30,
        wgpu::DeviceType::VirtualGpu => 20,
        wgpu::DeviceType::Other => 10,
        wgpu::DeviceType::Cpu => 0,
    };
    let api = match info.backend {
        wgpu::Backend::Vulkan | wgpu::Backend::Dx12 | wgpu::Backend::Metal => 2,
        _ => 0,
    };
    kind + api
}

impl Gpu {
    fn new(allow_software: bool) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let mut adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
        adapters.retain(|a| {
            let info = a.get_info();
            // Compute shaders with enough storage buffers are required (not on old OpenGL drivers).
            let compute = a.get_downlevel_capabilities().flags.contains(wgpu::DownlevelFlags::COMPUTE_SHADERS);
            let buffers = a.limits().max_storage_buffers_per_shader_stage >= 8;
            compute && buffers && (allow_software || info.device_type != wgpu::DeviceType::Cpu)
        });
        adapters.sort_by_key(|a| std::cmp::Reverse(rank(&a.get_info())));
        let adapter = adapters.into_iter().next().ok_or_else(|| anyhow!("keine geeignete Grafikkarte"))?;
        let info = adapter.get_info();
        let limits = adapter.limits();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("silberkorn"),
            required_limits: limits.clone(),
            ..Default::default()
        }))?;
        // Errors are caught per rendering with error scopes; anything else is only logged.
        device.on_uncaptured_error(Arc::new(|e| log::error!("Grafikkarte: {e}")));
        let broken: Arc<Mutex<Option<String>>> = Arc::default();
        let lost = broken.clone();
        device.set_device_lost_callback(move |reason, message| {
            log::warn!("Grafikkarte verloren ({reason:?}): {message}; weiter auf dem Prozessor");
            lost.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert(format!("Grafikkarte verloren: {message}"));
        });
        Ok(Self {
            device,
            queue,
            name: format!("{} ({:?})", info.name, info.backend),
            kernels: Mutex::new(HashMap::new()),
            max_binding: (limits.max_storage_buffer_binding_size as u64).min(limits.max_buffer_size),
            serial: (info.backend == wgpu::Backend::Gl).then(|| Mutex::new(())),
            broken,
            pool: Mutex::default(),
            display_tables: OnceLock::new(),
            #[cfg(test)]
            inject_incomplete: AtomicBool::new(false),
        })
    }

    /// Takes the device out of use for the rest of the session.
    pub(crate) fn fail(&self, reason: impl Into<String>) {
        let reason = reason.into();
        let mut broken = self.broken.lock().unwrap_or_else(|e| e.into_inner());
        if broken.is_none() {
            log::warn!("{reason}; die Grafikkarte wird nicht mehr verwendet");
            *broken = Some(reason);
        }
    }

    pub(crate) fn problem(&self) -> Option<String> {
        self.broken.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// A device of its own for tests (software adapters allowed), independent of the shared
    /// one the pipeline uses. `None` with `SILBERKORN_GPU=0`.
    #[cfg(test)]
    pub(crate) fn new_for_test() -> Option<Self> {
        if env_setting().as_deref() == Some("0") {
            return None;
        }
        Self::new(true).ok()
    }

    /// Compiles kernels ahead of their first use.
    pub(crate) fn prepare(&self, kernels: &[Kernel]) {
        for (name, src) in kernels {
            self.kernel(name, src);
        }
    }

    /// The compute pipeline of a kernel (`main` in `src`), compiled on first use.
    fn kernel(&self, name: &'static str, src: &str) -> wgpu::ComputePipeline {
        let mut kernels = self.kernels.lock().unwrap();
        kernels
            .entry(name)
            .or_insert_with(|| {
                let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some(name),
                    source: wgpu::ShaderSource::Wgsl(format!("{COMMON}\n{src}").into()),
                });
                self.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(name),
                    layout: None,
                    module: &module,
                    entry_point: Some("main"),
                    compilation_options: Default::default(),
                    cache: None,
                })
            })
            .clone()
    }

    fn pool(&self) -> std::sync::MutexGuard<'_, Pool> {
        self.pool.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A storage buffer of at least `bytes` from the pool; `true` if it was used before (and
    /// must be cleared before use). Give it back with [`Gpu::release`].
    pub(crate) fn pooled_buffer(&self, bytes: u64) -> (wgpu::Buffer, bool) {
        let size = bytes.max(16).next_multiple_of(4);
        match self.pool().take(size, false) {
            Some(b) => (b, true),
            None => (self.buffer(size), false),
        }
    }

    /// Returns storage buffers no longer needed by anything (also not by caches).
    pub(crate) fn release(&self, buffers: impl IntoIterator<Item = wgpu::Buffer>) {
        let mut pool = self.pool();
        for b in buffers {
            pool.give(b, false);
        }
    }

    #[cfg(test)]
    pub(crate) fn pooled_bytes(&self) -> u64 {
        self.pool().bytes
    }

    /// A storage buffer of `bytes` (at least 16).
    pub(crate) fn buffer(&self, bytes: u64) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes.max(16).next_multiple_of(4),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// A storage buffer holding `data` (padded to at least 16 bytes).
    pub(crate) fn upload<T: bytemuck::Pod>(&self, data: &[T]) -> wgpu::Buffer {
        let bytes: &[u8] = bytemuck::cast_slice(data);
        if bytes.len() < 16 || bytes.len() % 4 != 0 {
            let mut padded = bytes.to_vec();
            padded.resize(bytes.len().max(16).next_multiple_of(4), 0);
            return self.upload_bytes(&padded);
        }
        self.upload_bytes(bytes)
    }

    fn upload_bytes(&self, bytes: &[u8]) -> wgpu::Buffer {
        // Not `create_buffer_init`: it panics when the buffer can't be created (out of memory,
        // above the size limit) instead of reporting the error to `checked`, so the CPU fallback
        // never ran.
        let buf = self.buffer(bytes.len() as u64);
        self.queue.write_buffer(&buf, 0, bytes);
        buf
    }

    pub(crate) fn encoder(&self) -> Encoder<'_> {
        Encoder { gpu: self, enc: self.device.create_command_encoder(&Default::default()) }
    }

    /// Copies `bytes` of a buffer back into memory (waits for all submitted work).
    pub(crate) fn read<T: bytemuck::Pod>(&self, buf: &wgpu::Buffer, count: usize) -> Result<Vec<T>> {
        let bytes = (count * size_of::<T>()) as u64;
        if bytes == 0 {
            return Ok(Vec::new());
        }
        let staging = self.pool().take(bytes.next_multiple_of(4), true).unwrap_or_else(|| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("readback"),
                size: bytes.next_multiple_of(4),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(buf, 0, &staging, 0, bytes.next_multiple_of(4));
        self.queue.submit([enc.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        staging.map_async(wgpu::MapMode::Read, .., move |r| {
            let _ = tx.send(r);
        });
        // A hanging driver must not block the app forever.
        if let Err(e) = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(READ_TIMEOUT) }) {
            self.fail(format!("Grafikkarte antwortet nicht ({e})"));
            return Err(anyhow!("Grafikkarte antwortet nicht: {e}"));
        }
        rx.recv_timeout(Duration::from_secs(1)).map_err(|_| anyhow!("Grafikkarte: Ergebnis kam nicht zurück"))??;
        let view = staging.get_mapped_range(..)?;
        let out = bytemuck::cast_slice::<u8, T>(&view[..bytes as usize]).to_vec();
        drop(view);
        staging.unmap();
        self.pool().give(staging, true);
        Ok(out)
    }

    /// Runs `f` and returns the first validation or out-of-memory error it caused. The
    /// work of `f` must not overlap with other threads on OpenGL, so it runs exclusively there.
    pub(crate) fn checked<R>(&self, f: impl FnOnce() -> Result<R>) -> Result<R> {
        let _serial = self.serial.as_ref().map(|m| m.lock().unwrap_or_else(|e| e.into_inner()));
        let oom = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let r = f();
        let v = pollster::block_on(validation.pop());
        let o = pollster::block_on(oom.pop());
        if let Some(e) = v.or(o) {
            return Err(anyhow!("Grafikkarte: {e}"));
        }
        r
    }
}

/// A compute kernel: name and WGSL source with a `main` entry point.
pub(crate) type Kernel = (&'static str, &'static str);

/// Records kernel dispatches.
pub(crate) struct Encoder<'a> {
    gpu: &'a Gpu,
    enc: wgpu::CommandEncoder,
}

impl Encoder<'_> {
    /// Dispatches kernel `name` with `bufs` bound to bindings 0, 1, … of group 0 and
    /// `groups` workgroups.
    pub(crate) fn run(&mut self, (name, src): Kernel, bufs: &[&wgpu::Buffer], groups: (u32, u32)) {
        let pipeline = self.gpu.kernel(name, src);
        let entries: Vec<wgpu::BindGroupEntry> =
            bufs.iter().enumerate().map(|(i, b)| wgpu::BindGroupEntry { binding: i as u32, resource: b.as_entire_binding() }).collect();
        let bind = self.gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(name),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &entries,
        });
        let mut pass = self.enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some(name), timestamp_writes: None });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(groups.0.max(1), groups.1.max(1), 1);
    }

    /// Per-pixel kernel over `w` x `h` (workgroups of 16 x 16).
    pub(crate) fn run_2d(&mut self, k: Kernel, bufs: &[&wgpu::Buffer], w: usize, h: usize) {
        self.run(k, bufs, ((w as u32).div_ceil(16), (h as u32).div_ceil(16)));
    }

    /// One invocation per item (workgroups of 64).
    pub(crate) fn run_1d(&mut self, k: Kernel, bufs: &[&wgpu::Buffer], n: usize) {
        let groups = (n as u32).div_ceil(64);
        // Split into two dimensions beyond the per-dimension limit.
        let (x, y) = if groups > 65535 { (65535, groups.div_ceil(65535)) } else { (groups, 1) };
        self.run(k, bufs, (x, y));
    }

    /// Fills a buffer with zeros (a reused buffer then behaves like a new one).
    pub(crate) fn clear(&mut self, b: &wgpu::Buffer) {
        self.enc.clear_buffer(b, 0, None);
    }

    /// Copies `bytes` from the start of `from` to `offset` (bytes) in `to`.
    pub(crate) fn copy(&mut self, from: &wgpu::Buffer, to: &wgpu::Buffer, offset: u64, bytes: u64) {
        if bytes > 0 {
            self.enc.copy_buffer_to_buffer(from, 0, to, offset, bytes.next_multiple_of(4));
        }
    }

    pub(crate) fn submit(self) {
        self.gpu.queue.submit([self.enc.finish()]);
    }
}
