//! Links kept on disk between runs, in SQLite: a row per track and format
//! (the yt-dlp format string of a quality), with the link's fields. Only
//! links with an expiry are kept; those past it are dropped when the file
//! is opened and with every new link, so the file holds a few hours of
//! links at most. The links carry signatures, so the file is the user's
//! alone (mode 0600).

use std::path::Path;

use rusqlite::{Connection, params};
use serde_json::Value;

use crate::Stream;

const SCHEMA_VERSION: i32 = 1;

#[derive(Debug)]
pub(crate) struct Disk {
    connection: Connection,
    /// The format the links of this cache are in.
    format: String,
}

impl Disk {
    /// Opens (or creates) the file at `path` for links in `format`.
    pub(crate) fn open(path: &Path, format: &str) -> rusqlite::Result<Self> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let connection = Connection::open(path)?;
        restrict(path);
        connection.busy_timeout(std::time::Duration::from_secs(1))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        let version: i32 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != SCHEMA_VERSION {
            // A cache: another layout is dropped, not migrated.
            connection.execute_batch(
                "DROP TABLE IF EXISTS links;
                 CREATE TABLE links (
                     video_id TEXT NOT NULL,
                     format TEXT NOT NULL,
                     expires_at INTEGER NOT NULL,
                     url TEXT NOT NULL,
                     format_id TEXT NOT NULL,
                     codec TEXT NOT NULL,
                     container TEXT NOT NULL,
                     bitrate_kbps REAL,
                     sample_rate INTEGER,
                     duration REAL,
                     headers TEXT NOT NULL,
                     PRIMARY KEY (video_id, format)
                 );",
            )?;
            connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        restrict(path);
        Ok(Self {
            connection,
            format: format.to_owned(),
        })
    }

    /// The links of this format that still work `until` (Unix seconds);
    /// the others, of any format, are dropped.
    pub(crate) fn load(&self, until: i64) -> rusqlite::Result<Vec<Stream>> {
        self.connection
            .execute("DELETE FROM links WHERE expires_at <= ?1", [until])?;
        let mut statement = self.connection.prepare(
            "SELECT video_id, url, format_id, codec, container, bitrate_kbps, sample_rate,
                    duration, expires_at, headers
             FROM links WHERE format = ?1",
        )?;
        statement
            .query_map([&self.format], |row| {
                let headers: String = row.get(9)?;
                Ok(Stream {
                    video_id: row.get(0)?,
                    url: row.get(1)?,
                    format_id: row.get(2)?,
                    codec: row.get(3)?,
                    container: row.get(4)?,
                    bitrate_kbps: row.get(5)?,
                    sample_rate: row.get(6)?,
                    duration: row.get(7)?,
                    expires_at: row.get(8)?,
                    headers: headers_from_json(&headers),
                })
            })?
            .collect()
    }

    /// Keeps `stream`, in place of the track's earlier link; links expired
    /// by `now` (Unix seconds) go.
    pub(crate) fn put(&self, stream: &Stream, now: i64) -> rusqlite::Result<()> {
        let Some(expires_at) = stream.expires_at else {
            return Ok(());
        };
        self.connection
            .execute("DELETE FROM links WHERE expires_at <= ?1", [now])?;
        let headers: Vec<Value> = stream
            .headers
            .iter()
            .map(|(name, value)| Value::from(vec![name.as_str(), value.as_str()]))
            .collect();
        self.connection.execute(
            "INSERT OR REPLACE INTO links (video_id, format, expires_at, url, format_id, codec,
                 container, bitrate_kbps, sample_rate, duration, headers)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                stream.video_id,
                self.format,
                expires_at,
                stream.url,
                stream.format_id,
                stream.codec,
                stream.container,
                stream.bitrate_kbps,
                stream.sample_rate,
                stream.duration,
                Value::from(headers).to_string(),
            ],
        )?;
        Ok(())
    }

    /// Forgets the track's link.
    pub(crate) fn remove(&self, video_id: &str) -> rusqlite::Result<()> {
        self.connection.execute(
            "DELETE FROM links WHERE video_id = ?1 AND format = ?2",
            params![video_id, self.format],
        )?;
        Ok(())
    }

    /// How many links are kept in this format (tests).
    #[cfg(test)]
    pub(crate) fn count(&self) -> i64 {
        self.connection
            .query_row(
                "SELECT count(*) FROM links WHERE format = ?1",
                [&self.format],
                |r| r.get(0),
            )
            .unwrap_or(0)
    }
}

fn headers_from_json(json: &str) -> Vec<(String, String)> {
    let Ok(Value::Array(pairs)) = serde_json::from_str(json) else {
        return Vec::new();
    };
    pairs
        .iter()
        .filter_map(|pair| {
            let name = pair.get(0)?.as_str()?;
            let value = pair.get(1)?.as_str()?;
            Some((name.to_owned(), value.to_owned()))
        })
        .collect()
}

/// Makes the file readable by its owner alone, with SQLite's side files.
fn restrict(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::Permissions::from_mode(0o600);
    let _ = std::fs::set_permissions(path, mode.clone());
    for suffix in ["-wal", "-shm"] {
        let mut side = path.as_os_str().to_owned();
        side.push(suffix);
        let _ = std::fs::set_permissions(Path::new(&side), mode.clone());
    }
}

/// The address a link was made for (its `ip` parameter); YouTube refuses
/// it from another one.
pub(crate) fn link_ip(url: &str) -> Option<&str> {
    let (_, query) = url.split_once('?')?;
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix("ip="))
        .filter(|ip| !ip.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_address_is_read_from_the_link() {
        assert_eq!(
            link_ip("https://r1.googlevideo.com/videoplayback?expire=1&ip=203.0.113.5&id=x"),
            Some("203.0.113.5")
        );
        assert_eq!(
            link_ip("https://r1.googlevideo.com/videoplayback?sip=1&ip=2001%3Adb8%3A%3A1"),
            Some("2001%3Adb8%3A%3A1")
        );
        assert_eq!(link_ip("https://example.com/a?vip=1"), None);
        assert_eq!(link_ip("https://example.com/a"), None);
    }

    fn stream(video_id: &str, expires_at: i64) -> Stream {
        Stream {
            video_id: video_id.into(),
            url: format!(
                "https://r1.googlevideo.com/videoplayback?expire={expires_at}&ip=203.0.113.5"
            ),
            format_id: "251".into(),
            codec: "opus".into(),
            container: "webm".into(),
            bitrate_kbps: None,
            sample_rate: None,
            duration: None,
            expires_at: Some(expires_at),
            headers: Vec::new(),
        }
    }

    #[test]
    fn expired_links_go_with_each_new_one() {
        let dir = tempfile::tempdir().unwrap();
        let disk = Disk::open(&dir.path().join("streams.db"), "251").unwrap();
        disk.put(&stream("old", 100), 50).unwrap();
        disk.put(&stream("new", 1000), 50).unwrap();
        assert_eq!(disk.count(), 2);
        disk.put(&stream("newer", 1000), 200).unwrap();
        assert_eq!(disk.count(), 2, "the one that expired at 100 is gone");
        // A link without an expiry is not kept.
        let mut forever = stream("forever", 0);
        forever.expires_at = None;
        disk.put(&forever, 200).unwrap();
        assert_eq!(disk.count(), 2);
    }

    #[test]
    fn the_file_is_the_owners_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("streams.db");
        Disk::open(&path, "251").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
