export type CardId = string;

export interface ManaCostProfile { generic: number; colored: number[]; hybrid: string[][]; variable: boolean; }
export type ManaColorRule = "Fixed" | "CommanderIdentity" | "LegendaryPermanents";
export type ManaSpendRule = "Unrestricted" | "CastLegendary" | { CastCardTypes: string[] } | { CastSubtypes: string[] } | "CastChosenCreatureType" | "UnknownRestricted";
export type ManaStateRule = "None" | "RemoveCounter";
export interface ManaSourceProfile { colors: string[]; color_rule: ManaColorRule; spend_rule: ManaSpendRule; state_rule: ManaStateRule; uses_tap: boolean; activation_generic: number; earliest_turn: number; label: string; }
export type AbilityKind = "Unknown" | "Draw" | "Destroy" | "Exile" | "Bounce" | "CounterSpell" | "AddCounters" | "GraveyardHate" | "Recursion" | "Search" | "CreateToken" | "Copy" | "TokenMultiplier" | "SacrificeOutlet" | "Ramp" | "Crew" | "Damage" | "Mill" | "AlternateWin" | "Blink";
export type AbilityTiming = "Spell" | "Activated" | "Triggered" | "Static";
export type AbilityZone = "Battlefield" | "Hand" | "Graveyard" | "Stack" | "Library" | "Exile" | "Command";
export type ActionTargetProfile = "None" | "OwnCreature" | "OpponentCreature" | "AnyCreature" | "ArtifactOrEnchantment" | "GraveyardCard" | "OwnGraveyardCreature" | "Spell" | "BasicLandInLibrary" | "OpponentPlayer";
export type ClauseClassification = "ParsedGameplayRule" | "RecognizedKeywordRule" | "ReminderText" | "UnresolvedGameplayResidual";
export type ClauseCriticality = "RuleCritical" | "OutcomeCritical" | "Informational";
export interface OracleClause { id: string; mode_name: string; ordinal: number; text: string; normalized_text: string; classification: ClauseClassification; criticality: ClauseCriticality; parser_confidence: number; action_ids: string[]; }
export interface OracleCoverageSummary { gameplay_clauses: number; accounted_clauses: number; parsed_clauses: number; unresolved_clauses: number; accounted_clause_rate: number; parsed_clause_rate: number; unresolved_clause_rate: number; unresolved_text: string[]; }
export interface AbilityProfile { clause_ids: string[]; label: string; kind: AbilityKind; timing: AbilityTiming; zone: AbilityZone; cost: { mana: ManaCostProfile; uses_tap: boolean; sacrifices_self: boolean; sacrifice_permanents: number; discards_self: boolean; discard_cards: number; exiles_self: boolean; exile_cards: number; life: number; remove_counters: number }; target: ActionTargetProfile; conditions: string[]; effect_value: number; narrow: boolean; once_per_turn: boolean; parser_confidence: number; }
export type RuleTiming = "Spell" | "Trigger" | "Activation" | "Static" | "Replacement" | "Mana" | "SpecialAction";
export type AmountExpression = "One" | "X" | { Constant: number } | { Variable: string } | { Counted: string };
export interface TokenProfile { name: string; types: string[]; subtypes: string[]; colors: string[]; power?: AmountExpression; toughness?: AmountExpression; keywords: string[]; tapped: boolean; attacking: boolean; copy_of_target: boolean; }
export interface CounterProfile { name: string; amount: AmountExpression; }
export type RuleCost = "None" | "TapSource" | { Mana: ManaCostProfile } | { Sacrifice: { object: string; amount: AmountExpression } } | { Discard: { object: string; amount: AmountExpression } } | { Exile: { object: string; amount: AmountExpression } } | { PayLife: AmountExpression } | { RemoveCounters: CounterProfile } | { Additional: string };
export type RuleCondition = "Always" | { Event: string } | { InterveningIf: string } | { AsLongAs: string } | { Unless: string } | { Limit: string } | { Text: string };
export interface RuleTarget { controller: string; zones: AbilityZone[]; card_types: string[]; subtypes: string[]; restrictions: string[]; amount: AmountExpression; optional: boolean; }
export type RuleEffect = "NoOp" | "Transform" | "ExtraCombat" | "ExtraTurn" | { MoveCard: { from: AbilityZone; to: AbilityZone; object: string } } | { Draw: { player: string; amount: AmountExpression } } | { Discard: { player: string; amount: AmountExpression } } | { Life: { player: string; amount: AmountExpression; gain: boolean } } | { ModifyPowerToughness: { power: AmountExpression; toughness: AmountExpression } } | { ModifyTypes: { add: string[]; remove: string[] } } | { GrantKeyword: { keyword: string } } | { Tap: { untap: boolean } } | { AddMana: { colors: string[]; amount: AmountExpression } } | { PutCounters: CounterProfile } | { CreateToken: { token: TokenProfile; amount: AmountExpression } } | { Damage: { recipient: string; amount: AmountExpression } } | { Prevent: { event: string; amount: AmountExpression } } | { Destroy: { object: string } } | { Sacrifice: { object: string } } | { Exile: { object: string } } | { Search: { zone: AbilityZone; destination: AbilityZone; object: string } } | { CastPermission: { zone: AbilityZone; object: string } } | { CostChange: { object: string; amount: AmountExpression; increase: boolean } } | { ControlChange: { object: string; duration: string } } | { Copy: { object: string; as_token: boolean } } | { Replace: { event: string; replacement: string } } | { WinLoss: { player: string; wins: boolean } } | { Mill: { player: string; amount: AmountExpression } } | { Perpetual: { modification: string } } | { Conjure: { object: string; destination: AbilityZone } } | { Seek: { object: string } } | { Text: string };
export interface RuleAbility { id: string; clause_ids: string[]; label: string; timing: RuleTiming; source_zones: AbilityZone[]; costs: RuleCost[]; targets: RuleTarget[]; conditions: RuleCondition[]; choices: string[]; effects: RuleEffect[]; duration?: string; delayed_effects: RuleEffect[]; once_per_turn: boolean; once_per_use: boolean; parser_confidence: number; }
export interface CardMode { name: string; type_line: string; colors: string[]; mana_cost: ManaCostProfile; normal_castable?: boolean | null; is_land: boolean; is_legendary: boolean; creature_types: string[]; mana_source_colors: string[]; late_mana_source_colors: string[]; mana_sources: ManaSourceProfile[]; abilities: AbilityProfile[]; rule_abilities: RuleAbility[]; oracle_clauses: OracleClause[]; enters_tapped: boolean; }
export type SearchTargetProfile = "None" | "BasicLand" | { BasicLandOrSubtypes: string[] } | { CardSubtypes: string[] };
export interface SearchActionProfile { mana_cost: ManaCostProfile; from_hand: boolean; discards_self: boolean; target: SearchTargetProfile; }
export interface MechanicProfile { requires_creature: boolean; requires_humans: boolean; requires_death: boolean; provides_death: boolean; exile_cost: boolean; reanimation: boolean; graveyard_setup: boolean; legendary_payoff: boolean; crew_power: number; devotion_threshold: number; devotion_color_mask: number; searches_basic_land: boolean; search_action?: SearchActionProfile; opponent_discard_supply: number; opponent_discard_demand: number; sacrifice_outlet_supply: number; sacrifice_fodder_supply: number; sacrifice_demand: number; graveyard_target_supply: number; graveyard_target_demand: number; graveyard_setup_supply: number; graveyard_setup_demand: number; legendary_cast_demand: number; }
export interface MechanicDescriptor { id: string; label: string; source: string; critical: boolean; criticality: ClauseCriticality; parser_confidence: number; }
export interface CardFeatures {
  is_land: boolean;
  unlimited_copies: boolean;
  enters_tapped: boolean;
  mana_value: number;
  mana_production: number;
  ramp: number;
  card_advantage: number;
  interaction: number;
  pressure: number;
  resilience: number;
  tags: string[];
  is_legendary: boolean;
  can_be_land: boolean;
  initial_modes: CardMode[];
  mechanics: MechanicProfile;
  mechanic_inventory: MechanicDescriptor[];
  oracle_coverage: OracleCoverageSummary;
}

