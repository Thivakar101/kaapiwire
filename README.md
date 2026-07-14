# kaapi wire

kaapi wire is a small always-on-top Windows desktop widget for live news, tasks, a timer, and lightweight YouTube media search. It sits in the corner of the desktop, collapses into a compact icon, and expands into a retro Windows-style panel.

The project is intentionally local-first. There are no accounts, no telemetry, no analytics SDKs, no cloud backend, no paid APIs, and no code-signing requirement. Everything runs on the user's machine.

## What It Does

- Shows a borderless, transparent Tauri widget on Windows.
- Uses an old Windows-inspired UI with crisp buttons, square panels, and a minimal system color palette.
- Collapses into a small movable icon.
- Expands into a resizable panel with Tech, General, Todo, Timer, and Media sections.
- Opens headlines in the system browser instead of navigating inside the WebView.
- Shows only fresh visible news; stale items are filtered out of the widget.
- Searches YouTube music and podcasts from the Media tab.
- Displays a Media notice that licensed music is not allowed.
- Polls public free sources and stores dedupe/history locally in SQLite.
- Persists widget state in a local JSON config file.
- Release builds register themselves to start automatically when the Windows user logs in.

## Why Tauri

kaapi wire uses Tauri v2 instead of Electron because this app should feel almost invisible. The frontend is plain HTML, CSS, and JavaScript with no framework and no frontend build step. The backend is Rust, using async polling and SQLite for local storage.

The goal is simple: keep RAM usage low, avoid npm bloat, and keep the app easy to inspect.

## Data Sources

kaapi wire only uses public, no-key, zero-cost sources:

- Hacker News API
- Techmeme RSS
- Reddit public JSON endpoints
- GitHub Trending page scraping
- Company blog RSS/feed discovery
- General world-news RSS feeds
- NewsData.io latest news API, when a local API key is configured

Default company sources are:

- OpenAI Blog
- Anthropic News
- Google DeepMind Blog

Default general sources are:

- BBC World News RSS
- NPR World RSS
- The Guardian World RSS

Anything that requires a paid tier, API billing, cloud hosting, or an account is deliberately skipped.

## Architecture

```text
src/
  index.html          Frontend markup
  styles.css          Retro widget visuals and responsive layout
  main.js             UI state, Tauri events, drag/resize behavior

src-tauri/
  src/main.rs         Tauri setup, window state, poller orchestration
  src/db.rs           SQLite schema, raw logs, visible item queries
  src/pipeline.rs     Normalize, dedupe, score, tag, and filter items
  src/sources/        Individual source adapters
```

The app has three main layers:

1. Source pollers fetch public feeds/APIs on a schedule.
2. The pipeline normalizes, dedupes, scores, and tags each item.
3. Rust emits events to the WebView, and the frontend renders the feed.

There is no client-side polling for news. The frontend listens for Tauri events from Rust.

## News Pipeline

Every story is normalized into this shape:

```text
id, title, url, source, timestamp, raw_score
```

The local pipeline then:

- Logs every fetched item into SQLite.
- Skips exact IDs that were already seen.
- Uses simple token-overlap similarity to merge near-duplicate stories.
- Scores importance from cross-source corroboration and source-specific momentum.
- Scores relevance from user-editable keywords.
- Tags stories as Breaking, Watching, or General.
- Pushes only useful stories to the UI while keeping the local log complete.
- Lets the frontend hide stale visible items so old news does not stay on screen.

There is no ML, no embeddings, and no external ranking service.

## Media

The Media tab searches YouTube for music and podcasts and embeds the selected result in the app. Licensed music is not allowed, and the app shows that warning directly in the Media panel.

## Local Files

kaapi wire creates local config files under the app config directory:

```powershell
$env:APPDATA\com.kaapiwire.widget
```

Useful files:

- `config.json` stores collapsed/expanded state and widget size.
- `keywords.json` stores relevance keywords.
- `sources.json` stores editable blog/general RSS sources and optional NewsData.io settings.

To enable NewsData.io live polling, add a local `newsdata.apiKey` entry to `sources.json` or set `NEWSDATA_API_KEY` in the app environment. API keys should stay local and should not be committed.

The SQLite database lives under:

```powershell
$env:LOCALAPPDATA\com.kaapiwire.widget
```

Useful file:

- `kaapi-wire.sqlite3` stores raw items, seen items, and source metrics.

## Requirements

Install these once:

- Rust
- Microsoft Edge WebView2 Runtime
- Tauri CLI v2

Install the Tauri CLI:

```powershell
cargo install tauri-cli --version "^2"
```

## Running In Development

From the project root:

```powershell
cd path\to\kaapiwire
cargo tauri dev
```

If an old copy is already running:

```powershell
taskkill /IM kaapi-wire.exe /F
cargo tauri dev
```

## Startup Behavior

Release builds of kaapi wire register themselves in the current user's Windows startup list:

```text
HKCU\Software\Microsoft\Windows\CurrentVersion\Run
```

That means it starts automatically after login without admin rights, a Windows service, a scheduled task, or cloud sync. Development builds do not register themselves, so `cargo tauri dev` will not leave a debug console app in Windows startup.

To check the startup entry:

```powershell
reg query "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v "kaapi wire"
```

To remove the startup entry:

```powershell
reg delete "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v "kaapi wire" /f
```

## Building

Development builds are unsigned by design:

```powershell
cargo tauri build
```

Windows SmartScreen may warn on first launch because the app is not code-signed. That is expected for this zero-cost setup.

## Resetting Local State

If the widget opens in a strange size or old cached news keeps appearing, reset local state:

```powershell
Remove-Item "$env:APPDATA\com.kaapiwire.widget\config.json" -Force -ErrorAction SilentlyContinue
Remove-Item "$env:LOCALAPPDATA\com.kaapiwire.widget\kaapi-wire.sqlite3*" -Force -ErrorAction SilentlyContinue
cargo tauri dev
```

## Known Limitations

- Reddit may return `403 Blocked` on some networks even with a descriptive User-Agent. kaapi wire skips Reddit for that session and keeps the other sources running.
- GitHub Trending is scraped from a public webpage, so markup changes on GitHub can require parser updates.
- News sources do not all publish at the same speed. The app polls frequently, but it can only show stories after the source makes them public.
- Some YouTube videos may not permit embedding, depending on the uploader's settings.
- There is no installer polish or code signing because the project keeps the cost at zero.

## Design Principles

- Local-first by default.
- No paid services.
- No cloud dependencies.
- No accounts.
- No telemetry.
- Minimal frontend surface.
- Boring, inspectable Rust backend.

kaapi wire is meant to feel like a tiny desktop instrument: quiet most of the time, useful when something important breaks.
