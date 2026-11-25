//! ffmpeg makes the file from the fetched stream: copies Opus into an Ogg
//! `.opus`, or encodes MP3 and AAC. Only the audio goes in, without the
//! stream's metadata (tags are written separately).

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

use crate::DownloadError;
use crate::format::Recipe;

/// Runs ffmpeg on `input` into `output` by `recipe`; `progress` gets the
/// share done, from 0 to 1, when the length is known. Dropping the future
/// stops ffmpeg.
pub(crate) async fn convert(
    input: &Path,
    output: &Path,
    recipe: &Recipe,
    duration: Option<f64>,
    progress: &mut (dyn FnMut(f64) + Send),
) -> Result<(), DownloadError> {
    let mut args: Vec<OsString> = ["-loglevel", "error", "-y", "-i"]
        .map(OsString::from)
        .into();
    args.push(input.into());
    args.extend(["-map", "0:a:0", "-vn", "-map_metadata", "-1"].map(OsString::from));
    args.extend(recipe.codec.iter().map(OsString::from));
    args.extend(["-f", recipe.muxer].map(OsString::from));
    args.push(output.into());
    ffmpeg(&args, duration, progress).await.map(|_| ())
}

/// Runs ffmpeg with `args` (after its own `-nostdin -hide_banner` and
/// before `-progress`); `progress` gets the share of `duration` done.
/// Returns what ffmpeg wrote to stderr; a failure carries its last line.
/// Dropping the future stops ffmpeg.
pub(crate) async fn ffmpeg(
    args: &[OsString],
    duration: Option<f64>,
    progress: &mut (dyn FnMut(f64) + Send),
) -> Result<String, DownloadError> {
    let mut command = tokio::process::Command::new("ffmpeg");
    command
        .args([
            "-nostdin",
            "-hide_banner",
            "-nostats",
            "-progress",
            "pipe:1",
        ])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|e| DownloadError::NoFfmpeg(e.to_string()))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let errors = tokio::spawn(async move {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text).await;
        text
    });
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let (Some(micros), Some(duration)) = (
            line.strip_prefix("out_time_us=")
                .and_then(|v| v.trim().parse::<f64>().ok()),
            duration.filter(|d| *d > 0.0),
        ) {
            progress((micros / 1e6 / duration).clamp(0.0, 1.0));
        }
    }
    let status = child.wait().await?;
    let errors = errors.await.unwrap_or_default();
    if !status.success() {
        let last = errors.lines().rev().find(|l| !l.trim().is_empty());
        return Err(DownloadError::Ffmpeg(
            last.map_or_else(|| status.to_string(), str::to_owned),
        ));
    }
    progress(1.0);
    Ok(errors)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::format::{Format, aac_encoder, recipe};

    /// A three-second Opus tone in WebM, like YouTube's stream, or `None`
    /// without ffmpeg.
    pub(crate) fn opus_tone(dir: &Path) -> Option<PathBuf> {
        let path = dir.join("tone.webm");
        let made = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=3"])
            .args(["-c:a", "libopus", "-b:a", "128k", "-f", "webm"])
            .arg(&path)
            .status()
            .ok()?
            .success();
        made.then_some(path)
    }

    /// The codec and bit rate of the first audio stream, by ffprobe.
    pub(crate) fn probe(path: &Path) -> (String, String, String) {
        let out = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "a:0"])
            .args([
                "-show_entries",
                "stream=codec_name,bit_rate:format=format_name",
            ])
            .args(["-of", "default=noprint_wrappers=1"])
            .arg(path)
            .output()
            .expect("ffprobe");
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let field = |name: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(&format!("{name}=")))
                .unwrap_or_default()
                .to_owned()
        };
        (field("codec_name"), field("bit_rate"), field("format_name"))
    }

    #[tokio::test]
    async fn opus_is_copied_and_mp3_and_m4a_encoded() {
        let dir = tempfile::tempdir().unwrap();
        let Some(tone) = opus_tone(dir.path()) else {
            eprintln!("ffmpeg is missing; skipping");
            return;
        };
        for (format, codec, container) in [
            (Format::Opus, "opus", "ogg"),
            (Format::Mp3, "mp3", "mp3"),
            (Format::M4a, "aac", "mov,mp4,m4a,3gp,3g2,mj2"),
        ] {
            let recipe = recipe(format, "opus", aac_encoder());
            let out = dir.path().join(format!("out.{}", recipe.extension));
            let mut shares = Vec::new();
            convert(&tone, &out, &recipe, Some(3.0), &mut |s| shares.push(s))
                .await
                .unwrap();
            let (got_codec, bit_rate, got_container) = probe(&out);
            assert_eq!(
                (got_codec.as_str(), got_container.as_str()),
                (codec, container)
            );
            if format == Format::Mp3 {
                assert_eq!(bit_rate, "320000");
            }
            assert_eq!(shares.last(), Some(&1.0));
        }
    }

    #[tokio::test]
    async fn ffmpeg_errors_come_back() {
        let dir = tempfile::tempdir().unwrap();
        let broken = dir.path().join("broken.webm");
        std::fs::write(&broken, b"not a stream").unwrap();
        let recipe = recipe(Format::Opus, "opus", "aac");
        let error = convert(
            &broken,
            &dir.path().join("out.opus"),
            &recipe,
            None,
            &mut |_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            DownloadError::Ffmpeg(_) | DownloadError::NoFfmpeg(_)
        ));
    }
}
