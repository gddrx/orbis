//! How loud a track is, measured by ffmpeg's EBU R128 filter, and the gain
//! tags players level it with: `R128_TRACK_GAIN` in Opus (to -23 LUFS, as
//! RFC 7845 asks), `REPLAYGAIN_TRACK_GAIN` and `_PEAK` in MP3 and M4A (to
//! -18 LUFS, ReplayGain 2.0). An album's loudness is that of all its
//! tracks played one after another ([`measure_album`]), for the same tags
//! with `ALBUM` in place of `TRACK`.

use std::ffi::OsString;
use std::path::Path;

use crate::DownloadError;
use crate::convert::ffmpeg;

/// The loudness Opus gains lead to (EBU R128).
const R128_REFERENCE: f64 = -23.0;
/// The loudness ReplayGain 2.0 gains lead to.
const REPLAYGAIN_REFERENCE: f64 = -18.0;
/// What the filter reports when the gate leaves nothing: silence.
const SILENCE: f64 = -70.0;

/// The loudness of a track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loudness {
    /// Integrated loudness, LUFS.
    pub integrated: f64,
    /// True peak, dBFS; unknown for YouTube's own measure.
    pub peak: Option<f64>,
}

impl Loudness {
    /// The Opus gain: Q7.8 dB, as `R128_TRACK_GAIN` wants it.
    pub fn r128_gain(&self) -> i16 {
        let q78 = ((R128_REFERENCE - self.integrated) * 256.0).round();
        q78.clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16
    }

    /// The ReplayGain track gain, such as `-6.20 dB`.
    pub fn replaygain_gain(&self) -> String {
        format!("{:.2} dB", REPLAYGAIN_REFERENCE - self.integrated)
    }

    /// The ReplayGain track peak as a linear amplitude, such as `0.988553`.
    pub fn replaygain_peak(&self) -> Option<String> {
        self.peak.map(|db| format!("{:.6}", 10f64.powf(db / 20.0)))
    }
}

/// The loudness in the summary the `ebur128` filter prints at the end:
/// `I: -9.1 LUFS` under "Integrated loudness" and `Peak: 0.4 dBFS` under
/// "True peak". Silence (or no summary) has no loudness.
pub(crate) fn parse(text: &str) -> Option<Loudness> {
    let summary = &text[text.rfind("Summary:")?..];
    let value = |label: &str, unit: &str| {
        summary.lines().find_map(|line| {
            line.trim()
                .strip_prefix(label)?
                .trim()
                .strip_suffix(unit)?
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
        })
    };
    let integrated = value("I:", "LUFS").filter(|i| *i > SILENCE)?;
    Some(Loudness {
        integrated,
        peak: value("Peak:", "dBFS"),
    })
}

/// Measures the first audio stream of `path`; `progress` gets the share of
/// `duration` done. `None` when the track is silent.
pub(crate) async fn measure(
    path: &Path,
    duration: Option<f64>,
    progress: &mut (dyn FnMut(f64) + Send),
) -> Result<Option<Loudness>, DownloadError> {
    let mut args: Vec<OsString> = ["-loglevel", "info", "-i"].map(OsString::from).into();
    args.push(path.into());
    args.extend(
        [
            "-map",
            "0:a:0",
            "-af",
            "ebur128=peak=true:framelog=quiet",
            "-f",
            "null",
            "-",
        ]
        .map(OsString::from),
    );
    let report = ffmpeg(&args, duration, progress).await?;
    Ok(parse(&report))
}

/// Measures the files of an album as one: their audio one after another,
/// each brought to 48 kHz stereo. `None` when it is all silence.
pub(crate) async fn measure_album(
    paths: &[std::path::PathBuf],
) -> Result<Option<Loudness>, DownloadError> {
    if paths.is_empty() {
        return Ok(None);
    }
    let mut args: Vec<OsString> = ["-loglevel", "info"].map(OsString::from).into();
    let mut graph = String::new();
    for (i, path) in paths.iter().enumerate() {
        args.push("-i".into());
        args.push(path.into());
        graph.push_str(&format!(
            "[{i}:a:0]aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo[a{i}];"
        ));
    }
    for i in 0..paths.len() {
        graph.push_str(&format!("[a{i}]"));
    }
    graph.push_str(&format!(
        "concat=n={}:v=0:a=1,ebur128=peak=true:framelog=quiet[out]",
        paths.len()
    ));
    args.extend(
        [
            "-filter_complex",
            &graph,
            "-map",
            "[out]",
            "-f",
            "null",
            "-",
        ]
        .map(OsString::from),
    );
    let report = ffmpeg(&args, None, &mut |_| {}).await?;
    Ok(parse(&report))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUMMARY: &str = "[Parsed_ebur128_0 @ 0x55] Summary:

  Integrated loudness:
    I:          -9.1 LUFS
    Threshold: -19.4 LUFS

  Loudness range:
    LRA:         4.2 LU
    Threshold: -29.3 LUFS
    LRA low:   -11.8 LUFS
    LRA high:   -7.6 LUFS

  True peak:
    Peak:        0.4 dBFS
";

    #[test]
    fn the_summary_gives_loudness_and_peak() {
        let loudness = parse(&format!("Stream #0:0: Audio: opus\n{SUMMARY}")).unwrap();
        assert_eq!(
            loudness,
            Loudness {
                integrated: -9.1,
                peak: Some(0.4)
            }
        );
        // -23 - (-9.1) = -13.9 dB, as Q7.8.
        assert_eq!(loudness.r128_gain(), -3558);
        assert_eq!(loudness.replaygain_gain(), "-8.90 dB");
        assert_eq!(loudness.replaygain_peak().as_deref(), Some("1.047129"));
    }

    #[test]
    fn silence_and_missing_summaries_have_no_loudness() {
        let silent = SUMMARY
            .replace("-9.1 LUFS", "-70.0 LUFS")
            .replace("0.4 dBFS", "-inf dBFS");
        assert_eq!(parse(&silent), None);
        assert_eq!(parse("no summary"), None);
        let no_peak = SUMMARY.replace("0.4 dBFS", "-inf dBFS");
        assert_eq!(parse(&no_peak).unwrap().peak, None);
    }

    #[test]
    fn quiet_tracks_are_raised_and_gains_stay_in_range() {
        let quiet = Loudness {
            integrated: -30.5,
            peak: None,
        };
        assert_eq!(quiet.r128_gain(), 1920);
        assert_eq!(quiet.replaygain_gain(), "12.50 dB");
        assert_eq!(quiet.replaygain_peak(), None);
        let absurd = Loudness {
            integrated: -500.0,
            peak: None,
        };
        assert_eq!(absurd.r128_gain(), i16::MAX);
    }

    #[tokio::test]
    async fn a_tone_is_measured() {
        let dir = tempfile::tempdir().unwrap();
        let Some(tone) = crate::convert::tests::opus_tone(dir.path()) else {
            eprintln!("ffmpeg is missing; skipping");
            return;
        };
        let mut shares = Vec::new();
        let loudness = measure(&tone, Some(3.0), &mut |s| shares.push(s))
            .await
            .unwrap()
            .unwrap();
        // ffmpeg's sine is at an eighth of full scale.
        assert!(
            (-30.0..-10.0).contains(&loudness.integrated),
            "{loudness:?}"
        );
        assert!(loudness.peak.is_some());
        assert_eq!(shares.last(), Some(&1.0));
    }
}
