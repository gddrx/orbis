//! The object QML talks to: commands from the UI become calls on the playback
//! service and the YouTube Music session, long work runs on tokio, and the
//! results come back to the GUI thread through a `QThreadHandle`.

use qtbridge::qobject;

use std::cell::RefCell;

use crate::i18n::{Language, tr, tr_in, trf, trf_in};
use std::rc::Rc;
use std::sync::Arc;

use crate::models::{
    CardRow, CardsModel, DownloadsModel, ShelfRow, ShelvesModel, TabsModel, TrackRow, TracksModel,
    format_duration,
};
use orbis_core::{Playback, SessionManager};
use player::{QueueItem, Rated, TabSource};

/// Told about playback when it starts: at launch, or after a sign-in.
pub type OnStarted = Box<dyn Fn(&Arc<Playback>)>;

/// What the backend works with once signed in.
pub struct Services {
    pub session: Arc<SessionManager>,
    pub playback: Arc<Playback>,
}

/// Where shelves are scrolled (smoke test): the shelf at the top of the view
/// (-1: the page's header) and how far into it, in pixels, or at the end.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShelvesPlace {
    pub shelf: i32,
    pub offset: f64,
    pub at_end: bool,
}

impl ShelvesPlace {
    /// The same place, give or take a pixel or two of rounding.
    pub fn same_as(&self, other: &ShelvesPlace) -> bool {
        (self.at_end && other.at_end)
            || (self.shelf == other.shelf && (self.offset - other.offset).abs() <= 2.0)
    }

    /// At the top of the page.
    pub fn at_top(&self) -> bool {
        self.shelf <= 0 && self.offset.abs() < 1.0
    }
}

/// Content loaded for a tab that has not played yet.
#[derive(Clone)]
pub(crate) enum Loaded {
    Tracks {
        items: Vec<QueueItem>,
        continuation: Option<String>,
    },
    Cards(Vec<CardRow>),
    /// Shelves of carousels; the home page has more pages after
    /// `continuation`.
    Shelves {
        rows: Vec<ShelfRow>,
        continuation: Option<String>,
    },
}

/// What the top of a page shows: an album's or artist's name and picture.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct PageHeader {
    pub title: String,
    pub subtitle: String,
    pub thumbnail: String,
}

pub struct Models {
    pub tabs: Rc<RefCell<TabsModel>>,
    pub tracks: Rc<RefCell<TracksModel>>,
    pub cards: Rc<RefCell<CardsModel>>,
    pub shelves: Rc<RefCell<ShelvesModel>>,
    pub options: Rc<RefCell<CardsModel>>,
    pub downloads: Rc<RefCell<DownloadsModel>>,
}

/// A tab's name in `language`. The home page, the library, a search and a
/// radio are named from what they show, so they follow the language; other
/// tabs keep the name of their playlist, album or artist.
fn tab_title(language: Language, tab: &player::Tab) -> String {
    match &tab.source {
        TabSource::Home => tr_in(language, "Home").to_owned(),
        TabSource::Library => tr_in(language, "Library").to_owned(),
        TabSource::Search { query } => trf_in(language, "Search: %1", &[query]),
        TabSource::Radio { .. } => trf_in(language, "Radio: %1", &[&radio_track(&tab.title)]),
        _ => tab.title.clone(),
    }
}

/// The track of a radio tab, from its stored title: the track's title
/// alone, or — in tabs saved by earlier versions — "Radio: …" in the
/// language the tab was opened in.
fn radio_track(title: &str) -> &str {
    [Language::English, Language::Ukrainian]
        .into_iter()
        .filter_map(|language| tr_in(language, "Radio: %1").strip_suffix("%1"))
        .find_map(|prefix| title.strip_prefix(prefix))
        .unwrap_or(title)
}

/// A playback failure in `language`: its kind in words, then its cause as
/// it came. `title` names the track of a video ID when a queue has it.
fn failure_text(
    language: Language,
    failure: &orbis_core::PlaybackFailure,
    title: impl Fn(&str) -> Option<String>,
) -> String {
    use orbis_core::PlaybackFailure;
    match failure {
        PlaybackFailure::Play {
            video_id: Some(id),
            cause,
        } => {
            let track = title(id).unwrap_or_else(|| id.clone());
            trf_in(language, "Cannot play “%1”: %2", &[&track, cause])
        }
        PlaybackFailure::Play {
            video_id: None,
            cause,
        } => trf_in(language, "Cannot play: %1", &[cause]),
        PlaybackFailure::SaveTabs(cause) => trf_in(language, "Cannot save the tabs: %1", &[cause]),
        PlaybackFailure::PrepareNext(cause) => {
            trf_in(language, "Cannot prepare the next track: %1", &[cause])
        }
        PlaybackFailure::LoadMore(cause) => {
            trf_in(language, "Cannot load more tracks: %1", &[cause])
        }
    }
}

/// How the session was signed in, for the settings: "Signed in with a
/// Google window", "Signed in from Chrome".
fn source_text(source: &orbis_core::Source) -> String {
    use orbis_core::{Browser, Source};
    match source {
        Source::Window { .. } => tr("Signed in with a Google window").to_owned(),
        Source::Browser { browser, .. } => trf(
            "Signed in from %1",
            &[&match browser {
                Browser::Chrome => "Chrome",
                Browser::Chromium => "Chromium",
                Browser::Brave => "Brave",
                Browser::Vivaldi => "Vivaldi",
                Browser::Firefox => "Firefox",
            }],
        ),
    }
}

/// «Download all» on a list longer than this asks first: the Liked Music of
/// an account can hold thousands of tracks.
pub(crate) const ASK_ABOVE: usize = 100;

/// Ratings asked for at once when rows come into view: one `next` request
/// each.
const RATING_CHECKS_AT_ONCE: usize = 4;

/// Whether a tab's whole content can be downloaded: an album or a playlist,
/// not a mix or a radio, which never end.
pub(crate) fn downloadable(source: &TabSource) -> bool {
    match source {
        TabSource::Album { .. } => true,
        TabSource::Playlist { id } => !id.starts_with("RD"),
        _ => false,
    }
}

/// Gives memory that big responses left behind back to the system: glibc
/// keeps freed memory in its arenas otherwise (measured: 32 MB after the
/// home page and a few playlists).
pub(crate) fn release_memory() {
    // SAFETY: plain libc call without arguments to uphold.
    unsafe {
        libc::malloc_trim(0);
    }
}

pub(crate) fn kind_of(source: &TabSource) -> &'static str {
    match source {
        TabSource::Home => "home",
        TabSource::Library => "library",
        TabSource::Playlist { .. } => "playlist",
        TabSource::Album { .. } => "album",
        TabSource::Artist { .. } => "artist",
        TabSource::Search { .. } => "search",
        TabSource::Radio { .. } => "radio",
    }
}

/// What the smoke test reads back from the window.
#[derive(Debug, Clone, Default)]
pub struct SmokeState {
    pub menu_video_id: String,
    pub menu_title: String,
    pub menu_album: String,
    pub menu_artists: Vec<String>,
    pub menu_rating: String,
    pub menu_ready: bool,
    /// The track after the current one in the queue that plays, and the
    /// last one.
    pub next: Option<String>,
    pub last: Option<String>,
    /// The kind of the tab that plays and of the shown one.
    pub playing_kind: String,
    pub shown_kind: String,
    pub status: String,
    /// The sign-in: done, under way, and why the last one failed.
    pub signed_in: bool,
    pub signing_in: bool,
    pub sign_in_error: String,
    /// The stage of every download of the list, in order.
    pub downloads: Vec<String>,
    pub downloads_active: i32,
    pub shown_downloadable: bool,
    /// «Download all» reads its list, and how many tracks its question is
    /// about (0: none).
    pub download_reading: bool,
    /// The shown search's filter, and how many cards the shelves hold.
    pub search_filter: String,
    pub shelf_cards: usize,
    /// The suggestions under the search field.
    pub suggestions: Vec<String>,
    pub download_ask: i32,
    /// The window made on first use that QML last reported shown.
    pub window_shown: String,
    /// What playback and downloads use now: the quality's name, whether
    /// loudness is corrected, the format and the music folder.
    pub quality: String,
    pub normalize: bool,
    pub download_format: String,
    pub download_root: String,
}

/// What the sign-in dialog says about a failed sign-in; `None` when the
/// user closed the window, which needs no words.
pub(crate) fn sign_in_message(e: &orbis_core::CoreError) -> Option<String> {
    use desktop::login_window::LoginWindowError;
    use orbis_core::CoreError;
    Some(match e {
        CoreError::LoginWindow(LoginWindowError::Cancelled) => return None,
        CoreError::LoginWindow(LoginWindowError::Start { .. }) => {
            trf("Could not open the sign-in window: %1", &[e])
        }
        CoreError::LoginWindow(_) => trf("The sign-in window closed with an error: %1", &[e]),
        CoreError::WindowSignedOut => {
            tr("The sign-in window closed without signing in to YouTube.").to_owned()
        }
        CoreError::BrowserSignedOut => tr(
            "This browser is not signed in to YouTube Music: open music.youtube.com there and sign in.",
        )
        .to_owned(),
        CoreError::Import(e) => trf("Could not read the browser's sign-in: %1", &[e]),
        CoreError::Api(api) if api.is_auth_failure() => {
            tr("YouTube Music did not accept the sign-in.").to_owned()
        }
        e => trf("Sign-in failed: %1", &[e]),
    })
}

/// `like`, `dislike` or empty, as QML shows a rating.
pub(crate) fn rated_name(rated: Rated) -> &'static str {
    match rated {
        Rated::Like => "like",
        Rated::Dislike => "dislike",
        Rated::None => "",
    }
}

pub(crate) fn rated_from_name(name: &str) -> Rated {
    match name {
        "like" => Rated::Like,
        "dislike" => Rated::Dislike,
        _ => Rated::None,
    }
}

pub(crate) fn rating_of(rated: Rated) -> ytm::Rating {
    match rated {
        Rated::Like => ytm::Rating::Like,
        Rated::Dislike => ytm::Rating::Dislike,
        Rated::None => ytm::Rating::None,
    }
}

pub(crate) fn track_row(item: &QueueItem, current: bool) -> TrackRow {
    TrackRow {
        video_id: item.video_id.clone(),
        title: item.title.clone(),
        artists: item.artists.clone(),
        duration: format_duration(item.duration),
        thumbnail: item.thumbnail.clone().unwrap_or_default(),
        current,
        rating: rated_name(item.rated).to_owned(),
    }
}

#[qobject(NoQmlElement, ConvertToCamelCase)]
mod backend {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::future::Future;
    use std::rc::Rc;
    use std::sync::Arc;

    use orbis_core::CoreError;
    use orbis_core::settings::{DownloadSettings, PlaybackSettings};
    use orbis_core::{
        Browser, DownloadEvent, Downloads, MusicBrainz, Outcome, Playback, PlaybackEvent,
        SessionManager, Settings, SignedIn, Wanted, login_program,
    };
    use player::{QueueItem, Rated, TabId, TabSource};
    use qtbridge::{QThreadHandle, QmlObject};
    use tokio::runtime::Handle;
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
    use ytm::{Client, LIKED_MUSIC, Privacy};

    use super::{
        ASK_ABOVE, Loaded, Models, OnStarted, PageHeader, RATING_CHECKS_AT_ONCE, Services,
        ShelvesPlace, SmokeState, downloadable, failure_text, kind_of, rated_from_name, rated_name,
        rating_of, release_memory, sign_in_message, source_text, tab_title, track_row,
    };
    use crate::content::{
        filter_title, joined, names, queue_items, search_page_rows, search_rows, shelf_rows,
    };
    use crate::i18n::{tr, trf};
    use crate::models::{CardRow, DownloadRow, TabRow};
    use crate::start::Starter;

    pub struct Backend {
        now_title: String,
        now_artists: String,
        now_thumbnail: String,
        now_video_id: String,
        playing: bool,
        position: f64,
        duration: f64,
        volume: f64,
        /// `like`, `dislike` or empty.
        like_status: String,
        /// The video ID of the track asked to play until its audio starts,
        /// `…` for the next or previous one, or empty.
        starting: String,
        shown_kind: String,
        page_title: String,
        page_subtitle: String,
        page_thumbnail: String,
        /// The shown shelves have more pages to load.
        has_more: bool,
        /// Some tabs are not pinned: the window shows them in a column at
        /// the left, the pinned ones (home, mixes, library) at the top.
        side_tabs: bool,
        status: String,
        loading: bool,
        signed_in: bool,

