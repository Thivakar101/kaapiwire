use crate::{
    pipeline::{clean_text, now_unix_seconds, stable_id, RawItem, SourceKind},
    sources::SourceResult,
};
use feed_rs::parser;
use reqwest::Client;

pub async fn fetch(client: &Client, feeds: &[String]) -> SourceResult<Vec<RawItem>> {
    let mut items = Vec::new();
    let mut errors = Vec::new();

    for feed in feeds {
        match fetch_feed(client, feed).await {
            Ok(mut feed_items) => items.append(&mut feed_items),
            Err(error) => errors.push(error),
        }
    }

    if items.is_empty() && !errors.is_empty() {
        return Err(format!(
            "General news polling skipped: {}",
            errors.join("; ")
        ));
    }

    Ok(items)
}

async fn fetch_feed(client: &Client, feed_url: &str) -> SourceResult<Vec<RawItem>> {
    let body = client
        .get(feed_url)
        .send()
        .await
        .map_err(|error| format!("{feed_url} request failed: {error}"))?
        .error_for_status()
        .map_err(|error| format!("{feed_url} returned an error: {error}"))?
        .text()
        .await
        .map_err(|error| format!("{feed_url} text failed: {error}"))?;
    let feed = parser::parse(body.as_bytes())
        .map_err(|error| format!("{feed_url} parse failed: {error}"))?;
    let source = feed
        .title
        .as_ref()
        .map(|title| clean_text(&title.content))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| "World News".into());
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
                format!("{feed_url}:{url}:{title}")
            } else {
                format!("{feed_url}:{}", entry.id)
            };

            Some(RawItem {
                id: stable_id("general", &id_seed),
                title,
                url,
                source: source.clone(),
                timestamp,
                raw_score: 45,
                comments: 0,
                source_kind: SourceKind::General,
                section: "General".into(),
            })
        })
        .collect();

    Ok(items)
}
