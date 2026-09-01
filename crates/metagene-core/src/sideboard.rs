use crate::{CardId, CardPool, Deck, DeckConstraints, DeckEntry, DeckLegalityError};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

/// The cards exchanged after game one for one play/draw branch of a matchup.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SideboardExchange {
    #[serde(default)]
    pub cards_in: Vec<DeckEntry>,
    #[serde(default)]
    pub cards_out: Vec<DeckEntry>,
}

impl SideboardExchange {
    pub fn cards_in_count(&self) -> usize {
        self.cards_in
            .iter()
            .map(|entry| entry.quantity as usize)
            .sum()
    }

    pub fn cards_out_count(&self) -> usize {
        self.cards_out
            .iter()
            .map(|entry| entry.quantity as usize)
            .sum()
    }
}

/// An inspectable Best-of-Three policy for one stable opponent identity.
///
/// The two branches are explicit because the loser of the previous game chooses
/// play or draw. Callers choose the applicable branch before every post-board
/// game rather than allowing an engine adapter to silently infer it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchupSideboardPlan {
    pub opponent_key: String,
    #[serde(default)]
    pub on_play: SideboardExchange,
    #[serde(default)]
    pub on_draw: SideboardExchange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayDraw {
    Play,
    Draw,
}

impl MatchupSideboardPlan {
    pub fn exchange(&self, play_draw: PlayDraw) -> &SideboardExchange {
        match play_draw {
            PlayDraw::Play => &self.on_play,
            PlayDraw::Draw => &self.on_draw,
        }
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum SideboardPlanError {
    #[error("a sideboard plan must identify its opponent")]
    EmptyOpponentKey,
    #[error("more than one sideboard plan targets opponent '{0}'")]
    DuplicateOpponent(String),
    #[error("{zone} contains a zero-quantity decision for card '{card}'")]
    ZeroQuantity { zone: &'static str, card: String },
    #[error("{zone} contains more than one decision for card '{card}'")]
    DuplicateCard { zone: &'static str, card: String },
    #[error("card '{0}' appears in both cards in and cards out")]
    CardInBothDirections(String),
    #[error("cannot board in {requested} copies of '{card}'; sideboard has {available}")]
    InsufficientSideboard {
        card: String,
        requested: usize,
        available: usize,
    },
    #[error("cannot board out {requested} copies of '{card}'; mainboard has {available}")]
    InsufficientMainboard {
        card: String,
        requested: usize,
        available: usize,
    },
    #[error("sideboard exchange moves {cards_in} cards in but {cards_out} cards out")]
    UnequalExchange { cards_in: usize, cards_out: usize },
    #[error("source deck is illegal: {0}")]
    SourceDeckIllegal(DeckLegalityError),
    #[error("post-board deck is illegal: {0}")]
    ResultDeckIllegal(DeckLegalityError),
    #[error("post-board mainboard size changed from {before} to {after}")]
    MainboardSizeChanged { before: usize, after: usize },
    #[error("post-board sideboard size changed from {before} to {after}")]
    SideboardSizeChanged { before: usize, after: usize },
}

/// Validate all branches and ensure exactly one policy exists per opponent key.
pub fn validate_sideboard_plans(
    deck: &Deck,
    plans: &[MatchupSideboardPlan],
    pool: &CardPool,
    constraints: &DeckConstraints,
) -> Result<(), SideboardPlanError> {
    let mut opponent_keys = HashSet::new();
    for plan in plans {
        let key = plan.opponent_key.trim();
        if key.is_empty() {
            return Err(SideboardPlanError::EmptyOpponentKey);
        }
        if !opponent_keys.insert(key) {
            return Err(SideboardPlanError::DuplicateOpponent(key.to_string()));
        }
        apply_sideboard_exchange(deck, &plan.on_play, pool, constraints)?;
        apply_sideboard_exchange(deck, &plan.on_draw, pool, constraints)?;
    }
    Ok(())
}

/// Apply one legal exchange, returning a deterministic deck with compact zones.
pub fn apply_sideboard_exchange(
    deck: &Deck,
    exchange: &SideboardExchange,
    pool: &CardPool,
    constraints: &DeckConstraints,
) -> Result<Deck, SideboardPlanError> {
    crate::validate_deck_legality(deck, pool, constraints)
        .map_err(SideboardPlanError::SourceDeckIllegal)?;
    validate_decisions(exchange)?;

    let cards_in = exchange.cards_in_count();
    let cards_out = exchange.cards_out_count();
    if cards_in != cards_out {
        return Err(SideboardPlanError::UnequalExchange {
            cards_in,
            cards_out,
        });
    }

    let main_counts = zone_counts(&deck.mainboard);
    let side_counts = zone_counts(&deck.sideboard);
    ensure_available(&exchange.cards_in, &side_counts, true, pool)?;
    ensure_available(&exchange.cards_out, &main_counts, false, pool)?;

    let mut result = deck.clone();
    move_cards(
        &mut result.sideboard,
        &mut result.mainboard,
        &exchange.cards_in,
    );
    move_cards(
        &mut result.mainboard,
        &mut result.sideboard,
        &exchange.cards_out,
    );

    let mainboard_before = deck.mainboard_count();
    let mainboard_after = result.mainboard_count();
    if mainboard_before != mainboard_after {
        return Err(SideboardPlanError::MainboardSizeChanged {
            before: mainboard_before,
            after: mainboard_after,
        });
    }
    let sideboard_before = deck.sideboard_count();
    let sideboard_after = result.sideboard_count();
    if sideboard_before != sideboard_after {
        return Err(SideboardPlanError::SideboardSizeChanged {
            before: sideboard_before,
            after: sideboard_after,
        });
    }
    crate::validate_deck_legality(&result, pool, constraints)
        .map_err(SideboardPlanError::ResultDeckIllegal)?;
    Ok(result)
}

fn validate_decisions(exchange: &SideboardExchange) -> Result<(), SideboardPlanError> {
    let cards_in = validate_zone_decisions("cards in", &exchange.cards_in)?;
    let cards_out = validate_zone_decisions("cards out", &exchange.cards_out)?;
    if let Some(card) = cards_in.intersection(&cards_out).next() {
        return Err(SideboardPlanError::CardInBothDirections(card.0.clone()));
    }
    Ok(())
}

fn validate_zone_decisions<'a>(
    zone: &'static str,
    entries: &'a [DeckEntry],
) -> Result<HashSet<&'a CardId>, SideboardPlanError> {
    let mut cards = HashSet::new();
    for entry in entries {
        if entry.quantity == 0 {
            return Err(SideboardPlanError::ZeroQuantity {
                zone,
                card: entry.card_id.0.clone(),
            });
        }
        if !cards.insert(&entry.card_id) {
            return Err(SideboardPlanError::DuplicateCard {
                zone,
                card: entry.card_id.0.clone(),
            });
        }
    }
    Ok(cards)
}

fn zone_counts(entries: &[DeckEntry]) -> HashMap<&CardId, usize> {
    let mut counts = HashMap::new();
    for entry in entries {
        *counts.entry(&entry.card_id).or_default() += entry.quantity as usize;
    }
    counts
}

fn ensure_available(
    requested: &[DeckEntry],
    available: &HashMap<&CardId, usize>,
    boarding_in: bool,
    pool: &CardPool,
) -> Result<(), SideboardPlanError> {
    for entry in requested {
        let available = available.get(&entry.card_id).copied().unwrap_or(0);
        let requested = entry.quantity as usize;
        if requested > available {
            let card = pool
                .cards
                .iter()
                .find(|card| card.id == entry.card_id)
                .map(|card| card.name.clone())
                .unwrap_or_else(|| entry.card_id.0.clone());
            return Err(if boarding_in {
                SideboardPlanError::InsufficientSideboard {
                    card,
                    requested,
                    available,
                }
            } else {
                SideboardPlanError::InsufficientMainboard {
                    card,
                    requested,
                    available,
                }
            });
        }
    }
    Ok(())
}

fn move_cards(source: &mut Vec<DeckEntry>, target: &mut Vec<DeckEntry>, cards: &[DeckEntry]) {
    for moved in cards {
        adjust_zone(source, &moved.card_id, -(moved.quantity as i16));
        adjust_zone(target, &moved.card_id, moved.quantity as i16);
    }
}

fn adjust_zone(zone: &mut Vec<DeckEntry>, card_id: &CardId, delta: i16) {
    if let Some(entry) = zone.iter_mut().find(|entry| entry.card_id == *card_id) {
        entry.quantity = (entry.quantity as i16 + delta) as u8;
    } else if delta > 0 {
        zone.push(DeckEntry {
            card_id: card_id.clone(),
            quantity: delta as u8,
        });
    }
    zone.retain(|entry| entry.quantity > 0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Card, CardFeatures, Color};
    use std::collections::BTreeMap;

    fn card(name: &str, unlimited: bool) -> Card {
        Card {
            id: CardId(name.into()),
            arena_id: None,
            platforms: vec!["arena".into()],
            name: name.into(),
            set_code: "tst".into(),
            collector_number: "1".into(),
            type_line: if unlimited { "Basic Land" } else { "Instant" }.into(),
            oracle_text: String::new(),
            colors: vec![Color::Blue],
            color_identity: vec![Color::Blue],
            image_uri: None,
            legalities: BTreeMap::new(),
            features: CardFeatures {
                is_land: unlimited,
                unlimited_copies: unlimited,
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

    fn fixture() -> (Deck, CardPool, DeckConstraints) {
        let pool = CardPool {
            cards: vec![
                card("Island", true),
                card("Threat", false),
                card("Counter", false),
                card("Removal", false),
            ],
        };
        let deck = Deck {
            name: "Candidate".into(),
            command_zone: vec![],
            mainboard: vec![entry("Island", 56), entry("Threat", 4)],
            sideboard: vec![entry("Counter", 4), entry("Removal", 4)],
        };
        let constraints = DeckConstraints {
            target_mainboard_size: 60,
            target_sideboard_size: 8,
            minimum_commanders: 0,
            maximum_commanders: 0,
            default_max_copies: 4,
            locked_cards: vec![],
            minimum_lands: None,
            maximum_lands: None,
            allowed_colors: None,
        };
        (deck, pool, constraints)
    }

    #[test]
    fn applies_a_legal_exchange_and_preserves_both_zone_sizes() {
        let (deck, pool, constraints) = fixture();
        let result = apply_sideboard_exchange(
            &deck,
            &SideboardExchange {
                cards_in: vec![entry("Counter", 3)],
                cards_out: vec![entry("Threat", 3)],
            },
            &pool,
            &constraints,
        )
        .unwrap();
        assert_eq!(result.mainboard_count(), 60);
        assert_eq!(result.sideboard_count(), 8);
        assert_eq!(zone_counts(&result.mainboard)[&CardId("Counter".into())], 3);
        assert_eq!(zone_counts(&result.sideboard)[&CardId("Threat".into())], 3);
    }

    #[test]
    fn rejects_unequal_and_unavailable_exchanges() {
        let (deck, pool, constraints) = fixture();
        let unequal = SideboardExchange {
            cards_in: vec![entry("Counter", 2)],
            cards_out: vec![entry("Threat", 1)],
        };
        assert_eq!(
            apply_sideboard_exchange(&deck, &unequal, &pool, &constraints),
            Err(SideboardPlanError::UnequalExchange {
                cards_in: 2,
                cards_out: 1,
            })
        );
        let unavailable = SideboardExchange {
            cards_in: vec![entry("Counter", 5)],
            cards_out: vec![entry("Threat", 5)],
        };
        assert!(matches!(
            apply_sideboard_exchange(&deck, &unavailable, &pool, &constraints),
            Err(SideboardPlanError::InsufficientSideboard { .. })
        ));
    }

    #[test]
    fn rejects_ambiguous_decisions_and_duplicate_opponents() {
        let (deck, pool, constraints) = fixture();
        let ambiguous = SideboardExchange {
            cards_in: vec![entry("Counter", 1)],
            cards_out: vec![entry("Counter", 1)],
        };
        assert!(matches!(
            apply_sideboard_exchange(&deck, &ambiguous, &pool, &constraints),
            Err(SideboardPlanError::CardInBothDirections(_))
        ));
        let plan = MatchupSideboardPlan {
            opponent_key: "opponent:1".into(),
            on_play: SideboardExchange::default(),
            on_draw: SideboardExchange::default(),
        };
        assert!(matches!(
            validate_sideboard_plans(&deck, &[plan.clone(), plan], &pool, &constraints),
            Err(SideboardPlanError::DuplicateOpponent(_))
        ));
    }

    #[test]
    fn validates_play_and_draw_branches_independently() {
        let (deck, pool, constraints) = fixture();
        let plan = MatchupSideboardPlan {
            opponent_key: "opponent:control".into(),
            on_play: SideboardExchange {
                cards_in: vec![entry("Counter", 4)],
                cards_out: vec![entry("Threat", 4)],
            },
            on_draw: SideboardExchange {
                cards_in: vec![entry("Removal", 2)],
                cards_out: vec![entry("Threat", 2)],
            },
        };
        validate_sideboard_plans(&deck, &[plan], &pool, &constraints).unwrap();
    }
}