export interface Card {
  id: CardId;
  arena_id?: number;
  platforms: string[];
  name: string;
  set_code: string;
  collector_number: string;
  type_line: string;
  oracle_text: string;
  colors: string[];
  color_identity: string[];
  image_uri?: string;
  legalities: Record<string, string>;
  features: CardFeatures;
}

export interface DeckEntry {
  card_id: CardId;
  quantity: number;
}

export interface Deck {
  name: string;
  command_zone: DeckEntry[];
  mainboard: DeckEntry[];
  sideboard: DeckEntry[];
}

export interface Fitness {
  score: number;
  consistency: number;
  tempo: number;
  synergy: number;
  interaction: number;
  resilience: number;
  mana_failure_rate: number;
  required_color_count: number;
  color_complexity_penalty: number;
  commander_quality: number;
  sideboard_quality: number;
  severe_mana_failure: boolean;
  mana_blocking_reasons: string[];
  early_sources: number[];
  late_sources: number[];
  unrestricted_early_sources: number[];
  conditional_early_sources: number[];
  unusable_sources: number[];
  colored_pip_demand: number[];
  on_curve_castability: number;
  turn_horizon_castability: number;
  stranded_card_rate: number;
  legendary_dead_draw_rate: number;
  unmet_requirement_rate: number;
  legendary_penalty: number;
  unsupported_synergy_penalty: number;
  card_castability: CardCastabilityEvidence[];
  requirement_evidence: RequirementEvidence[];
  synergy_warnings: string[];
  action_evidence: ActionEvidence[];
  strategy_packages: StrategyPackageEvidence[];
  role_evidence: RoleEvidence[];
  copy_evidence: CopyEvidence[];
  functional_action_rate: number;
  accounted_clause_rate: number;
  parsed_clause_rate: number;
  unresolved_clause_rate: number;
  unresolved_oracle_text: string[];
  dead_action_rate: number;
  narrow_mainboard_rate: number;
  copy_excess_rate: number;
  closing_power: number;
  role_balance: number;
  combo_assembly_probability: number;
  combo_expected_turn?: number;
  combo_dead_card_rate: number;
  combo_resilience: number;
  combo_execution_rate: number;
  combo_proof_level: string;
  archetype: string;
  strategy_similarity: number;
  nonland_edit_fraction: number;
  severe_structural_failure: boolean;
  structural_blocking_reasons: string[];
}

