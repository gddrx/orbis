//! Cookies of Firefox: `cookies.sqlite` of a profile, stored in clear.
//!
//! Profiles are listed in `profiles.ini` under `~/.config/mozilla/firefox`
//! (newer Firefox) or `~/.mozilla/firefox`. The profile Firefox starts with is
//! the `Default` of an `[Install…]` section, or else the profile marked
//! `Default=1`. Only cookies of the default container are taken.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use super::{BrowserCookie, ImportError, Imported, config_dir, host_in_domains, private_copy};

/// Imports the cookies of `domains` from the profile in `profile_dir`, or from
/// the profile Firefox starts with when `None`.
pub fn import(profile_dir: Option<&Path>, domains: &[&str]) -> Result<Imported, ImportError> {
    let profile_dir = match profile_dir {
        Some(dir) => dir.to_path_buf(),
        None => default_profile().ok_or_else(|| ImportError::ProfileNotFound {
            browser: "Firefox".into(),
            detail: "no profiles.ini with a default profile".into(),
        })?,
    };
    let db = profile_dir.join("cookies.sqlite");
    if !db.is_file() {
        return Err(ImportError::ProfileNotFound {
            browser: "Firefox".into(),
            detail: format!("no cookies.sqlite in {}", profile_dir.display()),
        });
    }
    let (_dir, copy) = private_copy(&db)?;
    Ok(Imported {
        cookies: read_cookies(&copy, domains)?,
        undecryptable: 0,
    })
}

/// Whether the profile Firefox starts with has a cookie database: without
/// one there is no sign-in to take.
pub fn has_cookies() -> bool {
    default_profile().is_some_and(|dir| dir.join("cookies.sqlite").is_file())
}

/// The directory of the profile Firefox starts with.
pub fn default_profile() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    default_profile_in([
        config_dir().map(|c| c.join("mozilla").join("firefox")),
        home.map(|h| h.join(".mozilla").join("firefox")),
    ])
}

/// The profile Firefox starts with in the first of `roots` that has one.
fn default_profile_in(roots: [Option<PathBuf>; 2]) -> Option<PathBuf> {
    roots.into_iter().flatten().find_map(|root| {
        let ini = std::fs::read_to_string(root.join("profiles.ini")).ok()?;
        let path = default_profile_path(&ini)?;
        let dir = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        dir.is_dir().then_some(dir)
    })
}

/// The default profile's path from the text of `profiles.ini`.
fn default_profile_path(ini: &str) -> Option<PathBuf> {
    let mut install_default = None;
    let mut marked_default = None;
    let mut section = String::new();
    let mut path = None;
    let mut is_default = false;
    let mut finish = |section: &str, path: &mut Option<String>, is_default: &mut bool| {
        if section.starts_with("Profile") && *is_default && marked_default.is_none() {
            marked_default = path.clone();
        }
        *path = None;
        *is_default = false;
    };
    for line in ini.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            finish(&section, &mut path, &mut is_default);
            section = name.to_owned();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match (section.as_str(), key.trim()) {
            (s, "Default") if s.starts_with("Install") && install_default.is_none() => {
                install_default = Some(value.trim().to_owned());
            }
            (s, "Path") if s.starts_with("Profile") => path = Some(value.trim().to_owned()),
            (s, "Default") if s.starts_with("Profile") => is_default = value.trim() == "1",
            _ => {}
        }
    }
    finish(&section, &mut path, &mut is_default);
    install_default.or(marked_default).map(PathBuf::from)
}

fn read_cookies(db: &Path, domains: &[&str]) -> Result<Vec<BrowserCookie>, ImportError> {
    let connection = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut statement = connection.prepare(
        "SELECT host, name, value, path, expiry, isSecure, isHttpOnly FROM moz_cookies \
         WHERE originAttributes = ''",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(BrowserCookie {
            host: row.get(0)?,
            name: row.get(1)?,
            value: row.get(2)?,
            path: row.get(3)?,
            expires: firefox_expiry_to_unix(row.get(4)?),
            secure: row.get(5)?,
            http_only: row.get(6)?,
        })
    })?;
    let mut cookies = Vec::new();
    for cookie in rows {
        let cookie = cookie?;
        if host_in_domains(&cookie.host, domains) {
            cookies.push(cookie);
        }
    }
    Ok(cookies)
}

