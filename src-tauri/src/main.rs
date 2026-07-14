#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod db;
mod pipeline;
mod sources;

use crate::{
    db::Db,
    pipeline::{now_unix_seconds, NewsItem, RawItem},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tauri::{
    Emitter, LogicalPosition, LogicalSize, Manager, PhysicalPosition, WebviewWindow, Window,
};
use tauri_plugin_shell::ShellExt;
use tauri_runtime::ResizeDirection;
use tokio::time::MissedTickBehavior;

const MARGIN: i32 = 16;
const COLLAPSED_WINDOW_SIZE: u32 = 48;
const DEFAULT_EXPANDED_WIDTH: u32 = 360;
const DEFAULT_EXPANDED_HEIGHT: u32 = 430;
const MIN_EXPANDED_WIDTH: u32 = 300;
const MIN_EXPANDED_HEIGHT: u32 = 260;
const MAX_EXPANDED_WIDTH: u32 = 640;
const MAX_EXPANDED_HEIGHT: u32 = 760;
const DISPLAY_ITEM_LIMIT: usize = 40;
const DISPLAY_SECTION_SOFT_LIMIT: usize = 20;
const STARTUP_POLL_DELAY_SECONDS: u64 = 20;
const USER_AGENT: &str =
    "kaapi-wire/0.1 (local Windows desktop widget; public polling; no telemetry)";

static HN_ERROR_NOTICE_SHOWN: AtomicBool = AtomicBool::new(false);
static TECHMEME_ERROR_NOTICE_SHOWN: AtomicBool = AtomicBool::new(false);
static GITHUB_TRENDING_ERROR_NOTICE_SHOWN: AtomicBool = AtomicBool::new(false);
static GENERAL_ERROR_NOTICE_SHOWN: AtomicBool = AtomicBool::new(false);
static NEWSDATA_ERROR_NOTICE_SHOWN: AtomicBool = AtomicBool::new(false);

#[derive(Clone)]
struct AppState {
    db: Arc<Db>,
    client: reqwest::Client,
    config_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WidgetConfig {
    #[serde(default)]
    collapsed: bool,
    #[serde(default = "default_expanded_width")]
    width: u32,
    #[serde(default = "default_expanded_height")]
    height: u32,
}

impl Default for WidgetConfig {
    fn default() -> Self {
        Self {
            collapsed: false,
            width: DEFAULT_EXPANDED_WIDTH,
            height: DEFAULT_EXPANDED_HEIGHT,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct SourcesConfig {
    #[serde(default)]
    blogs: Vec<String>,
    #[serde(default)]
    general: Vec<String>,
    #[serde(default)]
    newsdata: sources::newsdata::NewsDataConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ViewportMetrics {
    avail_left: Option<i32>,
    avail_top: Option<i32>,
    avail_width: Option<u32>,
    avail_height: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NewsEvent {
    items: Vec<NewsItem>,
    generated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct YouTubeMediaResult {
    id: String,
    title: String,
    detail: String,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    video_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    playlist_id: Option<String>,
}

#[tauri::command]
fn get_config(app: tauri::AppHandle) -> Result<WidgetConfig, String> {
    ensure_default_config_files(&app)?;
    load_or_create_config(&app)
}

#[tauri::command]
fn get_initial_items(state: tauri::State<'_, Arc<AppState>>) -> Vec<NewsItem> {
    display_items(&state.db).unwrap_or_default()
}

#[tauri::command]
fn set_collapsed(
    app: tauri::AppHandle,
    collapsed: bool,
    width: Option<u32>,
    height: Option<u32>,
    viewport: Option<ViewportMetrics>,
) -> Result<WidgetConfig, String> {
    let mut config = load_or_create_config(&app).unwrap_or_default();
    config.collapsed = collapsed;
    if let Some(width) = width {
        config.width = clamp_width(width);
    }
    if let Some(height) = height {
        config.height = clamp_height(height);
    }
    save_config(&app, &config)?;

    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Main window is not available.".to_string())?;
    apply_widget_window_state(&window, &config, viewport.as_ref())?;

    Ok(config)
}

#[tauri::command]
fn resize_widget(
    app: tauri::AppHandle,
    width: u32,
    height: u32,
    viewport: Option<ViewportMetrics>,
) -> Result<WidgetConfig, String> {
    let mut config = load_or_create_config(&app).unwrap_or_default();
    config.collapsed = false;
    config.width = clamp_width(width);
    config.height = clamp_height(height);
    save_config(&app, &config)?;

    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Main window is not available.".to_string())?;
    apply_widget_window_state(&window, &config, viewport.as_ref())?;

    Ok(config)
}

#[tauri::command]
fn save_widget_size(
    app: tauri::AppHandle,
    width: u32,
    height: u32,
) -> Result<WidgetConfig, String> {
    let mut config = load_or_create_config(&app).unwrap_or_default();
    config.width = clamp_width(width);
    config.height = clamp_height(height);
    save_config(&app, &config)?;
    Ok(config)
}

#[tauri::command]
fn start_drag(window: WebviewWindow) -> Result<(), String> {
    window
        .start_dragging()
        .map_err(|error| format!("Could not start window drag: {error}"))
}

#[tauri::command]
fn start_resize(window: Window) -> Result<(), String> {
    window
        .start_resize_dragging(ResizeDirection::SouthEast)
        .map_err(|error| format!("Could not start window resize: {error}"))
}

#[tauri::command]
fn move_window_by(window: WebviewWindow, delta_x: i32, delta_y: i32) -> Result<(), String> {
    let position = window
        .outer_position()
        .map_err(|error| format!("Could not read window position: {error}"))?;
    window
        .set_position(PhysicalPosition::new(
            position.x + delta_x,
            position.y + delta_y,
        ))
        .map_err(|error| format!("Could not move widget: {error}"))
}

#[tauri::command]
fn open_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("Only http(s) URLs can be opened.".into());
    }

    #[allow(deprecated)]
    app.shell()
        .open(url, None)
        .map_err(|error| format!("Could not open URL: {error}"))
}

#[tauri::command]
async fn search_youtube_media(
    state: tauri::State<'_, Arc<AppState>>,
    query: String,
    media_type: String,
) -> Result<Vec<YouTubeMediaResult>, String> {
    let query = query.trim();
    if query.len() < 2 {
        return Ok(Vec::new());
    }
    let media_type = match media_type.trim().to_ascii_lowercase().as_str() {
        "podcast" => "podcast",
        _ => "music",
    };
    let search_query = format!("{query} {media_type}");

    let mut url = url::Url::parse("https://www.youtube.com/results")
        .map_err(|error| format!("Could not build YouTube search URL: {error}"))?;
    url.query_pairs_mut()
        .append_pair("search_query", &search_query)
        .append_pair("hl", "en");

    let html = state
        .client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("Could not search YouTube: {error}"))?
        .error_for_status()
        .map_err(|error| format!("YouTube search failed: {error}"))?
        .text()
        .await
        .map_err(|error| format!("Could not read YouTube search results: {error}"))?;

    parse_youtube_search_results(&html)
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            if let Err(error) = ensure_autostart_registered() {
                eprintln!("{error}");
            }

            if let Err(error) = ensure_default_config_files(app.handle()) {
                eprintln!("{error}");
            }

            let config_dir = app.path().app_config_dir().map_err(|error| {
                setup_error(format!("Could not resolve app config directory: {error}"))
            })?;
            let data_dir = app.path().app_local_data_dir().map_err(|error| {
                setup_error(format!("Could not resolve app data directory: {error}"))
            })?;
            fs::create_dir_all(&data_dir).map_err(|error| {
                setup_error(format!("Could not create app data directory: {error}"))
            })?;

            let db = Arc::new(Db::open(&data_dir.join("kaapi-wire.sqlite3")).map_err(setup_error)?);
            let client = build_http_client().map_err(setup_error)?;
            let state = Arc::new(AppState {
                db,
                client,
                config_dir,
            });
            app.manage(state.clone());

            if let Some(window) = app.get_webview_window("main") {
                let config = load_or_create_config(app.handle()).unwrap_or_default();
                if let Err(error) = apply_widget_window_state(&window, &config, None) {
                    eprintln!("{error}");
                }
            }

            start_pollers(app.handle().clone(), state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            get_initial_items,
            set_collapsed,
            resize_widget,
            save_widget_size,
            start_drag,
            start_resize,
            move_window_by,
            open_url,
            search_youtube_media
        ])
        .run(tauri::generate_context!())
        .expect("error while running kaapi wire");
}

fn setup_error(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, message.into())
}

fn ensure_autostart_registered() -> Result<(), String> {
    #[cfg(all(target_os = "windows", not(debug_assertions)))]
    {
        let exe = std::env::current_exe()
            .map_err(|error| format!("Could not resolve kaapi wire executable path: {error}"))?;
        let exe_arg = format!("\"{}\"", exe.display());
        let status = std::process::Command::new("reg")
            .args([
                "add",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "kaapi wire",
                "/t",
                "REG_SZ",
                "/d",
                &exe_arg,
                "/f",
            ])
            .status()
            .map_err(|error| format!("Could not register kaapi wire startup entry: {error}"))?;

        if !status.success() {
            return Err(format!(
                "Could not register kaapi wire startup entry: reg.exe exited with {status}"
            ));
        }
    }

    Ok(())
}

fn build_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(12))
        .pool_max_idle_per_host(2)
        .build()
        .map_err(|error| format!("Could not build HTTP client: {error}"))
}

fn apply_vibrancy(window: &WebviewWindow) {
    #[cfg(target_os = "windows")]
    {
        if window_vibrancy::apply_mica(window, Some(true)).is_err() {
            let _ = window_vibrancy::apply_acrylic(window, Some((12, 14, 16, 188)));
        }
    }
}

fn clear_vibrancy(window: &WebviewWindow) {
    #[cfg(target_os = "windows")]
    {
        let _ = window_vibrancy::clear_mica(window);
        let _ = window_vibrancy::clear_acrylic(window);
        let _ = window_vibrancy::clear_blur(window);
        let _ = window_vibrancy::clear_tabbed(window);
    }
}

fn start_pollers(app: tauri::AppHandle, state: Arc<AppState>) {
    spawn_hn_poller(app.clone(), state.clone());
    spawn_techmeme_poller(app.clone(), state.clone());
    spawn_reddit_poller(app.clone(), state.clone());
    spawn_github_trending_poller(app.clone(), state.clone());
    spawn_blog_poller(app.clone(), state.clone());
    spawn_general_poller(app.clone(), state.clone());
    spawn_newsdata_poller(app.clone(), state.clone());
    spawn_snapshot_loop(app, state.clone());
    spawn_prune_loop(state);
}

fn spawn_hn_poller(app: tauri::AppHandle, state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(STARTUP_POLL_DELAY_SECONDS)).await;
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            match sources::hn::fetch(&state.client).await {
                Ok(items) => handle_source_items(&app, &state, items),
                Err(error) => log_poll_error_once(&HN_ERROR_NOTICE_SHOWN, error),
            }
        }
    });
}

