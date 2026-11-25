//! Whole responses: the home feed, playlists, the library, albums, artists
//! and search.

use serde_json::Value;

use super::items::{RowContext, parse_item, parse_panel_video, parse_track_row};
use super::{continuation_token, find_key, find_string, runs, text, thumbnail};
use crate::model::{
    Album, Artist, HomePage, Item, Link, Lyrics, Playlist, PlaylistCard, SearchResults, Shelf,
    Track, TrackPage, WatchQueue,
};

/// The `sectionListRenderer` of the first tab of a single-column page.
fn first_tab_sections(response: &Value) -> Option<&Value> {
    response
        .pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer")
}

/// Sections and the next-page token of a home page or its continuation.
fn section_list(response: &Value) -> (&[Value], Option<String>) {
    let list = response
        .pointer("/continuationContents/sectionListContinuation")
        .or_else(|| first_tab_sections(response));
    let Some(list) = list else {
        return (&[], None);
    };
    let contents = list
        .get("contents")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    (contents, continuation_token(list))
}

/// Parses one section into a shelf: carousels and list shelves.
pub fn parse_shelf(section: &Value) -> Option<Shelf> {
    let (kind, renderer) = section.as_object()?.iter().next()?;
    if kind != "musicShelfRenderer" && !kind.ends_with("CarouselShelfRenderer") {
        return None;
    }
    let context = RowContext::List;
    let header = renderer
        .get("header")
        .and_then(|h| h.as_object()?.values().next());
    let title = header
        .and_then(|h| h.get("title"))
        .or_else(|| renderer.get("title"))
        .and_then(text)
        .unwrap_or_default();
    let more = header
        .and_then(|h| {
            h.pointer(
                "/moreContentButton/buttonRenderer/navigationEndpoint/browseEndpoint/browseId",
            )
        })
        .or_else(|| renderer.pointer("/bottomEndpoint/browseEndpoint/browseId"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let items = renderer
        .get("contents")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|i| parse_item(i, context))
                .collect()
        })
        .unwrap_or_default();
    Some(Shelf { title, items, more })
}

/// A page of the home feed: the first response or a continuation.
pub fn parse_home(response: &Value) -> HomePage {
    let (sections, continuation) = section_list(response);
    HomePage {
        shelves: sections.iter().filter_map(parse_shelf).collect(),
        continuation,
    }
}

/// Tracks of a list and the token of its next page (the last
/// `continuationItemRenderer`).
fn tracks_and_token(items: &[Value]) -> (Vec<Track>, Option<String>) {
    let tracks = items
        .iter()
        .filter_map(|i| parse_track_row(i, RowContext::List))
        .collect();
    let token = items.iter().rev().find_map(continuation_token);
    (tracks, token)
}

