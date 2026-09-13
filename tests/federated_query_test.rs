//! End-to-end test for the Phase C federated query coordinator
//! (`sigiro query "<SQL>" --edge name=host:port ...`).
//!
//! Spawns TWO real `sigiro serve` processes (each with
//! `SIGIRO_QUERY_SURFACE=quack`, matching `tests/quack_query_surface_test.rs`'s
//! convention of driving the real production boot sequence rather than an
//! in-process `AppState`), ingests DISTINCT-but-OVERLAPPING span data into
//! each via real OTLP/HTTP export with EXACT, controlled span durations (so
//! sum/avg/min/max ground truth is exact arithmetic, not guesswork), then
//! drives the REAL `sigiro` binary's `query` subcommand as a child process
//! (`--edge` overrides, bypassing tailnet discovery — the documented
//! testing path) and asserts the folded result against hand-computed ground
//! truth.
//!
//! The money assertion is `avg(duration)`: naive avg-of-avgs across the two
//! edges would be measurably wrong here (each edge's own per-service average
//! differs from the true cross-edge average — see the per-service comments
//! below) — the federated coordinator must fold via
//! `sum(partial sums) / sum(partial counts)`, not by averaging averages.
//!
//! Run with: `cargo test --release --test federated_query_test -- --nocapture`
//! (add `--ignored` to include the 5-iteration stability loop).

#![allow(clippy::disallowed_methods)]
// Test harnesses read and write the process environment directly to drive
// storage/config permutations. `clippy.toml` bans that in production code so
// every reader goes through typed-env; a test-only knob routed through
// typed-env would add a production config entry production never reads.

use opentelemetry::trace::{Span, Tracer, TracerProvider};
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::resource::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

fn test_extension_dir() -> String {
    std::env::var("SIGIRO_EXTENSION_DIR").unwrap_or_else(|_| {
        std::env::var("HOME")
            .map(|home| format!("{home}/.duckdb/extensions"))
            .unwrap_or_else(|_| "/tmp/sigiro-test-extensions".to_string())
    })
}

/// A live `sigiro serve` child process configured as a quack edge.
struct EdgeServer {
    child: Child,
    _data_dir: tempfile::TempDir,
    http_port: u16,
    otlp_http_port: u16,
    quack_port: u16,
}

impl EdgeServer {
    /// Launch the child process and return WITHOUT waiting for readiness.
    /// `Command::spawn` does not block, so the OS boots this server while the
    /// caller starts the next one. The caller MUST call `wait_ready` before
    /// using the returned edge.
    fn spawn_booting(name: &str, base: u16, token: &str) -> Self {
        let data_dir = tempfile::tempdir().expect("failed to create temp data dir");
        let (http_port, otlp_http_port, grpc_port, quack_port) =
            (base, base + 1, base + 2, base + 3);

        let child = Command::new(env!("CARGO_BIN_EXE_sigiro"))
            .arg("serve")
            .env("SIGIRO_DATA_DIR", data_dir.path())
            .env("SIGIRO_HTTP_PORT", http_port.to_string())
            .env("SIGIRO_OTLP_HTTP_PORT", otlp_http_port.to_string())
            .env("SIGIRO_GRPC_PORT", grpc_port.to_string())
            .env("SIGIRO_QUERY_SURFACE", "quack")
            .env("SIGIRO_QUACK_BIND", "127.0.0.1")
            .env("SIGIRO_QUACK_PORT", quack_port.to_string())
            .env("SIGIRO_QUACK_TOKEN", token)
            .env("SIGIRO_EXTENSION_DIR", test_extension_dir())
            // Pin the flush cadence: this test waits for ingest to become
            // visible on a 10s deadline, so it depends on the flusher timer.
            // The production default is 60s and would time it out. Nothing here
            // is testing flush cadence, so fix the input rather than the wait.
            .env("SIGIRO_FLUSH_INTERVAL_SECS", "1")
            .env("OTEL_EXPORTER_OTLP_ENDPOINT", "")
            .env("RUST_LOG", "info")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("failed to spawn edge {name}: {e}"));

