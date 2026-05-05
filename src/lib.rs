pub mod admin;
pub mod auth;
pub mod client;
pub mod config;
pub mod drain;

#[allow(clippy::items_after_test_module)]
pub mod ebpf;
pub mod federation;
pub mod ingest;
pub mod inserter;
pub mod investigate;
pub mod mcp;
pub mod otel;
pub mod pool;
pub mod profiling;
pub mod responses;
pub mod schema;
pub mod server;
pub mod storage;
pub mod supervisor;
pub mod toon;
pub mod types;

use anyhow::{Context, Result};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Shared type re-exports
// ---------------------------------------------------------------------------
// These types are used across multiple modules and re-exported at the crate
// root so consumers can use `sigiro::IngestMsg` instead of digging through
// `sigiro::ingest` or `sigiro::inserter`.

pub use ingest::OtlpGrpcService;
pub use investigate::{InvestigationBlock, Window};
pub use types::{IngestMsg, SignalTable, StatusCode, SupervisorHandle, TimestampMicros};
// Re-export MsgRouter from inserter (not in types — it's a runtime router, not a data type)
pub use inserter::MsgRouter;
pub use server::ApiDoc;

pub fn data_dir() -> Result<PathBuf> {
    crate::config::DATA_DIR.value().map(PathBuf::from).map_err(|e| anyhow::anyhow!("{e}")).context(
        "SIGIRO_DATA_DIR not set — export SIGIRO_DATA_DIR=/path/to/data before starting sigiro",
    )
}
