use flate2::read::GzDecoder;
use metagene_core::{
    CancellationToken, CardPool, Deck, DeckConstraints, MatchupSideboardPlan, PlayDraw, RankedDeck,
    apply_sideboard_exchange, evaluate_structural_deck, validate_sideboard_plans,
};
use rayon::prelude::*;
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicUsize, Ordering as AtomicOrdering},
};
use std::time::{Duration, Instant};
use thiserror::Error;
use uuid::Uuid;

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const HEXDEK_BUNDLED_RUNTIME: &[u8] =
    include_bytes!("../resources/hexdek-runtime-windows-x64.tar.gz");
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const HEXDEK_BUNDLED_RUNTIME_SHA256: &str =
    "b7f8fbc8a932d9075e6fd55382cdd43334a9124d814f10d365b9e0f0bb72364d";

const HEXDEK_REPOSITORY: &str = "https://github.com/hexdek-labs/HexDek.git";
const SCRYFALL_BULK_API: &str = "https://api.scryfall.com/bulk-data";
const HEXDEK_COVERAGE_INDEX: &str = "coverage-index.json";
const HEXDEK_FIXED_ORDER_ADAPTER: &str = "metagene-fixed-order-commander-metrics-combo-plan-v3";
const HEXDEK_FIXED_ORDER_PATCH: &[u8] = include_bytes!("../resources/hexdek-fixed-order-v1.patch");
const HEXDEK_COMMANDER_METRICS_PATCH: &[u8] =
    include_bytes!("../resources/hexdek-commander-metrics-v1.patch");
const HEXDEK_COMBO_PLAN_PATCH: &[u8] = include_bytes!("../resources/hexdek-combo-plan-v1.patch");
const COMMANDER_OBJECTIVES_JSON: &str =
    include_str!("../../data/commander/multiplayer-objectives.schema1.json");

static COVERAGE_CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<HexdekCoverageIndex>>>> =
    OnceLock::new();

