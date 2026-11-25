//! `orbis-cli download`: a track, an album or a playlist into the music
//! folder, through the same queue as the window.

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use orbis_core::{
    DownloadEvent, Downloads, Format, MusicBrainz, Outcome, SessionManager, Settings, Wanted,
};

/// What to download.
pub enum What {
    Track(String),
    Album(String),
    Playlist(String),
}

/// Downloads `what` as the window would, by the settings; `format`, `to`
/// and `musicbrainz: false` change them for this run only.
pub async fn run(
    what: What,
    format: Option<Format>,
    to: Option<PathBuf>,
    musicbrainz: bool,
) -> Result<(), Box<dyn Error>> {
    let session = Arc::new(SessionManager::open().await?);
    let settings = Settings::load(&Settings::path())?.downloads;
    let client = Arc::new(MusicBrainz::new(Some(
        desktop::cache_dir().join("musicbrainz"),
    )));
    let mut options = settings.options(&client);
    if let Some(format) = format {
        options.format = format;
    }
    if let Some(to) = to {
        options.root = to;
    }
    if !musicbrainz {
        options.musicbrainz = None;
    }
    let (root, format) = (options.root.clone(), options.format);
    let (downloads, mut events) = Downloads::new(Arc::clone(&session), options);
    let ids = match &what {
        What::Track(video_id) => vec![downloads.track(Wanted::video(video_id))],
        What::Album(id) => downloads.album(id).await?,
        What::Playlist(id) => downloads.playlist(id).await?,
    };
    println!(
        "Downloading {} track(s) as {} into {}",
        ids.len(),
        format.extension(),
        root.display()
    );
    let started = Instant::now();
    let shown = |path: &std::path::Path| {
        path.strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string()
    };
    let (mut saved, mut there, mut failed) = (0, 0, 0);
    let mut left = ids.len();
    // MusicBrainz is asked after each file is saved, and an album's gain
    // goes in once all its files are: the run waits for these too.
    let (mut asking, mut asked, mut known) = (0, 0, 0);
    let mut files_done = None;
    while left > 0 || asking > 0 {
        if left == 0 && files_done.is_none() {
            files_done = Some(started.elapsed());
        }
        let Some(event) = events.recv().await else {
            break;
        };
        match event {
            DownloadEvent::Started { id, title } => println!("[{id}] {title}"),
            DownloadEvent::Finished {
                id,
                outcome,
                tagging,
            } => {
                left -= 1;
                asking += usize::from(tagging);
                match outcome {
                    Outcome::Saved(path) => {
                        saved += 1;
                        println!("[{id}] saved: {}", shown(&path));
                    }
                    Outcome::AlreadyThere(path) => {
                        there += 1;
                        println!("[{id}] already there: {}", shown(&path));
                    }
                }
            }
            DownloadEvent::Failed { id, message } => {
                left -= 1;
                failed += 1;
                eprintln!("[{id}] failed: {message}");
            }
            DownloadEvent::Tagged {
                id,
                musicbrainz,
                album,
            } => {
                asking -= 1;
                if let Some(found) = musicbrainz {
                    asked += 1;
                    known += usize::from(found);
                    let what = if found { "tags added" } else { "not found" };
                    println!("[{id}] MusicBrainz: {what}");
                }
                if let Some(album) = album {
                    println!(
                        "[{id}] the album is all here: its loudness {:.1} LUFS goes into each file",
                        album.integrated
                    );
                }
            }
            DownloadEvent::Cancelled { .. } => left -= 1,
            DownloadEvent::Queued { .. } | DownloadEvent::Progress { .. } => {}
        }
    }
    session.flush().await?;
    let files_done = files_done.unwrap_or_else(|| started.elapsed());
    println!(
        "{saved} saved, {there} already there, {failed} failed in {:.1} s",
        files_done.as_secs_f64()
    );
    if asked > 0 {
        println!(
            "MusicBrainz knew {known} of {asked}; its tags were all in after {:.1} s",
            started.elapsed().as_secs_f64()
        );
    }
    if failed > 0 {
        return Err(format!("{failed} download(s) failed").into());
    }
    Ok(())
}
