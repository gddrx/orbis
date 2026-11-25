//! Reading YouTube Music: the home feed and its mixes, playlists, the
//! library, albums, artists and search.

use serde_json::{Map, Value, json};

use crate::client::{Client, Error};
use crate::model::{
    Album, Artist, HomePage, Item, Mix, Playlist, PlaylistCard, SearchFilter, SearchResults,
    TrackPage,
};
use crate::parse::pages;

/// Upper bound on pages fetched by the `*_all` helpers, against endless loops.
const MAX_PAGES: usize = 100;

fn body(pairs: &[(&str, &str)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), json!(v)))
        .collect()
}

/// A playlist's browse ID: its ID with `VL` in front.
fn playlist_browse_id(id: &str) -> String {
    if id.starts_with("VL") {
        id.to_owned()
    } else {
        format!("VL{id}")
    }
}

/// An album's own playlist (`OLAK5uy_…`, with or without `VL`): it has no
/// page of its own — [`Client::playlist`] fails on it — and its tracks are
/// read from the album, found with [`Client::album_of_playlist`].
pub fn is_album_playlist(id: &str) -> bool {
    id.strip_prefix("VL").unwrap_or(id).starts_with("OLAK5uy_")
}

impl Client {
    /// The first page of the home feed.
    pub async fn home(&self) -> Result<HomePage, Error> {
        self.home_in(None).await
    }

    async fn home_in(&self, language: Option<&str>) -> Result<HomePage, Error> {
        let response = self
            .post_in("browse", body(&[("browseId", "FEmusic_home")]), language)
            .await?;
        Ok(pages::parse_home(&response))
    }

    /// The home page after `continuation`.
    pub async fn home_next(&self, continuation: &str) -> Result<HomePage, Error> {
        self.home_next_in(continuation, None).await
    }

    async fn home_next_in(
        &self,
        continuation: &str,
        language: Option<&str>,
    ) -> Result<HomePage, Error> {
        let response = self
            .post_in("browse", body(&[("continuation", continuation)]), language)
            .await?;
        Ok(pages::parse_home(&response))
    }

    /// All pages of the home feed.
    pub async fn home_all(&self) -> Result<Vec<HomePage>, Error> {
        self.home_all_in(None).await
    }

    async fn home_all_in(&self, language: Option<&str>) -> Result<Vec<HomePage>, Error> {
        let mut pages = vec![self.home_in(language).await?];
        while let Some(token) = pages.last().and_then(|p| p.continuation.clone()) {
            if pages.len() >= MAX_PAGES {
                break;
            }
            pages.push(self.home_next_in(&token, language).await?);
        }
        Ok(pages)
    }

    /// The personal mixes on the home feed, asked for in English, in which
    /// their titles name them (see [`find_mixes`]): in another interface
    /// language they are called otherwise.
    pub async fn personal_mixes(&self) -> Result<Vec<(Mix, PlaylistCard)>, Error> {
        Ok(find_mixes(&self.home_all_in(Some("en")).await?))
    }

    /// A playlist with its first page of tracks. `id` may carry the `VL`
    /// browse prefix or not.
    pub async fn playlist(&self, id: &str) -> Result<Playlist, Error> {
        let browse_id = playlist_browse_id(id);
        let response = self
            .post("browse", body(&[("browseId", &browse_id)]))
            .await?;
        pages::parse_playlist(&response)
            .ok_or_else(|| Error::Unexpected(format!("no playlist header for {browse_id}")))
    }

    /// The album (`MPREb_…`) of an album's own playlist (`OLAK5uy_…`,
    /// see [`is_album_playlist`]).
    pub async fn album_of_playlist(&self, id: &str) -> Result<String, Error> {
        let browse_id = playlist_browse_id(id);
        let response = self
            .post("browse", body(&[("browseId", &browse_id)]))
            .await?;
        pages::parse_album_of_playlist(&response)
            .ok_or_else(|| Error::Unexpected(format!("no album for {browse_id}")))
    }

    /// The next page of a playlist's tracks.
    pub async fn playlist_next(&self, continuation: &str) -> Result<TrackPage, Error> {
        let response = self
            .post("browse", body(&[("continuation", continuation)]))
            .await?;
        Ok(pages::parse_track_page(&response))
    }

    /// A playlist with all its tracks.
    pub async fn playlist_all(&self, id: &str) -> Result<Playlist, Error> {
        let mut playlist = self.playlist(id).await?;
        let mut pages = 1;
        while let Some(token) = playlist.continuation.take() {
            if pages >= MAX_PAGES {
                break;
            }
            let page = self.playlist_next(&token).await?;
            playlist.tracks.extend(page.tracks);
            playlist.continuation = page.continuation;
            pages += 1;
        }
        Ok(playlist)
    }