        services: Option<Services>,
        models: Models,
        me: Option<Arc<QThreadHandle<Backend>>>,
        loaded: HashMap<TabId, Loaded>,
        headers: HashMap<TabId, PageHeader>,
        /// The tab whose tracks the tracks model holds.
        tracks_of: Option<TabId>,
        /// The tab whose shelves the shelves model holds, or -1: QML keeps
        /// where each tab's shelves were scrolled.
        shelves_tab: i64,
        /// The tab whose next page of shelves is being loaded.
        loading_more: Option<TabId>,
        /// A radio tab to start playing once its tracks are loaded.
        play_when_loaded: Option<TabId>,

        /// The track the track menu and the "add to playlist" dialog are for.
        menu_item: Option<QueueItem>,
        menu_video_id: String,
        menu_title: String,
        /// `like`, `dislike` or empty.
        menu_rating: String,
        menu_album: String,
        menu_album_id: String,
        menu_artists: Vec<String>,
        menu_artist_ids: Vec<String>,
        /// The album, artists and rating of the menu's track have come.
        menu_ready: bool,

        /// Gets the number of shelves and cards on the home page when the
        /// smoke test's scroll is over.
        smoke_listener: Option<UnboundedSender<(usize, usize)>>,
        /// Gets where the shown shelves are scrolled (smoke test).
        place_listener: Option<UnboundedSender<ShelvesPlace>>,
        /// The window made on first use that QML last reported shown, or
        /// empty (smoke test).
        smoke_window: String,

        /// A sign-in is under way.
        signing_in: bool,
        /// What the sign-in waits for.
        sign_in_note: String,
        /// Why the last sign-in failed, or empty.
        sign_in_error: String,
        /// The browsers with a cookie database, as the dialog names them
        /// (`chrome`, `firefox`…): the sign-in dialog offers only these.
        sign_in_browsers: Vec<String>,
        sign_in_task: Option<tokio::task::AbortHandle>,
        /// The tabs were closed on signing out: the next account's mixes
        /// are to be pinned.
        mixes_wanted: bool,
        /// The queries YouTube Music suggests for what the search field
        /// holds, best first; empty when nothing is typed.
        search_suggestions: Vec<String>,
        /// The request for suggestions under way: a newer one replaces it.
        suggest_task: Option<tokio::task::AbortHandle>,
        runtime: Handle,
        /// Starts playback once signed in.
        starter: Starter,
        /// Told about playback when it starts (MPRIS's position).
        on_started: OnStarted,

        downloads: Option<Arc<Downloads>>,
        /// Downloads that wait or run.
        downloads_active: i32,
        /// The shown tab is an album or a playlist that can be downloaded.
        shown_downloadable: bool,
        /// What the shown search is narrowed to: `songs`, `videos`,
        /// `albums`, `artists`, or empty for everything.
        search_filter: String,
        /// The filters of search tabs, kept for this run.
        search_filters: HashMap<TabId, ytm::SearchFilter>,
        /// «Download all» reads the shown list before it queues it.
        download_reading: bool,
        /// A list longer than [`ASK_ABOVE`] waits for the user's word: its
        /// tracks, its title and how many they are (0: no question).
        download_question: Vec<Wanted>,
        download_ask_title: String,
        download_ask: i32,

        /// The settings of playback and downloads (the settings object
        /// changes them).
        playback_settings: PlaybackSettings,
        download_settings: DownloadSettings,
        /// The window is shown (not in the tray), for playback's yt-dlp.
        window_shown: bool,
        /// The rows QML last asked to fetch ahead (smoke test).
        visible_asked: Option<(i32, i32)>,
        musicbrainz: Arc<MusicBrainz>,
        /// The signed-in account's name and how it signed in, for the
        /// settings; empty until asked for.
        account_name: String,
        account_source: String,
    }

    impl Backend {
        qproperty!("nowTitle", Member = now_title, Notify = now_changed);
        qproperty!("nowArtists", Member = now_artists, Notify = now_changed);
        qproperty!("nowThumbnail", Member = now_thumbnail, Notify = now_changed);
        qproperty!("nowVideoId", Member = now_video_id, Notify = now_changed);
        qproperty!("playing", Member = playing, Notify = playing_changed);
        qproperty!("position", Member = position, Notify = position_changed);
        qproperty!("duration", Member = duration, Notify = now_changed);
        qproperty!("volume", Member = volume, Notify = volume_changed);
        qproperty!("likeStatus", Member = like_status, Notify = like_changed);
        qproperty!("starting", Member = starting, Notify = starting_changed);
        qproperty!("shownKind", Member = shown_kind, Notify = shown_changed);
        qproperty!("pageTitle", Member = page_title, Notify = page_changed);
        qproperty!(
            "pageSubtitle",
            Member = page_subtitle,
            Notify = page_changed
        );
        qproperty!(
            "pageThumbnail",
            Member = page_thumbnail,
            Notify = page_changed
        );
        qproperty!("hasMore", Member = has_more, Notify = more_changed);
        qproperty!("sideTabs", Member = side_tabs, Notify = side_tabs_changed);
        qproperty!("shelvesTab", Member = shelves_tab, Notify = shown_changed);
        qproperty!("menuVideoId", Member = menu_video_id, Notify = menu_changed);
        qproperty!("menuTitle", Member = menu_title, Notify = menu_changed);
        qproperty!("menuRating", Member = menu_rating, Notify = menu_changed);
        qproperty!("menuAlbum", Member = menu_album, Notify = menu_changed);
        qproperty!("menuArtists", Member = menu_artists, Notify = menu_changed);
        qproperty!("menuReady", Member = menu_ready, Notify = menu_changed);
        qproperty!("status", Member = status, Notify = status_changed);
        qproperty!("loading", Member = loading, Notify = loading_changed);
        qproperty!("signedIn", Member = signed_in, Notify = signed_in_changed);
        qproperty!("signingIn", Member = signing_in, Notify = sign_in_changed);
        qproperty!(
            "signInNote",
            Member = sign_in_note,
            Notify = sign_in_changed
        );
        qproperty!(
            "signInError",
            Member = sign_in_error,
            Notify = sign_in_changed
        );
        qproperty!(
            "signInBrowsers",
            Member = sign_in_browsers,
            Notify = sign_in_changed
        );
        qproperty!(
            "downloadsActive",
            Member = downloads_active,
            Notify = downloads_changed
        );
        qproperty!(
            "shownDownloadable",
            Member = shown_downloadable,
            Notify = shown_changed
        );
        qproperty!(
            "searchFilter",
            Member = search_filter,
            Notify = shown_changed
        );
        qproperty!(
            "searchSuggestions",
            Member = search_suggestions,
            Notify = suggestions_changed
        );
        qproperty!(
            "downloadReading",
            Member = download_reading,
            Notify = downloads_changed
        );
        qproperty!(
            "downloadAsk",
            Member = download_ask,
            Notify = downloads_changed
        );
        qproperty!(
            "downloadAskTitle",
            Member = download_ask_title,
            Notify = downloads_changed
        );
        qproperty!(
            "accountName",
            Member = account_name,
            Notify = account_changed
        );
        qproperty!(
            "accountSource",
            Member = account_source,
            Notify = account_changed
        );

        #[qsignal]
        fn now_changed(&mut self);
        #[qsignal]
        fn playing_changed(&mut self);
        #[qsignal]
        fn position_changed(&mut self);
        #[qsignal]
        fn volume_changed(&mut self);
        #[qsignal]
        fn like_changed(&mut self);
        #[qsignal]
        fn starting_changed(&mut self);
        #[qsignal]
        fn shown_changed(&mut self);
        #[qsignal]
        fn page_changed(&mut self);
        #[qsignal]
        fn more_changed(&mut self);
        #[qsignal]
        fn side_tabs_changed(&mut self);
        #[qsignal]
        fn menu_changed(&mut self);
        #[qsignal]
        fn status_changed(&mut self);
        #[qsignal]
        fn loading_changed(&mut self);
        #[qsignal]
        fn signed_in_changed(&mut self);
        #[qsignal]
        fn sign_in_changed(&mut self);
        #[qsignal]
        fn downloads_changed(&mut self);
        #[qsignal]
        fn account_changed(&mut self);
        /// Asks QML to close the application (the tray's and MPRIS's "quit",
        /// the smoke test).
        #[qsignal]
        fn quit_requested(&mut self);
        /// The position jumped to `position` seconds: a seek.
        #[qsignal]
        fn seeked(&mut self, position: f64);
        /// Asks QML to save a picture of the window's content (smoke test).
        #[qsignal]
        fn screenshot_requested(&mut self, path: String);
        /// Asks QML to scroll the shown shelves to the end, slowly, loading
        /// every page (smoke test).
        #[qsignal]
        fn scroll_requested(&mut self);
        /// Asks QML where the shown shelves are scrolled, after moving them
        /// to shelf `shelf` when it is not -1 (smoke test).
        #[qsignal]
        fn place_requested(&mut self, shelf: i32);
        /// Asks QML to open or close the downloads list (smoke test).
        #[qsignal]
        fn downloads_list_requested(&mut self, open: bool);
        /// Asks QML to open a window made on first use — `menu`,
        /// `playlist` or `login` — or, with an empty name, to close them
        /// (smoke test).
        #[qsignal]
        fn window_requested(&mut self, name: String);
        #[qsignal]
        fn suggestions_changed(&mut self);
        /// Asks QML to type `text` into the search field, as a user would
        /// (smoke test).
        #[qsignal]
        fn typing_requested(&mut self, text: String);
        /// Asks QML to choose the suggestion at `index` (smoke test).
        #[qsignal]
        fn suggestion_requested(&mut self, index: i32);

        /// Creates the backend, not signed in yet: [`Backend::install`] gives
        /// it the services of a saved session, or the user signs in.
        pub fn create(
            runtime: Handle,
            starter: Starter,
            on_started: OnStarted,
            models: Models,
            status: String,
            settings: &Settings,
        ) -> Rc<RefCell<Self>> {
            let backend = Rc::new(RefCell::new(Self {
                now_title: String::new(),
                now_artists: String::new(),
                now_thumbnail: String::new(),
                now_video_id: String::new(),
                playing: false,
                position: 0.0,
                duration: 0.0,
                volume: 100.0,
                like_status: String::new(),
                starting: String::new(),
                shown_kind: String::new(),
                page_title: String::new(),
                page_subtitle: String::new(),
                page_thumbnail: String::new(),
                has_more: false,
                side_tabs: false,
                status,
                loading: false,
                signed_in: false,
                services: None,
                models,
                me: None,
                loaded: HashMap::new(),
                headers: HashMap::new(),
                tracks_of: None,
                shelves_tab: -1,
                loading_more: None,
                play_when_loaded: None,
                menu_item: None,
                menu_video_id: String::new(),
                menu_title: String::new(),
                menu_rating: String::new(),
                menu_album: String::new(),
                menu_album_id: String::new(),
                menu_artists: Vec::new(),
                menu_artist_ids: Vec::new(),
                menu_ready: false,
                smoke_listener: None,
                place_listener: None,
                smoke_window: String::new(),
                signing_in: false,
                sign_in_note: String::new(),
                sign_in_error: String::new(),
                sign_in_browsers: Vec::new(),
                sign_in_task: None,
                search_suggestions: Vec::new(),
                suggest_task: None,
                mixes_wanted: false,
                runtime,
                starter,
                on_started,
                downloads: None,
                downloads_active: 0,
                shown_downloadable: false,
                search_filter: String::new(),
                search_filters: HashMap::new(),
                download_reading: false,
                download_question: Vec::new(),
                download_ask_title: String::new(),
                download_ask: 0,
                playback_settings: settings.playback.clone(),
                download_settings: settings.downloads.clone(),
                window_shown: true,
                visible_asked: None,
                musicbrainz: Arc::new(MusicBrainz::new(Some(
                    crate::cache_dir().join("musicbrainz"),
                ))),
                account_name: String::new(),
                account_source: String::new(),
            }));
            Self::attach_qobject(&backend);
            let handle = Arc::new(backend.borrow().get_thread_handle());
            backend.borrow_mut().me = Some(handle);
            backend
        }

