use metagene_app_lib::{catalog, simulator};
use metagene_core::{
    CancellationToken, CardPool, Deck, DeckConstraints, EvolutionConfig, EvolutionEngine,
    export_mtga, import_decklist, validate_deck_legality,
};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

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
            card.features.tags.iter().any(|tag| {
                matches!(
                    tag.as_str(),
                    "interaction" | "resilience" | "draw" | "recursion" | "ramp"
                )
            })
        }))
        .take(256)
        .map(|card| card.name.clone())
        .collect::<Vec<_>>();
    let focused = filtered.focus_on_names(&focused_names);
    let constraints = DeckConstraints {
        target_mainboard_size: 60,
        target_sideboard_size: 15,
        minimum_commanders: 0,
        maximum_commanders: 0,
        default_max_copies: 4,
        locked_cards: Vec::new(),
        minimum_lands: Some(20),
        maximum_lands: Some(28),
        allowed_colors: None,
    };
    let config = EvolutionConfig {
        population_size: 128,
        generations: 25,
        elite_count: 12,
        trials_per_candidate: 128,
        mutation_swaps: 2,
        mutation_rate: 0.85,
        evolve_sideboard: true,
        worker_threads: Some(30),
        seed: 0xB035_1DE2_0260_0815,
        ..EvolutionConfig::default()
    };
    let started = Instant::now();
    let result = EvolutionEngine::default().run(
        &Deck {
            name: "Blank Historic Best-of-Three".into(),
            command_zone: Vec::new(),
            mainboard: Vec::new(),
            sideboard: Vec::new(),
        },
        &focused,
        &constraints,
        &config,
        &CancellationToken::default(),
        |_| {},
    )?;
    let elapsed = started.elapsed().as_secs_f64();

    let mut sideboards = HashSet::new();
    for candidate in &result.frontier {
        if candidate.deck.mainboard_count() != 60 || candidate.deck.sideboard_count() != 15 {
            return Err("frontier did not retain the configured 60+15 zone sizes".into());
        }
        validate_deck_legality(&candidate.deck, &focused, &constraints)?;
        sideboards.insert(
            candidate
                .deck
                .sideboard
                .iter()
                .map(|entry| (entry.card_id.0.clone(), entry.quantity))
                .collect::<Vec<_>>(),
        );
    }
    if sideboards.len() < 2 {
        return Err("sideboard evolution did not retain multiple distinct finalists".into());
    }

    let winner = &result.frontier[0];
    let exported = export_mtga(&winner.deck, &focused);
    if !exported.contains("\nSideboard\n") {
        return Err("MTGA export omitted the evolved sideboard".into());
    }
    let round_trip = import_decklist(&exported, &focused)?;
    if round_trip.mainboard_count() != 60 || round_trip.sideboard_count() != 15 {
        return Err("MTGA export/import did not round-trip the 60+15 deck".into());
    }
    let deck_refs = result
        .frontier
        .iter()
        .map(|candidate| &candidate.deck)
        .collect::<Vec<_>>();
    let coverage = simulator::audit_hexdek_coverage(
        &data_dir,
        &simulator::coverage_inputs_from_decks(&deck_refs, &focused)?,
    )?;
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
            "sideboard frontier was not parser-complete: {missing} missing, {partial} partial"
        )
        .into());
    }

    println!(
        "focused={} population_slots={} unique_evaluations={} reused={} frontier={} distinct_sideboards={} elapsed={elapsed:.3}s unique_throughput={:.0}/s winner_sideboard_quality={:.3} export_lines={} coverage_missing={} coverage_partial={}",
        focused.cards.len(),
        result.evaluated_candidates,
        result.unique_evaluations,
        result.evaluations_avoided,
        result.frontier.len(),
        sideboards.len(),
        result.unique_evaluations as f64 / elapsed.max(f64::EPSILON),
        winner.fitness.sideboard_quality,
        exported.lines().count(),
        missing,
        partial,
    );
    Ok(())
}
