//! The window beside the desktop: shown, hidden in the tray, or unloaded
//! there (the QML scene destroyed, only the core plays); the tray icon and
//! MPRIS (media keys, Plasma's media player widget, KDE Connect), which show
//! the current track and drive playback; and the single instance — a second
//! start of orbis shows this one's window.
//!
//! The window's life is this object's, not the backend's: QML creates and
//! destroys the window from `shell`'s properties, so a window being built
//! never finds the backend in the middle of a slot.
//!
//! Environment: `ORBIS_TRAY=0` puts no icon in the tray (closing the window
//! then quits); `ORBIS_TRAY_UNLOAD=0` only hides the window in the tray.

use std::time::Duration;

use qtbridge::qobject;

use crate::i18n::tr;

/// The tray menu's actions.
const PLAY_PAUSE: u32 = 1;
const NEXT: u32 = 2;
const PREVIOUS: u32 = 3;
const LIKE: u32 = 4;
const WINDOW: u32 = 5;
const QUIT: u32 = 6;
const SETTINGS: u32 = 7;
const SIGN_IN: u32 = 8;
/// One notch of the wheel over the icon turns the volume by this, percent.
const VOLUME_STEP: f64 = 5.0;
/// The cover MPRIS shows, in pixels: Plasma's media widget and KDE Connect
/// show it large.
const ART_PX: u32 = 480;

/// The current track and playback, as the backend tells them.
#[derive(Debug, Clone, Default, PartialEq)]
struct Track {
    video_id: String,
    title: String,
    artists: String,
    /// The cover's URL from the API.
    thumbnail: String,
    /// Seconds; 0 when unknown.
    duration: f64,
    playing: bool,
    liked: bool,
    /// 0 to 100.
    volume: f64,
}

fn mpris_state(track: &Track, art_url: Option<String>) -> desktop::MprisState {
    let current = (!track.video_id.is_empty()).then(|| desktop::MprisTrack {
        id: track.video_id.clone(),
        title: track.title.clone(),
        artists: if track.artists.is_empty() {
            Vec::new()
        } else {
            vec![track.artists.clone()]
        },
        album: None,
        length: (track.duration > 0.0).then(|| Duration::from_secs_f64(track.duration)),
        art_url,
        url: Some(format!(
            "https://music.youtube.com/watch?v={}",
            track.video_id
        )),
    });
    desktop::MprisState {
        track: current,
        playing: track.playing,
        volume: (track.volume / 100.0).clamp(0.0, 1.0),
    }
}

/// Sizes of the tray icon's pictures: the tray takes the nearest to its own
/// at the screen's scale (22 px at 1.5 is 33).
const TRAY_SIZES: [u32; 7] = [16, 22, 24, 32, 48, 64, 96];

/// The orbis icon for the tray, made once: an AppImage's icon is not in the
/// icon theme, so the tray gets the pictures themselves, as the taskbar
/// shows them.
fn tray_pixmaps() -> std::sync::Arc<desktop::Pixmaps> {
    static PIXMAPS: std::sync::OnceLock<std::sync::Arc<desktop::Pixmaps>> =
        std::sync::OnceLock::new();
    PIXMAPS
        .get_or_init(|| {
            let icon = image::load_from_memory_with_format(
                crate::window_system::ICON_PNG,
                image::ImageFormat::Png,
            );
            let Ok(icon) = icon else {
                return Default::default();
            };
            let pixmaps = TRAY_SIZES
                .iter()
                .map(|&size| {
                    let picture = icon
                        .resize_exact(size, size, image::imageops::FilterType::Lanczos3)
                        .to_rgba8();
                    // RGBA to ARGB, byte by byte: network byte order.
                    let argb = picture
                        .pixels()
                        .flat_map(|p| [p[3], p[0], p[1], p[2]])
                        .collect();
                    (size as i32, size as i32, argb)
                })
                .collect();
            std::sync::Arc::new(pixmaps)
        })
        .clone()
}

