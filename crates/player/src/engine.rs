//! The playback engine: one mpv instance for audio only, with a thread that
//! turns mpv's events into [`PlayerEvent`]s on a channel.
//!
//! The next track is appended to mpv's playlist ahead of time (see
//! [`Engine::enqueue`]), so mpv opens it while the current one plays and the
//! switch has no gap. Each file carries its video ID as a forced media title,
//! which is how events name the track.

use std::sync::Arc;
use std::thread::JoinHandle;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::mpv::{EndReason, Event, Mpv, MpvError, escape_value};

/// A track ready to play.
#[derive(Clone, PartialEq)]
pub struct Media {
    pub video_id: String,
    /// A stream URL or a local path.
    pub url: String,
    /// The User-Agent the URL must be fetched with.
    pub user_agent: Option<String>,
    /// Volume change for this track in dB (loudness normalization).
    pub gain_db: Option<f64>,
    /// Seconds to start at, to continue where a tab stopped.
    pub start: Option<f64>,
}

impl std::fmt::Debug for Media {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Media")
            .field("video_id", &self.video_id)
            .field("gain_db", &self.gain_db)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlayerEvent {
    /// A track started playing: after [`Engine::play`], or by the switch to
    /// an enqueued one.
    Started {
        video_id: String,
        duration: Option<f64>,
    },
    /// A track stopped. `Eof` means it played to the end.
    Ended {
        video_id: Option<String>,
        reason: EndReason,
        error: Option<String>,
    },
    Paused(bool),
    /// Nothing is loaded any more.
    Idle(bool),
    /// Playback (re)started after a load or a seek: the audio flows.
    Restarted,
}

#[derive(Debug, Clone)]
pub struct EngineOptions {
    /// mpv's audio output; `None` for the system default, `Some("null")` for
    /// silent tests.
    pub audio_output: Option<String>,
    /// The stream name the sound server (PipeWire) shows.
    pub client_name: String,
    /// Initial volume, 0–100.
    pub volume: f64,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            audio_output: None,
            client_name: "orbis".into(),
            volume: 100.0,
        }
    }
}

const PAUSE: u64 = 1;
const IDLE: u64 = 2;

#[derive(Debug)]
pub struct Engine {
    mpv: Arc<Mpv>,
    thread: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn new(
        options: &EngineOptions,
    ) -> Result<(Self, UnboundedReceiver<PlayerEvent>), MpvError> {
        let volume = format!("{:.0}", options.volume.clamp(0.0, 100.0));
        let mut settings = vec![
            ("config", "no"),
            ("terminal", "no"),
            ("input-default-bindings", "no"),
            ("osc", "no"),
            ("vid", "no"),
            ("audio-display", "no"),
            ("ytdl", "no"),
            ("idle", "yes"),
            ("keep-open", "no"),
            ("cache", "yes"),
            // Audio needs little read-ahead; mpv's defaults (150 MiB ahead,
            // 50 MiB behind) are sized for video.
            ("demuxer-max-bytes", "8MiB"),
            ("demuxer-max-back-bytes", "2MiB"),
            ("prefetch-playlist", "yes"),
            ("gapless-audio", "weak"),
            ("replaygain", "no"),
            ("network-timeout", "20"),
            ("volume-max", "100"),
            ("audio-client-name", options.client_name.as_str()),
            ("volume", volume.as_str()),
        ];
        if let Some(output) = &options.audio_output {
            settings.push(("ao", output.as_str()));
        }
        let mpv = Arc::new(Mpv::new(&settings)?);
        mpv.observe(PAUSE, "pause", true)?;
        mpv.observe(IDLE, "idle-active", true)?;

        let (sender, receiver) = unbounded_channel();
        let events = Arc::clone(&mpv);
        let thread = std::thread::Builder::new()
            .name("mpv-events".into())
            .spawn(move || event_loop(&events, &sender))
            .map_err(|e| MpvError(e.to_string()))?;
        Ok((
            Self {
                mpv,
                thread: Some(thread),
            },
            receiver,
        ))
    }

