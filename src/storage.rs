use anyhow::{Context, Result};
use duckdb::Connection;
use std::path::Path;

/// Initialize DuckLake with SQLite catalog + local Parquet (zero config).
/// Tables accessible as `lake.sigiro_*`.
/// Catalog stored in `project_dir/catalog.sqlite`.
/// Parquet stored in `project_dir/catalog.ducklake.files/` (auto-created by DuckLake).
pub fn init_default(conn: &Connection, project_dir: &Path) -> Result<()> {
    let project_dir_str = project_dir.to_string_lossy();
    std::fs::create_dir_all(project_dir).with_context(|| {
        format!(
            "failed to create project directory: {}",
            project_dir.display()
        )
    })?;

    // Load required extensions (ducklake for storage, sqlite for catalog)
    conn.execute_batch("INSTALL ducklake; INSTALL sqlite; LOAD ducklake; LOAD sqlite;")
        .context("failed to install/load extensions")?;
    tracing::info!("loaded DuckDB extensions: ducklake, sqlite");

    // Attach DuckLake with SQLite catalog + local Parquet.
    // catalog.sqlite is a SQLite database with DuckLake schema.
    // Data files go to catalog.ducklake.files/ (auto-created).
    let attach_sql = format!("ATTACH 'ducklake:{project_dir_str}/catalog.sqlite' AS lake");
    conn.execute_batch(&attach_sql)
        .context("failed to attach DuckLake catalog")?;
    tracing::info!("DuckLake attached — catalog: SQLite file, data: local Parquet");

    Ok(())
}

/// Try to load distributed storage config from SIGIRO_* env vars.
/// Always returns None for now (distributed mode not yet implemented).
pub fn load_config() -> Option<StorageConfig> {
    // Distributed mode with PostgreSQL catalog + S3 data is planned but not yet implemented.
    // For now, always use local SQLite catalog + local Parquet.
    None
}

/// Distributed storage configuration (not yet implemented).
/// When implemented: PostgreSQL catalog + S3 data for cloud mode.
#[derive(Debug)]
pub struct StorageConfig {
    pub catalog_url: String,
    pub s3_bucket: String,
    pub s3_region: String,
    pub s3_prefix: String,
}

/// Always returns "lake." — all tables are accessed through DuckLake.
pub fn table_prefix() -> &'static str {
    "lake."
}

/// The 6 sigiro table names (without prefix).
pub const TABLE_NAMES: &[&str] = &[
    "sigiro_spans",
    "sigiro_logs",
    "sigiro_metrics_gauge",
    "sigiro_metrics_sum",
    "sigiro_metrics_histogram",
    "sigiro_metrics_exp_histogram",
];
