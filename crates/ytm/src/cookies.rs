//! The cookies of a signed-in YouTube Music session.
//!
//! Google rotates some session cookies (`__Secure-*PSIDTS` among them) through
//! `Set-Cookie` headers on ordinary responses, so the jar takes every response's
//! cookies in and reports whether anything changed: the caller then saves it.
//! Matching follows RFC 6265 in the parts this client needs: host-only and
//! domain cookies, path prefixes, `Max-Age` over `Expires`, and deletion by an
//! expiry in the past. All requests go over HTTPS, so `Secure` never filters.

use serde::{Deserialize, Serialize};

/// One stored cookie. `Debug` hides the value.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCookie {
    pub name: String,
    pub value: String,
    /// Lowercase, without a leading dot.
    pub domain: String,
    /// `true` when the cookie came without a `Domain` attribute and is sent
    /// only to the exact host that set it.
    pub host_only: bool,
    pub path: String,
    /// Expiry as Unix seconds; `None` for a session cookie.
    pub expires: Option<i64>,
    pub secure: bool,
    pub http_only: bool,
}

impl std::fmt::Debug for StoredCookie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredCookie")
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .field("domain", &self.domain)
            .field("host_only", &self.host_only)
            .field("path", &self.path)
            .field("expires", &self.expires)
            .finish_non_exhaustive()
    }
}

impl StoredCookie {
    fn matches_host(&self, host: &str) -> bool {
        if self.host_only {
            host == self.domain
        } else {
            domain_match(host, &self.domain)
        }
    }

    fn matches_path(&self, path: &str) -> bool {
        path_match(path, &self.path)
    }

    fn is_expired(&self, now: i64) -> bool {
        self.expires.is_some_and(|expires| expires <= now)
    }

    fn same_slot(&self, other: &StoredCookie) -> bool {
        self.name == other.name && self.domain == other.domain && self.path == other.path
    }
}

/// The cookie jar of one account.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CookieJar {
    cookies: Vec<StoredCookie>,
}

impl CookieJar {
    /// Builds a jar from a `Cookie` request header as copied from the browser's
    /// developer tools (`a=1; b=2`). The cookies get `domain` (for example
    /// `youtube.com`) and the path `/`; their expiry is unknown, so they are kept
    /// as session cookies until a response replaces them.
    pub fn from_cookie_header(header: &str, domain: &str) -> Self {
        let domain = normalize_domain(domain);
        let mut jar = Self::default();
        for pair in header.split(';') {
            let Some((name, value)) = pair.trim().split_once('=') else {
                continue;
            };
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            jar.insert(StoredCookie {
                name: name.to_owned(),
                value: value.trim().to_owned(),
                domain: domain.clone(),
                host_only: false,
                path: "/".to_owned(),
                expires: None,
                secure: true,
                http_only: false,
            });
        }
        jar
    }

    /// Adds `cookie`, replacing the one with the same name, domain and path.
    pub fn insert(&mut self, cookie: StoredCookie) {
        match self.cookies.iter_mut().find(|c| c.same_slot(&cookie)) {
            Some(existing) => *existing = cookie,
            None => self.cookies.push(cookie),
        }
    }

    /// Applies one `Set-Cookie` header received from `request_host`.
    ///
    /// Returns `true` when the jar changed. Headers that do not parse, or whose
    /// `Domain` does not cover `request_host`, are ignored.
    pub fn apply_set_cookie(&mut self, header: &str, request_host: &str, now: i64) -> bool {
        let Ok(parsed) = cookie::Cookie::parse(header) else {
            return false;
        };
        let request_host = request_host.to_ascii_lowercase();

        let (domain, host_only) = match parsed.domain() {
            Some(domain) if !domain.is_empty() => {
                let domain = normalize_domain(domain);
                if !domain_match(&request_host, &domain) {
                    return false;
                }
                (domain, false)
            }
            _ => (request_host, true),
        };

        let path = match parsed.path() {
            Some(path) if path.starts_with('/') => path.to_owned(),
            _ => "/".to_owned(),
        };

        // Max-Age wins over Expires (RFC 6265, 5.3, step 3).
        let expires = if let Some(max_age) = parsed.max_age() {
            Some(now.saturating_add(max_age.whole_seconds()))
        } else {
            parsed.expires_datetime().map(|at| at.unix_timestamp())
        };

        let cookie = StoredCookie {
            name: parsed.name().to_owned(),
            value: parsed.value().to_owned(),
            domain,
            host_only,
            path,
            expires,
            secure: parsed.secure().unwrap_or(false),
            http_only: parsed.http_only().unwrap_or(false),
        };

        if cookie.is_expired(now) {
            let before = self.cookies.len();
            self.cookies.retain(|c| !c.same_slot(&cookie));
            return self.cookies.len() != before;
        }

        match self.cookies.iter_mut().find(|c| c.same_slot(&cookie)) {
            Some(existing) if *existing == cookie => false,
            Some(existing) => {
                *existing = cookie;
                true
            }
            None => {
                self.cookies.push(cookie);
                true
            }
        }
    }

