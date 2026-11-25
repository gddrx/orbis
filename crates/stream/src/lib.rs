//! Turns a video ID into a playable audio stream URL.
//!
//! The resolver sits behind [`StreamResolver`] so the backend (yt-dlp today)
//! can be replaced without touching the player. [`StreamCache`] keeps resolved
//! links until shortly before they expire, joins concurrent requests for the
//! same track and resolves the next track ahead of time. Does not depend on Qt.

mod cache;
mod disk;
mod worker;
mod ytdlp;

use std::future::Future;

pub use cache::StreamCache;
pub use worker::IDLE as WORKER_IDLE;
pub use ytdlp::{DEFAULT_FORMAT, YtDlp};

/// A resolved audio stream.
#[derive(Clone, PartialEq)]
pub struct Stream {
    pub video_id: String,
    /// The media URL. It carries signatures and is bound to the session and
    /// the IP address; it is not logged.
    pub url: String,
    /// YouTube's format ID: 774 is Opus ~256 kbit/s (Premium), 251 Opus
    /// ~130–160 kbit/s, 140 AAC ~128 kbit/s.
    pub format_id: String,
    /// Such as `opus` or `mp4a.40.2`.
    pub codec: String,
    /// Such as `webm` or `m4a`.
    pub container: String,
    pub bitrate_kbps: Option<f64>,
    pub sample_rate: Option<u32>,
    /// Length in seconds.
    pub duration: Option<f64>,
    /// When the URL stops working, as Unix seconds.
    pub expires_at: Option<i64>,
    /// Headers the URL must be fetched with.
    pub headers: Vec<(String, String)>,
}

impl std::fmt::Debug for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stream")
            .field("video_id", &self.video_id)
            .field("url", &"<signed URL>")
            .field("format_id", &self.format_id)
            .field("codec", &self.codec)
            .field("bitrate_kbps", &self.bitrate_kbps)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum StreamError {
    /// The backend program is missing or could not be started.
    #[error("cannot run {program}: {message}")]
    Unavailable { program: String, message: String },
    /// The backend ran and failed, such as for a removed or blocked track.
    #[error("no stream for {video_id}: {message}")]
    Failed { video_id: String, message: String },
    #[error("resolving {video_id} took longer than {seconds} s")]
    Timeout { video_id: String, seconds: u64 },
    /// yt-dlp's solver of YouTube's JavaScript challenges ran and failed:
    /// YouTube changed its player (it does every few weeks) and yt-dlp is
    /// likely too old for it, so the track has no audio formats left.
    /// `bundled` when it is the yt-dlp inside orbis's AppImage, which only
    /// a newer AppImage replaces.
    #[error("{}", outdated_text(.version.as_deref(), *.bundled))]
    Outdated {
        video_id: String,
        version: Option<String>,
        bundled: bool,
    },
}

fn outdated_text(version: Option<&str>, bundled: bool) -> String {
    let version = version.map(|v| format!(" {v}")).unwrap_or_default();
    let failed = "its challenge solver failed on YouTube's current player";
    if bundled {
        format!(
            "yt-dlp{version} inside the AppImage is likely out of date: {failed}. \
             A newer AppImage is needed"
        )
    } else {
        format!("yt-dlp{version} is likely out of date: {failed}. Update yt-dlp")
    }
}

/// How soon a link is wanted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Urgency {
    /// A track is to play now: a click, or the next track of the queue.
    Now,
    /// A track might be played: under the pointer, or in view. Such
    /// requests give way to those of [`Urgency::Now`].
    Ahead,
}

/// Something that finds the audio stream of a track.
pub trait StreamResolver: Send + Sync {
    fn resolve(
        &self,
        video_id: &str,
        urgency: Urgency,
    ) -> impl Future<Output = Result<Stream, StreamError>> + Send;
}
