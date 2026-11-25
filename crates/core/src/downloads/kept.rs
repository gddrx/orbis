//! The downloads that have not ended, kept in SQLite so that they come back
//! after a restart: a row per track, in the order it was queued, the track
//! as JSON. A row goes when its download ends — saved, already there,
//! failed or cancelled. `PRAGMA user_version` holds the schema version.

use std::path::Path;

use rusqlite::{Connection, params};

use super::Wanted;

const SCHEMA_VERSION: i32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum KeptError {
    #[error("database: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("stored track: {0}")]
    Json(#[from] serde_json::Error),
    #[error("the database has schema version {0}, newer than this build")]
    Newer(i32),
}

/// The kept queue; a row's key is its place in it.
#[derive(Debug)]
pub struct Kept {
    connection: Connection,
}

impl Kept {
    /// Opens (or creates) the database at `path`.
    pub fn open(path: &Path) -> Result<Self, KeptError> {
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        // A queue lost to a power cut is no harm; a sync per track of a long
        // list is.
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        let version: i32 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(KeptError::Newer(version));
        }
        if version < 1 {
            connection.execute_batch(
                "CREATE TABLE IF NOT EXISTS pending (
                     id INTEGER PRIMARY KEY,
                     track TEXT NOT NULL
                 );
                 PRAGMA user_version = 1;",
            )?;
        }
        Ok(Self { connection })
    }

    /// Keeps `tracks`, in their order, in one transaction; their rows.
    pub fn add(&mut self, tracks: &[Wanted]) -> Result<Vec<i64>, KeptError> {
        let transaction = self.connection.transaction()?;
        let mut rows = Vec::with_capacity(tracks.len());
        for track in tracks {
            transaction.execute(
                "INSERT INTO pending (track) VALUES (?1)",
                params![serde_json::to_string(track)?],
            )?;
            rows.push(transaction.last_insert_rowid());
        }
        transaction.commit()?;
        Ok(rows)
    }

    /// Forgets the downloads of `rows`, in one transaction: they have ended.
    pub fn remove(&mut self, rows: &[i64]) -> Result<(), KeptError> {
        let transaction = self.connection.transaction()?;
        for row in rows {
            transaction.execute("DELETE FROM pending WHERE id = ?1", [row])?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// The kept tracks with their rows, in the order they were queued. A
    /// row that no longer reads is dropped.
    pub fn load(&mut self) -> Result<Vec<(i64, Wanted)>, KeptError> {
        let rows: Vec<(i64, String)> = self
            .connection
            .prepare("SELECT id, track FROM pending ORDER BY id")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        let mut kept = Vec::with_capacity(rows.len());
        let mut unread = Vec::new();
        for (row, json) in rows {
            match serde_json::from_str(&json) {
                Ok(track) => kept.push((row, track)),
                Err(_) => unread.push(row),
            }
        }
        self.remove(&unread)?;
        Ok(kept)
    }
}

#[cfg(test)]
mod tests {
    use download::{AlbumMeta, TrackMeta};

    use super::*;

    fn wanted(video_id: &str) -> Wanted {
        Wanted {
            video_id: video_id.into(),
            title: format!("Song {video_id}"),
            artists: vec!["Band".into()],
            album_id: Some("MPREb_x".into()),
            meta: Some(TrackMeta {
                video_id: video_id.into(),
                title: format!("Song {video_id}"),
                artists: vec!["Band".into()],
                album: Some(AlbumMeta {
                    title: "Record".into(),
                    artists: vec!["Band".into()],
                    year: Some(2024),
                    track_number: Some(1),
                    track_count: Some(3),
                }),
            }),
        }
    }

    #[test]
    fn what_has_not_ended_comes_back_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("downloads.db");
        let queued = [wanted("a"), wanted("b"), Wanted::video("c")];
        {
            let mut kept = Kept::open(&path).unwrap();
            assert!(kept.load().unwrap().is_empty(), "first start");
            let rows = kept.add(&queued).unwrap();
            // The first one is done.
            kept.remove(&rows[..1]).unwrap();
        }

        let mut kept = Kept::open(&path).unwrap();
        let back = kept.load().unwrap();
        let ids: Vec<&str> = back.iter().map(|(_, w)| w.video_id.as_str()).collect();
        assert_eq!(ids, ["b", "c"]);
        // Everything known of a track comes back with it.
        assert_eq!(back[0].1.meta, queued[1].meta);
        assert_eq!(back[0].1.album_id.as_deref(), Some("MPREb_x"));
        assert!(back[1].1.title.is_empty() && back[1].1.meta.is_none());

        // New tracks queue after them.
        let rows = kept.add(&[wanted("d")]).unwrap();
        assert!(rows[0] > back[1].0);
    }

    #[test]
    fn a_row_that_does_not_read_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("downloads.db");
        let mut kept = Kept::open(&path).unwrap();
        kept.add(&[wanted("a")]).unwrap();
        kept.connection
            .execute("INSERT INTO pending (track) VALUES ('not json')", [])
            .unwrap();
        assert_eq!(kept.load().unwrap().len(), 1);
        let left: i64 = kept
            .connection
            .query_row("SELECT count(*) FROM pending", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 1);
    }

    #[test]
    fn a_newer_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("downloads.db");
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 9)
            .unwrap();
        assert!(matches!(Kept::open(&path), Err(KeptError::Newer(9))));
    }
}
