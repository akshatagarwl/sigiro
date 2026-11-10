pub mod admin;
pub mod alerts;
pub mod auth;
pub mod client;
pub mod config;
pub(crate) mod dashboard;
mod declared_schema;
pub mod disk_gate;
pub(crate) mod docs;
pub mod docs_mcp;
pub mod federate;

pub mod ingest;
pub mod ingest_keys;
pub mod inserter;
pub mod mcp;

pub mod duckdb_json;
pub mod oauth;
pub mod otel;
pub mod pool;
pub mod profiling;
pub(crate) mod quack;
pub mod responses;
pub mod runtime_metrics;
pub mod schema;
pub mod server;
pub(crate) mod static_site;
pub mod storage;
pub mod types;
pub mod workload;

use anyhow::{Context, Result};
use camino::Utf8PathBuf;

// ---------------------------------------------------------------------------
// Shared type re-exports
// ---------------------------------------------------------------------------
// These types are used across multiple modules and re-exported at the crate
// root so consumers can use `sigiro::IngestMsg` instead of digging through
// `sigiro::ingest` or `sigiro::inserter`.

pub use types::{IngestMsg, SignalTable, TenantId, TimestampMicros};
// Re-export MsgRouter from inserter (not in types — it's a runtime router, not a data type)
pub use inserter::MsgRouter;

pub fn data_dir() -> Result<Utf8PathBuf> {
    if let Ok(path) = crate::config::DATA_DIR.value() {
        return Ok(Utf8PathBuf::from(path));
    }
    // Fallback to platform-specific data directory
    let base = dirs::data_dir().context("cannot determine data directory")?;
    let base = Utf8PathBuf::from_path_buf(base)
        .map_err(|_| anyhow::anyhow!("data directory path is not valid UTF-8"))?;
    Ok(base.join("sigiro"))
}
