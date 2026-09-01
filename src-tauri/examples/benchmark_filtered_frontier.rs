use metagene_app_lib::{catalog, simulator};
use metagene_core::{
    CancellationToken, CardPool, Deck, DeckConstraints, EvolutionConfig, EvolutionEngine,
    export_mtga,
};
use std::path::PathBuf;
use std::time::Instant;

fn benchmark_setting(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is not defined")?
        .join("dev.metagene.app");
    let legal_pool = CardPool {
        cards: catalog::legal_pool(&data_dir, "arena", "historic")?,
    };
    let filtered = simulator::filter_pool_for_hexdek(&data_dir, &legal_pool, "parser_complete")?;
    let focused_names = filtered
        .cards
        .iter()
        .filter(|card| card.features.unlimited_copies)
        .chain(filtered.cards.iter().filter(|card| {
            card.features
                .tags
                .iter()
                .any(|tag| matches!(tag.as_str(), "token" | "copy" | "ramp" | "interaction"))
        }))
        .take(256)
        .map(|card| card.name.clone())
        .collect::<Vec<_>>();
    let focused = filtered.focus_on_names(&focused_names);
    let population_size = benchmark_setting("METAGENE_BENCH_POPULATION", 128).max(2);
    let generations = benchmark_setting("METAGENE_BENCH_GENERATIONS", 50).max(1);
    let trials = benchmark_setting("METAGENE_BENCH_TRIALS", 256).max(1);
    let workers = benchmark_setting("METAGENE_BENCH_WORKERS", 30).max(1);
    let config = EvolutionConfig {
        population_size,
        generations,
        elite_count: 12.min(population_size - 1),
        trials_per_candidate: trials,
        mutation_swaps: 2,
        mutation_rate: 0.85,
        tournament_size: 4,
        seed: 0xC0DE_2026_0829,
        turn_horizon: 6,
        worker_threads: Some(workers),
        ..EvolutionConfig::default()
    };
    let seed = Deck {
        name: "Coverage-filtered Historic".into(),
        command_zone: Vec::new(),
        mainboard: Vec::new(),
        sideboard: Vec::new(),
    };
    let started = Instant::now();
    let result = EvolutionEngine::default().run(
        &seed,
        &focused,
        &DeckConstraints {
            target_mainboard_size: 60,
            target_sideboard_size: 0,
            minimum_commanders: 0,
            maximum_commanders: 0,
            default_max_copies: 4,
            locked_cards: Vec::new(),
            minimum_lands: Some(20),
            maximum_lands: Some(28),
            allowed_colors: None,
        },
        &config,
        &CancellationToken::default(),
        |_| {},
    )?;
    let elapsed = started.elapsed().as_secs_f64();
    let deck_refs = result
        .frontier
        .iter()
        .map(|candidate| &candidate.deck)
        .collect::<Vec<_>>();
    let inputs = simulator::coverage_inputs_from_decks(&deck_refs, &focused)?;
    let coverage = simulator::audit_hexdek_coverage(&data_dir, &inputs)?;
    let missing = coverage
        .decks
        .iter()
        .map(|deck| deck.missing_unique)
        .sum::<usize>();
    let partial = coverage
        .decks
        .iter()
        .map(|deck| deck.partial_unique)
        .sum::<usize>();
    if missing != 0 || partial != 0 {
        return Err(format!(
            "filtered frontier was not parser-complete: {missing} missing, {partial} partial"
        )
        .into());
    }
    let exported = export_mtga(&result.frontier[0].deck, &focused);
    let expected_export_lines = result.frontier[0].deck.mainboard.len() + 1;
    if !exported.starts_with("Deck\n") || exported.lines().count() != expected_export_lines {
        return Err("filtered Arena frontier was not exportable to MTGA".into());
    }
    if result.frontier[0].packages.is_empty() {
        return Err("filtered Arena winner did not expose any package summaries".into());
    }
    let package_labels = result.frontier[0]
        .packages
        .iter()
        .map(|package| {
            format!(
                "{}:{}:{}",
                package.tag, package.unique_cards, package.copies
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    println!(
        "legal={} parser_complete={} focused={} population_slots={} unique_evaluations={} evaluations_reused={} frontier={} elapsed={:.3}s unique_throughput={:.0}/s coverage_missing={} coverage_partial={} export_lines={} winner_packages={}",
        legal_pool.cards.len(),
        filtered.cards.len(),
        focused.cards.len(),
        result.evaluated_candidates,
        result.unique_evaluations,
        result.evaluations_avoided,
        result.frontier.len(),
        elapsed,
        result.unique_evaluations as f64 / elapsed.max(f64::EPSILON),
        missing,
        partial,
        exported.lines().count(),
        package_labels
    );
    Ok(())
}
