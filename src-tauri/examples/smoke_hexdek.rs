use metagene_app_lib::{catalog, simulator};
use metagene_core::{Card, CardPool, Deck, DeckEntry};
use std::path::{Path, PathBuf};

fn exact_card(data_dir: &Path, name: &str) -> Result<Card, Box<dyn std::error::Error>> {
    catalog::search(data_dir, name, "paper", "commander", 20)?
        .into_iter()
        .find(|card| card.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| format!("Commander catalog does not contain {name}").into())
}

fn deck(name: &str, commander: &Card, basic: &Card) -> Deck {
    Deck {
        name: name.into(),
        command_zone: vec![DeckEntry {
            card_id: commander.id.clone(),
            quantity: 1,
        }],
        mainboard: vec![DeckEntry {
            card_id: basic.id.clone(),
            quantity: 99,
        }],
        sideboard: Vec::new(),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is not defined")?
        .join("dev.metagene.app");
    let leaders = [
        exact_card(&data_dir, "Isamaru, Hound of Konda")?,
        exact_card(&data_dir, "Krenko, Mob Boss")?,
        exact_card(&data_dir, "Talrand, Sky Summoner")?,
        exact_card(&data_dir, "Marwyn, the Nurturer")?,
    ];
    let basics = [
        exact_card(&data_dir, "Plains")?,
        exact_card(&data_dir, "Mountain")?,
        exact_card(&data_dir, "Island")?,
        exact_card(&data_dir, "Forest")?,
    ];
    let decks = leaders
        .iter()
        .zip(basics.iter())
        .enumerate()
        .map(|(index, (leader, basic))| deck(&format!("Smoke {}", index + 1), leader, basic))
        .collect::<Vec<_>>();
    let pool = CardPool {
        cards: leaders.into_iter().chain(basics).collect(),
    };
    let request = simulator::HexdekValidationRequest {
        candidate: decks[0].clone(),
        opponents: decks[1..].to_vec(),
        opponent_weights: Vec::new(),
        pool,
        games: 16,
        seed: 42,
        workers: 16,
        seats: 4,
        hat_budget: 8,
        max_turns: 20,
        game_timeout_seconds: 30,
        commander: true,
        fixed_order: false,
    };
    let report = simulator::run_hexdek(&data_dir, &request)?;
    let evidence = report
        .commander_evidence
        .as_ref()
        .ok_or("Commander evidence was not produced")?;
    assert_eq!(evidence.evidence_mode, "multiplayer_pod");
    assert_eq!(evidence.pod_size, 4);
    assert_eq!(evidence.complete_rotations, 4);
    assert_eq!(evidence.partial_rotation_games, 0);
    assert_eq!(evidence.seat_performance.len(), 4);
    assert!(
        [
            &evidence.threat_development,
            &evidence.interaction,
            &evidence.recovery,
            &evidence.mana_reliability,
        ]
        .iter()
        .all(|metric| metric.source == "rules_engine_compact")
    );
    assert!(evidence.penalties.kingmaking_signal.is_some());
    let repeated = simulator::run_hexdek(&data_dir, &request)?;
    let repeated_evidence = repeated
        .commander_evidence
        .as_ref()
        .ok_or("cached Commander evidence was not produced")?;
    assert_eq!(evidence.objective_score, repeated_evidence.objective_score);
    println!(
        "mode={} pod={} games={} rotations={} partial={} seats={} objective={:.4} penalty={:.4} native={} targets={} cached={} archetypes={}",
        evidence.evidence_mode,
        evidence.pod_size,
        report.games,
        evidence.complete_rotations,
        evidence.partial_rotation_games,
        evidence.seat_performance.len(),
        evidence.objective_score,
        evidence.penalties.total_penalty,
        evidence.threat_development.source,
        evidence.target_actions.values().sum::<usize>(),
        repeated.cached_games,
        evidence.pod_archetypes.join(" | ")
    );
    Ok(())
}
