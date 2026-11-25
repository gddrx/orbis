//! `orbis-cli` commands that read YouTube Music and print what the parsers
//! made of it, with counts of missing fields to spot parsing gaps.

use std::error::Error;

use ytm::{Item, Track, TrackKind};

use crate::with_client;

fn artists(track: &Track) -> String {
    track
        .artists
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn duration(seconds: Option<u32>) -> String {
    match seconds {
        Some(s) if s >= 3600 => format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60),
        Some(s) => format!("{}:{:02}", s / 60, s % 60),
        None => "-".into(),
    }
}

fn describe(item: &Item) -> String {
    match item {
        Item::Track(t) => format!("track {} — {} [{}]", t.title, artists(t), t.video_id),
        Item::Playlist(p) => format!("playlist {} [{}]", p.title, p.id),
        Item::Album(a) => format!(
            "album {} ({}) [{}]",
            a.title,
            a.year.map(|y| y.to_string()).unwrap_or_default(),
            a.id
        ),
        Item::Artist(a) => format!("artist {} [{}]", a.name, a.id),
        Item::Other { title, id } => format!("other {title} [{}]", id.as_deref().unwrap_or("-")),
    }
}

/// Counts of tracks with missing or notable fields.
fn track_summary(tracks: &[Track]) -> String {
    let count = |f: &dyn Fn(&Track) -> bool| tracks.iter().filter(|t| f(t)).count();
    format!(
        "{} tracks: {} songs, {} videos, {} episodes; without artists {}, without album {}, without duration {}, without thumbnail {}, unavailable {}, liked {}",
        tracks.len(),
        count(&|t| t.kind == TrackKind::Song),
        count(&|t| t.kind == TrackKind::Video),
        count(&|t| t.kind == TrackKind::Episode),
        count(&|t| t.artists.is_empty()),
        count(&|t| t.album.is_none()),
        count(&|t| t.duration.is_none()),
        count(&|t| t.thumbnail.is_none()),
        count(&|t| !t.available),
        count(&|t| t.like == Some(ytm::LikeStatus::Like)),
    )
}

pub async fn home() -> Result<(), Box<dyn Error>> {
    let pages = with_client(|client| async move { client.home_all().await }).await?;
    let shelves: Vec<_> = pages.iter().flat_map(|p| &p.shelves).collect();
    println!("{} pages, {} shelves", pages.len(), shelves.len());
    for shelf in &shelves {
        let first = shelf.items.first().map(describe).unwrap_or_default();
        println!("- {} ({} items): {first}", shelf.title, shelf.items.len());
    }
    // In English, whatever the interface language: their titles name them.
    let mixes = with_client(|client| async move { client.personal_mixes().await }).await?;
    println!("Mixes:");
    for (mix, card) in mixes {
        println!("  {:?}: {} [{}]", mix, card.title, card.playlist_id());
    }
    Ok(())
}

pub async fn playlist(id: &str, print_tracks: bool) -> Result<(), Box<dyn Error>> {
    let playlist = with_client(|client| async move { client.playlist_all(id).await }).await?;
    println!(
        "{} — {} — {} [{}]",
        playlist.title, playlist.subtitle, playlist.details, playlist.id
    );
    println!("{}", track_summary(&playlist.tracks));
    if print_tracks {
        for (i, t) in playlist.tracks.iter().enumerate() {
            println!(
                "{:>4}. {} — {} ({}) [{}]",
                i + 1,
                t.title,
                artists(t),
                duration(t.duration),
                t.video_id
            );
        }
    }
    Ok(())
}

pub async fn library() -> Result<(), Box<dyn Error>> {
    let playlists = with_client(|client| async move { client.library_playlists().await }).await?;
    println!("{} playlists in the library", playlists.len());
    for p in &playlists {
        println!("- {} — {} [{}]", p.title, p.subtitle, p.playlist_id());
    }
    Ok(())
}