fn spawn_techmeme_poller(app: tauri::AppHandle, state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(STARTUP_POLL_DELAY_SECONDS)).await;
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            match sources::techmeme::fetch(&state.client).await {
                Ok(items) => handle_source_items(&app, &state, items),
                Err(error) => log_poll_error_once(&TECHMEME_ERROR_NOTICE_SHOWN, error),
            }
        }
    });
}

fn spawn_reddit_poller(app: tauri::AppHandle, state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(STARTUP_POLL_DELAY_SECONDS)).await;
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            match sources::reddit::fetch(&state.client).await {
                Ok(items) => handle_source_items(&app, &state, items),
                Err(error) => eprintln!("{error}"),
            }
        }
    });
}

fn spawn_github_trending_poller(app: tauri::AppHandle, state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(STARTUP_POLL_DELAY_SECONDS)).await;
        let mut interval = tokio::time::interval(Duration::from_secs(5 * 60));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            match sources::github_trending::fetch(&state.client).await {
                Ok(items) => handle_source_items(&app, &state, items),
                Err(error) => log_poll_error_once(&GITHUB_TRENDING_ERROR_NOTICE_SHOWN, error),
            }
        }
    });
}

fn spawn_blog_poller(app: tauri::AppHandle, state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(STARTUP_POLL_DELAY_SECONDS)).await;
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            let sources = load_blog_sources(&state.config_dir);
            match sources::blogs::fetch(&state.client, &sources).await {
                Ok(items) => handle_source_items(&app, &state, items),
                Err(error) => eprintln!("{error}"),
            }
        }
    });
}