        Self {
            child,
            _data_dir: data_dir,
            http_port,
            otlp_http_port,
            quack_port,
        }
    }

    fn wait_ready(&self, name: &str) {
        // 90s (not 30s): under concurrent test load / first-boot quack
        // extension download+load, 30s was too tight and produced flaky
        // false failures unrelated to the thing under test.
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            if let Ok(resp) =
                reqwest::blocking::get(format!("http://127.0.0.1:{}/health", self.http_port))
                && resp.status().is_success()
                && let Ok(body) = resp.json::<serde_json::Value>()
                && body.get("status").and_then(|v| v.as_str()) == Some("ready")
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "edge {name} did not report /health ready within 90s"
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Ingest one span per entry in `durations_ms`, each with an EXACT
    /// controlled duration (explicit start/end timestamps, not wall-clock
    /// timing) so the resulting `duration` column (MICROSECONDS — verified
    /// directly against `src/otlp.rs`: `nanos_to_micros(dur)`) is exact,
    /// deterministic ground truth: N ms ingested → N*1000 µs stored.
    fn ingest_durations(&self, service_name: &str, durations_ms: &[u64]) {
        let exporter = SpanExporter::builder()
            .with_http()
            .with_endpoint(format!(
                "http://127.0.0.1:{}/v1/traces",
                self.otlp_http_port
            ))
            .build()
            .expect("failed to build OTLP span exporter");
        let provider = SdkTracerProvider::builder()
            .with_resource(
                Resource::builder()
                    .with_service_name(service_name.to_string())
                    .build(),
            )
            .with_batch_exporter(exporter)
            .build();
        let tracer = provider.tracer("federated-e2e-test");
        for (i, ms) in durations_ms.iter().enumerate() {
            let start = SystemTime::now();
            let mut span = tracer
                .span_builder(format!("op-{i}"))
                .with_start_time(start)
                .start(&tracer);
            span.end_with_timestamp(start + Duration::from_millis(*ms));
        }
        provider.shutdown().expect("failed to flush OTLP spans");
    }

    /// Poll row count for `service_name` until ingest is visible (this test
    /// pins the flusher to 1s; see `spawn`) or a 10s deadline passes. Uses a direct
    /// quack_query round trip (bypassing the coordinator entirely) purely as
    /// test setup — not part of the thing under test.
    fn wait_for_ingest(&self, service_name: &str, expected: i64, token: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let conn = duckdb::Connection::open_in_memory().expect("open probe conn");
            conn.execute_batch("LOAD quack;").expect("load quack");
            let sql = format!(
                "SELECT * FROM quack_query('quack://127.0.0.1:{}', 'SELECT count(*) FROM lake.sigiro_spans WHERE service_name = ''{service_name}''', token := '{token}', disable_ssl := true)",
                self.quack_port
            );
            let n: i64 = conn.query_row(&sql, [], |r| r.get(0)).unwrap_or(0);
            if n >= expected || Instant::now() >= deadline {
                assert!(
                    n >= expected,
                    "timed out waiting for {expected} rows for {service_name}, saw {n}"
                );
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn edge_arg(&self, name: &str) -> String {
        format!("{name}=127.0.0.1:{}", self.quack_port)
    }

    fn shutdown(mut self) {
        let pid = self.child.id() as libc::pid_t;
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        drop(self.child.kill());
                        drop(self.child.wait());
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(_) => return,
            }
        }
    }
}

/// Run `sigiro query` as a real child process against the given edges.
fn run_query(
    sql: &str,
    edges: &[String],
    token: &str,
    extra_args: &[&str],
) -> std::process::Output {
    run_query_env(sql, edges, token, extra_args, &[])
}

/// Same as `run_query`, plus extra environment variables on the child
/// process — used by the `SIGIRO_QUERY_ROW_CAP` fail-loud test, which needs
/// a cap override scoped to one query rather than the whole test binary.
fn run_query_env(
    sql: &str,
    edges: &[String],
    token: &str,
    extra_args: &[&str],
    extra_env: &[(&str, &str)],
) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sigiro"));
    cmd.arg("query").arg(sql);
    for e in edges {
        cmd.arg("--edge").arg(e);
    }
    for a in extra_args {
        cmd.arg(a);
    }
    cmd.env("SIGIRO_QUACK_TOKEN", token)
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", "")
        .env("RUST_LOG", "error")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.output().expect("failed to run `sigiro query`")
}

const COMBINED_SQL: &str = "SELECT service_name, count(*) AS c, sum(duration) AS total_duration, \
     avg(duration) AS avg_duration, min(duration) AS min_duration, max(duration) AS max_duration \
     FROM sigiro_spans WHERE timestamp > '2000-01-01' GROUP BY service_name ORDER BY service_name";

#[test]
fn federated_query_rejects_short_quack_token_before_connecting() {
    let output = run_query(
        COMBINED_SQL,
        &["unreachable=127.0.0.1:1".to_string()],
        "abc",
        &[],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid SIGIRO_QUACK_TOKEN"),
        "expected token validation failure, got: {stderr}"
    );
    assert!(!stderr.contains("edge unreachable"));
}

