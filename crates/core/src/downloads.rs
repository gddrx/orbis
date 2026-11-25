//! Downloads for the window and `orbis-cli`: a track, an album or a whole
//! playlist joins a queue that works on two tracks at a time. Each track
//! learns its album, year and place on the album (one album page serves all
//! its tracks), its cover and lyrics, then [`download::Downloader`] saves
//! its stream with the tags; YouTube's measure of the loudness is asked
//! only for a file ffmpeg cannot measure. MusicBrainz is asked once the
//! file is saved, in the order the files were (its one request a second
//! would hold the downloads up), and its tags are added to the file. When
//! a saved track is the last of its album to be there, the album's
//! loudness is measured over all its files and set in each. Events
//! tell the UI how each track fares. The window keeps the queue in a
//! database (see [`kept`]): what has not ended, MusicBrainz's part
//! included, comes back after a restart.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use download::{
    AlbumMeta, Bitrates, Downloader, Extras, Format, Loudness, MusicBrainz, Outcome, Template,
    TrackMeta,
};
use stream::{StreamCache, YtDlp};
use tokio::runtime::Handle;
use tokio::sync::Semaphore;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use ytm::{Album, Track, TrackKind, WatchQueue};

use crate::covers::sized_url;
use crate::{CoreError, SessionManager};

mod kept;

use kept::Kept;
pub use kept::KeptError;

/// Tracks downloaded at the same time.
const AT_ONCE: usize = 2;

/// What happens to a download.
#[derive(Debug, Clone, PartialEq)]
pub enum DownloadEvent {
    /// A track joined the queue.
    Queued {
        id: u64,
        title: String,
        artists: String,
    },
    /// Its turn came: the album and the stream are looked up.
    Started {
        id: u64,
        title: String,
    },
    /// The share done, from 0 to 1.
    Progress {
        id: u64,
        share: f64,
    },
    /// The file is there. With `tagging`, more tags follow — MusicBrainz's
    /// or the album's gain — and [`DownloadEvent::Tagged`] says when.
    Finished {
        id: u64,
        outcome: Outcome,
        tagging: bool,
    },
    /// The tags after the file are in. `musicbrainz`: whether MusicBrainz
    /// knew the track, when it was asked; `album`: the album's loudness,
    /// when this track completed its album.
    Tagged {
        id: u64,
        musicbrainz: Option<bool>,
        album: Option<Loudness>,
    },
    Failed {
        id: u64,
        message: String,
    },
    Cancelled {
        id: u64,
    },
}

/// A track to download, with what is known of it already.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Wanted {
    pub video_id: String,
    /// Empty when unknown: it is looked up.
    pub title: String,
    pub artists: Vec<String>,
    /// The album's browse ID (`MPREb_…`), when known.
    pub album_id: Option<String>,
    /// The whole name, when the album page has been read already.
    pub meta: Option<TrackMeta>,
}

impl Wanted {
    /// A track known only by its video ID.
    pub fn video(video_id: &str) -> Self {
        Self {
            video_id: video_id.to_owned(),
            ..Self::default()
        }
    }

    /// A track of a list (a playlist, the queue): its album is looked up.
    pub fn from_track(track: &Track) -> Self {
        Self {
            video_id: track.video_id.clone(),
            title: track.title.clone(),
            artists: track.artists.iter().map(|a| a.name.clone()).collect(),
            album_id: track.album.as_ref().and_then(|a| a.id.clone()),
            meta: None,
        }
    }
}