export interface ActionEvidence { card_id: CardId; card_name: string; action: string; kind: string; zone: string; conditions: string[]; copies: number; availability: number; mana_availability: number; target_availability: number; contribution: number; narrow: boolean; reasons: string[]; }
export interface StrategyPackageEvidence { package: string; primary: boolean; root_support: number; candidate_support: number; retention: number; }
export interface RoleEvidence { role: string; copies: number; target: number; fulfillment: number; }
export interface CopyEvidence { card_id: CardId; card_name: string; copies: number; functional_target: number; rationale: string; excess_copies: number; }
export interface StrategySignature { version: number; packages: { package: string; primary: boolean; root_support: number; root_unique_cards: number; required_roles: string[] }[]; root_nonland_copies: number; }
export interface PromotionEvidence { status: "baseline" | "provisional" | "validated" | "improved" | string; incumbent_rank?: number; challenger_rank?: number; promoted_rank?: number; selection_gain?: number; holdout_gain?: number; forge_gain?: number; single_engine_validated: boolean; reason: string; }

export interface RequirementEvidence {
  mechanic: string;
  demand: number;
  supply: number;
  fulfillment: number;
}

export interface CardCastabilityEvidence {
  card_id: CardId;
  name: string;
  copies: number;
  on_curve_castability: number;
  turn_horizon_castability: number;
  unrestricted_early_sources: number;
  conditional_early_sources: number;
  unusable_sources: number;
  source_restrictions: string[];
  reasons: string[];
}

export interface RankedDeck {
  rank: number;
  pareto_rank: number;
  deck: Deck;
  fitness: Fitness;
  packages: { tag: string; copies: number; unique_cards: number }[];
  commander_compatibility?: { policy_id: string; schema_version: number; rule_id: string };
  candidate_status: "" | "baseline" | "provisional" | "validated" | "improved" | "niche_elite";
}

export interface MapEliteDescriptor {
  archetype: string;
  colors: string;
  curve: string;
  interaction: string;
  victory_path: string;
}

export interface MapEliteCell {
  descriptor: MapEliteDescriptor;
  candidate: RankedDeck;
}

export interface HallOfFameEntry {
  id: string;
  platform: string;
  format: string;
  commander: boolean;
  candidate: RankedDeck;
  evidence_score: number;
  novelty: number;
  source_run_id?: string;
  source_cycle?: number;
  promotion_count: number;
  created_at: string;
  last_seen_at: string;
}

export interface HallOfFameStatus {
  entries: HallOfFameEntry[];
  active_diverse_ids: string[];
  maximum_entries_for_scope: number;
}

export interface CalibrationStatus {
  observations: number;
  empirical_interval_coverage: number;
  target_interval_coverage: number;
  mean_generalization_gap: number;
  mean_absolute_error: number;
  selection_agreement?: number;
  recommended_risk_aversion_delta: number;
  state: "collecting" | "undercovered" | "overconservative" | "calibrated";
  database_bytes: number;
}

export interface SurrogateStatus {
  labels: number;
  minimum_labels: number;
  ready: boolean;
  stale: boolean;
  recent_mae?: number;
  historical_mae?: number;
  gpu_training_enabled: boolean;
  gpu_decision: string;
  database_bytes: number;
  manifest?: SurrogateManifest;
  device?: DeviceProfile;
  trustworthy_only: boolean;
}

export interface SurrogateManifest {
  schema_version: number;
  model_revision: string;
  trained_at: number;
  feature_revision: string;
  platform: string;
  format: string;
  commander: boolean;
  excluded_run_id?: string;
  labels: number;
  lineage_field_groups: number;
  validation_labels: number;
  validation_groups: number;
  validation_mae: number;
  validation_log_loss: number;
  baseline_log_loss: number;
  group_isolated: boolean;
  holdout_isolation: string;
}

export interface DeviceProfile {
  schema_version: number;
  benchmarked_at: number;
  cpu_backend: string;
  gpu_backend: string;
  gpu_available: boolean;
  cpu_predictions_per_second: number;
  gpu_predictions_per_second: number;
  speedup: number;
  maximum_absolute_difference: number;
  calibration_gate_passed: boolean;
  closed_loop_quality_per_minute_gain: number;
  gpu_enabled: boolean;
  decision: string;
}

export interface MatchRecord {
  id: string;
  source: string;
  external_match_id?: string;
  revision_id?: string;
  deck_name?: string;
  platform?: string;
  format?: string;
  occurred_at?: string;
  result: "win" | "loss" | "draw";
  opponent_archetype: string;
  play_draw: "play" | "draw" | "unknown";
  sideboarded?: boolean;
  notes: string;
  predicted_win_rate?: number;
  identity_status: "exact" | "quarantined";
  imported_at: string;
}

export interface MatchImportReport {
  imported: number;
  exact_revision_matches: number;
  quarantined_identity: number;
  duplicates: number;
  ignored_lines: number;
  records: MatchRecord[];
}

export interface FeedbackSummary {
  matches: number;
  exact_revision_matches: number;
  wins: number;
  losses: number;
  draws: number;
  observed_win_rate: number;
  predicted_win_rate?: number;
  prediction_error?: number;
  by_archetype: { archetype: string; games: number; observed_win_rate: number; predicted_win_rate?: number }[];
}

export interface ReweightSuggestion {
  opponent: string;
  current_weight: number;
  suggested_weight: number;
  matched_games: number;
  reason: string;
}

