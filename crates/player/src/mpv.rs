//! A safe handle over libmpv's client API.
//!
//! The client API is thread-safe: one thread may wait for events while others
//! send commands and read properties, as long as the handle lives. The handle
//! is destroyed when the last [`std::sync::Arc`] to it is dropped.

use std::ffi::{CStr, CString, c_int, c_void};

use crate::ffi;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("mpv: {0}")]
pub struct MpvError(pub String);

fn check(code: c_int) -> Result<(), MpvError> {
    if code >= 0 {
        return Ok(());
    }
    // SAFETY: mpv_error_string returns a static string for any code.
    let text = unsafe { CStr::from_ptr(ffi::mpv_error_string(code)) };
    Err(MpvError(text.to_string_lossy().into_owned()))
}

/// Options of mpv's Lua scripts (the on-screen controller, the youtube-dl
/// hook): a libmpv built without Lua, as in the release AppImage, has no
/// scripts and so no such options — there is nothing to turn off then.
const SCRIPT_OPTIONS: [&str; 2] = ["osc", "ytdl"];

fn c(text: &str) -> Result<CString, MpvError> {
    CString::new(text).map_err(|_| MpvError(format!("NUL byte in {text:?}")))
}

/// One event, copied out of mpv's buffer.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Shutdown,
    StartFile {
        entry: i64,
    },
    FileLoaded,
    PlaybackRestart,
    EndFile {
        entry: i64,
        reason: EndReason,
        error: Option<String>,
    },
    Flag {
        id: u64,
        value: bool,
    },
    Double {
        id: u64,
        value: Option<f64>,
    },
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    Eof,
    Stop,
    Quit,
    Error,
    Other,
}

#[derive(Debug)]
pub struct Mpv {
    handle: *mut ffi::MpvHandle,
}

// SAFETY: the mpv client API may be used from any thread (client.h,
// "Thread-safety"); the handle is only destroyed in Drop.
unsafe impl Send for Mpv {}
unsafe impl Sync for Mpv {}

impl Mpv {
    /// Creates and initializes a player with `options` (set before start).
    pub fn new(options: &[(&str, &str)]) -> Result<Self, MpvError> {
        // SAFETY: plain constructor; null on failure.
        let handle = unsafe { ffi::mpv_create() };
        if handle.is_null() {
            return Err(MpvError("mpv_create failed".into()));
        }
        let mpv = Self { handle };
        for &(name, value) in options {
            let (c_name, c_value) = (c(name)?, c(value)?);
            // SAFETY: valid handle and NUL-terminated strings.
            let code = unsafe {
                ffi::mpv_set_option_string(mpv.handle, c_name.as_ptr(), c_value.as_ptr())
            };
            if code == ffi::ERROR_OPTION_NOT_FOUND && SCRIPT_OPTIONS.contains(&name) {
                continue;
            }
            check(code)?;
        }
        // SAFETY: valid, not yet initialized handle.
        check(unsafe { ffi::mpv_initialize(mpv.handle) })?;
        Ok(mpv)
    }

    /// Runs a command such as `["loadfile", url, "replace"]`.
    pub fn command(&self, args: &[&str]) -> Result<(), MpvError> {
        let owned: Vec<CString> = args.iter().map(|a| c(a)).collect::<Result<_, _>>()?;
        let mut pointers: Vec<*const std::ffi::c_char> = owned.iter().map(|a| a.as_ptr()).collect();
        pointers.push(std::ptr::null());
        // SAFETY: NULL-terminated array of strings that outlive the call.
        check(unsafe { ffi::mpv_command(self.handle, pointers.as_mut_ptr()) })
    }

    pub fn set_string(&self, name: &str, value: &str) -> Result<(), MpvError> {
        let (name, value) = (c(name)?, c(value)?);
        // SAFETY: valid handle and strings.
        check(unsafe { ffi::mpv_set_property_string(self.handle, name.as_ptr(), value.as_ptr()) })
    }

