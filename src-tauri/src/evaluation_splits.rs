use metagene_core::Deck;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

const POLICY_JSON: &str = include_str!("../../data/evaluation/splits.schema1.json");

#[derive(Debug, Deserialize)]
struct SplitPolicy {
    schema_version: u32,
    policy_id: String,
    train_share: f64,
    validation_share: f64,
    holdout_share: f64,
    train_seed_offset: u64,
    validation_seed_offset: u64,
    holdout_seed_offset: u64,
    minimum_distinct_field_size: usize,
    target_interval_coverage: f64,
    notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct EvidenceSplitPlan {
    pub schema_version: u32,
    pub policy_id: String,
    pub root_seed: u64,
    pub train_seed: u64,
    pub validation_seed: u64,
    pub holdout_seed: u64,
    pub train_opponents: Vec<Deck>,
    pub validation_opponents: Vec<Deck>,
    pub holdout_opponents: Vec<Deck>,
    pub opponents_reused: bool,
    pub target_interval_coverage: f64,
    pub notes: Vec<String>,
}

pub fn plan(opponents: &[Deck], commander: bool, root_seed: u64) -> EvidenceSplitPlan {
    let policy = policy();
    let mut ordered = opponents
        .iter()
        .cloned()
        .map(|deck| {
            let hash = crate::provenance::configuration_hash("evaluation-opponent-v1", &deck)
                .unwrap_or_default();
            (hash, deck)
        })
        .collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    let ordered = ordered
        .into_iter()
        .map(|(_, deck)| deck)
        .collect::<Vec<_>>();
    let can_partition = !commander && ordered.len() >= policy.minimum_distinct_field_size;
    let (train_opponents, validation_opponents, holdout_opponents, opponents_reused) =
        if can_partition {
            let holdout_count = ((ordered.len() as f64 * policy.holdout_share).round() as usize)
                .clamp(1, ordered.len() - 2);
            let validation_count = ((ordered.len() as f64 * policy.validation_share).round()
                as usize)
                .clamp(1, ordered.len() - holdout_count - 1);
            let train_count = ordered.len() - validation_count - holdout_count;
            (
                ordered[..train_count].to_vec(),
                ordered[train_count..train_count + validation_count].to_vec(),
                ordered[train_count + validation_count..].to_vec(),
                false,
            )
        } else {
            (ordered.clone(), ordered.clone(), ordered, true)
        };
    debug_assert!(
        (policy.train_share + policy.validation_share + policy.holdout_share - 1.0).abs() < 1e-9
    );
    EvidenceSplitPlan {
        schema_version: policy.schema_version,
        policy_id: policy.policy_id.clone(),
        root_seed,
        train_seed: root_seed ^ policy.train_seed_offset,
        validation_seed: root_seed ^ policy.validation_seed_offset,
        holdout_seed: root_seed ^ policy.holdout_seed_offset,
        train_opponents,
        validation_opponents,
        holdout_opponents,
        opponents_reused,
        target_interval_coverage: policy.target_interval_coverage,
        notes: policy.notes.clone(),
    }
}

pub fn target_interval_coverage() -> f64 {
    policy().target_interval_coverage
}

fn policy() -> &'static SplitPolicy {
    static POLICY: OnceLock<SplitPolicy> = OnceLock::new();
    POLICY.get_or_init(|| {
        let policy: SplitPolicy = serde_json::from_str(POLICY_JSON)
            .expect("embedded evaluation split policy must be valid JSON");
        assert_eq!(
            policy.schema_version, 1,
            "unsupported evaluation split schema"
        );
        assert!(!policy.policy_id.trim().is_empty());
        assert!(policy.minimum_distinct_field_size >= 3);
        assert!(
            (policy.train_share + policy.validation_share + policy.holdout_share - 1.0).abs()
                < 1e-9
        );
        policy
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deck(name: &str) -> Deck {
        Deck {
            name: name.into(),
            command_zone: Vec::new(),
            mainboard: Vec::new(),
            sideboard: Vec::new(),
        }
    }

    #[test]
    fn constructed_fields_partition_stably_and_small_or_commander_fields_reuse_only_opponents() {
        let field = vec![deck("A"), deck("B"), deck("C"), deck("D"), deck("E")];
        let first = plan(&field, false, 42);
        let mut reversed = field.clone();
        reversed.reverse();
        let second = plan(&reversed, false, 42);
        assert_eq!(first.train_opponents, second.train_opponents);
        assert_eq!(first.validation_opponents, second.validation_opponents);
        assert_eq!(first.holdout_opponents, second.holdout_opponents);
        assert_eq!(first.train_opponents.len(), 3);
        assert_eq!(first.validation_opponents.len(), 1);
        assert_eq!(first.holdout_opponents.len(), 1);
        assert!(!first.opponents_reused);
        assert_ne!(first.train_seed, first.validation_seed);
        assert_ne!(first.validation_seed, first.holdout_seed);

        let commander = plan(&field, true, 42);
        assert!(commander.opponents_reused);
        assert_eq!(commander.validation_opponents.len(), 5);
        let small = plan(&field[..2], false, 42);
        assert!(small.opponents_reused);
        assert_eq!(small.holdout_opponents.len(), 2);
    }
}
