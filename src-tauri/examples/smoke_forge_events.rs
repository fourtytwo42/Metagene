use metagene_app_lib::{catalog, forge, simulator};
use metagene_core::{CardPool, Deck, DeckEntry};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is not defined")?
        .join("dev.metagene.app");
    let pool = CardPool {
        cards: catalog::legal_pool(&data_dir, "arena", "historic")?,
    };
    let plains = pool
        .cards
        .iter()
        .find(|card| card.name == "Plains")
        .ok_or("Historic pool does not contain Plains")?;
    let mountain = pool
        .cards
        .iter()
        .find(|card| card.name == "Mountain")
        .ok_or("Historic pool does not contain Mountain")?;
    let deck = |name: &str, card_id| Deck {
        name: name.into(),
        command_zone: Vec::new(),
        mainboard: vec![DeckEntry {
            card_id,
            quantity: 60,
        }],
        sideboard: Vec::new(),
    };
    let request = simulator::HexdekValidationRequest {
        candidate: deck("Forge event candidate", plains.id.clone()),
        opponents: vec![deck("Forge event opponent", mountain.id.clone())],
        opponent_weights: Vec::new(),
        pool,
        games: 1,
        seed: 42,
        workers: 1,
        seats: 2,
        hat_budget: 0,
        max_turns: 0,
        game_timeout_seconds: 120,
        commander: false,
        fixed_order: false,
    };
    let probe = forge::run_event_probe(&data_dir, &request)?;
    if probe.events.is_empty()
        || !probe
            .events
            .iter()
            .any(|event| event.kind == "zone.changed")
    {
        return Err("Forge event probe did not capture a zone change".into());
    }
    println!(
        "engine=forge seed={} events={} outcome={}",
        probe.seed,
        probe.events.len(),
        probe.outcome
    );
    Ok(())
}
