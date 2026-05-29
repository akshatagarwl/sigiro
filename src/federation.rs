use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};
use tracing_attributes::instrument;

/// Information about a federation worker.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
    /// Quack port advertised by this worker for federated queries.
    /// Defaults to 5432 if not provided.
    pub quack_port: u16,
}

/// Platform information for a worker.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PlatformInfo {
    pub os: String,
    pub kernel: String,
    pub arch: String,
    pub container: String,
}

/// Capabilities of a federation worker.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
            quack_port INTEGER NOT NULL DEFAULT 5432,
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
            (id, endpoint, catalog_path, mode, platform_json, ebpf_status, capabilities_json, quack_port)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            worker.id,
            worker.endpoint,
            worker.catalog_path,
            worker.mode,
            platform_json,
            worker.ebpf_status,
            capabilities_json,
            worker.quack_port,
        ],
    )
    .context("failed to register worker")?;

    info!("registered federation worker: {}", worker.id);
    debug!("worker endpoint={}, mode={}", worker.endpoint, worker.mode);

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
    } else {
        debug!("no worker found to deregister: {id}");
    }

    Ok(affected > 0)
}

/// List all registered federation workers.
pub fn list_workers(conn: &Connection) -> Result<Vec<WorkerInfo>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, endpoint, catalog_path, mode, platform_json, ebpf_status, capabilities_json, quack_port
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
                row.get::<_, u16>(7)?,
            ))
        })
        .context("failed to query workers")?;

    let mut workers = Vec::new();
    for row in rows {
        let (
            id,
            endpoint,
            catalog_path,
            mode,
            platform_json,
            ebpf_status,
            capabilities_json,
            quack_port,
        ) = row.context("failed to read worker row")?;
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
            quack_port,
        });
    }
    Ok(workers)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            quack_port: 5432,
        }
    }

    fn insert_worker_directly(conn: &Connection, worker: &WorkerInfo) {
        let platform_json = serde_json::to_string(&worker.platform).unwrap();
        let capabilities_json = serde_json::to_string(&worker.capabilities).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO federation_workers
                (id, endpoint, catalog_path, mode, platform_json, ebpf_status, capabilities_json, quack_port)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                worker.id,
                worker.endpoint,
                worker.catalog_path,
                worker.mode,
                platform_json,
                worker.ebpf_status,
                capabilities_json,
                worker.quack_port,
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
}
