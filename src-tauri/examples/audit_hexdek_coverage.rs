use metagene_app_lib::{catalog, simulator};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is not defined")?
        .join("dev.metagene.app");
    for (platform, format) in [("arena", "historic"), ("paper", "commander")] {
        let cards = catalog::legal_pool(&data_dir, platform, format)?;
        let input = simulator::CoverageDeckInput {
            name: format!("{platform} {format} legal pool"),
            cards: cards
                .iter()
                .map(|card| simulator::CoverageCardInput {
                    name: card.name.clone(),
                    quantity: 1,
                    zone: "catalog".into(),
                })
                .collect(),
        };
        let report = simulator::audit_hexdek_coverage(&data_dir, &[input])?;
        let coverage = &report.decks[0];
        println!(
            "{} {}: {} unique, {} present ({:.2}%), {} parser-complete ({:.2}%), {} partial, {} missing, {} semantic-risk ({:.2}% of copies)",
            platform,
            format,
            coverage.unique_cards,
            coverage.present_unique,
            coverage.present_ratio * 100.0,
            coverage.parser_complete_unique,
            coverage.parser_complete_ratio * 100.0,
            coverage.partial_unique,
            coverage.missing_unique,
            coverage.semantic_risk_unique,
            coverage.semantic_risk_ratio * 100.0,
        );
        for status in ["missing", "partial", "semantic_risk"] {
            for issue in coverage
                .issues
                .iter()
                .filter(|issue| issue.status == status)
                .take(5)
            {
                println!(
                    "  {:13} {}{}",
                    issue.status,
                    issue.name,
                    issue
                        .parse_errors
                        .first()
                        .map(|error| format!(" — {error}"))
                        .unwrap_or_default()
                );
            }
        }
    }
    Ok(())
}
