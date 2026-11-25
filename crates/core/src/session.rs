//! The signed-in session: signing in through the Google sign-in window or
//! from a browser, keeping rotated cookies, and signing in again by itself
//! when a browser's session goes stale.
//!
//! The sign-in window (`orbis-login`) gives the player a session of its own,
//! which nobody else rotates. When the server stops accepting it, the user
//! signs in again.
//!
//! A session copied from a browser that keeps running stops being accepted
//! once the browser refreshes its Google cookies (12–14 minutes later, as
//! measured). So the source of the session is saved next to it, and a call
//! that the server refuses as signed out re-reads that browser's cookies and
//! runs once more. Concurrent calls share one re-sign-in.
//!
//! Rotated cookies are written to the wallet at most once a minute, and on
//! [`SessionManager::flush`].

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use desktop::browser_cookies::{self, BrowserCookie, ChromiumBrowser, ImportError, Imported};
use desktop::login_window::LoginWindowError;
use desktop::{SecretError, SecretServiceStore, SecretStore};
use serde::{Deserialize, Serialize};
use ytm::{Account, Client, ClientContext, CookieJar, Session, StoredCookie};

/// Key of the session in secret storage.
const SESSION_KEY: &str = "session";
const SESSION_LABEL: &str = "orbis — YouTube Music session";
/// Key of where the session came from.
const SOURCE_KEY: &str = "session-source";
const SOURCE_LABEL: &str = "orbis — source of the YouTube Music session";
/// Cookies taken from the browser or the sign-in window.
pub const DOMAINS: [&str; 2] = ["youtube.com", "google.com"];
/// Rotated cookies are saved at most this often.
const SAVE_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Browser {
    Chrome,
    Chromium,
    Brave,
    Vivaldi,
    Firefox,
}

impl Browser {
    pub const ALL: [Browser; 5] = [
        Browser::Chrome,
        Browser::Chromium,
        Browser::Brave,
        Browser::Vivaldi,
        Browser::Firefox,
    ];

    /// Whether the browser has a cookie database to take a sign-in from:
    /// the sign-in dialog offers only these.
    pub fn has_cookies(self) -> bool {
        match self {
            Browser::Firefox => browser_cookies::firefox::has_cookies(),
            Browser::Chrome => browser_cookies::chromium::has_cookies(ChromiumBrowser::Chrome),
            Browser::Chromium => browser_cookies::chromium::has_cookies(ChromiumBrowser::Chromium),
            Browser::Brave => browser_cookies::chromium::has_cookies(ChromiumBrowser::Brave),
            Browser::Vivaldi => browser_cookies::chromium::has_cookies(ChromiumBrowser::Vivaldi),
        }
    }
}

/// Where the session's cookies come from. Saved as JSON: an older save,
/// `{"browser":"chrome","profile":null}`, reads as [`Source::Browser`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Source {
    /// A browser where YouTube Music is open; a stale session reads the
    /// browser's cookies again.
    Browser {
        browser: Browser,
        profile: Option<String>,
    },
    /// The sign-in window: the player's own session.
    Window { window: Window },
}

/// The sign-in window a session came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Window {
    Google,
}

