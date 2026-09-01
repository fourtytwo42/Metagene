use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};
use sysinfo::System;
use thiserror::Error;

const SETTINGS_FILE: &str = "resource-budgets.json";
const ORPHAN_AGE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ResourceBudgets {
    pub cpu_threads: usize,
    pub max_engine_processes: usize,
    pub memory_mib: u64,
    pub run_time_seconds: u64,
    pub disk_mib: u64,
}

impl Default for ResourceBudgets {
    fn default() -> Self {
        let logical = std::thread::available_parallelism().map_or(1, usize::from);
        Self {
            cpu_threads: logical.saturating_sub(2).max(1),
            max_engine_processes: logical.saturating_sub(2).clamp(1, 64),
            memory_mib: 8 * 1024,
            run_time_seconds: 60 * 60,
            disk_mib: 4 * 1024,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ResourceBudgetStatus {
    pub budgets: ResourceBudgets,
    pub app_data_bytes: u64,
    pub process_memory_bytes: u64,
    pub orphan_job_directories_removed: usize,
}

#[derive(Debug, Error)]
pub enum ResourceError {
    #[error("resource budget file error: {0}")]
    Io(#[from] std::io::Error),
    #[error("resource budget JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid resource budget: {0}")]
    Invalid(String),
    #[error("resource budget exceeded: {0}")]
    Exceeded(String),
}

pub fn load(data_dir: &Path) -> Result<ResourceBudgets, ResourceError> {
    let path = data_dir.join(SETTINGS_FILE);
    if !path.is_file() {
        return Ok(ResourceBudgets::default());
    }
    let budgets: ResourceBudgets = serde_json::from_slice(&fs::read(path)?)?;
    validate(&budgets)?;
    Ok(budgets)
}

pub fn save(
    data_dir: &Path,
    budgets: &ResourceBudgets,
) -> Result<ResourceBudgetStatus, ResourceError> {
    validate(budgets)?;
    fs::create_dir_all(data_dir)?;
    let path = data_dir.join(SETTINGS_FILE);
    let temporary = data_dir.join(format!("{SETTINGS_FILE}.writing"));
    fs::write(&temporary, serde_json::to_vec_pretty(budgets)?)?;
    fs::rename(temporary, path)?;
    status(data_dir)
}

pub fn status(data_dir: &Path) -> Result<ResourceBudgetStatus, ResourceError> {
    let removed = prune_orphan_jobs(data_dir)?;
    Ok(ResourceBudgetStatus {
        budgets: load(data_dir)?,
        app_data_bytes: directory_bytes(data_dir)?,
        process_memory_bytes: process_memory(),
        orphan_job_directories_removed: removed,
    })
}

pub fn enforce(data_dir: &Path, budgets: &ResourceBudgets) -> Result<(), ResourceError> {
    validate(budgets)?;
    let _ = prune_orphan_jobs(data_dir)?;
    let memory = process_memory();
    let memory_limit = budgets.memory_mib.saturating_mul(1024 * 1024);
    if memory > memory_limit {
        return Err(ResourceError::Exceeded(format!(
            "process memory is {} MiB; configured limit is {} MiB",
            memory / (1024 * 1024),
            budgets.memory_mib
        )));
    }
    let disk = directory_bytes(data_dir)?;
    let disk_limit = budgets.disk_mib.saturating_mul(1024 * 1024);
    if disk > disk_limit {
        return Err(ResourceError::Exceeded(format!(
            "application data uses {} MiB; configured limit is {} MiB",
            disk / (1024 * 1024),
            budgets.disk_mib
        )));
    }
    Ok(())
}

fn validate(budgets: &ResourceBudgets) -> Result<(), ResourceError> {
    if !(1..=256).contains(&budgets.cpu_threads) {
        return Err(ResourceError::Invalid("CPU threads must be 1-256".into()));
    }
    if !(1..=256).contains(&budgets.max_engine_processes) {
        return Err(ResourceError::Invalid(
            "engine processes must be 1-256".into(),
        ));
    }
    if !(256..=1_048_576).contains(&budgets.memory_mib) {
        return Err(ResourceError::Invalid(
            "memory must be 256-1,048,576 MiB".into(),
        ));
    }
    if !(1..=31_536_000).contains(&budgets.run_time_seconds) {
        return Err(ResourceError::Invalid(
            "run time must be 1-31,536,000 seconds".into(),
        ));
    }
    if !(256..=1_048_576).contains(&budgets.disk_mib) {
        return Err(ResourceError::Invalid(
            "disk must be 256-1,048,576 MiB".into(),
        ));
    }
    Ok(())
}

fn process_memory() -> u64 {
    let mut system = System::new_all();
    system.refresh_all();
    sysinfo::get_current_pid()
        .ok()
        .and_then(|pid| system.process(pid))
        .map_or(0, |process| process.memory())
}

fn directory_bytes(path: &Path) -> Result<u64, std::io::Error> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total = 0u64;
    let mut pending = vec![path.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    Ok(total)
}

fn prune_orphan_jobs(data_dir: &Path) -> Result<usize, std::io::Error> {
    let root = data_dir.join("simulation-jobs");
    if !root.is_dir() {
        return Ok(0);
    }
    let now = SystemTime::now();
    let mut removed = 0usize;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if !metadata.is_dir() {
            continue;
        }
        let stale = metadata
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= ORPHAN_AGE);
        if stale {
            fs::remove_dir_all(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn budgets_round_trip_and_orphan_cleanup_is_age_bounded() {
        let root = std::env::temp_dir().join(format!("metagene-resources-{}", Uuid::new_v4()));
        let budgets = ResourceBudgets {
            cpu_threads: 3,
            max_engine_processes: 2,
            memory_mib: 1024,
            run_time_seconds: 60,
            disk_mib: 512,
        };
        let report = save(&root, &budgets).unwrap();
        assert_eq!(report.budgets.cpu_threads, 3);
        let active = root.join("simulation-jobs").join("active");
        fs::create_dir_all(&active).unwrap();
        fs::write(active.join("deck.txt"), b"deck").unwrap();
        let report = status(&root).unwrap();
        assert_eq!(report.orphan_job_directories_removed, 0);
        assert!(report.app_data_bytes > 0);
        fs::remove_dir_all(root).unwrap();
    }
}
