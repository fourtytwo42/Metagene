use metagene_core::Deck;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

const MAX_GAUNTLETS: usize = 200;

#[derive(Debug, Error)]
pub enum GauntletStoreError {
    #[error("failed to access gauntlet storage: {0}")]
    Io(#[from] std::io::Error),
    #[error("gauntlet database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("failed to serialize gauntlet: {0}")]
    Json(#[from] serde_json::Error),
    #[error("saved gauntlet not found: {0}")]
    NotFound(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GauntletSummary {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub format: String,
    pub commander: bool,
    pub revision: usize,
    pub deck_count: usize,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedGauntlet {
    #[serde(flatten)]
    pub summary: GauntletSummary,
    pub decks: Vec<Deck>,
    #[serde(default)]
    pub weights: Vec<f64>,
    #[serde(default)]
    pub metadata: GauntletMetadata,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GauntletMetadata {
    pub source: String,
    pub observed_at: String,
    pub notes: String,
    pub source_url: Option<String>,
    pub retrieved_at_unix: Option<u64>,
    pub event_start_unix: Option<i64>,
    pub event_end_unix: Option<i64>,
    pub sample_size: Option<usize>,
    pub transformation_notes: Vec<String>,
    pub field_revision_id: Option<String>,
    pub source_tier: Option<String>,
    pub competitive_claims_allowed: Option<bool>,
    pub archetype_labels: Vec<String>,
    pub quarantined_summary: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetagameBundle {
    pub schema_version: u32,
    pub name: String,
    pub platform: String,
    pub format: String,
    pub commander: bool,
    #[serde(default)]
    pub metadata: GauntletMetadata,
    pub opponents: Vec<BundleOpponent>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BundleOpponent {
    pub name: String,
    pub weight: f64,
    pub decklist: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct GauntletWeightChange {
    pub name: String,
    pub from_weight: f64,
    pub to_weight: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct GauntletComparison {
    pub from: GauntletSummary,
    pub to: GauntletSummary,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub unchanged: usize,
    pub weight_changes: Vec<GauntletWeightChange>,
}

// The storage boundary keeps the scoped identity and portable field payload explicit.
#[allow(clippy::too_many_arguments)]
pub fn save(
    data_dir: &Path,
    name: &str,
    platform: &str,
    format: &str,
    commander: bool,
    decks: &[Deck],
    weights: &[f64],
    metadata: &GauntletMetadata,
) -> Result<SavedGauntlet, GauntletStoreError> {
    let mut connection = open(data_dir)?;
    let transaction = connection.transaction()?;
    let name_key = name.to_lowercase();
    let revision = transaction
        .query_row(
            "SELECT MAX(revision) FROM gauntlets
             WHERE name_key=?1 AND platform=?2 AND format=?3 AND commander=?4",
            params![name_key, platform, format, commander],
            |row| row.get::<_, Option<i64>>(0),
        )?
        .unwrap_or(0)
        .saturating_add(1);
    let id = Uuid::new_v4().to_string();
    let decks_json = serde_json::to_string(decks)?;
    let weights_json = serde_json::to_string(weights)?;
    let metadata_json = serde_json::to_string(metadata)?;
    transaction.execute(
        "INSERT INTO gauntlets
         (id, name, name_key, platform, format, commander, revision, deck_count, decks_json,
          weights_json, metadata_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            id,
            name,
            name_key,
            platform,
            format,
            commander,
            revision,
            decks.len().min(i64::MAX as usize) as i64,
            decks_json,
            weights_json,
            metadata_json,
        ],
    )?;
    transaction.execute(
        "DELETE FROM gauntlets WHERE id NOT IN (
           SELECT id FROM gauntlets ORDER BY created_at DESC, rowid DESC LIMIT ?1
         )",
        [MAX_GAUNTLETS as i64],
    )?;
    transaction.commit()?;
    load(data_dir, &id)
}

pub fn list(
    data_dir: &Path,
    platform: &str,
    format: &str,
    commander: bool,
    limit: usize,
) -> Result<Vec<GauntletSummary>, GauntletStoreError> {
    let connection = open(data_dir)?;
    let mut statement = connection.prepare(
        "SELECT id, name, platform, format, commander, revision, deck_count, created_at
         FROM gauntlets WHERE platform=?1 AND format=?2 AND commander=?3
         ORDER BY created_at DESC, rowid DESC LIMIT ?4",
    )?;
    let rows = statement.query_map(
        params![
            platform,
            format,
            commander,
            limit.clamp(1, MAX_GAUNTLETS) as i64
        ],
        summary_from_row,
    )?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn load(data_dir: &Path, id: &str) -> Result<SavedGauntlet, GauntletStoreError> {
    let connection = open(data_dir)?;
    let row = connection
        .query_row(
            "SELECT id, name, platform, format, commander, revision, deck_count, created_at,
                    decks_json, weights_json, metadata_json
             FROM gauntlets WHERE id=?1",
            [id],
            |row| {
                Ok((
                    summary_from_row(row)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| GauntletStoreError::NotFound(id.into()))?;
    Ok(SavedGauntlet {
        summary: row.0,
        decks: serde_json::from_str(&row.1)?,
        weights: serde_json::from_str(&row.2)?,
        metadata: serde_json::from_str(&row.3)?,
    })
}

pub fn delete(data_dir: &Path, id: &str) -> Result<(), GauntletStoreError> {
    let connection = open(data_dir)?;
    if connection.execute("DELETE FROM gauntlets WHERE id=?1", [id])? == 0 {
        return Err(GauntletStoreError::NotFound(id.into()));
    }
    Ok(())
}

pub fn compare(
    data_dir: &Path,
    from_id: &str,
    to_id: &str,
) -> Result<GauntletComparison, GauntletStoreError> {
    let from = load(data_dir, from_id)?;
    let to = load(data_dir, to_id)?;
    let from_entries = comparable_entries(&from);
    let to_entries = comparable_entries(&to);
    let mut added = to_entries
        .iter()
        .filter(|(fingerprint, _)| !from_entries.contains_key(*fingerprint))
        .map(|(_, (name, _))| name.clone())
        .collect::<Vec<_>>();
    let mut removed = from_entries
        .iter()
        .filter(|(fingerprint, _)| !to_entries.contains_key(*fingerprint))
        .map(|(_, (name, _))| name.clone())
        .collect::<Vec<_>>();
    let mut weight_changes = from_entries
        .iter()
        .filter_map(|(fingerprint, (name, from_weight))| {
            let (_, to_weight) = to_entries.get(fingerprint)?;
            ((from_weight - to_weight).abs() > 1e-9).then(|| GauntletWeightChange {
                name: name.clone(),
                from_weight: *from_weight,
                to_weight: *to_weight,
            })
        })
        .collect::<Vec<_>>();
    added.sort();
    removed.sort();
    weight_changes.sort_by(|left, right| left.name.cmp(&right.name));
    let unchanged = from_entries
        .keys()
        .filter(|fingerprint| to_entries.contains_key(*fingerprint))
        .count();
    Ok(GauntletComparison {
        from: from.summary,
        to: to.summary,
        added,
        removed,
        unchanged,
        weight_changes,
    })
}

fn comparable_entries(saved: &SavedGauntlet) -> std::collections::HashMap<String, (String, f64)> {
    saved
        .decks
        .iter()
        .enumerate()
        .map(|(index, deck)| {
            let mut canonical = deck.clone();
            canonical.name.clear();
            canonical
                .command_zone
                .sort_by(|a, b| a.card_id.0.cmp(&b.card_id.0));
            canonical
                .mainboard
                .sort_by(|a, b| a.card_id.0.cmp(&b.card_id.0));
            canonical
                .sideboard
                .sort_by(|a, b| a.card_id.0.cmp(&b.card_id.0));
            let key = serde_json::to_string(&canonical).unwrap_or_default();
            (
                key,
                (
                    deck.name.clone(),
                    saved.weights.get(index).copied().unwrap_or(0.0),
                ),
            )
        })
        .collect()
}

fn summary_from_row(row: &rusqlite::Row<'_>) -> Result<GauntletSummary, rusqlite::Error> {
    Ok(GauntletSummary {
        id: row.get(0)?,
        name: row.get(1)?,
        platform: row.get(2)?,
        format: row.get(3)?,
        commander: row.get(4)?,
        revision: row.get::<_, i64>(5)?.max(0) as usize,
        deck_count: row.get::<_, i64>(6)?.max(0) as usize,
        created_at: row.get(7)?,
    })
}

fn open(data_dir: &Path) -> Result<Connection, GauntletStoreError> {
    fs::create_dir_all(data_dir)?;
    let path: PathBuf = data_dir.join("gauntlets.sqlite");
    let connection = Connection::open(path)?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS gauntlets (
           id TEXT PRIMARY KEY,
           name TEXT NOT NULL,
           name_key TEXT NOT NULL,
           platform TEXT NOT NULL,
           format TEXT NOT NULL,
           commander INTEGER NOT NULL,
           revision INTEGER NOT NULL,
           deck_count INTEGER NOT NULL,
           decks_json TEXT NOT NULL,
           weights_json TEXT NOT NULL DEFAULT '[]',
           metadata_json TEXT NOT NULL DEFAULT '{}',
           created_at TEXT NOT NULL DEFAULT (datetime('now')),
           UNIQUE(name_key, platform, format, commander, revision)
         );
         CREATE INDEX IF NOT EXISTS gauntlets_scope
           ON gauntlets(platform, format, commander, created_at DESC);",
    )?;
    ensure_column(
        &connection,
        "weights_json",
        "ALTER TABLE gauntlets ADD COLUMN weights_json TEXT NOT NULL DEFAULT '[]'",
    )?;
    ensure_column(
        &connection,
        "metadata_json",
        "ALTER TABLE gauntlets ADD COLUMN metadata_json TEXT NOT NULL DEFAULT '{}'",
    )?;
    Ok(connection)
}

fn ensure_column(
    connection: &Connection,
    column: &str,
    migration: &str,
) -> Result<(), rusqlite::Error> {
    let mut statement = connection.prepare("PRAGMA table_info(gauntlets)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !columns.iter().any(|existing| existing == column) {
        connection.execute_batch(migration)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use metagene_core::{CardId, DeckEntry};

    fn deck(name: &str) -> Deck {
        Deck {
            name: name.into(),
            command_zone: Vec::new(),
            mainboard: vec![DeckEntry {
                card_id: CardId(format!("{name}-card")),
                quantity: 4,
            }],
            sideboard: Vec::new(),
        }
    }

    #[test]
    fn versions_scopes_loads_and_deletes_gauntlets() {
        let directory = std::env::temp_dir().join(format!("metagene-gauntlets-{}", Uuid::new_v4()));
        let first = save(
            &directory,
            "Historic Field",
            "arena",
            "historic",
            false,
            &[deck("Aggro")],
            &[1.0],
            &GauntletMetadata::default(),
        )
        .unwrap();
        let second = save(
            &directory,
            "historic field",
            "arena",
            "historic",
            false,
            &[deck("Aggro"), deck("Control")],
            &[0.6, 0.4],
            &GauntletMetadata {
                source: "Tournament results".into(),
                observed_at: "2026-08-29".into(),
                notes: "Test field".into(),
                ..Default::default()
            },
        )
        .unwrap();
        save(
            &directory,
            "Commander Pod",
            "paper",
            "commander",
            true,
            &[deck("Pod")],
            &[1.0],
            &GauntletMetadata::default(),
        )
        .unwrap();

        assert_eq!(first.summary.revision, 1);
        assert_eq!(second.summary.revision, 2);
        let historic = list(&directory, "arena", "historic", false, 10).unwrap();
        assert_eq!(historic.len(), 2);
        assert_eq!(historic[0].revision, 2);
        let loaded = load(&directory, &second.summary.id).unwrap();
        assert_eq!(loaded.decks.len(), 2);
        assert_eq!(loaded.weights, vec![0.6, 0.4]);
        assert_eq!(loaded.metadata.source, "Tournament results");
        let comparison = compare(&directory, &first.summary.id, &second.summary.id).unwrap();
        assert_eq!(comparison.unchanged, 1);
        assert_eq!(comparison.added, vec!["Control"]);
        assert!(comparison.removed.is_empty());
        assert_eq!(comparison.weight_changes.len(), 1);
        delete(&directory, &first.summary.id).unwrap();
        assert!(matches!(
            load(&directory, &first.summary.id),
            Err(GauntletStoreError::NotFound(_))
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn retention_keeps_only_the_latest_two_hundred_revisions() {
        let directory =
            std::env::temp_dir().join(format!("metagene-gauntlet-cap-{}", Uuid::new_v4()));
        let mut first_id = String::new();
        for revision in 0..=MAX_GAUNTLETS {
            let saved = save(
                &directory,
                &format!("Field {revision}"),
                "arena",
                "historic",
                false,
                &[deck("Opponent")],
                &[1.0],
                &GauntletMetadata::default(),
            )
            .unwrap();
            if revision == 0 {
                first_id = saved.summary.id;
            }
        }
        assert_eq!(
            list(&directory, "arena", "historic", false, MAX_GAUNTLETS + 10)
                .unwrap()
                .len(),
            MAX_GAUNTLETS
        );
        assert!(matches!(
            load(&directory, &first_id),
            Err(GauntletStoreError::NotFound(_))
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn portable_bundle_round_trips_mtga_text_and_metadata() {
        let bundle = MetagameBundle {
            schema_version: 1,
            name: "Historic field".into(),
            platform: "arena".into(),
            format: "historic".into(),
            commander: false,
            metadata: GauntletMetadata {
                source: "Example event".into(),
                observed_at: "2026-08-29".into(),
                notes: "Top eight".into(),
                ..Default::default()
            },
            opponents: vec![BundleOpponent {
                name: "Control".into(),
                weight: 0.4,
                decklist: "Deck\n60 Island (TST) 1\n".into(),
            }],
        };
        let encoded = serde_json::to_string_pretty(&bundle).unwrap();
        let decoded: MetagameBundle = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded.schema_version, 1);
        assert_eq!(decoded.metadata.source, "Example event");
        assert_eq!(decoded.opponents[0].weight, 0.4);
        assert!(decoded.opponents[0].decklist.starts_with("Deck\n"));
    }

    #[test]
    fn migrates_legacy_rows_to_equal_weight_metadata_defaults() {
        let directory =
            std::env::temp_dir().join(format!("metagene-gauntlet-migration-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let connection = Connection::open(directory.join("gauntlets.sqlite")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE gauntlets (
                   id TEXT PRIMARY KEY, name TEXT NOT NULL, name_key TEXT NOT NULL,
                   platform TEXT NOT NULL, format TEXT NOT NULL, commander INTEGER NOT NULL,
                   revision INTEGER NOT NULL, deck_count INTEGER NOT NULL, decks_json TEXT NOT NULL,
                   created_at TEXT NOT NULL DEFAULT (datetime('now'))
                 );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO gauntlets
                 (id, name, name_key, platform, format, commander, revision, deck_count, decks_json)
                 VALUES ('legacy', 'Legacy', 'legacy', 'arena', 'historic', 0, 1, 1, ?1)",
                [serde_json::to_string(&[deck("Old")]).unwrap()],
            )
            .unwrap();
        drop(connection);

        let loaded = load(&directory, "legacy").unwrap();
        assert_eq!(loaded.decks.len(), 1);
        assert!(loaded.weights.is_empty());
        assert!(loaded.metadata.source.is_empty());
        fs::remove_dir_all(directory).unwrap();
    }
}
