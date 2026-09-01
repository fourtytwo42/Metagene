use crate::simulator::SimulationReport;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use thiserror::Error;

const DATABASE_NAME: &str = "matchup-cache.sqlite";
const MAX_ENTRIES: i64 = 10_000;
const MAX_JSON_BYTES: i64 = 256 * 1024 * 1024;

static ACCESS_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Error)]
pub enum MatchCacheError {
    #[error("match cache file error: {0}")]
    Io(#[from] std::io::Error),
    #[error("match cache database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("match cache serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("match cache lock is unavailable")]
    Lock,
}

#[derive(Clone, Debug, Serialize)]
pub struct MatchCacheStatus {
    pub entries: usize,
    pub hits: u64,
    pub logical_bytes: u64,
    pub database_bytes: u64,
    pub max_entries: usize,
    pub max_logical_bytes: u64,
}

pub fn key<T: Serialize>(
    engine: &str,
    revision: &str,
    input: &T,
) -> Result<String, MatchCacheError> {
    let mut hasher = Sha256::new();
    hasher.update(b"metagene-match-cache-v2\0");
    hasher.update(engine.as_bytes());
    hasher.update(b"\0");
    hasher.update(revision.as_bytes());
    hasher.update(b"\0");
    serde_json::to_writer(HashWriter(&mut hasher), input)?;
    Ok(format!("{engine}:{:x}", hasher.finalize()))
}

pub fn lookup(
    data_dir: &Path,
    cache_key: &str,
) -> Result<Option<SimulationReport>, MatchCacheError> {
    let _guard = access_lock().lock().map_err(|_| MatchCacheError::Lock)?;
    let connection = open(data_dir)?;
    let json = connection
        .query_row(
            "SELECT report_json FROM matchup_cache WHERE cache_key=?1",
            [cache_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(json) = json else {
        return Ok(None);
    };
    connection.execute(
        "UPDATE matchup_cache
         SET last_used_at=datetime('now'), hit_count=hit_count+1
         WHERE cache_key=?1",
        [cache_key],
    )?;
    let mut report: SimulationReport = serde_json::from_str(&json)?;
    report.cached_games = report.games;
    Ok(Some(report))
}

pub fn store(
    data_dir: &Path,
    cache_key: &str,
    engine: &str,
    revision: &str,
    report: &SimulationReport,
) -> Result<(), MatchCacheError> {
    if report.crashes > 0 {
        return Ok(());
    }
    let _guard = access_lock().lock().map_err(|_| MatchCacheError::Lock)?;
    let connection = open(data_dir)?;
    let mut stored = report.clone();
    stored.cached_games = 0;
    let json = serde_json::to_string(&stored)?;
    connection.execute(
        "INSERT INTO matchup_cache
           (cache_key, engine, engine_revision, report_json, json_bytes)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(cache_key) DO UPDATE SET
           report_json=excluded.report_json,
           json_bytes=excluded.json_bytes,
           last_used_at=datetime('now')",
        params![cache_key, engine, revision, json, json.len() as i64],
    )?;
    prune(&connection)?;
    Ok(())
}

pub fn status(data_dir: &Path) -> Result<MatchCacheStatus, MatchCacheError> {
    let _guard = access_lock().lock().map_err(|_| MatchCacheError::Lock)?;
    let connection = open(data_dir)?;
    let (entries, hits, logical_bytes) = connection.query_row(
        "SELECT COUNT(*), COALESCE(SUM(hit_count), 0), COALESCE(SUM(json_bytes), 0)
         FROM matchup_cache",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        },
    )?;
    Ok(MatchCacheStatus {
        entries: entries.max(0) as usize,
        hits: hits.max(0) as u64,
        logical_bytes: logical_bytes.max(0) as u64,
        database_bytes: database_size(data_dir),
        max_entries: MAX_ENTRIES as usize,
        max_logical_bytes: MAX_JSON_BYTES as u64,
    })
}

pub fn clear(data_dir: &Path) -> Result<MatchCacheStatus, MatchCacheError> {
    let _guard = access_lock().lock().map_err(|_| MatchCacheError::Lock)?;
    let connection = open(data_dir)?;
    connection.execute("DELETE FROM matchup_cache", [])?;
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")?;
    drop(connection);
    Ok(MatchCacheStatus {
        entries: 0,
        hits: 0,
        logical_bytes: 0,
        database_bytes: database_size(data_dir),
        max_entries: MAX_ENTRIES as usize,
        max_logical_bytes: MAX_JSON_BYTES as u64,
    })
}

fn access_lock() -> &'static Mutex<()> {
    ACCESS_LOCK.get_or_init(|| Mutex::new(()))
}

fn database_path(data_dir: &Path) -> PathBuf {
    data_dir.join(DATABASE_NAME)
}

fn open(data_dir: &Path) -> Result<Connection, MatchCacheError> {
    fs::create_dir_all(data_dir)?;
    let connection = Connection::open(database_path(data_dir))?;
    connection.busy_timeout(std::time::Duration::from_secs(10))?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS matchup_cache (
           cache_key TEXT PRIMARY KEY,
           engine TEXT NOT NULL,
           engine_revision TEXT NOT NULL,
           report_json TEXT NOT NULL,
           json_bytes INTEGER NOT NULL,
           created_at TEXT NOT NULL DEFAULT (datetime('now')),
           last_used_at TEXT NOT NULL DEFAULT (datetime('now')),
           hit_count INTEGER NOT NULL DEFAULT 0
         );
         CREATE INDEX IF NOT EXISTS matchup_cache_lru
           ON matchup_cache(last_used_at);",
    )?;
    Ok(connection)
}

fn prune(connection: &Connection) -> Result<(), rusqlite::Error> {
    loop {
        let (entries, bytes) = connection.query_row(
            "SELECT COUNT(*), COALESCE(SUM(json_bytes), 0) FROM matchup_cache",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )?;
        if entries <= MAX_ENTRIES && bytes <= MAX_JSON_BYTES {
            return Ok(());
        }
        let remove_for_count = entries.saturating_sub(MAX_ENTRIES);
        let average_bytes = (bytes / entries.max(1)).max(1);
        let remove_for_bytes = bytes
            .saturating_sub(MAX_JSON_BYTES)
            .saturating_add(average_bytes - 1)
            / average_bytes;
        let remove = remove_for_count.max(remove_for_bytes).clamp(1, 512);
        connection.execute(
            "DELETE FROM matchup_cache WHERE cache_key IN (
               SELECT cache_key FROM matchup_cache
               ORDER BY last_used_at ASC, rowid ASC LIMIT ?1
             )",
            [remove],
        )?;
    }
}

