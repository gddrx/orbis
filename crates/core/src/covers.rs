//! Covers on disk, for the window now and MPRIS later: a picture is fetched
//! once at about the size it is shown (the image hosts scale on request),
//! kept in a folder with a size limit that drops the least recently used
//! files, and handed out as a path.
//!
//! The index lives in memory: [`CoverCache::cached`] answers without touching
//! the disk or the network, so the GUI thread may call it. Downloads,
//! evictions and the scan of the folder at start run on tokio. Requests for
//! one file at once share one download.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use sha1::{Digest, Sha1};
use tokio::runtime::Handle;

/// Shown sizes are rounded up to a multiple of this, so a picture shown at
/// slightly different sizes is fetched once.
const SIZE_STEP: u32 = 60;
/// The largest size asked for.
const MAX_SIZE: u32 = 1200;
/// Bigger answers are not taken for covers.
const MAX_BYTES: usize = 8 * 1024 * 1024;
/// A used file's time is refreshed at most this often: the times order the
/// evictions after a restart.
const TOUCH_EVERY: Duration = Duration::from_secs(3600);
/// Recency of files found at start sits below this, of files used since
/// above it.
const RUN_CLOCK_START: u64 = 1 << 40;
/// The size limit of the cache unless the caller sets one.
pub const DEFAULT_LIMIT: u64 = 100 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum CoverError {
    #[error("cover download failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("the cover server answered {0}")]
    Status(u16),
    #[error("the answer is not a picture")]
    NotImage,
    #[error("the picture is larger than {MAX_BYTES} bytes")]
    TooBig,
    #[error("cover cache: {0}")]
    Io(#[from] std::io::Error),
}

/// What the cache did since it was opened, and what it holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CoverStats {
    /// Covers answered from the cache.
    pub hits: u64,
    pub downloads: u64,
    pub downloaded_bytes: u64,
    pub failures: u64,
    pub evicted: u64,
    pub files: u64,
    pub bytes: u64,
}

/// `px` rounded up to the size the cache fetches.
pub fn size_bucket(px: u32) -> u32 {
    px.clamp(1, MAX_SIZE).div_ceil(SIZE_STEP) * SIZE_STEP
}

/// The URL of the same picture just large enough to cover a `width`×`height`
/// box (it is cropped to the box), for the hosts that scale on request; other
/// URLs come back unchanged.
pub fn sized_url(url: &str, width: u32, height: u32) -> String {
    let Some(rest) = url.strip_prefix("https://") else {
        return url.to_owned();
    };
    let mut parts = rest.split('/');
    let host = parts.next().unwrap_or_default();
    if host.ends_with(".googleusercontent.com") || host.ends_with(".ggpht.com") {
        return google_sized(url, width, height).unwrap_or_else(|| url.to_owned());
    }
    if host == "i.ytimg.com"
        && width <= 320
        && height <= 180
        && let (Some("vi" | "vi_webp"), Some(video), Some(_)) =
            (parts.next(), parts.next(), parts.next())
    {
        // Of a video's pictures, 320×180 is the small one without black bars.
        return format!("https://i.ytimg.com/vi/{video}/mqdefault.jpg");
    }
    url.to_owned()
}

