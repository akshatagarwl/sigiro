pub mod admin;
pub mod auth;
pub mod client;
pub mod config;
pub mod diagnose;
pub mod drain;

pub mod ingest;
pub mod inserter;
pub mod investigate;
pub mod otel;
pub mod otlp;
pub mod pool;
pub mod profiling;
pub mod responses;
pub mod schema;
pub mod server;
pub mod storage;
pub mod types;

use anyhow::{Context, Result};
use camino::Utf8PathBuf;

// ---------------------------------------------------------------------------
// Shared type re-exports
// ---------------------------------------------------------------------------
// These types are used across multiple modules and re-exported at the crate
// root so consumers can use `sigiro::IngestMsg` instead of digging through
// `sigiro::ingest` or `sigiro::inserter`.

pub use ingest::OtlpGrpcService;
pub use investigate::{InvestigationBlock, Window, query_raw_logs};
pub use types::{IngestMsg, SignalTable, StatusCode, TimestampMicros};
// Re-export MsgRouter from inserter (not in types — it's a runtime router, not a data type)
pub use inserter::MsgRouter;
pub use responses::{ServiceBreakdownEntry, SpanDetail, TraceDetailResponse};
pub use server::ApiDoc;

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