/// The tray icon's tooltip and menu. Without an accepted session the
/// tooltip says a sign-in is needed and the menu offers it first: with the
/// window in the tray, nothing else would tell.
fn tray_state(track: &Track, window_visible: bool, signed_in: bool) -> desktop::TrayState {
    use desktop::MenuEntry::{Item, Separator};
    let has_track = !track.title.is_empty();
    let item = |action, label: &str, enabled, checked| Item {
        action,
        label: label.to_owned(),
        enabled,
        checked,
    };
    let sign_in = if signed_in {
        Vec::new()
    } else {
        vec![item(SIGN_IN, tr("Sign in…"), true, None), Separator]
    };
    desktop::TrayState {
        icon: String::new(),
        pixmaps: tray_pixmaps(),
        title: if has_track {
            track.title.clone()
        } else {
            "orbis".to_owned()
        },
        text: if !signed_in {
            tr("YouTube Music needs a sign-in").to_owned()
        } else if has_track {
            track.artists.clone()
        } else {
            tr("Nothing plays").to_owned()
        },
        menu: sign_in
            .into_iter()
            .chain([
                item(
                    PLAY_PAUSE,
                    if track.playing {
                        tr("Pause")
                    } else {
                        tr("Play")
                    },
                    has_track,
                    None,
                ),
                item(NEXT, tr("Next"), has_track, None),
                item(PREVIOUS, tr("Previous"), has_track, None),
                Separator,
                item(LIKE, tr("Like"), has_track, Some(track.liked)),
                Separator,
                item(
                    WINDOW,
                    if window_visible {
                        tr("Hide window")
                    } else {
                        tr("Show window")
                    },
                    true,
                    None,
                ),
                item(SETTINGS, tr("Settings…"), true, None),
                Separator,
                item(QUIT, tr("Quit"), true, None),
            ])
            .collect(),
    }
}

#[qobject(NoQmlElement, ConvertToCamelCase)]
mod shell {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::Duration;

    use desktop::{Connection, Mpris, MprisEvent, MprisState, Tray, TrayEvent, TrayState};
    use orbis_core::CoverCache;
    use orbis_core::covers::file_url;
    use qtbridge::{QThreadHandle, QmlObject};
    use tokio::runtime::Handle;
    use tokio::sync::mpsc::UnboundedReceiver;
    use tokio::sync::watch;

    use super::{
        ART_PX, LIKE, NEXT, PLAY_PAUSE, PREVIOUS, QUIT, SETTINGS, SIGN_IN, Track, VOLUME_STEP,
        WINDOW, mpris_state, tray_state,
    };
    use crate::backend::{Backend, release_memory};
    use crate::window_system;

    pub struct Shell {
        /// The window is wanted on screen.
        window_visible: bool,
        /// Hidden in the tray, the window is destroyed, not only hidden.
        unload_hidden: bool,
        /// A tray shows the icon: closing the window hides it there.
        tray_shown: bool,
        /// The tray's menu asked for the settings; the window opens them
        /// (once it exists) and says so.
        settings_wanted: bool,
        /// Takes the icon out of the tray.
        tray_stop: Option<tokio::sync::oneshot::Sender<()>>,
        /// From `main` and from the process's start to the first frame.
        first_frame: Option<(Duration, Option<Duration>)>,

        backend: Arc<QThreadHandle<Backend>>,
        runtime: Handle,
        me: Option<Arc<QThreadHandle<Shell>>>,
        track: Track,
        /// What the tray should show; one task applies the latest.
        tray_state: Option<watch::Sender<TrayState>>,
        /// The window has the focus (QML reports it).
        window_active: bool,
        /// The backend has a session YouTube Music accepts (QML reports it).
        signed_in: bool,

        /// The bus of the tray and MPRIS, when there is one.
        connection: Option<Connection>,
        mpris: Option<Arc<Mpris>>,
        covers: Option<Arc<CoverCache>>,
        /// The `file://` URL of the current track's cover for MPRIS.
        art: Option<String>,
        /// What MPRIS should show; one task applies the latest.
        media_state: Option<watch::Sender<MprisState>>,
    }

