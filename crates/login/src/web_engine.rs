//! The web engine's side, through `cpp/login.cpp`: initialization before
//! the application object, and the profile whose cookies come to Rust.

use std::ffi::c_void;
use std::sync::Arc;

use desktop::browser_cookies::BrowserCookie;
use qtbridge::QThreadHandle;

use crate::login::Login;

#[repr(C)]
struct Cookie {
    name: *const u8,
    name_len: usize,
    value: *const u8,
    value_len: usize,
    domain: *const u8,
    domain_len: usize,
    path: *const u8,
    path_len: usize,
    expires: i64,
    secure: bool,
    http_only: bool,
}

type CookieFn = extern "C" fn(context: *mut c_void, added: bool, cookie: *const Cookie);

unsafe extern "C" {
    fn orbis_login_initialize();
    fn orbis_login_install(engine: *mut c_void, callback: CookieFn, context: *mut c_void);
}

/// Initializes Qt WebEngine; call before the application object exists.
pub fn initialize() {
    // SAFETY: called once, on the main thread, before `QApp::new`.
    unsafe { orbis_login_initialize() }
}

/// Gives QML the window's profile as `loginProfile` and reports its cookies
/// to `login`. Call before the QML is loaded.
pub fn install<T>(engine: std::pin::Pin<&mut T>, login: Arc<QThreadHandle<Login>>) {
    // SAFETY: only the address is taken; C++ uses the engine on this thread
    // while it lives, and it lives as long as the application.
    let engine = unsafe { engine.get_unchecked_mut() } as *mut T as *mut c_void;
    // The handle is kept for the life of the process, which ends with
    // `_exit`.
    let context = Arc::into_raw(login) as *mut c_void;
    // SAFETY: `engine` is the application's QML engine, `context` stays
    // valid for every later call of `on_cookie`.
    unsafe { orbis_login_install(engine, on_cookie, context) }
}

fn text(pointer: *const u8, len: usize) -> String {
    if pointer.is_null() || len == 0 {
        return String::new();
    }
    // SAFETY: C++ passes a buffer of `len` bytes that lives for the call.
    String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(pointer, len) }).into_owned()
}

extern "C" fn on_cookie(context: *mut c_void, added: bool, cookie: *const Cookie) {
    if context.is_null() || cookie.is_null() {
        return;
    }
    // SAFETY: `context` came from `Arc::into_raw` in `install` and is never
    // released; `cookie` is valid for the call.
    let (login, cookie) = unsafe { (&*(context as *const QThreadHandle<Login>), &*cookie) };
    let cookie = BrowserCookie {
        name: text(cookie.name, cookie.name_len),
        value: text(cookie.value, cookie.value_len),
        host: text(cookie.domain, cookie.domain_len),
        path: text(cookie.path, cookie.path_len),
        expires: (cookie.expires >= 0).then_some(cookie.expires),
        secure: cookie.secure,
        http_only: cookie.http_only,
    };
    // The store reports from inside the web engine; the object takes the
    // cookie on its own turn of the event loop.
    login.invoke_mut(move |login| login.take_cookie(added, cookie));
}
