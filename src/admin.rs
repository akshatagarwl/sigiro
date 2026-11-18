use crate::declared_schema::{DeclaredTable, PostgresSchema, SqliteSchema, reconcile};
use anyhow::{Context, Result};
use postgres_native_tls::MakeTlsConnector;
use std::str::FromStr;

pub enum AdminConnection {
    Local(rusqlite::Connection),
    Hosted(HostedClient),
}

/// A sync `postgres::Client` that is safe to drop from async code.
///
/// `postgres::Client::drop` closes the connection through `block_on`, so a drop
/// on a Tokio worker aborts the process with "Cannot start a runtime from
/// within a runtime" — the same failure the call sites already guard against,
/// but reached through `drop_glue` where no call-site discipline can see it.
/// This bit production: the boot error that `run_serve` was returning unwound
/// through `Arc<Mutex<AdminConnection>>`, the drop panicked, and the panic
/// replaced the real error, so the actual startup failure never reached the
/// journal.
///
/// Moving the close onto `spawn_blocking` makes the safety structural: every
/// `AdminConnection`, wherever it is dropped, closes on a context-free thread.
/// Outside a runtime the client is dropped inline, which is already correct.
pub struct HostedClient(Option<Box<postgres::Client>>);

impl HostedClient {
    fn new(client: postgres::Client) -> Self {
        Self(Some(Box::new(client)))
    }
}

impl std::ops::Deref for HostedClient {
    type Target = postgres::Client;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("client is taken only in Drop")
    }
}

impl std::ops::DerefMut for HostedClient {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().expect("client is taken only in Drop")
    }
}

impl Drop for HostedClient {
    fn drop(&mut self) {
        let Some(client) = self.0.take() else {
            return;
        };
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                // Dropped on a runtime thread: hand the close to a blocking
                // thread that carries no runtime context.
                handle.spawn_blocking(move || drop(client));
            }
            Err(_) => drop(client),
        }
    }
}

const SQLITE_INSTANCE_STATUS: DeclaredTable = DeclaredTable {
    name: "instance_status",
    create: "CREATE TABLE {table} (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL,
        updated_at TEXT DEFAULT (datetime('now')))",
    indexes: &[],
};

const POSTGRES_INSTANCE_STATUS: DeclaredTable = DeclaredTable {
    name: "instance_status",
    create: "CREATE TABLE {table} (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL,
        updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP)",
    indexes: &[],
};

fn reconcile_sqlite(conn: &mut rusqlite::Connection) -> Result<()> {
    let tables: Vec<DeclaredTable> = std::iter::once(SQLITE_INSTANCE_STATUS)
        .chain(crate::alerts::SQLITE_TABLES.iter().copied())
        .collect();
    let transaction = conn
        .transaction()
        .context("failed to start admin schema reconcile")?;
    reconcile(&mut SqliteSchema(&transaction), &tables)?;
    transaction
        .commit()
        .context("failed to commit admin schema reconcile")
}

fn reconcile_postgres(client: &mut postgres::Client) -> Result<()> {
    let tables: Vec<DeclaredTable> = std::iter::once(POSTGRES_INSTANCE_STATUS)
        .chain(crate::alerts::POSTGRES_TABLES.iter().copied())
        .collect();
    let mut transaction = client
        .transaction()
        .context("failed to start hosted admin schema reconcile")?;
    reconcile(
        &mut PostgresSchema {
            transaction: &mut transaction,
            schema: "sigiro_admin",
        },
        &tables,
    )?;
    transaction
        .commit()
        .context("failed to commit hosted admin schema reconcile")
}

/// Write (upsert) a single key-value pair into instance_status.
pub fn write_instance_status(conn: &mut AdminConnection, key: &str, value: &str) -> Result<()> {
    write_instance_status_batch(conn, &[(key, value)])
}

/// Write (upsert) many key-value pairs into instance_status in ONE transaction.
///
/// Boot writes 13 status keys. As individual autocommit statements that is 13
/// fsyncs (measured 6.9ms median); wrapped in one transaction it is 0.5ms. The
/// larger win is the call graph: each `write_instance_status` expands to a
/// 4-node subtree, so the repeated leaf accounted for ~11% of `run_serve`'s
/// call tree and buried the boot sequence it sits in.
pub fn write_instance_status_batch(
    conn: &mut AdminConnection,
    entries: &[(&str, &str)],
) -> Result<()> {
    match conn {
        AdminConnection::Local(conn) => {
            let tx = conn
                .transaction()
                .context("failed to start instance_status transaction")?;
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO instance_status (key, value, updated_at) VALUES (?1, ?2, datetime('now'))
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                ).context("failed to prepare instance_status upsert")?;
                for (key, value) in entries {
                    stmt.execute(rusqlite::params![key, value])
                        .with_context(|| format!("failed to write instance_status key={key}"))?;
                }
            }
            tx.commit()
                .context("failed to commit instance_status transaction")?;
        }
        AdminConnection::Hosted(client) => {
            let mut tx = client
                .transaction()
                .context("failed to start instance_status transaction")?;
            for (key, value) in entries {
                tx.execute(
                    "INSERT INTO sigiro_admin.instance_status (key, value, updated_at) VALUES ($1, $2, CURRENT_TIMESTAMP)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                    &[key, value],
                ).with_context(|| format!("failed to write instance_status key={key}"))?;
            }
            tx.commit()
                .context("failed to commit instance_status transaction")?;
        }
    }
    Ok(())
}