fn spawn_general_poller(app: tauri::AppHandle, state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(STARTUP_POLL_DELAY_SECONDS)).await;
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            let feeds = load_general_sources(&state.config_dir);
            match sources::general::fetch(&state.client, &feeds).await {
                Ok(items) => handle_source_items(&app, &state, items),
                Err(error) => log_poll_error_once(&GENERAL_ERROR_NOTICE_SHOWN, error),
            }
        }
    });
}

fn spawn_newsdata_poller(app: tauri::AppHandle, state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(STARTUP_POLL_DELAY_SECONDS)).await;
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            let config = load_newsdata_config(&state.config_dir);
            match sources::newsdata::fetch(&state.client, &config).await {
                Ok(items) => handle_source_items(&app, &state, items),
                Err(error) => log_poll_error_once(&NEWSDATA_ERROR_NOTICE_SHOWN, error),
            }
        }
    });
}

fn log_poll_error_once(flag: &AtomicBool, error: String) {
    if !flag.swap(true, Ordering::Relaxed) {
        eprintln!("{error}");
    }
}

fn parse_youtube_search_results(html: &str) -> Result<Vec<YouTubeMediaResult>, String> {
    let initial_data = extract_json_assignment(html, "var ytInitialData = ")
        .or_else(|| extract_json_assignment(html, "window[\"ytInitialData\"] = "))
        .ok_or_else(|| "Could not find YouTube search data.".to_string())?;
    let value: serde_json::Value = serde_json::from_str(initial_data)
        .map_err(|error| format!("Could not parse YouTube search data: {error}"))?;

    let mut results = Vec::new();
    let mut seen = HashSet::new();
    collect_youtube_results(&value, &mut results, &mut seen);
    results.truncate(12);
    Ok(results)
}

