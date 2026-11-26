use crate::types::TenantId;
use anyhow::{Context, Result};
use camino::Utf8Path;
use duckdb::Connection;
use nutype::nutype;
use secrecy::{ExposeSecret, SecretString};
use sigiro_core::sql::escape_sql_string;
use tracing::instrument;
use url::{Position, Url};

const EXPECTED_DUCKDB_VERSION: &str = "v2.0.1";

/// Every DuckLake attach sets AUTOMATIC_MIGRATION, so bumping this pin migrates
/// existing catalogs forward on first attach (v2.0.0 moved `1.1-dev1` to `1.1`)
/// and the previous binary can no longer read them. Back up hosted catalogs
/// before deploying a bump.
const EXPECTED_DUCKLAKE_VERSION: &str = "08089b16dc";

/// The gatekeeper build `/v1/query` is held to (`crate::query_policy`), checked
/// when a project opens. A bump can change which functions a query may call:
/// review gatekeeper's default function list before bumping.
pub(crate) const EXPECTED_GATEKEEPER_VERSION: &str = "v0.4.5";

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

    // Local mode needs only ducklake, and gatekeeper for /v1/query
    // (`crate::query_policy`). Every extension is statically linked,
    // so loading it reads nothing from disk or the network.
    load_extensions(conn, &["ducklake", "gatekeeper"])?;
    verify_duckdb_stack(conn)?;

    // Attach DuckLake with DuckDB file catalog + local Parquet.
    // Uses catalog.ducklake (DuckDB native catalog) instead of SQLite
    // because the appender API is tested and verified to work with
    // DuckDB file catalogs (see tests/ducklake_appender_test.rs).
    let attach_sql = format!(
        "ATTACH 'ducklake:{project_dir_str}/catalog.ducklake' AS lake (AUTOMATIC_MIGRATION true)"
    );
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
    message: String,
}

impl AttachError {
    /// Categories whose cause is transient and clears without operator action.
    /// Everything else is permanent — fail fast rather than retry.
    pub(crate) fn is_retryable(category: &str) -> bool {
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
    if config.uses_s3() {
        load_extensions(conn, &["ducklake", "gatekeeper", "postgres", "httpfs"])?;
    } else {
        load_extensions(conn, &["ducklake", "gatekeeper", "postgres"])?;
    }
    verify_duckdb_stack(conn)?;

    let tenant_id = TenantId::try_new(tenant_id.to_owned())
        .map_err(|e| anyhow::anyhow!("invalid tenant id: {e}"))?;
    let attach = distributed_attach_sql(config, project_dir, tenant_id.as_ref())?;

    if let Some(secret_sql) = &attach.secret_sql {
        // Generic message: the DuckDB error could echo the CREATE SECRET SQL
        // (with the S3 access key/secret) — never surface it.
        conn.execute_batch(secret_sql).map_err(|_| {
            anyhow::anyhow!(
                "failed to configure S3 secret — verify SIGIRO_S3_* credentials and endpoint"
            )
        })?;
        if let DataLocation::S3(s3) = &config.data {
            tracing::info!(
                region = %s3.region,
                bucket = %s3.bucket,
                scope = ?attach.s3_scope,
                "S3 credentials configured"
            );
        }
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
            message: format!(
                "failed to attach DuckLake PostgreSQL catalog for tenant {tenant_id} \
                 (metadata schema {}, category {category}) — {hint}",
                attach.metadata_schema,
            ),
        })
    })?;
    // NEVER log the connection string — it embeds the PostgreSQL password.
    // tenant_id + metadata_schema + data_path fully identify the attach.
    if config.uses_s3() {
        tracing::info!(
            tenant_id = %tenant_id,
            metadata_schema = %attach.metadata_schema,
            data_path = %attach.data_path,
            "DuckLake attached — catalog: PostgreSQL, data: S3"
        );
    } else {
        tracing::info!(
            tenant_id = %tenant_id,
            metadata_schema = %attach.metadata_schema,
            data_path = %attach.data_path,
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
    catalog_url: SecretString,
    data: DataLocation,
}

impl StorageConfig {
    pub fn new(catalog_url: String, data: DataLocation) -> Result<Self> {
        anyhow::ensure!(
            !catalog_url.trim().is_empty(),
            "SIGIRO_CATALOG_URL must not be empty"
        );
        Ok(Self {
            catalog_url: catalog_url.into(),
            data,
        })
    }

    pub(crate) fn uses_s3(&self) -> bool {
        matches!(self.data, DataLocation::S3(_))
    }

    pub fn catalog_url(&self) -> &str {
        self.catalog_url.expose_secret()
    }
}

#[derive(Debug)]
pub enum DataLocation {
    LocalFilesystem,
    S3(S3Config),
}

#[nutype(
    sanitize(trim),
    validate(not_empty, regex = "^[A-Za-z0-9][A-Za-z0-9._-]*$"),
    derive(Debug, Clone, PartialEq, Eq, AsRef, Display)
)]
pub struct S3BucketName(String);

