use crate::sql::escape_sql_string;
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

    // Load the ONLY extension this mode needs: ducklake. The catalog is a
    // DuckDB file (see the ATTACH below), so the sqlite extension — needed only
    // for `ducklake:sqlite:` catalog backends — is NOT required here. It used to
    // be installed "for compatibility" but that pulled a runtime download into
    // the read-only baked extension dir (/opt/sigiro/extensions), failing with
    // "Permission denied" and wedging the attach-retry loop in local mode.
    // Emit a progress line before each step so cold starts don't look like
    // a hang (F-01/F-29). With a fresh extension dir, DuckDB synchronously
    // downloads ~5MB of .duckdb_extension files over HTTP and emits no
    // output of its own; users see nothing for 30-90s.
    install_extensions_with_progress(conn, &["ducklake"])?;

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

/// A classified DuckLake attach failure. Carries enough for the connection
/// pool's retry loop to decide whether trying again can ever help: transient
/// causes (the catalog Postgres still booting, a sibling container's DNS not yet
/// resolvable) clear on their own; permanent ones (the catalog pinned to a
/// different storage backend, wrong credentials, a misconfiguration) never will,
/// and retrying them only wedges the runtime behind the blocking retry loop.
///
/// The raw DuckDB error text is deliberately discarded before this is built: a
/// Postgres ATTACH failure can echo the connection string (with its password),
/// and scrubbing an arbitrary secret byte-form out of error text is not reliable
/// — so we classify in-process and surface only a secret-free category + hint.
#[derive(Debug, Clone)]
pub struct AttachError {
    pub(crate) category: &'static str,
    pub(crate) retryable: bool,
    message: String,
}

impl AttachError {
    /// Categories whose cause is transient and clears without operator action.
    /// Everything else is permanent — fail fast rather than retry.
    fn is_retryable(category: &str) -> bool {
        matches!(
            category,
            "dns" | "unreachable" | "unknown" | "catalog_conflict"
        )
    }
}

impl std::fmt::Display for AttachError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AttachError {}

