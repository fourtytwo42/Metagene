use metagene_core::{
    CancellationToken, Card, CardFeatures, CardId, CardPool, Color, Deck, DeckConstraints,
    DeckEntry, Evaluator, EvolutionConfig, EvolutionEngine, Fitness,
};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

struct HeldOutEvaluator;

impl Evaluator for HeldOutEvaluator {
    fn evaluate(
        &self,
        deck: &[(usize, u8)],
        _commanders: &[usize],
        pool: &CardPool,
        _seeds: &[u64],
        _turn_horizon: u8,
        _context: &metagene_core::StructuralEvaluationContext,
    ) -> Fitness {
        let package = deck
            .iter()
            .filter(|(index, _)| (24..32).contains(index))
            .map(|(_, count)| *count as usize)
            .sum::<usize>() as f64;
        let blue_sources = deck
            .iter()
            .filter(|(index, _)| {
                let card = &pool.cards[*index];
                card.features.is_land && card.color_identity.contains(&Color::Blue)
            })
            .map(|(_, count)| *count as usize)
            .sum::<usize>() as f64;
        Fitness {
            synergy: (package / 16.0).min(1.0),
            consistency: (blue_sources / 24.0).min(1.0),
            ..Fitness::default()
        }
    }
}

fn card(index: usize) -> Card {
    let is_land = index < 24;
    let blue_land = (12..24).contains(&index);
    Card {
        id: CardId(format!("stage5-card-{index}")),
        arena_id: Some(index as u64 + 1),
        platforms: vec!["arena".into()],
        name: format!("Stage 5 Card {index}"),
        set_code: "tst".into(),
        collector_number: index.to_string(),
        type_line: if is_land { "Basic Land" } else { "Creature" }.into(),
        oracle_text: String::new(),
        colors: if is_land {
            Vec::new()
        } else {
            vec![Color::Blue]
        },
        color_identity: if is_land {
            vec![if blue_land { Color::Blue } else { Color::Red }]
        } else {
            vec![Color::Blue]
        },
        image_uri: None,
        legalities: BTreeMap::new(),
        features: CardFeatures {
            is_land,
            unlimited_copies: is_land,
            mana_value: if is_land { 0.0 } else { 3.0 },
            mana_production: if is_land { 1.0 } else { 0.0 },
            ..CardFeatures::default()
        },
    }
}

fn run(pool: &CardPool, seed: &Deck, enhanced: bool) -> (f64, usize, Duration) {
    let config = EvolutionConfig {
        population_size: 256,
        generations: 5,
        elite_count: 16,
        trials_per_candidate: 1,
        mutation_swaps: 2,
        mutation_rate: 1.0,
        package_mutation_rate: if enhanced { 1.0 } else { 0.0 },
        package_size: 4,
        mana_mutation_rate: if enhanced { 1.0 } else { 0.0 },
        learned_packages: if enhanced {
            vec![
                (24..32)
                    .map(|index| CardId(format!("stage5-card-{index}")))
                    .collect(),
            ]
        } else {
            Vec::new()
        },
        seed: 0x5354_4147_4535_4F50,
        ..EvolutionConfig::default()
    };
    let started = Instant::now();
    let result = EvolutionEngine::new(HeldOutEvaluator)
        .run(
            seed,
            pool,
            &DeckConstraints::default(),
            &config,
            &CancellationToken::default(),
            |_| {},
        )
        .expect("fixed Stage 5 optimizer benchmark should complete");
    (
        result.frontier[0].fitness.score,
        result.unique_evaluations as usize,
        started.elapsed(),
    )
}

fn main() {
    let pool = CardPool {
        cards: (0..160).map(card).collect(),
    };
    let seed = Deck {
        name: "Stage 5 fixed operator seed".into(),
        command_zone: Vec::new(),
        mainboard: vec![DeckEntry {
            card_id: CardId("stage5-card-0".into()),
            quantity: 24,
        }]
        .into_iter()
        .chain((40..49).map(|index| DeckEntry {
            card_id: CardId(format!("stage5-card-{index}")),
            quantity: 4,
        }))
        .collect(),
        sideboard: Vec::new(),
    };
    let (baseline, baseline_evaluations, baseline_elapsed) = run(&pool, &seed, false);
    let (enhanced, enhanced_evaluations, enhanced_elapsed) = run(&pool, &seed, true);
    assert_eq!(baseline_evaluations, enhanced_evaluations);
    assert!(
        enhanced > baseline,
        "guided operators must improve held-out quality"
    );
    let wall_clock_budget = Duration::from_secs(5);
    assert!(baseline_elapsed <= wall_clock_budget && enhanced_elapsed <= wall_clock_budget);
    println!(
        "scenario=stage5-fixed-operators evaluations_each={} wall_clock_budget_ms={} baseline_quality={baseline:.3} enhanced_quality={enhanced:.3} baseline_ms={} enhanced_ms={}",
        baseline_evaluations,
        wall_clock_budget.as_millis(),
        baseline_elapsed.as_millis(),
        enhanced_elapsed.as_millis(),
    );
}
