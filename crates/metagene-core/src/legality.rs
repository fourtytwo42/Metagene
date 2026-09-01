use crate::commanders_compatible;
use crate::model::{Card, CardPool, Color, Deck, DeckConstraints};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum DeckLegalityError {
    #[error("deck references an unknown card: {0}")]
    UnknownCard(String),
    #[error("command zone has {actual} cards; expected {minimum}-{maximum}")]
    CommanderCount {
        actual: usize,
        minimum: usize,
        maximum: usize,
    },
    #[error("{0} is not eligible to be a commander")]
    IneligibleCommander(String),
    #[error("the selected commanders cannot share the command zone")]
    IncompatibleCommanders,
    #[error("{card} has color identity outside its commander ({commander_identity})")]
    ColorIdentity {
        card: String,
        commander_identity: String,
    },
    #[error("{card} has {quantity} copies; the maximum is {maximum}")]
    TooManyCopies {
        card: String,
        quantity: usize,
        maximum: usize,
    },
}

pub fn validate_deck_legality(
    deck: &Deck,
    pool: &CardPool,
    constraints: &DeckConstraints,
) -> Result<(), DeckLegalityError> {
    let cards: HashMap<_, _> = pool.cards.iter().map(|card| (&card.id, card)).collect();
    let commander_count = deck
        .command_zone
        .iter()
        .map(|entry| entry.quantity as usize)
        .sum();
    if commander_count < constraints.minimum_commanders
        || commander_count > constraints.maximum_commanders
    {
        return Err(DeckLegalityError::CommanderCount {
            actual: commander_count,
            minimum: constraints.minimum_commanders,
            maximum: constraints.maximum_commanders,
        });
    }

    let mut commanders = Vec::new();
    for entry in &deck.command_zone {
        let card = cards
            .get(&entry.card_id)
            .copied()
            .ok_or_else(|| DeckLegalityError::UnknownCard(entry.card_id.0.clone()))?;
        if entry.quantity != 1 || !is_commander_eligible(card) {
            return Err(DeckLegalityError::IneligibleCommander(card.name.clone()));
        }
        commanders.push(card);
    }
    if commanders.len() == 1
        && commanders[0]
            .type_line
            .to_ascii_lowercase()
            .contains("background")
    {
        return Err(DeckLegalityError::IneligibleCommander(
            commanders[0].name.clone(),
        ));
    }
    if commanders.len() == 2 && !commanders_compatible(commanders[0], commanders[1]) {
        return Err(DeckLegalityError::IncompatibleCommanders);
    }

    let commander_identity: HashSet<Color> = commanders
        .iter()
        .flat_map(|card| card.color_identity.iter().copied())
        .collect();
    let identity_label = format_color_identity(&commander_identity);
    let commander_names: HashSet<_> = commanders
        .iter()
        .map(|card| card.name.to_ascii_lowercase())
        .collect();

    // Magic's copy limit is based on card name, not printing identity. Legacy
    // imports can legitimately contain multiple Scryfall IDs for one name, so
    // aggregate across those printings before applying the format limit.
    let mut copy_counts: HashMap<String, usize> = HashMap::new();
    for entry in deck.mainboard.iter().chain(&deck.sideboard) {
        let card = cards
            .get(&entry.card_id)
            .copied()
            .ok_or_else(|| DeckLegalityError::UnknownCard(entry.card_id.0.clone()))?;
        let canonical_name = card.name.to_ascii_lowercase();
        if commander_names.contains(&canonical_name) {
            return Err(DeckLegalityError::TooManyCopies {
                card: card.name.clone(),
                quantity: entry.quantity as usize + 1,
                maximum: 1,
            });
        }
        if !commanders.is_empty()
            && card
                .color_identity
                .iter()
                .any(|color| !commander_identity.contains(color))
        {
            return Err(DeckLegalityError::ColorIdentity {
                card: card.name.clone(),
                commander_identity: identity_label.clone(),
            });
        }
        let total = copy_counts.entry(canonical_name).or_default();
        *total += entry.quantity as usize;
        let maximum = if card.features.unlimited_copies {
            usize::MAX
        } else {
            constraints.default_max_copies as usize
        };
        if *total > maximum {
            return Err(DeckLegalityError::TooManyCopies {
                card: card.name.clone(),
                quantity: *total,
                maximum,
            });
        }
    }
    Ok(())
}

pub fn is_commander_eligible(card: &Card) -> bool {
    let type_line = card.type_line.to_ascii_lowercase();
    let oracle = card.oracle_text.to_ascii_lowercase();
    (type_line.contains("legendary") && type_line.contains("creature"))
        || oracle.contains("can be your commander")
        || type_line.contains("legendary background")
}

