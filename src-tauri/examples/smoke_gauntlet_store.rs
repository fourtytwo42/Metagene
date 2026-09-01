use metagene_app_lib::{catalog, gauntlets};
use metagene_core::{CardPool, Deck, DeckConstraints, DeckEntry, validate_deck_legality};
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
    let basic = pool
        .cards
        .iter()
        .find(|card| card.features.is_land && card.features.unlimited_copies)
        .ok_or("Arena Historic pool has no unlimited-copy basic land")?;
    let deck = Deck {
        name: "Gauntlet storage smoke".into(),
        command_zone: Vec::new(),
        mainboard: vec![DeckEntry {
            card_id: basic.id.clone(),
            quantity: 60,
        }],
        sideboard: Vec::new(),
    };
    validate_deck_legality(&deck, &pool, &DeckConstraints::default())?;

    let name = format!("Storage smoke {}", Uuid::new_v4());
    let started = Instant::now();
    let first = gauntlets::save(
        &data_dir,
        &name,
        "arena",
        "historic",
        false,
        std::slice::from_ref(&deck),
        &[1.0],
        &gauntlets::GauntletMetadata::default(),
    )?;
    let second = gauntlets::save(
        &data_dir,
        &name,
        "arena",
        "historic",
        false,
        &[deck.clone(), deck],
        &[0.7, 0.3],
        &gauntlets::GauntletMetadata::default(),
    )?;
    let loaded = gauntlets::load(&data_dir, &second.summary.id)?;
    if first.summary.revision != 1 || second.summary.revision != 2 || loaded.decks.len() != 2 {
        return Err("gauntlet revision round-trip failed".into());
    }
    gauntlets::delete(&data_dir, &first.summary.id)?;
    gauntlets::delete(&data_dir, &second.summary.id)?;
    let database_bytes = std::fs::metadata(data_dir.join("gauntlets.sqlite"))?.len();
    println!(
        "revision_roundtrip=2 decks={} elapsed_ms={} database_bytes={database_bytes}",
        loaded.decks.len(),
        started.elapsed().as_millis()
    );
    Ok(())
}