fn database_size(data_dir: &Path) -> u64 {
    let path = database_path(data_dir);
    [
        path.clone(),
        PathBuf::from(format!("{}-wal", path.display())),
        PathBuf::from(format!("{}-shm", path.display())),
    ]
    .iter()
    .filter_map(|path| fs::metadata(path).ok().map(|metadata| metadata.len()))
    .sum()
}

struct HashWriter<'a>(&'a mut Sha256);

impl std::io::Write for HashWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0.update(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulator::SimulationReport;
    use std::collections::HashMap;
    use uuid::Uuid;

    fn report() -> SimulationReport {
        SimulationReport {
            engine: "test".into(),
            schema_version: "1".into(),
            engine_routes: HashMap::new(),
            route_failures: Vec::new(),
            provenance: Some(crate::provenance::ResultProvenance::new(
                "revision",
                "cards-revision",
                None,
                "sha256:configuration",
            )),
            games: 128,
            draws: 0,
            crashes: 0,
            cached_games: 0,
            seat_order: Vec::new(),
            average_turns: 8.0,
            games_per_second: 100.0,
            rows: Vec::new(),
            matchup_matrix: HashMap::new(),
            matchup_wins: HashMap::new(),
            matchup_games: HashMap::new(),
            matchup_weights: HashMap::new(),
            commander_evidence: None,
            combo_execution: None,
            performance: None,
        }
    }

    #[test]
    fn caches_successful_reports_and_clears_storage() {
        let data_dir = std::env::temp_dir().join(format!("metagene-cache-{}", Uuid::new_v4()));
        let cache_key = key("test", "revision", &("deck-a", "deck-b", 42)).unwrap();
        assert!(lookup(&data_dir, &cache_key).unwrap().is_none());
        store(&data_dir, &cache_key, "test", "revision", &report()).unwrap();
        let cached = lookup(&data_dir, &cache_key).unwrap().unwrap();
        assert_eq!(cached.cached_games, 128);
        assert_eq!(
            cached.provenance.unwrap().card_data_revision,
            "cards-revision"
        );
        let status = status(&data_dir).unwrap();
        assert_eq!(status.entries, 1);
        assert_eq!(status.hits, 1);
        assert!(status.logical_bytes > 0);
        assert_eq!(clear(&data_dir).unwrap().entries, 0);
        fs::remove_dir_all(data_dir).unwrap();
    }

    #[test]
    fn does_not_cache_reports_with_crashes() {
        let data_dir = std::env::temp_dir().join(format!("metagene-cache-{}", Uuid::new_v4()));
        let cache_key = key("test", "revision", &"crashy").unwrap();
        let mut crashy = report();
        crashy.crashes = 1;
        store(&data_dir, &cache_key, "test", "revision", &crashy).unwrap();
        assert!(lookup(&data_dir, &cache_key).unwrap().is_none());
        fs::remove_dir_all(data_dir).unwrap();
    }

    #[test]
    fn keys_change_with_revision_and_deterministic_inputs() {
        let first = key("hexdek", "rev-a", &("deck", 42)).unwrap();
        assert_eq!(first, key("hexdek", "rev-a", &("deck", 42)).unwrap());
        assert_ne!(first, key("hexdek", "rev-b", &("deck", 42)).unwrap());
        assert_ne!(first, key("hexdek", "rev-a", &("deck", 43)).unwrap());
    }
}
