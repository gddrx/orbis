//! The glass look: the compositor blurs what is behind the window and the
//! window lets it through; over it lies the current track's cover, blurred
//! (made by the core once per track), and the cover gives the accent color.
//! Without the blur — another compositor, KWin's blur effect turned off, the
//! glass turned off in the settings, or `ORBIS_BLUR=0` — the window is
//! opaque, with the same cover. How much of the dark ground covers the blur
//! is a setting too.

use qtbridge::qobject;

#[qobject(NoQmlElement, ConvertToCamelCase)]
mod glass {
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::Arc;

    use orbis_core::covers::file_url;
    use orbis_core::{CoverCache, backdrop};
    use qtbridge::{QThreadHandle, QmlObject};
    use tokio::runtime::Handle;

    use crate::window_system;

    /// The cover a backdrop is made from, in pixels: the size the player
    /// bar shows it at on a 1.5 screen, so both use one download.
    const COVER_PX: u32 = 120;

    pub struct Glass {
        /// The blur may be asked for: the glass is on.
        wanted: bool,
        /// How much of the ground covers the blur, from 0 to 1.
        opacity: f64,
        /// The blur was asked for.
        asked: bool,
        /// The compositor blurs behind the window.
        blur: bool,
        /// The `file://` URL of the current cover's backdrop, or empty.
        backdrop: String,
        /// `#rrggbb` from the current cover, or empty for the default.
        accent: String,

        cache: Option<Arc<CoverCache>>,
        /// Where backdrops are kept.
        dir: PathBuf,
        runtime: Handle,
        me: Option<Arc<QThreadHandle<Glass>>>,
        /// The cover URL shown, so a track with the same cover changes
        /// nothing.
        cover: String,
        /// Counts the covers asked for: a backdrop made for an older one is
        /// dropped.
        generation: u64,

        /// The smoke test counts the frames the window draws; QML reports
        /// them only while this is set.
        counting_frames: bool,
        frames: u64,
    }

    impl Glass {
        qproperty!("blur", Member = blur, Notify = blur_changed);
        qproperty!("backdrop", Member = backdrop, Notify = backdrop_changed);
        qproperty!("accent", Member = accent, Notify = backdrop_changed);
        qproperty!("opacity", Member = opacity, Notify = opacity_changed);
        qproperty!(
            "countingFrames",
            Member = counting_frames,
            Notify = counting_changed
        );

        #[qsignal]
        fn blur_changed(&mut self);
        #[qsignal]
        fn backdrop_changed(&mut self);
        #[qsignal]
        fn opacity_changed(&mut self);
        #[qsignal]
        fn counting_changed(&mut self);

        /// `wanted` is false when the window must stay opaque; `cache` is
        /// `None` when covers are not shown; backdrops are kept in `dir`.
        pub fn create(
            wanted: bool,
            opacity: f64,
            cache: Option<Arc<CoverCache>>,
            dir: PathBuf,
            runtime: Handle,
        ) -> Rc<RefCell<Self>> {
            let glass = Rc::new(RefCell::new(Self {
                wanted,
                opacity,
                asked: false,
                blur: false,
                backdrop: String::new(),
                accent: String::new(),
                cache,
                dir,
                runtime,
                me: None,
                cover: String::new(),
                generation: 0,
                counting_frames: false,
                frames: 0,
            }));
            Self::attach_qobject(&glass);
            let handle = Arc::new(glass.borrow().get_thread_handle());
            glass.borrow_mut().me = Some(handle);
            glass
        }

        /// Shows `url`'s cover behind the window: its backdrop is made on
        /// tokio, then `backdrop` and `accent` change. An empty URL takes
        /// them away.
        #[qslot]
        pub fn show_cover(&mut self, url: String) {
            if url == self.cover {
                return;
            }
            self.cover = url.clone();
            self.generation += 1;
            let generation = self.generation;
            let (Some(cache), Some(me)) = (self.cache.clone(), self.me.clone()) else {
                return;
            };
            if url.is_empty() {
                self.set_backdrop(String::new(), String::new());
                return;
            }
            let dir = self.dir.clone();
            self.runtime.spawn(async move {
                let made = match cache.fetch(&url, COVER_PX, COVER_PX).await {
                    Ok(cover) => tokio::task::spawn_blocking(move || {
                        backdrop::make(&cover, &dir).map_err(|e| e.to_string())
                    })
                    .await
                    .unwrap_or_else(|e| Err(e.to_string())),
                    Err(e) => Err(e.to_string()),
                };
                me.invoke_mut(move |glass| {
                    if glass.generation != generation {
                        return;
                    }
                    match made {
                        Ok(made) => {
                            let accent = made
                                .accent
                                .map(|[r, g, b]| format!("#{r:02x}{g:02x}{b:02x}"))
                                .unwrap_or_default();
                            glass.set_backdrop(file_url(&made.file), accent);
                        }
                        Err(e) => {
                            ytm::netlog::log(format_args!("backdrop failed: {e}"));
                            glass.set_backdrop(String::new(), String::new());
                        }
                    }
                });
            });
        }

        /// The window drew a frame (reported only while counting).
        #[qslot]
        pub fn count_frame(&mut self) {
            self.frames += 1;
        }

        /// Starts counting the window's frames from zero (smoke test).
        pub fn start_counting(&mut self) {
            self.frames = 0;
            if !self.counting_frames {
                self.counting_frames = true;
                self.counting_changed();
            }
        }

        /// The frames drawn since counting started.
        pub fn frames(&self) -> u64 {
            self.frames
        }

        /// Stops counting and tells how many frames were drawn.
        pub fn stop_counting(&mut self) -> u64 {
            if self.counting_frames {
                self.counting_frames = false;
                self.counting_changed();
            }
            self.frames
        }

        fn set_backdrop(&mut self, backdrop: String, accent: String) {
            if backdrop != self.backdrop || accent != self.accent {
                self.backdrop = backdrop;
                self.accent = accent;
                self.backdrop_changed();
            }
        }

        /// Asks the compositor for the blur behind the window. QML calls it
        /// once the window exists; whether it works is known a moment later
        /// (`checkBlur`).
        #[qslot]
        pub fn enable_blur(&mut self) {
            if self.wanted {
                window_system::blur_behind(true);
                self.asked = true;
            }
        }

        /// Reads again whether the compositor blurs behind the window: it
        /// can tell only after a round trip, and the user may turn the
        /// effect off and on.
        #[qslot]
        pub fn check_blur(&mut self) {
            let blur = self.asked && window_system::blur_available();
            if blur != self.blur {
                self.blur = blur;
                self.blur_changed();
            }
        }

        /// Turns the glass on or off (the settings): the blur is asked for
        /// or taken away at once.
        pub fn set_wanted(&mut self, wanted: bool) {
            if wanted == self.wanted {
                return;
            }
            self.wanted = wanted;
            if wanted {
                self.enable_blur();
            } else if self.asked {
                window_system::blur_behind(false);
                self.asked = false;
            }
            self.check_blur();
        }

        /// The glass wanted, the blur done and the ground's opacity (smoke
        /// test).
        pub fn looks(&self) -> (bool, bool, f64) {
            (self.wanted, self.blur, self.opacity)
        }

        /// The ground's opacity, from 0 to 1 (the settings).
        pub fn set_opacity(&mut self, opacity: f64) {
            if opacity != self.opacity {
                self.opacity = opacity;
                self.opacity_changed();
            }
        }
    }
}

pub use glass::Glass;