/// Titles compared without case and outer spaces.
fn same_title(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// The album part of a track's name: its place found by video ID, or by
/// title (a music video is a different ID from the album's song).
fn album_meta(album: &Album, video_id: &str, title: &str) -> AlbumMeta {
    let position = album
        .tracks
        .iter()
        .position(|t| t.video_id == video_id)
        .or_else(|| {
            album
                .tracks
                .iter()
                .position(|t| same_title(&t.title, title))
        });
    AlbumMeta {
        title: album.title.clone(),
        artists: album.artists.iter().map(|a| a.name.clone()).collect(),
        year: album.year,
        track_number: position.map(|p| p as u32 + 1),
        track_count: u32::try_from(album.tracks.len()).ok().filter(|n| *n > 0),
    }
}

/// The names of every playable track of `album`, in its order. Album rows
/// carry no artists: the album's own are used for them.
fn album_tracks(album: &Album) -> Vec<TrackMeta> {
    album
        .tracks
        .iter()
        .filter(|t| t.available)
        .map(|t| {
            let artists: Vec<String> = t.artists.iter().map(|a| a.name.clone()).collect();
            TrackMeta {
                video_id: t.video_id.clone(),
                title: t.title.clone(),
                artists: if artists.is_empty() {
                    album.artists.iter().map(|a| a.name.clone()).collect()
                } else {
                    artists
                },
                album: Some(album_meta(album, &t.video_id, &t.title)),
            }
        })
        .collect()
}

/// The pictures to try as a track's cover, best first: the album's, then
/// the track's own. A video's own is its largest frame, `maxresdefault`
/// (the downloader cuts it to a square), then the one YouTube Music lists.
fn cover_urls(video_id: &str, album: Option<&Album>, track: Option<&Track>) -> Vec<String> {
    let size = download::COVER_SIZE;
    let mut urls = Vec::new();
    if let Some(url) = album.and_then(|a| a.thumbnail.as_deref()) {
        urls.push(sized_url(url, size, size));
    }
    if !track.is_some_and(|t| t.kind == TrackKind::Song) {
        urls.push(format!(
            "https://i.ytimg.com/vi/{video_id}/maxresdefault.jpg"
        ));
    }
    if let Some(url) = track.and_then(|t| t.thumbnail.as_deref()) {
        urls.push(sized_url(url, size, size));
    }
    urls.dedup();
    urls
}

/// How downloads are saved: the settings' part of the downloader.
#[derive(Debug, Clone)]
pub struct DownloadOptions {
    /// The music folder.
    pub root: PathBuf,
    pub format: Format,
    pub template: Template,
    pub bitrates: Bitrates,
    /// Asked for each saved track, when set.
    pub musicbrainz: Option<Arc<MusicBrainz>>,
}

impl DownloadOptions {
    fn downloader(&self) -> Downloader {
        let downloader = Downloader::new(self.root.clone(), self.format)
            .with_template(self.template.clone())
            .with_bitrates(self.bitrates);
        match &self.musicbrainz {
            Some(musicbrainz) => downloader.with_musicbrainz(Arc::clone(musicbrainz)),
            None => downloader,
        }
    }
}

/// A download's file, and what MusicBrainz is asked about it.
struct Done {
    outcome: Outcome,
    meta: TrackMeta,
    /// The album's browse ID (`MPREb_…`), when known.
    album_id: Option<String>,
    /// The audio's length in seconds, when the stream said.
    length: Option<f64>,
}

/// A download that waits or runs.
struct Job {
    abort: tokio::task::AbortHandle,
    /// Its row in the kept queue, when kept.
    row: Option<i64>,
}

/// The download queue.
pub struct Downloads {
    session: Arc<SessionManager>,
    streams: Arc<StreamCache<YtDlp>>,
    /// Taken by each download as it starts; replaced by new options.
    downloader: std::sync::RwLock<Downloader>,
    runtime: Handle,
    slots: Arc<Semaphore>,
    /// Album pages by browse ID; `None` for one that failed.
    albums: tokio::sync::Mutex<HashMap<String, Option<Arc<Album>>>>,
    jobs: Mutex<HashMap<u64, Job>>,
    /// Held while an album's files are rewritten, by album browse ID.
    album_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    next: AtomicU64,
    /// The downloads not ended, kept across restarts once
    /// [`Downloads::resume`] has opened their database.
    kept: Mutex<Option<Kept>>,
    events: UnboundedSender<DownloadEvent>,
}

impl std::fmt::Debug for Downloads {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Downloads")
            .field("downloader", &self.downloader)
            .finish_non_exhaustive()
    }
}

