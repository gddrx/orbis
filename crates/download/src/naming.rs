//! Where a track goes: by a name template, by default
//! `{album_artist}/{year} - {album}/{track} - {title}` — which gives
//! `<Artist>/<Title>` for a track without an album, as an empty field drops
//! out with its separator.

use std::path::PathBuf;

/// What a track is called and where it belongs. Kept as JSON by the
/// download queue that outlives a restart.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TrackMeta {
    pub video_id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<AlbumMeta>,
}

/// The album a track is on.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AlbumMeta {
    pub title: String,
    pub artists: Vec<String>,
    pub year: Option<u16>,
    /// The track's place on the album, from 1.
    pub track_number: Option<u32>,
    pub track_count: Option<u32>,
}

/// The folder name for a track without a known artist.
const UNKNOWN_ARTIST: &str = "Unknown Artist";
/// Longest name of one path component, in bytes: file systems allow 255.
const MAX_COMPONENT: usize = 180;

/// `text` made safe as one path component on Linux, Windows and the memory
/// cards of players: no separators or characters Windows refuses, no
/// control characters, no leading dot, no trailing dot or space, not empty,
/// at most 180 bytes.
pub fn component(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_whitespace() => ' ',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.len() > MAX_COMPONENT {
        let mut end = MAX_COMPONENT;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
    }
    let out = out.trim_end_matches(['.', ' ']).to_owned();
    match out.strip_prefix('.') {
        Some(rest) => format!("_{rest}"),
        None if out.is_empty() => "_".to_owned(),
        None => out,
    }
}

/// The names joined by commas; `None` when there are none.
pub(crate) fn names(artists: &[String]) -> Option<String> {
    let joined = artists
        .iter()
        .map(|a| a.trim())
        .filter(|a| !a.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    (!joined.is_empty()).then_some(joined)
}

/// The track's path under the music folder by the default template,
/// without the extension.
pub fn relative_path(meta: &TrackMeta) -> PathBuf {
    Template::default().path(meta)
}

/// A field of a name template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    /// The album's artists, else the track's; never empty.
    AlbumArtist,
    /// The track's artists; never empty.
    Artist,
    Album,
    Year,
    /// The place on the album, two digits (three on albums of 100 tracks).
    Track,
    Title,
}

impl Field {
    const ALL: [(&'static str, Field); 6] = [
        ("album_artist", Field::AlbumArtist),
        ("artist", Field::Artist),
        ("album", Field::Album),
        ("year", Field::Year),
        ("track", Field::Track),
        ("title", Field::Title),
    ];

    fn value(self, meta: &TrackMeta) -> String {
        let artist = || names(&meta.artists).unwrap_or_else(|| UNKNOWN_ARTIST.to_owned());
        let album = meta.album.as_ref();
        match self {
            Field::AlbumArtist => album.and_then(|a| names(&a.artists)).unwrap_or_else(artist),
            Field::Artist => artist(),
            Field::Album => album.map(|a| a.title.trim().to_owned()).unwrap_or_default(),
            Field::Year => album
                .and_then(|a| a.year)
                .map(|y| y.to_string())
                .unwrap_or_default(),
            Field::Track => album
                .and_then(|a| {
                    let width = if a.track_count.unwrap_or(0) >= 100 {
                        3
                    } else {
                        2
                    };
                    a.track_number.map(|n| format!("{n:0width$}"))
                })
                .unwrap_or_default(),
            Field::Title => meta.title.clone(),
        }
    }
}

/// A piece of one folder or file name of a template.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Text(String),
    Field(Field),
}

/// Why a template cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TemplateError {
    #[error("unknown field {{{0}}}")]
    UnknownField(String),
    #[error("a {{ is not closed")]
    Unclosed,
    #[error("the template has no {{title}}")]
    NoTitle,
    #[error("the template is empty")]
    Empty,
}

/// A template of the path a track goes to under the music folder: folders
/// split by `/`, fields in braces — `{album_artist}`, `{artist}`,
/// `{album}`, `{year}`, `{track}`, `{title}` — and text between them. An
/// empty field (no album, year or number) drops out with the text that
/// joins it to its neighbour, and with brackets around it; a folder left
/// empty drops out of the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    text: String,
    folders: Vec<Vec<Piece>>,
}

/// The template orbis names tracks by unless told otherwise.
pub const DEFAULT_TEMPLATE: &str = "{album_artist}/{year} - {album}/{track} - {title}";

impl Default for Template {
    fn default() -> Self {
        Self::parse(DEFAULT_TEMPLATE).expect("the default template is valid")
    }
}

