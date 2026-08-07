//! Federated host-statistics (`sys_*`) queries, end to end.
//!
//! Covers the two independent gates a fleet-wide `SELECT ... FROM
//! sys_disk_info()` must pass, using the REAL production code paths — the
//! same `validate_read_only` firewall the edge's quack authorization UDF
//! runs, and the same DuckDB parser (`json_serialize_sql`) the coordinator
//! plans with:
//!
//! 1. the edge-side authorization gate (`server::validate_read_only`, backed
//!    by `SAFE_FUNCTIONS`), and
//! 2. the coordinator-side federated-query planner (`federate::plan_query`,
//!    backed by `SYS_STATS_FUNCTIONS`).
//!
//! A name must be on BOTH lists or a query passes one and is denied by the
//! other, so both are asserted here together.

use sigiro::server::validate_read_only;

/// The `system_stats` extension actually provides the columns this feature
/// reports, on this host, through DuckDB itself. Guards against the whole
/// design resting on a column name that does not exist.
#[test]
fn system_stats_extension_reports_disk_and_host_columns() {
    let conn = duckdb::Connection::open_in_memory().expect("open in-memory duckdb");
    conn.execute_batch("INSTALL system_stats FROM community; LOAD system_stats;")
        .expect("install/load system_stats from the community repo");

    // Disk: the fleet-monitoring payload. `free_space`/`total_space` are what
    // a "which node is filling up" query divides.
    let (mount, total, free): (String, u64, u64) = conn
        .query_row(
            "SELECT mount_point, total_space, free_space FROM sys_disk_info() \
             ORDER BY total_space DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("sys_disk_info() returns mount_point/total_space/free_space");
    assert!(!mount.is_empty(), "mount_point must not be empty");
    assert!(total > 0, "total_space must be positive, got {total}");
    assert!(
        free <= total,
        "free_space ({free}) must not exceed total_space ({total})"
    );

    // OS: `host_name` is the edge-reported node identity that labels each row
    // of a fanned-out fleet result.
    let host_name: String = conn
        .query_row("SELECT host_name FROM sys_os_info()", [], |r| r.get(0))
        .expect("sys_os_info() returns host_name");
    assert!(!host_name.is_empty(), "host_name must not be empty");

    println!("host {host_name}: largest mount {mount} total={total} free={free}");
}

/// Every `sys_*` function the coordinator can push must be accepted by the
/// edge's gate — otherwise the query is planned, fanned out, and denied at
/// every edge.
#[test]
fn gate_allows_every_sys_stats_function() {
    for func in [
        "sys_cpu_info",
        "sys_disk_info",
        "sys_memory_info",
        "sys_network_info",
        "sys_os_info",
    ] {
        let sql = format!("SELECT * FROM {func}()");
        validate_read_only(&sql)
            .unwrap_or_else(|e| panic!("gate must allow {func}(), but rejected it: {e}"));
    }

    // The shapes the coordinator actually generates: projection, aggregate
    // partials, GROUP BY, and a bare-call FROM (never `lake.`-qualified).
    let shapes = [
        "SELECT mount_point, free_space FROM sys_disk_info()",
        "SELECT count(*) AS n FROM sys_disk_info()",
        "SELECT file_system_type, min(free_space) AS lo FROM sys_disk_info() \
         GROUP BY file_system_type",
        "SELECT * FROM (SELECT free_space FROM sys_disk_info()) sigiro_partial LIMIT 101",
    ];
    for sql in shapes {
        validate_read_only(sql)
            .unwrap_or_else(|e| panic!("gate must allow coordinator shape {sql:?}: {e}"));
    }
}

/// The allowlist matches on function NAME, so an argument-carrying call would
/// otherwise ride in on a permitted name and hand a caller-controlled value to
/// the extension. No allowlisted `sys_*` function takes arguments.
#[test]
fn gate_rejects_sys_stats_calls_carrying_arguments() {
    let rejected = [
        "SELECT * FROM sys_disk_info('/etc/passwd')",
        "SELECT * FROM sys_os_info('x')",
        "SELECT * FROM sys_memory_info(0)",
    ];
    for sql in rejected {
        let result = validate_read_only(sql);
        assert!(
            result.is_err(),
            "gate must reject an argument-carrying sys_* call, but allowed {sql:?}"
        );
        println!("rejected {sql:?}: {}", result.unwrap_err());
    }
}

/// Widening the gate for `sys_*` must NOT reopen the reader-family or
/// config-leak holes the gate exists to close. These are the exact assertions
/// the quack authorization self-test refuses to boot without.
#[test]
fn gate_still_rejects_reader_family_and_config_leaks() {
    let rejected = [
        "SELECT * FROM read_csv('/etc/passwd')",
        "SELECT * FROM glob('/etc/*')",
        "SELECT * FROM duckdb_secrets()",
        "SELECT * FROM duckdb_settings()",
        "SELECT getenv('AWS_SECRET_ACCESS_KEY')",
        "SELECT current_setting('extension_directory')",
        "SELECT * FROM some_other_table",
        // A sys_*-looking name that is NOT on the allowlist must not pass on
        // resemblance alone.
        "SELECT * FROM sys_process_info()",
    ];
    for sql in rejected {
        assert!(
            validate_read_only(sql).is_err(),
            "gate must still reject {sql:?} after the sys_* widening"
        );
    }
}

/// `sys_*` describes the machine sigiro runs on. On the hosted service that
/// machine is the operator's, never the tenant's — so the surface both leaks
/// our infrastructure and can only ever return an answer about the wrong host.
/// Gated on the same `is_distributed()` axis `information_schema` already uses,
/// so the self-hosted fleet case is untouched.
#[test]
fn sys_functions_are_rejected_on_hosted_and_allowed_when_self_hosted() {
    for sql in [
        "SELECT * FROM sys_disk_info()",
        "SELECT * FROM sys_memory_info()",
        "SELECT * FROM sys_cpu_info()",
        "SELECT * FROM sys_network_info()",
        "SELECT * FROM sys_os_info()",
    ] {
        // Hosted: rejected, and the message must name the alternative rather
        // than only refusing.
        let err = sigiro::server::validate_read_only_gated(sql, false, false)
            .expect_err("hosted must reject sys_* — it describes the operator's host");
        assert!(
            err.contains("sigiro_metrics_*"),
            "rejection should redirect to sigiro_metrics_*, got: {err}"
        );

        // Self-host / local: allowed. This is the fleet-stats use case and it
        // must not regress.
        sigiro::server::validate_read_only_gated(sql, true, true)
            .unwrap_or_else(|e| panic!("self-host must allow {sql}: {e}"));
    }

    // The ordinary tables stay reachable on hosted — the gate is scoped to
    // sys_*, not a blanket tightening.
    sigiro::server::validate_read_only_gated(
        "SELECT service_name FROM sigiro_spans WHERE timestamp > now()",
        false,
        false,
    )
    .expect("hosted must still allow the sigiro_* tables");
}
