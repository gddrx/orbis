# MusicBrainz answers for the tests

Recorded on 23 November 2026 from the cache of real downloads
(`orbis-cli download`, cache folder `musicbrainz/`): each file is a request
orbis made and MusicBrainz's answer, trimmed to the fields orbis reads —
exactly what the cache keeps. `tests.rs` serves them from a local server.

- `*.search.json` — recording searches (`/ws/2/recording?query=…`);
- `*.recording.json` — recording lookups with genres and artists;
- `release_group.<id>.json` — release group lookups with genres.

Data from [MusicBrainz](https://musicbrainz.org). Its core data (names,
IDs, releases, lengths) is in the public domain (CC0); genres are
supplementary data under CC BY-NC-SA 3.0.

To record them again: download the tracks of `tests.rs` with
`ORBIS_CACHE_DIR` set to an empty folder and copy the cache files that
answer its requests here.
