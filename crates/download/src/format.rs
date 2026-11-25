//! The file formats and how ffmpeg makes each from the stream.

use std::sync::OnceLock;

/// The format a track is saved in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// The stream's Opus as it is, in an Ogg `.opus` file.
    #[default]
    Opus,
    /// MP3, re-encoded (320 kbit/s unless [`Bitrates`] say otherwise).
    Mp3,
    /// AAC in an `.m4a` file, re-encoded (256 kbit/s by default).
    M4a,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Opus, Format::Mp3, Format::M4a];

    /// The file extension of the format.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Opus => "opus",
            Self::Mp3 => "mp3",
            Self::M4a => "m4a",
        }
    }
}

impl std::str::FromStr for Format {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.to_ascii_lowercase().as_str() {
            "opus" => Ok(Self::Opus),
            "mp3" => Ok(Self::Mp3),
            "m4a" | "aac" => Ok(Self::M4a),
            other => Err(format!("unknown format {other:?}; opus, mp3 or m4a")),
        }
    }
}

/// The bit rates of the formats that are encoded anew, kbit/s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bitrates {
    pub mp3: u32,
    pub m4a: u32,
}

impl Bitrates {
    /// What MP3 can be encoded at.
    pub const MP3: [u32; 3] = [192, 256, 320];
    /// What AAC in M4A can be encoded at.
    pub const M4A: [u32; 3] = [128, 192, 256];

    /// The given rates, each moved to the nearest it can be.
    pub fn new(mp3: u32, m4a: u32) -> Self {
        let nearest = |rate: u32, allowed: &[u32]| {
            *allowed
                .iter()
                .min_by_key(|a| a.abs_diff(rate))
                .expect("rates are listed")
        };
        Self {
            mp3: nearest(mp3, &Self::MP3),
            m4a: nearest(m4a, &Self::M4A),
        }
    }
}

impl Default for Bitrates {
    /// MP3 at 320 and AAC at 256 kbit/s (the owner's choice, 22 November
    /// 2026).
    fn default() -> Self {
        Self { mp3: 320, m4a: 256 }
    }
}

/// How ffmpeg writes the file: its codec arguments, the muxer and the
/// extension that comes out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Recipe {
    pub codec: Vec<String>,
    pub muxer: &'static str,
    pub extension: &'static str,
    /// The audio is encoded anew, which takes a while.
    pub transcodes: bool,
}

/// `true` for a stream codec that is Opus (`opus`), `false` for anything
/// else, such as AAC (`mp4a.40.2`).
fn is_opus(codec: &str) -> bool {
    codec.eq_ignore_ascii_case("opus")
}

fn is_aac(codec: &str) -> bool {
    codec.to_ascii_lowercase().starts_with("mp4a")
}

fn copy(muxer: &'static str, extension: &'static str) -> Recipe {
    Recipe {
        codec: vec!["-c:a".into(), "copy".into()],
        muxer,
        extension,
        transcodes: false,
    }
}

/// The recipe for `format` from a stream in `source_codec`. Nothing is
/// encoded twice: Opus asked from an AAC stream (when YouTube gives no Opus)
/// is saved as the AAC it is, in `.m4a`; M4A from an AAC stream is copied.
#[cfg(test)]
pub(crate) fn recipe(format: Format, source_codec: &str, aac_encoder: &'static str) -> Recipe {
    recipe_at(format, source_codec, aac_encoder, Bitrates::default())
}