    pub fn set_flag(&self, name: &str, value: bool) -> Result<(), MpvError> {
        let name = c(name)?;
        let mut flag: c_int = value.into();
        // SAFETY: FORMAT_FLAG takes a pointer to an int.
        check(unsafe {
            ffi::mpv_set_property(
                self.handle,
                name.as_ptr(),
                ffi::FORMAT_FLAG,
                (&raw mut flag).cast::<c_void>(),
            )
        })
    }

    pub fn set_double(&self, name: &str, value: f64) -> Result<(), MpvError> {
        let name = c(name)?;
        let mut number = value;
        // SAFETY: FORMAT_DOUBLE takes a pointer to a double.
        check(unsafe {
            ffi::mpv_set_property(
                self.handle,
                name.as_ptr(),
                ffi::FORMAT_DOUBLE,
                (&raw mut number).cast::<c_void>(),
            )
        })
    }

    pub fn get_double(&self, name: &str) -> Option<f64> {
        let name = c(name).ok()?;
        let mut number = 0f64;
        // SAFETY: FORMAT_DOUBLE writes a double into the pointer.
        let code = unsafe {
            ffi::mpv_get_property(
                self.handle,
                name.as_ptr(),
                ffi::FORMAT_DOUBLE,
                (&raw mut number).cast::<c_void>(),
            )
        };
        (code >= 0).then_some(number)
    }

    pub fn get_flag(&self, name: &str) -> Option<bool> {
        let name = c(name).ok()?;
        let mut flag: c_int = 0;
        // SAFETY: FORMAT_FLAG writes an int into the pointer.
        let code = unsafe {
            ffi::mpv_get_property(
                self.handle,
                name.as_ptr(),
                ffi::FORMAT_FLAG,
                (&raw mut flag).cast::<c_void>(),
            )
        };
        (code >= 0).then_some(flag != 0)
    }

    pub fn get_int(&self, name: &str) -> Option<i64> {
        let name = c(name).ok()?;
        let mut number = 0i64;
        // SAFETY: FORMAT_INT64 writes an int64 into the pointer.
        let code = unsafe {
            ffi::mpv_get_property(
                self.handle,
                name.as_ptr(),
                ffi::FORMAT_INT64,
                (&raw mut number).cast::<c_void>(),
            )
        };
        (code >= 0).then_some(number)
    }

    pub fn get_string(&self, name: &str) -> Option<String> {
        let name = c(name).ok()?;
        let mut text: *mut std::ffi::c_char = std::ptr::null_mut();
        // SAFETY: FORMAT_STRING writes a char* that we free with mpv_free.
        let code = unsafe {
            ffi::mpv_get_property(
                self.handle,
                name.as_ptr(),
                ffi::FORMAT_STRING,
                (&raw mut text).cast::<c_void>(),
            )
        };
        if code < 0 || text.is_null() {
            return None;
        }
        // SAFETY: mpv returned a NUL-terminated string it allocated.
        let value = unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned();
        // SAFETY: freeing what mpv allocated.
        unsafe { ffi::mpv_free(text.cast::<c_void>()) };
        Some(value)
    }

    /// Reports changes of `name` as [`Event::Flag`] (`flag`) or
    /// [`Event::Double`] events carrying `id`.
    pub fn observe(&self, id: u64, name: &str, flag: bool) -> Result<(), MpvError> {
        let name = c(name)?;
        let format = if flag {
            ffi::FORMAT_FLAG
        } else {
            ffi::FORMAT_DOUBLE
        };
        // SAFETY: valid handle and string.
        check(unsafe { ffi::mpv_observe_property(self.handle, id, name.as_ptr(), format) })
    }