    /// The `Cookie` header for a request to `host` and `path`, or `None` when no
    /// cookie applies. Longer paths come first, as browsers send them.
    pub fn header_for(&self, host: &str, path: &str, now: i64) -> Option<String> {
        let host = host.to_ascii_lowercase();
        let mut matching: Vec<&StoredCookie> = self
            .cookies
            .iter()
            .filter(|c| !c.is_expired(now) && c.matches_host(&host) && c.matches_path(path))
            .collect();
        if matching.is_empty() {
            return None;
        }
        matching.sort_by_key(|c| std::cmp::Reverse(c.path.len()));
        let pairs: Vec<String> = matching
            .iter()
            .map(|c| format!("{}={}", c.name, c.value))
            .collect();
        Some(pairs.join("; "))
    }

    /// The value of cookie `name` that would be sent to `host`.
    pub fn get(&self, name: &str, host: &str, now: i64) -> Option<&str> {
        let host = host.to_ascii_lowercase();
        self.cookies
            .iter()
            .filter(|c| c.name == name && !c.is_expired(now) && c.matches_host(&host))
            .max_by_key(|c| c.path.len())
            .map(|c| c.value.as_str())
    }

    /// Drops expired cookies. Returns `true` when any were dropped.
    pub fn remove_expired(&mut self, now: i64) -> bool {
        let before = self.cookies.len();
        self.cookies.retain(|c| !c.is_expired(now));
        self.cookies.len() != before
    }

    /// The jar in the Netscape `cookies.txt` format that yt-dlp and curl
    /// read, without expired cookies. The text holds secrets: write it only
    /// to private files.
    pub fn to_netscape(&self, now: i64) -> String {
        let mut text = String::from("# Netscape HTTP Cookie File\n");
        for c in self.cookies.iter().filter(|c| !c.is_expired(now)) {
            let domain = if c.host_only {
                c.domain.clone()
            } else {
                format!(".{}", c.domain)
            };
            let flag = |b: bool| if b { "TRUE" } else { "FALSE" };
            text.push_str(&format!(
                "{}{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                if c.http_only { "#HttpOnly_" } else { "" },
                domain,
                flag(!c.host_only),
                c.path,
                flag(c.secure),
                c.expires.unwrap_or(0),
                c.name,
                c.value
            ));
        }
        text
    }

    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }
}

fn normalize_domain(domain: &str) -> String {
    domain.trim_start_matches('.').to_ascii_lowercase()
}

/// RFC 6265, 5.1.3: `host` equals `domain` or ends with `.` + `domain`.
fn domain_match(host: &str, domain: &str) -> bool {
    host == domain
        || (host.len() > domain.len()
            && host.ends_with(domain)
            && host.as_bytes()[host.len() - domain.len() - 1] == b'.')
}

