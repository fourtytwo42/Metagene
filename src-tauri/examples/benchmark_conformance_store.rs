use metagene_app_lib::conformance::{
    ConformanceBundle, ConformanceObservation, audit_cards, import_bundle,
};
use serde_json::json;
use std::time::Instant;
use uuid::Uuid;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory =
        std::env::temp_dir().join(format!("metagene-conformance-benchmark-{}", Uuid::new_v4()));
    let observations = (0..5_000i64)
        .flat_map(|seed| {
            ["hexdek", "forge"].map(move |engine| ConformanceObservation {
                engine: engine.into(),
                engine_revision: "benchmark-revision".into(),
                provenance: None,
                scenario_id: "tokens.parallel-lives".into(),
                scenario_version: 1,
                seed,
                passed: true,
                outcome: json!({"created_tokens": 2, "parallel_lives_permanents": 3}),
                event_hash: format!("sha256:benchmark-{seed:016x}"),
                cards: vec!["Parallel Lives".into(), "Mythos of Illuna".into()],
                claims: Vec::new(),
            })
        })
        .collect::<Vec<_>>();
    let started = Instant::now();
    let imported = import_bundle(
        &directory,
        &ConformanceBundle {
            schema_version: 1,
            producer: "metagene storage benchmark".into(),
            observations,
        },
    )?;
    let import_elapsed = started.elapsed();
    let audit_started = Instant::now();
    let audit = audit_cards(
        &directory,
        &["Parallel Lives".into(), "Mythos of Illuna".into()],
    )?;
    let audit_elapsed = audit_started.elapsed();
    if imported.summary.observations != 10_000
        || imported.summary.cross_engine_verified_scenarios != 5_000
        || audit.cross_engine_verified_unique != 2
    {
        return Err("conformance benchmark verification failed".into());
    }
    println!(
        "observations={} scenarios={} bytes={} import_ms={} audit_ms={} bytes_per_observation={:.1}",
        imported.summary.observations,
        imported.summary.scenario_runs,
        imported.summary.database_bytes,
        import_elapsed.as_millis(),
        audit_elapsed.as_millis(),
        imported.summary.database_bytes as f64 / imported.summary.observations as f64,
    );
    std::fs::remove_dir_all(directory)?;
    Ok(())
}