    impl Shell {
        qproperty!(
            "windowVisible",
            Member = window_visible,
            Notify = window_changed
        );
        qproperty!(
            "unloadHidden",
            Member = unload_hidden,
            Notify = window_changed
        );
        qproperty!("trayShown", Member = tray_shown, Notify = tray_changed);
        qproperty!(
            "settingsWanted",
            Member = settings_wanted,
            Notify = settings_changed
        );

        #[qsignal]
        fn window_changed(&mut self);
        #[qsignal]
        fn tray_changed(&mut self);
        #[qsignal]
        fn settings_changed(&mut self);
        /// No file manager showed a downloaded file: QML opens its folder
        /// (`file://` URL) the system's way.
        #[qsignal]
        fn folder_wanted(&mut self, folder: String);

        /// `connection` is the bus of the tray and MPRIS, `mpris` the player
        /// already claimed on it; `covers` gives MPRIS its cover.
        pub fn create(
            backend: Arc<QThreadHandle<Backend>>,
            runtime: Handle,
            unload_hidden: bool,
            connection: Option<Connection>,
            mpris: Option<Arc<Mpris>>,
            covers: Option<Arc<CoverCache>>,
        ) -> Rc<RefCell<Self>> {
            let shell = Rc::new(RefCell::new(Self {
                window_visible: true,
                unload_hidden,
                tray_shown: false,
                settings_wanted: false,
                tray_stop: None,
                first_frame: None,
                backend,
                runtime,
                me: None,
                track: Track::default(),
                tray_state: None,
                window_active: false,
                signed_in: true,
                connection,
                mpris,
                covers,
                art: None,
                media_state: None,
            }));
            Self::attach_qobject(&shell);
            let handle = Arc::new(shell.borrow().get_thread_handle());
            shell.borrow_mut().me = Some(handle);
            shell
        }

        /// Puts the icon into the tray, in the background; without a tray
        /// the window just stays, and closing it quits.
        pub fn start_tray(&mut self) {
            let (Some(me), Some(connection)) = (self.me.clone(), self.connection.clone()) else {
                return;
            };
            if self.tray_state.is_some() {
                return;
            }
            let (state_sender, mut wanted) =
                watch::channel(tray_state(&self.track, self.window_visible, self.signed_in));
            self.tray_state = Some(state_sender);
            let (stop_sender, mut stop) = tokio::sync::oneshot::channel();
            self.tray_stop = Some(stop_sender);
            self.runtime.spawn(async move {
                let (events_sender, mut events) = tokio::sync::mpsc::unbounded_channel();
                let initial = wanted.borrow_and_update().clone();
                let tray = match Tray::start(&connection, "orbis", initial, events_sender).await {
                    Ok(tray) => tray,
                    Err(e) => {
                        eprintln!("orbis: no tray icon: {e}");
                        return;
                    }
                };
                let mut shown = tray.shown();
                let follow_me = Arc::clone(&me);
                let report = move |shown: bool| {
                    follow_me.invoke_mut(move |shell| shell.set_tray_shown(shown));
                };
                report(*shown.borrow_and_update());
                loop {
                    tokio::select! {
                        changed = wanted.changed() => {
                            if changed.is_err() {
                                break;
                            }
                            let state = wanted.borrow_and_update().clone();
                            if let Err(e) = tray.update(state).await {
                                eprintln!("orbis: tray: {e}");
                            }
                        }
                        changed = shown.changed() => {
                            if changed.is_err() {
                                break;
                            }
                            report(*shown.borrow_and_update());
                        }
                        Some(event) = events.recv() => {
                            me.invoke_mut(move |shell| shell.on_tray_event(event));
                        }
                        _ = &mut stop => break,
                    }
                }
                if let Err(e) = tray.stop().await {
                    eprintln!("orbis: tray: {e}");
                }
                report(false);
            });
        }