export interface EvolutionConfig {
  population_size: number;
  generations: number;
  elite_count: number;
  trials_per_candidate: number;
  mutation_swaps: number;
  mutation_rate: number;
  package_mutation_rate: number;
  package_size: number;
  mana_mutation_rate: number;
  tournament_size: number;
  seed: number;
  turn_horizon: number;
  worker_threads?: number;
  evolve_commanders: boolean;
  evolve_sideboard: boolean;
  commander_mutation_rate: number;
  learned_packages: CardId[][];
  combo_packages?: { id: string; piece_ids: CardId[]; tutor_ids: CardId[]; protection_ids: CardId[]; recovery_ids: CardId[]; required_roles: string[]; expected_turn?: number; repeatable: boolean; finite_win: boolean; winning: boolean; proof: string }[];
  fitness_weights: {
    consistency: number;
    tempo: number;
    synergy: number;
    interaction: number;
    resilience: number;
    commander: number;
    sideboard: number;
    mana_failure_penalty: number;
    legendary_dead_draw_penalty: number;
    unsupported_synergy_penalty: number;
    dead_action_penalty: number;
    narrow_card_penalty: number;
    closing_power: number;
    role_balance: number;
  };
  evolution_mode: "tune" | "rebuild";
  tune_policy: { root_deck?: Deck; strategy_retention_floor: number; primary_package_floor: number; maximum_nonland_edit_fraction: number };
}

export interface RunRequest {
  platform: string;
  format: string;
  simulator_coverage: "all" | "present" | "parser_complete";
  pool_filter: string[];
  exclude_disputed_cards: boolean;
  exclude_unsupported_cards: boolean;
  pool: { cards: Card[] };
  seed_deck: Deck;
  constraints: {
    target_mainboard_size: number;
    target_sideboard_size: number;
    minimum_commanders: number;
    maximum_commanders: number;
    default_max_copies: number;
    locked_cards: { card_id: CardId; minimum: number; maximum: number }[];
    minimum_lands?: number;
    maximum_lands?: number;
    allowed_colors?: ("White" | "Blue" | "Black" | "Red" | "Green" | "Colorless")[];
  };
  config: EvolutionConfig;
  opponent_field: { available: boolean; creature_rate: number; artifact_or_enchantment_rate: number; graveyard_rate: number; stack_spell_rate: number };
}

export interface GenerationProgress {
  generation: number;
  total_generations: number;
  evaluated_candidates: number;
  unique_evaluations: number;
  evaluations_avoided: number;
  unique_in_generation: number;
  duplicates_in_generation: number;
  elapsed_ms: number;
  candidates_per_second: number;
  unique_evaluations_per_second: number;
  best: RankedDeck;
  top: RankedDeck[];
  baseline?: RankedDeck;
  strategy_signature?: StrategySignature;
  structural_model_revision: string;
  map_elite_cells?: number;
}

export interface RunResult {
  generations_completed: number;
  evaluated_candidates: number;
  unique_evaluations: number;
  evaluations_avoided: number;
  elapsed_ms: number;
  cancelled: boolean;
  frontier: RankedDeck[];
  baseline?: RankedDeck;
  strategy_signature?: StrategySignature;
  promotion_evidence?: PromotionEvidence;
  structural_model_revision: string;
  map_elites?: MapEliteCell[];
}

export interface JobStatus {
  id: string;
  state: "running" | "completed" | "failed" | "cancelled" | "interrupted";
  progress?: GenerationProgress;
  result?: RunResult;
  error?: string;
}

export interface SystemProfile {
  cpu_name: string;
  physical_cores: number;
  logical_cores: number;
  memory_gib: number;
  gpu_name?: string;
  gpu_memory_gib?: number;
  recommended_workers: number;
}

export interface ResourceBudgets {
  cpu_threads: number;
  max_engine_processes: number;
  memory_mib: number;
  run_time_seconds: number;
  disk_mib: number;
}

export interface ResourceBudgetStatus {
  budgets: ResourceBudgets;
  app_data_bytes: number;
  process_memory_bytes: number;
  orphan_job_directories_removed: number;
}

export interface BackupSummary { id: string; bytes: number; created_at: string; }
export interface StorageHealth {
  schema_version: number;
  databases: { name: string; bytes: number; healthy: boolean; detail: string }[];
  backups: BackupSummary[];
}

export interface CatalogStatus {
  ready: boolean;
  card_count: number;
  updated_at?: string;
  database_bytes: number;
  catalog_version: number;
}

export interface CatalogProgress {
  stage: string;
  processed: number;
  retained: number;
  downloaded_bytes: number;
  total_bytes?: number;
}

export interface ArtCacheStatus {
  entries: number;
  bytes: number;
  max_bytes: number;
}

export interface RunSummary {
  id: string;
  created_at: string;
  platform: string;
  format: string;
  state: string;
  best_score?: number;
  evaluated_candidates: number;
  elapsed_ms: number;
  title: string;
  notes: string;
  tags: string[];
}

export interface SavedRun {
  request: RunRequest;
  status: JobStatus;
  resumable: boolean;
  remaining_generations: number;
  resumed_from?: string;
  run_kind: "structural" | "hybrid";
  remaining_cycles: number;
  hybrid_phase?: "structural" | "validation" | "cycle_complete";
  hybrid_current_cycle?: number;
  hybrid_gauntlet_provenance?: GauntletProvenance;
}

export interface ResumedRun {
  id: string;
  request: RunRequest;
  resumed_from: string;
  resumed_generation: number;
  run_kind: "structural" | "hybrid";
  remaining_cycles: number;
}

export interface ContinuedRun {
  id: string;
  request: RunRequest;
  continued_from: string;
  status: JobStatus;
  generations_completed: number;
}

export interface GauntletSummary {
  id: string;
  name: string;
  platform: string;
  format: string;
  commander: boolean;
  revision: number;
  deck_count: number;
  created_at: string;
}

export interface SavedGauntlet extends GauntletSummary {
  decks: Deck[];
  weights: number[];
  metadata: GauntletMetadata;
}

