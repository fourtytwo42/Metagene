use metagene_app_lib::catalog;
use std::path::PathBuf;

fn main() {
    let data_dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .expect("usage: search_catalog <application-data-directory> [query]");
    let query = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "Parallel Lives".into());
    let platform = std::env::args().nth(3).unwrap_or_else(|| "arena".into());
    let format = std::env::args().nth(4).unwrap_or_else(|| "historic".into());
    let cards =
        catalog::search(&data_dir, &query, &platform, &format, 10).expect("catalog search failed");
    for card in cards {
        println!(
            "{} ({}) {} arena_id={:?} art={}",
            card.name,
            card.set_code.to_uppercase(),
            card.collector_number,
            card.arena_id,
            card.image_uri.is_some()
        );
    }
}
