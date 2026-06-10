use anyhow::{Context, Result};
use camino::Utf8Path;
use duckdb::Connection;
use tracing::instrument;
use url::Url;

/// Initialize DuckLake with SQLite catalog + local Parquet (zero config).
/// Tables accessible as `lake.sigiro_*`.
/// Catalog stored in `project_dir/catalog.sqlite`.
/// Parquet stored in `project_dir/catalog.ducklake.files/` (auto-created by DuckLake).
#[instrument(skip(conn), fields(dir = %project_dir))]
pub fn init_default(conn: &Connection, project_dir: &Utf8Path) -> Result<()> {
    let project_dir_str = project_dir.as_str();
    std::fs::create_dir_all(project_dir)
        .with_context(|| format!("failed to create project directory: {project_dir}",))?;

    // Load required extensions (ducklake for storage, sqlite for catalog).
    // Emit a progress line before each step so cold starts don't look like
    // a hang (F-01/F-29). With a fresh extension dir, DuckDB synchronously
    // downloads ~5MB of .duckdb_extension files over HTTP and emits no
    // output of its own; users see nothing for 30-90s.
    install_extensions_with_progress(conn, &["ducklake", "sqlite"])?;

    // Attach DuckLake with DuckDB file catalog + local Parquet.
    // Uses catalog.ducklake (DuckDB native catalog) instead of SQLite
    // because the appender API is tested and verified to work with
    // DuckDB file catalogs (see tests/ducklake_appender_test.rs).
    let attach_sql = format!("ATTACH 'ducklake:{project_dir_str}/catalog.ducklake' AS lake");
    conn.execute_batch(&attach_sql)
        .context("failed to attach DuckLake catalog")?;
    tracing::info!("DuckLake attached — catalog: DuckDB file, data: local Parquet");

    Ok(())
}

/// Initialize DuckLake with PostgreSQL catalog + optional S3 data files.
/// Multi-writer safe — multiple sigiro instances can attach to the same catalog.
#[instrument(skip(conn), fields(dir = %project_dir))]
pub fn init_distributed(
    conn: &Connection,
    config: &StorageConfig,
    project_dir: &Utf8Path,
) -> Result<()> {
    install_extensions_with_progress(conn, &["ducklake", "postgres"])?;

    // Convert catalog URL to libpq format that DuckLake expects.
    // Accepts both postgresql:// URI and postgres:host=... (libpq format, auto-converted).
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
            format!("failed to create project directory for local data: {project_dir}",)
        })?;
        let local_data_path = project_dir.as_str();
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
/// `catalog_url` can be:
/// - A libpq connection string: `postgres:dbname=db host=localhost user=u password=p port=5432`
/// - A PostgreSQL URI: `postgresql://user:***@host:port/dbname` (auto-converted to libpq format)
#[derive(Debug)]
pub struct StorageConfig {
    pub catalog_url: String,
    pub s3_bucket: String,
    pub s3_region: String,
    pub s3_prefix: String,
}

/// Load SIGIRO_* env vars to configure distributed (PostgreSQL catalog) storage.
/// Returns `Ok(None)` if `SIGIRO_CATALOG_URL` is not set — caller should use local SQLite.
/// Returns an error if env vars are set but malformed.
pub fn load_config() -> Result<Option<StorageConfig>> {
    let catalog_url = match crate::config::CATALOG_URL.value() {
        Ok(v) if !v.is_empty() => v,
        Ok(_) => anyhow::bail!(
            "SIGIRO_CATALOG_URL is set but empty — provide a valid PostgreSQL catalog URL or unset it"
        ),
        Err(_) => return Ok(None), // not set — use local SQLite
    };

    // At this point catalog_url is a non-empty string (Ok + !is_empty)
    // Proceed to use it as distributed catalog
    use crate::config;
    let s3_bucket = config::S3_BUCKET.value().map_err(anyhow::Error::from)?;
    let s3_region = config::S3_REGION.value().map_err(anyhow::Error::from)?;
    if !s3_bucket.is_empty() && s3_region.is_empty() {
        anyhow::bail!(
            "SIGIRO_S3_BUCKET is set but SIGIRO_S3_REGION is missing — provide a valid AWS region"
        );
    }
    if s3_bucket.is_empty() && !s3_region.is_empty() {
        tracing::warn!("SIGIRO_S3_REGION is set but SIGIRO_S3_BUCKET is empty — S3 region ignored");
    }

    Ok(Some(StorageConfig {
        catalog_url,
        s3_bucket,
        s3_region,
        s3_prefix: crate::config::S3_PREFIX
            .value()
            .map_err(anyhow::Error::from)?,
    }))
}

