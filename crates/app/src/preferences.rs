//! The settings as the window shows and changes them (`settings` in QML):
//! every change applies at once — to playback and downloads through the
//! backend, to the glass, to the tray — and is written to `config.toml`
//! off the GUI thread.
//!
//! The environment variables of a run (`ORBIS_BLUR=0`, `ORBIS_TRAY=0`,
//! `ORBIS_MUSIC_DIR`…) change what this run starts with, but not the file:
//! a setting changed in the window is saved as chosen, the others as they
//! were in the file.

use std::path::Path;

use orbis_core::settings::{OPACITY_RANGE, Settings};
use orbis_core::{Bitrates, Format, Template};
use qtbridge::qobject;

use crate::i18n::{tr, trf};

/// What the environment of this run changes: the settings it starts with.
pub fn with_environment(mut settings: Settings) -> Settings {
    let off = |name: &str| std::env::var(name).is_ok_and(|v| v == "0" || v == "off");
    if off("ORBIS_BLUR") {
        settings.window.glass = false;
    }
    if off("ORBIS_TRAY") {
        settings.window.tray = false;
    }
    if off("ORBIS_TRAY_UNLOAD") {
        settings.window.unload_in_tray = false;
    }
    if off("ORBIS_MUSICBRAINZ") {
        settings.downloads.musicbrainz = false;
    }
    if let Some(dir) = std::env::var_os("ORBIS_MUSIC_DIR").filter(|d| !d.is_empty()) {
        settings.downloads.folder = Some(dir.into());
    }
    if let Some(format) = std::env::var("ORBIS_DOWNLOAD_FORMAT")
        .ok()
        .and_then(|f| f.parse::<Format>().ok())
    {
        settings.downloads.format = format;
    }
    settings
}

/// Why a template is refused, in the window's words.
fn template_problem(error: &orbis_core::download::TemplateError) -> String {
    use orbis_core::download::TemplateError;
    match error {
        TemplateError::UnknownField(name) => trf("No field {%1}", &[name]),
        TemplateError::Unclosed => tr("The curly bracket is not closed").to_owned(),
        TemplateError::NoTitle => tr("The {title} field is needed").to_owned(),
        TemplateError::Empty => tr("The template is empty").to_owned(),
    }
}

/// Two names the template gives, as an example under the field: a track of
/// an album and a video without one.
fn template_example(template: &Template, format: Format) -> String {
    use orbis_core::download::{AlbumMeta, TrackMeta};
    let album = TrackMeta {
        video_id: String::new(),
        title: tr("Song").into(),
        artists: vec![tr("Artist").into()],
        album: Some(AlbumMeta {
            title: tr("Album").into(),
            artists: vec![tr("Artist").into()],
            year: Some(2024),
            track_number: Some(3),
            track_count: Some(12),
        }),
    };
    let video = TrackMeta {
        video_id: String::new(),
        title: tr("Video").into(),
        artists: vec![tr("Channel").into()],
        album: None,
    };
    let name = |meta: &TrackMeta| {
        format!(
            "{}.{}",
            template.path(meta).to_string_lossy(),
            format.extension()
        )
    };
    trf("%1\nwithout an album: %2", &[&name(&album), &name(&video)])
}

fn format_name(format: Format) -> String {
    format.extension().to_owned()
}

#[qobject(NoQmlElement, ConvertToCamelCase)]
mod preferences {
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::Arc;

    use orbis_core::Settings;
    use orbis_core::settings::Quality;
    use qtbridge::{QThreadHandle, QmlObject};
    use tokio::runtime::Handle;
    use tokio::sync::watch;

    use super::{
        Bitrates, Format, OPACITY_RANGE, Template, format_name, template_example, template_problem,
    };
    use crate::backend::Backend;
    use crate::glass::Glass;
    use crate::i18n::{self, Language};
    use crate::shell::Shell;

    pub struct Preferences {
        /// orbis's version, from Cargo.toml.
        version: String,
        // What the window shows: this run's settings.
        /// The window's language: `en` or `uk`.
        language: String,
        /// The words of that language by their English text, for
        /// `theme.tr` in QML.
        strings: serde_json::Value,
        quality: String,
        normalize: bool,
        skip_disliked: bool,
        hide_disliked: bool,
        download_format: String,
        download_folder: String,
        /// The folder is the desktop's music folder, not a chosen one.
        folder_is_default: bool,
        template: String,
        /// Why the template last typed is refused, or empty.
        template_error: String,
        template_example: String,
        mp3_bitrate: i32,
        m4a_bitrate: i32,
        musicbrainz: bool,
        glass_on: bool,
        /// Percent.
        opacity: i32,
        tray: bool,
        unload_in_tray: bool,
        /// Why the settings could not be read or written, or empty.
        problem: String,

