use crate::{
    conformance::ConformanceObservation,
    conformance_contract::{
        CanonicalEvent, ContractError, ScenarioDefinition, canonical_event_hash,
        evaluate_assertions, project_events,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

const RESULT_PREFIX: &str = "METAGENE_HEXDEK_SCENARIO_RESULT ";
pub const HEXDEK_ADAPTER_SOURCE: &[u8] = include_bytes!("../resources/hexdek-adapter/main.go");

#[derive(Debug, Error)]
pub enum HexdekConformanceError {
    #[error("HexDek conformance adapter is not installed under {0}")]
    NotInstalled(String),
    #[error("HexDek conformance file error: {0}")]
    Io(#[from] std::io::Error),
    #[error("HexDek conformance JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("HexDek conformance adapter exited unsuccessfully: {0}")]
    Process(String),
    #[error("HexDek conformance adapter returned an invalid protocol: {0}")]
    InvalidProtocol(String),
    #[error("canonical conformance contract error: {0}")]
    Contract(#[from] ContractError),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HexdekRecordedEvent {
    #[serde(rename = "Kind")]
    pub kind: String,
    #[serde(rename = "Seat")]
    pub seat: i32,
    #[serde(rename = "Target")]
    pub target: i32,
    #[serde(rename = "Source")]
    pub source: String,
    #[serde(rename = "Amount")]
    pub amount: i64,
    #[serde(rename = "Details", default)]
    pub details: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HexdekScenarioProbe {
    pub protocol_version: u32,
    pub scenario_id: String,
    pub scenario_version: u32,
    pub seed: i64,
    pub events: Vec<HexdekRecordedEvent>,
    pub final_state: Value,
    #[serde(default)]
    pub error: Option<String>,
}

struct HexdekPaths {
    root: PathBuf,
    executable: PathBuf,
    ast: PathBuf,
    version: PathBuf,
}

pub fn run_scenario_probe(
    data_dir: &Path,
    scenario: &ScenarioDefinition,
) -> Result<HexdekScenarioProbe, HexdekConformanceError> {
    let paths = paths(data_dir);
    if !paths.executable.is_file() || !paths.ast.is_file() || !paths.version.is_file() {
        return Err(HexdekConformanceError::NotInstalled(
            paths.root.display().to_string(),
        ));
    }
    let output = Command::new(&paths.executable)
        .current_dir(&paths.root)
        .arg("--scenario")
        .arg(&scenario.id)
        .arg("--seed")
        .arg(scenario.seed.to_string())
        .arg("--ast")
        .arg(&paths.ast)
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(HexdekConformanceError::Process(trim_process_error(
            &stderr, &stdout,
        )));
    }
    let probe = parse_probe_output(&stdout)?;
    if probe.protocol_version != 1 {
        return Err(HexdekConformanceError::InvalidProtocol(format!(
            "unsupported protocol {}; expected 1",
            probe.protocol_version
        )));
    }
    if probe.scenario_id != scenario.id
        || probe.scenario_version != scenario.version
        || probe.seed != scenario.seed
    {
        return Err(HexdekConformanceError::InvalidProtocol(
            "scenario identity/version/seed does not match the request".into(),
        ));
    }
    if let Some(error) = probe.error.as_deref() {
        return Err(HexdekConformanceError::Process(error.into()));
    }
    Ok(probe)
}

pub fn run_conformance_scenario(
    data_dir: &Path,
    scenario: &ScenarioDefinition,
) -> Result<ConformanceObservation, HexdekConformanceError> {
    let probe = run_scenario_probe(data_dir, scenario)?;
    let report = evaluate_assertions(&scenario.assertions, &probe.final_state)?;
    let normalized = normalize_scenario_events(scenario, &probe)?;
    let projected = project_events(&normalized, &scenario.event_projection)?;
    let event_hash = canonical_event_hash(&projected)?;
    let engine_revision = engine_revision(data_dir)?;
    let configuration_hash =
        crate::provenance::configuration_hash("conformance-scenario-v2", scenario)?;
    Ok(ConformanceObservation {
        engine: "hexdek".into(),
        engine_revision: engine_revision.clone(),
        provenance: Some(crate::provenance::ResultProvenance::new(
            engine_revision,
            card_data_revision(data_dir)?,
            Some(crate::conformance_contract::SCENARIO_SCHEMA_VERSION),
            configuration_hash,
        )),
        scenario_id: scenario.id.clone(),
        scenario_version: scenario.version,
        seed: scenario.seed,
        passed: report.passed,
        outcome: serde_json::to_value(report.outcome)?,
        event_hash,
        cards: scenario.cards.clone(),
        claims: scenario
            .claims
            .iter()
            .map(|claim| crate::conformance::ConformanceClaim {
                card_name: claim.card_name.clone(),
                subject_id: claim.subject_id.clone(),
                assertion: claim.assertion.clone(),
                ai_decision: claim.ai_decision,
            })
            .collect(),
    })
}

fn card_data_revision(data_dir: &Path) -> Result<String, HexdekConformanceError> {
    let version = fs::read_to_string(paths(data_dir).version)?;
    let revision = version
        .lines()
        .find_map(|line| line.strip_prefix("scryfall_updated_at="))
        .filter(|revision| !revision.trim().is_empty())
        .ok_or_else(|| {
            HexdekConformanceError::InvalidProtocol(
                "VERSION has no scryfall_updated_at card-data revision".into(),
            )
        })?;
    Ok(format!("scryfall:{}", revision.trim()))
}

fn normalize_scenario_events(
    scenario: &ScenarioDefinition,
    probe: &HexdekScenarioProbe,
) -> Result<Vec<CanonicalEvent>, HexdekConformanceError> {
    match scenario.id.as_str() {
        "layers.clone-copies-base-before-anthem" => {
            return normalize_clone_layer_events(probe);
        }
        "targeting.lethal-damage-state-based-action" => {
            return normalize_targeted_lethal_events(probe);
        }
        "triggers.creature-etb-gains-life" => {
            return normalize_creature_etb_life_trigger_events(probe);
        }
        "combat.unblocked-creature-damages-player" => {
            return normalize_unblocked_combat_damage_events(probe);
        }
        "multiplayer.combat-eliminates-one-opponent" => {
            return normalize_multiplayer_combat_elimination_events(probe);
        }
        "arena.oracle-of-the-alpha-conjures-power-nine" => {
            return normalize_oracle_of_the_alpha_events(probe);
        }
        "arena.rebalanced-falcon-abomination-has-flash" => {
            return normalize_rebalanced_falcon_abomination_events(probe);
        }
        _ => {}
    }
    let shape = parallel_lives_scenario_shape(&scenario.id).ok_or_else(|| {
        HexdekConformanceError::InvalidProtocol(format!(
            "no HexDek normalizer for scenario '{}'",
            scenario.id
        ))
    })?;
    #[derive(Default)]
    struct Episode {
        replacement_count: usize,
        tokens: usize,
        mythos_to_graveyard: bool,
    }
    let mut episodes: Vec<Episode> = Vec::new();
    for event in &probe.events {
        if event.kind == "cast" && event.source == "Mythos of Illuna" {
            episodes.push(Episode::default());
        } else if let Some(episode) = episodes.last_mut() {
            if event.kind == "replacement_applied" && event.source == "Parallel Lives" {
                episode.replacement_count =
                    episode.replacement_count.max(event.amount.max(0) as usize);
            } else if event.kind == "create_token_copy"
                && event.details.get("copy_of").and_then(Value::as_str) == Some("Parallel Lives")
            {
                episode.tokens += usize::try_from(event.amount)
                    .ok()
                    .filter(|count| *count > 0)
                    .unwrap_or(1);
            } else if event.kind == "zone_change"
                && event.source == "Mythos of Illuna"
                && event.details.get("from_zone").and_then(Value::as_str) == Some("stack")
                && event.details.get("to_zone").and_then(Value::as_str) == Some("graveyard")
            {
                episode.mythos_to_graveyard = true;
            }
        }
    }
    if episodes.len() != shape.cast_sources.len() {
        return Err(HexdekConformanceError::InvalidProtocol(format!(
            "scenario emitted {} Mythos cast episodes; expected {}",
            episodes.len(),
            shape.cast_sources.len()
        )));
    }

    let mut events = Vec::new();
    let mut push = |kind: &str,
                    actor: Option<u8>,
                    source: Option<&str>,
                    targets: Vec<String>,
                    payload: Value| {
        events.push(CanonicalEvent {
            sequence: events.len() as u64 + 1,
            kind: kind.into(),
            actor,
            source: source.map(str::to_owned),
            targets,
            payload,
        });
    };
    for (episode, source_alias) in episodes.iter().zip(shape.cast_sources) {
        push(
            "spell.cast",
            Some(1),
            Some(source_alias),
            vec![shape.target_alias.into()],
            serde_json::json!({}),
        );
        if episode.replacement_count > 1 {
            push(
                "replacement.applied",
                Some(1),
                Some(shape.target_alias),
                Vec::new(),
                serde_json::json!({
                    "affected_event": "token.created",
                    "multiplier": episode.replacement_count,
                    "source_card": "Parallel Lives"
                }),
            );
        }
        if episode.tokens > 0 {
            push(
                "token.created",
                Some(1),
                Some(source_alias),
                Vec::new(),
                serde_json::json!({
                    "card_name": "Parallel Lives",
                    "controller": 1,
                    "quantity": episode.tokens
                }),
            );
        }
        if episode.mythos_to_graveyard {
            push(
                "zone.changed",
                Some(1),
                Some(source_alias),
                Vec::new(),
                serde_json::json!({
                    "card_name": "Mythos of Illuna",
                    "from": "stack",
                    "to": "graveyard"
                }),
            );
        }
    }
    if events.is_empty() {
        return Err(HexdekConformanceError::InvalidProtocol(
            "scenario emitted no normalizable events".into(),
        ));
    }
    Ok(events)
}

fn normalize_clone_layer_events(
    probe: &HexdekScenarioProbe,
) -> Result<Vec<CanonicalEvent>, HexdekConformanceError> {
    let cast = probe
        .events
        .iter()
        .any(|event| event.kind == "cast" && event.source == "Clone");
    let copied = probe.events.iter().any(|event| {
        event.kind == "per_card_handler"
            && event.details.get("copied").and_then(Value::as_str) == Some("Grizzly Bears")
    });
    let entered = probe
        .events
        .iter()
        .any(|event| event.kind == "enter_battlefield" && event.source == "Clone");
    if !cast || !copied || !entered {
        return Err(HexdekConformanceError::InvalidProtocol(
            "Clone scenario did not emit cast, production copy handler, and battlefield entry"
                .into(),
        ));
    }
    let clone = probe.final_state.pointer("/objects/clone").ok_or_else(|| {
        HexdekConformanceError::InvalidProtocol("Clone final state is missing".into())
    })?;
    let number = |field: &str| {
        clone.get(field).and_then(Value::as_i64).ok_or_else(|| {
            HexdekConformanceError::InvalidProtocol(format!("Clone final state has no {field}"))
        })
    };
    let base_power = number("base_power")?;
    let base_toughness = number("base_toughness")?;
    let effective_power = number("effective_power")?;
    let effective_toughness = number("effective_toughness")?;
    Ok(vec![
        canonical_event(
            1,
            "spell.cast",
            Some(1),
            Some("clone"),
            Vec::new(),
            serde_json::json!({}),
        ),
        canonical_event(
            2,
            "copy.applied",
            Some(1),
            Some("clone"),
            vec!["bears".into()],
            serde_json::json!({"copied_card":"Grizzly Bears","new_object":"clone"}),
        ),
        canonical_event(
            3,
            "characteristics.evaluated",
            Some(1),
            Some("clone"),
            Vec::new(),
            serde_json::json!({
                "base_power":base_power,
                "base_toughness":base_toughness,
                "effective_power":effective_power,
                "effective_toughness":effective_toughness
            }),
        ),
        canonical_event(
            4,
            "zone.changed",
            Some(1),
            Some("clone"),
            Vec::new(),
            serde_json::json!({"card_name":"Grizzly Bears","from":"stack","to":"battlefield"}),
        ),
    ])
}

fn normalize_targeted_lethal_events(
    probe: &HexdekScenarioProbe,
) -> Result<Vec<CanonicalEvent>, HexdekConformanceError> {
    let cast = probe
        .events
        .iter()
        .any(|event| event.kind == "cast" && event.source == "Strangle");
    let damage = probe.events.iter().find(|event| {
        event.kind == "damage"
            && event.source == "Strangle"
            && event.details.get("target_card").and_then(Value::as_str) == Some("Grizzly Bears")
    });
    let spell_to_graveyard = probe.events.iter().any(|event| {
        event.kind == "zone_change"
            && event.source == "Strangle"
            && event.details.get("from_zone").and_then(Value::as_str) == Some("stack")
            && event.details.get("to_zone").and_then(Value::as_str) == Some("graveyard")
    });
    let sba = probe.events.iter().any(|event| {
        event.kind == "sba_704_5g"
            && event.source == "Grizzly Bears"
            && event.details.get("reason").and_then(Value::as_str) == Some("lethal_damage")
    });
    let bears_to_graveyard = probe.events.iter().any(|event| {
        event.kind == "zone_change"
            && event.source == "Grizzly Bears"
            && event.details.get("from_zone").and_then(Value::as_str) == Some("battlefield")
            && event.details.get("to_zone").and_then(Value::as_str) == Some("graveyard")
    });
    if !cast || damage.is_none() || !spell_to_graveyard || !sba || !bears_to_graveyard {
        return Err(HexdekConformanceError::InvalidProtocol(
            "targeted lethal-damage scenario omitted required production events".into(),
        ));
    }
    Ok(targeted_lethal_canonical_events(
        damage.map(|event| event.amount).unwrap_or_default(),
    ))
}

fn normalize_creature_etb_life_trigger_events(
    probe: &HexdekScenarioProbe,
) -> Result<Vec<CanonicalEvent>, HexdekConformanceError> {
    let cast = probe
        .events
        .iter()
        .any(|event| event.kind == "cast" && event.source == "Grizzly Bears");
    let entered = probe
        .events
        .iter()
        .any(|event| event.kind == "enter_battlefield" && event.source == "Grizzly Bears");
    let trigger = probe.events.iter().any(|event| {
        event.kind == "trigger_fires"
            && event.source == "Ajani's Welcome"
            && event.details.get("event").and_then(Value::as_str) == Some("observer_etb")
            && event.details.get("entering").and_then(Value::as_str) == Some("Grizzly Bears")
    });
    let life_event = probe
        .events
        .iter()
        .find(|event| event.kind == "gain_life" && event.source == "Ajani's Welcome");
    let life = probe
        .final_state
        .pointer("/players/seat-1/life")
        .and_then(Value::as_i64);
    if !cast || !entered || !trigger || life_event.is_none() || life.is_none() {
        return Err(HexdekConformanceError::InvalidProtocol(
            "creature-ETB trigger scenario omitted cast, entry, trigger, or life-change evidence"
                .into(),
        ));
    }
    Ok(creature_etb_life_trigger_canonical_events(
        life_event.map(|event| event.amount).unwrap_or_default(),
        life.unwrap_or_default(),
    ))
}

fn normalize_unblocked_combat_damage_events(
    probe: &HexdekScenarioProbe,
) -> Result<Vec<CanonicalEvent>, HexdekConformanceError> {
    let declared = probe.events.iter().any(|event| {
        event.kind == "declare_attackers"
            && event.seat == 0
            && event
                .details
                .get("attackers")
                .and_then(Value::as_array)
                .is_some_and(|attackers| {
                    attackers.iter().any(|attacker| {
                        attacker.get("attacker").and_then(Value::as_str) == Some("Grizzly Bears")
                            && attacker.get("defender_seat").and_then(Value::as_i64) == Some(1)
                    })
                })
    });
    let damage = probe.events.iter().find(|event| {
        event.kind == "damage"
            && event.seat == 0
            && event.target == 1
            && event.source == "Grizzly Bears"
            && event.details.get("combat").and_then(Value::as_bool) == Some(true)
            && event.details.get("target_kind").and_then(Value::as_str) == Some("player")
    });
    let tapped = probe
        .final_state
        .pointer("/objects/bears/tapped")
        .and_then(Value::as_bool)
        == Some(true);
    let life = probe
        .final_state
        .pointer("/players/seat-2/life")
        .and_then(Value::as_i64);
    if !declared || damage.is_none() || !tapped || life.is_none() {
        return Err(HexdekConformanceError::InvalidProtocol(
            "combat scenario omitted declaration, tap, combat-damage, or final-life evidence"
                .into(),
        ));
    }
    let amount = damage.map(|event| event.amount).unwrap_or_default();
    let life = life.unwrap_or_default();
    if amount <= 0 || life != 20 - amount {
        return Err(HexdekConformanceError::InvalidProtocol(
            "combat damage evidence disagrees with the final life total".into(),
        ));
    }
    Ok(unblocked_combat_damage_canonical_events(
        amount, -amount, life,
    ))
}

fn unblocked_combat_damage_canonical_events(
    amount: i64,
    delta: i64,
    life: i64,
) -> Vec<CanonicalEvent> {
    vec![
        canonical_event(
            1,
            "attacker.declared",
            Some(1),
            Some("bears"),
            vec!["seat-2".into()],
            serde_json::json!({"defender":2}),
        ),
        canonical_event(
            2,
            "combat.damage",
            Some(1),
            Some("bears"),
            vec!["seat-2".into()],
            serde_json::json!({"amount":amount,"combat":true,"target":"seat-2"}),
        ),
        canonical_event(
            3,
            "life.changed",
            Some(2),
            Some("bears"),
            Vec::new(),
            serde_json::json!({"delta":delta,"life":life}),
        ),
    ]
}

fn normalize_multiplayer_combat_elimination_events(
    probe: &HexdekScenarioProbe,
) -> Result<Vec<CanonicalEvent>, HexdekConformanceError> {
    let declared = probe.events.iter().any(|event| {
        event.kind == "declare_attackers"
            && event.seat == 0
            && event
                .details
                .get("attackers")
                .and_then(Value::as_array)
                .is_some_and(|attackers| {
                    attackers.iter().any(|attacker| {
                        attacker.get("attacker").and_then(Value::as_str) == Some("Grizzly Bears")
                            && attacker.get("defender_seat").and_then(Value::as_i64) == Some(2)
                    })
                })
    });
    let damage = probe.events.iter().any(|event| {
        event.kind == "damage"
            && event.seat == 0
            && event.target == 2
            && event.source == "Grizzly Bears"
            && event.amount == 2
            && event.details.get("combat").and_then(Value::as_bool) == Some(true)
            && event.details.get("target_kind").and_then(Value::as_str) == Some("player")
    });
    let loss_sba = probe.events.iter().any(|event| {
        event.kind == "sba_704_5a"
            && event.seat == 2
            && event.details.get("reason").and_then(Value::as_str)
                == Some("life_total_zero_or_less")
    });
    let eliminated = probe.events.iter().any(|event| {
        event.kind == "seat_eliminated"
            && event.seat == 2
            && event.amount == 1
            && event.details.get("loss_category").and_then(Value::as_str)
                == Some("life_zero_or_less")
            && event
                .details
                .get("permanents_removed")
                .and_then(Value::as_i64)
                == Some(1)
    });
    let boolean = |path: &str| probe.final_state.pointer(path).and_then(Value::as_bool);
    let number = |path: &str| probe.final_state.pointer(path).and_then(Value::as_i64);
    let final_state_matches = number("/players/seat-2/life") == Some(20)
        && boolean("/players/seat-2/lost") == Some(false)
        && number("/players/seat-3/life") == Some(0)
        && boolean("/players/seat-3/lost") == Some(true)
        && boolean("/players/seat-3/left_game") == Some(true)
        && number("/battlefield/seat-3/cards/Forest/count") == Some(0)
        && boolean("/objects/bears/tapped") == Some(true)
        && boolean("/objects/bears/declared_attacker") == Some(true)
        && number("/objects/bears/defender") == Some(3);
    if !declared || !damage || !loss_sba || !eliminated || !final_state_matches {
        return Err(HexdekConformanceError::InvalidProtocol(
            "multiplayer combat scenario omitted defender choice, lethal damage, survivor, or elimination cleanup evidence".into(),
        ));
    }
    Ok(multiplayer_combat_elimination_canonical_events())
}

fn multiplayer_combat_elimination_canonical_events() -> Vec<CanonicalEvent> {
    vec![
        canonical_event(
            1,
            "attacker.declared",
            Some(1),
            Some("bears"),
            vec!["seat-3".into()],
            serde_json::json!({"defender":3}),
        ),
        canonical_event(
            2,
            "combat.damage",
            Some(1),
            Some("bears"),
            vec!["seat-3".into()],
            serde_json::json!({"amount":2,"combat":true,"target":"seat-3"}),
        ),
        canonical_event(
            3,
            "life.changed",
            Some(3),
            Some("bears"),
            Vec::new(),
            serde_json::json!({"delta":-2,"life":0}),
        ),
        canonical_event(
            4,
            "player.eliminated",
            Some(3),
            Some("bears"),
            Vec::new(),
            serde_json::json!({"permanents_removed":1,"reason":"life_total_zero_or_less","seat":3}),
        ),
    ]
}

const POWER_NINE: [&str; 9] = [
    "Ancestral Recall",
    "Black Lotus",
    "Mox Emerald",
    "Mox Jet",
    "Mox Pearl",
    "Mox Ruby",
    "Mox Sapphire",
    "Time Walk",
    "Timetwister",
];

fn normalize_oracle_of_the_alpha_events(
    probe: &HexdekScenarioProbe,
) -> Result<Vec<CanonicalEvent>, HexdekConformanceError> {
    let cast = probe
        .events
        .iter()
        .any(|event| event.kind == "cast" && event.source == "Oracle of the Alpha");
    let entered = probe
        .events
        .iter()
        .any(|event| event.kind == "enter_battlefield" && event.source == "Oracle of the Alpha");
    let triggered = probe
        .events
        .iter()
        .any(|event| event.kind == "triggered_ability" && event.source == "Oracle of the Alpha");
    let residual = probe.events.iter().any(|event| {
        event.kind == "parsed_effect_residual"
            && event.source == "Oracle of the Alpha"
            && event.details.get("raw").and_then(Value::as_str)
                == Some("conjure the power nine into your library, then shuffle")
    });
    let final_state_matches = probe
        .final_state
        .pointer("/battlefield/seat-1/cards/Oracle of the Alpha/count")
        .and_then(Value::as_i64)
        == Some(1)
        && probe
            .final_state
            .pointer("/library/seat-1/total_count")
            .and_then(Value::as_i64)
            == Some(0)
        && POWER_NINE.iter().all(|name| {
            probe
                .final_state
                .pointer(&format!("/library/seat-1/cards/{name}/count"))
                .and_then(Value::as_i64)
                == Some(0)
        });
    if !cast || !entered || !triggered || !residual || !final_state_matches {
        return Err(HexdekConformanceError::InvalidProtocol(
            "Oracle of the Alpha scenario omitted cast, trigger, residual-effect, or empty-library evidence".into(),
        ));
    }
    Ok(oracle_of_the_alpha_canonical_events(false))
}

fn oracle_of_the_alpha_canonical_events(supported: bool) -> Vec<CanonicalEvent> {
    let mut events = vec![
        canonical_event(
            1,
            "spell.cast",
            Some(1),
            Some("oracle"),
            Vec::new(),
            serde_json::json!({}),
        ),
        canonical_event(
            2,
            "zone.changed",
            Some(1),
            Some("oracle"),
            Vec::new(),
            serde_json::json!({"card_name":"Oracle of the Alpha","from":"stack","to":"battlefield"}),
        ),
        canonical_event(
            3,
            "trigger.fired",
            Some(1),
            Some("oracle"),
            Vec::new(),
            serde_json::json!({"effect":"conjure_power_nine","event":"creature_entered"}),
        ),
    ];
    events.push(if supported {
        canonical_event(
            4,
            "card.conjured",
            Some(1),
            Some("oracle"),
            Vec::new(),
            serde_json::json!({"card_names":POWER_NINE,"quantity":9,"zone":"library"}),
        )
    } else {
        canonical_event(
            4,
            "effect.unsupported",
            Some(1),
            Some("oracle"),
            Vec::new(),
            serde_json::json!({"mechanic":"conjure","requested":"Power Nine","zone":"library"}),
        )
    });
    events
}

fn normalize_rebalanced_falcon_abomination_events(
    probe: &HexdekScenarioProbe,
) -> Result<Vec<CanonicalEvent>, HexdekConformanceError> {
    let cast = probe
        .events
        .iter()
        .any(|event| event.kind == "cast" && event.source == "A-Falcon Abomination");
    let entered = probe
        .events
        .iter()
        .any(|event| event.kind == "enter_battlefield" && event.source == "A-Falcon Abomination");
    let original_legal = probe
        .final_state
        .pointer("/timing/original_legal")
        .and_then(Value::as_bool);
    let rebalanced_legal = probe
        .final_state
        .pointer("/timing/rebalanced_legal")
        .and_then(Value::as_bool);
    let battlefield_count = probe
        .final_state
        .pointer("/battlefield/seat-1/cards/A-Falcon Abomination/count")
        .and_then(Value::as_i64);
    if !cast
        || !entered
        || original_legal.is_none()
        || rebalanced_legal.is_none()
        || battlefield_count != Some(1)
    {
        return Err(HexdekConformanceError::InvalidProtocol(
            "rebalanced Falcon Abomination scenario omitted timing, cast, entry, or final-state evidence".into(),
        ));
    }
    Ok(rebalanced_falcon_abomination_canonical_events(
        original_legal.unwrap_or_default(),
        rebalanced_legal.unwrap_or_default(),
    ))
}

fn rebalanced_falcon_abomination_canonical_events(
    original_legal: bool,
    rebalanced_legal: bool,
) -> Vec<CanonicalEvent> {
    vec![
        canonical_event(
            1,
            "timing.checked",
            Some(1),
            Some("original"),
            Vec::new(),
            serde_json::json!({"active_player":2,"card_name":"Falcon Abomination","caster":1,"legal":original_legal}),
        ),
        canonical_event(
            2,
            "timing.checked",
            Some(1),
            Some("rebalanced"),
            Vec::new(),
            serde_json::json!({"active_player":2,"card_name":"A-Falcon Abomination","caster":1,"legal":rebalanced_legal}),
        ),
        canonical_event(
            3,
            "spell.cast",
            Some(1),
            Some("rebalanced"),
            Vec::new(),
            serde_json::json!({}),
        ),
        canonical_event(
            4,
            "zone.changed",
            Some(1),
            Some("rebalanced"),
            Vec::new(),
            serde_json::json!({"card_name":"A-Falcon Abomination","from":"stack","to":"battlefield"}),
        ),
    ]
}

fn creature_etb_life_trigger_canonical_events(delta: i64, life: i64) -> Vec<CanonicalEvent> {
    vec![
        canonical_event(
            1,
            "spell.cast",
            Some(1),
            Some("bears"),
            Vec::new(),
            serde_json::json!({}),
        ),
        canonical_event(
            2,
            "zone.changed",
            Some(1),
            Some("bears"),
            Vec::new(),
            serde_json::json!({"card_name":"Grizzly Bears","from":"stack","to":"battlefield"}),
        ),
        canonical_event(
            3,
            "trigger.fired",
            Some(1),
            Some("welcome"),
            Vec::new(),
            serde_json::json!({"event":"creature_entered","entering":"bears"}),
        ),
        canonical_event(
            4,
            "life.changed",
            Some(1),
            Some("welcome"),
            Vec::new(),
            serde_json::json!({"delta":delta,"life":life}),
        ),
    ]
}

fn canonical_event(
    sequence: u64,
    kind: &str,
    actor: Option<u8>,
    source: Option<&str>,
    targets: Vec<String>,
    payload: Value,
) -> CanonicalEvent {
    CanonicalEvent {
        sequence,
        kind: kind.into(),
        actor,
        source: source.map(str::to_owned),
        targets,
        payload,
    }
}

fn targeted_lethal_canonical_events(amount: i64) -> Vec<CanonicalEvent> {
    vec![
        canonical_event(
            1,
            "spell.cast",
            Some(1),
            Some("strangle"),
            vec!["bears".into()],
            serde_json::json!({}),
        ),
        canonical_event(
            2,
            "damage.dealt",
            Some(1),
            Some("strangle"),
            vec!["bears".into()],
            serde_json::json!({"amount":amount,"target":"bears"}),
        ),
        canonical_event(
            3,
            "zone.changed",
            Some(1),
            Some("strangle"),
            Vec::new(),
            serde_json::json!({"card_name":"Strangle","from":"stack","to":"graveyard"}),
        ),
        canonical_event(
            4,
            "state_based_action",
            None,
            Some("bears"),
            Vec::new(),
            serde_json::json!({"action":"destroy","card_name":"Grizzly Bears","reason":"lethal_damage"}),
        ),
        canonical_event(
            5,
            "zone.changed",
            Some(2),
            Some("bears"),
            Vec::new(),
            serde_json::json!({"card_name":"Grizzly Bears","from":"battlefield","to":"graveyard"}),
        ),
    ]
}

struct ParallelLivesScenarioShape {
    cast_sources: &'static [&'static str],
    target_alias: &'static str,
}

fn parallel_lives_scenario_shape(id: &str) -> Option<ParallelLivesScenarioShape> {
    match id {
        "tokens.parallel-lives-copy" => Some(ParallelLivesScenarioShape {
            cast_sources: &["mythos"],
            target_alias: "parallel-lives",
        }),
        "tokens.stacked-parallel-lives-replacements" => Some(ParallelLivesScenarioShape {
            cast_sources: &["mythos"],
            target_alias: "parallel-lives-a",
        }),
        "tokens.copied-doubler-retains-ability" => Some(ParallelLivesScenarioShape {
            cast_sources: &["mythos-a", "mythos-b"],
            target_alias: "parallel-lives",
        }),
        _ => None,
    }
}

fn parse_probe_output(stdout: &str) -> Result<HexdekScenarioProbe, HexdekConformanceError> {
    let mut records = stdout
        .lines()
        .filter_map(|line| line.strip_prefix(RESULT_PREFIX));
    let record = records.next().ok_or_else(|| {
        HexdekConformanceError::InvalidProtocol("missing scenario result record".into())
    })?;
    if records.next().is_some() {
        return Err(HexdekConformanceError::InvalidProtocol(
            "multiple scenario result records".into(),
        ));
    }
    Ok(serde_json::from_str(record)?)
}

fn engine_revision(data_dir: &Path) -> Result<String, HexdekConformanceError> {
    let paths = paths(data_dir);
    let version = fs::read_to_string(&paths.version)?;
    let revision = version
        .lines()
        .find_map(|line| line.strip_prefix("revision="))
        .filter(|revision| !revision.trim().is_empty())
        .ok_or_else(|| HexdekConformanceError::InvalidProtocol("VERSION has no revision".into()))?;
    let mut hasher = Sha256::new();
    hasher.update(fs::read(&paths.executable)?);
    Ok(format!(
        "{}+adapter:{:x}",
        revision.trim(),
        hasher.finalize()
    ))
}

fn paths(data_dir: &Path) -> HexdekPaths {
    let root = data_dir.join("engines").join("hexdek");
    HexdekPaths {
        executable: root.join(if cfg!(windows) {
            "hexdek-conformance.exe"
        } else {
            "hexdek-conformance"
        }),
        ast: root.join("ast_dataset.jsonl"),
        version: root.join("VERSION"),
        root,
    }
}

fn trim_process_error(stderr: &str, stdout: &str) -> String {
    let message = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    message.chars().take(2_000).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_normalizes_parallel_lives_scenario() {
        let output = concat!(
            "noise\n",
            "METAGENE_HEXDEK_SCENARIO_RESULT {\"protocol_version\":1,\"scenario_id\":\"tokens.parallel-lives-copy\",\"scenario_version\":1,\"seed\":42,\"events\":[",
            "{\"Kind\":\"cast\",\"Seat\":0,\"Target\":0,\"Source\":\"Mythos of Illuna\",\"Amount\":4,\"Details\":{}},",
            "{\"Kind\":\"replacement_applied\",\"Seat\":0,\"Target\":0,\"Source\":\"Parallel Lives\",\"Amount\":2,\"Details\":{}},",
            "{\"Kind\":\"create_token_copy\",\"Seat\":0,\"Target\":0,\"Source\":\"Mythos of Illuna\",\"Amount\":2,\"Details\":{\"copy_of\":\"Parallel Lives\"}},",
            "{\"Kind\":\"zone_change\",\"Seat\":0,\"Target\":-1,\"Source\":\"Mythos of Illuna\",\"Amount\":0,\"Details\":{\"from_zone\":\"stack\",\"to_zone\":\"graveyard\"}}",
            "],\"final_state\":{\"battlefield\":{\"seat-1\":{\"cards\":{\"Parallel Lives\":{\"count\":3}},\"tokens\":{\"Parallel Lives\":{\"count\":2}}}},\"graveyard\":{\"seat-1\":{\"cards\":{\"Mythos of Illuna\":{\"count\":1}}}}}}\n"
        );
        let probe = parse_probe_output(output).unwrap();
        let suite = crate::conformance_contract::parse_scenario_suite(include_str!(
            "../../data/conformance/scenarios.schema1.json"
        ))
        .unwrap();
        let normalized = normalize_scenario_events(&suite.scenarios[0], &probe).unwrap();
        assert_eq!(normalized.len(), 4);
        assert_eq!(normalized[1].kind, "replacement.applied");
        assert_eq!(normalized[2].payload["quantity"], 2);
        let projected = project_events(&normalized, &suite.scenarios[0].event_projection).unwrap();
        assert_eq!(
            canonical_event_hash(&projected).unwrap(),
            "sha256:6075f701b27a244953d776b916e27592da13608e0bb6901419c0141e1c954d77"
        );
    }

    #[test]
    fn copied_doubler_uses_registered_production_card_handlers() {
        let output = concat!(
            "METAGENE_HEXDEK_SCENARIO_RESULT {\"protocol_version\":1,\"scenario_id\":\"tokens.copied-doubler-retains-ability\",\"scenario_version\":1,\"seed\":44,\"events\":[",
            "{\"Kind\":\"cast\",\"Seat\":0,\"Target\":0,\"Source\":\"Mythos of Illuna\",\"Amount\":4,\"Details\":{}},",
            "{\"Kind\":\"replacement_applied\",\"Seat\":0,\"Target\":0,\"Source\":\"Parallel Lives\",\"Amount\":2,\"Details\":{}},",
            "{\"Kind\":\"create_token_copy\",\"Seat\":0,\"Target\":0,\"Source\":\"Mythos of Illuna\",\"Amount\":2,\"Details\":{\"copy_of\":\"Parallel Lives\"}},",
            "{\"Kind\":\"zone_change\",\"Seat\":0,\"Target\":-1,\"Source\":\"Mythos of Illuna\",\"Amount\":0,\"Details\":{\"from_zone\":\"stack\",\"to_zone\":\"graveyard\"}},",
            "{\"Kind\":\"cast\",\"Seat\":0,\"Target\":0,\"Source\":\"Mythos of Illuna\",\"Amount\":4,\"Details\":{}},",
            "{\"Kind\":\"replacement_applied\",\"Seat\":0,\"Target\":0,\"Source\":\"Parallel Lives\",\"Amount\":8,\"Details\":{}},",
            "{\"Kind\":\"create_token_copy\",\"Seat\":0,\"Target\":0,\"Source\":\"Mythos of Illuna\",\"Amount\":8,\"Details\":{\"copy_of\":\"Parallel Lives\"}},",
            "{\"Kind\":\"zone_change\",\"Seat\":0,\"Target\":-1,\"Source\":\"Mythos of Illuna\",\"Amount\":0,\"Details\":{\"from_zone\":\"stack\",\"to_zone\":\"graveyard\"}}",
            "],\"final_state\":{\"battlefield\":{\"seat-1\":{\"cards\":{\"Parallel Lives\":{\"count\":11}},\"tokens\":{\"Parallel Lives\":{\"count\":10}}}},\"graveyard\":{\"seat-1\":{\"cards\":{\"Mythos of Illuna\":{\"count\":2}}}}}}\n"
        );
        let probe = parse_probe_output(output).unwrap();
        let suite = crate::conformance_contract::parse_scenario_suite(include_str!(
            "../../data/conformance/scenarios.schema1.json"
        ))
        .unwrap();
        let scenario = &suite.scenarios[2];
        let report = evaluate_assertions(&scenario.assertions, &probe.final_state).unwrap();
        assert!(report.passed);
        let normalized = normalize_scenario_events(scenario, &probe).unwrap();
        let token_quantities = normalized
            .iter()
            .filter(|event| event.kind == "token.created")
            .map(|event| event.payload["quantity"].as_u64().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(token_quantities, vec![2, 8]);
        let projected = project_events(&normalized, &scenario.event_projection).unwrap();
        assert_eq!(
            canonical_event_hash(&projected).unwrap(),
            "sha256:a316d0fd907d9754e06965079604ee5be4a7f1ff8aedfe6269bfa0502cbba1bf"
        );
    }

    #[test]
    fn clone_layer_failure_is_preserved_in_normalized_evidence() {
        let probe = HexdekScenarioProbe {
            protocol_version: 1,
            scenario_id: "layers.clone-copies-base-before-anthem".into(),
            scenario_version: 1,
            seed: 45,
            events: vec![
                HexdekRecordedEvent {
                    kind: "cast".into(),
                    seat: 0,
                    target: -1,
                    source: "Clone".into(),
                    amount: 0,
                    details: serde_json::json!({}),
                },
                HexdekRecordedEvent {
                    kind: "enter_battlefield".into(),
                    seat: 0,
                    target: -1,
                    source: "Clone".into(),
                    amount: 0,
                    details: serde_json::json!({}),
                },
                HexdekRecordedEvent {
                    kind: "per_card_handler".into(),
                    seat: 0,
                    target: -1,
                    source: "Clone".into(),
                    amount: 0,
                    details: serde_json::json!({"copied":"Grizzly Bears"}),
                },
            ],
            final_state: serde_json::json!({"objects":{"clone":{"card_name":"Grizzly Bears","base_power":2,"base_toughness":2,"effective_power":2,"effective_toughness":2}}}),
            error: None,
        };
        let suite = crate::conformance_contract::parse_scenario_suite(include_str!(
            "../../data/conformance/scenarios.schema1.json"
        ))
        .unwrap();
        let scenario = &suite.scenarios[3];
        assert!(
            !evaluate_assertions(&scenario.assertions, &probe.final_state)
                .unwrap()
                .passed
        );
        let projected = project_events(
            &normalize_scenario_events(scenario, &probe).unwrap(),
            &scenario.event_projection,
        )
        .unwrap();
        assert_eq!(
            canonical_event_hash(&projected).unwrap(),
            "sha256:d620ea005f2d5feafd73b5ce7690240e7b067a8cfcc34701e88a88bec9fcd49b"
        );
    }

    #[test]
    fn targeted_lethal_damage_normalizes_to_shared_hash() {
        let probe = HexdekScenarioProbe {
            protocol_version: 1,
            scenario_id: "targeting.lethal-damage-state-based-action".into(),
            scenario_version: 1,
            seed: 46,
            events: vec![
                HexdekRecordedEvent {
                    kind: "cast".into(),
                    seat: 0,
                    target: 1,
                    source: "Strangle".into(),
                    amount: 0,
                    details: serde_json::json!({}),
                },
                HexdekRecordedEvent {
                    kind: "damage".into(),
                    seat: 0,
                    target: 1,
                    source: "Strangle".into(),
                    amount: 3,
                    details: serde_json::json!({"target_card":"Grizzly Bears"}),
                },
                HexdekRecordedEvent {
                    kind: "zone_change".into(),
                    seat: 0,
                    target: -1,
                    source: "Strangle".into(),
                    amount: 0,
                    details: serde_json::json!({"from_zone":"stack","to_zone":"graveyard"}),
                },
                HexdekRecordedEvent {
                    kind: "sba_704_5g".into(),
                    seat: 1,
                    target: -1,
                    source: "Grizzly Bears".into(),
                    amount: 0,
                    details: serde_json::json!({"reason":"lethal_damage"}),
                },
                HexdekRecordedEvent {
                    kind: "zone_change".into(),
                    seat: 1,
                    target: -1,
                    source: "Grizzly Bears".into(),
                    amount: 0,
                    details: serde_json::json!({"from_zone":"battlefield","to_zone":"graveyard"}),
                },
            ],
            final_state: serde_json::json!({"battlefield":{"seat-2":{"cards":{"Grizzly Bears":{"count":0}}}},"graveyard":{"seat-2":{"cards":{"Grizzly Bears":{"count":1}}},"seat-1":{"cards":{"Strangle":{"count":1}}}},"players":{"seat-2":{"life":20}}}),
            error: None,
        };
        let suite = crate::conformance_contract::parse_scenario_suite(include_str!(
            "../../data/conformance/scenarios.schema1.json"
        ))
        .unwrap();
        let scenario = &suite.scenarios[4];
        assert!(
            evaluate_assertions(&scenario.assertions, &probe.final_state)
                .unwrap()
                .passed
        );
        let projected = project_events(
            &normalize_scenario_events(scenario, &probe).unwrap(),
            &scenario.event_projection,
        )
        .unwrap();
        assert_eq!(
            canonical_event_hash(&projected).unwrap(),
            "sha256:2e3adda780ff74ddb024056faab9da9c94f0d09270dc89b317d97c1eb0c1e8db"
        );
    }

    #[test]
    fn creature_etb_trigger_normalizes_to_shared_hash() {
        let probe = HexdekScenarioProbe {
            protocol_version: 1,
            scenario_id: "triggers.creature-etb-gains-life".into(),
            scenario_version: 1,
            seed: 47,
            events: vec![
                HexdekRecordedEvent {
                    kind: "cast".into(),
                    seat: 0,
                    target: 0,
                    source: "Grizzly Bears".into(),
                    amount: 2,
                    details: serde_json::json!({}),
                },
                HexdekRecordedEvent {
                    kind: "enter_battlefield".into(),
                    seat: 0,
                    target: 0,
                    source: "Grizzly Bears".into(),
                    amount: 0,
                    details: serde_json::json!({}),
                },
                HexdekRecordedEvent {
                    kind: "trigger_fires".into(),
                    seat: 0,
                    target: 0,
                    source: "Ajani's Welcome".into(),
                    amount: 0,
                    details: serde_json::json!({"event":"observer_etb","entering":"Grizzly Bears"}),
                },
                HexdekRecordedEvent {
                    kind: "gain_life".into(),
                    seat: 0,
                    target: 0,
                    source: "Ajani's Welcome".into(),
                    amount: 1,
                    details: serde_json::json!(null),
                },
            ],
            final_state: serde_json::json!({"players":{"seat-1":{"life":21}},"battlefield":{"seat-1":{"cards":{"Ajani's Welcome":{"count":1},"Grizzly Bears":{"count":1}}}}}),
            error: None,
        };
        let suite = crate::conformance_contract::parse_scenario_suite(include_str!(
            "../../data/conformance/scenarios.schema1.json"
        ))
        .unwrap();
        let scenario = &suite.scenarios[5];
        assert!(
            evaluate_assertions(&scenario.assertions, &probe.final_state)
                .unwrap()
                .passed
        );
        let projected = project_events(
            &normalize_scenario_events(scenario, &probe).unwrap(),
            &scenario.event_projection,
        )
        .unwrap();
        assert_eq!(
            canonical_event_hash(&projected).unwrap(),
            "sha256:2c165f28ecc0ee224ff085c564ad7a607d80d294b5dbf7ba869bc2178a2d80cf"
        );
    }

    #[test]
    fn unblocked_combat_damage_normalizes_to_shared_hash() {
        let probe = HexdekScenarioProbe {
            protocol_version: 1,
            scenario_id: "combat.unblocked-creature-damages-player".into(),
            scenario_version: 1,
            seed: 48,
            events: vec![
                HexdekRecordedEvent {
                    kind: "declare_attackers".into(),
                    seat: 0,
                    target: 0,
                    source: String::new(),
                    amount: 0,
                    details: serde_json::json!({"attackers":[{"attacker":"Grizzly Bears","defender_seat":1}]}),
                },
                HexdekRecordedEvent {
                    kind: "damage".into(),
                    seat: 0,
                    target: 1,
                    source: "Grizzly Bears".into(),
                    amount: 2,
                    details: serde_json::json!({"combat":true,"target_kind":"player"}),
                },
            ],
            final_state: serde_json::json!({"players":{"seat-2":{"life":18}},"battlefield":{"seat-1":{"cards":{"Grizzly Bears":{"count":1}}}},"objects":{"bears":{"tapped":true}}}),
            error: None,
        };
        let suite = crate::conformance_contract::parse_scenario_suite(include_str!(
            "../../data/conformance/scenarios.schema1.json"
        ))
        .unwrap();
        let scenario = &suite.scenarios[6];
        assert!(
            evaluate_assertions(&scenario.assertions, &probe.final_state)
                .unwrap()
                .passed
        );
        let projected = project_events(
            &normalize_scenario_events(scenario, &probe).unwrap(),
            &scenario.event_projection,
        )
        .unwrap();
        assert_eq!(projected.len(), 3);
        assert_eq!(projected[1].payload["amount"], 2);
        assert_eq!(projected[2].payload["life"], 18);
        assert_eq!(
            canonical_event_hash(&projected).unwrap(),
            "sha256:6d63adf00b9ba89581d73afbafcbfbd1f0e9c920fb29d4330823f021f17b5247"
        );
    }

    #[test]
    fn multiplayer_combat_elimination_normalizes_to_shared_hash() {
        let probe = HexdekScenarioProbe {
            protocol_version: 1,
            scenario_id: "multiplayer.combat-eliminates-one-opponent".into(),
            scenario_version: 1,
            seed: 49,
            events: vec![
                HexdekRecordedEvent {
                    kind: "declare_attackers".into(),
                    seat: 0,
                    target: 0,
                    source: String::new(),
                    amount: 0,
                    details: serde_json::json!({"attackers":[{"attacker":"Grizzly Bears","defender_seat":2}]}),
                },
                HexdekRecordedEvent {
                    kind: "damage".into(),
                    seat: 0,
                    target: 2,
                    source: "Grizzly Bears".into(),
                    amount: 2,
                    details: serde_json::json!({"combat":true,"target_kind":"player"}),
                },
                HexdekRecordedEvent {
                    kind: "sba_704_5a".into(),
                    seat: 2,
                    target: -1,
                    source: String::new(),
                    amount: 0,
                    details: serde_json::json!({"reason":"life_total_zero_or_less","rule":"704.5a"}),
                },
                HexdekRecordedEvent {
                    kind: "seat_eliminated".into(),
                    seat: 2,
                    target: -1,
                    source: String::new(),
                    amount: 1,
                    details: serde_json::json!({"loss_category":"life_zero_or_less","permanents_removed":1,"rule":"800.4a"}),
                },
            ],
            final_state: serde_json::json!({"players":{"seat-2":{"life":20,"lost":false},"seat-3":{"life":0,"lost":true,"left_game":true}},"battlefield":{"seat-3":{"cards":{"Forest":{"count":0}}}},"objects":{"bears":{"tapped":true,"declared_attacker":true,"defender":3}}}),
            error: None,
        };
        let suite = crate::conformance_contract::parse_scenario_suite(include_str!(
            "../../data/conformance/scenarios.schema1.json"
        ))
        .unwrap();
        let scenario = &suite.scenarios[7];
        assert!(
            evaluate_assertions(&scenario.assertions, &probe.final_state)
                .unwrap()
                .passed
        );
        let projected = project_events(
            &normalize_scenario_events(scenario, &probe).unwrap(),
            &scenario.event_projection,
        )
        .unwrap();
        assert_eq!(projected.len(), 4);
        assert_eq!(projected[0].payload["defender"], 3);
        assert_eq!(projected[3].payload["permanents_removed"], 1);
        assert_eq!(
            canonical_event_hash(&projected).unwrap(),
            "sha256:49f15486a8396361283aa780066549a2247a104988b00542a055c67899d2ad41"
        );
    }

    #[test]
    fn oracle_of_the_alpha_residual_is_preserved_as_unsupported() {
        let probe = HexdekScenarioProbe {
            protocol_version: 1,
            scenario_id: "arena.oracle-of-the-alpha-conjures-power-nine".into(),
            scenario_version: 1,
            seed: 50,
            events: vec![
                HexdekRecordedEvent {
                    kind: "cast".into(),
                    seat: 0,
                    target: 0,
                    source: "Oracle of the Alpha".into(),
                    amount: 3,
                    details: serde_json::json!({}),
                },
                HexdekRecordedEvent {
                    kind: "enter_battlefield".into(),
                    seat: 0,
                    target: 0,
                    source: "Oracle of the Alpha".into(),
                    amount: 0,
                    details: serde_json::json!({}),
                },
                HexdekRecordedEvent {
                    kind: "triggered_ability".into(),
                    seat: 0,
                    target: 0,
                    source: "Oracle of the Alpha".into(),
                    amount: 0,
                    details: serde_json::json!({}),
                },
                HexdekRecordedEvent {
                    kind: "parsed_effect_residual".into(),
                    seat: 0,
                    target: 0,
                    source: "Oracle of the Alpha".into(),
                    amount: 0,
                    details: serde_json::json!({"raw":"conjure the power nine into your library, then shuffle"}),
                },
            ],
            final_state: serde_json::json!({"battlefield":{"seat-1":{"cards":{"Oracle of the Alpha":{"count":1}}}},"library":{"seat-1":{"total_count":0,"cards":{"Ancestral Recall":{"count":0},"Black Lotus":{"count":0},"Mox Emerald":{"count":0},"Mox Jet":{"count":0},"Mox Pearl":{"count":0},"Mox Ruby":{"count":0},"Mox Sapphire":{"count":0},"Time Walk":{"count":0},"Timetwister":{"count":0}}}}}),
            error: None,
        };
        let suite = crate::conformance_contract::parse_scenario_suite(include_str!(
            "../../data/conformance/scenarios.schema1.json"
        ))
        .unwrap();
        let scenario = &suite.scenarios[8];
        assert!(
            !evaluate_assertions(&scenario.assertions, &probe.final_state)
                .unwrap()
                .passed
        );
        let projected = project_events(
            &normalize_scenario_events(scenario, &probe).unwrap(),
            &scenario.event_projection,
        )
        .unwrap();
        assert_eq!(projected[3].kind, "effect.unsupported");
        assert_eq!(projected[3].payload["mechanic"], "conjure");
        assert_eq!(
            canonical_event_hash(&projected).unwrap(),
            "sha256:397f5cd381de89fc862e3da0d61c53700caf00a4c5111ab47ceb4439fbab1f27"
        );
    }

    #[test]
    fn rebalanced_falcon_abomination_flash_normalizes_hexdek_timing() {
        let probe = HexdekScenarioProbe {
            protocol_version: 1,
            scenario_id: "arena.rebalanced-falcon-abomination-has-flash".into(),
            scenario_version: 1,
            seed: 51,
            events: vec![
                HexdekRecordedEvent {
                    kind: "cast".into(),
                    seat: 0,
                    target: 0,
                    source: "A-Falcon Abomination".into(),
                    amount: 3,
                    details: serde_json::json!({}),
                },
                HexdekRecordedEvent {
                    kind: "enter_battlefield".into(),
                    seat: 0,
                    target: 0,
                    source: "A-Falcon Abomination".into(),
                    amount: 0,
                    details: serde_json::json!({}),
                },
            ],
            final_state: serde_json::json!({"timing":{"original_legal":false,"rebalanced_legal":true},"battlefield":{"seat-1":{"cards":{"A-Falcon Abomination":{"count":1}}}}}),
            error: None,
        };
        let suite = crate::conformance_contract::parse_scenario_suite(include_str!(
            "../../data/conformance/scenarios.schema1.json"
        ))
        .unwrap();
        let scenario = &suite.scenarios[9];
        assert!(
            evaluate_assertions(&scenario.assertions, &probe.final_state)
                .unwrap()
                .passed
        );
        let projected = project_events(
            &normalize_scenario_events(scenario, &probe).unwrap(),
            &scenario.event_projection,
        )
        .unwrap();
        assert_eq!(projected.len(), 4);
        assert_eq!(projected[0].payload["legal"], false);
        assert_eq!(projected[1].payload["legal"], true);
        assert_eq!(
            canonical_event_hash(&projected).unwrap(),
            "sha256:3ab1d50217f6b174543caed2030569c4e1b55c4d021c7549fa8fec3b32ed0eb6"
        );
    }

    #[test]
    fn rejects_duplicate_result_records() {
        let line = "METAGENE_HEXDEK_SCENARIO_RESULT {}\n";
        assert!(matches!(
            parse_probe_output(&format!("{line}{line}")),
            Err(HexdekConformanceError::InvalidProtocol(_))
        ));
    }
}
