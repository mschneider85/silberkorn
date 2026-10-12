//! Lists the graphics adapters wgpu finds: `cargo run --release -p sk-core --example gpu_info`

fn main() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    for a in pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all())) {
        let info = a.get_info();
        let compute = a.get_downlevel_capabilities().flags.contains(wgpu::DownlevelFlags::COMPUTE_SHADERS);
        let l = a.limits();
        println!(
            "{} ({:?}, {:?}) compute={compute} storage/stage={} binding={} MB",
            info.name,
            info.backend,
            info.device_type,
            l.max_storage_buffers_per_shader_stage,
            l.max_storage_buffer_binding_size >> 20
        );
    }
    println!("verwendet: {:?}", sk_core::gpu::adapter_name());
}
