//! Which MusicBrainz recording and release a YouTube Music track is: names
//! compared loosely, the length within 3 seconds, the album by its title.

use std::collections::HashMap;

use super::api::{Credit, Genre, Recording, Release, SearchPage};

/// The length difference still taken for the same recording, ms.
pub(crate) const LENGTH_SLACK_MS: u64 = 3000;
/// How alike two names must be (1 is equal after [`normal`]).
const ALIKE: f64 = 0.9;
/// Genres kept: those with at least this share of the top one's votes, as
/// MusicBrainz Picard does by default.
const GENRE_SHARE: f64 = 0.9;
const MAX_GENRES: usize = 5;

/// `text` for comparing: lower case, `&` as "and", every run of what is not
/// a letter or a digit as one space.
pub(crate) fn normal(text: &str) -> String {
    let lower = text.to_lowercase().replace('&', " and ");
    lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The title without a credit of guests: "Song (feat. X)", "Song [ft. X]",
/// "Song feat. X". MusicBrainz keeps guests in the artist credit.
pub(crate) fn without_guests(title: &str) -> &str {
    let lower = title.to_lowercase();
    let cut = [
        "(feat", "[feat", "(ft.", "[ft.", "(with ", " feat. ", " ft. ",
    ]
    .iter()
    .filter_map(|marker| lower.find(marker))
    .min();
    match cut {
        // `find` on the lower-case copy: cut only where the original is a
        // char boundary too (lower case can change byte lengths).
        Some(at) if at > 0 && title.is_char_boundary(at) => title[..at].trim_end(),
        _ => title,
    }
}

fn distance(a: &[char], b: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous + usize::from(ca != cb);
            previous = row[j + 1];
            row[j + 1] = substitution.min(previous + 1).min(row[j] + 1);
        }
    }
    row[b.len()]
}

/// How alike two names are, from 0 to 1, after [`normal`].
pub(crate) fn alike(a: &str, b: &str) -> f64 {
    let (a, b) = (normal(a), normal(b));
    if a == b {
        return 1.0;
    }
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let longest = a.len().max(b.len());
    if longest == 0 {
        return 0.0;
    }
    1.0 - distance(&a, &b) as f64 / longest as f64
}

/// The track as YouTube Music knows it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Wanted {
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    /// Tracks on the album, all discs.
    pub track_count: Option<u32>,
    pub length_ms: Option<u64>,
    /// The release taken for another track of the album, kept so that all
    /// its tracks get the same one.
    pub release: Option<String>,
}

/// A release date for ordering: the earliest first, a date known only to
/// the year or month after the full dates of that period, none last.
/// (MusicBrainz sends unknown dates as `""` too.)
fn date_key(date: Option<&str>) -> String {
    match date.filter(|d| !d.is_empty()) {
        Some(date) => format!("{date:~<10}"),
        None => "~".repeat(11),
    }
}

/// Whether a release is on a digital medium, as YouTube Music's albums are.
fn is_digital(release: &Release) -> bool {
    release
        .media
        .first()
        .and_then(|m| m.format.as_deref())
        .is_some_and(|f| f == "Digital Media")
}

fn credit_names(credit: &[Credit]) -> Vec<String> {
    credit
        .iter()
        .flat_map(|c| [normal(&c.name), normal(&c.artist.name)])
        .collect()
}

fn is_live(recording: &Recording) -> bool {
    recording
        .disambiguation
        .as_deref()
        .is_some_and(|d| normal(d).split(' ').any(|w| w == "live"))
}

fn is_bootleg(release: &Release) -> bool {
    release.status.as_deref() == Some("Bootleg")
}

fn is_released(release: &Release) -> bool {
    !is_bootleg(release)
}

/// Whether `release` can be the wanted album: alike by title, not a
/// bootleg.
fn on_album(wanted: &Wanted, release: &Release) -> bool {
    wanted
        .album
        .as_deref()
        .is_some_and(|album| !is_bootleg(release) && alike(album, &release.title) >= ALIKE)
}

/// Whether `recording` can be the wanted track: the title alike, one of
/// the artists credited, the length within [`LENGTH_SLACK_MS`] when both
/// are known, not only on bootlegs.
fn fits(wanted: &Wanted, recording: &Recording) -> bool {
    let title = alike(
        without_guests(&wanted.title),
        without_guests(&recording.title),
    );
    let names = credit_names(&recording.artist_credit);
    let artist = wanted.artists.iter().any(|a| names.contains(&normal(a)));
    let length = match (wanted.length_ms, recording.length) {
        (Some(want), Some(have)) => want.abs_diff(have) <= LENGTH_SLACK_MS,
        _ => true,
    };
    let released = recording.releases.is_empty() || recording.releases.iter().any(is_released);
    title >= ALIKE && artist && length && released
}

/// A live take where a studio track is wanted: the title does not say
/// "live" and the take is not on the wanted album (a live album).
fn wrong_take(wanted: &Wanted, recording: &Recording, on_album: bool) -> bool {
    is_live(recording) && !on_album && !normal(&wanted.title).split(' ').any(|w| w == "live")
}

