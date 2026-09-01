use flate2::read::GzDecoder;
use metagene_core::{Card, CardFeatures, CardId, Color};
use reqwest::blocking::Client;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const CATALOG_FILE: &str = "catalog.sqlite";
const PENDING_BULK_FILE: &str = "default-cards.pending.jsonl.gz";
const USER_AGENT: &str = "Metagene/0.1 (local MTG Arena deck evolution workbench)";
const CATALOG_MODEL_VERSION: u32 = 11;
static LEGAL_POOL_CACHE: OnceLock<Mutex<HashMap<String, Vec<Card>>>> = OnceLock::new();

#[derive(Clone, Debug, Default, Serialize)]
pub struct CatalogStatus {
    pub ready: bool,
    pub card_count: usize,
    pub updated_at: Option<String>,
    pub database_bytes: u64,
    pub catalog_version: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct CatalogProgress {
    pub stage: String,
    pub processed: usize,
    pub retained: usize,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct OracleResidualAuditItem {
    pub card_id: String,
    pub card_name: String,
    pub mode_name: String,
    pub clause_id: String,
    pub criticality: metagene_core::ClauseCriticality,
    pub text: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct OracleAccountingAudit {
    pub catalog_model_version: u32,
    pub cards: usize,
    pub playable_modes: usize,
    pub gameplay_clauses: usize,
    pub accounted_clauses: usize,
    pub parsed_clauses: usize,
    pub unresolved_clauses: usize,
    pub accounted_clause_rate: f64,
    pub parsed_clause_rate: f64,
    pub unresolved_clause_rate: f64,
    pub claim_blocking_residuals: usize,
    pub residuals: Vec<OracleResidualAuditItem>,
}

#[derive(Debug, Deserialize)]
struct BulkResponse {
    data: Vec<BulkItem>,
}

#[derive(Debug, Deserialize)]
struct BulkItem {
    #[serde(rename = "type")]
    kind: String,
    updated_at: String,
    jsonl_download_uri: Option<String>,
    compressed_size: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ScryfallFace {
    #[serde(default)]
    name: String,
    #[serde(default)]
    oracle_text: String,
    #[serde(default)]
    type_line: String,
    #[serde(default)]
    mana_cost: String,
    #[serde(default)]
    colors: Vec<String>,
    image_uris: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
struct ScryfallCard {
    id: String,
    arena_id: Option<u64>,
    name: String,
    set: String,
    collector_number: String,
    #[serde(default)]
    layout: String,
    #[serde(default)]
    mana_cost: String,
    #[serde(default)]
    type_line: String,
    #[serde(default)]
    oracle_text: String,
    #[serde(default, rename = "cmc")]
    mana_value: f32,
    #[serde(default)]
    colors: Vec<String>,
    #[serde(default)]
    color_identity: Vec<String>,
    #[serde(default)]
    games: Vec<String>,
    #[serde(default)]
    legalities: BTreeMap<String, String>,
    image_uris: Option<HashMap<String, String>>,
    card_faces: Option<Vec<ScryfallFace>>,
    power: Option<String>,
    #[serde(default)]
    keywords: Vec<String>,
}

pub fn status(data_dir: &Path) -> Result<CatalogStatus, Box<dyn std::error::Error>> {
    let path = catalog_path(data_dir);
    if !path.exists() {
        return Ok(CatalogStatus::default());
    }
    let connection = Connection::open(&path)?;
    initialize(&connection)?;
    let card_count: i64 =
        connection.query_row("SELECT COUNT(*) FROM cards", [], |row| row.get(0))?;
    let updated_at = connection
        .query_row(
            "SELECT value FROM metadata WHERE key = 'updated_at'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let catalog_version = connection
        .query_row(
            "SELECT value FROM metadata WHERE key = 'catalog_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    Ok(CatalogStatus {
        ready: card_count > 0 && catalog_version == CATALOG_MODEL_VERSION,
        card_count: card_count.max(0) as usize,
        updated_at,
        database_bytes: fs::metadata(path)?.len(),
        catalog_version,
    })
}

pub fn sync<F>(
    data_dir: &Path,
    mut on_progress: F,
) -> Result<CatalogStatus, Box<dyn std::error::Error>>
where
    F: FnMut(CatalogProgress),
{
    fs::create_dir_all(data_dir)?;
    let client = Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(300))
        .build()?;
    on_progress(progress("metadata", 0, 0, 0, None));
    let metadata: BulkResponse = client
        .get("https://api.scryfall.com/bulk-data")
        .header("Accept", "application/json;q=0.9,*/*;q=0.8")
        .send()?
        .error_for_status()?
        .json()?;
    let bulk = metadata
        .data
        .into_iter()
        .find(|item| item.kind == "default_cards")
        .ok_or("Scryfall did not return the default_cards bulk dataset")?;
    let uri = bulk
        .jsonl_download_uri
        .as_deref()
        .ok_or("Scryfall did not return a compressed JSONL download")?;
    let pending = data_dir.join(PENDING_BULK_FILE);
    let mut response = client.get(uri).send()?.error_for_status()?;
    let total = response.content_length().or(bulk.compressed_size);
    let mut output = File::create(&pending)?;
    let downloaded = std::io::copy(&mut response, &mut output)?;
    on_progress(progress("importing", 0, 0, downloaded, total));

    let result = import_jsonl(
        data_dir,
        &pending,
        &bulk.updated_at,
        downloaded,
        total,
        &mut on_progress,
    );
    let _ = fs::remove_file(&pending);
    result?;
    on_progress(progress("complete", 0, 0, downloaded, total));
    status(data_dir)
}

fn import_jsonl<F>(
    data_dir: &Path,
    pending: &Path,
    updated_at: &str,
    downloaded: u64,
    total: Option<u64>,
    mut on_progress: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnMut(CatalogProgress),
{
    let mut connection = Connection::open(catalog_path(data_dir))?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    initialize(&connection)?;
    // A DeckEntry identifies a logical card, not one of its many set printings.
    // Preserve the representative IDs already used by saved decks whenever that
    // printing still exists, then fall back to a deterministic Arena-preferred
    // representative for new cards and installations.
    let preferred_ids = preferred_printing_ids(&connection)?;
    let mut logical_cards = HashMap::<String, Card>::new();
    let mut printing_aliases = Vec::<(String, String)>::new();
    let file = File::open(pending)?;
    let reader = BufReader::with_capacity(1024 * 1024, GzDecoder::new(file));
    let mut processed = 0_usize;
    for (index, line) in reader.lines().enumerate() {
        let line = line?;
        processed = index + 1;
        let source: ScryfallCard = serde_json::from_str(&line)?;
        let card = into_card(source);
        let key = card.name.to_ascii_lowercase();
        printing_aliases.push((card.id.0.clone(), key.clone()));
        if let Some(existing) = logical_cards.get_mut(&key) {
            merge_logical_printing(existing, card, preferred_ids.get(&key).map(String::as_str));
        } else {
            logical_cards.insert(key, card);
        }
        if processed.is_multiple_of(5_000) {
            on_progress(CatalogProgress {
                stage: "importing".into(),
                processed,
                retained: logical_cards.len(),
                downloaded_bytes: downloaded,
                total_bytes: total,
            });
        }
    }

    let mut cards = logical_cards.into_values().collect::<Vec<_>>();
    cards.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
    });
    let canonical_ids = cards
        .iter()
        .map(|card| (card.name.to_ascii_lowercase(), card.id.0.clone()))
        .collect::<HashMap<_, _>>();
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "DROP TABLE IF EXISTS cards_v6;
         DROP TABLE IF EXISTS printing_aliases_v6;
         CREATE TABLE cards_v6 (
           id TEXT PRIMARY KEY,
           name TEXT NOT NULL COLLATE NOCASE UNIQUE,
           card_json TEXT NOT NULL
         );
         CREATE TABLE printing_aliases_v6 (
           printing_id TEXT PRIMARY KEY,
           card_id TEXT NOT NULL
         );",
    )?;
    {
        let mut statement = transaction
            .prepare("INSERT INTO cards_v6 (id, name, card_json) VALUES (?1, ?2, ?3)")?;
        for card in &cards {
            statement.execute(params![card.id.0, card.name, serde_json::to_string(card)?])?;
        }
    }
    {
        let mut statement = transaction
            .prepare("INSERT INTO printing_aliases_v6 (printing_id, card_id) VALUES (?1, ?2)")?;
        for (printing_id, name) in &printing_aliases {
            if let Some(card_id) = canonical_ids.get(name) {
                statement.execute(params![printing_id, card_id])?;
            }
        }
    }
    transaction.execute_batch(
        "DROP TABLE cards;
         DROP TABLE IF EXISTS printing_aliases;
         ALTER TABLE cards_v6 RENAME TO cards;
         ALTER TABLE printing_aliases_v6 RENAME TO printing_aliases;
         CREATE INDEX cards_name ON cards(name);
         CREATE INDEX printing_aliases_card_id ON printing_aliases(card_id);",
    )?;
    transaction.execute(
        "INSERT OR REPLACE INTO metadata (key, value) VALUES ('updated_at', ?1)",
        [updated_at],
    )?;
    transaction.execute(
        "INSERT OR REPLACE INTO metadata (key, value) VALUES ('catalog_version', ?1)",
        [CATALOG_MODEL_VERSION.to_string()],
    )?;
    transaction.commit()?;
    on_progress(CatalogProgress {
        stage: "optimizing".into(),
        processed,
        retained: cards.len(),
        downloaded_bytes: downloaded,
        total_bytes: total,
    });
    // Dropping the printing-level table leaves its pages on SQLite's freelist.
    // Compact immediately so catalog v6 actually saves disk instead of carrying
    // hundreds of MiB of unreachable printing JSON until a future maintenance run.
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")?;
    Ok(())
}

fn preferred_printing_ids(connection: &Connection) -> rusqlite::Result<HashMap<String, String>> {
    let mut fallback = connection.prepare(
        "SELECT id, name FROM cards GROUP BY name COLLATE NOCASE ORDER BY name COLLATE NOCASE",
    )?;
    let mut preferred = fallback
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?.to_ascii_lowercase(),
                row.get::<_, String>(0)?,
            ))
        })?
        .collect::<rusqlite::Result<HashMap<_, _>>>()?;
    // Arena representatives are authoritative when available because MTGA
    // export needs a set/collector pair Arena actually recognizes.
    let mut arena = connection.prepare(
        "SELECT id, name FROM cards
         WHERE json_extract(card_json, '$.arena_id') IS NOT NULL
         GROUP BY name COLLATE NOCASE ORDER BY name COLLATE NOCASE",
    )?;
    for row in arena.query_map([], |row| {
        Ok((
            row.get::<_, String>(1)?.to_ascii_lowercase(),
            row.get::<_, String>(0)?,
        ))
    })? {
        let (name, id) = row?;
        preferred.insert(name, id);
    }
    Ok(preferred)
}

fn merge_logical_printing(existing: &mut Card, candidate: Card, preferred_id: Option<&str>) {
    let replace = match preferred_id {
        Some(preferred) if candidate.id.0 == preferred => true,
        Some(preferred) if existing.id.0 == preferred => false,
        _ => printing_preference(&candidate) < printing_preference(existing),
    };
    let (mut representative, alternate) = if replace {
        (candidate, existing.clone())
    } else {
        (existing.clone(), candidate)
    };
    representative.platforms.extend(alternate.platforms);
    representative.platforms.sort();
    representative.platforms.dedup();
    for (format, legality) in alternate.legalities {
        representative.legalities.entry(format).or_insert(legality);
    }
    if representative.arena_id.is_none() {
        representative.arena_id = alternate.arena_id;
    }
    if representative.image_uri.is_none() {
        representative.image_uri = alternate.image_uri;
    }
    *existing = representative;
}

fn printing_preference(card: &Card) -> (u8, u64, &str, &str, &str) {
    (
        u8::from(card.arena_id.is_none()),
        card.arena_id.unwrap_or(u64::MAX),
        card.set_code.as_str(),
        card.collector_number.as_str(),
        card.id.0.as_str(),
    )
}

