//! The tabs: what each one shows, its queue, which one plays.
//!
//! Every tab keeps its own queue and place. One tab plays at a time; when
//! another starts playing, the place of the one before is saved, and it
//! continues from there when it plays again. "Play next" and "Add to queue"
//! go to the queue of the tab that plays.

use serde::{Deserialize, Serialize};

use crate::queue::{QueueItem, TabQueue};

/// What a tab shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TabSource {
    Home,
    Library,
    /// A playlist or a mix, by playlist ID (without `VL`).
    Playlist {
        id: String,
    },
    Album {
        id: String,
    },
    Artist {
        id: String,
    },
    Search {
        query: String,
    },
    /// The radio of a track.
    Radio {
        video_id: String,
    },
}

pub type TabId = u64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tab {
    pub id: TabId,
    pub title: String,
    pub source: TabSource,
    /// Pinned tabs cannot be closed.
    pub pinned: bool,
    pub queue: TabQueue,
}

/// What the engine should do after a change of the playing tab.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayRequest {
    pub tab: TabId,
    pub item: QueueItem,
    /// Seconds to start at: the saved place when a tab continues.
    pub start_at: f64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Tabs {
    tabs: Vec<Tab>,
    /// The tab on screen.
    pub shown: Option<TabId>,
    playing: Option<TabId>,
    next_id: TabId,
}

impl Tabs {
    /// The first start: pinned Home, the two mixes when known, and Library.
    pub fn with_defaults(discover_mix: Option<&str>, new_release_mix: Option<&str>) -> Self {
        let mut tabs = Self::default();
        let home = tabs.open("Home", TabSource::Home, true);
        tabs.open("Library", TabSource::Library, true);
        tabs.pin_mixes(discover_mix, new_release_mix);
        tabs.shown = Some(home);
        tabs
    }

    /// Closes every tab and its queue, leaving what a first start opens
    /// before it knows the mixes: Home (shown) and Library. Nothing plays;
    /// the IDs go on after the old ones.
    pub fn reset(&mut self) {
        let next_id = self.next_id;
        *self = Self {
            next_id,
            ..Self::default()
        };
        let home = self.open("Home", TabSource::Home, true);
        self.open("Library", TabSource::Library, true);
        self.shown = Some(home);
    }