        /// Puts the icon into the tray or takes it out (the settings).
        /// Without it, closing the window quits.
        pub fn set_tray(&mut self, on: bool) {
            if on {
                self.start_tray();
            } else if let Some(stop) = self.tray_stop.take() {
                self.tray_state = None;
                let _ = stop.send(());
            }
        }

        /// Whether the tray icon is wanted and the hidden window unloaded
        /// (smoke test).
        pub fn tray_settings(&self) -> (bool, bool) {
            (self.tray_state.is_some(), self.unload_hidden)
        }

        /// Shows the window with the settings open (the tray's menu, the
        /// smoke test).
        pub fn open_settings(&mut self, token: Option<&str>) {
            self.present(token);
            if !self.settings_wanted {
                self.settings_wanted = true;
                self.settings_changed();
            }
        }

        /// Whether the window hidden in the tray is unloaded (the settings).
        pub fn set_unload_hidden(&mut self, unload: bool) {
            if unload != self.unload_hidden {
                self.unload_hidden = unload;
                self.window_changed();
            }
        }

        /// The window drew a frame; the first one ends the start. With
        /// `ORBIS_SMOKE_START` the time is printed and orbis quits.
        #[qslot]
        pub fn frame_shown(&mut self) {
            if self.first_frame.is_some() {
                return;
            }
            let since_main = crate::STARTED
                .get()
                .map(|s| s.elapsed())
                .unwrap_or_default();
            let since_exec = crate::process_age();
            self.first_frame = Some((since_main, since_exec));
            if std::env::var_os("ORBIS_SMOKE_START").is_some() {
                let steps = crate::START_STEPS
                    .lock()
                    .map(|steps| {
                        steps
                            .iter()
                            .map(|(name, at)| format!("{name} {} ms", at.as_millis()))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                println!(
                    "smoke test, start: first frame {} ms after main, {} after exec ({steps})",
                    since_main.as_millis(),
                    since_exec.map_or("?".to_owned(), |d| format!("{} ms", d.as_millis()))
                );
                self.backend.invoke_mut(|b| b.request_quit());
            }
        }

        /// From `main` and from the process's start to the first frame of
        /// the run (smoke test).
        pub fn first_frame(&self) -> Option<(Duration, Option<Duration>)> {
            self.first_frame
        }

        /// The window has opened the settings the tray asked for.
        #[qslot]
        pub fn settings_opened(&mut self) {
            if self.settings_wanted {
                self.settings_wanted = false;
                self.settings_changed();
            }
        }

        /// Lets MPRIS show the track and passes clients' asks (`events`)
        /// to the backend and the window.
        pub fn start_mpris(&mut self, mut events: UnboundedReceiver<MprisEvent>) {
            let (Some(me), Some(mpris)) = (self.me.clone(), self.mpris.clone()) else {
                return;
            };
            let (state_sender, mut wanted) =
                watch::channel(mpris_state(&self.track, self.art.clone()));
            self.media_state = Some(state_sender);
            self.runtime.spawn(async move {
                loop {
                    tokio::select! {
                        changed = wanted.changed() => {
                            if changed.is_err() {
                                break;
                            }
                            let state = wanted.borrow_and_update().clone();
                            if let Err(e) = mpris.update(state).await {
                                eprintln!("orbis: {e}");
                            }
                        }
                        Some(event) = events.recv() => {
                            me.invoke_mut(move |shell| shell.on_media_event(event));
                        }
                    }
                }
            });
        }

        fn on_media_event(&mut self, event: MprisEvent) {
            let backend = &self.backend;
            let playing = self.track.playing;
            match event {
                MprisEvent::Raise { token } => self.present(token.as_deref()),
                MprisEvent::Quit => backend.invoke_mut(|b| b.request_quit()),
                MprisEvent::PlayPause => backend.invoke_mut(|b| b.toggle_pause()),
                MprisEvent::Play if !playing => backend.invoke_mut(|b| b.toggle_pause()),
                // There is no stop: it pauses.
                MprisEvent::Pause | MprisEvent::Stop if playing => {
                    backend.invoke_mut(|b| b.toggle_pause());
                }
                MprisEvent::Play | MprisEvent::Pause | MprisEvent::Stop => {}
                MprisEvent::Next => backend.invoke_mut(|b| b.next()),
                MprisEvent::Previous => backend.invoke_mut(|b| b.previous()),
                MprisEvent::Seek { by, back } => {
                    let by = if back {
                        -by.as_secs_f64()
                    } else {
                        by.as_secs_f64()
                    };
                    backend.invoke_mut(move |b| b.seek_by(by));
                }
                MprisEvent::SetPosition { track, position } => {
                    if track == self.track.video_id {
                        backend.invoke_mut(move |b| b.seek(position.as_secs_f64()));
                    }
                }
                MprisEvent::Volume(volume) => {
                    backend.invoke_mut(move |b| b.set_volume(volume * 100.0))
                }
            }
        }

        fn sync_media(&self) {
            if let Some(sender) = &self.media_state {
                sender.send_replace(mpris_state(&self.track, self.art.clone()));
            }
        }

        /// Finds the current track's cover for MPRIS: from the cache at
        /// once, or downloaded in the background.
        fn fetch_art(&mut self) {
            self.art = None;
            let url = self.track.thumbnail.clone();
            let (Some(cache), Some(me)) = (self.covers.clone(), self.me.clone()) else {
                return;
            };
            if url.is_empty() {
                return;
            }
            if let Some(file) = cache.cached(&url, ART_PX, ART_PX) {
                self.art = Some(file_url(&file));
                return;
            }
            self.runtime.spawn(async move {
                if let Ok(file) = cache.fetch(&url, ART_PX, ART_PX).await {
                    let art = file_url(&file);
                    me.invoke_mut(move |shell| {
                        if shell.track.thumbnail == url {
                            shell.art = Some(art);
                            shell.sync_media();
                        }
                    });
                }
            });
        }

        fn set_tray_shown(&mut self, shown: bool) {
            if shown != self.tray_shown {
                self.tray_shown = shown;
                self.tray_changed();
            }
            // The tray went away with the window in it: bring the window back.
            if !shown && !self.window_visible {
                self.show_window();
            }
        }

        fn on_tray_event(&mut self, event: TrayEvent) {
            let backend = &self.backend;
            match event {
                // A click hides the window in front, brings up any other.
                TrayEvent::Activate { token } => {
                    if self.window_visible && self.window_active {
                        self.hide_window();
                    } else {
                        self.present(token.as_deref());
                    }
                }
                TrayEvent::SecondaryActivate { .. } => backend.invoke_mut(|b| b.toggle_pause()),
                TrayEvent::Scroll { delta } => {
                    let by = f64::from(delta) / 120.0 * VOLUME_STEP;
                    backend.invoke_mut(move |b| b.change_volume(by));
                }
                TrayEvent::Menu { action, token } => match action {
                    PLAY_PAUSE => backend.invoke_mut(|b| b.toggle_pause()),
                    NEXT => backend.invoke_mut(|b| b.next()),
                    PREVIOUS => backend.invoke_mut(|b| b.previous()),
                    LIKE => backend.invoke_mut(|b| b.like()),
                    WINDOW if self.window_visible => self.hide_window(),
                    WINDOW => self.present(token.as_deref()),
                    QUIT => backend.invoke_mut(|b| b.request_quit()),
                    SETTINGS => self.open_settings(token.as_deref()),
                    // The window asks for a sign-in whenever it is shown
                    // without one.
                    SIGN_IN => self.present(token.as_deref()),
                    _ => {}
                },
            }
        }

        /// The language changed: the tray's menu says it anew.
        pub fn retranslate(&self) {
            self.sync_tray();
        }

        fn sync_tray(&self) {
            if let Some(sender) = &self.tray_state {
                sender.send_replace(tray_state(&self.track, self.window_visible, self.signed_in));
            }
        }

        /// Whether the backend's session is accepted; QML passes it on
        /// whenever it changes.
        #[qslot]
        pub fn set_signed_in(&mut self, signed_in: bool) {
            if signed_in != self.signed_in {
                self.signed_in = signed_in;
                self.sync_tray();
            }
        }

        /// The tray's tooltip text and menu labels (smoke test).
        pub fn tray_texts(&self) -> (String, Vec<String>) {
            let state = tray_state(&self.track, self.window_visible, self.signed_in);
            let labels = state
                .menu
                .iter()
                .filter_map(|entry| match entry {
                    desktop::MenuEntry::Item { label, .. } => Some(label.clone()),
                    desktop::MenuEntry::Separator => None,
                })
                .collect();
            (state.text, labels)
        }

        fn set_window_visible(&mut self, visible: bool) {
            if visible != self.window_visible {
                self.window_visible = visible;
                self.window_changed();
                self.sync_tray();
                self.backend
                    .invoke_mut(move |b| b.set_window_shown(visible));
            }
        }

        #[qslot]
        pub fn show_window(&mut self) {
            self.set_window_visible(true);
        }

        /// Hides the window in the tray (QML calls it when the window is
        /// closed and a tray shows the icon).
        #[qslot]
        pub fn hide_window(&mut self) {
            self.set_window_visible(false);
        }

        /// Shows the window (QML builds it in the signal, if it was
        /// unloaded) and gives it the focus with the tray's activation token.
        fn present(&mut self, token: Option<&str>) {
            self.set_window_visible(true);
            window_system::activate_window(token);
        }

        /// Shows a downloaded file (`file://` URL) selected in the file
        /// manager; without one on the bus, QML opens `folder` instead.
        #[qslot]
        pub fn show_in_folder(&mut self, file: String, folder: String) {
            let (Some(bus), Some(me)) = (self.connection.clone(), self.me.clone()) else {
                self.folder_wanted(folder);
                return;
            };
            self.runtime.spawn(async move {
                if let Err(e) = desktop::file_manager::show_items(&bus, &[file]).await {
                    eprintln!("orbis: no file manager to show the file, opening its folder: {e}");
                    me.invoke_mut(move |s| s.folder_wanted(folder));
                }
            });
        }

        /// The window got or lost the focus.
        #[qslot]
        pub fn window_activated(&mut self, active: bool) {
            self.window_active = active;
        }

        /// The current track and playback, for the tray and MPRIS; QML
        /// passes them on whenever the backend's change.
        #[qslot]
        #[allow(clippy::too_many_arguments)]
        pub fn show_track(
            &mut self,
            video_id: String,
            title: String,
            artists: String,
            thumbnail: String,
            duration: f64,
            playing: bool,
            like_status: String,
            volume: f64,
        ) {
            let track = Track {
                video_id,
                title,
                artists,
                thumbnail,
                duration,
                playing,
                liked: like_status == "like",
                volume,
            };
            if track == self.track {
                return;
            }
            let new_cover = track.thumbnail != self.track.thumbnail;
            self.track = track;
            if new_cover {
                self.fetch_art();
            }
            self.sync_tray();
            self.sync_media();
        }

        /// The position jumped (a seek, from the window or a client): MPRIS
        /// tells its clients.
        #[qslot]
        pub fn media_seeked(&mut self, position: f64) {
            let Some(mpris) = self.mpris.clone() else {
                return;
            };
            let position = Duration::from_secs_f64(position.max(0.0));
            self.runtime.spawn(async move {
                let _ = mpris.seeked(position).await;
            });
        }

        /// The window was unloaded: memory its scene let go of goes back to
        /// the system.
        #[qslot]
        pub fn trim_memory(&mut self) {
            release_memory();
        }
    }
}

pub use shell::Shell;

#[cfg(test)]
mod tests {
    use super::*;

