# Changelog

All notable changes to orbis are written down here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html): until 1.0.0 a
minor version may change anything.

## [Unreleased]

## [0.1.0] - 2026-11-25

The first version: a YouTube Music player for the Linux desktop without a
web engine in its window.

### Added

- Sign-in in a Google window of orbis's own, or with the cookies of a
  browser where YouTube Music is open (Chrome, Chromium, Brave, Vivaldi,
  Firefox). The session is kept in the desktop's secret storage.
- The home feed, the personal mixes (Discover Mix, New Release Mix, My
  Supermix, Replay Mix), the library, playlists, albums, artists and search
  with suggestions.
- Tabs, each with a queue of its own, kept across restarts; radio of a
  track; mixes that go on without end.
- Playback on libmpv with three stream qualities and loudness
  normalization; tracks the account disliked can be skipped or hidden.
- Like, dislike and adding to a playlist, including a new one.
- Downloads of a track, an album or a whole playlist as Opus, MP3 or M4A,
  with tags, cover, lyrics and loudness gain, refined by MusicBrainz; the
  download queue comes back after a restart.
- Desktop integration: tray, MPRIS (media keys, the Plasma player widget,
  KDE Connect), one instance, the blur of KWin behind the window.
- An English and a Ukrainian interface.
- `orbis-cli`, the same core from the command line.
- `orbis --version` and the version in the settings window.

[Unreleased]: https://github.com/gddrx/orbis/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/gddrx/orbis/releases/tag/v0.1.0
