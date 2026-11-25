//! The queue of one tab: its tracks, the play order, the current track and
//! the place in it, shuffle and repeat.
//!
//! Tracks keep the order they were loaded in (`items`); `order` is the play
//! order over them, the identity without shuffle and a permutation with it.
//! The cursor points into `order`.

use serde::{Deserialize, Serialize};

/// How the account rated a track, as far as orbis knows: from the page the
/// track came with, or from a rating given in orbis since.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rated {
    #[default]
    None,
    Like,
    Dislike,
}

impl Rated {
    pub fn is_none(&self) -> bool {
        *self == Rated::None
    }
}

/// A track as the queue keeps it: enough to show and to play it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueItem {
    pub video_id: String,
    pub title: String,
    /// Artists joined for display.
    pub artists: String,
    /// Length in seconds.
    pub duration: Option<u32>,
    pub thumbnail: Option<String>,
    /// Queues saved before ratings were kept read as unrated.
    #[serde(default, skip_serializing_if = "Rated::is_none")]
    pub rated: Rated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Repeat {
    /// Stop after the last track.
    #[default]
    Off,
    /// Start over after the last track.
    All,
    /// Play the current track again and again.
    One,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TabQueue {
    items: Vec<QueueItem>,
    order: Vec<usize>,
    cursor: Option<usize>,
    /// Seconds into the current track, saved when another tab takes over.
    pub position: f64,
    shuffle: bool,
    pub repeat: Repeat,
    /// Token of more tracks of the source (a mix, a long playlist).
    pub continuation: Option<String>,
    seed: u64,
}

/// A small xorshift generator: shuffles need no cryptographic randomness,
/// and a stored seed makes them reproducible in tests.
fn next_random(state: &mut u64) -> u64 {
    let mut x = (*state).max(1);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

fn shuffled(mut indices: Vec<usize>, seed: &mut u64) -> Vec<usize> {
    for i in (1..indices.len()).rev() {
        let j = (next_random(seed) % (i as u64 + 1)) as usize;
        indices.swap(i, j);
    }
    indices
}

impl TabQueue {
    /// A queue of `items` that starts at `start` (an index into `items`).
    pub fn new(items: Vec<QueueItem>, start: usize, continuation: Option<String>) -> Self {
        let order = (0..items.len()).collect();
        let cursor = (start < items.len()).then_some(start);
        Self {
            items,
            order,
            cursor,
            position: 0.0,
            shuffle: false,
            repeat: Repeat::Off,
            continuation,
            seed: 0x9E37_79B9_7F4A_7C15,
        }
    }

    pub fn items(&self) -> &[QueueItem] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    /// Index into `items` of the current track.
    pub fn current_index(&self) -> Option<usize> {
        self.cursor.and_then(|c| self.order.get(c).copied())
    }

    pub fn current(&self) -> Option<&QueueItem> {
        self.current_index().map(|i| &self.items[i])
    }

    /// The tracks in play order, from the current one on.
    pub fn upcoming(&self) -> impl Iterator<Item = &QueueItem> {
        let from = self.cursor.map_or(0, |c| c + 1);
        self.order[from.min(self.order.len())..]
            .iter()
            .map(|&i| &self.items[i])
    }

    /// Where the cursor goes after the current track, by the repeat mode,
    /// past the tracks `skip` turns down (such as disliked ones). Repeating
    /// one track repeats it whatever it is.
    fn next_cursor(&self, skip: impl Fn(&QueueItem) -> bool) -> Option<usize> {
        let mut cursor = self.cursor?;
        if self.repeat == Repeat::One {
            return Some(cursor);
        }
        for _ in 0..self.order.len() {
            cursor = if cursor + 1 < self.order.len() {
                cursor + 1
            } else if self.repeat == Repeat::All {
                0
            } else {
                return None;
            };
            if !skip(&self.items[self.order[cursor]]) {
                return Some(cursor);
            }
        }
        None
    }

    /// The track that follows the current one, to resolve and enqueue ahead.
    pub fn peek_next(&self) -> Option<&QueueItem> {
        self.peek_next_where(|_| false)
    }

    /// The track that follows the current one, past those `skip` turns down.
    pub fn peek_next_where(&self, skip: impl Fn(&QueueItem) -> bool) -> Option<&QueueItem> {
        self.next_cursor(skip).map(|c| &self.items[self.order[c]])
    }

    /// The tracks after the next one, in play order (to resolve ahead).
    pub fn after_next(&self, skip: impl Fn(&QueueItem) -> bool, count: usize) -> Vec<&QueueItem> {
        let Some(next) = self.next_cursor(&skip) else {
            return Vec::new();
        };
        self.order[next + 1..]
            .iter()
            .map(|&i| &self.items[i])
            .filter(|item| !skip(item))
            .take(count)
            .collect()
    }

    /// Moves to the next track; `None` at the end of the queue.
    pub fn advance(&mut self) -> Option<&QueueItem> {
        self.advance_where(|_| false)
    }

    /// Moves to the next track past those `skip` turns down; `None` at the
    /// end of the queue.
    pub fn advance_where(&mut self, skip: impl Fn(&QueueItem) -> bool) -> Option<&QueueItem> {
        self.cursor = self.next_cursor(skip);
        self.position = 0.0;
        self.current()
    }

    /// Notes a new rating of `video_id`; whether the queue has the track.
    pub fn set_rated(&mut self, video_id: &str, rated: Rated) -> bool {
        let mut found = false;
        for item in self.items.iter_mut().filter(|i| i.video_id == video_id) {
            item.rated = rated;
            found = true;
        }
        found
    }

    /// Moves to the previous track; stays on the first one.
    pub fn back(&mut self) -> Option<&QueueItem> {
        if let Some(cursor) = self.cursor {
            self.cursor = Some(cursor.saturating_sub(1));
        }
        self.position = 0.0;
        self.current()
    }

    /// Makes `items[index]` the current track.
    pub fn jump_to(&mut self, index: usize) -> Option<&QueueItem> {
        self.cursor = self.order.iter().position(|&i| i == index);
        self.position = 0.0;
        self.current()
    }

    /// Turns shuffle on or off. The current track stays current: with
    /// shuffle it moves to the front of the new order, without it the order
    /// goes back to the loaded one at the same track.
    pub fn set_shuffle(&mut self, on: bool) {
        let current = self.current_index();
        self.shuffle = on;
        self.order = if on {
            let rest: Vec<usize> = (0..self.items.len())
                .filter(|&i| Some(i) != current)
                .collect();
            let mut order: Vec<usize> = current.into_iter().collect();
            order.extend(shuffled(rest, &mut self.seed));
            order
        } else {
            (0..self.items.len()).collect()
        };
        self.cursor = current.and_then(|c| self.order.iter().position(|&i| i == c));
    }

    /// "Play next": `items` right after the current track.
    pub fn insert_next(&mut self, items: Vec<QueueItem>) {
        let at = self.cursor.map_or(0, |c| c + 1);
        let first = self.items.len();
        self.items.extend(items);
        let added: Vec<usize> = (first..self.items.len()).collect();
        self.order.splice(at..at, added);
        if self.cursor.is_none() && !self.items.is_empty() {
            self.cursor = Some(at);
        }
    }

    /// "Add to queue": `items` at the end. With shuffle they are mixed into
    /// what is still to come.
    pub fn push_back(&mut self, items: Vec<QueueItem>) {
        let first = self.items.len();
        self.items.extend(items);
        let added: Vec<usize> = (first..self.items.len()).collect();
        if self.shuffle {
            let from = self.cursor.map_or(0, |c| c + 1);
            let mut rest: Vec<usize> = self.order.split_off(from.min(self.order.len()));
            rest.extend(added);
            self.order.extend(shuffled(rest, &mut self.seed));
        } else {
            self.order.extend(added);
        }
        if self.cursor.is_none() && !self.items.is_empty() {
            self.cursor = Some(0);
        }
    }

    /// Tracks of the next page of the source, without repeats.
    pub fn append_page(&mut self, items: Vec<QueueItem>, continuation: Option<String>) -> usize {
        let known: std::collections::HashSet<&str> =
            self.items.iter().map(|i| i.video_id.as_str()).collect();
        let fresh: Vec<QueueItem> = items
            .into_iter()
            .filter(|i| !known.contains(i.video_id.as_str()))
            .collect();
        let added = fresh.len();
        self.push_back(fresh);
        self.continuation = continuation;
        added
    }

    /// `true` when the next page should be loaded: few tracks remain and the
    /// source has more.
    pub fn wants_more(&self, remaining: usize) -> bool {
        self.continuation.is_some() && self.upcoming().count() < remaining
    }

    /// Removes `items[index]`. Removing the current track makes the next one
    /// current.
    pub fn remove(&mut self, index: usize) {
        if index >= self.items.len() {
            return;
        }
        let at = self.order.iter().position(|&i| i == index);
        self.items.remove(index);
        if let Some(at) = at {
            self.order.remove(at);
            if let Some(cursor) = self.cursor {
                if at < cursor {
                    self.cursor = Some(cursor - 1);
                } else if at == cursor {
                    self.position = 0.0;
                    if cursor >= self.order.len() {
                        self.cursor = None;
                    }
                }
            }
        }
        for i in &mut self.order {
            if *i > index {
                *i -= 1;
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn item(id: &str) -> QueueItem {
        QueueItem {
            video_id: id.into(),
            title: format!("Title {id}"),
            artists: "Artist".into(),
            duration: Some(180),
            thumbnail: None,
            rated: Rated::None,
        }
    }

    pub fn items(ids: &[&str]) -> Vec<QueueItem> {
        ids.iter().map(|id| item(id)).collect()
    }

    fn ids<'a>(it: impl Iterator<Item = &'a QueueItem>) -> Vec<&'a str> {
        it.map(|i| i.video_id.as_str()).collect()
    }

    fn current(q: &TabQueue) -> Option<&str> {
        q.current().map(|i| i.video_id.as_str())
    }

    #[test]
    fn plays_in_order_and_stops_at_the_end() {
        let mut q = TabQueue::new(items(&["a", "b", "c"]), 1, None);
        assert_eq!(current(&q), Some("b"));
        assert_eq!(q.peek_next().map(|i| i.video_id.as_str()), Some("c"));
        q.position = 42.0;
        assert_eq!(q.advance().map(|i| i.video_id.as_str()), Some("c"));
        assert_eq!(q.position, 0.0);
        assert_eq!(q.advance(), None);
        assert_eq!(current(&q), None);
    }

    #[test]
    fn repeat_modes() {
        let mut q = TabQueue::new(items(&["a", "b"]), 1, None);
        q.repeat = Repeat::All;
        assert_eq!(q.advance().map(|i| i.video_id.as_str()), Some("a"));
        q.repeat = Repeat::One;
        assert_eq!(q.advance().map(|i| i.video_id.as_str()), Some("a"));
        assert_eq!(
            q.back().map(|i| i.video_id.as_str()),
            Some("a"),
            "stays on the first"
        );
    }

    #[test]
    fn shuffle_keeps_the_current_track_and_covers_all() {
        let ids_in: Vec<String> = (0..50).map(|i| format!("t{i}")).collect();
        let refs: Vec<&str> = ids_in.iter().map(String::as_str).collect();
        let mut q = TabQueue::new(items(&refs), 10, None);
        q.set_shuffle(true);
        assert_eq!(current(&q), Some("t10"));
        let mut upcoming = ids(q.upcoming());
        assert_eq!(upcoming.len(), 49);
        assert_ne!(
            upcoming,
            refs.iter()
                .copied()
                .filter(|&i| i != "t10")
                .collect::<Vec<_>>(),
            "order changed"
        );
        upcoming.sort();
        let mut expected: Vec<&str> = refs.iter().copied().filter(|&i| i != "t10").collect();
        expected.sort();
        assert_eq!(upcoming, expected, "every other track once");

        q.advance();
        let now = current(&q).unwrap().to_owned();
        q.set_shuffle(false);
        assert_eq!(current(&q), Some(now.as_str()), "unshuffle keeps the track");
        // Back in loaded order: the next track is the one loaded after it.
        let number: usize = now[1..].parse().unwrap();
        let expected_next = (number < 49).then(|| format!("t{}", number + 1));
        assert_eq!(q.peek_next().map(|i| i.video_id.clone()), expected_next);
    }

    #[test]
    fn play_next_and_add_to_queue() {
        let mut q = TabQueue::new(items(&["a", "b", "c"]), 0, None);
        q.insert_next(items(&["x", "y"]));
        q.push_back(items(&["z"]));
        assert_eq!(ids(q.upcoming()), ["x", "y", "b", "c", "z"]);

        let mut empty = TabQueue::default();
        empty.push_back(items(&["p"]));
        assert_eq!(
            current(&empty),
            Some("p"),
            "an empty queue starts at the added track"
        );
    }

    #[test]
    fn next_pages_skip_repeats() {
        let mut q = TabQueue::new(items(&["a", "b"]), 0, Some("page2".into()));
        assert!(q.wants_more(5));
        assert_eq!(q.append_page(items(&["b", "c", "d"]), None), 2);
        assert_eq!(ids(q.upcoming()), ["b", "c", "d"]);
        assert!(!q.wants_more(5), "no continuation left");
    }

    #[test]
    fn removing_tracks_keeps_the_cursor_right() {
        let mut q = TabQueue::new(items(&["a", "b", "c", "d"]), 2, None);
        q.remove(0);
        assert_eq!(current(&q), Some("c"));
        q.remove(1); // the current one, "c"
        assert_eq!(current(&q), Some("d"));
        q.remove(1);
        assert_eq!(current(&q), None);
        assert_eq!(ids(q.items().iter()), ["b"]);
    }

    #[test]
    fn disliked_tracks_are_skipped_when_asked() {
        let mut q = TabQueue::new(items(&["a", "b", "c", "d", "e"]), 0, None);
        assert!(q.set_rated("b", Rated::Dislike));
        assert!(q.set_rated("c", Rated::Dislike));
        assert!(!q.set_rated("zz", Rated::Dislike));
        let disliked = |i: &QueueItem| i.rated == Rated::Dislike;
        assert_eq!(q.peek_next().map(|i| i.video_id.as_str()), Some("b"));
        assert_eq!(
            q.peek_next_where(disliked).map(|i| i.video_id.as_str()),
            Some("d")
        );
        assert_eq!(ids(q.after_next(disliked, 3).into_iter()), ["e"]);
        assert_eq!(
            q.advance_where(disliked).map(|i| i.video_id.as_str()),
            Some("d")
        );
        q.set_rated("e", Rated::Dislike);
        assert_eq!(
            q.advance_where(disliked),
            None,
            "nothing but disliked tracks remain"
        );

        // Around the end with repeat; one repeated track plays whatever it is.
        let mut q = TabQueue::new(items(&["a", "b", "c"]), 1, None);
        q.repeat = Repeat::All;
        q.set_rated("c", Rated::Dislike);
        assert_eq!(
            q.advance_where(disliked).map(|i| i.video_id.as_str()),
            Some("a")
        );
        q.repeat = Repeat::One;
        q.set_rated("a", Rated::Dislike);
        assert_eq!(
            q.advance_where(disliked).map(|i| i.video_id.as_str()),
            Some("a")
        );
    }
}