/// RFC 6265, 5.1.4.
fn path_match(request_path: &str, cookie_path: &str) -> bool {
    request_path == cookie_path
        || (request_path.starts_with(cookie_path)
            && (cookie_path.ends_with('/')
                || request_path.as_bytes().get(cookie_path.len()) == Some(&b'/')))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000;

    #[test]
    fn header_from_devtools_becomes_domain_cookies() {
        let jar = CookieJar::from_cookie_header(
            " SID=abc; __Secure-3PAPISID=xyz ;broken; =x",
            ".YouTube.com",
        );
        assert_eq!(jar.len(), 2);
        assert_eq!(jar.get("SID", "music.youtube.com", NOW), Some("abc"));
        assert_eq!(
            jar.get("__Secure-3PAPISID", "www.youtube.com", NOW),
            Some("xyz")
        );
        assert_eq!(jar.get("SID", "google.com", NOW), None);
    }

    #[test]
    fn domain_matching_needs_a_dot_boundary() {
        assert!(domain_match("music.youtube.com", "youtube.com"));
        assert!(domain_match("youtube.com", "youtube.com"));
        assert!(!domain_match("notyoutube.com", "youtube.com"));
        assert!(!domain_match("com", "youtube.com"));
    }

    #[test]
    fn path_matching_follows_rfc() {
        assert!(path_match("/youtubei/v1/browse", "/"));
        assert!(path_match("/youtubei/v1", "/youtubei"));
        assert!(!path_match("/youtubeix", "/youtubei"));
        assert!(path_match("/youtubei/", "/youtubei/"));
    }

    #[test]
    fn set_cookie_rotates_a_value() {
        let mut jar = CookieJar::from_cookie_header("__Secure-1PSIDTS=old", "youtube.com");
        let changed = jar.apply_set_cookie(
            "__Secure-1PSIDTS=new; Domain=.youtube.com; Path=/; Max-Age=3600; Secure; HttpOnly",
            "music.youtube.com",
            NOW,
        );
        assert!(changed);
        assert_eq!(jar.len(), 1);
        assert_eq!(
            jar.get("__Secure-1PSIDTS", "music.youtube.com", NOW),
            Some("new")
        );
        // The same header again changes nothing, so the caller need not save.
        let again = jar.apply_set_cookie(
            "__Secure-1PSIDTS=new; Domain=.youtube.com; Path=/; Max-Age=3600; Secure; HttpOnly",
            "music.youtube.com",
            NOW,
        );
        assert!(!again);
    }

    #[test]
    fn set_cookie_without_domain_is_host_only() {
        let mut jar = CookieJar::default();
        assert!(jar.apply_set_cookie("YSC=1; Path=/", "music.youtube.com", NOW));
        assert_eq!(jar.get("YSC", "music.youtube.com", NOW), Some("1"));
        assert_eq!(jar.get("YSC", "www.youtube.com", NOW), None);
    }

    #[test]
    fn set_cookie_for_a_foreign_domain_is_ignored() {
        let mut jar = CookieJar::default();
        assert!(!jar.apply_set_cookie("SID=evil; Domain=google.com", "music.youtube.com", NOW));
        assert!(jar.is_empty());
    }

    #[test]
    fn expiry_in_the_past_deletes() {
        let mut jar = CookieJar::from_cookie_header("VISITOR_INFO1_LIVE=v", "youtube.com");
        assert!(jar.apply_set_cookie(
            "VISITOR_INFO1_LIVE=; Domain=.youtube.com; Path=/; Max-Age=0",
            "music.youtube.com",
            NOW,
        ));
        assert!(jar.is_empty());
        // An already expired cookie that the jar does not hold changes nothing.
        assert!(!jar.apply_set_cookie(
            "A=1; Domain=youtube.com; Expires=Wed, 01 Jan 2020 00:00:00 GMT",
            "music.youtube.com",
            NOW,
        ));
        assert!(jar.is_empty());
    }

    #[test]
    fn max_age_wins_over_expires() {
        let mut jar = CookieJar::default();
        jar.apply_set_cookie(
            "A=1; Domain=youtube.com; Max-Age=60; Expires=Wed, 01 Jan 2020 00:00:00 GMT",
            "music.youtube.com",
            NOW,
        );
        assert_eq!(jar.get("A", "music.youtube.com", NOW + 59), Some("1"));
        assert_eq!(jar.get("A", "music.youtube.com", NOW + 60), None);
        assert!(jar.remove_expired(NOW + 60));
        assert!(jar.is_empty());
    }

    #[test]
    fn header_puts_longer_paths_first_and_skips_others() {
        let mut jar = CookieJar::default();
        jar.apply_set_cookie(
            "ROOT=r; Domain=youtube.com; Path=/",
            "music.youtube.com",
            NOW,
        );
        jar.apply_set_cookie(
            "API=a; Domain=youtube.com; Path=/youtubei",
            "music.youtube.com",
            NOW,
        );
        jar.apply_set_cookie("G=g; Domain=google.com", "accounts.google.com", NOW);
        assert_eq!(
            jar.header_for("music.youtube.com", "/youtubei/v1/browse", NOW)
                .as_deref(),
            Some("API=a; ROOT=r")
        );
        assert_eq!(
            jar.header_for("music.youtube.com", "/watch", NOW)
                .as_deref(),
            Some("ROOT=r")
        );
        assert_eq!(jar.header_for("example.com", "/", NOW), None);
    }

    #[test]
    fn netscape_export() {
        let mut jar = CookieJar::default();
        jar.apply_set_cookie(
            "SID=s1; Domain=.youtube.com; Path=/; Max-Age=100; Secure; HttpOnly",
            "music.youtube.com",
            NOW,
        );
        jar.apply_set_cookie("YSC=y1; Path=/", "music.youtube.com", NOW);
        jar.apply_set_cookie(
            "OLD=o; Domain=youtube.com; Max-Age=1",
            "music.youtube.com",
            NOW,
        );
        let text = jar.to_netscape(NOW + 10);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "# Netscape HTTP Cookie File");
        assert_eq!(
            lines[1],
            format!(
                "#HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t{}\tSID\ts1",
                NOW + 100
            )
        );
        assert_eq!(lines[2], "music.youtube.com\tFALSE\t/\tFALSE\t0\tYSC\ty1");
        assert_eq!(lines.len(), 3, "the expired cookie is left out");
    }

    #[test]
    fn debug_hides_values() {
        let jar = CookieJar::from_cookie_header("SID=secret-value", "youtube.com");
        let shown = format!("{jar:?}");
        assert!(shown.contains("SID") && !shown.contains("secret-value"));
    }

    #[test]
    fn jar_survives_serialization() {
        let mut jar = CookieJar::from_cookie_header("SID=abc", "youtube.com");
        jar.apply_set_cookie("YSC=1; Path=/; Max-Age=10", "music.youtube.com", NOW);
        let json = serde_json::to_string(&jar).unwrap();
        let back: CookieJar = serde_json::from_str(&json).unwrap();
        assert_eq!(back, jar);
    }
}
