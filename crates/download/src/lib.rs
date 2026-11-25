//! Downloads tracks (Opus without re-encoding, or MP3/M4A) and writes their
//! tags: basic tags, cover, lyrics, loudness, the track link (see [`tags`])
//! and what MusicBrainz knows of the track (see [`musicbrainz`]). Does not
//! depend on Qt.
//!
//! A track goes to `<music folder>/<name by the template>.<ext>`, by
//! default `<Album artist>/<Year> - <Album>/<NN> - <Title>` (see
//! [`naming`]). The stream is fetched next to it into a hidden part file
//! while the cover loads, ffmpeg writes a hidden temporary file, measures
//! its loudness, the tags go in, and only then the file takes the track's
//! name; a track whose file is already there is not fetched again.
//! MusicBrainz, one request a second, is asked afterwards
//! ([`Downloader::add_musicbrainz`]), so that it never holds a file up.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use stream::Stream;

mod convert;
mod cover;
mod fetch;
pub mod format;
pub mod loudness;
pub mod musicbrainz;
pub mod naming;
pub mod tags;

pub use cover::COVER_SIZE;
pub use format::{Bitrates, Format};
pub use loudness::Loudness;
pub use musicbrainz::{Identified, MusicBrainz, ReleaseMatch};
pub use naming::{AlbumMeta, DEFAULT_TEMPLATE, Template, TemplateError, TrackMeta};
pub use tags::track_link;

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    /// The stream could not be fetched; the URL is never in the text.
    #[error("the stream could not be fetched: {0}")]
    Http(String),
    #[error("ffmpeg is not available: {0}")]
    NoFfmpeg(String),
    #[error("ffmpeg failed: {0}")]
    Ffmpeg(String),
    /// The tags could not be written; the file is not kept, so that the
    /// next try downloads it again.
    #[error("the tags could not be written: {0}")]
    Tags(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// What the tags take besides the track's name, looked up by the caller.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extras {
    /// Cover pictures to try in this order; the first that loads goes in,
    /// as a square of at most [`COVER_SIZE`].
    pub covers: Vec<String>,
    pub lyrics: Option<String>,
}

/// What became of a download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The track was saved to this file.
    Saved(PathBuf),
    /// A file of the track was already there and is kept.
    AlreadyThere(PathBuf),
}

/// Files a download leaves next to the track until it ends; removed when
/// it fails or is dropped half-way.
struct Leftovers(Vec<PathBuf>);

impl Drop for Leftovers {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The loudness for the tags: ffmpeg's measure of the file, none for a
/// silent one; YouTube's measure (LUFS, without a peak) only when ffmpeg
/// could not measure the file — `youtube` is not even started otherwise.
async fn tagged_loudness(
    measured: Result<Option<Loudness>, DownloadError>,
    youtube: impl Future<Output = Option<f64>>,
) -> Option<Loudness> {
    match measured {
        Ok(measured) => measured,
        Err(_) => youtube.await.map(|integrated| Loudness {
            integrated,
            peak: None,
        }),
    }
}

/// `dir/.<name>.<suffix>`: hidden, so file managers and players skip it.
fn hidden(dir: &Path, name: &str, suffix: &str) -> PathBuf {
    dir.join(format!(".{name}.{suffix}"))
}

/// Saves tracks under a music folder in one format.
#[derive(Debug, Clone)]
pub struct Downloader {
    http: reqwest::Client,
    root: PathBuf,
    format: Format,
    template: Template,
    bitrates: Bitrates,
    /// Asked for each saved track, when set.
    musicbrainz: Option<Arc<MusicBrainz>>,
}

impl Downloader {
    /// Saves into `root` (the music folder) in `format`.
    pub fn new(root: PathBuf, format: Format) -> Self {
        Self {
            http: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(20))
                .read_timeout(std::time::Duration::from_secs(60))
                .build()
                .unwrap_or_default(),
            root,
            format,
            template: Template::default(),
            bitrates: Bitrates::default(),
            musicbrainz: None,
        }
    }

