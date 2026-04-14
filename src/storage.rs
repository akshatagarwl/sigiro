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

/// Initialize DuckLake with PostgreSQL catalog + optional S3 data files.
/// Multi-writer safe — multiple sigiro instances can attach to the same catalog.
pub fn init_distributed(conn: &Connection, config: &StorageConfig) -> Result<()> {
    // Load required extensions
    conn.execute_batch("INSTALL ducklake; INSTALL postgres; LOAD ducklake; LOAD postgres;")
        .context("failed to install/load ducklake + postgres extensions")?;
    tracing::info!("loaded DuckDB extensions: ducklake, postgres");

    // Configure S3 credentials if bucket is set
    if !config.s3_bucket.is_empty() {
        let s3_prefix = if config.s3_prefix.is_empty() {
            String::new()
        } else {
            format!("{}/", config.s3_prefix.trim_end_matches('/'))
        };

        let secret_sql = format!(
            "CREATE SECRET s3_sigiro (TYPE S3, REGION '{}');",
            config.s3_region
        );
        conn.execute_batch(&secret_sql)
            .context("failed to configure S3 secret")?;
        tracing::info!(region = %config.s3_region, bucket = %config.s3_bucket, "S3 credentials configured");

        // Attach DuckLake with PostgreSQL catalog + S3 data path
        let s3_data_path = format!("s3://{}/{}data", config.s3_bucket, s3_prefix);
        let attach_sql = format!(
            "ATTACH 'ducklake:{}' AS lake (DATA_PATH '{}')",
            config.catalog_url, s3_data_path
        );
        conn.execute_batch(&attach_sql)
            .context("failed to attach DuckLake with PostgreSQL catalog + S3")?;
        tracing::info!(
            catalog = %config.catalog_url,
            data_path = %s3_data_path,
            "DuckLake attached — catalog: PostgreSQL, data: S3"
        );
    } else {
        // PostgreSQL catalog + local Parquet (useful for multi-writer without S3)
        let attach_sql = format!("ATTACH 'ducklake:{}' AS lake", config.catalog_url);
        conn.execute_batch(&attach_sql)
            .context("failed to attach DuckLake with PostgreSQL catalog")?;
        tracing::info!(
            catalog = %config.catalog_url,
            "DuckLake attached — catalog: PostgreSQL, data: local Parquet"
        );
    }

    Ok(())
}

/// Load distributed storage config from SIGIRO_* env vars.
/// Returns Some if SIGIRO_CATALOG_URL is set, None otherwise (use local SQLite).
pub fn load_config() -> Option<StorageConfig> {
    let catalog_url = std::env::var("SIGIRO_CATALOG_URL").ok()?;

    if catalog_url.is_empty() {
        return None;
    }

    Some(StorageConfig {
        catalog_url,
        s3_bucket: std::env::var("SIGIRO_S3_BUCKET").unwrap_or_default(),
        s3_region: std::env::var("SIGIRO_S3_REGION").unwrap_or_else(|_| "us-east-1".to_string()),
        s3_prefix: std::env::var("SIGIRO_S3_PREFIX").unwrap_or_default(),
    })
}

/// Distributed storage configuration.
/// PostgreSQL catalog for multi-writer + optional S3 data files.
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
