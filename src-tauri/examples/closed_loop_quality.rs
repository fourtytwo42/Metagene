use metagene_app_lib::{catalog, forge, simulator};
use metagene_core::{
    CancellationToken, CardPool, Deck, DeckConstraints, DeckEntry, EvolutionConfig,
    EvolutionEngine, Fitness, RankedDeck, evaluate_structural_deck, export_mtga, import_decklist,
    validate_deck_legality,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

const IZZET_WIZARDS: &[(&str, u8)] = &[
    ("Soul-Scar Mage", 4),
    ("A-Symmetry Sage", 4),
    ("Slickshot Show-Off", 1),
    ("Dreadhorde Arcanist", 4),
    ("Balmor, Battlemage Captain", 3),
    ("Spell Pierce", 2),
    ("Consider", 4),
    ("Wizard's Lightning", 4),
    ("Flame of Anor", 4),
    ("Reckless Charge", 4),
    // Shove Aside is not yet in the managed HexDek corpus. Play with Fire
    // keeps this benchmark parser-complete while preserving the one-mana
    // interaction/face-damage role used by the current shell.
    ("Play with Fire", 4),
    ("Expressive Iteration", 2),
    ("Mountain", 1),
    ("Spirebluff Canal", 4),
    ("Fiery Islet", 3),
    ("Riverglide Pathway // Lavaglide Pathway", 4),
    ("Den of the Bugbear", 2),
    ("Steam Vents", 4),
    ("Otawara, Soaring City", 1),
    ("Sokenzan, Crucible of Defiance", 1),
];

const IZZET_AFFINITY: &[(&str, u8)] = &[
    ("Ornithopter", 4),
    ("Tamiyo, Inquisitive Student // Tamiyo, Seasoned Scholar", 2),
    ("Emry, Lurker of the Loch", 4),
    ("Pinnacle Emissary", 4),
    ("Kappa Cannoneer", 4),
    ("Mox Amber", 4),
    ("Tormod's Crypt", 2),
    // Managed HexDek does not yet ship Drix Interlacer or Machinist's
    // Dismissal. These parser-complete artifacts preserve the cheap artifact
    // density and interaction roles for the independent benchmark.
    ("Patchwork Automaton", 4),
    ("Springleaf Drum", 4),
    ("Sink into Stupor // Soporific Springs", 2),
    ("Metallic Rebuke", 2),
    ("Thoughtcast", 4),
    ("Weapons Manufacturing", 4),
    ("Island", 1),
    ("Seat of the Synod", 4),
    ("Great Furnace", 4),
    ("Steam Vents", 2),
    ("Spirebluff Canal", 2),
    ("Fiery Islet", 2),
    ("Otawara, Soaring City", 1),
];

// SkyBLUEkepl, Arena Historic ladder, 5-3, 2026-01-24. This gives the
// calibration field a non-Izzet, discard/removal-driven midrange axis.
const JUND_MIDRANGE: &[(&str, u8)] = &[
    ("Ignoble Hierarch", 4),
    ("Barrowgoyf", 4),
    ("Detective's Phoenix", 1),
    ("Fable of the Mirror-Breaker // Reflection of Kiki-Jiki", 1),
    // Managed HexDek lacks Jarsyl. Grim Flayer preserves the cheap
    // graveyard/value-threat role without adding another color.
    ("Grim Flayer", 3),
    ("Mutable Explorer", 3),
    ("Slimefoot and Squee", 1),
    ("Pyrogoyf", 4),
    ("Fatal Push", 3),
    ("Bloodchief's Thirst", 1),
    ("Inquisition of Kozilek", 1),
    ("Thoughtseize", 4),
    ("Malevolent Rumble", 4),
    ("Unholy Annex // Ritual Chamber", 3),
    ("Forest", 2),
    ("Swamp", 2),
    ("Mountain", 1),
    ("Blood Crypt", 2),
    ("Grove of the Burnwillows", 1),
    ("Overgrown Tomb", 3),
    ("Blackcleave Cliffs", 2),
    ("Copperline Gorge", 2),
    ("Blooming Marsh", 4),
    ("Prismatic Vista", 4),
];

#[derive(Deserialize)]
struct PoorOutputFixture {
    candidates: Vec<PoorOutputCandidate>,
}

#[derive(Deserialize)]
struct PoorOutputCandidate {
    id: usize,
    deck: Vec<String>,
}

fn resolve_deck(
    name: &str,
    entries: &[(&str, u8)],
    pool: &CardPool,
) -> Result<Deck, Box<dyn std::error::Error>> {
    let by_name = pool
        .cards
        .iter()
        .map(|card| (card.name.to_ascii_lowercase(), card))
        .collect::<HashMap<_, _>>();
    let mut mainboard = Vec::with_capacity(entries.len());
    for (card_name, quantity) in entries {
        let card = by_name
            .get(&card_name.to_ascii_lowercase())
            .ok_or_else(|| format!("{name}: card not found in legal pool: {card_name}"))?;
        mainboard.push(DeckEntry {
            card_id: card.id.clone(),
            quantity: *quantity,
        });
    }
    let deck = Deck {
        name: name.into(),
        command_zone: vec![],
        mainboard,
        sideboard: vec![],
    };
    if deck.mainboard_count() != 60 {
        return Err(format!("{name}: expected 60 cards, got {}", deck.mainboard_count()).into());
    }
    Ok(deck)
}

fn print_structural(label: &str, deck: &Deck, pool: &CardPool, seed: u64) -> Fitness {
    let fitness =
        evaluate_structural_deck(deck, pool, seed, 256, 8).expect("structural evaluation");
    println!(
        "{label}: structural={:.3} severe={} mana_fail={:.1}% on_curve={:.1}% horizon={:.1}% synergy={:.1}% interaction={:.1}% closing={:.1}% roles={:.1}% actions={:.1}% dead={:.1}% narrow={:.1}% excess={:.1}% unmet={:.1}% colors={} warnings={}",
        fitness.score,
        fitness.severe_mana_failure,
        fitness.mana_failure_rate * 100.0,
        fitness.on_curve_castability * 100.0,
        fitness.turn_horizon_castability * 100.0,
        fitness.synergy * 100.0,
        fitness.interaction * 100.0,
        fitness.closing_power * 100.0,
        fitness.role_balance * 100.0,
        fitness.functional_action_rate * 100.0,
        fitness.dead_action_rate * 100.0,
        fitness.narrow_mainboard_rate * 100.0,
        fitness.copy_excess_rate * 100.0,
        fitness.unmet_requirement_rate * 100.0,
        fitness.required_color_count,
        fitness.synergy_warnings.join(" | ")
    );
    let mut actions = fitness.action_evidence.iter().collect::<Vec<_>>();
    actions.sort_by(|left, right| right.contribution.total_cmp(&left.contribution));
    for action in actions.into_iter().take(12) {
        println!(
            "  {} x{} {} availability={:.0}% contribution={:.2} narrow={} ({})",
            action.card_name,
            action.copies,
            action.kind,
            action.availability * 100.0,
            action.contribution,
            action.narrow,
            action.reasons.join("; ")
        );
    }
    fitness
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::var_os("METAGENE_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .map(|path| path.join("dev.metagene.app"))
        })
        .ok_or("APPDATA and METAGENE_DATA_DIR are not defined")?;
    let legal = CardPool {
        cards: catalog::legal_pool(&data_dir, "arena", "historic")?,
    };
    let parser_complete = simulator::filter_pool_for_hexdek(&data_dir, &legal, "parser_complete")?;
    let izzet = resolve_deck("Current Historic Izzet Wizards", IZZET_WIZARDS, &legal)?;
    let affinity = resolve_deck("Current Historic Izzet Affinity", IZZET_AFFINITY, &legal)?;
    let jund = resolve_deck("Current Historic Jund Midrange", JUND_MIDRANGE, &legal)?;
    let constraints = DeckConstraints {
        target_mainboard_size: 60,
        target_sideboard_size: 0,
        minimum_commanders: 0,
        maximum_commanders: 0,
        default_max_copies: 4,
        locked_cards: vec![],
        minimum_lands: Some(20),
        maximum_lands: Some(28),
        allowed_colors: None,
    };
    validate_deck_legality(&izzet, &legal, &constraints)?;
    validate_deck_legality(&affinity, &legal, &constraints)?;
    validate_deck_legality(&jund, &legal, &constraints)?;
    let competitive_scores = [
        print_structural("Izzet Wizards", &izzet, &legal, 0x1A2B),
        print_structural("Izzet Affinity", &affinity, &legal, 0x3C4D),
        print_structural("Jund Midrange", &jund, &legal, 0x5E6F),
    ];
    let poor_fixture: PoorOutputFixture = serde_json::from_str(include_str!(
        "../../data/evaluation/closed-loop-quality-v11-candidates.json"
    ))?;
    let mut poor_scores = Vec::new();
    for candidate in poor_fixture.candidates {
        let deck_text = format!("Deck\n{}", candidate.deck.join("\n"));
        let mut deck = import_decklist(&deck_text, &legal)?;
        deck.name = format!("100k regression candidate {}", candidate.id);
        validate_deck_legality(&deck, &legal, &constraints)?;
        let fitness = print_structural(
            &format!("Poor 100k candidate {}", candidate.id),
            &deck,
            &legal,
            0xBAD0 + candidate.id as u64,
        );
        let tormod = fitness
            .copy_evidence
            .iter()
            .find(|evidence| evidence.card_name == "Tormod's Crypt")
            .expect("Tormod copy evidence");
        assert_eq!(tormod.functional_target, 1);
        poor_scores.push(fitness.score);
    }
    let competitive_ceiling = competitive_scores
        .iter()
        .map(|fitness| fitness.score)
        .fold(f64::NEG_INFINITY, f64::max);
    assert!(
        poor_scores.iter().all(|score| *score < 0.45),
        "known poor output still receives a plausible competitive screen score: {poor_scores:?}"
    );
    assert!(
        competitive_ceiling > 0.0,
        "all tournament references were structurally rejected: {competitive_scores:?}"
    );

    let deck_refs = [&izzet, &affinity, &jund];
    let coverage = simulator::audit_hexdek_coverage(
        &data_dir,
        &simulator::coverage_inputs_from_decks(&deck_refs, &legal)?,
    )?;
    for report in &coverage.decks {
        println!(
            "coverage {}: complete={}/{} partial={} missing={} risks={}",
            report.deck_name,
            report.parser_complete_unique,
            report.unique_cards,
            report.partial_unique,
            report.missing_unique,
            report.semantic_risk_unique
        );
        for issue in &report.issues {
            println!(
                "  {}: {} — {}",
                issue.status,
                issue.name,
                issue.parse_errors.join("; ")
            );
        }
    }

    let seed_choice = std::env::args().nth(1).unwrap_or_else(|| "izzet".into());
    let competitive_seed = match seed_choice.as_str() {
        "izzet" => izzet.clone(),
        "jund" => jund.clone(),
        other => return Err(format!("unknown seed '{other}'; use izzet or jund").into()),
    };
    println!("competitive seed: {}", competitive_seed.name);
    let trusted_ids = competitive_seed
        .mainboard
        .iter()
        .map(|entry| entry.card_id.clone())
        .collect::<std::collections::HashSet<_>>();
    let evolution_pool = CardPool {
        cards: parser_complete
            .cards
            .iter()
            .filter(|card| trusted_ids.contains(&card.id))
            .cloned()
            .collect(),
    };
    println!(
        "competitive mutation pool: {} seed-archetype card printings",
        evolution_pool.cards.len()
    );
    let started = Instant::now();
    let evolved = EvolutionEngine::default().run(
        &competitive_seed,
        &evolution_pool,
        &constraints,
        &EvolutionConfig {
            population_size: 128,
            generations: 20,
            elite_count: 12,
            trials_per_candidate: 96,
            worker_threads: Some(30),
            seed: 0xC105_ED10_0A11_7A11,
            ..EvolutionConfig::default()
        },
        &CancellationToken::default(),
        |_| {},
    )?;
    println!(
        "evolution: slots={} unique={} reused={} elapsed={:.2}s frontier={}",
        evolved.evaluated_candidates,
        evolved.unique_evaluations,
        evolved.evaluations_avoided,
        started.elapsed().as_secs_f64(),
        evolved.frontier.len()
    );
    for candidate in evolved.frontier.iter().take(3) {
        let _ = print_structural(
            &format!("Generated rank {}", candidate.rank),
            &candidate.deck,
            &parser_complete,
            0x9000 + candidate.rank as u64,
        );
        println!("{}", export_mtga(&candidate.deck, &parser_complete));
    }

    let complete_names = parser_complete
        .cards
        .iter()
        .map(|card| card.name.to_ascii_lowercase())
        .collect::<std::collections::HashSet<_>>();
    let opponents = [izzet, affinity, jund]
        .into_iter()
        .filter(|deck| {
            deck.mainboard.iter().all(|entry| {
                legal
                    .card(&entry.card_id)
                    .is_some_and(|card| complete_names.contains(&card.name.to_ascii_lowercase()))
            })
        })
        .collect::<Vec<_>>();
    if opponents.len() == 3 {
        let validation_opponents = opponents.clone();
        let mut candidates = evolved
            .frontier
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<RankedDeck>>();
        let baseline_rank = evolved
            .frontier
            .iter()
            .map(|candidate| candidate.rank)
            .max()
            .unwrap_or_default()
            + 1;
        candidates.push(RankedDeck {
            rank: baseline_rank,
            pareto_rank: usize::MAX,
            deck: competitive_seed.clone(),
            fitness: evaluate_structural_deck(&competitive_seed, &legal, 0xBA5E_11AE, 256, 8)?,
            packages: vec![],
            commander_compatibility: None,
            candidate_status: "baseline".into(),
        });
        let validation_candidates = candidates.clone();
        let halving = simulator::run_successive_halving(
            &data_dir,
            &simulator::SuccessiveHalvingRequest {
                candidates,
                opponents: validation_opponents.clone(),
                opponent_weights: vec![],
                pool: parser_complete,
                base_games: 64,
                rounds: 2,
                seed: 0x5151_2026,
                workers: 30,
                seats: 2,
                hat_budget: 0,
                max_turns: 30,
                game_timeout_seconds: 3,
                structural_weight: 0.2,
                risk_aversion: 0.5,
                worst_matchup_weight: 0.25,
                seat_position_weight: 0.25,
                adaptive_elimination: true,
                commander: false,
                route_plan: None,
            },
            |_| {},
        )?;
        let raw_winner_rank = halving.winner_rank;
        let incumbent = halving
            .rounds
            .iter()
            .rev()
            .flat_map(|round| round.results.iter())
            .find(|result| result.candidate_rank == baseline_rank);
        let challenger = raw_winner_rank.and_then(|rank| {
            halving
                .rounds
                .iter()
                .rev()
                .flat_map(|round| round.results.iter())
                .find(|result| result.candidate_rank == rank)
        });
        let selected_winner_rank = match (raw_winner_rank, challenger, incumbent) {
            (Some(rank), Some(challenger), Some(incumbent))
                if rank != baseline_rank
                    && (challenger.evidence_score < incumbent.evidence_score + 0.03
                        || challenger.evidence_lower_bound < incumbent.evidence_lower_bound) =>
            {
                Some(baseline_rank)
            }
            (rank, _, _) => rank,
        };
        println!(
            "validation: raw_winner={:?} selected_winner={:?} incumbent={} games={} attempts={} crashes={} elapsed={}ms",
            raw_winner_rank,
            selected_winner_rank,
            baseline_rank,
            halving.total_games,
            halving.total_attempts,
            halving.total_crashes,
            halving.elapsed_ms
        );
        for round in &halving.rounds {
            for result in &round.results {
                println!(
                    "  round={} rank={} win={:.1}% lower={:.1}% worst={:.1}% evidence={:.3} error={}",
                    round.round,
                    result.candidate_rank,
                    result.game_win_rate * 100.0,
                    result.win_rate_lower_bound * 100.0,
                    result.worst_matchup_win_rate * 100.0,
                    result.evidence_score,
                    result.error.as_deref().unwrap_or("none")
                );
            }
        }
        if let Some(winner_rank) = selected_winner_rank {
            let forge_candidates = validation_candidates
                .iter()
                .filter(|candidate| {
                    candidate.rank == winner_rank || candidate.rank == baseline_rank
                })
                .collect::<Vec<_>>();
            for winner in forge_candidates {
                match forge::run_validation(
                    &data_dir,
                    &simulator::HexdekValidationRequest {
                        candidate: winner.deck.clone(),
                        opponents: validation_opponents.clone(),
                        opponent_weights: vec![],
                        pool: legal.clone(),
                        games: 4,
                        seed: 0xF043_2026,
                        workers: 4,
                        seats: 2,
                        hat_budget: 0,
                        max_turns: 30,
                        game_timeout_seconds: 2,
                        commander: false,
                        fixed_order: false,
                    },
                ) {
                    Ok(report) => {
                        let row = report
                            .rows
                            .iter()
                            .find(|row| row.deck.starts_with("Candidate · "));
                        println!(
                            "forge cross-check rank={}: games={} crashes={} candidate_win={:.1}% cached={}",
                            winner.rank,
                            report.games,
                            report.crashes,
                            row.map_or(0.0, |row| row.win_rate * 100.0),
                            report.cached_games
                        );
                    }
                    Err(error) => println!(
                        "forge cross-check rank={} unavailable: {error}",
                        winner.rank
                    ),
                }
            }
        }
    } else {
        println!("validation skipped: a current Historic benchmark is not parser-complete");
    }
    Ok(())
}
