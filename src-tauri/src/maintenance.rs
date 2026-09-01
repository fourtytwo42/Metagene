use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
use tar::{Archive, Builder};
use thiserror::Error;
use uuid::Uuid;

const BACKUP_SCHEMA: u32 = 1;
const MAX_BACKUPS: usize = 10;

#[derive(Debug, Error)]
pub enum MaintenanceError {
    #[error("maintenance I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database check error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("backup manifest error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid backup: {0}")]
    Invalid(String),
}

#[derive(Clone, Debug, Serialize)]
pub struct DatabaseHealth {
    pub name: String,
    pub bytes: u64,
    pub healthy: bool,
    pub detail: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct StorageHealth {
    pub schema_version: u32,
    pub databases: Vec<DatabaseHealth>,
    pub backups: Vec<BackupSummary>,
}
#[derive(Clone, Debug, Serialize)]
pub struct BackupSummary {
    pub id: String,
    pub bytes: u64,
    pub created_at: String,
}
#[derive(Serialize, Deserialize)]
struct Manifest {
    schema_version: u32,
    created_at: String,
    files: Vec<String>,
}

pub fn health(data_dir: &Path) -> Result<StorageHealth, MaintenanceError> {
    fs::create_dir_all(data_dir)?;
    let mut databases = Vec::new();
    for entry in fs::read_dir(data_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("sqlite") {
            continue;
        }
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let result = Connection::open(&path).and_then(|db| {
            db.query_row("PRAGMA quick_check(1)", [], |row| row.get::<_, String>(0))
        });
        databases.push(DatabaseHealth {
            name,
            bytes: fs::metadata(&path)?.len(),
            healthy: result.as_deref() == Ok("ok"),
            detail: result.unwrap_or_else(|error| error.to_string()),
        });
    }
    databases.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(StorageHealth {
        schema_version: BACKUP_SCHEMA,
        databases,
        backups: list_backups(data_dir)?,
    })
}

pub fn diagnostics(data_dir: &Path) -> Result<String, MaintenanceError> {
    #[derive(Serialize)]
    struct Diagnostics {
        schema_version: u32,
        app_version: &'static str,
        storage: StorageHealth,
        privacy: &'static str,
    }
    serde_json::to_string_pretty(&Diagnostics { schema_version: 1, app_version: env!("CARGO_PKG_VERSION"), storage: health(data_dir)?, privacy: "Contains database filenames, sizes, and integrity results only; no deck lists, match records, paths, account data, or raw logs." }).map_err(Into::into)
}

pub fn create_backup(data_dir: &Path) -> Result<BackupSummary, MaintenanceError> {
    fs::create_dir_all(data_dir)?;
    let staging = data_dir.join(format!(".backup-stage-{}", Uuid::new_v4()));
    fs::create_dir_all(&staging)?;
    let result = (|| {
        let mut files = Vec::new();
        for entry in fs::read_dir(data_dir)? {
            let path = entry?.path();
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if path.extension().and_then(|value| value.to_str()) == Some("sqlite") {
                let destination = staging.join(name);
                let escaped = destination.to_string_lossy().replace('\'', "''");
                Connection::open(&path)?.execute_batch(&format!("VACUUM INTO '{escaped}'"))?;
                files.push(name.to_string());
            } else if path.extension().and_then(|value| value.to_str()) == Some("json") {
                fs::copy(&path, staging.join(name))?;
                files.push(name.to_string());
            }
        }
        let id = format!("backup-{}", Uuid::new_v4());
        let manifest = Manifest {
            schema_version: BACKUP_SCHEMA,
            created_at: id.clone(),
            files: files.clone(),
        };
        fs::write(
            staging.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        let directory = data_dir.join("backups");
        fs::create_dir_all(&directory)?;
        let path = directory.join(format!("{id}.tar.gz"));
        let encoder = GzEncoder::new(fs::File::create(&path)?, Compression::default());
        let mut archive = Builder::new(encoder);
        archive.append_path_with_name(staging.join("manifest.json"), "manifest.json")?;
        for name in &files {
            archive.append_path_with_name(staging.join(name), name)?;
        }
        archive.into_inner()?.finish()?;
        prune_backups(&directory)?;
        Ok(BackupSummary {
            id,
            bytes: fs::metadata(path)?.len(),
            created_at: manifest.created_at,
        })
    })();
    let _ = fs::remove_dir_all(&staging);
    result
}

pub fn restore_backup(data_dir: &Path, id: &str) -> Result<StorageHealth, MaintenanceError> {
    if !id.starts_with("backup-") || id.contains(['/', '\\']) {
        return Err(MaintenanceError::Invalid(
            "invalid backup identifier".into(),
        ));
    }
    let archive_path = data_dir.join("backups").join(format!("{id}.tar.gz"));
    if !archive_path.is_file() {
        return Err(MaintenanceError::Invalid("backup not found".into()));
    }
    let staging = data_dir.join(format!(".restore-stage-{}", Uuid::new_v4()));
    fs::create_dir_all(&staging)?;
    let result = (|| {
        Archive::new(GzDecoder::new(fs::File::open(&archive_path)?)).unpack(&staging)?;
        let manifest: Manifest = serde_json::from_slice(&fs::read(staging.join("manifest.json"))?)?;
        validate_manifest(&manifest)?;
        for name in &manifest.files {
            if Path::new(name).file_name().and_then(|v| v.to_str()) != Some(name) {
                return Err(MaintenanceError::Invalid("unsafe backup path".into()));
            }
            let source = staging.join(name);
            if source.extension().and_then(|v| v.to_str()) == Some("sqlite") {
                let check: String =
                    Connection::open(&source)?
                        .query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
                if check != "ok" {
                    return Err(MaintenanceError::Invalid(format!(
                        "corrupt database in backup: {name}"
                    )));
                }
            }
        }
        let _safety = create_backup(data_dir)?;
        for name in &manifest.files {
            fs::copy(staging.join(name), data_dir.join(name))?;
        }
        health(data_dir)
    })();
    let _ = fs::remove_dir_all(staging);
    result
}

fn validate_manifest(manifest: &Manifest) -> Result<(), MaintenanceError> {
    if manifest.schema_version != BACKUP_SCHEMA {
        return Err(MaintenanceError::Invalid(format!(
            "unsupported backup schema {}; this app supports schema {BACKUP_SCHEMA}",
            manifest.schema_version
        )));
    }
    if manifest.files.iter().any(|name| {
        Path::new(name).file_name().and_then(|value| value.to_str()) != Some(name)
            || !(name.ends_with(".sqlite") || name.ends_with(".json"))
    }) {
        return Err(MaintenanceError::Invalid(
            "backup contains an unsafe or unsupported file".into(),
        ));
    }
    Ok(())
}

pub fn quarantine_corrupt(data_dir: &Path, name: &str) -> Result<StorageHealth, MaintenanceError> {
    if Path::new(name).file_name().and_then(|v| v.to_str()) != Some(name)
        || !name.ends_with(".sqlite")
    {
        return Err(MaintenanceError::Invalid("invalid database name".into()));
    }
    let path = data_dir.join(name);
    let check = Connection::open(&path)
        .and_then(|db| db.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0)));
    if check.as_deref() == Ok("ok") {
        return Err(MaintenanceError::Invalid(
            "database is healthy; repair refused".into(),
        ));
    }
    fs::rename(
        &path,
        data_dir.join(format!("{name}.corrupt-{}", Uuid::new_v4())),
    )?;
    health(data_dir)
}