        /// Takes the services of a signed-in session: playback events go to
        /// the GUI thread, a session that stops being accepted asks for a
        /// sign-in, and the tabs show.
        pub fn install(&mut self, services: Services, events: UnboundedReceiver<PlaybackEvent>) {
            (self.on_started)(&services.playback);
            if let Some(volume) = services.playback.volume() {
                self.volume = volume;
                self.volume_changed();
            }
            let signed_out = services.session.signed_out();
            services
                .playback
                .set_quality(self.playback_settings.quality);
            services
                .playback
                .set_skip_disliked(self.playback_settings.skip_disliked);
            services
                .playback
                .set_hide_disliked(self.playback_settings.hide_disliked);
            services.playback.set_window_shown(self.window_shown);
            if let Err(e) = services
                .playback
                .set_normalize(self.playback_settings.normalize)
            {
                self.set_status(e.to_string());
            }
            let (downloads, download_events) = {
                let _runtime = self.runtime.enter();
                Downloads::new(
                    Arc::clone(&services.session),
                    self.download_settings.options(&self.musicbrainz),
                )
            };
            // What was queued and not ended comes back.
            let kept = downloads.resume(&self.starter.data_dir.join("downloads.db"));
            self.downloads = Some(downloads);
            self.listen_downloads(download_events);
            self.services = Some(services);
            self.listen(events);
            self.watch_session(signed_out);
            self.set_signed_in(true);
            self.set_status(String::new());
            if let Err(e) = kept {
                self.set_status(trf(
                    "The download queue will not come back after a restart: %1",
                    &[&e],
                ));
            }
            self.refresh_tabs();
            if let Some(shown) = self.initial_tab() {
                self.show_tab(shown);
            }
            if self.playback().is_some_and(|p| p.is_first_start()) {
                self.pin_mixes();
            }
        }

        /// Hands playback events to the GUI thread.
        fn listen(&self, mut events: UnboundedReceiver<PlaybackEvent>) {
            let Some(me) = self.me.clone() else {
                return;
            };
            self.runtime.spawn(async move {
                while let Some(event) = events.recv().await {
                    me.invoke_mut(move |b| b.on_playback_event(event));
                }
            });
        }

        /// Follows whether the session is still accepted.
        fn watch_session(&self, mut signed_out: tokio::sync::watch::Receiver<bool>) {
            let Some(me) = self.me.clone() else {
                return;
            };
            self.runtime.spawn(async move {
                while signed_out.changed().await.is_ok() {
                    let out = *signed_out.borrow_and_update();
                    me.invoke_mut(move |b| b.set_signed_in(!out));
                }
            });
        }

        fn set_signed_in(&mut self, signed_in: bool) {
            if self.signed_in != signed_in {
                self.signed_in = signed_in;
                self.signed_in_changed();
            }
        }

        fn set_sign_in(&mut self, signing_in: bool, note: String, error: String) {
            if (self.signing_in, &self.sign_in_note, &self.sign_in_error)
                != (signing_in, &note, &error)
            {
                self.signing_in = signing_in;
                self.sign_in_note = note;
                self.sign_in_error = error;
                self.sign_in_changed();
            }
        }

        // ---- settings and account -----------------------------------------

        /// Plays and downloads by these settings from now on.
        pub fn apply_settings(&mut self, playback: PlaybackSettings, downloads: DownloadSettings) {
            if let Some(services) = &self.services {
                services.playback.set_quality(playback.quality);
                services.playback.set_skip_disliked(playback.skip_disliked);
                services.playback.set_hide_disliked(playback.hide_disliked);
                if let Err(e) = services.playback.set_normalize(playback.normalize) {
                    self.set_status(e.to_string());
                }
            }
            if let Some(queue) = &self.downloads {
                queue.set_options(&downloads.options(&self.musicbrainz));
            }
            self.playback_settings = playback;
            self.download_settings = downloads;
        }

        fn set_account(&mut self, name: String, source: String) {
            if (&self.account_name, &self.account_source) != (&name, &source) {
                self.account_name = name;
                self.account_source = source;
                self.account_changed();
            }
        }

        /// Reads the account's name and how it signed in (the settings show
        /// them).
        #[qslot]
        pub fn load_account(&mut self) {
            let (Some(session), Some(me)) = (self.session(), self.me.clone()) else {
                self.set_account(String::new(), String::new());
                return;
            };
            self.runtime.spawn(async move {
                let (account, source) = tokio::join!(session.account(), session.source());
                let name = match account {
                    Ok(account) => match account.handle {
                        Some(handle) => format!("{} ({handle})", account.name),
                        None => account.name,
                    },
                    Err(_) => String::new(),
                };
                let source = source
                    .ok()
                    .flatten()
                    .map(|s| source_text(&s))
                    .unwrap_or_default();
                me.invoke_mut(move |b| b.set_account(name, source));
            });
        }

        /// Signs out: the saved session is removed, playback pauses,
        /// downloads stop, and the sign-in dialog comes up. With
        /// `close_tabs`, the tabs and queues close too (see
        /// [`Backend::close_account_tabs`]).
        #[qslot]
        pub fn sign_out(&mut self, close_tabs: bool) {
            let Some(session) = self.session() else {
                return;
            };
            if self.playing {
                self.toggle_pause();
            }
            if let Some(queue) = &self.downloads {
                queue.cancel_all();
            }
            if close_tabs {
                self.close_account_tabs();
            }
            // Another account rates otherwise.
            if let Some(playback) = self.playback() {
                playback.forget_ratings();
            }
            self.set_account(String::new(), String::new());
            let me = self.me.clone();
            self.runtime.spawn(async move {
                if let Err(e) = session.sign_out().await
                    && let Some(me) = me
                {
                    let message = trf("Could not delete the saved sign-in: %1", &[&e]);
                    me.invoke_mut(move |b| b.set_status(message));
                }
            });
        }

        /// Closes every tab and queue of the account, and what the window
        /// holds of them: Home and Library stay, as at a first start, and
        /// the next account's mixes are pinned once it signs in.
        pub fn close_account_tabs(&mut self) {
            let Some(playback) = self.playback() else {
                return;
            };
            if let Err(e) = playback.close_all_tabs() {
                self.set_status(e.to_string());
            }
            self.loaded.clear();
            self.headers.clear();
            self.search_filters.clear();
            self.loading_more = None;
            self.play_when_loaded = None;
            self.tracks_of = None;
            self.now_title.clear();
            self.now_artists.clear();
            self.now_thumbnail.clear();
            self.now_video_id.clear();
            self.duration = 0.0;
            self.now_changed();
            self.mixes_wanted = true;
            self.refresh_tabs();
            if let Some(home) = playback.tabs().shown {
                self.show_tab(home as i64);
            }
        }

        // ---- downloads ----------------------------------------------------

        /// Hands download events to the GUI thread.
        fn listen_downloads(&self, mut events: UnboundedReceiver<DownloadEvent>) {
            let Some(me) = self.me.clone() else {
                return;
            };
            self.runtime.spawn(async move {
                while let Some(event) = events.recv().await {
                    me.invoke_mut(move |b| b.on_download_event(event));
                }
            });
        }

        fn on_download_event(&mut self, event: DownloadEvent) {
            let model = Rc::clone(&self.models.downloads);
            let mut model = model.borrow_mut();
            let id = match &event {
                DownloadEvent::Queued { id, title, artists } => {
                    model.append(vec![DownloadRow {
                        download_id: *id as i64,
                        title: title.clone(),
                        artists: artists.clone(),
                        stage: "queued".into(),
                        ..DownloadRow::default()
                    }]);
                    drop(model);
                    self.downloads_active += 1;
                    self.downloads_changed();
                    return;
                }
                DownloadEvent::Started { id, .. }
                | DownloadEvent::Progress { id, .. }
                | DownloadEvent::Finished { id, .. }
                | DownloadEvent::Failed { id, .. }
                | DownloadEvent::Tagged { id, .. }
                | DownloadEvent::Cancelled { id } => *id as i64,
            };
            let Some(index) = model.rows().iter().position(|r| r.download_id == id) else {
                return;
            };
            let mut row = model.rows()[index].clone();
            // A cancel can cross a download that has just ended: the first
            // end counts.
            if !row.is_open() {
                return;
            }
            match event {
                DownloadEvent::Started { title, .. } => {
                    row.stage = "active".into();
                    if row.title.is_empty() {
                        row.title = title;
                    }
                }
                DownloadEvent::Progress { share, .. } => row.share = share,
                DownloadEvent::Finished { outcome, .. } => {
                    let (state, path) = match outcome {
                        Outcome::Saved(path) => ("done", path),
                        Outcome::AlreadyThere(path) => ("skipped", path),
                    };
                    row.stage = state.into();
                    row.share = 1.0;
                    row.file = orbis_core::covers::file_url(&path);
                    row.folder = path
                        .parent()
                        .map(orbis_core::covers::file_url)
                        .unwrap_or_default();
                }
                DownloadEvent::Failed { message, .. } => {
                    row.stage = "failed".into();
                    row.message = message;
                }
                DownloadEvent::Cancelled { .. } => row.stage = "cancelled".into(),
                // The row ended with its file: MusicBrainz's tags follow
                // unseen.
                DownloadEvent::Queued { .. } | DownloadEvent::Tagged { .. } => {}
            }
            let ended = !row.is_open();
            model.update(index, row);
            drop(model);
            if ended {
                self.downloads_active -= 1;
                self.downloads_changed();
            }
        }

        /// Downloads the track of the menu.
        #[qslot]
        pub fn menu_download(&mut self) {
            let (Some(downloads), Some(item)) = (self.downloads.clone(), self.menu_item.clone())
            else {
                return;
            };
            downloads.track(Wanted {
                video_id: item.video_id,
                title: item.title,
                artists: if item.artists.is_empty() {
                    Vec::new()
                } else {
                    vec![item.artists]
                },
                album_id: (!self.menu_album_id.is_empty()).then(|| self.menu_album_id.clone()),
                meta: None,
            });
        }

        /// Downloads the whole album or playlist of the shown tab: reads it
        /// first and, when it is longer than [`ASK_ABOVE`] tracks, asks.
        #[qslot]
        pub fn download_shown(&mut self) {
            if self.download_reading || self.download_ask > 0 {
                return;
            }
            let (Some(downloads), Some(playback)) = (self.downloads.clone(), self.playback())
            else {
                return;
            };
            let source = {
                let tabs = playback.tabs();
                tabs.shown
                    .and_then(|id| tabs.get(id))
                    .map(|t| t.source.clone())
            };
            let Some(source) = source.filter(downloadable) else {
                return;
            };
            let title = self.page_title.clone();
            self.download_reading = true;
            self.downloads_changed();
            self.spawn(
                async move {
                    match source {
                        TabSource::Album { id } => downloads.read_album(&id).await,
                        TabSource::Playlist { id } => downloads.read_playlist(&id).await,
                        _ => Ok(Vec::new()),
                    }
                    .map_err(|e| e.to_string())
                },
                move |b, read| {
                    b.download_reading = false;
                    match read {
                        Ok(tracks) if tracks.len() > ASK_ABOVE => {
                            b.download_ask = i32::try_from(tracks.len()).unwrap_or(i32::MAX);
                            b.download_ask_title = title;
                            b.download_question = tracks;
                        }
                        Ok(tracks) => b.queue_downloads(tracks),
                        Err(e) => b.set_status(trf("Could not queue: %1", &[&e])),
                    }
                    b.downloads_changed();
                },
            );
        }

        /// Answers the question about a long list: `yes` queues it, else it
        /// is forgotten. Without a question, nothing happens.
        #[qslot]
        pub fn answer_download(&mut self, yes: bool) {
            if self.download_ask == 0 {
                return;
            }
            let tracks = std::mem::take(&mut self.download_question);
            self.download_ask = 0;
            self.download_ask_title.clear();
            self.downloads_changed();
            if yes {
                self.queue_downloads(tracks);
            }
        }

        /// Queues `tracks` and tells how many.
        fn queue_downloads(&mut self, tracks: Vec<Wanted>) {
            let Some(downloads) = &self.downloads else {
                return;
            };
            let queued = downloads.queue(tracks).len();
            self.set_status(trf(
                "Queued for download: %1; the download button is below",
                &[&queued],
            ));
        }

