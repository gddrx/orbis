//! The `Authorization` header of a cookie-based session.
//!
//! InnerTube accepts a signed-in request when it carries the account cookies
//! and `SAPISIDHASH <ts>_<sha1("<ts> <SAPISID> <origin>")>`, where `<ts>` is the
//! current Unix time in seconds and `<origin>` is the site's origin. The web
//! app adds the same hash over the first- and third-party cookies
//! (`SAPISID1PHASH`, `SAPISID3PHASH`); some endpoints, such as playlist edits,
//! answer 401 without them.

use sha1::{Digest, Sha1};

use crate::cookies::CookieJar;

/// The origin YouTube Music requests are signed for.
pub const ORIGIN: &str = "https://music.youtube.com";

/// The host YouTube Music requests go to.
pub const HOST: &str = "music.youtube.com";

/// Cookies that hold the SAPISID, in order of preference.
const SAPISID_COOKIES: [&str; 2] = ["__Secure-3PAPISID", "SAPISID"];

/// `SAPISIDHASH <timestamp>_<hex sha1>` for `sapisid` and `origin`.
pub fn sapisid_hash(sapisid: &str, origin: &str, timestamp: i64) -> String {
    let digest = Sha1::digest(format!("{timestamp} {sapisid} {origin}").as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest.iter() {
        hex.push_str(&format!("{byte:02x}"));
    }
    format!("SAPISIDHASH {timestamp}_{hex}")
}

/// The `Authorization` header for a YouTube Music request at `now`, or `None`
/// when the jar holds no SAPISID (the session is not signed in).
pub fn authorization(jar: &CookieJar, now: i64) -> Option<String> {
    let sapisid = SAPISID_COOKIES
        .iter()
        .find_map(|name| jar.get(name, HOST, now))?;
    let mut header = sapisid_hash(sapisid, ORIGIN, now);
    for (cookie, scheme) in [
        ("__Secure-1PAPISID", "SAPISID1PHASH"),
        ("__Secure-3PAPISID", "SAPISID3PHASH"),
    ] {
        if let Some(value) = jar.get(cookie, HOST, now) {
            let hash = sapisid_hash(value, ORIGIN, now);
            let digest = hash.trim_start_matches("SAPISIDHASH ");
            header.push_str(&format!(" {scheme} {digest}"));
        }
    }
    Some(header)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_matches_a_known_vector() {
        // printf '%s' "1700000000 test-sapisid https://music.youtube.com" | sha1sum
        assert_eq!(
            sapisid_hash("test-sapisid", ORIGIN, 1_700_000_000),
            "SAPISIDHASH 1700000000_17d748c166afd876ceb872a291e5befdca771528"
        );
    }

    #[test]
    fn secure_cookie_is_preferred() {
        let jar = CookieJar::from_cookie_header(
            "SAPISID=plain; __Secure-3PAPISID=test-sapisid",
            "youtube.com",
        );
        assert_eq!(
            authorization(&jar, 1_700_000_000).as_deref(),
            Some(
                "SAPISIDHASH 1700000000_17d748c166afd876ceb872a291e5befdca771528 \
                 SAPISID3PHASH 1700000000_17d748c166afd876ceb872a291e5befdca771528"
            )
        );
    }

    #[test]
    fn first_and_third_party_hashes_follow() {
        let jar = CookieJar::from_cookie_header(
            "SAPISID=test-sapisid; __Secure-1PAPISID=test-sapisid; __Secure-3PAPISID=test-sapisid",
            "youtube.com",
        );
        let header = authorization(&jar, 1_700_000_000).unwrap();
        let parts: Vec<&str> = header.split(' ').collect();
        assert_eq!(
            parts.iter().step_by(2).copied().collect::<Vec<_>>(),
            ["SAPISIDHASH", "SAPISID1PHASH", "SAPISID3PHASH"]
        );
    }

    #[test]
    fn no_sapisid_means_signed_out() {
        let jar = CookieJar::from_cookie_header("YSC=1; VISITOR_INFO1_LIVE=v", "youtube.com");
        assert_eq!(authorization(&jar, 1_700_000_000), None);
    }
}
