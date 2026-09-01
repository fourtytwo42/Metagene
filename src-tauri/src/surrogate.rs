use burn::backend::flex::FlexDevice;
use burn::backend::wgpu::WgpuDevice;
use burn::backend::{Flex, Vulkan};
use burn::prelude::{Backend, Tensor};
use burn::tensor::activation;
use metagene_core::RankedDeck;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::Instant;
use thiserror::Error;

const MIN_LABELS: usize = 5_000;
const MAX_LABELS: usize = 250_000;
const MAX_DATABASE_BYTES: u64 = 750 * 1024 * 1024;
const MAX_MODEL_BYTES: u64 = 256 * 1024 * 1024;
const FEATURE_DIM: usize = 32;
const ENSEMBLE_SIZE: usize = 8;
const MODEL_SCHEMA: u32 = 4;
const CAPABILITY_POLICY_REVISION: &str = "historic-rules-capability-v3";
const RULES_IR_REVISION: &str = "rules-ir-exact-state-v2";
const OOD_UNCERTAINTY: f64 = 0.20;

#[derive(Debug, Error)]
pub enum SurrogateError {
    #[error("surrogate database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("surrogate storage error: {0}")]
    Io(#[from] std::io::Error),
    #[error("surrogate serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("surrogate label must be finite")]
    InvalidLabel,
    #[error("surrogate model error: {0}")]
    Model(String),
}

/// One completed, fully covered game label. Records that do not meet the trust
/// contract are rejected before reaching this persisted representation.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TrainingLabel {
    pub platform: String,
    pub format: String,
    pub commander: bool,
    pub run_id: String,
    pub cycle: usize,
    pub candidate_rank: usize,
    pub lineage_id: String,
    pub field_revision: String,
    pub engine_revision: String,
    pub route_revision: String,
    pub capability_policy_revision: String,
    pub rules_ir_revision: String,
    pub card_data_revision: String,
    pub configuration_hash: String,
    pub play_draw: String,
    pub games: usize,
    pub features: Vec<f32>,
    pub observed: f32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SurrogateManifest {
    pub schema_version: u32,
    pub model_revision: String,
    pub trained_at: u64,
    pub feature_revision: String,
    pub platform: String,
    pub format: String,
    pub commander: bool,
    pub excluded_run_id: Option<String>,
    pub labels: usize,
    pub lineage_field_groups: usize,
    pub validation_labels: usize,
    pub validation_groups: usize,
    pub validation_mae: f64,
    pub validation_log_loss: f64,
    pub baseline_log_loss: f64,
    pub group_isolated: bool,
    pub holdout_isolation: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceProfile {
    pub schema_version: u32,
    pub benchmarked_at: u64,
    pub cpu_backend: String,
    pub gpu_backend: String,
    pub gpu_available: bool,
    pub cpu_predictions_per_second: f64,
    pub gpu_predictions_per_second: f64,
    pub speedup: f64,
    pub maximum_absolute_difference: f64,
    pub calibration_gate_passed: bool,
    pub closed_loop_quality_per_minute_gain: f64,
    pub gpu_enabled: bool,
    pub decision: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct SurrogateStatus {
    pub labels: usize,
    pub minimum_labels: usize,
    pub ready: bool,
    pub stale: bool,
    pub recent_mae: Option<f64>,
    pub historical_mae: Option<f64>,
    pub gpu_training_enabled: bool,
    pub gpu_decision: String,
    pub database_bytes: u64,
    pub manifest: Option<SurrogateManifest>,
    pub device: Option<DeviceProfile>,
    pub trustworthy_only: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct TrainingLabelAudit {
    pub total_labels: usize,
    pub eligible_labels: usize,
    pub quarantined_labels: usize,
    pub capability_policy_revision: String,
    pub rules_ir_revision: String,
    pub quarantine_reasons: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ScreeningReport {
    pub input_candidates: usize,
    pub simulated_candidates: usize,
    pub uncertain_candidates_preserved: usize,
    pub model_used: bool,
    pub stale_model_bypassed: bool,
    pub gpu_used: bool,
    pub active_run_labels_excluded: usize,
    pub out_of_distribution_candidates: usize,
}

pub struct SurrogateObservation<'a> {
    pub platform: &'a str,
    pub format: &'a str,
    pub commander: bool,
    pub run_id: &'a str,
    pub cycle: usize,
    pub candidate: &'a RankedDeck,
    pub observed: f64,
    pub lineage_id: &'a str,
    pub field_revision: &'a str,
    pub engine_revision: &'a str,
    pub route_revision: &'a str,
    pub capability_policy_revision: &'a str,
    pub rules_ir_revision: &'a str,
    pub card_data_revision: &'a str,
    pub configuration_hash: &'a str,
    pub play_draw: &'a str,
    pub games: usize,
    pub expected_games: usize,
    pub field_coverage: f64,
    pub candidate_critical_coverage: f64,
    pub opponent_count: usize,
    pub completed: bool,
    pub holdout: bool,
    pub crashed: bool,
    pub timed_out: bool,
    pub disputed_critical_mechanic: bool,
    pub unresolved_critical_clause: bool,
}

pub struct ScreeningContext<'a> {
    pub platform: &'a str,
    pub format: &'a str,
    pub commander: bool,
    pub active_run_id: &'a str,
    pub field_revision: &'a str,
    pub opponent_count: usize,
    pub field_coverage: f64,
}

struct FeatureContext<'a> {
    platform: &'a str,
    format: &'a str,
    commander: bool,
    field_revision: &'a str,
    opponent_count: usize,
    field_coverage: f64,
    play_draw: &'a str,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct EnsembleModel {
    manifest: SurrogateManifest,
    center: Vec<f32>,
    scale: Vec<f32>,
    minimum: Vec<f32>,
    maximum: Vec<f32>,
    /// One normalized linear-logistic model per ensemble member. The final
    /// value in each vector is the bias.
    weights: Vec<Vec<f32>>,
}

#[derive(Clone)]
struct Label {
    lineage_id: String,
    field_revision: String,
    run_id: String,
    features: [f32; FEATURE_DIM],
    observed: f32,
}

pub fn record(
    data_dir: &Path,
    observation: SurrogateObservation<'_>,
) -> Result<bool, SurrogateError> {
    if !observation.observed.is_finite() || !observation.field_coverage.is_finite() {
        return Err(SurrogateError::InvalidLabel);
    }
    let provenance_complete = [
        observation.lineage_id,
        observation.field_revision,
        observation.engine_revision,
        observation.route_revision,
        observation.capability_policy_revision,
        observation.rules_ir_revision,
        observation.card_data_revision,
        observation.configuration_hash,
    ]
    .iter()
    .all(|value| !value.trim().is_empty());
    let trustworthy = provenance_complete
        && observation.completed
        && observation.holdout
        && !observation.crashed
        && !observation.timed_out
        && !observation.disputed_critical_mechanic
        && !observation.unresolved_critical_clause
        && observation.capability_policy_revision == CAPABILITY_POLICY_REVISION
        && observation.rules_ir_revision == RULES_IR_REVISION
        && observation.field_coverage >= 0.95
        && observation.candidate_critical_coverage >= 1.0
        && observation.games > 0
        && observation.games >= observation.expected_games;
    if !trustworthy {
        return Ok(false);
    }
    let features = candidate_features(
        observation.candidate,
        &FeatureContext {
            platform: observation.platform,
            format: observation.format,
            commander: observation.commander,
            field_revision: observation.field_revision,
            opponent_count: observation.opponent_count,
            field_coverage: observation.field_coverage,
            play_draw: observation.play_draw,
        },
    );
    let connection = open(data_dir)?;
    connection.execute(
        "INSERT OR REPLACE INTO training_labels
         (platform,format,commander,run_id,cycle,candidate_rank,lineage_id,field_revision,
          engine_revision,route_revision,capability_policy_revision,rules_ir_revision,
          card_data_revision,configuration_hash,play_draw,games,features,observed)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
        params![
            observation.platform,
            observation.format,
            observation.commander,
            observation.run_id,
            saturating_i64(observation.cycle),
            saturating_i64(observation.candidate.rank),
            observation.lineage_id,
            observation.field_revision,
            observation.engine_revision,
            observation.route_revision,
            observation.capability_policy_revision,
            observation.rules_ir_revision,
            observation.card_data_revision,
            observation.configuration_hash,
            observation.play_draw,
            saturating_i64(observation.games),
            serde_json::to_string(&features)?,
            observation.observed.clamp(0.0, 1.0),
        ],
    )?;
    enforce_storage_bounds(data_dir, &connection)?;
    Ok(true)
}

pub fn status(
    data_dir: &Path,
    platform: &str,
    format: &str,
    commander: bool,
) -> Result<SurrogateStatus, SurrogateError> {
    let labels = load_labels(data_dir, platform, format, commander, None)?;
    let manifest = load_model(data_dir)?.map(|model| model.manifest);
    let device = load_device_profile(data_dir)?;
    let historical_mae = manifest.as_ref().map(|manifest| manifest.validation_mae);
    let recent_mae = load_model(data_dir)?.and_then(|model| {
        let recent = labels.iter().take(128).cloned().collect::<Vec<_>>();
        if recent.is_empty() {
            None
        } else {
            predict_manual(
                &model,
                &recent
                    .iter()
                    .map(|label| label.features)
                    .collect::<Vec<_>>(),
            )
            .ok()
            .map(|predictions| {
                predictions
                    .iter()
                    .zip(&recent)
                    .map(|(prediction, label)| (prediction.0 - f64::from(label.observed)).abs())
                    .sum::<f64>()
                    / recent.len() as f64
            })
        }
    });
    let stale = recent_mae
        .zip(historical_mae)
        .is_some_and(|(recent, historical)| recent > 0.12_f64.max(historical * 1.5));
    let trained_for_scope = manifest.as_ref().is_some_and(|manifest| {
        manifest.labels <= labels.len()
            && manifest.group_isolated
            && manifest.platform == platform
            && manifest.format == format
            && manifest.commander == commander
    });
    let ready = labels.len() >= MIN_LABELS && trained_for_scope && !stale;
    let gpu_decision = if labels.len() < MIN_LABELS {
        format!(
            "Collect {} more fully covered holdout labels before training.",
            MIN_LABELS - labels.len()
        )
    } else if !trained_for_scope {
        "The trustworthy-label gate is open; train the group-isolated matchup model before screening."
            .into()
    } else if stale {
        "Recent calibration drift exceeded the safety threshold; inference is bypassed until retraining."
            .into()
    } else {
        device.as_ref().map_or_else(
            || "CPU inference is ready; the first eligible screen will benchmark WGPU automatically."
                .into(),
            |profile| profile.decision.clone(),
        )
    };
    Ok(SurrogateStatus {
        labels: labels.len(),
        minimum_labels: MIN_LABELS,
        ready,
        stale,
        recent_mae,
        historical_mae,
        gpu_training_enabled: device.as_ref().is_some_and(|profile| profile.gpu_enabled),
        gpu_decision,
        database_bytes: fs::metadata(database_path(data_dir)).map_or(0, |value| value.len()),
        manifest,
        device,
        trustworthy_only: true,
    })
}

pub fn audit_training_labels(data_dir: &Path) -> Result<TrainingLabelAudit, SurrogateError> {
    let connection = open(data_dir)?;
    let total = connection.query_row("SELECT COUNT(*) FROM training_labels", [], |row| {
        row.get::<_, i64>(0)
    })?;
    let eligible = connection.query_row(
        "SELECT COUNT(*) FROM training_labels
         WHERE capability_policy_revision=?1 AND rules_ir_revision=?2",
        params![CAPABILITY_POLICY_REVISION, RULES_IR_REVISION],
        |row| row.get::<_, i64>(0),
    )?;
    let total = total.max(0) as usize;
    let eligible = eligible.max(0) as usize;
    let quarantined = total.saturating_sub(eligible);
    Ok(TrainingLabelAudit {
        total_labels: total,
        eligible_labels: eligible,
        quarantined_labels: quarantined,
        capability_policy_revision: CAPABILITY_POLICY_REVISION.into(),
        rules_ir_revision: RULES_IR_REVISION.into(),
        quarantine_reasons: if quarantined > 0 {
            vec![
                "Legacy labels lack the current action-scoped capability policy or Rules IR revision; rows are retained for audit but excluded from training and the 5,000-label gate."
                    .into(),
            ]
        } else {
            Vec::new()
        },
    })
}

pub fn train(
    data_dir: &Path,
    platform: &str,
    format: &str,
    commander: bool,
) -> Result<SurrogateStatus, SurrogateError> {
    train_with_exclusion(data_dir, platform, format, commander, None)
}

fn train_with_exclusion(
    data_dir: &Path,
    platform: &str,
    format: &str,
    commander: bool,
    excluded_run_id: Option<&str>,
) -> Result<SurrogateStatus, SurrogateError> {
    let labels = load_labels(data_dir, platform, format, commander, excluded_run_id)?;
    if labels.len() < MIN_LABELS {
        return status(data_dir, platform, format, commander);
    }
    let model = train_ensemble(&labels, platform, format, commander, excluded_run_id)?;
    persist_model(data_dir, &model)?;
    let profile = benchmark_model(&model)?;
    persist_device_profile(data_dir, &profile)?;
    status(data_dir, platform, format, commander)
}

pub fn benchmark_device(
    data_dir: &Path,
    platform: &str,
    format: &str,
    commander: bool,
) -> Result<DeviceProfile, SurrogateError> {
    let Some(model) = load_model(data_dir)? else {
        let _ = train(data_dir, platform, format, commander)?;
        let Some(model) = load_model(data_dir)? else {
            return Err(SurrogateError::Model(format!(
                "At least {MIN_LABELS} trustworthy labels are required before device benchmarking."
            )));
        };
        let profile = benchmark_model(&model)?;
        persist_device_profile(data_dir, &profile)?;
        return Ok(profile);
    };
    let profile = benchmark_model(&model)?;
    persist_device_profile(data_dir, &profile)?;
    Ok(profile)
}

pub fn screen(
    data_dir: &Path,
    context: &ScreeningContext<'_>,
    candidates: &[RankedDeck],
) -> Result<(Vec<RankedDeck>, ScreeningReport), SurrogateError> {
    let labels_all = load_labels(
        data_dir,
        context.platform,
        context.format,
        context.commander,
        None,
    )?;
    let excluded = labels_all
        .iter()
        .filter(|label| label.run_id == context.active_run_id)
        .count();
    let report_without_model = |stale| {
        (
            candidates.to_vec(),
            ScreeningReport {
                input_candidates: candidates.len(),
                simulated_candidates: candidates.len(),
                uncertain_candidates_preserved: candidates.len(),
                model_used: false,
                stale_model_bypassed: stale,
                gpu_used: false,
                active_run_labels_excluded: excluded,
                out_of_distribution_candidates: 0,
            },
        )
    };
    if candidates.len() <= 2 || labels_all.len().saturating_sub(excluded) < MIN_LABELS {
        return Ok(report_without_model(false));
    }
    let mut model_status = status(
        data_dir,
        context.platform,
        context.format,
        context.commander,
    )?;
    let active_safe = excluded == 0
        || model_status.manifest.as_ref().is_some_and(|manifest| {
            manifest.excluded_run_id.as_deref() == Some(context.active_run_id)
        });
    if (model_status.manifest.is_none() || !active_safe) && !model_status.stale {
        model_status = train_with_exclusion(
            data_dir,
            context.platform,
            context.format,
            context.commander,
            Some(context.active_run_id),
        )?;
    }
    let eligible_labels = labels_all.len().saturating_sub(excluded);
    let trained_for_active_screen = model_status.manifest.as_ref().is_some_and(|manifest| {
        manifest.labels <= eligible_labels
            && manifest.platform == context.platform
            && manifest.format == context.format
            && manifest.commander == context.commander
            && (excluded == 0 || manifest.excluded_run_id.as_deref() == Some(context.active_run_id))
    });
    if (!model_status.ready && !trained_for_active_screen) || model_status.stale {
        return Ok(report_without_model(model_status.stale));
    }
    let model = load_model(data_dir)?
        .ok_or_else(|| SurrogateError::Model("trained model manifest disappeared".into()))?;
    let features = candidates
        .iter()
        .map(|candidate| {
            candidate_features(
                candidate,
                &FeatureContext {
                    platform: context.platform,
                    format: context.format,
                    commander: context.commander,
                    field_revision: context.field_revision,
                    opponent_count: context.opponent_count,
                    field_coverage: context.field_coverage,
                    play_draw: "rotated",
                },
            )
        })
        .collect::<Vec<_>>();
    let mut device = model_status.device.clone();
    if device.is_none() {
        let profile = benchmark_model(&model)?;
        persist_device_profile(data_dir, &profile)?;
        device = Some(profile);
    }
    let use_gpu = device.as_ref().is_some_and(|profile| profile.gpu_enabled);
    let predictions = if use_gpu {
        match burn_predict_wgpu(&model, &features) {
            Ok(predictions) => predictions,
            Err(error) => {
                let mut failed = device.unwrap_or_default();
                failed.gpu_enabled = false;
                failed.decision = format!(
                    "WGPU inference failed ({error}); Metagene fell back to the bundled Burn Flex CPU backend."
                );
                persist_device_profile(data_dir, &failed)?;
                burn_predict_flex(&model, &features)?
            }
        }
    } else {
        burn_predict_flex(&model, &features)?
    };
    let mut ranked = candidates
        .iter()
        .cloned()
        .zip(predictions)
        .zip(features)
        .map(|((candidate, prediction), features)| {
            let ood = out_of_distribution(&model, &features);
            let uncertainty = prediction.1.max(if ood { OOD_UNCERTAINTY } else { 0.0 });
            (candidate, prediction.0, uncertainty, ood)
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.1.total_cmp(&left.1));
    let guaranteed = candidates.len().div_ceil(2).max(2);
    let mut retained = Vec::new();
    let mut uncertain = 0usize;
    let mut out_of_distribution_candidates = 0usize;
    for (index, (candidate, _, uncertainty, ood)) in ranked.into_iter().enumerate() {
        out_of_distribution_candidates += usize::from(ood);
        if index < guaranteed || uncertainty >= 0.12 {
            uncertain += usize::from(uncertainty >= 0.12);
            retained.push(candidate);
        }
    }
    Ok((
        retained.clone(),
        ScreeningReport {
            input_candidates: candidates.len(),
            simulated_candidates: retained.len(),
            uncertain_candidates_preserved: uncertain,
            model_used: true,
            stale_model_bypassed: false,
            gpu_used: use_gpu,
            active_run_labels_excluded: excluded,
            out_of_distribution_candidates,
        },
    ))
}

fn candidate_features(candidate: &RankedDeck, context: &FeatureContext<'_>) -> [f32; FEATURE_DIM] {
    let fitness = &candidate.fitness;
    let nonland_copies = candidate
        .deck
        .mainboard
        .iter()
        .map(|entry| usize::from(entry.quantity))
        .sum::<usize>();
    let unique_cards = candidate.deck.mainboard.len();
    [
        unit(fitness.score),
        unit(fitness.consistency),
        unit(fitness.tempo),
        unit(fitness.synergy),
        unit(fitness.interaction),
        unit(fitness.resilience),
        unit(1.0 - fitness.mana_failure_rate),
        unit(fitness.commander_quality),
        unit(fitness.sideboard_quality),
        unit(fitness.on_curve_castability),
        unit(fitness.turn_horizon_castability),
        unit(1.0 - fitness.stranded_card_rate),
        unit(1.0 - fitness.legendary_dead_draw_rate),
        unit(1.0 - fitness.unmet_requirement_rate),
        unit(fitness.functional_action_rate),
        unit(1.0 - fitness.dead_action_rate),
        unit(1.0 - fitness.narrow_mainboard_rate),
        unit(1.0 - fitness.copy_excess_rate),
        unit(fitness.closing_power),
        unit(fitness.role_balance),
        unit(fitness.combo_assembly_probability),
        unit(1.0 - fitness.combo_expected_turn.unwrap_or(12.0) / 20.0),
        unit(1.0 - fitness.combo_dead_card_rate),
        unit(fitness.combo_resilience),
        unit(fitness.combo_execution_rate),
        unit(fitness.strategy_similarity),
        (nonland_copies as f32 / 100.0).clamp(0.0, 1.0),
        (unique_cards as f32 / nonland_copies.max(1) as f32).clamp(0.0, 1.0),
        (context.opponent_count as f32 / 16.0).clamp(0.0, 1.0),
        unit(context.field_coverage),
        categorical_unit(&format!(
            "{}:{}:{}:{}",
            context.platform, context.format, context.commander, context.play_draw
        )),
        categorical_unit(context.field_revision),
    ]
}

fn unit(value: f64) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0) as f32
    } else {
        0.0
    }
}

