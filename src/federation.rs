use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tracing::{debug, info};
use tracing_attributes::instrument;

use crate::supervisor::Supervisor;
use crate::types::SupervisorRegistry;

/// Information about a federation worker.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct WorkerInfo {
    pub id: String,
    pub endpoint: String,
    /// Path to this worker's DuckLake catalog (e.g. `/data/default/catalog.sqlite`).
    /// Empty if the worker has no locally-accessible catalog (remote-only).
    pub catalog_path: String,
    pub mode: String,
    pub platform: PlatformInfo,
    pub ebpf_status: String,
    pub capabilities: WorkerCapabilities,
    pub missing_permissions: Vec<String>,
    pub suggestions: Vec<String>,
}

/// Platform information for a worker.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PlatformInfo {
    pub os: String,
    pub kernel: String,
    pub arch: String,
    pub container: String,
}

/// Capabilities of a federation worker.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct WorkerCapabilities {
    pub bpf: bool,
    pub net_admin: bool,
}

/// Create the `federation_workers` table in SQLite.
pub fn create_workers_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS federation_workers (
            id TEXT PRIMARY KEY,
            endpoint TEXT NOT NULL,
            catalog_path TEXT NOT NULL DEFAULT '',
            mode TEXT NOT NULL DEFAULT 'otlp',
            platform_json TEXT NOT NULL DEFAULT '{}',
            ebpf_status TEXT NOT NULL DEFAULT 'unknown',
            capabilities_json TEXT NOT NULL DEFAULT '{}',
            registered_at TEXT DEFAULT (datetime('now')),
            last_heartbeat TEXT DEFAULT (datetime('now'))
        );",
    )
    .context("failed to create federation_workers table")?;
    Ok(())
}

/// Register (or update) a worker in the federation.
#[instrument]
pub fn register_worker(conn: &Connection, worker: &WorkerInfo) -> Result<()> {
    let platform_json =
        serde_json::to_string(&worker.platform).context("failed to serialize platform info")?;
    let capabilities_json =
        serde_json::to_string(&worker.capabilities).context("failed to serialize capabilities")?;

    conn.execute(
        "INSERT OR REPLACE INTO federation_workers
            (id, endpoint, catalog_path, mode, platform_json, ebpf_status, capabilities_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            worker.id,
            worker.endpoint,
            worker.catalog_path,
            worker.mode,
            platform_json,
            worker.ebpf_status,
            capabilities_json,
        ],
    )
    .context("failed to register worker")?;

    info!("registered federation worker: {}", worker.id);
    debug!("worker endpoint={}, mode={}", worker.endpoint, worker.mode);

    let data_dir = crate::data_dir()?;
    regenerate_spicepod(conn, &data_dir)?;
    Ok(())
}

/// Deregister a worker from the federation.
/// Returns `true` if a worker was actually deleted.
#[instrument]
pub fn deregister_worker(conn: &Connection, id: &str) -> Result<bool> {
    let affected = conn
        .execute("DELETE FROM federation_workers WHERE id = ?1", [id])
        .context("failed to deregister worker")?;

    if affected > 0 {
        info!("deregistered federation worker: {id}");
        let data_dir = crate::data_dir()?;
        regenerate_spicepod(conn, &data_dir)?;
    } else {
        debug!("no worker found to deregister: {id}");
    }

    Ok(affected > 0)
}

/// List all registered federation workers.
pub fn list_workers(conn: &Connection) -> Result<Vec<WorkerInfo>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, endpoint, catalog_path, mode, platform_json, ebpf_status, capabilities_json
             FROM federation_workers",
        )
        .context("failed to prepare workers query")?;

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .context("failed to query workers")?;

    let mut workers = Vec::new();
    for row in rows {
        let (id, endpoint, catalog_path, mode, platform_json, ebpf_status, capabilities_json) =
            row.context("failed to read worker row")?;
        let platform: PlatformInfo =
            serde_json::from_str(&platform_json).context("failed to parse platform JSON")?;
        let capabilities: WorkerCapabilities = serde_json::from_str(&capabilities_json)
            .context("failed to parse capabilities JSON")?;
        workers.push(WorkerInfo {
            id,
            endpoint,
            catalog_path,
            mode,
            platform,
            ebpf_status,
            capabilities,
            missing_permissions: Vec::new(),
            suggestions: Vec::new(),
        });
    }
    Ok(workers)
}

