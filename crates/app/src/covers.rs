//! Covers for QML: a cover's local file when the core's cache has it, else a
//! download on tokio and a signal once it is there; Qt itself loads no
//! picture from the network. QML asks only for the covers of delegates that
//! exist, that is, the ones in view, so scrolling fetches only what is seen.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use qtbridge::qobject;

/// How long a failed cover is not asked for again: long enough not to hammer
/// a server that refuses it, short enough that a minute without network does
/// not leave covers empty until orbis restarts.
const RETRY_AFTER: Duration = Duration::from_secs(5 * 60);

/// A cover as asked for: its URL and the size bucket it is shown at.
type Key = (String, u32, u32);

/// Covers whose download failed lately, with when it failed.
#[derive(Default)]
struct Failed(HashMap<Key, Instant>);

impl Failed {
    /// Whether `key` failed less than `RETRY_AFTER` before `now`.
    fn holds(&mut self, key: &Key, now: Instant) -> bool {
        match self.0.get(key) {
            Some(&at) if now.duration_since(at) < RETRY_AFTER => true,
            Some(_) => {
                self.0.remove(key);
                false
            }
            None => false,
        }
    }

    /// `key` failed at `now`; failures past their time are dropped, so the
    /// map holds only the last few minutes.
    fn insert(&mut self, key: Key, now: Instant) {
        self.0.retain(|_, at| now.duration_since(*at) < RETRY_AFTER);
        self.0.insert(key, now);
    }
}

#[qobject(NoQmlElement, ConvertToCamelCase)]
mod covers {
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::Instant;

    use orbis_core::CoverCache;
    use orbis_core::covers::{file_url, size_bucket};
    use qtbridge::{QThreadHandle, QmlObject};
    use tokio::runtime::Handle;

    use super::{Failed, Key};

    pub struct Covers {
        cache: Option<Arc<CoverCache>>,
        runtime: Handle,
        me: Option<Arc<QThreadHandle<Covers>>>,
        /// Covers being downloaded.
        pending: HashSet<Key>,
        /// Covers that failed lately: not asked for again for a few minutes.
        failed: Failed,
    }

    impl Covers {
        /// A cover asked for earlier is now in the cache.
        #[qsignal]
        fn cover_ready(&mut self, url: String);

        /// `cache` is `None` when the cache folder cannot be opened: covers
        /// are then not shown.
        pub fn create(cache: Option<Arc<CoverCache>>, runtime: Handle) -> Rc<RefCell<Self>> {
            let covers = Rc::new(RefCell::new(Self {
                cache,
                runtime,
                me: None,
                pending: HashSet::new(),
                failed: Failed::default(),
            }));
            Self::attach_qobject(&covers);
            let handle = Arc::new(covers.borrow().get_thread_handle());
            covers.borrow_mut().me = Some(handle);
            covers
        }

        /// The `file://` URL of `url` shown in a `width`×`height` box (device
        /// pixels), or an empty string while it downloads; `coverReady(url)`
        /// follows when it is there.
        #[qslot]
        pub fn request(&mut self, url: String, width: i32, height: i32) -> String {
            let (Some(cache), Some(me)) = (self.cache.clone(), self.me.clone()) else {
                return String::new();
            };
            if url.is_empty() || width <= 0 || height <= 0 {
                return String::new();
            }
            let (width, height) = (size_bucket(width as u32), size_bucket(height as u32));
            if let Some(file) = cache.cached(&url, width, height) {
                return file_url(&file);
            }
            let key = (url.clone(), width, height);
            if self.failed.holds(&key, Instant::now()) || !self.pending.insert(key.clone()) {
                return String::new();
            }
            self.runtime.spawn(async move {
                let result = cache.fetch(&url, width, height).await;
                me.invoke_mut(move |covers| {
                    covers.pending.remove(&key);
                    match result {
                        Ok(_) => covers.cover_ready(url),
                        Err(e) => {
                            ytm::netlog::log(format_args!("cover failed: {e}"));
                            covers.failed.insert(key, Instant::now());
                        }
                    }
                });
            });
            String::new()
        }
    }
}

pub use covers::Covers;

#[cfg(test)]
mod tests {
    use super::*;

    fn key(url: &str) -> Key {
        (url.to_owned(), 60, 60)
    }

    #[test]
    fn a_failed_cover_is_held_back_until_its_time_is_up() {
        let start = Instant::now();
        let mut failed = Failed::default();
        failed.insert(key("a"), start);

        assert!(failed.holds(&key("a"), start + Duration::from_secs(60)));
        assert!(
            !failed.holds(&key("b"), start),
            "another cover is asked for"
        );
        assert!(
            !failed.holds(&key("a"), start + RETRY_AFTER),
            "asked for again"
        );
        assert!(
            failed.0.is_empty(),
            "the failure is forgotten once asked again"
        );
    }

    #[test]
    fn old_failures_are_dropped_when_a_new_one_comes() {
        let start = Instant::now();
        let mut failed = Failed::default();
        failed.insert(key("a"), start);
        failed.insert(key("b"), start + RETRY_AFTER);

        assert_eq!(failed.0.len(), 1);
        assert!(failed.holds(&key("b"), start + RETRY_AFTER));
    }
}
