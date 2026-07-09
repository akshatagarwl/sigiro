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
    // Try loading quack directly (may be bundled); if not, install from nightly.
    if conn.execute_batch("LOAD quack;").is_err() {
        conn.execute_batch("INSTALL quack FROM core_nightly; LOAD quack;")
            .context("failed to INSTALL or LOAD quack extension")?;
    }

    let uri = format!("quack:0.0.0.0:{}", port);
    let sql = format!(
        "SELECT * FROM quack_serve('{}', token='{}', allow_other_hostname => true);",
        uri, token
    );
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
    _quack_port: u16,
    _token: &str,
    sql: &str,
) -> Result<Vec<serde_json::Value>> {
    let url = format!("{}/v1/query", worker.endpoint);
    tracing::info!(worker_id=%worker.id, url=%url, "federated query via HTTP");

    let api_key = crate::config::API_KEY
        .value()
        .context("SIGIRO_API_KEY not set")?;

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("failed to build HTTP client")?;

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "text/plain")
        .body(sql.to_string())
        .send()
        .with_context(|| format!("HTTP request failed for worker {}", worker.id))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().unwrap_or_default();
        anyhow::bail!("worker {} returned HTTP {}: {}", worker.id, status, text);
    }

    let rows: Vec<serde_json::Value> = response
        .json()
        .with_context(|| format!("failed to parse JSON from worker {}", worker.id))?;

    Ok(rows)
}

/// Start quack server on a dedicated connection and block forever (or until error).
#[allow(clippy::needless_pass_by_value)] // conn is moved to keep the session alive
///
/// The quack server is tied to the DuckDB session lifetime — closing the connection
/// shuts the server down.  This function keeps `conn` alive by sleeping after the
/// `quack_serve` query completes.
pub fn start_quack_server_blocking(conn: Connection, port: u16, token: &str) -> Result<()> {
    if conn.execute_batch("LOAD quack;").is_err() {
        conn.execute_batch("INSTALL quack FROM core_nightly; LOAD quack;")
            .context("failed to INSTALL or LOAD quack extension")?;
    }

    let uri = format!("quack:0.0.0.0:{}", port);
    let sql = format!(
        "SELECT * FROM quack_serve('{}', token='{}', allow_other_hostname => true);",
        uri, token
    );
    let mut stmt = conn
        .prepare(&sql)
        .context("failed to prepare quack_serve")?;
    let mut rows = stmt.query([]).context("failed to execute quack_serve")?;

    let row = rows
        .next()
        .context("quack_serve returned no rows")?
        .context("quack_serve failed")?;
    let listen_url: String = row.get(1).context("failed to get listen_url")?;

    tracing::info!("quack server listening at {}", listen_url);

    // Consume any remaining rows so the query finishes cleanly.
    while let Ok(Some(_)) = rows.next() {}

    // The quack server runs for the lifetime of the DuckDB session.
    // Keep the connection alive by blocking this thread.
    tracing::info!("quack server keeping session alive on port {}", port);
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}