pub fn search(
    data_dir: &Path,
    query: &str,
    platform: &str,
    format: &str,
    limit: usize,
) -> Result<Vec<Card>, Box<dyn std::error::Error>> {
    let path = catalog_path(data_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let connection = Connection::open(path)?;
    initialize(&connection)?;
    let pattern = format!("%{}%", query.trim());
    let legality_path = format!("$.legalities.{}", format.to_ascii_lowercase());
    let mut statement = connection.prepare(
        "SELECT card_json FROM cards
         WHERE name LIKE ?1 ESCAPE '\\'
           AND json_extract(card_json, ?2) IN ('legal', 'restricted')
           AND EXISTS (
             SELECT 1 FROM json_each(card_json, '$.platforms') WHERE value = ?3
           )
         ORDER BY CASE WHEN lower(name) = lower(?4) THEN 0 ELSE 1 END, length(name), name
         LIMIT ?5",
    )?;
    let rows = statement.query_map(
        params![pattern, legality_path, platform, query.trim(), limit as i64],
        |row| row.get::<_, String>(0),
    )?;
    let mut cards = Vec::new();
    for row in rows {
        cards.push(decode_card(&row?)?);
    }
    Ok(cards)
}

pub fn legal_pool(
    data_dir: &Path,
    platform: &str,
    format: &str,
) -> Result<Vec<Card>, Box<dyn std::error::Error>> {
    let path = catalog_path(data_dir);
    let metadata = fs::metadata(&path)?;
    let modified = metadata
        .modified()?
        .duration_since(std::time::SystemTime::UNIX_EPOCH)?
        .as_nanos();
    let key = format!(
        "{}|{}|{}|{}|{}",
        path.display(),
        metadata.len(),
        modified,
        platform,
        format
    );
    let cache = LEGAL_POOL_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(cards) = cache.lock().ok().and_then(|cache| cache.get(&key).cloned()) {
        return Ok(cards);
    }
    let cards = search(data_dir, "", platform, format, usize::MAX)?;
    if let Ok(mut cache) = cache.lock() {
        if cache.len() >= 12 {
            cache.clear();
        }
        cache.insert(key, cards.clone());
    }
    Ok(cards)
}

pub fn audit_oracle_accounting(cards: &[Card]) -> OracleAccountingAudit {
    let mut audit = OracleAccountingAudit {
        catalog_model_version: CATALOG_MODEL_VERSION,
        cards: cards.len(),
        ..OracleAccountingAudit::default()
    };
    for card in cards {
        audit.playable_modes += card.features.initial_modes.len();
        for mode in &card.features.initial_modes {
            for clause in &mode.oracle_clauses {
                use metagene_core::ClauseClassification;
                if clause.classification == ClauseClassification::ReminderText {
                    continue;
                }
                audit.gameplay_clauses += 1;
                audit.accounted_clauses += 1;
                if clause.classification == ClauseClassification::UnresolvedGameplayResidual {
                    audit.unresolved_clauses += 1;
                    if clause.criticality.blocks_claim() {
                        audit.claim_blocking_residuals += 1;
                    }
                    audit.residuals.push(OracleResidualAuditItem {
                        card_id: card.id.0.clone(),
                        card_name: card.name.clone(),
                        mode_name: mode.name.clone(),
                        clause_id: clause.id.clone(),
                        criticality: clause.criticality,
                        text: clause.text.clone(),
                    });
                } else {
                    audit.parsed_clauses += 1;
                }
            }
        }
    }
    let denominator = audit.gameplay_clauses.max(1) as f64;
    audit.accounted_clause_rate = audit.accounted_clauses as f64 / denominator;
    audit.parsed_clause_rate = audit.parsed_clauses as f64 / denominator;
    audit.unresolved_clause_rate = audit.unresolved_clauses as f64 / denominator;
    audit.residuals.sort_by(|left, right| {
        left.card_name
            .cmp(&right.card_name)
            .then_with(|| left.mode_name.cmp(&right.mode_name))
            .then_with(|| left.clause_id.cmp(&right.clause_id))
    });
    audit
}

pub fn cards_by_ids(
    data_dir: &Path,
    ids: &[CardId],
) -> Result<Vec<Card>, Box<dyn std::error::Error>> {
    let path = catalog_path(data_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let connection = Connection::open(path)?;
    initialize(&connection)?;
    let mut statement = connection.prepare("SELECT card_json FROM cards WHERE id = ?1")?;
    let mut alias_statement =
        connection.prepare("SELECT card_id FROM printing_aliases WHERE printing_id = ?1")?;
    let mut seen = std::collections::HashSet::new();
    let mut cards = Vec::new();
    for id in ids.iter().filter(|id| seen.insert(id.0.clone())) {
        let canonical_id = alias_statement
            .query_row([&id.0], |row| row.get::<_, String>(0))
            .optional()?
            .unwrap_or_else(|| id.0.clone());
        if let Some(json) = statement
            .query_row([&canonical_id], |row| row.get::<_, String>(0))
            .optional()?
        {
            cards.push(decode_card(&json)?);
        }
    }
    Ok(cards)
}

pub fn canonical_ids(
    data_dir: &Path,
    ids: &[CardId],
) -> Result<HashMap<CardId, CardId>, Box<dyn std::error::Error>> {
    let path = catalog_path(data_dir);
    if !path.exists() || ids.is_empty() {
        return Ok(HashMap::new());
    }
    let connection = Connection::open(path)?;
    initialize(&connection)?;
    let mut statement =
        connection.prepare("SELECT card_id FROM printing_aliases WHERE printing_id = ?1")?;
    let mut resolved = HashMap::new();
    for id in ids {
        if resolved.contains_key(id) {
            continue;
        }
        if let Some(canonical) = statement
            .query_row([&id.0], |row| row.get::<_, String>(0))
            .optional()?
        {
            resolved.insert(id.clone(), CardId(canonical));
        }
    }
    Ok(resolved)
}

fn initialize(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS cards (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL COLLATE NOCASE UNIQUE,
            card_json TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS cards_name ON cards(name);
         CREATE TABLE IF NOT EXISTS printing_aliases (
            printing_id TEXT PRIMARY KEY,
            card_id TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS printing_aliases_card_id ON printing_aliases(card_id);
         CREATE TABLE IF NOT EXISTS metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
         );",
    )
}

fn into_card(source: ScryfallCard) -> Card {
    let face_type_line = source
        .card_faces
        .as_ref()
        .map(|faces| {
            faces
                .iter()
                .map(|face| face.type_line.as_str())
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join(" // ")
        })
        .unwrap_or_default();
    let type_line = if source.type_line.is_empty() {
        face_type_line
    } else {
        source.type_line.clone()
    };
    let face_text = source
        .card_faces
        .as_ref()
        .map(|faces| {
            faces
                .iter()
                .map(|face| face.oracle_text.as_str())
                .collect::<Vec<_>>()
                .join(" // ")
        })
        .unwrap_or_default();
    let oracle_text = if source.oracle_text.is_empty() {
        face_text
    } else {
        source.oracle_text.clone()
    };
    let image_uri = source
        .image_uris
        .as_ref()
        .and_then(|images| images.get("normal").or_else(|| images.get("large")))
        .cloned()
        .or_else(|| {
            source.card_faces.as_ref()?.iter().find_map(|face| {
                face.image_uris
                    .as_ref()?
                    .get("normal")
                    .or_else(|| face.image_uris.as_ref()?.get("large"))
                    .cloned()
            })
        });
    let lower_text = oracle_text.to_ascii_lowercase();
    let lower_type = type_line.to_ascii_lowercase();
    let mut initial_modes = source.card_faces.as_ref().map_or_else(
        || {
            vec![card_mode(
                &source.name,
                &source.type_line,
                &source.mana_cost,
                &source.colors,
                &source.oracle_text,
                &source.keywords,
            )]
        },
        |faces| {
            let count = if matches!(
                source.layout.as_str(),
                "modal_dfc" | "split" | "adventure" | "aftermath"
            ) {
                faces.len()
            } else {
                1
            };
            faces
                .iter()
                .take(count)
                .map(|face| {
                    card_mode(
                        &face.name,
                        &face.type_line,
                        &face.mana_cost,
                        &face.colors,
                        &face.oracle_text,
                        &source.keywords,
                    )
                })
                .collect()
        },
    );
    if initial_modes.is_empty() {
        initial_modes.push(card_mode(
            &source.name,
            &source.type_line,
            &source.mana_cost,
            &source.colors,
            &source.oracle_text,
            &source.keywords,
        ));
    }
    let is_land = initial_modes.first().is_some_and(|mode| mode.is_land);
    let can_be_land = initial_modes.iter().any(|mode| mode.is_land);
    let is_legendary = initial_modes.iter().any(|mode| mode.is_legendary);
    let colors = initial_modes
        .iter()
        .flat_map(|mode| mode.colors.iter().copied())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let parsed_abilities = initial_modes
        .iter()
        .flat_map(|mode| mode.abilities.iter())
        .cloned()
        .collect::<Vec<_>>();
    let mechanics = mechanic_profile(&lower_text, &lower_type);
    let mechanic_inventory = mechanic_inventory(&source.keywords, &oracle_text, &parsed_abilities);
    let oracle_coverage = oracle_coverage(&initial_modes);
    let mut tags = Vec::new();
    for (tag, matches) in [
        ("token", lower_text.contains("token")),
        ("copy", lower_text.contains("copy")),
        ("draw", lower_text.contains("draw a card")),
        (
            "ramp",
            lower_text.contains("add {")
                || lower_text.contains("search your library for a basic land"),
        ),
        (
            "interaction",
            lower_text.contains("destroy target")
                || lower_text.contains("exile target")
                || lower_text.contains("counter target")
                || lower_text.contains("return target"),
        ),
        ("recursion", lower_text.contains("from your graveyard")),
        ("creature", lower_type.contains("creature")),
        ("artifact", lower_type.contains("artifact")),
    ] {
        if matches {
            tags.push(tag.into());
        }
    }
    let parsed_power = source
        .power
        .as_deref()
        .and_then(|power| power.parse::<f32>().ok())
        .unwrap_or(0.0);
    let mut card = Card {
        id: CardId(source.id),
        arena_id: source.arena_id,
        platforms: source.games,
        name: source.name,
        set_code: source.set,
        collector_number: source.collector_number,
        type_line,
        oracle_text,
        colors,
        color_identity: source
            .color_identity
            .iter()
            .filter_map(|color| parse_color(color))
            .collect(),
        image_uri,
        legalities: source.legalities,
        features: CardFeatures {
            is_land,
            unlimited_copies: lower_type.contains("basic land")
                || lower_text.contains("a deck can have any number of cards named"),
            enters_tapped: is_land && lower_text.contains("enters tapped"),
            mana_value: source.mana_value,
            mana_production: if is_land || lower_text.contains("{t}: add") {
                1.0
            } else {
                0.0
            },
            ramp: parsed_abilities
                .iter()
                .filter(|ability| {
                    matches!(
                        ability.kind,
                        metagene_core::AbilityKind::Ramp | metagene_core::AbilityKind::Search
                    )
                })
                .map(|ability| ability.effect_value * ability.parser_confidence)
                .fold(0.0_f32, f32::max),
            card_advantage: parsed_abilities
                .iter()
                .filter(|ability| ability.kind == metagene_core::AbilityKind::Draw)
                .map(|ability| ability.effect_value * ability.parser_confidence)
                .fold(0.0_f32, f32::max),
            interaction: parsed_abilities
                .iter()
                .filter(|ability| {
                    matches!(
                        ability.kind,
                        metagene_core::AbilityKind::Destroy
                            | metagene_core::AbilityKind::Exile
                            | metagene_core::AbilityKind::Bounce
                            | metagene_core::AbilityKind::CounterSpell
                            | metagene_core::AbilityKind::GraveyardHate
                    )
                })
                .map(|ability| ability.effect_value * ability.parser_confidence)
                .fold(0.0_f32, f32::max),
            pressure: (parsed_power / 4.0).clamp(0.0, 2.0),
            resilience: if lower_text.contains("indestructible") || lower_text.contains("ward") {
                0.8
            } else {
                0.0
            },
            tags,
            is_legendary,
            can_be_land,
            initial_modes,
            mechanics,
            mechanic_inventory,
            oracle_coverage,
        },
    };
    card.enrich_derived_tags();
    card
}

fn mechanic_inventory(
    keywords: &[String],
    oracle_text: &str,
    abilities: &[metagene_core::AbilityProfile],
) -> Vec<metagene_core::MechanicDescriptor> {
    let mut inventory = BTreeMap::<String, metagene_core::MechanicDescriptor>::new();
    let mut add = |id: String,
                   label: String,
                   source: &str,
                   criticality: metagene_core::ClauseCriticality,
                   confidence: f32| {
        inventory
            .entry(id.clone())
            .or_insert(metagene_core::MechanicDescriptor {
                id,
                label,
                source: source.into(),
                critical: criticality.blocks_claim(),
                criticality,
                parser_confidence: confidence,
            });
    };
    for keyword in keywords {
        let normalized = keyword
            .trim()
            .to_ascii_lowercase()
            .replace(|character: char| !character.is_ascii_alphanumeric(), "-")
            .trim_matches('-')
            .to_string();
        if !normalized.is_empty() {
            add(
                format!("keyword:{normalized}"),
                keyword.trim().into(),
                "scryfall_keyword",
                metagene_core::ClauseCriticality::OutcomeCritical,
                1.0,
            );
        }
    }
    for ability in abilities {
        add(
            format!("action:{:?}", ability.kind).to_ascii_lowercase(),
            format!("{:?}", ability.kind),
            "typed_action",
            if ability.kind == metagene_core::AbilityKind::Unknown {
                metagene_core::ClauseCriticality::RuleCritical
            } else {
                metagene_core::ClauseCriticality::OutcomeCritical
            },
            ability.parser_confidence,
        );
        if ability.once_per_turn {
            add(
                "restriction:once-per-turn".into(),
                "Once per turn".into(),
                "typed_action",
                metagene_core::ClauseCriticality::RuleCritical,
                ability.parser_confidence,
            );
        }
    }
    let lower = oracle_text.to_ascii_lowercase();
    for (needle, id, label) in [
        (" instead", "rules:replacement", "Replacement effect"),
        ("copy", "rules:copy", "Copy characteristics or object"),
        (
            "gain control",
            "rules:control-change",
            "Control-changing effect",
        ),
        ("extra turn", "rules:turn-structure", "Extra turn"),
        ("additional turn", "rules:turn-structure", "Additional turn"),
        ("conjure", "arena:conjure", "Conjure"),
        ("spellbook", "arena:spellbook", "Spellbook"),
        ("perpetually", "arena:perpetual", "Perpetual modification"),
        ("seek ", "arena:seek", "Seek"),
        ("intensity", "arena:intensity", "Intensity"),
        ("double team", "arena:double-team", "Double team"),
    ] {
        if lower.contains(needle) {
            add(
                id.into(),
                label.into(),
                "oracle_template",
                metagene_core::ClauseCriticality::RuleCritical,
                1.0,
            );
        }
    }
    inventory.into_values().collect()
}

fn oracle_coverage(modes: &[metagene_core::CardMode]) -> metagene_core::OracleCoverageSummary {
    let clauses = modes
        .iter()
        .flat_map(|mode| mode.oracle_clauses.iter())
        .filter(|clause| clause.classification != metagene_core::ClauseClassification::ReminderText)
        .collect::<Vec<_>>();
    let gameplay = clauses.len() as u32;
    let parsed = clauses
        .iter()
        .filter(|clause| {
            matches!(
                clause.classification,
                metagene_core::ClauseClassification::ParsedGameplayRule
                    | metagene_core::ClauseClassification::RecognizedKeywordRule
            )
        })
        .count() as u32;
    let unresolved = gameplay.saturating_sub(parsed);
    let denominator = gameplay.max(1) as f32;
    metagene_core::OracleCoverageSummary {
        gameplay_clauses: gameplay,
        accounted_clauses: gameplay,
        parsed_clauses: parsed,
        unresolved_clauses: unresolved,
        accounted_clause_rate: 1.0,
        parsed_clause_rate: parsed as f32 / denominator,
        unresolved_clause_rate: unresolved as f32 / denominator,
        unresolved_text: clauses
            .into_iter()
            .filter(|clause| {
                clause.classification
                    == metagene_core::ClauseClassification::UnresolvedGameplayResidual
            })
            .map(|clause| clause.text.clone())
            .collect(),
    }
}

fn oracle_clauses(
    mode_name: &str,
    oracle: &str,
    keywords: &[String],
) -> Vec<metagene_core::OracleClause> {
    let keyword_names = keywords
        .iter()
        .map(|keyword| keyword.trim().to_ascii_lowercase())
        .filter(|keyword| !keyword.is_empty())
        .collect::<Vec<_>>();
    let mut clauses = Vec::new();
    for raw_line in oracle
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let (gameplay, reminders) = separate_reminder_text(raw_line);
        if !gameplay.trim().is_empty() {
            let ordinal = clauses.len() as u16;
            let normalized = normalize_oracle_clause(&gameplay);
            let keyword_only = is_keyword_only_clause(&normalized, &keyword_names);
            clauses.push(metagene_core::OracleClause {
                id: oracle_clause_id(mode_name, ordinal, &normalized),
                mode_name: mode_name.into(),
                ordinal,
                text: gameplay.trim().into(),
                normalized_text: normalized,
                classification: if keyword_only {
                    metagene_core::ClauseClassification::RecognizedKeywordRule
                } else {
                    metagene_core::ClauseClassification::UnresolvedGameplayResidual
                },
                criticality: clause_criticality(&gameplay),
                parser_confidence: f32::from(keyword_only),
                action_ids: Vec::new(),
            });
        }
        for reminder in reminders {
            let ordinal = clauses.len() as u16;
            let normalized = normalize_oracle_clause(&reminder);
            clauses.push(metagene_core::OracleClause {
                id: oracle_clause_id(mode_name, ordinal, &normalized),
                mode_name: mode_name.into(),
                ordinal,
                text: reminder,
                normalized_text: normalized,
                classification: metagene_core::ClauseClassification::ReminderText,
                criticality: metagene_core::ClauseCriticality::Informational,
                parser_confidence: 1.0,
                action_ids: Vec::new(),
            });
        }
    }
    clauses
}

fn separate_reminder_text(value: &str) -> (String, Vec<String>) {
    let mut gameplay = String::new();
    let mut reminders = Vec::new();
    let mut reminder = String::new();
    let mut depth = 0_u16;
    for character in value.chars() {
        match character {
            '(' => {
                if depth > 0 {
                    reminder.push(character);
                }
                depth = depth.saturating_add(1);
            }
            ')' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    let text = reminder.trim();
                    if !text.is_empty() {
                        reminders.push(text.to_string());
                    }
                    reminder.clear();
                } else {
                    reminder.push(character);
                }
            }
            _ if depth > 0 => reminder.push(character),
            _ => gameplay.push(character),
        }
    }
    if depth > 0 {
        gameplay.push('(');
        gameplay.push_str(&reminder);
    }
    (
        gameplay.split_whitespace().collect::<Vec<_>>().join(" "),
        reminders,
    )
}

fn normalize_oracle_clause(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_ascii_lowercase()
}

fn oracle_clause_id(mode_name: &str, ordinal: u16, normalized: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"metagene.oracle-clause.v1\0");
    hasher.update(mode_name.trim().to_ascii_lowercase().as_bytes());
    hasher.update(b"\0");
    hasher.update(ordinal.to_le_bytes());
    hasher.update(b"\0");
    hasher.update(normalized.as_bytes());
    format!("oc-{:x}", hasher.finalize())
}

fn is_keyword_only_clause(value: &str, keywords: &[String]) -> bool {
    let value = value.trim();
    if value.is_empty() || keywords.is_empty() {
        return false;
    }
    for segment in value.split(',').map(str::trim) {
        let matches = keywords.iter().any(|keyword| {
            segment == keyword
                || segment.starts_with(&format!("{keyword} "))
                || segment.starts_with(&format!("{keyword}—"))
        });
        if !matches {
            return false;
        }
    }
    true
}

