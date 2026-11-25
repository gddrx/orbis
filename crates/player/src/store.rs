//! Saves the tabs and their queues in SQLite and restores them at start.
//!
//! One row per tab, in display order; the queue goes into the row as JSON.
//! `PRAGMA user_version` holds the schema version for later migrations.

use std::path::Path;

use rusqlite::{Connection, params};

use crate::tabs::{Tab, TabId, Tabs};

const SCHEMA_VERSION: i32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("stored state: {0}")]
    Json(#[from] serde_json::Error),
    #[error("the database has schema version {0}, newer than this build")]
    Newer(i32),
}

#[derive(Debug)]
pub struct Store {
    connection: Connection,
}

impl Store {
    /// Opens (or creates) the database at `path`.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        Self::init(Connection::open(path)?)
    }

    /// A database in memory, for tests.
    pub fn in_memory() -> Result<Self, StoreError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(connection: Connection) -> Result<Self, StoreError> {
        connection.pragma_update(None, "journal_mode", "WAL")?;
        let version: i32 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(StoreError::Newer(version));
        }
        if version < 1 {
            connection.execute_batch(
                "CREATE TABLE tabs (
                     id INTEGER PRIMARY KEY,
                     ordinal INTEGER NOT NULL,
                     tab TEXT NOT NULL
                 );
                 CREATE TABLE meta (key TEXT PRIMARY KEY, value INTEGER);
                 PRAGMA user_version = 1;",
            )?;
        }
        Ok(Self { connection })
    }

    /// Replaces the stored state with `tabs`.
    pub fn save(&mut self, tabs: &Tabs) -> Result<(), StoreError> {
        let transaction = self.connection.transaction()?;
        transaction.execute("DELETE FROM tabs", [])?;
        for (ordinal, tab) in tabs.tabs().iter().enumerate() {
            transaction.execute(
                "INSERT INTO tabs (id, ordinal, tab) VALUES (?1, ?2, ?3)",
                params![tab.id as i64, ordinal as i64, serde_json::to_string(tab)?],
            )?;
        }
        for (key, value) in [("shown", tabs.shown), ("playing", tabs.playing())] {
            transaction.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value.map(|v| v as i64)],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// The stored tabs, or `None` on the first start.
    pub fn load(&self) -> Result<Option<Tabs>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT tab FROM tabs ORDER BY ordinal")?;
        let tabs: Vec<Tab> = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .map(|json| Ok(serde_json::from_str(&json?)?))
            .collect::<Result<_, StoreError>>()?;
        if tabs.is_empty() {
            return Ok(None);
        }
        let meta = |key: &str| -> Result<Option<TabId>, StoreError> {
            let value: Option<Option<i64>> = self
                .connection
                .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    e => Err(e),
                })?;
            Ok(value.flatten().map(|v| v as TabId))
        };
        Ok(Some(Tabs::from_parts(
            tabs,
            meta("shown")?,
            meta("playing")?,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::Repeat;
    use crate::queue::tests::items;
    use crate::tabs::TabSource;

    #[test]
    fn tabs_and_queues_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");

        let mut tabs = Tabs::with_defaults(Some("RDTMAK5uy_d"), None);
        let discover = tabs.tabs()[1].id;
        let search = tabs.open(
            "rock",
            TabSource::Search {
                query: "rock".into(),
            },
            false,
        );
        tabs.play_list(
            discover,
            items(&["d1", "d2", "d3"]),
            1,
            Some("more".into()),
            0.0,
        );
        {
            let queue = tabs.playing_queue_mut().unwrap();
            queue.repeat = Repeat::All;
            queue.set_shuffle(true);
        }
        tabs.play_list(search, items(&["s1"]), 0, None, 95.5);
        tabs.move_tab(search, 0);
        tabs.shown = Some(search);
        {
            let mut store = Store::open(&path).unwrap();
            assert_eq!(store.load().unwrap(), None, "first start");
            store.save(&tabs).unwrap();
        }

        let restored = Store::open(&path).unwrap().load().unwrap().unwrap();
        assert_eq!(restored, tabs);
        let queue = &restored.get(discover).unwrap().queue;
        assert_eq!(queue.current().unwrap().video_id, "d2");
        assert_eq!(
            (queue.position, queue.repeat, queue.shuffle()),
            (95.5, Repeat::All, true)
        );
        assert_eq!(queue.continuation.as_deref(), Some("more"));

        // New tabs after a restart get fresh IDs.
        let mut restored = restored;
        let new = restored.open(
            "jazz",
            TabSource::Search {
                query: "jazz".into(),
            },
            false,
        );
        assert!(restored.tabs().iter().filter(|t| t.id == new).count() == 1 && new > search);
    }

    #[test]
    fn a_newer_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 9)
            .unwrap();
        assert!(matches!(Store::open(&path), Err(StoreError::Newer(9))));
    }
}
