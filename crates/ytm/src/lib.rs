//! YouTube Music client over the site's internal API (InnerTube, client `WEB_REMIX`).
//!
//! Covers the session (cookies, `SAPISIDHASH`), reading the home page, mixes,
//! library, playlists and search, the `next` queue, ratings and playlist edits.
//! Does not depend on Qt.

pub mod account;
pub mod actions;
pub mod auth;
pub mod browse;
pub mod client;
pub mod cookies;
pub mod model;
pub mod netlog;
mod parse;
pub mod session;
pub mod suggest;
pub mod watch;

pub use account::Account;
pub use actions::LIKED_MUSIC;
pub use browse::{find_mixes, is_album_playlist};
pub use client::{Client, Error, unix_now};
pub use cookies::{CookieJar, StoredCookie};
pub use model::*;
pub use session::{ClientContext, Session};
pub use watch::radio_playlist_id;
