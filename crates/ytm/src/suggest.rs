//! Suggestions while a search is typed (`music/get_search_suggestions`):
//! the queries YouTube Music offers for what is typed so far.

use serde_json::{Map, Value, json};

use crate::client::{Client, Error};
use crate::parse::text;

impl Client {
    /// The queries YouTube Music suggests for `input`, best first.
    pub async fn search_suggestions(&self, input: &str) -> Result<Vec<String>, Error> {
        let mut body = Map::new();
        body.insert("input".to_owned(), json!(input));
        let response = self.post("music/get_search_suggestions", body).await?;
        Ok(parse_suggestions(&response))
    }
}

/// The suggested queries of a response. The answer has a second section
/// of artists and songs; those are left out: choosing a suggestion searches.
pub fn parse_suggestions(response: &Value) -> Vec<String> {
    response
        .get("contents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|section| section.pointer("/searchSuggestionsSectionRenderer/contents"))
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|item| item.get("searchSuggestionRenderer"))
        .filter_map(|suggestion| {
            let query = suggestion
                .pointer("/navigationEndpoint/searchEndpoint/query")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| suggestion.get("suggestion").and_then(text))?;
            let query = query.trim().to_owned();
            (!query.is_empty()).then_some(query)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A suggestion as the service sends it: the typed part plain, the rest
    /// bold.
    fn suggestion(typed: &str, rest: &str) -> Value {
        json!({"searchSuggestionRenderer": {
            "suggestion": {"runs": [{"text": typed}, {"text": rest, "bold": true}]},
            "navigationEndpoint": {"searchEndpoint": {"query": format!("{typed}{rest}")}}
        }})
    }

    #[test]
    fn suggested_queries_are_read_and_the_rest_left_out() {
        let response = json!({"contents": [
            {"searchSuggestionsSectionRenderer": {"contents": [
                suggestion("metal", "lica"),
                suggestion("metal", "lica nothing else matters"),
                {"searchSuggestionRenderer": {"suggestion": {"runs": [{"text": "metal"}]}}}
            ]}},
            {"searchSuggestionsSectionRenderer": {"contents": [
                {"musicResponsiveListItemRenderer": {"flexColumns": []}}
            ]}}
        ]});
        assert_eq!(
            parse_suggestions(&response),
            ["metallica", "metallica nothing else matters", "metal"]
        );
        assert!(parse_suggestions(&json!({})).is_empty());
    }
}
