//! Single items: list rows (`musicResponsiveListItemRenderer`), cards
//! (`musicTwoRowItemRenderer`) and episode rows (`musicMultiRowListItemRenderer`).
//!
//! What an item is comes from its endpoints (`pageType`, `musicVideoType`),
//! never from the localized type word ("Song", "Album") in its subtitle.

use serde_json::Value;

use super::{find_key, page_type, parse_duration, runs, text, thumbnail};
use crate::model::{AlbumCard, ArtistCard, Item, LikeStatus, Link, PlaylistCard, Track, TrackKind};

/// Where a list row comes from. Search rows start their second column with
/// the item type ("Song • …"), which is dropped. Album rows carry play counts
/// where other lists name the performer, so their plain text is no artist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowContext {
    List,
    Album,
    Search,
}

/// Parses one entry of a shelf, grid or list (`{"<kind>Renderer": {…}}`).
pub fn parse_item(value: &Value, context: RowContext) -> Option<Item> {
    if let Some(row) = value.get("musicResponsiveListItemRenderer") {
        return parse_row(row, context);
    }
    if let Some(card) = value.get("musicTwoRowItemRenderer") {
        return parse_card(card);
    }
    if let Some(row) = value.get("musicMultiRowListItemRenderer") {
        return parse_episode_row(row);
    }
    None
}

/// Parses a list row that must be a track (playlist and album rows).
pub fn parse_track_row(value: &Value, context: RowContext) -> Option<Track> {
    match parse_item(value, context)? {
        Item::Track(track) => Some(track),
        _ => None,
    }
}

const SEPARATOR: &str = "•";

fn is_separator(run: &Value) -> bool {
    run.get("text")
        .and_then(Value::as_str)
        .is_some_and(|t| t.trim() == SEPARATOR)
}

fn run_text(run: &Value) -> &str {
    run.get("text").and_then(Value::as_str).unwrap_or_default()
}

fn run_browse(run: &Value) -> Option<(&str, Option<&str>)> {
    let endpoint = run.pointer("/navigationEndpoint/browseEndpoint")?;
    let id = endpoint.get("browseId")?.as_str()?;
    Some((id, page_type(endpoint)))
}

fn flex_column(row: &Value, index: usize) -> Option<&Value> {
    row.get("flexColumns")?
        .get(index)?
        .get("musicResponsiveListItemFlexColumnRenderer")?
        .get("text")
}

/// The detail runs of a row (columns after the title), cut into segments at
/// `•` separators and at column boundaries.
fn detail_segments(row: &Value, context: RowContext) -> Vec<Vec<&Value>> {
    let mut segments = Vec::new();
    let columns = row
        .get("flexColumns")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    for column in 1..columns {
        let Some(text) = flex_column(row, column) else {
            continue;
        };
        let mut current = Vec::new();
        for run in runs(text) {
            if is_separator(run) {
                segments.push(std::mem::take(&mut current));
            } else {
                current.push(run);
            }
        }
        segments.push(current);
        // Search rows: the first segment of the first detail column is the
        // item type, a plain word without an endpoint.
        if column == 1
            && context == RowContext::Search
            && segments.len() > 1
            && segments[0].iter().all(|r| run_browse(r).is_none())
        {
            segments.remove(0);
        }
    }
    segments.retain(|s| !s.is_empty());
    segments
}

/// Artists from runs: those linking to artist or channel pages; failing that,
/// the names of the first plain segment.
fn artists_from(segments: &[Vec<&Value>]) -> Vec<Link> {
    artists_in(segments, true)
}

fn artists_in(segments: &[Vec<&Value>], plain_fallback: bool) -> Vec<Link> {
    let linked: Vec<Link> = segments
        .iter()
        .flatten()
        .filter_map(|run| {
            let (id, kind) = run_browse(run)?;
            matches!(
                kind,
                Some("MUSIC_PAGE_TYPE_ARTIST" | "MUSIC_PAGE_TYPE_USER_CHANNEL")
            )
            .then(|| Link {
                name: run_text(run).to_owned(),
                id: Some(id.to_owned()),
            })
        })
        .collect();
    if !linked.is_empty() || !plain_fallback {
        return linked;
    }
    let Some(plain) = segments
        .iter()
        .find(|segment| segment.iter().all(|r| run_browse(r).is_none()))
    else {
        return Vec::new();
    };
    let joined: String = plain.iter().map(|r| run_text(r)).collect();
    if parse_duration(&joined).is_some() {
        return Vec::new();
    }
    joined
        .split(", ")
        .flat_map(|part| part.split(" & "))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| Link {
            name: name.to_owned(),
            id: None,
        })
        .collect()
}