/// The release of `recording` that is the wanted album: alike by title, not
/// a bootleg; the one another track of the album took, then official, with
/// the album's track count, digital, worldwide, the earliest.
fn album_release<'a>(wanted: &Wanted, recording: &'a Recording) -> Option<&'a Release> {
    recording
        .releases
        .iter()
        .filter(|r| on_album(wanted, r))
        .min_by_key(|r| {
            (
                wanted.release.as_deref().is_some_and(|id| id != r.id),
                rank(wanted, r),
            )
        })
}

/// How good a release of an album is: less is better.
pub(crate) type Rank = (bool, bool, bool, bool, String);

/// How good a release of the album is, best first: official, with the
/// album's track count, digital, worldwide, the earliest.
pub(crate) fn rank(wanted: &Wanted, release: &Release) -> Rank {
    (
        release.status.as_deref() != Some("Official"),
        wanted.track_count.is_some() && release.track_count != wanted.track_count,
        !is_digital(release),
        release.country.as_deref() != Some("XW"),
        date_key(release.date.as_deref()),
    )
}

/// The wanted track among search results, with its release when the album
/// is found. Of several that fit: the one on the album, then the one on the
/// release another track of the album took (so that an album keeps one
/// release, even when its first track fits only a remaster's recording),
/// then with a known length, then the first released, then the one on most
/// releases (the canonical recording rather than a duplicate).
pub(crate) fn pick<'a>(
    wanted: &Wanted,
    page: &'a SearchPage,
) -> Option<(&'a Recording, Option<&'a Release>)> {
    page.recordings
        .iter()
        .filter(|r| fits(wanted, r))
        .map(|r| (r, album_release(wanted, r)))
        .filter(|(r, release)| !wrong_take(wanted, r, release.is_some()))
        // A recording of unknown length is taken only on the wanted album.
        .filter(|(r, release)| {
            r.length.is_some() || wanted.length_ms.is_none() || release.is_some()
        })
        .min_by_key(|(r, release)| {
            let on_taken = release
                .zip(wanted.release.as_deref())
                .is_some_and(|(release, taken)| release.id == taken);
            (
                release.is_none(),
                wanted.release.is_some() && !on_taken,
                r.length.is_none(),
                date_key(r.first_release_date.as_deref()),
                std::cmp::Reverse(r.release_count()),
                std::cmp::Reverse(r.score.unwrap_or(0)),
            )
        })
}

/// The releases of the album kept for a recording: the one another track
/// of the album took, then the best by [`rank`].
const KEPT_RELEASES: usize = 3;

/// Cuts a search page down to what [`pick`] reads for `wanted`: the
/// recordings that can be the track, and of their releases the best
/// [`KEPT_RELEASES`] that can be the wanted album, else one that says
/// whether the recording came out at all (not only on bootlegs); how many
/// releases it is on is kept apart.
/// Popular songs bring hundreds of recordings and releases: the cache keeps
/// this copy, under the wanted album and artists (the title and length are
/// in the query).
pub(crate) fn trim(page: &mut SearchPage, wanted: &Wanted) {
    page.recordings.retain(|r| fits(wanted, r));
    for recording in &mut page.recordings {
        let count = recording.releases.len();
        let mut kept: Vec<Release> = recording
            .releases
            .iter()
            .filter(|r| on_album(wanted, r))
            .cloned()
            .collect();
        kept.sort_by_cached_key(|r| {
            (
                wanted.release.as_deref().is_some_and(|id| id != r.id),
                rank(wanted, r),
            )
        });
        kept.truncate(KEPT_RELEASES);
        if kept.is_empty()
            && let Some(any) = recording
                .releases
                .iter()
                .find(|r| is_released(r))
                .or(recording.releases.first())
        {
            kept.push(any.clone());
        }
        if kept.len() < count {
            recording.release_count = Some(recording.release_count.unwrap_or(count));
            recording.releases = kept;
        }
    }
}

