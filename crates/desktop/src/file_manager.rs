//! The file manager (`org.freedesktop.FileManager1`: Dolphin, Nautilus and
//! others): shows a file selected in its folder.

use zbus::Connection;

const SERVICE: &str = "org.freedesktop.FileManager1";
const PATH: &str = "/org/freedesktop/FileManager1";

/// Asks the file manager on `bus` to open the folders of `uris` (`file://`
/// URLs) with these files selected. `Err` when no file manager answers —
/// none is installed, or the bus is a private one: the caller then opens
/// the folder itself.
pub async fn show_items(bus: &Connection, uris: &[String]) -> Result<(), zbus::Error> {
    // The second argument is a startup ID for the new window's focus; an
    // empty one leaves the focus to the compositor.
    bus.call_method(Some(SERVICE), PATH, Some(SERVICE), "ShowItems", &(uris, ""))
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use zbus::connection;

    use super::*;
    use crate::test_bus::Bus;

    /// A file manager that records what it was asked to show.
    #[derive(Clone, Default)]
    struct FakeManager(Arc<Mutex<Vec<String>>>);

    #[zbus::interface(name = "org.freedesktop.FileManager1")]
    impl FakeManager {
        fn show_items(&self, uris: Vec<String>, _startup_id: String) {
            self.0.lock().unwrap().extend(uris);
        }
    }

    #[tokio::test]
    async fn the_file_manager_is_asked_to_show_the_file() {
        let Some(bus) = Bus::start() else {
            eprintln!("no dbus-daemon: skipped");
            return;
        };
        let manager = FakeManager::default();
        let _service = connection::Builder::address(bus.address.as_str())
            .serve_at(PATH, manager.clone())
            .name(SERVICE)
            .build()
            .await
            .unwrap();
        let file = "file:///music/Artist/Album/01%20Song.opus".to_owned();
        show_items(&bus.connect().await, std::slice::from_ref(&file))
            .await
            .unwrap();
        assert_eq!(*manager.0.lock().unwrap(), vec![file]);
    }

    #[tokio::test]
    async fn without_a_file_manager_the_caller_is_told() {
        let Some(bus) = Bus::start() else {
            eprintln!("no dbus-daemon: skipped");
            return;
        };
        let file = "file:///music/song.opus".to_owned();
        assert!(show_items(&bus.connect().await, &[file]).await.is_err());
    }
}