fn categorical_unit(value: &str) -> f32 {
    let digest = Sha256::digest(value.as_bytes());
    u32::from_le_bytes(digest[..4].try_into().expect("four digest bytes")) as f32 / u32::MAX as f32
}

fn train_ensemble(
    labels: &[Label],
    platform: &str,
    format: &str,
    commander: bool,
    excluded_run_id: Option<&str>,
) -> Result<EnsembleModel, SurrogateError> {
    if labels.is_empty() {
        return Err(SurrogateError::Model("training set is empty".into()));
    }
    let groups = labels.iter().map(group_key).collect::<BTreeSet<_>>();
    if groups.len() < 2 {
        return Err(SurrogateError::Model(
            "Training requires at least two independent deck-lineage/field-revision groups.".into(),
        ));
    }
    let mut validation_groups = groups
        .iter()
        .filter(|group| stable_bucket(group, 5) == 0)
        .cloned()
        .collect::<HashSet<_>>();
    if validation_groups.is_empty() {
        validation_groups.insert(groups.iter().next().cloned().unwrap_or_default());
    }
    if validation_groups.len() == groups.len()
        && let Some(training_group) = validation_groups.iter().max().cloned()
    {
        validation_groups.remove(&training_group);
    }
    let training = labels
        .iter()
        .filter(|label| !validation_groups.contains(&group_key(label)))
        .cloned()
        .collect::<Vec<_>>();
    let validation = labels
        .iter()
        .filter(|label| validation_groups.contains(&group_key(label)))
        .cloned()
        .collect::<Vec<_>>();
    if training.is_empty() || validation.is_empty() {
        return Err(SurrogateError::Model(
            "Could not construct a group-isolated training/validation split.".into(),
        ));
    }
    let (center, scale, minimum, maximum) = feature_statistics(&training);
    let normalized = training
        .iter()
        .map(|label| normalize(&label.features, &center, &scale))
        .collect::<Vec<_>>();
    let mut weights = Vec::with_capacity(ENSEMBLE_SIZE);
    for member in 0..ENSEMBLE_SIZE {
        let mut member_weights = vec![0.0f32; FEATURE_DIM + 1];
        for epoch in 0..28 {
            let learning_rate = 0.045 / (1.0 + epoch as f32 * 0.08);
            for (index, (features, label)) in normalized.iter().zip(&training).enumerate() {
                if stable_bucket(&format!("{}:{member}:{index}", group_key(label)), 10) == 0 {
                    continue;
                }
                let prediction = sigmoid(dot_with_bias(&member_weights, features));
                let gradient = (prediction - label.observed) * prediction * (1.0 - prediction);
                for feature in 0..FEATURE_DIM {
                    member_weights[feature] -= learning_rate
                        * (gradient * features[feature] + 0.0005 * member_weights[feature]);
                }
                member_weights[FEATURE_DIM] -= learning_rate * gradient;
            }
        }
        weights.push(member_weights);
    }
    let mut model = EnsembleModel {
        manifest: SurrogateManifest {
            schema_version: MODEL_SCHEMA,
            model_revision: String::new(),
            trained_at: unix_now(),
            feature_revision: "matchup-functions-packages-combo-v2".into(),
            platform: platform.into(),
            format: format.into(),
            commander,
            excluded_run_id: excluded_run_id.map(str::to_string),
            labels: labels.len(),
            lineage_field_groups: groups.len(),
            validation_labels: validation.len(),
            validation_groups: validation_groups.len(),
            validation_mae: 1.0,
            validation_log_loss: f64::INFINITY,
            baseline_log_loss: f64::INFINITY,
            group_isolated: true,
            holdout_isolation:
                "active run excluded; lineage + field revision groups never cross train/validation"
                    .into(),
        },
        center,
        scale,
        minimum,
        maximum,
        weights,
    };
    let queries = validation
        .iter()
        .map(|label| label.features)
        .collect::<Vec<_>>();
    let predictions = predict_manual(&model, &queries)?;
    model.manifest.validation_mae = predictions
        .iter()
        .zip(&validation)
        .map(|(prediction, label)| (prediction.0 - f64::from(label.observed)).abs())
        .sum::<f64>()
        / validation.len().max(1) as f64;
    model.manifest.validation_log_loss = predictions
        .iter()
        .zip(&validation)
        .map(|(prediction, label)| binary_log_loss(prediction.0, f64::from(label.observed)))
        .sum::<f64>()
        / validation.len().max(1) as f64;
    model.manifest.baseline_log_loss = validation
        .iter()
        .map(|label| binary_log_loss(f64::from(label.features[0]), f64::from(label.observed)))
        .sum::<f64>()
        / validation.len().max(1) as f64;
    model.manifest.model_revision = model_revision(&model)?;
    Ok(model)
}

