//! The smoke test (`ORBIS_SMOKE_TEST=1`): the window drives itself through
//! a scripted session, prints what it fetched, its memory, CPU and the frames
//! it draws while playing and paused, then hides in the tray (unloading the
//! window unless `ORBIS_TRAY_UNLOAD=0`), measures memory and CPU there, comes
//! back, saves pictures of itself with `ORBIS_SMOKE_SCREENSHOT`, and quits.
//!
//! With `ORBIS_SMOKE_WRITE=1` it instead checks the track menu's items that
//! write to the account — like, dislike, a new private playlist, adding to
//! it — on a real track, undoes each, and checks every step through the API.
//! Only with the account owner's leave.
//!
//! `ORBIS_SMOKE_SIGN_IN=1` checks the sign-in dialog's actions without
//! signing in, for a start without a session (see [`run_sign_in`]).
//!
//! `ORBIS_SMOKE_SETTINGS=write`, then `=read` in a second run, checks that
//! every setting changes, applies and survives the restart (see
//! [`run_settings`]); only with `ORBIS_CONFIG_DIR`, so that the owner's
//! settings are never touched.
//!
//! With `ORBIS_MUSIC_DIR` set the session also downloads (see
//! `smoke_downloads`); without it nothing is downloaded, so a smoke test
//! never writes into the owner's music folder. `ORBIS_SMOKE_DOWNLOADS=1`
//! runs only the downloads, on the first track of Discover Mix; with
//! `ORBIS_SMOKE_KEEP_DOWNLOADS=1` it leaves an album's downloads running
//! and says how many, and a second run over the same data folder with
//! `ORBIS_SMOKE_DOWNLOADS_BACK=<that many>` checks that they came back (see
//! `smoke_downloads_back`).

use std::sync::Arc;
use std::time::Duration;

use orbis_core::{CoverCache, SessionManager};
use player::TabSource;
use ytm::LikeStatus;

use crate::backend::{self, Backend, ShelvesPlace, SmokeState};
use crate::glass::Glass;
use crate::preferences::Preferences;
use crate::shell::Shell;

type Window = Arc<qtbridge::QThreadHandle<Backend>>;
type GlassHandle = Arc<qtbridge::QThreadHandle<Glass>>;
type ShellHandle = Arc<qtbridge::QThreadHandle<Shell>>;
type PreferencesHandle = Arc<qtbridge::QThreadHandle<Preferences>>;

async fn sleep(seconds: u64) {
    tokio::time::sleep(Duration::from_secs(seconds)).await;
}

/// Asks the backend on the GUI thread and waits for the answer.
async fn ask<T: Send + 'static>(
    backend: &Window,
    question: impl FnOnce(&Backend) -> T + Send + 'static,
) -> Option<T> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    backend.invoke(move |b| {
        let _ = sender.send(question(b));
    });
    receiver.await.ok()
}

/// Changes the glass object on the GUI thread and waits for the answer.
async fn ask_glass<T: Send + 'static>(
    glass: &GlassHandle,
    question: impl FnOnce(&mut Glass) -> T + Send + 'static,
) -> Option<T> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    glass.invoke_mut(move |g| {
        let _ = sender.send(question(g));
    });
    receiver.await.ok()
}

async fn state(backend: &Window) -> SmokeState {
    ask(backend, |b| b.smoke_state()).await.unwrap_or_default()
}

/// Prints one check and tells whether it passed.
fn check(what: &str, passed: bool) -> bool {
    println!(
        "smoke test, {what}: {}",
        if passed { "OK" } else { "FAILED" }
    );
    passed
}

/// The windows made on first use open — the track menu, «Add to playlist»,
/// the downloads list and the sign-in dialog — one at a time, for the
/// bench's pictures, and close again.
async fn smoke_windows(backend: &Window) {
    backend.invoke_mut(|b| b.menu_for_current());
    sleep(3).await;
    let mut shown = Vec::new();
    for name in ["menu", "playlist", "downloads", "login"] {
        if name == "downloads" {
            backend.invoke_mut(|b| b.request_downloads_list(true));
        } else {
            backend.invoke_mut(move |b| b.request_window(name.into()));
        }
        sleep(1).await;
        if state(backend).await.window_shown == name {
            shown.push(name);
        }
        // Long enough for a picture of the whole screen (every 20 s).
        sleep(20).await;
        if name == "downloads" {
            backend.invoke_mut(|b| b.request_downloads_list(false));
        } else {
            backend.invoke_mut(|b| b.request_window(String::new()));
        }
        sleep(1).await;
    }
    let closed = state(backend).await.window_shown.is_empty();
    println!("smoke test, windows made on first use: shown {shown:?}, closed again {closed}");
    check("windows made on first use", shown.len() == 4 && closed);
}

/// The track menu's items that write nothing to the account, on tracks of
/// the mix that plays. The radio comes last: it changes the tab that plays.
async fn smoke_menu(backend: &Window) {
    backend.invoke_mut(|b| b.menu_for_row(3));
    sleep(3).await;
    let row3 = state(backend).await;
    println!(
        "smoke test, track menu: «{}» — album «{}», artists {:?}, rating {:?}, details {}",
        row3.menu_title,
        row3.menu_album,
        row3.menu_artists,
        row3.menu_rating,
        if row3.menu_ready {
            "came"
        } else {
            "did not come"
        }
    );
    backend.invoke_mut(|b| b.menu_play_next());
    sleep(1).await;
    let after = state(backend).await;
    check(
        "menu «Play next»",
        after.next.as_deref() == Some(row3.menu_title.as_str()),
    );

    backend.invoke_mut(|b| b.menu_for_row(4));
    sleep(2).await;
    let row4 = state(backend).await;
    backend.invoke_mut(|b| b.menu_add_to_queue());
    sleep(1).await;
    let after = state(backend).await;
    check(
        "menu «Add to queue»",
        after.last.as_deref() == Some(row4.menu_title.as_str()),
    );

    backend.invoke_mut(|b| b.menu_for_row(3));
    sleep(3).await;
    backend.invoke_mut(|b| b.menu_open_album());
    sleep(4).await;
    let album = state(backend).await;
    check(
        "menu «Album»",
        !row3.menu_album.is_empty() && album.shown_kind == "album",
    );
    smoke_screenshot(backend, "album").await;
    backend.invoke_mut(|b| b.menu_open_artist(0));
    sleep(4).await;
    let artist = state(backend).await;
    check(
        "menu «Artist»",
        !row3.menu_artists.is_empty() && artist.shown_kind == "artist",
    );
    smoke_screenshot(backend, "artist").await;

    backend.invoke_mut(|b| b.menu_radio());
    sleep(10).await;
    let radio = state(backend).await;
    check(
        "menu «Start radio»",
        radio.playing_kind == "radio" && radio.shown_kind == "radio",
    );
}