    /// The playlists saved in the library.
    pub async fn library_playlists(&self) -> Result<Vec<PlaylistCard>, Error> {
        let response = self
            .post("browse", body(&[("browseId", "FEmusic_liked_playlists")]))
            .await?;
        let (mut playlists, mut token) = pages::parse_library_playlists(&response);
        let mut pages = 1;
        while let Some(next) = token.take() {
            if pages >= MAX_PAGES {
                break;
            }
            let response = self
                .post("browse", body(&[("continuation", &next)]))
                .await?;
            let (more, next) = pages::parse_library_playlists(&response);
            playlists.extend(more);
            token = next;
            pages += 1;
        }
        Ok(playlists)
    }

    /// An album with its tracks.
    pub async fn album(&self, id: &str) -> Result<Album, Error> {
        let response = self.post("browse", body(&[("browseId", id)])).await?;
        pages::parse_album(&response, id)
            .ok_or_else(|| Error::Unexpected(format!("no album header for {id}")))
    }

    /// An artist page.
    pub async fn artist(&self, id: &str) -> Result<Artist, Error> {
        let response = self.post("browse", body(&[("browseId", id)])).await?;
        pages::parse_artist(&response, id)
            .ok_or_else(|| Error::Unexpected(format!("no artist header for {id}")))
    }

    /// Searches everything.
    pub async fn search(&self, query: &str) -> Result<SearchResults, Error> {
        self.search_in(query, None).await
    }

    /// Searches for `query`, only for the kind `filter` when given: then 20
    /// results a page, and [`Client::search_next`] brings the next.
    pub async fn search_in(
        &self,
        query: &str,
        filter: Option<SearchFilter>,
    ) -> Result<SearchResults, Error> {
        let params = filter.map(SearchFilter::params);
        let mut fields = vec![("query", query)];
        if let Some(params) = &params {
            fields.push(("params", params));
        }
        let response = self.post("search", body(&fields)).await?;
        Ok(pages::parse_search(&response))
    }

    /// The page of a filtered search after `continuation`.
    pub async fn search_next(&self, continuation: &str) -> Result<SearchResults, Error> {
        let response = self
            .post("search", body(&[("continuation", continuation)]))
            .await?;
        Ok(pages::parse_search_next(&response))
    }
}

/// The personal mixes on the home feed, by their English titles.
///
/// The titles are what YouTube Music shows with `hl=en`; with another
/// interface language they differ: [`Client::personal_mixes`] asks for the
/// home feed in English, and the caller keeps the IDs it found once.
pub fn find_mixes(pages: &[HomePage]) -> Vec<(Mix, PlaylistCard)> {
    let mut found = Vec::new();
    for mix in Mix::ALL {
        let card = pages
            .iter()
            .flat_map(|p| &p.shelves)
            .flat_map(|s| &s.items)
            .find_map(|item| match item {
                Item::Playlist(card) if card.title == mix.english_title() => Some(card.clone()),
                _ => None,
            });
        if let Some(card) = card {
            found.push((mix, card));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Shelf;

    #[test]
    fn album_playlists_are_told_apart() {
        assert!(is_album_playlist("OLAK5uy_record"));
        assert!(is_album_playlist("VLOLAK5uy_record"));
        assert!(!is_album_playlist("PLcalm"));
        // A playlist YouTube Music puts together, not an album's.
        assert!(!is_album_playlist("RDCLAK5uy_hits"));
    }

    #[test]
    fn mixes_are_found_by_title() {
        let card = |title: &str, id: &str| {
            Item::Playlist(PlaylistCard {
                id: id.into(),
                title: title.into(),
                subtitle: String::new(),
                thumbnail: None,
            })
        };
        let pages = vec![
            HomePage {
                shelves: vec![Shelf {
                    title: "Fresh finds".into(),
                    items: vec![
                        card("New Release Mix", "VLRDTMAK5uy_n"),
                        card("Liked Music", "VLLM"),
                    ],
                    more: None,
                }],
                continuation: Some("next".into()),
            },
            HomePage {
                shelves: vec![Shelf {
                    title: "Mixed for you".into(),
                    items: vec![card("Discover Mix", "VLRDTMAK5uy_d")],
                    more: None,
                }],
                continuation: None,
            },
        ];
        let found: Vec<(Mix, String)> = find_mixes(&pages)
            .into_iter()
            .map(|(mix, card)| (mix, card.id))
            .collect();
        assert_eq!(
            found,
            [
                (Mix::Discover, "VLRDTMAK5uy_d".to_owned()),
                (Mix::NewRelease, "VLRDTMAK5uy_n".to_owned())
            ]
        );
    }
}