        /// What is in the file plus the changes made in the window.
        stored: Settings,
        /// What this run uses: `stored` with the environment's changes.
        shown: Settings,
        saver: Option<watch::Sender<Settings>>,
        backend: Arc<QThreadHandle<Backend>>,
        glass: Arc<QThreadHandle<Glass>>,
        shell: Arc<QThreadHandle<Shell>>,
    }

    impl Preferences {
        qproperty!("version", Member = version, Notify = changed);
        qproperty!("language", Member = language, Notify = language_changed);
        qproperty!("strings", Member = strings, Notify = language_changed);
        qproperty!("quality", Member = quality, Notify = changed);
        qproperty!("normalize", Member = normalize, Notify = changed);
        qproperty!("skipDisliked", Member = skip_disliked, Notify = changed);
        qproperty!("hideDisliked", Member = hide_disliked, Notify = changed);
        qproperty!("downloadFormat", Member = download_format, Notify = changed);
        qproperty!("downloadFolder", Member = download_folder, Notify = changed);
        qproperty!(
            "folderIsDefault",
            Member = folder_is_default,
            Notify = changed
        );
        qproperty!("template", Member = template, Notify = changed);
        qproperty!("templateError", Member = template_error, Notify = changed);
        qproperty!(
            "templateExample",
            Member = template_example,
            Notify = changed
        );
        qproperty!("mp3Bitrate", Member = mp3_bitrate, Notify = changed);
        qproperty!("m4aBitrate", Member = m4a_bitrate, Notify = changed);
        qproperty!("musicbrainz", Member = musicbrainz, Notify = changed);
        qproperty!("glassOn", Member = glass_on, Notify = changed);
        qproperty!("opacity", Member = opacity, Notify = changed);
        qproperty!("tray", Member = tray, Notify = changed);
        qproperty!("unloadInTray", Member = unload_in_tray, Notify = changed);
        qproperty!("problem", Member = problem, Notify = changed);

        #[qsignal]
        fn changed(&mut self);
        #[qsignal]
        fn language_changed(&mut self);
        /// Asks the dialog to show its tab `index` (smoke test, for the
        /// bench's pictures).
        #[qsignal]
        fn tab_requested(&mut self, index: i32);

        /// `stored` is what the file holds (`problem` says why it could not
        /// be read), `shown` what this run starts with; changes are written
        /// to `path`.
        #[allow(clippy::too_many_arguments)]
        pub fn create(
            stored: Settings,
            shown: Settings,
            path: PathBuf,
            problem: String,
            runtime: Handle,
            backend: Arc<QThreadHandle<Backend>>,
            glass: Arc<QThreadHandle<Glass>>,
            shell: Arc<QThreadHandle<Shell>>,
        ) -> Rc<RefCell<Self>> {
            let language = Language::from_code(&shown.language);
            i18n::set_language(language);
            let preferences = Rc::new(RefCell::new(Self {
                version: env!("CARGO_PKG_VERSION").to_owned(),
                language: language.code().to_owned(),
                strings: i18n::strings(language),
                quality: String::new(),
                normalize: false,
                skip_disliked: false,
                hide_disliked: false,
                download_format: String::new(),
                download_folder: String::new(),
                folder_is_default: true,
                template: String::new(),
                template_error: String::new(),
                template_example: String::new(),
                mp3_bitrate: 0,
                m4a_bitrate: 0,
                musicbrainz: false,
                glass_on: false,
                opacity: 0,
                tray: false,
                unload_in_tray: false,
                problem,
                stored: stored.clone(),
                shown,
                saver: None,
                backend,
                glass,
                shell,
            }));
            Self::attach_qobject(&preferences);
            let me = Arc::new(preferences.borrow().get_thread_handle());
            let (saver, mut wanted) = watch::channel(stored);
            runtime.spawn(async move {
                while wanted.changed().await.is_ok() {
                    let settings = wanted.borrow_and_update().clone();
                    let path = path.clone();
                    let saved = tokio::task::spawn_blocking(move || settings.save(&path))
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r.map_err(|e| e.to_string()));
                    let problem = saved.err().unwrap_or_default();
                    me.invoke_mut(move |p| p.set_problem(problem));
                }
            });
            {
                let mut this = preferences.borrow_mut();
                this.saver = Some(saver);
                this.show();
            }
            preferences
        }