/// Initialize DuckLake with PostgreSQL catalog + optional S3 data files.
/// Multi-writer safe — multiple sigiro instances can attach to the same catalog.
#[instrument(skip(conn), fields(dir = %project_dir))]
pub(crate) fn init_distributed(
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
        // Generic message: the DuckDB error could echo the CREATE SECRET SQL
        // (with the S3 access key/secret) — never surface it.
        conn.execute_batch(secret_sql).map_err(|_| {
            anyhow::anyhow!(
                "failed to configure S3 secret — verify SIGIRO_S3_* credentials and endpoint"
            )
        })?;
        tracing::info!(
            region = %config.s3_region,
            bucket = %config.s3_bucket,
            scope = ?attach.s3_scope,
            "S3 credentials configured"
        );
    }

    conn.execute_batch(&attach.attach_sql).map_err(|error| {
        // Deliberately DO NOT include the DuckDB error: a Postgres ATTACH
        // failure can echo the connection string (with password) verbatim, and
        // reliably scrubbing an arbitrary secret byte-form out of error text is
        // not achievable. Classify it in-process, discard the raw text, and fail
        // loud with an actionable, secret-free category.
        let detail = error.to_string().to_ascii_lowercase();
        let category = if detail.contains("password authentication failed")
            || detail.contains("authentication failed")
        {
            "authentication"
        } else if detail.contains("could not translate host")
            || detail.contains("name or service not known")
            || detail.contains("nodename nor servname")
            || detail.contains("name resolution")
        {
            "dns"
        } else if detail.contains("ssl") || detail.contains("tls") || detail.contains("certificate")
        {
            "tls"
        } else if detail.contains("permission denied")
            || detail.contains("insufficient privilege")
            || detail.contains("must be owner")
        {
            if detail.contains("s3")
                || detail.contains("http 403")
                || detail.contains("accessdenied")
            {
                "object_store_permission"
            } else if detail.contains("extension") || detail.contains("/opt/sigiro") {
                "extension_permission"
            } else if detail.contains("postgres")
                || detail.contains("schema")
                || detail.contains("database")
            {
                "catalog_permission"
            } else if detail.contains("/var/lib/sigiro")
                || detail.contains("file")
                || detail.contains("directory")
                || detail.contains("io error")
            {
                "filesystem_permission"
            } else {
                "permission"
            }
        } else if detail.contains("out of memory") || detail.contains("memory limit") {
            "memory"
        } else if detail.contains("data_path") || detail.contains("data path") {
            "data_path"
        } else if detail.contains("invalid connection option")
            || detail.contains("invalid configuration")
            || detail.contains("parser error")
        {
            "configuration"
        } else if detail.contains("connection refused")
            || detail.contains("could not connect")
            || detail.contains("unable to connect")
            || detail.contains("network is unreachable")
        {
            "unreachable"
        } else if detail.contains("already exists") || detail.contains("conflict") {
            "catalog_conflict"
        } else {
            "unknown"
        };
        let retryable = AttachError::is_retryable(category);
        // `data_path` is the signature of a backend swap: DuckLake pins DATA_PATH
        // into the catalog at creation, so a config that later points elsewhere
        // (e.g. an S3-created catalog now configured for local Parquet) can never
        // attach. Say that plainly instead of sending the operator to chase
        // connectivity that is fine.
        let hint = if category == "data_path" {
            "the tenant catalog was created against a different DATA_PATH (storage \
             backend) than the current config — DuckLake pins data_path at creation, \
             so it cannot be swapped. Restore the original backend (e.g. set \
             SIGIRO_S3_BUCKET) or reset/migrate the tenant catalog"
        } else {
            "verify SIGIRO_CATALOG_URL host/port/credentials and that PostgreSQL is \
             reachable"
        };
        anyhow::Error::from(AttachError {
            category,
            retryable,
            message: format!(
                "failed to attach DuckLake PostgreSQL catalog for tenant {tenant_id} \
                 (metadata schema {}, category {category}) — {hint}",
                attach.metadata_schema,
            ),
        })
    })?;
    // NEVER log the connection string — it embeds the PostgreSQL password.
    // tenant_id + metadata_schema + data_path fully identify the attach.
    if config.s3_bucket.is_empty() {
        tracing::info!(
            tenant_id,
            metadata_schema = %attach.metadata_schema,
            data_path = %attach.data_path,
            "DuckLake attached — catalog: PostgreSQL, data: local Parquet"
        );
    } else {
        tracing::info!(
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

/// Hand-written `Debug` that REDACTS every credential-bearing field so the
/// storage config (and, transitively, [`StorageMode`], which derives `Debug`
/// over this) can never leak secrets if logged. The `catalog_url` embeds the
/// PostgreSQL password, and `s3_key_id`/`s3_secret` are the S3/R2 access
/// credentials — all three are replaced with `<redacted>`. Non-secret fields
/// (bucket, region, endpoint, SSL flag) stay visible for debuggability.
impl std::fmt::Debug for StorageConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageConfig")
            .field("catalog_url", &"<redacted>")
            .field("s3_bucket", &self.s3_bucket)
            .field("s3_region", &self.s3_region)
            .field("s3_endpoint", &self.s3_endpoint)
            .field("s3_key_id", &"<redacted>")
            .field("s3_secret", &"<redacted>")
            .field("s3_use_ssl", &self.s3_use_ssl)
            .finish()
    }
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

#[derive(Clone, PartialEq, Eq)]
pub struct DistributedAttachSql {
    pub(crate) catalog_path: String,
    pub metadata_schema: String,
    pub(crate) data_path: String,
    pub secret_sql: Option<String>,
    pub s3_scope: Option<String>,
    pub attach_sql: String,
}

/// Hand-written so a debug-log of this struct can never leak credentials:
/// `catalog_path` (libpq string with the PG password), `secret_sql` (the S3
/// `CREATE SECRET` with the access key/secret), and `attach_sql` (embeds the
/// catalog_path) are all redacted. Non-secret fields stay visible for ops.
impl std::fmt::Debug for DistributedAttachSql {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DistributedAttachSql")
            .field("catalog_path", &"<redacted>")
            .field("metadata_schema", &self.metadata_schema)
            .field("data_path", &self.data_path)
            .field(
                "secret_sql",
                &self.secret_sql.as_ref().map(|_| "<redacted>"),
            )
            .field("s3_scope", &self.s3_scope)
            .field("attach_sql", &"<redacted>")
            .finish()
    }
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
/// (the actual per-tenant connection) and the diagnose handler's data-existence
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

pub(crate) const TABLE_PREFIX: &str = "lake.";

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
pub(crate) fn install_extensions_with_progress(conn: &Connection, names: &[&str]) -> Result<()> {
    install_extensions_from(conn, names, ExtensionRepo::Core)
}

/// The extensions [`crate::pool::Pool::lock_filesystem`] `LOAD`s immediately
/// before it turns `enable_external_access` off for good.
///
/// This list and that `LOAD` batch must agree: anything named there but not
/// satisfiable at that moment fails the connection, and because the lockdown is
/// irreversible and instance-global there is no recovery. `prewarm_extensions`
/// caches whichever of these the running binary does not already carry, and
/// [`lock_filesystem_loads_are_prewarmed`] asserts the two stay in step.
pub(crate) const LOCKDOWN_PRELOADED_EXTENSIONS: &[&str] = &["parquet", "json", "icu"];

/// Which of `names` this binary does NOT have statically linked, and therefore
/// has to cache as a file before the filesystem lockdown.
///
/// **Why this is a runtime question and not a constant.** `Cargo.toml` builds a
/// different DuckDB per platform: Linux links the merged static archive the
/// Dockerfile prepares (`ar`-merging every fragment, so `json`/`icu`/`parquet`
/// really are built in), while macOS uses the `bundled` feature — DuckDB core
/// compiled from the vendored amalgamation, WITHOUT those extension fragments —
/// because DuckDB publishes no self-contained macOS static archive. The same
/// split applies to the published binaries: the Linux downloads are the same
/// artifact as the image, the macOS downloads are `bundled`.
///
/// Hardcoding either answer breaks the other platform, which is exactly how this
/// bug happened: both `prewarm_extensions` and `lock_filesystem` asserted in
/// comments that these three were static, which was true only of the Docker
/// build, so on macOS `sigiro serve` came up and then answered every query with
/// a 400 about a missing `json.duckdb_extension`.
///
/// Asking DuckDB is exact and costs one query. **`INSTALL` is not a no-op on a
/// statically linked extension** — verified: it downloads from the repository,
/// writes a `.duckdb_extension` file and flips `install_mode` from
/// `STATICALLY_LINKED` to `REPOSITORY`. On the distroless image that write
/// targets a read-only `/opt/sigiro/extensions`, so installing unconditionally
/// would trade a working static load for a needless download at best and a
/// permission failure at worst.
fn extensions_needing_install(conn: &Connection, names: &[&str]) -> Result<Vec<String>> {
    let mut needed = Vec::new();
    for name in names {
        // `install_mode` is NULL for an extension that is neither loaded nor
        // installed, so treat "not statically linked" as the condition rather
        // than testing for any particular non-static value.
        let is_static: bool = conn
            .query_row(
                "SELECT coalesce(install_mode = 'STATICALLY_LINKED', false)
                 FROM duckdb_extensions() WHERE extension_name = ?",
                [name],
                |row| row.get(0),
            )
            .unwrap_or(false);
        if is_static {
            tracing::debug!("extension {name} is statically linked — no file needed");
        } else {
            needed.push((*name).to_string());
        }
    }
    Ok(needed)
}

/// Which DuckDB extension repository an extension is published to. A core
/// extension resolves from `extensions.duckdb.org`; a community extension is
/// NOT there and must be installed `FROM community`, or the download fails
/// with an HTTP 404 that names only the core candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExtensionRepo {
    Core,
    Community,
}

