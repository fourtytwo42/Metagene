use crate::model::{
    AbilityKind, AbilityProfile, AbilityZone, ActionTargetProfile, CardMode, CardPool, Color,
    ComboPackageProfile, Deck, DeckConstraints, DeckEntry, EvolutionConfig, EvolutionMode,
    FitnessWeights, ManaColorRule, ManaSourceProfile, ManaSpendRule, ManaStateRule,
    SearchTargetProfile, StructuralEvaluationContext, TunePolicy,
};
use crate::rng::SplitMix64;
use crate::{
    CommanderCompatibilityEvidence, commander_compatibility, commanders_compatible,
    is_commander_eligible, validate_deck_legality,
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering as AtomicOrdering},
};
use std::time::{Duration, Instant};
use thiserror::Error;

/// Constructed decks normally express their plan through repeated cards. Keep
/// enough exploration for bullets and discoveries, but make another copy of an
/// active card the common addition. Singleton formats bypass this prior.
pub const STRUCTURAL_MODEL_REVISION: &str = "closed-loop-quality-v11";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CardCastabilityEvidence {
    pub card_id: crate::CardId,
    pub name: String,
    pub copies: u8,
    pub on_curve_castability: f64,
    pub turn_horizon_castability: f64,
    pub unrestricted_early_sources: u16,
    pub conditional_early_sources: u16,
    pub unusable_sources: u16,
    pub source_restrictions: Vec<String>,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RequirementEvidence {
    pub mechanic: String,
    pub demand: f64,
    pub supply: f64,
    pub fulfillment: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ActionEvidence {
    pub card_id: crate::CardId,
    pub card_name: String,
    pub action: String,
    pub kind: String,
    pub zone: String,
    pub conditions: Vec<String>,
    pub copies: u8,
    pub availability: f64,
    pub mana_availability: f64,
    pub target_availability: f64,
    pub contribution: f64,
    pub narrow: bool,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StrategyPackageEvidence {
    pub package: String,
    pub primary: bool,
    pub root_support: f64,
    pub candidate_support: f64,
    pub retention: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RoleEvidence {
    pub role: String,
    pub copies: f64,
    pub target: f64,
    pub fulfillment: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CopyEvidence {
    pub card_id: crate::CardId,
    pub card_name: String,
    pub copies: u8,
    pub functional_target: u8,
    pub rationale: String,
    pub excess_copies: u8,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StrategyPackageDefinition {
    pub package: String,
    pub primary: bool,
    pub root_support: f64,
    pub root_unique_cards: usize,
    #[serde(default)]
    pub required_roles: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StrategySignature {
    pub version: u32,
    pub packages: Vec<StrategyPackageDefinition>,
    pub root_nonland_copies: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Fitness {
    pub score: f64,
    pub consistency: f64,
    pub tempo: f64,
    pub synergy: f64,
    pub interaction: f64,
    pub resilience: f64,
    pub mana_failure_rate: f64,
    #[serde(default)]
    pub required_color_count: u8,
    #[serde(default)]
    pub color_complexity_penalty: f64,
    #[serde(default)]
    pub commander_quality: f64,
    #[serde(default)]
    pub sideboard_quality: f64,
    #[serde(default)]
    pub severe_mana_failure: bool,
    #[serde(default)]
    pub mana_blocking_reasons: Vec<String>,
    #[serde(default)]
    pub early_sources: [u16; 6],
    #[serde(default)]
    pub late_sources: [u16; 6],
    #[serde(default)]
    pub colored_pip_demand: [u16; 6],
    #[serde(default)]
    pub on_curve_castability: f64,
    #[serde(default)]
    pub turn_horizon_castability: f64,
    #[serde(default)]
    pub stranded_card_rate: f64,
    #[serde(default)]
    pub legendary_dead_draw_rate: f64,
    #[serde(default)]
    pub synergy_warnings: Vec<String>,
    #[serde(default)]
    pub unrestricted_early_sources: [u16; 6],
    #[serde(default)]
    pub conditional_early_sources: [u16; 6],
    #[serde(default)]
    pub unusable_sources: [u16; 6],
    #[serde(default)]
    pub unmet_requirement_rate: f64,
    #[serde(default)]
    pub legendary_penalty: f64,
    #[serde(default)]
    pub unsupported_synergy_penalty: f64,
    #[serde(default)]
    pub card_castability: Vec<CardCastabilityEvidence>,
    #[serde(default)]
    pub requirement_evidence: Vec<RequirementEvidence>,
    #[serde(default)]
    pub action_evidence: Vec<ActionEvidence>,
    #[serde(default)]
    pub strategy_packages: Vec<StrategyPackageEvidence>,
    #[serde(default)]
    pub role_evidence: Vec<RoleEvidence>,
    #[serde(default)]
    pub copy_evidence: Vec<CopyEvidence>,
    #[serde(default)]
    pub functional_action_rate: f64,
    #[serde(default = "default_full_rate")]
    pub accounted_clause_rate: f64,
    #[serde(default = "default_full_rate")]
    pub parsed_clause_rate: f64,
    #[serde(default)]
    pub unresolved_clause_rate: f64,
    #[serde(default)]
    pub unresolved_oracle_text: Vec<String>,
    #[serde(default)]
    pub dead_action_rate: f64,
    #[serde(default)]
    pub narrow_mainboard_rate: f64,
    #[serde(default)]
    pub copy_excess_rate: f64,
    #[serde(default)]
    pub closing_power: f64,
    #[serde(default)]
    pub role_balance: f64,
    #[serde(default)]
    pub combo_assembly_probability: f64,
    #[serde(default)]
    pub combo_expected_turn: Option<f64>,
    #[serde(default)]
    pub combo_dead_card_rate: f64,
    #[serde(default)]
    pub combo_resilience: f64,
    #[serde(default)]
    pub combo_execution_rate: f64,
    #[serde(default)]
    pub combo_proof_level: String,
    #[serde(default)]
    pub archetype: String,
    #[serde(default = "default_strategy_similarity")]
    pub strategy_similarity: f64,
    #[serde(default)]
    pub nonland_edit_fraction: f64,
    #[serde(default)]
    pub severe_structural_failure: bool,
    #[serde(default)]
    pub structural_blocking_reasons: Vec<String>,
}

fn default_strategy_similarity() -> f64 {
    1.0
}

fn default_full_rate() -> f64 {
    1.0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackageSummary {
    pub tag: String,
    pub copies: usize,
    pub unique_cards: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RankedDeck {
    pub rank: usize,
    #[serde(default)]
    pub pareto_rank: usize,
    pub deck: Deck,
    pub fitness: Fitness,
    #[serde(default)]
    pub packages: Vec<PackageSummary>,
    #[serde(default)]
    pub commander_compatibility: Option<CommanderCompatibilityEvidence>,
    #[serde(default)]
    pub candidate_status: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(default)]
pub struct MapEliteDescriptor {
    pub archetype: String,
    pub colors: String,
    pub curve: String,
    pub interaction: String,
    pub victory_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MapEliteCell {
    pub descriptor: MapEliteDescriptor,
    pub candidate: RankedDeck,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GenerationProgress {
    pub generation: usize,
    pub total_generations: usize,
    pub evaluated_candidates: u64,
    #[serde(default)]
    pub unique_evaluations: u64,
    #[serde(default)]
    pub evaluations_avoided: u64,
    #[serde(default)]
    pub unique_in_generation: usize,
    #[serde(default)]
    pub duplicates_in_generation: usize,
    pub elapsed_ms: u64,
    pub candidates_per_second: f64,
    #[serde(default)]
    pub unique_evaluations_per_second: f64,
    pub best: RankedDeck,
    pub top: Vec<RankedDeck>,
    #[serde(default)]
    pub baseline: Option<RankedDeck>,
    #[serde(default)]
    pub strategy_signature: Option<StrategySignature>,
    #[serde(default)]
    pub structural_model_revision: String,
    #[serde(default)]
    pub map_elite_cells: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PromotionEvidence {
    pub status: String,
    pub incumbent_rank: Option<usize>,
    pub challenger_rank: Option<usize>,
    pub promoted_rank: Option<usize>,
    pub selection_gain: Option<f64>,
    pub holdout_gain: Option<f64>,
    pub forge_gain: Option<f64>,
    pub single_engine_validated: bool,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunResult {
    pub generations_completed: usize,
    pub evaluated_candidates: u64,
    #[serde(default)]
    pub unique_evaluations: u64,
    #[serde(default)]
    pub evaluations_avoided: u64,
    pub elapsed_ms: u64,
    pub cancelled: bool,
    pub frontier: Vec<RankedDeck>,
    #[serde(default)]
    pub baseline: Option<RankedDeck>,
    #[serde(default)]
    pub strategy_signature: Option<StrategySignature>,
    #[serde(default)]
    pub promotion_evidence: Option<PromotionEvidence>,
    #[serde(default)]
    pub structural_model_revision: String,
    #[serde(default)]
    pub map_elites: Vec<MapEliteCell>,
}

#[derive(Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, AtomicOrdering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(AtomicOrdering::Relaxed)
    }
}

#[derive(Debug, Error)]
pub enum EvolutionError {
    #[error("card pool is empty")]
    EmptyPool,
    #[error("seed deck references an unknown card: {0}")]
    UnknownCard(String),
    #[error("seed deck contains {actual} cards; expected {expected}")]
    WrongDeckSize { actual: usize, expected: usize },
    #[error("constraint for {0} cannot be satisfied")]
    UnsatisfiableConstraint(String),
    #[error("population size must be at least 2")]
    PopulationTooSmall,
    #[error("failed to create worker pool: {0}")]
    WorkerPool(String),
}

pub trait Evaluator: Sync {
    fn evaluate(
        &self,
        deck: &[(usize, u8)],
        commanders: &[usize],
        pool: &CardPool,
        seeds: &[u64],
        turn_horizon: u8,
        context: &StructuralEvaluationContext,
    ) -> Fitness;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HeuristicEvaluator;

#[derive(Default)]
struct TrialScore {
    consistency: f64,
    tempo: f64,
    synergy: f64,
    interaction: f64,
    resilience: f64,
    closing: f64,
    mana_failure: f64,
    legendary_dead_observations: u32,
    nonland_hand_observations: u32,
}

#[derive(Clone)]
struct TrialManaSource {
    abilities: Vec<ManaSourceProfile>,
    simple_colors: [bool; 6],
    unrestricted_fixed_colors: [bool; 6],
    simple_unrestricted: bool,
    stateful: bool,
    available_turn: u8,
    chosen_creature_type: Option<String>,
    charge_counters: u8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourcePaymentKind {
    Simple,
    Stateless,
    Stateful,
}

impl Evaluator for HeuristicEvaluator {
    fn evaluate(
        &self,
        deck: &[(usize, u8)],
        commanders: &[usize],
        pool: &CardPool,
        seeds: &[u64],
        turn_horizon: u8,
        context: &StructuralEvaluationContext,
    ) -> Fitness {
        if seeds.is_empty() {
            return Fitness::default();
        }

        let mut effective_context = context.clone();
        if !commanders.is_empty() {
            effective_context.commander = true;
            effective_context.commander_colors = commanders
                .iter()
                .flat_map(|index| pool.cards[*index].color_identity.iter().copied())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
        }

        let mut library =
            Vec::with_capacity(deck.iter().map(|(_, count)| *count as usize).sum::<usize>());
        for (index, count) in deck {
            library.extend(std::iter::repeat_n(*index, *count as usize));
        }
        let has_search_actions = library.iter().any(|index| {
            pool.cards[*index]
                .features
                .mechanics
                .search_action
                .is_some()
        });
        let has_legendary_cards = library
            .iter()
            .any(|index| pool.cards[*index].features.is_legendary);

        let mut aggregate = TrialScore::default();
        let mut scratch = TrialScratch {
            library: Vec::with_capacity(library.len()),
            hand: Vec::with_capacity(turn_horizon as usize + 7),
            sources: Vec::with_capacity(turn_horizon as usize + 8),
            used_sources: Vec::with_capacity(turn_horizon as usize + 8),
        };
        for seed in seeds {
            let score = simulate_trial(
                &library,
                pool,
                *seed,
                turn_horizon,
                &effective_context,
                has_search_actions,
                has_legendary_cards,
                &mut scratch,
            );
            aggregate.consistency += score.consistency;
            aggregate.tempo += score.tempo;
            aggregate.synergy += score.synergy;
            aggregate.interaction += score.interaction;
            aggregate.resilience += score.resilience;
            aggregate.closing += score.closing;
            aggregate.mana_failure += score.mana_failure;
            aggregate.legendary_dead_observations += score.legendary_dead_observations;
            aggregate.nonland_hand_observations += score.nonland_hand_observations;
        }

        let divisor = seeds.len() as f64;
        let consistency = aggregate.consistency / divisor;
        let tempo = aggregate.tempo / divisor;
        let mut synergy = aggregate.synergy / divisor;
        let interaction = aggregate.interaction / divisor;
        let resilience = aggregate.resilience / divisor;
        let trial_closing = aggregate.closing / divisor;
        let mana_failure_rate = aggregate.mana_failure / divisor;
        let required_color_count = required_spell_color_count(deck, pool);
        let color_complexity_penalty =
            required_color_count.saturating_sub(2) as f64 * mana_failure_rate * 0.15;
        let commander_value = commanders
            .iter()
            .map(|index| {
                let features = &pool.cards[*index].features;
                (features.card_advantage
                    + features.interaction
                    + features.pressure
                    + features.resilience
                    + features.ramp)
                    / (features.mana_value + 2.0).max(2.0)
            })
            .sum::<f32>() as f64;
        let commander_focus = commander_focus_tags(commanders, pool);
        if !commander_focus.is_empty() {
            let supporting_cards = deck
                .iter()
                .filter(|(_, count)| *count > 0)
                .filter(|(index, _)| {
                    pool.cards[*index]
                        .features
                        .tags
                        .iter()
                        .any(|tag| commander_focus.contains(tag))
                })
                .count();
            let theme_support = (supporting_cards as f64 / 12.0).min(1.0);
            synergy = synergy * 0.7 + theme_support * 0.3;
        }
        let mut fitness = Fitness {
            score: 0.0,
            consistency,
            tempo,
            synergy,
            interaction,
            resilience,
            mana_failure_rate,
            required_color_count: required_color_count.min(u8::MAX as usize) as u8,
            color_complexity_penalty,
            commander_quality: commander_value.min(1.0),
            sideboard_quality: 0.0,
            strategy_similarity: 1.0,
            ..Fitness::default()
        };
        apply_structural_evidence(&mut fitness, deck, pool, turn_horizon, &effective_context);
        // Static card counts answer whether a victory path exists. Goldfish
        // sequencing answers whether the deck can deploy that path in time.
        // Requiring both prevents piles of cheap utility artifacts from
        // receiving a strong closing score without meaningful pressure.
        fitness.closing_power =
            (fitness.closing_power * 0.45 + trial_closing * 0.55).clamp(0.0, 1.0);
        let commander_closing = commanders
            .iter()
            .map(|index| {
                let card = &pool.cards[*index];
                let typed_finish = card
                    .features
                    .initial_modes
                    .iter()
                    .flat_map(|mode| mode.abilities.iter())
                    .filter(|ability| {
                        matches!(
                            ability.kind,
                            AbilityKind::CreateToken
                                | AbilityKind::Copy
                                | AbilityKind::TokenMultiplier
                                | AbilityKind::Damage
                                | AbilityKind::Mill
                                | AbilityKind::AlternateWin
                        )
                    })
                    .map(|ability| {
                        f64::from(ability.effect_value) * f64::from(ability.parser_confidence)
                    })
                    .sum::<f64>();
                f64::from(card.features.pressure).max(typed_finish)
            })
            .sum::<f64>();
        if commander_closing > 0.0 {
            fitness.closing_power = fitness
                .closing_power
                .max((commander_closing / 8.0).clamp(0.0, 1.0));
            for role in &mut fitness.role_evidence {
                if role.role == "closing plan" {
                    role.copies += commander_closing;
                    role.fulfillment = fitness.closing_power;
                }
            }
            fitness
                .structural_blocking_reasons
                .retain(|reason| !reason.starts_with("No reachable threat"));
            fitness.severe_structural_failure = !fitness.structural_blocking_reasons.is_empty();
        }
        if aggregate.nonland_hand_observations > 0 {
            fitness.legendary_dead_draw_rate = f64::from(aggregate.legendary_dead_observations)
                / f64::from(aggregate.nonland_hand_observations);
            fitness.legendary_penalty = fitness.legendary_dead_draw_rate * 0.12;
        }
        fitness.score = weighted_fitness_score(&fitness, &FitnessWeights::default());
        fitness
    }
}

/// Evaluates one materialized deck with the same deterministic structural
/// model used during evolution. This is intentionally proxy evidence: callers
/// must not present it as rules-engine telemetry.
pub fn evaluate_structural_deck(
    deck: &Deck,
    pool: &CardPool,
    seed: u64,
    trials: usize,
    turn_horizon: u8,
) -> Result<Fitness, EvolutionError> {
    let commander_colors = deck
        .command_zone
        .iter()
        .filter_map(|entry| pool.card(&entry.card_id))
        .flat_map(|card| card.color_identity.iter().copied())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let context = StructuralEvaluationContext {
        commander: !deck.command_zone.is_empty(),
        commander_colors,
        ..StructuralEvaluationContext::default()
    };
    evaluate_structural_deck_with_context(deck, pool, seed, trials, turn_horizon, &context)
}

pub fn evaluate_structural_deck_with_context(
    deck: &Deck,
    pool: &CardPool,
    seed: u64,
    trials: usize,
    turn_horizon: u8,
    context: &StructuralEvaluationContext,
) -> Result<Fitness, EvolutionError> {
    let indices = pool
        .cards
        .iter()
        .enumerate()
        .map(|(index, card)| (&card.id, index))
        .collect::<HashMap<_, _>>();
    let resolve = |entries: &[DeckEntry]| {
        entries
            .iter()
            .map(|entry| {
                indices
                    .get(&entry.card_id)
                    .copied()
                    .map(|index| (index, entry.quantity))
                    .ok_or_else(|| EvolutionError::UnknownCard(entry.card_id.0.clone()))
            })
            .collect::<Result<Vec<_>, _>>()
    };
    let library = resolve(&deck.mainboard)?;
    let commanders = resolve(&deck.command_zone)?
        .into_iter()
        .flat_map(|(index, quantity)| std::iter::repeat_n(index, quantity as usize))
        .collect::<Vec<_>>();
    let seeds = (0..trials.max(1))
        .map(|index| seed.wrapping_add((index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)))
        .collect::<Vec<_>>();
    let mut fitness = HeuristicEvaluator.evaluate(
        &library,
        &commanders,
        pool,
        &seeds,
        turn_horizon.max(1),
        context,
    );
    fitness.sideboard_quality = sideboard_quality(&resolve(&deck.sideboard)?, pool);
    fitness.score = weighted_fitness_score(&fitness, &FitnessWeights::default());
    Ok(fitness)
}

/// Derives the immutable, card-name-agnostic strategy boundary used by Tune.
/// Primary packages require typed functional relationships from at least two
/// distinct root cards; tag-only overlap can only contribute fallback roles.
pub fn strategy_signature_for_deck(
    root: &Deck,
    pool: &CardPool,
) -> Result<StrategySignature, EvolutionError> {
    let root = compact_genome(&deck_to_genome(root, pool)?);
    Ok(derive_strategy_signature(&root, pool))
}

/// Re-scores an arbitrary deck against an immutable Tune root. Validation
/// entrypoints use this before spending simulator budget so a legal deck that
/// crosses the identity or edit boundary cannot bypass evolutionary screening.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_tune_deck_with_context(
    candidate: &Deck,
    root: &Deck,
    pool: &CardPool,
    seed: u64,
    trials: usize,
    turn_horizon: u8,
    context: &StructuralEvaluationContext,
    policy: &TunePolicy,
    weights: &FitnessWeights,
) -> Result<(Fitness, StrategySignature), EvolutionError> {
    let mut fitness = evaluate_structural_deck_with_context(
        candidate,
        pool,
        seed,
        trials,
        turn_horizon,
        context,
    )?;
    let candidate = compact_genome(&deck_to_genome(candidate, pool)?);
    let root = compact_genome(&deck_to_genome(root, pool)?);
    let signature = derive_strategy_signature(&root, pool);
    apply_tune_policy(
        &mut fitness,
        &candidate,
        &root,
        &signature,
        policy,
        pool,
        false,
    );
    fitness.score = weighted_fitness_score(&fitness, weights);
    Ok((fitness, signature))
}

fn weighted_fitness_score(fitness: &Fitness, weights: &FitnessWeights) -> f64 {
    if fitness.severe_mana_failure || fitness.severe_structural_failure {
        return -1.0;
    }
    let rewards = [
        (fitness.consistency, weights.consistency),
        (fitness.tempo, weights.tempo),
        (fitness.synergy, weights.synergy),
        (fitness.interaction, weights.interaction),
        (fitness.resilience, weights.resilience),
        (fitness.commander_quality, weights.commander),
        (fitness.closing_power, weights.closing_power),
        (fitness.role_balance, weights.role_balance),
    ];
    let mut total = rewards
        .iter()
        .map(|(_, weight)| weight.max(0.0))
        .sum::<f64>();
    let mut reward = rewards
        .iter()
        .map(|(value, weight)| value * weight.max(0.0))
        .sum::<f64>();
    if fitness.sideboard_quality > 0.0 {
        total += weights.sideboard.max(0.0);
        reward += fitness.sideboard_quality * weights.sideboard.max(0.0);
    }
    let reward = reward / total.max(f64::EPSILON);
    (reward
        - fitness.mana_failure_rate * weights.mana_failure_penalty.max(0.0) / 100.0
        - fitness.color_complexity_penalty
        - fitness.legendary_dead_draw_rate * weights.legendary_dead_draw_penalty.max(0.0) / 100.0
        - fitness.unmet_requirement_rate * weights.unsupported_synergy_penalty.max(0.0) / 100.0
        - fitness.dead_action_rate * weights.dead_action_penalty.max(0.0) / 100.0
        - fitness.narrow_mainboard_rate * weights.narrow_card_penalty.max(0.0) / 100.0
        - fitness.combo_dead_card_rate * 0.10
        - fitness.copy_excess_rate * 0.16)
        .clamp(-1.0, 1.0)
}

/// Applies the same user-configurable reward and penalty weights used by the
/// evolutionary population to independently evaluated baseline evidence.
pub fn apply_fitness_weights(fitness: &mut Fitness, weights: &FitnessWeights) {
    fitness.score = weighted_fitness_score(fitness, weights);
}

fn mana_slot(color: Color) -> usize {
    match color {
        Color::White => 0,
        Color::Blue => 1,
        Color::Black => 2,
        Color::Red => 3,
        Color::Green => 4,
        Color::Colorless => 5,
    }
}

fn required_spell_color_count(deck: &[(usize, u8)], pool: &CardPool) -> usize {
    let mut required = [false; 5];
    for (index, count) in deck {
        if *count == 0 || pool.cards[*index].features.is_land {
            continue;
        }
        let card = &pool.cards[*index];
        if card.features.initial_modes.is_empty() {
            for color in &card.colors {
                if let Some(slot) = color_slot(*color) {
                    required[slot] = true;
                }
            }
        } else {
            for mode in card
                .features
                .initial_modes
                .iter()
                .filter(|mode| !mode.is_land)
            {
                for (slot, pips) in mode.mana_cost.colored[..5].iter().enumerate() {
                    required[slot] |= *pips > 0;
                }
            }
        }
    }
    required.into_iter().filter(|required| *required).count()
}

fn apply_structural_evidence(
    fitness: &mut Fitness,
    deck: &[(usize, u8)],
    pool: &CardPool,
    turn_horizon: u8,
    context: &StructuralEvaluationContext,
) {
    let verbose = !context.compact_evaluation;
    let mut nonland_copies = 0_u32;
    let mut legendary_excess = 0_u32;
    let mut legendary_colors = HashSet::new();
    let chosen_creature_type = best_chosen_type_for_deck(deck, pool);
    let snapshot_creatures = deck
        .iter()
        .filter(|(index, _)| pool.cards[*index].type_line.contains("Creature"))
        .map(|(_, count)| u32::from(*count))
        .sum::<u32>();
    let snapshot_basics = deck
        .iter()
        .filter(|(index, _)| pool.cards[*index].type_line.contains("Basic Land"))
        .map(|(_, count)| u32::from(*count))
        .sum::<u32>();
    let snapshot_graveyard_setup = deck
        .iter()
        .filter(|(index, _)| {
            let card = &pool.cards[*index];
            card.features.mechanics.graveyard_setup_supply > 0
                || card
                    .features
                    .tags
                    .iter()
                    .any(|tag| tag == "mill" || tag == "discard")
        })
        .map(|(_, count)| u32::from(*count))
        .sum::<u32>();
    for (index, _) in deck.iter().copied().filter(|(_, count)| *count > 0) {
        let card = &pool.cards[index];
        if card.features.is_legendary
            && (card.type_line.contains("Creature")
                || card.type_line.contains("Artifact")
                || card.type_line.contains("Enchantment")
                || card.type_line.contains("Planeswalker"))
        {
            legendary_colors.extend(card.colors.iter().copied());
        }
    }

    let mut creature_power = 0_u32;
    let mut crew_demand = 0_u32;
    let mut humans = 0_u32;
    let mut human_payoff = false;
    let mut creature_copies = 0_u32;
    let mut death_supply = 0_u32;
    let mut needs_death = false;
    let mut needs_creature = false;
    let mut reanimation = false;
    let mut graveyard_setup = false;
    let mut basic_search = 0_u32;
    let mut individual_search_deficit = 0_u32;
    let mut basics = 0_u32;
    let mut legendary_payoff = false;
    let mut legendary_copies = 0_u32;
    let mut permanent_pips_by_color = [0_u32; 5];
    let mut devotion_profiles = Vec::<(String, u8, u8)>::new();
    let mut opponent_discard_demand = 0_u32;
    let mut opponent_discard_supply = 0_u32;
    let mut sacrifice_demand = 0_u32;
    let mut sacrifice_outlets = 0_u32;
    let mut sacrifice_fodder = 0_u32;
    let mut graveyard_target_demand = 0_u32;
    let mut graveyard_target_supply = 0_u32;
    let mut graveyard_setup_demand = 0_u32;
    let mut graveyard_setup_supply = 0_u32;
    let mut unsupported_narrow_sources = 0_u32;
    let mut explicit_payoff_copies = 0_u32;
    let mut total_requirement = 0.0;
    let mut unmet_requirement = 0.0;
    let mut action_weight = 0.0_f64;
    let mut functional_action_weight = 0.0_f64;
    let mut oracle_gameplay_weight = 0.0_f64;
    let mut oracle_accounted_weight = 0.0_f64;
    let mut oracle_parsed_weight = 0.0_f64;
    let mut oracle_unresolved_weight = 0.0_f64;
    let mut narrow_action_weight = 0.0_f64;
    let mut functional_interaction = 0.0_f64;
    let mut closing_supply = 0.0_f64;
    let mut card_flow_supply = 0.0_f64;
    let mut excess_nonland_copies = 0_u32;
    let mut nonland_mana_value = 0.0_f64;
    let mut threat_copies = 0_u32;
    let mut engine_copies = 0_u32;
    let mut castability_copies = 0.0_f64;
    let mut horizon_castability_sum = 0.0_f64;
    let mut curve_castability_sum = 0.0_f64;
    let mut unreachable_required_symbol = false;
    let mut uncastable_core = false;
    macro_rules! record_requirement {
        ($evidence:expr) => {{
            if verbose {
                fitness.requirement_evidence.push($evidence);
            }
        }};
    }
    macro_rules! record_warning {
        ($warning:expr) => {
            if verbose {
                fitness.synergy_warnings.push($warning);
            }
        };
    }

    if verbose
        && !context.commander
        && deck.iter().any(|(index, count)| {
            *count > 0
                && pool.cards[*index]
                    .features
                    .initial_modes
                    .iter()
                    .any(|mode| {
                        mode.mana_sources
                            .iter()
                            .any(|source| source.color_rule == ManaColorRule::CommanderIdentity)
                    })
        })
    {
        fitness
            .synergy_warnings
            .push("Command Tower: produces no mana without a commander".into());
    }

    // Classify each physical source once, then fan its result into the fixed
    // six-color arrays. The old slot-first loop reparsed every source and
    // rescanned every spell six times per candidate.
    let mut action_source_counts = [0_u16; 6];
    let mut action_physical_sources = 0_u16;
    for (index, count) in deck.iter().copied().filter(|(_, count)| *count > 0) {
        let card = &pool.cards[index];
        let mut card_unrestricted = [false; 6];
        let mut card_conditional = [false; 6];
        let mut card_unusable = [false; 6];
        let mut card_action_colors = [false; 6];
        for mode in card
            .features
            .initial_modes
            .iter()
            .filter(|mode| mode.is_land)
        {
            if mode.mana_sources.is_empty() {
                for color in &mode.mana_source_colors {
                    card_unrestricted[mana_slot(*color)] = true;
                    card_action_colors[mana_slot(*color)] = true;
                }
            }
            for ability in &mode.mana_sources {
                let colors = resolved_source_colors(ability, context, &legendary_colors);
                let unavailable = ability.spend_rule == ManaSpendRule::UnknownRestricted
                    || (ability.color_rule == ManaColorRule::CommanderIdentity
                        && !context.commander);
                let unrestricted = ability.spend_rule == ManaSpendRule::Unrestricted
                    && ability.color_rule == ManaColorRule::Fixed
                    && ability.state_rule == ManaStateRule::None;
                let predicate_has_target = !unavailable
                    && !unrestricted
                    && deck.iter().any(|(spell_index, _)| {
                        pool.cards[*spell_index]
                            .features
                            .initial_modes
                            .iter()
                            .filter(|spell_mode| !spell_mode.is_land)
                            .any(|spell_mode| {
                                spend_rule_allows(
                                    &ability.spend_rule,
                                    spell_mode,
                                    chosen_creature_type.as_deref(),
                                )
                            })
                    });
                for slot in 0..6 {
                    if !ability.colors.iter().any(|color| mana_slot(*color) == slot)
                        && !colors[slot]
                    {
                        continue;
                    }
                    if unavailable {
                        card_unusable[slot] = true;
                    } else if unrestricted {
                        card_unrestricted[slot] = true;
                    } else if predicate_has_target && colors[slot] {
                        card_conditional[slot] = true;
                    } else {
                        card_unusable[slot] = true;
                    }
                }
                if ability.activation_generic == 0
                    && ability.earliest_turn <= turn_horizon
                    && ability.spend_rule == ManaSpendRule::Unrestricted
                {
                    for (slot, produces) in colors.into_iter().enumerate() {
                        card_action_colors[slot] |= produces;
                    }
                }
            }
        }
        let copies = u16::from(count);
        for slot in 0..6 {
            if card_unrestricted[slot] {
                fitness.unrestricted_early_sources[slot] =
                    fitness.unrestricted_early_sources[slot].saturating_add(copies);
            }
            if card_conditional[slot] {
                fitness.conditional_early_sources[slot] =
                    fitness.conditional_early_sources[slot].saturating_add(copies);
            }
            if card_unusable[slot] && !card_unrestricted[slot] && !card_conditional[slot] {
                fitness.unusable_sources[slot] =
                    fitness.unusable_sources[slot].saturating_add(copies);
            }
            if card_action_colors[slot] {
                action_source_counts[slot] = action_source_counts[slot].saturating_add(copies);
            }
        }
        if card_action_colors.into_iter().any(|produces| produces) || card.features.is_land {
            action_physical_sources = action_physical_sources.saturating_add(copies);
        }
    }
    for slot in 0..6 {
        fitness.early_sources[slot] = fitness.unrestricted_early_sources[slot]
            .saturating_add(fitness.conditional_early_sources[slot]);
    }

    for (index, count) in deck.iter().copied().filter(|(_, count)| *count > 0) {
        let unsupported = pool.cards[index]
            .features
            .initial_modes
            .iter()
            .filter(|mode| mode.is_land)
            .flat_map(|mode| &mode.mana_sources)
            .filter(|ability| ability.spend_rule != ManaSpendRule::Unrestricted)
            .any(|ability| {
                ability.spend_rule == ManaSpendRule::UnknownRestricted
                    || !deck.iter().any(|(spell_index, _)| {
                        pool.cards[*spell_index]
                            .features
                            .initial_modes
                            .iter()
                            .filter(|mode| !mode.is_land)
                            .any(|mode| {
                                spend_rule_allows(
                                    &ability.spend_rule,
                                    mode,
                                    chosen_creature_type.as_deref(),
                                )
                            })
                    })
            });
        if unsupported {
            unsupported_narrow_sources += u32::from(count);
        }
    }

    for (index, count) in deck.iter().copied().filter(|(_, count)| *count > 0) {
        let card = &pool.cards[index];
        let copies = u32::from(count);
        let coverage = &card.features.oracle_coverage;
        let copies_f64 = f64::from(copies);
        oracle_gameplay_weight += f64::from(coverage.gameplay_clauses) * copies_f64;
        oracle_accounted_weight += f64::from(coverage.accounted_clauses) * copies_f64;
        oracle_parsed_weight += f64::from(coverage.parsed_clauses) * copies_f64;
        oracle_unresolved_weight += f64::from(coverage.unresolved_clauses) * copies_f64;
        if verbose && coverage.unresolved_clauses > 0 {
            fitness.unresolved_oracle_text.extend(
                coverage
                    .unresolved_text
                    .iter()
                    .map(|text| format!("{}: {text}", card.name)),
            );
        }
        let functional_target = preferred_copy_target(card);
        let excess_copies = count.saturating_sub(functional_target);
        if !card.features.is_land {
            excess_nonland_copies += u32::from(excess_copies);
        }
        if verbose {
            fitness.copy_evidence.push(CopyEvidence {
                card_id: card.id.clone(),
                card_name: card.name.clone(),
                copies: count,
                functional_target,
                rationale: copy_rationale(card).into(),
                excess_copies,
            });
        }
        let modes = &card.features.initial_modes;
        let has_land_mode = modes.iter().any(|mode| mode.is_land);
        if (has_land_mode || card.features.is_land) && card.type_line.contains("Basic Land") {
            basics += copies;
        }

        if !card.features.is_land {
            nonland_copies += copies;
            nonland_mana_value += f64::from(card.features.mana_value) * f64::from(copies);
            if card.features.pressure > 0.0 {
                threat_copies += copies;
            }
            if card.features.initial_modes.iter().any(|mode| {
                mode.abilities.iter().any(|ability| {
                    matches!(
                        ability.kind,
                        AbilityKind::Copy
                            | AbilityKind::TokenMultiplier
                            | AbilityKind::AlternateWin
                    )
                })
            }) {
                engine_copies += copies;
            }
            let cast_mode = modes.iter().find(|mode| !mode.is_land);
            if cast_mode.is_none() && card.features.pressure > 0.0 {
                // Legacy catalog/test records may predate initial modes. Preserve a
                // deliberately small combat-path contribution so they remain
                // inspectable, but never let legacy scalar pressure rival verified
                // v5 functional evidence.
                closing_supply += f64::from(copies) * f64::from(card.features.pressure) * 0.10;
            }
            if let Some(mode) = cast_mode {
                for slot in 0..6 {
                    fitness.colored_pip_demand[slot] = fitness.colored_pip_demand[slot]
                        .saturating_add(u16::from(mode.mana_cost.colored[slot]) * u16::from(count));
                }
                let mut evidence = castability_for_card(
                    index,
                    count,
                    mode,
                    deck,
                    pool,
                    turn_horizon,
                    context,
                    &legendary_colors,
                    chosen_creature_type.as_deref(),
                    verbose,
                );
                // Source counts answer whether a spell can ever be paid for, while the
                // trial model observes whether this particular deck actually develops
                // mana on time. Combine both so a slow five-color pile cannot report
                // perfect per-card castability merely because it contains enough fixing.
                evidence.on_curve_castability *= (1.0 - fitness.mana_failure_rate).clamp(0.0, 1.0);
                evidence.turn_horizon_castability *=
                    (1.0 - fitness.mana_failure_rate * 0.5).clamp(0.0, 1.0);
                if evidence.turn_horizon_castability <= f64::EPSILON
                    && mode.mana_cost.colored.iter().any(|pips| *pips > 0)
                {
                    unreachable_required_symbol = true;
                    if verbose {
                        fitness
                            .mana_blocking_reasons
                            .extend(evidence.reasons.clone());
                    }
                } else if count >= 3 && evidence.turn_horizon_castability < 0.35 {
                    uncastable_core = true;
                    if verbose {
                        fitness.mana_blocking_reasons.push(format!(
                            "{}: {count} copies, only {:.0}% castable by the turn horizon",
                            card.name,
                            evidence.turn_horizon_castability * 100.0
                        ));
                    }
                }
                let card_castability = evidence.turn_horizon_castability;
                let copies_weight = f64::from(count);
                castability_copies += copies_weight;
                horizon_castability_sum += card_castability * copies_weight;
                curve_castability_sum += evidence.on_curve_castability * copies_weight;
                if verbose {
                    fitness.card_castability.push(evidence);
                }
                // A castable permanent with pressure is itself a victory path even
                // when it has no separately parsed rules action. This keeps combat
                // threats distinct from text-derived damage while still requiring
                // the spell to be reachable in the deck's mana model.
                if card.features.pressure > 0.0 {
                    closing_supply +=
                        f64::from(count) * f64::from(card.features.pressure) * card_castability;
                }
                for ability in card
                    .features
                    .initial_modes
                    .iter()
                    .filter(|mode| !mode.is_land)
                    .flat_map(|mode| mode.abilities.iter())
                {
                    let stored_exile_supply = if card
                        .features
                        .initial_modes
                        .iter()
                        .flat_map(|mode| mode.abilities.iter())
                        .any(|candidate| candidate.kind == AbilityKind::GraveyardHate)
                    {
                        f64::from(context.opponent_field.graveyard_rate)
                            * f64::from(context.opponent_field.creature_rate)
                    } else {
                        0.0
                    };
                    let (mana_availability, target_availability, availability, reasons) =
                        action_availability(
                            ability,
                            card,
                            card_castability,
                            turn_horizon,
                            context,
                            &action_source_counts,
                            action_physical_sources,
                            snapshot_creatures,
                            snapshot_basics,
                            snapshot_graveyard_setup,
                            stored_exile_supply,
                            verbose,
                        );
                    let copies_weight = f64::from(count);
                    let contribution = availability
                        * f64::from(ability.effect_value)
                        * f64::from(ability.parser_confidence);
                    action_weight += copies_weight;
                    functional_action_weight += copies_weight * availability;
                    let narrow = ability_is_narrow(card, ability);
                    if narrow && target_availability < 0.50 {
                        narrow_action_weight += copies_weight * (1.0 - target_availability);
                    }
                    if matches!(
                        ability.kind,
                        AbilityKind::Destroy
                            | AbilityKind::Exile
                            | AbilityKind::Bounce
                            | AbilityKind::CounterSpell
                            | AbilityKind::GraveyardHate
                    ) {
                        functional_interaction += copies_weight * contribution;
                    }
                    if matches!(ability.kind, AbilityKind::Draw | AbilityKind::Recursion) {
                        card_flow_supply += copies_weight * contribution;
                    }
                    if matches!(
                        ability.kind,
                        AbilityKind::CreateToken
                            | AbilityKind::Copy
                            | AbilityKind::TokenMultiplier
                            | AbilityKind::Damage
                            | AbilityKind::Mill
                            | AbilityKind::AlternateWin
                    ) {
                        closing_supply += copies_weight * contribution;
                    }
                    if verbose && availability < 0.50 {
                        record_warning!(format!(
                            "{}: {} is only {:.0}% functionally available",
                            card.name,
                            ability_kind_label(ability.kind),
                            availability * 100.0
                        ));
                    }
                    if verbose {
                        fitness.action_evidence.push(ActionEvidence {
                            card_id: card.id.clone(),
                            card_name: card.name.clone(),
                            action: ability.label.clone(),
                            kind: ability_kind_label(ability.kind).into(),
                            zone: format!("{:?}", ability.zone).to_ascii_lowercase(),
                            conditions: ability.conditions.clone(),
                            copies: count,
                            availability,
                            mana_availability,
                            target_availability,
                            contribution,
                            narrow,
                            reasons,
                        });
                    }
                }
            }
            if card.features.is_legendary && count > 2 {
                legendary_excess += u32::from(count - 2);
            }
            if card.features.is_legendary {
                legendary_copies += copies;
            }
            if (card.type_line.contains("Creature")
                || card.type_line.contains("Artifact")
                || card.type_line.contains("Enchantment")
                || card.type_line.contains("Planeswalker"))
                && let Some(mode) = cast_mode
            {
                for (slot, pips) in mode.mana_cost.colored[..5].iter().enumerate() {
                    permanent_pips_by_color[slot] += u32::from(*pips) * copies;
                }
            }
        }

        if card.type_line.contains("Creature") {
            creature_copies += copies;
            creature_power += copies * card.features.pressure.max(1.0).round() as u32;
            if modes
                .iter()
                .any(|mode| mode.creature_types.iter().any(|kind| kind == "Human"))
            {
                humans += copies;
            }
        }
        let mechanics = &card.features.mechanics;
        if mechanics.requires_creature
            || mechanics.requires_humans
            || mechanics.requires_death
            || mechanics.reanimation
            || mechanics.legendary_payoff
            || mechanics.crew_power > 0
            || mechanics.devotion_threshold > 0
            || mechanics.opponent_discard_demand > 0
            || mechanics.sacrifice_demand > 0
            || mechanics.graveyard_target_demand > 0
            || mechanics.graveyard_setup_demand > 0
            || mechanics.legendary_cast_demand > 0
        {
            explicit_payoff_copies += copies;
        }
        human_payoff |= mechanics.requires_humans;
        crew_demand += copies * u32::from(mechanics.crew_power);
        needs_creature |= mechanics.requires_creature;
        needs_death |= mechanics.requires_death;
        death_supply += copies * u32::from(mechanics.provides_death);
        reanimation |= mechanics.reanimation;
        graveyard_setup |= mechanics.graveyard_setup;
        basic_search += copies * u32::from(mechanics.searches_basic_land);
        legendary_payoff |= mechanics.legendary_payoff;
        if mechanics.devotion_threshold > 0 {
            devotion_profiles.push((
                card.name.clone(),
                mechanics.devotion_color_mask,
                mechanics.devotion_threshold,
            ));
        }
        opponent_discard_demand += copies * u32::from(mechanics.opponent_discard_demand);
        opponent_discard_supply += copies * u32::from(mechanics.opponent_discard_supply);
        sacrifice_demand += copies * u32::from(mechanics.sacrifice_demand);
        sacrifice_outlets += copies * u32::from(mechanics.sacrifice_outlet_supply);
        sacrifice_fodder += copies * u32::from(mechanics.sacrifice_fodder_supply);
        graveyard_target_demand += copies * u32::from(mechanics.graveyard_target_demand);
        graveyard_target_supply += copies * u32::from(mechanics.graveyard_target_supply);
        graveyard_setup_demand += copies * u32::from(mechanics.graveyard_setup_demand);
        graveyard_setup_supply += copies * u32::from(mechanics.graveyard_setup_supply);

        if let Some(search) = &mechanics.search_action {
            let targets = deck
                .iter()
                .map(|(target_index, target_count)| {
                    u32::from(*target_count)
                        * u32::from(search_target_matches(
                            &search.target,
                            &pool.cards[*target_index],
                        ))
                })
                .sum::<u32>();
            total_requirement += copies as f64;
            let deficit = copies.saturating_sub(targets);
            record_requirement!(RequirementEvidence {
                mechanic: format!("{} search targets", card.name),
                demand: copies as f64,
                supply: targets.min(copies) as f64,
                fulfillment: targets.min(copies) as f64 / copies.max(1) as f64,
            });
            if deficit > 0 {
                individual_search_deficit = individual_search_deficit.saturating_add(deficit);
                if targets == 0 {
                    record_warning!(format!(
                        "{}: no searchable basic land or Desert targets",
                        card.name
                    ));
                } else {
                    record_warning!(format!(
                        "{}: {copies} search copies but only {targets} basic land/Desert targets",
                        card.name,
                    ));
                }
            }
        }
    }

    // Search targets are consumable across the whole deck. Two different Bell
    // effects cannot each claim the same lone basic as full supply.
    let shared_search_deficit = basic_search.saturating_sub(basics);
    unmet_requirement += f64::from(individual_search_deficit.max(shared_search_deficit));
    if basic_search > 0 {
        let shared_deficit = shared_search_deficit;
        if shared_deficit > individual_search_deficit {
            record_warning!(format!(
                "{basic_search} basic-land search copies compete for only {basics} consumable targets"
            ));
        }
    }

    fitness.turn_horizon_castability = if castability_copies == 0.0 {
        1.0
    } else {
        horizon_castability_sum / castability_copies
    };
    fitness.on_curve_castability = if castability_copies == 0.0 {
        1.0
    } else {
        curve_castability_sum / castability_copies
    };
    fitness.stranded_card_rate = (1.0 - fitness.turn_horizon_castability).clamp(0.0, 1.0);
    fitness.copy_excess_rate =
        (f64::from(excess_nonland_copies) / f64::from(nonland_copies.max(1))).clamp(0.0, 1.0);
    action_weight += oracle_unresolved_weight;
    let clause_denominator = oracle_gameplay_weight.max(1.0);
    fitness.accounted_clause_rate = (oracle_accounted_weight / clause_denominator).clamp(0.0, 1.0);
    fitness.parsed_clause_rate = (oracle_parsed_weight / clause_denominator).clamp(0.0, 1.0);
    fitness.unresolved_clause_rate =
        (oracle_unresolved_weight / clause_denominator).clamp(0.0, 1.0);
    fitness.unresolved_oracle_text.sort();
    fitness.unresolved_oracle_text.dedup();
    fitness.functional_action_rate = if action_weight > 0.0 {
        (functional_action_weight / action_weight).clamp(0.0, 1.0)
    } else {
        1.0
    };
    fitness.dead_action_rate = 1.0 - fitness.functional_action_rate;
    fitness.narrow_mainboard_rate = if action_weight > 0.0 {
        (narrow_action_weight / action_weight).clamp(0.0, 1.0)
    } else {
        0.0
    };
    fitness.interaction = (functional_interaction / 8.0).clamp(0.0, 1.0);
    let average_mana_value = nonland_mana_value / f64::from(nonland_copies.max(1));
    fitness.archetype = if context.commander {
        "commander".into()
    } else if engine_copies >= 8 {
        "combo/engine".into()
    } else if threat_copies >= 16 && average_mana_value <= 2.8 {
        "aggro".into()
    } else if functional_interaction >= 8.0 && card_flow_supply >= 4.0 {
        "control".into()
    } else {
        "midrange/value".into()
    };
    let closing_target = match fitness.archetype.as_str() {
        "aggro" => 14.0,
        "combo/engine" => 16.0,
        "control" => 10.0,
        "commander" => 24.0,
        _ => 32.0,
    };
    fitness.closing_power = (closing_supply / closing_target).clamp(0.0, 1.0);
    let flow_fulfillment = (card_flow_supply / 8.0).clamp(0.0, 1.0);
    fitness.role_balance = match fitness.archetype.as_str() {
        "aggro" => {
            fitness.closing_power * 0.55
                + fitness.tempo * 0.25
                + fitness.consistency * 0.15
                + fitness.interaction * 0.05
        }
        "combo/engine" => {
            fitness.closing_power * 0.45
                + fitness.synergy * 0.35
                + fitness.consistency * 0.15
                + fitness.interaction * 0.05
        }
        "control" => {
            fitness.interaction * 0.35
                + flow_fulfillment * 0.30
                + fitness.resilience * 0.20
                + fitness.closing_power * 0.15
        }
        "commander" => {
            fitness.closing_power * 0.30
                + fitness.interaction * 0.20
                + fitness.resilience * 0.25
                + fitness.synergy * 0.25
        }
        _ => {
            fitness.closing_power * 0.40
                + flow_fulfillment * 0.20
                + fitness.consistency * 0.20
                + fitness.interaction * 0.20
        }
    }
    .clamp(0.0, 1.0);
    if verbose {
        fitness.role_evidence = vec![
            RoleEvidence {
                role: "closing plan".into(),
                copies: closing_supply,
                target: closing_target,
                fulfillment: fitness.closing_power,
            },
            RoleEvidence {
                role: "functional interaction".into(),
                copies: functional_interaction,
                target: 8.0,
                fulfillment: fitness.interaction,
            },
            RoleEvidence {
                role: "card flow".into(),
                copies: card_flow_supply,
                target: 8.0,
                fulfillment: flow_fulfillment,
            },
        ];
    }
    if closing_supply <= f64::EPSILON {
        fitness.severe_structural_failure = true;
        fitness
            .structural_blocking_reasons
            .push("No reachable threat, damage, mill, token, copy, or alternate-win path".into());
    } else if verbose && fitness.closing_power < 0.50 {
        record_warning!(format!(
            "The {} deck has a weak or slow closing plan",
            fitness.archetype
        ));
    }
    fitness.legendary_dead_draw_rate = if nonland_copies == 0 {
        0.0
    } else {
        legendary_excess as f64 / f64::from(nonland_copies)
    };
    if fitness.turn_horizon_castability < 0.5 && verbose {
        fitness.mana_blocking_reasons.push(format!(
            "Only {:.0}% of nonland copies are castable by the turn horizon",
            fitness.turn_horizon_castability * 100.0
        ));
    }
    if opponent_discard_demand > 0 {
        total_requirement += opponent_discard_demand as f64;
        let deficit = opponent_discard_demand.saturating_sub(opponent_discard_supply);
        unmet_requirement += deficit as f64;
        record_requirement!(RequirementEvidence {
            mechanic: "opponent discard".into(),
            demand: opponent_discard_demand as f64,
            supply: opponent_discard_supply.min(opponent_discard_demand) as f64,
            fulfillment: 1.0 - deficit as f64 / opponent_discard_demand as f64,
        });
        if deficit > 0 {
            record_warning!(format!(
                "Opponent-discard payoff demand {opponent_discard_demand}, supply {opponent_discard_supply}"
            ));
        }
    }
    if sacrifice_demand > 0 {
        total_requirement += sacrifice_demand as f64;
        let supply = sacrifice_outlets.min(sacrifice_fodder);
        let deficit = sacrifice_demand.saturating_sub(supply);
        unmet_requirement += deficit as f64;
        record_requirement!(RequirementEvidence {
            mechanic: "sacrifice outlet and fodder".into(),
            demand: sacrifice_demand as f64,
            supply: supply.min(sacrifice_demand) as f64,
            fulfillment: 1.0 - deficit as f64 / sacrifice_demand as f64,
        });
        if deficit > 0 {
            record_warning!(format!(
                "Sacrifice payoff demand {sacrifice_demand}, sustainable fodder/outlet supply {supply}"
            ));
        }
    }
    if graveyard_target_demand > 0 {
        total_requirement += f64::from(graveyard_target_demand);
        let deficit = graveyard_target_demand.saturating_sub(graveyard_target_supply);
        unmet_requirement += f64::from(deficit);
        record_requirement!(RequirementEvidence {
            mechanic: "graveyard targets".into(),
            demand: f64::from(graveyard_target_demand),
            supply: f64::from(graveyard_target_supply.min(graveyard_target_demand)),
            fulfillment: 1.0 - f64::from(deficit) / f64::from(graveyard_target_demand),
        });
        if deficit > 0 {
            record_warning!(format!(
                "Graveyard-target demand {graveyard_target_demand}, target supply {graveyard_target_supply}"
            ));
        }
    }
    if graveyard_setup_demand > 0 {
        total_requirement += f64::from(graveyard_setup_demand);
        let deficit = graveyard_setup_demand.saturating_sub(graveyard_setup_supply);
        unmet_requirement += f64::from(deficit);
        record_requirement!(RequirementEvidence {
            mechanic: "graveyard setup".into(),
            demand: f64::from(graveyard_setup_demand),
            supply: f64::from(graveyard_setup_supply.min(graveyard_setup_demand)),
            fulfillment: 1.0 - f64::from(deficit) / f64::from(graveyard_setup_demand),
        });
        if deficit > 0 {
            record_warning!(format!(
                "Graveyard-setup demand {graveyard_setup_demand}, setup supply {graveyard_setup_supply}"
            ));
        }
    }
    if unsupported_narrow_sources > 0 {
        total_requirement += f64::from(unsupported_narrow_sources);
        unmet_requirement += f64::from(unsupported_narrow_sources);
        record_requirement!(RequirementEvidence {
            mechanic: "narrow mana source demand".into(),
            demand: f64::from(unsupported_narrow_sources),
            supply: 0.0,
            fulfillment: 0.0,
        });
        record_warning!(format!(
            "{unsupported_narrow_sources} narrow mana sources have no matching spells or payable actions"
        ));
    }
    if crew_demand > 0 {
        total_requirement += f64::from(crew_demand);
        unmet_requirement += f64::from(crew_demand.saturating_sub(creature_power));
        record_requirement!(RequirementEvidence {
            mechanic: "crew power".into(),
            demand: f64::from(crew_demand),
            supply: f64::from(creature_power.min(crew_demand)),
            fulfillment: f64::from(creature_power.min(crew_demand)) / f64::from(crew_demand),
        });
    }
    if needs_creature {
        total_requirement += 1.0;
        unmet_requirement += f64::from(creature_copies == 0);
        record_requirement!(RequirementEvidence {
            mechanic: "controlled creature".into(),
            demand: 1.0,
            supply: f64::from(creature_copies > 0),
            fulfillment: f64::from(creature_copies > 0),
        });
    }
    if needs_death {
        total_requirement += 1.0;
        unmet_requirement += f64::from(death_supply == 0);
        record_requirement!(RequirementEvidence {
            mechanic: "true death event".into(),
            demand: 1.0,
            supply: f64::from(death_supply > 0),
            fulfillment: f64::from(death_supply > 0),
        });
    }
    if human_payoff {
        total_requirement += 5.0;
        unmet_requirement += f64::from(5_u32.saturating_sub(humans));
        record_requirement!(RequirementEvidence {
            mechanic: "Human density".into(),
            demand: 5.0,
            supply: f64::from(humans.min(5)),
            fulfillment: f64::from(humans.min(5)) / 5.0,
        });
    }
    if reanimation {
        total_requirement += 2.0;
        unmet_requirement += f64::from(creature_copies < 6) + f64::from(!graveyard_setup);
        let supply = u32::from(creature_copies >= 6) + u32::from(graveyard_setup);
        record_requirement!(RequirementEvidence {
            mechanic: "reanimation targets and setup".into(),
            demand: 2.0,
            supply: f64::from(supply),
            fulfillment: f64::from(supply) / 2.0,
        });
    }
    if legendary_payoff {
        total_requirement += 8.0;
        unmet_requirement += f64::from(8_u32.saturating_sub(legendary_copies));
        record_requirement!(RequirementEvidence {
            mechanic: "legendary casts".into(),
            demand: 8.0,
            supply: f64::from(legendary_copies.min(8)),
            fulfillment: f64::from(legendary_copies.min(8)) / 8.0,
        });
    }
    for (name, color_mask, threshold) in &devotion_profiles {
        let supply = permanent_pips_by_color
            .iter()
            .enumerate()
            .filter(|(slot, _)| color_mask & (1 << slot) != 0)
            .map(|(_, pips)| *pips)
            .sum::<u32>();
        let target = u32::from(*threshold) * 2;
        total_requirement += f64::from(target);
        unmet_requirement += f64::from(target.saturating_sub(supply));
        record_requirement!(RequirementEvidence {
            mechanic: format!("{name} devotion"),
            demand: f64::from(target),
            supply: f64::from(supply.min(target)),
            fulfillment: f64::from(supply.min(target)) / f64::from(target.max(1)),
        });
        if supply < target {
            record_warning!(format!(
                "{name}: devotion colors have {supply} permanent pips for reliable threshold {threshold}"
            ));
        }
    }
    let has_explicit_requirements = total_requirement > 0.0;
    fitness.unmet_requirement_rate = if has_explicit_requirements {
        (unmet_requirement / total_requirement).clamp(0.0, 1.0)
    } else {
        0.0
    };
    if has_explicit_requirements {
        let payoff_density =
            (f64::from(explicit_payoff_copies) / f64::from(nonland_copies.max(1))).clamp(0.0, 1.0);
        fitness.synergy = (fitness.synergy * 0.20
            + (1.0 - fitness.unmet_requirement_rate) * payoff_density * 0.30)
            .clamp(0.0, 0.65);
    } else {
        // Generic catalog-tag overlap is useful as a weak fallback, but it is
        // not proof of an engine/payoff relationship.
        fitness.synergy *= 0.20;
    }
    if fitness.required_color_count >= 4 {
        record_warning!(format!(
            "{}-color spell suite receives a {:.3} reliability deduction",
            fitness.required_color_count, fitness.color_complexity_penalty
        ));
    }
    fitness.severe_mana_failure = unreachable_required_symbol
        || uncastable_core
        || fitness.turn_horizon_castability < 0.5
        || !fitness.mana_blocking_reasons.is_empty();
    if crew_demand > creature_power {
        record_warning!("Crew requirements exceed available creature power".into());
    }
    if needs_creature && creature_copies == 0 {
        record_warning!("Aura or creature-target payoff has no controlled-creature supply".into());
    }
    if needs_death && death_supply == 0 {
        record_warning!(
            "Death payoff has no true sacrifice or destroy supply; exile costs do not count".into()
        );
    }
    if human_payoff && humans.saturating_mul(5) < nonland_copies {
        record_warning!("Human-specific payoff has low Human density".into());
    }
    if reanimation && (creature_copies < 6 || !graveyard_setup) {
        record_warning!("Reanimation lacks creature targets or graveyard setup".into());
    }
    if basic_search > 0 && basics == 0 {
        record_warning!("Basic-land search has no searchable basic lands".into());
    }
    if legendary_payoff && legendary_copies < 8 {
        record_warning!(
            "Once-per-turn legendary-cast payoff has low legendary spell supply".into()
        );
    }
    fitness.legendary_penalty = fitness.legendary_dead_draw_rate * 0.12;
    fitness.unsupported_synergy_penalty = fitness.unmet_requirement_rate * 0.20;
}

#[allow(clippy::too_many_arguments)]
fn action_availability(
    ability: &AbilityProfile,
    card: &crate::Card,
    card_castability: f64,
    turn_horizon: u8,
    context: &StructuralEvaluationContext,
    source_counts: &[u16; 6],
    physical_sources: u16,
    creature_copies: u32,
    basic_copies: u32,
    graveyard_setup_copies: u32,
    stored_exile_supply: f64,
    collect_reasons: bool,
) -> (f64, f64, f64, Vec<String>) {
    let mut reasons = Vec::new();
    macro_rules! record_reason {
        ($reason:expr) => {
            if collect_reasons {
                reasons.push($reason);
            }
        };
    }
    let mana = &ability.cost.mana;
    let activation_value = usize::from(mana.generic)
        + mana
            .colored
            .iter()
            .map(|pips| usize::from(*pips))
            .sum::<usize>()
        + mana.hybrid.len();
    let cast_turn = card.features.mana_value.max(0.0).ceil() as usize;
    let timing_availability = if ability.zone == AbilityZone::Battlefield
        && ability.timing == crate::AbilityTiming::Activated
        && cast_turn.saturating_add(activation_value) > usize::from(turn_horizon)
    {
        record_reason!(format!(
            "requires about turn {} after casting",
            cast_turn.saturating_add(activation_value)
        ));
        0.0
    } else {
        1.0
    };
    let zone_availability = match ability.zone {
        AbilityZone::Stack => card_castability,
        AbilityZone::Battlefield => {
            if card.features.is_land {
                1.0
            } else {
                card_castability
            }
        }
        AbilityZone::Hand => 1.0,
        AbilityZone::Graveyard => {
            let availability = (f64::from(graveyard_setup_copies) / 8.0).clamp(0.0, 1.0);
            if availability < 0.25 {
                record_reason!("ability requires this card to reach the graveyard".into());
            }
            availability
        }
        AbilityZone::Exile | AbilityZone::Command => 0.25,
        AbilityZone::Library => 0.0,
    };

    let mut mana_availability = if activation_value == 0 {
        1.0
    } else if physical_sources == 0 || usize::from(mana.generic) > usize::from(turn_horizon) {
        0.0
    } else {
        1.0
    };
    for (slot, pips) in mana.colored.iter().copied().enumerate() {
        if pips == 0 {
            continue;
        }
        if source_counts[slot] == 0 {
            mana_availability = 0.0;
            record_reason!(format!(
                "no eligible {} source for the activated cost",
                mana_color_label(slot_color(slot))
            ));
        } else {
            mana_availability *= (f64::from(source_counts[slot])
                / f64::from(pips.saturating_mul(4).max(1)))
            .clamp(0.35, 1.0);
        }
    }
    for hybrid in &mana.hybrid {
        if !hybrid
            .iter()
            .any(|color| source_counts[mana_slot(*color)] > 0)
        {
            mana_availability = 0.0;
            record_reason!("no eligible source for a hybrid activated cost".into());
        }
    }
    mana_availability *= timing_availability;

    let mut nonmana_availability = 1.0_f64;
    if ability.cost.sacrifice_permanents > 0 {
        let needed = f64::from(ability.cost.sacrifice_permanents) * 8.0;
        let supply = (f64::from(creature_copies) / needed.max(1.0)).clamp(0.0, 1.0);
        nonmana_availability *= supply;
        if supply < 0.50 {
            record_reason!("insufficient sacrifice fodder for the activation cost".into());
        }
    }
    if ability.cost.discard_cards > 0 {
        // A structural trial does not model every hand resource after casting.
        // Charge a conservative availability cost per discarded card instead
        // of treating discard as free.
        nonmana_availability *= 0.72_f64.powi(i32::from(ability.cost.discard_cards));
    }
    if ability.cost.exile_cards > 0 {
        let supply = (f64::from(graveyard_setup_copies)
            / (4.0 * f64::from(ability.cost.exile_cards)))
        .clamp(0.0, 1.0);
        nonmana_availability *= supply;
        if supply < 0.50 {
            record_reason!("insufficient graveyard setup for the exile cost".into());
        }
    }
    if ability.cost.remove_counters > 0 {
        let counter_supply = f64::from(
            card.oracle_text.to_ascii_lowercase().contains("put a ")
                && card.oracle_text.to_ascii_lowercase().contains(" counter"),
        );
        nonmana_availability *= counter_supply;
        if counter_supply == 0.0 {
            record_reason!("no verified way to create the counters required by the cost".into());
        }
    }
    if ability
        .conditions
        .iter()
        .any(|condition| condition.contains("previously exiled with this permanent"))
    {
        nonmana_availability *= stored_exile_supply;
        if stored_exile_supply < 0.25 {
            record_reason!("six-mana payoff has not stored enough exiled creature cards".into());
        }
    }

    let mut target_availability = match ability.target {
        ActionTargetProfile::None | ActionTargetProfile::OpponentPlayer => 1.0,
        ActionTargetProfile::OwnCreature => (f64::from(creature_copies) / 8.0).clamp(0.0, 1.0),
        ActionTargetProfile::OpponentCreature | ActionTargetProfile::AnyCreature => {
            f64::from(context.opponent_field.creature_rate).clamp(0.0, 1.0)
        }
        ActionTargetProfile::ArtifactOrEnchantment => {
            f64::from(context.opponent_field.artifact_or_enchantment_rate).clamp(0.0, 1.0)
        }
        ActionTargetProfile::GraveyardCard => {
            f64::from(context.opponent_field.graveyard_rate).clamp(0.0, 1.0)
        }
        ActionTargetProfile::OwnGraveyardCreature => ((f64::from(creature_copies) / 8.0)
            .min(f64::from(graveyard_setup_copies) / 4.0))
        .clamp(0.0, 1.0),
        ActionTargetProfile::Spell => {
            f64::from(context.opponent_field.stack_spell_rate).clamp(0.0, 1.0)
        }
        ActionTargetProfile::BasicLandInLibrary => f64::from(basic_copies > 0),
    };
    if ability_is_graveyard_hate(card, ability) {
        target_availability = f64::from(context.opponent_field.graveyard_rate).clamp(0.0, 1.0);
    }
    if target_availability <= f64::EPSILON && ability.target != ActionTargetProfile::None {
        record_reason!("no reachable useful target".into());
    } else if ability_is_narrow(card, ability) && target_availability < 0.50 {
        record_reason!(format!(
            "narrow target is expected in only {:.0}% of relevant games",
            target_availability * 100.0
        ));
    }
    let availability =
        (zone_availability * mana_availability * nonmana_availability * target_availability)
            .clamp(0.0, 1.0);
    (
        mana_availability,
        target_availability,
        availability,
        reasons,
    )
}

fn ability_kind_label(kind: AbilityKind) -> &'static str {
    match kind {
        AbilityKind::Unknown => "unverified action",
        AbilityKind::Draw => "draw action",
        AbilityKind::Destroy => "destroy action",
        AbilityKind::Exile => "exile action",
        AbilityKind::Bounce => "bounce action",
        AbilityKind::CounterSpell => "counterspell action",
        AbilityKind::AddCounters => "counter-placement action",
        AbilityKind::GraveyardHate => "graveyard action",
        AbilityKind::Recursion => "recursion action",
        AbilityKind::Search => "search action",
        AbilityKind::CreateToken => "token action",
        AbilityKind::Copy => "copy action",
        AbilityKind::TokenMultiplier => "token-multiplier action",
        AbilityKind::SacrificeOutlet => "sacrifice outlet",
        AbilityKind::Ramp => "mana action",
        AbilityKind::Crew => "crew action",
        AbilityKind::Damage => "damage action",
        AbilityKind::Mill => "mill action",
        AbilityKind::AlternateWin => "alternate-win action",
        AbilityKind::Blink => "blink action",
    }
}

fn mana_color_label(color: Color) -> &'static str {
    match color {
        Color::White => "white",
        Color::Blue => "blue",
        Color::Black => "black",
        Color::Red => "red",
        Color::Green => "green",
        Color::Colorless => "colorless",
    }
}

fn resolved_source_colors(
    ability: &ManaSourceProfile,
    context: &StructuralEvaluationContext,
    legendary_colors: &HashSet<Color>,
) -> [bool; 6] {
    let mut result = [false; 6];
    match ability.color_rule {
        ManaColorRule::Fixed => {
            for color in &ability.colors {
                result[mana_slot(*color)] = true;
            }
        }
        ManaColorRule::CommanderIdentity if context.commander => {
            for color in &context.commander_colors {
                result[mana_slot(*color)] = true;
            }
        }
        ManaColorRule::CommanderIdentity => {}
        ManaColorRule::LegendaryPermanents => {
            for color in legendary_colors {
                result[mana_slot(*color)] = true;
            }
        }
    }
    result
}

fn best_chosen_type_for_deck(deck: &[(usize, u8)], pool: &CardPool) -> Option<String> {
    let mut counts = HashMap::<String, u32>::new();
    for (index, copies) in deck {
        for mode in pool.cards[*index]
            .features
            .initial_modes
            .iter()
            .filter(|mode| !mode.is_land)
        {
            for kind in &mode.creature_types {
                *counts.entry(kind.to_ascii_lowercase()).or_default() += u32::from(*copies);
            }
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, copies)| *copies)
        .map(|(kind, _)| kind)
}

fn spend_rule_allows(rule: &ManaSpendRule, mode: &CardMode, chosen_type: Option<&str>) -> bool {
    let lower_type = mode.type_line.to_ascii_lowercase();
    match rule {
        ManaSpendRule::Unrestricted => true,
        ManaSpendRule::CastLegendary => mode.is_legendary,
        ManaSpendRule::CastCardTypes(types) => types
            .iter()
            .any(|kind| lower_type.contains(&kind.to_ascii_lowercase())),
        ManaSpendRule::CastSubtypes(types) => types.iter().any(|kind| {
            let kind = kind.to_ascii_lowercase();
            lower_type.contains(&kind)
                || mode
                    .creature_types
                    .iter()
                    .any(|subtype| subtype.eq_ignore_ascii_case(&kind))
        }),
        ManaSpendRule::CastChosenCreatureType => chosen_type.is_some_and(|chosen| {
            mode.creature_types
                .iter()
                .any(|subtype| subtype.eq_ignore_ascii_case(chosen))
        }),
        ManaSpendRule::UnknownRestricted => false,
    }
}

fn spend_rule_label(rule: &ManaSpendRule, chosen_type: Option<&str>) -> String {
    match rule {
        ManaSpendRule::Unrestricted => "unrestricted".into(),
        ManaSpendRule::CastLegendary => "legendary spells only".into(),
        ManaSpendRule::CastCardTypes(types) => format!("{} spells only", types.join("/")),
        ManaSpendRule::CastSubtypes(types) => format!("{} only", types.join("/")),
        ManaSpendRule::CastChosenCreatureType => format!(
            "chosen creature type only{}",
            chosen_type.map_or_else(String::new, |kind| format!(" ({kind})"))
        ),
        ManaSpendRule::UnknownRestricted => "unknown restriction—colored mana ignored".into(),
    }
}

#[allow(clippy::too_many_arguments)]
fn castability_for_card(
    index: usize,
    copies: u8,
    mode: &CardMode,
    deck: &[(usize, u8)],
    pool: &CardPool,
    turn_horizon: u8,
    context: &StructuralEvaluationContext,
    legendary_colors: &HashSet<Color>,
    chosen_creature_type: Option<&str>,
    collect_detail: bool,
) -> CardCastabilityEvidence {
    let card = &pool.cards[index];
    if mode.normal_castable == Some(false) {
        return CardCastabilityEvidence {
            card_id: if collect_detail {
                card.id.clone()
            } else {
                Default::default()
            },
            name: if collect_detail {
                card.name.clone()
            } else {
                Default::default()
            },
            copies,
            reasons: if collect_detail {
                vec![format!(
                    "{} has no printed mana cost and no modeled alternate-cast route",
                    card.name
                )]
            } else {
                Default::default()
            },
            ..CardCastabilityEvidence::default()
        };
    }
    let mut unrestricted = u16::MAX;
    let mut conditional = u16::MAX;
    let mut unusable = 0_u16;
    let mut reasons = Vec::new();
    let mut source_restrictions = Vec::new();
    let mut has_colored_requirement = false;
    const LABELS: [&str; 6] = ["white", "blue", "black", "red", "green", "colorless"];
    for (slot, pips) in mode.mana_cost.colored.iter().copied().enumerate() {
        if pips == 0 {
            continue;
        }
        has_colored_requirement = true;
        let mut slot_unrestricted = 0_u16;
        let mut slot_conditional = 0_u16;
        let mut slot_unusable = 0_u16;
        for (source_index, source_count) in deck.iter().copied() {
            let mut source_unrestricted = false;
            let mut source_conditional = false;
            let mut source_claims = false;
            for source_mode in pool.cards[source_index]
                .features
                .initial_modes
                .iter()
                .filter(|candidate| candidate.is_land)
            {
                if source_mode.mana_sources.is_empty()
                    && source_mode
                        .mana_source_colors
                        .iter()
                        .any(|color| mana_slot(*color) == slot)
                {
                    source_unrestricted = true;
                }
                for ability in &source_mode.mana_sources {
                    if ability.earliest_turn > turn_horizon {
                        continue;
                    }
                    let colors = resolved_source_colors(ability, context, legendary_colors);
                    source_claims |= ability.colors.iter().any(|color| mana_slot(*color) == slot);
                    if !colors[slot]
                        || !spend_rule_allows(&ability.spend_rule, mode, chosen_creature_type)
                    {
                        continue;
                    }
                    if ability.spend_rule == ManaSpendRule::Unrestricted
                        && ability.color_rule == ManaColorRule::Fixed
                        && ability.state_rule == ManaStateRule::None
                    {
                        source_unrestricted = true;
                    } else {
                        source_conditional = true;
                        if collect_detail {
                            source_restrictions.push(format!(
                                "{} — {}",
                                pool.cards[source_index].name,
                                spend_rule_label(&ability.spend_rule, chosen_creature_type)
                            ));
                        }
                    }
                }
            }
            if source_unrestricted {
                slot_unrestricted = slot_unrestricted.saturating_add(u16::from(source_count));
            } else if source_conditional {
                slot_conditional = slot_conditional.saturating_add(u16::from(source_count));
            } else if source_claims {
                slot_unusable = slot_unusable.saturating_add(u16::from(source_count));
            }
        }
        unrestricted = unrestricted.min(slot_unrestricted);
        conditional = conditional.min(slot_conditional);
        unusable = unusable.saturating_add(slot_unusable);
        if slot_unrestricted + slot_conditional == 0 {
            let requirement = if pips >= 2 {
                format!("{} pips (double-{} requirement)", pips, LABELS[slot])
            } else {
                format!("{} source", LABELS[slot])
            };
            if collect_detail {
                reasons.push(format!(
                    "{}: {copies} copies, no eligible early {requirement}",
                    card.name
                ));
            }
        }
    }
    if !has_colored_requirement {
        unrestricted = 0;
        conditional = 0;
    }
    let eligible = unrestricted.saturating_add(conditional);
    let max_pips = mode.mana_cost.colored.iter().copied().max().unwrap_or(0);
    let target = ((f64::from(copies) / 3.0).ceil() as u16 + 3 + u16::from(max_pips) * 2).min(14);
    let horizon = if !has_colored_requirement {
        1.0
    } else {
        (f64::from(eligible) / f64::from(target.max(1))).min(1.0)
    };
    let on_curve = if !has_colored_requirement {
        1.0
    } else {
        (f64::from(unrestricted) + f64::from(conditional) * 0.65) / f64::from(target.max(1))
    }
    .min(1.0);
    if collect_detail {
        source_restrictions.sort();
        source_restrictions.dedup();
    }
    CardCastabilityEvidence {
        card_id: if collect_detail {
            card.id.clone()
        } else {
            Default::default()
        },
        name: if collect_detail {
            card.name.clone()
        } else {
            String::new()
        },
        copies,
        on_curve_castability: on_curve,
        turn_horizon_castability: horizon,
        unrestricted_early_sources: unrestricted,
        conditional_early_sources: conditional,
        unusable_sources: unusable,
        source_restrictions,
        reasons,
    }
}

fn search_target_matches(target: &SearchTargetProfile, card: &crate::Card) -> bool {
    let lower = card.type_line.to_ascii_lowercase();
    match target {
        SearchTargetProfile::None => false,
        SearchTargetProfile::BasicLand => lower.contains("basic land"),
        SearchTargetProfile::BasicLandOrSubtypes(subtypes) => {
            lower.contains("basic land")
                || subtypes
                    .iter()
                    .any(|subtype| lower.contains(&subtype.to_ascii_lowercase()))
        }
        SearchTargetProfile::CardSubtypes(subtypes) => subtypes
            .iter()
            .any(|subtype| lower.contains(&subtype.to_ascii_lowercase())),
    }
}

#[derive(Default)]
struct TrialScratch {
    library: Vec<usize>,
    hand: Vec<usize>,
    sources: Vec<TrialManaSource>,
    used_sources: Vec<bool>,
}

#[allow(clippy::too_many_arguments)]
fn simulate_trial(
    base_library: &[usize],
    pool: &CardPool,
    seed: u64,
    turn_horizon: u8,
    context: &StructuralEvaluationContext,
    has_search_actions: bool,
    has_legendary_cards: bool,
    scratch: &mut TrialScratch,
) -> TrialScore {
    let mut rng = SplitMix64::new(seed);
    let TrialScratch {
        library,
        hand,
        sources,
        used_sources,
    } = scratch;
    library.clear();
    library.extend_from_slice(base_library);
    let visible_draws = (usize::from(turn_horizon) + 7).min(library.len());
    if has_search_actions {
        rng.shuffle(library);
    } else {
        rng.shuffle_prefix(library, visible_draws);
    }

    hand.clear();
    hand.extend(library.iter().take(7).copied());
    let mut cursor = hand.len();
    let opening_lands = hand
        .iter()
        .filter(|index| {
            let features = &pool.cards[**index].features;
            features.is_land || features.can_be_land
        })
        .count();
    let mulligan_penalty = if (2..=5).contains(&opening_lands) {
        0.0
    } else {
        if has_search_actions {
            rng.shuffle(library);
        } else {
            rng.shuffle_prefix(library, visible_draws);
        }
        hand.clear();
        hand.extend(library.iter().take(6).copied());
        cursor = hand.len();
        0.12
    };

    sources.clear();
    let mut payment_kind = SourcePaymentKind::Simple;
    let mut ramp = 0.0_f32;
    let mut cast_value = 0.0_f64;
    let mut interaction = 0.0_f64;
    let mut resilience = 0.0_f64;
    let mut battlefield_power = 0.0_f64;
    let mut projected_damage = 0.0_f64;
    // A trial casts only a small number of distinct cards/tags. A compact
    // vector avoids allocating two hash tables per structural trial while
    // still counting distinct card identities rather than duplicate copies.
    let mut seen_tags = Vec::<(&str, Vec<usize>)>::new();
    let mut missed_land_turns = 0_u8;
    let mut stranded_turns = 0_u32;
    let mut legendary_colors = HashSet::new();
    let mut legendary_permanents = HashSet::new();
    let mut legendary_dead_observations = 0_u32;
    let mut nonland_hand_observations = 0_u32;

    for turn in 1..=turn_horizon {
        // Permanents cast on prior turns may attack now. This is a conservative
        // goldfish proxy; blockers and interaction are handled by game validation.
        projected_damage += battlefield_power;
        if cursor < library.len() {
            hand.push(library[cursor]);
            cursor += 1;
        }

        if has_legendary_cards {
            for index in hand.iter() {
                let features = &pool.cards[*index].features;
                if !features.is_land
                    || (features.is_legendary && legendary_permanents.contains(index))
                {
                    nonland_hand_observations += 1;
                    if features.is_legendary && legendary_permanents.contains(index) {
                        legendary_dead_observations += 1;
                    }
                }
            }
        }

        used_sources.clear();
        used_sources.resize(sources.len(), false);
        let used = &mut *used_sources;
        let search_choice = has_search_actions
            .then(|| {
                find_payable_search(
                    hand,
                    library,
                    cursor,
                    pool,
                    sources,
                    used,
                    turn,
                    context,
                    &legendary_colors,
                )
            })
            .flatten();
        if let Some((searcher_position, target_position)) = search_choice {
            let searcher = hand[searcher_position];
            let search = pool.cards[searcher]
                .features
                .mechanics
                .search_action
                .as_ref()
                .expect("selected search action");
            let mut trial_sources = sources.clone();
            if pay_activation(
                &search.mana_cost,
                &mut trial_sources,
                used,
                turn,
                context,
                &legendary_colors,
            ) {
                *sources = trial_sources;
                hand.remove(searcher_position);
                let target = library.remove(target_position);
                hand.push(target);
            }
        }

        // Compute the hand's colored demand once per turn. The previous land
        // chooser rescanned every held spell for every color on every land
        // candidate, which made structural trials quadratic in hand size.
        let mut hand_color_demand = [0_usize; 6];
        for held in hand.iter().copied() {
            for spell_mode in pool.cards[held]
                .features
                .initial_modes
                .iter()
                .filter(|mode| !mode.is_land)
            {
                for (slot, pips) in spell_mode.mana_cost.colored.iter().enumerate() {
                    hand_color_demand[slot] += usize::from(*pips);
                }
            }
        }
        let land_choice = hand
            .iter()
            .enumerate()
            .filter_map(|(position, index)| {
                let card = &pool.cards[*index];
                if card.features.is_legendary && legendary_permanents.contains(index) {
                    return None;
                }
                let modes = card
                    .features
                    .initial_modes
                    .iter()
                    .filter(|mode| mode.is_land);
                let best_mode = modes.enumerate().max_by_key(|(_, mode)| {
                    mode.mana_source_colors
                        .iter()
                        .map(|color| hand_color_demand[mana_slot(*color)])
                        .sum::<usize>()
                });
                if let Some((mode, _)) = best_mode {
                    Some((position, mode))
                } else if card.features.is_land {
                    Some((position, usize::MAX))
                } else {
                    None
                }
            })
            .max_by_key(|(_, mode)| usize::from(*mode != usize::MAX));
        if let Some((position, mode_index)) = land_choice {
            let card_index = hand.remove(position);
            let card = &pool.cards[card_index];
            let mode = (mode_index != usize::MAX)
                .then(|| {
                    card.features
                        .initial_modes
                        .iter()
                        .filter(|mode| mode.is_land)
                        .nth(mode_index)
                })
                .flatten();
            if let Some(mode) = mode {
                let chosen_type = mode
                    .mana_sources
                    .iter()
                    .any(|ability| ability.spend_rule == ManaSpendRule::CastChosenCreatureType)
                    .then(|| most_needed_creature_type(hand, pool))
                    .flatten();
                sources.push(make_trial_source_from_mode(
                    mode,
                    turn + u8::from(mode.enters_tapped),
                    chosen_type,
                    u8::from(
                        mode.mana_sources
                            .iter()
                            .any(|ability| ability.state_rule == ManaStateRule::RemoveCounter),
                    ),
                ));
                payment_kind = merge_payment_kind(payment_kind, sources.last().expect("source"));
                used.push(false);
                if mode.is_legendary {
                    legendary_permanents.insert(card_index);
                    legendary_colors.extend(mode.colors.iter().copied());
                }
            } else {
                sources.push(make_simple_trial_source(
                    &card.color_identity,
                    turn + u8::from(card.features.enters_tapped),
                    None,
                ));
                payment_kind = merge_payment_kind(payment_kind, sources.last().expect("source"));
                used.push(false);
                if card.features.is_legendary {
                    legendary_permanents.insert(card_index);
                    legendary_colors.extend(card.colors.iter().copied());
                }
            }
        } else {
            let floating_ramp = ramp.max(0.0);
            let has_nonland = hand
                .iter()
                .any(|index| !pool.cards[*index].features.is_land);
            let has_payable_spell = hand.iter().any(|index| {
                let features = &pool.cards[*index].features;
                if features.is_land {
                    return false;
                }
                features
                    .initial_modes
                    .iter()
                    .filter(|mode| !mode.is_land)
                    .any(|mode| {
                        can_pay_mode(
                            mode,
                            features.mana_value,
                            sources,
                            used,
                            floating_ramp,
                            turn,
                            payment_kind,
                            context,
                            &legendary_colors,
                        )
                    })
                    || (features.initial_modes.is_empty()
                        && available_mana(sources, used, floating_ramp, turn) + 0.01
                            >= features.mana_value)
            });
            if has_nonland && !has_payable_spell {
                missed_land_turns += 1;
            }
        }

        let mut floating_ramp = ramp.max(0.0);
        loop {
            let best = hand
                .iter()
                .enumerate()
                .filter_map(|(position, index)| {
                    let features = &pool.cards[*index].features;
                    if features.is_land {
                        return None;
                    }
                    if features.is_legendary && legendary_permanents.contains(index) {
                        return None;
                    }
                    let payable = features
                        .initial_modes
                        .iter()
                        .enumerate()
                        .filter(|(_, mode)| !mode.is_land)
                        .find(|(_, mode)| {
                            can_pay_mode(
                                mode,
                                features.mana_value,
                                sources,
                                used,
                                floating_ramp,
                                turn,
                                payment_kind,
                                context,
                                &legendary_colors,
                            )
                        })
                        .map(|(mode, _)| mode)
                        .or_else(|| {
                            (features.initial_modes.is_empty()
                                && available_mana(sources, used, floating_ramp, turn) + 0.01
                                    >= features.mana_value)
                                .then_some(usize::MAX)
                        });
                    payable.map(|mode| {
                        (
                            position,
                            mode,
                            features.pressure
                                + features.card_advantage
                                + features.ramp
                                + features.interaction
                                + features.resilience,
                        )
                    })
                })
                .max_by(|left, right| left.2.total_cmp(&right.2));

            let Some((position, mode_index, _)) = best else {
                if hand
                    .iter()
                    .any(|index| !pool.cards[*index].features.is_land)
                {
                    stranded_turns += 1;
                }
                break;
            };
            let card_index = hand.remove(position);
            let features = &pool.cards[card_index].features;
            let mut cast_creature = pool.cards[card_index].type_line.contains("Creature");
            if mode_index == usize::MAX {
                // Legacy records without modes can only spend virtual generic ramp.
                floating_ramp = (floating_ramp - features.mana_value).max(0.0);
            } else {
                let mode = features
                    .initial_modes
                    .iter()
                    .filter(|mode| !mode.is_land)
                    .nth(mode_index)
                    .expect("selected mode");
                cast_creature = mode.type_line.contains("Creature");
                pay_mode(
                    mode,
                    features.mana_value,
                    sources,
                    used,
                    &mut floating_ramp,
                    turn,
                    payment_kind,
                    context,
                    &legendary_colors,
                );
                if mode.is_legendary && mode_is_permanent(mode) {
                    legendary_permanents.insert(card_index);
                    legendary_colors.extend(mode.colors.iter().copied());
                }
            }
            ramp += features.ramp * 0.35;
            for mode in &features.initial_modes {
                if !mode.is_land && !mode.late_mana_source_colors.is_empty() {
                    let abilities = if mode.mana_sources.is_empty() {
                        mode.late_mana_source_colors
                            .iter()
                            .map(|color| ManaSourceProfile {
                                colors: vec![*color],
                                earliest_turn: turn.saturating_add(1),
                                ..ManaSourceProfile::default()
                            })
                            .collect()
                    } else {
                        mode.mana_sources.clone()
                    };
                    sources.push(make_trial_source(
                        abilities,
                        turn.saturating_add(1),
                        None,
                        0,
                    ));
                    payment_kind =
                        merge_payment_kind(payment_kind, sources.last().expect("source"));
                    used.push(false);
                    break;
                }
            }
            let turn_weight = 1.0 + (turn_horizon - turn) as f64 * 0.10;
            cast_value += (features.pressure as f64 * 0.75
                + features.card_advantage as f64 * 0.55
                + features.ramp as f64 * 0.45)
                * turn_weight;
            interaction += features.interaction as f64;
            resilience += features.resilience as f64 + features.card_advantage as f64 * 0.25;
            if cast_creature {
                battlefield_power += f64::from(features.pressure * 4.0).max(1.0);
            }
            for ability in features
                .initial_modes
                .iter()
                .filter(|mode| !mode.is_land)
                .flat_map(|mode| mode.abilities.iter())
            {
                let immediate = ability.timing == crate::AbilityTiming::Spell
                    || ability.label.contains("enter")
                    || ability.label.contains("Enter");
                if !immediate {
                    continue;
                }
                match ability.kind {
                    AbilityKind::CreateToken => {
                        battlefield_power += f64::from(ability.effect_value).max(0.0)
                    }
                    AbilityKind::Damage => {
                        projected_damage += f64::from(ability.effect_value).max(0.0)
                    }
                    _ => {}
                }
            }
            for tag in &features.tags {
                if is_synergy_tag(tag) {
                    if let Some((_, cards)) =
                        seen_tags.iter_mut().find(|(seen, _)| *seen == tag.as_str())
                    {
                        if !cards.contains(&card_index) {
                            cards.push(card_index);
                        }
                    } else {
                        seen_tags.push((tag.as_str(), vec![card_index]));
                    }
                }
            }
            if let Some(search) = features
                .mechanics
                .search_action
                .as_ref()
                .filter(|search| !search.from_hand)
                && let Some(target_position) = (cursor..library.len()).find(|position| {
                    search_target_matches(&search.target, &pool.cards[library[*position]])
                })
            {
                let target = library.remove(target_position);
                hand.push(target);
            }
        }
    }

    let tag_pairs: usize = seen_tags
        .iter()
        .map(|(_, cards)| cards.len().saturating_sub(1))
        .sum();
    let land_penalty = missed_land_turns as f64 / turn_horizon.max(1) as f64;
    let stranded_penalty = stranded_turns as f64 / turn_horizon.max(1) as f64;

    TrialScore {
        consistency: (1.0 - mulligan_penalty - land_penalty * 0.55).clamp(0.0, 1.0),
        tempo: (cast_value / 18.0).clamp(0.0, 1.0),
        synergy: (tag_pairs as f64 / 8.0).clamp(0.0, 1.0),
        interaction: (interaction / 5.0).clamp(0.0, 1.0),
        resilience: (resilience / 5.0).clamp(0.0, 1.0),
        closing: (projected_damage / 20.0).clamp(0.0, 1.0),
        mana_failure: (land_penalty * 0.65 + stranded_penalty * 0.35).clamp(0.0, 1.0),
        legendary_dead_observations,
        nonland_hand_observations,
    }
}

fn is_synergy_tag(tag: &str) -> bool {
    tag.starts_with("type:")
        || matches!(
            tag,
            "token"
                | "copy"
                | "recursion"
                | "sacrifice"
                | "graveyard"
                | "discard"
                | "lifegain"
                | "counters"
                | "death"
                | "mill"
                | "blink"
                | "landfall"
                | "spellslinger"
                | "treasure"
                | "clue"
                | "food"
                | "energy"
                | "equipment"
                | "aura"
        )
}

#[allow(clippy::too_many_arguments)]
fn find_payable_search(
    hand: &[usize],
    library: &[usize],
    cursor: usize,
    pool: &CardPool,
    sources: &[TrialManaSource],
    used: &[bool],
    turn: u8,
    context: &StructuralEvaluationContext,
    legendary_colors: &HashSet<Color>,
) -> Option<(usize, usize)> {
    hand.iter().enumerate().find_map(|(hand_position, index)| {
        let search = pool.cards[*index]
            .features
            .mechanics
            .search_action
            .as_ref()?;
        if !search.from_hand {
            return None;
        }
        let target_position = (cursor..library.len()).find(|position| {
            search_target_matches(&search.target, &pool.cards[library[*position]])
        })?;
        let mut trial_sources = sources.to_vec();
        let mut trial_used = used.to_vec();
        pay_activation(
            &search.mana_cost,
            &mut trial_sources,
            &mut trial_used,
            turn,
            context,
            legendary_colors,
        )
        .then_some((hand_position, target_position))
    })
}

fn pay_activation(
    cost: &crate::ManaCostProfile,
    sources: &mut [TrialManaSource],
    used: &mut [bool],
    turn: u8,
    context: &StructuralEvaluationContext,
    legendary_colors: &HashSet<Color>,
) -> bool {
    let original_sources = sources.to_vec();
    let original_used = used.to_vec();
    let consume =
        |options: Option<&[usize]>, sources: &mut [TrialManaSource], used: &mut [bool]| {
            for index in 0..sources.len() {
                if used[index] || sources[index].available_turn > turn {
                    continue;
                }
                if consume_unrestricted_source(
                    &mut sources[index],
                    options,
                    context,
                    legendary_colors,
                    turn,
                ) {
                    used[index] = true;
                    return true;
                }
            }
            false
        };
    for (slot, pips) in cost.colored.iter().enumerate() {
        for _ in 0..*pips {
            if !consume(Some(&[slot]), sources, used) {
                sources.clone_from_slice(&original_sources);
                used.copy_from_slice(&original_used);
                return false;
            }
        }
    }
    for hybrid in &cost.hybrid {
        let slots = hybrid
            .iter()
            .map(|color| mana_slot(*color))
            .collect::<Vec<_>>();
        if !consume(Some(&slots), sources, used) {
            sources.clone_from_slice(&original_sources);
            used.copy_from_slice(&original_used);
            return false;
        }
    }
    for _ in 0..cost.generic {
        if !consume(None, sources, used) {
            sources.clone_from_slice(&original_sources);
            used.copy_from_slice(&original_used);
            return false;
        }
    }
    true
}

fn consume_unrestricted_source(
    source: &mut TrialManaSource,
    color_options: Option<&[usize]>,
    context: &StructuralEvaluationContext,
    legendary_colors: &HashSet<Color>,
    turn: u8,
) -> bool {
    if color_options.is_none_or(|options| {
        options
            .iter()
            .any(|slot| source.unrestricted_fixed_colors[*slot])
    }) && source
        .unrestricted_fixed_colors
        .into_iter()
        .any(|available| available)
    {
        return true;
    }
    for ability in &source.abilities {
        if ability.spend_rule != ManaSpendRule::Unrestricted
            || ability.earliest_turn > turn
            || ability.activation_generic > 0
            || (ability.state_rule == ManaStateRule::RemoveCounter && source.charge_counters == 0)
        {
            continue;
        }
        let colors = resolved_source_colors(ability, context, legendary_colors);
        if color_options.is_some_and(|options| !options.iter().any(|slot| colors[*slot])) {
            continue;
        }
        if ability.state_rule == ManaStateRule::RemoveCounter {
            source.charge_counters -= 1;
        }
        return true;
    }
    false
}

fn mode_is_permanent(mode: &CardMode) -> bool {
    let kind = mode.type_line.to_ascii_lowercase();
    [
        "creature",
        "artifact",
        "enchantment",
        "planeswalker",
        "battle",
    ]
    .iter()
    .any(|value| kind.contains(value))
}

fn available_mana(sources: &[TrialManaSource], used: &[bool], ramp: f32, turn: u8) -> f32 {
    sources
        .iter()
        .enumerate()
        .filter(|(index, source)| source.available_turn <= turn && !used[*index])
        .count() as f32
        + ramp
}

#[allow(clippy::too_many_arguments)]
fn can_pay_mode(
    mode: &crate::CardMode,
    mana_value: f32,
    sources: &[TrialManaSource],
    used: &[bool],
    ramp: f32,
    turn: u8,
    payment_kind: SourcePaymentKind,
    context: &StructuralEvaluationContext,
    legendary_colors: &HashSet<Color>,
) -> bool {
    if mode.normal_castable == Some(false) {
        return false;
    }
    const STACK_SOURCE_LIMIT: usize = 32;
    let mut stack_used = [false; STACK_SOURCE_LIMIT];
    let mut heap_used = Vec::new();
    let trial_used: &mut [bool] = if used.len() <= STACK_SOURCE_LIMIT {
        stack_used[..used.len()].copy_from_slice(used);
        &mut stack_used[..used.len()]
    } else {
        heap_used.extend_from_slice(used);
        heap_used.as_mut_slice()
    };
    match payment_kind {
        SourcePaymentKind::Simple => {
            let mut trial_ramp = ramp;
            return try_pay_mode_simple(
                mode,
                mana_value,
                sources,
                trial_used,
                &mut trial_ramp,
                turn,
            );
        }
        SourcePaymentKind::Stateless => {
            let mut trial_ramp = ramp;
            return try_pay_mode_stateless(
                mode,
                mana_value,
                sources,
                trial_used,
                &mut trial_ramp,
                turn,
                context,
                legendary_colors,
            );
        }
        SourcePaymentKind::Stateful => {}
    }
    let mut trial_sources = sources.to_vec();
    let mut trial_ramp = ramp;
    pay_mode(
        mode,
        mana_value,
        &mut trial_sources,
        trial_used,
        &mut trial_ramp,
        turn,
        payment_kind,
        context,
        legendary_colors,
    )
}

#[allow(clippy::too_many_arguments)]
fn pay_generic_for_mode(
    amount: f32,
    mode: &CardMode,
    sources: &mut [TrialManaSource],
    used: &mut [bool],
    ramp: &mut f32,
    turn: u8,
    context: &StructuralEvaluationContext,
    legendary_colors: &HashSet<Color>,
) -> bool {
    let from_ramp = ramp.min(amount);
    *ramp -= from_ramp;
    let mut remaining = (amount - from_ramp).ceil() as usize;
    for (index, used_source) in used.iter_mut().enumerate().take(sources.len()) {
        if remaining == 0 {
            break;
        }
        if !*used_source
            && sources[index].available_turn <= turn
            && consume_source_for_mode(
                &mut sources[index],
                mode,
                None,
                context,
                legendary_colors,
                turn,
            )
        {
            *used_source = true;
            remaining -= 1;
        }
    }
    remaining == 0
}

#[allow(clippy::too_many_arguments)]
fn pay_mode(
    mode: &crate::CardMode,
    mana_value: f32,
    sources: &mut [TrialManaSource],
    used: &mut [bool],
    ramp: &mut f32,
    turn: u8,
    payment_kind: SourcePaymentKind,
    context: &StructuralEvaluationContext,
    legendary_colors: &HashSet<Color>,
) -> bool {
    if mode.normal_castable == Some(false) {
        return false;
    }
    match payment_kind {
        SourcePaymentKind::Simple => {
            return pay_mode_simple(mode, mana_value, sources, used, ramp, turn);
        }
        SourcePaymentKind::Stateless => {
            return pay_mode_stateless(
                mode,
                mana_value,
                sources,
                used,
                ramp,
                turn,
                context,
                legendary_colors,
            );
        }
        SourcePaymentKind::Stateful => {}
    }
    let original_sources = sources.to_vec();
    let original_used = used.to_vec();
    let original_ramp = *ramp;
    let consume = |options: &[usize], sources: &mut [TrialManaSource], used: &mut [bool]| -> bool {
        for index in 0..sources.len() {
            if !used[index]
                && sources[index].available_turn <= turn
                && consume_source_for_mode(
                    &mut sources[index],
                    mode,
                    Some(options),
                    context,
                    legendary_colors,
                    turn,
                )
            {
                used[index] = true;
                return true;
            }
        }
        false
    };
    for (slot, pips) in mode.mana_cost.colored.iter().enumerate() {
        for _ in 0..*pips {
            if !consume(&[slot], sources, used) {
                sources.clone_from_slice(&original_sources);
                used.copy_from_slice(&original_used);
                *ramp = original_ramp;
                return false;
            }
        }
    }
    for hybrid in &mode.mana_cost.hybrid {
        let options = hybrid
            .iter()
            .map(|color| mana_slot(*color))
            .collect::<Vec<_>>();
        if !consume(&options, sources, used) {
            sources.clone_from_slice(&original_sources);
            used.copy_from_slice(&original_used);
            *ramp = original_ramp;
            return false;
        }
    }
    let colored = mode
        .mana_cost
        .colored
        .iter()
        .map(|pips| f32::from(*pips))
        .sum::<f32>()
        + mode.mana_cost.hybrid.len() as f32;
    let generic = f32::from(mode.mana_cost.generic).max((mana_value - colored).max(0.0));
    if !pay_generic_for_mode(
        generic,
        mode,
        sources,
        used,
        ramp,
        turn,
        context,
        legendary_colors,
    ) {
        sources.clone_from_slice(&original_sources);
        used.copy_from_slice(&original_used);
        *ramp = original_ramp;
        return false;
    }
    true
}

fn merge_payment_kind(current: SourcePaymentKind, source: &TrialManaSource) -> SourcePaymentKind {
    if current == SourcePaymentKind::Stateful || source.stateful {
        SourcePaymentKind::Stateful
    } else if current == SourcePaymentKind::Stateless || !source.simple_unrestricted {
        SourcePaymentKind::Stateless
    } else {
        SourcePaymentKind::Simple
    }
}

fn make_trial_source_from_mode(
    mode: &CardMode,
    available_turn: u8,
    chosen_creature_type: Option<String>,
    charge_counters: u8,
) -> TrialManaSource {
    if mode.mana_sources.is_empty() {
        return make_simple_trial_source(
            &mode.mana_source_colors,
            available_turn,
            chosen_creature_type,
        );
    }
    let all_simple = mode.mana_sources.iter().all(|ability| {
        ability.color_rule == ManaColorRule::Fixed
            && ability.spend_rule == ManaSpendRule::Unrestricted
            && ability.state_rule == ManaStateRule::None
            && ability.activation_generic == 0
    });
    if all_simple {
        let colors = mode
            .mana_sources
            .iter()
            .flat_map(|ability| ability.colors.iter().copied())
            .collect::<Vec<_>>();
        return make_simple_trial_source(&colors, available_turn, chosen_creature_type);
    }
    make_trial_source(
        mode.mana_sources.clone(),
        available_turn,
        chosen_creature_type,
        charge_counters,
    )
}

fn make_simple_trial_source(
    colors: &[Color],
    available_turn: u8,
    chosen_creature_type: Option<String>,
) -> TrialManaSource {
    let mut fixed = [false; 6];
    for color in colors {
        fixed[mana_slot(*color)] = true;
    }
    let produces_mana = fixed.into_iter().any(|available| available);
    TrialManaSource {
        abilities: Vec::new(),
        simple_colors: fixed,
        unrestricted_fixed_colors: fixed,
        simple_unrestricted: produces_mana,
        stateful: false,
        available_turn,
        chosen_creature_type,
        charge_counters: 0,
    }
}

fn make_trial_source(
    abilities: Vec<ManaSourceProfile>,
    available_turn: u8,
    chosen_creature_type: Option<String>,
    charge_counters: u8,
) -> TrialManaSource {
    let simple_unrestricted = !abilities.is_empty()
        && abilities.iter().all(|ability| {
            ability.color_rule == ManaColorRule::Fixed
                && ability.spend_rule == ManaSpendRule::Unrestricted
                && ability.state_rule == ManaStateRule::None
                && ability.activation_generic == 0
        });
    let stateful = abilities
        .iter()
        .any(|ability| ability.state_rule != ManaStateRule::None);
    let mut simple_colors = [false; 6];
    let mut unrestricted_fixed_colors = [false; 6];
    for ability in &abilities {
        if ability.color_rule == ManaColorRule::Fixed
            && ability.spend_rule == ManaSpendRule::Unrestricted
            && ability.state_rule == ManaStateRule::None
            && ability.activation_generic == 0
        {
            for color in &ability.colors {
                unrestricted_fixed_colors[mana_slot(*color)] = true;
            }
        }
    }
    if simple_unrestricted {
        for color in abilities.iter().flat_map(|ability| &ability.colors) {
            simple_colors[mana_slot(*color)] = true;
        }
    }
    TrialManaSource {
        abilities,
        simple_colors,
        unrestricted_fixed_colors,
        simple_unrestricted,
        stateful,
        available_turn,
        chosen_creature_type,
        charge_counters,
    }
}

fn pay_mode_simple(
    mode: &CardMode,
    mana_value: f32,
    sources: &[TrialManaSource],
    used: &mut [bool],
    ramp: &mut f32,
    turn: u8,
) -> bool {
    const STACK_SOURCE_LIMIT: usize = 32;
    let mut stack_used = [false; STACK_SOURCE_LIMIT];
    let mut heap_used = Vec::new();
    let original_used: &[bool] = if used.len() <= STACK_SOURCE_LIMIT {
        stack_used[..used.len()].copy_from_slice(used);
        &stack_used[..used.len()]
    } else {
        heap_used.extend_from_slice(used);
        heap_used.as_slice()
    };
    let original_ramp = *ramp;
    let paid = try_pay_mode_simple(mode, mana_value, sources, used, ramp, turn);
    if !paid {
        used.copy_from_slice(original_used);
        *ramp = original_ramp;
    }
    paid
}

fn try_pay_mode_simple(
    mode: &CardMode,
    mana_value: f32,
    sources: &[TrialManaSource],
    used: &mut [bool],
    ramp: &mut f32,
    turn: u8,
) -> bool {
    let consume = |options: &[usize], used: &mut [bool]| {
        if let Some(index) = sources.iter().enumerate().position(|(index, source)| {
            !used[index]
                && source.available_turn <= turn
                && options.iter().any(|slot| source.simple_colors[*slot])
        }) {
            used[index] = true;
            true
        } else {
            false
        }
    };
    for (slot, pips) in mode.mana_cost.colored.iter().enumerate() {
        for _ in 0..*pips {
            if !consume(&[slot], used) {
                return false;
            }
        }
    }
    for hybrid in &mode.mana_cost.hybrid {
        let slots = hybrid
            .iter()
            .map(|color| mana_slot(*color))
            .collect::<Vec<_>>();
        if !consume(&slots, used) {
            return false;
        }
    }
    let colored = mode
        .mana_cost
        .colored
        .iter()
        .map(|pips| f32::from(*pips))
        .sum::<f32>()
        + mode.mana_cost.hybrid.len() as f32;
    let mut generic = f32::from(mode.mana_cost.generic).max((mana_value - colored).max(0.0));
    let from_ramp = ramp.min(generic);
    *ramp -= from_ramp;
    generic -= from_ramp;
    let mut remaining = generic.ceil() as usize;
    for (index, used_source) in used.iter_mut().enumerate().take(sources.len()) {
        if remaining == 0 {
            break;
        }
        if !*used_source && sources[index].available_turn <= turn {
            *used_source = true;
            remaining -= 1;
        }
    }
    if remaining > 0 {
        return false;
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn pay_mode_stateless(
    mode: &CardMode,
    mana_value: f32,
    sources: &[TrialManaSource],
    used: &mut [bool],
    ramp: &mut f32,
    turn: u8,
    context: &StructuralEvaluationContext,
    legendary_colors: &HashSet<Color>,
) -> bool {
    const STACK_SOURCE_LIMIT: usize = 32;
    let mut stack_used = [false; STACK_SOURCE_LIMIT];
    let mut heap_used = Vec::new();
    let original_used: &[bool] = if used.len() <= STACK_SOURCE_LIMIT {
        stack_used[..used.len()].copy_from_slice(used);
        &stack_used[..used.len()]
    } else {
        heap_used.extend_from_slice(used);
        heap_used.as_slice()
    };
    let original_ramp = *ramp;
    let paid = try_pay_mode_stateless(
        mode,
        mana_value,
        sources,
        used,
        ramp,
        turn,
        context,
        legendary_colors,
    );
    if !paid {
        used.copy_from_slice(original_used);
        *ramp = original_ramp;
    }
    paid
}

#[allow(clippy::too_many_arguments)]
fn try_pay_mode_stateless(
    mode: &CardMode,
    mana_value: f32,
    sources: &[TrialManaSource],
    used: &mut [bool],
    ramp: &mut f32,
    turn: u8,
    context: &StructuralEvaluationContext,
    legendary_colors: &HashSet<Color>,
) -> bool {
    let consume = |options: Option<&[usize]>, used: &mut [bool]| {
        if let Some(index) = sources.iter().enumerate().position(|(index, source)| {
            !used[index]
                && source.available_turn <= turn
                && stateless_source_can_pay(source, mode, options, context, legendary_colors, turn)
        }) {
            used[index] = true;
            true
        } else {
            false
        }
    };
    for (slot, pips) in mode.mana_cost.colored.iter().enumerate() {
        for _ in 0..*pips {
            if !consume(Some(&[slot]), used) {
                return false;
            }
        }
    }
    for hybrid in &mode.mana_cost.hybrid {
        let slots = hybrid
            .iter()
            .map(|color| mana_slot(*color))
            .collect::<Vec<_>>();
        if !consume(Some(&slots), used) {
            return false;
        }
    }
    let colored = mode
        .mana_cost
        .colored
        .iter()
        .map(|pips| f32::from(*pips))
        .sum::<f32>()
        + mode.mana_cost.hybrid.len() as f32;
    let mut generic = f32::from(mode.mana_cost.generic).max((mana_value - colored).max(0.0));
    let from_ramp = ramp.min(generic);
    *ramp -= from_ramp;
    generic -= from_ramp;
    for _ in 0..generic.ceil() as usize {
        if !consume(None, used) {
            return false;
        }
    }
    true
}

fn stateless_source_can_pay(
    source: &TrialManaSource,
    mode: &CardMode,
    color_options: Option<&[usize]>,
    context: &StructuralEvaluationContext,
    legendary_colors: &HashSet<Color>,
    turn: u8,
) -> bool {
    if let Some(options) = color_options {
        if options
            .iter()
            .any(|slot| source.unrestricted_fixed_colors[*slot])
        {
            return true;
        }
    } else if source
        .unrestricted_fixed_colors
        .into_iter()
        .any(|available| available)
    {
        return true;
    }
    source.abilities.iter().any(|ability| {
        if ability.color_rule == ManaColorRule::Fixed
            && ability.spend_rule == ManaSpendRule::Unrestricted
            && ability.state_rule == ManaStateRule::None
            && ability.activation_generic == 0
        {
            return false;
        }
        if ability.earliest_turn > turn
            || ability.activation_generic > 0
            || !spend_rule_allows(
                &ability.spend_rule,
                mode,
                source.chosen_creature_type.as_deref(),
            )
        {
            return false;
        }
        let colors = resolved_source_colors(ability, context, legendary_colors);
        if let Some(options) = color_options {
            options.iter().any(|slot| colors[*slot])
        } else {
            colors.into_iter().any(|available| available)
        }
    })
}

fn most_needed_creature_type(hand: &[usize], pool: &CardPool) -> Option<String> {
    let mut counts = HashMap::<String, usize>::new();
    for index in hand {
        for mode in &pool.cards[*index].features.initial_modes {
            if !mode.is_land {
                for kind in &mode.creature_types {
                    *counts.entry(kind.to_ascii_lowercase()).or_default() += 1;
                }
            }
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(kind, _)| kind)
}

fn consume_source_for_mode(
    source: &mut TrialManaSource,
    mode: &CardMode,
    color_options: Option<&[usize]>,
    context: &StructuralEvaluationContext,
    legendary_colors: &HashSet<Color>,
    turn: u8,
) -> bool {
    if color_options.is_none_or(|options| {
        options
            .iter()
            .any(|slot| source.unrestricted_fixed_colors[*slot])
    }) && source
        .unrestricted_fixed_colors
        .into_iter()
        .any(|available| available)
    {
        return true;
    }
    for ability in &source.abilities {
        if ability.earliest_turn > turn
            || ability.activation_generic > 0
            || !spend_rule_allows(
                &ability.spend_rule,
                mode,
                source.chosen_creature_type.as_deref(),
            )
            || (ability.state_rule == ManaStateRule::RemoveCounter && source.charge_counters == 0)
        {
            continue;
        }
        let colors = resolved_source_colors(ability, context, legendary_colors);
        if color_options.is_some_and(|options| !options.iter().any(|slot| colors[*slot])) {
            continue;
        }
        if ability.state_rule == ManaStateRule::RemoveCounter {
            source.charge_counters -= 1;
        }
        return true;
    }
    false
}

fn sideboard_quality(sideboard: &[(usize, u8)], pool: &CardPool) -> f64 {
    let copies = sideboard
        .iter()
        .map(|(_, quantity)| *quantity as usize)
        .sum::<usize>();
    if copies == 0 {
        return 0.0;
    }

    let mut tags = HashSet::new();
    let mut utility = 0.0_f64;
    for (index, quantity) in sideboard {
        let features = &pool.cards[*index].features;
        for tag in &features.tags {
            if is_package_tag(tag) {
                tags.insert(tag.as_str());
            }
        }
        let efficiency = (features.interaction as f64 * 0.45
            + features.resilience as f64 * 0.25
            + features.card_advantage as f64 * 0.20
            + features.pressure as f64 * 0.10)
            / (1.0 + features.mana_value as f64 * 0.12);
        utility += efficiency * *quantity as f64;
    }

    let coverage = (tags.len() as f64 / 8.0).min(1.0);
    let utility = (utility / copies as f64 / 1.5).clamp(0.0, 1.0);
    let diversity = (sideboard.len() as f64 / copies as f64).clamp(0.0, 1.0);
    (coverage * 0.35 + utility * 0.45 + diversity * 0.20).clamp(0.0, 1.0)
}

#[derive(Clone)]
struct Candidate {
    genome: Vec<u8>,
    active_cards: Vec<(usize, u8)>,
    sideboard_genome: Vec<u8>,
    active_sideboard: Vec<(usize, u8)>,
    commanders: Vec<usize>,
    fitness: Fitness,
}

struct EvaluationGroup {
    representative: usize,
    members: Vec<usize>,
}

#[derive(Default)]
struct MutationGuide {
    package_groups: Vec<Vec<usize>>,
    /// Compact typed-action/card-role neighborhoods. These are deterministic
    /// embeddings used for coherent one-card substitutions; tournament,
    /// validated-Hall, and combo packages add cross-role co-occurrence edges.
    embedding_neighbors: Vec<Vec<usize>>,
}

fn combo_roles_for_card(card: &crate::Card) -> HashSet<&'static str> {
    let mut roles = HashSet::new();
    for rule in card
        .features
        .initial_modes
        .iter()
        .flat_map(|mode| mode.rule_abilities.iter())
    {
        for effect in &rule.effects {
            match effect {
                crate::RuleEffect::Draw { .. } => {
                    roles.insert("draw");
                }
                crate::RuleEffect::MoveCard {
                    from: crate::AbilityZone::Graveyard,
                    ..
                } => {
                    roles.insert("recursion");
                }
                crate::RuleEffect::Search { .. } => {
                    roles.insert("tutor");
                }
                crate::RuleEffect::Copy { .. } => {
                    roles.insert("copy");
                }
                crate::RuleEffect::Replace { .. } => {
                    roles.insert("replacement");
                }
                crate::RuleEffect::AddMana { .. } => {
                    roles.insert("mana");
                }
                crate::RuleEffect::CreateToken { .. } => {
                    roles.insert("token");
                }
                crate::RuleEffect::PutCounters(_) => {
                    roles.insert("counter");
                }
                crate::RuleEffect::Damage { .. } => {
                    roles.insert("damage");
                }
                crate::RuleEffect::Mill { .. } => {
                    roles.insert("mill");
                }
                crate::RuleEffect::Sacrifice { .. } => {
                    roles.insert("sacrifice");
                }
                crate::RuleEffect::Life { .. } => {
                    roles.insert("life");
                }
                crate::RuleEffect::GrantKeyword { keyword }
                    if matches!(
                        keyword.as_str(),
                        "hexproof" | "indestructible" | "protection"
                    ) =>
                {
                    roles.insert("protection");
                }
                crate::RuleEffect::CostChange {
                    increase: false, ..
                } => {
                    roles.insert("cost_reduction");
                }
                crate::RuleEffect::Tap { untap: true } => {
                    roles.insert("untap");
                }
                crate::RuleEffect::WinLoss { wins: true, .. } => {
                    roles.insert("alternate_win");
                }
                _ => {}
            }
        }
        if rule.effects.iter().any(|effect| {
            matches!(
                effect,
                crate::RuleEffect::MoveCard {
                    from: crate::AbilityZone::Battlefield,
                    to: crate::AbilityZone::Exile,
                    ..
                }
            )
        }) && rule.effects.iter().any(|effect| {
            matches!(
                effect,
                crate::RuleEffect::MoveCard {
                    from: crate::AbilityZone::Exile,
                    to: crate::AbilityZone::Battlefield,
                    ..
                }
            )
        }) {
            roles.insert("blink");
        }
    }
    for ability in card
        .features
        .initial_modes
        .iter()
        .flat_map(|mode| mode.abilities.iter())
    {
        let label = ability.label.to_ascii_lowercase();
        if label.contains("untap") {
            roles.insert("untap");
        }
        if label.contains("cost") && label.contains(" less") {
            roles.insert("cost_reduction");
        }
        if label.contains("hexproof")
            || label.contains("indestructible")
            || label.contains("protection")
        {
            roles.insert("protection");
        }
        if label.contains("gain") && label.contains("life") {
            roles.insert("life");
        }
        match ability.kind {
            AbilityKind::Draw => {
                roles.insert("draw");
            }
            AbilityKind::Recursion => {
                roles.insert("recursion");
            }
            AbilityKind::Search => {
                roles.insert("tutor");
            }
            AbilityKind::Copy => {
                roles.insert("copy");
            }
            AbilityKind::TokenMultiplier => {
                roles.insert("replacement");
            }
            AbilityKind::Ramp => {
                roles.insert("mana");
            }
            AbilityKind::CreateToken => {
                roles.insert("token");
            }
            AbilityKind::AddCounters => {
                roles.insert("counter");
            }
            AbilityKind::Damage => {
                roles.insert("damage");
            }
            AbilityKind::Mill => {
                roles.insert("mill");
            }
            AbilityKind::SacrificeOutlet => {
                roles.insert("sacrifice");
            }
            AbilityKind::Blink => {
                roles.insert("blink");
            }
            AbilityKind::AlternateWin => {
                roles.insert("alternate_win");
            }
            _ => {}
        }
    }
    roles
}

/// Adds precomputed causal combo-package evidence without invoking the bounded
/// solver in the evolutionary hot loop. Assembly is an opening-hand/turn-
/// horizon approximation used for structural prioritization only; actual game
/// execution remains mandatory for promotion.
pub fn apply_combo_package_evidence(
    fitness: &mut Fitness,
    deck: &[(usize, u8)],
    pool: &CardPool,
    packages: &[ComboPackageProfile],
    turn_horizon: u8,
) {
    if packages.is_empty() {
        return;
    }
    let deck_size = deck
        .iter()
        .map(|(_, count)| usize::from(*count))
        .sum::<usize>()
        .max(1);
    let draws = (7 + usize::from(turn_horizon)).min(deck_size) as f64;
    let by_id = pool.index_by_id();
    let quantities = deck
        .iter()
        .filter(|(_, count)| *count > 0)
        .map(|(index, count)| (&pool.cards[*index].id, usize::from(*count)))
        .collect::<HashMap<_, _>>();
    let nonland_copies = deck
        .iter()
        .filter(|(index, _)| !pool.cards[*index].features.is_land)
        .map(|(_, count)| usize::from(*count))
        .sum::<usize>()
        .max(1);
    let mut best_execution = 0.0_f64;
    let mut best_assembly = 0.0_f64;
    let mut best_turn = None;
    let mut best_dead = 0.0_f64;
    let mut best_resilience = 0.0_f64;
    let mut best_proof = String::new();
    let mut best_winning = false;
    for package in packages {
        if package.required_roles.len() < 2 {
            continue;
        }
        let mut requirements = HashMap::<&str, usize>::new();
        for role in &package.required_roles {
            *requirements.entry(role.as_str()).or_default() += 1;
        }
        let mut assembly = 1.0_f64;
        for (role, needed) in requirements {
            let matching = deck
                .iter()
                .filter(|(index, count)| {
                    *count > 0 && combo_roles_for_card(&pool.cards[*index]).contains(role)
                })
                .collect::<Vec<_>>();
            let distinct = matching.len();
            let copies = matching
                .iter()
                .map(|(_, count)| usize::from(*count))
                .sum::<usize>();
            if distinct < needed || copies == 0 {
                assembly = 0.0;
                break;
            }
            let seen = 1.0 - (1.0 - copies as f64 / deck_size as f64).powf(draws);
            assembly *= seen
                .clamp(0.0, 1.0)
                .powi(needed.min(i32::MAX as usize) as i32);
        }
        let tutor_copies = package
            .tutor_ids
            .iter()
            .map(|id| quantities.get(id).copied().unwrap_or(0))
            .sum::<usize>()
            + deck
                .iter()
                .filter(|(index, count)| {
                    *count > 0 && combo_roles_for_card(&pool.cards[*index]).contains("tutor")
                })
                .map(|(_, count)| usize::from(*count))
                .sum::<usize>();
        let tutor_boost = (tutor_copies as f64 / 12.0).clamp(0.0, 0.35);
        assembly = (assembly + (1.0 - assembly) * tutor_boost).clamp(0.0, 1.0);
        let protection = package
            .protection_ids
            .iter()
            .map(|id| quantities.get(id).copied().unwrap_or(0))
            .sum::<usize>();
        let recovery = package
            .recovery_ids
            .iter()
            .map(|id| quantities.get(id).copied().unwrap_or(0))
            .sum::<usize>();
        let resilience = ((protection + recovery) as f64 / 8.0).clamp(0.0, 1.0);
        let expected_turn = package
            .expected_turn
            .map(f64::from)
            .unwrap_or_else(|| (package.required_roles.len() as f64 / 1.5).ceil().max(1.0));
        let pace = (f64::from(turn_horizon) / expected_turn.max(1.0)).clamp(0.0, 1.0);
        let execution = assembly * pace * (0.75 + resilience * 0.25);
        let dead_copies = package
            .piece_ids
            .iter()
            .filter_map(|id| by_id.get(id).copied().map(|index| (id, index)))
            .filter(|(_, index)| {
                let features = &pool.cards[*index].features;
                features.card_advantage
                    + features.interaction
                    + features.pressure
                    + features.resilience
                    + features.ramp
                    < 0.5
            })
            .map(|(id, _)| quantities.get(id).copied().unwrap_or(0))
            .sum::<usize>() as f64
            / nonland_copies as f64;
        if execution > best_execution {
            best_execution = execution;
            best_assembly = assembly;
            best_turn = Some(expected_turn);
            best_dead = dead_copies;
            best_resilience = resilience;
            best_proof = package.proof.clone();
            best_winning = package.winning;
        }
    }
    fitness.combo_assembly_probability = best_assembly;
    fitness.combo_expected_turn = best_turn;
    fitness.combo_dead_card_rate = best_dead;
    fitness.combo_resilience = best_resilience;
    fitness.combo_execution_rate = best_execution;
    fitness.combo_proof_level = best_proof;
    if best_execution > 0.0 {
        fitness.synergy = fitness.synergy.max(best_assembly);
        fitness.consistency = fitness.consistency.max(best_assembly * 0.85);
        fitness.resilience = fitness.resilience.max(best_resilience);
        if best_winning {
            fitness.closing_power = fitness.closing_power.max(best_execution);
        }
    }
}

fn typed_strategy_keys(card: &crate::Card) -> HashSet<&'static str> {
    let mut keys = HashSet::new();
    let rules = card
        .features
        .initial_modes
        .iter()
        .flat_map(|mode| mode.rule_abilities.iter())
        .collect::<Vec<_>>();
    let has_effect = |predicate: fn(&crate::RuleEffect) -> bool| {
        rules.iter().any(|rule| rule.effects.iter().any(predicate))
    };
    if has_effect(|effect| matches!(effect, crate::RuleEffect::Copy { .. })) {
        keys.insert("copy");
    }
    if has_effect(|effect| matches!(effect, crate::RuleEffect::CreateToken { .. }))
        || has_effect(
            |effect| matches!(effect, crate::RuleEffect::Replace { event, .. } if event.contains("token")),
        )
    {
        keys.insert("tokens");
    }
    let rules_blink = rules.iter().any(|rule| {
        rule.effects.iter().any(|effect| {
            matches!(
                effect,
                crate::RuleEffect::MoveCard {
                    from: crate::AbilityZone::Battlefield,
                    to: crate::AbilityZone::Exile,
                    ..
                }
            )
        }) && rule.effects.iter().any(|effect| {
            matches!(
                effect,
                crate::RuleEffect::MoveCard {
                    from: crate::AbilityZone::Exile,
                    to: crate::AbilityZone::Battlefield,
                    ..
                }
            )
        })
    });
    if rules_blink {
        keys.insert("blink-etb");
    }
    if has_effect(|effect| matches!(effect, crate::RuleEffect::Sacrifice { .. })) {
        keys.insert("sacrifice");
    }
    if has_effect(|effect| {
        matches!(
            effect,
            crate::RuleEffect::MoveCard {
                from: crate::AbilityZone::Graveyard,
                ..
            } | crate::RuleEffect::Mill { .. }
        )
    }) {
        keys.insert("graveyard");
    }
    if has_effect(|effect| matches!(effect, crate::RuleEffect::PutCounters(_))) {
        keys.insert("counters");
    }
    if has_effect(|effect| {
        matches!(
            effect,
            crate::RuleEffect::AddMana { .. } | crate::RuleEffect::Search { .. }
        )
    }) {
        keys.insert("ramp");
    }
    let useful_triggered_permanent = card
        .features
        .initial_modes
        .iter()
        .filter(|mode| !mode.is_land)
        .flat_map(|mode| mode.abilities.iter())
        .any(|ability| {
            ability.timing == crate::AbilityTiming::Triggered
                && matches!(
                    ability.kind,
                    AbilityKind::Draw
                        | AbilityKind::Destroy
                        | AbilityKind::Exile
                        | AbilityKind::Bounce
                        | AbilityKind::CreateToken
                        | AbilityKind::AddCounters
                        | AbilityKind::Ramp
                        | AbilityKind::Recursion
                )
        });
    for ability in card
        .features
        .initial_modes
        .iter()
        .flat_map(|mode| mode.abilities.iter())
    {
        match ability.kind {
            AbilityKind::Copy => {
                keys.insert("copy");
            }
            AbilityKind::CreateToken | AbilityKind::TokenMultiplier => {
                keys.insert("tokens");
            }
            AbilityKind::Blink => {
                keys.insert("blink-etb");
            }
            AbilityKind::SacrificeOutlet => {
                keys.insert("sacrifice");
            }
            AbilityKind::Recursion | AbilityKind::GraveyardHate => {
                keys.insert("graveyard");
            }
            AbilityKind::Mill => {
                keys.insert("graveyard");
            }
            AbilityKind::AddCounters => {
                keys.insert("counters");
            }
            AbilityKind::Ramp | AbilityKind::Search => {
                keys.insert("ramp");
            }
            _ => {}
        }
        if ability.timing == crate::AbilityTiming::Triggered
            && card.type_line.contains("Creature")
            && matches!(
                ability.kind,
                AbilityKind::Draw
                    | AbilityKind::Destroy
                    | AbilityKind::Exile
                    | AbilityKind::Bounce
                    | AbilityKind::CreateToken
                    | AbilityKind::Ramp
                    | AbilityKind::Recursion
            )
        {
            keys.insert("blink-etb");
        }
        if ability.timing == crate::AbilityTiming::Triggered
            && ability
                .conditions
                .iter()
                .any(|condition| condition.contains("cast") && condition.contains("spell"))
        {
            keys.insert("spellslinger");
        }
    }
    if useful_triggered_permanent {
        // Copy and blink effects are enablers; useful triggered permanents are
        // their target/payoff side of the causal package.
        keys.insert("copy");
        keys.insert("blink-etb");
    }
    if card.type_line.contains("Instant") || card.type_line.contains("Sorcery") {
        keys.insert("spellslinger");
    }
    keys
}

fn typed_strategy_roles(card: &crate::Card, package: &str) -> HashSet<&'static str> {
    let abilities = card
        .features
        .initial_modes
        .iter()
        .flat_map(|mode| mode.abilities.iter())
        .collect::<Vec<_>>();
    let mut roles = HashSet::new();
    let rules = card
        .features
        .initial_modes
        .iter()
        .flat_map(|mode| mode.rule_abilities.iter())
        .collect::<Vec<_>>();
    let rule_has = |predicate: fn(&crate::RuleEffect) -> bool| {
        rules.iter().any(|rule| rule.effects.iter().any(predicate))
    };
    match package {
        "copy" => {
            if rule_has(|effect| matches!(effect, crate::RuleEffect::Copy { .. }))
                || abilities
                    .iter()
                    .any(|ability| ability.kind == AbilityKind::Copy)
            {
                roles.insert("enabler");
            }
            if abilities.iter().any(|ability| {
                ability.timing == crate::AbilityTiming::Triggered
                    && matches!(
                        ability.kind,
                        AbilityKind::Draw
                            | AbilityKind::Destroy
                            | AbilityKind::Exile
                            | AbilityKind::Bounce
                            | AbilityKind::CreateToken
                            | AbilityKind::AddCounters
                            | AbilityKind::Ramp
                            | AbilityKind::Recursion
                    )
            }) {
                roles.insert("target-payoff");
            }
        }
        "tokens" => {
            if rule_has(|effect| matches!(effect, crate::RuleEffect::CreateToken { .. }))
                || abilities
                    .iter()
                    .any(|ability| ability.kind == AbilityKind::CreateToken)
            {
                roles.insert("producer");
            }
            if rule_has(
                |effect| matches!(effect, crate::RuleEffect::Replace { event, .. } if event.contains("token")),
            ) || abilities
                .iter()
                .any(|ability| ability.kind == AbilityKind::TokenMultiplier)
            {
                roles.insert("payoff");
            }
        }
        "blink-etb" => {
            let rules_blink = rules.iter().any(|rule| {
                rule.effects.iter().any(|effect| {
                    matches!(
                        effect,
                        crate::RuleEffect::MoveCard {
                            from: crate::AbilityZone::Battlefield,
                            to: crate::AbilityZone::Exile,
                            ..
                        }
                    )
                }) && rule.effects.iter().any(|effect| {
                    matches!(
                        effect,
                        crate::RuleEffect::MoveCard {
                            from: crate::AbilityZone::Exile,
                            to: crate::AbilityZone::Battlefield,
                            ..
                        }
                    )
                })
            });
            if rules_blink
                || abilities
                    .iter()
                    .any(|ability| ability.kind == AbilityKind::Blink)
            {
                roles.insert("enabler");
            }
            if abilities
                .iter()
                .any(|ability| ability.timing == crate::AbilityTiming::Triggered)
            {
                roles.insert("target-payoff");
            }
        }
        "sacrifice" => {
            if rule_has(|effect| matches!(effect, crate::RuleEffect::Sacrifice { .. }))
                || abilities
                    .iter()
                    .any(|ability| ability.kind == AbilityKind::SacrificeOutlet)
            {
                roles.insert("outlet");
            }
            if rule_has(|effect| matches!(effect, crate::RuleEffect::CreateToken { .. }))
                || abilities
                    .iter()
                    .any(|ability| ability.kind == AbilityKind::CreateToken)
                || card.features.mechanics.sacrifice_fodder_supply > 0
            {
                roles.insert("fodder");
            }
            if card.features.mechanics.requires_death {
                roles.insert("payoff");
            }
        }
        "graveyard" => {
            if rule_has(|effect| {
                matches!(
                    effect,
                    crate::RuleEffect::MoveCard {
                        from: crate::AbilityZone::Graveyard,
                        ..
                    }
                )
            }) || abilities
                .iter()
                .any(|ability| ability.kind == AbilityKind::Recursion)
            {
                roles.insert("payoff");
            }
            if rule_has(|effect| matches!(effect, crate::RuleEffect::Mill { .. }))
                || abilities
                    .iter()
                    .any(|ability| ability.kind == AbilityKind::Mill)
                || card.features.mechanics.graveyard_setup_supply > 0
            {
                roles.insert("setup");
            }
        }
        "spellslinger" => {
            if card.type_line.contains("Instant") || card.type_line.contains("Sorcery") {
                roles.insert("spell");
            }
            if abilities.iter().any(|ability| {
                ability.timing == crate::AbilityTiming::Triggered
                    && ability
                        .conditions
                        .iter()
                        .any(|condition| condition.contains("cast") && condition.contains("spell"))
            }) {
                roles.insert("payoff");
            }
        }
        "counters" => {
            if rule_has(|effect| matches!(effect, crate::RuleEffect::PutCounters(_)))
                || abilities
                    .iter()
                    .any(|ability| ability.kind == AbilityKind::AddCounters)
            {
                roles.insert("producer");
            }
            if card
                .oracle_text
                .to_ascii_lowercase()
                .contains("one or more counters")
            {
                roles.insert("payoff");
            }
        }
        _ => {}
    }
    roles
}

fn strategy_keys(card: &crate::Card) -> HashSet<&'static str> {
    let mut keys = typed_strategy_keys(card);
    for tag in &card.features.tags {
        match tag.as_str() {
            "copy" => {
                keys.insert("copy");
            }
            "token" => {
                keys.insert("tokens");
            }
            "blink" => {
                keys.insert("blink-etb");
            }
            "sacrifice" | "death" => {
                keys.insert("sacrifice");
            }
            "graveyard" | "recursion" | "mill" => {
                keys.insert("graveyard");
            }
            "spellslinger" => {
                keys.insert("spellslinger");
            }
            "counters" => {
                keys.insert("counters");
            }
            "artifact" => {
                keys.insert("artifacts");
            }
            "ramp" => {
                keys.insert("ramp");
            }
            _ => {}
        }
    }
    keys
}

fn fallback_strategy_keys(card: &crate::Card) -> Vec<String> {
    let mut keys = Vec::new();
    let typed = typed_strategy_keys(card);
    if card.features.pressure > 0.0 || typed.iter().any(|key| matches!(*key, "copy" | "tokens")) {
        keys.push("role:threat".into());
    }
    if card.features.initial_modes.iter().any(|mode| {
        mode.abilities.iter().any(|ability| {
            matches!(
                ability.kind,
                AbilityKind::Destroy
                    | AbilityKind::Exile
                    | AbilityKind::Bounce
                    | AbilityKind::CounterSpell
            ) && !ability.narrow
        })
    }) {
        keys.push("role:interaction".into());
    }
    if card.features.initial_modes.iter().any(|mode| {
        mode.abilities
            .iter()
            .any(|ability| matches!(ability.kind, AbilityKind::Draw | AbilityKind::Recursion))
    }) {
        keys.push("role:card-flow".into());
    }
    let curve = if card.features.mana_value <= 2.0 {
        "curve:early"
    } else if card.features.mana_value <= 4.0 {
        "curve:middle"
    } else {
        "curve:late"
    };
    keys.push(curve.into());
    for color in &card.colors {
        keys.push(format!("color:{color:?}"));
    }
    keys.sort();
    keys.dedup();
    keys
}

fn derive_strategy_signature(root: &[(usize, u8)], pool: &CardPool) -> StrategySignature {
    let mut support =
        HashMap::<String, (f64, HashSet<usize>, HashSet<usize>, HashSet<&'static str>)>::new();
    let mut root_nonland_copies = 0_usize;
    for (index, count) in root.iter().copied().filter(|(_, count)| *count > 0) {
        let card = &pool.cards[index];
        if card.features.is_land {
            continue;
        }
        root_nonland_copies += usize::from(count);
        for key in strategy_keys(card) {
            let entry = support.entry(key.into()).or_default();
            entry.0 += f64::from(count);
            entry.1.insert(index);
            if typed_strategy_keys(card).contains(key) {
                entry.2.insert(index);
                entry.3.extend(typed_strategy_roles(card, key));
            }
        }
    }
    let mut packages = support
        .into_iter()
        .filter(|(_, (_, cards, _, _))| cards.len() >= 2)
        .map(|(package, (copies, cards, typed_cards, causal_roles))| {
            let primary = typed_cards.len() >= 2
                && causal_roles.len() >= 2
                && matches!(
                    package.as_str(),
                    "copy"
                        | "tokens"
                        | "blink-etb"
                        | "sacrifice"
                        | "graveyard"
                        | "spellslinger"
                        | "counters"
                );
            let mut required_roles = if primary {
                causal_roles
                    .into_iter()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            required_roles.sort();
            StrategyPackageDefinition {
                primary,
                package,
                // Primary causal packages are retained by their distinct
                // engine/enabler/payoff roles. Copy count alone allowed a deck
                // to keep many token producers while deleting its only token
                // multiplier and still report high package retention.
                root_support: copies,
                root_unique_cards: cards.len(),
                required_roles,
            }
        })
        .collect::<Vec<_>>();
    if !packages.iter().any(|package| package.primary) {
        let mut fallback = HashMap::<String, (f64, HashSet<usize>)>::new();
        for (index, count) in root.iter().copied().filter(|(_, count)| *count > 0) {
            let card = &pool.cards[index];
            if card.features.is_land {
                continue;
            }
            for key in fallback_strategy_keys(card) {
                let entry = fallback.entry(key).or_default();
                entry.0 += f64::from(count);
                entry.1.insert(index);
            }
        }
        packages.extend(fallback.into_iter().map(|(package, (copies, cards))| {
            StrategyPackageDefinition {
                package,
                primary: false,
                root_support: copies,
                root_unique_cards: cards.len(),
                required_roles: Vec::new(),
            }
        }));
    }
    packages.sort_by(|left, right| {
        right
            .primary
            .cmp(&left.primary)
            .then_with(|| right.root_support.total_cmp(&left.root_support))
            .then_with(|| left.package.cmp(&right.package))
    });
    StrategySignature {
        version: 2,
        packages,
        root_nonland_copies,
    }
}

fn augment_combo_signature(
    signature: &mut StrategySignature,
    root: &[(usize, u8)],
    pool: &CardPool,
    packages: &[ComboPackageProfile],
) {
    let root_ids = root
        .iter()
        .filter(|(_, count)| *count > 0)
        .map(|(index, _)| &pool.cards[*index].id)
        .collect::<HashSet<_>>();
    for package in packages {
        let required_roles = package
            .required_roles
            .iter()
            .map(|role| role.to_ascii_lowercase())
            .collect::<Vec<_>>();
        if required_roles.len() < 2
            || package
                .piece_ids
                .iter()
                .filter(|id| root_ids.contains(id))
                .count()
                < 2
        {
            continue;
        }
        let name = format!("combo:{}", package.id);
        if signature
            .packages
            .iter()
            .any(|existing| existing.package == name)
        {
            continue;
        }
        signature.packages.push(StrategyPackageDefinition {
            package: name,
            primary: true,
            root_support: required_roles.len() as f64,
            root_unique_cards: package.piece_ids.len(),
            required_roles,
        });
    }
    signature.packages.sort_by(|left, right| {
        right
            .primary
            .cmp(&left.primary)
            .then_with(|| right.root_support.total_cmp(&left.root_support))
            .then_with(|| left.package.cmp(&right.package))
    });
}

fn package_support(
    deck: &[(usize, u8)],
    pool: &CardPool,
    package: &StrategyPackageDefinition,
) -> f64 {
    if !package.required_roles.is_empty() {
        let mut available = HashMap::<String, usize>::new();
        for (index, count) in deck.iter().copied().filter(|(_, count)| *count > 0) {
            if pool.cards[index].features.is_land {
                continue;
            }
            let roles = if package.package.starts_with("combo:") {
                combo_roles_for_card(&pool.cards[index])
            } else {
                typed_strategy_roles(&pool.cards[index], &package.package)
            };
            for role in roles {
                *available.entry(role.into()).or_default() += usize::from(count).min(1);
            }
        }
        let mut consumed = HashMap::<String, usize>::new();
        let fulfilled = package
            .required_roles
            .iter()
            .filter(|role| {
                let used = consumed.entry((*role).clone()).or_default();
                if *used < *available.get(*role).unwrap_or(&0) {
                    *used += 1;
                    true
                } else {
                    false
                }
            })
            .count() as f64;
        let role_fraction = fulfilled / package.required_roles.len().max(1) as f64;
        return role_fraction * package.root_support;
    }
    deck.iter()
        .copied()
        .filter(|(_, count)| *count > 0)
        .filter(|(index, _)| {
            strategy_keys(&pool.cards[*index]).contains(package.package.as_str())
                || fallback_strategy_keys(&pool.cards[*index])
                    .iter()
                    .any(|key| key == &package.package)
        })
        .map(|(_, count)| f64::from(count))
        .sum()
}

fn nonland_edit_fraction(candidate: &[(usize, u8)], root: &[(usize, u8)], pool: &CardPool) -> f64 {
    let name_counts = |deck: &[(usize, u8)]| {
        let mut counts = HashMap::<String, usize>::new();
        for (index, count) in deck.iter().copied().filter(|(_, count)| *count > 0) {
            let card = &pool.cards[index];
            if !card.features.is_land {
                *counts.entry(card.name.to_ascii_lowercase()).or_default() += usize::from(count);
            }
        }
        counts
    };
    let root_counts = name_counts(root);
    let candidate_counts = name_counts(candidate);
    let root_total = root_counts.values().sum::<usize>();
    if root_total == 0 {
        return 0.0;
    }
    let removed = root_counts
        .iter()
        .map(|(name, count)| count.saturating_sub(*candidate_counts.get(name).unwrap_or(&0)))
        .sum::<usize>();
    removed as f64 / root_total as f64
}

fn apply_tune_policy(
    fitness: &mut Fitness,
    candidate: &[(usize, u8)],
    root: &[(usize, u8)],
    signature: &StrategySignature,
    policy: &TunePolicy,
    pool: &CardPool,
    baseline: bool,
) {
    let mut weighted_retention = 0.0;
    let mut total_weight = 0.0;
    let mut primary_retention = 0.0;
    let mut primary_weight = 0.0;
    let mut secondary_retention = 0.0;
    let mut secondary_weight = 0.0;
    let mut primary_failure = false;
    fitness.strategy_packages.clear();
    for package in &signature.packages {
        let candidate_support = package_support(candidate, pool, package);
        let retention = (candidate_support / package.root_support.max(1.0)).clamp(0.0, 1.0);
        let weight = package.root_support * if package.primary { 1.5 } else { 0.75 };
        weighted_retention += retention * weight;
        total_weight += weight;
        if package.primary {
            primary_retention += retention * package.root_support;
            primary_weight += package.root_support;
        } else {
            secondary_retention += retention * package.root_support;
            secondary_weight += package.root_support;
        }
        if package.primary && retention < policy.primary_package_floor {
            primary_failure = true;
            fitness.structural_blocking_reasons.push(format!(
                "Primary {} package retained {:.0}%; Tune requires {:.0}%",
                package.package,
                retention * 100.0,
                policy.primary_package_floor * 100.0
            ));
        }
        fitness.strategy_packages.push(StrategyPackageEvidence {
            package: package.package.clone(),
            primary: package.primary,
            root_support: package.root_support,
            candidate_support,
            retention,
        });
    }
    fitness.strategy_similarity = if primary_weight > 0.0 {
        let primary = primary_retention / primary_weight;
        let secondary = if secondary_weight > 0.0 {
            secondary_retention / secondary_weight
        } else {
            primary
        };
        // Secondary curve/color/support similarities must not drown out the
        // causal engine packages that define an imported deck's identity.
        (primary * 0.80 + secondary * 0.20).clamp(0.0, 1.0)
    } else if total_weight > 0.0 {
        (weighted_retention / total_weight).clamp(0.0, 1.0)
    } else {
        1.0
    };
    fitness.nonland_edit_fraction = nonland_edit_fraction(candidate, root, pool);
    if !baseline {
        if fitness.strategy_similarity < policy.strategy_retention_floor {
            fitness.structural_blocking_reasons.push(format!(
                "Strategy retention {:.0}%; Tune requires {:.0}%",
                fitness.strategy_similarity * 100.0,
                policy.strategy_retention_floor * 100.0
            ));
        }
        if fitness.nonland_edit_fraction > policy.maximum_nonland_edit_fraction {
            fitness.structural_blocking_reasons.push(format!(
                "Changed {:.0}% of root nonland copies; Tune allows {:.0}%",
                fitness.nonland_edit_fraction * 100.0,
                policy.maximum_nonland_edit_fraction * 100.0
            ));
        }
        fitness.severe_structural_failure |= primary_failure
            || fitness.strategy_similarity < policy.strategy_retention_floor
            || fitness.nonland_edit_fraction > policy.maximum_nonland_edit_fraction;
    }
}

#[derive(Clone, Copy)]
struct MutationSettings {
    swaps: usize,
    package_rate: f32,
    package_size: usize,
    mana_rate: f32,
}

impl MutationGuide {
    fn new(
        pool: &CardPool,
        learned_packages: &[Vec<crate::CardId>],
        combo_packages: &[ComboPackageProfile],
    ) -> Self {
        let mut by_tag = BTreeMap::<&str, Vec<usize>>::new();
        for (index, card) in pool.cards.iter().enumerate() {
            if card.features.is_land {
                continue;
            }
            for tag in &card.features.tags {
                if is_package_tag(tag) {
                    let members = by_tag.entry(tag).or_default();
                    if members.last() != Some(&index) {
                        members.push(index);
                    }
                }
            }
            for function in strategy_keys(card) {
                let members = by_tag.entry(function).or_default();
                if members.last() != Some(&index) {
                    members.push(index);
                }
            }
        }
        let mut package_groups = by_tag
            .into_values()
            .filter(|members| members.len() >= 2)
            .collect::<Vec<_>>();
        let by_id = pool.index_by_id();
        package_groups.extend(learned_packages.iter().filter_map(|package| {
            let mut members = package
                .iter()
                .filter_map(|id| by_id.get(id).copied())
                .filter(|index| !pool.cards[*index].features.is_land)
                .collect::<Vec<_>>();
            members.sort_unstable();
            members.dedup();
            (members.len() >= 2).then_some(members)
        }));
        for package in combo_packages {
            let required = package
                .required_roles
                .iter()
                .map(|role| role.as_str())
                .collect::<HashSet<_>>();
            let mut members = pool
                .cards
                .iter()
                .enumerate()
                .filter(|(_, card)| !card.features.is_land)
                .filter(|(_, card)| {
                    combo_roles_for_card(card)
                        .iter()
                        .any(|role| required.contains(role))
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            members.extend(
                package
                    .piece_ids
                    .iter()
                    .filter_map(|id| by_id.get(id).copied()),
            );
            members.sort_unstable();
            members.dedup();
            if members.len() >= 2 {
                package_groups.push(members);
            }
        }
        package_groups.sort();
        package_groups.dedup();
        let mut embedding_neighbors = vec![Vec::<usize>::new(); pool.cards.len()];
        let mut embedding_buckets = BTreeMap::<u64, Vec<usize>>::new();
        for (index, card) in pool.cards.iter().enumerate() {
            embedding_buckets
                .entry(functional_embedding_key(card))
                .or_default()
                .push(index);
        }
        for members in embedding_buckets.values() {
            // Exact typed/card-role buckets are already high-similarity. A
            // bounded deterministic ring avoids quadratic startup cost in
            // buckets such as basic lands and generic creatures.
            for (position, index) in members.iter().copied().enumerate() {
                let neighbors = &mut embedding_neighbors[index];
                for offset in 1..=24.min(members.len().saturating_sub(1)) {
                    neighbors.push(members[(position + offset) % members.len()]);
                }
            }
        }
        for group in &package_groups {
            for index in group {
                let neighbors = &mut embedding_neighbors[*index];
                neighbors.extend(group.iter().copied().filter(|candidate| candidate != index));
            }
        }
        for neighbors in &mut embedding_neighbors {
            neighbors.sort_unstable();
            neighbors.dedup();
            neighbors.truncate(64);
        }
        Self {
            package_groups,
            embedding_neighbors,
        }
    }
}

fn functional_embedding_key(card: &crate::Card) -> u64 {
    let mut colors = 0_u64;
    for color in &card.colors {
        colors |= 1_u64 << mana_slot(*color);
    }
    if card.colors.is_empty() {
        colors |= 1 << 5;
    }
    let mana_band = card.features.mana_value.ceil().clamp(0.0, 7.0) as u64;
    let mut actions = 0_u64;
    for ability in card
        .features
        .initial_modes
        .iter()
        .flat_map(|mode| mode.abilities.iter())
    {
        let bit = match ability.kind {
            AbilityKind::Draw => 0,
            AbilityKind::Destroy | AbilityKind::Exile | AbilityKind::Bounce => 1,
            AbilityKind::CounterSpell => 2,
            AbilityKind::Recursion | AbilityKind::GraveyardHate => 3,
            AbilityKind::Search => 4,
            AbilityKind::CreateToken => 5,
            AbilityKind::Copy => 6,
            AbilityKind::TokenMultiplier => 7,
            AbilityKind::SacrificeOutlet => 8,
            AbilityKind::Ramp => 9,
            AbilityKind::Damage => 10,
            AbilityKind::Mill | AbilityKind::AlternateWin => 11,
            AbilityKind::Blink => 12,
            AbilityKind::AddCounters | AbilityKind::Crew | AbilityKind::Unknown => 13,
        };
        actions |= 1_u64 << bit;
    }
    if actions == 0 {
        actions = if card.features.interaction >= 0.5 {
            1 << 1
        } else if card.features.card_advantage >= 0.5 {
            1
        } else if card.features.ramp >= 0.5 {
            1 << 9
        } else {
            1 << 14
        };
    }
    let card_type = if card.features.is_land {
        0_u64
    } else if card.type_line.contains("Creature") {
        1
    } else if card.type_line.contains("Instant") || card.type_line.contains("Sorcery") {
        2
    } else if card.type_line.contains("Artifact") {
        3
    } else if card.type_line.contains("Enchantment") {
        4
    } else {
        5
    };
    colors | (mana_band << 6) | (card_type << 10) | (actions << 13)
}

pub struct EvolutionEngine<E = HeuristicEvaluator> {
    evaluator: E,
}

impl Default for EvolutionEngine<HeuristicEvaluator> {
    fn default() -> Self {
        Self {
            evaluator: HeuristicEvaluator,
        }
    }
}

impl<E: Evaluator> EvolutionEngine<E> {
    pub fn new(evaluator: E) -> Self {
        Self { evaluator }
    }

    pub fn run<F>(
        &self,
        seed_deck: &Deck,
        pool: &CardPool,
        constraints: &DeckConstraints,
        config: &EvolutionConfig,
        cancellation: &CancellationToken,
        on_progress: F,
    ) -> Result<RunResult, EvolutionError>
    where
        F: FnMut(GenerationProgress),
    {
        let context = StructuralEvaluationContext {
            commander: !seed_deck.command_zone.is_empty(),
            commander_colors: seed_deck
                .command_zone
                .iter()
                .filter_map(|entry| pool.card(&entry.card_id))
                .flat_map(|card| card.color_identity.iter().copied())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect(),
            ..StructuralEvaluationContext::default()
        };
        self.run_with_context(
            seed_deck,
            pool,
            constraints,
            config,
            cancellation,
            &context,
            on_progress,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run_with_context<F>(
        &self,
        seed_deck: &Deck,
        pool: &CardPool,
        constraints: &DeckConstraints,
        config: &EvolutionConfig,
        cancellation: &CancellationToken,
        context: &StructuralEvaluationContext,
        mut on_progress: F,
    ) -> Result<RunResult, EvolutionError>
    where
        F: FnMut(GenerationProgress),
    {
        validate_inputs(seed_deck, pool, constraints, config)?;
        let started = Instant::now();
        let base_bounds = bounds(pool, constraints)?;
        let base_copy_bounds = copy_bounds(pool, constraints);
        let side_constraints = sideboard_constraints(constraints);
        let mutation_guide =
            MutationGuide::new(pool, &config.learned_packages, &config.combo_packages);
        let mut rng = SplitMix64::new(config.seed);
        let fixed_commanders = command_zone_to_indices(seed_deck, pool)?;
        let commander_roots = if config.evolve_commanders {
            commander_candidates(pool)
                .into_iter()
                .filter(|index| {
                    constraints
                        .allowed_colors
                        .as_ref()
                        .is_none_or(|allowed| pool.cards[*index].matches_color_selection(allowed))
                })
                .collect()
        } else {
            Vec::new()
        };
        if config.evolve_commanders
            && constraints.minimum_commanders > 0
            && commander_roots.is_empty()
        {
            return Err(EvolutionError::UnsatisfiableConstraint(
                "no eligible commanders in the selected card pool".into(),
            ));
        }
        let first_commanders = if config.evolve_commanders && fixed_commanders.is_empty() {
            random_commander_selection(pool, &commander_roots, constraints, &mut rng)?
        } else {
            fixed_commanders.clone()
        };
        let first_main_template =
            commander_bounds(pool, constraints, &base_bounds, &first_commanders)?;
        let first_copy_bounds =
            commander_bounds(pool, constraints, &base_copy_bounds, &first_commanders)?;
        let imported_sideboard = sideboard_to_genome(seed_deck, pool)?;
        let blank_mainboard = seed_deck.mainboard_count() == 0;
        let tune_root_deck = (config.evolution_mode == EvolutionMode::Tune && !blank_mainboard)
            .then(|| config.tune_policy.root_deck.as_ref().unwrap_or(seed_deck));
        let tune_root_genome = tune_root_deck
            .map(|deck| deck_to_genome(deck, pool))
            .transpose()?;
        let tune_root_active = tune_root_genome
            .as_ref()
            .map(|genome| compact_genome(genome));
        let strategy_signature = tune_root_active.as_ref().map(|root| {
            let mut signature = derive_strategy_signature(root, pool);
            augment_combo_signature(&mut signature, root, pool, &config.combo_packages);
            signature
        });
        let mut seed_sideboard = if config.evolve_sideboard
            && (seed_deck.sideboard_count() == 0 || first_commanders != fixed_commanders)
        {
            vec![0; pool.cards.len()]
        } else {
            imported_sideboard
        };
        let first_main_bounds = bounds_after_other_zone(&first_main_template, &seed_sideboard)?;
        let first_commander_focus = commander_focus_tags(&first_commanders, pool);
        let mut seed_genome =
            if seed_deck.mainboard_count() == 0 || first_commanders != fixed_commanders {
                generate_seed_genome(
                    pool,
                    constraints,
                    &first_main_bounds,
                    &first_commander_focus,
                    &mut rng,
                )?
            } else {
                deck_to_genome(seed_deck, pool)?
            };
        repair_mana_base(
            &mut seed_genome,
            &first_main_bounds,
            pool,
            constraints,
            &mut rng,
        );
        if config.evolve_sideboard
            && (seed_deck.sideboard_count() == 0 || first_commanders != fixed_commanders)
            && constraints.target_sideboard_size > 0
        {
            let first_side_bounds = bounds_after_other_zone(&first_copy_bounds, &seed_genome)?;
            seed_sideboard =
                generate_seed_genome(pool, &side_constraints, &first_side_bounds, &[], &mut rng)?;
        }
        let worker_count = config.worker_threads.unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|threads| threads.get().saturating_sub(2).max(1))
                .unwrap_or(1)
        });
        let worker_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(worker_count)
            .build()
            .map_err(|error| EvolutionError::WorkerPool(error.to_string()))?;

        let mut population = Vec::with_capacity(config.population_size);
        population.push(Candidate {
            active_cards: compact_genome(&seed_genome),
            genome: seed_genome.clone(),
            active_sideboard: compact_genome(&seed_sideboard),
            sideboard_genome: seed_sideboard.clone(),
            commanders: first_commanders.clone(),
            fitness: Fitness::default(),
        });
        while population.len() < config.population_size {
            let commanders = if config.evolve_commanders {
                random_commander_selection(pool, &commander_roots, constraints, &mut rng)?
            } else {
                first_commanders.clone()
            };
            let main_template = commander_bounds(pool, constraints, &base_bounds, &commanders)?;
            let total_bounds = commander_bounds(pool, constraints, &base_copy_bounds, &commanders)?;
            let (mut genome, mut sideboard_genome) = if commanders == first_commanders
                && !blank_mainboard
            {
                (seed_genome.clone(), seed_sideboard.clone())
            } else {
                let empty_sideboard = vec![0; pool.cards.len()];
                let main_bounds = bounds_after_other_zone(&main_template, &empty_sideboard)?;
                let focus = commander_focus_tags(&commanders, pool);
                let genome =
                    generate_seed_genome(pool, constraints, &main_bounds, &focus, &mut rng)?;
                let sideboard_genome =
                    if config.evolve_sideboard && constraints.target_sideboard_size > 0 {
                        let side_bounds = bounds_after_other_zone(&total_bounds, &genome)?;
                        generate_seed_genome(pool, &side_constraints, &side_bounds, &[], &mut rng)?
                    } else {
                        seed_sideboard.clone()
                    };
                (genome, sideboard_genome)
            };
            let candidate_bounds = bounds_after_other_zone(&main_template, &sideboard_genome)?;
            mutate(
                &mut genome,
                &candidate_bounds,
                pool,
                constraints,
                &mutation_guide,
                MutationSettings {
                    swaps: config.mutation_swaps.max(1) * 2,
                    package_rate: config.package_mutation_rate,
                    package_size: config.package_size,
                    mana_rate: config.mana_mutation_rate,
                },
                &mut rng,
            );
            repair_mana_base(&mut genome, &candidate_bounds, pool, constraints, &mut rng);
            if config.evolve_sideboard && constraints.target_sideboard_size > 0 {
                let side_bounds = bounds_after_other_zone(&total_bounds, &genome)?;
                mutate(
                    &mut sideboard_genome,
                    &side_bounds,
                    pool,
                    &side_constraints,
                    &mutation_guide,
                    MutationSettings {
                        swaps: config.mutation_swaps.max(1) * 2,
                        package_rate: config.package_mutation_rate,
                        package_size: config.package_size,
                        mana_rate: config.mana_mutation_rate,
                    },
                    &mut rng,
                );
            }
            population.push(Candidate {
                active_cards: compact_genome(&genome),
                genome,
                active_sideboard: compact_genome(&sideboard_genome),
                sideboard_genome,
                commanders,
                fitness: Fitness::default(),
            });
        }

        let mut evaluated_candidates = 0_u64;
        let mut unique_evaluations = 0_u64;
        let mut completed = 0_usize;
        let mut map_archive = HashMap::<MapEliteDescriptor, Candidate>::new();
        let mut compact_context = context.clone();
        compact_context.compact_evaluation = true;

        for generation in 0..config.generations {
            if cancellation.is_cancelled() {
                break;
            }
            let seeds: Vec<u64> = (0..config.trials_per_candidate)
                .map(|trial| generation_seed(config.seed, generation, trial))
                .collect();
            let evaluation_groups = group_identical_candidates(&population);
            let unique_fitness = worker_pool.install(|| {
                evaluation_groups
                    .par_iter()
                    .map(|group| {
                        let candidate = &population[group.representative];
                        let mut fitness = self.evaluator.evaluate(
                            &candidate.active_cards,
                            &candidate.commanders,
                            pool,
                            &seeds,
                            config.turn_horizon,
                            &compact_context,
                        );
                        fitness.sideboard_quality =
                            sideboard_quality(&candidate.active_sideboard, pool);
                        fitness.strategy_similarity = 1.0;
                        apply_combo_package_evidence(
                            &mut fitness,
                            &candidate.active_cards,
                            pool,
                            &config.combo_packages,
                            config.turn_horizon,
                        );
                        if let (Some(root), Some(signature)) =
                            (tune_root_active.as_ref(), strategy_signature.as_ref())
                        {
                            apply_tune_policy(
                                &mut fitness,
                                &candidate.active_cards,
                                root,
                                signature,
                                &config.tune_policy,
                                pool,
                                false,
                            );
                        }
                        fitness.score = weighted_fitness_score(&fitness, &config.fitness_weights);
                        fitness
                    })
                    .collect::<Vec<_>>()
            });
            for (group, fitness) in evaluation_groups.iter().zip(unique_fitness) {
                for member in &group.members {
                    population[*member].fitness = fitness.clone();
                }
            }
            evaluated_candidates += population.len() as u64;
            unique_evaluations += evaluation_groups.len() as u64;
            population.sort_by(|left, right| fitness_order(&right.fitness, &left.fitness));
            update_map_elites(&mut map_archive, &population, pool, 128);
            completed = generation + 1;

            let elapsed = started.elapsed();
            // Progress updates only need the current feasible leader. Building
            // the quality-diverse visible frontier performs repeated copy-vector,
            // package, and descriptor comparisons; doing that for every
            // generation used more time than structural evaluation itself on a
            // 250-card pool. The complete frontier is still built and hydrated
            // once after the final generation.
            let ranked = population
                .iter()
                .find(|candidate| {
                    !candidate.fitness.severe_mana_failure
                        && !candidate.fitness.severe_structural_failure
                        && tune_root_genome
                            .as_ref()
                            .is_none_or(|root| candidate.genome != *root)
                })
                .map(|candidate| vec![candidate_as_ranked(candidate, pool, 1, "provisional")])
                .unwrap_or_default();
            if ranked.is_empty() && tune_root_genome.is_none() {
                let reasons = population
                    .first()
                    .map(|candidate| candidate.fitness.mana_blocking_reasons.join("; "))
                    .unwrap_or_else(|| "no feasible candidates".into());
                return Err(EvolutionError::UnsatisfiableConstraint(format!(
                    "all candidates failed severe mana feasibility: {reasons}"
                )));
            }
            let baseline = tune_root_deck
                .zip(tune_root_active.as_ref())
                .zip(strategy_signature.as_ref())
                .map(|((deck, root), signature)| {
                    let mut fitness = self.evaluator.evaluate(
                        root,
                        &first_commanders,
                        pool,
                        &seeds,
                        config.turn_horizon,
                        if completed == config.generations {
                            context
                        } else {
                            &compact_context
                        },
                    );
                    fitness.sideboard_quality = sideboard_quality(
                        &sideboard_to_genome(deck, pool)
                            .map(|genome| compact_genome(&genome))
                            .unwrap_or_default(),
                        pool,
                    );
                    apply_combo_package_evidence(
                        &mut fitness,
                        root,
                        pool,
                        &config.combo_packages,
                        config.turn_horizon,
                    );
                    apply_tune_policy(
                        &mut fitness,
                        root,
                        root,
                        signature,
                        &config.tune_policy,
                        pool,
                        true,
                    );
                    fitness.score = weighted_fitness_score(&fitness, &config.fitness_weights);
                    RankedDeck {
                        rank: 0,
                        pareto_rank: 0,
                        deck: deck.clone(),
                        fitness,
                        packages: summarize_packages(root, pool, 8),
                        commander_compatibility: None,
                        candidate_status: "baseline".into(),
                    }
                });
            let best = ranked
                .first()
                .cloned()
                .or_else(|| baseline.clone())
                .ok_or_else(|| {
                    EvolutionError::UnsatisfiableConstraint(
                        "no feasible evolved candidate or Tune baseline".into(),
                    )
                })?;
            on_progress(GenerationProgress {
                generation: completed,
                total_generations: config.generations,
                evaluated_candidates,
                unique_evaluations,
                evaluations_avoided: evaluated_candidates.saturating_sub(unique_evaluations),
                unique_in_generation: evaluation_groups.len(),
                duplicates_in_generation: population.len().saturating_sub(evaluation_groups.len()),
                elapsed_ms: duration_ms(elapsed),
                candidates_per_second: evaluated_candidates as f64
                    / elapsed.as_secs_f64().max(0.001),
                unique_evaluations_per_second: unique_evaluations as f64
                    / elapsed.as_secs_f64().max(0.001),
                best,
                top: ranked,
                baseline,
                strategy_signature: strategy_signature.clone(),
                structural_model_revision: STRUCTURAL_MODEL_REVISION.into(),
                map_elite_cells: map_archive.len(),
            });

            if completed == config.generations || cancellation.is_cancelled() {
                break;
            }

            let elite_count = config.elite_count.clamp(1, population.len() - 1);
            let mut next = map_elite_seed_population(&population, &map_archive, elite_count);
            while next.len() < config.population_size {
                let parent_index = tournament_select(&population, config.tournament_size, &mut rng);
                let mut commanders = population[parent_index].commanders.clone();
                let commander_changed = config.evolve_commanders
                    && rng.chance(config.commander_mutation_rate.clamp(0.0, 1.0));
                if commander_changed {
                    commanders =
                        random_commander_selection(pool, &commander_roots, constraints, &mut rng)?;
                }
                let main_template = commander_bounds(pool, constraints, &base_bounds, &commanders)?;
                let total_bounds =
                    commander_bounds(pool, constraints, &base_copy_bounds, &commanders)?;
                let (mut genome, mut sideboard_genome) = if commander_changed {
                    let empty_sideboard = vec![0; pool.cards.len()];
                    let main_bounds = bounds_after_other_zone(&main_template, &empty_sideboard)?;
                    let focus = commander_focus_tags(&commanders, pool);
                    let genome =
                        generate_seed_genome(pool, constraints, &main_bounds, &focus, &mut rng)?;
                    let sideboard_genome = if config.evolve_sideboard
                        && constraints.target_sideboard_size > 0
                    {
                        let side_bounds = bounds_after_other_zone(&total_bounds, &genome)?;
                        generate_seed_genome(pool, &side_constraints, &side_bounds, &[], &mut rng)?
                    } else {
                        seed_sideboard.clone()
                    };
                    (genome, sideboard_genome)
                } else {
                    (
                        population[parent_index].genome.clone(),
                        population[parent_index].sideboard_genome.clone(),
                    )
                };
                if rng.chance(config.mutation_rate) {
                    let candidate_bounds =
                        bounds_after_other_zone(&main_template, &sideboard_genome)?;
                    mutate(
                        &mut genome,
                        &candidate_bounds,
                        pool,
                        constraints,
                        &mutation_guide,
                        MutationSettings {
                            swaps: config.mutation_swaps.max(1),
                            package_rate: config.package_mutation_rate,
                            package_size: config.package_size,
                            mana_rate: config.mana_mutation_rate,
                        },
                        &mut rng,
                    );
                    repair_mana_base(&mut genome, &candidate_bounds, pool, constraints, &mut rng);
                    if config.evolve_sideboard && constraints.target_sideboard_size > 0 {
                        let side_bounds = bounds_after_other_zone(&total_bounds, &genome)?;
                        mutate(
                            &mut sideboard_genome,
                            &side_bounds,
                            pool,
                            &side_constraints,
                            &mutation_guide,
                            MutationSettings {
                                swaps: config.mutation_swaps.max(1),
                                package_rate: config.package_mutation_rate,
                                package_size: config.package_size,
                                mana_rate: config.mana_mutation_rate,
                            },
                            &mut rng,
                        );
                    }
                }
                next.push(Candidate {
                    active_cards: compact_genome(&genome),
                    genome,
                    active_sideboard: compact_genome(&sideboard_genome),
                    sideboard_genome,
                    commanders,
                    fitness: Fitness::default(),
                });
            }
            population = next;
        }

        population.sort_by(|left, right| fitness_order(&right.fitness, &left.fitness));
        let final_seeds = (0..config.trials_per_candidate)
            .map(|trial| generation_seed(config.seed, completed.saturating_sub(1), trial))
            .collect::<Vec<_>>();
        let baseline = tune_root_deck
            .zip(tune_root_active.as_ref())
            .zip(strategy_signature.as_ref())
            .map(|((deck, root), signature)| {
                let mut fitness = self.evaluator.evaluate(
                    root,
                    &first_commanders,
                    pool,
                    &final_seeds,
                    config.turn_horizon,
                    context,
                );
                apply_tune_policy(
                    &mut fitness,
                    root,
                    root,
                    signature,
                    &config.tune_policy,
                    pool,
                    true,
                );
                apply_combo_package_evidence(
                    &mut fitness,
                    root,
                    pool,
                    &config.combo_packages,
                    config.turn_horizon,
                );
                fitness.score = weighted_fitness_score(&fitness, &config.fitness_weights);
                RankedDeck {
                    rank: 0,
                    pareto_rank: 0,
                    deck: deck.clone(),
                    fitness,
                    packages: summarize_packages(root, pool, 8),
                    commander_compatibility: None,
                    candidate_status: "baseline".into(),
                }
            });
        let mut frontier = ranked_decks(&population, pool, 8);
        if let Some(root) = tune_root_genome.as_ref() {
            frontier.retain(|candidate| {
                deck_to_genome(&candidate.deck, pool)
                    .is_ok_and(|candidate_genome| candidate_genome != *root)
            });
        }
        // The hot loop carries scalar/blocking evidence only. Hydrate the
        // small visible frontier once so UI inspection still receives exact
        // per-card actions, copy rationales, targets, and warnings.
        for candidate in &mut frontier {
            let active = compact_genome(&deck_to_genome(&candidate.deck, pool)?);
            let commanders = command_zone_to_indices(&candidate.deck, pool)?;
            let mut fitness = self.evaluator.evaluate(
                &active,
                &commanders,
                pool,
                &final_seeds,
                config.turn_horizon,
                context,
            );
            fitness.sideboard_quality = sideboard_quality(
                &compact_genome(&sideboard_to_genome(&candidate.deck, pool)?),
                pool,
            );
            apply_combo_package_evidence(
                &mut fitness,
                &active,
                pool,
                &config.combo_packages,
                config.turn_horizon,
            );
            if let (Some(root), Some(signature)) =
                (tune_root_active.as_ref(), strategy_signature.as_ref())
            {
                apply_tune_policy(
                    &mut fitness,
                    &active,
                    root,
                    signature,
                    &config.tune_policy,
                    pool,
                    false,
                );
            }
            fitness.score = weighted_fitness_score(&fitness, &config.fitness_weights);
            candidate.fitness = fitness;
        }
        let map_elites = visible_map_elites(&map_archive, pool, 32);
        Ok(RunResult {
            generations_completed: completed,
            evaluated_candidates,
            unique_evaluations,
            evaluations_avoided: evaluated_candidates.saturating_sub(unique_evaluations),
            elapsed_ms: duration_ms(started.elapsed()),
            cancelled: cancellation.is_cancelled(),
            frontier,
            baseline,
            strategy_signature,
            promotion_evidence: None,
            structural_model_revision: STRUCTURAL_MODEL_REVISION.into(),
            map_elites,
        })
    }
}

fn group_identical_candidates(population: &[Candidate]) -> Vec<EvaluationGroup> {
    let mut groups = Vec::<EvaluationGroup>::new();
    let mut buckets = HashMap::<u64, Vec<usize>>::new();
    for (candidate_index, candidate) in population.iter().enumerate() {
        let fingerprint = candidate_fingerprint(candidate);
        let matching_group = buckets.get(&fingerprint).and_then(|group_indices| {
            group_indices.iter().copied().find(|group_index| {
                let representative = &population[groups[*group_index].representative];
                representative.active_cards == candidate.active_cards
                    && representative.active_sideboard == candidate.active_sideboard
                    && commanders_equivalent(&representative.commanders, &candidate.commanders)
            })
        });
        if let Some(group_index) = matching_group {
            groups[group_index].members.push(candidate_index);
        } else {
            let group_index = groups.len();
            groups.push(EvaluationGroup {
                representative: candidate_index,
                members: vec![candidate_index],
            });
            buckets.entry(fingerprint).or_default().push(group_index);
        }
    }
    groups
}

fn candidate_fingerprint(candidate: &Candidate) -> u64 {
    let mut hasher = DefaultHasher::new();
    candidate.active_cards.hash(&mut hasher);
    candidate.active_sideboard.hash(&mut hasher);
    let mut commanders = candidate.commanders.clone();
    commanders.sort_unstable();
    commanders.hash(&mut hasher);
    hasher.finish()
}

fn commanders_equivalent(left: &[usize], right: &[usize]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut left = left.to_vec();
    let mut right = right.to_vec();
    left.sort_unstable();
    right.sort_unstable();
    left == right
}

fn validate_inputs(
    seed_deck: &Deck,
    pool: &CardPool,
    constraints: &DeckConstraints,
    config: &EvolutionConfig,
) -> Result<(), EvolutionError> {
    if pool.cards.is_empty() {
        return Err(EvolutionError::EmptyPool);
    }
    let mut logical_names = HashSet::with_capacity(pool.cards.len());
    if let Some(duplicate) = pool
        .cards
        .iter()
        .find(|card| !logical_names.insert(card.name.to_ascii_lowercase()))
    {
        return Err(EvolutionError::UnsatisfiableConstraint(format!(
            "logical card pool contains duplicate printings of {}",
            duplicate.name
        )));
    }
    if constraints
        .allowed_colors
        .as_ref()
        .is_some_and(Vec::is_empty)
    {
        return Err(EvolutionError::UnsatisfiableConstraint(
            "choose at least one deck color; choose Colorless for a zero-color deck".into(),
        ));
    }
    if let Some(allowed) = &constraints.allowed_colors {
        let incompatible = seed_deck
            .command_zone
            .iter()
            .chain(seed_deck.mainboard.iter())
            .chain(seed_deck.sideboard.iter())
            .filter_map(|entry| pool.card(&entry.card_id))
            .find(|card| !card.matches_color_selection(allowed));
        if let Some(card) = incompatible {
            return Err(EvolutionError::UnsatisfiableConstraint(format!(
                "{} is outside the selected deck colors",
                card.name
            )));
        }
    }
    if config.population_size < 2 {
        return Err(EvolutionError::PopulationTooSmall);
    }
    if !config.mutation_rate.is_finite()
        || !(0.0..=1.0).contains(&config.mutation_rate)
        || !config.package_mutation_rate.is_finite()
        || !(0.0..=1.0).contains(&config.package_mutation_rate)
        || !config.mana_mutation_rate.is_finite()
        || !(0.0..=1.0).contains(&config.mana_mutation_rate)
        || !(2..=8).contains(&config.package_size)
        || !config.commander_mutation_rate.is_finite()
        || !(0.0..=1.0).contains(&config.commander_mutation_rate)
    {
        return Err(EvolutionError::UnsatisfiableConstraint(
            "mutation rates must be between 0 and 1 and package width must be between 2 and 8"
                .into(),
        ));
    }
    let weights = &config.fitness_weights;
    let values = [
        weights.consistency,
        weights.tempo,
        weights.synergy,
        weights.interaction,
        weights.resilience,
        weights.commander,
        weights.sideboard,
        weights.mana_failure_penalty,
        weights.legendary_dead_draw_penalty,
        weights.unsupported_synergy_penalty,
    ];
    if values
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
        || values[..7].iter().sum::<f64>() <= 0.0
    {
        return Err(EvolutionError::UnsatisfiableConstraint(
            "fitness weights must be finite and non-negative, with at least one positive reward"
                .into(),
        ));
    }
    let mut legality_constraints = constraints.clone();
    if config.evolve_commanders && seed_deck.command_zone.is_empty() {
        legality_constraints.minimum_commanders = 0;
    }
    validate_deck_legality(seed_deck, pool, &legality_constraints)
        .map_err(|error| EvolutionError::UnsatisfiableConstraint(error.to_string()))?;
    if seed_deck.mainboard_count() != 0
        && seed_deck.mainboard_count() != constraints.target_mainboard_size
    {
        return Err(EvolutionError::WrongDeckSize {
            actual: seed_deck.mainboard_count(),
            expected: constraints.target_mainboard_size,
        });
    }
    if config.evolve_sideboard
        && seed_deck.sideboard_count() != 0
        && seed_deck.sideboard_count() != constraints.target_sideboard_size
    {
        return Err(EvolutionError::UnsatisfiableConstraint(format!(
            "seed sideboard contains {} cards; the configured target is {}",
            seed_deck.sideboard_count(),
            constraints.target_sideboard_size
        )));
    }
    if config.evolve_commanders && !config.evolve_sideboard && seed_deck.sideboard_count() > 0 {
        return Err(EvolutionError::UnsatisfiableConstraint(
            "evolving commanders with a sideboard also requires sideboard evolution so color identity remains legal"
                .into(),
        ));
    }
    Ok(())
}

fn bounds(pool: &CardPool, constraints: &DeckConstraints) -> Result<Vec<(u8, u8)>, EvolutionError> {
    let by_id = pool.index_by_id();
    let mut bounds = copy_bounds(pool, constraints);
    for locked in &constraints.locked_cards {
        let Some(index) = by_id.get(&locked.card_id) else {
            return Err(EvolutionError::UnknownCard(locked.card_id.0.clone()));
        };
        if locked.minimum > locked.maximum {
            return Err(EvolutionError::UnsatisfiableConstraint(
                locked.card_id.0.clone(),
            ));
        }
        if bounds[*index].1 == 0 && locked.maximum > 0 {
            return Err(EvolutionError::UnsatisfiableConstraint(format!(
                "{} is outside the selected deck colors",
                pool.cards[*index].name
            )));
        }
        bounds[*index] = (locked.minimum, locked.maximum);
    }
    Ok(bounds)
}

fn copy_bounds(pool: &CardPool, constraints: &DeckConstraints) -> Vec<(u8, u8)> {
    let total_size = constraints
        .target_mainboard_size
        .saturating_add(constraints.target_sideboard_size)
        .min(u8::MAX as usize) as u8;
    pool.cards
        .iter()
        .map(|card| {
            (
                0,
                if constraints
                    .allowed_colors
                    .as_ref()
                    .is_some_and(|allowed| !card.matches_color_selection(allowed))
                {
                    0
                } else if card.features.unlimited_copies {
                    total_size
                } else {
                    constraints.default_max_copies
                },
            )
        })
        .collect()
}

fn bounds_after_other_zone(
    bounds: &[(u8, u8)],
    other_zone: &[u8],
) -> Result<Vec<(u8, u8)>, EvolutionError> {
    bounds
        .iter()
        .zip(other_zone)
        .map(|((minimum, maximum), other)| {
            let remaining = maximum.saturating_sub(*other);
            if remaining < *minimum {
                Err(EvolutionError::UnsatisfiableConstraint(
                    "combined mainboard and sideboard copy limits conflict with locked cards"
                        .into(),
                ))
            } else {
                Ok((*minimum, remaining))
            }
        })
        .collect()
}

fn sideboard_constraints(constraints: &DeckConstraints) -> DeckConstraints {
    DeckConstraints {
        target_mainboard_size: constraints.target_sideboard_size,
        target_sideboard_size: 0,
        minimum_commanders: 0,
        maximum_commanders: 0,
        default_max_copies: constraints.default_max_copies,
        locked_cards: Vec::new(),
        minimum_lands: None,
        maximum_lands: None,
        allowed_colors: constraints.allowed_colors.clone(),
    }
}

fn generate_seed_genome(
    pool: &CardPool,
    constraints: &DeckConstraints,
    bounds: &[(u8, u8)],
    preferred_focus_tags: &[String],
    rng: &mut SplitMix64,
) -> Result<Vec<u8>, EvolutionError> {
    let prefer_existing_copies = constraints.default_max_copies > 1;
    let mut genome: Vec<u8> = bounds.iter().map(|(minimum, _)| *minimum).collect();
    let palette = seed_color_palette(pool, constraints, &genome, rng);
    let focus_tag = seed_focus_tag(pool, bounds, &palette, preferred_focus_tags, rng);
    let mut total: usize = genome.iter().map(|count| *count as usize).sum();
    if total > constraints.target_mainboard_size {
        return Err(EvolutionError::UnsatisfiableConstraint(
            "locked card minimums exceed deck size".into(),
        ));
    }
    let mut lands: usize = genome
        .iter()
        .enumerate()
        .filter(|(index, _)| pool.cards[*index].features.is_land)
        .map(|(_, count)| *count as usize)
        .sum();
    let target_lands = constraints.minimum_lands.unwrap_or(0);
    let mut land_choices: Vec<usize> = genome
        .iter()
        .enumerate()
        .filter_map(|(index, count)| {
            (pool.cards[index].features.is_land
                && card_palette_compatible(&pool.cards[index], &palette)
                && *count < bounds[index].1)
                .then_some(index)
        })
        .collect();

    if land_choices.is_empty() {
        land_choices = genome
            .iter()
            .enumerate()
            .filter_map(|(index, count)| {
                (pool.cards[index].features.is_land && *count < bounds[index].1).then_some(index)
            })
            .collect();
    }

    while lands < target_lands && total < constraints.target_mainboard_size {
        if land_choices.is_empty() {
            return Err(EvolutionError::UnsatisfiableConstraint(
                "not enough eligible lands in the card pool".into(),
            ));
        }
        let identity_basics = land_choices
            .iter()
            .copied()
            .filter(|index| {
                let card = &pool.cards[*index];
                card.features.unlimited_copies
                    && card.features.initial_modes.iter().any(|mode| {
                        mode.is_land
                            && mode.mana_sources.iter().any(|source| {
                                source.spend_rule == ManaSpendRule::Unrestricted
                                    && source.colors.iter().any(|color| palette.contains(color))
                            })
                    })
            })
            .collect::<Vec<_>>();
        let active_land_choices = if constraints.default_max_copies == 1
            && !identity_basics.is_empty()
            && rng.chance(0.88)
        {
            &identity_basics
        } else {
            &land_choices
        };
        let choice = choose_fill_position(active_land_choices, &genome, pool, true, rng);
        let index = active_land_choices[choice];
        genome[index] += 1;
        if genome[index] >= bounds[index].1 {
            land_choices.retain(|candidate| *candidate != index);
        }
        lands += 1;
        total += 1;
    }

    let mut choices: Vec<usize> = genome
        .iter()
        .enumerate()
        .filter_map(|(index, count)| {
            (*count < bounds[index].1 && card_palette_compatible(&pool.cards[index], &palette))
                .then_some(index)
        })
        .collect();
    while total < constraints.target_mainboard_size {
        if constraints
            .maximum_lands
            .is_some_and(|maximum| lands >= maximum)
        {
            choices.retain(|index| !pool.cards[*index].features.is_land);
        }
        if choices.is_empty() {
            choices = genome
                .iter()
                .enumerate()
                .filter_map(|(index, count)| (*count < bounds[index].1).then_some(index))
                .collect();
            if constraints
                .maximum_lands
                .is_some_and(|maximum| lands >= maximum)
            {
                choices.retain(|index| !pool.cards[*index].features.is_land);
            }
            if choices.is_empty() {
                return Err(EvolutionError::UnsatisfiableConstraint(
                    "card pool cannot fill the requested deck size".into(),
                ));
            }
        }
        let focused_choices = focus_tag
            .as_deref()
            .filter(|_| rng.chance(0.82))
            .map(|focus| {
                choices
                    .iter()
                    .copied()
                    .filter(|index| {
                        !pool.cards[*index].features.is_land
                            && pool.cards[*index]
                                .features
                                .tags
                                .iter()
                                .any(|tag| tag == focus)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let active_choices = if focused_choices.is_empty() {
            &choices
        } else {
            &focused_choices
        };
        let choice =
            choose_fill_position(active_choices, &genome, pool, prefer_existing_copies, rng);
        let index = active_choices[choice];
        genome[index] += 1;
        if genome[index] >= bounds[index].1 {
            choices.retain(|candidate| *candidate != index);
        }
        lands += usize::from(pool.cards[index].features.is_land);
        total += 1;
    }
    Ok(genome)
}

fn seed_focus_tag(
    pool: &CardPool,
    bounds: &[(u8, u8)],
    palette: &HashSet<Color>,
    preferred_focus_tags: &[String],
    rng: &mut SplitMix64,
) -> Option<String> {
    let mut groups = BTreeMap::<&str, usize>::new();
    for (index, card) in pool.cards.iter().enumerate() {
        if bounds[index].1 == 0 || card.features.is_land || !card_palette_compatible(card, palette)
        {
            continue;
        }
        let mut seen = HashSet::new();
        for tag in &card.features.tags {
            if is_synergy_tag(tag) && seen.insert(tag.as_str()) {
                *groups.entry(tag).or_default() += 1;
            }
        }
    }
    let preferred = preferred_focus_tags
        .iter()
        .filter(|tag| groups.get(tag.as_str()).is_some_and(|count| *count >= 6))
        .collect::<Vec<_>>();
    if !preferred.is_empty() && rng.chance(0.9) {
        return Some((*preferred[rng.usize(preferred.len())]).clone());
    }
    let candidates = groups
        .into_iter()
        .filter_map(|(tag, members)| (6..=512).contains(&members).then_some(tag))
        .collect::<Vec<_>>();
    (!candidates.is_empty()).then(|| candidates[rng.usize(candidates.len())].to_string())
}

fn seed_color_palette(
    pool: &CardPool,
    constraints: &DeckConstraints,
    genome: &[u8],
    rng: &mut SplitMix64,
) -> HashSet<Color> {
    if let Some(allowed) = &constraints.allowed_colors {
        return allowed.iter().copied().collect();
    }
    let mut locked = HashSet::new();
    for (index, _) in genome.iter().enumerate().filter(|(_, count)| **count > 0) {
        for mode in pool.cards[index]
            .features
            .initial_modes
            .iter()
            .filter(|mode| !mode.is_land)
        {
            for (slot, pips) in mode.mana_cost.colored[..5].iter().enumerate() {
                if *pips > 0 {
                    locked.insert(slot_color(slot));
                }
            }
            for hybrid in &mode.mana_cost.hybrid {
                locked.extend(
                    hybrid
                        .iter()
                        .copied()
                        .filter(|color| *color != Color::Colorless),
                );
            }
        }
    }
    if !locked.is_empty() {
        return locked;
    }
    let roll = rng.usize(100);
    let mut count = match roll {
        0..=19 => 1,
        20..=74 => 2,
        75..=94 => 3,
        _ => 5,
    };
    if count > 3 {
        let broad_fixing_capacity = pool
            .cards
            .iter()
            .enumerate()
            .filter(|(_, card)| {
                card.features.initial_modes.iter().any(|mode| {
                    mode.is_land
                        && mode.mana_sources.iter().any(|source| {
                            source.spend_rule == ManaSpendRule::Unrestricted
                                && source.color_rule == ManaColorRule::Fixed
                                && source
                                    .colors
                                    .iter()
                                    .filter(|color| **color != Color::Colorless)
                                    .count()
                                    >= 3
                        })
                })
            })
            .map(|(index, _)| usize::from(pool.cards[index].features.unlimited_copies).max(4))
            .sum::<usize>();
        if broad_fixing_capacity < 8 {
            count = 3;
        }
    }
    let mut colors = pool
        .cards
        .iter()
        .filter(|card| !card.features.is_land)
        .flat_map(|card| card.colors.iter().copied())
        .filter(|color| *color != Color::Colorless)
        .collect::<Vec<_>>();
    colors.sort_by_key(|color| mana_slot(*color));
    colors.dedup();
    if colors.is_empty() {
        colors = vec![
            Color::White,
            Color::Blue,
            Color::Black,
            Color::Red,
            Color::Green,
        ];
    }
    rng.shuffle(&mut colors);
    colors.into_iter().take(count).collect()
}

fn slot_color(slot: usize) -> Color {
    [
        Color::White,
        Color::Blue,
        Color::Black,
        Color::Red,
        Color::Green,
        Color::Colorless,
    ][slot]
}

fn card_palette_compatible(card: &crate::Card, palette: &HashSet<Color>) -> bool {
    if card.features.is_land && card.features.initial_modes.is_empty() {
        return true;
    }
    if card.features.initial_modes.is_empty() {
        return card
            .colors
            .iter()
            .all(|color| *color == Color::Colorless || palette.contains(color));
    }
    card.features.initial_modes.iter().any(|mode| {
        if mode.is_land {
            let fixed = mode.mana_sources.iter().any(|source| {
                source.spend_rule == ManaSpendRule::Unrestricted
                    && source.color_rule == ManaColorRule::Fixed
                    && source
                        .colors
                        .iter()
                        .any(|color| *color == Color::Colorless || palette.contains(color))
            });
            fixed || mode.mana_sources.is_empty()
        } else {
            mode.mana_cost.colored[..5]
                .iter()
                .enumerate()
                .all(|(slot, pips)| *pips == 0 || palette.contains(&slot_color(slot)))
                && mode.mana_cost.hybrid.iter().all(|options| {
                    options
                        .iter()
                        .any(|color| *color == Color::Colorless || palette.contains(color))
                })
        }
    })
}

fn genome_spell_palette(genome: &[u8], pool: &CardPool) -> HashSet<Color> {
    let mut palette = HashSet::new();
    for (index, _) in genome.iter().enumerate().filter(|(_, count)| **count > 0) {
        let card = &pool.cards[index];
        if card.features.initial_modes.is_empty() {
            palette.extend(
                card.colors
                    .iter()
                    .copied()
                    .filter(|color| *color != Color::Colorless),
            );
            continue;
        }
        for mode in card
            .features
            .initial_modes
            .iter()
            .filter(|mode| !mode.is_land)
        {
            for (slot, pips) in mode.mana_cost.colored[..5].iter().enumerate() {
                if *pips > 0 {
                    palette.insert(slot_color(slot));
                }
            }
            palette.extend(
                mode.mana_cost
                    .hybrid
                    .iter()
                    .flatten()
                    .copied()
                    .filter(|color| *color != Color::Colorless),
            );
        }
    }
    palette
}

fn choose_fill_position(
    choices: &[usize],
    genome: &[u8],
    pool: &CardPool,
    prefer_existing_copies: bool,
    rng: &mut SplitMix64,
) -> usize {
    if prefer_existing_copies && rng.chance(existing_copy_probability(choices, genome, pool)) {
        let existing = choices
            .iter()
            .enumerate()
            .filter_map(|(position, index)| {
                (genome[*index] > 0 && role_copy_allowed(*index, genome[*index], pool, rng))
                    .then_some(position)
            })
            .collect::<Vec<_>>();
        if !existing.is_empty() {
            return existing[rng.usize(existing.len())];
        }
    }
    let mut position = rng.usize(choices.len());
    for _ in 0..16 {
        let index = choices[position];
        if genome[index] == 0 || role_copy_allowed(index, genome[index], pool, rng) {
            return position;
        }
        position = rng.usize(choices.len());
    }
    position
}

fn role_copy_allowed(index: usize, current: u8, pool: &CardPool, rng: &mut SplitMix64) -> bool {
    let features = &pool.cards[index].features;
    let target = preferred_copy_target(&pool.cards[index]);
    if current < target {
        return true;
    }
    if current > target {
        return false;
    }
    let has_narrow_action = is_narrow_mainboard_card(&pool.cards[index]);
    rng.chance(if has_narrow_action {
        0.03
    } else if features.is_legendary || features.mana_value >= 5.0 {
        0.08
    } else {
        0.18
    })
}

fn preferred_copy_target(card: &crate::Card) -> u8 {
    let features = &card.features;
    if features.is_land {
        return if features.is_legendary { 1 } else { 4 };
    }
    if is_narrow_mainboard_card(card) {
        return 1;
    }
    if features.is_legendary || features.mana_value >= 5.0 {
        return 2;
    }
    let actions = features
        .initial_modes
        .iter()
        .flat_map(|mode| mode.abilities.iter())
        .collect::<Vec<_>>();
    if actions.iter().any(|ability| {
        matches!(
            ability.kind,
            AbilityKind::Copy
                | AbilityKind::CreateToken
                | AbilityKind::TokenMultiplier
                | AbilityKind::CounterSpell
                | AbilityKind::Destroy
                | AbilityKind::Exile
        )
    }) || features
        .tags
        .iter()
        .any(|tag| tag == "copy" || tag == "spellslinger")
    {
        return 4;
    }
    if features.pressure >= 0.5 || features.card_advantage > 0.0 || features.ramp > 0.0 {
        return 3;
    }
    2
}

fn copy_rationale(card: &crate::Card) -> &'static str {
    let features = &card.features;
    if features.is_land {
        return "land";
    }
    if is_narrow_mainboard_card(card) {
        return "mainboard hate or narrow utility";
    }
    if features.is_legendary {
        return "legendary";
    }
    if features.mana_value >= 5.0 {
        return "expensive";
    }
    let actions = features
        .initial_modes
        .iter()
        .flat_map(|mode| mode.abilities.iter())
        .collect::<Vec<_>>();
    if actions.iter().any(|ability| {
        matches!(
            ability.kind,
            AbilityKind::Copy
                | AbilityKind::CreateToken
                | AbilityKind::TokenMultiplier
                | AbilityKind::SacrificeOutlet
        )
    }) {
        return "core engine or enabler";
    }
    if actions.iter().any(|ability| {
        matches!(
            ability.kind,
            AbilityKind::CounterSpell
                | AbilityKind::Destroy
                | AbilityKind::Exile
                | AbilityKind::Bounce
        ) && !ability.narrow
    }) {
        return "broad interaction";
    }
    if features.pressure >= 0.5 {
        return "efficient threat";
    }
    if actions
        .iter()
        .any(|ability| matches!(ability.kind, AbilityKind::Draw | AbilityKind::Ramp))
    {
        return "support";
    }
    "situational support"
}

fn is_narrow_mainboard_card(card: &crate::Card) -> bool {
    let text = card.oracle_text.to_ascii_lowercase();
    card.features
        .initial_modes
        .iter()
        .flat_map(|mode| mode.abilities.iter())
        .any(|ability| ability.narrow || ability.kind == AbilityKind::GraveyardHate)
        || text.contains("target player's graveyard")
        || text.contains("target opponent's graveyard")
}

fn ability_is_graveyard_hate(card: &crate::Card, ability: &AbilityProfile) -> bool {
    if ability.kind == AbilityKind::GraveyardHate {
        return true;
    }
    let label = ability.label.to_ascii_lowercase();
    let oracle = card.oracle_text.to_ascii_lowercase();
    ability.kind == AbilityKind::Exile
        && (label.contains("graveyard")
            || oracle.contains("target player's graveyard")
            || oracle.contains("target opponent's graveyard"))
}

fn ability_is_narrow(card: &crate::Card, ability: &AbilityProfile) -> bool {
    ability.narrow || ability_is_graveyard_hate(card, ability)
}

fn existing_copy_probability(choices: &[usize], genome: &[u8], pool: &CardPool) -> f32 {
    let (deficit, active) = choices.iter().filter(|index| genome[**index] > 0).fold(
        (0_u32, 0_u32),
        |(deficit, active), index| {
            (
                deficit
                    + u32::from(
                        preferred_copy_target(&pool.cards[*index]).saturating_sub(genome[*index]),
                    ),
                active + 1,
            )
        },
    );
    if active == 0 {
        0.0
    } else {
        (0.25 + (deficit as f32 / active as f32) * 0.14).clamp(0.25, 0.72)
    }
}

fn deck_to_genome(deck: &Deck, pool: &CardPool) -> Result<Vec<u8>, EvolutionError> {
    entries_to_genome(&deck.mainboard, pool)
}

fn sideboard_to_genome(deck: &Deck, pool: &CardPool) -> Result<Vec<u8>, EvolutionError> {
    entries_to_genome(&deck.sideboard, pool)
}

fn entries_to_genome(entries: &[DeckEntry], pool: &CardPool) -> Result<Vec<u8>, EvolutionError> {
    let by_id = pool.index_by_id();
    let mut genome = vec![0_u8; pool.cards.len()];
    for entry in entries {
        let Some(index) = by_id.get(&entry.card_id) else {
            return Err(EvolutionError::UnknownCard(entry.card_id.0.clone()));
        };
        genome[*index] = genome[*index].saturating_add(entry.quantity);
    }
    Ok(genome)
}

fn command_zone_to_indices(deck: &Deck, pool: &CardPool) -> Result<Vec<usize>, EvolutionError> {
    let by_id = pool.index_by_id();
    deck.command_zone
        .iter()
        .map(|entry| {
            by_id
                .get(&entry.card_id)
                .copied()
                .ok_or_else(|| EvolutionError::UnknownCard(entry.card_id.0.clone()))
        })
        .collect()
}

fn commander_focus_tags(commanders: &[usize], pool: &CardPool) -> Vec<String> {
    let mut tags = Vec::new();
    for index in commanders {
        let card = &pool.cards[*index];
        tags.extend(
            card.features
                .tags
                .iter()
                .filter(|tag| is_synergy_tag(tag))
                .cloned(),
        );
        if card.type_line.contains("Creature") && card.features.mana_value <= 2.0 {
            tags.push("equipment".into());
            tags.push("aura".into());
            tags.push("counters".into());
        }
    }
    tags.sort();
    tags.dedup();
    tags
}

fn commander_candidates(pool: &CardPool) -> Vec<usize> {
    pool.cards
        .iter()
        .enumerate()
        .filter_map(|(index, card)| {
            (is_commander_eligible(card)
                && !card.type_line.to_ascii_lowercase().contains("background"))
            .then_some(index)
        })
        .collect()
}

fn random_commander_selection(
    pool: &CardPool,
    roots: &[usize],
    constraints: &DeckConstraints,
    rng: &mut SplitMix64,
) -> Result<Vec<usize>, EvolutionError> {
    if constraints.maximum_commanders == 0 {
        return Ok(Vec::new());
    }
    if roots.is_empty() {
        return Err(EvolutionError::UnsatisfiableConstraint(
            "no eligible commanders".into(),
        ));
    }
    for _ in 0..64 {
        let first = roots[rng.usize(roots.len())];
        let needs_pair = constraints.minimum_commanders >= 2;
        if !needs_pair && !rng.chance(0.18) {
            return Ok(vec![first]);
        }
        let compatible: Vec<usize> = if constraints.maximum_commanders >= 2 {
            pool.cards
                .iter()
                .enumerate()
                .filter_map(|(index, card)| {
                    (index != first
                        && is_commander_eligible(card)
                        && commanders_compatible(&pool.cards[first], card))
                    .then_some(index)
                })
                .collect()
        } else {
            Vec::new()
        };
        if needs_pair && compatible.is_empty() {
            continue;
        }
        if !compatible.is_empty() {
            return Ok(vec![first, compatible[rng.usize(compatible.len())]]);
        }
        return Ok(vec![first]);
    }
    Err(EvolutionError::UnsatisfiableConstraint(
        "could not find a compatible commander pair".into(),
    ))
}

fn commander_bounds(
    pool: &CardPool,
    constraints: &DeckConstraints,
    base_bounds: &[(u8, u8)],
    commanders: &[usize],
) -> Result<Vec<(u8, u8)>, EvolutionError> {
    if commanders.is_empty() {
        return Ok(base_bounds.to_vec());
    }
    let identity: HashSet<Color> = commanders
        .iter()
        .flat_map(|index| pool.cards[*index].color_identity.iter().copied())
        .collect();
    let commander_set: HashSet<usize> = commanders.iter().copied().collect();
    let mut result = base_bounds.to_vec();
    for (index, card) in pool.cards.iter().enumerate() {
        let outside_identity = card
            .color_identity
            .iter()
            .any(|color| *color != Color::Colorless && !identity.contains(color));
        if commander_set.contains(&index) || outside_identity {
            if result[index].0 > 0 {
                return Err(EvolutionError::UnsatisfiableConstraint(format!(
                    "locked card {} is outside the selected commander identity",
                    card.name
                )));
            }
            result[index] = (0, 0);
        }
    }
    if result
        .iter()
        .map(|(_, maximum)| *maximum as usize)
        .sum::<usize>()
        < constraints.target_mainboard_size
    {
        return Err(EvolutionError::UnsatisfiableConstraint(
            "commander identity cannot fill the requested library".into(),
        ));
    }
    Ok(result)
}

fn command_entries(commanders: &[usize], pool: &CardPool) -> Vec<DeckEntry> {
    commanders
        .iter()
        .map(|index| DeckEntry {
            card_id: pool.cards[*index].id.clone(),
            quantity: 1,
        })
        .collect()
}

fn compact_genome(genome: &[u8]) -> Vec<(usize, u8)> {
    genome
        .iter()
        .enumerate()
        .filter_map(|(index, count)| (*count > 0).then_some((index, *count)))
        .collect()
}

fn genome_to_deck(
    genome: &[u8],
    sideboard_genome: &[u8],
    pool: &CardPool,
    command_zone: &[DeckEntry],
    name: String,
) -> Deck {
    Deck {
        name,
        command_zone: command_zone.to_vec(),
        mainboard: genome
            .iter()
            .enumerate()
            .filter(|(_, count)| **count > 0)
            .map(|(index, count)| DeckEntry {
                card_id: pool.cards[index].id.clone(),
                quantity: *count,
            })
            .collect(),
        sideboard: sideboard_genome
            .iter()
            .enumerate()
            .filter(|(_, count)| **count > 0)
            .map(|(index, count)| DeckEntry {
                card_id: pool.cards[index].id.clone(),
                quantity: *count,
            })
            .collect(),
    }
}

fn mutate(
    genome: &mut [u8],
    bounds: &[(u8, u8)],
    pool: &CardPool,
    constraints: &DeckConstraints,
    guide: &MutationGuide,
    settings: MutationSettings,
    rng: &mut SplitMix64,
) {
    let mana_swap_applied = rng.chance(settings.mana_rate.clamp(0.0, 1.0))
        && mutate_mana_base(genome, bounds, pool, constraints, rng);
    if !guide.package_groups.is_empty() && rng.chance(settings.package_rate) {
        let applied = mutate_package(
            genome,
            bounds,
            pool,
            guide,
            settings.package_size.clamp(2, 8),
            constraints.default_max_copies > 1,
            rng,
        );
        if applied > 0 {
            return;
        }
    }
    mutate_random(
        genome,
        bounds,
        pool,
        constraints,
        guide,
        settings
            .swaps
            .saturating_sub(usize::from(mana_swap_applied)),
        rng,
    );
}

fn color_slot(color: Color) -> Option<usize> {
    match color {
        Color::White => Some(0),
        Color::Blue => Some(1),
        Color::Black => Some(2),
        Color::Red => Some(3),
        Color::Green => Some(4),
        Color::Colorless => None,
    }
}

fn mutate_mana_base(
    genome: &mut [u8],
    bounds: &[(u8, u8)],
    pool: &CardPool,
    constraints: &DeckConstraints,
    rng: &mut SplitMix64,
) -> bool {
    let mut demand = [0.0_f64; 5];
    let mut sources = [0.0_f64; 5];
    let mut spell_copies = 0usize;
    let mut mana_value_total = 0.0_f64;
    let mut land_count = 0usize;
    for (index, count) in genome
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, count)| *count > 0)
    {
        let card = &pool.cards[index];
        let copies = count as usize;
        if card.features.can_be_land || card.features.is_land {
            land_count = land_count.saturating_add(copies);
            if card.features.initial_modes.is_empty() {
                for color in &card.color_identity {
                    if let Some(slot) = color_slot(*color) {
                        sources[slot] += copies as f64;
                    }
                }
            } else {
                for mode in card
                    .features
                    .initial_modes
                    .iter()
                    .filter(|mode| mode.is_land)
                {
                    for color in &mode.mana_source_colors {
                        if let Some(slot) = color_slot(*color) {
                            sources[slot] += copies as f64;
                        }
                    }
                }
            }
        } else {
            spell_copies = spell_copies.saturating_add(copies);
            mana_value_total += copies as f64 * card.features.mana_value.max(0.0) as f64;
            let curve_weight = 1.0 + card.features.mana_value.max(0.0) as f64 / 4.0;
            if card.features.initial_modes.is_empty() {
                for color in &card.colors {
                    if let Some(slot) = color_slot(*color) {
                        demand[slot] += copies as f64 * curve_weight;
                    }
                }
            } else if let Some(mode) = card
                .features
                .initial_modes
                .iter()
                .find(|mode| !mode.is_land)
            {
                for (slot, pips) in mode.mana_cost.colored[..5].iter().enumerate() {
                    demand[slot] += copies as f64 * curve_weight * f64::from(*pips);
                }
            }
        }
    }

    let total_demand = demand.iter().sum::<f64>();
    if let Some(target_color) = (0..demand.len())
        .filter(|slot| demand[*slot] > 0.0)
        .filter(|slot| {
            let share = demand[*slot] / total_demand.max(f64::EPSILON);
            let desired = (6.0 + share * 10.0).round().clamp(6.0, 14.0);
            sources[*slot] + 0.01 < desired
        })
        .max_by(|left, right| {
            let desired = |slot: usize| {
                let share = demand[slot] / total_demand.max(f64::EPSILON);
                (6.0 + share * 10.0).round().clamp(6.0, 14.0)
            };
            (desired(*left) - sources[*left]).total_cmp(&(desired(*right) - sources[*right]))
        })
    {
        let supports_target = |index: usize| {
            let card = &pool.cards[index];
            if card.features.initial_modes.is_empty() {
                card.color_identity
                    .iter()
                    .filter_map(|color| color_slot(*color))
                    .any(|slot| slot == target_color)
            } else {
                card.features
                    .initial_modes
                    .iter()
                    .filter(|mode| mode.is_land)
                    .flat_map(|mode| &mode.mana_source_colors)
                    .filter_map(|color| color_slot(*color))
                    .any(|slot| slot == target_color)
            }
        };
        let additions = genome
            .iter()
            .enumerate()
            .filter_map(|(index, count)| {
                ((pool.cards[index].features.is_land || pool.cards[index].features.can_be_land)
                    && supports_target(index)
                    && *count < bounds[index].1)
                    .then_some(index)
            })
            .collect::<Vec<_>>();
        let removals = genome
            .iter()
            .enumerate()
            .filter_map(|(index, count)| {
                ((pool.cards[index].features.is_land || pool.cards[index].features.can_be_land)
                    && !supports_target(index)
                    && *count > bounds[index].0)
                    .then_some(index)
            })
            .collect::<Vec<_>>();
        if !additions.is_empty() && !removals.is_empty() {
            let add_position = choose_fill_position(&additions, genome, pool, true, rng);
            let add = additions[add_position];
            let remove = removals[rng.usize(removals.len())];
            genome[remove] -= 1;
            genome[add] += 1;
            return true;
        }
    }

    if spell_copies == 0 {
        return false;
    }
    let average_mana_value = mana_value_total / spell_copies as f64;
    let target_fraction = (0.34 + (average_mana_value - 2.5) * 0.025).clamp(0.28, 0.46);
    let target_lands =
        ((constraints.target_mainboard_size as f64 * target_fraction).round() as usize).clamp(
            constraints.minimum_lands.unwrap_or(0),
            constraints
                .maximum_lands
                .unwrap_or(constraints.target_mainboard_size),
        );
    let (removals, additions) = if land_count < target_lands {
        (
            genome
                .iter()
                .enumerate()
                .filter_map(|(index, count)| {
                    (!pool.cards[index].features.is_land && *count > bounds[index].0)
                        .then_some(index)
                })
                .collect::<Vec<_>>(),
            genome
                .iter()
                .enumerate()
                .filter_map(|(index, count)| {
                    (pool.cards[index].features.is_land && *count < bounds[index].1)
                        .then_some(index)
                })
                .collect::<Vec<_>>(),
        )
    } else if land_count > target_lands {
        (
            genome
                .iter()
                .enumerate()
                .filter_map(|(index, count)| {
                    (pool.cards[index].features.is_land && *count > bounds[index].0)
                        .then_some(index)
                })
                .collect::<Vec<_>>(),
            genome
                .iter()
                .enumerate()
                .filter_map(|(index, count)| {
                    (!pool.cards[index].features.is_land && *count < bounds[index].1)
                        .then_some(index)
                })
                .collect::<Vec<_>>(),
        )
    } else {
        return false;
    };
    if removals.is_empty() || additions.is_empty() {
        return false;
    }
    let remove = removals[rng.usize(removals.len())];
    let add_position = choose_fill_position(&additions, genome, pool, true, rng);
    let add = additions[add_position];
    genome[remove] -= 1;
    genome[add] += 1;
    true
}

fn repair_mana_base(
    genome: &mut [u8],
    bounds: &[(u8, u8)],
    pool: &CardPool,
    constraints: &DeckConstraints,
    rng: &mut SplitMix64,
) {
    repair_land_count(genome, bounds, pool, constraints, rng);
    repair_missing_color_sources(genome, bounds, pool, rng);
}

fn repair_missing_color_sources(
    genome: &mut [u8],
    bounds: &[(u8, u8)],
    pool: &CardPool,
    rng: &mut SplitMix64,
) {
    let mut required = [false; 5];
    for (index, count) in genome.iter().copied().enumerate() {
        if count == 0 {
            continue;
        }
        let card = &pool.cards[index];
        if let Some(mode) = card
            .features
            .initial_modes
            .iter()
            .find(|mode| !mode.is_land)
        {
            for (slot, pips) in mode.mana_cost.colored[..5].iter().enumerate() {
                required[slot] |= *pips > 0;
            }
        } else if !card.features.is_land {
            for color in &card.colors {
                if let Some(slot) = color_slot(*color) {
                    required[slot] = true;
                }
            }
        }
    }

    for missing_slot in 0..5 {
        if !required[missing_slot]
            || genome.iter().enumerate().any(|(index, count)| {
                *count > 0 && card_has_early_unrestricted_color(&pool.cards[index], missing_slot)
            })
        {
            continue;
        }
        let additions = genome
            .iter()
            .enumerate()
            .filter_map(|(index, count)| {
                (*count < bounds[index].1
                    && card_has_early_unrestricted_color(&pool.cards[index], missing_slot))
                .then_some(index)
            })
            .collect::<Vec<_>>();
        let minimum_supported_required = genome
            .iter()
            .enumerate()
            .filter(|(index, count)| {
                **count > bounds[*index].0 && pool.cards[*index].features.is_land
            })
            .map(|(index, _)| {
                required
                    .iter()
                    .enumerate()
                    .filter(|(slot, needed)| {
                        **needed && card_has_early_unrestricted_color(&pool.cards[index], *slot)
                    })
                    .count()
            })
            .min();
        let removals = genome
            .iter()
            .enumerate()
            .filter_map(|(index, count)| {
                let supported_required = required
                    .iter()
                    .enumerate()
                    .filter(|(slot, needed)| {
                        **needed && card_has_early_unrestricted_color(&pool.cards[index], *slot)
                    })
                    .count();
                (*count > bounds[index].0
                    && pool.cards[index].features.is_land
                    && Some(supported_required) == minimum_supported_required)
                    .then_some(index)
            })
            .collect::<Vec<_>>();
        if additions.is_empty() || removals.is_empty() {
            continue;
        }
        let add = additions[choose_fill_position(&additions, genome, pool, true, rng)];
        let remove = removals[rng.usize(removals.len())];
        if add != remove {
            genome[remove] -= 1;
            genome[add] += 1;
        }
    }
}

fn card_has_early_unrestricted_color(card: &crate::Card, slot: usize) -> bool {
    card.features
        .initial_modes
        .iter()
        .filter(|mode| mode.is_land)
        .any(|mode| {
            if mode.mana_sources.is_empty() {
                mode.mana_source_colors
                    .iter()
                    .any(|color| color_slot(*color) == Some(slot))
            } else {
                mode.mana_sources.iter().any(|source| {
                    source.activation_generic == 0
                        && source.earliest_turn <= 1
                        && source.spend_rule == ManaSpendRule::Unrestricted
                        && source.color_rule == ManaColorRule::Fixed
                        && source
                            .colors
                            .iter()
                            .any(|color| color_slot(*color) == Some(slot))
                })
            }
        })
}

fn repair_land_count(
    genome: &mut [u8],
    bounds: &[(u8, u8)],
    pool: &CardPool,
    constraints: &DeckConstraints,
    rng: &mut SplitMix64,
) {
    let mut land_count = genome
        .iter()
        .enumerate()
        .filter(|(index, _)| pool.cards[*index].features.is_land)
        .map(|(_, count)| usize::from(*count))
        .sum::<usize>();
    let (spell_copies, mana_value_total) = genome
        .iter()
        .copied()
        .enumerate()
        .filter(|(index, count)| *count > 0 && !pool.cards[*index].features.is_land)
        .fold((0_usize, 0.0_f64), |(copies, total), (index, count)| {
            (
                copies + usize::from(count),
                total
                    + f64::from(count) * f64::from(pool.cards[index].features.mana_value.max(0.0)),
            )
        });
    if spell_copies == 0 {
        return;
    }
    let average_mana_value = mana_value_total / spell_copies as f64;
    let target_fraction = (0.34 + (average_mana_value - 2.5) * 0.025).clamp(0.28, 0.46);
    let target_lands =
        ((constraints.target_mainboard_size as f64 * target_fraction).round() as usize).clamp(
            constraints.minimum_lands.unwrap_or(0),
            constraints
                .maximum_lands
                .unwrap_or(constraints.target_mainboard_size),
        );

    while land_count != target_lands {
        let adding_land = land_count < target_lands;
        let removals = genome
            .iter()
            .enumerate()
            .filter_map(|(index, count)| {
                let is_land = pool.cards[index].features.is_land;
                (*count > bounds[index].0 && is_land != adding_land).then_some(index)
            })
            .collect::<Vec<_>>();
        let additions = genome
            .iter()
            .enumerate()
            .filter_map(|(index, count)| {
                let is_land = pool.cards[index].features.is_land;
                (*count < bounds[index].1 && is_land == adding_land).then_some(index)
            })
            .collect::<Vec<_>>();
        if removals.is_empty() || additions.is_empty() {
            break;
        }
        let remove = removals[rng.usize(removals.len())];
        let add_position = choose_fill_position(&additions, genome, pool, true, rng);
        let add = additions[add_position];
        genome[remove] -= 1;
        genome[add] += 1;
        if adding_land {
            land_count += 1;
        } else {
            land_count -= 1;
        }
    }
}

fn mutate_package(
    genome: &mut [u8],
    bounds: &[(u8, u8)],
    pool: &CardPool,
    guide: &MutationGuide,
    width: usize,
    prefer_existing_copies: bool,
    rng: &mut SplitMix64,
) -> usize {
    let palette = genome_spell_palette(genome, pool);
    let mut removable: Vec<usize> = genome
        .iter()
        .enumerate()
        .filter_map(|(index, count)| (*count > bounds[index].0).then_some(index))
        .collect();
    let represented = guide
        .package_groups
        .iter()
        .enumerate()
        .filter_map(|(group_index, group)| {
            let overlap = group.iter().filter(|index| genome[**index] > 0).count();
            (overlap >= 2).then_some((group_index, overlap))
        })
        .collect::<Vec<_>>();
    let first_group = if !represented.is_empty() && rng.chance(0.85) {
        let maximum = represented
            .iter()
            .map(|(_, overlap)| *overlap)
            .max()
            .unwrap_or_default();
        let strongest = represented
            .iter()
            .filter_map(|(index, overlap)| (*overlap == maximum).then_some(*index))
            .collect::<Vec<_>>();
        strongest[rng.usize(strongest.len())]
    } else {
        rng.usize(guide.package_groups.len())
    };
    for offset in 0..guide.package_groups.len() {
        let group_index = (first_group + offset) % guide.package_groups.len();
        let group = &guide.package_groups[group_index];
        let mut applied = 0;
        let mut selected = HashSet::new();
        for _ in 0..width {
            let Some(add) = choose_package_add(
                group,
                genome,
                bounds,
                &selected,
                pool,
                &palette,
                prefer_existing_copies,
                rng,
            ) else {
                break;
            };
            let remove = choose_package_removal(&removable, group, add, genome, bounds, pool, rng)
                .or_else(|| choose_compatible_removal(&removable, add, genome, bounds, pool, rng));
            let Some(remove) = remove else {
                break;
            };
            genome[remove] -= 1;
            genome[add] += 1;
            update_removable(&mut removable, remove, add, genome, bounds);
            selected.insert(add);
            applied += 1;
        }
        if applied > 0 {
            return applied;
        }
    }
    0
}

#[allow(clippy::too_many_arguments)]
fn choose_package_add(
    group: &[usize],
    genome: &[u8],
    bounds: &[(u8, u8)],
    selected: &HashSet<usize>,
    pool: &CardPool,
    palette: &HashSet<Color>,
    prefer_existing_copies: bool,
    rng: &mut SplitMix64,
) -> Option<usize> {
    let start = rng.usize(group.len());
    let choose_existing =
        prefer_existing_copies && rng.chance(existing_copy_probability(group, genome, pool));
    let mut eligible = |index: usize| {
        genome[index] < bounds[index].1
            && !selected.contains(&index)
            && (genome[index] > 0 || card_palette_compatible(&pool.cards[index], palette))
            && (genome[index] == 0
                || genome[index] < bounds[index].0
                || role_copy_allowed(index, genome[index], pool, rng))
    };
    if choose_existing {
        let existing = (0..group.len())
            .map(|offset| group[(start + offset) % group.len()])
            .find(|index| eligible(*index) && genome[*index] > 0);
        if existing.is_some() {
            return existing;
        }
    }
    (0..group.len())
        .map(|offset| group[(start + offset) % group.len()])
        .find(|index| eligible(*index))
        .or_else(|| {
            (0..group.len())
                .map(|offset| group[(start + offset) % group.len()])
                .find(|index| eligible(*index))
        })
}

fn choose_package_removal(
    removable: &[usize],
    group: &[usize],
    add: usize,
    genome: &[u8],
    bounds: &[(u8, u8)],
    pool: &CardPool,
    rng: &mut SplitMix64,
) -> Option<usize> {
    choose_removal(
        removable,
        |index| {
            index != add
                && genome[index] > bounds[index].0
                && pool.cards[index].features.is_land == pool.cards[add].features.is_land
                && group.binary_search(&index).is_err()
        },
        rng,
    )
}

fn choose_compatible_removal(
    removable: &[usize],
    add: usize,
    genome: &[u8],
    bounds: &[(u8, u8)],
    pool: &CardPool,
    rng: &mut SplitMix64,
) -> Option<usize> {
    choose_removal(
        removable,
        |index| {
            index != add
                && genome[index] > bounds[index].0
                && pool.cards[index].features.is_land == pool.cards[add].features.is_land
        },
        rng,
    )
}

fn choose_removal<F>(removable: &[usize], mut accepts: F, rng: &mut SplitMix64) -> Option<usize>
where
    F: FnMut(usize) -> bool,
{
    let mut chosen = None;
    let mut eligible = 0;
    for index in removable.iter().copied().filter(|index| accepts(*index)) {
        eligible += 1;
        if rng.usize(eligible) == 0 {
            chosen = Some(index);
        }
    }
    chosen
}

fn update_removable(
    removable: &mut Vec<usize>,
    remove: usize,
    add: usize,
    genome: &[u8],
    bounds: &[(u8, u8)],
) {
    if genome[remove] == bounds[remove].0
        && let Some(position) = removable.iter().position(|index| *index == remove)
    {
        removable.swap_remove(position);
    }
    if genome[add] > bounds[add].0 && !removable.contains(&add) {
        removable.push(add);
    }
}

fn update_active_addition(
    active_additions: &mut Vec<usize>,
    index: usize,
    genome: &[u8],
    bounds: &[(u8, u8)],
) {
    let eligible = genome[index] > 0 && genome[index] < bounds[index].1;
    match (
        eligible,
        active_additions
            .iter()
            .position(|current| *current == index),
    ) {
        (true, None) => active_additions.push(index),
        (false, Some(position)) => {
            active_additions.swap_remove(position);
        }
        _ => {}
    }
}

fn mutate_random(
    genome: &mut [u8],
    bounds: &[(u8, u8)],
    pool: &CardPool,
    constraints: &DeckConstraints,
    guide: &MutationGuide,
    swaps: usize,
    rng: &mut SplitMix64,
) {
    let prefer_existing_copies = constraints.default_max_copies > 1;
    let palette = genome_spell_palette(genome, pool);
    let mut removable: Vec<usize> = genome
        .iter()
        .enumerate()
        .filter_map(|(index, count)| (*count > bounds[index].0).then_some(index))
        .collect();
    let mut active_additions: Vec<usize> = genome
        .iter()
        .enumerate()
        .filter_map(|(index, count)| (*count > 0 && *count < bounds[index].1).then_some(index))
        .collect();
    let mut land_count: usize = genome
        .iter()
        .enumerate()
        .filter(|(index, _)| pool.cards[*index].features.is_land)
        .map(|(_, count)| *count as usize)
        .sum();

    for _ in 0..swaps {
        if removable.is_empty() {
            return;
        }

        let mut applied = false;
        for _ in 0..64 {
            let remove = removable[rng.usize(removable.len())];
            let add = if prefer_existing_copies
                && rng.chance(existing_copy_probability(&active_additions, genome, pool))
            {
                if active_additions.is_empty() {
                    rng.usize(genome.len())
                } else {
                    active_additions[rng.usize(active_additions.len())]
                }
            } else if !guide.embedding_neighbors[remove].is_empty() && rng.chance(0.65) {
                let neighbors = &guide.embedding_neighbors[remove];
                neighbors[rng.usize(neighbors.len())]
            } else {
                rng.usize(genome.len())
            };
            if remove == add
                || genome[add] >= bounds[add].1
                || (genome[add] > 0
                    && genome[add] >= bounds[add].0
                    && !role_copy_allowed(add, genome[add], pool, rng))
                || (genome[add] == 0 && !card_palette_compatible(&pool.cards[add], &palette))
            {
                continue;
            }
            let next_lands = land_count + usize::from(pool.cards[add].features.is_land)
                - usize::from(pool.cards[remove].features.is_land);
            if constraints
                .minimum_lands
                .is_some_and(|minimum| next_lands < minimum)
                || constraints
                    .maximum_lands
                    .is_some_and(|maximum| next_lands > maximum)
            {
                continue;
            }
            genome[remove] -= 1;
            genome[add] += 1;
            land_count = next_lands;
            update_removable(&mut removable, remove, add, genome, bounds);
            update_active_addition(&mut active_additions, remove, genome, bounds);
            update_active_addition(&mut active_additions, add, genome, bounds);
            applied = true;
            break;
        }
        if !applied {
            return;
        }
    }
}

fn tournament_select(population: &[Candidate], size: usize, rng: &mut SplitMix64) -> usize {
    let mut winner = rng.usize(population.len());
    for _ in 1..size.max(1) {
        let challenger = rng.usize(population.len());
        if fitness_order(&population[challenger].fitness, &population[winner].fitness)
            == Ordering::Greater
        {
            winner = challenger;
        }
    }
    winner
}

fn map_elite_descriptor(candidate: &Candidate, pool: &CardPool) -> MapEliteDescriptor {
    let mut colors = [false; 6];
    let mut mana_value_sum = 0.0_f64;
    let mut nonland_copies = 0_u32;
    let mut action_copies = BTreeMap::<&'static str, u32>::new();
    let mut graveyard_copies = 0_u32;

    for (index, count) in &candidate.active_cards {
        let card = &pool.cards[*index];
        if card.features.is_land {
            continue;
        }
        let copies = u32::from(*count);
        nonland_copies = nonland_copies.saturating_add(copies);
        mana_value_sum += f64::from(card.features.mana_value) * f64::from(copies);
        if card.colors.is_empty() {
            colors[5] = true;
        } else {
            for color in &card.colors {
                colors[mana_slot(*color)] = true;
            }
        }
        if card
            .features
            .tags
            .iter()
            .any(|tag| tag.contains("graveyard") || tag.contains("recursion"))
        {
            graveyard_copies = graveyard_copies.saturating_add(copies);
        }
        for ability in card
            .features
            .initial_modes
            .iter()
            .flat_map(|mode| mode.abilities.iter())
        {
            let key = match ability.kind {
                AbilityKind::AlternateWin => "alternate",
                AbilityKind::Mill => "mill",
                AbilityKind::Damage => "damage",
                AbilityKind::CreateToken | AbilityKind::Copy | AbilityKind::TokenMultiplier => {
                    "swarm"
                }
                AbilityKind::Recursion | AbilityKind::GraveyardHate => "graveyard",
                _ => continue,
            };
            *action_copies.entry(key).or_default() = action_copies
                .get(key)
                .copied()
                .unwrap_or_default()
                .saturating_add(copies);
        }
    }

    let colors = ["W", "U", "B", "R", "G", "C"]
        .into_iter()
        .enumerate()
        .filter_map(|(index, label)| colors[index].then_some(label))
        .collect::<String>();
    let mean_mana_value = mana_value_sum / f64::from(nonland_copies.max(1));
    let curve = if mean_mana_value <= 2.25 {
        "low"
    } else if mean_mana_value <= 3.50 {
        "mid"
    } else {
        "high"
    };
    let interaction = if candidate.fitness.interaction < 0.15 {
        "none"
    } else if candidate.fitness.interaction < 0.35 {
        "light"
    } else if candidate.fitness.interaction < 0.60 {
        "medium"
    } else {
        "heavy"
    };
    let archetype = if !candidate.fitness.archetype.is_empty() {
        candidate.fitness.archetype.clone()
    } else if candidate.fitness.combo_assembly_probability > 0.0 {
        "combo".into()
    } else if candidate.fitness.tempo >= 0.65 && candidate.fitness.closing_power >= 0.55 {
        "aggro".into()
    } else if candidate.fitness.interaction >= 0.55 && candidate.fitness.resilience >= 0.45 {
        "control".into()
    } else {
        "midrange".into()
    };
    let victory_path = if candidate.fitness.combo_assembly_probability > 0.0
        || !candidate.fitness.combo_proof_level.is_empty()
    {
        "combo"
    } else if action_copies.get("alternate").copied().unwrap_or_default() > 0 {
        "alternate"
    } else if action_copies.get("mill").copied().unwrap_or_default()
        >= action_copies.values().copied().max().unwrap_or_default()
        && action_copies.get("mill").copied().unwrap_or_default() > 0
    {
        "mill"
    } else if action_copies.get("swarm").copied().unwrap_or_default()
        >= action_copies.values().copied().max().unwrap_or_default()
        && action_copies.get("swarm").copied().unwrap_or_default() > 0
    {
        "tokens"
    } else if action_copies.get("damage").copied().unwrap_or_default() > 0 {
        "damage"
    } else if graveyard_copies > 0
        || action_copies.get("graveyard").copied().unwrap_or_default() > 0
    {
        "graveyard"
    } else if candidate.fitness.interaction >= 0.60 && candidate.fitness.closing_power < 0.45 {
        "control"
    } else {
        "combat"
    };

    MapEliteDescriptor {
        archetype,
        colors: if colors.is_empty() {
            "C".into()
        } else {
            colors
        },
        curve: curve.into(),
        interaction: interaction.into(),
        victory_path: victory_path.into(),
    }
}

fn update_map_elites(
    archive: &mut HashMap<MapEliteDescriptor, Candidate>,
    population: &[Candidate],
    pool: &CardPool,
    limit: usize,
) {
    for candidate in population.iter().filter(|candidate| {
        !candidate.fitness.severe_mana_failure && !candidate.fitness.severe_structural_failure
    }) {
        let descriptor = map_elite_descriptor(candidate, pool);
        let replace = archive.get(&descriptor).is_none_or(|incumbent| {
            fitness_order(&candidate.fitness, &incumbent.fitness) == Ordering::Greater
        });
        if replace {
            archive.insert(descriptor, candidate.clone());
        }
    }

    while archive.len() > limit.max(1) {
        let best = archive
            .iter()
            .max_by(|(left_descriptor, left), (right_descriptor, right)| {
                fitness_order(&left.fitness, &right.fitness)
                    .then_with(|| right_descriptor.cmp(left_descriptor))
            })
            .map(|(descriptor, _)| descriptor.clone());
        let remove = archive
            .iter()
            .filter(|(descriptor, _)| Some((*descriptor).clone()) != best)
            .min_by(|(left_descriptor, left), (right_descriptor, right)| {
                fitness_order(&left.fitness, &right.fitness)
                    .then_with(|| left_descriptor.cmp(right_descriptor))
            })
            .map(|(descriptor, _)| descriptor.clone());
        let Some(remove) = remove else { break };
        archive.remove(&remove);
    }
}

fn map_elite_seed_population(
    population: &[Candidate],
    archive: &HashMap<MapEliteDescriptor, Candidate>,
    elite_count: usize,
) -> Vec<Candidate> {
    if elite_count == 0 || population.is_empty() {
        return Vec::new();
    }
    let mut ranked_archive = archive.iter().collect::<Vec<_>>();
    ranked_archive.sort_by(|(left_descriptor, left), (right_descriptor, right)| {
        fitness_order(&right.fitness, &left.fitness)
            .then_with(|| left_descriptor.cmp(right_descriptor))
    });
    let mut selected = Vec::<Candidate>::with_capacity(elite_count);
    let mut fingerprints = HashSet::<u64>::with_capacity(elite_count);
    let mut insert = |candidate: &Candidate| {
        let fingerprint = candidate_fingerprint(candidate);
        if selected.len() < elite_count && fingerprints.insert(fingerprint) {
            selected.push(candidate.clone());
        }
    };

    // The sorted population's first entry is the global best. It is always
    // preserved before allocating the remaining elite slots to distinct niches.
    insert(&population[0]);
    for (_, candidate) in ranked_archive {
        insert(candidate);
    }
    for candidate in population {
        insert(candidate);
    }
    selected
}

fn candidate_as_ranked(
    candidate: &Candidate,
    pool: &CardPool,
    rank: usize,
    status: &str,
) -> RankedDeck {
    RankedDeck {
        rank,
        pareto_rank: 0,
        deck: genome_to_deck(
            &candidate.genome,
            &candidate.sideboard_genome,
            pool,
            &command_entries(&candidate.commanders, pool),
            format!("Evolved Niche {rank}"),
        ),
        fitness: candidate.fitness.clone(),
        packages: summarize_packages(&candidate.active_cards, pool, 8),
        commander_compatibility: (candidate.commanders.len() == 2).then(|| {
            commander_compatibility(
                &pool.cards[candidate.commanders[0]],
                &pool.cards[candidate.commanders[1]],
            )
            .expect("legal two-commander candidate must match the compatibility policy")
        }),
        candidate_status: status.into(),
    }
}

fn visible_map_elites(
    archive: &HashMap<MapEliteDescriptor, Candidate>,
    pool: &CardPool,
    limit: usize,
) -> Vec<MapEliteCell> {
    let mut cells = archive.iter().collect::<Vec<_>>();
    cells.sort_by(|(left_descriptor, left), (right_descriptor, right)| {
        fitness_order(&right.fitness, &left.fitness)
            .then_with(|| left_descriptor.cmp(right_descriptor))
    });
    cells
        .into_iter()
        .take(limit)
        .enumerate()
        .map(|(index, (descriptor, candidate))| MapEliteCell {
            descriptor: descriptor.clone(),
            candidate: candidate_as_ranked(candidate, pool, index + 1, "niche_elite"),
        })
        .collect()
}

fn ranked_decks(population: &[Candidate], pool: &CardPool, limit: usize) -> Vec<RankedDeck> {
    let candidate_indices = group_identical_candidates(population)
        .into_iter()
        .map(|group| group.representative)
        .filter(|index| {
            !population[*index].fitness.severe_mana_failure
                && !population[*index].fitness.severe_structural_failure
        })
        .collect::<Vec<_>>();
    let pareto_ranks = nondominated_ranks_for_indices(population, &candidate_indices);
    let mut order = (0..candidate_indices.len()).collect::<Vec<_>>();
    order.sort_by(|left, right| {
        population[candidate_indices[*right]]
            .fitness
            .score
            .total_cmp(&population[candidate_indices[*left]].fitness.score)
    });
    let minimum_score = order
        .iter()
        .map(|position| population[candidate_indices[*position]].fitness.score)
        .fold(f64::INFINITY, f64::min);
    let maximum_score = order
        .iter()
        .map(|position| population[candidate_indices[*position]].fitness.score)
        .fold(f64::NEG_INFINITY, f64::max);
    let mut selected = Vec::new();
    if let Some(best) = order.first().copied() {
        selected.push(best);
    }
    while selected.len() < limit && selected.len() < order.len() {
        let require_distance = order.len() >= limit;
        let pick = order
            .iter()
            .copied()
            .filter(|position| !selected.contains(position))
            .filter_map(|position| {
                let candidate = &population[candidate_indices[position]];
                let distance = selected
                    .iter()
                    .map(|chosen| {
                        copy_edit_distance_nonlands(
                            &candidate.genome,
                            &population[candidate_indices[*chosen]].genome,
                            pool,
                        )
                    })
                    .min()
                    .unwrap_or(0);
                let deck_size = candidate
                    .genome
                    .iter()
                    .map(|count| usize::from(*count))
                    .sum::<usize>();
                let minimum_distance = 8_usize.max(deck_size.div_ceil(5));
                let meaningfully_distinct = selected.iter().all(|chosen| {
                    descriptor_differs(
                        &candidate.genome,
                        &population[candidate_indices[*chosen]].genome,
                        pool,
                    )
                });
                if require_distance && (distance < minimum_distance || !meaningfully_distinct) {
                    return None;
                }
                let normalized_fitness = if (maximum_score - minimum_score).abs() < f64::EPSILON {
                    1.0
                } else {
                    (candidate.fitness.score - minimum_score) / (maximum_score - minimum_score)
                };
                let normalized_distance = (distance as f64 / deck_size.max(1) as f64).min(1.0);
                Some((
                    position,
                    normalized_fitness * 0.75 + normalized_distance * 0.25,
                ))
            })
            .max_by(|left, right| left.1.total_cmp(&right.1))
            .map(|(position, _)| position)
            .or_else(|| {
                (!require_distance)
                    .then(|| {
                        order
                            .iter()
                            .copied()
                            .find(|position| !selected.contains(position))
                    })
                    .flatten()
            });
        let Some(pick) = pick else { break };
        selected.push(pick);
    }
    selected
        .into_iter()
        .take(limit)
        .enumerate()
        .map(|(rank, candidate_position)| {
            let candidate_index = candidate_indices[candidate_position];
            let candidate = &population[candidate_index];
            RankedDeck {
                rank: rank + 1,
                pareto_rank: pareto_ranks[candidate_position],
                deck: genome_to_deck(
                    &candidate.genome,
                    &candidate.sideboard_genome,
                    pool,
                    &command_entries(&candidate.commanders, pool),
                    format!("Evolved Candidate {}", rank + 1),
                ),
                fitness: candidate.fitness.clone(),
                packages: summarize_packages(&candidate.active_cards, pool, 8),
                commander_compatibility: (candidate.commanders.len() == 2).then(|| {
                    commander_compatibility(
                        &pool.cards[candidate.commanders[0]],
                        &pool.cards[candidate.commanders[1]],
                    )
                    .expect("legal two-commander candidate must match the compatibility policy")
                }),
                candidate_status: "provisional".into(),
            }
        })
        .collect()
}

fn copy_edit_distance_nonlands(left: &[u8], right: &[u8], pool: &CardPool) -> usize {
    left.iter()
        .zip(right)
        .enumerate()
        .filter(|(index, _)| !pool.cards[*index].features.is_land)
        .map(|(_, (left, right))| usize::from(left.abs_diff(*right)))
        .sum::<usize>()
        .div_ceil(2)
}

fn descriptor_differs(left: &[u8], right: &[u8], pool: &CardPool) -> bool {
    let descriptor = |genome: &[u8]| {
        let mut packages = HashSet::<&'static str>::new();
        let mut curve = [0_u16; 4];
        let mut colors = [false; 5];
        for (index, count) in genome
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, count)| *count > 0)
        {
            let card = &pool.cards[index];
            if card.features.is_land {
                continue;
            }
            packages.extend(strategy_keys(card));
            let band = match card.features.mana_value.ceil() as usize {
                0..=1 => 0,
                2 => 1,
                3 => 2,
                _ => 3,
            };
            curve[band] = curve[band].saturating_add(u16::from(count));
            for color in &card.colors {
                if let Some(slot) = color_slot(*color) {
                    colors[slot] = true;
                }
            }
        }
        (packages, curve, colors)
    };
    let (left_packages, left_curve, left_colors) = descriptor(left);
    let (right_packages, right_curve, right_colors) = descriptor(right);
    let curve_distance = left_curve
        .iter()
        .zip(right_curve)
        .map(|(left, right)| left.abs_diff(right))
        .sum::<u16>()
        / 2;
    left_packages != right_packages || left_colors != right_colors || curve_distance >= 3
}

fn is_package_tag(tag: &str) -> bool {
    !matches!(tag, "creature" | "artifact" | "enchantment")
}

fn summarize_packages(
    active_cards: &[(usize, u8)],
    pool: &CardPool,
    limit: usize,
) -> Vec<PackageSummary> {
    let mut totals = BTreeMap::<&str, (usize, usize)>::new();
    for (index, count) in active_cards {
        let mut seen = HashSet::new();
        for tag in &pool.cards[*index].features.tags {
            if is_package_tag(tag) && seen.insert(tag.as_str()) {
                let total = totals.entry(tag).or_default();
                total.0 += *count as usize;
                total.1 += 1;
            }
        }
    }
    let mut packages = totals
        .into_iter()
        .filter(|(_, (_, unique_cards))| *unique_cards >= 2)
        .map(|(tag, (copies, unique_cards))| PackageSummary {
            tag: tag.into(),
            copies,
            unique_cards,
        })
        .collect::<Vec<_>>();
    packages.sort_by(|left, right| {
        right
            .copies
            .cmp(&left.copies)
            .then_with(|| right.unique_cards.cmp(&left.unique_cards))
            .then_with(|| left.tag.cmp(&right.tag))
    });
    packages.truncate(limit);
    packages
}

fn dominates(left: &Fitness, right: &Fitness) -> bool {
    let left_rewards = [
        left.consistency,
        left.tempo,
        left.synergy,
        left.interaction,
        left.resilience,
        left.commander_quality,
        left.sideboard_quality,
    ];
    let right_rewards = [
        right.consistency,
        right.tempo,
        right.synergy,
        right.interaction,
        right.resilience,
        right.commander_quality,
        right.sideboard_quality,
    ];
    let no_worse = left_rewards
        .iter()
        .zip(right_rewards.iter())
        .all(|(left, right)| left >= right)
        && left.mana_failure_rate <= right.mana_failure_rate;
    let strictly_better = left_rewards
        .iter()
        .zip(right_rewards.iter())
        .any(|(left, right)| left > right)
        || left.mana_failure_rate < right.mana_failure_rate;
    no_worse && strictly_better
}

fn nondominated_ranks_for_indices(population: &[Candidate], indices: &[usize]) -> Vec<usize> {
    let count = indices.len();
    let mut dominates_sets = vec![Vec::new(); count];
    let mut domination_counts = vec![0usize; count];
    for left in 0..count {
        for right in (left + 1)..count {
            if dominates(
                &population[indices[left]].fitness,
                &population[indices[right]].fitness,
            ) {
                dominates_sets[left].push(right);
                domination_counts[right] += 1;
            } else if dominates(
                &population[indices[right]].fitness,
                &population[indices[left]].fitness,
            ) {
                dominates_sets[right].push(left);
                domination_counts[left] += 1;
            }
        }
    }
    let mut ranks = vec![usize::MAX; count];
    let mut frontier = domination_counts
        .iter()
        .enumerate()
        .filter_map(|(index, dominated)| (*dominated == 0).then_some(index))
        .collect::<Vec<_>>();
    let mut rank = 0usize;
    while !frontier.is_empty() {
        let mut next = Vec::new();
        for index in frontier {
            ranks[index] = rank;
            for dominated in &dominates_sets[index] {
                domination_counts[*dominated] = domination_counts[*dominated].saturating_sub(1);
                if domination_counts[*dominated] == 0 {
                    next.push(*dominated);
                }
            }
        }
        frontier = next;
        rank += 1;
    }
    ranks
}

fn fitness_order(left: &Fitness, right: &Fitness) -> Ordering {
    left.score.total_cmp(&right.score)
}

fn generation_seed(root: u64, generation: usize, trial: usize) -> u64 {
    root ^ (generation as u64).wrapping_mul(0xD1B5_4A32_D192_ED03)
        ^ (trial as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AbilityProfile, Card, CardFeatures, CardId, CardMode, Color, LockedCard, ManaCostProfile,
    };
    use std::collections::BTreeMap;

    fn card(name: &str, land: bool, mana_value: f32, tag: &str) -> Card {
        Card {
            id: CardId(name.to_lowercase().replace(' ', "-")),
            arena_id: Some(1),
            platforms: vec!["arena".into(), "paper".into(), "mtgo".into()],
            name: name.into(),
            set_code: "tst".into(),
            collector_number: "1".into(),
            type_line: if land { "Land" } else { "Spell" }.into(),
            oracle_text: String::new(),
            colors: vec![Color::Green],
            color_identity: vec![Color::Green],
            image_uri: None,
            legalities: BTreeMap::new(),
            features: CardFeatures {
                is_land: land,
                unlimited_copies: land,
                mana_value,
                mana_production: if land { 1.0 } else { 0.0 },
                pressure: if land { 0.0 } else { 1.0 },
                tags: if tag.is_empty() {
                    vec![]
                } else {
                    vec![tag.into()]
                },
                ..CardFeatures::default()
            },
        }
    }

    fn functional_card(
        name: &str,
        kind: AbilityKind,
        timing: crate::AbilityTiming,
        effect_value: f32,
    ) -> Card {
        let mut result = card(name, false, 2.0, "");
        result.type_line = "Creature — Test".into();
        result.features.initial_modes = vec![CardMode {
            name: name.into(),
            type_line: result.type_line.clone(),
            mana_cost: ManaCostProfile {
                generic: 2,
                ..Default::default()
            },
            abilities: vec![AbilityProfile {
                label: format!("{name} action"),
                kind,
                timing,
                effect_value,
                parser_confidence: 1.0,
                ..Default::default()
            }],
            ..Default::default()
        }];
        result
    }

    fn niche_candidate(
        genome: Vec<u8>,
        active_cards: Vec<(usize, u8)>,
        fitness: Fitness,
    ) -> Candidate {
        Candidate {
            genome,
            active_cards,
            sideboard_genome: vec![0; 2],
            active_sideboard: Vec::new(),
            commanders: Vec::new(),
            fitness,
        }
    }

    #[test]
    fn map_elites_preserve_distinct_victory_niches_and_the_global_best() {
        let mut red = functional_card(
            "Red pressure",
            AbilityKind::Damage,
            crate::AbilityTiming::Triggered,
            1.0,
        );
        red.colors = vec![Color::Red];
        red.features.mana_value = 1.0;
        let mut blue = functional_card(
            "Blue mill",
            AbilityKind::Mill,
            crate::AbilityTiming::Activated,
            1.0,
        );
        blue.colors = vec![Color::Blue];
        blue.features.mana_value = 5.0;
        let pool = CardPool {
            cards: vec![red, blue],
        };
        let global_best = niche_candidate(
            vec![4, 0],
            vec![(0, 4)],
            Fitness {
                score: 0.90,
                archetype: "aggro".into(),
                tempo: 0.8,
                closing_power: 0.8,
                interaction: 0.1,
                ..Default::default()
            },
        );
        let distinct_niche = niche_candidate(
            vec![0, 4],
            vec![(1, 4)],
            Fitness {
                score: 0.55,
                archetype: "control".into(),
                interaction: 0.7,
                closing_power: 0.3,
                ..Default::default()
            },
        );
        let mut archive = HashMap::new();
        update_map_elites(
            &mut archive,
            &[global_best.clone(), distinct_niche.clone()],
            &pool,
            8,
        );
        assert_eq!(archive.len(), 2);
        assert!(
            archive
                .keys()
                .any(|descriptor| descriptor.victory_path == "damage")
        );
        assert!(
            archive
                .keys()
                .any(|descriptor| descriptor.victory_path == "mill")
        );

        let seeded =
            map_elite_seed_population(&[global_best.clone(), distinct_niche.clone()], &archive, 2);
        assert_eq!(seeded.len(), 2);
        assert_eq!(
            candidate_fingerprint(&seeded[0]),
            candidate_fingerprint(&global_best)
        );
        assert!(
            seeded
                .iter()
                .any(|candidate| candidate.active_cards == distinct_niche.active_cards)
        );

        update_map_elites(
            &mut archive,
            &[global_best.clone(), distinct_niche],
            &pool,
            1,
        );
        assert_eq!(archive.len(), 1);
        assert_eq!(
            archive.values().next().unwrap().fitness.score,
            global_best.fitness.score
        );
    }

    #[test]
    fn evolution_rejects_multiple_printings_of_the_same_logical_card() {
        let first = card("Reprinted Spell", false, 2.0, "value");
        let mut second = first.clone();
        second.id = CardId("different-set-printing".into());
        second.set_code = "alt".into();
        let error = validate_inputs(
            &Deck {
                name: "Blank".into(),
                command_zone: Vec::new(),
                mainboard: Vec::new(),
                sideboard: Vec::new(),
            },
            &CardPool {
                cards: vec![first, second],
            },
            &DeckConstraints::default(),
            &EvolutionConfig {
                population_size: 2,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("duplicate printings"));
        assert!(error.to_string().contains("Reprinted Spell"));
    }

    #[test]
    fn tune_signature_protects_functional_copy_token_and_blink_packages() {
        let pool = CardPool {
            cards: vec![
                functional_card(
                    "Copy A",
                    AbilityKind::Copy,
                    crate::AbilityTiming::Spell,
                    1.0,
                ),
                functional_card(
                    "Copy B",
                    AbilityKind::Copy,
                    crate::AbilityTiming::Spell,
                    1.0,
                ),
                functional_card(
                    "Token A",
                    AbilityKind::CreateToken,
                    crate::AbilityTiming::Triggered,
                    1.0,
                ),
                functional_card(
                    "Token B",
                    AbilityKind::TokenMultiplier,
                    crate::AbilityTiming::Static,
                    1.0,
                ),
                functional_card(
                    "Blink A",
                    AbilityKind::Blink,
                    crate::AbilityTiming::Activated,
                    1.0,
                ),
                functional_card(
                    "ETB A",
                    AbilityKind::Draw,
                    crate::AbilityTiming::Triggered,
                    1.0,
                ),
                functional_card(
                    "Artifact shell",
                    AbilityKind::Draw,
                    crate::AbilityTiming::Activated,
                    0.5,
                ),
            ],
        };
        let root = vec![(0, 4), (1, 4), (2, 4), (3, 4), (4, 4), (5, 4)];
        let signature = derive_strategy_signature(&root, &pool);
        for package in ["copy", "tokens", "blink-etb"] {
            assert!(
                signature
                    .packages
                    .iter()
                    .any(|entry| entry.package == package && entry.primary)
            );
        }

        let artifact_shell = vec![(6, 24)];
        let mut shell_fitness = Fitness::default();
        apply_tune_policy(
            &mut shell_fitness,
            &artifact_shell,
            &root,
            &signature,
            &TunePolicy::default(),
            &pool,
            false,
        );
        assert!(shell_fitness.severe_structural_failure);
        assert!(shell_fitness.strategy_similarity < 0.70);

        let producer_only = vec![(0, 4), (1, 4), (2, 8), (4, 4), (5, 4)];
        let mut producer_only_fitness = Fitness::default();
        apply_tune_policy(
            &mut producer_only_fitness,
            &producer_only,
            &root,
            &signature,
            &TunePolicy::default(),
            &pool,
            false,
        );
        let token_package = producer_only_fitness
            .strategy_packages
            .iter()
            .find(|package| package.package == "tokens")
            .expect("token package evidence");
        assert_eq!(token_package.retention, 0.5);
        assert!(producer_only_fitness.severe_structural_failure);
        assert!(
            producer_only_fitness
                .structural_blocking_reasons
                .iter()
                .any(|reason| reason.contains("Primary tokens package"))
        );

        let mut baseline = Fitness::default();
        apply_tune_policy(
            &mut baseline,
            &root,
            &root,
            &signature,
            &TunePolicy::default(),
            &pool,
            true,
        );
        assert_eq!(baseline.strategy_similarity, 1.0);
        assert_eq!(baseline.nonland_edit_fraction, 0.0);
        assert!(!baseline.severe_structural_failure);
    }

    #[test]
    fn decks_without_engines_receive_role_curve_and_color_fallback_signatures() {
        let pool = CardPool {
            cards: vec![
                card("Early threat", false, 1.0, ""),
                card("Middle threat", false, 3.0, ""),
            ],
        };
        let signature = derive_strategy_signature(&[(0, 4), (1, 4)], &pool);
        assert!(!signature.packages.iter().any(|package| package.primary));
        assert!(
            signature
                .packages
                .iter()
                .any(|package| package.package == "role:threat")
        );
        assert!(
            signature
                .packages
                .iter()
                .any(|package| package.package == "curve:early")
        );
    }

    #[test]
    fn generic_tag_overlap_cannot_invent_a_primary_strategy_package() {
        let pool = CardPool {
            cards: vec![
                card("Tagged copy one", false, 2.0, "copy"),
                card("Tagged copy two", false, 3.0, "copy"),
            ],
        };
        let signature = derive_strategy_signature(&[(0, 4), (1, 4)], &pool);
        assert!(
            signature
                .packages
                .iter()
                .any(|package| package.package == "copy")
        );
        assert!(!signature.packages.iter().any(|package| package.primary));
    }

    #[test]
    fn exact_palette_bounds_allow_zero_one_or_multiple_colored_colors() {
        let green = card("Green spell", false, 2.0, "engine");
        let mut blue = card("Blue spell", false, 2.0, "engine");
        blue.colors = vec![Color::Blue];
        blue.color_identity = vec![Color::Blue];
        let mut colorless = card("Colorless spell", false, 2.0, "engine");
        colorless.colors.clear();
        colorless.color_identity.clear();
        let pool = CardPool {
            cards: vec![green, blue, colorless],
        };

        let mut constraints = DeckConstraints {
            allowed_colors: Some(vec![Color::Colorless]),
            ..DeckConstraints::default()
        };
        assert_eq!(
            copy_bounds(&pool, &constraints)
                .iter()
                .map(|row| row.1)
                .collect::<Vec<_>>(),
            vec![0, 0, 4]
        );

        constraints.allowed_colors = Some(vec![Color::Green]);
        assert_eq!(
            copy_bounds(&pool, &constraints)
                .iter()
                .map(|row| row.1)
                .collect::<Vec<_>>(),
            vec![4, 0, 0]
        );

        constraints.allowed_colors = Some(vec![Color::Blue, Color::Colorless]);
        assert_eq!(
            copy_bounds(&pool, &constraints)
                .iter()
                .map(|row| row.1)
                .collect::<Vec<_>>(),
            vec![0, 4, 4]
        );
        let mut rng = SplitMix64::new(7);
        assert_eq!(
            seed_color_palette(&pool, &constraints, &[0, 0, 0], &mut rng),
            HashSet::from([Color::Blue, Color::Colorless])
        );
    }

    fn commander(name: &str, color: Color) -> Card {
        let mut card = card(name, false, 3.0, "commander");
        card.type_line = "Legendary Creature — Test".into();
        card.colors = vec![color];
        card.color_identity = vec![color];
        card
    }

    #[test]
    fn evolution_preserves_size_copy_and_lock_constraints() {
        let pool = CardPool {
            cards: vec![
                card("Forest", true, 0.0, "mana"),
                card("Island", true, 0.0, "mana"),
                card("Engine", false, 4.0, "copy"),
                card("Payoff", false, 3.0, "copy"),
                card("Removal", false, 2.0, "interaction"),
                card("Draw", false, 2.0, "velocity"),
            ],
        };
        let seed = Deck {
            name: "Seed".into(),
            command_zone: vec![],
            mainboard: vec![
                DeckEntry {
                    card_id: CardId("forest".into()),
                    quantity: 12,
                },
                DeckEntry {
                    card_id: CardId("island".into()),
                    quantity: 12,
                },
                DeckEntry {
                    card_id: CardId("engine".into()),
                    quantity: 4,
                },
                DeckEntry {
                    card_id: CardId("payoff".into()),
                    quantity: 4,
                },
                DeckEntry {
                    card_id: CardId("removal".into()),
                    quantity: 4,
                },
                DeckEntry {
                    card_id: CardId("draw".into()),
                    quantity: 4,
                },
            ],
            sideboard: vec![],
        };
        let constraints = DeckConstraints {
            target_mainboard_size: 40,
            target_sideboard_size: 0,
            minimum_commanders: 0,
            maximum_commanders: 0,
            default_max_copies: 12,
            locked_cards: vec![LockedCard {
                card_id: CardId("engine".into()),
                minimum: 4,
                maximum: 4,
            }],
            minimum_lands: Some(16),
            maximum_lands: Some(26),
            allowed_colors: None,
        };
        let config = EvolutionConfig {
            population_size: 16,
            generations: 4,
            elite_count: 2,
            trials_per_candidate: 16,
            mutation_swaps: 2,
            ..EvolutionConfig::default()
        };
        let result = EvolutionEngine::default()
            .run(
                &seed,
                &pool,
                &constraints,
                &config,
                &CancellationToken::default(),
                |_| {},
            )
            .unwrap();

        assert_eq!(result.generations_completed, 4);
        for candidate in result.frontier {
            assert_eq!(candidate.deck.mainboard_count(), 40);
            let engine = candidate
                .deck
                .mainboard
                .iter()
                .find(|entry| entry.card_id.0 == "engine")
                .unwrap();
            assert_eq!(engine.quantity, 4);
        }
    }

    #[test]
    fn same_seed_is_reproducible() {
        let mut rng_a = SplitMix64::new(42);
        let mut rng_b = SplitMix64::new(42);
        let a: Vec<_> = (0..100).map(|_| rng_a.next_u64()).collect();
        let b: Vec<_> = (0..100).map(|_| rng_b.next_u64()).collect();
        assert_eq!(a, b);
    }

    #[test]
    fn duplicate_groups_require_exact_genomes_and_ignore_commander_order() {
        let candidate = |active_cards, commanders| Candidate {
            genome: Vec::new(),
            active_cards,
            sideboard_genome: Vec::new(),
            active_sideboard: Vec::new(),
            commanders,
            fitness: Fitness::default(),
        };
        let population = vec![
            candidate(vec![(1, 2), (4, 1)], vec![7, 9]),
            candidate(vec![(1, 2), (4, 1)], vec![9, 7]),
            candidate(vec![(1, 1), (4, 2)], vec![7, 9]),
            candidate(vec![(1, 2), (4, 1)], vec![7]),
            Candidate {
                genome: Vec::new(),
                active_cards: vec![(1, 2), (4, 1)],
                sideboard_genome: Vec::new(),
                active_sideboard: vec![(5, 1)],
                commanders: vec![7, 9],
                fitness: Fitness::default(),
            },
        ];
        let groups = group_identical_candidates(&population);
        assert_eq!(groups.len(), 4);
        assert_eq!(groups[0].members, vec![0, 1]);
        assert_eq!(groups[1].members, vec![2]);
        assert_eq!(groups[2].members, vec![3]);
        assert_eq!(groups[3].members, vec![4]);
    }

    #[test]
    fn exact_duplicates_share_evaluation_and_frontier_slots() {
        let pool = CardPool {
            cards: vec![
                card("Forest", true, 0.0, "mana"),
                card("Reachable threat", false, 1.0, "threat"),
            ],
        };
        let seed = Deck {
            name: "Locked seed".into(),
            command_zone: vec![],
            mainboard: vec![
                DeckEntry {
                    card_id: CardId("forest".into()),
                    quantity: 59,
                },
                DeckEntry {
                    card_id: CardId("reachable-threat".into()),
                    quantity: 1,
                },
            ],
            sideboard: vec![],
        };
        let constraints = DeckConstraints {
            target_mainboard_size: 60,
            target_sideboard_size: 0,
            minimum_commanders: 0,
            maximum_commanders: 0,
            default_max_copies: 4,
            locked_cards: vec![
                LockedCard {
                    card_id: CardId("forest".into()),
                    minimum: 59,
                    maximum: 59,
                },
                LockedCard {
                    card_id: CardId("reachable-threat".into()),
                    minimum: 1,
                    maximum: 1,
                },
            ],
            minimum_lands: Some(59),
            maximum_lands: Some(59),
            allowed_colors: None,
        };
        let config = EvolutionConfig {
            population_size: 8,
            generations: 3,
            elite_count: 1,
            trials_per_candidate: 2,
            mutation_rate: 1.0,
            mutation_swaps: 2,
            ..EvolutionConfig::default()
        };
        let mut progress = Vec::new();
        let result = EvolutionEngine::default()
            .run(
                &seed,
                &pool,
                &constraints,
                &config,
                &CancellationToken::default(),
                |update| progress.push(update),
            )
            .unwrap();

        assert_eq!(result.evaluated_candidates, 24);
        assert_eq!(result.unique_evaluations, 3);
        assert_eq!(result.evaluations_avoided, 21);
        assert_eq!(result.frontier.len(), 1);
        assert_eq!(progress.len(), 3);
        assert!(progress.iter().all(|update| {
            update.unique_in_generation == 1
                && update.duplicates_in_generation == 7
                && update.top.len() == 1
        }));
    }

    #[test]
    fn evolves_a_full_sideboard_with_shared_copy_limits() {
        let mut cards = vec![card("Forest", true, 0.0, "mana")];
        cards.extend((0..20).map(|index| {
            card(
                &format!("Spell {index}"),
                false,
                1.0 + (index % 4) as f32,
                if index % 2 == 0 {
                    "interaction"
                } else {
                    "resilience"
                },
            )
        }));
        let pool = CardPool { cards };
        let seed = Deck {
            name: "Best of three seed".into(),
            command_zone: vec![],
            mainboard: std::iter::once(DeckEntry {
                card_id: CardId("forest".into()),
                quantity: 24,
            })
            .chain((0..9).map(|index| DeckEntry {
                card_id: CardId(format!("spell-{index}")),
                quantity: 4,
            }))
            .collect(),
            sideboard: vec![],
        };
        let constraints = DeckConstraints {
            target_mainboard_size: 60,
            target_sideboard_size: 15,
            minimum_commanders: 0,
            maximum_commanders: 0,
            default_max_copies: 4,
            locked_cards: vec![],
            minimum_lands: Some(20),
            maximum_lands: Some(28),
            allowed_colors: None,
        };
        let config = EvolutionConfig {
            population_size: 16,
            generations: 4,
            elite_count: 2,
            trials_per_candidate: 8,
            mutation_swaps: 2,
            evolve_sideboard: true,
            ..EvolutionConfig::default()
        };
        let result = EvolutionEngine::default()
            .run(
                &seed,
                &pool,
                &constraints,
                &config,
                &CancellationToken::default(),
                |_| {},
            )
            .unwrap();

        assert!(!result.frontier.is_empty());
        for candidate in &result.frontier {
            assert_eq!(candidate.deck.mainboard_count(), 60);
            assert_eq!(candidate.deck.sideboard_count(), 15);
            assert!(candidate.fitness.sideboard_quality > 0.0);
            validate_deck_legality(&candidate.deck, &pool, &constraints).unwrap();
        }
    }

    #[test]
    fn preserves_a_fixed_imported_sideboard_during_mainboard_evolution() {
        let mut cards = vec![card("Forest", true, 0.0, "mana")];
        cards.extend((0..16).map(|index| card(&format!("Spell {index}"), false, 2.0, "tempo")));
        let pool = CardPool { cards };
        let fixed_sideboard = vec![
            DeckEntry {
                card_id: CardId("spell-9".into()),
                quantity: 4,
            },
            DeckEntry {
                card_id: CardId("spell-10".into()),
                quantity: 4,
            },
            DeckEntry {
                card_id: CardId("spell-11".into()),
                quantity: 4,
            },
            DeckEntry {
                card_id: CardId("spell-12".into()),
                quantity: 3,
            },
        ];
        let seed = Deck {
            name: "Fixed sideboard".into(),
            command_zone: vec![],
            mainboard: std::iter::once(DeckEntry {
                card_id: CardId("forest".into()),
                quantity: 24,
            })
            .chain((0..9).map(|index| DeckEntry {
                card_id: CardId(format!("spell-{index}")),
                quantity: 4,
            }))
            .collect(),
            sideboard: fixed_sideboard.clone(),
        };
        let constraints = DeckConstraints {
            target_mainboard_size: 60,
            target_sideboard_size: 15,
            minimum_commanders: 0,
            maximum_commanders: 0,
            default_max_copies: 4,
            locked_cards: vec![],
            minimum_lands: Some(20),
            maximum_lands: Some(28),
            allowed_colors: None,
        };
        let result = EvolutionEngine::default()
            .run(
                &seed,
                &pool,
                &constraints,
                &EvolutionConfig {
                    population_size: 12,
                    generations: 3,
                    elite_count: 2,
                    trials_per_candidate: 8,
                    evolve_sideboard: false,
                    ..EvolutionConfig::default()
                },
                &CancellationToken::default(),
                |_| {},
            )
            .unwrap();

        for candidate in &result.frontier {
            assert_eq!(candidate.deck.sideboard, fixed_sideboard);
            validate_deck_legality(&candidate.deck, &pool, &constraints).unwrap();
        }
    }

    #[test]
    fn package_mutation_adds_a_correlated_tag_group_and_preserves_invariants() {
        let pool = CardPool {
            cards: vec![
                card("Forest", true, 0.0, "mana"),
                card("Engine A", false, 2.0, "engine"),
                card("Engine B", false, 2.0, "engine"),
                card("Engine C", false, 3.0, "engine"),
                card("Engine D", false, 4.0, "engine"),
                card("Filler A", false, 2.0, ""),
                card("Filler B", false, 2.0, ""),
                card("Filler C", false, 2.0, ""),
            ],
        };
        let mut genome = vec![20, 0, 0, 0, 0, 4, 4, 4];
        let bounds = vec![
            (0, 40),
            (0, 4),
            (0, 4),
            (0, 4),
            (0, 4),
            (0, 4),
            (0, 4),
            (0, 4),
        ];
        let guide = MutationGuide::new(&pool, &[], &[]);
        let before_total = genome.iter().map(|count| *count as usize).sum::<usize>();
        let before_lands = genome[0];
        let applied = mutate_package(
            &mut genome,
            &bounds,
            &pool,
            &guide,
            3,
            true,
            &mut SplitMix64::new(42),
        );

        assert_eq!(applied, 3);
        assert_eq!(genome[1..=4].iter().sum::<u8>(), 3);
        assert_eq!(genome[1..=4].iter().filter(|count| **count > 0).count(), 3);
        assert_eq!(genome[0], before_lands);
        assert_eq!(
            genome.iter().map(|count| *count as usize).sum::<usize>(),
            before_total
        );
        assert!(
            genome
                .iter()
                .zip(&bounds)
                .all(|(count, (minimum, maximum))| count >= minimum && count <= maximum)
        );
        let packages = summarize_packages(&compact_genome(&genome), &pool, 8);
        assert_eq!(packages[0].tag, "engine");
        assert_eq!(packages[0].copies, 3);
        assert_eq!(packages[0].unique_cards, 3);
    }

    #[test]
    fn learned_packages_extend_the_mutation_guide_without_hand_authored_tags() {
        let pool = CardPool {
            cards: vec![
                card("Forest", true, 0.0, "mana"),
                card("Learned A", false, 2.0, ""),
                card("Learned B", false, 3.0, ""),
            ],
        };
        let guide = MutationGuide::new(
            &pool,
            &[vec![CardId("learned-a".into()), CardId("learned-b".into())]],
            &[],
        );
        assert!(guide.package_groups.contains(&vec![1, 2]));
    }

    #[test]
    fn typed_embeddings_offer_coherent_substitutions_without_shared_tags() {
        let first = functional_card(
            "Damage A",
            AbilityKind::Damage,
            crate::AbilityTiming::Spell,
            1.0,
        );
        let second = functional_card(
            "Damage B",
            AbilityKind::Damage,
            crate::AbilityTiming::Spell,
            1.0,
        );
        let mill = functional_card(
            "Mill C",
            AbilityKind::Mill,
            crate::AbilityTiming::Spell,
            1.0,
        );
        let pool = CardPool {
            cards: vec![first, second, mill],
        };
        let guide = MutationGuide::new(&pool, &[], &[]);
        assert!(guide.embedding_neighbors[0].contains(&1));
        assert!(!guide.embedding_neighbors[0].contains(&2));
    }

    #[test]
    fn combo_packages_score_assembly_and_accept_functional_piece_replacements() {
        let original_mana = functional_card(
            "Original Mana",
            AbilityKind::Ramp,
            crate::AbilityTiming::Activated,
            2.0,
        );
        let mut original_untap = functional_card(
            "Original Untap",
            AbilityKind::Unknown,
            crate::AbilityTiming::Activated,
            1.0,
        );
        original_untap.features.initial_modes[0].abilities[0].label =
            "untap target artifact".into();
        let original_damage = functional_card(
            "Original Damage",
            AbilityKind::Damage,
            crate::AbilityTiming::Activated,
            1.0,
        );
        let alternate_mana = functional_card(
            "Alternate Mana",
            AbilityKind::Ramp,
            crate::AbilityTiming::Activated,
            2.0,
        );
        let mut alternate_untap = functional_card(
            "Alternate Untap",
            AbilityKind::Unknown,
            crate::AbilityTiming::Activated,
            1.0,
        );
        alternate_untap.features.initial_modes[0].abilities[0].label =
            "untap target artifact".into();
        let alternate_damage = functional_card(
            "Alternate Damage",
            AbilityKind::Damage,
            crate::AbilityTiming::Activated,
            1.0,
        );
        let profile = ComboPackageProfile {
            id: "loop".into(),
            piece_ids: vec![
                original_mana.id.clone(),
                original_untap.id.clone(),
                original_damage.id.clone(),
            ],
            required_roles: vec!["mana".into(), "untap".into(), "damage".into()],
            expected_turn: Some(3.0),
            repeatable: true,
            winning: true,
            proof: "symbolically_proven".into(),
            ..ComboPackageProfile::default()
        };
        let pool = CardPool {
            cards: vec![
                original_mana,
                original_untap,
                original_damage,
                alternate_mana,
                alternate_untap,
                alternate_damage,
            ],
        };
        let root = vec![(0, 4), (1, 4), (2, 4)];
        let candidate = vec![(3, 4), (4, 4), (5, 4)];
        let mut signature = derive_strategy_signature(&root, &pool);
        augment_combo_signature(&mut signature, &root, &pool, std::slice::from_ref(&profile));
        let mut fitness = Fitness::default();
        apply_tune_policy(
            &mut fitness,
            &candidate,
            &root,
            &signature,
            &TunePolicy {
                strategy_retention_floor: 0.0,
                maximum_nonland_edit_fraction: 1.0,
                ..TunePolicy::default()
            },
            &pool,
            false,
        );
        let combo = fitness
            .strategy_packages
            .iter()
            .find(|package| package.package == "combo:loop")
            .expect("causal combo signature");
        assert_eq!(combo.retention, 1.0);

        apply_combo_package_evidence(
            &mut fitness,
            &candidate,
            &pool,
            std::slice::from_ref(&profile),
            6,
        );
        assert!(fitness.combo_assembly_probability > 0.0);
        assert!(fitness.combo_execution_rate > 0.0);
        assert!(fitness.closing_power > 0.0);

        let guide = MutationGuide::new(&pool, &[], &[profile]);
        assert!(
            guide
                .package_groups
                .iter()
                .any(|group| group.contains(&3) && group.contains(&4) && group.contains(&5))
        );
    }

    #[test]
    fn mana_mutation_replaces_off_color_sources_using_curve_weighted_demand() {
        let mut forest = card("Forest", true, 0.0, "mana");
        forest.color_identity = vec![Color::Green];
        let mut island = card("Island", true, 0.0, "mana");
        island.color_identity = vec![Color::Blue];
        let mut blue_spell = card("Blue Payoff", false, 5.0, "engine");
        blue_spell.colors = vec![Color::Blue];
        blue_spell.color_identity = vec![Color::Blue];
        let pool = CardPool {
            cards: vec![forest, island, blue_spell],
        };
        let mut genome = vec![8, 0, 52];
        let bounds = vec![(0, 60), (0, 60), (0, 60)];
        let constraints = DeckConstraints {
            target_mainboard_size: 60,
            target_sideboard_size: 0,
            minimum_commanders: 0,
            maximum_commanders: 0,
            default_max_copies: 60,
            locked_cards: Vec::new(),
            minimum_lands: Some(8),
            maximum_lands: Some(24),
            allowed_colors: None,
        };
        assert!(mutate_mana_base(
            &mut genome,
            &bounds,
            &pool,
            &constraints,
            &mut SplitMix64::new(7),
        ));
        assert_eq!(genome, vec![7, 1, 52]);
        assert_eq!(genome.iter().sum::<u8>(), 60);
    }

    #[test]
    fn derives_archetype_tags_without_requiring_a_catalog_resync() {
        let mut themed = card("Engine", false, 2.0, "");
        themed.type_line = "Legendary Creature — Elf Druid".into();
        themed.oracle_text = "Whenever you sacrifice a token, put a +1/+1 counter on target creature. You gain life.".into();
        themed.enrich_derived_tags();
        for expected in [
            "token",
            "sacrifice",
            "lifegain",
            "counters",
            "creature",
            "type:elf",
            "type:druid",
        ] {
            assert!(
                themed.features.tags.iter().any(|tag| tag == expected),
                "missing derived tag {expected}"
            );
        }
    }

    #[test]
    fn user_weights_change_structural_priority_without_changing_evidence() {
        let consistent = Fitness {
            consistency: 1.0,
            tempo: 0.1,
            ..Fitness::default()
        };
        let fast = Fitness {
            consistency: 0.1,
            tempo: 1.0,
            ..Fitness::default()
        };
        let consistency_weights = FitnessWeights {
            consistency: 100.0,
            tempo: 0.0,
            ..FitnessWeights::default()
        };
        let tempo_weights = FitnessWeights {
            consistency: 0.0,
            tempo: 100.0,
            ..FitnessWeights::default()
        };
        assert!(
            weighted_fitness_score(&consistent, &consistency_weights)
                > weighted_fitness_score(&fast, &consistency_weights)
        );
        assert!(
            weighted_fitness_score(&fast, &tempo_weights)
                > weighted_fitness_score(&consistent, &tempo_weights)
        );
    }

    #[test]
    fn pareto_layers_preserve_distinct_tradeoffs() {
        let candidate = |consistency, tempo| Candidate {
            genome: Vec::new(),
            active_cards: Vec::new(),
            sideboard_genome: Vec::new(),
            active_sideboard: Vec::new(),
            commanders: Vec::new(),
            fitness: Fitness {
                consistency,
                tempo,
                ..Fitness::default()
            },
        };
        let population = vec![
            candidate(1.0, 0.0),
            candidate(0.0, 1.0),
            candidate(0.0, 0.0),
        ];
        assert_eq!(
            nondominated_ranks_for_indices(&population, &[0, 1, 2]),
            vec![0, 0, 1]
        );
    }

    #[test]
    fn blank_start_generates_a_legal_sized_deck() {
        let pool = CardPool {
            cards: vec![
                card("Plains", true, 0.0, "mana"),
                card("Island", true, 0.0, "mana"),
                card("One Drop", false, 1.0, "tempo"),
                card("Two Drop", false, 2.0, "tempo"),
                card("Three Drop", false, 3.0, "value"),
                card("Answer", false, 2.0, "interaction"),
                card("Finisher", false, 5.0, "payoff"),
            ],
        };
        let blank = Deck {
            name: "Blank".into(),
            command_zone: vec![],
            mainboard: vec![],
            sideboard: vec![],
        };
        let constraints = DeckConstraints {
            target_mainboard_size: 40,
            target_sideboard_size: 0,
            minimum_commanders: 0,
            maximum_commanders: 0,
            default_max_copies: 4,
            locked_cards: vec![],
            minimum_lands: Some(16),
            maximum_lands: Some(20),
            allowed_colors: None,
        };
        let config = EvolutionConfig {
            population_size: 8,
            generations: 2,
            elite_count: 2,
            trials_per_candidate: 8,
            ..EvolutionConfig::default()
        };
        let result = EvolutionEngine::default()
            .run(
                &blank,
                &pool,
                &constraints,
                &config,
                &CancellationToken::default(),
                |_| {},
            )
            .unwrap();
        assert!(
            result
                .frontier
                .iter()
                .all(|candidate| candidate.deck.mainboard_count() == 40)
        );
    }

    #[test]
    fn blank_start_never_fills_the_frontier_with_near_clones() {
        let pool = CardPool {
            cards: (0..100)
                .map(|index| card(&format!("Spell {index}"), false, 0.0, ""))
                .collect(),
        };
        let blank = Deck {
            name: "Blank".into(),
            command_zone: vec![],
            mainboard: vec![],
            sideboard: vec![],
        };
        let constraints = DeckConstraints {
            target_mainboard_size: 60,
            target_sideboard_size: 0,
            minimum_commanders: 0,
            maximum_commanders: 0,
            default_max_copies: 4,
            locked_cards: vec![],
            minimum_lands: None,
            maximum_lands: None,
            allowed_colors: None,
        };
        let result = EvolutionEngine::default()
            .run(
                &blank,
                &pool,
                &constraints,
                &EvolutionConfig {
                    population_size: 16,
                    generations: 1,
                    elite_count: 2,
                    trials_per_candidate: 8,
                    mutation_swaps: 1,
                    package_mutation_rate: 0.0,
                    mana_mutation_rate: 0.0,
                    ..EvolutionConfig::default()
                },
                &CancellationToken::default(),
                |_| {},
            )
            .unwrap();

        assert!(!result.frontier.is_empty());
        for pair in result.frontier.windows(2) {
            let left = deck_to_genome(&pair[0].deck, &pool).unwrap();
            let right = deck_to_genome(&pair[1].deck, &pool).unwrap();
            assert!(
                copy_edit_distance_nonlands(&left, &right, &pool) >= 12,
                "the visible frontier must show fewer decks instead of near-clones"
            );
        }
    }

    #[test]
    fn mandatory_repair_fixes_land_count_before_color_tuning() {
        let pool = CardPool {
            cards: vec![
                card("Forest", true, 0.0, "mana"),
                card("Four Drop", false, 4.0, ""),
            ],
        };
        let constraints = DeckConstraints {
            target_mainboard_size: 60,
            target_sideboard_size: 0,
            minimum_commanders: 0,
            maximum_commanders: 0,
            default_max_copies: 60,
            locked_cards: vec![],
            minimum_lands: Some(20),
            maximum_lands: Some(28),
            allowed_colors: None,
        };
        let card_bounds = bounds(&pool, &constraints).unwrap();
        let mut genome = vec![20, 40];
        repair_mana_base(
            &mut genome,
            &card_bounds,
            &pool,
            &constraints,
            &mut SplitMix64::new(9),
        );

        assert_eq!(genome[0], 23);
        assert_eq!(
            genome
                .iter()
                .map(|count| usize::from(*count))
                .sum::<usize>(),
            60
        );
    }

    #[test]
    fn decks_without_explicit_requirements_do_not_receive_free_synergy() {
        let pool = CardPool {
            cards: vec![
                card("Forest", true, 0.0, ""),
                card("Vanilla", false, 1.0, ""),
            ],
        };
        let deck = Deck {
            name: "No package".into(),
            command_zone: vec![],
            mainboard: vec![
                DeckEntry {
                    card_id: CardId("forest".into()),
                    quantity: 24,
                },
                DeckEntry {
                    card_id: CardId("vanilla".into()),
                    quantity: 36,
                },
            ],
            sideboard: vec![],
        };
        let fitness = evaluate_structural_deck(&deck, &pool, 42, 32, 7).unwrap();
        assert!(
            fitness.synergy < 0.10,
            "unexpected synergy: {}",
            fitness.synergy
        );
    }

    #[test]
    fn repeated_copies_of_one_card_do_not_count_as_cross_card_synergy() {
        let pool = CardPool {
            cards: vec![
                card("Forest", true, 0.0, ""),
                card("Copy Engine", false, 2.0, "copy"),
                card("Filler", false, 2.0, ""),
            ],
        };
        let deck = Deck {
            name: "One tagged name".into(),
            command_zone: vec![],
            mainboard: vec![
                DeckEntry {
                    card_id: CardId("forest".into()),
                    quantity: 24,
                },
                DeckEntry {
                    card_id: CardId("copy-engine".into()),
                    quantity: 4,
                },
                DeckEntry {
                    card_id: CardId("filler".into()),
                    quantity: 32,
                },
            ],
            sideboard: vec![],
        };

        let fitness = evaluate_structural_deck(&deck, &pool, 91, 32, 7).unwrap();
        assert!(
            fitness.synergy < 0.05,
            "unexpected synergy: {}",
            fitness.synergy
        );
    }

    #[test]
    fn blank_initialization_builds_a_coherent_mechanic_package() {
        let mut cards = vec![card("Forest", true, 0.0, "")];
        cards.extend((0..8).map(|index| card(&format!("Token {index}"), false, 2.0, "token")));
        cards.extend((0..8).map(|index| card(&format!("Copy {index}"), false, 2.0, "copy")));
        cards.extend((0..16).map(|index| card(&format!("Filler {index}"), false, 2.0, "")));
        let pool = CardPool { cards };
        let constraints = DeckConstraints {
            target_mainboard_size: 40,
            target_sideboard_size: 0,
            minimum_commanders: 0,
            maximum_commanders: 0,
            default_max_copies: 4,
            locked_cards: vec![],
            minimum_lands: Some(16),
            maximum_lands: Some(20),
            allowed_colors: None,
        };
        let card_bounds = bounds(&pool, &constraints).unwrap();
        let genome = generate_seed_genome(
            &pool,
            &constraints,
            &card_bounds,
            &[],
            &mut SplitMix64::new(0xF0C0_5EED),
        )
        .unwrap();
        let token_copies = genome[1..9]
            .iter()
            .map(|count| usize::from(*count))
            .sum::<usize>();
        let copy_copies = genome[9..17]
            .iter()
            .map(|count| usize::from(*count))
            .sum::<usize>();

        assert!(
            token_copies.max(copy_copies) >= 14,
            "expected a focused package, got token={token_copies}, copy={copy_copies}"
        );
    }

    #[test]
    fn singleton_commander_seed_prefers_identity_basics_and_commander_theme() {
        let mut plains = card("Forest", true, 0.0, "mana");
        plains.type_line = "Basic Land — Forest".into();
        plains.color_identity = vec![Color::Green];
        plains.features.initial_modes = vec![CardMode {
            name: "Forest".into(),
            type_line: "Basic Land — Forest".into(),
            is_land: true,
            mana_sources: vec![ManaSourceProfile {
                colors: vec![Color::Green],
                color_rule: ManaColorRule::Fixed,
                spend_rule: ManaSpendRule::Unrestricted,
                uses_tap: true,
                earliest_turn: 1,
                ..ManaSourceProfile::default()
            }],
            ..CardMode::default()
        }];
        let mut cards = vec![plains];
        cards.extend((0..30).map(|index| {
            let mut land = card(&format!("Utility Land {index}"), true, 0.0, "utility");
            land.features.unlimited_copies = false;
            land
        }));
        cards.extend((0..24).map(|index| {
            card(
                &format!("Equipment Support {index}"),
                false,
                2.0,
                "equipment",
            )
        }));
        cards.extend(
            (0..24).map(|index| card(&format!("Treasure Support {index}"), false, 2.0, "treasure")),
        );
        cards.extend((0..50).map(|index| card(&format!("Filler {index}"), false, 3.0, "")));
        let pool = CardPool { cards };
        let constraints = DeckConstraints {
            target_mainboard_size: 99,
            target_sideboard_size: 0,
            minimum_commanders: 1,
            maximum_commanders: 1,
            default_max_copies: 1,
            locked_cards: vec![],
            minimum_lands: Some(36),
            maximum_lands: Some(44),
            allowed_colors: None,
        };
        let copy_bounds = bounds(&pool, &constraints).unwrap();
        let genome = generate_seed_genome(
            &pool,
            &constraints,
            &copy_bounds,
            &["equipment".into()],
            &mut SplitMix64::new(0xC011_AA0D),
        )
        .unwrap();
        let equipment = genome[31..55]
            .iter()
            .map(|count| usize::from(*count))
            .sum::<usize>();
        let treasure = genome[55..79]
            .iter()
            .map(|count| usize::from(*count))
            .sum::<usize>();

        assert!(
            genome[0] >= 20,
            "expected identity basics, got {}",
            genome[0]
        );
        assert!(
            equipment > treasure,
            "expected commander theme to win, equipment={equipment}, treasure={treasure}"
        );
    }

    #[test]
    fn blank_constructed_initialization_uses_traditional_copy_spreads() {
        let pool = CardPool {
            cards: (0..120)
                .map(|index| card(&format!("Spell {index}"), false, 2.0, "tempo"))
                .collect(),
        };
        let constraints = DeckConstraints {
            target_mainboard_size: 60,
            target_sideboard_size: 0,
            minimum_commanders: 0,
            maximum_commanders: 0,
            default_max_copies: 4,
            locked_cards: vec![],
            minimum_lands: None,
            maximum_lands: None,
            allowed_colors: None,
        };
        let copy_bounds = bounds(&pool, &constraints).unwrap();
        let genome = generate_seed_genome(
            &pool,
            &constraints,
            &copy_bounds,
            &[],
            &mut SplitMix64::new(42),
        )
        .unwrap();
        let unique_cards = genome.iter().filter(|count| **count > 0).count();
        let four_ofs = genome.iter().filter(|count| **count == 4).count();

        assert_eq!(
            genome.iter().map(|count| *count as usize).sum::<usize>(),
            60
        );
        assert!(
            unique_cards <= 30,
            "unexpected singleton-heavy spread: {unique_cards}"
        );
        assert!(four_ofs >= 5, "expected several four-ofs, found {four_ofs}");
    }

    #[test]
    fn singleton_initialization_does_not_apply_constructed_copy_prior() {
        let pool = CardPool {
            cards: (0..120)
                .map(|index| card(&format!("Spell {index}"), false, 2.0, "tempo"))
                .collect(),
        };
        let constraints = DeckConstraints {
            target_mainboard_size: 60,
            target_sideboard_size: 0,
            minimum_commanders: 0,
            maximum_commanders: 0,
            default_max_copies: 1,
            locked_cards: vec![],
            minimum_lands: None,
            maximum_lands: None,
            allowed_colors: None,
        };
        let copy_bounds = bounds(&pool, &constraints).unwrap();
        let genome = generate_seed_genome(
            &pool,
            &constraints,
            &copy_bounds,
            &[],
            &mut SplitMix64::new(42),
        )
        .unwrap();

        assert_eq!(genome.iter().filter(|count| **count == 1).count(), 60);
        assert!(genome.iter().all(|count| *count <= 1));
    }

    #[test]
    fn constructed_random_mutation_consolidates_singletons_into_repeated_cards() {
        let pool = CardPool {
            cards: (0..100)
                .map(|index| card(&format!("Spell {index}"), false, 2.0, "tempo"))
                .collect(),
        };
        let constraints = DeckConstraints {
            target_mainboard_size: 60,
            target_sideboard_size: 0,
            minimum_commanders: 0,
            maximum_commanders: 0,
            default_max_copies: 4,
            locked_cards: vec![],
            minimum_lands: None,
            maximum_lands: None,
            allowed_colors: None,
        };
        let copy_bounds = bounds(&pool, &constraints).unwrap();
        let guide = MutationGuide::new(&pool, &[], &[]);
        let mut genome = vec![0; 100];
        genome[..60].fill(1);

        mutate_random(
            &mut genome,
            &copy_bounds,
            &pool,
            &constraints,
            &guide,
            24,
            &mut SplitMix64::new(7),
        );

        assert_eq!(
            genome.iter().map(|count| *count as usize).sum::<usize>(),
            60
        );
        assert!(genome.iter().filter(|count| **count > 0).count() < 55);
        assert!(genome.iter().any(|count| *count >= 3));
    }

    #[test]
    fn constructed_mutation_does_not_drift_outside_the_seeded_palette() {
        let mut red_cards = (0..20)
            .map(|index| {
                let mut result = card(&format!("Red {index}"), false, 2.0, "tempo");
                result.colors = vec![Color::Red];
                result.color_identity = vec![Color::Red];
                result
            })
            .collect::<Vec<_>>();
        let mut blue = card("Blue mutation", false, 2.0, "tempo");
        blue.colors = vec![Color::Blue];
        blue.color_identity = vec![Color::Blue];
        red_cards.push(blue);
        let pool = CardPool { cards: red_cards };
        let constraints = DeckConstraints {
            target_mainboard_size: 60,
            target_sideboard_size: 0,
            minimum_commanders: 0,
            maximum_commanders: 0,
            default_max_copies: 4,
            locked_cards: vec![],
            minimum_lands: None,
            maximum_lands: None,
            allowed_colors: None,
        };
        let card_bounds = bounds(&pool, &constraints).unwrap();
        let guide = MutationGuide::new(&pool, &[], &[]);
        let mut genome = vec![0; pool.cards.len()];
        genome[..15].fill(4);
        mutate_random(
            &mut genome,
            &card_bounds,
            &pool,
            &constraints,
            &guide,
            100,
            &mut SplitMix64::new(77),
        );
        assert_eq!(genome[20], 0);
    }

    #[test]
    fn repeated_search_actions_need_consumable_library_targets() {
        let mut searcher = card("Pathcruiser", false, 5.0, "crew");
        searcher.features.mechanics.search_action = Some(crate::SearchActionProfile {
            target: SearchTargetProfile::BasicLand,
            from_hand: false,
            ..crate::SearchActionProfile::default()
        });
        let mut basic = card("Swamp", true, 0.0, "mana");
        basic.type_line = "Basic Land — Swamp".into();
        let nonbasic = card("Cabal Pit", true, 0.0, "mana");
        let pool = CardPool {
            cards: vec![searcher, basic, nonbasic],
        };
        let deck = Deck {
            name: "Search target fixture".into(),
            command_zone: vec![],
            mainboard: vec![
                DeckEntry {
                    card_id: pool.cards[0].id.clone(),
                    quantity: 4,
                },
                DeckEntry {
                    card_id: pool.cards[1].id.clone(),
                    quantity: 1,
                },
                DeckEntry {
                    card_id: pool.cards[2].id.clone(),
                    quantity: 19,
                },
            ],
            sideboard: vec![],
        };
        let fitness = evaluate_structural_deck(&deck, &pool, 7, 16, 6).unwrap();
        assert!(
            fitness.synergy_warnings.iter().any(|warning| {
                warning.contains("4 search copies") && warning.contains("only 1")
            })
        );
        assert!(fitness.unmet_requirement_rate >= 0.75);
    }

    #[test]
    fn distinct_search_cards_compete_for_the_same_basic() {
        let search_profile = crate::SearchActionProfile {
            target: SearchTargetProfile::BasicLand,
            from_hand: false,
            ..crate::SearchActionProfile::default()
        };
        let mut first = card("World Map", false, 2.0, "search");
        first.features.mechanics.searches_basic_land = true;
        first.features.mechanics.search_action = Some(search_profile.clone());
        let mut second = card("Fountainport Bell", false, 1.0, "search");
        second.features.mechanics.searches_basic_land = true;
        second.features.mechanics.search_action = Some(search_profile);
        let mut basic = card("Wastes", true, 0.0, "mana");
        basic.type_line = "Basic Land — Wastes".into();
        let pool = CardPool {
            cards: vec![first, second, basic],
        };
        let mut fitness = Fitness::default();
        apply_structural_evidence(
            &mut fitness,
            &[(0, 1), (1, 1), (2, 1)],
            &pool,
            6,
            &StructuralEvaluationContext::default(),
        );
        assert!(fitness.synergy_warnings.iter().any(|warning| {
            warning.contains("2 basic-land search copies") && warning.contains("only 1 consumable")
        }));
        assert!(fitness.unmet_requirement_rate >= 0.5);
    }

    #[test]
    fn graveyard_hate_and_uncastable_faces_receive_conservative_evidence() {
        let mut crypt = card("Tormod's Crypt", false, 0.0, "graveyard");
        crypt.oracle_text =
            "{T}, Sacrifice Tormod's Crypt: Exile all cards from target player's graveyard.".into();
        assert_eq!(preferred_copy_target(&crypt), 1);

        let mut living_end = card("Living End", false, 0.0, "graveyard");
        living_end.features.initial_modes = vec![CardMode {
            name: living_end.name.clone(),
            type_line: "Sorcery".into(),
            normal_castable: Some(false),
            ..CardMode::default()
        }];
        let pool = CardPool {
            cards: vec![living_end],
        };
        let evidence = castability_for_card(
            0,
            1,
            &pool.cards[0].features.initial_modes[0],
            &[(0, 1)],
            &pool,
            6,
            &StructuralEvaluationContext::default(),
            &HashSet::new(),
            None,
            true,
        );
        assert_eq!(evidence.turn_horizon_castability, 0.0);
        assert!(evidence.reasons[0].contains("no printed mana cost"));
    }

    #[test]
    fn blank_commander_run_evolves_legal_leaders_and_libraries() {
        let mut forest = card("Forest", true, 0.0, "mana");
        forest.color_identity.clear();
        let mut plains = card("Plains", true, 0.0, "mana");
        plains.color_identity.clear();
        let pool = CardPool {
            cards: vec![
                commander("Green Leader", Color::Green),
                commander("White Leader", Color::White),
                forest,
                plains,
                card("Green One", false, 1.0, "pressure"),
                card("Green Two", false, 2.0, "draw"),
            ],
        };
        let seed = Deck {
            name: "Blank Commander".into(),
            command_zone: vec![],
            mainboard: vec![],
            sideboard: vec![],
        };
        let constraints = DeckConstraints {
            target_mainboard_size: 12,
            target_sideboard_size: 0,
            minimum_commanders: 1,
            maximum_commanders: 2,
            default_max_copies: 1,
            locked_cards: vec![],
            minimum_lands: Some(6),
            maximum_lands: Some(12),
            allowed_colors: None,
        };
        let config = EvolutionConfig {
            population_size: 24,
            generations: 4,
            elite_count: 4,
            trials_per_candidate: 8,
            evolve_commanders: true,
            commander_mutation_rate: 0.25,
            ..EvolutionConfig::default()
        };
        let result = EvolutionEngine::default()
            .run(
                &seed,
                &pool,
                &constraints,
                &config,
                &CancellationToken::default(),
                |_| {},
            )
            .unwrap();
        assert!(!result.frontier.is_empty());
        for candidate in result.frontier {
            assert_eq!(candidate.deck.mainboard_count(), 12);
            assert!(!candidate.deck.command_zone.is_empty());
            validate_deck_legality(&candidate.deck, &pool, &constraints).unwrap();
        }
    }

    #[test]
    fn commander_evolution_supports_compatible_pairs() {
        let mut leader_a = commander("Partner A", Color::Green);
        leader_a.oracle_text = "Partner".into();
        let mut leader_b = commander("Partner B", Color::White);
        leader_b.oracle_text = "Partner".into();
        let mut forest = card("Forest", true, 0.0, "mana");
        forest.color_identity.clear();
        let pool = CardPool {
            cards: vec![leader_a, leader_b, forest],
        };
        let constraints = DeckConstraints {
            target_mainboard_size: 8,
            target_sideboard_size: 0,
            minimum_commanders: 2,
            maximum_commanders: 2,
            default_max_copies: 1,
            locked_cards: vec![],
            minimum_lands: Some(8),
            maximum_lands: Some(8),
            allowed_colors: None,
        };
        let result = EvolutionEngine::default()
            .run(
                &Deck {
                    name: "Partners".into(),
                    command_zone: vec![],
                    mainboard: vec![],
                    sideboard: vec![],
                },
                &pool,
                &constraints,
                &EvolutionConfig {
                    population_size: 4,
                    generations: 1,
                    trials_per_candidate: 2,
                    evolve_commanders: true,
                    ..EvolutionConfig::default()
                },
                &CancellationToken::default(),
                |_| {},
            )
            .unwrap();
        assert!(
            result
                .frontier
                .iter()
                .all(|candidate| candidate.deck.command_zone.len() == 2)
        );
    }

    #[test]
    fn historic_regression_deck_is_legal_but_blocked_for_severe_colored_mana() {
        use crate::model::{CardMode, ManaCostProfile};

        fn profiled(
            name: &str,
            count_color: Option<(Color, u8)>,
            source: Option<Color>,
            legendary: bool,
        ) -> Card {
            let mut card = card(
                name,
                source.is_some(),
                if source.is_some() { 0.0 } else { 3.0 },
                "",
            );
            card.features.is_legendary = legendary;
            card.features.can_be_land = source.is_some();
            let mut cost = ManaCostProfile::default();
            if let Some((color, pips)) = count_color {
                cost.colored[mana_slot(color)] = pips;
            }
            card.features.initial_modes = vec![CardMode {
                name: name.into(),
                type_line: card.type_line.clone(),
                colors: count_color
                    .map(|(color, _)| vec![color])
                    .unwrap_or_default(),
                mana_cost: cost,
                is_land: source.is_some(),
                is_legendary: legendary,
                mana_source_colors: source.into_iter().collect(),
                ..CardMode::default()
            }];
            card
        }

        let pool = CardPool {
            cards: vec![
                profiled("Swamp", None, Some(Color::Black), false),
                profiled("Mountain", None, Some(Color::Red), false),
                profiled("Branchloft Pathway", None, Some(Color::Green), false),
                profiled("Waterlogged Grove", None, Some(Color::Blue), false),
                profiled("Venat", Some((Color::White, 2)), None, true),
                profiled("Tamiyo", Some((Color::Blue, 1)), None, true),
                profiled("Klothys", Some((Color::Green, 1)), None, true),
                profiled("Pharika", Some((Color::Green, 1)), None, true),
                profiled("Journey to Eternity", Some((Color::Green, 1)), None, true),
            ],
        };
        let quantities = [18, 2, 2, 1, 4, 3, 4, 4, 4];
        let deck = Deck {
            name: "User Historic regression".into(),
            command_zone: vec![],
            sideboard: vec![],
            mainboard: pool
                .cards
                .iter()
                .zip(quantities)
                .map(|(card, quantity)| DeckEntry {
                    card_id: card.id.clone(),
                    quantity,
                })
                .collect(),
        };
        let fitness = evaluate_structural_deck(&deck, &pool, 42, 16, 8).unwrap();
        assert!(fitness.severe_mana_failure);
        assert_eq!(fitness.score, -1.0);
        assert!(
            fitness
                .mana_blocking_reasons
                .iter()
                .any(|reason| reason.contains("Venat") && reason.contains("double-white"))
        );
    }

    fn source_card(
        name: &str,
        colors: Vec<Color>,
        color_rule: ManaColorRule,
        spend_rule: ManaSpendRule,
    ) -> Card {
        let mut result = card(name, true, 0.0, "mana");
        result.features.unlimited_copies = true;
        result.features.initial_modes = vec![CardMode {
            name: name.into(),
            type_line: "Land".into(),
            is_land: true,
            mana_sources: vec![ManaSourceProfile {
                colors,
                color_rule,
                spend_rule,
                uses_tap: true,
                earliest_turn: 1,
                ..ManaSourceProfile::default()
            }],
            ..CardMode::default()
        }];
        result
    }

    fn colored_spell(name: &str, color: Color, type_line: &str) -> Card {
        let mut result = card(name, false, 1.0, "");
        result.type_line = type_line.into();
        let mut cost = crate::ManaCostProfile::default();
        cost.colored[mana_slot(color)] = 1;
        result.features.initial_modes = vec![CardMode {
            name: name.into(),
            type_line: type_line.into(),
            colors: vec![color],
            mana_cost: cost,
            creature_types: type_line
                .split_once('—')
                .map(|(_, tail)| {
                    tail.split_whitespace()
                        .map(str::to_ascii_lowercase)
                        .collect()
                })
                .unwrap_or_default(),
            ..CardMode::default()
        }];
        result
    }

    #[test]
    fn restricted_sources_only_cast_eligible_spell_modes() {
        let hangar = source_card(
            "Mech Hangar",
            vec![Color::Blue],
            ManaColorRule::Fixed,
            ManaSpendRule::CastSubtypes(vec!["pilot".into(), "vehicle".into()]),
        );
        let whoosh = colored_spell("Whoosh!", Color::Blue, "Instant");
        let vehicle = colored_spell("Test Vehicle", Color::Blue, "Artifact — Vehicle");
        let pool = CardPool {
            cards: vec![hangar, whoosh, vehicle],
        };
        let make_deck = |spell: usize| Deck {
            name: "restriction fixture".into(),
            command_zone: vec![],
            sideboard: vec![],
            mainboard: vec![
                DeckEntry {
                    card_id: pool.cards[0].id.clone(),
                    quantity: 20,
                },
                DeckEntry {
                    card_id: pool.cards[spell].id.clone(),
                    quantity: 4,
                },
            ],
        };
        let blocked = evaluate_structural_deck(&make_deck(1), &pool, 7, 16, 6).unwrap();
        assert!(blocked.severe_mana_failure);
        assert_eq!(blocked.card_castability[0].conditional_early_sources, 0);
        assert!(blocked.card_castability[0].unusable_sources >= 20);
        let eligible = evaluate_structural_deck(&make_deck(2), &pool, 7, 16, 6).unwrap();
        assert!(!eligible.severe_mana_failure);
        assert!(eligible.card_castability[0].conditional_early_sources >= 20);
    }

    #[test]
    fn command_tower_is_zero_mana_in_constructed_and_dynamic_in_commander() {
        let tower = source_card(
            "Command Tower",
            vec![
                Color::White,
                Color::Blue,
                Color::Black,
                Color::Red,
                Color::Green,
            ],
            ManaColorRule::CommanderIdentity,
            ManaSpendRule::Unrestricted,
        );
        let spell = colored_spell("Blue Spell", Color::Blue, "Sorcery");
        let mut leader = commander("Blue Leader", Color::Blue);
        leader.features.initial_modes = vec![CardMode {
            name: leader.name.clone(),
            type_line: leader.type_line.clone(),
            colors: vec![Color::Blue],
            is_legendary: true,
            ..CardMode::default()
        }];
        let pool = CardPool {
            cards: vec![tower, spell, leader],
        };
        let mut deck = Deck {
            name: "tower fixture".into(),
            command_zone: vec![],
            sideboard: vec![],
            mainboard: vec![
                DeckEntry {
                    card_id: pool.cards[0].id.clone(),
                    quantity: 20,
                },
                DeckEntry {
                    card_id: pool.cards[1].id.clone(),
                    quantity: 4,
                },
            ],
        };
        let constructed = evaluate_structural_deck(&deck, &pool, 11, 16, 6).unwrap();
        assert!(constructed.severe_mana_failure);
        assert!(
            constructed
                .synergy_warnings
                .iter()
                .any(|warning| warning.contains("produces no mana without a commander"))
        );
        deck.command_zone.push(DeckEntry {
            card_id: pool.cards[2].id.clone(),
            quantity: 1,
        });
        let commander = evaluate_structural_deck(&deck, &pool, 11, 16, 6).unwrap();
        assert!(!commander.severe_mana_failure);
        assert!(commander.card_castability[0].conditional_early_sources >= 20);
    }

    #[test]
    fn duplicate_legendary_lands_are_observed_as_dead_occupancy_draws() {
        let mut legendary_land = source_card(
            "Otawara Fixture",
            vec![Color::Blue],
            ManaColorRule::Fixed,
            ManaSpendRule::Unrestricted,
        );
        legendary_land.features.is_legendary = true;
        legendary_land.features.unlimited_copies = false;
        legendary_land.features.initial_modes[0].is_legendary = true;
        let island = source_card(
            "Island",
            vec![Color::Blue],
            ManaColorRule::Fixed,
            ManaSpendRule::Unrestricted,
        );
        let spell = colored_spell("Blue Spell", Color::Blue, "Sorcery");
        let pool = CardPool {
            cards: vec![legendary_land, island, spell],
        };
        let deck = Deck {
            name: "Legendary land occupancy".into(),
            command_zone: vec![],
            sideboard: vec![],
            mainboard: vec![
                DeckEntry {
                    card_id: pool.cards[0].id.clone(),
                    quantity: 4,
                },
                DeckEntry {
                    card_id: pool.cards[1].id.clone(),
                    quantity: 20,
                },
                DeckEntry {
                    card_id: pool.cards[2].id.clone(),
                    quantity: 36,
                },
            ],
        };

        let fitness = evaluate_structural_deck(&deck, &pool, 0x0A7A, 512, 8).unwrap();
        assert!(
            fitness.legendary_dead_draw_rate > 0.0,
            "duplicate legendary lands should not be treated as independent sources"
        );
    }
}
