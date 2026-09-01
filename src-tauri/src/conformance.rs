use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const CONFORMANCE_SCHEMA_VERSION: u32 = 2;
const MAX_OBSERVATIONS: usize = 100_000;

#[derive(Debug, Error)]
pub enum ConformanceError {
    #[error("failed to access conformance storage: {0}")]
    Io(#[from] std::io::Error),
    #[error("conformance database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("invalid conformance JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported conformance schema {0}; expected {CONFORMANCE_SCHEMA_VERSION}")]
    UnsupportedSchema(u32),
    #[error("conformance bundle contains no observations")]
    EmptyBundle,
    #[error("invalid conformance observation {index}: {message}")]
    InvalidObservation { index: usize, message: String },
    #[error("cannot compare observations from different scenario identities")]
    MismatchedComparison,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConformanceBundle {
    pub schema_version: u32,
    pub producer: String,
    pub observations: Vec<ConformanceObservation>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConformanceObservation {
    pub engine: String,
    pub engine_revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<crate::provenance::ResultProvenance>,
    pub scenario_id: String,
    pub scenario_version: u32,
    pub seed: i64,
    pub passed: bool,
    pub outcome: Value,
    pub event_hash: String,
    #[serde(default)]
    pub cards: Vec<String>,
    /// Action-scoped assertions. Empty on schema-v1 observations, which
    /// remain card-level legacy evidence and cannot verify a clause.
    #[serde(default)]
    pub claims: Vec<ConformanceClaim>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(default)]
pub struct ConformanceClaim {
    pub card_name: String,
    pub subject_id: String,
    pub assertion: String,
    pub ai_decision: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonClassification {
    VerifiedAgreement,
    AssertionFailure,
    OutcomeDivergence,
    EventDivergence,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConformanceComparison {
    pub left_engine: String,
    pub right_engine: String,
    pub classification: ComparisonClassification,
    pub both_passed: bool,
    pub outcome_matches: bool,
    pub event_hash_matches: bool,
}

pub fn compare_observations(
    left: &ConformanceObservation,
    right: &ConformanceObservation,
) -> Result<ConformanceComparison, ConformanceError> {
    if left.scenario_id != right.scenario_id
        || left.scenario_version != right.scenario_version
        || left.seed != right.seed
        || provenance_identity(left) != provenance_identity(right)
    {
        return Err(ConformanceError::MismatchedComparison);
    }
    let both_passed = left.passed && right.passed;
    let outcome_matches = left.outcome == right.outcome;
    let event_hash_matches = left.event_hash == right.event_hash;
    let classification = if !both_passed {
        ComparisonClassification::AssertionFailure
    } else if !outcome_matches {
        ComparisonClassification::OutcomeDivergence
    } else if !event_hash_matches {
        ComparisonClassification::EventDivergence
    } else {
        ComparisonClassification::VerifiedAgreement
    };
    Ok(ConformanceComparison {
        left_engine: left.engine.clone(),
        right_engine: right.engine.clone(),
        classification,
        both_passed,
        outcome_matches,
        event_hash_matches,
    })
}

fn provenance_identity(observation: &ConformanceObservation) -> Option<(u32, &str)> {
    observation.provenance.as_ref().map(|provenance| {
        (
            provenance.scenario_schema_version.unwrap_or_default(),
            provenance.configuration_hash.as_str(),
        )
    })
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConformanceEngineSummary {
    pub engine: String,
    pub revisions: Vec<String>,
    pub observations: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ConformanceSummary {
    pub schema_version: u32,
    pub observations: usize,
    pub scenario_runs: usize,
    pub interaction_verified_scenarios: usize,
    pub cross_engine_verified_scenarios: usize,
    pub failed_scenarios: usize,
    pub conflicting_scenarios: usize,
    pub engines: Vec<ConformanceEngineSummary>,
    pub database_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConformanceImportResult {
    pub imported_observations: usize,
    pub summary: ConformanceSummary,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CardConformance {
    pub name: String,
    pub status: String,
    pub classification: String,
    pub scenarios: Vec<String>,
    #[serde(default)]
    pub claims: Vec<ActionConformance>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ActionConformance {
    pub subject_id: String,
    pub status: String,
    pub classification: String,
    pub ai_decision_covered: bool,
    pub scenarios: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeckConformanceReport {
    pub deck_name: String,
    pub unique_cards: usize,
    pub interaction_verified_unique: usize,
    pub cross_engine_verified_unique: usize,
    pub failed_unique: usize,
    pub conflicting_unique: usize,
    pub unverified_unique: usize,
    pub verified_unique: usize,
    pub partially_covered_unique: usize,
    pub disputed_unique: usize,
    pub unsupported_unique: usize,
    pub cards: Vec<CardConformance>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScenarioConformance {
    pub scenario_schema_version: u32,
    pub scenario_id: String,
    pub scenario_version: u32,
    pub seed: i64,
    pub configuration_hash: String,
    pub classification: String,
    pub cards: Vec<String>,
    pub engines: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConformanceAuditReport {
    pub summary: ConformanceSummary,
    pub scenarios: Vec<ScenarioConformance>,
    pub decks: Vec<DeckConformanceReport>,
}

#[derive(Clone)]
struct StoredObservation {
    engine: String,
    engine_revision: String,
    provenance_schema_version: u32,
    card_data_revision: String,
    scenario_schema_version: u32,
    configuration_hash: String,
    scenario_id: String,
    scenario_version: u32,
    seed: i64,
    passed: bool,
    outcome_json: String,
    event_hash: String,
    cards: Vec<String>,
    claims: Vec<ConformanceClaim>,
}

#[derive(Default)]
struct ScenarioEvidence {
    observations: Vec<StoredObservation>,
    cards: BTreeSet<String>,
}

type ScenarioKey = (u32, String, u32, i64, String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum EvidenceLevel {
    Unverified,
    InteractionVerified,
    CrossEngineVerified,
    Failed,
    Conflict,
}

impl EvidenceLevel {
    fn label(self) -> &'static str {
        match self {
            Self::Unverified => "unverified",
            Self::InteractionVerified => "interaction_verified",
            Self::CrossEngineVerified => "cross_engine_verified",
            Self::Failed => "failed",
            Self::Conflict => "conflict",
        }
    }

    fn classification(self) -> &'static str {
        match self {
            Self::Unverified => "unverified",
            Self::InteractionVerified => "partially_covered",
            Self::CrossEngineVerified => "verified",
            Self::Failed => "unsupported",
            Self::Conflict => "disputed",
        }
    }
}

pub fn import_bundle_json(
    data_dir: &Path,
    bundle_json: &str,
) -> Result<ConformanceImportResult, ConformanceError> {
    let bundle: ConformanceBundle = serde_json::from_str(bundle_json)?;
    import_bundle(data_dir, &bundle)
}

pub fn import_bundle(
    data_dir: &Path,
    bundle: &ConformanceBundle,
) -> Result<ConformanceImportResult, ConformanceError> {
    if bundle.schema_version == 0 || bundle.schema_version > CONFORMANCE_SCHEMA_VERSION {
        return Err(ConformanceError::UnsupportedSchema(bundle.schema_version));
    }
    if bundle.observations.is_empty() {
        return Err(ConformanceError::EmptyBundle);
    }
    let producer = required_field(&bundle.producer, 160).map_err(|message| {
        ConformanceError::InvalidObservation {
            index: 0,
            message: format!("producer {message}"),
        }
    })?;
    let mut normalized = Vec::with_capacity(bundle.observations.len());
    for (index, observation) in bundle.observations.iter().enumerate() {
        normalized.push(normalize_observation(
            observation,
            index + 1,
            bundle.schema_version,
        )?);
    }

    let mut connection = open(data_dir)?;
    let transaction = connection.transaction()?;
    for observation in &normalized {
        transaction.execute(
            "INSERT INTO conformance_observations
             (engine, engine_revision, provenance_schema_version, card_data_revision,
              scenario_schema_version, configuration_hash, scenario_id, scenario_version,
              seed, passed, outcome_json, event_hash, cards_json, claims_json, producer)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(engine, engine_revision, card_data_revision, scenario_schema_version,
                         configuration_hash, scenario_id, scenario_version, seed)
             DO UPDATE SET passed=excluded.passed, outcome_json=excluded.outcome_json,
               event_hash=excluded.event_hash, cards_json=excluded.cards_json,
               claims_json=excluded.claims_json,
               provenance_schema_version=excluded.provenance_schema_version,
               producer=excluded.producer, imported_at=datetime('now')",
            params![
                observation.engine,
                observation.engine_revision,
                observation.provenance_schema_version,
                observation.card_data_revision,
                observation.scenario_schema_version,
                observation.configuration_hash,
                observation.scenario_id,
                observation.scenario_version,
                observation.seed,
                observation.passed,
                observation.outcome_json,
                observation.event_hash,
                serde_json::to_string(&observation.cards)?,
                serde_json::to_string(&observation.claims)?,
                producer,
            ],
        )?;
    }
    transaction.execute(
        "DELETE FROM conformance_observations WHERE rowid NOT IN (
           SELECT rowid FROM conformance_observations
           ORDER BY imported_at DESC, rowid DESC LIMIT ?1
         )",
        [MAX_OBSERVATIONS as i64],
    )?;
    transaction.commit()?;
    Ok(ConformanceImportResult {
        imported_observations: normalized.len(),
        summary: summary(data_dir)?,
    })
}

pub fn summary(data_dir: &Path) -> Result<ConformanceSummary, ConformanceError> {
    let observations = load_observations(data_dir)?;
    Ok(build_summary(data_dir, &observations))
}

pub fn audit_cards(
    data_dir: &Path,
    card_names: &[String],
) -> Result<DeckConformanceReport, ConformanceError> {
    let observations = load_observations(data_dir)?;
    let scenarios = group_scenarios(&observations);
    Ok(audit_card_set("Deck", card_names, &scenarios))
}

pub fn audit_decks(
    data_dir: &Path,
    decks: &[(String, Vec<String>)],
) -> Result<ConformanceAuditReport, ConformanceError> {
    let observations = load_observations(data_dir)?;
    let scenarios = group_scenarios(&observations);
    Ok(ConformanceAuditReport {
        summary: build_summary(data_dir, &observations),
        scenarios: scenarios
            .iter()
            .map(
                |(
                    (
                        scenario_schema_version,
                        scenario_id,
                        scenario_version,
                        seed,
                        configuration_hash,
                    ),
                    evidence,
                )| ScenarioConformance {
                    scenario_schema_version: *scenario_schema_version,
                    scenario_id: scenario_id.clone(),
                    scenario_version: *scenario_version,
                    seed: *seed,
                    configuration_hash: configuration_hash.clone(),
                    classification: scenario_level(evidence).classification().into(),
                    cards: evidence.cards.iter().cloned().collect(),
                    engines: evidence
                        .observations
                        .iter()
                        .map(|observation| observation.engine.clone())
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect(),
                },
            )
            .collect(),
        decks: decks
            .iter()
            .map(|(name, cards)| audit_card_set(name, cards, &scenarios))
            .collect(),
    })
}

fn audit_card_set(
    deck_name: &str,
    card_names: &[String],
    scenarios: &BTreeMap<ScenarioKey, ScenarioEvidence>,
) -> DeckConformanceReport {
    let mut evidence_by_card: HashMap<String, (EvidenceLevel, BTreeSet<String>)> = HashMap::new();
    let mut evidence_by_claim: HashMap<(String, String), (EvidenceLevel, bool, BTreeSet<String>)> =
        HashMap::new();
    for ((scenario_schema_version, scenario_id, scenario_version, seed, _), evidence) in scenarios {
        let label = format!(
            "schema {scenario_schema_version} · {scenario_id}@{scenario_version} · seed {seed}"
        );
        for name in &evidence.cards {
            let normalized_name = normalize_card_name(name);
            let card_evidence = ScenarioEvidence {
                observations: evidence
                    .observations
                    .iter()
                    .filter(|observation| {
                        observation
                            .cards
                            .iter()
                            .any(|card| normalize_card_name(card) == normalized_name)
                    })
                    .cloned()
                    .collect(),
                cards: BTreeSet::from([name.clone()]),
            };
            let level = scenario_level(&card_evidence);
            let entry = evidence_by_card
                .entry(normalized_name)
                .or_insert_with(|| (EvidenceLevel::Unverified, BTreeSet::new()));
            entry.0 = entry.0.max(level);
            entry.1.insert(label.clone());
        }
        let claims = evidence
            .observations
            .iter()
            .flat_map(|observation| observation.claims.iter())
            .map(|claim| {
                (
                    normalize_card_name(&claim.card_name),
                    claim.subject_id.clone(),
                    claim.ai_decision,
                )
            })
            .collect::<BTreeSet<_>>();
        for (card_name, subject_id, ai_decision) in claims {
            let claim_evidence = ScenarioEvidence {
                observations: evidence
                    .observations
                    .iter()
                    .filter(|observation| {
                        observation.claims.iter().any(|claim| {
                            normalize_card_name(&claim.card_name) == card_name
                                && claim.subject_id == subject_id
                        })
                    })
                    .cloned()
                    .collect(),
                cards: BTreeSet::new(),
            };
            let level = scenario_level(&claim_evidence);
            let entry = evidence_by_claim
                .entry((card_name, subject_id))
                .or_insert_with(|| (EvidenceLevel::Unverified, false, BTreeSet::new()));
            entry.0 = entry.0.max(level);
            entry.1 |= ai_decision
                && matches!(
                    level,
                    EvidenceLevel::InteractionVerified | EvidenceLevel::CrossEngineVerified
                );
            entry.2.insert(label.clone());
        }
    }

    let mut distinct = BTreeMap::new();
    for name in card_names {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            distinct
                .entry(normalize_card_name(trimmed))
                .or_insert_with(|| trimmed.to_string());
        }
    }
    let cards = distinct
        .into_iter()
        .map(|(normalized, name)| {
            let (level, scenarios) = evidence_by_card
                .get(&normalized)
                .cloned()
                .unwrap_or_else(|| (EvidenceLevel::Unverified, BTreeSet::new()));
            CardConformance {
                name,
                status: level.label().into(),
                classification: level.classification().into(),
                scenarios: scenarios.into_iter().collect(),
                claims: evidence_by_claim
                    .iter()
                    .filter(|((card_name, _), _)| card_name == &normalized)
                    .map(
                        |((_, subject_id), (level, ai_decision_covered, scenarios))| {
                            ActionConformance {
                                subject_id: subject_id.clone(),
                                status: level.label().into(),
                                classification: level.classification().into(),
                                ai_decision_covered: *ai_decision_covered,
                                scenarios: scenarios.iter().cloned().collect(),
                            }
                        },
                    )
                    .collect(),
            }
        })
        .collect::<Vec<_>>();
    let count = |level: &str| cards.iter().filter(|card| card.status == level).count();
    DeckConformanceReport {
        deck_name: deck_name.into(),
        unique_cards: cards.len(),
        interaction_verified_unique: count("interaction_verified"),
        cross_engine_verified_unique: count("cross_engine_verified"),
        failed_unique: count("failed"),
        conflicting_unique: count("conflict"),
        unverified_unique: count("unverified"),
        verified_unique: cards
            .iter()
            .filter(|card| card.classification == "verified")
            .count(),
        partially_covered_unique: cards
            .iter()
            .filter(|card| card.classification == "partially_covered")
            .count(),
        disputed_unique: cards
            .iter()
            .filter(|card| card.classification == "disputed")
            .count(),
        unsupported_unique: cards
            .iter()
            .filter(|card| card.classification == "unsupported")
            .count(),
        cards,
    }
}

fn normalize_observation(
    observation: &ConformanceObservation,
    index: usize,
    bundle_schema_version: u32,
) -> Result<StoredObservation, ConformanceError> {
    let invalid = |message| ConformanceError::InvalidObservation { index, message };
    let engine = required_field(&observation.engine, 80).map_err(&invalid)?;
    let engine_revision = required_field(&observation.engine_revision, 160).map_err(&invalid)?;
    let provenance = observation
        .provenance
        .clone()
        .unwrap_or_else(|| crate::provenance::ResultProvenance::legacy(engine_revision.clone()));
    if provenance.schema_version > crate::provenance::RESULT_PROVENANCE_SCHEMA_VERSION {
        return Err(invalid(format!(
            "unsupported provenance schema {}",
            provenance.schema_version
        )));
    }
    if provenance.engine_revision != engine_revision {
        return Err(invalid(
            "provenance engine_revision does not match observation engine_revision".into(),
        ));
    }
    let card_data_revision =
        required_field(&provenance.card_data_revision, 256).map_err(&invalid)?;
    let configuration_hash =
        required_field(&provenance.configuration_hash, 256).map_err(&invalid)?;
    let scenario_schema_version = provenance.scenario_schema_version.unwrap_or_default();
    if provenance.schema_version > 0 && scenario_schema_version == 0 {
        return Err(invalid(
            "versioned provenance requires scenario_schema_version".into(),
        ));
    }
    let scenario_id = required_field(&observation.scenario_id, 160).map_err(&invalid)?;
    let event_hash = required_field(&observation.event_hash, 256).map_err(&invalid)?;
    if observation.scenario_version == 0 {
        return Err(invalid("scenario_version must be at least 1".into()));
    }
    if observation.cards.len() > 256 {
        return Err(invalid("cards exceeds the 256-card scenario limit".into()));
    }
    let mut cards = observation
        .cards
        .iter()
        .filter_map(|name| {
            let name = name.trim();
            (!name.is_empty()).then(|| name.to_string())
        })
        .collect::<Vec<_>>();
    cards.sort_by_key(|name| name.to_lowercase());
    cards.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    let mut claims = if bundle_schema_version >= 2 {
        observation.claims.clone()
    } else {
        Vec::new()
    };
    claims
        .retain(|claim| !claim.card_name.trim().is_empty() && !claim.subject_id.trim().is_empty());
    for claim in &mut claims {
        claim.card_name = claim.card_name.trim().to_string();
        claim.subject_id = claim.subject_id.trim().to_string();
        claim.assertion = claim.assertion.trim().to_string();
    }
    claims.sort_by(|left, right| {
        left.card_name
            .to_ascii_lowercase()
            .cmp(&right.card_name.to_ascii_lowercase())
            .then_with(|| left.subject_id.cmp(&right.subject_id))
    });
    claims.dedup_by(|left, right| {
        left.card_name.eq_ignore_ascii_case(&right.card_name)
            && left.subject_id == right.subject_id
            && left.assertion == right.assertion
            && left.ai_decision == right.ai_decision
    });
    let mut outcome = observation.outcome.clone();
    canonicalize_json(&mut outcome);
    Ok(StoredObservation {
        engine,
        engine_revision,
        provenance_schema_version: provenance.schema_version,
        card_data_revision,
        scenario_schema_version,
        configuration_hash,
        scenario_id,
        scenario_version: observation.scenario_version,
        seed: observation.seed,
        passed: observation.passed,
        outcome_json: serde_json::to_string(&outcome)?,
        event_hash,
        cards,
        claims,
    })
}

fn required_field(value: &str, maximum: usize) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("must not be empty".into());
    }
    if value.len() > maximum {
        return Err(format!("exceeds {maximum} bytes"));
    }
    Ok(value.to_string())
}

fn canonicalize_json(value: &mut Value) {
    match value {
        Value::Array(values) => values.iter_mut().for_each(canonicalize_json),
        Value::Object(values) => {
            let mut sorted = BTreeMap::new();
            for (key, mut value) in std::mem::take(values) {
                canonicalize_json(&mut value);
                sorted.insert(key, value);
            }
            values.extend(sorted);
        }
        _ => {}
    }
}

fn normalize_card_name(name: &str) -> String {
    name.trim().to_lowercase()
}

fn group_scenarios(observations: &[StoredObservation]) -> BTreeMap<ScenarioKey, ScenarioEvidence> {
    let mut scenarios: BTreeMap<ScenarioKey, ScenarioEvidence> = BTreeMap::new();
    for observation in observations {
        let evidence = scenarios
            .entry((
                observation.scenario_schema_version,
                observation.scenario_id.clone(),
                observation.scenario_version,
                observation.seed,
                observation.configuration_hash.clone(),
            ))
            .or_default();
        evidence.observations.push(observation.clone());
        evidence.cards.extend(observation.cards.iter().cloned());
    }
    scenarios
}

fn scenario_level(evidence: &ScenarioEvidence) -> EvidenceLevel {
    if evidence.observations.is_empty() {
        return EvidenceLevel::Unverified;
    }
    let signatures = evidence
        .observations
        .iter()
        .map(|observation| {
            (
                observation.passed,
                observation.outcome_json.as_str(),
                observation.event_hash.as_str(),
            )
        })
        .collect::<BTreeSet<_>>();
    if signatures.len() > 1 {
        return EvidenceLevel::Conflict;
    }
    if !evidence.observations[0].passed {
        return EvidenceLevel::Failed;
    }
    let engines = evidence
        .observations
        .iter()
        .map(|observation| observation.engine.to_lowercase())
        .collect::<BTreeSet<_>>();
    if engines.len() < 2 {
        return EvidenceLevel::InteractionVerified;
    }
    EvidenceLevel::CrossEngineVerified
}

fn build_summary(data_dir: &Path, observations: &[StoredObservation]) -> ConformanceSummary {
    let scenarios = group_scenarios(observations);
    let mut engines: BTreeMap<String, (BTreeSet<String>, usize)> = BTreeMap::new();
    for observation in observations {
        let entry = engines.entry(observation.engine.clone()).or_default();
        entry.0.insert(observation.engine_revision.clone());
        entry.1 += 1;
    }
    let levels = scenarios.values().map(scenario_level).collect::<Vec<_>>();
    ConformanceSummary {
        schema_version: CONFORMANCE_SCHEMA_VERSION,
        observations: observations.len(),
        scenario_runs: scenarios.len(),
        interaction_verified_scenarios: levels
            .iter()
            .filter(|level| {
                matches!(
                    **level,
                    EvidenceLevel::InteractionVerified | EvidenceLevel::CrossEngineVerified
                )
            })
            .count(),
        cross_engine_verified_scenarios: levels
            .iter()
            .filter(|level| **level == EvidenceLevel::CrossEngineVerified)
            .count(),
        failed_scenarios: levels
            .iter()
            .filter(|level| **level == EvidenceLevel::Failed)
            .count(),
        conflicting_scenarios: levels
            .iter()
            .filter(|level| **level == EvidenceLevel::Conflict)
            .count(),
        engines: engines
            .into_iter()
            .map(
                |(engine, (revisions, observations))| ConformanceEngineSummary {
                    engine,
                    revisions: revisions.into_iter().collect(),
                    observations,
                },
            )
            .collect(),
        database_bytes: [
            database_path(data_dir),
            data_dir.join("conformance.sqlite-wal"),
            data_dir.join("conformance.sqlite-shm"),
        ]
        .iter()
        .filter_map(|path| fs::metadata(path).ok())
        .map(|metadata| metadata.len())
        .sum(),
    }
}

fn load_observations(data_dir: &Path) -> Result<Vec<StoredObservation>, ConformanceError> {
    let connection = open(data_dir)?;
    let mut statement = connection.prepare(
        "SELECT engine, engine_revision, provenance_schema_version, card_data_revision,
                scenario_schema_version, configuration_hash, scenario_id, scenario_version,
                seed, passed, outcome_json, event_hash, cards_json, claims_json
         FROM conformance_observations",
    )?;
    let rows = statement.query_map([], |row| {
        let cards_json: String = row.get(12)?;
        let claims_json: String = row.get(13)?;
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get::<_, i64>(2)?.max(0) as u32,
            row.get(3)?,
            row.get::<_, i64>(4)?.max(0) as u32,
            row.get(5)?,
            row.get(6)?,
            row.get::<_, i64>(7)?.max(0) as u32,
            row.get(8)?,
            row.get(9)?,
            row.get(10)?,
            row.get(11)?,
            cards_json,
            claims_json,
        ))
    })?;
    rows.map(|row| {
        let (
            engine,
            engine_revision,
            provenance_schema_version,
            card_data_revision,
            scenario_schema_version,
            configuration_hash,
            scenario_id,
            scenario_version,
            seed,
            passed,
            outcome_json,
            event_hash,
            cards_json,
            claims_json,
        ) = row?;
        Ok(StoredObservation {
            engine,
            engine_revision,
            provenance_schema_version,
            card_data_revision,
            scenario_schema_version,
            configuration_hash,
            scenario_id,
            scenario_version,
            seed,
            passed,
            outcome_json,
            event_hash,
            cards: serde_json::from_str(&cards_json)?,
            claims: serde_json::from_str(&claims_json).unwrap_or_default(),
        })
    })
    .collect()
}

fn database_path(data_dir: &Path) -> PathBuf {
    data_dir.join("conformance.sqlite")
}

fn open(data_dir: &Path) -> Result<Connection, ConformanceError> {
    fs::create_dir_all(data_dir)?;
    let mut connection = Connection::open(database_path(data_dir))?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS conformance_observations (
           engine TEXT NOT NULL,
           engine_revision TEXT NOT NULL,
           provenance_schema_version INTEGER NOT NULL,
           card_data_revision TEXT NOT NULL,
           scenario_schema_version INTEGER NOT NULL,
           configuration_hash TEXT NOT NULL,
           scenario_id TEXT NOT NULL,
           scenario_version INTEGER NOT NULL,
           seed INTEGER NOT NULL,
           passed INTEGER NOT NULL,
           outcome_json TEXT NOT NULL,
           event_hash TEXT NOT NULL,
           cards_json TEXT NOT NULL,
           claims_json TEXT NOT NULL DEFAULT '[]',
           producer TEXT NOT NULL,
           imported_at TEXT NOT NULL DEFAULT (datetime('now')),
           PRIMARY KEY (engine, engine_revision, card_data_revision, scenario_schema_version,
                        configuration_hash, scenario_id, scenario_version, seed)
         );",
    )?;
    let columns = connection
        .prepare("PRAGMA table_info(conformance_observations)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<BTreeSet<_>, _>>()?;
    if !columns.contains("provenance_schema_version") {
        let transaction = connection.transaction()?;
        transaction.execute_batch(
            "ALTER TABLE conformance_observations RENAME TO conformance_observations_legacy;
             CREATE TABLE conformance_observations (
               engine TEXT NOT NULL,
               engine_revision TEXT NOT NULL,
               provenance_schema_version INTEGER NOT NULL,
               card_data_revision TEXT NOT NULL,
               scenario_schema_version INTEGER NOT NULL,
               configuration_hash TEXT NOT NULL,
               scenario_id TEXT NOT NULL,
               scenario_version INTEGER NOT NULL,
               seed INTEGER NOT NULL,
               passed INTEGER NOT NULL,
               outcome_json TEXT NOT NULL,
               event_hash TEXT NOT NULL,
               cards_json TEXT NOT NULL,
               claims_json TEXT NOT NULL DEFAULT '[]',
               producer TEXT NOT NULL,
               imported_at TEXT NOT NULL DEFAULT (datetime('now')),
               PRIMARY KEY (engine, engine_revision, card_data_revision,
                            scenario_schema_version, configuration_hash,
                            scenario_id, scenario_version, seed)
             );
             INSERT INTO conformance_observations
               (engine, engine_revision, provenance_schema_version, card_data_revision,
                scenario_schema_version, configuration_hash, scenario_id, scenario_version,
                seed, passed, outcome_json, event_hash, cards_json, claims_json, producer, imported_at)
             SELECT engine, engine_revision, 0, 'legacy-unknown', 0, 'legacy-unknown',
                    scenario_id, scenario_version, seed, passed, outcome_json, event_hash,
                    cards_json, '[]', producer, imported_at
             FROM conformance_observations_legacy;
             DROP TABLE conformance_observations_legacy;",
        )?;
        transaction.commit()?;
    }
    let columns = connection
        .prepare("PRAGMA table_info(conformance_observations)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<BTreeSet<_>, _>>()?;
    if !columns.contains("claims_json") {
        connection.execute(
            "ALTER TABLE conformance_observations ADD COLUMN claims_json TEXT NOT NULL DEFAULT '[]'",
            [],
        )?;
    }
    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS conformance_scenario
           ON conformance_observations(
             scenario_schema_version, scenario_id, scenario_version, seed, configuration_hash
           );",
    )?;
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    fn observation(engine: &str, event_hash: &str, cards: &[&str]) -> ConformanceObservation {
        ConformanceObservation {
            engine: engine.into(),
            engine_revision: "revision-1".into(),
            provenance: None,
            scenario_id: "tokens.parallel-lives".into(),
            scenario_version: 1,
            seed: 42,
            passed: true,
            outcome: json!({"tokens": 2, "permanents": {"parallel_lives": 3}}),
            event_hash: event_hash.into(),
            cards: cards.iter().map(|name| (*name).into()).collect(),
            claims: Vec::new(),
        }
    }

    fn add_provenance(
        mut observation: ConformanceObservation,
        card_data_revision: &str,
        configuration_hash: &str,
    ) -> ConformanceObservation {
        observation.provenance = Some(crate::provenance::ResultProvenance::new(
            observation.engine_revision.clone(),
            card_data_revision,
            Some(1),
            configuration_hash,
        ));
        observation
    }

    #[test]
    fn provenance_separates_configurations_and_allows_independent_card_revisions() {
        let directory =
            std::env::temp_dir().join(format!("metagene-provenance-{}", Uuid::new_v4()));
        let hexdek = add_provenance(
            observation("hexdek", "sha256:agree", &["Parallel Lives"]),
            "scryfall:one",
            "sha256:config-a",
        );
        let forge = add_provenance(
            observation("forge", "sha256:agree", &["Parallel Lives"]),
            "forge-cards:two",
            "sha256:config-a",
        );
        assert_eq!(
            compare_observations(&hexdek, &forge)
                .unwrap()
                .classification,
            ComparisonClassification::VerifiedAgreement
        );
        let changed = add_provenance(
            observation("hexdek", "sha256:agree", &["Parallel Lives"]),
            "scryfall:one",
            "sha256:config-b",
        );
        assert!(matches!(
            compare_observations(&hexdek, &changed),
            Err(ConformanceError::MismatchedComparison)
        ));
        import_bundle(
            &directory,
            &ConformanceBundle {
                schema_version: 1,
                producer: "test".into(),
                observations: vec![hexdek, changed],
            },
        )
        .unwrap();
        let summary = summary(&directory).unwrap();
        assert_eq!(summary.observations, 2);
        assert_eq!(summary.scenario_runs, 2);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn schema_v2_verifies_only_explicit_action_claims() {
        let directory =
            std::env::temp_dir().join(format!("metagene-scoped-conformance-{}", Uuid::new_v4()));
        let mut scoped = observation("hexdek", "sha256:scoped", &["Parallel Lives"]);
        scoped.claims = vec![ConformanceClaim {
            card_name: "Parallel Lives".into(),
            subject_id: "oc-token-replacement".into(),
            assertion: "replacement doubled the token event".into(),
            ai_decision: true,
        }];
        import_bundle(
            &directory,
            &ConformanceBundle {
                schema_version: 2,
                producer: "scoped test".into(),
                observations: vec![scoped],
            },
        )
        .unwrap();
        let report = audit_cards(&directory, &["Parallel Lives".into()]).unwrap();
        assert_eq!(report.cards[0].claims.len(), 1);
        assert_eq!(report.cards[0].claims[0].subject_id, "oc-token-replacement");
        assert!(report.cards[0].claims[0].ai_decision_covered);
        assert!(
            report.cards[0]
                .claims
                .iter()
                .all(|claim| claim.subject_id != "oc-unrelated-static")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn schema_v1_claim_fields_remain_legacy_card_level_only() {
        let directory =
            std::env::temp_dir().join(format!("metagene-legacy-claim-{}", Uuid::new_v4()));
        let mut legacy = observation("hexdek", "sha256:legacy", &["Parallel Lives"]);
        legacy.claims = vec![ConformanceClaim {
            card_name: "Parallel Lives".into(),
            subject_id: "must-not-promote".into(),
            assertion: "legacy payload attempted an action claim".into(),
            ai_decision: true,
        }];
        import_bundle(
            &directory,
            &ConformanceBundle {
                schema_version: 1,
                producer: "legacy test".into(),
                observations: vec![legacy],
            },
        )
        .unwrap();
        let report = audit_cards(&directory, &["Parallel Lives".into()]).unwrap();
        assert!(report.cards[0].claims.is_empty());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn migrates_legacy_observations_without_claiming_modern_provenance() {
        let directory =
            std::env::temp_dir().join(format!("metagene-legacy-conformance-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let connection = Connection::open(database_path(&directory)).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE conformance_observations (
                   engine TEXT NOT NULL, engine_revision TEXT NOT NULL,
                   scenario_id TEXT NOT NULL, scenario_version INTEGER NOT NULL,
                   seed INTEGER NOT NULL, passed INTEGER NOT NULL,
                   outcome_json TEXT NOT NULL, event_hash TEXT NOT NULL,
                   cards_json TEXT NOT NULL, producer TEXT NOT NULL,
                   imported_at TEXT NOT NULL DEFAULT (datetime('now')),
                   PRIMARY KEY (engine, engine_revision, scenario_id, scenario_version, seed)
                 );
                 INSERT INTO conformance_observations
                   (engine, engine_revision, scenario_id, scenario_version, seed, passed,
                    outcome_json, event_hash, cards_json, producer)
                 VALUES ('hexdek','old','legacy.scenario',1,7,1,'{}','sha256:old','[]','old');",
            )
            .unwrap();
        drop(connection);
        let connection = open(&directory).unwrap();
        let migrated = connection
            .query_row(
                "SELECT provenance_schema_version, card_data_revision,
                        scenario_schema_version, configuration_hash
                 FROM conformance_observations",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            migrated,
            (0, "legacy-unknown".into(), 0, "legacy-unknown".into())
        );
        drop(connection);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn imports_upserts_and_cross_checks_independent_engines() {
        let directory =
            std::env::temp_dir().join(format!("metagene-conformance-{}", Uuid::new_v4()));
        let first = ConformanceBundle {
            schema_version: 1,
            producer: "test harness".into(),
            observations: vec![observation("hexdek", "sha256:agree", &["Parallel Lives"])],
        };
        import_bundle(&directory, &first).unwrap();
        import_bundle(&directory, &first).unwrap();
        assert_eq!(summary(&directory).unwrap().observations, 1);

        let second = ConformanceBundle {
            schema_version: 1,
            producer: "independent harness".into(),
            observations: vec![observation("forge", "sha256:agree", &["Parallel Lives"])],
        };
        let imported = import_bundle(&directory, &second).unwrap();
        assert_eq!(imported.summary.cross_engine_verified_scenarios, 1);
        let audit = audit_cards(
            &directory,
            &["Parallel Lives".into(), "Mythos of Illuna".into()],
        )
        .unwrap();
        assert_eq!(audit.cross_engine_verified_unique, 1);
        assert_eq!(audit.verified_unique, 1);
        assert_eq!(audit.unverified_unique, 1);
        assert_eq!(audit.cards[0].status, "unverified");
        assert_eq!(audit.cards[1].status, "cross_engine_verified");
        assert_eq!(audit.cards[1].classification, "verified");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn event_disagreement_is_a_conflict() {
        let directory =
            std::env::temp_dir().join(format!("metagene-conformance-{}", Uuid::new_v4()));
        let mut hexdek = observation("hexdek", "sha256:a", &["Mythos of Illuna"]);
        hexdek.outcome = json!({"b": 2, "a": 1});
        let mut forge = observation("forge", "sha256:b", &["Mythos of Illuna"]);
        forge.outcome = json!({"a": 1, "b": 2});
        import_bundle(
            &directory,
            &ConformanceBundle {
                schema_version: 1,
                producer: "test".into(),
                observations: vec![hexdek, forge],
            },
        )
        .unwrap();
        let audit = audit_cards(&directory, &["Mythos of Illuna".into()]).unwrap();
        assert_eq!(audit.conflicting_unique, 1);
        assert_eq!(audit.disputed_unique, 1);
        assert_eq!(audit.cards[0].classification, "disputed");
        let report = audit_decks(
            &directory,
            &[("Candidate".into(), vec!["Mythos of Illuna".into()])],
        )
        .unwrap();
        assert_eq!(report.scenarios.len(), 1);
        assert_eq!(report.scenarios[0].classification, "disputed");
        assert_eq!(report.scenarios[0].engines, vec!["forge", "hexdek"]);
        assert_eq!(summary(&directory).unwrap().conflicting_scenarios, 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn canonical_json_key_order_agrees_but_failed_scenarios_do_not_verify() {
        let directory =
            std::env::temp_dir().join(format!("metagene-conformance-{}", Uuid::new_v4()));
        let mut hexdek = observation("hexdek", "sha256:same", &["Parallel Lives"]);
        hexdek.cards.push("Doubling Season".into());
        hexdek.outcome = json!({"tokens": 2, "state": {"b": 2, "a": 1}});
        let mut forge = observation("forge", "sha256:same", &["Parallel Lives"]);
        forge.outcome = json!({"state": {"a": 1, "b": 2}, "tokens": 2});
        import_bundle(
            &directory,
            &ConformanceBundle {
                schema_version: 1,
                producer: "test".into(),
                observations: vec![hexdek, forge],
            },
        )
        .unwrap();
        assert_eq!(
            summary(&directory).unwrap().cross_engine_verified_scenarios,
            1
        );
        let metadata_audit = audit_cards(
            &directory,
            &["Parallel Lives".into(), "Doubling Season".into()],
        )
        .unwrap();
        assert_eq!(metadata_audit.cross_engine_verified_unique, 1);
        assert_eq!(metadata_audit.interaction_verified_unique, 1);

        let mut failed = observation("hexdek", "sha256:failed", &["Mythos of Illuna"]);
        failed.scenario_id = "copy.characteristics".into();
        failed.passed = false;
        import_bundle(
            &directory,
            &ConformanceBundle {
                schema_version: 1,
                producer: "test".into(),
                observations: vec![failed],
            },
        )
        .unwrap();
        let audit = audit_cards(&directory, &["Mythos of Illuna".into()]).unwrap();
        assert_eq!(audit.failed_unique, 1);
        assert_eq!(audit.unsupported_unique, 1);
        assert_eq!(audit.cards[0].classification, "unsupported");
        assert_eq!(summary(&directory).unwrap().failed_scenarios, 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_unknown_schema_and_empty_observations() {
        let directory =
            std::env::temp_dir().join(format!("metagene-conformance-{}", Uuid::new_v4()));
        let empty = ConformanceBundle {
            schema_version: 3,
            producer: "test".into(),
            observations: Vec::new(),
        };
        assert!(matches!(
            import_bundle(&directory, &empty),
            Err(ConformanceError::UnsupportedSchema(3))
        ));
        let empty_current = ConformanceBundle {
            schema_version: CONFORMANCE_SCHEMA_VERSION,
            producer: "test".into(),
            observations: Vec::new(),
        };
        assert!(matches!(
            import_bundle(&directory, &empty_current),
            Err(ConformanceError::EmptyBundle)
        ));
        std::fs::remove_dir_all(directory).ok();
    }
}
