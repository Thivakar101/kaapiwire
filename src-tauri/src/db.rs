use crate::pipeline::{importance_tag_for, NewsItem, RawItem};
use rusqlite::{params, Connection, OptionalExtension};
use std::{path::Path, sync::Mutex};

pub struct Db {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone)]
pub struct SeenCandidate {
    pub id: String,
    pub canonical_id: String,
    pub title: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct RawCandidate {
    pub title: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct MetricSnapshot {
    pub score: u32,
    pub comments: u32,
    pub observed_at: i64,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path)
            .map_err(|error| format!("Could not open sqlite database: {error}"))?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.init()?;
        Ok(db)
    }

    pub fn record_raw_item(&self, item: &RawItem, observed_at: i64) -> Result<(), String> {
        let conn = self.connection()?;
        conn.execute(
            "INSERT OR REPLACE INTO raw_items
             (id, observed_at, title, url, source, source_kind, timestamp, raw_score, comments, section)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                &item.id,
                observed_at,
                &item.title,
                &item.url,
                &item.source,
                item.source_kind.as_str(),
                item.timestamp,
                item.raw_score as i64,
                item.comments as i64,
                &item.section
            ],
        )
        .map_err(|error| format!("Could not log raw item: {error}"))?;
        Ok(())
    }

    pub fn previous_metric(&self, item_id: &str) -> Result<Option<MetricSnapshot>, String> {
        let conn = self.connection()?;
        conn.query_row(
            "SELECT last_score, last_comments, last_observed_at
             FROM item_metrics
             WHERE item_id = ?1",
            params![item_id],
            |row| {
                let score = row.get::<_, i64>(0)?.max(0) as u32;
                let comments = row.get::<_, i64>(1)?.max(0) as u32;
                Ok(MetricSnapshot {
                    score,
                    comments,
                    observed_at: row.get::<_, i64>(2)?,
                })
            },
        )
        .optional()
        .map_err(|error| format!("Could not read item metric: {error}"))
    }

    pub fn upsert_metric(&self, item: &RawItem, observed_at: i64) -> Result<(), String> {
        let conn = self.connection()?;
        conn.execute(
            "INSERT INTO item_metrics
             (item_id, source_kind, last_score, last_comments, last_observed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(item_id) DO UPDATE SET
               source_kind = excluded.source_kind,
               last_score = excluded.last_score,
               last_comments = excluded.last_comments,
               last_observed_at = excluded.last_observed_at",
            params![
                &item.id,
                item.source_kind.as_str(),
                item.raw_score as i64,
                item.comments as i64,
                observed_at
            ],
        )
        .map_err(|error| format!("Could not update item metric: {error}"))?;
        Ok(())
    }

    pub fn has_seen_id(&self, item_id: &str) -> Result<bool, String> {
        let conn = self.connection()?;
        let exists: i64 = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM seen_items WHERE id = ?1)",
                params![item_id],
                |row| row.get(0),
            )
            .map_err(|error| format!("Could not check seen item: {error}"))?;
        Ok(exists == 1)
    }

    pub fn recent_seen_candidates(
        &self,
        timestamp: i64,
        window_seconds: i64,
    ) -> Result<Vec<SeenCandidate>, String> {
        let conn = self.connection()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, canonical_id, title, source
                 FROM seen_items
                 WHERE timestamp BETWEEN ?1 AND ?2
                 ORDER BY timestamp ASC, first_seen ASC
                 LIMIT 240",
            )
            .map_err(|error| format!("Could not prepare seen candidate query: {error}"))?;
        let rows = stmt
            .query_map(
                params![timestamp - window_seconds, timestamp + window_seconds],
                |row| {
                    Ok(SeenCandidate {
                        id: row.get(0)?,
                        canonical_id: row.get(1)?,
                        title: row.get(2)?,
                        source: row.get(3)?,
                    })
                },
            )
            .map_err(|error| format!("Could not query seen candidates: {error}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Could not read seen candidates: {error}"))
    }

    pub fn recent_raw_candidates(
        &self,
        timestamp: i64,
        window_seconds: i64,
    ) -> Result<Vec<RawCandidate>, String> {
        let conn = self.connection()?;
        let mut stmt = conn
            .prepare(
                "SELECT title, source
                 FROM raw_items
                 WHERE timestamp BETWEEN ?1 AND ?2
                 ORDER BY observed_at DESC
                 LIMIT 320",
            )
            .map_err(|error| format!("Could not prepare raw candidate query: {error}"))?;
        let rows = stmt
            .query_map(
                params![timestamp - window_seconds, timestamp + window_seconds],
                |row| {
                    Ok(RawCandidate {
                        title: row.get(0)?,
                        source: row.get(1)?,
                    })
                },
            )
            .map_err(|error| format!("Could not query raw candidates: {error}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Could not read raw candidates: {error}"))
    }

    pub fn insert_seen_item(
        &self,
        raw: &RawItem,
        canonical_id: &str,
        duplicate_of: Option<&str>,
        item: &NewsItem,
        visible: bool,
        first_seen: i64,
    ) -> Result<(), String> {
        let conn = self.connection()?;
        conn.execute(
            "INSERT OR IGNORE INTO seen_items
             (id, canonical_id, title, url, source, timestamp, first_seen, raw_score,
              importance, relevance, tag, visible, duplicate_of, section)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                &raw.id,
                canonical_id,
                &item.title,
                &item.url,
                &item.source,
                item.timestamp,
                first_seen,
                item.raw_score as i64,
                item.importance as i64,
                item.relevance as i64,
                &item.tag,
                if visible { 1 } else { 0 },
                duplicate_of,
                &item.section
            ],
        )
        .map_err(|error| format!("Could not insert seen item: {error}"))?;
        Ok(())
    }

    pub fn recent_visible_items(&self, limit: usize) -> Result<Vec<NewsItem>, String> {
        let conn = self.connection()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, title, url, source, timestamp, raw_score, importance, relevance, section
                 FROM seen_items
                 WHERE visible = 1
                 ORDER BY timestamp DESC, first_seen DESC
                 LIMIT ?1",
            )
            .map_err(|error| format!("Could not prepare visible item query: {error}"))?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                let raw_score = row.get::<_, i64>(5)?.max(0) as u32;
                let importance = row.get::<_, i64>(6)?.clamp(0, 100) as u8;
                let relevance = row.get::<_, i64>(7)?.clamp(0, 100) as u8;
                Ok(NewsItem {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    url: row.get(2)?,
                    source: row.get(3)?,
                    timestamp: row.get(4)?,
                    raw_score,
                    importance,
                    relevance,
                    tag: importance_tag_for(importance).into(),
                    section: row.get(8)?,
                })
            })
            .map_err(|error| format!("Could not query visible items: {error}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Could not read visible items: {error}"))
    }

    pub fn recent_logged_items(&self, limit: usize) -> Result<Vec<NewsItem>, String> {
        let conn = self.connection()?;
        let mut stmt = conn
            .prepare(
                "SELECT r.id, r.title, r.url, r.source, r.timestamp, r.raw_score, r.section
                 FROM raw_items r
                 INNER JOIN (
                   SELECT id, MAX(observed_at) AS latest_observed
                   FROM raw_items
                   GROUP BY id
                 ) latest ON latest.id = r.id AND latest.latest_observed = r.observed_at
                 ORDER BY r.timestamp DESC, r.observed_at DESC
                 LIMIT ?1",
            )
            .map_err(|error| format!("Could not prepare logged item query: {error}"))?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                let raw_score = row.get::<_, i64>(5)?.max(0) as u32;
                Ok(NewsItem {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    url: row.get(2)?,
                    source: row.get(3)?,
                    timestamp: row.get(4)?,
                    raw_score,
                    importance: 0,
                    relevance: 0,
                    tag: importance_tag_for(0).into(),
                    section: row.get(6)?,
                })
            })
            .map_err(|error| format!("Could not query logged items: {error}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Could not read logged items: {error}"))
    }

    pub fn prune_raw_items(&self, older_than: i64) -> Result<(), String> {
        let conn = self.connection()?;
        conn.execute(
            "DELETE FROM raw_items WHERE observed_at < ?1",
            params![older_than],
        )
        .map_err(|error| format!("Could not prune raw items: {error}"))?;
        Ok(())
    }

    fn init(&self) -> Result<(), String> {
        let conn = self.connection()?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA temp_store = MEMORY;

            CREATE TABLE IF NOT EXISTS seen_items (
                id TEXT PRIMARY KEY,
                canonical_id TEXT NOT NULL,
                title TEXT NOT NULL,
                url TEXT NOT NULL,
                source TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                first_seen INTEGER NOT NULL,
                raw_score INTEGER NOT NULL DEFAULT 0,
                importance INTEGER NOT NULL DEFAULT 0,
                relevance INTEGER NOT NULL DEFAULT 0,
                tag TEXT NOT NULL DEFAULT 'General',
                section TEXT NOT NULL DEFAULT 'Tech',
                visible INTEGER NOT NULL DEFAULT 0,
                duplicate_of TEXT
            );

            CREATE TABLE IF NOT EXISTS raw_items (
                id TEXT NOT NULL,
                observed_at INTEGER NOT NULL,
                title TEXT NOT NULL,
                url TEXT NOT NULL,
                source TEXT NOT NULL,
                source_kind TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                raw_score INTEGER NOT NULL DEFAULT 0,
                comments INTEGER NOT NULL DEFAULT 0,
                section TEXT NOT NULL DEFAULT 'Tech',
                PRIMARY KEY (id, observed_at)
            );

            CREATE TABLE IF NOT EXISTS item_metrics (
                item_id TEXT PRIMARY KEY,
                source_kind TEXT NOT NULL,
                last_score INTEGER NOT NULL DEFAULT 0,
                last_comments INTEGER NOT NULL DEFAULT 0,
                last_observed_at INTEGER NOT NULL DEFAULT 0
            );

            CREATE INDEX IF NOT EXISTS idx_seen_timestamp ON seen_items(timestamp);
            CREATE INDEX IF NOT EXISTS idx_seen_visible ON seen_items(visible, timestamp);
            CREATE INDEX IF NOT EXISTS idx_raw_timestamp ON raw_items(timestamp);
            CREATE INDEX IF NOT EXISTS idx_raw_observed ON raw_items(observed_at);
            ",
        )
        .map_err(|error| format!("Could not initialize sqlite schema: {error}"))?;
        add_column_if_missing(
            &conn,
            "seen_items",
            "section",
            "TEXT NOT NULL DEFAULT 'Tech'",
        )?;
        add_column_if_missing(
            &conn,
            "raw_items",
            "section",
            "TEXT NOT NULL DEFAULT 'Tech'",
        )?;
        Ok(())
    }

    fn connection(&self) -> Result<std::sync::MutexGuard<'_, Connection>, String> {
        self.conn
            .lock()
            .map_err(|_| "sqlite connection lock was poisoned".to_string())
    }
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), String> {
    let sql = format!("ALTER TABLE {table} ADD COLUMN {column} {definition}");
    match conn.execute(&sql, []) {
        Ok(_) => Ok(()),
        Err(error) if error.to_string().contains("duplicate column name") => Ok(()),
        Err(error) => Err(format!("Could not migrate sqlite table {table}: {error}")),
    }
}