        /// The settings this run uses and why the last write failed (the
        /// smoke test reads them).
        pub fn current(&self) -> (Settings, String, String) {
            (
                self.shown.clone(),
                self.template_error.clone(),
                self.problem.clone(),
            )
        }

        /// Shows the dialog's tab `index` (smoke test).
        pub fn show_tab(&mut self, index: i32) {
            self.tab_requested(index);
        }

        fn set_problem(&mut self, problem: String) {
            if problem != self.problem {
                self.problem = problem;
                self.changed();
            }
        }

        /// Copies the settings into the properties.
        fn show(&mut self) {
            let shown = &self.shown;
            self.quality = shown.playback.quality.name().to_owned();
            self.normalize = shown.playback.normalize;
            self.skip_disliked = shown.playback.skip_disliked;
            self.hide_disliked = shown.playback.hide_disliked;
            let downloads = &shown.downloads;
            self.download_format = format_name(downloads.format);
            self.download_folder = downloads.folder().to_string_lossy().into_owned();
            self.folder_is_default = downloads.folder.is_none();
            self.template = downloads.template.clone();
            self.template_example = template_example(&downloads.template(), downloads.format);
            self.mp3_bitrate = downloads.mp3_bitrate as i32;
            self.m4a_bitrate = downloads.m4a_bitrate as i32;
            self.musicbrainz = downloads.musicbrainz;
            self.glass_on = shown.window.glass;
            self.opacity = i32::from(shown.window.opacity);
            self.tray = shown.window.tray;
            self.unload_in_tray = shown.window.unload_in_tray;
            self.changed();
        }

        /// Changes a setting in this run and in the file, shows it and
        /// saves it.
        fn edit(&mut self, change: impl Fn(&mut Settings)) {
            change(&mut self.stored);
            change(&mut self.shown);
            if let Some(saver) = &self.saver {
                saver.send_replace(self.stored.clone());
            }
            self.show();
        }

        /// Gives playback and downloads their settings as they are now.
        fn apply_to_backend(&self) {
            let (playback, downloads) = (self.shown.playback.clone(), self.shown.downloads.clone());
            self.backend
                .invoke_mut(move |b| b.apply_settings(playback, downloads));
        }

        // ---- playback -----------------------------------------------------

        /// `best`, `normal` or `economy`.
        #[qslot]
        pub fn set_quality(&mut self, quality: String) {
            if let Some(quality) = Quality::from_name(&quality) {
                self.edit(|s| s.playback.quality = quality);
                self.apply_to_backend();
            }
        }

        #[qslot]
        pub fn set_normalize(&mut self, on: bool) {
            self.edit(|s| s.playback.normalize = on);
            self.apply_to_backend();
        }

        #[qslot]
        pub fn set_skip_disliked(&mut self, on: bool) {
            self.edit(|s| s.playback.skip_disliked = on);
            self.apply_to_backend();
        }

        #[qslot]
        pub fn set_hide_disliked(&mut self, on: bool) {
            self.edit(|s| s.playback.hide_disliked = on);
            self.apply_to_backend();
        }

        // ---- interface ----------------------------------------------------

        /// `en` or `uk`: the window, the tray's menu and new messages
        /// speak it at once.
        #[qslot]
        pub fn set_language(&mut self, code: String) {
            let language = Language::from_code(&code);
            if language.code() == self.language {
                return;
            }
            i18n::set_language(language);
            self.edit(|s| s.language = language.code().to_owned());
            self.language = language.code().to_owned();
            self.strings = i18n::strings(language);
            // Words made in Rust from the settings: the template's example.
            self.show();
            self.language_changed();
            self.backend.invoke_mut(|b| b.refresh_tabs());
            self.shell.invoke_mut(|s| s.retranslate());
        }

        // ---- downloads ----------------------------------------------------

        /// `opus`, `mp3` or `m4a`.
        #[qslot]
        pub fn set_download_format(&mut self, format: String) {
            if let Ok(format) = format.parse::<Format>() {
                self.edit(|s| s.downloads.format = format);
                self.apply_to_backend();
            }
        }

