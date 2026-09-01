use crate::{CardPool, Deck};
use std::collections::HashMap;

pub fn export_mtga(deck: &Deck, pool: &CardPool) -> String {
    let by_id: HashMap<_, _> = pool.cards.iter().map(|card| (&card.id, card)).collect();
    let mut output = String::new();

    if !deck.command_zone.is_empty() {
        output.push_str("Commander\n");
        for entry in &deck.command_zone {
            if let Some(card) = by_id.get(&entry.card_id) {
                output.push_str(&format!(
                    "{} {} ({}) {}\n",
                    entry.quantity,
                    card.name,
                    card.set_code.to_uppercase(),
                    card.collector_number
                ));
            }
        }
        output.push('\n');
    }
    output.push_str("Deck\n");

    for entry in &deck.mainboard {
        if let Some(card) = by_id.get(&entry.card_id) {
            output.push_str(&format!(
                "{} {} ({}) {}\n",
                entry.quantity,
                card.name,
                card.set_code.to_uppercase(),
                card.collector_number
            ));
        }
    }

    if !deck.sideboard.is_empty() {
        output.push_str("\nSideboard\n");
        for entry in &deck.sideboard {
            if let Some(card) = by_id.get(&entry.card_id) {
                output.push_str(&format!(
                    "{} {} ({}) {}\n",
                    entry.quantity,
                    card.name,
                    card.set_code.to_uppercase(),
                    card.collector_number
                ));
            }
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Card, CardFeatures, CardId, Color, DeckEntry};
    use std::collections::BTreeMap;

    #[test]
    fn exports_arena_format() {
        let pool = CardPool {
            cards: vec![Card {
                id: CardId("parallel-lives".into()),
                arena_id: Some(1),
                platforms: vec!["arena".into(), "paper".into(), "mtgo".into()],
                name: "Parallel Lives".into(),
                set_code: "j21".into(),
                collector_number: "103".into(),
                type_line: "Enchantment".into(),
                oracle_text: String::new(),
                colors: vec![Color::Green],
                color_identity: vec![Color::Green],
                image_uri: None,
                legalities: BTreeMap::new(),
                features: CardFeatures::default(),
            }],
        };
        let deck = Deck {
            name: "Copies".into(),
            command_zone: vec![],
            mainboard: vec![DeckEntry {
                card_id: CardId("parallel-lives".into()),
                quantity: 4,
            }],
            sideboard: vec![],
        };

        assert_eq!(
            export_mtga(&deck, &pool),
            "Deck\n4 Parallel Lives (J21) 103\n"
        );
    }

    #[test]
    fn exports_command_zone_before_deck() {
        let make = |id: &str, name: &str| Card {
            id: CardId(id.into()),
            arena_id: None,
            platforms: vec!["paper".into()],
            name: name.into(),
            set_code: "cmd".into(),
            collector_number: "1".into(),
            type_line: "Legendary Creature".into(),
            oracle_text: String::new(),
            colors: vec![Color::Green],
            color_identity: vec![Color::Green],
            image_uri: None,
            legalities: BTreeMap::new(),
            features: CardFeatures::default(),
        };
        let pool = CardPool {
            cards: vec![
                make("leader", "Test Commander"),
                make("spell", "Test Spell"),
            ],
        };
        let deck = Deck {
            name: "Commander test".into(),
            command_zone: vec![DeckEntry {
                card_id: CardId("leader".into()),
                quantity: 1,
            }],
            mainboard: vec![DeckEntry {
                card_id: CardId("spell".into()),
                quantity: 1,
            }],
            sideboard: vec![],
        };
        assert!(export_mtga(&deck, &pool).starts_with("Commander\n1 Test Commander"));
    }
}
