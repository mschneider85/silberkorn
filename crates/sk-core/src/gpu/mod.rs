//! Compute shaders for the development pipeline (wgpu, headless).
//!
//! The device is created on first use, independently of the window's renderer. Without a
//! usable graphics card, or with `SILBERKORN_GPU=0`, everything runs on the CPU. Software
//! adapters (lavapipe, WARP) are slower than the CPU path and only used when
//! `SILBERKORN_GPU=software` (for tests on machines without a graphics card).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Result, anyhow};
use wgpu::util::DeviceExt;

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
}

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

/// The device, when the graphics card is switched on and usable.
pub(crate) fn active() -> Option<&'static Gpu> {
    if enabled() { device() } else { None }
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
        Ok(Self {
            device,
            queue,
            name: format!("{} ({:?})", info.name, info.backend),
            kernels: Mutex::new(HashMap::new()),
            max_binding: (limits.max_storage_buffer_binding_size as u64).min(limits.max_buffer_size),
            serial: (info.backend == wgpu::Backend::Gl).then(|| Mutex::new(())),
        })
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
        self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        })
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
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: bytes.next_multiple_of(4),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(buf, 0, &staging, 0, bytes.next_multiple_of(4));
        self.queue.submit([enc.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        staging.map_async(wgpu::MapMode::Read, .., move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        rx.recv()??;
        let view = staging.get_mapped_range(..)?;
        let out = bytemuck::cast_slice::<u8, T>(&view[..bytes as usize]).to_vec();
        drop(view);
        staging.unmap();
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
