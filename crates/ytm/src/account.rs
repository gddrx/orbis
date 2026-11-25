//! The signed-in account, from the `account/account_menu` endpoint.

use serde_json::Value;

use crate::parse::{find_key, text};

/// The account a session is signed in to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub name: String,
    /// The channel handle, such as `@name`, when the account has a channel.
    pub handle: Option<String>,
    /// URL of the account picture.
    pub photo: Option<String>,
}

/// Reads the account from an `account/account_menu` response. `None` when the
/// response has no active account header: the session is signed out.
pub fn parse_account(response: &Value) -> Option<Account> {
    let header = find_key(response, "activeAccountHeaderRenderer")?;
    let name = text(header.get("accountName")?)?;
    let handle = header.get("channelHandle").and_then(text);
    let photo = header
        .get("accountPhoto")
        .and_then(|p| p.get("thumbnails"))
        .and_then(Value::as_array)
        .and_then(|t| t.last())
        .and_then(|t| t.get("url"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some(Account {
        name,
        handle,
        photo,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The shape of a real response, with made-up data.
    fn signed_in_response() -> Value {
        json!({
            "responseContext": {"serviceTrackingParams": []},
            "actions": [{
                "openPopupAction": {
                    "popup": {
                        "multiPageMenuRenderer": {
                            "header": {
                                "activeAccountHeaderRenderer": {
                                    "accountName": {"runs": [{"text": "Test User"}]},
                                    "accountPhoto": {"thumbnails": [
                                        {"url": "https://example.invalid/s48.jpg", "width": 48},
                                        {"url": "https://example.invalid/s88.jpg", "width": 88}
                                    ]},
                                    "channelHandle": {"runs": [{"text": "@testuser"}]}
                                }
                            },
                            "sections": []
                        }
                    }
                }
            }]
        })
    }

    #[test]
    fn reads_the_active_account() {
        assert_eq!(
            parse_account(&signed_in_response()),
            Some(Account {
                name: "Test User".into(),
                handle: Some("@testuser".into()),
                photo: Some("https://example.invalid/s88.jpg".into()),
            })
        );
    }

    #[test]
    fn signed_out_response_has_no_account() {
        assert_eq!(parse_account(&json!({"responseContext": {}})), None);
    }
}
