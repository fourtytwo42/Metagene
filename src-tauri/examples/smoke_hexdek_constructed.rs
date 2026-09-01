use metagene_app_lib::{catalog, simulator};
use metagene_core::{Card, CardPool, Deck, DeckEntry};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn exact_card(data_dir: &Path, name: &str) -> Result<Card, Box<dyn std::error::Error>> {
    catalog::search(data_dir, name, "paper", "standard", 20)?
        .into_iter()
        .find(|card| card.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| format!("Standard catalog does not contain {name}").into())
}

fn deck(name: &str, basic: &Card) -> Deck {
    Deck {
        name: name.into(),
        command_zone: Vec::new(),
        mainboard: vec![DeckEntry {
            card_id: basic.id.clone(),
            quantity: 60,
        }],
        sideboard: Vec::new(),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is not defined")?
        .join("dev.metagene.app");
    let plains = exact_card(&data_dir, "Plains")?;
    let mountain = exact_card(&data_dir, "Mountain")?;
    let request = simulator::HexdekValidationRequest {
        candidate: deck("Plains", &plains),
        opponents: vec![deck("Mountains", &mountain)],
        opponent_weights: Vec::new(),
        pool: CardPool {
            cards: vec![plains, mountain],
        },
        games: 16,
        seed: SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_nanos()
            .min(i64::MAX as u128) as i64,
        workers: 16,
        seats: 2,
        hat_budget: 8,
        max_turns: 20,
        game_timeout_seconds: 30,
        commander: false,
        fixed_order: false,
    };
    let cold_started = Instant::now();
    let cold = simulator::run_validation_gauntlet(&data_dir, &request)?;
    let cold_elapsed = cold_started.elapsed();
    let hot_started = Instant::now();
    let hot = simulator::run_validation_gauntlet(&data_dir, &request)?;
    let hot_elapsed = hot_started.elapsed();
    if cold.games + cold.crashes != 16 || cold.seat_order.len() != 4 {
        return Err("HexDek smoke did not preserve the budget and seat evidence".into());
    }
    if cold.cached_games != 0
        || hot.cached_games != 16
        || cold.provenance.is_none()
        || cold.provenance != hot.provenance
        || cold.matchup_wins != hot.matchup_wins
        || cold.seat_order.len() != hot.seat_order.len()
        || hot_elapsed >= cold_elapsed
    {
        return Err("HexDek cold/hot cache evidence was inconsistent".into());
    }
    println!(
        "engine=hexdek games={} cold_ms={} hot_ms={} cached_games={} speedup={:.1}x config={}",
        hot.games,
        cold_elapsed.as_millis(),
        hot_elapsed.as_millis(),
        hot.cached_games,
        cold_elapsed.as_secs_f64() / hot_elapsed.as_secs_f64().max(f64::EPSILON),
        hot.provenance.as_ref().unwrap().configuration_hash,
    );
    Ok(())
}
