//! The orbis core shared by the app and `orbis-cli`: the signed-in session
//! that renews itself, and the playback service over tabs, the stream cache
//! and mpv, the cache of covers and the backdrops of the glass look made from
//! them, and the download queue. Does not depend on Qt.

mod ahead;
pub mod backdrop;
pub mod covers;
pub mod downloads;
pub mod playback;
pub mod session;
pub mod settings;

pub use backdrop::{Backdrop, BackdropError};
pub use covers::{CoverCache, CoverError, CoverStats};
pub use download;
pub use download::{Bitrates, Format, MusicBrainz, Outcome, Template};
pub use downloads::{DownloadEvent, DownloadOptions, Downloads, Wanted};
pub use playback::{
    Playback, PlaybackError, PlaybackEvent, PlaybackFailure, item_from_track, rated_from,
};
pub use session::{Browser, CoreError, SessionManager, SignedIn, Source, Window, login_program};
pub use settings::{Quality, Settings};

/// yt-dlp as orbis runs it: its cache in orbis's cache folder
/// (`<cache>/yt-dlp`), which brings orbis's plugin and preprocessed players.
pub fn yt_dlp() -> stream::YtDlp {
    stream::YtDlp::default().with_cache_dir(desktop::cache_dir().join("yt-dlp"))
}