    use desktop::MenuEntry;

    fn labels(state: &desktop::TrayState) -> Vec<(String, bool)> {
        state
            .menu
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Item { label, enabled, .. } => Some((label.clone(), *enabled)),
                MenuEntry::Separator => None,
            })
            .collect()
    }

    #[test]
    fn the_tray_shows_the_track_and_offers_what_makes_sense() {
        let idle = tray_state(&Track::default(), true, true);
        assert_eq!(idle.title, "orbis");
        // The orbis icon itself, as pictures, whatever plays.
        assert_eq!(idle.icon, "");
        let sizes: Vec<_> = idle.pixmaps.iter().map(|(w, h, _)| (*w, *h)).collect();
        assert_eq!(sizes.len(), TRAY_SIZES.len());
        for (w, h, argb) in idle.pixmaps.iter() {
            assert_eq!(w, h);
            assert_eq!(argb.len(), (w * h * 4) as usize);
        }
        let (_, _, big) = idle.pixmaps.last().unwrap();
        assert!(
            big.chunks(4).any(|p| p[0] == 0xff),
            "the icon has opaque pixels"
        );
        assert_eq!(
            labels(&idle),
            [
                ("Play".to_owned(), false),
                ("Next".to_owned(), false),
                ("Previous".to_owned(), false),
                ("Like".to_owned(), false),
                ("Hide window".to_owned(), true),
                ("Settings…".to_owned(), true),
                ("Quit".to_owned(), true),
            ]
        );

        let track = Track {
            title: "Song".into(),
            artists: "Artist".into(),
            playing: true,
            liked: true,
            ..Track::default()
        };
        let playing = tray_state(&track, false, true);
        assert_eq!(
            (playing.title.as_str(), playing.text.as_str()),
            ("Song", "Artist")
        );
        assert_eq!(playing.pixmaps, idle.pixmaps);
        assert_eq!(labels(&playing)[0], ("Pause".to_owned(), true));
        assert_eq!(labels(&playing)[4].0, "Show window");
        assert!(playing.menu.contains(&MenuEntry::Item {
            action: LIKE,
            label: "Like".into(),
            enabled: true,
            checked: Some(true),
        }));
    }