/// The rating of `video_id` as the API, and so the web version, has it.
async fn api_rating(session: &SessionManager, video_id: &str) -> Option<LikeStatus> {
    session
        .call(|c| {
            let video_id = video_id.to_owned();
            async move { c.watch(&video_id, None).await }
        })
        .await
        .ok()
        .and_then(|q| q.current_like)
}

/// The library playlist titled `title`, by ID.
async fn library_playlist(session: &SessionManager, title: &str) -> Option<String> {
    session
        .call(|c| async move { c.library_playlists().await })
        .await
        .ok()?
        .into_iter()
        .find(|p| p.title == title)
        .map(|p| p.playlist_id().to_owned())
}

/// The menu's items that write to the account, each undone and checked
/// through the API. Returns how many checks passed of how many.
async fn smoke_write(backend: &Window, session: &SessionManager, mix: u64) -> (usize, usize) {
    backend.invoke_mut(move |b| b.show_tab(mix as i64));
    sleep(6).await;
    // A track of the mix the account has not rated.
    let mut track = None;
    for row in 8..30 {
        backend.invoke_mut(move |b| b.menu_for_row(row));
        sleep(3).await;
        let s = state(backend).await;
        if s.menu_ready && s.menu_rating.is_empty() {
            track = Some(s);
            break;
        }
    }
    let Some(track) = track else {
        println!("smoke test, write: no track without a rating in rows 8–29");
        return (0, 1);
    };
    let video = track.menu_video_id.clone();
    println!("smoke test, write: on «{}» ({video})", track.menu_title);
    let mut passed = Vec::new();

    for (action, expected) in [
        ("like", LikeStatus::Like),
        ("like again", LikeStatus::Indifferent),
        ("dislike", LikeStatus::Dislike),
        ("dislike again", LikeStatus::Indifferent),
    ] {
        if action.starts_with("like") {
            backend.invoke_mut(|b| b.menu_like());
        } else {
            backend.invoke_mut(|b| b.menu_dislike());
        }
        sleep(3).await;
        let rating = api_rating(session, &video).await;
        let shown = state(backend).await.menu_rating;
        passed.push(check(
            &format!("write, {action}: the API has {rating:?}, the menu shows {shown:?}"),
            rating == Some(expected),
        ));
    }

    let title = format!("orbis test {}", ytm::unix_now());
    let name = title.clone();
    backend.invoke_mut(move |b| b.menu_create_playlist(name, "private".into()));
    sleep(5).await;
    let created = library_playlist(session, &title).await;
    passed.push(check(
        &format!("write, new private playlist «{title}» in the library"),
        created.is_some(),
    ));
    if let Some(id) = created.clone() {
        let tracks = session
            .call(|c| {
                let id = id.clone();
                async move { c.playlist(&id).await }
            })
            .await
            .map(|p| {
                p.tracks
                    .iter()
                    .map(|t| t.video_id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        passed.push(check(
            "write, the new playlist holds the track",
            tracks == [video.clone()],
        ));
        let (id_for_menu, name) = (id.clone(), title.clone());
        backend.invoke_mut(move |b| b.menu_add_to(id_for_menu, name));
        sleep(4).await;
        let status = state(backend).await.status;
        passed.push(check(
            &format!("write, adding it again is refused: {status:?}"),
            status.starts_with(&format!("«{title}»:")),
        ));
    }

    // Undo whatever is left, whatever failed above.
    if api_rating(session, &video).await != Some(LikeStatus::Indifferent) {
        let _ = session
            .call(|c| {
                let video = video.clone();
                async move { c.rate(&video, ytm::Rating::None).await }
            })
            .await;
    }
    if let Some(id) = created {
        let _ = session
            .call(|c| {
                let id = id.clone();
                async move { c.delete_playlist(&id).await }
            })
            .await;
        sleep(2).await;
        passed.push(check(
            "write, the test playlist is deleted",
            library_playlist(session, &title).await.is_none(),
        ));
    }
    passed.push(check(
        "write, the track is left without a rating",
        api_rating(session, &video).await == Some(LikeStatus::Indifferent),
    ));
    (passed.iter().filter(|p| **p).count(), passed.len())
}

/// Where the shown shelves are scrolled, after moving them a little into
/// shelf `shelf` unless it is -1.
async fn shelves_place(backend: &Window, shelf: i32) -> Option<ShelvesPlace> {
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    backend.invoke_mut(move |b| b.request_place(shelf, sender));
    tokio::time::timeout(Duration::from_secs(5), receiver.recv())
        .await
        .ok()
        .flatten()
}

fn place_text(place: Option<ShelvesPlace>) -> String {
    match place {
        None => "?".to_owned(),
        Some(p) if p.at_end => "the end".to_owned(),
        Some(p) => format!("shelf {} + {:.0} px", p.shelf, p.offset),
    }
}

/// The home page and the search: a new tab's shelves start at the top, and
/// each tab's shelves are where they were left — at the end of the home page
/// after the scroll, and inside a shelf after that.
async fn smoke_places(backend: &Window, home: u64, home_end: Option<ShelvesPlace>) {
    let search = ask(backend, |b| b.tab_ids())
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|(_, s)| matches!(s, TabSource::Search { .. }))
        .map(|(id, _)| id);
    let (Some(search), Some(home_end)) = (search, home_end) else {
        println!("smoke test, places: no search tab or no place of the home page");
        return;
    };
    let show = |id: u64| backend.invoke_mut(move |b| b.show_tab(id as i64));
    let search_top = shelves_place(backend, -1).await;
    show(home);
    sleep(2).await;
    let home_again = shelves_place(backend, -1).await;
    let home_inside = shelves_place(backend, 6).await;
    show(search);
    sleep(2).await;
    show(home);
    sleep(2).await;
    let home_inside_again = shelves_place(backend, -1).await;
    println!(
        "smoke test, places: home page {} → search {} → home page {}; home page {} → search → home page {}",
        place_text(Some(home_end)),
        place_text(search_top),
        place_text(home_again),
        place_text(home_inside),
        place_text(home_inside_again)
    );
    check(
        "a new tab's shelves start at the top",
        search_top.is_some_and(|p| p.at_top()),
    );
    check(
        "the home page is back at its end",
        home_again.is_some_and(|p| p.same_as(&home_end)),
    );
    check(
        "the home page is back inside a shelf",
        home_inside.is_some_and(|inside| {
            !inside.at_end && home_inside_again.is_some_and(|again| again.same_as(&inside))
        }),
    );
}

/// Shows the home page, has QML scroll it to the end and prints what the
/// cover cache did and the memory after it; gives back how far it scrolled.
async fn smoke_scroll_home(
    backend: &Arc<qtbridge::QThreadHandle<Backend>>,
    home: u64,
    covers: Option<&CoverCache>,
) -> Option<ShelvesPlace> {
    let report = |when: &str| {
        let stats = covers.map(CoverCache::stats).unwrap_or_default();
        println!(
            "smoke test, {when}: covers downloaded {} ({:.0} KB), from the cache {}, failed {}",
            stats.downloads,
            stats.downloaded_bytes as f64 / 1024.0,
            stats.hits,
            stats.failures
        );
    };
    backend.invoke_mut(move |b| b.show_tab(home as i64));
    tokio::time::sleep(Duration::from_secs(5)).await;
    report("first screen of the home page");
    smoke_screenshot(backend, "home").await;
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let (_, _, cpu_start) = usage();
    let started = std::time::Instant::now();
    backend.invoke_mut(move |b| b.request_scroll(sender));
    let counts = tokio::time::timeout(Duration::from_secs(120), receiver.recv()).await;
    let (_, _, cpu_end) = usage();
    let cpu = (cpu_end - cpu_start) / started.elapsed().as_secs_f64() * 100.0;
    match counts {
        Ok(Some((shelves, cards))) => println!(
            "smoke test: the home page scrolled to the end in {:.1} s: {shelves} shelves, {cards} cards; CPU {cpu:.1} % of a core",
            started.elapsed().as_secs_f64()
        ),
        _ => println!("smoke test: the home page did not reach its end in 120 s"),
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    let place = shelves_place(backend, -1).await;
    report("whole home page scrolled");
    let (rss, pss, _) = usage();
    println!("smoke test, after scrolling the home page: RSS {rss:.0} MB, PSS {pss:.0} MB");
    backend::release_memory();
    let (rss, pss, _) = usage();
    println!("  after malloc_trim: RSS {rss:.0} MB, PSS {pss:.0} MB");
    place
}

/// With `ORBIS_SMOKE_SCREENSHOT=<dir/name.png>`, saves the window as
/// `<dir/name>-<label>.png`.
async fn smoke_screenshot(backend: &Arc<qtbridge::QThreadHandle<Backend>>, label: &str) {
    let Ok(path) = std::env::var("ORBIS_SMOKE_SCREENSHOT") else {
        return;
    };
    let path = match path.strip_suffix(".png") {
        Some(stem) => format!("{stem}-{label}.png"),
        None => format!("{path}-{label}.png"),
    };
    backend.invoke_mut(move |b| b.request_screenshot(path));
    tokio::time::sleep(Duration::from_secs(1)).await;
}

/// Memory and CPU of this process: RSS and PSS in MB, CPU seconds.
fn usage() -> (f64, f64, f64) {
    let read = |path| std::fs::read_to_string(path).unwrap_or_default();
    let kb = |text: &str, key: &str| -> f64 {
        text.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0)
    };
    let stat = read("/proc/self/stat");
    let cpu = stat
        .rsplit_once(')')
        .map(|(_, rest)| {
            let f: Vec<f64> = rest
                .split_whitespace()
                .map(|x| x.parse().unwrap_or(0.0))
                .collect();
            (f.get(11).copied().unwrap_or(0.0) + f.get(12).copied().unwrap_or(0.0)) / 100.0
        })
        .unwrap_or(0.0);
    (
        kb(&read("/proc/self/status"), "VmRSS:") / 1024.0,
        kb(&read("/proc/self/smaps_rollup"), "Pss:") / 1024.0,
        cpu,
    )
}

/// PSS in MB by kind of mapping: heap, anonymous, GPU driver, Qt, mpv and
/// FFmpeg, other libraries.
fn pss_breakdown() -> Vec<(String, f64)> {
    let smaps = std::fs::read_to_string("/proc/self/smaps").unwrap_or_default();
    let mut groups: std::collections::BTreeMap<&'static str, f64> = Default::default();
    let mut current = "other";
    for line in smaps.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.first().is_some_and(|f| {
            f.contains('-') && f.chars().next().is_some_and(|c| c.is_ascii_hexdigit())
        }) {
            let path = fields.get(5).copied().unwrap_or("");
            current = match path {
                "" => "anonymous (malloc arenas, JS heap)",
                "[heap]" => "[heap]",
                p if p.contains("nvidia") || p.starts_with("/dev/dri") => "GPU driver",
                p if p.contains("libQt6") => "Qt libraries",
                p if p.contains("libmpv")
                    || p.contains("libav")
                    || p.contains("libsw")
                    || p.contains("libpostproc") =>
                {
                    "mpv and FFmpeg"
                }
                p if p.contains("/orbis") => "orbis binary",
                p if p.starts_with('/') => "other libraries and files",
                _ => "other",
            };
        } else if fields.first() == Some(&"Pss:") {
            let kb: f64 = fields.get(1).and_then(|v| v.parse().ok()).unwrap_or(0.0);
            *groups.entry(current).or_default() += kb / 1024.0;
        }
    }
    let mut out: Vec<(String, f64)> = groups.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out
}

/// Whether a child of this process named `name` runs (a zombie does not).
fn child_runs(name: &str) -> bool {
    let me = std::process::id().to_string();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    entries.flatten().any(|entry| {
        let stat = std::fs::read_to_string(entry.path().join("stat")).unwrap_or_default();
        let Some((head, rest)) = stat.rsplit_once(") ") else {
            return false;
        };
        let comm = head.split_once(" (").map(|(_, c)| c).unwrap_or_default();
        let fields: Vec<&str> = rest.split(' ').collect();
        comm == name && fields.first() != Some(&"Z") && fields.get(1) == Some(&me.as_str())
    })
}

/// The sign-in dialog's actions, for a start without a session: the bench
/// gives orbis a bus without a wallet, so nothing is saved and nobody signs
/// in. Opens the Google window and cancels it, then asks Chrome, whose key
/// is in the wallet, so that fails and says why. Checks each step and
/// quits.
pub fn run_sign_in(runtime: &tokio::runtime::Runtime, backend: Window, shell: ShellHandle) {
    runtime.spawn(async move {
        sleep(4).await;
        let mut passed = 0;
        let s = state(&backend).await;
        passed += usize::from(check("no session, the dialog asks", !s.signed_in));
        let (text, labels) = ask_mut(&shell, |s| s.tray_texts())
            .await
            .unwrap_or_default();
        println!("smoke test, tray without a session: {text:?}, menu {labels:?}");
        passed += usize::from(check(
            "the tray asks for a sign-in",
            text == crate::i18n::tr("YouTube Music needs a sign-in")
                && labels.first().map(String::as_str) == Some(crate::i18n::tr("Sign in…")),
        ));

        backend.invoke_mut(|b| b.sign_in_window());
        sleep(10).await;
        let s = state(&backend).await;
        passed += usize::from(check(
            "the Google window opens",
            s.signing_in && child_runs("orbis-login"),
        ));
        sleep(4).await;

        backend.invoke_mut(|b| b.cancel_sign_in());
        sleep(2).await;
        let s = state(&backend).await;
        passed += usize::from(check(
            "cancel closes it",
            !s.signing_in && s.sign_in_error.is_empty() && !child_runs("orbis-login"),
        ));

        backend.invoke_mut(|b| b.sign_in_browser("chrome".into()));
        sleep(6).await;
        let s = state(&backend).await;
        println!("smoke test, sign-in error: {}", s.sign_in_error);
        passed += usize::from(check(
            "a failed sign-in says why",
            !s.signing_in && !s.signed_in && !s.sign_in_error.is_empty(),
        ));
        println!("smoke test, sign-in: {passed} of 5 checks passed");
        sleep(4).await;
        backend.invoke_mut(|b| b.request_quit());
    });
}

/// Asks any of the window's objects on the GUI thread and waits.
async fn ask_mut<O: qtbridge::qtbridge_runtime::QObjectHolder + 'static, T: Send + 'static>(
    object: &Arc<qtbridge::QThreadHandle<O>>,
    question: impl FnOnce(&mut O) -> T + Send + 'static,
) -> Option<T> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    object.invoke_mut(move |o| {
        let _ = sender.send(question(o));
    });
    receiver.await.ok()
}

