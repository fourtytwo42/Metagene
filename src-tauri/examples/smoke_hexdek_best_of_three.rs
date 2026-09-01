use metagene_app_lib::{catalog, sideboard_evolution, simulator};
use metagene_core::{
    CancellationToken, Card, CardPool, Deck, DeckConstraints, DeckEntry, MatchupSideboardPlan,
    SideboardExchange,
};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn exact_card(data_dir: &Path, name: &str) -> Result<Card, Box<dyn std::error::Error>> {
    catalog::search(data_dir, name, "paper", "legacy", 20)?
        .into_iter()
        .find(|card| card.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| format!("Legacy catalog does not contain {name}").into())
}

fn entry(card: &Card, quantity: u8) -> DeckEntry {
    DeckEntry {
        card_id: card.id.clone(),
        quantity,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is not defined")?
        .join("dev.metagene.app");
    let names = [
        "Forest",
        "Island",
        "Mountain",
        "Llanowar Elves",
        "Elvish Mystic",
        "Grizzly Bears",
        "Runeclaw Bear",
        "Centaur Courser",
        "Giant Spider",
        "Craw Wurm",
        "Colossal Dreadmaw",
        "Giant Growth",
        "Naturalize",
        "Fog",
    ];
    let cards = names
        .iter()
        .map(|name| exact_card(&data_dir, name))
        .collect::<Result<Vec<_>, _>>()?;
    let card = |name: &str| {
        cards
            .iter()
            .find(|card| card.name == name)
            .expect("loaded above")
    };
    let candidate = Deck {
        name: "Green creatures".into(),
        command_zone: vec![],
        mainboard: std::iter::once(entry(card("Forest"), 24))
            .chain(
                [
                    "Llanowar Elves",
                    "Elvish Mystic",
                    "Grizzly Bears",
                    "Runeclaw Bear",
                    "Centaur Courser",
                    "Giant Spider",
                    "Craw Wurm",
                    "Colossal Dreadmaw",
                    "Giant Growth",
                ]
                .into_iter()
                .map(|name| entry(card(name), 4)),
            )
            .collect(),
        sideboard: vec![entry(card("Naturalize"), 4), entry(card("Fog"), 4)],
    };
    let opponent = Deck {
        name: "Island control group".into(),
        command_zone: vec![],
        mainboard: vec![entry(card("Island"), 60)],
        sideboard: vec![],
    };
    let opponent_key = simulator::sideboard_opponent_key(0, &opponent)?;
    let mut request = simulator::BestOfThreeValidationRequest {
        candidate,
        opponents: vec![opponent],
        opponent_weights: vec![1.0],
        pool: CardPool {
            cards: cards.clone(),
        },
        constraints: DeckConstraints {
            target_mainboard_size: 60,
            target_sideboard_size: 8,
            minimum_commanders: 0,
            maximum_commanders: 0,
            default_max_copies: 4,
            locked_cards: vec![],
            minimum_lands: None,
            maximum_lands: None,
            allowed_colors: None,
        },
        sideboard_plans: vec![MatchupSideboardPlan {
            opponent_key,
            on_play: SideboardExchange {
                cards_in: vec![entry(card("Naturalize"), 4)],
                cards_out: vec![entry(card("Giant Spider"), 4)],
            },
            on_draw: SideboardExchange {
                cards_in: vec![entry(card("Fog"), 4)],
                cards_out: vec![entry(card("Giant Spider"), 4)],
            },
        }],
        matches: 4,
        seed: SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_nanos()
            .min(i64::MAX as u128) as i64,
        workers: 8,
        hat_budget: 8,
        max_turns: 60,
        game_timeout_seconds: 30,
    };

    let evolution = sideboard_evolution::evolve_sideboard_policies(
        &data_dir,
        &sideboard_evolution::SideboardEvolutionRequest {
            candidate: request.candidate.clone(),
            opponents: request.opponents.clone(),
            opponent_weights: request.opponent_weights.clone(),
            pool: request.pool.clone(),
            constraints: request.constraints.clone(),
            population_size: 4,
            generations: 1,
            games_per_policy: 2,
            maximum_swaps: 4,
            seed: request.seed.wrapping_sub(10_000_019),
            workers: request.workers,
            parallel_jobs: 2,
            hat_budget: request.hat_budget,
            max_turns: request.max_turns,
            game_timeout_seconds: request.game_timeout_seconds,
        },
        &CancellationToken::default(),
    )?;
    if evolution.plans.len() != 1
        || evolution.evaluated_policies < 2
        || evolution.attempted_games == 0
    {
        return Err("sideboard policy evolution produced incomplete evidence".into());
    }
    request.sideboard_plans = evolution.plans;

    let cold_started = Instant::now();
    let cold = simulator::run_best_of_three(&data_dir, &request)?;
    let cold_elapsed = cold_started.elapsed();
    let hot_started = Instant::now();
    let hot = simulator::run_best_of_three(&data_dir, &request)?;
    let hot_elapsed = hot_started.elapsed();
    let attempted_games = cold.preboard.games + cold.postboard.games;
    if cold.requested_matches != 4
        || cold.completed_matches != 4
        || cold.preboard.games != 4
        || cold.postboard.games < 4
        || cold.matchups[0].preboard_on_play.games != 2
        || cold.matchups[0].preboard_on_draw.games != 2
        || cold.matchups[0].postboard_on_draw.games < 4
        || cold.matchups[0].plan != request.sideboard_plans[0]
    {
        return Err("Best-of-Three phase accounting or plan retention failed".into());
    }
    if cold.cached_games != 0
        || hot.cached_games != attempted_games
        || cold.provenance != hot.provenance
        || cold.matches.wins != hot.matches.wins
        || hot_elapsed >= cold_elapsed
    {
        return Err("Best-of-Three cold/hot cache evidence was inconsistent".into());
    }
    println!(
        "engine={} matches={}/{} games={} preboard={:.1}% postboard={:.1}% match={:.1}% cold_ms={} hot_ms={} cached_games={} config={}",
        hot.engine,
        hot.completed_matches,
        hot.requested_matches,
        attempted_games,
        hot.preboard.win_rate * 100.0,
        hot.postboard.win_rate * 100.0,
        hot.matches.win_rate * 100.0,
        cold_elapsed.as_millis(),
        hot_elapsed.as_millis(),
        hot.cached_games,
        hot.provenance.configuration_hash,
    );
    println!(
        "policy_evolution policies={} games={} cached={} gps={:.1} config={}",
        evolution.evaluated_policies,
        evolution.attempted_games,
        evolution.cached_games,
        evolution.games_per_second,
        evolution.provenance.configuration_hash,
    );

    // Reverse the power relationship to force the candidate-lost branch and
    // prove that the on-play post-board policy is executed as well.
    let weak_candidate = Deck {
        name: "Basic-land candidate".into(),
        command_zone: vec![],
        mainboard: vec![entry(card("Island"), 60)],
        sideboard: vec![entry(card("Forest"), 4), entry(card("Mountain"), 4)],
    };
    let strong_opponent = request.candidate.clone();
    let strong_key = simulator::sideboard_opponent_key(0, &strong_opponent)?;
    let losing = simulator::run_best_of_three(
        &data_dir,
        &simulator::BestOfThreeValidationRequest {
            candidate: weak_candidate,
            opponents: vec![strong_opponent],
            opponent_weights: vec![1.0],
            pool: request.pool.clone(),
            constraints: request.constraints.clone(),
            sideboard_plans: vec![MatchupSideboardPlan {
                opponent_key: strong_key,
                on_play: SideboardExchange {
                    cards_in: vec![entry(card("Forest"), 4)],
                    cards_out: vec![entry(card("Island"), 4)],
                },
                on_draw: SideboardExchange {
                    cards_in: vec![entry(card("Mountain"), 4)],
                    cards_out: vec![entry(card("Island"), 4)],
                },
            }],
            matches: 4,
            seed: request.seed.wrapping_add(30_000_041),
            workers: request.workers,
            hat_budget: request.hat_budget,
            max_turns: request.max_turns,
            game_timeout_seconds: request.game_timeout_seconds,
        },
    )?;
    if losing.completed_matches != 4
        || losing.matches.wins != 0
        || losing.matchups[0].postboard_on_play.games < 4
        || losing.matchups[0].postboard_on_draw.games != 0
    {
        return Err("Best-of-Three loser-on-play sequencing failed".into());
    }
    println!(
        "loser_branch matches={}/{} post_on_play={} post_on_draw={} config={}",
        losing.completed_matches,
        losing.requested_matches,
        losing.matchups[0].postboard_on_play.games,
        losing.matchups[0].postboard_on_draw.games,
        losing.provenance.configuration_hash,
    );
    Ok(())
}
