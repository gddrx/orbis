//! The orbis player. The only crate that knows about Qt: the QML UI and the
//! bridge to the Rust core live here.
//!
//! Environment:
//! - `ORBIS_DATA_DIR` — where the tabs database lives (default
//!   `$XDG_DATA_HOME/orbis`).
//! - `ORBIS_CACHE_DIR` — where covers, their blurred backdrops and yt-dlp's
//!   cache (`yt-dlp/`) are kept (default `$XDG_CACHE_HOME/orbis`).
//! - `ORBIS_NET_LOG=1` — logs the core's network requests to stderr.
//! - `QT_QUICK_BACKEND` — Qt Quick's renderer; orbis draws in software unless
//!   it is set (`rhi` brings back the GPU renderer). The software renderer
//!   repaints only what changed, also at a fractional scale;
//!   `QSG_SOFTWARE_RENDERER_FORCE_PARTIAL_UPDATES=0` brings back Qt's full
//!   repaints there.
//! - `ORBIS_AUDIO_OUTPUT` — mpv's audio output; `null` plays silently.
//! - `ORBIS_YTDLP_QUICK=0` — yt-dlp finds links its own way, never the
//!   quick one (see `stream::ytdlp`).
//! - `ORBIS_YTDLP_WORKER=0` — no long-lived yt-dlp while the window is
//!   shown: every link is a yt-dlp run of its own (see `stream::worker`).
//! - `ORBIS_PREFETCH_VISIBLE=0` — the links of the rows in view are not
//!   fetched ahead when the list comes to rest (see `ahead.rs` in
//!   orbis-core).
//! - `ORBIS_BLUR=0` — does not ask the compositor to blur behind the window,
//!   which then stays opaque.
//! - `ORBIS_TRAY=0` — no icon in the tray; closing the window quits.
//! - `ORBIS_DESKTOP_BUS=<address>` — the D-Bus bus of the tray, MPRIS and the
//!   single instance instead of the session bus (for tests that must not show
//!   up on the desktop).
//! - `ORBIS_TRAY_UNLOAD=0` — the window hidden in the tray is only hidden, not
//!   unloaded (see `shell.rs`).
//! - `ORBIS_SMOKE_TEST=1` — drives the window through a short scripted
//!   session, prints memory and CPU, and quits; with
//!   `ORBIS_SMOKE_SCREENSHOT=<file.png>` it also saves a picture of the window.
//!   It starts by scrolling the whole home page, then reports what the cover
//!   cache fetched, and ends counting the frames the window draws while a
//!   track plays and while it is paused. `ORBIS_SMOKE_WRITE=1` checks the
//!   track menu's writes to the account instead (see `smoke.rs`).
//! - `ORBIS_SMOKE_SIGN_IN=1` — checks the sign-in dialog's actions on a start
//!   without a session, and quits (see `smoke.rs`).
//! - `ORBIS_SMOKE_SETTINGS=write`, then `read` — changes every setting, then
//!   checks they came back after the restart; needs `ORBIS_CONFIG_DIR`.
//! - `ORBIS_CONFIG_DIR` — where `config.toml` lives (default
//!   `$XDG_CONFIG_HOME/orbis`). The settings are changed in the window; the
//!   variables below change only the run they are set for.
//! - `ORBIS_LOGIN_BIN` — the Google sign-in window's program (default
//!   `orbis-login` next to `orbis`).
//! - `ORBIS_MUSIC_DIR` — where downloads go instead of the desktop's music
//!   folder; `ORBIS_DOWNLOAD_FORMAT` — `opus` (default), `mp3` or `m4a`;
//!   `ORBIS_MUSICBRAINZ=off` — tags of downloads without MusicBrainz.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use orbis_core::{CoverCache, Playback, PlaybackEvent, SessionManager, covers::DEFAULT_LIMIT};
use qtbridge::{QApp, QmlObject, include_bytes_qml};
use tokio::sync::mpsc::UnboundedReceiver;

mod backend;
mod content;
mod covers;
mod glass;
mod i18n;
mod models;
mod preferences;
mod shell;
mod smoke;
mod start;
mod window_system;

use backend::{Backend, Models, Services};
use covers::Covers;
use glass::Glass;
use models::{CardsModel, DownloadsModel, ShelvesModel, TabsModel, TracksModel};
use shell::Shell;
use start::Starter;