/// «Next» three times, each after the next track's link has been
/// resolved ahead: how long until its audio starts. Then clicks on tracks
/// further down the list: one cold, one after the pointer rested on it for
/// a second (its link fetched ahead), one whose link is ready. Then the
/// long-lived yt-dlp: its memory, that it stops when the window goes to the
/// tray, and the first link of one started anew. Last, the rows in view:
/// what the list asked for, what their links cost, and a click on one.
async fn smoke_next(backend: &Window) {
    let mut times = Vec::new();
    for _ in 0..3 {
        sleep(10).await;
        backend.invoke_mut(|b| b.next());
        sleep(4).await;
        if let Some(Some(took)) = ask(backend, |b| b.last_switch()).await {
            times.push(took.as_millis().to_string());
        }
    }
    println!(
        "smoke test, next track with its link resolved: sound after {} ms",
        times.join(", ")
    );

    // Rows far from the playing one, so nothing fetched them ahead.
    for (row, hover, what) in [
        (20, 0, "a cold click"),
        (24, 1, "a click after 1 s of hover"),
        (28, 8, "a click with the link ready"),
    ] {
        if hover > 0 {
            backend.invoke_mut(move |b| b.prefetch_row(row));
            sleep(hover).await;
        }
        backend.invoke_mut(move |b| b.play_track(row));
        sleep(9).await;
        report_switch(backend, what, row).await;
    }

    // The long-lived yt-dlp: what it holds, that it goes with the window,
    // and how soon one started anew gives its first link.
    let worker = ask(backend, |b| b.worker_pid()).await.flatten();
    match worker {
        Some(pid) => println!("smoke test, yt-dlp worker: PSS {:.0} MB", tree_pss_mb(pid)),
        None => println!("smoke test, yt-dlp worker: none"),
    }
    let processes = worker.map(process_tree).unwrap_or_default();
    backend.invoke_mut(|b| b.set_window_shown(false));
    sleep(2).await;
    let running = processes
        .iter()
        .filter(|pid| std::path::Path::new(&format!("/proc/{pid}")).exists())
        .count();
    let still = ask(backend, |b| b.worker_pid()).await.flatten();
    println!(
        "smoke test, yt-dlp worker with the window in the tray: {}, {running} of its {} processes left",
        if still.is_some() { "running" } else { "none" },
        processes.len()
    );
    // Row 12: above the playing row and below the rows in view, so that
    // nothing fetched its link ahead.
    backend.invoke_mut(|b| b.set_window_shown(true));
    backend.invoke_mut(|b| b.play_track(12));
    sleep(9).await;
    report_switch(backend, "the first link of a new worker", 12).await;

    // Rows in view: what the list asked for when it came to rest, then rows
    // 40–47 as if they were in view, what their links cost the worker, and
    // a click on one of them.
    let asked = ask(backend, |b| b.visible_asked()).await.flatten();
    println!(
        "smoke test, rows in view fetched ahead when the list rested: {}",
        asked
            .map(|(first, last)| format!("{first}–{last}"))
            .unwrap_or_else(|| "none".into())
    );
    let worker = ask(backend, |b| b.worker_pid()).await.flatten();
    let cpu_before = worker.map(tree_cpu_s);
    backend.invoke_mut(|b| b.prefetch_visible(40, 47));
    sleep(15).await;
    if let (Some(pid), Some(before)) = (worker, cpu_before) {
        let spent = tree_cpu_s(pid) - before;
        println!(
            "smoke test, CPU of the yt-dlp worker and Node.js for rows 40–47: {spent:.1} s, {:.2} s a link",
            spent / 8.0
        );
    }
    backend.invoke_mut(|b| b.play_track(44));
    sleep(9).await;
    report_switch(backend, "a click on a row in view after 15 s of rest", 44).await;
}