        /// The music folder, as a path (QML passes a chosen folder's).
        #[qslot]
        pub fn set_download_folder(&mut self, folder: String) {
            let folder = PathBuf::from(folder.trim());
            if folder.is_absolute() {
                self.edit(|s| s.downloads.folder = Some(folder.clone()));
                self.apply_to_backend();
            }
        }

        /// Back to the desktop's music folder.
        #[qslot]
        pub fn reset_download_folder(&mut self) {
            self.edit(|s| s.downloads.folder = None);
            self.apply_to_backend();
        }

        /// The name template as typed: taken when it is valid, else the
        /// error says why and the last valid one stays.
        #[qslot]
        pub fn set_template(&mut self, text: String) {
            match Template::parse(&text) {
                Ok(template) => {
                    self.template_error.clear();
                    let text = template.to_string();
                    self.edit(|s| s.downloads.template = text.clone());
                    self.apply_to_backend();
                }
                Err(e) => {
                    self.template_error = template_problem(&e);
                    self.changed();
                }
            }
        }

        #[qslot]
        pub fn reset_template(&mut self) {
            self.set_template(Template::default().to_string());
        }

        #[qslot]
        pub fn set_mp3_bitrate(&mut self, kbps: i32) {
            let rate = Bitrates::new(kbps.max(0) as u32, 0).mp3;
            self.edit(|s| s.downloads.mp3_bitrate = rate);
            self.apply_to_backend();
        }

        #[qslot]
        pub fn set_m4a_bitrate(&mut self, kbps: i32) {
            let rate = Bitrates::new(0, kbps.max(0) as u32).m4a;
            self.edit(|s| s.downloads.m4a_bitrate = rate);
            self.apply_to_backend();
        }

        #[qslot]
        pub fn set_musicbrainz(&mut self, on: bool) {
            self.edit(|s| s.downloads.musicbrainz = on);
            self.apply_to_backend();
        }

        // ---- window and tray ----------------------------------------------

        #[qslot]
        pub fn set_glass(&mut self, on: bool) {
            self.edit(|s| s.window.glass = on);
            self.glass.invoke_mut(move |g| g.set_wanted(on));
        }

        /// The ground's opacity, percent (40–90).
        #[qslot]
        pub fn set_opacity(&mut self, percent: i32) {
            let percent = percent.clamp(
                i32::from(*OPACITY_RANGE.start()),
                i32::from(*OPACITY_RANGE.end()),
            ) as u8;
            if percent == self.shown.window.opacity {
                return;
            }
            self.edit(|s| s.window.opacity = percent);
            let opacity = f64::from(percent) / 100.0;
            self.glass.invoke_mut(move |g| g.set_opacity(opacity));
        }

        #[qslot]
        pub fn set_tray(&mut self, on: bool) {
            self.edit(|s| s.window.tray = on);
            self.shell.invoke_mut(move |s| s.set_tray(on));
        }

        #[qslot]
        pub fn set_unload_in_tray(&mut self, on: bool) {
            self.edit(|s| s.window.unload_in_tray = on);
            self.shell.invoke_mut(move |s| s.set_unload_hidden(on));
        }
    }
}

pub use preferences::Preferences;

/// The settings file's path and its settings: the defaults, with the
/// reason, when it cannot be read.
pub fn load(path: &Path) -> (Settings, String) {
    match Settings::load(path) {
        Ok(settings) => (settings, String::new()),
        Err(e) => {
            eprintln!("orbis: {e}");
            (
                Settings::default(),
                trf(
                    "The settings were not read (%1); changes will overwrite the file.",
                    &[&e],
                ),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use orbis_core::settings::Quality;

    use super::*;

    #[test]
    fn the_example_shows_both_kinds_of_names() {
        assert_eq!(
            template_example(&Template::default(), Format::Opus),
            "Artist/2024 - Album/03 - Song.opus\nwithout an album: Channel/Video.opus"
        );
        let flat = Template::parse("{artist} - {title}").unwrap();
        assert_eq!(
            template_example(&flat, Format::Mp3),
            "Artist - Song.mp3\nwithout an album: Channel - Video.mp3"
        );
    }

    #[test]
    fn quality_names_are_the_settings_names() {
        for quality in Quality::ALL {
            assert_eq!(Quality::from_name(quality.name()), Some(quality));
        }
    }
}
