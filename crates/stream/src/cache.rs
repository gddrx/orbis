//! Keeps resolved links and resolves ahead; with a file, the links outlive
//! the run (see [`crate::disk`]).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::disk::{Disk, link_ip};
use crate::{Stream, StreamError, StreamResolver, Urgency};

/// A link is renewed this long before it expires (seconds), so a track that
/// starts near the end of a link's life still plays to the end.
const EXPIRY_MARGIN: i64 = 30 * 60;

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

type Slot = Arc<tokio::sync::Mutex<Option<Stream>>>;

/// A cache of resolved streams in front of a resolver.
///
/// Concurrent requests for one track share a single resolve: the second waits
/// for the first. A cached link is used until [`EXPIRY_MARGIN`] before its
/// `expire` time.
///
/// Made [`StreamCache::with_disk`], it keeps its links in a file and starts
/// with those an earlier run left. Such a link works only from the address
/// it was made for: once a link of this run shows another address, those
/// of earlier runs for the old one are not used.
#[derive(Debug)]
pub struct StreamCache<R> {
    resolver: R,
    slots: Mutex<HashMap<String, Slot>>,
    disk: Option<Mutex<Disk>>,
    /// Tracks whose link an earlier run left and this run has not renewed.
    earlier: Mutex<HashSet<String>>,
    /// The address of this run's newest link.
    ip: Mutex<Option<String>>,
}

