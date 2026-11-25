//! `orbis-cli`: checks the core (API, stream, playback) without the UI.

use std::error::Error;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

mod audio;
mod browse;
mod download;
mod edit;
mod session;

use session::{Browser, DOMAINS, with_client};

#[derive(Debug, Parser)]
#[command(
    name = "orbis-cli",
    version,
    about = "Checks the orbis core without the UI"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Signs in through the Google sign-in window, or with the cookies of a
    /// browser where YouTube Music is open.
    #[command(group = clap::ArgGroup::new("how").required(true).args(["window", "from"]))]
    Login {
        /// Sign in in the Google sign-in window (`orbis-login`): the player
        /// gets a session of its own.
        #[arg(long)]
        window: bool,
        /// The browser to take the cookies from.
        #[arg(long, value_enum)]
        from: Option<Browser>,
        /// Chromium profile name (such as `Default`) or Firefox profile
        /// directory; the browser's main profile when omitted.
        #[arg(long, requires = "from")]
        profile: Option<String>,
    },
    /// Shows the account of the saved session.
    Whoami,
    /// Removes the saved session.
    Logout,
    /// Lists the shelves of the home feed and the personal mixes on it.
    Home,
    /// Shows a playlist with all its tracks.
    Playlist {
        /// Playlist ID, with or without the `VL` prefix.
        id: String,
        /// Print every track, not only the summary.
        #[arg(long)]
        tracks: bool,
    },
    /// Lists the playlists saved in the library.
    Library,
    /// Shows an album.
    Album { id: String },
    /// Shows an artist page.
    Artist { id: String },
    /// Searches YouTube Music.
    Search {
        query: String,
        /// Only `songs`, `videos`, `albums` or `artists`: 20 a page, with
        /// more pages.
        #[arg(long)]
        filter: Option<String>,
        /// Pages of a filtered search to read.
        #[arg(long, default_value_t = 1)]
        pages: usize,
    },
    /// Builds the radio of a track.
    Radio {
        video_id: String,
        /// Fetch until the queue has at least this many tracks.
        #[arg(long, default_value_t = 50)]
        min: usize,
    },
    /// Prints the lyrics of a track.
    Lyrics { video_id: String },
    /// Plays a playlist from its first track.
    Play {
        playlist_id: String,
        /// How many tracks to play.
        #[arg(long, default_value_t = 3)]
        tracks: usize,
        /// Use mpv's null audio output: decode in real time without sound.
        #[arg(long)]
        silent: bool,
        /// Seek to 4 s before the end after this many seconds of each track,
        /// to check the switch between tracks quickly.
        #[arg(long)]
        skip_after: Option<u64>,
        /// Stop after this many seconds in total.
        #[arg(long)]
        seconds: Option<u64>,
    },
    /// Downloads a track, an album or a whole playlist into the music folder.
    #[command(group = clap::ArgGroup::new("what").required(true).args(["video_id", "album", "playlist"]))]
    Download {
        /// The track's video ID.
        video_id: Option<String>,
        /// An album's browse ID (`MPREb_…`) instead of a track.
        #[arg(long)]
        album: Option<String>,
        /// A playlist ID instead of a track; all its pages. An album's
        /// playlist (`OLAK5uy_…`) is downloaded as the album.
        #[arg(long)]
        playlist: Option<String>,
        /// `opus` (the stream as it is), `mp3` or `m4a` (AAC); the settings'
        /// format by default.
        #[arg(long)]
        format: Option<String>,
        /// The folder to save into instead of the settings' music folder.
        #[arg(long)]
        to: Option<std::path::PathBuf>,
        /// Tags from YouTube Music only, without asking MusicBrainz.
        #[arg(long)]
        no_musicbrainz: bool,
    },
    /// Resolves the audio stream of a track through yt-dlp.
    Stream {
        video_id: String,
        /// Resolve without the account's cookies (no Premium formats).
        #[arg(long)]
        anonymous: bool,
    },
    /// Likes, dislikes or un-rates a track.
    Rate {
        video_id: String,
        #[arg(value_enum)]
        rating: RatingArg,
    },
    /// Shows the account's rating of a track.
    Rating { video_id: String },
    /// Lists the playlists a track can be added to.
    Options { video_id: String },
    /// Adds a track to a playlist.
    Add {
        video_id: String,
        playlist_id: String,
    },
    /// Removes a track from a playlist (every entry of it).
    Remove {
        video_id: String,
        playlist_id: String,
    },
    /// Creates a private playlist and prints its ID.
    CreatePlaylist {
        title: String,
        #[arg(long, default_value = "")]
        description: String,
    },
    /// Deletes a playlist of the account.
    DeletePlaylist { playlist_id: String },
    /// Sends one InnerTube request and prints the JSON response (for development).
    Raw {
        /// The endpoint, such as `browse` or `next`.
        endpoint: String,
        /// The request body without the client context, as JSON.
        #[arg(default_value = "{}")]
        body: String,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum RatingArg {
    Like,
    Dislike,
    None,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    match cli.command {
        Command::Login {
            window: true,
            from: _,
            profile: _,
        } => login_window().await,
        Command::Login {
            window: false,
            from: Some(from),
            profile,
        } => login(from, profile).await,
        Command::Login { .. } => Err("say how to sign in: --window or --from <browser>".into()),
        Command::Whoami => whoami().await,
        Command::Raw { endpoint, body } => raw(&endpoint, &body).await,
        Command::Home => browse::home().await,
        Command::Playlist { id, tracks } => browse::playlist(&id, tracks).await,
        Command::Library => browse::library().await,
        Command::Album { id } => browse::album(&id).await,
        Command::Artist { id } => browse::artist(&id).await,
        Command::Search {
            query,
            filter,
            pages,
        } => browse::search(&query, filter.as_deref(), pages).await,
        Command::Radio { video_id, min } => browse::radio(&video_id, min).await,
        Command::Lyrics { video_id } => browse::lyrics(&video_id).await,
        Command::Play {
            playlist_id,
            tracks,
            silent,
            skip_after,
            seconds,
        } => audio::play(&playlist_id, tracks, silent, skip_after, seconds).await,
        Command::Stream {
            video_id,
            anonymous,
        } => audio::stream(&video_id, anonymous).await,
        Command::Rate { video_id, rating } => {
            let rating = match rating {
                RatingArg::Like => ytm::Rating::Like,
                RatingArg::Dislike => ytm::Rating::Dislike,
                RatingArg::None => ytm::Rating::None,
            };
            edit::rate(&video_id, rating).await
        }
        Command::Rating { video_id } => edit::rating(&video_id).await,
        Command::Options { video_id } => edit::options(&video_id).await,
        Command::Add {
            video_id,
            playlist_id,
        } => edit::add(&video_id, &playlist_id).await,
        Command::Remove {
            video_id,
            playlist_id,
        } => edit::remove(&video_id, &playlist_id).await,
        Command::CreatePlaylist { title, description } => edit::create(&title, &description).await,
        Command::DeletePlaylist { playlist_id } => edit::delete(&playlist_id).await,
        Command::Download {
            video_id,
            album,
            playlist,
            format,
            to,
            no_musicbrainz,
        } => {
            let format = format.map(|f| f.parse()).transpose()?;
            let what = match (video_id, album, playlist) {
                (Some(video_id), _, _) => download::What::Track(video_id),
                (_, Some(album), _) => download::What::Album(album),
                (_, _, Some(playlist)) => download::What::Playlist(playlist),
                _ => return Err("say what to download".into()),
            };
            download::run(what, format, to, !no_musicbrainz).await
        }
        Command::Logout => {
            session::logout().await?;
            println!("Signed out: the saved session is removed.");
            Ok(())
        }
    }
}

async fn login(from: Browser, profile: Option<String>) -> Result<(), Box<dyn Error>> {
    let (account, imported) = session::login(from, profile).await?;
    println!("Signed in as {}.", describe(&account));
    println!(
        "Imported {} cookies of {}; {} could not be decrypted. The session is saved in the wallet.",
        imported.cookies.len(),
        DOMAINS.join(" and "),
        imported.undecryptable
    );
    Ok(())
}

async fn login_window() -> Result<(), Box<dyn Error>> {
    println!("Sign in to Google in the window that opens.");
    let account = session::login_window().await?;
    println!(
        "Signed in as {}. The session is the player's own; it is saved in the wallet.",
        describe(&account)
    );
    Ok(())
}

async fn whoami() -> Result<(), Box<dyn Error>> {
    let account = with_client(|client| async move { client.account().await }).await?;
    println!("{}", describe(&account));
    Ok(())
}

async fn raw(endpoint: &str, body: &str) -> Result<(), Box<dyn Error>> {
    let serde_json::Value::Object(body) = serde_json::from_str(body)? else {
        return Err("the body must be a JSON object".into());
    };
    let response = with_client(|client| {
        let body = body.clone();
        async move { client.post(endpoint, body).await }
    })
    .await?;
    println!("{}", serde_json::to_string_pretty(&response)?);
    Ok(())
}

fn describe(account: &ytm::Account) -> String {
    match &account.handle {
        Some(handle) => format!("{} ({handle})", account.name),
        None => account.name.clone(),
    }
}