fn feature_statistics(labels: &[Label]) -> (Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>) {
    let mut center = vec![0.0; FEATURE_DIM];
    let mut minimum = vec![f32::INFINITY; FEATURE_DIM];
    let mut maximum = vec![f32::NEG_INFINITY; FEATURE_DIM];
    for label in labels {
        for index in 0..FEATURE_DIM {
            center[index] += label.features[index];
            minimum[index] = minimum[index].min(label.features[index]);
            maximum[index] = maximum[index].max(label.features[index]);
        }
    }
    for value in &mut center {
        *value /= labels.len().max(1) as f32;
    }
    let mut scale = vec![0.0; FEATURE_DIM];
    for label in labels {
        for index in 0..FEATURE_DIM {
            scale[index] += (label.features[index] - center[index]).powi(2);
        }
    }
    for value in &mut scale {
        *value = (*value / labels.len().max(1) as f32).sqrt().max(0.02);
    }
    (center, scale, minimum, maximum)
}

fn normalize(features: &[f32; FEATURE_DIM], center: &[f32], scale: &[f32]) -> [f32; FEATURE_DIM] {
    std::array::from_fn(|index| (features[index] - center[index]) / scale[index])
}

fn predict_manual(
    model: &EnsembleModel,
    queries: &[[f32; FEATURE_DIM]],
) -> Result<Vec<(f64, f64)>, SurrogateError> {
    if model.weights.is_empty() {
        return Err(SurrogateError::Model("model ensemble is empty".into()));
    }
    Ok(queries
        .iter()
        .map(|query| {
            let normalized = normalize(query, &model.center, &model.scale);
            summarize_ensemble(
                &model
                    .weights
                    .iter()
                    .map(|weights| sigmoid(dot_with_bias(weights, &normalized)))
                    .collect::<Vec<_>>(),
            )
        })
        .collect())
}

