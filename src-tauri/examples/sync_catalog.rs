use metagene_app_lib::catalog;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: sync_catalog <isolated-application-data-directory>")?;
    let mut last_stage = String::new();
    let status = catalog::sync(&data_dir, |progress| {
        if progress.stage != last_stage {
            eprintln!("{}", progress.stage);
            last_stage = progress.stage;
        }
    })?;
    println!(
        "catalog v{} · {} logical cards · {} bytes",
        status.catalog_version, status.card_count, status.database_bytes
    );
    Ok(())
}
