use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CardId(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Color {
    White,
    Blue,
    Black,
    Red,
    Green,
    Colorless,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ManaCostProfile {
    pub generic: u8,
    pub colored: [u8; 6],
    pub hybrid: Vec<Vec<Color>>,
    pub variable: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ManaColorRule {
    #[default]
    Fixed,
    CommanderIdentity,
    LegendaryPermanents,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ManaSpendRule {
    #[default]
    Unrestricted,
    CastLegendary,
    CastCardTypes(Vec<String>),
    CastSubtypes(Vec<String>),
    CastChosenCreatureType,
    UnknownRestricted,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ManaStateRule {
    #[default]
    None,
    RemoveCounter,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ManaSourceProfile {
    pub colors: Vec<Color>,
    pub color_rule: ManaColorRule,
    pub spend_rule: ManaSpendRule,
    pub state_rule: ManaStateRule,
    pub uses_tap: bool,
    pub activation_generic: u8,
    pub earliest_turn: u8,
    pub label: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum SearchTargetProfile {
    #[default]
    None,
    BasicLand,
    BasicLandOrSubtypes(Vec<String>),
    CardSubtypes(Vec<String>),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchActionProfile {
    pub mana_cost: ManaCostProfile,
    pub from_hand: bool,
    pub discards_self: bool,
    pub target: SearchTargetProfile,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum AbilityKind {
    #[default]
    Unknown,
    Draw,
    Destroy,
    Exile,
    Bounce,
    CounterSpell,
    AddCounters,
    GraveyardHate,
    Recursion,
    Search,
    CreateToken,
    Copy,
    TokenMultiplier,
    SacrificeOutlet,
    Ramp,
    Crew,
    Damage,
    Mill,
    AlternateWin,
    Blink,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum AbilityTiming {
    #[default]
    Spell,
    Activated,
    Triggered,
    Static,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum AbilityZone {
    #[default]
    Battlefield,
    Hand,
    Graveyard,
    Stack,
    Library,
    Exile,
    Command,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ActionTargetProfile {
    #[default]
    None,
    OwnCreature,
    OpponentCreature,
    AnyCreature,
    ArtifactOrEnchantment,
    GraveyardCard,
    OwnGraveyardCreature,
    Spell,
    BasicLandInLibrary,
    OpponentPlayer,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AbilityCostProfile {
    pub mana: ManaCostProfile,
    pub uses_tap: bool,
    pub sacrifices_self: bool,
    pub sacrifice_permanents: u8,
    pub discards_self: bool,
    pub discard_cards: u8,
    pub exiles_self: bool,
    pub exile_cards: u8,
    pub life: u8,
    pub remove_counters: u8,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AbilityProfile {
    /// Stable Oracle-clause identities consumed by this parsed action.
    pub clause_ids: Vec<String>,
    pub label: String,
    pub kind: AbilityKind,
    pub timing: AbilityTiming,
    pub zone: AbilityZone,
    pub cost: AbilityCostProfile,
    pub target: ActionTargetProfile,
    pub conditions: Vec<String>,
    pub effect_value: f32,
    pub narrow: bool,
    pub once_per_turn: bool,
    /// One for an exact supported template, less than one for conservative
    /// fallback parsing. Unknown text is capped at 0.10 by the catalog.
    pub parser_confidence: f32,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ClauseClassification {
    ParsedGameplayRule,
    RecognizedKeywordRule,
    ReminderText,
    #[default]
    UnresolvedGameplayResidual,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ClauseCriticality {
    RuleCritical,
    OutcomeCritical,
    #[default]
    Informational,
}

impl ClauseCriticality {
    pub fn blocks_claim(self) -> bool {
        matches!(self, Self::RuleCritical | Self::OutcomeCritical)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct OracleClause {
    pub id: String,
    pub mode_name: String,
    pub ordinal: u16,
    pub text: String,
    pub normalized_text: String,
    pub classification: ClauseClassification,
    pub criticality: ClauseCriticality,
    pub parser_confidence: f32,
    pub action_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct OracleCoverageSummary {
    pub gameplay_clauses: u32,
    pub accounted_clauses: u32,
    pub parsed_clauses: u32,
    pub unresolved_clauses: u32,
    pub accounted_clause_rate: f32,
    pub parsed_clause_rate: f32,
    pub unresolved_clause_rate: f32,
    pub unresolved_text: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum RuleTiming {
    #[default]
    Spell,
    Trigger,
    Activation,
    Static,
    Replacement,
    Mana,
    SpecialAction,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum AmountExpression {
    #[default]
    One,
    Constant(i32),
    Variable(String),
    Counted(String),
    X,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TokenProfile {
    pub name: String,
    pub types: Vec<String>,
    pub subtypes: Vec<String>,
    pub colors: Vec<Color>,
    pub power: Option<AmountExpression>,
    pub toughness: Option<AmountExpression>,
    pub keywords: Vec<String>,
    pub tapped: bool,
    pub attacking: bool,
    pub copy_of_target: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct CounterProfile {
    pub name: String,
    pub amount: AmountExpression,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum RuleCost {
    #[default]
    None,
    Mana(ManaCostProfile),
    TapSource,
    Sacrifice {
        object: String,
        amount: AmountExpression,
    },
    Discard {
        object: String,
        amount: AmountExpression,
    },
    Exile {
        object: String,
        amount: AmountExpression,
    },
    PayLife(AmountExpression),
    RemoveCounters(CounterProfile),
    Additional(String),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum RuleCondition {
    #[default]
    Always,
    Event(String),
    InterveningIf(String),
    AsLongAs(String),
    Unless(String),
    Limit(String),
    Text(String),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RuleTarget {
    pub controller: String,
    pub zones: Vec<AbilityZone>,
    pub card_types: Vec<String>,
    pub subtypes: Vec<String>,
    pub restrictions: Vec<String>,
    pub amount: AmountExpression,
    pub optional: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum RuleEffect {
    #[default]
    NoOp,
    MoveCard {
        from: AbilityZone,
        to: AbilityZone,
        object: String,
    },
    Draw {
        player: String,
        amount: AmountExpression,
    },
    Discard {
        player: String,
        amount: AmountExpression,
    },
    Life {
        player: String,
        amount: AmountExpression,
        gain: bool,
    },
    ModifyPowerToughness {
        power: AmountExpression,
        toughness: AmountExpression,
    },
    ModifyTypes {
        add: Vec<String>,
        remove: Vec<String>,
    },
    GrantKeyword {
        keyword: String,
    },
    Tap {
        untap: bool,
    },
    AddMana {
        colors: Vec<Color>,
        amount: AmountExpression,
    },
    PutCounters(CounterProfile),
    CreateToken {
        token: TokenProfile,
        amount: AmountExpression,
    },
    Damage {
        recipient: String,
        amount: AmountExpression,
    },
    Prevent {
        event: String,
        amount: AmountExpression,
    },
    Destroy {
        object: String,
    },
    Sacrifice {
        object: String,
    },
    Exile {
        object: String,
    },
    Search {
        zone: AbilityZone,
        destination: AbilityZone,
        object: String,
    },
    CastPermission {
        zone: AbilityZone,
        object: String,
    },
    CostChange {
        object: String,
        amount: AmountExpression,
        increase: bool,
    },
    ControlChange {
        object: String,
        duration: String,
    },
    Copy {
        object: String,
        as_token: bool,
    },
    Replace {
        event: String,
        replacement: String,
    },
    Transform,
    ExtraCombat,
    ExtraTurn,
    WinLoss {
        player: String,
        wins: bool,
    },
    Mill {
        player: String,
        amount: AmountExpression,
    },
    Perpetual {
        modification: String,
    },
    Conjure {
        object: String,
        destination: AbilityZone,
    },
    Seek {
        object: String,
    },
    Text(String),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RuleAbility {
    pub id: String,
    pub clause_ids: Vec<String>,
    pub label: String,
    pub timing: RuleTiming,
    pub source_zones: Vec<AbilityZone>,
    pub costs: Vec<RuleCost>,
    pub targets: Vec<RuleTarget>,
    pub conditions: Vec<RuleCondition>,
    pub choices: Vec<String>,
    pub effects: Vec<RuleEffect>,
    pub duration: Option<String>,
    pub delayed_effects: Vec<RuleEffect>,
    pub once_per_turn: bool,
    pub once_per_use: bool,
    pub parser_confidence: f32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CardMode {
    pub name: String,
    pub type_line: String,
    pub colors: Vec<Color>,
    pub mana_cost: ManaCostProfile,
    /// Whether this face may be cast normally from hand. A nonland face with
    /// no printed mana cost is not a zero-mana spell (for example Living End).
    /// `None` retains compatibility with legacy serialized catalogs.
    pub normal_castable: Option<bool>,
    pub is_land: bool,
    pub is_legendary: bool,
    pub creature_types: Vec<String>,
    pub mana_source_colors: Vec<Color>,
    pub late_mana_source_colors: Vec<Color>,
    pub mana_sources: Vec<ManaSourceProfile>,
    pub abilities: Vec<AbilityProfile>,
    /// Lossless, ordered rules interpretation. `abilities` is retained as a
    /// derived compatibility summary for persisted callers and hot loops.
    pub rule_abilities: Vec<RuleAbility>,
    pub oracle_clauses: Vec<OracleClause>,
    pub enters_tapped: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MechanicProfile {
    pub requires_creature: bool,
    pub requires_humans: bool,
    pub requires_death: bool,
    pub provides_death: bool,
    pub exile_cost: bool,
    pub reanimation: bool,
    pub graveyard_setup: bool,
    pub legendary_payoff: bool,
    pub crew_power: u8,
    pub devotion_threshold: u8,
    pub devotion_color_mask: u8,
    pub searches_basic_land: bool,
    pub search_action: Option<SearchActionProfile>,
    pub opponent_discard_supply: u8,
    pub opponent_discard_demand: u8,
    pub sacrifice_outlet_supply: u8,
    pub sacrifice_fodder_supply: u8,
    pub sacrifice_demand: u8,
    pub graveyard_target_supply: u8,
    pub graveyard_target_demand: u8,
    pub graveyard_setup_supply: u8,
    pub graveyard_setup_demand: u8,
    pub legendary_cast_demand: u8,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MechanicDescriptor {
    pub id: String,
    pub label: String,
    pub source: String,
    pub critical: bool,
    pub criticality: ClauseCriticality,
    pub parser_confidence: f32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StructuralEvaluationContext {
    pub platform: String,
    pub format: String,
    pub commander: bool,
    pub commander_colors: Vec<Color>,
    pub opponent_field: OpponentFieldProfile,
    /// Internal hot-loop mode. Evolution keeps all scalar/blocking evidence but
    /// defers verbose per-card explanations until finalists are selected.
    #[serde(default)]
    pub compact_evaluation: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct OpponentFieldProfile {
    pub available: bool,
    pub creature_rate: f32,
    pub artifact_or_enchantment_rate: f32,
    pub graveyard_rate: f32,
    pub stack_spell_rate: f32,
}

impl Default for OpponentFieldProfile {
    fn default() -> Self {
        Self {
            available: false,
            creature_rate: 0.70,
            artifact_or_enchantment_rate: 0.25,
            graveyard_rate: 0.30,
            stack_spell_rate: 0.75,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CardFeatures {
    pub is_land: bool,
    pub unlimited_copies: bool,
    pub enters_tapped: bool,
    pub mana_value: f32,
    pub mana_production: f32,
    pub ramp: f32,
    pub card_advantage: f32,
    pub interaction: f32,
    pub pressure: f32,
    pub resilience: f32,
    pub tags: Vec<String>,
    pub is_legendary: bool,
    pub can_be_land: bool,
    pub initial_modes: Vec<CardMode>,
    pub mechanics: MechanicProfile,
    pub mechanic_inventory: Vec<MechanicDescriptor>,
    pub oracle_coverage: OracleCoverageSummary,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Card {
    pub id: CardId,
    pub arena_id: Option<u64>,
    pub platforms: Vec<String>,
    pub name: String,
    pub set_code: String,
    pub collector_number: String,
    pub type_line: String,
    pub oracle_text: String,
    pub colors: Vec<Color>,
    pub color_identity: Vec<Color>,
    pub image_uri: Option<String>,
    pub legalities: BTreeMap<String, String>,
    pub features: CardFeatures,
}

impl Card {
    /// Returns whether at least one initially playable face belongs to the
    /// user's exact deck-color selection. `Colorless` is an explicit choice:
    /// a mono-red selection does not silently admit generic-cost artifacts,
    /// while `[Blue, Colorless]` admits both blue and colorless spells.
    pub fn matches_color_selection(&self, allowed: &[Color]) -> bool {
        if allowed.is_empty() {
            return false;
        }
        let allows = |color: Color| allowed.contains(&color);
        let mode_matches = |mode: &CardMode| {
            if mode.is_land {
                if mode.mana_sources.is_empty() {
                    return allows(Color::Colorless);
                }
                return mode
                    .mana_sources
                    .iter()
                    .any(|source| match source.color_rule {
                        ManaColorRule::Fixed => source.colors.iter().copied().any(&allows),
                        ManaColorRule::CommanderIdentity | ManaColorRule::LegendaryPermanents => {
                            allowed.iter().any(|color| *color != Color::Colorless)
                        }
                    });
            }

            let has_colored_requirement = mode.mana_cost.colored[..5].iter().any(|pips| *pips > 0)
                || mode
                    .mana_cost
                    .hybrid
                    .iter()
                    .any(|options| options.iter().any(|color| *color != Color::Colorless));
            let is_colorless_spell = mode.colors.is_empty() && !has_colored_requirement;
            if is_colorless_spell && !allows(Color::Colorless) {
                return false;
            }
            if mode.mana_cost.colored[5] > 0 && !allows(Color::Colorless) {
                return false;
            }
            mode.mana_cost.colored[..5]
                .iter()
                .enumerate()
                .all(|(slot, pips)| {
                    *pips == 0
                        || allows(
                            [
                                Color::White,
                                Color::Blue,
                                Color::Black,
                                Color::Red,
                                Color::Green,
                            ][slot],
                        )
                })
                && mode
                    .mana_cost
                    .hybrid
                    .iter()
                    .all(|options| options.iter().copied().any(&allows))
        };

        if !self.features.initial_modes.is_empty() {
            return self.features.initial_modes.iter().any(mode_matches);
        }
        if self.features.is_land {
            return allows(Color::Colorless) || self.colors.iter().copied().any(&allows);
        }
        if self.colors.is_empty() {
            allows(Color::Colorless)
        } else {
            self.colors.iter().copied().all(allows)
        }
    }

    pub fn enrich_derived_tags(&mut self) {
        let text = self.oracle_text.to_ascii_lowercase();
        let type_line = self.type_line.to_ascii_lowercase();
        for (tag, matches) in [
            ("token", text.contains("token")),
            ("copy", text.contains("copy")),
            ("draw", text.contains("draw a card")),
            (
                "ramp",
                text.contains("add {") || text.contains("search your library for a basic land"),
            ),
            (
                "interaction",
                text.contains("destroy target")
                    || text.contains("exile target")
                    || text.contains("counter target")
                    || text.contains("return target"),
            ),
            ("recursion", text.contains("from your graveyard")),
            ("sacrifice", text.contains("sacrifice")),
            ("graveyard", text.contains("graveyard")),
            ("discard", text.contains("discard")),
            (
                "lifegain",
                (text.contains("gain") || text.contains("gains")) && text.contains(" life"),
            ),
            (
                "counters",
                text.contains("counter on")
                    || text.contains("counters on")
                    || text.contains("+1/+1 counter")
                    || text.contains("proliferate"),
            ),
            (
                "death",
                text.contains("dies") || text.contains("creature die"),
            ),
            ("mill", text.contains("mill")),
            (
                "blink",
                text.contains("exile") && text.contains("return") && text.contains("battlefield"),
            ),
            (
                "landfall",
                text.contains("landfall")
                    || text.contains("land enters the battlefield under your control"),
            ),
            (
                "spellslinger",
                text.contains("instant or sorcery") || text.contains("instant and sorcery"),
            ),
            ("treasure", text.contains("treasure")),
            ("clue", text.contains("clue")),
            ("food", text.contains("food")),
            (
                "energy",
                text.contains("energy counter") || text.contains("{e}"),
            ),
            ("equipment", type_line.contains("equipment")),
            ("aura", type_line.contains("aura")),
            ("enchantment", type_line.contains("enchantment")),
            ("creature", type_line.contains("creature")),
            ("artifact", type_line.contains("artifact")),
        ] {
            if matches && !self.features.tags.iter().any(|current| current == tag) {
                self.features.tags.push(tag.into());
            }
        }

        if type_line.contains("creature") {
            for face in type_line.split("//") {
                let Some((_, subtypes)) = face.split_once('—') else {
                    continue;
                };
                for subtype in subtypes.split(|character: char| !character.is_alphanumeric()) {
                    if subtype.len() < 2
                        || matches!(
                            subtype,
                            "and" | "artifact" | "creature" | "enchantment" | "land"
                        )
                    {
                        continue;
                    }
                    let tag = format!("type:{subtype}");
                    if !self.features.tags.contains(&tag) {
                        self.features.tags.push(tag);
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CardPool {
    pub cards: Vec<Card>,
}

impl CardPool {
    pub fn card(&self, id: &CardId) -> Option<&Card> {
        self.cards.iter().find(|card| &card.id == id)
    }

    pub fn index_by_id(&self) -> HashMap<&CardId, usize> {
        self.cards
            .iter()
            .enumerate()
            .map(|(index, card)| (&card.id, index))
            .collect()
    }

    pub fn focus_on_names(&self, names: &[String]) -> Self {
        if names.is_empty() {
            return self.clone();
        }
        let names = names
            .iter()
            .map(|name| name.to_lowercase())
            .collect::<std::collections::HashSet<_>>();
        Self {
            cards: self
                .cards
                .iter()
                .filter(|card| names.contains(&card.name.to_lowercase()))
                .cloned()
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeckEntry {
    pub card_id: CardId,
    pub quantity: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deck {
    pub name: String,
    #[serde(default)]
    pub command_zone: Vec<DeckEntry>,
    pub mainboard: Vec<DeckEntry>,
    #[serde(default)]
    pub sideboard: Vec<DeckEntry>,
}

impl Deck {
    pub fn mainboard_count(&self) -> usize {
        self.mainboard
            .iter()
            .map(|entry| entry.quantity as usize)
            .sum()
    }

    pub fn sideboard_count(&self) -> usize {
        self.sideboard
            .iter()
            .map(|entry| entry.quantity as usize)
            .sum()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LockedCard {
    pub card_id: CardId,
    pub minimum: u8,
    pub maximum: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeckConstraints {
    pub target_mainboard_size: usize,
    #[serde(default)]
    pub target_sideboard_size: usize,
    pub minimum_commanders: usize,
    pub maximum_commanders: usize,
    pub default_max_copies: u8,
    #[serde(default)]
    pub locked_cards: Vec<LockedCard>,
    pub minimum_lands: Option<usize>,
    pub maximum_lands: Option<usize>,
    /// `None` lets blank evolution infer a palette. `Some` is an exact set of
    /// playable spell colors and may contain `Colorless` by itself or beside
    /// one or more W/U/B/R/G colors.
    #[serde(default)]
    pub allowed_colors: Option<Vec<Color>>,
}

impl Default for DeckConstraints {
    fn default() -> Self {
        Self {
            target_mainboard_size: 60,
            target_sideboard_size: 0,
            minimum_commanders: 0,
            maximum_commanders: 0,
            default_max_copies: 4,
            locked_cards: Vec::new(),
            minimum_lands: Some(20),
            maximum_lands: Some(28),
            allowed_colors: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FitnessWeights {
    pub consistency: f64,
    pub tempo: f64,
    pub synergy: f64,
    pub interaction: f64,
    pub resilience: f64,
    pub commander: f64,
    #[serde(default = "default_sideboard_weight")]
    pub sideboard: f64,
    pub mana_failure_penalty: f64,
    #[serde(default = "default_legendary_dead_draw_penalty")]
    pub legendary_dead_draw_penalty: f64,
    #[serde(default = "default_unsupported_synergy_penalty")]
    pub unsupported_synergy_penalty: f64,
    #[serde(default = "default_dead_action_penalty")]
    pub dead_action_penalty: f64,
    #[serde(default = "default_narrow_card_penalty")]
    pub narrow_card_penalty: f64,
    #[serde(default = "default_closing_power_weight")]
    pub closing_power: f64,
    #[serde(default = "default_role_balance_weight")]
    pub role_balance: f64,
}

fn default_sideboard_weight() -> f64 {
    8.0
}

fn default_legendary_dead_draw_penalty() -> f64 {
    12.0
}

fn default_unsupported_synergy_penalty() -> f64 {
    20.0
}

fn default_dead_action_penalty() -> f64 {
    18.0
}

fn default_narrow_card_penalty() -> f64 {
    12.0
}

fn default_closing_power_weight() -> f64 {
    18.0
}

fn default_role_balance_weight() -> f64 {
    10.0
}

impl Default for FitnessWeights {
    fn default() -> Self {
        Self {
            consistency: 30.0,
            tempo: 24.0,
            synergy: 20.0,
            interaction: 12.0,
            resilience: 14.0,
            commander: 8.0,
            sideboard: default_sideboard_weight(),
            mana_failure_penalty: 35.0,
            legendary_dead_draw_penalty: default_legendary_dead_draw_penalty(),
            unsupported_synergy_penalty: default_unsupported_synergy_penalty(),
            dead_action_penalty: default_dead_action_penalty(),
            narrow_card_penalty: default_narrow_card_penalty(),
            closing_power: default_closing_power_weight(),
            role_balance: default_role_balance_weight(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvolutionMode {
    Tune,
    #[default]
    Rebuild,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TunePolicy {
    pub root_deck: Option<Deck>,
    pub strategy_retention_floor: f64,
    pub primary_package_floor: f64,
    pub maximum_nonland_edit_fraction: f64,
}

/// A compact, engine-independent combo package discovered before evolution.
/// The optimizer consumes causal role requirements rather than card names so a
/// Tune mutation may replace a piece with a functionally equivalent card.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ComboPackageProfile {
    pub id: String,
    pub piece_ids: Vec<CardId>,
    pub tutor_ids: Vec<CardId>,
    pub protection_ids: Vec<CardId>,
    pub recovery_ids: Vec<CardId>,
    pub required_roles: Vec<String>,
    pub expected_turn: Option<f32>,
    pub repeatable: bool,
    pub finite_win: bool,
    pub winning: bool,
    pub proof: String,
}

impl Default for TunePolicy {
    fn default() -> Self {
        Self {
            root_deck: None,
            strategy_retention_floor: 0.70,
            primary_package_floor: 0.60,
            maximum_nonland_edit_fraction: 0.40,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvolutionConfig {
    pub population_size: usize,
    pub generations: usize,
    pub elite_count: usize,
    pub trials_per_candidate: usize,
    pub mutation_swaps: usize,
    pub mutation_rate: f32,
    #[serde(default = "default_package_mutation_rate")]
    pub package_mutation_rate: f32,
    #[serde(default = "default_package_size")]
    pub package_size: usize,
    #[serde(default = "default_mana_mutation_rate")]
    pub mana_mutation_rate: f32,
    pub tournament_size: usize,
    pub seed: u64,
    pub turn_horizon: u8,
    pub worker_threads: Option<usize>,
    #[serde(default)]
    pub evolve_commanders: bool,
    #[serde(default)]
    pub evolve_sideboard: bool,
    #[serde(default = "default_commander_mutation_rate")]
    pub commander_mutation_rate: f32,
    #[serde(default)]
    pub learned_packages: Vec<Vec<CardId>>,
    #[serde(default)]
    pub combo_packages: Vec<ComboPackageProfile>,
    #[serde(default)]
    pub fitness_weights: FitnessWeights,
    #[serde(default)]
    pub evolution_mode: EvolutionMode,
    #[serde(default)]
    pub tune_policy: TunePolicy,
}

fn default_commander_mutation_rate() -> f32 {
    0.08
}

fn default_package_mutation_rate() -> f32 {
    0.35
}

fn default_package_size() -> usize {
    3
}

fn default_mana_mutation_rate() -> f32 {
    0.35
}

impl Default for EvolutionConfig {
    fn default() -> Self {
        Self {
            population_size: 128,
            generations: 50,
            elite_count: 12,
            trials_per_candidate: 256,
            mutation_swaps: 2,
            mutation_rate: 0.85,
            package_mutation_rate: default_package_mutation_rate(),
            package_size: default_package_size(),
            mana_mutation_rate: default_mana_mutation_rate(),
            tournament_size: 4,
            seed: 0x4D45_5441_4745_4E45,
            turn_horizon: 6,
            worker_threads: None,
            evolve_commanders: false,
            evolve_sideboard: false,
            commander_mutation_rate: default_commander_mutation_rate(),
            learned_packages: Vec::new(),
            combo_packages: Vec::new(),
            fitness_weights: FitnessWeights::default(),
            evolution_mode: EvolutionMode::Rebuild,
            tune_policy: TunePolicy::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spell(name: &str, colors: Vec<Color>, cost: ManaCostProfile) -> Card {
        Card {
            id: CardId(name.into()),
            arena_id: None,
            platforms: vec!["arena".into()],
            name: name.into(),
            set_code: "tst".into(),
            collector_number: "1".into(),
            type_line: "Artifact".into(),
            oracle_text: String::new(),
            colors: colors.clone(),
            color_identity: colors.clone(),
            image_uri: None,
            legalities: BTreeMap::new(),
            features: CardFeatures {
                initial_modes: vec![CardMode {
                    name: name.into(),
                    type_line: "Artifact".into(),
                    colors,
                    mana_cost: cost,
                    ..CardMode::default()
                }],
                ..CardFeatures::default()
            },
        }
    }

    #[test]
    fn exact_color_selection_distinguishes_colored_and_colorless_spells() {
        let mut red_cost = ManaCostProfile::default();
        red_cost.colored[3] = 1;
        let red = spell("Red spell", vec![Color::Red], red_cost);
        let artifact = spell(
            "Colorless artifact",
            Vec::new(),
            ManaCostProfile {
                generic: 2,
                ..ManaCostProfile::default()
            },
        );

        assert!(red.matches_color_selection(&[Color::Red]));
        assert!(red.matches_color_selection(&[Color::Red, Color::Colorless]));
        assert!(!red.matches_color_selection(&[Color::Blue, Color::Colorless]));
        assert!(!artifact.matches_color_selection(&[Color::Red]));
        assert!(artifact.matches_color_selection(&[Color::Colorless]));
        assert!(artifact.matches_color_selection(&[Color::Blue, Color::Colorless]));
        assert!(!artifact.matches_color_selection(&[]));
    }

    #[test]
    fn exact_color_selection_respects_hybrid_routes_and_colorless_pips() {
        let hybrid = spell(
            "Hybrid spell",
            vec![Color::Blue, Color::Green],
            ManaCostProfile {
                hybrid: vec![vec![Color::Blue, Color::Green]],
                ..ManaCostProfile::default()
            },
        );
        let mut colored_colorless = ManaCostProfile::default();
        colored_colorless.colored[1] = 1;
        colored_colorless.colored[5] = 1;
        let blue_colorless = spell("Blue plus C", vec![Color::Blue], colored_colorless);

        assert!(hybrid.matches_color_selection(&[Color::Blue]));
        assert!(hybrid.matches_color_selection(&[Color::Green]));
        assert!(!hybrid.matches_color_selection(&[Color::Red]));
        assert!(!blue_colorless.matches_color_selection(&[Color::Blue]));
        assert!(blue_colorless.matches_color_selection(&[Color::Blue, Color::Colorless]));
    }
}