fn burn_predict_flex(
    model: &EnsembleModel,
    queries: &[[f32; FEATURE_DIM]],
) -> Result<Vec<(f64, f64)>, SurrogateError> {
    burn_predict::<Flex>(model, queries, &FlexDevice)
}

fn burn_predict_wgpu(
    model: &EnsembleModel,
    queries: &[[f32; FEATURE_DIM]],
) -> Result<Vec<(f64, f64)>, SurrogateError> {
    catch_unwind(AssertUnwindSafe(|| {
        burn_predict::<Vulkan>(model, queries, &WgpuDevice::default())
    }))
    .map_err(|_| SurrogateError::Model("WGPU backend panicked or lost its device".into()))?
}

fn burn_predict<B: Backend>(
    model: &EnsembleModel,
    queries: &[[f32; FEATURE_DIM]],
    device: &B::Device,
) -> Result<Vec<(f64, f64)>, SurrogateError> {
    if queries.is_empty() {
        return Ok(Vec::new());
    }
    let rows = queries
        .iter()
        .flat_map(|query| {
            let normalized = normalize(query, &model.center, &model.scale);
            normalized.into_iter().chain(std::iter::once(1.0))
        })
        .collect::<Vec<_>>();
    let weights = (0..=FEATURE_DIM)
        .flat_map(|feature| model.weights.iter().map(move |member| member[feature]))
        .collect::<Vec<_>>();
    let input = Tensor::<B, 1>::from_floats(rows.as_slice(), device)
        .reshape([queries.len(), FEATURE_DIM + 1]);
    let weights = Tensor::<B, 1>::from_floats(weights.as_slice(), device)
        .reshape([FEATURE_DIM + 1, model.weights.len()]);
    let values = activation::sigmoid(input.matmul(weights))
        .into_data()
        .into_vec::<f32>()
        .map_err(|error| SurrogateError::Model(error.to_string()))?;
    Ok(values
        .chunks_exact(model.weights.len())
        .map(summarize_ensemble)
        .collect())
}

