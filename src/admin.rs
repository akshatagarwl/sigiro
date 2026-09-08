use anyhow::{Context, Result};
use rusqlite::Connection;

/// Create instance_status table in admin DB (key-value with timestamps).
pub fn create_instance_status_table(conn: &Connection) -> Result<()> {
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
pub fn write_instance_status(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO instance_status (key, value, updated_at) VALUES (?1, ?2, datetime('now'))
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        rusqlite::params![key, value],
    )
    .with_context(|| format!("failed to write instance_status key={key}"))?;
    Ok(())
}

/// Write (upsert) many key-value pairs into instance_status in ONE transaction.
///
/// Boot writes 13 status keys. As individual autocommit statements that is 13
/// fsyncs (measured 6.9ms median); wrapped in one transaction it is 0.5ms. The
/// larger win is the call graph: each `write_instance_status` expands to a
/// 4-node subtree, so the repeated leaf accounted for ~11% of `run_serve`'s
/// call tree and buried the boot sequence it sits in.
pub fn write_instance_status_batch(conn: &Connection, entries: &[(&str, &str)]) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .context("failed to open instance_status transaction")?;
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO instance_status (key, value, updated_at) VALUES (?1, ?2, datetime('now'))
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            )
            .context("failed to prepare instance_status upsert")?;
        for (key, value) in entries {
            stmt.execute(rusqlite::params![key, value])
                .with_context(|| format!("failed to write instance_status key={key}"))?;
        }
    }
    tx.commit()
        .context("failed to commit instance_status transaction")?;
    Ok(())
}

/// Read all instance_status rows as a HashMap.
pub fn read_all_status(conn: &Connection) -> Result<std::collections::HashMap<String, String>> {
    let mut stmt = conn
        .prepare("SELECT key, value FROM instance_status")
        .context("failed to prepare instance_status query")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .context("failed to query instance_status")?;
    let mut map = std::collections::HashMap::new();
    for row in rows {
        let (k, v) = row.context("failed to read instance_status row")?;
        map.insert(k, v);
    }
    Ok(map)
}

/// Open or create the admin SQLite database (for instance status).
///
/// Takes the resolved data directory rather than calling `data_dir()` again:
/// the caller has already resolved and created it, and a second resolution is
/// both a redundant filesystem probe and a way for the two answers to disagree.
pub fn open_admin_db(dir: &camino::Utf8Path) -> Result<Connection> {
    std::fs::create_dir_all(dir).context("failed to create data directory")?;
    let path = dir.join("admin.sqlite");
    let conn =
        Connection::open(&path).with_context(|| format!("failed to open admin DB at {path}"))?;

    conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0))
        .context("failed to enable WAL mode")?;

    create_instance_status_table(&conn)?;
    crate::alerts::create_tables(&conn)?;
    Ok(conn)
}
