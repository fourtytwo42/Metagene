use metagene_app_lib::simulator;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is not defined")?
        .join("dev.metagene.app");
    let status = simulator::install_hexdek(&data_dir, |progress| {
        if let Some(total) = progress.total_units {
            eprintln!(
                "[{}] {} ({}/{})",
                progress.stage, progress.detail, progress.completed_units, total
            );
        } else {
            eprintln!("[{}] {}", progress.stage, progress.detail);
        }
    })?;
    println!(
        "{} installed={} data_ready={} path={}",
        status.name, status.installed, status.data_ready, status.executable
    );
    Ok(())
}