export interface GauntletComparison {
  from: GauntletSummary;
  to: GauntletSummary;
  added: string[];
  removed: string[];
  unchanged: number;
  weight_changes: { name: string; from_weight: number; to_weight: number }[];
}

export interface GauntletMetadata {
  source: string;
  observed_at: string;
  notes: string;
  source_url?: string;
  retrieved_at_unix?: number;
  event_start_unix?: number;
  event_end_unix?: number;
  sample_size?: number;
  transformation_notes?: string[];
  field_revision_id?: string;
  source_tier?: "tournament" | "public_list_fallback" | string;
  competitive_claims_allowed?: boolean;
  archetype_labels?: string[];
  quarantined_summary?: string[];
}

export interface ImportedMetagameBundle {
  name: string;
  platform: string;
  format: string;
  commander: boolean;
  decks: Deck[];
  weights: number[];
  metadata: GauntletMetadata;
}

export interface MetagameAcquisitionReport {
  schema_version: number;
  source: string;
  platform: string;
  format: string;
  commander: boolean;
  provenance: {
    adapter: string;
    source_url: string;
    retrieved_at_unix: number;
    event_start_unix?: number;
    event_end_unix?: number;
    event_count: number;
    submitted_deck_count: number;
    accepted_deck_count: number;
    transformation_notes: string[];
  };
  accepted: {
    archetype: string;
    deck: Deck;
    decklist: string;
    occurrences: number;
    weight: number;
    fingerprint: string;
    provenance: { event_id: string; event_name: string; event_url: string; event_start_unix: number }[];
  }[];
  quarantined: {
    event_id: string;
    event_name: string;
    submitted_name: string;
    reason: "missing_decklist" | "stale_event" | "format_mismatch" | "unresolvable_card" | "ambiguous_decklist" | "illegal_deck" | "malformed_source";
    detail: string;
    decklist?: string;
  }[];
}

export type StrategyArchetype = "aggro" | "midrange" | "control" | "combo" | "graveyard";

export interface ArchetypeCluster {
  id: string;
  label: string;
  strategy: StrategyArchetype;
  representative: Deck;
  representative_fingerprint: string;
  member_fingerprints: string[];
  occurrences: number;
  observed_weight: number;
  normalized_selected_weight: number;
}

export interface FieldAudit {
  schema_version: number;
  source_tier: "tournament" | "public_list_fallback";
  competitive_claims_allowed: boolean;
  observed_weight_covered: number;
  submitted_decks: number;
  accepted_decks: number;
  selected_archetypes: number;
  exact_duplicates_merged: number;
  near_duplicates_clustered: number;
  strategy_coverage: StrategyArchetype[];
  quarantined: MetagameAcquisitionReport["quarantined"];
  warnings: string[];
}

export interface MetagameFieldRevision {
  schema_version: number;
  revision_id: string;
  platform: string;
  format: string;
  commander: boolean;
  provenance: MetagameAcquisitionReport["provenance"];
  clusters: ArchetypeCluster[];
  decks: Deck[];
  weights: number[];
  audit: FieldAudit;
}

export interface CompetitiveFieldAcquisition {
  field: MetagameFieldRevision;
  saved: SavedGauntlet;
  source_attempts: string[];
}

export type MechanicCapabilityState = "unsupported" | "parsed" | "executable" | "scenario_verified" | "disputed";

export interface MechanicCapability {
  card_id: string;
  card_name: string;
  mode_name: string;
  subject_id: string;
  action: string;
  critical: boolean;
  criticality: ClauseCriticality;
  parser_confidence: number;
  high_risk: boolean;
  ai_decision_covered: boolean;
  state: MechanicCapabilityState;
  evidence: string[];
}

export interface DeckCapabilityReport {
  schema_version: number;
  engine: string;
  deck_name: string;
  card_copies: number;
  trusted_card_copies: number;
  trusted_copy_ratio: number;
  critical_actions: number;
  trusted_critical_actions: number;
  critical_coverage: number;
  oracle_accounted_rate: number;
  structurally_parsed_rate: number;
  engine_executable_rate: number;
  scenario_verified_rate: number;
  ai_decision_coverage: number;
  unresolved_clauses: string[];
  capabilities: MechanicCapability[];
  blockers: string[];
}

export type EvidenceTier = "screened" | "coverage_checked" | "provisional" | "field_tested" | "single_engine_validated" | "mixed_route_validated" | "cross_engine_verified" | "improved";

export interface EngineRoutePlan {
  schema_version: number;
  capability_policy_revision: string;
  rules_ir_revision: string;
  plan_id: string;
  candidate: DeckCapabilityReport;
  candidate_by_engine: DeckCapabilityReport[];
  opponent_reports: DeckCapabilityReport[];
  routes: {
    opponent_index: number;
    opponent_name: string;
    weight: number;
    engine?: "hexdek" | "forge";
    eligible: boolean;
    reasons: string[];
  }[];
  field_weighted_coverage: number;
  candidate_critical_coverage: number;
  locked: boolean;
  claim: {
    tier: EvidenceTier;
    promotable: boolean;
    label: string;
    reasons: string[];
  };
}

export interface SimulatorStatus {
  id: string;
  name: string;
  installed: boolean;
  data_ready: boolean;
  executable: string;
  supported_formats: string[];
  note: string;
}

export interface CoverageCardInput {
  name: string;
  quantity: number;
  zone: string;
}

export interface CoverageDeckInput {
  name: string;
  cards: CoverageCardInput[];
}

