//! A [`StreamResolver`] over the yt-dlp program.
//!
//! yt-dlp solves YouTube's JavaScript challenges with an external runtime
//! (Node.js here) and needs the account's cookies for Premium formats. The
//! cookies go to a `cookies.txt` that only the user can read, inside a fresh
//! private directory under `$XDG_RUNTIME_DIR`, removed when the call ends.
//!
//! With cookies, yt-dlp first goes the quick way ([`QUICK_ARGS`]); when that
//! fails and yt-dlp's own way works, the quick way is dropped for the run.
//! `ORBIS_YTDLP_QUICK=0` never tries it.
//!
//! With a cache folder of its own ([`YtDlp::with_cache_dir`]), yt-dlp also
//! gets orbis's plugin (`orbis_plugin.py`), which keeps the player JS that
//! Node.js has preprocessed: about 0.7 s less a link. The plugin also lets
//! yt-dlp keep running and answer requests (the worker, `worker.rs`): while
//! [`YtDlp::allow_worker`] allows it, links come from there, about 0.9 s
//! sooner again; `ORBIS_YTDLP_WORKER=0` never starts one.
//!
//! When YouTube changes its player, an older yt-dlp may no longer solve its
//! challenges and finds no audio: its warnings say so, and the error is
//! [`StreamError::Outdated`] with yt-dlp's version instead of yt-dlp's
//! last line ("Requested format is not available").

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use tokio::process::Command;

use crate::worker::{self, Failure, Workers};
use crate::{Stream, StreamError, StreamResolver, Urgency};

/// Formats in order of preference: Opus ~256 kbit/s (Premium), Opus 251,
/// any other Opus, AAC 140, anything with audio.
pub const DEFAULT_FORMAT: &str = "774/251/bestaudio[acodec=opus]/140/bestaudio";

/// How long one call may take before it is abandoned.
const TIMEOUT: Duration = Duration::from_secs(45);

/// How long `yt-dlp --version` may take.
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

/// The quick way to a signed-in link: only the `web_music` client, which
/// gives a Premium account Opus 774 without a PO token, and none of the
/// other clients' config pages. Measured on the owner's Premium account
/// (23 November 2026, yt-dlp 2026.10.26): 3.1 s a link against 4.5 s with
/// yt-dlp's defaults, the same format.
pub const QUICK_ARGS: [&str; 2] = [
    "--extractor-args",
    "youtube:player_client=web_music;player_skip=configs",
];

/// orbis's plugin for yt-dlp; [`plugin_dir`] writes it out.
const PLUGIN: &str = include_str!("orbis_plugin.py");

/// Preprocessed players kept in yt-dlp's cache: ~4.4 MB each, and YouTube
/// brings out a new one every few days or weeks.
const PLAYERS_KEPT: usize = 2;

/// Gives the current cookies in the Netscape format, or `None` to resolve
/// without signing in.
pub type CookieSource = Arc<dyn Fn() -> Option<String> + Send + Sync>;

pub struct YtDlp {
    program: PathBuf,
    js_runtime: Option<String>,
    format: String,
    cookies: Option<CookieSource>,
    /// yt-dlp's cache folder, and with it orbis's plugin; `None` leaves
    /// yt-dlp's own (`~/.cache/yt-dlp`) and runs without the plugin.
    cache_dir: Option<PathBuf>,
    /// The long-lived yt-dlp, with the cache folder and the plugin.
    workers: Option<Workers>,
    /// The quick way is still worth trying.
    quick: AtomicBool,
}

impl std::fmt::Debug for YtDlp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("YtDlp")
            .field("program", &self.program)
            .field("js_runtime", &self.js_runtime)
            .field("format", &self.format)
            .field("cookies", &self.cookies.is_some())
            .field("cache_dir", &self.cache_dir)
            .field("workers", &self.workers)
            .field("quick", &self.quick.load(Ordering::Relaxed))
            .finish()
    }
}