fn album_from(segments: &[Vec<&Value>]) -> Option<Link> {
    segments.iter().flatten().find_map(|run| {
        let (id, kind) = run_browse(run)?;
        (kind == Some("MUSIC_PAGE_TYPE_ALBUM")).then(|| Link {
            name: run_text(run).to_owned(),
            id: Some(id.to_owned()),
        })
    })
}

fn year_from(segments: &[Vec<&Value>]) -> Option<u16> {
    segments.iter().flatten().find_map(|run| {
        let t = run_text(run).trim();
        (t.len() == 4)
            .then(|| t.parse::<u16>().ok())
            .flatten()
            .filter(|y| (1900..=2100).contains(y))
    })
}

fn kind_from(music_video_type: Option<&str>) -> TrackKind {
    match music_video_type {
        None | Some("MUSIC_VIDEO_TYPE_ATV") => TrackKind::Song,
        Some("MUSIC_VIDEO_TYPE_PODCAST_EPISODE") => TrackKind::Episode,
        Some(_) => TrackKind::Video,
    }
}

fn like_status(row: &Value) -> Option<LikeStatus> {
    let buttons = row
        .pointer("/menu/menuRenderer/topLevelButtons")?
        .as_array()?;
    buttons.iter().find_map(|button| {
        match button.pointer("/likeButtonRenderer/likeStatus")?.as_str()? {
            "LIKE" => Some(LikeStatus::Like),
            "DISLIKE" => Some(LikeStatus::Dislike),
            "INDIFFERENT" => Some(LikeStatus::Indifferent),
            _ => None,
        }
    })
}

fn is_explicit(value: &Value) -> bool {
    value
        .get("badges")
        .and_then(Value::as_array)
        .is_some_and(|badges| {
            badges.iter().any(|b| {
                b.pointer("/musicInlineBadgeRenderer/icon/iconType")
                    .and_then(Value::as_str)
                    == Some("MUSIC_EXPLICIT_BADGE")
            })
        })
}

fn segments_of(text_object: Option<&Value>) -> Vec<Vec<&Value>> {
    let mut segments = Vec::new();
    let mut current = Vec::new();
    for run in text_object.map(runs).unwrap_or_default() {
        if is_separator(run) {
            segments.push(std::mem::take(&mut current));
        } else {
            current.push(run);
        }
    }
    segments.push(current);
    segments.retain(|s| !s.is_empty());
    segments
}