#[nutype(
    sanitize(trim),
    validate(not_empty),
    derive(Debug, Clone, PartialEq, Eq, AsRef, Display)
)]
pub struct S3Region(String);

#[derive(Debug)]
pub struct S3Config {
    bucket: S3BucketName,
    region: S3Region,
    endpoint: Url,
    key_id: SecretString,
    secret: SecretString,
}

impl S3Config {
    pub fn new(
        bucket: S3BucketName,
        region: S3Region,
        endpoint: Url,
        key_id: String,
        secret: String,
    ) -> Result<Self> {
        anyhow::ensure!(
            matches!(endpoint.scheme(), "http" | "https")
                && endpoint.host().is_some()
                && endpoint.username().is_empty()
                && endpoint.password().is_none()
                && endpoint.path() == "/"
                && endpoint.query().is_none()
                && endpoint.fragment().is_none(),
            "SIGIRO_S3_ENDPOINT must be an HTTP(S) origin URL without credentials, path, query, or fragment"
        );
        anyhow::ensure!(
            !key_id.is_empty(),
            "SIGIRO_S3_BUCKET is set but no S3 key ID is configured — set SIGIRO_S3_KEY_ID or AWS_ACCESS_KEY_ID"
        );
        anyhow::ensure!(
            !secret.is_empty(),
            "SIGIRO_S3_BUCKET is set but no S3 secret is configured — set SIGIRO_S3_SECRET or AWS_SECRET_ACCESS_KEY"
        );
        Ok(Self {
            bucket,
            region,
            endpoint,
            key_id: key_id.into(),
            secret: secret.into(),
        })
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
            None => Ok(Self::LocalFile),
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
        matches!(self, Self::Distributed(config) if config.uses_s3())
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
pub fn sanitize_tenant_id(tenant_id: &str) -> Result<TenantId> {
    TenantId::try_new(tenant_id.to_owned()).map_err(|e| anyhow::anyhow!("invalid tenant id: {e}"))
}

pub fn tenant_metadata_schema(tenant_id: &str) -> Result<String> {
    let tenant_id = sanitize_tenant_id(tenant_id)?;
    Ok(format!("tenant_{}", tenant_id.as_ref()))
}

/// Per-tenant data directory in distributed mode: `<data_dir>/tenants/<tenant_id>`.
/// Nesting under `tenants/` (rather than `<data_dir>/<tenant_id>` directly) keeps
/// per-tenant dirs from ever colliding with sibling top-level dirs (the default
/// project's `default/`, the shared `extensions/` dir). Shared by `ProjectPool`
/// (the actual per-tenant connection); catalog maintenance must use the same
/// tenant layout.
pub fn tenant_data_dir(data_dir: &Utf8Path, tenant_id: &str) -> Result<camino::Utf8PathBuf> {
    let tenant_id = sanitize_tenant_id(tenant_id)?;
    Ok(data_dir.join("tenants").join(tenant_id.as_ref()))
}

pub fn distributed_attach_sql(
    config: &StorageConfig,
    project_dir: &Utf8Path,
    tenant_id: &str,
) -> Result<DistributedAttachSql> {
    let tenant_id = TenantId::try_new(tenant_id.to_owned())
        .map_err(|e| anyhow::anyhow!("invalid tenant id: {e}"))?;
    let metadata_schema = tenant_metadata_schema(tenant_id.as_ref())?;
    let catalog_path = postgres_uri_to_libpq(config.catalog_url())?;
    let catalog_path_sql = escape_sql_string(&catalog_path);
    let metadata_schema_sql = escape_sql_string(&metadata_schema);

    let (data_path, secret_sql, s3_scope) = match &config.data {
        DataLocation::LocalFilesystem => {
            std::fs::create_dir_all(project_dir).with_context(|| {
                format!("failed to create project directory for local data: {project_dir}",)
            })?;
            (project_dir.as_str().to_string(), None, None)
        }
        DataLocation::S3(s3) => {
            let data_path = format!("s3://{}/{}/", s3.bucket, tenant_id.as_ref());
            let scope = data_path.clone();
            let endpoint = &s3.endpoint[Position::BeforeHost..Position::AfterPort];
            let use_ssl = s3.endpoint.scheme() == "https";
            let secret_sql = format!(
                "CREATE OR REPLACE SECRET s3_sigiro (TYPE S3, KEY_ID '{}', SECRET '{}', REGION '{}', \
                 ENDPOINT '{}', URL_STYLE 'path', USE_SSL {}, SCOPE '{}');",
                escape_sql_string(s3.key_id.expose_secret()),
                escape_sql_string(s3.secret.expose_secret()),
                escape_sql_string(s3.region.as_ref()),
                escape_sql_string(endpoint),
                use_ssl,
                escape_sql_string(&scope)
            );
            (data_path, Some(secret_sql), Some(scope))
        }
    };

    let attach_sql = format!(
        "ATTACH 'ducklake:postgres:{catalog_path_sql}' AS lake \
         (METADATA_SCHEMA '{metadata_schema_sql}', DATA_PATH '{}', CREATE_IF_NOT_EXISTS true, \
         AUTOMATIC_MIGRATION true)",
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
    use crate::config;

    let s3_bucket = config::S3_BUCKET.value().map_err(anyhow::Error::from)?;
    let s3_region = config::S3_REGION.value().map_err(anyhow::Error::from)?;
    let s3_endpoint = config::S3_ENDPOINT.value().map_err(anyhow::Error::from)?;
    let s3_key_id = config::S3_KEY_ID.value().map_err(anyhow::Error::from)?;
    let s3_secret = config::S3_SECRET.value().map_err(anyhow::Error::from)?;
    let configured_s3_fields = [
        ("SIGIRO_S3_BUCKET", &s3_bucket),
        ("SIGIRO_S3_REGION", &s3_region),
        ("SIGIRO_S3_ENDPOINT", &s3_endpoint),
        ("SIGIRO_S3_KEY_ID", &s3_key_id),
        ("SIGIRO_S3_SECRET", &s3_secret),
    ]
    .into_iter()
    .filter_map(|(name, value)| (!value.is_empty()).then_some(name))
    .collect::<Vec<_>>();

    let catalog_url = match crate::config::CATALOG_URL.value() {
        Ok(v) if !v.is_empty() => v,
        Ok(_) => anyhow::bail!(
            "SIGIRO_CATALOG_URL is set but empty — provide a valid PostgreSQL catalog URL or unset it"
        ),
        Err(_) => {
            anyhow::ensure!(
                configured_s3_fields.is_empty(),
                "{} configured without SIGIRO_CATALOG_URL",
                configured_s3_fields.join(", ")
            );
            return Ok(None);
        }
    };
    let data = if s3_bucket.is_empty() {
        anyhow::ensure!(
            configured_s3_fields.is_empty(),
            "{} configured without SIGIRO_S3_BUCKET",
            configured_s3_fields.join(", ")
        );
        DataLocation::LocalFilesystem
    } else {
        let region = S3Region::try_new(s3_region)
        .map_err(|_| anyhow::anyhow!(
            "SIGIRO_S3_BUCKET is set but SIGIRO_S3_REGION is missing — provide a valid AWS region"
        ))?;
        let endpoint = Url::parse(&s3_endpoint)
            .with_context(|| "SIGIRO_S3_ENDPOINT must be an absolute http:// or https:// URL")?;
        let key_id = resolve_with_aws_fallback(s3_key_id, &config::AWS_ACCESS_KEY_ID)?;
        let secret = resolve_with_aws_fallback(s3_secret, &config::AWS_SECRET_ACCESS_KEY)?;
        let bucket = S3BucketName::try_new(s3_bucket)
            .map_err(|e| anyhow::anyhow!("invalid SIGIRO_S3_BUCKET: {e}"))?;
        DataLocation::S3(S3Config::new(bucket, region, endpoint, key_id, secret)?)
    };

    validate_hosted_catalog_tls(&catalog_url)?;
    Ok(Some(StorageConfig::new(catalog_url, data)?))
}

/// Read `primary`, falling back to `secondary` (the conventional AWS_* name)
/// when `primary` is unset/empty. Used so SIGIRO_S3_KEY_ID/SIGIRO_S3_SECRET
/// can be overridden explicitly while still picking up plain AWS_ACCESS_KEY_ID/
/// AWS_SECRET_ACCESS_KEY from the environment by default.
fn resolve_with_aws_fallback(
    primary: String,
    secondary: &typed_env::Envar<String>,
) -> Result<String> {
    if primary.is_empty() {
        secondary.value().map_err(anyhow::Error::from)
    } else {
        Ok(primary)
    }
}

fn validate_hosted_catalog_tls(uri: &str) -> Result<()> {
    let parsed = Url::parse(uri).context("SIGIRO_CATALOG_URL must be a PostgreSQL URL")?;
    anyhow::ensure!(
        matches!(parsed.scheme(), "postgres" | "postgresql"),
        "SIGIRO_CATALOG_URL must use postgres:// or postgresql://"
    );
    let mut sslmode = None;
    let mut sslrootcert = None;
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "sslmode" => sslmode = Some(value.into_owned()),
            "sslrootcert" => sslrootcert = Some(value.into_owned()),
            _ => {}
        }
    }
    anyhow::ensure!(
        sslmode.as_deref() == Some("verify-full"),
        "hosted PostgreSQL requires sslmode=verify-full"
    );
    anyhow::ensure!(
        sslrootcert.as_deref().is_some_and(|path| !path.is_empty()),
        "hosted PostgreSQL requires sslrootcert"
    );
    Ok(())
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

            let mut sslmode = None;
            let mut sslrootcert = None;
            for (key, value) in parsed.query_pairs() {
                match key.as_ref() {
                    "sslmode" => sslmode = Some(value.into_owned()),
                    "sslrootcert" => sslrootcert = Some(value.into_owned()),
                    _ => {}
                }
            }
            let tls = match (sslmode, sslrootcert) {
                (Some(mode), Some(root)) => format!(" sslmode={mode} sslrootcert={root}"),
                _ => String::new(),
            };

            Ok(format!(
                "dbname={dbname} host={host} user={user} password={password} port={port}{tls}"
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

/// The on-disk format this build reads and writes (#100).
///
/// Before 1.0, stored telemetry is disposable across incompatible format
/// changes: there are no migrations, compatibility columns or dual writes.
/// Bump this in any release that changes what `schema::create_tables` creates
/// in a way an existing catalog cannot satisfy. A catalog with another
/// version, or with telemetry but no version, is refused before sigiro
/// accepts traffic.
pub const STORAGE_FORMAT_VERSION: i32 = 2;

/// One-row table holding the catalog's storage-format version. Its name does
/// not start with `sigiro_`, so the query surface cannot read it.
const STORAGE_FORMAT_TABLE: &str = "lake.storage_format";

/// A catalog this build must not open.
#[derive(Debug, thiserror::Error)]
#[error(
    "{dir} holds telemetry in {found} storage format, but this sigiro reads and writes format \
     {STORAGE_FORMAT_VERSION}. sigiro does not migrate stored telemetry before 1.0. Stop \
     sigiro, move {dir} aside (keep it as the backup, or delete it), and start sigiro again: it \
     creates a new, empty catalog there. To keep the old data readable, run the sigiro release \
     that wrote it against the moved directory."
)]
pub struct StorageFormatError {
    dir: String,
    found: String,
}

/// Whether the attached catalog needs its format version written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StorageFormat {
    /// New, or created by an older build but holding no rows: stamp it.
    Fresh,
    /// Already stamped with [`STORAGE_FORMAT_VERSION`].
    Current,
}

/// Check the attached `lake` catalog's storage format before any table is
/// created. A catalog with sigiro tables but no version is fresh only while
/// every table is empty; that is how an empty legacy directory is told apart
/// from one holding pre-versioned telemetry.
pub(crate) fn check_storage_format(
    conn: &Connection,
    project_dir: &Utf8Path,
) -> Result<StorageFormat> {
    let tables: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT table_name FROM duckdb_tables() WHERE database_name = 'lake' \
             AND schema_name = 'main'",
        )?;
        stmt.query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<_, _>>()?
    };
    let refuse = |found: String| StorageFormatError {
        dir: project_dir.to_string(),
        found,
    };
    if tables.iter().any(|t| t == "storage_format") {
        let version: Option<i32> = conn
            .query_row(
                &format!("SELECT max(version) FROM {STORAGE_FORMAT_TABLE}"),
                [],
                |row| row.get(0),
            )
            .context("failed to read the catalog's storage format")?;
        return match version {
            Some(STORAGE_FORMAT_VERSION) => Ok(StorageFormat::Current),
            Some(other) => Err(refuse(format!("format {other}")).into()),
            None => Err(refuse("an unreadable".to_string()).into()),
        };
    }
    for table in tables.iter().filter(|t| t.starts_with("sigiro_")) {
        let rows: i64 = conn
            .query_row(
                &format!("SELECT count(*) FROM lake.main.{table}"),
                [],
                |row| row.get(0),
            )
            .with_context(|| format!("failed to count rows in {table}"))?;
        if rows > 0 {
            return Err(refuse("an unversioned (pre-format-1)".to_string()).into());
        }
    }
    Ok(StorageFormat::Fresh)
}

