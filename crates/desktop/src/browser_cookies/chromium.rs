//! Cookies of Chromium-based browsers on Linux: Chrome, Chromium, Brave, Vivaldi.
//!
//! Values are AES-128-CBC encrypted. A `v10` prefix means the fixed password
//! `peanuts`; `v11` means a password the browser keeps in the desktop keyring
//! (KWallet folder `<Name> Keys`, entry `<Name> Safe Storage`, or a Secret
//! Service item with `application = <name>`). The key is
//! PBKDF2-HMAC-SHA1(password, `saltysalt`, 1 round, 16 bytes), the IV is 16
//! spaces. Since database version 24 the plaintext starts with SHA-256 of the
//! cookie's host, which is checked and dropped.

use std::path::{Path, PathBuf};

use aes::cipher::{KeyIvInit, block_padding::Pkcs7};
use cbc::cipher::BlockModeDecrypt;
use rusqlite::{Connection, OpenFlags};
use sha1::Sha1;
use sha2::{Digest, Sha256};

use super::{BrowserCookie, ImportError, Imported, config_dir, host_in_domains, private_copy};

/// Seconds between 1601-01-01 (Chromium's epoch) and 1970-01-01.
const WINDOWS_TO_UNIX_EPOCH: i64 = 11_644_473_600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromiumBrowser {
    Chrome,
    Chromium,
    Brave,
    Vivaldi,
}

impl ChromiumBrowser {
    /// The browser's directory under `~/.config`.
    pub fn config_subdir(self) -> &'static str {
        match self {
            Self::Chrome => "google-chrome",
            Self::Chromium => "chromium",
            Self::Brave => "BraveSoftware/Brave-Browser",
            Self::Vivaldi => "vivaldi",
        }
    }

    /// The name the browser uses for its keyring entries.
    fn keyring_name(self) -> &'static str {
        match self {
            Self::Chrome => "Chrome",
            Self::Chromium => "Chromium",
            Self::Brave => "Brave",
            Self::Vivaldi => "Vivaldi",
        }
    }
}

/// Imports the cookies of `domains` from `profile` of `browser` (the profile
/// the browser used last when `None`).
pub async fn import(
    browser: ChromiumBrowser,
    profile: Option<&str>,
    domains: &[&str],
) -> Result<Imported, ImportError> {
    let not_found = |detail: String| ImportError::ProfileNotFound {
        browser: browser.keyring_name().to_owned(),
        detail,
    };
    let root = config_dir()
        .ok_or_else(|| not_found("no home directory".into()))?
        .join(browser.config_subdir());
    let profile = match profile {
        Some(profile) => profile.to_owned(),
        None => last_used_profile(&root),
    };
    let db = cookie_db(&root.join(&profile))
        .ok_or_else(|| not_found(format!("no cookie database in profile {profile}")))?;

    let (_dir, copy) = private_copy(&db)?;
    let rows = read_rows(&copy, domains)?;

    let password = if rows.iter().any(|row| row.encrypted.starts_with(b"v11")) {
        Some(keyring_password(browser).await?)
    } else {
        None
    };
    Ok(decrypt_rows(rows, password.as_deref()))
}

/// Whether `browser` has a cookie database in the profile it used last:
/// without one there is no sign-in to take.
pub fn has_cookies(browser: ChromiumBrowser) -> bool {
    config_dir().is_some_and(|config| root_has_cookies(&config.join(browser.config_subdir())))
}

fn root_has_cookies(root: &Path) -> bool {
    cookie_db(&root.join(last_used_profile(root))).is_some()
}