fn extract_json_assignment<'a>(html: &'a str, marker: &str) -> Option<&'a str> {
    let marker_index = html.find(marker)?;
    let start = marker_index + marker.len();
    let bytes = html.as_bytes();
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escaped = false;
    let mut json_start = None;

    for index in start..bytes.len() {
        let byte = bytes[index];
        if json_start.is_none() {
            if byte == b'{' {
                json_start = Some(index);
                depth = 1;
            }
            continue;
        }

        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }

        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    let start = json_start?;
                    return html.get(start..=index);
                }
            }
            _ => {}
        }
    }

    None
}

fn collect_youtube_results(
    value: &serde_json::Value,
    results: &mut Vec<YouTubeMediaResult>,
    seen: &mut HashSet<String>,
) {
    if results.len() >= 12 {
        return;
    }

    match value {
        serde_json::Value::Object(object) => {
            if let Some(renderer) = object.get("playlistRenderer") {
                if let Some(result) = parse_playlist_renderer(renderer) {
                    if seen.insert(result.id.clone()) {
                        results.push(result);
                    }
                }
            }

            if let Some(renderer) = object.get("videoRenderer") {
                if let Some(result) = parse_video_renderer(renderer) {
                    if seen.insert(result.id.clone()) {
                        results.push(result);
                    }
                }
            }

            for child in object.values() {
                collect_youtube_results(child, results, seen);
                if results.len() >= 12 {
                    break;
                }
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                collect_youtube_results(child, results, seen);
                if results.len() >= 12 {
                    break;
                }
            }
        }
        _ => {}
    }
}