async fn report_switch(backend: &Window, what: &str, row: i32) {
    match ask(backend, |b| b.last_switch()).await {
        Some(Some(took)) => println!(
            "smoke test, {what} on row {row}: sound after {} ms",
            took.as_millis()
        ),
        _ => println!("smoke test, {what} on row {row}: no sound in 9 s"),
    }
}

/// `pid` and the processes it started, and theirs — from any of their
/// threads: orbis starts the yt-dlp worker from a tokio thread.
fn process_tree(pid: u32) -> Vec<u32> {
    let mut all = vec![pid];
    let mut next = 0;
    while next < all.len() {
        let parent = all[next];
        let tasks = std::fs::read_dir(format!("/proc/{parent}/task"))
            .map(|dir| dir.flatten().map(|e| e.path()).collect::<Vec<_>>())
            .unwrap_or_default();
        for task in tasks {
            let children = std::fs::read_to_string(task.join("children")).unwrap_or_default();
            all.extend(
                children
                    .split_whitespace()
                    .filter_map(|c| c.parse::<u32>().ok()),
            );
        }
        next += 1;
    }
    all
}

/// CPU seconds of `pid` and the processes under it, with the children
/// they have waited for (Node.js runs of the yt-dlp worker).
fn tree_cpu_s(pid: u32) -> f64 {
    process_tree(pid)
        .into_iter()
        .map(|pid| {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
            stat.rsplit_once(')')
                .map(|(_, rest)| {
                    let f: Vec<f64> = rest
                        .split_whitespace()
                        .map(|x| x.parse().unwrap_or(0.0))
                        .collect();
                    // utime, stime, cutime, cstime
                    (11..=14)
                        .map(|i| f.get(i).copied().unwrap_or(0.0))
                        .sum::<f64>()
                        / 100.0
                })
                .unwrap_or(0.0)
        })
        .sum()
}

