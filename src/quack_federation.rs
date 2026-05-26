//! Quack-based DuckDB federation.

use anyhow::{Context, Result};
use duckdb::Connection;

/// The central quack server handle.
pub struct QuackServer {
    pub token: String,
    pub port: u16,
}

/// Start quack server on `conn` at `port` with `token` auth.
pub fn start_quack_server(conn: &Connection, port: u16, token: &str) -> Result<QuackServer> {
    conn.execute_batch("LOAD quack;")
        .context("failed to LOAD quack extension")?;

    let uri = format!("quack:0.0.0.0:{}", port);
    let sql = format!("SELECT * FROM quack_serve('{}', token='{}');", uri, token);
    let mut stmt = conn
        .prepare(&sql)
        .context("failed to prepare quack_serve")?;
    let mut rows = stmt.query([]).context("failed to execute quack_serve")?;

    let row = rows
        .next()
        .context("quack_serve returned no rows")?
        .context("quack_serve failed")?;
    let _listen_url: String = row.get(1).context("failed to get listen_url")?;

    tracing::info!("quack server started at port {}", port);

    Ok(QuackServer {
        token: token.to_string(),
        port,
    })
}

/// Extract host from an endpoint URL.
pub fn extract_host(endpoint: &str) -> Result<&str> {
    let endpoint = endpoint
        .strip_prefix("http://")
        .or_else(|| endpoint.strip_prefix("https://"))
        .unwrap_or(endpoint);
    Ok(endpoint.split(':').next().unwrap_or(endpoint))
}

use tokio::task;

/// Execute `sql` as a federated query across all `workers`.
pub async fn federated_query(
    conn: &Connection,
    workers: &[super::federation::WorkerInfo],
    quack_port: u16,
    quack_token: &str,
    sql: &str,
) -> Result<Vec<serde_json::Value>> {
    if workers.is_empty() {
        anyhow::bail!("federation requires at least one registered worker");
    }

    let results: Vec<Vec<serde_json::Value>> = task::spawn_blocking({
        let workers = workers.to_vec();
        let token = quack_token.to_string();
        let sql = sql.to_string();
        move || -> Result<Vec<Vec<serde_json::Value>>> {
            workers
                .iter()
                .map(|worker| query_single_worker(worker, quack_port, &token, &sql))
                .collect()
        }
    })
    .await??;

    let mut all_rows = Vec::new();
    for worker_results in results {
        all_rows.extend(worker_results);
    }
    Ok(all_rows)
}

fn query_single_worker(
    worker: &super::federation::WorkerInfo,
    quack_port: u16,
    token: &str,
    sql: &str,
) -> Result<Vec<serde_json::Value>> {
    let host = extract_host(&worker.endpoint).with_context(|| {
        format!(
            "failed to extract host from worker endpoint: {}",
            worker.endpoint
        )
    })?;
    let quack_uri = format!("quack:{}:{}", host, quack_port);

    let conn = Connection::open_in_memory()?;
    conn.execute_batch(&format!("CREATE SECRET (TYPE quack, TOKEN '{}');", token))?;
    conn.execute_batch(&format!("ATTACH '{}' AS remote;", quack_uri))?;

    let remote_sql = sql.replace("lake.", "remote.lake.");

    let mut stmt = conn
        .prepare(&remote_sql)
        .with_context(|| format!("failed to prepare federated query for worker {}", worker.id))?;
    let mut rows = stmt.query([])?;

    let mut results = Vec::new();
    while let Some(row) = rows.next().context("row iteration failed")? {
        let row_json = row_to_json(&stmt, row)?;
        results.push(row_json);
    }
    Ok(results)
}

fn row_to_json(stmt: &duckdb::Statement, row: &duckdb::Row) -> Result<serde_json::Value> {
    let column_count = stmt.as_ref().column_count();
    let mut obj = serde_json::Map::new();
    for i in 0..column_count {
        let name = stmt.as_ref().column_name(i).unwrap_or("?").to_string();
        let val = row.get_ref(i).context("column read failed")?;
        obj.insert(name, duckdb_value_to_json(val));
    }
    Ok(serde_json::Value::Object(obj))
}

fn duckdb_value_to_json(val: duckdb::types::ValueRef<'_>) -> serde_json::Value {
    use duckdb::types::ValueRef;
    match val {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Boolean(b) => serde_json::Value::Bool(b),
        ValueRef::TinyInt(b) => serde_json::json!(b),
        ValueRef::SmallInt(b) => serde_json::json!(b),
        ValueRef::Integer(b) => serde_json::json!(b),
        ValueRef::BigInt(b) => serde_json::json!(b),
        ValueRef::Float(b) => serde_json::json!(b),
        ValueRef::Double(b) => serde_json::json!(b),
        ValueRef::Varchar(b) | ValueRef::Text(b) => {
            serde_json::Value::String(String::from_utf8_lossy(b).to_string())
        }
        ValueRef::Blob(b) => serde_json::Value::String(format!("[blob {} bytes]", b.len())),
        _ => serde_json::Value::String(format!("{:?}", val)),
    }
}
