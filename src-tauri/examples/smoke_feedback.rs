use metagene_app_lib::feedback;
use metagene_core::{Card, CardFeatures, CardId, CardPool, Deck, DeckEntry};
use std::collections::BTreeMap;
use uuid::Uuid;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::temp_dir().join(format!("metagene-feedback-smoke-{}", Uuid::new_v4()));
    let card = Card {
        id: CardId("forest".into()),
        arena_id: Some(123),
        platforms: vec!["arena".into()],
        name: "Forest".into(),
        set_code: "ana".into(),
        collector_number: "1".into(),
        type_line: "Basic Land".into(),
        oracle_text: String::new(),
        colors: Vec::new(),
        color_identity: Vec::new(),
        image_uri: None,
        legalities: BTreeMap::new(),
        features: CardFeatures {
            is_land: true,
            unlimited_copies: true,
            ..CardFeatures::default()
        },
    };
    let pool = CardPool { cards: vec![card] };
    let deck = Deck {
        name: "Feedback proof".into(),
        command_zone: Vec::new(),
        mainboard: vec![DeckEntry {
            card_id: CardId("forest".into()),
            quantity: 60,
        }],
        sideboard: Vec::new(),
    };
    let revision = feedback::register_revision(
        &root,
        "arena",
        "historic",
        &deck,
        &pool,
        "Deck\n60 Forest (ANA) 1",
    )?;
    let ids = std::iter::repeat_n("123", 60).collect::<Vec<_>>().join(",");
    let mut lines = vec![format!("Deck.Submit {{\"DeckCards\":[{ids}]}}")];
    for game in 0..3 {
        let payload = serde_json::json!({"payloadObject":{"teamId":1,"startingTeamId":2,"winningTeamId":2,"matchId":format!("feedback-{game}")}}).to_string();
        lines.push(format!(
            "DuelScene.GameStop {}",
            serde_json::json!({"request":payload})
        ));
    }
    lines.push("Deck.Submit {\"DeckCards\":[999]}".into());
    lines.push(
        "MatchGameRoomStateChangedEvent {\"matchId\":\"unmatched\",\"result\":\"Win\"}".into(),
    );
    let imported = feedback::import(&root, "arena_log", &lines.join("\n"))?;
    assert_eq!(imported.exact_revision_matches, 3);
    assert_eq!(imported.quarantined_identity, 1);

    let summary = feedback::summary(&root, Some("arena"), Some("historic"))?;
    assert_eq!((summary.matches, summary.losses), (3, 3));
    let weights = feedback::suggest_reweights(
        &root,
        "arena",
        "historic",
        &["Unknown".into(), "Control".into()],
        &[0.5, 0.5],
    )?;
    assert!(weights[0].suggested_weight > weights[1].suggested_weight);
    let portable = feedback::export(&root)?;
    assert!(portable.contains(&revision.id));
    let removed = feedback::clear(&root)?;
    assert_eq!(removed, 4);
    std::fs::remove_dir_all(&root)?;
    println!(
        "feedback smoke passed: 3 exact Arena results, scoped summary, transparent weights, portable export, local deletion"
    );
    Ok(())
}
