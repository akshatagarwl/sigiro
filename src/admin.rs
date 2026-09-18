use anyhow::{Context, Result};
use postgres_native_tls::MakeTlsConnector;
use std::str::FromStr;

pub enum AdminConnection {
    Local(rusqlite::Connection),
    Hosted(Box<postgres::Client>),
}

/// Create instance_status table in admin DB (key-value with timestamps).
pub fn create_instance_status_table(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS instance_status (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at TEXT DEFAULT (datetime('now'))
        );",
    )
    .context("failed to create instance_status table")?;
    Ok(())
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

/// Open or create the admin SQLite database (for instance status).
///
/// Takes the resolved data directory rather than calling `data_dir()` again:
/// the caller has already resolved and created it, and a second resolution is
/// both a redundant filesystem probe and a way for the two answers to disagree.
pub fn open_local_admin_db(dir: &camino::Utf8Path) -> Result<AdminConnection> {
    std::fs::create_dir_all(dir).context("failed to create data directory")?;
    let path = dir.join("admin.sqlite");
    let conn = rusqlite::Connection::open(&path)
        .with_context(|| format!("failed to open admin DB at {path}"))?;

    conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0))
        .context("failed to enable WAL mode")?;

    create_instance_status_table(&conn)?;
    crate::alerts::create_sqlite_tables(&conn)?;
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
    client
        .batch_execute(
            "CREATE SCHEMA IF NOT EXISTS sigiro_admin;
         CREATE TABLE IF NOT EXISTS sigiro_admin.instance_status (
           key TEXT PRIMARY KEY,
           value TEXT NOT NULL,
           updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
         );",
        )
        .context("failed to create hosted instance_status table")?;
    crate::alerts::create_postgres_tables(&mut client)?;
    Ok(AdminConnection::Hosted(Box::new(client)))
}

pub(crate) fn connect_postgres(catalog_url: &str) -> Result<postgres::Client> {
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
    let conn =
        rusqlite::Connection::open_in_memory().context("failed to open in-memory admin DB")?;
    create_instance_status_table(&conn)?;
    crate::alerts::create_sqlite_tables(&conn)?;
    Ok(AdminConnection::Local(conn))
}
