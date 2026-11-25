//! A signed-in YouTube Music session: the cookie jar, the client context and
//! the headers every InnerTube request carries.

use cookie::time::OffsetDateTime;
use serde_json::{Value, json};

use crate::auth::{self, HOST, ORIGIN};
use crate::cookies::CookieJar;

/// InnerTube client name of the YouTube Music web app.
pub const CLIENT_NAME: &str = "WEB_REMIX";

/// Numeric ID of `WEB_REMIX`, sent as `X-YouTube-Client-Name`.
pub const CLIENT_ID: &str = "67";

/// The browser the session presents itself as. Firefox on Linux, matching the
/// browser the cookies usually come from; updated together with the client.
pub const DEFAULT_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:159.0) Gecko/20100101 Firefox/159.0";

/// Interface language and content country of the requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientContext {
    /// Interface language, such as `uk` or `en`.
    pub hl: String,
    /// Content country, such as `UA` or `US`.
    pub gl: String,
}

impl Default for ClientContext {
    fn default() -> Self {
        Self {
            hl: "en".to_owned(),
            gl: "US".to_owned(),
        }
    }
}

impl ClientContext {
    /// Takes the language and country from a POSIX locale such as
    /// `uk_UA.UTF-8`; falls back to the default for `C`, `POSIX` or nothing.
    pub fn from_locale(locale: Option<&str>) -> Self {
        let Some(locale) = locale else {
            return Self::default();
        };
        let base = locale.split(['.', '@']).next().unwrap_or_default();
        let (lang, country) = base.split_once('_').unwrap_or((base, ""));
        let valid = |s: &str, len: std::ops::RangeInclusive<usize>| {
            len.contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphabetic())
        };
        if !valid(lang, 2..=3) || lang.eq_ignore_ascii_case("c") {
            return Self::default();
        }
        let defaults = Self::default();
        Self {
            hl: lang.to_ascii_lowercase(),
            gl: if valid(country, 2..=2) {
                country.to_ascii_uppercase()
            } else {
                defaults.gl
            },
        }
    }

    /// The web client version for the day of `now`: `1.YYYYMMDD.01.00`.
    /// The site accepts the current date, as the web app itself sends it.
    pub fn client_version(now: i64) -> String {
        let date = OffsetDateTime::from_unix_timestamp(now)
            .unwrap_or(OffsetDateTime::UNIX_EPOCH)
            .date();
        format!(
            "1.{:04}{:02}{:02}.01.00",
            date.year(),
            u8::from(date.month()),
            date.day()
        )
    }
}

/// A YouTube Music session.
#[derive(Debug, Clone)]
pub struct Session {
    jar: CookieJar,
    context: ClientContext,
    user_agent: String,
    dirty: bool,
}

impl Session {
    pub fn new(jar: CookieJar, context: ClientContext) -> Self {
        Self {
            jar,
            context,
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            dirty: false,
        }
    }

    /// Restores a session saved with [`Session::save`].
    pub fn restore(saved: &[u8], context: ClientContext) -> Result<Self, serde_json::Error> {
        Ok(Self::new(serde_json::from_slice(saved)?, context))
    }

    /// The bytes to keep in secret storage: the cookie jar as JSON.
    pub fn save(&self) -> Vec<u8> {
        serde_json::to_vec(&self.jar).expect("a cookie jar always serializes")
    }

    pub fn jar(&self) -> &CookieJar {
        &self.jar
    }

    pub fn context(&self) -> &ClientContext {
        &self.context
    }

    /// `true` when the jar holds a SAPISID, so requests go out signed.
    pub fn is_signed_in(&self, now: i64) -> bool {
        auth::authorization(&self.jar, now).is_some()
    }

    /// Headers of an InnerTube request to `path` (such as `/youtubei/v1/browse`)
    /// at `now`.
    pub fn headers(&self, path: &str, now: i64) -> Vec<(&'static str, String)> {
        let mut headers = vec![
            ("user-agent", self.user_agent.clone()),
            ("accept", "*/*".to_owned()),
            ("accept-language", format!("{},en;q=0.5", self.context.hl)),
            ("content-type", "application/json".to_owned()),
            ("origin", ORIGIN.to_owned()),
            ("x-origin", ORIGIN.to_owned()),
            ("x-goog-authuser", "0".to_owned()),
            ("x-youtube-client-name", CLIENT_ID.to_owned()),
            (
                "x-youtube-client-version",
                ClientContext::client_version(now),
            ),
        ];
        if let Some(cookie) = self.jar.header_for(HOST, path, now) {
            headers.push(("cookie", cookie));
        }
        if let Some(authorization) = auth::authorization(&self.jar, now) {
            headers.push(("authorization", authorization));
        }
        headers
    }

    /// The `context` object every InnerTube request body starts with.
    pub fn context_body(&self, now: i64) -> Value {
        json!({
            "context": {
                "client": {
                    "clientName": CLIENT_NAME,
                    "clientVersion": ClientContext::client_version(now),
                    "hl": self.context.hl,
                    "gl": self.context.gl,
                },
                "user": {},
            }
        })
    }

    /// Takes the `Set-Cookie` headers of a response from `host` into the jar.
    /// Marks the session dirty when a cookie changed: see [`Session::take_dirty`].
    pub fn absorb_set_cookies<'a>(
        &mut self,
        set_cookies: impl IntoIterator<Item = &'a str>,
        host: &str,
        now: i64,
    ) {
        for header in set_cookies {
            if self.jar.apply_set_cookie(header, host, now) {
                self.dirty = true;
            }
        }
    }

    /// Returns whether the jar changed since the last call, and clears the
    /// flag. When it returns `true`, the caller saves the session.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
}

