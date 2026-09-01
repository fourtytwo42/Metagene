use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub const SCENARIO_SCHEMA_VERSION: u32 = 2;
pub const CANONICAL_EVENT_SCHEMA_VERSION: u32 = 1;

const MAX_SCENARIOS: usize = 10_000;
const MAX_OBJECTS: usize = 512;
const MAX_ACTIONS: usize = 2_048;
const MAX_ASSERTIONS: usize = 512;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScenarioSuite {
    pub schema_version: u32,
    pub suite_id: String,
    pub suite_version: u32,
    pub scenarios: Vec<ScenarioDefinition>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScenarioDefinition {
    pub id: String,
    pub version: u32,
    pub title: String,
    pub mode: String,
    pub seats: u8,
    pub seed: i64,
    pub cards: Vec<String>,
    pub initial_state: ScenarioState,
    pub actions: Vec<ScenarioAction>,
    pub assertions: Vec<ScenarioAssertion>,
    pub event_projection: EventProjection,
    /// Exact Oracle/action claims exercised by the scenario. Omitting this
    /// keeps a legacy card-level scenario inspectable without granting
    /// clause-level verification.
    #[serde(default)]
    pub claims: Vec<ScenarioClaim>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ScenarioClaim {
    pub card_name: String,
    pub subject_id: String,
    pub assertion: String,
    pub ai_decision: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScenarioState {
    pub turn: u32,
    pub phase: String,
    pub active_player: u8,
    pub priority_player: u8,
    pub players: Vec<ScenarioPlayer>,
    pub objects: Vec<ScenarioObject>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScenarioPlayer {
    pub seat: u8,
    pub life: i32,
    #[serde(default)]
    pub poison: u8,
    #[serde(default)]
    pub energy: u32,
    #[serde(default)]
    pub mana_pool: BTreeMap<String, u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScenarioObject {
    pub alias: String,
    pub card: String,
    pub owner: u8,
    #[serde(default)]
    pub controller: Option<u8>,
    pub zone: String,
    #[serde(default)]
    pub position: Option<u32>,
    #[serde(default)]
    pub tapped: bool,
    #[serde(default)]
    pub token: bool,
    #[serde(default)]
    pub face: Option<String>,
    #[serde(default)]
    pub counters: BTreeMap<String, i32>,
    #[serde(default)]
    pub annotations: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScenarioAction {
    pub sequence: u32,
    pub actor: u8,
    pub action: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default)]
    pub choices: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScenarioAssertion {
    pub path: String,
    pub operator: String,
    pub expected: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventProjection {
    pub kinds: Vec<String>,
    #[serde(default)]
    pub payload_fields: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CanonicalEvent {
    pub sequence: u64,
    pub kind: String,
    #[serde(default)]
    pub actor: Option<u8>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AssertionResult {
    pub path: String,
    pub operator: String,
    pub expected: Value,
    pub actual: Value,
    pub passed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AssertionReport {
    pub passed: bool,
    pub outcome: BTreeMap<String, Value>,
    pub assertions: Vec<AssertionResult>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ContractError {
    #[error("invalid scenario suite: {0}")]
    InvalidSuite(String),
    #[error("invalid scenario {index}: {message}")]
    InvalidScenario { index: usize, message: String },
    #[error("invalid canonical event {index}: {message}")]
    InvalidEvent { index: usize, message: String },
    #[error("invalid normalized outcome: {0}")]
    InvalidOutcome(String),
    #[error("failed to serialize canonical events: {0}")]
    Serialization(String),
}

pub fn parse_scenario_suite(json: &str) -> Result<ScenarioSuite, ContractError> {
    let suite: ScenarioSuite = serde_json::from_str(json)
        .map_err(|error| ContractError::InvalidSuite(error.to_string()))?;
    validate_scenario_suite(&suite)?;
    Ok(suite)
}

pub fn validate_scenario_suite(suite: &ScenarioSuite) -> Result<(), ContractError> {
    if suite.schema_version != SCENARIO_SCHEMA_VERSION {
        return Err(ContractError::InvalidSuite(format!(
            "unsupported schema {}; expected {SCENARIO_SCHEMA_VERSION}",
            suite.schema_version
        )));
    }
    validate_identifier(&suite.suite_id, 120)
        .map_err(|message| ContractError::InvalidSuite(format!("suite_id {message}")))?;
    if suite.suite_version == 0 {
        return Err(ContractError::InvalidSuite(
            "suite_version must be at least 1".into(),
        ));
    }
    if suite.scenarios.is_empty() || suite.scenarios.len() > MAX_SCENARIOS {
        return Err(ContractError::InvalidSuite(format!(
            "scenarios must contain 1–{MAX_SCENARIOS} entries"
        )));
    }
    let mut identities = BTreeSet::new();
    for (index, scenario) in suite.scenarios.iter().enumerate() {
        validate_scenario(scenario, index + 1)?;
        if !identities.insert((scenario.id.to_lowercase(), scenario.version)) {
            return Err(ContractError::InvalidScenario {
                index: index + 1,
                message: "duplicate scenario id/version".into(),
            });
        }
    }
    Ok(())
}

pub fn canonical_event_hash(events: &[CanonicalEvent]) -> Result<String, ContractError> {
    if events.is_empty() {
        return Err(ContractError::InvalidEvent {
            index: 0,
            message: "event stream must not be empty".into(),
        });
    }
    let mut normalized = Vec::with_capacity(events.len());
    for (index, event) in events.iter().enumerate() {
        let expected_sequence = index as u64 + 1;
        if event.sequence != expected_sequence {
            return Err(ContractError::InvalidEvent {
                index: index + 1,
                message: format!(
                    "sequence must be contiguous; expected {expected_sequence}, got {}",
                    event.sequence
                ),
            });
        }
        validate_identifier(&event.kind, 120).map_err(|message| ContractError::InvalidEvent {
            index: index + 1,
            message: format!("kind {message}"),
        })?;
        if event.targets.len() > 64 {
            return Err(ContractError::InvalidEvent {
                index: index + 1,
                message: "targets exceeds 64 entries".into(),
            });
        }
        let mut event = event.clone();
        canonicalize_json(&mut event.payload);
        normalized.push(event);
    }
    let bytes = serde_json::to_vec(&normalized)
        .map_err(|error| ContractError::Serialization(error.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(b"metagene.canonical-events.v1\0");
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

pub fn project_events(
    events: &[CanonicalEvent],
    projection: &EventProjection,
) -> Result<Vec<CanonicalEvent>, ContractError> {
    let selected = projection.kinds.iter().collect::<BTreeSet<_>>();
    let mut projected = Vec::new();
    for event in events {
        if !selected.contains(&event.kind) {
            continue;
        }
        let mut event = event.clone();
        event.sequence = projected.len() as u64 + 1;
        if let Some(fields) = projection.payload_fields.get(&event.kind) {
            let payload = event
                .payload
                .as_object()
                .ok_or_else(|| ContractError::InvalidEvent {
                    index: event.sequence as usize,
                    message: format!(
                        "projected event '{}' requires an object payload",
                        event.kind
                    ),
                })?;
            let mut selected_payload = serde_json::Map::new();
            for field in fields {
                if let Some(value) = payload.get(field) {
                    selected_payload.insert(field.clone(), value.clone());
                }
            }
            event.payload = Value::Object(selected_payload);
        }
        projected.push(event);
    }
    Ok(projected)
}

pub fn evaluate_assertions(
    assertions: &[ScenarioAssertion],
    final_state: &Value,
) -> Result<AssertionReport, ContractError> {
    let mut outcome = BTreeMap::new();
    let mut results = Vec::with_capacity(assertions.len());
    for assertion in assertions {
        let actual = final_state
            .pointer(&assertion.path)
            .cloned()
            .ok_or_else(|| {
                ContractError::InvalidOutcome(format!(
                    "final state has no value at JSON Pointer '{}'",
                    assertion.path
                ))
            })?;
        let passed = assertion_matches(&assertion.operator, &actual, &assertion.expected)?;
        outcome.insert(assertion.path.clone(), actual.clone());
        results.push(AssertionResult {
            path: assertion.path.clone(),
            operator: assertion.operator.clone(),
            expected: assertion.expected.clone(),
            actual,
            passed,
        });
    }
    Ok(AssertionReport {
        passed: results.iter().all(|result| result.passed),
        outcome,
        assertions: results,
    })
}

fn assertion_matches(
    operator: &str,
    actual: &Value,
    expected: &Value,
) -> Result<bool, ContractError> {
    match operator {
        "equals" => Ok(actual == expected),
        "not_equals" => Ok(actual != expected),
        "contains" => match (actual, expected) {
            (Value::Array(values), expected) => Ok(values.contains(expected)),
            (Value::String(value), Value::String(expected)) => Ok(value.contains(expected)),
            _ => Err(ContractError::InvalidOutcome(
                "contains requires an array value or two strings".into(),
            )),
        },
        "count_equals" => {
            let expected = expected.as_u64().ok_or_else(|| {
                ContractError::InvalidOutcome(
                    "count_equals requires a non-negative integer expectation".into(),
                )
            })?;
            let actual = match actual {
                Value::Array(values) => values.len() as u64,
                Value::Object(values) => values.len() as u64,
                Value::String(value) => value.chars().count() as u64,
                _ => {
                    return Err(ContractError::InvalidOutcome(
                        "count_equals requires an array, object, or string value".into(),
                    ));
                }
            };
            Ok(actual == expected)
        }
        "at_least" | "at_most" => {
            let actual = actual.as_f64().ok_or_else(|| {
                ContractError::InvalidOutcome(format!("{operator} requires numeric values"))
            })?;
            let expected = expected.as_f64().ok_or_else(|| {
                ContractError::InvalidOutcome(format!("{operator} requires numeric values"))
            })?;
            Ok(if operator == "at_least" {
                actual >= expected
            } else {
                actual <= expected
            })
        }
        _ => Err(ContractError::InvalidOutcome(format!(
            "unsupported assertion operator '{operator}'"
        ))),
    }
}

fn validate_scenario(scenario: &ScenarioDefinition, index: usize) -> Result<(), ContractError> {
    let invalid = |message| ContractError::InvalidScenario { index, message };
    validate_identifier(&scenario.id, 160).map_err(|message| invalid(format!("id {message}")))?;
    validate_text(&scenario.title, 240).map_err(|message| invalid(format!("title {message}")))?;
    if scenario.version == 0 {
        return Err(invalid("version must be at least 1".into()));
    }
    if !matches!(scenario.mode.as_str(), "constructed" | "commander") {
        return Err(invalid("mode must be constructed or commander".into()));
    }
    if !(2..=6).contains(&scenario.seats) {
        return Err(invalid("seats must be between 2 and 6".into()));
    }
    if scenario.mode == "constructed" && scenario.seats != 2 {
        return Err(invalid(
            "constructed scenarios require exactly 2 seats".into(),
        ));
    }
    if scenario.cards.is_empty() || scenario.cards.len() > 256 {
        return Err(invalid("cards must contain 1–256 names".into()));
    }
    let mut cards = BTreeSet::new();
    for card in &scenario.cards {
        validate_text(card, 240).map_err(|message| invalid(format!("card {message}")))?;
        if !cards.insert(card.trim().to_lowercase()) {
            return Err(invalid(format!("duplicate card name '{card}'")));
        }
    }
    validate_state(&scenario.initial_state, scenario.seats).map_err(&invalid)?;
    for object in &scenario.initial_state.objects {
        if !cards.contains(&object.card.trim().to_lowercase()) {
            return Err(invalid(format!(
                "object '{}' uses card '{}' missing from cards",
                object.alias, object.card
            )));
        }
    }
    if scenario.actions.is_empty() || scenario.actions.len() > MAX_ACTIONS {
        return Err(invalid(format!(
            "actions must contain 1–{MAX_ACTIONS} entries"
        )));
    }
    let aliases = scenario
        .initial_state
        .objects
        .iter()
        .map(|object| object.alias.as_str())
        .collect::<BTreeSet<_>>();
    for (action_index, action) in scenario.actions.iter().enumerate() {
        let expected_sequence = action_index as u32 + 1;
        if action.sequence != expected_sequence {
            return Err(invalid(format!(
                "action {} sequence must be {expected_sequence}",
                action_index + 1
            )));
        }
        if !(1..=scenario.seats).contains(&action.actor) {
            return Err(invalid(format!(
                "action {} actor is outside the seat range",
                action_index + 1
            )));
        }
        if !matches!(
            action.action.as_str(),
            "cast"
                | "activate"
                | "attack"
                | "block"
                | "pass_priority"
                | "choose"
                | "resolve_top"
                | "advance_phase"
                | "check_cast_timing"
                | "concede"
        ) {
            return Err(invalid(format!(
                "action {} has unsupported action '{}'",
                action_index + 1,
                action.action
            )));
        }
        for reference in action.source.iter().chain(action.targets.iter()) {
            if !aliases.contains(reference.as_str()) {
                return Err(invalid(format!(
                    "action {} references unknown object '{reference}'",
                    action_index + 1
                )));
            }
        }
    }
    if scenario.assertions.is_empty() || scenario.assertions.len() > MAX_ASSERTIONS {
        return Err(invalid(format!(
            "assertions must contain 1–{MAX_ASSERTIONS} entries"
        )));
    }
    for claim in &scenario.claims {
        validate_text(&claim.card_name, 200).map_err(|message| ContractError::InvalidScenario {
            index,
            message: format!("claim card_name {message}"),
        })?;
        validate_identifier(&claim.subject_id, 256).map_err(|message| {
            ContractError::InvalidScenario {
                index,
                message: format!("claim subject_id {message}"),
            }
        })?;
        if claim.assertion.trim().is_empty() {
            return Err(ContractError::InvalidScenario {
                index,
                message: "claim assertion must not be empty".into(),
            });
        }
    }
    for (assertion_index, assertion) in scenario.assertions.iter().enumerate() {
        if !assertion.path.starts_with('/') || assertion.path.len() > 512 {
            return Err(invalid(format!(
                "assertion {} path must be a JSON Pointer of at most 512 bytes",
                assertion_index + 1
            )));
        }
        if !matches!(
            assertion.operator.as_str(),
            "equals" | "not_equals" | "contains" | "count_equals" | "at_least" | "at_most"
        ) {
            return Err(invalid(format!(
                "assertion {} has unsupported operator '{}'",
                assertion_index + 1,
                assertion.operator
            )));
        }
    }
    if scenario.event_projection.kinds.is_empty() {
        return Err(invalid("event_projection.kinds must not be empty".into()));
    }
    let mut kinds = BTreeSet::new();
    for kind in &scenario.event_projection.kinds {
        validate_identifier(kind, 120)
            .map_err(|message| invalid(format!("event kind {message}")))?;
        if !kinds.insert(kind) {
            return Err(invalid(format!("duplicate event kind '{kind}'")));
        }
    }
    for (kind, fields) in &scenario.event_projection.payload_fields {
        if !kinds.contains(kind) {
            return Err(invalid(format!(
                "payload fields reference unprojected event kind '{kind}'"
            )));
        }
        for field in fields {
            validate_identifier(field, 120)
                .map_err(|message| invalid(format!("payload field {message}")))?;
        }
    }
    Ok(())
}

fn validate_state(state: &ScenarioState, seats: u8) -> Result<(), String> {
    if state.turn == 0 {
        return Err("initial_state.turn must be at least 1".into());
    }
    validate_identifier(&state.phase, 80)
        .map_err(|message| format!("initial_state.phase {message}"))?;
    for (field, seat) in [
        ("active_player", state.active_player),
        ("priority_player", state.priority_player),
    ] {
        if !(1..=seats).contains(&seat) {
            return Err(format!("initial_state.{field} is outside the seat range"));
        }
    }
    let player_seats = state
        .players
        .iter()
        .map(|player| player.seat)
        .collect::<BTreeSet<_>>();
    let expected_seats = (1..=seats).collect::<BTreeSet<_>>();
    if player_seats != expected_seats || state.players.len() != seats as usize {
        return Err("initial_state.players must contain every seat exactly once".into());
    }
    if state.objects.len() > MAX_OBJECTS {
        return Err(format!(
            "initial_state.objects exceeds {MAX_OBJECTS} entries"
        ));
    }
    let mut aliases = BTreeSet::new();
    for object in &state.objects {
        validate_identifier(&object.alias, 120)
            .map_err(|message| format!("object alias {message}"))?;
        validate_text(&object.card, 240).map_err(|message| format!("object card {message}"))?;
        if !aliases.insert(object.alias.as_str()) {
            return Err(format!("duplicate object alias '{}'", object.alias));
        }
        if !(1..=seats).contains(&object.owner) {
            return Err(format!(
                "object '{}' owner is outside the seat range",
                object.alias
            ));
        }
        if object
            .controller
            .is_some_and(|controller| !(1..=seats).contains(&controller))
        {
            return Err(format!(
                "object '{}' controller is outside the seat range",
                object.alias
            ));
        }
        if !matches!(
            object.zone.as_str(),
            "library" | "hand" | "battlefield" | "graveyard" | "exile" | "command" | "stack"
        ) {
            return Err(format!("object '{}' has an unsupported zone", object.alias));
        }
        if object.zone == "battlefield" && object.controller.is_none() {
            return Err(format!(
                "battlefield object '{}' requires a controller",
                object.alias
            ));
        }
    }
    Ok(())
}

fn validate_identifier(value: &str, maximum: usize) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("must not be empty".into());
    }
    if value.len() > maximum {
        return Err(format!("exceeds {maximum} bytes"));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte))
    {
        return Err("must contain only lowercase ASCII letters, digits, '.', '_' or '-'".into());
    }
    Ok(())
}

fn validate_text(value: &str, maximum: usize) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("must not be empty".into());
    }
    if value.len() > maximum {
        return Err(format!("exceeds {maximum} bytes"));
    }
    Ok(())
}

fn canonicalize_json(value: &mut Value) {
    match value {
        Value::Array(values) => values.iter_mut().for_each(canonicalize_json),
        Value::Object(values) => {
            let mut sorted = BTreeMap::new();
            for (key, mut value) in std::mem::take(values) {
                canonicalize_json(&mut value);
                sorted.insert(key, value);
            }
            values.extend(sorted);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn checked_in_parallel_lives_scenario_satisfies_action_scoped_schema_two() {
        let suite = parse_scenario_suite(include_str!(
            "../../data/conformance/scenarios.schema1.json"
        ))
        .unwrap();
        assert_eq!(suite.scenarios.len(), 10);
        assert_eq!(suite.scenarios[0].id, "tokens.parallel-lives-copy");
        assert_eq!(suite.scenarios[0].claims.len(), 2);
        assert!(
            suite.scenarios[0]
                .claims
                .iter()
                .any(|claim| claim.ai_decision)
        );
    }

    #[test]
    fn canonical_hash_ignores_payload_key_order_but_preserves_event_order() {
        let event = |sequence, kind: &str, payload| CanonicalEvent {
            sequence,
            kind: kind.into(),
            actor: Some(1),
            source: Some("mythos".into()),
            targets: vec!["parallel-lives".into()],
            payload,
        };
        let first = vec![
            event(1, "spell.cast", json!({"z":2,"a":{"y":1,"x":0}})),
            event(2, "token.created", json!({"quantity":2})),
        ];
        let reordered_keys = vec![
            event(1, "spell.cast", json!({"a":{"x":0,"y":1},"z":2})),
            event(2, "token.created", json!({"quantity":2})),
        ];
        assert_eq!(
            canonical_event_hash(&first).unwrap(),
            canonical_event_hash(&reordered_keys).unwrap()
        );

        let reordered_events = vec![
            event(1, "token.created", json!({"quantity":2})),
            event(2, "spell.cast", json!({"z":2,"a":{"y":1,"x":0}})),
        ];
        assert_ne!(
            canonical_event_hash(&first).unwrap(),
            canonical_event_hash(&reordered_events).unwrap()
        );
    }

    #[test]
    fn rejects_noncontiguous_events_and_unknown_object_references() {
        let events = vec![CanonicalEvent {
            sequence: 2,
            kind: "zone.changed".into(),
            actor: None,
            source: None,
            targets: Vec::new(),
            payload: Value::Null,
        }];
        assert!(matches!(
            canonical_event_hash(&events),
            Err(ContractError::InvalidEvent { index: 1, .. })
        ));

        let mut suite = parse_scenario_suite(include_str!(
            "../../data/conformance/scenarios.schema1.json"
        ))
        .unwrap();
        suite.scenarios[0].actions[0].targets = vec!["missing".into()];
        assert!(matches!(
            validate_scenario_suite(&suite),
            Err(ContractError::InvalidScenario { index: 1, .. })
        ));
    }

    #[test]
    fn shared_assertion_evaluator_builds_a_stable_normalized_outcome() {
        let suite = parse_scenario_suite(include_str!(
            "../../data/conformance/scenarios.schema1.json"
        ))
        .unwrap();
        let final_state = json!({
            "battlefield": {"seat-1": {
                "cards": {"Parallel Lives": {"count": 3}},
                "tokens": {"Parallel Lives": {"count": 2}}
            }},
            "graveyard": {"seat-1": {
                "cards": {"Mythos of Illuna": {"count": 1}}
            }}
        });
        let report = evaluate_assertions(&suite.scenarios[0].assertions, &final_state).unwrap();
        assert!(report.passed);
        assert_eq!(report.assertions.len(), 3);
        assert_eq!(
            report.outcome["/battlefield/seat-1/tokens/Parallel Lives/count"],
            json!(2)
        );

        let mut failing_state = final_state;
        failing_state["battlefield"]["seat-1"]["tokens"]["Parallel Lives"]["count"] = json!(1);
        let report = evaluate_assertions(&suite.scenarios[0].assertions, &failing_state).unwrap();
        assert!(!report.passed);
        assert_eq!(
            report.assertions.iter().filter(|item| !item.passed).count(),
            1
        );
    }

    #[test]
    fn event_projection_filters_kinds_and_payload_fields_before_hashing() {
        let suite = parse_scenario_suite(include_str!(
            "../../data/conformance/scenarios.schema1.json"
        ))
        .unwrap();
        let events = vec![
            CanonicalEvent {
                sequence: 1,
                kind: "debug.unprojected".into(),
                actor: None,
                source: None,
                targets: Vec::new(),
                payload: json!({"unstable": 99}),
            },
            CanonicalEvent {
                sequence: 2,
                kind: "token.created".into(),
                actor: Some(1),
                source: Some("mythos".into()),
                targets: vec!["parallel-lives".into()],
                payload: json!({
                    "quantity": 2,
                    "controller": 1,
                    "card_name": "Parallel Lives",
                    "engine_internal_id": 8871
                }),
            },
        ];
        let projected = project_events(&events, &suite.scenarios[0].event_projection).unwrap();
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].sequence, 1);
        assert_eq!(
            projected[0].payload,
            json!({
                "quantity": 2,
                "controller": 1,
                "card_name": "Parallel Lives"
            })
        );
        assert!(canonical_event_hash(&projected).is_ok());
    }

    #[test]
    fn objects_must_be_declared_in_the_scenario_card_manifest() {
        let mut suite = parse_scenario_suite(include_str!(
            "../../data/conformance/scenarios.schema1.json"
        ))
        .unwrap();
        suite.scenarios[0]
            .cards
            .retain(|card| card != "Mythos of Illuna");
        assert!(matches!(
            validate_scenario_suite(&suite),
            Err(ContractError::InvalidScenario { index: 1, .. })
        ));
    }
}
