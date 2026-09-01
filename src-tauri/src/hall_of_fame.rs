use metagene_core::{CardId, CardPool, Deck, RankedDeck};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

const MAX_SCOPE_ENTRIES: usize = 64;
const MAX_TOTAL_ENTRIES: usize = 512;

#[derive(Debug, Error)]
pub enum HallOfFameError {
    #[error("failed to access Hall of Fame storage: {0}")]
    Io(#[from] std::io::Error),
    #[error("Hall of Fame database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("failed to serialize Hall of Fame candidate: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Hall of Fame entry not found: {0}")]
    NotFound(String),
    #[error("Hall of Fame evidence score must be finite")]
    InvalidScore,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HallOfFameEntry {
    pub id: String,
    pub platform: String,
    pub format: String,
    pub commander: bool,
    pub candidate: RankedDeck,
    pub evidence_score: f64,
    pub novelty: f64,
    pub source_run_id: Option<String>,
    pub source_cycle: Option<usize>,
    pub promotion_count: usize,
    pub created_at: String,
    pub last_seen_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct HallOfFameStatus {
    pub entries: Vec<HallOfFameEntry>,
    pub active_diverse_ids: Vec<String>,
    pub maximum_entries_for_scope: usize,
}

pub struct HallOfFamePromotion<'a> {
    pub platform: &'a str,
    pub format: &'a str,
    pub commander: bool,
    pub evidence_score: f64,
    pub source_run_id: Option<&'a str>,
    pub source_cycle: Option<usize>,
}

pub fn promote(
    data_dir: &Path,
    candidate: &RankedDeck,
    promotion: HallOfFamePromotion<'_>,
) -> Result<HallOfFameEntry, HallOfFameError> {
    let HallOfFamePromotion {
        platform,
        format,
        commander,
        evidence_score,
        source_run_id,
        source_cycle,
    } = promotion;
    if !evidence_score.is_finite() {
        return Err(HallOfFameError::InvalidScore);
    }
    let evidence_score = evidence_score.clamp(-1.0, 1.0);
    let deck_hash = deck_fingerprint(&candidate.deck)?;
    let candidate_json = serde_json::to_string(candidate)?;
    let mut connection = open(data_dir)?;
    let transaction = connection.transaction()?;
    let existing = transaction
        .query_row(
            "SELECT id, evidence_score FROM hall_of_fame
             WHERE platform=?1 AND format=?2 AND commander=?3 AND deck_hash=?4",
            params![platform, format, commander, deck_hash],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)),
        )
        .optional()?;
    let id = if let Some((id, prior_score)) = existing {
        transaction.execute(
            "UPDATE hall_of_fame SET
               candidate_json=CASE WHEN ?2 >= evidence_score THEN ?3 ELSE candidate_json END,
               evidence_score=MAX(evidence_score, ?2), source_run_id=?4, source_cycle=?5,
               promotion_count=promotion_count+1, last_seen_at=datetime('now')
             WHERE id=?1",
            params![
                id,
                evidence_score,
                candidate_json,
                source_run_id,
                source_cycle.map(|cycle| cycle.min(i64::MAX as usize) as i64)
            ],
        )?;
        debug_assert!(evidence_score.max(prior_score).is_finite());
        id
    } else {
        let id = Uuid::new_v4().to_string();
        transaction.execute(
            "INSERT INTO hall_of_fame
             (id, platform, format, commander, deck_hash, candidate_json, evidence_score,
              source_run_id, source_cycle)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                id,
                platform,
                format,
                commander,
                deck_hash,
                candidate_json,
                evidence_score,
                source_run_id,
                source_cycle.map(|cycle| cycle.min(i64::MAX as usize) as i64)
            ],
        )?;
        id
    };
    prune_scope(&transaction, platform, format, commander, &id)?;
    transaction.execute(
        "DELETE FROM hall_of_fame WHERE id NOT IN (
           SELECT id FROM hall_of_fame ORDER BY last_seen_at DESC, rowid DESC LIMIT ?1
         )",
        [MAX_TOTAL_ENTRIES as i64],
    )?;
    transaction.commit()?;
    load(data_dir, &id)
}

