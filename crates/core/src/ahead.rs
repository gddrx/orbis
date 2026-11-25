//! The rules for links fetched ahead for the rows in view: which rows, how
//! many, and when to stop asking YouTube (see `Playback::prefetch_visible`).
//!
//! Every link is a `player` request to YouTube on top of those of normal
//! listening (15–20 an hour), so the rows in view are limited: the first
//! [`ROWS_PER_STOP`] rows without a link each time the list comes to rest,
//! and [`LINKS_PER_HOUR`] links an hour. When YouTube answers with a check
//! that the client is not a bot, rows in view are not fetched again in the
//! run.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use player::QueueItem;

/// Rows in view fetched each time the list comes to rest.
pub(crate) const ROWS_PER_STOP: usize = 8;
/// Links of rows in view fetched in an hour.
pub(crate) const LINKS_PER_HOUR: usize = 60;
const HOUR: Duration = Duration::from_secs(3600);

/// The video IDs to fetch among `rows` (top to bottom): those not skipped
/// and without a link at hand, at most [`ROWS_PER_STOP`].
pub(crate) fn rows_to_fetch(
    rows: &[QueueItem],
    skip: impl Fn(&QueueItem) -> bool,
    has_link: impl Fn(&str) -> bool,
) -> VecDeque<String> {
    let mut wanted = VecDeque::new();
    for row in rows {
        if wanted.len() == ROWS_PER_STOP {
            break;
        }
        if skip(row) || has_link(&row.video_id) || wanted.contains(&row.video_id) {
            continue;
        }
        wanted.push_back(row.video_id.clone());
    }
    wanted
}

/// Counts the links fetched for rows in view over the last hour.
#[derive(Debug, Default)]
pub(crate) struct Budget {
    fetched: VecDeque<Instant>,
}

impl Budget {
    /// Takes one link at `now`, unless [`LINKS_PER_HOUR`] were taken in the
    /// hour before.
    pub(crate) fn take(&mut self, now: Instant) -> bool {
        while self
            .fetched
            .front()
            .is_some_and(|at| now.duration_since(*at) >= HOUR)
        {
            self.fetched.pop_front();
        }
        if self.fetched.len() >= LINKS_PER_HOUR {
            return false;
        }
        self.fetched.push_back(now);
        true
    }
}

/// The rows in view whose links are still to fetch, top first, and the
/// limits on them. One task fetches them one by one: [`Visible::replace`]
/// says when to start it, [`Visible::next`] gives it each row.
#[derive(Debug)]
pub(crate) struct Visible {
    rows: VecDeque<String>,
    running: bool,
    budget: Budget,
    /// Off for the run: `ORBIS_PREFETCH_VISIBLE=0`, or YouTube asked
    /// whether orbis is a bot.
    off: bool,
}

impl Visible {
    pub(crate) fn new(on: bool) -> Self {
        Self {
            rows: VecDeque::new(),
            running: false,
            budget: Budget::default(),
            off: !on,
        }
    }

    /// Takes the rows of the list's new rest in place of those not fetched
    /// yet; `true` when a task must start to fetch them.
    pub(crate) fn replace(&mut self, rows: VecDeque<String>) -> bool {
        if self.off {
            return false;
        }
        self.rows = rows;
        if self.running || self.rows.is_empty() {
            return false;
        }
        self.running = true;
        true
    }

    /// The next row to fetch at `now`, past those whose link is at hand.
    /// `None` ends the task, and the rows left are dropped: none are left,
    /// rows in view are off, the hour's links are taken, or not `go_on`
    /// (the window went to the tray).
    pub(crate) fn next(
        &mut self,
        has_link: impl Fn(&str) -> bool,
        go_on: bool,
        now: Instant,
    ) -> Option<String> {
        let mut next = None;
        while let Some(video_id) = self.rows.pop_front() {
            if !has_link(&video_id) {
                next = Some(video_id);
                break;
            }
        }
        match next {
            Some(video_id) if go_on && !self.off && self.budget.take(now) => Some(video_id),
            _ => {
                self.rows.clear();
                self.running = false;
                None
            }
        }
    }

    /// Notes that fetching a row failed with `message`: a check that orbis
    /// is not a bot turns rows in view off for the run. `true` the first
    /// time, to be told.
    pub(crate) fn failed(&mut self, message: &str) -> bool {
        if self.off || !is_bot_check(message) {
            return false;
        }
        self.off = true;
        self.rows.clear();
        true
    }
}

/// Whether yt-dlp's error says YouTube wants proof that the client is not a
/// bot, or that it asks too often.
pub(crate) fn is_bot_check(message: &str) -> bool {
    let message = message.to_lowercase();
    message.contains("not a bot")
        || message.contains("http error 429")
        || message.contains("too many requests")
}