fn format_color_identity(identity: &HashSet<Color>) -> String {
    let mut symbols = Vec::new();
    for (color, symbol) in [
        (Color::White, "W"),
        (Color::Blue, "U"),
        (Color::Black, "B"),
        (Color::Red, "R"),
        (Color::Green, "G"),
    ] {
        if identity.contains(&color) {
            symbols.push(symbol);
        }
    }
    if symbols.is_empty() {
        "colorless".into()
    } else {
        symbols.join("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CardFeatures, CardId, DeckEntry};
    use std::collections::BTreeMap;

    fn card(name: &str, type_line: &str, oracle: &str, identity: Vec<Color>, basic: bool) -> Card {
        Card {
            id: CardId(name.into()),
            arena_id: None,
            platforms: vec!["paper".into()],
            name: name.into(),
            set_code: "tst".into(),
            collector_number: "1".into(),
            type_line: type_line.into(),
            oracle_text: oracle.into(),
            colors: identity.clone(),
            color_identity: identity,
            image_uri: None,
            legalities: BTreeMap::new(),
            features: CardFeatures {
                unlimited_copies: basic,
                is_land: basic,
                ..Default::default()
            },
        }
    }

    fn entry(name: &str, quantity: u8) -> DeckEntry {
        DeckEntry {
            card_id: CardId(name.into()),
            quantity,
        }
    }

    #[test]
    fn enforces_commander_color_identity_and_singleton() {
        let pool = CardPool {
            cards: vec![
                card(
                    "Leader",
                    "Legendary Creature — Elf",
                    "",
                    vec![Color::Green],
                    false,
                ),
                card("Forest", "Basic Land — Forest", "", vec![], true),
                card("Red Spell", "Sorcery", "", vec![Color::Red], false),
            ],
        };
        let constraints = DeckConstraints {
            target_mainboard_size: 99,
            target_sideboard_size: 0,
            minimum_commanders: 1,
            maximum_commanders: 2,
            default_max_copies: 1,
            locked_cards: vec![],
            minimum_lands: None,
            maximum_lands: None,
            allowed_colors: None,
        };
        let valid = Deck {
            name: "valid".into(),
            command_zone: vec![entry("Leader", 1)],
            mainboard: vec![entry("Forest", 20)],
            sideboard: vec![],
        };
        assert!(validate_deck_legality(&valid, &pool, &constraints).is_ok());
        let invalid = Deck {
            mainboard: vec![entry("Red Spell", 1)],
            ..valid
        };
        assert!(matches!(
            validate_deck_legality(&invalid, &pool, &constraints),
            Err(DeckLegalityError::ColorIdentity { .. })
        ));
    }

    #[test]
    fn supports_partner_and_background_pairs() {
        let partner_a = card(
            "A",
            "Legendary Creature",
            "Partner",
            vec![Color::White],
            false,
        );
        let partner_b = card(
            "B",
            "Legendary Creature",
            "Partner",
            vec![Color::Blue],
            false,
        );
        assert!(commanders_compatible(&partner_a, &partner_b));
        let chooser = card(
            "C",
            "Legendary Creature",
            "Choose a Background",
            vec![Color::Black],
            false,
        );
        let background = card(
            "D",
            "Legendary Enchantment — Background",
            "Commander creatures you own",
            vec![Color::Red],
            false,
        );
        assert!(commanders_compatible(&chooser, &background));
    }

    #[test]
    fn aggregates_copy_limits_across_printings_with_the_same_name() {
        let mut first = card("Lightning Bolt", "Instant", "", vec![Color::Red], false);
        first.id = CardId("bolt-a".into());
        let mut second = first.clone();
        second.id = CardId("bolt-b".into());
        second.set_code = "alt".into();
        let pool = CardPool {
            cards: vec![first, second],
        };
        let deck = Deck {
            name: "alternate printings".into(),
            command_zone: vec![],
            mainboard: vec![entry("bolt-a", 3), entry("bolt-b", 2)],
            sideboard: vec![],
        };
        let constraints = DeckConstraints {
            target_mainboard_size: 60,
            target_sideboard_size: 0,
            minimum_commanders: 0,
            maximum_commanders: 0,
            default_max_copies: 4,
            locked_cards: vec![],
            minimum_lands: None,
            maximum_lands: None,
            allowed_colors: None,
        };

        assert!(matches!(
            validate_deck_legality(&deck, &pool, &constraints),
            Err(DeckLegalityError::TooManyCopies {
                quantity: 5,
                maximum: 4,
                ..
            })
        ));
    }
}
