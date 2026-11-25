//! Imports the cookies of a signed-in browser, so the player starts with the
//! same YouTube Music session.
//!
//! The browser's cookie database is copied into a private temporary directory
//! (the browser keeps it open) and only cookies of the requested domains are
//! read. Values never reach `Debug` output.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub mod chromium;
pub mod firefox;
mod kwallet;

pub use chromium::ChromiumBrowser;

/// A cookie as the browser stored it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserCookie {
    pub name: String,
    pub value: String,
    /// The host as the browser keeps it: `.youtube.com` for a domain cookie,
    /// `music.youtube.com` for a host-only one.
    pub host: String,
    pub path: String,
    /// Expiry as Unix seconds; `None` for a session cookie.
    pub expires: Option<i64>,
    pub secure: bool,
    pub http_only: bool,
}

impl fmt::Debug for BrowserCookie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BrowserCookie")
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .field("host", &self.host)
            .field("path", &self.path)
            .field("expires", &self.expires)
            .field("secure", &self.secure)
            .field("http_only", &self.http_only)
            .finish()
    }
}

/// What an import found.
#[derive(Debug, Default)]
pub struct Imported {
    pub cookies: Vec<BrowserCookie>,
    /// Cookies of the requested domains that could not be decrypted.
    pub undecryptable: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("no {browser} profile found: {detail}")]
    ProfileNotFound { browser: String, detail: String },
    #[error("cannot read the cookie database: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("cannot copy the cookie database: {0}")]
    Io(#[from] std::io::Error),
    #[error("the browser's cookie key is not available: {0}")]
    Key(String),
}

/// `true` when `host` (with or without a leading dot) is one of `domains` or
/// a subdomain of one.
pub fn host_in_domains(host: &str, domains: &[&str]) -> bool {
    let host = host.trim_start_matches('.').to_ascii_lowercase();
    domains.iter().any(|domain| {
        host == *domain
            || (host.len() > domain.len()
                && host.ends_with(domain)
                && host.as_bytes()[host.len() - domain.len() - 1] == b'.')
    })
}

/// `$XDG_CONFIG_HOME`, or `~/.config`.
fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

/// Copies `db` (and its journal or WAL, when present) into a fresh private
/// directory under `$XDG_RUNTIME_DIR` and returns the directory with the copy.
fn private_copy(db: &Path) -> Result<(tempfile::TempDir, PathBuf), ImportError> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    // Only the user may enter the directory: it holds session cookies.
    let dir = tempfile::Builder::new()
        .prefix("orbis-cookies-")
        .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .tempdir_in(base)?;
    let name = db.file_name().unwrap_or_default();
    let copy = dir.path().join(name);
    std::fs::copy(db, &copy)?;
    for suffix in ["-journal", "-wal"] {
        let mut side = db.as_os_str().to_owned();
        side.push(suffix);
        let side = PathBuf::from(side);
        if side.exists() {
            let mut target = copy.as_os_str().to_owned();
            target.push(suffix);
            std::fs::copy(&side, PathBuf::from(target))?;
        }
    }
    Ok((dir, copy))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_match_on_dot_boundaries() {
        let domains = ["youtube.com", "google.com"];
        assert!(host_in_domains(".youtube.com", &domains));
        assert!(host_in_domains("music.youtube.com", &domains));
        assert!(host_in_domains("accounts.google.com", &domains));
        assert!(!host_in_domains("notyoutube.com", &domains));
        assert!(!host_in_domains(".google.com.evil.example", &domains));
    }

    #[test]
    fn debug_hides_the_value() {
        let cookie = BrowserCookie {
            name: "SID".into(),
            value: "secret".into(),
            host: ".youtube.com".into(),
            path: "/".into(),
            expires: None,
            secure: true,
            http_only: true,
        };
        let shown = format!("{cookie:?}");
        assert!(shown.contains("SID") && !shown.contains("secret"));
    }

    #[test]
    fn private_copy_takes_side_files() {
        let source = tempfile::tempdir().unwrap();
        let db = source.path().join("Cookies");
        std::fs::write(&db, b"db").unwrap();
        std::fs::write(source.path().join("Cookies-journal"), b"j").unwrap();
        let (dir, copy) = private_copy(&db).unwrap();
        assert_eq!(std::fs::read(&copy).unwrap(), b"db");
        assert_eq!(
            std::fs::read(dir.path().join("Cookies-journal")).unwrap(),
            b"j"
        );
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(dir.path()).unwrap().permissions(),
        );
        assert_eq!(mode & 0o077, 0, "the copy directory is private");
    }
}