#[cfg(test)]
mod tests {
    use super::*;
    use player::Rated;

    fn row(video_id: &str, rated: Rated) -> QueueItem {
        QueueItem {
            video_id: video_id.into(),
            title: String::new(),
            artists: String::new(),
            duration: None,
            thumbnail: None,
            rated,
        }
    }

    #[test]
    fn rows_from_the_top_without_links_and_not_skipped() {
        let mut rows: Vec<QueueItem> = (0..14)
            .map(|i| row(&format!("v{i}"), Rated::None))
            .collect();
        rows[1].rated = Rated::Dislike;
        rows.insert(3, row("v2", Rated::None));
        let wanted = rows_to_fetch(
            &rows,
            |r| r.rated == Rated::Dislike,
            |id| id == "v0" || id == "v4",
        );
        assert_eq!(
            Vec::from(wanted),
            ["v2", "v3", "v5", "v6", "v7", "v8", "v9", "v10"],
            "v1 disliked, v0 and v4 ready, v2 once, eight at most"
        );
    }

    #[test]
    fn sixty_links_an_hour() {
        let start = Instant::now();
        let mut budget = Budget::default();
        for i in 0..LINKS_PER_HOUR {
            assert!(budget.take(start + Duration::from_secs(i as u64)));
        }
        assert!(!budget.take(start + Duration::from_secs(600)));
        // An hour after the first, its place is free again.
        assert!(budget.take(start + HOUR));
        assert!(!budget.take(start + HOUR));
    }

    fn ids(list: &[&str]) -> VecDeque<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn new_rows_replace_those_not_fetched_yet() {
        let now = Instant::now();
        let mut visible = Visible::new(true);
        assert!(visible.replace(ids(&["a", "b", "c"])), "a task starts");
        assert_eq!(visible.next(|_| false, true, now).as_deref(), Some("a"));
        // The list came to rest elsewhere while "a" was fetched.
        assert!(!visible.replace(ids(&["x", "y", "z"])), "the task goes on");
        assert_eq!(
            visible.next(|id| id == "x", true, now).as_deref(),
            Some("y"),
            "x has its link"
        );
        assert_eq!(visible.next(|_| false, true, now).as_deref(), Some("z"));
        assert_eq!(visible.next(|_| false, true, now), None, "the task ends");
        assert!(visible.replace(ids(&["d"])), "a new task starts");
        assert_eq!(visible.next(|_| false, false, now), None, "in the tray");
        assert!(!visible.replace(VecDeque::new()), "nothing to fetch");
    }

    #[test]
    fn the_hours_links_end_the_task() {
        let now = Instant::now();
        let mut visible = Visible::new(true);
        let mut fetched = 0;
        // Ten rests of the list, eight rows each.
        for _ in 0..10 {
            assert!(visible.replace(ids(&["a", "b", "c", "d", "e", "f", "g", "h"])));
            while visible.next(|_| false, true, now).is_some() {
                fetched += 1;
            }
        }
        assert_eq!(fetched, LINKS_PER_HOUR);
        assert!(visible.replace(ids(&["a"])));
        assert_eq!(visible.next(|_| false, true, now), None);
        assert!(visible.replace(ids(&["a"])), "and the rows are dropped");
        assert_eq!(
            visible.next(|_| false, true, now + HOUR).as_deref(),
            Some("a"),
            "an hour later"
        );
    }

    #[test]
    fn a_bot_check_turns_rows_in_view_off() {
        let now = Instant::now();
        let mut visible = Visible::new(true);
        assert!(visible.replace(ids(&["a", "b"])));
        assert_eq!(visible.next(|_| false, true, now).as_deref(), Some("a"));
        assert!(!visible.failed("ERROR: [youtube] a: Video unavailable"));
        assert!(visible.failed("ERROR: [youtube] a: Sign in to confirm you're not a bot"));
        assert!(
            !visible.failed("Sign in to confirm you're not a bot"),
            "told once"
        );
        assert!(visible.off);
        assert_eq!(visible.next(|_| false, true, now), None);
        assert!(!visible.replace(ids(&["c"])));

        let mut never = Visible::new(false);
        assert!(!never.replace(ids(&["a"])), "ORBIS_PREFETCH_VISIBLE=0");
    }

    #[test]
    fn bot_checks_are_recognized() {
        assert!(is_bot_check(
            "ERROR: [youtube] abc: Sign in to confirm you’re not a bot. Use --cookies…"
        ));
        assert!(is_bot_check(
            "ERROR: unable to download API page: HTTP Error 429: Too Many Requests"
        ));
        assert!(!is_bot_check("ERROR: [youtube] abc: Video unavailable"));
    }
}
