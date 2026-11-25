//! The window's words in the chosen language. English is the source: Rust
//! and QML write the English text, and [`CATALOG`] gives its Ukrainian.
//! Rust asks [`tr`] and [`trf`]; QML asks `theme.tr`, which looks
//! the text up in [`strings`] (the settings object hands it to QML).
//! Placeholders are `%1`, `%2`… — as QML's `String.arg` has them.
//!
//! A text missing from the catalog shows in English; a test checks that
//! every `tr("…")` of the Rust and QML sources is in it.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

/// A language of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Language {
    #[default]
    English,
    Ukrainian,
}

impl Language {
    /// The code in `config.toml` (`orbis_core::settings::LANGUAGES`).
    pub fn code(self) -> &'static str {
        match self {
            Language::English => "en",
            Language::Ukrainian => "uk",
        }
    }

    /// The language of `code`; English for an unknown one.
    pub fn from_code(code: &str) -> Self {
        match code {
            "uk" => Language::Ukrainian,
            _ => Language::English,
        }
    }

    fn index(self) -> u8 {
        self as u8
    }

    fn from_index(index: u8) -> Self {
        match index {
            1 => Language::Ukrainian,
            _ => Language::English,
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// The language texts are given in from now on; the Google sign-in window
/// speaks it too.
pub fn set_language(language: Language) {
    CURRENT.store(language.index(), Ordering::Relaxed);
    desktop::login_window::set_language(language.code());
}

pub fn language() -> Language {
    Language::from_index(CURRENT.load(Ordering::Relaxed))
}

fn catalog() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| CATALOG.iter().copied().collect())
}