export interface CardCoverageIssue {
  name: string;
  quantity: number;
  zone: string;
  status: "partial" | "missing" | "semantic_risk";
  parse_errors: string[];
}

export interface DeckCoverage {
  deck_name: string;
  unique_cards: number;
  card_copies: number;
  present_unique: number;
  parser_complete_unique: number;
  partial_unique: number;
  missing_unique: number;
  semantic_risk_unique: number;
  present_ratio: number;
  parser_complete_ratio: number;
  semantic_risk_ratio: number;
  issues: CardCoverageIssue[];
}

export interface HexdekCoverageReport {
  engine: string;
  corpus_entries: number;
  parser_revision?: string;
  decks: DeckCoverage[];
}

export interface ConformanceEngineSummary {
  engine: string;
  revisions: string[];
  observations: number;
}

export interface ConformanceSummary {
  schema_version: number;
  observations: number;
  scenario_runs: number;
  interaction_verified_scenarios: number;
  cross_engine_verified_scenarios: number;
  failed_scenarios: number;
  conflicting_scenarios: number;
  engines: ConformanceEngineSummary[];
  database_bytes: number;
}

export interface CommanderConformanceReport {
  schema_version: number;
  policy_id: string;
  passed: number;
  failed: number;
  covered_rules: string[];
  cases: {
    id: string;
    expected_rule?: string;
    actual_rule?: string;
    passed: boolean;
  }[];
}

export interface CardConformance {
  name: string;
  status: "unverified" | "interaction_verified" | "cross_engine_verified" | "failed" | "conflict";
  classification: "unverified" | "verified" | "partially_covered" | "disputed" | "unsupported";
  scenarios: string[];
  claims: {
    subject_id: string;
    status: "unverified" | "interaction_verified" | "cross_engine_verified" | "failed" | "conflict";
    classification: "unverified" | "verified" | "partially_covered" | "disputed" | "unsupported";
    ai_decision_covered: boolean;
    scenarios: string[];
  }[];
}

export interface DeckConformanceReport {
  deck_name: string;
  unique_cards: number;
  interaction_verified_unique: number;
  cross_engine_verified_unique: number;
  failed_unique: number;
  conflicting_unique: number;
  unverified_unique: number;
  verified_unique: number;
  partially_covered_unique: number;
  disputed_unique: number;
  unsupported_unique: number;
  cards: CardConformance[];
}

export interface ConformanceAuditReport {
  summary: ConformanceSummary;
  scenarios: {
    scenario_schema_version: number;
    scenario_id: string;
    scenario_version: number;
    seed: number;
    configuration_hash: string;
    classification: CardConformance["classification"];
    cards: string[];
    engines: string[];
  }[];
  decks: DeckConformanceReport[];
}

export interface ConformanceImportResult {
  imported_observations: number;
  summary: ConformanceSummary;
}

export interface InstallerProgress {
  stage: string;
  detail: string;
  completed_units: number;
  total_units?: number;
}

export interface MatchupRow {
  deck: string;
  games: number;
  wins: number;
  win_rate: number;
  win_rate_lower_bound: number;
  win_rate_upper_bound: number;
  average_turn_to_win?: number;
}

export interface SeatOrderRow {
  matchup: string;
  deck: string;
  position: number;
  games: number;
  wins: number;
  win_rate: number;
}

export interface SimulationReport {
  engine: string;
  schema_version: string;
  engine_routes?: Record<string, string>;
  route_failures?: string[];
  provenance?: ResultProvenance;
  games: number;
  draws: number;
  crashes: number;
  cached_games?: number;
  seat_order?: SeatOrderRow[];
  average_turns: number;
  games_per_second: number;
  rows: MatchupRow[];
  matchup_matrix: Record<string, Record<string, number>>;
  matchup_wins: Record<string, Record<string, number>>;
  matchup_games: Record<string, Record<string, number>>;
  matchup_weights?: Record<string, number>;
  commander_evidence?: CommanderEvidence;
  combo_execution?: ComboExecutionEvidence;
  performance?: PerformanceBreakdown;
}

export interface ComboExecutionEvidence {
  plan_available: boolean;
  proof_revision: string;
  planned_lines: number;
  assembling_games: number;
  executable_games: number;
  priority_actions: number;
  plan_wins: number;
  assembling_rate: number;
  executable_rate: number;
  win_conversion_rate: number;
  limitations: string[];
}

export interface PerformanceBreakdown {
  coverage_ms: number;
  cache_lookup_ms: number;
  serialization_ms: number;
  engine_ms: number;
  parse_and_store_ms: number;
  total_ms: number;
  process_starts: number;
  cache_hit: boolean;
}

export interface CommanderEvidence {
  schema_version: number;
  policy_id: string;
  evidence_mode: "commander_duel" | "multiplayer_pod";
  pod_size: number;
  seed: number;
  rotation_policy: string;
  complete_rotations: number;
  partial_rotation_games: number;
  pod_archetypes: string[];
  pod_win_share: CommanderMetric;
  seat_floor: CommanderMetric;
  threat_development: CommanderMetric;
  interaction: CommanderMetric;
  recovery: CommanderMetric;
  mana_reliability: CommanderMetric;
  closing_ability: CommanderMetric;
  seat_performance: { position: number; games: number; wins: number; win_share: number }[];
  elimination_slots: number[];
  target_actions: Record<string, number>;
  penalties: {
    draw_or_stall_rate: number;
    timeout_or_crash_rate: number;
    concession_rate: number;
    seat_bias: number;
    kingmaking_signal?: number;
    total_penalty: number;
  };
  objective_score: number;
  limitations: string[];
}

