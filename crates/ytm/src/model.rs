//! What the client reads from YouTube Music, independent of the JSON layout.

/// A name with an optional browse ID (an artist channel `UC…`, an album
/// `MPREb_…`). Uploaders without a channel page have no ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub name: String,
    pub id: Option<String>,
}

/// The rating of a track by the signed-in account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LikeStatus {
    Like,
    Dislike,
    Indifferent,
}

/// What a playable item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    /// An audio track (`MUSIC_VIDEO_TYPE_ATV`).
    Song,
    /// A music video or an upload.
    Video,
    /// A podcast episode.
    Episode,
}

/// A playable track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub video_id: String,
    pub title: String,
    pub artists: Vec<Link>,
    pub album: Option<Link>,
    /// Length in seconds.
    pub duration: Option<u32>,
    /// URL of the largest thumbnail.
    pub thumbnail: Option<String>,
    pub like: Option<LikeStatus>,
    /// The track's ID inside a playlist, needed to move or remove it.
    pub set_video_id: Option<String>,
    pub kind: TrackKind,
    /// `false` when the item is greyed out (removed or blocked in the region).
    pub available: bool,
    pub explicit: bool,
}

/// A playlist or a mix as a card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistCard {
    /// The browse ID (`VL…`).
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub thumbnail: Option<String>,
}

impl PlaylistCard {
    /// The playlist ID without the `VL` browse prefix.
    pub fn playlist_id(&self) -> &str {
        self.id.strip_prefix("VL").unwrap_or(&self.id)
    }
}

/// An album, single or EP as a card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumCard {
    /// The browse ID (`MPREb_…`).
    pub id: String,
    pub title: String,
    pub artists: Vec<Link>,
    pub year: Option<u16>,
    pub thumbnail: Option<String>,
}

/// An artist as a card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistCard {
    /// The channel ID (`UC…`).
    pub id: String,
    pub name: String,
    pub subtitle: String,
    pub thumbnail: Option<String>,
}

/// One entry of a shelf, a grid or search results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Track(Track),
    Playlist(PlaylistCard),
    Album(AlbumCard),
    Artist(ArtistCard),
    /// Podcasts, profiles and other pages the player does not open yet.
    Other {
        title: String,
        id: Option<String>,
    },
}

/// A titled row of items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shelf {
    pub title: String,
    pub items: Vec<Item>,
    /// Browse ID of the shelf's "More" page, when it has one.
    pub more: Option<String>,
}

/// One page of the home feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomePage {
    pub shelves: Vec<Shelf>,
    /// Token of the next page; `None` on the last one.
    pub continuation: Option<String>,
}

/// Well-known personal mixes found on the home page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mix {
    Discover,
    NewRelease,
    Supermix,
    Replay,
}

impl Mix {
    /// The mix's title as YouTube Music shows it with `hl=en`.
    pub fn english_title(self) -> &'static str {
        match self {
            Self::Discover => "Discover Mix",
            Self::NewRelease => "New Release Mix",
            Self::Supermix => "My Supermix",
            Self::Replay => "Replay Mix",
        }
    }

    pub const ALL: [Mix; 4] = [Mix::Discover, Mix::NewRelease, Mix::Supermix, Mix::Replay];
}

/// A playlist with its first page of tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playlist {
    /// The playlist ID (without `VL`).
    pub id: String,
    pub title: String,
    /// Such as "Mix • 2026" or "Playlist • 2026".
    pub subtitle: String,
    /// Such as "2,820 songs".
    pub details: String,
    pub author: Option<Link>,
    pub thumbnail: Option<String>,
    pub tracks: Vec<Track>,
    pub continuation: Option<String>,
}

/// A further page of a playlist's tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackPage {
    pub tracks: Vec<Track>,
    pub continuation: Option<String>,
}

/// An album with its tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Album {
    /// The browse ID (`MPREb_…`).
    pub id: String,
    /// The album's playlist (`OLAK5uy_…`), for playing it as a queue.
    pub playlist_id: Option<String>,
    pub title: String,
    /// Such as "Album • 1973" or "Single • 2026".
    pub subtitle: String,
    pub artists: Vec<Link>,
    pub year: Option<u16>,
    pub thumbnail: Option<String>,
    pub tracks: Vec<Track>,
}

/// An artist page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artist {
    pub id: String,
    pub name: String,
    pub thumbnail: Option<String>,
    /// Playlist that shuffles the artist's songs.
    pub shuffle_playlist: Option<String>,
    /// The artist's radio mix.
    pub radio_playlist: Option<String>,
    pub shelves: Vec<Shelf>,
}

