use metagene_app_lib::{catalog, gauntlets, metagame_sources};
use metagene_core::{
    CancellationToken, CardPool, DeckConstraints, EvolutionConfig, EvolutionEngine,
};
use std::path::PathBuf;
use uuid::Uuid;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is not defined")?
        .join("dev.metagene.app");
    let platform = std::env::args().nth(1).unwrap_or_else(|| "paper".into());
    let format = std::env::args().nth(2).unwrap_or_else(|| "legacy".into());
    let pool = CardPool {
        cards: catalog::legal_pool(&data_dir, &platform, &format)?,
    };
    let report = metagame_sources::acquire_getpaird(
        &metagame_sources::MetagameAcquisitionRequest {
            source: "getpaird".into(),
            platform: platform.clone(),
            format: format.clone(),
            commander: false,
            lookback_days: 365,
            api_key: None,
        },
        &pool,
    )?;
    if report.accepted.is_empty() {
        return Err(format!(
            "live source produced no legal decks ({} quarantined)",
            report.quarantined.len()
        )
        .into());
    }
    let accepted = report.accepted.iter().take(8).collect::<Vec<_>>();
    let decks = accepted
        .iter()
        .map(|entry| entry.deck.clone())
        .collect::<Vec<_>>();
    let mut weights = accepted
        .iter()
        .map(|entry| entry.weight)
        .collect::<Vec<_>>();
    let weight_total = weights.iter().sum::<f64>();
    for weight in &mut weights {
        *weight /= weight_total;
    }
    let seed = &accepted[0].deck;
    let evolution = EvolutionEngine::default().run(
        seed,
        &pool,
        &DeckConstraints {
            target_mainboard_size: seed.mainboard_count(),
            target_sideboard_size: seed.sideboard_count(),
            minimum_commanders: 0,
            maximum_commanders: 0,
            default_max_copies: 4,
            locked_cards: Vec::new(),
            minimum_lands: None,
            maximum_lands: None,
            allowed_colors: None,
        },
        &EvolutionConfig {
            population_size: 4,
            generations: 1,
            elite_count: 2,
            trials_per_candidate: 2,
            worker_threads: Some(1),
            evolve_sideboard: seed.sideboard_count() > 0,
            ..EvolutionConfig::default()
        },
        &CancellationToken::default(),
        |_| {},
    )?;
    let metadata = gauntlets::GauntletMetadata {
        source: report.source.clone(),
        observed_at: format!(
            "{}..{}",
            report.provenance.event_start_unix.unwrap_or_default(),
            report.provenance.event_end_unix.unwrap_or_default()
        ),
        notes: format!("{} live acquisition smoke", report.provenance.adapter),
        source_url: Some(report.provenance.source_url.clone()),
        retrieved_at_unix: Some(report.provenance.retrieved_at_unix),
        event_start_unix: report.provenance.event_start_unix,
        event_end_unix: report.provenance.event_end_unix,
        sample_size: Some(report.provenance.accepted_deck_count),
        transformation_notes: report.provenance.transformation_notes.clone(),
        ..Default::default()
    };
    let name = format!("Live acquisition smoke {}", Uuid::new_v4());
    let first = gauntlets::save(
        &data_dir, &name, &platform, &format, false, &decks, &weights, &metadata,
    )?;
    let second = gauntlets::save(
        &data_dir, &name, &platform, &format, false, &decks, &weights, &metadata,
    )?;
    let comparison = gauntlets::compare(&data_dir, &first.summary.id, &second.summary.id)?;
    let bundle = gauntlets::MetagameBundle {
        schema_version: 1,
        name: name.clone(),
        platform,
        format,
        commander: false,
        metadata,
        opponents: accepted
            .iter()
            .zip(weights.iter().copied())
            .map(|(entry, weight)| gauntlets::BundleOpponent {
                name: entry.archetype.clone(),
                weight,
                decklist: entry.decklist.clone(),
            })
            .collect(),
    };
    let encoded = serde_json::to_string(&bundle)?;
    let decoded: gauntlets::MetagameBundle = serde_json::from_str(&encoded)?;
    gauntlets::delete(&data_dir, &first.summary.id)?;
    gauntlets::delete(&data_dir, &second.summary.id)?;
    println!(
        "source={} events={} submitted={} accepted={} archetypes={} quarantined={} evolution_candidates={} frontier={} saved_revision={} unchanged={} portable_opponents={} bytes={}",
        report.provenance.adapter,
        report.provenance.event_count,
        report.provenance.submitted_deck_count,
        report.provenance.accepted_deck_count,
        report.accepted.len(),
        report.quarantined.len(),
        evolution.evaluated_candidates,
        evolution.frontier.len(),
        second.summary.revision,
        comparison.unchanged,
        decoded.opponents.len(),
        encoded.len(),
    );
    Ok(())
}
