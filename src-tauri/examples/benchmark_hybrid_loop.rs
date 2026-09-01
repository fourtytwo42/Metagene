use metagene_app_lib::{catalog, simulator};
use metagene_core::{
    CancellationToken, CardPool, Deck, DeckConstraints, DeckEntry, EvolutionConfig,
    EvolutionEngine, export_mtga,
};
use std::path::PathBuf;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is not defined")?
        .join("dev.metagene.app");
    let legal = CardPool {
        cards: catalog::legal_pool(&data_dir, "arena", "historic")?,
    };
    let validation_pool = simulator::filter_pool_for_hexdek(&data_dir, &legal, "parser_complete")?;
    let focused_names = validation_pool
        .cards
        .iter()
        .filter(|card| card.features.unlimited_copies)
        .chain(validation_pool.cards.iter().filter(|card| {
            card.type_line.contains("Creature")
                && card.features.mana_value <= 4.0
                && card.oracle_text.len() < 140
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
    let mountain = validation_pool
        .cards
        .iter()
        .find(|card| card.name == "Mountain")
        .ok_or("Historic parser-complete pool does not contain Mountain")?;
    let plains = validation_pool
        .cards
        .iter()
        .find(|card| card.name == "Plains")
        .ok_or("Historic parser-complete pool does not contain Plains")?;
    let red_opponent = Deck {
        name: "Hybrid benchmark red opponent".into(),
        command_zone: Vec::new(),
        mainboard: vec![DeckEntry {
            card_id: mountain.id.clone(),
            quantity: 60,
        }],
        sideboard: Vec::new(),
    };
    let white_opponent = Deck {
        name: "Hybrid benchmark white opponent".into(),
        command_zone: Vec::new(),
        mainboard: vec![DeckEntry {
            card_id: plains.id.clone(),
            quantity: 60,
        }],
        sideboard: Vec::new(),
    };
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
    let mut config = EvolutionConfig {
        population_size: 32,
        generations: 5,
        elite_count: 6,
        trials_per_candidate: 64,
        worker_threads: Some(30),
        seed: 0x4859_4252_4944,
        ..EvolutionConfig::default()
    };
    let mut seed = Deck {
        name: "Blank hybrid benchmark".into(),
        command_zone: Vec::new(),
        mainboard: Vec::new(),
        sideboard: Vec::new(),
    };
    let started = Instant::now();
    let mut evaluated = 0u64;
    let mut attempts = 0usize;
    let mut final_winner = None;

    for cycle in 1..=2usize {
        config.seed = config
            .seed
            .wrapping_add((cycle.saturating_sub(1) as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let result = EvolutionEngine::default().run(
            &seed,
            &evolution_pool,
            &constraints,
            &config,
            &CancellationToken::default(),
            |_| {},
        )?;
        evaluated = evaluated.saturating_add(result.evaluated_candidates);
        let candidates = result.frontier.iter().take(4).cloned().collect::<Vec<_>>();
        let halving = simulator::run_successive_halving(
            &data_dir,
            &simulator::SuccessiveHalvingRequest {
                candidates,
                opponents: vec![red_opponent.clone(), white_opponent.clone()],
                opponent_weights: Vec::new(),
                pool: validation_pool.clone(),
                base_games: 128,
                rounds: 2,
                seed: config.seed as i64,
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
        attempts = attempts.saturating_add(halving.total_attempts);
        let winner_rank = halving
            .winner_rank
            .ok_or("successive halving did not retain a winner")?;
        let winner = result
            .frontier
            .iter()
            .find(|candidate| candidate.rank == winner_rank)
            .ok_or("winner rank was absent from the structural frontier")?;
        let evidence = halving
            .rounds
            .iter()
            .rev()
            .flat_map(|round| round.results.iter())
            .find(|candidate| candidate.candidate_rank == winner_rank)
            .ok_or("winner evidence was absent from the halving report")?;
        if evidence.win_rate_lower_bound > evidence.game_win_rate
            || evidence.worst_matchup_lower_bound > evidence.worst_matchup_win_rate
        {
            return Err("confidence lower bound exceeded its observed win rate".into());
        }
        let evidence_report = evidence
            .report
            .as_ref()
            .ok_or("winner did not retain its matchup report")?;
        if !evidence_report
            .matchup_games
            .values()
            .any(|matchups| matchups.len() == 2)
        {
            return Err("constructed gauntlet did not retain both opponent matchups".into());
        }
        if halving.total_attempts != 2_048 {
            return Err(format!(
                "expected 2,048 two-opponent attempts, got {}",
                halving.total_attempts
            )
            .into());
        }
        seed = winner.deck.clone();
        final_winner = Some(winner.clone());
        println!(
            "cycle={} winner_rank={} structural={:.3} observed={:.3} lower={:.3} worst={:.3} evidence={:.3} attempts={} crashes={}",
            cycle,
            winner_rank,
            winner.fitness.score,
            evidence.game_win_rate,
            evidence.win_rate_lower_bound,
            evidence.worst_matchup_win_rate,
            evidence.evidence_score,
            halving.total_attempts,
            halving.total_crashes
        );
    }

    let winner = final_winner.ok_or("hybrid loop produced no winner")?;
    let export = export_mtga(&winner.deck, &validation_pool);
    if !export.starts_with("Deck\n") || winner.deck.mainboard_count() != 60 {
        return Err("hybrid winner was not a 60-card MTGA export".into());
    }
    if attempts != 4_096 {
        return Err(format!("expected 4,096 total attempts, got {attempts}").into());
    }
    println!(
        "hybrid_cycles=2 focused={} evaluated={} game_attempts={} elapsed={:.3}s export_lines={}",
        evolution_pool.cards.len(),
        evaluated,
        attempts,
        started.elapsed().as_secs_f64(),
        export.lines().count()
    );
    Ok(())
}