impl Downloads {
    /// Saves into `root` (the music folder) in `format`, with what
    /// `musicbrainz` knows of each track when given; call on tokio.
    pub fn new(
        session: Arc<SessionManager>,
        options: DownloadOptions,
    ) -> (Arc<Self>, UnboundedReceiver<DownloadEvent>) {
        let downloader = options.downloader();
        let cookies = Arc::clone(&session);
        let streams = StreamCache::new(
            crate::yt_dlp().with_cookies(Arc::new(move || Some(cookies.cookies()))),
        );
        let (events, receiver) = unbounded_channel();
        let downloads = Arc::new(Self {
            session,
            streams,
            downloader: std::sync::RwLock::new(downloader),
            runtime: Handle::current(),
            slots: Arc::new(Semaphore::new(AT_ONCE)),
            albums: tokio::sync::Mutex::new(HashMap::new()),
            jobs: Mutex::new(HashMap::new()),
            album_locks: Mutex::new(HashMap::new()),
            next: AtomicU64::new(0),
            kept: Mutex::new(None),
            events,
        });
        (downloads, receiver)
    }

    /// The downloader as it is now: what a download that starts takes.
    fn downloader(&self) -> Downloader {
        self.downloader.read().expect("downloader lock").clone()
    }

    /// Saves the tracks that start from now on by `options`; those under
    /// way finish as they began.
    pub fn set_options(&self, options: &DownloadOptions) {
        *self.downloader.write().expect("downloader lock") = options.downloader();
    }

    /// The music folder tracks go to.
    pub fn root(&self) -> PathBuf {
        self.downloader().root().to_owned()
    }

    pub fn format(&self) -> Format {
        self.downloader().format()
    }

    /// Keeps the downloads that have not ended in the database at `path`
    /// from now on, and queues those it kept before, in their order: one
    /// cut half-way starts over. Returns how many came back. Call before
    /// queueing anything: what is queued earlier is not kept.
    pub fn resume(self: &Arc<Self>, path: &Path) -> Result<usize, KeptError> {
        let mut kept = Kept::open(path)?;
        let back = kept.load()?;
        *self.kept.lock().expect("kept lock") = Some(kept);
        let count = back.len();
        for (row, wanted) in back {
            self.start(wanted, Some(row), true);
        }
        Ok(count)
    }

    /// Queues a track; returns its download's ID.
    pub fn track(self: &Arc<Self>, wanted: Wanted) -> u64 {
        self.queue(vec![wanted])[0]
    }

