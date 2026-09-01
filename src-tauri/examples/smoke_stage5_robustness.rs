use metagene_app_lib::{calibration, catalog, evaluation_splits, hall_of_fame, simulator};
use metagene_core::{
    CancellationToken, CardPool, Deck, DeckConstraints, DeckEntry, EvolutionConfig, EvolutionEngine,
};
use std::fs;
use std::path::PathBuf;
use std::time::Instant;
use uuid::Uuid;

fn basic_deck(pool: &CardPool, name: &str, basic: &str) -> Result<Deck, String> {
    let card = pool
        .cards
        .iter()
        .find(|card| card.name == basic)
        .ok_or_else(|| format!("missing {basic}"))?;
    Ok(Deck {
        name: name.into(),
        command_zone: Vec::new(),
        mainboard: vec![DeckEntry {
            card_id: card.id.clone(),
            quantity: 60,
        }],
        sideboard: Vec::new(),
    })
}

fn validate(
    data_dir: &std::path::Path,
    pool: &CardPool,
    mut deck: Deck,
    opponent: Deck,
    games: usize,
    seed: u64,
) -> Result<
    (
        simulator::SimulationReport,
        simulator::ValidationEvidenceScore,
    ),
    String,
> {
    deck.name = format!("stage5-{seed}-{}", deck.name);
    let label = format!("Candidate · {}", deck.name);
    let report = simulator::run_validation_gauntlet(
        data_dir,
        &simulator::HexdekValidationRequest {
            candidate: deck,
            opponents: vec![opponent],
            opponent_weights: vec![1.0],
            pool: pool.clone(),
            games,
            seed: seed as i64,
            workers: 30,
            seats: 2,
            hat_budget: 0,
            max_turns: 30,
            game_timeout_seconds: 3,
            commander: false,
            fixed_order: false,
        },
    )
    .map_err(|error| error.to_string())?;
    let score = simulator::score_validation_report(&report, &label, 0.5, 0.25, 0.25)
        .ok_or_else(|| "candidate evidence was missing".to_string())?;
    Ok((report, score))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let data_dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is not defined")?
        .join("dev.metagene.app");
    let scratch = std::env::temp_dir().join(format!("metagene-stage5-{}", Uuid::new_v4()));
    fs::create_dir_all(&scratch)?;
    let outcome = (|| -> Result<(), Box<dyn std::error::Error>> {
        let legal = CardPool {
            cards: catalog::legal_pool(&data_dir, "arena", "historic")?,
        };
        let validation_pool =
            simulator::filter_pool_for_hexdek(&data_dir, &legal, "parser_complete")?;
        let focused_names = validation_pool
            .cards
            .iter()
            .filter(|card| card.features.unlimited_copies)
            .chain(validation_pool.cards.iter().filter(|card| {
                card.type_line.contains("Creature") && card.features.mana_value <= 4.0
            }))
            .chain(validation_pool.cards.iter().filter(|card| {
                card.features
                    .tags
                    .iter()
                    .any(|tag| matches!(tag.as_str(), "ramp" | "interaction" | "card_advantage"))
            }))
            .take(128)
            .map(|card| card.name.clone())
            .collect::<Vec<_>>();
        let evolution_pool = validation_pool.focus_on_names(&focused_names);
        let field = ["Plains", "Island", "Swamp", "Mountain", "Forest"]
            .into_iter()
            .enumerate()
            .map(|(index, basic)| {
                basic_deck(
                    &validation_pool,
                    &format!("Fixed field {index} · {basic}"),
                    basic,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let split = evaluation_splits::plan(&field, false, 0x5354_4147_4535);
        if split.opponents_reused
            || split.train_seed == split.validation_seed
            || split.validation_seed == split.holdout_seed
        {
            return Err("fixed field did not produce leakage-safe domains".into());
        }
        let constraints = DeckConstraints {
            target_mainboard_size: 60,
            target_sideboard_size: 0,
            minimum_commanders: 0,
            maximum_commanders: 0,
            default_max_copies: 4,
            locked_cards: Vec::new(),
            minimum_lands: Some(20),
            maximum_lands: Some(28),
            allowed_colors: None,
        };
        let config = EvolutionConfig {
            population_size: 32,
            generations: 5,
            elite_count: 6,
            trials_per_candidate: 64,
            worker_threads: Some(30),
            seed: split.train_seed,
            mana_mutation_rate: 0.35,
            ..EvolutionConfig::default()
        };
        let result = EvolutionEngine::default().run(
            &Deck {
                name: "Blank Stage 5 benchmark".into(),
                command_zone: Vec::new(),
                mainboard: Vec::new(),
                sideboard: Vec::new(),
            },
            &evolution_pool,
            &constraints,
            &config,
            &CancellationToken::default(),
            |_| {},
        )?;
        let candidates = result.frontier.iter().take(4).cloned().collect::<Vec<_>>();
        if candidates.len() < 2 {
            return Err("structural benchmark produced fewer than two finalists".into());
        }
        let validation_opponent = split.validation_opponents[0].clone();
        let halving = simulator::run_successive_halving(
            &data_dir,
            &simulator::SuccessiveHalvingRequest {
                candidates: candidates.clone(),
                opponents: vec![validation_opponent],
                opponent_weights: vec![1.0],
                pool: validation_pool.clone(),
                base_games: 32,
                rounds: 2,
                seed: split.validation_seed as i64,
                workers: 30,
                seats: 2,
                hat_budget: 0,
                max_turns: 30,
                game_timeout_seconds: 3,
                structural_weight: 0.35,
                risk_aversion: 0.5,
                worst_matchup_weight: 0.25,
                seat_position_weight: 0.25,
                adaptive_elimination: true,
                commander: false,
                route_plan: None,
            },
            |_| {},
        )?;
        let winner_rank = halving.winner_rank.ok_or("halving produced no winner")?;
        let winner = candidates
            .iter()
            .find(|candidate| candidate.rank == winner_rank)
            .ok_or("winner was absent")?;
        let selection = halving
            .rounds
            .iter()
            .rev()
            .flat_map(|round| &round.results)
            .find(|row| row.candidate_rank == winner_rank)
            .ok_or("winner selection evidence was absent")?;
        let challenger_selection = halving
            .rounds
            .last()
            .and_then(|round| {
                round
                    .results
                    .iter()
                    .filter(|row| row.candidate_rank != winner_rank)
                    .max_by(|left, right| left.blended_score.total_cmp(&right.blended_score))
            })
            .ok_or("challenger selection evidence was absent")?;
        let challenger = candidates
            .iter()
            .find(|candidate| candidate.rank == challenger_selection.candidate_rank)
            .ok_or("challenger was absent")?;
        let holdout_opponent = split.holdout_opponents[0].clone();
        let (_, baseline_score) = validate(
            &data_dir,
            &validation_pool,
            basic_deck(&validation_pool, "Fixed all-land baseline", "Mountain")?,
            holdout_opponent.clone(),
            64,
            split.holdout_seed,
        )?;
        let mut latest_winner = None;
        let mut latest_challenger = None;
        let mut holdout_attempts = 64usize;
        for cycle in 1..=10usize {
            let seed = split.holdout_seed.wrapping_add(cycle as u64 - 1);
            let (winner_report, winner_score) = validate(
                &data_dir,
                &validation_pool,
                winner.deck.clone(),
                holdout_opponent.clone(),
                32,
                seed,
            )?;
            let (_, challenger_score) = validate(
                &data_dir,
                &validation_pool,
                challenger.deck.clone(),
                holdout_opponent.clone(),
                32,
                seed,
            )?;
            holdout_attempts += winner_report.games.saturating_mul(2);
            calibration::record(
                &scratch,
                calibration::CalibrationObservation {
                    platform: "arena",
                    format: "historic",
                    commander: false,
                    run_id: "stage5-live",
                    cycle,
                    selection_score: selection.evidence_score,
                    selection_lower: selection.evidence_lower_bound,
                    selection_upper: selection.evidence_upper_bound,
                    holdout_score: winner_score.observed,
                    challenger_selection_score: Some(challenger_selection.evidence_score),
                    challenger_holdout_score: Some(challenger_score.observed),
                    games: winner_report.games,
                },
            )?;
            latest_winner = Some(winner_score);
            latest_challenger = Some(challenger_score);
        }
        let winner_score = latest_winner.ok_or("missing winner holdout")?;
        let challenger_score = latest_challenger.ok_or("missing challenger holdout")?;
        if winner_score.observed <= baseline_score.observed {
            return Err(format!(
                "held-out winner {:.3} did not beat baseline {:.3}",
                winner_score.observed, baseline_score.observed
            )
            .into());
        }
        hall_of_fame::promote(
            &scratch,
            winner,
            hall_of_fame::HallOfFamePromotion {
                platform: "arena",
                format: "historic",
                commander: false,
                evidence_score: winner_score.observed,
                source_run_id: Some("stage5-live"),
                source_cycle: Some(1),
            },
        )?;
        hall_of_fame::promote(
            &scratch,
            challenger,
            hall_of_fame::HallOfFamePromotion {
                platform: "arena",
                format: "historic",
                commander: false,
                evidence_score: challenger_score.observed,
                source_run_id: Some("stage5-live"),
                source_cycle: Some(1),
            },
        )?;
        let hall = hall_of_fame::status(&scratch, "arena", "historic", false)?;
        let packages = hall_of_fame::discover_packages(
            &scratch,
            "arena",
            "historic",
            false,
            &validation_pool,
            32,
        )?;
        let calibration = calibration::status(
            &scratch,
            "arena",
            "historic",
            false,
            split.target_interval_coverage,
        )?;
        let decision = calibration::decision(&calibration, 0.5, true);
        if calibration.state == "undercovered" && decision.adaptive_elimination {
            return Err("undercovered evidence did not disable confidence elimination".into());
        }
        println!(
            "policy={} train_field={} validation_field={} holdout_field={} structural_evaluations={} halving_attempts={} holdout_attempts={} baseline_holdout={:.3} winner_holdout={:.3} challenger_holdout={:.3} calibration={} coverage={:.3} agreement={:.3} effective_risk={:.3} adaptive_elimination={} hall_entries={} active_diverse={} learned_packages={} elapsed={:.3}s",
            split.policy_id,
            split.train_opponents.len(),
            split.validation_opponents.len(),
            split.holdout_opponents.len(),
            result.evaluated_candidates,
            halving.total_attempts,
            holdout_attempts,
            baseline_score.observed,
            winner_score.observed,
            challenger_score.observed,
            calibration.state,
            calibration.empirical_interval_coverage,
            calibration.selection_agreement.unwrap_or_default(),
            decision.effective_risk_aversion,
            decision.adaptive_elimination,
            hall.entries.len(),
            hall.active_diverse_ids.len(),
            packages.len(),
            started.elapsed().as_secs_f64(),
        );
        Ok(())
    })();
    if scratch.exists() {
        fs::remove_dir_all(&scratch)?;
    }
    outcome
}
