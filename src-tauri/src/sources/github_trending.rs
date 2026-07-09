use crate::{
    pipeline::{clean_text, now_unix_seconds, RawItem, SourceKind},
    sources::SourceResult,
};
use chrono::Utc;
use reqwest::Client;
use scraper::{Html, Selector};

const TRENDING_URL: &str = "https://github.com/trending";

pub async fn fetch(client: &Client) -> SourceResult<Vec<RawItem>> {
    let html = client
        .get(TRENDING_URL)
        .send()
        .await
        .map_err(|error| format!("GitHub Trending request failed: {error}"))?
        .error_for_status()
        .map_err(|error| format!("GitHub Trending returned an error: {error}"))?
        .text()
        .await
        .map_err(|error| format!("GitHub Trending text failed: {error}"))?;

    let document = Html::parse_document(&html);
    let article_selector = selector("article.Box-row")?;
    let repo_selector = selector("h2 a")?;
    let description_selector = selector("p")?;
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let now = now_unix_seconds();

    let mut items = Vec::new();
    for article in document.select(&article_selector).take(25) {
        let Some(repo_link) = article.select(&repo_selector).next() else {
            continue;
        };

        let href = repo_link.value().attr("href").unwrap_or_default();
        if href.is_empty() {
            continue;
        }

        let repo = repo_link
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("")
            .replace("/ ", "/")
            .replace(" /", "/");
        let repo = clean_text(&repo);
        if repo.is_empty() {
            continue;
        }

        let description = article
            .select(&description_selector)
            .next()
            .map(|node| clean_text(&node.text().collect::<Vec<_>>().join(" ")))
            .unwrap_or_default();
        let title = if description.is_empty() {
            format!("{repo} is trending on GitHub")
        } else {
            format!("{repo}: {description}")
        };
        let raw_score = parse_stars_today(&article.text().collect::<Vec<_>>().join(" "));

        items.push(RawItem {
            id: format!("github-trending:{today}:{repo}"),
            title,
            url: format!("https://github.com{href}"),
            source: "GitHub Trending".into(),
            timestamp: now,
            raw_score,
            comments: 0,
            source_kind: SourceKind::GithubTrending,
            section: "Tech".into(),
        });
    }

    Ok(items)
}

fn selector(value: &str) -> SourceResult<Selector> {
    Selector::parse(value).map_err(|error| format!("Could not parse selector {value}: {error:?}"))
}

fn parse_stars_today(text: &str) -> u32 {
    for segment in text.split('\n').map(str::trim) {
        if !segment.contains("stars today") {
            continue;
        }

        let count = segment
            .split("stars today")
            .next()
            .unwrap_or_default()
            .chars()
            .filter(|character| character.is_ascii_digit())
            .collect::<String>();

        if let Ok(value) = count.parse::<u32>() {
            return value;
        }
    }

    45
}
