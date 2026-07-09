use crate::{
    pipeline::{clean_text, now_unix_seconds, stable_id, RawItem, SourceKind},
    sources::SourceResult,
};
use feed_rs::parser;
use reqwest::Client;

const TECHMEME_FEED_URL: &str = "https://www.techmeme.com/feed.xml";

pub async fn fetch(client: &Client) -> SourceResult<Vec<RawItem>> {
    let body = client
        .get(TECHMEME_FEED_URL)
        .send()
        .await
        .map_err(|error| format!("Techmeme RSS request failed: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Techmeme RSS returned an error: {error}"))?
        .text()
        .await
        .map_err(|error| format!("Techmeme RSS text failed: {error}"))?;

    let feed = parser::parse(body.as_bytes())
        .map_err(|error| format!("Techmeme RSS parse failed: {error}"))?;
    let source = feed
        .title
        .as_ref()
        .map(|title| clean_text(&title.content))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| "Techmeme".into());
    let now = now_unix_seconds();

    let items = feed
        .entries
        .into_iter()
        .take(30)
        .filter_map(|entry| {
            let title = entry
                .title
                .as_ref()
                .map(|title| clean_text(&title.content))?;
            let url = entry.links.first().map(|link| link.href.clone())?;
            let timestamp = entry
                .published
                .as_ref()
                .or(entry.updated.as_ref())
                .map(|date| date.timestamp())
                .unwrap_or(now);
            let id_seed = if entry.id.is_empty() {
                format!("{TECHMEME_FEED_URL}:{url}:{title}")
            } else {
                format!("{TECHMEME_FEED_URL}:{}", entry.id)
            };

            Some(RawItem {
                id: stable_id("techmeme", &id_seed),
                title,
                url,
                source: source.clone(),
                timestamp,
                raw_score: 55,
                comments: 0,
                source_kind: SourceKind::Techmeme,
                section: "Tech".into(),
            })
        })
        .collect();

    Ok(items)
}