export interface CommanderMetric {
  value: number;
  source: "rules_engine" | "rules_engine_compact" | "structural_proxy" | "unavailable";
}

export interface SideboardExchange {
  cards_in: DeckEntry[];
  cards_out: DeckEntry[];
}

export interface MatchupSideboardPlan {
  opponent_key: string;
  on_play: SideboardExchange;
  on_draw: SideboardExchange;
}

export interface BestOfThreeEvidence {
  games: number;
  wins: number;
  draws: number;
  crashes: number;
  win_rate: number;
  win_rate_lower_bound: number;
  win_rate_upper_bound: number;
}

export interface BestOfThreeMatchupReport {
  opponent_key: string;
  opponent_name: string;
  weight: number;
  plan: MatchupSideboardPlan;
  preboard_on_play: BestOfThreeEvidence;
  preboard_on_draw: BestOfThreeEvidence;
  preboard: BestOfThreeEvidence;
  postboard_on_play: BestOfThreeEvidence;
  postboard_on_draw: BestOfThreeEvidence;
  postboard: BestOfThreeEvidence;
  matches: BestOfThreeEvidence;
}

export interface BestOfThreeSimulationReport {
  engine: string;
  schema_version: string;
  provenance: ResultProvenance;
  requested_matches: number;
  completed_matches: number;
  cached_games: number;
  games_per_second: number;
  preboard: BestOfThreeEvidence;
  postboard: BestOfThreeEvidence;
  matches: BestOfThreeEvidence;
  matchups: BestOfThreeMatchupReport[];
}

export interface SideboardPolicyCandidate {
  exchange: SideboardExchange;
  evidence: BestOfThreeEvidence;
  score: number;
  configuration_hash: string;
}

export interface BranchPolicyEvolution {
  play_draw: "play" | "draw";
  best_exchange: SideboardExchange;
  best_evidence: BestOfThreeEvidence;
  alternatives: SideboardPolicyCandidate[];
  evaluated_policies: number;
  attempted_games: number;
  cached_games: number;
}

export interface MatchupPolicyEvolution {
  opponent_key: string;
  opponent_name: string;
  weight: number;
  games_per_policy: number;
  on_play: BranchPolicyEvolution;
  on_draw: BranchPolicyEvolution;
}

export interface SideboardEvolutionReport {
  engine: string;
  schema_version: string;
  provenance: ResultProvenance;
  plans: MatchupSideboardPlan[];
  matchups: MatchupPolicyEvolution[];
  evaluated_policies: number;
  attempted_games: number;
  cached_games: number;
  games_per_second: number;
}

export interface ResultProvenance {
  schema_version: number;
  engine_revision: string;
  card_data_revision: string;
  scenario_schema_version?: number;
  configuration_hash: string;
}

export interface SimulationCacheStatus {
  entries: number;
  hits: number;
  logical_bytes: number;
  database_bytes: number;
  max_entries: number;
  max_logical_bytes: number;
}

export interface SavedValidation {
  id: string;
  run_id?: string;
  candidate_rank?: number;
  created_at: string;
  request: Record<string, unknown>;
  report: SimulationReport;
}

export interface HalvingCandidateResult {
  candidate_rank: number;
  commander: string;
  structural_score: number;
  round_win_rate: number;
  game_win_rate: number;
  cumulative_games: number;
  crash_rate: number;
  win_rate_lower_bound: number;
  win_rate_upper_bound: number;
  worst_matchup_win_rate: number;
  worst_matchup_lower_bound: number;
  worst_matchup_upper_bound?: number;
  worst_position_win_rate?: number;
  worst_position_lower_bound?: number;
  worst_position_upper_bound?: number;
  position_win_rate_spread?: number;
  evidence_score: number;
  evidence_lower_bound?: number;
  evidence_upper_bound?: number;
  blended_score: number;
  blended_lower_bound?: number;
  blended_upper_bound?: number;
  report?: SimulationReport;
  error?: string;
}

export interface HalvingRound {
  round: number;
  games_per_candidate: number;
  entrants: number;
  survivors: number[];
  statistically_eliminated?: number[];
  results: HalvingCandidateResult[];
}

export interface SuccessiveHalvingReport {
  engine: string;
  total_games: number;
  total_attempts: number;
  maximum_game_budget?: number;
  scheduled_game_budget?: number;
  avoided_game_budget?: number;
  total_crashes: number;
  elapsed_ms: number;
  winner_rank?: number;
  cancelled: boolean;
  rounds: HalvingRound[];
}

export interface HalvingProgress {
  round: number;
  total_rounds: number;
  candidate_index: number;
  candidates_in_round: number;
  candidate_rank: number;
  games_per_candidate: number;
  completed_games: number;
  completed_attempts: number;
}

export interface HybridEvolutionRequest {
  evolution: RunRequest;
  opponents: Deck[];
  opponent_weights: number[];
  gauntlet_provenance?: GauntletProvenance;
  competitive_claims_allowed: boolean;
  field_revision_id?: string;
  cycles: number;
  finalists: number;
  base_games: number;
  rounds: number;
  workers: number;
  seats: number;
  hat_budget: number;
  max_turns: number;
  game_timeout_seconds: number;
  structural_weight: number;
  risk_aversion: number;
  worst_matchup_weight: number;
  seat_position_weight: number;
  adaptive_elimination: boolean;
  coevolve_adversaries: boolean;
  learn_packages_from_hall: boolean;
  holdout_games: number;
  commander: boolean;
  continue_run_id?: string;
  continuation_candidate_rank?: number;
}