        /// Stops a download that waits or runs.
        #[qslot]
        pub fn cancel_download(&mut self, download_id: i64) {
            if let Some(downloads) = &self.downloads {
                downloads.cancel(download_id as u64);
            }
        }

        /// Stops every download that waits or runs.
        #[qslot]
        pub fn cancel_downloads(&mut self) {
            if let Some(downloads) = &self.downloads {
                downloads.cancel_all();
            }
        }

        /// Leaves only the downloads that wait or run in the list.
        #[qslot]
        pub fn clear_downloads(&mut self) {
            let model = Rc::clone(&self.models.downloads);
            let open: Vec<DownloadRow> = model
                .borrow()
                .rows()
                .iter()
                .filter(|r| r.is_open())
                .cloned()
                .collect();
            model.borrow_mut().replace(open);
        }

        // ---- signing in --------------------------------------------------

        /// Signs in through the Google sign-in window: a session of the
        /// player's own.
        #[qslot]
        pub fn sign_in_window(&mut self) {
            self.sign_in(
                tr("Sign in to your account in the Google window; it closes by itself afterwards."),
                async move { SignedIn::from_window(&login_program()).await },
            );
        }

        /// Signs in with the cookies of `browser` (`chrome`, `firefox`…): the
        /// session follows the browser's.
        #[qslot]
        pub fn sign_in_browser(&mut self, browser: String) {
            let Ok(browser) = serde_json::from_value::<Browser>(serde_json::Value::String(browser))
            else {
                return;
            };
            self.sign_in(tr("Taking the sign-in from the browser…"), async move {
                SignedIn::from_browser(browser, None)
                    .await
                    .map(|(signed_in, _)| signed_in)
            });
        }

        /// Looks which browsers have a cookie database to take a sign-in
        /// from (the sign-in dialog, as it opens): a few file checks.
        #[qslot]
        pub fn find_browsers(&mut self) {
            let found: Vec<String> = Browser::ALL
                .into_iter()
                .filter(|browser| browser.has_cookies())
                .filter_map(|browser| match serde_json::to_value(browser) {
                    Ok(serde_json::Value::String(name)) => Some(name),
                    _ => None,
                })
                .collect();
            if found != self.sign_in_browsers {
                self.sign_in_browsers = found;
                self.sign_in_changed();
            }
        }

        /// Stops the sign-in under way; the sign-in window closes.
        #[qslot]
        pub fn cancel_sign_in(&mut self) {
            if let Some(task) = self.sign_in_task.take() {
                task.abort();
            }
            self.set_sign_in(false, String::new(), String::new());
        }

        /// Runs `sign_in`, saves the session and puts it to work: in place of
        /// the current one, or with playback started anew.
        fn sign_in(
            &mut self,
            note: &str,
            sign_in: impl Future<Output = Result<SignedIn, CoreError>> + Send + 'static,
        ) {
            let Some(me) = self.me.clone() else {
                return;
            };
            if self.signing_in {
                return;
            }
            self.set_sign_in(true, note.to_owned(), String::new());
            let session = self.session();
            let starter = self.starter.clone();
            let task = self.runtime.spawn(async move {
                let started = async {
                    let signed_in = sign_in.await.map_err(|e| sign_in_message(&e))?;
                    if let Some(session) = session {
                        session
                            .replace(signed_in)
                            .await
                            .map_err(|e| sign_in_message(&e))?;
                        return Ok(None);
                    }
                    let session = Arc::new(
                        SessionManager::login(signed_in)
                            .await
                            .map_err(|e| sign_in_message(&e))?,
                    );
                    let (playback, events) =
                        starter.start(Arc::clone(&session)).await.map_err(Some)?;
                    let services = Services { session, playback };
                    Ok(Some((services, events)))
                }
                .await;
                me.invoke_mut(move |b| b.signed_in(started));
            });
            self.sign_in_task = Some(task.abort_handle());
        }

        #[allow(clippy::type_complexity)]
        fn signed_in(
            &mut self,
            started: Result<Option<(Services, UnboundedReceiver<PlaybackEvent>)>, Option<String>>,
        ) {
            self.sign_in_task = None;
            match started {
                Ok(started) => {
                    self.set_sign_in(false, String::new(), String::new());
                    match started {
                        Some((services, events)) => self.install(services, events),
                        // The session in place of the old one says so itself.
                        None => {
                            self.set_signed_in(true);
                            // The tabs were closed with the last account:
                            // this one's mixes, as at a first start.
                            if std::mem::take(&mut self.mixes_wanted) {
                                self.pin_mixes();
                            }
                        }
                    }
                }
                Err(error) => self.set_sign_in(false, String::new(), error.unwrap_or_default()),
            }
        }

        /// The first start: finds the personal mixes on the home page while
        /// the window is up, and pins their tabs after Home.
        fn pin_mixes(&mut self) {
            let Some(session) = self.session() else {
                return;
            };
            self.spawn(
                async move { crate::start::personal_mixes(&session).await },
                |b, (discover, new_release)| {
                    let Some(playback) = b.playback() else {
                        return;
                    };
                    playback.edit_tabs(|tabs| {
                        tabs.pin_mixes(discover.as_deref(), new_release.as_deref());
                    });
                    b.refresh_tabs();
                },
            );
        }

        /// Runs `work` on tokio and `done` with its result on the GUI thread.
        fn spawn<T: Send + 'static>(
            &self,
            work: impl Future<Output = T> + Send + 'static,
            done: impl FnOnce(&mut Backend, T) + Send + 'static,
        ) {
            let Some(me) = self.me.clone() else {
                return;
            };
            self.runtime.spawn(async move {
                let value = work.await;
                me.invoke_mut(move |b| done(b, value));
            });
        }

        pub fn set_status(&mut self, status: String) {
            if self.status != status {
                self.status = status;
                self.status_changed();
            }
        }

        fn set_loading(&mut self, loading: bool) {
            if self.loading != loading {
                self.loading = loading;
                self.loading_changed();
            }
        }

        fn set_header(&mut self, header: PageHeader) {
            if self.page_title != header.title
                || self.page_subtitle != header.subtitle
                || self.page_thumbnail != header.thumbnail
            {
                self.page_title = header.title;
                self.page_subtitle = header.subtitle;
                self.page_thumbnail = header.thumbnail;
                self.page_changed();
            }
        }

        fn set_has_more(&mut self, has_more: bool) {
            if self.has_more != has_more {
                self.has_more = has_more;
                self.more_changed();
            }
        }

        /// Loads something for a tab with the loading indicator on: `done`
        /// gets the value on the GUI thread, a failure shows as
        /// `<failed>: <error>`.
        fn load_with<T, F, Fut>(
            &mut self,
            failed: &'static str,
            op: F,
            done: impl FnOnce(&mut Backend, T) + Send + 'static,
        ) where
            T: Send + 'static,
            F: Fn(Arc<Client>) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = Result<T, ytm::Error>> + Send,
        {
            let Some(session) = self.session() else {
                return;
            };
            self.set_loading(true);
            self.spawn(
                async move { session.call(op).await.map_err(|e| e.to_string()) },
                move |b, result| {
                    b.set_loading(false);
                    match result {
                        Ok(value) => done(b, value),
                        Err(e) => b.set_status(format!("{failed}: {e}")),
                    }
                },
            );
        }

        fn playback(&self) -> Option<Arc<Playback>> {
            self.services.as_ref().map(|s| Arc::clone(&s.playback))
        }

        fn session(&self) -> Option<Arc<SessionManager>> {
            self.services.as_ref().map(|s| Arc::clone(&s.session))
        }

        // ---- tabs --------------------------------------------------------

        /// Rebuilds the tab strip from the service.
        pub fn refresh_tabs(&mut self) {
            let Some(playback) = self.playback() else {
                return;
            };
            let tabs = playback.tabs();
            let side_tabs = tabs.tabs().iter().any(|tab| !tab.pinned);
            let rows = tabs
                .tabs()
                .iter()
                .map(|tab| TabRow {
                    tab_id: tab.id as i64,
                    title: tab_title(crate::i18n::language(), tab),
                    kind: kind_of(&tab.source).into(),
                    pinned: tab.pinned,
                    playing: tabs.playing() == Some(tab.id),
                    shown: tabs.shown == Some(tab.id),
                })
                .collect();
            self.models
                .tabs
                .borrow_mut()
                .sync(rows, |old, new| old.tab_id == new.tab_id);
            if self.side_tabs != side_tabs {
                self.side_tabs = side_tabs;
                self.side_tabs_changed();
            }
        }

        #[qslot]
        pub fn show_tab(&mut self, tab_id: i64) {
            let Some(playback) = self.playback() else {
                return;
            };
            let id = tab_id as TabId;
            let source = playback.edit_tabs(|tabs| {
                let source = tabs.get(id).map(|t| t.source.clone());
                if source.is_some() {
                    tabs.shown = Some(id);
                }
                source
            });
            let Some(source) = source else {
                return;
            };
            self.refresh_tabs();
            let kind = kind_of(&source).to_owned();
            let can_download = downloadable(&source);
            let filter = self
                .search_filters
                .get(&id)
                .map(|f| f.name().to_owned())
                .unwrap_or_default();
            if self.shown_kind != kind
                || self.shown_downloadable != can_download
                || self.search_filter != filter
            {
                self.shown_kind = kind;
                self.shown_downloadable = can_download;
                self.search_filter = filter;
                self.shown_changed();
            }
            self.refresh_content(id, source);
        }

