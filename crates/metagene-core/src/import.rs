use crate::{CardId, CardPool, Deck, DeckEntry};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum DeckImportError {
    #[error("line {line}: expected a quantity followed by a card name")]
    InvalidLine { line: usize },
    #[error("line {line}: quantity is outside the supported 1-255 range")]
    InvalidQuantity { line: usize },
    #[error("line {line}: card not found in the selected platform and format: {name}")]
    UnknownCard { line: usize, name: String },
    #[error("the deck list contains no cards")]
    EmptyDeck,
}

#[derive(Clone, Copy)]
enum Zone {
    Commander,
    Mainboard,
    Sideboard,
}

pub fn import_decklist(text: &str, pool: &CardPool) -> Result<Deck, DeckImportError> {
    let mut by_name = HashMap::<String, usize>::new();
    let mut by_printing = HashMap::<(String, String, String), usize>::new();
    for (index, card) in pool.cards.iter().enumerate() {
        by_name.insert(card.name.to_lowercase(), index);
        by_printing.insert(
            (
                card.name.to_lowercase(),
                card.set_code.to_lowercase(),
                card.collector_number.to_lowercase(),
            ),
            index,
        );
    }
    // Arena exports modal/transforming double-faced cards by the name printed on
    // the initially playable face, while Scryfall's canonical card name retains
    // `front // back`. Keep exact canonical names authoritative, then add the
    // front face as an import-only alias, including printing-aware lookups.
    for (index, card) in pool.cards.iter().enumerate() {
        let Some((front, _)) = card.name.split_once(" // ") else {
            continue;
        };
        by_name.entry(front.to_lowercase()).or_insert(index);
        by_printing
            .entry((
                front.to_lowercase(),
                card.set_code.to_lowercase(),
                card.collector_number.to_lowercase(),
            ))
            .or_insert(index);
    }
    let mut zone = Zone::Mainboard;
    let mut command_zone = Vec::new();
    let mut mainboard = Vec::new();
    let mut sideboard = Vec::new();

    for (offset, raw_line) in text.lines().enumerate() {
        let line_number = offset + 1;
        let line = raw_line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        match line.to_ascii_lowercase().as_str() {
            "commander" | "commanders" | "command zone" => {
                zone = Zone::Commander;
                continue;
            }
            "deck" | "mainboard" | "main deck" => {
                zone = Zone::Mainboard;
                continue;
            }
            "sideboard" | "side board" => {
                zone = Zone::Sideboard;
                continue;
            }
            _ => {}
        }

        let Some((quantity_text, remainder)) = line.split_once(char::is_whitespace) else {
            return Err(DeckImportError::InvalidLine { line: line_number });
        };
        let quantity_text = quantity_text.trim_end_matches(['x', 'X']);
        let quantity = quantity_text
            .parse::<u16>()
            .map_err(|_| DeckImportError::InvalidLine { line: line_number })?;
        if !(1..=u8::MAX as u16).contains(&quantity) {
            return Err(DeckImportError::InvalidQuantity { line: line_number });
        }
        let (name, printing) = parse_card_reference(remainder.trim());
        if name.is_empty() {
            return Err(DeckImportError::InvalidLine { line: line_number });
        }
        let index = printing
            .and_then(|(set, collector)| {
                by_printing
                    .get(&(
                        name.to_lowercase(),
                        set.to_lowercase(),
                        collector.to_lowercase(),
                    ))
                    .copied()
            })
            .or_else(|| by_name.get(&name.to_lowercase()).copied())
            .ok_or_else(|| DeckImportError::UnknownCard {
                line: line_number,
                name: name.to_string(),
            })?;
        let target = match zone {
            Zone::Commander => &mut command_zone,
            Zone::Mainboard => &mut mainboard,
            Zone::Sideboard => &mut sideboard,
        };
        add_entry(target, pool.cards[index].id.clone(), quantity as u8);
    }

    if command_zone.is_empty() && mainboard.is_empty() && sideboard.is_empty() {
        return Err(DeckImportError::EmptyDeck);
    }
    Ok(Deck {
        name: "Imported Deck".into(),
        command_zone,
        mainboard,
        sideboard,
    })
}