fn parse_row(row: &Value, context: RowContext) -> Option<Item> {
    let title_text = flex_column(row, 0)?;
    let title = text(title_text)?;
    let title_run = runs(title_text).first();

    let video_id = row
        .pointer("/playlistItemData/videoId")
        .or_else(|| title_run?.pointer("/navigationEndpoint/watchEndpoint/videoId"))
        .or_else(|| {
            row.get("overlay")
                .and_then(|o| find_key(o, "watchEndpoint"))
                .and_then(|w| w.get("videoId"))
        })
        .and_then(Value::as_str);

    let segments = detail_segments(row, context);

    if let Some(video_id) = video_id {
        let video_type = title_run
            .and_then(|r| find_key(r, "musicVideoType"))
            .or_else(|| {
                row.get("overlay")
                    .and_then(|o| find_key(o, "musicVideoType"))
            })
            .and_then(Value::as_str);
        let duration = row
            .pointer("/fixedColumns/0/musicResponsiveListItemFixedColumnRenderer/text")
            .and_then(text)
            .and_then(|t| parse_duration(&t))
            .or_else(|| {
                segments.iter().find_map(|s| {
                    let joined: String = s.iter().map(|r| run_text(r)).collect();
                    parse_duration(&joined)
                })
            });
        return Some(Item::Track(Track {
            video_id: video_id.to_owned(),
            title,
            artists: artists_in(&segments, context != RowContext::Album),
            album: album_from(&segments),
            duration,
            thumbnail: row.get("thumbnail").and_then(thumbnail),
            like: like_status(row),
            set_video_id: row
                .pointer("/playlistItemData/playlistSetVideoId")
                .and_then(Value::as_str)
                .map(str::to_owned),
            kind: kind_from(video_type),
            available: row
                .get("musicItemRendererDisplayPolicy")
                .and_then(Value::as_str)
                != Some("MUSIC_ITEM_RENDERER_DISPLAY_POLICY_GREY_OUT"),
            explicit: is_explicit(row),
        }));
    }

    let endpoint = row.pointer("/navigationEndpoint/browseEndpoint")?;
    let id = endpoint.get("browseId")?.as_str()?.to_owned();
    let thumb = row.get("thumbnail").and_then(thumbnail);
    let subtitle = |segments: &[Vec<&Value>]| -> String {
        segments
            .iter()
            .map(|s| s.iter().map(|r| run_text(r)).collect::<String>())
            .collect::<Vec<_>>()
            .join(" • ")
    };
    Some(match page_type(endpoint) {
        Some("MUSIC_PAGE_TYPE_ALBUM") => Item::Album(AlbumCard {
            id,
            title,
            artists: artists_from(&segments),
            year: year_from(&segments),
            thumbnail: thumb,
        }),
        Some("MUSIC_PAGE_TYPE_ARTIST") => Item::Artist(ArtistCard {
            id,
            name: title,
            subtitle: subtitle(&segments),
            thumbnail: thumb,
        }),
        Some("MUSIC_PAGE_TYPE_PLAYLIST") => Item::Playlist(PlaylistCard {
            id,
            title,
            subtitle: subtitle(&segments),
            thumbnail: thumb,
        }),
        _ => Item::Other {
            title,
            id: Some(id),
        },
    })
}

fn parse_card(card: &Value) -> Option<Item> {
    let title = card.get("title").and_then(text)?;
    let segments = segments_of(card.get("subtitle"));
    let subtitle = card.get("subtitle").and_then(text).unwrap_or_default();
    let thumb = card.get("thumbnailRenderer").and_then(thumbnail);
    let endpoint = card.get("navigationEndpoint")?;

    if let Some(watch) = endpoint.get("watchEndpoint")
        && let Some(video_id) = watch.get("videoId").and_then(Value::as_str)
    {
        let video_type = find_key(watch, "musicVideoType").and_then(Value::as_str);
        return Some(Item::Track(Track {
            video_id: video_id.to_owned(),
            title,
            artists: artists_from(&segments),
            album: album_from(&segments),
            duration: None,
            thumbnail: thumb,
            like: None,
            set_video_id: None,
            kind: kind_from(video_type),
            available: true,
            explicit: is_explicit(card),
        }));
    }

    if let Some(playlist_id) = endpoint
        .pointer("/watchPlaylistEndpoint/playlistId")
        .and_then(Value::as_str)
    {
        return Some(Item::Playlist(PlaylistCard {
            id: format!("VL{playlist_id}"),
            title,
            subtitle,
            thumbnail: thumb,
        }));
    }

    let browse = endpoint.get("browseEndpoint")?;
    let id = browse.get("browseId")?.as_str()?.to_owned();
    Some(match page_type(browse) {
        Some("MUSIC_PAGE_TYPE_ALBUM") => Item::Album(AlbumCard {
            id,
            title,
            artists: artists_from(&segments),
            year: year_from(&segments),
            thumbnail: thumb,
        }),
        Some("MUSIC_PAGE_TYPE_ARTIST") => Item::Artist(ArtistCard {
            id,
            name: title,
            subtitle,
            thumbnail: thumb,
        }),
        Some("MUSIC_PAGE_TYPE_PLAYLIST") => Item::Playlist(PlaylistCard {
            id,
            title,
            subtitle,
            thumbnail: thumb,
        }),
        _ => Item::Other {
            title,
            id: Some(id),
        },
    })
}