/// Convert a `postgresql://user:***@host:port/dbname` URI to libpq `key=value` format
/// that DuckLake expects: `postgres:dbname=db host=h user=u password=p port=5432`
fn postgres_uri_to_libpq(uri: &str) -> Result<String> {
    // Already in libpq format — return as-is with postgres: prefix
    if uri.starts_with("postgres:") || uri.starts_with("postgresql://") {
        // Check if it's a URI (contains //) or already libpq key=value
        if uri.contains("//") {
            // Parse URI: postgresql://user:***@host:port/dbname
            let parsed = Url::parse(uri).context("invalid postgres URI: failed to parse URL")?;

            let user = parsed.username();
            if user.is_empty() {
                anyhow::bail!("invalid postgres URI: expected non-empty username");
            }
            // password: required — empty password is invalid for PostgreSQL in distributed catalog mode
            let password = parsed.password().filter(|p| !p.is_empty()).ok_or_else(|| {
                anyhow::anyhow!(
                    "postgres URI missing or empty password — required for distributed catalog"
                )
            })?;
            let host = parsed.host_str().ok_or_else(|| {
                anyhow::anyhow!("postgres URI missing host — required for distributed catalog")
            })?;
            // port: required — default 5432 should not be implicit
            let port = parsed.port().ok_or_else(|| {
                anyhow::anyhow!("postgres URI missing port — required for distributed catalog")
            })?;
            let dbname = parsed.path().trim_start_matches('/');
            if dbname.is_empty() {
                anyhow::bail!("invalid postgres URI: expected non-empty database name");
            }

            Ok(format!(
                "postgres:dbname={dbname} host={host} user={user} password={password} port={port}"
            ))
        } else {
            // Already libpq format, just ensure postgres: prefix
            if uri.starts_with("postgres:") {
                Ok(uri.to_string())
            } else {
                Ok(format!("postgres:{uri}"))
            }
        }
    } else {
        // Assume it's a path or other format — return as-is
        Ok(uri.to_string())
    }
}

pub const TABLE_PREFIX: &str = "lake.";

pub fn table_prefix() -> &'static str {
    TABLE_PREFIX
}

/// Install and load a list of DuckDB extensions, emitting a progress line
/// before and after each so cold starts don't appear to hang (F-01/F-29).
///
/// DuckDB's `INSTALL` command silently downloads ~5MB of `.duckdb_extension`
/// files on first use. The download is synchronous and can take 30-90s
/// on a fresh data dir, with no progress output. The user sees nothing
/// and concludes the process has died.
pub fn install_extensions_with_progress(conn: &Connection, names: &[&str]) -> Result<()> {
    let total = names.len();
    for (i, name) in names.iter().enumerate() {
        tracing::info!(
            "installing DuckDB extension {name} ({}/{}) — first run may download ~5MB, please wait",
            i + 1,
            total
        );
        conn.execute_batch(&format!("INSTALL {name}; LOAD {name};"))
            .with_context(|| format!("failed to install/load extension {name}"))?;
        tracing::info!("loaded DuckDB extension: {name}");
    }
    Ok(())
}

/// Pre-download DuckDB extensions into the standard extension directory
/// so the first `sigiro serve` doesn't hang on a 30-90s blocking download.
/// This is what `sigiro fetch-extensions` invokes.
pub fn prewarm_extensions(project_dir: &Utf8Path) -> Result<()> {
    let ext_dir = project_dir.join("extensions");
    std::fs::create_dir_all(&ext_dir)
        .with_context(|| format!("failed to create extensions directory: {ext_dir}"))?;
    let conn = Connection::open_in_memory()
        .context("failed to open in-memory DuckDB for extension prewarm")?;
    let ext_dir_str = ext_dir.as_str();
    conn.execute_batch(&format!("SET extension_directory = '{ext_dir_str}';"))
        .context("failed to set extension_directory")?;
    tracing::info!(
        "pre-warming DuckDB extensions into {} (may take 30-90s on first run)...",
        ext_dir
    );
    install_extensions_with_progress(&conn, &["ducklake", "sqlite", "json", "parquet"])?;
    tracing::info!("DuckDB extensions pre-warmed successfully");
    Ok(())
}
