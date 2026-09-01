use metagene_core::{
    CancellationToken, Card, CardFeatures, CardId, CardPool, Color, Deck, DeckConstraints,
    DeckEntry, Evaluator, EvolutionConfig, EvolutionEngine, Fitness,
};
use std::collections::BTreeMap;
use std::time::Instant;

struct PackageEvaluator;

impl Evaluator for PackageEvaluator {
    fn evaluate(
        &self,
        deck: &[(usize, u8)],
        _commanders: &[usize],
        pool: &CardPool,
        _seeds: &[u64],
        _turn_horizon: u8,
        _context: &metagene_core::StructuralEvaluationContext,
    ) -> Fitness {
        let package_cards = deck
            .iter()
            .filter(|(index, _)| {
                pool.cards[*index]
                    .features
                    .tags
                    .iter()
                    .any(|tag| tag == "engine")
            })
            .map(|(_, count)| *count as usize)
            .sum::<usize>();
        Fitness {
            consistency: 0.5,
            synergy: (package_cards as f64 / 16.0).min(1.0),
            ..Fitness::default()
        }
    }
}

fn card(index: usize, land: bool, package: bool) -> Card {
    Card {
        id: CardId(format!("card-{index}")),
        arena_id: Some(index as u64 + 1),
        platforms: vec!["arena".into()],
        name: format!("Benchmark Card {index}"),
        set_code: "tst".into(),
        collector_number: index.to_string(),
        type_line: if land { "Basic Land" } else { "Spell" }.into(),
        oracle_text: String::new(),
        colors: vec![Color::Green],
        color_identity: vec![Color::Green],
        image_uri: None,
        legalities: BTreeMap::new(),
        features: CardFeatures {
            is_land: land,
            unlimited_copies: land,
            mana_value: if land { 0.0 } else { 2.0 },
            mana_production: if land { 1.0 } else { 0.0 },
            tags: if package {
                vec!["engine".into()]
            } else {
                Vec::new()
            },
            ..CardFeatures::default()
        },
    }
}

fn run(pool: &CardPool, seed: &Deck, package_rate: f32) -> (f64, f64) {
    let config = EvolutionConfig {
        population_size: 256,
        generations: 5,
        elite_count: 16,
        trials_per_candidate: 1,
        mutation_swaps: 2,
        mutation_rate: 1.0,
        package_mutation_rate: package_rate,
        package_size: 4,
        seed: 0x5041_434B_4147_4521,
        ..EvolutionConfig::default()
    };
    let started = Instant::now();
    let result = EvolutionEngine::new(PackageEvaluator)
        .run(
            seed,
            pool,
            &DeckConstraints::default(),
            &config,
            &CancellationToken::default(),
            |_| {},
        )
        .expect("benchmark evolution should complete");
    let elapsed = started.elapsed().as_secs_f64();
    (
        result.frontier[0].fitness.synergy,
        result.unique_evaluations as f64 / elapsed.max(f64::EPSILON),
    )
}

fn main() {
    let cards = (0..160)
        .map(|index| card(index, index < 24, (24..32).contains(&index)))
        .collect::<Vec<_>>();
    let pool = CardPool { cards };
    let seed = Deck {
        name: "Package benchmark".into(),
        command_zone: Vec::new(),
        mainboard: (0..24)
            .map(|index| DeckEntry {
                card_id: CardId(format!("card-{index}")),
                quantity: 1,
            })
            .chain((32..41).map(|index| DeckEntry {
                card_id: CardId(format!("card-{index}")),
                quantity: 4,
            }))
            .collect(),
        sideboard: Vec::new(),
    };
    let (independent_quality, independent_speed) = run(&pool, &seed, 0.0);
    let (package_quality, package_speed) = run(&pool, &seed, 1.0);
    assert!(
        package_quality > independent_quality,
        "package operator should converge further than independent swaps"
    );
    println!(
        "independent_synergy={independent_quality:.3} package_synergy={package_quality:.3} independent_candidates_per_second={independent_speed:.0} package_candidates_per_second={package_speed:.0}"
    );
}
