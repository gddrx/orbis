//! The Google sign-in window: `orbis-login`, a separate process with a web
//! engine that lives only while the user signs in. Its profile is kept in
//! memory, so every sign-in starts clean and leaves nothing on disk; the
//! session it gets is the player's own.
//!
//! The cookies come back through a pipe that the window finds on its file
//! descriptor 3: never in arguments or the environment (both can be read in
//! `/proc`), never in files or logs. The window's side takes the pipe with
//! [`Channel::from_env`], which keeps it from the web engine's own processes.

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;
use std::process::{ExitStatus, Stdio};

use serde::{Deserialize, Serialize};

use crate::browser_cookies::{BrowserCookie, host_in_domains};

/// The variable that tells the window which descriptor is the channel.
pub const CHANNEL_VAR: &str = "ORBIS_LOGIN_FD";
/// The descriptor the channel gets in the window's process.
const CHANNEL_FD: RawFd = 3;
/// A message longer than this is refused: a session is a few kilobytes.
const MAX_MESSAGE: usize = 1 << 20;
/// The window's exit code when the user closed it without signing in.
pub const EXIT_CANCELLED: i32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum LoginWindowError {
    #[error("cannot start the sign-in window {program}: {source}")]
    Start {
        program: String,
        source: std::io::Error,
    },
    #[error("the sign-in window was closed without signing in")]
    Cancelled,
    #[error("the sign-in window failed ({0})")]
    Failed(ExitStatus),
    #[error("the sign-in window sent a message that is too long")]
    TooLong,
    #[error("the sign-in window sent a malformed message: {0}")]
    Malformed(#[from] serde_json::Error),
    #[error("reading the sign-in window's message failed: {0}")]
    Io(#[from] std::io::Error),
}

/// The language the window speaks (`en`, `uk`): orbis's.
static LANGUAGE: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// Makes the next sign-in windows speak `code` (`en`, `uk`).
pub fn set_language(code: &str) {
    if let Ok(mut language) = LANGUAGE.lock() {
        code.clone_into(&mut language);
    }
}

/// What the window sends: the cookies of its profile.
#[derive(Serialize, Deserialize)]
struct Message {
    cookies: Vec<BrowserCookie>,
}

/// Runs the sign-in window `program` and returns the cookies of `domains`
/// it signed in with. Dropping the future closes the window.
pub async fn sign_in(
    program: &Path,
    domains: &[&str],
) -> Result<Vec<BrowserCookie>, LoginWindowError> {
    let start_error = |source| LoginWindowError::Start {
        program: program.display().to_string(),
        source,
    };
    // Both ends are close-on-exec; the child gets the writing end as fd 3.
    let (reader, writer) = std::io::pipe().map_err(start_error)?;
    let mut command = tokio::process::Command::new(program);
    for domain in domains {
        command.arg("--domain").arg(domain);
    }
    if let Ok(language) = LANGUAGE.lock()
        && !language.is_empty()
    {
        command.arg("--language").arg(language.as_str());
    }
    command
        .env(CHANNEL_VAR, CHANNEL_FD.to_string())
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let fd = writer.as_raw_fd();
    // SAFETY: the closure runs in the child between fork and exec and calls
    // only `dup2` and `fcntl`, which are async-signal-safe.
    unsafe {
        command.pre_exec(move || {
            if fd == CHANNEL_FD {
                // `dup2` onto itself would keep the close-on-exec flag.
                if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            } else if libc::dup2(fd, CHANNEL_FD) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(start_error)?;
    // Only the child holds the writing end now: its exit ends the message.
    drop(writer);

    let read = tokio::task::spawn_blocking(move || {
        let mut message = Vec::new();
        reader
            .take(MAX_MESSAGE as u64 + 1)
            .read_to_end(&mut message)
            .map(|_| message)
    });
    let (message, status) = tokio::join!(read, child.wait());
    let message = message.map_err(std::io::Error::other)??;
    let status = status?;
    if message.len() > MAX_MESSAGE {
        return Err(LoginWindowError::TooLong);
    }
    if message.is_empty() {
        return Err(if status.code() == Some(EXIT_CANCELLED) {
            LoginWindowError::Cancelled
        } else {
            LoginWindowError::Failed(status)
        });
    }
    let message: Message = serde_json::from_slice(&message)?;
    Ok(message
        .cookies
        .into_iter()
        .filter(|cookie| host_in_domains(&cookie.host, domains))
        .collect())
}

/// The window's end of the channel.
#[derive(Debug)]
pub struct Channel(std::fs::File);

impl Channel {
    /// Takes the channel that orbis handed this process, or `None` when it
    /// was started without one. The descriptor is made close-on-exec, so the
    /// processes this one starts (the web engine's) do not get it. Call it
    /// first thing in `main`.
    pub fn from_env() -> Option<Self> {
        let fd: RawFd = std::env::var(CHANNEL_VAR).ok()?.parse().ok()?;
        if fd < CHANNEL_FD {
            return None;
        }
        // SAFETY: `fcntl` on a number only inspects and flags the
        // descriptor; a closed one fails with EBADF.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return None;
        }
        // SAFETY: the descriptor is open, and orbis passed it for this
        // process alone to own.
        Some(Self::from_fd(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    fn from_fd(fd: OwnedFd) -> Self {
        Self(std::fs::File::from(fd))
    }

    /// Sends the cookies and closes the channel.
    pub fn send(mut self, cookies: Vec<BrowserCookie>) -> std::io::Result<()> {
        let message = serde_json::to_vec(&Message { cookies }).map_err(std::io::Error::other)?;
        self.0.write_all(&message)?;
        self.0.flush()
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    const DOMAINS: [&str; 2] = ["youtube.com", "google.com"];

    fn cookie(name: &str, host: &str) -> BrowserCookie {
        BrowserCookie {
            name: name.into(),
            value: format!("value of {name}"),
            host: host.into(),
            path: "/".into(),
            expires: Some(1_900_000_000),
            secure: true,
            http_only: true,
        }
    }

    /// A stand-in for `orbis-login`: a shell script.
    fn window(dir: &Path, body: &str) -> std::path::PathBuf {
        let path = dir.join("orbis-login");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    /// [`sign_in`] of a stand-in, tried again while it is busy: a child of
    /// another test, forked while the script was being written, holds it
    /// open until it starts its own program, and running the script fails
    /// with ETXTBSY meanwhile. The real window is never written by orbis.
    async fn sign_in_test(program: &Path) -> Result<Vec<BrowserCookie>, LoginWindowError> {
        for _ in 0..100 {
            match sign_in(program, &DOMAINS).await {
                Err(LoginWindowError::Start { source, .. })
                    if source.kind() == std::io::ErrorKind::ExecutableFileBusy =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                other => return other,
            }
        }
        sign_in(program, &DOMAINS).await
    }

    fn message(cookies: Vec<BrowserCookie>) -> String {
        serde_json::to_string(&Message { cookies }).unwrap()
    }

    #[tokio::test]
    async fn cookies_come_through_fd_3_and_other_domains_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let sent = message(vec![
            cookie("SAPISID", ".youtube.com"),
            cookie("SID", ".google.com"),
            cookie("tracker", ".example.com"),
        ]);
        // The script checks what it was given: the variable and the domains.
        let program = window(
            dir.path(),
            &format!(
                "[ \"${CHANNEL_VAR}\" = 3 ] || exit 7\n\
                 [ \"$*\" = '--domain youtube.com --domain google.com' ] || exit 8\n\
                 printf '%s' '{sent}' >&3"
            ),
        );
        let cookies = sign_in_test(&program).await.unwrap();
        let names: Vec<_> = cookies.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["SAPISID", "SID"]);
        assert_eq!(cookies[0], cookie("SAPISID", ".youtube.com"));
    }

    #[tokio::test]
    async fn closing_the_window_is_a_cancel_and_a_crash_is_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let cancelled = window(dir.path(), &format!("exit {EXIT_CANCELLED}"));
        let result = sign_in_test(&cancelled).await;
        assert!(
            matches!(result, Err(LoginWindowError::Cancelled)),
            "{result:?}"
        );
        let failed = window(dir.path(), "exit 2");
        let result = sign_in_test(&failed).await;
        assert!(
            matches!(&result, Err(LoginWindowError::Failed(status)) if status.code() == Some(2)),
            "{result:?}"
        );
        let missing = dir.path().join("no-such-window");
        assert!(matches!(
            sign_in_test(&missing).await,
            Err(LoginWindowError::Start { .. })
        ));
    }

    #[tokio::test]
    async fn long_and_malformed_messages_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let long = window(dir.path(), "head -c 1100000 /dev/zero >&3");
        assert!(matches!(
            sign_in_test(&long).await,
            Err(LoginWindowError::TooLong)
        ));
        let malformed = window(dir.path(), "printf 'not json' >&3");
        assert!(matches!(
            sign_in_test(&malformed).await,
            Err(LoginWindowError::Malformed(_))
        ));
    }

    #[tokio::test]
    async fn dropping_the_future_closes_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let program = window(
            dir.path(),
            &format!("echo $$ > '{}'\nexec sleep 30", pid_file.display()),
        );
        let waited = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            sign_in_test(&program),
        )
        .await;
        assert!(waited.is_err(), "the window should still be open");
        let pid: i32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // The child is killed on drop and reaped in the background: gone or
        // a zombie both mean the window is closed.
        let mut gone = false;
        for _ in 0..50 {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
            let state = stat.rsplit_once(") ").map(|(_, rest)| &rest[..1]);
            if matches!(state, None | Some("Z")) {
                gone = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(gone, "the window process should be gone");
    }

    #[test]
    fn the_channel_sends_one_message() {
        let (mut reader, writer) = std::io::pipe().unwrap();
        let channel = Channel::from_fd(OwnedFd::from(writer));
        channel.send(vec![cookie("SID", ".google.com")]).unwrap();
        let mut text = String::new();
        reader.read_to_string(&mut text).unwrap();
        assert_eq!(text, message(vec![cookie("SID", ".google.com")]));
    }
}