pub fn status(
    data_dir: &Path,
    platform: &str,
    format: &str,
    commander: bool,
) -> Result<HallOfFameStatus, HallOfFameError> {
    let entries = list(data_dir, platform, format, commander)?;
    let active_diverse_ids = diverse_indices(&entries, 16)
        .into_iter()
        .map(|index| entries[index].id.clone())
        .collect();
    Ok(HallOfFameStatus {
        entries,
        active_diverse_ids,
        maximum_entries_for_scope: MAX_SCOPE_ENTRIES,
    })
}

pub fn select_diverse(
    data_dir: &Path,
    platform: &str,
    format: &str,
    commander: bool,
    limit: usize,
) -> Result<Vec<HallOfFameEntry>, HallOfFameError> {
    let entries = list(data_dir, platform, format, commander)?;
    Ok(diverse_indices(&entries, limit.clamp(1, 32))
        .into_iter()
        .map(|index| entries[index].clone())
        .collect())
}

pub fn discover_packages(
    data_dir: &Path,
    platform: &str,
    format: &str,
    commander: bool,
    pool: &CardPool,
    limit: usize,
) -> Result<Vec<Vec<CardId>>, HallOfFameError> {
    let champions = select_diverse(data_dir, platform, format, commander, 16)?;
    if champions.len() < 2 {
        return Ok(Vec::new());
    }
    let eligible = pool
        .cards
        .iter()
        .filter(|card| !card.features.is_land)
        .map(|card| card.id.0.as_str())
        .collect::<HashSet<_>>();
    let mut pairs = HashMap::<(String, String), (usize, f64)>::new();
    for champion in &champions {
        let mut cards = champion
            .candidate
            .deck
            .mainboard
            .iter()
            .filter(|entry| entry.quantity > 0 && eligible.contains(entry.card_id.0.as_str()))
            .map(|entry| entry.card_id.0.clone())
            .collect::<Vec<_>>();
        cards.sort();
        cards.dedup();
        for left in 0..cards.len() {
            for right in left + 1..cards.len() {
                let pair = pairs
                    .entry((cards[left].clone(), cards[right].clone()))
                    .or_default();
                pair.0 += 1;
                pair.1 += champion.evidence_score.max(0.0);
            }
        }
    }
    let mut pairs = pairs
        .into_iter()
        .filter(|(_, (support, _))| *support >= 2)
        .collect::<Vec<_>>();
    pairs.sort_by(|left, right| {
        right
            .1
            .0
            .cmp(&left.1.0)
            .then_with(|| right.1.1.total_cmp(&left.1.1))
            .then_with(|| left.0.cmp(&right.0))
    });
    Ok(pairs
        .into_iter()
        .take(limit.clamp(1, 64))
        .map(|((left, right), _)| vec![CardId(left), CardId(right)])
        .collect())
}

pub fn delete(data_dir: &Path, id: &str) -> Result<(), HallOfFameError> {
    let connection = open(data_dir)?;
    if connection.execute("DELETE FROM hall_of_fame WHERE id=?1", [id])? == 0 {
        return Err(HallOfFameError::NotFound(id.into()));
    }
    Ok(())
}

fn load(data_dir: &Path, id: &str) -> Result<HallOfFameEntry, HallOfFameError> {
    let connection = open(data_dir)?;
    let mut entry = connection
        .query_row(
            "SELECT id, platform, format, commander, candidate_json, evidence_score,
                    source_run_id, source_cycle, promotion_count, created_at, last_seen_at
             FROM hall_of_fame WHERE id=?1",
            [id],
            entry_from_row,
        )
        .optional()?
        .ok_or_else(|| HallOfFameError::NotFound(id.into()))?;
    entry.novelty = novelty_against(
        &entry,
        &list(data_dir, &entry.platform, &entry.format, entry.commander)?,
    );
    Ok(entry)
}

