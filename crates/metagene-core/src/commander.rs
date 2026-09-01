use crate::Card;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

const POLICY_JSON: &str = include_str!("../../../data/commander/compatibility.schema1.json");
const CONFORMANCE_JSON: &str =
    include_str!("../../../data/commander/command-zone-conformance.schema1.json");

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommanderCompatibilityPolicy {
    pub schema_version: u32,
    pub policy_id: String,
    pub rules: Vec<CommanderCompatibilityRule>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommanderCompatibilityRule {
    SharedOracleLine {
        id: String,
        line: String,
        #[serde(default)]
        allow_reminder: bool,
    },
    NamedOraclePrefix {
        id: String,
        prefix: String,
    },
    OracleLineWithOtherType {
        id: String,
        line: String,
        other_type_contains: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommanderCompatibilityEvidence {
    pub policy_id: String,
    pub schema_version: u32,
    pub rule_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommanderConformanceReport {
    pub schema_version: u32,
    pub policy_id: String,
    pub passed: usize,
    pub failed: usize,
    pub covered_rules: Vec<String>,
    pub cases: Vec<CommanderConformanceResult>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommanderConformanceResult {
    pub id: String,
    pub expected_rule: Option<String>,
    pub actual_rule: Option<String>,
    pub passed: bool,
}

#[derive(Deserialize)]
struct CommanderConformanceSuite {
    schema_version: u32,
    policy_id: String,
    cases: Vec<CommanderConformanceCase>,
}

#[derive(Deserialize)]
struct CommanderConformanceCase {
    id: String,
    left: CommanderFixtureCard,
    right: CommanderFixtureCard,
    expected_rule: Option<String>,
}

#[derive(Deserialize)]
struct CommanderFixtureCard {
    name: String,
    type_line: String,
    oracle_text: String,
}

pub fn commander_compatibility_policy() -> &'static CommanderCompatibilityPolicy {
    static POLICY: OnceLock<CommanderCompatibilityPolicy> = OnceLock::new();
    POLICY.get_or_init(|| {
        let policy: CommanderCompatibilityPolicy = serde_json::from_str(POLICY_JSON)
            .expect("embedded Commander policy must be valid JSON");
        validate_policy(&policy).expect("embedded Commander policy must be valid");
        policy
    })
}

pub fn commander_compatibility(
    left: &Card,
    right: &Card,
) -> Option<CommanderCompatibilityEvidence> {
    let policy = commander_compatibility_policy();
    for rule in &policy.rules {
        if rule.matches(left, right) {
            return Some(CommanderCompatibilityEvidence {
                policy_id: policy.policy_id.clone(),
                schema_version: policy.schema_version,
                rule_id: rule.id().into(),
            });
        }
    }
    None
}

pub fn commanders_compatible(left: &Card, right: &Card) -> bool {
    commander_compatibility(left, right).is_some()
}

pub fn commander_conformance_report() -> CommanderConformanceReport {
    let suite: CommanderConformanceSuite =
        serde_json::from_str(CONFORMANCE_JSON).expect("embedded Commander conformance must parse");
    let policy = commander_compatibility_policy();
    assert_eq!(
        suite.schema_version, 1,
        "unsupported Commander conformance schema"
    );
    assert_eq!(
        suite.policy_id, policy.policy_id,
        "conformance policy mismatch"
    );
    let mut covered_rules = std::collections::BTreeSet::new();
    let cases = suite
        .cases
        .into_iter()
        .map(|case| {
            let actual_rule =
                commander_compatibility(&fixture_card(case.left), &fixture_card(case.right))
                    .map(|evidence| evidence.rule_id);
            if let Some(rule) = &actual_rule {
                covered_rules.insert(rule.clone());
            }
            let passed = actual_rule == case.expected_rule;
            CommanderConformanceResult {
                id: case.id,
                expected_rule: case.expected_rule,
                actual_rule,
                passed,
            }
        })
        .collect::<Vec<_>>();
    let passed = cases.iter().filter(|case| case.passed).count();
    CommanderConformanceReport {
        schema_version: suite.schema_version,
        policy_id: suite.policy_id,
        passed,
        failed: cases.len().saturating_sub(passed),
        covered_rules: covered_rules.into_iter().collect(),
        cases,
    }
}

fn fixture_card(fixture: CommanderFixtureCard) -> Card {
    Card {
        id: crate::CardId(fixture.name.clone()),
        arena_id: None,
        platforms: vec!["paper".into()],
        name: fixture.name,
        set_code: "fixture".into(),
        collector_number: "1".into(),
        type_line: fixture.type_line,
        oracle_text: fixture.oracle_text,
        colors: Vec::new(),
        color_identity: Vec::new(),
        image_uri: None,
        legalities: std::collections::BTreeMap::new(),
        features: crate::CardFeatures::default(),
    }
}

impl CommanderCompatibilityRule {
    fn id(&self) -> &str {
        match self {
            Self::SharedOracleLine { id, .. }
            | Self::NamedOraclePrefix { id, .. }
            | Self::OracleLineWithOtherType { id, .. } => id,
        }
    }

    fn matches(&self, left: &Card, right: &Card) -> bool {
        let left_oracle = left.oracle_text.to_ascii_lowercase();
        let right_oracle = right.oracle_text.to_ascii_lowercase();
        match self {
            Self::SharedOracleLine {
                line,
                allow_reminder,
                ..
            } => {
                oracle_has_line(&left_oracle, line, *allow_reminder)
                    && oracle_has_line(&right_oracle, line, *allow_reminder)
            }
            Self::NamedOraclePrefix { prefix, .. } => {
                oracle_names_other(&left_oracle, prefix, &right.name)
                    || oracle_names_other(&right_oracle, prefix, &left.name)
            }
            Self::OracleLineWithOtherType {
                line,
                other_type_contains,
                ..
            } => {
                (oracle_has_line(&left_oracle, line, true)
                    && right
                        .type_line
                        .to_ascii_lowercase()
                        .contains(other_type_contains))
                    || (oracle_has_line(&right_oracle, line, true)
                        && left
                            .type_line
                            .to_ascii_lowercase()
                            .contains(other_type_contains))
            }
        }
    }
}

fn oracle_has_line(oracle: &str, expected: &str, allow_reminder: bool) -> bool {
    let expected = expected.to_ascii_lowercase();
    oracle.lines().any(|line| {
        let line = line.trim();
        line == expected || allow_reminder && line.starts_with(&format!("{expected} ("))
    })
}

fn oracle_names_other(oracle: &str, prefix: &str, other_name: &str) -> bool {
    let expected = format!(
        "{}{}",
        prefix.to_ascii_lowercase(),
        other_name.to_ascii_lowercase()
    );
    oracle.lines().any(|line| {
        let line = line.trim();
        line == expected || line.starts_with(&format!("{expected} ("))
    })
}

fn validate_policy(policy: &CommanderCompatibilityPolicy) -> Result<(), String> {
    if policy.schema_version != 1 || policy.policy_id.trim().is_empty() || policy.rules.is_empty() {
        return Err("policy header is incomplete".into());
    }
    let mut ids = std::collections::HashSet::new();
    for rule in &policy.rules {
        if rule.id().trim().is_empty() || !ids.insert(rule.id()) {
            return Err("policy rule ids must be non-empty and unique".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CardFeatures, CardId, Color};
    use std::collections::BTreeMap;

    fn card(name: &str, type_line: &str, oracle: &str) -> Card {
        Card {
            id: CardId(name.into()),
            arena_id: None,
            platforms: vec!["paper".into()],
            name: name.into(),
            set_code: "tst".into(),
            collector_number: "1".into(),
            type_line: type_line.into(),
            oracle_text: oracle.into(),
            colors: vec![Color::Blue],
            color_identity: vec![Color::Blue],
            image_uri: None,
            legalities: BTreeMap::new(),
            features: CardFeatures::default(),
        }
    }

    #[test]
    fn embedded_policy_is_versioned_and_explains_every_supported_variant() {
        assert_eq!(commander_compatibility_policy().schema_version, 1);
        for (left, right, expected) in [
            (
                card("A", "Legendary Creature", "Partner"),
                card("B", "Legendary Creature", "Partner"),
                "partner",
            ),
            (
                card("A", "Legendary Creature", "Partner with B"),
                card("B", "Legendary Creature", "Partner with A"),
                "partner-with",
            ),
            (
                card("A", "Legendary Creature", "Friends forever"),
                card("B", "Legendary Creature", "Friends forever"),
                "friends-forever",
            ),
            (
                card("A", "Legendary Creature", "Choose a Background"),
                card("Lore", "Legendary Enchantment — Background", ""),
                "choose-a-background",
            ),
            (
                card("Companion", "Legendary Creature", "Doctor's companion"),
                card("Doctor", "Legendary Creature — Time Lord Doctor", ""),
                "doctors-companion",
            ),
        ] {
            assert_eq!(
                commander_compatibility(&left, &right).unwrap().rule_id,
                expected
            );
        }
    }

    #[test]
    fn ordinary_legends_and_substring_false_positives_are_rejected() {
        assert!(
            commander_compatibility(
                &card("A", "Legendary Creature", "Partners gain flying."),
                &card("B", "Legendary Creature", "Partner")
            )
            .is_none()
        );
    }

    #[test]
    fn versioned_conformance_covers_every_policy_rule_and_negative_case() {
        let report = commander_conformance_report();
        assert_eq!(report.failed, 0);
        assert_eq!(report.passed, 11);
        assert_eq!(
            report.covered_rules.len(),
            commander_compatibility_policy().rules.len()
        );
        assert!(report.cases.iter().any(|case| case.expected_rule.is_none()));
    }
}