// ---------------------------------------------------------------------------
// Spicepod YAML serialization
// ---------------------------------------------------------------------------

/// Corresponds to the `acceleration` block in spicepod.yaml.
#[derive(Debug, Serialize)]
struct Acceleration {
    enabled: bool,
    engine: &'static str,
}

/// Corresponds to each entry under `datasets` in spicepod.yaml.
#[derive(Debug, Serialize)]
struct Dataset {
    name: String,
    from: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    acceleration: Option<Acceleration>,
}

/// Corresponds to the top-level spicepod.yaml structure.
#[derive(Debug, Serialize)]
struct Spicepod {
    version: &'static str,
    kind: &'static str,
    name: &'static str,
    datasets: Vec<Dataset>,
}

/// Regenerate `spicepod.yaml` from all registered workers.
/// Uses the "default" project directory for the local DuckLake dataset.
/// If `SIGIRO_S3_BUCKET` is set, also adds a cold-storage S3 dataset.
#[instrument]
pub fn regenerate_spicepod(conn: &Connection, data_dir: &Path) -> Result<()> {
    let workers = list_workers(conn)?;

    let catalog_path = data_dir.join("default").join("catalog.sqlite");

    let mut datasets = vec![Dataset {
        name: "sigiro_local_spans".to_string(),
        from: format!("ducklake:{}", catalog_path.display()),
        acceleration: Some(Acceleration {
            enabled: true,
            engine: "duckdb",
        }),
    }];

    // Cold storage dataset via S3, if configured
    if let Some(bucket) = crate::config::S3_BUCKET
        .value()
        .ok()
        .filter(|s| !s.is_empty())
    {
        let prefix = crate::config::S3_PREFIX
            .value()
            .expect("S3_PREFIX config must be valid");
        let prefix_trimmed = prefix.trim_end_matches('/');
        let prefix_with_slash = if prefix_trimmed.is_empty() {
            String::new()
        } else {
            format!("{}/", prefix_trimmed)
        };
        datasets.push(Dataset {
            name: "sigiro_spans_cold".to_string(),
            from: format!("ducklake:s3://{}/{}data", bucket, prefix_with_slash),
            acceleration: None,
        });
    }

    let worker_datasets: Vec<&WorkerInfo> = workers
        .iter()
        .filter(|w| !w.catalog_path.is_empty())
        .collect();

    for worker in &worker_datasets {
        datasets.push(Dataset {
            name: format!("worker_{}_spans", worker.id),
            from: format!("ducklake:{}", worker.catalog_path),
            acceleration: None,
        });
    }

    let spicepod = Spicepod {
        version: "v1",
        kind: "Spicepod",
        name: "sigiro-central",
        datasets,
    };

    let mut yaml = serde_yaml::to_string(&spicepod).context("failed to serialize spicepod")?;

    // serde_yaml prepends "---\n" — strip it to match expected format
    yaml = yaml.strip_prefix("---\n").unwrap_or(&yaml).to_string();

    let spicepod_path = data_dir.join("spicepod.yaml");
    std::fs::write(&spicepod_path, &yaml).with_context(|| {
        format!(
            "failed to write spicepod.yaml to {}",
            spicepod_path.display()
        )
    })?;

    info!(
        "regenerated spicepod.yaml with {} worker dataset(s)",
        worker_datasets.len()
    );
    Ok(())
}

/// Start the `spiced` subprocess for Spice.ai federation.
/// Fails loudly if `spiced` is not in PATH or spicepod.yaml is missing.
#[instrument]
pub fn start_spice(data_dir: &Path, registry: &SupervisorRegistry) -> Result<()> {
    which::which("spiced")
        .context("spiced binary not found in PATH — install spiced for Spice federation (https://spiceai.org)")?;

    let spicepod_path = data_dir.join("spicepod.yaml");
    anyhow::ensure!(
        spicepod_path.exists(),
        "spicepod.yaml not found at {} — central federation requires a spicepod config",
        spicepod_path.display()
    );

    info!("starting spiced with config: {}", spicepod_path.display());

    let mut envs = HashMap::new();
    envs.insert("SPICE_LOG".to_string(), "info".to_string());

    let mut supervisor = Supervisor::new(
        "spiced",
        vec!["--config".to_string(), spicepod_path.display().to_string()],
        envs,
    );
    supervisor.start();

    registry.register("spiced", supervisor, None)?;

    info!("spiced started via supervisor");

    Ok(())
}

