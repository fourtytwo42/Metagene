use metagene_app_lib::{catalog, simulator};
use metagene_core::{
    CancellationToken, Card, CardPool, Deck, DeckConstraints, DeckEntry, EvolutionConfig,
    EvolutionEngine,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

fn exact_card(data_dir: &Path, name: &str) -> Result<Card, Box<dyn std::error::Error>> {
    catalog::search(data_dir, name, "paper", "commander", 20)?
        .into_iter()
        .find(|card| card.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| format!("Commander catalog does not contain {name}").into())
}

fn evolve_for_commander(
    all_cards: &[Card],
    commander: &Card,
    seed: u64,
) -> Result<Deck, Box<dyn std::error::Error>> {
    let identity: HashSet<_> = commander.color_identity.iter().copied().collect();
    let mut cards = all_cards
        .iter()
        .filter(|card| {
            card.id == commander.id
                || card
                    .color_identity
                    .iter()
                    .all(|color| identity.contains(color))
        })
        .cloned()
        .collect::<Vec<_>>();
    if !cards.iter().any(|card| card.id == commander.id) {
        cards.push(commander.clone());
    }
    let pool = CardPool { cards };
    let seed_deck = Deck {
        name: format!("{} structural seed", commander.name),
        command_zone: vec![DeckEntry {
            card_id: commander.id.clone(),
            quantity: 1,
        }],
        mainboard: Vec::new(),
        sideboard: Vec::new(),
    };
    let result = EvolutionEngine::default().run(
        &seed_deck,
        &pool,
        &DeckConstraints {
            target_mainboard_size: 99,
            target_sideboard_size: 0,
            minimum_commanders: 1,
            maximum_commanders: 1,
            default_max_copies: 1,
            locked_cards: Vec::new(),
            minimum_lands: Some(36),
            maximum_lands: Some(44),
            allowed_colors: None,
        },
        &EvolutionConfig {
            population_size: 48,
            generations: 4,
            elite_count: 8,
            trials_per_candidate: 64,
            mutation_swaps: 3,
            mutation_rate: 0.9,
            tournament_size: 4,
            seed,
            turn_horizon: 8,
            worker_threads: Some(30),
            evolve_commanders: false,
            commander_mutation_rate: 0.0,
            ..EvolutionConfig::default()
        },
        &CancellationToken::default(),
        |_| {},
    )?;
    let best = result.frontier.into_iter().next().ok_or("empty frontier")?;
    eprintln!(
        "{} structural fitness {:.3} ({} unique cards)",
        commander.name,
        best.fitness.score,
        best.deck.mainboard.len()
    );
    Ok(best.deck)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is not defined")?
        .join("dev.metagene.app");
    let all_cards = catalog::legal_pool(&data_dir, "paper", "commander")?;
    let leaders = [
        exact_card(&data_dir, "Isamaru, Hound of Konda")?,
        exact_card(&data_dir, "Krenko, Mob Boss")?,
        exact_card(&data_dir, "Talrand, Sky Summoner")?,
        exact_card(&data_dir, "Marwyn, the Nurturer")?,
    ];
    let mut decks = Vec::new();
    for (index, commander) in leaders.iter().enumerate() {
        decks.push(evolve_for_commander(
            &all_cards,
            commander,
            0xC0DE_0000 + index as u64,
        )?);
    }
    let report = simulator::run_hexdek(
        &data_dir,
        &simulator::HexdekValidationRequest {
            candidate: decks[0].clone(),
            opponents: decks[1..].to_vec(),
            opponent_weights: Vec::new(),
            pool: CardPool { cards: all_cards },
            games: 256,
            seed: 20260829,
            workers: 30,
            seats: 4,
            hat_budget: 20,
            max_turns: 60,
            game_timeout_seconds: 30,
            commander: true,
            fixed_order: false,
        },
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
