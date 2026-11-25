//! What the API gives, as rows for QML: cards of carousels, shelves and
//! queue items.

use player::QueueItem;
use serde_json::json;
use ytm::{Item, Link, SearchResults, Shelf, TrackKind};

use crate::i18n::tr;
use crate::models::{ShelfRow, format_duration};

/// How QML lays a shelf out.
pub(crate) const CAROUSEL: &str = "carousel";
/// Tracks one under another, as rows.
pub(crate) const LIST: &str = "list";

pub(crate) fn names(links: &[Link]) -> String {
    links
        .iter()
        .map(|l| l.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// `a • b`, leaving out the empty parts.
pub(crate) fn joined(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" • ")
}

/// A card for QML; podcasts and other pages the player does not open are
/// left out. Tracks also carry their length (`duration`, such as `3:36`).
pub(crate) fn card(item: &Item) -> Option<serde_json::Value> {
    let mut duration = String::new();
    let (kind, id, title, subtitle, thumbnail) = match item {
        Item::Playlist(c) => (
            "playlist",
            c.playlist_id().to_owned(),
            c.title.clone(),
            c.subtitle.clone(),
            c.thumbnail.clone(),
        ),
        Item::Album(c) => {
            let year = c.year.map(|y| y.to_string()).unwrap_or_default();
            let subtitle = joined(&[&names(&c.artists), &year]);
            (
                "album",
                c.id.clone(),
                c.title.clone(),
                subtitle,
                c.thumbnail.clone(),
            )
        }
        Item::Artist(c) => (
            "artist",
            c.id.clone(),
            c.name.clone(),
            c.subtitle.clone(),
            c.thumbnail.clone(),
        ),
        Item::Track(t) => {
            let album = t.album.as_ref().map(|a| a.name.as_str()).unwrap_or("");
            let artists = names(&t.artists);
            let subtitle = if artists.is_empty() {
                album.to_owned()
            } else {
                artists
            };
            duration = format_duration(t.duration);
            (
                "track",
                t.video_id.clone(),
                t.title.clone(),
                subtitle,
                t.thumbnail.clone(),
            )
        }
        Item::Other { .. } => return None,
    };
    let thumbnail = thumbnail.unwrap_or_default();
    // Video and playlist pictures from i.ytimg.com are 16:9.
    let wide = thumbnail.starts_with("https://i.ytimg.com/");
    Some(json!({
        "kind": kind,
        "id": id,
        "title": title,
        "subtitle": subtitle,
        "thumbnail": thumbnail,
        "wide": wide,
        "duration": duration,
    }))
}

/// Shelves as rows of cards; shelves left without cards are dropped.
pub(crate) fn shelf_rows(shelves: &[Shelf]) -> Vec<ShelfRow> {
    shelves
        .iter()
        .filter_map(|shelf| {
            let cards: Vec<_> = shelf.items.iter().filter_map(card).collect();
            (!cards.is_empty()).then(|| ShelfRow {
                title: shelf.title.clone(),
                layout: CAROUSEL.into(),
                cards,
            })
        })
        .collect()
}

/// Search results as shelves. YouTube Music answers with the top result and
/// one mixed list; it is grouped by kind, groups in the order their first
/// item came, songs and videos as rows, the rest as carousels.
pub(crate) fn search_rows(results: &SearchResults) -> Vec<ShelfRow> {
    let mut rows = Vec::new();
    if let Some(top) = results.top.as_ref().and_then(card) {
        rows.push(ShelfRow {
            title: tr("Top result").into(),
            layout: CAROUSEL.into(),
            cards: vec![top],
        });
    }
    // (title, shown as rows, cards)
    let mut groups: Vec<(&str, bool, Vec<serde_json::Value>)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in &results.items {
        let (group, rows) = match item {
            Item::Track(t) => match t.kind {
                TrackKind::Song => (tr("Songs"), true),
                TrackKind::Video => (tr("Videos"), true),
                TrackKind::Episode => (tr("Episodes"), true),
            },
            Item::Album(_) => (tr("Albums"), false),
            Item::Artist(_) => (tr("Artists"), false),
            Item::Playlist(_) => (tr("Playlists"), false),
            Item::Other { .. } => continue,
        };
        let Some(card) = card(item) else {
            continue;
        };
        // The same track comes back under another title now and then.
        if !seen.insert((card["kind"].to_string(), card["id"].to_string())) {
            continue;
        }
        match groups.iter_mut().find(|(title, _, _)| *title == group) {
            Some((_, _, cards)) => cards.push(card),
            None => groups.push((group, rows, vec![card])),
        }
    }
    rows.extend(groups.into_iter().map(|(title, as_rows, cards)| ShelfRow {
        title: title.into(),
        layout: if as_rows { LIST } else { CAROUSEL }.into(),
        cards,
    }));
    rows
}

/// A page of a filtered search as one shelf of rows under `title` (empty
/// for the pages after the first, which go on under it).
pub(crate) fn search_page_rows(results: &SearchResults, title: &str) -> Vec<ShelfRow> {
    let cards: Vec<_> = results.items.iter().filter_map(card).collect();
    if cards.is_empty() {
        return Vec::new();
    }
    vec![ShelfRow {
        title: title.into(),
        layout: LIST.into(),
        cards,
    }]
}

/// The heading of a filtered search's results.
pub(crate) fn filter_title(filter: ytm::SearchFilter) -> &'static str {
    match filter {
        ytm::SearchFilter::Songs => tr("Songs"),
        ytm::SearchFilter::Videos => tr("Videos"),
        ytm::SearchFilter::Albums => tr("Albums"),
        ytm::SearchFilter::Artists => tr("Artists"),
    }
}

