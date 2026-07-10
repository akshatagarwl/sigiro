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
    fn spawn(name: &str, base: u16, token: &str) -> Self {
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
            .env("OTEL_EXPORTER_OTLP_ENDPOINT", "")
            .env("RUST_LOG", "info")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("failed to spawn edge {name}: {e}"));

        let server = Self {
            child,
            _data_dir: data_dir,
            http_port,
            otlp_http_port,
            quack_port,
        };
        server.wait_ready(name);
        server
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
    /// timing) so the resulting `duration` column (milliseconds — verified
    /// directly against `src/otlp.rs`: `dur_ns / 1_000_000`) is exact,
    /// deterministic ground truth.
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

    /// Poll row count for `service_name` until ingest is visible (flusher
    /// default-flushes every 1s) or a 10s deadline passes. Uses a direct
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
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sigiro"));
    cmd.arg("--output").arg("json").arg("query").arg(sql);
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
    cmd.output().expect("failed to run `sigiro query`")
}

const COMBINED_SQL: &str = "SELECT service_name, count(*) AS c, sum(duration) AS total_duration, \
     avg(duration) AS avg_duration, min(duration) AS min_duration, max(duration) AS max_duration \
     FROM sigiro_spans WHERE timestamp > '2000-01-01' GROUP BY service_name ORDER BY service_name";

/// One full scenario: spawn 2 edges, ingest overlapping per-service data,
/// run the federated query, assert every fold against ground truth, assert
/// count(DISTINCT ...) and a dead edge both fail loud, then shut down.
fn run_scenario(iteration: u16) -> Result<(), String> {
    let token = format!("e2e-fed-token-{iteration:04}");
    let base_a = 43100 + iteration * 20;
    let base_b = base_a + 10;
    let dead_port = base_a + 999;

    let edge_a = EdgeServer::spawn("edgeA", base_a, &token);
    let edge_b = EdgeServer::spawn("edgeB", base_b, &token);

    // svcA: edge A contributes [10, 20]ms, edge B contributes [30]ms.
    //   ground truth: count=3, sum=60, avg=20.0, min=10, max=30
    //   (edge A's own avg for svcA is 15, edge B's is 30 — avg-of-avgs would
    //    give 22.5, which is WRONG; the correct fold is 20.0)
    // svcB: edge A contributes [100]ms, edge B contributes [50, 150]ms.
    //   ground truth: count=3, sum=300, avg=100.0, min=50, max=150
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

    check_row(&rows, "svcA", 3, 60, 20.0, 10, 30)?;
    check_row(&rows, "svcB", 3, 300, 100.0, 50, 150)?;
    println!("  [fold] svcA avg=20.0 svcB avg=100.0 — matches ground truth (NOT avg-of-avgs)");

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
    // 950 and 900 are the true global maxima (one per edge); 245 is the
    // global 3rd-largest (edge B's 2nd-largest). A per-edge-arbitrary-rows
    // bug can never produce this exact set, since neither 950 nor 900 would
    // even leave their edge under the old sortless LIMIT.
    let expected_topn = vec![950, 900, 245];
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

    // --- Fractional WHERE literal: `duration > 15.5` must be deparsed at the
    // correct decimal scale (a mis-scaled deparser emits `155`, ten times too
    // large). Ground truth proves the exact boundary: a duration=15 row must
    // be EXCLUDED, a duration=16 row must be INCLUDED, on BOTH edges.
    edge_a.ingest_durations("svcFrac", &[15, 16]);
    edge_b.ingest_durations("svcFrac", &[15, 16]);
    edge_a.wait_for_ingest("svcFrac", 2, &token);
    edge_b.wait_for_ingest("svcFrac", 2, &token);

    let frac_output = run_query(
        "SELECT duration FROM sigiro_spans WHERE timestamp > '2000-01-01' \
         AND service_name = 'svcFrac' AND duration > 15.5",
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
    // A 10x-scale bug turns `duration > 15.5` into `duration > 155`, which
    // matches NEITHER 15 NOR 16 (expect zero rows instead of two). A
    // sign/off-by-one bug could instead wrongly include the 15s.
    if frac_durations != vec![16, 16] {
        edge_a.shutdown();
        edge_b.shutdown();
        return Err(format!(
            "fractional predicate `duration > 15.5`: got durations {frac_durations:?} across \
             both edges, expected exactly two duration=16 rows (one per edge) with both \
             duration=15 rows excluded"
        ));
    }
    println!(
        "  [fractional predicate] duration > 15.5 excludes 15, includes 16 on both edges: {frac_durations:?}"
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