impl ExtensionRepo {
    /// The `FROM <repo>` suffix for an `INSTALL` statement.
    fn install_suffix(self) -> &'static str {
        match self {
            Self::Core => "",
            Self::Community => " FROM community",
        }
    }
}

/// Install + load each extension in `names` from `repo`, logging progress.
pub(crate) fn install_extensions_from(
    conn: &Connection,
    names: &[&str],
    repo: ExtensionRepo,
) -> Result<()> {
    let total = names.len();
    for (i, name) in names.iter().enumerate() {
        tracing::info!(
            "installing DuckDB extension {name} ({}/{}, {repo:?} repo) — first run may download \
             ~5MB, please wait",
            i + 1,
            total
        );
        conn.execute_batch(&format!(
            "INSTALL {name}{}; LOAD {name};",
            repo.install_suffix()
        ))
        .with_context(|| {
            format!("failed to install/load extension {name} from the {repo:?} repo")
        })?;
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
    // sqlite is local-catalog-only (hosted uses a postgres catalog) and
    // downloads on demand for local dev.
    install_extensions_with_progress(&conn, &["ducklake", "postgres", "httpfs"])?;
    // The extensions `lock_filesystem` LOADs, cached only where this binary does
    // not already carry them.
    //
    // This block used to not exist, on the stated premise that
    // core_functions/json/parquet/icu "are statically linked into the binary via
    // the prebuilt DuckDB static bundle, so they need no baked file". That is
    // true of the Linux builds (the Dockerfile ar-merges every fragment, and the
    // published Linux binaries are the same artifact) and FALSE of macOS, which
    // uses Cargo's `bundled` feature and gets DuckDB core without them. The
    // result was that a fresh macOS install served /health 200 and answered
    // every real query with a 400 naming a missing json.duckdb_extension.
    //
    // Deciding per-extension at runtime rather than per-platform at compile time
    // keeps this correct if either build's fragment set changes. See
    // `extensions_needing_install` for why an unconditional INSTALL is wrong.
    let missing = extensions_needing_install(&conn, LOCKDOWN_PRELOADED_EXTENSIONS)?;
    if missing.is_empty() {
        tracing::info!("parquet/json/icu are statically linked into this binary — no files needed");
    } else {
        tracing::info!(
            "caching lockdown-preloaded extensions not static in this binary: {}",
            missing.join(", ")
        );
        let refs: Vec<&str> = missing.iter().map(String::as_str).collect();
        install_extensions_with_progress(&conn, &refs)?;
    }
    // system_stats backs the `sys_*` host-statistics functions federated fleet
    // queries reach (`crate::federate`). `attach_ducklake` installs it right
    // before the irreversible filesystem lockdown, so pre-warming it here is
    // what keeps that path off the network at serve time — same reasoning as
    // httpfs/quack above. Published to the COMMUNITY repo, not core.
    install_extensions_from(&conn, &["system_stats"], ExtensionRepo::Community)
        .context("failed to pre-warm the system_stats extension")?;
    tracing::info!("pre-warming the quack query-surface extension (version-pinned)...");
    crate::quack::load_and_pin(&conn).context("failed to pre-warm the quack extension")?;
    tracing::info!("DuckDB extensions pre-warmed successfully");
    Ok(())
}

#[cfg(test)]
mod redaction_tests {
    use super::*;

    /// The redacting `Debug` impl must never emit the PostgreSQL password or
    /// the S3 key id/secret — neither directly on `StorageConfig` nor through
    /// `StorageMode`, which derives `Debug` over it.
    #[test]
    fn storage_config_debug_redacts_all_credentials() {
        const PG_PASSWORD: &str = "PgP4ssw0rd_SHOULD_NOT_LEAK";
        const S3_KEY_ID: &str = "AKIA_KEY_ID_SHOULD_NOT_LEAK";
        const S3_SECRET: &str = "S3Secret_SHOULD_NOT_LEAK_bytes";

        let config = StorageConfig {
            catalog_url: format!("postgresql://sigiro:{PG_PASSWORD}@db.internal:5432/ducklake"),
            s3_bucket: "telemetry-bucket".to_string(),
            s3_region: "us-east-1".to_string(),
            s3_endpoint: "s3.example.com".to_string(),
            s3_key_id: S3_KEY_ID.to_string(),
            s3_secret: S3_SECRET.to_string(),
            s3_use_ssl: true,
        };

        let config_dbg = format!("{config:?}");
        let mode_dbg = format!("{:?}", StorageMode::Distributed(config));

        for needle in [PG_PASSWORD, S3_KEY_ID, S3_SECRET] {
            assert!(
                !config_dbg.contains(needle),
                "StorageConfig Debug leaked secret {needle:?}: {config_dbg}"
            );
            assert!(
                !mode_dbg.contains(needle),
                "StorageMode Debug leaked secret {needle:?}: {mode_dbg}"
            );
        }

        // Non-secret fields stay visible so the config is still useful in logs.
        assert!(config_dbg.contains("telemetry-bucket"));
        assert!(config_dbg.contains("us-east-1"));
        assert!(config_dbg.contains("<redacted>"));
    }

    /// A debug-format of `DistributedAttachSql` must never leak the catalog
    /// password, the S3 secret SQL, or the attach SQL (which embeds the catalog).
    #[test]
    fn distributed_attach_sql_debug_redacts_credentials() {
        const PW: &str = "PgP4ssw0rd_SHOULD_NOT_LEAK";
        const S3_SECRET: &str = "S3Secret_SHOULD_NOT_LEAK";
        let attach = DistributedAttachSql {
            catalog_path: format!("dbname=d host=h user=u password={PW}"),
            metadata_schema: "tenant_acme".to_string(),
            data_path: "/data/tenants/acme".to_string(),
            secret_sql: Some(format!("CREATE SECRET (TYPE s3, SECRET '{S3_SECRET}')")),
            s3_scope: Some("s3://bucket/acme/".to_string()),
            attach_sql: format!("ATTACH 'ducklake:dbname=d password={PW}' AS lake"),
        };
        let dbg = format!("{attach:?}");
        assert!(!dbg.contains(PW), "attach debug leaked PG password: {dbg}");
        assert!(
            !dbg.contains(S3_SECRET),
            "attach debug leaked S3 secret: {dbg}"
        );
        // Non-secret fields stay visible.
        assert!(dbg.contains("tenant_acme") && dbg.contains("<redacted>"));
    }
}
