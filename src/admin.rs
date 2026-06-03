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

/// Open or create the admin SQLite database (for instance status and federation).
pub fn open_admin_db() -> Result<Connection> {
    let dir = crate::data_dir()?;
    std::fs::create_dir_all(&dir).context("failed to create data directory")?;
    let path = dir.join("admin.sqlite");
    let conn =
        Connection::open(&path).with_context(|| format!("failed to open admin DB at {path}"))?;

    conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0))
        .context("failed to enable WAL mode")?;

    create_instance_status_table(&conn)?;
    Ok(conn)
}