impl Default for YtDlp {
    fn default() -> Self {
        Self {
            program: PathBuf::from("yt-dlp"),
            js_runtime: Some("node".to_owned()),
            format: DEFAULT_FORMAT.to_owned(),
            cookies: None,
            cache_dir: None,
            workers: None,
            quick: AtomicBool::new(std::env::var("ORBIS_YTDLP_QUICK").as_deref() != Ok("0")),
        }
    }
}

impl YtDlp {
    pub fn with_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.program = program.into();
        self
    }

    /// The JavaScript runtime yt-dlp uses (`node`, `deno`, `bun`…), or `None`
    /// for yt-dlp's default.
    pub fn with_js_runtime(mut self, runtime: Option<String>) -> Self {
        self.js_runtime = runtime;
        self
    }

    pub fn with_format(mut self, format: impl Into<String>) -> Self {
        self.format = format.into();
        self
    }

    pub fn with_cookies(mut self, cookies: CookieSource) -> Self {
        self.cookies = Some(cookies);
        self
    }

    /// Keeps yt-dlp's cache in `dir` instead of `~/.cache/yt-dlp`, and
    /// hands yt-dlp orbis's plugin, which keeps preprocessed players there
    /// and makes the worker possible. All but the newest [`PLAYERS_KEPT`]
    /// players are removed now.
    pub fn with_cache_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        prune_players(&dir.join("challenge-solver"), PLAYERS_KEPT);
        self.cache_dir = Some(dir);
        let wanted = std::env::var("ORBIS_YTDLP_WORKER").as_deref() != Ok("0");
        self.workers = plugin_dir()
            .filter(|_| wanted)
            .map(|plugins| Workers::new(plugins.to_path_buf(), worker::IDLE));
        self
    }

    /// Lets links come from a long-lived yt-dlp, or stops it: orbis allows
    /// it while the window is shown. Without a cache folder (or with
    /// `ORBIS_YTDLP_WORKER=0`) there is none.
    pub fn allow_worker(&self, allowed: bool) {
        if let Some(workers) = &self.workers {
            workers.allow(allowed);
        }
    }

    /// Starts the long-lived yt-dlp now, when it is allowed and not
    /// running, so the next link does not wait for it.
    pub async fn warm_up(&self) {
        if let Some(workers) = &self.workers {
            workers.warm_up(&self.program).await;
        }
    }

    /// The process ID of the running long-lived yt-dlp.
    pub fn worker_pid(&self) -> Option<u32> {
        self.workers.as_ref().and_then(Workers::pid)
    }

    /// Stops trying the quick way, such as after mpv could not open a link
    /// it gave.
    pub fn drop_quick(&self) {
        self.quick.store(false, Ordering::Relaxed);
    }

    /// Resolves the quick way when it may work, else (or when it fails) as
    /// yt-dlp does by default.
    async fn resolve_with_fallback(
        &self,
        video_id: &str,
        urgency: Urgency,
    ) -> Result<Stream, StreamError> {
        let signed_in = self.cookies.is_some();
        if !(signed_in && self.quick.load(Ordering::Relaxed)) {
            return self.attempt(video_id, false, urgency).await;
        }
        match self.attempt(video_id, true, urgency).await {
            Ok(stream) => Ok(stream),
            Err(quick_error) => {
                let stream = self.attempt(video_id, false, urgency).await?;
                // The usual way works where the quick one did not: the quick
                // one is broken for this account or this yt-dlp.
                eprintln!("orbis: the quick way to links failed, dropped: {quick_error}");
                self.drop_quick();
                Ok(stream)
            }
        }
    }

    /// Resolves `video_id` once, the quick way or not: by the worker when
    /// one may run, else (or when it cannot answer) by a yt-dlp of its own.
    async fn attempt(
        &self,
        video_id: &str,
        quick: bool,
        urgency: Urgency,
    ) -> Result<Stream, StreamError> {
        if let Some(workers) = &self.workers {
            let request = json!({
                "video_id": video_id,
                "format": self.format,
                "quick": quick,
                "cookies": self.cookies.as_ref().and_then(|source| source()),
                "js_runtime": self.js_runtime,
                "cache_dir": self.cache_dir.as_ref().map(|d| d.to_string_lossy()),
            });
            let failed = |message: String| StreamError::Failed {
                video_id: video_id.to_owned(),
                message,
            };
            match workers
                .resolve(&self.program, request, urgency, TIMEOUT)
                .await
            {
                Ok(info) => {
                    return parse_info(video_id, &info)
                        .ok_or_else(|| failed("no URL in yt-dlp's answer".into()));
                }
                Err(Failure::Ytdlp(output)) => return Err(failure(video_id, &output)),
                Err(Failure::Timeout) => {
                    return Err(StreamError::Timeout {
                        video_id: video_id.to_owned(),
                        seconds: TIMEOUT.as_secs(),
                    });
                }
                Err(Failure::Worker(message)) => {
                    eprintln!(
                        "orbis: the yt-dlp worker gave no link, one yt-dlp run does: {message}"
                    );
                }
                Err(Failure::Off) => {}
            }
        }
        self.run(video_id, if quick { &QUICK_ARGS } else { &[] })
            .await
    }

    /// The yt-dlp command for `video_id`, and the private folder of its
    /// cookies, which must live until yt-dlp exits.
    fn command(
        &self,
        video_id: &str,
        extra: &[&str],
    ) -> Result<(Command, Option<tempfile::TempDir>), StreamError> {
        let mut command = Command::new(&self.program);
        // Warnings stay on: those of a failed run tell why (see `failure`).
        command
            .args(["--no-playlist", "--no-progress", "--dump-json"])
            .args(["--format", &self.format])
            .args(extra);
        if let Some(runtime) = &self.js_runtime {
            command.args(["--js-runtimes", runtime]);
        }
        if let Some(cache) = &self.cache_dir {
            command.arg("--cache-dir").arg(cache);
            if let Some(plugins) = plugin_dir() {
                command.arg("--plugin-dirs").arg(plugins);
            }
        }

        let mut private_dir = None;
        if let Some(cookies) = self.cookies.as_ref().and_then(|source| source()) {
            let (dir, file) = write_private_cookies(&cookies).map_err(|e| StreamError::Failed {
                video_id: video_id.to_owned(),
                message: e.to_string(),
            })?;
            command.arg("--cookies").arg(&file);
            private_dir = Some(dir);
        }
        command
            .arg(format!("https://music.youtube.com/watch?v={video_id}"))
            .kill_on_drop(true);
        Ok((command, private_dir))
    }

    async fn run(&self, video_id: &str, extra: &[&str]) -> Result<Stream, StreamError> {
        let failed = |message: String| StreamError::Failed {
            video_id: video_id.to_owned(),
            message,
        };
        // The cookies' folder is kept alive until yt-dlp exits.
        let (mut command, _private_dir) = self.command(video_id, extra)?;

        let output = tokio::time::timeout(TIMEOUT, command.output())
            .await
            .map_err(|_| StreamError::Timeout {
                video_id: video_id.to_owned(),
                seconds: TIMEOUT.as_secs(),
            })?
            .map_err(|e| StreamError::Unavailable {
                program: self.program.display().to_string(),
                message: e.to_string(),
            })?;
        if !output.status.success() {
            return Err(failure(video_id, &String::from_utf8_lossy(&output.stderr)));
        }
        let json: Value =
            serde_json::from_slice(&output.stdout).map_err(|e| failed(format!("bad JSON: {e}")))?;
        parse_info(video_id, &json).ok_or_else(|| failed("no URL in yt-dlp's answer".into()))
    }

    /// yt-dlp's version, such as `2026.10.26`.
    async fn version(&self) -> Option<String> {
        let mut command = Command::new(&self.program);
        command.arg("--version").kill_on_drop(true);
        let output = tokio::time::timeout(VERSION_TIMEOUT, command.output())
            .await
            .ok()?
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let version = stdout.lines().next()?.trim();
        (output.status.success() && !version.is_empty()).then(|| version.to_owned())
    }
}

