#![recursion_limit = "256"]

pub mod art_cache;
pub mod calibration;
pub mod catalog;
pub mod combo;
pub mod competitive;
pub mod conformance;
pub mod conformance_contract;
pub mod evaluation_splits;
pub mod feedback;
pub mod forge;
pub mod gauntlets;
pub mod hall_of_fame;
pub mod hexdek;
pub mod maintenance;
pub mod match_cache;
pub mod metagame_sources;
pub mod provenance;
pub mod resource_governor;
mod runs;
pub mod sideboard_evolution;
pub mod simulator;
pub mod surrogate;

use catalog::CatalogStatus;
use metagene_core::{
    CancellationToken, Card, CardPool, Deck, DeckConstraints, EvolutionConfig, EvolutionEngine,
    EvolutionMode, GenerationProgress, MapEliteCell, OpponentFieldProfile, RankedDeck, RunResult,
    StructuralEvaluationContext, apply_fitness_weights, evaluate_structural_deck_with_context,
    evaluate_tune_deck_with_context, export_mtga, import_decklist, validate_deck_legality,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use sysinfo::System;
use tauri::{Emitter, Manager, State};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
struct RunRequest {
    platform: String,
    format: String,
    #[serde(default = "default_simulator_coverage")]
    simulator_coverage: String,
    #[serde(default)]
    pool_filter: Vec<String>,
    #[serde(default)]
    exclude_disputed_cards: bool,
    #[serde(default)]
    exclude_unsupported_cards: bool,
    pool: CardPool,
    seed_deck: Deck,
    constraints: DeckConstraints,
    config: EvolutionConfig,
    #[serde(default)]
    opponent_field: OpponentFieldProfile,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredRequest {
    platform: String,
    format: String,
    #[serde(default = "default_simulator_coverage")]
    simulator_coverage: String,
    #[serde(default)]
    pool_filter: Vec<String>,
    #[serde(default)]
    exclude_disputed_cards: bool,
    #[serde(default)]
    exclude_unsupported_cards: bool,
    seed_deck: Deck,
    constraints: DeckConstraints,
    config: EvolutionConfig,
    #[serde(default)]
    opponent_field: OpponentFieldProfile,
    #[serde(default)]
    resumed_from: Option<String>,
    #[serde(default = "default_run_kind")]
    run_kind: String,
    #[serde(default)]
    hybrid: Option<StoredHybridState>,
    #[serde(default)]
    structural_lineage: Option<StoredStructuralLineage>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredStructuralLineage {
    segment: usize,
    generation_offset: usize,
    evaluated_offset: u64,
    unique_offset: u64,
    avoided_offset: u64,
    elapsed_offset_ms: u64,
    segment_generations: usize,
    incumbent: RankedDeck,
    #[serde(default)]
    map_elites: Vec<MapEliteCell>,
}

#[derive(Clone)]
struct StructuralContinuationLaunch {
    run_id: String,
    lineage: StoredStructuralLineage,
    initial_status: JobStatus,
}

fn default_simulator_coverage() -> String {
    "all".into()
}

fn structural_context(request: &RunRequest) -> StructuralEvaluationContext {
    let commander =
        !request.seed_deck.command_zone.is_empty() || request.constraints.minimum_commanders > 0;
    let commander_colors = request
        .seed_deck
        .command_zone
        .iter()
        .filter_map(|entry| request.pool.card(&entry.card_id))
        .flat_map(|card| card.color_identity.iter().copied())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    StructuralEvaluationContext {
        platform: request.platform.clone(),
        format: request.format.clone(),
        commander,
        commander_colors,
        opponent_field: request.opponent_field.clone(),
        compact_evaluation: false,
    }
}

fn opponent_field_profile(opponents: &[Deck], pool: &CardPool) -> OpponentFieldProfile {
    if opponents.is_empty() {
        return OpponentFieldProfile::default();
    }
    let mut creatures = 0.0_f64;
    let mut artifacts_or_enchantments = 0.0_f64;
    let mut graveyard = 0.0_f64;
    let mut stack_spells = 0.0_f64;
    for deck in opponents {
        let mut nonlands = 0.0_f64;
        let mut deck_creatures = 0.0_f64;
        let mut deck_artifacts_or_enchantments = 0.0_f64;
        let mut deck_graveyard = 0.0_f64;
        let mut deck_stack_spells = 0.0_f64;
        for entry in &deck.mainboard {
            let Some(card) = pool.card(&entry.card_id) else {
                continue;
            };
            if card.features.is_land {
                continue;
            }
            let copies = f64::from(entry.quantity);
            nonlands += copies;
            deck_creatures += copies * f64::from(card.type_line.contains("Creature"));
            deck_artifacts_or_enchantments += copies
                * f64::from(
                    card.type_line.contains("Artifact") || card.type_line.contains("Enchantment"),
                );
            let text = card.oracle_text.to_ascii_lowercase();
            deck_graveyard += copies
                * f64::from(
                    text.contains("graveyard") || text.contains("mill") || text.contains("discard"),
                );
            deck_stack_spells += copies
                * f64::from(
                    card.type_line.contains("Instant") || card.type_line.contains("Sorcery"),
                );
        }
        let divisor = nonlands.max(1.0);
        creatures += deck_creatures / divisor;
        artifacts_or_enchantments += deck_artifacts_or_enchantments / divisor;
        graveyard += (deck_graveyard / divisor * 1.5).min(1.0);
        stack_spells += (deck_stack_spells / divisor * 1.5).min(1.0);
    }
    let decks = opponents.len() as f64;
    OpponentFieldProfile {
        available: true,
        creature_rate: (creatures / decks).clamp(0.05, 0.95) as f32,
        artifact_or_enchantment_rate: (artifacts_or_enchantments / decks).clamp(0.05, 0.95) as f32,
        graveyard_rate: (graveyard / decks).clamp(0.05, 0.95) as f32,
        stack_spell_rate: (stack_spells / decks).clamp(0.05, 0.95) as f32,
    }
}

fn require_structural_validation(
    deck: &Deck,
    pool: &CardPool,
    commander: bool,
    seed: i64,
    turn_horizon: usize,
) -> Result<(), String> {
    let context = StructuralEvaluationContext {
        commander,
        commander_colors: deck
            .command_zone
            .iter()
            .filter_map(|entry| pool.card(&entry.card_id))
            .flat_map(|card| card.color_identity.iter().copied())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect(),
        ..StructuralEvaluationContext::default()
    };
    let fitness = evaluate_structural_deck_with_context(
        deck,
        pool,
        seed as u64,
        32,
        turn_horizon.min(u8::MAX as usize) as u8,
        &context,
    )
    .map_err(|error| format!("Functional structural rescore failed: {error}"))?;
    if fitness.severe_mana_failure || fitness.severe_structural_failure {
        let mut reasons = fitness.mana_blocking_reasons.clone();
        reasons.extend(fitness.structural_blocking_reasons.clone());
        return Err(format!(
            "Validation blocked by structural model {}: {}. The legal deck remains exportable.",
            metagene_core::STRUCTURAL_MODEL_REVISION,
            reasons.join("; ")
        ));
    }
    Ok(())
}

fn require_tune_validation(
    deck: &Deck,
    pool: &CardPool,
    config: &EvolutionConfig,
    context: &StructuralEvaluationContext,
    seed: i64,
    turn_horizon: usize,
) -> Result<(), String> {
    if config.evolution_mode != EvolutionMode::Tune {
        return Ok(());
    }
    let root = config.tune_policy.root_deck.as_ref().ok_or(
        "Tune validation requires the immutable original root deck. The legal deck remains exportable.",
    )?;
    let (fitness, _) = evaluate_tune_deck_with_context(
        deck,
        root,
        pool,
        seed as u64,
        config.trials_per_candidate.max(32),
        turn_horizon.min(u8::MAX as usize) as u8,
        context,
        &config.tune_policy,
        &config.fitness_weights,
    )
    .map_err(|error| format!("Functional Tune rescore failed: {error}"))?;
    if fitness.severe_mana_failure || fitness.severe_structural_failure {
        let mut reasons = fitness.mana_blocking_reasons;
        reasons.extend(fitness.structural_blocking_reasons);
        return Err(format!(
            "Validation blocked by Tune under structural model {}: {}. The legal deck remains exportable.",
            metagene_core::STRUCTURAL_MODEL_REVISION,
            reasons.join("; ")
        ));
    }
    Ok(())
}

fn require_saved_run_tune_validation(
    data_dir: &std::path::Path,
    run_id: Option<&str>,
    deck: &Deck,
    pool: &CardPool,
    commander: bool,
    seed: i64,
    turn_horizon: usize,
) -> Result<(), String> {
    let Some(run_id) = run_id else {
        return Ok(());
    };
    let row = runs::load(data_dir, run_id).map_err(|error| error.to_string())?;
    let mut stored: StoredRequest =
        serde_json::from_str(&row.request_json).map_err(|error| error.to_string())?;
    let mut status: JobStatus =
        serde_json::from_str(&row.status_json).map_err(|error| error.to_string())?;
    canonicalize_stored_run_ids(data_dir, &mut stored, &mut status)?;
    let original_seed = stored.seed_deck.clone();
    if saved_structural_revision(&status) != metagene_core::STRUCTURAL_MODEL_REVISION
        && !original_seed.mainboard.is_empty()
    {
        stored.config.evolution_mode = EvolutionMode::Tune;
        stored.config.tune_policy.root_deck = Some(original_seed);
        stored.config.tune_policy.strategy_retention_floor = 0.70;
        stored.config.tune_policy.primary_package_floor = 0.60;
        stored.config.tune_policy.maximum_nonland_edit_fraction = 0.40;
    }
    let context = StructuralEvaluationContext {
        commander,
        commander_colors: deck
            .command_zone
            .iter()
            .filter_map(|entry| pool.card(&entry.card_id))
            .flat_map(|card| card.color_identity.iter().copied())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect(),
        platform: stored.platform,
        format: stored.format,
        opponent_field: stored.opponent_field,
        compact_evaluation: false,
    };
    require_tune_validation(deck, pool, &stored.config, &context, seed, turn_horizon)
}

fn default_run_kind() -> String {
    "structural".into()
}

fn conformance_classification_excluded(
    classification: &str,
    exclude_disputed: bool,
    exclude_unsupported: bool,
) -> bool {
    (exclude_disputed && classification == "disputed")
        || (exclude_unsupported && classification == "unsupported")
}

impl From<&RunRequest> for StoredRequest {
    fn from(request: &RunRequest) -> Self {
        Self {
            platform: request.platform.clone(),
            format: request.format.clone(),
            simulator_coverage: request.simulator_coverage.clone(),
            pool_filter: request.pool_filter.clone(),
            exclude_disputed_cards: request.exclude_disputed_cards,
            exclude_unsupported_cards: request.exclude_unsupported_cards,
            seed_deck: request.seed_deck.clone(),
            constraints: request.constraints.clone(),
            config: request.config.clone(),
            opponent_field: request.opponent_field.clone(),
            resumed_from: None,
            run_kind: default_run_kind(),
            hybrid: None,
            structural_lineage: None,
        }
    }
}

#[derive(Clone, Serialize)]
struct SystemProfile {
    cpu_name: String,
    physical_cores: usize,
    logical_cores: usize,
    memory_gib: f64,
    gpu_name: Option<String>,
    gpu_memory_gib: Option<f64>,
    recommended_workers: usize,
}

#[derive(Clone, Serialize, Deserialize)]
struct JobStatus {
    id: String,
    state: String,
    progress: Option<GenerationProgress>,
    result: Option<RunResult>,
    error: Option<String>,
}

#[derive(Serialize)]
struct SavedRun {
    request: RunRequest,
    status: JobStatus,
    resumable: bool,
    remaining_generations: usize,
    resumed_from: Option<String>,
    run_kind: String,
    remaining_cycles: usize,
    hybrid_phase: Option<String>,
    hybrid_current_cycle: Option<usize>,
    hybrid_gauntlet_provenance: Option<GauntletProvenance>,
}

#[derive(Serialize)]
struct ResumedRun {
    id: String,
    request: RunRequest,
    resumed_from: String,
    resumed_generation: usize,
    run_kind: String,
    remaining_cycles: usize,
}

#[derive(Serialize)]
struct ContinuedRun {
    id: String,
    request: RunRequest,
    continued_from: String,
    status: JobStatus,
    generations_completed: usize,
}

#[derive(Serialize)]
struct SavedValidation {
    id: String,
    run_id: Option<String>,
    candidate_rank: Option<usize>,
    created_at: String,
    request: serde_json::Value,
    report: simulator::SimulationReport,
}

#[derive(Serialize)]
struct SavedHalving {
    id: String,
    run_id: Option<String>,
    created_at: String,
    request: serde_json::Value,
    report: simulator::SuccessiveHalvingReport,
}

#[derive(Serialize)]
struct SavedBestOfThree {
    id: String,
    run_id: Option<String>,
    candidate_rank: Option<usize>,
    created_at: String,
    request: serde_json::Value,
    report: simulator::BestOfThreeSimulationReport,
}

#[derive(Serialize)]
struct SavedSideboardEvolution {
    id: String,
    run_id: Option<String>,
    candidate_rank: Option<usize>,
    created_at: String,
    request: serde_json::Value,
    report: sideboard_evolution::SideboardEvolutionReport,
}

struct JobRecord {
    cancellation: CancellationToken,
    status: JobStatus,
}

#[derive(Clone)]
struct AppState {
    jobs: Arc<Mutex<HashMap<String, JobRecord>>>,
    export_pool: Arc<Mutex<Option<CardPool>>>,
    forge_validation: Arc<Mutex<Option<CancellationToken>>>,
    sideboard_evolution: Arc<Mutex<Option<CancellationToken>>>,
    data_dir: PathBuf,
}

impl AppState {
    fn new(data_dir: PathBuf) -> Self {
        Self {
            jobs: Arc::new(Mutex::new(HashMap::new())),
            export_pool: Arc::new(Mutex::new(None)),
            forge_validation: Arc::new(Mutex::new(None)),
            sideboard_evolution: Arc::new(Mutex::new(None)),
            data_dir,
        }
    }
}

#[derive(Clone, Serialize)]
struct ProgressEnvelope {
    job_id: String,
    progress: GenerationProgress,
}

#[derive(Clone, Serialize)]
struct CompleteEnvelope {
    job_id: String,
    status: JobStatus,
}

#[derive(Clone, Serialize, Deserialize)]
struct HybridEvolutionRequest {
    evolution: RunRequest,
    opponents: Vec<Deck>,
    #[serde(default)]
    opponent_weights: Vec<f64>,
    #[serde(default)]
    gauntlet_provenance: Option<GauntletProvenance>,
    #[serde(default = "default_competitive_claims_allowed")]
    competitive_claims_allowed: bool,
    #[serde(default)]
    field_revision_id: Option<String>,
    cycles: usize,
    finalists: usize,
    base_games: usize,
    rounds: usize,
    workers: usize,
    seats: usize,
    hat_budget: usize,
    max_turns: usize,
    game_timeout_seconds: usize,
    structural_weight: f64,
    #[serde(default = "default_hybrid_risk_aversion")]
    risk_aversion: f64,
    #[serde(default = "default_hybrid_worst_matchup_weight")]
    worst_matchup_weight: f64,
    #[serde(default = "default_hybrid_seat_position_weight")]
    seat_position_weight: f64,
    #[serde(default = "default_hybrid_adaptive_elimination")]
    adaptive_elimination: bool,
    #[serde(default)]
    coevolve_adversaries: bool,
    #[serde(default)]
    learn_packages_from_hall: bool,
    #[serde(default = "default_hybrid_holdout_games")]
    holdout_games: usize,
    commander: bool,
    #[serde(default)]
    resume: Option<HybridResumeState>,
    #[serde(default)]
    resumed_from: Option<String>,
    #[serde(default)]
    persisted_config: Option<StoredHybridConfig>,
    /// Continue a completed Tune run in-place instead of creating a child run.
    #[serde(default)]
    continue_run_id: Option<String>,
    #[serde(default)]
    continuation_candidate_rank: Option<usize>,
}

#[derive(Clone, Serialize, Deserialize)]
struct GroundEvolutionRequest {
    evolution: RunRequest,
    opponents: Vec<Deck>,
    #[serde(default)]
    opponent_weights: Vec<f64>,
    commander: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct GauntletProvenance {
    id: String,
    name: String,
    revision: usize,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredHybridConfig {
    opponents: Vec<Deck>,
    opponent_weights: Vec<f64>,
    #[serde(default)]
    gauntlet_provenance: Option<GauntletProvenance>,
    #[serde(default = "default_competitive_claims_allowed")]
    competitive_claims_allowed: bool,
    #[serde(default)]
    field_revision_id: Option<String>,
    cycles: usize,
    generations_per_cycle: usize,
    finalists: usize,
    base_games: usize,
    rounds: usize,
    workers: usize,
    seats: usize,
    hat_budget: usize,
    max_turns: usize,
    game_timeout_seconds: usize,
    structural_weight: f64,
    risk_aversion: f64,
    worst_matchup_weight: f64,
    seat_position_weight: f64,
    adaptive_elimination: bool,
    #[serde(default)]
    coevolve_adversaries: bool,
    #[serde(default)]
    learn_packages_from_hall: bool,
    #[serde(default = "default_hybrid_holdout_games")]
    holdout_games: usize,
    commander: bool,
    base_seed: u64,
    base_name: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredHybridCheckpoint {
    current_cycle: usize,
    cycles_completed: usize,
    phase: String,
    completed_attempts: usize,
    total_evaluated: u64,
    winner_rank: Option<usize>,
    #[serde(default)]
    structural_generations_completed: usize,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredHybridState {
    config: StoredHybridConfig,
    checkpoint: StoredHybridCheckpoint,
}

#[derive(Clone, Serialize, Deserialize)]
struct HybridResumeState {
    start_cycle: usize,
    cycles_completed: usize,
    phase: String,
    completed_attempts: usize,
    total_evaluated: u64,
    winner_rank: Option<usize>,
    first_cycle_generations: Option<usize>,
    first_cycle_seed: Option<u64>,
    initial_result: Option<RunResult>,
    generation_offset: usize,
    #[serde(default)]
    structural_lineage: Option<StoredStructuralLineage>,
}

fn default_hybrid_risk_aversion() -> f64 {
    0.5
}

fn default_hybrid_worst_matchup_weight() -> f64 {
    0.25
}

fn default_hybrid_seat_position_weight() -> f64 {
    0.25
}

fn default_hybrid_adaptive_elimination() -> bool {
    true
}

fn default_hybrid_holdout_games() -> usize {
    64
}

fn default_competitive_claims_allowed() -> bool {
    true
}

impl StoredHybridConfig {
    fn from_request(request: &HybridEvolutionRequest) -> Self {
        Self {
            opponents: request.opponents.clone(),
            opponent_weights: request.opponent_weights.clone(),
            gauntlet_provenance: request.gauntlet_provenance.clone(),
            competitive_claims_allowed: request.competitive_claims_allowed,
            field_revision_id: request.field_revision_id.clone(),
            cycles: request.cycles,
            generations_per_cycle: request.evolution.config.generations,
            finalists: request.finalists,
            base_games: request.base_games,
            rounds: request.rounds,
            workers: request.workers,
            seats: request.seats,
            hat_budget: request.hat_budget,
            max_turns: request.max_turns,
            game_timeout_seconds: request.game_timeout_seconds,
            structural_weight: request.structural_weight,
            risk_aversion: request.risk_aversion,
            worst_matchup_weight: request.worst_matchup_weight,
            seat_position_weight: request.seat_position_weight,
            adaptive_elimination: request.adaptive_elimination,
            coevolve_adversaries: request.coevolve_adversaries,
            learn_packages_from_hall: request.learn_packages_from_hall,
            holdout_games: request.holdout_games,
            commander: request.commander,
            base_seed: request.evolution.config.seed,
            base_name: request.evolution.seed_deck.name.clone(),
        }
    }

    fn into_request(
        self,
        evolution: RunRequest,
        resume: HybridResumeState,
        resumed_from: String,
    ) -> HybridEvolutionRequest {
        let persisted_config = self.clone();
        HybridEvolutionRequest {
            evolution,
            opponents: self.opponents,
            opponent_weights: self.opponent_weights,
            gauntlet_provenance: self.gauntlet_provenance,
            competitive_claims_allowed: self.competitive_claims_allowed,
            field_revision_id: self.field_revision_id,
            cycles: self.cycles,
            finalists: self.finalists,
            base_games: self.base_games,
            rounds: self.rounds,
            workers: self.workers,
            seats: self.seats,
            hat_budget: self.hat_budget,
            max_turns: self.max_turns,
            game_timeout_seconds: self.game_timeout_seconds,
            structural_weight: self.structural_weight,
            risk_aversion: self.risk_aversion,
            worst_matchup_weight: self.worst_matchup_weight,
            seat_position_weight: self.seat_position_weight,
            adaptive_elimination: self.adaptive_elimination,
            coevolve_adversaries: self.coevolve_adversaries,
            learn_packages_from_hall: self.learn_packages_from_hall,
            holdout_games: self.holdout_games,
            commander: self.commander,
            resume: Some(resume),
            resumed_from: Some(resumed_from),
            persisted_config: Some(persisted_config),
            continue_run_id: None,
            continuation_candidate_rank: None,
        }
    }
}

fn stored_hybrid_request(
    evolution: &RunRequest,
    config: &StoredHybridConfig,
    checkpoint: StoredHybridCheckpoint,
    resumed_from: Option<String>,
    structural_lineage: Option<StoredStructuralLineage>,
) -> StoredRequest {
    let mut stored = StoredRequest::from(evolution);
    stored.run_kind = "hybrid".into();
    stored.resumed_from = resumed_from;
    stored.hybrid = Some(StoredHybridState {
        config: config.clone(),
        checkpoint,
    });
    stored.structural_lineage = structural_lineage;
    stored
}

#[derive(Clone, Serialize)]
struct HybridProgressEnvelope {
    job_id: String,
    cycle: usize,
    total_cycles: usize,
    phase: String,
    generation: Option<GenerationProgress>,
    halving: Option<simulator::HalvingProgress>,
    winner_rank: Option<usize>,
    completed_attempts: usize,
}

#[derive(Clone, Serialize)]
struct HybridCompleteEnvelope {
    job_id: String,
    status: JobStatus,
    cycles_completed: usize,
    winner_rank: Option<usize>,
    total_attempts: usize,
}

#[tauri::command]
fn system_profile() -> SystemProfile {
    let mut system = System::new_all();
    system.refresh_all();
    let logical_cores = system.cpus().len().max(1);
    let physical_cores = System::physical_core_count().unwrap_or(logical_cores);
    let cpu_name = system
        .cpus()
        .first()
        .map(|cpu| cpu.brand().trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Unknown CPU".into());
    let (gpu_name, gpu_memory_gib) = nvidia_profile();

    SystemProfile {
        cpu_name,
        physical_cores,
        logical_cores,
        memory_gib: system.total_memory() as f64 / 1024.0_f64.powi(3),
        gpu_name,
        gpu_memory_gib,
        recommended_workers: logical_cores.saturating_sub(2).max(1),
    }
}

#[tauri::command]
fn resource_budget_status(
    state: State<'_, AppState>,
) -> Result<resource_governor::ResourceBudgetStatus, String> {
    resource_governor::status(&state.data_dir).map_err(|error| error.to_string())
}

#[tauri::command]
fn save_resource_budgets(
    budgets: resource_governor::ResourceBudgets,
    state: State<'_, AppState>,
) -> Result<resource_governor::ResourceBudgetStatus, String> {
    resource_governor::save(&state.data_dir, &budgets).map_err(|error| error.to_string())
}

fn nvidia_profile() -> (Option<String>, Option<f64>) {
    let output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output();
    let Ok(output) = output else {
        return (None, None);
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let Some(line) = text.lines().next() else {
        return (None, None);
    };
    let mut columns = line.split(',').map(str::trim);
    let name = columns
        .next()
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let memory = columns
        .next()
        .and_then(|value| value.parse::<f64>().ok())
        .map(|mib| mib / 1024.0);
    (name, memory)
}

#[tauri::command]
fn catalog_status(state: State<'_, AppState>) -> Result<CatalogStatus, String> {
    catalog::status(&state.data_dir).map_err(|error| error.to_string())
}

#[tauri::command]
async fn sync_catalog(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<CatalogStatus, String> {
    let data_dir = state.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        catalog::sync(&data_dir, |progress| {
            let _ = app.emit("catalog-progress", progress);
        })
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn search_cards(
    query: String,
    platform: String,
    format: String,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<Vec<Card>, String> {
    catalog::search(&state.data_dir, &query, &platform, &format, limit.min(250))
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn new_workspace(
    platform: String,
    format: String,
    deck_size: Option<usize>,
    state: State<'_, AppState>,
) -> Result<RunRequest, String> {
    let cards = catalog::legal_pool(&state.data_dir, &platform, &format)
        .map_err(|error| error.to_string())?;
    if cards.is_empty() {
        return Err(format!(
            "No {platform} cards were found for {format}. Refresh the catalog or choose another combination."
        ));
    }
    let normalized = format.to_ascii_lowercase();
    let is_commander = normalized == "commander";
    let is_brawl = normalized.contains("brawl");
    let library_size = deck_size.unwrap_or(if is_commander {
        99
    } else if is_brawl {
        59
    } else {
        60
    });
    let singleton = is_commander || is_brawl;
    let request = RunRequest {
        platform,
        format,
        simulator_coverage: if simulator::statuses(&state.data_dir)
            .iter()
            .any(|status| status.id == "hexdek" && status.installed && status.data_ready)
        {
            "parser_complete".into()
        } else {
            "all".into()
        },
        pool_filter: Vec::new(),
        exclude_disputed_cards: true,
        exclude_unsupported_cards: true,
        pool: CardPool { cards },
        seed_deck: Deck {
            name: "Untitled Evolution".into(),
            command_zone: Vec::new(),
            mainboard: Vec::new(),
            sideboard: Vec::new(),
        },
        constraints: DeckConstraints {
            target_mainboard_size: library_size,
            target_sideboard_size: 0,
            minimum_commanders: usize::from(singleton),
            maximum_commanders: if is_commander {
                2
            } else {
                usize::from(is_brawl)
            },
            default_max_copies: if singleton { 1 } else { 4 },
            locked_cards: Vec::new(),
            minimum_lands: Some(if is_commander {
                34
            } else if is_brawl {
                23
            } else {
                (library_size as f32 * 0.34).round() as usize
            }),
            maximum_lands: Some(if is_commander {
                46
            } else {
                (library_size as f32 * 0.48).round() as usize
            }),
            allowed_colors: None,
        },
        config: EvolutionConfig {
            evolve_commanders: singleton,
            ..EvolutionConfig::default()
        },
        opponent_field: OpponentFieldProfile::default(),
    };
    *state
        .export_pool
        .lock()
        .map_err(|_| "export pool poisoned")? = Some(request.pool.clone());
    Ok(request)
}

#[tauri::command]
fn import_workspace(
    text: String,
    platform: String,
    format: String,
    state: State<'_, AppState>,
) -> Result<RunRequest, String> {
    let mut request = new_workspace(platform, format.clone(), None, state)?;
    let mut deck = import_decklist(&text, &request.pool).map_err(|error| error.to_string())?;
    let normalized = format.to_ascii_lowercase();
    let fixed_library_size = if normalized == "commander" {
        Some(99)
    } else if normalized.contains("brawl") {
        Some(59)
    } else {
        None
    };
    let imported_size = deck.mainboard_count();
    request.constraints.target_sideboard_size = deck.sideboard_count();
    if let Some(expected) = fixed_library_size {
        if imported_size != expected {
            return Err(format!(
                "{format} requires a {expected}-card library; the import contains {imported_size}."
            ));
        }
    } else if imported_size < 60 {
        return Err(format!(
            "{format} requires at least 60 main-deck cards; the import contains {imported_size}."
        ));
    } else {
        request.constraints.target_mainboard_size = imported_size;
        request.constraints.minimum_lands = Some((imported_size as f32 * 0.34).round() as usize);
        request.constraints.maximum_lands = Some((imported_size as f32 * 0.48).round() as usize);
    }

    deck.name = format!("Imported {} Deck", title_case(&format));
    request.config.evolve_commanders =
        request.constraints.minimum_commanders > 0 && deck.command_zone.is_empty();
    request.config.evolve_sideboard = deck.sideboard_count() > 0;
    let mut legality_constraints = request.constraints.clone();
    if request.config.evolve_commanders {
        legality_constraints.minimum_commanders = 0;
    }
    validate_deck_legality(&deck, &request.pool, &legality_constraints)
        .map_err(|error| format!("Imported deck is not legal: {error}"))?;
    request.config.evolution_mode = EvolutionMode::Tune;
    request.config.tune_policy.root_deck = Some(deck.clone());
    request.config.tune_policy.strategy_retention_floor = 0.70;
    request.config.tune_policy.primary_package_floor = 0.60;
    request.config.tune_policy.maximum_nonland_edit_fraction = 0.40;
    request.seed_deck = deck;
    Ok(request)
}

#[tauri::command]
fn import_gauntlet_deck(
    text: String,
    platform: String,
    format: String,
    state: State<'_, AppState>,
) -> Result<Deck, String> {
    import_workspace(text, platform, format, state).map(|request| request.seed_deck)
}

#[tauri::command]
async fn acquire_metagame(
    mut request: metagame_sources::MetagameAcquisitionRequest,
    state: State<'_, AppState>,
) -> Result<metagame_sources::MetagameAcquisitionReport, String> {
    request.platform = request.platform.trim().to_ascii_lowercase();
    request.format = request.format.trim().to_ascii_lowercase();
    request.lookback_days = request.lookback_days.clamp(1, 365);
    let pool = CardPool {
        cards: catalog::legal_pool(&state.data_dir, &request.platform, &request.format)
            .map_err(|error| error.to_string())?,
    };
    tauri::async_runtime::spawn_blocking(move || {
        metagame_sources::acquire(&request, &pool).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[derive(Clone, Serialize, Deserialize)]
struct CompetitiveFieldRequest {
    platform: String,
    format: String,
    commander: bool,
    #[serde(default)]
    topdeck_api_key: Option<String>,
    #[serde(default = "default_field_lookback_days")]
    lookback_days: usize,
    #[serde(default)]
    policy: competitive::FieldSelectionPolicy,
}

fn default_field_lookback_days() -> usize {
    30
}

#[derive(Clone, Serialize)]
struct CompetitiveFieldAcquisition {
    field: competitive::MetagameFieldRevision,
    saved: gauntlets::SavedGauntlet,
    source_attempts: Vec<String>,
}

#[tauri::command]
async fn acquire_competitive_field(
    mut request: CompetitiveFieldRequest,
    state: State<'_, AppState>,
) -> Result<CompetitiveFieldAcquisition, String> {
    request.platform = request.platform.trim().to_ascii_lowercase();
    request.format = request.format.trim().to_ascii_lowercase();
    request.lookback_days = request.lookback_days.clamp(1, 365);
    let data_dir = state.data_dir.clone();
    let pool = CardPool {
        cards: catalog::legal_pool(&data_dir, &request.platform, &request.format)
            .map_err(|error| error.to_string())?,
    };
    tauri::async_runtime::spawn_blocking(move || {
        let mut attempts = Vec::new();
        let mut acquired = None;
        let mut sources = vec![("spicerack", None)];
        if request
            .topdeck_api_key
            .as_deref()
            .is_some_and(|key| !key.trim().is_empty())
        {
            sources.push(("topdeck", request.topdeck_api_key.clone()));
        }
        sources.push(("getpaird", None));
        for (source, api_key) in sources {
            let source_request = metagame_sources::MetagameAcquisitionRequest {
                source: source.into(),
                platform: request.platform.clone(),
                format: request.format.clone(),
                commander: request.commander,
                lookback_days: request.lookback_days,
                api_key,
            };
            match metagame_sources::acquire(&source_request, &pool) {
                Ok(report) if !report.accepted.is_empty() => {
                    attempts.push(format!("{source}: selected"));
                    acquired = Some(report);
                    break;
                }
                Ok(_) => attempts.push(format!("{source}: returned no legal decks")),
                Err(error) => attempts.push(format!("{source}: {error}")),
            }
        }
        let report = acquired.ok_or_else(|| {
            format!(
                "No compatible tournament or public-list field could be acquired: {}",
                attempts.join("; ")
            )
        })?;
        let mut field = competitive::select_competitive_field(&report, &pool, &request.policy);
        field.audit.warnings.extend(
            attempts
                .iter()
                .filter(|attempt| !attempt.ends_with(": selected"))
                .map(|attempt| format!("Source attempt: {attempt}")),
        );
        if field.decks.is_empty() {
            return Err("The acquired field contained no selectable archetypes.".into());
        }
        let name = format!(
            "{} {} field {}",
            title_case(&request.format),
            if field.audit.source_tier == "tournament" {
                "tournament"
            } else {
                "public-list"
            },
            report.provenance.retrieved_at_unix
        );
        let metadata = gauntlets::GauntletMetadata {
            source: report.source.clone(),
            observed_at: format!("last {} days", request.lookback_days),
            notes: format!(
                "Automatic competitive field · {} selected clusters · {:.1}% observed weight.",
                field.clusters.len(),
                field.audit.observed_weight_covered * 100.0
            ),
            source_url: Some(report.provenance.source_url.clone()),
            retrieved_at_unix: Some(report.provenance.retrieved_at_unix),
            event_start_unix: report.provenance.event_start_unix,
            event_end_unix: report.provenance.event_end_unix,
            sample_size: Some(report.provenance.accepted_deck_count),
            transformation_notes: report.provenance.transformation_notes.clone(),
            field_revision_id: Some(field.revision_id.clone()),
            source_tier: Some(field.audit.source_tier.clone()),
            competitive_claims_allowed: Some(field.audit.competitive_claims_allowed),
            archetype_labels: field
                .clusters
                .iter()
                .map(|cluster| cluster.label.clone())
                .collect(),
            quarantined_summary: field
                .audit
                .quarantined
                .iter()
                .map(|item| format!("{:?}: {}", item.reason, item.submitted_name))
                .collect(),
        };
        validate_gauntlet_decks(
            &data_dir,
            &request.platform,
            &request.format,
            request.commander,
            &field.decks,
        )?;
        let saved = gauntlets::save(
            &data_dir,
            &name,
            &request.platform,
            &request.format,
            request.commander,
            &field.decks,
            &field.weights,
            &metadata,
        )
        .map_err(|error| error.to_string())?;
        Ok(CompetitiveFieldAcquisition {
            field,
            saved,
            source_attempts: attempts,
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

#[derive(Clone, Serialize, Deserialize)]
struct EngineRouteRequest {
    platform: String,
    format: String,
    candidate: Deck,
    opponents: Vec<Deck>,
    #[serde(default)]
    opponent_weights: Vec<f64>,
    #[serde(default)]
    tournament_claims_allowed: bool,
}

#[tauri::command]
async fn build_engine_route_plan(
    request: EngineRouteRequest,
    state: State<'_, AppState>,
) -> Result<competitive::EngineRoutePlan, String> {
    let data_dir = state.data_dir.clone();
    let pool = CardPool {
        cards: catalog::legal_pool(&data_dir, &request.platform, &request.format)
            .map_err(|error| error.to_string())?,
    };
    validate_gauntlet_decks(
        &data_dir,
        &request.platform,
        &request.format,
        !request.candidate.command_zone.is_empty(),
        &request.opponents,
    )?;
    tauri::async_runtime::spawn_blocking(move || {
        Ok(competitive::build_engine_route_plan(
            &data_dir,
            &request.candidate,
            &request.opponents,
            &request.opponent_weights,
            &pool,
            request.tournament_claims_allowed,
        ))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn combo_corpus_status(state: State<'_, AppState>) -> Result<combo::ComboCorpusStatus, String> {
    combo::status(&state.data_dir)
}

#[tauri::command]
async fn sync_combo_corpus(
    request: combo::ComboCorpusSyncRequest,
    pool: CardPool,
    state: State<'_, AppState>,
) -> Result<combo::ComboCorpusSyncReport, String> {
    let data_dir = state.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || combo::synchronize(&data_dir, &request, &pool))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn prove_deck_combos(
    request: combo::ComboProofRequest,
    state: State<'_, AppState>,
) -> Result<combo::ComboProofReport, String> {
    let data_dir = state.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || combo::prove(&data_dir, &request))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
fn combo_conformance_scenario(
    deck: Deck,
    line_id: String,
    state: State<'_, AppState>,
) -> Result<combo::ComboConformanceScenario, String> {
    combo::conformance_scenario(&state.data_dir, &deck, &line_id)
}

#[tauri::command]
fn record_combo_conformance_observation(
    request: combo::ComboConformanceObservationRequest,
    state: State<'_, AppState>,
) -> Result<combo::ComboConformanceReport, String> {
    combo::record_conformance_observation(&state.data_dir, &request)
}

fn validate_gauntlet_decks(
    data_dir: &std::path::Path,
    platform: &str,
    format: &str,
    commander: bool,
    decks: &[Deck],
) -> Result<(), String> {
    let normalized_format = format.to_ascii_lowercase();
    let singleton_format = normalized_format == "commander" || normalized_format.contains("brawl");
    if commander != singleton_format {
        return Err(format!(
            "Gauntlet mode does not match the selected {format} format."
        ));
    }
    if decks.is_empty() {
        return Err("A gauntlet must contain at least one opponent deck.".into());
    }
    let maximum_decks = if commander { 5 } else { 32 };
    if decks.len() > maximum_decks {
        return Err(format!(
            "This {format} gauntlet contains {} decks; the current limit is {maximum_decks}.",
            decks.len()
        ));
    }
    let pool = CardPool {
        cards: catalog::legal_pool(data_dir, platform, format)
            .map_err(|error| error.to_string())?,
    };
    for (index, deck) in decks.iter().enumerate() {
        let library_size = deck.mainboard_count();
        let expected_size = if normalized_format == "commander" {
            Some(99)
        } else if normalized_format.contains("brawl") {
            Some(59)
        } else {
            None
        };
        if let Some(expected_size) = expected_size {
            if library_size != expected_size {
                return Err(format!(
                    "Opponent {} has {library_size} library cards; {format} requires {expected_size}.",
                    index + 1
                ));
            }
        } else if library_size < 60 {
            return Err(format!(
                "Opponent {} has {library_size} main-deck cards; {format} requires at least 60.",
                index + 1
            ));
        }
        let constraints = DeckConstraints {
            target_mainboard_size: library_size,
            target_sideboard_size: deck.sideboard_count(),
            minimum_commanders: usize::from(commander),
            maximum_commanders: if normalized_format == "commander" {
                2
            } else {
                usize::from(commander)
            },
            default_max_copies: if commander { 1 } else { 4 },
            locked_cards: Vec::new(),
            minimum_lands: None,
            maximum_lands: None,
            allowed_colors: None,
        };
        validate_deck_legality(deck, &pool, &constraints).map_err(|error| {
            format!(
                "Opponent {} is no longer legal for {platform} {format}: {error}",
                index + 1
            )
        })?;
    }
    Ok(())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri exposes these as named IPC fields to TypeScript.
fn save_gauntlet(
    name: String,
    platform: String,
    format: String,
    commander: bool,
    decks: Vec<Deck>,
    weights: Vec<f64>,
    metadata: gauntlets::GauntletMetadata,
    state: State<'_, AppState>,
) -> Result<gauntlets::SavedGauntlet, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err("Gauntlet name must contain 1 to 80 characters.".into());
    }
    let platform = platform.to_ascii_lowercase();
    let format = format.to_ascii_lowercase();
    validate_gauntlet_decks(&state.data_dir, &platform, &format, commander, &decks)?;
    let weights = normalize_opponent_weights(&weights, decks.len())?;
    let metadata = validate_gauntlet_metadata(metadata)?;
    gauntlets::save(
        &state.data_dir,
        name,
        &platform,
        &format,
        commander,
        &decks,
        &weights,
        &metadata,
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
fn gauntlet_history(
    platform: String,
    format: String,
    commander: bool,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<Vec<gauntlets::GauntletSummary>, String> {
    gauntlets::list(
        &state.data_dir,
        &platform.to_ascii_lowercase(),
        &format.to_ascii_lowercase(),
        commander,
        limit,
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
fn load_gauntlet(
    id: String,
    platform: String,
    format: String,
    commander: bool,
    state: State<'_, AppState>,
) -> Result<gauntlets::SavedGauntlet, String> {
    let mut saved = gauntlets::load(&state.data_dir, &id).map_err(|error| error.to_string())?;
    if !saved.summary.platform.eq_ignore_ascii_case(&platform)
        || !saved.summary.format.eq_ignore_ascii_case(&format)
        || saved.summary.commander != commander
    {
        return Err("Saved gauntlet belongs to a different platform, format, or play mode.".into());
    }
    validate_gauntlet_decks(&state.data_dir, &platform, &format, commander, &saved.decks)?;
    saved.weights = normalize_opponent_weights(&saved.weights, saved.decks.len())?;
    Ok(saved)
}

#[tauri::command]
fn delete_gauntlet(id: String, state: State<'_, AppState>) -> Result<(), String> {
    gauntlets::delete(&state.data_dir, &id).map_err(|error| error.to_string())
}

#[tauri::command]
fn hall_of_fame_status(
    platform: String,
    format: String,
    commander: bool,
    state: State<'_, AppState>,
) -> Result<hall_of_fame::HallOfFameStatus, String> {
    hall_of_fame::status(&state.data_dir, &platform, &format, commander)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn promote_hall_of_fame_candidate(
    platform: String,
    format: String,
    commander: bool,
    candidate: RankedDeck,
    evidence_score: Option<f64>,
    source_run_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<hall_of_fame::HallOfFameEntry, String> {
    validate_gauntlet_decks(
        &state.data_dir,
        &platform,
        &format,
        commander,
        std::slice::from_ref(&candidate.deck),
    )?;
    hall_of_fame::promote(
        &state.data_dir,
        &candidate,
        hall_of_fame::HallOfFamePromotion {
            platform: &platform,
            format: &format,
            commander,
            evidence_score: evidence_score.unwrap_or(candidate.fitness.score),
            source_run_id: source_run_id.as_deref(),
            source_cycle: None,
        },
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
fn delete_hall_of_fame_entry(id: String, state: State<'_, AppState>) -> Result<(), String> {
    hall_of_fame::delete(&state.data_dir, &id).map_err(|error| error.to_string())
}

#[tauri::command]
fn calibration_status(
    platform: String,
    format: String,
    commander: bool,
    state: State<'_, AppState>,
) -> Result<calibration::CalibrationStatus, String> {
    calibration::status(
        &state.data_dir,
        &platform,
        &format,
        commander,
        evaluation_splits::target_interval_coverage(),
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
fn import_match_history(
    source: String,
    text: String,
    state: State<'_, AppState>,
) -> Result<feedback::ImportReport, String> {
    feedback::import(&state.data_dir, &source, &text).map_err(|error| error.to_string())
}

#[tauri::command]
fn match_history(
    limit: usize,
    state: State<'_, AppState>,
) -> Result<Vec<feedback::MatchRecord>, String> {
    feedback::list(&state.data_dir, limit).map_err(|error| error.to_string())
}

#[tauri::command]
fn update_match_record(
    edit: feedback::MatchEdit,
    state: State<'_, AppState>,
) -> Result<feedback::MatchRecord, String> {
    feedback::update(&state.data_dir, &edit).map_err(|error| error.to_string())
}

#[tauri::command]
fn delete_match_records(ids: Vec<String>, state: State<'_, AppState>) -> Result<usize, String> {
    feedback::delete(&state.data_dir, &ids).map_err(|error| error.to_string())
}

#[tauri::command]
fn clear_match_history(state: State<'_, AppState>) -> Result<usize, String> {
    feedback::clear(&state.data_dir).map_err(|error| error.to_string())
}

#[tauri::command]
fn export_match_history(state: State<'_, AppState>) -> Result<String, String> {
    feedback::export(&state.data_dir).map_err(|error| error.to_string())
}

#[tauri::command]
fn feedback_summary(
    platform: String,
    format: String,
    state: State<'_, AppState>,
) -> Result<feedback::FeedbackSummary, String> {
    feedback::summary(&state.data_dir, Some(&platform), Some(&format))
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn suggest_feedback_reweights(
    platform: String,
    format: String,
    opponents: Vec<String>,
    current_weights: Vec<f64>,
    state: State<'_, AppState>,
) -> Result<Vec<feedback::ReweightSuggestion>, String> {
    feedback::suggest_reweights(
        &state.data_dir,
        &platform,
        &format,
        &opponents,
        &current_weights,
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
fn surrogate_status(
    platform: String,
    format: String,
    commander: bool,
    state: State<'_, AppState>,
) -> Result<surrogate::SurrogateStatus, String> {
    surrogate::status(&state.data_dir, &platform, &format, commander)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn train_surrogate_model(
    platform: String,
    format: String,
    commander: bool,
    state: State<'_, AppState>,
) -> Result<surrogate::SurrogateStatus, String> {
    surrogate::train(&state.data_dir, &platform, &format, commander)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn benchmark_surrogate_device(
    platform: String,
    format: String,
    commander: bool,
    state: State<'_, AppState>,
) -> Result<surrogate::DeviceProfile, String> {
    surrogate::benchmark_device(&state.data_dir, &platform, &format, commander)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn compare_gauntlets(
    from_id: String,
    to_id: String,
    state: State<'_, AppState>,
) -> Result<gauntlets::GauntletComparison, String> {
    let comparison =
        gauntlets::compare(&state.data_dir, &from_id, &to_id).map_err(|error| error.to_string())?;
    if comparison.from.platform != comparison.to.platform
        || comparison.from.format != comparison.to.format
        || comparison.from.commander != comparison.to.commander
        || !comparison
            .from
            .name
            .eq_ignore_ascii_case(&comparison.to.name)
    {
        return Err(
            "Only revisions of the same named platform/format field can be compared.".into(),
        );
    }
    Ok(comparison)
}

#[derive(Serialize)]
struct ImportedMetagameBundle {
    name: String,
    platform: String,
    format: String,
    commander: bool,
    decks: Vec<Deck>,
    weights: Vec<f64>,
    metadata: gauntlets::GauntletMetadata,
}

fn normalize_opponent_weights(weights: &[f64], count: usize) -> Result<Vec<f64>, String> {
    let mut weights = if weights.is_empty() {
        vec![1.0; count]
    } else if weights.len() == count {
        weights.to_vec()
    } else {
        return Err(format!(
            "The field has {count} opponents but {} metagame weights.",
            weights.len()
        ));
    };
    if weights
        .iter()
        .any(|weight| !weight.is_finite() || *weight <= 0.0)
    {
        return Err("Every metagame weight must be finite and greater than zero.".into());
    }
    let total = weights.iter().sum::<f64>();
    for weight in &mut weights {
        *weight /= total;
    }
    Ok(weights)
}

fn validate_gauntlet_metadata(
    mut metadata: gauntlets::GauntletMetadata,
) -> Result<gauntlets::GauntletMetadata, String> {
    metadata.source = metadata.source.trim().to_string();
    metadata.observed_at = metadata.observed_at.trim().to_string();
    metadata.notes = metadata.notes.trim().to_string();
    metadata.source_url = metadata
        .source_url
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    metadata.transformation_notes = metadata
        .transformation_notes
        .into_iter()
        .map(|note| note.trim().to_string())
        .filter(|note| !note.is_empty())
        .collect();
    if metadata.source.chars().count() > 500
        || metadata.observed_at.chars().count() > 80
        || metadata.notes.chars().count() > 2_000
        || metadata
            .source_url
            .as_ref()
            .is_some_and(|value| value.chars().count() > 2_000 || !value.starts_with("https://"))
        || metadata.transformation_notes.len() > 32
        || metadata
            .transformation_notes
            .iter()
            .any(|note| note.chars().count() > 500)
    {
        return Err("Metagame provenance is invalid or exceeds its storage limit.".into());
    }
    Ok(metadata)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Mirrors the named portable-bundle fields over IPC.
fn export_metagame_bundle(
    name: String,
    platform: String,
    format: String,
    commander: bool,
    decks: Vec<Deck>,
    weights: Vec<f64>,
    metadata: gauntlets::GauntletMetadata,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err("Metagame name must contain 1 to 80 characters.".into());
    }
    let platform = platform.to_ascii_lowercase();
    let format = format.to_ascii_lowercase();
    validate_gauntlet_decks(&state.data_dir, &platform, &format, commander, &decks)?;
    let weights = normalize_opponent_weights(&weights, decks.len())?;
    let metadata = validate_gauntlet_metadata(metadata)?;
    let pool = CardPool {
        cards: catalog::legal_pool(&state.data_dir, &platform, &format)
            .map_err(|error| error.to_string())?,
    };
    let opponents = decks
        .iter()
        .zip(weights)
        .map(|(deck, weight)| gauntlets::BundleOpponent {
            name: deck.name.clone(),
            weight,
            decklist: export_mtga(deck, &pool),
        })
        .collect();
    serde_json::to_string_pretty(&gauntlets::MetagameBundle {
        schema_version: 1,
        name: name.into(),
        platform,
        format,
        commander,
        metadata,
        opponents,
    })
    .map_err(|error| error.to_string())
}

#[tauri::command]
fn import_metagame_bundle(
    bundle_json: String,
    platform: String,
    format: String,
    commander: bool,
    state: State<'_, AppState>,
) -> Result<ImportedMetagameBundle, String> {
    if bundle_json.len() > 4 * 1024 * 1024 {
        return Err("Metagame bundle exceeds the 4 MiB import limit.".into());
    }
    let bundle: gauntlets::MetagameBundle =
        serde_json::from_str(&bundle_json).map_err(|error| format!("Invalid bundle: {error}"))?;
    if bundle.schema_version != 1 {
        return Err(format!(
            "Unsupported metagame bundle schema {}; expected 1.",
            bundle.schema_version
        ));
    }
    if !bundle.platform.eq_ignore_ascii_case(&platform)
        || !bundle.format.eq_ignore_ascii_case(&format)
        || bundle.commander != commander
    {
        return Err("Bundle platform, format, or play mode does not match this workspace.".into());
    }
    let pool = CardPool {
        cards: catalog::legal_pool(&state.data_dir, &platform, &format)
            .map_err(|error| error.to_string())?,
    };
    let mut decks = Vec::with_capacity(bundle.opponents.len());
    let mut weights = Vec::with_capacity(bundle.opponents.len());
    for (index, opponent) in bundle.opponents.into_iter().enumerate() {
        let mut deck = import_decklist(&opponent.decklist, &pool)
            .map_err(|error| format!("Opponent {} could not be imported: {error}", index + 1))?;
        deck.name = opponent.name.trim().to_string();
        if deck.name.is_empty() {
            deck.name = format!("Opponent {}", index + 1);
        }
        decks.push(deck);
        weights.push(opponent.weight);
    }
    validate_gauntlet_decks(&state.data_dir, &platform, &format, commander, &decks)?;
    let weights = normalize_opponent_weights(&weights, decks.len())?;
    let metadata = validate_gauntlet_metadata(bundle.metadata)?;
    Ok(ImportedMetagameBundle {
        name: bundle.name.trim().to_string(),
        platform: platform.to_ascii_lowercase(),
        format: format.to_ascii_lowercase(),
        commander,
        decks,
        weights,
        metadata,
    })
}

fn title_case(value: &str) -> String {
    let mut characters = value.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

fn prepare_evolution_request(
    data_dir: &std::path::Path,
    mut request: RunRequest,
) -> Result<RunRequest, String> {
    let mut legality_constraints = request.constraints.clone();
    if request.config.evolve_commanders && request.seed_deck.command_zone.is_empty() {
        legality_constraints.minimum_commanders = 0;
    }
    validate_deck_legality(&request.seed_deck, &request.pool, &legality_constraints)
        .map_err(|error| format!("Deck legality error: {error}"))?;
    if request.pool.cards.iter().any(|card| {
        !card
            .platforms
            .iter()
            .any(|platform| platform == &request.platform)
    }) {
        return Err(format!(
            "The selected pool contains a card unavailable on {}.",
            request.platform
        ));
    }

    let original_pool = request.pool.clone();
    if !request.pool_filter.is_empty() {
        request.pool = request.pool.focus_on_names(&request.pool_filter);
    }
    if let Some(allowed) = &request.constraints.allowed_colors {
        if allowed.is_empty() {
            return Err(
                "Choose at least one deck color; choose Colorless for a zero-color deck.".into(),
            );
        }
        let unique = allowed
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        if unique.len() != allowed.len() {
            return Err("The deck-color selection contains a duplicate color.".into());
        }
        request
            .pool
            .cards
            .retain(|card| card.matches_color_selection(allowed));
    }

    // Focused-pool and exact-color selections are hard user constraints. Engine
    // coverage and conformance exclusions are mutation preferences: an imported
    // legal seed must remain structurally evolvable even when HexDek cannot yet
    // parse one of its cards or its semantics are disputed. Preserve only required
    // seed/locked cards as policy exceptions instead of widening the whole pool.
    let hard_scope_pool = request.pool.clone();
    request.pool =
        simulator::filter_pool_for_hexdek(data_dir, &request.pool, &request.simulator_coverage)
            .map_err(|error| format!("Simulator coverage filter failed: {error}"))?;
    if request.exclude_disputed_cards || request.exclude_unsupported_cards {
        let names = request
            .pool
            .cards
            .iter()
            .map(|card| card.name.clone())
            .collect::<Vec<_>>();
        let audit = conformance::audit_cards(data_dir, &names)
            .map_err(|error| format!("Conformance exclusion filter failed: {error}"))?;
        let excluded = audit
            .cards
            .into_iter()
            .filter(|card| {
                conformance_classification_excluded(
                    &card.classification,
                    request.exclude_disputed_cards,
                    request.exclude_unsupported_cards,
                )
            })
            .map(|card| card.name.to_lowercase())
            .collect::<std::collections::HashSet<_>>();
        request
            .pool
            .cards
            .retain(|card| !excluded.contains(&card.name.to_lowercase()));
    }
    let required_ids = request
        .seed_deck
        .command_zone
        .iter()
        .chain(request.seed_deck.mainboard.iter())
        .chain(request.seed_deck.sideboard.iter())
        .map(|entry| entry.card_id.clone())
        .chain(
            request
                .constraints
                .locked_cards
                .iter()
                .map(|locked| locked.card_id.clone()),
        )
        .collect::<std::collections::HashSet<_>>();
    request.pool =
        restore_required_policy_exceptions(&hard_scope_pool, request.pool, &required_ids);
    let retained_ids = request
        .pool
        .cards
        .iter()
        .map(|card| &card.id)
        .collect::<std::collections::HashSet<_>>();
    let rejected_required_cards = request
        .seed_deck
        .command_zone
        .iter()
        .chain(request.seed_deck.mainboard.iter())
        .chain(request.seed_deck.sideboard.iter())
        .map(|entry| &entry.card_id)
        .chain(
            request
                .constraints
                .locked_cards
                .iter()
                .map(|locked| &locked.card_id),
        )
        .filter(|card_id| !retained_ids.contains(card_id))
        .filter_map(|card_id| {
            original_pool
                .cards
                .iter()
                .find(|card| card.id == *card_id)
                .map(|card| card.name.clone())
        })
        .collect::<std::collections::HashSet<_>>();
    if !rejected_required_cards.is_empty() {
        let mut names = rejected_required_cards.into_iter().collect::<Vec<_>>();
        names.sort();
        return Err(format!(
            "The seed, sideboard, or locked constraints contain cards excluded by hard evolution scope settings (focused pool {}, deck colors {}): {}. Clear the focused pool, change Deck colors, or replace those cards.",
            if request.pool_filter.is_empty() {
                "off"
            } else {
                "on"
            },
            request
                .constraints
                .allowed_colors
                .as_ref()
                .map(|colors| colors
                    .iter()
                    .map(|color| format!("{color:?}"))
                    .collect::<Vec<_>>()
                    .join("+"))
                .unwrap_or_else(|| "auto".into()),
            names.into_iter().take(12).collect::<Vec<_>>().join(", ")
        ));
    }
    if request.pool.cards.is_empty() {
        return Err(
            "The selected simulator, focused-pool, and deck-color settings removed every card from the evolution pool.".into(),
        );
    }
    if request.pool.cards.iter().any(|card| {
        !matches!(
            card.legalities.get(&request.format).map(String::as_str),
            Some("legal" | "restricted")
        )
    }) {
        return Err(format!(
            "The selected pool contains a card that is not legal in {}.",
            request.format
        ));
    }

    let combo_root = request
        .config
        .tune_policy
        .root_deck
        .as_ref()
        .unwrap_or(&request.seed_deck);
    if request.config.combo_packages.is_empty() && combo_root.mainboard_count() > 0 {
        request.config.combo_packages = combo::evolution_profiles(combo_root, &request.pool);
    }
    for package in &request.config.combo_packages {
        if package.piece_ids.len() >= 2
            && !request
                .config
                .learned_packages
                .iter()
                .any(|existing| existing == &package.piece_ids)
        {
            request
                .config
                .learned_packages
                .push(package.piece_ids.clone());
        }
    }

    Ok(request)
}

fn restore_required_policy_exceptions(
    hard_scope_pool: &CardPool,
    policy_filtered_pool: CardPool,
    required_ids: &std::collections::HashSet<metagene_core::CardId>,
) -> CardPool {
    let retained_ids = policy_filtered_pool
        .cards
        .iter()
        .map(|card| card.id.clone())
        .collect::<std::collections::HashSet<_>>();
    CardPool {
        cards: hard_scope_pool
            .cards
            .iter()
            .filter(|card| retained_ids.contains(&card.id) || required_ids.contains(&card.id))
            .cloned()
            .collect(),
    }
}

fn add_tournament_cooccurrence_packages(
    config: &mut EvolutionConfig,
    pool: &CardPool,
    opponents: &[Deck],
) {
    if config.evolution_mode != EvolutionMode::Rebuild {
        return;
    }
    for opponent in opponents.iter().take(16) {
        let mut weighted = opponent
            .mainboard
            .iter()
            .filter_map(|entry| {
                pool.card(&entry.card_id)
                    .filter(|card| !card.features.is_land)
                    .map(|card| (entry.quantity, card.id.clone()))
            })
            .collect::<Vec<_>>();
        weighted.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.0.cmp(&right.1.0)));
        let mut package = weighted
            .into_iter()
            .take(24)
            .map(|(_, id)| id)
            .collect::<Vec<_>>();
        package.sort_by(|left, right| left.0.cmp(&right.0));
        package.dedup();
        if package.len() >= 2 && !config.learned_packages.contains(&package) {
            config.learned_packages.push(package);
        }
    }
}

#[tauri::command]
fn ground_evolution_from_field(
    request: GroundEvolutionRequest,
    state: State<'_, AppState>,
) -> Result<RunRequest, String> {
    let mut evolution = prepare_evolution_request(&state.data_dir, request.evolution)?;
    validate_gauntlet_decks(
        &state.data_dir,
        &evolution.platform,
        &evolution.format,
        request.commander,
        &request.opponents,
    )?;
    let weights = if request.commander {
        vec![1.0; request.opponents.len()]
    } else {
        normalize_opponent_weights(&request.opponent_weights, request.opponents.len())?
    };
    add_tournament_cooccurrence_packages(
        &mut evolution.config,
        &evolution.pool,
        &request.opponents,
    );
    ground_prepared_evolution(evolution, &request.opponents, &weights, request.commander)
}

#[tauri::command]
fn start_evolution(
    request: RunRequest,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let request = prepare_evolution_request(&state.data_dir, request)?;
    if blank_rebuild_requires_grounding(&request) {
        return Err(
            "A competitive blank build needs a compatible tournament-field seed or a focused card pool. Automatic field acquisition did not produce a runnable seed, so Metagene stopped instead of evolving an ungrounded all-catalog pile. Retry field acquisition, change the selected colors, or choose a focused pool under Advanced."
                .into(),
        );
    }
    launch_evolution(request, None, None, app, &state)
}

fn blank_rebuild_requires_grounding(request: &RunRequest) -> bool {
    request.seed_deck.mainboard_count() == 0
        && request.seed_deck.command_zone.is_empty()
        && request.config.evolution_mode == EvolutionMode::Rebuild
        && request.pool_filter.is_empty()
        && request.config.learned_packages.is_empty()
}

fn status_contains_candidate(status: &JobStatus, candidate: &RankedDeck) -> bool {
    status
        .result
        .as_ref()
        .map(|result| result.frontier.as_slice())
        .unwrap_or_else(|| {
            status
                .progress
                .as_ref()
                .map(|progress| progress.top.as_slice())
                .unwrap_or_default()
        })
        .iter()
        .any(|saved| saved.deck == candidate.deck)
}

fn status_candidate_by_rank(status: &JobStatus, rank: usize) -> Option<RankedDeck> {
    if let Some(result) = status.result.as_ref() {
        return result_candidate(result, rank).cloned();
    }
    status.progress.as_ref().and_then(|progress| {
        progress
            .baseline
            .as_ref()
            .filter(|candidate| candidate.rank == rank)
            .or_else(|| progress.top.iter().find(|candidate| candidate.rank == rank))
            .cloned()
    })
}

fn build_continued_request(
    mut request: RunRequest,
    candidate: RankedDeck,
    completed_generations: usize,
) -> RunRequest {
    request.seed_deck = candidate.deck;
    request.seed_deck.name = format!("{} · continued", request.seed_deck.name);
    request.config.seed = continued_seed(request.config.seed, completed_generations.max(1));
    request
}

fn pin_continuation_incumbent(
    candidates: &mut Vec<RankedDeck>,
    best: &mut RankedDeck,
    incumbent: &RankedDeck,
) {
    let mut pinned = candidates
        .iter()
        .position(|candidate| candidate.deck == incumbent.deck)
        .map(|index| candidates.remove(index))
        .unwrap_or_else(|| incumbent.clone());
    pinned.rank = 1;
    pinned.candidate_status = "incumbent".into();
    for (index, candidate) in candidates.iter_mut().enumerate() {
        candidate.rank = index + 2;
    }
    if best.deck == pinned.deck {
        *best = pinned.clone();
    } else if let Some(current) = candidates
        .iter()
        .find(|candidate| candidate.deck == best.deck)
    {
        best.rank = current.rank;
    }
    candidates.insert(0, pinned);
}

fn continuation_launch(
    run_id: String,
    status: &JobStatus,
    candidate: &RankedDeck,
    additional_generations: usize,
    previous_lineage: Option<&StoredStructuralLineage>,
) -> Result<StructuralContinuationLaunch, String> {
    let (
        generation,
        evaluated,
        unique,
        avoided,
        elapsed,
        mut top,
        baseline,
        signature,
        revision,
        map_elites,
    ) = if let Some(result) = &status.result {
        (
            result.generations_completed,
            result.evaluated_candidates,
            result.unique_evaluations,
            result.evaluations_avoided,
            result.elapsed_ms,
            result.frontier.clone(),
            result.baseline.clone(),
            result.strategy_signature.clone(),
            result.structural_model_revision.clone(),
            result.map_elites.clone(),
        )
    } else if let Some(progress) = &status.progress {
        (
            progress.generation,
            progress.evaluated_candidates,
            progress.unique_evaluations,
            progress.evaluations_avoided,
            progress.elapsed_ms,
            progress.top.clone(),
            progress.baseline.clone(),
            progress.strategy_signature.clone(),
            progress.structural_model_revision.clone(),
            Vec::new(),
        )
    } else {
        return Err("This run has no completed generation checkpoint to continue.".into());
    };
    let mut incumbent = candidate.clone();
    incumbent.candidate_status = "incumbent".into();
    let mut best = incumbent.clone();
    pin_continuation_incumbent(&mut top, &mut best, &incumbent);
    let elapsed_seconds = elapsed as f64 / 1_000.0;
    let initial_status = JobStatus {
        id: run_id.clone(),
        state: "running".into(),
        progress: Some(GenerationProgress {
            generation,
            total_generations: generation.saturating_add(additional_generations),
            evaluated_candidates: evaluated,
            unique_evaluations: unique,
            evaluations_avoided: avoided,
            unique_in_generation: 0,
            duplicates_in_generation: 0,
            elapsed_ms: elapsed,
            candidates_per_second: evaluated as f64 / elapsed_seconds.max(0.001),
            unique_evaluations_per_second: unique as f64 / elapsed_seconds.max(0.001),
            best,
            top,
            baseline,
            strategy_signature: signature,
            structural_model_revision: revision,
            map_elite_cells: map_elites.len(),
        }),
        result: None,
        error: None,
    };
    Ok(StructuralContinuationLaunch {
        run_id,
        lineage: StoredStructuralLineage {
            segment: previous_lineage.map_or(2, |lineage| lineage.segment.saturating_add(1)),
            generation_offset: generation,
            evaluated_offset: evaluated,
            unique_offset: unique,
            avoided_offset: avoided,
            elapsed_offset_ms: elapsed,
            segment_generations: additional_generations,
            incumbent,
            map_elites,
        },
        initial_status,
    })
}

fn accumulate_continuation_progress(
    mut progress: GenerationProgress,
    lineage: &StoredStructuralLineage,
) -> GenerationProgress {
    progress.generation = progress
        .generation
        .saturating_add(lineage.generation_offset);
    progress.total_generations = lineage
        .generation_offset
        .saturating_add(lineage.segment_generations);
    progress.evaluated_candidates = progress
        .evaluated_candidates
        .saturating_add(lineage.evaluated_offset);
    progress.unique_evaluations = progress
        .unique_evaluations
        .saturating_add(lineage.unique_offset);
    progress.evaluations_avoided = progress
        .evaluations_avoided
        .saturating_add(lineage.avoided_offset);
    progress.elapsed_ms = progress
        .elapsed_ms
        .saturating_add(lineage.elapsed_offset_ms);
    let elapsed_seconds = progress.elapsed_ms as f64 / 1_000.0;
    progress.candidates_per_second =
        progress.evaluated_candidates as f64 / elapsed_seconds.max(0.001);
    progress.unique_evaluations_per_second =
        progress.unique_evaluations as f64 / elapsed_seconds.max(0.001);
    pin_continuation_incumbent(&mut progress.top, &mut progress.best, &lineage.incumbent);
    progress
}

fn accumulate_continuation_result(
    mut result: RunResult,
    lineage: &StoredStructuralLineage,
) -> RunResult {
    result.generations_completed = result
        .generations_completed
        .saturating_add(lineage.generation_offset);
    result.evaluated_candidates = result
        .evaluated_candidates
        .saturating_add(lineage.evaluated_offset);
    result.unique_evaluations = result
        .unique_evaluations
        .saturating_add(lineage.unique_offset);
    result.evaluations_avoided = result
        .evaluations_avoided
        .saturating_add(lineage.avoided_offset);
    result.elapsed_ms = result.elapsed_ms.saturating_add(lineage.elapsed_offset_ms);
    let mut best = result
        .frontier
        .first()
        .cloned()
        .unwrap_or_else(|| lineage.incumbent.clone());
    pin_continuation_incumbent(&mut result.frontier, &mut best, &lineage.incumbent);
    let mut archive = HashMap::new();
    for cell in lineage.map_elites.iter().chain(result.map_elites.iter()) {
        let replace = archive
            .get(&cell.descriptor)
            .is_none_or(|incumbent: &MapEliteCell| {
                cell.candidate.fitness.score > incumbent.candidate.fitness.score
            });
        if replace {
            archive.insert(cell.descriptor.clone(), cell.clone());
        }
    }
    let mut merged = archive.into_values().collect::<Vec<_>>();
    merged.sort_by(|left, right| {
        right
            .candidate
            .fitness
            .score
            .total_cmp(&left.candidate.fitness.score)
            .then_with(|| left.descriptor.cmp(&right.descriptor))
    });
    merged.truncate(32);
    for (index, cell) in merged.iter_mut().enumerate() {
        cell.candidate.rank = index + 1;
    }
    result.map_elites = merged;
    result
}

fn saved_structural_revision(status: &JobStatus) -> &str {
    status
        .result
        .as_ref()
        .map(|result| result.structural_model_revision.as_str())
        .or_else(|| {
            status
                .progress
                .as_ref()
                .map(|progress| progress.structural_model_revision.as_str())
        })
        .unwrap_or_default()
}

fn migrate_legacy_nonblank_run_to_tune(
    request: &mut RunRequest,
    original_seed: &Deck,
    structural_revision: &str,
) {
    if structural_revision != metagene_core::STRUCTURAL_MODEL_REVISION
        && !original_seed.mainboard.is_empty()
    {
        request.config.evolution_mode = EvolutionMode::Tune;
        request.config.tune_policy.root_deck = Some(original_seed.clone());
        request.config.tune_policy.strategy_retention_floor = 0.70;
        request.config.tune_policy.primary_package_floor = 0.60;
        request.config.tune_policy.maximum_nonland_edit_fraction = 0.40;
    }
}

fn preserve_saved_evolution_identity(
    request: &mut RunRequest,
    stored: &StoredRequest,
    status: &JobStatus,
) {
    let original_seed = stored.seed_deck.clone();
    let legacy_nonblank = saved_structural_revision(status)
        != metagene_core::STRUCTURAL_MODEL_REVISION
        && !original_seed.mainboard.is_empty();
    migrate_legacy_nonblank_run_to_tune(request, &original_seed, saved_structural_revision(status));
    if !legacy_nonblank {
        request.config.evolution_mode = stored.config.evolution_mode.clone();
    }
    if request.config.evolution_mode == EvolutionMode::Tune {
        request.config.tune_policy.root_deck = stored
            .config
            .tune_policy
            .root_deck
            .clone()
            .or(Some(original_seed));
    }
}

#[tauri::command]
fn continue_evolution(
    mut request: RunRequest,
    candidate: RankedDeck,
    source_run_id: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<ContinuedRun, String> {
    let saved = runs::load(&state.data_dir, &source_run_id).map_err(|error| error.to_string())?;
    let mut status: JobStatus =
        serde_json::from_str(&saved.status_json).map_err(|error| error.to_string())?;
    let mut stored: StoredRequest =
        serde_json::from_str(&saved.request_json).map_err(|error| error.to_string())?;
    canonicalize_stored_run_ids(&state.data_dir, &mut stored, &mut status)?;
    preserve_saved_evolution_identity(&mut request, &stored, &status);
    // Continuation may change search depth and weights, but it cannot silently
    // replace the saved run's mode or immutable tuning root with stale UI state.
    if status.state == "running" {
        return Err("Wait for the current run to stop before continuing a candidate.".into());
    }
    if !status_contains_candidate(&status, &candidate) {
        return Err("The selected candidate is not part of that saved run's frontier.".into());
    }
    let completed_generations = status
        .result
        .as_ref()
        .map(|result| result.generations_completed)
        .or_else(|| status.progress.as_ref().map(|progress| progress.generation))
        .unwrap_or(0);
    let additional_generations = request.config.generations;
    let launch = continuation_launch(
        source_run_id.clone(),
        &status,
        &candidate,
        additional_generations,
        stored.structural_lineage.as_ref(),
    )?;
    let request = build_continued_request(request, candidate, completed_generations);
    let request = prepare_evolution_request(&state.data_dir, request)?;
    let launched_request = request.clone();
    let initial_status = launch.initial_status.clone();
    let id = launch_evolution(request, stored.resumed_from, Some(launch), app, &state)?;
    Ok(ContinuedRun {
        id,
        request: launched_request,
        continued_from: source_run_id,
        status: initial_status,
        generations_completed: completed_generations,
    })
}

fn launch_evolution(
    request: RunRequest,
    resumed_from: Option<String>,
    continuation: Option<StructuralContinuationLaunch>,
    app: tauri::AppHandle,
    state: &AppState,
) -> Result<String, String> {
    let job_id = continuation
        .as_ref()
        .map(|launch| launch.run_id.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let cancellation = CancellationToken::default();
    let initial_status = continuation
        .as_ref()
        .map(|launch| launch.initial_status.clone())
        .unwrap_or_else(|| JobStatus {
            id: job_id.clone(),
            state: "running".into(),
            progress: None,
            result: None,
            error: None,
        });
    let mut stored_request = StoredRequest::from(&request);
    stored_request.resumed_from = resumed_from;
    stored_request.structural_lineage = continuation.as_ref().map(|launch| launch.lineage.clone());
    if continuation.is_some() {
        let progress = initial_status.progress.as_ref();
        runs::restart(
            &state.data_dir,
            &job_id,
            &stored_request,
            &initial_status,
            runs::RunMetrics {
                best_score: progress.map(|progress| progress.best.fitness.score),
                evaluated_candidates: progress
                    .map(|progress| progress.evaluated_candidates)
                    .unwrap_or(0),
                elapsed_ms: progress.map(|progress| progress.elapsed_ms).unwrap_or(0),
            },
        )
        .map_err(|error| error.to_string())?;
    } else {
        runs::create(
            &state.data_dir,
            &job_id,
            &request.platform,
            &request.format,
            &stored_request,
            &initial_status,
        )
        .map_err(|error| error.to_string())?;
    }
    state.jobs.lock().map_err(|_| "job store poisoned")?.insert(
        job_id.clone(),
        JobRecord {
            cancellation: cancellation.clone(),
            status: initial_status,
        },
    );
    *state
        .export_pool
        .lock()
        .map_err(|_| "export pool poisoned")? = Some(request.pool.clone());

    let jobs = state.jobs.clone();
    let run_data_dir = state.data_dir.clone();
    let thread_job_id = job_id.clone();
    let structural_lineage = stored_request.structural_lineage.clone();
    std::thread::spawn(move || {
        let mut last_checkpoint = Instant::now();
        let mut last_emit = Instant::now();
        let structural_context = structural_context(&request);
        let result = EvolutionEngine::default().run_with_context(
            &request.seed_deck,
            &request.pool,
            &request.constraints,
            &request.config,
            &cancellation,
            &structural_context,
            |progress| {
                let progress = structural_lineage
                    .as_ref()
                    .map_or(progress.clone(), |lineage| {
                        accumulate_continuation_progress(progress, lineage)
                    });
                let boundary =
                    progress.generation == 1 || progress.generation == progress.total_generations;
                let checkpoint_due = boundary || last_checkpoint.elapsed().as_millis() >= 1_000;
                let emit_due = boundary || last_emit.elapsed().as_millis() >= 100;
                let mut snapshot = None;
                if let Ok(mut records) = jobs.lock()
                    && let Some(record) = records.get_mut(&thread_job_id)
                {
                    record.status.progress = Some(progress.clone());
                    if checkpoint_due {
                        snapshot = Some(record.status.clone());
                    }
                }
                if checkpoint_due && let Some(status) = snapshot {
                    let _ = runs::checkpoint(
                        &run_data_dir,
                        &thread_job_id,
                        &status,
                        Some(progress.best.fitness.score),
                        progress.evaluated_candidates,
                        progress.elapsed_ms,
                    );
                    last_checkpoint = Instant::now();
                }
                if emit_due {
                    let _ = app.emit(
                        "evolution-progress",
                        ProgressEnvelope {
                            job_id: thread_job_id.clone(),
                            progress,
                        },
                    );
                    last_emit = Instant::now();
                }
            },
        );

        let status = match result {
            Ok(result) => {
                let result = structural_lineage
                    .as_ref()
                    .map_or(result.clone(), |lineage| {
                        accumulate_continuation_result(result, lineage)
                    });
                JobStatus {
                    id: thread_job_id.clone(),
                    state: if result.cancelled {
                        "cancelled"
                    } else {
                        "completed"
                    }
                    .into(),
                    progress: jobs
                        .lock()
                        .ok()
                        .and_then(|records| records.get(&thread_job_id)?.status.progress.clone()),
                    result: Some(result),
                    error: None,
                }
            }
            Err(error) => JobStatus {
                id: thread_job_id.clone(),
                state: "failed".into(),
                progress: jobs
                    .lock()
                    .ok()
                    .and_then(|records| records.get(&thread_job_id)?.status.progress.clone()),
                result: None,
                error: Some(error.to_string()),
            },
        };
        if let Ok(mut records) = jobs.lock()
            && let Some(record) = records.get_mut(&thread_job_id)
        {
            record.status = status.clone();
        }
        let best_score = status
            .result
            .as_ref()
            .and_then(|result| result.frontier.first())
            .map(|candidate| candidate.fitness.score)
            .or_else(|| {
                status
                    .progress
                    .as_ref()
                    .map(|progress| progress.best.fitness.score)
            });
        let evaluated = status
            .result
            .as_ref()
            .map(|result| result.evaluated_candidates)
            .or_else(|| {
                status
                    .progress
                    .as_ref()
                    .map(|progress| progress.evaluated_candidates)
            })
            .unwrap_or(0);
        let elapsed = status
            .result
            .as_ref()
            .map(|result| result.elapsed_ms)
            .or_else(|| status.progress.as_ref().map(|progress| progress.elapsed_ms))
            .unwrap_or(0);
        let _ = runs::finish(
            &run_data_dir,
            &thread_job_id,
            &status.state,
            &status,
            best_score,
            evaluated,
            elapsed,
        );
        let _ = app.emit(
            "evolution-complete",
            CompleteEnvelope {
                job_id: thread_job_id,
                status,
            },
        );
    });

    Ok(job_id)
}

fn halving_winner<'a>(
    result: &'a RunResult,
    report: &simulator::SuccessiveHalvingReport,
) -> Option<&'a RankedDeck> {
    let rank = report.winner_rank?;
    result_candidate(result, rank)
}

fn result_candidate(result: &RunResult, rank: usize) -> Option<&RankedDeck> {
    result
        .baseline
        .as_ref()
        .filter(|candidate| candidate.rank == rank)
        .or_else(|| {
            result
                .frontier
                .iter()
                .find(|candidate| candidate.rank == rank)
        })
}

const MINIMUM_INCUMBENT_REPLACEMENT_GAIN: f64 = 0.03;
const MINIMUM_CROSS_ENGINE_REPLACEMENT_GAIN: f64 = 0.05;
const FORGE_INCUMBENT_CROSSCHECK_GAMES: usize = 16;

fn latest_halving_evidence(
    report: &simulator::SuccessiveHalvingReport,
    rank: usize,
) -> Option<&simulator::HalvingCandidateResult> {
    report
        .rounds
        .iter()
        .rev()
        .flat_map(|round| round.results.iter())
        .find(|candidate| candidate.candidate_rank == rank && candidate.error.is_none())
}

fn challenger_demonstrates_improvement(
    challenger: &simulator::HalvingCandidateResult,
    incumbent: &simulator::HalvingCandidateResult,
) -> bool {
    challenger_clears_incumbent_threshold(
        challenger.evidence_score,
        challenger.evidence_lower_bound,
        incumbent.evidence_score,
        incumbent.evidence_lower_bound,
    ) && challenger_preserves_field_floor(
        challenger.worst_matchup_win_rate,
        challenger.worst_matchup_lower_bound,
        incumbent.worst_matchup_win_rate,
        incumbent.worst_matchup_lower_bound,
    )
}

fn challenger_preserves_field_floor(
    challenger_worst: f64,
    challenger_worst_lower: f64,
    incumbent_worst: f64,
    incumbent_worst_lower: f64,
) -> bool {
    challenger_worst + MINIMUM_INCUMBENT_REPLACEMENT_GAIN >= incumbent_worst
        && challenger_worst_lower >= incumbent_worst_lower
}

fn challenger_clears_incumbent_threshold(
    challenger_score: f64,
    challenger_lower_bound: f64,
    incumbent_score: f64,
    incumbent_lower_bound: f64,
) -> bool {
    challenger_score >= incumbent_score + MINIMUM_INCUMBENT_REPLACEMENT_GAIN
        && challenger_lower_bound >= incumbent_lower_bound
}

fn retain_incumbent_without_demonstrated_gain(
    report: &mut simulator::SuccessiveHalvingReport,
    incumbent_rank: Option<usize>,
) {
    let Some(incumbent_rank) = incumbent_rank else {
        return;
    };
    let Some(challenger_rank) = report.winner_rank else {
        report.winner_rank = Some(incumbent_rank);
        return;
    };
    if challenger_rank == incumbent_rank {
        return;
    }
    let demonstrated = latest_halving_evidence(report, challenger_rank)
        .zip(latest_halving_evidence(report, incumbent_rank))
        .is_some_and(|(challenger, incumbent)| {
            challenger_demonstrates_improvement(challenger, incumbent)
        });
    if !demonstrated {
        report.winner_rank = Some(incumbent_rank);
    }
}

fn cross_engine_supports_replacement(
    challenger: &simulator::ValidationEvidenceScore,
    incumbent: &simulator::ValidationEvidenceScore,
) -> bool {
    challenger.observed >= incumbent.observed + MINIMUM_CROSS_ENGINE_REPLACEMENT_GAIN
        && challenger.lower_bound >= incumbent.lower_bound
}

fn forge_candidate_evidence(
    report: &simulator::SimulationReport,
    risk_aversion: f64,
    worst_matchup_weight: f64,
    seat_position_weight: f64,
) -> Option<simulator::ValidationEvidenceScore> {
    let label = report
        .rows
        .iter()
        .find(|row| row.deck.starts_with("Candidate · "))?
        .deck
        .as_str();
    simulator::score_validation_report(
        report,
        label,
        risk_aversion,
        worst_matchup_weight,
        seat_position_weight,
    )
}

const HISTORICAL_CHAMPION_PREFIX: &str = "Historical · ";

fn hybrid_validation_opponents(
    opponents: &[Deck],
    seats: usize,
    commander: bool,
    cycle: usize,
) -> Vec<Deck> {
    if !commander {
        return opponents.to_vec();
    }
    let seats = seats.clamp(2, 6);
    let (historical, current): (Vec<_>, Vec<_>) = opponents
        .iter()
        .partition(|deck| deck.name.starts_with(HISTORICAL_CHAMPION_PREFIX));
    if historical.is_empty() {
        return current.into_iter().take(seats - 1).cloned().collect();
    }
    let mut selected = Vec::with_capacity(seats - 1);
    if seats > 2 && !current.is_empty() {
        let start = cycle.saturating_sub(1) % current.len();
        selected.extend(
            current
                .iter()
                .cycle()
                .skip(start)
                .take((seats - 2).min(current.len()))
                .map(|deck| (*deck).clone()),
        );
    }
    let historical_index = cycle.saturating_sub(1) % historical.len();
    selected.push((*historical[historical_index]).clone());
    if selected.len() < seats - 1 {
        selected.extend(
            historical
                .iter()
                .cycle()
                .skip(historical_index + 1)
                .take(seats - 1 - selected.len())
                .map(|deck| (*deck).clone()),
        );
    }
    selected
}

fn complete_holdout_games(requested: usize, seats: usize, commander: bool) -> usize {
    if !commander {
        return requested.clamp(1, 10_000_000);
    }
    let seats = seats.clamp(2, 6);
    requested
        .clamp(1, 10_000_000)
        .div_ceil(seats)
        .min(10_000_000 / seats)
        .max(1)
        * seats
}

fn same_deck_genome(left: &Deck, right: &Deck) -> bool {
    let canonical = |deck: &Deck| {
        let mut deck = deck.clone();
        deck.name.clear();
        deck.command_zone
            .sort_by(|left, right| left.card_id.0.cmp(&right.card_id.0));
        deck.mainboard
            .sort_by(|left, right| left.card_id.0.cmp(&right.card_id.0));
        deck.sideboard
            .sort_by(|left, right| left.card_id.0.cmp(&right.card_id.0));
        deck
    };
    canonical(left) == canonical(right)
}

fn focus_constructed_evolution_pool_on_field(
    evolution: &mut RunRequest,
    opponents: &[Deck],
    commander: bool,
) -> usize {
    let _ = commander;
    // A manually constrained blank start may intentionally have no field list
    // in the same colors. Keep its already prepared legal/color-filtered pool
    // so structural evolution can create the first challenger; opponents are
    // still used for validation and never become mutation material.
    if evolution.seed_deck.mainboard_count() == 0 {
        return evolution.pool.cards.len();
    }
    let card_names = evolution
        .pool
        .cards
        .iter()
        .map(|card| {
            (
                card.id.clone(),
                (card.name.to_ascii_lowercase(), card.features.is_land),
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    let nonland_names = |deck: &Deck| {
        deck.mainboard
            .iter()
            .filter_map(|entry| card_names.get(&entry.card_id))
            .filter(|(_, is_land)| !is_land)
            .map(|(name, _)| name.clone())
            .collect::<std::collections::HashSet<_>>()
    };
    let seed_nonlands = nonland_names(&evolution.seed_deck);
    let mut trusted = std::collections::HashSet::new();
    let mut include_deck = |deck: &Deck| {
        for entry in deck
            .command_zone
            .iter()
            .chain(deck.mainboard.iter())
            .chain(deck.sideboard.iter())
        {
            trusted.insert(entry.card_id.clone());
        }
    };
    include_deck(&evolution.seed_deck);
    for opponent in opponents {
        let opponent_nonlands = nonland_names(opponent);
        let intersection = seed_nonlands.intersection(&opponent_nonlands).count();
        let union = seed_nonlands.union(&opponent_nonlands).count().max(1);
        let archetype_overlap = intersection as f64 / union as f64;
        if archetype_overlap >= 0.25 {
            include_deck(opponent);
        }
    }
    for locked in &evolution.constraints.locked_cards {
        trusted.insert(locked.card_id.clone());
    }
    evolution
        .pool
        .cards
        .retain(|card| trusted.contains(&card.id));
    evolution.pool.cards.len()
}

fn weights_for_opponents(request: &HybridEvolutionRequest, opponents: &[Deck]) -> Vec<f64> {
    if request.commander {
        return vec![1.0; opponents.len()];
    }
    opponents
        .iter()
        .map(|opponent| {
            request
                .opponents
                .iter()
                .position(|candidate| same_deck_genome(candidate, opponent))
                .and_then(|index| request.opponent_weights.get(index).copied())
                .unwrap_or(1.0)
        })
        .collect()
}

fn simulation_candidate_name(
    candidate: &Deck,
    pool: &CardPool,
    commander: bool,
) -> Result<String, String> {
    if !commander {
        return Ok(format!("Candidate · {}", candidate.name));
    }
    let entry = candidate
        .command_zone
        .first()
        .ok_or_else(|| "Commander holdout candidate has no command zone.".to_string())?;
    pool.cards
        .iter()
        .find(|card| card.id == entry.card_id)
        .map(|card| card.name.clone())
        .ok_or_else(|| "Commander holdout leader is absent from the validation pool.".to_string())
}

fn add_coevolved_adversary(
    request: &mut HybridEvolutionRequest,
    config: &mut StoredHybridConfig,
    contender: &RankedDeck,
    cycle: usize,
) {
    if request
        .opponents
        .iter()
        .any(|opponent| same_deck_genome(opponent, &contender.deck))
    {
        return;
    }
    let maximum = if request.commander { 16 } else { 32 };
    if request.opponents.len() >= maximum {
        return;
    }
    let mut adversary = contender.deck.clone();
    adversary.name = format!(
        "{HISTORICAL_CHAMPION_PREFIX}Co-evolved cycle {cycle} · {}",
        contender.deck.name
    );
    if request.commander {
        request.opponent_weights.push(1.0);
    } else {
        for weight in &mut request.opponent_weights {
            *weight *= 0.9;
        }
        request.opponent_weights.push(0.1);
    }
    request.opponents.push(adversary);
    config.opponents = request.opponents.clone();
    config.opponent_weights = request.opponent_weights.clone();
}

#[tauri::command]
fn start_hybrid_evolution(
    mut request: HybridEvolutionRequest,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let continuation = if let Some(run_id) = request.continue_run_id.clone() {
        if state
            .jobs
            .lock()
            .map_err(|_| "job store poisoned")?
            .get(&run_id)
            .is_some_and(|record| record.status.state == "running")
        {
            return Err("Wait for the current run to stop before continuing it.".into());
        }
        let row = runs::load(&state.data_dir, &run_id).map_err(|error| error.to_string())?;
        let mut saved_status: JobStatus =
            serde_json::from_str(&row.status_json).map_err(|error| error.to_string())?;
        let mut saved_request: StoredRequest =
            serde_json::from_str(&row.request_json).map_err(|error| error.to_string())?;
        canonicalize_stored_run_ids(&state.data_dir, &mut saved_request, &mut saved_status)?;
        preserve_saved_evolution_identity(&mut request.evolution, &saved_request, &saved_status);
        let rank = request
            .continuation_candidate_rank
            .ok_or("Validated continuation requires a selected candidate rank.")?;
        let candidate = status_candidate_by_rank(&saved_status, rank)
            .ok_or("The selected candidate is not part of that saved run.")?;
        let completed_generations = saved_status
            .result
            .as_ref()
            .map(|result| result.generations_completed)
            .or_else(|| {
                saved_status
                    .progress
                    .as_ref()
                    .map(|progress| progress.generation)
            })
            .unwrap_or(0);
        let additional_generations = request.evolution.config.generations;
        let launch = continuation_launch(
            run_id,
            &saved_status,
            &candidate,
            additional_generations,
            saved_request.structural_lineage.as_ref(),
        )?;
        request.evolution =
            build_continued_request(request.evolution, candidate.clone(), completed_generations);
        let prior_checkpoint = saved_request
            .hybrid
            .as_ref()
            .map(|hybrid| &hybrid.checkpoint);
        let prior_cycles = prior_checkpoint.map_or(0, |checkpoint| checkpoint.cycles_completed);
        request.cycles = prior_cycles.saturating_add(request.cycles.clamp(1, 20));
        request.resume = Some(HybridResumeState {
            start_cycle: prior_cycles.saturating_add(1),
            cycles_completed: prior_cycles,
            phase: "structural".into(),
            completed_attempts: prior_checkpoint
                .map_or(0, |checkpoint| checkpoint.completed_attempts),
            total_evaluated: launch
                .lineage
                .evaluated_offset
                .max(prior_checkpoint.map_or(0, |checkpoint| checkpoint.total_evaluated)),
            winner_rank: Some(candidate.rank),
            first_cycle_generations: Some(additional_generations),
            first_cycle_seed: Some(request.evolution.config.seed),
            initial_result: None,
            generation_offset: 0,
            structural_lineage: Some(launch.lineage.clone()),
        });
        request.resumed_from = saved_request.resumed_from;
        request.persisted_config = None;
        Some(launch)
    } else {
        None
    };
    request.evolution = prepare_evolution_request(&state.data_dir, request.evolution)?;
    add_tournament_cooccurrence_packages(
        &mut request.evolution.config,
        &request.evolution.pool,
        &request.opponents,
    );
    let validation_pool = request.evolution.pool.clone();
    request.cycles = request.cycles.clamp(1, 20);
    request.finalists = request.finalists.clamp(2, 16);
    request.rounds = request.rounds.clamp(1, 8);
    request.seats = request.seats.clamp(2, 6);
    if !request.commander {
        request.seats = 2;
    }
    request.base_games = request.base_games.clamp(1, 10_000_000);
    request.workers = request.workers.clamp(1, 256);
    request.max_turns = request.max_turns.clamp(1, 1_000);
    request.game_timeout_seconds = request.game_timeout_seconds.clamp(1, 3_600);
    request.structural_weight = request.structural_weight.clamp(0.0, 1.0);
    request.risk_aversion = request.risk_aversion.clamp(0.0, 1.0);
    request.worst_matchup_weight = request.worst_matchup_weight.clamp(0.0, 1.0);
    request.seat_position_weight = request.seat_position_weight.clamp(0.0, 1.0);
    request.holdout_games = request.holdout_games.clamp(1, 10_000_000);
    if request.learn_packages_from_hall {
        let discovered = hall_of_fame::discover_packages(
            &state.data_dir,
            &request.evolution.platform,
            &request.evolution.format,
            request.commander,
            &request.evolution.pool,
            32,
        )
        .map_err(|error| error.to_string())?;
        for package in discovered {
            if !request.evolution.config.learned_packages.contains(&package) {
                request.evolution.config.learned_packages.push(package);
            }
        }
    }
    let required_opponents = if request.commander {
        request.seats - 1
    } else {
        1
    };
    if request.opponents.len() < required_opponents {
        return Err(format!(
            "Hybrid evolution requires {} opponent deck{} for {} seats.",
            required_opponents,
            if required_opponents == 1 { "" } else { "s" },
            request.seats
        ));
    }
    if request.commander && request.opponents.len() > 5 {
        for chunk in request.opponents.chunks(5) {
            validate_gauntlet_decks(
                &state.data_dir,
                &request.evolution.platform,
                &request.evolution.format,
                true,
                chunk,
            )?;
        }
    } else {
        validate_gauntlet_decks(
            &state.data_dir,
            &request.evolution.platform,
            &request.evolution.format,
            request.commander,
            &request.opponents,
        )?;
    }
    request.opponent_weights = if request.commander {
        vec![1.0; request.opponents.len()]
    } else {
        normalize_opponent_weights(&request.opponent_weights, request.opponents.len())?
    };
    if request.evolution.config.evolution_mode == EvolutionMode::Tune {
        // Structural evidence screens entrants, but it never promotes a deck.
        // Tune promotion is determined exclusively by paired game evidence.
        request.structural_weight = 0.0;
    }
    request.evolution = ground_prepared_evolution(
        request.evolution,
        &request.opponents,
        &request.opponent_weights,
        request.commander,
    )?;
    let resource_budgets =
        resource_governor::load(&state.data_dir).map_err(|error| error.to_string())?;
    resource_governor::enforce(&state.data_dir, &resource_budgets)
        .map_err(|error| error.to_string())?;
    request.workers = request
        .workers
        .min(resource_budgets.cpu_threads)
        .min(resource_budgets.max_engine_processes)
        .max(1);
    request.evolution.config.worker_threads = Some(
        request
            .evolution
            .config
            .worker_threads
            .unwrap_or(resource_budgets.cpu_threads)
            .min(resource_budgets.cpu_threads)
            .max(1),
    );
    let calibration_before = calibration::status(
        &state.data_dir,
        &request.evolution.platform,
        &request.evolution.format,
        request.commander,
        evaluation_splits::target_interval_coverage(),
    )
    .map_err(|error| error.to_string())?;
    let calibration_decision = calibration::decision(
        &calibration_before,
        request.risk_aversion,
        request.adaptive_elimination,
    );
    request.risk_aversion = calibration_decision.effective_risk_aversion;
    request.adaptive_elimination = calibration_decision.adaptive_elimination;
    let hexdek_ready = simulator::statuses(&state.data_dir)
        .into_iter()
        .any(|status| status.id == "hexdek" && status.installed && status.data_ready);
    let forge_ready = forge::status(&state.data_dir).data_ready;
    if !hexdek_ready && !forge_ready {
        return Err(
            "Hybrid evolution requires at least one installed, data-ready rules engine.".into(),
        );
    }

    let job_id = continuation
        .as_ref()
        .map(|launch| launch.run_id.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let cancellation = CancellationToken::default();
    let initial_status = continuation
        .as_ref()
        .map(|launch| launch.initial_status.clone())
        .unwrap_or_else(|| JobStatus {
            id: job_id.clone(),
            state: "running".into(),
            progress: None,
            result: None,
            error: None,
        });
    let mut hybrid_config = request
        .persisted_config
        .clone()
        .unwrap_or_else(|| StoredHybridConfig::from_request(&request));
    let initial_checkpoint = request
        .resume
        .as_ref()
        .map(|resume| StoredHybridCheckpoint {
            current_cycle: resume.start_cycle,
            cycles_completed: resume.cycles_completed,
            phase: resume.phase.clone(),
            completed_attempts: resume.completed_attempts,
            total_evaluated: resume.total_evaluated,
            winner_rank: resume.winner_rank,
            structural_generations_completed: resume.generation_offset,
        })
        .unwrap_or(StoredHybridCheckpoint {
            current_cycle: 1,
            cycles_completed: 0,
            phase: "structural".into(),
            completed_attempts: 0,
            total_evaluated: 0,
            winner_rank: None,
            structural_generations_completed: 0,
        });
    let stored_request = stored_hybrid_request(
        &request.evolution,
        &hybrid_config,
        initial_checkpoint,
        request.resumed_from.clone(),
        continuation.as_ref().map(|launch| launch.lineage.clone()),
    );
    if continuation.is_some() {
        runs::restart(
            &state.data_dir,
            &job_id,
            &stored_request,
            &initial_status,
            runs::RunMetrics {
                best_score: initial_status
                    .progress
                    .as_ref()
                    .map(|progress| progress.best.fitness.score),
                evaluated_candidates: initial_status
                    .progress
                    .as_ref()
                    .map_or(0, |progress| progress.evaluated_candidates),
                elapsed_ms: initial_status
                    .progress
                    .as_ref()
                    .map_or(0, |progress| progress.elapsed_ms),
            },
        )
    } else {
        runs::create(
            &state.data_dir,
            &job_id,
            &request.evolution.platform,
            &request.evolution.format,
            &stored_request,
            &initial_status,
        )
    }
    .map_err(|error| error.to_string())?;
    state.jobs.lock().map_err(|_| "job store poisoned")?.insert(
        job_id.clone(),
        JobRecord {
            cancellation: cancellation.clone(),
            status: initial_status,
        },
    );
    *state
        .export_pool
        .lock()
        .map_err(|_| "export pool poisoned")? = Some(validation_pool.clone());

    let jobs = state.jobs.clone();
    let data_dir = state.data_dir.clone();
    let thread_job_id = job_id.clone();
    let continuation_lineage = continuation.map(|launch| launch.lineage);
    std::thread::spawn(move || {
        let started = Instant::now();
        let total_cycles = hybrid_config.cycles;
        let base_seed = hybrid_config.base_seed;
        let base_name = hybrid_config.base_name.clone();
        let mut resume = request.resume.take();
        let start_cycle = resume.as_ref().map_or(1, |state| state.start_cycle);
        let mut total_evaluated = resume.as_ref().map_or(0, |state| state.total_evaluated);
        let mut total_attempts = resume.as_ref().map_or(0, |state| state.completed_attempts);
        let mut cycles_completed = resume.as_ref().map_or(0, |state| state.cycles_completed);
        let mut winner_rank = resume.as_ref().and_then(|state| state.winner_rank);
        let mut current_cycle = start_cycle;
        let mut current_phase = resume
            .as_ref()
            .map_or_else(|| "structural".to_string(), |state| state.phase.clone());
        let mut current_generation_offset =
            resume.as_ref().map_or(0, |state| state.generation_offset);
        let mut durable_attempts = total_attempts;
        let mut last_result = None;
        let mut last_structural_checkpoint = Instant::now();
        let mut last_structural_emit = Instant::now();
        let mut cumulative_lineage = continuation_lineage.or_else(|| {
            resume
                .as_ref()
                .and_then(|state| state.structural_lineage.clone())
        });

        let outcome: Result<(), String> = (|| {
            for cycle in start_cycle..=total_cycles {
                if started.elapsed().as_secs() >= resource_budgets.run_time_seconds {
                    return Err(format!(
                        "Configured run-time budget of {} seconds was exhausted before cycle {}.",
                        resource_budgets.run_time_seconds, cycle
                    ));
                }
                resource_governor::enforce(&data_dir, &resource_budgets)
                    .map_err(|error| error.to_string())?;
                if cancellation.is_cancelled() {
                    break;
                }
                let first_resumed_cycle = cycle == start_cycle;
                let resume_phase = resume
                    .as_ref()
                    .filter(|_| first_resumed_cycle)
                    .map_or("structural", |state| state.phase.as_str());
                let generation_offset = resume
                    .as_ref()
                    .filter(|_| first_resumed_cycle)
                    .map_or(0, |state| state.generation_offset);
                current_cycle = cycle;
                current_phase = resume_phase.to_string();
                current_generation_offset = generation_offset;
                let cycle_root_seed = base_seed.wrapping_add(
                    (cycle.saturating_sub(1) as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15),
                );
                let evidence_split =
                    evaluation_splits::plan(&request.opponents, request.commander, cycle_root_seed);
                request.evolution.config.seed = resume
                    .as_ref()
                    .filter(|_| first_resumed_cycle)
                    .and_then(|state| state.first_cycle_seed)
                    .unwrap_or(evidence_split.train_seed);
                request.evolution.config.generations = resume
                    .as_ref()
                    .filter(|_| first_resumed_cycle && resume_phase == "structural")
                    .and_then(|state| state.first_cycle_generations)
                    .unwrap_or(hybrid_config.generations_per_cycle);
                let evaluated_before_cycle = total_evaluated;
                let ran_structural = resume_phase != "validation";
                let cycle_lineage = cumulative_lineage.clone();
                let mut result = if ran_structural {
                    let _ = app.emit(
                        "hybrid-progress",
                        HybridProgressEnvelope {
                            job_id: thread_job_id.clone(),
                            cycle,
                            total_cycles,
                            phase: "structural".into(),
                            generation: None,
                            halving: None,
                            winner_rank,
                            completed_attempts: total_attempts,
                        },
                    );
                    let structural_context = structural_context(&request.evolution);
                    EvolutionEngine::default()
                        .run_with_context(
                            &request.evolution.seed_deck,
                            &request.evolution.pool,
                            &request.evolution.constraints,
                            &request.evolution.config,
                            &cancellation,
                            &structural_context,
                            |raw_progress| {
                                let segment_generation = raw_progress.generation;
                                let progress = cycle_lineage.as_ref().map_or_else(
                                    || raw_progress.clone(),
                                    |lineage| {
                                        accumulate_continuation_progress(
                                            raw_progress.clone(),
                                            lineage,
                                        )
                                    },
                                );
                                let boundary = raw_progress.generation == 1
                                    || raw_progress.generation == raw_progress.total_generations;
                                let checkpoint_due = boundary
                                    || last_structural_checkpoint.elapsed().as_millis() >= 1_000;
                                let emit_due =
                                    boundary || last_structural_emit.elapsed().as_millis() >= 100;
                                let mut snapshot = None;
                                if let Ok(mut records) = jobs.lock()
                                    && let Some(record) = records.get_mut(&thread_job_id)
                                {
                                    record.status.progress = Some(progress.clone());
                                    if checkpoint_due {
                                        snapshot = Some(record.status.clone());
                                    }
                                }
                                if checkpoint_due && let Some(status) = snapshot {
                                    let checkpoint = StoredHybridCheckpoint {
                                        current_cycle: cycle,
                                        cycles_completed,
                                        phase: "structural".into(),
                                        completed_attempts: total_attempts,
                                        total_evaluated: evaluated_before_cycle
                                            .saturating_add(raw_progress.evaluated_candidates),
                                        winner_rank,
                                        structural_generations_completed: generation_offset
                                            .saturating_add(segment_generation),
                                    };
                                    let stored = stored_hybrid_request(
                                        &request.evolution,
                                        &hybrid_config,
                                        checkpoint,
                                        request.resumed_from.clone(),
                                        cycle_lineage.clone(),
                                    );
                                    let _ = runs::checkpoint_with_request(
                                        &data_dir,
                                        &thread_job_id,
                                        &stored,
                                        &status,
                                        runs::RunMetrics {
                                            best_score: Some(progress.best.fitness.score),
                                            evaluated_candidates: evaluated_before_cycle
                                                .saturating_add(raw_progress.evaluated_candidates),
                                            elapsed_ms: progress.elapsed_ms,
                                        },
                                    );
                                    last_structural_checkpoint = Instant::now();
                                }
                                if emit_due {
                                    let _ = app.emit(
                                        "evolution-progress",
                                        ProgressEnvelope {
                                            job_id: thread_job_id.clone(),
                                            progress: progress.clone(),
                                        },
                                    );
                                    let _ = app.emit(
                                        "hybrid-progress",
                                        HybridProgressEnvelope {
                                            job_id: thread_job_id.clone(),
                                            cycle,
                                            total_cycles,
                                            phase: "structural".into(),
                                            generation: Some(progress),
                                            halving: None,
                                            winner_rank,
                                            completed_attempts: total_attempts,
                                        },
                                    );
                                    last_structural_emit = Instant::now();
                                }
                            },
                        )
                        .map_err(|error| error.to_string())?
                } else {
                    resume
                        .as_mut()
                        .and_then(|state| state.initial_result.take())
                        .ok_or_else(|| {
                            "The hybrid validation checkpoint has no persisted frontier."
                                .to_string()
                        })?
                };
                request.evolution.config.generations = hybrid_config.generations_per_cycle;
                if ran_structural {
                    total_evaluated = total_evaluated.saturating_add(result.evaluated_candidates);
                    if let Some(lineage) = cycle_lineage.as_ref() {
                        result = accumulate_continuation_result(result, lineage);
                    }
                }
                if result.cancelled || cancellation.is_cancelled() {
                    last_result = Some(result);
                    break;
                }

                let baseline_rank = ensure_constructed_seed_baseline(
                    &mut result,
                    &request.evolution,
                    evidence_split.train_seed,
                )?;
                let mut candidates = result
                    .frontier
                    .iter()
                    .filter(|candidate| Some(candidate.rank) != baseline_rank)
                    .take(
                        request
                            .finalists
                            .saturating_sub(usize::from(baseline_rank.is_some())),
                    )
                    .cloned()
                    .collect::<Vec<_>>();
                if let Some(rank) = baseline_rank
                    && let Some(baseline) = result_candidate(&result, rank)
                {
                    candidates.push(baseline.clone());
                }
                if candidates.len() < 2 {
                    return Err("Hybrid evolution produced fewer than two finalists.".into());
                }
                let surrogate_context = surrogate::ScreeningContext {
                    platform: &request.evolution.platform,
                    format: &request.evolution.format,
                    commander: request.commander,
                    active_run_id: &thread_job_id,
                    field_revision: request
                        .field_revision_id
                        .as_deref()
                        .unwrap_or("unversioned-field"),
                    opponent_count: request.opponents.len(),
                    field_coverage: if request.competitive_claims_allowed {
                        1.0
                    } else {
                        0.0
                    },
                };
                let (candidates, surrogate_screening) =
                    surrogate::screen(&data_dir, &surrogate_context, &candidates)
                        .map_err(|error| error.to_string())?;
                let validation_snapshot = {
                    let mut records = jobs
                        .lock()
                        .map_err(|_| "Hybrid job store is unavailable.".to_string())?;
                    let record = records
                        .get_mut(&thread_job_id)
                        .ok_or_else(|| "Hybrid job record disappeared.".to_string())?;
                    record.status.result = Some(result.clone());
                    record.status.clone()
                };
                let validation_checkpoint = StoredHybridCheckpoint {
                    current_cycle: cycle,
                    cycles_completed,
                    phase: "validation".into(),
                    completed_attempts: total_attempts,
                    total_evaluated,
                    winner_rank,
                    structural_generations_completed: hybrid_config.generations_per_cycle,
                };
                let stored = stored_hybrid_request(
                    &request.evolution,
                    &hybrid_config,
                    validation_checkpoint,
                    request.resumed_from.clone(),
                    cycle_lineage.clone(),
                );
                runs::checkpoint_with_request(
                    &data_dir,
                    &thread_job_id,
                    &stored,
                    &validation_snapshot,
                    runs::RunMetrics {
                        best_score: result
                            .frontier
                            .first()
                            .map(|candidate| candidate.fitness.score),
                        evaluated_candidates: total_evaluated,
                        elapsed_ms: result.elapsed_ms,
                    },
                )
                .map_err(|error| error.to_string())?;
                last_result = Some(result.clone());
                current_phase = "validation".into();

                let validation_opponents = hybrid_validation_opponents(
                    &evidence_split.validation_opponents,
                    request.seats,
                    request.commander,
                    cycle,
                );
                let validation_weights = weights_for_opponents(&request, &validation_opponents);
                let comparison_decks = candidates
                    .iter()
                    .map(|candidate| candidate.deck.clone())
                    .collect::<Vec<_>>();
                let selection_route_plan = competitive::build_comparison_engine_route_plan(
                    &data_dir,
                    &comparison_decks,
                    &validation_opponents,
                    &validation_weights,
                    &validation_pool,
                    request.competitive_claims_allowed,
                    request.commander,
                );
                let selection_route_promotable = selection_route_plan.claim.promotable;
                let halving_request = simulator::SuccessiveHalvingRequest {
                    candidates,
                    opponent_weights: validation_weights,
                    opponents: validation_opponents,
                    pool: validation_pool.clone(),
                    base_games: request.base_games,
                    rounds: request.rounds,
                    seed: evidence_split.validation_seed as i64,
                    workers: request.workers,
                    seats: request.seats,
                    hat_budget: request.hat_budget,
                    max_turns: request.max_turns,
                    game_timeout_seconds: request.game_timeout_seconds,
                    structural_weight: request.structural_weight,
                    risk_aversion: request.risk_aversion,
                    worst_matchup_weight: request.worst_matchup_weight,
                    seat_position_weight: request.seat_position_weight,
                    adaptive_elimination: request.adaptive_elimination,
                    commander: request.commander,
                    route_plan: Some(selection_route_plan.clone()),
                };
                let _ = app.emit(
                    "hybrid-progress",
                    HybridProgressEnvelope {
                        job_id: thread_job_id.clone(),
                        cycle,
                        total_cycles,
                        phase: "validation".into(),
                        generation: None,
                        halving: None,
                        winner_rank,
                        completed_attempts: total_attempts,
                    },
                );
                let attempts_before_cycle = total_attempts;
                let tune_context = structural_context(&request.evolution);
                for candidate in &halving_request.candidates {
                    require_structural_validation(
                        &candidate.deck,
                        &halving_request.pool,
                        halving_request.commander,
                        halving_request.seed,
                        halving_request.max_turns,
                    )?;
                    require_tune_validation(
                        &candidate.deck,
                        &halving_request.pool,
                        &request.evolution.config,
                        &tune_context,
                        halving_request.seed,
                        halving_request.max_turns,
                    )?;
                }
                let mut halving = simulator::run_successive_halving_cancellable(
                    &data_dir,
                    &halving_request,
                    &cancellation,
                    |progress| {
                        let _ = app.emit("halving-progress", progress.clone());
                        let _ = app.emit(
                            "hybrid-progress",
                            HybridProgressEnvelope {
                                job_id: thread_job_id.clone(),
                                cycle,
                                total_cycles,
                                phase: "validation".into(),
                                generation: None,
                                completed_attempts: attempts_before_cycle
                                    .saturating_add(progress.completed_attempts),
                                halving: Some(progress),
                                winner_rank,
                            },
                        );
                    },
                )
                .map_err(|error| error.to_string())?;
                let leading_rank = halving.winner_rank;
                retain_incumbent_without_demonstrated_gain(&mut halving, baseline_rank);
                total_attempts = total_attempts.saturating_add(halving.total_attempts);
                let forge_available = !request.commander && forge::status(&data_dir).data_ready;
                let mut forge_gain = None;
                let mut forge_proof_failed = false;
                if !request.commander
                    && forge_available
                    && let (Some(selected_rank), Some(incumbent_rank)) =
                        (halving.winner_rank, baseline_rank)
                    && selected_rank != incumbent_rank
                    && let (Some(selected), Some(incumbent)) = (
                        result_candidate(&result, selected_rank),
                        result_candidate(&result, incumbent_rank),
                    )
                {
                    let forge_request =
                        |candidate: Deck, seed_offset: i64| simulator::HexdekValidationRequest {
                            candidate,
                            opponent_weights: halving_request.opponent_weights.clone(),
                            opponents: halving_request.opponents.clone(),
                            pool: halving_request.pool.clone(),
                            games: FORGE_INCUMBENT_CROSSCHECK_GAMES,
                            seed: halving_request.seed.wrapping_add(seed_offset),
                            workers: request.workers.min(4),
                            seats: 2,
                            hat_budget: 0,
                            max_turns: request.max_turns,
                            game_timeout_seconds: 2,
                            commander: false,
                            fixed_order: false,
                        };
                    let selected_request = forge_request(selected.deck.clone(), 0xF043_0001_i64);
                    let incumbent_request = forge_request(incumbent.deck.clone(), 0xF043_0001_i64);
                    let selected_report = forge::run_validation_cancellable(
                        &data_dir,
                        &selected_request,
                        &cancellation,
                    );
                    let incumbent_report = forge::run_validation_cancellable(
                        &data_dir,
                        &incumbent_request,
                        &cancellation,
                    );
                    if let (Ok(selected_report), Ok(incumbent_report)) =
                        (selected_report, incumbent_report)
                    {
                        total_attempts = total_attempts
                            .saturating_add(selected_report.games)
                            .saturating_add(selected_report.crashes)
                            .saturating_add(incumbent_report.games)
                            .saturating_add(incumbent_report.crashes);
                        let selected_score = forge_candidate_evidence(
                            &selected_report,
                            request.risk_aversion,
                            request.worst_matchup_weight,
                            request.seat_position_weight,
                        );
                        let incumbent_score = forge_candidate_evidence(
                            &incumbent_report,
                            request.risk_aversion,
                            request.worst_matchup_weight,
                            request.seat_position_weight,
                        );
                        forge_gain = selected_score
                            .as_ref()
                            .zip(incumbent_score.as_ref())
                            .map(|(selected, incumbent)| selected.observed - incumbent.observed);
                        if !selected_score
                            .as_ref()
                            .zip(incumbent_score.as_ref())
                            .is_some_and(|(selected, incumbent)| {
                                cross_engine_supports_replacement(selected, incumbent)
                            })
                        {
                            halving.winner_rank = Some(incumbent_rank);
                        }
                        runs::save_validation(
                            &data_dir,
                            Some(&thread_job_id),
                            Some(selected_rank),
                            &serde_json::json!({
                                "phase": "forge_incumbent_crosscheck_challenger",
                                "cycle": cycle,
                                "candidate": &selected_request.candidate,
                                "opponents": &selected_request.opponents,
                            }),
                            &selected_report,
                        )
                        .map_err(|error| error.to_string())?;
                        runs::save_validation(
                            &data_dir,
                            Some(&thread_job_id),
                            Some(incumbent_rank),
                            &serde_json::json!({
                                "phase": "forge_incumbent_crosscheck_baseline",
                                "cycle": cycle,
                                "candidate": &incumbent_request.candidate,
                                "opponents": &incumbent_request.opponents,
                            }),
                            &incumbent_report,
                        )
                        .map_err(|error| error.to_string())?;
                    } else {
                        // An installed engine that cannot execute both decks is
                        // failed proof, not permission to promote on one engine.
                        forge_proof_failed = true;
                        halving.winner_rank = Some(incumbent_rank);
                    }
                }
                persist_halving_evidence(
                    &data_dir,
                    Some(&thread_job_id),
                    &halving_request,
                    &halving,
                    "hybrid_evolution",
                    Some(cycle),
                )?;
                if halving.cancelled || cancellation.is_cancelled() {
                    last_result = Some(result);
                    break;
                }
                durable_attempts = total_attempts;
                let winner = halving_winner(&result, &halving).cloned().ok_or_else(|| {
                    "HexDek halving did not produce a surviving finalist.".to_string()
                })?;
                let winner_selection = halving
                    .rounds
                    .iter()
                    .rev()
                    .flat_map(|round| round.results.iter())
                    .find(|candidate| candidate.candidate_rank == winner.rank)
                    .cloned()
                    .ok_or_else(|| "Winner selection evidence is missing.".to_string())?;
                let challenger_selection = halving.rounds.last().and_then(|round| {
                    round
                        .results
                        .iter()
                        .filter(|candidate| candidate.candidate_rank != winner.rank)
                        .max_by(|left, right| left.blended_score.total_cmp(&right.blended_score))
                        .cloned()
                });
                let challenger = challenger_selection.as_ref().and_then(|challenger| {
                    result
                        .frontier
                        .iter()
                        .find(|candidate| candidate.rank == challenger.candidate_rank)
                        .cloned()
                });
                let coevolved_contender = request
                    .coevolve_adversaries
                    .then(|| challenger.clone())
                    .flatten();
                let holdout_opponents = hybrid_validation_opponents(
                    &evidence_split.holdout_opponents,
                    request.seats,
                    request.commander,
                    cycle.saturating_add(10_000),
                );
                let holdout_games =
                    complete_holdout_games(request.holdout_games, request.seats, request.commander);
                let mut holdout_comparison_decks = vec![winner.deck.clone()];
                if let Some(incumbent_rank) = baseline_rank
                    && winner.rank != incumbent_rank
                    && let Some(incumbent) = result_candidate(&result, incumbent_rank)
                {
                    holdout_comparison_decks.push(incumbent.deck.clone());
                }
                if let Some(challenger) = challenger.as_ref()
                    && holdout_comparison_decks
                        .iter()
                        .all(|deck| deck != &challenger.deck)
                {
                    holdout_comparison_decks.push(challenger.deck.clone());
                }
                let holdout_weights = weights_for_opponents(&request, &holdout_opponents);
                let holdout_route_plan = competitive::build_comparison_engine_route_plan(
                    &data_dir,
                    &holdout_comparison_decks,
                    &holdout_opponents,
                    &holdout_weights,
                    &validation_pool,
                    request.competitive_claims_allowed,
                    request.commander,
                );
                let holdout_route_promotable = holdout_route_plan.claim.promotable;
                let holdout_request = simulator::HexdekValidationRequest {
                    candidate: winner.deck.clone(),
                    opponent_weights: holdout_weights,
                    opponents: holdout_opponents,
                    pool: validation_pool.clone(),
                    games: holdout_games,
                    seed: evidence_split.holdout_seed as i64,
                    workers: request.workers,
                    seats: request.seats,
                    hat_budget: request.hat_budget,
                    max_turns: request.max_turns,
                    game_timeout_seconds: request.game_timeout_seconds,
                    commander: request.commander,
                    fixed_order: false,
                };
                let holdout_report = competitive::run_routed_validation_cancellable(
                    &data_dir,
                    &holdout_request,
                    &holdout_route_plan,
                    &cancellation,
                )
                .map_err(|error| error.to_string())?;
                total_attempts = total_attempts
                    .saturating_add(holdout_report.games)
                    .saturating_add(holdout_report.crashes);
                let candidate_name =
                    simulation_candidate_name(&winner.deck, &validation_pool, request.commander)?;
                let holdout_score = simulator::score_validation_report(
                    &holdout_report,
                    &candidate_name,
                    request.risk_aversion,
                    request.worst_matchup_weight,
                    request.seat_position_weight,
                )
                .ok_or_else(|| "Holdout report did not contain candidate evidence.".to_string())?;
                let mut incumbent_holdout_score = None;
                let mut holdout_gain = None;
                let mut holdout_supports_promotion = true;
                if let Some(incumbent_rank) = baseline_rank
                    && winner.rank != incumbent_rank
                    && let Some(incumbent) = result_candidate(&result, incumbent_rank)
                {
                    let mut incumbent_request = holdout_request.clone();
                    incumbent_request.candidate = incumbent.deck.clone();
                    // Identical opponents and seed make the challenger/baseline
                    // holdout a paired comparison instead of two unrelated samples.
                    let incumbent_report = competitive::run_routed_validation_cancellable(
                        &data_dir,
                        &incumbent_request,
                        &holdout_route_plan,
                        &cancellation,
                    )
                    .map_err(|error| error.to_string())?;
                    total_attempts = total_attempts
                        .saturating_add(incumbent_report.games)
                        .saturating_add(incumbent_report.crashes);
                    let incumbent_name = simulation_candidate_name(
                        &incumbent.deck,
                        &validation_pool,
                        request.commander,
                    )?;
                    let score = simulator::score_validation_report(
                        &incumbent_report,
                        &incumbent_name,
                        request.risk_aversion,
                        request.worst_matchup_weight,
                        request.seat_position_weight,
                    )
                    .ok_or_else(|| {
                        "Incumbent holdout report did not contain candidate evidence.".to_string()
                    })?;
                    holdout_gain = Some(holdout_score.observed - score.observed);
                    holdout_supports_promotion = challenger_clears_incumbent_threshold(
                        holdout_score.observed,
                        holdout_score.lower_bound,
                        score.observed,
                        score.lower_bound,
                    );
                    runs::save_validation(
                        &data_dir,
                        Some(&thread_job_id),
                        Some(incumbent.rank),
                        &serde_json::json!({
                            "phase": "holdout_baseline",
                            "cycle": cycle,
                            "split_schema_version": evidence_split.schema_version,
                            "split_policy_id": &evidence_split.policy_id,
                            "holdout_seed": evidence_split.holdout_seed,
                            "candidate": &incumbent.deck,
                            "opponents": &incumbent_request.opponents,
                            "games": incumbent_request.games,
                            "holdout_evidence_score": score.observed,
                            "holdout_lower_bound": score.lower_bound,
                            "paired_with_rank": winner.rank,
                        }),
                        &incumbent_report,
                    )
                    .map_err(|error| error.to_string())?;
                    incumbent_holdout_score = Some(score);
                }
                let holdout_compact_request = serde_json::json!({
                    "phase": "holdout",
                    "cycle": cycle,
                    "split_schema_version": evidence_split.schema_version,
                    "split_policy_id": &evidence_split.policy_id,
                    "opponents_reused": evidence_split.opponents_reused,
                    "train_seed": evidence_split.train_seed,
                    "validation_seed": evidence_split.validation_seed,
                    "holdout_seed": evidence_split.holdout_seed,
                    "candidate": &winner.deck,
                    "opponents": &holdout_request.opponents,
                    "opponent_weights": &holdout_request.opponent_weights,
                    "games": holdout_request.games,
                    "selection_evidence_score": winner_selection.evidence_score,
                    "selection_lower_bound": winner_selection.evidence_lower_bound,
                    "selection_upper_bound": winner_selection.evidence_upper_bound,
                    "holdout_evidence_score": holdout_score.observed,
                    "holdout_lower_bound": holdout_score.lower_bound,
                    "holdout_upper_bound": holdout_score.upper_bound,
                    "paired_incumbent_score": incumbent_holdout_score.as_ref().map(|score| score.observed),
                    "paired_incumbent_lower_bound": incumbent_holdout_score.as_ref().map(|score| score.lower_bound),
                    "holdout_gain": holdout_gain,
                    "promotion_threshold_met": holdout_supports_promotion,
                    "generalization_gap": holdout_score.observed - winner_selection.evidence_score,
                    "calibration_before_run": &calibration_before,
                    "calibration_decision": &calibration_decision,
                    "surrogate_screening": &surrogate_screening,
                    "selection_route_plan": &selection_route_plan,
                    "holdout_route_plan": &holdout_route_plan,
                });
                runs::save_validation(
                    &data_dir,
                    Some(&thread_job_id),
                    Some(winner.rank),
                    &holdout_compact_request,
                    &holdout_report,
                )
                .map_err(|error| error.to_string())?;
                let mut challenger_holdout_score = None;
                let mut challenger_holdout_report = None;
                if let (Some(challenger), Some(challenger_selection)) =
                    (challenger.as_ref(), challenger_selection.as_ref())
                {
                    let mut challenger_request = holdout_request.clone();
                    challenger_request.candidate = challenger.deck.clone();
                    let challenger_report = competitive::run_routed_validation_cancellable(
                        &data_dir,
                        &challenger_request,
                        &holdout_route_plan,
                        &cancellation,
                    )
                    .map_err(|error| error.to_string())?;
                    total_attempts = total_attempts
                        .saturating_add(challenger_report.games)
                        .saturating_add(challenger_report.crashes);
                    let challenger_name = simulation_candidate_name(
                        &challenger.deck,
                        &validation_pool,
                        request.commander,
                    )?;
                    let score = simulator::score_validation_report(
                        &challenger_report,
                        &challenger_name,
                        request.risk_aversion,
                        request.worst_matchup_weight,
                        request.seat_position_weight,
                    )
                    .ok_or_else(|| {
                        "Challenger holdout report did not contain candidate evidence.".to_string()
                    })?;
                    runs::save_validation(
                        &data_dir,
                        Some(&thread_job_id),
                        Some(challenger.rank),
                        &serde_json::json!({
                            "phase": "holdout_challenger",
                            "cycle": cycle,
                            "split_schema_version": evidence_split.schema_version,
                            "split_policy_id": &evidence_split.policy_id,
                            "holdout_seed": evidence_split.holdout_seed,
                            "candidate": &challenger.deck,
                            "opponents": &challenger_request.opponents,
                            "games": challenger_request.games,
                            "selection_evidence_score": challenger_selection.evidence_score,
                            "holdout_evidence_score": score.observed,
                        }),
                        &challenger_report,
                    )
                    .map_err(|error| error.to_string())?;
                    challenger_holdout_score = Some(score.observed);
                    challenger_holdout_report = Some(challenger_report);
                }
                let selection_gain = leading_rank.zip(baseline_rank).and_then(
                    |(challenger_rank, incumbent_rank)| {
                        latest_halving_evidence(&halving, challenger_rank)
                            .zip(latest_halving_evidence(&halving, incumbent_rank))
                            .map(|(challenger, incumbent)| {
                                challenger.evidence_score - incumbent.evidence_score
                            })
                    },
                );
                let incumbent = baseline_rank.and_then(|rank| result_candidate(&result, rank));
                let challenger_won_selection = baseline_rank.is_none_or(|rank| winner.rank != rank);
                let promotion_accepted = challenger_won_selection
                    && holdout_supports_promotion
                    && !forge_proof_failed
                    && request.competitive_claims_allowed
                    && selection_route_promotable
                    && holdout_route_promotable;
                let promoted_winner = if promotion_accepted {
                    winner.clone()
                } else {
                    incumbent.cloned().unwrap_or_else(|| winner.clone())
                };
                let promotion_status = if promotion_accepted {
                    "improved"
                } else if baseline_rank.is_some() {
                    "baseline"
                } else {
                    "validated"
                };
                let promotion_reason = if !request.competitive_claims_allowed {
                    "Baseline retained: the active public-list fallback is provisional guidance and cannot prove an improvement."
                        .to_string()
                } else if !selection_route_promotable || !holdout_route_promotable {
                    format!(
                        "Baseline retained: trusted engine coverage was insufficient (selection {:.1}%, holdout {:.1}% field weight; 95% is required).",
                        selection_route_plan.field_weighted_coverage * 100.0,
                        holdout_route_plan.field_weighted_coverage * 100.0,
                    )
                } else if forge_proof_failed {
                    "Baseline retained: the installed Forge cross-check could not execute both decks cleanly."
                        .to_string()
                } else if !challenger_won_selection {
                    "Baseline retained: no challenger improved HexDek selection evidence by three points with non-worse confidence and matchup floors."
                        .to_string()
                } else if !holdout_supports_promotion {
                    "Baseline retained: the paired 64-game-per-opponent holdout did not improve by three points with a non-worse lower bound."
                        .to_string()
                } else if forge_available {
                    "Improved: HexDek selection and paired holdout thresholds passed, and Forge independently improved by at least five points."
                        .to_string()
                } else {
                    "Improved: HexDek selection and paired holdout thresholds passed; Single-engine validated because Forge is unavailable."
                        .to_string()
                };
                result.promotion_evidence = Some(metagene_core::PromotionEvidence {
                    status: promotion_status.into(),
                    incumbent_rank: baseline_rank,
                    challenger_rank: leading_rank.filter(|rank| Some(*rank) != baseline_rank),
                    promoted_rank: Some(promoted_winner.rank),
                    selection_gain,
                    holdout_gain,
                    forge_gain,
                    single_engine_validated: !forge_available,
                    reason: promotion_reason,
                });
                for candidate in &mut result.frontier {
                    candidate.candidate_status =
                        if candidate.rank == promoted_winner.rank && promotion_accepted {
                            "improved"
                        } else if latest_halving_evidence(&halving, candidate.rank).is_some() {
                            "validated"
                        } else {
                            "provisional"
                        }
                        .into();
                }
                if let Some(baseline) = &mut result.baseline {
                    baseline.candidate_status = "baseline".into();
                }
                last_result = Some(result.clone());
                let lineage_deck = request
                    .evolution
                    .config
                    .tune_policy
                    .root_deck
                    .as_ref()
                    .unwrap_or(&request.evolution.seed_deck);
                let lineage_id =
                    provenance::configuration_hash("surrogate-lineage-v2", lineage_deck)
                        .map_err(|error| error.to_string())?;
                let field_revision = request
                    .field_revision_id
                    .clone()
                    .unwrap_or_else(|| "unversioned-field".into());
                let card_data_revision = provenance::configuration_hash(
                    "surrogate-card-pool-v2",
                    &request.evolution.pool,
                )
                .map_err(|error| error.to_string())?;
                let label_configuration = provenance::configuration_hash(
                    "surrogate-holdout-label-v2",
                    &serde_json::json!({
                        "request": &holdout_request,
                        "route_plan": &holdout_route_plan,
                        "split": &evidence_split,
                    }),
                )
                .map_err(|error| error.to_string())?;
                let expected_holdout_games = if request.commander {
                    holdout_request.games
                } else {
                    holdout_request
                        .games
                        .saturating_mul(holdout_request.opponents.len())
                };
                surrogate::record(
                    &data_dir,
                    surrogate::SurrogateObservation {
                        platform: &request.evolution.platform,
                        format: &request.evolution.format,
                        commander: request.commander,
                        run_id: &thread_job_id,
                        cycle,
                        candidate: &winner,
                        observed: holdout_score.observed,
                        lineage_id: &lineage_id,
                        field_revision: &field_revision,
                        engine_revision: &holdout_report.engine,
                        route_revision: &holdout_route_plan.plan_id,
                        capability_policy_revision: &holdout_route_plan.capability_policy_revision,
                        rules_ir_revision: &holdout_route_plan.rules_ir_revision,
                        card_data_revision: &card_data_revision,
                        configuration_hash: &label_configuration,
                        play_draw: "rotated",
                        games: holdout_report.games,
                        expected_games: expected_holdout_games,
                        field_coverage: holdout_route_plan.field_weighted_coverage,
                        candidate_critical_coverage: holdout_route_plan.candidate_critical_coverage,
                        opponent_count: holdout_request.opponents.len(),
                        completed: holdout_report.games >= expected_holdout_games,
                        holdout: true,
                        crashed: holdout_report.crashes > 0,
                        timed_out: holdout_report
                            .route_failures
                            .iter()
                            .any(|failure| failure.to_ascii_lowercase().contains("timeout")),
                        disputed_critical_mechanic: !holdout_route_promotable,
                        unresolved_critical_clause: !holdout_route_plan
                            .candidate
                            .unresolved_clauses
                            .is_empty(),
                    },
                )
                .map_err(|error| error.to_string())?;
                if let (Some(challenger), Some(score), Some(challenger_report)) = (
                    challenger.as_ref(),
                    challenger_holdout_score,
                    challenger_holdout_report.as_ref(),
                ) {
                    surrogate::record(
                        &data_dir,
                        surrogate::SurrogateObservation {
                            platform: &request.evolution.platform,
                            format: &request.evolution.format,
                            commander: request.commander,
                            run_id: &thread_job_id,
                            cycle,
                            candidate: challenger,
                            observed: score,
                            lineage_id: &lineage_id,
                            field_revision: &field_revision,
                            engine_revision: &challenger_report.engine,
                            route_revision: &holdout_route_plan.plan_id,
                            capability_policy_revision: &holdout_route_plan
                                .capability_policy_revision,
                            rules_ir_revision: &holdout_route_plan.rules_ir_revision,
                            card_data_revision: &card_data_revision,
                            configuration_hash: &label_configuration,
                            play_draw: "rotated",
                            games: challenger_report.games,
                            expected_games: expected_holdout_games,
                            field_coverage: holdout_route_plan.field_weighted_coverage,
                            candidate_critical_coverage: holdout_route_plan
                                .candidate_critical_coverage,
                            opponent_count: holdout_request.opponents.len(),
                            completed: challenger_report.games >= expected_holdout_games,
                            holdout: true,
                            crashed: challenger_report.crashes > 0,
                            timed_out: challenger_report
                                .route_failures
                                .iter()
                                .any(|failure| failure.to_ascii_lowercase().contains("timeout")),
                            disputed_critical_mechanic: !holdout_route_promotable,
                            unresolved_critical_clause: !holdout_route_plan
                                .candidate
                                .unresolved_clauses
                                .is_empty(),
                        },
                    )
                    .map_err(|error| error.to_string())?;
                }
                calibration::record(
                    &data_dir,
                    calibration::CalibrationObservation {
                        platform: &request.evolution.platform,
                        format: &request.evolution.format,
                        commander: request.commander,
                        run_id: &thread_job_id,
                        cycle,
                        selection_score: winner_selection.evidence_score,
                        selection_lower: winner_selection.evidence_lower_bound,
                        selection_upper: winner_selection.evidence_upper_bound,
                        holdout_score: holdout_score.observed,
                        challenger_selection_score: challenger_selection
                            .as_ref()
                            .map(|selection| selection.evidence_score),
                        challenger_holdout_score,
                        games: holdout_report.games,
                    },
                )
                .map_err(|error| error.to_string())?;
                let winner_evidence_score = if promoted_winner.rank == winner.rank {
                    holdout_score.observed
                } else {
                    incumbent_holdout_score
                        .as_ref()
                        .map_or(holdout_score.observed, |score| score.observed)
                };
                hall_of_fame::promote(
                    &data_dir,
                    &promoted_winner,
                    hall_of_fame::HallOfFamePromotion {
                        platform: &request.evolution.platform,
                        format: &request.evolution.format,
                        commander: request.commander,
                        evidence_score: winner_evidence_score,
                        source_run_id: Some(&thread_job_id),
                        source_cycle: Some(cycle),
                    },
                )
                .map_err(|error| error.to_string())?;
                if let Some(contender) = coevolved_contender.as_ref() {
                    add_coevolved_adversary(&mut request, &mut hybrid_config, contender, cycle);
                }
                winner_rank = Some(promoted_winner.rank);
                request.evolution.seed_deck = promoted_winner.deck.clone();
                request.evolution.seed_deck.name =
                    format!("{base_name} · Hybrid cycle {cycle} champion");
                cycles_completed = cycle;
                current_phase = "cycle_complete".into();
                current_generation_offset = hybrid_config.generations_per_cycle;
                cumulative_lineage = Some(StoredStructuralLineage {
                    segment: cycle_lineage
                        .as_ref()
                        .map_or(2, |lineage| lineage.segment.saturating_add(1)),
                    generation_offset: result.generations_completed,
                    evaluated_offset: result.evaluated_candidates,
                    unique_offset: result.unique_evaluations,
                    avoided_offset: result.evaluations_avoided,
                    elapsed_offset_ms: result.elapsed_ms,
                    segment_generations: hybrid_config.generations_per_cycle,
                    incumbent: promoted_winner.clone(),
                    map_elites: result.map_elites.clone(),
                });
                let stored_request = stored_hybrid_request(
                    &request.evolution,
                    &hybrid_config,
                    StoredHybridCheckpoint {
                        current_cycle: cycle,
                        cycles_completed,
                        phase: "cycle_complete".into(),
                        completed_attempts: total_attempts,
                        total_evaluated,
                        winner_rank,
                        structural_generations_completed: hybrid_config.generations_per_cycle,
                    },
                    request.resumed_from.clone(),
                    cumulative_lineage.clone(),
                );
                let cycle_snapshot = jobs
                    .lock()
                    .map_err(|_| "Hybrid job store is unavailable.".to_string())?
                    .get(&thread_job_id)
                    .ok_or_else(|| "Hybrid job record disappeared.".to_string())?
                    .status
                    .clone();
                runs::checkpoint_with_request(
                    &data_dir,
                    &thread_job_id,
                    &stored_request,
                    &cycle_snapshot,
                    runs::RunMetrics {
                        best_score: Some(promoted_winner.fitness.score),
                        evaluated_candidates: total_evaluated,
                        elapsed_ms: result.elapsed_ms,
                    },
                )
                .map_err(|error| error.to_string())?;
                last_result = Some(result);
                let _ = app.emit(
                    "hybrid-progress",
                    HybridProgressEnvelope {
                        job_id: thread_job_id.clone(),
                        cycle,
                        total_cycles,
                        phase: "cycle_complete".into(),
                        generation: None,
                        halving: None,
                        winner_rank,
                        completed_attempts: total_attempts,
                    },
                );
                resume = None;
            }
            Ok(())
        })();

        let cancelled = cancellation.is_cancelled()
            || last_result
                .as_ref()
                .is_some_and(|result: &RunResult| result.cancelled);
        let status = JobStatus {
            id: thread_job_id.clone(),
            state: if outcome.is_err() {
                "failed"
            } else if cancelled {
                "cancelled"
            } else {
                "completed"
            }
            .into(),
            progress: jobs
                .lock()
                .ok()
                .and_then(|records| records.get(&thread_job_id)?.status.progress.clone()),
            result: last_result,
            error: outcome.err(),
        };
        if let Ok(mut records) = jobs.lock()
            && let Some(record) = records.get_mut(&thread_job_id)
        {
            record.status = status.clone();
        }
        let best_score = status
            .result
            .as_ref()
            .and_then(|result| {
                winner_rank
                    .and_then(|rank| {
                        result
                            .frontier
                            .iter()
                            .find(|candidate| candidate.rank == rank)
                    })
                    .or_else(|| result.frontier.first())
            })
            .map(|candidate| candidate.fitness.score);
        let elapsed = status.result.as_ref().map_or_else(
            || started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            |result| result.elapsed_ms,
        );
        let structural_generations_completed = if current_phase == "structural" {
            current_generation_offset.saturating_add(status.progress.as_ref().map_or(
                0,
                |progress| {
                    progress.generation.saturating_sub(
                        cumulative_lineage
                            .as_ref()
                            .map_or(0, |lineage| lineage.generation_offset),
                    )
                },
            ))
        } else {
            hybrid_config.generations_per_cycle
        }
        .min(hybrid_config.generations_per_cycle);
        let final_stored_request = stored_hybrid_request(
            &request.evolution,
            &hybrid_config,
            StoredHybridCheckpoint {
                current_cycle,
                cycles_completed,
                phase: current_phase,
                completed_attempts: durable_attempts,
                total_evaluated,
                winner_rank,
                structural_generations_completed,
            },
            request.resumed_from.clone(),
            cumulative_lineage,
        );
        let _ = runs::finish_with_request(
            &data_dir,
            &thread_job_id,
            &status.state,
            &final_stored_request,
            &status,
            runs::RunMetrics {
                best_score,
                evaluated_candidates: total_evaluated,
                elapsed_ms: elapsed,
            },
        );
        let _ = app.emit(
            "hybrid-complete",
            HybridCompleteEnvelope {
                job_id: thread_job_id,
                status,
                cycles_completed,
                winner_rank,
                total_attempts,
            },
        );
    });

    Ok(job_id)
}

fn seed_blank_constructed_from_field(
    evolution: &mut RunRequest,
    opponents: &[Deck],
    weights: &[f64],
    commander: bool,
) -> Option<String> {
    if evolution.seed_deck.mainboard_count() != 0
        || (commander && !evolution.seed_deck.command_zone.is_empty())
    {
        return None;
    }
    let available = evolution
        .pool
        .cards
        .iter()
        .map(|card| &card.id)
        .collect::<std::collections::HashSet<_>>();
    let (_, source) = opponents
        .iter()
        .enumerate()
        .filter(|(_, deck)| {
            ((!commander && deck.command_zone.is_empty())
                || (commander
                    && deck
                        .command_zone
                        .iter()
                        .map(|entry| usize::from(entry.quantity))
                        .sum::<usize>()
                        >= evolution.constraints.minimum_commanders
                    && deck
                        .command_zone
                        .iter()
                        .map(|entry| usize::from(entry.quantity))
                        .sum::<usize>()
                        <= evolution.constraints.maximum_commanders))
                && deck.mainboard_count() == evolution.constraints.target_mainboard_size
                && deck
                    .command_zone
                    .iter()
                    .chain(deck.mainboard.iter())
                    .all(|entry| available.contains(&entry.card_id))
        })
        .max_by(|(left, _), (right, _)| {
            weights
                .get(*left)
                .copied()
                .unwrap_or(1.0)
                .total_cmp(&weights.get(*right).copied().unwrap_or(1.0))
        })?;
    let source_name = source.name.clone();
    evolution.seed_deck = Deck {
        name: format!("Competitive field seed · {source_name}"),
        command_zone: source.command_zone.clone(),
        mainboard: source.mainboard.clone(),
        sideboard: if evolution.config.evolve_sideboard
            && source.sideboard_count() == evolution.constraints.target_sideboard_size
        {
            source.sideboard.clone()
        } else {
            vec![]
        },
    };
    Some(source_name)
}

fn ground_prepared_evolution(
    mut evolution: RunRequest,
    opponents: &[Deck],
    weights: &[f64],
    commander: bool,
) -> Result<RunRequest, String> {
    evolution.opponent_field = opponent_field_profile(opponents, &evolution.pool);
    seed_blank_constructed_from_field(&mut evolution, opponents, weights, commander);
    if evolution.seed_deck.mainboard_count() == 0
        && evolution.seed_deck.command_zone.is_empty()
        && evolution.config.evolution_mode == EvolutionMode::Rebuild
        && evolution.pool_filter.is_empty()
    {
        return Err(
            "The tournament field has no legal archetype matching the selected platform, format, and deck colors. Metagene stopped instead of starting an ungrounded full-catalog search. Change the color selection, import a seed deck, or choose a focused pool under Advanced."
                .into(),
        );
    }
    if focus_constructed_evolution_pool_on_field(&mut evolution, opponents, commander) == 0 {
        return Err("The competitive field contains no cards in the prepared legal pool.".into());
    }
    Ok(evolution)
}

fn ensure_constructed_seed_baseline(
    result: &mut RunResult,
    evolution: &RunRequest,
    seed: u64,
) -> Result<Option<usize>, String> {
    let baseline = &evolution.seed_deck;
    if baseline.mainboard_count() == 0 {
        return Ok(None);
    }
    let same_genome = |candidate: &RankedDeck| {
        candidate.deck.mainboard == baseline.mainboard
            && candidate.deck.sideboard == baseline.sideboard
            && candidate.deck.command_zone == baseline.command_zone
    };
    if let Some(existing) = result
        .baseline
        .as_ref()
        .filter(|candidate| same_genome(candidate))
    {
        return Ok(Some(existing.rank));
    }
    let context = structural_context(evolution);
    let mut fitness = evaluate_structural_deck_with_context(
        baseline,
        &evolution.pool,
        seed,
        evolution.config.trials_per_candidate,
        evolution.config.turn_horizon,
        &context,
    )
    .map_err(|error| format!("Could not score the competitive seed baseline: {error}"))?;
    apply_fitness_weights(&mut fitness, &evolution.config.fitness_weights);
    if fitness.severe_mana_failure {
        return Ok(None);
    }
    result.frontier.retain(|candidate| !same_genome(candidate));
    result.baseline = Some(RankedDeck {
        rank: 0,
        pareto_rank: result
            .frontier
            .iter()
            .map(|candidate| candidate.pareto_rank)
            .max()
            .unwrap_or_default()
            .saturating_add(1),
        deck: baseline.clone(),
        fitness,
        packages: vec![],
        commander_compatibility: None,
        candidate_status: "baseline".into(),
    });
    Ok(Some(0))
}

#[cfg(test)]
mod request_tests {
    use super::*;

    fn test_card(id: &str) -> Card {
        Card {
            id: metagene_core::CardId(id.into()),
            arena_id: Some(1),
            platforms: vec!["arena".into()],
            name: id.into(),
            set_code: "tst".into(),
            collector_number: "1".into(),
            type_line: "Basic Land".into(),
            oracle_text: String::new(),
            colors: vec![],
            color_identity: vec![],
            image_uri: None,
            legalities: std::collections::BTreeMap::new(),
            features: metagene_core::CardFeatures {
                is_land: true,
                unlimited_copies: true,
                ..Default::default()
            },
        }
    }

    #[test]
    fn required_seed_cards_survive_engine_policy_filters_without_widening_the_pool() {
        let hard_scope_pool = CardPool {
            cards: vec![
                test_card("supported"),
                test_card("nightclub-bouncer"),
                test_card("disputed-copy-card"),
                test_card("unrelated-unsupported"),
            ],
        };
        let policy_filtered_pool = CardPool {
            cards: vec![test_card("supported")],
        };
        let required_ids = [
            metagene_core::CardId("nightclub-bouncer".into()),
            metagene_core::CardId("disputed-copy-card".into()),
        ]
        .into_iter()
        .collect();

        let restored = restore_required_policy_exceptions(
            &hard_scope_pool,
            policy_filtered_pool,
            &required_ids,
        );
        let ids = restored
            .cards
            .iter()
            .map(|card| card.id.0.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            ids,
            vec!["supported", "nightclub-bouncer", "disputed-copy-card"]
        );
        assert!(!ids.contains(&"unrelated-unsupported"));
    }

    #[test]
    fn legacy_nonblank_runs_migrate_to_balanced_tune_but_blank_runs_remain_rebuilds() {
        let original = Deck {
            name: "Original".into(),
            command_zone: vec![],
            mainboard: vec![metagene_core::DeckEntry {
                card_id: metagene_core::CardId("seed".into()),
                quantity: 60,
            }],
            sideboard: vec![],
        };
        let mut request = RunRequest {
            platform: "arena".into(),
            format: "historic".into(),
            simulator_coverage: "all".into(),
            pool_filter: vec![],
            exclude_disputed_cards: true,
            exclude_unsupported_cards: true,
            pool: CardPool { cards: vec![] },
            seed_deck: original.clone(),
            constraints: DeckConstraints::default(),
            config: EvolutionConfig::default(),
            opponent_field: OpponentFieldProfile::default(),
        };
        migrate_legacy_nonblank_run_to_tune(&mut request, &original, "closed-loop-field-v5");
        assert_eq!(request.config.evolution_mode, EvolutionMode::Tune);
        assert_eq!(request.config.tune_policy.root_deck, Some(original.clone()));
        assert_eq!(request.config.tune_policy.strategy_retention_floor, 0.70);
        assert_eq!(request.config.tune_policy.primary_package_floor, 0.60);
        assert_eq!(
            request.config.tune_policy.maximum_nonland_edit_fraction,
            0.40
        );

        let blank = Deck {
            name: "Blank".into(),
            command_zone: vec![],
            mainboard: vec![],
            sideboard: vec![],
        };
        request.config = EvolutionConfig::default();
        migrate_legacy_nonblank_run_to_tune(&mut request, &blank, "closed-loop-field-v5");
        assert_eq!(request.config.evolution_mode, EvolutionMode::Rebuild);
    }

    #[test]
    fn blank_full_catalog_rebuild_requires_a_field_or_explicit_focus() {
        let mut request = RunRequest {
            platform: "arena".into(),
            format: "historic".into(),
            simulator_coverage: "all".into(),
            pool_filter: vec![],
            exclude_disputed_cards: true,
            exclude_unsupported_cards: true,
            pool: CardPool {
                cards: vec![test_card("forest")],
            },
            seed_deck: Deck {
                name: "Blank".into(),
                command_zone: vec![],
                mainboard: vec![],
                sideboard: vec![],
            },
            constraints: DeckConstraints::default(),
            config: EvolutionConfig::default(),
            opponent_field: OpponentFieldProfile::default(),
        };
        assert!(blank_rebuild_requires_grounding(&request));

        request.pool_filter.push("Forest".into());
        assert!(!blank_rebuild_requires_grounding(&request));
        request.pool_filter.clear();
        request.config.learned_packages.push(vec![
            metagene_core::CardId("forest".into()),
            metagene_core::CardId("second".into()),
        ]);
        assert!(!blank_rebuild_requires_grounding(&request));
    }

    #[test]
    fn opponent_field_profile_quantifies_narrow_target_coverage() {
        let mut creature = test_card("creature");
        creature.type_line = "Creature — Test".into();
        creature.features.is_land = false;
        creature.features.unlimited_copies = false;
        let mut artifact = test_card("artifact");
        artifact.type_line = "Artifact".into();
        artifact.features.is_land = false;
        artifact.features.unlimited_copies = false;
        let mut graveyard_spell = test_card("graveyard-spell");
        graveyard_spell.type_line = "Instant".into();
        graveyard_spell.oracle_text =
            "Mill three cards, then return a card from your graveyard.".into();
        graveyard_spell.features.is_land = false;
        graveyard_spell.features.unlimited_copies = false;
        let pool = CardPool {
            cards: vec![creature, artifact, graveyard_spell],
        };
        let field = vec![Deck {
            name: "Measured field".into(),
            command_zone: vec![],
            mainboard: vec![
                metagene_core::DeckEntry {
                    card_id: metagene_core::CardId("creature".into()),
                    quantity: 8,
                },
                metagene_core::DeckEntry {
                    card_id: metagene_core::CardId("artifact".into()),
                    quantity: 4,
                },
                metagene_core::DeckEntry {
                    card_id: metagene_core::CardId("graveyard-spell".into()),
                    quantity: 4,
                },
            ],
            sideboard: vec![],
        }];
        let profile = opponent_field_profile(&field, &pool);
        assert!((profile.creature_rate - 0.50).abs() < 0.001);
        assert!((profile.artifact_or_enchantment_rate - 0.25).abs() < 0.001);
        assert!((profile.graveyard_rate - 0.375).abs() < 0.001);
        assert!((profile.stack_spell_rate - 0.375).abs() < 0.001);
    }

    #[test]
    fn tournament_cooccurrence_guides_rebuild_but_never_leaks_into_tune() {
        let mut first = test_card("field-a");
        first.features.is_land = false;
        let mut second = test_card("field-b");
        second.features.is_land = false;
        let pool = CardPool {
            cards: vec![first, second],
        };
        let opponents = vec![Deck {
            name: "Tournament shell".into(),
            command_zone: Vec::new(),
            mainboard: vec![
                metagene_core::DeckEntry {
                    card_id: metagene_core::CardId("field-a".into()),
                    quantity: 4,
                },
                metagene_core::DeckEntry {
                    card_id: metagene_core::CardId("field-b".into()),
                    quantity: 4,
                },
            ],
            sideboard: Vec::new(),
        }];
        let mut rebuild = EvolutionConfig::default();
        add_tournament_cooccurrence_packages(&mut rebuild, &pool, &opponents);
        assert_eq!(rebuild.learned_packages.len(), 1);

        let mut tune = EvolutionConfig {
            evolution_mode: EvolutionMode::Tune,
            ..EvolutionConfig::default()
        };
        add_tournament_cooccurrence_packages(&mut tune, &pool, &opponents);
        assert!(tune.learned_packages.is_empty());
    }

    #[test]
    fn holdout_budget_is_a_complete_bounded_commander_rotation() {
        assert_eq!(complete_holdout_games(1, 4, true), 4);
        assert_eq!(complete_holdout_games(65, 4, true), 68);
        assert_eq!(complete_holdout_games(10_000_000, 6, true), 9_999_996);
        assert_eq!(complete_holdout_games(0, 2, false), 1);
        assert_eq!(complete_holdout_games(usize::MAX, 2, false), 10_000_000);
    }

    #[test]
    fn incumbent_requires_material_noninferior_evidence_to_be_replaced() {
        assert!(!challenger_clears_incumbent_threshold(
            0.258, 0.19, 0.235, 0.18
        ));
        assert!(!challenger_clears_incumbent_threshold(
            0.30, 0.17, 0.24, 0.18
        ));
        assert!(challenger_clears_incumbent_threshold(
            0.28, 0.19, 0.24, 0.18
        ));
        assert!(!challenger_preserves_field_floor(0.20, 0.12, 0.30, 0.18));
        assert!(challenger_preserves_field_floor(0.28, 0.19, 0.30, 0.18));
    }

    #[test]
    fn independent_engine_must_support_replacing_the_incumbent() {
        let score = |observed, lower_bound| simulator::ValidationEvidenceScore {
            observed,
            lower_bound,
            upper_bound: 1.0,
        };
        assert!(!cross_engine_supports_replacement(
            &score(0.375, 0.14),
            &score(0.875, 0.53)
        ));
        assert!(!cross_engine_supports_replacement(
            &score(0.54, 0.30),
            &score(0.50, 0.29)
        ));
        assert!(cross_engine_supports_replacement(
            &score(0.60, 0.34),
            &score(0.50, 0.33)
        ));
    }

    fn named_deck(name: &str) -> Deck {
        Deck {
            name: name.into(),
            command_zone: Vec::new(),
            mainboard: Vec::new(),
            sideboard: Vec::new(),
        }
    }

    #[test]
    fn commander_hybrid_rotates_current_and_historical_pod_members() {
        let opponents = vec![
            named_deck("Current A"),
            named_deck("Current B"),
            named_deck("Current C"),
            named_deck("Historical · Champion 1"),
            named_deck("Historical · Champion 2"),
        ];
        let first = hybrid_validation_opponents(&opponents, 4, true, 1);
        let second = hybrid_validation_opponents(&opponents, 4, true, 2);
        assert_eq!(first.len(), 3);
        assert_eq!(second.len(), 3);
        assert_eq!(first[0].name, "Current A");
        assert_eq!(first[1].name, "Current B");
        assert_eq!(first[2].name, "Historical · Champion 1");
        assert_eq!(second[0].name, "Current B");
        assert_eq!(second[1].name, "Current C");
        assert_eq!(second[2].name, "Historical · Champion 2");
        assert_eq!(
            hybrid_validation_opponents(&opponents, 2, true, 2)[0].name,
            "Historical · Champion 2"
        );
        assert_eq!(
            hybrid_validation_opponents(&opponents, 2, false, 1).len(),
            5
        );
    }

    #[test]
    fn blank_constructed_hybrid_uses_the_highest_weight_compatible_field_seed() {
        let pool = CardPool {
            cards: vec![test_card("low"), test_card("high"), test_card("junk")],
        };
        let deck = |name: &str, id: &str| Deck {
            name: name.into(),
            command_zone: vec![],
            mainboard: vec![metagene_core::DeckEntry {
                card_id: metagene_core::CardId(id.into()),
                quantity: 60,
            }],
            sideboard: vec![],
        };
        let mut evolution = RunRequest {
            platform: "arena".into(),
            format: "historic".into(),
            simulator_coverage: "parser_complete".into(),
            pool_filter: vec![],
            exclude_disputed_cards: true,
            exclude_unsupported_cards: true,
            pool,
            seed_deck: Deck {
                name: "Blank".into(),
                command_zone: vec![],
                mainboard: vec![],
                sideboard: vec![],
            },
            constraints: DeckConstraints::default(),
            config: EvolutionConfig::default(),
            opponent_field: OpponentFieldProfile::default(),
        };
        let opponents = vec![deck("Low share", "low"), deck("High share", "high")];

        let selected =
            seed_blank_constructed_from_field(&mut evolution, &opponents, &[0.2, 0.8], false);

        assert_eq!(selected.as_deref(), Some("High share"));
        assert_eq!(evolution.seed_deck.mainboard[0].card_id.0, "high");
        assert!(evolution.seed_deck.name.contains("Competitive field seed"));
        assert_eq!(
            focus_constructed_evolution_pool_on_field(&mut evolution, &opponents, false),
            1
        );
        assert_eq!(evolution.pool.cards[0].id.0, "high");

        let mut result = RunResult {
            generations_completed: 1,
            evaluated_candidates: 1,
            unique_evaluations: 1,
            evaluations_avoided: 0,
            elapsed_ms: 1,
            cancelled: false,
            frontier: vec![],
            baseline: None,
            strategy_signature: None,
            promotion_evidence: None,
            structural_model_revision: metagene_core::STRUCTURAL_MODEL_REVISION.into(),
            map_elites: Vec::new(),
        };
        let baseline_rank = ensure_constructed_seed_baseline(&mut result, &evolution, 7).unwrap();
        assert_eq!(baseline_rank, Some(0));
        assert!(result.frontier.is_empty());
        assert_eq!(result.baseline.unwrap().deck.mainboard[0].card_id.0, "high");
    }

    #[test]
    fn field_grounded_structural_fallback_can_start_without_a_rules_engine() {
        let pool = CardPool {
            cards: vec![test_card("seed"), test_card("unrelated")],
        };
        let evolution = RunRequest {
            platform: "arena".into(),
            format: "historic".into(),
            simulator_coverage: "all".into(),
            pool_filter: vec![],
            exclude_disputed_cards: true,
            exclude_unsupported_cards: true,
            pool,
            seed_deck: Deck {
                name: "Blank".into(),
                command_zone: vec![],
                mainboard: vec![],
                sideboard: vec![],
            },
            constraints: DeckConstraints::default(),
            config: EvolutionConfig::default(),
            opponent_field: OpponentFieldProfile::default(),
        };
        let opponents = vec![Deck {
            name: "Tournament archetype".into(),
            command_zone: vec![],
            mainboard: vec![metagene_core::DeckEntry {
                card_id: metagene_core::CardId("seed".into()),
                quantity: 60,
            }],
            sideboard: vec![],
        }];

        let grounded = ground_prepared_evolution(evolution, &opponents, &[1.0], false).unwrap();

        assert!(!blank_rebuild_requires_grounding(&grounded));
        assert_eq!(grounded.seed_deck.mainboard_count(), 60);
        assert!(
            grounded
                .seed_deck
                .name
                .contains("Competitive field seed · Tournament archetype")
        );
        assert!(grounded.opponent_field.available);
        assert_eq!(grounded.pool.cards.len(), 1);
        assert_eq!(grounded.pool.cards[0].id.0, "seed");
    }

    #[test]
    fn imported_tune_pool_excludes_unrelated_field_modules() {
        let pool = CardPool {
            cards: [
                "seed-a",
                "seed-b",
                "related-c",
                "unrelated-x",
                "unrelated-y",
            ]
            .into_iter()
            .map(|id| {
                let mut card = test_card(id);
                card.type_line = "Creature".into();
                card.features.is_land = false;
                card.features.unlimited_copies = false;
                card
            })
            .collect(),
        };
        let deck = |name: &str, ids: &[&str]| Deck {
            name: name.into(),
            command_zone: vec![],
            mainboard: ids
                .iter()
                .map(|id| metagene_core::DeckEntry {
                    card_id: metagene_core::CardId((*id).into()),
                    quantity: 4,
                })
                .collect(),
            sideboard: vec![],
        };
        let mut evolution = RunRequest {
            platform: "arena".into(),
            format: "historic".into(),
            simulator_coverage: "parser_complete".into(),
            pool_filter: vec![],
            exclude_disputed_cards: true,
            exclude_unsupported_cards: true,
            pool,
            seed_deck: deck("Imported seed", &["seed-a", "seed-b"]),
            constraints: DeckConstraints::default(),
            config: EvolutionConfig {
                evolution_mode: EvolutionMode::Tune,
                ..EvolutionConfig::default()
            },
            opponent_field: OpponentFieldProfile::default(),
        };
        let opponents = vec![
            deck("Related", &["seed-a", "related-c"]),
            deck("Unrelated", &["unrelated-x", "unrelated-y"]),
        ];

        assert_eq!(
            focus_constructed_evolution_pool_on_field(&mut evolution, &opponents, false),
            3
        );
        let retained = evolution
            .pool
            .cards
            .iter()
            .map(|card| card.id.0.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(
            retained,
            std::collections::HashSet::from(["seed-a", "seed-b", "related-c"])
        );
    }

    #[test]
    fn exact_color_blank_hybrid_is_identified_as_ungrounded_without_a_compatible_field_seed() {
        let mut evolution = RunRequest {
            platform: "arena".into(),
            format: "historic".into(),
            simulator_coverage: "parser_complete".into(),
            pool_filter: vec![],
            exclude_disputed_cards: true,
            exclude_unsupported_cards: true,
            pool: CardPool {
                cards: vec![test_card("red-a"), test_card("red-b")],
            },
            seed_deck: named_deck("Blank red deck"),
            constraints: DeckConstraints {
                allowed_colors: Some(vec![metagene_core::Color::Red]),
                ..DeckConstraints::default()
            },
            config: EvolutionConfig::default(),
            opponent_field: OpponentFieldProfile::default(),
        };
        let incompatible = Deck {
            name: "Blue field deck".into(),
            command_zone: vec![],
            mainboard: vec![metagene_core::DeckEntry {
                card_id: metagene_core::CardId("blue-only".into()),
                quantity: 60,
            }],
            sideboard: vec![],
        };

        assert_eq!(
            seed_blank_constructed_from_field(
                &mut evolution,
                std::slice::from_ref(&incompatible),
                &[1.0],
                false,
            ),
            None
        );
        assert_eq!(
            focus_constructed_evolution_pool_on_field(
                &mut evolution,
                std::slice::from_ref(&incompatible),
                false,
            ),
            2
        );
        assert!(evolution.seed_deck.mainboard.is_empty());
        assert!(evolution.pool_filter.is_empty());
        assert_eq!(evolution.config.evolution_mode, EvolutionMode::Rebuild);
        assert!(blank_rebuild_requires_grounding(&evolution));
    }

    #[test]
    fn blank_commander_hybrid_uses_a_field_deck_as_its_protected_seed() {
        let pool = CardPool {
            cards: vec![test_card("leader"), test_card("plains")],
        };
        let field = Deck {
            name: "Known Commander field list".into(),
            command_zone: vec![metagene_core::DeckEntry {
                card_id: metagene_core::CardId("leader".into()),
                quantity: 1,
            }],
            mainboard: vec![metagene_core::DeckEntry {
                card_id: metagene_core::CardId("plains".into()),
                quantity: 99,
            }],
            sideboard: vec![],
        };
        let mut evolution = RunRequest {
            platform: "paper".into(),
            format: "commander".into(),
            simulator_coverage: "parser_complete".into(),
            pool_filter: vec![],
            exclude_disputed_cards: true,
            exclude_unsupported_cards: true,
            pool,
            seed_deck: Deck {
                name: "Blank Commander".into(),
                command_zone: vec![],
                mainboard: vec![],
                sideboard: vec![],
            },
            constraints: DeckConstraints {
                target_mainboard_size: 99,
                target_sideboard_size: 0,
                minimum_commanders: 1,
                maximum_commanders: 1,
                default_max_copies: 1,
                locked_cards: vec![],
                minimum_lands: Some(36),
                maximum_lands: Some(44),
                allowed_colors: None,
            },
            config: EvolutionConfig::default(),
            opponent_field: OpponentFieldProfile::default(),
        };

        let selected = seed_blank_constructed_from_field(
            &mut evolution,
            std::slice::from_ref(&field),
            &[1.0],
            true,
        );

        assert_eq!(selected.as_deref(), Some("Known Commander field list"));
        assert_eq!(evolution.seed_deck.command_zone, field.command_zone);
        assert_eq!(evolution.seed_deck.mainboard, field.mainboard);
    }

    #[test]
    fn conformance_exclusions_are_independent_and_do_not_hide_uncertainty() {
        assert!(conformance_classification_excluded("disputed", true, false));
        assert!(!conformance_classification_excluded(
            "unsupported",
            true,
            false
        ));
        assert!(conformance_classification_excluded(
            "unsupported",
            false,
            true
        ));
        assert!(!conformance_classification_excluded(
            "unverified",
            true,
            true
        ));
        assert!(!conformance_classification_excluded(
            "partially_covered",
            true,
            true
        ));
        assert!(!conformance_classification_excluded("verified", true, true));
    }

    #[test]
    fn older_saved_requests_default_to_an_unrestricted_pool() {
        let stored = StoredRequest {
            platform: "arena".into(),
            format: "historic".into(),
            simulator_coverage: "parser_complete".into(),
            pool_filter: vec!["Focus Card".into()],
            exclude_disputed_cards: true,
            exclude_unsupported_cards: true,
            seed_deck: Deck {
                name: "Legacy".into(),
                command_zone: Vec::new(),
                mainboard: Vec::new(),
                sideboard: Vec::new(),
            },
            constraints: DeckConstraints::default(),
            config: EvolutionConfig::default(),
            opponent_field: OpponentFieldProfile::default(),
            resumed_from: None,
            run_kind: "structural".into(),
            hybrid: None,
            structural_lineage: None,
        };
        let mut value = serde_json::to_value(stored).unwrap();
        value.as_object_mut().unwrap().remove("simulator_coverage");
        value.as_object_mut().unwrap().remove("pool_filter");
        value
            .as_object_mut()
            .unwrap()
            .remove("exclude_disputed_cards");
        value
            .as_object_mut()
            .unwrap()
            .remove("exclude_unsupported_cards");
        value.as_object_mut().unwrap().remove("resumed_from");
        value.as_object_mut().unwrap().remove("run_kind");
        value.as_object_mut().unwrap().remove("hybrid");
        value.as_object_mut().unwrap().remove("structural_lineage");
        value["config"]
            .as_object_mut()
            .unwrap()
            .remove("fitness_weights");
        value["config"]
            .as_object_mut()
            .unwrap()
            .remove("package_mutation_rate");
        value["config"]
            .as_object_mut()
            .unwrap()
            .remove("package_size");
        value["config"]
            .as_object_mut()
            .unwrap()
            .remove("mana_mutation_rate");
        value["config"]
            .as_object_mut()
            .unwrap()
            .remove("evolve_sideboard");
        value["constraints"]
            .as_object_mut()
            .unwrap()
            .remove("target_sideboard_size");
        let restored: StoredRequest = serde_json::from_value(value).unwrap();
        assert_eq!(restored.simulator_coverage, "all");
        assert!(restored.pool_filter.is_empty());
        assert!(!restored.exclude_disputed_cards);
        assert!(!restored.exclude_unsupported_cards);
        assert_eq!(restored.config.fitness_weights.consistency, 30.0);
        assert_eq!(
            restored.config.fitness_weights.legendary_dead_draw_penalty,
            12.0
        );
        assert_eq!(
            restored.config.fitness_weights.unsupported_synergy_penalty,
            20.0
        );
        assert_eq!(restored.config.package_mutation_rate, 0.35);
        assert_eq!(restored.config.package_size, 3);
        assert!(!restored.config.evolve_sideboard);
        assert_eq!(restored.constraints.target_sideboard_size, 0);
        assert!(restored.resumed_from.is_none());
        assert_eq!(restored.run_kind, "structural");
        assert!(restored.hybrid.is_none());
    }

    #[test]
    fn normalizes_metagame_weights_and_rejects_invalid_fields() {
        assert_eq!(normalize_opponent_weights(&[], 2).unwrap(), vec![0.5, 0.5]);
        let normalized = normalize_opponent_weights(&[6.0, 3.0, 1.0], 3).unwrap();
        assert!((normalized[0] - 0.6).abs() < f64::EPSILON);
        assert!((normalized.iter().sum::<f64>() - 1.0).abs() < f64::EPSILON);
        assert!(normalize_opponent_weights(&[1.0], 2).is_err());
        assert!(normalize_opponent_weights(&[1.0, 0.0], 2).is_err());
    }

    #[test]
    fn interrupted_checkpoint_retains_candidate_generation_and_advances_seed() {
        let candidate = RankedDeck {
            rank: 1,
            pareto_rank: 0,
            deck: Deck {
                name: "Checkpoint champion".into(),
                command_zone: Vec::new(),
                mainboard: Vec::new(),
                sideboard: Vec::new(),
            },
            fitness: metagene_core::Fitness {
                score: 0.75,
                ..Default::default()
            },
            packages: Vec::new(),
            commander_compatibility: None,
            candidate_status: "provisional".into(),
        };
        let status = JobStatus {
            id: "interrupted".into(),
            state: "interrupted".into(),
            progress: Some(GenerationProgress {
                generation: 7,
                total_generations: 20,
                evaluated_candidates: 700,
                unique_evaluations: 650,
                evaluations_avoided: 50,
                unique_in_generation: 90,
                duplicates_in_generation: 10,
                elapsed_ms: 1_000,
                candidates_per_second: 700.0,
                unique_evaluations_per_second: 650.0,
                best: candidate,
                top: Vec::new(),
                baseline: None,
                strategy_signature: None,
                structural_model_revision: metagene_core::STRUCTURAL_MODEL_REVISION.into(),
                map_elite_cells: 0,
            }),
            result: None,
            error: Some("desktop restarted".into()),
        };

        let (checkpoint, generation) = checkpoint_candidate(&status).unwrap();
        assert_eq!(checkpoint.deck.name, "Checkpoint champion");
        assert_eq!(generation, 7);
        assert_eq!(20_usize.saturating_sub(generation), 13);
        assert_ne!(continued_seed(42, generation), 42);
        assert_eq!(
            continued_seed(42, generation),
            continued_seed(42, generation)
        );
    }

    #[test]
    fn completed_candidate_becomes_a_lineage_seed_for_additional_generations() {
        let candidate = ranked("Finished champion", 1);
        let status = JobStatus {
            id: "complete".into(),
            state: "completed".into(),
            progress: None,
            result: Some(RunResult {
                generations_completed: 50,
                evaluated_candidates: 6_400,
                unique_evaluations: 6_000,
                evaluations_avoided: 400,
                elapsed_ms: 1_000,
                cancelled: false,
                frontier: vec![candidate.clone()],
                baseline: None,
                strategy_signature: None,
                promotion_evidence: None,
                structural_model_revision: metagene_core::STRUCTURAL_MODEL_REVISION.into(),
                map_elites: Vec::new(),
            }),
            error: None,
        };
        let request = RunRequest {
            platform: "arena".into(),
            format: "historic".into(),
            simulator_coverage: "all".into(),
            pool_filter: Vec::new(),
            exclude_disputed_cards: false,
            exclude_unsupported_cards: false,
            pool: CardPool { cards: Vec::new() },
            seed_deck: named_deck("Original seed"),
            constraints: DeckConstraints::default(),
            config: EvolutionConfig {
                generations: 75,
                seed: 42,
                ..EvolutionConfig::default()
            },
            opponent_field: OpponentFieldProfile::default(),
        };

        assert!(status_contains_candidate(&status, &candidate));
        assert!(!status_contains_candidate(&status, &ranked("Other", 2)));
        let continued = build_continued_request(request, candidate, 50);
        assert_eq!(continued.seed_deck.name, "Finished champion · continued");
        assert_eq!(continued.config.generations, 75);
        assert_eq!(continued.config.seed, continued_seed(42, 50));
    }

    #[test]
    fn continuation_keeps_the_same_run_and_accumulates_prior_work() {
        let incumbent = ranked("Validated incumbent", 3);
        let prior_status = JobStatus {
            id: "same-run".into(),
            state: "completed".into(),
            progress: None,
            result: Some(RunResult {
                generations_completed: 2_000,
                evaluated_candidates: 256_000,
                unique_evaluations: 200_000,
                evaluations_avoided: 56_000,
                elapsed_ms: 40_000,
                cancelled: false,
                frontier: vec![ranked("Other finalist", 1), incumbent.clone()],
                baseline: Some(ranked("Immutable root", 0)),
                strategy_signature: None,
                promotion_evidence: None,
                structural_model_revision: metagene_core::STRUCTURAL_MODEL_REVISION.into(),
                map_elites: Vec::new(),
            }),
            error: None,
        };
        let launch =
            continuation_launch("same-run".into(), &prior_status, &incumbent, 500, None).unwrap();
        assert_eq!(launch.run_id, "same-run");
        assert_eq!(launch.lineage.generation_offset, 2_000);
        assert_eq!(launch.lineage.segment_generations, 500);
        let initial = launch.initial_status.progress.as_ref().unwrap();
        assert_eq!(initial.generation, 2_000);
        assert_eq!(initial.total_generations, 2_500);
        assert_eq!(initial.top[0].deck.name, "Validated incumbent");
        assert_eq!(initial.top[0].candidate_status, "incumbent");
        assert_eq!(
            initial.baseline.as_ref().unwrap().deck.name,
            "Immutable root"
        );

        let segment = GenerationProgress {
            generation: 10,
            total_generations: 500,
            evaluated_candidates: 1_280,
            unique_evaluations: 1_000,
            evaluations_avoided: 280,
            unique_in_generation: 100,
            duplicates_in_generation: 28,
            elapsed_ms: 500,
            candidates_per_second: 0.0,
            unique_evaluations_per_second: 0.0,
            best: ranked("New challenger", 1),
            top: vec![ranked("New challenger", 1)],
            baseline: Some(ranked("Immutable root", 0)),
            strategy_signature: None,
            structural_model_revision: metagene_core::STRUCTURAL_MODEL_REVISION.into(),
            map_elite_cells: 0,
        };
        let cumulative = accumulate_continuation_progress(segment, &launch.lineage);
        assert_eq!(cumulative.generation, 2_010);
        assert_eq!(cumulative.total_generations, 2_500);
        assert_eq!(cumulative.evaluated_candidates, 257_280);
        assert_eq!(cumulative.unique_evaluations, 201_000);
        assert_eq!(cumulative.top[0].deck.name, "Validated incumbent");
        assert_eq!(cumulative.top[1].deck.name, "New challenger");
        assert_eq!(cumulative.best.rank, 2);
    }

    #[test]
    fn continuation_uses_the_persisted_immutable_tune_root_not_stale_ui_state() {
        let root = named_deck("Original tuning root");
        let mut saved_request = RunRequest {
            platform: "arena".into(),
            format: "historic".into(),
            simulator_coverage: "all".into(),
            pool_filter: Vec::new(),
            exclude_disputed_cards: false,
            exclude_unsupported_cards: false,
            pool: CardPool { cards: Vec::new() },
            seed_deck: root.clone(),
            constraints: DeckConstraints::default(),
            config: EvolutionConfig {
                evolution_mode: EvolutionMode::Tune,
                tune_policy: metagene_core::TunePolicy {
                    root_deck: Some(root.clone()),
                    ..Default::default()
                },
                ..Default::default()
            },
            opponent_field: OpponentFieldProfile::default(),
        };
        let stored = StoredRequest::from(&saved_request);
        saved_request.config.evolution_mode = EvolutionMode::Rebuild;
        saved_request.config.tune_policy.root_deck = Some(named_deck("Stale UI root"));
        let status = JobStatus {
            id: "saved".into(),
            state: "completed".into(),
            progress: None,
            result: Some(RunResult {
                generations_completed: 50,
                evaluated_candidates: 100,
                unique_evaluations: 90,
                evaluations_avoided: 10,
                elapsed_ms: 10,
                cancelled: false,
                frontier: vec![ranked("Candidate", 1)],
                baseline: Some(ranked("Original tuning root", 0)),
                strategy_signature: None,
                promotion_evidence: None,
                structural_model_revision: metagene_core::STRUCTURAL_MODEL_REVISION.into(),
                map_elites: Vec::new(),
            }),
            error: None,
        };
        preserve_saved_evolution_identity(&mut saved_request, &stored, &status);
        assert_eq!(saved_request.config.evolution_mode, EvolutionMode::Tune);
        assert_eq!(saved_request.config.tune_policy.root_deck, Some(root));
    }

    #[test]
    fn saved_deck_printing_aliases_merge_into_one_logical_entry() {
        let mut deck = Deck {
            name: "Legacy printings".into(),
            command_zone: Vec::new(),
            mainboard: vec![
                metagene_core::DeckEntry {
                    card_id: metagene_core::CardId("set-a".into()),
                    quantity: 2,
                },
                metagene_core::DeckEntry {
                    card_id: metagene_core::CardId("set-b".into()),
                    quantity: 2,
                },
            ],
            sideboard: Vec::new(),
        };
        let mapping = [
            (
                metagene_core::CardId("set-a".into()),
                metagene_core::CardId("logical-card".into()),
            ),
            (
                metagene_core::CardId("set-b".into()),
                metagene_core::CardId("logical-card".into()),
            ),
        ]
        .into_iter()
        .collect();
        remap_deck_ids(&mut deck, &mapping);
        assert_eq!(deck.mainboard.len(), 1);
        assert_eq!(deck.mainboard[0].card_id.0, "logical-card");
        assert_eq!(deck.mainboard[0].quantity, 4);
    }

    fn ranked(name: &str, rank: usize) -> RankedDeck {
        RankedDeck {
            rank,
            pareto_rank: 0,
            deck: Deck {
                name: name.into(),
                command_zone: Vec::new(),
                mainboard: Vec::new(),
                sideboard: Vec::new(),
            },
            fitness: metagene_core::Fitness {
                score: 0.5,
                ..Default::default()
            },
            packages: Vec::new(),
            commander_compatibility: None,
            candidate_status: "provisional".into(),
        }
    }

    fn hybrid_state(
        phase: &str,
        completed_cycles: usize,
        completed_generations: usize,
    ) -> StoredHybridState {
        StoredHybridState {
            config: StoredHybridConfig {
                opponents: Vec::new(),
                opponent_weights: Vec::new(),
                gauntlet_provenance: None,
                competitive_claims_allowed: true,
                field_revision_id: None,
                cycles: 3,
                generations_per_cycle: 20,
                finalists: 8,
                base_games: 16,
                rounds: 3,
                workers: 4,
                seats: 2,
                hat_budget: 20,
                max_turns: 60,
                game_timeout_seconds: 5,
                structural_weight: 0.35,
                risk_aversion: 0.5,
                worst_matchup_weight: 0.25,
                seat_position_weight: 0.25,
                adaptive_elimination: true,
                coevolve_adversaries: false,
                learn_packages_from_hall: false,
                holdout_games: 64,
                commander: false,
                base_seed: 42,
                base_name: "Hybrid".into(),
            },
            checkpoint: StoredHybridCheckpoint {
                current_cycle: completed_cycles.saturating_add(1),
                cycles_completed: completed_cycles,
                phase: phase.into(),
                completed_attempts: 120,
                total_evaluated: 700,
                winner_rank: Some(1),
                structural_generations_completed: completed_generations,
            },
        }
    }

    #[test]
    fn coevolved_runner_up_becomes_a_persisted_weighted_adversary_once() {
        let current = named_deck("Current field");
        let mut config = hybrid_state("structural", 0, 0).config;
        config.opponents = vec![current.clone()];
        config.opponent_weights = vec![1.0];
        config.coevolve_adversaries = true;
        let mut request = HybridEvolutionRequest {
            evolution: RunRequest {
                platform: "arena".into(),
                format: "historic".into(),
                simulator_coverage: "all".into(),
                pool_filter: Vec::new(),
                exclude_disputed_cards: false,
                exclude_unsupported_cards: false,
                pool: CardPool { cards: Vec::new() },
                seed_deck: named_deck("Seed"),
                constraints: DeckConstraints::default(),
                config: EvolutionConfig::default(),
                opponent_field: OpponentFieldProfile::default(),
            },
            opponents: vec![current],
            opponent_weights: vec![1.0],
            gauntlet_provenance: None,
            competitive_claims_allowed: true,
            field_revision_id: None,
            cycles: 3,
            finalists: 8,
            base_games: 16,
            rounds: 3,
            workers: 4,
            seats: 2,
            hat_budget: 20,
            max_turns: 60,
            game_timeout_seconds: 5,
            structural_weight: 0.35,
            risk_aversion: 0.5,
            worst_matchup_weight: 0.25,
            seat_position_weight: 0.25,
            adaptive_elimination: true,
            coevolve_adversaries: true,
            learn_packages_from_hall: false,
            holdout_games: 64,
            commander: false,
            resume: None,
            resumed_from: None,
            persisted_config: None,
            continue_run_id: None,
            continuation_candidate_rank: None,
        };
        let mut contender = ranked("Runner-up", 2);
        contender.deck.mainboard.push(metagene_core::DeckEntry {
            card_id: metagene_core::CardId("runner-card".into()),
            quantity: 4,
        });
        add_coevolved_adversary(&mut request, &mut config, &contender, 1);
        assert_eq!(request.opponents.len(), 2);
        assert!(
            request.opponents[1]
                .name
                .starts_with(HISTORICAL_CHAMPION_PREFIX)
        );
        assert_eq!(request.opponent_weights, vec![0.9, 0.1]);
        assert_eq!(config.opponents, request.opponents);
        assert_eq!(config.opponent_weights, request.opponent_weights);
        add_coevolved_adversary(&mut request, &mut config, &contender, 2);
        assert_eq!(request.opponents.len(), 2);
    }

    #[test]
    fn hybrid_resume_budget_is_phase_aware_and_survives_an_empty_new_status() {
        let empty_status = JobStatus {
            id: "resume".into(),
            state: "interrupted".into(),
            progress: None,
            result: None,
            error: None,
        };
        assert_eq!(
            hybrid_resume_budget(&hybrid_state("structural", 0, 7), &empty_status),
            Some((13, 3))
        );

        let frontier = vec![ranked("Current A", 1), ranked("Current B", 2)];
        let validation_status = JobStatus {
            result: Some(RunResult {
                generations_completed: 20,
                evaluated_candidates: 2_000,
                unique_evaluations: 1_900,
                evaluations_avoided: 100,
                elapsed_ms: 500,
                cancelled: false,
                frontier,
                baseline: None,
                strategy_signature: None,
                promotion_evidence: None,
                structural_model_revision: metagene_core::STRUCTURAL_MODEL_REVISION.into(),
                map_elites: Vec::new(),
            }),
            ..empty_status.clone()
        };
        assert_eq!(
            hybrid_resume_budget(&hybrid_state("validation", 0, 20), &validation_status),
            Some((0, 3))
        );
        assert_eq!(
            hybrid_resume_budget(&hybrid_state("cycle_complete", 2, 20), &empty_status),
            Some((20, 1))
        );
        assert!(
            hybrid_resume_budget(&hybrid_state("cycle_complete", 3, 20), &empty_status).is_none()
        );
    }

    #[test]
    fn hybrid_config_preserves_saved_gauntlet_revision_and_accepts_legacy_rows() {
        let mut config = hybrid_state("structural", 0, 7).config;
        config.gauntlet_provenance = Some(GauntletProvenance {
            id: "field-id".into(),
            name: "Historic field".into(),
            revision: 4,
        });
        let value = serde_json::to_value(&config).unwrap();
        let restored: StoredHybridConfig = serde_json::from_value(value.clone()).unwrap();
        let provenance = restored.gauntlet_provenance.unwrap();
        assert_eq!(provenance.id, "field-id");
        assert_eq!(provenance.name, "Historic field");
        assert_eq!(provenance.revision, 4);

        let mut legacy = value;
        legacy
            .as_object_mut()
            .unwrap()
            .remove("gauntlet_provenance");
        let restored: StoredHybridConfig = serde_json::from_value(legacy).unwrap();
        assert!(restored.gauntlet_provenance.is_none());
    }

    #[test]
    fn completed_progress_frontier_wins_over_a_stale_prior_cycle_result() {
        let current = vec![ranked("Current A", 1), ranked("Current B", 2)];
        let status = JobStatus {
            id: "hybrid".into(),
            state: "interrupted".into(),
            progress: Some(GenerationProgress {
                generation: 20,
                total_generations: 20,
                evaluated_candidates: 2_000,
                unique_evaluations: 1_900,
                evaluations_avoided: 100,
                unique_in_generation: 90,
                duplicates_in_generation: 10,
                elapsed_ms: 500,
                candidates_per_second: 4_000.0,
                unique_evaluations_per_second: 3_800.0,
                best: current[0].clone(),
                top: current,
                baseline: None,
                strategy_signature: None,
                structural_model_revision: metagene_core::STRUCTURAL_MODEL_REVISION.into(),
                map_elite_cells: 0,
            }),
            result: Some(RunResult {
                generations_completed: 20,
                evaluated_candidates: 1_000,
                unique_evaluations: 950,
                evaluations_avoided: 50,
                elapsed_ms: 250,
                cancelled: false,
                frontier: vec![ranked("Stale A", 1), ranked("Stale B", 2)],
                baseline: None,
                strategy_signature: None,
                promotion_evidence: None,
                structural_model_revision: metagene_core::STRUCTURAL_MODEL_REVISION.into(),
                map_elites: Vec::new(),
            }),
            error: None,
        };
        let recovered = checkpoint_result(&status).unwrap();
        assert_eq!(recovered.frontier[0].deck.name, "Current A");
    }

    #[test]
    fn repeated_structural_interruptions_advance_only_the_remaining_segment() {
        let progress = |generation, total_generations, name: &str| GenerationProgress {
            generation,
            total_generations,
            evaluated_candidates: generation as u64 * 100,
            unique_evaluations: generation as u64 * 90,
            evaluations_avoided: generation as u64 * 10,
            unique_in_generation: 90,
            duplicates_in_generation: 10,
            elapsed_ms: generation as u64 * 10,
            candidates_per_second: 1_000.0,
            unique_evaluations_per_second: 900.0,
            best: ranked(name, 1),
            top: vec![ranked(name, 1), ranked("Runner up", 2)],
            baseline: None,
            strategy_signature: None,
            structural_model_revision: metagene_core::STRUCTURAL_MODEL_REVISION.into(),
            map_elite_cells: 0,
        };
        let first_status = JobStatus {
            id: "first".into(),
            state: "interrupted".into(),
            progress: Some(progress(7, 20, "Generation 7")),
            result: None,
            error: None,
        };
        let (first_resume, first_generation) =
            build_hybrid_resume(&hybrid_state("structural", 0, 7), &first_status, 42, None)
                .unwrap();
        assert_eq!(first_generation, 7);
        assert_eq!(first_resume.first_cycle_generations, Some(13));
        assert_eq!(first_resume.generation_offset, 7);
        assert_eq!(first_resume.first_cycle_seed, Some(continued_seed(42, 7)));

        let second_status = JobStatus {
            id: "second".into(),
            state: "interrupted".into(),
            progress: Some(progress(5, 13, "Generation 12")),
            result: None,
            error: None,
        };
        let active_seed = first_resume.first_cycle_seed.unwrap();
        let (second_resume, second_generation) = build_hybrid_resume(
            &hybrid_state("structural", 0, 12),
            &second_status,
            active_seed,
            None,
        )
        .unwrap();
        assert_eq!(second_generation, 12);
        assert_eq!(second_resume.first_cycle_generations, Some(8));
        assert_eq!(second_resume.generation_offset, 12);
        assert_eq!(
            second_resume.first_cycle_seed,
            Some(continued_seed(active_seed, 5))
        );
    }

    #[test]
    fn validation_and_cycle_boundaries_resume_without_repeating_structural_work() {
        let validation_result = RunResult {
            generations_completed: 20,
            evaluated_candidates: 2_000,
            unique_evaluations: 1_900,
            evaluations_avoided: 100,
            elapsed_ms: 500,
            cancelled: false,
            frontier: vec![ranked("Validated A", 1), ranked("Validated B", 2)],
            baseline: None,
            strategy_signature: None,
            promotion_evidence: None,
            structural_model_revision: metagene_core::STRUCTURAL_MODEL_REVISION.into(),
            map_elites: Vec::new(),
        };
        let validation_status = JobStatus {
            id: "validation".into(),
            state: "interrupted".into(),
            progress: None,
            result: Some(validation_result),
            error: None,
        };
        let (validation_resume, generation) = build_hybrid_resume(
            &hybrid_state("validation", 0, 20),
            &validation_status,
            42,
            None,
        )
        .unwrap();
        assert_eq!(generation, 20);
        assert_eq!(validation_resume.phase, "validation");
        assert_eq!(validation_resume.first_cycle_generations, None);
        assert_eq!(
            validation_resume.initial_result.unwrap().frontier[0]
                .deck
                .name,
            "Validated A"
        );

        let empty_status = JobStatus {
            id: "cycle".into(),
            state: "interrupted".into(),
            progress: None,
            result: None,
            error: None,
        };
        let (cycle_resume, generation) = build_hybrid_resume(
            &hybrid_state("cycle_complete", 2, 20),
            &empty_status,
            42,
            None,
        )
        .unwrap();
        assert_eq!(generation, 0);
        assert_eq!(cycle_resume.start_cycle, 3);
        assert_eq!(cycle_resume.cycles_completed, 2);
        assert_eq!(cycle_resume.phase, "structural");
        assert_eq!(cycle_resume.first_cycle_seed, None);
        assert_eq!(cycle_resume.generation_offset, 0);
    }
}

#[tauri::command]
fn evolution_status(job_id: String, state: State<'_, AppState>) -> Result<JobStatus, String> {
    state
        .jobs
        .lock()
        .map_err(|_| "job store poisoned")?
        .get(&job_id)
        .map(|record| record.status.clone())
        .ok_or_else(|| "Unknown evolution job".into())
}

#[tauri::command]
fn cancel_evolution(job_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let jobs = state.jobs.lock().map_err(|_| "job store poisoned")?;
    let record = jobs.get(&job_id).ok_or("Unknown evolution job")?;
    record.cancellation.cancel();
    Ok(())
}

#[tauri::command]
fn export_deck(
    deck: Deck,
    platform: String,
    format: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let pool = state
        .export_pool
        .lock()
        .map_err(|_| "export pool poisoned")?;
    let pool = pool.as_ref().ok_or("No card pool is loaded")?;
    let text = export_mtga(&deck, pool);
    feedback::register_revision(&state.data_dir, &platform, &format, &deck, pool, &text)
        .map_err(|error| error.to_string())?;
    Ok(text)
}

#[tauri::command]
fn run_history(limit: usize, state: State<'_, AppState>) -> Result<Vec<runs::RunSummary>, String> {
    runs::list(&state.data_dir, limit).map_err(|error| error.to_string())
}

#[tauri::command]
fn update_run_metadata(
    metadata: runs::RunMetadata,
    state: State<'_, AppState>,
) -> Result<runs::RunSummary, String> {
    runs::update_metadata(&state.data_dir, &metadata).map_err(|error| error.to_string())
}

#[tauri::command]
fn export_run_manifest(id: String, state: State<'_, AppState>) -> Result<String, String> {
    runs::export_manifest(&state.data_dir, &id).map_err(|error| error.to_string())
}

#[tauri::command]
async fn storage_health(state: State<'_, AppState>) -> Result<maintenance::StorageHealth, String> {
    let data_dir = state.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        maintenance::health(&data_dir).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn create_local_backup(
    state: State<'_, AppState>,
) -> Result<maintenance::BackupSummary, String> {
    let data_dir = state.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        maintenance::create_backup(&data_dir).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn restore_local_backup(
    id: String,
    state: State<'_, AppState>,
) -> Result<maintenance::StorageHealth, String> {
    let data_dir = state.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        maintenance::restore_backup(&data_dir, &id).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn repair_corrupt_database(
    name: String,
    state: State<'_, AppState>,
) -> Result<maintenance::StorageHealth, String> {
    let data_dir = state.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        maintenance::quarantine_corrupt(&data_dir, &name).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn export_safe_diagnostics(state: State<'_, AppState>) -> Result<String, String> {
    let data_dir = state.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        maintenance::diagnostics(&data_dir).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn load_saved_run(id: String, state: State<'_, AppState>) -> Result<SavedRun, String> {
    let row = runs::load(&state.data_dir, &id).map_err(|error| error.to_string())?;
    let mut stored: StoredRequest =
        serde_json::from_str(&row.request_json).map_err(|error| error.to_string())?;
    let mut status: JobStatus =
        serde_json::from_str(&row.status_json).map_err(|error| error.to_string())?;
    canonicalize_stored_run_ids(&state.data_dir, &mut stored, &mut status)?;
    let resumed_from = stored.resumed_from.clone();
    let run_kind = stored.run_kind.clone();
    let hybrid = stored.hybrid.clone();
    let structural_lineage = stored.structural_lineage.clone();
    let original_seed = stored.seed_deck.clone();
    let mut request = hydrate_stored_request(&state.data_dir, stored)?;
    migrate_legacy_nonblank_run_to_tune(
        &mut request,
        &original_seed,
        saved_structural_revision(&status),
    );
    let completed_generations = checkpoint_candidate(&status)
        .map(|(_, generation)| generation)
        .unwrap_or(0);
    let segment_completed_generations = structural_lineage
        .as_ref()
        .map(|lineage| completed_generations.saturating_sub(lineage.generation_offset))
        .unwrap_or(completed_generations);
    let structural_generation_budget = structural_lineage
        .as_ref()
        .map(|lineage| lineage.segment_generations)
        .unwrap_or(request.config.generations);
    let structural_remaining_generations =
        structural_generation_budget.saturating_sub(segment_completed_generations);
    let recoverable_state = matches!(
        status.state.as_str(),
        "interrupted" | "cancelled" | "failed"
    );
    let hybrid_budget = hybrid
        .as_ref()
        .and_then(|hybrid| hybrid_resume_budget(hybrid, &status));
    let resumable = recoverable_state
        && ((run_kind == "structural"
            && checkpoint_candidate(&status).is_some()
            && structural_remaining_generations > 0)
            || (run_kind == "hybrid" && hybrid_budget.is_some()));
    let (remaining_generations, remaining_cycles) = if run_kind == "hybrid" {
        hybrid_budget.unwrap_or((0, 0))
    } else {
        (structural_remaining_generations, 0)
    };
    let hybrid_phase = hybrid
        .as_ref()
        .map(|hybrid| hybrid.checkpoint.phase.clone());
    let hybrid_current_cycle = hybrid
        .as_ref()
        .map(|hybrid| hybrid.checkpoint.current_cycle);
    let hybrid_gauntlet_provenance = hybrid
        .as_ref()
        .and_then(|hybrid| hybrid.config.gauntlet_provenance.clone());
    *state
        .export_pool
        .lock()
        .map_err(|_| "export pool poisoned")? = Some(request.pool.clone());
    Ok(SavedRun {
        request,
        status,
        resumable,
        remaining_generations,
        resumed_from,
        run_kind,
        remaining_cycles,
        hybrid_phase,
        hybrid_current_cycle,
        hybrid_gauntlet_provenance,
    })
}

fn hydrate_stored_request(
    data_dir: &std::path::Path,
    stored: StoredRequest,
) -> Result<RunRequest, String> {
    let cards = catalog::legal_pool(data_dir, &stored.platform, &stored.format)
        .map_err(|error| error.to_string())?;
    Ok(RunRequest {
        platform: stored.platform,
        format: stored.format,
        simulator_coverage: stored.simulator_coverage,
        pool_filter: stored.pool_filter,
        exclude_disputed_cards: stored.exclude_disputed_cards,
        exclude_unsupported_cards: stored.exclude_unsupported_cards,
        pool: CardPool { cards },
        seed_deck: stored.seed_deck,
        constraints: stored.constraints,
        config: stored.config,
        opponent_field: stored.opponent_field,
    })
}

fn collect_deck_ids(deck: &Deck, ids: &mut Vec<metagene_core::CardId>) {
    ids.extend(
        deck.command_zone
            .iter()
            .chain(deck.mainboard.iter())
            .chain(deck.sideboard.iter())
            .map(|entry| entry.card_id.clone()),
    );
}

fn remap_deck_ids(
    deck: &mut Deck,
    mapping: &HashMap<metagene_core::CardId, metagene_core::CardId>,
) {
    let remap_zone = |entries: &mut Vec<metagene_core::DeckEntry>| {
        let mut merged = Vec::<metagene_core::DeckEntry>::new();
        for mut entry in std::mem::take(entries) {
            if let Some(canonical) = mapping.get(&entry.card_id) {
                entry.card_id = canonical.clone();
            }
            if let Some(existing) = merged
                .iter_mut()
                .find(|existing| existing.card_id == entry.card_id)
            {
                existing.quantity = existing.quantity.saturating_add(entry.quantity);
            } else {
                merged.push(entry);
            }
        }
        *entries = merged;
    };
    remap_zone(&mut deck.command_zone);
    remap_zone(&mut deck.mainboard);
    remap_zone(&mut deck.sideboard);
}

fn canonicalize_stored_run_ids(
    data_dir: &std::path::Path,
    stored: &mut StoredRequest,
    status: &mut JobStatus,
) -> Result<(), String> {
    let mut ids = Vec::new();
    collect_deck_ids(&stored.seed_deck, &mut ids);
    if let Some(root) = stored.config.tune_policy.root_deck.as_ref() {
        collect_deck_ids(root, &mut ids);
    }
    ids.extend(
        stored
            .constraints
            .locked_cards
            .iter()
            .map(|locked| locked.card_id.clone()),
    );
    ids.extend(stored.config.learned_packages.iter().flatten().cloned());
    if let Some(hybrid) = stored.hybrid.as_ref() {
        for deck in &hybrid.config.opponents {
            collect_deck_ids(deck, &mut ids);
        }
    }
    if let Some(lineage) = stored.structural_lineage.as_ref() {
        collect_deck_ids(&lineage.incumbent.deck, &mut ids);
    }
    if let Some(progress) = status.progress.as_ref() {
        collect_deck_ids(&progress.best.deck, &mut ids);
        for candidate in &progress.top {
            collect_deck_ids(&candidate.deck, &mut ids);
        }
        if let Some(baseline) = progress.baseline.as_ref() {
            collect_deck_ids(&baseline.deck, &mut ids);
        }
    }
    if let Some(result) = status.result.as_ref() {
        for candidate in &result.frontier {
            collect_deck_ids(&candidate.deck, &mut ids);
        }
        if let Some(baseline) = result.baseline.as_ref() {
            collect_deck_ids(&baseline.deck, &mut ids);
        }
    }
    let mapping = catalog::canonical_ids(data_dir, &ids).map_err(|error| error.to_string())?;
    if mapping.is_empty() {
        return Ok(());
    }

    remap_deck_ids(&mut stored.seed_deck, &mapping);
    if let Some(root) = stored.config.tune_policy.root_deck.as_mut() {
        remap_deck_ids(root, &mapping);
    }
    for locked in &mut stored.constraints.locked_cards {
        if let Some(canonical) = mapping.get(&locked.card_id) {
            locked.card_id = canonical.clone();
        }
    }
    stored.constraints.locked_cards.sort_by(|left, right| {
        left.card_id
            .0
            .cmp(&right.card_id.0)
            .then(left.minimum.cmp(&right.minimum))
    });
    stored.constraints.locked_cards.dedup_by(|left, right| {
        if left.card_id != right.card_id {
            return false;
        }
        right.minimum = right.minimum.max(left.minimum);
        right.maximum = right.maximum.max(left.maximum);
        true
    });
    for package in &mut stored.config.learned_packages {
        for id in package.iter_mut() {
            if let Some(canonical) = mapping.get(id) {
                *id = canonical.clone();
            }
        }
        package.sort_by(|left, right| left.0.cmp(&right.0));
        package.dedup();
    }
    if let Some(hybrid) = stored.hybrid.as_mut() {
        for deck in &mut hybrid.config.opponents {
            remap_deck_ids(deck, &mapping);
        }
    }
    if let Some(lineage) = stored.structural_lineage.as_mut() {
        remap_deck_ids(&mut lineage.incumbent.deck, &mapping);
    }
    if let Some(progress) = status.progress.as_mut() {
        remap_deck_ids(&mut progress.best.deck, &mapping);
        for candidate in &mut progress.top {
            remap_deck_ids(&mut candidate.deck, &mapping);
        }
        if let Some(baseline) = progress.baseline.as_mut() {
            remap_deck_ids(&mut baseline.deck, &mapping);
        }
    }
    if let Some(result) = status.result.as_mut() {
        for candidate in &mut result.frontier {
            remap_deck_ids(&mut candidate.deck, &mapping);
        }
        if let Some(baseline) = result.baseline.as_mut() {
            remap_deck_ids(&mut baseline.deck, &mapping);
        }
    }
    Ok(())
}

fn checkpoint_candidate(status: &JobStatus) -> Option<(&RankedDeck, usize)> {
    status
        .progress
        .as_ref()
        .map(|progress| (&progress.best, progress.generation))
        .or_else(|| {
            status.result.as_ref().and_then(|result| {
                result
                    .frontier
                    .first()
                    .map(|candidate| (candidate, result.generations_completed))
            })
        })
}

fn checkpoint_result(status: &JobStatus) -> Option<RunResult> {
    status
        .progress
        .as_ref()
        .and_then(|progress| {
            (progress.generation == progress.total_generations && progress.top.len() >= 2).then(
                || RunResult {
                    generations_completed: progress.generation,
                    evaluated_candidates: progress.evaluated_candidates,
                    unique_evaluations: progress.unique_evaluations,
                    evaluations_avoided: progress.evaluations_avoided,
                    elapsed_ms: progress.elapsed_ms,
                    cancelled: false,
                    frontier: progress.top.clone(),
                    baseline: progress.baseline.clone(),
                    strategy_signature: progress.strategy_signature.clone(),
                    promotion_evidence: None,
                    structural_model_revision: progress.structural_model_revision.clone(),
                    map_elites: Vec::new(),
                },
            )
        })
        .or_else(|| status.result.clone())
}

fn hybrid_resume_budget(hybrid: &StoredHybridState, status: &JobStatus) -> Option<(usize, usize)> {
    let remaining_cycles = hybrid
        .config
        .cycles
        .saturating_sub(hybrid.checkpoint.cycles_completed);
    if remaining_cycles == 0 {
        return None;
    }
    let remaining_generations = match hybrid.checkpoint.phase.as_str() {
        "structural" => {
            let remaining = hybrid
                .config
                .generations_per_cycle
                .saturating_sub(hybrid.checkpoint.structural_generations_completed);
            if remaining == 0 && checkpoint_result(status).is_none() {
                return None;
            }
            remaining
        }
        "validation" => {
            checkpoint_result(status)?;
            0
        }
        "cycle_complete" => hybrid.config.generations_per_cycle,
        _ => return None,
    };
    Some((remaining_generations, remaining_cycles))
}

fn continued_seed(seed: u64, completed_generations: usize) -> u64 {
    seed.wrapping_add((completed_generations as u64 + 1).wrapping_mul(0xD1B5_4A32_D192_ED03))
}

fn build_hybrid_resume(
    hybrid: &StoredHybridState,
    status: &JobStatus,
    active_seed: u64,
    structural_lineage: Option<StoredStructuralLineage>,
) -> Result<(HybridResumeState, usize), String> {
    let (remaining_generations, _) = hybrid_resume_budget(hybrid, status)
        .ok_or("This hybrid run has no recoverable work remaining.")?;
    let checkpoint = &hybrid.checkpoint;
    let mut resumed_generation = checkpoint.structural_generations_completed;
    let resume = match checkpoint.phase.as_str() {
        "structural" if remaining_generations > 0 => {
            let segment_generation = status
                .progress
                .as_ref()
                .map_or(0, |progress| progress.generation);
            HybridResumeState {
                start_cycle: checkpoint.current_cycle,
                cycles_completed: checkpoint.cycles_completed,
                phase: "structural".into(),
                completed_attempts: checkpoint.completed_attempts,
                total_evaluated: checkpoint.total_evaluated,
                winner_rank: checkpoint.winner_rank,
                first_cycle_generations: Some(remaining_generations),
                first_cycle_seed: Some(if segment_generation == 0 {
                    active_seed
                } else {
                    continued_seed(active_seed, segment_generation)
                }),
                initial_result: None,
                generation_offset: checkpoint.structural_generations_completed,
                structural_lineage: structural_lineage.clone(),
            }
        }
        "structural" | "validation" => HybridResumeState {
            start_cycle: checkpoint.current_cycle,
            cycles_completed: checkpoint.cycles_completed,
            phase: "validation".into(),
            completed_attempts: checkpoint.completed_attempts,
            total_evaluated: checkpoint.total_evaluated,
            winner_rank: checkpoint.winner_rank,
            first_cycle_generations: None,
            first_cycle_seed: Some(active_seed),
            initial_result: Some(
                checkpoint_result(status)
                    .ok_or("The hybrid validation checkpoint has no durable frontier.")?,
            ),
            generation_offset: hybrid.config.generations_per_cycle,
            structural_lineage: structural_lineage.clone(),
        },
        "cycle_complete" => {
            resumed_generation = 0;
            HybridResumeState {
                start_cycle: checkpoint.cycles_completed.saturating_add(1),
                cycles_completed: checkpoint.cycles_completed,
                phase: "structural".into(),
                completed_attempts: checkpoint.completed_attempts,
                total_evaluated: checkpoint.total_evaluated,
                winner_rank: checkpoint.winner_rank,
                first_cycle_generations: None,
                first_cycle_seed: None,
                initial_result: None,
                generation_offset: 0,
                structural_lineage,
            }
        }
        phase => return Err(format!("Unknown hybrid checkpoint phase '{phase}'.")),
    };
    Ok((resume, resumed_generation))
}

#[tauri::command]
fn resume_evolution(
    id: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<ResumedRun, String> {
    if state
        .jobs
        .lock()
        .map_err(|_| "job store poisoned")?
        .get(&id)
        .is_some_and(|record| record.status.state == "running")
    {
        return Err("This run is still active and cannot be resumed yet.".into());
    }
    let row = runs::load(&state.data_dir, &id).map_err(|error| error.to_string())?;
    let mut stored: StoredRequest =
        serde_json::from_str(&row.request_json).map_err(|error| error.to_string())?;
    let mut status: JobStatus =
        serde_json::from_str(&row.status_json).map_err(|error| error.to_string())?;
    canonicalize_stored_run_ids(&state.data_dir, &mut stored, &mut status)?;
    let original_seed = stored.seed_deck.clone();
    let previous_lineage = stored.structural_lineage.clone();
    let prior_resumed_from = stored.resumed_from.clone();
    let structural_revision = saved_structural_revision(&status).to_string();
    if !matches!(
        status.state.as_str(),
        "interrupted" | "cancelled" | "failed"
    ) {
        return Err(format!("A {} run cannot be resumed.", status.state));
    }
    if stored.run_kind == "hybrid" {
        let hybrid = stored
            .hybrid
            .clone()
            .ok_or("This legacy hybrid run has no phase-aware checkpoint payload.")?;
        let (_, remaining_cycles) = hybrid_resume_budget(&hybrid, &status)
            .ok_or("This hybrid run has no recoverable work remaining.")?;
        let active_seed = stored.config.seed;
        let mut evolution = hydrate_stored_request(&state.data_dir, stored)?;
        migrate_legacy_nonblank_run_to_tune(&mut evolution, &original_seed, &structural_revision);
        let (resume, resumed_generation) =
            build_hybrid_resume(&hybrid, &status, active_seed, previous_lineage.clone())?;
        if hybrid.checkpoint.phase == "structural" && resume.first_cycle_generations.is_some() {
            evolution.seed_deck = status
                .progress
                .as_ref()
                .map(|progress| progress.best.deck.clone())
                .unwrap_or_else(|| evolution.seed_deck.clone());
            evolution.seed_deck.name = format!("{} · resumed", evolution.seed_deck.name);
        }
        let hybrid_request = hybrid.config.into_request(evolution, resume, id.clone());
        let launched_request = hybrid_request.evolution.clone();
        let new_id = start_hybrid_evolution(hybrid_request, app, state)?;
        return Ok(ResumedRun {
            id: new_id,
            request: launched_request,
            resumed_from: id,
            resumed_generation,
            run_kind: "hybrid".into(),
            remaining_cycles,
        });
    }
    if stored.run_kind != "structural" {
        return Err(format!("Unknown saved run kind '{}'.", stored.run_kind));
    }
    let (candidate, completed_generations) = checkpoint_candidate(&status)
        .ok_or("This run has no durable candidate checkpoint to resume from.")?;
    let candidate = candidate.clone();
    let segment_completed_generations = previous_lineage
        .as_ref()
        .map(|lineage| completed_generations.saturating_sub(lineage.generation_offset))
        .unwrap_or(completed_generations);
    let segment_generation_budget = previous_lineage
        .as_ref()
        .map(|lineage| lineage.segment_generations)
        .unwrap_or(stored.config.generations);
    let remaining_generations =
        segment_generation_budget.saturating_sub(segment_completed_generations);
    if remaining_generations == 0 {
        return Err("This run already consumed its configured generation budget.".into());
    }
    let mut request = hydrate_stored_request(&state.data_dir, stored)?;
    migrate_legacy_nonblank_run_to_tune(&mut request, &original_seed, &structural_revision);
    request.seed_deck = candidate.deck.clone();
    request.seed_deck.name = format!("{} · resumed", request.seed_deck.name);
    request.config.generations = remaining_generations;
    request.config.seed = continued_seed(request.config.seed, completed_generations);
    let request = prepare_evolution_request(&state.data_dir, request)?;
    let launched_request = request.clone();
    let launch = continuation_launch(
        id.clone(),
        &status,
        &candidate,
        remaining_generations,
        previous_lineage.as_ref(),
    )?;
    let new_id = launch_evolution(request, prior_resumed_from, Some(launch), app, &state)?;
    Ok(ResumedRun {
        id: new_id,
        request: launched_request,
        resumed_from: id,
        resumed_generation: completed_generations,
        run_kind: "structural".into(),
        remaining_cycles: 0,
    })
}

#[tauri::command]
fn simulator_status(state: State<'_, AppState>) -> Vec<simulator::SimulatorStatus> {
    let mut statuses = simulator::statuses(&state.data_dir);
    statuses.push(forge::status(&state.data_dir));
    statuses
}

#[tauri::command]
fn simulation_cache_status(
    state: State<'_, AppState>,
) -> Result<match_cache::MatchCacheStatus, String> {
    match_cache::status(&state.data_dir).map_err(|error| error.to_string())
}

#[tauri::command]
fn clear_simulation_cache(
    state: State<'_, AppState>,
) -> Result<match_cache::MatchCacheStatus, String> {
    match_cache::clear(&state.data_dir).map_err(|error| error.to_string())
}

#[tauri::command]
async fn card_art(
    image_uri: String,
    state: State<'_, AppState>,
) -> Result<tauri::ipc::Response, String> {
    let data_dir = state.data_dir.clone();
    let bytes = tauri::async_runtime::spawn_blocking(move || {
        art_cache::get(&data_dir, &image_uri).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
fn art_cache_status(state: State<'_, AppState>) -> Result<art_cache::ArtCacheStatus, String> {
    art_cache::status(&state.data_dir).map_err(|error| error.to_string())
}

#[tauri::command]
fn clear_art_cache(state: State<'_, AppState>) -> Result<art_cache::ArtCacheStatus, String> {
    art_cache::clear(&state.data_dir).map_err(|error| error.to_string())
}

#[tauri::command]
fn conformance_summary(
    state: State<'_, AppState>,
) -> Result<conformance::ConformanceSummary, String> {
    conformance::summary(&state.data_dir).map_err(|error| error.to_string())
}

#[tauri::command]
async fn audit_oracle_accounting(
    platform: String,
    format: String,
    state: State<'_, AppState>,
) -> Result<catalog::OracleAccountingAudit, String> {
    let data_dir = state.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let cards = catalog::legal_pool(&data_dir, &platform, &format)
            .map_err(|error| error.to_string())?;
        Ok(catalog::audit_oracle_accounting(&cards))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn audit_training_labels(
    state: State<'_, AppState>,
) -> Result<surrogate::TrainingLabelAudit, String> {
    surrogate::audit_training_labels(&state.data_dir).map_err(|error| error.to_string())
}

#[tauri::command]
async fn rebuild_engine_capability_manifest(
    engine: String,
    platform: String,
    format: String,
    state: State<'_, AppState>,
) -> Result<competitive::EngineCapabilityManifest, String> {
    let data_dir = state.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let pool = CardPool {
            cards: catalog::legal_pool(&data_dir, &platform, &format)
                .map_err(|error| error.to_string())?,
        };
        competitive::rebuild_engine_capability_manifest(&data_dir, &engine, &pool)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn audit_format_capability(
    platform: String,
    format: String,
    field: Option<competitive::MetagameFieldRevision>,
    candidate: Option<Deck>,
    state: State<'_, AppState>,
) -> Result<competitive::FormatCapabilityAudit, String> {
    let data_dir = state.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let pool = CardPool {
            cards: catalog::legal_pool(&data_dir, &platform, &format)
                .map_err(|error| error.to_string())?,
        };
        Ok(competitive::audit_format_capability(
            &data_dir,
            &platform,
            &format,
            &pool,
            field.as_ref(),
            candidate.as_ref(),
        ))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn commander_conformance() -> metagene_core::CommanderConformanceReport {
    metagene_core::commander_conformance_report()
}

#[tauri::command]
async fn import_conformance_bundle(
    bundle_json: String,
    state: State<'_, AppState>,
) -> Result<conformance::ConformanceImportResult, String> {
    let data_dir = state.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        conformance::import_bundle_json(&data_dir, &bundle_json).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn deck_conformance(
    decks: Vec<Deck>,
    state: State<'_, AppState>,
) -> Result<conformance::ConformanceAuditReport, String> {
    let data_dir = state.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let ids = decks
            .iter()
            .flat_map(|deck| {
                deck.command_zone
                    .iter()
                    .chain(deck.mainboard.iter())
                    .chain(deck.sideboard.iter())
            })
            .map(|entry| entry.card_id.clone())
            .collect::<Vec<_>>();
        let names_by_id = catalog::cards_by_ids(&data_dir, &ids)
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|card| (card.id, card.name))
            .collect::<HashMap<_, _>>();
        let deck_cards = decks
            .into_iter()
            .map(|deck| {
                let names = deck
                    .command_zone
                    .iter()
                    .chain(deck.mainboard.iter())
                    .chain(deck.sideboard.iter())
                    .filter_map(|entry| names_by_id.get(&entry.card_id).cloned())
                    .collect::<Vec<_>>();
                (deck.name, names)
            })
            .collect::<Vec<_>>();
        conformance::audit_decks(&data_dir, &deck_cards).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn card_conformance(
    card_names: Vec<String>,
    state: State<'_, AppState>,
) -> Result<conformance::DeckConformanceReport, String> {
    let data_dir = state.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        conformance::audit_cards(&data_dir, &card_names).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn hexdek_deck_coverage(
    decks: Vec<Deck>,
    state: State<'_, AppState>,
) -> Result<simulator::HexdekCoverageReport, String> {
    let data_dir = state.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let ids = decks
            .iter()
            .flat_map(|deck| {
                deck.command_zone
                    .iter()
                    .chain(deck.mainboard.iter())
                    .chain(deck.sideboard.iter())
            })
            .map(|entry| entry.card_id.clone())
            .collect::<Vec<_>>();
        let pool = CardPool {
            cards: catalog::cards_by_ids(&data_dir, &ids).map_err(|error| error.to_string())?,
        };
        let refs = decks.iter().collect::<Vec<_>>();
        let inputs = simulator::coverage_inputs_from_decks(&refs, &pool)
            .map_err(|error| error.to_string())?;
        simulator::audit_hexdek_coverage(&data_dir, &inputs).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn run_hexdek_validation(
    mut request: simulator::HexdekValidationRequest,
    run_id: Option<String>,
    candidate_rank: Option<usize>,
    state: State<'_, AppState>,
) -> Result<simulator::SimulationReport, String> {
    require_structural_validation(
        &request.candidate,
        &request.pool,
        request.commander,
        request.seed,
        request.max_turns,
    )?;
    require_saved_run_tune_validation(
        &state.data_dir,
        run_id.as_deref(),
        &request.candidate,
        &request.pool,
        request.commander,
        request.seed,
        request.max_turns,
    )?;
    request.opponent_weights = if request.commander {
        vec![1.0; request.opponents.len()]
    } else {
        normalize_opponent_weights(&request.opponent_weights, request.opponents.len())?
    };
    let data_dir = state.data_dir.clone();
    let compact_request = serde_json::json!({
        "candidate": &request.candidate,
        "opponents": &request.opponents,
        "opponent_weights": &request.opponent_weights,
        "games": request.games,
        "seed": request.seed,
        "workers": request.workers,
        "seats": request.seats,
        "hat_budget": request.hat_budget,
        "max_turns": request.max_turns,
        "game_timeout_seconds": request.game_timeout_seconds,
        "commander": request.commander,
    });
    tauri::async_runtime::spawn_blocking(move || {
        let report = simulator::run_validation_gauntlet(&data_dir, &request)
            .map_err(|error| error.to_string())?;
        runs::save_validation(
            &data_dir,
            run_id.as_deref(),
            candidate_rank,
            &compact_request,
            &report,
        )
        .map_err(|error| error.to_string())?;
        Ok(report)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn run_hexdek_best_of_three(
    mut request: simulator::BestOfThreeValidationRequest,
    run_id: Option<String>,
    candidate_rank: Option<usize>,
    state: State<'_, AppState>,
) -> Result<simulator::BestOfThreeSimulationReport, String> {
    require_structural_validation(
        &request.candidate,
        &request.pool,
        false,
        request.seed,
        request.max_turns,
    )?;
    require_saved_run_tune_validation(
        &state.data_dir,
        run_id.as_deref(),
        &request.candidate,
        &request.pool,
        false,
        request.seed,
        request.max_turns,
    )?;
    request.opponent_weights =
        normalize_opponent_weights(&request.opponent_weights, request.opponents.len())?;
    simulator::validate_best_of_three_request(&request).map_err(|error| error.to_string())?;
    let data_dir = state.data_dir.clone();
    let compact_request = serde_json::json!({
        "candidate": &request.candidate,
        "opponents": &request.opponents,
        "opponent_weights": &request.opponent_weights,
        "constraints": &request.constraints,
        "sideboard_plans": &request.sideboard_plans,
        "matches": request.matches,
        "seed": request.seed,
        "workers": request.workers,
        "hat_budget": request.hat_budget,
        "max_turns": request.max_turns,
        "game_timeout_seconds": request.game_timeout_seconds,
    });
    tauri::async_runtime::spawn_blocking(move || {
        let report =
            simulator::run_best_of_three(&data_dir, &request).map_err(|error| error.to_string())?;
        runs::save_best_of_three(
            &data_dir,
            run_id.as_deref(),
            candidate_rank,
            &compact_request,
            &report,
        )
        .map_err(|error| error.to_string())?;
        Ok(report)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn evolve_sideboard_plans(
    mut request: sideboard_evolution::SideboardEvolutionRequest,
    run_id: Option<String>,
    candidate_rank: Option<usize>,
    state: State<'_, AppState>,
) -> Result<sideboard_evolution::SideboardEvolutionReport, String> {
    require_structural_validation(
        &request.candidate,
        &request.pool,
        false,
        request.seed,
        request.max_turns,
    )?;
    require_saved_run_tune_validation(
        &state.data_dir,
        run_id.as_deref(),
        &request.candidate,
        &request.pool,
        false,
        request.seed,
        request.max_turns,
    )?;
    request.opponent_weights =
        normalize_opponent_weights(&request.opponent_weights, request.opponents.len())?;
    let cancellation = CancellationToken::default();
    {
        let mut active = state
            .sideboard_evolution
            .lock()
            .map_err(|_| "Sideboard evolution state is unavailable".to_string())?;
        if active.is_some() {
            return Err("A sideboard evolution is already running".into());
        }
        *active = Some(cancellation.clone());
    }
    let active_state = state.sideboard_evolution.clone();
    let data_dir = state.data_dir.clone();
    let compact_request = serde_json::json!({
        "candidate": &request.candidate,
        "opponents": &request.opponents,
        "opponent_weights": &request.opponent_weights,
        "constraints": &request.constraints,
        "population_size": request.population_size,
        "generations": request.generations,
        "games_per_policy": request.games_per_policy,
        "maximum_swaps": request.maximum_swaps,
        "seed": request.seed,
        "workers": request.workers,
        "parallel_jobs": request.parallel_jobs,
        "hat_budget": request.hat_budget,
        "max_turns": request.max_turns,
        "game_timeout_seconds": request.game_timeout_seconds,
    });
    let result = tauri::async_runtime::spawn_blocking(move || {
        let report =
            sideboard_evolution::evolve_sideboard_policies(&data_dir, &request, &cancellation)
                .map_err(|error| error.to_string())?;
        runs::save_sideboard_evolution(
            &data_dir,
            run_id.as_deref(),
            candidate_rank,
            &compact_request,
            &report,
        )
        .map_err(|error| error.to_string())?;
        Ok(report)
    })
    .await
    .map_err(|error| error.to_string());
    if let Ok(mut active) = active_state.lock() {
        *active = None;
    }
    result?
}

#[tauri::command]
fn cancel_sideboard_evolution(state: State<'_, AppState>) -> Result<(), String> {
    let active = state
        .sideboard_evolution
        .lock()
        .map_err(|_| "Sideboard evolution state is unavailable".to_string())?;
    if let Some(cancellation) = active.as_ref() {
        cancellation.cancel();
    }
    Ok(())
}

#[tauri::command]
async fn run_forge_validation(
    mut request: simulator::HexdekValidationRequest,
    run_id: Option<String>,
    candidate_rank: Option<usize>,
    state: State<'_, AppState>,
) -> Result<simulator::SimulationReport, String> {
    require_structural_validation(
        &request.candidate,
        &request.pool,
        request.commander,
        request.seed,
        request.max_turns,
    )?;
    require_saved_run_tune_validation(
        &state.data_dir,
        run_id.as_deref(),
        &request.candidate,
        &request.pool,
        request.commander,
        request.seed,
        request.max_turns,
    )?;
    request.opponent_weights = if request.commander {
        vec![1.0; request.opponents.len()]
    } else {
        normalize_opponent_weights(&request.opponent_weights, request.opponents.len())?
    };
    let data_dir = state.data_dir.clone();
    let cancellation = CancellationToken::default();
    {
        let mut active = state
            .forge_validation
            .lock()
            .map_err(|_| "Forge validation state is unavailable".to_string())?;
        if active.is_some() {
            return Err("A Forge validation is already running".into());
        }
        *active = Some(cancellation.clone());
    }
    let forge_validation = state.forge_validation.clone();
    let compact_request = serde_json::json!({
        "engine": "forge",
        "engine_revision": "2.0.14-a37a865",
        "candidate": &request.candidate,
        "opponents": &request.opponents,
        "opponent_weights": &request.opponent_weights,
        "games": request.games,
        "seed": request.seed,
        "seats": request.seats,
        "game_timeout_seconds": request.game_timeout_seconds,
        "commander": request.commander,
    });
    let result = tauri::async_runtime::spawn_blocking(move || {
        let report = forge::run_validation_cancellable(&data_dir, &request, &cancellation)
            .map_err(|error| error.to_string())?;
        runs::save_validation(
            &data_dir,
            run_id.as_deref(),
            candidate_rank,
            &compact_request,
            &report,
        )
        .map_err(|error| error.to_string())?;
        Ok(report)
    })
    .await
    .map_err(|error| error.to_string());
    if let Ok(mut active) = forge_validation.lock() {
        *active = None;
    }
    result?
}

#[tauri::command]
fn cancel_forge_validation(state: State<'_, AppState>) -> Result<(), String> {
    let active = state
        .forge_validation
        .lock()
        .map_err(|_| "Forge validation state is unavailable".to_string())?;
    if let Some(cancellation) = active.as_ref() {
        cancellation.cancel();
    }
    Ok(())
}

#[tauri::command]
fn validation_history(
    run_id: Option<String>,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<Vec<SavedValidation>, String> {
    runs::list_validations(&state.data_dir, run_id.as_deref(), limit)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|row| {
            Ok(SavedValidation {
                id: row.id,
                run_id: row.run_id,
                candidate_rank: row.candidate_rank,
                created_at: row.created_at,
                request: serde_json::from_str(&row.request_json)
                    .map_err(|error| error.to_string())?,
                report: serde_json::from_str(&row.report_json)
                    .map_err(|error| error.to_string())?,
            })
        })
        .collect()
}

#[tauri::command]
fn best_of_three_history(
    run_id: Option<String>,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<Vec<SavedBestOfThree>, String> {
    runs::list_best_of_three(&state.data_dir, run_id.as_deref(), limit)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|row| {
            Ok(SavedBestOfThree {
                id: row.id,
                run_id: row.run_id,
                candidate_rank: row.candidate_rank,
                created_at: row.created_at,
                request: serde_json::from_str(&row.request_json)
                    .map_err(|error| error.to_string())?,
                report: serde_json::from_str(&row.report_json)
                    .map_err(|error| error.to_string())?,
            })
        })
        .collect()
}

#[tauri::command]
fn sideboard_evolution_history(
    run_id: Option<String>,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<Vec<SavedSideboardEvolution>, String> {
    runs::list_sideboard_evolutions(&state.data_dir, run_id.as_deref(), limit)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|row| {
            Ok(SavedSideboardEvolution {
                id: row.id,
                run_id: row.run_id,
                candidate_rank: row.candidate_rank,
                created_at: row.created_at,
                request: serde_json::from_str(&row.request_json)
                    .map_err(|error| error.to_string())?,
                report: serde_json::from_str(&row.report_json)
                    .map_err(|error| error.to_string())?,
            })
        })
        .collect()
}

fn persist_halving_evidence(
    data_dir: &std::path::Path,
    run_id: Option<&str>,
    request: &simulator::SuccessiveHalvingRequest,
    report: &simulator::SuccessiveHalvingReport,
    phase: &str,
    cycle: Option<usize>,
) -> Result<(), String> {
    let compact_request = serde_json::json!({
        "phase": phase,
        "cycle": cycle,
        "candidate_ranks": request.candidates.iter().map(|candidate| candidate.rank).collect::<Vec<_>>(),
        "opponents": &request.opponents,
        "opponent_weights": &request.opponent_weights,
        "base_games": request.base_games,
        "rounds": request.rounds,
        "seed": request.seed,
        "workers": request.workers,
        "seats": request.seats,
        "hat_budget": request.hat_budget,
        "max_turns": request.max_turns,
        "game_timeout_seconds": request.game_timeout_seconds,
        "structural_weight": request.structural_weight,
        "risk_aversion": request.risk_aversion,
        "worst_matchup_weight": request.worst_matchup_weight,
        "seat_position_weight": request.seat_position_weight,
        "adaptive_elimination": request.adaptive_elimination,
        "commander": request.commander,
    });
    for round in &report.rounds {
        for result in &round.results {
            let Some(simulation_report) = &result.report else {
                continue;
            };
            let Some(candidate) = request
                .candidates
                .iter()
                .find(|candidate| candidate.rank == result.candidate_rank)
            else {
                continue;
            };
            let evidence_request = serde_json::json!({
                "phase": phase,
                "cycle": cycle,
                "round": round.round,
                "candidate": &candidate.deck,
                "opponents": &request.opponents,
                "opponent_weights": &request.opponent_weights,
                "games": round.games_per_candidate,
                "seed": request.seed.wrapping_add((round.round.saturating_sub(1) as i64).wrapping_mul(1_000_003)),
                "workers": request.workers,
                "seats": request.seats,
                "hat_budget": request.hat_budget,
                "max_turns": request.max_turns,
                "game_timeout_seconds": request.game_timeout_seconds,
                "structural_weight": request.structural_weight,
                "risk_aversion": request.risk_aversion,
                "worst_matchup_weight": request.worst_matchup_weight,
                "seat_position_weight": request.seat_position_weight,
                "adaptive_elimination": request.adaptive_elimination,
                "commander": request.commander,
                "blended_score": result.blended_score,
                "evidence_score": result.evidence_score,
                "win_rate_lower_bound": result.win_rate_lower_bound,
                "worst_matchup_lower_bound": result.worst_matchup_lower_bound,
                "worst_matchup_upper_bound": result.worst_matchup_upper_bound,
                "worst_position_lower_bound": result.worst_position_lower_bound,
                "worst_position_upper_bound": result.worst_position_upper_bound,
                "position_win_rate_spread": result.position_win_rate_spread,
                "evidence_lower_bound": result.evidence_lower_bound,
                "evidence_upper_bound": result.evidence_upper_bound,
                "blended_lower_bound": result.blended_lower_bound,
                "blended_upper_bound": result.blended_upper_bound,
            });
            runs::save_validation(
                data_dir,
                run_id,
                Some(result.candidate_rank),
                &evidence_request,
                simulation_report,
            )
            .map_err(|error| error.to_string())?;
        }
    }
    runs::save_halving(data_dir, run_id, &compact_request, report)
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
async fn run_hexdek_halving(
    mut request: simulator::SuccessiveHalvingRequest,
    run_id: Option<String>,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<simulator::SuccessiveHalvingReport, String> {
    for candidate in &request.candidates {
        require_structural_validation(
            &candidate.deck,
            &request.pool,
            request.commander,
            request.seed,
            request.max_turns,
        )?;
        require_saved_run_tune_validation(
            &state.data_dir,
            run_id.as_deref(),
            &candidate.deck,
            &request.pool,
            request.commander,
            request.seed,
            request.max_turns,
        )?;
    }
    request.opponent_weights = if request.commander {
        vec![1.0; request.opponents.len()]
    } else {
        normalize_opponent_weights(&request.opponent_weights, request.opponents.len())?
    };
    let data_dir = state.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let report = simulator::run_successive_halving(&data_dir, &request, |progress| {
            let _ = app.emit("halving-progress", progress);
        })
        .map_err(|error| error.to_string())?;
        persist_halving_evidence(
            &data_dir,
            run_id.as_deref(),
            &request,
            &report,
            "successive_halving",
            None,
        )?;
        Ok(report)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn halving_history(
    run_id: Option<String>,
    limit: usize,
    state: State<'_, AppState>,
) -> Result<Vec<SavedHalving>, String> {
    runs::list_halvings(&state.data_dir, run_id.as_deref(), limit)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|row| {
            Ok(SavedHalving {
                id: row.id,
                run_id: row.run_id,
                created_at: row.created_at,
                request: serde_json::from_str(&row.request_json)
                    .map_err(|error| error.to_string())?,
                report: serde_json::from_str(&row.report_json)
                    .map_err(|error| error.to_string())?,
            })
        })
        .collect()
}

#[tauri::command]
async fn install_hexdek_engine(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<simulator::SimulatorStatus, String> {
    let data_dir = state.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        simulator::install_hexdek(&data_dir, |progress| {
            let _ = app.emit("simulator-install-progress", progress);
        })
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn install_forge_engine(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<simulator::SimulatorStatus, String> {
    let data_dir = state.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        forge::install(&data_dir, |progress| {
            let _ = app.emit("simulator-install-progress", progress);
        })
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            runs::recover_interrupted(&data_dir)?;
            let _ = resource_governor::status(&data_dir);
            app.manage(AppState::new(data_dir));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            system_profile,
            resource_budget_status,
            save_resource_budgets,
            catalog_status,
            sync_catalog,
            search_cards,
            new_workspace,
            import_workspace,
            import_gauntlet_deck,
            acquire_metagame,
            acquire_competitive_field,
            build_engine_route_plan,
            combo_corpus_status,
            sync_combo_corpus,
            prove_deck_combos,
            combo_conformance_scenario,
            record_combo_conformance_observation,
            save_gauntlet,
            gauntlet_history,
            load_gauntlet,
            delete_gauntlet,
            hall_of_fame_status,
            promote_hall_of_fame_candidate,
            delete_hall_of_fame_entry,
            calibration_status,
            import_match_history,
            match_history,
            update_match_record,
            delete_match_records,
            clear_match_history,
            export_match_history,
            feedback_summary,
            suggest_feedback_reweights,
            surrogate_status,
            train_surrogate_model,
            benchmark_surrogate_device,
            compare_gauntlets,
            export_metagame_bundle,
            import_metagame_bundle,
            ground_evolution_from_field,
            start_evolution,
            continue_evolution,
            resume_evolution,
            start_hybrid_evolution,
            evolution_status,
            cancel_evolution,
            export_deck,
            run_history,
            update_run_metadata,
            export_run_manifest,
            storage_health,
            create_local_backup,
            restore_local_backup,
            repair_corrupt_database,
            export_safe_diagnostics,
            load_saved_run,
            simulator_status,
            simulation_cache_status,
            clear_simulation_cache,
            card_art,
            art_cache_status,
            clear_art_cache,
            conformance_summary,
            audit_oracle_accounting,
            audit_training_labels,
            rebuild_engine_capability_manifest,
            audit_format_capability,
            commander_conformance,
            import_conformance_bundle,
            deck_conformance,
            card_conformance,
            hexdek_deck_coverage,
            install_hexdek_engine,
            install_forge_engine,
            run_hexdek_validation,
            run_hexdek_best_of_three,
            evolve_sideboard_plans,
            cancel_sideboard_evolution,
            run_forge_validation,
            cancel_forge_validation,
            validation_history,
            best_of_three_history,
            sideboard_evolution_history,
            run_hexdek_halving,
            halving_history
        ])
        .run(tauri::generate_context!())
        .expect("error while running Metagene");
}
