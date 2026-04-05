use anyhow::{Context, Result};
use duckdb::Connection;
use serde::Deserialize;
use std::path::Path;

/// DuckLake storage configuration, parsed from SIGIRO_* env vars via envy.
/// When SIGIRO_CATALOG_URL is set: PostgreSQL catalog + S3 (distributed mode).
/// When not set: DuckDB file catalog + local Parquet (default, zero config).
#[derive(Deserialize, Debug)]
pub struct StorageConfig {
    /// PostgreSQL connection string for DuckLake catalog (required for distributed mode)
    pub catalog_url: String,
    /// S3 bucket for Parquet data files
    pub s3_bucket: String,
    /// S3 region (default: us-east-1)
    #[serde(default = "default_region")]
    pub s3_region: String,
    /// S3 key prefix within the bucket (default: "sigiro/")
    #[serde(default = "default_prefix")]
    pub s3_prefix: String,
    /// S3 endpoint override (for MinIO, R2, etc.)
    pub s3_endpoint: Option<String>,
    /// Local directory for DuckDB extension files (for Docker/offline)
    pub extension_dir: Option<String>,
}

fn default_region() -> String {
    "us-east-1".to_string()
}

fn default_prefix() -> String {
    "sigiro/".to_string()
}

/// Escape single quotes for DuckDB string literals.
/// Config values come from env vars, not user input, but a postgres URL with
/// special chars (e.g., passwords containing ') would break the SQL otherwise.
fn escape_sql_literal(s: &str) -> String {
    s.replace('\'', "''")
}

/// Initialize DuckLake with DuckDB file catalog + local Parquet (zero config).
/// Tables accessible as `lake.sigiro_*`.
/// Catalog stored in `project_dir/catalog.ducklake`.
/// Parquet stored in `project_dir/catalog.ducklake.files/` (auto-created by DuckLake).
pub fn init_default(conn: &Connection, project_dir: &Path) -> Result<()> {
    let project_dir_str = escape_sql_literal(&project_dir.to_string_lossy());
    std::fs::create_dir_all(project_dir).with_context(|| {
        format!(
            "failed to create project directory: {}",
            project_dir.display()
        )
    })?;

    // Load required extension (only ducklake — no sqlite extension needed)
    conn.execute_batch("INSTALL ducklake; LOAD ducklake;")
        .context("failed to install/load ducklake extension")?;
    tracing::info!("loaded DuckDB extension: ducklake");

    // Attach DuckLake with DuckDB file catalog + local Parquet.
    // catalog.ducklake is a DuckDB database with the DuckLake schema.
    // Data files go to catalog.ducklake.files/ (auto-created).
    let attach_sql = format!("ATTACH 'ducklake:{project_dir_str}/catalog.ducklake' AS lake");
    conn.execute_batch(&attach_sql)
        .context("failed to attach DuckLake catalog")?;
    tracing::info!("DuckLake attached — catalog: DuckDB file, data: local Parquet");

    Ok(())
}

/// Initialize DuckLake with PostgreSQL catalog + S3 (distributed mode).
/// Tables accessible as `lake.sigiro_*`.
pub fn init_ducklake(conn: &Connection, config: &StorageConfig) -> Result<()> {
    // Set extension directory if provided (Docker/offline environments)
    if let Some(ref dir) = config.extension_dir {
        let escaped = escape_sql_literal(dir);
        conn.execute_batch(&format!("SET extension_directory = '{escaped}'"))
            .with_context(|| format!("failed to set extension_directory to {dir}"))?;
    }

    // Load required extensions
    for ext in &["ducklake", "postgres", "httpfs", "cache_httpfs"] {
        conn.execute_batch(&format!("INSTALL {ext}; LOAD {ext};"))
            .with_context(|| format!("failed to install/load {ext} extension"))?;
        tracing::info!("loaded DuckDB extension: {ext}");
    }

    // Configure S3 credentials via credential chain (IAM roles, env vars, config files)
    let region = escape_sql_literal(&config.s3_region);
    let mut secret_sql = format!(
        "CREATE SECRET IF NOT EXISTS sigiro_s3 (TYPE s3, PROVIDER credential_chain, REGION '{region}'"
    );
    if let Some(ref endpoint) = config.s3_endpoint {
        let escaped = escape_sql_literal(endpoint);
        secret_sql.push_str(&format!(
            ", ENDPOINT '{escaped}', USE_SSL false, URL_STYLE path"
        ));
    }
    secret_sql.push(')');
    conn.execute_batch(&secret_sql)
        .context("failed to create S3 secret")?;
    tracing::info!("S3 credentials configured (region: {})", config.s3_region);

    // Attach DuckLake catalog
    let data_path = format!(
        "s3://{}/{}",
        escape_sql_literal(&config.s3_bucket),
        escape_sql_literal(config.s3_prefix.trim_end_matches('/'))
    );
    let catalog_url = escape_sql_literal(&config.catalog_url);
    let attach_sql =
        format!("ATTACH 'ducklake:postgres:{catalog_url}' AS lake (DATA_PATH '{data_path}')");
    conn.execute_batch(&attach_sql)
        .context("failed to attach DuckLake catalog")?;
    tracing::info!("DuckLake attached — catalog: postgres, data: {data_path}");

    Ok(())
}

/// Try to load DuckLake distributed config from SIGIRO_* env vars.
/// Returns None if SIGIRO_CATALOG_URL is not set (will use DuckDB file default).
pub fn load_config() -> Option<StorageConfig> {
    match envy::prefixed("SIGIRO_").from_env::<StorageConfig>() {
        Ok(config) => Some(config),
        Err(envy::Error::MissingValue(_)) => None,
        Err(e) => {
            tracing::warn!("invalid storage config, using DuckDB file default: {e}");
            None
        }
    }
}

/// Always returns "lake." — there is no longer a non-DuckLake path.
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
