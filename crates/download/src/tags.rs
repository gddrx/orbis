//! The tags of a saved track, written by lofty into the file ffmpeg made:
//! title, artist, album artist, album, year, track number and count, the
//! cover, the lyrics, the track's link in YouTube Music (in the comment)
//! and the loudness gain; from MusicBrainz, when it knows the track, its
//! IDs, genres, first release date and the disc and number on the album's
//! release. Vorbis comments in Opus, ID3v2.4 in MP3, iTunes atoms in M4A,
//! under the names MusicBrainz Picard uses. Without MusicBrainz there is no
//! disc number: YouTube Music does not split albums into discs.

use std::path::Path;

use lofty::config::WriteOptions;
use lofty::picture::{Picture, PictureType};
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::{ItemValue, Tag, TagItem, TagType, items::Timestamp};

use crate::DownloadError;
use crate::cover::Cover;
use crate::loudness::Loudness;
use crate::musicbrainz::Identified;
use crate::naming::{TrackMeta, names};

/// The track's page in YouTube Music.
pub fn track_link(video_id: &str) -> String {
    format!("https://music.youtube.com/watch?v={video_id}")
}

/// What goes into the tags besides the name.
#[derive(Debug, Default)]
pub(crate) struct Contents<'a> {
    pub lyrics: Option<&'a str>,
    pub cover: Option<&'a Cover>,
    pub loudness: Option<Loudness>,
    pub musicbrainz: Option<&'a Identified>,
}

fn tags_error(error: impl std::fmt::Display) -> DownloadError {
    DownloadError::Tags(error.to_string())
}

/// The tag of `kind` for the track.
fn tag(kind: TagType, meta: &TrackMeta, contents: &Contents<'_>) -> Tag {
    let mut tag = Tag::new(kind);
    tag.set_title(meta.title.clone());
    let artist = names(&meta.artists);
    if let Some(artist) = &artist {
        tag.set_artist(artist.clone());
    }
    if let Some(album) = &meta.album {
        tag.set_album(album.title.clone());
        if let Some(album_artist) = names(&album.artists).or(artist) {
            tag.insert_text(ItemKey::AlbumArtist, album_artist);
        }
        if let Some(year) = album.year {
            tag.set_date(Timestamp {
                year,
                ..Timestamp::default()
            });
        }
        if let Some(number) = album.track_number {
            tag.set_track(number);
        }
        if let Some(count) = album.track_count {
            tag.set_track_total(count);
        }
    }
    tag.set_comment(track_link(&meta.video_id));
    if let Some(lyrics) = contents.lyrics.filter(|l| !l.trim().is_empty()) {
        // Vorbis calls them `LYRICS`, ID3v2 has only the unsynchronised
        // `USLT`; M4A takes either as `©lyr`.
        let key = if kind == TagType::VorbisComments {
            ItemKey::Lyrics
        } else {
            ItemKey::UnsyncLyrics
        };
        tag.insert_text(key, lyrics.to_owned());
    }
    if let Some(loudness) = contents.loudness {
        if kind == TagType::VorbisComments {
            tag.insert_text(ItemKey::R128TrackGain, loudness.r128_gain().to_string());
        } else {
            tag.insert_text(ItemKey::ReplayGainTrackGain, loudness.replaygain_gain());
            if let Some(peak) = loudness.replaygain_peak() {
                tag.insert_text(ItemKey::ReplayGainTrackPeak, peak);
            }
        }
    }
    if let Some(found) = contents.musicbrainz {
        musicbrainz(&mut tag, found);
    }
    if let Some(cover) = contents.cover {
        tag.push_picture(
            Picture::unchecked(cover.bytes.clone())
                .pic_type(PictureType::CoverFront)
                .mime_type(cover.mime.clone())
                .build(),
        );
    }
    tag
}

