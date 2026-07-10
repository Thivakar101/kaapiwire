use crate::db::{Db, MetricSnapshot};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    time::{SystemTime, UNIX_EPOCH},
};

const DEDUPE_WINDOW_SECONDS: i64 = 10 * 60;
const DUPLICATE_THRESHOLD: f32 = 0.85;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    HackerNews,
    Techmeme,
    Reddit,
    GithubTrending,
    Blog,
    General,
    NewsData,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HackerNews => "hacker_news",
            Self::Techmeme => "techmeme",
            Self::Reddit => "reddit",
            Self::GithubTrending => "github_trending",
            Self::Blog => "blog",
            Self::General => "general",
            Self::NewsData => "newsdata",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RawItem {
    pub id: String,
    pub title: String,
    pub url: String,
    pub source: String,
    pub timestamp: i64,
    pub raw_score: u32,
    pub comments: u32,
    pub source_kind: SourceKind,
    pub section: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsItem {
    pub id: String,
    pub title: String,
    pub url: String,
    pub source: String,
    pub timestamp: i64,
    pub raw_score: u32,
    pub importance: u8,
    pub relevance: u8,
    pub tag: String,
    pub section: String,
}

pub fn process_items(db: &Db, keywords: &[String], items: Vec<RawItem>) -> Vec<NewsItem> {
    let mut visible_items = Vec::new();
    let observed_at = now_unix_seconds();

    for raw in items.into_iter().filter(is_viable_item) {
        if let Err(error) = db.record_raw_item(&raw, observed_at) {
            eprintln!("{error}");
        }

        let previous_metric = db.previous_metric(&raw.id).ok().flatten();
        if let Err(error) = db.upsert_metric(&raw, observed_at) {
            eprintln!("{error}");
        }

        if db.has_seen_id(&raw.id).unwrap_or(false) {
            continue;
        }

        let duplicate_of = duplicate_canonical_id(db, &raw);
        let corroborating_sources = corroborating_source_count(db, &raw);
        let importance = score_importance(
            &raw,
            previous_metric.as_ref(),
            corroborating_sources,
            observed_at,
        );
        let relevance = score_relevance(&raw.title, keywords);
        let tag = tag_for(importance);
        let fresh_age = observed_at.saturating_sub(raw.timestamp);
        let fresh_public_story =
            matches!(raw.source_kind, SourceKind::HackerNews) && fresh_age <= 180;
        let general_story = matches!(raw.source_kind, SourceKind::General);
        let newsdata_story = matches!(raw.source_kind, SourceKind::NewsData);
        let visible = duplicate_of.is_none()
            && (importance > 40
                || relevance > 50
                || fresh_public_story
                || general_story
                || newsdata_story);

        let item = NewsItem {
            id: raw.id.clone(),
            title: raw.title.clone(),
            url: raw.url.clone(),
            source: raw.source.clone(),
            timestamp: raw.timestamp,
            raw_score: raw.raw_score,
            importance,
            relevance,
            tag,
            section: raw.section.clone(),
        };

        let canonical_id = duplicate_of.as_deref().unwrap_or(&raw.id);
        if let Err(error) = db.insert_seen_item(
            &raw,
            canonical_id,
            duplicate_of.as_deref(),
            &item,
            visible,
            observed_at,
        ) {
            eprintln!("{error}");
        }

        if visible {
            visible_items.push(item);
        }
    }

    visible_items.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    visible_items.truncate(12);
    visible_items
}

pub fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

pub fn clean_text(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut last_was_space = false;

    for character in value.chars() {
        let next = match character {
            '\n' | '\r' | '\t' => ' ',
            _ => character,
        };

        if next.is_whitespace() {
            if !last_was_space {
                output.push(' ');
            }
            last_was_space = true;
        } else {
            output.push(next);
            last_was_space = false;
        }
    }

    output.trim().to_string()
}

pub fn stable_id(prefix: &str, value: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{prefix}:{hash:016x}")
}

fn is_viable_item(item: &RawItem) -> bool {
    !item.id.trim().is_empty()
        && !item.title.trim().is_empty()
        && (item.url.starts_with("https://") || item.url.starts_with("http://"))
}

fn duplicate_canonical_id(db: &Db, raw: &RawItem) -> Option<String> {
    let candidates = db
        .recent_seen_candidates(raw.timestamp, DEDUPE_WINDOW_SECONDS)
        .ok()?;

    candidates
        .into_iter()
        .filter(|candidate| candidate.id.as_str() != raw.id.as_str())
        .filter(|candidate| title_similarity(&raw.title, &candidate.title) > DUPLICATE_THRESHOLD)
        .min_by_key(|candidate| {
            if candidate.source.as_str() == raw.source.as_str() {
                1
            } else {
                0
            }
        })
        .map(|candidate| candidate.canonical_id)
}

fn corroborating_source_count(db: &Db, raw: &RawItem) -> usize {
    let mut sources = HashSet::new();
    sources.insert(raw.source.to_lowercase());

    if let Ok(candidates) = db.recent_raw_candidates(raw.timestamp, DEDUPE_WINDOW_SECONDS) {
        for candidate in candidates {
            if title_similarity(&raw.title, &candidate.title) > DUPLICATE_THRESHOLD {
                sources.insert(candidate.source.to_lowercase());
            }
        }
    }

    sources.len()
}

fn score_importance(
    raw: &RawItem,
    previous_metric: Option<&MetricSnapshot>,
    corroborating_sources: usize,
    observed_at: i64,
) -> u8 {
    let mut score = match raw.source_kind {
        SourceKind::HackerNews => {
            (raw.raw_score / 3).min(42) as i32 + (raw.comments / 2).min(18) as i32
        }
        SourceKind::Reddit => {
            (raw.raw_score / 8).min(38) as i32 + (raw.comments / 4).min(12) as i32
        }
        SourceKind::Techmeme => 54,
        SourceKind::GithubTrending => 38 + (raw.raw_score / 30).min(14) as i32,
        SourceKind::Blog => 46,
        SourceKind::General => 42,
        SourceKind::NewsData => 44,
    };

    let cross_source_bonus = corroborating_sources.saturating_sub(1).min(2) as i32 * 22;
    score += cross_source_bonus;

    if let Some(previous) = previous_metric {
        let elapsed_minutes = ((observed_at - previous.observed_at).max(1) as f32) / 60.0;
        let score_delta = raw.raw_score.saturating_sub(previous.score) as f32;
        let comment_delta = raw.comments.saturating_sub(previous.comments) as f32;
        let velocity = (score_delta * 1.5 + comment_delta * 2.25) / elapsed_minutes;
        score += velocity.min(30.0) as i32;
    }

    let age = observed_at.saturating_sub(raw.timestamp);
    if age <= 180 {
        score += 8;
    } else if age <= 600 {
        score += 4;
    }

    score.clamp(0, 100) as u8
}

fn score_relevance(title: &str, keywords: &[String]) -> u8 {
    let title_lower = title.to_lowercase();
    let mut score = 0_i32;

    for keyword in keywords {
        let keyword = keyword.trim().to_lowercase();
        if keyword.is_empty() {
            continue;
        }

        if title_lower.contains(&keyword) {
            score += if keyword.contains(' ') { 68 } else { 62 };
        }
    }

    score.clamp(0, 100) as u8
}

pub fn importance_tag_for(importance: u8) -> &'static str {
    if importance > 70 {
        "Very Important"
    } else if importance >= 40 {
        "Medium Important"
    } else {
        "Less Important"
    }
}

fn tag_for(importance: u8) -> String {
    importance_tag_for(importance).into()
}

fn title_similarity(a: &str, b: &str) -> f32 {
    let a_tokens = title_tokens(a);
    let b_tokens = title_tokens(b);

    if a_tokens.is_empty() || b_tokens.is_empty() {
        return 0.0;
    }

    let intersection = a_tokens.intersection(&b_tokens).count() as f32;
    let denominator = a_tokens.len().min(b_tokens.len()) as f32;
    intersection / denominator
}

fn title_tokens(title: &str) -> HashSet<String> {
    title
        .to_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| token.len() > 2)
        .map(ToOwned::to_owned)
        .collect()
}
