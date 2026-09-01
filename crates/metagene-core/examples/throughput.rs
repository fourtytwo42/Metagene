use metagene_core::{
    CancellationToken, Card, CardFeatures, CardId, CardMode, CardPool, Color, Deck,
    DeckConstraints, DeckEntry, Evaluator, EvolutionConfig, EvolutionEngine, HeuristicEvaluator,
    ManaColorRule, ManaCostProfile, ManaSourceProfile, ManaSpendRule, StructuralEvaluationContext,
};
use std::collections::BTreeMap;
use std::time::Instant;

fn main() {
    let trials_per_candidate = std::env::var("METAGENE_BENCH_TRIALS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(64);
    let mut cards = Vec::new();
    for index in 0..250 {
        let is_land = index < 24;
        let mut mana_cost = ManaCostProfile::default();
        if !is_land {
            mana_cost.generic = (index % 5) as u8;
            mana_cost.colored[4] = 1;
        }
        let mana_sources = if !is_land {
            Vec::new()
        } else if index < 12 {
            vec![ManaSourceProfile {
                colors: vec![Color::Green],
                uses_tap: true,
                earliest_turn: 1,
                label: "unrestricted green".into(),
                ..ManaSourceProfile::default()
            }]
        } else if index < 18 {
            vec![
                ManaSourceProfile {
                    colors: vec![Color::Colorless],
                    uses_tap: true,
                    earliest_turn: 1,
                    label: "unrestricted colorless".into(),
                    ..ManaSourceProfile::default()
                },
                ManaSourceProfile {
                    colors: vec![Color::Green],
                    spend_rule: ManaSpendRule::CastSubtypes(vec!["vehicle".into()]),
                    uses_tap: true,
                    earliest_turn: 1,
                    label: "Vehicle only".into(),
                    ..ManaSourceProfile::default()
                },
            ]
        } else if index < 21 {
            vec![
                ManaSourceProfile {
                    colors: vec![Color::Colorless],
                    uses_tap: true,
                    earliest_turn: 1,
                    label: "unrestricted colorless".into(),
                    ..ManaSourceProfile::default()
                },
                ManaSourceProfile {
                    colors: vec![Color::Green],
                    spend_rule: ManaSpendRule::UnknownRestricted,
                    uses_tap: true,
                    earliest_turn: 1,
                    label: "unknown restriction".into(),
                    ..ManaSourceProfile::default()
                },
            ]
        } else {
            vec![ManaSourceProfile {
                colors: vec![Color::Green],
                color_rule: ManaColorRule::CommanderIdentity,
                uses_tap: true,
                earliest_turn: 1,
                label: "Command Tower fixture".into(),
                ..ManaSourceProfile::default()
            }]
        };
        let early_colors = if index < 12 {
            vec![Color::Green]
        } else if index < 21 {
            vec![Color::Colorless]
        } else {
            Vec::new()
        };
        cards.push(Card {
            id: CardId(format!("card-{index}")),
            arena_id: Some(index as u64 + 1),
            platforms: vec!["arena".into()],
            name: format!("Benchmark Card {index}"),
            set_code: "tst".into(),
            collector_number: index.to_string(),
            type_line: if is_land { "Land" } else { "Spell" }.into(),
            oracle_text: String::new(),
            colors: vec![Color::Green],
            color_identity: vec![Color::Green],
            image_uri: None,
            legalities: BTreeMap::new(),
            features: CardFeatures {
                is_land,
                unlimited_copies: is_land,
                mana_value: if is_land { 0.0 } else { (index % 7 + 1) as f32 },
                mana_production: if is_land { 1.0 } else { 0.0 },
                ramp: if index % 13 == 0 { 1.0 } else { 0.0 },
                card_advantage: if index % 11 == 0 { 1.0 } else { 0.0 },
                interaction: if index % 7 == 0 { 1.0 } else { 0.0 },
                pressure: if is_land { 0.0 } else { 0.8 },
                resilience: if index % 17 == 0 { 1.0 } else { 0.0 },
                tags: vec![format!("package-{}", index % 12)],
                can_be_land: is_land,
                initial_modes: vec![CardMode {
                    name: format!("Benchmark Card {index}"),
                    type_line: if is_land { "Land" } else { "Spell" }.into(),
                    colors: (!is_land).then_some(Color::Green).into_iter().collect(),
                    mana_cost,
                    is_land,
                    mana_source_colors: early_colors,
                    mana_sources,
                    ..CardMode::default()
                }],
                ..CardFeatures::default()
            },
        });
    }
    let pool = CardPool { cards };
    let seed = Deck {
        name: "Throughput seed".into(),
        command_zone: Vec::new(),
        mainboard: (0..24)
            .map(|index| DeckEntry {
                card_id: CardId(format!("card-{index}")),
                quantity: 1,
            })
            .chain((24..33).map(|index| DeckEntry {
                card_id: CardId(format!("card-{index}")),
                quantity: 4,
            }))
            .collect(),
        sideboard: Vec::new(),
    };
    let config = EvolutionConfig {
        population_size: 1_024,
        generations: 10,
        elite_count: 64,
        trials_per_candidate,
        mutation_swaps: 3,
        ..EvolutionConfig::default()
    };

    let started = Instant::now();
    let result = EvolutionEngine::default()
        .run(
            &seed,
            &pool,
            &DeckConstraints::default(),
            &config,
            &CancellationToken::default(),
            |_| {},
        )
        .expect("benchmark should complete");
    let elapsed = started.elapsed().as_secs_f64();
    println!(
        "{} population slots, {} unique evaluations, {} reused, {} unique trials in {:.2}s: {:.0} slots/s, {:.0} unique evaluations/s, {:.0} unique trials/s",
        result.evaluated_candidates,
        result.unique_evaluations,
        result.evaluations_avoided,
        result.unique_evaluations * config.trials_per_candidate as u64,
        elapsed,
        result.evaluated_candidates as f64 / elapsed,
        result.unique_evaluations as f64 / elapsed,
        result.unique_evaluations as f64 * config.trials_per_candidate as f64 / elapsed,
    );

    if std::env::var_os("METAGENE_BENCH_PROFILE").is_some() {
        let genome = (0..24)
            .map(|index| (index, 1))
            .chain((24..33).map(|index| (index, 4)))
            .collect::<Vec<_>>();
        let seeds = (0..trials_per_candidate)
            .map(|trial| trial as u64)
            .collect::<Vec<_>>();
        let started = Instant::now();
        for _ in 0..10_000 {
            std::hint::black_box(HeuristicEvaluator.evaluate(
                &genome,
                &[],
                &pool,
                &seeds,
                8,
                &StructuralEvaluationContext::default(),
            ));
        }
        println!(
            "10000 evaluator calls in {:.3}s",
            started.elapsed().as_secs_f64()
        );
    }
}
