//! Helpers for reading InnerTube responses. The JSON is deeply nested and its
//! layout shifts between client versions, so lookups search by key instead of
//! walking fixed paths where that is safe.

use serde_json::Value;

pub mod items;
pub mod pages;

/// The first value under `key`, searching `value` depth-first.
pub fn find_key<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) => map
            .get(key)
            .or_else(|| map.values().find_map(|v| find_key(v, key))),
        Value::Array(items) => items.iter().find_map(|v| find_key(v, key)),
        _ => None,
    }
}

/// The text of an InnerTube text object: `{"runs":[{"text":…},…]}` or
/// `{"simpleText":…}`.
pub fn text(value: &Value) -> Option<String> {
    if let Some(simple) = value.get("simpleText").and_then(Value::as_str) {
        return Some(simple.to_owned());
    }
    let runs = value.get("runs")?.as_array()?;
    let joined: String = runs
        .iter()
        .filter_map(|run| run.get("text").and_then(Value::as_str))
        .collect();
    (!joined.is_empty()).then_some(joined)
}

/// The runs of a text object, or an empty slice.
pub fn runs(value: &Value) -> &[Value] {
    value
        .get("runs")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// URL of the largest thumbnail under `value`.
pub fn thumbnail(value: &Value) -> Option<String> {
    let thumbnails = find_key(value, "thumbnails")?.as_array()?;
    thumbnails
        .iter()
        .max_by_key(|t| t.get("width").and_then(Value::as_u64).unwrap_or(0))
        .and_then(|t| t.get("url"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The first string under `value` (depth-first) that satisfies `pred`.
pub fn find_string<'a>(value: &'a Value, pred: &dyn Fn(&str) -> bool) -> Option<&'a str> {
    match value {
        Value::String(s) if pred(s) => Some(s),
        Value::Object(map) => map.values().find_map(|v| find_string(v, pred)),
        Value::Array(items) => items.iter().find_map(|v| find_string(v, pred)),
        _ => None,
    }
}

/// `3:36` or `1:02:03` in seconds.
pub fn parse_duration(text: &str) -> Option<u32> {
    let parts: Vec<&str> = text.trim().split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let mut total = 0u32;
    for (i, part) in parts.iter().enumerate() {
        let digits_only = !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
        if !digits_only || (i > 0 && part.len() != 2) {
            return None;
        }
        total = total.checked_mul(60)?.checked_add(part.parse().ok()?)?;
    }
    Some(total)
}

/// The page type of a browse endpoint, such as `MUSIC_PAGE_TYPE_ALBUM`.
pub fn page_type(browse_endpoint: &Value) -> Option<&str> {
    browse_endpoint
        .pointer("/browseEndpointContextSupportedConfigs/browseEndpointContextMusicConfig/pageType")
        .and_then(Value::as_str)
}

/// The continuation token of a `continuationItemRenderer` or of a renderer's
/// `continuations` list.
pub fn continuation_token(value: &Value) -> Option<String> {
    if let Some(token) = value
        .pointer("/continuationItemRenderer/continuationEndpoint/continuationCommand/token")
        .and_then(Value::as_str)
    {
        return Some(token.to_owned());
    }
    value
        .get("continuations")?
        .as_array()?
        .iter()
        .find_map(|c| {
            c.pointer("/nextContinuationData/continuation")
                .and_then(Value::as_str)
        })
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finds_nested_keys() {
        let doc = json!({"a": [{"b": 1}, {"c": {"target": "here"}}]});
        assert_eq!(find_key(&doc, "target"), Some(&json!("here")));
        assert_eq!(find_key(&doc, "missing"), None);
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("3:36"), Some(216));
        assert_eq!(parse_duration("1:02:03"), Some(3723));
        assert_eq!(parse_duration("0:05"), Some(5));
        assert_eq!(parse_duration("Song"), None);
        assert_eq!(parse_duration("2,820"), None);
        assert_eq!(parse_duration("1:xx"), None);
    }

    #[test]
    fn largest_thumbnail_wins() {
        let doc = json!({"thumbnail": {"thumbnails": [
            {"url": "small", "width": 60}, {"url": "big", "width": 544}, {"url": "mid", "width": 226}
        ]}});
        assert_eq!(thumbnail(&doc).as_deref(), Some("big"));
    }

    #[test]
    fn continuation_tokens() {
        let item = json!({"continuationItemRenderer": {"continuationEndpoint": {"continuationCommand": {"token": "t1"}}}});
        assert_eq!(continuation_token(&item).as_deref(), Some("t1"));
        let list = json!({"continuations": [{"nextContinuationData": {"continuation": "t2"}}]});
        assert_eq!(continuation_token(&list).as_deref(), Some("t2"));
        assert_eq!(continuation_token(&json!({})), None);
    }

    #[test]
    fn reads_both_text_forms() {
        assert_eq!(text(&json!({"simpleText": "One"})).as_deref(), Some("One"));
        assert_eq!(
            text(&json!({"runs": [{"text": "Two "}, {"text": "parts"}]})).as_deref(),
            Some("Two parts")
        );
        assert_eq!(text(&json!({"runs": []})), None);
    }
}