    /// Waits up to `timeout` seconds (negative: forever) for the next event.
    pub fn wait_event(&self, timeout: f64) -> Option<Event> {
        // SAFETY: the returned event stays valid until the next wait on this
        // handle; everything is copied out before returning.
        let event = unsafe { &*ffi::mpv_wait_event(self.handle, timeout) };
        Some(match event.event_id {
            ffi::EVENT_NONE => return None,
            ffi::EVENT_SHUTDOWN => Event::Shutdown,
            ffi::EVENT_START_FILE => {
                // SAFETY: data is mpv_event_start_file for this event.
                let data = unsafe { &*event.data.cast::<ffi::MpvEventStartFile>() };
                Event::StartFile {
                    entry: data.playlist_entry_id,
                }
            }
            ffi::EVENT_FILE_LOADED => Event::FileLoaded,
            ffi::EVENT_PLAYBACK_RESTART => Event::PlaybackRestart,
            ffi::EVENT_END_FILE => {
                // SAFETY: data is mpv_event_end_file for this event.
                let data = unsafe { &*event.data.cast::<ffi::MpvEventEndFile>() };
                let reason = match data.reason {
                    ffi::END_FILE_REASON_EOF => EndReason::Eof,
                    ffi::END_FILE_REASON_STOP => EndReason::Stop,
                    ffi::END_FILE_REASON_QUIT => EndReason::Quit,
                    ffi::END_FILE_REASON_ERROR => EndReason::Error,
                    _ => EndReason::Other,
                };
                let error = (reason == EndReason::Error)
                    .then(|| check(data.error).err().map(|e| e.0))
                    .flatten();
                Event::EndFile {
                    entry: data.playlist_entry_id,
                    reason,
                    error,
                }
            }
            ffi::EVENT_PROPERTY_CHANGE => {
                // SAFETY: data is mpv_event_property for this event; its data
                // points to a value of the announced format, or is null.
                let property = unsafe { &*event.data.cast::<ffi::MpvEventProperty>() };
                match property.format {
                    ffi::FORMAT_FLAG if !property.data.is_null() => Event::Flag {
                        id: event.reply_userdata,
                        value: unsafe { *property.data.cast::<c_int>() } != 0,
                    },
                    ffi::FORMAT_DOUBLE if !property.data.is_null() => Event::Double {
                        id: event.reply_userdata,
                        value: Some(unsafe { *property.data.cast::<f64>() }),
                    },
                    ffi::FORMAT_NONE => Event::Double {
                        id: event.reply_userdata,
                        value: None,
                    },
                    _ => Event::Other,
                }
            }
            _ => Event::Other,
        })
    }

    /// Interrupts a [`Mpv::wait_event`] in another thread.
    pub fn wakeup(&self) {
        // SAFETY: valid handle.
        unsafe { ffi::mpv_wakeup(self.handle) }
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        // SAFETY: the last owner destroys the handle once.
        unsafe { ffi::mpv_terminate_destroy(self.handle) }
    }
}

/// Escapes an option value for mpv's `key=value,…` lists: `%<bytes>%value`.
pub fn escape_value(value: &str) -> String {
    format!("%{}%{value}", value.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_length_escaped() {
        assert_eq!(escape_value("a,b"), "%3%a,b");
        assert_eq!(
            escape_value("Mozilla/5.0 (KHTML, like Gecko)"),
            "%31%Mozilla/5.0 (KHTML, like Gecko)"
        );
    }

    #[test]
    fn a_silent_player_starts_and_reports_properties() {
        let mpv = Mpv::new(&[
            ("ao", "null"),
            ("vid", "no"),
            ("config", "no"),
            ("idle", "yes"),
            ("terminal", "no"),
        ])
        .unwrap();
        mpv.set_double("volume", 42.0).unwrap();
        assert_eq!(mpv.get_double("volume"), Some(42.0));
        assert_eq!(mpv.get_flag("idle-active"), Some(true));
        assert!(mpv.get_string("mpv-version").unwrap().starts_with("mpv "));
        assert!(mpv.command(&["no-such-command"]).is_err());
    }

    #[test]
    fn only_script_options_may_be_missing() {
        // Present or not in this libmpv, the scripts' options never fail.
        assert!(Mpv::new(&[("ao", "null"), ("osc", "no"), ("ytdl", "no")]).is_ok());
        let unknown = Mpv::new(&[("ao", "null"), ("no-such-option", "no")]).unwrap_err();
        assert_eq!(unknown, MpvError("option not found".into()));
    }
}
