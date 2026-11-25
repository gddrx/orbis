//! The client against MusicBrainz's recorded answers (`fixtures/`, taken
//! on 23 November 2026 from the cache of real downloads): a local server
//! answers each recorded request with its recorded body.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::naming::{AlbumMeta, TrackMeta};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/musicbrainz/fixtures");
/// The short pause of the tests instead of MusicBrainz's second.
const TEST_GAP: Duration = Duration::from_millis(20);

/// The recorded answers by request target (`/ws/2/…?…`).
fn recorded() -> HashMap<String, String> {
    let mut answers = HashMap::new();
    for entry in std::fs::read_dir(FIXTURES).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let text = std::fs::read_to_string(path).unwrap();
        let kept: Kept<String, serde_json::Value> = serde_json::from_str(&text).unwrap();
        let target = kept
            .url
            .strip_prefix("https://musicbrainz.org")
            .unwrap()
            .to_owned();
        answers.insert(target, kept.body.to_string());
    }
    answers
}

/// What the server saw of one request.
#[derive(Debug, Clone)]
pub(crate) struct Seen {
    target: String,
    user_agent: String,
    at: Instant,
}

/// A stand-in for MusicBrainz: the recorded answers, 404 for anything
/// else; the first `busy` requests get 503.
pub(crate) async fn service(busy: usize) -> (SocketAddr, Arc<std::sync::Mutex<Vec<Seen>>>) {
    let answers = Arc::new(recorded());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let turned_away = Arc::new(AtomicUsize::new(0));
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let answers = Arc::clone(&answers);
            let log = Arc::clone(&log);
            let turned_away = Arc::clone(&turned_away);
            tokio::spawn(async move {
                let mut buffer = vec![0u8; 16384];
                let read = socket.read(&mut buffer).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buffer[..read]).into_owned();
                let target = head.split(' ').nth(1).unwrap_or_default().to_owned();
                let user_agent = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .starts_with("user-agent:")
                            .then(|| l[11..].trim().to_owned())
                    })
                    .unwrap_or_default();
                log.lock().unwrap().push(Seen {
                    target: target.clone(),
                    user_agent,
                    at: Instant::now(),
                });
                let (status, body) = if turned_away.fetch_add(1, Ordering::SeqCst) < busy {
                    (
                        "503 Service Unavailable",
                        "{\"error\": \"busy\"}".to_owned(),
                    )
                } else {
                    match answers.get(&target) {
                        Some(body) => ("200 OK", body.clone()),
                        None => ("404 Not Found", "{\"error\": \"Not Found\"}".to_owned()),
                    }
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    (address, seen)
}

pub(crate) fn client(address: SocketAddr, cache: &Path) -> MusicBrainz {
    MusicBrainz::at(
        &format!("http://{address}/ws/2"),
        Some(cache.to_owned()),
        TEST_GAP,
    )
}

/// A track as YouTube Music names it, with the length it was downloaded at.
fn track(
    title: &str,
    artists: &[&str],
    album: Option<(&str, &[&str], u32)>,
    length: f64,
) -> (TrackMeta, Option<f64>) {
    let names = |list: &[&str]| list.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
    let meta = TrackMeta {
        video_id: "v".into(),
        title: title.into(),
        artists: names(artists),
        album: album.map(|(title, artists, count)| AlbumMeta {
            title: title.into(),
            artists: names(artists),
            year: None,
            track_number: None,
            track_count: Some(count),
        }),
    };
    (meta, Some(length))
}

pub(crate) fn set_you_free() -> (TrackMeta, Option<f64>) {
    track(
        "Set You Free",
        &["T95", "Operate"],
        Some(("Point of Origin, Vol. 4", &["Various Artists"], 15)),
        292.0,
    )
}

