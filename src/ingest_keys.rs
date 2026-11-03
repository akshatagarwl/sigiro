//! Ingest keys: fixed, revocable credentials that can only send telemetry.
//!
//! An OAuth access token from `sigiro auth token` lasts 15 minutes, which is
//! fine for trying sigiro and wrong for a service that runs for weeks. An
//! ingest key is the credential for that service. It belongs to one workspace
//! (the tenant: sigiro has no smaller "project" inside a workspace), it is sent
//! as an ordinary `Authorization: Bearer <key>` header so zero-code
//! instrumentation works with nothing but `OTEL_EXPORTER_OTLP_HEADERS`, and it
//! authorizes OTLP and Pyroscope ingest only: every read API rejects it.
//!
//! The key is `sigiro_<id>_<secret>` ([`prefixed_api_key`], the format GitHub
//! and Seam use). The id is public and is what `list` and `revoke` name. Only a
//! SHA-256 of the secret is stored, so the secret is shown once, at creation,
//! and cannot be recovered afterwards.
//!
//! Every ingest request has to resolve its key, and the admin database is one
//! synchronous connection behind a mutex. So a resolved key is cached for
//! [`CACHE_TTL`]. Revoking through this server clears the cache entry at once;
//! the TTL only bounds how long another server process could keep accepting a
//! key revoked elsewhere.

use crate::admin::AdminConnection;
use crate::declared_schema::DeclaredTable;
use crate::types::TenantId;
use anyhow::{Context, Result};
use dashmap::DashMap;
use parking_lot::Mutex;
use prefixed_api_key::sha2::Sha256;
use prefixed_api_key::{PrefixedApiKey, PrefixedApiKeyController, rand::rngs::OsRng};
use std::sync::Arc;
use std::time::{Duration, Instant};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// The first part of every ingest key. Distinct from a JWT (`eyJ…`), so the
/// two credentials never need to be told apart by trial verification.
pub const KEY_PREFIX: &str = "sigiro";
/// How long a resolved key is trusted without reading the database again.
pub const CACHE_TTL: Duration = Duration::from_secs(60);
/// Active keys one workspace may hold. A guard against a runaway script, not a
/// plan limit.
pub const MAX_ACTIVE_KEYS: usize = 50;
const MAX_NAME_CHARS: usize = 64;

/// What a client may see about a key. Never carries the secret or its hash.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct IngestKeyInfo {
    /// Public key id: the middle part of `sigiro_<id>_<secret>`.
    pub id: String,
    pub name: String,
    /// RFC 3339.
    pub created_at: String,
    /// Subject of the OAuth token that created the key.
    pub created_by: String,
    /// RFC 3339; updated at most once a minute per key.
    pub last_used_at: Option<String>,
    /// RFC 3339; set once the key is revoked.
    pub revoked_at: Option<String>,
}

/// The one response that carries a secret.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct CreatedIngestKey {
    #[serde(flatten)]
    pub info: IngestKeyInfo,
    /// The full key. Shown once; only its hash is stored.
    pub key: String,
}

/// Why a presented ingest key was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRejection {
    /// Not of the form `sigiro_<id>_<secret>`.
    Malformed,
    /// No key with this id and secret exists.
    Unknown,
    /// The key exists and was revoked.
    Revoked,
    /// The admin database could not be read.
    Unavailable,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateError {
    #[error("name must be 1 to {MAX_NAME_CHARS} printable characters")]
    InvalidName,
    #[error("this workspace already has {MAX_ACTIVE_KEYS} active ingest keys; revoke one first")]
    TooManyKeys,
    #[error(transparent)]
    Storage(#[from] anyhow::Error),
}

#[derive(Clone)]
struct ResolvedKey {
    project_id: String,
    key_hash: String,
    revoked: bool,
    fetched_at: Instant,
}

/// True when `token` is shaped like an ingest key rather than a JWT.
pub fn is_ingest_key(token: &str) -> bool {
    token
        .strip_prefix(KEY_PREFIX)
        .is_some_and(|rest| rest.starts_with('_'))
}

fn controller() -> PrefixedApiKeyController<OsRng, Sha256> {
    PrefixedApiKeyController::configure()
        .prefix(KEY_PREFIX.to_owned())
        .seam_defaults()
        .finalize()
        .expect("seam defaults configure every builder field")
}

fn parse(token: &str) -> Result<PrefixedApiKey, KeyRejection> {
    let key = PrefixedApiKey::from_string(token).map_err(|_| KeyRejection::Malformed)?;
    let base58 = |part: &str| !part.is_empty() && part.len() <= 64 && bs58_alphabet(part);
    if key.prefix() != KEY_PREFIX || !base58(key.short_token()) || !base58(key.long_token()) {
        return Err(KeyRejection::Malformed);
    }
    Ok(key)
}

fn bs58_alphabet(part: &str) -> bool {
    part.bytes()
        .all(|byte| byte.is_ascii_alphanumeric() && !matches!(byte, b'0' | b'O' | b'I' | b'l'))
}

fn valid_name(name: &str) -> bool {
    let count = name.chars().count();
    (1..=MAX_NAME_CHARS).contains(&count) && !name.chars().any(char::is_control)
}

fn now_us() -> i64 {
    (OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000) as i64
}

fn rfc3339(us: i64) -> String {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(us) * 1_000)
        .ok()
        .and_then(|time| time.format(&Rfc3339).ok())
        .unwrap_or_default()
}

