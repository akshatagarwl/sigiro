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
    match std::env::var("SIGIRO_DATA_DIR") {
        Ok(dir) => Ok(PathBuf::from(dir)),
        Err(_) => {
            let home = dirs::home_dir().context("cannot determine home directory")?;
            Ok(home.join(".sigiro").join("data"))
        }
    }
}
