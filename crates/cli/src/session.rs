//! The CLI side of the session: the shared [`orbis_core::SessionManager`]
//! does the work; commands run through [`with_client`].

use std::error::Error;
use std::sync::Arc;

use clap::ValueEnum;
pub use orbis_core::session::DOMAINS;
use orbis_core::{SessionManager, SignedIn};
use ytm::Client;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Browser {
    Chrome,
    Chromium,
    Brave,
    Vivaldi,
    Firefox,
}

impl From<Browser> for orbis_core::Browser {
    fn from(browser: Browser) -> Self {
        match browser {
            Browser::Chrome => Self::Chrome,
            Browser::Chromium => Self::Chromium,
            Browser::Brave => Self::Brave,
            Browser::Vivaldi => Self::Vivaldi,
            Browser::Firefox => Self::Firefox,
        }
    }
}

/// Signs in from `browser` and saves the session.
pub async fn login(
    browser: Browser,
    profile: Option<String>,
) -> Result<(ytm::Account, desktop::browser_cookies::Imported), Box<dyn Error>> {
    let (signed_in, imported) = SignedIn::from_browser(browser.into(), profile).await?;
    let account = signed_in.account.clone();
    SessionManager::login(signed_in).await?;
    Ok((account, imported))
}

/// Signs in through the Google sign-in window and saves the session.
pub async fn login_window() -> Result<ytm::Account, Box<dyn Error>> {
    let signed_in = SignedIn::from_window(&orbis_core::login_program()).await?;
    let account = signed_in.account.clone();
    SessionManager::login(signed_in).await?;
    Ok(account)
}

pub async fn logout() -> Result<(), Box<dyn Error>> {
    Ok(SessionManager::logout().await?)
}

/// Runs `op` with a client over the saved session; a stale session from a
/// browser signs in again from it. Rotated cookies are saved before returning.
pub async fn with_client<T, F, Fut>(op: F) -> Result<T, Box<dyn Error>>
where
    F: Fn(Arc<Client>) -> Fut,
    Fut: std::future::Future<Output = Result<T, ytm::Error>>,
{
    let session = SessionManager::open().await?;
    let result = session.call(op).await;
    session.flush().await?;
    Ok(result?)
}