fn parse_playlist_renderer(renderer: &serde_json::Value) -> Option<YouTubeMediaResult> {
    let playlist_id = renderer.get("playlistId")?.as_str()?.to_string();
    let title = extract_text(renderer.get("title")?)?;
    let detail = renderer
        .get("videoCount")
        .and_then(serde_json::Value::as_str)
        .map(|count| format!("{count} videos playlist"))
        .unwrap_or_else(|| "YouTube playlist".to_string());

    Some(YouTubeMediaResult {
        id: format!("yt-playlist-{playlist_id}"),
        title: clean_youtube_title(&title),
        detail,
        kind: "playlist".into(),
        video_id: None,
        playlist_id: Some(playlist_id),
    })
}

fn parse_video_renderer(renderer: &serde_json::Value) -> Option<YouTubeMediaResult> {
    let video_id = renderer.get("videoId")?.as_str()?.to_string();
    let title = extract_text(renderer.get("title")?)?;
    let channel = renderer
        .get("ownerText")
        .or_else(|| renderer.get("shortBylineText"))
        .and_then(extract_text)
        .unwrap_or_else(|| "YouTube video".to_string());

    Some(YouTubeMediaResult {
        id: format!("yt-video-{video_id}"),
        title: clean_youtube_title(&title),
        detail: channel,
        kind: "video".into(),
        video_id: Some(video_id),
        playlist_id: None,
    })
}

fn extract_text(value: &serde_json::Value) -> Option<String> {
    if let Some(text) = value.get("simpleText").and_then(serde_json::Value::as_str) {
        return Some(text.to_string());
    }

    let runs = value.get("runs")?.as_array()?;
    let text = runs
        .iter()
        .filter_map(|run| run.get("text").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn clean_youtube_title(title: &str) -> String {
    title
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
        .trim()
        .to_string()
}

fn spawn_prune_loop(state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60 * 60));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            let older_than = now_unix_seconds() - 7 * 24 * 60 * 60;
            if let Err(error) = state.db.prune_raw_items(older_than) {
                eprintln!("{error}");
            }
        }
    });
}

fn spawn_snapshot_loop(app: tauri::AppHandle, state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            let Ok(items) = display_items(&state.db) else {
                continue;
            };
            if items.is_empty() {
                continue;
            }

            let payload = NewsEvent {
                items,
                generated_at: now_unix_seconds(),
            };
            if let Err(error) = app.emit("news:snapshot", payload) {
                eprintln!("Could not emit news snapshot: {error}");
            }
        }
    });
}

fn display_items(db: &Db) -> Result<Vec<NewsItem>, String> {
    let mut items = Vec::new();
    let mut seen = HashSet::new();

    for item in db.recent_visible_items(DISPLAY_ITEM_LIMIT * 2)? {
        if seen.insert(item.id.clone()) {
            items.push(item);
        }
    }

    for item in db.recent_logged_items(DISPLAY_ITEM_LIMIT * 2)? {
        if seen.insert(item.id.clone()) {
            items.push(item);
        }
    }

    items.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    let mut selected = Vec::new();
    let mut selected_ids = HashSet::new();
    let mut section_counts: HashMap<String, usize> = HashMap::new();

    for item in &items {
        let count = section_counts.entry(item.section.clone()).or_default();
        if *count >= DISPLAY_SECTION_SOFT_LIMIT {
            continue;
        }

        selected_ids.insert(item.id.clone());
        selected.push(item.clone());
        *count += 1;

        if selected.len() >= DISPLAY_ITEM_LIMIT {
            break;
        }
    }

    if selected.len() < DISPLAY_ITEM_LIMIT {
        for item in items {
            if selected_ids.insert(item.id.clone()) {
                selected.push(item);
            }
            if selected.len() >= DISPLAY_ITEM_LIMIT {
                break;
            }
        }
    }

    selected.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(selected)
}

