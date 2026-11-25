//! Play queues from the `next` endpoint (radios, watch playlists) and lyrics.
//!
//! A personal mix (`RDTMAK…`) asked through `next` without a start track comes
//! back empty, so a mix's queue is built from its playlist pages
//! ([`Client::playlist`] and [`Client::playlist_next`]); `next` serves radios.

use serde_json::{Map, Value, json};

use crate::client::{Client, Error};
use crate::model::{Lyrics, PlayerInfo, WatchQueue};
use crate::parse::pages;

/// Fields every `next` request of the web app carries.
fn watch_body(extra: &[(&str, Value)]) -> Map<String, Value> {
    let mut body: Map<String, Value> = [
        ("enablePersistentPlaylistPanel", json!(true)),
        ("isAudioOnly", json!(true)),
        ("tunerSettingValue", json!("AUTOMIX_SETTING_NORMAL")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    for (k, v) in extra {
        body.insert((*k).to_owned(), v.clone());
    }
    body
}

/// The radio playlist of a track.
pub fn radio_playlist_id(video_id: &str) -> String {
    format!("RDAMVM{video_id}")
}

impl Client {
    /// The queue that plays `video_id`, inside `playlist_id` when given.
    pub async fn watch(
        &self,
        video_id: &str,
        playlist_id: Option<&str>,
    ) -> Result<WatchQueue, Error> {
        let mut extra = vec![("videoId", json!(video_id))];
        if let Some(playlist_id) = playlist_id {
            extra.push(("playlistId", json!(playlist_id)));
        }
        let response = self.post("next", watch_body(&extra)).await?;
        Ok(pages::parse_watch(&response))
    }

    /// The radio of a track: the track first, then an endless stream of
    /// similar ones.
    pub async fn radio(&self, video_id: &str) -> Result<WatchQueue, Error> {
        let response = self
            .post(
                "next",
                watch_body(&[
                    ("videoId", json!(video_id)),
                    ("playlistId", json!(radio_playlist_id(video_id))),
                    // The web app's radio parameter.
                    ("params", json!("wAEB")),
                ]),
            )
            .await?;
        Ok(pages::parse_watch(&response))
    }

    /// More tracks of a queue.
    pub async fn watch_next(&self, continuation: &str) -> Result<WatchQueue, Error> {
        let response = self
            .post("next", watch_body(&[("continuation", json!(continuation))]))
            .await?;
        Ok(pages::parse_watch(&response))
    }

    /// Loudness, length and playability of a track (`player`).
    pub async fn player_info(&self, video_id: &str) -> Result<PlayerInfo, Error> {
        let mut body = Map::new();
        body.insert("videoId".into(), json!(video_id));
        let response = self.post("player", body).await?;
        Ok(parse_player_info(&response))
    }

    /// The lyrics of a track, or `None` when YouTube Music has none.
    pub async fn lyrics(&self, video_id: &str) -> Result<Option<Lyrics>, Error> {
        let queue = self.watch(video_id, None).await?;
        match queue.lyrics_id {
            Some(id) => self.lyrics_page(&id).await,
            None => Ok(None),
        }
    }

    /// The lyrics behind a queue's lyrics ID ([`WatchQueue::lyrics_id`],
    /// `MPLYt…`), or `None` when the page has no text.
    pub async fn lyrics_page(&self, lyrics_id: &str) -> Result<Option<Lyrics>, Error> {
        let mut body = Map::new();
        body.insert("browseId".into(), json!(lyrics_id));
        let response = self.post("browse", body).await?;
        Ok(pages::parse_lyrics(&response))
    }
}

fn parse_player_info(response: &Value) -> PlayerInfo {
    let audio = response.pointer("/playerConfig/audioConfig");
    let loudness_lkfs = audio
        .and_then(|a| a.get("trackAbsoluteLoudnessLkfs"))
        .or_else(|| audio.and_then(|a| a.get("perceptualLoudnessDb")))
        .and_then(Value::as_f64);
    let status = response
        .pointer("/playabilityStatus/status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    PlayerInfo {
        loudness_lkfs,
        duration: response
            .pointer("/videoDetails/lengthSeconds")
            .and_then(Value::as_str)
            .and_then(|s| s.parse().ok()),
        playable: status == "OK",
        reason: response
            .pointer("/playabilityStatus/reason")
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_info() {
        let ok = json!({
            "playabilityStatus": {"status": "OK"},
            "playerConfig": {"audioConfig": {"loudnessDb": -8.16, "trackAbsoluteLoudnessLkfs": -15.16}},
            "videoDetails": {"lengthSeconds": "335"}
        });
        assert_eq!(
            parse_player_info(&ok),
            PlayerInfo {
                loudness_lkfs: Some(-15.16),
                duration: Some(335),
                playable: true,
                reason: None
            }
        );
        let blocked = json!({"playabilityStatus": {"status": "UNPLAYABLE", "reason": "Not available in your country"}});
        let info = parse_player_info(&blocked);
        assert!(!info.playable);
        assert_eq!(
            info.reason.as_deref(),
            Some("Not available in your country")
        );
    }
}
