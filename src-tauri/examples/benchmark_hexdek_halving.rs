use metagene_app_lib::{catalog, simulator};
use metagene_core::{
    CancellationToken, Card, CardPool, Deck, DeckConstraints, DeckEntry, EvolutionConfig,
    EvolutionEngine, RankedDeck, export_mtga,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

fn exact_card(data_dir: &Path, name: &str) -> Result<Card, Box<dyn std::error::Error>> {
    catalog::search(data_dir, name, "paper", "commander", 20)?
        .into_iter()
        .find(|card| card.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| format!("Commander catalog does not contain {name}").into())
}

fn evolve(
    all_cards: &[Card],
    commander: &Card,
    seed: u64,
) -> Result<Vec<RankedDeck>, Box<dyn std::error::Error>> {
    let identity: HashSet<_> = commander.color_identity.iter().copied().collect();
    let pool = CardPool {
        cards: all_cards
            .iter()
            .filter(|card| {
                card.id == commander.id
                    || card
                        .color_identity
                        .iter()
                        .all(|color| identity.contains(color))
            })
            .cloned()
            .collect(),
    };
    let seed_deck = Deck {
        name: format!("{} structural seed", commander.name),
        command_zone: vec![DeckEntry {
            card_id: commander.id.clone(),
            quantity: 1,
        }],
        mainboard: Vec::new(),
        sideboard: Vec::new(),
    };
    Ok(EvolutionEngine::default()
        .run(
            &seed_deck,
            &pool,
            &DeckConstraints {
                target_mainboard_size: 99,
                target_sideboard_size: 0,
                minimum_commanders: 1,
                maximum_commanders: 1,
                default_max_copies: 1,
                locked_cards: Vec::new(),
                minimum_lands: Some(36),
                maximum_lands: Some(44),
                allowed_colors: None,
            },
            &EvolutionConfig {
                population_size: 64,
                generations: 4,
                elite_count: 8,
                trials_per_candidate: 64,
                mutation_swaps: 3,
                mutation_rate: 0.9,
                tournament_size: 4,
                seed,
                turn_horizon: 8,
                worker_threads: Some(30),
                evolve_commanders: false,
                commander_mutation_rate: 0.0,
                ..EvolutionConfig::default()
            },
            &CancellationToken::default(),
            |_| {},
        )?
        .frontier)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is not defined")?
        .join("dev.metagene.app");
    let legal_pool = CardPool {
        cards: catalog::legal_pool(&data_dir, "paper", "commander")?,
    };
    let all_cards =
        simulator::filter_pool_for_hexdek(&data_dir, &legal_pool, "parser_complete")?.cards;
    let isamaru = exact_card(&data_dir, "Isamaru, Hound of Konda")?;
    let candidates = evolve(&all_cards, &isamaru, 0x51CC_E551)?
        .into_iter()
        .take(8)
        .collect::<Vec<_>>();
    if let Some(candidate) = candidates.first() {
        println!(
            "Isamaru candidate #{} structural={:.3} mana_fail={:.1}% horizon={:.1}% synergy={:.1}% interaction={:.1}%\n{}",
            candidate.rank,
            candidate.fitness.score,
            candidate.fitness.mana_failure_rate * 100.0,
            candidate.fitness.turn_horizon_castability * 100.0,
            candidate.fitness.synergy * 100.0,
            candidate.fitness.interaction * 100.0,
            export_mtga(
                &candidate.deck,
                &CardPool {
                    cards: all_cards.clone()
                }
            )
        );
    }
    let opponent_leaders = [
        exact_card(&data_dir, "Krenko, Mob Boss")?,
        exact_card(&data_dir, "Talrand, Sky Summoner")?,
        exact_card(&data_dir, "Marwyn, the Nurturer")?,
    ];
    let mut opponents = Vec::new();
    for (index, commander) in opponent_leaders.iter().enumerate() {
        let mut opponent = evolve(&all_cards, commander, 0x0FF0_0000 + index as u64)?
            .into_iter()
            .next()
            .ok_or("empty opponent frontier")?
            .deck;
        opponent.name = format!("{} pod archetype", commander.name);
        opponents.push(opponent);
    }

    let report = simulator::run_successive_halving(
        &data_dir,
        &simulator::SuccessiveHalvingRequest {
            candidates,
            opponents,
            opponent_weights: Vec::new(),
            pool: CardPool { cards: all_cards },
            base_games: 16,
            rounds: 3,
            seed: 20260829,
            workers: 30,
            seats: 4,
            hat_budget: 20,
            max_turns: 60,
            game_timeout_seconds: 5,
            structural_weight: 0.35,
            risk_aversion: 0.5,
            worst_matchup_weight: 0.25,
            seat_position_weight: 0.25,
            adaptive_elimination: true,
            commander: true,
            route_plan: None,
        },
        |progress| {
            eprintln!(
                "round {}/{} candidate {}/{} (#{}), {} games each, {} attempts complete",
                progress.round,
                progress.total_rounds,
                progress.candidate_index,
                progress.candidates_in_round,
                progress.candidate_rank,
                progress.games_per_candidate,
                progress.completed_attempts
            );
        },
    )?;
    assert!(report.winner_rank.is_some());
    assert!(
        report
            .rounds
            .iter()
            .all(|round| round.games_per_candidate % 4 == 0)
    );
    let commander_reports = report
        .rounds
        .iter()
        .flat_map(|round| round.results.iter())
        .filter_map(|result| result.report.as_ref())
        .collect::<Vec<_>>();
    assert!(!commander_reports.is_empty());
    assert!(commander_reports.iter().all(|result| {
        result.commander_evidence.as_ref().is_some_and(|evidence| {
            evidence.evidence_mode == "multiplayer_pod"
                && evidence.threat_development.source == "rules_engine_compact"
                && evidence.partial_rotation_games == result.games % 4
        })
    }));
    assert!(
        commander_reports
            .iter()
            .filter(|result| result.crashes > 0)
            .all(|result| result
                .commander_evidence
                .as_ref()
                .is_some_and(|evidence| evidence.penalties.timeout_or_crash_rate > 0.0))
    );
    let games_per_second = report.total_games as f64 / (report.elapsed_ms.max(1) as f64 / 1_000.0);
    println!(
        "rounds={} scheduled={} completed={} attempts={} crashes={} winner=#{} elapsed_ms={} throughput={games_per_second:.1} games/s native_reports={}",
        report.rounds.len(),
        report.scheduled_game_budget,
        report.total_games,
        report.total_attempts,
        report.total_crashes,
        report.winner_rank.unwrap_or_default(),
        report.elapsed_ms,
        commander_reports.len()
    );
    Ok(())
}