/// `…=w544-h544-l90-rj` or `…=s576`: the size options after the last `=`.
fn google_sized(url: &str, width: u32, height: u32) -> Option<String> {
    let (base, options) = url.rsplit_once('=')?;
    if options.contains(['/', '?', '&']) {
        return None;
    }
    let number = |token: &str, prefix: char| -> Option<u32> {
        token
            .strip_prefix(prefix)
            .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))?
            .parse()
            .ok()
    };
    let tokens: Vec<&str> = options.split('-').collect();
    let resized: Vec<String> = match (
        tokens.iter().find_map(|t| number(t, 'w')),
        tokens.iter().find_map(|t| number(t, 'h')),
        tokens.iter().find_map(|t| number(t, 's')),
    ) {
        (Some(w), Some(h), _) if w > 0 && h > 0 => {
            // Keeps the shape (banners stay wide) and covers the box.
            let (w, h) = (u64::from(w), u64::from(h));
            let (box_w, box_h) = (u64::from(width), u64::from(height));
            let (new_w, new_h) = if box_w * h >= box_h * w {
                (box_w, (box_w * h).div_ceil(w))
            } else {
                ((box_h * w).div_ceil(h), box_h)
            };
            tokens
                .iter()
                .map(|t| {
                    if number(t, 'w').is_some() {
                        format!("w{new_w}")
                    } else if number(t, 'h').is_some() {
                        format!("h{new_h}")
                    } else {
                        (*t).to_owned()
                    }
                })
                .collect()
        }
        (_, _, Some(_)) => tokens
            .iter()
            .map(|t| {
                if number(t, 's').is_some() {
                    format!("s{}", width.max(height))
                } else {
                    (*t).to_owned()
                }
            })
            .collect(),
        _ => return None,
    };
    Some(format!("{base}={}", resized.join("-")))
}

/// The file name (without extension) of a sized URL.
fn file_stem(sized_url: &str) -> String {
    let digest = Sha1::digest(sized_url.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// The extension for a picture, from its type or its first bytes.
fn extension(content_type: Option<&str>, bytes: &[u8]) -> Option<&'static str> {
    let by_type = match content_type.map(|t| t.split(';').next().unwrap_or("").trim()) {
        Some("image/jpeg") => Some("jpg"),
        Some("image/png") => Some("png"),
        Some("image/webp") => Some("webp"),
        Some("image/gif") => Some("gif"),
        _ => None,
    };
    by_type.or_else(|| {
        if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            Some("jpg")
        } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some("png")
        } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            Some("webp")
        } else if bytes.starts_with(b"GIF8") {
            Some("gif")
        } else {
            None
        }
    })
}

/// A `file://` URL of `path`, for QML.
pub fn file_url(path: &Path) -> String {
    reqwest::Url::from_file_path(path)
        .map(String::from)
        .unwrap_or_default()
}

#[derive(Debug)]
struct Entry {
    file: PathBuf,
    bytes: u64,
    /// Larger is more recent.
    used: u64,
    /// When the file's time was last set in this run.
    touched: Option<Instant>,
}

#[derive(Debug, Default)]
struct Index {
    entries: HashMap<String, Entry>,
    total: u64,
    clock: u64,
    stats: CoverStats,
}

impl Index {
    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    fn insert(&mut self, stem: String, entry: Entry) {
        self.total += entry.bytes;
        if let Some(old) = self.entries.insert(stem, entry) {
            self.total -= old.bytes;
        }
    }

    /// Takes the least recently used entries out until the total is at most
    /// `target`, and returns their files.
    fn evict_to(&mut self, target: u64) -> Vec<PathBuf> {
        if self.total <= target {
            return Vec::new();
        }
        let mut order: Vec<(u64, String)> = self
            .entries
            .iter()
            .map(|(stem, e)| (e.used, stem.clone()))
            .collect();
        order.sort_unstable();
        let mut gone = Vec::new();
        for (_, stem) in order {
            if self.total <= target {
                break;
            }
            if let Some(entry) = self.entries.remove(&stem) {
                self.total -= entry.bytes;
                gone.push(entry.file);
            }
        }
        self.stats.evicted += gone.len() as u64;
        gone
    }
}

/// A cover file found in the folder.
#[derive(Debug, PartialEq, Eq)]
struct Found {
    stem: String,
    file: PathBuf,
    bytes: u64,
    modified: SystemTime,
}

/// The cover files in `dir`, oldest first; half-written files are removed.
fn scan(dir: &Path) -> Vec<Found> {
    let Ok(listing) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in listing.flatten() {
        let file = entry.path();
        let Some(name) = file.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.ends_with(".part") {
            let _ = std::fs::remove_file(&file);
            continue;
        }
        let Some((stem, _)) = name.split_once('.') else {
            continue;
        };
        if stem.len() != 40 || !stem.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        found.push(Found {
            stem: stem.to_owned(),
            bytes: meta.len(),
            modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            file,
        });
    }
    found.sort_by(|a, b| a.modified.cmp(&b.modified).then(a.stem.cmp(&b.stem)));
    found
}

