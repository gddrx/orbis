//! List models for QML: tabs, tracks, cards, shelves and downloads. Each keeps its rows
//! in Rust and is replaced as a whole from the backend (`replace`), which QML
//! sees as a model reset, grows at the end (`append`), which keeps what QML
//! already shows, or takes new rows for the same items (`sync`), which
//! redraws only the rows that changed and keeps the list where it was
//! scrolled.

use qtbridge::{QModelItem, qobject};

/// One tab of the tab strip.
#[derive(Debug, Default, Clone, PartialEq, QModelItem)]
pub struct TabRow {
    pub tab_id: i64,
    pub title: String,
    /// `home`, `library`, `playlist`, `album`, `artist`, `search`, `radio`.
    pub kind: String,
    pub pinned: bool,
    pub playing: bool,
    pub shown: bool,
}

/// One track of a list.
#[derive(Debug, Default, Clone, PartialEq, QModelItem)]
pub struct TrackRow {
    pub video_id: String,
    pub title: String,
    pub artists: String,
    /// Such as `3:36`.
    pub duration: String,
    pub thumbnail: String,
    /// The track that plays now.
    pub current: bool,
    /// The account's rating: `like`, `dislike` or empty.
    pub rating: String,
}

/// One card of the library grid or the "add to playlist" list: a playlist or
/// a mix.
#[derive(Debug, Default, Clone, PartialEq, QModelItem)]
pub struct CardRow {
    pub title: String,
    pub subtitle: String,
    /// Playlist ID without `VL`.
    pub target_id: String,
    pub thumbnail: String,
}

/// One shelf of the home page or an artist page: a title and its cards.
#[derive(Debug, Default, Clone, PartialEq, QModelItem)]
pub struct ShelfRow {
    pub title: String,
    /// `carousel` or `list` (tracks as rows).
    pub layout: String,
    /// Cards as objects for QML: `kind` (`playlist`, `album`, `artist`,
    /// `track`), `id`, `title`, `subtitle`, `thumbnail`, `wide` (a 16:9
    /// picture). A shelf has a few dozen cards at most, so a plain array is
    /// enough for its carousel.
    pub cards: Vec<serde_json::Value>,
}

/// One download of the downloads list.
#[derive(Debug, Default, Clone, PartialEq, QModelItem)]
pub struct DownloadRow {
    pub download_id: i64,
    pub title: String,
    pub artists: String,
    /// `queued`, `active`, `done`, `skipped` (already there), `failed`,
    /// `cancelled`. Not `state`: QML items have a property of that name.
    pub stage: String,
    /// The share done, from 0 to 1.
    pub share: f64,
    /// The saved file and its folder as `file://` URLs.
    pub file: String,
    pub folder: String,
    /// Why the download failed.
    pub message: String,
}

impl DownloadRow {
    /// Still waiting or running.
    pub fn is_open(&self) -> bool {
        self.stage == "queued" || self.stage == "active"
    }
}

macro_rules! list_model {
    ($module:ident, $name:ident, $row:ty) => {
        #[qobject(Base = QListModel, NoQmlElement)]
        mod $module {
            use std::cell::RefCell;
            use std::rc::Rc;

            use qtbridge::{QListModel, QListModelBase, QmlObject};

            use super::*;

            #[derive(Debug, Default)]
            pub struct $name {
                rows: Vec<$row>,
                incoming: Vec<$row>,
                count: i32,
            }

            impl $name {
                qproperty!("count", Member = count, Notify = count_changed);

                #[qsignal]
                fn count_changed(&mut self);

                pub fn create() -> Rc<RefCell<Self>> {
                    let model = Rc::new(RefCell::new(Self::default()));
                    Self::attach_qobject(&model);
                    model
                }

                /// Replaces all rows.
                pub fn replace(&mut self, rows: Vec<$row>) {
                    self.incoming = rows;
                    self.reset();
                    self.count = self.rows.len() as i32;
                    self.count_changed();
                }

                /// Adds rows at the end.
                #[allow(dead_code)]
                pub fn append(&mut self, rows: Vec<$row>) {
                    for row in rows {
                        self.push(row);
                    }
                    self.count = self.rows.len() as i32;
                    self.count_changed();
                }

                #[allow(dead_code)]
                pub fn rows(&self) -> &[$row] {
                    &self.rows
                }

                /// Replaces one row; QML redraws only it.
                #[allow(dead_code)]
                pub fn update(&mut self, index: usize, row: $row) {
                    self.set(index, row);
                }

                /// Takes `rows` without a reset when they begin with the
                /// same items as now (`same` tells): changed rows are
                /// replaced, new ones added at the end, and the list stays
                /// where it was scrolled. Otherwise replaces all rows.
                #[allow(dead_code)]
                pub fn sync(&mut self, rows: Vec<$row>, same: impl Fn(&$row, &$row) -> bool) {
                    let keeps = rows.len() >= self.rows.len()
                        && self.rows.iter().zip(&rows).all(|(old, new)| same(old, new));
                    if !keeps {
                        return self.replace(rows);
                    }
                    let mut rows = rows.into_iter();
                    for index in 0..self.rows.len() {
                        let row = rows.next().expect("as many rows as before");
                        if self.rows[index] != row {
                            self.set(index, row);
                        }
                    }
                    let rest: Vec<$row> = rows.collect();
                    if !rest.is_empty() {
                        self.append(rest);
                    }
                }
            }

            impl QListModel for $name {
                type Item = $row;

                fn len(&self) -> usize {
                    self.rows.len()
                }

                fn get(&self, index: usize) -> Option<&$row> {
                    self.rows.get(index)
                }

                fn set_unnotified(&mut self, index: usize, value: $row) -> bool {
                    match self.rows.get_mut(index) {
                        Some(row) => {
                            *row = value;
                            true
                        }
                        None => false,
                    }
                }

                fn insert_unnotified(&mut self, index: usize, value: $row) {
                    self.rows.insert(index, value);
                }

                fn reset_unnotified(&mut self) {
                    self.rows = std::mem::take(&mut self.incoming);
                }
            }
        }

        pub use $module::$name;
    };
}

list_model!(tabs_model, TabsModel, TabRow);
list_model!(tracks_model, TracksModel, TrackRow);
list_model!(cards_model, CardsModel, CardRow);
list_model!(shelves_model, ShelvesModel, ShelfRow);
list_model!(downloads_model, DownloadsModel, DownloadRow);

/// `3:36` or `1:02:03`.
pub fn format_duration(seconds: Option<u32>) -> String {
    match seconds {
        Some(s) if s >= 3600 => format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60),
        Some(s) => format!("{}:{:02}", s / 60, s % 60),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::format_duration;

    #[test]
    fn durations() {
        assert_eq!(format_duration(Some(216)), "3:36");
        assert_eq!(format_duration(Some(3723)), "1:02:03");
        assert_eq!(format_duration(None), "");
    }
}
