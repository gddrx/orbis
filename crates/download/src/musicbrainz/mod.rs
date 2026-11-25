//! Identifies a track in MusicBrainz for its tags: the recording, its
//! artists, genres and first release date and, when the YouTube Music album
//! is found among its releases, that release with the track's disc and
//! number on it.
//!
//! The search asks for the title, the first artist, the album and the
//! length (±3 s), then without the album; a lookup of the recording brings
//! its genres and its artists' (and one of the album's release group). As
//! MusicBrainz asks of clients: at most one request a second, a User-Agent
//! with the application's name and version (nothing about the user), and
//! answers kept — trimmed to what is read — for 30 days. A search keeps
//! only the releases that can be the wanted album (see `pick::trim`), and
//! the kept answers stay under [`MAX_KEPT`] bytes, the oldest going first.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::{Mutex, OnceCell};

use crate::naming::TrackMeta;

mod api;
mod pick;

use api::{Recording, ReleaseGroup, SearchPage};
use pick::{LENGTH_SLACK_MS, Wanted, normal, without_guests};

/// How orbis introduces itself to MusicBrainz.
pub const USER_AGENT: &str = concat!("orbis/", env!("CARGO_PKG_VERSION"));
const SERVICE: &str = "https://musicbrainz.org/ws/2";
/// The pause between requests: MusicBrainz allows one a second.
const GAP: Duration = Duration::from_millis(1100);
/// How long a kept answer is used.
const KEEP: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// The most the kept answers take, in bytes: some 5000 tracks.
pub const MAX_KEPT: u64 = 20 * 1024 * 1024;
/// Tries of a request MusicBrainz turns away as busy (503), which it often
/// does (2 of 10 tracks on 23 November 2026 with three tries); the pause
/// doubles each time, 1.1 s to 8.8 s.
const TRIES: u32 = 5;
const RESULTS: &str = "25";

#[derive(Debug, thiserror::Error)]
pub enum MusicBrainzError {
    #[error("MusicBrainz did not answer: {0}")]
    Http(String),
    #[error("MusicBrainz answered with something unexpected: {0}")]
    Parse(String),
}

/// The track as MusicBrainz knows it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identified {
    pub recording_id: String,
    /// The recording's artists, in credit order.
    pub artist_ids: Vec<String>,
    /// The first release of the recording: `YYYY`, `YYYY-MM` or `YYYY-MM-DD`.
    pub original_date: Option<String>,
    pub genres: Vec<String>,
    /// The release that is the YouTube Music album, when found.
    pub release: Option<ReleaseMatch>,
}

/// The track on the release that is its album.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReleaseMatch {
    pub release_id: String,
    pub release_group_id: Option<String>,
    /// The album's artists.
    pub artist_ids: Vec<String>,
    /// The release track's own ID (not the recording's).
    pub track_id: Option<String>,
    /// The track's place on its disc, from 1.
    pub track_number: Option<u32>,
    /// Tracks on that disc.
    pub track_count: Option<u32>,
    pub disc_number: Option<u32>,
    pub disc_count: Option<u32>,
}

/// A kept answer: the request it answers and the trimmed body.
#[derive(Serialize, serde::Deserialize)]
struct Kept<U, T> {
    url: U,
    body: T,
}

/// A stable short name of a URL for the cache: FNV-1a, 64 bits.
fn key(url: &str) -> String {
    let hash = url.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{hash:016x}")
}