/// `text` in `language`.
pub fn tr_in(language: Language, text: &'static str) -> &'static str {
    let Some(&uk) = catalog().get(text) else {
        return text;
    };
    match language {
        Language::English => text,
        Language::Ukrainian => uk,
    }
}

/// `text` in the current language.
pub fn tr(text: &'static str) -> &'static str {
    tr_in(language(), text)
}

/// `text` in the current language with `%1`, `%2`… replaced by `args`.
pub fn trf(text: &'static str, args: &[&dyn std::fmt::Display]) -> String {
    trf_in(language(), text, args)
}

/// `text` in `language` with `%1`, `%2`… replaced by `args`.
pub fn trf_in(language: Language, text: &'static str, args: &[&dyn std::fmt::Display]) -> String {
    let mut out = tr_in(language, text).to_owned();
    // From the last, so that `%1` does not eat the start of `%10`.
    for (i, arg) in args.iter().enumerate().rev() {
        out = out.replace(&format!("%{}", i + 1), &arg.to_string());
    }
    out
}

/// Every text of the catalog in `language`, by its English text, for QML;
/// empty for English.
pub fn strings(language: Language) -> serde_json::Value {
    let map = match language {
        Language::English => serde_json::Map::new(),
        _ => CATALOG
            .iter()
            .map(|&(en, _)| (en.to_owned(), tr_in(language, en).into()))
            .collect(),
    };
    serde_json::Value::Object(map)
}

/// English, Ukrainian.
#[rustfmt::skip]
const CATALOG: &[(&str, &str)] = &[
    // ---- the window: tabs, list, player bar ----
    ("Settings", "Налаштування"),
    ("Search", "Пошук"),
    ("Close", "Закрити"),
    ("Previous", "Попередній"),
    ("Pause", "Пауза"),
    ("Play", "Грати"),
    ("Next", "Наступний"),
    ("Like", "Подобається"),
    ("Remove from liked", "Прибрати з уподобаних"),
    ("Dislike", "Не подобається"),
    ("Remove dislike", "Прибрати «Не подобається»"),
    ("Add to playlist", "Додати до плейлиста"),
    ("Track menu", "Меню треку"),
    ("Downloads", "Завантаження"),
    ("Mute", "Вимкнути звук"),
    ("Unmute", "Увімкнути звук"),
    ("Not signed in to YouTube Music.", "Немає входу в YouTube Music."),
    ("Sign in…", "Увійти…"),
    ("Download all", "Завантажити все"),
    ("Reading the list…", "Читаю список…"),
    ("Download the whole list?", "Завантажити весь список?"),
    ("Tracks in “%1”: %2. Downloading them takes a long time and much disk space.",
     "Треків у «%1»: %2. Завантаження займе багато часу й місця на диску."),
    ("The download queue will not come back after a restart: %1",
     "Черга завантажень не повернеться після перезапуску: %1"),
    ("Home", "Головна"),
    ("All", "Усе"),
    ("Search did not load further: %1",
     "Пошук не довантажився: %1"),
    ("Library", "Бібліотека"),
    ("Search: %1", "Пошук: %1"),
    ("Radio: %1", "Радіо: %1"),

    // ---- the track menu ----
    ("Play next", "Грати наступним"),
    ("Add to queue", "Додати до черги"),
    ("Start radio", "Радіо за треком"),
    ("Album: %1", "Альбом: %1"),
    ("No album", "Альбому немає"),
    ("Album…", "Альбом…"),
    ("Artist: %1", "Виконавець: %1"),
    ("Add to playlist…", "Додати до плейлиста…"),
    ("Download", "Завантажити"),

    // ---- downloads ----
    ("Queued", "У черзі"),
    ("Downloading — %1 %", "Завантажується — %1 %"),
    ("Looking for the stream…", "Шукаю потік…"),
    ("Saved", "Збережено"),
    ("Already downloaded", "Уже завантажено"),
    ("Error: %1", "Помилка: %1"),
    ("Cancelled", "Скасовано"),
    ("Cancel all", "Скасувати всі"),
    ("Clear", "Очистити"),
    ("Cancel", "Скасувати"),
    ("Show in folder", "Показати в теці"),

    // ---- signing in ----
    ("Sign in to YouTube Music", "Вхід у YouTube Music"),
    ("Without signing in there is no library, no mixes and no ratings.",
     "Без входу немає бібліотеки, міксів і оцінок."),
    ("Sign in with a Google window", "Увійти вікном Google"),
    ("A separate Google window opens. The sign-in belongs to orbis alone; your browser is not touched.",
     "Відкриється окреме вікно Google. Вхід буде лише в orbis — браузер його не торкається."),
    ("Or take the sign-in from a browser", "Або взяти вхід із браузера"),
    ("No browser to take the sign-in from: Chrome, Chromium, Brave, Vivaldi and Firefox are not found here.",
     "Немає звідки взяти вхід: Chrome, Chromium, Brave, Vivaldi і Firefox тут не знайдено."),
    ("Take sign-in", "Взяти вхід"),
    ("The browser must have music.youtube.com open and signed in. The sign-in follows the browser: when the browser renews its own, orbis reads it again.",
     "У браузері має бути відкрито music.youtube.com із входом. Вхід стежитиме за браузером: коли браузер оновить свій, orbis прочитає його знову."),
    ("Signed in with a Google window", "Вхід вікном Google"),
    ("Signed in from %1", "Вхід із %1"),
    ("Could not open the sign-in window: %1", "Не вдалося відкрити вікно входу: %1"),
    ("The sign-in window closed with an error: %1", "Вікно входу закрилося з помилкою: %1"),
    ("The sign-in window closed without signing in to YouTube.",
     "Вікно входу закрилося без входу в YouTube."),
    ("This browser is not signed in to YouTube Music: open music.youtube.com there and sign in.",
     "У цьому браузері немає входу в YouTube Music: відкрийте там music.youtube.com і увійдіть."),
    ("Could not read the browser's sign-in: %1", "Не вдалося прочитати вхід із браузера: %1"),
    ("YouTube Music did not accept the sign-in.", "YouTube Music не прийняв вхід."),
    ("Sign-in failed: %1", "Вхід не вдався: %1"),
    ("Could not delete the saved sign-in: %1", "Не вдалося видалити збережений вхід: %1"),
    ("Sign in to your account in the Google window; it closes by itself afterwards.",
     "Увійдіть в акаунт у вікні Google — після входу воно закриється саме."),
    ("Taking the sign-in from the browser…", "Беру вхід із браузера…"),
    ("The saved sign-in is not available: %1", "Збережений вхід недоступний: %1"),

    // ---- add to playlist ----
    ("New playlist with this track", "Новий плейлист із цим треком"),
    ("Name", "Назва"),
    ("Private", "Приватний"),
    ("Unlisted", "За посиланням"),
    ("Public", "Відкритий"),
    ("Create", "Створити"),

    // ---- settings ----
    ("Playback", "Відтворення"),
    ("Interface", "Інтерфейс"),
    ("Account", "Акаунт"),
    ("Quality", "Якість"),
    ("Best — Opus ~256 kbit/s (Premium)", "Найкраща — Opus ~256 кбіт/с (Premium)"),
    ("Normal — Opus ~130–160 kbit/s", "Звичайна — Opus ~130–160 кбіт/с"),
    ("Economy — Opus ~50–70 kbit/s", "Економна — Opus ~50–70 кбіт/с"),
    ("From the next track on. Downloads always take the best stream.",
     "З наступного треку. Завантаження завжди беруть найкращий потік."),
    ("Even out loudness", "Вирівнювати гучність"),
    ("Tracks that are too loud play quieter, down to YouTube's level (−14 LUFS). Quiet ones are not raised.",
     "Надто гучні треки звучать тихіше — до рівня YouTube (−14 LUFS). Тихі не підсилюються."),
    ("Skip tracks I disliked", "Пропускати треки з моїм «Не подобається»"),
    ("Next and the end of a track go past them. A disliked track you click still plays.",
     "«Наступний» і кінець треку їх пропускають. Трек, на який ви натиснули самі, однаково заграє."),
    ("Hide disliked tracks in lists", "Не показувати в списках треки з «Не подобається»"),
    ("Playlists, mixes and radios load without them. Lists already shown change when loaded again.",
     "Плейлисти, мікси й радіо завантажуються без них. Уже відкриті списки зміняться під час наступного завантаження."),
    ("Format", "Формат"),
    ("Opus — as streamed, no re-encoding", "Opus — як у потоці, без перекодування"),
    ("%1 kbit/s", "%1 кбіт/с"),
    ("Folder", "Тека"),
    ("Choose…", "Вибрати…"),
    ("System default", "Як у системі"),
    ("File name", "Ім'я файлу"),
    ("Default", "Типово"),
    ("Fields: {album_artist} {artist} {album} {year} {track} {title}; / makes a folder. An empty field drops out with its \" - \". Tracks already downloaded are looked for by the new name: under the old one they are not recognized.",
     "Поля: {album_artist} {artist} {album} {year} {track} {title}; / — тека. Порожнє поле випадає разом зі своїм « - ». Уже завантажені треки шукаються за новим ім'ям: під старим їх не впізнати."),
    ("Refine tags with MusicBrainz", "Уточнювати теги за MusicBrainz"),
    ("Genre, first release date, disc number. MusicBrainz receives the track's artist, title and album.",
     "Жанр, дата першого випуску, номер диска. MusicBrainz отримує виконавця, назву й альбом треку."),
    ("Folder for downloads", "Тека для завантажень"),
    ("Language", "Мова"),
    ("Glass", "Скло"),
    ("KWin blurs what is behind the window. Off: the window is opaque.",
     "KWin розмиває те, що за вікном. Вимкнено — вікно непрозоре."),
    ("Background opacity: %1 %", "Непрозорість тла: %1 %"),
    ("Less shows more of the desktop. Below 40 % text is hard to read over light windows.",
     "Менше — більше видно робочий стіл. Нижче 40 % текст погано читається на світлому."),
    ("Icon in the tray", "Значок у треї"),
    ("× hides the window in the tray. Without the icon × closes orbis.",
     "× ховає вікно в трей. Без значка × закриває orbis."),
    ("Unload the window in the tray", "Вивантажувати вікно в треї"),
    ("The hidden window frees its memory and is built again when opened; the music plays on.",
     "Сховане вікно звільняє пам'ять і будується заново під час відкриття; музика грає."),
    ("Loading the account…", "Акаунт завантажується…"),
    ("Sign in again…", "Увійти знову…"),
    ("Sign out", "Вийти"),
    ("\"Sign in again\" changes the way of signing in or the account: a Google window or a browser's sign-in.",
     "«Увійти знову» — змінити спосіб входу або акаунт: вікно Google або вхід із браузера."),
    ("Sign out of the account? orbis forgets the sign-in, music and downloads stop.",
     "Вийти з акаунта? orbis забуде вхід, музика й завантаження зупиняться."),
    ("Close the tabs and queues too",
     "Закрити й вкладки з чергами"),
    ("No field {%1}", "Немає поля {%1}"),
    ("The curly bracket is not closed", "Не закрито фігурну дужку"),
    ("The {title} field is needed", "Потрібне поле {title}"),
    ("The template is empty", "Шаблон порожній"),
    ("Song", "Пісня"),
    ("Artist", "Виконавець"),
    ("Album", "Альбом"),
    ("Video", "Відео"),
    ("Channel", "Канал"),
    ("%1\nwithout an album: %2", "%1\nбез альбому: %2"),
    ("The settings were not read (%1); changes will overwrite the file.",
     "Налаштування не прочитано (%1); зміни перезапишуть файл."),

    // ---- pages and their loading ----
    ("Home did not load", "Головна не завантажилася"),
    ("Library did not load", "Бібліотека не завантажилася"),
    ("Playlist did not load", "Плейлист не завантажився"),
    ("Album did not load", "Альбом не завантажився"),
    ("Artist did not load", "Виконавець не завантажився"),
    ("Radio did not load", "Радіо не завантажилося"),
    ("Radio: similar tracks without end", "Радіо: схожі треки без кінця"),
    ("Search failed", "Пошук не вдався"),
    ("Search results", "Результати пошуку"),
    ("Home did not load further: %1", "Головна не довантажилася: %1"),
    ("Top result", "Найкращий результат"),
    ("Songs", "Треки"),
    ("Videos", "Відео"),
    ("Episodes", "Епізоди"),
    ("Albums", "Альбоми"),
    ("Artists", "Виконавці"),
    ("Playlists", "Плейлисти"),

    // ---- playing, the queue, ratings ----
    ("Could not start the track: %1", "Не вдалося запустити трек: %1"),
    ("Could not resume: %1", "Не вдалося продовжити: %1"),
    ("Track details did not load: %1", "Відомості про трек не завантажилися: %1"),
    ("“%1” plays next", "«%1» заграє наступним"),
    ("“%1” is at the end of the queue", "«%1» — у кінці черги"),
    ("The playlists did not load: %1", "Список плейлистів не завантажився: %1"),
    ("“%1” added to “%2”", "«%1» додано до «%2»"),
    ("“%1”: YouTube Music did not take the track; most likely it is there already",
     "«%1»: YouTube Music не прийняв трек — найімовірніше, він там уже є"),
    ("Not added to “%1”: %2", "Не додалося до «%1»: %2"),
    ("Created the playlist “%1” with “%2”", "Створено плейлист «%1» з треком «%2»"),
    ("The playlist “%1” was not created: %2", "Плейлист «%1» не створився: %2"),
    ("The rating was not saved: %1", "Оцінка не збереглася: %1"),
    ("Queued for download: %1; the download button is below",
     "У черзі завантажень: %1 — кнопка завантажень унизу"),
    ("Could not queue: %1", "Не вдалося поставити в чергу: %1"),
    ("No data folder %1: %2", "Немає теки даних %1: %2"),
    ("The player did not start: %1", "Плеєр не запустився: %1"),

    // ---- the tray ----
    ("Nothing plays", "Нічого не грає"),
    ("Cannot play “%1”: %2", "Не вдалося відтворити «%1»: %2"),
    ("Cannot play: %1", "Не вдалося відтворити: %1"),
    ("Cannot save the tabs: %1", "Не вдалося зберегти вкладки: %1"),
    ("Cannot prepare the next track: %1", "Не вдалося підготувати наступний трек: %1"),
    ("Cannot load more tracks: %1", "Не вдалося завантажити ще треки: %1"),
    ("YouTube Music needs a sign-in", "Потрібен вхід у YouTube Music"),
    ("Hide window", "Сховати вікно"),
    ("Show window", "Показати вікно"),
    ("Settings…", "Налаштування…"),
    ("Quit", "Вихід"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_come_in_the_chosen_language() {
        assert_eq!(tr_in(Language::English, "Settings"), "Settings");
        assert_eq!(tr_in(Language::Ukrainian, "Settings"), "Налаштування");
        assert_eq!(
            tr_in(Language::Ukrainian, "not in the catalog"),
            "not in the catalog"
        );
        for language in [Language::English, Language::Ukrainian] {
            assert_eq!(Language::from_code(language.code()), language);
        }
        assert_eq!(Language::from_code("de"), Language::English);
        assert_eq!(Language::from_code("ru"), Language::English);
        assert_eq!(strings(Language::English), serde_json::json!({}));
        assert_eq!(strings(Language::Ukrainian)["Play"], "Грати");
    }

    #[test]
    fn placeholders_are_filled() {
        // The default language of a test thread is English.
        assert_eq!(
            trf("“%1” added to “%2”", &[&"Song", &"Mix"]),
            "“Song” added to “Mix”"
        );
    }

    #[test]
    fn the_catalog_is_whole() {
        let mut seen = std::collections::HashSet::new();
        for &(en, uk) in CATALOG {
            assert!(seen.insert(en), "twice in the catalog: {en}");
            assert!(!uk.is_empty(), "untranslated: {en}");
            for n in 1..=3 {
                let mark = format!("%{n}");
                assert_eq!(
                    en.contains(&mark),
                    uk.contains(&mark),
                    "{mark} in Ukrainian: {en}"
                );
            }
        }
    }

    /// The texts of `tr("…")`, `trf("…"`, `tr_in(…, "…")` and
    /// `trf_in(…, "…"` in `source`, unescaped.
    fn asked(source: &str) -> Vec<String> {
        let mut found = Vec::new();
        for opener in ["tr(\"", "trf(\"", "tr_in(", "trf_in("] {
            let mut rest = source;
            while let Some(at) = rest.find(opener) {
                // `attr("` and the like are not asks.
                let word = rest[..at]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_');
                rest = &rest[at + opener.len()..];
                if word {
                    continue;
                }
                // `tr_in(language, "…")`: the text is the second argument;
                // a definition or a call without a text in it is skipped.
                if !opener.ends_with('"') {
                    let Some(quote) = rest.find('"') else {
                        break;
                    };
                    if rest[..quote].contains([')', ';', '{']) {
                        continue;
                    }
                    rest = &rest[quote + 1..];
                }
                let mut text = String::new();
                let mut chars = rest.chars();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => match chars.next() {
                            Some('n') => text.push('\n'),
                            Some(other) => text.push(other),
                            None => break,
                        },
                        '"' => break,
                        c => text.push(c),
                    }
                }
                found.push(text);
            }
        }
        found
    }

    #[test]
    fn every_text_asked_for_is_in_the_catalog() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        for dir in [root.clone(), root.join("qml")] {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                let wanted = path.extension().is_some_and(|e| e == "rs" || e == "qml");
                if wanted && path.file_name() != Some("i18n.rs".as_ref()) {
                    files.push(path);
                }
            }
        }
        let mut missing = Vec::new();
        let mut count = 0;
        for file in &files {
            let source = std::fs::read_to_string(file).unwrap();
            for text in asked(&source) {
                count += 1;
                if !catalog().contains_key(text.as_str()) {
                    missing.push(format!("{}: {text}", file.display()));
                }
            }
        }
        assert!(count > 100, "the sources ask for {count} texts");
        assert!(
            missing.is_empty(),
            "not in the catalog:\n{}",
            missing.join("\n")
        );
    }
}
