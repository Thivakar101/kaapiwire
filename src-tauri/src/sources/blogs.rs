use crate::{
    pipeline::{clean_text, now_unix_seconds, stable_id, RawItem, SourceKind},
    sources::SourceResult,
};
use feed_rs::parser;
use reqwest::Client;
use scraper::{Html, Selector};
use std::{
    collections::HashSet,
    sync::atomic::{AtomicBool, Ordering},
};
use url::Url;

static BLOG_ERROR_NOTICE_SHOWN: AtomicBool = AtomicBool::new(false);

pub async fn fetch(client: &Client, sources: &[String]) -> SourceResult<Vec<RawItem>> {
    let mut items = Vec::new();
    let mut errors = Vec::new();

    for source in sources {
        match fetch_one_source(client, source).await {
            Ok(mut source_items) => items.append(&mut source_items),
            Err(error) => errors.push(error),
        }
    }

    if !errors.is_empty() && !BLOG_ERROR_NOTICE_SHOWN.swap(true, Ordering::Relaxed) {
        eprintln!(
            "One or more blog sources could not be read as RSS/Atom and will be skipped unless a page fallback exists."
        );
    }

    Ok(items)
}

async fn fetch_one_source(client: &Client, source: &str) -> SourceResult<Vec<RawItem>> {
    let source = source.trim();
    if source.is_empty() {
        return Ok(Vec::new());
    }

    if looks_like_feed(source) {
        return fetch_feed(client, source).await;
    }

    if let Ok(feed_url) = discover_feed_url(client, source).await {
        return fetch_feed(client, &feed_url).await;
    }

    for candidate in fallback_feed_candidates(source) {
        if let Ok(items) = fetch_feed(client, &candidate).await {
            return Ok(items);
        }
    }

    if source.contains("anthropic.com/news") {
        return fetch_anthropic_news_page(client, source).await;
    }

    Err(format!("No RSS/Atom feed discovered for {source}"))
}

async fn fetch_feed(client: &Client, feed_url: &str) -> SourceResult<Vec<RawItem>> {
    let body = client
        .get(feed_url)
        .send()
        .await
        .map_err(|error| format!("Blog feed request failed for {feed_url}: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Blog feed returned an error for {feed_url}: {error}"))?
        .text()
        .await
        .map_err(|error| format!("Blog feed text failed for {feed_url}: {error}"))?;
    let feed = parser::parse(body.as_bytes())
        .map_err(|error| format!("Blog feed parse failed for {feed_url}: {error}"))?;
    let source = feed
        .title
        .as_ref()
        .map(|title| clean_text(&title.content))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| source_label_from_url(feed_url));
    let now = now_unix_seconds();

    let items = feed
        .entries
        .into_iter()
        .take(25)
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
                id: stable_id("blog", &id_seed),
                title,
                url,
                source: source.clone(),
                timestamp,
                raw_score: 48,
                comments: 0,
                source_kind: SourceKind::Blog,
                section: "Tech".into(),
            })
        })
        .collect();

    Ok(items)
}

async fn discover_feed_url(client: &Client, page_url: &str) -> SourceResult<String> {
    let body = client
        .get(page_url)
        .send()
        .await
        .map_err(|error| format!("Blog page request failed for {page_url}: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Blog page returned an error for {page_url}: {error}"))?
        .text()
        .await
        .map_err(|error| format!("Blog page text failed for {page_url}: {error}"))?;
    let base =
        Url::parse(page_url).map_err(|error| format!("Invalid blog URL {page_url}: {error}"))?;
    let document = Html::parse_document(&body);
    let selector = Selector::parse("link[rel~='alternate']")
        .map_err(|error| format!("Could not parse feed discovery selector: {error:?}"))?;

    for node in document.select(&selector) {
        let value = node.value();
        let type_attr = value.attr("type").unwrap_or_default().to_lowercase();
        let href = value.attr("href").unwrap_or_default();

        if href.is_empty()
            || !(type_attr.contains("rss")
                || type_attr.contains("atom")
                || href.to_lowercase().contains("rss")
                || href.to_lowercase().contains("feed"))
        {
            continue;
        }

        if let Ok(resolved) = base.join(href) {
            return Ok(resolved.to_string());
        }
    }

    Err(format!("No feed link found on {page_url}"))
}