        /// Shows the content of tab `id`: its queue once it has played, else
        /// what was loaded for it, else loads it.
        fn refresh_content(&mut self, id: TabId, source: TabSource) {
            let Some(playback) = self.playback() else {
                return;
            };
            let tabs = playback.tabs();
            let header = match source {
                TabSource::Home | TabSource::Library => PageHeader::default(),
                _ => self
                    .headers
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| PageHeader {
                        title: tabs
                            .get(id)
                            .map(|t| tab_title(crate::i18n::language(), t))
                            .unwrap_or_default(),
                        ..PageHeader::default()
                    }),
            };
            self.set_header(header);
            let has_more = self.loading_more == Some(id)
                || matches!(
                    self.loaded.get(&id),
                    Some(Loaded::Shelves {
                        continuation: Some(_),
                        ..
                    })
                );
            self.set_has_more(has_more);
            if let Some(tab) = tabs.get(id)
                && !tab.queue.is_empty()
            {
                let current = (tabs.playing() == Some(id))
                    .then(|| tab.queue.current_index())
                    .flatten();
                let rows = tab
                    .queue
                    .items()
                    .iter()
                    .enumerate()
                    .map(|(i, item)| track_row(item, current == Some(i)))
                    .collect();
                self.show_tracks(id, rows);
                return;
            }
            match self.loaded.get(&id).cloned() {
                Some(Loaded::Tracks { items, .. }) => {
                    let rows = items.iter().map(|i| track_row(i, false)).collect();
                    self.show_tracks(id, rows);
                }
                Some(Loaded::Cards(cards)) => self.models.cards.borrow_mut().replace(cards),
                Some(Loaded::Shelves { rows, .. }) => {
                    self.shelves_tab = id as i64;
                    self.models.shelves.borrow_mut().replace(rows)
                }
                None => self.load(id, source),
            }
        }

        /// Shows the tracks of tab `id`. The same list again (another track
        /// plays, a rating changed) keeps where it was scrolled; another
        /// tab's list starts at the top.
        fn show_tracks(&mut self, id: TabId, rows: Vec<crate::models::TrackRow>) {
            let mut model = self.models.tracks.borrow_mut();
            if self.tracks_of == Some(id) {
                model.sync(rows, |old, new| old.video_id == new.video_id);
            } else {
                model.replace(rows);
            }
            drop(model);
            self.tracks_of = Some(id);
        }

        fn load(&mut self, id: TabId, source: TabSource) {
            match source {
                // The first page only; the next ones come with scrolling.
                TabSource::Home => self.load_with(
                    tr("Home did not load"),
                    |c| async move { c.home().await },
                    move |b, page| {
                        let rows = shelf_rows(&page.shelves);
                        b.loaded_content(
                            id,
                            Loaded::Shelves {
                                rows,
                                continuation: page.continuation,
                            },
                        );
                    },
                ),
                TabSource::Library => self.load_with(
                    tr("Library did not load"),
                    |c| async move { c.library_playlists().await },
                    move |b, playlists| {
                        let cards = playlists
                            .iter()
                            .map(|card| CardRow {
                                title: card.title.clone(),
                                subtitle: card.subtitle.clone(),
                                target_id: card.playlist_id().to_owned(),
                                thumbnail: card.thumbnail.clone().unwrap_or_default(),
                            })
                            .collect();
                        b.loaded_content(id, Loaded::Cards(cards));
                    },
                ),
                TabSource::Playlist { id: playlist_id } => self.load_with(
                    tr("Playlist did not load"),
                    move |c| {
                        let playlist_id = playlist_id.clone();
                        async move { c.playlist(&playlist_id).await }
                    },
                    move |b, playlist| {
                        b.headers.insert(
                            id,
                            PageHeader {
                                subtitle: joined(&[&playlist.subtitle, &playlist.details]),
                                title: playlist.title,
                                thumbnail: playlist.thumbnail.unwrap_or_default(),
                            },
                        );
                        let items = queue_items(&playlist.tracks);
                        b.loaded_content(
                            id,
                            Loaded::Tracks {
                                items,
                                continuation: playlist.continuation,
                            },
                        );
                    },
                ),
                TabSource::Album { id: album_id } => self.load_with(
                    tr("Album did not load"),
                    move |c| {
                        let album_id = album_id.clone();
                        async move { c.album(&album_id).await }
                    },
                    move |b, album| {
                        b.headers.insert(
                            id,
                            PageHeader {
                                subtitle: joined(&[&names(&album.artists), &album.subtitle]),
                                title: album.title,
                                thumbnail: album.thumbnail.unwrap_or_default(),
                            },
                        );
                        let items = queue_items(&album.tracks);
                        b.loaded_content(
                            id,
                            Loaded::Tracks {
                                items,
                                continuation: None,
                            },
                        );
                    },
                ),
                TabSource::Artist { id: artist_id } => self.load_with(
                    tr("Artist did not load"),
                    move |c| {
                        let artist_id = artist_id.clone();
                        async move { c.artist(&artist_id).await }
                    },
                    move |b, artist| {
                        b.headers.insert(
                            id,
                            PageHeader {
                                title: artist.name,
                                subtitle: tr("Artist").into(),
                                thumbnail: artist.thumbnail.unwrap_or_default(),
                            },
                        );
                        let rows = shelf_rows(&artist.shelves);
                        b.loaded_content(
                            id,
                            Loaded::Shelves {
                                rows,
                                continuation: None,
                            },
                        );
                    },
                ),
                TabSource::Radio { video_id } => self.load_with(
                    tr("Radio did not load"),
                    move |c| {
                        let video_id = video_id.clone();
                        async move { c.radio(&video_id).await }
                    },
                    move |b, queue| {
                        let title = b
                            .playback()
                            .and_then(|p| {
                                p.tabs()
                                    .get(id)
                                    .map(|t| tab_title(crate::i18n::language(), t))
                            })
                            .unwrap_or_default();
                        b.headers.insert(
                            id,
                            PageHeader {
                                title,
                                subtitle: tr("Radio: similar tracks without end").into(),
                                thumbnail: queue
                                    .tracks
                                    .first()
                                    .and_then(|t| t.thumbnail.clone())
                                    .unwrap_or_default(),
                            },
                        );
                        let items = queue_items(&queue.tracks);
                        b.loaded_content(
                            id,
                            Loaded::Tracks {
                                items,
                                continuation: queue.continuation,
                            },
                        );
                        if b.play_when_loaded == Some(id) {
                            b.play_when_loaded = None;
                            b.play_from(id, 0);
                        }
                    },
                ),
                TabSource::Search { query } => {
                    let asked = query.clone();
                    let filter = self.search_filters.get(&id).copied();
                    self.load_with(
                        tr("Search failed"),
                        move |c| {
                            let query = asked.clone();
                            async move { c.search_in(&query, filter).await }
                        },
                        move |b, results| {
                            b.headers.insert(
                                id,
                                PageHeader {
                                    title: format!("«{query}»"),
                                    subtitle: tr("Search results").into(),
                                    thumbnail: String::new(),
                                },
                            );
                            let rows = match filter {
                                Some(filter) => search_page_rows(&results, filter_title(filter)),
                                None => search_rows(&results),
                            };
                            b.loaded_content(
                                id,
                                Loaded::Shelves {
                                    rows,
                                    continuation: results.continuation,
                                },
                            );
                        },
                    )
                }
            }
        }

        fn loaded_content(&mut self, id: TabId, content: Loaded) {
            // Disliked tracks are left out of lists, when the settings say so.
            let content = match content {
                Loaded::Tracks {
                    mut items,
                    continuation,
                } => {
                    if self.playback_settings.hide_disliked {
                        items.retain(|item| item.rated != Rated::Dislike);
                    }
                    if let Some(playback) = self.playback() {
                        playback.heard_ratings(items.iter().map(|item| item.video_id.clone()));
                    }
                    Loaded::Tracks {
                        items,
                        continuation,
                    }
                }
                other => other,
            };
            self.loaded.insert(id, content);
            self.refresh_if_shown(id);
            release_memory();
        }

        /// Loads the next page of the shown shelves (the home page) when it
        /// has one; QML calls it when the end of the page comes near.
        #[qslot]
        pub fn load_more(&mut self) {
            let Some(id) = self.playback().and_then(|p| p.tabs().shown) else {
                return;
            };
            if self.loading_more.is_some() {
                return;
            }
            let Some(Loaded::Shelves {
                continuation: Some(token),
                ..
            }) = self.loaded.get(&id)
            else {
                return;
            };
            let Some(session) = self.session() else {
                return;
            };
            let token = token.clone();
            let search = self
                .playback()
                .and_then(|p| p.tabs().get(id).map(|t| t.source.clone()))
                .is_some_and(|s| matches!(s, TabSource::Search { .. }));
            self.loading_more = Some(id);
            self.spawn(
                async move {
                    session
                        .call(|c| {
                            let token = token.clone();
                            async move {
                                // The next page of a filtered search, or of
                                // the home page.
                                if search {
                                    c.search_next(&token).await.map(|page| {
                                        (search_page_rows(&page, ""), page.continuation)
                                    })
                                } else {
                                    c.home_next(&token)
                                        .await
                                        .map(|page| (shelf_rows(&page.shelves), page.continuation))
                                }
                            }
                        })
                        .await
                        .map_err(|e| e.to_string())
                },
                move |b, result| {
                    b.loading_more = None;
                    let shown = b.playback().and_then(|p| p.tabs().shown) == Some(id);
                    match result {
                        Ok((rows, continuation)) => {
                            let more = continuation.is_some();
                            if let Some(Loaded::Shelves {
                                rows: all,
                                continuation: continuation_kept,
                            }) = b.loaded.get_mut(&id)
                            {
                                all.extend(rows.iter().cloned());
                                *continuation_kept = continuation;
                            }
                            let empty = rows.is_empty();
                            if shown {
                                b.models.shelves.borrow_mut().append(rows);
                                b.set_has_more(more);
                                // A page without shelves does not move the
                                // view, so nothing would ask for the next one.
                                if empty && more {
                                    b.load_more();
                                }
                            }
                            release_memory();
                        }
                        Err(e) => {
                            if shown {
                                b.set_has_more(false);
                            }
                            let failed = if search {
                                "Search did not load further: %1"
                            } else {
                                "Home did not load further: %1"
                            };
                            b.set_status(trf(failed, &[&e]));
                        }
                    }
                },
            );
        }

        fn refresh_if_shown(&mut self, id: TabId) {
            let Some(playback) = self.playback() else {
                return;
            };
            let tabs = playback.tabs();
            if tabs.shown == Some(id)
                && let Some(tab) = tabs.get(id)
            {
                let source = tab.source.clone();
                self.refresh_content(id, source);
            }
        }

        #[qslot]
        pub fn close_tab(&mut self, tab_id: i64) {
            let Some(playback) = self.playback() else {
                return;
            };
            let id = tab_id as TabId;
            let (closed, shown) = playback.edit_tabs(|tabs| (tabs.close(id), tabs.shown));
            if closed {
                self.loaded.remove(&id);
                self.headers.remove(&id);
                if let Some(shown) = shown {
                    self.show_tab(shown as i64);
                } else {
                    self.refresh_tabs();
                }
            }
        }

        #[qslot]
        pub fn move_tab(&mut self, tab_id: i64, to: i32) {
            let Some(playback) = self.playback() else {
                return;
            };
            playback.edit_tabs(|tabs| tabs.move_tab(tab_id as TabId, to.max(0) as usize));
            self.refresh_tabs();
        }

        /// Opens `source` in a tab (the one it already has, if any); `show`
        /// switches to it, else it opens in the background.
        fn open_source(&mut self, title: &str, source: TabSource, show: bool) -> Option<TabId> {
            let playback = self.playback()?;
            let id = playback.edit_tabs(|tabs| tabs.open(title, source, false));
            if show {
                self.show_tab(id as i64);
            } else {
                self.refresh_tabs();
            }
            Some(id)
        }

        /// Searches YouTube Music in a tab of its own (the same query shows
        /// the tab it already has).
        #[qslot]
        pub fn search(&mut self, query: String) {
            let query = query.trim().to_owned();
            if !query.is_empty() {
                // Named from the query when shown, in the language then.
                self.open_source(&query.clone(), TabSource::Search { query }, true);
            }
        }

        /// Narrows the shown search to `filter` (`songs`, `videos`, `albums`,
        /// `artists`) or, empty, shows everything again; the results load
        /// anew. A filtered search goes on as the page is scrolled.
        #[qslot]
        pub fn set_search_filter(&mut self, filter: String) {
            let Some(playback) = self.playback() else {
                return;
            };
            let Some(id) = playback.tabs().shown else {
                return;
            };
            let Some(source @ TabSource::Search { .. }) =
                playback.tabs().get(id).map(|t| t.source.clone())
            else {
                return;
            };
            match ytm::SearchFilter::from_name(&filter) {
                Some(chosen) => self.search_filters.insert(id, chosen),
                None => self.search_filters.remove(&id),
            };
            let name = self
                .search_filters
                .get(&id)
                .map(|f| f.name().to_owned())
                .unwrap_or_default();
            if self.search_filter != name {
                self.search_filter = name;
                self.shown_changed();
            }
            self.loaded.remove(&id);
            if self.loading_more == Some(id) {
                self.loading_more = None;
            }
            self.set_has_more(false);
            self.models.shelves.borrow_mut().replace(Vec::new());
            self.load(id, source);
        }

        /// Asks YouTube Music for queries to suggest for `input`, what the
        /// search field holds; QML asks 200 ms after the last key. An empty
        /// field has none. Only the newest answer counts.
        #[qslot]
        pub fn suggest(&mut self, input: String) {
            if let Some(task) = self.suggest_task.take() {
                task.abort();
            }
            let input = input.trim().to_owned();
            let (Some(session), Some(me), false) =
                (self.session(), self.me.clone(), input.is_empty())
            else {
                self.set_suggestions(Vec::new());
                return;
            };
            let task = self.runtime.spawn(async move {
                let found = session
                    .call(|c| {
                        let input = input.clone();
                        async move { c.search_suggestions(&input).await }
                    })
                    .await
                    .unwrap_or_default();
                me.invoke_mut(move |b| {
                    b.suggest_task = None;
                    b.set_suggestions(found);
                });
            });
            self.suggest_task = Some(task.abort_handle());
        }

        /// Drops the suggestions, such as when the search field is left.
        #[qslot]
        pub fn clear_suggestions(&mut self) {
            if let Some(task) = self.suggest_task.take() {
                task.abort();
            }
            self.set_suggestions(Vec::new());
        }

        fn set_suggestions(&mut self, suggestions: Vec<String>) {
            if self.search_suggestions != suggestions {
                self.search_suggestions = suggestions;
                self.suggestions_changed();
            }
        }

        /// Opens a playlist (or mix) in a tab.
        pub fn open_playlist(&mut self, playlist_id: String, title: String, show: bool) {
            self.open_source(&title, TabSource::Playlist { id: playlist_id }, show);
        }

        /// Opens a card of a page: a playlist, album or artist in its tab, a
        /// track as its radio. `show` switches to the tab (a click), else it
        /// opens in the background (middle click, Ctrl+click).
        #[qslot]
        pub fn open_card(&mut self, kind: String, id: String, title: String, show: bool) {
            let source = match kind.as_str() {
                "playlist" => TabSource::Playlist { id },
                "album" => TabSource::Album { id },
                "artist" => TabSource::Artist { id },
                "track" => return self.open_radio(id, &title, show),
                _ => return,
            };
            self.open_source(&title, source, show);
        }

        /// Opens the radio of a track; shown, it starts playing.
        fn open_radio(&mut self, video_id: String, title: &str, show: bool) {
            let Some(playback) = self.playback() else {
                return;
            };
            let source = TabSource::Radio { video_id };
            // The track's title alone: the tab strip says "Radio: …" in the
            // language of the moment.
            let Some(id) = self.open_source(title, source, show) else {
                return;
            };
            if !show {
                return;
            }
            let tabs = playback.tabs();
            let has_queue = tabs.get(id).is_some_and(|t| !t.queue.is_empty());
            if has_queue {
                if tabs.playing() != Some(id) {
                    self.resume_tab(id as i64);
                }
            } else if self.loaded.contains_key(&id) {
                self.play_from(id, 0);
            } else {
                self.play_when_loaded = Some(id);
            }
        }

        // ---- playback ----------------------------------------------------

        /// Plays the shown tab's list from `index`.
        #[qslot]
        pub fn play_track(&mut self, index: i32) {
            if let Some(id) = self.playback().and_then(|p| p.tabs().shown) {
                self.play_from(id, index.max(0) as usize);
            }
        }

        /// Plays tab `id`'s list from `index`.
        fn play_from(&mut self, id: TabId, index: usize) {
            let Some(playback) = self.playback() else {
                return;
            };
            let tabs = playback.tabs();
            let (items, continuation) = match (tabs.get(id), self.loaded.get(&id)) {
                (Some(tab), _) if !tab.queue.is_empty() => {
                    (tab.queue.items().to_vec(), tab.queue.continuation.clone())
                }
                (
                    _,
                    Some(Loaded::Tracks {
                        items,
                        continuation,
                    }),
                ) => (items.clone(), continuation.clone()),
                _ => return,
            };
            if index >= items.len() {
                return;
            }
            self.set_status(String::new());
            self.set_starting(items[index].video_id.clone());
            self.spawn(
                async move {
                    playback
                        .play_list(id, items, index, continuation)
                        .await
                        .map_err(|e| e.to_string())
                },
                |b, result| {
                    if let Err(e) = result {
                        b.set_starting(String::new());
                        b.set_status(trf("Could not start the track: %1", &[&e]));
                    }
                    b.refresh_tabs();
                },
            );
        }

        /// Lets a tab continue from where it stopped.
        #[qslot]
        pub fn resume_tab(&mut self, tab_id: i64) {
            let Some(playback) = self.playback() else {
                return;
            };
            self.spawn(
                async move {
                    playback
                        .resume(tab_id as TabId)
                        .await
                        .map_err(|e| e.to_string())
                },
                |b, result| {
                    if let Err(e) = result {
                        b.set_status(trf("Could not resume: %1", &[&e]));
                    }
                    b.refresh_tabs();
                },
            );
        }

        #[qslot]
        pub fn toggle_pause(&mut self) {
            if let Some(playback) = self.playback()
                && let Err(e) = playback.toggle_pause()
            {
                self.set_status(e.to_string());
            }
        }

        #[qslot]
        pub fn next(&mut self) {
            let Some(playback) = self.playback() else {
                return;
            };
            self.set_starting("…".into());
            self.spawn(
                async move { playback.next().await.map_err(|e| e.to_string()) },
                |b, r| {
                    if let Err(e) = r {
                        b.set_starting(String::new());
                        b.set_status(e);
                    }
                },
            );
        }

        #[qslot]
        pub fn previous(&mut self) {
            let Some(playback) = self.playback() else {
                return;
            };
            self.spawn(
                async move { playback.previous().await.map_err(|e| e.to_string()) },
                |b, r| {
                    if let Err(e) = r {
                        b.set_status(e);
                    }
                },
            );
        }

        fn set_starting(&mut self, starting: String) {
            if self.starting != starting {
                self.starting = starting;
                self.starting_changed();
            }
        }

        /// Fetches the link of the shown tab's track `index` ahead: the
        /// pointer rests on it, a click may follow.
        #[qslot]
        pub fn prefetch_row(&mut self, index: i32) {
            if let (Some(playback), Some(item)) = (self.playback(), self.shown_item(index)) {
                playback.prefetch(&item.video_id);
            }
        }

        /// Fetches ahead the links of the shown tab's rows `first` to
        /// `last`: they are in view and the list has come to rest.
        #[qslot]
        pub fn prefetch_visible(&mut self, first: i32, last: i32) {
            self.visible_asked = Some((first, last));
            let (Some(playback), Some(rows)) = (self.playback(), self.shown_items(first, last))
            else {
                return;
            };
            playback.prefetch_visible(&rows);
            self.check_ratings(&rows);
        }

        /// The rows QML last asked to fetch ahead (smoke test).
        pub fn visible_asked(&self) -> Option<(i32, i32)> {
            self.visible_asked
        }

        /// The thumbs of a row: `kind` is `like` or `dislike`; the same
        /// rating again takes it back.
        #[qslot]
        pub fn rate_row(&mut self, index: i32, kind: String) {
            let Some(item) = self.shown_item(index) else {
                return;
            };
            let wanted = rated_from_name(&kind);
            let rated = if item.rated == wanted {
                Rated::None
            } else {
                wanted
            };
            self.rate(item.video_id, rated);
        }

        #[qslot]
        pub fn seek(&mut self, seconds: f64) {
            if let Some(playback) = self.playback() {
                let seconds = seconds.max(0.0);
                let _ = playback.seek(seconds);
                self.position = seconds;
                self.position_changed();
                self.seeked(seconds);
            }
        }

        /// Moves by `by` seconds, back when negative (MPRIS's `Seek`); past
        /// the end it goes to the next track.
        pub fn seek_by(&mut self, by: f64) {
            let Some(playback) = self.playback() else {
                return;
            };
            let to = playback.position().unwrap_or(self.position) + by;
            if self.duration > 0.0 && to >= self.duration {
                self.next();
            } else {
                self.seek(to);
            }
        }

        #[qslot]
        pub fn set_volume(&mut self, volume: f64) {
            if let Some(playback) = self.playback()
                && playback.set_volume(volume).is_ok()
            {
                self.volume = volume;
                self.volume_changed();
            }
        }

        /// Turns the volume up or down by `by` percent (the wheel over the
        /// tray icon).
        pub fn change_volume(&mut self, by: f64) {
            self.set_volume((self.volume + by).clamp(0.0, 100.0));
        }

        /// Reads the position from mpv; QML calls it once a second while the
        /// window is visible and something plays.
        #[qslot]
        pub fn refresh_position(&mut self) {
            if let Some(position) = self.playback().and_then(|p| p.position())
                && (position - self.position).abs() >= 0.5
            {
                self.position = position;
                self.position_changed();
            }
        }

        /// The shown tab's rows `first` to `last` (fewer at its end).
        fn shown_items(&self, first: i32, last: i32) -> Option<Vec<QueueItem>> {
            let playback = self.playback()?;
            let tabs = playback.tabs();
            let id = tabs.shown?;
            let range = |items: &[QueueItem]| {
                let first = (first.max(0) as usize).min(items.len());
                let last = ((last.max(-1) + 1) as usize).clamp(first, items.len());
                items[first..last].to_vec()
            };
            match (tabs.get(id), self.loaded.get(&id)) {
                (Some(tab), _) if !tab.queue.is_empty() => Some(range(tab.queue.items())),
                (_, Some(Loaded::Tracks { items, .. })) => Some(range(items)),
                _ => None,
            }
        }

        fn shown_item(&self, index: i32) -> Option<QueueItem> {
            let playback = self.playback()?;
            let tabs = playback.tabs();
            let id = tabs.shown?;
            match (tabs.get(id), self.loaded.get(&id)) {
                (Some(tab), _) if !tab.queue.is_empty() => {
                    tab.queue.items().get(index as usize).cloned()
                }
                (_, Some(Loaded::Tracks { items, .. })) => items.get(index as usize).cloned(),
                _ => None,
            }
        }

        // ---- the track menu ----------------------------------------------

        /// Points the track menu at the shown tab's track `index`.
        #[qslot]
        pub fn menu_for_row(&mut self, index: i32) {
            if let Some(item) = self.shown_item(index) {
                self.set_menu_item(item);
            }
        }

        /// Points the track menu at a track card or row of a page.
        #[qslot]
        pub fn menu_for_card(
            &mut self,
            video_id: String,
            title: String,
            subtitle: String,
            thumbnail: String,
        ) {
            self.set_menu_item(QueueItem {
                video_id,
                title,
                artists: subtitle,
                duration: None,
                thumbnail: (!thumbnail.is_empty()).then_some(thumbnail),
                rated: Rated::None,
            });
        }

        /// Points the track menu at the track that plays.
        #[qslot]
        pub fn menu_for_current(&mut self) {
            if self.now_video_id.is_empty() {
                return;
            }
            let from_queue = self.playback().and_then(|p| {
                let tabs = p.tabs();
                let queue = &tabs.get(tabs.playing()?)?.queue;
                queue.items().get(queue.current_index()?).cloned()
            });
            let item = from_queue
                .filter(|item| item.video_id == self.now_video_id)
                .unwrap_or_else(|| QueueItem {
                    video_id: self.now_video_id.clone(),
                    title: self.now_title.clone(),
                    artists: self.now_artists.clone(),
                    duration: (self.duration > 0.0).then_some(self.duration as u32),
                    thumbnail: (!self.now_thumbnail.is_empty()).then(|| self.now_thumbnail.clone()),
                    rated: rated_from_name(&self.like_status),
                });
            self.set_menu_item(item);
        }

        /// Shows `item` in the menu at once, and its album, artists and
        /// rating when they come: one `next` request tells all three, for any
        /// track, even one from a queue saved without them.
        fn set_menu_item(&mut self, item: QueueItem) {
            self.menu_video_id = item.video_id.clone();
            self.menu_title = item.title.clone();
            self.menu_rating.clear();
            self.menu_album.clear();
            self.menu_album_id.clear();
            self.menu_artists.clear();
            self.menu_artist_ids.clear();
            self.menu_ready = false;
            self.menu_item = Some(item);
            self.menu_changed();
            let Some(session) = self.session() else {
                return;
            };
            let video_id = self.menu_video_id.clone();
            let asked = video_id.clone();
            self.spawn(
                async move {
                    session
                        .call(|c| {
                            let video_id = video_id.clone();
                            async move { c.watch(&video_id, None).await }
                        })
                        .await
                        .map_err(|e| e.to_string())
                },
                move |b, result| {
                    // The menu moved on to another track meanwhile.
                    if b.menu_video_id != asked {
                        return;
                    }
                    match result {
                        Ok(queue) => {
                            if let Some(track) = queue.tracks.iter().find(|t| t.video_id == asked) {
                                if let Some(album) = &track.album
                                    && let Some(id) = &album.id
                                {
                                    b.menu_album = album.name.clone();
                                    b.menu_album_id = id.clone();
                                }
                                for artist in &track.artists {
                                    if let Some(id) = &artist.id {
                                        b.menu_artists.push(artist.name.clone());
                                        b.menu_artist_ids.push(id.clone());
                                    }
                                }
                                if let Some(item) = b.menu_item.as_mut() {
                                    item.duration = item.duration.or(track.duration);
                                    if item.thumbnail.is_none() {
                                        item.thumbnail = track.thumbnail.clone();
                                    }
                                }
                            }
                            if let Some(like) = queue.current_like {
                                b.show_rating(&asked, orbis_core::rated_from(Some(like)));
                            }
                        }
                        Err(e) => b.set_status(trf("Track details did not load: %1", &[&e])),
                    }
                    b.menu_ready = true;
                    b.menu_changed();
                },
            );
        }

        /// Puts the menu's track right after the one that plays; with nothing
        /// playing, it plays now with its radio.
        #[qslot]
        pub fn menu_play_next(&mut self) {
            self.menu_enqueue(true);
        }

        /// Adds the menu's track to the end of the queue that plays; with
        /// nothing playing, it plays now with its radio.
        #[qslot]
        pub fn menu_add_to_queue(&mut self) {
            self.menu_enqueue(false);
        }

        fn menu_enqueue(&mut self, next: bool) {
            let (Some(playback), Some(item)) = (self.playback(), self.menu_item.clone()) else {
                return;
            };
            // A queue goes to the tab that plays; the shown one may be the
            // home page, which has none.
            let Some(playing) = playback.tabs().playing() else {
                self.menu_radio();
                return;
            };
            let title = item.title.clone();
            if next {
                playback.play_next(vec![item], playing);
                self.set_status(trf("“%1” plays next", &[&title]));
            } else {
                playback.add_to_queue(vec![item], playing);
                self.set_status(trf("“%1” is at the end of the queue", &[&title]));
            }
        }

        /// Plays the radio of the menu's track in its tab.
        #[qslot]
        pub fn menu_radio(&mut self) {
            if let Some(item) = self.menu_item.clone() {
                self.open_radio(item.video_id, &item.title, true);
            }
        }

        /// Opens the album of the menu's track in a tab.
        #[qslot]
        pub fn menu_open_album(&mut self) {
            if !self.menu_album_id.is_empty() {
                let id = self.menu_album_id.clone();
                let title = self.menu_album.clone();
                self.open_source(&title, TabSource::Album { id }, true);
            }
        }

        /// Opens artist `index` of the menu's track in a tab.
        #[qslot]
        pub fn menu_open_artist(&mut self, index: i32) {
            let at = index.max(0) as usize;
            if let (Some(id), Some(name)) = (
                self.menu_artist_ids.get(at).cloned(),
                self.menu_artists.get(at).cloned(),
            ) {
                self.open_source(&name, TabSource::Artist { id }, true);
            }
        }

        /// Likes the menu's track, or takes the like back.
        #[qslot]
        pub fn menu_like(&mut self) {
            let rated = if self.menu_rating == "like" {
                Rated::None
            } else {
                Rated::Like
            };
            self.rate(self.menu_video_id.clone(), rated);
        }

        /// Dislikes the menu's track (skipped when it plays and disliked
        /// tracks are), or takes the dislike back.
        #[qslot]
        pub fn menu_dislike(&mut self) {
            let rated = if self.menu_rating == "dislike" {
                Rated::None
            } else {
                Rated::Dislike
            };
            self.rate(self.menu_video_id.clone(), rated);
        }

        /// Loads the playlists the menu's track can be added to.
        #[qslot]
        pub fn menu_load_playlists(&mut self) {
            let Some(session) = self.session() else {
                return;
            };
            let video_id = self.menu_video_id.clone();
            self.models.options.borrow_mut().replace(Vec::new());
            self.spawn(
                async move {
                    session
                        .call(|c| {
                            let video_id = video_id.clone();
                            async move { c.playlist_options(&video_id).await }
                        })
                        .await
                        .map_err(|e| e.to_string())
                },
                |b, result| match result {
                    Ok(options) => {
                        let rows = options
                            .into_iter()
                            .map(|o| CardRow {
                                title: o.title,
                                subtitle: o.subtitle,
                                target_id: o.playlist_id,
                                thumbnail: String::new(),
                            })
                            .collect();
                        b.models.options.borrow_mut().replace(rows);
                    }
                    Err(e) => b.set_status(trf("The playlists did not load: %1", &[&e])),
                },
            );
        }

        /// Adds the menu's track to `playlist_id` (Liked Music: likes it).
        #[qslot]
        pub fn menu_add_to(&mut self, playlist_id: String, title: String) {
            let Some(session) = self.session() else {
                return;
            };
            let video_id = self.menu_video_id.clone();
            let track = self.menu_title.clone();
            let liked = playlist_id == LIKED_MUSIC;
            self.spawn(
                async move {
                    session
                        .call(|c| {
                            let (playlist_id, video_id) = (playlist_id.clone(), video_id.clone());
                            async move { c.add_to_playlist(&playlist_id, &video_id).await }
                        })
                        .await
                        .map(|_| video_id)
                },
                move |b, result| match result {
                    Ok(video_id) => {
                        b.set_status(trf("“%1” added to “%2”", &[&track, &title]));
                        if liked {
                            b.show_rating(&video_id, Rated::Like);
                        }
                        b.forget_library();
                    }
                    // A track already there is refused, mostly without words.
                    Err(CoreError::Api(ytm::Error::Rejected(why))) => {
                        b.set_status(if why.starts_with("status ") {
                            trf(
                                "“%1”: YouTube Music did not take the track; most likely it is there already",
                                &[&title],
                            )
                        } else {
                            format!("“{title}”: {why}")
                        })
                    }
                    Err(e) => b.set_status(trf("Not added to “%1”: %2", &[&title, &e])),
                },
            );
        }

        /// Creates a playlist with the menu's track; `privacy` is `private`,
        /// `unlisted` or `public`.
        #[qslot]
        pub fn menu_create_playlist(&mut self, title: String, privacy: String) {
            let Some(session) = self.session() else {
                return;
            };
            let title = title.trim().to_owned();
            if title.is_empty() {
                return;
            }
            let privacy = match privacy.as_str() {
                "public" => Privacy::Public,
                "unlisted" => Privacy::Unlisted,
                _ => Privacy::Private,
            };
            let video_id = self.menu_video_id.clone();
            let track = self.menu_title.clone();
            let name = title.clone();
            self.spawn(
                async move {
                    session
                        .call(|c| {
                            let (title, video_id) = (title.clone(), video_id.clone());
                            async move {
                                c.create_playlist(&title, "", privacy, &[&video_id]).await
                            }
                        })
                        .await
                        .map_err(|e| e.to_string())
                },
                move |b, result| match result {
                    Ok(_) => {
                        b.set_status(trf("Created the playlist “%1” with “%2”", &[&name, &track]));
                        b.forget_library();
                    }
                    Err(e) => {
                        b.set_status(trf("The playlist “%1” was not created: %2", &[&name, &e]))
                    }
                },
            );
        }

        /// Drops the loaded library, so it shows new and changed playlists
        /// next time.
        fn forget_library(&mut self) {
            let Some(playback) = self.playback() else {
                return;
            };
            let library = playback
                .tabs()
                .tabs()
                .iter()
                .find(|t| t.source == TabSource::Library)
                .map(|t| t.id);
            if let Some(id) = library {
                self.loaded.remove(&id);
                self.refresh_if_shown(id);
            }
        }

        // ---- ratings -----------------------------------------------------

        /// Rates `video_id`. The rating shows at once everywhere, and goes
        /// back when YouTube Music does not take it. A disliked track that
        /// plays is skipped when disliked tracks are.
        fn rate(&mut self, video_id: String, rated: Rated) {
            let Some(session) = self.session() else {
                return;
            };
            if video_id.is_empty() {
                return;
            }
            let before = self.known_rating(&video_id);
            if let Some(playback) = self.playback() {
                playback.rating_given(&video_id);
            }
            self.show_rating(&video_id, rated);
            if rated == Rated::Dislike
                && video_id == self.now_video_id
                && self.playback_settings.skip_disliked
            {
                self.next();
            }
            let rating = rating_of(rated);
            self.spawn(
                async move {
                    session
                        .call(|c| {
                            let video_id = video_id.clone();
                            async move { c.rate(&video_id, rating).await }
                        })
                        .await
                        .map_err(|e| (video_id, e.to_string()))
                },
                move |b, result| {
                    if let Err((video_id, e)) = result {
                        b.show_rating(&video_id, before);
                        b.set_status(trf("The rating was not saved: %1", &[&e]));
                    }
                },
            );
        }

        /// The rating of `video_id` as shown now.
        fn known_rating(&self, video_id: &str) -> Rated {
            if self.now_video_id == video_id {
                return rated_from_name(&self.like_status);
            }
            if self.menu_video_id == video_id && self.menu_ready {
                return rated_from_name(&self.menu_rating);
            }
            self.models
                .tracks
                .borrow()
                .rows()
                .iter()
                .find(|row| row.video_id == video_id)
                .map(|row| rated_from_name(&row.rating))
                .unwrap_or_default()
        }

        /// Shows `video_id`'s rating in the player bar, the menu, the lists
        /// and the queues (where "next" reads it).
        fn show_rating(&mut self, video_id: &str, rated: Rated) {
            self.show_ratings(&[(video_id.to_owned(), rated)]);
        }

        /// [`Backend::show_rating`] for several tracks, with one save of
        /// the queues and one refresh of the list.
        fn show_ratings(&mut self, ratings: &[(String, Rated)]) {
            if ratings.is_empty() {
                return;
            }
            for (video_id, rated) in ratings {
                let name = rated_name(*rated);
                if self.now_video_id == *video_id && self.like_status != name {
                    self.like_status = name.into();
                    self.like_changed();
                }
                if self.menu_video_id == *video_id && self.menu_rating != name {
                    self.menu_rating = name.into();
                    self.menu_changed();
                }
                if let Some(item) = self.menu_item.as_mut()
                    && item.video_id == *video_id
                {
                    item.rated = *rated;
                }
                for loaded in self.loaded.values_mut() {
                    if let Loaded::Tracks { items, .. } = loaded {
                        for item in items.iter_mut().filter(|i| i.video_id == *video_id) {
                            item.rated = *rated;
                        }
                    }
                }
            }
            if let Some(playback) = self.playback() {
                playback.set_ratings(ratings);
                if let Some(shown) = playback.tabs().shown {
                    self.refresh_if_shown(shown);
                }
            }
        }

        /// Asks YouTube Music how the account rated the rows in view that
        /// orbis has not heard about since the start, a few at a time, and
        /// shows what changed. A queue restored from the last run keeps the
        /// ratings of the day it was loaded; a track disliked since on the
        /// phone would look unrated until it plays.
        fn check_ratings(&mut self, rows: &[QueueItem]) {
            let (Some(session), Some(playback)) = (self.session(), self.playback()) else {
                return;
            };
            let wanted = playback.unheard_ratings(rows.iter().map(|row| row.video_id.clone()));
            if wanted.is_empty() {
                return;
            }
            self.spawn(
                async move {
                    let permits = Arc::new(tokio::sync::Semaphore::new(RATING_CHECKS_AT_ONCE));
                    let mut asked = tokio::task::JoinSet::new();
                    for video_id in wanted {
                        let session = Arc::clone(&session);
                        let permits = Arc::clone(&permits);
                        asked.spawn(async move {
                            let _permit = permits.acquire_owned().await;
                            let like = session
                                .call(|c| {
                                    let video_id = video_id.clone();
                                    async move { c.watch(&video_id, None).await }
                                })
                                .await
                                .map(|queue| queue.current_like);
                            (video_id, like)
                        });
                    }
                    let mut answers = Vec::new();
                    while let Some(answer) = asked.join_next().await {
                        if let Ok(answer) = answer {
                            answers.push(answer);
                        }
                    }
                    answers
                },
                move |b, answers| {
                    // The queues and the list change only where they differ.
                    let mut changed = Vec::new();
                    for (video_id, like) in answers {
                        match like {
                            Ok(Some(like)) if !playback.rating_was_given(&video_id) => {
                                changed.push((video_id, orbis_core::rated_from(Some(like))));
                            }
                            Ok(_) => {}
                            // Asked again when the row is in view again.
                            Err(_) => playback.rating_not_heard(&video_id),
                        }
                    }
                    b.show_ratings(&changed);
                },
            );
        }

        /// Likes the current track, or takes the like back.
        #[qslot]
        pub fn like(&mut self) {
            let rated = if self.like_status == "like" {
                Rated::None
            } else {
                Rated::Like
            };
            self.rate(self.now_video_id.clone(), rated);
        }

        /// Dislikes the current track (skipped when disliked tracks are), or
        /// takes the dislike back.
        #[qslot]
        pub fn dislike(&mut self) {
            let rated = if self.like_status == "dislike" {
                Rated::None
            } else {
                Rated::Dislike
            };
            self.rate(self.now_video_id.clone(), rated);
        }

        // ---- events from the playback service -----------------------------

        fn on_playback_event(&mut self, event: PlaybackEvent) {
            match event {
                PlaybackEvent::NowPlaying {
                    tab,
                    item,
                    duration,
                } => {
                    self.now_title = item.title.clone();
                    self.now_artists = item.artists.clone();
                    self.now_thumbnail = item.thumbnail.clone().unwrap_or_default();
                    self.now_video_id = item.video_id.clone();
                    self.duration = duration.unwrap_or_default();
                    self.now_changed();
                    self.position = 0.0;
                    self.position_changed();
                    if !self.playing {
                        self.playing = true;
                        self.playing_changed();
                    }
                    self.set_starting(String::new());
                    self.like_status = rated_name(item.rated).into();
                    self.like_changed();
                    self.refresh_tabs();
                    self.refresh_if_shown(tab);
                    self.fetch_rating(item.video_id);
                }
                // mpv tells its pause state at the start too, before any
                // track: nothing plays then.
                PlaybackEvent::Paused(paused) => {
                    if self.playing == paused && (paused || !self.now_video_id.is_empty()) {
                        self.playing = !paused;
                        self.playing_changed();
                    }
                }
                PlaybackEvent::Stopped => {
                    self.set_starting(String::new());
                    self.playing = false;
                    self.playing_changed();
                    self.refresh_tabs();
                }
                PlaybackEvent::QueueChanged { tab } => self.refresh_if_shown(tab),
                // The queues have it already; the list, the bar and the menu
                // follow.
                PlaybackEvent::Rated { video_id, rated } => {
                    self.show_ratings(&[(video_id, rated)]);
                }
                PlaybackEvent::Error(failure) => {
                    self.set_starting(String::new());
                    let tabs = self.playback().map(|p| p.tabs());
                    let text = failure_text(crate::i18n::language(), &failure, |id| {
                        tabs.as_ref()?
                            .tabs()
                            .iter()
                            .flat_map(|tab| tab.queue.items())
                            .find(|item| item.video_id == id)
                            .map(|item| item.title.clone())
                    });
                    self.set_status(text)
                }
            }
        }

        fn fetch_rating(&mut self, video_id: String) {
            let Some(session) = self.session() else {
                return;
            };
            let asked = video_id.clone();
            self.spawn(
                async move {
                    session
                        .call(|c| {
                            let video_id = video_id.clone();
                            async move { c.watch(&video_id, None).await }
                        })
                        .await
                        .ok()
                        .and_then(|q| q.current_like)
                },
                move |b, like| {
                    if b.now_video_id == asked
                        && let Some(like) = like
                    {
                        b.show_rating(&asked, orbis_core::rated_from(Some(like)));
                    }
                },
            );
        }

        /// Takes the services out for an orderly shutdown: dropping them stops
        /// mpv, which must happen before the process exits.
        pub fn take_services(&mut self) -> Option<Services> {
            self.services.take()
        }

        /// Asks QML to save a picture of the window to `path`.
        pub fn request_screenshot(&mut self, path: String) {
            self.screenshot_requested(path);
        }

        /// Asks QML to open or close the downloads list.
        pub fn request_downloads_list(&mut self, open: bool) {
            self.downloads_list_requested(open);
        }

        /// Asks QML to open a window made on first use (smoke test).
        pub fn request_window(&mut self, name: String) {
            self.window_requested(name);
        }

        /// Asks QML to type into the search field (smoke test).
        pub fn request_typing(&mut self, text: String) {
            self.typing_requested(text);
        }

        /// Asks QML to choose a suggestion (smoke test).
        pub fn request_suggestion(&mut self, index: i32) {
            self.suggestion_requested(index);
        }

        /// QML tells whether the window `name` it was asked to open (or the
        /// downloads list) is shown (smoke test).
        #[qslot]
        pub fn smoke_window_shown(&mut self, name: String, shown: bool) {
            self.smoke_window = if shown { name } else { String::new() };
        }

        /// Asks QML to quit.
        pub fn request_quit(&mut self) {
            self.quit_requested();
        }

        /// What the smoke test checks.
        pub fn smoke_state(&self) -> SmokeState {
            let mut state = SmokeState {
                menu_video_id: self.menu_video_id.clone(),
                menu_title: self.menu_title.clone(),
                menu_album: self.menu_album.clone(),
                menu_artists: self.menu_artists.clone(),
                menu_rating: self.menu_rating.clone(),
                menu_ready: self.menu_ready,
                shown_kind: self.shown_kind.clone(),
                status: self.status.clone(),
                signed_in: self.signed_in,
                signing_in: self.signing_in,
                sign_in_error: self.sign_in_error.clone(),
                downloads: self
                    .models
                    .downloads
                    .borrow()
                    .rows()
                    .iter()
                    .map(|r| r.stage.clone())
                    .collect(),
                downloads_active: self.downloads_active,
                shown_downloadable: self.shown_downloadable,
                download_reading: self.download_reading,
                search_filter: self.search_filter.clone(),
                suggestions: self.search_suggestions.clone(),
                shelf_cards: self
                    .models
                    .shelves
                    .borrow()
                    .rows()
                    .iter()
                    .map(|r| r.cards.len())
                    .sum(),
                download_ask: self.download_ask,
                window_shown: self.smoke_window.clone(),
                ..SmokeState::default()
            };
            if let Some(queue) = &self.downloads {
                state.download_format = queue.format().extension().to_owned();
                state.download_root = queue.root().to_string_lossy().into_owned();
            }
            if let Some(playback) = self.playback() {
                state.quality = playback.quality().name().to_owned();
                state.normalize = playback.normalizes();
                let tabs = playback.tabs();
                if let Some(tab) = tabs.playing().and_then(|id| tabs.get(id)) {
                    state.playing_kind = kind_of(&tab.source).into();
                    let upcoming: Vec<&QueueItem> = tab.queue.upcoming().collect();
                    state.next = upcoming.first().map(|i| i.title.clone());
                    state.last = upcoming.last().map(|i| i.title.clone());
                }
            }
            state
        }

        /// The window was shown or hidden in the tray (the shell tells).
        pub fn set_window_shown(&mut self, shown: bool) {
            self.window_shown = shown;
            if let Some(playback) = self.playback() {
                playback.set_window_shown(shown);
            }
        }

        /// The process ID of playback's long-lived yt-dlp (smoke test).
        pub fn worker_pid(&self) -> Option<u32> {
            self.playback().and_then(|p| p.worker_pid())
        }

        /// How many of the shown tab's rows `first` to `last` have their
        /// link at hand (smoke test).
        pub fn links_ready(&self, first: i32, last: i32) -> usize {
            let (Some(playback), Some(rows)) = (self.playback(), self.shown_items(first, last))
            else {
                return 0;
            };
            rows.iter()
                .filter(|r| playback.has_link(&r.video_id))
                .count()
        }

        /// How long the last next or previous track took to sound (smoke
        /// test).
        pub fn last_switch(&self) -> Option<std::time::Duration> {
            self.playback().and_then(|p| p.last_switch())
        }

        /// Asks QML to scroll the shown shelves to the end; `listener` then
        /// gets the number of shelves and cards (smoke test).
        pub fn request_scroll(&mut self, listener: UnboundedSender<(usize, usize)>) {
            self.smoke_listener = Some(listener);
            self.scroll_requested();
        }

        /// Asks QML where the shown shelves are scrolled, first moving them
        /// a little into shelf `shelf` unless it is -1; `listener` then gets
        /// it (smoke test).
        pub fn request_place(&mut self, shelf: i32, listener: UnboundedSender<ShelvesPlace>) {
            self.place_listener = Some(listener);
            self.place_requested(shelf);
        }

        /// QML tells where the shown shelves are scrolled (smoke test).
        #[qslot]
        pub fn smoke_place(&mut self, shelf: i32, offset: f64, at_end: bool) {
            if let Some(listener) = self.place_listener.take() {
                let _ = listener.send(ShelvesPlace {
                    shelf,
                    offset,
                    at_end,
                });
            }
        }

        /// Writes a line from QML to the network log, to place UI events
        /// among the requests (smoke test).
        #[qslot]
        pub fn smoke_note(&mut self, text: String) {
            ytm::netlog::log(format_args!("ui: {text}"));
        }

        /// QML scrolled the shown shelves to the end (smoke test).
        #[qslot]
        pub fn smoke_scrolled(&mut self) {
            let id = self.playback().and_then(|p| p.tabs().shown);
            let counts = match id.and_then(|id| self.loaded.get(&id)) {
                Some(Loaded::Shelves { rows, .. }) => {
                    (rows.len(), rows.iter().map(|r| r.cards.len()).sum())
                }
                _ => (0, 0),
            };
            if let Some(listener) = self.smoke_listener.take() {
                let _ = listener.send(counts);
            }
        }

        /// Opens the first `count` library cards in background tabs (smoke test).
        pub fn smoke_open_library_cards(&mut self, count: usize) {
            let cards: Vec<CardRow> = self
                .models
                .cards
                .borrow()
                .rows()
                .iter()
                .take(count)
                .cloned()
                .collect();
            for card in cards {
                self.open_playlist(card.target_id, card.title, false);
            }
        }

        /// The tab to show at start: the one shown last time, else the first.
        pub fn initial_tab(&self) -> Option<i64> {
            let tabs = self.playback()?.tabs();
            tabs.shown
                .or_else(|| tabs.tabs().first().map(|t| t.id))
                .map(|id| id as i64)
        }

        /// The tab IDs, for the smoke test.
        pub fn tab_ids(&self) -> Vec<(TabId, TabSource)> {
            self.playback()
                .map(|p| {
                    p.tabs()
                        .tabs()
                        .iter()
                        .map(|t| (t.id, t.source.clone()))
                        .collect()
                })
                .unwrap_or_default()
        }
    }
}

