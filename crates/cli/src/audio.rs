//! `orbis-cli` commands for streams and playback.

use std::error::Error;
use std::sync::Arc;
use std::time::Instant;

use orbis_core::{Playback, PlaybackEvent, SessionManager, item_from_track};
use player::{QueueItem, Store, TabSource};
use stream::StreamCache;

use crate::with_client;

pub async fn stream(video_id: &str, anonymous: bool) -> Result<(), Box<dyn Error>> {
    let mut resolver = orbis_core::yt_dlp();
    if !anonymous {
        // Checking the account first renews a stale session, so yt-dlp does
        // not quietly fall back to the formats of a signed-out visitor.
        let cookies = with_client(|client| async move {
            client.account().await?;
            Ok(client.cookies_netscape())
        })
        .await?;
        let cookies = Arc::new(cookies);
        resolver = resolver.with_cookies(Arc::new(move || Some(cookies.as_ref().clone())));
    }
    let cache = StreamCache::new(resolver);

    let started = Instant::now();
    let stream = cache.get(video_id).await?;
    let first = started.elapsed();
    let started = Instant::now();
    cache.get(video_id).await?;
    let second = started.elapsed();

    let expires_in = stream
        .expires_at
        .map(|at| format!("{:.1} h", (at - ytm::unix_now()) as f64 / 3600.0))
        .unwrap_or_else(|| "?".into());
    println!(
        "{video_id}: format {} {} in {}, {} kbit/s, {} Hz, {:.0} s long; link valid for {expires_in}",
        stream.format_id,
        stream.codec,
        stream.container,
        stream
            .bitrate_kbps
            .map(|b| format!("{b:.0}"))
            .unwrap_or_else(|| "?".into()),
        stream
            .sample_rate
            .map(|r| r.to_string())
            .unwrap_or_else(|| "?".into()),
        stream.duration.unwrap_or_default(),
    );
    println!(
        "resolved in {:.2} s; from the cache in {:.3} ms",
        first.as_secs_f64(),
        second.as_secs_f64() * 1000.0
    );
    Ok(())
}

/// CPU seconds of this process and of its waited-for children (yt-dlp).
fn cpu_seconds() -> (f64, f64) {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let Some((_, rest)) = stat.rsplit_once(')') else {
        return (0.0, 0.0);
    };
    let fields: Vec<f64> = rest
        .split_whitespace()
        .map(|f| f.parse().unwrap_or(0.0))
        .collect();
    let field = |i: usize| fields.get(i).copied().unwrap_or(0.0) / 100.0;
    (field(11) + field(12), field(13) + field(14))
}

fn memory_mb() -> (f64, f64) {
    let read = |path| std::fs::read_to_string(path).unwrap_or_default();
    let kb = |text: &str, key: &str| -> f64 {
        text.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0)
    };
    (
        kb(&read("/proc/self/status"), "VmRSS:") / 1024.0,
        kb(&read("/proc/self/smaps_rollup"), "Pss:") / 1024.0,
    )
}

pub async fn play(
    playlist_id: &str,
    count: usize,
    silent: bool,
    skip_after: Option<u64>,
    seconds: Option<u64>,
) -> Result<(), Box<dyn Error>> {
    let session = Arc::new(SessionManager::open().await?);
    let playlist = session
        .call(|c| {
            let id = playlist_id.to_owned();
            async move { c.playlist(&id).await }
        })
        .await?;
    let items: Vec<QueueItem> = playlist
        .tracks
        .iter()
        .filter(|t| t.available)
        .take(count)
        .map(item_from_track)
        .collect();
    if items.is_empty() {
        return Err("the playlist has no playable tracks".into());
    }

    // The service saves tabs; a throwaway database keeps the app's own clean.
    let state = tempfile::tempdir()?;
    let (playback, mut events) = Playback::start(
        Arc::clone(&session),
        &state.path().join("state.db"),
        (None, None),
        silent.then(|| "null".to_owned()),
    )?;
    let tab = playback.edit_tabs(|tabs| {
        tabs.open(
            &playlist.title,
            TabSource::Playlist {
                id: playlist.id.clone(),
            },
            false,
        )
    });

    let started = Instant::now();
    playback.play_list(tab, items, 0, None).await?;
    println!(
        "first track ready in {:.2} s",
        started.elapsed().as_secs_f64()
    );

    let (cpu_start, _) = cpu_seconds();
    let clock = Instant::now();
    let limit = seconds.map(std::time::Duration::from_secs);
    let mut played = 0;
    let mut last_start: Option<Instant> = None;
    loop {
        let wait = limit
            .map(|l| l.saturating_sub(clock.elapsed()))
            .unwrap_or(std::time::Duration::from_secs(3600));
        let event = match tokio::time::timeout(wait, events.recv()).await {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(_) => {
                println!("time limit reached");
                break;
            }
        };
        match event {
            PlaybackEvent::NowPlaying { item, duration, .. } => {
                played += 1;
                let since = last_start
                    .replace(Instant::now())
                    .map(|t| {
                        format!(
                            ", {:.1} s after the previous start",
                            t.elapsed().as_secs_f64()
                        )
                    })
                    .unwrap_or_default();
                println!(
                    "▶ {} — {} ({:.0} s{since})",
                    item.title,
                    item.artists,
                    duration.unwrap_or_default()
                );
                if let (Some(after), Some(duration)) = (skip_after, duration) {
                    let playback = Arc::clone(&playback);
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(after)).await;
                        let _ = playback.seek((duration - 4.0).max(0.0));
                    });
                }
            }
            PlaybackEvent::Stopped => {
                println!("■ the queue is over");
                break;
            }
            PlaybackEvent::Error(e) => println!("! {e}"),
            PlaybackEvent::Paused(_)
            | PlaybackEvent::QueueChanged { .. }
            | PlaybackEvent::Rated { .. } => {}
        }
    }

    let wall = clock.elapsed().as_secs_f64();
    let (cpu_end, _) = cpu_seconds();
    let (rss, pss) = memory_mb();
    let saved = Store::open(&state.path().join("state.db"))?
        .load()?
        .map(|t| t.tabs().len())
        .unwrap_or(0);
    println!(
        "played {played} tracks in {wall:.0} s; CPU {:.2} % of a core; RSS {rss:.0} MB, PSS {pss:.0} MB; {saved} tabs saved",
        (cpu_end - cpu_start) / wall * 100.0
    );
    session.flush().await?;
    Ok(())
}
