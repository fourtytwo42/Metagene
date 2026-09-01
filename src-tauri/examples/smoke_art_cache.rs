use metagene_app_lib::{art_cache, catalog};
use std::path::{Path, PathBuf};
use std::time::Instant;
use uuid::Uuid;

fn smoke(cache_root: &Path, catalog_root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let card = catalog::search(catalog_root, "Parallel Lives", "arena", "historic", 10)?
        .into_iter()
        .find(|card| card.name == "Parallel Lives" && card.image_uri.is_some())
        .ok_or("Parallel Lives with art was not found in the Arena Historic catalog")?;
    let image_uri = card.image_uri.as_deref().expect("checked above");

    let cold_start = Instant::now();
    let cold = art_cache::get(cache_root, image_uri)?;
    let cold_elapsed = cold_start.elapsed();
    let hot_start = Instant::now();
    let hot = art_cache::get(cache_root, image_uri)?;
    let hot_elapsed = hot_start.elapsed();
    let status = art_cache::status(cache_root)?;

    assert_eq!(cold, hot);
    assert!(
        cold.starts_with(&[0xff, 0xd8, 0xff]),
        "expected JPEG card art"
    );
    assert_eq!(status.entries, 1);
    assert_eq!(status.bytes, cold.len() as u64);
    println!(
        "card={} bytes={} cold_ms={} hot_ms={} entries={} ceiling_mib={}",
        card.name,
        cold.len(),
        cold_elapsed.as_millis(),
        hot_elapsed.as_millis(),
        status.entries,
        status.max_bytes / 1024 / 1024
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let catalog_root = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is unavailable")?
        .join("dev.metagene.app");
    let cache_root = std::env::temp_dir().join(format!("metagene-art-smoke-{}", Uuid::new_v4()));
    let result = smoke(&cache_root, &catalog_root);
    let _ = std::fs::remove_dir_all(&cache_root);
    result
}
