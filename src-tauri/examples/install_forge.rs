use metagene_app_lib::forge;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is not defined")?
        .join("dev.metagene.app");
    let status = forge::install(&data_dir, |progress| {
        println!(
            "stage={} completed={} total={} detail={}",
            progress.stage,
            progress.completed_units,
            progress
                .total_units
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".into()),
            progress.detail
        );
    })?;
    println!(
        "forge installed={} ready={} runtime={}",
        status.installed, status.data_ready, status.executable
    );
    Ok(())
}