fn looks_like_feed(url: &str) -> bool {
    let lower = url.to_lowercase();
    lower.ends_with(".xml")
        || lower.ends_with(".rss")
        || lower.ends_with(".atom")
        || lower.contains("/rss")
        || lower.contains("/feed")
}

fn fallback_feed_candidates(source: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let Ok(url) = Url::parse(source) else {
        return candidates;
    };

    let base = format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default());
    let path = url.path().trim_end_matches('/');

    if source.contains("openai.com/blog") {
        candidates.push("https://openai.com/blog/rss.xml".into());
    }
    if source.contains("anthropic.com/news") {
        candidates.push("https://www.anthropic.com/news/rss.xml".into());
    }
    if source.contains("deepmind.google/blog") {
        candidates.push("https://deepmind.google/blog/rss.xml".into());
    }

    candidates.push(format!("{base}{path}/rss.xml"));
    candidates.push(format!("{base}{path}/feed.xml"));
    candidates.push(format!("{base}{path}/feed"));
    candidates.push(format!("{base}/rss.xml"));
    candidates.push(format!("{base}/feed.xml"));
    candidates
}

fn source_label_from_url(feed_url: &str) -> String {
    Url::parse(feed_url)
        .ok()
        .and_then(|url| {
            url.host_str()
                .map(|host| host.trim_start_matches("www.").to_string())
        })
        .unwrap_or_else(|| "Company Blog".into())
}

async fn fetch_anthropic_news_page(client: &Client, page_url: &str) -> SourceResult<Vec<RawItem>> {
    let body = client
        .get(page_url)
        .send()
        .await
        .map_err(|error| format!("Anthropic news page request failed: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Anthropic news page returned an error: {error}"))?
        .text()
        .await
        .map_err(|error| format!("Anthropic news page text failed: {error}"))?;

    let base = Url::parse(page_url)
        .map_err(|error| format!("Invalid Anthropic news URL {page_url}: {error}"))?;
    let document = Html::parse_document(&body);
    let link_selector = Selector::parse("a[href*='/news/']")
        .map_err(|error| format!("Could not parse Anthropic link selector: {error:?}"))?;
    let heading_selector = Selector::parse("h1, h2, h3, h4")
        .map_err(|error| format!("Could not parse Anthropic heading selector: {error:?}"))?;
    let mut seen_urls = HashSet::new();
    let now = now_unix_seconds();
    let mut items = Vec::new();

    for link in document.select(&link_selector) {
        let Some(href) = link.value().attr("href") else {
            continue;
        };
        let Ok(url) = base.join(href) else {
            continue;
        };
        let url = url.to_string();
        if url.trim_end_matches('/') == "https://www.anthropic.com/news"
            || !seen_urls.insert(url.clone())
        {
            continue;
        }

        let heading = link
            .select(&heading_selector)
            .next()
            .map(|node| clean_text(&node.text().collect::<Vec<_>>().join(" ")));
        let mut title =
            heading.unwrap_or_else(|| clean_text(&link.text().collect::<Vec<_>>().join(" ")));
        title = normalize_anthropic_title(&title);

        if title.len() < 8 {
            continue;
        }

        items.push(RawItem {
            id: stable_id("anthropic-news-v2", &format!("{url}:{title}")),
            title,
            url,
            source: "Anthropic News".into(),
            timestamp: now,
            raw_score: 50,
            comments: 0,
            source_kind: SourceKind::Blog,
            section: "Tech".into(),
        });

        if items.len() >= 20 {
            break;
        }
    }

    Ok(items)
}

fn normalize_anthropic_title(title: &str) -> String {
    let mut title = clean_text(title);
    if let Some((_, after_year)) = title.split_once(", 2026 ") {
        title = after_year.to_string();
    } else if let Some((_, after_year)) = title.split_once(", 2025 ") {
        title = after_year.to_string();
    } else if let Some((_, after_year)) = title.split_once(", 2024 ") {
        title = after_year.to_string();
    }

    if title.len() > 120 {
        title.truncate(117);
        title.push_str("...");
    }

    title
}