/// Adds what MusicBrainz knows: its IDs, genres, the first release date and,
/// on the album's release, the disc and the place on it (instead of YouTube
/// Music's running number).
fn musicbrainz(tag: &mut Tag, found: &Identified) {
    let mut push = |key: ItemKey, value: &str| {
        tag.push(TagItem::new(key, ItemValue::Text(value.to_owned())));
    };
    push(ItemKey::MusicBrainzRecordingId, &found.recording_id);
    for id in &found.artist_ids {
        push(ItemKey::MusicBrainzArtistId, id);
    }
    for genre in &found.genres {
        push(ItemKey::Genre, genre);
    }
    if let Some(date) = &found.original_date {
        push(ItemKey::OriginalReleaseDate, date);
    }
    let Some(release) = &found.release else {
        return;
    };
    push(ItemKey::MusicBrainzReleaseId, &release.release_id);
    if let Some(id) = &release.release_group_id {
        push(ItemKey::MusicBrainzReleaseGroupId, id);
    }
    if let Some(id) = &release.track_id {
        push(ItemKey::MusicBrainzTrackId, id);
    }
    for id in &release.artist_ids {
        push(ItemKey::MusicBrainzReleaseArtistId, id);
    }
    if let Some(number) = release.track_number {
        tag.set_track(number);
    }
    if let Some(count) = release.track_count {
        tag.set_track_total(count);
    }
    if let Some(disc) = release.disc_number {
        tag.set_disk(disc);
    }
    if let Some(count) = release.disc_count {
        tag.set_disk_total(count);
    }
}

/// MusicBrainz's keys, cleared before its tags go in again.
const MUSICBRAINZ_KEYS: [ItemKey; 8] = [
    ItemKey::MusicBrainzRecordingId,
    ItemKey::MusicBrainzArtistId,
    ItemKey::Genre,
    ItemKey::OriginalReleaseDate,
    ItemKey::MusicBrainzReleaseId,
    ItemKey::MusicBrainzReleaseGroupId,
    ItemKey::MusicBrainzTrackId,
    ItemKey::MusicBrainzReleaseArtistId,
];

/// Adds what MusicBrainz knows to the tags already in `path`, in place of
/// what an earlier pass added; the rest stays. Blocks: call off the async
/// threads.
pub(crate) fn add_musicbrainz(path: &Path, found: &Identified) -> Result<(), DownloadError> {
    let mut file = Probe::open(path)
        .map_err(tags_error)?
        .guess_file_type()?
        .read()
        .map_err(tags_error)?;
    let kind = file.primary_tag_type();
    if file.tag(kind).is_none() {
        file.insert_tag(Tag::new(kind));
    }
    let tag = file.tag_mut(kind).ok_or_else(|| tags_error("no tag"))?;
    for key in MUSICBRAINZ_KEYS {
        tag.remove_key(key);
    }
    musicbrainz(tag, found);
    tag.save_to_path(path, WriteOptions::default())
        .map_err(tags_error)
}

/// Sets the album's loudness gain in the tags already in `path`:
/// `R128_ALBUM_GAIN` in Opus, `REPLAYGAIN_ALBUM_GAIN` and `_PEAK` in MP3
/// and M4A; the rest stays. Blocks: call off the async threads.
pub(crate) fn add_album_loudness(path: &Path, loudness: &Loudness) -> Result<(), DownloadError> {
    let mut file = Probe::open(path)
        .map_err(tags_error)?
        .guess_file_type()?
        .read()
        .map_err(tags_error)?;
    let kind = file.primary_tag_type();
    if file.tag(kind).is_none() {
        file.insert_tag(Tag::new(kind));
    }
    let tag = file.tag_mut(kind).ok_or_else(|| tags_error("no tag"))?;
    if kind == TagType::VorbisComments {
        tag.insert_text(ItemKey::R128AlbumGain, loudness.r128_gain().to_string());
    } else {
        tag.insert_text(ItemKey::ReplayGainAlbumGain, loudness.replaygain_gain());
        match loudness.replaygain_peak() {
            Some(peak) => {
                tag.insert_text(ItemKey::ReplayGainAlbumPeak, peak);
            }
            None => tag.remove_key(ItemKey::ReplayGainAlbumPeak),
        }
    }
    tag.save_to_path(path, WriteOptions::default())
        .map_err(tags_error)
}