/// [`recipe`] with the bit rates of MP3 and AAC.
pub(crate) fn recipe_at(
    format: Format,
    source_codec: &str,
    aac_encoder: &'static str,
    bitrates: Bitrates,
) -> Recipe {
    match format {
        Format::Opus if is_opus(source_codec) => copy("opus", "opus"),
        Format::Opus | Format::M4a if is_aac(source_codec) => {
            let mut recipe = copy("ipod", "m4a");
            recipe
                .codec
                .extend(["-movflags".into(), "+faststart".into()]);
            recipe
        }
        Format::Opus => Recipe {
            codec: vec![
                "-c:a".into(),
                "libopus".into(),
                "-b:a".into(),
                "160k".into(),
            ],
            muxer: "opus",
            extension: "opus",
            transcodes: true,
        },
        Format::Mp3 => Recipe {
            codec: vec![
                "-c:a".into(),
                "libmp3lame".into(),
                "-b:a".into(),
                format!("{}k", bitrates.mp3),
            ],
            muxer: "mp3",
            extension: "mp3",
            transcodes: true,
        },
        Format::M4a => Recipe {
            codec: vec![
                "-c:a".into(),
                aac_encoder.into(),
                "-b:a".into(),
                format!("{}k", bitrates.m4a),
                "-movflags".into(),
                "+faststart".into(),
            ],
            muxer: "ipod",
            extension: "m4a",
            transcodes: true,
        },
    }
}

/// The best AAC encoder ffmpeg has: Fraunhofer's `libfdk_aac` when it
/// encodes — a build may list it and still lack the library, as FFmpeg in
/// an AppImage on a host without libfdk-aac — else ffmpeg's own `aac`.
/// Tried once, on a tenth of a second of silence.
pub(crate) fn aac_encoder() -> &'static str {
    static ENCODER: OnceLock<&'static str> = OnceLock::new();
    ENCODER.get_or_init(|| {
        let encodes = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-hide_banner", "-loglevel", "error"])
            .args([
                "-f",
                "lavfi",
                "-i",
                "anullsrc=r=48000:cl=stereo",
                "-t",
                "0.1",
            ])
            .args(["-c:a", "libfdk_aac", "-f", "null", "-"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if encodes { "libfdk_aac" } else { "aac" }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_parse_and_name_their_files() {
        assert_eq!("Opus".parse::<Format>(), Ok(Format::Opus));
        assert_eq!("mp3".parse::<Format>(), Ok(Format::Mp3));
        assert_eq!("aac".parse::<Format>(), Ok(Format::M4a));
        assert!("flac".parse::<Format>().is_err());
        let extensions: Vec<_> = Format::ALL.iter().map(|f| f.extension()).collect();
        assert_eq!(extensions, ["opus", "mp3", "m4a"]);
    }

    #[test]
    fn opus_is_copied_and_the_rest_encoded_once() {
        let opus = recipe(Format::Opus, "opus", "aac");
        assert!(!opus.transcodes);
        assert_eq!((opus.muxer, opus.extension), ("opus", "opus"));

        let mp3 = recipe(Format::Mp3, "opus", "aac");
        assert!(mp3.transcodes && mp3.codec.contains(&"320k".to_owned()));

        let m4a = recipe(Format::M4a, "opus", "libfdk_aac");
        assert!(m4a.transcodes && m4a.codec.contains(&"libfdk_aac".to_owned()));
        assert_eq!((m4a.muxer, m4a.extension), ("ipod", "m4a"));
    }

    #[test]
    fn bit_rates_are_chosen_from_what_is_offered() {
        let rates = Bitrates::new(200, 100);
        assert_eq!(rates, Bitrates { mp3: 192, m4a: 128 });
        assert_eq!(Bitrates::new(999, 999), Bitrates::default());
        let mp3 = recipe_at(Format::Mp3, "opus", "aac", rates);
        assert!(
            mp3.codec.windows(2).any(|w| w == ["-b:a", "192k"]),
            "{mp3:?}"
        );
        let m4a = recipe_at(Format::M4a, "opus", "aac", rates);
        assert!(
            m4a.codec.windows(2).any(|w| w == ["-b:a", "128k"]),
            "{m4a:?}"
        );
    }

    #[test]
    fn an_aac_stream_is_never_encoded_again_into_aac_or_opus() {
        for format in [Format::Opus, Format::M4a] {
            let recipe = recipe(format, "mp4a.40.2", "aac");
            assert!(!recipe.transcodes, "{format:?}");
            assert_eq!(recipe.extension, "m4a");
        }
        assert!(recipe(Format::Mp3, "mp4a.40.2", "aac").transcodes);
    }
}