fn summarize_ensemble(values: &[f32]) -> (f64, f64) {
    let mean =
        values.iter().map(|value| f64::from(*value)).sum::<f64>() / values.len().max(1) as f64;
    let variance = values
        .iter()
        .map(|value| (f64::from(*value) - mean).powi(2))
        .sum::<f64>()
        / values.len().max(1) as f64;
    (mean, variance.sqrt())
}

fn benchmark_model(model: &EnsembleModel) -> Result<DeviceProfile, SurrogateError> {
    let mut queries = Vec::with_capacity(8_192);
    for row in 0..8_192 {
        queries.push(std::array::from_fn(|column| {
            ((row * 37 + column * 17) % 1_009) as f32 / 1_008.0
        }));
    }
    let _ = burn_predict_flex(model, &queries[..64])?;
    let cpu_started = Instant::now();
    let cpu = burn_predict_flex(model, &queries)?;
    let cpu_seconds = cpu_started.elapsed().as_secs_f64().max(f64::EPSILON);
    let cpu_throughput = queries.len() as f64 / cpu_seconds;
    let gpu_attempt = burn_predict_wgpu(model, &queries[..64]).and_then(|_| {
        let started = Instant::now();
        let predictions = burn_predict_wgpu(model, &queries)?;
        Ok((
            predictions,
            started.elapsed().as_secs_f64().max(f64::EPSILON),
        ))
    });
    let (gpu_available, gpu_throughput, maximum_absolute_difference) = match gpu_attempt {
        Ok((gpu, seconds)) => {
            let maximum = cpu
                .iter()
                .zip(&gpu)
                .map(|(left, right)| (left.0 - right.0).abs())
                .reduce(f64::max)
                .unwrap_or_default();
            (true, queries.len() as f64 / seconds, maximum)
        }
        Err(_) => (false, 0.0, f64::INFINITY),
    };
    let calibration_gate = model.manifest.validation_log_loss < model.manifest.baseline_log_loss
        && model.manifest.validation_log_loss.is_finite();
    Ok(device_profile_from_measurements(
        gpu_available,
        cpu_throughput,
        gpu_throughput,
        maximum_absolute_difference,
        calibration_gate,
    ))
}