#[derive(Clone)]
struct KeyRow {
    id: String,
    name: String,
    created_by: String,
    created_at_us: i64,
    last_used_at_us: Option<i64>,
    revoked_at_us: Option<i64>,
}

impl KeyRow {
    fn info(self) -> IngestKeyInfo {
        IngestKeyInfo {
            id: self.id,
            name: self.name,
            created_at: rfc3339(self.created_at_us),
            created_by: self.created_by,
            last_used_at: self.last_used_at_us.map(rfc3339),
            revoked_at: self.revoked_at_us.map(rfc3339),
        }
    }
}

/// Ingest keys of every workspace, in the admin database.
#[derive(Clone)]
pub struct IngestKeys {
    conn: Arc<Mutex<AdminConnection>>,
    cache: Arc<DashMap<String, ResolvedKey>>,
}

impl std::fmt::Debug for IngestKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IngestKeys")
            .field("cached", &self.cache.len())
            .finish_non_exhaustive()
    }
}

impl IngestKeys {
    pub fn new(conn: Arc<Mutex<AdminConnection>>) -> Self {
        Self {
            conn,
            cache: Arc::new(DashMap::new()),
        }
    }

    async fn blocking<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut AdminConnection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        // The hosted connection is the sync `postgres` client, which must not
        // run on a runtime thread (see `admin::with_admin_blocking`).
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || f(&mut conn.lock()))
            .await
            .context("ingest key task panicked")?
    }

    /// Create a key for `project_id`. The returned secret is never stored.
    pub async fn create(
        &self,
        project_id: &TenantId,
        name: &str,
        created_by: &str,
    ) -> Result<CreatedIngestKey, CreateError> {
        let name = name.trim().to_owned();
        if !valid_name(&name) {
            return Err(CreateError::InvalidName);
        }
        let (key, hash) = controller()
            .try_generate_key_and_hash()
            .map_err(|error| anyhow::anyhow!("no randomness for an ingest key: {error}"))?;
        let row = KeyRow {
            id: key.short_token().to_owned(),
            name,
            created_by: created_by.to_owned(),
            created_at_us: now_us(),
            last_used_at_us: None,
            revoked_at_us: None,
        };
        let project = project_id.as_ref().to_owned();
        let stored = row.clone();
        let inserted = self
            .blocking(move |conn| {
                if count_active(conn, &project)? >= MAX_ACTIVE_KEYS {
                    return Ok(false);
                }
                insert(conn, &project, &stored, &hash)?;
                Ok(true)
            })
            .await?;
        if !inserted {
            return Err(CreateError::TooManyKeys);
        }
        Ok(CreatedIngestKey {
            info: row.info(),
            key: key.to_string(),
        })
    }

    /// Every key of `project_id`, newest first, revoked ones included.
    pub async fn list(&self, project_id: &TenantId) -> Result<Vec<IngestKeyInfo>> {
        let project = project_id.as_ref().to_owned();
        let rows = self.blocking(move |conn| list(conn, &project)).await?;
        Ok(rows.into_iter().map(KeyRow::info).collect())
    }

    /// Revoke key `id` of `project_id`. `false` when the workspace has no such
    /// key; revoking a revoked key is a no-op that still returns `true`.
    pub async fn revoke(&self, project_id: &TenantId, id: &str) -> Result<bool> {
        let project = project_id.as_ref().to_owned();
        let key_id = id.to_owned();
        let found = self
            .blocking(move |conn| revoke(conn, &project, &key_id, now_us()))
            .await?;
        self.cache.remove(id);
        Ok(found)
    }

    /// The workspace a presented key sends to, or why it is refused.
    pub async fn verify(&self, token: &str) -> Result<TenantId, KeyRejection> {
        let key = parse(token)?;
        let id = key.short_token().to_owned();
        let cached = self
            .cache
            .get(&id)
            .filter(|entry| entry.fetched_at.elapsed() < CACHE_TTL)
            .map(|entry| entry.clone());
        let resolved = match cached {
            Some(resolved) => resolved,
            None => {
                let key_id = id.clone();
                let row = self
                    .blocking(move |conn| {
                        let row = lookup(conn, &key_id)?;
                        if let Some((_, _, None)) = &row {
                            touch(conn, &key_id, now_us())?;
                        }
                        Ok(row)
                    })
                    .await
                    .map_err(|error| {
                        tracing::warn!("ingest key lookup failed: {error:#}");
                        KeyRejection::Unavailable
                    })?;
                // Unknown ids are not cached: they would let anyone grow the
                // map without bound. The source rate limiter bounds their cost.
                let Some((project_id, key_hash, revoked_at)) = row else {
                    return Err(KeyRejection::Unknown);
                };
                let resolved = ResolvedKey {
                    project_id,
                    key_hash,
                    revoked: revoked_at.is_some(),
                    fetched_at: Instant::now(),
                };
                self.cache.insert(id, resolved.clone());
                resolved
            }
        };
        // Constant-time comparison of the secret's hash. "Revoked" is told only
        // to a caller who holds the secret, so it reveals nothing to a guesser.
        if !controller().check_hash(&key, &resolved.key_hash) {
            return Err(KeyRejection::Unknown);
        }
        if resolved.revoked {
            return Err(KeyRejection::Revoked);
        }
        TenantId::try_new(resolved.project_id).map_err(|_| KeyRejection::Unknown)
    }
}

