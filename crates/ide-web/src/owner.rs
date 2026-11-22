//! Window ownership (FFI glue): the app's detail window "pinned" to the app window.
//!
//! Hermes Desk pins a region window with Electron's `BrowserWindow.setParentWindow`, which on
//! Windows sets the window's *owner* (`GWLP_HWNDPARENT`, not `SetParent`): an owned window stays
//! in front of its owner, is minimised and restored with it, and is destroyed with it. winit only
//! offers an owner at creation time, so the app sets it on the live window here, the same way
//! Electron does (including its hide / show to make the change take effect at once).

/// Make window `child` (an HWND) owned by `owner` (an HWND), or an ordinary top-level window
/// again with `None`. No-op when the owner is already the requested one, and off Windows.
pub fn set_window_owner(child: isize, owner: Option<isize>) {
    #[cfg(windows)]
    imp::set_owner(child, owner.unwrap_or(0));
    #[cfg(not(windows))]
    let _ = (child, owner);
}

/// The owner (an HWND) of window `child`, if any (`None` off Windows).
pub fn window_owner(child: isize) -> Option<isize> {
    #[cfg(windows)]
    return imp::owner(child);
    #[cfg(not(windows))]
    {
        let _ = child;
        None
    }
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GW_OWNER, GWLP_HWNDPARENT, GetWindow, GetWindowLongPtrW,
        GetWindowPlacement, IsWindowVisible, SW_HIDE, SetWindowLongPtrW, ShowWindow,
        WINDOWPLACEMENT,
    };

    pub fn set_owner(child: isize, owner: isize) {
        let hwnd = child as HWND;
        // SAFETY: `child` is the handle of one of this process's own live windows (taken from
        // winit inside `window::run` on the event loop thread); these calls only read or write
        // its owner and show state, and the placement struct is a plain C struct we own.
        unsafe {
            if GetWindowLongPtrW(hwnd, GWLP_HWNDPARENT) == owner {
                return;
            }
            SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, owner);
            // Like Electron: re-show the window so the new owner takes effect immediately.
            if IsWindowVisible(hwnd) != 0 {
                let mut wp: WINDOWPLACEMENT = std::mem::zeroed();
                wp.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
                if GetWindowPlacement(hwnd, &mut wp) != 0 {
                    ShowWindow(hwnd, SW_HIDE);
                    ShowWindow(hwnd, wp.showCmd as i32);
                }
                BringWindowToTop(hwnd);
            }
        }
    }

    pub fn owner(child: isize) -> Option<isize> {
        // SAFETY: read-only query on one of this process's own window handles.
        let o = unsafe { GetWindow(child as HWND, GW_OWNER) };
        (!o.is_null()).then_some(o as isize)
    }
}