/// Run a blocking admin operation off the async runtime.
///
/// The `postgres` crate's sync client drives its own runtime via `block_on`,
/// which aborts with "Cannot start a runtime from within a runtime" on any
/// thread that already has a Tokio runtime context. `block_in_place` is not a
/// fix: it keeps the runtime context on the thread. Only `spawn_blocking`
/// provides a context-free thread, so every sync admin call reached from async
/// code must go through here. Ownership moves in and back out because the
/// closure must be `'static`.
pub async fn with_admin_blocking<T, F>(
    mut conn: AdminConnection,
    f: F,
) -> Result<(AdminConnection, T)>
where
    F: FnOnce(&mut AdminConnection) -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let value = f(&mut conn)?;
        Ok((conn, value))
    })
    .await
    .context("admin database task panicked")?
}

/// Read all instance_status rows as a HashMap.
pub fn read_all_status(
    conn: &mut AdminConnection,
) -> Result<std::collections::HashMap<String, String>> {
    let mut map = std::collections::HashMap::new();
    match conn {
        AdminConnection::Local(conn) => {
            let mut stmt = conn
                .prepare("SELECT key, value FROM instance_status")
                .context("failed to prepare instance_status query")?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .context("failed to query instance_status")?;
            for row in rows {
                let (key, value) = row.context("failed to read instance_status row")?;
                map.insert(key, value);
            }
        }
        AdminConnection::Hosted(client) => {
            for row in client
                .query("SELECT key, value FROM sigiro_admin.instance_status", &[])
                .context("failed to query instance_status")?
            {
                map.insert(row.get(0), row.get(1));
            }
        }
    }
    Ok(map)
}

/// Open or create the local admin SQLite database.
///
/// Runs only in local (non-distributed) self-hosted mode; hosted deployments use
/// `open_hosted_admin_db` and the PostgreSQL `sigiro_admin` schema instead. Creates
/// both the instance status table and the local alerts tables.
///
/// Takes the resolved data directory rather than calling `data_dir()` again:
/// the caller has already resolved and created it, and a second resolution is
/// both a redundant filesystem probe and a way for the two answers to disagree.
pub fn open_local_admin_db(dir: &camino::Utf8Path) -> Result<AdminConnection> {
    std::fs::create_dir_all(dir).context("failed to create data directory")?;
    let path = dir.join("admin.sqlite");
    let mut conn = rusqlite::Connection::open(&path)
        .with_context(|| format!("failed to open admin DB at {path}"))?;

    conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0))
        .context("failed to enable WAL mode")?;

    reconcile_sqlite(&mut conn)?;
    Ok(AdminConnection::Local(conn))
}

/// Open the hosted admin database.
///
/// The `postgres` crate's sync client drives its own runtime via `block_on`,
/// which panics with "Cannot start a runtime from within a runtime" when called
/// on a `#[tokio::main]` thread. Hosted boot goes through exactly that path, so
/// the blocking connect is moved onto a dedicated blocking thread.
pub async fn open_hosted_admin_db(catalog_url: &str) -> Result<AdminConnection> {
    let catalog_url = catalog_url.to_owned();
    tokio::task::spawn_blocking(move || open_hosted_admin_db_blocking(&catalog_url))
        .await
        .context("hosted admin DB connect task panicked")?
}

fn open_hosted_admin_db_blocking(catalog_url: &str) -> Result<AdminConnection> {
    let mut client = connect_postgres(catalog_url)?;
    reconcile_postgres(&mut client)?;
    Ok(AdminConnection::Hosted(HostedClient::new(client)))
}

pub fn connect_postgres(catalog_url: &str) -> Result<postgres::Client> {
    let mut url = url::Url::parse(catalog_url).context("catalog URL is invalid")?;
    anyhow::ensure!(
        matches!(url.scheme(), "postgres" | "postgresql"),
        "catalog URL must use postgres:// or postgresql://"
    );
    let mut sslmode = None;
    let mut root_cert = None;
    let retained = url
        .query_pairs()
        .filter_map(|(key, value)| match key.as_ref() {
            "sslmode" => {
                sslmode = Some(value.into_owned());
                None
            }
            "sslrootcert" => {
                root_cert = Some(value.into_owned());
                None
            }
            _ => Some((key.into_owned(), value.into_owned())),
        })
        .collect::<Vec<_>>();
    url.set_query(None);
    if !retained.is_empty() {
        url.query_pairs_mut().extend_pairs(retained);
    }
    anyhow::ensure!(
        sslmode.as_deref() == Some("verify-full"),
        "hosted PostgreSQL requires sslmode=verify-full"
    );
    let root_cert = root_cert.context("hosted PostgreSQL requires sslrootcert")?;
    let certificate = std::fs::read(&root_cert)
        .with_context(|| format!("failed to read PostgreSQL root certificate {root_cert}"))?;
    let certificate = native_tls::Certificate::from_pem(&certificate)
        .context("PostgreSQL root certificate is not valid PEM")?;
    let connector = native_tls::TlsConnector::builder()
        .add_root_certificate(certificate)
        .build()
        .context("failed to configure PostgreSQL TLS")?;
    let mut config = postgres::Config::from_str(url.as_str()).context("catalog URL is invalid")?;
    config.ssl_mode(postgres::config::SslMode::Require);
    config
        .connect(MakeTlsConnector::new(connector))
        .context("failed to connect to hosted PostgreSQL")
}

pub fn test_connection() -> Result<AdminConnection> {
    let mut conn =
        rusqlite::Connection::open_in_memory().context("failed to open in-memory admin DB")?;
    reconcile_sqlite(&mut conn)?;
    Ok(AdminConnection::Local(conn))
}
