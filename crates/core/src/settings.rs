//! The user's settings, in `~/.config/orbis/config.toml`: the language of
//! the window, playback (quality, loudness, disliked tracks), downloads
//! (format, folder, name template, bit rates, MusicBrainz), the glass and the
//! tray. A missing file or key means the default; values out of range are
//! brought back into it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use download::{Bitrates, DEFAULT_TEMPLATE, Format, MusicBrainz, Template};
use serde::{Deserialize, Serialize};

/// The quality of the stream that plays (downloads always take the best).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    /// Opus ~256 kbit/s with Premium, else the best there is.
    #[default]
    Best,
    /// Opus ~130–160 kbit/s.
    Normal,
    /// Opus ~50–70 kbit/s, for mobile internet.
    Economy,
}

impl Quality {
    pub const ALL: [Quality; 3] = [Quality::Best, Quality::Normal, Quality::Economy];

    /// yt-dlp's format selection: YouTube's format IDs in order of
    /// preference (774 Opus Premium, 251 Opus ~160, 250 Opus ~70, 249 Opus
    /// ~50, 140 AAC 128, 139 AAC 48), then anything.
    pub fn ytdlp_format(self) -> &'static str {
        match self {
            Quality::Best => stream::DEFAULT_FORMAT,
            Quality::Normal => "251/140/bestaudio[abr<=170]/bestaudio",
            Quality::Economy => "250/249/139/worstaudio[acodec=opus]/worstaudio",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Quality::Best => "best",
            Quality::Normal => "normal",
            Quality::Economy => "economy",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|q| q.name() == name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaybackSettings {
    pub quality: Quality,
    /// Loud tracks are turned down to YouTube's level (-14 LUFS).
    pub normalize: bool,
    /// "Next" and the end of a track go past the tracks the account
    /// disliked, as YouTube Music's own player does.
    pub skip_disliked: bool,
    /// Playlists, mixes and radios load without the tracks the account
    /// disliked.
    pub hide_disliked: bool,
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        Self {
            quality: Quality::Best,
            normalize: true,
            skip_disliked: true,
            hide_disliked: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadSettings {
    pub format: Format,
    /// The music folder; `None` is the desktop's (`XDG_MUSIC_DIR`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<PathBuf>,
    /// The name template (see [`download::Template`]).
    pub template: String,
    pub mp3_bitrate: u32,
    pub m4a_bitrate: u32,
    /// MusicBrainz is asked for each saved track.
    pub musicbrainz: bool,
}

impl Default for DownloadSettings {
    fn default() -> Self {
        let bitrates = Bitrates::default();
        Self {
            format: Format::Opus,
            folder: None,
            template: DEFAULT_TEMPLATE.to_owned(),
            mp3_bitrate: bitrates.mp3,
            m4a_bitrate: bitrates.m4a,
            musicbrainz: true,
        }
    }
}

impl DownloadSettings {
    /// The template to name by: the default when the saved one is broken.
    pub fn template(&self) -> Template {
        Template::parse(&self.template).unwrap_or_default()
    }

    pub fn bitrates(&self) -> Bitrates {
        Bitrates::new(self.mp3_bitrate, self.m4a_bitrate)
    }

    /// The music folder downloads go to.
    pub fn folder(&self) -> PathBuf {
        self.folder.clone().unwrap_or_else(desktop::music_dir)
    }

    /// The downloader's options; `musicbrainz` is used when it is on.
    pub fn options(&self, musicbrainz: &Arc<MusicBrainz>) -> crate::downloads::DownloadOptions {
        crate::downloads::DownloadOptions {
            root: self.folder(),
            format: self.format,
            template: self.template(),
            bitrates: self.bitrates(),
            musicbrainz: self.musicbrainz.then(|| Arc::clone(musicbrainz)),
        }
    }
}

/// The ground's opacity under the glass, in percent: below 40 text reads
/// badly over light windows behind orbis.
pub const OPACITY_RANGE: std::ops::RangeInclusive<u8> = 40..=90;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowSettings {
    /// The compositor blurs behind the window, and the window lets it
    /// through.
    pub glass: bool,
    /// How much of the dark ground covers the blur, percent.
    pub opacity: u8,
    /// An icon in the tray; closing the window hides it there.
    pub tray: bool,
    /// Hidden in the tray, the window is unloaded (only the core plays).
    pub unload_in_tray: bool,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            glass: true,
            opacity: 60,
            tray: true,
            unload_in_tray: true,
        }
    }
}

/// The languages of the window, by their codes; the first is the default.
pub const LANGUAGES: [&str; 2] = ["en", "uk"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The window's language: a code of [`LANGUAGES`]. First, as TOML wants
    /// plain values before the tables.
    pub language: String,
    pub playback: PlaybackSettings,
    pub downloads: DownloadSettings,
    pub window: WindowSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: LANGUAGES[0].to_owned(),
            playback: PlaybackSettings::default(),
            downloads: DownloadSettings::default(),
            window: WindowSettings::default(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("cannot read {path}: {message}")]
    Read { path: PathBuf, message: String },
    #[error("cannot write {path}: {message}")]
    Write { path: PathBuf, message: String },
}

impl Settings {
    /// `$ORBIS_CONFIG_DIR/config.toml`, else `$XDG_CONFIG_HOME/orbis/…`,
    /// else `~/.config/orbis/config.toml`.
    pub fn path() -> PathBuf {
        let dir = std::env::var_os("ORBIS_CONFIG_DIR")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute())
                    .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("orbis")
            });
        dir.join("config.toml")
    }

    /// The settings in `path`: the defaults when there is no file. A file
    /// that cannot be read is an error, so that it is not overwritten
    /// unseen; the caller may go on with the defaults.
    pub fn load(path: &Path) -> Result<Self, SettingsError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => {
                return Err(SettingsError::Read {
                    path: path.to_owned(),
                    message: e.to_string(),
                });
            }
        };
        let settings: Settings = toml::from_str(&text).map_err(|e| SettingsError::Read {
            path: path.to_owned(),
            message: e.message().to_owned(),
        })?;
        Ok(settings.checked())
    }

    /// The settings with every value in its range.
    pub fn checked(mut self) -> Self {
        if !LANGUAGES.contains(&self.language.as_str()) {
            self.language = LANGUAGES[0].to_owned();
        }
        let window = &mut self.window;
        window.opacity = window
            .opacity
            .clamp(*OPACITY_RANGE.start(), *OPACITY_RANGE.end());
        let bitrates = self.downloads.bitrates();
        self.downloads.mp3_bitrate = bitrates.mp3;
        self.downloads.m4a_bitrate = bitrates.m4a;
        if Template::parse(&self.downloads.template).is_err() {
            self.downloads.template = DEFAULT_TEMPLATE.to_owned();
        }
        self
    }

    /// Writes the settings to `path` whole: a temporary file next to it is
    /// renamed over it.
    pub fn save(&self, path: &Path) -> Result<(), SettingsError> {
        let error = |e: &dyn std::fmt::Display| SettingsError::Write {
            path: path.to_owned(),
            message: e.to_string(),
        };
        let text = toml::to_string_pretty(self).map_err(|e| error(&e))?;
        let text =
            format!("# orbis settings: changed in the window (⚙), read at the start.\n\n{text}");
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| error(&e))?;
        }
        let partial = path.with_extension("toml.part");
        std::fs::write(&partial, text).map_err(|e| error(&e))?;
        std::fs::rename(&partial, path).map_err(|e| error(&e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_survive_a_write_and_a_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("orbis/config.toml");
        assert_eq!(Settings::load(&path).unwrap(), Settings::default());

        let mut settings = Settings {
            language: "uk".into(),
            ..Settings::default()
        };
        settings.playback.quality = Quality::Economy;
        settings.playback.normalize = false;
        settings.playback.skip_disliked = false;
        settings.playback.hide_disliked = true;
        settings.downloads.format = Format::Mp3;
        settings.downloads.folder = Some(PathBuf::from("/data/music"));
        settings.downloads.template = "{artist}/{title}".into();
        settings.downloads.mp3_bitrate = 192;
        settings.downloads.m4a_bitrate = 128;
        settings.downloads.musicbrainz = false;
        settings.window = WindowSettings {
            glass: false,
            opacity: 85,
            tray: false,
            unload_in_tray: false,
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), settings);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("quality = \"economy\""), "{text}");
        assert!(text.contains("format = \"mp3\""), "{text}");
        assert!(text.contains("language = \"uk\""), "{text}");
        assert!(!path.with_extension("toml.part").exists());
    }

    #[test]
    fn missing_and_wrong_values_fall_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "language = \"xx\"\n[window]\nopacity = 5\n\n[downloads]\nmp3_bitrate = 300\ntemplate = \"{nope}\"\n",
        )
        .unwrap();
        let settings = Settings::load(&path).unwrap();
        assert_eq!(settings.window.opacity, 40);
        assert_eq!(settings.language, "en", "an unknown language is English");
        assert!(settings.window.glass, "a missing key is the default");
        assert_eq!(settings.downloads.mp3_bitrate, 320);
        assert_eq!(settings.downloads.template, DEFAULT_TEMPLATE);
        assert_eq!(settings.playback, PlaybackSettings::default());

        std::fs::write(&path, "[window\nglass = maybe").unwrap();
        assert!(matches!(
            Settings::load(&path),
            Err(SettingsError::Read { .. })
        ));
    }

    #[test]
    fn qualities_choose_streams() {
        assert_eq!(Quality::Best.ytdlp_format(), stream::DEFAULT_FORMAT);
        assert!(Quality::Economy.ytdlp_format().starts_with("250/249"));
        for quality in Quality::ALL {
            assert_eq!(Quality::from_name(quality.name()), Some(quality));
        }
        assert_eq!(Quality::from_name("lossless"), None);
    }
}
