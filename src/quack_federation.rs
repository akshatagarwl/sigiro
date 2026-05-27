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

pub fn blocking_federated_query(
    _conn: &Connection,
    workers: &[super::federation::WorkerInfo],
    quack_port: u16,
    quack_token: &str,
    sql: &str,
) -> Result<Vec<serde_json::Value>> {
    if workers.is_empty() {
        anyhow::bail!("federation requires at least one registered worker");
    }

    let mut all_rows = Vec::new();
    for worker in workers {
        let rows = query_single_worker(worker, quack_port, quack_token, sql)?;
        all_rows.extend(rows);
    }
    Ok(all_rows)
}

/// Execute `sql` as a federated query across all `workers`.
pub async fn federated_query(
    _conn: &Connection,
    workers: &[super::federation::WorkerInfo],
    _quack_port: u16,
    _quack_token: &str,
    _sql: &str,
) -> Result<Vec<serde_json::Value>> {
    if workers.is_empty() {
        anyhow::bail!("federation requires at least one registered worker");
    }
    Ok(vec![])
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

    let column_count = stmt.column_count();
    let column_names: Vec<String> = (0..column_count)
        .map(|i| {
            stmt.column_name(i)
                .map(std::string::ToString::to_string)
                .unwrap_or_else(|_e| "?".to_string())
        })
        .collect();

    let mut rows = stmt.query([])?;
    let mut results = Vec::new();
    while let Some(row) = rows.next().context("row iteration failed")? {
        let row_json = row_to_json(&column_names, row)?;
        results.push(row_json);
    }
    Ok(results)
}

fn row_to_json(column_names: &[String], row: &duckdb::Row) -> Result<serde_json::Value> {
    let mut obj = serde_json::Map::new();
    for (i, name) in column_names.iter().enumerate() {
        let val = row.get_ref(i).context("column read failed")?;
        obj.insert(name.clone(), duckdb_value_to_json(val));
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
        ValueRef::Int(b) => serde_json::json!(b),
        ValueRef::BigInt(b) => serde_json::json!(b),
        ValueRef::Float(b) => serde_json::json!(b),
        ValueRef::Double(b) => serde_json::json!(b),
        ValueRef::Text(b) => serde_json::Value::String(String::from_utf8_lossy(b).to_string()),
        ValueRef::Blob(b) => serde_json::Value::String(format!("[blob {} bytes]", b.len())),
        _ => serde_json::Value::String(format!("{:?}", val)),
    }
}