/// The sign-in window's program: `$ORBIS_LOGIN_BIN`, else `orbis-login`
/// next to the running program, else `orbis-login` from `PATH`.
pub fn login_program() -> PathBuf {
    if let Some(path) = std::env::var_os("ORBIS_LOGIN_BIN") {
        return PathBuf::from(path);
    }
    std::env::current_exe()
        .ok()
        .map(|exe| exe.with_file_name("orbis-login"))
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from("orbis-login"))
}

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("not signed in; sign in first")]
    NotSignedIn,
    #[error(
        "the browser has no YouTube sign-in (no SAPISID cookie); sign in to music.youtube.com there first"
    )]
    BrowserSignedOut,
    #[error("the sign-in window gave no YouTube sign-in (no SAPISID cookie)")]
    WindowSignedOut,
    #[error("the session of the sign-in window is no longer accepted; sign in again")]
    SessionExpired,
    #[error(transparent)]
    LoginWindow(#[from] LoginWindowError),
    #[error(transparent)]
    Api(#[from] ytm::Error),
    #[error(transparent)]
    Import(#[from] ImportError),
    #[error(transparent)]
    Secret(#[from] SecretError),
    #[error("stored session: {0}")]
    Stored(#[from] serde_json::Error),
}

impl CoreError {
    /// `true` when the user has to sign in (again) for calls to work.
    pub fn needs_sign_in(&self) -> bool {
        match self {
            Self::NotSignedIn
            | Self::BrowserSignedOut
            | Self::WindowSignedOut
            | Self::SessionExpired => true,
            Self::Api(e) => e.is_auth_failure(),
            _ => false,
        }
    }
}

/// Language and country from the user's locale.
pub fn context() -> ClientContext {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty());
    ClientContext::from_locale(locale.as_deref())
}

fn stored(cookie: &BrowserCookie) -> StoredCookie {
    StoredCookie {
        name: cookie.name.clone(),
        value: cookie.value.clone(),
        domain: cookie.host.trim_start_matches('.').to_ascii_lowercase(),
        host_only: !cookie.host.starts_with('.'),
        path: if cookie.path.starts_with('/') {
            cookie.path.clone()
        } else {
            "/".to_owned()
        },
        expires: cookie.expires,
        secure: cookie.secure,
        http_only: cookie.http_only,
    }
}

/// A session over `cookies`, or `None` when they hold no sign-in.
fn session_of(cookies: &[BrowserCookie]) -> Option<Session> {
    let mut jar = CookieJar::default();
    for cookie in cookies {
        jar.insert(stored(cookie));
    }
    let session = Session::new(jar, context());
    session.is_signed_in(ytm::unix_now()).then_some(session)
}

/// Reads the browser's cookies into a new session.
async fn import(browser: Browser, profile: Option<&str>) -> Result<(Session, Imported), CoreError> {
    let imported = match browser {
        Browser::Firefox => {
            browser_cookies::firefox::import(profile.map(PathBuf::from).as_deref(), &DOMAINS)?
        }
        chromium => {
            let browser = match chromium {
                Browser::Chrome => ChromiumBrowser::Chrome,
                Browser::Chromium => ChromiumBrowser::Chromium,
                Browser::Brave => ChromiumBrowser::Brave,
                _ => ChromiumBrowser::Vivaldi,
            };
            browser_cookies::chromium::import(browser, profile, &DOMAINS).await?
        }
    };
    let session = session_of(&imported.cookies).ok_or(CoreError::BrowserSignedOut)?;
    Ok((session, imported))
}

/// A session that has just signed in and passed the account check: hand it
/// to [`SessionManager::login`] or [`SessionManager::replace`].
#[derive(Debug)]
pub struct SignedIn {
    client: Client,
    source: Source,
    pub account: Account,
}

impl SignedIn {
    /// Signs in with the cookies of `browser`.
    pub async fn from_browser(
        browser: Browser,
        profile: Option<String>,
    ) -> Result<(Self, Imported), CoreError> {
        let (session, imported) = import(browser, profile.as_deref()).await?;
        let signed_in = Self::checked(session, Source::Browser { browser, profile }).await?;
        Ok((signed_in, imported))
    }

    /// Signs in through the Google sign-in window `program` (see
    /// [`login_program`]); the user signs in there. Dropping the future
    /// closes the window.
    pub async fn from_window(program: &Path) -> Result<Self, CoreError> {
        let cookies = desktop::login_window::sign_in(program, &DOMAINS).await?;
        let session = session_of(&cookies).ok_or(CoreError::WindowSignedOut)?;
        let source = Source::Window {
            window: Window::Google,
        };
        Self::checked(session, source).await
    }

    async fn checked(session: Session, source: Source) -> Result<Self, CoreError> {
        let client = Client::new(session)?;
        let account = client.account().await?;
        Ok(Self {
            client,
            source,
            account,
        })
    }

    async fn save(&self, store: &SecretServiceStore) -> Result<(), CoreError> {
        store
            .save(SESSION_KEY, SESSION_LABEL, &self.client.session_bytes())
            .await?;
        store
            .save(SOURCE_KEY, SOURCE_LABEL, &serde_json::to_vec(&self.source)?)
            .await?;
        Ok(())
    }
}

/// The session shared by everything that talks to YouTube Music.
#[derive(Debug)]
pub struct SessionManager {
    store: SecretServiceStore,
    client: RwLock<Arc<Client>>,
    /// Serializes re-sign-ins; held across the browser import.
    refresh: tokio::sync::Mutex<()>,
    /// Rotated cookies not written yet, and when the last write was.
    pending: Mutex<(Option<Vec<u8>>, Instant)>,
    /// `true` once the server refused the session and it could not be
    /// renewed: the user has to sign in.
    signed_out: tokio::sync::watch::Sender<bool>,
    /// The user signed out: nothing is asked or saved until a new sign-in.
    left: std::sync::atomic::AtomicBool,
}

impl SessionManager {
    /// The saved session.
    pub async fn open() -> Result<Self, CoreError> {
        let store = SecretServiceStore::connect().await?;
        let Some(saved) = store.load(SESSION_KEY).await? else {
            return Err(CoreError::NotSignedIn);
        };
        let client = Client::new(Session::restore(&saved, context())?)?;
        Ok(Self::new(store, client))
    }

    fn new(store: SecretServiceStore, client: Client) -> Self {
        Self {
            store,
            client: RwLock::new(Arc::new(client)),
            refresh: tokio::sync::Mutex::new(()),
            pending: Mutex::new((None, Instant::now())),
            signed_out: tokio::sync::watch::Sender::new(false),
            left: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn has_left(&self) -> bool {
        self.left.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Becomes `true` when the server stops accepting the session and it
    /// cannot be renewed, and `false` again after [`SessionManager::replace`].
    pub fn signed_out(&self) -> tokio::sync::watch::Receiver<bool> {
        self.signed_out.subscribe()
    }

    /// Saves a session that has just signed in, and its source.
    pub async fn login(signed_in: SignedIn) -> Result<Self, CoreError> {
        let store = SecretServiceStore::connect().await?;
        signed_in.save(&store).await?;
        Ok(Self::new(store, signed_in.client))
    }

    /// Saves a session that has just signed in in place of the current one,
    /// for every call from now on.
    pub async fn replace(&self, signed_in: SignedIn) -> Result<(), CoreError> {
        let _one_at_a_time = self.refresh.lock().await;
        signed_in.save(&self.store).await?;
        *self.client.write().expect("client lock") = Arc::new(signed_in.client);
        *self.pending.lock().expect("pending lock") = (None, Instant::now());
        self.left.store(false, std::sync::atomic::Ordering::SeqCst);
        self.signed_out.send_replace(false);
        Ok(())
    }

    /// Signs the user out: the saved session and its source are removed,
    /// and calls fail with [`CoreError::NotSignedIn`] (without a request)
    /// until [`SessionManager::replace`] brings a new sign-in.
    pub async fn sign_out(&self) -> Result<(), CoreError> {
        let _one_at_a_time = self.refresh.lock().await;
        self.left.store(true, std::sync::atomic::Ordering::SeqCst);
        *self.pending.lock().expect("pending lock") = (None, Instant::now());
        self.signed_out.send_replace(true);
        self.store.delete(SESSION_KEY).await?;
        self.store.delete(SOURCE_KEY).await?;
        Ok(())
    }

    /// Where the saved session comes from; `None` without one.
    pub async fn source(&self) -> Result<Option<Source>, CoreError> {
        if self.has_left() {
            return Ok(None);
        }
        match self.store.load(SOURCE_KEY).await? {
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            None => Ok(None),
        }
    }

    /// The signed-in account.
    pub async fn account(&self) -> Result<Account, CoreError> {
        self.call(|c| async move { c.account().await }).await
    }

    /// Removes the saved session and its source.
    pub async fn logout() -> Result<(), CoreError> {
        let store = SecretServiceStore::connect().await?;
        store.delete(SESSION_KEY).await?;
        store.delete(SOURCE_KEY).await?;
        Ok(())
    }

    fn current(&self) -> Arc<Client> {
        Arc::clone(&self.client.read().expect("client lock"))
    }

    /// The current cookies in the Netscape format, for yt-dlp. They hold
    /// secrets: write them only to private files.
    pub fn cookies(&self) -> String {
        if self.has_left() {
            return String::new();
        }
        self.current().cookies_netscape()
    }

    /// Runs `op` with the client. When the server refuses the session as
    /// signed out, signs in again from the saved browser and runs `op` once
    /// more; a session of the sign-in window fails with
    /// [`CoreError::SessionExpired`] instead.
    ///
    /// `op` takes the client by `Arc` rather than by reference, so that its
    /// future can be `Send` and run in spawned tasks.
    pub async fn call<T, F, Fut>(&self, op: F) -> Result<T, CoreError>
    where
        F: Fn(Arc<Client>) -> Fut,
        Fut: std::future::Future<Output = Result<T, ytm::Error>>,
    {
        if self.has_left() {
            return Err(CoreError::NotSignedIn);
        }
        let client = self.current();
        match op(Arc::clone(&client)).await {
            Err(e) if e.is_auth_failure() => {
                let fresh = match self.renew(&client).await {
                    Ok(fresh) => fresh,
                    Err(e) => {
                        if e.needs_sign_in() {
                            self.signed_out.send_replace(true);
                        }
                        return Err(e);
                    }
                };
                let result = op(Arc::clone(&fresh)).await;
                if matches!(&result, Err(e) if e.is_auth_failure()) {
                    self.signed_out.send_replace(true);
                }
                self.note_changes(&fresh).await?;
                Ok(result?)
            }
            result => {
                self.note_changes(&client).await?;
                Ok(result?)
            }
        }
    }

    /// Replaces `stale` with a session from the browser, unless another call
    /// already did. A session of the sign-in window cannot be renewed here.
    async fn renew(&self, stale: &Arc<Client>) -> Result<Arc<Client>, CoreError> {
        let _one_at_a_time = self.refresh.lock().await;
        let current = self.current();
        if !Arc::ptr_eq(&current, stale) {
            return Ok(current);
        }
        let Some(source) = self.store.load(SOURCE_KEY).await? else {
            return Err(CoreError::NotSignedIn);
        };
        let Source::Browser { browser, profile } = serde_json::from_slice(&source)? else {
            return Err(CoreError::SessionExpired);
        };
        let (session, _) = import(browser, profile.as_deref()).await?;
        let fresh = Arc::new(Client::new(session)?);
        *self.client.write().expect("client lock") = Arc::clone(&fresh);
        self.store
            .save(SESSION_KEY, SESSION_LABEL, &fresh.session_bytes())
            .await?;
        *self.pending.lock().expect("pending lock") = (None, Instant::now());
        Ok(fresh)
    }

    /// Keeps rotated cookies and writes them when a minute has passed.
    async fn note_changes(&self, client: &Client) -> Result<(), CoreError> {
        if self.has_left() {
            return Ok(());
        }
        let due = {
            let mut pending = self.pending.lock().expect("pending lock");
            if let Some(bytes) = client.take_changed_session() {
                pending.0 = Some(bytes);
            }
            if pending.0.is_some() && pending.1.elapsed() >= SAVE_INTERVAL {
                pending.1 = Instant::now();
                pending.0.take()
            } else {
                None
            }
        };
        if let Some(bytes) = due {
            self.store.save(SESSION_KEY, SESSION_LABEL, &bytes).await?;
        }
        Ok(())
    }

    /// Writes cookies that changed since the last write; call before exit.
    pub async fn flush(&self) -> Result<(), CoreError> {
        if self.has_left() {
            return Ok(());
        }
        let client = self.current();
        let bytes = {
            let mut pending = self.pending.lock().expect("pending lock");
            if let Some(bytes) = client.take_changed_session() {
                pending.0 = Some(bytes);
            }
            pending.1 = Instant::now();
            pending.0.take()
        };
        if let Some(bytes) = bytes {
            self.store.save(SESSION_KEY, SESSION_LABEL, &bytes).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_round_trips_as_json() {
        let source = Source::Browser {
            browser: Browser::Chrome,
            profile: Some("Profile 2".into()),
        };
        let json = serde_json::to_string(&source).unwrap();
        assert_eq!(json, r#"{"browser":"chrome","profile":"Profile 2"}"#);
        assert_eq!(serde_json::from_str::<Source>(&json).unwrap(), source);

        let window = Source::Window {
            window: Window::Google,
        };
        let json = serde_json::to_string(&window).unwrap();
        assert_eq!(json, r#"{"window":"google"}"#);
        assert_eq!(serde_json::from_str::<Source>(&json).unwrap(), window);
    }

    #[test]
    fn sources_saved_before_the_window_still_read() {
        for (json, profile) in [
            (r#"{"browser":"chrome","profile":null}"#, None),
            (r#"{"browser":"chrome"}"#, None),
            (
                r#"{"browser":"chrome","profile":"Default"}"#,
                Some("Default"),
            ),
        ] {
            assert_eq!(
                serde_json::from_str::<Source>(json).unwrap(),
                Source::Browser {
                    browser: Browser::Chrome,
                    profile: profile.map(str::to_owned),
                },
                "{json}"
            );
        }
        assert!(serde_json::from_str::<Source>(r#"{"window":"other"}"#).is_err());
    }

    #[test]
    fn cookies_without_sapisid_are_no_session() {
        let cookie = |name: &str| BrowserCookie {
            name: name.into(),
            value: "v".into(),
            host: ".youtube.com".into(),
            path: "/".into(),
            expires: None,
            secure: true,
            http_only: true,
        };
        assert!(session_of(&[cookie("SID"), cookie("YSC")]).is_none());
        assert!(session_of(&[cookie("SID"), cookie("SAPISID")]).is_some());
    }

    #[test]
    fn errors_that_need_a_sign_in() {
        assert!(CoreError::NotSignedIn.needs_sign_in());
        assert!(CoreError::SessionExpired.needs_sign_in());
        assert!(CoreError::Api(ytm::Error::SignedOut).needs_sign_in());
        assert!(!CoreError::LoginWindow(LoginWindowError::Cancelled).needs_sign_in());
    }

    #[test]
    fn browser_cookies_become_jar_cookies() {
        let domain = stored(&BrowserCookie {
            name: "SID".into(),
            value: "v".into(),
            host: ".YouTube.com".into(),
            path: "".into(),
            expires: Some(5),
            secure: true,
            http_only: true,
        });
        assert_eq!(
            (
                domain.domain.as_str(),
                domain.host_only,
                domain.path.as_str()
            ),
            ("youtube.com", false, "/")
        );
        let host = stored(&BrowserCookie {
            name: "YSC".into(),
            value: "v".into(),
            host: "music.youtube.com".into(),
            path: "/".into(),
            expires: None,
            secure: true,
            http_only: false,
        });
        assert!(host.host_only);
    }
}
