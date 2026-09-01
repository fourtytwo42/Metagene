use metagene_app_lib::{catalog, simulator};
use metagene_core::{CancellationToken, Card, CardPool, Deck, DeckEntry};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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
    let cancellation = CancellationToken::default();
    let trigger = cancellation.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(250));
        trigger.cancel();
    });
    let started = Instant::now();
    let result = simulator::run_hexdek_cancellable(
        &data_dir,
        &simulator::HexdekValidationRequest {
            candidate: deck("Plains", &plains),
            opponents: vec![deck("Mountains", &mountain)],
            opponent_weights: Vec::new(),
            pool: CardPool {
                cards: vec![plains, mountain],
            },
            games: 10_000_000,
            seed: 20260829,
            workers: 30,
            seats: 2,
            hat_budget: 20,
            max_turns: 80,
            game_timeout_seconds: 30,
            commander: false,
            fixed_order: false,
        },
        &cancellation,
    );
    match result {
        Err(simulator::SimulatorError::Cancelled) => {
            println!("cancelled_in_ms={}", started.elapsed().as_millis());
            Ok(())
        }
        Err(error) => Err(format!("unexpected cancellation result: {error}").into()),
        Ok(report) => Err(format!(
            "simulation unexpectedly completed {} games before cancellation",
            report.games
        )
        .into()),
    }
}
