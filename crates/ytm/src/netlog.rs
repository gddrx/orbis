//! A log of the core's network requests on stderr, for checking what the
//! player fetches and when. On with `ORBIS_NET_LOG=1`; lines carry the time
//! since the first logged request, never cookies or response bodies.

use std::sync::OnceLock;
use std::time::Instant;

fn start() -> Option<Instant> {
    static START: OnceLock<Option<Instant>> = OnceLock::new();
    *START.get_or_init(|| {
        std::env::var_os("ORBIS_NET_LOG")
            .is_some_and(|v| !v.is_empty() && v != "0")
            .then(Instant::now)
    })
}

/// Whether the log is on.
pub fn enabled() -> bool {
    start().is_some()
}

/// Writes one line to the log when it is on.
pub fn log(line: std::fmt::Arguments<'_>) {
    if let Some(start) = start() {
        eprintln!("[{:8.3} s] net: {line}", start.elapsed().as_secs_f64());
    }
}