/// Results of a search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResults {
    /// The top result, when YouTube Music shows one.
    pub top: Option<Item>,
    pub items: Vec<Item>,
    /// The next page: a filtered search has them, 20 items each.
    pub continuation: Option<String>,
}

/// What a search is narrowed to. Unfiltered, YouTube Music answers with a
/// top result and a few of each kind, some 30 in all, without more pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFilter {
    Songs,
    Videos,
    Albums,
    Artists,
}

impl SearchFilter {
    pub const ALL: [SearchFilter; 4] = [Self::Songs, Self::Videos, Self::Albums, Self::Artists];

    /// The name orbis keeps it under: `songs`, `videos`, `albums`, `artists`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Songs => "songs",
            Self::Videos => "videos",
            Self::Albums => "albums",
            Self::Artists => "artists",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == name)
    }

    /// The `params` of the `search` request, as the web app sends them.
    pub fn params(self) -> String {
        let kind = match self {
            Self::Songs => "II",
            Self::Videos => "IQ",
            Self::Albums => "IY",
            Self::Artists => "Ig",
        };
        format!("EgWKAQ{kind}AWoMEA4QChADEAQQCRAF")
    }
}

/// A play queue from the `next` endpoint: a radio or a watch playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchQueue {
    pub tracks: Vec<Track>,
    /// Token of more tracks; radios never run out.
    pub continuation: Option<String>,
    /// Browse ID of the first track's lyrics (`MPLYt…`), when it has any.
    pub lyrics_id: Option<String>,
    /// Browse ID of tracks related to the first one (`MPTRt…`).
    pub related_id: Option<String>,
    /// The account's rating of the track the queue starts with.
    pub current_like: Option<LikeStatus>,
}

/// The lyrics of a track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lyrics {
    pub text: String,
    /// Such as "Source: Musixmatch".
    pub source: Option<String>,
}

impl WatchQueue {
    /// Appends the tracks of a continuation that are not in the queue yet and
    /// takes its token. Radio continuations are a sliding window that repeats
    /// most of what came before. Returns how many tracks were added.
    pub fn append(&mut self, next: WatchQueue) -> usize {
        let before = self.tracks.len();
        let mut known: std::collections::HashSet<String> =
            self.tracks.iter().map(|t| t.video_id.clone()).collect();
        self.tracks.extend(
            next.tracks
                .into_iter()
                .filter(|t| known.insert(t.video_id.clone())),
        );
        self.continuation = next.continuation;
        self.tracks.len() - before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(video_id: &str) -> Track {
        Track {
            video_id: video_id.into(),
            title: video_id.into(),
            artists: Vec::new(),
            album: None,
            duration: None,
            thumbnail: None,
            like: None,
            set_video_id: None,
            kind: TrackKind::Song,
            available: true,
            explicit: false,
        }
    }

    fn queue(ids: &[&str], continuation: Option<&str>) -> WatchQueue {
        WatchQueue {
            tracks: ids.iter().map(|id| track(id)).collect(),
            continuation: continuation.map(str::to_owned),
            lyrics_id: None,
            related_id: None,
            current_like: None,
        }
    }

    #[test]
    fn append_skips_repeats_and_keeps_order() {
        let mut radio = queue(&["a", "b", "c"], Some("t1"));
        let added = radio.append(queue(&["b", "c", "d", "e", "d"], Some("t2")));
        assert_eq!(added, 2);
        let ids: Vec<_> = radio.tracks.iter().map(|t| t.video_id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c", "d", "e"]);
        assert_eq!(radio.continuation.as_deref(), Some("t2"));
        assert_eq!(radio.append(queue(&["a"], None)), 0);
        assert_eq!(radio.continuation, None);
    }
}

/// A rating to give a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rating {
    Like,
    Dislike,
    /// Removes a like or a dislike.
    None,
}

/// A playlist the "Add to playlist" dialog offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistOption {
    /// The playlist ID (`LM` is Liked Music: adding there is a like).
    pub playlist_id: String,
    pub title: String,
    /// Such as the owner and the track count.
    pub subtitle: String,
}

/// Privacy of a new playlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Privacy {
    Private,
    Unlisted,
    Public,
}

impl Privacy {
    pub fn as_api(self) -> &'static str {
        match self {
            Self::Private => "PRIVATE",
            Self::Unlisted => "UNLISTED",
            Self::Public => "PUBLIC",
        }
    }
}

/// What the `player` endpoint says about a track, without its streams.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerInfo {
    /// Measured loudness of the track (`trackAbsoluteLoudnessLkfs`).
    pub loudness_lkfs: Option<f64>,
    /// Length in seconds.
    pub duration: Option<u32>,
    /// `false` when YouTube refuses to play it (removed, blocked, age-gated).
    pub playable: bool,
    /// Why it is not playable.
    pub reason: Option<String>,
}
