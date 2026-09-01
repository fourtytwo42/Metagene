use crate::simulator::{
    HexdekValidationRequest, InstallerProgress, MatchupRow, SimulationReport, SimulatorStatus,
};
use crate::{
    conformance::ConformanceObservation,
    conformance_contract::{
        CanonicalEvent, ContractError, ScenarioDefinition, canonical_event_hash,
        evaluate_assertions, project_events,
    },
};
use bzip2::read::BzDecoder;
use metagene_core::{CancellationToken, CardPool, Deck};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tar::Archive;
use thiserror::Error;
use uuid::Uuid;

const FORGE_VERSION: &str = "2.0.14";
const FORGE_RELEASE_REVISION: &str = "a37a865";
const FORGE_ARCHIVE_URL: &str = "https://github.com/Card-Forge/forge/releases/download/forge-2.0.14/forge-installer-2.0.14.tar.bz2";
const FORGE_ARCHIVE_BYTES: u64 = 301_832_993;
const FORGE_ARCHIVE_SHA256: &str =
    "e71749376945177d603d52a21a089bc1574330cf34599544b6bae1a04c83da41";
const MAX_CAPTURE_BYTES: u64 = 16 * 1024 * 1024;
const FORGE_ADAPTER_BYTES: &[u8] =
    include_bytes!("../resources/forge-adapter/forge-conformance-adapter.jar");
pub const FORGE_ADAPTER_REVISION: &str =
    "sha256:502334d232ca8b3031fdb102230dc0f173e30f7034484847f000b3c46e4aa0a0";