impl StreamResolver for YtDlp {
    async fn resolve(&self, video_id: &str, urgency: Urgency) -> Result<Stream, StreamError> {
        match self.resolve_with_fallback(video_id, urgency).await {
            // Which yt-dlp is too old: asked once the last attempt has
            // failed, not for each.
            Err(StreamError::Outdated { video_id, .. }) => Err(StreamError::Outdated {
                video_id,
                version: self.version().await,
                bundled: bundled(
                    &self.program,
                    std::env::var_os("APPDIR").as_deref(),
                    std::env::var_os("PATH").as_deref(),
                ),
            }),
            other => other,
        }
    }
}

/// What a failed yt-dlp said, from its stderr or the worker's answer (the
/// warnings, then the error): [`StreamError::Outdated`] when its warnings
/// say the challenge solver ran and failed, else its last error line.
/// Without a JavaScript runtime the solver does not run, and only the
/// second warning comes: that is not an old yt-dlp.
fn failure(video_id: &str, output: &str) -> StreamError {
    if output.contains("Error solving") && output.contains(" solving failed") {
        return StreamError::Outdated {
            video_id: video_id.to_owned(),
            version: None,
            bundled: false,
        };
    }
    let message = output
        .lines()
        .rev()
        .find(|line| line.contains("ERROR"))
        .or_else(|| output.lines().last())
        .unwrap_or("yt-dlp failed")
        .trim()
        .to_owned();
    StreamError::Failed {
        video_id: video_id.to_owned(),
        message,
    }
}