    fn load(&self, media: &Media, mode: &str) -> Result<(), MpvError> {
        let mut options = vec![format!(
            "force-media-title={}",
            escape_value(&media.video_id)
        )];
        if let Some(agent) = &media.user_agent {
            options.push(format!("user-agent={}", escape_value(agent)));
        }
        if let Some(gain) = media.gain_db {
            options.push(format!("volume-gain={gain:.2}"));
        }
        if let Some(start) = media.start.filter(|s| *s > 0.0) {
            options.push(format!("start={start:.3}"));
        }
        self.mpv
            .command(&["loadfile", &media.url, mode, "-1", &options.join(",")])
    }

    /// Plays `media` now, dropping whatever was playing or enqueued.
    pub fn play(&self, media: &Media) -> Result<(), MpvError> {
        self.load(media, "replace")
    }

    /// Plays `media` after the current track, without a gap.
    pub fn enqueue(&self, media: &Media) -> Result<(), MpvError> {
        self.load(media, "append")
    }

    /// Forgets enqueued tracks; the current one keeps playing.
    pub fn clear_enqueued(&self) -> Result<(), MpvError> {
        self.mpv.command(&["playlist-clear"])
    }

    pub fn stop(&self) -> Result<(), MpvError> {
        self.mpv.command(&["stop"])
    }

    pub fn set_paused(&self, paused: bool) -> Result<(), MpvError> {
        self.mpv.set_flag("pause", paused)
    }

    pub fn toggle_pause(&self) -> Result<(), MpvError> {
        self.mpv.command(&["cycle", "pause"])
    }

    /// Jumps to `seconds` from the start of the track.
    pub fn seek(&self, seconds: f64) -> Result<(), MpvError> {
        self.mpv
            .command(&["seek", &format!("{seconds:.3}"), "absolute"])
    }

    /// Volume 0–100.
    pub fn set_volume(&self, volume: f64) -> Result<(), MpvError> {
        self.mpv.set_double("volume", volume.clamp(0.0, 100.0))
    }

    pub fn volume(&self) -> Option<f64> {
        self.mpv.get_double("volume")
    }

    /// Changes the current track's loudness correction, dB (what
    /// [`Media::gain_db`] set when it started).
    pub fn set_gain(&self, gain_db: f64) -> Result<(), MpvError> {
        self.mpv.set_double("volume-gain", gain_db)
    }

    /// Seconds played of the current track. Read it when the UI needs it
    /// (once a second while visible) instead of having mpv report every step.
    pub fn position(&self) -> Option<f64> {
        self.mpv.get_double("time-pos")
    }

    pub fn duration(&self) -> Option<f64> {
        self.mpv.get_double("duration")
    }

    pub fn paused(&self) -> bool {
        self.mpv.get_flag("pause").unwrap_or(false)
    }

