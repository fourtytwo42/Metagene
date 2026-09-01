use rusqlite::{Connection, params};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

const MAX_OBSERVATIONS: usize = 2_000;

#[derive(Debug, Error)]
pub enum CalibrationError {
    #[error("failed to access calibration storage: {0}")]
    Io(#[from] std::io::Error),
    #[error("calibration database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("calibration scores must be finite")]
    InvalidScore,
}

pub struct CalibrationObservation<'a> {
    pub platform: &'a str,
    pub format: &'a str,
    pub commander: bool,
    pub run_id: &'a str,
    pub cycle: usize,
    pub selection_score: f64,
    pub selection_lower: f64,
    pub selection_upper: f64,
    pub holdout_score: f64,
    pub challenger_selection_score: Option<f64>,
    pub challenger_holdout_score: Option<f64>,
    pub games: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct CalibrationStatus {
    pub observations: usize,
    pub empirical_interval_coverage: f64,
    pub target_interval_coverage: f64,
    pub mean_generalization_gap: f64,
    pub mean_absolute_error: f64,
    pub selection_agreement: Option<f64>,
    pub recommended_risk_aversion_delta: f64,
    pub state: String,
    pub database_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct CalibrationDecision {
    pub effective_risk_aversion: f64,
    pub adaptive_elimination: bool,
    pub reason: String,
}

pub fn decision(
    status: &CalibrationStatus,
    requested_risk_aversion: f64,
    requested_adaptive_elimination: bool,
) -> CalibrationDecision {
    let undercovered = status.observations >= 10 && status.state == "undercovered";
    CalibrationDecision {
        effective_risk_aversion: (requested_risk_aversion
            + if undercovered {
                status.recommended_risk_aversion_delta.max(0.0)
            } else {
                0.0
            })
        .clamp(0.0, 1.0),
        adaptive_elimination: requested_adaptive_elimination && !undercovered,
        reason: if undercovered {
            "Held-out coverage is below target; widen caution and use ordinary halving until calibration recovers."
        } else if status.observations < 10 {
            "Fewer than ten held-out observations exist; retain the requested policy and continue collecting evidence."
        } else {
            "Held-out coverage supports the requested confidence-racing policy."
        }
        .into(),
    }
}

pub fn record(
    data_dir: &Path,
    observation: CalibrationObservation<'_>,
) -> Result<(), CalibrationError> {
    let values = [
        observation.selection_score,
        observation.selection_lower,
        observation.selection_upper,
        observation.holdout_score,
    ];
    if values.iter().any(|value| !value.is_finite())
        || observation
            .challenger_selection_score
            .is_some_and(|value| !value.is_finite())
        || observation
            .challenger_holdout_score
            .is_some_and(|value| !value.is_finite())
    {
        return Err(CalibrationError::InvalidScore);
    }
    let connection = open(data_dir)?;
    let covered = observation.holdout_score >= observation.selection_lower
        && observation.holdout_score <= observation.selection_upper;
    let agreed = observation
        .challenger_selection_score
        .zip(observation.challenger_holdout_score)
        .map(|(selection, holdout)| {
            (observation.selection_score >= selection) == (observation.holdout_score >= holdout)
        });
    connection.execute(
        "INSERT OR REPLACE INTO calibration
         (platform, format, commander, run_id, cycle, selection_score, selection_lower,
          selection_upper, holdout_score, challenger_selection_score,
          challenger_holdout_score, interval_covered, selection_agreed, games)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            observation.platform,
            observation.format,
            observation.commander,
            observation.run_id,
            observation.cycle.min(i64::MAX as usize) as i64,
            observation.selection_score,
            observation.selection_lower,
            observation.selection_upper,
            observation.holdout_score,
            observation.challenger_selection_score,
            observation.challenger_holdout_score,
            covered,
            agreed,
            observation.games.min(i64::MAX as usize) as i64,
        ],
    )?;
    connection.execute(
        "DELETE FROM calibration WHERE rowid NOT IN (
           SELECT rowid FROM calibration ORDER BY created_at DESC, rowid DESC LIMIT ?1
         )",
        [MAX_OBSERVATIONS as i64],
    )?;
    Ok(())
}