export type ComboProof = "corpus_only" | "legacy_resource_hypothesis" | "resource_balanced_hypothesis" | "symbolically_proven" | "state_proven" | "hexdek_executed" | "cross_engine_verified" | "arena_verified" | "disputed" | "unsupported";
export type ComboActionKind = "unknown" | "draw" | "tutor" | "recursion" | "copy" | "replacement" | "cost_reduction" | "untap" | "mana" | "token" | "counter" | "damage" | "mill" | "life" | "sacrifice" | "blink" | "protection" | "alternate_win";

export interface ComboResources {
  mana: number;
  cards: number;
  permanents: number;
  tokens: number;
  life: number;
  graveyard: number;
  counters: number;
  damage: number;
  mill: number;
}

export interface ComboAction {
  id: string;
  card_id: string;
  card_name: string;
  mode_name: string;
  source_types: string[];
  source_subtypes: string[];
  label: string;
  kind: ComboActionKind;
  timing: string;
  zone: string;
  cost: ComboResources;
  effect: ComboResources;
  uses_tap: boolean;
  consumes_self: boolean;
  once_per_turn: boolean;
  parser_confidence: number;
  conditions: string[];
  rule_timing: RuleTiming;
  rule_costs: RuleCost[];
  rule_targets: RuleTarget[];
  rule_effects: RuleEffect[];
  rule_delayed_effects: RuleEffect[];
  exact_state_supported: boolean;
}

export interface ComboObjectState {
  instance_id: string;
  card_id: string;
  card_name: string;
  controller: number;
  zone: AbilityZone;
  types: string[];
  subtypes: string[];
  token: boolean;
  tapped: boolean;
  summoning_sick: boolean;
  counters: [string, number][];
}

export interface ComboState {
  resources: ComboResources;
  colored_mana: number[];
  objects: ComboObjectState[];
  life_by_player: number[];
  tapped_cards: string[];
  consumed_cards: string[];
  once_per_turn_used: string[];
  delayed_triggers: string[];
  replacement_effects: string[];
  continuous_modifiers: string[];
  actions_taken: number;
}

export interface ExactStateEvidence {
  proof_revision: string;
  initial_state_hash: string;
  final_state_hash: string;
  source_state_restored: boolean;
  another_iteration_legal: boolean;
  applied_actions: number;
  failure_reasons: string[];
}

export interface ComboLine {
  id: string;
  corpus_id?: string;
  cards: string[];
  actions: ComboAction[];
  proof: ComboProof;
  finite_win: boolean;
  repeatable: boolean;
  loop_output: ComboResources;
  minimum_resources: ComboResources;
  expected_turn?: number;
  setup: string[];
  disruption_points: string[];
  dead_pieces: string[];
  unsupported_assumptions: string[];
  explored_states: number;
  elapsed_ms: number;
  exact_state?: ExactStateEvidence;
}

export interface ComboConformanceScenario {
  schema_version: number;
  scenario_id: string;
  line_id: string;
  proof_revision: string;
  cards: string[];
  action_ids: string[];
  initial_resources: ComboResources;
  expected_class: string;
  expected_terminal_hash: string;
  maximum_actions: number;
  deterministic: boolean;
}

export interface ComboConformanceObservation {
  scenario_id: string;
  line_id: string;
  engine: "hexdek" | "forge" | "arena" | string;
  engine_revision: string;
  card_data_revision: string;
  expected_terminal_hash: string;
  terminal_state_hash: string;
  outcome: "passed" | "disputed" | "unsupported" | "incomplete" | string;
  detail: string;
  recorded_at: number;
}

export interface ComboConformanceReport {
  scenario: ComboConformanceScenario;
  proof: ComboProof;
  observations: ComboConformanceObservation[];
  decision: string;
}

export interface ComboPackage {
  line_id: string;
  engine: string[];
  enablers: string[];
  payoffs: string[];
  tutors: string[];
  protection: string[];
  recovery: string[];
}

export interface ComboProofReport {
  schema_version: number;
  proof_revision: string;
  deck_name: string;
  corpus_revision?: string;
  corpus_hypotheses: number;
  lines: ComboLine[];
  packages: ComboPackage[];
  explored_states: number;
  elapsed_ms: number;
  truncated: boolean;
  warnings: string[];
}

export interface ComboCorpusStatus {
  schema_version: number;
  source?: string;
  revision?: string;
  synced_at_unix?: number;
  hypotheses: number;
  fully_resolved: number;
  database_bytes: number;
  conformance_observations: number;
}

export interface ComboCorpusSyncReport {
  status: ComboCorpusStatus;
  downloaded: number;
  resolved: number;
  unresolved_card_names: string[];
  source_pages: number;
}

export interface GauntletProvenance {
  id: string;
  name: string;
  revision: number;
}

export interface HybridProgress {
  job_id: string;
  cycle: number;
  total_cycles: number;
  phase: "structural" | "validation" | "cycle_complete";
  generation?: GenerationProgress;
  halving?: HalvingProgress;
  winner_rank?: number;
  completed_attempts: number;
}

export interface HybridComplete {
  job_id: string;
  status: JobStatus;
  cycles_completed: number;
  winner_rank?: number;
  total_attempts: number;
}

export interface SavedHalving {
  id: string;
  run_id?: string;
  created_at: string;
  request: Record<string, unknown>;
  report: SuccessiveHalvingReport;
}