#[tokio::test]
async fn recorded_tracks_are_identified() {
    let (address, seen) = service(0).await;
    let cache = tempfile::tempdir().unwrap();
    let musicbrainz = client(address, cache.path());

    // A compilation track: the album found, its genres from an artist.
    let (meta, length) = set_you_free();
    let found = musicbrainz.identify(&meta, length).await.unwrap().unwrap();
    assert_eq!(found.recording_id, "5bf8efa0-651a-4cd3-9590-ec4b08e270d4");
    assert_eq!(found.artist_ids.len(), 2);
    assert_eq!(found.original_date.as_deref(), Some("2021-02-26"));
    assert_eq!(found.genres, ["drum and bass"]);
    let release = found.release.clone().unwrap();
    assert_eq!(release.release_id, "364022b9-6122-4c1c-9701-93bc0a2ccac1");
    assert_eq!(
        (
            release.track_number,
            release.track_count,
            release.disc_number,
            release.disc_count
        ),
        (Some(10), Some(15), Some(1), Some(1))
    );

    // Two discs; among bootlegs and duplicates, the canonical recording.
    let (meta, length) = track(
        "Comfortably Numb",
        &["Pink Floyd"],
        Some(("The Wall", &["Pink Floyd"], 26)),
        382.0,
    );
    let found = musicbrainz.identify(&meta, length).await.unwrap().unwrap();
    assert_eq!(found.recording_id, "9ffabbe4-e078-4906-80a7-3a02b537e251");
    assert_eq!(found.original_date.as_deref(), Some("1979-11-30"));
    let release = found.release.unwrap();
    assert_eq!(
        (
            release.track_number,
            release.track_count,
            release.disc_number,
            release.disc_count
        ),
        (Some(6), Some(13), Some(2), Some(2))
    );

    // The plain recording, not its Dolby Atmos mix on the same album.
    let (meta, length) = track(
        "bad guy",
        &["Billie Eilish"],
        Some((
            "WHEN WE ALL FALL ASLEEP, WHERE DO WE GO?",
            &["Billie Eilish"],
            14,
        )),
        194.0,
    );
    let found = musicbrainz.identify(&meta, length).await.unwrap().unwrap();
    assert_eq!(found.recording_id, "694da04d-1ffc-435c-8b4b-59cc23ac8003");
    assert_eq!(found.release.unwrap().track_number, Some(2));

    // A live album MusicBrainz has only as a bootleg; other live takes
    // are other concerts: nothing fits.
    let (meta, length) = track(
        "Bohemian Rhapsody",
        &["Queen"],
        Some(("Rock In Rio", &["Queen"], 16)),
        317.0,
    );
    assert_eq!(musicbrainz.identify(&meta, length).await.unwrap(), None);

    // A label's upload of a video: no such recording.
    let (meta, length) = track(
        "T95 & Operate - Set You Free",
        &["Shogun Audio"],
        None,
        292.0,
    );
    assert_eq!(musicbrainz.identify(&meta, length).await.unwrap(), None);

    let seen = seen.lock().unwrap().clone();
    assert!(seen.iter().all(|s| s.user_agent == USER_AGENT), "{seen:?}");
    assert!(USER_AGENT.starts_with("orbis/"));
    assert!(
        seen.windows(2)
            .all(|w| w[1].at.duration_since(w[0].at) >= TEST_GAP - Duration::from_millis(2)),
        "{seen:?}"
    );
    assert!(seen.iter().all(|s| s.target.ends_with("fmt=json")));
}

#[tokio::test]
async fn answers_are_kept_and_one_release_serves_an_album() {
    let (address, seen) = service(0).await;
    let cache = tempfile::tempdir().unwrap();
    let (meta, length) = set_you_free();
    let first = client(address, cache.path())
        .identify(&meta, length)
        .await
        .unwrap();
    let asked = seen.lock().unwrap().len();
    assert!(asked >= 3, "search, recording and release group: {asked}");
    // Another client with the same cache asks nothing.
    let again = client(address, cache.path())
        .identify(&meta, length)
        .await
        .unwrap();
    assert_eq!(again, first);
    assert_eq!(seen.lock().unwrap().len(), asked);

    // Another track of the album takes the same release.
    let musicbrainz = client(address, cache.path());
    musicbrainz.identify(&meta, length).await.unwrap();
    let (other, length) = track(
        "Land of the Lost Souls (feat. Krissy Twigge)",
        &["Soul:Motion", "VECTOR"],
        Some(("Point of Origin, Vol. 4", &["Various Artists"], 15)),
        276.0,
    );
    let land = musicbrainz.identify(&other, length).await.unwrap().unwrap();
    assert_eq!(
        land.release.map(|r| r.release_id),
        first.and_then(|f| f.release).map(|r| r.release_id)
    );
}

#[tokio::test]
async fn tracks_take_their_turns_whole() {
    let (address, seen) = service(0).await;
    let cache = tempfile::tempdir().unwrap();
    let musicbrainz = client(address, cache.path());
    let (first, length) = set_you_free();
    let (second, other_length) = track(
        "Land of the Lost Souls (feat. Krissy Twigge)",
        &["Soul:Motion", "VECTOR"],
        Some(("Point of Origin, Vol. 4", &["Various Artists"], 15)),
        276.0,
    );
    let (a, b) = tokio::join!(
        musicbrainz.identify(&first, length),
        musicbrainz.identify(&second, other_length)
    );
    assert!(a.unwrap().is_some() && b.unwrap().is_some());
    let targets: Vec<String> = seen
        .lock()
        .unwrap()
        .iter()
        .map(|s| s.target.clone())
        .collect();
    // The first track's lookup goes out before the second track's search.
    let land = targets.iter().position(|t| t.contains("Land")).unwrap();
    assert!(
        targets[..land].iter().any(|t| t.contains("/recording/")),
        "{targets:?}"
    );
}