    /// Pins the personal mixes right after Home, Discover Mix first, as the
    /// first start has them; the window finds them after it is shown. A mix
    /// already open stays where it is.
    pub fn pin_mixes(&mut self, discover_mix: Option<&str>, new_release_mix: Option<&str>) {
        let mut at = self
            .tabs
            .iter()
            .position(|t| t.source == TabSource::Home)
            .map_or(0, |home| home + 1);
        for (title, id) in [
            ("Discover Mix", discover_mix),
            ("New Release Mix", new_release_mix),
        ] {
            let Some(id) = id else {
                continue;
            };
            let source = TabSource::Playlist { id: id.into() };
            if self.tabs.iter().any(|t| t.source == source) {
                continue;
            }
            let tab = self.open(title, source, true);
            self.move_tab(tab, at);
            at += 1;
        }
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn get(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    pub fn get_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    pub fn playing(&self) -> Option<TabId> {
        self.playing
    }

    /// Opens a tab at the end and returns its ID. A source that is already
    /// open is not opened twice: its tab is returned.
    pub fn open(&mut self, title: &str, source: TabSource, pinned: bool) -> TabId {
        if let Some(tab) = self.tabs.iter().find(|t| t.source == source) {
            return tab.id;
        }
        self.next_id += 1;
        let id = self.next_id;
        self.tabs.push(Tab {
            id,
            title: title.to_owned(),
            source,
            pinned,
            queue: TabQueue::default(),
        });
        id
    }

    /// Closes a tab that is not pinned. Closing the playing tab leaves
    /// nothing playing; closing the shown one shows its neighbour.
    pub fn close(&mut self, id: TabId) -> bool {
        let Some(at) = self.tabs.iter().position(|t| t.id == id && !t.pinned) else {
            return false;
        };
        self.tabs.remove(at);
        if self.playing == Some(id) {
            self.playing = None;
        }
        if self.shown == Some(id) {
            self.shown = self
                .tabs
                .get(at.min(self.tabs.len().saturating_sub(1)))
                .map(|t| t.id);
        }
        true
    }

    /// Moves a tab to position `to` (clamped).
    pub fn move_tab(&mut self, id: TabId, to: usize) {
        if let Some(from) = self.tabs.iter().position(|t| t.id == id) {
            let tab = self.tabs.remove(from);
            self.tabs.insert(to.min(self.tabs.len()), tab);
        }
    }

    /// Saves the place of the tab that plays now (the engine's position).
    fn park_playing(&mut self, position: f64) {
        if let Some(playing) = self.playing
            && let Some(tab) = self.get_mut(playing)
        {
            tab.queue.position = position;
        }
    }

    /// Plays `items` in tab `id` from `start`, replacing that tab's queue.
    /// `position` is where the playing tab is now, saved for its return.
    pub fn play_list(
        &mut self,
        id: TabId,
        items: Vec<QueueItem>,
        start: usize,
        continuation: Option<String>,
        position: f64,
    ) -> Option<PlayRequest> {
        self.park_playing(position);
        let tab = self.get_mut(id)?;
        let shuffle = tab.queue.shuffle();
        let repeat = tab.queue.repeat;
        tab.queue = TabQueue::new(items, start, continuation);
        tab.queue.repeat = repeat;
        if shuffle {
            tab.queue.set_shuffle(true);
        }
        let item = tab.queue.current()?.clone();
        self.playing = Some(id);
        Some(PlayRequest {
            tab: id,
            item,
            start_at: 0.0,
        })
    }

    /// Lets tab `id` play again from its saved place.
    pub fn resume(&mut self, id: TabId, position: f64) -> Option<PlayRequest> {
        if self.playing == Some(id) {
            return None;
        }
        let tab = self.get(id)?;
        let item = tab.queue.current()?.clone();
        let start_at = tab.queue.position;
        self.park_playing(position);
        self.playing = Some(id);
        Some(PlayRequest {
            tab: id,
            item,
            start_at,
        })
    }

    /// The queue of the playing tab.
    pub fn playing_queue_mut(&mut self) -> Option<&mut TabQueue> {
        let id = self.playing?;
        self.get_mut(id).map(|t| &mut t.queue)
    }

    /// "Play next" in the playing tab; without one, in tab `fallback`.
    pub fn play_next(&mut self, items: Vec<QueueItem>, fallback: TabId) {
        let id = self.playing.unwrap_or(fallback);
        if let Some(tab) = self.get_mut(id) {
            tab.queue.insert_next(items);
        }
    }

    /// "Add to queue" in the playing tab; without one, in tab `fallback`.
    pub fn add_to_queue(&mut self, items: Vec<QueueItem>, fallback: TabId) {
        let id = self.playing.unwrap_or(fallback);
        if let Some(tab) = self.get_mut(id) {
            tab.queue.push_back(items);
        }
    }

    /// The playing tab moved on after a track ended: the request for the
    /// next track, or `None` when its queue is over (nothing plays then).
    pub fn advance(&mut self) -> Option<PlayRequest> {
        self.advance_where(|_| false)
    }

    /// As [`Tabs::advance`], past the tracks `skip` turns down.
    pub fn advance_where(&mut self, skip: impl Fn(&QueueItem) -> bool) -> Option<PlayRequest> {
        let id = self.playing?;
        let item = self.get_mut(id)?.queue.advance_where(skip).cloned();
        match item {
            Some(item) => Some(PlayRequest {
                tab: id,
                item,
                start_at: 0.0,
            }),
            None => {
                self.playing = None;
                None
            }
        }
    }

    /// Notes a new rating of `video_id` in every tab's queue; the tabs that
    /// have the track.
    pub fn set_rated(&mut self, video_id: &str, rated: crate::queue::Rated) -> Vec<TabId> {
        self.tabs
            .iter_mut()
            .filter_map(|tab| tab.queue.set_rated(video_id, rated).then_some(tab.id))
            .collect()
    }

    /// Restores the IDs counter after loading tabs from storage.
    pub(crate) fn from_parts(tabs: Vec<Tab>, shown: Option<TabId>, playing: Option<TabId>) -> Self {
        let next_id = tabs.iter().map(|t| t.id).max().unwrap_or(0);
        let known = |id: Option<TabId>| id.filter(|id| tabs.iter().any(|t| t.id == *id));
        Self {
            shown: known(shown),
            playing: known(playing),
            tabs,
            next_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::tests::items;

    fn tabs() -> (Tabs, TabId, TabId) {
        let tabs = Tabs::with_defaults(Some("RDTMAK5uy_d"), Some("RDTMAK5uy_n"));
        let discover = tabs.tabs()[1].id;
        let new_release = tabs.tabs()[2].id;
        (tabs, discover, new_release)
    }

    #[test]
    fn defaults_are_pinned_and_home_is_shown() {
        let (tabs, _, _) = tabs();
        let titles: Vec<&str> = tabs.tabs().iter().map(|t| t.title.as_str()).collect();
        assert_eq!(
            titles,
            ["Home", "Discover Mix", "New Release Mix", "Library"]
        );
        assert!(tabs.tabs().iter().all(|t| t.pinned));
        assert_eq!(tabs.shown, Some(tabs.tabs()[0].id));
        assert_eq!(Tabs::with_defaults(None, None).tabs().len(), 2);
    }

    #[test]
    fn mixes_found_later_are_pinned_after_home() {
        let mut tabs = Tabs::with_defaults(None, None);
        let home = tabs.tabs()[0].id;
        // Opened meanwhile, before the mixes came.
        tabs.open("Search: x", TabSource::Search { query: "x".into() }, false);
        tabs.pin_mixes(Some("RDTMAK5uy_d"), Some("RDTMAK5uy_n"));
        let titles: Vec<&str> = tabs.tabs().iter().map(|t| t.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Home",
                "Discover Mix",
                "New Release Mix",
                "Library",
                "Search: x"
            ]
        );
        assert!(tabs.tabs()[1].pinned && tabs.tabs()[2].pinned);
        assert_eq!(tabs.shown, Some(home));
        // Again, or with one already open: nothing doubles.
        tabs.pin_mixes(Some("RDTMAK5uy_d"), None);
        assert_eq!(tabs.tabs().len(), 5);
    }

    #[test]
    fn a_reset_leaves_home_and_library_and_nothing_playing() {
        let mut tabs = Tabs::with_defaults(Some("RDTMAK5uy_d"), None);
        let mix = tabs.tabs()[1].id;
        let search = tabs.open("Search: x", TabSource::Search { query: "x".into() }, false);
        tabs.play_list(mix, crate::queue::tests::items(&["a", "b"]), 1, None, 12.0);
        tabs.shown = Some(search);
        tabs.reset();
        let kept: Vec<(&str, bool)> = tabs
            .tabs()
            .iter()
            .map(|t| (t.title.as_str(), t.queue.is_empty()))
            .collect();
        assert_eq!(kept, [("Home", true), ("Library", true)]);
        assert_eq!(tabs.playing(), None);
        assert_eq!(tabs.shown, Some(tabs.tabs()[0].id));
        // New IDs: nothing that held an old one finds a new tab by it.
        assert!(tabs.tabs().iter().all(|t| t.id > search));
    }

    #[test]
    fn each_tab_keeps_its_place() {
        let (mut tabs, discover, new_release) = tabs();
        let request = tabs
            .play_list(discover, items(&["d1", "d2", "d3"]), 1, None, 0.0)
            .unwrap();
        assert_eq!(
            (request.item.video_id.as_str(), request.start_at),
            ("d2", 0.0)
        );

        // The other mix starts: Discover Mix keeps d2 at 73 s.
        let request = tabs
            .play_list(new_release, items(&["n1", "n2"]), 0, None, 73.0)
            .unwrap();
        assert_eq!(request.item.video_id, "n1");
        assert_eq!(tabs.playing(), Some(new_release));

        // Back to Discover Mix: d2 from 73 s; New Release Mix keeps 12 s.
        let request = tabs.resume(discover, 12.0).unwrap();
        assert_eq!(
            (request.item.video_id.as_str(), request.start_at),
            ("d2", 73.0)
        );
        assert_eq!(tabs.get(new_release).unwrap().queue.position, 12.0);
        assert_eq!(tabs.resume(discover, 0.0), None, "already playing");
    }

    #[test]
    fn queue_actions_go_to_the_playing_tab() {
        let (mut tabs, discover, new_release) = tabs();
        tabs.play_list(discover, items(&["d1", "d2"]), 0, None, 0.0);
        tabs.play_next(items(&["x"]), new_release);
        tabs.add_to_queue(items(&["z"]), new_release);
        let upcoming: Vec<&str> = tabs
            .get(discover)
            .unwrap()
            .queue
            .upcoming()
            .map(|i| i.video_id.as_str())
            .collect();
        assert_eq!(upcoming, ["x", "d2", "z"]);
        assert!(tabs.get(new_release).unwrap().queue.is_empty());
    }

    #[test]
    fn advancing_past_the_end_stops() {
        let (mut tabs, discover, _) = tabs();
        tabs.play_list(discover, items(&["d1", "d2"]), 0, None, 0.0);
        assert_eq!(tabs.advance().unwrap().item.video_id, "d2");
        assert_eq!(tabs.advance(), None);
        assert_eq!(tabs.playing(), None);
    }

    #[test]
    fn open_close_and_move() {
        let (mut tabs, discover, _) = tabs();
        let search = tabs.open(
            "pink floyd",
            TabSource::Search {
                query: "pink floyd".into(),
            },
            false,
        );
        assert_eq!(
            tabs.open(
                "again",
                TabSource::Search {
                    query: "pink floyd".into()
                },
                false
            ),
            search
        );
        tabs.shown = Some(search);
        tabs.play_list(search, items(&["s1"]), 0, None, 0.0);
        assert!(!tabs.close(discover), "pinned");
        assert!(tabs.close(search));
        assert_eq!(tabs.playing(), None);
        assert_eq!(tabs.shown, Some(tabs.tabs().last().unwrap().id));
        tabs.move_tab(discover, 0);
        assert_eq!(tabs.tabs()[0].id, discover);
    }
}
