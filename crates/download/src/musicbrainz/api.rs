//! What orbis reads from MusicBrainz's JSON web service (`/ws/2`): only
//! the fields it uses, so a response serialised back is also its trimmed
//! copy for the cache.

use serde::{Deserialize, Serialize};

/// A page of recording search results.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SearchPage {
    #[serde(default)]
    pub recordings: Vec<Recording>,
}

/// A recording: a search result, or a lookup with genres and artists.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Recording {
    pub id: String,
    pub title: String,
    /// Length in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
    /// Such as "live, 1988-07-06: …" or "6:24 album version".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disambiguation: Option<String>,
    /// `YYYY`, `YYYY-MM` or `YYYY-MM-DD`: the first release with it.
    #[serde(
        rename = "first-release-date",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub first_release_date: Option<String>,
    /// MusicBrainz's own relevance of a search result, 0–100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<u32>,
    #[serde(rename = "artist-credit", default)]
    pub artist_credit: Vec<Credit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub releases: Vec<Release>,
    /// How many releases the recording is on, when the cache's copy lists
    /// only some of them (orbis's own field, see `pick::trim`).
    #[serde(
        rename = "orbis-release-count",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub release_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<Genre>,
}

impl Recording {
    /// How many releases the recording is on.
    pub fn release_count(&self) -> usize {
        self.release_count.unwrap_or(self.releases.len())
    }
}

/// One artist of an artist credit, as credited.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Credit {
    /// The name as credited here.
    pub name: String,
    pub artist: Artist,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Artist {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<Genre>,
}

/// A release a recording is on, with the medium and track it is at.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Release {
    pub id: String,
    pub title: String,
    /// "Official", "Promotion", "Bootleg", "Withdrawn"…
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// Where it came out: `XW` is worldwide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// The number of media (discs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    /// Tracks on all media.
    #[serde(
        rename = "track-count",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub track_count: Option<u32>,
    #[serde(
        rename = "artist-credit",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub artist_credit: Vec<Credit>,
    #[serde(
        rename = "release-group",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub release_group: Option<ReleaseGroup>,
    /// In search results: only the medium with the recording, and on it
    /// only its track.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub media: Vec<Medium>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ReleaseGroup {
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<Genre>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Medium {
    /// The disc number, from 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
    /// "CD", "Digital Media", "12\" Vinyl"…
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(
        rename = "track-count",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub track_count: Option<u32>,
    /// Tracks on the medium before the listed one.
    #[serde(
        rename = "track-offset",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub track_offset: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub track: Vec<Track>,
}

/// A track of a release (not the recording): its own ID.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Track {
    pub id: String,
}

/// A genre with the number of votes for it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Genre {
    pub name: String,
    #[serde(default)]
    pub count: u32,
}