/// `$<override>`, else `$<xdg>/orbis`, else `~/<fallback>/orbis`.
fn xdg_dir(override_var: &str, xdg_var: &str, fallback: &str) -> PathBuf {
    if let Some(dir) = std::env::var_os(override_var) {
        return PathBuf::from(dir);
    }
    std::env::var_os(xdg_var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(fallback)))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("orbis")
}

fn data_dir() -> PathBuf {
    xdg_dir("ORBIS_DATA_DIR", "XDG_DATA_HOME", ".local/share")
}

fn cache_dir() -> PathBuf {
    desktop::cache_dir()
}

/// Opens the saved session and starts playback. Without a session there is
/// nothing to start and no error: the window asks the user to sign in.
#[allow(clippy::type_complexity)]
fn start(
    runtime: &tokio::runtime::Runtime,
    starter: &Starter,
) -> Result<Option<(Services, UnboundedReceiver<PlaybackEvent>)>, String> {
    let session = match runtime.block_on(SessionManager::open()) {
        Ok(session) => Arc::new(session),
        Err(e) if e.needs_sign_in() => return Ok(None),
        Err(e) => return Err(i18n::trf("The saved sign-in is not available: %1", &[&e])),
    };
    let (playback, events) = runtime.block_on(starter.start(Arc::clone(&session)))?;
    Ok(Some((Services { session, playback }, events)))
}

/// Another orbis runs: asks it to show its window, with the activation
/// token the launcher gave this start, and leaves.
fn show_running_and_exit(runtime: &tokio::runtime::Runtime, bus: &desktop::Connection) -> ! {
    let token = std::env::var("XDG_ACTIVATION_TOKEN").ok();
    match runtime.block_on(desktop::Mpris::show_running(bus, "orbis", token)) {
        Ok(()) => eprintln!("orbis: already running; its window is shown"),
        Err(e) => eprintln!("orbis: already running, but it did not answer: {e}"),
    }
    use std::io::Write;
    let _ = std::io::stderr().flush();
    // SAFETY: ends the process; no Qt object exists yet.
    unsafe { libc::_exit(0) }
}

/// When `main` began: the start's first frame is timed from it.
pub static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// The start's steps and when each ended, from `main` (`ORBIS_SMOKE_START`
/// prints them with the first frame).
pub static START_STEPS: std::sync::Mutex<Vec<(&'static str, Duration)>> =
    std::sync::Mutex::new(Vec::new());

/// Notes that the start's step `name` is over.
fn start_step(name: &'static str) {
    if let (Some(started), Ok(mut steps)) = (STARTED.get(), START_STEPS.lock()) {
        steps.push((name, started.elapsed()));
    }
}

/// How long this process has lived, from the kernel's clock (10 ms steps):
/// what comes before `main` — loading the binary and its libraries — too.
pub fn process_age() -> Option<Duration> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // Fields after the command, which ends with the last ')'.
    let fields: Vec<&str> = stat[stat.rfind(')')? + 2..].split(' ').collect();
    let started_ticks: f64 = fields.get(19)?.parse().ok()?;
    let uptime: f64 = std::fs::read_to_string("/proc/uptime")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    // SAFETY: sysconf only reads a system constant.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    (ticks > 0.0).then(|| Duration::from_secs_f64((uptime - started_ticks / ticks).max(0.0)))
}

