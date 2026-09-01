use crate::simulator::{
    BestOfThreeEvidence, BestOfThreeValidationRequest, FixedOrderBranch, SimulatorError,
    evaluate_fixed_order_branch, sideboard_opponent_key,
};
use metagene_core::{
    CancellationToken, CardPool, Deck, DeckConstraints, DeckEntry, MatchupSideboardPlan, PlayDraw,
    SideboardExchange, apply_sideboard_exchange, validate_sideboard_plans,
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SideboardEvolutionRequest {
    pub candidate: Deck,
    pub opponents: Vec<Deck>,
    #[serde(default)]
    pub opponent_weights: Vec<f64>,
    pub pool: CardPool,
    pub constraints: DeckConstraints,
    pub population_size: usize,
    pub generations: usize,
    pub games_per_policy: usize,
    pub maximum_swaps: usize,
    pub seed: i64,
    pub workers: usize,
    pub parallel_jobs: usize,
    pub hat_budget: usize,
    pub max_turns: usize,
    pub game_timeout_seconds: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SideboardPolicyCandidate {
    pub exchange: SideboardExchange,
    pub evidence: BestOfThreeEvidence,
    pub score: f64,
    pub configuration_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BranchPolicyEvolution {
    pub play_draw: PlayDraw,
    pub best_exchange: SideboardExchange,
    pub best_evidence: BestOfThreeEvidence,
    pub alternatives: Vec<SideboardPolicyCandidate>,
    pub evaluated_policies: usize,
    pub attempted_games: usize,
    pub cached_games: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatchupPolicyEvolution {
    pub opponent_key: String,
    pub opponent_name: String,
    pub weight: f64,
    pub games_per_policy: usize,
    pub on_play: BranchPolicyEvolution,
    pub on_draw: BranchPolicyEvolution,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SideboardEvolutionReport {
    pub engine: String,
    pub schema_version: String,
    pub provenance: crate::provenance::ResultProvenance,
    pub plans: Vec<MatchupSideboardPlan>,
    pub matchups: Vec<MatchupPolicyEvolution>,
    pub evaluated_policies: usize,
    pub attempted_games: usize,
    pub cached_games: usize,
    pub games_per_second: f64,
}

struct BranchRun {
    report: BranchPolicyEvolution,
    provenance: crate::provenance::ResultProvenance,
    estimated_seconds: f64,
}

struct EvaluatedExchange {
    candidate: SideboardPolicyCandidate,
    cached_games: usize,
    estimated_seconds: f64,
    provenance: crate::provenance::ResultProvenance,
}

pub fn evolve_sideboard_policies(
    data_dir: &Path,
    request: &SideboardEvolutionRequest,
    cancellation: &CancellationToken,
) -> Result<SideboardEvolutionReport, SimulatorError> {
    validate_request(request)?;
    let weights = normalized_weights(&request.opponent_weights, request.opponents.len());
    let mut matchups = Vec::with_capacity(request.opponents.len());
    let mut plans = Vec::with_capacity(request.opponents.len());
    let mut evaluated_policies = 0usize;
    let mut attempted_games = 0usize;
    let mut cached_games = 0usize;
    let mut estimated_seconds = 0.0;
    let mut provenance_template = None;
    let mut child_hashes = Vec::new();

    for (opponent_index, opponent) in request.opponents.iter().enumerate() {
        if cancellation.is_cancelled() {
            return Err(SimulatorError::Cancelled);
        }
        let opponent_key = sideboard_opponent_key(opponent_index, opponent)?;
        // Shares determine confidence budget while the floor guarantees every
        // opponent receives actual post-board evidence.
        let games_per_policy = ((request.games_per_policy.max(1) as f64
            * weights[opponent_index]
            * request.opponents.len() as f64)
            .round() as usize)
            .max(1);
        let on_play = evolve_branch(
            data_dir,
            request,
            opponent,
            PlayDraw::Play,
            games_per_policy,
            request
                .seed
                .wrapping_add((opponent_index as i64).wrapping_mul(4_000_037)),
            cancellation,
        )?;
        let on_draw = evolve_branch(
            data_dir,
            request,
            opponent,
            PlayDraw::Draw,
            games_per_policy,
            request
                .seed
                .wrapping_add((opponent_index as i64).wrapping_mul(4_000_037))
                .wrapping_add(2_000_033),
            cancellation,
        )?;
        for branch in [&on_play, &on_draw] {
            evaluated_policies =
                evaluated_policies.saturating_add(branch.report.evaluated_policies);
            attempted_games = attempted_games.saturating_add(branch.report.attempted_games);
            cached_games = cached_games.saturating_add(branch.report.cached_games);
            estimated_seconds += branch.estimated_seconds;
            provenance_template.get_or_insert_with(|| branch.provenance.clone());
            child_hashes.push(branch.provenance.configuration_hash.clone());
        }
        let plan = MatchupSideboardPlan {
            opponent_key: opponent_key.clone(),
            on_play: on_play.report.best_exchange.clone(),
            on_draw: on_draw.report.best_exchange.clone(),
        };
        plans.push(plan);
        matchups.push(MatchupPolicyEvolution {
            opponent_key,
            opponent_name: opponent.name.clone(),
            weight: weights[opponent_index],
            games_per_policy,
            on_play: on_play.report,
            on_draw: on_draw.report,
        });
    }

    validate_sideboard_plans(
        &request.candidate,
        &plans,
        &request.pool,
        &request.constraints,
    )
    .map_err(|error| SimulatorError::InvalidSideboardPolicy(error.to_string()))?;
    let mut provenance = provenance_template.ok_or_else(|| {
        SimulatorError::Process("sideboard evolution produced no simulator evidence".into())
    })?;
    provenance.configuration_hash = crate::provenance::configuration_hash(
        "hexdek-sideboard-evolution-v1",
        &serde_json::json!({
            "request": request,
            "normalized_weights": weights,
            "plans": plans,
            "child_configuration_hashes": child_hashes,
        }),
    )?;
    Ok(SideboardEvolutionReport {
        engine: "hexdek".into(),
        schema_version: "1.0.0".into(),
        provenance,
        plans,
        matchups,
        evaluated_policies,
        attempted_games,
        cached_games,
        games_per_second: attempted_games as f64 / estimated_seconds.max(f64::EPSILON),
    })
}

fn evolve_branch(
    data_dir: &Path,
    request: &SideboardEvolutionRequest,
    opponent: &Deck,
    play_draw: PlayDraw,
    games_per_policy: usize,
    seed: i64,
    cancellation: &CancellationToken,
) -> Result<BranchRun, SimulatorError> {
    let population_size = request.population_size.clamp(2, 128);
    let generations = request.generations.clamp(1, 12);
    let elite_count = (population_size / 4).max(1);
    let parallel_jobs = request.parallel_jobs.clamp(1, population_size);
    let engine_workers = (request.workers.max(1) / parallel_jobs).max(1);
    let base_request = BestOfThreeValidationRequest {
        candidate: request.candidate.clone(),
        opponents: vec![opponent.clone()],
        opponent_weights: vec![1.0],
        pool: request.pool.clone(),
        constraints: request.constraints.clone(),
        sideboard_plans: vec![],
        matches: 1,
        seed,
        workers: engine_workers,
        hat_budget: request.hat_budget,
        max_turns: request.max_turns,
        game_timeout_seconds: request.game_timeout_seconds,
    };
    let mut frontier = vec![SideboardExchange::default()];
    let mut total_evaluated = 0usize;
    let mut attempted_games = 0usize;
    let mut cached_games = 0usize;
    let mut estimated_seconds = 0.0;
    let mut final_ranked = Vec::new();

    for generation in 0..generations {
        if cancellation.is_cancelled() {
            return Err(SimulatorError::Cancelled);
        }
        let mut candidates = frontier.clone();
        for exchange in &frontier {
            candidates.extend(expand_exchange(
                &request.candidate,
                exchange,
                &request.pool,
                &request.constraints,
                request
                    .maximum_swaps
                    .min(request.candidate.sideboard_count()),
            ));
        }
        deduplicate_exchanges(&mut candidates)?;
        let elite_keys = frontier
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<HashSet<_>, _>>()?;
        candidates.sort_by_key(|exchange| {
            let key = serde_json::to_string(exchange).unwrap_or_default();
            (
                usize::from(!elite_keys.contains(&key)),
                exchange_priority(exchange, seed, generation),
            )
        });
        candidates.truncate(population_size);

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(parallel_jobs)
            .build()
            .map_err(|error| SimulatorError::Process(error.to_string()))?;
        let evaluated = pool.install(|| {
            candidates
                .par_iter()
                .map(|exchange| {
                    if cancellation.is_cancelled() {
                        return Err(SimulatorError::Cancelled);
                    }
                    let deck = apply_sideboard_exchange(
                        &request.candidate,
                        exchange,
                        &request.pool,
                        &request.constraints,
                    )
                    .map_err(|error| SimulatorError::InvalidSideboardPolicy(error.to_string()))?;
                    let hash =
                        crate::provenance::configuration_hash("sideboard-exchange-v1", exchange)?;
                    let policy_seed = seed.wrapping_add(hash_seed(&hash));
                    let evaluation = evaluate_fixed_order_branch(
                        data_dir,
                        &base_request,
                        &deck,
                        opponent,
                        FixedOrderBranch {
                            candidate_on_play: play_draw == PlayDraw::Play,
                            games: games_per_policy,
                            seed: policy_seed,
                        },
                        cancellation,
                    )?;
                    let score = (evaluation.evidence.win_rate
                        + evaluation.evidence.win_rate_lower_bound)
                        / 2.0;
                    Ok(EvaluatedExchange {
                        candidate: SideboardPolicyCandidate {
                            exchange: exchange.clone(),
                            evidence: evaluation.evidence,
                            score,
                            configuration_hash: evaluation.provenance.configuration_hash.clone(),
                        },
                        cached_games: evaluation.cached_games,
                        estimated_seconds: evaluation.estimated_seconds,
                        provenance: evaluation.provenance,
                    })
                })
                .collect::<Result<Vec<_>, SimulatorError>>()
        })?;
        total_evaluated = total_evaluated.saturating_add(evaluated.len());
        attempted_games =
            attempted_games.saturating_add(evaluated.len().saturating_mul(games_per_policy));
        cached_games = cached_games.saturating_add(
            evaluated
                .iter()
                .map(|result| result.cached_games)
                .sum::<usize>(),
        );
        estimated_seconds += evaluated
            .iter()
            .map(|result| result.estimated_seconds)
            .sum::<f64>()
            / parallel_jobs as f64;
        let mut ranked = evaluated;
        ranked.sort_by(|left, right| {
            right
                .candidate
                .score
                .total_cmp(&left.candidate.score)
                .then_with(|| {
                    right
                        .candidate
                        .evidence
                        .games
                        .cmp(&left.candidate.evidence.games)
                })
                .then_with(|| {
                    left.candidate
                        .configuration_hash
                        .cmp(&right.candidate.configuration_hash)
                })
        });
        frontier = ranked
            .iter()
            .take(elite_count)
            .map(|result| result.candidate.exchange.clone())
            .collect();
        final_ranked = ranked;
    }

    let best = final_ranked.first().ok_or_else(|| {
        SimulatorError::Process("sideboard policy generation produced no candidate".into())
    })?;
    let provenance = best.provenance.clone();
    Ok(BranchRun {
        report: BranchPolicyEvolution {
            play_draw,
            best_exchange: best.candidate.exchange.clone(),
            best_evidence: best.candidate.evidence.clone(),
            alternatives: final_ranked
                .into_iter()
                .take(8)
                .map(|result| result.candidate)
                .collect(),
            evaluated_policies: total_evaluated,
            attempted_games,
            cached_games,
        },
        provenance,
        estimated_seconds,
    })
}

fn validate_request(request: &SideboardEvolutionRequest) -> Result<(), SimulatorError> {
    if request.opponents.is_empty() {
        return Err(SimulatorError::NotEnoughDecks(2));
    }
    if request.candidate.sideboard.is_empty() {
        return Err(SimulatorError::InvalidSideboardPolicy(
            "sideboard evolution requires at least one candidate sideboard card".into(),
        ));
    }
    metagene_core::validate_deck_legality(&request.candidate, &request.pool, &request.constraints)
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
    Ok(())
}

fn normalized_weights(weights: &[f64], count: usize) -> Vec<f64> {
    let usable = weights.len() == count
        && weights
            .iter()
            .all(|weight| weight.is_finite() && *weight > 0.0);
    let mut result = if usable {
        weights.to_vec()
    } else {
        vec![1.0; count]
    };
    let total = result.iter().sum::<f64>();
    for weight in &mut result {
        *weight /= total.max(f64::EPSILON);
    }
    result
}

fn expand_exchange(
    deck: &Deck,
    exchange: &SideboardExchange,
    pool: &CardPool,
    constraints: &DeckConstraints,
    maximum_swaps: usize,
) -> Vec<SideboardExchange> {
    let moved_in = quantity_map(&exchange.cards_in);
    let moved_out = quantity_map(&exchange.cards_out);
    let remaining_total = maximum_swaps.saturating_sub(exchange.cards_in_count());
    if remaining_total == 0 {
        return Vec::new();
    }
    let mut expansions = Vec::new();
    for side in &deck.sideboard {
        let side_remaining = (side.quantity as usize)
            .saturating_sub(moved_in.get(&side.card_id.0).copied().unwrap_or(0));
        if side_remaining == 0 || moved_out.contains_key(&side.card_id.0) {
            continue;
        }
        for main in &deck.mainboard {
            let main_remaining = (main.quantity as usize)
                .saturating_sub(moved_out.get(&main.card_id.0).copied().unwrap_or(0));
            if main_remaining == 0
                || moved_in.contains_key(&main.card_id.0)
                || side.card_id == main.card_id
            {
                continue;
            }
            let maximum = side_remaining.min(main_remaining).min(remaining_total);
            let quantities = if maximum > 1 {
                vec![1, maximum]
            } else {
                vec![1]
            };
            for quantity in quantities {
                let mut candidate = exchange.clone();
                add_quantity(&mut candidate.cards_in, &side.card_id.0, quantity);
                add_quantity(&mut candidate.cards_out, &main.card_id.0, quantity);
                sort_entries(&mut candidate.cards_in);
                sort_entries(&mut candidate.cards_out);
                if apply_sideboard_exchange(deck, &candidate, pool, constraints).is_ok() {
                    expansions.push(candidate);
                }
            }
        }
    }
    expansions
}

fn quantity_map(entries: &[DeckEntry]) -> HashMap<String, usize> {
    entries
        .iter()
        .map(|entry| (entry.card_id.0.clone(), entry.quantity as usize))
        .collect()
}

fn add_quantity(entries: &mut Vec<DeckEntry>, card_id: &str, quantity: usize) {
    if let Some(entry) = entries.iter_mut().find(|entry| entry.card_id.0 == card_id) {
        entry.quantity = entry.quantity.saturating_add(quantity as u8);
    } else {
        entries.push(DeckEntry {
            card_id: metagene_core::CardId(card_id.into()),
            quantity: quantity as u8,
        });
    }
}

fn sort_entries(entries: &mut [DeckEntry]) {
    entries.sort_by(|left, right| left.card_id.0.cmp(&right.card_id.0));
}

fn deduplicate_exchanges(exchanges: &mut Vec<SideboardExchange>) -> Result<(), SimulatorError> {
    let mut seen = HashSet::new();
    let mut retained = Vec::with_capacity(exchanges.len());
    for exchange in exchanges.drain(..) {
        let key = serde_json::to_string(&exchange)?;
        if seen.insert(key) {
            retained.push(exchange);
        }
    }
    *exchanges = retained;
    Ok(())
}

fn exchange_priority(exchange: &SideboardExchange, seed: i64, generation: usize) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"metagene-sideboard-population-v1\0");
    hasher.update(seed.to_le_bytes());
    hasher.update(generation.to_le_bytes());
    hasher.update(serde_json::to_vec(exchange).unwrap_or_default());
    hasher.finalize().into()
}

fn hash_seed(hash: &str) -> i64 {
    let digest = Sha256::digest(hash.as_bytes());
    i64::from_le_bytes(digest[..8].try_into().expect("fixed digest length"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use metagene_core::{Card, CardFeatures, CardId, Color};
    use std::collections::BTreeMap;

    fn card(name: &str, basic: bool) -> Card {
        Card {
            id: CardId(name.into()),
            arena_id: None,
            platforms: vec!["paper".into()],
            name: name.into(),
            set_code: "tst".into(),
            collector_number: "1".into(),
            type_line: if basic { "Basic Land" } else { "Instant" }.into(),
            oracle_text: String::new(),
            colors: vec![Color::Blue],
            color_identity: vec![Color::Blue],
            image_uri: None,
            legalities: BTreeMap::new(),
            features: CardFeatures {
                unlimited_copies: basic,
                ..Default::default()
            },
        }
    }

    #[test]
    fn expansion_is_legal_deterministic_and_respects_swap_cap() {
        let pool = CardPool {
            cards: vec![
                card("Land", true),
                card("Main A", false),
                card("Side A", false),
            ],
        };
        let deck = Deck {
            name: "candidate".into(),
            command_zone: vec![],
            mainboard: vec![
                DeckEntry {
                    card_id: CardId("Land".into()),
                    quantity: 56,
                },
                DeckEntry {
                    card_id: CardId("Main A".into()),
                    quantity: 4,
                },
            ],
            sideboard: vec![DeckEntry {
                card_id: CardId("Side A".into()),
                quantity: 4,
            }],
        };
        let constraints = DeckConstraints {
            target_mainboard_size: 60,
            target_sideboard_size: 4,
            minimum_commanders: 0,
            maximum_commanders: 0,
            default_max_copies: 4,
            locked_cards: vec![],
            minimum_lands: None,
            maximum_lands: None,
            allowed_colors: None,
        };
        let expansions =
            expand_exchange(&deck, &SideboardExchange::default(), &pool, &constraints, 3);
        assert!(
            expansions
                .iter()
                .any(|exchange| exchange.cards_in_count() == 1)
        );
        assert!(
            expansions
                .iter()
                .any(|exchange| exchange.cards_in_count() == 3)
        );
        assert!(expansions.iter().all(|exchange| {
            exchange.cards_in_count() <= 3
                && apply_sideboard_exchange(&deck, exchange, &pool, &constraints).is_ok()
        }));
    }

    #[test]
    fn normalized_weights_fall_back_and_preserve_valid_shares() {
        assert_eq!(normalized_weights(&[], 2), vec![0.5, 0.5]);
        assert_eq!(normalized_weights(&[3.0, 1.0], 2), vec![0.75, 0.25]);
    }
}
