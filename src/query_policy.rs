//! What `/v1/query` may run, decided by DuckDB itself.
//!
//! The policy is the [gatekeeper](https://github.com/nozzle/duckdb-gatekeeper)
//! DuckDB extension, compiled into the binary (duckdb-static). It parses and
//! binds a statement, then checks what it resolved to: one read-only statement
//! (`SELECT`, `WITH`, `DESCRIBE`, `SUMMARIZE`, `PIVOT`), only the `sigiro_*`
//! tables in [`sigiro_segments::ALLOWED_TABLES`], and only functions on
//! gatekeeper's reviewed list of about 920 side-effect-free defaults. Readers
//! of files, settings, secrets, the environment, the catalog or remote
//! databases are not on it, and SQL that runs other SQL (`query`,
//! `quack_query`) can never be granted.
//!
//! A project's DuckDB instance is set up once, by [`install`]: the policy is
//! configured and the configuration locked. Every reader connection is then
//! enforced ([`enforce`]): DuckDB refuses anything the policy denies, with a
//! `Permission Error`. Each query is also checked first, by [`check`] on a
//! separate, unenforced connection, because DuckDB evaluates `PRAGMA`
//! arguments while parsing, before an enforced connection can refuse the
//! statement (gatekeeper's documented residual). [`check`] passes the text as
//! a bound parameter, so it is never parsed as a statement.
//!
//! The policy is not a resource limit: a recursive CTE can run until the query
//! timeout (`SIGIRO_QUERY_TIMEOUT_SECS`) interrupts it. It filters no rows or
//! columns either; a project's tables are the project's own.
//!
//! The quack edge (`crate::quack`) does not use this: quack runs each remote
//! session on a connection it creates itself, which nothing here enforces, so
//! the edge keeps authorizing queries with `sigiro_segments::validate_read_only`.

use anyhow::{Context, Result};
use duckdb::Connection;

/// Settings sigiro still changes after the configuration is locked: the writer
/// lowers `threads` around a checkpoint, and the quack edge installs its
/// authorization function. Enforced connections cannot run `SET` at all.
const SETTINGS_CHANGED_AFTER_LOCK: &[&str] = &["threads", "quack_authorization_function"];

/// A statement the policy refused, or one DuckDB could not parse or bind.
#[derive(Debug, thiserror::Error)]
#[error("{code}: {detail}")]
pub struct QueryRejected {
    /// gatekeeper's decision: `forbidden`, `unsupported`, `parser`, `binding`
    /// or `invalid_input`.
    pub code: String,
    /// The violated rules, or DuckDB's own error for `parser` and `binding`.
    pub detail: String,
}

/// The `allowed_tables` policy: every queryable `sigiro_*` table, by its
/// resolved identity in the DuckLake catalog, whichever name the query used
/// (`sigiro_spans`, `lake.sigiro_spans`, `main.sigiro_spans`).
fn allowed_tables() -> String {
    let tables: Vec<String> = sigiro_segments::ALLOWED_TABLES
        .iter()
        .map(|table| {
            format!(
                "{{catalog: 'lake', schema_path: ['main'], 'table': '{}'}}",
                table.to_lowercase()
            )
        })
        .collect();
    format!("[{}]", tables.join(", "))
}

/// Configure the query policy on a project's DuckDB instance and lock the
/// configuration. Runs once, after DuckLake is attached and the filesystem
/// locked down, before any reader connection is handed out.
pub(crate) fn install(conn: &Connection) -> Result<()> {
    // Extension autoloading is compiled out, so the extensions a query may use
    // are loaded here, in every storage mode (only local mode's filesystem
    // lockdown loads them otherwise). gatekeeper itself is already loaded.
    crate::storage::load_extensions(conn, crate::storage::LOCKDOWN_PRELOADED_EXTENSIONS)?;

    let late: Vec<String> = SETTINGS_CHANGED_AFTER_LOCK
        .iter()
        .map(|name| format!("'{name}'"))
        .collect();
    conn.execute_batch(&format!(
        "SET autoload_known_extensions = false;
         SET autoinstall_known_extensions = false;
         CALL gatekeeper_configure(allowed_tables := {});
         SET allowed_configs = [{}];
         SET lock_configuration = true;",
        allowed_tables(),
        late.join(", ")
    ))
    .context("failed to install the /v1/query policy")?;
    Ok(())
}

