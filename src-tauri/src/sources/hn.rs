use crate::{
    pipeline::{clean_text, RawItem, SourceKind},
    sources::SourceResult,
};
use reqwest::Client;
use serde::Deserialize;

const NEW_STORIES_URL: &str = "https://hacker-news.firebaseio.com/v0/newstories.json";
const ITEM_URL_PREFIX: &str = "https://hacker-news.firebaseio.com/v0/item/";

#[derive(Debug, Deserialize)]
struct HnItem {
    id: u64,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    time: Option<i64>,
    #[serde(default)]
    score: Option<u32>,
    #[serde(default)]
    descendants: Option<u32>,
    #[serde(default, rename = "type")]
    item_type: Option<String>,
    #[serde(default)]
    deleted: Option<bool>,
    #[serde(default)]
    dead: Option<bool>,
}

pub async fn fetch(client: &Client) -> SourceResult<Vec<RawItem>> {
    let ids = client
        .get(NEW_STORIES_URL)
        .send()
        .await
        .map_err(|error| format!("HN newstories request failed: {error}"))?
        .error_for_status()
        .map_err(|error| format!("HN newstories returned an error: {error}"))?
        .json::<Vec<u64>>()
        .await
        .map_err(|error| format!("HN newstories JSON failed: {error}"))?;

    let mut items = Vec::new();

    for id in ids.into_iter().take(40) {
        let url = format!("{ITEM_URL_PREFIX}{id}.json");
        let item = match client.get(url).send().await {
            Ok(response) => response.error_for_status().ok(),
            Err(_) => None,
        };
        let Some(response) = item else {
            continue;
        };
        let Ok(item) = response.json::<HnItem>().await else {
            continue;
        };

        if item.deleted.unwrap_or(false)
            || item.dead.unwrap_or(false)
            || item.item_type.as_deref() != Some("story")
        {
            continue;
        }

        let Some(title) = item
            .title
            .as_deref()
            .map(clean_text)
            .filter(|title| !title.is_empty())
        else {
            continue;
        };
        let url = item
            .url
            .filter(|url| url.starts_with("http://") || url.starts_with("https://"))
            .unwrap_or_else(|| format!("https://news.ycombinator.com/item?id={}", item.id));

        items.push(RawItem {
            id: format!("hn:{}", item.id),
            title,
            url,
            source: "Hacker News".into(),
            timestamp: item.time.unwrap_or_default(),
            raw_score: item.score.unwrap_or_default(),
            comments: item.descendants.unwrap_or_default(),
            source_kind: SourceKind::HackerNews,
            section: "Tech".into(),
        });
    }

    Ok(items)
}
