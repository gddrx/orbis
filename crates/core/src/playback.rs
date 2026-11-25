//! The playback service: tabs and their queues, the stream cache and mpv,
//! driven by commands from the UI and by mpv's events.
//!
//! The next track of the playing tab is resolved and appended to mpv while
//! the current one plays, and the link of the one after it is fetched too;
//! when mpv switches to the next track, the tab's queue advances. A queue
//! that runs low loads the next page of its source (a mix, a long playlist, a
//! radio). Tracks the account disliked are skipped and left out of new pages
//! when the settings say so. A track whose link fails is tried once more with
//! a new link, then skipped. Tabs are saved to SQLite after every change.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use player::{
    EndReason, Engine, EngineOptions, Media, PlayRequest, PlayerEvent, QueueItem, Rated, Store,
    TabId, TabSource, Tabs, normalization_gain,
};
use stream::{StreamCache, YtDlp};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use ytm::Track;

use crate::session::SessionManager;
use crate::settings::Quality;

/// Loudness target of normalization, as YouTube's web player.
const TARGET_LKFS: f64 = -14.0;
/// Load the next page when fewer tracks than this remain.
const LOW_WATER: usize = 5;
/// "Previous" restarts the track when it played longer than this.
const RESTART_AFTER: f64 = 5.0;
/// Links fetched ahead at once for tracks the user may click (each is a
/// yt-dlp run of ~2 s of CPU).
const PREFETCHES: usize = 2;
/// The long-lived yt-dlp starts this long after the window is shown: not
/// while orbis itself starts. A link asked for sooner starts it at once.
const WARM_UP_AFTER: std::time::Duration = std::time::Duration::from_secs(3);

/// What the UI hears about.
#[derive(Debug, Clone, PartialEq)]
pub enum PlaybackEvent {
    NowPlaying {
        tab: TabId,
        item: QueueItem,
        duration: Option<f64>,
    },
    Paused(bool),
    /// The playing tab's queue is over.
    Stopped,
    /// A tab's queue changed (tracks added or loaded).
    QueueChanged {
        tab: TabId,
    },
    /// YouTube Music rates `video_id` otherwise than the queues had it
    /// (rated elsewhere since the queue was loaded); the queues have it now.
    Rated {
        video_id: String,
        rated: Rated,
    },
    /// Something failed that the user should know about.
    Error(PlaybackFailure),
}

/// What failed in playback: the kind and the cause, so that the window
/// words it in its own language; `Display` gives it in English.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaybackFailure {
    /// A track did not play: its video ID when the player told it.
    Play {
        video_id: Option<String>,
        cause: String,
    },
    /// The tabs could not be saved.
    SaveTabs(String),
    /// The next track could not be prepared ahead.
    PrepareNext(String),
    /// The next page of a queue did not load.
    LoadMore(String),
}

impl std::fmt::Display for PlaybackFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Play {
                video_id: Some(id),
                cause,
            } => write!(f, "cannot play {id}: {cause}"),
            Self::Play {
                video_id: None,
                cause,
            } => write!(f, "cannot play: {cause}"),
            Self::SaveTabs(cause) => write!(f, "cannot save tabs: {cause}"),
            Self::PrepareNext(cause) => write!(f, "cannot prepare the next track: {cause}"),
            Self::LoadMore(cause) => write!(f, "cannot load more tracks: {cause}"),
        }
    }
}

/// How orbis keeps an API track's rating.
pub fn rated_from(like: Option<ytm::LikeStatus>) -> Rated {
    match like {
        Some(ytm::LikeStatus::Like) => Rated::Like,
        Some(ytm::LikeStatus::Dislike) => Rated::Dislike,
        _ => Rated::None,
    }
}

/// A queue item from an API track.
pub fn item_from_track(track: &Track) -> QueueItem {
    QueueItem {
        rated: rated_from(track.like),
        video_id: track.video_id.clone(),
        title: track.title.clone(),
        artists: track
            .artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        duration: track.duration,
        thumbnail: track.thumbnail.clone(),
    }
}