pub use backend::Backend;

#[cfg(test)]
mod tests {
    use super::*;
    use orbis_core::PlaybackFailure;

    #[test]
    fn every_playback_failure_is_worded_in_each_language() {
        let failures = [
            PlaybackFailure::Play {
                video_id: Some("abc".into()),
                cause: "HTTP 403".into(),
            },
            PlaybackFailure::Play {
                video_id: None,
                cause: "HTTP 403".into(),
            },
            PlaybackFailure::SaveTabs("disk full".into()),
            PlaybackFailure::PrepareNext("timed out".into()),
            PlaybackFailure::LoadMore("timed out".into()),
        ];
        let title = |id: &str| (id == "abc").then(|| "Song".to_owned());
        for failure in &failures {
            let words = [
                (Language::English, "Cannot"),
                (Language::Ukrainian, "Не вдалося"),
            ];
            for (language, start) in words {
                let text = failure_text(language, failure, title);
                assert!(text.starts_with(start), "{language:?}: {text}");
                assert!(!text.contains('%'), "{language:?}: {text}");
            }
        }
        assert_eq!(
            failure_text(Language::Ukrainian, &failures[0], title),
            "Не вдалося відтворити «Song»: HTTP 403"
        );
        assert_eq!(
            failure_text(Language::English, &failures[0], |_| None),
            "Cannot play “abc”: HTTP 403"
        );
    }