/// PSS in MB of `pid` and the processes under it.
fn tree_pss_mb(pid: u32) -> f64 {
    process_tree(pid)
        .into_iter()
        .map(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
                .unwrap_or_default()
                .lines()
                .find(|l| l.starts_with("Pss:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0)
        })
        .sum::<f64>()
        / 1024.0
}

/// The settings the write run chooses: none of them the default.
const SMOKE_TEMPLATE: &str = "{artist}/{album} ({year})/{track}. {title}";

/// What the settings run checks: that the settings are those of the write
/// run, in the settings object, in playback and downloads, in the glass and
/// the tray, and (`in_file`) in `config.toml`.
async fn settings_applied(
    backend: &Window,
    glass: &GlassHandle,
    shell: &ShellHandle,
    preferences: &PreferencesHandle,
    folder: &std::path::Path,
    in_file: bool,
) -> usize {
    use orbis_core::settings::Quality;
    let (settings, _, problem) = ask_mut(preferences, |p| p.current())
        .await
        .unwrap_or_default();
    let s = state(backend).await;
    let (wanted, blur, opacity) = ask_glass(glass, |g| g.looks()).await.unwrap_or_default();
    let (tray, unload) = ask_mut(shell, |s| s.tray_settings())
        .await
        .unwrap_or_default();
    let mut passed = 0;
    passed += usize::from(check(
        "settings: economy quality, no loudness correction",
        settings.playback.quality == Quality::Economy
            && !settings.playback.normalize
            && s.quality == "economy"
            && !s.normalize,
    ));
    passed += usize::from(check(
        "settings: MP3 192, AAC 128, template, folder, no MusicBrainz",
        settings.downloads.format == orbis_core::Format::Mp3
            && settings.downloads.mp3_bitrate == 192
            && settings.downloads.m4a_bitrate == 128
            && settings.downloads.template == SMOKE_TEMPLATE
            && settings.downloads.folder.as_deref() == Some(folder)
            && !settings.downloads.musicbrainz
            && s.download_format == "mp3"
            && s.download_root == folder.to_string_lossy(),
    ));
    passed += usize::from(check(
        "settings: no glass, opacity 80 %",
        !settings.window.glass
            && settings.window.opacity == 80
            && !wanted
            && !blur
            && (opacity - 0.8).abs() < 1e-9,
    ));
    passed += usize::from(check(
        "settings: no tray icon, the window only hidden",
        !settings.window.tray && !settings.window.unload_in_tray && !tray && !unload,
    ));
    if in_file {
        let text = std::fs::read_to_string(orbis_core::Settings::path()).unwrap_or_default();
        passed += usize::from(check(
            "settings: config.toml has them",
            problem.is_empty()
                && orbis_core::Settings::load(&orbis_core::Settings::path()).ok()
                    == Some(settings.clone())
                && text.contains("quality = \"economy\""),
        ));
    }
    passed
}

/// The settings (`ORBIS_SMOKE_SETTINGS`): `write` opens the dialog, shows
/// each tab for the bench's pictures, changes every setting away from its
/// default, refuses a broken template, checks that all of it applies and is
/// in `config.toml`; `read`, the next run, checks that it all came back.
pub fn run_settings(
    runtime: &tokio::runtime::Runtime,
    backend: Window,
    glass: GlassHandle,
    shell: ShellHandle,
    preferences: PreferencesHandle,
) {
    let mode = std::env::var("ORBIS_SMOKE_SETTINGS").unwrap_or_default();
    runtime.spawn(async move {
        let Some(dir) = std::env::var_os("ORBIS_CONFIG_DIR").map(std::path::PathBuf::from) else {
            println!("smoke test, settings: ORBIS_CONFIG_DIR is not set; nothing is changed");
            backend.invoke_mut(|b| b.request_quit());
            return;
        };
        let folder = dir.join("music");
        sleep(8).await;
        let mut passed = 0;
        let checks = if mode == "write" {
            shell.invoke_mut(|s| s.open_settings(None));
            for tab in 0..4 {
                preferences.invoke_mut(move |p| p.show_tab(tab));
                sleep(5).await;
            }
            preferences.invoke_mut(|p| p.show_tab(1));
            let broken = ask_mut(&preferences, |p| {
                p.set_template("{oops}/{title}".into());
                p.current()
            })
            .await
            .unwrap_or_default();
            passed += usize::from(check(
                "settings: a broken template is refused",
                !broken.1.is_empty()
                    && broken.0.downloads.template == orbis_core::download::DEFAULT_TEMPLATE,
            ));
            let chosen = folder.to_string_lossy().into_owned();
            preferences.invoke_mut(move |p| {
                p.set_quality("economy".into());
                p.set_normalize(false);
                p.set_download_format("mp3".into());
                p.set_mp3_bitrate(192);
                p.set_m4a_bitrate(128);
                p.set_download_folder(chosen);
                p.set_template(SMOKE_TEMPLATE.into());
                p.set_musicbrainz(false);
                p.set_glass(false);
                p.set_opacity(80);
                p.set_tray(false);
                p.set_unload_in_tray(false);
            });
            sleep(5).await;
            passed += settings_applied(&backend, &glass, &shell, &preferences, &folder, true).await;
            6
        } else {
            passed +=
                settings_applied(&backend, &glass, &shell, &preferences, &folder, false).await;
            4
        };
        println!("smoke test, settings ({mode}): {passed} of {checks} checks passed");
        sleep(2).await;
        backend.invoke_mut(|b| b.request_quit());
    });
}

/// Audio files and hidden files under `dir`, recursively.
fn music_files(dir: &std::path::Path) -> (usize, usize) {
    let (mut audio, mut hidden) = (0, 0);
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            let (a, h) = music_files(&path);
            audio += a;
            hidden += h;
        } else if entry.file_name().to_string_lossy().starts_with('.') {
            hidden += 1;
        } else {
            audio += 1;
        }
    }
    (audio, hidden)
}