fn clause_criticality(value: &str) -> metagene_core::ClauseCriticality {
    let lower = value.to_ascii_lowercase();
    if [
        " instead",
        "copy",
        "gain control",
        "extra turn",
        "additional turn",
        "conjure",
        "spellbook",
        "perpetually",
        "seek ",
        "intensity",
        "double team",
        "cast",
        "costs",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        metagene_core::ClauseCriticality::RuleCritical
    } else {
        metagene_core::ClauseCriticality::OutcomeCritical
    }
}

fn card_mode(
    name: &str,
    type_line: &str,
    mana_cost: &str,
    colors: &[String],
    oracle: &str,
    keywords: &[String],
) -> metagene_core::CardMode {
    let lower_type = type_line.to_ascii_lowercase();
    let is_land = lower_type.contains("land");
    let parsed_cost = parse_mana_cost(mana_cost);
    let mana_sources = mana_source_profiles(name, oracle, is_land, &parsed_cost);
    let mut early = mana_sources
        .iter()
        .filter(|source| {
            source.earliest_turn <= 2
                && source.activation_generic == 0
                && source.spend_rule == metagene_core::ManaSpendRule::Unrestricted
                && source.color_rule == metagene_core::ManaColorRule::Fixed
        })
        .flat_map(|source| source.colors.iter().copied())
        .collect::<Vec<_>>();
    let mut late = mana_sources
        .iter()
        .filter(|source| source.earliest_turn > 2 || source.activation_generic > 0)
        .flat_map(|source| source.colors.iter().copied())
        .collect::<Vec<_>>();
    early.sort_by_key(|color| color_index(*color));
    early.dedup();
    late.sort_by_key(|color| color_index(*color));
    late.dedup();
    let mut clauses = oracle_clauses(name, oracle, keywords);
    let compatibility_seed = ability_profiles(name, &mut clauses);
    let rule_abilities = rule_abilities(&mut clauses, keywords, &compatibility_seed);
    let abilities = compatibility_profiles_from_rules(&rule_abilities);
    metagene_core::CardMode {
        name: name.into(),
        type_line: type_line.into(),
        colors: colors
            .iter()
            .filter_map(|color| parse_color(color))
            .collect(),
        mana_cost: parsed_cost,
        normal_castable: Some(is_land || !mana_cost.trim().is_empty()),
        is_land,
        is_legendary: lower_type.contains("legendary"),
        creature_types: type_line
            .split_once('—')
            .map(|(_, types)| {
                types
                    .split_whitespace()
                    .map(|value| value.to_ascii_lowercase())
                    .collect()
            })
            .unwrap_or_default(),
        mana_source_colors: early,
        late_mana_source_colors: late,
        mana_sources,
        abilities,
        rule_abilities,
        oracle_clauses: clauses,
        enters_tapped: is_land && oracle.to_ascii_lowercase().contains("enters tapped"),
    }
}

fn rule_abilities(
    clauses: &mut [metagene_core::OracleClause],
    keywords: &[String],
    compatibility: &[metagene_core::AbilityProfile],
) -> Vec<metagene_core::RuleAbility> {
    let mut rules = compatibility
        .iter()
        .enumerate()
        .map(|(index, ability)| rule_from_compatibility(index, ability))
        .collect::<Vec<_>>();

    for clause in clauses
        .iter_mut()
        .filter(|clause| clause.classification != metagene_core::ClauseClassification::ReminderText)
    {
        let already = rules
            .iter()
            .any(|rule| rule.clause_ids.iter().any(|id| id == &clause.id));
        let effects = typed_effects(&clause.text, keywords);
        if effects.is_empty() {
            continue;
        }
        if already {
            // Compatibility parsing found the broad action. Replace its
            // coarse effect list with ordered Rules-IR effects when the typed
            // parser has more exact information.
            let mut first = true;
            for rule in rules
                .iter_mut()
                .filter(|rule| rule.clause_ids.iter().any(|id| id == &clause.id))
            {
                if first {
                    rule.effects = effects.clone();
                    first = false;
                } else {
                    // One Oracle clause is one ordered effect sequence. Keeping
                    // one compatibility rule per coarse effect and then copying
                    // the full sequence onto each rule multiplied its value.
                    rule.effects.clear();
                }
            }
        } else {
            let id = format!("{}:rule:0", clause.id);
            clause.action_ids.push(id.clone());
            rules.push(metagene_core::RuleAbility {
                id,
                clause_ids: vec![clause.id.clone()],
                label: clause.text.clone(),
                timing: rule_timing(&clause.text),
                source_zones: vec![rule_source_zone(&clause.text)],
                costs: rule_costs(&clause.text),
                targets: rule_targets(&clause.text),
                conditions: rule_conditions(&clause.text),
                choices: modal_choices(&clause.text),
                effects,
                duration: rule_duration(&clause.text),
                delayed_effects: Vec::new(),
                once_per_turn: clause.normalized_text.contains("once each turn")
                    || clause.normalized_text.contains("once per turn"),
                once_per_use: false,
                parser_confidence: 1.0,
            });
        }
        clause.classification = if is_keyword_only_clause(
            &clause.normalized_text,
            &keywords
                .iter()
                .map(|keyword| keyword.to_ascii_lowercase())
                .collect::<Vec<_>>(),
        ) {
            metagene_core::ClauseClassification::RecognizedKeywordRule
        } else {
            metagene_core::ClauseClassification::ParsedGameplayRule
        };
        clause.parser_confidence = 1.0;
    }
    rules.sort_by_key(|rule| {
        rule.clause_ids
            .first()
            .and_then(|id| clauses.iter().find(|clause| &clause.id == id))
            .map_or(u16::MAX, |clause| clause.ordinal)
    });
    rules
}

fn rule_from_compatibility(
    index: usize,
    ability: &metagene_core::AbilityProfile,
) -> metagene_core::RuleAbility {
    let id = ability.clause_ids.first().map_or_else(
        || format!("legacy-rule:{index}"),
        |clause| format!("{clause}:rule:{index}"),
    );
    metagene_core::RuleAbility {
        id,
        clause_ids: ability.clause_ids.clone(),
        label: ability.label.clone(),
        timing: match ability.timing {
            metagene_core::AbilityTiming::Spell => metagene_core::RuleTiming::Spell,
            metagene_core::AbilityTiming::Activated => metagene_core::RuleTiming::Activation,
            metagene_core::AbilityTiming::Triggered => metagene_core::RuleTiming::Trigger,
            metagene_core::AbilityTiming::Static => {
                if ability.kind == metagene_core::AbilityKind::TokenMultiplier {
                    metagene_core::RuleTiming::Replacement
                } else {
                    metagene_core::RuleTiming::Static
                }
            }
        },
        source_zones: vec![ability.zone],
        costs: costs_from_compatibility(&ability.cost),
        targets: targets_from_compatibility(ability.target),
        conditions: ability
            .conditions
            .iter()
            .cloned()
            .map(metagene_core::RuleCondition::Text)
            .collect(),
        choices: Vec::new(),
        effects: effects_from_compatibility(ability.kind, &ability.label),
        duration: rule_duration(&ability.label),
        delayed_effects: Vec::new(),
        once_per_turn: ability.once_per_turn,
        once_per_use: false,
        parser_confidence: ability.parser_confidence,
    }
}

fn costs_from_compatibility(
    cost: &metagene_core::AbilityCostProfile,
) -> Vec<metagene_core::RuleCost> {
    let mut result = Vec::new();
    if cost.mana != metagene_core::ManaCostProfile::default() {
        result.push(metagene_core::RuleCost::Mana(cost.mana.clone()));
    }
    if cost.uses_tap {
        result.push(metagene_core::RuleCost::TapSource);
    }
    if cost.sacrifices_self || cost.sacrifice_permanents > 0 {
        result.push(metagene_core::RuleCost::Sacrifice {
            object: if cost.sacrifices_self {
                "this permanent"
            } else {
                "permanent"
            }
            .into(),
            amount: metagene_core::AmountExpression::Constant(i32::from(
                cost.sacrifice_permanents
                    .max(u8::from(cost.sacrifices_self)),
            )),
        });
    }
    if cost.discards_self || cost.discard_cards > 0 {
        result.push(metagene_core::RuleCost::Discard {
            object: if cost.discards_self {
                "this card"
            } else {
                "card"
            }
            .into(),
            amount: metagene_core::AmountExpression::Constant(i32::from(
                cost.discard_cards.max(u8::from(cost.discards_self)),
            )),
        });
    }
    if cost.exiles_self || cost.exile_cards > 0 {
        result.push(metagene_core::RuleCost::Exile {
            object: if cost.exiles_self {
                "this card"
            } else {
                "card"
            }
            .into(),
            amount: metagene_core::AmountExpression::Constant(i32::from(
                cost.exile_cards.max(u8::from(cost.exiles_self)),
            )),
        });
    }
    if cost.life > 0 {
        result.push(metagene_core::RuleCost::PayLife(
            metagene_core::AmountExpression::Constant(i32::from(cost.life)),
        ));
    }
    if cost.remove_counters > 0 {
        result.push(metagene_core::RuleCost::RemoveCounters(
            metagene_core::CounterProfile {
                name: "counter".into(),
                amount: metagene_core::AmountExpression::Constant(i32::from(cost.remove_counters)),
            },
        ));
    }
    result
}

fn targets_from_compatibility(
    target: metagene_core::ActionTargetProfile,
) -> Vec<metagene_core::RuleTarget> {
    use metagene_core::{AbilityZone, ActionTargetProfile as A, AmountExpression, RuleTarget};
    let (controller, zones, types, restrictions) = match target {
        A::None => return Vec::new(),
        A::OwnCreature => (
            "you",
            vec![AbilityZone::Battlefield],
            vec!["creature"],
            vec![],
        ),
        A::OpponentCreature => (
            "opponent",
            vec![AbilityZone::Battlefield],
            vec!["creature"],
            vec![],
        ),
        A::AnyCreature => (
            "any",
            vec![AbilityZone::Battlefield],
            vec!["creature"],
            vec![],
        ),
        A::ArtifactOrEnchantment => (
            "any",
            vec![AbilityZone::Battlefield],
            vec!["artifact", "enchantment"],
            vec![],
        ),
        A::GraveyardCard => ("any", vec![AbilityZone::Graveyard], vec![], vec![]),
        A::OwnGraveyardCreature => (
            "you",
            vec![AbilityZone::Graveyard],
            vec!["creature"],
            vec![],
        ),
        A::Spell => ("any", vec![AbilityZone::Stack], vec![], vec!["spell"]),
        A::BasicLandInLibrary => (
            "you",
            vec![AbilityZone::Library],
            vec!["land"],
            vec!["basic"],
        ),
        A::OpponentPlayer => ("opponent", vec![], vec![], vec!["player"]),
    };
    vec![RuleTarget {
        controller: controller.into(),
        zones,
        card_types: types.into_iter().map(str::to_string).collect(),
        subtypes: Vec::new(),
        restrictions: restrictions.into_iter().map(str::to_string).collect(),
        amount: AmountExpression::One,
        optional: false,
    }]
}

fn effects_from_compatibility(
    kind: metagene_core::AbilityKind,
    label: &str,
) -> Vec<metagene_core::RuleEffect> {
    use metagene_core::{
        AbilityKind as K, AbilityZone as Z, AmountExpression as A, RuleEffect as E, TokenProfile,
    };
    vec![match kind {
        K::Unknown => E::Text(label.into()),
        K::Draw => E::Draw {
            player: "you".into(),
            amount: A::One,
        },
        K::Destroy => E::Destroy {
            object: "target".into(),
        },
        K::Exile | K::GraveyardHate => E::Exile {
            object: "target".into(),
        },
        K::Bounce => E::MoveCard {
            from: Z::Battlefield,
            to: Z::Hand,
            object: "target".into(),
        },
        K::CounterSpell => E::MoveCard {
            from: Z::Stack,
            to: Z::Graveyard,
            object: "target spell".into(),
        },
        K::AddCounters => E::PutCounters(metagene_core::CounterProfile {
            name: "+1/+1".into(),
            amount: A::One,
        }),
        K::Recursion => E::MoveCard {
            from: Z::Graveyard,
            to: Z::Battlefield,
            object: "target".into(),
        },
        K::Search => E::Search {
            zone: Z::Library,
            destination: Z::Hand,
            object: "card".into(),
        },
        K::CreateToken => E::CreateToken {
            token: TokenProfile {
                name: "token".into(),
                ..Default::default()
            },
            amount: A::One,
        },
        K::Copy => E::Copy {
            object: "target".into(),
            as_token: label.to_ascii_lowercase().contains("token"),
        },
        K::TokenMultiplier => E::Replace {
            event: "create tokens".into(),
            replacement: label.into(),
        },
        K::SacrificeOutlet => E::Sacrifice {
            object: "creature".into(),
        },
        K::Ramp => E::AddMana {
            colors: Vec::new(),
            amount: A::One,
        },
        K::Crew => E::Text("crew".into()),
        K::Damage => E::Damage {
            recipient: "target".into(),
            amount: A::One,
        },
        K::Mill => E::Mill {
            player: "target".into(),
            amount: A::One,
        },
        K::AlternateWin => E::WinLoss {
            player: "you".into(),
            wins: true,
        },
        K::Blink => E::MoveCard {
            from: Z::Battlefield,
            to: Z::Exile,
            object: "target".into(),
        },
    }]
}

fn compatibility_profiles_from_rules(
    rules: &[metagene_core::RuleAbility],
) -> Vec<metagene_core::AbilityProfile> {
    // Compatibility summaries intentionally cover only effects represented by
    // the older hot-loop vocabulary. Rules IR remains the source of truth.
    rules
        .iter()
        .flat_map(|rule| {
            rule.effects.iter().filter_map(move |effect| {
                let kind = match effect {
                    metagene_core::RuleEffect::Draw { .. } => metagene_core::AbilityKind::Draw,
                    metagene_core::RuleEffect::Destroy { .. } => {
                        metagene_core::AbilityKind::Destroy
                    }
                    metagene_core::RuleEffect::Exile { .. }
                        if rule.targets.iter().any(|target| {
                            target
                                .zones
                                .contains(&metagene_core::AbilityZone::Graveyard)
                        }) =>
                    {
                        metagene_core::AbilityKind::GraveyardHate
                    }
                    metagene_core::RuleEffect::Exile { .. } => metagene_core::AbilityKind::Exile,
                    metagene_core::RuleEffect::MoveCard {
                        from: metagene_core::AbilityZone::Battlefield,
                        to: metagene_core::AbilityZone::Hand,
                        ..
                    } => metagene_core::AbilityKind::Bounce,
                    metagene_core::RuleEffect::MoveCard {
                        from: metagene_core::AbilityZone::Graveyard,
                        to:
                            metagene_core::AbilityZone::Hand
                            | metagene_core::AbilityZone::Battlefield,
                        ..
                    } => metagene_core::AbilityKind::Recursion,
                    metagene_core::RuleEffect::Search { .. } => metagene_core::AbilityKind::Search,
                    metagene_core::RuleEffect::CreateToken { .. } => {
                        metagene_core::AbilityKind::CreateToken
                    }
                    metagene_core::RuleEffect::Copy { .. } => metagene_core::AbilityKind::Copy,
                    metagene_core::RuleEffect::Replace { event, .. } if event.contains("token") => {
                        metagene_core::AbilityKind::TokenMultiplier
                    }
                    metagene_core::RuleEffect::Sacrifice { .. } => {
                        metagene_core::AbilityKind::SacrificeOutlet
                    }
                    metagene_core::RuleEffect::AddMana { .. } => metagene_core::AbilityKind::Ramp,
                    metagene_core::RuleEffect::Damage { .. } => metagene_core::AbilityKind::Damage,
                    metagene_core::RuleEffect::Mill { .. } => metagene_core::AbilityKind::Mill,
                    metagene_core::RuleEffect::WinLoss { wins: true, .. } => {
                        metagene_core::AbilityKind::AlternateWin
                    }
                    _ => return None,
                };
                Some(metagene_core::AbilityProfile {
                    clause_ids: rule.clause_ids.clone(),
                    label: rule.label.clone(),
                    kind,
                    timing: match rule.timing {
                        metagene_core::RuleTiming::Activation | metagene_core::RuleTiming::Mana => {
                            metagene_core::AbilityTiming::Activated
                        }
                        metagene_core::RuleTiming::Trigger => {
                            metagene_core::AbilityTiming::Triggered
                        }
                        metagene_core::RuleTiming::Static
                        | metagene_core::RuleTiming::Replacement => {
                            metagene_core::AbilityTiming::Static
                        }
                        _ => metagene_core::AbilityTiming::Spell,
                    },
                    zone: rule
                        .source_zones
                        .first()
                        .copied()
                        .unwrap_or(metagene_core::AbilityZone::Battlefield),
                    cost: compatibility_cost(&rule.costs),
                    target: compatibility_target(&rule.targets),
                    conditions: rule
                        .conditions
                        .iter()
                        .map(rule_condition_text)
                        .collect(),
                    effect_value: compatibility_effect_value(kind),
                    narrow: rule.targets.iter().any(|target| {
                        target.zones.iter().any(|zone| {
                            matches!(
                                zone,
                                metagene_core::AbilityZone::Graveyard
                                    | metagene_core::AbilityZone::Library
                            )
                        })
                    }) || rule
                        .label
                        .to_ascii_lowercase()
                        .contains("artifact or enchantment")
                        || rule.label.to_ascii_lowercase().contains("exiled with"),
                    once_per_turn: rule.once_per_turn,
                    parser_confidence: rule.parser_confidence,
                })
            })
        })
        .collect()
}

