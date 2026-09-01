use metagene_app_lib::{catalog, forge, simulator};
use metagene_core::{CardPool, Deck, DeckEntry};
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

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
    let candidate = Deck {
        name: "Forge Plains smoke".into(),
        command_zone: Vec::new(),
        mainboard: vec![DeckEntry {
            card_id: plains.id.clone(),
            quantity: 60,
        }],
        sideboard: Vec::new(),
    };
    let opponent = Deck {
        name: "Forge Mountain smoke".into(),
        command_zone: Vec::new(),
        mainboard: vec![DeckEntry {
            card_id: mountain.id.clone(),
            quantity: 60,
        }],
        sideboard: Vec::new(),
    };
    let request = simulator::HexdekValidationRequest {
        candidate,
        opponents: vec![opponent],
        opponent_weights: Vec::new(),
        pool,
        games: 4,
        seed: SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_nanos()
            .min(i64::MAX as u128) as i64,
        workers: 1,
        seats: 2,
        hat_budget: 0,
        max_turns: 0,
        game_timeout_seconds: 10,
        commander: false,
        fixed_order: false,
    };
    let cold_started = Instant::now();
    let cold = forge::run_validation(&data_dir, &request)?;
    let cold_elapsed = cold_started.elapsed();
    let hot_started = Instant::now();
    let hot = forge::run_validation(&data_dir, &request)?;
    let hot_elapsed = hot_started.elapsed();
    if cold.games + cold.crashes != 4 || cold.engine != "forge" {
        return Err("Forge smoke result did not account for all requested games".into());
    }
    if cold.cached_games != 0
        || hot.cached_games != 4
        || cold.provenance.is_none()
        || cold.provenance != hot.provenance
        || cold.matchup_wins != hot.matchup_wins
        || hot_elapsed >= cold_elapsed
    {
        return Err("Forge cold/hot cache evidence was inconsistent".into());
    }
    println!(
        "engine=forge games={} cold_ms={} hot_ms={} cached_games={} speedup={:.1}x config={}",
        hot.games,
        cold_elapsed.as_millis(),
        hot_elapsed.as_millis(),
        hot.cached_games,
        cold_elapsed.as_secs_f64() / hot_elapsed.as_secs_f64().max(f64::EPSILON),
        hot.provenance.as_ref().unwrap().configuration_hash,
    );
    Ok(())
}