#[allow(clippy::disallowed_methods)]
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        create_workers_table(&conn).unwrap();
        conn
    }

    fn test_worker(id: &str, endpoint: &str, catalog_path: &str) -> WorkerInfo {
        WorkerInfo {
            id: id.to_string(),
            endpoint: endpoint.to_string(),
            catalog_path: catalog_path.to_string(),
            mode: "otlp".to_string(),
            platform: PlatformInfo {
                os: "linux".to_string(),
                kernel: "5.15.0".to_string(),
                arch: "x86_64".to_string(),
                container: "docker".to_string(),
            },
            ebpf_status: "ready".to_string(),
            capabilities: WorkerCapabilities {
                bpf: true,
                net_admin: true,
            },
            missing_permissions: vec![],
            suggestions: vec![],
        }
    }

    fn insert_worker_directly(conn: &Connection, worker: &WorkerInfo) {
        let platform_json = serde_json::to_string(&worker.platform).unwrap();
        let capabilities_json = serde_json::to_string(&worker.capabilities).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO federation_workers
                (id, endpoint, catalog_path, mode, platform_json, ebpf_status, capabilities_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                worker.id,
                worker.endpoint,
                worker.catalog_path,
                worker.mode,
                platform_json,
                worker.ebpf_status,
                capabilities_json,
            ],
        )
        .unwrap();
    }

    #[test]
    fn test_register_and_list() {
        let conn = setup_db();

        let worker = test_worker(
            "w1",
            "http://localhost:9999",
            "/data/default/catalog.sqlite",
        );
        insert_worker_directly(&conn, &worker);

        let workers = list_workers(&conn).unwrap();
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].id, "w1");
        assert_eq!(workers[0].endpoint, "http://localhost:9999");
        assert_eq!(workers[0].catalog_path, "/data/default/catalog.sqlite");
        assert_eq!(workers[0].platform.os, "linux");
        assert_eq!(workers[0].platform.kernel, "5.15.0");
        assert!(workers[0].capabilities.bpf);
        assert!(workers[0].capabilities.net_admin);
    }

    #[test]
    fn test_register_upsert() {
        let conn = setup_db();

        let mut worker = test_worker(
            "w1",
            "http://localhost:9999",
            "/data/default/catalog.sqlite",
        );
        insert_worker_directly(&conn, &worker);

        // Update the same worker
        worker.endpoint = "http://updated:9999".to_string();
        worker.catalog_path = "/data2/default/catalog.sqlite".to_string();
        insert_worker_directly(&conn, &worker);

        let workers = list_workers(&conn).unwrap();
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].endpoint, "http://updated:9999");
        assert_eq!(workers[0].catalog_path, "/data2/default/catalog.sqlite");
    }

    #[test]
    fn test_deregister() {
        let conn = setup_db();

        let w1 = test_worker("w1", "http://host1:9999", "/data1/default/catalog.sqlite");
        let w2 = test_worker("w2", "http://host2:9999", "/data2/default/catalog.sqlite");
        insert_worker_directly(&conn, &w1);
        insert_worker_directly(&conn, &w2);

        let deleted = conn
            .execute("DELETE FROM federation_workers WHERE id = ?1", ["w1"])
            .unwrap();
        assert_eq!(deleted, 1);

        let deleted_again = conn
            .execute("DELETE FROM federation_workers WHERE id = ?1", ["w1"])
            .unwrap();
        assert_eq!(deleted_again, 0);

        let workers = list_workers(&conn).unwrap();
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].id, "w2");
    }

    #[test]
    fn test_list_workers_empty() {
        let conn = setup_db();
        let workers = list_workers(&conn).unwrap();
        assert!(workers.is_empty());
    }

    #[test]
    fn test_regenerate_spicepod_yaml() {
        let conn = setup_db();
        let dir = TempDir::new().unwrap();

        // Workers with catalog_path — should be included as DuckLake datasets
        let w1 = test_worker(
            "alpha",
            "http://10.0.0.1:9999",
            "/data/default/catalog.sqlite",
        );
        let w2 = test_worker(
            "beta",
            "http://10.0.0.2:9999",
            "/data2/default/catalog.sqlite",
        );
        // Worker without catalog_path — should be skipped (needs shared storage)
        let w3 = test_worker("gamma", "http://10.0.0.3:9999", "");
        insert_worker_directly(&conn, &w1);
        insert_worker_directly(&conn, &w2);
        insert_worker_directly(&conn, &w3);

        regenerate_spicepod(&conn, dir.path()).unwrap();

        let yaml = std::fs::read_to_string(dir.path().join("spicepod.yaml")).unwrap();
        assert!(yaml.contains("version: v1"));
        assert!(yaml.contains("kind: Spicepod"));
        assert!(yaml.contains("name: sigiro-central"));
        assert!(yaml.contains("sigiro_local_spans"));
        assert!(yaml.contains("ducklake:"));
        assert!(yaml.contains("default/catalog.sqlite"));
        assert!(yaml.contains("enabled: true"));
        assert!(yaml.contains("engine: duckdb"));
        assert!(yaml.contains("worker_alpha_spans"));
        assert!(yaml.contains("ducklake:/data/default/catalog.sqlite"));
        assert!(yaml.contains("worker_beta_spans"));
        assert!(yaml.contains("ducklake:/data2/default/catalog.sqlite"));
        // gamma should be excluded (no catalog_path)
        assert!(!yaml.contains("worker_gamma_spans"));
    }

    #[test]
    fn test_regenerate_spicepod_cold_storage() {
        let conn = setup_db();
        let dir = TempDir::new().unwrap();

        // Enable cold storage via env
        temp_env::with_vars(
            [
                ("SIGIRO_S3_BUCKET", Some("my-bucket")),
                ("SIGIRO_S3_PREFIX", Some("traces/")),
            ],
            || {
                let w1 = test_worker("w1", "http://10.0.0.1:9999", "/data/default/catalog.sqlite");
                insert_worker_directly(&conn, &w1);

                regenerate_spicepod(&conn, dir.path()).unwrap();

                let yaml = std::fs::read_to_string(dir.path().join("spicepod.yaml")).unwrap();
                assert!(yaml.contains("sigiro_spans_cold"));
                assert!(yaml.contains("ducklake:s3://my-bucket/traces/data"));
                assert!(yaml.contains("worker_w1_spans"));
                assert!(yaml.contains("ducklake:/data/default/catalog.sqlite"));
            },
        );
    }

    #[test]
    fn test_regenerate_spicepod_empty_workers() {
        let conn = setup_db();
        let dir = TempDir::new().unwrap();

        regenerate_spicepod(&conn, dir.path()).unwrap();

        let yaml = std::fs::read_to_string(dir.path().join("spicepod.yaml")).unwrap();
        assert!(yaml.contains("version: v1"));
        assert!(yaml.contains("name: sigiro-central"));
        // Only the local dataset, no worker datasets
        assert!(yaml.contains("sigiro_local_spans"));
        assert!(yaml.contains("ducklake:"));
        assert!(!yaml.contains("worker_"));
    }

    #[test]
    fn test_spicepod_yaml_format() {
        let conn = setup_db();
        let dir = TempDir::new().unwrap();

        let w1 = test_worker("x", "http://localhost:9999", "/var/data/catalog.sqlite");
        insert_worker_directly(&conn, &w1);

        regenerate_spicepod(&conn, dir.path()).unwrap();

        let yaml = std::fs::read_to_string(dir.path().join("spicepod.yaml")).unwrap();
        // No YAML document marker
        assert!(!yaml.starts_with("---"));
        // Valid YAML structure
        let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        let datasets = parsed
            .as_mapping()
            .and_then(|m| m.get(serde_yaml::Value::String("datasets".into())))
            .and_then(|v| v.as_sequence())
            .unwrap();
        assert!(!datasets.is_empty());
    }
}
