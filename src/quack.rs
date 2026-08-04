//! Edge query-serving surface: serves this node's DuckLake data read-only
//! over the DuckDB `quack` client-server protocol, in-process, on a
//! connection cloned from the same in-memory DuckDB instance ingest writes
//! to (a DuckDB-file catalog is one-writer-XOR-many-readers, so a separate
//! process cannot share the lock — see `pool::ProjectPool::clone_connection`).
//!
//! Two independent guards, both required:
//! - [`load_and_pin`]: refuses to serve an unpinned/unexpected quack build.
//! - [`install_authorization_gate`]: the actual security boundary — a Rust
//!   scalar UDF that runs the SAME [`crate::server::validate_read_only`]
//!   firewall `/v1/query` enforces (SELECT-only + `sigiro_*` table allowlist
//!   + reader-function denylist), fail-closed.

use anyhow::{Context, Result};
use duckdb::Connection;
use duckdb::core::{DataChunkHandle, LogicalTypeHandle, LogicalTypeId};
use duckdb::ffi::duckdb_string_t;
use duckdb::types::DuckString;
use duckdb::vscalar::{ScalarFunctionSignature, VScalar};
use duckdb::vtab::arrow::WritableVector;

/// Pinned quack extension build. Asserted on every load (see
/// [`load_and_pin`]) so a silent quack upgrade can never change
/// authorization semantics underneath this gate without a deliberate,
/// re-verified bump here.
///
/// Empirically, against the DuckDB build actually vendored by this crate's
/// `duckdb = "1.10503.1"` dependency (DuckDB v1.5.3): quack is NOT in
/// DuckDB's autoloadable "known extensions" list, so a bare `LOAD quack;`
/// always fails with "Extension \"quack\" is an existing extension. Install
/// it first" — [`load_and_pin`] must always `INSTALL` it explicitly. The
/// `core` repository (DuckDB's default, i.e. plain `INSTALL quack;` with no
/// `FROM` clause) serves this pinned version. **`core_nightly` serves a
/// DIFFERENT, newer, UNVERIFIED build** (confirmed empirically: version
/// `40de7ba` vs. this pin's `1693647`, for the same DuckDB v1.5.3/platform)
/// — using it as a fallback here was the actual root cause of a boot failure
/// (`quack extension version mismatch: installed "40de7ba", pinned
/// "1693647"`) once `core_nightly`'s build drifted away from what this gate
/// was verified against. Never fall back to `core_nightly`; if `core` stops
/// serving this exact version, that is exactly the drift this pin exists to
/// catch — re-verify the authorization gate against the new build and bump
/// this constant deliberately, rather than widening the install source.
pub(crate) const EXPECTED_QUACK_VERSION: &str = "1693647";

/// Scalar UDF name installed as the quack authorization gate. Re-registered
/// from this Rust source on every server boot (see
/// [`install_authorization_gate`]) — never trusted from whatever a previous
/// run may have left in the catalog.
const AUTH_FUNCTION_NAME: &str = "sigiro_quack_authorize";