    /// Starts the download of `wanted`, kept in the database's `row`;
    /// `restored` when it comes back after a restart.
    fn start(self: &Arc<Self>, wanted: Wanted, row: Option<i64>, restored: bool) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.events.send(DownloadEvent::Queued {
            id,
            title: wanted.title.clone(),
            artists: wanted.artists.join(", "),
        });
        let me = Arc::clone(self);
        let task = self.runtime.spawn(async move {
            let result = me.run(id, wanted).await;
            // From here on the download cannot be cancelled: the file is
            // there, MusicBrainz only adds to its tags.
            me.jobs.lock().expect("jobs lock").remove(&id);
            let downloader = me.downloader();
            let done = match result {
                Ok(done) => done,
                Err(message) => {
                    me.forget(row.as_slice());
                    let _ = me.events.send(DownloadEvent::Failed { id, message });
                    return;
                }
            };
            // A file found already there was saved before: by a run that
            // stopped before the tags after the file when the download came
            // back after a restart, or else with its tags complete.
            let fresh = match &done.outcome {
                Outcome::Saved(path) => Some(path.clone()),
                Outcome::AlreadyThere(path) if restored => Some(path.clone()),
                Outcome::AlreadyThere(_) => None,
            };
            let musicbrainz = fresh.clone().filter(|_| downloader.asks_musicbrainz());
            let album = match (&fresh, &done.album_id) {
                (Some(_), Some(album_id)) => me.complete_album(album_id, &downloader).await,
                _ => None,
            };
            let _ = me.events.send(DownloadEvent::Finished {
                id,
                outcome: done.outcome,
                tagging: musicbrainz.is_some() || album.is_some(),
            });
            if musicbrainz.is_none() && album.is_none() {
                me.forget(row.as_slice());
                return;
            }
            // One rewrite of an album's files at a time: the album's gain
            // goes in after MusicBrainz's tags of its tracks, never over a
            // copy one of them is making.
            let lock = done.album_id.as_deref().map(|a| me.album_lock(a));
            let _album = match &lock {
                Some(lock) => Some(lock.lock().await),
                None => None,
            };
            let found = match &musicbrainz {
                Some(path) => Some(
                    downloader
                        .add_musicbrainz(path, &done.meta, done.length)
                        .await
                        .unwrap_or_else(|e| {
                            eprintln!("orbis: MusicBrainz's tags did not go in: {e}");
                            false
                        }),
                ),
                None => None,
            };
            let loudness = match &album {
                Some(files) => downloader
                    .add_album_loudness(files)
                    .await
                    .unwrap_or_else(|e| {
                        eprintln!("orbis: the album's gain did not go in: {e}");
                        None
                    }),
                None => None,
            };
            me.forget(row.as_slice());
            let _ = me.events.send(DownloadEvent::Tagged {
                id,
                musicbrainz: found,
                album: loudness,
            });
        });
        self.jobs.lock().expect("jobs lock").insert(
            id,
            Job {
                abort: task.abort_handle(),
                row,
            },
        );
        id
    }

    /// Keeps `tracks` in the database, when there is one; their rows, or
    /// none when they cannot be kept (they download all the same).
    fn keep(&self, tracks: &[Wanted]) -> Vec<Option<i64>> {
        let mut kept = self.kept.lock().expect("kept lock");
        match kept.as_mut().map(|k| k.add(tracks)) {
            Some(Ok(rows)) => rows.into_iter().map(Some).collect(),
            Some(Err(e)) => {
                eprintln!("orbis: the download queue is not kept: {e}");
                vec![None; tracks.len()]
            }
            None => vec![None; tracks.len()],
        }
    }

    /// Removes ended downloads from the database.
    fn forget(&self, rows: &[i64]) {
        if rows.is_empty() {
            return;
        }
        if let Some(kept) = self.kept.lock().expect("kept lock").as_mut()
            && let Err(e) = kept.remove(rows)
        {
            eprintln!("orbis: ended downloads stay in the kept queue: {e}");
        }
    }

    /// Queues every playable track of the album `browse_id` (`MPREb_…`).
    pub async fn album(self: &Arc<Self>, browse_id: &str) -> Result<Vec<u64>, CoreError> {
        Ok(self.queue(self.read_album(browse_id).await?))
    }

    /// Queues every playable track of the playlist `playlist_id`, all its
    /// pages. Mixes and radios never end and are not downloaded.
    pub async fn playlist(self: &Arc<Self>, playlist_id: &str) -> Result<Vec<u64>, CoreError> {
        Ok(self.queue(self.read_playlist(playlist_id).await?))
    }

    /// The playable tracks of the album `browse_id`, to queue with
    /// [`Downloads::queue`]: the window asks first when they are many.
    pub async fn read_album(&self, browse_id: &str) -> Result<Vec<Wanted>, CoreError> {
        let id = browse_id.to_owned();
        let album = Arc::new(
            self.session
                .call(|c| {
                    let id = id.clone();
                    async move { c.album(&id).await }
                })
                .await?,
        );
        self.albums
            .lock()
            .await
            .insert(browse_id.to_owned(), Some(Arc::clone(&album)));
        Ok(album_tracks(&album)
            .into_iter()
            .map(|meta| Wanted {
                video_id: meta.video_id.clone(),
                title: meta.title.clone(),
                artists: meta.artists.clone(),
                album_id: Some(browse_id.to_owned()),
                meta: Some(meta),
            })
            .collect())
    }

    /// The playable tracks of the playlist `playlist_id`, all its pages, to
    /// queue with [`Downloads::queue`]. An album's own playlist
    /// (`OLAK5uy_…`) is read as its album.
    pub async fn read_playlist(&self, playlist_id: &str) -> Result<Vec<Wanted>, CoreError> {
        let id = playlist_id.to_owned();
        if ytm::is_album_playlist(playlist_id) {
            let album = self
                .session
                .call(|c| {
                    let id = id.clone();
                    async move { c.album_of_playlist(&id).await }
                })
                .await?;
            return self.read_album(&album).await;
        }
        let playlist = self
            .session
            .call(|c| {
                let id = id.clone();
                async move { c.playlist_all(&id).await }
            })
            .await?;
        Ok(playlist
            .tracks
            .iter()
            .filter(|t| t.available)
            .map(Wanted::from_track)
            .collect())
    }

    /// Queues `tracks`, in their order; their download IDs. A long list is
    /// kept in one write.
    pub fn queue(self: &Arc<Self>, tracks: Vec<Wanted>) -> Vec<u64> {
        let rows = self.keep(&tracks);
        tracks
            .into_iter()
            .zip(rows)
            .map(|(wanted, row)| self.start(wanted, row, false))
            .collect()
    }

    /// Stops a download that waits or runs; its files are removed, and it
    /// does not come back after a restart.
    pub fn cancel(&self, id: u64) {
        let job = self.jobs.lock().expect("jobs lock").remove(&id);
        if let Some(job) = job {
            self.forget(job.row.as_slice());
            self.stop(id, &job);
        }
    }

    /// Stops every download that waits or runs; the database forgets them
    /// in one write.
    pub fn cancel_all(&self) {
        let mut jobs: Vec<(u64, Job)> = self.jobs.lock().expect("jobs lock").drain().collect();
        jobs.sort_by_key(|(id, _)| *id);
        let rows: Vec<i64> = jobs.iter().filter_map(|(_, job)| job.row).collect();
        self.forget(&rows);
        for (id, job) in &jobs {
            self.stop(*id, job);
        }
    }

    fn stop(&self, id: u64, job: &Job) {
        if !job.abort.is_finished() {
            job.abort.abort();
            let _ = self.events.send(DownloadEvent::Cancelled { id });
        }
    }

    async fn run(&self, id: u64, wanted: Wanted) -> Result<Done, String> {
        let _slot = self.slots.acquire().await.map_err(|e| e.to_string())?;
        let downloader = self.downloader();
        let (meta, album, queue) = match wanted.meta.clone() {
            Some(meta) => {
                let album = match &wanted.album_id {
                    Some(album_id) => self.album_page(album_id).await,
                    None => None,
                };
                (meta, album, None)
            }
            None => self.meta(&wanted).await,
        };
        let _ = self.events.send(DownloadEvent::Started {
            id,
            title: meta.title.clone(),
        });
        let album_id = album
            .as_ref()
            .map(|a| a.id.clone())
            .or(wanted.album_id.clone());
        if let Some(path) = downloader.existing(&meta) {
            return Ok(Done {
                outcome: Outcome::AlreadyThere(path),
                meta,
                album_id,
                length: None,
            });
        }
        let queue = match queue {
            Some(queue) => Some(queue),
            None => self.watch(&meta.video_id).await,
        };
        let extras = self.extras(&meta, album.as_deref(), queue.as_ref()).await;
        let stream = self
            .streams
            .get(&meta.video_id)
            .await
            .map_err(|e| e.to_string())?;
        let events = self.events.clone();
        let mut shown = 0.0;
        let youtube_loudness = self.youtube_loudness(&meta.video_id);
        let outcome = downloader
            .download(&meta, &extras, &stream, youtube_loudness, &mut |share| {
                if share - shown >= 0.01 || (share >= 1.0 && shown < 1.0) {
                    shown = share;
                    let _ = events.send(DownloadEvent::Progress { id, share });
                }
            })
            .await
            .map_err(|e| e.to_string())?;
        Ok(Done {
            outcome,
            meta,
            album_id,
            length: stream.duration,
        })
    }

    /// The files of every playable track of the album `album_id`, when all
    /// are there in the format and by the names `downloader` saves them.
    async fn complete_album(
        &self,
        album_id: &str,
        downloader: &Downloader,
    ) -> Option<Vec<PathBuf>> {
        let album = self.album_page(album_id).await?;
        let files: Option<Vec<PathBuf>> = album_tracks(&album)
            .iter()
            .map(|meta| downloader.existing(meta))
            .collect();
        files.filter(|files| !files.is_empty())
    }

    /// The lock an album's files are rewritten under.
    fn album_lock(&self, album_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(
            self.album_locks
                .lock()
                .expect("album locks lock")
                .entry(album_id.to_owned())
                .or_default(),
        )
    }

    /// YouTube's measure of the track's loudness, LUFS (`player`): asked
    /// only for a file ffmpeg cannot measure.
    async fn youtube_loudness(&self, video_id: &str) -> Option<f64> {
        let video_id = video_id.to_owned();
        self.session
            .call(|c| {
                let video_id = video_id.clone();
                async move { c.player_info(&video_id).await }
            })
            .await
            .ok()
            .and_then(|i| i.loudness_lkfs)
    }

    /// The track's queue from `next`: its lyrics ID, its picture and, for
    /// a track known only by its ID, its name and album. `None` when it
    /// cannot be read.
    async fn watch(&self, video_id: &str) -> Option<WatchQueue> {
        let video_id = video_id.to_owned();
        self.session
            .call(|c| {
                let video_id = video_id.clone();
                async move { c.watch(&video_id, None).await }
            })
            .await
            .ok()
    }

    /// What goes into the tags besides the name; what cannot be read is
    /// left out.
    async fn extras(
        &self,
        meta: &TrackMeta,
        album: Option<&Album>,
        queue: Option<&WatchQueue>,
    ) -> Extras {
        let track = queue.and_then(|q| q.tracks.iter().find(|t| t.video_id == meta.video_id));
        let lyrics_id = queue.and_then(|q| q.lyrics_id.clone());
        let lyrics = async {
            let id = lyrics_id?;
            self.session
                .call(|c| {
                    let id = id.clone();
                    async move { c.lyrics_page(&id).await }
                })
                .await
                .ok()
                .flatten()
        };
        Extras {
            covers: cover_urls(&meta.video_id, album, track),
            lyrics: lyrics.await.map(|l| l.text),
        }
    }

    /// What the track is called: its artists and album from `next` when
    /// they are not known, the album's year and the track's place from the
    /// album page. Also returns the album page and the queue, when read.
    async fn meta(&self, wanted: &Wanted) -> (TrackMeta, Option<Arc<Album>>, Option<WatchQueue>) {
        let mut meta = TrackMeta {
            video_id: wanted.video_id.clone(),
            title: wanted.title.clone(),
            artists: wanted.artists.clone(),
            album: None,
        };
        let mut album_id = wanted.album_id.clone();
        let mut queue = None;
        if album_id.is_none() || meta.artists.is_empty() || meta.title.is_empty() {
            queue = self.watch(&wanted.video_id).await;
            if let Some(track) = queue
                .as_ref()
                .and_then(|q| q.tracks.iter().find(|t| t.video_id == wanted.video_id))
            {
                if meta.title.is_empty() {
                    meta.title = track.title.clone();
                }
                if meta.artists.is_empty() {
                    meta.artists = track.artists.iter().map(|a| a.name.clone()).collect();
                }
                if album_id.is_none() {
                    album_id = track.album.as_ref().and_then(|a| a.id.clone());
                }
            }
        }
        if meta.title.is_empty() {
            meta.title = wanted.video_id.clone();
        }
        let album = match album_id {
            Some(album_id) => self.album_page(&album_id).await,
            None => None,
        };
        if let Some(album) = &album {
            meta.album = Some(album_meta(album, &meta.video_id, &meta.title));
        }
        (meta, album, queue)
    }

    /// The album page, read once per album; `None` when it cannot be read.
    async fn album_page(&self, id: &str) -> Option<Arc<Album>> {
        let mut albums = self.albums.lock().await;
        if let Some(album) = albums.get(id) {
            return album.clone();
        }
        let browse_id = id.to_owned();
        let album = self
            .session
            .call(|c| {
                let browse_id = browse_id.clone();
                async move { c.album(&browse_id).await }
            })
            .await
            .ok()
            .map(Arc::new);
        albums.insert(id.to_owned(), album.clone());
        album
    }
}