    #[test]
    fn without_a_session_the_tray_says_so_and_offers_a_sign_in() {
        let track = Track {
            title: "Song".into(),
            artists: "Artist".into(),
            playing: true,
            ..Track::default()
        };
        let out = tray_state(&track, false, false);
        assert_eq!(out.title, "Song", "what plays still shows");
        assert_eq!(out.text, "YouTube Music needs a sign-in");
        assert_eq!(
            out.menu[..2],
            [
                MenuEntry::Item {
                    action: SIGN_IN,
                    label: "Sign in…".into(),
                    enabled: true,
                    checked: None,
                },
                MenuEntry::Separator,
            ]
        );
        let signed_in = tray_state(&track, false, true);
        assert!(
            !signed_in
                .menu
                .iter()
                .any(|entry| matches!(entry, MenuEntry::Item { action, .. } if *action == SIGN_IN))
        );
        assert_eq!(signed_in.text, "Artist");
    }

    #[test]
    fn mpris_shows_the_track_with_its_cover_and_link() {
        assert_eq!(mpris_state(&Track::default(), None).track, None);
        let track = Track {
            video_id: "abc".into(),
            title: "Song".into(),
            artists: "One, Two".into(),
            thumbnail: "https://lh3.googleusercontent.com/x=w60-h60".into(),
            duration: 200.5,
            playing: true,
            liked: false,
            volume: 40.0,
        };
        let state = mpris_state(&track, Some("file:///c.jpg".into()));
        assert!(state.playing);
        assert!((state.volume - 0.4).abs() < 1e-9);
        let shown = state.track.unwrap();
        assert_eq!(shown.id, "abc");
        assert_eq!(shown.artists, ["One, Two"]);
        assert_eq!(shown.length, Some(Duration::from_secs_f64(200.5)));
        assert_eq!(shown.art_url.as_deref(), Some("file:///c.jpg"));
        assert_eq!(
            shown.url.as_deref(),
            Some("https://music.youtube.com/watch?v=abc")
        );
    }
}
