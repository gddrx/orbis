//! What the window asks of the compositor, through `cpp/window.cpp` (KF6
//! WindowSystem): the blur behind it and focus with an activation token; and
//! the window's icon. Only for the GUI thread, once the application object
//! exists.

/// The orbis icon (`data/orbis.png`, drawn from `orbis.svg` at 256 px): the
/// window's and the tray's.
pub const ICON_PNG: &[u8] = include_bytes!("../../../data/orbis.png");

#[cfg(target_os = "linux")]
mod ffi {
    use std::ffi::{CString, c_char};

    unsafe extern "C" {
        fn orbis_blur_behind(enable: bool);
        fn orbis_blur_available() -> bool;
        fn orbis_activate_window(token: *const c_char);
        fn orbis_set_window_icon(png: *const u8, size: usize);
    }

    pub fn set_window_icon(png: &[u8]) {
        // SAFETY: called on the GUI thread once the application object
        // exists; C++ only reads the bytes during the call.
        unsafe { orbis_set_window_icon(png.as_ptr(), png.len()) }
    }

    pub fn blur_behind(enable: bool) {
        // SAFETY: called from slots, on the GUI thread, while the application
        // object lives.
        unsafe { orbis_blur_behind(enable) }
    }

    pub fn blur_available() -> bool {
        // SAFETY: as above.
        unsafe { orbis_blur_available() }
    }

    pub fn activate_window(token: Option<&str>) {
        let token = token.and_then(|t| CString::new(t).ok());
        let pointer = token.as_ref().map_or(std::ptr::null(), |t| t.as_ptr());
        // SAFETY: as above; the token outlives the call, and C++ copies it.
        unsafe { orbis_activate_window(pointer) }
    }
}

#[cfg(not(target_os = "linux"))]
mod ffi {
    pub fn blur_behind(_enable: bool) {}

    pub fn blur_available() -> bool {
        false
    }

    pub fn activate_window(_token: Option<&str>) {}

    pub fn set_window_icon(_png: &[u8]) {}
}

/// Gives the app's windows the orbis icon (before the first window is
/// made, so that it has it from the start).
pub fn set_window_icon() {
    ffi::set_window_icon(ICON_PNG);
}

/// Asks the compositor to blur behind the app's windows, or to stop.
pub fn blur_behind(enable: bool) {
    ffi::blur_behind(enable);
}

/// Whether the compositor blurs behind windows; false until a round trip
/// after the first call.
pub fn blur_available() -> bool {
    ffi::blur_available()
}

/// Raises and focuses the shown window; on Wayland focus comes only with an
/// activation `token`, such as the one the tray hands with a click.
pub fn activate_window(token: Option<&str>) {
    ffi::activate_window(token);
}
