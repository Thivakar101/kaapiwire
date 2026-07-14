use crate::{
    pipeline::{clean_text, RawItem, SourceKind},
    sources::SourceResult,
};
use reqwest::Client;
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};

const SUBREDDITS: &[&str] = &["technology", "programming"];
static REDDIT_BLOCKED_NOTICE_SHOWN: AtomicBool = AtomicBool::new(false);
static REDDIT_ERROR_NOTICE_SHOWN: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Deserialize)]
struct Listing {
    data: ListingData,
}

#[derive(Debug, Deserialize)]
struct ListingData {
    children: Vec<PostChild>,
}

#[derive(Debug, Deserialize)]
struct PostChild {
    data: PostData,
}

#[derive(Debug, Deserialize)]
struct PostData {
    id: String,
    title: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    permalink: Option<String>,
    #[serde(default)]
    created_utc: Option<f64>,
    #[serde(default)]
    score: Option<i64>,
    #[serde(default)]
    ups: Option<i64>,
    #[serde(default)]
    num_comments: Option<u32>,
    #[serde(default)]
    stickied: Option<bool>,
}

pub async fn fetch(client: &Client) -> SourceResult<Vec<RawItem>> {
    let mut items = Vec::new();
    let mut errors = Vec::new();

    for subreddit in SUBREDDITS {
        match fetch_subreddit(client, subreddit).await {
            Ok(mut subreddit_items) => items.append(&mut subreddit_items),
            Err(error) => errors.push(error),
        }
    }

    if !errors.is_empty() {
        let combined = errors.join("; ");
        if combined.contains("403") || combined.contains("Blocked") {
            if !REDDIT_BLOCKED_NOTICE_SHOWN.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "Reddit public JSON is blocked on this network, so kaapi wire is skipping Reddit for this session."
                );
            }
        } else if !REDDIT_ERROR_NOTICE_SHOWN.swap(true, Ordering::Relaxed) {
            eprintln!("Reddit polling skipped after an error: {combined}");
        }
    }

    Ok(items)
}

async fn fetch_subreddit(client: &Client, subreddit: &str) -> SourceResult<Vec<RawItem>> {
    let old_endpoint = format!("https://old.reddit.com/r/{subreddit}/new.json?limit=25&raw_json=1");
    let www_endpoint = format!("https://www.reddit.com/r/{subreddit}/new.json?limit=25&raw_json=1");

    let listing = match fetch_listing(client, &old_endpoint, subreddit).await {
        Ok(listing) => listing,
        Err(old_error) => match fetch_listing(client, &www_endpoint, subreddit).await {
            Ok(listing) => listing,
            Err(www_error) => {
                return Err(format!("{old_error}; fallback failed: {www_error}"));
            }
        },
    };

    listing_to_items(listing, subreddit)
}

async fn fetch_listing(client: &Client, endpoint: &str, subreddit: &str) -> SourceResult<Listing> {
    let listing = client
        .get(endpoint)
        .send()
        .await
        .map_err(|error| format!("Reddit r/{subreddit} request failed: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Reddit r/{subreddit} returned an error: {error}"))?
        .json::<Listing>()
        .await
        .map_err(|error| format!("Reddit r/{subreddit} JSON failed: {error}"))?;

    Ok(listing)
}

fn listing_to_items(listing: Listing, subreddit: &str) -> SourceResult<Vec<RawItem>> {
    let items = listing
        .data
        .children
        .into_iter()
        .filter(|child| !child.data.stickied.unwrap_or(false))
        .filter_map(|child| {
            let post = child.data;
            let title = clean_text(&post.title);
            if title.is_empty() {
                return None;
            }

            let url = post
                .url
                .filter(|url| url.starts_with("http://") || url.starts_with("https://"))
                .or_else(|| {
                    post.permalink
                        .map(|permalink| format!("https://old.reddit.com{permalink}"))
                })?;

            Some(RawItem {
                id: format!("reddit:{subreddit}:{}", post.id),
                title,
                url,
                source: format!("r/{subreddit}"),
                timestamp: post.created_utc.unwrap_or_default() as i64,
                raw_score: post.score.or(post.ups).unwrap_or_default().max(0) as u32,
                comments: post.num_comments.unwrap_or_default(),
                source_kind: SourceKind::Reddit,
                section: "Tech".into(),
            })
        })
        .collect();

    Ok(items)
}