fn handle_source_items(app: &tauri::AppHandle, state: &AppState, items: Vec<RawItem>) {
    if items.is_empty() {
        return;
    }

    let keywords = load_keywords(&state.config_dir);
    let visible_items = pipeline::process_items(state.db.as_ref(), &keywords, items);

    if visible_items.is_empty() {
        return;
    }

    let payload = NewsEvent {
        items: visible_items,
        generated_at: now_unix_seconds(),
    };
    if let Err(error) = app.emit("news:new-items", payload) {
        eprintln!("Could not emit news event: {error}");
    }
}

fn apply_widget_window_state(
    window: &WebviewWindow,
    config: &WidgetConfig,
    viewport: Option<&ViewportMetrics>,
) -> Result<(), String> {
    let size = if config.collapsed {
        LogicalSize::new(COLLAPSED_WINDOW_SIZE, COLLAPSED_WINDOW_SIZE)
    } else {
        LogicalSize::new(clamp_width(config.width), clamp_height(config.height))
    };

    window
        .set_size(size)
        .map_err(|error| format!("Could not set widget size: {error}"))?;
    if config.collapsed {
        clear_vibrancy(window);
    } else {
        apply_vibrancy(window);
    }
    dock_bottom_right(window, size, viewport)?;
    window
        .set_always_on_top(true)
        .map_err(|error| format!("Could not keep widget always on top: {error}"))?;

    Ok(())
}

fn dock_bottom_right(
    window: &WebviewWindow,
    size: LogicalSize<u32>,
    viewport: Option<&ViewportMetrics>,
) -> Result<(), String> {
    if let Some(viewport) = viewport {
        if let (Some(width), Some(height)) = (viewport.avail_width, viewport.avail_height) {
            let left = viewport.avail_left.unwrap_or_default();
            let top = viewport.avail_top.unwrap_or_default();
            let x = left + width as i32 - size.width as i32 - MARGIN;
            let y = top + height as i32 - size.height as i32 - MARGIN;
            return window
                .set_position(LogicalPosition::new(x, y))
                .map_err(|error| format!("Could not dock widget: {error}"));
        }
    }

    let monitor = window
        .primary_monitor()
        .map_err(|error| format!("Could not read primary monitor: {error}"))?
        .or_else(|| window.current_monitor().ok().flatten())
        .ok_or_else(|| "No monitor was available for positioning.".to_string())?;

    let monitor_position = monitor.position();
    let monitor_size = monitor.size();
    let x = monitor_position.x + monitor_size.width as i32 - size.width as i32 - MARGIN;
    let y = monitor_position.y + monitor_size.height as i32 - size.height as i32 - MARGIN - 56;

    window
        .set_position(LogicalPosition::new(x, y))
        .map_err(|error| format!("Could not dock widget: {error}"))
}

fn load_or_create_config(app: &tauri::AppHandle) -> Result<WidgetConfig, String> {
    let path = config_path(app)?;

    if !path.exists() {
        let config = WidgetConfig::default();
        save_config(app, &config)?;
        return Ok(config);
    }

    let bytes = fs::read(&path).map_err(|error| format!("Could not read config.json: {error}"))?;
    let mut config: WidgetConfig = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Could not parse config.json: {error}"))?;
    sanitize_config(&mut config);
    Ok(config)
}

fn save_config(app: &tauri::AppHandle, config: &WidgetConfig) -> Result<(), String> {
    let path = config_path(app)?;
    ensure_parent_dir(&path)?;

    let json = serde_json::to_vec_pretty(config)
        .map_err(|error| format!("Could not serialize config.json: {error}"))?;
    fs::write(path, json).map_err(|error| format!("Could not write config.json: {error}"))
}