fn parse_card_reference(text: &str) -> (&str, Option<(&str, &str)>) {
    let Some(open) = text.rfind(" (") else {
        return (text.trim(), None);
    };
    let suffix = &text[open + 2..];
    let Some(close) = suffix.find(')') else {
        return (text.trim(), None);
    };
    let set = suffix[..close].trim();
    let collector = suffix[close + 1..].trim();
    if set.is_empty() || collector.is_empty() || collector.contains(char::is_whitespace) {
        return (text.trim(), None);
    }
    (text[..open].trim(), Some((set, collector)))
}

fn add_entry(entries: &mut Vec<DeckEntry>, card_id: CardId, quantity: u8) {
    if let Some(existing) = entries.iter_mut().find(|entry| entry.card_id == card_id) {
        existing.quantity = existing.quantity.saturating_add(quantity);
    } else {
        entries.push(DeckEntry { card_id, quantity });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Card, CardFeatures, Color};
    use std::collections::BTreeMap;

    fn card(id: &str, name: &str, set: &str, collector: &str) -> Card {
        Card {
            id: CardId(id.into()),
            arena_id: Some(1),
            platforms: vec!["arena".into()],
            name: name.into(),
            set_code: set.into(),
            collector_number: collector.into(),
            type_line: "Card".into(),
            oracle_text: String::new(),
            colors: vec![Color::Green],
            color_identity: vec![Color::Green],
            image_uri: None,
            legalities: BTreeMap::new(),
            features: CardFeatures::default(),
        }
    }

    #[test]
    fn imports_arena_sections_and_round_trips_printings() {
        let pool = CardPool {
            cards: vec![
                card("leader", "Test Leader", "cmd", "7"),
                card("spell", "Test Spell", "tst", "12"),
            ],
        };
        let deck = import_decklist(
            "Commander\n1 Test Leader (CMD) 7\n\nDeck\n4 Test Spell (TST) 12\n\nSideboard\n1x Test Spell",
            &pool,
        )
        .unwrap();
        assert_eq!(deck.command_zone[0].card_id.0, "leader");
        assert_eq!(deck.mainboard[0].quantity, 4);
        assert_eq!(deck.sideboard[0].quantity, 1);
    }

    #[test]
    fn reports_unknown_cards_with_line_numbers() {
        let error =
            import_decklist("Deck\n4 Missing Card", &CardPool { cards: vec![] }).unwrap_err();
        assert_eq!(
            error,
            DeckImportError::UnknownCard {
                line: 2,
                name: "Missing Card".into()
            }
        );
    }

    #[test]
    fn imports_arena_front_face_names_for_double_faced_cards() {
        let pool = CardPool {
            cards: vec![
                card(
                    "glasspool",
                    "Glasspool Mimic // Glasspool Shore",
                    "znr",
                    "60",
                ),
                card(
                    "bala-ged",
                    "Bala Ged Recovery // Bala Ged Sanctuary",
                    "znr",
                    "180",
                ),
            ],
        };
        let deck = import_decklist(
            "Deck\n3 Glasspool Mimic (ZNR) 60\n4 Bala Ged Recovery (ZNR) 180",
            &pool,
        )
        .unwrap();

        assert_eq!(deck.mainboard.len(), 2);
        assert_eq!(deck.mainboard[0].card_id.0, "glasspool");
        assert_eq!(deck.mainboard[0].quantity, 3);
        assert_eq!(deck.mainboard[1].card_id.0, "bala-ged");
        assert_eq!(deck.mainboard[1].quantity, 4);

        let without_printing = import_decklist("Deck\n1 Glasspool Mimic", &pool).unwrap();
        assert_eq!(without_printing.mainboard[0].card_id.0, "glasspool");

        let canonical =
            import_decklist("Deck\n1 Glasspool Mimic // Glasspool Shore (ZNR) 60", &pool).unwrap();
        assert_eq!(canonical.mainboard[0].card_id.0, "glasspool");
    }
}