impl std::fmt::Display for Template {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl Template {
    pub fn parse(text: &str) -> Result<Self, TemplateError> {
        let mut folders = Vec::new();
        let mut title = false;
        for folder in text.split(['/', '\\']) {
            let mut pieces = Vec::new();
            let mut rest = folder;
            while let Some(open) = rest.find('{') {
                if open > 0 {
                    pieces.push(Piece::Text(rest[..open].to_owned()));
                }
                let close = rest[open..].find('}').ok_or(TemplateError::Unclosed)? + open;
                let name = rest[open + 1..close].trim();
                let field = Field::ALL
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case(name))
                    .map(|(_, f)| *f)
                    .ok_or_else(|| TemplateError::UnknownField(name.to_owned()))?;
                title |= field == Field::Title;
                pieces.push(Piece::Field(field));
                rest = &rest[close + 1..];
            }
            if !rest.is_empty() {
                pieces.push(Piece::Text(rest.to_owned()));
            }
            if pieces.iter().any(|p| {
                matches!(p, Piece::Field(_)) || matches!(p, Piece::Text(t) if !t.trim().is_empty())
            }) {
                folders.push(pieces);
            }
        }
        if folders.is_empty() {
            return Err(TemplateError::Empty);
        }
        if !title {
            return Err(TemplateError::NoTitle);
        }
        Ok(Self {
            text: text.trim().to_owned(),
            folders,
        })
    }

    /// The track's path under the music folder, without the extension.
    pub fn path(&self, meta: &TrackMeta) -> PathBuf {
        self.folders
            .iter()
            .filter_map(|pieces| render(pieces, meta))
            .collect()
    }
}

/// Opening brackets and the closing ones they pair with.
const BRACKETS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];

/// One folder or file name, or `None` when nothing of it is left.
fn render(pieces: &[Piece], meta: &TrackMeta) -> Option<String> {
    // Texts and values in order; `None` for an empty field.
    let mut parts: Vec<Option<String>> = pieces
        .iter()
        .map(|piece| match piece {
            Piece::Text(text) => Some(text.clone()),
            Piece::Field(field) => {
                let value = field_text(&field.value(meta));
                (!value.is_empty()).then_some(value)
            }
        })
        .collect();
    let is_field: Vec<bool> = pieces
        .iter()
        .map(|p| matches!(p, Piece::Field(_)))
        .collect();
    for i in 0..parts.len() {
        if !is_field[i] || parts[i].is_some() {
            continue;
        }
        let left = (0..i).rev().find(|&j| !is_field[j]).filter(|&j| j + 1 == i);
        let right = Some(i + 1).filter(|&j| j < parts.len() && !is_field[j]);
        // Brackets right around the empty field go with it.
        if let (Some(l), Some(r)) = (left, right) {
            let (lt, rt) = (
                parts[l].clone().unwrap_or_default(),
                parts[r].clone().unwrap_or_default(),
            );
            if let (Some(open), Some(close)) =
                (lt.trim_end().chars().last(), rt.trim_start().chars().next())
                && BRACKETS.contains(&(open, close))
            {
                let lt = lt.trim_end();
                parts[l] = Some(lt[..lt.len() - open.len_utf8()].to_owned());
                parts[r] = Some(rt.trim_start()[close.len_utf8()..].to_owned());
            }
        }
        // The text joining it to the next field, else to the one before.
        let field_after = is_field[i + 1..]
            .iter()
            .zip(&parts[i + 1..])
            .any(|(f, p)| *f && p.is_some());
        let field_before = is_field[..i]
            .iter()
            .zip(&parts[..i])
            .any(|(f, p)| *f && p.is_some());
        let joint = if field_after {
            right
        } else if field_before {
            left
        } else {
            None
        };
        if let Some(joint) = joint {
            parts[joint] = None;
        }
    }
    let name: String = parts.into_iter().flatten().collect();
    let name = name.trim_matches(|c: char| c.is_whitespace() || "-_.,".contains(c));
    (!name.is_empty()).then(|| component(name))
}