fn main() {
    // `orbis --version` prints the version and starts nothing.
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--version")
    {
        println!("orbis {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    // Blocks of 128 KB and more (big responses as they are parsed, decoded
    // pictures) get mappings of their own, which go back to the system when
    // freed. glibc otherwise raises this threshold as such blocks are freed
    // and keeps them in its heap: after a tour of eleven tabs PSS was 160
    // MB, 143 MB with the threshold held.
    // SAFETY: a plain libc call, made before any other thread exists.
    unsafe {
        libc::mallopt(libc::M_MMAP_THRESHOLD, 128 * 1024);
    }
    STARTED.get_or_init(std::time::Instant::now);
    // The software renderer keeps only decoded pictures, without textures and
    // the GPU driver's copies of them: measured on step 13, PSS 126–135 MB
    // against 178–179 MB with OpenGL after scrolling the whole home page, for
    // three times the CPU while scrolling. The owner chose it as the default
    // (22 November 2026).
    if std::env::var_os("QT_QUICK_BACKEND").is_none() {
        // SAFETY: the first thing main does, before any other thread exists.
        unsafe { std::env::set_var("QT_QUICK_BACKEND", "software") };
    }
    // Qt Quick packs every picture up to half the window's size into shared
    // atlas textures that never give space back, and covers fill one atlas
    // after another (measured on step 13 with OpenGL: 12 MB more PSS after
    // scrolling the home page). The window shows no other pictures, so it
    // goes without an atlas unless told otherwise.
    if std::env::var_os("QSG_ATLAS_SIZE_LIMIT").is_none() {
        // SAFETY: the first thing main does, before any other thread exists.
        unsafe { std::env::set_var("QSG_ATLAS_SIZE_LIMIT", "1") };
    }
    // At a fractional scale (1.5 on the owner's screen) the software renderer
    // repaints the whole window on every frame, so the progress, once a
    // second, cost 3–5 ms over the glass backdrop instead of next to nothing.
    // Forced partial updates repaint only what changed; on a probe at 1.5
    // the picture came out identical to a full repaint (step 15).
    if std::env::var_os("QSG_SOFTWARE_RENDERER_FORCE_PARTIAL_UPDATES").is_none() {
        // SAFETY: the first thing main does, before any other thread exists.
        unsafe { std::env::set_var("QSG_SOFTWARE_RENDERER_FORCE_PARTIAL_UPDATES", "1") };
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("orbis-core")
        .enable_all()
        .build()
        .expect("tokio runtime");

    // One orbis at a time: MPRIS's bus name is taken before anything else,
    // and a second start shows the first one's window instead.
    let (media_sender, media_events) = tokio::sync::mpsc::unbounded_channel();
    let desktop_bus = runtime
        .block_on(desktop::connect(
            std::env::var("ORBIS_DESKTOP_BUS").ok().as_deref(),
        ))
        .inspect_err(|e| eprintln!("orbis: no desktop bus (tray, media keys): {e}"))
        .ok();
    let mpris = match &desktop_bus {
        Some(bus) => match runtime.block_on(desktop::Mpris::claim(bus, "orbis", media_sender)) {
            Ok(Some(mpris)) => Some(Arc::new(mpris)),
            Ok(None) => show_running_and_exit(&runtime, bus),
            Err(e) => {
                eprintln!("orbis: no media keys: {e}");
                None
            }
        },
        None => None,
    };
    start_step("bus and MPRIS");

    let mut app = QApp::new();
    window_system::set_window_icon();
    start_step("Qt application");
    // Qt sets the user's locale; libmpv refuses to start unless numbers are
    // formatted the C way.
    // SAFETY: called on the main thread before mpv or any other thread that
    // reads the locale starts.
    unsafe {
        libc::setlocale(libc::LC_NUMERIC, c"C".as_ptr());
    }
    include_bytes_qml!("qml", "orbis");

    let models = Models {
        tabs: TabsModel::create(),
        tracks: TracksModel::create(),
        cards: CardsModel::create(),
        shelves: ShelvesModel::create(),
        options: CardsModel::create(),
        downloads: DownloadsModel::create(),
    };
    let (tabs, tracks, cards, shelves, options, downloads) = (
        models.tabs.clone(),
        models.tracks.clone(),
        models.cards.clone(),
        models.shelves.clone(),
        models.options.clone(),
        models.downloads.clone(),
    );

    let cover_cache = {
        let _context = runtime.enter();
        let dir = cache_dir().join("covers");
        CoverCache::open(dir.clone(), DEFAULT_LIMIT)
            .inspect_err(|e| eprintln!("orbis: no cover cache in {}: {e}", dir.display()))
            .ok()
    };
    let covers = Covers::create(cover_cache.clone(), runtime.handle().clone());
    let settings_path = orbis_core::Settings::path();
    let (stored, settings_problem) = preferences::load(&settings_path);
    let settings = preferences::with_environment(stored.clone());
    i18n::set_language(i18n::Language::from_code(&settings.language));

    let glass = Glass::create(
        settings.window.glass,
        f64::from(settings.window.opacity) / 100.0,
        cover_cache.clone(),
        cache_dir().join("backdrops"),
        runtime.handle().clone(),
    );

    let starter = Starter {
        data_dir: data_dir(),
        audio: std::env::var("ORBIS_AUDIO_OUTPUT").ok(),
    };
    start_step("covers and glass");
    let (started, status) = match start(&runtime, &starter) {
        Ok(started) => (started, String::new()),
        Err(message) => {
            eprintln!("orbis: {message}");
            (None, message)
        }
    };
    let session = started.as_ref().map(|(s, _)| Arc::clone(&s.session));
    // MPRIS reads the position from playback, whenever playback starts:
    // now, or after the user signs in.
    let on_started = {
        let mpris = mpris.clone();
        Box::new(move |playback: &Arc<Playback>| {
            let Some(mpris) = &mpris else {
                return;
            };
            let playback = Arc::clone(playback);
            mpris.set_position_source(move || {
                playback
                    .position()
                    .filter(|p| p.is_finite() && *p >= 0.0)
                    .map(Duration::from_secs_f64)
            });
        })
    };
    let backend = Backend::create(
        runtime.handle().clone(),
        starter,
        on_started,
        models,
        status,
        &settings,
    );
    if let Some((services, events)) = started {
        backend.borrow_mut().install(services, events);
    }

    let shell = Shell::create(
        Arc::new(backend.borrow().get_thread_handle()),
        runtime.handle().clone(),
        settings.window.unload_in_tray,
        desktop_bus,
        mpris,
        cover_cache.clone(),
    );
    shell.borrow_mut().start_mpris(media_events);
    if settings.window.tray {
        shell.borrow_mut().start_tray();
    }
    let preferences = preferences::Preferences::create(
        stored,
        settings,
        settings_path,
        settings_problem,
        runtime.handle().clone(),
        Arc::new(backend.borrow().get_thread_handle()),
        Arc::new(glass.borrow().get_thread_handle()),
        Arc::new(shell.borrow().get_thread_handle()),
    );

    if std::env::var_os("ORBIS_SMOKE_SETTINGS").is_some() {
        smoke::run_settings(
            &runtime,
            Arc::new(backend.borrow().get_thread_handle()),
            Arc::new(glass.borrow().get_thread_handle()),
            Arc::new(shell.borrow().get_thread_handle()),
            Arc::new(preferences.borrow().get_thread_handle()),
        );
    } else if std::env::var_os("ORBIS_SMOKE_SIGN_IN").is_some() {
        smoke::run_sign_in(
            &runtime,
            Arc::new(backend.borrow().get_thread_handle()),
            Arc::new(shell.borrow().get_thread_handle()),
        );
    } else if std::env::var_os("ORBIS_SMOKE_TEST").is_some() {
        smoke::run(
            &runtime,
            Arc::new(backend.borrow().get_thread_handle()),
            Arc::new(glass.borrow().get_thread_handle()),
            Arc::new(shell.borrow().get_thread_handle()),
            cover_cache,
            session.clone(),
        );
    }

    start_step("session and playback, backend, shell, settings");
    let code = app
        .set_initial_object("backend", backend.clone())
        .set_initial_object("covers", covers)
        .set_initial_object("glass", glass)
        .set_initial_object("shell", shell)
        .set_initial_object("settings", preferences)
        .set_initial_object("tabsModel", tabs)
        .set_initial_object("tracksModel", tracks)
        .set_initial_object("cardsModel", cards)
        .set_initial_object("shelvesModel", shelves)
        .set_initial_object("optionsModel", options)
        .set_initial_object("downloadsModel", downloads)
        .load_qml_from_file("qrc:/orbis/qml/Main.qml")
        .run();

    // Orderly shutdown: save rotated cookies, stop mpv, stop the tasks. The
    // session may have come after the start, with a sign-in.
    let services = backend.borrow_mut().take_services();
    if let Some(services) = &services {
        let _ = runtime.block_on(services.session.flush());
    }
    drop(services);
    runtime.shutdown_timeout(Duration::from_secs(2));
    drop(backend);
    // Everything worth keeping is saved: tabs on every change, cookies above,
    // and mpv is stopped. C exit handlers would now delete Qt objects after
    // the Qt Quick theme is gone and crash, so leave without them.
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    // SAFETY: ends the process; nothing runs after it.
    unsafe { libc::_exit(code) }
}
