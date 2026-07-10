//! Edge query-serving surface: serves this node's DuckLake data read-only
//! over the DuckDB `quack` client-server protocol, in-process, on a
//! connection cloned from the same in-memory DuckDB instance ingest writes
//! to (a DuckDB-file catalog is one-writer-XOR-many-readers, so a separate
//! process cannot share the lock — see `pool::ProjectPool::clone_connection`).
//!
//! Two independent guards, both required:
//! - [`load_and_pin`]: refuses to serve an unpinned/unexpected quack build.
//! - [`install_authorization_gate`]: the actual security boundary — a
//!   DuckDB-parser-classified, fail-closed, SELECT-only allowlist.

use anyhow::{Context, Result};
use duckdb::Connection;

/// Pinned quack extension build. Asserted on every load (see
/// [`load_and_pin`]) so a silent quack upgrade can never change
/// authorization semantics underneath this gate without a deliberate,
/// re-verified bump here.
///
/// NOTE: the task brief this module was built from assumed quack ships from
/// the `core_nightly` repository at version `29fc039`. Empirically, against
/// the DuckDB build actually vendored by this crate's `duckdb = "1.10503.1"`
/// dependency (DuckDB v1.5.3), a bare `LOAD quack;` succeeds with NO
/// `core_nightly` install step — quack autoinstalls from the default `core`
/// repository, and `duckdb_extensions()` reports `extension_version =
/// '1693647'`, `install_mode = 'REPOSITORY'`, `installed_from = 'core'`.
/// This constant reflects that directly-observed reality, not the brief's
/// assumption; the `core_nightly` path is kept below as a fallback for
/// older/other DuckDB builds where quack isn't in the default repo yet.
pub const EXPECTED_QUACK_VERSION: &str = "1693647";

/// SQL macro name installed as the quack authorization gate. Re-created from
/// this Rust source on every server boot (see [`install_authorization_gate`])
/// — never trusted from whatever a previous run may have left in the catalog.
const AUTH_FUNCTION_NAME: &str = "sigiro_quack_authorize";

/// Install (if needed) and load the `quack` DuckDB extension on `conn`, then
/// assert the loaded build matches [`EXPECTED_QUACK_VERSION`]. Fails loud on
/// a mismatch — refuses to serve rather than trust an unpinned extension.
///
// TODO(phase-B): vendor the .duckdb_extension artifact into sigiro's own
// extension dir (alongside `storage::prewarm_extensions`) instead of relying
// on a network INSTALL/autoload the first time a quack-enabled node boots.
pub fn load_and_pin(conn: &Connection) -> Result<()> {
    if let Err(bare_load_error) = conn.execute_batch("LOAD quack;") {
        conn.execute_batch("INSTALL quack FROM core_nightly; LOAD quack;")
            .with_context(|| {
                format!(
                    "failed to INSTALL/LOAD the quack extension; bare `LOAD quack;` \
                     failed first with: {bare_load_error:#}"
                )
            })?;
    }

    let installed_version: String = conn
        .query_row(
            "SELECT extension_version FROM duckdb_extensions() WHERE extension_name = 'quack'",
            [],
            |row| row.get(0),
        )
        .context(
            "failed to read quack's extension_version from duckdb_extensions() — is quack loaded?",
        )?;

    anyhow::ensure!(
        installed_version == EXPECTED_QUACK_VERSION,
        "quack extension version mismatch: installed {installed_version:?}, pinned {EXPECTED_QUACK_VERSION:?} \
         — refusing to serve the quack query surface with an unpinned/unexpected build. \
         Only bump EXPECTED_QUACK_VERSION after re-verifying the authorization gate against the new build."
    );
    tracing::info!(version = %installed_version, "quack extension loaded and version-pinned");
    Ok(())
}