/// Record [`STORAGE_FORMAT_VERSION`] in a fresh catalog.
pub(crate) fn stamp_storage_format(conn: &Connection) -> Result<()> {
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {STORAGE_FORMAT_TABLE} (version INTEGER NOT NULL); \
         DELETE FROM {STORAGE_FORMAT_TABLE}; \
         INSERT INTO {STORAGE_FORMAT_TABLE} VALUES ({STORAGE_FORMAT_VERSION});"
    ))
    .context("failed to record the catalog's storage format")
}

pub fn table_prefix() -> &'static str {
    TABLE_PREFIX
}

/// Load DuckDB extensions. Every extension sigiro uses is compiled into the
/// binary and registered by `crate::pool::open_duckdb` (duckdb-static),
/// so this never downloads anything or reads an extension file.
pub(crate) fn load_extensions(conn: &Connection, names: &[&str]) -> Result<()> {
    for name in names {
        conn.execute_batch(&format!("LOAD {name};"))
            .with_context(|| format!("failed to load DuckDB extension {name}"))?;
    }
    Ok(())
}

fn verify_duckdb_stack(conn: &Connection) -> Result<()> {
    let (duckdb_version, ducklake_version): (String, String) = conn
        .query_row(
            "SELECT version(), extension_version FROM duckdb_extensions() \
             WHERE extension_name = 'ducklake'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .context("failed to read DuckDB/DuckLake versions")?;
    anyhow::ensure!(
        duckdb_version == EXPECTED_DUCKDB_VERSION,
        "DuckDB version mismatch: linked {duckdb_version:?}, expected {EXPECTED_DUCKDB_VERSION:?}"
    );

    anyhow::ensure!(
        ducklake_version == EXPECTED_DUCKLAKE_VERSION,
        "DuckLake version mismatch: loaded {ducklake_version:?}, expected \
         {EXPECTED_DUCKLAKE_VERSION:?}"
    );

    // Read here, before the filesystem lockdown: listing `duckdb_extensions()`
    // afterwards fails, because it reads the extension directory.
    let gatekeeper_version: String = conn
        .query_row(
            "SELECT extension_version FROM duckdb_extensions() \
             WHERE extension_name = 'gatekeeper' AND loaded",
            [],
            |row| row.get(0),
        )
        .context("failed to read the gatekeeper version")?;
    anyhow::ensure!(
        gatekeeper_version == EXPECTED_GATEKEEPER_VERSION,
        "gatekeeper version mismatch: loaded {gatekeeper_version:?}, expected \
         {EXPECTED_GATEKEEPER_VERSION:?}"
    );
    Ok(())
}

/// The extensions [`crate::pool::Pool::lock_filesystem`] `LOAD`s immediately
/// before it turns `enable_external_access` off for good.
///
/// This list and that `LOAD` batch must agree: anything named there but not
/// satisfiable at that moment fails the connection, and because the lockdown is
/// irreversible and instance-global there is no recovery.
pub(crate) const LOCKDOWN_PRELOADED_EXTENSIONS: &[&str] = &["parquet", "json", "icu"];

#[cfg(test)]
mod storage_format_tests {
    use super::*;
    use crate::pool::ProjectPool;

    fn pool(dir: &tempfile::TempDir) -> ProjectPool {
        let data = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        ProjectPool::new(data, 4, StorageMode::LocalFile)
    }

    /// Run `sql` on project `p` with its writer, in a pool that is then closed.
    fn with_writer(dir: &tempfile::TempDir, sql: &str) {
        let pool = pool(dir);
        let writer = pool.create_project_writer("p").unwrap();
        writer.execute_batch(sql).unwrap();
    }

    fn open_error(dir: &tempfile::TempDir) -> Option<String> {
        let error = pool(dir).create_project("p").err()?;
        assert!(
            error.downcast_ref::<StorageFormatError>().is_some(),
            "not a format error: {error:#}"
        );
        Some(format!("{error:#}"))
    }

    #[test]
    fn a_new_catalog_is_stamped_and_reopens() {
        let dir = tempfile::tempdir().unwrap();
        with_writer(&dir, "SELECT 1");
        let pool = pool(&dir);
        let writer = pool.create_project_writer("p").unwrap();
        let version: i32 = writer
            .query_row("SELECT version FROM lake.storage_format", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, STORAGE_FORMAT_VERSION);
    }

    #[test]
    fn another_format_is_refused_with_reset_instructions() {
        let dir = tempfile::tempdir().unwrap();
        with_writer(&dir, "UPDATE lake.storage_format SET version = 99");
        let error = open_error(&dir).expect("format 99 is refused");
        assert!(error.contains("format 99"), "{error}");
        assert!(error.contains("move"), "{error}");
        assert!(error.contains(dir.path().to_str().unwrap()), "{error}");
    }

    #[test]
    fn unversioned_telemetry_is_refused_but_an_unversioned_empty_catalog_is_adopted() {
        let dir = tempfile::tempdir().unwrap();
        with_writer(&dir, "DROP TABLE lake.storage_format");
        assert_eq!(open_error(&dir), None, "empty legacy catalog is adopted");

        with_writer(
            &dir,
            "DROP TABLE lake.storage_format; \
             INSERT INTO lake.sigiro_logs (timestamp, observed_timestamp, service_name, severity_number, severity_text, body) \
             VALUES (TIMESTAMP '2026-11-26 00:00:00', TIMESTAMP '2026-11-26 00:00:00', 'legacy', 9, 'INFO', 'old row')",
        );
        let error = open_error(&dir).expect("legacy telemetry is refused");
        assert!(error.contains("unversioned"), "{error}");
    }
}