fn device_profile_from_measurements(
    gpu_available: bool,
    cpu_throughput: f64,
    gpu_throughput: f64,
    maximum_absolute_difference: f64,
    calibration_gate: bool,
) -> DeviceProfile {
    let speedup = gpu_throughput / cpu_throughput.max(f64::EPSILON);
    let quality_gain = if gpu_available && maximum_absolute_difference <= 1e-4 {
        speedup - 1.0
    } else {
        0.0
    };
    let gpu_enabled = gpu_available
        && speedup >= 2.0
        && calibration_gate
        && quality_gain >= 0.15
        && maximum_absolute_difference <= 1e-4;
    let decision = if !gpu_available {
        "No compatible WGPU device completed the benchmark; Burn Flex CPU is active.".into()
    } else if speedup < 2.0 {
        format!(
            "WGPU measured {speedup:.2}× CPU after transfer overhead; 2.00× is required, so Burn Flex CPU remains active."
        )
    } else if !calibration_gate {
        "The matchup model did not beat the screen-score log-loss baseline; learned inference is disabled regardless of device speed."
            .into()
    } else if quality_gain < 0.15 {
        format!(
            "WGPU closed-loop quality/minute estimate improved only {:.1}%; 15% is required.",
            quality_gain * 100.0
        )
    } else if maximum_absolute_difference > 1e-4 {
        "CPU/GPU prediction parity exceeded tolerance; Burn Flex CPU remains active.".into()
    } else {
        format!(
            "WGPU enabled: {speedup:.2}× CPU throughput after transfer overhead with {:.1}% estimated closed-loop quality/minute gain.",
            quality_gain * 100.0
        )
    };
    DeviceProfile {
        schema_version: MODEL_SCHEMA,
        benchmarked_at: unix_now(),
        cpu_backend: "burn-flex-0.21".into(),
        gpu_backend: "burn-wgpu-0.21".into(),
        gpu_available,
        cpu_predictions_per_second: cpu_throughput,
        gpu_predictions_per_second: gpu_throughput,
        speedup,
        maximum_absolute_difference,
        calibration_gate_passed: calibration_gate,
        closed_loop_quality_per_minute_gain: quality_gain,
        gpu_enabled,
        decision,
    }
}

fn out_of_distribution(model: &EnsembleModel, features: &[f32; FEATURE_DIM]) -> bool {
    let outside = features
        .iter()
        .enumerate()
        .filter(|(index, value)| {
            **value < model.minimum[*index] - 0.05 || **value > model.maximum[*index] + 0.05
        })
        .count();
    let maximum_z = features
        .iter()
        .enumerate()
        .map(|(index, value)| ((value - model.center[index]) / model.scale[index]).abs())
        .reduce(f32::max)
        .unwrap_or_default();
    outside * 4 > FEATURE_DIM || maximum_z > 5.0
}

fn load_labels(
    data_dir: &Path,
    platform: &str,
    format: &str,
    commander: bool,
    excluded_run_id: Option<&str>,
) -> Result<Vec<Label>, SurrogateError> {
    let connection = open(data_dir)?;
    let mut statement = connection.prepare(
        "SELECT lineage_id,field_revision,run_id,features,observed FROM training_labels
         WHERE platform=?1 AND format=?2 AND commander=?3
           AND capability_policy_revision=?4 AND rules_ir_revision=?5
         ORDER BY created_at DESC,id DESC LIMIT ?6",
    )?;
    let rows = statement.query_map(
        params![
            platform,
            format,
            commander,
            CAPABILITY_POLICY_REVISION,
            RULES_IR_REVISION,
            MAX_LABELS as i64
        ],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, f32>(4)?,
            ))
        },
    )?;
    let mut labels = Vec::new();
    for row in rows {
        let (lineage_id, field_revision, run_id, serialized, observed) = row?;
        if excluded_run_id.is_some_and(|excluded| run_id == excluded) {
            continue;
        }
        if let Ok(features) = serde_json::from_str::<[f32; FEATURE_DIM]>(&serialized) {
            labels.push(Label {
                lineage_id,
                field_revision,
                run_id,
                features,
                observed,
            });
        }
    }
    Ok(labels)
}

fn persist_model(data_dir: &Path, model: &EnsembleModel) -> Result<(), SurrogateError> {
    let root = model_directory(data_dir);
    fs::create_dir_all(&root)?;
    let current = root.join("surrogate-current.json");
    let rollback = root.join("surrogate-rollback.json");
    if current.is_file() {
        fs::copy(&current, &rollback)?;
    }
    let bytes = serde_json::to_vec(model)?;
    if bytes.len() as u64 > MAX_MODEL_BYTES {
        return Err(SurrogateError::Model(
            "serialized model exceeds the 256 MiB model budget".into(),
        ));
    }
    fs::write(&current, bytes)?;
    enforce_model_bounds(&root)?;
    Ok(())
}

fn load_model(data_dir: &Path) -> Result<Option<EnsembleModel>, SurrogateError> {
    let path = model_directory(data_dir).join("surrogate-current.json");
    if !path.is_file() {
        return Ok(None);
    }
    let model: EnsembleModel = serde_json::from_slice(&fs::read(path)?)?;
    if model.manifest.schema_version != MODEL_SCHEMA
        || model.center.len() != FEATURE_DIM
        || model
            .weights
            .iter()
            .any(|weights| weights.len() != FEATURE_DIM + 1)
    {
        return Ok(None);
    }
    Ok(Some(model))
}

fn persist_device_profile(data_dir: &Path, profile: &DeviceProfile) -> Result<(), SurrogateError> {
    let root = model_directory(data_dir);
    fs::create_dir_all(&root)?;
    fs::write(
        root.join("device-profile.json"),
        serde_json::to_vec_pretty(profile)?,
    )?;
    Ok(())
}

fn load_device_profile(data_dir: &Path) -> Result<Option<DeviceProfile>, SurrogateError> {
    let path = model_directory(data_dir).join("device-profile.json");
    if !path.is_file() {
        return Ok(None);
    }
    Ok(serde_json::from_slice(&fs::read(path)?).ok())
}

