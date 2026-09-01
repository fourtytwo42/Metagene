use metagene_app_lib::{catalog, simulator};
use metagene_core::{
    CancellationToken, CardPool, Deck, DeckConstraints, DeckEntry, EvolutionConfig, EvolutionEngine,
};
use serde::Serialize;
use std::path::PathBuf;
use std::time::Instant;
use sysinfo::System;

#[derive(Serialize)]
struct PipelineProfile {
    schema_version: u32,
    scenario: &'static str,
    simulation_seed: i64,
    catalog_ms: u64,
    catalog_warm_ms: u64,
    coverage_filter_ms: u64,
    structural_ms: u64,
    structural_unique_evaluations: u64,
    serialization_ms: u64,
    serialized_bytes: usize,
    cold: simulator::PerformanceBreakdown,
    warm: simulator::PerformanceBreakdown,
    cold_completed_games: usize,
    warm_cached_games: usize,
    resident_memory_before_bytes: u64,
    resident_memory_after_bytes: u64,
}

fn elapsed(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is not defined")?
        .join("dev.metagene.app");
    let mut system = System::new_all();
    system.refresh_all();
    let pid = sysinfo::get_current_pid()?;
    let memory_before = system.process(pid).map_or(0, |process| process.memory());

    let started = Instant::now();
    let legal = CardPool {
        cards: catalog::legal_pool(&data_dir, "arena", "historic")?,
    };
    let catalog_ms = elapsed(started);
    let started = Instant::now();
    let warm_catalog = catalog::legal_pool(&data_dir, "arena", "historic")?;
    let catalog_warm_ms = elapsed(started);
    if warm_catalog.len() != legal.cards.len() {
        return Err("warm catalog cache changed the legal pool".into());
    }
    let started = Instant::now();
    let parser_complete = simulator::filter_pool_for_hexdek(&data_dir, &legal, "parser_complete")?;
    let coverage_filter_ms = elapsed(started);
    let names =
        parser_complete
            .cards
            .iter()
            .filter(|card| card.features.unlimited_copies)
            .chain(parser_complete.cards.iter().filter(|card| {
                card.type_line.contains("Creature") && card.features.mana_value <= 4.0
            }))
            .take(128)
            .map(|card| card.name.clone())
            .collect::<Vec<_>>();
    let evolution_pool = parser_complete.focus_on_names(&names);
    let started = Instant::now();
    let evolved = EvolutionEngine::default().run(
        &Deck {
            name: "Stage 6 profile seed".into(),
            command_zone: Vec::new(),
            mainboard: Vec::new(),
            sideboard: Vec::new(),
        },
        &evolution_pool,
        &DeckConstraints::default(),
        &EvolutionConfig {
            population_size: 64,
            generations: 4,
            elite_count: 8,
            trials_per_candidate: 64,
            worker_threads: Some(30),
            seed: 0x5052_4F46_494C_4536,
            ..EvolutionConfig::default()
        },
        &CancellationToken::default(),
        |_| {},
    )?;
    let structural_ms = elapsed(started);
    let candidate = evolved.frontier[0].deck.clone();
    let forest = parser_complete
        .cards
        .iter()
        .find(|card| card.name == "Forest")
        .ok_or("Forest is absent")?;
    let opponent = Deck {
        name: "Stage 6 profile opponent".into(),
        command_zone: Vec::new(),
        mainboard: vec![DeckEntry {
            card_id: forest.id.clone(),
            quantity: 60,
        }],
        sideboard: Vec::new(),
    };
    let started = Instant::now();
    let bytes = serde_json::to_vec(&(&candidate, &opponent, &parser_complete))?;
    let serialization_ms = elapsed(started);
    let mut request = simulator::HexdekValidationRequest {
        candidate,
        opponents: vec![opponent],
        opponent_weights: vec![1.0],
        pool: parser_complete,
        games: 64,
        seed: 0x5052_4F46_494C_4536_u64.wrapping_add(1) as i64,
        workers: 30,
        seats: 2,
        hat_budget: 0,
        max_turns: 30,
        game_timeout_seconds: 3,
        commander: false,
        fixed_order: false,
    };
    let mut cold = simulator::run_validation_gauntlet(&data_dir, &request)?;
    for _ in 0..100 {
        if !cold
            .performance
            .as_ref()
            .is_some_and(|timing| timing.cache_hit)
        {
            break;
        }
        request.seed = request.seed.wrapping_add(1);
        cold = simulator::run_validation_gauntlet(&data_dir, &request)?;
    }
    if cold
        .performance
        .as_ref()
        .is_none_or(|timing| timing.cache_hit)
    {
        return Err("could not obtain a cold profile after 100 deterministic seed probes".into());
    }
    let warm = simulator::run_validation_gauntlet(&data_dir, &request)?;
    system.refresh_all();
    let memory_after = system.process(pid).map_or(0, |process| process.memory());
    let report = PipelineProfile {
        schema_version: 1,
        scenario: "arena-historic-bo1-64",
        simulation_seed: request.seed,
        catalog_ms,
        catalog_warm_ms,
        coverage_filter_ms,
        structural_ms,
        structural_unique_evaluations: evolved.unique_evaluations,
        serialization_ms,
        serialized_bytes: bytes.len(),
        cold: cold.performance.ok_or("cold timing was absent")?,
        warm: warm.performance.ok_or("warm timing was absent")?,
        cold_completed_games: cold.games,
        warm_cached_games: warm.cached_games,
        resident_memory_before_bytes: memory_before,
        resident_memory_after_bytes: memory_after,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
