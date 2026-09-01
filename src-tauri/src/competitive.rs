use crate::{conformance, forge, metagame_sources, simulator};
use metagene_core::{
    AbilityKind, CancellationToken, Card, CardPool, ClauseClassification, ClauseCriticality, Deck,
    DeckEntry,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::Path;

pub const FIELD_SCHEMA_VERSION: u32 = 2;
pub const CAPABILITY_SCHEMA_VERSION: u32 = 2;
pub const ROUTE_SCHEMA_VERSION: u32 = 2;
pub const CAPABILITY_POLICY_REVISION: &str = "historic-rules-capability-v3";
pub const RULES_IR_REVISION: &str = "rules-ir-exact-state-v2";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct FieldSelectionPolicy {
    pub minimum_constructed_archetypes: usize,
    pub maximum_constructed_archetypes: usize,
    pub target_observed_weight: f64,
    pub similarity_threshold: f64,
    pub commander_opponents: usize,
}

impl Default for FieldSelectionPolicy {
    fn default() -> Self {
        Self {
            minimum_constructed_archetypes: 8,
            maximum_constructed_archetypes: 16,
            target_observed_weight: 0.90,
            similarity_threshold: 0.72,
            commander_opponents: 5,
        }
    }
}

#[derive(
    Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum StrategyArchetype {
    Aggro,
    Control,
    Combo,
    Graveyard,
    #[default]
    Midrange,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArchetypeCluster {
    pub id: String,
    pub label: String,
    pub strategy: StrategyArchetype,
    pub representative: Deck,
    pub representative_fingerprint: String,
    pub member_fingerprints: Vec<String>,
    pub occurrences: usize,
    pub observed_weight: f64,
    pub normalized_selected_weight: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FieldAudit {
    pub schema_version: u32,
    pub source_tier: String,
    pub competitive_claims_allowed: bool,
    pub observed_weight_covered: f64,
    pub submitted_decks: usize,
    pub accepted_decks: usize,
    pub selected_archetypes: usize,
    pub exact_duplicates_merged: usize,
    pub near_duplicates_clustered: usize,
    pub strategy_coverage: Vec<StrategyArchetype>,
    pub quarantined: Vec<metagame_sources::QuarantinedDeck>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetagameFieldRevision {
    pub schema_version: u32,
    pub revision_id: String,
    pub platform: String,
    pub format: String,
    pub commander: bool,
    pub provenance: metagame_sources::SourceProvenance,
    pub clusters: Vec<ArchetypeCluster>,
    pub decks: Vec<Deck>,
    pub weights: Vec<f64>,
    pub audit: FieldAudit,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum MechanicCapabilityState {
    #[default]
    Unsupported,
    Parsed,
    Executable,
    ScenarioVerified,
    Disputed,
}

impl MechanicCapabilityState {
    pub fn trusted(self) -> bool {
        matches!(self, Self::Executable | Self::ScenarioVerified)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MechanicCapability {
    pub card_id: String,
    pub card_name: String,
    pub mode_name: String,
    pub subject_id: String,
    pub action: String,
    pub critical: bool,
    pub criticality: ClauseCriticality,
    pub parser_confidence: f32,
    pub high_risk: bool,
    pub ai_decision_covered: bool,
    pub state: MechanicCapabilityState,
    pub evidence: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeckCapabilityReport {
    pub schema_version: u32,
    pub engine: String,
    pub deck_name: String,
    pub card_copies: usize,
    pub trusted_card_copies: usize,
    pub trusted_copy_ratio: f64,
    pub critical_actions: usize,
    pub trusted_critical_actions: usize,
    pub critical_coverage: f64,
    pub oracle_accounted_rate: f64,
    pub structurally_parsed_rate: f64,
    pub engine_executable_rate: f64,
    pub scenario_verified_rate: f64,
    pub ai_decision_coverage: f64,
    pub unresolved_clauses: Vec<String>,
    pub capabilities: Vec<MechanicCapability>,
    pub blockers: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EngineRoute {
    pub opponent_index: usize,
    pub opponent_name: String,
    pub weight: f64,
    pub engine: Option<String>,
    pub eligible: bool,
    pub reasons: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceTier {
    #[default]
    Screened,
    CoverageChecked,
    Provisional,
    FieldTested,
    SingleEngineValidated,
    MixedRouteValidated,
    CrossEngineVerified,
    Improved,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidationClaim {
    pub tier: EvidenceTier,
    pub promotable: bool,
    pub label: String,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EngineRoutePlan {
    pub schema_version: u32,
    pub capability_policy_revision: String,
    pub rules_ir_revision: String,
    pub plan_id: String,
    pub candidate: DeckCapabilityReport,
    pub candidate_by_engine: Vec<DeckCapabilityReport>,
    pub opponent_reports: Vec<DeckCapabilityReport>,
    pub routes: Vec<EngineRoute>,
    pub field_weighted_coverage: f64,
    pub candidate_critical_coverage: f64,
    pub locked: bool,
    pub claim: ValidationClaim,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EngineCapabilityManifest {
    pub schema_version: u32,
    pub capability_policy_revision: String,
    pub rules_ir_revision: String,
    pub engine: String,
    pub engine_revision: String,
    pub card_data_revision: String,
    pub generated_at_unix: u64,
    pub cards: usize,
    pub entries: Vec<MechanicCapability>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConformanceWorkItem {
    pub card_id: String,
    pub card_name: String,
    pub mode_name: String,
    pub subject_id: String,
    pub clause: String,
    pub criticality: ClauseCriticality,
    pub high_risk: bool,
    pub field_weight: f64,
    pub candidate_present: bool,
    pub missing_engines: Vec<String>,
    pub needs_ai_decision_scenario: bool,
    pub priority: f64,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FormatCapabilityAudit {
    pub schema_version: u32,
    pub capability_policy_revision: String,
    pub rules_ir_revision: String,
    pub platform: String,
    pub format: String,
    pub catalog_cards: usize,
    pub field_weighted_executable_coverage: f64,
    pub candidate_critical_coverage: f64,
    pub work_items: Vec<ConformanceWorkItem>,
    pub blockers: Vec<String>,
}

pub fn audit_format_capability(
    data_dir: &Path,
    platform: &str,
    format: &str,
    pool: &CardPool,
    field: Option<&MetagameFieldRevision>,
    candidate: Option<&Deck>,
) -> FormatCapabilityAudit {
    let corpus = Deck {
        name: format!("{platform} {format} capability ledger"),
        command_zone: Vec::new(),
        mainboard: pool
            .cards
            .iter()
            .map(|card| DeckEntry {
                card_id: card.id.clone(),
                quantity: 1,
            })
            .collect(),
        sideboard: Vec::new(),
    };
    let hexdek = audit_deck_capability(data_dir, "hexdek", &corpus, pool);
    let forge = audit_deck_capability(data_dir, "forge", &corpus, pool);
    let candidate_ids = candidate
        .into_iter()
        .flat_map(|deck| {
            deck.command_zone
                .iter()
                .chain(&deck.mainboard)
                .chain(&deck.sideboard)
        })
        .map(|entry| entry.card_id.0.clone())
        .collect::<BTreeSet<_>>();
    let mut field_weights = BTreeMap::<String, f64>::new();
    if let Some(field) = field {
        for (deck, weight) in field.decks.iter().zip(&field.weights) {
            let copies = deck
                .command_zone
                .iter()
                .chain(&deck.mainboard)
                .chain(&deck.sideboard)
                .map(|entry| f64::from(entry.quantity))
                .sum::<f64>()
                .max(1.0);
            for entry in deck
                .command_zone
                .iter()
                .chain(&deck.mainboard)
                .chain(&deck.sideboard)
            {
                *field_weights.entry(entry.card_id.0.clone()).or_default() +=
                    weight * f64::from(entry.quantity) / copies;
            }
        }
    }
    let forge_by_subject = forge
        .capabilities
        .iter()
        .map(|capability| (capability.subject_id.as_str(), capability))
        .collect::<HashMap<_, _>>();
    let mut executable_weight = 0.0;
    let mut total_weight = 0.0;
    let mut candidate_critical = 0usize;
    let mut candidate_trusted = 0usize;
    let mut work_items = Vec::new();
    for capability in &hexdek.capabilities {
        let forge_capability = forge_by_subject
            .get(capability.subject_id.as_str())
            .copied();
        let candidate_present = candidate_ids.contains(&capability.card_id);
        let trusted = capability.state.trusted()
            || forge_capability.is_some_and(|entry| entry.state.trusted());
        let field_weight = field_weights
            .get(&capability.card_id)
            .copied()
            .unwrap_or(0.0);
        total_weight += field_weight;
        if trusted {
            executable_weight += field_weight;
        }
        if candidate_present && capability.critical {
            candidate_critical += 1;
            candidate_trusted += usize::from(trusted);
        }
        let high_risk_verified = !capability.high_risk
            || capability.state == MechanicCapabilityState::ScenarioVerified
            || forge_capability
                .is_some_and(|entry| entry.state == MechanicCapabilityState::ScenarioVerified);
        let ai_covered = capability.ai_decision_covered
            || forge_capability.is_some_and(|entry| entry.ai_decision_covered);
        if trusted && high_risk_verified && (!candidate_present || ai_covered) {
            continue;
        }
        let mut missing_engines = Vec::new();
        if !capability.state.trusted() {
            missing_engines.push("hexdek".into());
        }
        if !forge_capability.is_some_and(|entry| entry.state.trusted()) {
            missing_engines.push("forge".into());
        }
        let priority = field_weight * 1000.0
            + f64::from(candidate_present) * 500.0
            + f64::from(capability.critical) * 200.0
            + f64::from(capability.high_risk) * 150.0
            + f64::from(!ai_covered) * 25.0;
        work_items.push(ConformanceWorkItem {
            card_id: capability.card_id.clone(),
            card_name: capability.card_name.clone(),
            mode_name: capability.mode_name.clone(),
            subject_id: capability.subject_id.clone(),
            clause: capability.action.clone(),
            criticality: capability.criticality,
            high_risk: capability.high_risk,
            field_weight,
            candidate_present,
            missing_engines,
            needs_ai_decision_scenario: candidate_present && !ai_covered,
            priority,
            reason: if capability.high_risk && !high_risk_verified {
                "High-risk Rules IR operation requires a current action-scoped scenario.".into()
            } else if candidate_present && !ai_covered {
                "Primary candidate action requires deterministic AI decision evidence.".into()
            } else {
                "No current engine execution manifest covers this exact Rules IR subject.".into()
            },
        });
    }
    work_items.sort_by(|left, right| {
        right
            .priority
            .total_cmp(&left.priority)
            .then_with(|| left.subject_id.cmp(&right.subject_id))
    });
    let field_coverage = if total_weight > 0.0 {
        executable_weight / total_weight
    } else {
        0.0
    };
    let candidate_coverage = if candidate_critical > 0 {
        candidate_trusted as f64 / candidate_critical as f64
    } else {
        1.0
    };
    let mut blockers = Vec::new();
    if field.is_none() {
        blockers.push(
            "No tournament field was supplied; field-weighted coverage is unavailable.".into(),
        );
    }
    if field.is_some() && field_coverage < 0.95 {
        blockers.push(format!(
            "Weighted executable coverage is {:.1}%; 95% is required.",
            field_coverage * 100.0
        ));
    }
    if candidate_coverage < 1.0 {
        blockers.push(format!(
            "Candidate critical Rules IR coverage is {:.1}%; 100% is required.",
            candidate_coverage * 100.0
        ));
    }
    FormatCapabilityAudit {
        schema_version: CAPABILITY_SCHEMA_VERSION,
        capability_policy_revision: CAPABILITY_POLICY_REVISION.into(),
        rules_ir_revision: RULES_IR_REVISION.into(),
        platform: platform.into(),
        format: format.into(),
        catalog_cards: pool.cards.len(),
        field_weighted_executable_coverage: field_coverage,
        candidate_critical_coverage: candidate_coverage,
        work_items,
        blockers,
    }
}

pub fn rebuild_engine_capability_manifest(
    data_dir: &Path,
    engine: &str,
    pool: &CardPool,
) -> Result<EngineCapabilityManifest, String> {
    if !matches!(engine, "hexdek" | "forge") {
        return Err("Capability manifests support HexDek or Forge.".into());
    }
    let deck = Deck {
        name: format!("{engine} capability corpus"),
        command_zone: Vec::new(),
        mainboard: pool
            .cards
            .iter()
            .map(|card| DeckEntry {
                card_id: card.id.clone(),
                quantity: 1,
            })
            .collect(),
        sideboard: Vec::new(),
    };
    let report = audit_deck_capability(data_dir, engine, &deck, pool);
    let status = if engine == "forge" {
        forge::status(data_dir)
    } else {
        simulator::statuses(data_dir)
            .into_iter()
            .find(|status| status.id == "hexdek")
            .unwrap_or_else(|| simulator::SimulatorStatus {
                id: "hexdek".into(),
                name: "HexDek".into(),
                installed: false,
                data_ready: false,
                executable: String::new(),
                supported_formats: Vec::new(),
                note: "not installed".into(),
            })
    };
    let card_data_revision = short_hash_bytes(&serde_json::to_vec(pool).unwrap_or_default());
    let manifest = EngineCapabilityManifest {
        schema_version: CAPABILITY_SCHEMA_VERSION,
        capability_policy_revision: CAPABILITY_POLICY_REVISION.into(),
        rules_ir_revision: RULES_IR_REVISION.into(),
        engine: engine.into(),
        engine_revision: short_hash_bytes(status.note.as_bytes()),
        card_data_revision,
        generated_at_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        cards: pool.cards.len(),
        entries: report.capabilities,
    };
    let directory = data_dir.join("capability-manifests");
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    fs::write(
        directory.join(format!("{engine}-capability-v2.json")),
        serde_json::to_vec(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    Ok(manifest)
}

#[derive(Clone)]
struct ClusterAccumulator {
    label: String,
    strategy: StrategyArchetype,
    representative: Deck,
    representative_fingerprint: String,
    representative_occurrences: usize,
    members: Vec<String>,
    occurrences: usize,
    weight: f64,
    vector: BTreeMap<String, u16>,
}

pub fn select_competitive_field(
    report: &metagame_sources::MetagameAcquisitionReport,
    pool: &CardPool,
    policy: &FieldSelectionPolicy,
) -> MetagameFieldRevision {
    let minimum = policy.minimum_constructed_archetypes.clamp(1, 32);
    let maximum = policy.maximum_constructed_archetypes.clamp(minimum, 32);
    let similarity_threshold = policy.similarity_threshold.clamp(0.25, 0.98);
    let mut source = report.accepted.iter().collect::<Vec<_>>();
    source.sort_by(|left, right| {
        right
            .occurrences
            .cmp(&left.occurrences)
            .then_with(|| left.fingerprint.cmp(&right.fingerprint))
    });

    let exact_occurrences = source.iter().map(|entry| entry.occurrences).sum::<usize>();
    let exact_duplicates_merged = exact_occurrences.saturating_sub(source.len());
    let mut clusters: Vec<ClusterAccumulator> = Vec::new();
    for entry in source {
        let vector = nonland_copy_vector(&entry.deck, pool);
        let strategy = classify_strategy(&entry.deck, pool);
        let commander_key = commander_identity_key(&entry.deck);
        let compatible = |cluster: &ClusterAccumulator| {
            (!report.commander || commander_identity_key(&cluster.representative) == commander_key)
                && weighted_jaccard(&cluster.vector, &vector) >= similarity_threshold
        };
        if let Some(cluster) = clusters.iter_mut().find(|cluster| compatible(cluster)) {
            cluster.members.push(entry.fingerprint.clone());
            cluster.occurrences = cluster.occurrences.saturating_add(entry.occurrences);
            cluster.weight += entry.weight;
            if entry.occurrences > cluster.representative_occurrences {
                cluster.label.clone_from(&entry.archetype);
                cluster.strategy = strategy;
                cluster.representative.clone_from(&entry.deck);
                cluster
                    .representative_fingerprint
                    .clone_from(&entry.fingerprint);
                cluster.representative_occurrences = entry.occurrences;
                cluster.vector = vector;
            }
        } else {
            clusters.push(ClusterAccumulator {
                label: entry.archetype.clone(),
                strategy,
                representative: entry.deck.clone(),
                representative_fingerprint: entry.fingerprint.clone(),
                representative_occurrences: entry.occurrences,
                members: vec![entry.fingerprint.clone()],
                occurrences: entry.occurrences,
                weight: entry.weight,
                vector,
            });
        }
    }
    clusters.sort_by(|left, right| {
        right.weight.total_cmp(&left.weight).then_with(|| {
            left.representative_fingerprint
                .cmp(&right.representative_fingerprint)
        })
    });
    let near_duplicates_clustered = report.accepted.len().saturating_sub(clusters.len());

    let target_count = if report.commander {
        policy.commander_opponents.clamp(1, 5)
    } else {
        maximum
    };
    let mut selected = Vec::<usize>::new();
    if !report.commander {
        for strategy in [
            StrategyArchetype::Aggro,
            StrategyArchetype::Midrange,
            StrategyArchetype::Control,
            StrategyArchetype::Combo,
            StrategyArchetype::Graveyard,
        ] {
            if let Some((index, _)) = clusters
                .iter()
                .enumerate()
                .find(|(index, cluster)| cluster.strategy == strategy && !selected.contains(index))
            {
                selected.push(index);
            }
        }
    }
    for index in 0..clusters.len() {
        if selected.len() >= target_count {
            break;
        }
        let covered = selected
            .iter()
            .map(|index| clusters[*index].weight)
            .sum::<f64>();
        let enough_count = report.commander || selected.len() >= minimum;
        if enough_count && covered >= policy.target_observed_weight.clamp(0.50, 1.0) {
            break;
        }
        if !selected.contains(&index) {
            selected.push(index);
        }
    }
    selected.sort_by(|left, right| clusters[*right].weight.total_cmp(&clusters[*left].weight));
    selected.truncate(target_count);
    let observed_weight_covered = selected
        .iter()
        .map(|index| clusters[*index].weight)
        .sum::<f64>();
    let selected_weight = observed_weight_covered.max(f64::EPSILON);
    let selected_clusters = selected
        .iter()
        .map(|index| {
            let cluster = &clusters[*index];
            ArchetypeCluster {
                id: short_hash(&format!(
                    "{}:{}:{}",
                    report.provenance.adapter,
                    cluster.representative_fingerprint,
                    cluster.members.join(",")
                )),
                label: cluster.label.clone(),
                strategy: cluster.strategy,
                representative: cluster.representative.clone(),
                representative_fingerprint: cluster.representative_fingerprint.clone(),
                member_fingerprints: cluster.members.clone(),
                occurrences: cluster.occurrences,
                observed_weight: cluster.weight,
                normalized_selected_weight: cluster.weight / selected_weight,
            }
        })
        .collect::<Vec<_>>();
    let source_tier = if report.provenance.adapter.starts_with("spicerack")
        || report.provenance.adapter.starts_with("topdeck")
    {
        "tournament"
    } else {
        "public_list_fallback"
    };
    let competitive_claims_allowed = source_tier == "tournament";
    let mut warnings = Vec::new();
    if !competitive_claims_allowed {
        warnings.push(
            "Public-list fallback fields are guidance only and cannot promote a deck to Improved."
                .into(),
        );
    }
    if !report.commander && selected_clusters.len() < minimum {
        warnings.push(format!(
            "Only {} distinct archetype clusters were available; {minimum} are required for a complete constructed field.",
            selected_clusters.len()
        ));
    }
    if observed_weight_covered + f64::EPSILON < policy.target_observed_weight.clamp(0.50, 1.0) {
        warnings.push(format!(
            "Selected representatives cover {:.1}% of observed weight; the target is {:.1}%.",
            observed_weight_covered * 100.0,
            policy.target_observed_weight * 100.0
        ));
    }
    let strategy_coverage = selected_clusters
        .iter()
        .map(|cluster| cluster.strategy)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let decks = selected_clusters
        .iter()
        .map(|cluster| cluster.representative.clone())
        .collect::<Vec<_>>();
    let weights = selected_clusters
        .iter()
        .map(|cluster| cluster.normalized_selected_weight)
        .collect::<Vec<_>>();
    let revision_material = serde_json::to_vec(&(
        FIELD_SCHEMA_VERSION,
        &report.provenance,
        selected_clusters
            .iter()
            .map(|cluster| (&cluster.representative_fingerprint, cluster.observed_weight))
            .collect::<Vec<_>>(),
    ))
    .unwrap_or_default();
    MetagameFieldRevision {
        schema_version: FIELD_SCHEMA_VERSION,
        revision_id: short_hash_bytes(&revision_material),
        platform: report.platform.clone(),
        format: report.format.clone(),
        commander: report.commander,
        provenance: report.provenance.clone(),
        clusters: selected_clusters,
        decks,
        weights,
        audit: FieldAudit {
            schema_version: FIELD_SCHEMA_VERSION,
            source_tier: source_tier.into(),
            competitive_claims_allowed,
            observed_weight_covered,
            submitted_decks: report.provenance.submitted_deck_count,
            accepted_decks: report.provenance.accepted_deck_count,
            selected_archetypes: selected.len(),
            exact_duplicates_merged,
            near_duplicates_clustered,
            strategy_coverage,
            quarantined: report.quarantined.clone(),
            warnings,
        },
    }
}

pub fn audit_deck_capability(
    data_dir: &Path,
    engine: &str,
    deck: &Deck,
    pool: &CardPool,
) -> DeckCapabilityReport {
    let cards = deck_cards(deck, pool);
    let names = cards
        .iter()
        .map(|(card, _)| card.name.clone())
        .collect::<Vec<_>>();
    let conformance = conformance::audit_cards(data_dir, &names).ok();
    let conformance_by_name = conformance
        .as_ref()
        .map(|report| {
            report
                .cards
                .iter()
                .map(|card| (normalize_name(&card.name), card))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let mut coverage_by_name = HashMap::new();
    let mut engine_ready = false;
    if engine == "hexdek" {
        engine_ready = simulator::statuses(data_dir)
            .into_iter()
            .any(|status| status.id == "hexdek" && status.installed && status.data_ready);
        if engine_ready
            && let Ok(inputs) = simulator::coverage_inputs_from_decks(&[deck], pool)
            && let Ok(report) = simulator::audit_hexdek_coverage(data_dir, &inputs)
            && let Some(deck_report) = report.decks.first()
        {
            for issue in &deck_report.issues {
                coverage_by_name.insert(normalize_name(&issue.name), issue.status.clone());
            }
        }
    } else if engine == "forge" {
        let status = forge::status(data_dir);
        engine_ready = status.installed && status.data_ready;
    }

    let mut capabilities = Vec::new();
    let mut trusted_card_copies = 0usize;
    let mut critical_actions = 0usize;
    let mut trusted_critical_actions = 0usize;
    let mut oracle_subjects = 0usize;
    let mut parsed_oracle_subjects = 0usize;
    let mut executable_oracle_subjects = 0usize;
    let mut verified_oracle_subjects = 0usize;
    let mut ai_covered_oracle_subjects = 0usize;
    let mut unresolved_clauses = Vec::new();
    let mut blockers = Vec::new();
    for (card, quantity) in &cards {
        let conformance_card = conformance_by_name
            .get(&normalize_name(&card.name))
            .copied();
        let card_state = capability_state(
            engine,
            engine_ready,
            coverage_by_name
                .get(&normalize_name(&card.name))
                .map(String::as_str),
            conformance_card,
            card,
        );
        let subjects = capability_subjects(card);
        let mut card_trusted = true;
        for subject in subjects {
            let scoped_claim = conformance_card.and_then(|card| {
                card.claims
                    .iter()
                    .find(|claim| claim.subject_id == subject.subject_id)
            });
            let subject_state =
                capability_subject_state(engine, card_state, &subject, scoped_claim);
            let ai_decision_covered = subject.ai_decision_covered
                || scoped_claim.is_some_and(|claim| claim.ai_decision_covered);
            if subject.oracle_clause {
                oracle_subjects += 1;
                if subject.classification != ClauseClassification::UnresolvedGameplayResidual
                    && subject.parser_confidence >= 1.0
                {
                    parsed_oracle_subjects += 1;
                } else {
                    unresolved_clauses.push(format!("{}: {}", card.name, subject.action));
                }
                if subject_state.trusted() {
                    executable_oracle_subjects += 1;
                }
                if subject_state == MechanicCapabilityState::ScenarioVerified {
                    verified_oracle_subjects += 1;
                }
                if ai_decision_covered {
                    ai_covered_oracle_subjects += 1;
                }
            }
            if subject.criticality.blocks_claim() {
                critical_actions += 1;
                if subject_state.trusted() {
                    trusted_critical_actions += 1;
                } else {
                    card_trusted = false;
                }
            }
            let mut evidence = Vec::new();
            if !engine_ready {
                evidence.push(format!("{engine} is not installed and ready"));
            }
            if let Some(status) = coverage_by_name.get(&normalize_name(&card.name)) {
                evidence.push(format!("HexDek parser status: {status}"));
            }
            if let Some(card) = conformance_card {
                evidence.push(format!("Conformance: {}", card.classification));
                evidence.extend(card.scenarios.iter().cloned());
            }
            if let Some(claim) = scoped_claim {
                evidence.push(format!("Scoped claim: {}", claim.classification));
                evidence.extend(claim.scenarios.iter().cloned());
            }
            if subject.criticality.blocks_claim() && !subject_state.trusted() {
                blockers.push(format!(
                    "{} · {} is {} in {engine}",
                    card.name,
                    subject.action,
                    state_label(subject_state)
                ));
            }
            capabilities.push(MechanicCapability {
                card_id: card.id.0.clone(),
                card_name: card.name.clone(),
                mode_name: subject.mode_name,
                subject_id: subject.subject_id,
                action: subject.action,
                critical: subject.criticality.blocks_claim(),
                criticality: subject.criticality,
                parser_confidence: subject.parser_confidence,
                high_risk: subject.high_risk,
                ai_decision_covered,
                state: subject_state,
                evidence,
            });
        }
        if card_trusted {
            trusted_card_copies = trusted_card_copies.saturating_add(*quantity);
        }
    }
    blockers.sort();
    blockers.dedup();
    unresolved_clauses.sort();
    unresolved_clauses.dedup();
    let card_copies = cards.iter().map(|(_, quantity)| *quantity).sum::<usize>();
    let oracle_denominator = oracle_subjects.max(1) as f64;
    DeckCapabilityReport {
        schema_version: CAPABILITY_SCHEMA_VERSION,
        engine: engine.into(),
        deck_name: deck.name.clone(),
        card_copies,
        trusted_card_copies,
        trusted_copy_ratio: trusted_card_copies as f64 / card_copies.max(1) as f64,
        critical_actions,
        trusted_critical_actions,
        critical_coverage: trusted_critical_actions as f64 / critical_actions.max(1) as f64,
        oracle_accounted_rate: 1.0,
        structurally_parsed_rate: parsed_oracle_subjects as f64 / oracle_denominator,
        engine_executable_rate: executable_oracle_subjects as f64 / oracle_denominator,
        scenario_verified_rate: verified_oracle_subjects as f64 / oracle_denominator,
        ai_decision_coverage: ai_covered_oracle_subjects as f64 / oracle_denominator,
        unresolved_clauses,
        capabilities,
        blockers,
    }
}

pub fn build_engine_route_plan(
    data_dir: &Path,
    candidate: &Deck,
    opponents: &[Deck],
    weights: &[f64],
    pool: &CardPool,
    tournament_claims_allowed: bool,
) -> EngineRoutePlan {
    let candidate_hexdek = audit_deck_capability(data_dir, "hexdek", candidate, pool);
    let candidate_forge = audit_deck_capability(data_dir, "forge", candidate, pool);
    let normalized_weights = normalized_weights(opponents.len(), weights);
    let mut routes = Vec::new();
    let mut opponent_reports = Vec::new();
    let mut covered_weight = 0.0;
    let mut used_engines = BTreeSet::new();
    for (index, opponent) in opponents.iter().enumerate() {
        let hexdek = audit_deck_capability(data_dir, "hexdek", opponent, pool);
        let forge = audit_deck_capability(data_dir, "forge", opponent, pool);
        let hexdek_eligible = candidate_hexdek.critical_coverage >= 1.0
            && hexdek.critical_coverage >= 1.0
            && candidate_hexdek.trusted_copy_ratio >= 1.0
            && hexdek.trusted_copy_ratio >= 0.95;
        let forge_eligible = candidate_forge.critical_coverage >= 1.0
            && forge.critical_coverage >= 1.0
            && candidate_forge.trusted_copy_ratio >= 1.0
            && forge.trusted_copy_ratio >= 0.95;
        let engine = if hexdek_eligible {
            Some("hexdek".to_string())
        } else if forge_eligible {
            Some("forge".to_string())
        } else {
            None
        };
        if let Some(engine) = engine.as_ref() {
            used_engines.insert(engine.clone());
            covered_weight += normalized_weights[index];
        }
        let mut reasons = Vec::new();
        if engine.is_none() {
            reasons.push(format!(
                "HexDek candidate/opponent critical coverage {:.1}%/{:.1}%",
                candidate_hexdek.critical_coverage * 100.0,
                hexdek.critical_coverage * 100.0
            ));
            reasons.push(format!(
                "Forge candidate/opponent critical coverage {:.1}%/{:.1}%",
                candidate_forge.critical_coverage * 100.0,
                forge.critical_coverage * 100.0
            ));
        }
        opponent_reports.push(if engine.as_deref() == Some("forge") {
            forge
        } else {
            hexdek
        });
        routes.push(EngineRoute {
            opponent_index: index,
            opponent_name: opponent.name.clone(),
            weight: normalized_weights[index],
            eligible: engine.is_some(),
            engine,
            reasons,
        });
    }
    let candidate_coverage = candidate_hexdek
        .critical_coverage
        .max(candidate_forge.critical_coverage);
    let candidate_ai_coverage = candidate_hexdek
        .ai_decision_coverage
        .max(candidate_forge.ai_decision_coverage);
    let mut reasons = Vec::new();
    if !tournament_claims_allowed {
        reasons.push("The active field is not tournament-derived.".into());
    }
    if candidate_coverage < 1.0 {
        reasons.push(format!(
            "Candidate critical-action coverage is {:.1}%; 100% is required.",
            candidate_coverage * 100.0
        ));
    }
    if covered_weight + f64::EPSILON < 0.95 {
        reasons.push(format!(
            "Routed opponent coverage is {:.1}%; 95% is required.",
            covered_weight * 100.0
        ));
    }
    if candidate_ai_coverage < 1.0 {
        reasons.push(format!(
            "Candidate AI decision coverage is {:.1}%; primary strategy decisions require scoped evidence.",
            candidate_ai_coverage * 100.0
        ));
    }
    let promotable = tournament_claims_allowed
        && candidate_coverage >= 1.0
        && candidate_ai_coverage >= 1.0
        && covered_weight >= 0.95;
    let tier = if !promotable {
        EvidenceTier::Provisional
    } else if used_engines.len() > 1 {
        EvidenceTier::MixedRouteValidated
    } else {
        EvidenceTier::CoverageChecked
    };
    let claim = ValidationClaim {
        tier,
        promotable,
        label: if promotable {
            if used_engines.len() > 1 {
                "Coverage checked · mixed engine route"
            } else {
                "Coverage checked · single engine route"
            }
        } else {
            "Provisional · coverage or field provenance incomplete"
        }
        .into(),
        reasons,
    };
    let plan_material = serde_json::to_vec(&(
        ROUTE_SCHEMA_VERSION,
        candidate,
        opponents,
        &normalized_weights,
        &routes,
    ))
    .unwrap_or_default();
    EngineRoutePlan {
        schema_version: ROUTE_SCHEMA_VERSION,
        capability_policy_revision: CAPABILITY_POLICY_REVISION.into(),
        rules_ir_revision: RULES_IR_REVISION.into(),
        plan_id: short_hash_bytes(&plan_material),
        candidate: if candidate_hexdek.critical_coverage >= candidate_forge.critical_coverage {
            candidate_hexdek.clone()
        } else {
            candidate_forge.clone()
        },
        candidate_by_engine: vec![candidate_hexdek, candidate_forge],
        opponent_reports,
        routes,
        field_weighted_coverage: covered_weight,
        candidate_critical_coverage: candidate_coverage,
        locked: true,
        claim,
    }
}

/// Locks a single engine choice per opponent that every finalist can execute.
/// This is the route used for paired selection and holdout comparisons; it
/// prevents an incumbent and challenger from being judged by different rules.
pub fn build_comparison_engine_route_plan(
    data_dir: &Path,
    candidates: &[Deck],
    opponents: &[Deck],
    weights: &[f64],
    pool: &CardPool,
    tournament_claims_allowed: bool,
    commander: bool,
) -> EngineRoutePlan {
    let candidate_hexdek = candidates
        .iter()
        .map(|deck| audit_deck_capability(data_dir, "hexdek", deck, pool))
        .collect::<Vec<_>>();
    let candidate_forge = candidates
        .iter()
        .map(|deck| audit_deck_capability(data_dir, "forge", deck, pool))
        .collect::<Vec<_>>();
    let opponent_hexdek = opponents
        .iter()
        .map(|deck| audit_deck_capability(data_dir, "hexdek", deck, pool))
        .collect::<Vec<_>>();
    let opponent_forge = opponents
        .iter()
        .map(|deck| audit_deck_capability(data_dir, "forge", deck, pool))
        .collect::<Vec<_>>();
    let normalized_weights = normalized_weights(opponents.len(), weights);
    let candidates_ready = |reports: &[DeckCapabilityReport]| {
        !reports.is_empty()
            && reports
                .iter()
                .all(|report| report.critical_coverage >= 1.0 && report.trusted_copy_ratio >= 1.0)
    };
    let opponent_ready = |report: &DeckCapabilityReport| {
        report.critical_coverage >= 1.0 && report.trusted_copy_ratio >= 0.95
    };
    let hexdek_candidates_ready = candidates_ready(&candidate_hexdek);
    let forge_candidates_ready = candidates_ready(&candidate_forge);
    let commander_hexdek_ready =
        hexdek_candidates_ready && opponent_hexdek.iter().all(opponent_ready);
    let commander_forge_ready = forge_candidates_ready && opponent_forge.iter().all(opponent_ready);
    let commander_engine = if !commander {
        None
    } else if commander_hexdek_ready {
        Some("hexdek")
    } else if commander_forge_ready {
        Some("forge")
    } else {
        None
    };

    let mut covered_weight = 0.0;
    let mut used_engines = BTreeSet::new();
    let mut opponent_reports = Vec::new();
    let mut routes = Vec::new();
    for (index, opponent) in opponents.iter().enumerate() {
        let hexdek_eligible = hexdek_candidates_ready && opponent_ready(&opponent_hexdek[index]);
        let forge_eligible = forge_candidates_ready && opponent_ready(&opponent_forge[index]);
        let engine = if commander {
            commander_engine.map(str::to_string)
        } else if hexdek_eligible {
            Some("hexdek".to_string())
        } else if forge_eligible {
            Some("forge".to_string())
        } else {
            None
        };
        if let Some(engine) = engine.as_ref() {
            covered_weight += normalized_weights[index];
            used_engines.insert(engine.clone());
        }
        let mut reasons = Vec::new();
        if engine.is_none() {
            reasons.push(format!(
                "No common trusted engine supports all {} finalist(s) against this opponent",
                candidates.len()
            ));
            reasons.push(format!(
                "HexDek opponent critical/copy coverage {:.1}%/{:.1}%",
                opponent_hexdek[index].critical_coverage * 100.0,
                opponent_hexdek[index].trusted_copy_ratio * 100.0
            ));
            reasons.push(format!(
                "Forge opponent critical/copy coverage {:.1}%/{:.1}%",
                opponent_forge[index].critical_coverage * 100.0,
                opponent_forge[index].trusted_copy_ratio * 100.0
            ));
        }
        opponent_reports.push(if engine.as_deref() == Some("forge") {
            opponent_forge[index].clone()
        } else {
            opponent_hexdek[index].clone()
        });
        routes.push(EngineRoute {
            opponent_index: index,
            opponent_name: opponent.name.clone(),
            weight: normalized_weights[index],
            eligible: engine.is_some(),
            engine,
            reasons,
        });
    }

    let candidate_critical_coverage = candidate_hexdek
        .iter()
        .zip(&candidate_forge)
        .map(|(hexdek, forge)| hexdek.critical_coverage.max(forge.critical_coverage))
        .fold(1.0_f64, f64::min);
    let candidate_ai_coverage = candidate_hexdek
        .iter()
        .zip(&candidate_forge)
        .map(|(hexdek, forge)| hexdek.ai_decision_coverage.max(forge.ai_decision_coverage))
        .fold(1.0_f64, f64::min);
    let mut reasons = Vec::new();
    if !tournament_claims_allowed {
        reasons.push("The active field is not tournament-derived.".into());
    }
    if candidate_critical_coverage < 1.0 {
        reasons.push(format!(
            "At least one finalist has only {:.1}% critical-action coverage; 100% is required.",
            candidate_critical_coverage * 100.0
        ));
    }
    if covered_weight + f64::EPSILON < 0.95 {
        reasons.push(format!(
            "Common routed opponent coverage is {:.1}%; 95% is required.",
            covered_weight * 100.0
        ));
    }
    if candidate_ai_coverage < 1.0 {
        reasons.push(format!(
            "At least one finalist has only {:.1}% AI decision coverage; primary strategy decisions require scoped evidence.",
            candidate_ai_coverage * 100.0
        ));
    }
    let promotable = tournament_claims_allowed
        && candidate_critical_coverage >= 1.0
        && candidate_ai_coverage >= 1.0
        && covered_weight + f64::EPSILON >= 0.95;
    let tier = if !promotable {
        EvidenceTier::Provisional
    } else if used_engines.len() > 1 {
        EvidenceTier::MixedRouteValidated
    } else {
        EvidenceTier::CoverageChecked
    };
    let candidate = candidate_hexdek
        .iter()
        .chain(&candidate_forge)
        .max_by(|left, right| {
            left.critical_coverage
                .total_cmp(&right.critical_coverage)
                .then_with(|| left.trusted_copy_ratio.total_cmp(&right.trusted_copy_ratio))
        })
        .cloned()
        .unwrap_or_else(|| DeckCapabilityReport {
            schema_version: CAPABILITY_SCHEMA_VERSION,
            engine: "none".into(),
            deck_name: "No finalist".into(),
            card_copies: 0,
            trusted_card_copies: 0,
            trusted_copy_ratio: 0.0,
            critical_actions: 0,
            trusted_critical_actions: 0,
            critical_coverage: 0.0,
            oracle_accounted_rate: 0.0,
            structurally_parsed_rate: 0.0,
            engine_executable_rate: 0.0,
            scenario_verified_rate: 0.0,
            ai_decision_coverage: 0.0,
            unresolved_clauses: Vec::new(),
            capabilities: Vec::new(),
            blockers: vec!["No finalist was supplied".into()],
        });
    let plan_material = serde_json::to_vec(&(
        ROUTE_SCHEMA_VERSION,
        candidates,
        opponents,
        &normalized_weights,
        &routes,
    ))
    .unwrap_or_default();
    EngineRoutePlan {
        schema_version: ROUTE_SCHEMA_VERSION,
        capability_policy_revision: CAPABILITY_POLICY_REVISION.into(),
        rules_ir_revision: RULES_IR_REVISION.into(),
        plan_id: short_hash_bytes(&plan_material),
        candidate,
        candidate_by_engine: candidate_hexdek
            .into_iter()
            .chain(candidate_forge)
            .collect(),
        opponent_reports,
        routes,
        field_weighted_coverage: covered_weight,
        candidate_critical_coverage,
        locked: true,
        claim: ValidationClaim {
            tier,
            promotable,
            label: if promotable {
                if used_engines.len() > 1 {
                    "Coverage checked · common mixed-engine route"
                } else {
                    "Coverage checked · common single-engine route"
                }
            } else {
                "Provisional · no promotable common engine route"
            }
            .into(),
            reasons,
        },
    }
}

/// Executes a locked route plan. Unsupported, crashed, timed-out, or incomplete
/// matchups remain visible and are charged as failed evidence.
pub fn run_routed_validation_cancellable(
    data_dir: &Path,
    request: &simulator::HexdekValidationRequest,
    plan: &EngineRoutePlan,
    cancellation: &CancellationToken,
) -> Result<simulator::SimulationReport, String> {
    if !plan.locked {
        return Err("Competitive validation requires an immutable engine route plan.".into());
    }
    if request.opponents.len() != plan.routes.len() {
        return Err("Engine route plan does not match the opponent field.".into());
    }
    if request.commander {
        let engines = plan
            .routes
            .iter()
            .filter_map(|route| route.engine.as_deref())
            .collect::<BTreeSet<_>>();
        let engine = (engines.len() == 1 && plan.routes.iter().all(|route| route.eligible))
            .then(|| *engines.iter().next().expect("checked length"));
        let Some(engine) = engine else {
            return Ok(failed_route_report(request, plan, request.games));
        };
        let mut report = run_engine(data_dir, engine, request, cancellation)?;
        report.engine = format!("{engine} · routed");
        report.engine_routes = plan
            .routes
            .iter()
            .map(|route| (route.opponent_name.clone(), engine.to_string()))
            .collect();
        report.route_failures.extend(
            plan.routes
                .iter()
                .flat_map(|route| route.reasons.iter().cloned()),
        );
        return Ok(report);
    }

    let allocations = simulator::weighted_game_allocations(
        request.games,
        request.opponents.len(),
        &request.opponent_weights,
    );
    let mut reports = Vec::new();
    let mut failures = Vec::new();
    let mut failed_attempts = 0usize;
    let mut engine_routes = HashMap::new();
    for (index, opponent) in request.opponents.iter().enumerate() {
        if cancellation.is_cancelled() {
            return Err("Validation cancelled.".into());
        }
        let route = &plan.routes[index];
        let stable_opponent = format!("Opponent {} · {}", index + 1, opponent.name);
        let Some(engine) = route.engine.as_deref().filter(|_| route.eligible) else {
            failed_attempts = failed_attempts.saturating_add(allocations[index]);
            failures.push(format!(
                "{}: no eligible engine ({})",
                route.opponent_name,
                route.reasons.join("; ")
            ));
            engine_routes.insert(stable_opponent, "unsupported".into());
            continue;
        };
        engine_routes.insert(stable_opponent.clone(), engine.into());
        let mut matchup = request.clone();
        matchup.opponents = vec![opponent.clone()];
        matchup.opponent_weights = vec![1.0];
        matchup.games = allocations[index];
        matchup.seats = 2;
        matchup.seed = request
            .seed
            .wrapping_add((index as i64).wrapping_mul(2_000_033));
        match run_engine(data_dir, engine, &matchup, cancellation) {
            Ok(report) => reports.push((index, report)),
            Err(error) => {
                failed_attempts = failed_attempts.saturating_add(allocations[index]);
                failures.push(format!("{} via {engine}: {error}", route.opponent_name));
            }
        }
    }
    Ok(merge_constructed_reports(
        request,
        plan,
        reports,
        engine_routes,
        failures,
        failed_attempts,
    ))
}

fn run_engine(
    data_dir: &Path,
    engine: &str,
    request: &simulator::HexdekValidationRequest,
    cancellation: &CancellationToken,
) -> Result<simulator::SimulationReport, String> {
    match engine {
        "hexdek" => simulator::run_validation_gauntlet_cancellable(data_dir, request, cancellation)
            .map_err(|error| error.to_string()),
        "forge" => forge::run_validation_cancellable(data_dir, request, cancellation)
            .map_err(|error| error.to_string()),
        other => Err(format!("Unknown engine route '{other}'.")),
    }
}

fn failed_route_report(
    request: &simulator::HexdekValidationRequest,
    plan: &EngineRoutePlan,
    failed_attempts: usize,
) -> simulator::SimulationReport {
    let candidate = format!("Candidate · {}", request.candidate.name);
    simulator::SimulationReport {
        engine: "unroutable".into(),
        schema_version: "capability-routing-v8".into(),
        engine_routes: plan
            .routes
            .iter()
            .map(|route| (route.opponent_name.clone(), "unsupported".into()))
            .collect(),
        route_failures: plan
            .routes
            .iter()
            .flat_map(|route| route.reasons.iter().cloned())
            .collect(),
        provenance: None,
        games: 0,
        draws: 0,
        crashes: failed_attempts,
        cached_games: 0,
        seat_order: Vec::new(),
        average_turns: 0.0,
        games_per_second: 0.0,
        rows: vec![simulator::MatchupRow {
            deck: candidate.clone(),
            games: 0,
            wins: 0,
            win_rate: 0.0,
            win_rate_lower_bound: 0.0,
            win_rate_upper_bound: 1.0,
            average_turn_to_win: None,
        }],
        matchup_matrix: HashMap::from([(candidate.clone(), HashMap::new())]),
        matchup_wins: HashMap::from([(candidate.clone(), HashMap::new())]),
        matchup_games: HashMap::from([(candidate, HashMap::new())]),
        matchup_weights: plan
            .routes
            .iter()
            .enumerate()
            .map(|(index, route)| {
                (
                    format!("Opponent {} · {}", index + 1, route.opponent_name),
                    route.weight,
                )
            })
            .collect(),
        commander_evidence: None,
        combo_execution: None,
        performance: None,
    }
}

fn merge_constructed_reports(
    request: &simulator::HexdekValidationRequest,
    plan: &EngineRoutePlan,
    reports: Vec<(usize, simulator::SimulationReport)>,
    engine_routes: HashMap<String, String>,
    mut failures: Vec<String>,
    failed_attempts: usize,
) -> simulator::SimulationReport {
    let candidate_name = format!("Candidate · {}", request.candidate.name);
    let mut rows = HashMap::<String, (usize, usize, f64, usize)>::new();
    let mut matchup_wins = HashMap::<String, HashMap<String, usize>>::new();
    let mut matchup_games = HashMap::<String, HashMap<String, usize>>::new();
    let mut seat_order = Vec::new();
    let mut games = 0usize;
    let mut draws = 0usize;
    let mut crashes = failed_attempts;
    let mut cached_games = 0usize;
    let mut weighted_turns = 0.0;
    let mut estimated_seconds = 0.0;
    let mut combo_execution = simulator::ComboExecutionEvidence::default();
    let mut has_combo_execution = false;
    for (index, report) in reports {
        if let Some(evidence) = &report.combo_execution {
            has_combo_execution = true;
            combo_execution.plan_available |= evidence.plan_available;
            if combo_execution.proof_revision.is_empty() {
                combo_execution.proof_revision = evidence.proof_revision.clone();
            }
            combo_execution.planned_lines =
                combo_execution.planned_lines.max(evidence.planned_lines);
            combo_execution.assembling_games = combo_execution
                .assembling_games
                .saturating_add(evidence.assembling_games);
            combo_execution.executable_games = combo_execution
                .executable_games
                .saturating_add(evidence.executable_games);
            combo_execution.priority_actions = combo_execution
                .priority_actions
                .saturating_add(evidence.priority_actions);
            combo_execution.plan_wins =
                combo_execution.plan_wins.saturating_add(evidence.plan_wins);
            for limitation in &evidence.limitations {
                if !combo_execution.limitations.contains(limitation) {
                    combo_execution.limitations.push(limitation.clone());
                }
            }
        }
        let stable_opponent = format!("Opponent {} · {}", index + 1, request.opponents[index].name);
        let candidate_row = report
            .rows
            .iter()
            .find(|row| row.deck.starts_with("Candidate · "))
            .or_else(|| report.rows.first());
        let opponent_row = report
            .rows
            .iter()
            .find(|row| candidate_row.is_none_or(|candidate| row.deck != candidate.deck));
        if let Some(row) = candidate_row {
            let entry = rows.entry(candidate_name.clone()).or_default();
            entry.0 = entry.0.saturating_add(row.wins);
            entry.1 = entry.1.saturating_add(row.games);
            if let Some(turn) = row.average_turn_to_win {
                entry.2 += turn * row.wins as f64;
                entry.3 = entry.3.saturating_add(row.wins);
            }
            matchup_wins
                .entry(candidate_name.clone())
                .or_default()
                .insert(stable_opponent.clone(), row.wins);
            matchup_games
                .entry(candidate_name.clone())
                .or_default()
                .insert(stable_opponent.clone(), row.games);
        }
        if let Some(row) = opponent_row {
            let entry = rows.entry(stable_opponent.clone()).or_default();
            entry.0 = entry.0.saturating_add(row.wins);
            entry.1 = entry.1.saturating_add(row.games);
            if let Some(turn) = row.average_turn_to_win {
                entry.2 += turn * row.wins as f64;
                entry.3 = entry.3.saturating_add(row.wins);
            }
            matchup_wins
                .entry(stable_opponent.clone())
                .or_default()
                .insert(candidate_name.clone(), row.wins);
            matchup_games
                .entry(stable_opponent.clone())
                .or_default()
                .insert(candidate_name.clone(), row.games);
        }
        for mut seat in report.seat_order {
            seat.matchup = stable_opponent.clone();
            if candidate_row.is_some_and(|row| seat.deck == row.deck) {
                seat.deck = candidate_name.clone();
            } else {
                seat.deck = stable_opponent.clone();
            }
            seat_order.push(seat);
        }
        games = games.saturating_add(report.games);
        draws = draws.saturating_add(report.draws);
        crashes = crashes.saturating_add(report.crashes);
        cached_games = cached_games.saturating_add(report.cached_games);
        weighted_turns += report.average_turns * report.games as f64;
        estimated_seconds += report.games as f64 / report.games_per_second.max(f64::EPSILON);
        failures.extend(report.route_failures);
    }
    rows.entry(candidate_name.clone()).or_default();
    let mut rows = rows
        .into_iter()
        .map(|(deck, (wins, row_games, win_turns, turn_wins))| {
            let (lower, upper) = wilson_interval(wins, row_games);
            simulator::MatchupRow {
                deck,
                games: row_games,
                wins,
                win_rate: wins as f64 / row_games.max(1) as f64,
                win_rate_lower_bound: lower,
                win_rate_upper_bound: upper,
                average_turn_to_win: (turn_wins > 0).then_some(win_turns / turn_wins as f64),
            }
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        usize::from(right.deck == candidate_name)
            .cmp(&usize::from(left.deck == candidate_name))
            .then_with(|| left.deck.cmp(&right.deck))
    });
    let matchup_matrix = matchup_wins
        .iter()
        .map(|(left, opponents)| {
            let rates = opponents
                .iter()
                .map(|(right, wins)| {
                    let matchup_games = matchup_games
                        .get(left)
                        .and_then(|values| values.get(right))
                        .copied()
                        .unwrap_or(0);
                    (right.clone(), *wins as f64 / matchup_games.max(1) as f64)
                })
                .collect();
            (left.clone(), rates)
        })
        .collect();
    let used = engine_routes
        .values()
        .filter(|engine| engine.as_str() != "unsupported")
        .cloned()
        .collect::<BTreeSet<_>>();
    if has_combo_execution {
        let observed_games = games.max(1) as f64;
        combo_execution.assembling_rate = combo_execution.assembling_games as f64 / observed_games;
        combo_execution.executable_rate = combo_execution.executable_games as f64 / observed_games;
        combo_execution.win_conversion_rate =
            combo_execution.plan_wins as f64 / combo_execution.executable_games.max(1) as f64;
    }
    simulator::SimulationReport {
        engine: if used.len() > 1 {
            "mixed-route".into()
        } else {
            used.iter().next().map_or_else(
                || "unroutable".into(),
                |engine| format!("{engine} · routed"),
            )
        },
        schema_version: "capability-routing-v8".into(),
        engine_routes,
        route_failures: failures,
        provenance: None,
        games,
        draws,
        crashes,
        cached_games,
        seat_order,
        average_turns: weighted_turns / games.max(1) as f64,
        games_per_second: games as f64 / estimated_seconds.max(f64::EPSILON),
        rows,
        matchup_matrix,
        matchup_wins,
        matchup_games,
        matchup_weights: plan
            .routes
            .iter()
            .enumerate()
            .map(|(index, route)| {
                (
                    format!("Opponent {} · {}", index + 1, route.opponent_name),
                    route.weight,
                )
            })
            .collect(),
        commander_evidence: None,
        combo_execution: has_combo_execution.then_some(combo_execution),
        performance: None,
    }
}

fn wilson_interval(wins: usize, games: usize) -> (f64, f64) {
    if games == 0 {
        return (0.0, 1.0);
    }
    let z = 1.959_963_984_540_054_f64;
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

fn capability_state(
    engine: &str,
    ready: bool,
    parser_status: Option<&str>,
    conformance: Option<&conformance::CardConformance>,
    _card: &Card,
) -> MechanicCapabilityState {
    if !ready {
        return MechanicCapabilityState::Unsupported;
    }
    if let Some(conformance) = conformance {
        match conformance.classification.as_str() {
            "disputed" => return MechanicCapabilityState::Disputed,
            "unsupported" => return MechanicCapabilityState::Unsupported,
            // Schema-v1 evidence is card-wide. It remains useful negative
            // evidence, but cannot positively verify an unrelated clause.
            _ => {}
        }
    }
    if engine == "forge" {
        // Forge script presence is not clause-level execution evidence. A
        // future v2 manifest or scoped scenario may promote individual
        // subjects; the conservative truth gate starts at Parsed.
        return MechanicCapabilityState::Parsed;
    }
    match parser_status {
        Some("missing") => MechanicCapabilityState::Unsupported,
        Some("partial" | "semantic_risk") => MechanicCapabilityState::Parsed,
        _ => MechanicCapabilityState::Executable,
    }
}

#[derive(Clone)]
struct CapabilitySubject {
    mode_name: String,
    subject_id: String,
    action: String,
    criticality: ClauseCriticality,
    parser_confidence: f32,
    classification: ClauseClassification,
    high_risk: bool,
    ai_decision_covered: bool,
    oracle_clause: bool,
}

fn capability_subject_state(
    engine: &str,
    card_state: MechanicCapabilityState,
    subject: &CapabilitySubject,
    scoped_claim: Option<&conformance::ActionConformance>,
) -> MechanicCapabilityState {
    if matches!(
        card_state,
        MechanicCapabilityState::Unsupported | MechanicCapabilityState::Disputed
    ) {
        return card_state;
    }
    if let Some(claim) = scoped_claim {
        match claim.classification.as_str() {
            "verified" | "partially_covered" => {
                return MechanicCapabilityState::ScenarioVerified;
            }
            "disputed" => return MechanicCapabilityState::Disputed,
            "unsupported" => return MechanicCapabilityState::Unsupported,
            _ => {}
        }
    }
    if subject.classification == ClauseClassification::UnresolvedGameplayResidual
        || subject.parser_confidence < 1.0
        || subject.high_risk
        || engine == "forge"
    {
        return MechanicCapabilityState::Parsed;
    }
    card_state
}

fn capability_subjects(card: &Card) -> Vec<CapabilitySubject> {
    let mut subjects = Vec::new();
    for (mode_index, mode) in card.features.initial_modes.iter().enumerate() {
        subjects.push(CapabilitySubject {
            mode_name: mode.name.clone(),
            subject_id: format!("{}:mode:{mode_index}", card.id.0),
            action: if mode.is_land {
                format!("play {}", mode.name)
            } else {
                format!("cast {}", mode.name)
            },
            criticality: ClauseCriticality::RuleCritical,
            parser_confidence: 1.0,
            classification: ClauseClassification::ParsedGameplayRule,
            high_risk: false,
            ai_decision_covered: true,
            oracle_clause: false,
        });
        if !mode.oracle_clauses.is_empty() {
            subjects.extend(
                mode.oracle_clauses
                    .iter()
                    .filter(|clause| clause.classification != ClauseClassification::ReminderText)
                    .map(|clause| CapabilitySubject {
                        mode_name: mode.name.clone(),
                        subject_id: clause.id.clone(),
                        action: clause.text.clone(),
                        criticality: clause.criticality,
                        parser_confidence: clause.parser_confidence,
                        classification: clause.classification,
                        high_risk: high_risk_clause(&clause.normalized_text),
                        ai_decision_covered: false,
                        oracle_clause: true,
                    }),
            );
        } else {
            // Legacy serialized cards remain readable, but Unknown abilities
            // are now explicit claim blockers instead of disappearing.
            subjects.extend(mode.abilities.iter().enumerate().map(|(index, ability)| {
                CapabilitySubject {
                    mode_name: mode.name.clone(),
                    subject_id: format!("{}:legacy:{mode_index}:{index}", card.id.0),
                    action: if ability.label.trim().is_empty() {
                        format!("{:?} ability", ability.kind).to_ascii_lowercase()
                    } else {
                        ability.label.clone()
                    },
                    criticality: if ability.kind == AbilityKind::Unknown {
                        ClauseCriticality::RuleCritical
                    } else {
                        ClauseCriticality::OutcomeCritical
                    },
                    parser_confidence: ability.parser_confidence,
                    classification: if ability.kind == AbilityKind::Unknown {
                        ClauseClassification::UnresolvedGameplayResidual
                    } else {
                        ClauseClassification::ParsedGameplayRule
                    },
                    high_risk: high_risk_clause(&ability.label.to_ascii_lowercase()),
                    ai_decision_covered: false,
                    oracle_clause: true,
                }
            }));
        }
    }
    if subjects.is_empty() {
        subjects.push(CapabilitySubject {
            mode_name: card.name.clone(),
            subject_id: format!("{}:legacy-mode", card.id.0),
            action: if card.features.is_land {
                "play land".into()
            } else {
                "cast spell".into()
            },
            criticality: ClauseCriticality::RuleCritical,
            parser_confidence: 1.0,
            classification: ClauseClassification::ParsedGameplayRule,
            high_risk: false,
            ai_decision_covered: true,
            oracle_clause: false,
        });
    }
    subjects
}

fn high_risk_clause(text: &str) -> bool {
    [
        " instead",
        "copy",
        "gain control",
        "extra turn",
        "additional turn",
        "conjure",
        "spellbook",
        "perpetually",
        "seek ",
        "intensity",
        "double team",
    ]
    .iter()
    .any(|marker| text.contains(marker))
}

fn classify_strategy(deck: &Deck, pool: &CardPool) -> StrategyArchetype {
    let mut nonlands = 0.0f64;
    let mut pressure = 0.0f64;
    let mut interaction = 0.0f64;
    let mut combo = 0.0f64;
    let mut graveyard = 0.0f64;
    for (card, quantity) in deck_cards(deck, pool) {
        if card.features.is_land {
            continue;
        }
        let quantity = quantity as f64;
        nonlands += quantity;
        pressure += quantity * f64::from(card.features.pressure);
        interaction += quantity * f64::from(card.features.interaction);
        if card.features.tags.iter().any(|tag| {
            matches!(
                tag.as_str(),
                "copy" | "token" | "blink" | "sacrifice" | "spellslinger"
            )
        }) {
            combo += quantity;
        }
        if card
            .features
            .tags
            .iter()
            .any(|tag| matches!(tag.as_str(), "graveyard" | "recursion" | "death"))
        {
            graveyard += quantity;
        }
    }
    let total = nonlands.max(1.0);
    if graveyard / total >= 0.30 {
        StrategyArchetype::Graveyard
    } else if combo / total >= 0.38 {
        StrategyArchetype::Combo
    } else if interaction / total >= 0.34 && pressure / total < 0.45 {
        StrategyArchetype::Control
    } else if pressure / total >= 0.70 {
        StrategyArchetype::Aggro
    } else {
        StrategyArchetype::Midrange
    }
}

fn deck_cards<'a>(deck: &'a Deck, pool: &'a CardPool) -> Vec<(&'a Card, usize)> {
    deck.command_zone
        .iter()
        .chain(&deck.mainboard)
        .chain(&deck.sideboard)
        .filter_map(|entry| {
            pool.card(&entry.card_id)
                .map(|card| (card, usize::from(entry.quantity)))
        })
        .collect()
}

fn nonland_copy_vector(deck: &Deck, pool: &CardPool) -> BTreeMap<String, u16> {
    let mut vector = BTreeMap::new();
    for (card, quantity) in deck_cards(deck, pool) {
        if !card.features.is_land {
            *vector.entry(normalize_name(&card.name)).or_default() += quantity as u16;
        }
    }
    vector
}

fn weighted_jaccard(left: &BTreeMap<String, u16>, right: &BTreeMap<String, u16>) -> f64 {
    let keys = left.keys().chain(right.keys()).collect::<BTreeSet<_>>();
    let intersection = keys
        .iter()
        .map(|key| {
            left.get(*key)
                .copied()
                .unwrap_or(0)
                .min(right.get(*key).copied().unwrap_or(0)) as u64
        })
        .sum::<u64>();
    let union = keys
        .iter()
        .map(|key| {
            left.get(*key)
                .copied()
                .unwrap_or(0)
                .max(right.get(*key).copied().unwrap_or(0)) as u64
        })
        .sum::<u64>();
    intersection as f64 / union.max(1) as f64
}

fn commander_identity_key(deck: &Deck) -> String {
    deck.command_zone
        .iter()
        .map(|entry| entry.card_id.0.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("+")
}

fn normalized_weights(count: usize, weights: &[f64]) -> Vec<f64> {
    if count == 0 {
        return Vec::new();
    }
    let mut normalized = (0..count)
        .map(|index| weights.get(index).copied().unwrap_or(1.0).max(0.0))
        .collect::<Vec<_>>();
    let total = normalized.iter().sum::<f64>();
    if total <= f64::EPSILON {
        normalized.fill(1.0 / count as f64);
    } else {
        for weight in &mut normalized {
            *weight /= total;
        }
    }
    normalized
}

fn state_label(state: MechanicCapabilityState) -> &'static str {
    match state {
        MechanicCapabilityState::Unsupported => "unsupported",
        MechanicCapabilityState::Parsed => "parsed but unverified",
        MechanicCapabilityState::Executable => "executable",
        MechanicCapabilityState::ScenarioVerified => "scenario verified",
        MechanicCapabilityState::Disputed => "disputed",
    }
}

fn normalize_name(name: &str) -> String {
    name.split(" // ")
        .next()
        .unwrap_or(name)
        .trim()
        .to_ascii_lowercase()
}

fn short_hash(value: &str) -> String {
    short_hash_bytes(value.as_bytes())
}

fn short_hash_bytes(value: &[u8]) -> String {
    let digest = format!("{:x}", Sha256::digest(value));
    format!("sha256:{}", &digest[..24])
}

#[cfg(test)]
mod tests {
    use super::*;
    use metagene_core::{CardFeatures, CardId, DeckEntry};

    fn card(id: &str, land: bool, tags: &[&str], pressure: f32, interaction: f32) -> Card {
        Card {
            id: CardId(id.into()),
            arena_id: Some(1),
            platforms: vec!["paper".into()],
            name: id.into(),
            set_code: "tst".into(),
            collector_number: "1".into(),
            type_line: if land { "Land" } else { "Creature" }.into(),
            oracle_text: String::new(),
            colors: Vec::new(),
            color_identity: Vec::new(),
            image_uri: None,
            legalities: BTreeMap::new(),
            features: CardFeatures {
                is_land: land,
                tags: tags.iter().map(|tag| (*tag).into()).collect(),
                pressure,
                interaction,
                ..CardFeatures::default()
            },
        }
    }

    fn deck(name: &str, cards: &[(&str, u8)]) -> Deck {
        Deck {
            name: name.into(),
            command_zone: Vec::new(),
            mainboard: cards
                .iter()
                .map(|(id, quantity)| DeckEntry {
                    card_id: CardId((*id).into()),
                    quantity: *quantity,
                })
                .collect(),
            sideboard: Vec::new(),
        }
    }

    #[test]
    fn weighted_jaccard_uses_copy_counts() {
        let left = BTreeMap::from([("a".into(), 4), ("b".into(), 2)]);
        let right = BTreeMap::from([("a".into(), 3), ("c".into(), 3)]);
        assert!((weighted_jaccard(&left, &right) - 3.0 / 9.0).abs() < 1e-9);
    }

    #[test]
    fn tournament_field_clusters_near_duplicates_and_preserves_source_tier() {
        let pool = CardPool {
            cards: vec![
                card("Forest", true, &[], 0.0, 0.0),
                card("Threat", false, &[], 1.0, 0.0),
                card("Support", false, &[], 0.2, 0.2),
                card("Control", false, &[], 0.0, 1.0),
            ],
        };
        let entries = [
            (
                "one",
                deck("One", &[("Forest", 24), ("Threat", 4), ("Support", 32)]),
                4,
            ),
            (
                "two",
                deck(
                    "Two",
                    &[
                        ("Forest", 24),
                        ("Threat", 4),
                        ("Support", 31),
                        ("Control", 1),
                    ],
                ),
                3,
            ),
            (
                "three",
                deck("Three", &[("Forest", 24), ("Control", 36)]),
                2,
            ),
        ];
        let total = entries.iter().map(|entry| entry.2).sum::<usize>();
        let report = metagame_sources::MetagameAcquisitionReport {
            schema_version: 1,
            source: "Spicerack".into(),
            platform: "paper".into(),
            format: "standard".into(),
            commander: false,
            provenance: metagame_sources::SourceProvenance {
                adapter: "spicerack-public-v1".into(),
                source_url: "https://example.invalid".into(),
                retrieved_at_unix: 1,
                event_start_unix: Some(1),
                event_end_unix: Some(1),
                event_count: 1,
                submitted_deck_count: total,
                accepted_deck_count: total,
                transformation_notes: Vec::new(),
            },
            accepted: entries
                .into_iter()
                .map(
                    |(fingerprint, deck, occurrences)| metagame_sources::AcquiredArchetype {
                        archetype: deck.name.clone(),
                        deck,
                        decklist: String::new(),
                        occurrences,
                        weight: occurrences as f64 / total as f64,
                        fingerprint: fingerprint.into(),
                        provenance: Vec::new(),
                    },
                )
                .collect(),
            quarantined: Vec::new(),
        };
        let field = select_competitive_field(
            &report,
            &pool,
            &FieldSelectionPolicy {
                minimum_constructed_archetypes: 1,
                maximum_constructed_archetypes: 8,
                target_observed_weight: 0.90,
                ..FieldSelectionPolicy::default()
            },
        );
        assert_eq!(field.clusters.len(), 2);
        assert_eq!(field.audit.near_duplicates_clustered, 1);
        assert!(field.audit.competitive_claims_allowed);
        assert_eq!(field.audit.source_tier, "tournament");
        assert!((field.weights.iter().sum::<f64>() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn getpaird_field_is_provisional_only() {
        let report = metagame_sources::MetagameAcquisitionReport {
            schema_version: 1,
            source: "GetPaird".into(),
            platform: "paper".into(),
            format: "standard".into(),
            commander: false,
            provenance: metagame_sources::SourceProvenance {
                adapter: "getpaird-public-decks-v1".into(),
                source_url: String::new(),
                retrieved_at_unix: 0,
                event_start_unix: None,
                event_end_unix: None,
                event_count: 0,
                submitted_deck_count: 0,
                accepted_deck_count: 0,
                transformation_notes: Vec::new(),
            },
            accepted: Vec::new(),
            quarantined: Vec::new(),
        };
        let field = select_competitive_field(
            &report,
            &CardPool { cards: Vec::new() },
            &FieldSelectionPolicy::default(),
        );
        assert!(!field.audit.competitive_claims_allowed);
        assert_eq!(field.audit.source_tier, "public_list_fallback");
    }

    #[test]
    fn unroutable_matchups_are_failed_evidence_instead_of_disappearing() {
        let pool = CardPool {
            cards: vec![
                card("Forest", true, &[], 0.0, 0.0),
                card("Threat", false, &[], 1.0, 0.0),
            ],
        };
        let candidate = deck("Candidate", &[("Forest", 24), ("Threat", 36)]);
        let opponent = deck("Opponent", &[("Forest", 24), ("Threat", 36)]);
        let capability = DeckCapabilityReport {
            schema_version: CAPABILITY_SCHEMA_VERSION,
            engine: "none".into(),
            deck_name: candidate.name.clone(),
            card_copies: 60,
            trusted_card_copies: 0,
            trusted_copy_ratio: 0.0,
            critical_actions: 1,
            trusted_critical_actions: 0,
            critical_coverage: 0.0,
            oracle_accounted_rate: 0.0,
            structurally_parsed_rate: 0.0,
            engine_executable_rate: 0.0,
            scenario_verified_rate: 0.0,
            ai_decision_coverage: 0.0,
            unresolved_clauses: Vec::new(),
            capabilities: Vec::new(),
            blockers: vec!["unsupported fixture".into()],
        };
        let plan = EngineRoutePlan {
            schema_version: ROUTE_SCHEMA_VERSION,
            capability_policy_revision: CAPABILITY_POLICY_REVISION.into(),
            rules_ir_revision: RULES_IR_REVISION.into(),
            plan_id: "fixture".into(),
            candidate: capability.clone(),
            candidate_by_engine: vec![capability.clone()],
            opponent_reports: vec![capability],
            routes: vec![EngineRoute {
                opponent_index: 0,
                opponent_name: opponent.name.clone(),
                weight: 1.0,
                engine: None,
                eligible: false,
                reasons: vec!["no trusted route".into()],
            }],
            field_weighted_coverage: 0.0,
            candidate_critical_coverage: 0.0,
            locked: true,
            claim: ValidationClaim {
                tier: EvidenceTier::Provisional,
                promotable: false,
                label: "Provisional".into(),
                reasons: vec!["no trusted route".into()],
            },
        };
        let request = simulator::HexdekValidationRequest {
            candidate,
            opponents: vec![opponent],
            opponent_weights: vec![1.0],
            pool,
            games: 16,
            seed: 7,
            workers: 1,
            seats: 2,
            hat_budget: 0,
            max_turns: 20,
            game_timeout_seconds: 1,
            commander: false,
            fixed_order: false,
        };
        let report = run_routed_validation_cancellable(
            Path::new("."),
            &request,
            &plan,
            &CancellationToken::default(),
        )
        .expect("unroutable evidence should remain inspectable");
        assert_eq!(report.games, 0);
        assert_eq!(report.crashes, 16);
        assert_eq!(report.engine_routes.values().next().unwrap(), "unsupported");
        assert!(
            report
                .route_failures
                .iter()
                .any(|failure| failure.contains("no eligible"))
        );
    }
}
