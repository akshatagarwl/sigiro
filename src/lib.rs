pub mod ingest;
pub mod inserter;
pub mod investigate;
pub mod mcp;
pub mod schema;
pub mod server;

use anyhow::{Context, Result};
use duckdb::Connection;
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

pub fn open_db() -> Result<Connection> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create data directory: {}", dir.display()))?;
    let db_path = dir.join("sigiro.duckdb");
    Connection::open(&db_path)
        .with_context(|| format!("failed to open DuckDB at {}", db_path.display()))
}

pub fn parse_window(s: &str) -> Result<i64> {
    let s = s.trim();
    if let Some(m) = s.strip_suffix('m') {
        Ok(m.parse::<i64>().context("invalid minutes")? * 60 * 1_000_000)
    } else if let Some(h) = s.strip_suffix('h') {
        Ok(h.parse::<i64>().context("invalid hours")? * 3600 * 1_000_000)
    } else {
        anyhow::bail!("window must end with 'm' or 'h' (e.g. 15m, 1h)")
    }
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}