fn list_backups(data_dir: &Path) -> Result<Vec<BackupSummary>, std::io::Error> {
    let directory = data_dir.join("backups");
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let mut rows = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let name = path
                .file_name()?
                .to_str()?
                .strip_suffix(".tar.gz")?
                .to_string();
            Some(BackupSummary {
                id: name.clone(),
                bytes: entry.metadata().ok()?.len(),
                created_at: name,
            })
        })
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(rows)
}
fn prune_backups(directory: &Path) -> Result<(), std::io::Error> {
    let rows = list_backups(directory.parent().unwrap_or(directory))?;
    for row in rows.into_iter().skip(MAX_BACKUPS) {
        fs::remove_file(directory.join(format!("{}.tar.gz", row.id)))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_restore_and_corruption_recovery_are_bounded_and_versioned() {
        let root = std::env::temp_dir().join(format!("metagene-maintenance-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let database = root.join("runs.sqlite");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute("CREATE TABLE proof(value TEXT)", [])
            .unwrap();
        connection
            .execute("INSERT INTO proof VALUES ('before')", [])
            .unwrap();
        drop(connection);
        fs::write(root.join("resource-budgets.json"), b"{\"cpu_threads\":4}").unwrap();
        let backup = create_backup(&root).unwrap();
        let connection = Connection::open(&database).unwrap();
        connection.execute("DELETE FROM proof", []).unwrap();
        drop(connection);
        restore_backup(&root, &backup.id).unwrap();
        let value: String = Connection::open(&database)
            .unwrap()
            .query_row("SELECT value FROM proof", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "before");
        fs::write(root.join("broken.sqlite"), b"not sqlite").unwrap();
        let repaired = quarantine_corrupt(&root, "broken.sqlite").unwrap();
        assert!(
            !repaired
                .databases
                .iter()
                .any(|row| row.name == "broken.sqlite")
        );
        assert!(
            fs::read_dir(&root)
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("broken.sqlite.corrupt-"))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_future_and_unsafe_backup_manifests() {
        assert!(
            validate_manifest(&Manifest {
                schema_version: 2,
                created_at: "future".into(),
                files: vec![]
            })
            .is_err()
        );
        assert!(
            validate_manifest(&Manifest {
                schema_version: 1,
                created_at: "unsafe".into(),
                files: vec!["../runs.sqlite".into()]
            })
            .is_err()
        );
        assert!(
            validate_manifest(&Manifest {
                schema_version: 1,
                created_at: "current".into(),
                files: vec!["runs.sqlite".into(), "resource-budgets.json".into()]
            })
            .is_ok()
        );
    }
}