pub(crate) fn queue_items(tracks: &[ytm::Track]) -> Vec<QueueItem> {
    tracks
        .iter()
        .filter(|t| t.available)
        .map(orbis_core::item_from_track)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ytm::{AlbumCard, Track, TrackKind};

    fn link(name: &str, id: &str) -> Link {
        Link {
            name: name.into(),
            id: Some(id.into()),
        }
    }

    pub(crate) fn track(video_id: &str, kind: TrackKind) -> Track {
        Track {
            video_id: video_id.into(),
            title: format!("Song {video_id}"),
            artists: vec![link("Band", "UCband")],
            album: Some(link("Record", "MPREb_record")),
            duration: Some(216),
            thumbnail: Some("https://lh3.googleusercontent.com/x=w120-h120-l90-rj".into()),
            like: None,
            set_video_id: None,
            kind,
            available: true,
            explicit: false,
        }
    }

    #[test]
    fn cards_carry_what_a_click_needs() {
        let album = Item::Album(AlbumCard {
            id: "MPREb_1".into(),
            title: "Record".into(),
            artists: vec![link("Band", "UCband"), link("Guest", "UCguest")],
            year: Some(1991),
            thumbnail: None,
        });
        let card = card(&album).expect("album card");
        assert_eq!(card["kind"], "album");
        assert_eq!(card["id"], "MPREb_1");
        assert_eq!(card["subtitle"], "Band, Guest • 1991");
        assert_eq!(card["wide"], false);

        let mut video = track("v1", TrackKind::Video);
        video.thumbnail = Some("https://i.ytimg.com/vi/v1/hq720.jpg".into());
        let card = super::card(&Item::Track(video)).expect("track card");
        assert_eq!(
            (card["kind"].as_str(), card["id"].as_str()),
            (Some("track"), Some("v1"))
        );
        assert_eq!(card["subtitle"], "Band");
        assert_eq!(card["wide"], true);

        let other = Item::Other {
            title: "A podcast".into(),
            id: None,
        };
        assert_eq!(super::card(&other), None);
    }

    #[test]
    fn shelves_without_cards_are_dropped() {
        let shelves = [
            Shelf {
                title: "Podcasts".into(),
                items: vec![Item::Other {
                    title: "x".into(),
                    id: None,
                }],
                more: None,
            },
            Shelf {
                title: "Quick picks".into(),
                items: vec![Item::Track(track("a", TrackKind::Song))],
                more: None,
            },
        ];
        let rows = shelf_rows(&shelves);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Quick picks");
        assert_eq!(rows[0].layout, CAROUSEL);
        assert_eq!(rows[0].cards.len(), 1);
        assert_eq!(rows[0].cards[0]["duration"], "3:36");
    }

    #[test]
    fn search_results_are_grouped_by_kind_in_order() {
        let artist = |id: &str| {
            Item::Artist(ytm::ArtistCard {
                id: id.into(),
                name: id.into(),
                subtitle: String::new(),
                thumbnail: None,
            })
        };
        let results = SearchResults {
            top: Some(artist("UCtop")),
            items: vec![
                Item::Track(track("s1", TrackKind::Song)),
                Item::Track(track("v1", TrackKind::Video)),
                artist("UCother"),
                Item::Track(track("s2", TrackKind::Song)),
                // The same song again.
                Item::Track(track("s1", TrackKind::Song)),
                Item::Other {
                    title: "profile".into(),
                    id: None,
                },
            ],
            continuation: None,
        };
        let rows = search_rows(&results);
        let shape: Vec<(&str, &str, Vec<&str>)> = rows
            .iter()
            .map(|r| {
                let ids = r.cards.iter().filter_map(|c| c["id"].as_str()).collect();
                (r.title.as_str(), r.layout.as_str(), ids)
            })
            .collect();
        assert_eq!(
            shape,
            [
                ("Top result", CAROUSEL, vec!["UCtop"]),
                ("Songs", LIST, vec!["s1", "s2"]),
                ("Videos", LIST, vec!["v1"]),
                ("Artists", CAROUSEL, vec!["UCother"]),
            ]
        );

        // A page of a filtered search: one list, whatever its items are.
        let page = SearchResults {
            top: None,
            items: vec![artist("UCa"), artist("UCb")],
            continuation: Some("next".into()),
        };
        let rows = search_page_rows(&page, filter_title(ytm::SearchFilter::Artists));
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].title.as_str(), rows[0].layout.as_str()),
            ("Artists", LIST)
        );
        assert_eq!(rows[0].cards.len(), 2);
        // The pages after it go on without a title; an empty one adds no row.
        assert_eq!(search_page_rows(&page, "")[0].title, "");
        let empty = SearchResults {
            top: None,
            items: Vec::new(),
            continuation: None,
        };
        assert!(search_page_rows(&empty, "").is_empty());
    }
}