    /// Names tracks by `template` instead of the default one.
    pub fn with_template(mut self, template: Template) -> Self {
        self.template = template;
        self
    }

    /// Encodes MP3 and AAC at `bitrates`.
    pub fn with_bitrates(mut self, bitrates: Bitrates) -> Self {
        self.bitrates = bitrates;
        self
    }

    /// Also writes what `musicbrainz` knows of each track.
    pub fn with_musicbrainz(mut self, musicbrainz: Arc<MusicBrainz>) -> Self {
        self.musicbrainz = Some(musicbrainz);
        self
    }

    pub fn format(&self) -> Format {
        self.format
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where the track goes in the asked format.
    pub fn target(&self, meta: &TrackMeta) -> PathBuf {
        let mut path = self.root.join(self.template.path(meta));
        path.as_mut_os_string().push(".");
        path.as_mut_os_string().push(self.format.extension());
        path
    }

    /// The track's file when it is already there.
    pub fn existing(&self, meta: &TrackMeta) -> Option<PathBuf> {
        let target = self.target(meta);
        target.exists().then_some(target)
    }

    /// Saves the track from `stream` with its tags; `progress` gets the
    /// share done, from 0 to 1. `youtube_loudness` gives YouTube's measure
    /// of the track's loudness (LUFS), awaited only when ffmpeg cannot
    /// measure the file. Dropping the future stops the download and
    /// removes its files.
    pub async fn download(
        &self,
        meta: &TrackMeta,
        extras: &Extras,
        stream: &Stream,
        youtube_loudness: impl Future<Output = Option<f64>> + Send,
        progress: &mut (dyn FnMut(f64) + Send),
    ) -> Result<Outcome, DownloadError> {
        self.download_in(
            meta,
            extras,
            stream,
            youtube_loudness,
            fetch::CHUNK,
            progress,
        )
        .await
    }

    async fn download_in(
        &self,
        meta: &TrackMeta,
        extras: &Extras,
        stream: &Stream,
        youtube_loudness: impl Future<Output = Option<f64>> + Send,
        chunk: u64,
        progress: &mut (dyn FnMut(f64) + Send),
    ) -> Result<Outcome, DownloadError> {
        let recipe = format::recipe_at(
            self.format,
            &stream.codec,
            format::aac_encoder(),
            self.bitrates,
        );
        let mut target = self.root.join(self.template.path(meta));
        target.as_mut_os_string().push(".");
        target.as_mut_os_string().push(recipe.extension);
        if target.exists() {
            return Ok(Outcome::AlreadyThere(target));
        }
        let dir = target.parent().unwrap_or(&self.root).to_owned();
        tokio::fs::create_dir_all(&dir).await?;
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let part = hidden(&dir, &name, "part");
        let written = hidden(&dir, &name, &format!("tmp.{}", recipe.extension));
        let _leftovers = Leftovers(vec![part.clone(), written.clone()]);

        // Fetching is most of the work unless the audio is encoded anew;
        // measuring the loudness decodes the whole file once more.
        let (fetch_share, convert_share) = if recipe.transcodes {
            (0.5, 0.4)
        } else {
            (0.85, 0.05)
        };
        let measure_from = fetch_share + convert_share;
        let mut fetch_progress = |done: u64, total: Option<u64>| {
            if let Some(total) = total.filter(|t| *t > 0) {
                progress(fetch_share * done as f64 / total as f64);
            }
        };
        let (fetched, cover) = tokio::join!(
            fetch::fetch(&self.http, stream, &part, chunk, &mut fetch_progress),
            cover::fetch(&self.http, &extras.covers),
        );
        fetched?;
        convert::convert(&part, &written, &recipe, stream.duration, &mut |share| {
            progress(fetch_share + convert_share * share);
        })
        .await?;
        // A silent track gets no gain; one ffmpeg cannot measure gets
        // YouTube's measure.
        let measured = loudness::measure(&written, stream.duration, &mut |share| {
            progress(measure_from + (1.0 - measure_from) * share);
        })
        .await;
        let loudness = tagged_loudness(measured, youtube_loudness).await;
        let (path, meta, lyrics) = (written.clone(), meta.clone(), extras.lyrics.clone());
        tokio::task::spawn_blocking(move || {
            let contents = tags::Contents {
                lyrics: lyrics.as_deref(),
                cover: cover.as_ref(),
                loudness,
                musicbrainz: None,
            };
            tags::write(&path, &meta, &contents)
        })
        .await
        .map_err(|e| DownloadError::Tags(e.to_string()))??;
        progress(1.0);
        tokio::fs::rename(&written, &target).await?;
        Ok(Outcome::Saved(target))
    }

    /// Measures the album whose tracks are `files`, all of them, as one
    /// (see [`loudness::measure_album`]) and sets its gain in each file,
    /// each replaced whole, as [`Downloader::add_musicbrainz`] does.
    /// Returns the album's loudness, `None` for silence.
    pub async fn add_album_loudness(
        &self,
        files: &[PathBuf],
    ) -> Result<Option<Loudness>, DownloadError> {
        let Some(loudness) = loudness::measure_album(files).await? else {
            return Ok(None);
        };
        for path in files {
            rewrite(path, &self.root, "gain", move |copy| {
                tags::add_album_loudness(copy, &loudness)
            })
            .await?;
        }
        Ok(Some(loudness))
    }

    /// Whether MusicBrainz is asked for the saved tracks: then
    /// [`Downloader::add_musicbrainz`] is to follow each.
    pub fn asks_musicbrainz(&self) -> bool {
        self.musicbrainz.is_some()
    }

    /// Adds what MusicBrainz knows of the track to its saved file at
    /// `path`. `length` is the audio's length in seconds, read from the
    /// file when not given. Returns whether MusicBrainz knew the track;
    /// when it does not or cannot be reached, the file keeps YouTube
    /// Music's tags. The file is replaced whole: one cut half-way stays as
    /// it was. Calls wait their turn with the other downloads', in order.
    pub async fn add_musicbrainz(
        &self,
        path: &Path,
        meta: &TrackMeta,
        length: Option<f64>,
    ) -> Result<bool, DownloadError> {
        let Some(musicbrainz) = &self.musicbrainz else {
            return Ok(false);
        };
        let length = match length {
            Some(length) => Some(length),
            None => {
                let file = path.to_owned();
                tokio::task::spawn_blocking(move || tags::length(&file))
                    .await
                    .ok()
                    .flatten()
            }
        };
        let Some(found) = musicbrainz.identify(meta, length).await.ok().flatten() else {
            return Ok(false);
        };
        rewrite(path, &self.root, "mb", move |copy| {
            tags::add_musicbrainz(copy, &found)
        })
        .await?;
        Ok(true)
    }
}

/// Changes the tags of the saved file at `path` by `change`, made on a
/// hidden copy next to it that then takes the file's place: a change cut
/// half-way leaves the file as it was. `what` names the copy.
async fn rewrite(
    path: &Path,
    root: &Path,
    what: &str,
    change: impl FnOnce(&Path) -> Result<(), DownloadError> + Send + 'static,
) -> Result<(), DownloadError> {
    let dir = path.parent().unwrap_or(root);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default();
    let copy = hidden(dir, &name, &format!("{what}.{extension}"));
    let _leftovers = Leftovers(vec![copy.clone()]);
    tokio::fs::copy(path, &copy).await?;
    let changed = copy.clone();
    tokio::task::spawn_blocking(move || change(&changed))
        .await
        .map_err(|e| DownloadError::Tags(e.to_string()))??;
    tokio::fs::rename(&copy, path).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use lofty::prelude::*;

    use super::*;
    use crate::convert::tests::{opus_tone, probe};
    use crate::fetch::tests::{range_server, stream_at};

    fn meta() -> TrackMeta {
        TrackMeta {
            video_id: "v".into(),
            title: "Tone".into(),
            artists: vec!["Lab".into()],
            album: Some(AlbumMeta {
                title: "Tests".into(),
                artists: vec!["Lab".into()],
                year: Some(2026),
                track_number: Some(1),
                track_count: Some(1),
            }),
        }
    }

    #[tokio::test]
    async fn a_track_is_saved_whole_and_not_fetched_twice() {
        let work = tempfile::tempdir().unwrap();
        let Some(tone) = opus_tone(work.path()) else {
            eprintln!("ffmpeg is missing; skipping");
            return;
        };
        let (address, requests) = range_server(std::fs::read(&tone).unwrap()).await;
        let (pictures, _) = range_server(crate::cover::tests::jpeg(1280, 720)).await;
        let extras = Extras {
            covers: vec![format!("http://{pictures}/maxresdefault.jpg")],
            lyrics: Some("A line\nAnother line".into()),
        };
        // The tone is measured: YouTube is never asked.
        let asked = std::sync::atomic::AtomicBool::new(false);
        let youtube = || async {
            asked.store(true, std::sync::atomic::Ordering::SeqCst);
            Some(-7.0)
        };
        let music = tempfile::tempdir().unwrap();
        for format in Format::ALL {
            let downloader = Downloader::new(music.path().to_owned(), format);
            let mut stream = stream_at(format!("http://{address}/stream"), "opus");
            stream.duration = Some(3.0);
            let mut shares = Vec::new();
            let saved = downloader
                .download_in(&meta(), &extras, &stream, youtube(), 4096, &mut |s| {
                    shares.push(s)
                })
                .await
                .unwrap();
            let expected = music
                .path()
                .join(format!("Lab/2026 - Tests/01 - Tone.{}", format.extension()));
            assert_eq!(saved, Outcome::Saved(expected.clone()));
            assert_eq!(downloader.existing(&meta()), Some(expected.clone()));
            assert!(shares.windows(2).all(|w| w[0] <= w[1]), "{shares:?}");
            assert_eq!(shares.last(), Some(&1.0));
            assert!(!probe(&expected).0.is_empty());
            // The tags are in: the cover cut to a square, the measured gain.
            let file = lofty::read_from_path(&expected).unwrap();
            let tag = file.primary_tag().unwrap();
            assert_eq!(tag.title().as_deref(), Some("Tone"));
            assert_eq!(tag.pictures().len(), 1, "{format:?}");
            let gain = if format == Format::Opus {
                lofty::tag::ItemKey::R128TrackGain
            } else {
                lofty::tag::ItemKey::ReplayGainTrackGain
            };
            assert!(tag.get_string(gain).is_some(), "{format:?}");

            let before = requests.load(std::sync::atomic::Ordering::SeqCst);
            let again = downloader
                .download_in(&meta(), &extras, &stream, youtube(), 4096, &mut |_| {})
                .await
                .unwrap();
            assert_eq!(again, Outcome::AlreadyThere(expected));
            assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), before);
        }
        // Only the three tracks: no part or temporary files are left.
        let album = music.path().join("Lab/2026 - Tests");
        let mut names: Vec<_> = std::fs::read_dir(&album)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["01 - Tone.m4a", "01 - Tone.mp3", "01 - Tone.opus"]);
        assert!(!asked.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn the_file_does_not_wait_for_musicbrainz() {
        use crate::musicbrainz::tests::{client, service, set_you_free};
        use lofty::tag::ItemKey;

        let work = tempfile::tempdir().unwrap();
        let Some(tone) = opus_tone(work.path()) else {
            eprintln!("ffmpeg is missing; skipping");
            return;
        };
        let (address, _) = range_server(std::fs::read(&tone).unwrap()).await;
        // A MusicBrainz that takes requests and never answers.
        let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let silent_at = silent.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = silent.accept().await {
                held.push(socket);
            }
        });
        let cache = tempfile::tempdir().unwrap();
        let music = tempfile::tempdir().unwrap();
        let (meta, length) = set_you_free();
        let downloader = Downloader::new(music.path().to_owned(), Format::Opus)
            .with_musicbrainz(Arc::new(client(silent_at, cache.path())));
        assert!(downloader.asks_musicbrainz());
        let mut stream = stream_at(format!("http://{address}/stream"), "opus");
        stream.duration = Some(3.0);
        let saved = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            downloader.download(
                &meta,
                &Extras::default(),
                &stream,
                async { None },
                &mut |_| {},
            ),
        )
        .await
        .expect("the file waited for MusicBrainz")
        .unwrap();
        let Outcome::Saved(path) = saved else {
            panic!("not saved: {saved:?}");
        };
        let tags = |path: &Path| {
            let file = lofty::read_from_path(path).unwrap();
            file.primary_tag().unwrap().clone()
        };
        let before = tags(&path);
        assert_eq!(before.title().as_deref(), Some("Set You Free"));
        assert!(before.get_string(ItemKey::MusicBrainzRecordingId).is_none());

        // MusicBrainz's turn: its tags join the file's, which stay.
        let (recorded, _) = service(0).await;
        let downloader = downloader.with_musicbrainz(Arc::new(client(recorded, cache.path())));
        for _ in 0..2 {
            assert!(
                downloader
                    .add_musicbrainz(&path, &meta, length)
                    .await
                    .unwrap()
            );
        }
        let after = tags(&path);
        assert_eq!(
            after
                .get_items(ItemKey::MusicBrainzRecordingId)
                .map(|i| i.value().text().unwrap_or_default().to_owned())
                .collect::<Vec<_>>(),
            ["5bf8efa0-651a-4cd3-9590-ec4b08e270d4"],
            "once, after two passes"
        );
        assert_eq!((after.track(), after.disk()), (Some(10), Some(1)));
        assert_eq!(after.title().as_deref(), Some("Set You Free"));
        assert_eq!(after.comment(), before.comment());
        assert!(after.get_string(ItemKey::R128TrackGain).is_some());
        assert_eq!(
            after.get_strings(ItemKey::Genre).collect::<Vec<_>>(),
            ["drum and bass"]
        );
        // A track MusicBrainz does not know: the file stays as it is.
        let mut unknown = meta.clone();
        unknown.title = "Nothing like it".into();
        assert!(
            !downloader
                .add_musicbrainz(&path, &unknown, length)
                .await
                .unwrap()
        );
        assert_eq!(
            tags(&path).get_string(ItemKey::MusicBrainzRecordingId),
            after.get_string(ItemKey::MusicBrainzRecordingId)
        );
        // Only the track is in its folder: no copy is left.
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
    }

    /// A stereo tone of `seconds` at `volume` (dB) in `format`, written by
    /// ffmpeg as the downloader would, with the track tags in; `None`
    /// without ffmpeg.
    async fn album_track(dir: &Path, name: &str, volume: i32, format: Format) -> Option<PathBuf> {
        let path = dir.join(format!("{name}.{}", format.extension()));
        let codec = match format {
            Format::Opus => "libopus",
            Format::Mp3 => "libmp3lame",
            Format::M4a => "aac",
        };
        let made = tokio::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg(format!(
                "sine=frequency={}:duration=2",
                300 + volume.abs() * 20
            ))
            .args([
                "-af",
                &format!("volume={volume}dB"),
                "-ac",
                "2",
                "-c:a",
                codec,
            ])
            .arg(&path)
            .status()
            .await
            .ok()?;
        if !made.success() {
            return None;
        }
        let tagged = path.clone();
        tokio::task::spawn_blocking(move || {
            tags::write(&tagged, &meta(), &tags::Contents::default())
        })
        .await
        .unwrap()
        .unwrap();
        Some(path)
    }

    #[tokio::test]
    async fn an_album_gets_one_gain_measured_over_all_its_tracks() {
        use lofty::tag::ItemKey;

        for format in [Format::Opus, Format::Mp3] {
            let dir = tempfile::tempdir().unwrap();
            let mut files = Vec::new();
            for (name, volume) in [("a", -3), ("b", -12), ("c", -20)] {
                let Some(file) = album_track(dir.path(), name, volume, format).await else {
                    eprintln!("ffmpeg is missing; skipping");
                    return;
                };
                files.push(file);
            }
            let downloader = Downloader::new(dir.path().to_owned(), format);
            let album = downloader
                .add_album_loudness(&files)
                .await
                .unwrap()
                .unwrap();

            // ffmpeg's own measure of the three as one, read another way:
            // its concat demuxer.
            let list = dir.path().join("list.txt");
            std::fs::write(
                &list,
                files
                    .iter()
                    .map(|f| format!("file '{}'\n", f.display()))
                    .collect::<String>(),
            )
            .unwrap();
            let out = tokio::process::Command::new("ffmpeg")
                .args([
                    "-hide_banner",
                    "-nostdin",
                    "-f",
                    "concat",
                    "-safe",
                    "0",
                    "-i",
                ])
                .arg(&list)
                .args(["-af", "ebur128=peak=true:framelog=quiet", "-f", "null", "-"])
                .output()
                .await
                .unwrap();
            let expected = loudness::parse(&String::from_utf8_lossy(&out.stderr)).unwrap();
            assert!(
                (album.integrated - expected.integrated).abs() < 0.11,
                "{album:?} {expected:?}"
            );
            // Louder than the quietest track, quieter than the loudest.
            assert!(
                -30.0 < album.integrated && album.integrated < -5.0,
                "{album:?}"
            );

            let gains: Vec<_> = files
                .iter()
                .map(|f| {
                    let file = lofty::read_from_path(f).unwrap();
                    let tag = file.primary_tag().unwrap();
                    assert_eq!(tag.title().as_deref(), Some("Tone"), "the rest stays");
                    if format == Format::Opus {
                        tag.get_string(ItemKey::R128AlbumGain).map(str::to_owned)
                    } else {
                        tag.get_string(ItemKey::ReplayGainAlbumGain)
                            .map(str::to_owned)
                    }
                })
                .collect();
            let expected_gain = if format == Format::Opus {
                album.r128_gain().to_string()
            } else {
                album.replaygain_gain()
            };
            assert_eq!(gains, vec![Some(expected_gain); 3], "{format:?}");
            // No copies are left.
            let left = std::fs::read_dir(dir.path()).unwrap().count();
            assert_eq!(left, 4, "three tracks and the list");
        }
    }

    #[tokio::test]
    async fn youtube_is_asked_for_the_loudness_only_when_ffmpeg_fails() {
        let asked = std::sync::atomic::AtomicUsize::new(0);
        let youtube = || async {
            asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Some(-7.0)
        };
        let measured = Loudness {
            integrated: -9.1,
            peak: Some(0.4),
        };
        assert_eq!(
            tagged_loudness(Ok(Some(measured)), youtube()).await,
            Some(measured)
        );
        // Silence gets no gain, and no question either.
        assert_eq!(tagged_loudness(Ok(None), youtube()).await, None);
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 0);
        // ffmpeg failed: YouTube's measure, as the tags had it before.
        let failed = Err(DownloadError::Ffmpeg("no summary".into()));
        let from_youtube = tagged_loudness(failed, youtube()).await.unwrap();
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            from_youtube,
            Loudness {
                integrated: -7.0,
                peak: None
            }
        );
        assert_eq!(from_youtube.r128_gain(), -4096);
        assert_eq!(from_youtube.replaygain_gain(), "-11.00 dB");
        assert_eq!(from_youtube.replaygain_peak(), None);
    }

    #[tokio::test]
    async fn a_failed_download_leaves_nothing() {
        let work = tempfile::tempdir().unwrap();
        let (address, _) = range_server(b"not audio at all".to_vec()).await;
        let downloader = Downloader::new(work.path().to_owned(), Format::Opus);
        let stream = stream_at(format!("http://{address}/stream"), "opus");
        assert!(
            downloader
                .download(
                    &meta(),
                    &Extras::default(),
                    &stream,
                    async { None },
                    &mut |_| {}
                )
                .await
                .is_err()
        );
        let album = work.path().join("Lab/2026 - Tests");
        assert_eq!(std::fs::read_dir(&album).unwrap().count(), 0);
    }
}