/// Parses a queue entry of the `next` endpoint (`playlistPanelVideoRenderer`,
/// possibly inside a `playlistPanelVideoWrapperRenderer`).
pub fn parse_panel_video(value: &Value) -> Option<Track> {
    let video = value.get("playlistPanelVideoRenderer").or_else(|| {
        value.pointer(
            "/playlistPanelVideoWrapperRenderer/primaryRenderer/playlistPanelVideoRenderer",
        )
    })?;
    let video_id = video.get("videoId")?.as_str()?;
    let segments = segments_of(video.get("longBylineText"));
    let video_type = video
        .get("navigationEndpoint")
        .and_then(|e| find_key(e, "musicVideoType"))
        .and_then(Value::as_str);
    let like = video
        .get("menu")
        .and_then(|m| find_key(m, "likeStatus"))
        .and_then(Value::as_str)
        .and_then(|status| match status {
            "LIKE" => Some(LikeStatus::Like),
            "DISLIKE" => Some(LikeStatus::Dislike),
            "INDIFFERENT" => Some(LikeStatus::Indifferent),
            _ => None,
        });
    Some(Track {
        video_id: video_id.to_owned(),
        title: video.get("title").and_then(text)?,
        artists: artists_from(&segments),
        album: album_from(&segments),
        duration: video
            .get("lengthText")
            .and_then(text)
            .and_then(|t| parse_duration(&t)),
        thumbnail: video.get("thumbnail").and_then(thumbnail),
        like,
        set_video_id: video
            .get("playlistSetVideoId")
            .and_then(Value::as_str)
            .map(str::to_owned),
        kind: kind_from(video_type),
        available: video.get("unplayableText").is_none(),
        explicit: is_explicit(video),
    })
}