/// The bytes of the files in `dir`.
fn size_of(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum()
}

#[tokio::test]
async fn searches_are_kept_with_the_album_releases_only() {
    let (address, seen) = service(0).await;
    let cache = tempfile::tempdir().unwrap();
    let tracks = [
        set_you_free(),
        track(
            "Comfortably Numb",
            &["Pink Floyd"],
            Some(("The Wall", &["Pink Floyd"], 26)),
            382.0,
        ),
        track(
            "bad guy",
            &["Billie Eilish"],
            Some((
                "WHEN WE ALL FALL ASLEEP, WHERE DO WE GO?",
                &["Billie Eilish"],
                14,
            )),
            194.0,
        ),
        track(
            "Bohemian Rhapsody",
            &["Queen"],
            Some(("Rock In Rio", &["Queen"], 16)),
            317.0,
        ),
    ];
    let first: Vec<_> = {
        let musicbrainz = client(address, cache.path());
        let mut found = Vec::new();
        for (meta, length) in &tracks {
            found.push(musicbrainz.identify(meta, *length).await.unwrap());
        }
        found
    };
    let asked = seen.lock().unwrap().len();
    // The answers to these requests as the cache held them before (the
    // recorded ones: trimmed to the fields read, all releases).
    let answers = recorded();
    let recorded: u64 = seen
        .lock()
        .unwrap()
        .iter()
        .filter_map(|s| answers.get(&s.target))
        .map(|body| body.len() as u64)
        .sum();
    let kept = size_of(cache.path());
    println!("kept {kept} bytes for answers recorded as {recorded} bytes");
    assert!(kept * 10 < recorded, "kept {kept} of {recorded}");
    // From the trimmed copies the same tracks are found, without a request.
    let musicbrainz = client(address, cache.path());
    for ((meta, length), before) in tracks.iter().zip(&first) {
        assert_eq!(&musicbrainz.identify(meta, *length).await.unwrap(), before);
    }
    assert_eq!(seen.lock().unwrap().len(), asked);
    assert!(first[1].is_some() && first[3].is_none());
}

#[test]
fn an_album_moves_to_a_better_release_a_later_track_took() {
    let mut albums = HashMap::new();
    let rank =
        |digital: bool, country: &str| (false, false, !digital, country != "XW", "2016".to_owned());
    // The first track only on a Blu-ray; the next on the digital release.
    keep_best(&mut albums, "album".into(), "bluray", rank(false, "US"));
    keep_best(&mut albums, "album".into(), "digital", rank(true, "XW"));
    // A track after them on the Blu-ray again does not move it back.
    keep_best(&mut albums, "album".into(), "bluray", rank(false, "US"));
    assert_eq!(albums["album"].0, "digital");
}

#[test]
fn the_oldest_answers_go_past_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    for (name, hours) in [("old", 3), ("mid", 2), ("new", 1)] {
        let path = dir.path().join(format!("{name}.json"));
        std::fs::write(&path, vec![b'x'; 1000]).unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(now - Duration::from_secs(hours * 3600))
            .unwrap();
    }
    prune(dir.path(), 2000);
    let mut left: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, ["mid.json", "new.json"]);
}

#[tokio::test]
async fn a_busy_service_is_asked_again() {
    let (address, seen) = service(2).await;
    let cache = tempfile::tempdir().unwrap();
    let (meta, length) = set_you_free();
    let found = client(address, cache.path())
        .identify(&meta, length)
        .await
        .unwrap();
    assert!(found.is_some());
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen[0].target, seen[2].target, "the search was repeated");
    // The pause doubles after each busy answer.
    assert!(seen[2].at.duration_since(seen[1].at) >= 2 * TEST_GAP - Duration::from_millis(2));
}

#[tokio::test]
async fn an_unreachable_service_is_an_error() {
    let closed = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap()
    };
    let (meta, length) = set_you_free();
    let result = client(closed, tempfile::tempdir().unwrap().path())
        .identify(&meta, length)
        .await;
    assert!(
        matches!(result, Err(MusicBrainzError::Http(_))),
        "{result:?}"
    );
}

#[test]
fn queries_quote_what_they_ask() {
    let wanted = Wanted {
        title: "Say \"Hi\" (feat. Guest)".into(),
        artists: vec!["A\\B".into()],
        album: None,
        track_count: None,
        length_ms: Some(2000),
        release: None,
    };
    assert_eq!(
        search_query(&wanted, Some("Album")),
        r#"recording:"Say \"Hi\"" AND artist:"A\\B" AND release:"Album" AND dur:[0 TO 5000]"#
    );
    assert_eq!(key("a"), key("a"));
    assert_ne!(key("a"), key("b"));
}