/// One full scenario: spawn 2 edges, ingest overlapping per-service data,
/// run the federated query, assert every fold against ground truth, assert
/// count(DISTINCT ...) and a dead edge both fail loud, then shut down.
fn run_scenario(iteration: u16) -> Result<(), String> {
    let token = format!("e2e-fed-token-{iteration:04}");
    let base_a = 43100 + iteration * 20;
    let base_b = base_a + 10;
    let dead_port = base_a + 999;

    // Both children are launched before either is awaited, so the two ~6s
    // boots overlap. Their port blocks (base_a, base_a+10) and temp data dirs
    // are disjoint, so concurrent boot is safe.
    let edge_a = EdgeServer::spawn_booting("edgeA", base_a, &token);
    let edge_b = EdgeServer::spawn_booting("edgeB", base_b, &token);
    edge_a.wait_ready("edgeA");
    edge_b.wait_ready("edgeB");

    // Durations below are in ms at ingest; the stored `duration` column is
    // MICROSECONDS, so every ground-truth number is the ms value * 1000.
    // svcA: edge A contributes [10, 20]ms, edge B contributes [30]ms.
    //   ground truth (µs): count=3, sum=60000, avg=20000.0, min=10000, max=30000
    //   (edge A's own avg for svcA is 15000, edge B's is 30000 — avg-of-avgs
    //    would give 22500, which is WRONG; the correct fold is 20000.0)
    // svcB: edge A contributes [100]ms, edge B contributes [50, 150]ms.
    //   ground truth (µs): count=3, sum=300000, avg=100000.0, min=50000, max=150000
    //   (edge A's own avg for svcB is 100, edge B's is 100 too — this pair
    //    is deliberately chosen so avg-of-avgs would ACCIDENTALLY also give
    //    100.0; svcA is the pair that actually distinguishes correct folding
    //    from the avg-of-avgs bug)
    edge_a.ingest_durations("svcA", &[10, 20]);
    edge_a.ingest_durations("svcB", &[100]);
    edge_b.ingest_durations("svcA", &[30]);
    edge_b.ingest_durations("svcB", &[50, 150]);

    edge_a.wait_for_ingest("svcA", 2, &token);
    edge_a.wait_for_ingest("svcB", 1, &token);
    edge_b.wait_for_ingest("svcA", 1, &token);
    edge_b.wait_for_ingest("svcB", 2, &token);

    let edges = vec![edge_a.edge_arg("edgeA"), edge_b.edge_arg("edgeB")];

    // --- The money assertions: count/sum/min/max/avg fold + GROUP BY. ---
    let output = run_query(COMBINED_SQL, &edges, &token, &[]);
    if !output.status.success() {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "federated query failed unexpectedly: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).map_err(|e| {
        format!(
            "failed to parse federated query JSON output: {e}\nstdout={}",
            String::from_utf8_lossy(&output.stdout)
        )
    })?;
    println!(
        "--- iteration {iteration}: folded rows ---\n{}",
        serde_json::to_string_pretty(&rows).unwrap()
    );

    check_row(&rows, "svcA", 3, 60_000, 20_000.0, 10_000, 30_000)?;
    check_row(&rows, "svcB", 3, 300_000, 100_000.0, 50_000, 150_000)?;
    println!(
        "  [fold] svcA avg=20000.0 svcB avg=100000.0 — matches ground truth (NOT avg-of-avgs)"
    );

    // --- count(DISTINCT ...) must be refused, not silently mis-folded. ---
    let distinct_output = run_query(
        "SELECT count(DISTINCT service_name) AS c FROM sigiro_spans WHERE timestamp > '2000-01-01'",
        &edges,
        &token,
        &[],
    );
    if distinct_output.status.success() {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err("count(DISTINCT ...) unexpectedly SUCCEEDED — must fail loud".to_string());
    }
    let distinct_stderr = String::from_utf8_lossy(&distinct_output.stderr).to_string();
    if !distinct_stderr.to_lowercase().contains("distinct")
        && !distinct_stderr.to_lowercase().contains("federat")
    {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "count(DISTINCT ...) was rejected, but not with a clear reason: {distinct_stderr}"
        ));
    }
    println!(
        "  [count(DISTINCT)] rejected as expected: {}",
        distinct_stderr.trim()
    );

    // --- Top-N pushdown: `ORDER BY duration DESC LIMIT 3` must return the
    // GLOBAL top-3 rows, not each edge's own arbitrary top-3-by-scan-order.
    // Ground truth is engineered so a per-edge sortless `LIMIT 3` would drop
    // the true maximum on BOTH edges: each edge gets 5 spans with its
    // largest value inserted LAST (after 4 smaller ones), so an
    // ORDER-BY-less `LIMIT 3` (first 3 in scan/insertion order) would keep
    // only the 3 smallest per edge and lose the true max entirely.
    edge_a.ingest_durations("svcTopN", &[210, 220, 230, 240, 900]);
    edge_b.ingest_durations("svcTopN", &[215, 225, 235, 245, 950]);
    edge_a.wait_for_ingest("svcTopN", 5, &token);
    edge_b.wait_for_ingest("svcTopN", 5, &token);

    let topn_output = run_query(
        "SELECT trace_id, duration FROM sigiro_spans WHERE timestamp > '2000-01-01' \
         ORDER BY duration DESC LIMIT 3",
        &edges,
        &token,
        &[],
    );
    if !topn_output.status.success() {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "top-N federated query failed unexpectedly: stdout={} stderr={}",
            String::from_utf8_lossy(&topn_output.stdout),
            String::from_utf8_lossy(&topn_output.stderr)
        ));
    }
    let topn_rows: Vec<serde_json::Value> =
        serde_json::from_slice(&topn_output.stdout).map_err(|e| {
            format!(
                "failed to parse top-N federated query JSON output: {e}\nstdout={}",
                String::from_utf8_lossy(&topn_output.stdout)
            )
        })?;
    println!(
        "--- iteration {iteration}: top-N rows ---\n{}",
        serde_json::to_string_pretty(&topn_rows).unwrap()
    );
    let topn_durations = topn_rows
        .iter()
        .map(duration_of)
        .collect::<Result<Vec<i64>, String>>()?;
    // 950ms and 900ms are the true global maxima (one per edge); 245ms is
    // the global 3rd-largest (edge B's 2nd-largest). A per-edge-arbitrary-
    // rows bug can never produce this exact set, since neither maximum would
    // even leave its edge under the old sortless LIMIT. Values in µs.
    let expected_topn = vec![950_000, 900_000, 245_000];
    if topn_durations != expected_topn {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "top-N: got durations {topn_durations:?}, expected the GLOBAL top-3 \
             {expected_topn:?} — a per-edge-arbitrary-rows bug would return some other \
             combination (e.g. missing 950 and/or 900, the true maxima on each edge)"
        ));
    }
    println!(
        "  [top-N] ORDER BY duration DESC LIMIT 3 = {topn_durations:?} — matches global ground truth"
    );

    // --- Same top-N, but selecting ONLY trace_id (duration not in the
    // SELECT list). Before the fix, this failed at the coordinator fold
    // with a confusing binder error ("Referenced column duration not
    // found") because the per-edge partial only projected the SELECT list
    // (trace_id), never the ORDER BY key. Ground truth: the exact same 3
    // rows as the trace_id+duration query above (trace_id is unique per
    // span), but each row must carry ONLY the trace_id key — duration was
    // pulled into the per-edge partial only to sort by, and must be
    // dropped from the final output.
    let topn_ids_output = run_query(
        "SELECT trace_id FROM sigiro_spans WHERE timestamp > '2000-01-01' \
         ORDER BY duration DESC LIMIT 3",
        &edges,
        &token,
        &[],
    );
    if !topn_ids_output.status.success() {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "ORDER BY on a non-selected column failed unexpectedly: stdout={} stderr={}",
            String::from_utf8_lossy(&topn_ids_output.stdout),
            String::from_utf8_lossy(&topn_ids_output.stderr)
        ));
    }
    let topn_ids_rows: Vec<serde_json::Value> = serde_json::from_slice(&topn_ids_output.stdout)
        .map_err(|e| format!("failed to parse trace_id-only top-N JSON output: {e}"))?;
    if topn_ids_rows.len() != 3 {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "trace_id-only top-N: expected 3 rows, got {}: {topn_ids_rows:?}",
            topn_ids_rows.len()
        ));
    }
    for row in &topn_ids_rows {
        let obj = row
            .as_object()
            .ok_or_else(|| format!("row not an object: {row:?}"))?;
        if obj.len() != 1 || !obj.contains_key("trace_id") {
            edge_a.shutdown();
            edge_b.shutdown();
            return Err(format!(
                "trace_id-only top-N row must contain ONLY trace_id (the ORDER BY sort column, \
                 duration, must be dropped from the output): {row:?}"
            ));
        }
    }
    let expected_trace_ids: std::collections::HashSet<&str> = topn_rows
        .iter()
        .filter_map(|r| r.get("trace_id").and_then(|v| v.as_str()))
        .collect();
    let got_trace_ids: std::collections::HashSet<&str> = topn_ids_rows
        .iter()
        .filter_map(|r| r.get("trace_id").and_then(|v| v.as_str()))
        .collect();
    if got_trace_ids != expected_trace_ids {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "trace_id-only top-N returned a different row set than the trace_id+duration top-N: \
             got {got_trace_ids:?}, expected {expected_trace_ids:?}"
        ));
    }
    println!(
        "  [order-by non-selected column] SELECT trace_id ... ORDER BY duration (unselected) = \
         {got_trace_ids:?} — matches the trace_id+duration top-N, output carries only trace_id"
    );

    // --- Fractional WHERE literal: `duration > 15500.5` must be deparsed at
    // the correct decimal scale (a mis-scaled deparser emits `155005`, ten
    // times too large). Ground truth proves the exact boundary: the 15ms rows
    // (15000µs stored) must be EXCLUDED, the 16ms rows (16000µs) INCLUDED,
    // on BOTH edges.
    edge_a.ingest_durations("svcFrac", &[15, 16]);
    edge_b.ingest_durations("svcFrac", &[15, 16]);
    edge_a.wait_for_ingest("svcFrac", 2, &token);
    edge_b.wait_for_ingest("svcFrac", 2, &token);

    let frac_output = run_query(
        "SELECT duration FROM sigiro_spans WHERE timestamp > '2000-01-01' \
         AND service_name = 'svcFrac' AND duration > 15500.5",
        &edges,
        &token,
        &[],
    );
    if !frac_output.status.success() {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "fractional-predicate federated query failed unexpectedly: stdout={} stderr={}",
            String::from_utf8_lossy(&frac_output.stdout),
            String::from_utf8_lossy(&frac_output.stderr)
        ));
    }
    let frac_rows: Vec<serde_json::Value> = serde_json::from_slice(&frac_output.stdout)
        .map_err(|e| format!("failed to parse fractional-predicate JSON output: {e}"))?;
    let mut frac_durations = frac_rows
        .iter()
        .map(duration_of)
        .collect::<Result<Vec<i64>, String>>()?;
    frac_durations.sort_unstable();
    // A 10x-scale bug turns `duration > 15500.5` into `duration > 155005`,
    // which matches NEITHER 15000 NOR 16000 (expect zero rows instead of
    // two). A sign/off-by-one bug could instead wrongly include the 15000s.
    if frac_durations != vec![16_000, 16_000] {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "fractional predicate `duration > 15500.5`: got durations {frac_durations:?} across \
             both edges, expected exactly two duration=16000 rows (one per edge) with both \
             duration=15000 rows excluded"
        ));
    }
    println!(
        "  [fractional predicate] duration > 15500.5 excludes 15000, includes 16000 on both edges: {frac_durations:?}"
    );

    // --- Alias fold: a plain-column alias (`service_name AS svc`) and an
    // aggregate alias (`count(*) AS n`) must both come back under the
    // ALIASED JSON key, not the underlying column/function name. svcA's
    // total count (3) is already established above.
    let alias_output = run_query(
        "SELECT service_name AS svc, count(*) AS n FROM sigiro_spans \
         WHERE timestamp > '2000-01-01' AND service_name = 'svcA' GROUP BY service_name",
        &edges,
        &token,
        &[],
    );
    if !alias_output.status.success() {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "aliased SELECT failed unexpectedly: stdout={} stderr={}",
            String::from_utf8_lossy(&alias_output.stdout),
            String::from_utf8_lossy(&alias_output.stderr)
        ));
    }
    let alias_rows: Vec<serde_json::Value> = serde_json::from_slice(&alias_output.stdout)
        .map_err(|e| format!("failed to parse aliased-SELECT JSON output: {e}"))?;
    if alias_rows.len() != 1 {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "aliased SELECT: expected exactly 1 row for svcA, got {}: {alias_rows:?}",
            alias_rows.len()
        ));
    }
    let alias_row = &alias_rows[0];
    if alias_row.get("svc").and_then(|v| v.as_str()) != Some("svcA") {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "aliased SELECT: expected key \"svc\" == \"svcA\", got {alias_row:?} (plain-column \
             alias dropped — output key regressed to the underlying column name)"
        ));
    }
    if alias_row.get("n").and_then(serde_json::Value::as_i64) != Some(3) {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "aliased SELECT: expected key \"n\" == 3, got {alias_row:?} (aggregate alias dropped)"
        ));
    }
    println!("  [alias] service_name AS svc, count(*) AS n -> keys svc/n present: {alias_row:?}");

    // --- Zero-matching-rows edge + a GROUP BY key that exists on only ONE
    // edge: edge B never receives any `svcOnlyA` span, so its per-edge
    // GROUP BY partial for this WHERE-filtered query returns literally ZERO
    // rows (GROUP BY over zero matching input rows produces zero output
    // groups — unlike a bare aggregate, which would still emit one row of
    // NULLs). `fetch_edge` must treat that as a legitimate "contributed
    // nothing" edge, and the fold must be exactly edge A's numbers alone —
    // not NaN/NULL-corrupted by the absent edge.
    edge_a.ingest_durations("svcOnlyA", &[40, 60]);
    edge_a.wait_for_ingest("svcOnlyA", 2, &token);

    let only_a_output = run_query(
        "SELECT service_name, count(*) AS c, sum(duration) AS s, avg(duration) AS a, \
         min(duration) AS mn, max(duration) AS mx FROM sigiro_spans \
         WHERE timestamp > '2000-01-01' AND service_name = 'svcOnlyA' GROUP BY service_name",
        &edges,
        &token,
        &[],
    );
    if !only_a_output.status.success() {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "single-edge GROUP BY key query failed unexpectedly: stdout={} stderr={}",
            String::from_utf8_lossy(&only_a_output.stdout),
            String::from_utf8_lossy(&only_a_output.stderr)
        ));
    }
    let only_a_rows: Vec<serde_json::Value> = serde_json::from_slice(&only_a_output.stdout)
        .map_err(|e| format!("failed to parse single-edge GROUP BY JSON output: {e}"))?;
    if only_a_rows.len() != 1 {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "single-edge GROUP BY key: expected exactly 1 row (edge B contributes zero rows, \
             must not duplicate or drop the group), got {}: {only_a_rows:?}",
            only_a_rows.len()
        ));
    }
    let only_a_row = &only_a_rows[0];
    let only_a_i64 = |field: &str| -> Result<i64, String> {
        only_a_row
            .get(field)
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
            .ok_or_else(|| format!("svcOnlyA.{field} missing/non-numeric in {only_a_row:?}"))
    };
    let only_a_c = only_a_i64("c")?;
    let only_a_s = only_a_i64("s")?;
    let only_a_mn = only_a_i64("mn")?;
    let only_a_mx = only_a_i64("mx")?;
    let only_a_a = only_a_row
        .get("a")
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| format!("svcOnlyA.a missing/non-numeric in {only_a_row:?}"))?;
    if (only_a_c, only_a_s, only_a_mn, only_a_mx) != (2, 100_000, 40_000, 60_000)
        || (only_a_a - 50_000.0).abs() > 1e-9
    {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "single-edge GROUP BY key: got c={only_a_c} s={only_a_s} a={only_a_a} mn={only_a_mn} \
             mx={only_a_mx}, expected c=2 s=100000 a=50000.0 mn=40000 mx=60000 in µs (edge B's \
             zero-row contribution must not corrupt the fold)"
        ));
    }
    println!(
        "  [zero-row edge + single-edge group key] svcOnlyA (edge B contributes nothing): \
         c={only_a_c} s={only_a_s} a={only_a_a} mn={only_a_mn} mx={only_a_mx} — matches ground truth"
    );

    // --- A NULL GROUP BY key. Root spans (as created by `ingest_durations`,
    // which never sets an explicit parent) always have parent_span_id =
    // NULL — a real, naturally-occurring NULL group key.
    edge_a.ingest_durations("svcNullGroup", &[5, 7]);
    edge_b.ingest_durations("svcNullGroup", &[9]);
    edge_a.wait_for_ingest("svcNullGroup", 2, &token);
    edge_b.wait_for_ingest("svcNullGroup", 1, &token);

    let null_group_output = run_query(
        "SELECT parent_span_id, count(*) AS c, sum(duration) AS s FROM sigiro_spans \
         WHERE timestamp > '2000-01-01' AND service_name = 'svcNullGroup' \
         GROUP BY parent_span_id",
        &edges,
        &token,
        &[],
    );
    if !null_group_output.status.success() {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "NULL-group-key query failed unexpectedly: stdout={} stderr={}",
            String::from_utf8_lossy(&null_group_output.stdout),
            String::from_utf8_lossy(&null_group_output.stderr)
        ));
    }
    let null_group_rows: Vec<serde_json::Value> = serde_json::from_slice(&null_group_output.stdout)
        .map_err(|e| format!("failed to parse NULL-group-key JSON output: {e}"))?;
    if null_group_rows.len() != 1 {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "NULL-group-key: expected exactly 1 group (all root spans share parent_span_id = \
             NULL), got {}: {null_group_rows:?}",
            null_group_rows.len()
        ));
    }
    let null_group_row = &null_group_rows[0];
    if !null_group_row
        .get("parent_span_id")
        .is_some_and(serde_json::Value::is_null)
    {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "NULL-group-key: expected parent_span_id to fold to JSON null, got {null_group_row:?}"
        ));
    }
    let null_group_i64 = |field: &str| -> Result<i64, String> {
        null_group_row
            .get(field)
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
            .ok_or_else(|| {
                format!("NULL-group-key: {field} missing/non-numeric in {null_group_row:?}")
            })
    };
    let null_group_c = null_group_i64("c")?;
    let null_group_s = null_group_i64("s")?;
    if (null_group_c, null_group_s) != (3, 21_000) {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "NULL-group-key: got c={null_group_c} s={null_group_s}, expected c=3 s=21000µs \
             (2 rows on edge A [5,7]ms + 1 row on edge B [9]ms)"
        ));
    }
    println!(
        "  [NULL group key] parent_span_id folds to NULL across both edges: c={null_group_c} \
         s={null_group_s} — matches ground truth"
    );

    // --- A user LIMIT that exceeds SIGIRO_QUERY_ROW_CAP must fail loud,
    // never silently truncate to the cap.
    let limit_cap_output = run_query_env(
        "SELECT trace_id FROM sigiro_spans WHERE timestamp > '2000-01-01' LIMIT 1000",
        &edges,
        &token,
        &[],
        &[("SIGIRO_QUERY_ROW_CAP", "5")],
    );
    if limit_cap_output.status.success() {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(
            "LIMIT 1000 with SIGIRO_QUERY_ROW_CAP=5 unexpectedly SUCCEEDED — must fail loud \
             instead of silently truncating to the row cap"
                .to_string(),
        );
    }
    let limit_cap_stderr = String::from_utf8_lossy(&limit_cap_output.stderr).to_string();
    if !limit_cap_stderr.contains("LIMIT") || !limit_cap_stderr.contains("SIGIRO_QUERY_ROW_CAP") {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "LIMIT-exceeds-cap was rejected, but not with a clear reason naming LIMIT and \
             SIGIRO_QUERY_ROW_CAP: {limit_cap_stderr}"
        ));
    }
    println!(
        "  [LIMIT > row cap] LIMIT 1000 with SIGIRO_QUERY_ROW_CAP=5 rejected as expected: {}",
        limit_cap_stderr.trim()
    );

    // --- A qualified/struct column reference that isn't the FROM table's own
    // name or alias must fail loud, never silently rebind to an unrelated
    // top-level column. ---
    let bogus_output = run_query(
        "SELECT duration FROM sigiro_spans WHERE timestamp > '2000-01-01' AND bogus.duration > 0",
        &edges,
        &token,
        &[],
    );
    if bogus_output.status.success() {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(
            "query with a bogus qualified column (bogus.duration) unexpectedly SUCCEEDED — must \
             fail loud"
                .to_string(),
        );
    }
    let bogus_stderr = String::from_utf8_lossy(&bogus_output.stderr).to_string();
    if !bogus_stderr.to_lowercase().contains("bogus") {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "bogus qualified column was rejected, but not with a clear reason naming it: \
             {bogus_stderr}"
        ));
    }
    println!(
        "  [qualified column] bogus.duration rejected as expected: {}",
        bogus_stderr.trim()
    );

    // --- The same edge listed twice must fail loud (never silently double
    // every count/sum/avg). ---
    let dup_edges = vec![edge_a.edge_arg("edgeA"), edge_a.edge_arg("edgeA")];
    let dup_output = run_query(COMBINED_SQL, &dup_edges, &token, &[]);
    if dup_output.status.success() {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(
            "federated query against the same edge listed twice unexpectedly SUCCEEDED — must \
             fail loud instead of silently doubling every aggregate"
                .to_string(),
        );
    }
    let dup_stderr = String::from_utf8_lossy(&dup_output.stderr).to_string();
    if !dup_stderr.to_lowercase().contains("duplicate") {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "duplicate-edge query was rejected, but not with a clear reason: {dup_stderr}"
        ));
    }
    println!(
        "  [duplicate edge] rejected as expected: {}",
        dup_stderr.trim()
    );

    // --- A dead/unreachable edge must fail loud, naming it. ---
    let dead_edges = vec![
        edge_a.edge_arg("edgeA"),
        format!("deadEdge=127.0.0.1:{dead_port}"),
    ];
    let dead_output = run_query(COMBINED_SQL, &dead_edges, &token, &[]);
    if dead_output.status.success() {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(
            "query against a dead edge unexpectedly SUCCEEDED — must fail loud".to_string(),
        );
    }
    let dead_stderr = String::from_utf8_lossy(&dead_output.stderr).to_string();
    if !dead_stderr.contains("deadEdge") {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "dead-edge failure did not name the edge (deadEdge): {dead_stderr}"
        ));
    }
    println!(
        "  [dead edge] rejected and named as expected: {}",
        dead_stderr.trim()
    );

    edge_a.shutdown();
    edge_b.shutdown();
    Ok(())
}