fn ensure_default_config_files(app: &tauri::AppHandle) -> Result<(), String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("Could not resolve app config directory: {error}"))?;
    fs::create_dir_all(&config_dir)
        .map_err(|error| format!("Could not create app config directory: {error}"))?;

    write_if_missing(&config_dir.join("keywords.json"), DEFAULT_KEYWORDS_JSON)?;
    write_if_missing(&config_dir.join("sources.json"), DEFAULT_SOURCES_JSON)?;
    Ok(())
}

fn load_keywords(config_dir: &Path) -> Vec<String> {
    let path = config_dir.join("keywords.json");
    fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str::<Vec<String>>(&contents).ok())
        .filter(|keywords| !keywords.is_empty())
        .or_else(|| serde_json::from_str::<Vec<String>>(DEFAULT_KEYWORDS_JSON).ok())
        .unwrap_or_default()
}

fn load_blog_sources(config_dir: &Path) -> Vec<String> {
    let path = config_dir.join("sources.json");
    fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str::<SourcesConfig>(&contents).ok())
        .map(|config| config.blogs)
        .filter(|blogs| !blogs.is_empty())
        .or_else(|| {
            serde_json::from_str::<SourcesConfig>(DEFAULT_SOURCES_JSON)
                .ok()
                .map(|config| config.blogs)
        })
        .unwrap_or_default()
}

fn load_general_sources(config_dir: &Path) -> Vec<String> {
    let path = config_dir.join("sources.json");
    fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str::<SourcesConfig>(&contents).ok())
        .map(|config| config.general)
        .filter(|feeds| !feeds.is_empty())
        .or_else(|| {
            serde_json::from_str::<SourcesConfig>(DEFAULT_SOURCES_JSON)
                .ok()
                .map(|config| config.general)
        })
        .unwrap_or_default()
}

fn load_newsdata_config(config_dir: &Path) -> sources::newsdata::NewsDataConfig {
    let path = config_dir.join("sources.json");
    fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str::<SourcesConfig>(&contents).ok())
        .map(|config| config.newsdata)
        .unwrap_or_default()
}

fn write_if_missing(path: &Path, contents: &str) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }

    ensure_parent_dir(path)?;
    fs::write(path, contents)
        .map_err(|error| format!("Could not write {}: {error}", path.display()))
}

fn ensure_parent_dir(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
    }
    Ok(())
}

fn config_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|dir| dir.join("config.json"))
        .map_err(|error| format!("Could not resolve config.json path: {error}"))
}

const DEFAULT_KEYWORDS_JSON: &str = r#"[
  "anthropic",
  "openai",
  "claude",
  "llm",
  "nvidia",
  "chip export"
]
"#;

const DEFAULT_SOURCES_JSON: &str = r#"{
  "blogs": [
    "https://openai.com/blog",
    "https://www.anthropic.com/news",
    "https://deepmind.google/blog"
  ],
  "general": [
    "https://feeds.bbci.co.uk/news/world/rss.xml",
    "https://feeds.npr.org/1004/rss.xml",
    "https://www.theguardian.com/world/rss"
  ],
  "newsdata": {
    "enabled": true,
    "language": "en"
  }
}
"#;

fn default_expanded_width() -> u32 {
    DEFAULT_EXPANDED_WIDTH
}

fn default_expanded_height() -> u32 {
    DEFAULT_EXPANDED_HEIGHT
}

fn clamp_width(width: u32) -> u32 {
    width.clamp(MIN_EXPANDED_WIDTH, MAX_EXPANDED_WIDTH)
}

fn clamp_height(height: u32) -> u32 {
    height.clamp(MIN_EXPANDED_HEIGHT, MAX_EXPANDED_HEIGHT)
}

fn sanitize_config(config: &mut WidgetConfig) {
    if config.width == 460 && config.height == 560 {
        config.width = DEFAULT_EXPANDED_WIDTH;
        config.height = DEFAULT_EXPANDED_HEIGHT;
    }
    config.width = clamp_width(config.width);
    config.height = clamp_height(config.height);
}
