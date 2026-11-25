//! Starting playback over a signed-in session: at launch when a session is
//! saved, or later, once the user has signed in from the window.

use std::path::PathBuf;
use std::sync::Arc;

use orbis_core::{Playback, PlaybackEvent, SessionManager};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::i18n::trf;

/// What playback needs besides the session.
#[derive(Debug, Clone)]
pub struct Starter {
    /// Where the tabs database lives.
    pub data_dir: PathBuf,
    /// mpv's audio output; `None` for the default.
    pub audio: Option<String>,
}

impl Starter {
    /// Starts playback. The first start's personal mixes are not waited
    /// for: the window finds them with [`personal_mixes`] once it is up.
    /// Runs on tokio.
    pub async fn start(
        &self,
        session: Arc<SessionManager>,
    ) -> Result<(Arc<Playback>, UnboundedReceiver<PlaybackEvent>), String> {
        let dir = &self.data_dir;
        std::fs::create_dir_all(dir)
            .map_err(|e| trf("No data folder %1: %2", &[&dir.display(), &e]))?;
        let state = dir.join("state.db");

        Playback::start(session, &state, (None, None), self.audio.clone())
            .map_err(|e| trf("The player did not start: %1", &[&e]))
    }
}

/// The playlist IDs of Discover Mix and New Release Mix on the home page,
/// all its pages, asked for in English whatever the interface language;
/// `None` for a mix not found or a page not read.
pub async fn personal_mixes(session: &SessionManager) -> (Option<String>, Option<String>) {
    let found = session
        .call(|c| async move { c.personal_mixes().await })
        .await
        .unwrap_or_default();
    let id = |mix: ytm::Mix| {
        found
            .iter()
            .find(|(m, _)| *m == mix)
            .map(|(_, card)| card.playlist_id().to_owned())
    };
    (id(ytm::Mix::Discover), id(ytm::Mix::NewRelease))
}