/// Extract the `duration` field of a folded/passthrough row as an `i64`,
/// shared by the top-N and fractional-predicate assertions below.
fn duration_of(row: &serde_json::Value) -> Result<i64, String> {
    row.get("duration")
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
        .ok_or_else(|| format!("duration missing/non-numeric in row {row:?}"))
}

/// Seven arguments against the ceiling in `clippy.toml`, exempted on purpose:
/// five of them are the expected aggregates, and spelling them positionally is
/// what makes the call sites readable as assertions —
/// `check_row(&rows, "svcA", 3, 60_000, 20_000.0, 10_000, 30_000)`. An
/// `Expected { count, sum, avg, min, max }` struct would move the same five
/// numbers behind field names on a type with one call path, which reads as more
/// ceremony for less obvious tests.
#[allow(clippy::too_many_arguments)]
fn check_row(
    rows: &[serde_json::Value],
    service: &str,
    expect_count: i64,
    expect_sum: i64,
    expect_avg: f64,
    expect_min: i64,
    expect_max: i64,
) -> Result<(), String> {
    let row = rows
        .iter()
        .find(|r| r.get("service_name").and_then(|v| v.as_str()) == Some(service))
        .ok_or_else(|| format!("no folded row for service {service}; rows={rows:?}"))?;
    let get_i64 = |field: &str| -> Result<i64, String> {
        row.get(field)
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
            .ok_or_else(|| format!("{service}.{field} missing/non-numeric in {row:?}"))
    };
    let get_f64 = |field: &str| -> Result<f64, String> {
        row.get(field)
            .and_then(serde_json::Value::as_f64)
            .ok_or_else(|| format!("{service}.{field} missing/non-numeric in {row:?}"))
    };

    let count = get_i64("c")?;
    let sum = get_i64("total_duration")?;
    let avg = get_f64("avg_duration")?;
    let min = get_i64("min_duration")?;
    let max = get_i64("max_duration")?;

    if count != expect_count {
        return Err(format!("{service}: count={count}, expected {expect_count}"));
    }
    if sum != expect_sum {
        return Err(format!("{service}: sum={sum}, expected {expect_sum}"));
    }
    if (avg - expect_avg).abs() > 1e-9 {
        return Err(format!(
            "{service}: avg={avg}, expected EXACTLY {expect_avg} (a wrong-but-close value here \
             would indicate avg-of-avgs, not the correct sum/count fold)"
        ));
    }
    if min != expect_min {
        return Err(format!("{service}: min={min}, expected {expect_min}"));
    }
    if max != expect_max {
        return Err(format!("{service}: max={max}, expected {expect_max}"));
    }
    Ok(())
}