fn compatibility_cost(costs: &[metagene_core::RuleCost]) -> metagene_core::AbilityCostProfile {
    let mut result = metagene_core::AbilityCostProfile::default();
    for cost in costs {
        match cost {
            metagene_core::RuleCost::Mana(mana) => result.mana = mana.clone(),
            metagene_core::RuleCost::TapSource => result.uses_tap = true,
            metagene_core::RuleCost::Sacrifice { object, .. } => {
                result.sacrifices_self = object.contains("this")
            }
            metagene_core::RuleCost::Discard { object, .. } => {
                result.discards_self = object.contains("this")
            }
            metagene_core::RuleCost::Exile { object, .. } => {
                result.exiles_self = object.contains("this")
            }
            metagene_core::RuleCost::PayLife(metagene_core::AmountExpression::Constant(value)) => {
                result.life = (*value).try_into().unwrap_or(u8::MAX)
            }
            _ => {}
        }
    }
    result
}

fn compatibility_target(
    targets: &[metagene_core::RuleTarget],
) -> metagene_core::ActionTargetProfile {
    let Some(target) = targets.first() else {
        return metagene_core::ActionTargetProfile::None;
    };
    if target.restrictions.iter().any(|value| value == "spell") {
        return metagene_core::ActionTargetProfile::Spell;
    }
    if target.card_types.iter().any(|value| value == "artifact")
        && target.card_types.iter().any(|value| value == "enchantment")
    {
        return metagene_core::ActionTargetProfile::ArtifactOrEnchantment;
    }
    if target.card_types.iter().any(|value| value == "creature") {
        if target
            .zones
            .contains(&metagene_core::AbilityZone::Graveyard)
        {
            return metagene_core::ActionTargetProfile::OwnGraveyardCreature;
        }
        return if target.controller == "you" {
            metagene_core::ActionTargetProfile::OwnCreature
        } else {
            metagene_core::ActionTargetProfile::AnyCreature
        };
    }
    if target.zones.contains(&metagene_core::AbilityZone::Library)
        && target.card_types.iter().any(|value| value == "land")
        && target.restrictions.iter().any(|value| value == "basic")
    {
        return metagene_core::ActionTargetProfile::BasicLandInLibrary;
    }
    if target
        .zones
        .contains(&metagene_core::AbilityZone::Graveyard)
    {
        return metagene_core::ActionTargetProfile::GraveyardCard;
    }
    metagene_core::ActionTargetProfile::None
}

fn rule_condition_text(condition: &metagene_core::RuleCondition) -> String {
    match condition {
        metagene_core::RuleCondition::Always => "always".into(),
        metagene_core::RuleCondition::Event(value) => value.clone(),
        metagene_core::RuleCondition::InterveningIf(value) => format!("if {value}"),
        metagene_core::RuleCondition::AsLongAs(value) => format!("as long as {value}"),
        metagene_core::RuleCondition::Unless(value) => format!("unless {value}"),
        metagene_core::RuleCondition::Limit(value) | metagene_core::RuleCondition::Text(value) => {
            value.clone()
        }
    }
}

fn compatibility_effect_value(kind: metagene_core::AbilityKind) -> f32 {
    match kind {
        metagene_core::AbilityKind::TokenMultiplier => 1.20,
        metagene_core::AbilityKind::AlternateWin => 2.0,
        metagene_core::AbilityKind::Destroy
        | metagene_core::AbilityKind::Exile
        | metagene_core::AbilityKind::CounterSpell
        | metagene_core::AbilityKind::AddCounters
        | metagene_core::AbilityKind::Copy => 1.0,
        metagene_core::AbilityKind::Bounce | metagene_core::AbilityKind::Blink => 0.80,
        metagene_core::AbilityKind::Draw
        | metagene_core::AbilityKind::Recursion
        | metagene_core::AbilityKind::Damage
        | metagene_core::AbilityKind::Mill => 0.75,
        metagene_core::AbilityKind::Search | metagene_core::AbilityKind::Ramp => 0.65,
        metagene_core::AbilityKind::CreateToken | metagene_core::AbilityKind::SacrificeOutlet => {
            0.60
        }
        metagene_core::AbilityKind::Crew => 0.40,
        metagene_core::AbilityKind::GraveyardHate => 0.35,
        metagene_core::AbilityKind::Unknown => 0.10,
    }
}

fn rule_timing(text: &str) -> metagene_core::RuleTiming {
    let lower = text.to_ascii_lowercase();
    if lower.contains("would ") && lower.contains(" instead") {
        metagene_core::RuleTiming::Replacement
    } else if lower.starts_with("when ")
        || lower.starts_with("whenever ")
        || lower.starts_with("at ")
    {
        metagene_core::RuleTiming::Trigger
    } else if text.split_once(':').is_some() {
        metagene_core::RuleTiming::Activation
    } else if lower.starts_with("as long as ")
        || lower.starts_with("you may look")
        || lower.contains("can't be")
        || lower.contains("isn't a creature")
    {
        metagene_core::RuleTiming::Static
    } else {
        metagene_core::RuleTiming::Spell
    }
}

fn rule_source_zone(text: &str) -> metagene_core::AbilityZone {
    let lower = text.to_ascii_lowercase();
    if lower.contains("from your graveyard") || lower.starts_with("flashback") {
        metagene_core::AbilityZone::Graveyard
    } else if lower.contains("discard this card") || lower.starts_with("channel") {
        metagene_core::AbilityZone::Hand
    } else if rule_timing(text) == metagene_core::RuleTiming::Spell {
        metagene_core::AbilityZone::Stack
    } else {
        metagene_core::AbilityZone::Battlefield
    }
}

fn rule_costs(text: &str) -> Vec<metagene_core::RuleCost> {
    let Some((cost_text, _)) = text.split_once(':') else {
        return Vec::new();
    };
    let lower = cost_text.to_ascii_lowercase();
    let mut result = Vec::new();
    let mana = parse_mana_cost(cost_text);
    if mana != metagene_core::ManaCostProfile::default() {
        result.push(metagene_core::RuleCost::Mana(mana));
    }
    if cost_text.contains("{T}") {
        result.push(metagene_core::RuleCost::TapSource);
    }
    if lower.contains("sacrifice") {
        result.push(metagene_core::RuleCost::Sacrifice {
            object: lower
                .split_once("sacrifice")
                .map_or("permanent", |(_, value)| value.trim())
                .into(),
            amount: metagene_core::AmountExpression::One,
        });
    }
    if lower.contains("discard") {
        result.push(metagene_core::RuleCost::Discard {
            object: lower
                .split_once("discard")
                .map_or("card", |(_, value)| value.trim())
                .into(),
            amount: metagene_core::AmountExpression::One,
        });
    }
    if lower.contains("exile") {
        result.push(metagene_core::RuleCost::Exile {
            object: lower
                .split_once("exile")
                .map_or("card", |(_, value)| value.trim())
                .into(),
            amount: metagene_core::AmountExpression::One,
        });
    }
    if lower.contains("tap two untapped artifacts") {
        result.push(metagene_core::RuleCost::Additional(
            "tap two untapped artifacts you control".into(),
        ));
    }
    result
}

fn rule_targets(text: &str) -> Vec<metagene_core::RuleTarget> {
    let lower = text.to_ascii_lowercase();
    if !lower.contains("target ") {
        return Vec::new();
    }
    let mut target = metagene_core::RuleTarget {
        amount: metagene_core::AmountExpression::One,
        controller: if lower.contains("target creature you control") {
            "you".into()
        } else if lower.contains("target opponent") {
            "opponent".into()
        } else {
            "any".into()
        },
        zones: if lower.contains("graveyard") {
            vec![metagene_core::AbilityZone::Graveyard]
        } else if lower.contains("target spell") {
            vec![metagene_core::AbilityZone::Stack]
        } else {
            vec![metagene_core::AbilityZone::Battlefield]
        },
        ..Default::default()
    };
    for card_type in [
        "artifact",
        "creature",
        "enchantment",
        "land",
        "planeswalker",
        "battle",
    ] {
        if lower.contains(&format!("target {card_type}"))
            || lower.contains(&format!("artifact or {card_type}"))
        {
            target.card_types.push(card_type.into());
        }
    }
    if lower.contains("nonland permanent") {
        target.restrictions.push("nonland permanent".into());
    }
    if lower.contains("you control") {
        target.restrictions.push("you control".into());
    }
    vec![target]
}

fn rule_conditions(text: &str) -> Vec<metagene_core::RuleCondition> {
    let lower = text.to_ascii_lowercase();
    let mut result = Vec::new();
    if matches!(rule_timing(text), metagene_core::RuleTiming::Trigger) {
        let event = text.split(',').next().unwrap_or(text).trim();
        result.push(metagene_core::RuleCondition::Event(event.into()));
    }
    if let Some((_, condition)) = lower.split_once(" if ") {
        result.push(metagene_core::RuleCondition::InterveningIf(
            condition
                .split([',', '.'])
                .next()
                .unwrap_or(condition)
                .trim()
                .into(),
        ));
    }
    if let Some((_, condition)) = lower.split_once("as long as ") {
        result.push(metagene_core::RuleCondition::AsLongAs(
            condition
                .split([',', '.'])
                .next()
                .unwrap_or(condition)
                .trim()
                .into(),
        ));
    }
    if lower.contains("only once each turn") || lower.contains("once per turn") {
        result.push(metagene_core::RuleCondition::Limit("once per turn".into()));
    }
    result
}

fn modal_choices(text: &str) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    if lower.contains("choose one")
        || lower.contains("choose two")
        || lower.contains("choose any number")
    {
        text.split('•')
            .skip(1)
            .map(str::trim)
            .filter(|choice| !choice.is_empty())
            .map(str::to_string)
            .collect()
    } else {
        Vec::new()
    }
}

fn rule_duration(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    [
        "until end of turn",
        "for as long as",
        "during your next turn",
        "this turn",
    ]
    .into_iter()
    .find(|marker| lower.contains(marker))
    .map(str::to_string)
}

fn amount_from_text(text: &str, marker: &str) -> metagene_core::AmountExpression {
    let lower = text.to_ascii_lowercase();
    let tail = lower.split_once(marker).map_or("", |(_, tail)| tail.trim());
    let word = tail
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(|c: char| !c.is_ascii_alphanumeric());
    match word {
        "a" | "an" | "one" => metagene_core::AmountExpression::One,
        "two" => metagene_core::AmountExpression::Constant(2),
        "three" => metagene_core::AmountExpression::Constant(3),
        "four" => metagene_core::AmountExpression::Constant(4),
        "x" => metagene_core::AmountExpression::X,
        value => value.parse::<i32>().map_or(
            metagene_core::AmountExpression::One,
            metagene_core::AmountExpression::Constant,
        ),
    }
}

fn typed_effects(text: &str, keywords: &[String]) -> Vec<metagene_core::RuleEffect> {
    use metagene_core::{AbilityZone as Z, AmountExpression as A, RuleEffect as E, TokenProfile};
    let effect_text = oracle_effect_segment(text);
    let lower = effect_text.to_ascii_lowercase();
    let mut positioned = Vec::<(usize, E)>::new();
    let mut push = |needle: &str, effect: E| {
        if let Some(index) = lower.find(needle) {
            positioned.push((index, effect));
        }
    };
    if is_keyword_only_clause(
        &lower,
        &keywords
            .iter()
            .map(|value| value.to_ascii_lowercase())
            .collect::<Vec<_>>(),
    ) {
        for keyword in lower.split(',').map(str::trim) {
            push(
                keyword,
                E::GrantKeyword {
                    keyword: keyword.into(),
                },
            );
        }
    }
    push(
        "draw a card",
        E::Draw {
            player: "you".into(),
            amount: A::One,
        },
    );
    push(
        "draws an additional card",
        E::Draw {
            player: "active player".into(),
            amount: A::One,
        },
    );
    if lower.contains("gain") && lower.contains(" life") {
        push(
            "gain",
            E::Life {
                player: "you".into(),
                amount: amount_from_text(&lower, "gain"),
                gain: true,
            },
        );
    }
    if lower.contains("create") && lower.contains("token") {
        let amount = amount_from_text(&lower, "create");
        let mut token = TokenProfile {
            name: "token".into(),
            ..Default::default()
        };
        for subtype in [
            "treasure", "thopter", "food", "clue", "soldier", "spirit", "zombie",
        ] {
            if lower.contains(subtype) {
                token.name = subtype.into();
                token.subtypes.push(subtype.into());
            }
        }
        for card_type in ["artifact", "creature", "enchantment", "land"] {
            if lower.contains(card_type) {
                token.types.push(card_type.into());
            }
        }
        token.copy_of_target = lower.contains("copy of");
        push("create", E::CreateToken { token, amount });
        if lower.contains("copy of") {
            push(
                "copy of",
                E::Copy {
                    object: "target".into(),
                    as_token: true,
                },
            );
        }
    }
    if lower.contains("copy") && !lower.contains("create") {
        push(
            "copy",
            E::Copy {
                object: "target".into(),
                as_token: lower.contains("token"),
            },
        );
    }
    if lower.contains("would create") && lower.contains("instead") {
        push(
            "would create",
            E::Replace {
                event: "create tokens".into(),
                replacement: text.into(),
            },
        );
    }
    if lower.contains("destroy target") {
        push(
            "destroy target",
            E::Destroy {
                object: "target".into(),
            },
        );
    }
    if lower.contains("return target") && lower.contains("hand") {
        let from = if lower.contains("graveyard") {
            Z::Graveyard
        } else {
            Z::Battlefield
        };
        push(
            "return target",
            E::MoveCard {
                from,
                to: Z::Hand,
                object: "target".into(),
            },
        );
    }
    if lower.contains("exile target") {
        push(
            "exile target",
            E::Exile {
                object: "target".into(),
            },
        );
    }
    if lower.contains("exile") && lower.contains("return") && lower.contains("battlefield") {
        push(
            "exile",
            E::MoveCard {
                from: Z::Battlefield,
                to: Z::Exile,
                object: "target".into(),
            },
        );
        push(
            "return",
            E::MoveCard {
                from: Z::Exile,
                to: Z::Battlefield,
                object: "that card".into(),
            },
        );
    }
    if lower.contains("perpetually") {
        push(
            "perpetually",
            E::Perpetual {
                modification: text.into(),
            },
        );
    }
    if lower.contains("can't be blocked") || lower.contains("cannot be blocked") {
        push(
            "can't be blocked",
            E::GrantKeyword {
                keyword: "unblockable".into(),
            },
        );
    }
    if lower.contains("tap target") || lower.contains("tap another target") {
        push("tap", E::Tap { untap: false });
    }
    if lower.contains("untap target") {
        push("untap target", E::Tap { untap: true });
    }
    if lower.contains("add {") || lower.contains("add one mana") {
        let colors = fixed_add_mana_colors(text);
        push(
            "add",
            E::AddMana {
                colors,
                amount: A::One,
            },
        );
    }
    if lower.contains("deals") && lower.contains("damage") {
        push(
            "deals",
            E::Damage {
                recipient: "target".into(),
                amount: A::One,
            },
        );
    }
    if lower.contains("mill") {
        push(
            "mill",
            E::Mill {
                player: "target".into(),
                amount: amount_from_text(&lower, "mill"),
            },
        );
    }
    if lower.contains("transform") {
        push("transform", E::Transform);
    }
    if lower.contains("isn't a creature") || lower.contains("is not a creature") {
        push(
            "creature",
            E::ModifyTypes {
                add: Vec::new(),
                remove: vec!["creature".into()],
            },
        );
    }
    if lower.contains("becomes a creature") {
        push(
            "becomes",
            E::ModifyTypes {
                add: vec!["creature".into()],
                remove: Vec::new(),
            },
        );
    }
    if lower.contains("gain control") {
        push(
            "gain control",
            E::ControlChange {
                object: "target".into(),
                duration: rule_duration(text).unwrap_or_else(|| "indefinite".into()),
            },
        );
    }
    if lower.contains("extra turn") || lower.contains("additional turn") {
        push("turn", E::ExtraTurn);
    }
    if lower.contains("win the game") {
        push(
            "win the game",
            E::WinLoss {
                player: "you".into(),
                wins: true,
            },
        );
    }
    if lower.contains("conjure") {
        push(
            "conjure",
            E::Conjure {
                object: text.into(),
                destination: Z::Hand,
            },
        );
    }
    if lower.contains("seek") {
        push(
            "seek",
            E::Seek {
                object: text.into(),
            },
        );
    }
    positioned.sort_by_key(|(index, _)| *index);
    positioned.into_iter().map(|(_, effect)| effect).collect()
}