struct State {
    tabs: Tabs,
    store: Store,
    /// Measured loudness by video ID.
    loudness: HashMap<String, Option<f64>>,
    /// Video ID appended to mpv after the current one, if any.
    enqueued: Option<String>,
    /// Counts the tracks started, so that work for an older one is dropped.
    started: u64,
    /// When the user last asked for the next or the previous track, until
    /// its audio starts; and how long that took the last time.
    switch_asked: Option<std::time::Instant>,
    last_switch: Option<std::time::Duration>,
    /// The track mpv failed to open and that is being tried again with a
    /// new link; a second failure skips it.
    retried: Option<String>,
    /// The rows in view whose links are still to fetch, and their limits.
    visible: crate::ahead::Visible,
    /// Tracks whose rating is as YouTube Music has it now: on a page loaded
    /// since the start, asked for, or being asked. A queue from the last run
    /// keeps the ratings of the day it was loaded.
    heard: HashSet<String>,
    /// Tracks rated in orbis since the start: an answer asked for before
    /// the rating must not take it back.
    given: HashSet<String>,
}

pub struct Playback {
    session: Arc<SessionManager>,
    /// Resolved links in the chosen quality; replaced when it changes.
    cache: RwLock<(Quality, Arc<StreamCache<YtDlp>>)>,
    /// Loud tracks are turned down.
    normalize: AtomicBool,
    /// "Next" and the end of a track go past disliked tracks.
    skip_disliked: AtomicBool,
    /// New pages of a queue come without disliked tracks.
    hide_disliked: AtomicBool,
    /// The window is shown: links may come from a long-lived yt-dlp.
    window_shown: AtomicBool,
    /// Room for links fetched ahead of a click.
    prefetches: Arc<tokio::sync::Semaphore>,
    /// Where work asked for from the GUI thread runs.
    runtime: tokio::runtime::Handle,
    engine: Engine,
    state: Mutex<State>,
    events: UnboundedSender<PlaybackEvent>,
    /// The tabs were made at this start, not read from the store.
    first_start: bool,
}

/// A cache of links resolved by yt-dlp with the session's cookies, in
/// `quality`. The links are kept in `<cache>/streams.db`: the first tracks
/// after a restart play without yt-dlp while their links last.
fn stream_cache(session: &Arc<SessionManager>, quality: Quality) -> Arc<StreamCache<YtDlp>> {
    let session = Arc::clone(session);
    StreamCache::with_disk(
        crate::yt_dlp()
            .with_format(quality.ytdlp_format())
            .with_cookies(Arc::new(move || Some(session.cookies()))),
        &desktop::cache_dir().join("streams.db"),
        quality.ytdlp_format(),
    )
}