    /// Entries in mpv's playlist: the current track and the enqueued ones.
    pub fn loaded_count(&self) -> i64 {
        self.mpv.get_int("playlist-count").unwrap_or(0)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // `quit` makes mpv send Shutdown, which ends the event thread; the
        // handle is destroyed when the last Arc goes.
        if self.mpv.command(&["quit"]).is_err() {
            self.mpv.wakeup();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn event_loop(mpv: &Mpv, sender: &UnboundedSender<PlayerEvent>) {
    let mut current: Option<String> = None;
    loop {
        let Some(event) = mpv.wait_event(-1.0) else {
            continue;
        };
        let message = match event {
            Event::Shutdown => break,
            Event::FileLoaded => {
                current = mpv.get_string("media-title");
                current.clone().map(|video_id| PlayerEvent::Started {
                    video_id,
                    duration: mpv.get_double("duration"),
                })
            }
            Event::EndFile { reason, error, .. } => Some(PlayerEvent::Ended {
                video_id: current.take(),
                reason,
                error,
            }),
            Event::PlaybackRestart => Some(PlayerEvent::Restarted),
            Event::Flag { id: PAUSE, value } => Some(PlayerEvent::Paused(value)),
            Event::Flag { id: IDLE, value } => Some(PlayerEvent::Idle(value)),
            _ => None,
        };
        if let Some(message) = message
            && sender.send(message).is_err()
        {
            // Nobody listens any more; keep draining until shutdown.
        }
    }
}

/// The volume change that brings a track to `target_lkfs`, from YouTube's
/// measured loudness. Like the web player it only turns loud tracks down and
/// never raises quiet ones.
pub fn normalization_gain(track_lkfs: Option<f64>, target_lkfs: f64) -> Option<f64> {
    let gain = target_lkfs - track_lkfs?;
    Some(gain.min(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    #[test]
    fn gain_only_turns_down() {
        assert_eq!(normalization_gain(Some(-8.0), -14.0), Some(-6.0));
        assert_eq!(normalization_gain(Some(-15.16), -14.0), Some(0.0));
        assert_eq!(normalization_gain(None, -14.0), None);
    }

    /// A short Opus file made by ffmpeg, or `None` when ffmpeg is missing.
    fn tone(dir: &Path, name: &str, seconds: f32, frequency: u32) -> Option<PathBuf> {
        let path = dir.join(name);
        let status = std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-f", "lavfi", "-i"])
            .arg(format!("sine=frequency={frequency}:duration={seconds}"))
            .args(["-c:a", "libopus", "-b:a", "64k"])
            .arg(&path)
            .status()
            .ok()?;
        status.success().then_some(path)
    }

    async fn next_event(events: &mut UnboundedReceiver<PlayerEvent>) -> PlayerEvent {
        loop {
            let event = tokio::time::timeout(Duration::from_secs(10), events.recv())
                .await
                .expect("an event within 10 s")
                .expect("the channel is open");
            // Pause and idle reports interleave; the test follows the tracks.
            if matches!(
                event,
                PlayerEvent::Started { .. } | PlayerEvent::Ended { .. }
            ) {
                return event;
            }
        }
    }

    #[tokio::test]
    async fn plays_two_tracks_in_a_row_and_reports_them() {
        let dir = tempfile::tempdir().unwrap();
        let (Some(a), Some(b)) = (
            tone(dir.path(), "a.opus", 0.6, 440),
            tone(dir.path(), "b.opus", 0.6, 660),
        ) else {
            eprintln!("ffmpeg is missing; skipping");
            return;
        };
        let options = EngineOptions {
            audio_output: Some("null".into()),
            ..Default::default()
        };
        let (engine, mut events) = Engine::new(&options).unwrap();
        let media = |id: &str, path: &Path| Media {
            video_id: id.into(),
            url: path.display().to_string(),
            user_agent: Some("test agent (KHTML, like Gecko)".into()),
            gain_db: Some(-3.0),
            start: None,
        };
        engine.play(&media("first", &a)).unwrap();
        engine.enqueue(&media("second", &b)).unwrap();

        let PlayerEvent::Started { video_id, duration } = next_event(&mut events).await else {
            panic!("expected a start");
        };
        assert_eq!(video_id, "first");
        assert!((duration.unwrap() - 0.6).abs() < 0.1);
        assert_eq!(
            next_event(&mut events).await,
            PlayerEvent::Ended {
                video_id: Some("first".into()),
                reason: EndReason::Eof,
                error: None
            }
        );
        assert!(
            matches!(next_event(&mut events).await, PlayerEvent::Started { video_id, .. } if video_id == "second")
        );
        assert!(matches!(
            next_event(&mut events).await,
            PlayerEvent::Ended {
                reason: EndReason::Eof,
                ..
            }
        ));
        drop(engine);
    }

    #[tokio::test]
    async fn a_missing_file_ends_with_an_error() {
        let options = EngineOptions {
            audio_output: Some("null".into()),
            ..Default::default()
        };
        let (engine, mut events) = Engine::new(&options).unwrap();
        engine
            .play(&Media {
                video_id: "gone".into(),
                url: "/nonexistent/track.opus".into(),
                user_agent: None,
                gain_db: None,
                start: None,
            })
            .unwrap();
        match next_event(&mut events).await {
            PlayerEvent::Ended {
                reason: EndReason::Error,
                error,
                ..
            } => assert!(error.is_some()),
            other => panic!("unexpected {other:?}"),
        }
        engine.set_volume(55.0).unwrap();
        assert_eq!(engine.volume(), Some(55.0));
    }
}