/// `text` as a Lucene phrase.
fn phrase(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The search for the wanted track; `album` narrows it to that release.
fn search_query(wanted: &Wanted, album: Option<&str>) -> String {
    let mut query = format!(
        "recording:{} AND artist:{}",
        phrase(without_guests(&wanted.title)),
        phrase(&wanted.artists[0])
    );
    if let Some(album) = album {
        query.push_str(&format!(" AND release:{}", phrase(album)));
    }
    if let Some(length) = wanted.length_ms {
        query.push_str(&format!(
            " AND dur:[{} TO {}]",
            length.saturating_sub(LENGTH_SLACK_MS),
            length + LENGTH_SLACK_MS
        ));
    }
    query
}

/// The MusicBrainz client of the downloader; one per process, shared, so
/// that the pause between requests holds for all downloads.
#[derive(Debug)]
pub struct MusicBrainz {
    http: reqwest::Client,
    service: String,
    cache: Option<PathBuf>,
    gap: Duration,
    /// When the last request went out; held while one is under way.
    last: Mutex<Option<Instant>>,
    /// Held while a track is identified: tracks take their turns in the
    /// order they ask, and a track's requests are not spread among those
    /// of the tracks behind it.
    turn: Mutex<()>,
    /// The release taken for each album (by its title and artists), with
    /// how good it is (`pick::rank`), so that its other tracks get the
    /// same: the best any of its tracks took so far.
    albums: std::sync::Mutex<HashMap<String, (String, pick::Rank)>>,
    pruned: OnceCell<()>,
}

impl MusicBrainz {
    /// A client that keeps answers in `cache`, when given.
    pub fn new(cache: Option<PathBuf>) -> Self {
        Self::at(SERVICE, cache, GAP)
    }

    fn at(service: &str, cache: Option<PathBuf>, gap: Duration) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            service: service.trim_end_matches('/').to_owned(),
            cache,
            gap,
            last: Mutex::new(None),
            turn: Mutex::new(()),
            albums: std::sync::Mutex::new(HashMap::new()),
            pruned: OnceCell::new(),
        }
    }

    /// The track in MusicBrainz, or `None` when no recording fits well
    /// enough. `length` is the audio's length in seconds. One track at a
    /// time, in the order asked.
    pub async fn identify(
        &self,
        meta: &TrackMeta,
        length: Option<f64>,
    ) -> Result<Option<Identified>, MusicBrainzError> {
        let _turn = self.turn.lock().await;
        let album_key = meta
            .album
            .as_ref()
            .map(|a| format!("{}\u{1f}{}", normal(&a.title), normal(&a.artists.join(" "))));
        let taken = album_key.as_ref().and_then(|key| {
            self.albums
                .lock()
                .expect("albums lock")
                .get(key)
                .map(|(id, _)| id.clone())
        });
        let wanted = Wanted {
            title: meta.title.clone(),
            artists: meta.artists.clone(),
            album: meta.album.as_ref().map(|a| a.title.clone()),
            track_count: meta.album.as_ref().and_then(|a| a.track_count),
            length_ms: length
                .filter(|l| l.is_finite() && *l > 0.0)
                .map(|l| (l * 1000.0).round() as u64),
            release: taken,
        };
        if wanted.title.trim().is_empty() || wanted.artists.is_empty() {
            return Ok(None);
        }
        // On the album first; then anywhere, for a track whose album
        // MusicBrainz names otherwise or does not have.
        let mut found = None;
        if let Some(album) = wanted.album.as_deref() {
            let page = self.search(&wanted, Some(album)).await?;
            found = pick::pick(&wanted, &page).map(|(r, release)| (r.clone(), release.cloned()));
        }
        if found.is_none() {
            let page = self.search(&wanted, None).await?;
            found = pick::pick(&wanted, &page).map(|(r, release)| (r.clone(), release.cloned()));
        }
        let Some((recording, release)) = found else {
            return Ok(None);
        };
        // A track that could not take the album's release took another:
        // when that one is better (say, the digital release rather than a
        // Blu-ray only the first track's remaster is on), the tracks after
        // it take that one. Whatever order the tracks come in, the album
        // ends on the best release most of them are on.
        if let (Some(key), Some(release)) = (album_key, &release) {
            keep_best(
                &mut self.albums.lock().expect("albums lock"),
                key,
                &release.id,
                pick::rank(&wanted, release),
            );
        }
        // Genres are a bonus: without them the track is still identified.
        let lookup: Option<Recording> = self
            .get(
                &format!("recording/{}", recording.id),
                &[("inc", "genres+artists")],
            )
            .await
            .ok();
        let release_group = match release.as_ref().and_then(|r| r.release_group.as_ref()) {
            Some(group) => self
                .get::<ReleaseGroup>(&format!("release-group/{}", group.id), &[("inc", "genres")])
                .await
                .ok(),
            None => None,
        };
        let own = lookup
            .iter()
            .flat_map(|l| l.genres.iter())
            .chain(release_group.iter().flat_map(|g| g.genres.iter()));
        let artists = lookup
            .iter()
            .flat_map(|l| l.artist_credit.iter())
            .flat_map(|c| c.artist.genres.iter());
        Ok(Some(Identified {
            recording_id: recording.id.clone(),
            artist_ids: recording
                .artist_credit
                .iter()
                .map(|c| c.artist.id.clone())
                .collect(),
            original_date: recording
                .first_release_date
                .clone()
                .filter(|d| !d.is_empty()),
            genres: pick::genres(own, artists),
            release: release.map(|release| {
                let medium = release.media.first();
                ReleaseMatch {
                    release_id: release.id.clone(),
                    release_group_id: release.release_group.as_ref().map(|g| g.id.clone()),
                    artist_ids: release
                        .artist_credit
                        .iter()
                        .map(|c| c.artist.id.clone())
                        .collect(),
                    track_id: medium.and_then(|m| m.track.first()).map(|t| t.id.clone()),
                    track_number: medium.and_then(|m| m.track_offset).map(|o| o + 1),
                    track_count: medium.and_then(|m| m.track_count),
                    disc_number: medium.and_then(|m| m.position),
                    disc_count: release.count,
                }
            }),
        }))
    }

    async fn search(
        &self,
        wanted: &Wanted,
        album: Option<&str>,
    ) -> Result<SearchPage, MusicBrainzError> {
        let query = search_query(wanted, album);
        // The page is cut down for the wanted album, artists and the
        // release the album took, and kept under them.
        let scope = format!(
            "album={} artists={} release={}",
            wanted.album.as_deref().map(normal).unwrap_or_default(),
            normal(&wanted.artists.join("\u{1f}")),
            wanted.release.as_deref().unwrap_or_default()
        );
        self.get_kept(
            "recording",
            &[("query", &query), ("limit", RESULTS)],
            &scope,
            |page| pick::trim(page, wanted),
        )
        .await
    }

    /// `GET <service>/<path>?<params>&fmt=json`, from the cache when kept.
    async fn get<T: Serialize + DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<T, MusicBrainzError> {
        self.get_kept(path, params, "", |_| {}).await
    }

    /// [`MusicBrainz::get`], `trim`med to what the caller reads; the copy
    /// kept is the trimmed one, under the URL and `scope` (what the trim
    /// depends on).
    async fn get_kept<T: Serialize + DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
        scope: &str,
        trim: impl FnOnce(&mut T),
    ) -> Result<T, MusicBrainzError> {
        let mut params = params.to_vec();
        params.push(("fmt", "json"));
        let url = reqwest::Url::parse_with_params(&format!("{}/{path}", self.service), &params)
            .map_err(|e| MusicBrainzError::Http(e.to_string()))?;
        let kept_as = if scope.is_empty() {
            url.to_string()
        } else {
            format!("{url} {scope}")
        };
        if let Some(body) = self.kept(&kept_as).await {
            return Ok(body);
        }
        let bytes = self.fetch(url).await?;
        let mut body: T =
            serde_json::from_slice(&bytes).map_err(|e| MusicBrainzError::Parse(e.to_string()))?;
        trim(&mut body);
        self.keep(&kept_as, &body).await;
        Ok(body)
    }

    /// One request, after the pause since the last; a busy answer (503) is
    /// tried again after a pause twice as long.
    async fn fetch(&self, url: reqwest::Url) -> Result<Vec<u8>, MusicBrainzError> {
        let mut last = self.last.lock().await;
        for attempt in 0..TRIES {
            if let Some(at) = *last {
                tokio::time::sleep_until((at + self.gap * 2u32.pow(attempt)).into()).await;
            }
            let response = self.http.get(url.clone()).send().await;
            *last = Some(Instant::now());
            let response = response.map_err(|e| MusicBrainzError::Http(e.to_string()))?;
            let status = response.status();
            if status == reqwest::StatusCode::SERVICE_UNAVAILABLE && attempt + 1 < TRIES {
                continue;
            }
            if !status.is_success() {
                return Err(MusicBrainzError::Http(status.to_string()));
            }
            return response
                .bytes()
                .await
                .map(|b| b.to_vec())
                .map_err(|e| MusicBrainzError::Http(e.to_string()));
        }
        Err(MusicBrainzError::Http("busy".into()))
    }

    fn file(&self, url: &str) -> Option<PathBuf> {
        Some(self.cache.as_ref()?.join(format!("{}.json", key(url))))
    }

    /// The kept answer to `url`, when it is there and fresh.
    async fn kept<T: DeserializeOwned>(&self, url: &str) -> Option<T> {
        let file = self.file(url)?;
        if let Some(dir) = self.cache.clone() {
            self.pruned
                .get_or_init(|| async move {
                    let _ = tokio::task::spawn_blocking(move || prune(&dir, MAX_KEPT)).await;
                })
                .await;
        }
        let bytes = tokio::fs::read(&file).await.ok()?;
        let kept: Kept<String, T> = serde_json::from_slice(&bytes).ok()?;
        let fresh = tokio::fs::metadata(&file)
            .await
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .is_some_and(|age| age < KEEP);
        (fresh && kept.url == url).then_some(kept.body)
    }

    /// Keeps the answer to `url`; a failure to write only loses the copy.
    async fn keep<T: Serialize>(&self, url: &str, body: &T) {
        let Some(file) = self.file(url) else {
            return;
        };
        let Ok(bytes) = serde_json::to_vec(&Kept { url, body }) else {
            return;
        };
        let Some(dir) = file.parent() else {
            return;
        };
        let partial = file.with_extension("part");
        let _ = tokio::fs::create_dir_all(dir).await;
        if tokio::fs::write(&partial, &bytes).await.is_ok() {
            let _ = tokio::fs::rename(&partial, &file).await;
        }
    }
}

/// Remembers `id` as the release of the album `key` when it is the first
/// or better than the one remembered.
fn keep_best(
    albums: &mut HashMap<String, (String, pick::Rank)>,
    key: String,
    id: &str,
    rank: pick::Rank,
) {
    if albums.get(&key).is_none_or(|(_, kept)| rank < *kept) {
        albums.insert(key, (id.to_owned(), rank));
    }
}

/// Removes the kept answers that are too old to be used, then the oldest
/// until they take at most `max` bytes.
fn prune(dir: &Path, max: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut left = Vec::new();
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let old = SystemTime::now()
            .duration_since(modified)
            .is_ok_and(|age| age >= KEEP);
        if old {
            let _ = std::fs::remove_file(entry.path());
        } else {
            left.push((modified, metadata.len(), entry.path()));
        }
    }
    let mut total: u64 = left.iter().map(|(_, len, _)| len).sum();
    left.sort();
    for (_, len, path) in left {
        if total <= max {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
