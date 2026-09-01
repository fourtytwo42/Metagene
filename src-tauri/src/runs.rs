use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RunStoreError {
    #[error("failed to access run storage: {0}")]
    Io(#[from] std::io::Error),
    #[error("run database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("failed to serialize run data: {0}")]
    Json(#[from] serde_json::Error),
    #[error("saved run not found: {0}")]
    NotFound(String),
}

#[derive(Clone, Debug, Serialize)]
pub struct RunSummary {
    pub id: String,
    pub created_at: String,
    pub platform: String,
    pub format: String,
    pub state: String,
    pub best_score: Option<f64>,
    pub evaluated_candidates: u64,
    pub elapsed_ms: u64,
    pub title: String,
    pub notes: String,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RunMetadata {
    pub id: String,
    pub title: String,
    pub notes: String,
    pub tags: Vec<String>,
}

pub struct StoredRunRow {
    pub request_json: String,
    pub status_json: String,
}

pub struct StoredValidationRow {
    pub id: String,
    pub run_id: Option<String>,
    pub candidate_rank: Option<usize>,
    pub created_at: String,
    pub request_json: String,
    pub report_json: String,
}

pub struct StoredHalvingRow {
    pub id: String,
    pub run_id: Option<String>,
    pub created_at: String,
    pub request_json: String,
    pub report_json: String,
}

pub struct StoredBestOfThreeRow {
    pub id: String,
    pub run_id: Option<String>,
    pub candidate_rank: Option<usize>,
    pub created_at: String,
    pub request_json: String,
    pub report_json: String,
}

pub struct StoredSideboardEvolutionRow {
    pub id: String,
    pub run_id: Option<String>,
    pub candidate_rank: Option<usize>,
    pub created_at: String,
    pub request_json: String,
    pub report_json: String,
}

#[derive(Clone, Copy)]
pub struct RunMetrics {
    pub best_score: Option<f64>,
    pub evaluated_candidates: u64,
    pub elapsed_ms: u64,
}

pub fn create<T: Serialize, S: Serialize>(
    data_dir: &Path,
    id: &str,
    platform: &str,
    format: &str,
    request: &T,
    status: &S,
) -> Result<(), RunStoreError> {
    let connection = open(data_dir)?;
    connection.execute(
        "INSERT INTO runs (id, platform, format, state, request_json, status_json)
         VALUES (?1, ?2, ?3, 'running', ?4, ?5)",
        params![
            id,
            platform,
            format,
            serde_json::to_string(request)?,
            serde_json::to_string(status)?,
        ],
    )?;
    connection.execute(
        "DELETE FROM runs WHERE id NOT IN (
           SELECT id FROM runs ORDER BY created_at DESC, rowid DESC LIMIT 200
         )",
        [],
    )?;
    Ok(())
}

/// Restarts a completed/interrupted structural run in place. Keeping the same
/// row preserves its title, notes, tags, validation evidence, and run-history
/// identity while request/status move atomically to the next generation segment.
pub fn restart<T: Serialize, S: Serialize>(
    data_dir: &Path,
    id: &str,
    request: &T,
    status: &S,
    metrics: RunMetrics,
) -> Result<(), RunStoreError> {
    let connection = open(data_dir)?;
    let changed = connection.execute(
        "UPDATE runs SET state='running', request_json=?2, status_json=?3,
         best_score=?4, evaluated_candidates=?5, elapsed_ms=?6,
         updated_at=datetime('now') WHERE id=?1",
        params![
            id,
            serde_json::to_string(request)?,
            serde_json::to_string(status)?,
            metrics.best_score,
            metrics.evaluated_candidates.min(i64::MAX as u64) as i64,
            metrics.elapsed_ms.min(i64::MAX as u64) as i64,
        ],
    )?;
    if changed == 0 {
        return Err(RunStoreError::NotFound(id.into()));
    }
    Ok(())
}

pub fn checkpoint<S: Serialize>(
    data_dir: &Path,
    id: &str,
    status: &S,
    best_score: Option<f64>,
    evaluated_candidates: u64,
    elapsed_ms: u64,
) -> Result<(), RunStoreError> {
    let connection = open(data_dir)?;
    connection.execute(
        "UPDATE runs SET status_json=?2, best_score=?3, evaluated_candidates=?4,
         elapsed_ms=?5, updated_at=datetime('now') WHERE id=?1",
        params![
            id,
            serde_json::to_string(status)?,
            best_score,
            evaluated_candidates.min(i64::MAX as u64) as i64,
            elapsed_ms.min(i64::MAX as u64) as i64,
        ],
    )?;
    Ok(())
}

/// Atomically persists the request-side recovery metadata and its matching job snapshot.
/// Hybrid recovery depends on these two JSON documents describing the same phase boundary.
pub fn checkpoint_with_request<T: Serialize, S: Serialize>(
    data_dir: &Path,
    id: &str,
    request: &T,
    status: &S,
    metrics: RunMetrics,
) -> Result<(), RunStoreError> {
    let connection = open(data_dir)?;
    connection.execute(
        "UPDATE runs SET request_json=?2, status_json=?3, best_score=?4,
         evaluated_candidates=?5, elapsed_ms=?6, updated_at=datetime('now') WHERE id=?1",
        params![
            id,
            serde_json::to_string(request)?,
            serde_json::to_string(status)?,
            metrics.best_score,
            metrics.evaluated_candidates.min(i64::MAX as u64) as i64,
            metrics.elapsed_ms.min(i64::MAX as u64) as i64,
        ],
    )?;
    Ok(())
}

pub fn finish<S: Serialize>(
    data_dir: &Path,
    id: &str,
    state: &str,
    status: &S,
    best_score: Option<f64>,
    evaluated_candidates: u64,
    elapsed_ms: u64,
) -> Result<(), RunStoreError> {
    let connection = open(data_dir)?;
    connection.execute(
        "UPDATE runs SET state=?2, status_json=?3, best_score=?4,
         evaluated_candidates=?5, elapsed_ms=?6, updated_at=datetime('now') WHERE id=?1",
        params![
            id,
            state,
            serde_json::to_string(status)?,
            best_score,
            evaluated_candidates.min(i64::MAX as u64) as i64,
            elapsed_ms.min(i64::MAX as u64) as i64,
        ],
    )?;
    Ok(())
}

/// Atomically closes a run with the recovery request that describes its final durable boundary.
pub fn finish_with_request<T: Serialize, S: Serialize>(
    data_dir: &Path,
    id: &str,
    state: &str,
    request: &T,
    status: &S,
    metrics: RunMetrics,
) -> Result<(), RunStoreError> {
    let connection = open(data_dir)?;
    connection.execute(
        "UPDATE runs SET state=?2, request_json=?3, status_json=?4, best_score=?5,
         evaluated_candidates=?6, elapsed_ms=?7, updated_at=datetime('now') WHERE id=?1",
        params![
            id,
            state,
            serde_json::to_string(request)?,
            serde_json::to_string(status)?,
            metrics.best_score,
            metrics.evaluated_candidates.min(i64::MAX as u64) as i64,
            metrics.elapsed_ms.min(i64::MAX as u64) as i64,
        ],
    )?;
    Ok(())
}

/// Marks jobs that could not have survived a desktop-process restart as interrupted.
/// Their last durable progress snapshot remains intact and can be used to resume.
pub fn recover_interrupted(data_dir: &Path) -> Result<usize, RunStoreError> {
    let mut connection = open(data_dir)?;
    let transaction = connection.transaction()?;
    let rows = {
        let mut statement =
            transaction.prepare("SELECT id, status_json FROM runs WHERE state='running'")?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    for (id, status_json) in &rows {
        let mut status: serde_json::Value =
            serde_json::from_str(status_json).unwrap_or_else(|_| {
                serde_json::json!({
                    "id": id,
                    "state": "running",
                    "progress": null,
                    "result": null,
                    "error": null
                })
            });
        status["state"] = serde_json::Value::String("interrupted".into());
        status["error"] = serde_json::Value::String(
            "The desktop process stopped before this run completed. Resume from the last durable checkpoint."
                .into(),
        );
        transaction.execute(
            "UPDATE runs SET state='interrupted', status_json=?2, updated_at=datetime('now')
             WHERE id=?1",
            params![id, serde_json::to_string(&status)?],
        )?;
    }
    transaction.commit()?;
    Ok(rows.len())
}

pub fn list(data_dir: &Path, limit: usize) -> Result<Vec<RunSummary>, RunStoreError> {
    let connection = open(data_dir)?;
    let mut statement = connection.prepare(
        "SELECT id, created_at, platform, format, state, best_score,
                evaluated_candidates, elapsed_ms, title, notes, tags_json
         FROM runs ORDER BY created_at DESC, rowid DESC LIMIT ?1",
    )?;
    let rows = statement.query_map([limit.min(200) as i64], |row| {
        Ok(RunSummary {
            id: row.get(0)?,
            created_at: row.get(1)?,
            platform: row.get(2)?,
            format: row.get(3)?,
            state: row.get(4)?,
            best_score: row.get(5)?,
            evaluated_candidates: row.get::<_, i64>(6)?.max(0) as u64,
            elapsed_ms: row.get::<_, i64>(7)?.max(0) as u64,
            title: row.get(8)?,
            notes: row.get(9)?,
            tags: serde_json::from_str(&row.get::<_, String>(10)?).unwrap_or_default(),
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn update_metadata(
    data_dir: &Path,
    metadata: &RunMetadata,
) -> Result<RunSummary, RunStoreError> {
    let title = metadata.title.trim();
    let notes = metadata.notes.trim();
    let tags = metadata
        .tags
        .iter()
        .map(|tag| tag.trim().to_ascii_lowercase())
        .filter(|tag| !tag.is_empty())
        .take(12)
        .collect::<Vec<_>>();
    let connection = open(data_dir)?;
    let changed = connection.execute(
        "UPDATE runs SET title=?2,notes=?3,tags_json=?4,updated_at=datetime('now') WHERE id=?1",
        params![
            metadata.id,
            title.chars().take(120).collect::<String>(),
            notes.chars().take(4_000).collect::<String>(),
            serde_json::to_string(&tags)?
        ],
    )?;
    if changed == 0 {
        return Err(RunStoreError::NotFound(metadata.id.clone()));
    }
    list(data_dir, 200)?
        .into_iter()
        .find(|run| run.id == metadata.id)
        .ok_or_else(|| RunStoreError::NotFound(metadata.id.clone()))
}

pub fn export_manifest(data_dir: &Path, id: &str) -> Result<String, RunStoreError> {
    #[derive(Serialize)]
    struct Manifest {
        schema_version: u32,
        run: RunSummary,
        request: serde_json::Value,
        outcome: serde_json::Value,
    }
    let run = list(data_dir, 200)?
        .into_iter()
        .find(|run| run.id == id)
        .ok_or_else(|| RunStoreError::NotFound(id.into()))?;
    let stored = load(data_dir, id)?;
    serde_json::to_string_pretty(&Manifest {
        schema_version: 1,
        run,
        request: serde_json::from_str(&stored.request_json)?,
        outcome: serde_json::from_str(&stored.status_json)?,
    })
    .map_err(Into::into)
}

pub fn load(data_dir: &Path, id: &str) -> Result<StoredRunRow, RunStoreError> {
    let connection = open(data_dir)?;
    connection
        .query_row(
            "SELECT request_json, status_json FROM runs WHERE id=?1",
            [id],
            |row| {
                Ok(StoredRunRow {
                    request_json: row.get(0)?,
                    status_json: row.get(1)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| RunStoreError::NotFound(id.into()))
}

pub fn save_validation<T: Serialize, R: Serialize>(
    data_dir: &Path,
    run_id: Option<&str>,
    candidate_rank: Option<usize>,
    request: &T,
    report: &R,
) -> Result<String, RunStoreError> {
    let connection = open(data_dir)?;
    let id = uuid::Uuid::new_v4().to_string();
    connection.execute(
        "INSERT INTO validations
         (id, run_id, candidate_rank, request_json, report_json)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            id,
            run_id,
            candidate_rank.map(|rank| rank.min(i64::MAX as usize) as i64),
            serde_json::to_string(request)?,
            serde_json::to_string(report)?,
        ],
    )?;
    connection.execute(
        "DELETE FROM validations WHERE id NOT IN (
           SELECT id FROM validations ORDER BY created_at DESC, rowid DESC LIMIT 500
         )",
        [],
    )?;
    Ok(id)
}

pub fn list_validations(
    data_dir: &Path,
    run_id: Option<&str>,
    limit: usize,
) -> Result<Vec<StoredValidationRow>, RunStoreError> {
    let connection = open(data_dir)?;
    let mut statement = connection.prepare(
        "SELECT id, run_id, candidate_rank, created_at, request_json, report_json
         FROM validations
         WHERE (?1 IS NULL OR run_id = ?1)
         ORDER BY created_at DESC, rowid DESC LIMIT ?2",
    )?;
    let rows = statement.query_map(params![run_id, limit.min(500) as i64], |row| {
        Ok(StoredValidationRow {
            id: row.get(0)?,
            run_id: row.get(1)?,
            candidate_rank: row
                .get::<_, Option<i64>>(2)?
                .map(|rank| rank.max(0) as usize),
            created_at: row.get(3)?,
            request_json: row.get(4)?,
            report_json: row.get(5)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn save_halving<T: Serialize, R: Serialize>(
    data_dir: &Path,
    run_id: Option<&str>,
    request: &T,
    report: &R,
) -> Result<String, RunStoreError> {
    let connection = open(data_dir)?;
    let id = uuid::Uuid::new_v4().to_string();
    connection.execute(
        "INSERT INTO halving_runs (id, run_id, request_json, report_json)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            id,
            run_id,
            serde_json::to_string(request)?,
            serde_json::to_string(report)?,
        ],
    )?;
    connection.execute(
        "DELETE FROM halving_runs WHERE id NOT IN (
           SELECT id FROM halving_runs ORDER BY created_at DESC, rowid DESC LIMIT 100
         )",
        [],
    )?;
    Ok(id)
}

pub fn save_best_of_three<T: Serialize, R: Serialize>(
    data_dir: &Path,
    run_id: Option<&str>,
    candidate_rank: Option<usize>,
    request: &T,
    report: &R,
) -> Result<String, RunStoreError> {
    let connection = open(data_dir)?;
    let id = uuid::Uuid::new_v4().to_string();
    connection.execute(
        "INSERT INTO best_of_three_validations
         (id, run_id, candidate_rank, request_json, report_json)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            id,
            run_id,
            candidate_rank.map(|rank| rank.min(i64::MAX as usize) as i64),
            serde_json::to_string(request)?,
            serde_json::to_string(report)?,
        ],
    )?;
    connection.execute(
        "DELETE FROM best_of_three_validations WHERE id NOT IN (
           SELECT id FROM best_of_three_validations
           ORDER BY created_at DESC, rowid DESC LIMIT 500
         )",
        [],
    )?;
    Ok(id)
}

pub fn list_best_of_three(
    data_dir: &Path,
    run_id: Option<&str>,
    limit: usize,
) -> Result<Vec<StoredBestOfThreeRow>, RunStoreError> {
    let connection = open(data_dir)?;
    let mut statement = connection.prepare(
        "SELECT id, run_id, candidate_rank, created_at, request_json, report_json
         FROM best_of_three_validations
         WHERE (?1 IS NULL OR run_id = ?1)
         ORDER BY created_at DESC, rowid DESC LIMIT ?2",
    )?;
    let rows = statement.query_map(params![run_id, limit.min(500) as i64], |row| {
        Ok(StoredBestOfThreeRow {
            id: row.get(0)?,
            run_id: row.get(1)?,
            candidate_rank: row
                .get::<_, Option<i64>>(2)?
                .map(|rank| rank.max(0) as usize),
            created_at: row.get(3)?,
            request_json: row.get(4)?,
            report_json: row.get(5)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn save_sideboard_evolution<T: Serialize, R: Serialize>(
    data_dir: &Path,
    run_id: Option<&str>,
    candidate_rank: Option<usize>,
    request: &T,
    report: &R,
) -> Result<String, RunStoreError> {
    let connection = open(data_dir)?;
    let id = uuid::Uuid::new_v4().to_string();
    connection.execute(
        "INSERT INTO sideboard_evolutions
         (id, run_id, candidate_rank, request_json, report_json)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            id,
            run_id,
            candidate_rank.map(|rank| rank.min(i64::MAX as usize) as i64),
            serde_json::to_string(request)?,
            serde_json::to_string(report)?,
        ],
    )?;
    connection.execute(
        "DELETE FROM sideboard_evolutions WHERE id NOT IN (
           SELECT id FROM sideboard_evolutions
           ORDER BY created_at DESC, rowid DESC LIMIT 250
         )",
        [],
    )?;
    Ok(id)
}

pub fn list_sideboard_evolutions(
    data_dir: &Path,
    run_id: Option<&str>,
    limit: usize,
) -> Result<Vec<StoredSideboardEvolutionRow>, RunStoreError> {
    let connection = open(data_dir)?;
    let mut statement = connection.prepare(
        "SELECT id, run_id, candidate_rank, created_at, request_json, report_json
         FROM sideboard_evolutions
         WHERE (?1 IS NULL OR run_id = ?1)
         ORDER BY created_at DESC, rowid DESC LIMIT ?2",
    )?;
    let rows = statement.query_map(params![run_id, limit.min(250) as i64], |row| {
        Ok(StoredSideboardEvolutionRow {
            id: row.get(0)?,
            run_id: row.get(1)?,
            candidate_rank: row
                .get::<_, Option<i64>>(2)?
                .map(|rank| rank.max(0) as usize),
            created_at: row.get(3)?,
            request_json: row.get(4)?,
            report_json: row.get(5)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn list_halvings(
    data_dir: &Path,
    run_id: Option<&str>,
    limit: usize,
) -> Result<Vec<StoredHalvingRow>, RunStoreError> {
    let connection = open(data_dir)?;
    let mut statement = connection.prepare(
        "SELECT id, run_id, created_at, request_json, report_json
         FROM halving_runs
         WHERE (?1 IS NULL OR run_id = ?1)
         ORDER BY created_at DESC, rowid DESC LIMIT ?2",
    )?;
    let rows = statement.query_map(params![run_id, limit.min(100) as i64], |row| {
        Ok(StoredHalvingRow {
            id: row.get(0)?,
            run_id: row.get(1)?,
            created_at: row.get(2)?,
            request_json: row.get(3)?,
            report_json: row.get(4)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn open(data_dir: &Path) -> Result<Connection, RunStoreError> {
    fs::create_dir_all(data_dir)?;
    let path: PathBuf = data_dir.join("runs.sqlite");
    let connection = Connection::open(path)?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS runs (
           id TEXT PRIMARY KEY,
           created_at TEXT NOT NULL DEFAULT (datetime('now')),
           updated_at TEXT NOT NULL DEFAULT (datetime('now')),
           platform TEXT NOT NULL,
           format TEXT NOT NULL,
           state TEXT NOT NULL,
           request_json TEXT NOT NULL,
           status_json TEXT NOT NULL,
           best_score REAL,
           evaluated_candidates INTEGER NOT NULL DEFAULT 0,
           elapsed_ms INTEGER NOT NULL DEFAULT 0
         );
         CREATE INDEX IF NOT EXISTS runs_created_at ON runs(created_at DESC);
         CREATE TABLE IF NOT EXISTS validations (
           id TEXT PRIMARY KEY,
           run_id TEXT,
           candidate_rank INTEGER,
           created_at TEXT NOT NULL DEFAULT (datetime('now')),
           request_json TEXT NOT NULL,
           report_json TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS validations_run_id
           ON validations(run_id, created_at DESC);
         CREATE TABLE IF NOT EXISTS halving_runs (
           id TEXT PRIMARY KEY,
           run_id TEXT,
           created_at TEXT NOT NULL DEFAULT (datetime('now')),
           request_json TEXT NOT NULL,
           report_json TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS halving_runs_run_id
           ON halving_runs(run_id, created_at DESC);
         CREATE TABLE IF NOT EXISTS best_of_three_validations (
           id TEXT PRIMARY KEY,
           run_id TEXT,
           candidate_rank INTEGER,
           created_at TEXT NOT NULL DEFAULT (datetime('now')),
           request_json TEXT NOT NULL,
           report_json TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS best_of_three_validations_run_id
           ON best_of_three_validations(run_id, created_at DESC);
         CREATE TABLE IF NOT EXISTS sideboard_evolutions (
           id TEXT PRIMARY KEY,
           run_id TEXT,
           candidate_rank INTEGER,
           created_at TEXT NOT NULL DEFAULT (datetime('now')),
           request_json TEXT NOT NULL,
           report_json TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS sideboard_evolutions_run_id
           ON sideboard_evolutions(run_id, created_at DESC);",
    )?;
    ensure_column(&connection, "runs", "title", "TEXT NOT NULL DEFAULT ''")?;
    ensure_column(&connection, "runs", "notes", "TEXT NOT NULL DEFAULT ''")?;
    ensure_column(
        &connection,
        "runs",
        "tags_json",
        "TEXT NOT NULL DEFAULT '[]'",
    )?;
    Ok(connection)
}

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), rusqlite::Error> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !names.iter().any(|name| name == column) {
        connection.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stores_compact_request_and_loadable_status() {
        let directory =
            std::env::temp_dir().join(format!("metagene-run-store-{}", uuid::Uuid::new_v4()));
        let request = json!({"platform":"arena","format":"historic","seed_deck":{}});
        let running = json!({"id":"run-1","state":"running"});
        create(&directory, "run-1", "arena", "historic", &request, &running).unwrap();
        let finished = json!({"id":"run-1","state":"completed"});
        finish(
            &directory,
            "run-1",
            "completed",
            &finished,
            Some(0.75),
            2048,
            320,
        )
        .unwrap();
        let history = list(&directory, 10).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].evaluated_candidates, 2048);
        let updated_request =
            json!({"platform":"arena","format":"historic","seed_deck":{"name":"validated"}});
        checkpoint_with_request(
            &directory,
            "run-1",
            &updated_request,
            &finished,
            RunMetrics {
                best_score: Some(0.75),
                evaluated_candidates: 2048,
                elapsed_ms: 320,
            },
        )
        .unwrap();
        assert_eq!(
            load(&directory, "run-1").unwrap().request_json,
            updated_request.to_string()
        );
        assert_eq!(
            load(&directory, "run-1").unwrap().status_json,
            finished.to_string()
        );
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn structural_continuation_restarts_the_same_row_without_losing_metadata() {
        let directory = std::env::temp_dir().join(format!(
            "metagene-run-continuation-{}",
            uuid::Uuid::new_v4()
        ));
        create(
            &directory,
            "run-1",
            "arena",
            "historic",
            &json!({"segment":1}),
            &json!({"id":"run-1","state":"completed"}),
        )
        .unwrap();
        update_metadata(
            &directory,
            &RunMetadata {
                id: "run-1".into(),
                title: "Keep me".into(),
                notes: "Prior evidence remains attached".into(),
                tags: vec!["tune".into()],
            },
        )
        .unwrap();
        restart(
            &directory,
            "run-1",
            &json!({"segment":2}),
            &json!({"id":"run-1","state":"running","progress":{"generation":50}}),
            RunMetrics {
                best_score: Some(0.8),
                evaluated_candidates: 6_400,
                elapsed_ms: 1_000,
            },
        )
        .unwrap();

        let rows = list(&directory, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "run-1");
        assert_eq!(rows[0].state, "running");
        assert_eq!(rows[0].title, "Keep me");
        assert_eq!(rows[0].notes, "Prior evidence remains attached");
        assert_eq!(rows[0].tags, vec!["tune"]);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                &load(&directory, "run-1").unwrap().request_json
            )
            .unwrap()["segment"],
            2
        );
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn startup_recovery_marks_only_running_rows_interrupted() {
        let directory =
            std::env::temp_dir().join(format!("metagene-run-recovery-{}", uuid::Uuid::new_v4()));
        let request = json!({"platform":"arena","format":"historic"});
        create(
            &directory,
            "running",
            "arena",
            "historic",
            &request,
            &json!({"id":"running","state":"running","progress":{"generation":5}}),
        )
        .unwrap();
        create(
            &directory,
            "complete",
            "arena",
            "historic",
            &request,
            &json!({"id":"complete","state":"completed"}),
        )
        .unwrap();
        finish(
            &directory,
            "complete",
            "completed",
            &json!({"id":"complete","state":"completed"}),
            Some(0.5),
            10,
            20,
        )
        .unwrap();

        assert_eq!(recover_interrupted(&directory).unwrap(), 1);
        assert_eq!(recover_interrupted(&directory).unwrap(), 0);
        let recovered: serde_json::Value =
            serde_json::from_str(&load(&directory, "running").unwrap().status_json).unwrap();
        assert_eq!(recovered["state"], "interrupted");
        assert_eq!(recovered["progress"]["generation"], 5);
        assert!(recovered["error"].as_str().unwrap().contains("Resume"));
        let completed: serde_json::Value =
            serde_json::from_str(&load(&directory, "complete").unwrap().status_json).unwrap();
        assert_eq!(completed["state"], "completed");
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn hybrid_snapshots_keep_request_and_status_on_one_boundary() {
        let directory = std::env::temp_dir().join(format!(
            "metagene-atomic-hybrid-snapshot-{}",
            uuid::Uuid::new_v4()
        ));
        create(
            &directory,
            "hybrid",
            "arena",
            "historic",
            &json!({"phase":"structural","cycle":1}),
            &json!({"id":"hybrid","state":"running","frontier":null}),
        )
        .unwrap();

        checkpoint_with_request(
            &directory,
            "hybrid",
            &json!({"phase":"validation","cycle":1}),
            &json!({"id":"hybrid","state":"running","frontier":[1,2]}),
            RunMetrics {
                best_score: Some(0.75),
                evaluated_candidates: 2_000,
                elapsed_ms: 400,
            },
        )
        .unwrap();
        let checkpoint = load(&directory, "hybrid").unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&checkpoint.request_json).unwrap()["phase"],
            "validation"
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&checkpoint.status_json).unwrap()["frontier"],
            json!([1, 2])
        );

        finish_with_request(
            &directory,
            "hybrid",
            "cancelled",
            &json!({"phase":"cycle_complete","cycle":1}),
            &json!({"id":"hybrid","state":"cancelled","frontier":[2]}),
            RunMetrics {
                best_score: Some(0.8),
                evaluated_candidates: 2_000,
                elapsed_ms: 500,
            },
        )
        .unwrap();
        let finished = load(&directory, "hybrid").unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&finished.request_json).unwrap()["phase"],
            "cycle_complete"
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&finished.status_json).unwrap()["state"],
            "cancelled"
        );
        assert_eq!(list(&directory, 10).unwrap()[0].state, "cancelled");
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn abrupt_process_exit_preserves_each_hybrid_phase_boundary() {
        const CHILD_DIRECTORY: &str = "METAGENE_RECOVERY_CHILD_DIRECTORY";
        const CHILD_PHASE: &str = "METAGENE_RECOVERY_CHILD_PHASE";
        if let (Ok(directory), Ok(phase)) =
            (std::env::var(CHILD_DIRECTORY), std::env::var(CHILD_PHASE))
        {
            let directory = PathBuf::from(directory);
            let request = json!({"run_kind":"hybrid","hybrid":{"checkpoint":{"phase":phase}}});
            let status = json!({
                "id":"hybrid",
                "state":"running",
                "result":{"frontier":[{"rank":1},{"rank":2}]}
            });
            create(
                &directory,
                "hybrid",
                "arena",
                "historic",
                &json!({"run_kind":"hybrid","hybrid":null}),
                &json!({"id":"hybrid","state":"running","result":null}),
            )
            .unwrap();
            checkpoint_with_request(
                &directory,
                "hybrid",
                &request,
                &status,
                RunMetrics {
                    best_score: Some(0.8),
                    evaluated_candidates: 2_000,
                    elapsed_ms: 500,
                },
            )
            .unwrap();
            // Model an operating-system/process loss: leave the row marked running and
            // terminate without executing application shutdown or recovery code.
            std::process::exit(73);
        }

        for phase in ["structural", "validation", "cycle_complete"] {
            let directory = std::env::temp_dir().join(format!(
                "metagene-live-hybrid-recovery-{phase}-{}",
                uuid::Uuid::new_v4()
            ));
            let exit = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "runs::tests::abrupt_process_exit_preserves_each_hybrid_phase_boundary",
                    "--nocapture",
                ])
                .env(CHILD_DIRECTORY, &directory)
                .env(CHILD_PHASE, phase)
                .status()
                .unwrap();
            assert_eq!(exit.code(), Some(73));

            assert_eq!(recover_interrupted(&directory).unwrap(), 1);
            assert_eq!(recover_interrupted(&directory).unwrap(), 0);
            let row = load(&directory, "hybrid").unwrap();
            let request: serde_json::Value = serde_json::from_str(&row.request_json).unwrap();
            let status: serde_json::Value = serde_json::from_str(&row.status_json).unwrap();
            assert_eq!(request["hybrid"]["checkpoint"]["phase"], phase);
            assert_eq!(status["state"], "interrupted");
            assert_eq!(status["result"]["frontier"], json!([{"rank":1},{"rank":2}]));
            std::fs::remove_dir_all(&directory).unwrap();
        }
    }

    #[test]
    fn persists_validation_evidence_by_evolution_run() {
        let directory = std::env::temp_dir().join(format!(
            "metagene-validation-store-{}",
            uuid::Uuid::new_v4()
        ));
        let request = json!({"candidate":{"name":"A"},"games":1000});
        let report = json!({"engine":"hexdek","games":1000,"crashes":0});
        let id = save_validation(&directory, Some("run-a"), Some(2), &request, &report).unwrap();
        save_validation(&directory, Some("run-b"), Some(1), &request, &report).unwrap();

        let rows = list_validations(&directory, Some("run-a"), 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].candidate_rank, Some(2));
        assert_eq!(rows[0].request_json, request.to_string());
        assert_eq!(rows[0].report_json, report.to_string());
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn persists_successive_halving_rounds_by_evolution_run() {
        let directory =
            std::env::temp_dir().join(format!("metagene-halving-store-{}", uuid::Uuid::new_v4()));
        let request = json!({"candidate_ranks":[1,2,3,4],"base_games":64});
        let report = json!({"winner_rank":2,"rounds":[{"round":1},{"round":2}]});
        let id = save_halving(&directory, Some("run-a"), &request, &report).unwrap();
        save_halving(&directory, Some("run-b"), &request, &report).unwrap();

        let rows = list_halvings(&directory, Some("run-a"), 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].request_json, request.to_string());
        assert_eq!(rows[0].report_json, report.to_string());
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn persists_best_of_three_and_policy_evolution_separately() {
        let directory =
            std::env::temp_dir().join(format!("metagene-sideboard-store-{}", uuid::Uuid::new_v4()));
        let request = json!({"candidate":{"name":"A"},"matches":32});
        let match_report = json!({"engine":"hexdek","completed_matches":32});
        let policy_report = json!({"engine":"hexdek","evaluated_policies":48});
        let match_id =
            save_best_of_three(&directory, Some("run-a"), Some(2), &request, &match_report)
                .unwrap();
        let policy_id =
            save_sideboard_evolution(&directory, Some("run-a"), Some(2), &request, &policy_report)
                .unwrap();

        let matches = list_best_of_three(&directory, Some("run-a"), 10).unwrap();
        let policies = list_sideboard_evolutions(&directory, Some("run-a"), 10).unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, match_id);
        assert_eq!(policies.len(), 1);
        assert_eq!(policies[0].id, policy_id);
        assert_eq!(policies[0].candidate_rank, Some(2));
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn migrates_metadata_and_exports_a_reproducibility_manifest() {
        let directory =
            std::env::temp_dir().join(format!("metagene-run-metadata-{}", uuid::Uuid::new_v4()));
        let request = json!({"platform":"arena","format":"historic","config":{"seed":42}});
        let status = json!({"id":"run-meta","state":"completed","result":{"frontier":[]}});
        create(
            &directory, "run-meta", "arena", "historic", &request, &status,
        )
        .unwrap();
        let updated = update_metadata(
            &directory,
            &RunMetadata {
                id: "run-meta".into(),
                title: " Copy trial ".into(),
                notes: "held-out comparison".into(),
                tags: vec!["Historic".into(), "copy".into(), "".into()],
            },
        )
        .unwrap();
        assert_eq!(updated.title, "Copy trial");
        assert_eq!(updated.tags, vec!["historic", "copy"]);
        let manifest: serde_json::Value =
            serde_json::from_str(&export_manifest(&directory, "run-meta").unwrap()).unwrap();
        assert_eq!(manifest["schema_version"], 1);
        assert_eq!(manifest["request"]["config"]["seed"], 42);
        assert_eq!(manifest["run"]["notes"], "held-out comparison");
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
