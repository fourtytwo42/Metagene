use metagene_app_lib::{conformance, conformance_contract, forge, hexdek};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is not defined")?
        .join("dev.metagene.app");
    let suite = conformance_contract::parse_scenario_suite(include_str!(
        "../../data/conformance/scenarios.schema1.json"
    ))?;
    let mut observations = Vec::with_capacity(suite.scenarios.len() * 2);
    for scenario in &suite.scenarios {
        let forge_observation = forge::run_conformance_scenario(&data_dir, scenario)?;
        let hexdek_observation = hexdek::run_conformance_scenario(&data_dir, scenario)?;
        let comparison =
            conformance::compare_observations(&forge_observation, &hexdek_observation)?;
        println!(
            "scenario={} seed={} forge_passed={} hexdek_passed={} forge_hash={} hexdek_hash={} classification={:?} outcome_matches={} event_hash_matches={}",
            scenario.id,
            scenario.seed,
            forge_observation.passed,
            hexdek_observation.passed,
            forge_observation.event_hash,
            hexdek_observation.event_hash,
            comparison.classification,
            comparison.outcome_matches,
            comparison.event_hash_matches,
        );
        let expected = if matches!(
            scenario.id.as_str(),
            "layers.clone-copies-base-before-anthem"
                | "arena.oracle-of-the-alpha-conjures-power-nine"
        ) {
            conformance::ComparisonClassification::AssertionFailure
        } else {
            conformance::ComparisonClassification::VerifiedAgreement
        };
        if comparison.classification != expected {
            return Err(format!(
                "unexpected classification for {}: {:?}; expected {:?}",
                scenario.id, comparison.classification, expected
            )
            .into());
        }
        observations.push(forge_observation);
        observations.push(hexdek_observation);
    }
    let imported = conformance::import_bundle(
        &data_dir,
        &conformance::ConformanceBundle {
            schema_version: conformance::CONFORMANCE_SCHEMA_VERSION,
            producer: "metagene-live-cross-engine-smoke".into(),
            observations,
        },
    )?;
    println!(
        "persisted={} cross_engine_verified={} conflicting={} database_bytes={}",
        imported.imported_observations,
        imported.summary.cross_engine_verified_scenarios,
        imported.summary.conflicting_scenarios,
        imported.summary.database_bytes
    );
    Ok(())
}
