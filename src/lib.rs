pub mod admin;
pub mod auth;
pub mod billing;
pub mod client;
pub mod compactor;
pub mod config;
pub mod drain;
pub mod ebpf;
pub mod federation;
pub mod ingest;
pub mod inserter;
pub mod investigate;
pub mod mcp;
pub mod pool;
pub mod schema;
pub mod server;
pub mod storage;
pub mod toon;

use anyhow::{Context, Result};
use std::path::PathBuf;

pub fn data_dir() -> Result<PathBuf> {
    std::env::var("SIGIRO_DATA_DIR").map(PathBuf::from).context(
        "SIGIRO_DATA_DIR not set — export SIGIRO_DATA_DIR=/path/to/data before starting sigiro",
    )
}
