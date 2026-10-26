//! Windows' own view of the app window, for the log. On some machines clicks land beside their
//! target, more so the larger the window: this tells whether the app's idea of the window size
//! or of the mouse position differs from Windows' (see `App::log_geometry` and
//! `App::check_click`). Elsewhere it knows nothing.

/// The client area in physical pixels, its DPI and how the process handles DPI.
pub struct WindowFacts {
    pub client: [i32; 2],
    pub dpi: u32,
    pub awareness: &'static str,
}

#[cfg(windows)]
pub fn window(hwnd: isize) -> Option<WindowFacts> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_PER_MONITOR_AWARE, DPI_AWARENESS_SYSTEM_AWARE, DPI_AWARENESS_UNAWARE, GetAwarenessFromDpiAwarenessContext, GetDpiForWindow,
        GetWindowDpiAwarenessContext,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;
    // SAFETY: plain Win32 queries on our own window.
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        if GetClientRect(hwnd, &mut r) == 0 {
            return None;
        }
        let awareness = match GetAwarenessFromDpiAwarenessContext(GetWindowDpiAwarenessContext(hwnd)) {
            DPI_AWARENESS_UNAWARE => "DPI-unbewusst",
            DPI_AWARENESS_SYSTEM_AWARE => "DPI systemweit",
            DPI_AWARENESS_PER_MONITOR_AWARE => "DPI je Monitor",
            _ => "DPI unbekannt",
        };
        Some(WindowFacts { client: [r.right - r.left, r.bottom - r.top], dpi: GetDpiForWindow(hwnd), awareness })
    }
}

/// Where the mouse is in the client area, in physical pixels.
#[cfg(windows)]
pub fn cursor(hwnd: isize) -> Option<[i32; 2]> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    // SAFETY: see above.
    unsafe {
        let mut p = POINT { x: 0, y: 0 };
        (GetCursorPos(&mut p) != 0 && ScreenToClient(hwnd, &mut p) != 0).then_some([p.x, p.y])
    }
}

#[cfg(not(windows))]
pub fn window(_hwnd: isize) -> Option<WindowFacts> {
    None
}

#[cfg(not(windows))]
pub fn cursor(_hwnd: isize) -> Option<[i32; 2]> {
    None
}