fn oracle_effect_segment(text: &str) -> &str {
    let lower = text.trim_start().to_ascii_lowercase();
    // Replacement clauses need their antecedent as well as their result. If
    // the token-creation event is stripped from a doubler, package detection
    // sees an effect but loses what that effect replaces.
    if lower.starts_with("if ") && lower.contains(" instead") {
        return text;
    }
    let trigger_start = ["whenever ", "when ", "at ", "if "]
        .iter()
        .filter_map(|marker| {
            lower.find(marker).filter(|start| {
                // A trigger may be the clause itself or an ability granted in
                // quotation marks. An unquoted mid-clause conditional (for
                // example Mythos of Illuna's optional fight rider) is an
                // ordered effect and must not erase the effect before it.
                let prefix = &text[..*start];
                *start == 0
                    || prefix.contains('"')
                    || prefix.contains('‘')
                    || prefix.contains('“')
                    || prefix.to_ascii_lowercase().contains(" has ")
            })
        })
        .min();
    if let Some(start) = trigger_start
        && let Some(comma) = text[start..].find(',')
    {
        return text[start + comma + 1..].trim_start();
    }
    text
}

fn fixed_add_mana_colors(text: &str) -> Vec<Color> {
    let Some((_, mana_text)) = text.split_once("Add").or_else(|| text.split_once("add")) else {
        return Vec::new();
    };
    if mana_text.to_ascii_lowercase().contains("any color") {
        return Vec::new();
    }
    let mut colors = Vec::new();
    let mut remaining = mana_text;
    while let Some(open) = remaining.find('{') {
        remaining = &remaining[open + 1..];
        let Some(close) = remaining.find('}') else {
            break;
        };
        if let Some(color) = parse_color(&remaining[..close].to_ascii_uppercase())
            && !colors.contains(&color)
        {
            colors.push(color);
        }
        remaining = &remaining[close + 1..];
    }
    colors
}

fn ability_profiles(
    _name: &str,
    clauses: &mut [metagene_core::OracleClause],
) -> Vec<metagene_core::AbilityProfile> {
    let mut abilities = Vec::new();
    for clause in clauses.iter_mut().filter(|clause| {
        !matches!(
            clause.classification,
            metagene_core::ClauseClassification::ReminderText
                | metagene_core::ClauseClassification::RecognizedKeywordRule
        )
    }) {
        let line = clause.text.trim();
        let lower = line.to_ascii_lowercase();
        let activated = line.split_once(':').filter(|(cost, _)| {
            cost.contains('{')
                || cost.to_ascii_lowercase().contains("sacrifice")
                || cost.to_ascii_lowercase().contains("discard")
                || cost.to_ascii_lowercase().contains("pay ")
                || cost.to_ascii_lowercase().contains("remove ")
        });
        let is_activated = activated.is_some();
        let (cost_text, effect_text, timing) = if let Some((cost, effect)) = activated {
            (cost, effect.trim(), metagene_core::AbilityTiming::Activated)
        } else {
            let timing = if lower.starts_with("when ")
                || lower.starts_with("whenever ")
                || lower.starts_with("at ")
                || lower.contains(" — when ")
                || lower.contains("whenever ")
            {
                metagene_core::AbilityTiming::Triggered
            } else if lower.contains("would create") && lower.contains("instead") {
                metagene_core::AbilityTiming::Static
            } else {
                metagene_core::AbilityTiming::Spell
            };
            ("", oracle_effect_segment(line), timing)
        };
        let effect = effect_text.to_ascii_lowercase();
        let mut kinds = Vec::new();
        {
            let mut add_kind = |kind| {
                if !kinds.contains(&kind) {
                    kinds.push(kind);
                }
            };
            if effect.contains("win the game") {
                add_kind(metagene_core::AbilityKind::AlternateWin);
            } else if effect.contains("would create")
                && effect.contains("token")
                && (effect.contains("twice") || effect.contains("additional"))
            {
                add_kind(metagene_core::AbilityKind::TokenMultiplier);
            }
            if effect.contains("counter target spell") {
                add_kind(metagene_core::AbilityKind::CounterSpell);
            }
            if (effect.contains("put ") && effect.contains(" counter"))
                || effect.contains("proliferate")
            {
                add_kind(metagene_core::AbilityKind::AddCounters);
            }
            if (effect.contains("return target") && effect.contains("graveyard"))
                || (effect.contains("creature card exiled with")
                    && effect.contains("onto the battlefield"))
            {
                add_kind(metagene_core::AbilityKind::Recursion);
            }
            if effect.contains("exile target") && effect.contains("graveyard") {
                add_kind(metagene_core::AbilityKind::GraveyardHate);
            }
            if effect.contains("destroy target") {
                add_kind(metagene_core::AbilityKind::Destroy);
            }
            if effect.contains("exile target") {
                add_kind(metagene_core::AbilityKind::Exile);
            }
            if effect.contains("return target")
                && (effect.contains("owner's hand") || effect.contains("owners' hands"))
            {
                add_kind(metagene_core::AbilityKind::Bounce);
            }
            if effect.contains("draw a card")
                || effect.contains("draw cards")
                || effect.contains("draws an additional card")
            {
                add_kind(metagene_core::AbilityKind::Draw);
            }
            if effect.contains("search your library") {
                add_kind(metagene_core::AbilityKind::Search);
            }
            if effect.contains("token that's a copy")
                || effect.contains("token that is a copy")
                || effect.contains("becomes a copy")
                || effect.contains("copy of target")
                || effect.contains("enter as a copy")
            {
                add_kind(metagene_core::AbilityKind::Copy);
            }
            if effect.contains("create") && effect.contains("token") {
                add_kind(metagene_core::AbilityKind::CreateToken);
            }
            if effect.contains("exile")
                && effect.contains("return")
                && effect.contains("battlefield")
            {
                add_kind(metagene_core::AbilityKind::Blink);
            }
            if effect.contains("sacrifice a creature")
                || effect.contains("sacrifice another creature")
            {
                add_kind(metagene_core::AbilityKind::SacrificeOutlet);
            }
            if effect.contains("deals") && effect.contains("damage") {
                add_kind(metagene_core::AbilityKind::Damage);
            }
            if effect.contains("mill") {
                add_kind(metagene_core::AbilityKind::Mill);
            }
            if effect.contains("add {") || effect.contains("add one mana") {
                add_kind(metagene_core::AbilityKind::Ramp);
            }
            if effect.contains("crew ") {
                add_kind(metagene_core::AbilityKind::Crew);
            }
        }
        if kinds.is_empty() && is_activated {
            kinds.push(metagene_core::AbilityKind::Unknown);
        }
        if kinds.is_empty() {
            continue;
        }
        let target = if effect.contains("artifact or enchantment") {
            metagene_core::ActionTargetProfile::ArtifactOrEnchantment
        } else if effect.contains("target creature card") && effect.contains("your graveyard") {
            metagene_core::ActionTargetProfile::OwnGraveyardCreature
        } else if effect.contains("target card") && effect.contains("graveyard") {
            metagene_core::ActionTargetProfile::GraveyardCard
        } else if effect.contains("target creature you control") {
            metagene_core::ActionTargetProfile::OwnCreature
        } else if effect.contains("target creature") {
            metagene_core::ActionTargetProfile::AnyCreature
        } else if effect.contains("target spell") {
            metagene_core::ActionTargetProfile::Spell
        } else if effect.contains("basic land") && effect.contains("library") {
            metagene_core::ActionTargetProfile::BasicLandInLibrary
        } else if effect.contains("target opponent") {
            metagene_core::ActionTargetProfile::OpponentPlayer
        } else {
            metagene_core::ActionTargetProfile::None
        };
        let target_narrow = matches!(
            target,
            metagene_core::ActionTargetProfile::ArtifactOrEnchantment
                | metagene_core::ActionTargetProfile::GraveyardCard
                | metagene_core::ActionTargetProfile::OwnGraveyardCreature
                | metagene_core::ActionTargetProfile::BasicLandInLibrary
        ) || effect.contains("exiled with");
        let lower_cost = cost_text.to_ascii_lowercase();
        let zone = if timing == metagene_core::AbilityTiming::Spell {
            metagene_core::AbilityZone::Stack
        } else if lower_cost.contains("discard this") {
            metagene_core::AbilityZone::Hand
        } else if lower_cost.contains("from your graveyard")
            || lower.contains("exile this card from your graveyard")
        {
            metagene_core::AbilityZone::Graveyard
        } else {
            metagene_core::AbilityZone::Battlefield
        };
        let mut conditions = Vec::new();
        if timing == metagene_core::AbilityTiming::Triggered {
            conditions.push(
                effect_text
                    .split_once(',')
                    .map_or(effect_text, |(condition, _)| condition)
                    .trim()
                    .to_string(),
            );
        }
        for marker in ["if ", "as long as ", "only if ", "for each "] {
            if let Some((_, suffix)) = lower.split_once(marker) {
                let condition = suffix.split([',', '.']).next().unwrap_or(suffix).trim();
                if !condition.is_empty() {
                    conditions.push(format!("{marker}{condition}"));
                }
            }
        }
        if effect.contains("exiled with") {
            conditions.push("requires creature cards previously exiled with this permanent".into());
        }
        conditions.sort();
        conditions.dedup();
        for (effect_index, kind) in kinds.into_iter().enumerate() {
            let effect_value = match kind {
                metagene_core::AbilityKind::Unknown => 0.10,
                metagene_core::AbilityKind::Draw => 0.75,
                metagene_core::AbilityKind::Destroy
                | metagene_core::AbilityKind::Exile
                | metagene_core::AbilityKind::CounterSpell
                | metagene_core::AbilityKind::AddCounters => 1.0,
                metagene_core::AbilityKind::Bounce => 0.80,
                metagene_core::AbilityKind::GraveyardHate => 0.35,
                metagene_core::AbilityKind::Recursion => 0.75,
                metagene_core::AbilityKind::Search | metagene_core::AbilityKind::Ramp => 0.65,
                metagene_core::AbilityKind::CreateToken => 0.60,
                metagene_core::AbilityKind::Copy => 1.0,
                metagene_core::AbilityKind::TokenMultiplier => 1.20,
                metagene_core::AbilityKind::SacrificeOutlet => 0.60,
                metagene_core::AbilityKind::Crew => 0.40,
                metagene_core::AbilityKind::Damage | metagene_core::AbilityKind::Mill => 0.75,
                metagene_core::AbilityKind::AlternateWin => 2.0,
                metagene_core::AbilityKind::Blink => 0.80,
            };
            let action_id = format!("{}:{effect_index}", clause.id);
            clause.action_ids.push(action_id);
            abilities.push(metagene_core::AbilityProfile {
                clause_ids: vec![clause.id.clone()],
                label: effect_text.trim().to_string(),
                kind,
                timing,
                zone,
                cost: metagene_core::AbilityCostProfile {
                    mana: parse_mana_cost(cost_text),
                    uses_tap: cost_text.contains("{T}"),
                    sacrifices_self: lower_cost.contains("sacrifice this")
                        || (lower_cost.contains("sacrifice ")
                            && !lower_cost.contains("sacrifice a ")
                            && !lower_cost.contains("sacrifice another ")),
                    sacrifice_permanents: u8::from(
                        lower_cost.contains("sacrifice a ")
                            || lower_cost.contains("sacrifice another "),
                    ),
                    discards_self: lower_cost.contains("discard this")
                        || (lower_cost.contains("discard ")
                            && !lower_cost.contains("discard a card")
                            && !lower_cost.contains("discard two cards")),
                    discard_cards: if lower_cost.contains("discard two cards") {
                        2
                    } else {
                        u8::from(lower_cost.contains("discard a card"))
                    },
                    exiles_self: lower_cost.contains("exile this"),
                    exile_cards: u8::from(
                        lower_cost.contains("exile a card")
                            || lower_cost.contains("exile target card")
                            || lower_cost.contains("exile a creature card"),
                    ),
                    life: parse_number_after(&lower_cost, "pay ")
                        .filter(|_| lower_cost.contains(" life"))
                        .unwrap_or(0),
                    remove_counters: parse_number_after(&lower_cost, "remove ").unwrap_or(0),
                },
                target,
                conditions: conditions.clone(),
                effect_value,
                narrow: target_narrow || kind == metagene_core::AbilityKind::GraveyardHate,
                once_per_turn: lower.contains("only once each turn")
                    || lower.contains("once per turn"),
                parser_confidence: if kind == metagene_core::AbilityKind::Unknown {
                    0.10
                } else {
                    1.0
                },
            });
        }
        clause.classification = metagene_core::ClauseClassification::ParsedGameplayRule;
        clause.parser_confidence = abilities
            .iter()
            .filter(|ability| ability.clause_ids.contains(&clause.id))
            .map(|ability| ability.parser_confidence)
            .fold(1.0_f32, f32::min);
    }
    abilities
}

fn parse_number_after(value: &str, needle: &str) -> Option<u8> {
    value
        .split_once(needle)?
        .1
        .split_whitespace()
        .next()?
        .trim_matches(|character: char| !character.is_ascii_digit())
        .parse()
        .ok()
}

fn parse_mana_cost(value: &str) -> metagene_core::ManaCostProfile {
    let mut result = metagene_core::ManaCostProfile::default();
    for token in value
        .split('{')
        .skip(1)
        .filter_map(|part| part.split('}').next())
    {
        if let Ok(generic) = token.parse::<u8>() {
            result.generic = result.generic.saturating_add(generic);
            continue;
        }
        if matches!(token, "X" | "Y" | "Z") {
            result.variable = true;
            continue;
        }
        let options = token.split('/').filter_map(parse_color).collect::<Vec<_>>();
        if options.len() > 1 || token.contains('/') {
            if !options.is_empty() {
                result.hybrid.push(options);
            }
        } else if let Some(color) = options.first().copied() {
            result.colored[color_index(color)] =
                result.colored[color_index(color)].saturating_add(1);
        }
    }
    result
}

fn color_index(color: Color) -> usize {
    match color {
        Color::White => 0,
        Color::Blue => 1,
        Color::Black => 2,
        Color::Red => 3,
        Color::Green => 4,
        Color::Colorless => 5,
    }
}

