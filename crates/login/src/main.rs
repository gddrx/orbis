//! `orbis-login`: the Google sign-in window of orbis.
//!
//! orbis starts it when the user signs in "through the Google window": a web
//! engine (Qt WebEngine) with a profile in memory shows Google's sign-in,
//! and once the page reaches YouTube Music the profile's cookies of the
//! domains given with `--domain` go back to orbis through the pipe on fd 3
//! (see `desktop::login_window`), and the process ends. The web engine never
//! lives in orbis itself.
//!
//! `--language en|uk` — the language of the window's few words (orbis
//! passes its own).
//!
//! Exit codes: 0 — the cookies were sent; 1 — the window was closed without
//! signing in; 2 — the cookies could not be sent.
//!
//! `ORBIS_LOGIN_TRACE=1` prints the pages the window goes through (without
//! queries) and the names and hosts of the cookies it gets (without values).

use desktop::login_window::Channel;
use qtbridge::{QApp, QmlObject};

mod login;
mod web_engine;

use login::Login;

/// The domains after `--domain`; YouTube's and Google's when none is given.
fn domains() -> Vec<String> {
    let mut domains = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--domain"
            && let Some(domain) = args.next()
        {
            domains.push(domain.to_ascii_lowercase());
        }
    }
    if domains.is_empty() {
        domains = vec!["youtube.com".to_owned(), "google.com".to_owned()];
    }
    domains
}

/// The code after `--language`, or English.
fn language() -> String {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--language"
            && let Some(code) = args.next()
        {
            return code;
        }
    }
    "en".to_owned()
}

fn main() {
    // First: the channel must not reach the web engine's processes.
    let channel = Channel::from_env();
    let domains = domains();

    web_engine::initialize();
    let mut app = QApp::new();
    let login = Login::create(channel, domains, language());
    web_engine::install(
        app.qml_engine(),
        std::sync::Arc::new(login.borrow().get_thread_handle()),
    );
    app.application_name("orbis-login")
        .set_initial_object("login", login.clone())
        .load_qml(include_bytes!("qml/Login.qml"))
        .run();

    let code = login.borrow().exit_code();
    use std::io::Write;
    let _ = std::io::stderr().flush();
    // SAFETY: ends the process; the cookies are sent and nothing is kept.
    // Exit handlers would tear the web engine down after Qt and may crash.
    unsafe { libc::_exit(code) }
}