/// Whether `program` is the yt-dlp inside orbis's AppImage: the AppImage's
/// runtime names the folder it is mounted in by `APPDIR` (`appdir`), and
/// `AppRun` puts the yt-dlp there first on `PATH` (`path`).
fn bundled(program: &Path, appdir: Option<&OsStr>, path: Option<&OsStr>) -> bool {
    let Some(appdir) = appdir.filter(|dir| !dir.is_empty()) else {
        return false;
    };
    let found = if program.components().count() > 1 {
        Some(program.to_path_buf())
    } else {
        path.into_iter()
            .flat_map(std::env::split_paths)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    };
    found.is_some_and(|found| found.starts_with(appdir))
}

/// Writes `cookies` to `cookies.txt` (mode 0600) in a new 0700 directory.
fn write_private_cookies(cookies: &str) -> std::io::Result<(tempfile::TempDir, PathBuf)> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = tempfile::Builder::new()
        .prefix("orbis-ytdlp-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(base)?;
    let path = dir.path().join("cookies.txt");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    file.write_all(cookies.as_bytes())?;
    Ok((dir, path))
}

/// The folder handed to yt-dlp with `--plugin-dirs`, with orbis's plugin in
/// it, made once a run: `orbis-yt-dlp-plugins` in `$XDG_RUNTIME_DIR`, or a
/// fresh private folder in the temporary one (a fixed name there could be
/// taken by another user first). `None` when it cannot be made: yt-dlp then
/// runs without the plugin.
fn plugin_dir() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute());
        let made = match runtime {
            Some(runtime) => write_plugin(&runtime.join("orbis-yt-dlp-plugins")),
            None => {
                use std::os::unix::fs::PermissionsExt;
                tempfile::Builder::new()
                    .prefix("orbis-yt-dlp-plugins-")
                    .permissions(std::fs::Permissions::from_mode(0o700))
                    .tempdir()
                    .and_then(|dir| write_plugin(&dir.keep()))
            }
        };
        made.map_err(|e| eprintln!("orbis: yt-dlp runs without orbis's plugin: {e}"))
            .ok()
    })
    .as_deref()
}

