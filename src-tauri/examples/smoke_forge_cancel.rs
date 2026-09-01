use metagene_app_lib::{catalog, forge, simulator};
use metagene_core::{CancellationToken, CardPool, Deck, DeckEntry};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is not defined")?
        .join("dev.metagene.app");
    let pool = CardPool {
        cards: catalog::legal_pool(&data_dir, "arena", "historic")?,
    };
    let basic = |name: &str| {
        pool.cards
            .iter()
            .find(|card| card.name == name)
            .ok_or_else(|| format!("Historic pool does not contain {name}"))
    };
    let plains = basic("Plains")?;
    let mountain = basic("Mountain")?;
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
        candidate: deck("Forge cancellation candidate", plains.id.clone()),
        opponents: vec![deck("Forge cancellation opponent", mountain.id.clone())],
        opponent_weights: Vec::new(),
        pool,
        games: 10_000,
        seed: 20260829,
        workers: 1,
        seats: 2,
        hat_budget: 0,
        max_turns: 0,
        game_timeout_seconds: 30,
        commander: false,
        fixed_order: false,
    };
    let cancellation = CancellationToken::default();
    let trigger = cancellation.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        trigger.cancel();
    });
    let started = Instant::now();
    match forge::run_validation_cancellable(&data_dir, &request, &cancellation) {
        Err(forge::ForgeError::Cancelled) => {
            println!("cancelled_in_ms={}", started.elapsed().as_millis());
            Ok(())
        }
        Err(error) => Err(format!("unexpected cancellation result: {error}").into()),
        Ok(report) => Err(format!(
            "Forge unexpectedly completed {} games before cancellation",
            report.games
        )
        .into()),
    }
}