/// Install the read-only authorization gate and point quack at it.
///
/// Classification uses DuckDB's OWN parser via `json_serialize_sql`, not a
/// naive prefix/keyword match (a prefix gate is bypassable, e.g. leading
/// whitespace/comments, and a denylist can never enumerate every write
/// surface). Empirically verified against the real DuckDB build (see
/// `EXPECTED_QUACK_VERSION`'s doc comment): `json_serialize_sql(sql)` returns
/// `{"error": false, ...}` ONLY when every statement in `sql` parses as a
/// pure SELECT-family statement (plain SELECT, `WITH ... SELECT`, the bare
/// `TABLE t` / `FROM t` shorthands, and multi-statement batches where every
/// statement is one of those). For literally everything else — INSERT,
/// UPDATE, DELETE, CREATE, DROP, ALTER, ATTACH, COPY, CALL, PRAGMA, SET,
/// EXPLAIN, LOAD, or a syntax error — DuckDB itself throws
/// `NotImplementedException("Only SELECT statements can be serialized to
/// json!")`, which `json_serialize_sql` surfaces as `{"error": true, ...}`
/// rather than propagating the exception. The gate allows exactly the
/// `error: false` case.
///
/// Fail-closed by construction: `COALESCE(..., 'true')` treats anything the
/// extraction can't classify (a missing/malformed `error` key) as `error =
/// true`, i.e. denied — there is no default-allow path.
///
/// (Re)created on every server boot from this Rust source — never trusted
/// from a persisted catalog macro a previous run may have left behind, so a
/// stale or tampered macro definition can never survive a restart.
pub fn install_authorization_gate(conn: &Connection) -> Result<()> {
    conn.execute_batch("INSTALL json; LOAD json;")
        .context("failed to load the json extension required by the quack authorization gate")?;

    // Quack's authorization callback runs in a fresh, transient server-side
    // connection per call (per quack's own docs) — NOT the connection this
    // gate is installed from, and NOT one that inherits the `USE lake;`
    // default-catalog setting `pool::clone_connection` sets on `conn`.
    // Qualifying the macro as `memory.main.<name>` (DuckDB's always-present
    // built-in catalog/schema, distinct from the `lake` DuckLake catalog)
    // guarantees it resolves from that fresh connection regardless of
    // ambient `USE`/search_path state — confirmed necessary: creating the
    // macro unqualified while `USE lake;` was active left every real
    // PREPARE_REQUEST authorization check unable to resolve the function,
    // which quack reports uniformly as "Authorization failed" (indistinguishable
    // from a real denial without server-side duckdb_logs() inspection).
    conn.execute_batch(&format!(
        "CREATE OR REPLACE MACRO memory.main.{AUTH_FUNCTION_NAME}(connection_id, query) AS \
         COALESCE(json_extract_string(json_serialize_sql(query::VARCHAR), '$.error'), 'true') = 'false';"
    ))
    .context("failed to create the quack authorization macro")?;

    conn.execute_batch(&format!(
        "SET GLOBAL quack_authorization_function = 'memory.main.{AUTH_FUNCTION_NAME}';"
    ))
    .context("failed to install quack_authorization_function")?;

    tracing::info!(
        function = AUTH_FUNCTION_NAME,
        "quack authorization gate installed — SELECT-only, DuckDB-parser-classified, fail-closed"
    );
    Ok(())
}

/// Result of starting the quack server: what it reports listening on.
pub struct QuackListen {
    pub listen_uri: String,
    pub listen_url: String,
}

/// Start the quack server on `conn`, bound to `bind_addr:port`. `conn` must
/// stay alive for as long as the server should keep listening — quack's
/// server lifetime is tied to the DuckDB session/connection, not to this one
/// SQL call (`quack_serve` streams back its listen info immediately; the
/// listener itself is owned by the extension state on `conn`).
///
/// `disable_ssl` is always passed: quack has no working TLS of its own (see
/// module docs) — a network perimeter (e.g. Tailscale) is the operator's
/// responsibility. `allow_other_hostname` is always passed too: the operator
/// already made an explicit, non-default choice by setting
/// `SIGIRO_QUACK_BIND` to anything other than loopback (never `0.0.0.0`,
/// enforced by the caller), so there is no additional guard this flag would
/// add beyond what the bind-address check already provides.
pub fn start_server(
    conn: &Connection,
    bind_addr: &str,
    port: u16,
    token: Option<&str>,
) -> Result<QuackListen> {
    if let Some(t) = token {
        anyhow::ensure!(
            t.len() >= 4,
            "SIGIRO_QUACK_TOKEN must be at least 4 characters (quack's own minimum)"
        );
    }

    let uri = format!("quack://{bind_addr}:{port}");
    let mut sql = format!(
        "SELECT listen_uri, listen_url FROM quack_serve('{}'",
        crate::storage::escape_sql_string(&uri)
    );
    if let Some(t) = token {
        sql.push_str(&format!(
            ", token := '{}'",
            crate::storage::escape_sql_string(t)
        ));
    }
    sql.push_str(", allow_other_hostname := true, disable_ssl := true);");

    let (listen_uri, listen_url) = conn
        .query_row(&sql, [], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .context("failed to start quack_serve")?;

    Ok(QuackListen {
        listen_uri,
        listen_url,
    })
}

/// Stop the quack server started by [`start_server`] for the same
/// `bind_addr:port`. Called before dropping `conn` on shutdown — dropping a
/// live quack-served connection without stopping the server first aborts
/// the process (observed directly: `libc++abi: terminating`).
pub fn stop_server(conn: &Connection, bind_addr: &str, port: u16) -> Result<()> {
    let uri = format!("quack://{bind_addr}:{port}");
    conn.execute_batch(&format!(
        "CALL quack_stop('{}');",
        crate::storage::escape_sql_string(&uri)
    ))
    .context("failed to stop the quack server cleanly")?;
    Ok(())
}