fn count_active(conn: &mut AdminConnection, project: &str) -> Result<usize> {
    let count: i64 = match conn {
        AdminConnection::Local(conn) => conn.query_row(
            "SELECT count(*) FROM ingest_keys WHERE project_id = ?1 AND revoked_at_us IS NULL",
            [project],
            |row| row.get(0),
        )?,
        AdminConnection::Hosted(client) => client
            .query_one(
                "SELECT count(*) FROM sigiro_admin.ingest_keys WHERE project_id = $1 AND revoked_at_us IS NULL",
                &[&project],
            )?
            .get(0),
    };
    Ok(usize::try_from(count).unwrap_or(usize::MAX))
}

fn insert(conn: &mut AdminConnection, project: &str, row: &KeyRow, hash: &str) -> Result<()> {
    let KeyRow {
        id,
        name,
        created_by,
        created_at_us,
        ..
    } = row;
    match conn {
        AdminConnection::Local(conn) => {
            conn.execute(
                "INSERT INTO ingest_keys (key_id, project_id, name, key_hash, created_by, created_at_us)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![id, project, name, hash, created_by, created_at_us],
            )?;
        }
        AdminConnection::Hosted(client) => {
            client.execute(
                "INSERT INTO sigiro_admin.ingest_keys (key_id, project_id, name, key_hash, created_by, created_at_us)
                 VALUES ($1, $2, $3, $4, $5, $6)",
                &[&id, &project, &name, &hash, &created_by, &created_at_us],
            )?;
        }
    }
    Ok(())
}

