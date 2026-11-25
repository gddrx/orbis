//! The part of libmpv's C API (`mpv/client.h`, API 2.x) the player uses.
//! Layouts and values follow the header of mpv 0.42.

use std::ffi::{c_char, c_double, c_int, c_void};

#[repr(C)]
pub struct MpvHandle {
    _private: [u8; 0],
}

/// `MPV_ERROR_OPTION_NOT_FOUND`.
pub const ERROR_OPTION_NOT_FOUND: c_int = -5;

pub const FORMAT_NONE: c_int = 0;
pub const FORMAT_STRING: c_int = 1;
pub const FORMAT_FLAG: c_int = 3;
pub const FORMAT_INT64: c_int = 4;
pub const FORMAT_DOUBLE: c_int = 5;

pub const EVENT_NONE: c_int = 0;
pub const EVENT_SHUTDOWN: c_int = 1;
pub const EVENT_START_FILE: c_int = 6;
pub const EVENT_END_FILE: c_int = 7;
pub const EVENT_FILE_LOADED: c_int = 8;
pub const EVENT_PLAYBACK_RESTART: c_int = 21;
pub const EVENT_PROPERTY_CHANGE: c_int = 22;

pub const END_FILE_REASON_EOF: c_int = 0;
pub const END_FILE_REASON_STOP: c_int = 2;
pub const END_FILE_REASON_QUIT: c_int = 3;
pub const END_FILE_REASON_ERROR: c_int = 4;

#[repr(C)]
pub struct MpvEvent {
    pub event_id: c_int,
    pub error: c_int,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct MpvEventProperty {
    pub name: *const c_char,
    pub format: c_int,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct MpvEventEndFile {
    pub reason: c_int,
    pub error: c_int,
    pub playlist_entry_id: i64,
    pub playlist_insert_id: i64,
    pub playlist_insert_num_entries: c_int,
}

#[repr(C)]
pub struct MpvEventStartFile {
    pub playlist_entry_id: i64,
}

unsafe extern "C" {
    pub fn mpv_create() -> *mut MpvHandle;
    pub fn mpv_initialize(ctx: *mut MpvHandle) -> c_int;
    pub fn mpv_terminate_destroy(ctx: *mut MpvHandle);
    pub fn mpv_set_option_string(
        ctx: *mut MpvHandle,
        name: *const c_char,
        data: *const c_char,
    ) -> c_int;
    pub fn mpv_command(ctx: *mut MpvHandle, args: *mut *const c_char) -> c_int;
    pub fn mpv_set_property(
        ctx: *mut MpvHandle,
        name: *const c_char,
        format: c_int,
        data: *mut c_void,
    ) -> c_int;
    pub fn mpv_set_property_string(
        ctx: *mut MpvHandle,
        name: *const c_char,
        data: *const c_char,
    ) -> c_int;
    pub fn mpv_get_property(
        ctx: *mut MpvHandle,
        name: *const c_char,
        format: c_int,
        data: *mut c_void,
    ) -> c_int;
    pub fn mpv_observe_property(
        ctx: *mut MpvHandle,
        reply_userdata: u64,
        name: *const c_char,
        format: c_int,
    ) -> c_int;
    pub fn mpv_wait_event(ctx: *mut MpvHandle, timeout: c_double) -> *mut MpvEvent;
    pub fn mpv_wakeup(ctx: *mut MpvHandle);
    pub fn mpv_free(data: *mut c_void);
    pub fn mpv_error_string(error: c_int) -> *const c_char;
}
