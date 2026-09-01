use metagene_core::{CardPool, Deck};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

const MAX_MATCHES: usize = 20_000;
const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum FeedbackError {
    #[error("feedback database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("feedback storage error: {0}")]
    Io(#[from] std::io::Error),
    #[error("feedback JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid feedback: {0}")]
    Invalid(String),
    #[error("match record not found: {0}")]
    NotFound(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeckRevision {
    pub id: String,
    pub platform: String,
    pub format: String,
    pub name: String,
    pub mtga_text: String,
    pub arena_fingerprint: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatchRecord {
    pub id: String,
    pub source: String,
    pub external_match_id: Option<String>,
    pub revision_id: Option<String>,
    pub deck_name: Option<String>,
    pub platform: Option<String>,
    pub format: Option<String>,
    pub occurred_at: Option<String>,
    pub result: String,
    pub opponent_archetype: String,
    pub play_draw: String,
    pub sideboarded: Option<bool>,
    pub notes: String,
    pub predicted_win_rate: Option<f64>,
    pub identity_status: String,
    pub imported_at: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MatchEdit {
    pub id: String,
    pub opponent_archetype: String,
    pub play_draw: String,
    pub sideboarded: Option<bool>,
    pub notes: String,
    pub predicted_win_rate: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ImportReport {
    pub imported: usize,
    pub exact_revision_matches: usize,
    pub quarantined_identity: usize,
    pub duplicates: usize,
    pub ignored_lines: usize,
    pub records: Vec<MatchRecord>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FeedbackSummary {
    pub matches: usize,
    pub exact_revision_matches: usize,
    pub wins: usize,
    pub losses: usize,
    pub draws: usize,
    pub observed_win_rate: f64,
    pub predicted_win_rate: Option<f64>,
    pub prediction_error: Option<f64>,
    pub by_archetype: Vec<ArchetypeFeedback>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ArchetypeFeedback {
    pub archetype: String,
    pub games: usize,
    pub observed_win_rate: f64,
    pub predicted_win_rate: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReweightSuggestion {
    pub opponent: String,
    pub current_weight: f64,
    pub suggested_weight: f64,
    pub matched_games: usize,
    pub reason: String,
}

#[derive(Debug, Deserialize)]
struct PortableHistory {
    schema_version: u32,
    #[serde(default)]
    matches: Vec<PortableMatch>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PortableMatch {
    #[serde(default)]
    external_match_id: Option<String>,
    #[serde(default)]
    revision_id: Option<String>,
    #[serde(default)]
    occurred_at: Option<String>,
    result: String,
    #[serde(default = "unknown_archetype")]
    opponent_archetype: String,
    #[serde(default = "unknown_play_draw")]
    play_draw: String,
    #[serde(default)]
    sideboarded: Option<bool>,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    predicted_win_rate: Option<f64>,
}

fn unknown_archetype() -> String {
    "Unknown".into()
}
fn unknown_play_draw() -> String {
    "unknown".into()
}

pub fn register_revision(
    data_dir: &Path,
    platform: &str,
    format: &str,
    deck: &Deck,
    pool: &CardPool,
    mtga_text: &str,
) -> Result<DeckRevision, FeedbackError> {
    let arena_fingerprint = arena_fingerprint_for_deck(deck, pool);
    let mut hasher = Sha256::new();
    hasher.update(platform.as_bytes());
    hasher.update([0]);
    hasher.update(format.as_bytes());
    hasher.update([0]);
    hasher.update(mtga_text.replace("\r\n", "\n").trim().as_bytes());
    let id = format!("sha256:{:x}", hasher.finalize());
    let connection = open(data_dir)?;
    connection.execute(
        "INSERT OR IGNORE INTO deck_revisions
         (id,platform,format,name,mtga_text,arena_fingerprint) VALUES (?1,?2,?3,?4,?5,?6)",
        params![
            id,
            platform,
            format,
            deck.name,
            mtga_text,
            arena_fingerprint
        ],
    )?;
    revision(&connection, &id)?.ok_or_else(|| FeedbackError::NotFound(id))
}

pub fn import(data_dir: &Path, source: &str, text: &str) -> Result<ImportReport, FeedbackError> {
    match source {
        "arena_log" => import_arena_log(data_dir, text),
        "json" => import_portable(data_dir, text),
        _ => Err(FeedbackError::Invalid(
            "source must be arena_log or json".into(),
        )),
    }
}

pub fn list(data_dir: &Path, limit: usize) -> Result<Vec<MatchRecord>, FeedbackError> {
    let connection = open(data_dir)?;
    let mut statement = connection.prepare(
        "SELECT m.id,m.source,m.external_match_id,m.revision_id,r.name,m.occurred_at,m.result,
                m.opponent_archetype,m.play_draw,m.sideboarded,m.notes,m.predicted_win_rate,
                m.identity_status,m.imported_at,r.platform,r.format
         FROM match_feedback m LEFT JOIN deck_revisions r ON r.id=m.revision_id
         ORDER BY m.imported_at DESC,m.rowid DESC LIMIT ?1",
    )?;
    let rows = statement.query_map([limit.clamp(1, 20_000) as i64], match_from_row)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn update(data_dir: &Path, edit: &MatchEdit) -> Result<MatchRecord, FeedbackError> {
    validate_result_fields("win", &edit.play_draw, edit.predicted_win_rate)?;
    let connection = open(data_dir)?;
    let changed = connection.execute(
        "UPDATE match_feedback SET opponent_archetype=?2,play_draw=?3,sideboarded=?4,
         notes=?5,predicted_win_rate=?6 WHERE id=?1",
        params![
            edit.id,
            edit.opponent_archetype.trim(),
            edit.play_draw,
            edit.sideboarded,
            edit.notes.trim(),
            edit.predicted_win_rate
        ],
    )?;
    if changed == 0 {
        return Err(FeedbackError::NotFound(edit.id.clone()));
    }
    load_match(&connection, &edit.id)?.ok_or_else(|| FeedbackError::NotFound(edit.id.clone()))
}

pub fn delete(data_dir: &Path, ids: &[String]) -> Result<usize, FeedbackError> {
    let mut connection = open(data_dir)?;
    let transaction = connection.transaction()?;
    let mut removed = 0usize;
    for id in ids {
        removed += transaction.execute("DELETE FROM match_feedback WHERE id=?1", [id])?;
    }
    transaction.commit()?;
    Ok(removed)
}

pub fn clear(data_dir: &Path) -> Result<usize, FeedbackError> {
    Ok(open(data_dir)?.execute("DELETE FROM match_feedback", [])?)
}

pub fn export(data_dir: &Path) -> Result<String, FeedbackError> {
    #[derive(Serialize)]
    struct Export {
        schema_version: u32,
        matches: Vec<MatchRecord>,
    }
    Ok(serde_json::to_string_pretty(&Export {
        schema_version: SCHEMA_VERSION,
        matches: list(data_dir, MAX_MATCHES)?,
    })?)
}

pub fn summary(
    data_dir: &Path,
    platform: Option<&str>,
    format: Option<&str>,
) -> Result<FeedbackSummary, FeedbackError> {
    let records = list(data_dir, MAX_MATCHES)?;
    let exact = records
        .iter()
        .filter(|record| {
            record.revision_id.is_some()
                && platform.is_none_or(|value| record.platform.as_deref() == Some(value))
                && format.is_none_or(|value| record.format.as_deref() == Some(value))
        })
        .collect::<Vec<_>>();
    let wins = exact.iter().filter(|record| record.result == "win").count();
    let losses = exact
        .iter()
        .filter(|record| record.result == "loss")
        .count();
    let draws = exact
        .iter()
        .filter(|record| record.result == "draw")
        .count();
    let decisive = wins + losses;
    let observed = wins as f64 / decisive.max(1) as f64;
    let predictions = exact
        .iter()
        .filter_map(|record| record.predicted_win_rate)
        .collect::<Vec<_>>();
    let predicted = (!predictions.is_empty())
        .then(|| predictions.iter().sum::<f64>() / predictions.len() as f64);
    let mut groups = HashMap::<String, Vec<&MatchRecord>>::new();
    for record in &exact {
        groups
            .entry(record.opponent_archetype.clone())
            .or_default()
            .push(record);
    }
    let mut by_archetype = groups
        .into_iter()
        .map(|(archetype, rows)| {
            let wins = rows.iter().filter(|row| row.result == "win").count();
            let losses = rows.iter().filter(|row| row.result == "loss").count();
            let predictions = rows
                .iter()
                .filter_map(|row| row.predicted_win_rate)
                .collect::<Vec<_>>();
            ArchetypeFeedback {
                archetype,
                games: rows.len(),
                observed_win_rate: wins as f64 / (wins + losses).max(1) as f64,
                predicted_win_rate: (!predictions.is_empty())
                    .then(|| predictions.iter().sum::<f64>() / predictions.len() as f64),
            }
        })
        .collect::<Vec<_>>();
    by_archetype.sort_by(|left, right| {
        right
            .games
            .cmp(&left.games)
            .then_with(|| left.archetype.cmp(&right.archetype))
    });
    Ok(FeedbackSummary {
        matches: exact.len(),
        exact_revision_matches: exact.len(),
        wins,
        losses,
        draws,
        observed_win_rate: observed,
        predicted_win_rate: predicted,
        prediction_error: predicted.map(|value| observed - value),
        by_archetype,
    })
}

pub fn suggest_reweights(
    data_dir: &Path,
    platform: &str,
    format: &str,
    opponents: &[String],
    current_weights: &[f64],
) -> Result<Vec<ReweightSuggestion>, FeedbackError> {
    if opponents.len() != current_weights.len() || opponents.is_empty() {
        return Err(FeedbackError::Invalid(
            "opponents and weights must have equal nonzero length".into(),
        ));
    }
    let summary = summary(data_dir, Some(platform), Some(format))?;
    let total_current = current_weights.iter().sum::<f64>().max(f64::EPSILON);
    let mut raw = Vec::new();
    for (opponent, current) in opponents.iter().zip(current_weights) {
        let evidence = summary
            .by_archetype
            .iter()
            .find(|row| row.archetype.eq_ignore_ascii_case(opponent));
        let games = evidence.map_or(0, |row| row.games);
        let weakness = evidence
            .filter(|row| row.games >= 3)
            .map_or(1.0, |row| 1.5 - row.observed_win_rate);
        raw.push((opponent, current / total_current * weakness, games));
    }
    let total = raw
        .iter()
        .map(|(_, value, _)| value)
        .sum::<f64>()
        .max(f64::EPSILON);
    Ok(raw.into_iter().zip(current_weights).map(|((opponent, value, games), current)| ReweightSuggestion {
        opponent: opponent.clone(), current_weight: *current / total_current,
        suggested_weight: value / total, matched_games: games,
        reason: if games >= 3 { "User-approved emphasis increases for empirically difficult matchups.".into() } else { "Fewer than three exact-revision matches; current share is retained before normalization.".into() },
    }).collect())
}

fn import_portable(data_dir: &Path, text: &str) -> Result<ImportReport, FeedbackError> {
    let history: PortableHistory = serde_json::from_str(text)?;
    if history.schema_version != SCHEMA_VERSION {
        return Err(FeedbackError::Invalid(
            "unsupported match-history schema".into(),
        ));
    }
    let mut connection = open(data_dir)?;
    let transaction = connection.transaction()?;
    let mut report = ImportReport {
        imported: 0,
        exact_revision_matches: 0,
        quarantined_identity: 0,
        duplicates: 0,
        ignored_lines: 0,
        records: Vec::new(),
    };
    for row in history.matches {
        validate_result_fields(&row.result, &row.play_draw, row.predicted_win_rate)?;
        let exact_revision = match row.revision_id {
            Some(id) if revision(&transaction, &id)?.is_some() => Some(id),
            _ => None,
        };
        let exact = exact_revision.is_some();
        let record = insert_match(
            &transaction,
            "json",
            row.external_match_id,
            exact_revision,
            row.occurred_at,
            row.result,
            row.opponent_archetype,
            row.play_draw,
            row.sideboarded,
            row.notes,
            row.predicted_win_rate,
        )?;
        match record {
            Some(record) => {
                report.imported += 1;
                report.exact_revision_matches += usize::from(exact);
                report.quarantined_identity += usize::from(!exact);
                report.records.push(record);
            }
            None => report.duplicates += 1,
        }
    }
    prune(&transaction)?;
    transaction.commit()?;
    Ok(report)
}

fn import_arena_log(data_dir: &Path, text: &str) -> Result<ImportReport, FeedbackError> {
    let connection = open(data_dir)?;
    let mut report = ImportReport {
        imported: 0,
        exact_revision_matches: 0,
        quarantined_identity: 0,
        duplicates: 0,
        ignored_lines: 0,
        records: Vec::new(),
    };
    let mut current_revision = None;
    for line in text.lines() {
        let Some(start) = line.find('{') else {
            report.ignored_lines += 1;
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&line[start..]) else {
            report.ignored_lines += 1;
            continue;
        };
        if let Some(ids) = find_numeric_array(
            &value,
            &[
                "mainDeck",
                "MainDeck",
                "main_deck",
                "deckCards",
                "DeckCards",
            ],
        ) {
            let fingerprint = arena_fingerprint(ids);
            current_revision = connection.query_row("SELECT id FROM deck_revisions WHERE arena_fingerprint=?1 ORDER BY created_at DESC LIMIT 1", [&fingerprint], |row| row.get(0)).optional()?;
        }
        let arena_event = line.contains("MatchGameRoomStateChangedEvent")
            || line.contains("DuelScene.GameStop")
            || line.contains("METAGENE_MATCH");
        if !arena_event {
            continue;
        }
        let result = find_string(&value, &["result", "Result", "matchResult"])
            .as_deref()
            .and_then(normalize_result)
            .or_else(|| result_from_telemetry(&value));
        let Some(result) = result else {
            continue;
        };
        let external = find_string(&value, &["matchId", "match_id", "gameRoomId"]);
        let opponent = find_string(&value, &["opponentArchetype", "opponent_archetype"])
            .unwrap_or_else(|| "Unknown".to_string());
        let play_draw = find_string(&value, &["playDraw", "play_draw"])
            .as_deref()
            .and_then(normalize_play_draw)
            .or_else(|| play_draw_from_telemetry(&value))
            .unwrap_or("unknown")
            .to_string();
        let record = insert_match(
            &connection,
            "arena_log",
            external,
            current_revision.clone(),
            None,
            result.into(),
            opponent,
            play_draw,
            None,
            String::new(),
            None,
        )?;
        match record {
            Some(record) => {
                report.imported += 1;
                report.exact_revision_matches += usize::from(current_revision.is_some());
                report.quarantined_identity += usize::from(current_revision.is_none());
                report.records.push(record);
            }
            None => report.duplicates += 1,
        }
    }
    prune(&connection)?;
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
fn insert_match(
    connection: &Connection,
    source: &str,
    external: Option<String>,
    revision_id: Option<String>,
    occurred_at: Option<String>,
    result: String,
    opponent: String,
    play_draw: String,
    sideboarded: Option<bool>,
    notes: String,
    predicted: Option<f64>,
) -> Result<Option<MatchRecord>, FeedbackError> {
    validate_result_fields(&result, &play_draw, predicted)?;
    let id = Uuid::new_v4().to_string();
    let status = if revision_id.is_some() {
        "exact"
    } else {
        "quarantined"
    };
    let changed = connection.execute(
        "INSERT OR IGNORE INTO match_feedback
         (id,source,external_match_id,revision_id,occurred_at,result,opponent_archetype,play_draw,sideboarded,notes,predicted_win_rate,identity_status)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![id, source, external, revision_id, occurred_at, result, opponent.trim(), play_draw, sideboarded, notes.trim(), predicted, status],
    )?;
    if changed == 0 {
        return Ok(None);
    }
    load_match(connection, &id)
}

fn validate_result_fields(
    result: &str,
    play_draw: &str,
    predicted: Option<f64>,
) -> Result<(), FeedbackError> {
    if !matches!(result, "win" | "loss" | "draw") {
        return Err(FeedbackError::Invalid(
            "result must be win, loss, or draw".into(),
        ));
    }
    if !matches!(play_draw, "play" | "draw" | "unknown") {
        return Err(FeedbackError::Invalid(
            "play_draw must be play, draw, or unknown".into(),
        ));
    }
    if predicted.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
        return Err(FeedbackError::Invalid(
            "predicted win rate must be 0-1".into(),
        ));
    }
    Ok(())
}

fn arena_fingerprint_for_deck(deck: &Deck, pool: &CardPool) -> Option<String> {
    let by_id = pool
        .cards
        .iter()
        .map(|card| (&card.id, card.arena_id))
        .collect::<HashMap<_, _>>();
    let mut ids = Vec::new();
    for entry in &deck.mainboard {
        let arena_id = by_id.get(&entry.card_id).copied().flatten()?;
        ids.extend(std::iter::repeat_n(arena_id, entry.quantity as usize));
    }
    Some(arena_fingerprint(ids))
}

fn arena_fingerprint(ids: impl IntoIterator<Item = u64>) -> String {
    let mut ids = ids.into_iter().collect::<Vec<_>>();
    ids.sort_unstable();
    let mut hasher = Sha256::new();
    for id in ids {
        hasher.update(id.to_le_bytes());
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn find_numeric_array(value: &Value, keys: &[&str]) -> Option<Vec<u64>> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(Value::Array(values)) = map.get(*key) {
                    let ids = values.iter().filter_map(Value::as_u64).collect::<Vec<_>>();
                    if !ids.is_empty() {
                        return Some(ids);
                    }
                }
            }
            map.values()
                .find_map(|value| find_numeric_array(value, keys))
        }
        Value::Array(values) => values
            .iter()
            .find_map(|value| find_numeric_array(value, keys)),
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|nested| find_numeric_array(&nested, keys)),
        _ => None,
    }
}

fn find_string(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(value) = map.get(*key).and_then(Value::as_str) {
                    return Some(value.to_string());
                }
            }
            map.values().find_map(|value| find_string(value, keys))
        }
        Value::Array(values) => values.iter().find_map(|value| find_string(value, keys)),
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|nested| find_string(&nested, keys)),
        _ => None,
    }
}

fn find_u64(value: &Value, keys: &[&str]) -> Option<u64> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(value) = map.get(*key).and_then(Value::as_u64) {
                    return Some(value);
                }
            }
            map.values().find_map(|value| find_u64(value, keys))
        }
        Value::Array(values) => values.iter().find_map(|value| find_u64(value, keys)),
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|nested| find_u64(&nested, keys)),
        _ => None,
    }
}

