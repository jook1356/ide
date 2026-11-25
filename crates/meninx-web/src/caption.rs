//! Windows 11 snap layouts on the app-drawn maximise button of a frameless window (FFI glue).
//!
//! Windows shows the snap layout flyout only when the pointer is over what hit-testing reports
//! as the maximise button (`HTMAXBUTTON`). winit never answers `WM_NCHITTEST` for undecorated
//! windows (the whole window is client area), so the window is subclassed: over the maximise
//! button's rectangle (fixed from the right edge, like `meninx-app`'s title bar) it answers
//! `HTMAXBUTTON`, and a click there maximises / restores the window itself (Windows would
//! otherwise track and paint a classic button). Everything else goes to winit unchanged.

/// Subclass window `hwnd`: the maximise button is the `button_w` wide slot left of the close
/// button (also `button_w` wide) at the right end of a `bar_h` tall title bar (logical px).
/// No-op off Windows.
pub fn enable_snap_layout(hwnd: isize, bar_h: f32, button_w: f32) {
    #[cfg(windows)]
    imp::enable(hwnd, bar_h, button_w);
    #[cfg(not(windows))]
    let _ = (hwnd, bar_h, button_w);
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClientRect, HTMAXBUTTON, IsZoomed, SW_MAXIMIZE, SW_RESTORE, ShowWindow,
        WM_NCHITTEST, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP,
    };

    /// Subclass id (any constant unique to this module).
    const ID: usize = 0x1de_ca9;
    /// The top edge stays the resize grip of `meninx-app` (logical px).
    const GRIP: f32 = 5.0;

    pub fn enable(hwnd: isize, bar_h: f32, button_w: f32) {
        // The geometry travels in the subclass's reference data: bar height and button width
        // in 1/100 logical px.
        let data = ((bar_h * 100.0) as usize) << 16 | ((button_w * 100.0) as usize & 0xffff);
        // SAFETY: `hwnd` is one of this process's live top-level windows (taken from winit in
        // `window::run` on the event loop thread, the thread that owns it, as subclassing
        // requires); `proc_` matches `SUBCLASSPROC` and only reads its arguments. Setting the
        // same subclass again only updates its data.
        unsafe {
            SetWindowSubclass(hwnd as HWND, Some(proc_), ID, data);
        }
    }

    /// Whether client point (`x`, `y`) in physical px is over the maximise button.
    fn over_max(hwnd: HWND, x: i32, y: i32, data: usize) -> bool {
        let bar_h = (data >> 16) as f32 / 100.0;
        let button_w = (data & 0xffff) as f32 / 100.0;
        // SAFETY: plain queries on a live window this thread owns; `rect` is ours.
        let (scale, width, zoomed) = unsafe {
            let mut rect: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut rect);
            (
                GetDpiForWindow(hwnd).max(96) as f32 / 96.0,
                rect.right - rect.left,
                IsZoomed(hwnd) != 0,
            )
        };
        let top = if zoomed { 0.0 } else { GRIP * scale };
        let (x, y) = (x as f32, y as f32);
        let right = width as f32 - button_w * scale;
        y >= top && y < bar_h * scale && x >= right - button_w * scale && x < right
    }

    unsafe extern "system" fn proc_(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        data: usize,
    ) -> LRESULT {
        match msg {
            WM_NCHITTEST => {
                // Screen coordinates in the low / high words (signed: other monitors).
                let mut pt = POINT {
                    x: (lparam & 0xffff) as i16 as i32,
                    y: ((lparam >> 16) & 0xffff) as i16 as i32,
                };
                // SAFETY: `pt` is ours; `hwnd` is the live window this message is for.
                unsafe { ScreenToClient(hwnd, &mut pt) };
                if over_max(hwnd, pt.x, pt.y, data) {
                    return HTMAXBUTTON as LRESULT;
                }
            }
            WM_NCLBUTTONDOWN if wparam == HTMAXBUTTON as WPARAM => return 0,
            WM_NCLBUTTONUP if wparam == HTMAXBUTTON as WPARAM => {
                // SAFETY: show-state change of the live window this message is for.
                unsafe {
                    let cmd = if IsZoomed(hwnd) != 0 {
                        SW_RESTORE
                    } else {
                        SW_MAXIMIZE
                    };
                    ShowWindow(hwnd, cmd);
                }
                return 0;
            }
            _ => {}
        }
        // SAFETY: forwards the message unchanged to the next window procedure (winit's).
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }
}