/// `profile.last_used` from `Local State`, or `Default`.
fn last_used_profile(root: &Path) -> String {
    std::fs::read(root.join("Local State"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|state| state["profile"]["last_used"].as_str().map(str::to_owned))
        .filter(|name| !name.is_empty() && !name.contains('/'))
        .unwrap_or_else(|| "Default".to_owned())
}

/// `Network/Cookies` (Chromium 96 and later) or `Cookies`.
fn cookie_db(profile_dir: &Path) -> Option<PathBuf> {
    [
        profile_dir.join("Network").join("Cookies"),
        profile_dir.join("Cookies"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

async fn keyring_password(browser: ChromiumBrowser) -> Result<String, ImportError> {
    let name = browser.keyring_name();
    match super::kwallet::read_password(&format!("{name} Keys"), &format!("{name} Safe Storage"))
        .await
    {
        Ok(Some(password)) => return Ok(password),
        Ok(None) => {}
        Err(e) => return Err(ImportError::Key(format!("KWallet: {e}"))),
    }
    // GNOME and other desktops: a Secret Service item of the browser.
    let keyring = oo7::Keyring::new()
        .await
        .map_err(|e| ImportError::Key(e.to_string()))?;
    let attributes = [("application", name.to_ascii_lowercase())];
    let items = keyring
        .search_items(&attributes)
        .await
        .map_err(|e| ImportError::Key(e.to_string()))?;
    let item = items.first().ok_or_else(|| {
        ImportError::Key(format!(
            "no “{name} Safe Storage” entry in KWallet or the Secret Service"
        ))
    })?;
    let secret = item
        .secret()
        .await
        .map_err(|e| ImportError::Key(e.to_string()))?;
    String::from_utf8(secret.as_bytes().to_vec())
        .map_err(|_| ImportError::Key("the stored key is not text".into()))
}

/// One row of the `cookies` table, still encrypted.
struct Row {
    cookie: BrowserCookie,
    encrypted: Vec<u8>,
    version: i64,
}

fn read_rows(db: &Path, domains: &[&str]) -> Result<Vec<Row>, ImportError> {
    let connection = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let version: i64 = connection
        .query_row("SELECT value FROM meta WHERE key = 'version'", [], |row| {
            row.get::<_, String>(0)
        })
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut statement = connection.prepare(
        "SELECT host_key, name, value, encrypted_value, path, expires_utc, is_secure, is_httponly \
         FROM cookies",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Vec<u8>>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, bool>(6)?,
            row.get::<_, bool>(7)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (host, name, value, encrypted, path, expires_utc, secure, http_only) = row?;
        if !host_in_domains(&host, domains) {
            continue;
        }
        out.push(Row {
            cookie: BrowserCookie {
                name,
                value,
                host,
                path,
                expires: chromium_time_to_unix(expires_utc),
                secure,
                http_only,
            },
            encrypted,
            version,
        });
    }
    Ok(out)
}

fn decrypt_rows(rows: Vec<Row>, password: Option<&str>) -> Imported {
    let v10 = derive_key(b"peanuts");
    let v11 = password.map(|p| derive_key(p.as_bytes()));
    // Some setups store `v11` values under an empty password.
    let empty = derive_key(b"");
    let mut imported = Imported::default();
    for Row {
        mut cookie,
        encrypted,
        version,
    } in rows
    {
        if encrypted.is_empty() {
            imported.cookies.push(cookie);
            continue;
        }
        let value = match encrypted.get(..3) {
            Some(b"v10") => decrypt_value(&encrypted[3..], &v10, &cookie.host, version),
            Some(b"v11") => v11
                .as_ref()
                .and_then(|key| decrypt_value(&encrypted[3..], key, &cookie.host, version))
                .or_else(|| decrypt_value(&encrypted[3..], &empty, &cookie.host, version)),
            _ => None,
        };
        match value {
            Some(value) => {
                cookie.value = value;
                imported.cookies.push(cookie);
            }
            None => imported.undecryptable += 1,
        }
    }
    imported
}

/// PBKDF2-HMAC-SHA1 with the salt `saltysalt`, one round, 16 bytes.
fn derive_key(password: &[u8]) -> [u8; 16] {
    let mut key = [0u8; 16];
    pbkdf2::pbkdf2_hmac::<Sha1>(password, b"saltysalt", 1, &mut key);
    key
}

const IV: [u8; 16] = [b' '; 16];

/// Decrypts one value (without its `v1x` prefix). Returns `None` on a wrong
/// key: bad padding, a host hash that does not match, or text that is not UTF-8.
fn decrypt_value(ciphertext: &[u8], key: &[u8; 16], host: &str, version: i64) -> Option<String> {
    let decryptor = cbc::Decryptor::<aes::Aes128>::new_from_slices(key, &IV).ok()?;
    let mut buffer = ciphertext.to_vec();
    let plain = decryptor
        .decrypt_padded::<Pkcs7>(&mut buffer)
        .ok()?
        .to_vec();
    let plain = if version >= 24 {
        let (hash, rest) = plain.split_at_checked(32)?;
        if hash != Sha256::digest(host.as_bytes()).as_slice() {
            return None;
        }
        rest.to_vec()
    } else {
        plain
    };
    String::from_utf8(plain).ok()
}

/// Microseconds since 1601-01-01 to Unix seconds; 0 means a session cookie.
fn chromium_time_to_unix(micros: i64) -> Option<i64> {
    (micros > 0).then(|| micros / 1_000_000 - WINDOWS_TO_UNIX_EPOCH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cbc::cipher::BlockModeEncrypt;

    #[test]
    fn a_browser_has_cookies_when_its_last_profile_has_a_database() {
        let root = tempfile::tempdir().unwrap();
        assert!(!root_has_cookies(root.path()), "not installed");
        std::fs::create_dir_all(root.path().join("Default")).unwrap();
        assert!(!root_has_cookies(root.path()), "no database yet");

        std::fs::create_dir_all(root.path().join("Profile 2/Network")).unwrap();
        std::fs::write(root.path().join("Profile 2/Network/Cookies"), b"db").unwrap();
        assert!(
            !root_has_cookies(root.path()),
            "only in a profile not used last"
        );
        std::fs::write(
            root.path().join("Local State"),
            br#"{"profile":{"last_used":"Profile 2"}}"#,
        )
        .unwrap();
        assert!(root_has_cookies(root.path()));
    }

    fn encrypt(plain: &[u8], key: &[u8; 16]) -> Vec<u8> {
        let mut buffer = plain.to_vec();
        buffer.resize(plain.len() + 16, 0);
        let len = cbc::Encryptor::<aes::Aes128>::new_from_slices(key, &IV)
            .unwrap()
            .encrypt_padded::<Pkcs7>(&mut buffer, plain.len())
            .unwrap()
            .len();
        buffer.truncate(len);
        buffer
    }

    fn v24_plaintext(host: &str, value: &str) -> Vec<u8> {
        let mut plain = Sha256::digest(host.as_bytes()).to_vec();
        plain.extend_from_slice(value.as_bytes());
        plain
    }

    #[test]
    fn key_derivation_matches_chromium() {
        // The well-known v10 key: PBKDF2-HMAC-SHA1("peanuts", "saltysalt", 1, 16).
        assert_eq!(
            derive_key(b"peanuts"),
            [
                0xfd, 0x62, 0x1f, 0xe5, 0xa2, 0xb4, 0x02, 0x53, 0x9d, 0xfa, 0x14, 0x7c, 0xa9, 0x27,
                0x27, 0x78
            ]
        );
    }

    #[test]
    fn v24_value_needs_the_right_host_hash() {
        let key = derive_key(b"wallet-password");
        let cipher = encrypt(&v24_plaintext(".youtube.com", "abc"), &key);
        assert_eq!(
            decrypt_value(&cipher, &key, ".youtube.com", 24).as_deref(),
            Some("abc")
        );
        assert_eq!(decrypt_value(&cipher, &key, ".google.com", 24), None);
        assert_eq!(
            decrypt_value(&cipher, &derive_key(b"wrong"), ".youtube.com", 24),
            None
        );
    }

    #[test]
    fn old_databases_have_no_hash() {
        let key = derive_key(b"peanuts");
        let cipher = encrypt(b"plain", &key);
        assert_eq!(
            decrypt_value(&cipher, &key, ".youtube.com", 23).as_deref(),
            Some("plain")
        );
    }

    #[test]
    fn chromium_epoch_converts() {
        assert_eq!(chromium_time_to_unix(0), None);
        // 2023-11-14 22:13:20 UTC.
        assert_eq!(
            chromium_time_to_unix((1_700_000_000 + WINDOWS_TO_UNIX_EPOCH) * 1_000_000),
            Some(1_700_000_000)
        );
    }

    #[test]
    fn reads_a_cookie_database() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("Cookies");
        let connection = Connection::open(&db).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
                 INSERT INTO meta VALUES ('version', '24');
                 CREATE TABLE cookies (host_key TEXT, name TEXT, value TEXT, encrypted_value BLOB,
                   path TEXT, expires_utc INTEGER, is_secure INTEGER, is_httponly INTEGER);",
            )
            .unwrap();
        let password_key = derive_key(b"wallet-password");
        let insert = |host: &str, name: &str, encrypted: Vec<u8>| {
            connection
                .execute(
                    "INSERT INTO cookies VALUES (?1, ?2, '', ?3, '/', ?4, 1, 1)",
                    rusqlite::params![
                        host,
                        name,
                        encrypted,
                        (1_800_000_000 + WINDOWS_TO_UNIX_EPOCH) * 1_000_000
                    ],
                )
                .unwrap();
        };
        let v11 = |host: &str, value: &str| {
            let mut blob = b"v11".to_vec();
            blob.extend(encrypt(&v24_plaintext(host, value), &password_key));
            blob
        };
        insert(".youtube.com", "SAPISID", v11(".youtube.com", "s1"));
        insert("music.youtube.com", "YSC", v11("music.youtube.com", "y1"));
        insert(".example.com", "OTHER", v11(".example.com", "x"));
        // Encrypted with another key: counted, not imported.
        let mut foreign = b"v11".to_vec();
        foreign.extend(encrypt(
            &v24_plaintext(".google.com", "g"),
            &derive_key(b"other"),
        ));
        insert(".google.com", "NID", foreign);
        drop(connection);

        let rows = read_rows(&db, &["youtube.com", "google.com"]).unwrap();
        assert_eq!(rows.len(), 3);
        let imported = decrypt_rows(rows, Some("wallet-password"));
        assert_eq!(imported.undecryptable, 1);
        let mut found: Vec<(&str, &str, &str)> = imported
            .cookies
            .iter()
            .map(|c| (c.host.as_str(), c.name.as_str(), c.value.as_str()))
            .collect();
        found.sort();
        assert_eq!(
            found,
            [
                (".youtube.com", "SAPISID", "s1"),
                ("music.youtube.com", "YSC", "y1")
            ]
        );
        assert!(
            imported
                .cookies
                .iter()
                .all(|c| c.expires == Some(1_800_000_000))
        );
    }

    #[test]
    fn last_used_profile_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(last_used_profile(dir.path()), "Default");
        std::fs::write(
            dir.path().join("Local State"),
            br#"{"profile":{"last_used":"Profile 2"}}"#,
        )
        .unwrap();
        assert_eq!(last_used_profile(dir.path()), "Profile 2");
        std::fs::write(
            dir.path().join("Local State"),
            br#"{"profile":{"last_used":"../escape"}}"#,
        )
        .unwrap();
        assert_eq!(last_used_profile(dir.path()), "Default");
    }
}