/// Single-pass smoke test — always runs as part of the standard `cargo test` gate.
#[test]
fn federated_query_single_iteration() {
    run_scenario(0).expect("federated query E2E iteration failed");
}

/// Repeats the full scenario 5+ times, as explicitly requested ("test it
/// again and again"), reporting a pass count. `#[ignore]`d by default
/// (spawns 4 full server processes + several child `sigiro query` processes
/// per iteration) — run explicitly with:
/// `cargo test --release --test federated_query_test -- --ignored --nocapture`
#[test]
#[ignore = "spawns many sigiro processes per iteration; run explicitly for the stability loop"]
fn federated_query_stability_loop() {
    const ITERATIONS: u16 = 5;
    let mut failures = Vec::new();

    for iteration in 0..ITERATIONS {
        match run_scenario(iteration) {
            Ok(()) => println!("iteration {iteration}: PASS"),
            Err(e) => {
                println!("iteration {iteration}: FAIL — {e}");
                failures.push((iteration, e));
            }
        }
    }

    let passed = ITERATIONS as usize - failures.len();
    println!("\n=== federated query stability loop: {passed}/{ITERATIONS} passed ===");
    for (iteration, error) in &failures {
        println!("  iteration {iteration} failed: {error}");
    }
    assert!(
        failures.is_empty(),
        "{}/{ITERATIONS} iterations failed: {failures:?}",
        failures.len()
    );
}
