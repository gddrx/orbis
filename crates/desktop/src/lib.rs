//! Desktop integration behind traits, so other systems can be added later:
//! MPRIS, the tray icon, secret storage, browser cookies, the sign-in
//! window, the file manager, the user's folders and window blur.
//! Linux (KDE Plasma) first. Does not depend on Qt.

pub mod browser_cookies;
pub mod file_manager;
pub mod login_window;
pub mod mpris;
pub mod secrets;
#[cfg(test)]
mod test_bus;
pub mod tray;
pub mod user_dirs;

pub use mpris::{Mpris, MprisError, MprisEvent, MprisState, MprisTrack};
pub use secrets::{MemoryStore, SecretError, SecretServiceStore, SecretStore};
pub use tray::{MenuEntry, Pixmaps, Tray, TrayError, TrayEvent, TrayState};
pub use user_dirs::{cache_dir, music_dir};
pub use zbus::Connection;

/// Connects to the bus the desktop services share — the tray, MPRIS and the
/// single instance: the one at `address`, or the session bus.
pub async fn connect(address: Option<&str>) -> Result<Connection, zbus::Error> {
    match address {
        Some(address) => zbus::connection::Builder::address(address).build().await,
        None => zbus::connection::Builder::session().build().await,
    }
}
