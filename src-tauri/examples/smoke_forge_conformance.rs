use metagene_app_lib::{conformance_contract, forge};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is not defined")?
        .join("dev.metagene.app");
    let suite = conformance_contract::parse_scenario_suite(include_str!(
        "../../data/conformance/scenarios.schema1.json"
    ))?;
    let observation = forge::run_conformance_scenario(&data_dir, &suite.scenarios[0])?;
    if !observation.passed {
        return Err("Forge failed the Parallel Lives/Mythos conformance scenario".into());
    }
    println!(
        "engine={} revision={} scenario={} seed={} passed={} event_hash={} outcome={}",
        observation.engine,
        observation.engine_revision,
        observation.scenario_id,
        observation.seed,
        observation.passed,
        observation.event_hash,
        observation.outcome
    );
    Ok(())
}