pub fn status(
    data_dir: &Path,
    platform: &str,
    format: &str,
    commander: bool,
    target_interval_coverage: f64,
) -> Result<CalibrationStatus, CalibrationError> {
    let connection = open(data_dir)?;
    let aggregates = connection.query_row(
        "SELECT COUNT(*), COALESCE(AVG(interval_covered), 0.0),
                COALESCE(AVG(holdout_score-selection_score), 0.0),
                COALESCE(AVG(ABS(holdout_score-selection_score)), 0.0),
                AVG(selection_agreed)
         FROM calibration WHERE platform=?1 AND format=?2 AND commander=?3",
        params![platform, format, commander],
        |row| {
            Ok((
                row.get::<_, i64>(0)?.max(0) as usize,
                row.get::<_, f64>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, Option<f64>>(4)?,
            ))
        },
    )?;
    let target = target_interval_coverage.clamp(0.5, 0.999);
    let delta = ((target - aggregates.1) * 0.75).clamp(-0.2, 0.35);
    let state = if aggregates.0 < 10 {
        "collecting"
    } else if aggregates.1 + 0.02 < target {
        "undercovered"
    } else if aggregates.1 > (target + 0.08).min(1.0) {
        "overconservative"
    } else {
        "calibrated"
    };
    Ok(CalibrationStatus {
        observations: aggregates.0,
        empirical_interval_coverage: aggregates.1,
        target_interval_coverage: target,
        mean_generalization_gap: aggregates.2,
        mean_absolute_error: aggregates.3,
        selection_agreement: aggregates.4,
        recommended_risk_aversion_delta: delta,
        state: state.into(),
        database_bytes: fs::metadata(database_path(data_dir)).map_or(0, |metadata| metadata.len()),
    })
}

fn database_path(data_dir: &Path) -> PathBuf {
    data_dir.join("calibration.sqlite")
}

fn open(data_dir: &Path) -> Result<Connection, CalibrationError> {
    fs::create_dir_all(data_dir)?;
    let connection = Connection::open(database_path(data_dir))?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS calibration (
           platform TEXT NOT NULL,
           format TEXT NOT NULL,
           commander INTEGER NOT NULL,
           run_id TEXT NOT NULL,
           cycle INTEGER NOT NULL,
           selection_score REAL NOT NULL,
           selection_lower REAL NOT NULL,
           selection_upper REAL NOT NULL,
           holdout_score REAL NOT NULL,
           challenger_selection_score REAL,
           challenger_holdout_score REAL,
           interval_covered INTEGER NOT NULL,
           selection_agreed INTEGER,
           games INTEGER NOT NULL,
           created_at TEXT NOT NULL DEFAULT (datetime('now')),
           UNIQUE(run_id, cycle)
         );
         CREATE INDEX IF NOT EXISTS calibration_scope
           ON calibration(platform, format, commander, created_at DESC);",
    )?;
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn held_out_observations_report_interval_and_selection_calibration() {
        let directory =
            std::env::temp_dir().join(format!("metagene-calibration-{}", Uuid::new_v4()));
        for cycle in 1..=10 {
            record(
                &directory,
                CalibrationObservation {
                    platform: "arena",
                    format: "historic",
                    commander: false,
                    run_id: "run",
                    cycle,
                    selection_score: 0.6,
                    selection_lower: 0.5,
                    selection_upper: 0.7,
                    holdout_score: if cycle == 10 { 0.4 } else { 0.58 },
                    challenger_selection_score: Some(0.5),
                    challenger_holdout_score: Some(0.48),
                    games: 128,
                },
            )
            .unwrap();
        }
        let report = status(&directory, "arena", "historic", false, 0.9).unwrap();
        assert_eq!(report.observations, 10);
        assert_eq!(report.empirical_interval_coverage, 0.9);
        assert_eq!(report.selection_agreement, Some(0.9));
        assert_eq!(report.state, "calibrated");
        let decision = decision(&report, 0.5, true);
        assert!(decision.adaptive_elimination);
        assert_eq!(decision.effective_risk_aversion, 0.5);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn undercoverage_makes_future_racing_conservative() {
        let status = CalibrationStatus {
            observations: 10,
            empirical_interval_coverage: 0.0,
            target_interval_coverage: 0.9,
            mean_generalization_gap: -0.06,
            mean_absolute_error: 0.06,
            selection_agreement: Some(1.0),
            recommended_risk_aversion_delta: 0.35,
            state: "undercovered".into(),
            database_bytes: 0,
        };
        let decision = decision(&status, 0.5, true);
        assert!(!decision.adaptive_elimination);
        assert_eq!(decision.effective_risk_aversion, 0.85);
    }
}