/// Downloads, only into `ORBIS_MUSIC_DIR` and never into the owner's music
/// folder: the menu's «Download» on the playing track, the list for the
/// bench's pictures, then «Download all» on its album (or another track's)
/// and «Cancel all», and «Download all» on Liked Music, which asks first.
async fn smoke_downloads(backend: &Window) {
    let Some(dir) = std::env::var_os("ORBIS_MUSIC_DIR").map(std::path::PathBuf::from) else {
        return;
    };
    backend.invoke_mut(|b| b.menu_for_current());
    sleep(3).await;
    backend.invoke_mut(|b| b.menu_download());
    let mut first = String::new();
    for _ in 0..90 {
        sleep(1).await;
        first = state(backend)
            .await
            .downloads
            .first()
            .cloned()
            .unwrap_or_default();
        if !first.is_empty() && first != "queued" && first != "active" {
            break;
        }
    }
    let (audio, _) = music_files(&dir);
    check("download «Download»", first == "done" && audio == 1);

    backend.invoke_mut(|b| b.request_downloads_list(true));
    sleep(21).await;
    backend.invoke_mut(|b| b.request_downloads_list(false));

    // An album: the playing track's, which the menu still points at, else
    // the first of the next rows that has one (a video has none).
    let mut row = 0;
    while state(backend).await.menu_album.is_empty() && row < 6 {
        row += 1;
        backend.invoke_mut(move |b| b.menu_for_row(row));
        sleep(3).await;
    }
    backend.invoke_mut(|b| b.menu_open_album());
    sleep(4).await;
    let album = state(backend).await;
    println!(
        "smoke test, album for «Download all»: «{}», shown {:?}, downloadable {}",
        album.menu_album, album.shown_kind, album.shown_downloadable
    );
    backend.invoke_mut(|b| b.download_shown());
    let mut queued = 0;
    let mut asked = 0;
    for _ in 0..15 {
        sleep(1).await;
        let s = state(backend).await;
        queued = s.downloads.len().saturating_sub(1);
        asked = s.download_ask;
        if queued > 0 || asked > 0 {
            break;
        }
    }
    println!("smoke test, status: {:?}", state(backend).await.status);
    if std::env::var_os("ORBIS_SMOKE_KEEP_DOWNLOADS").is_some() {
        // Left for the next run, once one of the album's tracks is done.
        for _ in 0..90 {
            sleep(1).await;
            if state(backend)
                .await
                .downloads
                .iter()
                .skip(1)
                .any(|s| s == "done")
            {
                break;
            }
        }
        let stages = state(backend).await.downloads;
        let left = stages
            .iter()
            .filter(|s| *s == "queued" || *s == "active")
            .count();
        println!("smoke test, downloads left for the restart: {left} (stages {stages:?})");
        return;
    }
    backend.invoke_mut(|b| b.cancel_downloads());
    sleep(3).await;
    let after = state(backend).await;
    let (_, hidden) = music_files(&dir);
    println!(
        "smoke test, «Download all»: {queued} tracks queued; stages after «Cancel all»: {:?}",
        after.downloads
    );
    check(
        "download «Download all» and «Cancel all»",
        album.shown_downloadable
            && queued > 0
            && asked == 0
            && after.downloads_active == 0
            && after
                .downloads
                .iter()
                .all(|s| s != "queued" && s != "active")
            && hidden == 0,
    );

    // A long list: Liked Music asks first, and «Cancel» queues nothing.
    let before = state(backend).await.downloads.len();
    backend.invoke_mut(|b| b.open_playlist(ytm::LIKED_MUSIC.into(), "Liked Music".into(), true));
    sleep(6).await;
    let reading = std::time::Instant::now();
    backend.invoke_mut(|b| b.download_shown());
    let mut long = state(backend).await;
    for _ in 0..120 {
        sleep(1).await;
        long = state(backend).await;
        if !long.download_reading {
            break;
        }
    }
    let read_in = reading.elapsed().as_secs();
    // The question stays for the bench's pictures (the window's own
    // picture leaves dialogs out).
    sleep(21).await;
    backend.invoke_mut(|b| b.answer_download(false));
    sleep(2).await;
    if long.download_ask == 0 {
        // Not asked: whatever was queued is not for a smoke test.
        backend.invoke_mut(|b| b.cancel_downloads());
        sleep(2).await;
    }
    let answered = state(backend).await;
    println!(
        "smoke test, «Download all» on Liked Music: read in {read_in} s, asked about {} tracks; downloads before {before}, after «Cancel» {}",
        long.download_ask,
        answered.downloads.len()
    );
    check(
        "download asks about a long list, «Cancel» queues nothing",
        long.download_ask > backend::ASK_ABOVE as i32
            && answered.download_ask == 0
            && answered.downloads.len() == before,
    );
}

/// Typing «metal» into the search field brings YouTube Music's suggestions
/// under it, and choosing one searches it; its tab is closed again.
async fn smoke_suggestions(backend: &Window) {
    backend.invoke_mut(|b| b.request_typing("metal".into()));
    sleep(3).await;
    let typed = state(backend).await;
    println!(
        "smoke test, suggestions for «metal»: {:?}",
        typed.suggestions
    );
    // The list stays for the bench's pictures (the window's own picture
    // leaves popups out).
    sleep(18).await;
    backend.invoke_mut(|b| b.request_suggestion(0));
    sleep(5).await;
    let chosen = state(backend).await;
    let wanted = typed.suggestions.first().cloned().unwrap_or_default();
    let tab = ask(backend, |b| b.tab_ids())
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|(_, source)| matches!(source, TabSource::Search { query } if *query == wanted));
    check(
        "suggestions show, and one chosen opens its search",
        !typed.suggestions.is_empty()
            && chosen.suggestions.is_empty()
            && chosen.shown_kind == "search"
            && tab.is_some(),
    );
    if let Some((id, _)) = tab {
        backend.invoke_mut(move |b| b.close_tab(id as i64));
        sleep(2).await;
    }
}

/// The shown search narrowed to songs goes on page after page, past the
/// some 30 results of a search for everything; then everything again.
async fn smoke_search_filter(backend: &Window) {
    let everything = state(backend).await.shelf_cards;
    backend.invoke_mut(|b| b.set_search_filter("songs".into()));
    sleep(5).await;
    let first = state(backend).await.shelf_cards;
    for _ in 0..2 {
        backend.invoke_mut(|b| b.load_more());
        sleep(3).await;
    }
    let songs = state(backend).await;
    smoke_screenshot(backend, "search-songs").await;
    println!(
        "smoke test, search: {everything} results for everything; songs only {first}, {} after two more pages (filter {:?})",
        songs.shelf_cards, songs.search_filter
    );
    backend.invoke_mut(|b| b.set_search_filter(String::new()));
    sleep(5).await;
    let back = state(backend).await;
    check(
        "a search for songs goes on past 30 results",
        songs.search_filter == "songs"
            && songs.shelf_cards > 30
            && songs.shelf_cards > first
            && back.search_filter.is_empty()
            && back.shelf_cards > 0,
    );
}

/// Shows each of `tabs` in turn, `rounds` times, and prints how many frames
/// the window drew in the 3 s after each was shown: a page that came to rest
/// draws only while its pictures arrive.
async fn smoke_tab_frames(
    backend: &Window,
    glass: &GlassHandle,
    tabs: &[(u64, TabSource)],
    rounds: usize,
) {
    for round in 1..=rounds {
        let mut counts = Vec::new();
        for (id, source) in tabs {
            let id = *id;
            ask_glass(glass, |g| g.start_counting()).await;
            backend.invoke_mut(move |b| b.show_tab(id as i64));
            sleep(3).await;
            let frames = ask_glass(glass, |g| g.stop_counting())
                .await
                .unwrap_or_default();
            let kind = match source {
                TabSource::Home => "home",
                TabSource::Library => "library",
                TabSource::Playlist { .. } => "playlist",
                TabSource::Album { .. } => "album",
                TabSource::Artist { .. } => "artist",
                TabSource::Search { .. } => "search",
                TabSource::Radio { .. } => "radio",
            };
            counts.push(format!("{kind} {frames}"));
        }
        println!(
            "smoke test, frames in the 3 s after a tab is shown, round {round}: {}",
            counts.join(", ")
        );
    }
}