/// The audio's length in seconds, as the file says.
pub(crate) fn length(path: &Path) -> Option<f64> {
    let file = Probe::open(path)
        .ok()?
        .guess_file_type()
        .ok()?
        .read()
        .ok()?;
    let length = file.properties().duration().as_secs_f64();
    (length > 0.0).then_some(length)
}

/// Writes the track's tags into `path`, replacing what ffmpeg left; the
/// file's kind decides the tag format. Blocks: call off the async threads.
pub(crate) fn write(
    path: &Path,
    meta: &TrackMeta,
    contents: &Contents<'_>,
) -> Result<(), DownloadError> {
    let kind = Probe::open(path)
        .map_err(tags_error)?
        .guess_file_type()?
        .file_type()
        .ok_or_else(|| tags_error("unknown file type"))?
        .primary_tag_type();
    tag(kind, meta, contents)
        .save_to_path(path, WriteOptions::default())
        .map_err(tags_error)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use lofty::picture::MimeType;
    use serde_json::Value;

    use super::*;
    use crate::convert::convert;
    use crate::convert::tests::opus_tone;
    use crate::cover::tests::jpeg;
    use crate::format::{Format, aac_encoder, recipe};
    use crate::naming::AlbumMeta;

    const LYRICS: &str = "First line of the song\nSecond line";

    fn meta(album: bool) -> TrackMeta {
        TrackMeta {
            video_id: "abcdefghijk".into(),
            title: "Set You Free".into(),
            artists: vec!["T95".into(), "Operate".into()],
            album: album.then(|| AlbumMeta {
                title: "Point of Origin, Vol. 4".into(),
                artists: vec!["Various Artists".into()],
                year: Some(2021),
                track_number: Some(10),
                track_count: Some(12),
            }),
        }
    }

    /// What ffprobe reads: the tags of the file and its streams (keys in
    /// lower case), and the size of the attached picture.
    fn probe(path: &Path) -> (HashMap<String, String>, Option<(u64, u64)>) {
        let out = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-show_format", "-show_streams"])
            .args(["-of", "json"])
            .arg(path)
            .output()
            .expect("ffprobe");
        let json: Value = serde_json::from_slice(&out.stdout).unwrap();
        let mut tags = HashMap::new();
        let mut collect = |value: Option<&Value>| {
            for (key, value) in value.and_then(Value::as_object).into_iter().flatten() {
                tags.insert(
                    key.to_lowercase(),
                    value.as_str().unwrap_or_default().to_owned(),
                );
            }
        };
        collect(json.pointer("/format/tags"));
        let streams = json["streams"].as_array().cloned().unwrap_or_default();
        let mut picture = None;
        for stream in &streams {
            if stream.pointer("/disposition/attached_pic") == Some(&Value::from(1)) {
                picture = Some((
                    stream["width"].as_u64().unwrap_or(0),
                    stream["height"].as_u64().unwrap_or(0),
                ));
            } else {
                collect(stream.get("tags"));
            }
        }
        (tags, picture)
    }

    /// A tone in `format` as the downloader makes it, or `None` without
    /// ffmpeg.
    async fn track_in(dir: &Path, format: Format) -> Option<std::path::PathBuf> {
        let tone = opus_tone(dir)?;
        let recipe = recipe(format, "opus", aac_encoder());
        let out = dir.join(format!("track.{}", recipe.extension));
        convert(&tone, &out, &recipe, Some(3.0), &mut |_| {})
            .await
            .unwrap();
        Some(out)
    }

    fn first<'a>(tags: &'a HashMap<String, String>, keys: &[&str]) -> &'a str {
        keys.iter()
            .find_map(|k| tags.get(*k))
            .map_or("", String::as_str)
    }

    #[tokio::test]
    async fn every_tag_goes_into_each_format() {
        let cover = Cover {
            bytes: jpeg(600, 600),
            mime: MimeType::Jpeg,
        };
        let loudness = Loudness {
            integrated: -9.1,
            peak: Some(0.4),
        };
        for format in Format::ALL {
            let dir = tempfile::tempdir().unwrap();
            let Some(path) = track_in(dir.path(), format).await else {
                eprintln!("ffmpeg is missing; skipping");
                return;
            };
            let contents = Contents {
                lyrics: Some(LYRICS),
                cover: Some(&cover),
                loudness: Some(loudness),
                musicbrainz: None,
            };
            write(&path, &meta(true), &contents).unwrap();

            let (tags, picture) = probe(&path);
            let keys: Vec<_> = tags.keys().collect();
            assert_eq!(
                first(&tags, &["title"]),
                "Set You Free",
                "{format:?} {keys:?}"
            );
            assert_eq!(first(&tags, &["artist"]), "T95, Operate", "{format:?}");
            assert_eq!(
                first(&tags, &["album_artist", "albumartist"]),
                "Various Artists",
                "{format:?} {keys:?}"
            );
            assert_eq!(first(&tags, &["album"]), "Point of Origin, Vol. 4");
            assert!(
                first(&tags, &["date"]).starts_with("2021"),
                "{format:?} {keys:?}"
            );
            assert!(
                first(&tags, &["track"]).starts_with("10"),
                "{format:?} {keys:?}"
            );
            assert!(
                first(&tags, &["track"]).ends_with("/12") || first(&tags, &["tracktotal"]) == "12",
                "{format:?} {keys:?}"
            );
            assert_eq!(
                first(&tags, &["comment"]),
                "https://music.youtube.com/watch?v=abcdefghijk",
                "{format:?} {keys:?}"
            );
            assert_eq!(
                first(&tags, &["lyrics", "lyrics-xxx", "lyrics-eng"]),
                LYRICS,
                "{format:?} {keys:?}"
            );
            if format == Format::Opus {
                assert_eq!(first(&tags, &["r128_track_gain"]), "-3558", "{keys:?}");
                assert!(!tags.contains_key("replaygain_track_gain"));
            } else {
                assert_eq!(
                    first(&tags, &["replaygain_track_gain"]),
                    "-8.90 dB",
                    "{format:?} {keys:?}"
                );
                assert_eq!(first(&tags, &["replaygain_track_peak"]), "1.047129");
            }
            assert!(!tags.keys().any(|k| k.starts_with("disc")), "{keys:?}");
            assert_eq!(picture, Some((600, 600)), "{format:?}");

            // lofty reads back the same picture.
            let file = lofty::read_from_path(&path).unwrap();
            let tag = file.primary_tag().unwrap();
            assert_eq!(tag.pictures().len(), 1);
            assert_eq!(tag.pictures()[0].data(), cover.bytes.as_slice());
            // MP4 keeps no picture type: its `covr` is the cover.
            if format != Format::M4a {
                assert_eq!(
                    tag.pictures()[0].pic_type(),
                    PictureType::CoverFront,
                    "{format:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_track_without_album_or_extras_gets_the_basic_tags() {
        for format in Format::ALL {
            let dir = tempfile::tempdir().unwrap();
            let Some(path) = track_in(dir.path(), format).await else {
                eprintln!("ffmpeg is missing; skipping");
                return;
            };
            write(&path, &meta(false), &Contents::default()).unwrap();
            let (tags, picture) = probe(&path);
            let keys: Vec<_> = tags.keys().collect();
            assert_eq!(first(&tags, &["title"]), "Set You Free", "{format:?}");
            assert_eq!(first(&tags, &["artist"]), "T95, Operate");
            assert!(first(&tags, &["comment"]).ends_with("abcdefghijk"));
            for absent in ["album", "album_artist", "date", "track", "lyrics"] {
                assert!(!tags.contains_key(absent), "{format:?} {absent} {keys:?}");
            }
            assert_eq!(picture, None);
        }
    }

    fn identified() -> Identified {
        Identified {
            recording_id: "9ffabbe4-e078-4906-80a7-3a02b537e251".into(),
            artist_ids: vec![
                "31b5082c-79db-4793-aa4b-9ea9a71d4405".into(),
                "24581c6a-408e-47f4-9123-35adb6d60cd3".into(),
            ],
            original_date: Some("1979-11-30".into()),
            genres: vec!["progressive rock".into(), "art rock".into()],
            release: Some(crate::musicbrainz::ReleaseMatch {
                release_id: "9c9df7ba-67c6-4a5a-b696-bd0e28a1549d".into(),
                release_group_id: Some("f2026101-945b-3d05-9ef4-aa718fc3feef".into()),
                artist_ids: vec!["83d91898-7763-47d7-b03b-b92132375c47".into()],
                track_id: Some("c351f791-7989-4381-914b-53fe368a4f87".into()),
                track_number: Some(6),
                track_count: Some(13),
                disc_number: Some(2),
                disc_count: Some(2),
            }),
        }
    }

    #[tokio::test]
    async fn musicbrainz_tags_go_into_each_format() {
        let found = identified();
        for format in Format::ALL {
            let dir = tempfile::tempdir().unwrap();
            let Some(path) = track_in(dir.path(), format).await else {
                eprintln!("ffmpeg is missing; skipping");
                return;
            };
            let contents = Contents {
                musicbrainz: Some(&found),
                ..Contents::default()
            };
            write(&path, &meta(true), &contents).unwrap();

            let file = lofty::read_from_path(&path).unwrap();
            let tag = file.primary_tag().unwrap();
            let text = |key: ItemKey| tag.get_string(key).map(str::to_owned);
            let all = |key: ItemKey| {
                tag.get_strings(key)
                    .flat_map(|v| v.split(['\0', ';', '/']))
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                text(ItemKey::MusicBrainzRecordingId).as_deref(),
                Some(found.recording_id.as_str()),
                "{format:?}"
            );
            assert_eq!(
                all(ItemKey::MusicBrainzArtistId),
                found.artist_ids,
                "{format:?}"
            );
            assert_eq!(
                all(ItemKey::Genre),
                ["progressive rock", "art rock"],
                "{format:?}"
            );
            assert_eq!(
                text(ItemKey::OriginalReleaseDate).as_deref(),
                Some("1979-11-30"),
                "{format:?}"
            );
            let release = found.release.as_ref().unwrap();
            for (key, value) in [
                (ItemKey::MusicBrainzReleaseId, &release.release_id),
                (
                    ItemKey::MusicBrainzReleaseGroupId,
                    release.release_group_id.as_ref().unwrap(),
                ),
                (
                    ItemKey::MusicBrainzTrackId,
                    release.track_id.as_ref().unwrap(),
                ),
                (ItemKey::MusicBrainzReleaseArtistId, &release.artist_ids[0]),
            ] {
                assert_eq!(
                    text(key).as_deref(),
                    Some(value.as_str()),
                    "{format:?} {key:?}"
                );
            }
            // The release's disc and place replace YouTube Music's 10 of 12.
            assert_eq!(
                (tag.track(), tag.track_total(), tag.disk(), tag.disk_total()),
                (Some(6), Some(13), Some(2), Some(2)),
                "{format:?}"
            );
            let (probed, _) = probe(&path);
            assert!(first(&probed, &["disc"]).starts_with('2'), "{format:?}");
        }
    }

    #[test]
    fn the_link_is_the_tracks_page() {
        assert_eq!(
            track_link("dQw4w9WgXcQ"),
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ"
        );
    }
}
