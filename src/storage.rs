use anyhow::{Context, Result};
use camino::Utf8Path;
use duckdb::Connection;
use tracing::instrument;
use url::Url;

/// Initialize DuckLake with a DuckDB-native file catalog + local Parquet (zero
/// config). Tables accessible as `lake.sigiro_*`.
/// Catalog stored in `project_dir/catalog.ducklake` (a DuckDB file catalog, NOT
/// SQLite — see the ATTACH below for why).
/// Parquet stored in `project_dir/catalog.ducklake.files/` (auto-created by DuckLake).
#[instrument(skip(conn), fields(dir = %project_dir))]
pub fn init_default(conn: &Connection, project_dir: &Utf8Path) -> Result<()> {
    let project_dir_str = project_dir.as_str();
    std::fs::create_dir_all(project_dir)
        .with_context(|| format!("failed to create project directory: {project_dir}",))?;

    // Load required extensions (ducklake for storage; sqlite is kept loaded for
    // DuckLake compatibility, though the catalog itself is a DuckDB file, not
    // a SQLite database — see the ATTACH below).
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
    tenant_id: &str,
) -> Result<()> {
    if config.s3_bucket.is_empty() {
        install_extensions_with_progress(conn, &["ducklake", "postgres"])?;
    } else {
        install_extensions_with_progress(conn, &["ducklake", "postgres", "httpfs"])?;
    }

    let attach = distributed_attach_sql(config, project_dir, tenant_id)?;

    if let Some(secret_sql) = &attach.secret_sql {
        conn.execute_batch(secret_sql)
            .context("failed to configure S3 secret")?;
        tracing::info!(
            region = %config.s3_region,
            bucket = %config.s3_bucket,
            scope = ?attach.s3_scope,
            "S3 credentials configured"
        );
    }

    conn.execute_batch(&attach.attach_sql)
        .context("failed to attach DuckLake with PostgreSQL catalog")?;
    if config.s3_bucket.is_empty() {
        tracing::info!(
            catalog = %attach.catalog_path,
            tenant_id,
            metadata_schema = %attach.metadata_schema,
            data_path = %attach.data_path,
            "DuckLake attached — catalog: PostgreSQL, data: local Parquet"
        );
    } else {
        tracing::info!(
            catalog = %attach.catalog_path,
            tenant_id,
            metadata_schema = %attach.metadata_schema,
            data_path = %attach.data_path,
            "DuckLake attached — catalog: PostgreSQL, data: S3"
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
    pub s3_endpoint: String,
    pub s3_key_id: String,
    pub s3_secret: String,
    /// `USE_SSL` for the S3 secret SQL. `true` for real S3/R2; `false` for
    /// plain-HTTP dev/CI S3-compatible stores (MinIO). See
    /// `crate::config::S3_USE_SSL`.
    pub s3_use_ssl: bool,
}

/// The storage backend this process runs against, resolved ONCE from the
/// environment at boot (see [`StorageMode::from_env`]) and threaded through the
/// startup / pool / auth selection paths — so the mode is decided in a single
/// place instead of each call site re-deriving it from [`load_config`].
///
/// sigiro is a single binary with a dual-mode storage contract along ONE
/// coherent config axis:
/// - **local file** (zero-config default): a DuckDB-native `catalog.ducklake`
///   file catalog + local Parquet, one per project dir. No external services.
/// - **distributed/hosted**: a shared PostgreSQL catalog (multi-writer safe,
///   per-tenant metadata schemas) with data files either on local disk or in
///   S3/R2.
///
/// The axis is coherent by construction: S3/R2 data is only meaningful WITH a
/// PostgreSQL catalog, so a bucket configured without a catalog URL is rejected
/// at [`from_env`](StorageMode::from_env) rather than silently ignored.
#[derive(Debug)]
pub enum StorageMode {
    /// Zero-config self-host edge: DuckDB file catalog + local Parquet.
    /// Selected when `SIGIRO_CATALOG_URL` is unset.
    LocalFile,
    /// Hosted/distributed: shared PostgreSQL catalog + optional S3/R2 data.
    /// Selected when `SIGIRO_CATALOG_URL` is set.
    Distributed(StorageConfig),
}

impl StorageMode {
    /// Resolve the storage mode from the environment ONCE, validating the
    /// config as a coherent axis (fail-loud on incoherence). Call at boot.
    pub fn from_env() -> Result<Self> {
        match load_config()? {
            Some(config) => Ok(Self::Distributed(config)),
            None => {
                // Coherence gate: S3/R2 data storage requires the distributed
                // PostgreSQL catalog. A bucket set without a catalog URL is a
                // misconfiguration — fail loud instead of silently dropping to
                // local-file mode and ignoring the operator's clear intent to
                // put data in S3.
                let s3_bucket = crate::config::S3_BUCKET
                    .value()
                    .map_err(anyhow::Error::from)?;
                anyhow::ensure!(
                    s3_bucket.is_empty(),
                    "SIGIRO_S3_BUCKET is set (={s3_bucket:?}) but SIGIRO_CATALOG_URL is not — \
                     S3/R2 data storage requires the distributed PostgreSQL catalog. Set \
                     SIGIRO_CATALOG_URL to enable distributed mode, or unset SIGIRO_S3_BUCKET to \
                     use the zero-config local file catalog."
                );
                Ok(Self::LocalFile)
            }
        }
    }

    /// `true` in distributed (PostgreSQL catalog) mode.
    pub fn is_distributed(&self) -> bool {
        matches!(self, Self::Distributed(_))
    }

    /// The distributed storage config, or `None` in local-file mode.
    pub fn distributed_config(&self) -> Option<&StorageConfig> {
        match self {
            Self::Distributed(config) => Some(config),
            Self::LocalFile => None,
        }
    }

    /// `true` only when data files live in S3/R2 (distributed + bucket set).
    /// This is the gate for skipping the local-filesystem lockdown, since
    /// DuckLake needs network access to reach S3.
    pub fn uses_s3(&self) -> bool {
        matches!(self, Self::Distributed(config) if !config.s3_bucket.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistributedAttachSql {
    pub catalog_path: String,
    pub metadata_schema: String,
    pub data_path: String,
    pub secret_sql: Option<String>,
    pub s3_scope: Option<String>,
    pub attach_sql: String,
}

/// `"default"` is a valid tenant id — it is NOT reserved. In distributed mode
/// every tenant's project dir lives under `data_dir/tenants/<id>` (see
/// [`tenant_data_dir`]), so a tenant literally named `default` gets
/// `data_dir/tenants/default`, never `data_dir/default` (the LOCAL-mode
/// default project dir, a completely different code path/config). Its
/// metadata schema (`tenant_default`) and S3 data prefix (`default/`) are
/// likewise namespaced by the same `tenant_` / bucket-prefix scheme every
/// other tenant gets — nothing about the name `default` collides with
/// anything. `"extensions"` and `"tenants"` remain reserved because they name
/// real fixed directories under `data_dir` that a tenant dir would shadow.
pub fn sanitize_tenant_id(tenant_id: &str) -> Result<String> {
    anyhow::ensure!(!tenant_id.is_empty(), "tenant id must not be empty");
    anyhow::ensure!(
        tenant_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_'),
        "tenant id must contain only ASCII letters, digits, and underscores"
    );
    anyhow::ensure!(
        tenant_id.len() <= 56,
        "tenant id is too long: metadata schema tenant_<id> must fit PostgreSQL's 63-byte identifier limit"
    );
    anyhow::ensure!(
        !matches!(tenant_id, "extensions" | "tenants"),
        "tenant id {tenant_id:?} is reserved (collides with the shared extension dir \
         or the tenants/ parent dir)"
    );
    Ok(tenant_id.to_string())
}

pub fn tenant_metadata_schema(tenant_id: &str) -> Result<String> {
    let token = sanitize_tenant_id(tenant_id)?;
    Ok(format!("tenant_{token}"))
}

/// Per-tenant data directory in distributed mode: `<data_dir>/tenants/<tenant_id>`.
/// Nesting under `tenants/` (rather than `<data_dir>/<tenant_id>` directly) keeps
/// per-tenant dirs from ever colliding with sibling top-level dirs (the default
/// project's `default/`, the shared `extensions/` dir). Shared by `ProjectPool`
/// (the actual per-tenant connection) and the investigate handler's data-existence
/// check so the two can never disagree on layout.
pub fn tenant_data_dir(data_dir: &Utf8Path, tenant_id: &str) -> Result<camino::Utf8PathBuf> {
    Ok(data_dir
        .join("tenants")
        .join(sanitize_tenant_id(tenant_id)?))
}

pub fn distributed_attach_sql(
    config: &StorageConfig,
    project_dir: &Utf8Path,
    tenant_id: &str,
) -> Result<DistributedAttachSql> {
    let tenant_token = sanitize_tenant_id(tenant_id)?;
    let metadata_schema = format!("tenant_{tenant_token}");
    let catalog_path = postgres_uri_to_libpq(&config.catalog_url)?;
    let catalog_path_sql = escape_sql_string(&catalog_path);
    let metadata_schema_sql = escape_sql_string(&metadata_schema);

    let (data_path, secret_sql, s3_scope) = if config.s3_bucket.is_empty() {
        std::fs::create_dir_all(project_dir).with_context(|| {
            format!("failed to create project directory for local data: {project_dir}",)
        })?;
        (project_dir.as_str().to_string(), None, None)
    } else {
        anyhow::ensure!(
            !config.s3_endpoint.is_empty(),
            "SIGIRO_S3_BUCKET is set but SIGIRO_S3_ENDPOINT is missing — required to reach an \
             S3-compatible store (e.g. Cloudflare R2's <account>.r2.cloudflarestorage.com)"
        );
        anyhow::ensure!(
            !config.s3_key_id.is_empty(),
            "SIGIRO_S3_BUCKET is set but no S3 key ID is configured — set SIGIRO_S3_KEY_ID or \
             AWS_ACCESS_KEY_ID"
        );
        anyhow::ensure!(
            !config.s3_secret.is_empty(),
            "SIGIRO_S3_BUCKET is set but no S3 secret is configured — set SIGIRO_S3_SECRET or \
             AWS_SECRET_ACCESS_KEY"
        );

        let data_path = format!("s3://{}/{tenant_token}/", config.s3_bucket);
        let scope = data_path.clone();
        let secret_sql = format!(
            "CREATE OR REPLACE SECRET s3_sigiro (TYPE S3, KEY_ID '{}', SECRET '{}', REGION '{}', \
             ENDPOINT '{}', URL_STYLE 'path', USE_SSL {}, SCOPE '{}');",
            escape_sql_string(&config.s3_key_id),
            escape_sql_string(&config.s3_secret),
            escape_sql_string(&config.s3_region),
            escape_sql_string(&config.s3_endpoint),
            config.s3_use_ssl,
            escape_sql_string(&scope)
        );
        (data_path, Some(secret_sql), Some(scope))
    };

    let attach_sql = format!(
        "ATTACH 'ducklake:postgres:{catalog_path_sql}' AS lake \
         (METADATA_SCHEMA '{metadata_schema_sql}', DATA_PATH '{}', CREATE_IF_NOT_EXISTS true)",
        escape_sql_string(&data_path)
    );

    Ok(DistributedAttachSql {
        catalog_path,
        metadata_schema,
        data_path,
        secret_sql,
        s3_scope,
        attach_sql,
    })
}

/// Load SIGIRO_* env vars to configure distributed (PostgreSQL catalog) storage.
/// Returns `Ok(None)` if `SIGIRO_CATALOG_URL` is not set — caller should use the
/// local DuckDB file catalog (see [`init_default`]).
/// Returns an error if env vars are set but malformed.
///
/// This is the low-level env reader. Boot code should call
/// [`StorageMode::from_env`], which wraps this AND validates the config as a
/// coherent axis (e.g. S3 bucket without a catalog URL is rejected).
pub fn load_config() -> Result<Option<StorageConfig>> {
    let catalog_url = match crate::config::CATALOG_URL.value() {
        Ok(v) if !v.is_empty() => v,
        Ok(_) => anyhow::bail!(
            "SIGIRO_CATALOG_URL is set but empty — provide a valid PostgreSQL catalog URL or unset it"
        ),
        Err(_) => return Ok(None), // not set — use the local DuckDB file catalog
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
    let s3_endpoint = config::S3_ENDPOINT.value().map_err(anyhow::Error::from)?;
    let s3_key_id = resolve_with_aws_fallback(&config::S3_KEY_ID, &config::AWS_ACCESS_KEY_ID)?;
    let s3_secret = resolve_with_aws_fallback(&config::S3_SECRET, &config::AWS_SECRET_ACCESS_KEY)?;
    let s3_use_ssl = config::S3_USE_SSL.value().map_err(anyhow::Error::from)?;

    Ok(Some(StorageConfig {
        catalog_url,
        s3_bucket,
        s3_region,
        s3_endpoint,
        s3_key_id,
        s3_secret,
        s3_use_ssl,
    }))
}

/// Read `primary`, falling back to `secondary` (the conventional AWS_* name)
/// when `primary` is unset/empty. Used so SIGIRO_S3_KEY_ID/SIGIRO_S3_SECRET
/// can be overridden explicitly while still picking up plain AWS_ACCESS_KEY_ID/
/// AWS_SECRET_ACCESS_KEY from the environment by default.
fn resolve_with_aws_fallback(
    primary: &typed_env::Envar<String>,
    secondary: &typed_env::Envar<String>,
) -> Result<String> {
    let value = primary.value().map_err(anyhow::Error::from)?;
    if value.is_empty() {
        secondary.value().map_err(anyhow::Error::from)
    } else {
        Ok(value)
    }
}

/// Convert a `postgresql://user:***@host:port/dbname` URI to a BARE libpq
/// keyword string: `dbname=db host=h user=u password=p port=5432`.
/// Callers add their own context prefix: DuckLake wraps it as
/// `ducklake:postgres:<libpq>`, and the control-plane attaches it directly via
/// `ATTACH '<libpq>' AS ... (TYPE postgres)`. A stray `postgres:` prefix here
/// broke the direct control-plane attach ("invalid connection option postgres:dbname").
pub(crate) fn postgres_uri_to_libpq(uri: &str) -> Result<String> {
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
                "dbname={dbname} host={host} user={user} password={password} port={port}"
            ))
        } else {
            // Already libpq key=value — strip any leading `postgres:` so the
            // result is a bare libpq string in every case.
            Ok(uri.strip_prefix("postgres:").unwrap_or(uri).to_string())
        }
    } else {
        // Assume it's a path or other format — return as-is
        Ok(uri.to_string())
    }
}

pub(crate) fn escape_sql_string(s: &str) -> String {
    s.replace('\'', "''")
}

pub const TABLE_PREFIX: &str = "lake.";

pub fn table_prefix() -> &'static str {
    TABLE_PREFIX
}

pub fn resolve_extension_dir(data_dir: &Utf8Path) -> camino::Utf8PathBuf {
    crate::config::EXTENSION_DIR
        .value()
        .ok()
        .filter(|s| !s.is_empty())
        .map(camino::Utf8PathBuf::from)
        .unwrap_or_else(|| data_dir.join("extensions"))
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
///
/// Takes the pool's overall `data_dir` (not a per-project dir) and resolves
/// the target directory via [`resolve_extension_dir`] — the SAME resolution
/// `attach_ducklake` uses at serve-time (`SIGIRO_EXTENSION_DIR` if set, else
/// `data_dir/extensions`, deliberately WITHOUT a per-project segment: the
/// extension cache is shared across all projects). Previously this hardcoded
/// `<project_dir>/extensions`, which for the common case (no
/// `SIGIRO_EXTENSION_DIR` override, prewarming the "default" project)
/// resolved to `<data_dir>/default/extensions` — a directory
/// `attach_ducklake` never reads from (it reads `<data_dir>/extensions`) —
/// so `sigiro fetch-extensions` silently prewarmed a directory serve-time
/// never consulted, defeating its entire purpose.
///
/// Includes `httpfs` and `quack`: `attach_ducklake` loads both, unconditionally
/// of `SIGIRO_QUERY_SURFACE`, right before it locks down filesystem access
/// (`enable_external_access = false` is instance-global and irreversible — an
/// extension not already loaded before that point can never load afterward on
/// this or any cloned connection). Pre-warming them here means that lockdown
/// never has to fall back to a live network install. `quack` is warmed via
/// [`crate::quack::load_and_pin`] (not [`install_extensions_with_progress`])
/// so its version pin is verified here too, not just at serve-time — a drift
/// between what `sigiro fetch-extensions` cached and what serve-time expects
/// would otherwise surface as a confusing later failure.
pub fn prewarm_extensions(data_dir: &Utf8Path) -> Result<()> {
    let ext_dir = resolve_extension_dir(data_dir);
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
    install_extensions_with_progress(
        &conn,
        &[
            "ducklake", "sqlite", "json", "parquet", "postgres", "httpfs",
        ],
    )?;
    tracing::info!("pre-warming the quack query-surface extension (version-pinned)...");
    crate::quack::load_and_pin(&conn).context("failed to pre-warm the quack extension")?;
    tracing::info!("DuckDB extensions pre-warmed successfully");
    Ok(())
}