fn parse_episode_row(row: &Value) -> Option<Item> {
    let title = row.get("title").and_then(text)?;
    let Some(video_id) = row
        .get("onTap")
        .and_then(|t| find_key(t, "videoId"))
        .and_then(Value::as_str)
    else {
        return Some(Item::Other { title, id: None });
    };
    Some(Item::Track(Track {
        video_id: video_id.to_owned(),
        title,
        // The subtitle holds the date and length, not a performer.
        artists: Vec::new(),
        album: None,
        duration: None,
        thumbnail: row.get("thumbnail").and_then(thumbnail),
        like: None,
        set_video_id: None,
        kind: TrackKind::Episode,
        available: true,
        explicit: false,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn artist_run(name: &str, id: &str) -> Value {
        json!({"text": name, "navigationEndpoint": {"browseEndpoint": {"browseId": id,
            "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": "MUSIC_PAGE_TYPE_ARTIST"}}}}})
    }

    fn album_run(name: &str, id: &str) -> Value {
        json!({"text": name, "navigationEndpoint": {"browseEndpoint": {"browseId": id,
            "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": "MUSIC_PAGE_TYPE_ALBUM"}}}}})
    }

    fn column(runs: Vec<Value>) -> Value {
        json!({"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": runs}}})
    }

    /// A playlist row: title, artists, album, duration, rating.
    pub fn playlist_row() -> Value {
        json!({"musicResponsiveListItemRenderer": {
            "flexColumns": [
                column(vec![json!({"text": "Song A", "navigationEndpoint": {"watchEndpoint": {"videoId": "vid00000001",
                    "watchEndpointMusicSupportedConfigs": {"watchEndpointMusicConfig": {"musicVideoType": "MUSIC_VIDEO_TYPE_ATV"}}}}})]),
                column(vec![artist_run("Artist One", "UCartist1"), json!({"text": ", "}), artist_run("Artist Two", "UCartist2")]),
                column(vec![album_run("Album X", "MPREb_album1")]),
            ],
            "fixedColumns": [{"musicResponsiveListItemFixedColumnRenderer": {"text": {"runs": [{"text": "3:36"}]}}}],
            "playlistItemData": {"videoId": "vid00000001", "playlistSetVideoId": "SET1"},
            "thumbnail": {"musicThumbnailRenderer": {"thumbnail": {"thumbnails": [{"url": "t60", "width": 60}, {"url": "t120", "width": 120}]}}},
            "menu": {"menuRenderer": {"topLevelButtons": [{"likeButtonRenderer": {"likeStatus": "LIKE"}}]}},
            "badges": [{"musicInlineBadgeRenderer": {"icon": {"iconType": "MUSIC_EXPLICIT_BADGE"}}}]
        }})
    }

    #[test]
    fn playlist_row_becomes_a_track() {
        let track = parse_track_row(&playlist_row(), RowContext::List).unwrap();
        assert_eq!(track.video_id, "vid00000001");
        assert_eq!(track.title, "Song A");
        assert_eq!(
            track
                .artists
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>(),
            ["Artist One", "Artist Two"]
        );
        assert_eq!(track.artists[1].id.as_deref(), Some("UCartist2"));
        assert_eq!(
            track.album.as_ref().unwrap().id.as_deref(),
            Some("MPREb_album1")
        );
        assert_eq!(track.duration, Some(216));
        assert_eq!(track.thumbnail.as_deref(), Some("t120"));
        assert_eq!(track.like, Some(LikeStatus::Like));
        assert_eq!(track.set_video_id.as_deref(), Some("SET1"));
        assert_eq!(track.kind, TrackKind::Song);
        assert!(track.available && track.explicit);
    }

    #[test]
    fn uploader_without_channel_is_a_plain_artist() {
        let row = json!({"musicResponsiveListItemRenderer": {
            "flexColumns": [
                column(vec![json!({"text": "Upload", "navigationEndpoint": {"watchEndpoint": {"videoId": "vid00000002",
                    "watchEndpointMusicSupportedConfigs": {"watchEndpointMusicConfig": {"musicVideoType": "MUSIC_VIDEO_TYPE_UGC"}}}}})]),
                column(vec![json!({"text": "someuploader"})]),
            ],
            "playlistItemData": {"videoId": "vid00000002"},
            "musicItemRendererDisplayPolicy": "MUSIC_ITEM_RENDERER_DISPLAY_POLICY_GREY_OUT"
        }});
        let track = parse_track_row(&row, RowContext::List).unwrap();
        assert_eq!(
            track.artists,
            [Link {
                name: "someuploader".into(),
                id: None
            }]
        );
        // In an album the same plain column is a play count, not an artist.
        assert!(
            parse_track_row(&row, RowContext::Album)
                .unwrap()
                .artists
                .is_empty()
        );
        assert_eq!(track.kind, TrackKind::Video);
        assert!(!track.available);
        assert_eq!(track.duration, None);
    }

    #[test]
    fn search_row_drops_the_type_word() {
        let row = json!({"musicResponsiveListItemRenderer": {
            "flexColumns": [
                column(vec![json!({"text": "Song B"})]),
                column(vec![json!({"text": "Song"}), json!({"text": " • "}), artist_run("Band", "UCband"),
                    json!({"text": " • "}), album_run("Record", "MPREb_rec"), json!({"text": " • "}), json!({"text": "6:23"})]),
            ],
            "overlay": {"musicItemThumbnailOverlayRenderer": {"content": {"musicPlayButtonRenderer": {"playNavigationEndpoint": {
                "watchEndpoint": {"videoId": "vid00000003", "watchEndpointMusicSupportedConfigs": {"watchEndpointMusicConfig": {"musicVideoType": "MUSIC_VIDEO_TYPE_ATV"}}}}}}}}
        }});
        let Some(Item::Track(track)) = parse_item(&row, RowContext::Search) else {
            panic!("not a track");
        };
        assert_eq!(track.video_id, "vid00000003");
        assert_eq!(track.artists[0].name, "Band");
        assert_eq!(track.album.unwrap().name, "Record");
        assert_eq!(track.duration, Some(383));
    }

    #[test]
    fn search_row_of_an_album_and_an_artist() {
        let browse = |id: &str, kind: &str| {
            json!({"browseEndpoint": {"browseId": id,
            "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": kind}}}})
        };
        let album = json!({"musicResponsiveListItemRenderer": {
            "flexColumns": [column(vec![json!({"text": "Record"})]),
                column(vec![json!({"text": "Album"}), json!({"text": " • "}), artist_run("Band", "UCband"), json!({"text": " • "}), json!({"text": "1973"})])],
            "navigationEndpoint": browse("MPREb_rec", "MUSIC_PAGE_TYPE_ALBUM")
        }});
        let Some(Item::Album(card)) = parse_item(&album, RowContext::Search) else {
            panic!("not an album");
        };
        assert_eq!((card.id.as_str(), card.year), ("MPREb_rec", Some(1973)));
        assert_eq!(card.artists[0].name, "Band");

        let artist = json!({"musicResponsiveListItemRenderer": {
            "flexColumns": [column(vec![json!({"text": "Band"})]),
                column(vec![json!({"text": "Artist"}), json!({"text": " • "}), json!({"text": "4.9M monthly audience"})])],
            "navigationEndpoint": browse("UCband", "MUSIC_PAGE_TYPE_ARTIST")
        }});
        let Some(Item::Artist(card)) = parse_item(&artist, RowContext::Search) else {
            panic!("not an artist");
        };
        assert_eq!(
            (card.id.as_str(), card.subtitle.as_str()),
            ("UCband", "4.9M monthly audience")
        );
    }

    #[test]
    fn cards_by_endpoint() {
        let playlist = json!({"musicTwoRowItemRenderer": {
            "title": {"runs": [{"text": "Discover Mix"}]},
            "subtitle": {"runs": [{"text": "Mix"}, {"text": " • "}, {"text": "Made for you"}]},
            "navigationEndpoint": {"browseEndpoint": {"browseId": "VLRDTMAK5uy_test",
                "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": "MUSIC_PAGE_TYPE_PLAYLIST"}}}},
            "thumbnailRenderer": {"musicThumbnailRenderer": {"thumbnail": {"thumbnails": [{"url": "cover", "width": 226}]}}}
        }});
        let Some(Item::Playlist(card)) = parse_item(&playlist, RowContext::List) else {
            panic!("not a playlist");
        };
        assert_eq!(card.playlist_id(), "RDTMAK5uy_test");
        assert_eq!(card.subtitle, "Mix • Made for you");
        assert_eq!(card.thumbnail.as_deref(), Some("cover"));

        let song = json!({"musicTwoRowItemRenderer": {
            "title": {"runs": [{"text": "Grateful"}]},
            "subtitle": {"runs": [artist_run("Singer", "UCsinger")]},
            "navigationEndpoint": {"watchEndpoint": {"videoId": "vid00000004", "playlistId": "RDAMVMvid00000004",
                "watchEndpointMusicSupportedConfigs": {"watchEndpointMusicConfig": {"musicVideoType": "MUSIC_VIDEO_TYPE_OMV"}}}}
        }});
        let Some(Item::Track(track)) = parse_item(&song, RowContext::List) else {
            panic!("not a track");
        };
        assert_eq!(
            (track.video_id.as_str(), track.kind),
            ("vid00000004", TrackKind::Video)
        );
        assert_eq!(track.artists[0].id.as_deref(), Some("UCsinger"));
    }

    #[test]
    fn queue_entries() {
        let video = json!({"playlistPanelVideoRenderer": {
            "videoId": "vid00000006",
            "title": {"runs": [{"text": "Wish"}]},
            "longBylineText": {"runs": [artist_run("Band", "UCband"), {"text": " • "}, album_run("Record", "MPREb_rec"), {"text": " • "}, {"text": "1975"}]},
            "lengthText": {"runs": [{"text": "5:35"}]},
            "playlistSetVideoId": "SET6",
            "navigationEndpoint": {"watchEndpoint": {"videoId": "vid00000006",
                "watchEndpointMusicSupportedConfigs": {"watchEndpointMusicConfig": {"musicVideoType": "MUSIC_VIDEO_TYPE_ATV"}}}}
        }});
        let track = parse_panel_video(&video).unwrap();
        assert_eq!(track.title, "Wish");
        assert_eq!(track.artists[0].id.as_deref(), Some("UCband"));
        assert_eq!(track.album.unwrap().id.as_deref(), Some("MPREb_rec"));
        assert_eq!((track.duration, track.kind), (Some(335), TrackKind::Song));
        assert_eq!(track.set_video_id.as_deref(), Some("SET6"));

        let wrapped = json!({"playlistPanelVideoWrapperRenderer": {"primaryRenderer": video, "counterpart": []}});
        assert_eq!(parse_panel_video(&wrapped).unwrap().video_id, "vid00000006");
    }

    #[test]
    fn episode_rows() {
        let row = json!({"musicMultiRowListItemRenderer": {
            "title": {"runs": [{"text": "Episode 1"}]},
            "subtitle": {"runs": [{"text": "Show"}]},
            "onTap": {"watchEndpoint": {"videoId": "vid00000005"}}
        }});
        let Some(Item::Track(track)) = parse_item(&row, RowContext::List) else {
            panic!("not a track");
        };
        assert_eq!(
            (track.video_id.as_str(), track.kind),
            ("vid00000005", TrackKind::Episode)
        );
    }
}