/// Make `conn` refuse every statement the policy denies, for good. `conn` must
/// be a fresh connection outside a transaction.
pub(crate) fn enforce(conn: &Connection) -> Result<()> {
    conn.execute_batch("CALL gatekeeper_enforce();")
        .context("failed to enforce the /v1/query policy on a reader connection")
}

/// Check `sql` against the policy without running it. `validator` is a
/// connection of the same instance, with the same default catalog as the
/// readers, that is not enforced (an enforced connection may not call
/// `gatekeeper_validate`).
pub(crate) fn check(validator: &Connection, sql: &str) -> Result<(), QueryRejected> {
    let decision = validator.query_row(
        "SELECT allowed, code, error_message, \
                array_to_string(list_transform(violations, lambda v: \
                    v.rule || ': ' || v.message || \
                    CASE WHEN v.\"table\" <> '' \
                         THEN ' (' || array_to_string(list_append(v.schema_path, v.\"table\"), '.') || ')' \
                         ELSE '' END), '; ') \
         FROM gatekeeper_validate(?)",
        [sql],
        |row| {
            Ok((
                row.get::<_, bool>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        },
    );
    match decision {
        Ok((true, code, _, _)) if code == "ok" => Ok(()),
        Ok((_, code, error, violations)) => {
            let detail = [violations, error]
                .into_iter()
                .flatten()
                .find(|text| !text.is_empty())
                .unwrap_or_else(|| "denied".to_string());
            Err(QueryRejected { code, detail })
        }
        // No decision is a denial.
        Err(error) => Err(QueryRejected {
            code: "invalid_input".to_string(),
            detail: error.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use crate::pool::{ProjectPool, ReaderPool};
    use crate::storage::StorageMode;

    /// A local project opened as `sigiro serve` opens it, holding two logs and
    /// a table and a sequence outside the policy, in the instance's own
    /// in-memory catalog.
    fn project() -> (tempfile::TempDir, ProjectPool, ReaderPool) {
        let dir = tempfile::tempdir().unwrap();
        let data = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let pool = ProjectPool::new(data, 4, StorageMode::LocalFile);
        let writer = pool.create_project_writer("p").unwrap();
        writer
            .execute_batch(
                "INSERT INTO lake.sigiro_logs (timestamp, observed_timestamp, service_name, \
                     severity_number, severity_text, body, log_attributes) VALUES \
                 (TIMESTAMP '2026-11-26 00:00:00', TIMESTAMP '2026-11-26 00:00:00', 'api', 9, 'INFO', 'ok', '{\"k\": \"a\"}'), \
                 (TIMESTAMP '2026-11-26 00:00:01', TIMESTAMP '2026-11-26 00:00:01', 'api', 17, 'ERROR', 'not json', '{\"k\": \"b\"}'); \
                 CREATE TABLE memory.main.secrets AS SELECT 'hunter2' AS token; \
                 CREATE SEQUENCE memory.main.s;",
            )
            .unwrap();
        pool.return_writer("p", writer);
        let readers = pool.create_project("p").unwrap();
        (dir, pool, readers)
    }

    /// Check `sql`, then run it on a reader; the first column of each row.
    fn query(readers: &ReaderPool, sql: &str) -> Result<Vec<String>, String> {
        readers.check(sql).map_err(|e| e.to_string())?;
        let reader = readers.get();
        let mut stmt = reader.prepare(sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| row.get::<_, duckdb::types::Value>(0))
            .map_err(|e| e.to_string())?;
        rows.map(|value| value.map(|v| format!("{v:?}")).map_err(|e| e.to_string()))
            .collect()
    }

    #[test]
    fn issue_227_queries_run() {
        let (_dir, _pool, readers) = project();
        for sql in [
            "DESCRIBE sigiro_spans",
            "SUMMARIZE sigiro_logs",
            "WITH errors AS (SELECT * FROM sigiro_logs WHERE severity_number >= 17) SELECT count(*) FROM errors",
            "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r WHERE n < 3) SELECT sum(n) FROM r",
            "SELECT string_agg(body, ',' ORDER BY body) FROM sigiro_logs",
            "SELECT list_distinct(list(service_name)) FROM sigiro_logs",
            "SELECT json_valid(body), left(body, 2), md5(body) FROM sigiro_logs",
            "SELECT log_attributes::JSON->>'k', epoch_us(timestamp) FROM sigiro_logs",
            "SELECT quantile_cont(severity_number, 0.9) FROM lake.sigiro_logs",
            "SELECT list_filter([1, 2, 3], lambda x: x > 1) FROM main.sigiro_logs",
        ] {
            assert!(
                query(&readers, sql).is_ok_and(|rows| !rows.is_empty()),
                "{sql}: {:?}",
                query(&readers, sql)
            );
        }
        // DuckLake rows, read through the policy.
        assert_eq!(
            query(&readers, "SELECT count(*) FROM sigiro_logs WHERE timestamp >= TIMESTAMP '2026-11-26 00:00:00'").unwrap(),
            ["BigInt(2)"]
        );
    }

    #[test]
    fn everything_outside_the_telemetry_tables_is_refused() {
        let (_dir, _pool, readers) = project();
        for sql in [
            // A CTE named like an allowed table must not unlock another table.
            "WITH sigiro_logs AS (SELECT * FROM memory.main.secrets) SELECT * FROM sigiro_logs",
            "SELECT * FROM memory.main.secrets",
            "SELECT table_name FROM information_schema.tables",
            "SELECT * FROM __ducklake_metadata_lake.ducklake_snapshot",
            "SELECT current_setting('threads')",
            "SELECT getenv('HOME')",
            "SELECT * FROM read_csv('/etc/passwd')",
            "SELECT * FROM '/etc/passwd'",
            "SELECT * FROM query('SELECT * FROM memory.main.secrets')",
            "SELECT * FROM duckdb_settings()",
            "SET threads = 1",
            "COPY sigiro_logs TO 'out.csv'",
            "ATTACH ':memory:' AS other",
            "INSTALL httpfs",
            "DELETE FROM sigiro_logs",
            "SELECT 1 FROM sigiro_logs; SELECT 2 FROM sigiro_logs",
        ] {
            assert!(readers.check(sql).is_err(), "{sql} was allowed");
        }
    }

    #[test]
    fn a_pragma_is_refused_before_duckdb_evaluates_its_arguments() {
        let (_dir, pool, readers) = project();
        assert!(
            readers
                .check("PRAGMA version_x(nextval('memory.main.s'))")
                .is_err()
        );
        let writer = pool.take_writer("p").unwrap();
        let next: i64 = writer
            .query_row("SELECT nextval('memory.main.s')", [], |r| r.get(0))
            .unwrap();
        assert_eq!(next, 1, "the refused PRAGMA advanced the sequence");
        pool.return_writer("p", writer);
    }

    #[test]
    fn a_reader_refuses_a_denied_query_by_itself() {
        let (_dir, _pool, readers) = project();
        let reader = readers.get();
        let error = reader
            .query_row("SELECT token FROM memory.main.secrets", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap_err()
            .to_string();
        assert!(error.contains("Permission Error"), "{error}");
    }

    /// The federated `sigiro query` path: an edge serves its locked instance
    /// over quack, and its own authorization gate decides what runs.
    #[test]
    fn a_locked_project_still_serves_quack() {
        let (_dir, pool, _readers) = project();
        let edge = pool.clone_connection("p").unwrap();
        edge.execute_batch("LOAD httpfs; LOAD quack;").unwrap();
        crate::quack::install_authorization_gate(&edge).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let uri = crate::quack::QuackUri::new([127, 0, 0, 1].into(), port);
        let token = crate::quack::QuackToken::try_new("test-token".to_string()).unwrap();
        crate::quack::start_server(&edge, &uri, Some(&token)).unwrap();

        let client = crate::pool::open_duckdb().unwrap();
        client.execute_batch("LOAD httpfs; LOAD quack;").unwrap();
        let remote = |sql: &str| {
            client.query_row(
                &format!("SELECT * FROM quack_query('quack://127.0.0.1:{port}', '{sql}', token := 'test-token')"),
                [],
                |r| r.get::<_, i64>(0),
            )
        };
        let served = remote("SELECT count(*) FROM lake.sigiro_logs");
        let refused = remote("SELECT count(*) FROM memory.main.secrets");
        crate::quack::stop_server(&edge, &uri).unwrap();
        assert_eq!(served.unwrap(), 2);
        assert!(refused.is_err());
    }
}