fn mana_source_profiles(
    name: &str,
    oracle: &str,
    is_land: bool,
    cost: &metagene_core::ManaCostProfile,
) -> Vec<metagene_core::ManaSourceProfile> {
    let cast_turn = cost
        .generic
        .saturating_add(cost.colored.iter().copied().sum::<u8>())
        .saturating_add(cost.hybrid.len() as u8)
        .max(1);
    let mut profiles = Vec::new();
    for raw_line in oracle.lines() {
        // Reminder text can contain another object's mana ability (Treasure is
        // the common offender). It is not an ability of this card. Likewise,
        // triggered mana rewards are not reusable mana sources.
        let line = separate_reminder_text(raw_line).0.trim().to_string();
        let lower_line = line.to_ascii_lowercase();
        let Some(colon) = line.find(':') else {
            continue;
        };
        let before_add = &line[..colon];
        let effect = line[colon + 1..].trim_start();
        if !effect.to_ascii_lowercase().starts_with("add ")
            || lower_line.contains("activate only if")
        {
            continue;
        }
        let activation_generic = before_add
            .split('{')
            .skip(1)
            .filter_map(|part| part.split('}').next())
            .filter_map(|token| token.parse::<u8>().ok())
            .sum();
        let mut source_colors = Vec::new();
        let legendary_color_rule = lower_line.contains("among legendary permanents")
            || lower_line.contains("among legendary creatures and planeswalkers");
        if lower_line.contains("any color")
            || lower_line.contains("your commander's color identity")
            || legendary_color_rule
        {
            source_colors.extend([
                Color::White,
                Color::Blue,
                Color::Black,
                Color::Red,
                Color::Green,
            ]);
        }
        let after_add = effect.get(3..).unwrap_or(effect);
        for token in after_add
            .split('{')
            .skip(1)
            .filter_map(|part| part.split('}').next())
            .filter_map(parse_color)
        {
            source_colors.push(token);
        }
        source_colors.sort_by_key(|color| color_index(*color));
        source_colors.dedup();
        let color_rule = if lower_line.contains("your commander's color identity") {
            metagene_core::ManaColorRule::CommanderIdentity
        } else if legendary_color_rule {
            metagene_core::ManaColorRule::LegendaryPermanents
        } else {
            metagene_core::ManaColorRule::Fixed
        };
        let spend_rule = if lower_line.contains("spend this mana only") {
            if lower_line.contains("legendary spell") {
                metagene_core::ManaSpendRule::CastLegendary
            } else if lower_line.contains("creature spell of the chosen type") {
                metagene_core::ManaSpendRule::CastChosenCreatureType
            } else if lower_line.contains("pilot or vehicle") {
                metagene_core::ManaSpendRule::CastSubtypes(vec!["pilot".into(), "vehicle".into()])
            } else if lower_line.contains("ally spell") {
                metagene_core::ManaSpendRule::CastSubtypes(vec!["ally".into()])
            } else if lower_line.contains("villain spell") {
                metagene_core::ManaSpendRule::CastSubtypes(vec!["villain".into()])
            } else if lower_line.contains("dragon spell") && lower_line.contains("omen spell") {
                metagene_core::ManaSpendRule::CastSubtypes(vec!["dragon".into(), "omen".into()])
            } else {
                metagene_core::ManaSpendRule::UnknownRestricted
            }
        } else {
            metagene_core::ManaSpendRule::Unrestricted
        };
        let state_rule = if before_add
            .to_ascii_lowercase()
            .contains("remove a charge counter")
        {
            metagene_core::ManaStateRule::RemoveCounter
        } else {
            metagene_core::ManaStateRule::None
        };
        let state_delay = u8::from(state_rule == metagene_core::ManaStateRule::RemoveCounter);
        profiles.push(metagene_core::ManaSourceProfile {
            colors: source_colors,
            color_rule,
            spend_rule,
            state_rule,
            uses_tap: before_add.contains("{T}"),
            activation_generic,
            earliest_turn: if is_land && activation_generic == 0 {
                1 + state_delay
            } else {
                cast_turn
                    .saturating_add(activation_generic)
                    .saturating_add(u8::from(!is_land))
            },
            label: name.into(),
        });
    }
    profiles
}

fn mechanic_profile(text: &str, type_line: &str) -> metagene_core::MechanicProfile {
    let crew_power = text
        .split("crew ")
        .nth(1)
        .and_then(|tail| tail.split_whitespace().next())
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let searches_basic_land = text.contains("search your library for a basic land");
    let search_action = searches_basic_land.then(|| {
        let prefix = text.split("discard this card").next().unwrap_or_default();
        metagene_core::SearchActionProfile {
            mana_cost: parse_mana_cost(prefix),
            from_hand: text.contains("discard this card"),
            discards_self: text.contains("discard this card"),
            target: if text.contains("or a desert") {
                metagene_core::SearchTargetProfile::BasicLandOrSubtypes(vec!["desert".into()])
            } else {
                metagene_core::SearchTargetProfile::BasicLand
            },
        }
    });
    let is_creature = type_line.contains("creature");
    metagene_core::MechanicProfile {
        requires_creature: type_line.contains("aura") && text.contains("enchant creature"),
        requires_humans: text.contains("humans you control"),
        requires_death: text.contains("dies") || text.contains("creature die"),
        provides_death: text.contains("sacrifice")
            && !text.contains("exile a creature you control"),
        exile_cost: text.contains("exile a creature you control"),
        reanimation: text.contains("creature card")
            && text.contains("graveyard")
            && text.contains("battlefield"),
        graveyard_setup: text.contains("mill") || text.contains("discard"),
        legendary_payoff: text.contains("whenever you cast a legendary")
            || text.contains("legendary spell"),
        crew_power,
        devotion_threshold: if text.contains("devotion") { 7 } else { 0 },
        devotion_color_mask: if text.contains("devotion") {
            ["white", "blue", "black", "red", "green"]
                .iter()
                .enumerate()
                .fold(0_u8, |mask, (slot, color)| {
                    mask | (u8::from(text.contains(color)) << slot)
                })
        } else {
            0
        },
        searches_basic_land,
        search_action,
        opponent_discard_supply: u8::from(
            text.contains("target opponent discards") || text.contains("each opponent discards"),
        ),
        opponent_discard_demand: u8::from(text.contains("whenever an opponent discards")),
        sacrifice_outlet_supply: u8::from(
            text.contains("sacrifice a creature:")
                || text.contains("sacrifice another creature:")
                || text.contains("as an additional cost to cast this spell, you may sacrifice")
                || text.contains("as an additional cost to cast this spell, sacrifice"),
        ),
        sacrifice_fodder_supply: u8::from(
            text.contains("create") && text.contains("creature token"),
        ),
        sacrifice_demand: u8::from(
            text.contains("whenever you sacrifice") || text.contains("if you sacrificed"),
        ),
        graveyard_target_supply: u8::from(is_creature),
        graveyard_target_demand: u8::from(
            text.contains("card from your graveyard") || text.contains("cards from your graveyard"),
        ),
        graveyard_setup_supply: u8::from(
            text.contains("mill")
                || text.contains("discard")
                || text.contains("sacrifice a creature"),
        ),
        graveyard_setup_demand: u8::from(
            text.contains("card from your graveyard") || text.contains("cards from your graveyard"),
        ),
        legendary_cast_demand: u8::from(
            text.contains("whenever you cast a legendary") || text.contains("legendary spell"),
        ),
    }
}

fn decode_card(json: &str) -> Result<Card, serde_json::Error> {
    let mut card: Card = serde_json::from_str(json)?;
    card.enrich_derived_tags();
    Ok(card)
}

fn parse_color(color: &str) -> Option<Color> {
    match color {
        "W" => Some(Color::White),
        "U" => Some(Color::Blue),
        "B" => Some(Color::Black),
        "R" => Some(Color::Red),
        "G" => Some(Color::Green),
        "C" => Some(Color::Colorless),
        _ => None,
    }
}

fn catalog_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CATALOG_FILE)
}

fn progress(
    stage: &str,
    processed: usize,
    retained: usize,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
) -> CatalogProgress {
    CatalogProgress {
        stage: stage.into(),
        processed,
        retained,
        downloaded_bytes,
        total_bytes,
    }
}

