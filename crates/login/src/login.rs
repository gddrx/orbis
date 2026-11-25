//! The sign-in window's object for QML: where the page is, whether the user
//! has signed in, and the cookies to send to orbis.
//!
//! The user is signed in once the page reaches YouTube Music and the
//! profile holds a YouTube sign-in cookie; in whichever order the two come.

use qtbridge::qobject;

/// Where the window starts: Google's sign-in for YouTube, which then opens
/// YouTube Music — the link of YouTube Music's own "Sign in" button as the
/// page had it on 23 November 2026, without `hl` (Google takes the language
/// from the system).
const START_URL: &str = "https://accounts.google.com/ServiceLogin?ltmpl=music&service=youtube&uilel=3&passive=true&continue=https%3A%2F%2Fwww.youtube.com%2Fsignin%3Faction_handle_signin%3Dtrue%26app%3Ddesktop%26next%3Dhttps%253A%252F%252Fmusic.youtube.com%252F%26feature%3D__FEATURE__";
/// The host of YouTube Music, where the sign-in ends.
const MUSIC_HOST: &str = "music.youtube.com";
/// The cookies that sign requests to YouTube (`SAPISIDHASH`).
const SIGN_IN_COOKIES: [&str; 2] = ["SAPISID", "__Secure-3PAPISID"];

/// `url` without its query and fragment, which may carry tokens: what the
/// trace shows of a page.
pub fn page_of(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or_default()
}

