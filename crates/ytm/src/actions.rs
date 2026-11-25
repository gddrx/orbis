//! Changes to the account: ratings, playlist edits, new and deleted playlists.

use serde_json::{Map, Value, json};

use crate::client::{Client, Error};
use crate::model::{PlaylistOption, Privacy, Rating};
use crate::parse::{find_key, text};

/// The Liked Music playlist: adding a track there is liking it.
pub const LIKED_MUSIC: &str = "LM";

fn object(pairs: Vec<(&str, Value)>) -> Map<String, Value> {
    pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
}

/// Options of `playlist/get_add_to_playlist`.
pub(crate) fn parse_playlist_options(response: &Value) -> Vec<PlaylistOption> {
    find_key(response, "addToPlaylistRenderer")
        .and_then(|r| r.get("playlists"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let option = p.get("playlistAddToOptionRenderer")?;
            Some(PlaylistOption {
                playlist_id: option.get("playlistId")?.as_str()?.to_owned(),
                title: option.get("title").and_then(text)?,
                subtitle: option
                    .get("shortBylineText")
                    .and_then(text)
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// Checks the `status` of a `browse/edit_playlist` response and returns the
/// set video IDs of added tracks.
pub(crate) fn parse_edit_result(response: &Value) -> Result<Vec<String>, Error> {
    let status = response
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if status != "STATUS_SUCCEEDED" {
        // A duplicate comes back as a confirm dialog instead of a status.
        let message = find_key(response, "confirmDialogRenderer")
            .and_then(|d| d.get("dialogMessages"))
            .and_then(Value::as_array)
            .and_then(|m| m.first())
            .and_then(text)
            .unwrap_or_else(|| format!("status {status:?}"));
        return Err(Error::Rejected(message));
    }
    Ok(response
        .get("playlistEditResults")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| {
            r.pointer("/playlistEditVideoAddedResultData/setVideoId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect())
}

impl Client {
    /// Likes, dislikes or un-rates a track.
    pub async fn rate(&self, video_id: &str, rating: Rating) -> Result<(), Error> {
        let endpoint = match rating {
            Rating::Like => "like/like",
            Rating::Dislike => "like/dislike",
            Rating::None => "like/removelike",
        };
        self.post(
            endpoint,
            object(vec![("target", json!({"videoId": video_id}))]),
        )
        .await?;
        Ok(())
    }

    /// The playlists the account can add `video_id` to.
    pub async fn playlist_options(&self, video_id: &str) -> Result<Vec<PlaylistOption>, Error> {
        let response = self
            .post(
                "playlist/get_add_to_playlist",
                object(vec![("videoIds", json!([video_id]))]),
            )
            .await?;
        Ok(parse_playlist_options(&response))
    }

    /// Adds `video_id` to the end of `playlist_id`; a track already there is
    /// refused. Returns the new entry's set video ID (none for Liked Music,
    /// where adding is a like).
    pub async fn add_to_playlist(
        &self,
        playlist_id: &str,
        video_id: &str,
    ) -> Result<Option<String>, Error> {
        if playlist_id == LIKED_MUSIC {
            self.rate(video_id, Rating::Like).await?;
            return Ok(None);
        }
        let response = self
            .post(
                "browse/edit_playlist",
                object(vec![
                    ("playlistId", json!(playlist_id)),
                    (
                        "actions",
                        json!([{
                            "action": "ACTION_ADD_VIDEO",
                            "addedVideoId": video_id,
                            "dedupeOption": "DEDUPE_OPTION_CHECK",
                        }]),
                    ),
                ]),
            )
            .await?;
        Ok(parse_edit_result(&response)?.into_iter().next())
    }

    /// Removes one entry from a playlist. `set_video_id` tells which, when the
    /// track is there more than once.
    pub async fn remove_from_playlist(
        &self,
        playlist_id: &str,
        video_id: &str,
        set_video_id: &str,
    ) -> Result<(), Error> {
        if playlist_id == LIKED_MUSIC {
            return self.rate(video_id, Rating::None).await;
        }
        let response = self
            .post(
                "browse/edit_playlist",
                object(vec![
                    ("playlistId", json!(playlist_id)),
                    (
                        "actions",
                        json!([{
                            "action": "ACTION_REMOVE_VIDEO",
                            "removedVideoId": video_id,
                            "setVideoId": set_video_id,
                        }]),
                    ),
                ]),
            )
            .await?;
        parse_edit_result(&response).map(|_| ())
    }

    /// Creates a playlist and returns its ID.
    pub async fn create_playlist(
        &self,
        title: &str,
        description: &str,
        privacy: Privacy,
        video_ids: &[&str],
    ) -> Result<String, Error> {
        let mut body = object(vec![
            ("title", json!(title)),
            ("description", json!(description)),
            ("privacyStatus", json!(privacy.as_api())),
        ]);
        if !video_ids.is_empty() {
            body.insert("videoIds".into(), json!(video_ids));
        }
        let response = self.post("playlist/create", body).await?;
        response
            .get("playlistId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Error::Unexpected("no playlistId in the answer".into()))
    }

    /// Deletes a playlist of the account.
    pub async fn delete_playlist(&self, playlist_id: &str) -> Result<(), Error> {
        self.post(
            "playlist/delete",
            object(vec![("playlistId", json!(playlist_id))]),
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlist_options() {
        let response = json!({"contents": [{"addToPlaylistRenderer": {"playlists": [
            {"playlistAddToOptionRenderer": {"playlistId": "LM", "title": {"runs": [{"text": "Liked Music"}]}}},
            {"playlistAddToOptionRenderer": {"playlistId": "PLcalm", "title": {"runs": [{"text": "Calm"}]},
                "shortBylineText": {"runs": [{"text": "164 tracks"}]}}}
        ]}}]});
        let options = parse_playlist_options(&response);
        assert_eq!(options.len(), 2);
        assert_eq!(options[1].playlist_id, "PLcalm");
        assert_eq!(options[1].subtitle, "164 tracks");
    }

    #[test]
    fn edit_results() {
        let added = json!({"status": "STATUS_SUCCEEDED", "playlistEditResults": [
            {"playlistEditVideoAddedResultData": {"setVideoId": "SET9", "videoId": "vid00000009"}}
        ]});
        assert_eq!(parse_edit_result(&added).unwrap(), ["SET9"]);

        let removed = json!({"status": "STATUS_SUCCEEDED"});
        assert!(parse_edit_result(&removed).unwrap().is_empty());

        let duplicate = json!({"actions": [{"addToPlaylistCommand": {}, "confirmDialogEndpoint": {"content": {
            "confirmDialogRenderer": {"dialogMessages": [{"runs": [{"text": "This song is already in your playlist"}]}]}}}}]});
        match parse_edit_result(&duplicate) {
            Err(Error::Rejected(message)) => assert!(message.contains("already")),
            other => panic!("unexpected {other:?}"),
        }
    }
}
