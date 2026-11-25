//! The InnerTube HTTP client of a session.
//!
//! Every response's `Set-Cookie` headers go back into the session, so rotated
//! cookies are kept; [`Client::take_changed_session`] tells the caller when to
//! save them.

use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, SET_COOKIE};
use serde_json::{Map, Value};

use crate::account::{Account, parse_account};
use crate::auth::HOST;
use crate::session::Session;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("YouTube Music answered {status}: {message}")]
    Status { status: u16, message: String },
    #[error("unexpected response: {0}")]
    Unexpected(String),
    #[error("the session is not signed in")]
    SignedOut,
    #[error("YouTube Music refused the change: {0}")]
    Rejected(String),
}

/// The current Unix time in seconds.
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A YouTube Music client over one session.
#[derive(Debug)]
pub struct Client {
    http: reqwest::Client,
    session: Mutex<Session>,
}

impl Client {
    pub fn new(session: Session) -> Result<Self, Error> {
        let http = reqwest::Client::builder().gzip(true).brotli(true).build()?;
        Ok(Self {
            http,
            session: Mutex::new(session),
        })
    }

    /// Calls `endpoint` (such as `browse`) with `body` merged into the client
    /// context, and returns the JSON response.
    pub async fn post(&self, endpoint: &str, body: Map<String, Value>) -> Result<Value, Error> {
        self.post_in(endpoint, body, None).await
    }

    /// [`Client::post`] in the interface language `language` instead of
    /// the session's, when given.
    pub(crate) async fn post_in(
        &self,
        endpoint: &str,
        body: Map<String, Value>,
        language: Option<&str>,
    ) -> Result<Value, Error> {
        let path = format!("/youtubei/v1/{endpoint}");
        let url = format!("https://{HOST}{path}?prettyPrint=false");
        let now = unix_now();
        let (mut headers, mut payload) = {
            let session = self.session.lock().expect("session lock");
            (session.headers(&path, now), session.context_body(now))
        };
        if let Some(language) = language {
            crate::session::speak(&mut headers, &mut payload, language);
        }
        if let Value::Object(map) = &mut payload {
            map.extend(body);
        }

        let mut header_map = HeaderMap::new();
        for (name, value) in headers {
            let value = HeaderValue::from_str(&value)
                .map_err(|_| Error::Unexpected(format!("header {name} is not valid")))?;
            header_map.insert(HeaderName::from_static(name), value);
        }

        let started = Instant::now();
        let response = self
            .http
            .post(url)
            .headers(header_map)
            .json(&payload)
            .send()
            .await?;
        crate::netlog::log(format_args!(
            "innertube {endpoint} {} in {} ms",
            response.status().as_u16(),
            started.elapsed().as_millis()
        ));

        let set_cookies: Vec<String> = response
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok().map(str::to_owned))
            .collect();
        self.session
            .lock()
            .expect("session lock")
            .absorb_set_cookies(set_cookies.iter().map(String::as_str), HOST, unix_now());

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Status {
                status: status.as_u16(),
                message: error_message(&body),
            });
        }
        let json: Value = response.json().await?;
        // A session whose cookies went stale still gets answers, only
        // anonymous ones: catch that instead of showing someone else's feed.
        let signed_in = self.session.lock().expect("session lock").is_signed_in(now);
        if signed_in && logged_in(&json) == Some(false) {
            return Err(Error::SignedOut);
        }
        Ok(json)
    }

    /// The account the session is signed in to.
    pub async fn account(&self) -> Result<Account, Error> {
        let response = self.post("account/account_menu", Map::new()).await?;
        parse_account(&response).ok_or(Error::SignedOut)
    }

    /// The session to save when its cookies changed since the last call.
    pub fn take_changed_session(&self) -> Option<Vec<u8>> {
        let mut session = self.session.lock().expect("session lock");
        session.take_dirty().then(|| session.save())
    }

    /// The session's cookies in the Netscape format, for tools such as
    /// yt-dlp. The text holds secrets: write it only to private files.
    pub fn cookies_netscape(&self) -> String {
        self.session
            .lock()
            .expect("session lock")
            .jar()
            .to_netscape(unix_now())
    }

    /// The session to save, whether or not it changed.
    pub fn session_bytes(&self) -> Vec<u8> {
        self.session.lock().expect("session lock").save()
    }
}

/// The `logged_in` flag InnerTube reports in `responseContext`, when present.
fn logged_in(response: &Value) -> Option<bool> {
    response
        .pointer("/responseContext/serviceTrackingParams")?
        .as_array()?
        .iter()
        .filter_map(|service| service.get("params")?.as_array())
        .flatten()
        .find(|param| param.get("key").and_then(Value::as_str) == Some("logged_in"))
        .and_then(|param| param.get("value")?.as_str())
        .map(|value| value == "1")
}

impl Error {
    /// `true` when the server no longer accepts the session: sign in again.
    pub fn is_auth_failure(&self) -> bool {
        matches!(self, Self::SignedOut | Self::Status { status: 401, .. })
    }
}

/// The `error.message` of an InnerTube error body, cut to a readable length.
fn error_message(body: &str) -> String {
    let message = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
        .unwrap_or_else(|| body.to_owned());
    message.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logged_in_flag() {
        let response = |value: &str| {
            serde_json::json!({"responseContext": {"serviceTrackingParams": [
                {"service": "CSI", "params": [{"key": "c", "value": "WEB_REMIX"}]},
                {"service": "GFEEDBACK", "params": [{"key": "logged_in", "value": value}]}
            ]}})
        };
        assert_eq!(logged_in(&response("1")), Some(true));
        assert_eq!(logged_in(&response("0")), Some(false));
        assert_eq!(logged_in(&serde_json::json!({})), None);
    }

    #[test]
    fn auth_failures() {
        assert!(Error::SignedOut.is_auth_failure());
        assert!(
            Error::Status {
                status: 401,
                message: String::new()
            }
            .is_auth_failure()
        );
        assert!(
            !Error::Status {
                status: 500,
                message: String::new()
            }
            .is_auth_failure()
        );
    }

    #[test]
    fn error_message_prefers_the_json_message() {
        assert_eq!(
            error_message(
                r#"{"error":{"code":400,"message":"Request contains an invalid argument.","status":"INVALID_ARGUMENT"}}"#
            ),
            "Request contains an invalid argument."
        );
        assert_eq!(error_message("plain failure"), "plain failure");
        assert_eq!(error_message(&"x".repeat(500)).len(), 200);
    }
}