/// The host of `url` and whether it is HTTPS; an empty host for other
/// schemes (`about:blank`, `data:`).
pub fn host_of(url: &str) -> (String, bool) {
    let (secure, rest) = if let Some(rest) = url.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = url.strip_prefix("http://") {
        (false, rest)
    } else {
        return (String::new(), false);
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    let host = host.split(':').next().unwrap_or_default();
    (host.to_ascii_lowercase(), secure)
}

#[qobject(NoQmlElement, ConvertToCamelCase)]
mod login {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::rc::Rc;

    use desktop::browser_cookies::{BrowserCookie, host_in_domains};
    use desktop::login_window::{Channel, EXIT_CANCELLED};
    use qtbridge::QmlObject;

    use super::{MUSIC_HOST, SIGN_IN_COOKIES, START_URL, host_of, page_of};

    /// The exit code when sending the cookies failed.
    const EXIT_FAILED: i32 = 2;

    pub struct Login {
        start_url: String,
        /// `en` or `uk`.
        language: String,
        /// The host of the page shown, so the user sees where the password
        /// goes.
        host: String,
        /// The page came over HTTPS.
        secure: bool,
        /// Signed in: the window is about to close.
        done: bool,

        /// The page is on YouTube Music.
        on_music: bool,
        /// The profile's cookies by host, path and name.
        cookies: BTreeMap<(String, String, String), BrowserCookie>,
        /// Only cookies of these domains go to orbis.
        domains: Vec<String>,
        /// Where the cookies go; `None` when started without orbis.
        channel: Option<Channel>,
        exit_code: i32,
        /// `ORBIS_LOGIN_TRACE=1`: the pages and the cookies' names go to
        /// stderr, never a value or a query string.
        trace: bool,
    }

    impl Login {
        qproperty!("startUrl", Member = start_url, Notify = start_url_changed);
        qproperty!("language", Member = language, Notify = start_url_changed);
        qproperty!("host", Member = host, Notify = page_changed);
        qproperty!("secure", Member = secure, Notify = page_changed);
        qproperty!("done", Member = done, Notify = done_changed);

        #[qsignal]
        fn start_url_changed(&mut self);
        #[qsignal]
        fn page_changed(&mut self);
        #[qsignal]
        fn done_changed(&mut self);

        pub fn create(
            channel: Option<Channel>,
            domains: Vec<String>,
            language: String,
        ) -> Rc<RefCell<Self>> {
            let login = Rc::new(RefCell::new(Self {
                start_url: START_URL.to_owned(),
                language,
                host: String::new(),
                secure: true,
                done: false,
                on_music: false,
                cookies: BTreeMap::new(),
                domains,
                channel,
                exit_code: EXIT_CANCELLED,
                trace: std::env::var("ORBIS_LOGIN_TRACE").is_ok_and(|v| v == "1"),
            }));
            Self::attach_qobject(&login);
            login
        }

        /// 0 when the cookies were sent, else why not.
        pub fn exit_code(&self) -> i32 {
            self.exit_code
        }

        /// The page moved to `url`.
        #[qslot]
        pub fn visit(&mut self, url: String) {
            if self.trace {
                eprintln!("orbis-login: page {}", page_of(&url));
            }
            let (host, secure) = host_of(&url);
            self.on_music = host == MUSIC_HOST;
            if host != self.host || secure != self.secure {
                self.host = host;
                self.secure = secure;
                self.page_changed();
            }
            self.check();
        }

        /// The profile's cookie store added or removed `cookie`.
        pub fn take_cookie(&mut self, added: bool, cookie: BrowserCookie) {
            if self.trace {
                let change = if added { "+" } else { "-" };
                eprintln!(
                    "orbis-login: cookie {change} {} {}{}",
                    cookie.name, cookie.host, cookie.path
                );
            }
            let key = (
                cookie.host.clone(),
                cookie.path.clone(),
                cookie.name.clone(),
            );
            if added {
                self.cookies.insert(key, cookie);
            } else {
                self.cookies.remove(&key);
            }
            self.check();
        }

        fn signed_in(&self) -> bool {
            self.cookies.values().any(|cookie| {
                SIGN_IN_COOKIES.contains(&cookie.name.as_str())
                    && host_in_domains(&cookie.host, &["youtube.com"])
            })
        }

        fn check(&mut self) {
            if !self.done && self.on_music && self.signed_in() {
                self.done = true;
                self.done_changed();
            }
        }

        /// Sends the cookies to orbis; QML closes the window after it.
        #[qslot]
        pub fn finish(&mut self) {
            if !self.done || self.exit_code != EXIT_CANCELLED {
                return;
            }
            let domains: Vec<&str> = self.domains.iter().map(String::as_str).collect();
            let cookies: Vec<BrowserCookie> = self
                .cookies
                .values()
                .filter(|cookie| host_in_domains(&cookie.host, &domains))
                .cloned()
                .collect();
            self.exit_code = match self.channel.take() {
                Some(channel) => match channel.send(cookies) {
                    Ok(()) => 0,
                    Err(e) => {
                        eprintln!("orbis-login: cannot send the cookies to orbis: {e}");
                        EXIT_FAILED
                    }
                },
                None => {
                    // Started by hand: say what would go, without values.
                    eprintln!(
                        "orbis-login: signed in; {} cookies of {} (no orbis to send them to)",
                        cookies.len(),
                        domains.join(" and ")
                    );
                    for cookie in &cookies {
                        eprintln!("  {} {}{}", cookie.name, cookie.host, cookie.path);
                    }
                    0
                }
            };
        }
    }
}

pub use login::Login;

#[cfg(test)]
mod tests {
    use super::{host_of, page_of};

    #[test]
    fn pages_lose_queries_and_fragments() {
        assert_eq!(
            page_of("https://www.youtube.com/signin?action_handle_signin=true&next=x#y"),
            "https://www.youtube.com/signin"
        );
        assert_eq!(
            page_of("https://music.youtube.com/"),
            "https://music.youtube.com/"
        );
    }

    #[test]
    fn hosts_of_urls() {
        assert_eq!(
            host_of("https://accounts.google.com/v3/signin?x=1"),
            ("accounts.google.com".into(), true)
        );
        assert_eq!(
            host_of("https://Music.YouTube.com"),
            ("music.youtube.com".into(), true)
        );
        assert_eq!(
            host_of("http://user@example.com:8080/#a"),
            ("example.com".into(), false)
        );
        assert_eq!(host_of("about:blank"), (String::new(), false));
    }
}
