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
pub fn init_distributed(
    conn: &Connection,
    config: &StorageConfig,
    project_dir: &Path,
) -> Result<()> {
    // Load required extensions
    conn.execute_batch("INSTALL ducklake; INSTALL postgres; LOAD ducklake; LOAD postgres;")
        .context("failed to install/load ducklake + postgres extensions")?;
    tracing::info!("loaded DuckDB extensions: ducklake, postgres");

    // Convert catalog URL to libpq format that DuckLake expects.
    // Accepts both postgresql:// URI and postgres:***@host:5432/dbname` (auto-converted to libpq format)
    let catalog_path = postgres_uri_to_libpq(&config.catalog_url)?;

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
            "ATTACH 'ducklake:{catalog_path}' AS lake (DATA_PATH '{}')",
            s3_data_path
        );
        conn.execute_batch(&attach_sql)
            .context("failed to attach DuckLake with PostgreSQL catalog + S3")?;
        tracing::info!(
            catalog = %catalog_path,
            data_path = %s3_data_path,
            "DuckLake attached — catalog: PostgreSQL, data: S3"
        );
    } else {
        // PostgreSQL catalog + local Parquet (useful for multi-writer without S3)
        std::fs::create_dir_all(project_dir).with_context(|| {
            format!(
                "failed to create project directory for local data: {}",
                project_dir.display()
            )
        })?;
        let local_data_path = project_dir.to_string_lossy();
        let attach_sql =
            format!("ATTACH 'ducklake:{catalog_path}' AS lake (DATA_PATH '{local_data_path}')");
        conn.execute_batch(&attach_sql)
            .context("failed to attach DuckLake with PostgreSQL catalog")?;
        tracing::info!(
            catalog = %catalog_path,
            data_path = %local_data_path,
            "DuckLake attached — catalog: PostgreSQL, data: local Parquet"
        );
    }

    Ok(())
}

/// Distributed storage configuration.
/// PostgreSQL catalog for multi-writer + optional S3 data files.
///
/// `catalog_url` can be:
/// - A libpq connection string: `postgres:dbname=db host=localhost user=u password=p port=5432`
/// - A PostgreSQL URI: `postgresql://user:***@host:5432/dbname` (auto-converted to libpq format)
#[derive(Debug)]
pub struct StorageConfig {
    pub catalog_url: String,
    pub s3_bucket: String,
    pub s3_region: String,
    pub s3_prefix: String,
}

/// A parsed host:port pair.
///
/// Used when parsing PostgreSQL URIs to extract the host and default port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostPort {
    pub host: &'static str,
    pub port: u16,
}

impl HostPort {
    const DEFAULT_POSTGRES_PORT: u16 = 5432;

    /// Parse a host:port string, defaulting to 5432 if no port given.
    fn parse(s: &str) -> Result<Self> {
        match s.split_once(':') {
            Some((host, port_str)) => {
                let port = port_str
                    .parse()
                    .context("invalid port in postgres URI: must be a number")?;
                Ok(Self { host, port })
            }
            None => Ok(Self {
                host: s,
                port: Self::DEFAULT_POSTGRES_PORT,
            }),
        }
    }
}

/// Convert a `postgresql://user:***@host:port/dbname` URI to libpq `key=value` format
/// that DuckLake expects: `postgres:dbname=db host=h user=u password=p port=5432`
fn postgres_uri_to_libpq(uri: &str) -> Result<String> {
    // Already in libpq format — return as-is with postgres: prefix
    if uri.starts_with("postgres:") || uri.starts_with("postgresql://") {
        // Check if it's a URI (contains //) or already libpq key=value
        if uri.starts_with("postgresql://") || uri.starts_with("postgres://") {
            // Parse URI: postgresql://user:***@host:port/dbname
            let stripped = uri
                .trim_start_matches("postgres://")
                .trim_start_matches("postgresql://");

            // Split user:pass@host:port/dbname
            let (credentials, rest) = stripped
                .split_once('@')
                .context("invalid postgres URI: expected user:pass@host format")?;

            let (user, password) = credentials
                .split_once(':')
                .context("invalid postgres URI: expected user:password in credentials")?;

            // rest = host:port/dbname
            let (host_port_str, dbname) = rest
                .split_once('/')
                .context("invalid postgres URI: expected host:port/dbname")?;

            let host_port = HostPort::parse(host_port_str)?;

            Ok(format!(
                "postgres:dbname={dbname} host={} user={user} password={password} port={}",
                host_port.host, host_port.port
            ))
        } else {
            // Already libpq format, just ensure postgres: prefix
            let libpq = uri.trim_start_matches("postgresql:");
            if libpq.starts_with("postgres:") {
                Ok(uri.to_string())
            } else {
                Ok(format!("postgres:{libpq}"))
            }
        }
    } else {
        // Assume it's a path or other format — return as-is
        Ok(uri.to_string())
    }
}

/// Always returns "lake." — all tables are accessed through DuckLake.
pub fn table_prefix() -> &'static str {
    "lake."
}
