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

/// A rendering the CPU redoes although the card worked (a part too large for it, values from
/// the image data): unlike a failure, it leaves the buffers cached on the card valid.
#[derive(Debug)]
pub(crate) struct Fallback(pub String);

impl std::fmt::Display for Fallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Fallback {}

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
    /// PCI vendor and device id (to find the adapter for [`vram`]).
    ids: (u32, u32),
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
struct Pool {
    free: Vec<(wgpu::Buffer, bool, Instant)>,
    bytes: u64,
    /// Most memory it keeps (a quarter of the video memory budget where that is known).
    limit: u64,
}

impl Default for Pool {
    fn default() -> Self {
        Self { free: Vec::new(), bytes: 0, limit: POOL_LIMIT }
    }
}

/// Most memory the pool keeps without a known budget, and how long an unused buffer is kept.
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
        if b.size() < 1 << 20 || b.size() > self.limit / 2 {
            return;
        }
        self.bytes += b.size();
        self.free.push((b, readback, Instant::now()));
        self.trim();
    }

    /// Drops buffers unused for a while, and the oldest beyond the limit.
    fn trim(&mut self) {
        self.free.sort_by_key(|e| std::cmp::Reverse(e.2));
        let limit = self.limit;
        let mut kept = 0u64;
        self.free.retain(|(b, _, t)| {
            let keep = t.elapsed() < POOL_IDLE && kept + b.size() <= limit;
            if keep {
                kept += b.size();
            }
            keep
        });
        self.bytes = kept;
    }
}