#[derive(Clone, Debug, Serialize)]
pub struct InstallerProgress {
    pub stage: String,
    pub detail: String,
    pub completed_units: u64,
    pub total_units: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SimulatorStatus {
    pub id: String,
    pub name: String,
    pub installed: bool,
    pub data_ready: bool,
    pub executable: String,
    pub supported_formats: Vec<String>,
    pub note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CoverageCardInput {
    pub name: String,
    pub quantity: usize,
    pub zone: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CoverageDeckInput {
    pub name: String,
    pub cards: Vec<CoverageCardInput>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CardCoverageIssue {
    pub name: String,
    pub quantity: usize,
    pub zone: String,
    pub status: String,
    pub parse_errors: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeckCoverage {
    pub deck_name: String,
    pub unique_cards: usize,
    pub card_copies: usize,
    pub present_unique: usize,
    pub parser_complete_unique: usize,
    pub partial_unique: usize,
    pub missing_unique: usize,
    pub semantic_risk_unique: usize,
    pub present_ratio: f64,
    pub parser_complete_ratio: f64,
    pub semantic_risk_ratio: f64,
    pub issues: Vec<CardCoverageIssue>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HexdekCoverageReport {
    pub engine: String,
    pub corpus_entries: usize,
    pub parser_revision: Option<String>,
    pub decks: Vec<DeckCoverage>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AstCoverage {
    fully_parsed: bool,
    parse_errors: Vec<String>,
    #[serde(default)]
    semantic_risks: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct HexdekCoverageIndex {
    schema: u32,
    corpus_entries: usize,
    parser_revision: Option<String>,
    cards: HashMap<String, AstCoverage>,
}

#[derive(Deserialize)]
struct AstDatasetLine {
    #[serde(rename = "__ast_type__")]
    ast_type: Option<String>,
    name: Option<String>,
    #[serde(default)]
    oracle_text: String,
    ast: Option<AstCardCoverage>,
    parser_git_sha: Option<String>,
    entry_count: Option<usize>,
}

#[derive(Deserialize)]
struct AstCardCoverage {
    #[serde(default)]
    fully_parsed: bool,
    #[serde(default)]
    parse_errors: Vec<serde_json::Value>,
}

fn semantic_risks(oracle_text: &str) -> Vec<String> {
    let text = oracle_text.to_ascii_lowercase();
    let mut risks = Vec::new();
    let mut add = |condition: bool, message: &str| {
        if condition {
            risks.push(message.to_string());
        }
    };
    add(
        text.contains(" instead") || text.starts_with("instead"),
        "Replacement-effect ordering requires interaction-level parity evidence",
    );
    add(
        text.contains("copy") || text.contains("copies"),
        "Copy semantics require characteristics, targets, and zone-change parity evidence",
    );
    add(
        text.contains("gain control")
            || text.contains("control of target")
            || text.contains("control enchanted"),
        "Control-changing effects require duration and layer parity evidence",
    );
    add(
        text.contains("extra turn") || text.contains("additional turn"),
        "Turn-structure effects require sequencing parity evidence",
    );
    add(
        [
            "conjure",
            "spellbook",
            "perpetually",
            "seek ",
            "intensity",
            "boon",
            "draft a card",
            "double team",
        ]
        .iter()
        .any(|mechanic| text.contains(mechanic)),
        "Arena digital mechanic requires live-client or Arena-aware engine evidence",
    );
    risks
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HexdekValidationRequest {
    pub candidate: Deck,
    pub opponents: Vec<Deck>,
    #[serde(default)]
    pub opponent_weights: Vec<f64>,
    pub pool: CardPool,
    pub games: usize,
    pub seed: i64,
    pub workers: usize,
    pub seats: usize,
    pub hat_budget: usize,
    pub max_turns: usize,
    pub game_timeout_seconds: usize,
    #[serde(default = "default_commander_mode")]
    pub commander: bool,
    /// Keep deck order fixed and start seat zero. This requires Metagene's
    /// installed fixed-order adapter and is used by explicit play/draw jobs.
    #[serde(default)]
    pub fixed_order: bool,
}

/// A constructed Best-of-Three request. `matches` is a match budget, not a
/// game budget: every completed match runs game one and then enough post-board
/// games to produce a winner.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BestOfThreeValidationRequest {
    pub candidate: Deck,
    pub opponents: Vec<Deck>,
    #[serde(default)]
    pub opponent_weights: Vec<f64>,
    pub pool: CardPool,
    pub constraints: DeckConstraints,
    pub sideboard_plans: Vec<MatchupSideboardPlan>,
    pub matches: usize,
    pub seed: i64,
    pub workers: usize,
    pub hat_budget: usize,
    pub max_turns: usize,
    pub game_timeout_seconds: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BestOfThreeEvidence {
    pub games: usize,
    pub wins: usize,
    pub draws: usize,
    pub crashes: usize,
    pub win_rate: f64,
    pub win_rate_lower_bound: f64,
    pub win_rate_upper_bound: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BestOfThreeMatchupReport {
    pub opponent_key: String,
    pub opponent_name: String,
    pub weight: f64,
    pub plan: MatchupSideboardPlan,
    pub preboard_on_play: BestOfThreeEvidence,
    pub preboard_on_draw: BestOfThreeEvidence,
    pub preboard: BestOfThreeEvidence,
    pub postboard_on_play: BestOfThreeEvidence,
    pub postboard_on_draw: BestOfThreeEvidence,
    pub postboard: BestOfThreeEvidence,
    pub matches: BestOfThreeEvidence,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BestOfThreeSimulationReport {
    pub engine: String,
    pub schema_version: String,
    pub provenance: crate::provenance::ResultProvenance,
    pub requested_matches: usize,
    pub completed_matches: usize,
    pub cached_games: usize,
    pub games_per_second: f64,
    pub preboard: BestOfThreeEvidence,
    pub postboard: BestOfThreeEvidence,
    pub matches: BestOfThreeEvidence,
    pub matchups: Vec<BestOfThreeMatchupReport>,
}

pub fn sideboard_opponent_key(
    opponent_index: usize,
    opponent: &Deck,
) -> Result<String, SimulatorError> {
    let hash = crate::provenance::configuration_hash("sideboard-opponent-v1", opponent)
        .map_err(|error| SimulatorError::InvalidSideboardPolicy(error.to_string()))?;
    Ok(format!("opponent-{}:{hash}", opponent_index + 1))
}

pub fn validate_best_of_three_request(
    request: &BestOfThreeValidationRequest,
) -> Result<(), SimulatorError> {
    if request.opponents.is_empty() {
        return Err(SimulatorError::NotEnoughDecks(2));
    }
    if request.matches < request.opponents.len() {
        return Err(SimulatorError::InvalidSideboardPolicy(format!(
            "at least one match per opponent is required ({} matches, {} opponents)",
            request.matches,
            request.opponents.len()
        )));
    }
    if request.sideboard_plans.len() != request.opponents.len() {
        return Err(SimulatorError::InvalidSideboardPolicy(format!(
            "received {} plans for {} opponents",
            request.sideboard_plans.len(),
            request.opponents.len()
        )));
    }
    validate_sideboard_plans(
        &request.candidate,
        &request.sideboard_plans,
        &request.pool,
        &request.constraints,
    )
    .map_err(|error| SimulatorError::InvalidSideboardPolicy(error.to_string()))?;
    for opponent in &request.opponents {
        metagene_core::validate_deck_legality(opponent, &request.pool, &request.constraints)
            .map_err(|error| {
                SimulatorError::InvalidSideboardPolicy(format!(
                    "opponent '{}' is illegal: {error}",
                    opponent.name
                ))
            })?;
    }

    let expected = request
        .opponents
        .iter()
        .enumerate()
        .map(|(index, opponent)| sideboard_opponent_key(index, opponent))
        .collect::<Result<HashSet<_>, _>>()?;
    let actual = request
        .sideboard_plans
        .iter()
        .map(|plan| plan.opponent_key.clone())
        .collect::<HashSet<_>>();
    if actual != expected {
        let mut missing = expected.difference(&actual).cloned().collect::<Vec<_>>();
        let mut unexpected = actual.difference(&expected).cloned().collect::<Vec<_>>();
        missing.sort();
        unexpected.sort();
        return Err(SimulatorError::InvalidSideboardPolicy(format!(
            "plan identities do not match the current gauntlet (missing: {}; unexpected: {})",
            missing.join(", "),
            unexpected.join(", ")
        )));
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatchupRow {
    pub deck: String,
    pub games: usize,
    pub wins: usize,
    pub win_rate: f64,
    #[serde(default)]
    pub win_rate_lower_bound: f64,
    #[serde(default)]
    pub win_rate_upper_bound: f64,
    pub average_turn_to_win: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SeatOrderRow {
    pub matchup: String,
    pub deck: String,
    pub position: usize,
    pub games: usize,
    pub wins: usize,
    pub win_rate: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SimulationReport {
    pub engine: String,
    pub schema_version: String,
    #[serde(default)]
    pub engine_routes: HashMap<String, String>,
    #[serde(default)]
    pub route_failures: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<crate::provenance::ResultProvenance>,
    pub games: usize,
    pub draws: usize,
    pub crashes: usize,
    #[serde(default)]
    pub cached_games: usize,
    #[serde(default)]
    pub seat_order: Vec<SeatOrderRow>,
    pub average_turns: f64,
    pub games_per_second: f64,
    pub rows: Vec<MatchupRow>,
    pub matchup_matrix: HashMap<String, HashMap<String, f64>>,
    #[serde(default)]
    pub matchup_wins: HashMap<String, HashMap<String, usize>>,
    #[serde(default)]
    pub matchup_games: HashMap<String, HashMap<String, usize>>,
    #[serde(default)]
    pub matchup_weights: HashMap<String, f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commander_evidence: Option<CommanderEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub combo_execution: Option<ComboExecutionEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub performance: Option<PerformanceBreakdown>,
}

/// Compact rules-engine telemetry for a proof-gated combo plan. These counters
/// describe plan reach and legal AI prioritization only. They never upgrade a
/// symbolic line to engine-executed proof without a deterministic conformance
/// scenario that observes the line's terminal state.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ComboExecutionEvidence {
    pub plan_available: bool,
    pub proof_revision: String,
    pub planned_lines: usize,
    pub assembling_games: usize,
    pub executable_games: usize,
    pub priority_actions: usize,
    pub plan_wins: usize,
    pub assembling_rate: f64,
    pub executable_rate: f64,
    pub win_conversion_rate: f64,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PerformanceBreakdown {
    pub coverage_ms: u64,
    pub cache_lookup_ms: u64,
    pub serialization_ms: u64,
    pub engine_ms: u64,
    pub parse_and_store_ms: u64,
    pub total_ms: u64,
    pub process_starts: usize,
    pub cache_hit: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommanderMetric {
    pub value: f64,
    pub source: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommanderSeatEvidence {
    pub position: usize,
    pub games: usize,
    pub wins: usize,
    pub win_share: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CommanderPenaltyEvidence {
    pub draw_or_stall_rate: f64,
    pub timeout_or_crash_rate: f64,
    pub concession_rate: f64,
    pub seat_bias: f64,
    pub kingmaking_signal: Option<f64>,
    pub total_penalty: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommanderEvidence {
    pub schema_version: u32,
    pub policy_id: String,
    pub evidence_mode: String,
    pub pod_size: usize,
    pub seed: i64,
    pub rotation_policy: String,
    pub complete_rotations: usize,
    pub partial_rotation_games: usize,
    pub pod_archetypes: Vec<String>,
    pub pod_win_share: CommanderMetric,
    pub seat_floor: CommanderMetric,
    pub threat_development: CommanderMetric,
    pub interaction: CommanderMetric,
    pub recovery: CommanderMetric,
    pub mana_reliability: CommanderMetric,
    pub closing_ability: CommanderMetric,
    pub seat_performance: Vec<CommanderSeatEvidence>,
    pub elimination_slots: Vec<usize>,
    pub target_actions: HashMap<String, usize>,
    pub penalties: CommanderPenaltyEvidence,
    pub objective_score: f64,
    pub limitations: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CommanderObjectivePolicy {
    schema_version: u32,
    policy_id: String,
    structural_trials: usize,
    closing_turn_scale: f64,
    ranking_weight: f64,
    objectives: CommanderObjectiveWeights,
    penalties: CommanderPenaltyWeights,
    notes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CommanderObjectiveWeights {
    pod_win_share: f64,
    seat_floor: f64,
    threat_development: f64,
    interaction: f64,
    recovery: f64,
    mana_reliability: f64,
    closing_ability: f64,
}

#[derive(Debug, Deserialize)]
struct CommanderPenaltyWeights {
    draw_or_stall_rate: f64,
    timeout_or_crash_rate: f64,
    concession_rate: f64,
    seat_bias: f64,
    kingmaking_signal: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SuccessiveHalvingRequest {
    pub candidates: Vec<RankedDeck>,
    pub opponents: Vec<Deck>,
    #[serde(default)]
    pub opponent_weights: Vec<f64>,
    pub pool: CardPool,
    pub base_games: usize,
    pub rounds: usize,
    pub seed: i64,
    pub workers: usize,
    pub seats: usize,
    pub hat_budget: usize,
    pub max_turns: usize,
    pub game_timeout_seconds: usize,
    pub structural_weight: f64,
    #[serde(default = "default_risk_aversion")]
    pub risk_aversion: f64,
    #[serde(default = "default_worst_matchup_weight")]
    pub worst_matchup_weight: f64,
    #[serde(default = "default_seat_position_weight")]
    pub seat_position_weight: f64,
    #[serde(default = "default_adaptive_elimination")]
    pub adaptive_elimination: bool,
    #[serde(default = "default_commander_mode")]
    pub commander: bool,
    #[serde(default)]
    pub route_plan: Option<crate::competitive::EngineRoutePlan>,
}

fn default_commander_mode() -> bool {
    true
}

fn default_risk_aversion() -> f64 {
    0.5
}

fn default_worst_matchup_weight() -> f64 {
    0.25
}

fn default_seat_position_weight() -> f64 {
    0.25
}

fn default_adaptive_elimination() -> bool {
    true
}

fn commander_objective_policy() -> &'static CommanderObjectivePolicy {
    static POLICY: OnceLock<CommanderObjectivePolicy> = OnceLock::new();
    POLICY.get_or_init(|| {
        let policy: CommanderObjectivePolicy = serde_json::from_str(COMMANDER_OBJECTIVES_JSON)
            .expect("embedded Commander objective policy must parse");
        assert_eq!(
            policy.schema_version, 1,
            "unsupported Commander objective schema"
        );
        assert!(
            !policy.policy_id.trim().is_empty(),
            "Commander objective policy id is required"
        );
        assert!(
            policy.structural_trials > 0,
            "Commander structural trials are required"
        );
        policy
    })
}

fn default_upper_bound() -> f64 {
    1.0
}

fn wilson_interval(wins: usize, games: usize) -> (f64, f64) {
    if games == 0 {
        return (0.0, 1.0);
    }
    let z = 1.959_963_984_540_054_f64;
    let n = games as f64;
    let p = wins.min(games) as f64 / n;
    let z_squared = z * z;
    let denominator = 1.0 + z_squared / n;
    let center = (p + z_squared / (2.0 * n)) / denominator;
    let margin = z * (p * (1.0 - p) / n + z_squared / (4.0 * n * n)).sqrt() / denominator;
    (
        (center - margin).clamp(0.0, 1.0),
        (center + margin).clamp(0.0, 1.0),
    )
}

#[derive(Clone, Copy)]
struct ConfidenceScoreInput {
    win_rate: f64,
    win_rate_lower_bound: f64,
    worst_matchup_win_rate: f64,
    worst_matchup_lower_bound: f64,
    crash_rate: f64,
    seat_position: Option<(f64, f64)>,
}

#[derive(Clone, Copy)]
struct ConfidencePolicy {
    risk_aversion: f64,
    worst_matchup_weight: f64,
    seat_position_weight: f64,
}

fn confidence_adjusted_score(input: ConfidenceScoreInput, policy: ConfidencePolicy) -> f64 {
    let risk_aversion = policy.risk_aversion.clamp(0.0, 1.0);
    let worst_matchup_weight = policy.worst_matchup_weight.clamp(0.0, 1.0);
    let overall =
        input.win_rate * (1.0 - risk_aversion) + input.win_rate_lower_bound * risk_aversion;
    let worst = input.worst_matchup_win_rate * (1.0 - risk_aversion)
        + input.worst_matchup_lower_bound * risk_aversion;
    let matchup_adjusted = overall * (1.0 - worst_matchup_weight) + worst * worst_matchup_weight;
    let position_adjusted =
        input
            .seat_position
            .map_or(matchup_adjusted, |(win_rate, lower_bound)| {
                let position = win_rate * (1.0 - risk_aversion) + lower_bound * risk_aversion;
                let weight = policy.seat_position_weight.clamp(0.0, 1.0);
                matchup_adjusted * (1.0 - weight) + position * weight
            });
    (position_adjusted - input.crash_rate).clamp(0.0, 1.0)
}

fn evidence_bound(
    overall: f64,
    worst_matchup: f64,
    crash_rate: f64,
    worst_matchup_weight: f64,
    worst_position: Option<f64>,
    seat_position_weight: f64,
) -> f64 {
    let matchup_weight = worst_matchup_weight.clamp(0.0, 1.0);
    let matchup_adjusted = overall * (1.0 - matchup_weight) + worst_matchup * matchup_weight;
    let position_adjusted = worst_position.map_or(matchup_adjusted, |position| {
        let position_weight = seat_position_weight.clamp(0.0, 1.0);
        matchup_adjusted * (1.0 - position_weight) + position * position_weight
    });
    (position_adjusted - crash_rate).clamp(0.0, 1.0)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HalvingCandidateResult {
    pub candidate_rank: usize,
    pub commander: String,
    pub structural_score: f64,
    pub round_win_rate: f64,
    pub game_win_rate: f64,
    pub cumulative_games: usize,
    pub crash_rate: f64,
    #[serde(default)]
    pub win_rate_lower_bound: f64,
    #[serde(default)]
    pub win_rate_upper_bound: f64,
    #[serde(default)]
    pub worst_matchup_win_rate: f64,
    #[serde(default)]
    pub worst_matchup_lower_bound: f64,
    #[serde(default = "default_upper_bound")]
    pub worst_matchup_upper_bound: f64,
    #[serde(default)]
    pub worst_position_win_rate: Option<f64>,
    #[serde(default)]
    pub worst_position_lower_bound: Option<f64>,
    #[serde(default)]
    pub worst_position_upper_bound: Option<f64>,
    #[serde(default)]
    pub position_win_rate_spread: Option<f64>,
    #[serde(default)]
    pub evidence_score: f64,
    #[serde(default)]
    pub evidence_lower_bound: f64,
    #[serde(default = "default_upper_bound")]
    pub evidence_upper_bound: f64,
    pub blended_score: f64,
    #[serde(default)]
    pub blended_lower_bound: f64,
    #[serde(default = "default_upper_bound")]
    pub blended_upper_bound: f64,
    pub report: Option<SimulationReport>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug)]
struct SeatPositionSummary {
    worst_rate: f64,
    worst_lower: f64,
    worst_upper: f64,
    spread: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidationEvidenceScore {
    pub observed: f64,
    pub lower_bound: f64,
    pub upper_bound: f64,
}

pub fn score_validation_report(
    report: &SimulationReport,
    candidate: &str,
    risk_aversion: f64,
    worst_matchup_weight: f64,
    seat_position_weight: f64,
) -> Option<ValidationEvidenceScore> {
    let row = report.rows.iter().find(|row| row.deck == candidate)?;
    let matchup_evidence = report
        .matchup_wins
        .get(candidate)
        .into_iter()
        .flat_map(|matchups| matchups.iter())
        .filter_map(|(opponent, wins)| {
            let games = report.matchup_games.get(candidate)?.get(opponent)?;
            (*games > 0).then(|| {
                let (lower, upper) = wilson_interval(*wins, *games);
                (*wins as f64 / *games as f64, lower, upper)
            })
        })
        .collect::<Vec<_>>();
    let worst = matchup_evidence.iter().copied().fold(
        (
            row.win_rate,
            row.win_rate_lower_bound,
            row.win_rate_upper_bound,
        ),
        |current, matchup| {
            (
                current.0.min(matchup.0),
                current.1.min(matchup.1),
                current.2.min(matchup.2),
            )
        },
    );
    let mut positions = HashMap::<usize, (usize, usize)>::new();
    for seat in report
        .seat_order
        .iter()
        .filter(|seat| seat.deck == candidate && seat.games > 0)
    {
        let total = positions.entry(seat.position).or_default();
        total.0 = total.0.saturating_add(seat.wins);
        total.1 = total.1.saturating_add(seat.games);
    }
    let position = seat_position_summary(&positions);
    let crash_rate =
        report.crashes as f64 / report.games.saturating_add(report.crashes).max(1) as f64;
    let confidence = confidence_adjusted_score(
        ConfidenceScoreInput {
            win_rate: row.win_rate,
            win_rate_lower_bound: row.win_rate_lower_bound,
            worst_matchup_win_rate: worst.0,
            worst_matchup_lower_bound: worst.1,
            crash_rate,
            seat_position: position.map(|summary| (summary.worst_rate, summary.worst_lower)),
        },
        ConfidencePolicy {
            risk_aversion,
            worst_matchup_weight,
            seat_position_weight,
        },
    );
    let lower = evidence_bound(
        row.win_rate_lower_bound,
        worst.1,
        crash_rate,
        worst_matchup_weight,
        position.map(|summary| summary.worst_lower),
        seat_position_weight,
    );
    let upper = evidence_bound(
        row.win_rate_upper_bound,
        worst.2,
        crash_rate,
        worst_matchup_weight,
        position.map(|summary| summary.worst_upper),
        seat_position_weight,
    );
    let commander_objective = report
        .commander_evidence
        .as_ref()
        .map(|evidence| evidence.objective_score);
    let weight = commander_objective
        .map(|_| commander_objective_policy().ranking_weight.clamp(0.0, 1.0))
        .unwrap_or_default();
    let objective = commander_objective.unwrap_or(confidence);
    Some(ValidationEvidenceScore {
        observed: confidence * (1.0 - weight) + objective * weight,
        lower_bound: lower * (1.0 - weight) + objective * weight,
        upper_bound: upper * (1.0 - weight) + objective * weight,
    })
}

fn seat_position_summary(totals: &HashMap<usize, (usize, usize)>) -> Option<SeatPositionSummary> {
    let mut rates = totals
        .values()
        .filter(|(_, games)| *games > 0)
        .map(|(wins, games)| {
            let rate = *wins as f64 / *games as f64;
            let (lower, upper) = wilson_interval(*wins, *games);
            (rate, lower, upper)
        });
    let (first_rate, first_lower, first_upper) = rates.next()?;
    let mut minimum_rate = first_rate;
    let mut minimum_lower = first_lower;
    let mut minimum_upper = first_upper;
    let mut maximum_rate = first_rate;
    for (rate, lower, upper) in rates {
        minimum_rate = minimum_rate.min(rate);
        minimum_lower = minimum_lower.min(lower);
        minimum_upper = minimum_upper.min(upper);
        maximum_rate = maximum_rate.max(rate);
    }
    Some(SeatPositionSummary {
        worst_rate: minimum_rate,
        worst_lower: minimum_lower,
        worst_upper: minimum_upper,
        spread: maximum_rate - minimum_rate,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HalvingRound {
    pub round: usize,
    pub games_per_candidate: usize,
    pub entrants: usize,
    pub survivors: Vec<usize>,
    #[serde(default)]
    pub statistically_eliminated: Vec<usize>,
    pub results: Vec<HalvingCandidateResult>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SuccessiveHalvingReport {
    pub engine: String,
    pub total_games: usize,
    pub total_attempts: usize,
    #[serde(default)]
    pub maximum_game_budget: usize,
    #[serde(default)]
    pub scheduled_game_budget: usize,
    #[serde(default)]
    pub avoided_game_budget: usize,
    pub total_crashes: usize,
    pub elapsed_ms: u64,
    pub winner_rank: Option<usize>,
    #[serde(default)]
    pub cancelled: bool,
    pub rounds: Vec<HalvingRound>,
}

fn maximum_halving_game_budget(
    candidates: usize,
    base_games: usize,
    rounds: usize,
    matchup_multiplier: usize,
    commander: bool,
    seats: usize,
) -> usize {
    let mut entrants = candidates;
    let mut budget = 0usize;
    for round in 0..rounds.clamp(1, 8) {
        if entrants == 0 {
            break;
        }
        let games = complete_rotation_games(
            base_games
                .clamp(1, 10_000_000)
                .saturating_mul(1usize << round)
                .min(10_000_000),
            commander,
            seats,
        );
        budget = budget.saturating_add(
            entrants
                .saturating_mul(games)
                .saturating_mul(matchup_multiplier.max(1)),
        );
        if entrants == 1 {
            break;
        }
        entrants = entrants.div_ceil(2);
    }
    budget
}

fn complete_rotation_games(requested: usize, commander: bool, seats: usize) -> usize {
    let requested = requested.clamp(1, 10_000_000);
    if !commander {
        return requested;
    }
    let seats = seats.clamp(2, 6);
    let rounded = requested.div_ceil(seats).saturating_mul(seats);
    if rounded <= 10_000_000 {
        rounded
    } else {
        10_000_000 - (10_000_000 % seats)
    }
}

pub fn weighted_game_allocations(
    games_per_opponent: usize,
    opponent_count: usize,
    weights: &[f64],
) -> Vec<usize> {
    if opponent_count == 0 {
        return Vec::new();
    }
    let total_games = games_per_opponent.max(1).saturating_mul(opponent_count);
    let usable = weights.len() == opponent_count
        && weights
            .iter()
            .all(|weight| weight.is_finite() && *weight > 0.0);
    let weights = if usable {
        weights.to_vec()
    } else {
        vec![1.0; opponent_count]
    };
    let weight_total = weights.iter().sum::<f64>();
    let raw = weights
        .iter()
        .map(|weight| total_games as f64 * *weight / weight_total)
        .collect::<Vec<_>>();
    let mut allocations = raw
        .iter()
        .map(|value| value.floor().max(1.0) as usize)
        .collect::<Vec<_>>();
    let mut allocated = allocations.iter().sum::<usize>();
    let mut order = (0..opponent_count).collect::<Vec<_>>();
    order.sort_by(|left, right| {
        (raw[*right] - raw[*right].floor())
            .total_cmp(&(raw[*left] - raw[*left].floor()))
            .then_with(|| left.cmp(right))
    });
    let mut cursor = 0usize;
    while allocated < total_games {
        let index = order[cursor % order.len()];
        allocations[index] = allocations[index].saturating_add(1);
        allocated += 1;
        cursor += 1;
    }
    while allocated > total_games {
        let Some(index) = order
            .iter()
            .rev()
            .copied()
            .find(|index| allocations[*index] > 1)
        else {
            break;
        };
        allocations[index] -= 1;
        allocated -= 1;
    }
    allocations
}

#[derive(Clone, Debug, Serialize)]
pub struct HalvingProgress {
    pub round: usize,
    pub total_rounds: usize,
    pub candidate_index: usize,
    pub candidates_in_round: usize,
    pub candidate_rank: usize,
    pub games_per_candidate: usize,
    pub completed_games: usize,
    pub completed_attempts: usize,
}

#[derive(Debug, Error)]
pub enum SimulatorError {
    #[error("HexDek is not installed or its AST corpus is missing; expected files under {0}")]
    NotInstalled(String),
    #[error("HexDek validation requires at least {0} decks")]
    NotEnoughDecks(usize),
    #[error("each HexDek seat must have a distinct primary commander name")]
    DuplicateCommander,
    #[error("constructed validation requires a distinct representative card in every deck")]
    NoDistinctIdentifiers,
    #[error("HexDek's card corpus is missing cards required by this matchup: {0}")]
    MissingCards(String),
    #[error("simulation cancelled")]
    Cancelled,
    #[error("deck references a card absent from the supplied pool: {0}")]
    UnknownCard(String),
    #[error("invalid Best-of-Three sideboard policy: {0}")]
    InvalidSideboardPolicy(String),
    #[error("simulator file error: {0}")]
    Io(#[from] std::io::Error),
    #[error("simulator download error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("HexDek exited unsuccessfully: {0}")]
    Process(String),
    #[error("invalid HexDek result: {0}")]
    Result(#[from] serde_json::Error),
    #[error("unsupported HexDek result schema: {0}")]
    UnsupportedSchema(String),
}

#[derive(Deserialize)]
struct ScryfallBulkList {
    data: Vec<ScryfallBulkItem>,
}

#[derive(Deserialize)]
struct ScryfallBulkItem {
    #[serde(rename = "type")]
    kind: String,
    download_uri: Option<String>,
    jsonl_download_uri: Option<String>,
    size: Option<u64>,
    compressed_size: Option<u64>,
    updated_at: Option<String>,
}

#[derive(Deserialize)]
struct HexdekResult {
    schema_version: String,
    #[serde(default)]
    mode: String,
    games: usize,
    crashes: usize,
    draws: usize,
    avg_turns: f64,
    commander_names: Vec<String>,
    wins_by_commander: HashMap<String, usize>,
    games_played_by_commander: Option<HashMap<String, usize>>,
    wins_by_commander_by_seat: Option<HashMap<String, Vec<usize>>>,
    games_by_commander_by_seat: Option<HashMap<String, Vec<usize>>>,
    #[serde(default)]
    wins_by_seat: Vec<usize>,
    #[serde(default)]
    elimination_by_commander_by_slot: HashMap<String, Vec<usize>>,
    #[serde(default)]
    total_concessions: usize,
    #[serde(default)]
    n_seats: usize,
    #[serde(default)]
    commander_metrics: HashMap<String, CommanderNativeMetricSummary>,
    avg_turn_to_win: Option<HashMap<String, f64>>,
    matchup_matrix: Option<HashMap<String, HashMap<String, usize>>>,
    matchup_games: Option<HashMap<String, HashMap<String, usize>>>,
    games_per_second: f64,
}

#[derive(Deserialize)]
struct CommanderNativeMetricSummary {
    games: usize,
    lands_played: usize,
    mana_spent: usize,
    mana_wasted: usize,
    damage_dealt: usize,
    damage_taken: usize,
    creatures_lost: usize,
    creatures_killed: usize,
    spells_cast: usize,
    counters_cast: usize,
    removal_cast: usize,
    peak_board_size: usize,
    final_life: i64,
    survived: usize,
    #[serde(default)]
    target_actions: HashMap<String, usize>,
    kingmaking_signals: usize,
    #[serde(default)]
    combo_assembling_games: usize,
    #[serde(default)]
    combo_executable_games: usize,
    #[serde(default)]
    combo_priority_actions: usize,
    #[serde(default)]
    combo_plan_wins: usize,
}

pub fn statuses(data_dir: &Path) -> Vec<SimulatorStatus> {
    let hexdek = hexdek_paths(data_dir);
    let hexdek_files_ready = hexdek.executable.is_file()
        && hexdek.conformance.is_file()
        && hexdek.ast.is_file()
        && hexdek.oracle.is_file();
    let hexdek_adapter_current = hexdek_adapter_is_current(&hexdek.root);
    vec![
        SimulatorStatus {
            id: "structural".into(),
            name: "Metagene structural screen".into(),
            installed: true,
            data_ready: true,
            executable: "built in".into(),
            supported_formats: vec!["all".into()],
            note: "Fast proxy evaluation; not a rules-complete game simulation.".into(),
        },
        SimulatorStatus {
            id: "hexdek".into(),
            name: "HexDek Yggdrasil".into(),
            installed: hexdek.executable.is_file(),
            data_ready: hexdek_files_ready && hexdek_adapter_current,
            executable: hexdek.executable.display().to_string(),
            supported_formats: vec!["commander".into(), "constructed".into()],
            note: if hexdek_files_ready && !hexdek_adapter_current {
                "A bundled HexDek upgrade is required before validation; Metagene installs it without a compiler."
                    .into()
            } else {
                "External MIT-licensed rules engine with a Metagene conformance companion; validates Commander pods and constructed matchups. Arena-only or rebalanced card semantics still require per-card auditing."
                    .into()
            },
        },
    ]
}

fn hexdek_adapter_is_current(root: &Path) -> bool {
    fs::read_to_string(root.join("VERSION"))
        .ok()
        .is_some_and(|version| {
            version
                .lines()
                .any(|line| line.trim() == format!("adapter={HEXDEK_FIXED_ORDER_ADAPTER}"))
        })
}

fn normalized_card_name(name: &str) -> String {
    let trimmed = name.trim();
    let faces = trimmed.split(" // ").collect::<Vec<_>>();
    let canonical = if faces.len() > 1 && faces.iter().all(|face| *face == faces[0]) {
        faces[0]
    } else {
        trimmed
    };
    canonical.to_lowercase()
}

fn coverage_cache() -> &'static Mutex<HashMap<PathBuf, Arc<HexdekCoverageIndex>>> {
    COVERAGE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn invalidate_coverage_cache(root: &Path) {
    if let Ok(mut cache) = coverage_cache().lock() {
        cache.remove(root);
    }
}

fn build_hexdek_coverage_index(
    ast_path: &Path,
    index_path: &Path,
) -> Result<HexdekCoverageIndex, SimulatorError> {
    let reader = BufReader::new(fs::File::open(ast_path)?);
    let mut cards = HashMap::new();
    let mut parser_revision = None;
    let mut manifest_entries = None;
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let record: AstDatasetLine = serde_json::from_str(&line)?;
        if record.ast_type.as_deref() == Some("DatasetManifest") {
            parser_revision = record.parser_git_sha;
            manifest_entries = record.entry_count;
            continue;
        }
        let Some(name) = record.name else {
            continue;
        };
        let semantic_risks = semantic_risks(&record.oracle_text);
        let ast = record.ast.unwrap_or(AstCardCoverage {
            fully_parsed: false,
            parse_errors: Vec::new(),
        });
        let parse_errors = ast
            .parse_errors
            .into_iter()
            .map(|error| {
                let value = error
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| error.to_string());
                value.chars().take(240).collect()
            })
            .collect::<Vec<_>>();
        cards.insert(
            normalized_card_name(&name),
            AstCoverage {
                fully_parsed: ast.fully_parsed && parse_errors.is_empty(),
                parse_errors,
                semantic_risks,
            },
        );
    }
    let index = HexdekCoverageIndex {
        schema: 4,
        corpus_entries: manifest_entries.unwrap_or(cards.len()),
        parser_revision,
        cards,
    };
    let pending = index_path.with_extension(format!("pending-{}", Uuid::new_v4()));
    let mut writer = BufWriter::new(fs::File::create(&pending)?);
    serde_json::to_writer(&mut writer, &index)?;
    writer.flush()?;
    if index_path.exists() {
        fs::remove_file(index_path)?;
    }
    fs::rename(pending, index_path)?;
    Ok(index)
}

fn load_hexdek_coverage_index(data_dir: &Path) -> Result<Arc<HexdekCoverageIndex>, SimulatorError> {
    let paths = hexdek_paths(data_dir);
    if !paths.ast.is_file() {
        return Err(SimulatorError::NotInstalled(
            paths.root.display().to_string(),
        ));
    }
    if let Some(index) = coverage_cache()
        .lock()
        .map_err(|_| SimulatorError::Process("coverage cache poisoned".into()))?
        .get(&paths.root)
        .cloned()
    {
        return Ok(index);
    }
    let index_path = paths.root.join(HEXDEK_COVERAGE_INDEX);
    let index = if index_path.is_file() {
        let existing = fs::File::open(&index_path).ok().and_then(|file| {
            serde_json::from_reader::<_, HexdekCoverageIndex>(BufReader::new(file)).ok()
        });
        match existing {
            Some(existing) if existing.schema == 4 => existing,
            _ => build_hexdek_coverage_index(&paths.ast, &index_path)?,
        }
    } else {
        build_hexdek_coverage_index(&paths.ast, &index_path)?
    };
    let index = Arc::new(index);
    coverage_cache()
        .lock()
        .map_err(|_| SimulatorError::Process("coverage cache poisoned".into()))?
        .insert(paths.root, index.clone());
    Ok(index)
}

pub fn coverage_inputs_from_decks(
    decks: &[&Deck],
    pool: &CardPool,
) -> Result<Vec<CoverageDeckInput>, SimulatorError> {
    let cards: HashMap<_, _> = pool.cards.iter().map(|card| (&card.id, card)).collect();
    decks
        .iter()
        .map(|deck| {
            let mut entries = Vec::new();
            for (zone, deck_entries) in [
                ("command", deck.command_zone.as_slice()),
                ("mainboard", deck.mainboard.as_slice()),
                ("sideboard", deck.sideboard.as_slice()),
            ] {
                for entry in deck_entries {
                    let card = cards
                        .get(&entry.card_id)
                        .ok_or_else(|| SimulatorError::UnknownCard(entry.card_id.0.clone()))?;
                    entries.push(CoverageCardInput {
                        name: card.name.clone(),
                        quantity: entry.quantity as usize,
                        zone: zone.into(),
                    });
                }
            }
            Ok(CoverageDeckInput {
                name: deck.name.clone(),
                cards: entries,
            })
        })
        .collect()
}

pub fn audit_hexdek_coverage(
    data_dir: &Path,
    decks: &[CoverageDeckInput],
) -> Result<HexdekCoverageReport, SimulatorError> {
    let index = load_hexdek_coverage_index(data_dir)?;
    let decks = decks
        .iter()
        .map(|deck| {
            let mut merged: HashMap<String, (String, usize, HashSet<String>)> = HashMap::new();
            for card in &deck.cards {
                let key = normalized_card_name(&card.name);
                let entry = merged
                    .entry(key)
                    .or_insert_with(|| (card.name.clone(), 0, HashSet::new()));
                entry.1 = entry.1.saturating_add(card.quantity);
                entry.2.insert(card.zone.clone());
            }
            let unique_cards = merged.len();
            let card_copies = merged.values().map(|entry| entry.1).sum::<usize>();
            let mut present_unique = 0usize;
            let mut parser_complete_unique = 0usize;
            let mut present_copies = 0usize;
            let mut parser_complete_copies = 0usize;
            let mut semantic_risk_unique = 0usize;
            let mut semantic_risk_copies = 0usize;
            let mut issues = Vec::new();
            for (key, (name, quantity, zones)) in merged {
                match index.cards.get(&key) {
                    Some(coverage) if coverage.fully_parsed => {
                        present_unique += 1;
                        parser_complete_unique += 1;
                        present_copies = present_copies.saturating_add(quantity);
                        parser_complete_copies = parser_complete_copies.saturating_add(quantity);
                        if !coverage.semantic_risks.is_empty() {
                            semantic_risk_unique += 1;
                            semantic_risk_copies = semantic_risk_copies.saturating_add(quantity);
                            issues.push(CardCoverageIssue {
                                name,
                                quantity,
                                zone: zones.into_iter().collect::<Vec<_>>().join(" + "),
                                status: "semantic_risk".into(),
                                parse_errors: coverage.semantic_risks.clone(),
                            });
                        }
                    }
                    Some(coverage) => {
                        present_unique += 1;
                        present_copies = present_copies.saturating_add(quantity);
                        issues.push(CardCoverageIssue {
                            name,
                            quantity,
                            zone: zones.into_iter().collect::<Vec<_>>().join(" + "),
                            status: "partial".into(),
                            parse_errors: coverage.parse_errors.clone(),
                        });
                    }
                    None => issues.push(CardCoverageIssue {
                        name,
                        quantity,
                        zone: zones.into_iter().collect::<Vec<_>>().join(" + "),
                        status: "missing".into(),
                        parse_errors: Vec::new(),
                    }),
                }
            }
            issues.sort_by(|left, right| {
                left.status
                    .cmp(&right.status)
                    .then_with(|| left.name.cmp(&right.name))
            });
            let partial_unique = present_unique.saturating_sub(parser_complete_unique);
            Ok(DeckCoverage {
                deck_name: deck.name.clone(),
                unique_cards,
                card_copies,
                present_unique,
                parser_complete_unique,
                partial_unique,
                missing_unique: unique_cards.saturating_sub(present_unique),
                semantic_risk_unique,
                present_ratio: present_copies as f64 / card_copies.max(1) as f64,
                parser_complete_ratio: parser_complete_copies as f64 / card_copies.max(1) as f64,
                semantic_risk_ratio: semantic_risk_copies as f64 / card_copies.max(1) as f64,
                issues,
            })
        })
        .collect::<Result<Vec<_>, SimulatorError>>()?;
    Ok(HexdekCoverageReport {
        engine: "hexdek".into(),
        corpus_entries: index.corpus_entries,
        parser_revision: index.parser_revision.clone(),
        decks,
    })
}

pub fn filter_pool_for_hexdek(
    data_dir: &Path,
    pool: &CardPool,
    policy: &str,
) -> Result<CardPool, SimulatorError> {
    if policy == "all" {
        return Ok(pool.clone());
    }
    let index = load_hexdek_coverage_index(data_dir)?;
    let require_complete = policy == "parser_complete";
    if !require_complete && policy != "present" {
        return Err(SimulatorError::Process(format!(
            "unknown simulator coverage policy: {policy}"
        )));
    }
    Ok(CardPool {
        cards: pool
            .cards
            .iter()
            .filter(|card| {
                index
                    .cards
                    .get(&normalized_card_name(&card.name))
                    .is_some_and(|coverage| !require_complete || coverage.fully_parsed)
            })
            .cloned()
            .collect(),
    })
}

/// Installs only HexDek's runtime artifacts under the operating-system app-data
/// directory. Source, Go modules, compiler caches, and the raw build tree live in
/// a uniquely named staging directory and are deleted before the install is made
/// visible, keeping generated data out of the repository.
pub fn install_hexdek(
    data_dir: &Path,
    mut progress: impl FnMut(InstallerProgress),
) -> Result<SimulatorStatus, SimulatorError> {
    if statuses(data_dir)
        .into_iter()
        .any(|status| status.id == "hexdek" && status.installed && status.data_ready)
    {
        return statuses(data_dir)
            .into_iter()
            .find(|status| status.id == "hexdek")
            .ok_or_else(|| {
                SimulatorError::Process("installed engine status was not found".into())
            });
    }
    let engines_root = data_dir.join("engines");
    fs::create_dir_all(&engines_root)?;
    let installation_id = Uuid::new_v4().to_string();
    let staging = engines_root.join(format!(".hexdek-install-{installation_id}"));
    let backup = engines_root.join(format!(".hexdek-backup-{installation_id}"));
    let final_paths = hexdek_paths(data_dir);
    fs::create_dir(&staging)?;

    let outcome = install_hexdek_runtime(&staging, &mut progress).and_then(|_| {
        progress(InstallerProgress {
            stage: "finalizing".into(),
            detail: "Activating the validated runtime".into(),
            completed_units: 0,
            total_units: None,
        });
        if final_paths.root.exists() {
            fs::rename(&final_paths.root, &backup)?;
        }
        if let Err(error) = fs::rename(&staging, &final_paths.root) {
            if backup.exists() {
                let _ = fs::rename(&backup, &final_paths.root);
            }
            return Err(error.into());
        }
        if backup.exists() {
            fs::remove_dir_all(&backup)?;
        }
        Ok(())
    });

    if outcome.is_err() && staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    outcome?;
    invalidate_coverage_cache(&final_paths.root);
    progress(InstallerProgress {
        stage: "complete".into(),
        detail: "HexDek and its card corpus are ready".into(),
        completed_units: 1,
        total_units: Some(1),
    });
    statuses(data_dir)
        .into_iter()
        .find(|status| status.id == "hexdek")
        .ok_or_else(|| SimulatorError::Process("installed engine status was not found".into()))
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn install_hexdek_runtime(
    staging: &Path,
    progress: &mut impl FnMut(InstallerProgress),
) -> Result<(), SimulatorError> {
    progress(InstallerProgress {
        stage: "extracting-bundled-engine".into(),
        detail: "Installing the bundled HexDek runtime — no compiler required".into(),
        completed_units: 0,
        total_units: Some(HEXDEK_BUNDLED_RUNTIME.len() as u64),
    });
    let actual = format!("{:x}", Sha256::digest(HEXDEK_BUNDLED_RUNTIME));
    if actual != HEXDEK_BUNDLED_RUNTIME_SHA256 {
        return Err(SimulatorError::Process(format!(
            "bundled HexDek checksum mismatch: expected {HEXDEK_BUNDLED_RUNTIME_SHA256}, got {actual}"
        )));
    }
    tar::Archive::new(GzDecoder::new(Cursor::new(HEXDEK_BUNDLED_RUNTIME))).unpack(staging)?;
    let paths = [
        "hexdek-tournament.exe",
        "hexdek-conformance.exe",
        "ast_dataset.jsonl",
        "coverage-index.json",
        "oracle-cards.json",
        "LICENSE-HexDek",
        "VERSION",
    ];
    if let Some(missing) = paths.iter().find(|name| !staging.join(name).is_file()) {
        return Err(SimulatorError::Process(format!(
            "bundled HexDek runtime is missing {missing}"
        )));
    }
    let version = fs::read_to_string(staging.join("VERSION"))?;
    if !version.contains(&format!("adapter={HEXDEK_FIXED_ORDER_ADAPTER}")) {
        return Err(SimulatorError::Process(
            "bundled HexDek adapter revision is not compatible with this app".into(),
        ));
    }
    progress(InstallerProgress {
        stage: "bundled-engine-ready".into(),
        detail: "Bundled HexDek runtime verified".into(),
        completed_units: HEXDEK_BUNDLED_RUNTIME.len() as u64,
        total_units: Some(HEXDEK_BUNDLED_RUNTIME.len() as u64),
    });
    Ok(())
}

#[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
fn install_hexdek_runtime(
    staging: &Path,
    progress: &mut impl FnMut(InstallerProgress),
) -> Result<(), SimulatorError> {
    install_hexdek_staged(staging, progress)
}

#[allow(dead_code)]
fn install_hexdek_staged(
    staging: &Path,
    progress: &mut impl FnMut(InstallerProgress),
) -> Result<(), SimulatorError> {
    require_command("git", &["--version"])?;
    require_command("go", &["version"])?;
    let python = if command_succeeds("python", &["--version"]) {
        "python"
    } else if command_succeeds("python3", &["--version"]) {
        "python3"
    } else {
        return Err(SimulatorError::Process(
            "Python 3 is required to prepare HexDek's rules corpus".into(),
        ));
    };

    let source = staging.join("source");
    progress(InstallerProgress {
        stage: "source".into(),
        detail: "Downloading the HexDek source snapshot".into(),
        completed_units: 0,
        total_units: None,
    });
    run_checked(
        Command::new("git").args([
            "clone",
            "--depth",
            "1",
            "--filter=blob:none",
            "--no-checkout",
            HEXDEK_REPOSITORY,
            &source.display().to_string(),
        ]),
        "download HexDek source",
    )?;
    run_checked(
        Command::new("git").arg("-C").arg(&source).args([
            "sparse-checkout",
            "set",
            "--no-cone",
            "cmd/hexdek-tournament",
            "internal",
            "scripts",
            "go.mod",
            "go.sum",
            "LICENSE",
        ]),
        "select the HexDek runtime source",
    )?;
    run_checked(
        Command::new("git").arg("-C").arg(&source).arg("checkout"),
        "materialize the HexDek runtime source",
    )?;
    let revision = command_stdout(
        Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(["rev-parse", "HEAD"]),
        "read HexDek revision",
    )?;
    let adapter_patch = source.join("metagene-fixed-order.patch");
    fs::write(&adapter_patch, HEXDEK_FIXED_ORDER_PATCH)?;
    run_checked(
        Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(["apply", "--check"])
            .arg(&adapter_patch),
        "validate Metagene's fixed-order HexDek adapter",
    )?;
    run_checked(
        Command::new("git")
            .arg("-C")
            .arg(&source)
            .arg("apply")
            .arg(&adapter_patch),
        "apply Metagene's fixed-order HexDek adapter",
    )?;
    fs::remove_file(adapter_patch)?;
    let metrics_patch = source.join("metagene-commander-metrics.patch");
    fs::write(&metrics_patch, HEXDEK_COMMANDER_METRICS_PATCH)?;
    run_checked(
        Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(["apply", "--check"])
            .arg(&metrics_patch),
        "validate Metagene's Commander-metrics HexDek adapter",
    )?;
    run_checked(
        Command::new("git")
            .arg("-C")
            .arg(&source)
            .arg("apply")
            .arg(&metrics_patch),
        "apply Metagene's Commander-metrics HexDek adapter",
    )?;
    fs::remove_file(metrics_patch)?;
    let combo_patch = source.join("metagene-combo-plan.patch");
    fs::write(&combo_patch, HEXDEK_COMBO_PLAN_PATCH)?;
    run_checked(
        Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(["apply", "--check"])
            .arg(&combo_patch),
        "validate Metagene's combo-plan HexDek adapter",
    )?;
    run_checked(
        Command::new("git")
            .arg("-C")
            .arg(&source)
            .arg("apply")
            .arg(&combo_patch),
        "apply Metagene's combo-plan HexDek adapter",
    )?;
    fs::remove_file(combo_patch)?;

    progress(InstallerProgress {
        stage: "building".into(),
        detail: "Compiling the optimized Rust-hosted simulator worker".into(),
        completed_units: 0,
        total_units: None,
    });
    let executable = staging.join(if cfg!(windows) {
        "hexdek-tournament.exe"
    } else {
        "hexdek-tournament"
    });
    let conformance_source = source.join("cmd").join("metagene-conformance");
    fs::create_dir_all(&conformance_source)?;
    fs::write(
        conformance_source.join("main.go"),
        crate::hexdek::HEXDEK_ADAPTER_SOURCE,
    )?;
    let mut build = Command::new("go");
    build
        .current_dir(&source)
        .env("GOTOOLCHAIN", "auto")
        .env("GOCACHE", staging.join("go-cache"))
        .env("GOMODCACHE", staging.join("go-modules"))
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.longpaths")
        .env("GIT_CONFIG_VALUE_0", "true")
        .args(["build", "-trimpath", "-ldflags=-s -w", "-o"])
        .arg(&executable)
        .arg("./cmd/hexdek-tournament");
    run_checked(&mut build, "compile HexDek")?;
    let conformance_executable = staging.join(if cfg!(windows) {
        "hexdek-conformance.exe"
    } else {
        "hexdek-conformance"
    });
    let mut conformance_build = Command::new("go");
    conformance_build
        .current_dir(&source)
        .env("GOTOOLCHAIN", "auto")
        .env("GOCACHE", staging.join("go-cache"))
        .env("GOMODCACHE", staging.join("go-modules"))
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.longpaths")
        .env("GIT_CONFIG_VALUE_0", "true")
        .args(["build", "-trimpath", "-ldflags=-s -w", "-o"])
        .arg(&conformance_executable)
        .arg("./cmd/metagene-conformance");
    run_checked(&mut conformance_build, "compile HexDek conformance adapter")?;

    progress(InstallerProgress {
        stage: "oracle".into(),
        detail: "Locating Scryfall's current Oracle card data".into(),
        completed_units: 0,
        total_units: None,
    });
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    let client = Client::builder()
        .user_agent("Metagene/0.1 (local MTG evolution workbench)")
        .default_headers(headers)
        .timeout(Duration::from_secs(900))
        .build()?;
    let bulk: ScryfallBulkList = client
        .get(SCRYFALL_BULK_API)
        .send()?
        .error_for_status()?
        .json()?;
    let oracle = bulk
        .data
        .into_iter()
        .find(|item| {
            item.kind == "oracle_cards"
                && (item.download_uri.is_some() || item.jsonl_download_uri.is_some())
        })
        .ok_or_else(|| {
            SimulatorError::Process("Scryfall did not advertise oracle_cards bulk data".into())
        })?;
    let oracle_path = staging.join("oracle-cards.json");
    if let Some(url) = oracle.jsonl_download_uri.as_deref() {
        let compressed = staging.join("oracle-cards.jsonl.gz");
        download_with_progress(&client, url, &compressed, oracle.compressed_size, progress)?;
        progress(InstallerProgress {
            stage: "oracle".into(),
            detail: "Expanding compressed Oracle JSONL for HexDek".into(),
            completed_units: 0,
            total_units: None,
        });
        jsonl_gzip_to_json_array(&compressed, &oracle_path)?;
        fs::remove_file(compressed)?;
    } else {
        download_with_progress(
            &client,
            oracle.download_uri.as_deref().expect("filtered above"),
            &oracle_path,
            oracle.size,
            progress,
        )?;
    }

    progress(InstallerProgress {
        stage: "corpus".into(),
        detail: "Parsing Oracle text into HexDek's compact AST corpus".into(),
        completed_units: 0,
        total_units: None,
    });
    let rules_dir = source.join("data").join("rules");
    fs::create_dir_all(&rules_dir)?;
    let source_oracle = rules_dir.join("oracle-cards.json");
    if fs::hard_link(&oracle_path, &source_oracle).is_err() {
        fs::copy(&oracle_path, &source_oracle)?;
    }
    let mut parser = Command::new(python);
    parser
        .current_dir(&source)
        .env("PYTHONUTF8", "1")
        .arg(source.join("scripts").join("export_ast_dataset.py"));
    run_checked(&mut parser, "generate HexDek AST corpus")?;
    fs::rename(
        rules_dir.join("ast_dataset.jsonl"),
        staging.join("ast_dataset.jsonl"),
    )?;
    build_hexdek_coverage_index(
        &staging.join("ast_dataset.jsonl"),
        &staging.join(HEXDEK_COVERAGE_INDEX),
    )?;
    fs::copy(source.join("LICENSE"), staging.join("LICENSE-HexDek"))?;
    fs::write(
        staging.join("VERSION"),
        format!(
            "revision={revision}\nadapter={HEXDEK_FIXED_ORDER_ADAPTER}\nscryfall_updated_at={}\n",
            oracle.updated_at.unwrap_or_else(|| "unknown".into())
        ),
    )?;

    progress(InstallerProgress {
        stage: "cleanup".into(),
        detail: "Removing source, compiler caches, and downloaded modules".into(),
        completed_units: 0,
        total_units: None,
    });
    for path in [source, staging.join("go-cache"), staging.join("go-modules")] {
        if path.exists() {
            fs::remove_dir_all(path)?;
        }
    }
    Ok(())
}

fn download_with_progress(
    client: &Client,
    url: &str,
    destination: &Path,
    advertised_size: Option<u64>,
    progress: &mut impl FnMut(InstallerProgress),
) -> Result<(), SimulatorError> {
    let mut response = client.get(url).send()?.error_for_status()?;
    let total = response.content_length().or(advertised_size);
    let temporary = destination.with_extension("download");
    let mut file = fs::File::create(&temporary)?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut downloaded = 0_u64;
    let report_interval = total
        .map(|bytes| (bytes / 100).max(256 * 1024))
        .unwrap_or(1024 * 1024);
    let mut next_report = report_interval;
    loop {
        let read = response.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])?;
        downloaded += read as u64;
        if downloaded >= next_report || total.is_some_and(|bytes| downloaded >= bytes) {
            progress(InstallerProgress {
                stage: "oracle".into(),
                detail: "Downloading Scryfall Oracle cards".into(),
                completed_units: downloaded,
                total_units: total,
            });
            next_report = downloaded.saturating_add(report_interval);
        }
    }
    file.sync_all()?;
    fs::rename(temporary, destination)?;
    Ok(())
}

fn jsonl_gzip_to_json_array(source: &Path, destination: &Path) -> Result<(), SimulatorError> {
    let decoder = GzDecoder::new(fs::File::open(source)?);
    let mut reader = BufReader::new(decoder);
    let temporary = destination.with_extension("converting");
    let mut writer = BufWriter::new(fs::File::create(&temporary)?);
    writer.write_all(b"[")?;
    let mut line = String::new();
    let mut first = true;
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let object = line.trim();
        if object.is_empty() {
            continue;
        }
        if !first {
            writer.write_all(b",")?;
        }
        writer.write_all(object.as_bytes())?;
        first = false;
    }
    writer.write_all(b"]")?;
    writer.flush()?;
    drop(writer);
    fs::rename(temporary, destination)?;
    Ok(())
}

fn command_succeeds(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .output()
        .is_ok_and(|output| output.status.success())
}

fn require_command(program: &str, args: &[&str]) -> Result<(), SimulatorError> {
    if command_succeeds(program, args) {
        Ok(())
    } else {
        Err(SimulatorError::Process(format!(
            "{program} is required to install HexDek"
        )))
    }
}

fn run_checked(command: &mut Command, action: &str) -> Result<(), SimulatorError> {
    let output = command.output()?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    Err(SimulatorError::Process(format!(
        "could not {action} ({}): {}{}",
        output.status,
        stderr.chars().take(3_000).collect::<String>(),
        stdout.chars().take(1_000).collect::<String>()
    )))
}

fn command_stdout(command: &mut Command, action: &str) -> Result<String, SimulatorError> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(SimulatorError::Process(format!(
            "could not {action}: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn run_hexdek(
    data_dir: &Path,
    request: &HexdekValidationRequest,
) -> Result<SimulationReport, SimulatorError> {
    run_hexdek_cancellable(data_dir, request, &CancellationToken::default())
}

pub fn run_hexdek_cancellable(
    data_dir: &Path,
    request: &HexdekValidationRequest,
    cancellation: &CancellationToken,
) -> Result<SimulationReport, SimulatorError> {
    let total_started = Instant::now();
    let paths = hexdek_paths(data_dir);
    if !paths.executable.is_file() || !paths.ast.is_file() || !paths.oracle.is_file() {
        return Err(SimulatorError::NotInstalled(
            paths.root.display().to_string(),
        ));
    }
    let seats = request.seats.clamp(2, 6);
    let decks: Vec<&Deck> = std::iter::once(&request.candidate)
        .chain(request.opponents.iter())
        .take(seats)
        .collect();
    if decks.len() < seats {
        return Err(SimulatorError::NotEnoughDecks(seats));
    }
    let coverage_started = Instant::now();
    let coverage_inputs = coverage_inputs_from_decks(&decks, &request.pool)?;
    let coverage = audit_hexdek_coverage(data_dir, &coverage_inputs)?;
    let coverage_ms = elapsed_ms(coverage_started);
    let missing = coverage
        .decks
        .iter()
        .flat_map(|deck| deck.issues.iter())
        .filter(|issue| issue.status == "missing")
        .map(|issue| issue.name.as_str())
        .collect::<HashSet<_>>();
    if !missing.is_empty() {
        let mut names = missing.into_iter().collect::<Vec<_>>();
        names.sort_unstable();
        return Err(SimulatorError::MissingCards(
            names.into_iter().take(12).collect::<Vec<_>>().join(", "),
        ));
    }
    let identifiers = if request.commander {
        let commander_names: Vec<String> = decks
            .iter()
            .map(|deck| primary_commander_name(deck, &request.pool))
            .collect::<Result<_, _>>()?;
        if commander_names.iter().collect::<HashSet<_>>().len() != commander_names.len() {
            return Err(SimulatorError::DuplicateCommander);
        }
        commander_names
    } else {
        distinct_constructed_identifiers(&decks, &request.pool)?
    };
    if cancellation.is_cancelled() {
        return Err(SimulatorError::Cancelled);
    }
    let cache_started = Instant::now();
    let cache_identity = hexdek_cache_identity(&paths, request, &decks, &identifiers, seats)?;
    if let Ok(Some(mut report)) = crate::match_cache::lookup(data_dir, &cache_identity.cache_key) {
        enrich_commander_evidence(&mut report, request, &identifiers);
        report.performance = Some(PerformanceBreakdown {
            coverage_ms,
            cache_lookup_ms: elapsed_ms(cache_started),
            total_ms: elapsed_ms(total_started),
            cache_hit: true,
            ..PerformanceBreakdown::default()
        });
        return Ok(report);
    }
    let cache_lookup_ms = elapsed_ms(cache_started);

    let jobs_root = data_dir.join("simulation-jobs");
    fs::create_dir_all(&jobs_root)?;
    let job_dir = jobs_root.join(Uuid::new_v4().to_string());
    fs::create_dir_all(&job_dir)?;
    let outcome = run_hexdek_job(
        &paths,
        &job_dir,
        request,
        &decks,
        &identifiers,
        seats,
        cancellation,
    );
    let cleanup = fs::remove_dir_all(&job_dir);
    match (outcome, cleanup) {
        (Ok(mut outcome), _) => {
            let store_started = Instant::now();
            enrich_commander_evidence(&mut outcome.report, request, &identifiers);
            outcome.report.provenance = Some(cache_identity.provenance.clone());
            let _ = crate::match_cache::store(
                data_dir,
                &cache_identity.cache_key,
                "hexdek",
                &cache_identity.cache_revision,
                &outcome.report,
            );
            outcome.report.performance = Some(PerformanceBreakdown {
                coverage_ms,
                cache_lookup_ms,
                serialization_ms: outcome.serialization_ms,
                engine_ms: outcome.engine_ms,
                parse_and_store_ms: outcome.parse_ms.saturating_add(elapsed_ms(store_started)),
                total_ms: elapsed_ms(total_started),
                process_starts: 1,
                cache_hit: false,
            });
            Ok(outcome.report)
        }
        (Err(error), _) => Err(error),
    }
}

struct MatchIdentity {
    cache_key: String,
    cache_revision: String,
    provenance: crate::provenance::ResultProvenance,
}

fn hexdek_cache_identity(
    paths: &HexdekPaths,
    request: &HexdekValidationRequest,
    decks: &[&Deck],
    identifiers: &[String],
    seats: usize,
) -> Result<MatchIdentity, SimulatorError> {
    let version = fs::read_to_string(paths.root.join("VERSION")).unwrap_or_else(|_| {
        let executable_bytes = fs::metadata(&paths.executable)
            .map(|metadata| metadata.len())
            .unwrap_or_default();
        let corpus_bytes = fs::metadata(&paths.ast)
            .map(|metadata| metadata.len())
            .unwrap_or_default();
        format!("unversioned-exe-{executable_bytes}-corpus-{corpus_bytes}")
    });
    let engine_revision = version
        .lines()
        .find_map(|line| line.strip_prefix("revision="))
        .filter(|value| !value.trim().is_empty())
        .map(str::trim)
        .unwrap_or("unversioned")
        .to_string();
    let engine_revision = version
        .lines()
        .find_map(|line| line.strip_prefix("adapter="))
        .filter(|value| !value.trim().is_empty())
        .map_or(engine_revision.clone(), |adapter| {
            format!("{engine_revision}+adapter:{}", adapter.trim())
        });
    let card_data_revision = version
        .lines()
        .find_map(|line| line.strip_prefix("scryfall_updated_at="))
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("scryfall:{}", value.trim()))
        .unwrap_or_else(|| "legacy-unknown".into());
    let deck_texts = decks
        .iter()
        .enumerate()
        .map(|(index, deck)| {
            if request.commander {
                export_hexdek(deck, &request.pool)
            } else {
                export_hexdek_constructed(deck, &request.pool, &identifiers[index])
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let input = serde_json::json!({
        "decks": deck_texts,
        "games": request.games.clamp(1, 10_000_000),
        "seed": request.seed,
        "seats": seats,
        "hat_budget": request.hat_budget.min(10_000),
        "max_turns": request.max_turns.clamp(1, 1_000),
        "game_timeout_seconds": request.game_timeout_seconds.clamp(1, 3_600),
        "commander": request.commander,
        "fixed_order": request.fixed_order,
        "commander_objective_policy": request.commander.then(|| commander_objective_policy().policy_id.clone()),
    });
    let configuration_hash = crate::provenance::configuration_hash("hexdek-matchup-v1", &input)?;
    let cache_revision = format!("{engine_revision}+cards:{card_data_revision}");
    let cache_key = crate::match_cache::key("hexdek", &cache_revision, &input)
        .map_err(|error| SimulatorError::Process(error.to_string()))?;
    Ok(MatchIdentity {
        cache_key,
        cache_revision,
        provenance: crate::provenance::ResultProvenance::new(
            engine_revision,
            card_data_revision,
            None,
            configuration_hash,
        ),
    })
}

pub fn run_validation_gauntlet(
    data_dir: &Path,
    request: &HexdekValidationRequest,
) -> Result<SimulationReport, SimulatorError> {
    run_validation_gauntlet_cancellable(data_dir, request, &CancellationToken::default())
}

pub fn run_validation_gauntlet_cancellable(
    data_dir: &Path,
    request: &HexdekValidationRequest,
    cancellation: &CancellationToken,
) -> Result<SimulationReport, SimulatorError> {
    if request.commander {
        return run_hexdek_cancellable(data_dir, request, cancellation);
    }
    if request.opponents.is_empty() {
        return Err(SimulatorError::NotEnoughDecks(2));
    }

    let mut rows: HashMap<String, (usize, usize, f64, usize)> = HashMap::new();
    let mut matchup_wins: HashMap<String, HashMap<String, usize>> = HashMap::new();
    let mut matchup_games: HashMap<String, HashMap<String, usize>> = HashMap::new();
    let allocations = weighted_game_allocations(
        request.games,
        request.opponents.len(),
        &request.opponent_weights,
    );
    let allocated_total = allocations.iter().sum::<usize>().max(1);
    let mut matchup_weights = HashMap::new();
    let candidate_name = format!("Candidate · {}", request.candidate.name);
    let mut total_games = 0usize;
    let mut total_draws = 0usize;
    let mut total_crashes = 0usize;
    let mut total_cached_games = 0usize;
    let mut weighted_turns = 0.0;
    let mut estimated_seconds = 0.0;
    let mut schema_version = None;
    let mut seat_order = Vec::new();
    let mut provenance_template = None;
    let mut matchup_configuration_hashes = Vec::new();
    let mut performance = PerformanceBreakdown::default();
    let mut measured_performance = false;

    for (opponent_index, opponent) in request.opponents.iter().enumerate() {
        if cancellation.is_cancelled() {
            return Err(SimulatorError::Cancelled);
        }
        let identifiers =
            distinct_constructed_identifiers(&[&request.candidate, opponent], &request.pool)?;
        let candidate_marker = identifiers[0].clone();
        let opponent_marker = identifiers[1].clone();
        let stable_opponent = format!("Opponent {} · {}", opponent_index + 1, opponent.name);
        matchup_weights.insert(
            stable_opponent.clone(),
            allocations[opponent_index] as f64 / allocated_total as f64,
        );
        let mut matchup_request = request.clone();
        matchup_request.opponents = vec![opponent.clone()];
        matchup_request.opponent_weights = vec![1.0];
        matchup_request.games = allocations[opponent_index];
        matchup_request.seats = 2;
        matchup_request.seed = request
            .seed
            .wrapping_add((opponent_index as i64).wrapping_mul(2_000_033));
        let report = run_hexdek_cancellable(data_dir, &matchup_request, cancellation)?;
        if let Some(measured) = report.performance.as_ref() {
            measured_performance = true;
            performance.coverage_ms = performance.coverage_ms.saturating_add(measured.coverage_ms);
            performance.cache_lookup_ms = performance
                .cache_lookup_ms
                .saturating_add(measured.cache_lookup_ms);
            performance.serialization_ms = performance
                .serialization_ms
                .saturating_add(measured.serialization_ms);
            performance.engine_ms = performance.engine_ms.saturating_add(measured.engine_ms);
            performance.parse_and_store_ms = performance
                .parse_and_store_ms
                .saturating_add(measured.parse_and_store_ms);
            performance.total_ms = performance.total_ms.saturating_add(measured.total_ms);
            performance.process_starts = performance
                .process_starts
                .saturating_add(measured.process_starts);
            performance.cache_hit |= measured.cache_hit;
        }
        if let Some(provenance) = report.provenance.as_ref() {
            provenance_template.get_or_insert_with(|| provenance.clone());
            matchup_configuration_hashes.push(provenance.configuration_hash.clone());
        }
        schema_version.get_or_insert_with(|| report.schema_version.clone());
        total_games = total_games.saturating_add(report.games);
        total_draws = total_draws.saturating_add(report.draws);
        total_crashes = total_crashes.saturating_add(report.crashes);
        total_cached_games = total_cached_games.saturating_add(report.cached_games);
        weighted_turns += report.average_turns * report.games as f64;
        estimated_seconds += report.games as f64 / report.games_per_second.max(f64::EPSILON);

        for (source, stable_name) in [
            (candidate_marker.as_str(), candidate_name.as_str()),
            (opponent_marker.as_str(), stable_opponent.as_str()),
        ] {
            let Some(row) = report.rows.iter().find(|row| row.deck == source) else {
                continue;
            };
            let entry = rows.entry(stable_name.to_string()).or_default();
            entry.0 = entry.0.saturating_add(row.wins);
            entry.1 = entry.1.saturating_add(row.games);
            if let Some(turn) = row.average_turn_to_win {
                entry.2 += turn * row.wins as f64;
                entry.3 = entry.3.saturating_add(row.wins);
            }
        }

        for mut seat in report.seat_order {
            seat.matchup = stable_opponent.clone();
            if seat.deck == candidate_marker {
                seat.deck = candidate_name.clone();
            } else if seat.deck == opponent_marker {
                seat.deck = stable_opponent.clone();
            }
            seat_order.push(seat);
        }

        let candidate_wins = report
            .matchup_wins
            .get(&candidate_marker)
            .and_then(|values| values.get(&opponent_marker))
            .copied()
            .unwrap_or_else(|| {
                report
                    .rows
                    .iter()
                    .find(|row| row.deck == candidate_marker)
                    .map(|row| row.wins)
                    .unwrap_or(0)
            });
        let games = report
            .matchup_games
            .get(&candidate_marker)
            .and_then(|values| values.get(&opponent_marker))
            .copied()
            .unwrap_or(report.games);
        let opponent_wins = report
            .matchup_wins
            .get(&opponent_marker)
            .and_then(|values| values.get(&candidate_marker))
            .copied()
            .unwrap_or_else(|| {
                report
                    .rows
                    .iter()
                    .find(|row| row.deck == opponent_marker)
                    .map(|row| row.wins)
                    .unwrap_or(0)
            });
        matchup_wins
            .entry(candidate_name.clone())
            .or_default()
            .insert(stable_opponent.clone(), candidate_wins);
        matchup_games
            .entry(candidate_name.clone())
            .or_default()
            .insert(stable_opponent.clone(), games);
        matchup_wins
            .entry(stable_opponent.clone())
            .or_default()
            .insert(candidate_name.clone(), opponent_wins);
        matchup_games
            .entry(stable_opponent)
            .or_default()
            .insert(candidate_name.clone(), games);
    }

    let rows = rows
        .into_iter()
        .map(|(deck, (wins, games, weighted_win_turns, turn_wins))| {
            let (win_rate_lower_bound, win_rate_upper_bound) = wilson_interval(wins, games);
            MatchupRow {
                deck,
                games,
                wins,
                win_rate: wins as f64 / games.max(1) as f64,
                win_rate_lower_bound,
                win_rate_upper_bound,
                average_turn_to_win: (turn_wins > 0)
                    .then_some(weighted_win_turns / turn_wins as f64),
            }
        })
        .collect::<Vec<_>>();
    let matchup_matrix = matchup_wins
        .iter()
        .map(|(left, opponents)| {
            let rates = opponents
                .iter()
                .map(|(right, wins)| {
                    let games = matchup_games
                        .get(left)
                        .and_then(|values| values.get(right))
                        .copied()
                        .unwrap_or(0);
                    (right.clone(), *wins as f64 / games.max(1) as f64)
                })
                .collect();
            (left.clone(), rates)
        })
        .collect();
    let mut rows = rows;
    rows.sort_by(|left, right| {
        usize::from(right.deck == candidate_name)
            .cmp(&usize::from(left.deck == candidate_name))
            .then_with(|| left.deck.cmp(&right.deck))
    });
    let provenance = if let Some(mut provenance) = provenance_template {
        provenance.configuration_hash = crate::provenance::configuration_hash(
            "hexdek-constructed-gauntlet-v1",
            &serde_json::json!({
                "matchups": matchup_configuration_hashes,
                "weights": &matchup_weights,
            }),
        )?;
        Some(provenance)
    } else {
        None
    };
    Ok(SimulationReport {
        engine: "hexdek".into(),
        schema_version: schema_version.unwrap_or_else(|| "1.0.0".into()),
        engine_routes: HashMap::new(),
        route_failures: Vec::new(),
        provenance,
        games: total_games,
        draws: total_draws,
        crashes: total_crashes,
        cached_games: total_cached_games,
        seat_order,
        average_turns: weighted_turns / total_games.max(1) as f64,
        games_per_second: total_games as f64 / estimated_seconds.max(f64::EPSILON),
        rows,
        matchup_matrix,
        matchup_wins,
        matchup_games,
        matchup_weights,
        commander_evidence: None,
        combo_execution: None,
        performance: measured_performance.then_some(performance),
    })
}

#[derive(Default)]
struct BestOfThreeBatch {
    evidence: BestOfThreeEvidence,
    opponent_wins: usize,
    cached_games: usize,
    estimated_seconds: f64,
    provenance: Option<crate::provenance::ResultProvenance>,
}

pub(crate) struct FixedOrderEvaluation {
    pub evidence: BestOfThreeEvidence,
    pub cached_games: usize,
    pub estimated_seconds: f64,
    pub provenance: crate::provenance::ResultProvenance,
}

pub(crate) struct FixedOrderBranch {
    pub candidate_on_play: bool,
    pub games: usize,
    pub seed: i64,
}

pub(crate) fn evaluate_fixed_order_branch(
    data_dir: &Path,
    request: &BestOfThreeValidationRequest,
    candidate: &Deck,
    opponent: &Deck,
    branch: FixedOrderBranch,
    cancellation: &CancellationToken,
) -> Result<FixedOrderEvaluation, SimulatorError> {
    require_fixed_order_adapter(data_dir)?;
    let batch = run_best_of_three_batch(
        data_dir,
        request,
        candidate,
        opponent,
        FixedOrderBranch {
            games: branch.games.max(1),
            ..branch
        },
        cancellation,
    )?;
    Ok(FixedOrderEvaluation {
        evidence: batch.evidence,
        cached_games: batch.cached_games,
        estimated_seconds: batch.estimated_seconds,
        provenance: batch.provenance.ok_or_else(|| {
            SimulatorError::Process("fixed-order evaluation returned no provenance".into())
        })?,
    })
}

pub fn run_best_of_three(
    data_dir: &Path,
    request: &BestOfThreeValidationRequest,
) -> Result<BestOfThreeSimulationReport, SimulatorError> {
    run_best_of_three_cancellable(data_dir, request, &CancellationToken::default())
}

pub fn run_best_of_three_cancellable(
    data_dir: &Path,
    request: &BestOfThreeValidationRequest,
    cancellation: &CancellationToken,
) -> Result<BestOfThreeSimulationReport, SimulatorError> {
    validate_best_of_three_request(request)?;
    require_fixed_order_adapter(data_dir)?;

    let allocations = weighted_match_allocations(
        request.matches,
        request.opponents.len(),
        &request.opponent_weights,
    );
    let plan_by_key = request
        .sideboard_plans
        .iter()
        .map(|plan| (plan.opponent_key.as_str(), plan))
        .collect::<HashMap<_, _>>();
    let mut matchups = Vec::with_capacity(request.opponents.len());
    let mut aggregate_preboard = BestOfThreeEvidence::default();
    let mut aggregate_postboard = BestOfThreeEvidence::default();
    let mut aggregate_matches = BestOfThreeEvidence::default();
    let mut cached_games = 0usize;
    let mut estimated_seconds = 0.0;
    let mut provenance_template = None;
    let mut child_configuration_hashes = Vec::new();

    for (opponent_index, opponent) in request.opponents.iter().enumerate() {
        if cancellation.is_cancelled() {
            return Err(SimulatorError::Cancelled);
        }
        let opponent_key = sideboard_opponent_key(opponent_index, opponent)?;
        let plan = plan_by_key[opponent_key.as_str()];
        let on_play = apply_sideboard_exchange(
            &request.candidate,
            plan.exchange(PlayDraw::Play),
            &request.pool,
            &request.constraints,
        )
        .map_err(|error| SimulatorError::InvalidSideboardPolicy(error.to_string()))?;
        let on_draw = apply_sideboard_exchange(
            &request.candidate,
            plan.exchange(PlayDraw::Draw),
            &request.pool,
            &request.constraints,
        )
        .map_err(|error| SimulatorError::InvalidSideboardPolicy(error.to_string()))?;

        let matches = allocations[opponent_index];
        let game_one_play = matches.div_ceil(2);
        let game_one_draw = matches / 2;
        let seed_base = request
            .seed
            .wrapping_add((opponent_index as i64).wrapping_mul(20_000_033));

        let preboard_on_play = run_best_of_three_batch(
            data_dir,
            request,
            &request.candidate,
            opponent,
            FixedOrderBranch {
                candidate_on_play: true,
                games: game_one_play,
                seed: seed_base,
            },
            cancellation,
        )?;
        let preboard_on_draw = run_best_of_three_batch(
            data_dir,
            request,
            &request.candidate,
            opponent,
            FixedOrderBranch {
                candidate_on_play: false,
                games: game_one_draw,
                seed: seed_base.wrapping_add(1_000_003),
            },
            cancellation,
        )?;

        // The loser of game one chooses to play in game two. These two
        // aggregate branches are sufficient to preserve exact match flow.
        let game_two_draw = run_best_of_three_batch(
            data_dir,
            request,
            &on_draw,
            opponent,
            FixedOrderBranch {
                candidate_on_play: false,
                games: preboard_on_play
                    .evidence
                    .wins
                    .saturating_add(preboard_on_draw.evidence.wins),
                seed: seed_base.wrapping_add(2_000_003),
            },
            cancellation,
        )?;
        let game_one_opponent_wins = preboard_on_play
            .opponent_wins
            .saturating_add(preboard_on_draw.opponent_wins);
        let game_two_play = run_best_of_three_batch(
            data_dir,
            request,
            &on_play,
            opponent,
            FixedOrderBranch {
                candidate_on_play: true,
                games: game_one_opponent_wins,
                seed: seed_base.wrapping_add(3_000_003),
            },
            cancellation,
        )?;

        // A third game is required only after a split. Whoever lost game two
        // is explicitly put on the play for game three.
        let game_three_play = run_best_of_three_batch(
            data_dir,
            request,
            &on_play,
            opponent,
            FixedOrderBranch {
                candidate_on_play: true,
                games: game_two_draw.opponent_wins,
                seed: seed_base.wrapping_add(4_000_003),
            },
            cancellation,
        )?;
        let game_three_draw = run_best_of_three_batch(
            data_dir,
            request,
            &on_draw,
            opponent,
            FixedOrderBranch {
                candidate_on_play: false,
                games: game_two_play.evidence.wins,
                seed: seed_base.wrapping_add(5_000_003),
            },
            cancellation,
        )?;

        let batches = [
            &preboard_on_play,
            &preboard_on_draw,
            &game_two_play,
            &game_two_draw,
            &game_three_play,
            &game_three_draw,
        ];
        for batch in batches {
            cached_games = cached_games.saturating_add(batch.cached_games);
            estimated_seconds += batch.estimated_seconds;
            if let Some(provenance) = batch.provenance.as_ref() {
                provenance_template.get_or_insert_with(|| provenance.clone());
                child_configuration_hashes.push(provenance.configuration_hash.clone());
            }
        }

        let mut preboard = BestOfThreeEvidence::default();
        merge_best_of_three_evidence(&mut preboard, &preboard_on_play.evidence);
        merge_best_of_three_evidence(&mut preboard, &preboard_on_draw.evidence);
        finalize_best_of_three_evidence(&mut preboard);
        let mut postboard_on_play = BestOfThreeEvidence::default();
        merge_best_of_three_evidence(&mut postboard_on_play, &game_two_play.evidence);
        merge_best_of_three_evidence(&mut postboard_on_play, &game_three_play.evidence);
        finalize_best_of_three_evidence(&mut postboard_on_play);
        let mut postboard_on_draw = BestOfThreeEvidence::default();
        merge_best_of_three_evidence(&mut postboard_on_draw, &game_two_draw.evidence);
        merge_best_of_three_evidence(&mut postboard_on_draw, &game_three_draw.evidence);
        finalize_best_of_three_evidence(&mut postboard_on_draw);
        let mut postboard = BestOfThreeEvidence::default();
        merge_best_of_three_evidence(&mut postboard, &postboard_on_play);
        merge_best_of_three_evidence(&mut postboard, &postboard_on_draw);
        finalize_best_of_three_evidence(&mut postboard);

        let match_wins = game_two_draw
            .evidence
            .wins
            .saturating_add(game_three_play.evidence.wins)
            .saturating_add(game_three_draw.evidence.wins);
        let match_losses = game_two_play
            .opponent_wins
            .saturating_add(game_three_play.opponent_wins)
            .saturating_add(game_three_draw.opponent_wins);
        let completed_matches = match_wins.saturating_add(match_losses);
        let game_crashes = preboard.crashes.saturating_add(postboard.crashes);
        let mut match_evidence = BestOfThreeEvidence {
            games: matches,
            wins: match_wins,
            draws: matches.saturating_sub(completed_matches),
            crashes: game_crashes,
            ..Default::default()
        };
        finalize_best_of_three_evidence(&mut match_evidence);

        merge_best_of_three_evidence(&mut aggregate_preboard, &preboard);
        merge_best_of_three_evidence(&mut aggregate_postboard, &postboard);
        merge_best_of_three_evidence(&mut aggregate_matches, &match_evidence);
        matchups.push(BestOfThreeMatchupReport {
            opponent_key,
            opponent_name: opponent.name.clone(),
            weight: matches as f64 / request.matches.max(1) as f64,
            plan: plan.clone(),
            preboard_on_play: preboard_on_play.evidence,
            preboard_on_draw: preboard_on_draw.evidence,
            preboard,
            postboard_on_play,
            postboard_on_draw,
            postboard,
            matches: match_evidence,
        });
    }

    finalize_best_of_three_evidence(&mut aggregate_preboard);
    finalize_best_of_three_evidence(&mut aggregate_postboard);
    finalize_best_of_three_evidence(&mut aggregate_matches);
    let mut provenance = provenance_template.ok_or_else(|| {
        SimulatorError::Process("Best-of-Three produced no simulator provenance".into())
    })?;
    provenance.configuration_hash = crate::provenance::configuration_hash(
        "hexdek-best-of-three-v1",
        &serde_json::json!({
            "request": request,
            "allocations": allocations,
            "initial_play_draw": "candidate_play_for_even_allocation_slot_then_alternating",
            "postboard_choice": "previous_game_loser_plays",
            "child_configuration_hashes": child_configuration_hashes,
        }),
    )?;
    let attempted_games = aggregate_preboard
        .games
        .saturating_add(aggregate_postboard.games);
    Ok(BestOfThreeSimulationReport {
        engine: "hexdek".into(),
        schema_version: "1.0.0".into(),
        provenance,
        requested_matches: request.matches,
        completed_matches: aggregate_matches
            .games
            .saturating_sub(aggregate_matches.draws),
        cached_games,
        games_per_second: attempted_games as f64 / estimated_seconds.max(f64::EPSILON),
        preboard: aggregate_preboard,
        postboard: aggregate_postboard,
        matches: aggregate_matches,
        matchups,
    })
}

fn run_best_of_three_batch(
    data_dir: &Path,
    request: &BestOfThreeValidationRequest,
    candidate: &Deck,
    opponent: &Deck,
    branch: FixedOrderBranch,
    cancellation: &CancellationToken,
) -> Result<BestOfThreeBatch, SimulatorError> {
    if branch.games == 0 {
        return Ok(BestOfThreeBatch::default());
    }
    let (first, second, candidate_index) = if branch.candidate_on_play {
        (candidate, opponent, 0usize)
    } else {
        (opponent, candidate, 1usize)
    };
    let identifiers = distinct_constructed_identifiers(&[first, second], &request.pool)?;
    let candidate_marker = identifiers[candidate_index].clone();
    let opponent_marker = identifiers[1 - candidate_index].clone();
    let child_request = HexdekValidationRequest {
        candidate: first.clone(),
        opponents: vec![second.clone()],
        opponent_weights: vec![1.0],
        pool: request.pool.clone(),
        games: branch.games,
        seed: branch.seed,
        workers: request.workers,
        seats: 2,
        hat_budget: request.hat_budget,
        max_turns: request.max_turns,
        game_timeout_seconds: request.game_timeout_seconds,
        commander: false,
        fixed_order: true,
    };
    let report = run_hexdek_cancellable(data_dir, &child_request, cancellation)?;
    let wins = report
        .rows
        .iter()
        .find(|row| row.deck == candidate_marker)
        .map(|row| row.wins)
        .unwrap_or(0);
    let opponent_wins = report
        .rows
        .iter()
        .find(|row| row.deck == opponent_marker)
        .map(|row| row.wins)
        .unwrap_or(0);
    let mut evidence = BestOfThreeEvidence {
        games: report.games,
        wins,
        draws: report.draws,
        crashes: report.crashes,
        ..Default::default()
    };
    finalize_best_of_three_evidence(&mut evidence);
    Ok(BestOfThreeBatch {
        evidence,
        opponent_wins,
        cached_games: report.cached_games,
        estimated_seconds: report.games as f64 / report.games_per_second.max(f64::EPSILON),
        provenance: report.provenance,
    })
}

fn require_fixed_order_adapter(data_dir: &Path) -> Result<(), SimulatorError> {
    let version_path = hexdek_paths(data_dir).root.join("VERSION");
    let version = fs::read_to_string(&version_path)
        .map_err(|_| SimulatorError::NotInstalled(version_path.display().to_string()))?;
    if version
        .lines()
        .any(|line| line.trim() == format!("adapter={HEXDEK_FIXED_ORDER_ADAPTER}"))
    {
        Ok(())
    } else {
        Err(SimulatorError::Process(
            "Best-of-Three requires the current fixed-order HexDek adapter; reinstall HexDek from Metagene"
                .into(),
        ))
    }
}

fn weighted_match_allocations(total: usize, count: usize, weights: &[f64]) -> Vec<usize> {
    if count == 0 {
        return Vec::new();
    }
    let usable = weights.len() == count
        && weights
            .iter()
            .all(|weight| weight.is_finite() && *weight > 0.0);
    let weights = if usable {
        weights.to_vec()
    } else {
        vec![1.0; count]
    };
    let weight_total = weights.iter().sum::<f64>();
    let distributable = total.saturating_sub(count);
    let raw = weights
        .iter()
        .map(|weight| distributable as f64 * *weight / weight_total)
        .collect::<Vec<_>>();
    let mut allocations = raw
        .iter()
        .map(|value| 1usize.saturating_add(value.floor() as usize))
        .collect::<Vec<_>>();
    let remaining = total.saturating_sub(allocations.iter().sum());
    let mut order = (0..count).collect::<Vec<_>>();
    order.sort_by(|left, right| {
        (raw[*right] - raw[*right].floor())
            .total_cmp(&(raw[*left] - raw[*left].floor()))
            .then_with(|| left.cmp(right))
    });
    for index in order.into_iter().cycle().take(remaining) {
        allocations[index] = allocations[index].saturating_add(1);
    }
    allocations
}

fn merge_best_of_three_evidence(target: &mut BestOfThreeEvidence, source: &BestOfThreeEvidence) {
    target.games = target.games.saturating_add(source.games);
    target.wins = target.wins.saturating_add(source.wins);
    target.draws = target.draws.saturating_add(source.draws);
    target.crashes = target.crashes.saturating_add(source.crashes);
}

fn finalize_best_of_three_evidence(evidence: &mut BestOfThreeEvidence) {
    evidence.win_rate = evidence.wins as f64 / evidence.games.max(1) as f64;
    let (lower, upper) = wilson_interval(evidence.wins, evidence.games);
    evidence.win_rate_lower_bound = lower;
    evidence.win_rate_upper_bound = upper;
}

pub fn run_successive_halving(
    data_dir: &Path,
    request: &SuccessiveHalvingRequest,
    on_progress: impl Fn(HalvingProgress) + Sync,
) -> Result<SuccessiveHalvingReport, SimulatorError> {
    run_successive_halving_cancellable(
        data_dir,
        request,
        &CancellationToken::default(),
        on_progress,
    )
}

pub fn run_successive_halving_cancellable(
    data_dir: &Path,
    request: &SuccessiveHalvingRequest,
    cancellation: &CancellationToken,
    on_progress: impl Fn(HalvingProgress) + Sync,
) -> Result<SuccessiveHalvingReport, SimulatorError> {
    let started = Instant::now();
    let total_rounds = request.rounds.clamp(1, 8);
    let structural_weight = request.structural_weight.clamp(0.0, 1.0);
    let risk_aversion = request.risk_aversion.clamp(0.0, 1.0);
    let worst_matchup_weight = request.worst_matchup_weight.clamp(0.0, 1.0);
    let seat_position_weight = request.seat_position_weight.clamp(0.0, 1.0);
    let mut active = request
        .candidates
        .iter()
        .take(64)
        .cloned()
        .collect::<Vec<_>>();
    active.sort_by(|left, right| right.fitness.score.total_cmp(&left.fitness.score));
    active.dedup_by_key(|candidate| candidate.rank);
    if active.is_empty() {
        return Err(SimulatorError::Process(
            "successive halving requires at least one finalist".into(),
        ));
    }
    let matchup_multiplier = if request.commander {
        1
    } else {
        request.opponents.len().max(1)
    };
    let maximum_game_budget = maximum_halving_game_budget(
        active.len(),
        request.base_games,
        total_rounds,
        matchup_multiplier,
        request.commander,
        request.seats,
    );

    let mut validation = HexdekValidationRequest {
        candidate: active[0].deck.clone(),
        opponents: request.opponents.clone(),
        opponent_weights: request.opponent_weights.clone(),
        pool: request.pool.clone(),
        games: complete_rotation_games(request.base_games, request.commander, request.seats),
        seed: request.seed,
        workers: request.workers,
        seats: request.seats,
        hat_budget: request.hat_budget,
        max_turns: request.max_turns,
        game_timeout_seconds: request.game_timeout_seconds,
        commander: request.commander,
        fixed_order: false,
    };
    let mut rounds = Vec::new();
    let mut completed_games = 0usize;
    let mut completed_attempts = 0usize;
    let mut scheduled_game_budget = 0usize;
    let mut total_crashes = 0usize;
    let mut cumulative: HashMap<usize, (usize, usize, usize)> = HashMap::new();
    let mut cumulative_matchups: HashMap<usize, HashMap<String, (usize, usize)>> = HashMap::new();
    let mut cumulative_positions: HashMap<usize, HashMap<usize, (usize, usize)>> = HashMap::new();
    let mut cancelled = false;

    for round_index in 0..total_rounds {
        if active.is_empty() || cancellation.is_cancelled() {
            cancelled = cancellation.is_cancelled();
            break;
        }
        let entrants = active.len();
        let games = complete_rotation_games(
            request
                .base_games
                .clamp(1, 10_000_000)
                .saturating_mul(1usize << round_index)
                .min(10_000_000),
            request.commander,
            request.seats,
        );
        scheduled_game_budget = scheduled_game_budget.saturating_add(
            entrants
                .saturating_mul(games)
                .saturating_mul(matchup_multiplier),
        );
        validation.games = games;
        validation.seed = request
            .seed
            .wrapping_add((round_index as i64).wrapping_mul(1_000_003));
        let process_count = entrants
            .min((request.workers.clamp(1, 256) / 4).max(1))
            .min(8);
        let workers_per_process = (request.workers.clamp(1, 256) / process_count).max(1);
        let process_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(process_count)
            .build()
            .map_err(|error| {
                SimulatorError::Process(format!("could not create simulator process pool: {error}"))
            })?;
        let finished_candidates = AtomicUsize::new(0);
        let round_completed_games = AtomicUsize::new(0);
        let round_completed_attempts = AtomicUsize::new(0);
        let raw_results = process_pool.install(|| {
            active
                .par_iter()
                .map(|candidate| {
                    let mut candidate_validation = validation.clone();
                    candidate_validation.candidate = candidate.deck.clone();
                    candidate_validation.workers = workers_per_process;
                    let commander = if request.commander {
                        primary_commander_name(&candidate.deck, &request.pool)
                            .unwrap_or_else(|_| format!("Candidate #{}", candidate.rank))
                    } else {
                        format!("Candidate · {}", candidate.deck.name)
                    };
                    let outcome = if let Some(route_plan) = request.route_plan.as_ref() {
                        crate::competitive::run_routed_validation_cancellable(
                            data_dir,
                            &candidate_validation,
                            route_plan,
                            cancellation,
                        )
                    } else {
                        run_validation_gauntlet_cancellable(
                            data_dir,
                            &candidate_validation,
                            cancellation,
                        )
                        .map_err(|error| error.to_string())
                    };
                    let (reported_games, reported_attempts) = outcome
                        .as_ref()
                        .map(|report| (report.games, report.games.saturating_add(report.crashes)))
                        .unwrap_or_default();
                    let games_finished = round_completed_games
                        .fetch_add(reported_games, AtomicOrdering::Relaxed)
                        .saturating_add(reported_games);
                    let attempts_finished = round_completed_attempts
                        .fetch_add(reported_attempts, AtomicOrdering::Relaxed)
                        .saturating_add(reported_attempts);
                    let completion_index = finished_candidates
                        .fetch_add(1, AtomicOrdering::Relaxed)
                        .saturating_add(1);
                    on_progress(HalvingProgress {
                        round: round_index + 1,
                        total_rounds,
                        candidate_index: completion_index,
                        candidates_in_round: entrants,
                        candidate_rank: candidate.rank,
                        games_per_candidate: games,
                        completed_games: completed_games.saturating_add(games_finished),
                        completed_attempts: completed_attempts.saturating_add(attempts_finished),
                    });
                    (candidate, commander, outcome)
                })
                .collect::<Vec<_>>()
        });
        let mut results = Vec::with_capacity(entrants);
        for (candidate, commander, outcome) in raw_results {
            let result = match outcome {
                Ok(report) => {
                    completed_games = completed_games.saturating_add(report.games);
                    completed_attempts = completed_attempts
                        .saturating_add(report.games)
                        .saturating_add(report.crashes);
                    total_crashes = total_crashes.saturating_add(report.crashes);
                    let row = report.rows.iter().find(|row| row.deck == commander);
                    let round_wins = row.map(|row| row.wins).unwrap_or(0);
                    let round_games = row.map(|row| row.games).unwrap_or(report.games);
                    let round_win_rate = round_wins as f64 / round_games.max(1) as f64;
                    let totals = cumulative.entry(candidate.rank).or_default();
                    totals.0 = totals.0.saturating_add(round_wins);
                    totals.1 = totals.1.saturating_add(round_games);
                    totals.2 = totals.2.saturating_add(report.crashes);
                    let win_rate = totals.0 as f64 / totals.1.max(1) as f64;
                    let (win_rate_lower_bound, win_rate_upper_bound) =
                        wilson_interval(totals.0, totals.1);
                    let matchup_totals = cumulative_matchups.entry(candidate.rank).or_default();
                    if let Some(opponent_wins) = report.matchup_wins.get(&commander) {
                        for (opponent, wins) in opponent_wins {
                            let matchup_games = report
                                .matchup_games
                                .get(&commander)
                                .and_then(|games| games.get(opponent))
                                .copied()
                                .unwrap_or(0);
                            if matchup_games == 0 {
                                continue;
                            }
                            let matchup = matchup_totals.entry(opponent.clone()).or_default();
                            matchup.0 = matchup.0.saturating_add(*wins);
                            matchup.1 = matchup.1.saturating_add(matchup_games);
                        }
                    }
                    let (
                        worst_matchup_win_rate,
                        worst_matchup_lower_bound,
                        worst_matchup_upper_bound,
                    ) = matchup_totals
                        .values()
                        .filter(|(_, games)| *games > 0)
                        .map(|(wins, games)| {
                            let (lower, upper) = wilson_interval(*wins, *games);
                            (*wins as f64 / *games as f64, lower, upper)
                        })
                        .fold(
                            (win_rate, win_rate_lower_bound, win_rate_upper_bound),
                            |worst, matchup| {
                                (
                                    worst.0.min(matchup.0),
                                    worst.1.min(matchup.1),
                                    worst.2.min(matchup.2),
                                )
                            },
                        );
                    let position_totals = cumulative_positions.entry(candidate.rank).or_default();
                    for position in report
                        .seat_order
                        .iter()
                        .filter(|position| position.deck == commander && position.games > 0)
                    {
                        let total = position_totals.entry(position.position).or_default();
                        total.0 = total.0.saturating_add(position.wins);
                        total.1 = total.1.saturating_add(position.games);
                    }
                    let position_summary = seat_position_summary(position_totals);
                    let crash_rate =
                        totals.2 as f64 / totals.1.saturating_add(totals.2).max(1) as f64;
                    let confidence_score = confidence_adjusted_score(
                        ConfidenceScoreInput {
                            win_rate,
                            win_rate_lower_bound,
                            worst_matchup_win_rate,
                            worst_matchup_lower_bound,
                            crash_rate,
                            seat_position: position_summary
                                .map(|summary| (summary.worst_rate, summary.worst_lower)),
                        },
                        ConfidencePolicy {
                            risk_aversion,
                            worst_matchup_weight,
                            seat_position_weight,
                        },
                    );
                    let confidence_lower_bound = evidence_bound(
                        win_rate_lower_bound,
                        worst_matchup_lower_bound,
                        crash_rate,
                        worst_matchup_weight,
                        position_summary.map(|summary| summary.worst_lower),
                        seat_position_weight,
                    );
                    let confidence_upper_bound = evidence_bound(
                        win_rate_upper_bound,
                        worst_matchup_upper_bound,
                        crash_rate,
                        worst_matchup_weight,
                        position_summary.map(|summary| summary.worst_upper),
                        seat_position_weight,
                    );
                    let commander_objective = request
                        .commander
                        .then_some(report.commander_evidence.as_ref())
                        .flatten()
                        .map(|evidence| evidence.objective_score);
                    let objective_weight = commander_objective
                        .map(|_| commander_objective_policy().ranking_weight.clamp(0.0, 1.0))
                        .unwrap_or_default();
                    let objective_score = commander_objective.unwrap_or(confidence_score);
                    let evidence_score = confidence_score * (1.0 - objective_weight)
                        + objective_score * objective_weight;
                    let evidence_lower_bound = confidence_lower_bound * (1.0 - objective_weight)
                        + objective_score * objective_weight;
                    let evidence_upper_bound = confidence_upper_bound * (1.0 - objective_weight)
                        + objective_score * objective_weight;
                    let blended_score = candidate.fitness.score * structural_weight
                        + evidence_score * (1.0 - structural_weight);
                    let blended_lower_bound = candidate.fitness.score * structural_weight
                        + evidence_lower_bound * (1.0 - structural_weight);
                    let blended_upper_bound = candidate.fitness.score * structural_weight
                        + evidence_upper_bound * (1.0 - structural_weight);
                    HalvingCandidateResult {
                        candidate_rank: candidate.rank,
                        commander: commander.clone(),
                        structural_score: candidate.fitness.score,
                        round_win_rate,
                        game_win_rate: win_rate,
                        cumulative_games: totals.1,
                        crash_rate,
                        win_rate_lower_bound,
                        win_rate_upper_bound,
                        worst_matchup_win_rate,
                        worst_matchup_lower_bound,
                        worst_matchup_upper_bound,
                        worst_position_win_rate: position_summary.map(|summary| summary.worst_rate),
                        worst_position_lower_bound: position_summary
                            .map(|summary| summary.worst_lower),
                        worst_position_upper_bound: position_summary
                            .map(|summary| summary.worst_upper),
                        position_win_rate_spread: position_summary.map(|summary| summary.spread),
                        evidence_score,
                        evidence_lower_bound,
                        evidence_upper_bound,
                        blended_score,
                        blended_lower_bound,
                        blended_upper_bound,
                        report: Some(report),
                        error: None,
                    }
                }
                Err(error) => HalvingCandidateResult {
                    candidate_rank: candidate.rank,
                    commander: commander.clone(),
                    structural_score: candidate.fitness.score,
                    round_win_rate: 0.0,
                    game_win_rate: 0.0,
                    cumulative_games: cumulative
                        .get(&candidate.rank)
                        .map(|totals| totals.1)
                        .unwrap_or(0),
                    crash_rate: 1.0,
                    win_rate_lower_bound: 0.0,
                    win_rate_upper_bound: 1.0,
                    worst_matchup_win_rate: 0.0,
                    worst_matchup_lower_bound: 0.0,
                    worst_matchup_upper_bound: 1.0,
                    worst_position_win_rate: None,
                    worst_position_lower_bound: None,
                    worst_position_upper_bound: None,
                    position_win_rate_spread: None,
                    evidence_score: 0.0,
                    evidence_lower_bound: 0.0,
                    evidence_upper_bound: 1.0,
                    blended_score: -1.0,
                    blended_lower_bound: -1.0,
                    blended_upper_bound: -1.0,
                    report: None,
                    error: Some(error),
                },
            };
            results.push(result);
        }

        if cancellation.is_cancelled() {
            cancelled = true;
            rounds.push(HalvingRound {
                round: round_index + 1,
                games_per_candidate: games,
                entrants,
                survivors: Vec::new(),
                statistically_eliminated: Vec::new(),
                results,
            });
            break;
        }

        let (survivor_ranks, statistically_eliminated) =
            select_halving_survivors(&mut results, entrants, request.adaptive_elimination);
        rounds.push(HalvingRound {
            round: round_index + 1,
            games_per_candidate: games,
            entrants,
            survivors: survivor_ranks.clone(),
            statistically_eliminated,
            results,
        });
        active.retain(|candidate| survivor_ranks.contains(&candidate.rank));
        active.sort_by_key(|candidate| {
            survivor_ranks
                .iter()
                .position(|rank| rank == &candidate.rank)
                .unwrap_or(usize::MAX)
        });
        if active.len() <= 1 {
            break;
        }
    }

    Ok(SuccessiveHalvingReport {
        engine: request.route_plan.as_ref().map_or_else(
            || "hexdek".into(),
            |plan| {
                let engines = plan
                    .routes
                    .iter()
                    .filter_map(|route| route.engine.as_deref())
                    .collect::<HashSet<_>>();
                if engines.len() > 1 {
                    "mixed-route".into()
                } else {
                    engines.into_iter().next().map_or_else(
                        || "unroutable".into(),
                        |engine| format!("{engine} · routed"),
                    )
                }
            },
        ),
        total_games: completed_games,
        total_attempts: completed_attempts,
        maximum_game_budget,
        scheduled_game_budget,
        avoided_game_budget: maximum_game_budget.saturating_sub(scheduled_game_budget),
        total_crashes,
        elapsed_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        winner_rank: if cancelled {
            None
        } else {
            active.first().map(|candidate| candidate.rank)
        },
        cancelled,
        rounds,
    })
}

fn select_halving_survivors(
    results: &mut [HalvingCandidateResult],
    entrants: usize,
    adaptive_elimination: bool,
) -> (Vec<usize>, Vec<usize>) {
    results.sort_by(|left, right| {
        right
            .blended_score
            .total_cmp(&left.blended_score)
            .then_with(|| left.candidate_rank.cmp(&right.candidate_rank))
    });
    let successful = results
        .iter()
        .filter(|result| result.error.is_none())
        .count();
    let survivor_cap = successful
        .min(entrants.div_ceil(2))
        .max(usize::from(successful > 0));
    let best_lower_bound = results
        .iter()
        .filter(|result| result.error.is_none())
        .map(|result| result.blended_lower_bound)
        .max_by(f64::total_cmp)
        .unwrap_or(f64::NEG_INFINITY);
    let statistically_eliminated = if adaptive_elimination {
        results
            .iter()
            .filter(|result| {
                result.error.is_none()
                    && result.blended_upper_bound + f64::EPSILON < best_lower_bound
            })
            .map(|result| result.candidate_rank)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let survivors = results
        .iter()
        .filter(|result| {
            result.error.is_none() && !statistically_eliminated.contains(&result.candidate_rank)
        })
        .take(survivor_cap)
        .map(|result| result.candidate_rank)
        .collect::<Vec<_>>();
    (survivors, statistically_eliminated)
}

fn run_hexdek_job(
    paths: &HexdekPaths,
    job_dir: &Path,
    request: &HexdekValidationRequest,
    decks: &[&Deck],
    identifiers: &[String],
    seats: usize,
    cancellation: &CancellationToken,
) -> Result<HexdekJobOutcome, SimulatorError> {
    let serialization_started = Instant::now();
    let mut deck_paths = Vec::with_capacity(decks.len());
    for (index, deck) in decks.iter().enumerate() {
        let path = job_dir.join(format!("deck-{index}.txt"));
        let exported = if request.commander {
            export_hexdek(deck, &request.pool)?
        } else {
            export_hexdek_constructed(deck, &request.pool, &identifiers[index])?
        };
        fs::write(&path, exported)?;
        deck_paths.push(path);
    }
    let result_path = job_dir.join("result.json");
    let report_path = job_dir.join("report.md");
    let decks_argument = deck_paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(",");
    let combo_decks = deck_paths
        .iter()
        .zip(decks.iter())
        .filter_map(|(path, deck)| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| (name, *deck))
        })
        .collect::<Vec<_>>();
    let combo_plan = crate::combo::hexdek_plan_for_decks(&combo_decks, &request.pool);
    let combo_plan_path = if combo_plan.decks.is_empty() {
        None
    } else {
        let path = job_dir.join("metagene-combo-plan.json");
        fs::write(&path, serde_json::to_vec(&combo_plan)?)?;
        Some(path)
    };
    let mut command = Command::new(&paths.executable);
    command.current_dir(job_dir).args([
        "--decks",
        &decks_argument,
        "--games",
        &request.games.clamp(1, 10_000_000).to_string(),
        "--seed",
        &request.seed.to_string(),
        "--seats",
        &seats.to_string(),
        "--workers",
        &request.workers.clamp(1, 256).to_string(),
        "--hat",
        "yggdrasil",
        "--hat-budget",
        &request.hat_budget.min(10_000).to_string(),
        "--max-turns",
        &request.max_turns.clamp(1, 1_000).to_string(),
        "--game-timeout",
        &request.game_timeout_seconds.clamp(1, 3_600).to_string(),
        if request.commander {
            "--commander=true"
        } else {
            "--commander=false"
        },
        if request.commander {
            "--compact-metrics=true"
        } else {
            "--compact-metrics=false"
        },
        if request.fixed_order {
            "--fixed-order=true"
        } else {
            "--fixed-order=false"
        },
        "--matchup",
        "--ast",
        &paths.ast.display().to_string(),
        "--oracle",
        &paths.oracle.display().to_string(),
        "--result-json",
        &result_path.display().to_string(),
        "--report",
        &report_path.display().to_string(),
    ]);
    if let Some(path) = &combo_plan_path {
        command.arg("--combo-plan").arg(path);
    }
    let stderr_path = job_dir.join("stderr.log");
    command
        .stdout(Stdio::null())
        .stderr(Stdio::from(fs::File::create(&stderr_path)?));
    let serialization_ms = elapsed_ms(serialization_started);
    let engine_started = Instant::now();
    let mut child = command.spawn()?;
    let status = loop {
        if cancellation.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(SimulatorError::Cancelled);
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if !status.success() {
        let stderr = fs::read_to_string(stderr_path).unwrap_or_default();
        return Err(SimulatorError::Process(
            stderr.chars().take(4_000).collect(),
        ));
    }
    let engine_ms = elapsed_ms(engine_started);
    let parse_started = Instant::now();
    let mut report = parse_hexdek_result(&fs::read_to_string(result_path)?)?;
    if let Some(candidate_plan) = combo_plan
        .decks
        .iter()
        .find(|plan| plan.deck == "deck-0.txt")
    {
        let evidence = report.combo_execution.get_or_insert_with(|| {
            ComboExecutionEvidence {
                limitations: vec![
                    "Plan telemetry records reached states and legal priorities; it is not proof that a combo resolved."
                        .into(),
                ],
                ..ComboExecutionEvidence::default()
            }
        });
        evidence.plan_available = true;
        evidence.proof_revision = candidate_plan.proof_revision.clone();
        evidence.planned_lines = candidate_plan.plans.len();
    }
    Ok(HexdekJobOutcome {
        report,
        serialization_ms,
        engine_ms,
        parse_ms: elapsed_ms(parse_started),
    })
}

struct HexdekJobOutcome {
    report: SimulationReport,
    serialization_ms: u64,
    engine_ms: u64,
    parse_ms: u64,
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

pub fn export_hexdek(deck: &Deck, pool: &CardPool) -> Result<String, SimulatorError> {
    let cards: HashMap<_, _> = pool.cards.iter().map(|card| (&card.id, card)).collect();
    let mut output = String::new();
    for entry in &deck.mainboard {
        let card = cards
            .get(&entry.card_id)
            .ok_or_else(|| SimulatorError::UnknownCard(entry.card_id.0.clone()))?;
        output.push_str(&format!("{} {}\n", entry.quantity, card.name));
    }
    for (index, entry) in deck.command_zone.iter().enumerate() {
        let card = cards
            .get(&entry.card_id)
            .ok_or_else(|| SimulatorError::UnknownCard(entry.card_id.0.clone()))?;
        output.push_str(if index == 0 {
            "COMMANDER: "
        } else {
            "PARTNER: "
        });
        output.push_str(&card.name);
        output.push('\n');
    }
    Ok(output)
}

/// HexDek currently identifies seats through its Commander-shaped result schema
/// even when Commander rules are disabled. Give each constructed deck a distinct
/// card name so its row remains unambiguous.
fn distinct_constructed_identifiers(
    decks: &[&Deck],
    pool: &CardPool,
) -> Result<Vec<String>, SimulatorError> {
    let cards: HashMap<_, _> = pool.cards.iter().map(|card| (&card.id, card)).collect();
    let mut used = HashSet::new();
    let mut identifiers = Vec::with_capacity(decks.len());
    for deck in decks {
        let identifier = deck
            .mainboard
            .iter()
            .filter(|entry| entry.quantity > 0)
            .filter_map(|entry| cards.get(&entry.card_id))
            .map(|card| card.name.clone())
            .find(|name| used.insert(name.clone()))
            .ok_or(SimulatorError::NoDistinctIdentifiers)?;
        identifiers.push(identifier);
    }
    Ok(identifiers)
}

/// Export an exact constructed library through HexDek's Commander-shaped deck
/// loader. The loader removes one copy of the labeled card even with
/// `--commander=false`, so the file deliberately contains one extra copy.
pub fn export_hexdek_constructed(
    deck: &Deck,
    pool: &CardPool,
    identifier: &str,
) -> Result<String, SimulatorError> {
    let cards: HashMap<_, _> = pool.cards.iter().map(|card| (&card.id, card)).collect();
    let mut output = String::new();
    let mut found = false;
    for entry in &deck.mainboard {
        let card = cards
            .get(&entry.card_id)
            .ok_or_else(|| SimulatorError::UnknownCard(entry.card_id.0.clone()))?;
        let extra = usize::from(!found && card.name == identifier);
        found |= extra == 1;
        output.push_str(&format!(
            "{} {}\n",
            entry.quantity as usize + extra,
            card.name
        ));
    }
    if !found {
        return Err(SimulatorError::NoDistinctIdentifiers);
    }
    output.push_str("COMMANDER: ");
    output.push_str(identifier);
    output.push('\n');
    Ok(output)
}

fn primary_commander_name(deck: &Deck, pool: &CardPool) -> Result<String, SimulatorError> {
    let Some(entry) = deck.command_zone.first() else {
        return Ok(String::new());
    };
    pool.cards
        .iter()
        .find(|card| card.id == entry.card_id)
        .map(|card| card.name.clone())
        .ok_or_else(|| SimulatorError::UnknownCard(entry.card_id.0.clone()))
}

fn commander_metric(value: f64, source: &str) -> CommanderMetric {
    CommanderMetric {
        value: value.clamp(0.0, 1.0),
        source: source.into(),
    }
}

fn score_commander_evidence(evidence: &mut CommanderEvidence) {
    let policy = commander_objective_policy();
    let objectives = &policy.objectives;
    let reward = evidence.pod_win_share.value * objectives.pod_win_share
        + evidence.seat_floor.value * objectives.seat_floor
        + evidence.threat_development.value * objectives.threat_development
        + evidence.interaction.value * objectives.interaction
        + evidence.recovery.value * objectives.recovery
        + evidence.mana_reliability.value * objectives.mana_reliability
        + evidence.closing_ability.value * objectives.closing_ability;
    let penalties = &policy.penalties;
    evidence.penalties.total_penalty = (evidence.penalties.draw_or_stall_rate
        * penalties.draw_or_stall_rate
        + evidence.penalties.timeout_or_crash_rate * penalties.timeout_or_crash_rate
        + evidence.penalties.concession_rate * penalties.concession_rate
        + evidence.penalties.seat_bias * penalties.seat_bias
        + evidence.penalties.kingmaking_signal.unwrap_or_default() * penalties.kingmaking_signal)
        .clamp(0.0, 1.0);
    evidence.objective_score = (reward - evidence.penalties.total_penalty).clamp(0.0, 1.0);
}

fn native_commander_evidence(
    result: &HexdekResult,
    rows: &[MatchupRow],
    seat_order: &[SeatOrderRow],
) -> Option<CommanderEvidence> {
    let candidate = result.commander_names.first()?;
    let candidate_row = rows.iter().find(|row| &row.deck == candidate)?;
    let pod_size = result.n_seats.max(result.commander_names.len()).max(2);
    let seat_performance = seat_order
        .iter()
        .filter(|row| &row.deck == candidate)
        .map(|row| CommanderSeatEvidence {
            position: row.position,
            games: row.games,
            wins: row.wins,
            win_share: row.win_rate,
        })
        .collect::<Vec<_>>();
    let seat_floor = seat_performance
        .iter()
        .map(|seat| seat.win_share)
        .reduce(f64::min)
        .unwrap_or(candidate_row.win_rate);
    let global_seat_rates = result
        .wins_by_seat
        .iter()
        .map(|wins| *wins as f64 / result.games.max(1) as f64)
        .collect::<Vec<_>>();
    let seat_bias = global_seat_rates
        .iter()
        .copied()
        .reduce(f64::max)
        .zip(global_seat_rates.iter().copied().reduce(f64::min))
        .map(|(maximum, minimum)| maximum - minimum)
        .unwrap_or_default();
    let attempted = result.games.saturating_add(result.crashes).max(1) as f64;
    let policy = commander_objective_policy();
    let closing = candidate_row.average_turn_to_win.map_or(0.0, |turn| {
        candidate_row.win_rate / (1.0 + turn.max(0.0) / policy.closing_turn_scale.max(1.0))
    });
    let native = result.commander_metrics.get(candidate);
    let (threat, interaction, recovery, mana_reliability, target_actions, kingmaking_signal) =
        native.map_or_else(
            || {
                (
                    commander_metric(0.0, "unavailable"),
                    commander_metric(0.0, "unavailable"),
                    commander_metric(0.0, "unavailable"),
                    commander_metric(0.0, "unavailable"),
                    HashMap::new(),
                    None,
                )
            },
            |metrics| {
                let games = metrics.games.max(1) as f64;
                let development_load = (metrics.damage_dealt as f64 / games / 20.0
                    + metrics.peak_board_size as f64 / games / 6.0
                    + metrics.spells_cast as f64 / games / 8.0)
                    / 3.0;
                let interaction_load = (metrics.counters_cast
                    + metrics.removal_cast
                    + metrics.creatures_killed) as f64
                    / games
                    / 3.0;
                let recovered_life = (metrics.final_life as f64 + metrics.damage_taken as f64
                    - 40.0 * metrics.games as f64)
                    .max(0.0)
                    / games;
                let creature_recovery = metrics.creatures_killed as f64
                    / (metrics.creatures_killed + metrics.creatures_lost).max(1) as f64;
                let recovery = metrics.survived as f64 / games * 0.5
                    + (recovered_life / 10.0).clamp(0.0, 1.0) * 0.3
                    + creature_recovery * 0.2;
                let mana_total = (metrics.mana_spent + metrics.mana_wasted).max(1) as f64;
                let mana = metrics.mana_spent as f64 / mana_total * 0.7
                    + (metrics.lands_played as f64 / games / 6.0).clamp(0.0, 1.0) * 0.3;
                (
                    commander_metric(1.0 - (-development_load).exp(), "rules_engine_compact"),
                    commander_metric(1.0 - (-interaction_load).exp(), "rules_engine_compact"),
                    commander_metric(recovery, "rules_engine_compact"),
                    commander_metric(mana, "rules_engine_compact"),
                    metrics.target_actions.clone(),
                    Some(metrics.kingmaking_signals as f64 / games),
                )
            },
        );
    let mut evidence = CommanderEvidence {
        schema_version: policy.schema_version,
        policy_id: policy.policy_id.clone(),
        evidence_mode: if pod_size > 2 {
            "multiplayer_pod"
        } else {
            "commander_duel"
        }
        .into(),
        pod_size,
        seed: 0,
        rotation_policy: if result.mode.trim().is_empty() {
            "engine-reported-unknown".into()
        } else {
            result.mode.clone()
        },
        complete_rotations: result.games / pod_size,
        partial_rotation_games: result.games % pod_size,
        pod_archetypes: result.commander_names.clone(),
        pod_win_share: commander_metric(candidate_row.win_rate, "rules_engine"),
        seat_floor: commander_metric(seat_floor, "rules_engine"),
        threat_development: threat,
        interaction,
        recovery,
        mana_reliability,
        closing_ability: commander_metric(closing, "rules_engine"),
        seat_performance,
        elimination_slots: result
            .elimination_by_commander_by_slot
            .get(candidate)
            .cloned()
            .unwrap_or_default(),
        target_actions,
        penalties: CommanderPenaltyEvidence {
            draw_or_stall_rate: result.draws as f64 / attempted,
            timeout_or_crash_rate: result.crashes as f64 / attempted,
            concession_rate: result.total_concessions as f64 / attempted,
            seat_bias,
            kingmaking_signal,
            total_penalty: 0.0,
        },
        objective_score: 0.0,
        limitations: policy.notes.clone(),
    };
    score_commander_evidence(&mut evidence);
    Some(evidence)
}

fn enrich_commander_evidence(
    report: &mut SimulationReport,
    request: &HexdekValidationRequest,
    identifiers: &[String],
) {
    if !request.commander {
        report.commander_evidence = None;
        return;
    }
    let Some(evidence) = report.commander_evidence.as_mut() else {
        return;
    };
    evidence.seed = request.seed;
    evidence.rotation_policy = if request.fixed_order {
        "fixed-deck-order-seat-zero-start-v1"
    } else {
        "cyclic-deck-order-seeded-start-v1"
    }
    .into();
    evidence.pod_archetypes = std::iter::once(request.candidate.name.clone())
        .chain(request.opponents.iter().map(|deck| deck.name.clone()))
        .take(identifiers.len())
        .collect();
    let policy = commander_objective_policy();
    if evidence.threat_development.source != "unavailable" {
        score_commander_evidence(evidence);
        return;
    }
    match evaluate_structural_deck(
        &request.candidate,
        &request.pool,
        request.seed as u64,
        policy.structural_trials,
        request.max_turns.min(u8::MAX as usize) as u8,
    ) {
        Ok(fitness) => {
            evidence.threat_development = commander_metric(fitness.tempo, "structural_proxy");
            evidence.interaction = commander_metric(fitness.interaction, "structural_proxy");
            evidence.recovery = commander_metric(fitness.resilience, "structural_proxy");
            evidence.mana_reliability =
                commander_metric(1.0 - fitness.mana_failure_rate, "structural_proxy");
        }
        Err(error) => evidence
            .limitations
            .push(format!("Structural proxy evaluation failed: {error}")),
    }
    score_commander_evidence(evidence);
}

fn parse_hexdek_result(json: &str) -> Result<SimulationReport, SimulatorError> {
    let result: HexdekResult = serde_json::from_str(json)?;
    if result.schema_version != "1.0.0" {
        return Err(SimulatorError::UnsupportedSchema(result.schema_version));
    }
    let mut rows = Vec::with_capacity(result.commander_names.len());
    for name in &result.commander_names {
        let games = result
            .games_played_by_commander
            .as_ref()
            .and_then(|counts| counts.get(name).copied())
            .unwrap_or(result.games);
        let wins = result.wins_by_commander.get(name).copied().unwrap_or(0);
        let (win_rate_lower_bound, win_rate_upper_bound) = wilson_interval(wins, games);
        rows.push(MatchupRow {
            deck: name.clone(),
            games,
            wins,
            win_rate: wins as f64 / games.max(1) as f64,
            win_rate_lower_bound,
            win_rate_upper_bound,
            average_turn_to_win: result
                .avg_turn_to_win
                .as_ref()
                .and_then(|turns| turns.get(name).copied()),
        });
    }
    let raw_matchups = result.matchup_matrix.clone().unwrap_or_default();
    let matchup_games = result.matchup_games.clone().unwrap_or_default();
    let mut seat_order = Vec::new();
    for name in &result.commander_names {
        let wins = result
            .wins_by_commander_by_seat
            .as_ref()
            .and_then(|values| values.get(name));
        let games = result
            .games_by_commander_by_seat
            .as_ref()
            .and_then(|values| values.get(name));
        let positions = wins
            .map(Vec::len)
            .unwrap_or_default()
            .max(games.map(Vec::len).unwrap_or_default());
        for index in 0..positions {
            let games = games
                .and_then(|values| values.get(index))
                .copied()
                .unwrap_or(0);
            if games == 0 {
                continue;
            }
            let wins = wins
                .and_then(|values| values.get(index))
                .copied()
                .unwrap_or(0);
            seat_order.push(SeatOrderRow {
                matchup: "Pod".into(),
                deck: name.clone(),
                position: index + 1,
                games,
                wins,
                win_rate: wins as f64 / games as f64,
            });
        }
    }
    let matchup_matrix = raw_matchups
        .iter()
        .map(|(left, opponents)| {
            let rates = opponents
                .iter()
                .map(|(right, wins)| {
                    let games = matchup_games
                        .get(left)
                        .and_then(|values| values.get(right))
                        .copied()
                        .unwrap_or(0);
                    (right.clone(), *wins as f64 / games.max(1) as f64)
                })
                .collect();
            (left.clone(), rates)
        })
        .collect();
    let commander_evidence = native_commander_evidence(&result, &rows, &seat_order);
    let combo_execution = result
        .commander_names
        .first()
        .and_then(|candidate| result.commander_metrics.get(candidate))
        .and_then(|metrics| {
            let observed = metrics.combo_assembling_games > 0
                || metrics.combo_executable_games > 0
                || metrics.combo_priority_actions > 0
                || metrics.combo_plan_wins > 0;
            observed.then(|| {
                let games = metrics.games.max(result.games).max(1);
                ComboExecutionEvidence {
                    assembling_games: metrics.combo_assembling_games,
                    executable_games: metrics.combo_executable_games,
                    priority_actions: metrics.combo_priority_actions,
                    plan_wins: metrics.combo_plan_wins,
                    assembling_rate: metrics.combo_assembling_games as f64 / games as f64,
                    executable_rate: metrics.combo_executable_games as f64 / games as f64,
                    win_conversion_rate: metrics.combo_plan_wins as f64
                        / metrics.combo_executable_games.max(1) as f64,
                    limitations: vec![
                        "Plan telemetry records reached states and legal priorities; it is not proof that a combo resolved."
                            .into(),
                    ],
                    ..ComboExecutionEvidence::default()
                }
            })
        });
    Ok(SimulationReport {
        engine: "hexdek".into(),
        schema_version: result.schema_version,
        engine_routes: HashMap::new(),
        route_failures: Vec::new(),
        provenance: None,
        games: result.games,
        draws: result.draws,
        crashes: result.crashes,
        cached_games: 0,
        seat_order,
        average_turns: result.avg_turns,
        games_per_second: result.games_per_second,
        rows,
        matchup_matrix,
        matchup_wins: raw_matchups,
        matchup_games,
        matchup_weights: HashMap::new(),
        commander_evidence,
        combo_execution,
        performance: None,
    })
}

struct HexdekPaths {
    root: PathBuf,
    executable: PathBuf,
    conformance: PathBuf,
    ast: PathBuf,
    oracle: PathBuf,
}

fn hexdek_paths(data_dir: &Path) -> HexdekPaths {
    let root = data_dir.join("engines").join("hexdek");
    HexdekPaths {
        executable: root.join(if cfg!(windows) {
            "hexdek-tournament.exe"
        } else {
            "hexdek-tournament"
        }),
        conformance: root.join(if cfg!(windows) {
            "hexdek-conformance.exe"
        } else {
            "hexdek-conformance"
        }),
        ast: root.join("ast_dataset.jsonl"),
        oracle: root.join("oracle-cards.json"),
        root,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metagene_core::{Card, CardFeatures, CardId, DeckEntry};
    use std::collections::BTreeMap;

    struct TestDirectory(PathBuf);

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn pool() -> CardPool {
        CardPool {
            cards: vec![
                Card {
                    id: CardId("leader".into()),
                    arena_id: None,
                    platforms: vec!["paper".into()],
                    name: "Leader".into(),
                    set_code: "tst".into(),
                    collector_number: "1".into(),
                    type_line: "Legendary Creature".into(),
                    oracle_text: String::new(),
                    colors: vec![],
                    color_identity: vec![],
                    image_uri: None,
                    legalities: BTreeMap::new(),
                    features: CardFeatures::default(),
                },
                Card {
                    id: CardId("spell".into()),
                    name: "Spell".into(),
                    ..Card {
                        id: CardId("leader".into()),
                        arena_id: None,
                        platforms: vec!["paper".into()],
                        name: "Leader".into(),
                        set_code: "tst".into(),
                        collector_number: "1".into(),
                        type_line: "Legendary Creature".into(),
                        oracle_text: String::new(),
                        colors: vec![],
                        color_identity: vec![],
                        image_uri: None,
                        legalities: BTreeMap::new(),
                        features: CardFeatures::default(),
                    }
                },
            ],
        }
    }

    #[test]
    #[ignore = "release smoke test extracts the 289 MiB bundled Windows runtime"]
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    fn bundled_hexdek_installs_without_a_compiler() {
        let root = std::env::temp_dir().join(format!("metagene-bundled-hexdek-{}", Uuid::new_v4()));
        let old_engine = root.join("engines/hexdek");
        fs::create_dir_all(&old_engine).unwrap();
        for file in [
            "hexdek-tournament.exe",
            "hexdek-conformance.exe",
            "ast_dataset.jsonl",
            "oracle-cards.json",
        ] {
            fs::write(old_engine.join(file), b"old runtime").unwrap();
        }
        fs::write(
            old_engine.join("VERSION"),
            b"revision=old\nadapter=metagene-fixed-order-commander-metrics-v2\n",
        )
        .unwrap();
        let old_status = statuses(&root)
            .into_iter()
            .find(|status| status.id == "hexdek")
            .unwrap();
        assert!(old_status.installed && !old_status.data_ready);
        let _cleanup = TestDirectory(root.clone());
        let mut details = Vec::new();
        let status = install_hexdek(&root, |progress| details.push(progress.detail)).unwrap();
        assert!(status.installed && status.data_ready);
        assert!(
            details
                .iter()
                .any(|detail| detail.contains("no compiler required"))
        );
        assert!(root.join("engines/hexdek/hexdek-tournament.exe").is_file());
        assert!(root.join("engines/hexdek/hexdek-conformance.exe").is_file());
        assert!(root.join("engines/hexdek/coverage-index.json").is_file());
        assert!(hexdek_adapter_is_current(&root.join("engines/hexdek")));
    }

    fn constructed_deck(name: &str) -> Deck {
        Deck {
            name: name.into(),
            command_zone: vec![],
            mainboard: vec![DeckEntry {
                card_id: CardId("spell".into()),
                quantity: 4,
            }],
            sideboard: vec![DeckEntry {
                card_id: CardId("leader".into()),
                quantity: 4,
            }],
        }
    }

    #[test]
    fn best_of_three_request_requires_a_current_plan_for_every_opponent() {
        let candidate = constructed_deck("Candidate");
        let opponents = vec![constructed_deck("Aggro"), constructed_deck("Control")];
        let plans = opponents
            .iter()
            .enumerate()
            .map(|(index, opponent)| MatchupSideboardPlan {
                opponent_key: sideboard_opponent_key(index, opponent).unwrap(),
                on_play: Default::default(),
                on_draw: Default::default(),
            })
            .collect();
        let mut request = BestOfThreeValidationRequest {
            candidate,
            opponents,
            opponent_weights: vec![0.6, 0.4],
            pool: pool(),
            constraints: DeckConstraints {
                target_mainboard_size: 4,
                target_sideboard_size: 4,
                minimum_commanders: 0,
                maximum_commanders: 0,
                default_max_copies: 4,
                locked_cards: vec![],
                minimum_lands: None,
                maximum_lands: None,
                allowed_colors: None,
            },
            sideboard_plans: plans,
            matches: 10,
            seed: 7,
            workers: 2,
            hat_budget: 0,
            max_turns: 40,
            game_timeout_seconds: 5,
        };
        validate_best_of_three_request(&request).unwrap();

        request.opponents[0].name = "Changed after planning".into();
        assert!(matches!(
            validate_best_of_three_request(&request),
            Err(SimulatorError::InvalidSideboardPolicy(message))
                if message.contains("do not match the current gauntlet")
        ));
    }

    #[test]
    fn best_of_three_request_rejects_an_illegal_opponent() {
        let candidate = constructed_deck("Candidate");
        let mut opponent = constructed_deck("Illegal opponent");
        let key = sideboard_opponent_key(0, &opponent).unwrap();
        opponent.mainboard[0].quantity = 5;
        let request = BestOfThreeValidationRequest {
            candidate,
            opponents: vec![opponent],
            opponent_weights: vec![1.0],
            pool: pool(),
            constraints: DeckConstraints {
                target_mainboard_size: 4,
                target_sideboard_size: 4,
                minimum_commanders: 0,
                maximum_commanders: 0,
                default_max_copies: 4,
                locked_cards: vec![],
                minimum_lands: None,
                maximum_lands: None,
                allowed_colors: None,
            },
            sideboard_plans: vec![MatchupSideboardPlan {
                opponent_key: key,
                on_play: Default::default(),
                on_draw: Default::default(),
            }],
            matches: 1,
            seed: 7,
            workers: 1,
            hat_budget: 0,
            max_turns: 40,
            game_timeout_seconds: 5,
        };
        assert!(matches!(
            validate_best_of_three_request(&request),
            Err(SimulatorError::InvalidSideboardPolicy(message))
                if message.contains("Illegal opponent") && message.contains("maximum is 4")
        ));
    }

    #[test]
    fn best_of_three_request_rejects_a_missing_matchup_plan() {
        let request = BestOfThreeValidationRequest {
            candidate: constructed_deck("Candidate"),
            opponents: vec![constructed_deck("Aggro")],
            opponent_weights: vec![1.0],
            pool: pool(),
            constraints: DeckConstraints {
                target_mainboard_size: 4,
                target_sideboard_size: 4,
                minimum_commanders: 0,
                maximum_commanders: 0,
                default_max_copies: 4,
                locked_cards: vec![],
                minimum_lands: None,
                maximum_lands: None,
                allowed_colors: None,
            },
            sideboard_plans: vec![],
            matches: 10,
            seed: 7,
            workers: 2,
            hat_budget: 0,
            max_turns: 40,
            game_timeout_seconds: 5,
        };
        assert!(matches!(
            validate_best_of_three_request(&request),
            Err(SimulatorError::InvalidSideboardPolicy(message))
                if message.contains("0 plans for 1 opponents")
        ));
    }

    #[test]
    fn best_of_three_match_allocations_are_exact_and_cover_every_opponent() {
        let allocations = weighted_match_allocations(10, 3, &[0.6, 0.3, 0.1]);
        assert_eq!(allocations, vec![5, 3, 2]);
        assert_eq!(allocations.iter().sum::<usize>(), 10);
        assert!(allocations.into_iter().all(|matches| matches >= 1));
    }

    #[test]
    fn writes_hexdek_commander_footer_format() {
        let deck = Deck {
            name: "Test".into(),
            command_zone: vec![DeckEntry {
                card_id: CardId("leader".into()),
                quantity: 1,
            }],
            mainboard: vec![DeckEntry {
                card_id: CardId("spell".into()),
                quantity: 2,
            }],
            sideboard: vec![],
        };
        assert_eq!(
            export_hexdek(&deck, &pool()).unwrap(),
            "2 Spell\nCOMMANDER: Leader\n"
        );
    }

    #[test]
    fn coverage_inputs_include_every_deck_zone() {
        let deck = Deck {
            name: "All zones".into(),
            command_zone: vec![DeckEntry {
                card_id: CardId("leader".into()),
                quantity: 1,
            }],
            mainboard: vec![DeckEntry {
                card_id: CardId("spell".into()),
                quantity: 2,
            }],
            sideboard: vec![DeckEntry {
                card_id: CardId("leader".into()),
                quantity: 3,
            }],
        };
        let inputs = coverage_inputs_from_decks(&[&deck], &pool()).unwrap();

        assert_eq!(inputs[0].cards.len(), 3);
        assert_eq!(inputs[0].cards[0].zone, "command");
        assert_eq!(inputs[0].cards[1].zone, "mainboard");
        assert_eq!(inputs[0].cards[2].zone, "sideboard");
        assert_eq!(inputs[0].cards[2].quantity, 3);
    }

    #[test]
    fn metagame_weights_allocate_an_exact_fixed_game_budget() {
        assert_eq!(
            weighted_game_allocations(100, 3, &[0.6, 0.3, 0.1]),
            vec![180, 90, 30]
        );
        assert_eq!(
            weighted_game_allocations(100, 3, &[1.0]),
            vec![100, 100, 100]
        );
        let rounded = weighted_game_allocations(17, 3, &[2.0, 1.0, 1.0]);
        assert_eq!(rounded.iter().sum::<usize>(), 51);
        assert!(rounded.iter().all(|games| *games >= 1));
        assert!(rounded[0] > rounded[1] && rounded[1] == rounded[2]);
    }

    #[test]
    fn constructed_export_restores_the_exact_library_after_loader_removal() {
        let deck = Deck {
            name: "Constructed".into(),
            command_zone: vec![],
            mainboard: vec![
                DeckEntry {
                    card_id: CardId("spell".into()),
                    quantity: 4,
                },
                DeckEntry {
                    card_id: CardId("leader".into()),
                    quantity: 56,
                },
            ],
            sideboard: vec![],
        };
        assert_eq!(
            export_hexdek_constructed(&deck, &pool(), "Spell").unwrap(),
            "5 Spell\n56 Leader\nCOMMANDER: Spell\n"
        );
    }

    #[test]
    fn constructed_seats_receive_distinct_result_identifiers() {
        let first = Deck {
            name: "First".into(),
            command_zone: vec![],
            mainboard: vec![DeckEntry {
                card_id: CardId("leader".into()),
                quantity: 60,
            }],
            sideboard: vec![],
        };
        let second = Deck {
            name: "Second".into(),
            command_zone: vec![],
            mainboard: vec![
                DeckEntry {
                    card_id: CardId("leader".into()),
                    quantity: 56,
                },
                DeckEntry {
                    card_id: CardId("spell".into()),
                    quantity: 4,
                },
            ],
            sideboard: vec![],
        };
        assert_eq!(
            distinct_constructed_identifiers(&[&first, &second], &pool()).unwrap(),
            vec!["Leader", "Spell"]
        );
    }

    #[test]
    fn coverage_audit_distinguishes_complete_partial_and_missing_cards() {
        let data_dir = std::env::temp_dir().join(format!("metagene-coverage-{}", Uuid::new_v4()));
        let engine_root = data_dir.join("engines").join("hexdek");
        fs::create_dir_all(&engine_root).unwrap();
        fs::write(
            engine_root.join("ast_dataset.jsonl"),
            concat!(
                "{\"__ast_type__\":\"DatasetManifest\",\"entry_count\":2,\"parser_git_sha\":\"test-revision\"}\n",
                "{\"name\":\"Complete\",\"ast\":{\"fully_parsed\":true,\"parse_errors\":[]}}\n",
                "{\"name\":\"Partial\",\"ast\":{\"fully_parsed\":false,\"parse_errors\":[\"unsupported action\"]}}\n"
            ),
        )
        .unwrap();
        let report = audit_hexdek_coverage(
            &data_dir,
            &[CoverageDeckInput {
                name: "Audit".into(),
                cards: vec![
                    CoverageCardInput {
                        name: "Complete".into(),
                        quantity: 4,
                        zone: "mainboard".into(),
                    },
                    CoverageCardInput {
                        name: "Partial".into(),
                        quantity: 2,
                        zone: "mainboard".into(),
                    },
                    CoverageCardInput {
                        name: "Absent".into(),
                        quantity: 1,
                        zone: "command".into(),
                    },
                ],
            }],
        )
        .unwrap();
        let deck = &report.decks[0];
        assert_eq!(report.parser_revision.as_deref(), Some("test-revision"));
        assert_eq!(deck.parser_complete_unique, 1);
        assert_eq!(deck.partial_unique, 1);
        assert_eq!(deck.missing_unique, 1);
        assert!((deck.present_ratio - (6.0 / 7.0)).abs() < f64::EPSILON);
        assert_eq!(deck.issues[0].status, "missing");
        assert_eq!(deck.issues[1].parse_errors, vec!["unsupported action"]);
        let test_pool = CardPool {
            cards: vec![
                Card {
                    id: CardId("complete".into()),
                    name: "Complete".into(),
                    ..pool().cards[0].clone()
                },
                Card {
                    id: CardId("partial".into()),
                    name: "Partial".into(),
                    ..pool().cards[0].clone()
                },
                Card {
                    id: CardId("absent".into()),
                    name: "Absent".into(),
                    ..pool().cards[0].clone()
                },
            ],
        };
        assert_eq!(
            filter_pool_for_hexdek(&data_dir, &test_pool, "present")
                .unwrap()
                .cards
                .len(),
            2
        );
        assert_eq!(
            filter_pool_for_hexdek(&data_dir, &test_pool, "parser_complete")
                .unwrap()
                .cards
                .len(),
            1
        );
        assert_eq!(
            filter_pool_for_hexdek(&data_dir, &test_pool, "all")
                .unwrap()
                .cards
                .len(),
            3
        );
        invalidate_coverage_cache(&engine_root);
        fs::remove_dir_all(data_dir).unwrap();
    }

    #[test]
    fn semantic_risk_detection_does_not_claim_runtime_conformance() {
        let replacement = semantic_risks(
            "If an effect would create one or more tokens, it creates twice that many instead.",
        );
        assert!(replacement.iter().any(|risk| risk.contains("Replacement")));

        let copy = semantic_risks("Create a token that's a copy of target permanent.");
        assert!(copy.iter().any(|risk| risk.contains("Copy semantics")));

        let arena = semantic_risks("Conjure a card from this card's spellbook into your hand.");
        assert!(arena.iter().any(|risk| risk.contains("Arena digital")));

        let control = semantic_risks("You control enchanted creature.");
        assert!(control.iter().any(|risk| risk.contains("Control-changing")));

        assert!(semantic_risks("Draw a card.").is_empty());
    }

    #[test]
    fn parses_canonical_hexdek_result() {
        let report = parse_hexdek_result(
            r#"{"schema_version":"1.0.0","mode":"rotate","games":100,"crashes":1,"draws":3,"avg_turns":9.5,"commander_names":["A","B"],"wins_by_commander":{"A":55,"B":42},"games_played_by_commander":{"A":100,"B":100},"wins_by_commander_by_seat":{"A":[30,25],"B":[20,22]},"games_by_commander_by_seat":{"A":[50,50],"B":[50,50]},"wins_by_seat":[50,47],"elimination_by_commander_by_slot":{"A":[45,55]},"total_concessions":2,"n_seats":2,"avg_turn_to_win":{"A":8.0},"matchup_matrix":{"A":{"B":55}},"matchup_games":{"A":{"B":100}},"commander_metrics":{"A":{"games":100,"lands_played":600,"mana_spent":900,"mana_wasted":100,"damage_dealt":1200,"damage_taken":800,"creatures_lost":200,"creatures_killed":240,"spells_cast":700,"counters_cast":30,"removal_cast":90,"peak_board_size":500,"final_life":3300,"survived":70,"target_actions":{"B":210},"kingmaking_signals":4,"combo_assembling_games":40,"combo_executable_games":20,"combo_priority_actions":31,"combo_plan_wins":12}},"games_per_second":500.0}"#,
        )
        .unwrap();
        assert_eq!(report.rows[0].win_rate, 0.55);
        assert!(report.rows[0].win_rate_lower_bound < 0.55);
        assert!(report.rows[0].win_rate_upper_bound > 0.55);
        assert_eq!(report.matchup_matrix["A"]["B"], 0.55);
        assert_eq!(report.matchup_wins["A"]["B"], 55);
        assert_eq!(report.matchup_games["A"]["B"], 100);
        assert_eq!(report.crashes, 1);
        assert_eq!(report.seat_order.len(), 4);
        assert_eq!(report.seat_order[0].position, 1);
        assert_eq!(report.seat_order[0].games, 50);
        assert_eq!(report.seat_order[0].wins, 30);
        assert_eq!(report.seat_order[0].win_rate, 0.6);
        let combo = report.combo_execution.as_ref().unwrap();
        assert_eq!(combo.assembling_games, 40);
        assert_eq!(combo.executable_games, 20);
        assert_eq!(combo.priority_actions, 31);
        assert_eq!(combo.plan_wins, 12);
        assert_eq!(combo.assembling_rate, 0.4);
        assert_eq!(combo.executable_rate, 0.2);
        assert_eq!(combo.win_conversion_rate, 0.6);
        assert!(!combo.plan_available);
        let evidence = report.commander_evidence.unwrap();
        assert_eq!(evidence.schema_version, 1);
        assert_eq!(evidence.evidence_mode, "commander_duel");
        assert_eq!(evidence.complete_rotations, 50);
        assert_eq!(evidence.elimination_slots, vec![45, 55]);
        assert_eq!(evidence.threat_development.source, "rules_engine_compact");
        assert_eq!(evidence.interaction.source, "rules_engine_compact");
        assert_eq!(evidence.target_actions["B"], 210);
        assert!(evidence.penalties.draw_or_stall_rate > 0.0);
        assert!(evidence.penalties.concession_rate > 0.0);
        assert_eq!(evidence.penalties.kingmaking_signal, Some(0.04));
    }

    #[test]
    fn commander_failure_and_exploit_signals_can_only_reduce_objective() {
        let mut clean = CommanderEvidence {
            schema_version: 1,
            policy_id: "test".into(),
            evidence_mode: "multiplayer_pod".into(),
            pod_size: 4,
            seed: 1,
            rotation_policy: "test".into(),
            complete_rotations: 4,
            partial_rotation_games: 0,
            pod_archetypes: vec!["A".into(), "B".into(), "C".into(), "D".into()],
            pod_win_share: commander_metric(0.5, "test"),
            seat_floor: commander_metric(0.4, "test"),
            threat_development: commander_metric(0.6, "test"),
            interaction: commander_metric(0.6, "test"),
            recovery: commander_metric(0.6, "test"),
            mana_reliability: commander_metric(0.8, "test"),
            closing_ability: commander_metric(0.5, "test"),
            seat_performance: Vec::new(),
            elimination_slots: Vec::new(),
            target_actions: HashMap::new(),
            penalties: CommanderPenaltyEvidence::default(),
            objective_score: 0.0,
            limitations: Vec::new(),
        };
        score_commander_evidence(&mut clean);
        let clean_score = clean.objective_score;

        let mut penalized = clean.clone();
        penalized.penalties.draw_or_stall_rate = 0.2;
        penalized.penalties.timeout_or_crash_rate = 0.2;
        penalized.penalties.concession_rate = 0.2;
        penalized.penalties.seat_bias = 0.2;
        penalized.penalties.kingmaking_signal = Some(0.2);
        score_commander_evidence(&mut penalized);

        assert!(penalized.penalties.total_penalty > 0.0);
        assert!(penalized.objective_score < clean_score);
    }

    #[test]
    fn confidence_bounds_and_risk_policy_penalize_thin_or_lopsided_evidence() {
        let (lower, upper) = wilson_interval(55, 100);
        assert!((lower - 0.4524).abs() < 0.001);
        assert!((upper - 0.6439).abs() < 0.001);

        let policy = ConfidencePolicy {
            risk_aversion: 0.5,
            worst_matchup_weight: 0.5,
            seat_position_weight: 0.25,
        };
        let score = |win_rate, lower, worst, worst_lower, crash_rate, seat_position| {
            confidence_adjusted_score(
                ConfidenceScoreInput {
                    win_rate,
                    win_rate_lower_bound: lower,
                    worst_matchup_win_rate: worst,
                    worst_matchup_lower_bound: worst_lower,
                    crash_rate,
                    seat_position,
                },
                policy,
            )
        };
        let balanced = score(0.60, 0.55, 0.55, 0.50, 0.0, None);
        let weak_matchup = score(0.65, 0.60, 0.20, 0.12, 0.0, None);
        let crashy = score(0.70, 0.65, 0.65, 0.60, 0.20, None);
        let position_biased = confidence_adjusted_score(
            ConfidenceScoreInput {
                win_rate: 0.60,
                win_rate_lower_bound: 0.55,
                worst_matchup_win_rate: 0.55,
                worst_matchup_lower_bound: 0.50,
                crash_rate: 0.0,
                seat_position: Some((0.10, 0.04)),
            },
            ConfidencePolicy {
                seat_position_weight: 0.5,
                ..policy
            },
        );
        assert!(balanced > weak_matchup);
        assert!(balanced > crashy);
        assert!(balanced > position_biased);
        assert_eq!(wilson_interval(0, 0), (0.0, 1.0));
    }

    #[test]
    fn seat_position_summary_exposes_floor_and_spread() {
        let totals = HashMap::from([(1, (18, 20)), (2, (2, 20))]);
        let summary = seat_position_summary(&totals).unwrap();
        assert!((summary.worst_rate - 0.10).abs() < f64::EPSILON);
        assert!(summary.worst_lower < summary.worst_rate);
        assert!(summary.worst_upper > summary.worst_rate);
        assert!((summary.spread - 0.80).abs() < f64::EPSILON);
        assert!(seat_position_summary(&HashMap::new()).is_none());
    }

    #[test]
    fn successive_halving_keeps_the_best_half_and_drops_failures() {
        let result = |rank, score, error: Option<&str>| HalvingCandidateResult {
            candidate_rank: rank,
            commander: format!("Commander {rank}"),
            structural_score: score,
            round_win_rate: score,
            game_win_rate: score,
            cumulative_games: 100,
            crash_rate: 0.0,
            win_rate_lower_bound: score,
            win_rate_upper_bound: score,
            worst_matchup_win_rate: score,
            worst_matchup_lower_bound: score,
            worst_matchup_upper_bound: score,
            worst_position_win_rate: None,
            worst_position_lower_bound: None,
            worst_position_upper_bound: None,
            position_win_rate_spread: None,
            evidence_score: score,
            evidence_lower_bound: score,
            evidence_upper_bound: score,
            blended_score: score,
            blended_lower_bound: score,
            blended_upper_bound: score,
            report: None,
            error: error.map(str::to_string),
        };
        let mut results = vec![
            result(1, 0.51, None),
            result(2, 0.80, None),
            result(3, 0.20, Some("crashed")),
            result(4, 0.65, None),
            result(5, 0.40, None),
        ];
        assert_eq!(
            select_halving_survivors(&mut results, 5, false).0,
            vec![2, 4, 1]
        );
        assert_eq!(results.last().unwrap().candidate_rank, 3);
    }

    #[test]
    fn adaptive_halving_stops_statistically_dominated_candidates() {
        assert_eq!(maximum_halving_game_budget(8, 16, 3, 1, false, 2), 384);
        assert_eq!(maximum_halving_game_budget(8, 16, 3, 3, false, 2), 1_152);
        assert_eq!(maximum_halving_game_budget(8, 17, 3, 1, true, 4), 440);
        assert_eq!(complete_rotation_games(17, true, 4), 20);
        assert_eq!(complete_rotation_games(17, true, 6), 18);
        assert_eq!(complete_rotation_games(17, false, 6), 17);
        let result = |rank, score, lower, upper| HalvingCandidateResult {
            candidate_rank: rank,
            commander: format!("Commander {rank}"),
            structural_score: score,
            round_win_rate: score,
            game_win_rate: score,
            cumulative_games: 200,
            crash_rate: 0.0,
            win_rate_lower_bound: lower,
            win_rate_upper_bound: upper,
            worst_matchup_win_rate: score,
            worst_matchup_lower_bound: lower,
            worst_matchup_upper_bound: upper,
            worst_position_win_rate: None,
            worst_position_lower_bound: None,
            worst_position_upper_bound: None,
            position_win_rate_spread: None,
            evidence_score: score,
            evidence_lower_bound: lower,
            evidence_upper_bound: upper,
            blended_score: score,
            blended_lower_bound: lower,
            blended_upper_bound: upper,
            report: None,
            error: None,
        };
        let mut clear = vec![
            result(1, 0.80, 0.72, 0.86),
            result(2, 0.48, 0.41, 0.56),
            result(3, 0.45, 0.38, 0.53),
            result(4, 0.42, 0.35, 0.50),
        ];
        let (survivors, eliminated) = select_halving_survivors(&mut clear, 4, true);
        assert_eq!(survivors, vec![1]);
        assert_eq!(eliminated, vec![2, 3, 4]);

        let mut close = vec![
            result(1, 0.60, 0.50, 0.70),
            result(2, 0.58, 0.48, 0.68),
            result(3, 0.56, 0.46, 0.66),
            result(4, 0.54, 0.44, 0.64),
        ];
        let (survivors, eliminated) = select_halving_survivors(&mut close, 4, true);
        assert_eq!(survivors, vec![1, 2]);
        assert!(eliminated.is_empty());
    }
}