fn list(conn: &mut AdminConnection, project: &str) -> Result<Vec<KeyRow>> {
    const COLUMNS: &str = "key_id, name, created_by, created_at_us, last_used_at_us, revoked_at_us";
    match conn {
        AdminConnection::Local(conn) => {
            let mut statement = conn.prepare(&format!(
                "SELECT {COLUMNS} FROM ingest_keys WHERE project_id = ?1 ORDER BY created_at_us DESC, key_id"
            ))?;
            let rows = statement.query_map([project], |row| {
                Ok(KeyRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    created_by: row.get(2)?,
                    created_at_us: row.get(3)?,
                    last_used_at_us: row.get(4)?,
                    revoked_at_us: row.get(5)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .context("failed to read ingest keys")
        }
        AdminConnection::Hosted(client) => Ok(client
            .query(
                &format!(
                    "SELECT {COLUMNS} FROM sigiro_admin.ingest_keys WHERE project_id = $1 ORDER BY created_at_us DESC, key_id"
                ),
                &[&project],
            )?
            .into_iter()
            .map(|row| KeyRow {
                id: row.get(0),
                name: row.get(1),
                created_by: row.get(2),
                created_at_us: row.get(3),
                last_used_at_us: row.get(4),
                revoked_at_us: row.get(5),
            })
            .collect()),
    }
}

fn revoke(conn: &mut AdminConnection, project: &str, id: &str, now_us: i64) -> Result<bool> {
    let changed = match conn {
        AdminConnection::Local(conn) => conn.execute(
            "UPDATE ingest_keys SET revoked_at_us = COALESCE(revoked_at_us, ?3)
             WHERE project_id = ?1 AND key_id = ?2",
            rusqlite::params![project, id, now_us],
        )? as u64,
        AdminConnection::Hosted(client) => client.execute(
            "UPDATE sigiro_admin.ingest_keys SET revoked_at_us = COALESCE(revoked_at_us, $3)
             WHERE project_id = $1 AND key_id = $2",
            &[&project, &id, &now_us],
        )?,
    };
    Ok(changed > 0)
}

type Lookup = Option<(String, String, Option<i64>)>;

fn lookup(conn: &mut AdminConnection, id: &str) -> Result<Lookup> {
    use rusqlite::OptionalExtension;
    Ok(match conn {
        AdminConnection::Local(conn) => conn
            .query_row(
                "SELECT project_id, key_hash, revoked_at_us FROM ingest_keys WHERE key_id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?,
        AdminConnection::Hosted(client) => client
            .query_opt(
                "SELECT project_id, key_hash, revoked_at_us FROM sigiro_admin.ingest_keys WHERE key_id = $1",
                &[&id],
            )?
            .map(|row| (row.get(0), row.get(1), row.get(2))),
    })
}

fn touch(conn: &mut AdminConnection, id: &str, now_us: i64) -> Result<()> {
    match conn {
        AdminConnection::Local(conn) => {
            conn.execute(
                "UPDATE ingest_keys SET last_used_at_us = ?2 WHERE key_id = ?1",
                rusqlite::params![id, now_us],
            )?;
        }
        AdminConnection::Hosted(client) => {
            client.execute(
                "UPDATE sigiro_admin.ingest_keys SET last_used_at_us = $2 WHERE key_id = $1",
                &[&id, &now_us],
            )?;
        }
    }
    Ok(())
}

pub(crate) const SQLITE_TABLES: &[DeclaredTable] = &[DeclaredTable {
    name: "ingest_keys",
    create: "CREATE TABLE {table} (
        key_id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        name TEXT NOT NULL,
        key_hash TEXT NOT NULL,
        created_by TEXT NOT NULL,
        created_at_us INTEGER NOT NULL,
        last_used_at_us INTEGER,
        revoked_at_us INTEGER)",
    indexes: &["CREATE INDEX ingest_keys_project ON {table} (project_id, created_at_us)"],
}];

pub(crate) const POSTGRES_TABLES: &[DeclaredTable] = &[DeclaredTable {
    name: "ingest_keys",
    create: "CREATE TABLE {table} (
        key_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, name TEXT NOT NULL,
        key_hash TEXT NOT NULL, created_by TEXT NOT NULL, created_at_us BIGINT NOT NULL,
        last_used_at_us BIGINT, revoked_at_us BIGINT)",
    indexes: &["CREATE INDEX ingest_keys_project ON {table} (project_id, created_at_us)"],
}];

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> IngestKeys {
        IngestKeys::new(Arc::new(Mutex::new(
            crate::admin::test_connection().unwrap(),
        )))
    }

    fn tenant(id: &str) -> TenantId {
        TenantId::try_new(id.to_owned()).unwrap()
    }

    #[tokio::test]
    async fn a_created_key_sends_to_its_workspace_until_revoked() {
        let keys = store();
        let acme = tenant("acme");
        let created = keys.create(&acme, "checkout prod", "user-1").await.unwrap();
        assert!(created.key.starts_with("sigiro_"));
        assert!(is_ingest_key(&created.key));
        assert_eq!(created.info.name, "checkout prod");
        assert!(created.key.contains(&created.info.id));

        assert_eq!(keys.verify(&created.key).await, Ok(acme.clone()));

        assert!(keys.revoke(&acme, &created.info.id).await.unwrap());
        assert_eq!(keys.verify(&created.key).await, Err(KeyRejection::Revoked));
        let listed = keys.list(&acme).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].revoked_at.is_some());
        assert!(listed[0].last_used_at.is_some());
    }

    #[tokio::test]
    async fn only_a_hash_of_the_secret_is_stored() {
        let keys = store();
        let created = keys.create(&tenant("acme"), "k", "user-1").await.unwrap();
        let secret = created.key.rsplit('_').next().unwrap().to_owned();
        let stored: Vec<String> = {
            let mut conn = keys.conn.lock();
            let AdminConnection::Local(conn) = &mut *conn else {
                unreachable!()
            };
            let mut statement = conn.prepare("SELECT * FROM ingest_keys").unwrap();
            let width = statement.column_count();
            statement
                .query_map([], |row| {
                    (0..width)
                        .map(|i| {
                            row.get::<_, rusqlite::types::Value>(i)
                                .map(|v| format!("{v:?}"))
                        })
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap()
                .flat_map(Result::unwrap)
                .collect()
        };
        assert!(stored.iter().all(|value| !value.contains(&secret)));
        assert!(stored.iter().all(|value| !value.contains(&created.key)));
    }

    #[tokio::test]
    async fn workspaces_cannot_see_or_revoke_each_others_keys() {
        let keys = store();
        let created = keys.create(&tenant("acme"), "k", "user-1").await.unwrap();
        assert!(keys.list(&tenant("globex")).await.unwrap().is_empty());
        assert!(
            !keys
                .revoke(&tenant("globex"), &created.info.id)
                .await
                .unwrap()
        );
        assert_eq!(keys.verify(&created.key).await, Ok(tenant("acme")));
    }

    #[tokio::test]
    async fn wrong_or_malformed_keys_are_refused() {
        let keys = store();
        let created = keys.create(&tenant("acme"), "k", "user-1").await.unwrap();
        let forged = format!("sigiro_{}_{}", created.info.id, "1".repeat(33));
        assert_eq!(keys.verify(&forged).await, Err(KeyRejection::Unknown));
        assert_eq!(
            keys.verify("sigiro_nope_nope").await,
            Err(KeyRejection::Unknown)
        );
        for malformed in [
            "sigiro_",
            "sigiro_a",
            "other_a_b",
            "sigiro_a_b_c",
            "sigiro_0_1",
        ] {
            assert_eq!(
                keys.verify(malformed).await,
                Err(KeyRejection::Malformed),
                "{malformed}"
            );
        }
        // Revoked is only reported to the holder of the secret.
        keys.revoke(&tenant("acme"), &created.info.id)
            .await
            .unwrap();
        assert_eq!(keys.verify(&forged).await, Err(KeyRejection::Unknown));
    }

    #[tokio::test]
    async fn a_revocation_written_elsewhere_is_seen_after_the_cache_ttl() {
        let keys = store();
        let acme = tenant("acme");
        let created = keys.create(&acme, "k", "user-1").await.unwrap();
        assert_eq!(keys.verify(&created.key).await, Ok(acme.clone()));
        // Another process revokes it: same database, a different cache.
        let other = IngestKeys {
            conn: Arc::clone(&keys.conn),
            cache: Arc::new(DashMap::new()),
        };
        assert!(other.revoke(&acme, &created.info.id).await.unwrap());
        assert_eq!(keys.verify(&created.key).await, Ok(acme.clone()));
        keys.cache.get_mut(&created.info.id).unwrap().fetched_at -= CACHE_TTL;
        assert_eq!(keys.verify(&created.key).await, Err(KeyRejection::Revoked));
    }

    #[tokio::test]
    async fn names_and_counts_are_bounded() {
        let keys = store();
        let acme = tenant("acme");
        for bad in ["", "   ", "a\nb", &"x".repeat(65)] {
            assert!(matches!(
                keys.create(&acme, bad, "u").await,
                Err(CreateError::InvalidName)
            ));
        }
        for i in 0..MAX_ACTIVE_KEYS {
            keys.create(&acme, &format!("k{i}"), "u").await.unwrap();
        }
        assert!(matches!(
            keys.create(&acme, "one more", "u").await,
            Err(CreateError::TooManyKeys)
        ));
        let first = keys.list(&acme).await.unwrap().pop().unwrap();
        keys.revoke(&acme, &first.id).await.unwrap();
        keys.create(&acme, "fits again", "u").await.unwrap();
    }

    #[test]
    fn jwts_are_not_mistaken_for_ingest_keys() {
        assert!(!is_ingest_key("eyJhbGciOiJFZERTQSJ9.e30.sig"));
        assert!(!is_ingest_key("sigirox_a_b"));
        assert!(is_ingest_key("sigiro_a_b"));
    }
}