impl<R: StreamResolver + 'static> StreamCache<R> {
    pub fn new(resolver: R) -> Arc<Self> {
        Arc::new(Self::bare(resolver, None))
    }

    /// A cache that keeps its links in the file at `path`, as links in
    /// `format` (they are kept apart by format), and starts with the
    /// fresh ones kept there. Without a usable file it works as
    /// [`StreamCache::new`].
    pub fn with_disk(resolver: R, path: &Path, format: &str) -> Arc<Self> {
        let disk = Disk::open(path, format)
            .and_then(|disk| Ok((disk.load(unix_now() + EXPIRY_MARGIN)?, disk)))
            .map_err(|e| eprintln!("orbis: links are not kept between runs: {e}"))
            .ok();
        let Some((kept, disk)) = disk else {
            return Self::new(resolver);
        };
        let cache = Self::bare(resolver, Some(disk));
        {
            let mut slots = cache.slots.lock().expect("cache lock");
            let mut earlier = cache.earlier.lock().expect("earlier lock");
            for stream in kept {
                earlier.insert(stream.video_id.clone());
                slots.insert(
                    stream.video_id.clone(),
                    Arc::new(tokio::sync::Mutex::new(Some(stream))),
                );
            }
        }
        Arc::new(cache)
    }

    fn bare(resolver: R, disk: Option<Disk>) -> Self {
        Self {
            resolver,
            slots: Mutex::new(HashMap::new()),
            disk: disk.map(Mutex::new),
            earlier: Mutex::new(HashSet::new()),
            ip: Mutex::new(None),
        }
    }

    fn slot(&self, video_id: &str) -> Slot {
        self.slots
            .lock()
            .expect("cache lock")
            .entry(video_id.to_owned())
            .or_default()
            .clone()
    }

    fn fresh(stream: &Stream, now: i64) -> bool {
        stream
            .expires_at
            .is_none_or(|expires| expires - EXPIRY_MARGIN > now)
    }

    /// Whether `stream` may be played now: fresh, and when an earlier run
    /// left it, made for the address of this run's links (or none known
    /// yet).
    fn usable(&self, stream: &Stream, now: i64) -> bool {
        if !Self::fresh(stream, now) {
            return false;
        }
        if !self
            .earlier
            .lock()
            .expect("earlier lock")
            .contains(&stream.video_id)
        {
            return true;
        }
        let ip = self.ip.lock().expect("ip lock");
        match (ip.as_deref(), link_ip(&stream.url)) {
            (Some(now), Some(then)) => now == then,
            _ => true,
        }
    }

    /// Takes in a link resolved now: its address is this run's, and it is
    /// kept on disk.
    fn learn(&self, stream: &Stream) {
        if let Some(ip) = link_ip(&stream.url) {
            *self.ip.lock().expect("ip lock") = Some(ip.to_owned());
        }
        self.earlier
            .lock()
            .expect("earlier lock")
            .remove(&stream.video_id);
        if let Some(disk) = &self.disk
            && let Err(e) = disk.lock().expect("disk lock").put(stream, unix_now())
        {
            eprintln!("orbis: a link is not kept between runs: {e}");
        }
    }

    /// The stream of `video_id`, from the cache or resolved now.
    pub async fn get(&self, video_id: &str) -> Result<Stream, StreamError> {
        self.fetch(video_id, Urgency::Now).await
    }

    /// The stream of a track that might be played (under the pointer, in
    /// view): resolved, when it is not in the cache, behind the tracks to
    /// play now.
    pub async fn get_ahead(&self, video_id: &str) -> Result<Stream, StreamError> {
        self.fetch(video_id, Urgency::Ahead).await
    }

    async fn fetch(&self, video_id: &str, urgency: Urgency) -> Result<Stream, StreamError> {
        let slot = self.slot(video_id);
        let mut cached = slot.lock().await;
        if let Some(stream) = cached.as_ref()
            && self.usable(stream, unix_now())
        {
            return Ok(stream.clone());
        }
        let stream = self.resolver.resolve(video_id, urgency).await?;
        self.learn(&stream);
        *cached = Some(stream.clone());
        Ok(stream)
    }

    /// Resolves `video_id` in the background (a track after the next one),
    /// so that [`StreamCache::get`] finds it ready. Errors are left for the
    /// real request to meet.
    pub fn prefetch(self: &Arc<Self>, video_id: &str) {
        let cache = Arc::clone(self);
        let video_id = video_id.to_owned();
        tokio::spawn(async move {
            let _ = cache.get_ahead(&video_id).await;
        });
    }

    /// The resolver behind the cache.
    pub fn resolver(&self) -> &R {
        &self.resolver
    }

    /// Whether the link of `video_id` is at hand (resolved and fresh).
    pub fn has(&self, video_id: &str) -> bool {
        let slot = self
            .slots
            .lock()
            .expect("cache lock")
            .get(video_id)
            .cloned();
        slot.and_then(|slot| {
            slot.try_lock()
                .ok()
                .map(|s| s.as_ref().is_some_and(|s| self.usable(s, unix_now())))
        })
        .unwrap_or(false)
    }

    /// Forgets a link, such as one the player could not open. Returns
    /// whether an earlier run left it: then its failing says nothing of how
    /// links are resolved now (the address may have changed since).
    pub fn invalidate(&self, video_id: &str) -> bool {
        self.slots.lock().expect("cache lock").remove(video_id);
        if let Some(disk) = &self.disk
            && let Err(e) = disk.lock().expect("disk lock").remove(video_id)
        {
            eprintln!("orbis: a dropped link stays on disk: {e}");
        }
        self.earlier.lock().expect("earlier lock").remove(video_id)
    }

    /// Number of tracks with a slot (resolved, failed or in progress).
    pub fn len(&self) -> usize {
        self.slots.lock().expect("cache lock").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[derive(Debug, Default)]
    struct Counting {
        calls: AtomicUsize,
        /// Calls made ahead.
        ahead: AtomicUsize,
        expires_in: i64,
        /// The address the links are made for.
        ip: &'static str,
    }

    fn counting(expires_in: i64, ip: &'static str) -> Counting {
        Counting {
            expires_in,
            ip,
            ..Default::default()
        }
    }

    impl StreamResolver for Counting {
        fn resolve(
            &self,
            video_id: &str,
            urgency: Urgency,
        ) -> impl std::future::Future<Output = Result<Stream, StreamError>> + Send {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if urgency == Urgency::Ahead {
                self.ahead.fetch_add(1, Ordering::SeqCst);
            }
            let video_id = video_id.to_owned();
            let expires_at = Some(unix_now() + self.expires_in);
            let url = format!(
                "https://r1.googlevideo.com/videoplayback?expire={}&ip={}&id={video_id}",
                unix_now() + self.expires_in,
                self.ip
            );
            async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                if video_id == "broken" {
                    return Err(StreamError::Failed {
                        video_id,
                        message: "gone".into(),
                    });
                }
                Ok(Stream {
                    video_id,
                    url,
                    format_id: "251".into(),
                    codec: "opus".into(),
                    container: "webm".into(),
                    bitrate_kbps: Some(135.2),
                    sample_rate: Some(48000),
                    duration: Some(201.5),
                    expires_at,
                    headers: vec![("User-Agent".into(), "Mozilla/5.0".into())],
                })
            }
        }
    }

    #[tokio::test]
    async fn concurrent_requests_share_one_resolve() {
        let cache = StreamCache::new(Counting {
            expires_in: 6 * 3600,
            ..Default::default()
        });
        let (a, b) = tokio::join!(cache.get("v1"), cache.get("v1"));
        assert_eq!(a.unwrap(), b.unwrap());
        cache.get("v1").await.unwrap();
        assert_eq!(cache.resolver.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn links_near_expiry_are_renewed() {
        let cache = StreamCache::new(Counting {
            expires_in: 60,
            ..Default::default()
        });
        cache.get("v1").await.unwrap();
        cache.get("v1").await.unwrap();
        assert_eq!(cache.resolver.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn prefetch_fills_the_cache_and_errors_are_not_kept() {
        let cache = StreamCache::new(Counting {
            expires_in: 6 * 3600,
            ..Default::default()
        });
        cache.prefetch("v2");
        tokio::time::sleep(Duration::from_millis(5)).await;
        // The prefetch is still running: get waits for it instead of resolving again.
        cache.get("v2").await.unwrap();
        assert_eq!(cache.resolver.calls.load(Ordering::SeqCst), 1);
        assert_eq!(cache.resolver.ahead.load(Ordering::SeqCst), 1, "made ahead");

        assert!(cache.get("broken").await.is_err());
        assert!(cache.get("broken").await.is_err());
        assert_eq!(
            cache.resolver.calls.load(Ordering::SeqCst),
            3,
            "a failure is retried"
        );

        cache.invalidate("v2");
        cache.get("v2").await.unwrap();
        assert_eq!(cache.resolver.calls.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn links_outlive_the_cache_in_their_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("streams.db");
        let first = StreamCache::with_disk(counting(6 * 3600, "203.0.113.5"), &path, "251");
        let resolved = first.get("v1").await.unwrap();
        first.get("v2").await.unwrap();
        drop(first);

        let again = StreamCache::with_disk(counting(6 * 3600, "203.0.113.5"), &path, "251");
        assert!(again.has("v1") && again.has("v2"));
        assert_eq!(again.get("v1").await.unwrap(), resolved);
        assert_eq!(again.resolver.calls.load(Ordering::SeqCst), 0);
        // Links of another quality are kept apart.
        let other = StreamCache::with_disk(counting(6 * 3600, "203.0.113.5"), &path, "140");
        assert!(!other.has("v1"));
    }

    #[tokio::test]
    async fn stale_links_and_links_for_another_address_are_not_taken() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("streams.db");
        // Within the margin before its expiry: not kept for the next run.
        let short = StreamCache::with_disk(counting(60, "203.0.113.5"), &path, "251");
        short.get("old").await.unwrap();
        drop(short);
        let first = StreamCache::with_disk(counting(6 * 3600, "203.0.113.5"), &path, "251");
        first.get("v1").await.unwrap();
        first.get("v2").await.unwrap();
        drop(first);

        // The address changed between the runs.
        let moved = StreamCache::with_disk(counting(6 * 3600, "198.51.100.7"), &path, "251");
        assert!(!moved.has("old"), "expired");
        assert!(
            moved.has("v1"),
            "no address known yet: the old link is tried"
        );
        moved.get("v3").await.unwrap();
        assert!(!moved.has("v1"), "made for the old address");
        moved.get("v1").await.unwrap();
        assert_eq!(moved.resolver.calls.load(Ordering::SeqCst), 2);
        // An earlier run's link that fails says so; one of this run does not.
        assert!(moved.invalidate("v2"));
        assert!(!moved.invalidate("v1"));
        assert_eq!(
            moved.disk.as_ref().unwrap().lock().unwrap().count(),
            1,
            "only v3 is left in the file"
        );
    }
}