fn list(
    data_dir: &Path,
    platform: &str,
    format: &str,
    commander: bool,
) -> Result<Vec<HallOfFameEntry>, HallOfFameError> {
    let connection = open(data_dir)?;
    let mut statement = connection.prepare(
        "SELECT id, platform, format, commander, candidate_json, evidence_score,
                source_run_id, source_cycle, promotion_count, created_at, last_seen_at
         FROM hall_of_fame WHERE platform=?1 AND format=?2 AND commander=?3
         ORDER BY evidence_score DESC, last_seen_at DESC, rowid DESC",
    )?;
    let rows = statement.query_map(params![platform, format, commander], entry_from_row)?;
    let mut entries = rows.collect::<Result<Vec<_>, _>>()?;
    let snapshot = entries.clone();
    for entry in &mut entries {
        entry.novelty = novelty_against(entry, &snapshot);
    }
    Ok(entries)
}

fn entry_from_row(row: &rusqlite::Row<'_>) -> Result<HallOfFameEntry, rusqlite::Error> {
    let candidate_json = row.get::<_, String>(4)?;
    let candidate = serde_json::from_str(&candidate_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            candidate_json.len(),
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })?;
    Ok(HallOfFameEntry {
        id: row.get(0)?,
        platform: row.get(1)?,
        format: row.get(2)?,
        commander: row.get(3)?,
        candidate,
        evidence_score: row.get(5)?,
        novelty: 1.0,
        source_run_id: row.get(6)?,
        source_cycle: row
            .get::<_, Option<i64>>(7)?
            .map(|cycle| cycle.max(0) as usize),
        promotion_count: row.get::<_, i64>(8)?.max(0) as usize,
        created_at: row.get(9)?,
        last_seen_at: row.get(10)?,
    })
}