fn enforce_storage_bounds(data_dir: &Path, connection: &Connection) -> Result<(), SurrogateError> {
    connection.execute(
        "DELETE FROM training_labels WHERE id NOT IN
         (SELECT id FROM training_labels ORDER BY created_at DESC,id DESC LIMIT ?1)",
        [MAX_LABELS as i64],
    )?;
    if fs::metadata(database_path(data_dir)).map_or(0, |value| value.len()) > MAX_DATABASE_BYTES {
        connection.execute(
            "DELETE FROM training_labels WHERE id IN
             (SELECT id FROM training_labels ORDER BY created_at ASC,id ASC LIMIT 25000)",
            [],
        )?;
        connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    }
    Ok(())
}

fn enforce_model_bounds(root: &Path) -> Result<(), SurrogateError> {
    let current = root.join("surrogate-current.json");
    let rollback = root.join("surrogate-rollback.json");
    let current_size = fs::metadata(&current).map_or(0, |value| value.len());
    let rollback_size = fs::metadata(&rollback).map_or(0, |value| value.len());
    if current_size.saturating_add(rollback_size) > MAX_MODEL_BYTES && rollback.is_file() {
        fs::remove_file(rollback)?;
    }
    Ok(())
}

fn open(data_dir: &Path) -> Result<Connection, SurrogateError> {
    fs::create_dir_all(data_dir)?;
    let connection = Connection::open(database_path(data_dir))?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS training_labels(
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           platform TEXT NOT NULL, format TEXT NOT NULL, commander INTEGER NOT NULL,
           run_id TEXT NOT NULL, cycle INTEGER NOT NULL, candidate_rank INTEGER NOT NULL,
           lineage_id TEXT NOT NULL, field_revision TEXT NOT NULL,
           engine_revision TEXT NOT NULL, route_revision TEXT NOT NULL,
           capability_policy_revision TEXT NOT NULL DEFAULT '',
           rules_ir_revision TEXT NOT NULL DEFAULT '',
           card_data_revision TEXT NOT NULL, configuration_hash TEXT NOT NULL,
           play_draw TEXT NOT NULL, games INTEGER NOT NULL,
           features TEXT NOT NULL, observed REAL NOT NULL,
           created_at TEXT NOT NULL DEFAULT(datetime('now')),
           UNIQUE(run_id,cycle,candidate_rank,field_revision,configuration_hash)
         );
         CREATE INDEX IF NOT EXISTS training_scope
           ON training_labels(platform,format,commander,created_at DESC);
         CREATE INDEX IF NOT EXISTS training_group
           ON training_labels(lineage_id,field_revision);
         PRAGMA user_version=3;",
    )?;
    ensure_column(
        &connection,
        "training_labels",
        "capability_policy_revision",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    ensure_column(
        &connection,
        "training_labels",
        "rules_ir_revision",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    Ok(connection)
}

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), rusqlite::Error> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !names.iter().any(|name| name == column) {
        connection.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )?;
    }
    Ok(())
}

fn group_key(label: &Label) -> String {
    format!("{}:{}", label.lineage_id, label.field_revision)
}

fn stable_bucket(value: &str, buckets: usize) -> usize {
    let digest = Sha256::digest(value.as_bytes());
    u64::from_le_bytes(digest[..8].try_into().expect("eight digest bytes")) as usize
        % buckets.max(1)
}

fn dot_with_bias(weights: &[f32], features: &[f32; FEATURE_DIM]) -> f32 {
    weights[..FEATURE_DIM]
        .iter()
        .zip(features)
        .map(|(weight, feature)| weight * feature)
        .sum::<f32>()
        + weights[FEATURE_DIM]
}

fn sigmoid(value: f32) -> f32 {
    1.0 / (1.0 + (-value.clamp(-30.0, 30.0)).exp())
}

fn binary_log_loss(predicted: f64, observed: f64) -> f64 {
    let predicted = predicted.clamp(1e-6, 1.0 - 1e-6);
    let observed = observed.clamp(0.0, 1.0);
    -(observed * predicted.ln() + (1.0 - observed) * (1.0 - predicted).ln())
}

fn model_revision(model: &EnsembleModel) -> Result<String, SurrogateError> {
    let mut copy = model.clone();
    copy.manifest.model_revision.clear();
    Ok(format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&copy)?)
    ))
}