#[cfg(test)]
mod structural_mode_tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use metagene_core::{AbilityKind, AbilityZone};
    use serde_json::json;
    use std::io::Write;

    fn convert(value: serde_json::Value) -> Card {
        into_card(serde_json::from_value(value).expect("valid fixture"))
    }

    fn base(name: &str, layout: &str, faces: serde_json::Value) -> serde_json::Value {
        json!({
            "id": name, "name": name, "set": "tst", "collector_number": "1",
            "layout": layout, "cmc": 3.0, "games": ["arena"], "legalities": {},
            "color_identity": [], "card_faces": faces
        })
    }

    #[test]
    fn oracle_accounting_separates_reminder_colons_and_never_drops_residuals() {
        use metagene_core::ClauseClassification;
        let card = convert(json!({
            "id":"truth-gate", "name":"Truth Gate Fixture", "set":"tst", "collector_number":"1",
            "type_line":"Creature — Test", "mana_cost":"{1}{G}",
            "oracle_text":"When this creature enters, create a Treasure token. (It's an artifact with \"{T}, Sacrifice this artifact: Add one mana of any color.\")\nMoonlight arithmetic applies to this creature.",
            "games":["arena"], "legalities":{}, "color_identity":["G"], "colors":["G"]
        }));
        let mode = &card.features.initial_modes[0];
        assert!(mode.oracle_clauses.iter().any(|clause| {
            clause.classification == ClauseClassification::ReminderText
                && clause.text.contains("Sacrifice this artifact:")
        }));
        assert!(mode.oracle_clauses.iter().any(|clause| {
            clause.classification == ClauseClassification::UnresolvedGameplayResidual
                && clause.text == "Moonlight arithmetic applies to this creature."
        }));
        assert!(
            !mode
                .abilities
                .iter()
                .any(|ability| { ability.label.contains("Sacrifice this artifact:") })
        );
        assert!(card.features.oracle_coverage.accounted_clause_rate >= 1.0);
        assert!(card.features.oracle_coverage.unresolved_clause_rate > 0.0);
    }

    #[test]
    fn one_oracle_clause_can_emit_multiple_ordered_functional_actions() {
        let card = convert(json!({
            "id":"multi-action", "name":"Multi Action Fixture", "set":"tst", "collector_number":"1",
            "type_line":"Sorcery", "mana_cost":"{2}{U}",
            "oracle_text":"Return target creature to its owner's hand. Draw a card.",
            "games":["arena"], "legalities":{}, "color_identity":["U"], "colors":["U"]
        }));
        let mode = &card.features.initial_modes[0];
        assert!(
            mode.abilities
                .iter()
                .any(|ability| ability.kind == AbilityKind::Bounce)
        );
        assert!(
            mode.abilities
                .iter()
                .any(|ability| ability.kind == AbilityKind::Draw)
        );
        let clause = &mode.oracle_clauses[0];
        assert_eq!(clause.action_ids.len(), 2);
        assert!(
            mode.abilities
                .iter()
                .all(|ability| ability.clause_ids == vec![clause.id.clone()])
        );
    }

    #[test]
    fn catalog_schema_and_merge_keep_one_logical_card_name() {
        let connection = Connection::open_in_memory().unwrap();
        initialize(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO cards (id, name, card_json) VALUES ('one', 'Lightning Bolt', '{}')",
                [],
            )
            .unwrap();
        assert!(
            connection
                .execute(
                    "INSERT INTO cards (id, name, card_json) VALUES ('two', 'LIGHTNING BOLT', '{}')",
                    [],
                )
                .is_err()
        );

        let mut paper = convert(json!({
            "id":"paper-bolt", "name":"Lightning Bolt", "set":"lea", "collector_number":"161",
            "type_line":"Instant", "mana_cost":"{R}", "oracle_text":"Lightning Bolt deals 3 damage to any target.",
            "games":["paper"], "legalities":{"modern":"legal"}, "color_identity":["R"], "colors":["R"]
        }));
        let arena = convert(json!({
            "id":"arena-bolt", "arena_id":777, "name":"Lightning Bolt", "set":"sta", "collector_number":"42",
            "type_line":"Instant", "mana_cost":"{R}", "oracle_text":"Lightning Bolt deals 3 damage to any target.",
            "games":["arena"], "legalities":{"historic":"legal"}, "color_identity":["R"], "colors":["R"]
        }));
        merge_logical_printing(&mut paper, arena, None);
        assert_eq!(paper.id.0, "arena-bolt");
        assert_eq!(paper.arena_id, Some(777));
        assert_eq!(paper.platforms, vec!["arena", "paper"]);
        assert_eq!(
            paper.legalities.get("modern").map(String::as_str),
            Some("legal")
        );
        assert_eq!(
            paper.legalities.get("historic").map(String::as_str),
            Some("legal")
        );
    }

    #[test]
    fn catalog_rebuild_preserves_the_previous_representative_id() {
        let mut previous = convert(json!({
            "id":"saved-run-id", "name":"Opt", "set":"dom", "collector_number":"60",
            "type_line":"Instant", "mana_cost":"{U}", "oracle_text":"Scry 1. Draw a card.",
            "games":["paper","arena"], "legalities":{"historic":"legal"}, "color_identity":["U"], "colors":["U"]
        }));
        let newer_arena = convert(json!({
            "id":"new-printing", "arena_id":999, "name":"Opt", "set":"new", "collector_number":"1",
            "type_line":"Instant", "mana_cost":"{U}", "oracle_text":"Scry 1. Draw a card.",
            "games":["arena"], "legalities":{"historic":"legal"}, "color_identity":["U"], "colors":["U"]
        }));
        merge_logical_printing(&mut previous, newer_arena, Some("saved-run-id"));
        assert_eq!(previous.id.0, "saved-run-id");
        assert!(previous.platforms.contains(&"arena".into()));
    }

    #[test]
    fn printing_aliases_migrate_saved_deck_ids_to_the_logical_card() {
        let directory =
            std::env::temp_dir().join(format!("metagene-catalog-alias-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let logical = convert(json!({
            "id":"logical-opt", "arena_id":123, "name":"Opt", "set":"dom", "collector_number":"60",
            "type_line":"Instant", "mana_cost":"{U}", "oracle_text":"Scry 1. Draw a card.",
            "games":["arena"], "legalities":{"historic":"legal"}, "color_identity":["U"], "colors":["U"]
        }));
        let connection = Connection::open(catalog_path(&directory)).unwrap();
        initialize(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO cards (id, name, card_json) VALUES (?1, ?2, ?3)",
                params![
                    logical.id.0,
                    logical.name,
                    serde_json::to_string(&logical).unwrap()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO printing_aliases (printing_id, card_id) VALUES ('old-printing', 'logical-opt')",
                [],
            )
            .unwrap();
        drop(connection);

        let aliases = canonical_ids(&directory, &[CardId("old-printing".into())]).unwrap();
        assert_eq!(
            aliases.get(&CardId("old-printing".into())),
            Some(&CardId("logical-opt".into()))
        );
        let cards = cards_by_ids(&directory, &[CardId("old-printing".into())]).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].id.0, "logical-opt");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn v7_import_replaces_printing_rows_with_unique_logical_cards_and_aliases() {
        let directory = std::env::temp_dir().join(format!(
            "metagene-catalog-v6-import-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let connection = Connection::open(catalog_path(&directory)).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE cards (id TEXT PRIMARY KEY, name TEXT NOT NULL COLLATE NOCASE, card_json TEXT NOT NULL);
                 CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
            )
            .unwrap();
        let legacy_arena = convert(json!({
            "id":"arena-opt", "arena_id":123, "name":"Opt", "set":"dom", "collector_number":"60",
            "type_line":"Instant", "mana_cost":"{U}", "oracle_text":"Scry 1. Draw a card.",
            "games":["arena"], "legalities":{"historic":"legal"}, "color_identity":["U"], "colors":["U"]
        }));
        let legacy_paper = convert(json!({
            "id":"paper-opt", "name":"Opt", "set":"inv", "collector_number":"64",
            "type_line":"Instant", "mana_cost":"{U}", "oracle_text":"Scry 1. Draw a card.",
            "games":["paper"], "legalities":{"modern":"legal"}, "color_identity":["U"], "colors":["U"]
        }));
        for card in [&legacy_paper, &legacy_arena] {
            connection
                .execute(
                    "INSERT INTO cards (id, name, card_json) VALUES (?1, ?2, ?3)",
                    params![card.id.0, card.name, serde_json::to_string(card).unwrap()],
                )
                .unwrap();
        }
        drop(connection);

        let bulk_rows = [
            json!({
                "id":"paper-opt", "name":"Opt", "set":"inv", "collector_number":"64",
                "type_line":"Instant", "mana_cost":"{U}", "oracle_text":"Scry 1. Draw a card.",
                "games":["paper"], "legalities":{"modern":"legal"}, "color_identity":["U"], "colors":["U"]
            }),
            json!({
                "id":"arena-opt", "arena_id":123, "name":"Opt", "set":"dom", "collector_number":"60",
                "type_line":"Instant", "mana_cost":"{U}", "oracle_text":"Scry 1. Draw a card.",
                "games":["arena"], "legalities":{"historic":"legal"}, "color_identity":["U"], "colors":["U"]
            }),
            json!({
                "id":"shock", "arena_id":456, "name":"Shock", "set":"m20", "collector_number":"160",
                "type_line":"Instant", "mana_cost":"{R}", "oracle_text":"Shock deals 2 damage to any target.",
                "games":["arena"], "legalities":{"historic":"legal"}, "color_identity":["R"], "colors":["R"]
            }),
        ];
        let pending = directory.join("fixture.jsonl.gz");
        let file = File::create(&pending).unwrap();
        let mut encoder = GzEncoder::new(file, Compression::default());
        for row in bulk_rows {
            writeln!(encoder, "{}", serde_json::to_string(&row).unwrap()).unwrap();
        }
        encoder.finish().unwrap();
        import_jsonl(&directory, &pending, "test", 0, None, |_| {}).unwrap();

        let connection = Connection::open(catalog_path(&directory)).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM cards", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM printing_aliases", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert_eq!(
            connection
                .query_row("SELECT id FROM cards WHERE name='Opt'", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "arena-opt"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT value FROM metadata WHERE key='catalog_version'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            CATALOG_MODEL_VERSION.to_string()
        );
        drop(connection);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn transforming_cards_only_expose_the_front_face_initially() {
        let journey = convert(base(
            "Journey to Eternity",
            "transform",
            json!([
                {"name":"Journey to Eternity", "type_line":"Legendary Enchantment — Aura", "mana_cost":"{1}{B}{G}", "colors":["B","G"], "oracle_text":"Enchant creature you control"},
                {"name":"Atzal, Cave of Eternity", "type_line":"Legendary Land", "colors":[], "oracle_text":"{T}: Add one mana of any color."}
            ]),
        ));
        assert!(!journey.features.is_land);
        assert!(!journey.features.can_be_land);
        assert_eq!(journey.features.initial_modes.len(), 1);
    }

    #[test]
    fn front_face_colors_and_double_pips_are_preserved() {
        let tamiyo = convert(base(
            "Tamiyo",
            "transform",
            json!([
                {"name":"Tamiyo", "type_line":"Legendary Creature — Moonfolk Wizard", "mana_cost":"{U}", "colors":["U"], "oracle_text":""},
                {"name":"Tamiyo, Seasoned Scholar", "type_line":"Legendary Planeswalker — Tamiyo", "colors":["U"], "oracle_text":""}
            ]),
        ));
        assert_eq!(tamiyo.colors, vec![Color::Blue]);
        assert_eq!(tamiyo.features.initial_modes[0].mana_cost.colored[1], 1);
        let venat = convert(base(
            "Venat",
            "transform",
            json!([
                {"name":"Venat", "type_line":"Legendary Creature — Bird", "mana_cost":"{1}{W}{W}", "colors":["W"], "oracle_text":""},
                {"name":"Hydaelyn", "type_line":"Legendary Enchantment", "colors":["W"], "oracle_text":""}
            ]),
        ));
        assert_eq!(venat.features.initial_modes[0].mana_cost.colored[0], 2);
    }

    #[test]
    fn modal_lands_and_late_fixing_are_distinguished() {
        let pathway = convert(base(
            "Branchloft Pathway",
            "modal_dfc",
            json!([
                {"name":"Branchloft Pathway", "type_line":"Land", "oracle_text":"{T}: Add {G}."},
                {"name":"Boulderloft Pathway", "type_line":"Land", "oracle_text":"{T}: Add {W}."}
            ]),
        ));
        assert_eq!(pathway.features.initial_modes.len(), 2);
        assert_eq!(
            pathway.features.initial_modes[0].mana_source_colors,
            vec![Color::Green]
        );
        assert_eq!(
            pathway.features.initial_modes[1].mana_source_colors,
            vec![Color::White]
        );

        let quarry = convert(json!({
            "id":"quarry", "name":"Crystal Quarry", "set":"tst", "collector_number":"1",
            "type_line":"Land", "oracle_text":"{T}: Add {C}.\n{5}, {T}: Add {W}{U}{B}{R}{G}.",
            "games":["arena"], "legalities":{}, "color_identity":["W","U","B","R","G"]
        }));
        assert_eq!(
            quarry.features.initial_modes[0].mana_source_colors,
            vec![Color::Colorless]
        );
        assert_eq!(
            quarry.features.initial_modes[0]
                .late_mana_source_colors
                .len(),
            5
        );

        let stone = convert(json!({
            "id":"stone", "name":"The Soul Stone", "set":"tst", "collector_number":"1",
            "type_line":"Legendary Artifact", "mana_cost":"{2}", "oracle_text":"{T}: Add {B}.",
            "games":["arena"], "legalities":{}, "color_identity":["B"]
        }));
        assert!(
            stone.features.initial_modes[0]
                .mana_source_colors
                .is_empty()
        );
        assert_eq!(
            stone.features.initial_modes[0].late_mana_source_colors,
            vec![Color::Black]
        );
    }

    #[test]
    fn mana_parser_covers_hybrid_phyrexian_colorless_and_variable() {
        let parsed = parse_mana_cost("{X}{2}{W/U}{B/P}{C}");
        assert!(parsed.variable);
        assert_eq!(parsed.generic, 2);
        assert_eq!(parsed.hybrid.len(), 2);
        assert_eq!(parsed.colored[5], 1);
    }

    #[test]
    fn mana_sources_ignore_reminder_tokens_triggers_and_unmet_activation_conditions() {
        let treasure_aura = card_mode(
            "Sticky Fingers",
            "Enchantment — Aura",
            "{R}",
            &["R".into()],
            "Enchant creature\nEnchanted creature has menace and ‘Whenever this creature deals combat damage to a player, create a Treasure token.’ (It's an artifact with ‘{T}, Sacrifice this artifact: Add one mana of any color.’)",
            &[],
        );
        assert!(treasure_aura.mana_sources.is_empty());
        assert!(
            !treasure_aura
                .abilities
                .iter()
                .any(|ability| ability.kind == metagene_core::AbilityKind::Damage)
        );

        let plow = card_mode(
            "Colossal Plow",
            "Artifact — Vehicle",
            "{2}",
            &[],
            "Whenever Colossal Plow attacks, add {W}{W}{W} and you gain 3 life.\nCrew 6",
            &[],
        );
        assert!(plow.mana_sources.is_empty());

        let mirrex = card_mode(
            "Mirrex",
            "Land — Sphere",
            "",
            &[],
            "{T}: Add {C}.\n{T}: Add one mana of any color. Activate only if Mirrex entered the battlefield this turn.",
            &[],
        );
        assert_eq!(mirrex.mana_sources.len(), 1);
        assert_eq!(mirrex.mana_sources[0].colors, vec![Color::Colorless]);

        let verge = card_mode(
            "Sunbillow Verge",
            "Land",
            "",
            &[],
            "{T}: Add {W}.\n{T}: Add {R}. Activate only if you control a Mountain or a Plains.",
            &[],
        );
        assert_eq!(verge.mana_sources.len(), 1);
        assert_eq!(verge.mana_sources[0].colors, vec![Color::White]);
    }

    #[test]
    fn missing_printed_cost_is_not_zero_mana() {
        let living_end = card_mode(
            "Living End",
            "Sorcery",
            "",
            &["B".into()],
            "Suspend 3—{2}{B}{B}",
            &["Suspend".into()],
        );
        let crypt = card_mode("Tormod's Crypt", "Artifact", "{0}", &[], "", &[]);
        assert_eq!(living_end.normal_castable, Some(false));
        assert_eq!(crypt.normal_castable, Some(true));
    }

    #[test]
    fn mox_amber_uses_controlled_legendary_colors_instead_of_fixed_five_color_mana() {
        let mox = card_mode(
            "Mox Amber",
            "Legendary Artifact",
            "{0}",
            &[],
            "{T}: Add one mana of any color among legendary creatures and planeswalkers you control.",
            &[],
        );
        assert_eq!(mox.mana_sources.len(), 1);
        assert_eq!(
            mox.mana_sources[0].color_rule,
            metagene_core::ManaColorRule::LegendaryPermanents
        );
        assert!(mox.mana_source_colors.is_empty());
    }

    #[test]
    fn ordered_rules_ir_does_not_multiply_one_clause_across_compatibility_profiles() {
        let crypt = card_mode(
            "Tormod's Crypt",
            "Artifact",
            "{0}",
            &[],
            "{T}, Sacrifice Tormod's Crypt: Exile target player's graveyard.",
            &[],
        );
        assert_eq!(
            crypt
                .abilities
                .iter()
                .filter(|ability| matches!(
                    ability.kind,
                    metagene_core::AbilityKind::Exile | metagene_core::AbilityKind::GraveyardHate
                ))
                .count(),
            1
        );

        let fire_imp = card_mode(
            "Fire Imp",
            "Creature — Imp",
            "{2}{R}",
            &["R".into()],
            "When Fire Imp enters the battlefield, it deals 2 damage to target creature.",
            &[],
        );
        assert!(
            fire_imp
                .abilities
                .iter()
                .any(|ability| ability.kind == metagene_core::AbilityKind::Damage)
        );
    }

    fn restricted_land(name: &str, oracle_text: &str) -> Card {
        convert(json!({
            "id": name, "name": name, "set": "tst", "collector_number": "1",
            "type_line": "Land", "oracle_text": oracle_text,
            "games": ["arena"], "legalities": {}, "color_identity": ["W","U","B","R","G"]
        }))
    }

    #[test]
    fn named_restricted_lands_keep_exact_spending_predicates() {
        use metagene_core::{ManaColorRule, ManaSpendRule};
        let plaza = restricted_land(
            "Plaza of Heroes",
            "{T}: Add {C}.\n{T}: Add one mana of any color among legendary permanents you control.\n{T}: Add one mana of any color. Spend this mana only to cast a legendary spell.",
        );
        let abilities = &plaza.features.initial_modes[0].mana_sources;
        assert!(
            abilities
                .iter()
                .any(|source| source.colors == vec![Color::Colorless]
                    && source.spend_rule == ManaSpendRule::Unrestricted)
        );
        assert!(
            abilities
                .iter()
                .any(|source| source.color_rule == ManaColorRule::LegendaryPermanents)
        );
        assert!(
            abilities
                .iter()
                .any(|source| source.spend_rule == ManaSpendRule::CastLegendary)
        );

        for (name, oracle, expected) in [
            (
                "Mech Hangar",
                "{T}: Add {C}.\n{T}: Add one mana of any color. Spend this mana only to cast a Pilot or Vehicle spell.",
                vec!["pilot", "vehicle"],
            ),
            (
                "Villainous Hideout",
                "{T}: Add {C}.\n{T}: Add one mana of any color. Spend this mana only to cast a Villain spell.",
                vec!["villain"],
            ),
            (
                "Jasmine Dragon Tea Shop",
                "{T}: Add {C}.\n{T}: Add one mana of any color. Spend this mana only to cast an Ally spell or activate an ability of an Ally source.",
                vec!["ally"],
            ),
            (
                "Maelstrom of the Spirit Dragon",
                "{T}: Add {C}.\n{T}: Add one mana of any color. Spend this mana only to cast a Dragon spell or an Omen spell.",
                vec!["dragon", "omen"],
            ),
        ] {
            let card = restricted_land(name, oracle);
            let abilities = &card.features.initial_modes[0].mana_sources;
            assert!(
                abilities
                    .iter()
                    .any(|source| source.colors == vec![Color::Colorless]
                        && source.spend_rule == ManaSpendRule::Unrestricted)
            );
            assert!(abilities.iter().any(|source| source.spend_rule
                == ManaSpendRule::CastSubtypes(
                    expected.iter().map(|value| (*value).to_string()).collect()
                )));
        }

        let courtyard = restricted_land(
            "Secluded Courtyard",
            "As Secluded Courtyard enters, choose a creature type.\n{T}: Add {C}.\n{T}: Add one mana of any color. Spend this mana only to cast a creature spell of the chosen type.",
        );
        assert!(
            courtyard.features.initial_modes[0]
                .mana_sources
                .iter()
                .any(|source| source.spend_rule == ManaSpendRule::CastChosenCreatureType)
        );
    }

    #[test]
    fn command_tower_and_unknown_restrictions_are_conservative() {
        use metagene_core::{ManaColorRule, ManaSpendRule, ManaStateRule};
        let tower = restricted_land(
            "Command Tower",
            "{T}: Add one mana of any color in your commander's color identity.",
        );
        let tower_source = &tower.features.initial_modes[0].mana_sources[0];
        assert_eq!(tower_source.color_rule, ManaColorRule::CommanderIdentity);
        assert!(
            tower.features.initial_modes[0]
                .mana_source_colors
                .is_empty()
        );

        let unknown = restricted_land(
            "Jasmine Dragon Tea Shop",
            "{T}: Add {C}.\n{T}: Add one mana of any color. Spend this mana only to pay for a triggered ability from a Food you control.",
        );
        assert_eq!(
            unknown.features.initial_modes[0].mana_source_colors,
            vec![Color::Colorless]
        );
        assert!(
            unknown.features.initial_modes[0]
                .mana_sources
                .iter()
                .any(|source| source.spend_rule == ManaSpendRule::UnknownRestricted)
        );

        let core = restricted_land(
            "Mirrodin's Core",
            "{T}: Add {C}.\n{T}: Put a charge counter on Mirrodin's Core.\n{T}, Remove a charge counter from Mirrodin's Core: Add one mana of any color.",
        );
        assert!(
            core.features.initial_modes[0]
                .mana_sources
                .iter()
                .any(|source| source.state_rule == ManaStateRule::RemoveCounter
                    && source.earliest_turn >= 2)
        );
    }

    #[test]
    fn skullbomb_modes_keep_their_distinct_costs_and_targets() {
        use metagene_core::{AbilityKind, ActionTargetProfile};
        let dross = convert(json!({
            "id":"dross", "name":"Dross Skullbomb", "set":"one", "collector_number":"225",
            "type_line":"Artifact", "mana_cost":"{1}",
            "oracle_text":"{1}, Sacrifice Dross Skullbomb: Draw a card.\n{2}{B}, Sacrifice Dross Skullbomb: Return target creature card from your graveyard to your hand. Draw a card.",
            "games":["arena"], "legalities":{}, "color_identity":["B"]
        }));
        let actions = &dross.features.initial_modes[0].abilities;
        assert!(actions.iter().any(|action| action.kind == AbilityKind::Draw
            && action.cost.mana.generic == 1
            && action.cost.sacrifices_self));
        assert!(
            actions
                .iter()
                .any(|action| action.kind == AbilityKind::Recursion
                    && action.cost.mana.generic == 2
                    && action.cost.mana.colored[2] == 1
                    && action.target == ActionTargetProfile::OwnGraveyardCreature)
        );

        let surgical = convert(json!({
            "id":"surgical", "name":"Surgical Skullbomb", "set":"one", "collector_number":"243",
            "type_line":"Artifact", "mana_cost":"{1}",
            "oracle_text":"{1}, Sacrifice Surgical Skullbomb: Draw a card.\n{2}{U}, Sacrifice Surgical Skullbomb: Return target creature to its owner's hand. Draw a card.",
            "games":["arena"], "legalities":{}, "color_identity":["U"]
        }));
        let actions = &surgical.features.initial_modes[0].abilities;
        assert!(
            actions
                .iter()
                .any(|action| action.kind == AbilityKind::Bounce
                    && action.cost.mana.colored[1] == 1
                    && action.target == ActionTargetProfile::AnyCreature)
        );
        assert!(actions.iter().all(|action| {
            action.zone == AbilityZone::Battlefield && !action.label.trim().is_empty()
        }));
    }

    #[test]
    fn action_profiles_record_zone_conditions_and_original_text() {
        let hand_search = convert(json!({
            "id":"hand-search", "name":"Hand Search", "set":"tst", "collector_number":"1",
            "name":"Hand Search", "type_line":"Creature — Test", "mana_cost":"{2}{G}",
            "oracle_text":"Discard this card: Search your library for a basic land card if you control a creature, reveal it, put it into your hand, then shuffle.",
            "games":["arena"], "legalities":{}, "color_identity":["G"]
        }));
        let action = hand_search.features.initial_modes[0]
            .abilities
            .iter()
            .find(|action| action.kind == AbilityKind::Search)
            .unwrap();
        assert_eq!(action.zone, AbilityZone::Hand);
        assert!(action.cost.discards_self);
        assert!(action.label.starts_with("Search your library"));
        assert!(
            action
                .conditions
                .iter()
                .any(|condition| condition.starts_with("if "))
        );
    }

    #[test]
    fn narrow_and_consumable_actions_are_typed_instead_of_generic_removal() {
        use metagene_core::{AbilityKind, ActionTargetProfile};
        let seedship = convert(json!({
            "id":"seedship", "name":"Seedship Impact", "set":"eoe", "collector_number":"205",
            "type_line":"Instant", "mana_cost":"{1}{G}",
            "oracle_text":"Destroy target artifact or enchantment.",
            "games":["arena"], "legalities":{}, "color_identity":["G"]
        }));
        let action = &seedship.features.initial_modes[0].abilities[0];
        assert_eq!(action.kind, AbilityKind::Destroy);
        assert_eq!(action.target, ActionTargetProfile::ArtifactOrEnchantment);
        assert!(action.narrow);

        let bell = convert(json!({
            "id":"bell", "name":"Fountainport Bell", "set":"blb", "collector_number":"245",
            "type_line":"Artifact", "mana_cost":"{1}",
            "oracle_text":"When Fountainport Bell enters, search your library for a basic land card, reveal it, then shuffle and put that card on top.",
            "games":["arena"], "legalities":{}, "color_identity":[]
        }));
        let action = &bell.features.initial_modes[0].abilities[0];
        assert_eq!(action.kind, AbilityKind::Search);
        assert_eq!(action.target, ActionTargetProfile::BasicLandInLibrary);
        assert!(action.narrow);
    }

    #[test]
    fn ghost_vacuum_payoff_requires_creatures_stored_by_its_graveyard_mode() {
        use metagene_core::{AbilityKind, ActionTargetProfile};
        let vacuum = convert(json!({
            "id":"vacuum", "name":"Ghost Vacuum", "set":"dsk", "collector_number":"248",
            "type_line":"Artifact", "mana_cost":"{1}",
            "oracle_text":"{T}: Exile target card from a graveyard.\n{6}, {T}, Sacrifice Ghost Vacuum: Put each creature card exiled with Ghost Vacuum onto the battlefield under your control.",
            "games":["arena"], "legalities":{}, "color_identity":[]
        }));
        let abilities = &vacuum.features.initial_modes[0].abilities;
        assert!(abilities.iter().any(|ability| {
            ability.kind == AbilityKind::GraveyardHate
                && ability.target == ActionTargetProfile::GraveyardCard
        }));
        let payoff = abilities
            .iter()
            .find(|ability| ability.kind == AbilityKind::Recursion)
            .unwrap();
        assert_eq!(payoff.cost.mana.generic, 6);
        assert!(payoff.cost.sacrifices_self);
        assert!(payoff.narrow);
        assert!(
            payoff
                .conditions
                .iter()
                .any(|condition| condition.contains("previously exiled"))
        );
    }

    #[test]
    fn functional_evaluation_does_not_credit_unpayable_skullbomb_modes() {
        use metagene_core::{
            AbilityKind, CardId, CardPool, Deck, DeckEntry, OpponentFieldProfile,
            StructuralEvaluationContext, evaluate_structural_deck_with_context,
        };
        let forest = convert(json!({
            "id":"forest", "name":"Forest", "set":"tst", "collector_number":"1",
            "type_line":"Basic Land — Forest", "oracle_text":"{T}: Add {G}.",
            "games":["arena"], "legalities":{}, "color_identity":["G"]
        }));
        let dross = convert(json!({
            "id":"dross", "name":"Dross Skullbomb", "set":"one", "collector_number":"225",
            "type_line":"Artifact", "mana_cost":"{1}",
            "oracle_text":"{1}, Sacrifice Dross Skullbomb: Draw a card.\n{2}{B}, Sacrifice Dross Skullbomb: Return target creature card from your graveyard to your hand. Draw a card.",
            "games":["arena"], "legalities":{}, "color_identity":["B"]
        }));
        let surgical = convert(json!({
            "id":"surgical", "name":"Surgical Skullbomb", "set":"one", "collector_number":"243",
            "type_line":"Artifact", "mana_cost":"{1}",
            "oracle_text":"{1}, Sacrifice Surgical Skullbomb: Draw a card.\n{2}{U}, Sacrifice Surgical Skullbomb: Return target creature to its owner's hand. Draw a card.",
            "games":["arena"], "legalities":{}, "color_identity":["U"]
        }));
        let finisher = convert(json!({
            "id":"finisher", "name":"Token Finisher", "set":"tst", "collector_number":"2",
            "type_line":"Creature — Beast", "mana_cost":"{2}{G}", "power":"3", "toughness":"3",
            "oracle_text":"When Token Finisher enters, create a 1/1 green Saproling creature token.",
            "games":["arena"], "legalities":{}, "color_identity":["G"], "colors":["G"]
        }));
        let pool = CardPool {
            cards: vec![forest, dross, surgical, finisher],
        };
        let deck = Deck {
            name: "Green artifact exploit".into(),
            command_zone: vec![],
            mainboard: vec![
                DeckEntry {
                    card_id: CardId("forest".into()),
                    quantity: 12,
                },
                DeckEntry {
                    card_id: CardId("dross".into()),
                    quantity: 4,
                },
                DeckEntry {
                    card_id: CardId("surgical".into()),
                    quantity: 4,
                },
                DeckEntry {
                    card_id: CardId("finisher".into()),
                    quantity: 4,
                },
            ],
            sideboard: vec![],
        };
        let fitness = evaluate_structural_deck_with_context(
            &deck,
            &pool,
            7,
            32,
            8,
            &StructuralEvaluationContext {
                opponent_field: OpponentFieldProfile::default(),
                ..Default::default()
            },
        )
        .unwrap();
        let draw = fitness
            .action_evidence
            .iter()
            .find(|action| action.card_name == "Dross Skullbomb" && action.kind == "draw action")
            .unwrap();
        assert!(draw.mana_availability > 0.0);
        let recursion = fitness
            .action_evidence
            .iter()
            .find(|action| {
                action.card_name == "Dross Skullbomb" && action.kind == "recursion action"
            })
            .unwrap();
        assert_eq!(recursion.mana_availability, 0.0);
        let bounce = fitness
            .action_evidence
            .iter()
            .find(|action| {
                action.card_name == "Surgical Skullbomb" && action.kind == "bounce action"
            })
            .unwrap();
        assert_eq!(bounce.mana_availability, 0.0);
        assert!(fitness.dead_action_rate > 0.25);
        assert!(
            pool.cards[3].features.initial_modes[0]
                .abilities
                .iter()
                .any(|ability| ability.kind == AbilityKind::CreateToken)
        );
    }

    #[test]
    fn generated_historic_regression_fixture_keeps_all_eight_legal_size_lists() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../data/evaluation/restricted-mana-synergy-v4-candidates.json"
        ))
        .unwrap();
        let candidates = fixture["candidates"].as_array().unwrap();
        assert_eq!(candidates.len(), 8);
        for (expected_id, candidate) in (1_u64..=8).zip(candidates) {
            assert_eq!(candidate["id"].as_u64(), Some(expected_id));
            let total = candidate["deck"]
                .as_array()
                .unwrap()
                .iter()
                .map(|line| {
                    line.as_str()
                        .unwrap()
                        .split_whitespace()
                        .next()
                        .unwrap()
                        .parse::<usize>()
                        .unwrap()
                })
                .sum::<usize>();
            assert_eq!(total, 60, "candidate {expected_id} fixture changed size");
        }
    }

    #[test]
    fn functional_tune_fixture_preserves_root_and_all_artifact_shell_failures() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../data/evaluation/functional-actions-tune-v6-candidates.json"
        ))
        .unwrap();
        let count = |lines: &serde_json::Value| {
            lines
                .as_array()
                .unwrap()
                .iter()
                .map(|line| {
                    line.as_str()
                        .unwrap()
                        .split_whitespace()
                        .next()
                        .unwrap()
                        .parse::<usize>()
                        .unwrap()
                })
                .sum::<usize>()
        };
        assert_eq!(count(&fixture["root"]), 60);
        let candidates = fixture["candidates"].as_array().unwrap();
        assert_eq!(candidates.len(), 8);
        for candidate in candidates {
            assert_eq!(count(&candidate["deck"]), 60);
            assert!(
                candidate["expected"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|value| value == "rejected_by_tune_identity")
            );
        }
    }

    #[test]
    fn original_copy_deck_core_actions_have_lossless_rules_ir() {
        use metagene_core::{RuleCost, RuleEffect, RuleTiming};
        let fixtures = [
            (
                "Prosperous Innkeeper",
                "Creature — Halfling Citizen",
                "When Prosperous Innkeeper enters the battlefield, create a Treasure token.\nWhenever another creature enters the battlefield under your control, you gain 1 life.",
                vec![],
            ),
            (
                "Glasspool Mimic",
                "Creature — Shapeshifter Rogue",
                "You may have Glasspool Mimic enter the battlefield as a copy of a creature you control, except it's a Shapeshifter Rogue in addition to its other types.",
                vec![],
            ),
            (
                "Howling Mine",
                "Artifact",
                "At the beginning of each player's draw step, if Howling Mine is untapped, that player draws an additional card.",
                vec![],
            ),
            (
                "Nightclub Bouncer",
                "Creature — Human Rogue",
                "When Nightclub Bouncer enters the battlefield, return target nonland permanent an opponent controls to its owner's hand. It perpetually costs {1} more to cast.",
                vec![],
            ),
            (
                "Whirler Rogue",
                "Creature — Human Rogue Artificer",
                "When Whirler Rogue enters the battlefield, create two 1/1 colorless Thopter artifact creature tokens with flying.\nTap two untapped artifacts you control: Target creature can't be blocked this turn.",
                vec![],
            ),
            (
                "Acidic Slime",
                "Creature — Ooze",
                "Deathtouch\nWhen Acidic Slime enters the battlefield, destroy target artifact, enchantment, or land.",
                vec!["Deathtouch".into()],
            ),
            (
                "Thassa, Deep-Dwelling",
                "Legendary Enchantment Creature — God",
                "Indestructible\nAs long as your devotion to blue is less than five, Thassa isn't a creature.\nAt the beginning of your end step, exile up to one other target creature you control, then return that card to the battlefield under your control.\n{3}{U}: Tap another target creature.",
                vec!["Indestructible".into()],
            ),
            (
                "Quasiduplicate",
                "Sorcery",
                "Create a token that's a copy of target creature you control.\nJump-start {5}{U}{U}",
                vec!["Jump-start".into()],
            ),
            (
                "Cackling Counterpart",
                "Instant",
                "Create a token that's a copy of target creature you control.\nFlashback {5}{U}{U}",
                vec!["Flashback".into()],
            ),
            (
                "Parallel Lives",
                "Enchantment",
                "If an effect would create one or more tokens under your control, it creates twice that many of those tokens instead.",
                vec![],
            ),
            (
                "Mythos of Illuna",
                "Sorcery",
                "Create a token that's a copy of target permanent. If {R}{G} was spent to cast this spell, instead that token fights up to one target creature you don't control.",
                vec![],
            ),
        ];
        let modes = fixtures
            .iter()
            .map(|(name, type_line, text, keywords)| {
                card_mode(name, type_line, "", &[], text, keywords)
            })
            .collect::<Vec<_>>();
        for mode in &modes {
            let unresolved = mode
                .oracle_clauses
                .iter()
                .filter(|clause| {
                    clause.classification
                        == metagene_core::ClauseClassification::UnresolvedGameplayResidual
                })
                .map(|clause| clause.text.as_str())
                .collect::<Vec<_>>();
            assert!(
                unresolved.is_empty(),
                "{} unresolved: {unresolved:?}",
                mode.name
            );
            assert!(
                !mode.rule_abilities.is_empty(),
                "{} has no Rules IR",
                mode.name
            );
        }

        let innkeeper = &modes[0].rule_abilities;
        assert!(innkeeper.iter().flat_map(|rule| &rule.effects).any(|effect| matches!(effect, RuleEffect::CreateToken { token, .. } if token.name == "treasure")));
        assert!(
            innkeeper
                .iter()
                .flat_map(|rule| &rule.effects)
                .any(|effect| matches!(effect, RuleEffect::Life { gain: true, .. }))
        );
        assert!(
            modes[1]
                .rule_abilities
                .iter()
                .flat_map(|rule| &rule.effects)
                .any(|effect| matches!(effect, RuleEffect::Copy { .. }))
        );
        assert!(modes[2].rule_abilities.iter().any(|rule| {
            rule.timing == RuleTiming::Trigger
                && rule
                    .conditions
                    .iter()
                    .any(|condition| format!("{condition:?}").contains("untapped"))
        }));
        assert!(
            modes[3]
                .rule_abilities
                .iter()
                .flat_map(|rule| &rule.effects)
                .any(|effect| matches!(effect, RuleEffect::Perpetual { .. }))
        );
        assert!(modes[4].rule_abilities.iter().flat_map(|rule| &rule.costs).any(|cost| matches!(cost, RuleCost::Additional(value) if value.contains("two untapped artifacts"))));
        assert!(modes[5].rule_abilities.iter().flat_map(|rule| &rule.effects).any(|effect| matches!(effect, RuleEffect::GrantKeyword { keyword } if keyword == "deathtouch")));
        assert!(
            !modes[5]
                .rule_abilities
                .iter()
                .flat_map(|rule| &rule.effects)
                .any(|effect| matches!(effect, RuleEffect::Damage { .. }))
        );
        assert!(modes[6].rule_abilities.iter().flat_map(|rule| &rule.effects).any(|effect| matches!(effect, RuleEffect::ModifyTypes { remove, .. } if remove.contains(&"creature".to_string()))));
        assert!(modes[7].rule_abilities.iter().flat_map(|rule| &rule.effects).any(|effect| matches!(effect, RuleEffect::CreateToken { token, .. } if token.copy_of_target)));
        assert!(
            modes[9]
                .rule_abilities
                .iter()
                .any(|rule| rule.timing == RuleTiming::Replacement)
        );
        assert!(modes[9]
            .rule_abilities
            .iter()
            .flat_map(|rule| &rule.effects)
            .any(|effect| matches!(effect, RuleEffect::Replace { event, .. } if event.contains("token"))));
    }

    #[test]
    fn every_non_reminder_clause_is_parsed_or_explicitly_residual() {
        let mode = card_mode(
            "Accounting Fixture",
            "Enchantment",
            "{2}",
            &[],
            "When this enters, draw a card. (This is reminder text.)\nUnmodeled moon arithmetic happens.",
            &[],
        );
        assert!(
            mode.oracle_clauses
                .iter()
                .filter(|clause| clause.classification
                    != metagene_core::ClauseClassification::ReminderText)
                .all(|clause| matches!(
                    clause.classification,
                    metagene_core::ClauseClassification::ParsedGameplayRule
                        | metagene_core::ClauseClassification::RecognizedKeywordRule
                        | metagene_core::ClauseClassification::UnresolvedGameplayResidual
                ))
        );
        assert!(
            mode.oracle_clauses
                .iter()
                .any(|clause| clause.classification
                    == metagene_core::ClauseClassification::UnresolvedGameplayResidual
                    && clause.text.contains("moon arithmetic"))
        );
    }
}