fn prune_scope(
    connection: &Connection,
    platform: &str,
    format: &str,
    commander: bool,
    preserve_id: &str,
) -> Result<(), HallOfFameError> {
    let mut statement = connection.prepare(
        "SELECT id, platform, format, commander, candidate_json, evidence_score,
                source_run_id, source_cycle, promotion_count, created_at, last_seen_at
         FROM hall_of_fame WHERE platform=?1 AND format=?2 AND commander=?3
         ORDER BY evidence_score DESC, last_seen_at DESC, rowid DESC",
    )?;
    let entries = statement
        .query_map(params![platform, format, commander], entry_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    if entries.len() <= MAX_SCOPE_ENTRIES {
        return Ok(());
    }
    let mut retained = diverse_indices(&entries, MAX_SCOPE_ENTRIES)
        .into_iter()
        .map(|index| entries[index].id.clone())
        .collect::<HashSet<_>>();
    if !retained.contains(preserve_id) {
        if let Some(replace) = entries
            .iter()
            .rev()
            .find(|entry| retained.contains(&entry.id) && entry.id != preserve_id)
        {
            retained.remove(&replace.id);
        }
        retained.insert(preserve_id.into());
    }
    for entry in entries.iter().filter(|entry| !retained.contains(&entry.id)) {
        connection.execute("DELETE FROM hall_of_fame WHERE id=?1", [&entry.id])?;
    }
    Ok(())
}

fn diverse_indices(entries: &[HallOfFameEntry], limit: usize) -> Vec<usize> {
    if entries.is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut selected = vec![0usize];
    while selected.len() < entries.len().min(limit) {
        let next = (0..entries.len())
            .filter(|index| !selected.contains(index))
            .max_by(|left, right| {
                let score = |index: usize| {
                    let diversity = selected
                        .iter()
                        .map(|selected| {
                            deck_distance(
                                &entries[index].candidate.deck,
                                &entries[*selected].candidate.deck,
                            )
                        })
                        .reduce(f64::min)
                        .unwrap_or(1.0);
                    entries[index].evidence_score.clamp(0.0, 1.0) * 0.6 + diversity * 0.4
                };
                score(*left).total_cmp(&score(*right))
            });
        let Some(next) = next else { break };
        selected.push(next);
    }
    selected
}

fn novelty_against(entry: &HallOfFameEntry, entries: &[HallOfFameEntry]) -> f64 {
    entries
        .iter()
        .filter(|other| other.id != entry.id)
        .map(|other| deck_distance(&entry.candidate.deck, &other.candidate.deck))
        .reduce(f64::min)
        .unwrap_or(1.0)
}

fn deck_distance(left: &Deck, right: &Deck) -> f64 {
    let counts = |deck: &Deck| {
        let mut output = HashMap::new();
        for (zone, entries) in [
            ("command", &deck.command_zone),
            ("main", &deck.mainboard),
            ("side", &deck.sideboard),
        ] {
            for entry in entries {
                output.insert(
                    format!("{zone}:{}", entry.card_id.0),
                    entry.quantity as usize,
                );
            }
        }
        output
    };
    let left = counts(left);
    let right = counts(right);
    let keys = left.keys().chain(right.keys()).collect::<HashSet<_>>();
    let intersection = keys
        .iter()
        .map(|key| {
            left.get(*key)
                .copied()
                .unwrap_or(0)
                .min(right.get(*key).copied().unwrap_or(0))
        })
        .sum::<usize>();
    let union = keys
        .iter()
        .map(|key| {
            left.get(*key)
                .copied()
                .unwrap_or(0)
                .max(right.get(*key).copied().unwrap_or(0))
        })
        .sum::<usize>();
    if union == 0 {
        0.0
    } else {
        1.0 - intersection as f64 / union as f64
    }
}

fn deck_fingerprint(deck: &Deck) -> Result<String, serde_json::Error> {
    let mut canonical = deck.clone();
    canonical.name.clear();
    canonical
        .command_zone
        .sort_by(|left, right| left.card_id.0.cmp(&right.card_id.0));
    canonical
        .mainboard
        .sort_by(|left, right| left.card_id.0.cmp(&right.card_id.0));
    canonical
        .sideboard
        .sort_by(|left, right| left.card_id.0.cmp(&right.card_id.0));
    let payload = serde_json::to_vec(&canonical)?;
    Ok(format!("sha256:{:x}", Sha256::digest(payload)))
}

fn open(data_dir: &Path) -> Result<Connection, HallOfFameError> {
    fs::create_dir_all(data_dir)?;
    let path: PathBuf = data_dir.join("hall-of-fame.sqlite");
    let connection = Connection::open(path)?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS hall_of_fame (
           id TEXT PRIMARY KEY,
           platform TEXT NOT NULL,
           format TEXT NOT NULL,
           commander INTEGER NOT NULL,
           deck_hash TEXT NOT NULL,
           candidate_json TEXT NOT NULL,
           evidence_score REAL NOT NULL,
           source_run_id TEXT,
           source_cycle INTEGER,
           promotion_count INTEGER NOT NULL DEFAULT 1,
           created_at TEXT NOT NULL DEFAULT (datetime('now')),
           last_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
           UNIQUE(platform, format, commander, deck_hash)
         );
         CREATE INDEX IF NOT EXISTS hall_of_fame_scope
           ON hall_of_fame(platform, format, commander, evidence_score DESC);",
    )?;
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use metagene_core::{Card, CardFeatures, CardId, DeckEntry, Fitness};
    use std::collections::BTreeMap;

    fn candidate(rank: usize, cards: &[(&str, u8)], score: f64) -> RankedDeck {
        RankedDeck {
            rank,
            pareto_rank: 0,
            deck: Deck {
                name: format!("Candidate {rank}"),
                command_zone: Vec::new(),
                mainboard: cards
                    .iter()
                    .map(|(id, quantity)| DeckEntry {
                        card_id: CardId((*id).into()),
                        quantity: *quantity,
                    })
                    .collect(),
                sideboard: Vec::new(),
            },
            fitness: Fitness {
                score,
                ..Default::default()
            },
            packages: Vec::new(),
            commander_compatibility: None,
            candidate_status: "provisional".into(),
        }
    }

    fn pool_card(id: &str, land: bool) -> Card {
        Card {
            id: CardId(id.into()),
            arena_id: None,
            platforms: vec!["arena".into()],
            name: id.into(),
            set_code: "TST".into(),
            collector_number: "1".into(),
            type_line: if land { "Land" } else { "Creature" }.into(),
            oracle_text: String::new(),
            colors: Vec::new(),
            color_identity: Vec::new(),
            image_uri: None,
            legalities: BTreeMap::new(),
            features: CardFeatures {
                is_land: land,
                ..Default::default()
            },
        }
    }

    #[test]
    fn exact_champions_deduplicate_and_keep_the_best_evidence() {
        let directory = std::env::temp_dir().join(format!("metagene-hof-{}", Uuid::new_v4()));
        let first = candidate(1, &[("a", 4), ("b", 4)], 0.5);
        let second = candidate(2, &[("b", 4), ("a", 4)], 0.6);
        let saved = promote(
            &directory,
            &first,
            HallOfFamePromotion {
                platform: "arena",
                format: "historic",
                commander: false,
                evidence_score: 0.4,
                source_run_id: Some("run-1"),
                source_cycle: Some(1),
            },
        )
        .unwrap();
        let updated = promote(
            &directory,
            &second,
            HallOfFamePromotion {
                platform: "arena",
                format: "historic",
                commander: false,
                evidence_score: 0.7,
                source_run_id: Some("run-2"),
                source_cycle: Some(2),
            },
        )
        .unwrap();
        assert_eq!(saved.id, updated.id);
        assert_eq!(updated.evidence_score, 0.7);
        assert_eq!(updated.promotion_count, 2);
        assert_eq!(
            status(&directory, "arena", "historic", false)
                .unwrap()
                .entries
                .len(),
            1
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn diverse_selection_does_not_fill_the_archive_with_near_duplicates() {
        let directory =
            std::env::temp_dir().join(format!("metagene-hof-diverse-{}", Uuid::new_v4()));
        let entries = [
            candidate(1, &[("a", 4), ("b", 4)], 0.9),
            candidate(2, &[("a", 4), ("b", 3), ("c", 1)], 0.89),
            candidate(3, &[("x", 4), ("y", 4)], 0.7),
        ];
        for entry in &entries {
            promote(
                &directory,
                entry,
                HallOfFamePromotion {
                    platform: "arena",
                    format: "historic",
                    commander: false,
                    evidence_score: entry.fitness.score,
                    source_run_id: None,
                    source_cycle: None,
                },
            )
            .unwrap();
        }
        let selected = select_diverse(&directory, "arena", "historic", false, 2).unwrap();
        assert_eq!(selected[0].candidate.rank, 1);
        assert_eq!(selected[1].candidate.rank, 3);
        assert!(selected[1].novelty > selected[0].novelty);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn package_discovery_mines_repeated_nonland_pairs_from_successful_decks() {
        let directory =
            std::env::temp_dir().join(format!("metagene-hof-packages-{}", Uuid::new_v4()));
        for entry in [
            candidate(1, &[("a", 4), ("b", 4), ("c", 4)], 0.8),
            candidate(2, &[("a", 4), ("b", 3), ("land", 20)], 0.7),
        ] {
            promote(
                &directory,
                &entry,
                HallOfFamePromotion {
                    platform: "arena",
                    format: "historic",
                    commander: false,
                    evidence_score: entry.fitness.score,
                    source_run_id: None,
                    source_cycle: None,
                },
            )
            .unwrap();
        }
        let pool = CardPool {
            cards: vec![
                pool_card("a", false),
                pool_card("b", false),
                pool_card("c", false),
                pool_card("land", true),
            ],
        };
        let packages = discover_packages(&directory, "arena", "historic", false, &pool, 8).unwrap();
        assert_eq!(packages, vec![vec![CardId("a".into()), CardId("b".into())]]);
        fs::remove_dir_all(directory).unwrap();
    }
}