pub async fn album(id: &str) -> Result<(), Box<dyn Error>> {
    let album = with_client(|client| async move { client.album(id).await }).await?;
    let names: Vec<_> = album.artists.iter().map(|a| a.name.as_str()).collect();
    println!(
        "{} — {} — {} [{}], playlist {:?}",
        album.title,
        names.join(", "),
        album.subtitle,
        album.id,
        album.playlist_id
    );
    println!("{}", track_summary(&album.tracks));
    for (i, t) in album.tracks.iter().enumerate() {
        println!(
            "{:>3}. {} — {} ({})",
            i + 1,
            t.title,
            artists(t),
            duration(t.duration)
        );
    }
    Ok(())
}

pub async fn artist(id: &str) -> Result<(), Box<dyn Error>> {
    let artist = with_client(|client| async move { client.artist(id).await }).await?;
    println!(
        "{} [{}], shuffle {:?}, radio {:?}",
        artist.name, artist.id, artist.shuffle_playlist, artist.radio_playlist
    );
    for shelf in &artist.shelves {
        let first = shelf.items.first().map(describe).unwrap_or_default();
        println!("- {} ({} items): {first}", shelf.title, shelf.items.len());
    }
    Ok(())
}

pub async fn search(query: &str, filter: Option<&str>, pages: usize) -> Result<(), Box<dyn Error>> {
    let filter = match filter {
        Some(name) => Some(
            ytm::SearchFilter::from_name(name)
                .ok_or_else(|| format!("no filter {name}: songs, videos, albums or artists"))?,
        ),
        None => None,
    };
    let (results, read) = with_client(|client| async move {
        let mut results = client.search_in(query, filter).await?;
        let mut read = 1;
        while read < pages {
            let Some(token) = results.continuation.take() else {
                break;
            };
            let page = client.search_next(&token).await?;
            results.items.extend(page.items);
            results.continuation = page.continuation;
            read += 1;
        }
        Ok((results, read))
    })
    .await?;
    if let Some(top) = &results.top {
        println!("top: {}", describe(top));
    }
    for item in &results.items {
        println!("- {}", describe(item));
    }
    println!(
        "{} items in {read} page(s){}",
        results.items.len(),
        if results.continuation.is_some() {
            ", more after"
        } else {
            ""
        }
    );
    Ok(())
}

pub async fn radio(video_id: &str, min: usize) -> Result<(), Box<dyn Error>> {
    let (queue, requests) = with_client(|client| async move {
        let mut queue = client.radio(video_id).await?;
        let mut requests = 1;
        // Continuations repeat most of the queue; give up after a few
        // requests that bring nothing new.
        let mut idle = 0;
        while queue.tracks.len() < min && idle < 3 {
            let Some(token) = queue.continuation.clone() else {
                break;
            };
            let added = queue.append(client.watch_next(&token).await?);
            idle = if added == 0 { idle + 1 } else { 0 };
            requests += 1;
        }
        Ok((queue, requests))
    })
    .await?;
    println!(
        "radio of {video_id}: {} requests, more available: {}, lyrics: {}",
        requests,
        queue.continuation.is_some(),
        queue.lyrics_id.is_some()
    );
    println!("{}", track_summary(&queue.tracks));
    for (i, t) in queue.tracks.iter().take(5).enumerate() {
        println!(
            "{:>3}. {} — {} ({})",
            i + 1,
            t.title,
            artists(t),
            duration(t.duration)
        );
    }
    Ok(())
}

pub async fn lyrics(video_id: &str) -> Result<(), Box<dyn Error>> {
    let lyrics = with_client(|client| async move { client.lyrics(video_id).await }).await?;
    match lyrics {
        Some(lyrics) => {
            println!("{}", lyrics.text);
            if let Some(source) = lyrics.source {
                eprintln!("{source}");
            }
        }
        None => eprintln!("no lyrics for {video_id}"),
    }
    Ok(())
}
