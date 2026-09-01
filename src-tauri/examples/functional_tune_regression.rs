use metagene_app_lib::catalog;
use metagene_core::{
    CancellationToken, CardPool, Deck, DeckConstraints, EvolutionConfig, EvolutionEngine,
    EvolutionMode, StructuralEvaluationContext, TunePolicy, evaluate_tune_deck_with_context,
    export_mtga, import_decklist, strategy_signature_for_deck, validate_deck_legality,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Deserialize)]
struct Fixture {
    root: Vec<String>,
    candidates: Vec<FixtureCandidate>,
}

#[derive(Deserialize)]
struct FixtureCandidate {
    id: usize,
    deck: Vec<String>,
}

fn import_lines(name: &str, lines: &[String], pool: &CardPool) -> Deck {
    let mut deck = import_decklist(&format!("Deck\n{}", lines.join("\n")), pool)
        .unwrap_or_else(|error| panic!("{name} import failed: {error}"));
    deck.name = name.into();
    deck
}

fn copy_edit_distance(left: &Deck, right: &Deck, pool: &CardPool) -> usize {
    let names = |deck: &Deck| {
        let mut counts = HashMap::<String, usize>::new();
        for entry in &deck.mainboard {
            let card = pool.card(&entry.card_id).expect("fixture card in pool");
            if !card.features.is_land {
                *counts.entry(card.name.to_ascii_lowercase()).or_default() +=
                    usize::from(entry.quantity);
            }
        }
        counts
    };
    let left = names(left);
    let right = names(right);
    left.keys()
        .chain(right.keys())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .map(|name| {
            left.get(name)
                .copied()
                .unwrap_or_default()
                .abs_diff(right.get(name).copied().unwrap_or_default())
        })
        .sum::<usize>()
        .div_ceil(2)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .map(|path| path.join("dev.metagene.app"))
        })
        .ok_or("usage: functional_tune_regression [application-data-directory] [generations]")?;
    let generations = std::env::args()
        .nth(2)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(2_000);
    let fixture: Fixture = serde_json::from_str(include_str!(
        "../../data/evaluation/functional-actions-tune-v6-candidates.json"
    ))?;
    let pool = CardPool {
        cards: catalog::legal_pool(&data_dir, "arena", "historic")?,
    };
    let root = import_lines("Original Simic copy/token root", &fixture.root, &pool);
    let constraints = DeckConstraints::default();
    validate_deck_legality(&root, &pool, &constraints)?;
    let context = StructuralEvaluationContext {
        platform: "arena".into(),
        format: "historic".into(),
        ..StructuralEvaluationContext::default()
    };
    let policy = TunePolicy {
        root_deck: Some(root.clone()),
        strategy_retention_floor: 0.70,
        primary_package_floor: 0.60,
        maximum_nonland_edit_fraction: 0.40,
    };
    let signature = strategy_signature_for_deck(&root, &pool)?;
    for package in ["copy", "tokens", "blink-etb"] {
        assert!(
            signature
                .packages
                .iter()
                .any(|entry| entry.package == package && entry.primary),
            "root did not detect primary {package} package: {:?}",
            signature
                .packages
                .iter()
                .map(|entry| (&entry.package, entry.primary, entry.root_support))
                .collect::<Vec<_>>()
        );
    }

    for fixture_candidate in &fixture.candidates {
        let deck = import_lines(
            &format!("Submitted artifact shell {}", fixture_candidate.id),
            &fixture_candidate.deck,
            &pool,
        );
        validate_deck_legality(&deck, &pool, &constraints)?;
        let (fitness, _) = evaluate_tune_deck_with_context(
            &deck,
            &root,
            &pool,
            0xF00D_0000 + fixture_candidate.id as u64,
            128,
            8,
            &context,
            &policy,
            &EvolutionConfig::default().fitness_weights,
        )?;
        assert!(fitness.severe_structural_failure);
        assert!(
            fitness
                .strategy_packages
                .iter()
                .filter(|package| package.primary)
                .any(|package| package.retention < 0.60)
        );
        if fixture_candidate.id == 1 {
            let dross = fitness
                .action_evidence
                .iter()
                .find(|evidence| {
                    evidence.card_name == "Dross Skullbomb" && evidence.kind.contains("recursion")
                })
                .expect("Dross recursion evidence");
            let surgical = fitness
                .action_evidence
                .iter()
                .find(|evidence| {
                    evidence.card_name == "Surgical Skullbomb" && evidence.kind.contains("bounce")
                })
                .expect("Surgical bounce evidence");
            assert_eq!(dross.mana_availability, 0.0);
            assert_eq!(surgical.mana_availability, 0.0);
            assert!(fitness.dead_action_rate > 0.20);
            assert!(
                fitness.closing_power < 0.50,
                "artifact shell closing power was {:.3} ({})",
                fitness.closing_power,
                fitness.archetype
            );
        }
    }

    if generations == 0 {
        println!(
            "functional fixture score gate passed; root packages: {}",
            signature
                .packages
                .iter()
                .filter(|package| package.primary)
                .map(|package| package.package.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        return Ok(());
    }

    let started = Instant::now();
    let result = EvolutionEngine::default().run_with_context(
        &root,
        &pool,
        &constraints,
        &EvolutionConfig {
            population_size: 64,
            generations,
            elite_count: 8,
            trials_per_candidate: 32,
            worker_threads: Some(
                std::thread::available_parallelism()
                    .map(|threads| threads.get().saturating_sub(2).max(1))
                    .unwrap_or(1),
            ),
            seed: 0xF00D_CAFE_0110,
            evolution_mode: EvolutionMode::Tune,
            tune_policy: policy,
            ..EvolutionConfig::default()
        },
        &CancellationToken::default(),
        &context,
        |_| {},
    )?;
    let baseline = result.baseline.as_ref().expect("Tune baseline is pinned");
    assert_eq!(baseline.rank, 0);
    assert_eq!(baseline.deck.mainboard, root.mainboard);
    assert!(!result.frontier.is_empty());
    for candidate in &result.frontier {
        assert!(!candidate.fitness.severe_structural_failure);
        assert!(candidate.fitness.strategy_similarity >= 0.70);
        assert!(candidate.fitness.nonland_edit_fraction <= 0.40);
        assert!(
            candidate
                .fitness
                .strategy_packages
                .iter()
                .filter(|package| package.primary)
                .all(|package| package.retention >= 0.60)
        );
    }
    let minimum_distance = 12;
    for left in 0..result.frontier.len() {
        for right in left + 1..result.frontier.len() {
            assert!(
                copy_edit_distance(
                    &result.frontier[left].deck,
                    &result.frontier[right].deck,
                    &pool,
                ) >= minimum_distance,
                "visible Tune candidates {} and {} are near-clones",
                result.frontier[left].rank,
                result.frontier[right].rank
            );
        }
    }
    println!(
        "{} generations · {} slots · {} unique · {:.2}s · {} diverse finalists",
        result.generations_completed,
        result.evaluated_candidates,
        result.unique_evaluations,
        started.elapsed().as_secs_f64(),
        result.frontier.len()
    );
    for candidate in &result.frontier {
        println!(
            "rank {} screen {:.3} retention {:.1}% edit {:.1}% actions {:.1}% closing {:.1}%",
            candidate.rank,
            candidate.fitness.score,
            candidate.fitness.strategy_similarity * 100.0,
            candidate.fitness.nonland_edit_fraction * 100.0,
            candidate.fitness.functional_action_rate * 100.0,
            candidate.fitness.closing_power * 100.0,
        );
        println!("{}", export_mtga(&candidate.deck, &pool));
    }
    Ok(())
}