/// A playlist page (`browse` with `VL<playlist id>`).
pub fn parse_playlist(response: &Value) -> Option<Playlist> {
    let header = response
        .pointer("/contents/twoColumnBrowseResultsRenderer/tabs/0")
        .and_then(|tab| find_key(tab, "musicResponsiveHeaderRenderer"))?;
    let shelf = response
        .pointer("/contents/twoColumnBrowseResultsRenderer/secondaryContents")
        .and_then(|c| find_key(c, "musicPlaylistShelfRenderer"));
    let items = shelf
        .and_then(|s| s.get("contents"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let (tracks, continuation) = tracks_and_token(items);
    let author = header.get("straplineTextOne").and_then(|t| {
        let run = runs(t).first()?;
        Some(Link {
            name: text(t)?,
            id: run
                .pointer("/navigationEndpoint/browseEndpoint/browseId")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    });
    Some(Playlist {
        id: shelf
            .and_then(|s| s.get("playlistId"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        title: header.get("title").and_then(text).unwrap_or_default(),
        subtitle: header.get("subtitle").and_then(text).unwrap_or_default(),
        details: header
            .get("secondSubtitle")
            .and_then(text)
            .unwrap_or_default(),
        author,
        thumbnail: header.get("thumbnail").and_then(thumbnail),
        tracks,
        continuation,
    })
}

/// The album (`MPREb_…`) of an album's own playlist (`OLAK5uy_…`): YouTube
/// Music shows such a playlist without a header, only its tracks, and they
/// link to the album.
pub fn parse_album_of_playlist(response: &Value) -> Option<String> {
    let shelf = response
        .pointer("/contents/twoColumnBrowseResultsRenderer/secondaryContents")
        .and_then(|c| find_key(c, "musicPlaylistShelfRenderer"))?;
    find_string(shelf, &|s| s.starts_with("MPREb_")).map(str::to_owned)
}

/// A continuation of a playlist's tracks.
pub fn parse_track_page(response: &Value) -> TrackPage {
    let items: Vec<Value> = response
        .get("onResponseReceivedActions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|action| action.pointer("/appendContinuationItemsAction/continuationItems"))
        .filter_map(Value::as_array)
        .flatten()
        .cloned()
        .collect();
    let (tracks, continuation) = tracks_and_token(&items);
    TrackPage {
        tracks,
        continuation,
    }
}

/// The library's playlists (`FEmusic_liked_playlists`), without the
/// "New playlist" button. Returns the token of the next page as well.
pub fn parse_library_playlists(response: &Value) -> (Vec<PlaylistCard>, Option<String>) {
    let grid = first_tab_sections(response)
        .and_then(|s| find_key(s, "gridRenderer"))
        .or_else(|| response.pointer("/continuationContents/gridContinuation"));
    let Some(grid) = grid else {
        return (Vec::new(), None);
    };
    let playlists = grid
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| match parse_item(item, RowContext::List) {
            Some(Item::Playlist(card)) => Some(card),
            _ => None,
        })
        .collect();
    (playlists, continuation_token(grid))
}

/// An album page (`browse` with `MPREb_…`).
pub fn parse_album(response: &Value, id: &str) -> Option<Album> {
    let header = response
        .pointer("/contents/twoColumnBrowseResultsRenderer/tabs/0")
        .and_then(|tab| find_key(tab, "musicResponsiveHeaderRenderer"))?;
    let artists: Vec<Link> = header
        .get("straplineTextOne")
        .map(runs)
        .unwrap_or_default()
        .iter()
        .filter_map(|run| {
            let name = run.get("text")?.as_str()?;
            let id = run
                .pointer("/navigationEndpoint/browseEndpoint/browseId")
                .and_then(Value::as_str);
            (id.is_some() || !matches!(name.trim(), "," | "&" | "•" | "")).then(|| Link {
                name: name.to_owned(),
                id: id.map(str::to_owned),
            })
        })
        .collect();
    let subtitle = header.get("subtitle").and_then(text).unwrap_or_default();
    let year = subtitle
        .split('•')
        .map(str::trim)
        .find_map(|part| (part.len() == 4).then(|| part.parse().ok()).flatten());
    let thumbnail = header.get("thumbnail").and_then(thumbnail);
    let items = response
        .pointer("/contents/twoColumnBrowseResultsRenderer/secondaryContents")
        .and_then(|c| find_key(c, "musicShelfRenderer"))
        .and_then(|s| s.get("contents"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let tracks = items
        .iter()
        .filter_map(|i| parse_track_row(i, RowContext::Album))
        .map(|mut track| {
            // Album rows leave out what the header says.
            if track.artists.is_empty() {
                track.artists = artists.clone();
            }
            if track.album.is_none() {
                track.album = Some(Link {
                    name: header.get("title").and_then(text).unwrap_or_default(),
                    id: Some(id.to_owned()),
                });
            }
            if track.thumbnail.is_none() {
                track.thumbnail = thumbnail.clone();
            }
            track
        })
        .collect();
    Some(Album {
        id: id.to_owned(),
        playlist_id: find_string(response, &|s| s.starts_with("OLAK5uy_")).map(str::to_owned),
        title: header.get("title").and_then(text).unwrap_or_default(),
        subtitle,
        artists,
        year,
        thumbnail,
        tracks,
    })
}

/// An artist page (`browse` with the channel ID).
pub fn parse_artist(response: &Value, id: &str) -> Option<Artist> {
    let header = response
        .get("header")
        .and_then(|h| h.as_object()?.values().next())?;
    let playlist_of = |button: &str| {
        header
            .get(button)
            .and_then(|b| find_key(b, "playlistId"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let shelves = first_tab_sections(response)
        .and_then(|s| s.get("contents"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(parse_shelf)
        .filter(|shelf| !shelf.items.is_empty())
        .collect();
    Some(Artist {
        id: id.to_owned(),
        name: header.get("title").and_then(text).unwrap_or_default(),
        thumbnail: header.get("thumbnail").and_then(thumbnail),
        shuffle_playlist: playlist_of("playButton"),
        radio_playlist: playlist_of("startRadioButton"),
        shelves,
    })
}

/// Search results (`search` with `query`).
pub fn parse_search(response: &Value) -> SearchResults {
    let sections = response
        .pointer("/contents/tabbedSearchResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut results = SearchResults {
        top: None,
        items: Vec::new(),
        continuation: None,
    };
    for section in sections {
        if let Some(card) = section.get("musicCardShelfRenderer") {
            results.top = top_result(card);
            // Songs inside an artist's card leave the artist out.
            let card_artist = match &results.top {
                Some(Item::Artist(artist)) => Some(Link {
                    name: artist.name.clone(),
                    id: Some(artist.id.clone()),
                }),
                _ => None,
            };
            results.items.extend(
                card.get("contents")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|i| parse_item(i, RowContext::Search))
                    .map(|item| match (item, &card_artist) {
                        (Item::Track(mut track), Some(artist)) if track.artists.is_empty() => {
                            track.artists.push(artist.clone());
                            Item::Track(track)
                        }
                        (item, _) => item,
                    }),
            );
            continue;
        }
        let renderer = section
            .get("itemSectionRenderer")
            .or_else(|| section.get("musicShelfRenderer"));
        results.items.extend(
            renderer
                .and_then(|r| r.get("contents"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|i| parse_item(i, RowContext::Search)),
        );
        // A filtered search is one shelf that goes on.
        if let Some(token) = renderer.and_then(continuation_token) {
            results.continuation = Some(token);
        }
    }
    results
}

/// The next page of a filtered search (`search` with `continuation`).
pub fn parse_search_next(response: &Value) -> SearchResults {
    let shelf = response.pointer("/continuationContents/musicShelfContinuation");
    SearchResults {
        top: None,
        items: shelf
            .and_then(|s| s.get("contents"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|i| parse_item(i, RowContext::Search))
            .collect(),
        continuation: shelf.and_then(continuation_token),
    }
}

/// A `next` response: the first one (with tabs) or a continuation.
pub fn parse_watch(response: &Value) -> WatchQueue {
    let tabs = response
        .pointer("/contents/singleColumnMusicWatchNextResultsRenderer/tabbedRenderer/watchNextTabbedResultsRenderer/tabs")
        .and_then(Value::as_array);
    let panel = response
        .pointer("/continuationContents/playlistPanelContinuation")
        .or_else(|| {
            tabs?
                .first()?
                .pointer("/tabRenderer/content/musicQueueRenderer/content/playlistPanelRenderer")
        });
    let tab_browse_id = |prefix: &str| {
        tabs.into_iter().flatten().find_map(|tab| {
            let renderer = tab.get("tabRenderer")?;
            if renderer.get("unselectable").and_then(Value::as_bool) == Some(true) {
                return None;
            }
            let id = renderer
                .pointer("/endpoint/browseEndpoint/browseId")?
                .as_str()?;
            id.starts_with(prefix).then(|| id.to_owned())
        })
    };
    let (tracks, continuation) = match panel {
        Some(panel) => (
            panel
                .get("contents")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(parse_panel_video)
                .collect(),
            panel
                .get("continuations")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find_map(|c| {
                    c.pointer("/nextRadioContinuationData/continuation")
                        .or_else(|| c.pointer("/nextContinuationData/continuation"))
                        .and_then(Value::as_str)
                })
                .map(str::to_owned),
        ),
        None => (Vec::new(), None),
    };
    let current_like = response
        .get("playerOverlays")
        .and_then(|o| find_key(o, "likeButtonRenderer"))
        .and_then(|b| b.get("likeStatus"))
        .and_then(Value::as_str)
        .and_then(|status| match status {
            "LIKE" => Some(crate::model::LikeStatus::Like),
            "DISLIKE" => Some(crate::model::LikeStatus::Dislike),
            "INDIFFERENT" => Some(crate::model::LikeStatus::Indifferent),
            _ => None,
        });
    WatchQueue {
        tracks,
        continuation,
        lyrics_id: tab_browse_id("MPLYt"),
        related_id: tab_browse_id("MPTRt"),
        current_like,
    }
}

/// The lyrics page (`browse` with the `MPLYt…` ID of a `next` response).
pub fn parse_lyrics(response: &Value) -> Option<Lyrics> {
    let shelf = find_key(response, "musicDescriptionShelfRenderer")?;
    Some(Lyrics {
        text: shelf.get("description").and_then(text)?,
        source: shelf.get("footer").and_then(text),
    })
}

/// The top result card: its title run links to the item.
fn top_result(card: &Value) -> Option<Item> {
    let title = card.get("title")?;
    let run = runs(title).first()?;
    let name = text(title)?;
    let thumbnail = card.get("thumbnail").and_then(thumbnail);
    let subtitle = card.get("subtitle").and_then(text).unwrap_or_default();
    let endpoint = run.get("navigationEndpoint")?;
    if let Some(video_id) = endpoint
        .pointer("/watchEndpoint/videoId")
        .and_then(Value::as_str)
    {
        return Some(Item::Track(Track {
            video_id: video_id.to_owned(),
            title: name,
            artists: Vec::new(),
            album: None,
            duration: None,
            thumbnail,
            like: None,
            set_video_id: None,
            kind: crate::model::TrackKind::Song,
            available: true,
            explicit: false,
        }));
    }
    let browse = endpoint.get("browseEndpoint")?;
    let id = browse.get("browseId")?.as_str()?.to_owned();
    Some(match super::page_type(browse) {
        Some("MUSIC_PAGE_TYPE_ARTIST") => Item::Artist(crate::model::ArtistCard {
            id,
            name,
            subtitle,
            thumbnail,
        }),
        Some("MUSIC_PAGE_TYPE_ALBUM") => Item::Album(crate::model::AlbumCard {
            id,
            title: name,
            artists: Vec::new(),
            year: None,
            thumbnail,
        }),
        Some("MUSIC_PAGE_TYPE_PLAYLIST") => Item::Playlist(PlaylistCard {
            id,
            title: name,
            subtitle,
            thumbnail,
        }),
        _ => Item::Other {
            title: name,
            id: Some(id),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn card(title: &str, browse_id: &str, page_type: &str) -> Value {
        json!({"musicTwoRowItemRenderer": {
            "title": {"runs": [{"text": title}]},
            "subtitle": {"runs": [{"text": "Mix"}]},
            "navigationEndpoint": {"browseEndpoint": {"browseId": browse_id,
                "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": page_type}}}}
        }})
    }

    fn track_row(video_id: &str) -> Value {
        json!({"musicResponsiveListItemRenderer": {
            "flexColumns": [{"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": format!("Title {video_id}")}]}}}],
            "playlistItemData": {"videoId": video_id}
        }})
    }

    fn continuation_item(token: &str) -> Value {
        json!({"continuationItemRenderer": {"continuationEndpoint": {"continuationCommand": {"token": token}}}})
    }

    #[test]
    fn home_first_page_and_continuation() {
        let first = json!({"contents": {"singleColumnBrowseResultsRenderer": {"tabs": [{"tabRenderer": {"content": {"sectionListRenderer": {
            "contents": [
                {"musicCarouselShelfRenderer": {
                    "header": {"musicCarouselShelfBasicHeaderRenderer": {"title": {"runs": [{"text": "Fresh finds"}]},
                        "moreContentButton": {"buttonRenderer": {"navigationEndpoint": {"browseEndpoint": {"browseId": "FEmore"}}}}}},
                    "contents": [card("Discover Mix", "VLRDTMAK5uy_d", "MUSIC_PAGE_TYPE_PLAYLIST"),
                                 card("New Release Mix", "VLRDTMAK5uy_n", "MUSIC_PAGE_TYPE_PLAYLIST")]}},
                {"chipCloudRenderer": {}}
            ],
            "continuations": [{"nextContinuationData": {"continuation": "page2"}}]
        }}}}]}}});
        let page = parse_home(&first);
        assert_eq!(page.continuation.as_deref(), Some("page2"));
        assert_eq!(page.shelves.len(), 1);
        assert_eq!(page.shelves[0].title, "Fresh finds");
        assert_eq!(page.shelves[0].more.as_deref(), Some("FEmore"));
        assert_eq!(page.shelves[0].items.len(), 2);

        let next = json!({"continuationContents": {"sectionListContinuation": {"contents": [
            {"musicShelfRenderer": {"title": {"runs": [{"text": "Song mixes"}]}, "contents": [track_row("vid00000001")]}}
        ]}}});
        let page = parse_home(&next);
        assert_eq!(page.continuation, None);
        assert_eq!(page.shelves[0].title, "Song mixes");
        assert!(matches!(page.shelves[0].items[0], Item::Track(_)));

        // The empty last page.
        let last = json!({"contents": {"singleColumnBrowseResultsRenderer": {"tabs": [{"tabRenderer": {}}]}}});
        assert_eq!(
            parse_home(&last),
            HomePage {
                shelves: vec![],
                continuation: None
            }
        );
    }

    fn playlist_response(header_wrapper: Option<&str>) -> Value {
        let header = json!({"musicResponsiveHeaderRenderer": {
            "title": {"runs": [{"text": "Liked Music"}]},
            "subtitle": {"runs": [{"text": "Auto playlist"}, {"text": " • "}, {"text": "2026"}]},
            "secondSubtitle": {"runs": [{"text": "2,820 songs"}]},
            "straplineTextOne": {"runs": [{"text": "Owner", "navigationEndpoint": {"browseEndpoint": {"browseId": "UCowner"}}}]}
        }});
        let header = match header_wrapper {
            Some(wrapper) => json!({wrapper: {"header": header}}),
            None => header,
        };
        json!({"contents": {"twoColumnBrowseResultsRenderer": {
            "tabs": [{"tabRenderer": {"content": {"sectionListRenderer": {"contents": [header]}}}}],
            "secondaryContents": {"sectionListRenderer": {"contents": [{"musicPlaylistShelfRenderer": {
                "playlistId": "LM",
                "contents": [track_row("vid00000001"), track_row("vid00000002"), continuation_item("more")]
            }}]}}
        }}})
    }

    #[test]
    fn playlist_with_continuation() {
        for wrapper in [None, Some("musicEditablePlaylistDetailHeaderRenderer")] {
            let playlist = parse_playlist(&playlist_response(wrapper)).unwrap();
            assert_eq!(playlist.id, "LM");
            assert_eq!(playlist.title, "Liked Music");
            assert_eq!(playlist.subtitle, "Auto playlist • 2026");
            assert_eq!(playlist.details, "2,820 songs");
            assert_eq!(
                playlist.author.as_ref().unwrap().id.as_deref(),
                Some("UCowner")
            );
            assert_eq!(playlist.tracks.len(), 2);
            assert_eq!(playlist.continuation.as_deref(), Some("more"));
        }

        let next = json!({"onResponseReceivedActions": [{"appendContinuationItemsAction": {"continuationItems": [
            track_row("vid00000003"), continuation_item("even-more")
        ]}}]});
        let page = parse_track_page(&next);
        assert_eq!(page.tracks[0].video_id, "vid00000003");
        assert_eq!(page.continuation.as_deref(), Some("even-more"));
        let end = json!({"onResponseReceivedActions": [{"appendContinuationItemsAction": {"continuationItems": [track_row("vid00000004")]}}]});
        assert_eq!(parse_track_page(&end).continuation, None);
    }

    #[test]
    fn library_skips_the_new_playlist_button() {
        let response = json!({"contents": {"singleColumnBrowseResultsRenderer": {"tabs": [{"tabRenderer": {"content": {"sectionListRenderer": {"contents": [
            {"gridRenderer": {"items": [
                {"musicTwoRowItemRenderer": {"title": {"runs": [{"text": "New playlist"}]}, "navigationEndpoint": {"createPlaylistEndpoint": {}}}},
                card("Calm", "VLPLcalm", "MUSIC_PAGE_TYPE_PLAYLIST")
            ]}}
        ]}}}}]}}});
        let (playlists, next) = parse_library_playlists(&response);
        assert_eq!(playlists.len(), 1);
        assert_eq!(playlists[0].playlist_id(), "PLcalm");
        assert_eq!(next, None);
    }

    #[test]
    fn an_album_playlist_leads_to_its_album() {
        // As `browse` with `VLOLAK5uy_…` answers: no header, the tracks
        // link to their album in the third column.
        let response = json!({"contents": {"twoColumnBrowseResultsRenderer": {
            "tabs": [{"tabRenderer": {"content": {"sectionListRenderer": {"targetId": "VLOLAK5uy_record_header"}}}}],
            "secondaryContents": {"sectionListRenderer": {"contents": [{"musicPlaylistShelfRenderer": {
                "playlistId": "OLAK5uy_record",
                "contents": [{"musicResponsiveListItemRenderer": {
                    "flexColumns": [
                        {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Song",
                            "navigationEndpoint": {"watchEndpoint": {"videoId": "vid00000001", "playlistId": "OLAK5uy_record"}}}]}}},
                        {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Band",
                            "navigationEndpoint": {"browseEndpoint": {"browseId": "UCband"}}}]}}},
                        {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Record",
                            "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREb_record",
                                "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": "MUSIC_PAGE_TYPE_ALBUM"}}}}}]}}}
                    ],
                    "playlistItemData": {"videoId": "vid00000001"}
                }}]
            }}]}}
        }}});
        assert!(parse_playlist(&response).is_none());
        assert_eq!(
            parse_album_of_playlist(&response).as_deref(),
            Some("MPREb_record")
        );
        // A playlist of its own has no album to lead to.
        let playlist = json!({"contents": {"twoColumnBrowseResultsRenderer": {
            "secondaryContents": {"sectionListRenderer": {"contents": [{"musicPlaylistShelfRenderer": {
                "contents": [track_row("vid00000001")]}}]}}}}});
        assert_eq!(parse_album_of_playlist(&playlist), None);
    }

    #[test]
    fn album_rows_take_the_header_artist() {
        let response = json!({"contents": {"twoColumnBrowseResultsRenderer": {
            "tabs": [{"tabRenderer": {"content": {"sectionListRenderer": {"contents": [{"musicResponsiveHeaderRenderer": {
                "title": {"runs": [{"text": "Record"}]},
                "subtitle": {"runs": [{"text": "Album"}, {"text": " • "}, {"text": "1973"}]},
                "straplineTextOne": {"runs": [{"text": "Band", "navigationEndpoint": {"browseEndpoint": {"browseId": "UCband"}}}]},
                "buttons": [{"musicPlayButtonRenderer": {"playNavigationEndpoint": {"watchPlaylistEndpoint": {"playlistId": "OLAK5uy_record"}}}}]
            }}]}}}}],
            "secondaryContents": {"sectionListRenderer": {"contents": [{"musicShelfRenderer": {"contents": [track_row("vid00000001")]}}]}}
        }}});
        let album = parse_album(&response, "MPREb_record").unwrap();
        assert_eq!(album.year, Some(1973));
        assert_eq!(album.playlist_id.as_deref(), Some("OLAK5uy_record"));
        assert_eq!(album.artists[0].id.as_deref(), Some("UCband"));
        let track = &album.tracks[0];
        assert_eq!(track.artists[0].name, "Band");
        assert_eq!(
            track.album.as_ref().unwrap().id.as_deref(),
            Some("MPREb_record")
        );
    }

    #[test]
    fn artist_page() {
        let response = json!({
            "header": {"musicImmersiveHeaderRenderer": {
                "title": {"runs": [{"text": "Band"}]},
                "playButton": {"buttonRenderer": {"navigationEndpoint": {"watchEndpoint": {"playlistId": "RDAOshuffle"}}}},
                "startRadioButton": {"buttonRenderer": {"navigationEndpoint": {"watchEndpoint": {"playlistId": "RDEMradio"}}}}
            }},
            "contents": {"singleColumnBrowseResultsRenderer": {"tabs": [{"tabRenderer": {"content": {"sectionListRenderer": {"contents": [
                {"musicShelfRenderer": {"title": {"runs": [{"text": "Top songs"}]}, "contents": [track_row("vid00000001")]}},
                {"musicCarouselShelfRenderer": {"header": {"musicCarouselShelfBasicHeaderRenderer": {"title": {"runs": [{"text": "Albums"}]}}},
                    "contents": [card("Record", "MPREb_record", "MUSIC_PAGE_TYPE_ALBUM")]}},
                {"musicDescriptionShelfRenderer": {}}
            ]}}}}]}}
        });
        let artist = parse_artist(&response, "UCband").unwrap();
        assert_eq!(artist.name, "Band");
        assert_eq!(artist.shuffle_playlist.as_deref(), Some("RDAOshuffle"));
        assert_eq!(artist.radio_playlist.as_deref(), Some("RDEMradio"));
        assert_eq!(
            artist
                .shelves
                .iter()
                .map(|s| s.title.as_str())
                .collect::<Vec<_>>(),
            ["Top songs", "Albums"]
        );
    }

    fn panel_video(video_id: &str) -> Value {
        json!({"playlistPanelVideoRenderer": {"videoId": video_id, "title": {"runs": [{"text": "T"}]}}})
    }

    #[test]
    fn watch_queue_and_its_continuation() {
        let first = json!({"contents": {"singleColumnMusicWatchNextResultsRenderer": {"tabbedRenderer": {"watchNextTabbedResultsRenderer": {"tabs": [
            {"tabRenderer": {"content": {"musicQueueRenderer": {"content": {"playlistPanelRenderer": {
                "contents": [panel_video("vid00000001"), panel_video("vid00000002")],
                "continuations": [{"nextRadioContinuationData": {"continuation": "radio2"}}]
            }}}}}},
            {"tabRenderer": {"endpoint": {"browseEndpoint": {"browseId": "MPLYt_lyrics"}}}},
            {"tabRenderer": {"unselectable": true, "endpoint": {"browseEndpoint": {"browseId": "MPTRt_hidden"}}}}
        ]}}}},
            "playerOverlays": {"playerOverlayRenderer": {"actions": [{"likeButtonRenderer": {"likeStatus": "DISLIKE"}}]}}});
        let queue = parse_watch(&first);
        assert_eq!(queue.current_like, Some(crate::model::LikeStatus::Dislike));
        assert_eq!(queue.tracks.len(), 2);
        assert_eq!(queue.continuation.as_deref(), Some("radio2"));
        assert_eq!(queue.lyrics_id.as_deref(), Some("MPLYt_lyrics"));
        assert_eq!(queue.related_id, None, "an unselectable tab has no page");

        let next = json!({"continuationContents": {"playlistPanelContinuation": {
            "contents": [panel_video("vid00000003")],
            "continuations": [{"nextRadioContinuationData": {"continuation": "radio3"}}]
        }}});
        let queue = parse_watch(&next);
        assert_eq!(queue.tracks[0].video_id, "vid00000003");
        assert_eq!(queue.continuation.as_deref(), Some("radio3"));

        // A mix asked without a start track comes back without a queue.
        let empty = json!({"contents": {"singleColumnMusicWatchNextResultsRenderer": {"tabbedRenderer": {"watchNextTabbedResultsRenderer": {"tabs": [
            {"tabRenderer": {"content": {"musicQueueRenderer": {"hack": true}}}}
        ]}}}}});
        assert!(parse_watch(&empty).tracks.is_empty());
    }

    #[test]
    fn lyrics_page() {
        let response = json!({"contents": {"sectionListRenderer": {"contents": [{"musicDescriptionShelfRenderer": {
            "description": {"runs": [{"text": "line one\nline two"}]},
            "footer": {"runs": [{"text": "Source: Example"}]}
        }}]}}});
        assert_eq!(
            parse_lyrics(&response),
            Some(Lyrics {
                text: "line one\nline two".into(),
                source: Some("Source: Example".into())
            })
        );
        assert_eq!(parse_lyrics(&json!({})), None);
    }

    #[test]
    fn a_filtered_search_goes_on() {
        let shelf = |rows: Vec<Value>, next: Option<&str>| {
            let mut shelf = json!({"title": {"runs": [{"text": "Songs"}]}, "contents": rows});
            if let Some(token) = next {
                shelf["continuations"] = json!([{"nextContinuationData": {"continuation": token}}]);
            }
            shelf
        };
        let first = json!({"contents": {"tabbedSearchResultsRenderer": {"tabs": [{"tabRenderer": {"content": {"sectionListRenderer": {"contents": [
            {"musicShelfRenderer": shelf(vec![track_row("vid00000001"), track_row("vid00000002")], Some("page2"))}
        ]}}}}]}}});
        let results = parse_search(&first);
        assert_eq!(results.top, None);
        assert_eq!(results.items.len(), 2);
        assert_eq!(results.continuation.as_deref(), Some("page2"));

        let next = json!({"continuationContents": {"musicShelfContinuation":
            shelf(vec![track_row("vid00000003")], Some("page3"))}});
        let page = parse_search_next(&next);
        assert!(matches!(&page.items[..], [Item::Track(t)] if t.video_id == "vid00000003"));
        assert_eq!(page.continuation.as_deref(), Some("page3"));
        // The last page ends it.
        let last = json!({"continuationContents": {"musicShelfContinuation":
            shelf(vec![track_row("vid00000004")], None)}});
        assert_eq!(parse_search_next(&last).continuation, None);
        // Unfiltered results have no next page.
        assert_eq!(parse_search(&json!({})).continuation, None);
    }

    #[test]
    fn search_filters_ask_as_the_web_app() {
        use crate::model::SearchFilter;
        assert_eq!(SearchFilter::Songs.params(), "EgWKAQIIAWoMEA4QChADEAQQCRAF");
        assert_eq!(
            SearchFilter::Albums.params(),
            "EgWKAQIYAWoMEA4QChADEAQQCRAF"
        );
        for filter in SearchFilter::ALL {
            assert_eq!(SearchFilter::from_name(filter.name()), Some(filter));
        }
        assert_eq!(SearchFilter::from_name("podcasts"), None);
    }

    #[test]
    fn search_with_a_top_card() {
        let response = json!({"contents": {"tabbedSearchResultsRenderer": {"tabs": [{"tabRenderer": {"content": {"sectionListRenderer": {"contents": [
            {"musicCardShelfRenderer": {
                "title": {"runs": [{"text": "Band", "navigationEndpoint": {"browseEndpoint": {"browseId": "UCband",
                    "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": "MUSIC_PAGE_TYPE_ARTIST"}}}}}]},
                "subtitle": {"runs": [{"text": "Artist"}]},
                "contents": [track_row("vid00000001")]
            }},
            {"itemSectionRenderer": {"contents": [track_row("vid00000002")]}}
        ]}}}}]}}});
        let results = parse_search(&response);
        assert!(matches!(&results.top, Some(Item::Artist(a)) if a.id == "UCband"));
        assert_eq!(results.items.len(), 2);
        let Item::Track(first) = &results.items[0] else {
            panic!("not a track");
        };
        assert_eq!(first.artists[0].id.as_deref(), Some("UCband"));
    }
}