thread_local! {
    /// A buffer couldn't be created for lack of video memory in this thread's current
    /// [`Gpu::checked`] (error scopes are per thread too).
    static OUT_OF_MEMORY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Bytes of buffers newly created in this thread's current [`Gpu::checked`] (for the log
    /// when video memory runs out).
    static CREATED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

mod vram;

const MB: u64 = 1 << 20;

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
        match Gpu::new(env_setting().as_deref() == Some("software"), None) {
            Ok(g) => {
                log::info!("Grafikkarte für die Entwicklung: {} ({})", g.name, g.memory_note());
                // Without this the pool keeps its buffers while nothing is rendered, memory
                // other programs (and the networks) could use.
                let _ = std::thread::Builder::new().name("Grafikspeicher aufräumen".into()).spawn(|| {
                    loop {
                        std::thread::sleep(POOL_IDLE);
                        if let Some(Some(g)) = GPU.get() {
                            g.pool().trim();
                        }
                    }
                });
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

/// Ranks adapters: real graphics cards first, native APIs before OpenGL. Also used for the
/// adapter the window is drawn with.
///
/// Intel's Vulkan driver on Windows crashes on device creation (UHD Graphics, driver
/// 31.0.101.2137, access violation in igvk64.dll), so Intel takes DX12 there. Elsewhere Vulkan
/// stays first among equals (on NVIDIA both compute about equally fast).
pub fn rank(info: &wgpu::AdapterInfo) -> u32 {
    rank_on(info, cfg!(windows))
}

fn rank_on(info: &wgpu::AdapterInfo, windows: bool) -> u32 {
    let kind = match info.device_type {
        wgpu::DeviceType::DiscreteGpu => 40,
        wgpu::DeviceType::IntegratedGpu => 30,
        wgpu::DeviceType::VirtualGpu => 20,
        wgpu::DeviceType::Other => 10,
        wgpu::DeviceType::Cpu => 0,
    };
    let intel_on_windows = windows && info.vendor == 0x8086;
    let api = match info.backend {
        wgpu::Backend::Vulkan if intel_on_windows => 1,
        wgpu::Backend::Vulkan | wgpu::Backend::Dx12 | wgpu::Backend::Metal => 2,
        _ => 0,
    };
    kind + api
}

impl Gpu {
    fn new(allow_software: bool, budget_percent: Option<u8>) -> Result<Self> {
        // When video memory doesn't even allow the device, Vulkan's wgpu-hal panics while
        // dropping the half-made one ("buckets are not empty") instead of returning an error.
        std::panic::catch_unwind(|| Self::create(allow_software, budget_percent))
            .unwrap_or_else(|_| Err(anyhow!("Grafikkarte ließ sich nicht einrichten (zu wenig Grafikspeicher?)")))
    }

    fn create(allow_software: bool, budget_percent: Option<u8>) -> Result<Self> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        // Only for tests: wgpu's budget applies per memory heap, and on NVIDIA (Vulkan) the small
        // heap the CPU writes uploads into is soon beyond it. A refused `queue.write_buffer`
        // then takes the whole device as lost.
        desc.memory_budget_thresholds.for_resource_creation = budget_percent;
        let instance = wgpu::Instance::new(desc);
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
            ids: (info.vendor, info.device),
            kernels: Mutex::new(HashMap::new()),
            max_binding: (limits.max_storage_buffer_binding_size as u64).min(limits.max_buffer_size),
            serial: (info.backend == wgpu::Backend::Gl).then(|| Mutex::new(())),
            broken,
            pool: Mutex::new(Pool {
                limit: vram::video_memory(info.vendor, info.device).map_or(POOL_LIMIT, |(budget, _)| (budget / 4).clamp(128 * MB, POOL_LIMIT)),
                ..Pool::default()
            }),
            display_tables: OnceLock::new(),
            #[cfg(test)]
            inject_incomplete: AtomicBool::new(false),
        })
    }

    /// Takes the device out of use for the rest of the session. Not when video memory ran out
    /// (the incomplete result comes from that): [`Gpu::checked`] only redoes that rendering.
    pub(crate) fn fail(&self, reason: impl Into<String>) {
        if OUT_OF_MEMORY.get() {
            return;
        }
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
        Self::new(true, None).ok()
    }

    /// Like [`Self::new_for_test`] with almost no video memory for new buffers (where the
    /// driver reports a budget).
    #[cfg(test)]
    pub(crate) fn new_starved_for_test() -> Option<Self> {
        if env_setting().as_deref() == Some("0") {
            return None;
        }
        Self::new(true, Some(1)).ok()
    }

    /// Compiles kernels ahead of their first use.
    pub(crate) fn prepare(&self, kernels: &[Kernel]) {
        for (name, src) in kernels {
            self.kernel(name, src);
        }
    }

    /// The compute pipeline of a kernel (`main` in `src`), compiled on first use.
    fn kernel(&self, name: &'static str, src: &str) -> wgpu::ComputePipeline {
        let mut kernels = self.kernels.lock().unwrap_or_else(|e| e.into_inner());
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

    /// Video memory free for this process (the pool's buffers count as free: they are reused
    /// first), where the system says (Windows).
    pub(crate) fn available_bytes(&self) -> Option<u64> {
        let (budget, used) = vram::video_memory(self.ids.0, self.ids.1)?;
        Some(budget.saturating_sub(used) + self.pool().bytes)
    }

    /// Video memory as the system sees it for this process (Windows), and what the pool holds.
    pub(crate) fn memory_note(&self) -> String {
        let pool = self.pool().bytes / MB;
        match vram::video_memory(self.ids.0, self.ids.1) {
            Some((budget, used)) => format!("Budget {} MB, belegt {} MB, davon Vorrat {pool} MB", budget / MB, used / MB),
            None => format!("Budget unbekannt, Vorrat {pool} MB"),
        }
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
        self.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes.max(16).next_multiple_of(4),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Creates a buffer and notes when video memory ran out (the buffer is invalid then).
    fn create_buffer(&self, desc: &wgpu::BufferDescriptor) -> wgpu::Buffer {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let b = self.device.create_buffer(desc);
        CREATED.set(CREATED.get() + desc.size);
        if pollster::block_on(scope.pop()).is_some() {
            OUT_OF_MEMORY.set(true);
        }
        b
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

    /// A storage buffer of `bytes` whose content `fill` writes straight into wgpu's staging
    /// memory, in rows of `row` bytes (in parallel); no copy of the whole data in between.
    pub(crate) fn upload_rows(&self, bytes: usize, row: usize, fill: impl Fn(usize, wgpu::WriteOnly<[u8]>) + Sync) -> wgpu::Buffer {
        use rayon::prelude::*;
        let size = (bytes as u64).max(16).next_multiple_of(4);
        let buf = self.buffer(size);
        // `None`: the error went to the error scope (see `checked`).
        if let Some(mut view) = self.queue.write_buffer_with(&buf, 0, wgpu::BufferSize::new(size).expect("at least 16")) {
            let (mut rest, mut tail) = view.slice(..).split_at(bytes);
            tail.fill(0);
            /// A row of the mapping for another thread. wgpu marks only `WriteOnly` of sized types
            /// `Send`; a slice of one is just as safe to write from another thread: the rows
            /// don't overlap, and the view outlives the loop.
            struct Row<'a>(wgpu::WriteOnly<'a, [u8]>);
            unsafe impl Send for Row<'_> {}
            let mut rows = Vec::with_capacity(bytes / row.max(1));
            while rest.len() >= row && row > 0 {
                let (r, more) = rest.split_at(row);
                rows.push(Row(r));
                rest = more;
            }
            rows.into_par_iter().enumerate().for_each(|(y, r)| fill(y, r.0));
        }
        buf
    }

    pub(crate) fn encoder(&self) -> Encoder<'_> {
        Encoder { gpu: self, enc: self.device.create_command_encoder(&Default::default()) }
    }

    /// Copies `count` values of a buffer back into memory (waits for all submitted work).
    pub(crate) fn read<T: bytemuck::Pod + Send>(&self, buf: &wgpu::Buffer, count: usize) -> Result<Vec<T>> {
        use rayon::prelude::*;
        // Straight from the mapping into the result, in parallel (hundreds of MB for an export).
        self.read_mapped([(buf, count * size_of::<T>())], |view, [offset]| {
            let mut out = vec![T::zeroed(); count];
            let bytes: &mut [u8] = bytemuck::cast_slice_mut(&mut out);
            let len = bytes.len();
            bytes.par_chunks_mut(1 << 20).zip(view[offset..offset + len].par_chunks(1 << 20)).for_each(|(o, i)| o.copy_from_slice(i));
            out
        })
    }

    /// Like [`Self::read`] for several buffers (bytes of each) with one wait for the card.
    pub(crate) fn read_bytes<const N: usize>(&self, parts: [(&wgpu::Buffer, usize); N]) -> Result<[Vec<u8>; N]> {
        let lens = parts.map(|p| p.1);
        self.read_mapped(parts, |view, offsets| std::array::from_fn(|i| view[offsets[i]..offsets[i] + lens[i]].to_vec()))
    }

    /// Copies the parts (bytes of each) into a mapped buffer, waits for the card once, and gives
    /// `f` the mapping with the offset of each part.
    fn read_mapped<const N: usize, R>(&self, parts: [(&wgpu::Buffer, usize); N], f: impl FnOnce(&[u8], [usize; N]) -> R) -> Result<R> {
        // Each part at an offset the copy accepts.
        let mut offsets = [0u64; N];
        let mut total = 0u64;
        for (o, (_, bytes)) in offsets.iter_mut().zip(&parts) {
            *o = total;
            total += (*bytes as u64).next_multiple_of(8);
        }
        if total == 0 {
            return Ok(f(&[], [0; N]));
        }
        let staging = self.pool().take(total, true).unwrap_or_else(|| {
            self.create_buffer(&wgpu::BufferDescriptor {
                label: Some("readback"),
                size: total,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        for ((buf, bytes), o) in parts.iter().zip(offsets) {
            if *bytes > 0 {
                enc.copy_buffer_to_buffer(buf, 0, &staging, o, (*bytes as u64).next_multiple_of(4));
            }
        }
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
        let out = f(&view, offsets.map(|o| o as usize));
        drop(view);
        staging.unmap();
        self.pool().give(staging, true);
        Ok(out)
    }

    /// Runs `f` and returns the first validation or out-of-memory error it caused. The
    /// work of `f` must not overlap with other threads on OpenGL, so it runs exclusively there.
    pub(crate) fn checked<R>(&self, f: impl FnOnce() -> Result<R>) -> Result<R> {
        let _serial = self.serial.as_ref().map(|m| m.lock().unwrap_or_else(|e| e.into_inner()));
        OUT_OF_MEMORY.set(false);
        CREATED.set(0);
        let oom = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let r = f();
        let v = pollster::block_on(validation.pop());
        let o = pollster::block_on(oom.pop());
        if OUT_OF_MEMORY.replace(false) {
            let note = self.memory_note();
            // Free what the pool holds for the next attempt; this rendering runs on the CPU.
            let mut pool = self.pool();
            *pool = Pool { limit: pool.limit, ..Pool::default() };
            drop(pool);
            log::warn!("Grafikspeicher voll ({} MB neu angefordert; {note}), dieses Bild wird auf dem Prozessor berechnet", CREATED.get() / MB);
            return Err(anyhow!("Grafikkarte: Grafikspeicher voll"));
        }
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

    /// Copies `bytes` from `from_offset` in `from` to `offset` in `to` (all in bytes).
    pub(crate) fn copy_at(&mut self, from: &wgpu::Buffer, from_offset: u64, to: &wgpu::Buffer, offset: u64, bytes: u64) {
        if bytes > 0 {
            self.enc.copy_buffer_to_buffer(from, from_offset, to, offset, bytes);
        }
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

#[cfg(test)]
mod tests {
    use super::rank_on;

    fn adapter(vendor: u32, device_type: wgpu::DeviceType, backend: wgpu::Backend) -> wgpu::AdapterInfo {
        wgpu::AdapterInfo {
            name: String::new(),
            vendor,
            device: 0,
            device_type,
            device_pci_bus_id: String::new(),
            driver: String::new(),
            driver_info: String::new(),
            backend,
            subgroup_min_size: 0,
            subgroup_max_size: 0,
            transient_saves_memory: None,
            limit_bucket: None,
        }
    }

    /// Intel under Windows takes DX12 (its Vulkan driver crashes); others keep Vulkan first.
    #[test]
    fn intel_avoids_vulkan_on_windows() {
        use wgpu::{Backend::*, DeviceType::*};
        let intel_vk = adapter(0x8086, IntegratedGpu, Vulkan);
        let intel_dx = adapter(0x8086, IntegratedGpu, Dx12);
        let intel_gl = adapter(0x8086, IntegratedGpu, Gl);
        assert!(rank_on(&intel_dx, true) > rank_on(&intel_vk, true));
        assert!(rank_on(&intel_vk, true) > rank_on(&intel_gl, true));
        assert_eq!(rank_on(&intel_dx, false), rank_on(&intel_vk, false));
        let nvidia_vk = adapter(0x10DE, DiscreteGpu, Vulkan);
        let nvidia_dx = adapter(0x10DE, DiscreteGpu, Dx12);
        assert_eq!(rank_on(&nvidia_dx, true), rank_on(&nvidia_vk, true));
        assert!(rank_on(&nvidia_vk, true) > rank_on(&intel_dx, true));
    }
}
