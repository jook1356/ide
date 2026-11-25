//! Window placement (FFI glue): save and restore a window's place in physical pixels.
//!
//! iced reports and takes window positions in logical pixels, converted with the scale factor
//! the window has at that moment. With monitors of different scale that is not stable (the
//! same logical point lands on another monitor after a restart, as winit converts it with the
//! scale of the monitor the window is created on), and a minimised window reports Windows'
//! parking spot (-32000, -32000). The app keeps the detail window's place the way Windows apps
//! do: the restored ("normal") rectangle in physical pixels, the monitor's DPI at that time and
//! whether it was maximised; restoring clamps the rectangle into the nearest monitor's work area.

/// A window's restored rectangle (outer, physical screen pixels), the DPI of the monitor it is
/// on and whether it is maximised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub dpi: u32,
    pub maximized: bool,
}

/// The placement of window `hwnd` (also while minimised or maximised: its restored rectangle).
/// `None` off Windows or when the window cannot be asked.
pub fn window_placement(hwnd: isize) -> Option<Placement> {
    #[cfg(windows)]
    return imp::get(hwnd);
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        None
    }
}

/// Move and size window `hwnd` (not maximised) to `p`, clamped into the work area of the
/// monitor nearest to it; the size follows that monitor's DPI if it changed since `p` was
/// taken. Its show state is unchanged (the app shows / maximises it afterwards). Returns the
/// rectangle used. No-op off Windows.
pub fn place_window(hwnd: isize, p: &Placement) -> Option<Placement> {
    #[cfg(windows)]
    return imp::place(hwnd, p);
    #[cfg(not(windows))]
    {
        let _ = (hwnd, p);
        None
    }
}

/// Fit `(x, y, w, h)` into `work` (left, top, right, bottom): shrink it to the area, then move it
/// inside.
#[cfg_attr(not(windows), allow(dead_code))]
fn clamp_rect(x: i32, y: i32, w: i32, h: i32, work: (i32, i32, i32, i32)) -> (i32, i32, i32, i32) {
    let (l, t, r, b) = work;
    let w = w.min(r - l).max(1);
    let h = h.min(b - t).max(1);
    (x.clamp(l, r - w), y.clamp(t, b - h), w, h)
}

#[cfg(windows)]
mod imp {
    use super::{Placement, clamp_rect};
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect,
    };
    use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowPlacement, GetWindowRect, IsIconic, IsZoomed, SWP_NOACTIVATE, SWP_NOSIZE,
        SWP_NOZORDER, SetWindowPos, WINDOWPLACEMENT,
    };

    /// (monitor, its full rectangle, its work area) nearest to `r`.
    fn monitor_of(r: &RECT) -> Option<(HMONITOR, RECT, RECT)> {
        // SAFETY: plain queries with a RECT and a MONITORINFO struct we own (size set).
        unsafe {
            let m = MonitorFromRect(r, MONITOR_DEFAULTTONEAREST);
            if m.is_null() {
                return None;
            }
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            (GetMonitorInfoW(m, &mut mi) != 0).then_some((m, mi.rcMonitor, mi.rcWork))
        }
    }

    fn dpi_of(m: HMONITOR) -> u32 {
        let (mut x, mut y) = (0u32, 0u32);
        // SAFETY: `m` comes from MonitorFromRect; the out values are locals.
        let ok = unsafe { GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut x, &mut y) } == 0;
        if ok && x > 0 { x } else { 96 }
    }

    pub fn get(hwnd: isize) -> Option<Placement> {
        let hwnd = hwnd as HWND;
        // SAFETY: `hwnd` is one of this process's own live windows (from winit inside
        // `window::run`, on the event loop thread); read-only queries into structs we own.
        unsafe {
            let maximized = IsZoomed(hwnd) != 0;
            let mut r: RECT = std::mem::zeroed();
            if IsIconic(hwnd) == 0 && !maximized {
                // Shown normally: its rectangle on the screen.
                if GetWindowRect(hwnd, &mut r) == 0 {
                    return None;
                }
            } else {
                // Minimised / maximised: the restored rectangle, in workspace coordinates
                // (relative to the work area); shift it back to screen coordinates.
                let mut wp: WINDOWPLACEMENT = std::mem::zeroed();
                wp.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
                if GetWindowPlacement(hwnd, &mut wp) == 0 {
                    return None;
                }
                r = wp.rcNormalPosition;
                if let Some((_, mon, work)) = monitor_of(&r) {
                    let (dx, dy) = (work.left - mon.left, work.top - mon.top);
                    r.left += dx;
                    r.right += dx;
                    r.top += dy;
                    r.bottom += dy;
                }
            }
            let (m, _, _) = monitor_of(&r)?;
            Some(Placement {
                x: r.left,
                y: r.top,
                width: r.right - r.left,
                height: r.bottom - r.top,
                dpi: dpi_of(m),
                maximized,
            })
        }
    }

    pub fn place(hwnd: isize, p: &Placement) -> Option<Placement> {
        let hwnd = hwnd as HWND;
        let want = RECT {
            left: p.x,
            top: p.y,
            right: p.x + p.width,
            bottom: p.y + p.height,
        };
        let (m, _, work) = monitor_of(&want)?;
        let dpi = dpi_of(m);
        let scale = |v: i32| {
            if p.dpi == 0 || p.dpi == dpi {
                v
            } else {
                (v as i64 * dpi as i64 / p.dpi as i64) as i32
            }
        };
        let (x, y, w, h) = clamp_rect(
            p.x,
            p.y,
            scale(p.width),
            scale(p.height),
            (work.left, work.top, work.right, work.bottom),
        );
        // SAFETY: `hwnd` is one of this process's own live windows (see `get`); SetWindowPos
        // only moves / sizes it. First move it (on another monitor's DPI winit rescales it to
        // keep its logical size), then give it the exact size there.
        unsafe {
            let flags = SWP_NOZORDER | SWP_NOACTIVATE;
            SetWindowPos(hwnd, std::ptr::null_mut(), x, y, 0, 0, flags | SWP_NOSIZE);
            SetWindowPos(hwnd, std::ptr::null_mut(), x, y, w, h, flags);
        }
        Some(Placement {
            x,
            y,
            width: w,
            height: h,
            dpi,
            maximized: p.maximized,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::clamp_rect;

    #[test]
    fn clamp_into_work_area() {
        let work = (0, 0, 3840, 2112);
        // Inside: unchanged.
        assert_eq!(clamp_rect(100, 50, 1440, 960, work), (100, 50, 1440, 960));
        // Windows' minimised parking spot: pulled onto the monitor.
        assert_eq!(
            clamp_rect(-32000, -32000, 1440, 960, work),
            (0, 0, 1440, 960)
        );
        // A maximised rectangle with its invisible borders: shrunk to the work area.
        assert_eq!(clamp_rect(-12, -12, 3864, 2136, work), (0, 0, 3840, 2112));
        // Past the right / bottom edge: moved back in.
        assert_eq!(
            clamp_rect(3000, 2000, 1440, 960, work),
            (2400, 1152, 1440, 960)
        );
        // A monitor left of / above the primary one (negative coordinates).
        let left = (-1920, -200, 0, 880);
        assert_eq!(clamp_rect(-500, 0, 800, 600, left), (-800, 0, 800, 600));
    }
}
