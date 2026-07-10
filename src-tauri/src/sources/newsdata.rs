use crate::{
    pipeline::{clean_text, now_unix_seconds, stable_id, RawItem, SourceKind},
    sources::SourceResult,
};
use reqwest::{Client, Url};
use serde::Deserialize;

const NEWSDATA_LATEST_URL: &str = "https://newsdata.io/api/1/latest";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsDataConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default)]
    pub country: String,
    #[serde(default)]
    pub category: String,
    #[serde(default = "default_timeframe")]
    pub timeframe: String,
    #[serde(default)]
    pub query: String,
}

impl Default for NewsDataConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            api_key: String::new(),
            language: default_language(),
            country: String::new(),
            category: String::new(),
            timeframe: default_timeframe(),
            query: String::new(),
        }
    }
}

impl NewsDataConfig {
    pub fn normalized(mut self) -> Self {
        if self.api_key.trim().is_empty() {
            self.api_key = std::env::var("NEWSDATA_API_KEY")
                .or_else(|_| std::env::var("NEWS_DATA_IO_API_KEY"))
                .unwrap_or_default();
        }

        self.api_key = self.api_key.trim().to_string();
        self.language = self.language.trim().to_string();
        self.country = clean_csv_param(&self.country);
        self.category = clean_csv_param(&self.category);
        self.timeframe = self.timeframe.trim().to_string();
        self.query = self.query.trim().to_string();
        self
    }
}

#[derive(Debug, Deserialize)]
struct NewsDataResponse {
    status: String,
    #[serde(default)]
    results: Vec<NewsDataArticle>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NewsDataArticle {
    #[serde(default)]
    article_id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    link: Option<String>,
    #[serde(default)]
    source_id: Option<String>,
    #[serde(default)]
    source_name: Option<String>,
    #[serde(default, rename = "pubDate")]
    pub_date: Option<String>,
    #[serde(default)]
    category: Vec<String>,
}

pub async fn fetch(client: &Client, config: &NewsDataConfig) -> SourceResult<Vec<RawItem>> {
    let config = config.clone().normalized();
    if !config.enabled || config.api_key.is_empty() {
        return Ok(Vec::new());
    }

    let mut url = Url::parse(NEWSDATA_LATEST_URL)
        .map_err(|error| format!("Could not build NewsData.io latest URL: {error}"))?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("apikey", &config.api_key);
        append_param(&mut query, "language", &config.language);
        append_param(&mut query, "country", &config.country);
        append_param(&mut query, "category", &config.category);
        append_param(&mut query, "timeframe", &config.timeframe);
        append_param(&mut query, "q", &config.query);
    }

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| request_error_message("NewsData.io latest request failed", &error))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("NewsData.io latest returned HTTP {status}"));
    }

    let payload = response
        .json::<NewsDataResponse>()
        .await
        .map_err(|error| request_error_message("NewsData.io latest JSON failed", &error))?;
    if !payload.status.eq_ignore_ascii_case("success") {
        let detail = payload
            .message
            .or(payload.code)
            .unwrap_or_else(|| "unknown API error".into());
        return Err(format!("NewsData.io latest returned {detail}"));
    }

    let now = now_unix_seconds();
    let items = payload
        .results
        .into_iter()
        .take(30)
        .filter_map(|article| article.into_raw_item(now))
        .collect();

    Ok(items)
}

impl NewsDataArticle {
    fn into_raw_item(self, now: i64) -> Option<RawItem> {
        let title = clean_text(self.title.as_deref()?);
        if title.is_empty() {
            return None;
        }

        let url = self.link?;
        let source = self
            .source_name
            .or(self.source_id)
            .map(|source| clean_text(&source))
            .filter(|source| !source.is_empty())
            .unwrap_or_else(|| "NewsData.io".into());
        let id_seed = if self.article_id.trim().is_empty() {
            format!("{source}:{url}:{title}")
        } else {
            self.article_id
        };
        let section = section_for_categories(&self.category);

        Some(RawItem {
            id: stable_id("newsdata", &id_seed),
            title,
            url,
            source,
            timestamp: parse_pub_date(self.pub_date.as_deref()).unwrap_or(now),
            raw_score: if section == "Tech" { 52 } else { 46 },
            comments: 0,
            source_kind: SourceKind::NewsData,
            section: section.into(),
        })
    }
}

fn append_param(
    query: &mut url::form_urlencoded::Serializer<'_, url::UrlQuery<'_>>,
    key: &str,
    value: &str,
) {
    if !value.trim().is_empty() {
        query.append_pair(key, value.trim());
    }
}

fn clean_csv_param(value: &str) -> String {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

fn section_for_categories(categories: &[String]) -> &'static str {
    if categories.iter().any(|category| {
        matches!(
            category.trim().to_ascii_lowercase().as_str(),
            "technology" | "science" | "business"
        )
    }) {
        "Tech"
    } else {
        "General"
    }
}

fn parse_pub_date(value: Option<&str>) -> Option<i64> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }

    chrono::DateTime::parse_from_rfc3339(value)
        .map(|date| date.timestamp())
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
                .map(|date| date.and_utc().timestamp())
        })
        .ok()
}

fn request_error_message(context: &str, error: &reqwest::Error) -> String {
    if error.is_timeout() {
        format!("{context}: timed out")
    } else if error.is_decode() {
        format!("{context}: response format was not recognized")
    } else {
        context.to_string()
    }
}

fn default_enabled() -> bool {
    true
}

fn default_language() -> String {
    "en".into()
}

fn default_timeframe() -> String {
    String::new()
}