/// A field's value as part of a name: what `component` would replace, so
/// that a value like "AC/DC" makes no folder.
fn field_text(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' | '\\' => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, artists: &[&str], album: Option<AlbumMeta>) -> TrackMeta {
        TrackMeta {
            video_id: "v".into(),
            title: title.into(),
            artists: artists.iter().map(|a| (*a).to_owned()).collect(),
            album,
        }
    }

    #[test]
    fn an_album_track_goes_under_the_album_artist_year_and_album() {
        let album = AlbumMeta {
            title: "Point of Origin, Vol. 4".into(),
            artists: vec!["T95".into()],
            year: Some(2024),
            track_number: Some(3),
            track_count: Some(12),
        };
        let path = relative_path(&track("Set You Free", &["T95", "Operate"], Some(album)));
        assert_eq!(
            path,
            PathBuf::from("T95/2024 - Point of Origin, Vol. 4/03 - Set You Free")
        );
    }

    #[test]
    fn missing_parts_drop_out_of_the_name() {
        let album = AlbumMeta {
            title: "Live".into(),
            artists: vec![],
            year: None,
            track_number: Some(105),
            track_count: Some(120),
        };
        assert_eq!(
            relative_path(&track("Song", &["A", "B"], Some(album))),
            PathBuf::from("A, B/Live/105 - Song")
        );
        let unnumbered = AlbumMeta {
            title: "Live".into(),
            ..AlbumMeta::default()
        };
        assert_eq!(
            relative_path(&track("Song", &[], Some(unnumbered))),
            PathBuf::from("Unknown Artist/Live/Song")
        );
    }

    #[test]
    fn a_track_without_an_album_goes_under_its_artist() {
        assert_eq!(
            relative_path(&track("Live at Home", &["Channel"], None)),
            PathBuf::from("Channel/Live at Home")
        );
    }

    fn album(year: Option<u16>, number: Option<u32>) -> Option<AlbumMeta> {
        Some(AlbumMeta {
            title: "Point of Origin, Vol. 4".into(),
            artists: vec!["Various Artists".into()],
            year,
            track_number: number,
            track_count: Some(15),
        })
    }

    fn path(template: &str, meta: &TrackMeta) -> String {
        Template::parse(template)
            .unwrap()
            .path(meta)
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn templates_name_tracks() {
        let full = track(
            "Set You Free",
            &["T95", "Operate"],
            album(Some(2021), Some(10)),
        );
        assert_eq!(
            path(DEFAULT_TEMPLATE, &full),
            "Various Artists/2021 - Point of Origin, Vol. 4/10 - Set You Free"
        );
        assert_eq!(
            path("{artist}/{album} ({year})/{track}. {title}", &full),
            "T95, Operate/Point of Origin, Vol. 4 (2021)/10. Set You Free"
        );
        assert_eq!(
            path("{artist} - {title}", &full),
            "T95, Operate - Set You Free"
        );
        // Names and spaces of fields are taken loosely.
        assert_eq!(path("{ Title }", &full), "Set You Free");
    }

    #[test]
    fn empty_fields_drop_out_with_their_separators() {
        let bare = track("Set You Free", &["T95"], album(None, None));
        assert_eq!(
            path(DEFAULT_TEMPLATE, &bare),
            "Various Artists/Point of Origin, Vol. 4/Set You Free"
        );
        assert_eq!(
            path("{artist}/{album} ({year})/[{track}] {title}", &bare),
            "T95/Point of Origin, Vol. 4/Set You Free"
        );
        let video = track("Live at Home", &["Channel"], None);
        // The default template gives `<Artist>/<Title>` without an album.
        assert_eq!(path(DEFAULT_TEMPLATE, &video), "Channel/Live at Home");
        assert_eq!(
            path("{artist} - {album} - {title}", &video),
            "Channel - Live at Home"
        );
        assert_eq!(
            path("{album_artist} - {album}/{title}", &video),
            "Channel/Live at Home"
        );
        // A field's slash makes no folder.
        let slash = track("T.N.T.", &["AC/DC"], None);
        assert_eq!(path(DEFAULT_TEMPLATE, &slash), "AC_DC/T.N.T");
    }

    #[test]
    fn bad_templates_are_refused() {
        assert_eq!(
            Template::parse("{artist}/{titel}"),
            Err(TemplateError::UnknownField("titel".into()))
        );
        assert_eq!(
            Template::parse("{artist}/{title"),
            Err(TemplateError::Unclosed)
        );
        assert_eq!(
            Template::parse("{artist}/{album}"),
            Err(TemplateError::NoTitle)
        );
        assert_eq!(Template::parse(" / "), Err(TemplateError::Empty));
        assert_eq!(Template::parse(" {title} ").unwrap().to_string(), "{title}");
    }

    #[test]
    fn components_are_safe() {
        assert_eq!(component("AC/DC"), "AC_DC");
        assert_eq!(
            component("What? <Live>: \"Best\" | *"),
            "What_ _Live__ _Best_ _ _"
        );
        assert_eq!(component(".hidden"), "_hidden");
        assert_eq!(component("Vol. 2..."), "Vol. 2");
        assert_eq!(component("  a \t b\n "), "a b");
        assert_eq!(component(""), "_");
        assert_eq!(component(".."), "_");
        let long = "я".repeat(200);
        let cut = component(&long);
        assert!(cut.len() <= MAX_COMPONENT && cut.chars().all(|c| c == 'я'));
    }
}
