//! How much video memory the system gives this process, and how much it uses: on Windows from
//! DXGI (the video memory manager counts every allocation, also Vulkan's); wgpu's own budgets
//! are of no use (see `Gpu::new`). Elsewhere unknown.

/// Budget and current use of local video memory, in bytes, for the adapter with these PCI ids.
#[cfg(windows)]
pub(crate) fn video_memory(vendor: u32, device: u32) -> Option<(u64, u64)> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, IDXGIAdapter3, IDXGIFactory1};
    use windows::core::Interface;
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;
    for i in 0.. {
        let adapter = unsafe { factory.EnumAdapters1(i) }.ok()?;
        let desc = unsafe { adapter.GetDesc1() }.ok()?;
        if desc.VendorId != vendor || desc.DeviceId != device {
            continue;
        }
        let adapter: IDXGIAdapter3 = adapter.cast().ok()?;
        let mut info = Default::default();
        unsafe { adapter.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info) }.ok()?;
        return Some((info.Budget, info.CurrentUsage));
    }
    None
}

#[cfg(not(windows))]
pub(crate) fn video_memory(_vendor: u32, _device: u32) -> Option<(u64, u64)> {
    None
}