/// Writes the plugin where yt-dlp looks for it under `root`
/// (`orbis/yt_dlp_plugins/extractor/orbis.py`), making `root` with mode 0700
/// when it is missing. yt-dlp runs the code it finds there, so a `root` that
/// is not this user's private folder is refused.
fn write_plugin(root: &Path) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    match std::fs::DirBuilder::new().mode(0o700).create(root) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e),
        _ => {}
    }
    let found = std::fs::symlink_metadata(root)?;
    let me = std::fs::metadata("/proc/self")?.uid();
    if !found.is_dir() || found.uid() != me || found.mode() & 0o077 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{} is not a private folder", root.display()),
        ));
    }
    let folder = root.join("orbis/yt_dlp_plugins/extractor");
    std::fs::create_dir_all(&folder)?;
    let file = folder.join("orbis.py");
    if std::fs::read_to_string(&file).ok().as_deref() != Some(PLUGIN) {
        // Another orbis may be writing it too: each writes its own file
        // and renames it into place.
        let part = folder.join(format!(".orbis.py.{}", std::process::id()));
        std::fs::write(&part, PLUGIN)?;
        std::fs::rename(&part, &file)?;
    }
    Ok(root.to_path_buf())
}

/// Removes all but the `keep` newest preprocessed players (`player,…`
/// files) from yt-dlp's `challenge-solver` cache folder; the solver's own
/// scripts stay. A file that cannot be removed is left.
fn prune_players(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut players: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("player,"))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    players.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    for (_, path) in players.into_iter().skip(keep) {
        let _ = std::fs::remove_file(path);
    }
}

