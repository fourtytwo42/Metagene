use metagene_core::{
    CancellationToken, Card, CardFeatures, CardId, CardPool, Color, Deck, DeckConstraints,
    DeckEntry, Evaluator, EvolutionConfig, EvolutionEngine, Fitness, LockedCard,
};
use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Instant;

struct CountingEvaluator {
    calls: Arc<AtomicUsize>,
}

impl Evaluator for CountingEvaluator {
    fn evaluate(
        &self,
        _deck: &[(usize, u8)],
        _commanders: &[usize],
        _pool: &CardPool,
        _seeds: &[u64],
        _turn_horizon: u8,
        _context: &metagene_core::StructuralEvaluationContext,
    ) -> Fitness {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Fitness {
            consistency: 1.0,
            ..Fitness::default()
        }
    }
}

fn main() {
    let forest_id = CardId("forest".into());
    let pool = CardPool {
        cards: vec![Card {
            id: forest_id.clone(),
            arena_id: Some(1),
            platforms: vec!["arena".into()],
            name: "Forest".into(),
            set_code: "tst".into(),
            collector_number: "1".into(),
            type_line: "Basic Land — Forest".into(),
            oracle_text: "{T}: Add {G}.".into(),
            colors: Vec::new(),
            color_identity: vec![Color::Green],
            image_uri: None,
            legalities: BTreeMap::new(),
            features: CardFeatures {
                is_land: true,
                unlimited_copies: true,
                mana_production: 1.0,
                ..CardFeatures::default()
            },
        }],
    };
    let seed = Deck {
        name: "Locked duplicate benchmark".into(),
        command_zone: Vec::new(),
        mainboard: vec![DeckEntry {
            card_id: forest_id.clone(),
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
        locked_cards: vec![LockedCard {
            card_id: forest_id,
            minimum: 60,
            maximum: 60,
        }],
        minimum_lands: Some(60),
        maximum_lands: Some(60),
        allowed_colors: None,
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let config = EvolutionConfig {
        population_size: 4_096,
        generations: 20,
        elite_count: 64,
        trials_per_candidate: 64,
        mutation_rate: 1.0,
        ..EvolutionConfig::default()
    };
    let started = Instant::now();
    let result = EvolutionEngine::new(CountingEvaluator {
        calls: calls.clone(),
    })
    .run(
        &seed,
        &pool,
        &constraints,
        &config,
        &CancellationToken::default(),
        |_| {},
    )
    .expect("duplicate benchmark should complete");
    let elapsed = started.elapsed().as_secs_f64();
    assert_eq!(result.evaluated_candidates, 81_920);
    assert_eq!(result.unique_evaluations, 20);
    assert_eq!(result.evaluations_avoided, 81_900);
    assert_eq!(calls.load(Ordering::Relaxed), 20);
    println!(
        "population_slots={} unique_evaluations={} evaluations_avoided={} reuse={:.2}% elapsed_ms={:.1}",
        result.evaluated_candidates,
        result.unique_evaluations,
        result.evaluations_avoided,
        result.evaluations_avoided as f64 / result.evaluated_candidates as f64 * 100.0,
        elapsed * 1_000.0,
    );
}