/// Signing out with «Close the tabs and queues too»: with a mix playing and
/// a search open, the account's tabs close, Home and Library stay, nothing
/// plays. The check runs `close_account_tabs`, the part of signing out the
/// switch adds: a real sign-out would remove the owner's saved sign-in.
async fn smoke_close_tabs(backend: &Window, mix: u64) {
    backend.invoke_mut(move |b| b.show_tab(mix as i64));
    sleep(6).await;
    backend.invoke_mut(|b| b.play_track(0));
    sleep(8).await;
    backend.invoke_mut(|b| b.search("Metallica".into()));
    sleep(5).await;
    let before = ask(backend, |b| b.tab_ids()).await.unwrap_or_default();
    let playing = state(backend).await.playing_kind;
    backend.invoke_mut(|b| b.close_account_tabs());
    sleep(4).await;
    let after = ask(backend, |b| b.tab_ids()).await.unwrap_or_default();
    let s = state(backend).await;
    let kinds: Vec<&str> = after
        .iter()
        .map(|(_, source)| match source {
            TabSource::Home => "home",
            TabSource::Library => "library",
            _ => "other",
        })
        .collect();
    println!(
        "smoke test, closing the account's tabs: {} tabs, {playing:?} playing → {kinds:?}, playing {:?}, shown {:?}",
        before.len(),
        s.playing_kind,
        s.shown_kind
    );
    check(
        "closing the account's tabs leaves Home and Library, nothing playing",
        before.len() >= 4
            && !playing.is_empty()
            && kinds == ["home", "library"]
            && s.playing_kind.is_empty()
            && s.shown_kind == "home",
    );
}