/// Reads the chosen format from yt-dlp's `--dump-json` output.
fn parse_info(video_id: &str, info: &Value) -> Option<Stream> {
    // With a single chosen format its fields are at the top level; with a
    // merged format they sit in `requested_formats`.
    let format = info
        .get("requested_formats")
        .and_then(Value::as_array)
        .and_then(|f| {
            f.iter()
                .find(|f| f.get("acodec").and_then(Value::as_str) != Some("none"))
        })
        .unwrap_or(info);
    let url = format.get("url")?.as_str()?.to_owned();
    let text = |key: &str| format.get(key).and_then(Value::as_str).map(str::to_owned);
    let headers = format
        .get("http_headers")
        .and_then(Value::as_object)
        .map(|h| {
            h.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    Some(Stream {
        video_id: video_id.to_owned(),
        expires_at: expiry_of(&url),
        url,
        format_id: text("format_id").unwrap_or_default(),
        codec: text("acodec").unwrap_or_default(),
        container: text("ext").unwrap_or_default(),
        bitrate_kbps: format
            .get("abr")
            .and_then(Value::as_f64)
            .or_else(|| format.get("tbr").and_then(Value::as_f64)),
        sample_rate: format
            .get("asr")
            .and_then(Value::as_u64)
            .and_then(|r| u32::try_from(r).ok()),
        duration: info.get("duration").and_then(Value::as_f64),
        headers,
    })
}

/// The `expire` query parameter of a googlevideo URL.
fn expiry_of(url: &str) -> Option<i64> {
    let query = url.split_once('?')?.1;
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix("expire="))
        .and_then(|v| v.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_chosen_format() {
        let info = json!({
            "id": "vid00000001", "duration": 335.0,
            "format_id": "774", "acodec": "opus", "ext": "webm", "abr": 256.4, "asr": 48000,
            "url": "https://rr1.example.invalid/videoplayback?expire=1800000000&itag=774&sig=x",
            "http_headers": {"User-Agent": "UA", "Accept": "*/*"}
        });
        let stream = parse_info("vid00000001", &info).unwrap();
        assert_eq!(stream.format_id, "774");
        assert_eq!(
            (stream.codec.as_str(), stream.container.as_str()),
            ("opus", "webm")
        );
        assert_eq!(stream.bitrate_kbps, Some(256.4));
        assert_eq!(stream.sample_rate, Some(48000));
        assert_eq!(stream.expires_at, Some(1_800_000_000));
        assert_eq!(stream.duration, Some(335.0));
        assert!(stream.headers.contains(&("User-Agent".into(), "UA".into())));
        assert!(
            !format!("{stream:?}").contains("sig=x"),
            "Debug hides the URL"
        );
    }

    #[test]
    fn reads_the_audio_part_of_a_merged_format() {
        let info = json!({"requested_formats": [
            {"format_id": "137", "acodec": "none", "url": "https://v.example.invalid/video"},
            {"format_id": "251", "acodec": "opus", "ext": "webm", "url": "https://v.example.invalid/audio?expire=5"}
        ]});
        let stream = parse_info("vid", &info).unwrap();
        assert_eq!(
            (stream.format_id.as_str(), stream.expires_at),
            ("251", Some(5))
        );
        assert_eq!(parse_info("vid", &json!({"title": "no url"})), None);
    }

    #[test]
    fn cookie_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, file) = write_private_cookies("# Netscape HTTP Cookie File\n").unwrap();
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(dir.path()), 0o700);
        assert_eq!(mode(&file), 0o600);
        let path = dir.path().to_path_buf();
        drop(dir);
        assert!(!path.exists(), "removed with the guard");
    }

    #[test]
    fn the_cache_folder_brings_the_plugin() {
        let args = |resolver: &YtDlp| -> Vec<String> {
            let (command, _dir) = resolver.command("vid", &[]).unwrap();
            command
                .as_std()
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect()
        };
        let after = |args: &[String], flag: &str| {
            args.iter()
                .position(|a| a == flag)
                .map(|i| args[i + 1].clone())
        };

        let plain = args(&YtDlp::default());
        assert_eq!(after(&plain, "--cache-dir"), None);
        assert_eq!(after(&plain, "--plugin-dirs"), None);

        let cache = tempfile::tempdir().unwrap();
        let with_cache = args(&YtDlp::default().with_cache_dir(cache.path()));
        assert_eq!(
            after(&with_cache, "--cache-dir").as_deref(),
            cache.path().to_str()
        );
        let plugins = after(&with_cache, "--plugin-dirs").expect("plugin folder");
        let plugin = Path::new(&plugins).join("orbis/yt_dlp_plugins/extractor/orbis.py");
        assert_eq!(std::fs::read_to_string(plugin).unwrap(), PLUGIN);
        assert_eq!(
            with_cache.last().map(String::as_str),
            Some("https://music.youtube.com/watch?v=vid")
        );
    }

    #[test]
    fn the_plugin_folder_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("plugins");
        assert_eq!(write_plugin(&root).unwrap(), root);
        let mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        // Written again over a changed file, and the same twice.
        let file = root.join("orbis/yt_dlp_plugins/extractor/orbis.py");
        std::fs::write(&file, "changed").unwrap();
        write_plugin(&root).unwrap();
        write_plugin(&root).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), PLUGIN);

        let open = base.path().join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(write_plugin(&open).is_err(), "others may read or write it");
    }

    #[test]
    fn only_the_newest_players_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let day = Duration::from_secs(24 * 3600);
        let now = SystemTime::now();
        for (name, age) in [
            ("player,3Ahttps,3A,2F,2Fa.json", 1),
            ("player,3Ahttps,3A,2F,2Fb.json", 9),
            ("player,3Ahttps,3A,2F,2Fc.json", 3),
            ("player,3Ahttps,3A,2F,2Fd.json", 20),
            ("lib.json", 30),
        ] {
            let file = std::fs::File::create(dir.path().join(name)).unwrap();
            file.set_modified(now - day * age).unwrap();
        }
        prune_players(dir.path(), 2);
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "lib.json",
                "player,3Ahttps,3A,2F,2Fa.json",
                "player,3Ahttps,3A,2F,2Fc.json"
            ]
        );
        prune_players(&dir.path().join("missing"), 2);
    }

    #[test]
    fn the_plugin_is_python() {
        // Python's own parser, when there is one: a syntax error would only
        // show as yt-dlp failing on every link.
        let Ok(output) = std::process::Command::new("python3")
            .args([
                "-I",
                "-c",
                "import sys; compile(sys.stdin.read(), 'orbis.py', 'exec')",
            ])
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.take().unwrap().write_all(PLUGIN.as_bytes())?;
                child.wait_with_output()
            })
        else {
            return;
        };
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    async fn without_the_worker_a_run_of_its_own_gives_the_link() {
        use std::os::unix::fs::PermissionsExt;
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        // A yt-dlp that serves `orbis-serve:` (format 774) and dies on the
        // track `crash`; run for one track it answers with format 251.
        const FAKE: &str = r#"#!/usr/bin/env python3
import json, os, sys
if sys.argv[-1] == 'orbis-serve:':
    print(json.dumps({'ready': True}), flush=True)
    for line in sys.stdin:
        r = json.loads(line)
        if r['video_id'] == 'crash':
            os._exit(3)
        print(json.dumps({'n': r['n'], 'info': {'url': 'https://w.example.invalid/?expire=9',
                                                'format_id': '774'}}), flush=True)
else:
    print(json.dumps({'url': 'https://r.example.invalid/?expire=9', 'format_id': '251'}))
"#;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("yt-dlp");
        std::fs::write(&program, FAKE).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        crate::worker::tests::wait_until_runnable(&program);
        let resolver = YtDlp::default()
            .with_program(&program)
            .with_cache_dir(dir.path().join("cache"));
        let format = |stream: Result<Stream, StreamError>| stream.unwrap().format_id;

        assert_eq!(
            format(resolver.resolve("a", Urgency::Now).await),
            "251",
            "not allowed yet"
        );
        resolver.allow_worker(true);
        assert_eq!(format(resolver.resolve("a", Urgency::Now).await), "774");
        assert_eq!(
            format(resolver.resolve("crash", Urgency::Now).await),
            "251",
            "the worker died"
        );
        assert_eq!(
            format(resolver.resolve("b", Urgency::Ahead).await),
            "774",
            "a new one"
        );
        resolver.allow_worker(false);
        assert_eq!(resolver.worker_pid(), None);
        assert_eq!(
            format(resolver.resolve("c", Urgency::Now).await),
            "251",
            "in the tray"
        );
    }

    /// yt-dlp 2026.10.26 on YouTube's player `b485e332` (25 November 2026),
    /// shortened: the solver ran in Node.js and failed, so only images were
    /// left.
    const SOLVER_FAILED: &str = r#"WARNING: [youtube] [jsc] Error solving 2 challenge requests using "node" provider: Error running node process (returncode: 1): <anonymous_script>:4345
