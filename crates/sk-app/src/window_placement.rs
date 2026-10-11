//! Windows: switching between a maximized window and full screen without the window visibly
//! passing through its normal (restored) size.
//!
//! A full screen window must not be maximized (winit then limits it to the work area, leaving
//! the taskbar area black). Leaving maximized normally shows the restored size for a moment;
//! setting the restored size to the monitor first avoids that. The window manager's
//! maximize / restore animations are switched off around these steps (they looked like a zoom).

/// Normal (restored) window rect: left, top, right, bottom in workspace coordinates.
pub type NormalRect = [i32; 4];

/// Native window handle of the app window, if available.
#[cfg(windows)]
pub fn native_handle(frame: &eframe::Frame) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match frame.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

#[cfg(not(windows))]
pub fn native_handle(_frame: &eframe::Frame) -> Option<isize> {
    None
}

/// Leaves maximized straight to the size of the monitor. Returns the previous normal rect,
/// for [`maximize_again`]; `None` if it didn't work (the caller falls back to un-maximizing).
#[cfg(windows)]
pub fn leave_maximized_to_monitor(hwnd: isize) -> Option<NormalRect> {
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOWNORMAL, SetWindowPlacement};
    let mut wp = placement(hwnd)?;
    let r = wp.rcNormalPosition;
    let previous = [r.left, r.top, r.right, r.bottom];
    // SAFETY: plain Win32 calls with properly sized, initialized structs.
    unsafe {
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mi) == 0 {
            return None;
        }
        // The normal rect is in workspace coordinates (relative to the work area).
        let (dx, dy) = (mi.rcWork.left - mi.rcMonitor.left, mi.rcWork.top - mi.rcMonitor.top);
        let m = mi.rcMonitor;
        wp.rcNormalPosition.left = m.left - dx;
        wp.rcNormalPosition.top = m.top - dy;
        wp.rcNormalPosition.right = m.right - dx;
        wp.rcNormalPosition.bottom = m.bottom - dy;
        wp.showCmd = SW_SHOWNORMAL as u32;
        wp.flags = 0;
        (SetWindowPlacement(hwnd, &wp) != 0).then_some(previous)
    }
}

/// Maximizes the window in one step and restores its previous normal rect.
#[cfg(windows)]
pub fn maximize_again(hwnd: isize, normal: NormalRect) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_MAXIMIZE, SW_SHOWMAXIMIZED, SetWindowPlacement, ShowWindow};
    // Maximize first (straight from the monitor size), then put back the normal rect: setting it
    // while the window is still in its normal state would move the window there for a moment,
    // showing the desktop behind it.
    // SAFETY: plain Win32 call on our own window.
    unsafe {
        ShowWindow(hwnd, SW_MAXIMIZE);
    }
    let Some(mut wp) = placement(hwnd) else { return };
    let [left, top, right, bottom] = normal;
    wp.rcNormalPosition.left = left;
    wp.rcNormalPosition.top = top;
    wp.rcNormalPosition.right = right;
    wp.rcNormalPosition.bottom = bottom;
    wp.showCmd = SW_SHOWMAXIMIZED as u32;
    wp.flags = 0;
    // SAFETY: see above.
    unsafe {
        SetWindowPlacement(hwnd, &wp);
    }
}

/// Switches the window manager's minimize / maximize / restore animations of the window.
#[cfg(windows)]
pub fn set_animations(hwnd: isize, enabled: bool) {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_TRANSITIONS_FORCEDISABLED, DwmSetWindowAttribute};
    let disabled: i32 = (!enabled).into();
    // SAFETY: the attribute takes a BOOL (i32).
    unsafe {
        DwmSetWindowAttribute(hwnd, DWMWA_TRANSITIONS_FORCEDISABLED as u32, (&disabled as *const i32).cast(), std::mem::size_of::<i32>() as u32);
    }
}

#[cfg(windows)]
fn placement(hwnd: isize) -> Option<windows_sys::Win32::UI::WindowsAndMessaging::WINDOWPLACEMENT> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowPlacement, WINDOWPLACEMENT};
    // SAFETY: see above.
    unsafe {
        let mut wp: WINDOWPLACEMENT = std::mem::zeroed();
        wp.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        (GetWindowPlacement(hwnd, &mut wp) != 0).then_some(wp)
    }
}

#[cfg(not(windows))]
pub fn leave_maximized_to_monitor(_hwnd: isize) -> Option<NormalRect> {
    None
}

#[cfg(not(windows))]
pub fn maximize_again(_hwnd: isize, _normal: NormalRect) {}

#[cfg(not(windows))]
pub fn set_animations(_hwnd: isize, _enabled: bool) {}