#[derive(Debug, Error)]
pub enum ForgeError {
    #[error("Forge {FORGE_VERSION} is not installed under {0}")]
    NotInstalled(String),
    #[error("Java 17 or newer is required to run Forge")]
    JavaUnavailable,
    #[error("Forge validation requires at least two decks")]
    NotEnoughDecks,
    #[error("deck references a card absent from the supplied pool: {0}")]
    UnknownCard(String),
    #[error("Forge file error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Forge download error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("Forge metadata error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Forge archive checksum mismatch: expected {expected}, got {actual}")]
    Checksum { expected: String, actual: String },
    #[error("Forge archive did not contain its desktop runtime jar")]
    MissingRuntime,
    #[error("Forge exited unsuccessfully: {0}")]
    Process(String),
    #[error("Forge validation was cancelled")]
    Cancelled,
    #[error("Forge event adapter returned an invalid protocol: {0}")]
    InvalidEventProtocol(String),
    #[error("Forge event probing does not support Commander yet")]
    UnsupportedEventMode,
    #[error("canonical conformance contract error: {0}")]
    Contract(#[from] ContractError),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ForgeRecordedEvent {
    pub event_schema_version: u32,
    pub sequence: u64,
    pub kind: String,
    pub forge_type: String,
    pub text: String,
    #[serde(flatten)]
    pub components: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ForgeEventProbe {
    pub protocol_version: u32,
    pub seed: i64,
    pub outcome: String,
    pub events: Vec<ForgeRecordedEvent>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ForgeScenarioProbe {
    pub protocol_version: u32,
    pub scenario_id: String,
    pub scenario_version: u32,
    pub seed: i64,
    pub final_state: serde_json::Value,
    pub events: Vec<ForgeRecordedEvent>,
}

#[derive(Debug, Deserialize)]
struct ForgeProbeResult {
    protocol_version: u32,
    seed: i64,
    events: u64,
    outcome: String,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ForgeScenarioResult {
    protocol_version: u32,
    scenario_id: String,
    scenario_version: u32,
    seed: i64,
    events: u64,
    #[serde(default)]
    final_state: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ForgeManifest {
    version: String,
    revision: String,
    archive_sha256: String,
    jar_relative: String,
}

struct ForgePaths {
    root: PathBuf,
    jar: PathBuf,
    manifest: PathBuf,
}

struct JobDirectory(PathBuf);

impl Drop for JobDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct TemporaryDeckFiles(Vec<PathBuf>);

impl Drop for TemporaryDeckFiles {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

pub fn status(data_dir: &Path) -> SimulatorStatus {
    let paths = forge_paths(data_dir);
    let java_version = java_major_version();
    let java_ready = java_version.is_some_and(|version| version >= 17);
    SimulatorStatus {
        id: "forge".into(),
        name: format!("Forge {FORGE_VERSION}"),
        installed: paths.jar.is_file() && paths.manifest.is_file(),
        data_ready: paths.jar.is_file() && java_ready,
        executable: paths.jar.display().to_string(),
        supported_formats: vec!["constructed".into(), "commander".into()],
        note: if java_ready {
            format!(
                "Independent GPL-3.0 rules engine for low-volume finalist cross-checks using Java {}. Its heuristic AI is strongest with aggro/midrange and weaker with combo/control; stock CLI results are matchup evidence, not canonical conformance evidence.",
                java_version.unwrap_or_default()
            )
        } else {
            "Java 17+ is required before Forge can run.".into()
        },
    }
}

pub fn install(
    data_dir: &Path,
    on_progress: impl Fn(InstallerProgress),
) -> Result<SimulatorStatus, ForgeError> {
    if status(data_dir).data_ready {
        return Ok(status(data_dir));
    }
    if !java_ready() {
        return Err(ForgeError::JavaUnavailable);
    }
    let engines_root = data_dir.join("engines").join("forge");
    fs::create_dir_all(&engines_root)?;
    let stage = engines_root.join(format!(".install-{}", Uuid::new_v4()));
    fs::create_dir_all(&stage)?;
    let result = install_staged(&engines_root, &stage, &on_progress);
    let _ = fs::remove_dir_all(&stage);
    result?;
    prune_old_versions(&engines_root)?;
    Ok(status(data_dir))
}

fn install_staged(
    engines_root: &Path,
    stage: &Path,
    on_progress: &impl Fn(InstallerProgress),
) -> Result<(), ForgeError> {
    let archive_path = stage.join("forge.tar.bz2");
    on_progress(InstallerProgress {
        stage: "forge-download".into(),
        detail: format!("Downloading Forge {FORGE_VERSION} · 288 MiB"),
        completed_units: 0,
        total_units: Some(FORGE_ARCHIVE_BYTES),
    });
    let client = Client::builder()
        .user_agent("Metagene/0.1 (+https://github.com/Card-Forge/forge)")
        .build()?;
    let mut response = client.get(FORGE_ARCHIVE_URL).send()?.error_for_status()?;
    let mut archive_file = fs::File::create(&archive_path)?;
    let mut downloaded = 0u64;
    let mut last_reported = 0u64;
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = response.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        archive_file.write_all(&buffer[..read])?;
        downloaded = downloaded.saturating_add(read as u64);
        if downloaded.saturating_sub(last_reported) >= 1024 * 1024
            || downloaded >= FORGE_ARCHIVE_BYTES
        {
            last_reported = downloaded;
            on_progress(InstallerProgress {
                stage: "forge-download".into(),
                detail: format!(
                    "Downloading Forge {FORGE_VERSION} · {:.0}%",
                    downloaded as f64 / FORGE_ARCHIVE_BYTES as f64 * 100.0
                ),
                completed_units: downloaded,
                total_units: Some(FORGE_ARCHIVE_BYTES),
            });
        }
    }
    archive_file.flush()?;
    drop(archive_file);
    let actual_hash = sha256_file(&archive_path)?;
    if actual_hash != FORGE_ARCHIVE_SHA256 {
        return Err(ForgeError::Checksum {
            expected: FORGE_ARCHIVE_SHA256.into(),
            actual: actual_hash,
        });
    }

    on_progress(InstallerProgress {
        stage: "forge-extract".into(),
        detail: "Verifying and extracting Forge outside the repository".into(),
        completed_units: downloaded,
        total_units: Some(downloaded),
    });
    let extracted = stage.join("extracted");
    fs::create_dir_all(&extracted)?;
    let archive = fs::File::open(&archive_path)?;
    Archive::new(BzDecoder::new(BufReader::new(archive))).unpack(&extracted)?;
    fs::remove_file(&archive_path)?;
    let jar = find_desktop_jar(&extracted).ok_or(ForgeError::MissingRuntime)?;
    let runtime_root = jar
        .parent()
        .ok_or(ForgeError::MissingRuntime)?
        .to_path_buf();
    if !runtime_root.join("res").is_dir() {
        return Err(ForgeError::MissingRuntime);
    }
    let final_root = engines_root.join(FORGE_VERSION);
    if final_root.exists() {
        fs::remove_dir_all(&final_root)?;
    }
    fs::rename(&runtime_root, &final_root)?;
    let final_jar = find_desktop_jar(&final_root).ok_or(ForgeError::MissingRuntime)?;
    let jar_relative = final_jar
        .strip_prefix(&final_root)
        .map_err(|_| ForgeError::MissingRuntime)?
        .to_string_lossy()
        .replace('\\', "/");
    fs::write(
        final_root.join("metagene-manifest.json"),
        serde_json::to_vec_pretty(&ForgeManifest {
            version: FORGE_VERSION.into(),
            revision: FORGE_RELEASE_REVISION.into(),
            archive_sha256: FORGE_ARCHIVE_SHA256.into(),
            jar_relative,
        })?,
    )?;
    write_profile(engines_root, &final_root)?;
    on_progress(InstallerProgress {
        stage: "forge-ready".into(),
        detail: format!("Forge {FORGE_VERSION} is ready"),
        completed_units: downloaded,
        total_units: Some(downloaded),
    });
    Ok(())
}

pub fn run_validation(
    data_dir: &Path,
    request: &HexdekValidationRequest,
) -> Result<SimulationReport, ForgeError> {
    run_validation_cancellable(data_dir, request, &CancellationToken::default())
}

/// Runs one deterministic Forge game while recording its public `GameEvent` stream.
/// This is the transport layer for conformance work; scenario-state injection and canonical
/// alias normalization are intentionally applied by the higher-level conformance runner.
pub fn run_event_probe(
    data_dir: &Path,
    request: &HexdekValidationRequest,
) -> Result<ForgeEventProbe, ForgeError> {
    let paths = forge_paths(data_dir);
    if !paths.jar.is_file() || !paths.manifest.is_file() {
        return Err(ForgeError::NotInstalled(paths.root.display().to_string()));
    }
    if !java_ready() {
        return Err(ForgeError::JavaUnavailable);
    }
    if request.commander {
        return Err(ForgeError::UnsupportedEventMode);
    }
    let opponent = request
        .opponents
        .first()
        .ok_or(ForgeError::NotEnoughDecks)?;
    let engines_root = data_dir.join("engines").join("forge");
    write_profile(&engines_root, &paths.root)?;

    let job = JobDirectory(
        engines_root
            .join("jobs")
            .join(format!("forge-events-{}", Uuid::new_v4())),
    );
    fs::create_dir_all(&job.0)?;
    let adapter_path = job.0.join("forge-conformance-adapter.jar");
    let candidate_path = job.0.join("seat-1.dck");
    let opponent_path = job.0.join("seat-2.dck");
    fs::write(&adapter_path, FORGE_ADAPTER_BYTES)?;
    fs::write(
        &candidate_path,
        export_forge_deck(&request.candidate, &request.pool, false, "Seat 1")?,
    )?;
    fs::write(
        &opponent_path,
        export_forge_deck(opponent, &request.pool, false, "Seat 2")?,
    )?;

    let classpath = std::env::join_paths([adapter_path.as_path(), paths.jar.as_path()])
        .map_err(|error| ForgeError::InvalidEventProtocol(error.to_string()))?;
    let output = Command::new("java")
        .current_dir(&paths.root)
        .arg("-Xmx2048m")
        .arg("-cp")
        .arg(classpath)
        .arg("ForgeConformanceAdapter")
        .arg(&candidate_path)
        .arg(&opponent_path)
        .arg(request.seed.to_string())
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(ForgeError::Process(trim_process_error(&stderr, &stdout)));
    }
    parse_event_probe_output(&stdout)
}

pub fn run_scenario_probe(
    data_dir: &Path,
    scenario: &ScenarioDefinition,
) -> Result<ForgeScenarioProbe, ForgeError> {
    let paths = forge_paths(data_dir);
    if !paths.jar.is_file() || !paths.manifest.is_file() {
        return Err(ForgeError::NotInstalled(paths.root.display().to_string()));
    }
    if !java_ready() {
        return Err(ForgeError::JavaUnavailable);
    }
    let engines_root = data_dir.join("engines").join("forge");
    write_profile(&engines_root, &paths.root)?;
    let job = JobDirectory(
        engines_root
            .join("jobs")
            .join(format!("forge-scenario-{}", Uuid::new_v4())),
    );
    fs::create_dir_all(&job.0)?;
    let adapter_path = job.0.join("forge-conformance-adapter.jar");
    fs::write(&adapter_path, FORGE_ADAPTER_BYTES)?;
    let classpath = std::env::join_paths([adapter_path.as_path(), paths.jar.as_path()])
        .map_err(|error| ForgeError::InvalidEventProtocol(error.to_string()))?;
    let output = Command::new("java")
        .current_dir(&paths.root)
        .arg("-Xmx2048m")
        .arg("-cp")
        .arg(classpath)
        .arg("ForgeConformanceAdapter")
        .arg("scenario")
        .arg(&scenario.id)
        .arg(scenario.seed.to_string())
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(ForgeError::Process(trim_process_error(&stderr, &stdout)));
    }
    let probe = parse_scenario_probe_output(&stdout)?;
    if probe.scenario_id != scenario.id
        || probe.scenario_version != scenario.version
        || probe.seed != scenario.seed
    {
        return Err(ForgeError::InvalidEventProtocol(
            "scenario identity/version/seed does not match the request".into(),
        ));
    }
    Ok(probe)
}

pub fn run_conformance_scenario(
    data_dir: &Path,
    scenario: &ScenarioDefinition,
) -> Result<ConformanceObservation, ForgeError> {
    let probe = run_scenario_probe(data_dir, scenario)?;
    let report = evaluate_assertions(&scenario.assertions, &probe.final_state)?;
    let normalized = normalize_scenario_events(scenario, &probe)?;
    let projected = project_events(&normalized, &scenario.event_projection)?;
    let event_hash = canonical_event_hash(&projected)?;
    let engine_revision = format!(
        "{FORGE_VERSION}-{FORGE_RELEASE_REVISION}+adapter:{}",
        FORGE_ADAPTER_REVISION.trim_start_matches("sha256:")
    );
    let configuration_hash =
        crate::provenance::configuration_hash("conformance-scenario-v2", scenario)?;
    Ok(ConformanceObservation {
        engine: "forge".into(),
        engine_revision: engine_revision.clone(),
        provenance: Some(crate::provenance::ResultProvenance::new(
            engine_revision,
            forge_card_data_revision(&forge_paths(data_dir))?,
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

fn forge_card_data_revision(paths: &ForgePaths) -> Result<String, ForgeError> {
    let archive = paths
        .root
        .join("res")
        .join("cardsfolder")
        .join("cardsfolder.zip");
    let mut hasher = Sha256::new();
    hasher.update(fs::read(archive)?);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn normalize_scenario_events(
    scenario: &ScenarioDefinition,
    probe: &ForgeScenarioProbe,
) -> Result<Vec<CanonicalEvent>, ForgeError> {
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
        ForgeError::InvalidEventProtocol(format!(
            "no Forge normalizer for scenario '{}'",
            scenario.id
        ))
    })?;
    #[derive(Default)]
    struct Episode {
        tokens: usize,
        mythos_to_graveyard: bool,
    }
    let mut episodes: Vec<Episode> = Vec::new();
    for event in &probe.events {
        if event.kind == "spell.cast"
            && component_contains(event, "targetDescription", "Parallel Lives")
        {
            episodes.push(Episode::default());
        } else if let Some(episode) = episodes.last_mut() {
            if event.kind == "zone.changed"
                && component_is_null(event, "from")
                && component_contains(event, "card", "Parallel Lives")
                && component_contains(event, "to", "zoneType=Battlefield")
            {
                episode.tokens += 1;
            } else if event.kind == "zone.changed"
                && component_contains(event, "card", "Mythos of Illuna")
                && component_contains(event, "from", "zoneType=Stack")
                && component_contains(event, "to", "zoneType=Graveyard")
            {
                episode.mythos_to_graveyard = true;
            }
        }
    }
    if episodes.len() != shape.cast_sources.len() {
        return Err(ForgeError::InvalidEventProtocol(format!(
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
                    payload: serde_json::Value| {
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
        if episode.tokens > 1 {
            push(
                "replacement.applied",
                Some(1),
                Some(shape.target_alias),
                Vec::new(),
                serde_json::json!({
                    "affected_event": "token.created",
                    "multiplier": episode.tokens,
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
        return Err(ForgeError::InvalidEventProtocol(
            "scenario emitted no normalizable events".into(),
        ));
    }
    Ok(events)
}

fn normalize_clone_layer_events(
    probe: &ForgeScenarioProbe,
) -> Result<Vec<CanonicalEvent>, ForgeError> {
    let cast = probe
        .events
        .iter()
        .any(|event| event.kind == "spell.cast" && event.text.contains("cast Clone"));
    let entered = probe.events.iter().any(|event| {
        event.kind == "zone.changed"
            && component_contains(event, "card", "Grizzly Bears")
            && component_contains(event, "from", "zoneType=Stack")
            && component_contains(event, "to", "zoneType=Battlefield")
    });
    if !cast || !entered {
        return Err(ForgeError::InvalidEventProtocol(
            "Clone scenario did not emit a cast and copied battlefield entry".into(),
        ));
    }
    let clone = probe
        .final_state
        .pointer("/objects/clone")
        .ok_or_else(|| ForgeError::InvalidEventProtocol("Clone final state is missing".into()))?;
    let number = |field: &str| {
        clone
            .get(field)
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| {
                ForgeError::InvalidEventProtocol(format!("Clone final state has no {field}"))
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
    probe: &ForgeScenarioProbe,
) -> Result<Vec<CanonicalEvent>, ForgeError> {
    let cast = probe.events.iter().any(|event| {
        event.kind == "spell.cast"
            && event.text.contains("cast Strangle")
            && component_contains(event, "targetDescription", "Grizzly Bears")
    });
    let damage = probe.events.iter().find(|event| {
        event.kind == "damage.dealt"
            && event.text.contains("Strangle")
            && component_contains(event, "card", "Grizzly Bears")
    });
    let spell_to_graveyard = probe.events.iter().any(|event| {
        event.kind == "zone.changed"
            && component_contains(event, "card", "Strangle")
            && component_contains(event, "from", "zoneType=Stack")
            && component_contains(event, "to", "zoneType=Graveyard")
    });
    let bears_to_graveyard = probe.events.iter().any(|event| {
        event.kind == "zone.changed"
            && component_contains(event, "card", "Grizzly Bears")
            && component_contains(event, "from", "zoneType=Battlefield")
            && component_contains(event, "to", "zoneType=Graveyard")
    });
    let destroyed = probe
        .events
        .iter()
        .any(|event| event.kind == "card.destroyed");
    let amount = damage
        .and_then(|event| event.text.split(" dealt ").nth(1))
        .and_then(|tail| tail.split_whitespace().next())
        .and_then(|value| value.parse::<i64>().ok());
    if !cast || amount.is_none() || !spell_to_graveyard || !bears_to_graveyard || !destroyed {
        return Err(ForgeError::InvalidEventProtocol(
            "targeted lethal-damage scenario omitted required production events".into(),
        ));
    }
    Ok(targeted_lethal_canonical_events(amount.unwrap_or_default()))
}

fn normalize_creature_etb_life_trigger_events(
    probe: &ForgeScenarioProbe,
) -> Result<Vec<CanonicalEvent>, ForgeError> {
    let cast = probe
        .events
        .iter()
        .any(|event| event.kind == "spell.cast" && event.text.contains("cast Grizzly Bears"));
    let entered = probe.events.iter().any(|event| {
        event.kind == "zone.changed"
            && component_contains(event, "card", "Grizzly Bears")
            && component_contains(event, "from", "zoneType=Stack")
            && component_contains(event, "to", "zoneType=Battlefield")
    });
    let trigger = probe.events.iter().any(|event| {
        event.kind == "spell.cast"
            && event.text.contains("triggered")
            && event.text.contains("you gain 1 life")
            && event.text.contains("Grizzly Bears")
    });
    let life_event = probe
        .events
        .iter()
        .find(|event| event.kind == "life.changed")
        .and_then(|event| event.text.split("changed: ").nth(1))
        .and_then(|change| change.split_once(" -> "))
        .and_then(|(before, after)| {
            Some((before.parse::<i64>().ok()?, after.parse::<i64>().ok()?))
        });
    let life = probe
        .final_state
        .pointer("/players/seat-1/life")
        .and_then(serde_json::Value::as_i64);
    if !cast || !entered || !trigger || life_event.is_none() || life.is_none() {
        return Err(ForgeError::InvalidEventProtocol(
            "creature-ETB trigger scenario omitted cast, entry, trigger, or life-change evidence"
                .into(),
        ));
    }
    let (before, after) = life_event.unwrap_or_default();
    if Some(after) != life {
        return Err(ForgeError::InvalidEventProtocol(
            "creature-ETB life event disagrees with final state".into(),
        ));
    }
    Ok(creature_etb_life_trigger_canonical_events(
        after - before,
        after,
    ))
}

fn normalize_unblocked_combat_damage_events(
    probe: &ForgeScenarioProbe,
) -> Result<Vec<CanonicalEvent>, ForgeError> {
    let declared = probe
        .final_state
        .pointer("/objects/bears/declared_attacker")
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    let tapped = probe
        .final_state
        .pointer("/objects/bears/tapped")
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    let damage = probe
        .events
        .iter()
        .find(|event| {
            event.kind == "damage.dealt"
                && event.text.contains("combat damage")
                && event.text.contains("Grizzly Bears")
        })
        .and_then(|event| event.text.split(" took ").nth(1))
        .and_then(|tail| tail.split_whitespace().next())
        .and_then(|value| value.parse::<i64>().ok());
    let life_event = probe
        .events
        .iter()
        .find(|event| event.kind == "life.changed")
        .and_then(|event| event.text.split("changed: ").nth(1))
        .and_then(|change| change.split_once(" -> "))
        .and_then(|(before, after)| {
            Some((before.parse::<i64>().ok()?, after.parse::<i64>().ok()?))
        });
    let life = probe
        .final_state
        .pointer("/players/seat-2/life")
        .and_then(serde_json::Value::as_i64);
    if !declared || !tapped || damage.is_none() || life_event.is_none() || life.is_none() {
        return Err(ForgeError::InvalidEventProtocol(
            "combat scenario omitted declaration, tap, combat-damage, or life-change evidence"
                .into(),
        ));
    }
    let (before, after) = life_event.unwrap_or_default();
    if Some(after) != life || damage != Some(before - after) {
        return Err(ForgeError::InvalidEventProtocol(
            "combat damage evidence disagrees with the life change or final state".into(),
        ));
    }
    Ok(unblocked_combat_damage_canonical_events(
        damage.unwrap_or_default(),
        after - before,
        after,
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
    probe: &ForgeScenarioProbe,
) -> Result<Vec<CanonicalEvent>, ForgeError> {
    let boolean = |path: &str| {
        probe
            .final_state
            .pointer(path)
            .and_then(serde_json::Value::as_bool)
    };
    let number = |path: &str| {
        probe
            .final_state
            .pointer(path)
            .and_then(serde_json::Value::as_i64)
    };
    let declared = boolean("/objects/bears/declared_attacker") == Some(true);
    let tapped = boolean("/objects/bears/tapped") == Some(true);
    let defender = number("/objects/bears/defender");
    let damage = probe
        .events
        .iter()
        .find(|event| {
            event.kind == "damage.dealt"
                && event.text.contains("Metagene-Seat-3 took")
                && event.text.contains("combat damage from Grizzly Bears")
        })
        .and_then(|event| event.text.split(" took ").nth(1))
        .and_then(|tail| tail.split_whitespace().next())
        .and_then(|value| value.parse::<i64>().ok());
    let life_event = probe
        .events
        .iter()
        .find(|event| {
            event.kind == "life.changed" && event.text.contains("Metagene-Seat-3's lives changed")
        })
        .and_then(|event| event.text.split("changed: ").nth(1))
        .and_then(|change| change.split_once(" -> "))
        .and_then(|(before, after)| {
            Some((before.parse::<i64>().ok()?, after.parse::<i64>().ok()?))
        });
    let seat_two_untouched = number("/players/seat-2/life") == Some(20)
        && boolean("/players/seat-2/lost") == Some(false);
    let seat_three_eliminated = number("/players/seat-3/life") == Some(0)
        && boolean("/players/seat-3/lost") == Some(true)
        && number("/battlefield/seat-3/cards/Forest/count") == Some(0);
    if !declared
        || !tapped
        || defender != Some(3)
        || damage != Some(2)
        || life_event != Some((2, 0))
        || !seat_two_untouched
        || !seat_three_eliminated
    {
        return Err(ForgeError::InvalidEventProtocol(
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
    probe: &ForgeScenarioProbe,
) -> Result<Vec<CanonicalEvent>, ForgeError> {
    let cast = probe
        .events
        .iter()
        .any(|event| event.kind == "spell.cast" && event.text.contains("cast Oracle of the Alpha"));
    let entered = probe.events.iter().any(|event| {
        event.kind == "zone.changed"
            && component_contains(event, "card", "Oracle of the Alpha")
            && component_contains(event, "from", "zoneType=Stack")
            && component_contains(event, "to", "zoneType=Battlefield")
    });
    let triggered = probe.events.iter().any(|event| {
        event.kind == "spell.cast"
            && event.text.contains("triggered")
            && event.text.contains("conjure the Power Nine")
    });
    let conjured = POWER_NINE.iter().all(|name| {
        probe.events.iter().any(|event| {
            event.kind == "zone.changed"
                && component_contains(event, "card", name)
                && component_contains(event, "to", "zoneType=Library")
        })
    });
    let final_state_matches = probe
        .final_state
        .pointer("/battlefield/seat-1/cards/Oracle of the Alpha/count")
        .and_then(serde_json::Value::as_i64)
        == Some(1)
        && probe
            .final_state
            .pointer("/library/seat-1/total_count")
            .and_then(serde_json::Value::as_i64)
            == Some(9)
        && POWER_NINE.iter().all(|name| {
            probe
                .final_state
                .pointer(&format!("/library/seat-1/cards/{name}/count"))
                .and_then(serde_json::Value::as_i64)
                == Some(1)
        });
    if !cast || !entered || !triggered || !conjured || !final_state_matches {
        return Err(ForgeError::InvalidEventProtocol(
            "Oracle of the Alpha scenario omitted cast, trigger, Power Nine conjure, or final-library evidence".into(),
        ));
    }
    Ok(oracle_of_the_alpha_canonical_events(true))
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
    probe: &ForgeScenarioProbe,
) -> Result<Vec<CanonicalEvent>, ForgeError> {
    let cast = probe.events.iter().any(|event| {
        event.kind == "spell.cast" && event.text.contains("cast A-Falcon Abomination")
    });
    let entered = probe.events.iter().any(|event| {
        event.kind == "zone.changed"
            && component_contains(event, "card", "A-Falcon Abomination")
            && component_contains(event, "from", "zoneType=Stack")
            && component_contains(event, "to", "zoneType=Battlefield")
    });
    let original_legal = probe
        .final_state
        .pointer("/timing/original_legal")
        .and_then(serde_json::Value::as_bool);
    let rebalanced_legal = probe
        .final_state
        .pointer("/timing/rebalanced_legal")
        .and_then(serde_json::Value::as_bool);
    let battlefield_count = probe
        .final_state
        .pointer("/battlefield/seat-1/cards/A-Falcon Abomination/count")
        .and_then(serde_json::Value::as_i64);
    if !cast
        || !entered
        || original_legal.is_none()
        || rebalanced_legal.is_none()
        || battlefield_count != Some(1)
    {
        return Err(ForgeError::InvalidEventProtocol(
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
    payload: serde_json::Value,
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

fn component_contains(event: &ForgeRecordedEvent, key: &str, needle: &str) -> bool {
    event
        .components
        .get(key)
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| value.contains(needle))
}

fn component_is_null(event: &ForgeRecordedEvent, key: &str) -> bool {
    event
        .components
        .get(key)
        .is_some_and(serde_json::Value::is_null)
}

pub fn run_validation_cancellable(
    data_dir: &Path,
    request: &HexdekValidationRequest,
    cancellation: &CancellationToken,
) -> Result<SimulationReport, ForgeError> {
    let paths = forge_paths(data_dir);
    if !paths.jar.is_file() || !paths.manifest.is_file() {
        return Err(ForgeError::NotInstalled(paths.root.display().to_string()));
    }
    if !java_ready() {
        return Err(ForgeError::JavaUnavailable);
    }
    let engines_root = data_dir.join("engines").join("forge");
    write_profile(&engines_root, &paths.root)?;
    if request.opponents.is_empty() {
        return Err(ForgeError::NotEnoughDecks);
    }
    if request.commander && request.opponents.len() < request.seats.clamp(2, 8) - 1 {
        return Err(ForgeError::NotEnoughDecks);
    }
    if cancellation.is_cancelled() {
        return Err(ForgeError::Cancelled);
    }
    let cache_identity = forge_cache_identity(&paths, request)?;
    if let Ok(Some(report)) = crate::match_cache::lookup(data_dir, &cache_identity.cache_key) {
        return Ok(report);
    }
    let started = Instant::now();
    let jobs_root = data_dir.join("simulation-jobs");
    fs::create_dir_all(&jobs_root)?;
    let job = JobDirectory(jobs_root.join(format!("forge-{}", Uuid::new_v4())));
    fs::create_dir_all(&job.0)?;

    let mut aggregate = ForgeAggregate::default();
    if request.commander {
        let seats = request.seats.clamp(2, 8);
        let decks = std::iter::once(&request.candidate)
            .chain(request.opponents.iter())
            .take(seats)
            .collect::<Vec<_>>();
        let labels = decks
            .iter()
            .enumerate()
            .map(|(index, deck)| {
                if index == 0 {
                    format!("Candidate · {}", deck.name)
                } else {
                    format!("Opponent {index} · {}", deck.name)
                }
            })
            .collect::<Vec<_>>();
        run_batch(
            ForgeBatchContext {
                paths: &paths,
                job_dir: &job.0.join("pod"),
                request,
                cancellation,
            },
            &decks,
            &labels,
            request.seed,
            &mut aggregate,
        )?;
    } else {
        let allocations = crate::simulator::weighted_game_allocations(
            request.games.clamp(1, 10_000),
            request.opponents.len(),
            &request.opponent_weights,
        );
        let allocated_total = allocations.iter().sum::<usize>().max(1);
        for (index, opponent) in request.opponents.iter().enumerate() {
            let labels = vec![
                format!("Candidate · {}", request.candidate.name),
                format!("Opponent {} · {}", index + 1, opponent.name),
            ];
            let mut matchup_request = request.clone();
            matchup_request.games = allocations[index];
            matchup_request.opponents = vec![opponent.clone()];
            matchup_request.opponent_weights = vec![1.0];
            aggregate.matchup_weights.insert(
                labels[1].clone(),
                allocations[index] as f64 / allocated_total as f64,
            );
            run_batch(
                ForgeBatchContext {
                    paths: &paths,
                    job_dir: &job.0.join(format!("matchup-{index}")),
                    request: &matchup_request,
                    cancellation,
                },
                &[&request.candidate, opponent],
                &labels,
                request
                    .seed
                    .wrapping_add((index as i64).wrapping_mul(2_000_033)),
                &mut aggregate,
            )?;
        }
    }
    let mut report = aggregate.report(started.elapsed());
    report.provenance = Some(cache_identity.provenance.clone());
    let _ = crate::match_cache::store(
        data_dir,
        &cache_identity.cache_key,
        "forge",
        &cache_identity.cache_revision,
        &report,
    );
    Ok(report)
}

struct ForgeMatchIdentity {
    cache_key: String,
    cache_revision: String,
    provenance: crate::provenance::ResultProvenance,
}

fn forge_cache_identity(
    paths: &ForgePaths,
    request: &HexdekValidationRequest,
) -> Result<ForgeMatchIdentity, ForgeError> {
    let batches = if request.commander {
        let seats = request.seats.clamp(2, 8);
        let decks = std::iter::once(&request.candidate)
            .chain(request.opponents.iter())
            .take(seats);
        vec![
            decks
                .enumerate()
                .map(|(index, deck)| {
                    export_forge_deck(deck, &request.pool, true, &format!("Seat {index}"))
                })
                .collect::<Result<Vec<_>, _>>()?,
        ]
    } else {
        request
            .opponents
            .iter()
            .map(|opponent| {
                [&request.candidate, opponent]
                    .into_iter()
                    .enumerate()
                    .map(|(index, deck)| {
                        export_forge_deck(deck, &request.pool, false, &format!("Seat {index}"))
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    let input = serde_json::json!({
        "batches": batches,
        "games": if request.commander {
            vec![request.games.clamp(1, 10_000)]
        } else {
            crate::simulator::weighted_game_allocations(
                request.games.clamp(1, 10_000),
                request.opponents.len(),
                &request.opponent_weights,
            )
        },
        "seed": request.seed,
        "seats": request.seats.clamp(2, 8),
        "game_timeout_seconds": request.game_timeout_seconds.clamp(1, 3_600),
        "commander": request.commander,
    });
    let engine_revision = format!("{FORGE_VERSION}-{FORGE_RELEASE_REVISION}");
    let card_data_revision = forge_card_data_revision(paths)?;
    let configuration_hash = crate::provenance::configuration_hash("forge-matchup-v1", &input)?;
    let cache_revision = format!("{engine_revision}+cards:{card_data_revision}");
    let cache_key = crate::match_cache::key("forge", &cache_revision, &input)
        .map_err(|error| ForgeError::InvalidEventProtocol(error.to_string()))?;
    Ok(ForgeMatchIdentity {
        cache_key,
        cache_revision,
        provenance: crate::provenance::ResultProvenance::new(
            engine_revision,
            card_data_revision,
            None,
            configuration_hash,
        ),
    })
}

#[derive(Default)]
struct ForgeAggregate {
    rows: HashMap<String, (usize, usize)>,
    matchup_wins: HashMap<String, HashMap<String, usize>>,
    matchup_games: HashMap<String, HashMap<String, usize>>,
    matchup_weights: HashMap<String, f64>,
    games: usize,
    draws: usize,
    crashes: usize,
    duration_ms: u64,
}

impl ForgeAggregate {
    fn report(self, elapsed: Duration) -> SimulationReport {
        let mut rows = self
            .rows
            .into_iter()
            .map(|(deck, (wins, games))| {
                let (lower, upper) = wilson_interval(wins, games);
                MatchupRow {
                    deck,
                    games,
                    wins,
                    win_rate: wins as f64 / games.max(1) as f64,
                    win_rate_lower_bound: lower,
                    win_rate_upper_bound: upper,
                    average_turn_to_win: None,
                }
            })
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| left.deck.cmp(&right.deck));
        let matchup_matrix = self
            .matchup_wins
            .iter()
            .map(|(left, opponents)| {
                let rates = opponents
                    .iter()
                    .map(|(right, wins)| {
                        let games = self
                            .matchup_games
                            .get(left)
                            .and_then(|values| values.get(right))
                            .copied()
                            .unwrap_or(0);
                        (right.clone(), *wins as f64 / games.max(1) as f64)
                    })
                    .collect();
                (left.clone(), rates)
            })
            .collect();
        SimulationReport {
            engine: "forge".into(),
            schema_version: format!("forge-{FORGE_VERSION}-text-v1"),
            engine_routes: HashMap::new(),
            route_failures: Vec::new(),
            provenance: None,
            games: self.games,
            draws: self.draws,
            crashes: self.crashes,
            cached_games: 0,
            seat_order: Vec::new(),
            average_turns: 0.0,
            games_per_second: self.games as f64 / elapsed.as_secs_f64().max(f64::EPSILON),
            rows,
            matchup_matrix,
            matchup_wins: self.matchup_wins,
            matchup_games: self.matchup_games,
            matchup_weights: self.matchup_weights,
            commander_evidence: None,
            combo_execution: None,
            performance: None,
        }
    }
}

struct ForgeBatchContext<'a> {
    paths: &'a ForgePaths,
    job_dir: &'a Path,
    request: &'a HexdekValidationRequest,
    cancellation: &'a CancellationToken,
}

fn run_batch(
    context: ForgeBatchContext<'_>,
    decks: &[&Deck],
    labels: &[String],
    seed: i64,
    aggregate: &mut ForgeAggregate,
) -> Result<(), ForgeError> {
    let ForgeBatchContext {
        paths,
        job_dir,
        request,
        cancellation,
    } = context;
    fs::create_dir_all(job_dir)?;
    let pool = &request.pool;
    debug_assert_eq!(labels.len(), decks.len());
    let engine_root = paths.root.parent().ok_or(ForgeError::MissingRuntime)?;
    let deck_directory =
        engine_root
            .join("runtime-data")
            .join("decks")
            .join(if request.commander {
                "commander"
            } else {
                "constructed"
            });
    fs::create_dir_all(&deck_directory)?;
    let batch_id = Uuid::new_v4().simple().to_string();
    let mut filenames = Vec::with_capacity(decks.len());
    let mut temporary_paths = Vec::with_capacity(decks.len());
    for (index, deck) in decks.iter().enumerate() {
        let filename = format!("metagene-{batch_id}-seat-{index}.dck");
        let deck_path = deck_directory.join(&filename);
        fs::write(
            &deck_path,
            export_forge_deck(deck, pool, request.commander, &format!("Seat {index}"))?,
        )?;
        temporary_paths.push(deck_path);
        filenames.push(filename);
    }
    let _temporary_decks = TemporaryDeckFiles(temporary_paths);
    let stdout_path = job_dir.join("stdout.log");
    let stderr_path = job_dir.join("stderr.log");
    let mut command = Command::new("java");
    command
        .current_dir(&paths.root)
        .arg("-Xmx2048m")
        .arg("-jar")
        .arg(&paths.jar)
        .arg("sim")
        .arg("-d");
    for filename in &filenames {
        command.arg(filename);
    }
    command
        .arg("-n")
        .arg(request.games.clamp(1, 10_000).to_string())
        .arg("-s")
        .arg(seed.to_string())
        .arg("-c")
        .arg(request.game_timeout_seconds.clamp(1, 3_600).to_string())
        .arg("-q");
    if request.commander {
        command.arg("-f").arg("commander");
    }
    command
        .stdout(Stdio::from(fs::File::create(&stdout_path)?))
        .stderr(Stdio::from(fs::File::create(&stderr_path)?));
    let mut child = command.spawn()?;
    // Forge's `-c` setting bounds individual games, not JVM startup, deck
    // loading, or a wedged batch. Bound the complete external process too.
    let batch_timeout_seconds = 20_u64.saturating_add(
        (request.games.clamp(1, 10_000) as u64)
            .saturating_mul(request.game_timeout_seconds.clamp(1, 3_600) as u64),
    );
    let batch_deadline = Instant::now() + Duration::from_secs(batch_timeout_seconds);
    let status = loop {
        if cancellation.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ForgeError::Cancelled);
        }
        if Instant::now() >= batch_deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ForgeError::Process(format!(
                "batch exceeded its {batch_timeout_seconds} second wall-clock deadline"
            )));
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let stdout = read_limited(&stdout_path)?;
    let stderr = read_limited(&stderr_path)?;
    if !status.success() {
        return Err(ForgeError::Process(trim_process_error(&stderr, &stdout)));
    }
    if stdout.contains("Could not load deck") || stdout.contains("No deck found") {
        return Err(ForgeError::Process(trim_process_error(&stdout, &stderr)));
    }
    let parsed = parse_forge_output(&stdout, labels.len(), request.games.clamp(1, 10_000));
    aggregate.games = aggregate.games.saturating_add(parsed.completed);
    aggregate.draws = aggregate.draws.saturating_add(parsed.draws);
    aggregate.crashes = aggregate.crashes.saturating_add(parsed.missing);
    aggregate.duration_ms = aggregate.duration_ms.saturating_add(parsed.duration_ms);
    for (index, wins) in parsed.wins.iter().copied().enumerate() {
        let games = parsed.completed;
        let entry = aggregate.rows.entry(labels[index].clone()).or_default();
        entry.0 = entry.0.saturating_add(wins);
        entry.1 = entry.1.saturating_add(games);
    }
    for left in 0..labels.len() {
        for right in 0..labels.len() {
            if left == right {
                continue;
            }
            aggregate
                .matchup_wins
                .entry(labels[left].clone())
                .or_default()
                .entry(labels[right].clone())
                .and_modify(|wins| *wins = wins.saturating_add(parsed.wins[left]))
                .or_insert(parsed.wins[left]);
            aggregate
                .matchup_games
                .entry(labels[left].clone())
                .or_default()
                .entry(labels[right].clone())
                .and_modify(|games| *games = games.saturating_add(parsed.completed))
                .or_insert(parsed.completed);
        }
    }
    Ok(())
}

struct ParsedForgeOutput {
    wins: Vec<usize>,
    draws: usize,
    completed: usize,
    missing: usize,
    duration_ms: u64,
}

fn parse_forge_output(output: &str, seats: usize, requested: usize) -> ParsedForgeOutput {
    let mut parsed = ParsedForgeOutput {
        wins: vec![0; seats],
        draws: 0,
        completed: 0,
        missing: 0,
        duration_ms: 0,
    };
    for line in output.lines().filter(|line| line.contains("Game Result:")) {
        parsed.completed = parsed.completed.saturating_add(1);
        if line.contains("ended in a Draw") {
            parsed.draws = parsed.draws.saturating_add(1);
        } else {
            for seat in 0..seats {
                if line.contains(&format!("Ai({})-", seat + 1)) && line.contains("has won!") {
                    parsed.wins[seat] = parsed.wins[seat].saturating_add(1);
                    break;
                }
            }
        }
        if let Some(after) = line.split("ended in ").nth(1)
            && let Some(milliseconds) = after.split_whitespace().next()
        {
            parsed.duration_ms = parsed
                .duration_ms
                .saturating_add(milliseconds.parse::<u64>().unwrap_or(0));
        }
    }
    parsed.missing = requested.saturating_sub(parsed.completed);
    parsed
}

fn export_forge_deck(
    deck: &Deck,
    pool: &CardPool,
    commander: bool,
    name: &str,
) -> Result<String, ForgeError> {
    let cards = pool
        .cards
        .iter()
        .map(|card| (&card.id, card))
        .collect::<HashMap<_, _>>();
    let mut output = format!(
        "[metadata]\nName={name}\nDeck Type={}\n",
        if commander {
            "Commander"
        } else {
            "Constructed"
        }
    );
    if commander {
        output.push_str("[Commander]\n");
        for entry in &deck.command_zone {
            let card = cards
                .get(&entry.card_id)
                .ok_or_else(|| ForgeError::UnknownCard(entry.card_id.0.clone()))?;
            output.push_str(&format!("{} {}\n", entry.quantity, card.name));
        }
    }
    output.push_str("[Main]\n");
    for entry in &deck.mainboard {
        let card = cards
            .get(&entry.card_id)
            .ok_or_else(|| ForgeError::UnknownCard(entry.card_id.0.clone()))?;
        output.push_str(&format!("{} {}\n", entry.quantity, card.name));
    }
    if !deck.sideboard.is_empty() {
        output.push_str("[Sideboard]\n");
        for entry in &deck.sideboard {
            let card = cards
                .get(&entry.card_id)
                .ok_or_else(|| ForgeError::UnknownCard(entry.card_id.0.clone()))?;
            output.push_str(&format!("{} {}\n", entry.quantity, card.name));
        }
    }
    Ok(output)
}

fn forge_paths(data_dir: &Path) -> ForgePaths {
    let root = data_dir.join("engines").join("forge").join(FORGE_VERSION);
    let manifest = root.join("metagene-manifest.json");
    let jar = fs::read(&manifest)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ForgeManifest>(&bytes).ok())
        .map(|manifest| root.join(manifest.jar_relative))
        .unwrap_or_else(|| {
            root.join(format!(
                "forge-gui-desktop-{FORGE_VERSION}-jar-with-dependencies.jar"
            ))
        });
    ForgePaths {
        root,
        jar,
        manifest,
    }
}

fn java_major_version() -> Option<u32> {
    let output = Command::new("java").arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(if output.stdout.is_empty() {
        &output.stderr
    } else {
        &output.stdout
    });
    parse_java_major_version(&text)
}

fn parse_java_major_version(text: &str) -> Option<u32> {
    let version = text
        .split_whitespace()
        .map(|token| token.trim_matches('"'))
        .find(|token| {
            token
                .chars()
                .next()
                .is_some_and(|value| value.is_ascii_digit())
        })?;
    let mut parts = version.split('.');
    let first = parts.next()?.parse::<u32>().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

fn java_ready() -> bool {
    java_major_version().is_some_and(|version| version >= 17)
}

fn find_desktop_jar(root: &Path) -> Option<PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("forge-gui-desktop-")
                        && name.ends_with("-jar-with-dependencies.jar")
                })
            {
                return Some(path);
            }
        }
    }
    None
}

fn prune_old_versions(engines_root: &Path) -> Result<(), ForgeError> {
    for entry in fs::read_dir(engines_root)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if entry.path().is_dir()
            && name != FORGE_VERSION
            && name != "runtime-data"
            && name != "runtime-cache"
            && !name.starts_with(".install-")
        {
            fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

fn property_path(path: &Path) -> String {
    format!("{}{}", path.to_string_lossy(), std::path::MAIN_SEPARATOR).replace('\\', "\\\\")
}

fn write_profile(engines_root: &Path, runtime_root: &Path) -> Result<(), std::io::Error> {
    let runtime_data = engines_root.join("runtime-data");
    let runtime_cache = engines_root.join("runtime-cache");
    fs::create_dir_all(&runtime_data)?;
    fs::create_dir_all(&runtime_cache)?;
    fs::write(
        runtime_root.join("forge.profile.properties"),
        format!(
            "userDir={}\ncacheDir={}\n",
            property_path(&runtime_data),
            property_path(&runtime_cache)
        ),
    )
}

fn sha256_file(path: &Path) -> Result<String, std::io::Error> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn read_limited(path: &Path) -> Result<String, std::io::Error> {
    let file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_CAPTURE_BYTES).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn parse_event_probe_output(output: &str) -> Result<ForgeEventProbe, ForgeError> {
    let mut events = Vec::new();
    let mut result = None;
    for line in output.lines() {
        if let Some(json) = line.strip_prefix("METAGENE_EVENT ") {
            let event: ForgeRecordedEvent = serde_json::from_str(json).map_err(|error| {
                ForgeError::InvalidEventProtocol(format!("invalid event JSON: {error}"))
            })?;
            if event.event_schema_version != 1 {
                return Err(ForgeError::InvalidEventProtocol(format!(
                    "unsupported event schema {}",
                    event.event_schema_version
                )));
            }
            let expected = events.len() as u64 + 1;
            if event.sequence != expected {
                return Err(ForgeError::InvalidEventProtocol(format!(
                    "event sequence must be contiguous; expected {expected}, got {}",
                    event.sequence
                )));
            }
            events.push(event);
        } else if let Some(json) = line.strip_prefix("METAGENE_RESULT ") {
            if result.is_some() {
                return Err(ForgeError::InvalidEventProtocol(
                    "multiple result records".into(),
                ));
            }
            result = Some(
                serde_json::from_str::<ForgeProbeResult>(json).map_err(|error| {
                    ForgeError::InvalidEventProtocol(format!("invalid result JSON: {error}"))
                })?,
            );
        }
    }
    let result = result
        .ok_or_else(|| ForgeError::InvalidEventProtocol("missing METAGENE_RESULT record".into()))?;
    if result.protocol_version != 1 {
        return Err(ForgeError::InvalidEventProtocol(format!(
            "unsupported protocol version {}",
            result.protocol_version
        )));
    }
    if let Some(error) = result.error {
        return Err(ForgeError::Process(error));
    }
    if result.events != events.len() as u64 {
        return Err(ForgeError::InvalidEventProtocol(format!(
            "result declares {} events but {} were parsed",
            result.events,
            events.len()
        )));
    }
    Ok(ForgeEventProbe {
        protocol_version: result.protocol_version,
        seed: result.seed,
        outcome: result.outcome,
        events,
    })
}

fn parse_scenario_probe_output(output: &str) -> Result<ForgeScenarioProbe, ForgeError> {
    let mut events = Vec::new();
    let mut result = None;
    for line in output.lines() {
        if let Some(json) = line.strip_prefix("METAGENE_EVENT ") {
            let event: ForgeRecordedEvent = serde_json::from_str(json).map_err(|error| {
                ForgeError::InvalidEventProtocol(format!("invalid event JSON: {error}"))
            })?;
            if event.event_schema_version != 1 {
                return Err(ForgeError::InvalidEventProtocol(format!(
                    "unsupported event schema {}",
                    event.event_schema_version
                )));
            }
            let expected = events.len() as u64 + 1;
            if event.sequence != expected {
                return Err(ForgeError::InvalidEventProtocol(format!(
                    "event sequence must be contiguous; expected {expected}, got {}",
                    event.sequence
                )));
            }
            events.push(event);
        } else if let Some(json) = line.strip_prefix("METAGENE_SCENARIO_RESULT ") {
            if result.is_some() {
                return Err(ForgeError::InvalidEventProtocol(
                    "multiple scenario result records".into(),
                ));
            }
            result = Some(
                serde_json::from_str::<ForgeScenarioResult>(json).map_err(|error| {
                    ForgeError::InvalidEventProtocol(format!(
                        "invalid scenario result JSON: {error}"
                    ))
                })?,
            );
        }
    }
    let result = result.ok_or_else(|| {
        ForgeError::InvalidEventProtocol("missing METAGENE_SCENARIO_RESULT record".into())
    })?;
    if result.protocol_version != 1 {
        return Err(ForgeError::InvalidEventProtocol(format!(
            "unsupported protocol version {}",
            result.protocol_version
        )));
    }
    if let Some(error) = result.error {
        return Err(ForgeError::Process(error));
    }
    if result.events != events.len() as u64 {
        return Err(ForgeError::InvalidEventProtocol(format!(
            "scenario declares {} events but {} were parsed",
            result.events,
            events.len()
        )));
    }
    let final_state = result.final_state.ok_or_else(|| {
        ForgeError::InvalidEventProtocol("successful scenario has no final_state".into())
    })?;
    Ok(ForgeScenarioProbe {
        protocol_version: result.protocol_version,
        scenario_id: result.scenario_id,
        scenario_version: result.scenario_version,
        seed: result.seed,
        final_state,
        events,
    })
}

fn trim_process_error(primary: &str, secondary: &str) -> String {
    let combined = format!("{}\n{}", primary.trim(), secondary.trim());
    combined.chars().take(4_000).collect()
}

fn wilson_interval(wins: usize, games: usize) -> (f64, f64) {
    if games == 0 {
        return (0.0, 1.0);
    }
    let z = 1.959_963_984_540_054;
    let n = games as f64;
    let p = wins.min(games) as f64 / n;
    let z2 = z * z;
    let denominator = 1.0 + z2 / n;
    let center = (p + z2 / (2.0 * n)) / denominator;
    let margin = z * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt() / denominator;
    (
        (center - margin).clamp(0.0, 1.0),
        (center + margin).clamp(0.0, 1.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use metagene_core::{Card, CardFeatures, CardId, Color, DeckEntry};
    use std::collections::BTreeMap;

    #[test]
    fn parses_quiet_forge_results_and_missing_games() {
        let output = "Simulation mode\n\
Ai(1)-Seat 0 vs Ai(2)-Seat 1 - 3 games of Constructed seed 42\n\
Game Result: Game 1 ended in 12 ms. Ai(1)-Seat 0 has won!\n\
Game Result: Game 2 ended in a Draw! Took 20 ms.\n";
        let parsed = parse_forge_output(output, 2, 3);
        assert_eq!(parsed.completed, 2);
        assert_eq!(parsed.wins, vec![1, 0]);
        assert_eq!(parsed.draws, 1);
        assert_eq!(parsed.missing, 1);
        assert_eq!(parsed.duration_ms, 12);
    }

    #[test]
    fn parses_modern_and_legacy_java_versions() {
        assert_eq!(
            parse_java_major_version("openjdk 17.0.18 2026-01-20"),
            Some(17)
        );
        assert_eq!(
            parse_java_major_version("java version \"1.8.0_402\""),
            Some(8)
        );
        assert_eq!(parse_java_major_version("not installed"), None);
    }

    #[test]
    fn parses_prefixed_event_protocol_and_ignores_forge_noise() {
        let output = "Forge startup noise\n\
METAGENE_EVENT {\"event_schema_version\":1,\"sequence\":1,\"kind\":\"zone.changed\",\"forge_type\":\"GameEventCardChangeZone\",\"text\":\"Island: Hand -> Battlefield\",\"card\":\"Island\"}\n\
METAGENE_RESULT {\"protocol_version\":1,\"seed\":42,\"events\":1,\"outcome\":\"winner:Metagene-Seat-1\"}\n";
        let probe = parse_event_probe_output(output).unwrap();
        assert_eq!(probe.seed, 42);
        assert_eq!(probe.events.len(), 1);
        assert_eq!(probe.events[0].components["card"], "Island");
    }

    #[test]
    fn rejects_truncated_event_protocol() {
        let output = "METAGENE_EVENT {\"event_schema_version\":1,\"sequence\":1,\"kind\":\"zone.changed\",\"forge_type\":\"GameEventCardChangeZone\",\"text\":\"x\"}\n\
METAGENE_RESULT {\"protocol_version\":1,\"seed\":42,\"events\":2,\"outcome\":\"draw\"}\n";
        assert!(matches!(
            parse_event_probe_output(output),
            Err(ForgeError::InvalidEventProtocol(_))
        ));
    }

    #[test]
    fn bundled_event_adapter_matches_its_versioned_digest() {
        let mut hasher = Sha256::new();
        hasher.update(FORGE_ADAPTER_BYTES);
        assert_eq!(
            FORGE_ADAPTER_REVISION,
            format!("sha256:{:x}", hasher.finalize())
        );
    }

    #[test]
    fn parses_and_normalizes_parallel_lives_scenario_evidence() {
        let output = concat!(
            "METAGENE_EVENT {\"event_schema_version\":1,\"sequence\":1,\"kind\":\"zone.changed\",\"forge_type\":\"GameEventCardChangeZone\",\"text\":\"Mythos to stack\",\"card\":\"Mythos of Illuna (121)\",\"from\":\"ZoneView[player=p1, zoneType=Hand]\",\"to\":\"ZoneView[player=null, zoneType=Stack]\"}\n",
            "METAGENE_EVENT {\"event_schema_version\":1,\"sequence\":2,\"kind\":\"spell.cast\",\"forge_type\":\"GameEventSpellAbilityCast\",\"text\":\"cast Mythos\",\"targetDescription\":\"[Parallel Lives (124)]\"}\n",
            "METAGENE_EVENT {\"event_schema_version\":1,\"sequence\":3,\"kind\":\"zone.changed\",\"forge_type\":\"GameEventCardChangeZone\",\"text\":\"token one\",\"card\":\"Parallel Lives (124)\",\"from\":null,\"to\":\"ZoneView[player=p1, zoneType=Battlefield]\"}\n",
            "METAGENE_EVENT {\"event_schema_version\":1,\"sequence\":4,\"kind\":\"zone.changed\",\"forge_type\":\"GameEventCardChangeZone\",\"text\":\"token two\",\"card\":\"Parallel Lives (125)\",\"from\":null,\"to\":\"ZoneView[player=p1, zoneType=Battlefield]\"}\n",
            "METAGENE_EVENT {\"event_schema_version\":1,\"sequence\":5,\"kind\":\"zone.changed\",\"forge_type\":\"GameEventCardChangeZone\",\"text\":\"Mythos resolved\",\"card\":\"Mythos of Illuna (121)\",\"from\":\"ZoneView[player=null, zoneType=Stack]\",\"to\":\"ZoneView[player=p1, zoneType=Graveyard]\"}\n",
            "METAGENE_SCENARIO_RESULT {\"protocol_version\":1,\"scenario_id\":\"tokens.parallel-lives-copy\",\"scenario_version\":1,\"seed\":42,\"events\":5,\"final_state\":{\"battlefield\":{\"seat-1\":{\"cards\":{\"Parallel Lives\":{\"count\":3}},\"tokens\":{\"Parallel Lives\":{\"count\":2}}}},\"graveyard\":{\"seat-1\":{\"cards\":{\"Mythos of Illuna\":{\"count\":1}}}}}}\n"
        );
        let probe = parse_scenario_probe_output(output).unwrap();
        let suite = crate::conformance_contract::parse_scenario_suite(include_str!(
            "../../data/conformance/scenarios.schema1.json"
        ))
        .unwrap();
        let scenario = &suite.scenarios[0];
        let normalized = normalize_scenario_events(scenario, &probe).unwrap();
        assert_eq!(normalized.len(), 4);
        assert_eq!(normalized[1].kind, "replacement.applied");
        assert_eq!(normalized[2].payload["quantity"], 2);
        let report = evaluate_assertions(&scenario.assertions, &probe.final_state).unwrap();
        assert!(report.passed);
        let projected = project_events(&normalized, &scenario.event_projection).unwrap();
        assert_eq!(
            canonical_event_hash(&projected).unwrap(),
            "sha256:6075f701b27a244953d776b916e27592da13608e0bb6901419c0141e1c954d77"
        );
    }

    fn scenario_event(
        sequence: u64,
        kind: &str,
        text: &str,
        components: serde_json::Value,
    ) -> ForgeRecordedEvent {
        ForgeRecordedEvent {
            event_schema_version: 1,
            sequence,
            kind: kind.into(),
            forge_type: "test".into(),
            text: text.into(),
            components: serde_json::from_value(components).unwrap(),
        }
    }

    #[test]
    fn clone_layer_result_normalizes_to_production_hash() {
        let probe = ForgeScenarioProbe {
            protocol_version: 1,
            scenario_id: "layers.clone-copies-base-before-anthem".into(),
            scenario_version: 1,
            seed: 45,
            final_state: serde_json::json!({"objects":{"clone":{"card_name":"Grizzly Bears","base_power":2,"base_toughness":2,"effective_power":3,"effective_toughness":3}}}),
            events: vec![
                scenario_event(1, "spell.cast", "cast Clone", serde_json::json!({})),
                scenario_event(
                    2,
                    "zone.changed",
                    "copied creature entered",
                    serde_json::json!({"card":"Grizzly Bears (1)","from":"ZoneView[player=null, zoneType=Stack]","to":"ZoneView[player=p1, zoneType=Battlefield]"}),
                ),
            ],
        };
        let suite = crate::conformance_contract::parse_scenario_suite(include_str!(
            "../../data/conformance/scenarios.schema1.json"
        ))
        .unwrap();
        let scenario = &suite.scenarios[3];
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
            "sha256:9b4ffab3bc04d09989b657829fb1b403fdafd6d5ac16b0fa4b829c2b085b44da"
        );
    }

    #[test]
    fn targeted_lethal_damage_normalizes_to_shared_hash() {
        let probe = ForgeScenarioProbe {
            protocol_version: 1,
            scenario_id: "targeting.lethal-damage-state-based-action".into(),
            scenario_version: 1,
            seed: 46,
            final_state: serde_json::json!({"battlefield":{"seat-2":{"cards":{"Grizzly Bears":{"count":0}}}},"graveyard":{"seat-2":{"cards":{"Grizzly Bears":{"count":1}}},"seat-1":{"cards":{"Strangle":{"count":1}}}},"players":{"seat-2":{"life":20}}}),
            events: vec![
                scenario_event(
                    1,
                    "spell.cast",
                    "cast Strangle",
                    serde_json::json!({"targetDescription":"[Grizzly Bears (2)]"}),
                ),
                scenario_event(
                    2,
                    "damage.dealt",
                    "Strangle dealt 3 damage",
                    serde_json::json!({"card":"Grizzly Bears (2)"}),
                ),
                scenario_event(
                    3,
                    "zone.changed",
                    "spell resolved",
                    serde_json::json!({"card":"Strangle (1)","from":"ZoneView[player=null, zoneType=Stack]","to":"ZoneView[player=p1, zoneType=Graveyard]"}),
                ),
                scenario_event(
                    4,
                    "card.destroyed",
                    "destroyed",
                    serde_json::json!({"card":"Grizzly Bears (2)"}),
                ),
                scenario_event(
                    5,
                    "zone.changed",
                    "creature died",
                    serde_json::json!({"card":"Grizzly Bears (2)","from":"ZoneView[player=p2, zoneType=Battlefield]","to":"ZoneView[player=p2, zoneType=Graveyard]"}),
                ),
            ],
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
        let probe = ForgeScenarioProbe {
            protocol_version: 1,
            scenario_id: "triggers.creature-etb-gains-life".into(),
            scenario_version: 1,
            seed: 47,
            final_state: serde_json::json!({"players":{"seat-1":{"life":21}},"battlefield":{"seat-1":{"cards":{"Ajani's Welcome":{"count":1},"Grizzly Bears":{"count":1}}}}}),
            events: vec![
                scenario_event(1, "spell.cast", "cast Grizzly Bears", serde_json::json!({})),
                scenario_event(
                    2,
                    "zone.changed",
                    "entered",
                    serde_json::json!({"card":"Grizzly Bears (1)","from":"zoneType=Stack","to":"zoneType=Battlefield"}),
                ),
                scenario_event(
                    3,
                    "spell.cast",
                    "triggered Whenever a creature you control enters, you gain 1 life. Grizzly Bears",
                    serde_json::json!({}),
                ),
                scenario_event(
                    4,
                    "life.changed",
                    "Seat-1's lives changed: 20 -> 21",
                    serde_json::json!({}),
                ),
            ],
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
        let probe = ForgeScenarioProbe {
            protocol_version: 1,
            scenario_id: "combat.unblocked-creature-damages-player".into(),
            scenario_version: 1,
            seed: 48,
            final_state: serde_json::json!({"players":{"seat-2":{"life":18}},"battlefield":{"seat-1":{"cards":{"Grizzly Bears":{"count":1}}}},"objects":{"bears":{"tapped":true,"declared_attacker":true}}}),
            events: vec![
                scenario_event(
                    1,
                    "damage.dealt",
                    "Metagene-Seat-2 took 2 combat damage from Grizzly Bears (121)",
                    serde_json::json!({}),
                ),
                scenario_event(
                    2,
                    "life.changed",
                    "Metagene-Seat-2's lives changed: 20 -> 18",
                    serde_json::json!({}),
                ),
            ],
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
        let probe = ForgeScenarioProbe {
            protocol_version: 1,
            scenario_id: "multiplayer.combat-eliminates-one-opponent".into(),
            scenario_version: 1,
            seed: 49,
            final_state: serde_json::json!({"players":{"seat-2":{"life":20,"lost":false},"seat-3":{"life":0,"lost":true}},"battlefield":{"seat-3":{"cards":{"Forest":{"count":0}}}},"objects":{"bears":{"tapped":true,"declared_attacker":true,"defender":3}}}),
            events: vec![
                scenario_event(
                    1,
                    "damage.dealt",
                    "Metagene-Seat-3 took 2 combat damage from Grizzly Bears (181)",
                    serde_json::json!({}),
                ),
                scenario_event(
                    2,
                    "life.changed",
                    "Metagene-Seat-3's lives changed: 2 -> 0",
                    serde_json::json!({}),
                ),
            ],
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
    fn oracle_of_the_alpha_conjure_normalizes_supported_forge_evidence() {
        let mut events = vec![
            scenario_event(
                1,
                "spell.cast",
                "Metagene-Seat-1 cast Oracle of the Alpha",
                serde_json::json!({}),
            ),
            scenario_event(
                2,
                "zone.changed",
                "Oracle entered",
                serde_json::json!({"card":"Oracle of the Alpha (1)","from":"zoneType=Stack","to":"zoneType=Battlefield"}),
            ),
            scenario_event(
                3,
                "spell.cast",
                "Metagene-Seat-1 triggered conjure the Power Nine into your library",
                serde_json::json!({}),
            ),
        ];
        for (index, name) in POWER_NINE.iter().enumerate() {
            events.push(scenario_event(
                index as u64 + 4,
                "zone.changed",
                "conjured card entered library",
                serde_json::json!({"card":format!("{name} (1)"),"to":"zoneType=Library"}),
            ));
        }
        let probe = ForgeScenarioProbe {
            protocol_version: 1,
            scenario_id: "arena.oracle-of-the-alpha-conjures-power-nine".into(),
            scenario_version: 1,
            seed: 50,
            final_state: serde_json::json!({"battlefield":{"seat-1":{"cards":{"Oracle of the Alpha":{"count":1}}}},"library":{"seat-1":{"total_count":9,"cards":{"Ancestral Recall":{"count":1},"Black Lotus":{"count":1},"Mox Emerald":{"count":1},"Mox Jet":{"count":1},"Mox Pearl":{"count":1},"Mox Ruby":{"count":1},"Mox Sapphire":{"count":1},"Time Walk":{"count":1},"Timetwister":{"count":1}}}}}),
            events,
        };
        let suite = crate::conformance_contract::parse_scenario_suite(include_str!(
            "../../data/conformance/scenarios.schema1.json"
        ))
        .unwrap();
        let scenario = &suite.scenarios[8];
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
        assert_eq!(projected[3].kind, "card.conjured");
        assert_eq!(projected[3].payload["quantity"], 9);
        assert_eq!(
            canonical_event_hash(&projected).unwrap(),
            "sha256:9a0ff2666b36dcde148fbdf4d7dd32eb9003fe09b8897fb034bf1dab8933d691"
        );
    }

    #[test]
    fn rebalanced_falcon_abomination_flash_normalizes_forge_timing() {
        let probe = ForgeScenarioProbe {
            protocol_version: 1,
            scenario_id: "arena.rebalanced-falcon-abomination-has-flash".into(),
            scenario_version: 1,
            seed: 51,
            final_state: serde_json::json!({"timing":{"original_legal":false,"rebalanced_legal":true},"battlefield":{"seat-1":{"cards":{"A-Falcon Abomination":{"count":1}}}}}),
            events: vec![
                scenario_event(
                    1,
                    "spell.cast",
                    "Metagene-Seat-1 cast A-Falcon Abomination",
                    serde_json::json!({}),
                ),
                scenario_event(
                    2,
                    "zone.changed",
                    "A-Falcon Abomination entered",
                    serde_json::json!({"card":"A-Falcon Abomination (1)","from":"zoneType=Stack","to":"zoneType=Battlefield"}),
                ),
            ],
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
    fn exports_constructed_and_commander_dck_sections() {
        let card = Card {
            id: CardId("plains-id".into()),
            arena_id: Some(1),
            platforms: vec!["arena".into()],
            name: "Plains".into(),
            set_code: "dmu".into(),
            collector_number: "250".into(),
            type_line: "Basic Land — Plains".into(),
            oracle_text: String::new(),
            colors: Vec::new(),
            color_identity: vec![Color::White],
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
            name: "Test".into(),
            command_zone: vec![DeckEntry {
                card_id: CardId("plains-id".into()),
                quantity: 1,
            }],
            mainboard: vec![DeckEntry {
                card_id: CardId("plains-id".into()),
                quantity: 99,
            }],
            sideboard: Vec::new(),
        };
        let exported = export_forge_deck(&deck, &pool, true, "Seat 0").unwrap();
        assert!(exported.contains("Deck Type=Commander"));
        assert!(exported.contains("[Commander]\n1 Plains"));
        assert!(exported.contains("[Main]\n99 Plains"));
    }
}