impl std::fmt::Debug for Playback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Playback").finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PlaybackError {
    #[error(transparent)]
    Engine(#[from] player::MpvError),
    #[error(transparent)]
    Store(#[from] player::StoreError),
    #[error(transparent)]
    Stream(#[from] stream::StreamError),
    #[error(transparent)]
    Core(#[from] crate::session::CoreError),
}

impl Playback {
    /// Starts the service with the tabs stored at `state_path` (or the
    /// defaults with `mixes` on the first start). `audio_output` is mpv's
    /// `ao` (`Some("null")` for silent runs). Must run inside a tokio runtime.
    pub fn start(
        session: Arc<SessionManager>,
        state_path: &Path,
        mixes: (Option<&str>, Option<&str>),
        audio_output: Option<String>,
    ) -> Result<(Arc<Self>, UnboundedReceiver<PlaybackEvent>), PlaybackError> {
        let store = Store::open(state_path)?;
        let stored = store.load()?;
        let first_start = stored.is_none();
        let tabs = stored.unwrap_or_else(|| Tabs::with_defaults(mixes.0, mixes.1));
        let cache = stream_cache(&session, Quality::Best);
        let (engine, mut engine_events) = Engine::new(&EngineOptions {
            audio_output,
            ..Default::default()
        })?;
        let (sender, receiver) = unbounded_channel();
        let playback = Arc::new(Self {
            session,
            cache: RwLock::new((Quality::Best, cache)),
            normalize: AtomicBool::new(true),
            skip_disliked: AtomicBool::new(true),
            hide_disliked: AtomicBool::new(false),
            window_shown: AtomicBool::new(false),
            prefetches: Arc::new(tokio::sync::Semaphore::new(PREFETCHES)),
            runtime: tokio::runtime::Handle::current(),
            engine,
            state: Mutex::new(State {
                tabs,
                store,
                loudness: HashMap::new(),
                enqueued: None,
                started: 0,
                switch_asked: None,
                last_switch: None,
                retried: None,
                visible: crate::ahead::Visible::new(
                    std::env::var("ORBIS_PREFETCH_VISIBLE").as_deref() != Ok("0"),
                ),
                heard: HashSet::new(),
                given: HashSet::new(),
            }),
            events: sender,
            first_start,
        });
        let weak = Arc::downgrade(&playback);
        tokio::spawn(async move {
            while let Some(event) = engine_events.recv().await {
                let Some(playback) = weak.upgrade() else {
                    break;
                };
                playback.on_engine_event(event);
            }
        });
        Ok((playback, receiver))
    }

    fn cache(&self) -> Arc<StreamCache<YtDlp>> {
        Arc::clone(&self.cache.read().expect("stream cache lock").1)
    }

    /// The quality of the streams played.
    pub fn quality(&self) -> Quality {
        self.cache.read().expect("stream cache lock").0
    }

    /// Plays streams of `quality` from the next track on; the links
    /// resolved so far are forgotten.
    pub fn set_quality(&self, quality: Quality) {
        let mut cache = self.cache.write().expect("stream cache lock");
        if cache.0 != quality {
            *cache = (quality, stream_cache(&self.session, quality));
            self.follow_window(&cache.1);
        }
    }

    /// Notes whether the window is shown. While it is, links come from a
    /// long-lived yt-dlp, about a second sooner, for some 90–100 MB; in the tray
    /// it stops: the next track's link has a whole track to come.
    pub fn set_window_shown(&self, shown: bool) {
        self.window_shown.store(shown, Ordering::SeqCst);
        self.follow_window(&self.cache());
    }

    /// Lets `cache`'s yt-dlp keep running while the window is shown, and
    /// starts it a moment after.
    fn follow_window(&self, cache: &Arc<StreamCache<YtDlp>>) {
        let shown = self.window_shown.load(Ordering::SeqCst);
        cache.resolver().allow_worker(shown);
        if shown {
            let cache = Arc::clone(cache);
            self.runtime.spawn(async move {
                tokio::time::sleep(WARM_UP_AFTER).await;
                cache.resolver().warm_up().await;
            });
        }
    }

    /// The process ID of the long-lived yt-dlp, while it runs (smoke test).
    pub fn worker_pid(&self) -> Option<u32> {
        self.cache().resolver().worker_pid()
    }

    /// Whether the link of `video_id` is at hand (smoke test).
    pub fn has_link(&self, video_id: &str) -> bool {
        self.cache().has(video_id)
    }

    pub fn normalizes(&self) -> bool {
        self.normalize.load(Ordering::Relaxed)
    }

    /// Turns loudness correction on or off, for the current track at once.
    pub fn set_normalize(&self, on: bool) -> Result<(), PlaybackError> {
        if self.normalize.swap(on, Ordering::Relaxed) == on {
            return Ok(());
        }
        let gain = {
            let mut state = self.state();
            let current = state
                .tabs
                .playing_queue_mut()
                .and_then(|q| q.current().map(|i| i.video_id.clone()));
            let loudness = current.and_then(|id| state.loudness.get(&id).copied().flatten());
            self.gain(loudness).unwrap_or(0.0)
        };
        Ok(self.engine.set_gain(gain)?)
    }

    /// Whether "next" goes past disliked tracks.
    pub fn skips_disliked(&self) -> bool {
        self.skip_disliked.load(Ordering::Relaxed)
    }

    /// Lets "next" and the end of a track go past disliked tracks, or not.
    pub fn set_skip_disliked(&self, on: bool) {
        self.skip_disliked.store(on, Ordering::Relaxed);
    }

    /// Whether lists load without disliked tracks.
    pub fn hides_disliked(&self) -> bool {
        self.hide_disliked.load(Ordering::Relaxed)
    }

    /// Lets new pages of queues come without disliked tracks, or with them.
    pub fn set_hide_disliked(&self, on: bool) {
        self.hide_disliked.store(on, Ordering::Relaxed);
    }

    /// Which tracks "next" goes past now.
    fn skip(&self) -> impl Fn(&QueueItem) -> bool + use<> {
        let on = self.skips_disliked();
        move |item: &QueueItem| on && item.rated == Rated::Dislike
    }

    /// Notes that the account rated `video_id` (in orbis or elsewhere): the
    /// queues keep it, and a disliked track that waits in mpv as the next one
    /// is taken back when such tracks are skipped.
    pub fn set_rating(self: &Arc<Self>, video_id: &str, rated: Rated) {
        self.set_ratings(&[(video_id.to_owned(), rated)]);
    }

    /// [`Playback::set_rating`] for several tracks, saved once.
    pub fn set_ratings(self: &Arc<Self>, ratings: &[(String, Rated)]) {
        let (changed, requeue) = {
            let mut state = self.state();
            let mut changed = false;
            let mut requeue = false;
            for (video_id, rated) in ratings {
                if state.tabs.set_rated(video_id, *rated).is_empty() {
                    continue;
                }
                changed = true;
                if *rated == Rated::Dislike
                    && self.skips_disliked()
                    && state.enqueued.as_deref() == Some(video_id.as_str())
                {
                    state.enqueued = None;
                    requeue = true;
                }
            }
            (changed, requeue.then_some(state.started))
        };
        if !changed {
            return;
        }
        self.save();
        if let Some(started) = requeue {
            let _ = self.engine.clear_enqueued();
            let me = Arc::clone(self);
            self.runtime
                .spawn(async move { me.enqueue_next(started).await });
        }
    }

    /// Of `video_ids`, the tracks whose rating orbis has not heard since the
    /// start; they count as heard from now on, while they are asked for.
    pub fn unheard_ratings(&self, video_ids: impl IntoIterator<Item = String>) -> Vec<String> {
        let mut state = self.state();
        video_ids
            .into_iter()
            .filter(|id| state.heard.insert(id.clone()))
            .collect()
    }

    /// The ratings of `video_ids` are as YouTube Music has them now: the
    /// tracks came on a page just loaded.
    pub fn heard_ratings(&self, video_ids: impl IntoIterator<Item = String>) {
        self.state().heard.extend(video_ids);
    }

    /// The rating of `video_id` did not come: it is asked again later.
    pub fn rating_not_heard(&self, video_id: &str) {
        self.state().heard.remove(video_id);
    }

    /// The user rated `video_id` in orbis.
    pub fn rating_given(&self, video_id: &str) {
        let mut state = self.state();
        state.given.insert(video_id.to_owned());
        state.heard.insert(video_id.to_owned());
    }

    /// Whether the user rated `video_id` in orbis since the start: an
    /// answer asked for before must not take the rating back.
    pub fn rating_was_given(&self, video_id: &str) -> bool {
        self.state().given.contains(video_id)
    }

    /// Forgets the ratings heard and given: another account signs in.
    pub fn forget_ratings(&self) {
        let mut state = self.state();
        state.heard.clear();
        state.given.clear();
    }

    /// Asks YouTube Music how the account rates `video_id`, unless orbis
    /// heard it since the start; the queues take an answer that differs
    /// from theirs. Whether an answer came.
    async fn hear_rating(&self, video_id: &str) -> bool {
        if self.unheard_ratings([video_id.to_owned()]).is_empty() {
            return false;
        }
        let answer = self
            .session
            .call(|c| {
                let video_id = video_id.to_owned();
                async move { c.watch(&video_id, None).await }
            })
            .await;
        let like = match answer {
            Ok(queue) => match queue.current_like {
                Some(like) => like,
                None => return false,
            },
            Err(_) => {
                self.rating_not_heard(video_id);
                return false;
            }
        };
        let rated = rated_from(Some(like));
        let changed = {
            let mut state = self.state();
            !state.given.contains(video_id) && !state.tabs.set_rated(video_id, rated).is_empty()
        };
        if changed {
            self.save();
            self.emit(PlaybackEvent::Rated {
                video_id: video_id.to_owned(),
                rated,
            });
        }
        true
    }

    /// Fetches the link and the loudness of `video_id` ahead, such as for a
    /// track under the pointer, when there is room: a click on it then plays
    /// at once.
    pub fn prefetch(self: &Arc<Self>, video_id: &str) {
        let cache = self.cache();
        if cache.has(video_id) && self.state().loudness.contains_key(video_id) {
            return;
        }
        let Ok(permit) = Arc::clone(&self.prefetches).try_acquire_owned() else {
            return;
        };
        let me = Arc::clone(self);
        let video_id = video_id.to_owned();
        self.runtime.spawn(async move {
            let _permit = permit;
            let _ = tokio::join!(cache.get_ahead(&video_id), me.loudness(&video_id));
        });
    }

    /// Fetches ahead the links and loudness of `rows`, the shown tab's rows
    /// in view (top first), once the list has come to rest: a click there
    /// then plays at once. Within the limits of `ahead.rs`, one link at a
    /// time and after the track under the pointer; new rows replace those
    /// not fetched yet.
    pub fn prefetch_visible(self: &Arc<Self>, rows: &[QueueItem]) {
        if !self.window_shown.load(Ordering::SeqCst) {
            return;
        }
        let cache = self.cache();
        // A link an earlier run left comes without its loudness, which a
        // click would wait for.
        let wanted = {
            let state = self.state();
            crate::ahead::rows_to_fetch(rows, self.skip(), |id| {
                cache.has(id) && state.loudness.contains_key(id)
            })
        };
        if !self.state().visible.replace(wanted) {
            return;
        }
        let me = Arc::clone(self);
        self.runtime.spawn(async move { me.fetch_visible().await });
    }

    /// Fetches the rows of [`Playback::prefetch_visible`] one by one.
    async fn fetch_visible(&self) {
        loop {
            let cache = self.cache();
            let shown = self.window_shown.load(Ordering::SeqCst);
            let next = {
                let mut guard = self.state();
                let state = &mut *guard;
                let loudness = &state.loudness;
                state.visible.next(
                    |id| cache.has(id) && loudness.contains_key(id),
                    shown,
                    std::time::Instant::now(),
                )
            };
            let Some(video_id) = next else {
                return;
            };
            let (link, _) = tokio::join!(cache.get_ahead(&video_id), self.loudness(&video_id));
            if let Err(e) = link
                && self.state().visible.failed(&e.to_string())
            {
                eprintln!(
                    "orbis: YouTube asks whether orbis is a bot; rows in view are not fetched ahead any more: {e}"
                );
            }
        }
    }

    /// The measured loudness of `video_id`, asked for once.
    async fn loudness(&self, video_id: &str) -> Option<f64> {
        if let Some(known) = self.state().loudness.get(video_id).copied() {
            return known;
        }
        let loudness = self
            .session
            .call(|c| {
                let id = video_id.to_owned();
                async move { c.player_info(&id).await }
            })
            .await
            .ok()
            .and_then(|info| info.loudness_lkfs);
        self.state().loudness.insert(video_id.to_owned(), loudness);
        loudness
    }

    /// The correction of a track of `loudness`, when correcting.
    fn gain(&self, loudness: Option<f64>) -> Option<f64> {
        self.normalizes()
            .then(|| normalization_gain(loudness, TARGET_LKFS))
            .flatten()
    }

    fn emit(&self, event: PlaybackEvent) {
        let _ = self.events.send(event);
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("playback state lock")
    }

    fn save(&self) {
        let mut state = self.state();
        let State { tabs, store, .. } = &mut *state;
        if let Err(e) = store.save(tabs) {
            drop(state);
            self.emit(PlaybackEvent::Error(PlaybackFailure::SaveTabs(
                e.to_string(),
            )));
        }
    }

    /// A copy of the tabs for the UI.
    pub fn tabs(&self) -> Tabs {
        self.state().tabs.clone()
    }

    /// The tabs were made at this start: there were none stored.
    pub fn is_first_start(&self) -> bool {
        self.first_start
    }

    /// Changes the tabs (open, close, move, show) and saves them.
    /// Closes every tab and queue — on signing out, when asked to, so that
    /// another account does not find them: Home and Library stay, and
    /// nothing plays.
    pub fn close_all_tabs(&self) -> Result<(), PlaybackError> {
        {
            let mut state = self.state();
            state.tabs.reset();
            state.enqueued = None;
            state.retried = None;
        }
        self.save();
        self.engine.stop()?;
        self.emit(PlaybackEvent::Stopped);
        Ok(())
    }

    pub fn edit_tabs<T>(&self, edit: impl FnOnce(&mut Tabs) -> T) -> T {
        let result = edit(&mut self.state().tabs);
        self.save();
        result
    }

    pub fn position(&self) -> Option<f64> {
        self.engine.position()
    }

    pub fn duration(&self) -> Option<f64> {
        self.engine.duration()
    }

    pub fn paused(&self) -> bool {
        self.engine.paused()
    }

    pub fn volume(&self) -> Option<f64> {
        self.engine.volume()
    }

    pub fn set_volume(&self, volume: f64) -> Result<(), PlaybackError> {
        Ok(self.engine.set_volume(volume)?)
    }

    pub fn toggle_pause(&self) -> Result<(), PlaybackError> {
        Ok(self.engine.toggle_pause()?)
    }

    pub fn seek(&self, seconds: f64) -> Result<(), PlaybackError> {
        Ok(self.engine.seek(seconds)?)
    }

    /// Resolves a queue item into something mpv can play.
    async fn media(&self, item: &QueueItem, start: f64) -> Result<Media, PlaybackError> {
        let cache = self.cache();
        let (stream, loudness) =
            tokio::join!(cache.get(&item.video_id), self.loudness(&item.video_id));
        let stream = stream?;
        let user_agent = stream
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("user-agent"))
            .map(|(_, v)| v.clone());
        Ok(Media {
            video_id: item.video_id.clone(),
            url: stream.url,
            user_agent,
            gain_db: self.gain(loudness),
            start: Some(start),
        })
    }

    /// Plays a request from the tabs.
    async fn start_request(&self, request: PlayRequest) -> Result<(), PlaybackError> {
        let media = self.media(&request.item, request.start_at).await?;
        self.state().enqueued = None;
        self.engine.play(&media)?;
        Ok(())
    }

    /// Plays `items` in tab `tab` from `start`, replacing its queue.
    pub async fn play_list(
        &self,
        tab: TabId,
        items: Vec<QueueItem>,
        start: usize,
        continuation: Option<String>,
    ) -> Result<(), PlaybackError> {
        let position = self.engine.position().unwrap_or(0.0);
        self.state().switch_asked = Some(std::time::Instant::now());
        let request = self
            .state()
            .tabs
            .play_list(tab, items, start, continuation, position);
        self.save();
        if let Some(request) = request {
            self.start_request(request).await?;
        }
        Ok(())
    }

    /// Lets tab `tab` play again from its saved place.
    pub async fn resume(&self, tab: TabId) -> Result<(), PlaybackError> {
        let position = self.engine.position().unwrap_or(0.0);
        let request = self.state().tabs.resume(tab, position);
        self.save();
        if let Some(request) = request {
            self.start_request(request).await?;
        }
        Ok(())
    }

    /// How long the last next, previous or clicked track took from the ask
    /// to its audio (the smoke test reports it).
    pub fn last_switch(&self) -> Option<std::time::Duration> {
        self.state().last_switch
    }

    /// Skips to the next track of the playing tab.
    pub async fn next(&self) -> Result<(), PlaybackError> {
        self.state().switch_asked = Some(std::time::Instant::now());
        let skip = self.skip();
        let request = self.state().tabs.advance_where(skip);
        self.save();
        match request {
            Some(request) => self.start_request(request).await,
            None => {
                self.engine.stop()?;
                self.emit(PlaybackEvent::Stopped);
                Ok(())
            }
        }
    }

    /// Restarts the track, or goes back one when it just began.
    pub async fn previous(&self) -> Result<(), PlaybackError> {
        self.state().switch_asked = Some(std::time::Instant::now());
        if self.engine.position().unwrap_or(0.0) > RESTART_AFTER {
            return self.seek(0.0);
        }
        let request = {
            let mut state = self.state();
            let id = state.tabs.playing();
            let item = state
                .tabs
                .playing_queue_mut()
                .and_then(|q| q.back().cloned());
            id.zip(item).map(|(tab, item)| PlayRequest {
                tab,
                item,
                start_at: 0.0,
            })
        };
        self.save();
        match request {
            Some(request) => self.start_request(request).await,
            None => Ok(()),
        }
    }

    /// "Play next" in the playing tab (or `fallback` when none plays).
    pub fn play_next(&self, items: Vec<QueueItem>, fallback: TabId) {
        let tab = {
            let mut state = self.state();
            state.tabs.play_next(items, fallback);
            state.enqueued = None;
            state.tabs.playing().unwrap_or(fallback)
        };
        // What follows the current track changed: drop what mpv holds.
        let _ = self.engine.clear_enqueued();
        self.save();
        self.emit(PlaybackEvent::QueueChanged { tab });
    }

    /// "Add to queue" in the playing tab (or `fallback` when none plays).
    pub fn add_to_queue(&self, items: Vec<QueueItem>, fallback: TabId) {
        let tab = {
            let mut state = self.state();
            state.tabs.add_to_queue(items, fallback);
            state.tabs.playing().unwrap_or(fallback)
        };
        self.save();
        self.emit(PlaybackEvent::QueueChanged { tab });
    }

    /// Follows mpv. Never waits for the network: what a started track needs
    /// (more of the queue, the next track's link) runs in its own task, so
    /// that pauses and the start of the audio are seen at once.
    fn on_engine_event(self: &Arc<Self>, event: PlayerEvent) {
        match event {
            PlayerEvent::Started { video_id, duration } => {
                let (now, started) = {
                    let mut state = self.state();
                    state.started += 1;
                    let started = state.started;
                    let current = state
                        .tabs
                        .playing_queue_mut()
                        .and_then(|q| q.current().map(|i| i.video_id.clone()));
                    // mpv switched to the enqueued track: the queue follows,
                    // past the tracks skipped when it was chosen.
                    if current.as_deref() != Some(video_id.as_str())
                        && state.enqueued.as_deref() == Some(video_id.as_str())
                    {
                        state.tabs.advance_where(self.skip());
                        if let Some(queue) = state.tabs.playing_queue_mut()
                            && queue.current().map(|i| i.video_id.as_str()) != Some(&video_id)
                            && let Some(index) =
                                queue.items().iter().position(|i| i.video_id == video_id)
                        {
                            queue.jump_to(index);
                        }
                    }
                    if state.retried.as_deref() != Some(video_id.as_str()) {
                        state.retried = None;
                    }
                    state.enqueued = None;
                    // The next track went to mpv before a change of the
                    // correction: set it as it is now.
                    let loudness = state.loudness.get(&video_id).copied().flatten();
                    let _ = self.engine.set_gain(self.gain(loudness).unwrap_or(0.0));
                    let tab = state.tabs.playing();
                    let item = state
                        .tabs
                        .playing_queue_mut()
                        .and_then(|q| q.current().cloned());
                    (tab.zip(item), started)
                };
                self.save();
                if let Some((tab, item)) = now {
                    self.emit(PlaybackEvent::NowPlaying {
                        tab,
                        item,
                        duration,
                    });
                    let me = Arc::clone(self);
                    tokio::spawn(async move {
                        me.load_more_if_low(tab).await;
                        me.enqueue_next(started).await;
                    });
                }
            }
            PlayerEvent::Ended {
                reason: EndReason::Error,
                error,
                video_id,
            } => {
                // The current track: tried once more with a new link, taken
                // the usual way; a second failure skips it.
                let mut earlier = false;
                let retry = {
                    let mut state = self.state();
                    // The enqueued track failed as mpv went on to it: the
                    // queue goes on to it too.
                    if let Some(failed) = video_id.as_deref()
                        && state.enqueued.as_deref() == Some(failed)
                    {
                        state.enqueued = None;
                        state.tabs.advance_where(self.skip());
                    }
                    let tab = state.tabs.playing();
                    let item = state
                        .tabs
                        .playing_queue_mut()
                        .and_then(|q| q.current().cloned());
                    match tab.zip(item) {
                        Some((tab, item)) => {
                            earlier = self.cache().invalidate(&item.video_id);
                            let first = state.retried.as_deref() != Some(item.video_id.as_str());
                            state.retried = Some(item.video_id.clone());
                            first.then_some(PlayRequest {
                                tab,
                                item,
                                start_at: 0.0,
                            })
                        }
                        None => None,
                    }
                };
                let me = Arc::clone(self);
                match retry {
                    Some(request) => {
                        // A link an earlier run left may fail for the
                        // address alone: the quick way is not to blame.
                        if !earlier {
                            self.cache().resolver().drop_quick();
                        }
                        tokio::spawn(async move {
                            if let Err(e) = me.start_request(request).await {
                                me.emit(PlaybackEvent::Error(PlaybackFailure::Play {
                                    video_id: None,
                                    cause: e.to_string(),
                                }));
                                let _ = me.next().await;
                            }
                        });
                    }
                    None => {
                        self.emit(PlaybackEvent::Error(PlaybackFailure::Play {
                            video_id,
                            cause: error.unwrap_or_default(),
                        }));
                        tokio::spawn(async move {
                            let _ = me.next().await;
                        });
                    }
                }
            }
            PlayerEvent::Ended {
                reason: EndReason::Eof,
                ..
            } => {
                // With a track enqueued mpv goes on by itself (Started follows).
                let skip = self.skip();
                let last = {
                    let mut state = self.state();
                    state.enqueued.is_none()
                        && state
                            .tabs
                            .playing_queue_mut()
                            .is_some_and(|q| q.peek_next_where(skip).is_none())
                };
                if last {
                    let me = Arc::clone(self);
                    tokio::spawn(async move {
                        let _ = me.next().await;
                    });
                }
            }
            PlayerEvent::Paused(paused) => self.emit(PlaybackEvent::Paused(paused)),
            PlayerEvent::Restarted => {
                let mut state = self.state();
                if let Some(asked) = state.switch_asked.take() {
                    state.last_switch = Some(asked.elapsed());
                }
            }
            PlayerEvent::Ended { .. } | PlayerEvent::Idle(_) => {}
        }
    }

    /// Resolves the track after the current one and hands it to mpv, unless
    /// another track has started since start number `started`; the links of
    /// the two tracks after that one are fetched too, for a quick second and
    /// third "next". When disliked tracks are skipped, the next one's rating
    /// is asked first if orbis has not heard it since the start: a track
    /// disliked elsewhere since its queue was loaded is skipped too.
    async fn enqueue_next(&self, started: u64) {
        let (next, after) = loop {
            let skip = self.skip();
            let (next, after) = {
                let mut state = self.state();
                let queue = state.tabs.playing_queue_mut();
                let next = queue
                    .as_ref()
                    .and_then(|q| q.peek_next_where(&skip).cloned());
                let after: Vec<String> = queue
                    .map(|q| {
                        q.after_next(&skip, 2)
                            .into_iter()
                            .map(|i| i.video_id.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                (next, after)
            };
            let Some(next) = next else {
                return;
            };
            if !self.skips_disliked() {
                break (next, after);
            }
            // Told now: the queue is looked at again, and goes past the
            // track if it is disliked (there or in orbis meanwhile).
            let told = self.hear_rating(&next.video_id).await;
            if self.state().started != started {
                return;
            }
            if !told {
                break (next, after);
            }
        };
        let cache = self.cache();
        for video_id in &after {
            cache.prefetch(video_id);
        }
        match self.media(&next, 0.0).await {
            Ok(media) => {
                // Another track started meanwhile: this one is not next.
                if self.state().started != started {
                    return;
                }
                if self.engine.enqueue(&media).is_ok() {
                    self.state().enqueued = Some(next.video_id);
                }
            }
            Err(e) => self.emit(PlaybackEvent::Error(PlaybackFailure::PrepareNext(
                e.to_string(),
            ))),
        }
    }

    /// Loads the next page of the playing tab's source when few tracks remain.
    async fn load_more_if_low(&self, tab: TabId) {
        let (source, token) = {
            let state = self.state();
            let Some(t) = state.tabs.get(tab) else {
                return;
            };
            if !t.queue.wants_more(LOW_WATER) {
                return;
            }
            (t.source.clone(), t.queue.continuation.clone())
        };
        let Some(token) = token else {
            return;
        };
        let page = match source {
            TabSource::Radio { .. } => self
                .session
                .call(|c| {
                    let token = token.clone();
                    async move { c.watch_next(&token).await }
                })
                .await
                .map(|q| (q.tracks, q.continuation)),
            _ => self
                .session
                .call(|c| {
                    let token = token.clone();
                    async move { c.playlist_next(&token).await }
                })
                .await
                .map(|p| (p.tracks, p.continuation)),
        };
        match page {
            Ok((tracks, continuation)) => {
                self.heard_ratings(tracks.iter().map(|t| t.video_id.clone()));
                let hide = self.hides_disliked();
                let items = tracks
                    .iter()
                    .filter(|t| t.available)
                    .map(item_from_track)
                    .filter(|i| !(hide && i.rated == Rated::Dislike))
                    .collect();
                {
                    let mut state = self.state();
                    let Some(t) = state.tabs.get_mut(tab) else {
                        return;
                    };
                    // Two tracks started in quick succession both asked for
                    // this page: the first one appended it already.
                    if t.queue.continuation.as_ref() != Some(&token) {
                        return;
                    }
                    t.queue.append_page(items, continuation);
                }
                self.save();
                self.emit(PlaybackEvent::QueueChanged { tab });
            }
            Err(e) => self.emit(PlaybackEvent::Error(PlaybackFailure::LoadMore(
                e.to_string(),
            ))),
        }
    }
}
