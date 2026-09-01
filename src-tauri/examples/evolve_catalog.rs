use metagene_app_lib::catalog;
use metagene_core::{
    CancellationToken, CardPool, Deck, DeckConstraints, EvolutionConfig, EvolutionEngine,
    export_mtga,
};
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let data_dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .expect("usage: evolve_catalog <application-data-directory> [platform] [format]");
    let platform = std::env::args().nth(2).unwrap_or_else(|| "arena".into());
    let format = std::env::args().nth(3).unwrap_or_else(|| "historic".into());
    let commander = format == "commander";
    let requested_population = std::env::args().nth(4).and_then(|value| value.parse().ok());
    let requested_generations = std::env::args().nth(5).and_then(|value| value.parse().ok());
    let requested_trials = std::env::args().nth(6).and_then(|value| value.parse().ok());
    let cards = catalog::legal_pool(&data_dir, &platform, &format).expect("pool query failed");
    let pool = CardPool { cards };
    let blank = Deck {
        name: format!("Blank {format}"),
        command_zone: vec![],
        mainboard: vec![],
        sideboard: vec![],
    };
    let config = EvolutionConfig {
        population_size: requested_population.unwrap_or(if commander { 128 } else { 256 }),
        generations: requested_generations.unwrap_or(if commander { 3 } else { 10 }),
        elite_count: 16,
        trials_per_candidate: requested_trials.unwrap_or(if commander { 16 } else { 64 }),
        mutation_swaps: 3,
        evolve_commanders: commander,
        commander_mutation_rate: 0.15,
        ..EvolutionConfig::default()
    };
    let constraints = if commander {
        DeckConstraints {
            target_mainboard_size: 99,
            target_sideboard_size: 0,
            minimum_commanders: 1,
            maximum_commanders: 2,
            default_max_copies: 1,
            locked_cards: vec![],
            minimum_lands: Some(34),
            maximum_lands: Some(46),
            allowed_colors: None,
        }
    } else {
        DeckConstraints::default()
    };
    let started = Instant::now();
    let result = EvolutionEngine::default()
        .run(
            &blank,
            &pool,
            &constraints,
            &config,
            &CancellationToken::default(),
            |_| {},
        )
        .expect("evolution failed");
    println!(
        "{platform} {format}: {} cards, {} population slots, {} unique evaluations, {} reused in {:.2}s ({:.0} unique evaluations/s), best commander(s): {}",
        pool.cards.len(),
        result.evaluated_candidates,
        result.unique_evaluations,
        result.evaluations_avoided,
        started.elapsed().as_secs_f64(),
        result.unique_evaluations as f64 / started.elapsed().as_secs_f64(),
        result.frontier[0].deck.command_zone.len(),
    );
    for candidate in result.frontier.iter().take(3) {
        println!(
            "rank={} fitness={:.3} consistency={:.0}% tempo={:.0}% synergy={:.0}% interaction={:.0}% mana_fail={:.0}% on_curve={:.0}% horizon={:.0}% colors={} color_penalty={:.3} unmet={:.0}% warnings={}",
            candidate.rank,
            candidate.fitness.score,
            candidate.fitness.consistency * 100.0,
            candidate.fitness.tempo * 100.0,
            candidate.fitness.synergy * 100.0,
            candidate.fitness.interaction * 100.0,
            candidate.fitness.mana_failure_rate * 100.0,
            candidate.fitness.on_curve_castability * 100.0,
            candidate.fitness.turn_horizon_castability * 100.0,
            candidate.fitness.required_color_count,
            candidate.fitness.color_complexity_penalty,
            candidate.fitness.unmet_requirement_rate * 100.0,
            candidate.fitness.synergy_warnings.join(" | "),
        );
        println!("{}", export_mtga(&candidate.deck, &pool));
    }
}