/// Makes a request's `headers` and `payload` (built by [`Session::headers`]
/// and [`Session::context_body`]) ask in the interface language `hl`.
pub(crate) fn speak(headers: &mut [(&'static str, String)], payload: &mut Value, hl: &str) {
    for (name, value) in headers.iter_mut() {
        if *name == "accept-language" {
            *value = hl.to_owned();
        }
    }
    if let Some(client) = payload.pointer_mut("/context/client") {
        client["hl"] = Value::from(hl);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000; // 2023-11-14 22:13:20 UTC

    fn signed_in() -> Session {
        let jar = CookieJar::from_cookie_header(
            "SID=s; __Secure-3PAPISID=test-sapisid; __Secure-1PSIDTS=old",
            "youtube.com",
        );
        Session::new(jar, ClientContext::from_locale(Some("uk_UA.UTF-8")))
    }

    #[test]
    fn locale_gives_language_and_country() {
        let uk = ClientContext::from_locale(Some("uk_UA.UTF-8"));
        assert_eq!((uk.hl.as_str(), uk.gl.as_str()), ("uk", "UA"));
        let de = ClientContext::from_locale(Some("de_DE@euro"));
        assert_eq!((de.hl.as_str(), de.gl.as_str()), ("de", "DE"));
        let en = ClientContext::from_locale(Some("en"));
        assert_eq!((en.hl.as_str(), en.gl.as_str()), ("en", "US"));
        assert_eq!(
            ClientContext::from_locale(Some("C.UTF-8")),
            ClientContext::default()
        );
        assert_eq!(
            ClientContext::from_locale(Some("POSIX")),
            ClientContext::default()
        );
        assert_eq!(ClientContext::from_locale(None), ClientContext::default());
    }

    #[test]
    fn client_version_uses_the_utc_date() {
        assert_eq!(ClientContext::client_version(NOW), "1.20231114.01.00");
    }

    #[test]
    fn signed_in_headers_carry_cookie_and_authorization() {
        let session = signed_in();
        assert!(session.is_signed_in(NOW));
        let headers = session.headers("/youtubei/v1/browse", NOW);
        let get = |name: &str| {
            headers
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.as_str())
        };
        assert!(
            get("authorization")
                .unwrap()
                .starts_with("SAPISIDHASH 1700000000_17d748c166afd876ceb872a291e5befdca771528")
        );
        assert!(
            get("cookie")
                .unwrap()
                .contains("__Secure-3PAPISID=test-sapisid")
        );
        assert_eq!(get("x-origin"), Some(ORIGIN));
        assert_eq!(get("x-youtube-client-name"), Some("67"));
        assert_eq!(get("accept-language"), Some("uk,en;q=0.5"));
    }

    #[test]
    fn signed_out_session_sends_no_authorization() {
        let session = Session::new(CookieJar::default(), ClientContext::default());
        let headers = session.headers("/youtubei/v1/browse", NOW);
        assert!(
            headers
                .iter()
                .all(|(n, _)| *n != "authorization" && *n != "cookie")
        );
        assert!(!session.is_signed_in(NOW));
    }

    #[test]
    fn a_request_can_ask_in_another_language() {
        let session = Session::new(
            CookieJar::default(),
            ClientContext {
                hl: "uk".into(),
                gl: "UA".into(),
            },
        );
        let mut headers = session.headers("/youtubei/v1/browse", 0);
        let mut payload = session.context_body(0);
        assert_eq!(payload["context"]["client"]["hl"], "uk");
        speak(&mut headers, &mut payload, "en");
        assert_eq!(payload["context"]["client"]["hl"], "en");
        // The country stays: the mixes are the account's in its country.
        assert_eq!(payload["context"]["client"]["gl"], "UA");
        let language = headers.iter().find(|(n, _)| *n == "accept-language");
        assert_eq!(language.map(|(_, v)| v.as_str()), Some("en"));
    }

    #[test]
    fn context_body_names_the_client() {
        let body = signed_in().context_body(NOW);
        let client = &body["context"]["client"];
        assert_eq!(client["clientName"], "WEB_REMIX");
        assert_eq!(client["clientVersion"], "1.20231114.01.00");
        assert_eq!(client["hl"], "uk");
        assert_eq!(client["gl"], "UA");
    }

    #[test]
    fn rotated_cookie_marks_the_session_dirty_once() {
        let mut session = signed_in();
        session.absorb_set_cookies(
            [
                "__Secure-1PSIDTS=new; Domain=.youtube.com; Path=/; Max-Age=31536000; Secure",
                "IGNORED=1; Domain=google.com",
            ],
            HOST,
            NOW,
        );
        assert!(session.take_dirty());
        assert!(!session.take_dirty());
        assert_eq!(
            session.jar().get("__Secure-1PSIDTS", HOST, NOW),
            Some("new")
        );

        // An unchanged cookie leaves the session clean.
        session.absorb_set_cookies(
            ["__Secure-1PSIDTS=new; Domain=.youtube.com; Path=/; Max-Age=31536000; Secure"],
            HOST,
            NOW,
        );
        assert!(!session.take_dirty());
    }

    #[test]
    fn saved_session_restores() {
        let mut session = signed_in();
        session.absorb_set_cookies(["YSC=1; Path=/"], HOST, NOW);
        let restored = Session::restore(&session.save(), session.context().clone()).unwrap();
        assert_eq!(restored.jar(), session.jar());
        assert!(Session::restore(b"not json", ClientContext::default()).is_err());
    }
}