fn saturating_i64(value: usize) -> i64 {
    value.min(i64::MAX as usize) as i64
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn database_path(data_dir: &Path) -> PathBuf {
    data_dir.join("surrogate.sqlite")
}

fn model_directory(data_dir: &Path) -> PathBuf {
    data_dir.join("models")
}

#[cfg(test)]
mod tests {
    use super::*;
    use metagene_core::{Deck, Fitness};
    use uuid::Uuid;

    fn candidate(rank: usize, score: f64) -> RankedDeck {
        RankedDeck {
            rank,
            pareto_rank: 0,
            deck: Deck {
                name: format!("deck-{rank}"),
                command_zone: Vec::new(),
                mainboard: Vec::new(),
                sideboard: Vec::new(),
            },
            fitness: Fitness {
                score,
                consistency: score,
                closing_power: score,
                ..Fitness::default()
            },
            packages: Vec::new(),
            commander_compatibility: None,
            candidate_status: "provisional".into(),
        }
    }

    fn observation<'a>(
        run_id: &'a str,
        cycle: usize,
        candidate: &'a RankedDeck,
    ) -> SurrogateObservation<'a> {
        SurrogateObservation {
            platform: "arena",
            format: "historic",
            commander: false,
            run_id,
            cycle,
            candidate,
            observed: 0.6,
            lineage_id: "lineage",
            field_revision: "field",
            engine_revision: "engine",
            route_revision: "route",
            capability_policy_revision: CAPABILITY_POLICY_REVISION,
            rules_ir_revision: RULES_IR_REVISION,
            card_data_revision: "cards",
            configuration_hash: "configuration",
            play_draw: "rotated",
            games: 64,
            expected_games: 64,
            field_coverage: 1.0,
            candidate_critical_coverage: 1.0,
            opponent_count: 8,
            completed: true,
            holdout: true,
            crashed: false,
            timed_out: false,
            disputed_critical_mechanic: false,
            unresolved_critical_clause: false,
        }
    }

    #[test]
    fn only_complete_provenance_rich_holdouts_are_persisted() {
        let root = std::env::temp_dir().join(format!("metagene-surrogate-{}", Uuid::new_v4()));
        let candidate = candidate(1, 0.8);
        let mut rejected = observation("run", 1, &candidate);
        rejected.crashed = true;
        assert!(!record(&root, rejected).unwrap());
        let mut incomplete_candidate = observation("run", 2, &candidate);
        incomplete_candidate.candidate_critical_coverage = 0.99;
        assert!(!record(&root, incomplete_candidate).unwrap());
        let mut unresolved = observation("run", 3, &candidate);
        unresolved.unresolved_critical_clause = true;
        assert!(!record(&root, unresolved).unwrap());
        assert!(record(&root, observation("run", 4, &candidate)).unwrap());
        let status = status(&root, "arena", "historic", false).unwrap();
        assert_eq!(status.labels, 1);
        assert_eq!(status.minimum_labels, 5_000);
        assert!(!status.ready);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sparse_models_never_filter_candidates() {
        let root = std::env::temp_dir().join(format!("metagene-surrogate-{}", Uuid::new_v4()));
        let candidates = vec![candidate(1, 0.8), candidate(2, 0.5), candidate(3, 0.2)];
        for index in 0..20 {
            let run = format!("run-{index}");
            record(&root, observation(&run, index, &candidates[index % 3])).unwrap();
        }
        let context = ScreeningContext {
            platform: "arena",
            format: "historic",
            commander: false,
            active_run_id: "active",
            field_revision: "field",
            opponent_count: 8,
            field_coverage: 1.0,
        };
        let (screened, report) = screen(&root, &context, &candidates).unwrap();
        assert_eq!(screened.len(), candidates.len());
        assert!(!report.model_used);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn group_isolated_training_produces_uncertainty_and_ood_fallback() {
        let mut labels = Vec::new();
        for index in 0..240 {
            let value = (index % 100) as f32 / 100.0;
            labels.push(Label {
                lineage_id: format!("lineage-{}", index % 12),
                field_revision: format!("field-{}", index % 3),
                run_id: format!("run-{index}"),
                features: [value; FEATURE_DIM],
                observed: (0.15 + value * 0.7).clamp(0.0, 1.0),
            });
        }
        let model = train_ensemble(&labels, "arena", "historic", false, None).unwrap();
        assert!(model.manifest.group_isolated);
        assert!(model.manifest.validation_groups > 0);
        let predictions = burn_predict_flex(&model, &[[0.5; FEATURE_DIM]]).unwrap();
        assert_eq!(predictions.len(), 1);
        assert!(predictions[0].0.is_finite());
        assert!(!out_of_distribution(&model, &[0.5; FEATURE_DIM]));
        assert!(out_of_distribution(&model, &[1.5; FEATURE_DIM]));
    }

    #[test]
    fn training_is_reproducible_and_never_splits_one_lineage_across_groups() {
        assert_eq!(MIN_LABELS, 5_000);
        let labels = (0..240)
            .map(|index| Label {
                lineage_id: format!("lineage-{}", index % 12),
                field_revision: format!("field-{}", index % 3),
                run_id: format!("run-{index}"),
                features: [index as f32 / 240.0; FEATURE_DIM],
                observed: (index % 2) as f32,
            })
            .collect::<Vec<_>>();
        let first = train_ensemble(&labels, "arena", "historic", false, None).unwrap();
        let second = train_ensemble(&labels, "arena", "historic", false, None).unwrap();
        assert_eq!(first.weights, second.weights);
        assert!(first.manifest.group_isolated);
        let single_group = labels
            .iter()
            .cloned()
            .map(|mut label| {
                label.lineage_id = "only-lineage".into();
                label.field_revision = "only-field".into();
                label
            })
            .collect::<Vec<_>>();
        assert!(
            train_ensemble(&single_group, "arena", "historic", false, None)
                .unwrap_err()
                .to_string()
                .contains("two independent")
        );
    }

    #[test]
    fn gpu_gate_enforces_speed_calibration_quality_and_cpu_parity() {
        let unavailable = device_profile_from_measurements(false, 100.0, 0.0, f64::INFINITY, true);
        assert!(!unavailable.gpu_enabled);
        let slow = device_profile_from_measurements(true, 100.0, 190.0, 0.0, true);
        assert!(!slow.gpu_enabled);
        let uncalibrated = device_profile_from_measurements(true, 100.0, 250.0, 0.0, false);
        assert!(!uncalibrated.gpu_enabled);
        let divergent = device_profile_from_measurements(true, 100.0, 250.0, 0.01, true);
        assert!(!divergent.gpu_enabled);
        let eligible = device_profile_from_measurements(true, 100.0, 250.0, 1e-6, true);
        assert!(eligible.gpu_enabled);
        assert!(eligible.closed_loop_quality_per_minute_gain >= 0.15);
    }

    #[test]
    fn available_wgpu_device_matches_cpu_predictions() {
        let labels = (0..240)
            .map(|index| Label {
                lineage_id: format!("lineage-{}", index % 12),
                field_revision: format!("field-{}", index % 3),
                run_id: format!("run-{index}"),
                features: [index as f32 / 240.0; FEATURE_DIM],
                observed: (index % 2) as f32,
            })
            .collect::<Vec<_>>();
        let model = train_ensemble(&labels, "arena", "historic", false, None).unwrap();
        let queries = [[0.25; FEATURE_DIM], [0.75; FEATURE_DIM]];
        let cpu = burn_predict_flex(&model, &queries).unwrap();
        let Ok(gpu) = burn_predict_wgpu(&model, &queries) else {
            // Device loss/absence is a supported state and is separately
            // required to fall back to Flex CPU.
            return;
        };
        let maximum = cpu
            .iter()
            .zip(gpu)
            .map(|(left, right)| (left.0 - right.0).abs())
            .reduce(f64::max)
            .unwrap_or_default();
        assert!(maximum <= 1e-4, "CPU/GPU difference was {maximum}");
    }

    #[test]
    fn active_run_labels_are_excluded_from_screening_accounting() {
        let root = std::env::temp_dir().join(format!("metagene-surrogate-{}", Uuid::new_v4()));
        let candidate = candidate(1, 0.5);
        record(&root, observation("active", 1, &candidate)).unwrap();
        let context = ScreeningContext {
            platform: "arena",
            format: "historic",
            commander: false,
            active_run_id: "active",
            field_revision: "field",
            opponent_count: 8,
            field_coverage: 1.0,
        };
        let (_, report) = screen(&root, &context, &[candidate.clone(), candidate]).unwrap();
        assert_eq!(report.active_run_labels_excluded, 1);
        fs::remove_dir_all(root).unwrap();
    }
}