/// Install (if needed) and load the `quack` DuckDB extension on `conn`, then
/// assert the loaded build matches [`EXPECTED_QUACK_VERSION`]. Fails loud on
/// a mismatch — refuses to serve rather than trust an unpinned extension.
///
/// Also called by [`crate::storage::prewarm_extensions`] so `sigiro
/// fetch-extensions` and the quack E2E test warm (and version-verify) this
/// exact same pinned build ahead of time, not just at serve-time.
pub(crate) fn load_and_pin(conn: &Connection) -> Result<()> {
    if let Err(bare_load_error) = conn.execute_batch("LOAD quack;") {
        // `FROM core` is required, not merely the default: see
        // [`EXPECTED_QUACK_VERSION`]'s doc comment for why `core_nightly`
        // must never be used here.
        conn.execute_batch("INSTALL quack FROM core; LOAD quack;")
            .with_context(|| {
                format!(
                    "failed to INSTALL/LOAD the quack extension from the core repository; \
                     bare `LOAD quack;` failed first with: {bare_load_error:#}"
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

/// Fail-closed Rust scalar UDF backing `quack_authorization_function`.
///
/// Signature: `sigiro_quack_authorize(connection_id VARCHAR, query VARCHAR) ->
/// BOOLEAN` — matches quack's authorization-callback contract exactly.
/// `connection_id` is ignored: sigiro's guard is per-query, not
/// per-connection. Returns `true` iff [`crate::server::validate_read_only`]
/// — the SAME firewall `/v1/query` enforces — accepts `query`: SELECT-only,
/// `sigiro_*` table allowlist, reader-function denylist.
///
/// Fails closed on every edge:
/// - a NULL `query` row this function's OWN `row_is_null` check sees is
///   denied without ever calling into the validator. In practice, per-row
///   NULL propagation happens a level ABOVE this: this crate's `VScalar` has
///   no hook to opt into DuckDB's "special" null handling (see
///   `self_test_authorization_gate`'s doc comment below), so DuckDB's
///   DEFAULT null handling intercepts any row where `query` is NULL and
///   substitutes SQL NULL as that row's output directly — `invoke` is never
///   called for it at all. Either way (SQL NULL from DuckDB's own
///   interception, or this function explicitly returning `false`) the
///   authorization result for that row is something other than the SQL
///   literal `TRUE`, and quack's authorization callback contract treats
///   `TRUE` as the ONLY value that authorizes a query — `FALSE` and `NULL`
///   are both denials. So a query whose `query` argument is NULL is denied
///   either way; this function's own `row_is_null` branch is redundant
///   defense-in-depth (for any duckdb-crate version/build where that
///   default-null-interception assumption doesn't hold), not the sole guard;
/// - `DuckString::as_str` lossily replaces invalid UTF-8 rather than
///   erroring — the resulting mangled text simply fails to parse as valid
///   SQL, which `validate_read_only` already rejects, so no special case is
///   needed for it;
/// - the validator call is wrapped in `catch_unwind` — a panic must never
///   unwind across the DuckDB FFI boundary (undefined behavior), so one is
///   converted into a denial instead.
struct QuackAuthorizeScalar;

impl VScalar for QuackAuthorizeScalar {
    type State = ();

    unsafe fn invoke(
        _: &Self::State,
        input: &mut DataChunkHandle,
        output: &mut dyn WritableVector,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let len = input.len();
        // Param 0 is `connection_id` (ignored); param 1 is `query`.
        let query_col = input.flat_vector(1);
        let query_ptrs = unsafe { query_col.as_slice_with_len::<duckdb_string_t>(len) };

        let mut output_vec = output.flat_vector();
        let out_slice = unsafe { output_vec.as_mut_slice::<bool>() };
        for (i, ptr) in query_ptrs.iter().enumerate().take(len) {
            out_slice[i] = if query_col.row_is_null(i as u64) {
                false
            } else {
                let query = DuckString::new(&mut { *ptr }).as_str().to_string();
                std::panic::catch_unwind(|| crate::server::validate_read_only(&query).is_ok())
                    .unwrap_or(false)
            };
        }
        Ok(())
    }

    fn signatures() -> Vec<ScalarFunctionSignature> {
        vec![ScalarFunctionSignature::exact(
            vec![
                LogicalTypeHandle::from(LogicalTypeId::Varchar),
                LogicalTypeHandle::from(LogicalTypeId::Varchar),
            ],
            LogicalTypeHandle::from(LogicalTypeId::Boolean),
        )]
    }
}

/// Boot-time self-test: invoke the just-registered gate directly by SQL name
/// (not over the quack wire — quack isn't serving yet) with one known-good
/// and one known-bad query, and refuse to serve if either misbehaves. This
/// catches a registration mistake (wrong resolvable name, wrong signature)
/// before quack ever binds a port, rather than discovering it as a silent
/// "Authorization failed" on every real client request.
///
/// Uses a non-NULL placeholder for `connection_id` (never literal SQL
/// `NULL`, and never something a real quack call would send): the
/// `duckdb = "1.10503.1"` crate's `VScalar` API has no hook for DuckDB's
/// "special" null handling, so a registered scalar UDF gets DEFAULT null
/// handling — DuckDB substitutes SQL NULL as the *whole row's output*
/// whenever ANY argument is NULL, without ever calling into
/// [`QuackAuthorizeScalar::invoke`]. A literal NULL connection_id here
/// previously made every self-test call return NULL instead of a boolean
/// (`row.get::<_, bool>(0)` then failed with "Invalid column type Null"),
/// regardless of whether `query` was good or bad — a self-test bug, not a
/// gate bug: quack's real authorization callback always sends a real
/// per-connection id, never NULL.
fn self_test_authorization_gate(conn: &Connection) -> Result<()> {
    let allowed: bool = conn
        .query_row(
            &format!(
                "SELECT {AUTH_FUNCTION_NAME}('self-test', 'SELECT 1 FROM sigiro_spans LIMIT 1')"
            ),
            [],
            |row| row.get(0),
        )
        .context("authorization gate self-test: failed to invoke the gate on a known-good query")?;
    anyhow::ensure!(
        allowed,
        "authorization gate self-test FAILED: a known-good SELECT against an \
         allowlisted table was DENIED — refusing to serve the quack query surface"
    );

    let denied: bool = conn
        .query_row(
            &format!("SELECT {AUTH_FUNCTION_NAME}('self-test', 'DROP TABLE sigiro_spans')"),
            [],
            |row| row.get(0),
        )
        .context("authorization gate self-test: failed to invoke the gate on a known-bad query")?;
    anyhow::ensure!(
        !denied,
        "authorization gate self-test FAILED: a known-bad DROP TABLE was ALLOWED — \
         refusing to serve the quack query surface"
    );

    // A syntactically valid, structurally allowlisted `SELECT` (no relation
    // to reject, no blocked keyword) that leaks process/DuckDB config via a
    // scalar function call — the exact hole `AllowlistVisitor`'s scalar
    // allowlist (`server::SCALAR_FUNCTIONS`) closes. Catches a regression
    // where the gate's function-call check is missing or misregistered
    // before quack ever binds a port.
    let getenv_denied: bool = conn
        .query_row(
            &format!(
                "SELECT {AUTH_FUNCTION_NAME}('self-test', 'SELECT getenv(''AWS_SECRET_ACCESS_KEY'')')"
            ),
            [],
            |row| row.get(0),
        )
        .context(
            "authorization gate self-test: failed to invoke the gate on a known-bad getenv query",
        )?;
    anyhow::ensure!(
        !getenv_denied,
        "authorization gate self-test FAILED: a known-bad SELECT getenv(...) was ALLOWED — \
         refusing to serve the quack query surface"
    );

    let current_setting_denied: bool = conn
        .query_row(
            &format!(
                "SELECT {AUTH_FUNCTION_NAME}('self-test', 'SELECT current_setting(''extension_directory'')')"
            ),
            [],
            |row| row.get(0),
        )
        .context(
            "authorization gate self-test: failed to invoke the gate on a known-bad current_setting query",
        )?;
    anyhow::ensure!(
        !current_setting_denied,
        "authorization gate self-test FAILED: a known-bad SELECT current_setting(...) was \
         ALLOWED — refusing to serve the quack query surface"
    );

    // The `sys_*` host-statistics functions are a DELIBERATE, NAMED exception
    // to the config-leak denials asserted immediately above: federated fleet
    // queries (crate::federate) reach them through this surface, so they must
    // be permitted here — while `getenv`/`current_setting` stay denied. Both
    // halves are asserted so the pairing can never drift: widening the gate
    // far enough to leak process env or DuckDB config still refuses to bind a
    // port, and narrowing it far enough to break fleet queries fails at boot
    // instead of at the first query.
    let sys_stats_allowed: bool = conn
        .query_row(
            &format!("SELECT {AUTH_FUNCTION_NAME}('self-test', 'SELECT * FROM sys_disk_info()')"),
            [],
            |row| row.get(0),
        )
        .context(
            "authorization gate self-test: failed to invoke the gate on a known-good \
             sys_disk_info() query",
        )?;
    anyhow::ensure!(
        sys_stats_allowed,
        "authorization gate self-test FAILED: SELECT * FROM sys_disk_info() was DENIED — \
         federated fleet queries would fail at every edge. server::SAFE_FUNCTIONS and \
         federate::SYS_STATS_FUNCTIONS must agree; refusing to serve the quack query surface"
    );

    // A `sys_*` call carrying arguments is NOT part of that exception. The
    // allowlist matches on function NAME, so this asserts the surrounding
    // structural checks still reject a call shape the coordinator never
    // produces, rather than trusting the name alone.
    let sys_stats_args_denied: bool = conn
        .query_row(
            &format!(
                "SELECT {AUTH_FUNCTION_NAME}('self-test', 'SELECT * FROM sys_disk_info(''/etc/passwd'')')"
            ),
            [],
            |row| row.get(0),
        )
        .context(
            "authorization gate self-test: failed to invoke the gate on a known-bad \
             argument-carrying sys_disk_info() query",
        )?;
    anyhow::ensure!(
        !sys_stats_args_denied,
        "authorization gate self-test FAILED: sys_disk_info() WITH AN ARGUMENT was ALLOWED — \
         refusing to serve the quack query surface"
    );

    Ok(())
}

/// Install the read-only authorization gate and point quack at it.
///
/// (Re)registered on every server boot from this Rust source — never trusted
/// from anything a previous run may have left behind, so a stale/tampered
/// definition can never survive a restart.
pub(crate) fn install_authorization_gate(conn: &Connection) -> Result<()> {
    conn.register_scalar_function::<QuackAuthorizeScalar>(AUTH_FUNCTION_NAME)
        .context("failed to register the quack authorization scalar UDF")?;

    self_test_authorization_gate(conn)
        .context("quack authorization gate self-test failed immediately after registration")?;

    // Quack's authorization callback runs in a fresh, transient server-side
    // connection per call (per quack's own docs) — NOT the connection this
    // gate is installed from. Empirically proved by a since-deleted spike
    // (tests/spike_quack_udf_authz.rs): unlike the SQL-macro gate this
    // replaces — which needed catalog-qualifying as `memory.main.<name>` to
    // resolve from that transient connection — a Rust-registered scalar UDF
    // resolves from its UNQUALIFIED name instead; `memory.main.<name>` does
    // NOT resolve for a UDF (only for a macro). Qualifying it here would
    // silently break every authorization check exactly the way the
    // unqualified macro once did.
    conn.execute_batch(&format!(
        "SET GLOBAL quack_authorization_function = '{AUTH_FUNCTION_NAME}';"
    ))
    .context("failed to install quack_authorization_function")?;

    tracing::info!(
        function = AUTH_FUNCTION_NAME,
        "quack authorization gate installed — Rust scalar UDF running sigiro's \
         validate_read_only firewall (SELECT-only + sigiro_* table allowlist + \
         reader-function denylist), fail-closed, self-tested"
    );
    Ok(())
}

/// Result of starting the quack server: what it reports listening on.
pub(crate) struct QuackListen {
    pub(crate) listen_uri: String,
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
pub(crate) fn start_server(
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
        "SELECT listen_uri FROM quack_serve('{}'",
        crate::storage::escape_sql_string(&uri)
    );
    if let Some(t) = token {
        sql.push_str(&format!(
            ", token := '{}'",
            crate::storage::escape_sql_string(t)
        ));
    }
    sql.push_str(", allow_other_hostname := true, disable_ssl := true);");

    let listen_uri = conn
        .query_row(&sql, [], |row| row.get::<_, String>(0))
        .context("failed to start quack_serve")?;

    Ok(QuackListen { listen_uri })
}

/// Stop the quack server started by [`start_server`] for the same
/// `bind_addr:port`. Called before dropping `conn` on shutdown — dropping a
/// live quack-served connection without stopping the server first aborts
/// the process (observed directly: `libc++abi: terminating`).
pub(crate) fn stop_server(conn: &Connection, bind_addr: &str, port: u16) -> Result<()> {
    let uri = format!("quack://{bind_addr}:{port}");
    conn.execute_batch(&format!(
        "CALL quack_stop('{}');",
        crate::storage::escape_sql_string(&uri)
    ))
    .context("failed to stop the quack server cleanly")?;
    Ok(())
}