/// The genres to write: the votes of the recording and its release group
/// added up, or the artists' when those have none; the top ones within
/// [`GENRE_SHARE`] of the most voted, at most [`MAX_GENRES`].
pub(crate) fn genres<'a>(
    own: impl IntoIterator<Item = &'a Genre>,
    artists: impl IntoIterator<Item = &'a Genre>,
) -> Vec<String> {
    let add = |genres: &mut HashMap<String, u32>, genre: &Genre| {
        *genres.entry(genre.name.clone()).or_default() += genre.count.max(1);
    };
    let mut votes = HashMap::new();
    for genre in own {
        add(&mut votes, genre);
    }
    if votes.is_empty() {
        for genre in artists {
            add(&mut votes, genre);
        }
    }
    let Some(top) = votes.values().copied().max() else {
        return Vec::new();
    };
    let mut kept: Vec<(String, u32)> = votes
        .into_iter()
        .filter(|(_, count)| f64::from(*count) >= GENRE_SHARE * f64::from(top))
        .collect();
    kept.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    kept.into_iter()
        .take(MAX_GENRES)
        .map(|(name, _)| name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::api::{Artist, Medium};

    fn release(id: &str, format: &str, country: &str, date: &str) -> Release {
        Release {
            id: id.into(),
            title: "The Dark Side of the Moon".into(),
            status: Some("Official".into()),
            date: Some(date.into()),
            country: Some(country.into()),
            count: Some(1),
            track_count: Some(10),
            media: vec![Medium {
                position: Some(1),
                format: Some(format.into()),
                track_count: Some(10),
                track_offset: Some(1),
                track: Vec::new(),
            }],
            ..Release::default()
        }
    }

    fn recording(id: &str, first: &str, releases: Vec<Release>) -> Recording {
        Recording {
            id: id.into(),
            title: "Breathe (In the Air)".into(),
            length: Some(169_000),
            first_release_date: Some(first.into()),
            artist_credit: vec![Credit {
                name: "Pink Floyd".into(),
                artist: Artist {
                    id: "pf".into(),
                    name: "Pink Floyd".into(),
                    genres: Vec::new(),
                },
            }],
            releases,
            ..Recording::default()
        }
    }

    #[test]
    fn an_album_keeps_the_release_its_first_track_took() {
        // The original recording on the digital release, and a remaster's
        // only on a Blu-ray, behind three better releases.
        let page = SearchPage {
            recordings: vec![
                recording(
                    "original",
                    "1973-03-01",
                    vec![release("digital", "Digital Media", "XW", "2016")],
                ),
                recording(
                    "remaster",
                    "2023-03-24",
                    vec![
                        release("bluray", "Blu-ray", "US", "2023-10-13"),
                        release("d1", "Digital Media", "XW", "2023-03-24"),
                        release("d2", "Digital Media", "XW", "2023-03-25"),
                        release("d3", "Digital Media", "XW", "2023-03-26"),
                    ],
                ),
            ],
        };
        let mut wanted = Wanted {
            title: "Breathe (In the Air)".into(),
            artists: vec!["Pink Floyd".into()],
            album: Some("The Dark Side of the Moon".into()),
            track_count: Some(10),
            length_ms: Some(169_500),
            release: None,
        };
        let picked = |wanted: &Wanted, page: &SearchPage| {
            pick(wanted, page).map(|(r, release)| (r.id.clone(), release.map(|r| r.id.clone())))
        };
        // On its own: the first released recording.
        assert_eq!(
            picked(&wanted, &page),
            Some(("original".into(), Some("digital".into())))
        );
        // The first track of the album took the Blu-ray: so does this one.
        wanted.release = Some("bluray".into());
        assert_eq!(
            picked(&wanted, &page),
            Some(("remaster".into(), Some("bluray".into())))
        );
        // The page as the cache keeps it for that album still has it.
        let mut kept = page.clone();
        trim(&mut kept, &wanted);
        assert!(kept.recordings[1].releases.iter().any(|r| r.id == "bluray"));
        assert_eq!(kept.recordings[1].release_count(), 4);
        assert_eq!(picked(&wanted, &kept), picked(&wanted, &page));
    }

    #[test]
    fn names_are_compared_loosely() {
        assert_eq!(normal("  T95 & Operate! "), "t95 and operate");
        assert_eq!(normal("Don’t Stop — Live"), "don t stop live");
        assert_eq!(alike("Set You Free", "Set you free"), 1.0);
        assert!(alike("Point of Origin, Vol. 4", "Point of Origin Vol 4") > 0.99);
        assert!(alike("Circuits", "Circuit") >= ALIKE - 0.05);
        assert!(alike("Hunter", "Daybreak") < 0.5);
        assert_eq!(alike("", ""), 1.0);
        assert_eq!(normal("Обійми"), "обійми");
    }

    #[test]
    fn guests_leave_the_title() {
        assert_eq!(
            without_guests("Land of the Lost Souls (feat. Krissy Twigge)"),
            "Land of the Lost Souls"
        );
        assert_eq!(without_guests("Song [ft. Someone]"), "Song");
        assert_eq!(without_guests("Song feat. Someone"), "Song");
        assert_eq!(without_guests("Feather"), "Feather");
        assert_eq!(without_guests("(feat. X)"), "(feat. X)");
    }

    fn genre(name: &str, count: u32) -> Genre {
        Genre {
            name: name.into(),
            count,
        }
    }

    #[test]
    fn the_most_voted_genres_are_kept() {
        let own = [
            genre("rock", 10),
            genre("progressive rock", 9),
            genre("art rock", 3),
        ];
        // The release group's votes add up with the recording's.
        let rg = [genre("progressive rock", 1)];
        assert_eq!(
            genres(own.iter().chain(rg.iter()), []),
            ["progressive rock", "rock"]
        );
        assert_eq!(genres(own.iter(), []), ["rock", "progressive rock"]);
        // Without the track's own genres, the artists' are taken.
        let artists = [genre("drum and bass", 2), genre("electronic", 1)];
        assert_eq!(genres([], artists.iter()), ["drum and bass"]);
        assert!(genres([], []).is_empty());
        let many: Vec<Genre> = (0..8).map(|i| genre(&format!("g{i}"), 5)).collect();
        assert_eq!(genres(many.iter(), []).len(), MAX_GENRES);
    }
}