/// Firefox stores the expiry in seconds, newer versions in milliseconds.
fn firefox_expiry_to_unix(expiry: i64) -> Option<i64> {
    match expiry {
        e if e <= 0 => None,
        e if e > 100_000_000_000 => Some(e / 1000),
        e => Some(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_profile_is_found_in_the_first_root_that_has_one() {
        let (newer, older) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let roots = || {
            [
                Some(newer.path().to_path_buf()),
                Some(older.path().to_path_buf()),
            ]
        };
        assert_eq!(default_profile_in(roots()), None, "no Firefox");

        std::fs::create_dir(older.path().join("o.default")).unwrap();
        std::fs::write(
            older.path().join("profiles.ini"),
            "[Profile0]\nIsRelative=1\nPath=o.default\nDefault=1\n",
        )
        .unwrap();
        assert_eq!(
            default_profile_in(roots()),
            Some(older.path().join("o.default"))
        );
        // A profile listed but missing on disk does not count.
        std::fs::write(
            newer.path().join("profiles.ini"),
            "[Profile0]\nIsRelative=1\nPath=gone\nDefault=1\n",
        )
        .unwrap();
        assert_eq!(
            default_profile_in(roots()),
            Some(older.path().join("o.default"))
        );
    }

    #[test]
    fn install_default_wins() {
        let ini = "[Profile1]\nName=default\nIsRelative=1\nPath=a.default\nDefault=1\n\n\
                   [Profile0]\nName=default-release\nIsRelative=1\nPath=b.default-release\n\n\
                   [General]\nVersion=2\n\n[Install11457493C5A56847]\nDefault=b.default-release\nLocked=1\n";
        assert_eq!(
            default_profile_path(ini),
            Some(PathBuf::from("b.default-release"))
        );
    }

    #[test]
    fn marked_default_without_install_section() {
        let ini = "[Profile0]\nPath=x.other\n\n[Profile1]\nPath=y.main\nDefault=1\n";
        assert_eq!(default_profile_path(ini), Some(PathBuf::from("y.main")));
        assert_eq!(default_profile_path("[General]\nVersion=2\n"), None);
    }

    #[test]
    fn expiry_units() {
        assert_eq!(firefox_expiry_to_unix(0), None);
        assert_eq!(firefox_expiry_to_unix(1_800_000_000), Some(1_800_000_000));
        assert_eq!(
            firefox_expiry_to_unix(1_800_000_000_000),
            Some(1_800_000_000)
        );
    }

    #[test]
    fn reads_default_container_cookies_of_the_domains() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("cookies.sqlite");
        let connection = Connection::open(&db).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE moz_cookies (host TEXT, name TEXT, value TEXT, path TEXT,
                   expiry INTEGER, isSecure INTEGER, isHttpOnly INTEGER, originAttributes TEXT);
                 INSERT INTO moz_cookies VALUES ('.youtube.com', 'SAPISID', 's', '/', 1800000000000, 1, 0, '');
                 INSERT INTO moz_cookies VALUES ('.youtube.com', 'SAPISID', 'container', '/', 1800000000, 1, 0, '^userContextId=2');
                 INSERT INTO moz_cookies VALUES ('.example.com', 'X', 'x', '/', 1800000000, 1, 0, '');",
            )
            .unwrap();
        drop(connection);
        let cookies = read_cookies(&db, &["youtube.com", "google.com"]).unwrap();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].value, "s");
        assert_eq!(cookies[0].expires, Some(1_800_000_000));
    }
}
