use anyhow::{Context, Result};
use prefixed_api_key::{PrefixedApiKey, PrefixedApiKeyController};
use rusqlite::Connection;
use std::sync::Mutex;

/// Build prefixed-api-key controller with sigiro's key format.
/// Keys look like: sigiro_BRTRKFsL_51FwqftsmMDHHbJAMEXXHCgG
pub fn key_controller() -> prefixed_api_key::PakControllerOsSha256 {
    PrefixedApiKeyController::configure()
        .prefix("sigiro".to_string())
        .seam_defaults() // OsRng + SHA-256 + default lengths (8 short, 24 long)
        .finalize()
        .expect("valid controller config")
}

/// Create api_keys table in SQLite admin DB.
pub fn create_keys_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sigiro_api_keys (
            short_token TEXT PRIMARY KEY,
            long_token_hash TEXT NOT NULL,
            project_id TEXT NOT NULL,
            plan TEXT NOT NULL DEFAULT 'free',
            rate_limit INTEGER NOT NULL DEFAULT 100,
            created_at TEXT DEFAULT (datetime('now'))
        );",
    )
    .context("failed to create api_keys table")?;
    Ok(())
}

/// Generate a new API key, store hash in admin DB, return raw key string.
/// The raw key is shown once to user and never stored.
pub fn create_key(
    conn: &Mutex<Connection>,
    project_id: &str,
    plan: &str,
    rate_limit: u32,
) -> Result<String> {
    let controller = key_controller();
    let (pak, hash) = controller
        .try_generate_key_and_hash()
        .map_err(|e| anyhow::anyhow!("key generation failed: {e}"))?;

    let short_token = pak.short_token().to_string();
    let hash_str = hash.to_string();
    let key_string = pak.to_string();

    let guard = conn
        .lock()
        .map_err(|e| anyhow::anyhow!("admin DB mutex poisoned: {e}"))?;
    guard.execute(
        "INSERT INTO sigiro_api_keys (short_token, long_token_hash, project_id, plan, rate_limit)
         VALUES (?, ?, ?, ?, ?)",
        rusqlite::params![short_token, hash_str, project_id, plan, rate_limit as i32],
    )
    .context("failed to insert API key")?;

    Ok(key_string)
}

/// Revoke an API key by short token.
pub fn revoke_key(conn: &Mutex<Connection>, short_token: &str) -> Result<bool> {
    let guard = conn
        .lock()
        .map_err(|e| anyhow::anyhow!("admin DB mutex poisoned: {e}"))?;
    let affected = guard
        .execute(
            "DELETE FROM sigiro_api_keys WHERE short_token = ?",
            [short_token],
        )
        .context("failed to revoke API key")?;
    Ok(affected > 0)
}

/// Open or create the admin SQLite database (for API key storage).
pub fn open_admin_db() -> Result<Connection> {
    let dir = crate::data_dir()?;
    std::fs::create_dir_all(&dir).context("failed to create data directory")?;
    let path = dir.join("admin.sqlite");
    let conn = Connection::open(&path)
        .with_context(|| format!("failed to open admin DB at {}", path.display()))?;

    // Enable WAL mode for better concurrency
    conn.execute("PRAGMA journal_mode=WAL", [])
        .context("failed to enable WAL mode")?;

    create_keys_table(&conn)?;
    Ok(conn)
}

/// Verify an API key against admin DB and return (project_id, plan).
/// Returns Result to allow auth.rs to convert to ProblemDetails.
pub fn verify_key_db(conn: &Mutex<Connection>, raw_key: &str) -> Result<(String, String)> {
    let pak =
        PrefixedApiKey::from_string(raw_key).map_err(|_| anyhow::anyhow!("Malformed API key"))?;

    let short_token = pak.short_token().to_string();
    let controller = key_controller();

    let guard = conn
        .lock()
        .map_err(|e| anyhow::anyhow!("admin DB mutex poisoned: {e}"))?;

    let mut stmt = guard
        .prepare(
            "SELECT long_token_hash, project_id, plan FROM sigiro_api_keys WHERE short_token = ?",
        )
        .context("failed to prepare statement")?;

    let result = stmt.query_row([&short_token], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    });

    match result {
        Ok((stored_hash, project_id, plan)) => {
            if controller.check_hash(&pak, &stored_hash) {
                Ok((project_id, plan))
            } else {
                Err(anyhow::anyhow!("Invalid API key"))
            }
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(anyhow::anyhow!("Invalid API key")),
        Err(e) => Err(anyhow::anyhow!("Auth database error: {e}")),
    }
}
