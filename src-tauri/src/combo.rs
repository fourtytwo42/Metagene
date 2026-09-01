use metagene_core::{
    AbilityKind, AbilityProfile, AbilityTiming, AbilityZone, AmountExpression, Card, CardId,
    CardPool, Color, ComboPackageProfile, Deck, RuleCost, RuleEffect, RuleTarget, RuleTiming,
};
use reqwest::blocking::Client;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SCHEMA_VERSION: u32 = 3;
const DEFAULT_SOURCE: &str = "https://backend.commanderspellbook.com/variants/?limit=100";
const DEFAULT_MAX_PIECES: usize = 5;
const DEFAULT_MAX_ACTIONS: usize = 24;
const DEFAULT_MAX_STATES: usize = 100_000;
const DEFAULT_TIME_LIMIT_MS: u64 = 2_000;
const MAX_STORED_PROOFS: usize = 10_000;
const MAX_CONFORMANCE_OBSERVATIONS: usize = 25_000;
const HEXDEK_PLAN_SCHEMA_VERSION: u32 = 1;
const COMBO_PROOF_REVISION: &str = "rules-ir-exact-state-v3";
const BEAM_WIDTH: usize = 256;

const MANA: usize = 0;
const CARDS: usize = 1;
const PERMANENTS: usize = 2;
const TOKENS: usize = 3;
const LIFE: usize = 4;
const GRAVEYARD: usize = 5;
const COUNTERS: usize = 6;
const DAMAGE: usize = 7;
const MILL: usize = 8;
const RESOURCE_COUNT: usize = 9;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ComboProof {
    #[default]
    CorpusOnly,
    /// Preserved audit state for a pre-truth-gate resource proof.
    LegacyResourceHypothesis,
    /// Mana/resource accounting only. This is not exact-state proof and may
    /// not guide the game AI or support an Improved claim.
    ResourceBalancedHypothesis,
    /// Costs, targets, zones, identity, colored mana, tap/once state, and
    /// another legal iteration were replayed in the typed abstract state.
    StateProven,
    /// Legacy serialized spelling retained only for migration.
    SymbolicallyProven,
    HexdekExecuted,
    CrossEngineVerified,
    ArenaVerified,
    Disputed,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ComboActionKind {
    #[default]
    Unknown,
    Draw,
    Tutor,
    Recursion,
    Copy,
    Replacement,
    CostReduction,
    Untap,
    Mana,
    Token,
    Counter,
    Damage,
    Mill,
    Life,
    Sacrifice,
    Blink,
    Protection,
    AlternateWin,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(default)]
pub struct ComboResources {
    pub mana: i16,
    pub cards: i16,
    pub permanents: i16,
    pub tokens: i16,
    pub life: i16,
    pub graveyard: i16,
    pub counters: i16,
    pub damage: i16,
    pub mill: i16,
}

impl ComboResources {
    fn array(&self) -> [i16; RESOURCE_COUNT] {
        [
            self.mana,
            self.cards,
            self.permanents,
            self.tokens,
            self.life,
            self.graveyard,
            self.counters,
            self.damage,
            self.mill,
        ]
    }

    fn from_array(values: [i16; RESOURCE_COUNT]) -> Self {
        Self {
            mana: values[MANA],
            cards: values[CARDS],
            permanents: values[PERMANENTS],
            tokens: values[TOKENS],
            life: values[LIFE],
            graveyard: values[GRAVEYARD],
            counters: values[COUNTERS],
            damage: values[DAMAGE],
            mill: values[MILL],
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ComboAction {
    pub id: String,
    pub card_id: CardId,
    pub card_name: String,
    pub mode_name: String,
    #[serde(default)]
    pub source_types: Vec<String>,
    #[serde(default)]
    pub source_subtypes: Vec<String>,
    pub label: String,
    pub kind: ComboActionKind,
    pub timing: AbilityTiming,
    pub zone: AbilityZone,
    pub cost: ComboResources,
    pub effect: ComboResources,
    pub uses_tap: bool,
    pub consumes_self: bool,
    pub once_per_turn: bool,
    pub parser_confidence: f32,
    pub conditions: Vec<String>,
    #[serde(default)]
    pub rule_timing: RuleTiming,
    #[serde(default)]
    pub rule_costs: Vec<RuleCost>,
    #[serde(default)]
    pub rule_targets: Vec<RuleTarget>,
    #[serde(default)]
    pub rule_effects: Vec<RuleEffect>,
    #[serde(default)]
    pub rule_delayed_effects: Vec<RuleEffect>,
    #[serde(default)]
    pub exact_state_supported: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ComboState {
    pub resources: ComboResources,
    pub colored_mana: [i16; 6],
    pub objects: Vec<ComboObjectState>,
    pub life_by_player: Vec<i16>,
    pub tapped_cards: Vec<String>,
    pub consumed_cards: Vec<String>,
    pub once_per_turn_used: Vec<String>,
    pub delayed_triggers: Vec<String>,
    pub replacement_effects: Vec<String>,
    pub continuous_modifiers: Vec<String>,
    pub actions_taken: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ComboObjectState {
    pub instance_id: String,
    pub card_id: String,
    pub card_name: String,
    pub controller: u8,
    pub zone: AbilityZone,
    pub types: Vec<String>,
    pub subtypes: Vec<String>,
    pub token: bool,
    pub tapped: bool,
    pub summoning_sick: bool,
    pub counters: Vec<(String, i16)>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExactStateEvidence {
    pub proof_revision: String,
    pub initial_state_hash: String,
    pub final_state_hash: String,
    pub source_state_restored: bool,
    pub another_iteration_legal: bool,
    pub applied_actions: usize,
    pub failure_reasons: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComboLine {
    pub id: String,
    pub corpus_id: Option<String>,
    pub cards: Vec<String>,
    pub actions: Vec<ComboAction>,
    pub proof: ComboProof,
    pub finite_win: bool,
    pub repeatable: bool,
    pub loop_output: ComboResources,
    pub minimum_resources: ComboResources,
    pub expected_turn: Option<f32>,
    pub setup: Vec<String>,
    pub disruption_points: Vec<String>,
    pub dead_pieces: Vec<String>,
    pub unsupported_assumptions: Vec<String>,
    pub explored_states: usize,
    pub elapsed_ms: u64,
    #[serde(default)]
    pub exact_state: Option<ExactStateEvidence>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComboConformanceScenario {
    pub schema_version: u32,
    pub scenario_id: String,
    pub line_id: String,
    pub proof_revision: String,
    pub cards: Vec<String>,
    pub action_ids: Vec<String>,
    pub initial_resources: ComboResources,
    pub expected_class: String,
    pub expected_terminal_hash: String,
    pub maximum_actions: usize,
    pub deterministic: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ComboConformanceObservationRequest {
    pub deck: Deck,
    pub line_id: String,
    pub scenario_id: String,
    pub engine: String,
    pub engine_revision: String,
    pub card_data_revision: String,
    pub terminal_state_hash: String,
    pub completed: bool,
    pub critical_mechanics_supported: bool,
    pub passed: bool,
    pub detail: String,
}

impl Default for ComboConformanceObservationRequest {
    fn default() -> Self {
        Self {
            deck: Deck {
                name: String::new(),
                command_zone: Vec::new(),
                mainboard: Vec::new(),
                sideboard: Vec::new(),
            },
            line_id: String::new(),
            scenario_id: String::new(),
            engine: String::new(),
            engine_revision: String::new(),
            card_data_revision: String::new(),
            terminal_state_hash: String::new(),
            completed: false,
            critical_mechanics_supported: false,
            passed: false,
            detail: String::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComboConformanceObservation {
    pub scenario_id: String,
    pub line_id: String,
    pub engine: String,
    pub engine_revision: String,
    pub card_data_revision: String,
    pub expected_terminal_hash: String,
    pub terminal_state_hash: String,
    pub outcome: String,
    pub detail: String,
    pub recorded_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComboConformanceReport {
    pub scenario: ComboConformanceScenario,
    pub proof: ComboProof,
    pub observations: Vec<ComboConformanceObservation>,
    pub decision: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct HexdekComboPlanFile {
    pub schema_version: u32,
    pub decks: Vec<HexdekDeckComboPlan>,
}

#[derive(Clone, Debug, Serialize)]
pub struct HexdekDeckComboPlan {
    pub deck: String,
    pub proof_revision: String,
    pub plans: Vec<HexdekComboLine>,
    pub tutor_targets: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct HexdekComboLine {
    pub pieces: Vec<String>,
    #[serde(rename = "type")]
    pub line_type: String,
    pub class: String,
    pub cast_order: Vec<String>,
    pub proof: ComboProof,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ComboPackage {
    pub line_id: String,
    pub engine: Vec<String>,
    pub enablers: Vec<String>,
    pub payoffs: Vec<String>,
    pub tutors: Vec<String>,
    pub protection: Vec<String>,
    pub recovery: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComboProofReport {
    pub schema_version: u32,
    #[serde(default)]
    pub proof_revision: String,
    pub deck_name: String,
    pub corpus_revision: Option<String>,
    pub corpus_hypotheses: usize,
    pub lines: Vec<ComboLine>,
    pub packages: Vec<ComboPackage>,
    pub explored_states: usize,
    pub elapsed_ms: u64,
    pub truncated: bool,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComboCorpusStatus {
    pub schema_version: u32,
    pub source: Option<String>,
    pub revision: Option<String>,
    pub synced_at_unix: Option<u64>,
    pub hypotheses: usize,
    pub fully_resolved: usize,
    pub database_bytes: u64,
    pub conformance_observations: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ComboCorpusSyncRequest {
    pub source_url: String,
    pub max_entries: usize,
}

impl Default for ComboCorpusSyncRequest {
    fn default() -> Self {
        Self {
            source_url: DEFAULT_SOURCE.into(),
            max_entries: 100_000,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComboCorpusSyncReport {
    pub status: ComboCorpusStatus,
    pub downloaded: usize,
    pub resolved: usize,
    pub unresolved_card_names: Vec<String>,
    pub source_pages: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ComboSearchLimits {
    pub max_pieces: usize,
    pub max_actions: usize,
    pub max_states: usize,
    pub time_limit_ms: u64,
}

impl Default for ComboSearchLimits {
    fn default() -> Self {
        Self {
            max_pieces: DEFAULT_MAX_PIECES,
            max_actions: DEFAULT_MAX_ACTIONS,
            max_states: DEFAULT_MAX_STATES,
            time_limit_ms: DEFAULT_TIME_LIMIT_MS,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComboProofRequest {
    pub deck: Deck,
    pub pool: CardPool,
    #[serde(default)]
    pub limits: ComboSearchLimits,
}

#[derive(Clone)]
struct CorpusHypothesis {
    id: String,
    cards: Vec<String>,
    description: String,
    status: String,
}

pub fn status(data_dir: &Path) -> Result<ComboCorpusStatus, String> {
    let path = database_path(data_dir);
    if !path.is_file() {
        return Ok(ComboCorpusStatus {
            schema_version: SCHEMA_VERSION,
            source: None,
            revision: None,
            synced_at_unix: None,
            hypotheses: 0,
            fully_resolved: 0,
            database_bytes: 0,
            conformance_observations: 0,
        });
    }
    let connection = open(data_dir)?;
    let meta = |key: &str| -> Result<Option<String>, String> {
        connection
            .query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|error| error.to_string())
    };
    let hypotheses = connection
        .query_row("SELECT COUNT(*) FROM combos", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|error| error.to_string())? as usize;
    let fully_resolved = connection
        .query_row(
            "SELECT COUNT(*) FROM combos WHERE resolved = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| error.to_string())? as usize;
    let conformance_observations = connection
        .query_row("SELECT COUNT(*) FROM combo_conformance", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|error| error.to_string())? as usize;
    Ok(ComboCorpusStatus {
        schema_version: SCHEMA_VERSION,
        source: meta("source")?,
        revision: meta("revision")?,
        synced_at_unix: meta("synced_at")?.and_then(|value| value.parse().ok()),
        hypotheses,
        fully_resolved,
        database_bytes: fs::metadata(path).map_or(0, |metadata| metadata.len()),
        conformance_observations,
    })
}

pub fn synchronize(
    data_dir: &Path,
    request: &ComboCorpusSyncRequest,
    pool: &CardPool,
) -> Result<ComboCorpusSyncReport, String> {
    let source = if request.source_url.trim().is_empty() {
        DEFAULT_SOURCE
    } else {
        request.source_url.trim()
    };
    let max_entries = request.max_entries.clamp(1, 250_000);
    let client = Client::builder()
        .user_agent("Metagene/0.2 combo-corpus")
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|error| error.to_string())?;
    let mut next = Some(source.to_string());
    let mut values = Vec::new();
    let mut pages = 0usize;
    while let Some(url) = next.take() {
        if values.len() >= max_entries || pages >= 10_000 {
            break;
        }
        let page: Value = client
            .get(&url)
            .send()
            .and_then(|response| response.error_for_status())
            .map_err(|error| format!("Commander Spellbook request failed: {error}"))?
            .json()
            .map_err(|error| format!("Commander Spellbook response was invalid: {error}"))?;
        pages += 1;
        let entries = page
            .get("results")
            .or_else(|| page.get("data"))
            .and_then(Value::as_array)
            .ok_or("Commander Spellbook response has no results array.")?;
        values.extend(entries.iter().take(max_entries - values.len()).cloned());
        next = page
            .get("next")
            .and_then(Value::as_str)
            .filter(|url| !url.is_empty())
            .map(str::to_string);
    }
    replace_corpus(data_dir, source, &values, pool, pages)
}

fn replace_corpus(
    data_dir: &Path,
    source: &str,
    values: &[Value],
    pool: &CardPool,
    source_pages: usize,
) -> Result<ComboCorpusSyncReport, String> {
    let by_name = pool
        .cards
        .iter()
        .map(|card| (normalize_name(&card.name), card.id.0.clone()))
        .collect::<HashMap<_, _>>();
    let mut unresolved = BTreeSet::new();
    let mut records = Vec::new();
    for (index, value) in values.iter().enumerate() {
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("spellbook-{index}"));
        let cards = extract_card_names(value);
        if cards.len() < 2 {
            continue;
        }
        let resolved_ids = cards
            .iter()
            .map(|name| by_name.get(&normalize_name(name)).cloned())
            .collect::<Vec<_>>();
        for (name, resolved) in cards.iter().zip(&resolved_ids) {
            if resolved.is_none() {
                unresolved.insert(name.clone());
            }
        }
        let resolved = resolved_ids.iter().all(Option::is_some);
        let status = value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let description = value
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        records.push((
            id,
            status.to_string(),
            description.to_string(),
            cards,
            resolved_ids,
            resolved,
            value.to_string(),
        ));
    }
    records.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    for record in &records {
        hasher.update(record.0.as_bytes());
        hasher.update(record.6.as_bytes());
    }
    let revision = format!("sha256:{:x}", hasher.finalize());
    let synced_at = unix_now();
    let synced_at_sql = synced_at.min(i64::MAX as u64) as i64;
    let mut connection = open(data_dir)?;
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute("DELETE FROM combo_cards", [])
        .map_err(|error| error.to_string())?;
    transaction
        .execute("DELETE FROM combos", [])
        .map_err(|error| error.to_string())?;
    for (id, status, description, cards, resolved_ids, resolved, payload) in &records {
        transaction
            .execute(
                "INSERT INTO combos(id, status, description, card_names_json, payload_json, resolved, updated_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![id, status, description, serde_json::to_string(cards).unwrap_or_default(), payload, i64::from(*resolved), synced_at_sql],
            )
            .map_err(|error| error.to_string())?;
        for (position, (name, card_id)) in cards.iter().zip(resolved_ids).enumerate() {
            transaction
                .execute(
                    "INSERT INTO combo_cards(combo_id, position, card_name, card_id) VALUES(?1, ?2, ?3, ?4)",
                    params![id, position as i64, name, card_id],
                )
                .map_err(|error| error.to_string())?;
        }
    }
    for (key, value) in [
        ("source", source.to_string()),
        ("revision", revision),
        ("synced_at", synced_at.to_string()),
    ] {
        transaction
            .execute(
                "INSERT INTO metadata(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())?;
    let corpus_status = status(data_dir)?;
    Ok(ComboCorpusSyncReport {
        downloaded: corpus_status.hypotheses,
        resolved: corpus_status.fully_resolved,
        status: corpus_status,
        unresolved_card_names: unresolved.into_iter().take(500).collect(),
        source_pages,
    })
}

pub fn prove(data_dir: &Path, request: &ComboProofRequest) -> Result<ComboProofReport, String> {
    let started = Instant::now();
    let limits = normalized_limits(&request.limits);
    let deck_cards = resolve_deck_cards(&request.deck, &request.pool);
    let deck_names = deck_cards
        .iter()
        .map(|card| normalize_name(&card.name))
        .collect::<HashSet<_>>();
    let hypotheses = load_hypotheses(data_dir)?
        .into_iter()
        .filter(|hypothesis| {
            hypothesis.cards.len() <= limits.max_pieces
                && hypothesis
                    .cards
                    .iter()
                    .all(|name| deck_names.contains(&normalize_name(name)))
        })
        .collect::<Vec<_>>();
    let all_actions = deck_cards
        .iter()
        .flat_map(|card| combo_actions(card))
        .collect::<Vec<_>>();
    let mut lines = Vec::new();
    let mut explored_states = 0usize;
    let mut truncated = false;
    for hypothesis in &hypotheses {
        if started.elapsed().as_millis() >= u128::from(limits.time_limit_ms) {
            truncated = true;
            break;
        }
        let names = hypothesis
            .cards
            .iter()
            .map(|name| normalize_name(name))
            .collect::<HashSet<_>>();
        let actions = all_actions
            .iter()
            .filter(|action| names.contains(&normalize_name(&action.card_name)))
            .cloned()
            .collect::<Vec<_>>();
        let mut solved = solve_actions(&actions, &limits, started);
        explored_states = explored_states.saturating_add(solved.explored);
        truncated |= solved.truncated;
        if solved.lines.is_empty() {
            lines.push(ComboLine {
                id: line_id(&hypothesis.cards, "corpus"),
                corpus_id: Some(hypothesis.id.clone()),
                cards: hypothesis.cards.clone(),
                actions: Vec::new(),
                proof: if hypothesis.status.eq_ignore_ascii_case("invalid") {
                    ComboProof::Disputed
                } else {
                    ComboProof::CorpusOnly
                },
                finite_win: false,
                repeatable: false,
                loop_output: ComboResources::default(),
                minimum_resources: ComboResources::default(),
                expected_turn: None,
                setup: vec![hypothesis.description.clone()]
                    .into_iter()
                    .filter(|value| !value.is_empty())
                    .collect(),
                disruption_points: Vec::new(),
                dead_pieces: hypothesis.cards.clone(),
                unsupported_assumptions: vec![
                    "Corpus entry is a hypothesis; Metagene did not prove another legal iteration."
                        .into(),
                ],
                explored_states: solved.explored,
                elapsed_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                exact_state: None,
            });
        } else {
            for line in &mut solved.lines {
                line.corpus_id = Some(hypothesis.id.clone());
            }
            lines.extend(solved.lines);
        }
    }
    if started.elapsed().as_millis() < u128::from(limits.time_limit_ms) {
        let solved = solve_actions(&all_actions, &limits, started);
        explored_states = explored_states.saturating_add(solved.explored);
        truncated |= solved.truncated;
        lines.extend(solved.lines);
    } else {
        truncated = true;
    }
    deduplicate_lines(&mut lines);
    apply_conformance_proofs(data_dir, &request.deck, &mut lines)?;
    let packages = lines.iter().map(package_for_line).collect::<Vec<_>>();
    let corpus_status = status(data_dir)?;
    let report = ComboProofReport {
        schema_version: SCHEMA_VERSION,
        proof_revision: COMBO_PROOF_REVISION.into(),
        deck_name: request.deck.name.clone(),
        corpus_revision: corpus_status.revision,
        corpus_hypotheses: hypotheses.len(),
        lines,
        packages,
        explored_states,
        elapsed_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        truncated,
        warnings: truncated
            .then(|| {
                "Combo search reached its state or time budget; unreported lines remain unknown."
                    .into()
            })
            .into_iter()
            .collect(),
    };
    persist_proof(data_dir, &request.deck, &report)?;
    Ok(report)
}

/// Build a compact, proof-gated strategy overlay for the staged HexDek deck
/// files. This intentionally excludes corpus-only hypotheses: tournament AI is
/// allowed to prioritize only lines whose resource cycle was symbolically
/// proven under the bounded action search.
pub fn hexdek_plan_for_decks(decks: &[(&str, &Deck)], pool: &CardPool) -> HexdekComboPlanFile {
    let mut planned = Vec::new();
    for (deck_file, deck) in decks {
        let started = Instant::now();
        let actions = resolve_deck_cards(deck, pool)
            .into_iter()
            .flat_map(combo_actions)
            .collect::<Vec<_>>();
        let limits = ComboSearchLimits {
            time_limit_ms: 250,
            ..ComboSearchLimits::default()
        };
        let solved = solve_actions(&actions, &limits, started);
        let mut plans = solved
            .lines
            .into_iter()
            .filter(|line| {
                proof_is_plan_eligible(line.proof)
                    && line.cards.len() >= 2
                    && line.cards.len() <= limits.max_pieces
            })
            .map(|line| {
                let class = combo_output_class(&line);
                let mut cast_order = Vec::new();
                for action in &line.actions {
                    if !cast_order
                        .iter()
                        .any(|name: &String| name.eq_ignore_ascii_case(&action.card_name))
                    {
                        cast_order.push(action.card_name.clone());
                    }
                }
                for card in &line.cards {
                    if !cast_order
                        .iter()
                        .any(|name| name.eq_ignore_ascii_case(card))
                    {
                        cast_order.push(card.clone());
                    }
                }
                HexdekComboLine {
                    pieces: line.cards,
                    line_type: if line.repeatable {
                        "infinite".into()
                    } else if line.finite_win {
                        "finisher".into()
                    } else {
                        "determined".into()
                    },
                    class,
                    cast_order,
                    proof: line.proof,
                }
            })
            .collect::<Vec<_>>();
        plans.sort_by(|left, right| {
            left.pieces
                .len()
                .cmp(&right.pieces.len())
                .then_with(|| left.class.cmp(&right.class))
        });
        plans.dedup_by(|left, right| {
            left.pieces
                .iter()
                .map(|card| normalize_name(card))
                .collect::<BTreeSet<_>>()
                == right
                    .pieces
                    .iter()
                    .map(|card| normalize_name(card))
                    .collect::<BTreeSet<_>>()
        });
        if plans.is_empty() {
            continue;
        }
        let tutor_targets = plans
            .iter()
            .flat_map(|plan| plan.pieces.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        planned.push(HexdekDeckComboPlan {
            deck: (*deck_file).into(),
            proof_revision: COMBO_PROOF_REVISION.into(),
            plans,
            tutor_targets,
        });
    }
    HexdekComboPlanFile {
        schema_version: HEXDEK_PLAN_SCHEMA_VERSION,
        decks: planned,
    }
}

/// Precompute causal package profiles once per launch so the evolutionary hot
/// loop can score assembly/redundancy and mutate functional alternatives
/// without rerunning symbolic search for every candidate.
pub fn evolution_profiles(deck: &Deck, pool: &CardPool) -> Vec<ComboPackageProfile> {
    let started = Instant::now();
    let actions = resolve_deck_cards(deck, pool)
        .into_iter()
        .flat_map(combo_actions)
        .collect::<Vec<_>>();
    let limits = ComboSearchLimits {
        time_limit_ms: 500,
        ..ComboSearchLimits::default()
    };
    let solved = solve_actions(&actions, &limits, started);
    let by_name = pool
        .cards
        .iter()
        .map(|card| (normalize_name(&card.name), card.id.clone()))
        .collect::<HashMap<_, _>>();
    let ids = |names: &[String]| {
        let mut ids = names
            .iter()
            .filter_map(|name| by_name.get(&normalize_name(name)).cloned())
            .collect::<Vec<_>>();
        ids.sort_by(|left, right| left.0.cmp(&right.0));
        ids.dedup();
        ids
    };
    let mut profiles = solved
        .lines
        .into_iter()
        .filter(|line| {
            proof_is_plan_eligible(line.proof)
                && line.cards.len() >= 2
                && line.cards.len() <= limits.max_pieces
        })
        .filter_map(|line| {
            let piece_ids = ids(&line.cards);
            if piece_ids.len() < 2 {
                return None;
            }
            let package = package_for_line(&line);
            let required_roles = line
                .actions
                .iter()
                .filter_map(|action| combo_role(action.kind))
                .map(str::to_string)
                .collect::<Vec<_>>();
            (required_roles.len() >= 2).then(|| ComboPackageProfile {
                id: line.id,
                piece_ids,
                tutor_ids: ids(&package.tutors),
                protection_ids: ids(&package.protection),
                recovery_ids: ids(&package.recovery),
                required_roles,
                expected_turn: line.expected_turn,
                repeatable: line.repeatable,
                finite_win: line.finite_win,
                winning: line.finite_win
                    || line.loop_output.damage > 0
                    || line.loop_output.mill > 0
                    || line.loop_output.tokens > 0
                    || line.loop_output.permanents > 0,
                proof: combo_proof_label(line.proof).into(),
            })
        })
        .collect::<Vec<_>>();
    profiles.sort_by(|left, right| {
        left.piece_ids
            .len()
            .cmp(&right.piece_ids.len())
            .then_with(|| left.id.cmp(&right.id))
    });
    profiles.dedup_by(|left, right| left.id == right.id);
    profiles
}

fn combo_role(kind: ComboActionKind) -> Option<&'static str> {
    match kind {
        ComboActionKind::Draw => Some("draw"),
        ComboActionKind::Tutor => Some("tutor"),
        ComboActionKind::Recursion => Some("recursion"),
        ComboActionKind::Copy => Some("copy"),
        ComboActionKind::Replacement => Some("replacement"),
        ComboActionKind::CostReduction => Some("cost_reduction"),
        ComboActionKind::Untap => Some("untap"),
        ComboActionKind::Mana => Some("mana"),
        ComboActionKind::Token => Some("token"),
        ComboActionKind::Counter => Some("counter"),
        ComboActionKind::Damage => Some("damage"),
        ComboActionKind::Mill => Some("mill"),
        ComboActionKind::Life => Some("life"),
        ComboActionKind::Sacrifice => Some("sacrifice"),
        ComboActionKind::Blink => Some("blink"),
        ComboActionKind::Protection => Some("protection"),
        ComboActionKind::AlternateWin => Some("alternate_win"),
        ComboActionKind::Unknown => None,
    }
}

fn combo_output_class(line: &ComboLine) -> String {
    let output = &line.loop_output;
    let prefix = if line.repeatable {
        "infinite_"
    } else {
        "finite_"
    };
    let kind = if output.damage > 0 {
        "damage"
    } else if output.mill > 0 {
        "mill"
    } else if output.mana > 0 {
        "mana"
    } else if output.tokens > 0 || output.permanents > 0 {
        "tokens"
    } else if output.cards > 0 {
        "draw"
    } else if output.life > 0 {
        "life"
    } else if line.finite_win {
        "alternate_win"
    } else {
        "value"
    };
    format!("{prefix}{kind}")
}

struct SolveResult {
    lines: Vec<ComboLine>,
    explored: usize,
    truncated: bool,
}

fn solve_actions(
    actions: &[ComboAction],
    limits: &ComboSearchLimits,
    started: Instant,
) -> SolveResult {
    let useful = actions
        .iter()
        .filter(|action| action.parser_confidence >= 0.65)
        .take(128)
        .cloned()
        .collect::<Vec<_>>();
    let components = strongly_connected_components(&useful);
    let mut lines = Vec::new();
    let mut explored = 0usize;
    let mut truncated = false;
    for component in components {
        if explored >= limits.max_states
            || started.elapsed().as_millis() >= u128::from(limits.time_limit_ms)
        {
            truncated = true;
            break;
        }
        let component_actions = component
            .iter()
            .filter_map(|index| useful.get(*index))
            .take(limits.max_actions)
            .cloned()
            .collect::<Vec<_>>();
        let cards = component_actions
            .iter()
            .map(|action| action.card_name.clone())
            .collect::<BTreeSet<_>>();
        if cards.is_empty() || cards.len() > limits.max_pieces {
            continue;
        }
        let remaining = limits.max_states.saturating_sub(explored);
        let searched = search_component(&component_actions, limits, remaining, started);
        explored = explored.saturating_add(searched.explored);
        truncated |= searched.truncated;
        lines.extend(searched.lines);
    }
    SolveResult {
        lines,
        explored,
        truncated,
    }
}

#[derive(Clone)]
struct BeamNode {
    sequence: Vec<usize>,
    used: Vec<bool>,
    score: i32,
}

fn search_component(
    actions: &[ComboAction],
    limits: &ComboSearchLimits,
    remaining_states: usize,
    started: Instant,
) -> SolveResult {
    if actions.is_empty() || remaining_states == 0 {
        return SolveResult {
            lines: Vec::new(),
            explored: 0,
            truncated: remaining_states == 0,
        };
    }
    let mut frontier = vec![BeamNode {
        sequence: Vec::new(),
        used: vec![false; actions.len()],
        score: 0,
    }];
    let mut lines = Vec::new();
    let mut seen_lines = HashSet::new();
    let mut explored = 0usize;
    let mut truncated = false;
    let max_depth = actions.len().min(limits.max_actions);
    for _depth in 0..max_depth {
        let mut next = Vec::new();
        'nodes: for node in &frontier {
            for index in 0..actions.len() {
                if node.used[index] {
                    continue;
                }
                if explored >= remaining_states
                    || started.elapsed().as_millis() >= u128::from(limits.time_limit_ms)
                {
                    truncated = true;
                    break 'nodes;
                }
                let mut candidate = node.clone();
                candidate.sequence.push(index);
                candidate.used[index] = true;
                explored += 1;
                let ordered = candidate
                    .sequence
                    .iter()
                    .map(|position| actions[*position].clone())
                    .collect::<Vec<_>>();
                candidate.score = sequence_score(&ordered);
                if let Some(line) = prove_ordered_component(&ordered, explored, started)
                    && seen_lines.insert(line.id.clone())
                {
                    lines.push(line);
                }
                next.push(candidate);
            }
        }
        if next.is_empty() || truncated {
            break;
        }
        next.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.sequence.cmp(&right.sequence))
        });
        next.truncate(BEAM_WIDTH);
        frontier = next;
    }
    SolveResult {
        lines,
        explored,
        truncated,
    }
}

fn sequence_score(actions: &[ComboAction]) -> i32 {
    let mut net = [0i16; RESOURCE_COUNT];
    let mut minimum = [0i16; RESOURCE_COUNT];
    let mut balance = [0i16; RESOURCE_COUNT];
    for action in actions {
        let cost = action.cost.array();
        let effect = action.effect.array();
        for index in 0..RESOURCE_COUNT {
            minimum[index] = minimum[index].max(cost[index].saturating_sub(balance[index]));
            balance[index] = balance[index]
                .saturating_sub(cost[index])
                .saturating_add(effect[index]);
            net[index] = net[index]
                .saturating_sub(cost[index])
                .saturating_add(effect[index]);
        }
    }
    let output = net[DAMAGE].max(0) as i32 * 20
        + net[MILL].max(0) as i32 * 16
        + net[MANA].max(0) as i32 * 10
        + net[TOKENS].max(0) as i32 * 8
        + net[CARDS].max(0) as i32 * 6
        + net[LIFE].max(0) as i32 * 2;
    let deficit = minimum.iter().map(|value| (*value).max(0)).sum::<i16>() as i32;
    output * 100 + actions.len() as i32 * 4 - deficit
}

fn prove_ordered_component(
    actions: &[ComboAction],
    explored: usize,
    started: Instant,
) -> Option<ComboLine> {
    let finite_win = actions
        .iter()
        .any(|action| action.kind == ComboActionKind::AlternateWin);
    let once_blocked = actions.iter().any(|action| action.once_per_turn);
    let mut net = [0i16; RESOURCE_COUNT];
    let mut minimum = [0i16; RESOURCE_COUNT];
    let mut balance = [0i16; RESOURCE_COUNT];
    let mut tapped = BTreeSet::new();
    let mut consumed = BTreeSet::new();
    for action in actions {
        let card_key = normalize_name(&action.card_name);
        if consumed.contains(&card_key) && action.kind != ComboActionKind::Recursion {
            return None;
        }
        if action.uses_tap && !tapped.insert(card_key.clone()) {
            return None;
        }
        let cost = action.cost.array();
        let effect = action.effect.array();
        for index in 0..RESOURCE_COUNT {
            minimum[index] = minimum[index].max(cost[index].saturating_sub(balance[index]));
            balance[index] = balance[index]
                .saturating_sub(cost[index])
                .saturating_add(effect[index]);
            net[index] = net[index]
                .saturating_sub(cost[index])
                .saturating_add(effect[index]);
        }
        if action.consumes_self {
            consumed.insert(card_key.clone());
        }
        if action.kind == ComboActionKind::Untap
            && let Some(target) = tapped
                .iter()
                .find(|target| **target != card_key)
                .cloned()
                .or_else(|| tapped.iter().next().cloned())
        {
            tapped.remove(&target);
        }
        if action.kind == ComboActionKind::Recursion
            && let Some(target) = consumed
                .iter()
                .find(|target| **target != card_key)
                .cloned()
                .or_else(|| consumed.iter().next().cloned())
        {
            consumed.remove(&target);
        }
    }
    let resources_replenished = [MANA, CARDS, PERMANENTS, LIFE, GRAVEYARD, COUNTERS]
        .into_iter()
        .all(|index| net[index] >= 0);
    let meaningful_output = [
        MANA, CARDS, PERMANENTS, TOKENS, LIFE, COUNTERS, DAMAGE, MILL,
    ]
    .into_iter()
    .any(|index| net[index] > 0);
    let repeatable = !once_blocked
        && tapped.is_empty()
        && consumed.is_empty()
        && resources_replenished
        && meaningful_output;
    if !finite_win && !repeatable {
        return None;
    }
    if actions.iter().any(|action| action.parser_confidence < 0.75) {
        return None;
    }
    let cards = actions
        .iter()
        .map(|action| action.card_name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let minimum_resources = ComboResources::from_array(minimum);
    let expected_turn = Some(
        ((minimum_resources.mana.max(0) as f32) / 1.5)
            .ceil()
            .max(1.0),
    );
    let disruption_points = actions
        .iter()
        .filter(|action| {
            matches!(
                action.timing,
                AbilityTiming::Activated | AbilityTiming::Triggered | AbilityTiming::Spell
            )
        })
        .map(|action| format!("Respond to {} · {}", action.card_name, action.label))
        .take(8)
        .collect();
    let exact_state = Some(exact_state_proof(actions, repeatable, finite_win));
    let proof = if exact_state.as_ref().is_some_and(|evidence| {
        evidence.failure_reasons.is_empty()
            && evidence.applied_actions == actions.len()
            && (evidence.another_iteration_legal || finite_win)
    }) {
        ComboProof::StateProven
    } else {
        ComboProof::ResourceBalancedHypothesis
    };
    Some(ComboLine {
        id: line_id(&cards, if finite_win { "finite" } else { "loop" }),
        corpus_id: None,
        cards,
        actions: actions.to_vec(),
        proof,
        finite_win,
        repeatable,
        loop_output: ComboResources::from_array(net),
        minimum_resources,
        expected_turn,
        setup: actions
            .iter()
            .flat_map(|action| action.conditions.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        disruption_points,
        dead_pieces: Vec::new(),
        unsupported_assumptions: exact_state
            .as_ref()
            .map_or_else(Vec::new, |evidence| evidence.failure_reasons.clone()),
        explored_states: explored,
        elapsed_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        exact_state,
    })
}

fn exact_state_proof(
    actions: &[ComboAction],
    resource_repeatable: bool,
    finite_win: bool,
) -> ExactStateEvidence {
    let mut evidence = ExactStateEvidence {
        proof_revision: COMBO_PROOF_REVISION.into(),
        ..ExactStateEvidence::default()
    };
    if actions.is_empty() {
        evidence
            .failure_reasons
            .push("No actions were supplied.".into());
        return evidence;
    }
    for action in actions {
        if !action.exact_state_supported {
            evidence.failure_reasons.push(format!(
                "{} · {} contains an unresolved or unsupported exact-state relationship.",
                action.card_name, action.label
            ));
        }
    }
    if !evidence.failure_reasons.is_empty() {
        evidence.failure_reasons.sort();
        evidence.failure_reasons.dedup();
        return evidence;
    }
    let mut initial = initial_exact_state(actions);
    evidence.initial_state_hash = exact_state_hash(&initial);
    let mut final_state = initial.clone();
    for action in actions {
        if let Err(reason) = apply_exact_action(&mut final_state, action) {
            evidence.failure_reasons.push(reason);
            evidence.final_state_hash = exact_state_hash(&final_state);
            return evidence;
        }
        evidence.applied_actions += 1;
    }
    evidence.final_state_hash = exact_state_hash(&final_state);
    evidence.source_state_restored = source_state_restored(&initial, &final_state);
    if finite_win {
        evidence.another_iteration_legal = false;
        if !actions.iter().any(|action| {
            action
                .rule_effects
                .iter()
                .any(|effect| matches!(effect, RuleEffect::WinLoss { wins: true, .. }))
        }) {
            evidence.failure_reasons.push(
                "The resource summary claimed a win, but Rules IR contains no win effect.".into(),
            );
        }
        return evidence;
    }
    if !resource_repeatable || !evidence.source_state_restored {
        evidence.failure_reasons.push(
            "The post-line state does not restore every source identity, zone, tap, counter, once-per-turn, and colored-mana prerequisite."
                .into(),
        );
        return evidence;
    }
    let mut another = final_state.clone();
    evidence.another_iteration_legal = actions
        .iter()
        .all(|action| apply_exact_action(&mut another, action).is_ok());
    if !evidence.another_iteration_legal {
        evidence.failure_reasons.push(
            "A complete second iteration is not legal from the canonical post-line state.".into(),
        );
    }
    // Retain the canonical initial state only for hashing; callers receive no
    // mutable state that could be mistaken for a live game position.
    initial.actions_taken = 0;
    evidence
}

fn initial_exact_state(actions: &[ComboAction]) -> ComboState {
    let mut state = ComboState {
        life_by_player: vec![20, 20],
        ..ComboState::default()
    };
    let mut seen = BTreeSet::new();
    for action in actions {
        let instance_id = format!("source:{}", action.card_id.0);
        if seen.insert(instance_id.clone()) {
            let zone = if action.rule_timing == RuleTiming::Spell {
                AbilityZone::Hand
            } else {
                AbilityZone::Battlefield
            };
            state.objects.push(ComboObjectState {
                instance_id,
                card_id: action.card_id.0.clone(),
                card_name: action.card_name.clone(),
                controller: 1,
                zone,
                types: action.source_types.clone(),
                subtypes: action.source_subtypes.clone(),
                ..ComboObjectState::default()
            });
        }
        for cost in &action.rule_costs {
            if let RuleCost::Mana(mana) = cost {
                for (index, amount) in mana.colored.iter().enumerate() {
                    state.colored_mana[index] =
                        state.colored_mana[index].saturating_add(i16::from(*amount));
                }
                for hybrid in &mana.hybrid {
                    if let Some(color) = hybrid.first() {
                        state.colored_mana[color_slot(*color)] =
                            state.colored_mana[color_slot(*color)].saturating_add(1);
                    }
                }
                state.resources.mana = state.resources.mana.saturating_add(i16::from(mana.generic));
            }
        }
        for effect in &action.rule_effects {
            if let RuleEffect::Replace { replacement, .. } = effect {
                state.replacement_effects.push(replacement.clone());
            }
        }
    }
    normalize_exact_state(&mut state);
    state
}

fn apply_exact_action(state: &mut ComboState, action: &ComboAction) -> Result<(), String> {
    if action.once_per_turn && state.once_per_turn_used.contains(&action.id) {
        return Err(format!("{} is limited to once per turn.", action.label));
    }
    let source_index = state
        .objects
        .iter()
        .position(|object| {
            object.card_id == action.card_id.0 && object.zone == AbilityZone::Battlefield
        })
        .ok_or_else(|| format!("{} has no controlled battlefield source.", action.card_name))?;
    let target_index = select_exact_target(state, &action.rule_targets);
    if !action.rule_targets.is_empty() && target_index.is_none() {
        return Err(format!("{} has no legal exact-state target.", action.label));
    }
    for cost in &action.rule_costs {
        pay_exact_cost(state, source_index, cost)?;
    }
    if action.once_per_turn {
        state.once_per_turn_used.push(action.id.clone());
    }
    for effect in &action.rule_effects {
        apply_exact_effect(state, source_index, target_index, effect)?;
    }
    for delayed in &action.rule_delayed_effects {
        state.delayed_triggers.push(format!("{delayed:?}"));
    }
    state.actions_taken += 1;
    normalize_exact_state(state);
    Ok(())
}

fn pay_exact_cost(
    state: &mut ComboState,
    source_index: usize,
    cost: &RuleCost,
) -> Result<(), String> {
    match cost {
        RuleCost::None => Ok(()),
        RuleCost::Mana(mana) => {
            for (index, amount) in mana.colored.iter().enumerate() {
                let amount = i16::from(*amount);
                if state.colored_mana[index] < amount {
                    return Err(format!("Missing colored mana in slot {index}."));
                }
                state.colored_mana[index] -= amount;
            }
            for hybrid in &mana.hybrid {
                let Some(index) = hybrid
                    .iter()
                    .map(|color| color_slot(*color))
                    .find(|index| state.colored_mana[*index] > 0)
                else {
                    return Err("No legal hybrid-mana option is available.".into());
                };
                state.colored_mana[index] -= 1;
            }
            let generic = i16::from(mana.generic);
            let colored_total = state.colored_mana.iter().copied().sum::<i16>();
            if state.resources.mana + colored_total < generic {
                return Err("Insufficient generic mana after colored requirements.".into());
            }
            let from_generic = state.resources.mana.min(generic);
            state.resources.mana -= from_generic;
            let mut remaining = generic - from_generic;
            for pool in &mut state.colored_mana {
                let spend = (*pool).min(remaining);
                *pool -= spend;
                remaining -= spend;
            }
            Ok(())
        }
        RuleCost::TapSource => {
            let source = state
                .objects
                .get_mut(source_index)
                .ok_or("The source object disappeared before its tap cost.")?;
            if source.tapped
                || source.summoning_sick && source.types.iter().any(|value| value == "creature")
            {
                return Err(format!("{} cannot legally tap.", source.card_name));
            }
            source.tapped = true;
            Ok(())
        }
        RuleCost::Sacrifice { object, amount } => {
            move_cost_objects(state, source_index, object, amount, AbilityZone::Graveyard)
        }
        RuleCost::Discard { object, amount } => {
            move_cost_objects(state, source_index, object, amount, AbilityZone::Graveyard)
        }
        RuleCost::Exile { object, amount } => {
            move_cost_objects(state, source_index, object, amount, AbilityZone::Exile)
        }
        RuleCost::PayLife(amount) => {
            let amount = exact_amount(amount)?;
            if state.life_by_player.first().copied().unwrap_or(0) <= amount {
                return Err("Life payment is not legal.".into());
            }
            state.life_by_player[0] -= amount;
            Ok(())
        }
        RuleCost::RemoveCounters(counter) => {
            let amount = exact_amount(&counter.amount)?;
            let source = state
                .objects
                .get_mut(source_index)
                .ok_or("Missing counter source.")?;
            let Some((_, value)) = source
                .counters
                .iter_mut()
                .find(|(name, _)| name == &counter.name)
            else {
                return Err(format!(
                    "{} has no {} counters.",
                    source.card_name, counter.name
                ));
            };
            if *value < amount {
                return Err("Not enough counters to pay the cost.".into());
            }
            *value -= amount;
            Ok(())
        }
        RuleCost::Additional(reason) => Err(format!(
            "Additional cost is not yet represented exactly: {reason}"
        )),
    }
}

fn move_cost_objects(
    state: &mut ComboState,
    source_index: usize,
    object: &str,
    amount: &AmountExpression,
    destination: AbilityZone,
) -> Result<(), String> {
    let count = usize::try_from(exact_amount(amount)?).map_err(|_| "Invalid object count.")?;
    let source_id = state
        .objects
        .get(source_index)
        .map(|value| value.instance_id.clone());
    let self_cost = object.to_ascii_lowercase().contains("this");
    let mut candidates = state
        .objects
        .iter()
        .enumerate()
        .filter(|(_, value)| value.controller == 1 && value.zone == AbilityZone::Battlefield)
        .filter(|(_, value)| !self_cost || Some(&value.instance_id) == source_id.as_ref())
        .map(|(index, _)| index)
        .take(count)
        .collect::<Vec<_>>();
    if candidates.len() < count {
        return Err(format!(
            "Cost requires {count} legal objects matching {object}."
        ));
    }
    candidates.sort_unstable();
    for index in candidates {
        state.objects[index].zone = destination;
        state.objects[index].tapped = false;
    }
    Ok(())
}

fn select_exact_target(state: &ComboState, targets: &[RuleTarget]) -> Option<usize> {
    let target = targets.first()?;
    state.objects.iter().position(|object| {
        (target.zones.is_empty() || target.zones.contains(&object.zone))
            && match target.controller.as_str() {
                "you" => object.controller == 1,
                "opponent" => object.controller != 1,
                _ => true,
            }
            && target
                .card_types
                .iter()
                .all(|card_type| object.types.contains(card_type))
            && target
                .subtypes
                .iter()
                .all(|subtype| object.subtypes.contains(subtype))
            && (!target
                .restrictions
                .iter()
                .any(|value| value == "you control")
                || object.controller == 1)
            && (!target
                .restrictions
                .iter()
                .any(|value| value == "nonland permanent")
                || !object.types.iter().any(|value| value == "land"))
    })
}

fn apply_exact_effect(
    state: &mut ComboState,
    source_index: usize,
    target_index: Option<usize>,
    effect: &RuleEffect,
) -> Result<(), String> {
    match effect {
        RuleEffect::Draw { amount, .. } => {
            state.resources.cards += exact_amount(amount)?;
            Ok(())
        }
        RuleEffect::Discard { amount, .. } => {
            state.resources.cards -= exact_amount(amount)?;
            Ok(())
        }
        RuleEffect::Life { amount, gain, .. } => {
            let amount = exact_amount(amount)?;
            state.life_by_player[0] += if *gain { amount } else { -amount };
            Ok(())
        }
        RuleEffect::Tap { untap } => {
            let index = target_index.unwrap_or(source_index);
            let object = state
                .objects
                .get_mut(index)
                .ok_or("Tap effect has no object.")?;
            object.tapped = !*untap;
            Ok(())
        }
        RuleEffect::AddMana { colors, amount } => {
            let amount = exact_amount(amount)?;
            let Some(color) = colors.first() else {
                return Err("Mana effect has no exact color.".into());
            };
            state.colored_mana[color_slot(*color)] += amount;
            Ok(())
        }
        RuleEffect::PutCounters(counter) => {
            let amount = exact_amount(&counter.amount)?;
            let index = target_index.unwrap_or(source_index);
            let object = state
                .objects
                .get_mut(index)
                .ok_or("Counter effect has no object.")?;
            if let Some((_, value)) = object
                .counters
                .iter_mut()
                .find(|(name, _)| name == &counter.name)
            {
                *value += amount;
            } else {
                object.counters.push((counter.name.clone(), amount));
            }
            Ok(())
        }
        RuleEffect::CreateToken { token, amount } => {
            let mut count = exact_amount(amount)?;
            for replacement in &state.replacement_effects {
                if replacement.to_ascii_lowercase().contains("twice") {
                    count = count.saturating_mul(2);
                }
            }
            for offset in 0..count.max(0) {
                state.objects.push(ComboObjectState {
                    instance_id: format!("token:{}:{}", state.actions_taken, offset),
                    card_name: token.name.clone(),
                    controller: 1,
                    zone: AbilityZone::Battlefield,
                    types: token.types.clone(),
                    subtypes: token.subtypes.clone(),
                    token: true,
                    tapped: token.tapped,
                    summoning_sick: token.types.iter().any(|value| value == "creature"),
                    ..ComboObjectState::default()
                });
            }
            state.resources.tokens += count;
            state.resources.permanents += count;
            Ok(())
        }
        RuleEffect::MoveCard { from, to, .. } => {
            let index = target_index.ok_or("Card movement requires an exact target.")?;
            if state.objects[index].zone != *from {
                return Err(format!(
                    "Card movement expected {:?}, but the target is in {:?}.",
                    from, state.objects[index].zone
                ));
            }
            state.objects[index].zone = *to;
            state.objects[index].tapped = false;
            Ok(())
        }
        RuleEffect::Destroy { .. } | RuleEffect::Sacrifice { .. } => {
            let index = target_index.ok_or("Destruction/sacrifice requires an exact target.")?;
            state.objects[index].zone = AbilityZone::Graveyard;
            state.objects[index].tapped = false;
            Ok(())
        }
        RuleEffect::Exile { .. } => {
            let index = target_index.ok_or("Exile requires an exact target.")?;
            state.objects[index].zone = AbilityZone::Exile;
            state.objects[index].tapped = false;
            Ok(())
        }
        RuleEffect::Damage { amount, .. } => {
            state.resources.damage += exact_amount(amount)?;
            Ok(())
        }
        RuleEffect::Mill { amount, .. } => {
            state.resources.mill += exact_amount(amount)?;
            Ok(())
        }
        RuleEffect::Replace { replacement, .. } => {
            if !state.replacement_effects.contains(replacement) {
                state.replacement_effects.push(replacement.clone());
            }
            Ok(())
        }
        RuleEffect::ControlChange { .. } => {
            let index = target_index.ok_or("Control change requires a target.")?;
            state.objects[index].controller = 1;
            Ok(())
        }
        RuleEffect::ModifyTypes { add, remove } => {
            let index = target_index.unwrap_or(source_index);
            let object = &mut state.objects[index];
            object.types.retain(|value| !remove.contains(value));
            let missing = add
                .iter()
                .filter(|value| !object.types.contains(value))
                .cloned()
                .collect::<Vec<_>>();
            object.types.extend(missing);
            Ok(())
        }
        RuleEffect::GrantKeyword { keyword } => {
            state
                .continuous_modifiers
                .push(format!("keyword:{keyword}"));
            Ok(())
        }
        RuleEffect::ModifyPowerToughness { .. }
        | RuleEffect::CostChange { .. }
        | RuleEffect::Perpetual { .. }
        | RuleEffect::Transform => {
            state.continuous_modifiers.push(format!("{effect:?}"));
            Ok(())
        }
        RuleEffect::Copy { as_token: true, .. } => {
            let target = target_index
                .and_then(|index| state.objects.get(index))
                .cloned()
                .ok_or("Token copy requires an exact target.")?;
            let mut copy = target;
            copy.instance_id = format!("copy:{}:{}", state.actions_taken, state.objects.len());
            copy.card_id.clear();
            copy.token = true;
            copy.controller = 1;
            copy.summoning_sick = copy.types.iter().any(|value| value == "creature");
            state.objects.push(copy);
            state.resources.tokens += 1;
            state.resources.permanents += 1;
            Ok(())
        }
        RuleEffect::WinLoss { wins: true, .. } => Ok(()),
        _ => Err(format!(
            "Rules IR effect is not exact-state executable: {effect:?}"
        )),
    }
}

fn source_state_restored(initial: &ComboState, final_state: &ComboState) -> bool {
    let sources_match = initial
        .objects
        .iter()
        .filter(|object| !object.token)
        .all(|before| {
            final_state
                .objects
                .iter()
                .find(|after| after.instance_id == before.instance_id)
                .is_some_and(|after| {
                    after.zone == before.zone
                        && after.controller == before.controller
                        && after.tapped == before.tapped
                        && after.counters == before.counters
                })
        });
    sources_match
        && final_state.resources.mana >= initial.resources.mana
        && final_state.resources.cards >= initial.resources.cards
        && final_state.life_by_player.first() >= initial.life_by_player.first()
        && final_state
            .colored_mana
            .iter()
            .zip(initial.colored_mana)
            .all(|(after, before)| *after >= before)
        && final_state.once_per_turn_used == initial.once_per_turn_used
}

fn exact_state_hash(state: &ComboState) -> String {
    let mut canonical = state.clone();
    normalize_exact_state(&mut canonical);
    format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&canonical).unwrap_or_default())
    )
}

fn normalize_exact_state(state: &mut ComboState) {
    state
        .objects
        .sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
    for object in &mut state.objects {
        object.types.sort();
        object.types.dedup();
        object.subtypes.sort();
        object.subtypes.dedup();
        object.counters.sort();
    }
    state.tapped_cards = state
        .objects
        .iter()
        .filter(|object| object.tapped)
        .map(|object| object.instance_id.clone())
        .collect();
    state.consumed_cards = state
        .objects
        .iter()
        .filter(|object| matches!(object.zone, AbilityZone::Graveyard | AbilityZone::Exile))
        .map(|object| object.instance_id.clone())
        .collect();
    state.once_per_turn_used.sort();
    state.once_per_turn_used.dedup();
    state.delayed_triggers.sort();
    state.replacement_effects.sort();
    state.replacement_effects.dedup();
    state.continuous_modifiers.sort();
}

fn exact_amount(amount: &AmountExpression) -> Result<i16, String> {
    match amount {
        AmountExpression::One => Ok(1),
        AmountExpression::Constant(value) => (*value)
            .try_into()
            .map_err(|_| "Quantity is outside exact-state bounds.".into()),
        AmountExpression::X | AmountExpression::Variable(_) | AmountExpression::Counted(_) => Err(
            format!("Unbound quantity cannot be proven exactly: {amount:?}"),
        ),
    }
}

fn color_slot(color: Color) -> usize {
    match color {
        Color::White => 0,
        Color::Blue => 1,
        Color::Black => 2,
        Color::Red => 3,
        Color::Green => 4,
        Color::Colorless => 5,
    }
}

fn strongly_connected_components(actions: &[ComboAction]) -> Vec<Vec<usize>> {
    let mut graph = vec![Vec::new(); actions.len()];
    for (left_index, left) in actions.iter().enumerate() {
        let left_effect = left.effect.array();
        for (right_index, right) in actions.iter().enumerate() {
            let right_cost = right.cost.array();
            let supplies =
                (0..RESOURCE_COUNT).any(|index| left_effect[index] > 0 && right_cost[index] > 0);
            let same_engine = left.card_id == right.card_id
                || left.kind == ComboActionKind::Untap
                || right.kind == ComboActionKind::Untap
                || left.kind == ComboActionKind::Recursion
                || right.kind == ComboActionKind::Recursion;
            let replacement_bridge = matches!(
                left.kind,
                ComboActionKind::Replacement | ComboActionKind::Copy
            ) && right.effect.array().iter().any(|value| *value > 0)
                || matches!(
                    right.kind,
                    ComboActionKind::Replacement | ComboActionKind::Copy
                ) && left.effect.array().iter().any(|value| *value > 0);
            if supplies
                || same_engine
                || replacement_bridge
                || left_index == right_index && action_self_repeatable(left)
            {
                graph[left_index].push(right_index);
            }
        }
    }
    tarjan(&graph)
}

fn tarjan(graph: &[Vec<usize>]) -> Vec<Vec<usize>> {
    struct State {
        next_index: usize,
        indices: Vec<Option<usize>>,
        low: Vec<usize>,
        stack: Vec<usize>,
        on_stack: Vec<bool>,
        components: Vec<Vec<usize>>,
    }
    fn visit(node: usize, graph: &[Vec<usize>], state: &mut State) {
        let index = state.next_index;
        state.next_index += 1;
        state.indices[node] = Some(index);
        state.low[node] = index;
        state.stack.push(node);
        state.on_stack[node] = true;
        for &next in &graph[node] {
            if state.indices[next].is_none() {
                visit(next, graph, state);
                state.low[node] = state.low[node].min(state.low[next]);
            } else if state.on_stack[next] {
                state.low[node] = state.low[node].min(state.indices[next].unwrap_or(index));
            }
        }
        if state.low[node] == index {
            let mut component = Vec::new();
            while let Some(current) = state.stack.pop() {
                state.on_stack[current] = false;
                component.push(current);
                if current == node {
                    break;
                }
            }
            state.components.push(component);
        }
    }
    let mut state = State {
        next_index: 0,
        indices: vec![None; graph.len()],
        low: vec![0; graph.len()],
        stack: Vec::new(),
        on_stack: vec![false; graph.len()],
        components: Vec::new(),
    };
    for node in 0..graph.len() {
        if state.indices[node].is_none() {
            visit(node, graph, &mut state);
        }
    }
    state.components
}

fn action_self_repeatable(action: &ComboAction) -> bool {
    !action.once_per_turn
        && !action.uses_tap
        && !action.consumes_self
        && action.effect.array().iter().any(|value| *value > 0)
}

fn parse_types(type_line: &str) -> Vec<String> {
    type_line
        .split_once('—')
        .map_or(type_line, |(types, _)| types)
        .split_whitespace()
        .map(|value| {
            value
                .trim_matches(|character: char| !character.is_alphanumeric())
                .to_ascii_lowercase()
        })
        .filter(|value| !value.is_empty() && value != "legendary" && value != "basic")
        .collect()
}

fn combo_kind_for_rule(rule: &metagene_core::RuleAbility) -> ComboActionKind {
    for effect in &rule.effects {
        let kind = match effect {
            RuleEffect::Draw { .. } => ComboActionKind::Draw,
            RuleEffect::Search { .. } => ComboActionKind::Tutor,
            RuleEffect::MoveCard {
                from: AbilityZone::Graveyard,
                ..
            } => ComboActionKind::Recursion,
            RuleEffect::Copy { .. } => ComboActionKind::Copy,
            RuleEffect::Replace { .. } => ComboActionKind::Replacement,
            RuleEffect::CostChange {
                increase: false, ..
            } => ComboActionKind::CostReduction,
            RuleEffect::Tap { untap: true } => ComboActionKind::Untap,
            RuleEffect::AddMana { .. } => ComboActionKind::Mana,
            RuleEffect::CreateToken { .. } => ComboActionKind::Token,
            RuleEffect::PutCounters(_) => ComboActionKind::Counter,
            RuleEffect::Damage { .. } => ComboActionKind::Damage,
            RuleEffect::Mill { .. } => ComboActionKind::Mill,
            RuleEffect::Life { gain: true, .. } => ComboActionKind::Life,
            RuleEffect::Sacrifice { .. } => ComboActionKind::Sacrifice,
            RuleEffect::GrantKeyword { keyword }
                if matches!(
                    keyword.as_str(),
                    "hexproof" | "indestructible" | "protection"
                ) =>
            {
                ComboActionKind::Protection
            }
            RuleEffect::WinLoss { wins: true, .. } => ComboActionKind::AlternateWin,
            _ => continue,
        };
        return kind;
    }
    let blink = rule.effects.iter().any(|effect| {
        matches!(
            effect,
            RuleEffect::MoveCard {
                from: AbilityZone::Battlefield,
                to: AbilityZone::Exile,
                ..
            }
        )
    }) && rule.effects.iter().any(|effect| {
        matches!(
            effect,
            RuleEffect::MoveCard {
                from: AbilityZone::Exile,
                to: AbilityZone::Battlefield,
                ..
            }
        )
    });
    if blink {
        ComboActionKind::Blink
    } else {
        ComboActionKind::Unknown
    }
}

fn combo_action_from_rule(
    card: &Card,
    mode_index: usize,
    rule_index: usize,
    mode: &metagene_core::CardMode,
    rule: &metagene_core::RuleAbility,
) -> ComboAction {
    let kind = combo_kind_for_rule(rule);
    let mut cost = [0i16; RESOURCE_COUNT];
    for rule_cost in &rule.costs {
        match rule_cost {
            RuleCost::Mana(mana) => {
                cost[MANA] += i16::from(mana.generic)
                    + mana.colored.iter().copied().map(i16::from).sum::<i16>()
                    + mana.hybrid.len() as i16;
            }
            RuleCost::Sacrifice { amount, .. } => cost[PERMANENTS] += approximate_amount(amount),
            RuleCost::Discard { amount, .. } => cost[CARDS] += approximate_amount(amount),
            RuleCost::Exile { amount, .. } => cost[GRAVEYARD] += approximate_amount(amount),
            RuleCost::PayLife(amount) => cost[LIFE] += approximate_amount(amount),
            RuleCost::RemoveCounters(counter) => {
                cost[COUNTERS] += approximate_amount(&counter.amount)
            }
            _ => {}
        }
    }
    let mut effect = [0i16; RESOURCE_COUNT];
    for rule_effect in &rule.effects {
        match rule_effect {
            RuleEffect::Draw { amount, .. } => effect[CARDS] += approximate_amount(amount),
            RuleEffect::Search { .. } => effect[CARDS] += 1,
            RuleEffect::MoveCard {
                from: AbilityZone::Graveyard,
                to: AbilityZone::Battlefield,
                ..
            } => {
                effect[PERMANENTS] += 1;
                effect[GRAVEYARD] -= 1;
            }
            RuleEffect::AddMana { amount, .. } => effect[MANA] += approximate_amount(amount),
            RuleEffect::CreateToken { amount, .. } => {
                let amount = approximate_amount(amount);
                effect[TOKENS] += amount;
                effect[PERMANENTS] += amount;
            }
            RuleEffect::PutCounters(counter) => {
                effect[COUNTERS] += approximate_amount(&counter.amount)
            }
            RuleEffect::Damage { amount, .. } => effect[DAMAGE] += approximate_amount(amount),
            RuleEffect::Mill { amount, .. } => effect[MILL] += approximate_amount(amount),
            RuleEffect::Life {
                amount, gain: true, ..
            } => effect[LIFE] += approximate_amount(amount),
            RuleEffect::WinLoss { wins: true, .. } => effect[DAMAGE] = effect[DAMAGE].max(20),
            _ => {}
        }
    }
    ComboAction {
        id: format!("{}:{mode_index}:rule:{rule_index}", card.id.0),
        card_id: card.id.clone(),
        card_name: card.name.clone(),
        mode_name: mode.name.clone(),
        source_types: parse_types(&mode.type_line),
        source_subtypes: mode.creature_types.clone(),
        label: rule.label.clone(),
        kind,
        timing: match rule.timing {
            RuleTiming::Activation | RuleTiming::Mana => AbilityTiming::Activated,
            RuleTiming::Trigger => AbilityTiming::Triggered,
            RuleTiming::Static | RuleTiming::Replacement => AbilityTiming::Static,
            _ => AbilityTiming::Spell,
        },
        zone: rule.source_zones.first().copied().unwrap_or(AbilityZone::Battlefield),
        cost: ComboResources::from_array(cost),
        effect: ComboResources::from_array(effect),
        uses_tap: rule.costs.iter().any(|cost| matches!(cost, RuleCost::TapSource)),
        consumes_self: rule.costs.iter().any(|cost| matches!(cost, RuleCost::Sacrifice { object, .. } | RuleCost::Discard { object, .. } | RuleCost::Exile { object, .. } if object.to_ascii_lowercase().contains("this"))),
        once_per_turn: rule.once_per_turn,
        parser_confidence: rule.parser_confidence,
        conditions: rule.conditions.iter().map(|condition| format!("{condition:?}")).collect(),
        rule_timing: rule.timing,
        rule_costs: rule.costs.clone(),
        rule_targets: rule.targets.clone(),
        rule_effects: rule.effects.clone(),
        rule_delayed_effects: rule.delayed_effects.clone(),
        exact_state_supported: rule_is_exact_state_supported(rule),
    }
}

fn approximate_amount(amount: &AmountExpression) -> i16 {
    match amount {
        AmountExpression::One => 1,
        AmountExpression::Constant(value) => (*value).clamp(0, i32::from(i16::MAX)) as i16,
        _ => 1,
    }
}

fn rule_is_exact_state_supported(rule: &metagene_core::RuleAbility) -> bool {
    rule.parser_confidence >= 1.0
        && matches!(
            rule.timing,
            RuleTiming::Activation
                | RuleTiming::Mana
                | RuleTiming::Static
                | RuleTiming::Replacement
                | RuleTiming::SpecialAction
        )
        && !rule.once_per_turn
        && rule
            .conditions
            .iter()
            .all(|condition| matches!(condition, metagene_core::RuleCondition::Always))
        && rule.delayed_effects.is_empty()
        && rule
            .costs
            .iter()
            .all(|cost| !matches!(cost, RuleCost::Additional(_)))
        && rule
            .effects
            .iter()
            .chain(&rule.delayed_effects)
            .all(exact_effect_supported)
}

fn exact_effect_supported(effect: &RuleEffect) -> bool {
    match effect {
        RuleEffect::AddMana { colors, .. } => colors.len() == 1,
        RuleEffect::Copy { as_token, .. } => *as_token,
        RuleEffect::Replace { event, replacement } => {
            event.to_ascii_lowercase().contains("token")
                && replacement.to_ascii_lowercase().contains("twice")
        }
        RuleEffect::WinLoss { wins, .. } => *wins,
        RuleEffect::Draw { .. }
        | RuleEffect::Discard { .. }
        | RuleEffect::Life { .. }
        | RuleEffect::Tap { .. }
        | RuleEffect::PutCounters(_)
        | RuleEffect::CreateToken { .. }
        | RuleEffect::MoveCard { .. }
        | RuleEffect::Destroy { .. }
        | RuleEffect::Sacrifice { .. }
        | RuleEffect::Exile { .. }
        | RuleEffect::Damage { .. }
        | RuleEffect::Mill { .. }
        | RuleEffect::ControlChange { .. } => true,
        _ => false,
    }
}

fn combo_actions(card: &Card) -> Vec<ComboAction> {
    let mut actions = Vec::new();
    for (mode_index, mode) in card.features.initial_modes.iter().enumerate() {
        for (ability_index, ability) in mode.abilities.iter().enumerate() {
            let text = ability.label.to_ascii_lowercase();
            let kind = combo_kind(ability, &text);
            let magnitude = ability.effect_value.max(1.0).ceil() as i16;
            let colored = ability.cost.mana.colored.iter().copied().sum::<u8>();
            let mana_cost = i16::from(ability.cost.mana.generic)
                + i16::from(colored)
                + ability.cost.mana.hybrid.len() as i16;
            let mut cost = [0i16; RESOURCE_COUNT];
            cost[MANA] = mana_cost;
            cost[CARDS] =
                i16::from(ability.cost.discard_cards) + i16::from(ability.cost.discards_self);
            cost[PERMANENTS] = i16::from(ability.cost.sacrifice_permanents)
                + i16::from(ability.cost.sacrifices_self);
            cost[LIFE] = i16::from(ability.cost.life);
            cost[COUNTERS] = i16::from(ability.cost.remove_counters);
            if ability.cost.exiles_self || ability.cost.exile_cards > 0 {
                cost[GRAVEYARD] =
                    i16::from(ability.cost.exile_cards) + i16::from(ability.cost.exiles_self);
            }
            let mut effect = [0i16; RESOURCE_COUNT];
            match kind {
                ComboActionKind::Draw | ComboActionKind::Tutor => effect[CARDS] = magnitude,
                ComboActionKind::Recursion => {
                    effect[CARDS] = magnitude;
                    effect[PERMANENTS] = i16::from(text.contains("battlefield"));
                }
                ComboActionKind::Mana | ComboActionKind::CostReduction => effect[MANA] = magnitude,
                ComboActionKind::Token => {
                    effect[TOKENS] = magnitude;
                    effect[PERMANENTS] = magnitude;
                }
                ComboActionKind::Counter => effect[COUNTERS] = magnitude,
                ComboActionKind::Damage => effect[DAMAGE] = magnitude,
                ComboActionKind::Mill => effect[MILL] = magnitude,
                ComboActionKind::Life => effect[LIFE] = magnitude,
                ComboActionKind::Sacrifice => effect[GRAVEYARD] = magnitude,
                ComboActionKind::Untap => effect[MANA] = i16::from(text.contains("land")),
                ComboActionKind::AlternateWin => effect[DAMAGE] = 20,
                ComboActionKind::Copy
                | ComboActionKind::Replacement
                | ComboActionKind::Blink
                | ComboActionKind::Protection
                | ComboActionKind::Unknown => {}
            }
            let rule = mode.rule_abilities.iter().find(|rule| {
                !ability.clause_ids.is_empty()
                    && rule
                        .clause_ids
                        .iter()
                        .any(|id| ability.clause_ids.contains(id))
                    && combo_kind_for_rule(rule) == kind
            });
            actions.push(ComboAction {
                id: format!("{}:{mode_index}:{ability_index}", card.id.0),
                card_id: card.id.clone(),
                card_name: card.name.clone(),
                mode_name: mode.name.clone(),
                source_types: parse_types(&mode.type_line),
                source_subtypes: mode.creature_types.clone(),
                label: ability.label.clone(),
                kind,
                timing: ability.timing,
                zone: ability.zone,
                cost: ComboResources::from_array(cost),
                effect: ComboResources::from_array(effect),
                uses_tap: ability.cost.uses_tap,
                consumes_self: ability.cost.sacrifices_self
                    || ability.cost.discards_self
                    || ability.cost.exiles_self,
                once_per_turn: ability.once_per_turn,
                parser_confidence: ability.parser_confidence,
                conditions: ability.conditions.clone(),
                rule_timing: rule.map_or(RuleTiming::Spell, |rule| rule.timing),
                rule_costs: rule.map_or_else(Vec::new, |rule| rule.costs.clone()),
                rule_targets: rule.map_or_else(Vec::new, |rule| rule.targets.clone()),
                rule_effects: rule.map_or_else(Vec::new, |rule| rule.effects.clone()),
                rule_delayed_effects: rule
                    .map_or_else(Vec::new, |rule| rule.delayed_effects.clone()),
                exact_state_supported: rule.is_some_and(rule_is_exact_state_supported),
            });
        }
        for (rule_index, rule) in mode.rule_abilities.iter().enumerate() {
            let kind = combo_kind_for_rule(rule);
            if kind == ComboActionKind::Unknown
                || actions.iter().any(|action| {
                    action.card_id == card.id
                        && action.mode_name == mode.name
                        && action.kind == kind
                        && action.rule_effects == rule.effects
                })
            {
                continue;
            }
            actions.push(combo_action_from_rule(
                card, mode_index, rule_index, mode, rule,
            ));
        }
        let oracle = card.oracle_text.to_ascii_lowercase();
        for (suffix, needle, kind, effect) in [
            (
                "untap",
                "untap",
                ComboActionKind::Untap,
                ComboResources::default(),
            ),
            (
                "reduce",
                "cost {1} less",
                ComboActionKind::CostReduction,
                ComboResources {
                    mana: 1,
                    ..ComboResources::default()
                },
            ),
            (
                "protect",
                "hexproof",
                ComboActionKind::Protection,
                ComboResources::default(),
            ),
        ] {
            if oracle.contains(needle)
                && !actions
                    .iter()
                    .any(|action| action.card_id == card.id && action.kind == kind)
            {
                actions.push(ComboAction {
                    id: format!("{}:{mode_index}:{suffix}", card.id.0),
                    card_id: card.id.clone(),
                    card_name: card.name.clone(),
                    mode_name: mode.name.clone(),
                    source_types: parse_types(&mode.type_line),
                    source_subtypes: mode.creature_types.clone(),
                    label: needle.into(),
                    kind,
                    timing: AbilityTiming::Static,
                    zone: AbilityZone::Battlefield,
                    cost: ComboResources::default(),
                    effect,
                    uses_tap: false,
                    consumes_self: false,
                    once_per_turn: false,
                    parser_confidence: 0.75,
                    conditions: Vec::new(),
                    rule_timing: RuleTiming::Static,
                    rule_costs: Vec::new(),
                    rule_targets: Vec::new(),
                    rule_effects: Vec::new(),
                    rule_delayed_effects: Vec::new(),
                    exact_state_supported: false,
                });
            }
        }
    }
    actions
}

fn combo_kind(ability: &AbilityProfile, text: &str) -> ComboActionKind {
    if text.contains("untap") {
        return ComboActionKind::Untap;
    }
    if text.contains("cost") && text.contains(" less") {
        return ComboActionKind::CostReduction;
    }
    if text.contains("search your library") && !text.contains("land") {
        return ComboActionKind::Tutor;
    }
    if text.contains("gain") && text.contains("life") {
        return ComboActionKind::Life;
    }
    if text.contains("hexproof") || text.contains("indestructible") || text.contains("protection") {
        return ComboActionKind::Protection;
    }
    match ability.kind {
        AbilityKind::Draw => ComboActionKind::Draw,
        AbilityKind::Recursion => ComboActionKind::Recursion,
        AbilityKind::Search => ComboActionKind::Tutor,
        AbilityKind::Copy => ComboActionKind::Copy,
        AbilityKind::TokenMultiplier => ComboActionKind::Replacement,
        AbilityKind::Ramp => ComboActionKind::Mana,
        AbilityKind::CreateToken => ComboActionKind::Token,
        AbilityKind::AddCounters => ComboActionKind::Counter,
        AbilityKind::Damage => ComboActionKind::Damage,
        AbilityKind::Mill => ComboActionKind::Mill,
        AbilityKind::SacrificeOutlet => ComboActionKind::Sacrifice,
        AbilityKind::Blink => ComboActionKind::Blink,
        AbilityKind::AlternateWin => ComboActionKind::AlternateWin,
        _ => ComboActionKind::Unknown,
    }
}

fn package_for_line(line: &ComboLine) -> ComboPackage {
    let mut package = ComboPackage {
        line_id: line.id.clone(),
        ..ComboPackage::default()
    };
    for action in &line.actions {
        let target = match action.kind {
            ComboActionKind::Tutor => &mut package.tutors,
            ComboActionKind::Protection => &mut package.protection,
            ComboActionKind::Recursion => &mut package.recovery,
            ComboActionKind::Damage | ComboActionKind::Mill | ComboActionKind::AlternateWin => {
                &mut package.payoffs
            }
            ComboActionKind::Mana
            | ComboActionKind::Untap
            | ComboActionKind::CostReduction
            | ComboActionKind::Sacrifice => &mut package.enablers,
            _ => &mut package.engine,
        };
        if !target.contains(&action.card_name) {
            target.push(action.card_name.clone());
        }
    }
    package
}

fn extract_card_names(value: &Value) -> Vec<String> {
    let mut names = BTreeSet::new();
    if let Some(uses) = value.get("uses").and_then(Value::as_array) {
        for use_entry in uses {
            if let Some(name) = use_entry
                .get("card")
                .and_then(|card| card.get("name"))
                .and_then(Value::as_str)
                .or_else(|| use_entry.get("cardName").and_then(Value::as_str))
                .or_else(|| use_entry.get("name").and_then(Value::as_str))
            {
                names.insert(name.to_string());
            }
        }
    }
    if names.is_empty()
        && let Some(cards) = value.get("cards").and_then(Value::as_array)
    {
        for card in cards {
            if let Some(name) = card
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| card.as_str())
            {
                names.insert(name.to_string());
            }
        }
    }
    names.into_iter().collect()
}

fn load_hypotheses(data_dir: &Path) -> Result<Vec<CorpusHypothesis>, String> {
    if !database_path(data_dir).is_file() {
        return Ok(Vec::new());
    }
    let connection = open(data_dir)?;
    let mut statement = connection
        .prepare("SELECT id, status, description, card_names_json FROM combos")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            let cards: String = row.get(3)?;
            Ok(CorpusHypothesis {
                id: row.get(0)?,
                status: row.get(1)?,
                description: row.get(2)?,
                cards: serde_json::from_str(&cards).unwrap_or_default(),
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

pub fn conformance_scenario(
    data_dir: &Path,
    deck: &Deck,
    line_id: &str,
) -> Result<ComboConformanceScenario, String> {
    let report = load_latest_proof(data_dir, deck)?
        .ok_or("Scan this exact deck for combos before creating a conformance scenario.")?;
    let line = report
        .lines
        .iter()
        .find(|line| line.id == line_id)
        .ok_or("The requested combo line does not belong to the latest exact-deck proof.")?;
    if !proof_is_plan_eligible(line.proof) {
        return Err(format!(
            "Combo line {line_id} is {:?}; exact state proof is required before a line can become a deterministic engine scenario.",
            line.proof
        ));
    }
    Ok(scenario_for_line(line))
}

pub fn record_conformance_observation(
    data_dir: &Path,
    request: &ComboConformanceObservationRequest,
) -> Result<ComboConformanceReport, String> {
    let engine = request.engine.trim().to_ascii_lowercase();
    if !matches!(engine.as_str(), "hexdek" | "forge" | "arena") {
        return Err("Combo conformance engine must be HexDek, Forge, or Arena.".into());
    }
    if request.engine_revision.trim().is_empty() || request.card_data_revision.trim().is_empty() {
        return Err("Engine and card-data revisions are required for combo proof evidence.".into());
    }
    let mut report = load_latest_proof(data_dir, &request.deck)?
        .ok_or("Scan this exact deck for combos before recording engine evidence.")?;
    let line = report
        .lines
        .iter()
        .find(|line| line.id == request.line_id)
        .ok_or("The combo line is not present in the latest exact-deck proof.")?;
    if !proof_is_plan_eligible(line.proof) {
        return Err(
            "Only a current exact-state-proven combo may receive engine proof evidence.".into(),
        );
    }
    let scenario = scenario_for_line(line);
    if request.scenario_id != scenario.scenario_id {
        return Err("The engine result targets a stale or different combo scenario.".into());
    }
    let outcome = if !request.completed {
        "incomplete"
    } else if !request.critical_mechanics_supported {
        "unsupported"
    } else if request.passed
        && request.terminal_state_hash == scenario.expected_terminal_hash
        && !request.terminal_state_hash.is_empty()
    {
        "passed"
    } else {
        "disputed"
    };
    let recorded_at = unix_now();
    let connection = open(data_dir)?;
    connection
        .execute(
            "INSERT INTO combo_conformance(
               deck_key,line_id,scenario_id,engine,engine_revision,card_data_revision,
               expected_terminal_hash,terminal_state_hash,outcome,detail,recorded_at
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
             ON CONFLICT(deck_key,line_id,scenario_id,engine,engine_revision,card_data_revision)
             DO UPDATE SET expected_terminal_hash=excluded.expected_terminal_hash,
               terminal_state_hash=excluded.terminal_state_hash,outcome=excluded.outcome,
               detail=excluded.detail,recorded_at=excluded.recorded_at",
            params![
                exact_deck_key(&request.deck),
                request.line_id,
                scenario.scenario_id,
                engine,
                request.engine_revision.trim(),
                request.card_data_revision.trim(),
                scenario.expected_terminal_hash,
                request.terminal_state_hash.trim(),
                outcome,
                request.detail.trim(),
                recorded_at.min(i64::MAX as u64) as i64,
            ],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "DELETE FROM combo_conformance WHERE id NOT IN (
               SELECT id FROM combo_conformance ORDER BY id DESC LIMIT ?1
             )",
            [MAX_CONFORMANCE_OBSERVATIONS as i64],
        )
        .map_err(|error| error.to_string())?;

    let observations = load_conformance_observations(data_dir, &request.deck, &request.line_id)?;
    let base = report
        .lines
        .iter()
        .find(|line| line.id == request.line_id)
        .map(|line| line.proof)
        .unwrap_or(ComboProof::ResourceBalancedHypothesis);
    let proof = proof_from_observations(base, &observations);
    if let Some(line) = report
        .lines
        .iter_mut()
        .find(|line| line.id == request.line_id)
    {
        line.proof = proof;
    }
    report.packages = report.lines.iter().map(package_for_line).collect();
    persist_proof(data_dir, &request.deck, &report)?;
    let decision = match proof {
        ComboProof::HexdekExecuted => {
            "HexDek reproduced the deterministic symbolic terminal state.".into()
        }
        ComboProof::CrossEngineVerified => {
            "HexDek and Forge reproduced the same deterministic terminal state.".into()
        }
        ComboProof::ArenaVerified => {
            "Arena reproduced the deterministic terminal state for this exact line.".into()
        }
        ComboProof::Disputed => {
            "At least one fully supported engine disagreed with the symbolic terminal state; the line is disputed and cannot guide play.".into()
        }
        ComboProof::Unsupported => {
            "The attempted engine route did not support every critical action in this line.".into()
        }
        _ => "The observation was retained, but it did not raise the symbolic proof tier.".into(),
    };
    Ok(ComboConformanceReport {
        scenario,
        proof,
        observations,
        decision,
    })
}

fn scenario_for_line(line: &ComboLine) -> ComboConformanceScenario {
    let action_ids = line
        .actions
        .iter()
        .map(|action| action.id.clone())
        .collect::<Vec<_>>();
    let expected_class = combo_output_class(line);
    let terminal = serde_json::json!({
        "proof_revision": COMBO_PROOF_REVISION,
        "line_id": line.id,
        "finite_win": line.finite_win,
        "repeatable": line.repeatable,
        "output": line.loop_output,
        "class": expected_class,
        "actions": action_ids,
    });
    let expected_terminal_hash = format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&terminal).unwrap_or_default())
    );
    let scenario_id = format!(
        "combo-scenario:{:x}",
        Sha256::digest(format!(
            "{}:{}:{}",
            COMBO_PROOF_REVISION, line.id, expected_terminal_hash
        ))
    );
    ComboConformanceScenario {
        schema_version: SCHEMA_VERSION,
        scenario_id,
        line_id: line.id.clone(),
        proof_revision: COMBO_PROOF_REVISION.into(),
        cards: line.cards.clone(),
        action_ids,
        initial_resources: line.minimum_resources.clone(),
        expected_class,
        expected_terminal_hash,
        maximum_actions: line.actions.len().clamp(1, DEFAULT_MAX_ACTIONS),
        deterministic: true,
    }
}

fn apply_conformance_proofs(
    data_dir: &Path,
    deck: &Deck,
    lines: &mut [ComboLine],
) -> Result<(), String> {
    for line in lines {
        if !proof_is_plan_eligible(line.proof) {
            continue;
        }
        let observations = load_conformance_observations(data_dir, deck, &line.id)?;
        line.proof = proof_from_observations(line.proof, &observations);
    }
    Ok(())
}

fn proof_from_observations(
    base: ComboProof,
    observations: &[ComboConformanceObservation],
) -> ComboProof {
    if observations.iter().any(|row| row.outcome == "disputed") {
        return ComboProof::Disputed;
    }
    let passed = observations
        .iter()
        .filter(|row| row.outcome == "passed")
        .collect::<Vec<_>>();
    if passed.iter().any(|row| row.engine == "arena") {
        return ComboProof::ArenaVerified;
    }
    let cross_engine = passed.iter().any(|left| {
        left.engine == "hexdek"
            && passed.iter().any(|right| {
                right.engine == "forge"
                    && right.scenario_id == left.scenario_id
                    && right.terminal_state_hash == left.terminal_state_hash
                    && right.expected_terminal_hash == left.expected_terminal_hash
            })
    });
    if cross_engine {
        return ComboProof::CrossEngineVerified;
    }
    if passed.iter().any(|row| row.engine == "hexdek") {
        return ComboProof::HexdekExecuted;
    }
    if !observations.is_empty()
        && observations
            .iter()
            .all(|row| matches!(row.outcome.as_str(), "unsupported" | "incomplete"))
        && observations.iter().any(|row| row.outcome == "unsupported")
    {
        return ComboProof::Unsupported;
    }
    base
}

fn load_conformance_observations(
    data_dir: &Path,
    deck: &Deck,
    line_id: &str,
) -> Result<Vec<ComboConformanceObservation>, String> {
    if !database_path(data_dir).is_file() {
        return Ok(Vec::new());
    }
    let connection = open(data_dir)?;
    let mut statement = connection
        .prepare(
            "SELECT scenario_id,line_id,engine,engine_revision,card_data_revision,
                    expected_terminal_hash,terminal_state_hash,outcome,detail,recorded_at
             FROM combo_conformance WHERE deck_key=?1 AND line_id=?2
             ORDER BY recorded_at ASC,id ASC",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![exact_deck_key(deck), line_id], |row| {
            Ok(ComboConformanceObservation {
                scenario_id: row.get(0)?,
                line_id: row.get(1)?,
                engine: row.get(2)?,
                engine_revision: row.get(3)?,
                card_data_revision: row.get(4)?,
                expected_terminal_hash: row.get(5)?,
                terminal_state_hash: row.get(6)?,
                outcome: row.get(7)?,
                detail: row.get(8)?,
                recorded_at: row.get::<_, i64>(9)?.max(0) as u64,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

fn load_latest_proof(data_dir: &Path, deck: &Deck) -> Result<Option<ComboProofReport>, String> {
    if !database_path(data_dir).is_file() {
        return Ok(None);
    }
    let connection = open(data_dir)?;
    let payload = connection
        .query_row(
            "SELECT payload_json FROM proofs WHERE deck_key=?1 ORDER BY id DESC LIMIT 1",
            [exact_deck_key(deck)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    payload
        .map(|payload| {
            let mut report: ComboProofReport =
                serde_json::from_str(&payload).map_err(|error| error.to_string())?;
            if report.proof_revision != COMBO_PROOF_REVISION {
                for line in &mut report.lines {
                    if matches!(
                        line.proof,
                        ComboProof::SymbolicallyProven
                            | ComboProof::ResourceBalancedHypothesis
                            | ComboProof::StateProven
                            | ComboProof::HexdekExecuted
                            | ComboProof::CrossEngineVerified
                            | ComboProof::ArenaVerified
                    ) {
                        line.proof = ComboProof::LegacyResourceHypothesis;
                        line.unsupported_assumptions.push(
                            "Legacy resource hypothesis—reprove under the current Rules IR before AI guidance or competitive claims."
                                .into(),
                        );
                    }
                }
                report.packages = report.lines.iter().map(package_for_line).collect();
            }
            Ok(report)
        })
        .transpose()
}

fn persist_proof(data_dir: &Path, deck: &Deck, report: &ComboProofReport) -> Result<(), String> {
    let connection = open(data_dir)?;
    let deck_key = exact_deck_key(deck);
    let payload = serde_json::to_string(report).map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT INTO proofs(deck_key, created_at, payload_json) VALUES(?1, ?2, ?3)",
            params![deck_key, unix_now().min(i64::MAX as u64) as i64, payload],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "DELETE FROM proofs WHERE id NOT IN (SELECT id FROM proofs ORDER BY id DESC LIMIT ?1)",
            [MAX_STORED_PROOFS as i64],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn exact_deck_key(deck: &Deck) -> String {
    format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(deck).unwrap_or_default())
    )
}

fn resolve_deck_cards<'a>(deck: &Deck, pool: &'a CardPool) -> Vec<&'a Card> {
    let ids = deck
        .command_zone
        .iter()
        .chain(&deck.mainboard)
        .map(|entry| &entry.card_id)
        .collect::<HashSet<_>>();
    pool.cards
        .iter()
        .filter(|card| ids.contains(&card.id))
        .collect()
}

fn normalized_limits(limits: &ComboSearchLimits) -> ComboSearchLimits {
    ComboSearchLimits {
        max_pieces: limits.max_pieces.clamp(2, 12),
        max_actions: limits.max_actions.clamp(2, 128),
        max_states: limits.max_states.clamp(100, 2_000_000),
        time_limit_ms: limits.time_limit_ms.clamp(50, 30_000),
    }
}

fn deduplicate_lines(lines: &mut Vec<ComboLine>) {
    lines.sort_by(|left, right| {
        proof_rank(right.proof)
            .cmp(&proof_rank(left.proof))
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut seen = HashSet::new();
    lines.retain(|line| seen.insert(line.id.clone()));
}

fn proof_rank(proof: ComboProof) -> u8 {
    match proof {
        ComboProof::CorpusOnly => 1,
        ComboProof::LegacyResourceHypothesis => 1,
        ComboProof::ResourceBalancedHypothesis | ComboProof::SymbolicallyProven => 2,
        ComboProof::StateProven => 3,
        ComboProof::HexdekExecuted => 4,
        ComboProof::CrossEngineVerified => 5,
        ComboProof::ArenaVerified => 6,
        // A dispute must win deduplication over an older optimistic proof of
        // the same line so stale symbolic evidence cannot hide disagreement.
        ComboProof::Disputed => 7,
        ComboProof::Unsupported => 0,
    }
}

fn combo_proof_label(proof: ComboProof) -> &'static str {
    match proof {
        ComboProof::CorpusOnly => "corpus_hypothesis",
        ComboProof::LegacyResourceHypothesis => "legacy_resource_hypothesis",
        ComboProof::ResourceBalancedHypothesis | ComboProof::SymbolicallyProven => {
            "resource_balanced_hypothesis"
        }
        ComboProof::StateProven => "state_proven",
        ComboProof::HexdekExecuted => "hexdek_executed",
        ComboProof::CrossEngineVerified => "cross_engine_verified",
        ComboProof::ArenaVerified => "arena_verified",
        ComboProof::Disputed => "disputed",
        ComboProof::Unsupported => "unsupported",
    }
}

fn proof_is_plan_eligible(proof: ComboProof) -> bool {
    matches!(
        proof,
        ComboProof::StateProven
            | ComboProof::HexdekExecuted
            | ComboProof::CrossEngineVerified
            | ComboProof::ArenaVerified
    )
}

fn line_id(cards: &[String], kind: &str) -> String {
    let mut cards = cards
        .iter()
        .map(|card| normalize_name(card))
        .collect::<Vec<_>>();
    cards.sort();
    format!(
        "combo:{:x}",
        Sha256::digest(format!("{kind}:{}", cards.join("|")))
    )
}

fn normalize_name(name: &str) -> String {
    name.split(" // ")
        .next()
        .unwrap_or(name)
        .trim()
        .to_ascii_lowercase()
}

fn database_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("combos.sqlite")
}

fn open(data_dir: &Path) -> Result<Connection, String> {
    fs::create_dir_all(data_dir).map_err(|error| error.to_string())?;
    let connection =
        Connection::open(database_path(data_dir)).map_err(|error| error.to_string())?;
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS combos(
               id TEXT PRIMARY KEY,
               status TEXT NOT NULL,
               description TEXT NOT NULL,
               card_names_json TEXT NOT NULL,
               payload_json TEXT NOT NULL,
               resolved INTEGER NOT NULL,
               updated_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS combo_cards(
               combo_id TEXT NOT NULL REFERENCES combos(id) ON DELETE CASCADE,
               position INTEGER NOT NULL,
               card_name TEXT NOT NULL,
               card_id TEXT,
               PRIMARY KEY(combo_id, position)
             );
             CREATE INDEX IF NOT EXISTS combo_cards_card_id ON combo_cards(card_id);
             CREATE TABLE IF NOT EXISTS proofs(
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               deck_key TEXT NOT NULL,
               created_at INTEGER NOT NULL,
               payload_json TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS proofs_deck_key ON proofs(deck_key, id DESC);
             CREATE TABLE IF NOT EXISTS combo_conformance(
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               deck_key TEXT NOT NULL,
               line_id TEXT NOT NULL,
               scenario_id TEXT NOT NULL,
               engine TEXT NOT NULL,
               engine_revision TEXT NOT NULL,
               card_data_revision TEXT NOT NULL,
               expected_terminal_hash TEXT NOT NULL,
               terminal_state_hash TEXT NOT NULL,
               outcome TEXT NOT NULL,
               detail TEXT NOT NULL,
               recorded_at INTEGER NOT NULL,
               UNIQUE(deck_key,line_id,scenario_id,engine,engine_revision,card_data_revision)
             );
             CREATE INDEX IF NOT EXISTS combo_conformance_line
               ON combo_conformance(deck_key,line_id,scenario_id);
             PRAGMA user_version=3;",
        )
        .map_err(|error| error.to_string())?;
    Ok(connection)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use metagene_core::{
        AbilityCostProfile, ActionTargetProfile, CardFeatures, CardMode, Color, DeckEntry,
        ManaCostProfile,
    };
    use std::collections::BTreeMap;
    use uuid::Uuid;

    fn action_card(
        name: &str,
        kind: AbilityKind,
        cost: AbilityCostProfile,
        value: f32,
        once: bool,
        label: &str,
    ) -> Card {
        Card {
            id: CardId(name.into()),
            arena_id: Some(1),
            platforms: vec!["arena".into()],
            name: name.into(),
            set_code: "TST".into(),
            collector_number: "1".into(),
            type_line: "Artifact".into(),
            oracle_text: label.into(),
            colors: vec![Color::Colorless],
            color_identity: Vec::new(),
            image_uri: None,
            legalities: BTreeMap::new(),
            features: CardFeatures {
                initial_modes: vec![CardMode {
                    name: name.into(),
                    type_line: "Artifact".into(),
                    abilities: vec![AbilityProfile {
                        label: label.into(),
                        kind,
                        timing: AbilityTiming::Activated,
                        zone: AbilityZone::Battlefield,
                        cost,
                        target: ActionTargetProfile::None,
                        conditions: Vec::new(),
                        effect_value: value,
                        narrow: false,
                        once_per_turn: once,
                        parser_confidence: 1.0,
                        clause_ids: Vec::new(),
                    }],
                    ..CardMode::default()
                }],
                ..CardFeatures::default()
            },
        }
    }

    #[test]
    fn corpus_is_versioned_resolved_and_replaced_atomically() {
        let data_dir = std::env::temp_dir().join(format!("metagene-combo-{}", Uuid::new_v4()));
        let pool = CardPool {
            cards: vec![
                action_card(
                    "Alpha",
                    AbilityKind::Draw,
                    AbilityCostProfile::default(),
                    1.0,
                    false,
                    "draw",
                ),
                action_card(
                    "Beta",
                    AbilityKind::Ramp,
                    AbilityCostProfile::default(),
                    1.0,
                    false,
                    "mana",
                ),
            ],
        };
        let values = vec![serde_json::json!({
            "id": "fixture-1",
            "status": "example",
            "description": "fixture",
            "uses": [{"card": {"name": "Alpha"}}, {"card": {"name": "Beta"}}]
        })];
        let report = replace_corpus(&data_dir, "fixture", &values, &pool, 1).unwrap();
        assert_eq!(report.downloaded, 1);
        assert_eq!(report.resolved, 1);
        assert_eq!(report.status.schema_version, 3);
        assert!(report.status.revision.unwrap().starts_with("sha256:"));
        fs::remove_dir_all(data_dir).unwrap();
    }

    #[test]
    fn proves_only_resource_complete_repeatable_loops() {
        let mana = action_card(
            "Mana Engine",
            AbilityKind::Ramp,
            AbilityCostProfile {
                uses_tap: true,
                ..AbilityCostProfile::default()
            },
            2.0,
            false,
            "add two mana",
        );
        let untap = action_card(
            "Untapper",
            AbilityKind::Unknown,
            AbilityCostProfile {
                mana: ManaCostProfile {
                    generic: 1,
                    ..ManaCostProfile::default()
                },
                ..AbilityCostProfile::default()
            },
            1.0,
            false,
            "untap target artifact",
        );
        let damage = action_card(
            "Payoff",
            AbilityKind::Damage,
            AbilityCostProfile::default(),
            1.0,
            false,
            "deal one damage",
        );
        let actions = [&mana, &untap, &damage]
            .into_iter()
            .flat_map(combo_actions)
            .collect::<Vec<_>>();
        let solved = solve_actions(&actions, &ComboSearchLimits::default(), Instant::now());
        assert!(
            solved
                .lines
                .iter()
                .any(|line| line.repeatable && line.loop_output.damage > 0)
        );
    }

    #[test]
    fn once_per_turn_and_unrecovered_sacrifice_are_not_infinite() {
        let once = action_card(
            "Once",
            AbilityKind::Damage,
            AbilityCostProfile::default(),
            1.0,
            true,
            "deal one damage",
        );
        let sacrifice = action_card(
            "Sacrifice",
            AbilityKind::Ramp,
            AbilityCostProfile {
                sacrifices_self: true,
                ..AbilityCostProfile::default()
            },
            3.0,
            false,
            "sacrifice: add three mana",
        );
        for card in [&once, &sacrifice] {
            let actions = combo_actions(card);
            let solved = solve_actions(&actions, &ComboSearchLimits::default(), Instant::now());
            assert!(solved.lines.is_empty());
        }
    }

    #[test]
    fn resource_hypotheses_cannot_enter_the_hexdek_overlay() {
        let mana = action_card(
            "Mana Engine",
            AbilityKind::Ramp,
            AbilityCostProfile {
                uses_tap: true,
                ..AbilityCostProfile::default()
            },
            2.0,
            false,
            "add two mana",
        );
        let untap = action_card(
            "Untapper",
            AbilityKind::Unknown,
            AbilityCostProfile {
                mana: ManaCostProfile {
                    generic: 1,
                    ..ManaCostProfile::default()
                },
                ..AbilityCostProfile::default()
            },
            1.0,
            false,
            "untap target artifact",
        );
        let once = action_card(
            "Once",
            AbilityKind::Damage,
            AbilityCostProfile::default(),
            1.0,
            true,
            "deal one damage",
        );
        let pool = CardPool {
            cards: vec![mana.clone(), untap.clone(), once.clone()],
        };
        let combo_deck = Deck {
            name: "Loop".into(),
            command_zone: Vec::new(),
            mainboard: vec![
                DeckEntry {
                    card_id: mana.id,
                    quantity: 1,
                },
                DeckEntry {
                    card_id: untap.id,
                    quantity: 1,
                },
            ],
            sideboard: Vec::new(),
        };
        let pseudo_deck = Deck {
            name: "Pseudo".into(),
            command_zone: Vec::new(),
            mainboard: vec![DeckEntry {
                card_id: once.id,
                quantity: 1,
            }],
            sideboard: Vec::new(),
        };
        let plans = hexdek_plan_for_decks(
            &[("deck-0.txt", &combo_deck), ("deck-1.txt", &pseudo_deck)],
            &pool,
        );
        assert_eq!(plans.schema_version, 1);
        assert!(plans.decks.is_empty());
        let payload = serde_json::to_string(&plans).unwrap();
        assert!(!payload.contains("resource_balanced_hypothesis"));
        assert!(!payload.contains("Once"));
    }

    #[test]
    fn resource_hypotheses_require_exact_state_reproof_before_engine_evidence() {
        let data_dir = std::env::temp_dir().join(format!("metagene-combo-{}", Uuid::new_v4()));
        let mana = action_card(
            "Mana Engine",
            AbilityKind::Ramp,
            AbilityCostProfile {
                uses_tap: true,
                ..AbilityCostProfile::default()
            },
            2.0,
            false,
            "add two mana",
        );
        let untap = action_card(
            "Untapper",
            AbilityKind::Unknown,
            AbilityCostProfile {
                mana: ManaCostProfile {
                    generic: 1,
                    ..ManaCostProfile::default()
                },
                ..AbilityCostProfile::default()
            },
            1.0,
            false,
            "untap target artifact",
        );
        let damage = action_card(
            "Payoff",
            AbilityKind::Damage,
            AbilityCostProfile::default(),
            1.0,
            false,
            "deal one damage",
        );
        let pool = CardPool {
            cards: vec![mana.clone(), untap.clone(), damage.clone()],
        };
        let deck = Deck {
            name: "Observed loop".into(),
            command_zone: Vec::new(),
            mainboard: vec![mana, untap, damage]
                .into_iter()
                .map(|card| DeckEntry {
                    card_id: card.id,
                    quantity: 1,
                })
                .collect(),
            sideboard: Vec::new(),
        };
        let proof = prove(
            &data_dir,
            &ComboProofRequest {
                deck: deck.clone(),
                pool,
                limits: ComboSearchLimits::default(),
            },
        )
        .unwrap();
        let line = proof
            .lines
            .iter()
            .find(|line| line.proof == ComboProof::ResourceBalancedHypothesis)
            .unwrap();
        let error = conformance_scenario(&data_dir, &deck, &line.id).unwrap_err();
        assert!(error.contains("exact state proof is required"));
        fs::remove_dir_all(data_dir).unwrap();
    }

    fn exact_action(
        id: &str,
        color: Color,
        mana_cost: u8,
        effect: RuleEffect,
        kind: ComboActionKind,
        resource_cost: i16,
        resource_effect: ComboResources,
    ) -> ComboAction {
        let mut colored = [0; 6];
        colored[color_slot(color)] = mana_cost;
        ComboAction {
            id: id.into(),
            card_id: CardId(id.into()),
            card_name: id.into(),
            mode_name: id.into(),
            source_types: vec!["artifact".into()],
            label: id.into(),
            kind,
            timing: AbilityTiming::Activated,
            zone: AbilityZone::Battlefield,
            cost: ComboResources {
                mana: resource_cost,
                ..ComboResources::default()
            },
            effect: resource_effect,
            parser_confidence: 1.0,
            rule_timing: RuleTiming::Activation,
            rule_costs: (mana_cost > 0)
                .then(|| {
                    RuleCost::Mana(ManaCostProfile {
                        colored,
                        ..ManaCostProfile::default()
                    })
                })
                .into_iter()
                .collect(),
            rule_effects: vec![effect],
            exact_state_supported: true,
            ..ComboAction::default()
        }
    }

    #[test]
    fn exact_state_proof_replays_colored_mana_and_a_second_iteration() {
        let mana = exact_action(
            "Blue Engine",
            Color::Blue,
            0,
            RuleEffect::AddMana {
                colors: vec![Color::Blue],
                amount: AmountExpression::One,
            },
            ComboActionKind::Mana,
            0,
            ComboResources {
                mana: 1,
                ..ComboResources::default()
            },
        );
        let damage = exact_action(
            "Blue Payoff",
            Color::Blue,
            1,
            RuleEffect::Damage {
                recipient: "opponent".into(),
                amount: AmountExpression::One,
            },
            ComboActionKind::Damage,
            1,
            ComboResources {
                damage: 1,
                ..ComboResources::default()
            },
        );
        let line = prove_ordered_component(&[mana, damage], 2, Instant::now()).unwrap();
        assert_eq!(line.proof, ComboProof::StateProven);
        let evidence = line.exact_state.unwrap();
        assert!(evidence.source_state_restored);
        assert!(evidence.another_iteration_legal);
        assert!(evidence.failure_reasons.is_empty());
    }

    #[test]
    fn wrong_colored_mana_and_missing_target_remain_hypotheses() {
        let red_mana = exact_action(
            "Red Engine",
            Color::Red,
            0,
            RuleEffect::AddMana {
                colors: vec![Color::Red],
                amount: AmountExpression::One,
            },
            ComboActionKind::Mana,
            0,
            ComboResources {
                mana: 1,
                ..ComboResources::default()
            },
        );
        let mut blue_damage = exact_action(
            "Blue Payoff",
            Color::Blue,
            1,
            RuleEffect::Damage {
                recipient: "opponent".into(),
                amount: AmountExpression::One,
            },
            ComboActionKind::Damage,
            1,
            ComboResources {
                damage: 1,
                ..ComboResources::default()
            },
        );
        let line =
            prove_ordered_component(&[red_mana.clone(), blue_damage.clone()], 2, Instant::now())
                .unwrap();
        assert_eq!(line.proof, ComboProof::ResourceBalancedHypothesis);
        assert!(!line.exact_state.unwrap().another_iteration_legal);

        blue_damage.rule_costs.clear();
        blue_damage.cost.mana = 0;
        blue_damage.rule_targets = vec![RuleTarget {
            controller: "opponent".into(),
            zones: vec![AbilityZone::Battlefield],
            card_types: vec!["creature".into()],
            amount: AmountExpression::One,
            ..RuleTarget::default()
        }];
        let line = prove_ordered_component(&[red_mana, blue_damage], 2, Instant::now()).unwrap();
        assert_eq!(line.proof, ComboProof::ResourceBalancedHypothesis);
        assert!(
            line.exact_state
                .unwrap()
                .failure_reasons
                .iter()
                .any(|reason| reason.contains("target"))
        );
    }
}
