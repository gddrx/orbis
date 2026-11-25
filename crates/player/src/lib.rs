//! Playback on libmpv and the per-tab queues: every tab keeps its own queue and
//! position, and one tab plays at a time. Does not depend on Qt.

pub mod engine;
mod ffi;
pub mod mpv;
pub mod queue;
pub mod store;
pub mod tabs;

pub use engine::{Engine, EngineOptions, Media, PlayerEvent, normalization_gain};
pub use mpv::{EndReason, MpvError};
pub use queue::{QueueItem, Rated, Repeat, TabQueue};
pub use store::{Store, StoreError};
pub use tabs::{PlayRequest, Tab, TabId, TabSource, Tabs};