/// A pointer moving slowly down a long list (Liked Music): every row
/// stays under it for 300 ms, longer than the list waits before it fetches
/// the row's link (250 ms) — the worst case. How much CPU orbis, the yt-dlp
/// worker and Node.js spend on it in 30 s, and how many links come.
async fn smoke_hover(backend: &Window) {
    backend.invoke_mut(|b| b.open_playlist(ytm::LIKED_MUSIC.into(), "Liked Music".into(), true));
    // The list loads, and its rows in view are fetched ahead.
    sleep(20).await;
    let me = std::process::id();
    let idle_from = tree_cpu_s(me);
    sleep(10).await;
    let idle = tree_cpu_s(me) - idle_from;
    let first = 10;
    let before = ask(backend, move |b| b.links_ready(first, first + 99))
        .await
        .unwrap_or(0);
    let from = tree_cpu_s(me);
    let started = std::time::Instant::now();
    let mut row = first;
    while started.elapsed() < Duration::from_secs(30) {
        backend.invoke_mut(move |b| b.prefetch_row(row));
        row += 1;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let spent = tree_cpu_s(me) - from;
    // The links asked last come in after the pointer has passed.
    sleep(5).await;
    let spent_after = tree_cpu_s(me) - from;
    let after = ask(backend, move |b| b.links_ready(first, first + 99))
        .await
        .unwrap_or(0);
    println!(
        "smoke test, hovering rows {first}–{} for 30 s: CPU {spent:.1} s ({spent_after:.1} s with the last links, {idle:.2} s over 10 s idle before), links at hand {before} → {after}",
        row - 1
    );
}

/// The downloads a run with `ORBIS_SMOKE_KEEP_DOWNLOADS` left are in the
/// list after the restart, `left` of them; then they are cancelled, and a
/// run after that finds none.
async fn smoke_downloads_back(backend: &Window, left: usize) {
    let back = state(backend).await.downloads;
    println!(
        "smoke test, downloads back after the restart: {} (stages {back:?})",
        back.len()
    );
    backend.invoke_mut(|b| b.cancel_downloads());
    sleep(3).await;
    let after = state(backend).await;
    check(
        "the download queue comes back after a restart",
        back.len() == left && after.downloads_active == 0,
    );
}

/// Runs the scripted session: scroll the whole home page, search, show Discover Mix,
/// play its first track, open more tabs from the library, then report and
/// quit.
pub fn run(
    runtime: &tokio::runtime::Runtime,
    backend: Window,
    glass: GlassHandle,
    shell: ShellHandle,
    covers: Option<Arc<CoverCache>>,
    session: Option<Arc<SessionManager>>,
) {
    runtime.spawn(async move {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<Vec<(u64, TabSource)>>();
        let ask_tabs = |backend: &Arc<qtbridge::QThreadHandle<Backend>>, sender: tokio::sync::mpsc::UnboundedSender<_>| {
            backend.invoke(move |b| {
                let _ = sender.send(b.tab_ids());
            });
        };

        sleep(3).await;
        if let Some(Some((main, exec))) = ask_mut(&shell, |s| s.first_frame()).await {
            println!(
                "smoke test, start: first frame {} ms after main, {} after exec",
                main.as_millis(),
                exec.map_or("?".to_owned(), |d| format!("{} ms", d.as_millis()))
            );
        }
        // On a first start the mixes are found after the window is up.
        let mut mix = None;
        let mut tabs = Vec::new();
        for waited in 0..20 {
            ask_tabs(&backend, sender.clone());
            tabs = receiver.recv().await.unwrap_or_default();
            mix = tabs.iter().find(|(_, s)| matches!(s, TabSource::Playlist { .. })).map(|(id, _)| *id);
            if mix.is_some() {
                println!("smoke test, start: a mix tab {waited} s after the first look (3 s after the start)");
                break;
            }
            sleep(1).await;
        }
        if std::env::var_os("ORBIS_SMOKE_CLOSE_TABS").is_some() {
            if let Some(mix) = mix {
                smoke_close_tabs(&backend, mix).await;
            }
            backend.invoke_mut(|b| b.request_quit());
            return;
        }
        if std::env::var_os("ORBIS_SMOKE_HOVER").is_some() {
            smoke_hover(&backend).await;
            backend.invoke_mut(|b| b.request_quit());
            return;
        }
        if std::env::var_os("ORBIS_SMOKE_TABS").is_some() {
            smoke_tab_frames(&backend, &glass, &tabs, 3).await;
            sleep(5).await;
            let (rss, pss, _) = usage();
            println!(
                "smoke test, after the tabs with {} open: RSS {rss:.0} MB, PSS {pss:.0} MB",
                tabs.len()
            );
            backend.invoke_mut(|b| b.request_quit());
            return;
        }
        if let Some(left) = std::env::var("ORBIS_SMOKE_DOWNLOADS_BACK")
            .ok()
            .and_then(|v| v.parse().ok())
        {
            smoke_downloads_back(&backend, left).await;
            backend.invoke_mut(|b| b.request_quit());
            return;
        }
        if std::env::var_os("ORBIS_SMOKE_DOWNLOADS").is_some() {
            if let Some(mix) = mix {
                backend.invoke_mut(move |b| b.show_tab(mix as i64));
                sleep(6).await;
                backend.invoke_mut(|b| b.play_track(0));
                sleep(10).await;
                smoke_downloads(&backend).await;
            }
            backend.invoke_mut(|b| b.request_quit());
            return;
        }
        if std::env::var_os("ORBIS_SMOKE_NEXT").is_some() {
            if let Some(mix) = mix {
                backend.invoke_mut(move |b| b.show_tab(mix as i64));
                sleep(6).await;
                backend.invoke_mut(|b| b.play_track(0));
                sleep(8).await;
                // Its link may be one an earlier run left on disk.
                report_switch(&backend, "the first click after the start", 0).await;
                smoke_next(&backend).await;
            }
            backend.invoke_mut(|b| b.request_quit());
            return;
        }
        if std::env::var_os("ORBIS_SMOKE_WRITE").is_some() {
            if let (Some(session), Some(mix)) = (session, mix) {
                let (passed, of) = smoke_write(&backend, &session, mix).await;
                println!("smoke test, write: {passed} of {of} checks passed");
            }
            backend.invoke_mut(|b| b.request_quit());
            return;
        }
        let home = tabs.iter().find(|(_, s)| *s == TabSource::Home).map(|(id, _)| *id);
        let mut home_place = None;
        if let Some(home) = home {
            home_place = smoke_scroll_home(&backend, home, covers.as_deref()).await;
        }
        smoke_suggestions(&backend).await;
        backend.invoke_mut(|b| b.search("Metallica".into()));
        sleep(5).await;
        smoke_screenshot(&backend, "search").await;
        smoke_search_filter(&backend).await;
        if let Some(home) = home {
            smoke_places(&backend, home, home_place).await;
        }
        let library = tabs.iter().find(|(_, s)| *s == TabSource::Library).map(|(id, _)| *id);
        if let Some(mix) = mix {
            backend.invoke_mut(move |b| b.show_tab(mix as i64));
            sleep(6).await;
            backend.invoke_mut(|b| b.play_track(0));
            sleep(12).await;
            // The resources' target "playing, window shown, 5 tabs": the
            // home page, the two mixes, the library and the search.
            ask_tabs(&backend, sender.clone());
            let open = receiver.recv().await.unwrap_or_default().len();
            let (_, _, cpu_start) = usage();
            sleep(10).await;
            let (rss, pss, cpu_end) = usage();
            println!(
                "smoke test: playing with {open} tabs open: RSS {rss:.0} MB, PSS {pss:.0} MB, CPU {:.2} % of a core over 10 s",
                (cpu_end - cpu_start) / 10.0 * 100.0
            );
            smoke_windows(&backend).await;
            smoke_menu(&backend).await;
            smoke_next(&backend).await;
            if std::env::var_os("ORBIS_MUSIC_DIR").is_some() {
                smoke_downloads(&backend).await;
            }
        }
        if let Some(library) = library {
            backend.invoke_mut(move |b| b.show_tab(library as i64));
            sleep(5).await;
            smoke_screenshot(&backend, "library").await;
            backend.invoke_mut(|b| b.smoke_open_library_cards(3));
            sleep(2).await;
        }
        ask_tabs(&backend, sender.clone());
        let tabs = receiver.recv().await.unwrap_or_default();
        smoke_tab_frames(&backend, &glass, &tabs, 2).await;
        if let Ok(path) = std::env::var("ORBIS_SMOKE_SCREENSHOT") {
            if let Some(mix) = mix {
                backend.invoke_mut(move |b| b.show_tab(mix as i64));
                sleep(3).await;
            }
            backend.invoke_mut(move |b| b.request_screenshot(path));
            sleep(2).await;
        }
        let (_, _, cpu_start) = usage();
        ask_glass(&glass, |g| g.start_counting()).await;
        let quiet = 20;
        sleep(quiet).await;
        let frames = ask_glass(&glass, |g| g.stop_counting()).await.unwrap_or_default();
        let (rss, pss, cpu_end) = usage();
        println!(
            "smoke test: {} tabs open; RSS {rss:.0} MB, PSS {pss:.0} MB; over {quiet} s of playback with the window shown: CPU {:.2} % of a core, {frames} frames",
            tabs.len(),
            (cpu_end - cpu_start) / quiet as f64 * 100.0
        );
        // Paused, nothing on screen changes: the window should not draw.
        backend.invoke_mut(|b| b.toggle_pause());
        sleep(2).await;
        let (_, _, cpu_start) = usage();
        ask_glass(&glass, |g| g.start_counting()).await;
        let still = 10;
        sleep(still).await;
        let frames = ask_glass(&glass, |g| g.stop_counting()).await.unwrap_or_default();
        let (_, _, cpu_end) = usage();
        println!(
            "smoke test, paused: over {still} s CPU {:.2} % of a core, {frames} frames",
            (cpu_end - cpu_start) / still as f64 * 100.0
        );
        for (kind, mb) in pss_breakdown() {
            println!("  PSS {mb:7.1} MB  {kind}");
        }
        // SAFETY: plain libc call; returns free malloc memory to the system.
        unsafe { libc::malloc_trim(0) };
        let (rss, pss, _) = usage();
        println!("after malloc_trim: RSS {rss:.0} MB, PSS {pss:.0} MB");

        // In the tray, playing: the window hidden and, by default, unloaded;
        // Main.qml collects the JS heap and trims malloc a second after.
        backend.invoke_mut(|b| b.toggle_pause());
        shell.invoke_mut(|s| s.hide_window());
        sleep(4).await;
        let (rss, pss, cpu_start) = usage();
        let worker = ask(&backend, |b| b.worker_pid()).await.flatten();
        println!(
            "smoke test, in the tray: RSS {rss:.0} MB, PSS {pss:.0} MB; yt-dlp worker: {}",
            if worker.is_some() { "running" } else { "none" }
        );
        let tray = 20;
        sleep(tray).await;
        let (rss, pss, cpu_end) = usage();
        println!(
            "smoke test, {tray} s of playback in the tray: CPU {:.2} % of a core; RSS {rss:.0} MB, PSS {pss:.0} MB",
            (cpu_end - cpu_start) / tray as f64 * 100.0
        );
        for (kind, mb) in pss_breakdown() {
            println!("  PSS {mb:7.1} MB  {kind}");
        }
        // Back from the tray: the window is built again; how long until its
        // first frame.
        ask_glass(&glass, |g| g.start_counting()).await;
        let started = std::time::Instant::now();
        shell.invoke_mut(|s| s.show_window());
        let mut first_frame = None;
        while started.elapsed() < Duration::from_secs(5) {
            if ask_glass(&glass, |g| g.frames()).await.unwrap_or_default() > 0 {
                first_frame = Some(started.elapsed());
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        ask_glass(&glass, |g| g.stop_counting()).await;
        sleep(5).await;
        let (rss, pss, _) = usage();
        println!(
            "smoke test, the window back from the tray: first frame after {}; RSS {rss:.0} MB, PSS {pss:.0} MB",
            first_frame.map_or("more than 5 s".to_owned(), |t| format!("{} ms", t.as_millis()))
        );
        smoke_screenshot(&backend, "back").await;
        backend.invoke_mut(|b| b.request_quit());
    });
}