/// Writes `bytes` to `file` through a temporary file, so a reader never sees
/// half a picture. Each write has a temporary file of its own: two writers
/// of one file (another process on the same folder) never rename each
/// other's.
fn write_atomically(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let name = file.file_name().unwrap_or_default().to_string_lossy();
    let part = file.with_file_name(format!(
        ".{name}.{}.{}.part",
        std::process::id(),
        WRITES.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&part, bytes)?;
    std::fs::rename(&part, file).inspect_err(|_| {
        let _ = std::fs::remove_file(&part);
    })
}

fn touch(file: &Path) {
    if let Ok(f) = std::fs::File::open(file) {
        let _ = f.set_modified(SystemTime::now());
    }
}

fn remove_files(files: Vec<PathBuf>) {
    for file in files {
        let _ = std::fs::remove_file(file);
    }
}

/// The cover cache over one folder.
#[derive(Debug)]
pub struct CoverCache {
    dir: PathBuf,
    limit: u64,
    http: reqwest::Client,
    runtime: Handle,
    index: Mutex<Index>,
    /// A slot for each file being downloaded: requests for one file at once
    /// wait for one download instead of each writing it.
    fetching: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl CoverCache {
    /// Opens the cache in `dir` (created if missing) holding at most `limit`
    /// bytes. Must be called within a tokio runtime: the folder is scanned in
    /// the background, and covers already there are answered once it is done.
    pub fn open(dir: PathBuf, limit: u64) -> Result<Arc<Self>, CoverError> {
        std::fs::create_dir_all(&dir)?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()?;
        let cache = Arc::new(Self {
            dir,
            limit,
            http,
            runtime: Handle::current(),
            index: Mutex::new(Index {
                clock: RUN_CLOCK_START,
                ..Index::default()
            }),
            fetching: Mutex::new(HashMap::new()),
        });
        let scanning = Arc::clone(&cache);
        cache.runtime.spawn_blocking(move || {
            let found = scan(&scanning.dir);
            let gone = {
                let mut index = scanning.index();
                for (order, f) in found.into_iter().enumerate() {
                    // Covers fetched while the scan ran are newer.
                    if !index.entries.contains_key(&f.stem) {
                        let entry = Entry {
                            file: f.file,
                            bytes: f.bytes,
                            used: order as u64,
                            touched: None,
                        };
                        index.insert(f.stem, entry);
                    }
                }
                index.evict_to(scanning.limit / 10 * 9)
            };
            remove_files(gone);
        });
        Ok(cache)
    }

    fn index(&self) -> MutexGuard<'_, Index> {
        self.index.lock().expect("cover index lock")
    }

    /// The file of `url` shown in a `width`×`height` box (device pixels)
    /// when the cache has it. Touches neither the disk nor the network.
    pub fn cached(&self, url: &str, width: u32, height: u32) -> Option<PathBuf> {
        let stem = file_stem(&sized_url(url, size_bucket(width), size_bucket(height)));
        self.cached_stem(&stem)
    }

    fn cached_stem(&self, stem: &str) -> Option<PathBuf> {
        let mut index = self.index();
        let clock = index.tick();
        let entry = index.entries.get_mut(stem)?;
        entry.used = clock;
        let file = entry.file.clone();
        if entry.touched.is_none_or(|t| t.elapsed() >= TOUCH_EVERY) {
            entry.touched = Some(Instant::now());
            let file = file.clone();
            self.runtime.spawn_blocking(move || touch(&file));
        }
        index.stats.hits += 1;
        Some(file)
    }

    /// The file of `url` shown in a `width`×`height` box (device pixels):
    /// from the cache, or downloaded at about that size.
    pub async fn fetch(&self, url: &str, width: u32, height: u32) -> Result<PathBuf, CoverError> {
        let size = (size_bucket(width), size_bucket(height));
        let sized = sized_url(url, size.0, size.1);
        let stem = file_stem(&sized);
        if let Some(file) = self.cached_stem(&stem) {
            return Ok(file);
        }
        // Another request may be downloading this file: wait for it, then
        // take its file. A failed download is not remembered: the next one
        // in line tries again.
        let slot = Arc::clone(
            self.fetching
                .lock()
                .expect("cover fetching lock")
                .entry(stem.clone())
                .or_default(),
        );
        let result = {
            let _turn = slot.lock().await;
            match self.cached_stem(&stem) {
                Some(file) => Ok(file),
                None => self.download(&sized, &stem, size).await,
            }
        };
        {
            // The last one out takes the slot away (one Arc is the map's).
            let mut fetching = self.fetching.lock().expect("cover fetching lock");
            if Arc::strong_count(&slot) == 2 {
                fetching.remove(&stem);
            }
        }
        if result.is_err() {
            self.index().stats.failures += 1;
        }
        result
    }

    async fn download(
        &self,
        sized: &str,
        stem: &str,
        (width, height): (u32, u32),
    ) -> Result<PathBuf, CoverError> {
        let host = reqwest::Url::parse(sized)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_default();
        let started = Instant::now();
        let response = self.http.get(sized).send().await?;
        let status = response.status();
        if !status.is_success() {
            ytm::netlog::log(format_args!(
                "cover {host} {width}×{height}: {}",
                status.as_u16()
            ));
            return Err(CoverError::Status(status.as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_BYTES as u64)
        {
            return Err(CoverError::TooBig);
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let bytes = response.bytes().await?;
        if bytes.len() > MAX_BYTES {
            return Err(CoverError::TooBig);
        }
        let ext = extension(content_type.as_deref(), &bytes).ok_or(CoverError::NotImage)?;
        ytm::netlog::log(format_args!(
            "cover {host} {width}×{height} {} B in {} ms",
            bytes.len(),
            started.elapsed().as_millis()
        ));

        let file = self.dir.join(format!("{stem}.{ext}"));
        let size = bytes.len() as u64;
        {
            let file = file.clone();
            tokio::task::spawn_blocking(move || write_atomically(&file, &bytes))
                .await
                .map_err(std::io::Error::other)??;
        }
        let gone = {
            let mut index = self.index();
            let used = index.tick();
            index.insert(
                stem.to_owned(),
                Entry {
                    file: file.clone(),
                    bytes: size,
                    used,
                    touched: Some(Instant::now()),
                },
            );
            index.stats.downloads += 1;
            index.stats.downloaded_bytes += size;
            if index.total > self.limit {
                index.evict_to(self.limit / 10 * 9)
            } else {
                Vec::new()
            }
        };
        if !gone.is_empty() {
            self.runtime.spawn_blocking(move || remove_files(gone));
        }
        Ok(file)
    }

    pub fn stats(&self) -> CoverStats {
        let index = self.index();
        CoverStats {
            files: index.entries.len() as u64,
            bytes: index.total,
            ..index.stats
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_round_up_to_steps() {
        assert_eq!(size_bucket(0), 60);
        assert_eq!(size_bucket(60), 60);
        assert_eq!(size_bucket(61), 120);
        assert_eq!(size_bucket(160), 180);
        assert_eq!(size_bucket(5000), 1200);
    }

    #[test]
    fn google_pictures_are_asked_at_the_shown_size() {
        assert_eq!(
            sized_url(
                "https://lh3.googleusercontent.com/abc=w544-h544-l90-rj",
                180,
                180
            ),
            "https://lh3.googleusercontent.com/abc=w180-h180-l90-rj"
        );
        assert_eq!(
            sized_url(
                "https://yt3.googleusercontent.com/abc=w120-h120-p-l90-rj",
                60,
                60
            ),
            "https://yt3.googleusercontent.com/abc=w60-h60-p-l90-rj"
        );
        assert_eq!(
            sized_url("https://yt3.ggpht.com/abc=s576", 240, 240),
            "https://yt3.ggpht.com/abc=s240"
        );
        // The `s` flag alone is not a size.
        assert_eq!(
            sized_url(
                "https://yt3.googleusercontent.com/abc=w226-h226-s-l90-rj",
                120,
                120
            ),
            "https://yt3.googleusercontent.com/abc=w120-h120-s-l90-rj"
        );
    }

    #[test]
    fn banners_keep_their_shape_and_cover_the_box() {
        let banner = "https://lh3.googleusercontent.com/abc=w1500-h844-l90-rj";
        // A wide box: the width decides.
        assert_eq!(
            sized_url(banner, 600, 120),
            "https://lh3.googleusercontent.com/abc=w600-h338-l90-rj"
        );
        // A square box: the height decides, the sides are cropped.
        assert_eq!(
            sized_url(banner, 180, 180),
            "https://lh3.googleusercontent.com/abc=w320-h180-l90-rj"
        );
    }

    #[test]
    fn other_pictures_stay_as_they_are() {
        let unknown = "https://yt3.ggpht.com/abc=xyz";
        assert_eq!(sized_url(unknown, 120, 120), unknown);
        let mix = "https://music.youtube.com/image/mixart?r=abc";
        assert_eq!(sized_url(mix, 180, 180), mix);
        let png = "https://www.gstatic.com/youtube/media/ytm/images/pbg/x.png";
        assert_eq!(sized_url(png, 180, 180), png);
    }

    #[test]
    fn small_video_pictures_come_without_bars() {
        let video = "https://i.ytimg.com/vi/VIDEO/hq720.jpg?sqp=abc&rs=def";
        assert_eq!(
            sized_url(video, 300, 180),
            "https://i.ytimg.com/vi/VIDEO/mqdefault.jpg"
        );
        assert_eq!(
            sized_url(video, 60, 60),
            "https://i.ytimg.com/vi/VIDEO/mqdefault.jpg"
        );
        assert_eq!(sized_url(video, 480, 300), video);
        // A playlist's picture has no smaller form.
        let playlist = "https://i.ytimg.com/pl_c/PL1/abc.jpg?sqp=x";
        assert_eq!(sized_url(playlist, 300, 180), playlist);
    }

    #[test]
    fn pictures_are_told_by_type_or_bytes() {
        assert_eq!(extension(Some("image/jpeg"), b""), Some("jpg"));
        assert_eq!(extension(Some("image/webp; q=1"), b""), Some("webp"));
        assert_eq!(extension(None, b"\x89PNG\r\n\x1a\nrest"), Some("png"));
        assert_eq!(extension(Some("text/html"), b"<html>"), None);
    }

    #[test]
    fn the_least_recently_used_go_first() {
        let mut index = Index::default();
        for (stem, used) in [("a", 1), ("b", 2), ("c", 3)] {
            let entry = Entry {
                file: PathBuf::from(stem),
                bytes: 10,
                used,
                touched: None,
            };
            index.insert(stem.to_owned(), entry);
        }
        index.entries.get_mut("a").expect("a").used = 4;
        assert_eq!(index.evict_to(20), [PathBuf::from("b")]);
        assert_eq!(index.total, 20);
        assert_eq!(index.evict_to(20), Vec::<PathBuf>::new());
    }

    #[test]
    fn the_scan_orders_by_time_and_drops_half_written_files() {
        let dir = tempfile::tempdir().expect("temp dir");
        let old = dir.path().join(format!("{}.jpg", "a".repeat(40)));
        let new = dir.path().join(format!("{}.png", "b".repeat(40)));
        std::fs::write(&old, b"old").expect("write");
        std::fs::write(&new, b"newer").expect("write");
        let past = SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::open(&old)
            .and_then(|f| f.set_modified(past))
            .expect("set time");
        std::fs::write(dir.path().join("x.part"), b"half").expect("write");
        std::fs::write(dir.path().join("notes.txt"), b"other").expect("write");

        let found = scan(dir.path());
        let names: Vec<(&str, u64)> = found.iter().map(|f| (&f.stem[..1], f.bytes)).collect();
        assert_eq!(names, [("a", 3), ("b", 5)]);
        assert!(!dir.path().join("x.part").exists());
        assert!(dir.path().join("notes.txt").exists());
    }

    /// Serves `body` as a JPEG to every request, counting them.
    async fn picture_server(body: Vec<u8>) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let served = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&served);
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut request = [0u8; 2048];
                let _ = socket.read(&mut request).await;
                counter.fetch_add(1, Ordering::SeqCst);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            }
        });
        (format!("http://{address}"), served)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn covers_are_downloaded_once_and_the_limit_holds() {
        use std::sync::atomic::Ordering;

        let mut body = vec![0xff, 0xd8, 0xff];
        body.resize(1000, 0);
        let (server, served) = picture_server(body).await;
        let dir = tempfile::tempdir().expect("temp dir");
        let cache = CoverCache::open(dir.path().to_owned(), 2500).expect("open");

        let first = format!("{server}/1.jpg");
        assert_eq!(cache.cached(&first, 100, 100), None);
        let file = cache.fetch(&first, 100, 100).await.expect("fetch");
        assert_eq!(file.extension().and_then(|e| e.to_str()), Some("jpg"));
        assert_eq!(std::fs::read(&file).expect("read").len(), 1000);
        // Sizes within one step share the file.
        assert_eq!(cache.cached(&first, 110, 101), Some(file.clone()));
        assert_eq!(cache.fetch(&first, 100, 100).await.expect("again"), file);
        assert_eq!(served.load(Ordering::SeqCst), 1);

        // A third picture goes over the limit: the least recently used one,
        // the second, leaves.
        let second = format!("{server}/2.jpg");
        let second_file = cache.fetch(&second, 100, 100).await.expect("second");
        assert!(cache.cached(&first, 100, 100).is_some());
        cache
            .fetch(&format!("{server}/3.jpg"), 100, 100)
            .await
            .expect("third");
        let stats = cache.stats();
        assert_eq!((stats.downloads, stats.files, stats.evicted), (3, 2, 1));
        assert!(stats.bytes <= 2500);
        assert_eq!(cache.cached(&second, 100, 100), None);
        for _ in 0..50 {
            if !second_file.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!second_file.exists());
        assert!(file.exists());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn one_cover_asked_for_at_once_is_downloaded_once() {
        use std::sync::atomic::Ordering;

        let mut body = vec![0xff, 0xd8, 0xff];
        body.resize(1000, 0);
        let (server, served) = picture_server(body).await;
        let dir = tempfile::tempdir().expect("temp dir");
        let cache = CoverCache::open(dir.path().to_owned(), 1 << 20).expect("open");
        // The player bar and the glass ask for the cover of a track that
        // starts at the same moment, in the same size bucket.
        let url = format!("{server}/same.jpg");
        let asking: Vec<_> = (0..6)
            .map(|_| {
                let cache = Arc::clone(&cache);
                let url = url.clone();
                tokio::spawn(async move { cache.fetch(&url, 100, 100).await })
            })
            .collect();
        let mut files = Vec::new();
        for asked in asking {
            files.push(
                asked
                    .await
                    .expect("task")
                    .expect("every request gets the file"),
            );
        }
        assert_eq!(served.load(Ordering::SeqCst), 1, "one download");
        assert!(files.iter().all(|f| f == &files[0]));
        assert_eq!(std::fs::read(&files[0]).expect("read").len(), 1000);
        let stats = cache.stats();
        assert_eq!((stats.downloads, stats.failures, stats.files), (1, 0, 1));
        assert!(cache.fetching.lock().unwrap().is_empty(), "no slot left");
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .expect("list")
            .map(|e| e.expect("entry").file_name())
            .collect();
        assert_eq!(left.len(), 1, "no temporary file left: {left:?}");
    }

    #[test]
    fn file_urls_are_escaped() {
        assert_eq!(
            file_url(Path::new("/home/a b/c.jpg")),
            "file:///home/a%20b/c.jpg"
        );
    }
}
