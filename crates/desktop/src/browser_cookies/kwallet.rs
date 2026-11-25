//! Reads one password from KWallet over its own D-Bus API (`org.kde.KWallet`).
//! Chromium-based browsers keep their cookie key there on KDE Plasma.

#[zbus::proxy(
    interface = "org.kde.KWallet",
    default_service = "org.kde.kwalletd6",
    default_path = "/modules/kwalletd6"
)]
trait KWallet {
    #[zbus(name = "networkWallet")]
    fn network_wallet(&self) -> zbus::Result<String>;

    #[zbus(name = "open")]
    fn open(&self, wallet: &str, window_id: i64, app_id: &str) -> zbus::Result<i32>;

    #[zbus(name = "readPassword")]
    fn read_password(
        &self,
        handle: i32,
        folder: &str,
        key: &str,
        app_id: &str,
    ) -> zbus::Result<String>;

    #[zbus(name = "close")]
    fn close(&self, handle: i32, force: bool, app_id: &str) -> zbus::Result<i32>;
}

/// The application name KWallet shows when it asks for access.
const APP_ID: &str = "orbis";

/// The password stored under `folder`/`key` in the network wallet, or `None`
/// when KWallet is not running or the entry is empty.
pub(super) async fn read_password(folder: &str, key: &str) -> Result<Option<String>, String> {
    let connection = zbus::Connection::session()
        .await
        .map_err(|e| e.to_string())?;
    let wallet = match KWalletProxy::new(&connection).await {
        Ok(proxy) => proxy,
        Err(_) => return Ok(None),
    };
    let Ok(name) = wallet.network_wallet().await else {
        return Ok(None);
    };
    let handle = wallet
        .open(&name, 0, APP_ID)
        .await
        .map_err(|e| e.to_string())?;
    if handle < 0 {
        return Err(format!("KWallet refused to open wallet {name}"));
    }
    let password = wallet.read_password(handle, folder, key, APP_ID).await;
    let _ = wallet.close(handle, false, APP_ID).await;
    let password = password.map_err(|e| e.to_string())?;
    Ok((!password.is_empty()).then_some(password))
}