    fn tab(title: &str, source: TabSource) -> player::Tab {
        player::Tab {
            id: 1,
            title: title.into(),
            source,
            pinned: false,
            queue: Default::default(),
        }
    }

    #[test]
    fn search_and_radio_tabs_are_named_in_the_language_of_the_moment() {
        let search = tab(
            "Search: Metallica",
            TabSource::Search {
                query: "Metallica".into(),
            },
        );
        assert_eq!(tab_title(Language::English, &search), "Search: Metallica");
        assert_eq!(tab_title(Language::Ukrainian, &search), "Пошук: Metallica");

        let radio = |title: &str| {
            tab(
                title,
                TabSource::Radio {
                    video_id: "abc".into(),
                },
            )
        };
        // Opened now: the track's title alone.
        assert_eq!(
            tab_title(Language::Ukrainian, &radio("Song")),
            "Радіо: Song"
        );
        // Saved by an earlier version, in whatever language it was then.
        for old in ["Radio: Song", "Радіо: Song"] {
            assert_eq!(tab_title(Language::Ukrainian, &radio(old)), "Радіо: Song");
            assert_eq!(tab_title(Language::English, &radio(old)), "Radio: Song");
        }

        let album = tab("Ride the Lightning", TabSource::Album { id: "x".into() });
        assert_eq!(tab_title(Language::Ukrainian, &album), "Ride the Lightning");
        let home = tab("", TabSource::Home);
        assert_eq!(
            tab_title(Language::Ukrainian, &home),
            crate::i18n::tr_in(Language::Ukrainian, "Home")
        );
    }
}