fn result_from_telemetry(value: &Value) -> Option<&'static str> {
    let player_team = find_u64(value, &["teamId"])?;
    let winning_team = find_u64(value, &["winningTeamId"])?;
    Some(if winning_team == 0 {
        "draw"
    } else if player_team == winning_team {
        "win"
    } else {
        "loss"
    })
}

fn play_draw_from_telemetry(value: &Value) -> Option<&'static str> {
    let player_team = find_u64(value, &["teamId"])?;
    let starting_team = find_u64(value, &["startingTeamId"])?;
    Some(if player_team == starting_team {
        "play"
    } else {
        "draw"
    })
}

fn normalize_result(value: &str) -> Option<&'static str> {
    match value.to_ascii_lowercase().as_str() {
        "win" | "won" | "victory" => Some("win"),
        "loss" | "lost" | "defeat" => Some("loss"),
        "draw" | "tie" => Some("draw"),
        _ => None,
    }
}
fn normalize_play_draw(value: &str) -> Option<&'static str> {
    match value.to_ascii_lowercase().as_str() {
        "play" | "on_play" => Some("play"),
        "draw" | "on_draw" => Some("draw"),
        "unknown" => Some("unknown"),
        _ => None,
    }
}

fn revision(connection: &Connection, id: &str) -> Result<Option<DeckRevision>, rusqlite::Error> {
    connection.query_row("SELECT id,platform,format,name,mtga_text,arena_fingerprint,created_at FROM deck_revisions WHERE id=?1", [id], |row| Ok(DeckRevision { id: row.get(0)?, platform: row.get(1)?, format: row.get(2)?, name: row.get(3)?, mtga_text: row.get(4)?, arena_fingerprint: row.get(5)?, created_at: row.get(6)? })).optional()
}
fn load_match(connection: &Connection, id: &str) -> Result<Option<MatchRecord>, FeedbackError> {
    Ok(connection.query_row("SELECT m.id,m.source,m.external_match_id,m.revision_id,r.name,m.occurred_at,m.result,m.opponent_archetype,m.play_draw,m.sideboarded,m.notes,m.predicted_win_rate,m.identity_status,m.imported_at,r.platform,r.format FROM match_feedback m LEFT JOIN deck_revisions r ON r.id=m.revision_id WHERE m.id=?1", [id], match_from_row).optional()?)
}
fn match_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MatchRecord> {
    Ok(MatchRecord {
        id: row.get(0)?,
        source: row.get(1)?,
        external_match_id: row.get(2)?,
        revision_id: row.get(3)?,
        deck_name: row.get(4)?,
        occurred_at: row.get(5)?,
        result: row.get(6)?,
        opponent_archetype: row.get(7)?,
        play_draw: row.get(8)?,
        sideboarded: row.get(9)?,
        notes: row.get(10)?,
        predicted_win_rate: row.get(11)?,
        identity_status: row.get(12)?,
        imported_at: row.get(13)?,
        platform: row.get(14)?,
        format: row.get(15)?,
    })
}
fn prune(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute("DELETE FROM match_feedback WHERE rowid NOT IN (SELECT rowid FROM match_feedback ORDER BY imported_at DESC,rowid DESC LIMIT ?1)", [MAX_MATCHES as i64])?;
    Ok(())
}
fn database_path(data_dir: &Path) -> PathBuf {
    data_dir.join("feedback.sqlite")
}
fn open(data_dir: &Path) -> Result<Connection, FeedbackError> {
    std::fs::create_dir_all(data_dir)?;
    let connection = Connection::open(database_path(data_dir))?;
    connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
      CREATE TABLE IF NOT EXISTS deck_revisions(id TEXT PRIMARY KEY,platform TEXT NOT NULL,format TEXT NOT NULL,name TEXT NOT NULL,mtga_text TEXT NOT NULL,arena_fingerprint TEXT,created_at TEXT NOT NULL DEFAULT(datetime('now')));
      CREATE INDEX IF NOT EXISTS revision_arena ON deck_revisions(arena_fingerprint);
      CREATE TABLE IF NOT EXISTS match_feedback(id TEXT PRIMARY KEY,source TEXT NOT NULL,external_match_id TEXT,revision_id TEXT REFERENCES deck_revisions(id),occurred_at TEXT,result TEXT NOT NULL,opponent_archetype TEXT NOT NULL,play_draw TEXT NOT NULL,sideboarded INTEGER,notes TEXT NOT NULL,predicted_win_rate REAL,identity_status TEXT NOT NULL,imported_at TEXT NOT NULL DEFAULT(datetime('now')),UNIQUE(source,external_match_id));")?;
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use metagene_core::{Card, CardFeatures, CardId, DeckEntry};
    use std::collections::BTreeMap;

    fn fixture() -> (CardPool, Deck) {
        let card = Card {
            id: CardId("forest".into()),
            arena_id: Some(123),
            platforms: vec!["arena".into()],
            name: "Forest".into(),
            set_code: "ana".into(),
            collector_number: "1".into(),
            type_line: "Basic Land".into(),
            oracle_text: String::new(),
            colors: Vec::new(),
            color_identity: Vec::new(),
            image_uri: None,
            legalities: BTreeMap::new(),
            features: CardFeatures {
                is_land: true,
                unlimited_copies: true,
                ..CardFeatures::default()
            },
        };
        (
            CardPool { cards: vec![card] },
            Deck {
                name: "Exact export".into(),
                command_zone: Vec::new(),
                mainboard: vec![DeckEntry {
                    card_id: CardId("forest".into()),
                    quantity: 60,
                }],
                sideboard: Vec::new(),
            },
        )
    }

    #[test]
    fn arena_log_matches_only_an_exact_registered_arena_multiset() {
        let root = std::env::temp_dir().join(format!("metagene-feedback-{}", Uuid::new_v4()));
        let (pool, deck) = fixture();
        let revision =
            register_revision(&root, "arena", "historic", &deck, &pool, "Deck\n60 Forest").unwrap();
        let ids = std::iter::repeat_n("123", 60).collect::<Vec<_>>().join(",");
        let log = format!(
            "DeckEvent {{\"mainDeck\":[{ids}]}}\nMatchGameRoomStateChangedEvent {{\"matchId\":\"m1\",\"result\":\"Win\",\"opponentArchetype\":\"Goblins\",\"playDraw\":\"play\"}}"
        );
        let report = import(&root, "arena_log", &log).unwrap();
        assert_eq!(report.exact_revision_matches, 1);
        assert_eq!(
            report.records[0].revision_id.as_deref(),
            Some(revision.id.as_str())
        );
        let ambiguous = import(
            &root,
            "arena_log",
            "MatchGameRoomStateChangedEvent {\"matchId\":\"m2\",\"result\":\"Loss\"}",
        )
        .unwrap();
        assert_eq!(ambiguous.quarantined_identity, 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn arena_duel_scene_telemetry_parses_embedded_json_result_and_play_draw() {
        let root = std::env::temp_dir().join(format!("metagene-feedback-{}", Uuid::new_v4()));
        let (pool, deck) = fixture();
        register_revision(&root, "arena", "historic", &deck, &pool, "Deck\n60 Forest").unwrap();
        let ids = std::iter::repeat_n("123", 60).collect::<Vec<_>>().join(",");
        let inner = serde_json::json!({"payloadObject":{"teamId":1,"startingTeamId":2,"winningTeamId":1,"matchId":"real-shape"}}).to_string();
        let log = format!(
            "Deck.Submit {{\"DeckCards\":[{ids}]}}\nDuelScene.GameStop {}",
            serde_json::json!({"request":inner})
        );
        let report = import(&root, "arena_log", &log).unwrap();
        assert_eq!(report.exact_revision_matches, 1);
        assert_eq!(report.records[0].result, "win");
        assert_eq!(report.records[0].play_draw, "draw");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reweighting_requires_three_exact_matches_and_is_normalized() {
        let root = std::env::temp_dir().join(format!("metagene-feedback-{}", Uuid::new_v4()));
        let (pool, deck) = fixture();
        let revision =
            register_revision(&root, "arena", "historic", &deck, &pool, "Deck\n60 Forest").unwrap();
        let matches = (0..3).map(|index| serde_json::json!({"external_match_id":format!("m{index}"),"revision_id":revision.id,"result":"loss","opponent_archetype":"Goblins"})).collect::<Vec<_>>();
        import(
            &root,
            "json",
            &serde_json::json!({"schema_version":1,"matches":matches}).to_string(),
        )
        .unwrap();
        let suggestions = suggest_reweights(
            &root,
            "arena",
            "historic",
            &["Goblins".into(), "Control".into()],
            &[0.5, 0.5],
        )
        .unwrap();
        assert!(suggestions[0].suggested_weight > suggestions[1].suggested_weight);
        assert!(
            (suggestions
                .iter()
                .map(|row| row.suggested_weight)
                .sum::<f64>()
                - 1.0)
                .abs()
                < 1e-9
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exported_history_round_trips_exact_and_quarantined_records() {
        let root = std::env::temp_dir().join(format!("metagene-feedback-{}", Uuid::new_v4()));
        let (pool, deck) = fixture();
        let revision =
            register_revision(&root, "arena", "historic", &deck, &pool, "Deck\n60 Forest").unwrap();
        let history = serde_json::json!({"schema_version":1,"matches":[
            {"external_match_id":"exact","revision_id":revision.id,"result":"win"},
            {"external_match_id":"quarantine","result":"loss"}
        ]});
        let first = import(&root, "json", &history.to_string()).unwrap();
        assert_eq!(
            (first.exact_revision_matches, first.quarantined_identity),
            (1, 1)
        );
        let exported = export(&root).unwrap();
        clear(&root).unwrap();
        let second = import(&root, "json", &exported).unwrap();
        assert_eq!(
            (second.exact_revision_matches, second.quarantined_identity),
            (1, 1)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn feedback_summary_is_scoped_to_platform_and_format() {
        let root = std::env::temp_dir().join(format!("metagene-feedback-{}", Uuid::new_v4()));
        let (pool, deck) = fixture();
        let historic =
            register_revision(&root, "arena", "historic", &deck, &pool, "Deck\n60 Forest").unwrap();
        let standard =
            register_revision(&root, "arena", "standard", &deck, &pool, "Deck\n60 Forest").unwrap();
        let rows = serde_json::json!({"schema_version":1,"matches":[
            {"external_match_id":"h","revision_id":historic.id,"result":"loss"},
            {"external_match_id":"s","revision_id":standard.id,"result":"win"}
        ]});
        import(&root, "json", &rows.to_string()).unwrap();
        let report = summary(&root, Some("arena"), Some("historic")).unwrap();
        assert_eq!((report.matches, report.wins, report.losses), (1, 0, 1));
        std::fs::remove_dir_all(root).unwrap();
    }
}