#[cfg(test)]
mod tests {
    use ytm::{Link, TrackKind};

    use super::*;

    fn song(video_id: &str, title: &str, artists: &[&str]) -> Track {
        Track {
            video_id: video_id.into(),
            title: title.into(),
            artists: artists
                .iter()
                .map(|a| Link {
                    name: (*a).into(),
                    id: None,
                })
                .collect(),
            album: None,
            duration: Some(200),
            thumbnail: None,
            like: None,
            set_video_id: None,
            kind: TrackKind::Song,
            available: true,
            explicit: false,
        }
    }

    fn album() -> Album {
        let mut gone = song("c", "Gone", &[]);
        gone.available = false;
        Album {
            id: "MPREb_x".into(),
            playlist_id: None,
            title: "Record".into(),
            subtitle: "Album • 2024".into(),
            artists: vec![Link {
                name: "Band".into(),
                id: None,
            }],
            year: Some(2024),
            thumbnail: None,
            tracks: vec![
                song("a", "Intro", &[]),
                song("b", "Single", &["Band", "Guest"]),
                gone,
            ],
        }
    }

    #[test]
    fn a_track_finds_its_place_on_the_album() {
        let album = album();
        let by_id = album_meta(&album, "b", "whatever");
        assert_eq!(
            (by_id.track_number, by_id.track_count, by_id.year),
            (Some(2), Some(3), Some(2024))
        );
        assert_eq!(by_id.artists, ["Band"]);
        // A music video has another ID than the album's song.
        assert_eq!(
            album_meta(&album, "video", " single ").track_number,
            Some(2)
        );
        assert_eq!(album_meta(&album, "video", "Other").track_number, None);
    }

