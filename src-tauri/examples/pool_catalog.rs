use metagene_app_lib::catalog;
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let data_dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .expect("usage: pool_catalog <application-data-directory> [platform] [format]");
    let platform = std::env::args().nth(2).unwrap_or_else(|| "arena".into());
    let format = std::env::args().nth(3).unwrap_or_else(|| "historic".into());
    let started = Instant::now();
    let cards = catalog::legal_pool(&data_dir, &platform, &format).expect("pool query failed");
    println!(
        "{} {}: {} unique legal cards in {:.2}s",
        platform,
        format,
        cards.len(),
        started.elapsed().as_secs_f64()
    );
}
