use metagene_app_lib::{catalog, gauntlets, simulator};
use metagene_core::{
    CardPool, Deck, DeckConstraints, DeckEntry, export_mtga, import_decklist,
    validate_deck_legality,
};
use std::fs;
use std::path::PathBuf;
use std::time::Instant;
use uuid::Uuid;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is not defined")?
        .join("dev.metagene.app");
    let pool = CardPool {
        cards: catalog::legal_pool(&data_dir, "arena", "historic")?,
    };
    let basics = pool
        .cards
        .iter()
        .filter(|card| card.features.is_land && card.features.unlimited_copies)
        .take(3)
        .collect::<Vec<_>>();
    if basics.len() != 3 {
        return Err("Arena Historic pool has fewer than three basic lands".into());
    }

    let weights = [0.6, 0.3, 0.1];
    let mut decks = Vec::new();
    let mut opponents = Vec::new();
    for (index, basic) in basics.into_iter().enumerate() {
        let deck = Deck {
            name: format!("Portable opponent {}", index + 1),
            command_zone: Vec::new(),
            mainboard: vec![DeckEntry {
                card_id: basic.id.clone(),
                quantity: 60,
            }],
            sideboard: Vec::new(),
        };
        validate_deck_legality(&deck, &pool, &DeckConstraints::default())?;
        opponents.push(gauntlets::BundleOpponent {
            name: deck.name.clone(),
            weight: weights[index],
            decklist: export_mtga(&deck, &pool),
        });
        decks.push(deck);
    }

    let started = Instant::now();
    let bundle = gauntlets::MetagameBundle {
        schema_version: 1,
        name: "Portable Historic benchmark".into(),
        platform: "arena".into(),
        format: "historic".into(),
        commander: false,
        metadata: gauntlets::GauntletMetadata {
            source: "local benchmark".into(),
            observed_at: "2026-08-29".into(),
            notes: "Portable MTGA text round-trip".into(),
            ..Default::default()
        },
        opponents,
    };
    let encoded = serde_json::to_string_pretty(&bundle)?;
    let decoded: gauntlets::MetagameBundle = serde_json::from_str(&encoded)?;
    for opponent in &decoded.opponents {
        let imported = import_decklist(&opponent.decklist, &pool)?;
        validate_deck_legality(&imported, &pool, &DeckConstraints::default())?;
    }
    let allocation = simulator::weighted_game_allocations(100, decks.len(), &weights);
    if allocation != [180, 90, 30] || allocation.iter().sum::<usize>() != 300 {
        return Err("weighted fixed-budget allocation failed".into());
    }

    let temporary =
        std::env::temp_dir().join(format!("metagene-metagame-bundle-{}", Uuid::new_v4()));
    let saved = gauntlets::save(
        &temporary,
        &bundle.name,
        &bundle.platform,
        &bundle.format,
        bundle.commander,
        &decks,
        &weights,
        &bundle.metadata,
    )?;
    let loaded = gauntlets::load(&temporary, &saved.summary.id)?;
    if loaded.decks.len() != 3 || loaded.weights != weights {
        return Err("saved weighted field did not round-trip".into());
    }
    let database_bytes = fs::metadata(temporary.join("gauntlets.sqlite"))?.len();
    fs::remove_dir_all(&temporary)?;
    println!(
        "opponents={} bundle_bytes={} allocation={allocation:?} database_bytes={database_bytes} elapsed_ms={}",
        loaded.decks.len(),
        encoded.len(),
        started.elapsed().as_millis()
    );
    Ok(())
}