    #[test]
    fn an_album_downloads_its_playable_tracks_with_artists() {
        let tracks = album_tracks(&album());
        let names: Vec<_> = tracks
            .iter()
            .map(|t| {
                (
                    t.title.as_str(),
                    t.artists.join(", "),
                    t.album.as_ref().and_then(|a| a.track_number),
                )
            })
            .collect();
        assert_eq!(
            names,
            [
                ("Intro", "Band".to_owned(), Some(1)),
                ("Single", "Band, Guest".to_owned(), Some(2)),
            ]
        );
    }

    #[test]
    fn the_album_cover_comes_first_and_videos_try_their_largest_frame() {
        let mut album = album();
        album.thumbnail = Some("https://lh3.googleusercontent.com/a=w544-h544-l90-rj".into());
        let mut song = song("s", "Single", &["Band"]);
        song.thumbnail = Some("https://lh3.googleusercontent.com/s=w120-h120-l90-rj".into());
        assert_eq!(
            cover_urls("s", Some(&album), Some(&song)),
            [
                "https://lh3.googleusercontent.com/a=w1200-h1200-l90-rj",
                "https://lh3.googleusercontent.com/s=w1200-h1200-l90-rj",
            ]
        );
        let mut video = song.clone();
        video.kind = TrackKind::Video;
        video.thumbnail = Some("https://i.ytimg.com/vi/v/hq720.jpg?sqp=x".into());
        assert_eq!(
            cover_urls("v", None, Some(&video)),
            [
                "https://i.ytimg.com/vi/v/maxresdefault.jpg",
                "https://i.ytimg.com/vi/v/hq720.jpg?sqp=x",
            ]
        );
        // Nothing known of the track: its frame is all there is to try.
        assert_eq!(
            cover_urls("v", None, None),
            ["https://i.ytimg.com/vi/v/maxresdefault.jpg"]
        );
    }

    #[test]
    fn a_listed_track_keeps_what_the_list_knows() {
        let mut track = song("v", "Song", &["Artist"]);
        track.album = Some(Link {
            name: "Album".into(),
            id: Some("MPREb_y".into()),
        });
        let wanted = Wanted::from_track(&track);
        assert_eq!(wanted.album_id.as_deref(), Some("MPREb_y"));
        assert_eq!(wanted.artists, ["Artist"]);
        assert!(Wanted::video("v").title.is_empty());
    }
}