TypeError: a.addOnDisposeCallback is not a function
WARNING: [youtube] vid: Signature solving failed: Some formats may be missing. Ensure you have a supported JavaScript runtime and challenge solver script distribution installed.
WARNING: [youtube] vid: n challenge solving failed: Some formats may be missing. Ensure you have a supported JavaScript runtime and challenge solver script distribution installed.
WARNING: Only images are available for download. use --list-formats to see them
ERROR: [youtube] vid: Requested format is not available. Use --list-formats for a list of available formats
"#;

    #[test]
    fn a_failed_challenge_solver_means_an_old_yt_dlp() {
        assert!(matches!(
            failure("vid", SOLVER_FAILED),
            StreamError::Outdated { video_id, .. } if video_id == "vid"
        ));
        let message = |output: &str| match failure("vid", output) {
            StreamError::Failed { message, .. } => message,
            other => panic!("unexpected {other:?}"),
        };
        // No JavaScript runtime: the solver did not run.
        assert_eq!(
            message(
                "WARNING: [youtube] vid: n challenge solving failed: Some formats may be missing.\n\
                 ERROR: [youtube] vid: Requested format is not available.\n"
            ),
            "ERROR: [youtube] vid: Requested format is not available."
        );
        assert_eq!(
            message("WARNING: [youtube] slow\nERROR: [youtube] gone: Video unavailable\n"),
            "ERROR: [youtube] gone: Video unavailable"
        );
        assert_eq!(message("Traceback: boom\n"), "Traceback: boom");
        assert_eq!(message(""), "yt-dlp failed");
    }

    #[test]
    fn the_old_yt_dlp_is_named() {
        let text = |version: Option<&str>, bundled| {
            StreamError::Outdated {
                video_id: "vid".into(),
                version: version.map(str::to_owned),
                bundled,
            }
            .to_string()
        };
        assert_eq!(
            text(Some("2026.10.26"), false),
            "yt-dlp 2026.10.26 is likely out of date: its challenge solver failed on \
             YouTube's current player. Update yt-dlp"
        );
        assert_eq!(
            text(Some("2026.10.26"), true),
            "yt-dlp 2026.10.26 inside the AppImage is likely out of date: its challenge \
             solver failed on YouTube's current player. A newer AppImage is needed"
        );
        assert!(text(None, false).starts_with("yt-dlp is likely out of date: "));
    }

    #[test]
    fn the_yt_dlp_inside_the_appimage_is_told_apart() {
        let root = tempfile::tempdir().unwrap();
        let appdir = root.path().join("mount");
        let inside = appdir.join("opt/yt-dlp");
        let system = root.path().join("bin");
        for dir in [&inside, &system] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("yt-dlp"), "").unwrap();
        }
        let path = |dirs: &[&Path]| std::env::join_paths(dirs).unwrap();
        let appdir = Some(appdir.as_os_str());
        let program = Path::new("yt-dlp");

        let first_inside = path(&[&inside, &system]);
        assert!(bundled(program, appdir, Some(&first_inside)));
        let first_system = path(&[&system, &inside]);
        assert!(!bundled(program, appdir, Some(&first_system)));
        assert!(!bundled(program, None, Some(&first_inside)), "no AppImage");
        assert!(!bundled(Path::new("other"), appdir, Some(&first_inside)));
        assert!(bundled(&inside.join("yt-dlp"), appdir, None));
        assert!(!bundled(&system.join("yt-dlp"), appdir, None));
    }

    #[tokio::test]
    async fn an_old_yt_dlp_is_named_by_its_version() {
        use std::os::unix::fs::PermissionsExt;
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        // A yt-dlp whose solver fails, both run for one track and as the
        // worker, which answers with the warnings like orbis's plugin.
        const FAKE: &str = r#"#!/usr/bin/env python3
import json, sys
OUTPUT = @OUTPUT@
if sys.argv[1:] == ['--version']:
    print('2026.10.26')
elif sys.argv[-1] == 'orbis-serve:':
    print(json.dumps({'ready': True}), flush=True)
    for line in sys.stdin:
        r = json.loads(line)
        print(json.dumps({'n': r['n'], 'error': OUTPUT.strip()}), flush=True)
else:
    sys.stderr.write(OUTPUT)
    sys.exit(1)
"#;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("yt-dlp");
        let script = FAKE.replace("@OUTPUT@", &format!("{SOLVER_FAILED:?}"));
        std::fs::write(&program, script).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        crate::worker::tests::wait_until_runnable(&program);
        let resolver = YtDlp::default()
            .with_program(&program)
            .with_cache_dir(dir.path().join("cache"));

        for (worker, how) in [(false, "a run of its own"), (true, "the worker")] {
            resolver.allow_worker(worker);
            match resolver.resolve("vid", Urgency::Now).await {
                Err(StreamError::Outdated {
                    version, bundled, ..
                }) => {
                    assert_eq!(version.as_deref(), Some("2026.10.26"), "{how}");
                    assert!(!bundled, "{how}");
                }
                other => panic!("{how}: unexpected {other:?}"),
            }
            assert_eq!(resolver.worker_pid().is_some(), worker, "{how}");
        }
        resolver.allow_worker(false);
    }

    #[tokio::test]
    async fn missing_program_is_reported() {
        let resolver = YtDlp::default().with_program("/nonexistent/yt-dlp");
        match resolver.resolve("vid", Urgency::Now).await {
            Err(StreamError::Unavailable { program, .. }) => {
                assert!(program.contains("nonexistent"))
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
